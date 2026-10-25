use std::fs;
use std::path::{Path, PathBuf};

use herdr_capabilities::config::{Dirs, load_or_init};
use herdr_capabilities::send::{agent_name, main_checkout, render_prompt, send, uses_workbrokr};
use herdr_capabilities::store::Store;

fn fake_herdr() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-herdr.sh")
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?} in {}", dir.display());
}

#[test]
fn main_checkout_resolves_worktree_to_main() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(
        &main,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let wt = tmp.path().join("wt");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            wt.to_str().unwrap(),
        ],
    );
    let want = fs::canonicalize(&main).unwrap();
    assert_eq!(
        main_checkout(&wt).unwrap(),
        want,
        "worktree resolves to the main checkout"
    );
    assert_eq!(main_checkout(&main).unwrap(), want);
    let sub = wt.join("deep/er");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(
        main_checkout(&sub).unwrap(),
        want,
        "from a subdirectory too"
    );
    assert!(main_checkout(tmp.path()).is_err(), "not a repo");
}

#[test]
fn workbrokr_detected_by_file_and_overridable() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!uses_workbrokr(tmp.path(), None));
    fs::write(tmp.path().join("helmsmith.config.ts"), "export default {}").unwrap();
    assert!(uses_workbrokr(tmp.path(), None));
    assert!(!uses_workbrokr(tmp.path(), Some(false)));
    assert!(uses_workbrokr(Path::new("/nonexistent"), Some(true)));
}

#[test]
fn render_prompt_prefixes_work_start_only_for_building_handoffs_in_workbrokr_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = Dirs {
        config: tmp.path().join("cfg"),
        root: tmp.path().join("root"),
    };
    let cfg = load_or_init(&dirs).unwrap();
    let store = Store::open(&dirs.root).unwrap();
    let it = store
        .create(
            Path::new("Inbox"),
            "Pending label",
            "issue",
            "## Observed\nx\n",
            None,
        )
        .unwrap();
    let shot = tmp.path().join("s.png");
    fs::write(&shot, b"png").unwrap();
    Store::attach(&it, &shot).unwrap();
    let atts = Store::attachments(&it);
    let issue = cfg
        .handoffs
        .iter()
        .find(|h| h.name == "issue-report")
        .unwrap();
    let grill = cfg.handoffs.iter().find(|h| h.name == "grill").unwrap();

    let p = render_prompt(issue, &it, &atts, true);
    assert!(p.starts_with("This repo uses workbrokr."), "{p}");
    assert!(p.contains("work start --area"));
    assert!(p.contains("## Observed\nx"), "intent substituted");
    assert!(p.contains("s.png"), "attachments substituted: {p}");
    assert!(!p.contains("{intent}") && !p.contains("{attachments}"));

    let p = render_prompt(grill, &it, &atts, true);
    assert!(
        !p.contains("work start"),
        "planning handoffs never start a stack: {p}"
    );
    assert!(p.starts_with("/grill-me"));

    let p = render_prompt(issue, &it, &atts, false);
    assert!(!p.contains("work start"), "non-workbrokr repo: {p}");
}

#[test]
fn send_runs_create_start_prompt_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("herdr.log");
    let env = vec![("FAKE_HERDR_LOG".to_string(), log.display().to_string())];
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let out = send(
        &fake_herdr(),
        &env,
        &repo,
        "Pending label",
        "claude",
        "do the thing",
    )
    .unwrap();
    assert_eq!(out.workspace_id, "w9");
    assert_eq!(out.pane_id, "w9:p1");
    assert!(out.agent.starts_with("intent-"));
    let l = fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = l.lines().collect();
    assert!(lines[0].starts_with("workspace create --cwd"), "{l}");
    assert!(
        lines[0].contains(&format!("--cwd {}", repo.display()))
            && lines[0].contains("--no-focus")
            && lines[0].contains("--label Pending label"),
        "{l}"
    );
    assert!(
        lines[1].starts_with(&format!(
            "agent start {} --kind claude --pane w9:p1",
            out.agent
        )),
        "{l}"
    );
    assert!(
        lines[2].starts_with(&format!("agent prompt {} do the thing", out.agent)),
        "{l}"
    );
}

#[test]
fn send_failure_names_the_step_and_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("herdr.log");
    let env = vec![
        ("FAKE_HERDR_LOG".to_string(), log.display().to_string()),
        ("FAKE_AGENT_START_FAILS".to_string(), "1".to_string()),
    ];
    let e = send(&fake_herdr(), &env, tmp.path(), "t", "claude", "p").unwrap_err();
    assert_eq!(e.step, "agent start");
    assert!(
        e.detail.contains("agent did not become ready"),
        "{}",
        e.detail
    );
    let l = fs::read_to_string(&log).unwrap();
    assert!(
        !l.contains("agent prompt"),
        "no prompt after a failed start: {l}"
    );
}

#[test]
fn agent_name_is_valid_and_short() {
    let n = agent_name("2026-10-24-pending-label-on-skipped-folder-with-a-very-long-slug");
    assert!(n.starts_with("intent-"));
    assert!(n.len() <= 32, "{n} ({})", n.len());
    assert!(
        n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    );
    assert!(!n.ends_with('-'));
    assert!(agent_name("---").starts_with("intent-x-"));
}

#[test]
fn agent_names_are_unique_per_send() {
    let a = agent_name("same-slug");
    let b = agent_name("same-slug");
    assert_ne!(
        a, b,
        "two sends of one intent must not collide on the agent name"
    );
    assert!(a.len() <= 32 && b.len() <= 32);
}

#[test]
fn failed_start_reports_the_orphaned_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("herdr.log");
    let env = vec![
        ("FAKE_HERDR_LOG".to_string(), log.display().to_string()),
        ("FAKE_AGENT_START_FAILS".to_string(), "1".to_string()),
    ];
    let e = send(&fake_herdr(), &env, tmp.path(), "t", "claude", "p").unwrap_err();
    assert!(
        e.detail.contains("w9"),
        "names the workspace left behind: {}",
        e.detail
    );
}

#[test]
fn prompts_ask_the_agent_to_verify_preconditions_first_when_there_are_any() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = Dirs {
        config: tmp.path().join("cfg"),
        root: tmp.path().join("root"),
    };
    let cfg = load_or_init(&dirs).unwrap();
    let store = Store::open(&dirs.root).unwrap();
    let with = store
        .create(
            Path::new("Inbox"),
            "With",
            "idea",
            "## Description\nx\n\n## Preconditions\n- [ ] report cards optional\n",
            None,
        )
        .unwrap();
    let without = store
        .create(
            Path::new("Inbox"),
            "Without",
            "idea",
            "## Description\nx\n",
            None,
        )
        .unwrap();
    let grill = cfg.handoffs.iter().find(|h| h.name == "grill").unwrap();
    let p = render_prompt(grill, &with, &[], false);
    assert!(p.to_lowercase().contains("precondition"), "{p}");
    assert!(
        p.find("precondition").unwrap() < p.find("/grill-me").unwrap(),
        "the instruction comes first: {p}"
    );
    let p = render_prompt(grill, &without, &[], false);
    assert!(!p.to_lowercase().contains("precondition"), "{p}");
}
