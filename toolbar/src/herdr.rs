//! Everything that talks to Herdr: the environment it hands a plugin, the
//! `toggle` action, and the control socket the `select` mode writes to.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::app::ControlCmd;

pub const PLUGIN_ID: &str = "jefelabs.toolbar";
pub const PANE_ENTRYPOINT: &str = "toolbar";

/// Herdr variables passed through to hosted tools so they can drive Herdr themselves.
const PASSTHROUGH: &[&str] = &[
    "HERDR_SOCKET_PATH",
    "HERDR_BIN_PATH",
    "HERDR_ENV",
    "HERDR_WORKSPACE_ID",
    "HERDR_TAB_ID",
    "HERDR_PANE_ID",
    // so `herdr-toolbar trust` works from a shell hosted in the toolbar
    "HERDR_PLUGIN_ID",
    "HERDR_PLUGIN_CONFIG_DIR",
    "HERDR_PLUGIN_STATE_DIR",
    "HERDR_PLUGIN_CONTEXT_JSON",
];

#[derive(Debug, Clone)]
pub struct HerdrContext {
    pub bin: PathBuf,
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    /// The pane a split should be cut from: `focused_pane_id` from the action
    /// context, else `HERDR_PANE_ID`.
    pub target_pane_id: Option<String>,
    /// The workspace's directory from `HERDR_PLUGIN_CONTEXT_JSON.workspace_cwd`.
    pub workspace_cwd: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Herdr variables to hand to hosted tools.
    pub inherit: Vec<(String, String)>,
    /// Extra variables for `herdr` CLI invocations (tests use this for the fake).
    pub extra_env: Vec<(String, String)>,
}

#[derive(serde::Deserialize, Default)]
struct ContextJson {
    workspace_cwd: Option<PathBuf>,
    focused_pane_id: Option<String>,
}

impl HerdrContext {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// Build from a variable lookup; `from_env` is this over the real environment.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let bin = get("HERDR_BIN_PATH").context("HERDR_BIN_PATH is not set: herdr-toolbar must run inside a Herdr plugin pane or action")?;
        let config_dir =
            get("HERDR_PLUGIN_CONFIG_DIR").context("HERDR_PLUGIN_CONFIG_DIR is not set")?;
        let state_dir =
            get("HERDR_PLUGIN_STATE_DIR").context("HERDR_PLUGIN_STATE_DIR is not set")?;
        let ctx: ContextJson = match get("HERDR_PLUGIN_CONTEXT_JSON") {
            Some(json) => serde_json::from_str(&json)
                .context("HERDR_PLUGIN_CONTEXT_JSON is not valid JSON")?,
            None => ContextJson::default(),
        };
        let workspace_cwd = ctx
            .workspace_cwd
            .or_else(|| std::env::current_dir().ok())
            .context("no workspace_cwd in HERDR_PLUGIN_CONTEXT_JSON and no current directory")?;
        let inherit = PASSTHROUGH
            .iter()
            .filter_map(|k| get(k).map(|v| (k.to_string(), v)))
            .collect();
        let pane_id = get("HERDR_PANE_ID");
        Ok(Self {
            bin: PathBuf::from(bin),
            workspace_id: get("HERDR_WORKSPACE_ID"),
            target_pane_id: ctx.focused_pane_id.or_else(|| pane_id.clone()),
            pane_id,
            workspace_cwd,
            config_dir: PathBuf::from(config_dir),
            state_dir: PathBuf::from(state_dir),
            inherit,
            extra_env: Vec::new(),
        })
    }

    fn herdr(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(args);
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        cmd.output()
            .with_context(|| format!("running {} {}", self.bin.display(), args.join(" ")))
    }

    fn ws(&self) -> &str {
        self.workspace_id.as_deref().unwrap_or("default")
    }
}

/// `<state_dir>/<workspace>.pane`: the toolbar pane id for this workspace.
pub fn pane_file(ctx: &HerdrContext) -> PathBuf {
    ctx.state_dir.join(format!("{}.pane", ctx.ws()))
}

