//! herdr-doc, the toolbar's document tool:
//! - `herdr-doc` (the pane): Neovim on the workspace's Editor socket, with
//!   the document view;
//! - `herdr-doc popout`: the same Neovim in a pane of its own (`Ctrl-] p`
//!   in the toolbar), while the slot lets go;
//! - `herdr-doc open <file>`: what an agent runs to show you a file;
//! - `herdr-doc view <file>`: `less` over the formatted view;
//! - `herdr-doc render <file>`: the formatted view (for `less`);
//! - `herdr-doc edit [+line] <file>`: `v` in the view.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use herdr_doc::{kind, nvim, paths, popout, render, vim};

fn get(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

fn me() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "herdr-doc".into())
}

/// `text` quoted for `sh`, for the commands `less` runs through a shell.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Waits up to ten seconds for a Neovim on `socket`.
fn await_nvim(socket: &Path) -> bool {
    let started = Instant::now();
    while !nvim::answers(socket) {
        if started.elapsed() > Duration::from_secs(10) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// A UI on `socket`'s Neovim, in this terminal.
fn attach(socket: &Path) -> std::io::Result<std::process::Child> {
    Command::new("nvim")
        .arg("--server")
        .arg(socket)
        .arg("--remote-ui")
        .spawn()
}

fn popped_out(socket: &Path) -> bool {
    popout::popped_out(socket, popout::alive)
}

/// The pane (the toolbar's slot): the workspace's Neovim, started in the
/// background on its socket unless one already listens there (a stale
/// socket removed first), shown here while no pop-out holds it. `files`
/// open as usual in a Neovim it starts. It ends when its UI does: Neovim
/// quit, or `:detach`. Neovim is in this pty's process group, so closing
/// the slot stops it, as before.
fn pane(files: &[String]) -> anyhow::Result<()> {
    let socket = paths::nvim_socket(get);
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if !nvim::answers(&socket) {
        let _ = std::fs::remove_file(&socket);
        Command::new("nvim")
            .arg("--headless")
            .args(nvim::args(&socket))
            .args(files)
            .env("HERDR_DOC_BIN", me())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot start nvim: {e}"))?;
        anyhow::ensure!(
            await_nvim(&socket),
            "nvim never listened on {}",
            socket.display()
        );
    }
    loop {
        if popped_out(&socket) {
            hold(&socket)?;
            if !nvim::answers(&socket) {
                return Ok(());
            }
            continue;
        }
        let mut ui = attach(&socket)?;
        let let_go = loop {
            if ui.try_wait()?.is_some() {
                break false;
            }
            if popped_out(&socket) {
                // SIGTERM: the UI restores the terminal on its way out
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(ui.id() as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
                let _ = ui.wait();
                break true;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        if !let_go {
            return Ok(());
        }
    }
}

/// The slot while a pop-out holds Neovim: a note, keys swallowed (they
/// would reach Neovim later otherwise), until the pop-out ends or Neovim
/// does.
fn hold(socket: &Path) -> anyhow::Result<()> {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use nix::sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr};
    use std::os::fd::AsFd;

    let stdin = std::io::stdin();
    let saved = tcgetattr(stdin.as_fd()).ok();
    if let Some(t) = &saved {
        let mut raw = t.clone();
        cfmakeraw(&mut raw);
        let _ = tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &raw);
    }
    let mut out = std::io::stdout();
    let _ = write!(
        out,
        "\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H\r\n  Shown in the pop-out \u{2197}\r\n\r\n  \x1b[2mClose that pane to bring it back here.\x1b[0m"
    );
    let _ = out.flush();
    let mut buf = [0u8; 1024];
    while popped_out(socket) && nvim::answers(socket) {
        let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        if poll(&mut fds, PollTimeout::from(200u16)).unwrap_or(0) > 0 {
            match nix::unistd::read(stdin.as_fd(), &mut buf) {
                Ok(0) | Err(_) => break, // the terminal is gone
                Ok(_) => {}
            }
        }
    }
    let _ = write!(out, "\x1b[2J\x1b[H\x1b[?25h\x1b[?1049l");
    let _ = out.flush();
    if let Some(t) = &saved {
        let _ = tcsetattr(stdin.as_fd(), SetArg::TCSANOW, t);
    }
    Ok(())
}

/// The pop-out pane: a UI on the workspace's Neovim, holding it (the slot
/// lets go) until the UI ends. A pane Herdr closes kills this first; the
/// slot sees the marker's process gone.
fn popout_pane() -> anyhow::Result<()> {
    let socket = paths::nvim_socket(get);
    anyhow::ensure!(
        await_nvim(&socket),
        "the Editor is not running in this workspace (no Neovim at {})",
        socket.display()
    );
    let me = std::process::id() as i32;
    popout::claim(&socket, me)?;
    let shown = attach(&socket).and_then(|mut ui| ui.wait());
    popout::release(&socket, me);
    shown?;
    Ok(())
}

/// The view: `less` over the formatted file, `v` editing it.
fn view(file: &str) -> anyhow::Result<()> {
    let err = Command::new("less")
        .args(["-R", file])
        .env("LESSOPEN", format!("|{} render %s", sh_quote(&me())))
        .env("VISUAL", format!("{} edit", sh_quote(&me())))
        .exec();
    Err(anyhow::anyhow!("cannot start less: {err}"))
}

/// `v` in the view: an editable file opens in the session's Neovim (or a
/// Neovim of its own outside one); anything else is view-only.
fn edit(args: &[String]) -> anyhow::Result<()> {
    let Some(file) = args.last() else {
        anyhow::bail!("edit: which file?");
    };
    let path = Path::new(file);
    if !kind::kind(path).editable() {
        let name = path
            .file_name()
            .map_or(file.clone(), |n| n.to_string_lossy().into_owned());
        let mut out = std::io::stdout();
        let _ = writeln!(out, "\n  View-only: {name}");
        let _ = out.flush();
        std::thread::sleep(Duration::from_secs(1));
        return Ok(());
    }
    match get("NVIM").filter(|s| !s.is_empty()) {
        Some(server) => {
            let lit = vim::string(&absolute(path)?.display().to_string())
                .ok_or_else(|| anyhow::anyhow!("{file}: a control character in its name"))?;
            let done = Command::new("nvim")
                .args([
                    "--server",
                    &server,
                    "--remote-expr",
                    &format!("v:lua.HerdrDoc.edit({lit})"),
                ])
                .output()?;
            anyhow::ensure!(done.status.success(), "Neovim did not take the file");
            Ok(())
        }
        None => {
            let err = Command::new("nvim").args(args).exec();
            Err(anyhow::anyhow!("cannot start nvim: {err}"))
        }
    }
}

fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

/// What an agent runs: the toolbar shows the document tool (starting it
/// if need be), then the session's Neovim shows `file` formatted.
fn open(file: &str) -> anyhow::Result<()> {
    let path = absolute(Path::new(file))?;
    anyhow::ensure!(path.is_file(), "{file}: no such file");
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let lit = vim::string(&path.display().to_string())
        .ok_or_else(|| anyhow::anyhow!("{file}: a control character in its name"))?;
    let socket = paths::nvim_socket(get);
    // popped out, the file shows in the pop-out pane: the slot only says so
    if let Some(toolbar) = paths::toolbar_socket(get).filter(|_| !popped_out(&socket)) {
        // best effort: without a toolbar, an Editor already open still shows it
        let tool = get("HERDR_DOC_TOOL")
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "editor".into());
        if let Ok(mut conn) = std::os::unix::net::UnixStream::connect(&toolbar) {
            let _ = writeln!(conn, "select {tool}");
        }
    }
    let socket = socket.display().to_string();
    let started = Instant::now();
    loop {
        let shown = Command::new("nvim")
            .args([
                "--server",
                &socket,
                "--remote-expr",
                &format!("v:lua.HerdrDoc.view({lit})"),
            ])
            .output();
        if shown
            .as_ref()
            .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "1")
        {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(10) {
            anyhow::bail!(
                "the document tool is not running in this workspace (no Neovim at {socket})"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => pane(&[]),
        Some("popout") if args.len() == 1 => popout_pane(),
        Some("open") if args.len() == 2 => open(&args[1]),
        Some("view") if args.len() == 2 => view(&args[1]),
        Some("render") if args.len() == 2 => render::render(
            Path::new(&args[1]),
            render::columns(get),
            &mut std::io::stdout(),
        ),
        Some("edit") => edit(&args[1..]),
        Some(a) if !a.starts_with('-') && !["open", "view", "render", "popout"].contains(&a) => {
            pane(&args)
        }
        _ => Err(anyhow::anyhow!(
            "usage: herdr-doc [files…] | popout | open <file> | view <file> | render <file> | edit [+line] <file>"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "herdr-doc: {e:#}");
            ExitCode::FAILURE
        }
    }
}
