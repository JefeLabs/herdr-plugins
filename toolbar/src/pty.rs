//! One hosted tool: its pseudo-terminal, screen model and child process.
//!
//! A `ToolProcess` owns a pty pair, a `vt100::Parser` fed by a reader thread,
//! and the child. Output wakes the UI through a channel; the UI reads the
//! screen under a read lock and renders it with tui-term.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tui_term::vt100;

use crate::config::ToolConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

impl Size {
    /// The pty size; in pixels too when the cell size is known, which is how
    /// a graphics tool learns how big to draw.
    fn pty(self, cell: Option<(u16, u16)>) -> PtySize {
        let (rows, cols) = (self.rows.max(1), self.cols.max(1));
        let (w, h) = cell.unwrap_or((0, 0));
        PtySize {
            rows,
            cols,
            pixel_width: cols.saturating_mul(w),
            pixel_height: rows.saturating_mul(h),
        }
    }
}

/// Image commands a graphics tool printed, each with its cursor (row, col).
type GraphicsQueue = Arc<Mutex<Vec<(Vec<u8>, u16, u16)>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    NotStarted,
    Running,
    Exited { code: Option<i32> },
    Failed(String),
}

/// What every hosted tool inherits from the toolbar's own environment.
#[derive(Debug, Clone)]
pub struct SpawnEnv {
    /// Default working directory (the workspace's), used when the tool has no `cwd`.
    pub cwd: PathBuf,
    /// Herdr variables to pass through, e.g. `HERDR_BIN_PATH`.
    pub inherit: Vec<(String, String)>,
    pub tool_id: String,
    /// The toolbar's cell size in pixels (width, height), when its terminal
    /// reports one: graphics tools are told their size in pixels from it.
    pub cell: Option<(u16, u16)>,
}

/// Variables removed from the inherited environment: they tell a tool it sits
/// in a graphics-capable terminal, which the embedded one is not (text-only v1).
const DENIED_ENV: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "KITTY_PUBLIC_KEY",
    "ITERM_SESSION_ID",
    "GHOSTTY_RESOURCES_DIR",
];
const DENIED_ENV_PREFIXES: &[&str] = &["WEZTERM_", "KITTY_"];

fn env_is_denied(name: &str) -> bool {
    DENIED_ENV.contains(&name) || DENIED_ENV_PREFIXES.iter().any(|p| name.starts_with(p))
}

pub struct ToolProcess {
    parser: Arc<RwLock<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    graphics: GraphicsQueue,
    cell: Option<(u16, u16)>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    exit: Arc<Mutex<Option<Option<i32>>>>,
    pid: Option<u32>,
}

impl ToolProcess {
    /// Start the tool. Errors are user-facing strings: "cannot start `<name>`: <os error>".
    pub fn spawn(
        cfg: &ToolConfig,
        env: &SpawnEnv,
        size: Size,
        wake: Sender<()>,
    ) -> Result<Self, String> {
        // portable-pty appends the whole PATH to a spawn failure; keep the first line.
        let fail = |e: &dyn std::fmt::Display| {
            let text = e.to_string();
            let first = text
                .lines()
                .next()
                .unwrap_or("")
                .trim_end_matches(':')
                .trim();
            format!(
                "cannot start `{}`: {} ({})",
                cfg.name,
                first,
                cfg.command.join(" ")
            )
        };

        // openpty is not safe to call from several threads at once on macOS
        // (it fails with "Unknown error: -6"); serialise pty creation
        static OPENPTY: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let pair = {
            let _guard = OPENPTY.lock().unwrap_or_else(|e| e.into_inner());
            native_pty_system()
                .openpty(size.pty(env.cell))
                .map_err(|e| fail(&e))?
        };

        let mut cmd = CommandBuilder::new(&cfg.command[0]);
        cmd.args(&cfg.command[1..]);
        cmd.cwd(
            cfg.cwd
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| env.cwd.clone()),
        );
        // Inherit the toolbar's environment so tools behave as they do standalone
        // (SSH agent, EDITOR, tokens, proxies), minus graphics identifiers.
        cmd.env_clear();
        for (k, v) in std::env::vars() {
            if !env_is_denied(&k) {
                cmd.env(k, v);
            }
        }
        for (k, v) in &env.inherit {
            cmd.env(k, v);
        }
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        // Text-only v1: look like a plain xterm so tools fall back from graphics.
        cmd.env("TERM", "xterm-256color");
        cmd.env("HERDR_TOOLBAR_TOOL_ID", &env.tool_id);

        let child = pair.slave.spawn_command(cmd).map_err(|e| fail(&e))?;
        drop(pair.slave);
        let pid = child.process_id();