/// `<state_dir>/<workspace>.sock`: the running toolbar's control socket.
pub fn socket_path(ctx: &HerdrContext) -> PathBuf {
    ctx.state_dir.join(format!("{}.sock", ctx.ws()))
}

/// `<state_dir>/trusted.json`: the workspace-config allowlist.
pub fn trust_file(ctx: &HerdrContext) -> PathBuf {
    ctx.state_dir.join("trusted.json")
}

/// Close the workspace's toolbar pane if one is recorded and alive, otherwise open one.
pub fn toggle(ctx: &HerdrContext) -> anyhow::Result<()> {
    std::fs::create_dir_all(&ctx.state_dir)?;
    let file = pane_file(ctx);
    if let Ok(recorded) = std::fs::read_to_string(&file) {
        let id = recorded.trim();
        if !id.is_empty() {
            let out = ctx.herdr(&["pane", "get", id])?;
            let text = String::from_utf8_lossy(&out.stdout);
            // Pane ids may be reused after a Herdr restart: close only a pane
            // that still looks like ours.
            let is_toolbar = text.contains(PLUGIN_ID) || text.contains("\"label\":\"Toolbar\"");
            if out.status.success() && is_toolbar {
                let closed = ctx.herdr(&["pane", "close", id])?;
                if !closed.status.success() {
                    bail!(
                        "herdr pane close {id} failed: {}",
                        String::from_utf8_lossy(&closed.stderr).trim()
                    );
                }
                let _ = std::fs::remove_file(&file);
                let _ = std::fs::remove_file(socket_path(ctx));
                return Ok(());
            }
        }
    }
    let cwd = ctx.workspace_cwd.display().to_string();
    let target = ctx.target_pane_id.as_deref().context(
        "no pane to split: the action context has no focused_pane_id and HERDR_PANE_ID is unset",
    )?;
    // No `--workspace`: on Herdr 0.9.5 it makes the server ignore the target pane
    // and reject the split. The target pane already pins the workspace.
    let args = [
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        PANE_ENTRYPOINT,
        "--placement",
        "split",
        "--direction",
        "right",
        "--target-pane",
        target,
        "--cwd",
        &cwd,
    ];
    let out = ctx.herdr(&args)?;
    if !out.status.success() {
        bail!(
            "herdr plugin pane open failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pane_id = serde_json::from_str::<serde_json::Value>(stdout.trim())
        .ok()
        .and_then(|v| {
            ["/result/plugin_pane/pane/pane_id", "/result/pane/pane_id"]
                .iter()
                .find_map(|ptr| v.pointer(ptr).and_then(|p| p.as_str()).map(str::to_string))
        });
    if let Some(id) = pane_id {
        std::fs::write(&file, format!("{id}\n"))?;
    }
    Ok(())
}

/// Tell the running toolbar to switch tools. `tool` is an id or a 1-based index.
pub fn select(ctx: &HerdrContext, tool: &str) -> anyhow::Result<()> {
    let path = socket_path(ctx);
    let mut stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "toolbar is not open in this workspace (no socket at {})",
            path.display()
        )
    })?;
    stream.write_all(format!("select {tool}\n").as_bytes())?;
    Ok(())
}

/// The toolbar's end of the control socket. Non-blocking; `poll` each loop.
pub struct Control {
    listener: UnixListener,
    path: PathBuf,
}

impl Control {
    pub fn bind(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let _ = std::fs::remove_file(path);
        let listener =
            UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn poll(&self) -> Vec<ControlCmd> {
        let mut cmds = Vec::new();
        while let Ok((mut stream, _)) = self.listener.accept() {
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            let mut text = String::new();
            let _ = stream.read_to_string(&mut text);
            for line in text.lines() {
                if let Some(tool) = line.strip_prefix("select ") {
                    cmds.push(ControlCmd::Select(tool.trim().to_string()));
                }
            }
        }
        cmds
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
