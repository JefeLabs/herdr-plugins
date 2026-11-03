//! A pop-out whose tool has a `popout` command of its own (a browser that
//! draws in the pane) runs `herdr-toolbar wrap <control> <tool> <prefix> --
//! <command…>`: the command gets a terminal of its own, at the pane's size,
//! and the pane keeps the toolbar's leave keys. With real pseudo-terminals.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use herdr_toolbar::app::ControlCmd;
use herdr_toolbar::herdr::Control;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

fn size(rows: u16, cols: u16, pixel_width: u16, pixel_height: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width,
        pixel_height,
    }
}

/// `herdr-toolbar wrap` for tool `t` (prefix `Ctrl-]`) on a terminal like
/// a Herdr pane's, and what that terminal has shown.
struct Pane {
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    keys: Box<dyn Write + Send>,
    shown: Receiver<Vec<u8>>,
    seen: Vec<u8>,
}

impl Pane {
    fn wrap(control: &Path, command: &[&str], at: PtySize) -> Pane {
        let pty = native_pty_system().openpty(at).unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr-toolbar"));
        cmd.args(["wrap", &control.display().to_string(), "t", "ctrl-]", "--"]);
        cmd.args(command);
        let child = pty.slave.spawn_command(cmd).unwrap();
        let mut reader = pty.master.try_clone_reader().unwrap();
        let (tx, shown) = channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let keys = pty.master.take_writer().unwrap();
        Pane {
            child,
            master: pty.master,
            keys,
            shown,
            seen: Vec::new(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen).into_owned()
    }

    /// Waits until the terminal has shown `text`.
    fn wait_for(&mut self, text: &str) {
        let start = Instant::now();
        while !self.text().contains(text) {
            let left = Duration::from_secs(5).saturating_sub(start.elapsed());
            match self.shown.recv_timeout(left) {
                Ok(more) => self.seen.extend(more),
                Err(_) => panic!("never saw {text:?} in {:?}", self.text()),
            }
        }
    }

    /// Everything shown over the next `time`.
    fn watch(&mut self, time: Duration) {
        let start = Instant::now();
        while let Some(left) = time.checked_sub(start.elapsed()) {
            if let Ok(more) = self.shown.recv_timeout(left) {
                self.seen.extend(more);
            }
        }
    }

    fn type_keys(&mut self, bytes: &[u8]) {
        self.keys.write_all(bytes).unwrap();
        self.keys.flush().unwrap();
    }

    /// The wrapper's exit code, once it has ended (five seconds at most).
    fn exit_code(&mut self) -> u32 {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.exit_code();
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the wrapper never ended; it showed {:?}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// A toolbar's control socket to tell, in a short folder (sockets have a
/// short path limit).
fn toolbar() -> (tempfile::TempDir, PathBuf, Control) {
    let dir = tempfile::Builder::new()
        .prefix("tw")
        .tempdir_in("/tmp")
        .unwrap();
    let path = dir.path().join("w1.sock");
    let control = Control::bind(&path).unwrap();
    (dir, path, control)
}

/// What the toolbar was told, waiting up to five seconds for something.
fn told(control: &Control) -> Vec<ControlCmd> {
    let mut got = Vec::new();
    let start = Instant::now();
    while got.is_empty() && start.elapsed() < Duration::from_secs(5) {
        got.extend(control.poll());
        std::thread::sleep(Duration::from_millis(20));
    }
    got
}

/// A shell that prints `ready`, then hands what is typed to `cat`: the
/// terminal echoes each key, a control key as `^]` and the like.
const CAT: [&str; 3] = ["sh", "-c", "echo ready; exec cat"];

#[test]
fn the_command_shows_and_types_through_and_ctrl_bracket_d_docks() {
    let (_dir, path, control) = toolbar();
    let mut pane = Pane::wrap(&path, &CAT, size(15, 60, 0, 0));
    pane.wait_for("ready");
    pane.type_keys(b"hi\r");
    pane.wait_for("hi\r\nhi"); // the echo, then cat printing the line back
    // the prefix twice types it once: this terminal shows a typed Ctrl-] as ^]
    pane.type_keys(b"\x1d\x1d");
    pane.wait_for("^]");

    pane.type_keys(b"\x1dd");
    assert_eq!(told(&control), vec![ControlCmd::Dock("t".into())]);
    pane.watch(Duration::from_millis(300));
    assert_eq!(
        pane.text().matches("^]").count(),
        1,
        "neither the prefix nor d reached the command: {:?}",
        pane.text()
    );
}

#[test]
fn ctrl_bracket_c_asks_the_toolbar_to_close_the_pane() {
    let (_dir, path, control) = toolbar();
    let mut pane = Pane::wrap(&path, &CAT, size(15, 60, 0, 0));
    pane.wait_for("ready");
    pane.type_keys(b"\x1dc");
    assert_eq!(told(&control), vec![ControlCmd::Close("t".into())]);
    pane.watch(Duration::from_millis(300));
    assert!(!pane.text().contains("^]"), "{:?}", pane.text());
}

#[test]
fn the_command_gets_the_panes_size_in_cells_and_pixels_and_follows_it() {
    // a browser draws images to fit, so it needs the pixels as well
    let (dir, path, _control) = toolbar();
    let script = dir.path().join("size.py");
    std::fs::write(
        &script,
        "import fcntl, signal, struct, termios, time\n\
         def show(*_):\n\
         \x20   r, c, w, h = struct.unpack('4H', fcntl.ioctl(0, termios.TIOCGWINSZ, bytes(8)))\n\
         \x20   print(f'size {r}x{c} {w}x{h}px', flush=True)\n\
         signal.signal(signal.SIGWINCH, show)\n\
         show()\n\
         while True:\n\
         \x20   time.sleep(1)\n",
    )
    .unwrap();
    let mut pane = Pane::wrap(
        &path,
        &["python3", &script.display().to_string()],
        size(15, 60, 600, 300),
    );
    pane.wait_for("size 15x60 600x300px");
    pane.master.resize(size(20, 80, 800, 400)).unwrap();
    pane.wait_for("size 20x80 800x400px");
}

#[test]
fn the_wrapper_ends_with_its_commands_exit_status() {
    let (_dir, path, _control) = toolbar();
    let mut pane = Pane::wrap(&path, &["sh", "-c", "exit 3"], size(15, 60, 0, 0));
    assert_eq!(pane.exit_code(), 3);
}

#[test]
fn with_no_toolbar_to_tell_a_leave_key_ends_the_command() {
    let (dir, _path, _control) = toolbar();
    let nobody = dir.path().join("nobody.sock");
    let mut pane = Pane::wrap(&nobody, &CAT, size(15, 60, 0, 0));
    pane.wait_for("ready");
    pane.type_keys(b"\x1dd");
    pane.exit_code(); // ends, rather than leaving a pane nothing can close
}
