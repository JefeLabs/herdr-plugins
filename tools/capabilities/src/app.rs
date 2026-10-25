//! App state machine and run loop.
//!
//! `App` owns the tree and the current screen. Every field-only change goes
//! through `Store::update_fields` (reload, change, save) so a stale in-memory
//! row never clobbers another writer's body. The chat is about one intent path,
//! records the change stamp it opened with, and apply prompts if that moved.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::config::{
    Config, Dirs, Handoff, KINDS, LastUsed, last_used, polish_cwd, save_last_used, template,
};
use crate::conversation::{Turn, TurnMsg, follow_up_prompt, seed_prompt};
use crate::editor::TextArea;
use crate::provider::Reply;
use crate::store::{Group, Intent, SaveError, State, Store};
use crate::ui::{self, Pane};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
    /// Leave the TUI, run `$EDITOR` on the intent in the editor, come back and reload.
    ExternalEdit,
}

#[allow(clippy::large_enum_variant)] // one Intent per row is the point of the row
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
    Broken {
        path: PathBuf,
        error: String,
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
    /// The intent this chat is about (never "whatever row is selected now").
    pub intent_path: PathBuf,
    /// Change stamp of the intent when the chat opened; apply prompts if it moved.
    pub opened_stamp: chrono::DateTime<chrono::Local>,
    /// The seed prompt, re-sent with the transcript when the provider cannot resume.
    pub seed: String,
    /// Go back to the editor (buffer intact) rather than browse on Esc.
    pub from_editor: bool,
    /// Wrapped lines scrolled back from the tail of the transcript.
    pub scroll: u16,
    /// Model chosen in the picker, if the provider offers any.
    pub model: Option<String>,
}

/// What the agent picker is choosing right now.
pub enum PickStep {
    Provider,
    Model { provider: String },
}

/// The intent and context a chat will open on once the picker is done.
pub struct PendingChat {
    pub intent: Intent,
    pub selection: Option<String>,
    pub from_editor: bool,
}

pub enum Pending {
    OverwriteSave,
    DiscardEdits,
    OverwriteApply { body: String, title: Option<String> },
}

pub enum InputAction {
    AddIntent {
        group: PathBuf,
    },
    NewGroup {
        parent: PathBuf,
    },
    Attach,
    MoveTo,
    MoveGroup {
        group: PathBuf,
    },
    /// Set the capability's title (the file name is kept).
    Rename,
    /// Append a `- [ ]` item under `heading`, then return focus to that pane.
    AddItem {
        heading: &'static str,
        pane: Pane,
    },
    /// Append an open question.
    AddQuestion,
    /// Append `- kind: target` under Artifacts (input is "kind: target").
    AddArtifact,
}

#[allow(clippy::large_enum_variant)] // Chat carries the live turn; boxing buys nothing here
pub enum Screen {
    Browse,
    Edit,
    /// Choose provider, then model (if any), before a chat opens.
    AgentPick {
        step: PickStep,
        items: Vec<String>,
        selected: usize,
        pending: PendingChat,
    },
    /// Typing an answer for question `index` inline in the questions pane.
    Answer {
        index: usize,
        text: String,
    },
    /// Read-only Markdown viewer for an artifact file.
    Reader {
        title: String,
        lines: Vec<ratatui::text::Line<'static>>,
        scroll: u16,
    },
    Chat(ChatState),
    Diff {
        proposal: String,
        questions: Vec<String>,
        title: Option<String>,
        scroll: u16,
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

/// What the browser shows: the group tree with detail panes, or one flat list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Tree,
    List,
}

impl Layout {
    pub fn label(self) -> &'static str {
        match self {
            Layout::Tree => "tree",
            Layout::List => "list",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "tree" => Some(Layout::Tree),
            "list" => Some(Layout::List),
            _ => None,
        }
    }
}

/// The list's sort orders, cycled with `S`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Most recently changed first.
    Updated,
    /// Least advanced first: raw, ?N, ready, sent by bar fill, complete.
    Progress,
    /// Newest created first.
    Captured,
    /// A to Z.
    Title,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            SortOrder::Updated => SortOrder::Progress,
            SortOrder::Progress => SortOrder::Captured,
            SortOrder::Captured => SortOrder::Title,
            SortOrder::Title => SortOrder::Updated,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SortOrder::Updated => "Updated",
            SortOrder::Progress => "Progress",
            SortOrder::Captured => "Captured",
            SortOrder::Title => "Title",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        [
            SortOrder::Updated,
            SortOrder::Progress,
            SortOrder::Captured,
            SortOrder::Title,
        ]
        .into_iter()
        .find(|o| o.label() == s)
    }
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
    pub chat_input: String,
    pub chat_scroll: u16,
    pub diff_scroll: u16,
    pub input_text: String,
    pub send_prompt: Option<String>,
    pub pick_items: Vec<String>,
    pub pick_selected: usize,
    pub focus: &'static str,
    pub question_count: usize,
    pub question_selected: usize,
    pub criteria_selected: usize,
    pub answer_text: String,
    pub artifact_count: usize,
    /// Artifacts found in the repo but not yet linked.
    pub found_count: usize,
    pub reader_title: String,
    /// `tree` or `list`.
    pub layout: &'static str,
    /// Every capability's title in list order.
    pub list_titles: Vec<String>,
    pub list_selected: usize,
    /// The list's sort order label.
    pub sort: &'static str,
}

