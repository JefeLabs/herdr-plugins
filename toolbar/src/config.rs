//! Tool configuration: a global `tools.toml` merged with an optional
//! `.herdr-toolbar.toml` at the workspace root.

use std::collections::BTreeMap;
use std::path::Path;

use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const WORKSPACE_FILE: &str = ".herdr-toolbar.toml";
pub const DEFAULT_PREFIX: &str = "ctrl-]";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ToolConfig {
    pub id: String,
    pub name: String,
    pub icon: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Where the tool runs: in the toolbar's slot (the default), or in a
    /// Herdr pane opened beside the work pane (`open = "pane"`).
    #[serde(default)]
    pub open: Open,
    /// Experimental: pass the tool's Kitty graphics through to Herdr, so a
    /// slot can show images (terminal-browser pages). Off by default.
    #[serde(default)]
    pub graphics: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Open {
    #[default]
    Slot,
    Pane,
}

/// One file, as written.
#[derive(Debug, Default, Deserialize)]
pub struct RawConfig {
    pub prefix: Option<String>,
    pub order: Option<Vec<String>>,
    #[serde(default)]
    pub tool: Vec<ToolConfig>,
}

/// The merged, ordered result the toolbar runs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub prefix: String,
    pub tools: Vec<ToolConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_PREFIX.to_string(),
            tools: Vec::new(),
        }
    }
}

pub fn parse(text: &str) -> anyhow::Result<RawConfig> {
    let raw: RawConfig =
        toml::from_str(text).map_err(|e| anyhow::anyhow!("{}", describe(&e, text)))?;
    for t in &raw.tool {
        if t.command.is_empty() {
            bail!("tool `{}` has no command", t.id);
        }
        if t.id.is_empty() {
            bail!("a tool has an empty id");
        }
    }
    Ok(raw)
}

/// toml's error Display already carries a line/column header; make sure
/// the message says "line N" so the UI can show it tersely.
fn describe(e: &toml::de::Error, text: &str) -> String {
    match e.span() {
        Some(span) => {
            let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
            format!("line {line}: {}", e.message())
        }
        None => e.message().to_string(),
    }
}

/// Environment variables a workspace file may never set: they change how
/// the tool's binary is loaded or found, so a cloned repo must not control them.
const DENIED_WORKSPACE_ENV_PREFIXES: &[&str] = &["LD_", "DYLD_"];
const DENIED_WORKSPACE_ENV: &[&str] = &[
    "PATH",
    "SHELL",
    "HOME",
    "IFS",
    "BASH_ENV",
    "ENV",
    "PERL5OPT",
    "PYTHONPATH",
    "NODE_OPTIONS",
    "RUBYOPT",
];

fn env_is_denied(name: &str) -> bool {
    DENIED_WORKSPACE_ENV.contains(&name)
        || DENIED_WORKSPACE_ENV_PREFIXES
            .iter()
            .any(|p| name.starts_with(p))
}

/// A direnv-style allowlist: a workspace file is honoured only while its
/// path and content hash are recorded here. Stored as JSON in the plugin
/// state dir.
#[derive(Debug, Clone)]
pub struct Trust {
    path: PathBuf,
}

impl Trust {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn hash(text: &str) -> String {
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }

    /// The key is the path exactly as the workspace root names it, never
    /// canonicalised: a symlink must not borrow another file's trust.
    fn key(file: &Path) -> String {
        file.display().to_string()
    }

    fn is_regular_file(file: &Path) -> bool {
        std::fs::symlink_metadata(file)
            .map(|m| m.file_type().is_file())
            .unwrap_or(false)
    }

    pub fn is_trusted(&self, file: &Path, text: &str) -> bool {
        Self::is_regular_file(file) && self.read().get(&Self::key(file)) == Some(&Self::hash(text))
    }

