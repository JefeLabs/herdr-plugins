//! App state machine and run loop.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::config::{Config, Dirs, Handoff, KINDS, template};
use crate::conversation::{Turn, TurnMsg, seed_prompt};
use crate::editor::TextArea;
use crate::provider::Reply;
use crate::store::{Group, Intent, SaveError, State, Store};
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
    /// Leave the TUI, run `$EDITOR` on this file, come back and reload.
    ExternalEdit(usize),
}

enum Row {
    Group {
        path: PathBuf,
        name: String,
        depth: usize,
        count: usize,
    },
    Intent {
        intent: Intent,
        depth: usize,
    },
}

pub struct ChatState {
    pub turn: Option<Turn>,
    pub transcript: Vec<(String, String)>,
    pub latest: Option<Reply>,
    pub input: String,
    pub provider: String,
    pub conv_id: Option<String>,
    pub hint: Option<String>,
}

pub enum Pending {
    OverwriteSave,
    DiscardEdits,
    OverwriteApply(String),
}

pub enum InputAction {
    AddIntent { group: PathBuf },
    NewGroup { parent: PathBuf },
    Attach,
    MoveTo,
}

#[allow(clippy::large_enum_variant)] // Chat carries the live turn; boxing buys nothing here
pub enum Screen {
    Browse,
    Edit,
    Chat(ChatState),
    Diff {
        proposal: String,
        questions: Vec<String>,
    },
    SendPick {
        handoffs: Vec<Handoff>,
        selected: usize,
    },
    SendConfirm {
        handoff: Handoff,
        provider: String,
        repo: PathBuf,
        prompt: String,
        scroll: u16,
    },
    Prompt {
        question: String,
        pending: Pending,
    },
    Input {
        label: String,
        text: String,
        action: InputAction,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub depth: usize,
    pub label: String,
    pub is_group: bool,
    pub questions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub rows: Vec<RowView>,
    pub selected: usize,
    pub screen: &'static str,
    pub can_apply: bool,
    pub hint: Option<String>,
    pub message: Option<String>,
    pub turn_running: bool,
    pub transcript: Vec<(String, String)>,
    pub transcript_len: usize,
}

pub struct App {
    pub store: Store,
    pub cfg: Config,
    pub dirs: Dirs,
    pub tree: Group,
    rows: Vec<Row>,
    pub selected: usize,
    tree_scroll: usize,
    collapsed: HashSet<PathBuf>,
    pub screen: Screen,
    pub editor: TextArea,
    /// Intent loaded into the editor (its `loaded_updated` drives conflict checks).
    editing: Option<Intent>,
    pub message: Option<String>,
    /// Extra environment for provider processes (tests point the fake at fixtures).
    pub provider_env: Vec<(String, String)>,
    /// Extra environment for `herdr` calls on send (tests use the fake).
    pub herdr_env: Vec<(String, String)>,
    /// Chat state kept while the Diff screen is showing.
    parked_chat: Option<ChatState>,
}

impl App {
    pub fn new(cfg: Config, dirs: Dirs) -> anyhow::Result<Self> {
        let store = Store::open(&cfg.root)?;
        let tree = store.scan()?;
        let mut app = Self {
            store,
            cfg,
            dirs,
            tree,
            rows: Vec::new(),
            selected: 0,
            tree_scroll: 0,
            collapsed: HashSet::new(),
            screen: Screen::Browse,
            editor: TextArea::default(),
            editing: None,
            message: None,
            provider_env: Vec::new(),
            herdr_env: Vec::new(),
            parked_chat: None,
        };
        app.rebuild_rows();
        Ok(app)
    }

    // ---- tree -------------------------------------------------------------

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        fn walk(g: &Group, depth: usize, collapsed: &HashSet<PathBuf>, rows: &mut Vec<Row>) {
            for sub in &g.groups {
                let count = count_intents(sub);
                rows.push(Row::Group {
                    path: sub.path.clone(),
                    name: sub.name.clone(),
                    depth,
                    count,
                });
                if !collapsed.contains(&sub.path) {
                    walk(sub, depth + 1, collapsed, rows);
                }
            }
            for it in &g.intents {
                rows.push(Row::Intent {
                    intent: it.clone(),
                    depth,
                });
            }
        }
        fn count_intents(g: &Group) -> usize {
            g.intents.len() + g.groups.iter().map(count_intents).sum::<usize>()
        }
        walk(&self.tree, 0, &self.collapsed, &mut rows);
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    pub fn rescan(&mut self) {
        let keep = self.selected_path();
        match self.store.scan() {
            Ok(t) => self.tree = t,
            Err(e) => self.message = Some(format!("rescan failed: {e:#}")),
        }
        self.rebuild_rows();
        if let Some(p) = keep
            && let Some(i) = self.rows.iter().position(|r| row_path(r) == p)
        {
            self.selected = i;
        }
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.rows.get(self.selected).map(row_path)
    }

    pub fn selected_intent(&self) -> Option<&Intent> {
        match self.rows.get(self.selected) {
            Some(Row::Intent { intent, .. }) => Some(intent),
            _ => None,
        }
    }

    fn selected_intent_mut(&mut self) -> Option<&mut Intent> {
        match self.rows.get_mut(self.selected) {
            Some(Row::Intent { intent, .. }) => Some(intent),
            _ => None,
        }
    }

    /// Group directory the selection belongs to (the group itself, or the intent's parent).
    fn selected_group_dir(&self) -> PathBuf {
        match self.rows.get(self.selected) {
            Some(Row::Group { path, .. }) => path.clone(),
            Some(Row::Intent { intent, .. }) => intent
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.store.root().to_path_buf()),
            None => self.store.root().join(crate::store::INBOX),
        }
    }

