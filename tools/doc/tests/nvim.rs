//! The document view in a real (headless) Neovim: an agent's `open` shows
//! the file formatted, `v` edits it in the same Neovim, `:Doc` goes back,
//! and the toolbar is asked to show the tool. Skipped without `nvim`.

use std::fs;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-doc");

fn have_nvim() -> bool {
    Command::new("nvim")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Session {
    tmp: tempfile::TempDir,
    nvim: Child,
    sock: PathBuf,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.nvim.kill();
        let _ = self.nvim.wait();
    }
}

impl Session {
    /// The environment a pane in workspace w7 has, pointed into the temp folder.
    fn env<'a>(&'a self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env("HERDR_WORKSPACE_ID", "w7")
            .env("XDG_CACHE_HOME", self.tmp.path().join("cache"))
            .env("XDG_STATE_HOME", self.tmp.path().join("state"))
            .env_remove("NVIM")
    }
    fn expr(&self, e: &str) -> String {
        let out = Command::new("nvim")
            .args([
                "--server",
                &self.sock.display().to_string(),
                "--remote-expr",
                e,
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn wait_for(&self, what: &str, e: &str, want: &str) {
        let start = Instant::now();
        loop {
            let got = self.expr(e);
            if got == want {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{what}: {e} = {got:?}, want {want:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A headless Neovim started the way the pane starts it, on w7's socket.
fn session() -> Session {
    // short paths: a Unix socket's path must fit in 104 bytes on macOS
    let tmp = tempfile::Builder::new()
        .prefix("hd")
        .tempdir_in("/tmp")
        .unwrap();
    let sock = tmp.path().join("cache/nvim-editor-w7.sock");
    fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let mut args = herdr_doc::nvim::args(&sock);
    args.insert(0, "--headless".into());
    args.insert(1, "--clean".into());
    let nvim = Command::new("nvim")
        .args(&args)
        .env("HERDR_DOC_BIN", BIN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let s = Session { tmp, nvim, sock };
    let start = Instant::now();
    while s.expr("1") != "1" {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "nvim never listened"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    s
}

fn put(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = fs::canonicalize(dir).unwrap().join(name);
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn an_agents_open_shows_the_file_and_asks_the_toolbar_for_the_tool() {
    if !have_nvim() {
        return;
    }
    let s = session();
    // the workspace's toolbar, listening for `select`
    let tb = s
        .tmp
        .path()
        .join("state/herdr/plugins/jefelabs.toolbar/w7.sock");
    fs::create_dir_all(tb.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&tb).unwrap();
    let file = put(s.tmp.path(), "plan.md", "# Plan\n");
    let out = s
        .env(
            Command::new(BIN)
                .args(["open", "plan.md"])
                .current_dir(s.tmp.path()),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (mut conn, _) = listener.accept().unwrap();
    let mut said = String::new();
    conn.read_to_string(&mut said).unwrap();
    assert_eq!(said, "select editor\n");
    s.wait_for(
        "a view of the file",
        "luaeval('vim.b.herdr_doc or \"\"')",
        &file.display().to_string(),
    );
    assert_eq!(
        s.expr("&buftype"),
        "terminal",
        "the formatted view runs in a terminal buffer"
    );
}

#[test]
fn v_edits_the_file_in_the_same_neovim_and_doc_goes_back_to_the_view() {
    if !have_nvim() {
        return;
    }
    let s = session();
    let file = put(s.tmp.path(), "data.json", "{\"a\":1}\n");
    let out = s
        .env(Command::new(BIN).args(["open", &file.display().to_string()]))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    s.wait_for(
        "the view",
        "luaeval('vim.b.herdr_doc or \"\"')",
        &file.display().to_string(),
    );
    // `v` in less runs VISUAL with +line and the file, with $NVIM set
    let edit = Command::new(BIN)
        .args(["edit", "+1", &file.display().to_string()])
        .env("NVIM", &s.sock)
        .output()
        .unwrap();
    assert!(
        edit.status.success(),
        "{}",
        String::from_utf8_lossy(&edit.stderr)
    );
    s.wait_for(
        "editing the file",
        "expand('%:p')",
        &file.display().to_string(),
    );
    assert_eq!(s.expr("&buftype"), "", "a normal buffer, not the view");
    s.expr("execute('Doc')");
    s.wait_for("back to the view", "&buftype", "terminal");
}

#[test]
fn a_view_only_file_says_so_and_stays_in_view() {
    let tmp = tempfile::tempdir().unwrap();
    let png = put(tmp.path(), "shot.png", "x");
    let out = Command::new(BIN)
        .args(["edit", "+1", &png.display().to_string()])
        .env_remove("NVIM")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("View-only: shot.png"));
}

#[test]
fn a_file_named_like_code_stays_a_name() {
    if !have_nvim() {
        return;
    }
    let s = session();
    let file = put(s.tmp.path(), "it's|!touch PWNED.md", "# x\n");
    let out = s
        .env(Command::new(BIN).args(["open", &file.display().to_string()]))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    s.wait_for(
        "the view of that very file",
        "luaeval('vim.b.herdr_doc or \"\"')",
        &file.display().to_string(),
    );
    assert!(!s.tmp.path().join("PWNED").exists() && !Path::new("PWNED").exists());
}

#[test]
fn opening_a_file_that_is_not_there_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .args(["open", "missing.md"])
        .current_dir(tmp.path())
        .env("HERDR_WORKSPACE_ID", "w7")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("missing.md"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
