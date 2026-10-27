//! The toolbar state machine and event loop.
//!
//! `App` owns one `Slot` per configured tool. Processes start lazily on first
//! selection and keep running in the background; switching only changes
//! which screen is painted. `run` drives it with crossterm events, pty wake
//! messages and control-socket commands.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;

use crate::config::{Loaded, Open, ToolConfig, Trust, load_merged};
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
    /// Set when running inside Herdr: pane tools need it.
    herdr: Option<crate::herdr::HerdrContext>,
    /// What background launches have to say (a failure), for the footer.
    notes_tx: Sender<String>,
    notes_rx: Receiver<String>,
    /// Bytes for the terminal after the next frame (clearing a hidden
    /// graphics tool's images).
    pending_out: Vec<u8>,
    /// Button size, from the config.
    bar: crate::config::BarSize,
    /// The button the mouse rests on.
    pub hover: Option<ui::Target>,
    /// Shrunk to the bar: tools keep running and keep their size, unseen.
    pub compact: bool,
    /// Before compacting: the pane's width in Herdr's columns, and ours.
    expanded: Option<(u16, u16)>,
    /// The too-narrow width we last asked Herdr to widen, so a width Herdr
    /// will not widen is asked about once.
    widened_at: Option<u16>,
}

impl App {
    /// A broken config is not fatal: the bar starts empty with the error as the
    /// message, so `r` (reload) can recover once the file is fixed. `Err` is
    /// reserved for things no reload can fix (none today).
    pub fn new(paths: ConfigPaths, env: SpawnEnv, size: Size) -> anyhow::Result<Self> {
        let (wake_tx, wake_rx) = channel();
        let (notes_tx, notes_rx) = channel();
        let mut app = Self {
            slots: Vec::new(),
            selected: 0,
            herdr: None,
            notes_tx,
            notes_rx,
            pending_out: Vec::new(),
            bar: Default::default(),
            hover: None,
            compact: false,
            expanded: None,
            widened_at: None,
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
        self.bar = config.bar;
        self.hover = None;
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

    /// Where the bar's rule goes: before the first tool that opens a pane.
    fn rule(&self) -> Option<usize> {
        ui::rule_index(self.slots.iter().map(|s| s.cfg.open == Open::Pane))
    }

    /// The compact toggle shows inside Herdr, where the pane can be resized.
    fn toggle_shown(&self) -> bool {
        self.herdr.as_ref().is_some_and(|h| h.pane_id.is_some())
    }

    /// The bar's shape now: large buttons only while they all fit.
    fn layout(&self) -> ui::BarLayout {
        ui::bar_layout(
            self.bar,
            self.slots.len() + usize::from(self.toggle_shown()),
            self.rule().is_some(),
            self.size.rows,
        )
    }

    /// Never narrower than the bar: when Herdr (or a dragged divider) leaves
    /// the pane too narrow for the icons, widen it by the difference. Each
    /// too-narrow width is tried once, since Herdr may not be able to.
    fn keep_icons_visible(&mut self) {
        let need = self.layout().width;
        if self.size.cols >= need {
            self.widened_at = None;
            return;
        }
        if !self.toggle_shown() || self.widened_at == Some(self.size.cols) {
            return;
        }
        let Some(ctx) = self.herdr.clone() else {
            return;
        };
        let Some(pane) = ctx.pane_id.clone() else {
            return;
        };
        self.widened_at = Some(self.size.cols);
        crate::herdr::widen(&ctx, &pane, need - self.size.cols);
    }

    /// Shrink the toolbar's pane to its bar (as far as Herdr allows), or give
    /// it back the width it had. Tools keep running and keep their size.
    pub fn set_compact(&mut self, on: bool) {
        if on == self.compact {
            return;
        }
        let Some(ctx) = self.herdr.clone() else {
            return;
        };
        let Some(pane) = ctx.pane_id.clone() else {
            return;
        };
        if on {
            let had = crate::herdr::set_width(&ctx, &pane, self.layout().width);
            self.expanded = had.map(|w| (w, self.size.cols));
            // a graphics tool's images would stay on screen
            if self
                .slots
                .get(self.selected)
                .is_some_and(|s| s.cfg.graphics)
            {
                self.pending_out
                    .extend_from_slice(crate::graphics::DELETE_ALL);
            }
            self.compact = true;
        } else {
            self.compact = false;
            if let Some((width, cols)) = self.expanded.take() {
                crate::herdr::set_width(&ctx, &pane, width);
                // until Herdr's resize reaches us
                self.size.cols = cols;
            }
            self.resize(self.size);
        }
    }

    fn content_size(&self) -> Size {
        let footer = self.footer().is_some();
        let (rows, cols) = ui::content_size(
            Rect::new(0, 0, self.size.cols, self.size.rows),
            footer,
            self.layout().width,
        );
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

    /// The Herdr context, for tools that open in a Herdr pane.
    pub fn set_herdr(&mut self, ctx: crate::herdr::HerdrContext) {
        self.herdr = Some(ctx);
    }

    /// Show tool `idx`, starting it on first selection. Out-of-range is clamped.
    pub fn select(&mut self, idx: usize) {
        if self.slots.is_empty() {
            return;
        }
        let idx = idx.min(self.slots.len() - 1);
        // a pane tool opens beside the work pane and leaves the slot alone
        if self.slots[idx].cfg.open == Open::Pane {
            self.toggle_pane(idx);
            return;
        }
        // a tool that runs here needs the room: expand first
        self.set_compact(false);
        let previous = self.selected;
        if previous != idx {
            // a hidden graphics tool's images would stay on screen
            if self.slots.get(previous).is_some_and(|s| s.cfg.graphics) {
                self.pending_out
                    .extend_from_slice(crate::graphics::DELETE_ALL);
            }
            // a graphics tool shown again redraws: its images were cleared
            if self.slots[idx].cfg.graphics
                && let Some(p) = self.slots[idx].proc.as_ref()
            {
                let size = self.content_size();
                p.resize(Size {
                    rows: size.rows,
                    cols: size.cols.saturating_sub(1).max(1),
                });
                p.resize(size);
            }
        }
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

    /// Open a pane tool, or close the pane it opened, off the UI thread;
    /// Herdr answers in tens of milliseconds but the toolbar must never wait.
    fn toggle_pane(&mut self, idx: usize) {
        let cfg = self.slots[idx].cfg.clone();
        let Some(ctx) = self.herdr.clone() else {
            self.message = Some(format!(
                "{} opens in a Herdr pane: run the toolbar inside Herdr",
                cfg.name
            ));
            return;
        };
        self.message = None;
        let (tx, wake) = (self.notes_tx.clone(), self.wake_tx.clone());
        std::thread::spawn(move || {
            if let Err(e) = crate::herdr::toggle_pane(&ctx, &cfg) {
                let _ = tx.send(format!("{}: {e:#}", cfg.name));
                let _ = wake.send(());
            }
        });
    }

    /// Bytes to write after a frame: what is pending, then the image
    /// commands of the shown graphics tool placed in the slot. Hidden graphics
    /// tools' commands are dropped; the tool redraws when shown again.
    pub fn graphics_out(&mut self) -> Vec<u8> {
        let mut out = std::mem::take(&mut self.pending_out);
        for (i, slot) in self.slots.iter().enumerate() {
            if !slot.cfg.graphics {
                continue;
            }
            let Some(p) = slot.proc.as_ref() else {
                continue;
            };
            let cmds = p.take_graphics();
            if i == self.selected && !self.compact {
                // the slot's screen starts at the pane's top-left cell
                for (cmd, row, col) in cmds {
                    out.extend(crate::graphics::placed(&cmd, 0, 0, row, col));
                }
            }
        }
        out
    }

    /// Show what background launches reported.
    pub fn drain_notes(&mut self) {
        while let Ok(note) = self.notes_rx.try_recv() {
            self.message = Some(note);
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

    /// A click on a button selects that tool, and resting the mouse on one
    /// names it. Inside the tool's area, mouse events go to the tool when it
    /// has turned mouse reporting on, in the mode and encoding it asked for,
    /// at its own coordinates.
    pub fn apply_mouse(&mut self, ev: crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};
        let area = Rect::new(0, 0, self.size.cols, self.size.rows);
        let layout = self.layout();
        let (content, bar) = ui::split(area, layout.width);
        if ev.column >= bar.x && bar.width > 0 {
            // not the rule, the marker row or the empty rows below
            let at = ui::target_at(
                bar,
                layout.rows,
                self.slots.len(),
                self.rule(),
                self.toggle_shown(),
                ev.row,
            );
            match (ev.kind, at) {
                (MouseEventKind::Down(MouseButton::Left), Some(ui::Target::Toggle)) => {
                    self.set_compact(!self.compact);
                }
                (MouseEventKind::Down(MouseButton::Left), Some(ui::Target::Tool(index))) => {
                    self.message = None;
                    self.select(index);
                }
                (MouseEventKind::Moved, at) => self.hover = at,
                _ => {}
            }
            return;
        }
        self.hover = None;
        let tool_rows = self.content_size().rows;
        if ev.column < content.x || ev.row < content.y || ev.row - content.y >= tool_rows {
            return;
        }
        let Some(p) = self.slots.get(self.selected).and_then(|s| s.proc.as_ref()) else {
            return;
        };
        if p.state() != ToolState::Running {
            return;
        }
        let (mode, encoding) = {
            let screen = p.screen();
            (
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };
        if let Some(bytes) = crate::keys::mouse_to_bytes(
            &ev,
            ev.column - content.x,
            ev.row - content.y,
            mode,
            encoding,
        ) {
            let _ = p.write(&bytes);
        }
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
        self.keep_icons_visible();
        if self.compact {
            // unseen while compact: keep each tool's size until we expand
            return;
        }
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
                pane: s.cfg.open == Open::Pane,
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
            bar: self.bar,
            hover: self.hover,
            toggle: self.toggle_shown().then_some(self.compact),
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

/// Run the TUI until quit, SIGTERM/SIGHUP/SIGINT, or the terminal goes away.
/// `control` is polled every loop for commands from the `select` mode.
///
/// Terminal events are read on their own thread: crossterm's reader spins
/// forever once the pty is gone, so the main loop must never block on it.
/// When the stop flag is set (a signal, a dead reader, a failed draw) the
/// tools are killed, the terminal restored, and the process exits outright so
/// no thread can keep it alive.
pub fn run(
    paths: ConfigPaths,
    env: SpawnEnv,
    herdr: Option<crate::herdr::HerdrContext>,
    mut control: Option<Box<dyn FnMut() -> Vec<ControlCmd>>>,
    on_exit: Option<Box<dyn FnOnce()>>,
) -> anyhow::Result<()> {
    install_signal_handlers();

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture
    );
    let area = terminal.size().unwrap_or(ratatui::layout::Size {
        width: 80,
        height: 24,
    });
    // the cell size in pixels, when the terminal reports its pixel size
    let cell = crossterm::terminal::window_size().ok().and_then(|w| {
        (w.width > 0 && w.height > 0 && w.columns > 0 && w.rows > 0)
            .then(|| (w.width / w.columns, w.height / w.rows))
    });
    let env = SpawnEnv { cell, ..env };
    let mut app = match App::new(
        paths,
        env,
        Size {
            rows: area.height,
            cols: area.width,
        },
    ) {
        Ok(app) => app,
        Err(e) => {
            restore_terminal();
            return Err(e);
        }
    };
    if let Some(ctx) = herdr {
        app.set_herdr(ctx);
    }

    let (events_tx, events_rx) = channel::<Event>();
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if events_tx.send(ev).is_err() {
                break;
            }
        }
        STOP.store(true, Ordering::Relaxed);
    });

    let outcome: anyhow::Result<()> = (|| loop {
        if terminal
            .draw(|f| app.draw(f.area(), f.buffer_mut()))
            .is_err()
        {
            return Ok(()); // the terminal is gone
        }
        // graphics tools' images go after the frame, placed in the slot
        let images = app.graphics_out();
        if !images.is_empty() {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&images);
            let _ = out.flush();
        }
        if STOP.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(c) = control.as_mut() {
            for cmd in c() {
                app.apply_control(cmd);
            }
        }
        match events_rx.recv_timeout(Duration::from_millis(16)) {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                if app.apply_key(k) == Flow::Quit {
                    return Ok(());
                }
            }
            Ok(Event::Resize(cols, rows)) => app.resize(Size { rows, cols }),
            Ok(Event::Paste(text)) => app.paste(&text),
            Ok(Event::Mouse(m)) => app.apply_mouse(m),
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
        // Drain pty wakes so one redraw covers a burst of output.
        while app.wake_rx.try_recv().is_ok() {}
        app.drain_notes();
    })();

    app.shutdown();
    restore_terminal();
    // Drop the control poller (its socket unlinks on drop), then the caller's
    // cleanup: `run` exits the process below, so nothing after it would run.
    drop(control);
    if let Some(f) = on_exit {
        f();
    }
    // The event-reader thread may be spinning on a dead pty; do not wait for it.
    match outcome {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "herdr-toolbar: {e:#}");
            std::process::exit(1)
        }
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

fn install_signal_handlers() {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
    for sig in [Signal::SIGTERM, Signal::SIGHUP, Signal::SIGINT] {
        // SAFETY: the handler only stores into a static AtomicBool.
        unsafe {
            let _ = sigaction(
                sig,
                &SigAction::new(
                    SigHandler::Handler(on_signal),
                    SaFlags::SA_RESTART,
                    SigSet::empty(),
                ),
            );
        }
    }
}

/// Undo what `ratatui::init` did, ignoring every error: the terminal may be
/// gone (hang-up), and `ratatui::restore` reports failures with `eprintln!`,
/// which itself panics when stderr is dead and would abort the process from
/// inside the panic hook.
fn restore_terminal() {
    use crossterm::{
        cursor::Show,
        event::{DisableBracketedPaste, DisableMouseCapture},
        terminal::LeaveAlternateScreen,
    };
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::config::{Trust, WORKSPACE_FILE};
    use crate::pty::ToolState;
    use tui_term::vt100;

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
            cell: None,
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
            // generous: under a full parallel test run a shell can take seconds to start
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn click(column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn clicking_an_icon_selects_its_tool() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        // the bar is the rightmost three columns, one icon per row
        app.apply_mouse(click(SIZE.cols - 2, 2));
        assert_eq!(app.selected, 2);
        app.apply_mouse(click(SIZE.cols - 1, 0));
        assert_eq!(app.selected, 0);
        // below the icons nothing happens
        app.apply_mouse(click(SIZE.cols - 2, 7));
        assert_eq!(app.selected, 0);
        app.shutdown();
    }

    fn moved(column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn clicking_anywhere_on_a_large_button_selects_its_tool() {
        let fx = fixture(&format!("bar = \"large\"\n{}", three()));
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        // large buttons: the rightmost five columns, two rows each
        app.apply_mouse(click(SIZE.cols - 5, 2));
        assert_eq!(app.selected, 1);
        app.apply_mouse(click(SIZE.cols - 1, 5));
        assert_eq!(app.selected, 2);
        app.apply_mouse(click(SIZE.cols - 3, 0));
        assert_eq!(app.selected, 0);
        // below the buttons nothing happens
        app.apply_mouse(click(SIZE.cols - 3, 7));
        assert_eq!(app.selected, 0);
        app.shutdown();
    }

    /// A Herdr context on the fake `herdr`, the toolbar being pane w1:p9.
    fn herdr_ctx(dir: &Path) -> (crate::herdr::HerdrContext, PathBuf) {
        let log = dir.join("herdr.log");
        let ctx = crate::herdr::HerdrContext {
            bin: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-herdr.sh"),
            workspace_id: Some("w1".into()),
            pane_id: Some("w1:p9".into()),
            tab_id: Some("w1:t1".into()),
            target_pane_id: Some("w1:p3".into()),
            workspace_cwd: dir.to_path_buf(),
            config_dir: dir.join("config"),
            state_dir: dir.join("state"),
            socket: dir.join("herdr.sock"),
            inherit: vec![],
            extra_env: vec![("FAKE_HERDR_LOG".into(), log.display().to_string())],
        };
        (ctx, log)
    }

    fn narrow(app: &mut App, cols: &str) {
        app.herdr
            .as_mut()
            .unwrap()
            .extra_env
            .push(("FAKE_TOOLBAR_WIDTH".into(), cols.into()));
    }

    #[test]
    fn the_toggle_is_the_top_button_only_inside_herdr() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        app.apply_mouse(moved(SIZE.cols - 2, 0));
        assert_eq!(
            app.hover,
            Some(ui::Target::Tool(0)),
            "no toggle outside Herdr"
        );
        app.set_herdr(herdr_ctx(fx._tmp.path()).0);
        app.apply_mouse(moved(SIZE.cols - 2, 0));
        assert_eq!(app.hover, Some(ui::Target::Toggle));
        app.apply_mouse(moved(SIZE.cols - 2, 1));
        assert_eq!(app.hover, Some(ui::Target::Tool(0)));
        app.shutdown();
    }

    #[test]
    fn the_toggle_compacts_the_pane_and_expands_it_back() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        let (ctx, log) = herdr_ctx(fx._tmp.path());
        app.set_herdr(ctx);
        app.apply_mouse(click(SIZE.cols - 2, 0));
        assert!(app.compact);
        let l = fs::read_to_string(&log).unwrap_or_default();
        // from 90 columns down to the small bar's 3, of a 200-column tab
        assert!(
            l.contains("pane resize --pane w1:p9 --direction right --amount 0.4350"),
            "{l}"
        );
        narrow(&mut app, "14");
        app.apply_mouse(click(SIZE.cols - 2, 0));
        assert!(!app.compact);
        let l = fs::read_to_string(&log).unwrap_or_default();
        assert!(
            l.contains("pane resize --pane w1:p9 --direction left --amount 0.3800"),
            "back to 90: {l}"
        );
        app.shutdown();
    }

    #[test]
    fn while_compact_tools_keep_their_size_and_are_not_drawn() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        app.set_herdr(herdr_ctx(fx._tmp.path()).0);
        wait_until("tool a runs", || app.view().states[0] == ToolState::Running);
        let size = |app: &App| app.slots[0].proc.as_ref().unwrap().screen().size();
        let before = size(&app);
        app.apply_mouse(click(SIZE.cols - 2, 0));
        app.resize(Size { rows: 10, cols: 14 });
        assert_eq!(size(&app), before, "not squeezed into the narrow pane");
        let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 14, 10));
        app.draw(Rect::new(0, 0, 14, 10), &mut buf);
        let text: String = (0..10)
            .flat_map(|y| (0..11).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        assert_eq!(text.trim(), "", "only the bar is drawn");
        app.shutdown();
    }

