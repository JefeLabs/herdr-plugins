//! Tool configuration: a global `tools.toml` merged with an optional
//! `.herdr-toolbar.toml` at the workspace root.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

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
        Self { prefix: DEFAULT_PREFIX.to_string(), tools: Vec::new() }
    }
}

pub fn parse(text: &str) -> anyhow::Result<RawConfig> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| anyhow::anyhow!("{}", describe(&e, text)))?;
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

/// Read the global file (required to exist, may be empty) and, if present,
/// `<workspace_root>/.herdr-toolbar.toml`; merge by id; apply order.
pub fn load_merged(global: &Path, workspace_root: &Path) -> anyhow::Result<Config> {
    let global_text = std::fs::read_to_string(global)
        .with_context(|| format!("reading {}", global.display()))?;
    let global_raw = parse(&global_text).with_context(|| global.display().to_string())?;

    let ws_path = workspace_root.join(WORKSPACE_FILE);
    let ws_raw = if ws_path.is_file() {
        let text = std::fs::read_to_string(&ws_path)
            .with_context(|| format!("reading {}", ws_path.display()))?;
        Some(parse(&text).with_context(|| ws_path.display().to_string())?)
    } else {
        None
    };

    merge(global_raw, ws_raw)
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

    Ok(Config { prefix: prefix.unwrap_or_else(|| DEFAULT_PREFIX.to_string()), tools })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const GLOBAL: &str = include_str!("../tools.example.toml");

    fn dirs(global: &str, workspace: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
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
        assert_eq!(raw.tool.len(), 5);
        assert_eq!(raw.tool[2].id, "db");
        assert_eq!(raw.tool[4].env["JIRA_PROJECT"], "SKOOL");
        let want: Vec<String> = ["issues", "todo", "db", "files", "jira"].map(String::from).to_vec();
        assert_eq!(raw.order.as_deref(), Some(&want[..]));
        assert_eq!(raw.prefix.as_deref(), Some("ctrl-]"));
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
        let (_t, g, w) = dirs(GLOBAL, Some(ws));
        let cfg = load_merged(&g, &w).unwrap();
        assert_eq!(cfg.tools.len(), 5);
        assert_eq!(cfg.tools[2].id, "db");
        assert_eq!(cfg.tools[2].name, "Slot DB");
        assert_eq!(cfg.tools[2].command[1], "postgres://localhost:5433/x");
    }

    #[test]
    fn workspace_order_wins() {
        let ws = "order = [\"files\", \"db\"]\n";
        let (_t, g, w) = dirs(GLOBAL, Some(ws));
        let cfg = load_merged(&g, &w).unwrap();
        let ids: Vec<&str> = cfg.tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["files", "db", "issues", "todo", "jira"]);
    }

    #[test]
    fn missing_workspace_file_is_fine() {
        let (_t, g, w) = dirs(GLOBAL, None);
        let cfg = load_merged(&g, &w).unwrap();
        assert_eq!(cfg.tools.len(), 5);
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
        let (_t, g, w) = dirs(GLOBAL, Some(ws));
        let e = load_merged(&g, &w).unwrap_err().to_string();
        assert!(e.contains("`nope`"), "got: {e}");
    }

    #[test]
    fn cwd_tilde_is_expanded() {
        let (_t, g, w) = dirs(GLOBAL, None);
        let cfg = load_merged(&g, &w).unwrap();
        let cwd = cfg.tools[2].cwd.as_deref().unwrap();
        assert!(!cwd.starts_with('~'), "got: {cwd}");
    }
}