pub struct App {
    pub store: Store,
    pub cfg: Config,
    pub dirs: Dirs,
    pub tree: Group,
    rows: Vec<Row>,
    pub selected: usize,
    /// Groups seen at least once; a sub-group (depth >= 1) starts collapsed the
    /// first time it appears, so a deep tree discloses itself one level at a time.
    seen_groups: HashSet<PathBuf>,
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
    /// Chat state kept while the Diff or a Prompt screen is showing.
    parked_chat: Option<ChatState>,
    pub focus: Pane,
    description_scroll: u16,
    /// mtime of our own binary at startup; a newer one on disk means a rebuild
    /// is waiting for a restart of the slot.
    pub binary_stamp: Option<std::time::SystemTime>,
    newer_build: bool,
    ticks: u32,
    preconditions_selected: usize,
    criteria_selected: usize,
    question_selected: usize,
    artifacts_selected: usize,
    /// Artifacts found in the selected capability's repo, cached per path.
    found_cache: Option<(PathBuf, Vec<crate::store::Artifact>)>,
    pub layout: Layout,
    pub sort: SortOrder,
    /// Every capability in the tree, in list order (all groups, collapsed or not).
    list: Vec<Intent>,
    list_selected: usize,
    /// Plan tasks per capability path, as last reported by the poller.
    plan_evidence: HashMap<PathBuf, Vec<crate::progress::PlanTasks>>,
    /// Pull request states per `(repo, pr: target as linked)`, as last
    /// reported by `gh`.
    pr_states: HashMap<(PathBuf, String), crate::progress::PrState>,
    /// Latest `herdr agent list` (`name → agent_status`); `None` until known
    /// or when herdr cannot answer.
    agents: Option<HashMap<String, crate::progress::AgentInfo>>,
    /// herdr was asked and could not answer: the agent field stays blank
    /// rather than looking like it is still loading.
    agents_unavailable: bool,
    /// Background runner for herdr, gh and plan reads; made on first use so
    /// it picks up `herdr_env`.
    poller: Option<crate::progress::Poller>,
    /// Ticks since the list started polling; 0 means "poll everything now".
    poll_ticks: u32,
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
            seen_groups: HashSet::new(),
            collapsed: HashSet::new(),
            screen: Screen::Browse,
            editor: TextArea::default(),
            editing: None,
            message: None,
            provider_env: Vec::new(),
            herdr_env: Vec::new(),
            parked_chat: None,
            focus: Pane::Tree,
            description_scroll: 0,
            binary_stamp: binary_mtime(),
            newer_build: false,
            ticks: 0,
            preconditions_selected: 0,
            criteria_selected: 0,
            question_selected: 0,
            artifacts_selected: 0,
            found_cache: None,
            layout: Layout::Tree,
            sort: SortOrder::Updated,
            list: Vec::new(),
            list_selected: 0,
            plan_evidence: HashMap::new(),
            pr_states: HashMap::new(),
            agents: None,
            agents_unavailable: false,
            poller: None,
            poll_ticks: 0,
        };
        let last = crate::config::last_used(&app.dirs, &app.cfg);
        if let Some(l) = last.layout.as_deref().and_then(Layout::parse) {
            app.layout = l;
        }
        if let Some(o) = last.sort.as_deref().and_then(SortOrder::parse) {
            app.sort = o;
        }
        app.rebuild_rows();
        Ok(app)
    }

    // ---- tree -------------------------------------------------------------

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        fn count_intents(g: &Group) -> usize {
            g.intents.len() + g.groups.iter().map(count_intents).sum::<usize>()
        }
        fn walk(
            g: &Group,
            depth: usize,
            collapsed: &mut HashSet<PathBuf>,
            seen: &mut HashSet<PathBuf>,
            rows: &mut Vec<Row>,
        ) {
            for sub in &g.groups {
                // a sub-group starts collapsed the first time it is seen
                if seen.insert(sub.path.clone()) && depth >= 1 {
                    collapsed.insert(sub.path.clone());
                }
                rows.push(Row::Group {
                    path: sub.path.clone(),
                    name: sub.name.clone(),
                    depth,
                    count: count_intents(sub),
                });
                if !collapsed.contains(&sub.path) {
                    walk(sub, depth + 1, collapsed, seen, rows);
                }
            }
            for it in &g.intents {
                rows.push(Row::Intent {
                    intent: it.clone(),
                    depth,
                });
            }
            for (path, error) in &g.broken {
                rows.push(Row::Broken {
                    path: path.clone(),
                    error: error.clone(),
                    depth,
                });
            }
        }
        walk(
            &self.tree,
            0,
            &mut self.collapsed,
            &mut self.seen_groups,
            &mut rows,
        );
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.rebuild_list();
    }

    // ---- list -------------------------------------------------------------

    /// Every capability in the tree, sorted by `self.sort`, keeping the list
    /// selection on the same capability when it still exists.
    fn rebuild_list(&mut self) {
        let keep = self.list.get(self.list_selected).map(|i| i.path.clone());
        fn collect(g: &Group, out: &mut Vec<Intent>) {
            out.extend(g.intents.iter().cloned());
            for sub in &g.groups {
                collect(sub, out);
            }
        }
        let mut list = Vec::new();
        collect(&self.tree, &mut list);
        match self.sort {
            SortOrder::Updated => list.sort_by_key(|i| std::cmp::Reverse(crate::store::stamp(i))),
            SortOrder::Captured => list.sort_by_key(|i| std::cmp::Reverse(i.fm.created)),
            SortOrder::Title => list.sort_by_key(|i| i.fm.title.to_lowercase()),
            SortOrder::Progress => {
                let key = |i: &Intent| {
                    use crate::store::Readiness;
                    let prog = crate::progress::progress(i, &self.evidence_for(i));
                    let (rank, open, fill) = match crate::store::readiness(i) {
                        Readiness::Raw => (0, 0, 0.0),
                        Readiness::Open(n) => (1, n, 0.0),
                        Readiness::Ready => (2, 0, 0.0),
                        Readiness::Sent => match prog {
                            Some(p) if p.complete => (4, 0, 10.0),
                            Some(p) => (3, 0, p.fill),
                            None => (3, 0, 0.0),
                        },
                    };
                    (rank, open, fill, crate::store::stamp(i))
                };
                list.sort_by(|a, b| {
                    let (ra, oa, fa, sa) = key(a);
                    let (rb, ob, fb, sb) = key(b);
                    ra.cmp(&rb)
                        .then(ob.cmp(&oa)) // more open questions first
                        .then(fa.total_cmp(&fb))
                        .then(sb.cmp(&sa)) // then most recently changed
                });
            }
        }
        self.list = list;
        self.list_selected = keep
            .and_then(|p| self.list.iter().position(|i| i.path == p))
            .unwrap_or(self.list_selected)
            .min(self.list.len().saturating_sub(1));
    }

    /// Switch layout, carrying the selected capability across.
    fn set_layout(&mut self, layout: Layout) {
        match layout {
            Layout::List => {
                // the tree's capability, or the first one under a selected group
                let target = match self.rows.get(self.selected) {
                    Some(Row::Intent { intent, .. }) => Some(intent.path.clone()),
                    Some(Row::Group { path, .. }) => self
                        .list
                        .iter()
                        .filter(|i| i.path.starts_with(path))
                        .map(|i| i.path.clone())
                        .next(),
                    _ => None,
                };
                self.layout = Layout::List;
                self.poll_ticks = 0;
                if let Some(i) = target.and_then(|p| self.list.iter().position(|i| i.path == p)) {
                    self.list_selected = i;
                }
            }
            Layout::Tree => {
                let target = self.list.get(self.list_selected).map(|i| i.path.clone());
                self.layout = Layout::Tree;
                if let Some(p) = target {
                    self.reveal(&p);
                }
            }
        }
        let _ = crate::config::save_view(&self.dirs, self.layout.label(), self.sort.label());
    }

    /// Expand every group above `path` in the tree and select it.
    fn reveal(&mut self, path: &Path) {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if !d.starts_with(self.store.root()) {
                break;
            }
            self.collapsed.remove(d);
            dir = d.parent();
        }
        self.rebuild_rows();
        self.select_path(path);
    }

    /// Plans and PR states gathered so far for `it`; a PR not yet answered by
    /// `gh` reads as `Unknown` (open).
    fn evidence_for(&self, it: &Intent) -> crate::progress::Evidence {
        use crate::progress::{PrState, pr_number};
        let prs = crate::store::artifacts(&it.body)
            .into_iter()
            .filter(|a| a.kind.eq_ignore_ascii_case("pr"))
            .filter_map(|a| {
                let n = pr_number(&a.target)?;
                let state = it
                    .fm
                    .repo
                    .as_ref()
                    .and_then(|r| {
                        self.pr_states
                            .get(&(r.clone(), a.target.trim().to_string()))
                    })
                    .copied()
                    .unwrap_or(PrState::Unknown);
                Some((n, state))
            })
            .collect();
        crate::progress::Evidence {
            plans: self
                .plan_evidence
                .get(&it.path)
                .cloned()
                .unwrap_or_default(),
            prs,
        }
    }

    fn poller(&mut self) -> &mut crate::progress::Poller {
        let env = self.herdr_env.clone();
        self.poller
            .get_or_insert_with(|| crate::progress::Poller::new(env))
    }

    /// Background requests started and not yet reported.
    pub fn polls_in_flight(&self) -> usize {
        self.poller.as_ref().map_or(0, |p| p.in_flight())
    }

    /// Ask for agent states and, when `all`, every sent capability's plans and
    /// PR states.
    fn request_polls(&mut self, all: bool) {
        self.poller().request_agents();
        if !all {
            return;
        }
        let sent: Vec<Intent> = self
            .list
            .iter()
            .filter(|i| i.fm.state == State::Sent)
            .cloned()
            .collect();
        for it in sent {
            let prs: Vec<String> = crate::store::artifacts(&it.body)
                .into_iter()
                .filter(|a| a.kind.eq_ignore_ascii_case("pr"))
                .filter(|a| crate::progress::pr_number(&a.target).is_some())
                .map(|a| a.target.trim().to_string())
                .collect();
            if let Some(repo) = it.fm.repo.clone() {
                for target in prs {
                    self.poller().request_pr(repo.clone(), target);
                }
            }
            self.poller().request_plans(it);
        }
    }

    /// Fold finished background requests into the list's evidence.
    fn drain_polls(&mut self) {
        let Some(p) = self.poller.as_mut() else {
            return;
        };
        let updates = p.drain();
        if updates.is_empty() {
            return;
        }
        for u in updates {
            match u {
                crate::progress::Update::Agents(a) => {
                    self.agents_unavailable = a.is_none();
                    self.agents = a;
                }
                crate::progress::Update::Pr {
                    repo,
                    target,
                    state,
                } => {
                    self.pr_states.insert((repo, target), state);
                }
                crate::progress::Update::Plans { path, plans } => {
                    self.plan_evidence.insert(path, plans);
                }
            }
        }
        if self.sort == SortOrder::Progress {
            self.rebuild_list();
        }
    }

    /// `s`: refuse with the reason, or open the hand-off picker.
    fn start_send(&mut self) {
        // the picker is drawn beside the tree: point the tree at this capability
        if self.layout == Layout::List
            && let Some(p) = self.selected_intent().map(|i| i.path.clone())
        {
            self.reveal(&p);
        }
        if let Some(refusal) = self.selected_intent().and_then(crate::store::send_refusal) {
            self.message = Some(refusal);
        } else if let Some(it) = self.selected_intent() {
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

    /// `g`: focus the agent the selected capability was sent to.
    fn go_to_agent(&mut self) {
        let Some(agent) = self
            .selected_intent()
            .and_then(|i| i.fm.sent_to.as_ref())
            .map(|s| s.agent.clone())
        else {
            self.message = Some("not sent yet".into());
            return;
        };
        let env = self.herdr_env.clone();
        let Some(bin) = crate::progress::tool_bin(&env, "HERDR_BIN_PATH", None) else {
            self.message = Some("Herdr is required: HERDR_BIN_PATH is not set".into());
            return;
        };
        self.message = Some(format!("focusing {agent}"));
        std::thread::spawn(move || {
            let _ = std::process::Command::new(bin)
                .args(["agent", "focus", &agent])
                .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }

    fn key_list(&mut self, k: KeyEvent) -> Flow {
        let last = self.list.len().saturating_sub(1);
        match k.code {
            KeyCode::Char('q') => return Flow::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                self.list_selected = (self.list_selected + 1).min(last)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.list_selected = self.list_selected.saturating_sub(1)
            }
            KeyCode::PageDown => self.list_selected = (self.list_selected + 10).min(last),
            KeyCode::PageUp => self.list_selected = self.list_selected.saturating_sub(10),
            KeyCode::Char('v') => self.set_layout(Layout::Tree),
            KeyCode::Enter => self.set_layout(Layout::Tree),
            KeyCode::Char('S') => {
                self.sort = self.sort.next();
                self.rebuild_list();
                let _ =
                    crate::config::save_view(&self.dirs, self.layout.label(), self.sort.label());
            }
            KeyCode::Char('r') => self.rescan(),
            KeyCode::Char('s') => self.start_send(),
            KeyCode::Char('g') => self.go_to_agent(),
            _ => {}
        }
        Flow::Continue
    }

    pub fn rescan(&mut self) {
        self.found_cache = None;
        self.reload_tree();
        self.refresh_found();
        if self.layout == Layout::List {
            self.poll_ticks = 0;
        }
    }

    /// Re-read every capability from disk, keeping both selections.
    fn reload_tree(&mut self) {
        let keep = match self.layout {
            Layout::Tree => self.selected_path(),
            Layout::List => self.list.get(self.list_selected).map(|i| i.path.clone()),
        };
        match self.store.scan() {
            Ok(t) => self.tree = t,
            Err(e) => self.message = Some(format!("rescan failed: {e:#}")),
        }
        self.rebuild_rows();
        if let Some(p) = keep {
            self.select_path(&p);
            if let Some(i) = self.list.iter().position(|i| i.path == p) {
                self.list_selected = i;
            }
        }
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.rows.get(self.selected).map(row_path)
    }

    /// The capability under the cursor: the list's in the list layout, the
    /// tree's otherwise.
    pub fn selected_intent(&self) -> Option<&Intent> {
        if self.layout == Layout::List {
            return self.list.get(self.list_selected);
        }
        match self.rows.get(self.selected) {
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
            Some(Row::Broken { path, .. }) => path
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

    fn reset_pane_cursors(&mut self) {
        self.description_scroll = 0;
        self.preconditions_selected = 0;
        self.criteria_selected = 0;
        self.question_selected = 0;
        self.artifacts_selected = 0;
    }

    fn select_path(&mut self, p: &Path) {
        if let Some(i) = self.rows.iter().position(|r| row_path(r) == p) {
            self.selected = i;
        }
    }

    /// Fill the found-artifacts cache for the selected capability (scans its
    /// repo's `docs/superpowers/{specs,plans}` and `docs/adr`). Cached per path,
    /// dropped on rescan, so a key press costs nothing once computed.
    fn refresh_found(&mut self) {
        let Some(it) = self.selected_intent().cloned() else {
            self.found_cache = None;
            return;
        };
        if matches!(&self.found_cache, Some((p, _)) if *p == it.path) {
            return;
        }
        let found = match &it.fm.repo {
            Some(repo) if repo.is_dir() => {
                let already: Vec<String> = crate::store::artifacts(&it.body)
                    .into_iter()
                    .map(|a| a.target)
                    .collect();
                crate::store::discover_artifacts(repo, &it.fm.title, &already)
            }
            _ => Vec::new(),
        };
        self.found_cache = Some((it.path, found));
    }

    /// The cached found artifacts for the selected capability.
    fn found_artifacts(&self) -> &[crate::store::Artifact] {
        match (&self.found_cache, self.selected_intent()) {
            (Some((p, found)), Some(it)) if *p == it.path => found,
            _ => &[],
        }
    }

    /// A field-only change: reload from disk, change, save, rescan. Never
    /// writes the cached row's body (C2).
    fn update_selected_fields(&mut self, f: impl FnOnce(&mut crate::store::FrontMatter)) {
        let Some(path) = self.selected_intent().map(|i| i.path.clone()) else {
            return;
        };
        match Store::update_fields(&path, f) {
            Ok(_) => self.rescan(),
            Err(e) => self.message = Some(format!("save failed: {e:#}")),
        }
    }

    // ---- keys -------------------------------------------------------------

    pub fn key(&mut self, k: KeyEvent) -> Flow {
        let flow = self.key_inner(k);
        self.refresh_found();
        flow
    }

    fn key_inner(&mut self, k: KeyEvent) -> Flow {
        if k.kind == KeyEventKind::Release {
            return Flow::Continue;
        }
        self.message = if self.newer_build {
            Some("a newer build is installed: Ctrl-] x in the toolbar restarts this slot".into())
        } else {
            None
        };
        match std::mem::replace(&mut self.screen, Screen::Browse) {
            Screen::Browse => self.key_browse(k),
            Screen::Edit => {
                self.screen = Screen::Edit;
                self.key_edit(k)
            }
            Screen::AgentPick {
                step,
                items,
                selected,
                pending,
            } => self.key_agent_pick(step, items, selected, pending, k),
            Screen::Answer { index, text } => self.key_answer(index, text, k),
            Screen::Reader {
                title,
                lines,
                scroll,
            } => self.key_reader(title, lines, scroll, k),
            Screen::Chat(st) => self.key_chat(st, k),
            Screen::Diff {
                proposal,
                questions,
                title,
                scroll,
            } => self.key_diff(proposal, questions, title, scroll, k),
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
        if self.layout == Layout::List {
            return self.key_list(k);
        }
        if k.code == KeyCode::Char('v') {
            self.set_layout(Layout::List);
            return Flow::Continue;
        }
        // pane focus: Tab / Shift-Tab cycle, Esc returns to the tree
        match k.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Pane::Tree => Pane::Description,
                    Pane::Description => Pane::Preconditions,
                    Pane::Preconditions => Pane::Criteria,
                    Pane::Criteria => Pane::Questions,
                    Pane::Questions => Pane::Artifacts,
                    Pane::Artifacts => Pane::Tree,
                };
                return Flow::Continue;
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Pane::Tree => Pane::Artifacts,
                    Pane::Artifacts => Pane::Questions,
                    Pane::Description => Pane::Tree,
                    Pane::Preconditions => Pane::Description,
                    Pane::Criteria => Pane::Preconditions,
                    Pane::Questions => Pane::Criteria,
                };
                return Flow::Continue;
            }
            KeyCode::Char('Q') => {
                if self.selected_intent().is_some() {
                    self.focus = Pane::Questions;
                    self.question_selected = 0;
                }
                return Flow::Continue;
            }
            KeyCode::Char('V') => {
                if self.selected_intent().is_some() {
                    self.focus = Pane::Artifacts;
                    self.artifacts_selected = 0;
                }
                return Flow::Continue;
            }
            KeyCode::Char('C') => {
                if self.selected_intent().is_some() {
                    // C goes to the criteria; C again flips to the preconditions, and back
                    self.focus = if self.focus == Pane::Criteria {
                        Pane::Preconditions
                    } else {
                        Pane::Criteria
                    };
                    self.criteria_selected = 0;
                    self.preconditions_selected = 0;
                }
                return Flow::Continue;
            }
            _ => {}
        }
        if self.focus != Pane::Tree {
            return self.key_detail_pane(k);
        }
        match k.code {
            KeyCode::Char('q') => return Flow::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                    self.reset_pane_cursors();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.reset_pane_cursors();
            }
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
            KeyCode::Enter | KeyCode::Char('e') => match self.rows.get(self.selected) {
                Some(Row::Intent { intent, .. }) => {
                    let it = intent.clone();
                    self.editor = TextArea::from_text(&it.body);
                    self.editing = Some(it);
                    self.screen = Screen::Edit;
                }
                Some(Row::Broken { error, .. }) => {
                    self.message = Some(format!("cannot load: {error}"))
                }
                _ => {}
            },
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
            KeyCode::Char('m') => match self.rows.get(self.selected) {
                Some(Row::Intent { .. }) => {
                    self.screen = Screen::Input {
                        label: "move to group".into(),
                        text: String::new(),
                        action: InputAction::MoveTo,
                    }
                }
                Some(Row::Group { path, .. }) => {
                    let group = path.clone();
                    self.screen = Screen::Input {
                        label: "move group under".into(),
                        text: String::new(),
                        action: InputAction::MoveGroup { group },
                    };
                }
                _ => {}
            },
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
                    match Store::attachments(it).into_iter().next() {
                        Some((name, _)) => {
                            let path = it.assets_dir().join(name);
                            let opener = if cfg!(target_os = "macos") {
                                "open"
                            } else {
                                "xdg-open"
                            };
                            if let Err(e) = std::process::Command::new(opener)
                                .arg(&path)
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .spawn()
                            {
                                self.message = Some(format!("cannot open attachment: {e}"));
                            }
                        }
                        None => self.message = Some("no attachments".into()),
                    }
                }
            }
            KeyCode::Char('K') => self.update_selected_fields(|fm| {
                let i = KINDS.iter().position(|k| *k == fm.kind).unwrap_or(0);
                fm.kind = KINDS[(i + 1) % KINDS.len()].to_string();
            }),
            KeyCode::Char('R') => {
                // ready means nothing is left unanswered
                let open = self
                    .selected_intent()
                    .map(|it| it.open_questions())
                    .unwrap_or(0);
                if open > 0 {
                    self.message = Some(format!(
                        "{open} open question{} still unanswered: answer or remove {} first (Q)",
                        if open == 1 { "" } else { "s" },
                        if open == 1 { "it" } else { "them" }
                    ));
                } else {
                    self.update_selected_fields(|fm| fm.state = State::Ready);
                }
            }
            KeyCode::Char('U') => self.update_selected_fields(|fm| fm.state = State::Refining),
            KeyCode::Char('p') => {
                if let Some(it) = self.selected_intent().cloned() {
                    self.open_agent_pick(PendingChat {
                        intent: it,
                        selection: None,
                        from_editor: false,
                    });
                }
            }
            KeyCode::Char('s') => self.start_send(),
            KeyCode::Char('t') => {
                if let Some(it) = self.selected_intent() {
                    self.screen = Screen::Input {
                        label: "title (a capability phrase: who can what)".into(),
                        text: it.fm.title.clone(),
                        action: InputAction::Rename,
                    };
                }
            }
            KeyCode::Char('r') => self.rescan(),
            _ => {}
        }
        Flow::Continue
    }

    /// Keys while one of the three detail panes has focus.
    fn key_detail_pane(&mut self, k: KeyEvent) -> Flow {
        let Some(it) = self.selected_intent().cloned() else {
            self.focus = Pane::Tree;
            return Flow::Continue;
        };
        if k.code == KeyCode::Char('a') {
            let (label, action) = match self.focus {
                Pane::Preconditions => (
                    "new precondition (what must already be true)",
                    InputAction::AddItem {
                        heading: crate::store::PRECONDITIONS_HEADING,
                        pane: Pane::Preconditions,
                    },
                ),
                Pane::Criteria => (
                    "new acceptance criterion (a check a reviewer could verify)",
                    InputAction::AddItem {
                        heading: crate::store::CRITERIA_HEADING,
                        pane: Pane::Criteria,
                    },
                ),
                Pane::Questions => ("new open question", InputAction::AddQuestion),
                Pane::Artifacts => (
                    "new artifact as kind: path-or-url (spec, plan, adr, decision, issue)",
                    InputAction::AddArtifact,
                ),
                _ => return Flow::Continue,
            };
            self.screen = Screen::Input {
                label: label.into(),
                text: String::new(),
                action,
            };
            return Flow::Continue;
        }
        match self.focus {
            Pane::Description => match k.code {
                KeyCode::Esc => self.focus = Pane::Tree,
                KeyCode::Char('j') | KeyCode::Down | KeyCode::PageDown => {
                    self.description_scroll = self
                        .description_scroll
                        .saturating_add(if k.code == KeyCode::PageDown { 5 } else { 1 })
                }
                KeyCode::Char('k') | KeyCode::Up | KeyCode::PageUp => {
                    self.description_scroll = self
                        .description_scroll
                        .saturating_sub(if k.code == KeyCode::PageUp { 5 } else { 1 })
                }
                KeyCode::Enter | KeyCode::Char('e') => {
                    self.editor = TextArea::from_text(&it.body);
                    self.editing = Some(it);
                    self.screen = Screen::Edit;
                }
                _ => {}
            },
            Pane::Preconditions => {
                let n = crate::store::preconditions(&it.body).len();
                match k.code {
                    KeyCode::Esc => self.focus = Pane::Tree,
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.preconditions_selected =
                            (self.preconditions_selected + 1).min(n.saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.preconditions_selected = self.preconditions_selected.saturating_sub(1)
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => self.toggle_selected_checklist(
                        crate::store::PRECONDITIONS_HEADING,
                        self.preconditions_selected,
                    ),
                    _ => {}
                }
            }
            Pane::Criteria => {
                let n = crate::store::criteria(&it.body).len();
                match k.code {
                    KeyCode::Esc => self.focus = Pane::Tree,
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.criteria_selected =
                            (self.criteria_selected + 1).min(n.saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.criteria_selected = self.criteria_selected.saturating_sub(1)
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => self.toggle_selected_checklist(
                        crate::store::CRITERIA_HEADING,
                        self.criteria_selected,
                    ),
                    _ => {}
                }
            }
            Pane::Questions => {
                let qs = crate::store::questions(&it.body);
                match k.code {
                    KeyCode::Esc => self.focus = Pane::Tree,
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.question_selected =
                            (self.question_selected + 1).min(qs.len().saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.question_selected = self.question_selected.saturating_sub(1)
                    }
                    KeyCode::Enter => {
                        if let Some(q) = qs.get(self.question_selected) {
                            self.screen = Screen::Answer {
                                index: self.question_selected,
                                text: q.answer.clone().unwrap_or_default(),
                            };
                        }
                    }
                    _ => {}
                }
            }
            Pane::Artifacts => {
                let linked = crate::store::artifacts(&it.body);
                let found = self.found_artifacts().to_vec();
                let total = linked.len() + found.len();
                // (artifact, is_found) at a row of the combined list
                let pick = |i: usize| {
                    if i < linked.len() {
                        linked.get(i).cloned().map(|a| (a, false))
                    } else {
                        found.get(i - linked.len()).cloned().map(|a| (a, true))
                    }
                };
                match k.code {
                    KeyCode::Esc => self.focus = Pane::Tree,
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.artifacts_selected =
                            (self.artifacts_selected + 1).min(total.saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.artifacts_selected = self.artifacts_selected.saturating_sub(1)
                    }
                    KeyCode::Enter | KeyCode::Char('o') => {
                        if let Some((a, _)) = pick(self.artifacts_selected) {
                            self.open_artifact(&it, &a, k.code == KeyCode::Char('o'));
                        }
                    }
                    KeyCode::Char('L') => match pick(self.artifacts_selected) {
                        Some((a, true)) => {
                            self.edit_selected_body(|body| {
                                crate::store::add_artifact(body, &a.kind, &a.target)
                            });
                            self.found_cache = None;
                            self.artifacts_selected = linked.len();
                            self.message = Some(format!("linked {}", a.target));
                        }
                        Some((_, false)) => {
                            self.message = Some("already linked".into());
                        }
                        None => {}
                    },
                    _ => {}
                }
            }
            Pane::Tree => {}
        }
        Flow::Continue
    }

    /// Enter reads a Markdown file in the tool; tickets, URLs and `o` open externally.
    fn open_artifact(&mut self, it: &Intent, a: &crate::store::Artifact, external: bool) {
        let target = a.target.trim();
        let is_ref = target.starts_with('#')
            || target.starts_with("http://")
            || target.starts_with("https://");
        if is_ref || external {
            if let Some(n) = target.strip_prefix('#') {
                let repo = it
                    .fm
                    .repo
                    .clone()
                    .filter(|r| r.is_dir())
                    .unwrap_or_else(std::env::temp_dir);
                let _ = std::process::Command::new("gh")
                    .args(["issue", "view", n, "--web"])
                    .current_dir(repo)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                self.message = Some(format!("opened #{n} in the browser"));
                return;
            }
            let arg = if is_ref {
                target.to_string()
            } else {
                self.resolve_artifact_path(it, target).display().to_string()
            };
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            match std::process::Command::new(opener)
                .arg(&arg)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => self.message = Some(format!("opened {arg}")),
                Err(e) => self.message = Some(format!("cannot open {arg}: {e}")),
            }
            return;
        }
        let path = self.resolve_artifact_path(it, target);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let lines = crate::markdown::render_markdown(&text);
                let title = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| target.to_string());
                self.screen = Screen::Reader {
                    title,
                    lines,
                    scroll: 0,
                };
            }
            Err(e) => self.message = Some(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// Relative targets resolve against the capability's repo, else its own folder.
    fn resolve_artifact_path(&self, it: &Intent, target: &str) -> PathBuf {
        if let Some(found) = it
            .fm
            .repo
            .as_deref()
            .and_then(|repo| crate::send::resolve_in_repo(repo, target))
        {
            return found;
        }
        let p = PathBuf::from(shellexpand::tilde(target).into_owned());
        if p.is_absolute() {
            return p;
        }
        match &it.fm.repo {
            Some(repo) if repo.is_dir() => repo.join(&p),
            _ => it.path.parent().map(|d| d.join(&p)).unwrap_or(p),
        }
    }

    fn key_reader(
        &mut self,
        title: String,
        lines: Vec<ratatui::text::Line<'static>>,
        mut scroll: u16,
        k: KeyEvent,
    ) -> Flow {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.screen = Screen::Browse;
                self.focus = Pane::Artifacts;
                return Flow::Continue;
            }
            KeyCode::Char('j') | KeyCode::Down => scroll = scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => scroll = scroll.saturating_add(10),
            KeyCode::PageUp => scroll = scroll.saturating_sub(10),
            _ => {}
        }
        self.screen = Screen::Reader {
            title,
            lines,
            scroll,
        };
        Flow::Continue
    }

    /// Reload the selected capability, transform its body, save (conflict-checked), rescan.
    fn edit_selected_body(&mut self, f: impl FnOnce(&str) -> String) {
        let Some(path) = self.selected_intent().map(|i| i.path.clone()) else {
            return;
        };
        match Store::load(&path) {
            Ok(mut it) => {
                it.body = f(&it.body);
                if let Err(e) = Store::save(&mut it) {
                    self.message = Some(format!("save failed: {e}"));
                }
                self.rescan();
            }
            Err(e) => self.message = Some(format!("{e:#}")),
        }
    }

    fn toggle_selected_checklist(&mut self, heading: &str, index: usize) {
        let Some(path) = self.selected_intent().map(|i| i.path.clone()) else {
            return;
        };
        match Store::load(&path) {
            Ok(mut it) => {
                if let Some(body) = crate::store::toggle_checklist(&it.body, heading, index) {
                    it.body = body;
                    if let Err(e) = Store::save(&mut it) {
                        self.message = Some(format!("save failed: {e}"));
                    }
                }
                self.rescan();
            }
            Err(e) => self.message = Some(format!("{e:#}")),
        }
    }

    /// Inline answer box under the selected question.
    fn key_answer(&mut self, index: usize, mut text: String, k: KeyEvent) -> Flow {
        match k.code {
            KeyCode::Esc => self.screen = Screen::Browse,
            KeyCode::Backspace => {
                text.pop();
                self.screen = Screen::Answer { index, text };
            }
            KeyCode::Enter => {
                self.screen = Screen::Browse;
                let value = text.trim().to_string();
                if value.is_empty() {
                    return Flow::Continue;
                }
                if let Some(path) = self.selected_intent().map(|i| i.path.clone()) {
                    match Store::load(&path) {
                        Ok(mut it) => {
                            match crate::store::answer_question(&it.body, index, &value) {
                                Some(body) => {
                                    it.body = body;
                                    match Store::save(&mut it) {
                                        Ok(()) => {
                                            self.message = Some("answer recorded".into());
                                            self.rescan();
                                        }
                                        Err(e) => self.message = Some(format!("save failed: {e}")),
                                    }
                                }
                                None => self.message = Some("that question is gone".into()),
                            }
                        }
                        Err(e) => self.message = Some(format!("{e:#}")),
                    }
                }
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                text.push(c);
                self.screen = Screen::Answer { index, text };
            }
            _ => self.screen = Screen::Answer { index, text },
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
                    self.open_agent_pick(PendingChat {
                        intent: it,
                        selection: sel,
                        from_editor: true,
                    });
                }
            }
            (KeyCode::Char('e'), true) => {
                // hand the file to $EDITOR only once the buffer is on disk
                self.save_editor(false);
                if matches!(self.screen, Screen::Edit) {
                    return Flow::ExternalEdit;
                }
            }
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

    /// Step one of the picker: providers, with the last used one preselected.
    fn open_agent_pick(&mut self, pending: PendingChat) {
        let items: Vec<String> = self.cfg.providers.keys().cloned().collect();
        let last = last_used(&self.dirs, &self.cfg);
        let selected = items.iter().position(|p| *p == last.provider).unwrap_or(0);
        self.screen = Screen::AgentPick {
            step: PickStep::Provider,
            items,
            selected,
            pending,
        };
    }

    fn key_agent_pick(
        &mut self,
        step: PickStep,
        items: Vec<String>,
        mut selected: usize,
        pending: PendingChat,
        k: KeyEvent,
    ) -> Flow {
        match k.code {
            KeyCode::Esc => {
                self.screen = if pending.from_editor && self.editing.is_some() {
                    Screen::Edit
                } else {
                    Screen::Browse
                };
            }
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(items.len().saturating_sub(1));
                self.screen = Screen::AgentPick {
                    step,
                    items,
                    selected,
                    pending,
                };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.screen = Screen::AgentPick {
                    step,
                    items,
                    selected,
                    pending,
                };
            }
            KeyCode::Enter => {
                let Some(choice) = items.get(selected).cloned() else {
                    self.screen = Screen::Browse;
                    return Flow::Continue;
                };
                match step {
                    PickStep::Provider => {
                        let models = self
                            .cfg
                            .providers
                            .get(&choice)
                            .map(|p| p.models.clone())
                            .unwrap_or_default();
                        if models.is_empty() {
                            self.open_chat(pending, &choice, None);
                        } else {
                            let last = last_used(&self.dirs, &self.cfg);
                            let msel = if last.provider == choice {
                                last.model
                                    .and_then(|m| models.iter().position(|x| *x == m))
                                    .unwrap_or(0)
                            } else {
                                0
                            };
                            self.screen = Screen::AgentPick {
                                step: PickStep::Model { provider: choice },
                                items: models,
                                selected: msel,
                                pending,
                            };
                        }
                    }
                    PickStep::Model { provider } => {
                        self.open_chat(pending, &provider, Some(choice))
                    }
                }
            }
            _ => {
                self.screen = Screen::AgentPick {
                    step,
                    items,
                    selected,
                    pending,
                }
            }
        }
        Flow::Continue
    }

    fn open_chat(&mut self, pending: PendingChat, provider: &str, model: Option<String>) {
        let PendingChat {
            intent,
            selection,
            from_editor,
        } = pending;
        let _ = save_last_used(
            &self.dirs,
            &LastUsed {
                provider: provider.to_string(),
                model: model.clone(),
                layout: Some(self.layout.label().to_string()),
                sort: Some(self.sort.label().to_string()),
            },
        );
        let conv_id = intent.fm.conversations.get(provider).cloned();
        let tpl = template(&self.dirs, &intent.fm.kind, true)
            .unwrap_or_else(|_| "Refine this intent.".to_string());
        let seed = seed_prompt(&tpl, &intent.body, selection.as_deref(), &intent.fm.kind);
        let mut st = ChatState {
            turn: None,
            transcript: Vec::new(),
            latest: None,
            input: String::new(),
            provider: provider.to_string(),
            conv_id,
            hint: None,
            intent_path: intent.path.clone(),
            opened_stamp: intent.loaded_updated,
            seed: seed.clone(),
            from_editor,
            scroll: 0,
            model,
        };
        self.select_path(&intent.path);
        let shown = match selection.as_deref() {
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
        self.start_turn(&mut st, &intent, &seed, &shown);
        self.screen = Screen::Chat(st);
    }

    fn start_turn(&mut self, st: &mut ChatState, intent: &Intent, prompt: &str, shown: &str) {
        let Some(mut spec) = self.cfg.providers.get(&st.provider).cloned() else {
            st.hint = Some(format!("unknown provider `{}`", st.provider));
            return;
        };
        if let Some(m) = &st.model {
            spec.extra.extend([spec.model_flag.clone(), m.clone()]);
        }
        // neutral per-intent directory: no repository in the provider's reach
        let cwd = polish_cwd(self.store.root(), &intent.stem());
        if let Err(e) = std::fs::create_dir_all(&cwd) {
            st.hint = Some(format!("cannot create {}: {e}", cwd.display()));
            return;
        }
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
                st.scroll = 0;
                st.turn = Some(turn);
                let _ = Store::append_transcript(intent, "user", shown);
            }
            Err(e) => st.hint = Some(format!("{e:#}")),
        }
    }

    fn leave_chat(&mut self, st: ChatState) {
        self.screen = if st.from_editor && self.editing.is_some() {
            Screen::Edit
        } else {
            Screen::Browse
        };
        self.rescan();
    }

    fn key_chat(&mut self, mut st: ChatState, k: KeyEvent) -> Flow {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match (k.code, ctrl) {
            (KeyCode::Esc, _) => {
                if let Some(mut t) = st.turn.take() {
                    t.cancel();
                    if let Some(last) = st.transcript.last_mut() {
                        last.1.push_str("\n[cancelled]");
                    }
                    self.screen = Screen::Chat(st);
                } else {
                    self.leave_chat(st);
                }
            }
            (KeyCode::Enter, _) if st.turn.is_none() => {
                let text = st.input.trim().to_string();
                if !text.is_empty()
                    && let Ok(it) = Store::load(&st.intent_path)
                {
                    st.input.clear();
                    let resumable = st.conv_id.is_some()
                        && self
                            .cfg
                            .providers
                            .get(&st.provider)
                            .map(|p| !p.resume.is_empty())
                            .unwrap_or(false);
                    let prompt = follow_up_prompt(&st.seed, &st.transcript, &text, resumable);
                    self.start_turn(&mut st, &it, &prompt, &text);
                }
                self.screen = Screen::Chat(st);
            }
            // apply and dismiss live on Ctrl so a message starting with "a" or "d" is just typed (C1)
            (KeyCode::Char('a'), true) if st.turn.is_none() => {
                match st.latest.as_ref().and_then(|r| r.proposal.clone()) {
                    Some(proposal) => {
                        let questions = st
                            .latest
                            .as_ref()
                            .map(|r| r.questions.clone())
                            .unwrap_or_default();
                        let title = st.latest.as_ref().and_then(|r| r.title.clone());
                        self.parked_chat = Some(st);
                        self.screen = Screen::Diff {
                            proposal,
                            questions,
                            title,
                            scroll: 0,
                        };
                    }
                    None => {
                        st.hint = Some("no proposal to apply in the latest reply".into());
                        self.screen = Screen::Chat(st);
                    }
                }
            }
            (KeyCode::Char('d'), true) if st.turn.is_none() => {
                st.latest = None;
                st.hint = Some("proposal dismissed".into());
                self.screen = Screen::Chat(st);
            }
            (KeyCode::PageUp, _) => {
                st.scroll = st.scroll.saturating_add(5);
                self.screen = Screen::Chat(st);
            }
            (KeyCode::PageDown, _) => {
                st.scroll = st.scroll.saturating_sub(5);
                self.screen = Screen::Chat(st);
            }
            (KeyCode::Backspace, _) => {
                st.input.pop();
                self.screen = Screen::Chat(st);
            }
            (KeyCode::Char(c), false) => {
                st.input.push(c);
                self.screen = Screen::Chat(st);
            }
            _ => self.screen = Screen::Chat(st),
        }
        Flow::Continue
    }

    fn key_diff(
        &mut self,
        proposal: String,
        questions: Vec<String>,
        title: Option<String>,
        scroll: u16,
        k: KeyEvent,
    ) -> Flow {
        match k.code {
            KeyCode::Enter => self.apply_proposal(
                &merge_questions(&proposal, &questions),
                title.as_deref(),
                false,
            ),
            KeyCode::Esc => {
                self.screen = match self.parked_chat.take() {
                    Some(st) => Screen::Chat(st),
                    None => Screen::Browse,
                };
            }
            KeyCode::PageDown | KeyCode::Char('j') | KeyCode::Down => {
                self.screen = Screen::Diff {
                    proposal,
                    questions,
                    title,
                    scroll: scroll.saturating_add(5),
                }
            }
            KeyCode::PageUp | KeyCode::Char('k') | KeyCode::Up => {
                self.screen = Screen::Diff {
                    proposal,
                    questions,
                    title,
                    scroll: scroll.saturating_sub(5),
                }
            }
            // any other key does nothing: applying needs an explicit Enter
            _ => {
                self.screen = Screen::Diff {
                    proposal,
                    questions,
                    title,
                    scroll,
                }
            }
        }
        Flow::Continue
    }

    /// Write `body` into the chat's intent. Unless forced, refuse when the file
    /// changed since the chat opened (another toolbar, another editor) and ask.
    fn apply_proposal(&mut self, body: &str, new_title: Option<&str>, force: bool) {
        let Some(st) = self.parked_chat.as_ref() else {
            self.screen = Screen::Browse;
            return;
        };
        let path = st.intent_path.clone();
        let opened = st.opened_stamp;
        let provider = st.provider.clone();
        let conv_id = st.conv_id.clone();
        let mut it = match Store::load(&path) {
            Ok(it) => it,
            Err(e) => {
                self.message = Some(format!("apply failed: {e:#}"));
                self.screen = Screen::Browse;
                return;
            }
        };
        if !force && it.loaded_updated.timestamp() != opened.timestamp() {
            self.screen = Screen::Prompt { question: "File changed on disk since this chat opened. Overwrite with the proposal? (y/n)".into(), pending: Pending::OverwriteApply { body: body.to_string(), title: new_title.map(str::to_string) } };
            return;
        }
        it.body = body.to_string();
        if let Some(t) = new_title.map(str::trim).filter(|t| !t.is_empty()) {
            it.fm.title = t.to_string();
        }
        it.fm.polished_at = Some(chrono::Local::now());
        if it.fm.state == State::Raw {
            it.fm.state = State::Refining;
        }
        if let Some(id) = conv_id {
            it.fm.conversations.insert(provider, id);
        }
        match Store::save_force(&mut it) {
            Ok(()) => {
                self.message = Some("proposal applied".into());
                self.rescan();
                // Apply ends the refinement for now: back to the view, or to the
                // editor with the new body if polish started there. The
                // conversation id is stored, so `p` resumes it later.
                let from_editor = self
                    .parked_chat
                    .take()
                    .map(|st| st.from_editor)
                    .unwrap_or(false);
                if from_editor && self.editing.is_some() {
                    self.editor = TextArea::from_text(&it.body);
                    self.editing = Some(it);
                    self.screen = Screen::Edit;
                } else {
                    self.focus = Pane::Tree;
                    self.screen = Screen::Browse;
                }
            }
            Err(e) => {
                self.message = Some(format!("apply failed: {e:#}"));
                self.screen = match self.parked_chat.take() {
                    Some(st) => Screen::Chat(st),
                    None => Screen::Browse,
                };
            }
        }
    }

    // ---- send ----------------------------------------------------------------

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
                // render from disk, never from the cached row (C2)
                let fresh = self
                    .selected_intent()
                    .and_then(|i| Store::load(&i.path).ok());
                match fresh {
                    Some(it) => match crate::send::prepare(
                        &self.cfg,
                        &handoff,
                        &it,
                        &Store::attachments(&it),
                    ) {
                        Ok((repo, prompt)) => {
                            self.screen = Screen::SendConfirm {
                                handoff,
                                provider,
                                repo,
                                prompt,
                                scroll: 0,
                            }
                        }
                        Err(e) => {
                            self.message = Some(e);
                            self.screen = Screen::Browse;
                        }
                    },
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
            KeyCode::Char('j') | KeyCode::Down | KeyCode::PageDown => {
                scroll = scroll.saturating_add(1);
                self.screen = Screen::SendConfirm {
                    handoff,
                    provider,
                    repo,
                    prompt,
                    scroll,
                };
            }
            KeyCode::Char('k') | KeyCode::Up | KeyCode::PageUp => {
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
        let Some(path) = self.selected_intent().map(|i| i.path.clone()) else {
            return;
        };
        let Ok(it) = Store::load(&path) else { return };
        let kind = self
            .cfg
            .providers
            .get(provider)
            .map(|p| p.send_kind.clone())
            .unwrap_or_else(|| provider.to_string());
        match crate::send::send_intent(&self.herdr_env, repo, &it, &kind, prompt) {
            Ok(outcome) => {
                let record = crate::store::SentTo {
                    handoff: handoff.name.clone(),
                    workspace_id: outcome.workspace_id.clone(),
                    agent: outcome.agent.clone(),
                    at: chrono::Local::now(),
                };
                match Store::update_fields(&path, |fm| {
                    fm.state = State::Sent;
                    fm.sent_to = Some(record);
                }) {
                    Ok(_) => {
                        self.message = Some(format!(
                            "sent to {} in workspace {} (agent {})",
                            handoff.name, outcome.workspace_id, outcome.agent
                        ))
                    }
                    Err(e) => {
                        self.message = Some(format!(
                            "sent to {} in workspace {}, but recording it failed: {e:#}",
                            handoff.name, outcome.workspace_id
                        ))
                    }
                }
                self.rescan();
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
                Pending::OverwriteApply { body, title } => {
                    self.apply_proposal(&body, title.as_deref(), true)
                }
            },
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = match pending {
                    Pending::OverwriteSave | Pending::DiscardEdits => Screen::Edit,
                    Pending::OverwriteApply { .. } => match self.parked_chat.take() {
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
                self.run_input_action(action, &value);
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

    fn run_input_action(&mut self, action: InputAction, value: &str) {
        match action {
            InputAction::AddIntent { group } => {
                let rel = self.rel(&group);
                let kind = "idea";
                let body = template(&self.dirs, kind, false).unwrap_or_default();
                match self
                    .store
                    .create(&rel, value, kind, &body, crate::send::capture_repo())
                {
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
                match self.store.create_group(&rel, value) {
                    Ok(p) => {
                        self.rescan();
                        self.select_path(&p);
                    }
                    Err(e) => self.message = Some(format!("group failed: {e:#}")),
                }
            }
            InputAction::Attach => {
                if let Some(it) = self.selected_intent().cloned() {
                    let src = PathBuf::from(shellexpand::tilde(value).into_owned());
                    match Store::attach(&it, &src) {
                        Ok(_) => self.message = Some("attached".into()),
                        Err(e) => self.message = Some(format!("attach failed: {e:#}")),
                    }
                }
            }
            InputAction::MoveTo => {
                if let Some(it) = self.selected_intent().cloned() {
                    match self.store.move_intent(&it, Path::new(value)) {
                        Ok(moved) => {
                            self.rescan();
                            self.select_path(&moved.path);
                        }
                        Err(e) => self.message = Some(format!("move failed: {e:#}")),
                    }
                }
            }
            InputAction::Rename => {
                let v = value.to_string();
                self.update_selected_fields(|fm| fm.title = v);
            }
            InputAction::AddItem { heading, pane } => {
                self.edit_selected_body(|body| {
                    crate::store::add_checklist_item(body, heading, value)
                });
                self.focus = pane;
                let n = self
                    .selected_intent()
                    .map(|it| crate::store::checklist(&it.body, heading).len())
                    .unwrap_or(1);
                if pane == Pane::Preconditions {
                    self.preconditions_selected = n.saturating_sub(1);
                } else {
                    self.criteria_selected = n.saturating_sub(1);
                }
            }
            InputAction::AddQuestion => {
                self.edit_selected_body(|body| crate::store::add_question(body, value));
                self.focus = Pane::Questions;
                self.question_selected = self
                    .selected_intent()
                    .map(|it| crate::store::questions(&it.body).len())
                    .unwrap_or(1)
                    .saturating_sub(1);
            }
            InputAction::AddArtifact => {
                let (kind, target) = match value.split_once(':') {
                    Some((k, t))
                        if !k.trim().contains(' ')
                            && !t.trim().is_empty()
                            && !matches!(k.trim(), "http" | "https") =>
                    {
                        (k.trim().to_string(), t.trim().to_string())
                    }
                    _ => ("link".to_string(), value.to_string()),
                };
                self.edit_selected_body(|body| crate::store::add_artifact(body, &kind, &target));
                self.focus = Pane::Artifacts;
                self.artifacts_selected = self
                    .selected_intent()
                    .map(|it| crate::store::artifacts(&it.body).len())
                    .unwrap_or(1)
                    .saturating_sub(1);
            }
            InputAction::MoveGroup { group } => {
                let rel = self.rel(&group);
                match self.store.move_group(&rel, Path::new(value)) {
                    Ok(p) => {
                        self.rescan();
                        self.select_path(&p);
                    }
                    Err(e) => self.message = Some(format!("move failed: {e:#}")),
                }
            }
        }
    }

    /// Pasted text goes to whichever text input is active; CR line endings are normalised.
    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        match &mut self.screen {
            Screen::Edit => {
                for ch in text.chars() {
                    let code = if ch == '\n' {
                        KeyCode::Enter
                    } else {
                        KeyCode::Char(ch)
                    };
                    self.editor
                        .handle(KeyEvent::new(code, KeyModifiers::NONE), 20);
                }
            }
            Screen::Chat(st) => st.input.push_str(text.trim_end_matches('\n')),
            Screen::Input { text: t, .. } => t.push_str(text.trim_end_matches('\n')),
            Screen::Answer { text: t, .. } => t.push_str(text.trim_end_matches('\n')),
            _ => {}
        }
    }

    // ---- ticking ------------------------------------------------------------

    /// Compare the binary's current mtime with the one we started with.
    pub fn check_for_newer_build(&mut self, now: Option<std::time::SystemTime>) {
        if let (Some(start), Some(now)) = (self.binary_stamp, now)
            && now > start
            && !self.newer_build
        {
            self.newer_build = true;
            self.message = Some(
                "a newer build is installed: Ctrl-] x in the toolbar restarts this slot".into(),
            );
        }
    }

    /// Drain messages from a running turn.
    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(150) {
            // about every 5 s at the loop's 33 ms cadence
            self.check_for_newer_build(binary_mtime());
        }
        // list evidence: agents every ~3 s, plans and PRs on entry, on r and
        // every ~60 s; all on background threads
        self.drain_polls();
        if self.layout == Layout::List {
            if self.poll_ticks.is_multiple_of(1800) {
                // pick up lines agents appended to capability files
                if self.poll_ticks > 0 {
                    self.reload_tree();
                }
                self.request_polls(true);
            } else if self.poll_ticks.is_multiple_of(90) {
                self.request_polls(false);
            }
            self.poll_ticks = self.poll_ticks.wrapping_add(1);
        }
        let Screen::Chat(st) = &mut self.screen else {
            return;
        };
        let Some(turn) = st.turn.as_ref() else { return };
        let path = st.intent_path.clone();
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
                    // bookkeeping only: reload, record, no `updated` bump (C2)
                    let provider = st.provider.clone();
                    let _ = Store::update_meta(&path, |fm| {
                        fm.conversations.insert(provider, id);
                    });
                }
                TurnMsg::Finished(reply) => {
                    if let Ok(it) = Store::load(&path)
                        && let Some(last) = st.transcript.last()
                    {
                        let _ = Store::append_transcript(&it, "agent", &last.1);
                    }
                    st.hint = if reply.proposal.is_some() {
                        Some("Ctrl-a = apply, Ctrl-d = dismiss".into())
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
                Row::Broken { path, depth, .. } => RowView {
                    depth: *depth,
                    label: format!(
                        "! {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ),
                    is_group: false,
                    questions: 0,
                },
            })
            .collect();
        let mut v = View {
            rows,
            selected: self.selected,
            screen: "browse",
            can_apply: false,
            hint: None,
            message: self.message.clone(),
            turn_running: false,
            transcript: Vec::new(),
            transcript_len: 0,
            chat_input: String::new(),
            chat_scroll: 0,
            diff_scroll: 0,
            input_text: String::new(),
            send_prompt: None,
            pick_items: Vec::new(),
            pick_selected: 0,
            focus: match self.focus {
                Pane::Tree => "tree",
                Pane::Description => "description",
                Pane::Preconditions => "preconditions",
                Pane::Criteria => "criteria",
                Pane::Questions => "questions",
                Pane::Artifacts => "artifacts",
            },
            question_count: self
                .selected_intent()
                .map(|i| crate::store::questions(&i.body).len())
                .unwrap_or(0),
            question_selected: self.question_selected,
            criteria_selected: self.criteria_selected,
            answer_text: String::new(),
            artifact_count: self
                .selected_intent()
                .map(|i| crate::store::artifacts(&i.body).len())
                .unwrap_or(0),
            found_count: self.found_artifacts().len(),
            reader_title: String::new(),
            layout: self.layout.label(),
            list_titles: self.list.iter().map(|i| i.fm.title.clone()).collect(),
            list_selected: self.list_selected,
            sort: self.sort.label(),
        };
        match &self.screen {
            Screen::Browse => {}
            Screen::Edit => v.screen = "edit",
            Screen::AgentPick {
                items, selected, ..
            } => {
                v.screen = "agent-pick";
                v.pick_items = items.clone();
                v.pick_selected = *selected;
            }
            Screen::Answer { text, .. } => {
                v.screen = "answer";
                v.answer_text = text.clone();
            }
            Screen::Reader { title, .. } => {
                v.screen = "reader";
                v.reader_title = title.clone();
            }
            Screen::Chat(st) => {
                v.screen = "chat";
                v.can_apply = st
                    .latest
                    .as_ref()
                    .map(|r| r.proposal.is_some())
                    .unwrap_or(false);
                v.hint = st.hint.clone();
                v.turn_running = st.turn.is_some();
                v.transcript = st.transcript.clone();
                v.transcript_len = st.transcript.len();
                v.chat_input = st.input.clone();
                v.chat_scroll = st.scroll;
            }
            Screen::Diff { scroll, .. } => {
                v.screen = "diff";
                v.can_apply = true;
                v.diff_scroll = *scroll;
            }
            Screen::SendPick { .. } => v.screen = "send-pick",
            Screen::SendConfirm { prompt, .. } => {
                v.screen = "send-confirm";
                v.send_prompt = Some(prompt.clone());
            }
            Screen::Prompt { question, .. } => {
                v.screen = "prompt";
                v.hint = Some(question.clone());
            }
            Screen::Input { label, text, .. } => {
                v.screen = "input";
                v.hint = Some(label.clone());
                v.input_text = text.clone();
            }
        }
        v
    }

    /// The list layout over the whole area, status line at the bottom.
    fn draw_list(&self, area: Rect, buf: &mut Buffer) {
        let list_area = Rect {
            height: area.height.saturating_sub(1),
            ..area
        };
        let status = Rect {
            y: area.y + area.height.saturating_sub(1),
            height: 1,
            ..area
        };
        struct Owned {
            description: String,
            agent: Option<String>,
            location: String,
        }
        let owned: Vec<Owned> = self
            .list
            .iter()
            .map(|i| {
                // every non-empty line of the description; the card wraps it
                let description = ui::description_text(&i.body)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect::<Vec<_>>()
                    .join("\n");
                // the sent agent's live Herdr state; `…` until herdr answers
                let agent = match (&i.fm.sent_to, i.fm.state) {
                    (Some(sent), State::Sent) => Some(match &self.agents {
                        Some(agents) => match agents.get(&sent.agent).map(|a| a.status.as_str()) {
                            Some("blocked") => "needs you".to_string(),
                            Some(st) => st.to_string(),
                            None => "closed".to_string(),
                        },
                        None if self.agents_unavailable => "—".to_string(),
                        None => "…".to_string(),
                    }),
                    _ => None,
                };
                // group path, then the sent agent's worktree and slot
                let mut location = vec![
                    i.path
                        .parent()
                        .and_then(|d| d.strip_prefix(self.store.root()).ok())
                        .map(|d| d.display().to_string())
                        .unwrap_or_default(),
                ];
                if i.fm.state == State::Sent
                    && let (Some(sent), Some(agents)) = (&i.fm.sent_to, &self.agents)
                    && let Some(info) = agents.get(&sent.agent)
                {
                    if let Some(w) = &info.worktree {
                        location.push(format!("worktree {w}"));
                    }
                    if let Some(n) = info.slot {
                        location.push(format!("slot {n}"));
                    }
                }
                Owned {
                    description,
                    agent,
                    location: location.join(" │ "),
                }
            })
            .collect();
        let rows: Vec<ui::ListRow> = self
            .list
            .iter()
            .zip(&owned)
            .map(|(i, o)| {
                let when = match self.sort {
                    SortOrder::Captured => i.fm.created,
                    _ => crate::store::stamp(i),
                };
                ui::ListRow {
                    status: crate::store::readiness(i).label(),
                    progress: crate::progress::progress(i, &self.evidence_for(i))
                        .map(|p| (p.fill, p.label, p.complete)),
                    agent: o.agent.clone(),
                    title: &i.fm.title,
                    location: o.location.clone(),
                    when: when.format("%b %d %H:%M").to_string(),
                    description: &o.description,
                }
            })
            .collect();
        let header = format!(
            "Capabilities · {} · sorted by {}",
            self.list.len(),
            self.sort.label()
        );
        ui::render_capability_list(&rows, self.list_selected, &header, list_area, buf);
        let text = self.message.clone().unwrap_or_else(|| {
            "j/k move  Enter open in tree  v tree  S sort  s send  g agent  r rescan  q quit"
                .to_string()
        });
        ui::render_status(&text, status, buf);
    }

    pub fn draw(&self, area: Rect, buf: &mut Buffer) {
        if matches!(self.screen, Screen::Browse) && self.layout == Layout::List {
            self.draw_list(area, buf);
            return;
        }
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
                    broken: false,
                },
                Row::Intent { intent, depth } => ui::TreeRow {
                    depth: *depth,
                    label: &intent.fm.title,
                    is_group: false,
                    collapsed: false,
                    questions: intent.open_questions(),
                    state: Some(intent.fm.state),
                    count: 0,
                    broken: false,
                },
                Row::Broken { path, depth, .. } => ui::TreeRow {
                    depth: *depth,
                    label: path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    is_group: false,
                    collapsed: false,
                    questions: 0,
                    state: None,
                    count: 0,
                    broken: true,
                },
            })
            .collect();
        // scroll the tree just enough to keep the selection on screen
        let visible = left.height as usize;
        let tree_scroll = if visible > 0 && self.selected >= visible {
            self.selected + 1 - visible
        } else {
            0
        };
        ui::render_tree(&tree_rows, self.selected, tree_scroll, left, buf);

        let chat_intent = |st: &ChatState| Store::load(&st.intent_path).ok();
        match &self.screen {
            Screen::Edit => self.editor.render(right, buf, true),
            Screen::Reader {
                title,
                lines,
                scroll,
            } => ui::render_reader(title, lines, *scroll, right, buf),
            Screen::AgentPick {
                step,
                items,
                selected,
                ..
            } => {
                let title = match step {
                    PickStep::Provider => "refine with which agent?".to_string(),
                    PickStep::Model { provider } => format!("{provider}: which model?"),
                };
                ui::render_list(&title, items, *selected, right, buf);
            }
            Screen::Chat(st) => {
                let (top, bottom) = ui::split_chat(right);
                if let Some(it) = chat_intent(st) {
                    ui::render_detail(&it, &Store::attachments(&it), top, buf);
                }
                ui::render_chat(
                    &st.transcript,
                    &st.input,
                    st.turn.is_some(),
                    st.hint.as_deref(),
                    &st.provider,
                    st.scroll,
                    bottom,
                    buf,
                );
            }
            Screen::Diff {
                proposal,
                questions,
                scroll,
                ..
            } => {
                let old = self
                    .parked_chat
                    .as_ref()
                    .and_then(chat_intent)
                    .map(|i| i.body)
                    .unwrap_or_default();
                ui::render_diff(
                    &old,
                    &merge_questions(proposal, questions),
                    *scroll,
                    right,
                    buf,
                );
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
                    "handoff: {}   provider: {} (Tab to change)   repo: {}\n\nEnter = send, Esc = cancel, j/k scroll\n\n",
                    handoff.name,
                    provider,
                    repo.display()
                );
                let text = header + &ui::sanitize(prompt);
                ratatui::widgets::Paragraph::new(text)
                    .wrap(ratatui::widgets::Wrap { trim: false })
                    .scroll((*scroll, 0))
                    .render(right, buf);
            }
            _ => match self.rows.get(self.selected) {
                Some(Row::Intent { intent, .. }) => {
                    let answering = match &self.screen {
                        Screen::Answer { index, text } => Some((*index, text.as_str())),
                        _ => None,
                    };
                    let attachments = Store::attachments(intent);
                    ui::render_capability(
                        &ui::DetailView {
                            intent,
                            attachments: &attachments,
                            focus: self.focus,
                            description_scroll: self.description_scroll,
                            preconditions_selected: self.preconditions_selected,
                            criteria_selected: self.criteria_selected,
                            question_selected: self.question_selected,
                            artifacts_selected: self.artifacts_selected,
                            found: self.found_artifacts(),
                            answering,
                        },
                        right,
                        buf,
                    );
                }
                Some(Row::Broken { path, error, .. }) => ratatui::widgets::Paragraph::new(
                    ui::sanitize(&format!("{}\n\n{}", path.display(), error)),
                )
                .wrap(ratatui::widgets::Wrap { trim: false })
                .render(right, buf),
                _ => ratatui::widgets::Paragraph::new("a add capability · n new group · ? help")
                    .render(right, buf),
            },
        }

        let status_text = match &self.screen {
            Screen::Input { label, text, .. } => format!("{label}: {text}▏"),
            Screen::Prompt { question, .. } => question.clone(),
            Screen::Edit => "Ctrl-s save · Ctrl-p polish · Ctrl-e $EDITOR · Esc back".to_string(),
            Screen::Chat(_) => "Enter send · Ctrl-a apply · Ctrl-d dismiss · PgUp/PgDn scroll · Esc cancel/back".to_string(),
            Screen::Diff { title, .. } => match title {
                Some(t) => format!("Enter apply (title → {t}) · Esc back · PgUp/PgDn scroll"),
                None => "Enter apply · Esc back · PgUp/PgDn scroll".to_string(),
            },
            Screen::SendPick { .. } | Screen::AgentPick { .. } => "j/k choose · Enter select · Esc back".to_string(),
            Screen::Answer { .. } => "type the answer · Enter saves · Esc cancels".to_string(),
            Screen::Reader { .. } => "j/k · PgUp/PgDn scroll · Esc back".to_string(),
            Screen::SendConfirm { .. } => "Enter send · Tab provider · j/k scroll · Esc cancel".to_string(),
            Screen::Browse => match (&self.message, self.focus) {
                (Some(m), _) => m.clone(),
                (None, Pane::Tree) => "v list  Tab panes  a add  t title  e edit  p polish  Q answer  C criteria  V artifacts  s send  R ready  K kind  n group  m move  A attach  r rescan  q quit".to_string(),
                (None, Pane::Description) => "Description: j/k scroll · Enter edit · Tab next pane · Esc tree".to_string(),
                (None, Pane::Preconditions) => "Preconditions: j/k choose · Enter/space confirm · a add · Tab next pane · Esc tree".to_string(),
                (None, Pane::Criteria) => "Criteria: j/k choose · Enter/space toggle · a add · Tab next pane · Esc tree".to_string(),
                (None, Pane::Questions) => "Questions: j/k choose · Enter answer · a add · Tab next pane · Esc tree".to_string(),
                (None, Pane::Artifacts) => "Artifacts: j/k choose · Enter read · o open outside · L link a found one · a add · Esc tree".to_string(),
            },
        };
        ui::render_status(&status_text, status, buf);
    }
}