    #[test]
    fn clicking_a_toolbar_tool_while_compact_expands_and_shows_it() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        let (ctx, log) = herdr_ctx(fx._tmp.path());
        app.set_herdr(ctx);
        app.apply_mouse(click(SIZE.cols - 2, 0));
        narrow(&mut app, "14");
        // the toggle is row 0, so tool b is row 2
        app.apply_mouse(click(SIZE.cols - 2, 2));
        assert!(!app.compact);
        assert_eq!(app.selected, 1);
        let l = fs::read_to_string(&log).unwrap_or_default();
        assert!(l.contains("--direction left"), "expanded: {l}");
        app.shutdown();
    }

    #[test]
    fn clicking_a_pane_tool_while_compact_stays_compact() {
        let pane = "[[tool]]\nid = \"p\"\nname = \"p\"\nicon = \"x\"\ncommand = [\"x\"]\nopen = \"pane\"\n";
        let fx = fixture(&(tool("a", "sleep 30") + pane));
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        app.set_herdr(herdr_ctx(fx._tmp.path()).0);
        app.apply_mouse(click(SIZE.cols - 2, 0));
        assert!(app.compact);
        // toggle row 0, a row 1, the rule row 2, p row 3
        app.apply_mouse(click(SIZE.cols - 2, 3));
        assert!(app.compact, "opening a pane does not need the toolbar");
        app.shutdown();
    }

    fn resizes(log: &Path) -> usize {
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("pane resize"))
            .count()
    }

    #[test]
    fn a_pane_narrower_than_the_bar_widens_to_show_the_icons() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        let (ctx, log) = herdr_ctx(fx._tmp.path());
        app.set_herdr(ctx);
        // the small bar needs three columns; the pane shows two
        app.resize(Size { rows: 10, cols: 2 });
        let l = fs::read_to_string(&log).unwrap_or_default();
        assert!(
            l.contains("pane resize --pane w1:p9 --direction left --amount 0.0050"),
            "{l}"
        );
        app.shutdown();
    }

    #[test]
    fn a_pane_as_wide_as_the_bar_is_left_alone() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        let (ctx, log) = herdr_ctx(fx._tmp.path());
        app.set_herdr(ctx);
        app.resize(Size { rows: 10, cols: 3 });
        app.resize(Size { rows: 10, cols: 40 });
        assert_eq!(resizes(&log), 0);
        app.shutdown();
    }

    #[test]
    fn a_narrow_size_herdr_will_not_widen_is_asked_about_once() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        let (ctx, log) = herdr_ctx(fx._tmp.path());
        app.set_herdr(ctx);
        app.resize(Size { rows: 10, cols: 2 });
        app.resize(Size { rows: 10, cols: 2 });
        assert_eq!(resizes(&log), 1, "no fight with Herdr");
        app.shutdown();
    }

    #[test]
    fn outside_herdr_the_toolbar_does_not_resize_itself() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        app.resize(Size { rows: 10, cols: 2 });
        assert!(app.herdr.is_none());
        app.shutdown();
    }

    #[test]
    fn hover_skips_the_rule_between_toolbar_and_pane_tools() {
        let pane = "[[tool]]\nid = \"p\"\nname = \"p\"\nicon = \"x\"\ncommand = [\"x\"]\nopen = \"pane\"\n";
        let fx = fixture(&(tool("a", "sleep 30") + &tool("b", "sleep 30") + pane));
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        // small buttons: a on row 0, b on row 1, the rule on row 2, p on row 3
        app.apply_mouse(moved(SIZE.cols - 2, 1));
        assert_eq!(app.hover, Some(ui::Target::Tool(1)));
        app.apply_mouse(moved(SIZE.cols - 2, 2));
        assert_eq!(app.hover, None, "the rule");
        app.apply_mouse(moved(SIZE.cols - 2, 3));
        assert_eq!(app.hover, Some(ui::Target::Tool(2)));
        app.shutdown();
    }

    #[test]
    fn moving_over_an_icon_names_it_and_moving_off_the_bar_clears_it() {
        let fx = fixture(&three());
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        app.apply_mouse(moved(SIZE.cols - 2, 1));
        assert_eq!(app.hover, Some(ui::Target::Tool(1)));
        app.apply_mouse(moved(SIZE.cols - 2, SIZE.rows - 1));
        assert_eq!(app.hover, None, "the marker row is no tool");
        app.apply_mouse(moved(SIZE.cols - 2, 2));
        assert_eq!(app.hover, Some(ui::Target::Tool(2)));
        app.apply_mouse(moved(3, 3));
        assert_eq!(app.hover, None, "off the bar");
        app.shutdown();
    }

    #[test]
    fn clicks_reach_the_tool_only_when_it_asked_for_the_mouse() {
        let fx = fixture("");
        let out = fx._tmp.path().join("mouse.out");
        let plain = fx._tmp.path().join("plain.out");
        let cfg = tool(
            "m",
            &format!(
                "printf '\\\\033[?1000h\\\\033[?1006h'; stty raw -echo; exec cat > {}",
                out.display()
            ),
        ) + &tool(
            "p",
            &format!("stty raw -echo; exec cat > {}", plain.display()),
        );
        fs::write(&fx.paths.global, cfg).unwrap();
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        wait_until("tool m turns mouse reporting on", || {
            app.slots[0]
                .proc
                .as_ref()
                .is_some_and(|p| p.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
        });
        app.apply_mouse(click(3, 1));
        wait_until("the click arrives as SGR, 1-based", || {
            fs::read(&out).unwrap_or_default() == b"\x1b[<0;4;2M"
        });
        // a tool that never asked gets nothing
        app.select(1);
        wait_until("tool p runs", || {
            app.slots[1]
                .proc
                .as_ref()
                .is_some_and(|p| p.state() == ToolState::Running)
        });
        std::thread::sleep(Duration::from_millis(200));
        app.apply_mouse(click(3, 1));
        std::thread::sleep(Duration::from_millis(200));
        assert!(fs::read(&plain).unwrap_or_default().is_empty());
        app.shutdown();
    }

    fn pid(app: &App, i: usize) -> u32 {
        app.slots[i]
            .proc
            .as_ref()
            .unwrap_or_else(|| panic!("slot {i} did not start: {:?}", app.slots[i].last_error))
            .pid()
            .unwrap()
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
    fn a_graphics_tool_draws_its_images_in_the_slot_and_clears_them_when_hidden() {
        let fx = fixture("");
        let script = fx._tmp.path().join("draw.sh");
        fs::write(
            &script,
            "stty raw -echo; printf 'ab\\033_Ga=T,f=100;AAAA\\033\\\\'; sleep 30\n",
        )
        .unwrap();
        let cfg = format!(
            "[[tool]]\nid = \"browser\"\nname = \"Browser\"\nicon = \"o\"\ncommand = [\"sh\", \"{}\"]\ngraphics = true\n\n{}",
            script.display(),
            tool("b", "echo B; sleep 30")
        );
        fs::write(&fx.paths.global, cfg).unwrap();
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        app.select(0);
        let mut out = Vec::new();
        let start = Instant::now();
        while out.is_empty() {
            assert!(start.elapsed() < Duration::from_secs(10), "no image");
            out = app.graphics_out();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            out,
            crate::graphics::placed(b"\x1b_Ga=T,f=100;AAAA\x1b\\", 0, 0, 0, 2),
            "placed at the tool's cursor, the slot's top-left being the pane's"
        );
        app.select(1);
        assert_eq!(
            app.graphics_out(),
            crate::graphics::DELETE_ALL,
            "switching away clears its images"
        );
        assert!(app.graphics_out().is_empty());
    }

    #[test]
    fn a_pane_tool_never_takes_the_slot() {
        let cfg = tool("a", "echo A; sleep 30")
            + "[[tool]]\nid = \"browser\"\nname = \"Browser\"\nicon = \"o\"\ncommand = [\"terminal-browser\", \"open\"]\nopen = \"pane\"\n";
        let fx = fixture(&cfg);
        let mut app = App::new(fx.paths.clone(), fx.env.clone(), SIZE).unwrap();
        app.select(0);
        app.select(1);
        assert_eq!(app.selected, 0, "the slot keeps showing a");
        assert!(app.slots[1].proc.is_none(), "nothing runs in the slot");
        assert!(
            app.message.as_deref().unwrap_or("").contains("Herdr"),
            "outside Herdr it says why: {:?}",
            app.message
        );

        // inside Herdr it opens the pane in the background
        let log = fx._tmp.path().join("herdr.log");
        let state = fx._tmp.path().join("state");
        fs::create_dir_all(&state).unwrap();
        app.set_herdr(crate::herdr::HerdrContext {
            bin: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/fake-herdr.sh"),
            workspace_id: Some("w1".into()),
            pane_id: Some("w1:p9".into()),
            tab_id: Some("w1:t1".into()),
            target_pane_id: Some("w1:p9".into()),
            workspace_cwd: fx.env.cwd.clone(),
            config_dir: fx._tmp.path().join("config"),
            state_dir: state,
            socket: fx._tmp.path().join("herdr.sock"),
            inherit: vec![],
            extra_env: vec![
                ("FAKE_HERDR_LOG".into(), log.display().to_string()),
                ("FAKE_NEIGHBOR".into(), "w1:p3".into()),
            ],
        });
        app.select(1);
        assert_eq!(app.selected, 0);
        wait_until("the pane is opened", || {
            fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("pane run w1:p12 'terminal-browser' 'open'")
        });
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
        assert!(
            app.slots[0].proc.is_some(),
            "slot 0 did not start: {:?}",
            app.slots[0].last_error
        );
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
