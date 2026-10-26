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
    /// The tab the toolbar is in (`HERDR_TAB_ID`).
    pub tab_id: Option<String>,
    /// The pane a split should be cut from: `focused_pane_id` from the action
    /// context, else `HERDR_PANE_ID`.
    pub target_pane_id: Option<String>,
    /// The workspace's directory from `HERDR_PLUGIN_CONTEXT_JSON.workspace_cwd`.
    pub workspace_cwd: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Herdr's API socket: `HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock`.
    pub socket: PathBuf,
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
        let socket = get("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(get("HOME").unwrap_or_default()).join(".config/herdr/herdr.sock")
            });
        Ok(Self {
            bin: PathBuf::from(bin),
            workspace_id: get("HERDR_WORKSPACE_ID"),
            tab_id: get("HERDR_TAB_ID"),
            socket,
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
        fit_width(ctx, &id);
    }
    Ok(())
}

/// The toolbar should take about a third of the tab. Herdr opens a split at
/// roughly half, so measure the new pane and move the divider right by the
/// difference (`pane resize --direction right` shrinks a right-hand pane).
/// Best effort: any failure leaves Herdr's default width.
const TARGET_WIDTH_FRACTION: f64 = 0.33;

fn fit_width(ctx: &HerdrContext, pane_id: &str) {
    let Ok(out) = ctx.herdr(&["pane", "layout", "--pane", pane_id]) else {
        return;
    };
    let Ok(v) =
        serde_json::from_str::<serde_json::Value>(String::from_utf8_lossy(&out.stdout).trim())
    else {
        return;
    };
    let layout = &v["result"]["layout"];
    let total = layout["area"]["width"].as_f64().unwrap_or(0.0);
    let mine = layout["panes"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["pane_id"].as_str() == Some(pane_id)))
        .and_then(|p| p["rect"]["width"].as_f64())
        .unwrap_or(0.0);
    if total <= 0.0 || mine <= 0.0 {
        return;
    }
    let delta = mine / total - TARGET_WIDTH_FRACTION;
    if delta > 0.02 {
        let amount = format!("{:.2}", delta);
        let _ = ctx.herdr(&[
            "pane",
            "resize",
            "--pane",
            pane_id,
            "--direction",
            "right",
            "--amount",
            &amount,
        ]);
    }
}

