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
    fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows.max(1),
            cols: self.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

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
    writer: Mutex<Box<dyn Write + Send>>,
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

        let pair = native_pty_system()
            .openpty(size.pty())
            .map_err(|e| fail(&e))?;

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
        let writer = pair.master.take_writer().map_err(|e| fail(&e))?;

        {
            let parser = Arc::clone(&parser);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            parser.write().unwrap().process(&buf[..n]);
                            if wake.send(()).is_err() {
                                break;
                            }
                        }
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
            writer: Mutex::new(writer),
            child,
            exit,
            pid,
        })
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
        let _ = self.master.resize(size.pty());
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
        let pid = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGHUP);
        let deadline = Instant::now() + Duration::from_millis(500);
        while self.state() == ToolState::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.state() == ToolState::Running {
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = self.child.lock().unwrap().wait();
            *self.exit.lock().unwrap() = Some(None);
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
