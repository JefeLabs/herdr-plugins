//! Opening a meta repo's folder in a new Herdr workspace opens its space.
//!
//! Herdr runs `herdr-intents open-space` on `workspace.created`. When the new
//! workspace's folder is in the main checkout of a meta repo (one whose
//! `.helmsmith/intents/` names a space), the workspace takes the space's name
//! and its intents load: the toolbar opens (Intents is its first tool), or,
//! without one, the Intents pane does.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// What the hook did with the new workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The workspace now carries the space's name and shows its intents.
    Opened { space: String, intents: Load },
    /// The workspace was left as it was, and why.
    Skipped(String),
}

/// How the space's intents were loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load {
    /// The toolbar opened, starting its first tool.
    Toolbar,
    /// Intents opened in its own pane.
    Pane,
    /// Neither could open; the reason Herdr gave.
    Failed(String),
}

/// The toolbar plugin, whose first tool is Intents.
pub const TOOLBAR_PLUGIN: &str = "jefelabs.toolbar";
/// This plugin.
pub const PLUGIN: &str = "jefelabs.intents";

/// The id of the workspace a `workspace.created` event is about: the event's
/// workspace (as Herdr's event data, or wrapped in an envelope), else
/// `HERDR_WORKSPACE_ID`.
pub fn event_workspace(event: Option<&str>, env_id: Option<&str>) -> Option<String> {
    let from_event = event
        .and_then(|e| serde_json::from_str::<Value>(e).ok())
        .and_then(|v| {
            [
                "/workspace/workspace_id",
                "/data/workspace/workspace_id",
                "/workspace_id",
            ]
            .iter()
            .find_map(|p| v.pointer(p).and_then(Value::as_str).map(str::to_string))
        });
    from_event
        .or_else(|| env_id.map(str::to_string))
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// The git repo a folder is in: its top folder, and whether that is a linked
/// worktree rather than the main checkout. `None` outside a git repo.
pub fn repo_of(folder: &Path) -> Option<(PathBuf, bool)> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(folder)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let top = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let top = std::fs::canonicalize(&top).unwrap_or(top);
    let main = crate::send::main_checkout(folder).ok()?;
    let linked = main != top;
    Some((top, linked))
}

/// Open the space of workspace `id`'s folder, asking Herdr through `herdr`
/// (the CLI's arguments → its reply, or why it failed).
pub fn open(id: &str, herdr: impl Fn(&[&str]) -> Result<Value, String>) -> Outcome {
    let skip = Outcome::Skipped;
    let ws = match herdr(&["workspace", "get", id]) {
        Ok(reply) => reply
            .pointer("/result/workspace")
            .cloned()
            .unwrap_or_default(),
        Err(e) => return skip(format!("cannot read it: {e}")),
    };
    let label = ws.get("label").and_then(Value::as_str).unwrap_or_default();
    let focused = ws.get("focused").and_then(Value::as_bool).unwrap_or(false);
    let panes = match herdr(&["pane", "list", "--workspace", id]) {
        Ok(reply) => reply,
        Err(e) => return skip(format!("cannot list its panes: {e}")),
    };
    let first = panes.pointer("/result/panes/0");
    let field = |k: &str| first.and_then(|p| p.get(k)).and_then(Value::as_str);
    let (Some(pane), Some(cwd)) = (field("pane_id"), field("cwd")) else {
        return skip("it has no pane with a folder".into());
    };
    let Some((top, linked)) = repo_of(Path::new(cwd)) else {
        return skip(format!("{cwd} is not in a git repo"));
    };
    if linked {
        return skip(format!("{} is a linked worktree", top.display()));
    }
    // Herdr names a workspace after its repo and keeps it in step with the
    // folder; any other name was chosen by someone, and stays
    let folder_name = top.file_name().map(|n| n.to_string_lossy().into_owned());
    if folder_name.as_deref() != Some(label) {
        return skip(format!("someone named it \"{label}\""));
    }
    let space = match crate::space::intents_space(&top) {
        None => return skip(format!("{} is not a meta repo", top.display())),
        Some(Err(why)) => return skip(why),
        Some(Ok(space)) => space,
    };
    match herdr(&["workspace", "list"]) {
        Ok(list) => {
            let all = list.pointer("/result/workspaces").and_then(Value::as_array);
            let taken = all.into_iter().flatten().find_map(|w| {
                let other = w.get("workspace_id").and_then(Value::as_str) != Some(id);
                let name = w.get("label").and_then(Value::as_str)?;
                (other && name.trim().eq_ignore_ascii_case(&space)).then_some(name)
            });
            if let Some(name) = taken {
                return skip(format!("{space} already has a workspace, \"{name}\""));
            }
        }
        Err(e) => return skip(format!("cannot list workspaces to keep one per space: {e}")),
    }
    if let Err(e) = herdr(&["workspace", "rename", id, &space]) {
        return skip(format!("could not name it {space}: {e}"));
    }
    let intents = load(pane, &top, focused, &herdr);
    Outcome::Opened { space, intents }
}

