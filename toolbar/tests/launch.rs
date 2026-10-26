//! `open_in_pane` against a fake `herdr` binary and a fake Herdr socket.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use herdr_toolbar::config::{Open, ToolConfig};
use herdr_toolbar::herdr::{HerdrContext, launched_pane_file, open_in_pane};

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
    assert_eq!(open_in_pane(&fx.ctx, &browser()).unwrap(), "w1:p12");
    let l = log(&fx);
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
fn a_second_click_focuses_the_pane_it_opened() {
    let fx = fx(Some("w1:p3"), true);
    fs::write(launched_pane_file(&fx.ctx, "browser"), "w1:p12\n").unwrap();
    let listener = UnixListener::bind(&fx.ctx.socket).unwrap();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        stream.write_all(b"{\"id\":\"t\",\"result\":{}}\n").unwrap();
        tx.send(line).unwrap();
    });
    assert_eq!(open_in_pane(&fx.ctx, &browser()).unwrap(), "w1:p12");
    let request: serde_json::Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
    assert_eq!(request["method"], "pane.focus");
    assert_eq!(request["params"]["pane_id"], "w1:p12");
    assert!(!log(&fx).contains("pane split"), "{}", log(&fx));
}

#[test]
fn a_closed_pane_is_opened_again() {
    let fx = fx(Some("w1:p3"), false);
    fs::write(launched_pane_file(&fx.ctx, "browser"), "w1:p12\n").unwrap();
    assert_eq!(open_in_pane(&fx.ctx, &browser()).unwrap(), "w1:p12");
    assert!(log(&fx).contains("pane split --pane w1:p3"), "{}", log(&fx));
}

#[test]
fn without_a_left_neighbour_it_splits_the_focused_pane() {
    let fx = fx(None, false);
    open_in_pane(&fx.ctx, &browser()).unwrap();
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
