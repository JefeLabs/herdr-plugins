//! `toggle_pane` against a fake `herdr` binary.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use herdr_toolbar::config::{Open, ToolConfig};
use herdr_toolbar::herdr::{HerdrContext, PaneToggle, launched_pane_file, toggle_pane};

fn fake_herdr() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-herdr.sh")
}

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: HerdrContext,
    log: PathBuf,
}

/// The toolbar is pane w1:p9; the work pane on its left is w1:p3 unless
/// `neighbour` is None.
fn fx(neighbour: Option<&str>, pane_exists: bool) -> Fx {
    fx_labelled(neighbour, pane_exists, "Browser")
}

/// As `fx`, with the recorded pane carrying `label`.
fn fx_labelled(neighbour: Option<&str>, pane_exists: bool, label: &str) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let ws = tmp.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    let log = tmp.path().join("herdr.log");
    let mut extra = vec![("FAKE_HERDR_LOG".to_string(), log.display().to_string())];
    if let Some(n) = neighbour {
        extra.push(("FAKE_NEIGHBOR".into(), n.into()));
    }
    if pane_exists {
        extra.push(("FAKE_PANE_EXISTS".into(), "1".into()));
        extra.push(("FAKE_PANE_LABEL".into(), label.into()));
    }
    let ctx = HerdrContext {
        bin: fake_herdr(),
        workspace_id: Some("w1".into()),
        pane_id: Some("w1:p9".into()),
        tab_id: Some("w1:t1".into()),
        target_pane_id: Some("w1:p5".into()),
        workspace_cwd: ws,
        config_dir: tmp.path().join("config"),
        state_dir: state,
        socket: tmp.path().join("herdr.sock"),
        inherit: vec![],
        extra_env: extra,
    };
    Fx {
        _tmp: tmp,
        ctx,
        log,
    }
}

fn browser() -> ToolConfig {
    ToolConfig {
        id: "browser".into(),
        name: "Browser".into(),
        icon: "o".into(),
        command: vec![
            "terminal-browser".into(),
            "open".into(),
            "https://example.com/a b".into(),
        ],
        cwd: None,
        env: BTreeMap::from([("TB_X".to_string(), "1".to_string())]),
        open: Open::Pane,
        graphics: false,
    }
}

fn log(fx: &Fx) -> String {
    fs::read_to_string(&fx.log).unwrap_or_default()
}

#[test]
fn a_pane_tool_opens_beside_the_work_pane_and_runs_its_command() {
    let fx = fx(Some("w1:p3"), false);
    assert_eq!(
        toggle_pane(&fx.ctx, &browser()).unwrap(),
        PaneToggle::Opened("w1:p12".into())
    );
    let l = log(&fx);
    assert!(
        l.contains("pane rename w1:p12 Browser"),
        "labelled, so a later press knows the pane is ours: {l}"
    );
    assert!(
        l.contains("pane neighbor --pane w1:p9 --direction left"),
        "{l}"
    );
    let split = format!(
        "pane split --pane w1:p3 --direction right --focus --cwd {} --env TB_X=1",
        fx.ctx.workspace_cwd.display()
    );
    assert!(l.contains(&split), "{l}");
    assert!(
        l.contains("pane run w1:p12 'terminal-browser' 'open' 'https://example.com/a b'"),
        "{l}"
    );
    let recorded = fs::read_to_string(launched_pane_file(&fx.ctx, "browser")).unwrap();
    assert_eq!(recorded.trim(), "w1:p12");
}

#[test]
fn a_second_press_closes_the_pane_it_opened() {
    let fx = fx(Some("w1:p3"), true);
    let record = launched_pane_file(&fx.ctx, "browser");
    fs::write(&record, "w1:p12\n").unwrap();
    assert_eq!(
        toggle_pane(&fx.ctx, &browser()).unwrap(),
        PaneToggle::Closed("w1:p12".into())
    );
    let l = log(&fx);
    assert!(l.contains("pane close w1:p12"), "{l}");
    assert!(!l.contains("pane split"), "{l}");
    assert!(!record.exists(), "forgotten, so the next press opens one");
}

#[test]
fn a_pane_that_is_no_longer_the_tools_is_never_closed() {
    // after a Herdr restart the recorded id may name someone else's pane
    let fx = fx_labelled(Some("w1:p3"), true, "claude");
    fs::write(launched_pane_file(&fx.ctx, "browser"), "w1:p12\n").unwrap();
    assert_eq!(
        toggle_pane(&fx.ctx, &browser()).unwrap(),
        PaneToggle::Opened("w1:p12".into())
    );
    let l = log(&fx);
    assert!(!l.contains("pane close"), "{l}");
    assert!(
        l.contains("pane split --pane w1:p3"),
        "a fresh pane instead: {l}"
    );
}

#[test]
fn a_closed_pane_is_opened_again() {
    let fx = fx(Some("w1:p3"), false);
    fs::write(launched_pane_file(&fx.ctx, "browser"), "w1:p12\n").unwrap();
    assert_eq!(
        toggle_pane(&fx.ctx, &browser()).unwrap(),
        PaneToggle::Opened("w1:p12".into())
    );
    assert!(log(&fx).contains("pane split --pane w1:p3"), "{}", log(&fx));
}

#[test]
fn without_a_left_neighbour_it_splits_the_focused_pane() {
    let fx = fx(None, false);
    toggle_pane(&fx.ctx, &browser()).unwrap();
    assert!(log(&fx).contains("pane split --pane w1:p5"), "{}", log(&fx));
}

#[test]
fn tool_ids_cannot_leave_the_state_folder() {
    let fx = fx(None, false);
    let file = launched_pane_file(&fx.ctx, "../../x");
    assert_eq!(file.parent().unwrap(), fx.ctx.state_dir.as_path());
}

#[test]
fn arguments_are_quoted_for_the_shell() {
    use herdr_toolbar::herdr::shell_quote;
    assert_eq!(
        shell_quote(&["open".into(), "it's here".into()]),
        "'open' 'it'\\''s here'"
    );
}
