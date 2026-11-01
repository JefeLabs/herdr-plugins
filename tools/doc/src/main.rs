//! herdr-doc, the toolbar's document tool:
//! - `herdr-doc` (the pane): Neovim on the workspace's Editor socket, with
//!   the document view;
//! - `herdr-doc open <file>`: what an agent runs to show you a file;
//! - `herdr-doc view <file>`: `less` over the formatted view;
//! - `herdr-doc render <file>`: the formatted view (for `less`);
//! - `herdr-doc edit [+line] <file>`: `v` in the view.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use herdr_doc::{kind, nvim, paths, render, vim};

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

/// The pane: Neovim listening on the workspace's socket (a stale one
/// removed first, as the Editor does), with the document view. `files`
/// open as usual.
fn pane(files: &[String]) -> anyhow::Result<()> {
    let socket = paths::nvim_socket(get);
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&socket);
    let err = Command::new("nvim")
        .args(nvim::args(&socket))
        .args(files)
        .env("HERDR_DOC_BIN", me())
        .exec();
    Err(anyhow::anyhow!("cannot start nvim: {err}"))
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
    if let Some(toolbar) = paths::toolbar_socket(get) {
        // best effort: without a toolbar, an Editor already open still shows it
        let tool = get("HERDR_DOC_TOOL")
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "editor".into());
        if let Ok(mut conn) = std::os::unix::net::UnixStream::connect(&toolbar) {
            let _ = writeln!(conn, "select {tool}");
        }
    }
    let socket = paths::nvim_socket(get).display().to_string();
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
        Some("open") if args.len() == 2 => open(&args[1]),
        Some("view") if args.len() == 2 => view(&args[1]),
        Some("render") if args.len() == 2 => render::render(
            Path::new(&args[1]),
            render::columns(get),
            &mut std::io::stdout(),
        ),
        Some("edit") => edit(&args[1..]),
        Some(a) if !a.starts_with('-') && !["open", "view", "render"].contains(&a) => pane(&args),
        _ => Err(anyhow::anyhow!(
            "usage: herdr-doc [files…] | open <file> | view <file> | render <file> | edit [+line] <file>"
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
