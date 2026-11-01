//! What a toolbar in a space needs to know: the space, its meta repo, and the
//! database of the agent whose chat is in the main pane, which the space's
//! run guide says how to find (`db:`, a command run in the agent's worktree).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use herdr_intents::capture::read_guide;
use herdr_intents::config::Dirs;
use herdr_intents::context::{agent_db, gather, trust_db};
use herdr_intents::space::pointers_path;

fn git(repo: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The slot-port database command skoolscout's run guide would carry.
const SLOT_DB: &str = r#". ./.env.slot && echo "postgres://u@localhost:$SS_PG_PORT/db""#;

fn guide(db: Option<&str>) -> String {
    let db = db.map(|d| format!("db: '{d}'\n")).unwrap_or_default();
    format!("---\ndefault_area: org/social\nurl: https://x.local:{{port}}\n{db}---\nBody.\n")
}

#[test]
fn the_run_guide_says_how_to_find_an_agents_database() {
    let tmp = tempfile::tempdir().unwrap();
    put(&tmp.path().join(".helmsmith/run.md"), &guide(Some(SLOT_DB)));
    assert_eq!(read_guide(tmp.path()).unwrap().db.as_deref(), Some(SLOT_DB));
    put(&tmp.path().join(".helmsmith/run.md"), &guide(None));
    assert_eq!(read_guide(tmp.path()).unwrap().db, None);
}

/// An agent's worktree, `<tmp>/wt`, on slot port `port` (none: no slot).
fn worktree(tmp: &Path, port: Option<&str>) -> PathBuf {
    let wt = tmp.join("wt");
    fs::create_dir_all(wt.join("app-ui")).unwrap();
    git(&wt, &["init", "-q"]);
    if let Some(port) = port {
        put(&wt.join(".env.slot"), &format!("SS_PG_PORT={port}\n"));
    }
    fs::canonicalize(wt).unwrap()
}

#[test]
fn the_db_command_runs_at_the_top_of_the_agents_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = worktree(tmp.path(), Some("5433"));
    // the agent works in a subfolder; the slot file is at the worktree's top
    let got = agent_db(
        SLOT_DB,
        &wt.join("app-ui"),
        tmp.path(),
        Duration::from_secs(5),
    );
    assert_eq!(got.as_deref(), Ok("postgres://u@localhost:5433/db"));
    let meta = agent_db(
        r#"echo "$INTENTS_META""#,
        &wt,
        Path::new("/m/eta"),
        Duration::from_secs(5),
    );
    assert_eq!(
        meta.as_deref(),
        Ok("/m/eta"),
        "the command knows the meta repo"
    );
}

#[test]
fn a_db_command_that_fails_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = worktree(tmp.path(), None);
    let cmd = r#"echo starting; echo "no slot database: work start --mode fullstack" >&2; exit 3"#;
    let why = agent_db(cmd, &wt, tmp.path(), Duration::from_secs(5)).unwrap_err();
    assert!(
        why.contains("no slot database: work start --mode fullstack"),
        "{why}"
    );
    let empty = agent_db("true", &wt, tmp.path(), Duration::from_secs(5)).unwrap_err();
    assert!(empty.contains("printed nothing"), "{empty}");
}

#[test]
fn a_db_command_that_hangs_is_given_up_on() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = worktree(tmp.path(), None);
    let started = std::time::Instant::now();
    let why = agent_db("sleep 30", &wt, tmp.path(), Duration::from_millis(300)).unwrap_err();
    assert!(why.contains("took longer than"), "{why}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

struct Space {
    _tmp: tempfile::TempDir,
    dirs: Dirs,
    meta: PathBuf,
    wt: PathBuf,
}

/// The skoolscout-com space: its meta repo (with a run guide whose `db:`
/// finds a slot's database), the pointer to it, and an agent's worktree.
fn space(db: Option<&str>) -> Space {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let meta = base.join("skoolscout-com");
    fs::create_dir_all(meta.join(".helmsmith/intents")).unwrap();
    git(&meta, &["init", "-q"]);
    git(
        &meta,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:skoolscout/skoolscout-com.git",
        ],
    );
    put(&meta.join(".helmsmith/capture.md"), &guide(db));
    let dirs = Dirs {
        config: base.join("cfg"),
        root: base.join("spaces"),
    };
    put(
        &pointers_path(&dirs),
        &format!("[spaces.skoolscout-com]\nmeta = \"{}\"\n", meta.display()),
    );
    let wt = worktree(&base, Some("5434"));
    Space {
        _tmp: tmp,
        dirs,
        meta,
        wt,
    }
}

fn json(map: &serde_json::Map<String, serde_json::Value>) -> String {
    serde_json::Value::Object(map.clone()).to_string()
}

#[test]
fn a_space_with_an_active_agent_gives_its_meta_repo_and_the_agents_database() {
    let s = space(Some(SLOT_DB));
    trust_db(&s.dirs, &s.meta).unwrap();
    let got = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.wt),
        Some(&s.wt.join("app-ui")),
        Duration::from_secs(5),
    );
    assert_eq!(
        json(&got),
        serde_json::json!({
            "space": "skoolscout-com",
            "meta": s.meta.display().to_string(),
            "agent": s.wt.display().to_string(),
            "db": "postgres://u@localhost:5434/db",
            "browse_error": "the run guide has no browse: page for the space (its url needs a slot's port)",
        })
        .to_string()
    );
}