/// Move focus into the toolbar, or back out to the pane on its left when the
/// toolbar already has it. The toolbar is always opened as a right split of
/// the pane that was focused, so left/right neighbours are the right ones.
pub fn focus(ctx: &HerdrContext) -> anyhow::Result<()> {
    let recorded = std::fs::read_to_string(pane_file(ctx)).unwrap_or_default();
    let toolbar = recorded.trim();
    if toolbar.is_empty() {
        bail!("toolbar is not open in this workspace");
    }
    let focused = ctx.target_pane_id.as_deref().unwrap_or("");
    let (pane, direction) = if focused == toolbar {
        (toolbar, "left")
    } else {
        (focused, "right")
    };
    if pane.is_empty() {
        bail!("no focused pane in the action context");
    }
    let out = ctx.herdr(&["pane", "focus", "--pane", pane, "--direction", direction])?;
    if !out.status.success() {
        bail!(
            "herdr pane focus failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
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

/// `<state_dir>/<workspace>.<tool>.pane`: the pane a pane tool was opened in.
pub fn launched_pane_file(ctx: &HerdrContext, tool_id: &str) -> PathBuf {
    let safe: String = tool_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    ctx.state_dir.join(format!("{}.{safe}.pane", ctx.ws()))
}

/// Open `tool` in a Herdr pane beside the work pane, or focus the pane it
/// was opened in before if that pane is still there. Returns the pane id.
///
/// The new pane is a right split of the toolbar's left neighbour (the work
/// pane), else of the focused pane; the tool's command runs in it. This is
/// how terminal-browser opens itself in Herdr, so a browser gets a real pane.
pub fn open_in_pane(
    ctx: &HerdrContext,
    tool: &crate::config::ToolConfig,
) -> anyhow::Result<String> {
    if tool.command.is_empty() {
        bail!("{} has no command", tool.name);
    }
    let file = launched_pane_file(ctx, &tool.id);
    if let Ok(recorded) = std::fs::read_to_string(&file) {
        let id = recorded.trim();
        if !id.is_empty() {
            let out = ctx.herdr(&["pane", "get", id])?;
            if out.status.success() {
                focus_pane(ctx, id, &out.stdout)?;
                return Ok(id.to_string());
            }
        }
    }
    let from = ctx
        .pane_id
        .as_deref()
        .and_then(|toolbar| left_neighbour(ctx, toolbar))
        .or_else(|| ctx.target_pane_id.clone())
        .or_else(|| ctx.pane_id.clone())
        .context("no pane to open beside")?;
    let cwd = match tool.cwd.as_deref() {
        Some(c) => expand_home(c),
        None => ctx.workspace_cwd.clone(),
    };
    let cwd = cwd.display().to_string();
    let mut args: Vec<String> = [
        "pane",
        "split",
        "--pane",
        &from,
        "--direction",
        "right",
        "--focus",
        "--cwd",
        &cwd,
    ]
    .iter()
    .map(|a| a.to_string())
    .collect();
    for (k, v) in &tool.env {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = ctx.herdr(&refs)?;
    if !out.status.success() {
        bail!(
            "herdr pane split failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let id = json_str(&out.stdout, "/result/pane/pane_id")
        .context("herdr pane split gave no pane id")?;
    let run = ctx.herdr(&["pane", "run", &id, &shell_quote(&tool.command)])?;
    if !run.status.success() {
        bail!(
            "herdr pane run failed: {}",
            String::from_utf8_lossy(&run.stderr).trim()
        );
    }
    std::fs::create_dir_all(&ctx.state_dir)?;
    std::fs::write(&file, format!("{id}\n"))?;
    Ok(id)
}

/// The pane on `pane`'s left, if there is one.
fn left_neighbour(ctx: &HerdrContext, pane: &str) -> Option<String> {
    let out = ctx
        .herdr(&["pane", "neighbor", "--pane", pane, "--direction", "left"])
        .ok()?;
    if !out.status.success() {
        return None;
    }
    json_str(&out.stdout, "/result/neighbor/neighbor_pane_id")
}

/// Bring `pane` to the front: its tab first when it is in another one, then
/// the pane itself through Herdr's socket (the CLI only focuses neighbours).
fn focus_pane(ctx: &HerdrContext, pane: &str, get_stdout: &[u8]) -> anyhow::Result<()> {
    if let Some(tab) = json_str(get_stdout, "/result/pane/tab_id")
        && ctx.tab_id.as_deref() != Some(tab.as_str())
    {
        let _ = ctx.herdr(&["tab", "focus", &tab]);
    }
    socket_call(ctx, "pane.focus", serde_json::json!({ "pane_id": pane }))?;
    Ok(())
}

/// One request on Herdr's API socket: a JSON line out, a JSON line back.
fn socket_call(
    ctx: &HerdrContext,
    method: &str,
    params: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    use std::io::BufRead;
    let mut stream = UnixStream::connect(&ctx.socket)
        .with_context(|| format!("connecting to Herdr at {}", ctx.socket.display()))?;
    let limit = Some(std::time::Duration::from_secs(5));
    stream.set_read_timeout(limit)?;
    stream.set_write_timeout(limit)?;
    let request = serde_json::json!({ "id": "herdr-toolbar", "method": method, "params": params });
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line)?;
    let reply: serde_json::Value =
        serde_json::from_str(line.trim()).context("Herdr's reply is not JSON")?;
    if let Some(err) = reply.get("error") {
        bail!(
            "Herdr refused {method}: {}",
            err.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("error")
        );
    }
    Ok(reply.get("result").cloned().unwrap_or_default())
}

fn json_str(bytes: &[u8], pointer: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(String::from_utf8_lossy(bytes).trim())
        .ok()?
        .pointer(pointer)?
        .as_str()
        .map(str::to_string)
}

/// `~` and `~/…` under `$HOME`; anything else as written.
fn expand_home(path: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    if path == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = path.strip_prefix("~/") {
        Path::new(&home).join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// A command line for a shell: every argument in single quotes.
pub fn shell_quote(args: &[String]) -> String {
    args.iter()
        .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}