    /// Record the file's current content as trusted. Refuses symlinks.
    pub fn allow(&self, file: &Path) -> anyhow::Result<()> {
        if !Self::is_regular_file(file) {
            bail!(
                "{} is not a regular file (symlinks are never trusted)",
                file.display()
            );
        }
        let text =
            std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        let mut map = self.read();
        map.insert(Self::key(file), Self::hash(&text));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&map)?)
            .with_context(|| format!("writing {}", self.path.display()))
    }
}

/// Result of a load: the merged config and, if a workspace file was present
/// but not trusted, its path so the UI can say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub config: Config,
    pub ignored_untrusted: Option<PathBuf>,
}

/// Read the global file (required to exist, may be empty) and, if present and
/// trusted, `<workspace_root>/.herdr-toolbar.toml`; merge by id; apply order.
pub fn load_merged(global: &Path, workspace_root: &Path, trust: &Trust) -> anyhow::Result<Loaded> {
    let global_text =
        std::fs::read_to_string(global).with_context(|| format!("reading {}", global.display()))?;
    let global_raw = parse(&global_text).with_context(|| global.display().to_string())?;

    let ws_path = workspace_root.join(WORKSPACE_FILE);
    let mut ignored_untrusted = None;
    let ws_raw = if ws_path.is_file() {
        let text = std::fs::read_to_string(&ws_path)
            .with_context(|| format!("reading {}", ws_path.display()))?;
        if trust.is_trusted(&ws_path, &text) {
            let mut raw = parse(&text).with_context(|| ws_path.display().to_string())?;
            for t in &mut raw.tool {
                t.env.retain(|k, _| !env_is_denied(k));
            }
            Some(raw)
        } else {
            ignored_untrusted = Some(ws_path);
            None
        }
    } else {
        None
    };

    Ok(Loaded {
        config: merge(global_raw, ws_raw)?,
        ignored_untrusted,
    })
}

