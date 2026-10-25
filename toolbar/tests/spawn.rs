//! Real pseudo-terminal tests for one hosted tool.

use std::collections::BTreeMap;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use herdr_toolbar::config::ToolConfig;
use herdr_toolbar::pty::{Size, SpawnEnv, ToolProcess, ToolState};

fn cfg(cmd: &[&str]) -> ToolConfig {
    ToolConfig {
        id: "t".into(),
        name: "T".into(),
        icon: "x".into(),
        command: cmd.iter().map(|s| s.to_string()).collect(),
        cwd: None,
        env: BTreeMap::new(),
    }
}

fn env() -> SpawnEnv {
    SpawnEnv {
        cwd: std::env::temp_dir(),
        inherit: vec![("HERDR_BIN_PATH".into(), "/x/herdr".into())],
        tool_id: "t".into(),
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn contents(p: &ToolProcess) -> String {
    p.screen().contents()
}

#[test]
fn renders_output_into_the_screen() {
    let (tx, _rx) = channel();
    let mut p = ToolProcess::spawn(
        &cfg(&["sh", "-c", "printf 'hello toolbar'; sleep 30"]),
        &env(),
        Size { rows: 5, cols: 40 },
        tx,
    )
    .unwrap();
    wait_until("output", || contents(&p).contains("hello toolbar"));
    p.kill();
}

#[test]
fn exit_code_is_captured() {
    let (tx, _rx) = channel();
    let p = ToolProcess::spawn(
        &cfg(&["sh", "-c", "exit 3"]),
        &env(),
        Size { rows: 5, cols: 40 },
        tx,
    )
    .unwrap();
    wait_until("exit", || matches!(p.state(), ToolState::Exited { .. }));
    assert_eq!(p.state(), ToolState::Exited { code: Some(3) });
}

#[test]
fn missing_binary_is_failed_with_os_error() {
    let (tx, _rx) = channel();
    let e = ToolProcess::spawn(
        &cfg(&["definitely-not-installed-xyz"]),
        &env(),
        Size { rows: 5, cols: 40 },
        tx,
    )
    .unwrap_err();
    assert!(e.contains("cannot start `T`"), "got: {e}");
    assert!(e.contains("definitely-not-installed-xyz"), "got: {e}");
    assert_eq!(
        e.lines().count(),
        1,
        "one line only, portable-pty's PATH dump is dropped: {e}"
    );
}

#[test]
fn resize_reaches_pty_and_parser() {
    let (tx, _rx) = channel();
    let mut p = ToolProcess::spawn(
        &cfg(&["sh", "-c", "sleep 0.3; stty size; sleep 30"]),
        &env(),
        Size { rows: 5, cols: 40 },
        tx,
    )
    .unwrap();
    p.resize(Size { rows: 7, cols: 33 });
    assert_eq!(p.screen().size(), (7, 33));
    wait_until("stty output", || contents(&p).contains("7 33"));
    p.kill();
}

#[test]
fn env_passthrough_and_tool_id() {
    let (tx, _rx) = channel();
    let mut c = cfg(&[
        "sh",
        "-c",
        "echo $HERDR_TOOLBAR_TOOL_ID $HERDR_BIN_PATH $TERM $FOO ${TERM_PROGRAM:-unset}; sleep 30",
    ]);
    c.env.insert("FOO".into(), "bar".into());
    let mut p = ToolProcess::spawn(&c, &env(), Size { rows: 5, cols: 60 }, tx).unwrap();
    wait_until("echo", || {
        contents(&p).contains("t /x/herdr xterm-256color bar unset")
    });
    p.kill();
}

#[test]
fn kill_leaves_no_child() {
    let (tx, _rx) = channel();
    let mut p = ToolProcess::spawn(
        &cfg(&["sleep", "60"]),
        &env(),
        Size { rows: 5, cols: 40 },
        tx,
    )
    .unwrap();
    let pid = p.pid().expect("pid");
    p.kill();
    wait_until("process gone", || {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_err()
    });
}

#[test]
fn wake_is_sent_on_output() {
    let (tx, rx) = channel();
    let mut p = ToolProcess::spawn(
        &cfg(&["sh", "-c", "echo hi; sleep 30"]),
        &env(),
        Size { rows: 5, cols: 40 },
        tx,
    )
    .unwrap();
    assert!(
        rx.recv_timeout(Duration::from_secs(3)).is_ok(),
        "no wake received"
    );
    p.kill();
}