/// Open the space's intents in the new workspace: the toolbar when it is
/// installed and the workspace has focus (its toggle opens beside the focused
/// pane), else the Intents pane, cut from the workspace's own pane.
fn load(
    pane: &str,
    folder: &Path,
    focused: bool,
    herdr: &impl Fn(&[&str]) -> Result<Value, String>,
) -> Load {
    if focused && has_toolbar(herdr) {
        let toggle = herdr(&[
            "plugin",
            "action",
            "invoke",
            "toggle",
            "--plugin",
            TOOLBAR_PLUGIN,
        ]);
        if toggle.is_ok() {
            return Load::Toolbar;
        }
    }
    let cwd = folder.display().to_string();
    let opened = herdr(&[
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN,
        "--entrypoint",
        "intents",
        "--placement",
        "split",
        "--direction",
        "right",
        "--target-pane",
        pane,
        "--cwd",
        &cwd,
        "--no-focus",
    ]);
    match opened {
        Ok(_) => Load::Pane,
        Err(e) => Load::Failed(e),
    }
}

/// Whether the toolbar plugin is installed and enabled.
fn has_toolbar(herdr: &impl Fn(&[&str]) -> Result<Value, String>) -> bool {
    let Ok(list) = herdr(&["plugin", "list", "--json"]) else {
        return false;
    };
    let plugins = list.pointer("/result/plugins").and_then(Value::as_array);
    plugins.into_iter().flatten().any(|p| {
        p.get("plugin_id").and_then(Value::as_str) == Some(TOOLBAR_PLUGIN)
            && p.get("enabled").and_then(Value::as_bool) != Some(false)
    })
}

/// The `herdr` CLI at `bin`: its reply (JSON, or the text a few commands
/// print), or why it failed. A reply that carries an `error` is a failure even
/// when the CLI exits 0.
pub fn cli(bin: PathBuf) -> impl Fn(&[&str]) -> Result<Value, String> {
    move |args| {
        let out = std::process::Command::new(&bin)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{}: {e}", bin.display()))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        // errors come back as JSON, on stderr or (exit 0) on stdout
        for text in [stdout.trim(), stderr.trim()] {
            if let Ok(v) = serde_json::from_str::<Value>(text)
                && let Some(err) = v.get("error")
            {
                let part = |k: &str| err.get(k).and_then(Value::as_str).unwrap_or_default();
                return Err(format!("{}: {}", part("code"), part("message")));
            }
        }
        if !out.status.success() {
            return Err(stderr.trim().to_string());
        }
        // most commands answer in JSON; a few (`plugin list`) in text
        let text = stdout.trim();
        Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())))
    }
}