pub fn merge(global: RawConfig, workspace: Option<RawConfig>) -> anyhow::Result<Config> {
    let mut tools = global.tool;
    let mut prefix = global.prefix;
    let mut order = global.order;

    if let Some(ws) = workspace {
        for t in ws.tool {
            match tools.iter_mut().find(|g| g.id == t.id) {
                Some(slot) => *slot = t,
                None => tools.push(t),
            }
        }
        if ws.prefix.is_some() {
            prefix = ws.prefix;
        }
        if ws.order.is_some() {
            order = ws.order;
        }
    }

    if let Some(order) = order {
        let mut ordered = Vec::with_capacity(tools.len());
        for id in &order {
            match tools.iter().position(|t| &t.id == id) {
                Some(i) => ordered.push(tools.remove(i)),
                None => bail!("`order` names unknown tool `{id}`"),
            }
        }
        ordered.append(&mut tools);
        tools = ordered;
    }

    for t in &mut tools {
        if let Some(cwd) = &t.cwd {
            t.cwd = Some(shellexpand::tilde(cwd).into_owned());
        }
    }

    Ok(Config {
        prefix: prefix.unwrap_or_else(|| DEFAULT_PREFIX.to_string()),
        tools,
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_tool_opens_in_the_slot_unless_it_asks_for_a_pane() {
        let raw = parse(
            "[[tool]]\nid = \"files\"\nname = \"Files\"\nicon = \"x\"\ncommand = [\"yazi\"]\n\n[[tool]]\nid = \"browser\"\nname = \"Browser\"\nicon = \"o\"\ncommand = [\"terminal-browser\", \"open\"]\nopen = \"pane\"\n",
        )
        .unwrap();
        assert_eq!(raw.tool[0].open, Open::Slot);
        assert_eq!(raw.tool[1].open, Open::Pane);
        assert!(
            parse("[[tool]]\nid = \"x\"\nname = \"X\"\nicon = \"x\"\ncommand = [\"x\"]\nopen = \"window\"\n").is_err(),
            "only slot or pane"
        );
    }
    use super::*;
    use std::fs;

    const GLOBAL: &str = include_str!("../tools.example.toml");

    fn trusting(t: &tempfile::TempDir, w: &std::path::Path) -> Trust {
        let trust = Trust::new(t.path().join("trusted.json"));
        let f = w.join(WORKSPACE_FILE);
        if f.is_file() {
            trust.allow(&f).unwrap();
        }
        trust
    }

    fn dirs(
        global: &str,
        workspace: Option<&str>,
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path().join("tools.toml");
        fs::write(&g, global).unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir(&ws).unwrap();
        if let Some(w) = workspace {
            fs::write(ws.join(".herdr-toolbar.toml"), w).unwrap();
        }
        (tmp, g, ws)
    }

    #[test]
    fn parses_the_spec_example() {
        let raw = parse(GLOBAL).unwrap();
        assert_eq!(raw.tool.len(), 6);
        assert_eq!(raw.tool[2].id, "db");
        assert_eq!(raw.tool[4].env["JIRA_PROJECT"], "SKOOL");
        let want: Vec<String> = ["issues", "capabilities", "db", "files", "jira", "browser"]
            .map(String::from)
            .to_vec();
        assert_eq!(raw.order.as_deref(), Some(&want[..]));
        assert_eq!(raw.prefix.as_deref(), Some("ctrl-]"));
        assert_eq!(raw.tool[5].id, "browser");
        assert_eq!(raw.tool[5].open, Open::Pane, "the browser opens in a pane");
    }

    #[test]
    fn workspace_entry_replaces_global_by_id_and_keeps_position() {
        let ws = r#"
[[tool]]
id = "db"
name = "Slot DB"
icon = "▤"
command = ["lazysql", "postgres://localhost:5433/x"]
"#;
        let (t, g, w) = dirs(GLOBAL, Some(ws));
        let cfg = load_merged(&g, &w, &trusting(&t, &w)).unwrap().config;
        assert_eq!(cfg.tools.len(), 6);
        assert_eq!(cfg.tools[2].id, "db");
        assert_eq!(cfg.tools[2].name, "Slot DB");
        assert_eq!(cfg.tools[2].command[1], "postgres://localhost:5433/x");
    }

    #[test]
    fn workspace_order_wins() {
        let ws = "order = [\"files\", \"db\"]\n";
        let (t, g, w) = dirs(GLOBAL, Some(ws));
        let cfg = load_merged(&g, &w, &trusting(&t, &w)).unwrap().config;
        let ids: Vec<&str> = cfg.tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            ["files", "db", "issues", "capabilities", "jira", "browser"]
        );
    }

    #[test]
    fn missing_workspace_file_is_fine() {
        let (t, g, w) = dirs(GLOBAL, None);
        let cfg = load_merged(&g, &w, &trusting(&t, &w)).unwrap().config;
        assert_eq!(cfg.tools.len(), 6);
        assert_eq!(cfg.prefix, "ctrl-]");
    }

    #[test]
    fn syntax_error_reports_line() {
        let e = parse("[[tool]\nid=1").unwrap_err().to_string();
        assert!(e.contains("line 1"), "got: {e}");
    }

    #[test]
    fn tool_without_command_is_an_error_naming_the_id() {
        let e = parse("[[tool]]\nid = \"todo\"\nname = \"Todo\"\nicon = \"x\"\ncommand = []\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("tool `todo` has no command"), "got: {e}");
    }

    #[test]
    fn unknown_id_in_order_is_an_error() {
        let ws = "order = [\"nope\"]\n";
        let (t, g, w) = dirs(GLOBAL, Some(ws));
        let e = load_merged(&g, &w, &trusting(&t, &w))
            .unwrap_err()
            .to_string();
        assert!(e.contains("`nope`"), "got: {e}");
    }

    #[test]
    fn cwd_tilde_is_expanded() {
        let (t, g, w) = dirs(GLOBAL, None);
        let cfg = load_merged(&g, &w, &trusting(&t, &w)).unwrap().config;
        let cwd = cfg.tools[2].cwd.as_deref().unwrap();
        assert!(!cwd.starts_with('~'), "got: {cwd}");
    }

    #[test]
    fn untrusted_workspace_file_is_ignored_and_reported() {
        let ws = "[[tool]]\nid = \"evil\"\nname = \"E\"\nicon = \"!\"\ncommand = [\"rm\", \"-rf\", \"/\"]\n";
        let (t, g, w) = dirs(GLOBAL, Some(ws));
        let trust = Trust::new(t.path().join("trusted.json"));
        let loaded = load_merged(&g, &w, &trust).unwrap();
        assert_eq!(
            loaded.config.tools.len(),
            6,
            "workspace tool must not be merged"
        );
        assert_eq!(loaded.ignored_untrusted, Some(w.join(WORKSPACE_FILE)));
    }

    #[test]
    fn trusted_workspace_file_is_merged_until_it_changes() {
        let ws = "[[tool]]\nid = \"extra\"\nname = \"X\"\nicon = \"x\"\ncommand = [\"true\"]\n";
        let (t, g, w) = dirs(GLOBAL, Some(ws));
        let trust = Trust::new(t.path().join("trusted.json"));
        trust.allow(&w.join(WORKSPACE_FILE)).unwrap();
        let loaded = load_merged(&g, &w, &trust).unwrap();
        assert_eq!(loaded.config.tools.len(), 7);
        assert_eq!(loaded.ignored_untrusted, None);
        fs::write(w.join(WORKSPACE_FILE), format!("{ws}# changed\n")).unwrap();
        let loaded = load_merged(&g, &w, &trust).unwrap();
        assert_eq!(
            loaded.config.tools.len(),
            6,
            "a changed file needs re-trusting"
        );
        assert!(loaded.ignored_untrusted.is_some());
    }

    #[test]
    fn loader_env_vars_from_workspace_are_dropped() {
        let ws = "[[tool]]\nid = \"db\"\nname = \"D\"\nicon = \"d\"\ncommand = [\"lazysql\"]\nenv = { DYLD_INSERT_LIBRARIES = \"/x.dylib\", LD_PRELOAD = \"/x.so\", PATH = \"/evil\", PGHOST = \"localhost\" }\n";
        let (t, g, w) = dirs(GLOBAL, Some(ws));
        let trust = Trust::new(t.path().join("trusted.json"));
        trust.allow(&w.join(WORKSPACE_FILE)).unwrap();
        let cfg = load_merged(&g, &w, &trust).unwrap().config;
        let env = &cfg.tools[2].env;
        assert_eq!(env.get("PGHOST").map(String::as_str), Some("localhost"));
        assert!(!env.contains_key("DYLD_INSERT_LIBRARIES"));
        assert!(!env.contains_key("LD_PRELOAD"));
        assert!(!env.contains_key("PATH"));
    }

    #[test]
    fn symlinked_workspace_file_is_never_trusted() {
        // A repo could ship `.herdr-toolbar.toml -> /some/already-trusted/file`
        // so its key canonicalises to a trusted path. Refuse symlinks outright.
        let ws = "[[tool]]\nid = \"extra\"\nname = \"X\"\nicon = \"x\"\ncommand = [\"true\"]\n";
        let (t, g, w) = dirs(GLOBAL, None);
        let real = t.path().join("elsewhere.toml");
        fs::write(&real, ws).unwrap();
        let trust = Trust::new(t.path().join("trusted.json"));
        trust.allow(&real).unwrap();
        std::os::unix::fs::symlink(&real, w.join(WORKSPACE_FILE)).unwrap();
        let loaded = load_merged(&g, &w, &trust).unwrap();
        assert_eq!(loaded.config.tools.len(), 6);
        assert!(loaded.ignored_untrusted.is_some());
        // and `allow` on the symlink itself must refuse
        assert!(trust.allow(&w.join(WORKSPACE_FILE)).is_err());
    }
}