    fn rel(&self, abs: &Path) -> PathBuf {
        abs.strip_prefix(self.store.root())
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| abs.to_path_buf())
    }

    fn select_path(&mut self, p: &Path) {
        if let Some(i) = self.rows.iter().position(|r| row_path(r) == p) {
            self.selected = i;
        }
    }

    // ---- persistence helpers ----------------------------------------------

    fn save_selected(&mut self, mut f: impl FnMut(&mut Intent)) {
        if let Some(it) = self.selected_intent_mut() {
            f(it);
            if let Err(e) = Store::save_force(it) {
                self.message = Some(format!("save failed: {e:#}"));
            }
        }
    }

    // ---- keys -------------------------------------------------------------

    pub fn key(&mut self, k: KeyEvent) -> Flow {
        if k.kind == KeyEventKind::Release {
            return Flow::Continue;
        }
        self.message = None;
        match std::mem::replace(&mut self.screen, Screen::Browse) {
            Screen::Browse => self.key_browse(k),
            Screen::Edit => {
                self.screen = Screen::Edit;
                self.key_edit(k)
            }
            Screen::Chat(st) => self.key_chat(st, k),
            Screen::Diff {
                proposal,
                questions,
            } => self.key_diff(proposal, questions, k),
            Screen::SendPick { handoffs, selected } => self.key_send_pick(handoffs, selected, k),
            Screen::SendConfirm {
                handoff,
                provider,
                repo,
                prompt,
                scroll,
            } => self.key_send_confirm(handoff, provider, repo, prompt, scroll, k),
            Screen::Prompt { question, pending } => self.key_prompt(question, pending, k),
            Screen::Input {
                label,
                text,
                action,
            } => self.key_input(label, text, action, k),
        }
    }

    fn key_browse(&mut self, k: KeyEvent) -> Flow {
        self.screen = Screen::Browse;
        match k.code {
            KeyCode::Char('q') => return Flow::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(Row::Group { path, .. }) = self.rows.get(self.selected) {
                    self.collapsed.insert(path.clone());
                    self.rebuild_rows();
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(Row::Group { path, .. }) = self.rows.get(self.selected) {
                    self.collapsed.remove(path);
                    self.rebuild_rows();
                }
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                if let Some(it) = self.selected_intent().cloned() {
                    self.editor = TextArea::from_text(&it.body);
                    self.editing = Some(it);
                    self.screen = Screen::Edit;
                }
            }
            KeyCode::Char('a') => {
                let group = self.selected_group_dir();
                self.screen = Screen::Input {
                    label: "title".into(),
                    text: String::new(),
                    action: InputAction::AddIntent { group },
                };
            }
            KeyCode::Char('n') => {
                let parent = self.selected_group_dir();
                self.screen = Screen::Input {
                    label: "new group".into(),
                    text: String::new(),
                    action: InputAction::NewGroup { parent },
                };
            }
            KeyCode::Char('m') => {
                if self.selected_intent().is_some() {
                    self.screen = Screen::Input {
                        label: "move to group".into(),
                        text: String::new(),
                        action: InputAction::MoveTo,
                    };
                }
            }
            KeyCode::Char('A') => {
                if self.selected_intent().is_some() {
                    self.screen = Screen::Input {
                        label: "attach file path".into(),
                        text: String::new(),
                        action: InputAction::Attach,
                    };
                }
            }
            KeyCode::Char('o') => {
                if let Some(it) = self.selected_intent() {
                    let first = Store::attachments(it).into_iter().next();
                    if let Some((name, _)) = first {
                        let path = it.assets_dir().join(name);
                        let _ = std::process::Command::new("open").arg(&path).spawn();
                    } else {
                        self.message = Some("no attachments".into());
                    }
                }
            }
            KeyCode::Char('K') => {
                self.save_selected(|it| {
                    let i = KINDS.iter().position(|k| *k == it.fm.kind).unwrap_or(0);
                    it.fm.kind = KINDS[(i + 1) % KINDS.len()].to_string();
                });
            }
            KeyCode::Char('R') => self.save_selected(|it| it.fm.state = State::Ready),
            KeyCode::Char('U') => self.save_selected(|it| it.fm.state = State::Refining),
            KeyCode::Char('p') => {
                if let Some(it) = self.selected_intent().cloned() {
                    self.open_chat(&it, None);
                }
            }
            KeyCode::Char('s') => {
                if let Some(it) = self.selected_intent() {
                    let handoffs: Vec<Handoff> = self
                        .cfg
                        .handoffs
                        .iter()
                        .filter(|h| h.applies_to(&it.fm.kind))
                        .cloned()
                        .collect();
                    if handoffs.is_empty() {
                        self.message = Some(format!("no handoff for kind `{}`", it.fm.kind));
                    } else {
                        self.screen = Screen::SendPick {
                            handoffs,
                            selected: 0,
                        };
                    }
                }
            }
            KeyCode::Char('r') => self.rescan(),
            _ => {}
        }
        Flow::Continue
    }

    fn key_edit(&mut self, k: KeyEvent) -> Flow {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match (k.code, ctrl) {
            (KeyCode::Char('s'), true) => self.save_editor(false),
            (KeyCode::Char('p'), true) => {
                let sel = self.editor.selection();
                if let Some(mut it) = self.editing.clone() {
                    it.body = self.editor.text();
                    self.open_chat(&it, sel.as_deref());
                }
            }
            (KeyCode::Char('e'), true) => return Flow::ExternalEdit(self.selected),
            (KeyCode::Esc, _) => {
                let dirty = self
                    .editing
                    .as_ref()
                    .map(|e| e.body != self.editor.text())
                    .unwrap_or(false);
                if dirty {
                    self.screen = Screen::Prompt {
                        question: "Discard unsaved edits? (y/n)".into(),
                        pending: Pending::DiscardEdits,
                    };
                } else {
                    self.editing = None;
                    self.screen = Screen::Browse;
                }
            }
            _ => {
                self.editor.handle(k, 20);
            }
        }
        Flow::Continue
    }

    fn save_editor(&mut self, force: bool) {
        let Some(mut it) = self.editing.clone() else {
            return;
        };
        it.body = self.editor.text();
        let result = if force {
            Store::save_force(&mut it).map_err(SaveError::Io)
        } else {
            Store::save(&mut it)
        };
        match result {
            Ok(()) => {
                self.editing = Some(it);
                self.message = Some("saved".into());
                self.rescan();
            }
            Err(SaveError::Conflict) => {
                self.screen = Screen::Prompt {
                    question: "File changed on disk. Overwrite? (y/n)".into(),
                    pending: Pending::OverwriteSave,
                };
            }
            Err(SaveError::Io(e)) => self.message = Some(format!("save failed: {e:#}")),
        }
    }

    // ---- chat -------------------------------------------------------------

    fn open_chat(&mut self, intent: &Intent, selection: Option<&str>) {
        let provider = self.cfg.polish_provider.clone();
        let conv_id = intent.fm.conversations.get(&provider).cloned();
        let mut st = ChatState {
            turn: None,
            transcript: Vec::new(),
            latest: None,
            input: String::new(),
            provider,
            conv_id,
            hint: None,
        };
        self.select_path(&intent.path);
        let tpl = template(&self.dirs, &intent.fm.kind, true)
            .unwrap_or_else(|_| "Refine this intent.".to_string());
        let prompt = seed_prompt(&tpl, &intent.body, selection, &intent.fm.kind);
        let shown = match selection {
            Some(s) => format!(
                "polish (focus: {})",
                s.lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect::<String>()
            ),
            None => "polish".to_string(),
        };
        self.start_turn(&mut st, intent, &prompt, &shown);
        self.screen = Screen::Chat(st);
    }

    fn start_turn(&mut self, st: &mut ChatState, intent: &Intent, prompt: &str, shown: &str) {
        let Some(spec) = self.cfg.providers.get(&st.provider).cloned() else {
            st.hint = Some(format!("unknown provider `{}`", st.provider));
            return;
        };
        let cwd = intent
            .fm
            .repo
            .clone()
            .unwrap_or_else(|| self.store.root().to_path_buf());
        let cwd = if cwd.is_dir() {
            cwd
        } else {
            self.store.root().to_path_buf()
        };
        match Turn::start(
            &spec,
            prompt,
            st.conv_id.as_deref(),
            &cwd,
            &self.provider_env,
        ) {
            Ok(turn) => {
                st.transcript.push(("user".into(), shown.to_string()));
                st.transcript.push(("agent".into(), String::new()));
                st.latest = None;
                st.hint = None;
                st.turn = Some(turn);
                let _ = Store::append_transcript(intent, "user", shown);
            }
            Err(e) => st.hint = Some(format!("{e:#}")),
        }
    }

    fn key_chat(&mut self, mut st: ChatState, k: KeyEvent) -> Flow {
        match k.code {
            KeyCode::Esc => {
                if let Some(mut t) = st.turn.take() {
                    t.cancel();
                    if let Some(last) = st.transcript.last_mut() {
                        last.1.push_str("\n[cancelled]");
                    }
                    self.screen = Screen::Chat(st);
                } else {
                    self.screen = Screen::Browse;
                    self.rescan();
                }
            }
            KeyCode::Enter if st.turn.is_none() => {
                let text = st.input.trim().to_string();
                if !text.is_empty()
                    && let Some(it) = self.selected_intent().cloned()
                {
                    st.input.clear();
                    self.start_turn(&mut st, &it, &text, &text);
                }
                self.screen = Screen::Chat(st);
            }
            KeyCode::Char('a') if st.turn.is_none() && st.input.is_empty() => {
                match st.latest.as_ref().and_then(|r| r.proposal.clone()) {
                    Some(proposal) => {
                        let questions = st
                            .latest
                            .as_ref()
                            .map(|r| r.questions.clone())
                            .unwrap_or_default();
                        self.screen = Screen::Diff {
                            proposal,
                            questions,
                        };
                        self.parked_chat = Some(st);
                    }
                    None => {
                        st.hint = Some("no proposal to apply in the latest reply".into());
                        self.screen = Screen::Chat(st);
                    }
                }
            }
            KeyCode::Char('d') if st.turn.is_none() && st.input.is_empty() => {
                st.latest = None;
                st.hint = Some("proposal dismissed".into());
                self.screen = Screen::Chat(st);
            }
            KeyCode::Backspace => {
                st.input.pop();
                self.screen = Screen::Chat(st);
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                st.input.push(c);
                self.screen = Screen::Chat(st);
            }
            _ => self.screen = Screen::Chat(st),
        }
        Flow::Continue
    }

    fn key_diff(&mut self, proposal: String, questions: Vec<String>, k: KeyEvent) -> Flow {
        match k.code {
            KeyCode::Enter => {
                self.apply_proposal(&proposal, &questions, false);
            }
            KeyCode::Esc => {
                self.screen = match self.parked_chat.take() {
                    Some(st) => Screen::Chat(st),
                    None => Screen::Browse,
                };
            }
            _ => {
                self.screen = Screen::Diff {
                    proposal,
                    questions,
                }
            }
        }
        Flow::Continue
    }

    fn apply_proposal(&mut self, proposal: &str, questions: &[String], force: bool) {
        let Some(mut it) = self.selected_intent().cloned() else {
            self.screen = Screen::Browse;
            return;
        };
        // reload for the freshest `updated`, then apply
        if let Ok(fresh) = Store::load(&it.path) {
            it.loaded_updated = fresh.loaded_updated;
            it.fm = fresh.fm;
        }
        it.body = merge_questions(proposal, questions);
        it.fm.polished_at = Some(chrono::Local::now());
        if it.fm.state == State::Raw {
            it.fm.state = State::Refining;
        }
        if let Some(st) = self.parked_chat.as_ref()
            && let Some(id) = &st.conv_id
        {
            it.fm.conversations.insert(st.provider.clone(), id.clone());
        }
        let result = if force {
            Store::save_force(&mut it).map_err(SaveError::Io)
        } else {
            Store::save(&mut it)
        };
        match result {
            Ok(()) => {
                self.message = Some("proposal applied".into());
                self.rescan();
                self.screen = match self.parked_chat.take() {
                    Some(mut st) => {
                        st.latest = None;
                        st.hint = Some("applied".into());
                        Screen::Chat(st)
                    }
                    None => Screen::Browse,
                };
            }
            Err(SaveError::Conflict) => {
                self.screen = Screen::Prompt {
                    question: "File changed on disk. Overwrite with the proposal? (y/n)".into(),
                    pending: Pending::OverwriteApply(merge_questions(proposal, questions)),
                };
            }
            Err(SaveError::Io(e)) => {
                self.message = Some(format!("apply failed: {e:#}"));
                self.screen = Screen::Browse;
            }
        }
    }

    // ---- send (screens; the work is in `send.rs`, Task 7) --------------------

    fn key_send_pick(&mut self, handoffs: Vec<Handoff>, mut selected: usize, k: KeyEvent) -> Flow {
        match k.code {
            KeyCode::Esc => self.screen = Screen::Browse,
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(handoffs.len() - 1);
                self.screen = Screen::SendPick { handoffs, selected };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.screen = Screen::SendPick { handoffs, selected };
            }
            KeyCode::Enter => {
                let handoff = handoffs[selected].clone();
                let provider = if handoff.provider.is_empty() {
                    self.cfg.send_provider.clone()
                } else {
                    handoff.provider.clone()
                };
                match self.selected_intent().cloned() {
                    Some(it) => {
                        let (repo, prompt) = crate::send::prepare(
                            &self.cfg,
                            &handoff,
                            &it,
                            &Store::attachments(&it),
                        );
                        self.screen = Screen::SendConfirm {
                            handoff,
                            provider,
                            repo,
                            prompt,
                            scroll: 0,
                        };
                    }
                    None => self.screen = Screen::Browse,
                }
            }
            _ => self.screen = Screen::SendPick { handoffs, selected },
        }
        Flow::Continue
    }

    fn key_send_confirm(
        &mut self,
        handoff: Handoff,
        mut provider: String,
        repo: PathBuf,
        prompt: String,
        mut scroll: u16,
        k: KeyEvent,
    ) -> Flow {
        match k.code {
            KeyCode::Esc => self.screen = Screen::Browse,
            KeyCode::Tab => {
                let names: Vec<&String> = self.cfg.providers.keys().collect();
                if let Some(i) = names.iter().position(|n| **n == provider) {
                    provider = names[(i + 1) % names.len()].clone();
                }
                self.screen = Screen::SendConfirm {
                    handoff,
                    provider,
                    repo,
                    prompt,
                    scroll,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1);
                self.screen = Screen::SendConfirm {
                    handoff,
                    provider,
                    repo,
                    prompt,
                    scroll,
                };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                self.screen = Screen::SendConfirm {
                    handoff,
                    provider,
                    repo,
                    prompt,
                    scroll,
                };
            }
            KeyCode::Enter => {
                self.screen = Screen::Browse;
                self.do_send(&handoff, &provider, &repo, &prompt);
            }
            _ => {
                self.screen = Screen::SendConfirm {
                    handoff,
                    provider,
                    repo,
                    prompt,
                    scroll,
                }
            }
        }
        Flow::Continue
    }

    fn do_send(&mut self, handoff: &Handoff, provider: &str, repo: &Path, prompt: &str) {
        let Some(it) = self.selected_intent().cloned() else {
            return;
        };
        let kind = self
            .cfg
            .providers
            .get(provider)
            .map(|p| p.send_kind.clone())
            .unwrap_or_else(|| provider.to_string());
        match crate::send::send_intent(&self.herdr_env, repo, &it, &kind, prompt) {
            Ok(outcome) => {
                let handoff_name = handoff.name.clone();
                self.save_selected(|i| {
                    i.fm.state = State::Sent;
                    i.fm.sent_to = Some(crate::store::SentTo {
                        handoff: handoff_name.clone(),
                        workspace_id: outcome.workspace_id.clone(),
                        agent: outcome.agent.clone(),
                        at: chrono::Local::now(),
                    });
                });
                self.message = Some(format!(
                    "sent to {} in workspace {} (agent {})",
                    handoff.name, outcome.workspace_id, outcome.agent
                ));
            }
            Err(e) => self.message = Some(format!("send failed at {}: {}", e.step, e.detail)),
        }
    }

    // ---- prompts and inputs -------------------------------------------------

    fn key_prompt(&mut self, question: String, pending: Pending, k: KeyEvent) -> Flow {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => match pending {
                Pending::OverwriteSave => {
                    self.screen = Screen::Edit;
                    self.save_editor(true);
                }
                Pending::DiscardEdits => {
                    self.editing = None;
                    self.screen = Screen::Browse;
                }
                Pending::OverwriteApply(body) => {
                    self.apply_proposal(&body, &[], true);
                }
            },
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = match pending {
                    Pending::OverwriteSave | Pending::DiscardEdits => Screen::Edit,
                    Pending::OverwriteApply(_) => match self.parked_chat.take() {
                        Some(st) => Screen::Chat(st),
                        None => Screen::Browse,
                    },
                };
            }
            _ => self.screen = Screen::Prompt { question, pending },
        }
        Flow::Continue
    }

    fn key_input(
        &mut self,
        label: String,
        mut text: String,
        action: InputAction,
        k: KeyEvent,
    ) -> Flow {
        match k.code {
            KeyCode::Esc => self.screen = Screen::Browse,
            KeyCode::Backspace => {
                text.pop();
                self.screen = Screen::Input {
                    label,
                    text,
                    action,
                };
            }
            KeyCode::Enter => {
                self.screen = Screen::Browse;
                let value = text.trim().to_string();
                if value.is_empty() {
                    return Flow::Continue;
                }
                match action {
                    InputAction::AddIntent { group } => {
                        let rel = self.rel(&group);
                        let kind = "idea";
                        let body = template(&self.dirs, kind, false).unwrap_or_default();
                        match self.store.create(
                            &rel,
                            &value,
                            kind,
                            &body,
                            crate::send::main_checkout(
                                &std::env::current_dir().unwrap_or_default(),
                            )
                            .ok(),
                        ) {
                            Ok(it) => {
                                self.collapsed.remove(&group);
                                self.rescan();
                                self.select_path(&it.path);
                            }
                            Err(e) => self.message = Some(format!("create failed: {e:#}")),
                        }
                    }
                    InputAction::NewGroup { parent } => {
                        let rel = self.rel(&parent);
                        match self.store.create_group(&rel, &value) {
                            Ok(p) => {
                                self.rescan();
                                self.select_path(&p);
                            }
                            Err(e) => self.message = Some(format!("group failed: {e:#}")),
                        }
                    }
                    InputAction::Attach => {
                        if let Some(it) = self.selected_intent().cloned() {
                            let src = PathBuf::from(shellexpand::tilde(&value).into_owned());
                            match Store::attach(&it, &src) {
                                Ok(_) => self.message = Some("attached".into()),
                                Err(e) => self.message = Some(format!("attach failed: {e:#}")),
                            }
                        }
                    }
                    InputAction::MoveTo => {
                        if let Some(it) = self.selected_intent().cloned() {
                            match self.store.move_intent(&it, Path::new(&value)) {
                                Ok(moved) => {
                                    self.rescan();
                                    self.select_path(&moved.path);
                                }
                                Err(e) => self.message = Some(format!("move failed: {e:#}")),
                            }
                        }
                    }
                }
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                text.push(c);
                self.screen = Screen::Input {
                    label,
                    text,
                    action,
                };
            }
            _ => {
                self.screen = Screen::Input {
                    label,
                    text,
                    action,
                }
            }
        }
        Flow::Continue
    }

    // ---- ticking ------------------------------------------------------------

    /// Drain messages from a running turn.
    pub fn tick(&mut self) {
        let intent = self.selected_intent().cloned();
        let Screen::Chat(st) = &mut self.screen else {
            return;
        };
        let Some(turn) = st.turn.as_ref() else { return };
        let mut finished = false;
        while let Some(msg) = turn.try_recv() {
            match msg {
                TurnMsg::Delta(d) => {
                    if let Some(last) = st.transcript.last_mut() {
                        last.1.push_str(&d);
                    }
                }
                TurnMsg::ConversationId(id) => {
                    st.conv_id = Some(id.clone());
                    if let Some(it) = intent.as_ref() {
                        let mut it = it.clone();
                        it.fm.conversations.insert(st.provider.clone(), id);
                        let _ = Store::save_force(&mut it);
                    }
                }
                TurnMsg::Finished(reply) => {
                    if let Some(it) = intent.as_ref()
                        && let Some(last) = st.transcript.last()
                    {
                        let _ = Store::append_transcript(it, "agent", &last.1);
                    }
                    st.hint = if reply.proposal.is_some() {
                        Some("a = apply, d = dismiss".into())
                    } else {
                        Some("no proposal in this reply; ask for one".into())
                    };
                    st.latest = Some(reply);
                    finished = true;
                }
                TurnMsg::Failed(e) => {
                    st.hint = Some(e);
                    finished = true;
                }
            }
        }
        if finished {
            st.turn = None;
            self.rescan_keep_screen();
        }
    }

    fn rescan_keep_screen(&mut self) {
        let keep = self.selected_path();
        if let Ok(t) = self.store.scan() {
            self.tree = t;
        }
        self.rebuild_rows();
        if let Some(p) = keep {
            self.select_path(&p);
        }
    }

    // ---- views ----------------------------------------------------------------

    pub fn view(&self) -> View {
        let rows = self
            .rows
            .iter()
            .map(|r| match r {
                Row::Group { name, depth, .. } => RowView {
                    depth: *depth,
                    label: name.clone(),
                    is_group: true,
                    questions: 0,
                },
                Row::Intent { intent, depth } => RowView {
                    depth: *depth,
                    label: intent.fm.title.clone(),
                    is_group: false,
                    questions: intent.open_questions(),
                },
            })
            .collect();
        let (screen, can_apply, hint, turn_running, transcript) = match &self.screen {
            Screen::Browse => ("browse", false, None, false, Vec::new()),
            Screen::Edit => ("edit", false, None, false, Vec::new()),
            Screen::Chat(st) => (
                "chat",
                st.latest
                    .as_ref()
                    .map(|r| r.proposal.is_some())
                    .unwrap_or(false),
                st.hint.clone(),
                st.turn.is_some(),
                st.transcript.clone(),
            ),
            Screen::Diff { .. } => ("diff", true, None, false, Vec::new()),
            Screen::SendPick { .. } => ("send-pick", false, None, false, Vec::new()),
            Screen::SendConfirm { .. } => ("send-confirm", false, None, false, Vec::new()),
            Screen::Prompt { question, .. } => {
                ("prompt", false, Some(question.clone()), false, Vec::new())
            }
            Screen::Input { label, .. } => ("input", false, Some(label.clone()), false, Vec::new()),
        };
        View {
            rows,
            selected: self.selected,
            screen,
            can_apply,
            hint,
            message: self.message.clone(),
            turn_running,
            transcript_len: transcript.len(),
            transcript,
        }
    }

    pub fn draw(&self, area: Rect, buf: &mut Buffer) {
        let (left, right, status) = ui::split(area);
        let tree_rows: Vec<ui::TreeRow> = self
            .rows
            .iter()
            .map(|r| match r {
                Row::Group {
                    path,
                    name,
                    depth,
                    count,
                } => ui::TreeRow {
                    depth: *depth,
                    label: name,
                    is_group: true,
                    collapsed: self.collapsed.contains(path),
                    questions: 0,
                    state: None,
                    count: *count,
                },
                Row::Intent { intent, depth } => ui::TreeRow {
                    depth: *depth,
                    label: &intent.fm.title,
                    is_group: false,
                    collapsed: false,
                    questions: intent.open_questions(),
                    state: Some(intent.fm.state),
                    count: 0,
                },
            })
            .collect();
        ui::render_tree(&tree_rows, self.selected, self.tree_scroll, left, buf);

        match &self.screen {
            Screen::Edit => self.editor.render(right, buf, true),
            Screen::Chat(st) => {
                let (top, bottom) = ui::split_chat(right);
                if let Some(it) = self.selected_intent() {
                    ui::render_detail(it, &Store::attachments(it), top, buf);
                }
                ui::render_chat(
                    &st.transcript,
                    &st.input,
                    st.turn.is_some(),
                    st.hint.as_deref(),
                    &st.provider,
                    bottom,
                    buf,
                );
            }
            Screen::Diff { proposal, .. } => {
                let old = self
                    .selected_intent()
                    .map(|i| i.body.clone())
                    .unwrap_or_default();
                ui::render_diff(&old, proposal, right, buf);
            }
            Screen::SendPick { handoffs, selected } => {
                let items: Vec<String> = handoffs
                    .iter()
                    .map(|h| format!("{}  ({})  → {}", h.name, h.kinds.join(","), h.provider))
                    .collect();
                ui::render_list("send with", &items, *selected, right, buf);
            }
            Screen::SendConfirm {
                handoff,
                provider,
                repo,
                prompt,
                scroll,
            } => {
                let header = format!(
                    "handoff: {}   provider: {} (Tab to change)   repo: {}\n\nEnter = send, Esc = cancel\n\n",
                    handoff.name,
                    provider,
                    repo.display()
                );
                let text = header + prompt;
                ratatui::widgets::Paragraph::new(text)
                    .wrap(ratatui::widgets::Wrap { trim: false })
                    .scroll((*scroll, 0))
                    .render(right, buf);
            }
            _ => {
                if let Some(it) = self.selected_intent() {
                    ui::render_detail(it, &Store::attachments(it), right, buf);
                } else {
                    ratatui::widgets::Paragraph::new("a add intent · n new group · ? help")
                        .render(right, buf);
                }
            }
        }

        let status_text = match &self.screen {
            Screen::Input { label, text, .. } => format!("{label}: {text}▏"),
            Screen::Prompt { question, .. } => question.clone(),
            Screen::Edit => "Ctrl-s save · Ctrl-p polish · Ctrl-e $EDITOR · Esc back".to_string(),
            Screen::Chat(_) => "Enter send · a apply · d dismiss · Esc cancel/back".to_string(),
            _ => match &self.message {
                Some(m) => m.clone(),
                None => "a add  n group  m move  e edit  p polish  s send  R ready  K kind  A attach  o open  r rescan  q quit".to_string(),
            },
        };
        ui::render_status(&status_text, status, buf);
    }
}