        let parser = Arc::new(RwLock::new(vt100::Parser::new(size.rows, size.cols, 0)));
        let mut reader = pair.master.try_clone_reader().map_err(|e| fail(&e))?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| fail(&e))?));
        let graphics: GraphicsQueue = Arc::new(Mutex::new(Vec::new()));

        {
            let parser = Arc::clone(&parser);
            let writer = Arc::clone(&writer);
            let graphics = Arc::clone(&graphics);
            let (with_graphics, cell) = (cfg.graphics, env.cell);
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                let mut splitter = crate::graphics::Splitter::default();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) if !with_graphics => {
                            parser.write().unwrap().process(&buf[..n]);
                        }
                        Ok(n) => {
                            for piece in splitter.split(&buf[..n]) {
                                take_piece(piece, &parser, &writer, &graphics, cell);
                            }
                        }
                    }
                    if wake.send(()).is_err() {
                        break;
                    }
                }
            });
        }

        let child: Arc<Mutex<Box<dyn Child + Send + Sync>>> = Arc::new(Mutex::new(child));
        let exit: Arc<Mutex<Option<Option<i32>>>> = Arc::new(Mutex::new(None));
        {
            let child = Arc::clone(&child);
            let exit = Arc::clone(&exit);
            std::thread::spawn(move || {
                // Poll rather than block so `kill` can take the lock.
                loop {
                    let status = child.lock().unwrap().try_wait();
                    match status {
                        Ok(Some(s)) => {
                            *exit.lock().unwrap() = Some(Some(s.exit_code() as i32));
                            break;
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                        Err(_) => {
                            *exit.lock().unwrap() = Some(None);
                            break;
                        }
                    }
                }
            });
        }

        Ok(Self {
            parser,
            master: pair.master,
            writer,
            graphics,
            cell: env.cell,
            child,
            exit,
            pid,
        })
    }

    /// The image commands printed since the last call, each with the tool's
    /// cursor (row, column) where it was printed.
    pub fn take_graphics(&self) -> Vec<(Vec<u8>, u16, u16)> {
        std::mem::take(&mut *self.graphics.lock().unwrap())
    }

    pub fn screen(&self) -> impl std::ops::Deref<Target = vt100::Screen> + '_ {
        ScreenGuard(self.parser.read().unwrap())
    }

    pub fn write(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, size: Size) {
        self.parser
            .write()
            .unwrap()
            .screen_mut()
            .set_size(size.rows, size.cols);
        let _ = self.master.resize(size.pty(self.cell));
    }

    pub fn state(&self) -> ToolState {
        match *self.exit.lock().unwrap() {
            Some(code) => ToolState::Exited { code },
            None => ToolState::Running,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// SIGHUP, wait up to 500 ms, then SIGKILL whatever is left.
    pub fn kill(&mut self) {
        if self.state() != ToolState::Running {
            return;
        }
        let Some(pid) = self.pid else { return };
        // The child leads its own session (portable-pty calls setsid), so its
        // process group id is its pid: signal the whole group, so a `sh -c`
        // wrapper's children go with it.
        let pid = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGHUP);
        let deadline = Instant::now() + Duration::from_millis(500);
        while self.state() == ToolState::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.state() == ToolState::Running {
            let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = self.child.lock().unwrap().wait();
            *self.exit.lock().unwrap() = Some(None);
        }
    }
}

/// One piece of a graphics tool's output: text to the screen model (and
/// answers to the reports it asks for), queries answered, image commands
/// kept with the cursor where they were printed.
fn take_piece(
    piece: crate::graphics::Piece,
    parser: &RwLock<vt100::Parser>,
    writer: &Mutex<Box<dyn Write + Send>>,
    graphics: &GraphicsQueue,
    cell: Option<(u16, u16)>,
) {
    use crate::graphics::{Piece, is_query, query_reply, report_replies};
    let answer = |bytes: &[u8]| {
        if bytes.is_empty() {
            return;
        }
        let mut w = writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    };
    match piece {
        Piece::Text(text) => {
            let (rows, cols) = {
                let mut p = parser.write().unwrap();
                p.process(&text);
                p.screen().size()
            };
            answer(&report_replies(&text, cell, rows, cols));
        }
        Piece::Graphics(cmd) if is_query(&cmd) => {
            answer(&query_reply(&cmd).unwrap_or_default());
        }
        Piece::Graphics(cmd) => {
            let (row, col) = parser.read().unwrap().screen().cursor_position();
            graphics.lock().unwrap().push((cmd, row, col));
        }
    }
}

impl std::fmt::Debug for ToolProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolProcess")
            .field("pid", &self.pid)
            .field("state", &self.state())
            .finish()
    }
}

impl Drop for ToolProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

struct ScreenGuard<'a>(RwLockReadGuard<'a, vt100::Parser>);

impl std::ops::Deref for ScreenGuard<'_> {
    type Target = vt100::Screen;
    fn deref(&self) -> &vt100::Screen {
        self.0.screen()
    }
}
