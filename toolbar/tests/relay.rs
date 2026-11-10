//! The pop-out relay with real pseudo-terminals: a tool running in the
//! toolbar shown on another terminal (a Herdr pane running `herdr-toolbar
//! attach`), at that terminal's size, typing and all, then given back.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use herdr_toolbar::config::ToolConfig;
use herdr_toolbar::pty::{Size, SpawnEnv, ToolProcess};
use herdr_toolbar::relay::{self, Frame};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

fn cat() -> ToolProcess {
    let cfg = ToolConfig {
        id: "t".into(),
        name: "T".into(),
        icon: "x".into(),
        command: ["sh", "-c", "printf 'ready\\n'; exec cat"]
            .map(String::from)
            .to_vec(),
        cwd: None,
        env: BTreeMap::new(),
        open: Default::default(),
        graphics: false,
        panes: vec![],
        group: None,
        on_switch: None,
        popout: None,
        when: None,
    };
    let env = SpawnEnv {
        cwd: std::env::temp_dir(),
        inherit: vec![],
        tool_id: "t".into(),
        cell: None,
    };
    let (tx, rx) = channel();
    // nobody redraws in these tests: keep the wakes from piling up
    std::thread::spawn(move || while rx.recv().is_ok() {});
    let p = ToolProcess::spawn(&cfg, &env, SLOT, tx).unwrap();
    wait_until("the tool starts", || {
        p.screen().contents().contains("ready")
    });
    p
}