fn row_path(r: &Row) -> PathBuf {
    match r {
        Row::Group { path, .. } => path.clone(),
        Row::Intent { intent, .. } => intent.path.clone(),
    }
}

/// The proposal plus an `## Open questions` section holding the questions
/// (merged with any the proposal already lists).
pub fn merge_questions(proposal: &str, questions: &[String]) -> String {
    let mut body = proposal.trim_end().to_string();
    body.push('\n');
    if questions.is_empty() {
        return body;
    }
    let existing: Vec<String> = {
        let mut v = Vec::new();
        let mut in_q = false;
        for l in body.lines() {
            if l.starts_with("## ") {
                in_q = l.trim().eq_ignore_ascii_case("## open questions");
                continue;
            }
            if in_q && let Some(q) = l.trim_start().strip_prefix("- ") {
                v.push(q.trim().to_string());
            }
        }
        v
    };
    let new: Vec<&String> = questions.iter().filter(|q| !existing.contains(q)).collect();
    if new.is_empty() {
        return body;
    }
    if !existing.is_empty() || body.to_lowercase().contains("## open questions") {
        // append to the existing section (which is at the end by convention)
        body = body.trim_end().to_string();
        body.push('\n');
    } else {
        body.push_str("\n## Open questions\n");
    }
    for q in new {
        body.push_str("- ");
        body.push_str(q);
        body.push('\n');
    }
    body
}

