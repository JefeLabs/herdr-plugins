//! Drive the real `herdr-toolbar` binary inside a pseudo-terminal: the checks
//! the unit tests cannot make (terminal setup sequences, hang-up behaviour).

use std::fs;
use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const UNIQUE_SLEEP: &str = "61234";

struct Fx {
    _tmp: tempfile::TempDir,
    cmd: CommandBuilder,
    pid_file: std::path::PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let state = tmp.path().join("state");
    let ws = tmp.path().join("ws");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&ws).unwrap();
    let pid_file = state.join("tool.pid");
    fs::write(
        config.join("tools.toml"),
        format!(
            "[[tool]]\nid = \"s\"\nname = \"S\"\nicon = \"s\"\ncommand = [\"sh\", \"-c\", \"echo $$ > {}; exec sleep {UNIQUE_SLEEP}\"]\n",
            pid_file.display()
        ),
    )
    .unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr-toolbar"));
    cmd.env("HERDR_BIN_PATH", "/usr/bin/true");
    cmd.env("HERDR_PLUGIN_CONFIG_DIR", &config);
    cmd.env("HERDR_PLUGIN_STATE_DIR", &state);
    cmd.env("HERDR_WORKSPACE_ID", "wtest");
    cmd.env(
        "HERDR_PLUGIN_CONTEXT_JSON",
        format!(r#"{{"workspace_cwd":"{}"}}"#, ws.display()),
    );
    cmd.env("TERM", "xterm-256color");
    cmd.cwd(&ws);
    Fx {
        _tmp: tmp,
        cmd,
        pid_file,
    }
}

fn alive(pid: u32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
}

/// The hosted tool's pid, once it has written it.
fn tool_pid(fx: &Fx) -> Option<u32> {
    fs::read_to_string(&fx.pid_file).ok()?.trim().parse().ok()
}

fn wait_until(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < limit, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Spawn the toolbar in a pty; returns (master, child, receiver of everything it wrote).
type Spawned = (
    Box<dyn portable_pty::MasterPty + Send>,
    Box<dyn portable_pty::Child + Send + Sync>,
    mpsc::Receiver<Vec<u8>>,
);

fn spawn(fx: &Fx) -> Spawned {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 20,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let child = pair.slave.spawn_command(fx.cmd.clone()).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    (pair.master, child, rx)
}

fn collect(rx: &mpsc::Receiver<Vec<u8>>, for_: Duration) -> Vec<u8> {
    let mut out = Vec::new();
    let deadline = Instant::now() + for_;
    while Instant::now() < deadline {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(50)) {
            out.extend(chunk);
        }
    }
    out
}

#[test]
fn enables_bracketed_paste_on_start() {
    let fx = fx();
    let (master, mut child, rx) = spawn(&fx);
    let out = collect(&rx, Duration::from_millis(1500));
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("\x1b[?2004h"),
        "no EnableBracketedPaste sequence in startup output"
    );
    let _ = child.kill();
    drop(master);
}

#[test]
fn hang_up_of_the_terminal_ends_the_toolbar_and_its_tools_within_a_second() {
    let fx = fx();
    // No reader clone here: the hang-up only happens once every master fd is closed.
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 20,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut child = pair.slave.spawn_command(fx.cmd.clone()).unwrap();
    drop(pair.slave);
    wait_until("tool running", Duration::from_secs(5), || {
        tool_pid(&fx).is_some()
    });
    let tool = tool_pid(&fx).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().unwrap().is_none(),
        "toolbar must still be running before the hang-up"
    );
    // Herdr closing the pane: the pty master goes away (kernel sends SIGHUP to the session).
    drop(pair.master);
    // try_wait reaps, so a zombie counts as gone (a kill(pid, 0) probe would not).
    wait_until("toolbar process gone", Duration::from_millis(1500), || {
        child.try_wait().unwrap().is_some()
    });
    let status = child.try_wait().unwrap().unwrap();
    assert!(
        status.success(),
        "toolbar must exit cleanly on hang-up, got {status:?}"
    );
    wait_until(
        &format!("tool {tool} gone"),
        Duration::from_millis(1500),
        || !alive(tool),
    );
}
