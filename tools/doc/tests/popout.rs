//! The pop-out with a real Neovim: the toolbar's slot runs the Editor's
//! Neovim in the background and shows it; a pop-out pane shows the same
//! Neovim, unsaved edits and all, while the slot lets go; the pop-out
//! ending gives it back to the slot. Each side runs in a pty of its own
//! size, so the size Neovim has says who shows it. Skipped without `nvim`.

use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use herdr_doc::popout;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-doc");
const SLOT_COLS: u16 = 40;
const POPOUT_COLS: u16 = 160;

fn have_nvim() -> bool {
    Command::new("nvim")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A workspace (w7) in a temp folder, with no Neovim config of its own.
struct Ws {
    tmp: tempfile::TempDir,
    sock: PathBuf,
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = Command::new("nvim")
            .args(["--clean", "--server"])
            .arg(&self.sock)
            .args(["--remote-expr", "execute('qa!')"])
            .output();
    }
}

fn ws() -> Ws {
    // short paths: a Unix socket's path must fit in 104 bytes on macOS
    let tmp = tempfile::Builder::new()
        .prefix("hp")
        .tempdir_in("/tmp")
        .unwrap();
    let sock = tmp.path().join("cache/nvim-editor-w7.sock");
    Ws { tmp, sock }
}

/// A herdr-doc run in a pty `cols` wide, as the toolbar or a Herdr pane runs
/// it; its output is read and dropped so it never blocks.
struct Term {
    child: Box<dyn Child + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl Ws {
    fn run(&self, args: &[&str], cols: u16) -> Term {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 20,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(BIN);
        cmd.args(args);
        cmd.cwd(self.tmp.path());
        for (k, v) in self.vars() {
            cmd.env(k, v);
        }
        let child = pty.slave.spawn_command(cmd).unwrap();
        let mut reader = pty.master.try_clone_reader().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while matches!(reader.read(&mut buf), Ok(n) if n > 0) {}
        });
        Term {
            child,
            _master: pty.master,
        }
    }

    fn vars(&self) -> Vec<(&'static str, PathBuf)> {
        let t = self.tmp.path();
        vec![
            ("HERDR_WORKSPACE_ID", "w7".into()),
            ("XDG_CACHE_HOME", t.join("cache")),
            ("XDG_STATE_HOME", t.join("state")),
            ("XDG_CONFIG_HOME", t.join("config")),
            ("XDG_DATA_HOME", t.join("data")),
            ("TERM", "xterm-256color".into()),
        ]
    }

    fn expr(&self, e: &str) -> String {
        let out = Command::new("nvim")
            .args(["--clean", "--server"])
            .arg(&self.sock)
            .args(["--remote-expr", e])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The widths of the UIs showing the Editor's Neovim, e.g. `[40]`.
    fn ui_widths(&self) -> String {
        self.expr("luaeval('vim.inspect(vim.tbl_map(function(u) return u.width end, vim.api.nvim_list_uis()))')")
    }
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(10), "never: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn shown_by(w: &Ws, cols: u16) -> impl FnMut() -> bool + '_ {
    move || w.ui_widths() == format!("{{ {cols} }}")
}

#[test]
fn the_slot_lets_go_while_a_popout_holds_neovim_and_takes_it_back_after() {
    if !have_nvim() {
        return;
    }
    let w = ws();
    let mut slot = w.run(&[], SLOT_COLS);
    wait_for("the slot shows Neovim", shown_by(&w, SLOT_COLS));
    // a pop-out (this test, alive) claims it
    let me = std::process::id() as i32;
    popout::claim(&w.sock, me).unwrap();
    wait_for("the slot lets go", || w.ui_widths() == "{}");
    assert_eq!(w.expr("1"), "1", "Neovim keeps running without a UI");
    assert!(slot.child.try_wait().unwrap().is_none(), "the slot waits");
    popout::release(&w.sock, me);
    wait_for("the slot takes it back", shown_by(&w, SLOT_COLS));
}

#[test]
fn a_popout_shows_the_same_neovim_with_unsaved_edits_and_gives_it_back() {
    if !have_nvim() {
        return;
    }
    let w = ws();
    let _slot = w.run(&[], SLOT_COLS);
    wait_for("the slot shows Neovim", shown_by(&w, SLOT_COLS));
    w.expr("execute('enew | call setline(1, \"draft\")')");
    assert_eq!(w.expr("&modified"), "1");

    let mut pop = w.run(&["popout"], POPOUT_COLS);
    let pid = pop.child.process_id().unwrap() as i32;
    wait_for("the pop-out claims it", || {
        popout::holder(&w.sock) == Some(pid)
    });
    wait_for("only the pop-out shows it", shown_by(&w, POPOUT_COLS));
    assert_eq!(w.expr("getline(1)"), "draft", "the same buffer");
    assert_eq!(w.expr("&modified"), "1", "still unsaved");

    // Herdr closing the pane kills it, with no chance to clean up
    pop.child.kill().unwrap();
    wait_for("the slot takes it back", shown_by(&w, SLOT_COLS));
    assert_eq!(w.expr("getline(1)"), "draft");
}

#[test]
fn an_agents_open_while_popped_out_leaves_the_toolbar_alone() {
    if !have_nvim() {
        return;
    }
    let w = ws();
    let _slot = w.run(&[], SLOT_COLS);
    wait_for("the slot shows Neovim", shown_by(&w, SLOT_COLS));
    popout::claim(&w.sock, std::process::id() as i32).unwrap();
    let tb = w
        .tmp
        .path()
        .join("state/herdr/plugins/jefelabs.toolbar/w7.sock");
    std::fs::create_dir_all(tb.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&tb).unwrap();
    listener.set_nonblocking(true).unwrap();
    let file = put(w.tmp.path(), "plan.md", "# Plan\n");

    let mut open = Command::new(BIN);
    open.args(["open", "plan.md"]).current_dir(w.tmp.path());
    for (k, v) in w.vars() {
        open.env(k, v);
    }
    let out = open.env_remove("NVIM").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        listener.accept().is_err(),
        "no `select editor`: the slot is not where the file shows"
    );
    wait_for("the file shown in the pop-out's Neovim", || {
        w.expr("luaeval('vim.b.herdr_doc or \"\"')") == file.display().to_string()
    });
}

fn put(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = std::fs::canonicalize(dir).unwrap().join(name);
    std::fs::write(&p, text).unwrap();
    p
}