// ---- run loop (same shape as the toolbar's) -------------------------------------

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

fn restore_terminal() {
    use crossterm::{cursor::Show, event::DisableBracketedPaste, terminal::LeaveAlternateScreen};
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
}

fn init_terminal() -> ratatui::DefaultTerminal {
    let t = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    t
}

pub fn run(cfg: Config, dirs: Dirs) -> anyhow::Result<()> {
    install_signal_handlers();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    let mut app = App::new(cfg, dirs)?;
    let mut terminal = init_terminal();

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
            return Ok(());
        }
        if STOP.load(Ordering::Relaxed) {
            return Ok(());
        }
        app.tick();
        match events_rx.recv_timeout(Duration::from_millis(33)) {
            Ok(Event::Key(k)) => match app.key(k) {
                Flow::Quit => return Ok(()),
                Flow::ExternalEdit(_) => {
                    if let Some(it) = app.editing.clone() {
                        restore_terminal();
                        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                        let _ = std::process::Command::new("sh")
                            .arg("-c")
                            .arg(format!("{editor} \"$1\""))
                            .arg("--")
                            .arg(&it.path)
                            .status();
                        terminal = init_terminal();
                        if let Ok(fresh) = Store::load(&it.path) {
                            app.editor = TextArea::from_text(&fresh.body);
                            app.editing = Some(fresh);
                        }
                        app.rescan();
                    }
                }
                Flow::Continue => {}
            },
            Ok(Event::Paste(text)) => {
                if matches!(app.screen, Screen::Edit) {
                    for ch in text.chars() {
                        let code = if ch == '\n' {
                            KeyCode::Enter
                        } else {
                            KeyCode::Char(ch)
                        };
                        app.editor
                            .handle(KeyEvent::new(code, KeyModifiers::NONE), 20);
                    }
                }
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    })();

    if let Screen::Chat(st) = &mut app.screen
        && let Some(t) = st.turn.as_mut()
    {
        t.cancel();
    }
    restore_terminal();
    match outcome {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "herdr-intents: {e:#}");
            std::process::exit(1)
        }
    }
}