/// One line for Herdr's plugin log.
pub fn describe(id: &str, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Opened { space, intents } => {
            let how = match intents {
                Load::Toolbar => "its intents opened in the toolbar".to_string(),
                Load::Pane => "its intents opened in the Intents pane".to_string(),
                Load::Failed(why) => format!("could not open its intents: {why}"),
            };
            format!("workspace {id} is now {space}; {how}")
        }
        Outcome::Skipped(why) => format!("workspace {id} left as it is: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::fs;

    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `<tmp>/skoolscout-com`: a git repo whose intents folder names `skoolscout`.
    fn meta_repo(tmp: &Path) -> PathBuf {
        let meta = tmp.join("skoolscout-com");
        fs::create_dir_all(meta.join(".helmsmith/intents")).unwrap();
        fs::create_dir_all(meta.join("app-ui")).unwrap();
        fs::write(
            meta.join(".helmsmith/intents/config.toml"),
            "space = \"skoolscout\"\n",
        )
        .unwrap();
        git(&meta, &["init", "-q"]);
        fs::canonicalize(meta).unwrap()
    }

    /// A Herdr that answers from these facts and records every call.
    struct FakeHerdr {
        label: String,
        focused: bool,
        cwd: PathBuf,
        others: Vec<&'static str>,
        toolbar: bool,
        calls: RefCell<Vec<String>>,
    }

    impl FakeHerdr {
        fn new(label: &str, cwd: &Path) -> Self {
            FakeHerdr {
                label: label.into(),
                focused: true,
                cwd: cwd.to_path_buf(),
                others: vec!["~"],
                toolbar: true,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn call(&self, args: &[&str]) -> Result<Value, String> {
            self.calls.borrow_mut().push(args.join(" "));
            let ws = json!({
                "workspace_id": "w3", "label": self.label, "focused": self.focused,
                "number": 3, "pane_count": 1, "tab_count": 1,
                "active_tab_id": "w3:t1", "agent_status": "unknown"
            });
            match args {
                ["workspace", "get", "w3"] => Ok(json!({"result": {"workspace": ws}})),
                ["workspace", "list"] => {
                    let mut all: Vec<Value> = self
                        .others
                        .iter()
                        .enumerate()
                        .map(|(i, l)| json!({"workspace_id": format!("w{}", i + 1), "label": l}))
                        .collect();
                    all.push(ws);
                    Ok(json!({"result": {"workspaces": all}}))
                }
                ["pane", "list", "--workspace", "w3"] => Ok(json!({"result": {"panes": [
                    {"pane_id": "w3:p1", "cwd": self.cwd, "workspace_id": "w3"}
                ]}})),
                // as Herdr 0.9.5: `plugin list` answers in text unless asked for JSON
                ["plugin", "list"] => Ok(Value::String("2 plugins installed:\n…".into())),
                ["plugin", "list", "--json"] => {
                    let mut plugins = vec![json!({"plugin_id": PLUGIN, "enabled": true})];
                    if self.toolbar {
                        plugins.push(json!({"plugin_id": TOOLBAR_PLUGIN, "enabled": true}));
                    }
                    Ok(json!({"result": {"plugins": plugins}}))
                }
                ["workspace", "rename", ..]
                | ["plugin", "action", "invoke", ..]
                | ["plugin", "pane", "open", ..] => Ok(json!({"result": {}})),
                other => Err(format!("unexpected herdr call {other:?}")),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }

        fn changed_anything(&self) -> bool {
            self.calls().iter().any(|c| {
                c.starts_with("workspace rename")
                    || c.starts_with("plugin action")
                    || c.starts_with("plugin pane")
            })
        }
    }

    #[test]
    fn the_event_names_the_workspace_herdr_created() {
        let data = r#"{"type":"workspace_created","workspace":{"workspace_id":"w3","label":"x"}}"#;
        assert_eq!(event_workspace(Some(data), None).as_deref(), Some("w3"));
        let envelope =
            r#"{"event":"workspace.created","data":{"workspace":{"workspace_id":"w4"}}}"#;
        assert_eq!(event_workspace(Some(envelope), None).as_deref(), Some("w4"));
        assert_eq!(
            event_workspace(Some("not json"), Some("w5")).as_deref(),
            Some("w5"),
            "Herdr's workspace id when the event says nothing usable"
        );
        assert_eq!(
            event_workspace(None, Some(" ")),
            None,
            "an empty id is none"
        );
        assert_eq!(event_workspace(None, None), None);
    }

    #[test]
    fn a_folder_knows_its_repo_and_whether_it_is_a_linked_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        assert_eq!(repo_of(&meta.join("app-ui")), Some((meta.clone(), false)));
        git(&meta, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let linked = tmp.path().join("wt");
        git(&meta, &["worktree", "add", "-q", linked.to_str().unwrap()]);
        let linked = fs::canonicalize(linked).unwrap();
        assert_eq!(repo_of(&linked), Some((linked.clone(), true)));
        let plain = tmp.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(repo_of(&plain), None, "not a git repo");
    }

    #[test]
    fn a_new_workspace_in_a_meta_repo_becomes_its_space_and_opens_the_toolbar() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let herdr = FakeHerdr::new("skoolscout-com", &meta);
        let got = open("w3", |a| herdr.call(a));
        assert_eq!(
            got,
            Outcome::Opened {
                space: "skoolscout".into(),
                intents: Load::Toolbar
            }
        );
        let calls = herdr.calls();
        assert!(
            calls.contains(&"workspace rename w3 skoolscout".to_string()),
            "{calls:?}"
        );
        assert!(
            calls.contains(&format!(
                "plugin action invoke toggle --plugin {TOOLBAR_PLUGIN}"
            )),
            "{calls:?}"
        );
        let renamed = calls.iter().position(|c| c.starts_with("workspace rename"));
        let toolbar = calls.iter().position(|c| c.starts_with("plugin action"));
        assert!(
            renamed < toolbar,
            "named first, so Intents starts in the space"
        );
    }

    #[test]
    fn a_subfolder_of_the_meta_repo_opens_its_space_too() {
        // Herdr names a workspace after its repo, not the folder inside it
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let herdr = FakeHerdr::new("skoolscout-com", &meta.join("app-ui"));
        assert!(matches!(
            open("w3", |a| herdr.call(a)),
            Outcome::Opened { ref space, .. } if space == "skoolscout"
        ));
    }

    #[test]
    fn without_the_toolbar_intents_opens_in_its_own_pane() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let mut herdr = FakeHerdr::new("skoolscout-com", &meta);
        herdr.toolbar = false;
        let got = open("w3", |a| herdr.call(a));
        assert_eq!(
            got,
            Outcome::Opened {
                space: "skoolscout".into(),
                intents: Load::Pane
            }
        );
        let want = format!(
            "plugin pane open --plugin {PLUGIN} --entrypoint intents --placement split \
             --direction right --target-pane w3:p1 --cwd {} --no-focus",
            meta.display()
        );
        assert!(herdr.calls().contains(&want), "{:?}", herdr.calls());
    }

    #[test]
    fn a_workspace_opened_in_the_background_gets_the_intents_pane_not_the_toolbar() {
        // the toolbar's toggle opens beside the focused pane, which is elsewhere
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let mut herdr = FakeHerdr::new("skoolscout-com", &meta);
        herdr.focused = false;
        assert!(matches!(
            open("w3", |a| herdr.call(a)),
            Outcome::Opened {
                intents: Load::Pane,
                ..
            }
        ));
        assert!(!herdr.calls().iter().any(|c| c.starts_with("plugin action")));
    }

    #[test]
    fn a_workspace_someone_named_is_left_alone() {
        // e.g. an intent sent from outside Herdr: `workspace create --label <title>`
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let herdr = FakeHerdr::new("Fix the reminder bell", &meta);
        assert!(matches!(open("w3", |a| herdr.call(a)), Outcome::Skipped(_)));
        assert!(!herdr.changed_anything(), "{:?}", herdr.calls());
    }

    #[test]
    fn a_folder_outside_a_meta_repo_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("tourneyseason");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        let herdr = FakeHerdr::new("tourneyseason", &fs::canonicalize(&repo).unwrap());
        assert!(matches!(open("w3", |a| herdr.call(a)), Outcome::Skipped(_)));
        assert!(!herdr.changed_anything());
        let home = FakeHerdr::new("~", tmp.path());
        assert!(matches!(open("w3", |a| home.call(a)), Outcome::Skipped(_)));
        assert!(!home.changed_anything());
    }

    #[test]
    fn a_linked_worktree_of_the_meta_repo_is_left_alone() {
        // agents' worktrees carry .helmsmith/intents/ too: they are not the space
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        git(&meta, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let linked = tmp.path().join("skoolscout-com-wt");
        git(&meta, &["worktree", "add", "-q", linked.to_str().unwrap()]);
        fs::create_dir_all(linked.join(".helmsmith/intents")).unwrap();
        let herdr = FakeHerdr::new("skoolscout-com-wt", &fs::canonicalize(&linked).unwrap());
        assert!(matches!(open("w3", |a| herdr.call(a)), Outcome::Skipped(_)));
        assert!(!herdr.changed_anything());
    }

    #[test]
    fn a_space_that_already_has_a_workspace_is_not_opened_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let mut herdr = FakeHerdr::new("skoolscout-com", &meta);
        herdr.others = vec!["~", "SkoolScout"];
        match open("w3", |a| herdr.call(a)) {
            Outcome::Skipped(why) => assert!(why.contains("SkoolScout"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(!herdr.changed_anything());
    }

    #[test]
    fn an_unreadable_intents_config_is_named_and_nothing_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        fs::write(meta.join(".helmsmith/intents/config.toml"), "space = [").unwrap();
        let herdr = FakeHerdr::new("skoolscout-com", &meta);
        match open("w3", |a| herdr.call(a)) {
            Outcome::Skipped(why) => assert!(why.contains("config.toml"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(!herdr.changed_anything());
    }

    #[test]
    fn a_rename_herdr_refuses_stops_before_loading_intents() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let herdr = FakeHerdr::new("skoolscout-com", &meta);
        let refusing = |a: &[&str]| {
            if a.first() == Some(&"workspace") && a.get(1) == Some(&"rename") {
                herdr.calls.borrow_mut().push(a.join(" "));
                return Err("workspace_not_found".to_string());
            }
            herdr.call(a)
        };
        match open("w3", refusing) {
            Outcome::Skipped(why) => assert!(why.contains("workspace_not_found"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(!herdr.calls().iter().any(|c| c.starts_with("plugin")));
    }

    #[test]
    fn herdrs_error_reply_is_a_failure_even_when_it_exits_0() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let p = tmp.path().join(name);
            fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let ok = script("ok", r#"echo "{\"result\":{\"args\":\"$*\"}}""#);
        assert_eq!(
            cli(ok)(&["workspace", "get", "w3"]).unwrap(),
            json!({"result": {"args": "workspace get w3"}})
        );
        let refused = script(
            "refused",
            r#"echo '{"id":"x","error":{"code":"server_not_running","message":"no server"}}'"#,
        );
        let why = cli(refused)(&["workspace", "list"]).unwrap_err();
        assert!(
            why.contains("server_not_running") && why.contains("no server"),
            "{why}"
        );
        // some commands answer in text: done is done
        let text = script("text", "echo '2 plugins installed:'");
        assert_eq!(
            cli(text)(&["plugin", "list"]).unwrap(),
            Value::String("2 plugins installed:".into())
        );
        let broken = script("broken", "echo 'usage: herdr …' >&2; exit 2");
        assert!(cli(broken)(&["x"]).unwrap_err().contains("usage"));
        let missing = cli(tmp.path().join("no-such-herdr"))(&["x"]).unwrap_err();
        assert!(missing.contains("no-such-herdr"), "{missing}");
    }

    #[test]
    fn the_log_line_says_what_happened() {
        let opened = Outcome::Opened {
            space: "skoolscout".into(),
            intents: Load::Toolbar,
        };
        assert_eq!(
            describe("w3", &opened),
            "workspace w3 is now skoolscout; its intents opened in the toolbar"
        );
        let pane = Outcome::Opened {
            space: "skoolscout".into(),
            intents: Load::Pane,
        };
        assert!(describe("w3", &pane).ends_with("in the Intents pane"));
        let failed = Outcome::Opened {
            space: "skoolscout".into(),
            intents: Load::Failed("boom".into()),
        };
        assert!(describe("w3", &failed).contains("could not open its intents: boom"));
        assert_eq!(
            describe("w3", &Outcome::Skipped("not a meta repo".into())),
            "workspace w3 left as it is: not a meta repo"
        );
    }

    #[test]
    fn when_neither_the_toolbar_nor_the_pane_opens_the_reason_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_repo(tmp.path());
        let mut herdr = FakeHerdr::new("skoolscout-com", &meta);
        herdr.toolbar = false;
        let failing = |a: &[&str]| {
            if a.starts_with(&["plugin", "pane", "open"]) {
                return Err("plugin_pane_failed".to_string());
            }
            herdr.call(a)
        };
        assert_eq!(
            open("w3", failing),
            Outcome::Opened {
                space: "skoolscout".into(),
                intents: Load::Failed("plugin_pane_failed".into())
            }
        );
    }
}
