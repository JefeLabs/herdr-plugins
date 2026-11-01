//! What a tool may know about where it runs: values from the workspace's
//! context command (`context = […]` in tools.toml, e.g. `herdr-intents
//! context`), named `{name}` in a tool's `command`, `panes` or `cwd`. The
//! toolbar adds `{agent}`, the folder of the active agent: the agent whose
//! chat is in the main pane. The toolbar knows nothing else about them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::config::ToolConfig;

/// The values a workspace's tools may use, by name. `<name>_error` says why
/// `<name>` is missing; `_error` why all of them are.
pub type Vars = BTreeMap<String, String>;

/// The agent whose chat is in the main pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane: String,
    pub cwd: String,
}

/// Each `{name}` in `text` (a lower-case name: `{db}`, `{meta}`), passed to
/// `f`, which returns its value; a `{` right after `$` is the shell's
/// (`${HOME}`), and anything else stays as written.
fn fill(text: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let (before, from) = rest.split_at(open);
        out.push_str(before);
        let name = from[1..]
            .split('}')
            .next()
            .filter(|_| from[1..].contains('}'));
        // `{db}`, or `{db?}`: optional
        let is_name = |n: &str| {
            let n = n.strip_suffix('?').unwrap_or(n);
            n.starts_with(|c: char| c.is_ascii_lowercase())
                && n.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        };
        match name.filter(|n| is_name(n) && !before.ends_with('$')) {
            Some(n) => {
                out.push_str(&f(n).unwrap_or_else(|| format!("{{{n}}}")));
                rest = &from[n.len() + 2..];
            }
            None => {
                out.push('{');
                rest = &from[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Every text of a tool that may name a value: its command, its panes' and
/// its folder.
fn texts(tool: &ToolConfig) -> impl Iterator<Item = &String> {
    tool.command
        .iter()
        .chain(tool.panes.iter().flatten())
        .chain(tool.cwd.iter())
}

/// The `{name}`s a tool uses (`{name?}` counts as `name`).
pub fn placeholders(tool: &ToolConfig) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for text in texts(tool) {
        fill(text, |n| {
            names.insert(n.trim_end_matches('?').to_string());
            None
        });
    }
    names
}

/// The name of an optional value that is the whole of `text` (`{db?}`).
fn whole_optional(text: &str) -> Option<&str> {
    let name = text.strip_prefix('{')?.strip_suffix("?}")?;
    (!name.is_empty() && fill(text, |_| Some(String::new())).is_empty()).then_some(name)
}

/// The tool with its `{name}`s filled in from `vars`; `Err` names the first
/// one missing and why (`<name>_error`, else `_error`). An optional value
/// that is missing (`{name?}`) drops out: an argument that is nothing else
/// is left out, a folder that is nothing else is the workspace's, and inside
/// a longer text it is empty.
pub fn expand(tool: &ToolConfig, vars: &Vars) -> Result<ToolConfig, String> {
    let mut missing: Option<String> = None;
    let mut sub = |text: &String| -> Option<String> {
        if whole_optional(text).is_some_and(|n| !vars.contains_key(n)) {
            return None;
        }
        Some(fill(text, |n| match n.strip_suffix('?') {
            Some(optional) => Some(vars.get(optional).cloned().unwrap_or_default()),
            None => {
                let v = vars.get(n).cloned();
                if v.is_none() && missing.is_none() {
                    missing = Some(n.to_string());
                }
                v
            }
        }))
    };
    let mut out = tool.clone();
    out.command = tool.command.iter().filter_map(&mut sub).collect();
    out.panes = tool
        .panes
        .iter()
        .map(|p| p.iter().filter_map(&mut sub).collect())
        .collect();
    out.cwd = tool.cwd.as_ref().and_then(&mut sub);
    match missing {
        None => Ok(out),
        Some(n) => {
            let why = vars
                .get(&format!("{n}_error"))
                .or_else(|| vars.get("_error"))
                .map_or("nothing provides it here", String::as_str);
            Err(format!("{} needs {{{n}}}: {why}", tool.name))
        }
    }
}

/// The active agent of workspace `workspace`, from `herdr agent list`: the
/// focused agent there; else (the toolbar has focus) the one active before,
/// if it is still there; else the workspace's first agent.
pub fn active_agent(
    list: &serde_json::Value,
    workspace: &str,
    remembered: Option<&Agent>,
) -> Option<Agent> {
    let mut agents: Vec<(Agent, bool)> = Vec::new();
    for a in list
        .pointer("/result/agents")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |k: &str| a.get(k).and_then(|v| v.as_str()).map(str::to_string);
        if text("workspace_id").as_deref() != Some(workspace) {
            continue;
        }
        let (Some(pane), Some(cwd)) = (text("pane_id"), text("cwd")) else {
            continue;
        };
        let focused = a.get("focused").and_then(serde_json::Value::as_bool) == Some(true);
        agents.push((Agent { pane, cwd }, focused));
    }
    let focused = agents.iter().find(|(_, f)| *f);
    let before = remembered.and_then(|r| agents.iter().find(|(a, _)| a.pane == r.pane));
    focused
        .or(before)
        .or_else(|| agents.first())
        .map(|(a, _)| a.clone())
}

/// Runs the context command `cmd` in `cwd`, telling it the active agent's
/// folder (`TOOLBAR_AGENT_CWD`), and keeps the text values of the JSON object
/// it prints. `{agent}` is the agent's folder unless the command says
/// otherwise. A command that fails leaves `_error`, which every missing
/// value then gives as its reason.
pub fn fetch(cmd: &[String], cwd: &Path, agent: Option<&Agent>, env: &[(String, String)]) -> Vars {
    let mut vars = Vars::new();
    if let Some(a) = agent {
        vars.insert("agent".into(), a.cwd.clone());
    }
    let Some((program, args)) = cmd.split_first() else {
        return vars;
    };
    let ran = std::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env("TOOLBAR_AGENT_CWD", agent.map_or("", |a| a.cwd.as_str()))
        .stdin(std::process::Stdio::null())
        .output();
    let out = match ran {
        Ok(out) => out,
        Err(e) => {
            vars.insert(
                "_error".into(),
                format!("cannot run the context command `{program}`: {e}"),
            );
            return vars;
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.lines().rev().map(str::trim).find(|l| !l.is_empty());
        let why = why.map_or_else(
            || format!("the context command failed ({})", out.status),
            str::to_string,
        );
        vars.insert("_error".into(), why);
        return vars;
    }
    match serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&out.stdout) {
        Ok(map) => {
            for (k, v) in map {
                if let serde_json::Value::String(text) = v {
                    vars.insert(k, text);
                }
            }
        }
        Err(e) => {
            vars.insert(
                "_error".into(),
                format!("the context command printed no JSON object: {e}"),
            );
        }
    }
    vars
}

/// Follows the active agent of the toolbar's workspace (`herdr`) every
/// `every`, and sends the workspace's context (`cmd` run in `cwd`) whenever
/// it changes: once at the start, then for each other agent made active.
/// Outside Herdr there is no agent, and the context is sent once. Ends when
/// `stop` is set or nobody listens.
pub fn follow(
    herdr: Option<crate::herdr::HerdrContext>,
    cmd: Vec<String>,
    cwd: std::path::PathBuf,
    every: std::time::Duration,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: std::sync::mpsc::Sender<Vars>,
) {
    use std::sync::atomic::Ordering;
    let mut sent: Option<Option<Agent>> = None;
    while !stop.load(Ordering::Relaxed) {
        let agent = herdr.as_ref().and_then(|h| {
            let list = h.agent_list()?;
            let before = sent.clone().flatten();
            active_agent(&list, h.workspace_id.as_deref()?, before.as_ref()).or(before)
        });
        if sent.as_ref() != Some(&agent) {
            let env: &[(String, String)] = herdr.as_ref().map_or(&[], |h| &h.inherit);
            if tx.send(fetch(&cmd, &cwd, agent.as_ref(), env)).is_err() {
                return;
            }
            sent = Some(agent);
        }
        if herdr.is_none() {
            return;
        }
        std::thread::sleep(every);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(command: &[&str], cwd: Option<&str>) -> ToolConfig {
        ToolConfig {
            id: "db".into(),
            name: "Database".into(),
            icon: "D".into(),
            command: command.iter().map(|s| s.to_string()).collect(),
            cwd: cwd.map(str::to_string),
            env: BTreeMap::new(),
            open: Default::default(),
            graphics: false,
            panes: Vec::new(),
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_tool_names_what_it_needs_in_braces_and_shell_variables_are_not_names() {
        let t = tool(
            &[
                "sh",
                "-c",
                "exec lazysql {db} ${XDG_CACHE_HOME:-$HOME/.cache} ${HERDR_WORKSPACE_ID}",
            ],
            Some("{meta}"),
        );
        let names: Vec<String> = placeholders(&t).into_iter().collect();
        assert_eq!(names, vec!["db", "meta"]);
        assert!(placeholders(&tool(&["lazysql"], Some("~"))).is_empty());
    }

    #[test]
    fn a_tool_is_filled_in_from_the_context() {
        let t = tool(&["lazysql", "{db}", "--x={db}"], Some("{meta}/app"));
        let got = expand(
            &t,
            &vars(&[("db", "postgres://h:5433/d"), ("meta", "/w/skoolscout-com")]),
        )
        .unwrap();
        assert_eq!(
            got.command,
            vec!["lazysql", "postgres://h:5433/d", "--x=postgres://h:5433/d"]
        );
        assert_eq!(got.cwd.as_deref(), Some("/w/skoolscout-com/app"));
        let plain = tool(&["sh", "-c", "echo ${HOME}"], None);
        assert_eq!(expand(&plain, &Vars::new()).unwrap(), plain);
    }

    #[test]
    fn a_tool_missing_a_value_says_why_and_does_not_start() {
        let t = tool(&["lazysql", "{db}"], None);
        let why = expand(
            &t,
            &vars(&[("db_error", "no slot database: work start --mode fullstack")]),
        )
        .unwrap_err();
        assert_eq!(
            why,
            "Database needs {db}: no slot database: work start --mode fullstack"
        );
        let all = expand(&t, &vars(&[("_error", "the context command failed")])).unwrap_err();
        assert_eq!(all, "Database needs {db}: the context command failed");
        let bare = expand(&t, &Vars::new()).unwrap_err();
        assert_eq!(bare, "Database needs {db}: nothing provides it here");
    }

    #[test]
    fn an_optional_value_that_is_missing_drops_out() {
        let t = tool(&["lazysql", "{db?}"], Some("{meta?}"));
        assert_eq!(
            placeholders(&t).into_iter().collect::<Vec<_>>(),
            vec!["db", "meta"]
        );
        // nothing known: plain lazysql, in the workspace's folder
        let plain = expand(&t, &vars(&[("db_error", "no slot")])).unwrap();
        assert_eq!(plain.command, vec!["lazysql"]);
        assert_eq!(plain.cwd, None);
        let known = expand(&t, &vars(&[("db", "postgres://h/d"), ("meta", "/m")])).unwrap();
        assert_eq!(known.command, vec!["lazysql", "postgres://h/d"]);
        assert_eq!(known.cwd.as_deref(), Some("/m"));
        // inside a longer text it is empty
        let inner = tool(&["sh", "-c", "echo [{db?}]"], None);
        assert_eq!(expand(&inner, &Vars::new()).unwrap().command[2], "echo []");
    }

    /// `herdr agent list`, as Herdr 0.9.5 answers it (all fields), for agents
    /// `(pane, workspace, focused, cwd)`.
    fn list(agents: &[(&str, &str, bool, &str)]) -> serde_json::Value {
        let agents: Vec<serde_json::Value> = agents
            .iter()
            .map(|(pane, ws, focused, cwd)| {
                serde_json::json!({
                    "agent": "claude", "agent_session": null, "agent_status": "idle",
                    "cwd": cwd, "focused": focused, "foreground_cwd": cwd,
                    "pane_id": pane, "revision": 1, "state_change_seq": 1,
                    "tab_id": format!("{ws}:t1"), "terminal_id": "t1",
                    "terminal_title": "", "terminal_title_stripped": "",
                    "workspace_id": ws,
                })
            })
            .collect();
        serde_json::json!({ "result": { "agents": agents } })
    }

    fn agent(pane: &str, cwd: &str) -> Agent {
        Agent {
            pane: pane.into(),
            cwd: cwd.into(),
        }
    }

    #[test]
    fn the_active_agent_is_the_focused_one_in_this_workspace() {
        let l = list(&[
            ("w2:p1", "w2", true, "/other"),
            ("w1:p1", "w1", false, "/wt/a"),
            ("w1:p4", "w1", true, "/wt/b"),
        ]);
        assert_eq!(active_agent(&l, "w1", None), Some(agent("w1:p4", "/wt/b")));
    }

    #[test]
    fn with_the_toolbar_focused_the_active_agent_stays_the_one_before() {
        // clicking the toolbar focuses it: no agent is focused
        let l = list(&[
            ("w1:p1", "w1", false, "/wt/a"),
            ("w1:p4", "w1", false, "/wt/b"),
        ]);
        let before = agent("w1:p4", "/old");
        assert_eq!(
            active_agent(&l, "w1", Some(&before)),
            Some(agent("w1:p4", "/wt/b")),
            "the same agent, with the folder it is in now"
        );
        // it has gone: the workspace's first agent
        let gone = agent("w1:p9", "/gone");
        assert_eq!(
            active_agent(&l, "w1", Some(&gone)),
            Some(agent("w1:p1", "/wt/a"))
        );
        assert_eq!(
            active_agent(&l, "w3", None),
            None,
            "no agent in this workspace"
        );
        assert_eq!(active_agent(&serde_json::json!({}), "w1", None), None);
    }

    #[test]
    fn the_context_command_is_told_the_active_agents_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd: Vec<String> = [
            "sh",
            "-c",
            r#"printf '{"meta":"%s","agent_seen":"%s","n":1,"space":"s"}' "$PWD" "$TOOLBAR_AGENT_CWD""#,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let here = std::fs::canonicalize(tmp.path()).unwrap();
        let got = fetch(&cmd, &here, Some(&agent("w1:p4", "/wt/b")), &[]);
        assert_eq!(
            got.get("meta").map(String::as_str),
            Some(here.to_str().unwrap())
        );
        assert_eq!(got.get("agent_seen").map(String::as_str), Some("/wt/b"));
        assert_eq!(
            got.get("agent").map(String::as_str),
            Some("/wt/b"),
            "{{agent}} by default"
        );
        assert!(!got.contains_key("n"), "only text values");
    }

    #[test]
    fn a_failing_context_command_says_why_for_every_value() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd: Vec<String> = ["sh", "-c", "echo 'no intents here' >&2; exit 2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got = fetch(&cmd, tmp.path(), None, &[]);
        assert!(got["_error"].contains("no intents here"), "{got:?}");
        let missing: Vec<String> = vec!["/nonexistent/context-cmd".into()];
        let got = fetch(&missing, tmp.path(), None, &[]);
        assert!(got["_error"].contains("context-cmd"), "{got:?}");
        // the agent is still known without the command
        let got = fetch(&[], tmp.path(), Some(&agent("w1:p1", "/wt/a")), &[]);
        assert_eq!(got.get("agent").map(String::as_str), Some("/wt/a"));
    }

    /// A Herdr context on the fake `herdr`, for workspace w1, whose
    /// `agent list` answers from the file `agents`.
    fn fake_herdr(dir: &Path, agents: &Path) -> crate::herdr::HerdrContext {
        crate::herdr::HerdrContext {
            bin: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-herdr.sh"),
            workspace_id: Some("w1".into()),
            pane_id: Some("w1:p9".into()),
            tab_id: Some("w1:t1".into()),
            target_pane_id: None,
            workspace_cwd: dir.to_path_buf(),
            config_dir: dir.join("config"),
            state_dir: dir.join("state"),
            socket: dir.join("herdr.sock"),
            inherit: vec![],
            extra_env: vec![
                (
                    "FAKE_HERDR_LOG".into(),
                    dir.join("herdr.log").display().to_string(),
                ),
                ("FAKE_AGENTS".into(), agents.display().to_string()),
            ],
        }
    }

    #[test]
    fn the_context_follows_the_active_agent_and_not_the_toolbars_focus() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;
        let tmp = tempfile::tempdir().unwrap();
        let agents = tmp.path().join("agents.json");
        let put = |l: serde_json::Value| std::fs::write(&agents, l.to_string()).unwrap();
        put(list(&[
            ("w1:p3", "w1", true, "/wt/a"),
            ("w1:p4", "w1", false, "/wt/b"),
        ]));
        let cmd: Vec<String> = [
            "sh",
            "-c",
            r#"printf '{"db":"db-of-%s"}' "$TOOLBAR_AGENT_CWD""#,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let herdr = fake_herdr(tmp.path(), &agents);
        let (s, dir) = (stop.clone(), tmp.path().to_path_buf());
        let follower = std::thread::spawn(move || {
            follow(Some(herdr), cmd, dir, Duration::from_millis(30), s, tx)
        });
        let next = || rx.recv_timeout(Duration::from_secs(5)).expect("a context");
        let first = next();
        assert_eq!(first.get("db").map(String::as_str), Some("db-of-/wt/a"));
        assert_eq!(first.get("agent").map(String::as_str), Some("/wt/a"));
        // another agent becomes active
        put(list(&[
            ("w1:p3", "w1", false, "/wt/a"),
            ("w1:p4", "w1", true, "/wt/b"),
        ]));
        assert_eq!(next().get("db").map(String::as_str), Some("db-of-/wt/b"));
        // the toolbar takes focus: nothing changes
        put(list(&[
            ("w1:p3", "w1", false, "/wt/a"),
            ("w1:p4", "w1", false, "/wt/b"),
        ]));
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "no new context"
        );
        stop.store(true, Ordering::Relaxed);
        follower.join().unwrap();
    }
}
