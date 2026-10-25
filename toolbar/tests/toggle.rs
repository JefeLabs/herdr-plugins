//! `toggle` and `select` against a fake `herdr` binary.

use std::fs;
use std::path::{Path, PathBuf};

use herdr_toolbar::app::ControlCmd;
use herdr_toolbar::herdr::{Control, HerdrContext, pane_file, select, socket_path, toggle};

fn fake_herdr() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-herdr.sh")
}

struct Fx {
    tmp: tempfile::TempDir,
    ctx: HerdrContext,
    log: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let config = tmp.path().join("config");
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&config).unwrap();
    let ws = tmp.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    let log = tmp.path().join("herdr.log");
    let ctx = HerdrContext {
        bin: fake_herdr(),
        workspace_id: Some("w1".into()),
        pane_id: None,
        target_pane_id: Some("w1:p3".into()),
        workspace_cwd: ws,
        config_dir: config,
        state_dir: state,
        inherit: vec![],
        extra_env: vec![("FAKE_HERDR_LOG".into(), log.display().to_string())],
    };
    Fx { tmp, ctx, log }
}

fn log(fx: &Fx) -> String {
    fs::read_to_string(&fx.log).unwrap_or_default()
}

#[test]
fn toggle_opens_when_no_pane_recorded() {
    let fx = fx();
    toggle(&fx.ctx).unwrap();
    let l = log(&fx);
    let want = format!(
        "plugin pane open --plugin jefelabs.toolbar --entrypoint toolbar --placement split --direction right --target-pane w1:p3 --cwd {}",
        fx.ctx.workspace_cwd.display()
    );
    assert!(l.contains(&want), "log was:\n{l}");
    assert!(!l.contains("--focus"), "must not steal focus:\n{l}");
    assert!(!l.contains("--workspace"), "--workspace makes Herdr 0.9.5 reject the target pane:\n{l}");
    assert_eq!(fs::read_to_string(pane_file(&fx.ctx)).unwrap().trim(), "w1:p9");
}

#[test]
fn toggle_closes_when_pane_recorded_and_alive() {
    let mut fx = fx();
    fs::write(pane_file(&fx.ctx), "w1:p4\n").unwrap();
    fx.ctx.extra_env.push(("FAKE_PANE_EXISTS".into(), "1".into()));
    toggle(&fx.ctx).unwrap();
    let l = log(&fx);
    assert!(l.contains("pane close w1:p4"), "log was:\n{l}");
    assert!(!l.contains("plugin pane open"), "log was:\n{l}");
    assert!(!pane_file(&fx.ctx).exists());
}

#[test]
fn toggle_reopens_when_recorded_pane_is_gone() {
    let fx = fx();
    fs::write(pane_file(&fx.ctx), "w1:p4\n").unwrap();
    toggle(&fx.ctx).unwrap();
    let l = log(&fx);
    assert!(l.contains("pane get w1:p4"), "log was:\n{l}");
    assert!(l.contains("plugin pane open"), "log was:\n{l}");
    assert_eq!(fs::read_to_string(pane_file(&fx.ctx)).unwrap().trim(), "w1:p9");
}

#[test]
fn select_reaches_a_listening_control_socket() {
    let fx = fx();
    let control = Control::bind(&socket_path(&fx.ctx)).unwrap();
    select(&fx.ctx, "db").unwrap();
    let mut got = Vec::new();
    for _ in 0..50 {
        got.extend(control.poll());
        if !got.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(got, vec![ControlCmd::Select("db".into())]);
    drop(fx.tmp);
}

#[test]
fn select_without_a_toolbar_is_an_error() {
    let fx = fx();
    let e = select(&fx.ctx, "db").unwrap_err().to_string();
    assert!(e.contains("not open"), "got: {e}");
}

#[test]
fn from_env_reads_context_json() {
    let tmp = tempfile::tempdir().unwrap();
    let vars = [
        ("HERDR_BIN_PATH", "/x/herdr".to_string()),
        ("HERDR_SOCKET_PATH", "/x/sock".to_string()),
        ("HERDR_ENV", "1".to_string()),
        ("HERDR_WORKSPACE_ID", "w7".to_string()),
        ("HERDR_PANE_ID", "w7:p2".to_string()),
        ("HERDR_PLUGIN_CONFIG_DIR", tmp.path().join("c").display().to_string()),
        ("HERDR_PLUGIN_STATE_DIR", tmp.path().join("s").display().to_string()),
        ("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_cwd":"/w","focused_pane_cwd":"/w/tab","focused_pane_id":"w7:p5"}"#.to_string()),
    ];
    let ctx = HerdrContext::from_vars(|k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())).unwrap();
    assert_eq!(ctx.workspace_cwd, Path::new("/w"), "workspace_cwd, never focused_pane_cwd");
    assert_eq!(ctx.workspace_id.as_deref(), Some("w7"));
    assert_eq!(ctx.pane_id.as_deref(), Some("w7:p2"));
    assert_eq!(ctx.target_pane_id.as_deref(), Some("w7:p5"), "focused pane from the action context");
    assert_eq!(ctx.bin, Path::new("/x/herdr"));
    assert!(ctx.inherit.iter().any(|(k, v)| k == "HERDR_BIN_PATH" && v == "/x/herdr"));
    assert!(ctx.inherit.iter().any(|(k, _)| k == "HERDR_SOCKET_PATH"));
    assert!(ctx.inherit.iter().any(|(k, _)| k == "HERDR_ENV"));
    assert_eq!(pane_file(&ctx), tmp.path().join("s/w7.pane"));
    assert_eq!(socket_path(&ctx), tmp.path().join("s/w7.sock"));
}

#[test]
fn toggle_without_a_target_pane_is_a_clear_error() {
    let mut fx = fx();
    fx.ctx.target_pane_id = None;
    let e = toggle(&fx.ctx).unwrap_err().to_string();
    assert!(e.contains("no pane to split"), "got: {e}");
}

#[test]
fn from_env_without_herdr_is_an_error() {
    let e = HerdrContext::from_vars(|_| None).unwrap_err().to_string();
    assert!(e.contains("HERDR_BIN_PATH"), "got: {e}");
}