#[test]
fn the_meta_repo_is_found_from_the_workspace_folder_or_the_spaces_pointer() {
    let s = space(Some(SLOT_DB));
    // a workspace in the meta repo itself, with no pointer yet
    fs::remove_file(pointers_path(&s.dirs)).unwrap();
    let here = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.meta),
        None,
        Duration::from_secs(5),
    );
    assert_eq!(here["meta"], s.meta.display().to_string());
    // nowhere near it, and no pointer: no meta repo, and why
    let lost = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.wt),
        None,
        Duration::from_secs(5),
    );
    assert!(!lost.contains_key("meta"));
    assert!(
        lost["meta_error"]
            .as_str()
            .unwrap()
            .contains("skoolscout-com"),
        "{lost:?}"
    );
}

#[test]
fn what_cannot_be_known_says_why_instead_of_guessing() {
    let s = space(None);
    let no_db = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.meta),
        Some(&s.wt),
        Duration::from_secs(5),
    );
    assert!(!no_db.contains_key("db"));
    assert!(
        no_db["db_error"].as_str().unwrap().contains("db:"),
        "{no_db:?}"
    );
    let s = space(Some(SLOT_DB));
    let no_agent = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.meta),
        None,
        Duration::from_secs(5),
    );
    assert!(!no_agent.contains_key("agent") && !no_agent.contains_key("db"));
    assert_eq!(no_agent["db_error"], "no active agent");
    let home = gather(&s.dirs, None, None, Some(&s.wt), Duration::from_secs(5));
    assert!(!home.contains_key("space") && !home.contains_key("meta"));
    assert_eq!(home["meta_error"], "this workspace is not a space");
}

#[test]
fn a_run_guides_db_command_runs_only_once_trusted() {
    // a repo's run guide is its author's: opening it must not run its code
    let s = space(None);
    let marker = s.meta.with_file_name("ran");
    let cmd = format!("touch {}; echo postgres://h/d", marker.display());
    put(&s.meta.join(".helmsmith/capture.md"), &guide(Some(&cmd)));
    let untrusted = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.meta),
        Some(&s.wt),
        Duration::from_secs(5),
    );
    assert!(!untrusted.contains_key("db"));
    let why = untrusted["db_error"].as_str().unwrap();
    assert!(
        why.contains("not trusted") && why.contains("herdr-intents trust"),
        "{why}"
    );
    assert!(why.contains(&s.meta.display().to_string()), "{why}");
    assert!(!marker.exists(), "an untrusted db: command never runs");
    assert_eq!(
        trust_db(&s.dirs, &s.meta).as_deref(),
        Ok(cmd.as_str()),
        "says what it trusted"
    );
    let trusted = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.meta),
        Some(&s.wt),
        Duration::from_secs(5),
    );
    assert_eq!(trusted["db"], "postgres://h/d");
    assert!(marker.exists());
}

#[test]
fn a_changed_db_command_must_be_trusted_again() {
    let s = space(Some(SLOT_DB));
    trust_db(&s.dirs, &s.meta).unwrap();
    put(
        &s.meta.join(".helmsmith/capture.md"),
        &guide(Some("echo postgres://elsewhere/d")),
    );
    let got = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.meta),
        Some(&s.wt),
        Duration::from_secs(5),
    );
    assert!(!got.contains_key("db"));
    assert!(
        got["db_error"].as_str().unwrap().contains("not trusted"),
        "{got:?}"
    );
    trust_db(&s.dirs, &s.meta).unwrap();
    let got = gather(
        &s.dirs,
        Some("skoolscout-com"),
        Some(&s.meta),
        Some(&s.wt),
        Duration::from_secs(5),
    );
    assert_eq!(got["db"], "postgres://elsewhere/d");
}

#[test]
fn trusting_a_run_guide_without_a_db_command_says_so() {
    let s = space(None);
    let why = trust_db(&s.dirs, &s.meta).unwrap_err();
    assert!(why.contains("no db: command"), "{why}");
}

#[test]
fn the_space_has_a_default_page_for_the_browser() {
    let s = space(None);
    let guide = |front: &str| format!("---\ndefault_area: org/social\n{front}---\nBody.\n");
    let ctx = |s: &Space| {
        gather(
            &s.dirs,
            Some("skoolscout-com"),
            Some(&s.meta),
            None,
            Duration::from_secs(5),
        )
    };
    put(
        &s.meta.join(".helmsmith/capture.md"),
        &guide("url: https://x.local:{port}\nbrowse: https://demo.x.local:3000\n"),
    );
    assert_eq!(ctx(&s)["browse"], "https://demo.x.local:3000");
    // no browse: a fixed url stands in
    put(
        &s.meta.join(".helmsmith/capture.md"),
        &guide("url: https://staging.x.dev\n"),
    );
    assert_eq!(ctx(&s)["browse"], "https://staging.x.dev");
    // a url that needs a slot's port is no page for the whole space
    put(
        &s.meta.join(".helmsmith/capture.md"),
        &guide("url: https://x.local:{port}\n"),
    );
    let got = ctx(&s);
    assert!(!got.contains_key("browse"));
    assert!(
        got["browse_error"].as_str().unwrap().contains("browse:"),
        "{got:?}"
    );
}
