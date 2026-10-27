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
        open: Default::default(),
        graphics: false,
    }
}

fn env() -> SpawnEnv {
    SpawnEnv {
        cwd: std::env::temp_dir(),
        inherit: vec![("HERDR_BIN_PATH".into(), "/x/herdr".into())],
        tool_id: "t".into(),
        cell: None,
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

#[test]
fn parent_environment_is_inherited_except_graphics_identifiers() {
    // SAFETY: tests in this binary that read the environment do so through the child.
    unsafe {
        std::env::set_var("TOOLBAR_TEST_SSH_AUTH_SOCK", "/run/agent.sock");
        std::env::set_var("TERM_PROGRAM", "ghostty");
        std::env::set_var("KITTY_WINDOW_ID", "7");
        std::env::set_var("WEZTERM_PANE", "3");
    }
    let (tx, _rx) = channel();
    let mut p = ToolProcess::spawn(
        &cfg(&["sh", "-c", "echo \"[$TOOLBAR_TEST_SSH_AUTH_SOCK][${TERM_PROGRAM:-unset}][${KITTY_WINDOW_ID:-unset}][${WEZTERM_PANE:-unset}]\"; sleep 30"]),
        &env(),
        Size { rows: 5, cols: 60 },
        tx,
    )
    .unwrap();
    wait_until("echo", || {
        contents(&p).contains("[/run/agent.sock][unset][unset][unset]")
    });
    p.kill();
}

#[test]
fn a_graphics_tool_keeps_its_image_commands_and_gets_its_answers() {
    let dir = tempfile::tempdir().unwrap();
    let answers = dir.path().join("answers");
    // raw mode, some text, an image, then the two questions terminal-browser
    // asks before drawing; whatever comes back is saved
    let script = r"stty raw -echo; printf 'abc\033_Ga=T,f=100;AAAA\033\\'; printf '\033_Gi=7,a=q;AAAA\033\\\033[c'; head -c 20 > OUT; sleep 30"
        .replace("OUT", &answers.display().to_string());
    let mut c = cfg(&["sh", "-c", &script]);
    c.graphics = true;
    let (tx, _rx) = channel();
    let env = SpawnEnv {
        cell: Some((8, 16)),
        ..env()
    };
    let mut p = ToolProcess::spawn(&c, &env, Size { rows: 5, cols: 40 }, tx).unwrap();
    let mut got = Vec::new();
    let start = Instant::now();
    while got.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(3), "no image command");
        got.extend(p.take_graphics());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        got,
        vec![(b"\x1b_Ga=T,f=100;AAAA\x1b\\".to_vec(), 0, 3)],
        "the image, at the cursor after `abc`; the query is answered, not shown"
    );
    wait_until("the answers", || {
        std::fs::read(&answers).map(|a| a.len()).unwrap_or(0) == 20
    });
    assert_eq!(
        std::fs::read(&answers).unwrap(),
        b"\x1b_Gi=7;OK\x1b\\\x1b[?62;22c".to_vec()
    );
    let text = contents(&p);
    assert!(text.contains("abc") && !text.contains("AAAA"), "{text:?}");
    p.kill();
}
