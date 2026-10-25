//! The toolbar state machine and event loop.
//!
//! `App` owns one `Slot` per configured tool. Processes start lazily on first
//! selection and keep running in the background; switching only changes
//! which screen is painted. `run` drives it with crossterm events, pty wake
//! messages and control-socket commands.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;

use crate::config::{Loaded, ToolConfig, Trust, load_merged};
use crate::keys::{Action, Prefix};
use crate::pty::{Size, SpawnEnv, ToolProcess, ToolState};
use crate::ui;

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub global: PathBuf,
    pub workspace_root: PathBuf,
    pub trust: Trust,
}

pub struct Slot {
    pub cfg: ToolConfig,
    pub proc: Option<ToolProcess>,
    /// Why the last spawn failed, shown instead of a screen.
    pub last_error: Option<String>,
}

impl Slot {
    fn state(&self) -> ToolState {
        match (&self.proc, &self.last_error) {
            (Some(p), _) => p.state(),
            (None, Some(e)) => ToolState::Failed(e.clone()),
            (None, None) => ToolState::NotStarted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub states: Vec<ToolState>,
    pub selected: usize,
    pub has_screen: bool,
    pub message: Option<String>,
    pub footer: Option<String>,
    pub prefix_armed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

/// Commands that reach a running toolbar from outside (the `select` mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCmd {
    Select(String),
}

pub struct App {
    pub slots: Vec<Slot>,
    pub selected: usize,
    pub prefix: Prefix,
    /// Replaces the screen when set (config errors, untrusted file, spawn failures).
    pub message: Option<String>,
    /// Whole pane size; the content area is derived through `ui::split`.
    pub size: Size,
    env: SpawnEnv,
    paths: ConfigPaths,
    wake_tx: Sender<()>,
    wake_rx: Receiver<()>,
}

impl App {
    /// A broken config is not fatal: the bar starts empty with the error as the
    /// message, so `r` (reload) can recover once the file is fixed. `Err` is
    /// reserved for things no reload can fix (none today).
    pub fn new(paths: ConfigPaths, env: SpawnEnv, size: Size) -> anyhow::Result<Self> {
        let (wake_tx, wake_rx) = channel();
        let mut app = Self {
            slots: Vec::new(),
            selected: 0,
            prefix: Prefix::new(crate::config::DEFAULT_PREFIX)?,
            message: None,
            size,
            env,
            paths,
            wake_tx,
            wake_rx,
        };
        app.reload();
        Ok(app)
    }

    /// Replace the slot list from a fresh config, keeping running processes
    /// whose id still exists. Untrusted workspace files become a message.
    fn apply_loaded(&mut self, loaded: Loaded) {
        let Loaded {
            config,
            ignored_untrusted,
        } = loaded;
        let mut old: Vec<Slot> = std::mem::take(&mut self.slots);
        let selected_id = old.get(self.selected).map(|s| s.cfg.id.clone());
        for cfg in config.tools {
            let slot = match old.iter().position(|s| s.cfg.id == cfg.id) {
                Some(i) => {
                    let mut s = old.remove(i);
                    if s.cfg != cfg {
                        // Definition changed: the next start uses the new one.
                        s.cfg = cfg;
                    }
                    s
                }
                None => Slot {
                    cfg,
                    proc: None,
                    last_error: None,
                },
            };
            self.slots.push(slot);
        }
        for mut gone in old {
            if let Some(p) = gone.proc.as_mut() {
                p.kill();
            }
        }
        self.selected = selected_id
            .and_then(|id| self.slots.iter().position(|s| s.cfg.id == id))
            .unwrap_or(0);
        self.message = ignored_untrusted.map(|p| {
            format!(
                "untrusted {} ignored; run `herdr-toolbar trust` in this workspace to allow it",
                p.display()
            )
        });
        let _ = config.prefix; // prefix changes apply on next start; see reload()
    }

    fn content_size(&self) -> Size {
        let footer = self.footer().is_some();
        let (rows, cols) =
            ui::content_size(Rect::new(0, 0, self.size.cols, self.size.rows), footer);
        Size { rows, cols }
    }

    fn spawn(&mut self, idx: usize) {
        let size = self.content_size();
        let slot = &mut self.slots[idx];
        let env = SpawnEnv {
            tool_id: slot.cfg.id.clone(),
            ..self.env.clone()
        };
        match ToolProcess::spawn(&slot.cfg, &env, size, self.wake_tx.clone()) {
            Ok(p) => {
                slot.proc = Some(p);
                slot.last_error = None;
            }
            Err(e) => {
                slot.proc = None;
                slot.last_error = Some(e);
            }
        }
    }

    /// Show tool `idx`, starting it on first selection. Out-of-range is clamped.
    pub fn select(&mut self, idx: usize) {
        if self.slots.is_empty() {
            return;
        }
        let idx = idx.min(self.slots.len() - 1);
        self.selected = idx;
        if self.slots[idx].proc.is_none() && self.slots[idx].last_error.is_none() {
            self.spawn(idx);
        }
        // The footer may have appeared or vanished; keep the shown tool sized right.
        let size = self.content_size();
        if let Some(p) = self.slots[idx].proc.as_ref()
            && p.screen().size() != (size.rows, size.cols)
        {
            p.resize(size);
        }
    }

    pub fn select_by(&mut self, tool: &str) -> bool {
        let idx = match tool.parse::<usize>() {
            Ok(n) if n >= 1 => Some(n - 1),
            _ => self.slots.iter().position(|s| s.cfg.id == tool),
        };
        match idx {
            Some(i) if i < self.slots.len() => {
                self.select(i);
                true
            }
            _ => false,
        }
    }

    pub fn restart_selected(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        let idx = self.selected;
        if let Some(p) = self.slots[idx].proc.as_mut() {
            p.kill();
        }
        self.slots[idx].proc = None;
        self.slots[idx].last_error = None;
        self.spawn(idx);
    }

    pub fn reload(&mut self) {
        match load_merged(
            &self.paths.global,
            &self.paths.workspace_root,
            &self.paths.trust,
        ) {
            Ok(loaded) => {
                match Prefix::new(&loaded.config.prefix) {
                    Ok(p) => self.prefix = p,
                    Err(e) => {
                        self.message = Some(e.to_string());
                        return;
                    }
                }
                self.apply_loaded(loaded);
                if !self.slots.is_empty() {
                    self.select(self.selected);
                }
            }
            Err(e) => self.message = Some(format!("config not reloaded: {e:#}")),
        }
    }

    pub fn apply_key(&mut self, key: KeyEvent) -> Flow {
        match self.prefix.handle(key) {
            Action::Forward(key) => {
                self.message = None;
                let Some(slot) = self.slots.get(self.selected) else {
                    return Flow::Continue;
                };
                match slot.state() {
                    ToolState::Running => {
                        if let Some(p) = slot.proc.as_ref() {
                            let app_cursor = p.screen().application_cursor();
                            if let Some(bytes) = crate::keys::key_to_bytes(&key, app_cursor) {
                                let _ = p.write(&bytes);
                            }
                        }
                    }
                    ToolState::Exited { .. } | ToolState::Failed(_)
                        if key.code == KeyCode::Enter =>
                    {
                        self.restart_selected();
                    }
                    _ => {}
                }
            }
            Action::Select(i) => self.select(i),
            Action::Next => self.select((self.selected + 1) % self.slots.len().max(1)),
            Action::Prev => {
                let n = self.slots.len().max(1);
                self.select((self.selected + n - 1) % n)
            }
            Action::Reload => self.reload(),
            Action::Restart => self.restart_selected(),
            Action::Quit => return Flow::Quit,
            Action::Ignore => {}
        }
        Flow::Continue
    }

    /// Forward pasted text to the selected running tool, sanitised (see `keys::paste_bytes`).
    pub fn paste(&mut self, text: &str) {
        let Some(p) = self.slots.get(self.selected).and_then(|s| s.proc.as_ref()) else {
            return;
        };
        if p.state() != ToolState::Running {
            return;
        }
        let bracketed = p.screen().bracketed_paste();
        let _ = p.write(&crate::keys::paste_bytes(text, bracketed));
    }

    pub fn apply_control(&mut self, cmd: ControlCmd) {
        match cmd {
            ControlCmd::Select(tool) => {
                if !self.select_by(&tool) {
                    self.message = Some(format!("no tool `{tool}`"));
                }
            }
        }
    }

    /// New whole-pane size; every running tool is resized so hidden ones are right when shown.
    pub fn resize(&mut self, size: Size) {
        self.size = size;
        let content = self.content_size();
        for slot in &self.slots {
            if let Some(p) = slot.proc.as_ref() {
                p.resize(content);
            }
        }
    }

    fn footer(&self) -> Option<String> {
        let slot = self.slots.get(self.selected)?;
        match slot.state() {
            ToolState::Exited { code: Some(c) } => {
                Some(format!("exited (code {c}), enter to restart"))
            }
            ToolState::Exited { code: None } => Some("exited, enter to restart".to_string()),
            _ => None,
        }
    }

    fn message_text(&self) -> Option<&str> {
        if let Some(m) = &self.message {
            return Some(m.as_str());
        }
        self.slots
            .get(self.selected)
            .and_then(|s| s.last_error.as_deref())
    }

    /// A plain-data summary for tests and status: what `draw` would show.
    pub fn view(&self) -> View {
        let selected = self.slots.get(self.selected);
        View {
            states: self.slots.iter().map(Slot::state).collect(),
            selected: self.selected,
            has_screen: selected.and_then(|s| s.proc.as_ref()).is_some(),
            message: self.message_text().map(str::to_string),
            footer: self.footer(),
            prefix_armed: self.prefix.armed(),
        }
    }

    /// Paint the whole pane. The selected tool's screen lock is held only for this call.
    pub fn draw(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let states: Vec<ToolState> = self.slots.iter().map(Slot::state).collect();
        let items = self
            .slots
            .iter()
            .zip(states.iter())
            .enumerate()
            .map(|(i, (s, state))| ui::BarItem {
                icon: &s.cfg.icon,
                name: &s.cfg.name,
                state,
                selected: i == self.selected,
            })
            .collect();
        let footer = self.footer();
        let guard = self
            .slots
            .get(self.selected)
            .and_then(|s| s.proc.as_ref())
            .map(|p| p.screen());
        let frame = ui::Frame {
            items,
            screen: guard.as_deref(),
            message: self.message_text(),
            footer: footer.as_deref(),
            prefix_armed: self.prefix.armed(),
        };
        ui::render(&frame, area, buf);
    }

    pub fn shutdown(&mut self) {
        for slot in &mut self.slots {
            if let Some(p) = slot.proc.as_mut() {
                p.kill();
            }
            slot.proc = None;
        }
    }
}

/// Run the TUI until quit, SIGTERM/SIGHUP, or the controlling terminal goes away.
/// `control` is polled every loop for commands from the `select` mode.
pub fn run(
    paths: ConfigPaths,
    env: SpawnEnv,
    mut control: Option<Box<dyn FnMut() -> Vec<ControlCmd>>>,
) -> anyhow::Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [
        nix::sys::signal::Signal::SIGTERM,
        nix::sys::signal::Signal::SIGHUP,
        nix::sys::signal::Signal::SIGINT,
    ] {
        let flag = Arc::clone(&stop);
        // SAFETY: the handler only stores into an AtomicBool.
        unsafe {
            let _ = nix::sys::signal::sigaction(
                sig,
                &nix::sys::signal::SigAction::new(
                    nix::sys::signal::SigHandler::Handler(on_signal),
                    nix::sys::signal::SaFlags::empty(),
                    nix::sys::signal::SigSet::empty(),
                ),
            );
        }
        STOP_FLAGS.lock().unwrap().push(flag);
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let mut terminal = ratatui::init();
    let area = terminal.size()?;
    let mut app = App::new(
        paths,
        env,
        Size {
            rows: area.height,
            cols: area.width,
        },
    )?;

    let result = (|| -> anyhow::Result<()> {
        loop {
            terminal.draw(|f| app.draw(f.area(), f.buffer_mut()))?;
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Some(c) = control.as_mut() {
                for cmd in c() {
                    app.apply_control(cmd);
                }
            }
            if crossterm::event::poll(Duration::from_millis(16))? {
                match crossterm::event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        if app.apply_key(k) == Flow::Quit {
                            return Ok(());
                        }
                    }
                    Event::Resize(cols, rows) => app.resize(Size { rows, cols }),
                    Event::Paste(text) => app.paste(&text),
                    _ => {}
                }
            }
            // Drain pty wakes so one redraw covers a burst of output.
            while app.wake_rx.try_recv().is_ok() {}
        }
    })();