/// `add` mode: popup asks for a title; otherwise title from args or stdin's first line, body = rest.
pub fn add(cfg: Config, _dirs: Dirs, title: Option<String>, popup: bool) -> anyhow::Result<()> {
    let store = Store::open(&cfg.root)?;
    let repo = crate::send::main_checkout(&std::env::current_dir().unwrap_or_default()).ok();
    if popup {
        let mut terminal = init_terminal();
        let mut text = title.unwrap_or_default();
        let result: anyhow::Result<Option<String>> = (|| loop {
            terminal.draw(|f| {
                let area = f.area();
                let p = ratatui::widgets::Paragraph::new(format!(
                    "New intent (Inbox)\n\ntitle: {text}▏\n\nEnter = save, Esc = cancel"
                ));
                p.render(area, f.buffer_mut());
            })?;
            if let Event::Key(k) = crossterm::event::read()? {
                match k.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter => return Ok(Some(text.clone())),
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) => text.push(c),
                    _ => {}
                }
            }
        })();
        restore_terminal();
        if let Some(t) = result? {
            let t = t.trim();
            if !t.is_empty() {
                let body = template(
                    &Dirs {
                        config: cfg_dir_for(&cfg),
                        root: cfg.root.clone(),
                    },
                    "idea",
                    false,
                )
                .unwrap_or_default();
                store.create(Path::new(crate::store::INBOX), t, "idea", &body, repo)?;
            }
        }
        return Ok(());
    }
    let mut input = String::new();
    if title.is_none() || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        use std::io::Read;
        let _ = std::io::stdin().read_to_string(&mut input);
    }
    let (t, body) = match title {
        Some(t) => (t, input),
        None => {
            let mut lines = input.lines();
            let t = lines
                .next()
                .context("no title given and stdin is empty")?
                .to_string();
            (t, lines.collect::<Vec<_>>().join("\n"))
        }
    };
    let it = store.create(
        Path::new(crate::store::INBOX),
        t.trim(),
        "idea",
        &body,
        repo,
    )?;
    println!("{}", it.path.display());
    Ok(())
}