const SLOT: Size = Size { rows: 10, cols: 40 };
const PANE: Size = Size {
    rows: 20,
    cols: 100,
};

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Reads `from` until what arrived contains `text`.
fn read_until(from: &mut impl Read, text: &str) -> String {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    let start = Instant::now();
    while !String::from_utf8_lossy(&got).contains(text) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "never saw {text:?} in {:?}",
            String::from_utf8_lossy(&got)
        );
        match from.read(&mut buf) {
            Ok(0) => panic!(
                "closed before {text:?}: {:?}",
                String::from_utf8_lossy(&got)
            ),
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("{e}"),
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

#[test]
fn an_attached_terminal_gets_the_screen_then_the_output_and_its_typing() {
    let mut p = cat();
    let handle = p.handle();
    let (ours, mut theirs) = UnixStream::pair().unwrap();
    theirs
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    handle.attach(ours, PANE).unwrap();
    assert!(p.attached());
    read_until(&mut theirs, "ready"); // the screen as it was
    assert_eq!(p.screen().size(), (PANE.rows, PANE.cols), "the pane's size");

    handle.input(b"ping\n").unwrap();
    read_until(&mut theirs, "ping"); // what the tool printed since

    // the slot resizing meanwhile waits for the tool to come back
    p.resize(Size { rows: 12, cols: 44 });
    assert_eq!(p.screen().size(), (PANE.rows, PANE.cols));
    handle.detach();
    assert!(!p.attached());
    assert_eq!(p.screen().size(), (12, 44), "back at the slot's size");
    p.kill();
}

#[test]
fn serve_shows_the_tool_to_a_client_until_it_leaves() {
    let mut p = cat();
    let dir = tempfile::Builder::new()
        .prefix("tr")
        .tempdir_in("/tmp")
        .unwrap();
    let path = dir.path().join("t.relay.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (wake, _woken) = channel();
    let (handle, s) = (p.handle(), stop.clone());
    std::thread::spawn(move || relay::serve(listener, handle, s, wake));

    let mut client = UnixStream::connect(&path).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    relay::write_frame(&mut client, &Frame::Size(PANE)).unwrap();
    read_until(&mut client, "ready");
    relay::write_frame(&mut client, &Frame::Input(b"pong\n".to_vec())).unwrap();
    read_until(&mut client, "pong");
    relay::write_frame(
        &mut client,
        &Frame::Size(Size {
            rows: 30,
            cols: 120,
        }),
    )
    .unwrap();
    wait_until("the pane's new size", || p.screen().size() == (30, 120));

    drop(client);
    wait_until("given back", || !p.attached());
    assert_eq!(p.screen().size(), (SLOT.rows, SLOT.cols));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    p.kill();
}

#[test]
fn frames_round_trip() {
    let mut wire = Vec::new();
    relay::write_frame(&mut wire, &Frame::Input(b"abc".to_vec())).unwrap();
    relay::write_frame(&mut wire, &Frame::Size(PANE)).unwrap();
    let mut r = &wire[..];
    assert_eq!(
        relay::read_frame(&mut r).unwrap(),
        Some(Frame::Input(b"abc".to_vec()))
    );
    assert_eq!(relay::read_frame(&mut r).unwrap(), Some(Frame::Size(PANE)));
    assert_eq!(relay::read_frame(&mut r).unwrap(), None, "the end");
}

/// The next frame from `from`, waiting at most five seconds.
fn next_frame(from: &mut UnixStream) -> Option<Frame> {
    from.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    relay::read_frame(from).expect("a frame within five seconds")
}

#[test]
fn attach_relays_its_terminal_and_ends_with_the_toolbar() {
    let dir = tempfile::Builder::new()
        .prefix("ta")
        .tempdir_in("/tmp")
        .unwrap();
    let path = dir.path().join("t.relay.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 15,
            cols: 60,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr-toolbar"));
    cmd.args(["attach", &path.display().to_string()]);
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    // what the pane's terminal shows, read off the pty in the background
    let mut pane = pty.master.try_clone_reader().unwrap();
    let (shown_tx, shown) = channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = pane.read(&mut buf) {
            if n == 0 || shown_tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut keys = pty.master.take_writer().unwrap();

    let start = Instant::now();
    let mut toolbar = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(_) => {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "attach never connected"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    toolbar.set_nonblocking(false).unwrap();
    assert_eq!(
        next_frame(&mut toolbar),
        Some(Frame::Size(Size { rows: 15, cols: 60 })),
        "first, its size"
    );
    toolbar.write_all(b"hello from the tool").unwrap();
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("hello from the tool") {
        let more = shown
            .recv_timeout(Duration::from_secs(5))
            .expect("the tool's output on the pane's terminal");
        seen.extend(more);
    }
    keys.write_all(b"k").unwrap();
    assert_eq!(
        next_frame(&mut toolbar),
        Some(Frame::Input(b"k".to_vec())),
        "keys go to the tool as typed"
    );
    drop(toolbar);
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("attach never ended");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn ctrl_bracket_p_in_the_pane_asks_the_toolbar_to_dock_and_never_reaches_the_tool() {
    use herdr_toolbar::app::ControlCmd;
    use herdr_toolbar::herdr::Control;

    let dir = tempfile::Builder::new()
        .prefix("td")
        .tempdir_in("/tmp")
        .unwrap();
    let path = dir.path().join("t.relay.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let control_path = dir.path().join("w1.sock");
    let control = Control::bind(&control_path).unwrap();

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 15,
            cols: 60,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr-toolbar"));
    cmd.args([
        "attach",
        &path.display().to_string(),
        &control_path.display().to_string(),
        "t",
        "ctrl-]",
    ]);
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    // nobody reads the pane's screen here; keep the pty from filling up
    let mut pane = pty.master.try_clone_reader().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while matches!(pane.read(&mut buf), Ok(n) if n > 0) {}
    });
    let mut keys = pty.master.take_writer().unwrap();

    let start = Instant::now();
    let mut toolbar = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(_) => {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "attach never connected"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    toolbar.set_nonblocking(false).unwrap();
    assert!(matches!(next_frame(&mut toolbar), Some(Frame::Size(_))));

    keys.write_all(b"x").unwrap();
    assert_eq!(next_frame(&mut toolbar), Some(Frame::Input(b"x".to_vec())));

    keys.write_all(b"\x1dp").unwrap();
    let mut got = Vec::new();
    let start = Instant::now();
    while got.is_empty() && start.elapsed() < Duration::from_secs(5) {
        got.extend(control.poll());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(got, vec![ControlCmd::Dock("t".into())]);
    toolbar
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    assert!(
        relay::read_frame(&mut toolbar).is_err(),
        "the dock key is not typed into the tool"
    );

    drop(toolbar);
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("attach never ended");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