    app.shutdown();
    ratatui::restore();
    result
}

static STOP_FLAGS: std::sync::Mutex<Vec<Arc<AtomicBool>>> = std::sync::Mutex::new(Vec::new());

extern "C" fn on_signal(_: i32) {
    if let Ok(flags) = STOP_FLAGS.try_lock() {
        for f in flags.iter() {
            f.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::config::{Trust, WORKSPACE_FILE};
    use crate::pty::ToolState;

    const SIZE: Size = Size { rows: 10, cols: 50 };

    fn tool(id: &str, cmd: &str) -> String {
        format!(
            "[[tool]]\nid = \"{id}\"\nname = \"{id}\"\nicon = \"x\"\ncommand = [\"sh\", \"-c\", \"{cmd}\"]\n"
        )
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        paths: ConfigPaths,
        env: SpawnEnv,
    }

    fn fixture(global: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let global_path = tmp.path().join("tools.toml");
        fs::write(&global_path, global).unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir(&ws).unwrap();
        let paths = ConfigPaths {
            global: global_path,
            workspace_root: ws.clone(),
            trust: Trust::new(tmp.path().join("trusted.json")),
        };
        let env = SpawnEnv {
            cwd: ws,
            inherit: vec![],
            tool_id: String::new(),
        };
        Fixture {
            _tmp: tmp,
            paths,
            env,
        }
    }

    fn three() -> String {
        tool("a", "echo A; sleep 30")
            + &tool("b", "echo B; sleep 30")
            + &tool("c", "echo C; sleep 30")
    }

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn pid(app: &App, i: usize) -> u32 {
        app.slots[i].proc.as_ref().unwrap().pid().unwrap()
    }

    fn alive(pid: u32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }

    #[test]
    fn first_tool_starts_on_new_others_lazy() {
        let f = fixture(&three());
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        assert!(app.slots[0].proc.is_some());
        assert!(app.slots[1].proc.is_none());
        assert!(app.slots[2].proc.is_none());
        app.shutdown();
    }

    #[test]
    fn select_spawns_once_and_keeps_background_running() {
        let f = fixture(&three());
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        app.select(1);
        let p1 = pid(&app, 1);
        app.select(0);
        app.select(1);
        assert_eq!(pid(&app, 1), p1, "re-selecting must not respawn");
        assert_eq!(
            app.slots[1].proc.as_ref().unwrap().state(),
            ToolState::Running
        );
        assert_eq!(app.selected, 1);
        app.shutdown();
    }

    #[test]
    fn reload_with_broken_file_keeps_tools_and_sets_message() {
        let f = fixture(&three());
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        fs::write(&f.paths.global, "[[tool]\nid = 1").unwrap();
        app.reload();
        assert_eq!(app.slots.len(), 3);
        assert!(
            app.message.as_deref().unwrap_or("").contains("line 1"),
            "got {:?}",
            app.message
        );
        app.shutdown();
    }

    #[test]
    fn reload_with_new_tool_appends_without_restarting_running_ones() {
        let f = fixture(&three());
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        let p0 = pid(&app, 0);
        fs::write(&f.paths.global, three() + &tool("d", "sleep 30")).unwrap();
        app.reload();
        assert_eq!(app.slots.len(), 4);
        assert_eq!(pid(&app, 0), p0);
        assert!(app.message.is_none());
        app.shutdown();
    }

    #[test]
    fn reload_reports_untrusted_workspace_file() {
        let f = fixture(&three());
        fs::write(
            f.paths.workspace_root.join(WORKSPACE_FILE),
            tool("evil", "true"),
        )
        .unwrap();
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        assert_eq!(app.slots.len(), 3);
        assert!(
            app.message.as_deref().unwrap_or("").contains("untrusted"),
            "got {:?}",
            app.message
        );
        app.shutdown();
    }

    #[test]
    fn exited_hidden_tool_dims_and_enter_restarts_it() {
        let f = fixture(&(tool("a", "sleep 30") + &tool("b", "exit 3")));
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        app.select(1);
        wait_until("exit", || {
            app.slots[1].proc.as_ref().unwrap().state() != ToolState::Running
        });
        app.select(0);
        assert_eq!(app.view().states[1], ToolState::Exited { code: Some(3) });
        app.select(1);
        assert!(app.view().footer.unwrap().starts_with("exited (code 3)"));
        let old = pid(&app, 1);
        assert_eq!(app.apply_key(key(KeyCode::Enter)), Flow::Continue);
        assert_ne!(pid(&app, 1), old, "enter restarts");
        app.shutdown();
    }

    #[test]
    fn resize_fans_out_to_hidden_slots() {
        let f = fixture(&three());
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        app.select(1);
        app.select(0);
        app.resize(Size { rows: 9, cols: 50 });
        // content = 50 - 3 bar columns = 47 wide, footer not showing
        assert_eq!(app.slots[0].proc.as_ref().unwrap().screen().size(), (9, 47));
        assert_eq!(app.slots[1].proc.as_ref().unwrap().screen().size(), (9, 47));
        app.shutdown();
    }

    #[test]
    fn quit_kills_everything() {
        let f = fixture(&three());
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        app.select(1);
        let (p0, p1) = (pid(&app, 0), pid(&app, 1));
        assert_eq!(
            // crossterm reports Ctrl-] as Char('5') + CONTROL
            app.apply_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL)),
            Flow::Continue
        );
        assert_eq!(app.apply_key(key(KeyCode::Char('q'))), Flow::Quit);
        app.shutdown();
        wait_until("children gone", || !alive(p0) && !alive(p1));
    }

    #[test]
    fn missing_binary_shows_message_and_other_tools_work() {
        let f = fixture(
            &(tool("a", "sleep 30")
                + "[[tool]]\nid = \"nope\"\nname = \"Nope\"\nicon = \"!\"\ncommand = [\"definitely-not-installed-xyz\"]\n"),
        );
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        app.select(1);
        let view = app.view();
        assert!(!view.has_screen);
        assert!(view.message.unwrap().contains("cannot start `Nope`"));
        app.select(0);
        assert!(app.view().has_screen);
        app.shutdown();
    }

    #[test]
    fn broken_config_at_startup_gives_an_empty_bar_and_a_message() {
        let f = fixture("[[tool]\nid = 1");
        let mut app = App::new(f.paths.clone(), f.env.clone(), SIZE)
            .expect("startup must not fail on config");
        assert!(app.slots.is_empty());
        assert!(app.view().message.unwrap_or_default().contains("line 1"));
        // and reload recovers
        fs::write(&f.paths.global, three()).unwrap();
        app.reload();
        assert_eq!(app.slots.len(), 3);
        assert!(app.view().message.is_none());
        app.shutdown();
    }

    #[test]
    fn unknown_order_id_at_startup_is_a_message_not_a_crash() {
        let f = fixture(&(String::from("order = [\"nope\"]\n") + &three()));
        let app = App::new(f.paths.clone(), f.env.clone(), SIZE).unwrap();
        assert!(app.view().message.unwrap_or_default().contains("`nope`"));
    }
}