fn cfg_dir_for(_cfg: &Config) -> PathBuf {
    // `add` has no Dirs in scope for templates when called standalone; the
    // caller passes dirs through `run`; here we only need the kind skeleton,
    // and an empty body is acceptable if the template cannot be read.
    PathBuf::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::{Terminal, layout::Rect};

    use crate::config::{Dirs, Output, ProviderSpec, When, load_or_init};
    use crate::store::{State, Store};

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn fake_spec() -> ProviderSpec {
        ProviderSpec {
            name: "fake".into(),
            polish: vec![
                fixtures().join("fake-provider.sh").display().to_string(),
                "--prompt={prompt}".into(),
            ],
            resume: vec![
                fixtures().join("fake-provider.sh").display().to_string(),
                "--resume={conv}".into(),
                "--prompt={prompt}".into(),
            ],
            output: Output::Jsonl {
                type_key: "event".into(),
                text_path: "/step_update/text_delta".into(),
                text_types: vec!["step_update".into()],
                text_when: Some(When {
                    path: "/step_update/step_type".into(),
                    value: "agent_response".into(),
                }),
                id_paths: vec![
                    "/conversation_id".into(),
                    "/step_update/conversation_id".into(),
                ],
                done_type: Some("result".into()),
            },
            send_kind: "claude".into(),
        }
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        app: App,
        log: PathBuf,
    }

    /// Two groups, three intents: Inbox/"In the inbox" (issue, 2 open questions),
    /// skoolscout/"Alpha" (idea), skoolscout/"Beta" (enhancement).
    fn setup(events: &str) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let mut cfg = load_or_init(&dirs).unwrap();
        cfg.providers.insert("fake".into(), fake_spec());
        cfg.polish_provider = "fake".into();
        let store = Store::open(&dirs.root).unwrap();
        store
            .create(
                Path::new("Inbox"),
                "In the inbox",
                "issue",
                "## Observed\nx\n\n## Open questions\n- a?\n- b?\n",
                None,
            )
            .unwrap();
        store.create_group(Path::new(""), "skoolscout").unwrap();
        store
            .create(
                Path::new("skoolscout"),
                "Alpha",
                "idea",
                "alpha body\n",
                None,
            )
            .unwrap();
        store
            .create(
                Path::new("skoolscout"),
                "Beta",
                "enhancement",
                "beta body\n",
                None,
            )
            .unwrap();
        let log = tmp.path().join("provider.log");
        let mut app = App::new(cfg, dirs).unwrap();
        app.provider_env = vec![
            (
                "FAKE_EVENTS".into(),
                fixtures().join("events").join(events).display().to_string(),
            ),
            ("FAKE_PROVIDER_LOG".into(), log.display().to_string()),
        ];
        Fx {
            _tmp: tmp,
            app,
            log,
        }
    }

    fn key(app: &mut App, code: KeyCode) -> Flow {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn ctrl(app: &mut App, c: char) -> Flow {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }
    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            key(app, KeyCode::Char(c));
        }
    }
    fn wait_until(app: &mut App, what: &str, f: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while !f(app) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out: {what}; view = {:?}",
                app.view()
            );
            app.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// Move the browse cursor to the row whose label contains `needle`.
    fn select(app: &mut App, needle: &str) {
        for _ in 0..20 {
            if app.view().rows[app.view().selected].label.contains(needle) {
                return;
            }
            key(app, KeyCode::Char('j'));
        }
        panic!("no row containing {needle}: {:?}", app.view().rows);
    }

    #[test]
    fn browse_tree_snapshot() {
        let fx = setup("agy.jsonl");
        let v = fx.app.view();
        let labels: Vec<String> = v
            .rows
            .iter()
            .map(|r| {
                format!(
                    "{}{}{}",
                    "  ".repeat(r.depth),
                    r.label,
                    if r.questions > 0 {
                        format!(" ?{}", r.questions)
                    } else {
                        String::new()
                    }
                )
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Inbox",
                "  In the inbox ?2",
                "skoolscout",
                "  Alpha",
                "  Beta"
            ]
        );
        let mut t = Terminal::new(TestBackend::new(90, 16)).unwrap();
        t.draw(|f| fx.app.draw(Rect::new(0, 0, 90, 16), f.buffer_mut()))
            .unwrap();
        insta::assert_snapshot!(t.backend());
    }

    #[test]
    fn add_creates_raw_intent_in_selected_group() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        type_str(&mut fx.app, "Gamma thing");
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert!(
            v.rows
                .iter()
                .any(|r| r.label == "Gamma thing" && r.depth == 1),
            "{:?}",
            v.rows
        );
        let path = fx.app.selected_intent().unwrap().path.clone();
        assert!(path.starts_with(fx.app.store.root().join("skoolscout")));
        assert_eq!(Store::load(&path).unwrap().fm.state, State::Raw);
        assert_eq!(
            fx.app.selected_intent().unwrap().fm.kind,
            "idea",
            "default kind"
        );
    }

    #[test]
    fn edit_save_roundtrip_through_editor() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "edit");
        key(&mut fx.app, KeyCode::End);
        type_str(&mut fx.app, " more");
        ctrl(&mut fx.app, 's');
        let path = fx.app.selected_intent().unwrap().path.clone();
        assert_eq!(Store::load(&path).unwrap().body, "alpha body more\n");
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
    }

    #[test]
    fn chat_turn_streams_into_transcript_and_enables_apply_only_with_proposal() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('p'));
        assert_eq!(fx.app.view().screen, "chat");
        wait_until(&mut fx.app, "turn finished", |a| {
            !a.view().turn_running && a.view().transcript_len >= 2
        });
        let v = fx.app.view();
        assert!(v.can_apply, "{v:?}");
        assert!(
            v.transcript
                .last()
                .unwrap()
                .1
                .contains("Here is a tighter version.")
        );
        // transcript file written
        let chat = fx.app.selected_intent().unwrap().transcript_path();
        assert!(fs::read_to_string(&chat).unwrap().contains("### agent"));

        // a reply with no proposal block: apply disabled with a hint
        let mut fx2 = setup("agy.jsonl");
        let noblock = fx2._tmp.path().join("noblock.jsonl");
        fs::write(&noblock, r#"{"event":"init","conversation_id":"c1"}
{"event":"step_update","step_update":{"conversation_id":"c1","step_type":"agent_response","text_delta":"Just thinking out loud, no block."}}
{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS"}}
"#).unwrap();
        fx2.app.provider_env[0].1 = noblock.display().to_string();
        select(&mut fx2.app, "Alpha");
        key(&mut fx2.app, KeyCode::Char('p'));
        wait_until(&mut fx2.app, "turn finished", |a| {
            !a.view().turn_running && a.view().transcript_len >= 2
        });
        let v = fx2.app.view();
        assert!(!v.can_apply);
        assert!(
            v.hint.as_deref().unwrap_or("").contains("no proposal"),
            "{v:?}"
        );
        key(&mut fx2.app, KeyCode::Char('a'));
        assert_ne!(fx2.app.view().screen, "diff");
    }

    #[test]
    fn apply_replaces_whole_body_after_diff_and_records_questions_and_polished_at() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('p'));
        wait_until(&mut fx.app, "turn finished", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        key(&mut fx.app, KeyCode::Char('a'));
        assert_eq!(fx.app.view().screen, "diff");
        key(&mut fx.app, KeyCode::Enter);
        let it = Store::load(&fx.app.selected_intent().unwrap().path).unwrap();
        assert!(
            it.body.starts_with("## Observed\nParents who skip"),
            "{}",
            it.body
        );
        assert!(
            it.body
                .contains("## Open questions\n- Which org?\n- Since when?"),
            "{}",
            it.body
        );
        assert!(it.fm.polished_at.is_some());
        assert_eq!(it.fm.state, State::Refining);
        assert_eq!(
            it.fm.conversations.get("fake").map(String::as_str),
            Some("dde502fd-bcf1-4bfb-a275-4e6de5ad9a7d")
        );
    }

    #[test]
    fn conversation_id_persists_and_resume_argv_is_used_on_second_turn() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('p'));
        wait_until(&mut fx.app, "first turn", |a| {
            !a.view().turn_running && a.view().transcript_len >= 2
        });
        type_str(&mut fx.app, "shorter please");
        key(&mut fx.app, KeyCode::Enter);
        wait_until(&mut fx.app, "second turn", |a| {
            !a.view().turn_running && a.view().transcript_len >= 4
        });
        let log = fs::read_to_string(&fx.log).unwrap();
        // the fake logs `$*`, and prompts span lines: count invocations, not lines
        assert_eq!(log.matches("--prompt=").count(), 2, "{log}");
        assert_eq!(
            log.matches("--resume=").count(),
            1,
            "second turn resumes, first does not: {log}"
        );
        assert!(
            log.contains("--resume=dde502fd-bcf1-4bfb-a275-4e6de5ad9a7d"),
            "{log}"
        );
        let first = log.find("--prompt=").unwrap();
        let resume = log.find("--resume=").unwrap();
        assert!(
            resume > first,
            "resume belongs to the second invocation: {log}"
        );
    }

    #[test]
    fn esc_cancels_running_turn() {
        let mut fx = setup("agy.jsonl");
        fx.app
            .provider_env
            .push(("FAKE_MODE".into(), "hang".into()));
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('p'));
        wait_until(&mut fx.app, "deltas", |a| {
            a.view()
                .transcript
                .last()
                .map(|(_, t)| t.contains("tighter"))
                .unwrap_or(false)
        });
        assert!(fx.app.view().turn_running);
        key(&mut fx.app, KeyCode::Esc);
        wait_until(&mut fx.app, "cancelled", |a| !a.view().turn_running);
        assert_eq!(
            fx.app.view().screen,
            "chat",
            "first Esc cancels, stays in chat"
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
    }

    #[test]
    fn save_conflict_prompts_instead_of_clobbering() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Enter);
        // someone else saves meanwhile
        let path = fx.app.selected_intent().unwrap().path.clone();
        std::thread::sleep(Duration::from_millis(1100));
        let mut other = Store::load(&path).unwrap();
        other.body = "other\n".into();
        Store::save_force(&mut other).unwrap();
        type_str(&mut fx.app, "X");
        ctrl(&mut fx.app, 's');
        assert_eq!(fx.app.view().screen, "prompt", "{:?}", fx.app.view());
        assert_eq!(
            Store::load(&path).unwrap().body,
            "other\n",
            "not clobbered yet"
        );
        key(&mut fx.app, KeyCode::Char('y'));
        assert!(Store::load(&path).unwrap().body.contains('X'));
    }

    #[test]
    fn ready_and_refining_toggle_state_and_kind_cycles() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('R'));
        assert_eq!(fx.app.selected_intent().unwrap().fm.state, State::Ready);
        key(&mut fx.app, KeyCode::Char('U'));
        assert_eq!(fx.app.selected_intent().unwrap().fm.state, State::Refining);
        key(&mut fx.app, KeyCode::Char('K'));
        assert_eq!(fx.app.selected_intent().unwrap().fm.kind, "issue");
        let path = fx.app.selected_intent().unwrap().path.clone();
        assert_eq!(Store::load(&path).unwrap().fm.kind, "issue", "persisted");
    }

    #[test]
    fn send_marks_intent_sent_with_record_only_on_success() {
        let mut fx = setup("agy.jsonl");
        let log = fx._tmp.path().join("herdr.log");
        fx.app.herdr_env = vec![
            (
                "HERDR_BIN_PATH".into(),
                fixtures().join("fake-herdr.sh").display().to_string(),
            ),
            ("FAKE_HERDR_LOG".into(), log.display().to_string()),
        ];
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('s'));
        assert_eq!(fx.app.view().screen, "send-pick");
        key(&mut fx.app, KeyCode::Enter); // first applicable handoff for an idea: grill
        assert_eq!(fx.app.view().screen, "send-confirm");
        key(&mut fx.app, KeyCode::Enter);
        let it = Store::load(&fx.app.selected_intent().unwrap().path).unwrap();
        assert_eq!(it.fm.state, State::Sent);
        let sent = it.fm.sent_to.unwrap();
        assert_eq!(sent.handoff, "grill");
        assert_eq!(sent.workspace_id, "w9");
        assert!(fs::read_to_string(&log).unwrap().contains("agent prompt"));

        // a failing herdr call leaves the intent as it was
        let mut fx2 = setup("agy.jsonl");
        fx2.app.herdr_env = vec![
            (
                "HERDR_BIN_PATH".into(),
                fixtures().join("fake-herdr.sh").display().to_string(),
            ),
            (
                "FAKE_HERDR_LOG".into(),
                fx2._tmp.path().join("h.log").display().to_string(),
            ),
            ("FAKE_AGENT_START_FAILS".into(), "1".into()),
        ];
        select(&mut fx2.app, "Beta");
        key(&mut fx2.app, KeyCode::Char('R'));
        key(&mut fx2.app, KeyCode::Char('s'));
        key(&mut fx2.app, KeyCode::Enter);
        key(&mut fx2.app, KeyCode::Enter);
        let it = Store::load(&fx2.app.selected_intent().unwrap().path).unwrap();
        assert_eq!(it.fm.state, State::Ready, "unchanged on failure");
        assert!(it.fm.sent_to.is_none());
        assert!(
            fx2.app.view().message.unwrap().contains("agent start"),
            "{:?}",
            fx2.app.view().message
        );
    }

    #[test]
    fn send_without_herdr_shows_message_and_keeps_state() {
        let mut fx = setup("agy.jsonl");
        fx.app.herdr_env = vec![("HERDR_BIN_PATH".into(), String::new())];
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('s'));
        key(&mut fx.app, KeyCode::Enter);
        key(&mut fx.app, KeyCode::Enter);
        let it = Store::load(&fx.app.selected_intent().unwrap().path).unwrap();
        assert_eq!(it.fm.state, State::Raw);
        assert!(
            fx.app
                .view()
                .message
                .unwrap()
                .to_lowercase()
                .contains("herdr"),
            "{:?}",
            fx.app.view().message
        );
    }
}