fn binary_mtime() -> Option<std::time::SystemTime> {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

fn row_path(r: &Row) -> PathBuf {
    match r {
        Row::Group { path, .. } => path.clone(),
        Row::Intent { intent, .. } => intent.path.clone(),
        Row::Broken { path, .. } => path.clone(),
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
    if body.to_lowercase().contains("## open questions") {
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
/// While set, the event thread does not read the tty (an external editor owns it).
static PAUSE_INPUT: AtomicBool = AtomicBool::new(false);

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

/// Run `$EDITOR` (or the configured editor) on `path` with the tty handed over.
fn external_edit(cfg: &Config, path: &Path) {
    PAUSE_INPUT.store(true, Ordering::Relaxed);
    // let the event thread finish its current poll before the editor takes the tty
    std::thread::sleep(Duration::from_millis(120));
    restore_terminal();
    let editor = match cfg.editor.as_str() {
        "$EDITOR" | "" => std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string()),
        other => other.to_string(),
    };
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("--")
        .arg(path)
        .status();
    PAUSE_INPUT.store(false, Ordering::Relaxed);
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
        loop {
            if PAUSE_INPUT.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            match crossterm::event::poll(Duration::from_millis(100)) {
                Ok(true) => match crossterm::event::read() {
                    Ok(ev) => {
                        if events_tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
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
                Flow::ExternalEdit => {
                    if let Some(it) = app.editing.clone() {
                        external_edit(&app.cfg, &it.path);
                        // drop anything the editor left in the queue, then rebuild the TUI
                        while events_rx.try_recv().is_ok() {}
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
            Ok(Event::Paste(text)) => app.paste(&text),
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
            let _ = writeln!(std::io::stderr(), "herdr-capabilities: {e:#}");
            std::process::exit(1)
        }
    }
}

/// `add` mode: popup asks for a title; otherwise title from args or stdin's first line, body = rest.
pub fn add(cfg: Config, dirs: Dirs, title: Option<String>, popup: bool) -> anyhow::Result<()> {
    let store = Store::open(&cfg.root)?;
    let repo = crate::send::capture_repo();
    let body = template(&dirs, "idea", false).unwrap_or_default();
    if popup {
        let mut terminal = init_terminal();
        let mut text = title.unwrap_or_default();
        let result: anyhow::Result<Option<String>> = (|| loop {
            terminal.draw(|f| {
                let area = f.area();
                let p = ratatui::widgets::Paragraph::new(format!(
                    "New capability (Inbox)\n\ntitle: {text}▏\n\nEnter = save, Esc or Ctrl-c = cancel"
                ));
                p.render(area, f.buffer_mut());
            })?;
            match crossterm::event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    match (k.code, k.modifiers.contains(KeyModifiers::CONTROL)) {
                        (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => return Ok(None),
                        (KeyCode::Enter, _) => return Ok(Some(text.clone())),
                        (KeyCode::Backspace, _) => {
                            text.pop();
                        }
                        (KeyCode::Char(c), false) => text.push(c),
                        _ => {}
                    }
                }
                Event::Paste(p) => text.push_str(p.replace(['\r', '\n'], " ").trim()),
                _ => {}
            }
        })();
        restore_terminal();
        if let Some(t) = result? {
            let t = t.trim();
            if !t.is_empty() {
                store.create(Path::new(crate::store::INBOX), t, "idea", &body, repo)?;
            }
        }
        return Ok(());
    }
    let mut input = String::new();
    let stdin_is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if title.is_none() && stdin_is_tty {
        anyhow::bail!(
            "usage: herdr-capabilities add <title>   (or pipe a title line and body on stdin)"
        );
    }
    if !stdin_is_tty {
        use std::io::Read;
        let _ = std::io::stdin().read_to_string(&mut input);
    }
    let input = input.replace("\r\n", "\n").replace('\r', "\n");
    let (t, body_text) = match title {
        Some(t) => (t, if input.trim().is_empty() { body } else { input }),
        None => {
            let mut lines = input.lines();
            let t = lines
                .next()
                .context("no title given and stdin is empty")?
                .to_string();
            let rest = lines.collect::<Vec<_>>().join("\n");
            (t, if rest.trim().is_empty() { body } else { rest })
        }
    };
    let it = store.create(
        Path::new(crate::store::INBOX),
        t.trim(),
        "idea",
        &body_text,
        repo,
    )?;
    println!("{}", it.path.display());
    Ok(())
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
            extra: Vec::new(),
            models: vec!["model-a".into(), "model-b".into()],
            model_flag: "--model".into(),
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

    /// Move the selected capability out of `raw` so the send gate lets it through.
    fn make_refining(app: &mut App) {
        let path = app.selected_intent().unwrap().path.clone();
        Store::update_fields(&path, |fm| fm.state = State::Refining).unwrap();
        app.rescan();
    }

    #[test]
    fn s_refuses_a_raw_capability_and_one_with_open_questions() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox"); // two open questions
        make_refining(&mut fx.app);
        key(&mut fx.app, KeyCode::Char('s'));
        assert_eq!(fx.app.view().screen, "browse");
        assert!(
            fx.app
                .view()
                .message
                .unwrap_or_default()
                .contains("2 open questions")
        );
        select(&mut fx.app, "Alpha"); // raw
        key(&mut fx.app, KeyCode::Char('s'));
        assert_eq!(fx.app.view().screen, "browse");
        assert!(fx.app.view().message.unwrap_or_default().contains("raw"));
    }

    fn list_title(app: &App) -> String {
        let v = app.view();
        v.list_titles[v.list_selected].clone()
    }

    fn set_state(app: &mut App, needle: &str, state: State) {
        let path = app
            .store
            .scan()
            .unwrap()
            .groups
            .iter()
            .flat_map(|g| g.intents.iter())
            .find(|i| i.fm.title.contains(needle))
            .unwrap()
            .path
            .clone();
        Store::update_fields(&path, |fm| fm.state = state).unwrap();
        app.rescan();
    }

    /// Press `S` until the list is sorted by `order`.
    fn sort_by(app: &mut App, order: &str) {
        for _ in 0..4 {
            if app.view().sort == order {
                return;
            }
            key(app, KeyCode::Char('S'));
        }
        panic!("no sort order {order}");
    }

    #[test]
    fn v_toggles_the_list_and_keeps_the_selection() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Beta");
        key(&mut fx.app, KeyCode::Char('v'));
        assert_eq!(fx.app.view().layout, "list");
        assert_eq!(list_title(&fx.app), "Beta");
        assert_eq!(fx.app.selected_intent().unwrap().fm.title, "Beta");
        key(&mut fx.app, KeyCode::Char('j'));
        if list_title(&fx.app) == "Beta" {
            key(&mut fx.app, KeyCode::Char('k')); // Beta was last
        }
        let now = list_title(&fx.app);
        assert_ne!(now, "Beta");
        key(&mut fx.app, KeyCode::Char('v'));
        let v = fx.app.view();
        assert_eq!(v.layout, "tree");
        assert!(v.rows[v.selected].label.contains(&now), "{:?}", v.rows);
    }

    #[test]
    fn the_list_holds_every_capability_across_groups() {
        let mut fx = setup("agy.jsonl");
        let sub = fx
            .app
            .store
            .create_group(Path::new("skoolscout"), "deep")
            .unwrap();
        let rel = sub.strip_prefix(fx.app.store.root()).unwrap().to_path_buf();
        fx.app
            .store
            .create(&rel, "Hidden deep", "idea", "x\n", None)
            .unwrap();
        fx.app.rescan();
        assert!(
            !fx.app
                .view()
                .rows
                .iter()
                .any(|r| r.label.contains("Hidden deep")),
            "collapsed in the tree"
        );
        key(&mut fx.app, KeyCode::Char('v'));
        let v = fx.app.view();
        assert_eq!(v.list_titles.len(), 4, "{:?}", v.list_titles);
        assert!(v.list_titles.iter().any(|t| t == "Hidden deep"));
        // Enter on it opens the tree with its group expanded and it selected
        sort_by(&mut fx.app, "Title");
        for _ in 0..10 {
            key(&mut fx.app, KeyCode::Char('k'));
        }
        for _ in 0..10 {
            if list_title(&fx.app) == "Hidden deep" {
                break;
            }
            key(&mut fx.app, KeyCode::Char('j'));
        }
        assert_eq!(list_title(&fx.app), "Hidden deep", "{:?}", fx.app.view());
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.layout, "tree");
        assert!(
            v.rows[v.selected].label.contains("Hidden deep"),
            "{:?}",
            v.rows
        );
    }

    #[test]
    fn s_cycles_sort_orders() {
        let mut fx = setup("agy.jsonl");
        key(&mut fx.app, KeyCode::Char('v'));
        assert_eq!(fx.app.view().sort, "Updated", "the default");
        sort_by(&mut fx.app, "Title");
        assert_eq!(
            fx.app.view().list_titles,
            vec!["Alpha", "Beta", "In the inbox"]
        );
        // Progress: raw first, then ?N, then ready
        set_state(&mut fx.app, "In the inbox", State::Refining); // ?2
        set_state(&mut fx.app, "Beta", State::Refining); // no questions: ready
        sort_by(&mut fx.app, "Progress");
        assert_eq!(
            fx.app.view().list_titles,
            vec!["Alpha", "In the inbox", "Beta"]
        );
        // Captured: newest created first
        let scan = fx.app.store.scan().unwrap();
        for (i, it) in scan
            .groups
            .iter()
            .flat_map(|g| g.intents.iter())
            .enumerate()
        {
            let when = chrono::Local::now() - chrono::Duration::days(10 - i as i64);
            let title = it.fm.title.clone();
            Store::update_fields(&it.path, |fm| {
                fm.created = when;
                fm.title = title;
            })
            .unwrap();
        }
        fx.app.rescan();
        sort_by(&mut fx.app, "Captured");
        let titles = fx.app.view().list_titles;
        let created: Vec<_> = titles
            .iter()
            .map(|t| {
                fx.app
                    .store
                    .scan()
                    .unwrap()
                    .groups
                    .iter()
                    .flat_map(|g| g.intents.iter())
                    .find(|i| &i.fm.title == t)
                    .unwrap()
                    .fm
                    .created
            })
            .collect();
        assert!(created.windows(2).all(|w| w[0] >= w[1]), "{titles:?}");
    }

    #[test]
    fn the_layout_and_sort_are_remembered() {
        let mut fx = setup("agy.jsonl");
        key(&mut fx.app, KeyCode::Char('v'));
        sort_by(&mut fx.app, "Progress");
        let again = App::new(fx.app.cfg.clone(), fx.app.dirs.clone()).unwrap();
        let v = again.view();
        assert_eq!(v.layout, "list");
        assert_eq!(v.sort, "Progress");
    }

    #[test]
    fn save_view_keeps_the_provider() {
        let fx = setup("agy.jsonl");
        crate::config::save_last_used(
            &fx.app.dirs,
            &LastUsed {
                provider: "claude".into(),
                model: Some("opus".into()),
                ..Default::default()
            },
        )
        .unwrap();
        crate::config::save_view(&fx.app.dirs, "list", "Title").unwrap();
        let last = crate::config::last_used(&fx.app.dirs, &fx.app.cfg);
        assert_eq!(last.provider, "claude");
        assert_eq!(last.model.as_deref(), Some("opus"));
        assert_eq!(last.layout.as_deref(), Some("list"));
        assert_eq!(last.sort.as_deref(), Some("Title"));
    }

    struct ListFx {
        tmp: tempfile::TempDir,
        app: App,
    }

    /// A capability file written by hand so its timestamps are fixed.
    fn write_cap(root: &Path, dir: &str, stem: &str, front: &str, body: &str) {
        let d = root.join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join(format!("{stem}.md")),
            format!("---\n{front}---\n{body}"),
        )
        .unwrap();
    }

    /// Six capabilities covering every row kind, a repo with a plan and its
    /// ledger, a fake gh that knows PR #42 is open, and a fake herdr whose
    /// agent for the PR capability is blocked.
    fn list_fixture(repo_for_sent: bool) -> ListFx {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join("docs/superpowers/plans")).unwrap();
        fs::write(
            repo.join("docs/superpowers/plans/reassign.md"),
            "### Task 1: a\n### Task 2: b\n### Task 3: c\n",
        )
        .unwrap();
        fs::create_dir_all(repo.join(".superpowers/sdd/reassign")).unwrap();
        fs::write(
            repo.join(".superpowers/sdd/reassign/progress.md"),
            "# SDD ledger — plan: docs/superpowers/plans/reassign.md\nTask 1: complete\n",
        )
        .unwrap();
        let repo_line = if repo_for_sent {
            format!("repo: {}\n", repo.display())
        } else {
            String::new()
        };
        let sent = |agent: &str| {
            format!(
                "state: sent\n{repo_line}sent_to:\n  handoff: grill\n  workspace_id: w9\n  agent: {agent}\n  at: 2026-10-25T08:10:00-04:00\n"
            )
        };
        let root = &dirs.root;
        write_cap(
            root,
            "Inbox",
            "raw",
            "title: a\nkind: idea\nstate: raw\ncreated: 2026-10-25 07:30:00\nupdated: 2026-10-25 07:30:00\n",
            "## Description\n\n",
        );
        write_cap(
            root,
            "skoolscout/org",
            "open",
            "title: Families can skip a folder without seeing Pending\nkind: issue\nstate: refining\ncreated: 2026-10-24 09:00:00\nupdated: 2026-10-24 18:02:00\n",
            "## Description\nWhen a parent uses Skip for now on the report-card folder, the folder still shows up labelled Pending.\nIt reads as if something is still owed, so families write in to ask what they missed and staff have to explain it every time.\nA third line that should be cut.\n\n## Open questions\n- Which org saw this?\n- Since when?\n",
        );
        write_cap(
            root,
            "skoolscout/org",
            "ready",
            "title: Org admins can export candidate lists as CSV\nkind: enhancement\nstate: refining\ncreated: 2026-10-22 09:00:00\nupdated: 2026-10-23 09:41:00\n",
            "## Description\nAdmins need a one-click export of the filtered candidates table.\n\n## Open questions\n- Which columns?\n  → the visible ones\n",
        );
        write_cap(
            root,
            "skoolscout/org",
            "pr",
            &format!(
                "title: Notify primary contact to schedule a meeting\nkind: idea\ncreated: 2026-10-25 00:34:00\nupdated: 2026-10-25 08:10:00\n{}",
                sent("intent-notify-547e")
            ),
            "## Description\nThe primary contact gets no prompt to book the intro meeting.\n\n## Acceptance criteria\n- [ ] gets an email\n\n## Artifacts\n- spec: docs/superpowers/specs/notify.md\n- pr: #42\n",
        );
        write_cap(
            root,
            "skoolscout/org",
            "building",
            &format!(
                "title: Counselors can reassign a student between orgs\nkind: idea\ncreated: 2026-10-21 09:00:00\nupdated: 2026-10-22 16:15:00\n{}",
                sent("intent-reassign-0001")
            ),
            "## Description\nReassignment needs a support ticket today.\n\n## Acceptance criteria\n- [ ] moves\n- [ ] keeps history\n\n## Artifacts\n- plan: docs/superpowers/plans/reassign.md\n",
        );
        write_cap(
            root,
            "skoolscout/org",
            "done",
            &format!(
                "title: Invitees skip the join request\nkind: idea\ncreated: 2026-10-19 09:00:00\nupdated: 2026-10-20 10:30:00\n{}",
                sent("intent-invitees-0002")
            ),
            "## Description\nInvited parents no longer file a join request.\n\n## Acceptance criteria\n- [x] no request\n",
        );
        write_cap(
            root,
            "Inbox",
            "long",
            "title: \"A very long title that keeps going \\a well past any reasonable width so that it must be cut off cleanly at the edge\"\nkind: idea\nstate: raw\ncreated: 2026-10-18 09:00:00\nupdated: 2026-10-18 09:00:00\n",
            "## Description\nshort\n",
        );

        let notify_wt = repo.join(".claude/worktrees/org/notify");
        fs::create_dir_all(notify_wt.join("app-ui")).unwrap();
        fs::write(notify_wt.join(".slot"), "3\n").unwrap();
        let agents = tmp.path().join("agents.json");
        fs::write(
            &agents,
            format!(
                r#"{{"result":{{"agents":[{{"agent":"claude","agent_status":"blocked","name":"intent-notify-547e","foreground_cwd":"{}"}},{{"agent":"claude","agent_status":"working","name":"intent-reassign-0001"}}]}}}}"#,
                notify_wt.join("app-ui").display()
            ),
        )
        .unwrap();
        let prs = tmp.path().join("prs");
        fs::create_dir_all(&prs).unwrap();
        fs::write(prs.join("42"), "OPEN").unwrap();
        let mut app = App::new(cfg, dirs).unwrap();
        app.herdr_env = vec![
            (
                "HERDR_BIN_PATH".into(),
                fixtures().join("fake-herdr.sh").display().to_string(),
            ),
            (
                "FAKE_HERDR_LOG".into(),
                tmp.path().join("herdr.log").display().to_string(),
            ),
            ("FAKE_AGENT_LIST".into(), agents.display().to_string()),
            (
                "GH_BIN_PATH".into(),
                fixtures().join("fake-gh.sh").display().to_string(),
            ),
            ("FAKE_GH_DIR".into(), prs.display().to_string()),
        ];
        ListFx { tmp, app }
    }

    /// Tick until every background request has reported (5 s cap).
    fn settle(app: &mut App) {
        let start = Instant::now();
        app.tick();
        while app.polls_in_flight() > 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
            app.tick();
        }
        assert_eq!(app.polls_in_flight(), 0, "background polls never finished");
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| app.draw(Rect::new(0, 0, w, h), f.buffer_mut()))
            .unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn list_snapshot_at_80_and_120_columns() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let wide = render(&fx.app, 120, 60);
        assert!(wide.contains("! needs you"), "{wide}");
        assert!(wide.contains("PR #42 open"), "{wide}");
        assert!(wide.contains("building 1/3"), "{wide}");
        assert!(wide.contains("✓ complete"), "{wide}");
        assert!(wide.contains("?2"), "{wide}");
        assert!(wide.contains("working"), "live agent state: {wide}");
        assert!(wide.contains("closed"), "agent pane gone: {wide}");
        insta::assert_snapshot!("list_120", wide);
        let narrow = render(&fx.app, 80, 60);
        assert!(
            narrow.contains('…'),
            "long titles cut with an ellipsis: {narrow}"
        );
        assert!(!narrow.contains('\u{7}'));
        assert!(
            wide.contains("A very long title"),
            "the long row exists: {wide}"
        );
        insta::assert_snapshot!("list_80", narrow);
    }

    #[test]
    fn card_titles_are_bold_and_cards_open_with_a_rule() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let (w, h) = (100u16, 60u16);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| fx.app.draw(Rect::new(0, 0, w, h), f.buffer_mut()))
            .unwrap();
        let buf = t.backend().buffer().clone();
        let row = |y: u16| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>();
        let title_y = (0..h)
            .find(|&y| row(y).starts_with(" Invitees skip"))
            .expect("card title row");
        assert!(
            buf[(1, title_y)]
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD),
            "title is bold"
        );
        assert!(
            row(title_y - 1).starts_with("────"),
            "the card opens with a rule: {:?}",
            row(title_y - 1)
        );
    }

    fn list_goto(app: &mut App, title: &str) {
        for _ in 0..10 {
            key(app, KeyCode::Char('k'));
        }
        for _ in 0..10 {
            if list_title(app).starts_with(title) {
                return;
            }
            key(app, KeyCode::Char('j'));
        }
        panic!("no list row {title}");
    }

    #[test]
    fn send_from_the_list_shows_that_capability_in_the_tree() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        list_goto(&mut fx.app, "Org admins");
        key(&mut fx.app, KeyCode::Char('s'));
        let v = fx.app.view();
        assert_eq!(v.screen, "send-pick");
        assert!(
            v.rows[v.selected].label.contains("Org admins"),
            "the tree beside the picker points at the capability being sent: {:?}",
            v.rows.get(v.selected)
        );
    }

    #[test]
    fn the_list_timer_picks_up_lines_appended_on_disk() {
        let mut fx = list_fixture(true);
        fs::write(fx.tmp.path().join("prs/43"), "OPEN").unwrap();
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let path = fx.app.store.root().join("skoolscout/org/building.md");
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("- pr: #43\n");
        fs::write(&path, text).unwrap();
        fx.app.poll_ticks = 1800; // the minute timer is due
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 60);
        assert!(shown.contains("PR #43 open"), "{shown}");
    }

    #[test]
    fn the_agent_field_is_blank_when_herdr_cannot_answer() {
        let mut fx = list_fixture(true);
        fx.app.herdr_env.retain(|(k, _)| k != "FAKE_AGENT_LIST"); // fake herdr now fails
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 60);
        assert!(
            shown.contains("Oct 25 08:10 │ sent   │ —"),
            "no endless loading mark: {shown}"
        );
    }

    #[test]
    fn cards_show_where_the_capability_lives() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 60);
        assert!(
            shown.contains(" skoolscout/org │ worktree org/notify │ slot 3"),
            "group, then the sent agent's worktree and slot: {shown}"
        );
        assert!(
            shown.lines().any(|l| l == " Inbox"),
            "an unsent card shows its group alone: {shown}"
        );
        let lines: Vec<&str> = shown.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.contains("worktree org/notify"))
            .unwrap();
        assert_eq!(lines[at + 1], "", "a blank line before the description");
        assert!(
            lines[at + 2].starts_with(" The primary contact"),
            "{}",
            lines[at + 2]
        );
    }

    #[test]
    fn s_in_the_list_is_gated_and_sends_when_ready() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        sort_by(&mut fx.app, "Title");
        let goto = |app: &mut App, title: &str| {
            for _ in 0..10 {
                key(app, KeyCode::Char('k'));
            }
            for _ in 0..10 {
                if list_title(app).starts_with(title) {
                    return;
                }
                key(app, KeyCode::Char('j'));
            }
            panic!("no list row {title}");
        };
        goto(&mut fx.app, "Families");
        key(&mut fx.app, KeyCode::Char('s'));
        assert_eq!(fx.app.view().screen, "browse");
        assert!(
            fx.app
                .view()
                .message
                .unwrap_or_default()
                .contains("2 open questions")
        );
        goto(&mut fx.app, "Org admins");
        key(&mut fx.app, KeyCode::Char('s'));
        assert_eq!(fx.app.view().screen, "send-pick");
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(
            fx.app.view().layout,
            "list",
            "back in the list after the picker"
        );
    }

    #[test]
    fn g_focuses_the_sent_agent() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        sort_by(&mut fx.app, "Title");
        for _ in 0..10 {
            if list_title(&fx.app).starts_with("Notify") {
                break;
            }
            key(&mut fx.app, KeyCode::Char('j'));
        }
        key(&mut fx.app, KeyCode::Char('g'));
        let log = fx.tmp.path().join("herdr.log");
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("agent focus intent-notify-547e")
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no focus call: {:?}", fs::read_to_string(&log));
    }

    #[test]
    fn a_sent_capability_without_repo_still_renders() {
        let mut fx = list_fixture(false);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let text = render(&fx.app, 120, 60);
        assert!(text.contains("✓ complete"), "{text}");
        assert!(
            text.contains("building ?"),
            "plan unreadable without a repo: {text}"
        );
    }

    #[test]
    fn pollers_only_run_while_the_list_shows() {
        let mut fx = list_fixture(true);
        for _ in 0..200 {
            fx.app.tick();
        }
        std::thread::sleep(Duration::from_millis(100));
        let log = fs::read_to_string(fx.tmp.path().join("herdr.log")).unwrap_or_default();
        assert!(
            !log.contains("agent list"),
            "tree layout polls nothing: {log}"
        );
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let log = fs::read_to_string(fx.tmp.path().join("herdr.log")).unwrap_or_default();
        assert!(log.contains("agent list"), "the list polls on entry: {log}");
    }

    /// Give the selected intent a repo so `send` has somewhere to go.
    fn set_repo(app: &mut App, repo: &Path) {
        let path = app.selected_intent().unwrap().path.clone();
        let repo = repo.to_path_buf();
        Store::update_fields(&path, |fm| fm.repo = Some(repo)).unwrap();
        app.rescan();
    }

    fn key(app: &mut App, code: KeyCode) -> Flow {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    /// `p` opens the agent picker; accept the preselected provider and model.
    fn polish(app: &mut App) {
        key(app, KeyCode::Char('p'));
        assert_eq!(app.view().screen, "agent-pick", "{:?}", app.view());
        key(app, KeyCode::Enter);
        if app.view().screen == "agent-pick" {
            key(app, KeyCode::Enter);
        }
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
        polish(&mut fx.app);
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
        polish(&mut fx2.app);
        wait_until(&mut fx2.app, "turn finished", |a| {
            !a.view().turn_running && a.view().transcript_len >= 2
        });
        let v = fx2.app.view();
        assert!(!v.can_apply);
        assert!(
            v.hint.as_deref().unwrap_or("").contains("no proposal"),
            "{v:?}"
        );
        ctrl(&mut fx2.app, 'a');
        assert_ne!(fx2.app.view().screen, "diff");
    }

    #[test]
    fn apply_replaces_whole_body_after_diff_and_records_questions_and_polished_at() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        polish(&mut fx.app);
        wait_until(&mut fx.app, "turn finished", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        ctrl(&mut fx.app, 'a');
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
        polish(&mut fx.app);
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
        polish(&mut fx.app);
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
        let repo = fx._tmp.path().to_path_buf();
        set_repo(&mut fx.app, &repo);
        make_refining(&mut fx.app);
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
        let repo2 = fx2._tmp.path().to_path_buf();
        set_repo(&mut fx2.app, &repo2);
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
        let repo = fx._tmp.path().to_path_buf();
        set_repo(&mut fx.app, &repo);
        make_refining(&mut fx.app);
        key(&mut fx.app, KeyCode::Char('s'));
        key(&mut fx.app, KeyCode::Enter);
        key(&mut fx.app, KeyCode::Enter);
        let it = Store::load(&fx.app.selected_intent().unwrap().path).unwrap();
        assert_eq!(
            it.fm.state,
            State::Refining,
            "unchanged when herdr is missing"
        );
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

    #[test]
    fn subgroups_start_collapsed_and_expand_progressively() {
        let fx = setup("agy.jsonl");
        let root = fx.app.store.root().to_path_buf();
        for sub in ["a/one", "a/two", "a/one/deep"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let mut app = App::new(fx.app.cfg.clone(), fx.app.dirs.clone()).unwrap();
        let labels: Vec<String> = app.view().rows.iter().map(|r| r.label.clone()).collect();
        assert!(labels.contains(&"a".to_string()));
        assert!(
            labels.contains(&"one".to_string()),
            "sub-groups show as collapsed rows: {labels:?}"
        );
        assert!(
            !labels.contains(&"deep".to_string()),
            "a sub-group's contents stay hidden until expanded: {labels:?}"
        );
        assert!(
            labels.contains(&"In the inbox".to_string()),
            "top-level groups show their intents"
        );
        select(&mut app, "one");
        key(&mut app, KeyCode::Char('l'));
        let labels: Vec<String> = app.view().rows.iter().map(|r| r.label.clone()).collect();
        assert!(
            labels.contains(&"deep".to_string()),
            "expanding discloses one more level: {labels:?}"
        );
    }

    #[test]
    fn tree_scrolls_to_keep_the_selection_visible() {
        let fx = setup("agy.jsonl");
        let root = fx.app.store.root().to_path_buf();
        for i in 0..40 {
            fs::create_dir_all(root.join(format!("g{i:02}"))).unwrap();
        }
        let mut app = App::new(fx.app.cfg.clone(), fx.app.dirs.clone()).unwrap();
        for _ in 0..30 {
            key(&mut app, KeyCode::Char('j'));
        }
        let selected_label = app.view().rows[app.view().selected].label.clone();
        let mut t = Terminal::new(TestBackend::new(60, 10)).unwrap();
        t.draw(|f| app.draw(Rect::new(0, 0, 60, 10), f.buffer_mut()))
            .unwrap();
        let text = format!("{:?}", t.backend().buffer());
        assert!(
            text.contains(&selected_label),
            "selected row {selected_label} must be drawn; buffer:\n{text}"
        );
    }

    // C1: typing a message starting with "a" or "d" must not apply or dismiss
    #[test]
    fn chat_letters_are_typed_and_apply_is_ctrl_a() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        polish(&mut fx.app);
        wait_until(&mut fx.app, "turn", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        type_str(&mut fx.app, "also mention");
        assert_eq!(fx.app.view().screen, "chat", "`a` is typed, not apply");
        assert_eq!(fx.app.view().chat_input, "also mention");
        ctrl(&mut fx.app, 'a');
        assert_eq!(fx.app.view().screen, "diff", "Ctrl-a opens the diff");
        type_str(&mut fx.app, "xyz");
        assert_eq!(
            fx.app.view().screen,
            "diff",
            "printable keys do nothing on the diff screen"
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "chat");
        assert_eq!(
            fx.app.view().chat_input,
            "also mention",
            "input survives the round trip"
        );
        ctrl(&mut fx.app, 'd');
        assert!(!fx.app.view().can_apply, "Ctrl-d dismisses");
    }

    // C2: field changes and the conversation-id save must not clobber another writer
    #[test]
    fn state_change_and_conversation_id_keep_another_writers_body() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        std::thread::sleep(Duration::from_millis(1100));
        let mut other = Store::load(&path).unwrap();
        other.body = "other toolbar wrote this\n".into();
        Store::save_force(&mut other).unwrap();
        key(&mut fx.app, KeyCode::Char('R'));
        let it = Store::load(&path).unwrap();
        assert_eq!(it.fm.state, State::Ready);
        assert_eq!(
            it.body, "other toolbar wrote this\n",
            "R must not overwrite the body"
        );
        polish(&mut fx.app);
        wait_until(&mut fx.app, "turn", |a| !a.view().turn_running);
        let it = Store::load(&path).unwrap();
        assert_eq!(
            it.body, "other toolbar wrote this\n",
            "the id save must not overwrite the body"
        );
        assert!(it.fm.conversations.contains_key("fake"));
    }

    #[test]
    fn apply_prompts_when_the_file_changed_during_the_chat() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        polish(&mut fx.app);
        wait_until(&mut fx.app, "turn", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        let path = fx.app.selected_intent().unwrap().path.clone();
        std::thread::sleep(Duration::from_millis(1100));
        let mut other = Store::load(&path).unwrap();
        other.body = "changed elsewhere\n".into();
        Store::save_force(&mut other).unwrap();
        ctrl(&mut fx.app, 'a');
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "prompt", "{:?}", fx.app.view());
        assert_eq!(
            Store::load(&path).unwrap().body,
            "changed elsewhere\n",
            "nothing written yet"
        );
        key(&mut fx.app, KeyCode::Char('n'));
        assert_eq!(Store::load(&path).unwrap().body, "changed elsewhere\n");
    }

    #[test]
    fn send_confirm_renders_the_prompt_from_disk_not_the_cached_row() {
        let mut fx = setup("agy.jsonl");
        fx.app.herdr_env = vec![
            (
                "HERDR_BIN_PATH".into(),
                fixtures().join("fake-herdr.sh").display().to_string(),
            ),
            (
                "FAKE_HERDR_LOG".into(),
                fx._tmp.path().join("h.log").display().to_string(),
            ),
        ];
        select(&mut fx.app, "Alpha");
        make_refining(&mut fx.app);
        let path = fx.app.selected_intent().unwrap().path.clone();
        let mut other = Store::load(&path).unwrap();
        other.body = "fresh body from disk\n".into();
        other.fm.repo = Some(fx._tmp.path().to_path_buf());
        Store::save_force(&mut other).unwrap();
        key(&mut fx.app, KeyCode::Char('s'));
        key(&mut fx.app, KeyCode::Enter);
        assert!(
            fx.app
                .view()
                .send_prompt
                .unwrap()
                .contains("fresh body from disk")
        );
    }

    // I5: polishing from the editor returns to the editor with the buffer intact
    #[test]
    fn chat_from_editor_returns_to_editor_with_unsaved_text() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Enter);
        key(&mut fx.app, KeyCode::End);
        type_str(&mut fx.app, " unsaved");
        ctrl(&mut fx.app, 'p');
        assert_eq!(fx.app.view().screen, "agent-pick");
        key(&mut fx.app, KeyCode::Enter);
        key(&mut fx.app, KeyCode::Enter);
        wait_until(&mut fx.app, "turn", |a| !a.view().turn_running);
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "edit");
        assert_eq!(fx.app.editor.text(), "alpha body unsaved\n");
    }

    // I6: long chat and diff content scroll
    #[test]
    fn chat_and_diff_scroll_with_page_keys() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        polish(&mut fx.app);
        wait_until(&mut fx.app, "turn", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        key(&mut fx.app, KeyCode::PageUp);
        assert!(
            fx.app.view().chat_scroll > 0,
            "PageUp scrolls the transcript back"
        );
        key(&mut fx.app, KeyCode::PageDown);
        assert_eq!(fx.app.view().chat_scroll, 0);
        ctrl(&mut fx.app, 'a');
        key(&mut fx.app, KeyCode::PageDown);
        assert!(fx.app.view().diff_scroll > 0, "PageDown scrolls the diff");
    }

    // I7: paste goes into whichever text input is active, with CR normalised
    #[test]
    fn paste_reaches_inputs_and_normalises_line_endings() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('A'));
        fx.app.paste("/tmp/shot.png\r\n");
        assert_eq!(fx.app.view().hint.as_deref(), Some("attach file path"));
        assert_eq!(fx.app.view().input_text, "/tmp/shot.png");
        key(&mut fx.app, KeyCode::Esc);
        key(&mut fx.app, KeyCode::Enter);
        fx.app.paste("one\r\ntwo\rthree");
        assert_eq!(fx.app.editor.text(), "one\ntwo\nthreealpha body\n");
    }

    // I11: no repo → refuse to send
    #[test]
    fn send_refuses_when_the_intent_has_no_repo() {
        let mut fx = setup("agy.jsonl");
        fx.app.herdr_env = vec![
            (
                "HERDR_BIN_PATH".into(),
                fixtures().join("fake-herdr.sh").display().to_string(),
            ),
            (
                "FAKE_HERDR_LOG".into(),
                fx._tmp.path().join("h.log").display().to_string(),
            ),
        ];
        select(&mut fx.app, "Alpha"); // created with repo None
        make_refining(&mut fx.app);
        key(&mut fx.app, KeyCode::Char('s'));
        key(&mut fx.app, KeyCode::Enter);
        assert_ne!(fx.app.view().screen, "send-confirm");
        assert!(
            fx.app.view().message.unwrap_or_default().contains("repo"),
            "{:?}",
            fx.app.view().message
        );
    }

    // I12: groups can be moved too
    #[test]
    fn move_works_on_groups() {
        let mut fx = setup("agy.jsonl");
        let root = fx.app.store.root().to_path_buf();
        fs::create_dir_all(root.join("archive")).unwrap();
        fx.app.rescan();
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('m'));
        type_str(&mut fx.app, "archive");
        key(&mut fx.app, KeyCode::Enter);
        assert!(root.join("archive/skoolscout/").is_dir(), "group moved");
        assert!(!root.join("skoolscout").exists());
    }

    // I2: a broken file shows as a row with its error
    #[test]
    fn broken_files_show_as_rows() {
        let mut fx = setup("agy.jsonl");
        fs::write(
            fx.app.store.root().join("Inbox/bad.md"),
            "---\ntitle: [oops\n---\nx\n",
        )
        .unwrap();
        fx.app.rescan();
        let v = fx.app.view();
        assert!(
            v.rows
                .iter()
                .any(|r| r.label.contains("bad.md") && r.label.contains("!")),
            "{:?}",
            v.rows
        );
    }

    #[test]
    fn agent_picker_offers_provider_then_model_and_remembers_the_choice() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('p'));
        let v = fx.app.view();
        assert_eq!(v.screen, "agent-pick");
        assert_eq!(
            v.pick_items[v.pick_selected], "fake",
            "config's polish provider preselected"
        );
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.screen, "agent-pick");
        assert_eq!(v.pick_items, ["model-a", "model-b"]);
        key(&mut fx.app, KeyCode::Char('j'));
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "chat");
        wait_until(&mut fx.app, "turn", |a| !a.view().turn_running);
        let log = fs::read_to_string(&fx.log).unwrap();
        assert!(
            log.contains("--model model-b"),
            "model reaches the provider argv: {log}"
        );
        key(&mut fx.app, KeyCode::Esc);
        // next time, the last choice is preselected
        key(&mut fx.app, KeyCode::Char('p'));
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.pick_items[v.pick_selected], "model-b");
        key(&mut fx.app, KeyCode::Esc);
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
    }

    #[test]
    fn provider_without_models_skips_the_model_step() {
        let mut fx = setup("agy.jsonl");
        fx.app.cfg.providers.get_mut("fake").unwrap().models.clear();
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('p'));
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "chat");
    }

    #[test]
    fn tab_cycles_focus_through_the_three_panes() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox");
        assert_eq!(fx.app.view().focus, "tree");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "description");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "preconditions");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "criteria");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "questions");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "artifacts");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "tree");
        key(&mut fx.app, KeyCode::BackTab);
        assert_eq!(fx.app.view().focus, "artifacts");
        // j/k in the tree still move the selection only while the tree has focus
        key(&mut fx.app, KeyCode::Char('j'));
        assert_eq!(
            fx.app.view().rows[fx.app.view().selected].label,
            "In the inbox"
        );
    }

    #[test]
    fn q_focuses_questions_and_enter_opens_an_inline_answer_box() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox"); // body has "## Open questions\n- a?\n- b?"
        key(&mut fx.app, KeyCode::Char('Q'));
        let v = fx.app.view();
        assert_eq!(v.focus, "questions");
        assert_eq!(v.question_count, 2);
        key(&mut fx.app, KeyCode::Char('j'));
        assert_eq!(fx.app.view().question_selected, 1);
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "answer");
        type_str(&mut fx.app, "because of b");
        fx.app.paste(" (pasted)");
        assert_eq!(fx.app.view().answer_text, "because of b (pasted)");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "browse");
        let it = Store::load(&fx.app.selected_intent().unwrap().path).unwrap();
        assert!(
            it.body.contains("- b?\n  → because of b (pasted)\n"),
            "{}",
            it.body
        );
        assert_eq!(it.open_questions(), 1);
        // Esc cancels an answer without writing
        key(&mut fx.app, KeyCode::Char('k'));
        key(&mut fx.app, KeyCode::Enter);
        type_str(&mut fx.app, "nope");
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
        assert!(
            !Store::load(&fx.app.selected_intent().unwrap().path)
                .unwrap()
                .body
                .contains("nope")
        );
    }

    #[test]
    fn c_focuses_criteria_and_enter_toggles_in_place() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let mut it = Store::load(&path).unwrap();
        it.body = "## Description\nx\n\n## Acceptance criteria\n- [ ] one\n- [ ] two\n".into();
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('C'));
        assert_eq!(fx.app.view().focus, "criteria");
        key(&mut fx.app, KeyCode::Char('j'));
        key(&mut fx.app, KeyCode::Enter);
        assert!(Store::load(&path).unwrap().body.contains("- [x] two"));
        assert_eq!(
            fx.app.view().focus,
            "criteria",
            "focus stays for more toggles"
        );
        key(&mut fx.app, KeyCode::Char(' '));
        assert!(
            Store::load(&path).unwrap().body.contains("- [ ] two"),
            "space toggles back"
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().focus, "tree");
    }

    #[test]
    fn detail_draws_three_separate_panes() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox");
        let mut t = Terminal::new(TestBackend::new(100, 24)).unwrap();
        t.draw(|f| fx.app.draw(Rect::new(0, 0, 100, 24), f.buffer_mut()))
            .unwrap();
        let text = format!("{:?}", t.backend().buffer());
        for title in [
            "Description",
            "Preconditions",
            "Acceptance criteria",
            "Open questions",
        ] {
            assert!(text.contains(title), "pane {title} missing");
        }
        assert!(
            text.contains("? a?") && text.contains("? b?"),
            "questions listed with their open mark: {text}"
        );
    }

    #[test]
    fn t_renames_the_capability_with_the_current_title_prefilled() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('t'));
        let v = fx.app.view();
        assert_eq!(v.screen, "input");
        assert_eq!(v.input_text, "Alpha", "prefilled");
        for _ in 0..5 {
            key(&mut fx.app, KeyCode::Backspace);
        }
        type_str(
            &mut fx.app,
            "Advisors can request a meeting from Office Hours",
        );
        key(&mut fx.app, KeyCode::Enter);
        let path = fx.app.selected_intent().unwrap().path.clone();
        assert_eq!(
            Store::load(&path).unwrap().fm.title,
            "Advisors can request a meeting from Office Hours"
        );
        assert!(
            path.ends_with("alpha.md") || path.to_string_lossy().contains("alpha"),
            "file name is kept: {}",
            path.display()
        );
        assert!(
            fx.app
                .view()
                .rows
                .iter()
                .any(|r| r.label == "Advisors can request a meeting from Office Hours")
        );
    }

    #[test]
    fn applying_a_proposal_with_a_capability_phrase_renames_too() {
        let mut fx = setup("agy.jsonl");
        let titled = fx._tmp.path().join("titled.jsonl");
        fs::write(&titled, r#"{"event":"init","conversation_id":"c2"}
{"event":"step_update","step_update":{"conversation_id":"c2","step_type":"agent_response","text_delta":"capability: Parents can skip a folder without a Pending label\n\n```proposal\n## Description\nnew body\n```\nquestions: none\n"}}
{"event":"result","result":{"conversation_id":"c2","status":"SUCCESS"}}
"#).unwrap();
        fx.app.provider_env[0].1 = titled.display().to_string();
        select(&mut fx.app, "Alpha");
        polish(&mut fx.app);
        wait_until(&mut fx.app, "turn", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        ctrl(&mut fx.app, 'a');
        key(&mut fx.app, KeyCode::Enter);
        let it = Store::load(&fx.app.selected_intent().unwrap().path).unwrap();
        assert_eq!(
            it.fm.title,
            "Parents can skip a folder without a Pending label"
        );
        assert!(it.body.starts_with("## Description\nnew body"));
    }

    #[test]
    fn c_cycles_between_criteria_and_preconditions_and_toggles_each() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let mut it = Store::load(&path).unwrap();
        it.body = "## Description\nx\n\n## Preconditions\n- [ ] pre one\n\n## Acceptance criteria\n- [ ] crit one\n".into();
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('C'));
        assert_eq!(fx.app.view().focus, "criteria");
        key(&mut fx.app, KeyCode::Char('C'));
        assert_eq!(fx.app.view().focus, "preconditions");
        key(&mut fx.app, KeyCode::Enter);
        let body = Store::load(&path).unwrap().body;
        assert!(
            body.contains("- [x] pre one") && body.contains("- [ ] crit one"),
            "{body}"
        );
        key(&mut fx.app, KeyCode::Char('C'));
        assert_eq!(fx.app.view().focus, "criteria");
        key(&mut fx.app, KeyCode::Enter);
        assert!(Store::load(&path).unwrap().body.contains("- [x] crit one"));
    }

    #[test]
    fn apply_returns_to_the_view_and_the_chat_can_resume_later() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        polish(&mut fx.app);
        wait_until(&mut fx.app, "turn", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        ctrl(&mut fx.app, 'a');
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.screen, "browse", "apply lands back in the view: {v:?}");
        assert_eq!(v.focus, "tree");
        assert!(v.message.as_deref().unwrap_or("").contains("applied"));
        // the conversation id survived, so the next polish resumes
        let it = Store::load(&fx.app.selected_intent().unwrap().path).unwrap();
        assert!(it.fm.conversations.contains_key("fake"));
    }

    #[test]
    fn apply_from_the_editor_returns_to_the_editor_with_the_new_body() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Enter);
        ctrl(&mut fx.app, 'p');
        key(&mut fx.app, KeyCode::Enter);
        key(&mut fx.app, KeyCode::Enter);
        wait_until(&mut fx.app, "turn", |a| {
            !a.view().turn_running && a.view().can_apply
        });
        ctrl(&mut fx.app, 'a');
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "edit");
        assert!(
            fx.app
                .editor
                .text()
                .starts_with("## Observed\nParents who skip"),
            "{}",
            fx.app.editor.text()
        );
    }

    #[test]
    fn a_in_a_pane_adds_an_item_to_that_pane() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        key(&mut fx.app, KeyCode::Char('C'));
        key(&mut fx.app, KeyCode::Char('C')); // preconditions
        key(&mut fx.app, KeyCode::Char('a'));
        assert_eq!(fx.app.view().screen, "input");
        assert!(fx.app.view().hint.unwrap().contains("precondition"));
        type_str(&mut fx.app, "Report cards are optional here");
        key(&mut fx.app, KeyCode::Enter);
        let body = Store::load(&path).unwrap().body;
        assert!(
            body.contains("## Preconditions\n- [ ] Report cards are optional here"),
            "{body}"
        );
        assert_eq!(fx.app.view().focus, "preconditions", "stays in the pane");
        key(&mut fx.app, KeyCode::Char('C'));
        key(&mut fx.app, KeyCode::Char('a'));
        type_str(&mut fx.app, "Skipped folders read Skipped");
        key(&mut fx.app, KeyCode::Enter);
        assert!(
            Store::load(&path)
                .unwrap()
                .body
                .contains("## Acceptance criteria\n- [ ] Skipped folders read Skipped")
        );
        key(&mut fx.app, KeyCode::Char('Q'));
        key(&mut fx.app, KeyCode::Char('a'));
        type_str(&mut fx.app, "Which org?");
        key(&mut fx.app, KeyCode::Enter);
        let it = Store::load(&path).unwrap();
        assert!(
            it.body.contains("## Open questions\n- Which org?"),
            "{}",
            it.body
        );
        assert_eq!(it.open_questions(), 1);
    }

    #[test]
    fn a_newer_binary_on_disk_is_announced_in_the_status_line() {
        let mut fx = setup("agy.jsonl");
        let started = std::time::SystemTime::now();
        fx.app.binary_stamp = Some(started);
        fx.app.check_for_newer_build(Some(started));
        assert!(fx.app.view().message.is_none(), "same build: quiet");
        fx.app
            .check_for_newer_build(Some(started + Duration::from_secs(60)));
        let m = fx.app.view().message.unwrap_or_default();
        assert!(m.contains("newer build") && m.contains("Ctrl-] x"), "{m}");
        key(&mut fx.app, KeyCode::Char('j'));
        assert!(
            fx.app
                .view()
                .message
                .unwrap_or_default()
                .contains("newer build"),
            "the notice survives key presses"
        );
    }

    #[test]
    fn ready_requires_every_open_question_to_be_answered() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox"); // two unanswered questions
        key(&mut fx.app, KeyCode::Char('R'));
        let path = fx.app.selected_intent().unwrap().path.clone();
        assert_eq!(
            Store::load(&path).unwrap().fm.state,
            State::Raw,
            "not marked ready"
        );
        let m = fx.app.view().message.unwrap_or_default();
        assert!(m.contains("2 open question"), "{m}");
        // answer both, then R works
        let mut it = Store::load(&path).unwrap();
        it.body = crate::store::answer_question(&it.body, 0, "yes").unwrap();
        it.body = crate::store::answer_question(&it.body, 1, "no").unwrap();
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('R'));
        assert_eq!(Store::load(&path).unwrap().fm.state, State::Ready);
    }

    #[test]
    fn artifacts_pane_lists_links_and_enter_reads_a_markdown_file_in_the_tool() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let repo = fx._tmp.path().join("repo");
        fs::create_dir_all(repo.join("docs")).unwrap();
        fs::write(
            repo.join("docs/spec.md"),
            "# Spec title\n\nSome **bold** text.\n",
        )
        .unwrap();
        let mut it = Store::load(&path).unwrap();
        it.fm.repo = Some(repo.clone());
        it.body =
            "## Description\nx\n\n## Artifacts\n- spec: docs/spec.md\n- decision: #5813\n".into();
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('V'));
        let v = fx.app.view();
        assert_eq!(v.focus, "artifacts");
        assert_eq!(v.artifact_count, 2);
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.screen, "reader", "{v:?}");
        assert!(v.reader_title.contains("spec.md"));
        let mut t = Terminal::new(TestBackend::new(80, 12)).unwrap();
        t.draw(|f| fx.app.draw(Rect::new(0, 0, 80, 12), f.buffer_mut()))
            .unwrap();
        let text = format!("{:?}", t.backend().buffer());
        assert!(
            text.contains("Spec title") && !text.contains("# Spec"),
            "rendered, not raw: {text}"
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
        assert_eq!(fx.app.view().focus, "artifacts");
        // add one by hand
        key(&mut fx.app, KeyCode::Char('a'));
        type_str(&mut fx.app, "plan: docs/plan.md");
        key(&mut fx.app, KeyCode::Enter);
        assert!(
            Store::load(&path)
                .unwrap()
                .body
                .contains("- plan: docs/plan.md")
        );
        assert_eq!(fx.app.view().artifact_count, 3);
    }

    #[test]
    fn reading_a_missing_artifact_file_shows_a_message_not_a_crash() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let mut it = Store::load(&path).unwrap();
        it.fm.repo = Some(fx._tmp.path().to_path_buf());
        it.body = "## Description\nx\n\n## Artifacts\n- spec: docs/nope.md\n".into();
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('V'));
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "browse");
        assert!(
            fx.app
                .view()
                .message
                .unwrap_or_default()
                .contains("nope.md")
        );
    }

    #[test]
    fn found_artifacts_show_in_the_pane_and_l_links_one() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let repo = fx._tmp.path().join("repo");
        fs::create_dir_all(repo.join("docs/superpowers/specs")).unwrap();
        fs::write(
            repo.join("docs/superpowers/specs/2026-10-25-alpha-design.md"),
            "# Alpha design\n\nbody\n",
        )
        .unwrap();
        let mut it = Store::load(&path).unwrap();
        it.fm.repo = Some(repo.clone());
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('V'));
        let v = fx.app.view();
        assert_eq!(v.artifact_count, 0, "nothing linked yet");
        assert_eq!(v.found_count, 1, "but one found by scanning the repo");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(
            fx.app.view().screen,
            "reader",
            "a found file reads like a linked one"
        );
        key(&mut fx.app, KeyCode::Esc);
        key(&mut fx.app, KeyCode::Char('L'));
        let body = Store::load(&path).unwrap().body;
        assert!(
            body.contains("- spec: docs/superpowers/specs/2026-10-25-alpha-design.md"),
            "{body}"
        );
        let v = fx.app.view();
        assert_eq!(
            (v.artifact_count, v.found_count),
            (1, 0),
            "linked, so no longer merely found"
        );
    }
}
