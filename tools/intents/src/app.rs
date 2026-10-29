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
use crate::interview::{Asked, Awaiting, InterviewScreen, MAX_QUESTIONS, Qa, Step};
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
        /// Intents living elsewhere that affect this group or its sub-groups.
        refs: usize,
    },
    Intent {
        intent: Intent,
        depth: usize,
        /// `Some(group)` when shown under `group` because it affects it; the
        /// intent itself lives elsewhere.
        reference: Option<PathBuf>,
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

/// What tests (and the status line) can read about the add conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterviewView {
    /// words | turn | confirm | answer | review | retry
    pub awaiting: &'static str,
    /// The question being answered (1-based), or the next one to ask.
    pub question_no: usize,
    /// The home group's path, or "group to be decided".
    pub home: String,
    pub save_kind: String,
    pub suggested_kind: Option<String>,
    pub save_group: String,
    pub areas: Vec<String>,
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
    /// `D`: move this intent to `.trash`.
    Trash {
        path: PathBuf,
    },
    OverwriteApply {
        body: String,
        title: Option<String>,
        kind: Option<String>,
    },
}

pub enum InputAction {
    NewGroup {
        parent: PathBuf,
    },
    Attach,
    MoveTo,
    MoveGroup {
        group: PathBuf,
    },
    /// Set the intent's title (the file name is kept).
    Rename,
    /// Append a `- [ ]` item under `heading`, then return focus to that pane.
    AddItem {
        heading: &'static str,
        pane: Pane,
    },
    /// Append an open question.
    AddQuestion,
    /// Append `- kind: target` under Artifacts (input is "kind: target").
    /// `kind` is used when the text does not start with `kind:`.
    AddArtifact {
        kind: &'static str,
    },
    /// Replace the intent's affected areas (comma-separated group paths;
    /// empty clears them).
    EditAreas,
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
        /// Where to return after choosing or Esc (the add view after a
        /// failure); `None` returns to the browser.
        back: Option<Box<Screen>>,
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
        /// The kind the reply's first line named.
        kind: Option<String>,
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
    /// Adding: is it a bug, a refactor, a feature or an idea? `group` is the
    /// home group when it is known (the tree's selection), else `None`.
    KindPick {
        group: Option<PathBuf>,
    },
    /// Adding by conversation, in its own view.
    Interview(Box<InterviewScreen>),
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
    /// Agents Herdr still lists, sent longest ago first; then the rest by Updated.
    LongestRunning,
    /// Newest created first.
    Captured,
    /// Oldest created first.
    CreatedAsc,
    /// A to Z.
    Title,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            SortOrder::Updated => SortOrder::Progress,
            SortOrder::Progress => SortOrder::LongestRunning,
            SortOrder::LongestRunning => SortOrder::Captured,
            SortOrder::Captured => SortOrder::CreatedAsc,
            SortOrder::CreatedAsc => SortOrder::Title,
            SortOrder::Title => SortOrder::Updated,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SortOrder::Updated => "Updated",
            SortOrder::Progress => "Progress",
            SortOrder::LongestRunning => "Longest running",
            SortOrder::Captured => "Captured",
            SortOrder::CreatedAsc => "Created Asc",
            SortOrder::Title => "Title",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        [
            SortOrder::Updated,
            SortOrder::Progress,
            SortOrder::LongestRunning,
            SortOrder::Captured,
            SortOrder::CreatedAsc,
            SortOrder::Title,
        ]
        .into_iter()
        .find(|o| o.label() == s)
    }
}

/// The list's filter, cycled with `f`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    All,
    /// Everything not yet complete, drafts included.
    InProgress,
    /// Sent with every acceptance criterion ticked.
    Completed,
}

impl ListFilter {
    pub fn next(self) -> Self {
        match self {
            ListFilter::All => ListFilter::InProgress,
            ListFilter::InProgress => ListFilter::Completed,
            ListFilter::Completed => ListFilter::All,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ListFilter::All => "All",
            ListFilter::InProgress => "In progress",
            ListFilter::Completed => "Completed",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        [
            ListFilter::All,
            ListFilter::InProgress,
            ListFilter::Completed,
        ]
        .into_iter()
        .find(|f| f.label() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub depth: usize,
    pub label: String,
    pub is_group: bool,
    pub questions: usize,
    /// Shown here because it affects this group; it lives elsewhere.
    pub reference: bool,
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
    /// The detail section open under the description.
    pub open: &'static str,
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
    /// Every intent's title in list order.
    pub list_titles: Vec<String>,
    pub list_selected: usize,
    /// The list's sort order label.
    pub sort: &'static str,
    /// The list's filter label.
    pub filter: &'static str,
    /// The add-by-conversation screen, when it shows.
    pub interview: Option<InterviewView>,
}

pub struct App {
    /// Capture mode (the `add --popup` pane): quit once back on the browser.
    pub exit_on_browse: bool,
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
    /// The detail section open under the description; follows focus into a
    /// section and stays when focus leaves for the tree or the description.
    open: Pane,
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
    prs_selected: usize,
    demos_selected: usize,
    /// PR titles from `gh`, keyed like `pr_states`.
    pr_titles: HashMap<(PathBuf, String), String>,
    /// Artifacts found in the selected intent's repo, cached per path.
    found_cache: Option<(PathBuf, Vec<crate::store::Artifact>)>,
    pub layout: Layout,
    pub sort: SortOrder,
    pub filter: ListFilter,
    /// Every intent in the tree that passes the filter, in list order
    /// (all groups, collapsed or not).
    list: Vec<Intent>,
    /// How many intents there are before filtering.
    list_total: usize,
    list_selected: usize,
    /// Plan tasks per intent path, as last reported by the poller.
    plan_evidence: HashMap<PathBuf, Vec<crate::progress::PlanTasks>>,
    /// Pull requests found mentioning each intent's spec or plan files.
    found_prs: HashMap<PathBuf, crate::progress::FoundByName>,
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
    /// Tests pin the clock here; `None` means the real time.
    fixed_now: Option<chrono::DateTime<chrono::Local>>,
    /// Reconciles running: per intent, the merged set it covers, the
    /// agent turn, and when it started.
    reconcile_jobs: HashMap<PathBuf, (Vec<u64>, Turn, std::time::Instant)>,
    /// How long a reconcile may run before it is stopped and reported.
    reconcile_limit: std::time::Duration,
    /// Merged sets already started this session, so a failure is not retried in a loop.
    reconcile_tried: HashSet<(PathBuf, Vec<u64>)>,
    /// Why the last reconcile of an intent failed; shown while it is
    /// selected until `r` retries it.
    reconcile_failed: HashMap<PathBuf, String>,
    /// The area last drawn, so mouse clicks can be mapped to what is there.
    last_area: std::cell::Cell<Rect>,
    /// Where each list card was last drawn: `(list index, first line, end line)`.
    card_hits: std::cell::RefCell<Vec<(usize, u16, u16)>>,
}

fn pane_name(p: Pane) -> &'static str {
    match p {
        Pane::Tree => "tree",
        Pane::Description => "description",
        Pane::Preconditions => "preconditions",
        Pane::Criteria => "criteria",
        Pane::Questions => "questions",
        Pane::Artifacts => "artifacts",
        Pane::Demos => "demos",
        Pane::PullRequests => "pull requests",
    }
}

impl App {
    pub fn new(cfg: Config, dirs: Dirs) -> anyhow::Result<Self> {
        let store = Store::open(&cfg.root)?;
        let tree = store.scan()?.scoped(cfg.scope.as_deref());
        let mut app = Self {
            exit_on_browse: false,
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
            open: Pane::Criteria,
            description_scroll: 0,
            binary_stamp: binary_mtime(),
            newer_build: false,
            ticks: 0,
            preconditions_selected: 0,
            criteria_selected: 0,
            question_selected: 0,
            artifacts_selected: 0,
            prs_selected: 0,
            demos_selected: 0,
            pr_titles: HashMap::new(),
            found_cache: None,
            layout: Layout::Tree,
            sort: SortOrder::Updated,
            filter: ListFilter::All,
            list: Vec::new(),
            list_total: 0,
            list_selected: 0,
            plan_evidence: HashMap::new(),
            pr_states: HashMap::new(),
            found_prs: HashMap::new(),
            agents: None,
            agents_unavailable: false,
            poller: None,
            poll_ticks: 0,
            fixed_now: None,
            reconcile_jobs: HashMap::new(),
            reconcile_limit: std::time::Duration::from_secs(600),
            reconcile_tried: HashSet::new(),
            reconcile_failed: HashMap::new(),
            last_area: std::cell::Cell::new(Rect::default()),
            card_hits: std::cell::RefCell::new(Vec::new()),
        };
        let last = crate::config::last_used(&app.dirs, &app.cfg);
        if let Some(l) = last.layout.as_deref().and_then(Layout::parse) {
            app.layout = l;
        }
        if let Some(o) = last.sort.as_deref().and_then(SortOrder::parse) {
            app.sort = o;
        }
        if let Some(f) = last.filter.as_deref().and_then(ListFilter::parse) {
            app.filter = f;
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
        // intents by the groups they affect, keyed by group folder
        fn index(g: &Group, root: &Path, out: &mut HashMap<PathBuf, Vec<Intent>>) {
            for it in &g.intents {
                for area in &it.fm.areas {
                    let area = area.trim().trim_matches('/');
                    if area.is_empty() {
                        continue;
                    }
                    let dir = root.join(area);
                    if Some(dir.as_path()) != it.path.parent() {
                        out.entry(dir).or_default().push(it.clone());
                    }
                }
            }
            for sub in &g.groups {
                index(sub, root, out);
            }
        }
        let mut affecting: HashMap<PathBuf, Vec<Intent>> = HashMap::new();
        index(&self.tree, self.store.root(), &mut affecting);
        fn count_refs(g: &Group, affecting: &HashMap<PathBuf, Vec<Intent>>) -> usize {
            affecting.get(&g.path).map_or(0, Vec::len)
                + g.groups
                    .iter()
                    .map(|s| count_refs(s, affecting))
                    .sum::<usize>()
        }
        fn walk(
            g: &Group,
            depth: usize,
            collapsed: &mut HashSet<PathBuf>,
            seen: &mut HashSet<PathBuf>,
            affecting: &HashMap<PathBuf, Vec<Intent>>,
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
                    refs: count_refs(sub, affecting),
                });
                if !collapsed.contains(&sub.path) {
                    walk(sub, depth + 1, collapsed, seen, affecting, rows);
                }
            }
            for it in &g.intents {
                rows.push(Row::Intent {
                    intent: it.clone(),
                    depth,
                    reference: None,
                });
            }
            for it in affecting.get(&g.path).into_iter().flatten() {
                rows.push(Row::Intent {
                    intent: it.clone(),
                    depth,
                    reference: Some(g.path.clone()),
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
            &affecting,
            &mut rows,
        );
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.rebuild_list();
    }

    // ---- list -------------------------------------------------------------

    /// Every intent in the tree, sorted by `self.sort`, keeping the list
    /// selection on the same intent when it still exists.
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
        self.list_total = list.len();
        let complete = |i: &Intent| {
            crate::progress::progress(i, &self.evidence_for(i)).is_some_and(|p| p.complete)
        };
        match self.filter {
            ListFilter::All => {}
            ListFilter::InProgress => list.retain(|i| !complete(i)),
            ListFilter::Completed => list.retain(complete),
        }
        match self.sort {
            SortOrder::Updated => list.sort_by_key(|i| std::cmp::Reverse(crate::store::stamp(i))),
            SortOrder::Captured => list.sort_by_key(|i| std::cmp::Reverse(i.fm.created)),
            SortOrder::CreatedAsc => list.sort_by_key(|i| i.fm.created),
            SortOrder::LongestRunning => {
                // running = sent, and Herdr still lists its agent
                let started = |i: &Intent| {
                    let sent = i.fm.sent_to.as_ref()?;
                    let listed = i.fm.state == State::Sent
                        && self
                            .agents
                            .as_ref()
                            .is_some_and(|a| a.contains_key(&sent.agent));
                    listed.then_some(sent.at)
                };
                list.sort_by(|a, b| match (started(a), started(b)) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => crate::store::stamp(b).cmp(&crate::store::stamp(a)),
                });
            }
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

    /// Switch layout, carrying the selected intent across.
    fn set_layout(&mut self, layout: Layout) {
        match layout {
            Layout::List => {
                // the tree's intent, or the first one under a selected group
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
                // no card for the tree's selection (an empty group): the first
                self.list_selected = target
                    .and_then(|p| self.list.iter().position(|i| i.path == p))
                    .unwrap_or(0);
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
            // mark it seen too: a group never drawn before would otherwise be
            // collapsed again by rebuild_rows' first-sight rule
            self.seen_groups.insert(d.to_path_buf());
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
        // add pull requests found by spec/plan name that no pr: line names
        let mut prs: Vec<(u64, PrState)> = prs;
        let found = self.found_prs.get(&it.path);
        for f in found.into_iter().flatten().flat_map(|(_, prs)| prs) {
            if !prs.iter().any(|(m, _)| *m == f.number) {
                prs.push((f.number, f.state));
            }
        }
        // each plan learns the state of the pull requests found by its name
        let mut plans = self
            .plan_evidence
            .get(&it.path)
            .cloned()
            .unwrap_or_default();
        let plan_names: Vec<String> = crate::store::artifacts(&it.body)
            .into_iter()
            .filter(|a| a.kind.eq_ignore_ascii_case("plan"))
            .map(|a| {
                Path::new(a.target.trim())
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect();
        for (plan, name) in plans.iter_mut().zip(&plan_names) {
            let mine: Vec<PrState> = found
                .into_iter()
                .flatten()
                .filter(|(q, _)| q == name)
                .flat_map(|(_, prs)| prs.iter().map(|f| f.state))
                .collect();
            plan.pr = if mine
                .iter()
                .any(|s| matches!(s, PrState::Open | PrState::Unknown))
            {
                Some(PrState::Open)
            } else if mine.contains(&PrState::Merged) {
                Some(PrState::Merged)
            } else {
                None
            };
        }
        crate::progress::Evidence { plans, prs }
    }

    fn poller(&mut self) -> &mut crate::progress::Poller {
        let env = self.herdr_env.clone();
        self.poller
            .get_or_insert_with(|| crate::progress::Poller::new(env))
    }

    /// Background requests started and not yet reported, reconciles included.
    pub fn polls_in_flight(&self) -> usize {
        self.poller.as_ref().map_or(0, |p| p.in_flight()) + self.reconcile_jobs.len()
    }

    /// Every intent in the tree, whatever the list's filter.
    fn all_intents(&self) -> Vec<Intent> {
        fn all(g: &Group, out: &mut Vec<Intent>) {
            out.extend(g.intents.iter().cloned());
            for sub in &g.groups {
                all(sub, out);
            }
        }
        let mut out = Vec::new();
        all(&self.tree, &mut out);
        out
    }

    /// Start a reconcile for every sent intent whose pull requests have all
    /// merged since its last reconcile.
    fn check_reconciles(&mut self) {
        fn all(g: &Group, out: &mut Vec<Intent>) {
            out.extend(g.intents.iter().cloned());
            for sub in &g.groups {
                all(sub, out);
            }
        }
        let mut intents = Vec::new();
        all(&self.tree, &mut intents);
        for it in intents {
            if it.fm.repo.is_none() || self.reconcile_jobs.contains_key(&it.path) {
                continue;
            }
            // wait for the pull-request search (it suggests the areas the work touched)
            let searched = crate::store::artifacts(&it.body).iter().any(|a| {
                a.kind.eq_ignore_ascii_case("spec") || a.kind.eq_ignore_ascii_case("plan")
            });
            if searched && !self.found_prs.contains_key(&it.path) {
                continue;
            }
            let Some(merged) = crate::reconcile::needs_reconcile(&it, &self.evidence_for(&it))
            else {
                continue;
            };
            if self
                .reconcile_tried
                .insert((it.path.clone(), merged.clone()))
            {
                // the agent groups criteria by area: give it the suggested areas too
                let mut it = it;
                let suggested = self.suggested_areas(&it);
                it.fm.areas.extend(suggested);
                self.poller().request_reconcile(it, merged);
            }
        }
    }

    /// Ask the refinement agent (the last one used, else the configured one)
    /// for the reconciled text.
    fn start_reconcile(&mut self, path: PathBuf, merged: Vec<u64>, prompt: String) {
        let last = crate::config::last_used(&self.dirs, &self.cfg);
        let Some(mut spec) = self.cfg.providers.get(&last.provider).cloned() else {
            self.message = Some(format!("reconcile: unknown provider `{}`", last.provider));
            return;
        };
        if let Some(m) = &last.model {
            spec.extra.extend([spec.model_flag.clone(), m.clone()]);
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let cwd = polish_cwd(self.store.root(), &stem);
        if std::fs::create_dir_all(&cwd).is_err() {
            return;
        }
        match Turn::start(&spec, &prompt, None, &cwd, &self.provider_env) {
            Ok(turn) => {
                self.reconcile_jobs
                    .insert(path, (merged, turn, std::time::Instant::now()));
            }
            Err(e) => self.message = Some(format!("reconcile could not start: {e:#}")),
        }
    }

    /// Apply finished reconciles: the intent gets the agent's rewrite, the
    /// old version is kept for undo, and it is marked unread.
    fn poll_reconciles(&mut self) {
        let mut done: Vec<(PathBuf, Vec<u64>, Result<Reply, String>)> = Vec::new();
        let limit = self.reconcile_limit;
        for (path, (merged, turn, started)) in &mut self.reconcile_jobs {
            if started.elapsed() > limit {
                // the agent is stuck: stop its process group and say so
                turn.cancel();
                done.push((
                    path.clone(),
                    merged.clone(),
                    Err(format!(
                        "the agent did not finish within {} minutes",
                        limit.as_secs().div_ceil(60)
                    )),
                ));
                continue;
            }
            while let Some(msg) = turn.try_recv() {
                match msg {
                    TurnMsg::Finished(reply) => {
                        done.push((path.clone(), merged.clone(), Ok(reply)))
                    }
                    TurnMsg::Failed(e) => done.push((path.clone(), merged.clone(), Err(e))),
                    _ => {}
                }
            }
        }
        for (path, merged, result) in done {
            self.reconcile_jobs.remove(&path);
            // Ok(title) when the rewrite was applied, Err(reason) otherwise
            let outcome: Result<String, String> = match result {
                Ok(reply) => match reply.proposal {
                    Some(prop) => {
                        let body = merge_questions(&prop, &reply.questions);
                        crate::reconcile::apply(&path, &body, &merged, self.store.root())
                            .map(|it| it.fm.title)
                            .map_err(|e| format!("reconcile not saved: {e:#}"))
                    }
                    None => Err("reconcile: the agent returned no proposal".into()),
                },
                Err(e) => Err(format!("reconcile failed: {e}")),
            };
            match outcome {
                Ok(title) => {
                    self.reconcile_failed.remove(&path);
                    self.message = Some(format!(
                        "reconciled `{title}` with what shipped (* until you read it, u undoes)"
                    ));
                    self.reload_tree();
                }
                Err(why) => {
                    // kept until r retries it, shown while the intent is selected
                    self.message = Some(why.clone());
                    self.reconcile_failed.insert(path, why);
                }
            }
        }
    }

    /// `r`: forget failed reconciles so the next check starts them again.
    fn retry_reconciles(&mut self) {
        let failed: Vec<PathBuf> = self.reconcile_failed.drain().map(|(p, _)| p).collect();
        self.reconcile_tried.retain(|(p, _)| !failed.contains(p));
    }

    /// The status line for the selected intent when nothing else is said:
    /// a reconcile that failed, and how to retry it.
    fn reconcile_note(&self) -> Option<String> {
        let it = self.selected_intent()?;
        self.reconcile_failed
            .get(&it.path)
            .map(|why| format!("{why} · r retries"))
    }

    /// Opening an intent (it shows in the detail panes) reads it.
    fn mark_read(&mut self) {
        if self.layout != Layout::Tree || !matches!(self.screen, Screen::Browse) {
            return;
        }
        let Some(path) = self
            .selected_intent()
            .filter(|it| it.fm.unread)
            .map(|it| it.path.clone())
        else {
            return;
        };
        if Store::update_meta(&path, |fm| fm.unread = false).is_ok() {
            self.reload_tree();
        }
    }

    /// `u`: put back the version before the last reconcile.
    fn undo_reconcile(&mut self) {
        let Some(path) = self.selected_intent().map(|it| it.path.clone()) else {
            return;
        };
        self.message = Some(match crate::reconcile::undo(&path) {
            Ok(true) => {
                self.reload_tree();
                "restored the version before the reconcile".into()
            }
            Ok(false) => "nothing to undo: no reconcile backup".into(),
            Err(e) => format!("undo failed: {e:#}"),
        });
    }

    /// Ask for agent states and, when `all`, every sent intent's plans and
    /// PR states.
    fn request_polls(&mut self, all: bool) {
        if self.layout == Layout::List {
            self.poller().request_agents();
        }
        if !all {
            return;
        }
        let sent: Vec<Intent> = self
            .all_intents()
            .into_iter()
            .filter(|i| i.fm.state == State::Sent)
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

    /// Search GitHub for pull requests mentioning each sent intent's spec
    /// and plan file names (search is rate-limited, so this runs on entry, on
    /// `r` and about every ten minutes).
    fn request_found_prs(&mut self) {
        let sent: Vec<(PathBuf, PathBuf, Vec<String>)> = self
            .all_intents()
            .iter()
            .filter(|i| i.fm.state == State::Sent)
            .filter_map(|i| {
                let repo = i.fm.repo.clone()?;
                let names: Vec<String> = crate::store::artifacts(&i.body)
                    .into_iter()
                    .filter(|a| {
                        a.kind.eq_ignore_ascii_case("spec") || a.kind.eq_ignore_ascii_case("plan")
                    })
                    .filter_map(|a| {
                        Path::new(a.target.trim())
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                    })
                    .collect();
                (!names.is_empty()).then(|| (i.path.clone(), repo, names))
            })
            .collect();
        for (path, repo, names) in sent {
            self.poller().request_found_prs(path, repo, names);
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
                    title,
                } => {
                    if let Some(t) = title {
                        self.pr_titles.insert((repo.clone(), target.clone()), t);
                    }
                    self.pr_states.insert((repo, target), state);
                }
                crate::progress::Update::Plans { path, plans } => {
                    self.plan_evidence.insert(path, plans);
                }
                crate::progress::Update::FoundPrs { path, by_name } => {
                    self.found_prs.insert(path, by_name);
                }
                crate::progress::Update::Reconcile {
                    path,
                    merged,
                    prompt,
                } => self.start_reconcile(path, merged, prompt),
            }
        }
        if matches!(self.sort, SortOrder::Progress | SortOrder::LongestRunning) {
            self.rebuild_list();
        }
        self.check_reconciles();
    }

    /// `s`: refuse with the reason, or open the hand-off picker.
    /// A page of the list: the cards on screen at the last draw, less one so
    /// the last card seen stays on screen (and a card cut off at the bottom
    /// is not skipped); one before anything has been drawn.
    fn list_page(&self) -> usize {
        self.card_hits.borrow().len().saturating_sub(1).max(1)
    }

    fn start_send(&mut self) {
        // the picker is drawn beside the tree: point the tree at this intent
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

    /// `g`: focus the agent the selected intent was sent to.
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

    /// Mouse in the browser: a click selects a tree row or a list card; a
    /// click on the selected group opens or closes it, on the selected card
    /// opens it in the tree; the wheel moves the selection.
    pub fn mouse(&mut self, ev: crossterm::event::MouseEvent) {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind as K};
        if !matches!(self.screen, Screen::Browse) {
            return;
        }
        let step = |app: &mut App, c: char| {
            app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        };
        let area = self.last_area.get();
        match (self.layout, ev.kind) {
            (Layout::List, K::Down(MouseButton::Left)) => {
                let hit = self
                    .card_hits
                    .borrow()
                    .iter()
                    .find(|(_, top, end)| ev.row >= *top && ev.row < *end)
                    .map(|(i, _, _)| *i);
                match hit {
                    Some(i) if i == self.list_selected => self.set_layout(Layout::Tree),
                    Some(i) => self.list_selected = i,
                    None => {}
                }
            }
            (Layout::Tree, K::Down(MouseButton::Left)) => {
                let (left, _, _) = ui::split(area);
                if ev.column >= left.x + left.width || ev.row < left.y {
                    return;
                }
                // the same scroll the tree was drawn with
                let visible = left.height as usize;
                let scroll = if visible > 0 && self.selected >= visible {
                    self.selected + 1 - visible
                } else {
                    0
                };
                let i = scroll + usize::from(ev.row - left.y);
                if i >= self.rows.len() {
                    return;
                }
                self.focus = Pane::Tree;
                if i == self.selected
                    && let Some(Row::Group { path, .. }) = self.rows.get(i)
                {
                    let path = path.clone();
                    if !self.collapsed.remove(&path) {
                        self.collapsed.insert(path);
                    }
                    self.rebuild_rows();
                } else {
                    self.selected = i;
                }
            }
            (_, K::ScrollDown) => step(self, 'j'),
            (_, K::ScrollUp) => step(self, 'k'),
            _ => {}
        }
        self.refresh_found();
        self.mark_read();
    }

    /// Every group's path under the root, e.g. `skoolscout/org/candidates`.
    fn group_paths(&self) -> Vec<String> {
        fn walk(g: &Group, root: &Path, out: &mut Vec<String>) {
            for sub in &g.groups {
                if let Ok(rel) = sub.path.strip_prefix(root) {
                    out.push(rel.display().to_string());
                }
                walk(sub, root, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.tree, self.store.root(), &mut out);
        out
    }

    /// Areas the intent's pull requests were built in (workbrokr branch
    /// names) that it does not list yet: a branch area maps to the one group
    /// whose path ends with it; ambiguous or unknown areas are skipped.
    fn suggested_areas(&self, it: &Intent) -> Vec<String> {
        let Some(found) = self.found_prs.get(&it.path) else {
            return Vec::new();
        };
        let home = it
            .path
            .parent()
            .and_then(|d| d.strip_prefix(self.store.root()).ok())
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let groups = self.group_paths();
        let mut out: Vec<String> = Vec::new();
        for f in found.iter().flat_map(|(_, prs)| prs) {
            let Some(area) = crate::progress::branch_area(&f.branch) else {
                continue;
            };
            let matches: Vec<&String> = groups
                .iter()
                .filter(|g| **g == area || g.ends_with(&format!("/{area}")))
                .collect();
            if let [only] = matches.as_slice()
                && **only != home
                && !it
                    .fm
                    .areas
                    .iter()
                    .any(|a| a.trim().trim_matches('/') == only.as_str())
                && !out.contains(only)
            {
                out.push((*only).clone());
            }
        }
        out
    }

    /// `+`: edit the selected intent's affected areas; suggestions from
    /// its pull requests are already added, so Enter confirms them.
    fn open_areas_input(&mut self) {
        if let Some(it) = self.selected_intent().cloned() {
            let mut areas = it.fm.areas.clone();
            areas.extend(self.suggested_areas(&it));
            self.screen = Screen::Input {
                label: "affected areas (group paths, comma-separated; empty clears)".into(),
                text: areas.join(", "),
                action: InputAction::EditAreas,
            };
        }
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
            KeyCode::PageDown => {
                self.list_selected = (self.list_selected + self.list_page()).min(last)
            }
            KeyCode::PageUp => {
                self.list_selected = self.list_selected.saturating_sub(self.list_page())
            }
            KeyCode::Char('v') => self.set_layout(Layout::Tree),
            KeyCode::Enter => self.set_layout(Layout::Tree),
            KeyCode::Char('S') => {
                self.sort = self.sort.next();
                self.rebuild_list();
                let _ =
                    crate::config::save_view(&self.dirs, self.layout.label(), self.sort.label());
            }
            KeyCode::Char('f') => {
                self.filter = self.filter.next();
                self.rebuild_list();
                let _ = crate::config::save_view_with(
                    &self.dirs,
                    self.layout.label(),
                    self.sort.label(),
                    Some(self.filter.label()),
                );
            }
            KeyCode::Char('r') => {
                self.retry_reconciles();
                self.rescan();
            }
            KeyCode::Char('s') => self.start_send(),
            KeyCode::Char('g') => self.go_to_agent(),
            KeyCode::Char('+') => self.open_areas_input(),
            KeyCode::Char('u') => self.undo_reconcile(),
            // the card view says nothing about where an intent belongs
            KeyCode::Char('a') => self.screen = Screen::KindPick { group: None },
            KeyCode::Char('D') => self.ask_trash(),
            _ => {}
        }
        Flow::Continue
    }

    pub fn rescan(&mut self) {
        self.found_cache = None;
        self.reload_tree();
        self.refresh_found();
        self.poll_ticks = 0;
    }

    /// Re-read every intent from disk, keeping both selections.
    fn reload_tree(&mut self) {
        let keep = match self.layout {
            Layout::Tree => self.selected_path(),
            Layout::List => self.list.get(self.list_selected).map(|i| i.path.clone()),
        };
        match self.scan() {
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

    /// The intent under the cursor: the list's in the list layout, the
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

    /// The intent tree as this workspace sees it.
    fn scan(&self) -> anyhow::Result<Group> {
        Ok(self.store.scan()?.scoped(self.cfg.scope.as_deref()))
    }

    /// Where "the top level" is: the workspace's group, or the root.
    fn home_dir(&self) -> PathBuf {
        match &self.cfg.scope {
            Some(scope) => self.store.root().join(self.store.scope_group(scope)),
            None => self.store.root().to_path_buf(),
        }
    }

    /// Group directory the selection belongs to (the group itself, or the intent's parent).
    fn selected_group_dir(&self) -> PathBuf {
        if self.layout == Layout::List {
            // the card view adds next to the selected card, else at the top
            return self
                .selected_intent()
                .and_then(|it| it.path.parent())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.home_dir());
        }
        match self.rows.get(self.selected) {
            Some(Row::Group { path, .. }) => path.clone(),
            Some(Row::Intent { intent, .. }) => intent
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.home_dir()),
            Some(Row::Broken { path, .. }) => path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.home_dir()),
            None => self.home_dir(),
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
        self.prs_selected = 0;
        self.demos_selected = 0;
    }

    fn select_path(&mut self, p: &Path) {
        if let Some(i) = self.rows.iter().position(|r| row_path(r) == p) {
            self.selected = i;
        }
    }

    /// Fill the found-artifacts cache for the selected intent (scans its
    /// repo's `docs/superpowers/{specs,plans}` and `docs/adr`). Cached per path,
    /// dropped on rescan, so a key press costs nothing once computed.
    fn refresh_found(&mut self) {
        // only the detail view (tree layout) shows found artifacts
        if self.layout == Layout::List {
            return;
        }
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
                let mut found = crate::store::discover_artifacts(repo, &it.fm.title, &already);
                found.extend(crate::demos::discover_demos(repo, &it.fm.title, &already));
                found
            }
            _ => Vec::new(),
        };
        self.found_cache = Some((it.path, found));
    }

    /// Pull requests for the detail view: the linked ones with what `gh`
    /// said about them, then those the repo search found that no `pr:`
    /// line names.
    fn pr_rows(&self, it: &Intent) -> Vec<ui::PrRow> {
        use crate::progress::{PrState, pr_number};
        let key = |t: &str| {
            it.fm
                .repo
                .as_ref()
                .map(|r| (r.clone(), t.trim().to_string()))
        };
        let found: Vec<&crate::progress::FoundPr> = self
            .found_prs
            .get(&it.path)
            .into_iter()
            .flatten()
            .flat_map(|(_, prs)| prs)
            .collect();
        // until `gh` answers for a linked PR, the search's answer stands in
        // (a found PR just linked keeps its state and title)
        let searched = |target: &str| {
            let n = pr_number(target)?;
            found.iter().find(|f| f.number == n).copied()
        };
        let mut rows: Vec<ui::PrRow> = crate::store::artifacts(&it.body)
            .into_iter()
            .filter(|a| ui::list_section(&a.kind) == Pane::PullRequests)
            .map(|a| {
                let k = key(&a.target);
                let s = searched(&a.target);
                ui::PrRow {
                    state: k
                        .as_ref()
                        .and_then(|k| self.pr_states.get(k))
                        .copied()
                        .filter(|st| *st != PrState::Unknown)
                        .or(s.map(|f| f.state))
                        .unwrap_or(PrState::Unknown),
                    title: k
                        .as_ref()
                        .and_then(|k| self.pr_titles.get(k))
                        .cloned()
                        .or_else(|| s.map(|f| f.title.clone()).filter(|t| !t.trim().is_empty())),
                    target: a.target.trim().to_string(),
                    found: false,
                }
            })
            .collect();
        let mut seen: Vec<u64> = rows.iter().filter_map(|r| pr_number(&r.target)).collect();
        for f in found {
            if seen.contains(&f.number) {
                continue;
            }
            seen.push(f.number);
            rows.push(ui::PrRow {
                target: format!("#{}", f.number),
                state: f.state,
                title: (!f.title.trim().is_empty()).then(|| f.title.clone()),
                found: true,
            });
        }
        rows
    }

    /// A list section's rows: (linked, found), in the order the section
    /// draws them.
    fn section_lists(
        &self,
        it: &Intent,
        pane: Pane,
    ) -> (Vec<crate::store::Artifact>, Vec<crate::store::Artifact>) {
        let linked = crate::store::artifacts(&it.body)
            .into_iter()
            .filter(|a| ui::list_section(&a.kind) == pane)
            .collect();
        let found = if pane == Pane::PullRequests {
            self.pr_rows(it)
                .into_iter()
                .filter(|r| r.found)
                .map(|r| crate::store::Artifact {
                    kind: "pr".into(),
                    target: r.target,
                })
                .collect()
        } else {
            self.found_artifacts()
                .iter()
                .filter(|a| ui::list_section(&a.kind) == pane)
                .cloned()
                .collect()
        };
        (linked, found)
    }

    /// The selection of a list section (Artifacts, Demos, Pull requests).
    fn list_selected(&mut self, pane: Pane) -> &mut usize {
        match pane {
            Pane::PullRequests => &mut self.prs_selected,
            Pane::Demos => &mut self.demos_selected,
            _ => &mut self.artifacts_selected,
        }
    }

    /// The cached found artifacts for the selected intent.
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
        if self.exit_on_browse && matches!(self.screen, Screen::Browse) {
            return Flow::Quit;
        }
        // the accordion: a section that gets focus opens, and stays open
        // while focus is on the tree or the description
        if matches!(
            self.focus,
            Pane::Preconditions
                | Pane::Criteria
                | Pane::Demos
                | Pane::Questions
                | Pane::PullRequests
                | Pane::Artifacts
        ) {
            self.open = self.focus;
        }
        self.refresh_found();
        self.mark_read();
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
                back,
            } => self.key_agent_pick(step, items, selected, back, k),
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
                kind,
                scroll,
            } => self.key_diff(proposal, questions, title, kind, scroll, k),
            Screen::SendPick { handoffs, selected } => self.key_send_pick(handoffs, selected, k),
            Screen::SendConfirm {
                handoff,
                provider,
                repo,
                prompt,
                scroll,
            } => self.key_send_confirm(handoff, provider, repo, prompt, scroll, k),
            Screen::Prompt { question, pending } => self.key_prompt(question, pending, k),
            Screen::KindPick { group } => self.key_kind_pick(group, k),
            Screen::Interview(st) => self.key_interview(st, k),
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
                    Pane::Criteria => Pane::Demos,
                    Pane::Demos => Pane::Questions,
                    Pane::Questions => Pane::PullRequests,
                    Pane::PullRequests => Pane::Artifacts,
                    Pane::Artifacts => Pane::Tree,
                };
                return Flow::Continue;
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Pane::Tree => Pane::Artifacts,
                    Pane::Artifacts => Pane::PullRequests,
                    Pane::PullRequests => Pane::Questions,
                    Pane::Questions => Pane::Demos,
                    Pane::Demos => Pane::Criteria,
                    Pane::Criteria => Pane::Preconditions,
                    Pane::Preconditions => Pane::Description,
                    Pane::Description => Pane::Tree,
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
                let group = (!self.rows.is_empty()).then(|| self.selected_group_dir());
                self.screen = Screen::KindPick { group };
            }
            KeyCode::Char('D') => self.ask_trash(),
            KeyCode::Char('n') => {
                let parent = self.selected_group_dir();
                self.screen = Screen::Input {
                    label: "new group".into(),
                    text: String::new(),
                    action: InputAction::NewGroup { parent },
                };
            }
            KeyCode::Char('m') => self.open_agent_pick(None),
            KeyCode::Char('M') => match self.rows.get(self.selected) {
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
            KeyCode::Char('R') | KeyCode::Char('U')
                if self
                    .selected_intent()
                    .is_some_and(|i| i.fm.state == State::Sent) =>
            {
                self.message = Some(
                    "already sent: R and U only apply before sending (its progress tracks the work now)"
                        .into(),
                );
            }
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
                    self.polish(PendingChat {
                        intent: it,
                        selection: None,
                        from_editor: false,
                    });
                }
            }
            KeyCode::Char('s') => self.start_send(),
            KeyCode::Char('+') => self.open_areas_input(),
            KeyCode::Char('u') => self.undo_reconcile(),
            KeyCode::Char('t') => {
                if let Some(it) = self.selected_intent() {
                    self.screen = Screen::Input {
                        label: title_label(&it.fm.kind),
                        text: it.fm.title.clone(),
                        action: InputAction::Rename,
                    };
                }
            }
            KeyCode::Char('r') => {
                self.retry_reconciles();
                self.rescan();
            }
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
                    InputAction::AddArtifact { kind: "link" },
                ),
                Pane::PullRequests => (
                    "new pull request: #number or URL",
                    InputAction::AddArtifact { kind: "pr" },
                ),
                Pane::Demos => (
                    "new demo: story-id (criteria: 1, 4)",
                    InputAction::AddArtifact { kind: "demo" },
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
                    // acknowledge a criterion someone else added: the selected one, or all
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let which: Vec<usize> = if k.code == KeyCode::Char('Y') {
                            (0..n).collect()
                        } else {
                            vec![self.criteria_selected]
                        };
                        match crate::store::acknowledge_criteria(&it.path, &which) {
                            Ok(_) => self.reload_tree(),
                            Err(e) => self.message = Some(format!("not saved: {e:#}")),
                        }
                    }
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
            Pane::Artifacts | Pane::Demos | Pane::PullRequests => {
                let pane = self.focus;
                let (linked, found) = self.section_lists(&it, pane);
                let total = linked.len() + found.len();
                let selected = *self.list_selected(pane);
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
                        *self.list_selected(pane) = (selected + 1).min(total.saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        *self.list_selected(pane) = selected.saturating_sub(1)
                    }
                    KeyCode::Enter | KeyCode::Char('o') => {
                        if let Some((a, _)) = pick(selected) {
                            self.open_artifact(&it, &a, k.code == KeyCode::Char('o'));
                        }
                    }
                    KeyCode::Char('L') => match pick(selected) {
                        Some((a, true)) => {
                            self.edit_selected_body(|body| {
                                crate::store::add_artifact(body, &a.kind, &a.target)
                            });
                            self.found_cache = None;
                            *self.list_selected(pane) = linked.len();
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
        if a.kind.eq_ignore_ascii_case("demo") {
            self.open_demo(it, &a.target, external);
            return;
        }
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

    /// A demo line: Enter shows its app-demos story in the reader; `o` plays
    /// the newest recorded video, else opens the demos site at the story.
    fn open_demo(&mut self, it: &Intent, target: &str, external: bool) {
        let (id, criteria) = crate::demos::demo_target(target);
        if !crate::demos::safe_story_id(&id) {
            self.message = Some(format!("`{id}` is not a story id (letters, digits, . _ -)"));
            return;
        }
        let roots: Vec<PathBuf> = it
            .fm
            .repo
            .as_deref()
            .filter(|r| r.is_dir())
            .map(crate::send::worktrees)
            .unwrap_or_default();
        if external {
            let arg = crate::demos::video_for(&roots, &id)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| crate::demos::site_url(&id));
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            self.message = Some(
                match std::process::Command::new(opener)
                    .arg(&arg)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => format!("opened {arg}"),
                    Err(e) => format!("cannot open {arg}: {e}"),
                },
            );
            return;
        }
        let Some(story) = crate::demos::find_story(&roots, &id) else {
            self.message = Some(format!(
                "no app-demos story `{id}` in the intent's repo (o opens the demos site)"
            ));
            return;
        };
        let cs = crate::store::criteria(&it.body);
        let shows: Vec<(usize, String)> = criteria
            .iter()
            .filter_map(|n| cs.get(n.checked_sub(1)?).map(|c| (*n, c.text.clone())))
            .collect();
        self.screen = Screen::Reader {
            title: story.title.clone(),
            lines: crate::markdown::render_markdown(&crate::demos::story_markdown(&story, &shows)),
            scroll: 0,
        };
    }

    /// Relative targets resolve against the intent's repo, else its own folder.
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
                self.focus = self.open;
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

    /// Reload the selected intent, transform its body, save (conflict-checked), rescan.
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
                    self.polish(PendingChat {
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
        // criteria the owner typed in this save count as seen
        let before: Vec<String> = Store::load(&it.path)
            .map(|d| {
                crate::store::criteria(&d.body)
                    .iter()
                    .map(|c| crate::store::criterion_key(&c.text))
                    .collect()
            })
            .unwrap_or_default();
        let typed: Vec<usize> = crate::store::criteria(&it.body)
            .iter()
            .enumerate()
            .filter(|(_, c)| !before.contains(&crate::store::criterion_key(&c.text)))
            .map(|(i, _)| i)
            .collect();
        let result = if force {
            Store::save_force(&mut it).map_err(SaveError::Io)
        } else {
            Store::save(&mut it)
        };
        match result {
            Ok(()) => {
                if !typed.is_empty() {
                    let _ = crate::store::acknowledge_criteria(&it.path, &typed);
                }
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

    /// `D`: asks before moving the selected intent to `.trash`. A sent intent
    /// stays: an agent is working from it.
    fn ask_trash(&mut self) {
        let Some(it) = self.selected_intent().cloned() else {
            self.message = Some("select an intent to delete".into());
            return;
        };
        if let Some(sent) = &it.fm.sent_to {
            self.message = Some(format!(
                "sent to {} (agent {}): an agent is working from it, so it stays",
                sent.handoff, sent.agent
            ));
            return;
        }
        if it.fm.state == State::Sent {
            self.message = Some("already sent: an agent is working from it, so it stays".into());
            return;
        }
        self.screen = Screen::Prompt {
            question: format!(
                "Delete \"{}\"? It moves to .trash in the intents folder. y deletes, n keeps",
                it.fm.title
            ),
            pending: Pending::Trash { path: it.path },
        };
    }

    fn trash(&mut self, path: &Path) {
        self.screen = Screen::Browse;
        let moved = Store::load(path).and_then(|it| self.store.trash(&it).map(|d| (it, d)));
        match moved {
            Ok((it, dest)) => {
                self.message = Some(format!(
                    "deleted \"{}\": moved to {}",
                    it.fm.title,
                    self.rel(&dest).display()
                ));
                self.rescan();
            }
            Err(e) => self.message = Some(format!("delete failed: {e:#}")),
        }
    }

    // ---- chat -------------------------------------------------------------

    /// Capture mode (the popup): the add conversation, closing once the intent
    /// is saved or the conversation is cancelled.
    pub fn start_capture(&mut self) {
        self.exit_on_browse = true;
        self.screen = Screen::KindPick { group: None };
    }

    fn key_kind_pick(&mut self, group: Option<PathBuf>, k: KeyEvent) -> Flow {
        let kind = match k.code {
            KeyCode::Char('b') => "issue",
            KeyCode::Char('r') => "refactor",
            KeyCode::Char('f') => "enhancement",
            KeyCode::Char('i') => "idea",
            KeyCode::Esc => {
                self.screen = Screen::Browse;
                return Flow::Continue;
            }
            _ => {
                self.screen = Screen::KindPick { group };
                return Flow::Continue;
            }
        };
        self.open_interview(group, kind);
        Flow::Continue
    }

    /// Starts adding by conversation. The agent's folder is named now: the
    /// intent's file name exists only once the agent has written the title.
    fn open_interview(&mut self, home: Option<PathBuf>, kind: &str) {
        let fallback = match &home {
            Some(h) => h.clone(),
            None if self.exit_on_browse => self.home_dir(),
            None => self.selected_group_dir(),
        };
        let home_rel = home
            .as_ref()
            .map(|h| self.rel(h).display().to_string())
            .filter(|r| !r.is_empty());
        let guide = crate::config::interview_guide(&self.dirs, kind).unwrap_or_default();
        let iv = crate::interview::Interview::new(kind, home_rel, self.group_paths(), guide);
        let last = last_used(&self.dirs, &self.cfg);
        let folder = polish_cwd(self.store.root(), &crate::interview::polish_folder_name());
        self.screen = Screen::Interview(Box::new(InterviewScreen {
            iv,
            awaiting: Awaiting::Words,
            step: Step::Restate,
            message: String::new(),
            turn: None,
            transcript: Vec::new(),
            input: String::new(),
            provider: last.provider,
            model: last.model,
            conv_id: None,
            folder,
            fallback,
            write_up: None,
            save_kind: kind.to_string(),
            scroll: 0,
        }));
    }

    /// One agent turn for the interview. `show` puts `message` in the
    /// transcript as the owner's line (a retry or an automatic write-up does not).
    fn start_interview_turn(
        &mut self,
        st: &mut InterviewScreen,
        step: Step,
        message: String,
        show: bool,
    ) {
        // recorded first: a turn that cannot start is retried as this step
        st.step = step;
        st.message = message.clone();
        let Some(mut spec) = self.cfg.providers.get(&st.provider).cloned() else {
            st.awaiting = Awaiting::Retry(format!("unknown provider `{}`", st.provider));
            return;
        };
        if let Some(m) = &st.model {
            spec.extra.extend([spec.model_flag.clone(), m.clone()]);
        }
        if let Err(e) = std::fs::create_dir_all(&st.folder) {
            st.awaiting = Awaiting::Retry(format!("cannot create {}: {e}", st.folder.display()));
            return;
        }
        let resumable = st.conv_id.is_some() && !spec.resume.is_empty();
        let prompt = st.iv.prompt(step, resumable, &message);
        match Turn::start(
            &spec,
            &prompt,
            st.conv_id.as_deref(),
            &st.folder,
            &self.provider_env,
        ) {
            Ok(turn) => {
                if show && !message.is_empty() {
                    st.transcript.push(("user".into(), message));
                }
                st.transcript.push(("agent".into(), String::new()));
                st.turn = Some(turn);
                st.awaiting = Awaiting::Turn;
                st.scroll = 0;
            }
            Err(e) => st.awaiting = Awaiting::Retry(format!("{e:#}")),
        }
    }

    /// The next question, or the write-up once five have been asked.
    fn next_question_or_write_up(&mut self, st: &mut InterviewScreen, message: String) {
        let step = if st.iv.budget_left() == 0 {
            Step::WriteUp
        } else {
            Step::Ask
        };
        self.start_interview_turn(st, step, message, true);
    }

    fn key_interview(&mut self, mut st: Box<InterviewScreen>, k: KeyEvent) -> Flow {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match (k.code, ctrl) {
            (KeyCode::PageUp, _) => st.scroll = st.scroll.saturating_add(5),
            (KeyCode::PageDown, _) => st.scroll = st.scroll.saturating_sub(5),
            // Esc stops the agent; the words stay, ready to retry or save raw
            (KeyCode::Esc, _) if st.awaiting == Awaiting::Turn => {
                if let Some(mut t) = st.turn.take() {
                    t.cancel();
                }
                if let Some(last) = st.transcript.last_mut() {
                    last.1.push_str("\n[cancelled]");
                }
                st.awaiting = Awaiting::Retry("cancelled".into());
            }
            // the agent is working: nothing else until it answers
            _ if st.awaiting == Awaiting::Turn => {}
            (KeyCode::Char('m'), false) if matches!(st.awaiting, Awaiting::Retry(_)) => {
                self.open_agent_pick(Some(Box::new(Screen::Interview(st))));
                return Flow::Continue;
            }
            (KeyCode::Char('s'), true) if matches!(st.awaiting, Awaiting::Retry(_)) => {
                self.save_interview(st, true);
                return Flow::Continue;
            }
            (KeyCode::Esc, _) => {
                self.discard_interview(st);
                return Flow::Continue;
            }
            (KeyCode::Enter, _)
                if st.awaiting == Awaiting::Review && st.input.trim().is_empty() =>
            {
                self.save_interview(st, false);
                return Flow::Continue;
            }
            (KeyCode::Tab, _) if st.awaiting == Awaiting::Review => {
                let i = KINDS.iter().position(|k| *k == st.save_kind).unwrap_or(0);
                st.save_kind = KINDS[(i + 1) % KINDS.len()].to_string();
            }
            (KeyCode::Backspace, _) => {
                st.input.pop();
            }
            (KeyCode::Char('d'), true)
                if matches!(st.awaiting, Awaiting::Confirm | Awaiting::Answer(_)) =>
            {
                // what is typed counts: an answer, or a correction
                let typed = st.input.trim().to_string();
                let message = match &st.awaiting {
                    Awaiting::Answer(q) => {
                        let answer = (!typed.is_empty()).then(|| typed.clone());
                        st.iv.qas.push(Qa {
                            question: q.clone(),
                            answer,
                        });
                        if typed.is_empty() {
                            "(skipped)".to_string()
                        } else {
                            typed
                        }
                    }
                    _ if !typed.is_empty() => {
                        st.iv.corrections.push(typed.clone());
                        typed
                    }
                    _ => String::new(),
                };
                st.input.clear();
                self.start_interview_turn(&mut st, Step::WriteUp, message, true);
            }
            (KeyCode::Enter, _) => {
                let text = st.input.trim().to_string();
                match st.awaiting.clone() {
                    Awaiting::Words if !text.is_empty() => {
                        st.input.clear();
                        st.iv.words = text.clone();
                        self.start_interview_turn(&mut st, Step::Restate, text, true);
                    }
                    Awaiting::Confirm => {
                        st.input.clear();
                        // a plain "yes" confirms; anything more is a correction
                        if text.is_empty() || crate::interview::is_confirmation(&text) {
                            let said = if text.is_empty() {
                                "That's right.".to_string()
                            } else {
                                text
                            };
                            self.next_question_or_write_up(&mut st, said);
                        } else {
                            st.iv.corrections.push(text.clone());
                            self.start_interview_turn(&mut st, Step::Restate, text, true);
                        }
                    }
                    Awaiting::Answer(question) => {
                        st.input.clear();
                        let answer = (!text.is_empty()).then(|| text.clone());
                        st.iv.qas.push(Qa { question, answer });
                        let message = if text.is_empty() {
                            "(skipped)".to_string()
                        } else {
                            text
                        };
                        self.next_question_or_write_up(&mut st, message);
                    }
                    Awaiting::Retry(_) => {
                        let (step, message) = (st.step, st.message.clone());
                        self.start_interview_turn(&mut st, step, message, false);
                    }
                    // typing at the write-up asks for a new one
                    Awaiting::Review => {
                        st.input.clear();
                        st.iv.refinements.push(text.clone());
                        self.start_interview_turn(&mut st, Step::WriteUp, text, true);
                    }
                    _ => {}
                }
            }
            // after a failure there is nothing to type into: only the keys
            // the status line names act
            (KeyCode::Char(_), false) if matches!(st.awaiting, Awaiting::Retry(_)) => {}
            (KeyCode::Char(c), false) => st.input.push(c),
            _ => {}
        }
        self.screen = Screen::Interview(st);
        Flow::Continue
    }

    /// Files the interview as an intent: the write-up, or with `raw` the
    /// owner's own words, in the group the reply names, else where `a` adds
    /// today. The agent's folder is kept, so `p` resumes the conversation.
    fn save_interview(&mut self, st: Box<InterviewScreen>, raw: bool) {
        let (group, areas) = self.interview_target(&st);
        // Ctrl-s after a failed refinement keeps the write-up that was on
        // screen, with the request it did not get to under Open questions
        let pending = (raw && st.step == Step::WriteUp && !st.message.trim().is_empty())
            .then(|| st.message.clone());
        let (title, body, areas, raw) = match written_intent(&st, pending.as_deref()) {
            Some((title, body)) => (title, body, areas, false),
            None => (st.iv.raw_title(), st.iv.raw_body(), Vec::new(), true),
        };
        let created = self.store.create(
            Path::new(&group),
            &title,
            &st.save_kind,
            &body,
            crate::send::capture_repo(),
        );
        let it = match created {
            Ok(it) => it,
            Err(e) => {
                self.message = Some(format!("save failed: {e:#}"));
                self.screen = Screen::Interview(st);
                return;
            }
        };
        let folder = st
            .folder
            .file_name()
            .map(|n| n.to_string_lossy().to_string());
        let (provider, conv) = (st.provider.clone(), st.conv_id.clone());
        let _ = Store::update_fields(&it.path, |fm| {
            fm.state = if raw { State::Raw } else { State::Refining };
            fm.areas = areas;
            fm.polish_dir = folder;
            if let Some(c) = conv {
                fm.conversations.insert(provider, c);
            }
        });
        // the owner's own write-up: none of its criteria are someone else's news
        let _ = crate::store::acknowledge_all_criteria(&it.path);
        for (role, text) in &st.transcript {
            let _ = Store::append_transcript(&it, role, text);
        }
        self.collapsed.remove(&self.store.root().join(&group));
        self.rescan();
        self.select_path(&it.path);
        match self.list.iter().position(|i| i.path == it.path) {
            Some(i) => self.list_selected = i,
            None if self.layout == Layout::List => {
                self.message = Some(format!(
                    "added `{}`; the {} filter hides it (f changes it)",
                    it.fm.title,
                    self.filter.label()
                ));
            }
            None => {}
        }
        self.screen = Screen::Browse;
    }

    /// Before the process exits (process::exit skips destructors): stop every
    /// agent this tool started, and leave no unsaved interview's folder behind.
    pub fn shutdown(&mut self) {
        if let Screen::Chat(st) = &mut self.screen
            && let Some(t) = st.turn.as_mut()
        {
            t.cancel();
        }
        let interview = match &mut self.screen {
            Screen::Interview(st) => Some(st),
            Screen::AgentPick { back: Some(b), .. } => match b.as_mut() {
                Screen::Interview(st) => Some(st),
                _ => None,
            },
            _ => None,
        };
        if let Some(st) = interview {
            if let Some(mut t) = st.turn.take() {
                t.cancel();
            }
            let _ = std::fs::remove_dir_all(&st.folder);
        }
        for (_, turn, _) in self.reconcile_jobs.values_mut() {
            turn.cancel();
        }
    }

    /// Esc: nothing is kept, not even the agent's folder.
    fn discard_interview(&mut self, mut st: Box<InterviewScreen>) {
        if let Some(mut t) = st.turn.take() {
            t.cancel();
        }
        let _ = std::fs::remove_dir_all(&st.folder);
        self.screen = Screen::Browse;
    }

    fn tick_interview(&mut self) {
        let Screen::Interview(mut st) = std::mem::replace(&mut self.screen, Screen::Browse) else {
            return;
        };
        let mut done: Option<Result<Reply, String>> = None;
        if let Some(turn) = st.turn.as_ref() {
            while let Some(msg) = turn.try_recv() {
                match msg {
                    TurnMsg::Delta(d) => {
                        if let Some(last) = st.transcript.last_mut() {
                            last.1.push_str(&d);
                        }
                    }
                    TurnMsg::ConversationId(id) => st.conv_id = Some(id),
                    TurnMsg::Finished(reply) => {
                        done = Some(Ok(reply));
                        break;
                    }
                    TurnMsg::Failed(e) => {
                        done = Some(Err(e));
                        break;
                    }
                }
            }
        }
        if let Some(result) = done {
            st.turn = None;
            match result {
                Err(e) => st.awaiting = Awaiting::Retry(e),
                Ok(reply) => self.interview_reply(&mut st, reply),
            }
        }
        self.screen = Screen::Interview(st);
    }

    /// Reads a finished turn by the step it was for.
    fn interview_reply(&mut self, st: &mut InterviewScreen, reply: Reply) {
        let text = st
            .transcript
            .last()
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        if text.trim().is_empty() {
            st.awaiting = Awaiting::Retry("no reply".into());
            return;
        }
        // an agent that jumps ahead and writes it up at a question is taken at its word
        if st.step == Step::Ask && reply.proposal.is_some() && reply.title.is_some() {
            st.write_up = Some(reply);
            st.awaiting = Awaiting::Review;
            return;
        }
        match st.step {
            Step::Restate => st.awaiting = Awaiting::Confirm,
            Step::Ask => match crate::interview::parse_ask(&text) {
                Asked::Question(q) => st.awaiting = Awaiting::Answer(q),
                Asked::Enough => self.start_interview_turn(st, Step::WriteUp, String::new(), false),
            },
            Step::WriteUp => {
                if reply.proposal.is_some() && reply.title.is_some() {
                    st.write_up = Some(reply);
                    st.awaiting = Awaiting::Review;
                } else {
                    st.awaiting = Awaiting::Retry("no write-up in this reply".into());
                }
            }
        }
    }

    /// Where a save would file the intent, and the other groups its criteria name.
    fn interview_target(&self, st: &InterviewScreen) -> (String, Vec<String>) {
        let group = st
            .iv
            .save_group(st.write_up.as_ref().and_then(|r| r.group.as_deref()))
            .unwrap_or_else(|| self.rel(&st.fallback).display().to_string());
        let areas = st
            .write_up
            .as_ref()
            .and_then(|r| r.proposal.as_deref())
            .map(|b| crate::reconcile::heading_areas(b, self.store.root()))
            .unwrap_or_default()
            .into_iter()
            .filter(|a| *a != group)
            .collect();
        (group, areas)
    }

    fn interview_view(&self, st: &InterviewScreen) -> InterviewView {
        let (save_group, areas) = self.interview_target(st);
        InterviewView {
            awaiting: st.awaiting.name(),
            question_no: st.iv.qas.len() + 1,
            home: st
                .iv
                .home
                .clone()
                .unwrap_or_else(|| "group to be decided".into()),
            save_kind: st.save_kind.clone(),
            suggested_kind: st.write_up.as_ref().and_then(|r| r.kind.clone()),
            save_group,
            areas,
        }
    }

    /// Adding has its own view over the whole tool area: no tree, no cards.
    fn draw_adding(&self, area: Rect, buf: &mut Buffer) {
        let (body, status) = ui::split_status(area);
        let place = |home: Option<&str>| match home {
            Some(h) if !h.is_empty() => format!("in {h}"),
            _ => "group to be decided".to_string(),
        };
        match &self.screen {
            Screen::KindPick { group } => {
                let home = group.as_ref().map(|g| self.rel(g).display().to_string());
                let header = format!("new intent · {}", place(home.as_deref()));
                ui::render_kind_question(&header, body, buf);
                ui::render_status("b/r/f/i choose · Esc cancel", status, buf);
            }
            Screen::Interview(st) => {
                let v = self.interview_view(st);
                let mut header = format!(
                    "{} · {}",
                    crate::config::kind_label(&st.save_kind),
                    place(st.iv.home.as_deref())
                );
                let asking = matches!(st.awaiting, Awaiting::Answer(_))
                    || (st.awaiting == Awaiting::Turn && st.step == Step::Ask);
                if asking {
                    header.push_str(&format!(
                        " · question {} of up to {MAX_QUESTIONS}",
                        v.question_no
                    ));
                }
                let review = st.awaiting == Awaiting::Review;
                let rows = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([
                        ratatui::layout::Constraint::Length(1),
                        ratatui::layout::Constraint::Min(1),
                        ratatui::layout::Constraint::Length(if review { 3 } else { 0 }),
                        ratatui::layout::Constraint::Length(ui::input_rows(&st.input, body.width)),
                    ])
                    .split(body);
                ratatui::text::Line::from(ratatui::text::Span::styled(
                    header,
                    ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                ))
                .render(rows[0], buf);
                // at the write-up, its last turn reads as the intent: the
                // title, then the body, without the reply's markers
                let mut shown = st.transcript.clone();
                if let (true, Some(last)) = (review, shown.last_mut())
                    && let Some((title, body)) = written_intent(st, None)
                {
                    last.1 = format!("**{title}**\n\n{}", body.trim_end());
                }
                ui::render_transcript(
                    &shown,
                    st.turn.is_some(),
                    &st.provider,
                    st.scroll,
                    rows[1],
                    buf,
                );
                if review {
                    ui::render_save_rows(
                        &st.save_kind,
                        v.suggested_kind.as_deref(),
                        &v.save_group,
                        &v.areas,
                        rows[2],
                        buf,
                    );
                }
                ui::render_input_box(&st.input, rows[3], buf);
                let text = match &st.awaiting {
                    Awaiting::Words => "say it in your own words · Enter send · Esc cancel (nothing saved)".to_string(),
                    Awaiting::Turn => "waiting for the agent · Esc cancel".to_string(),
                    Awaiting::Confirm => "Enter (or yes) that's right · type a correction · Ctrl-d write it up now · Esc cancel".to_string(),
                    Awaiting::Answer(_) => "Enter answer · Enter on empty skips · Ctrl-d write it up now · Esc cancel".to_string(),
                    Awaiting::Review => "Enter save · Tab change type · type to keep refining · Esc discard".to_string(),
                    Awaiting::Retry(reason) => {
                        format!("{reason} · Enter retry · m agent · Ctrl-s save raw · Esc discard")
                    }
                };
                // a message (a failed save) shows until the next key
                let text = self.message.clone().unwrap_or(text);
                ui::render_status(&text, status, buf);
            }
            Screen::AgentPick {
                step,
                items,
                selected,
                ..
            } => {
                let title = match step {
                    PickStep::Provider => "retry with which agent?".to_string(),
                    PickStep::Model { provider } => format!("{provider}: which model?"),
                };
                ui::render_list(&title, items, *selected, body, buf);
                ui::render_status("j/k choose · Enter select · Esc back", status, buf);
            }
            _ => {}
        }
    }

    /// Polishes with the agent and model last chosen with `m` (or the
    /// configured polish provider), without asking.
    fn polish(&mut self, pending: PendingChat) {
        let last = last_used(&self.dirs, &self.cfg);
        self.open_chat(pending, &last.provider, last.model);
    }

    /// `m`: choose the agent, then its model, for every polish from now on.
    fn open_agent_pick(&mut self, back: Option<Box<Screen>>) {
        let items: Vec<String> = self.cfg.providers.keys().cloned().collect();
        let last = last_used(&self.dirs, &self.cfg);
        let selected = items.iter().position(|p| *p == last.provider).unwrap_or(0);
        self.screen = Screen::AgentPick {
            step: PickStep::Provider,
            items,
            selected,
            back,
        };
    }

    fn key_agent_pick(
        &mut self,
        step: PickStep,
        items: Vec<String>,
        mut selected: usize,
        back: Option<Box<Screen>>,
        k: KeyEvent,
    ) -> Flow {
        match k.code {
            KeyCode::Esc => self.screen = back.map(|b| *b).unwrap_or(Screen::Browse),
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(items.len().saturating_sub(1));
                self.screen = Screen::AgentPick {
                    step,
                    items,
                    selected,
                    back,
                };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.screen = Screen::AgentPick {
                    step,
                    items,
                    selected,
                    back,
                };
            }
            KeyCode::Enter => {
                let Some(choice) = items.get(selected).cloned() else {
                    self.screen = back.map(|b| *b).unwrap_or(Screen::Browse);
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
                            self.choose_agent(&choice, None, back);
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
                                back,
                            };
                        }
                    }
                    PickStep::Model { provider } => {
                        self.choose_agent(&provider, Some(choice), back)
                    }
                }
            }
            _ => {
                self.screen = Screen::AgentPick {
                    step,
                    items,
                    selected,
                    back,
                }
            }
        }
        Flow::Continue
    }

    /// Remembers the choice for every later polish and goes back to the tree,
    /// or to the add view the picker was opened from, which uses it at once.
    fn choose_agent(&mut self, provider: &str, model: Option<String>, back: Option<Box<Screen>>) {
        self.remember_agent(provider, model.clone());
        if let Some(back) = back {
            let mut screen = *back;
            if let Screen::Interview(st) = &mut screen {
                if st.provider != provider {
                    // its conversation id means nothing to another agent:
                    // the next turn starts fresh and carries everything
                    st.conv_id = None;
                }
                st.provider = provider.to_string();
                st.model = model;
            }
            self.screen = screen;
            return;
        }
        self.message = Some(match model {
            Some(m) => format!("polish will use {provider} · {m}   (m to change)"),
            None => format!("polish will use {provider}   (m to change)"),
        });
        self.screen = Screen::Browse;
    }

    fn remember_agent(&mut self, provider: &str, model: Option<String>) {
        let _ = save_last_used(
            &self.dirs,
            &LastUsed {
                provider: provider.to_string(),
                model,
                layout: Some(self.layout.label().to_string()),
                sort: Some(self.sort.label().to_string()),
                filter: Some(self.filter.label().to_string()),
            },
        );
    }

    fn open_chat(&mut self, pending: PendingChat, provider: &str, model: Option<String>) {
        let PendingChat {
            intent,
            selection,
            from_editor,
        } = pending;
        self.remember_agent(provider, model.clone());
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
        let cwd = polish_cwd(self.store.root(), &intent.polish_name());
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
                        let kind = st.latest.as_ref().and_then(|r| r.kind.clone());
                        self.parked_chat = Some(st);
                        self.screen = Screen::Diff {
                            proposal,
                            questions,
                            title,
                            kind,
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
        kind: Option<String>,
        scroll: u16,
        k: KeyEvent,
    ) -> Flow {
        match k.code {
            KeyCode::Enter => self.apply_proposal(
                &merge_questions(&proposal, &questions),
                title.as_deref(),
                kind.as_deref(),
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
                    kind,
                    scroll: scroll.saturating_add(5),
                }
            }
            KeyCode::PageUp | KeyCode::Char('k') | KeyCode::Up => {
                self.screen = Screen::Diff {
                    proposal,
                    questions,
                    title,
                    kind,
                    scroll: scroll.saturating_sub(5),
                }
            }
            // any other key does nothing: applying needs an explicit Enter
            _ => {
                self.screen = Screen::Diff {
                    proposal,
                    questions,
                    title,
                    kind,
                    scroll,
                }
            }
        }
        Flow::Continue
    }

    /// Write `body` into the chat's intent. Unless forced, refuse when the file
    /// changed since the chat opened (another toolbar, another editor) and ask.
    fn apply_proposal(
        &mut self,
        body: &str,
        new_title: Option<&str>,
        new_kind: Option<&str>,
        force: bool,
    ) {
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
            self.screen = Screen::Prompt { question: "File changed on disk since this chat opened. Overwrite with the proposal? (y/n)".into(), pending: Pending::OverwriteApply { body: body.to_string(), title: new_title.map(str::to_string), kind: new_kind.map(str::to_string) } };
            return;
        }
        it.body = body.to_string();
        if let Some(t) = new_title.map(str::trim).filter(|t| !t.is_empty()) {
            it.fm.title = t.to_string();
        }
        // the agent judged what this intent is: a bug, a capability or an idea
        if let Some(k) = new_kind.filter(|k| KINDS.contains(k)) {
            it.fm.kind = k.to_string();
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
                // the owner reviewed and accepted this text: its criteria are seen
                let _ = crate::store::acknowledge_all_criteria(&it.path);
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
                // the file may have changed since `s`: check the gate again
                if let Some(refusal) = fresh.as_ref().and_then(crate::store::send_refusal) {
                    self.message = Some(refusal);
                    self.screen = Screen::Browse;
                    return Flow::Continue;
                }
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
        // and once more right before sending: the confirm screen can stay open
        if let Some(refusal) = crate::store::send_refusal(&it) {
            self.message = Some(refusal);
            return;
        }
        let kind = self
            .cfg
            .providers
            .get(provider)
            .map(|p| p.send_kind.clone())
            .unwrap_or_else(|| provider.to_string());
        match crate::send::send_intent(&self.herdr_env, repo, &it, &kind, prompt) {
            Ok(outcome) => {
                let place = if outcome.tab_id.is_empty() {
                    format!("new workspace {}", outcome.workspace_id)
                } else {
                    format!("a new tab {}", outcome.tab_id)
                };
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
                        // what is sent is the owner's: every criterion counts as seen
                        let _ = crate::store::acknowledge_all_criteria(&path);
                        self.message = Some(format!(
                            "sent to {} in {} (agent {}; g focuses it)",
                            handoff.name, place, outcome.agent
                        ))
                    }
                    Err(e) => {
                        self.message = Some(format!(
                            "sent to {} in {place}, but recording it failed: {e:#}",
                            handoff.name
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
                Pending::OverwriteApply { body, title, kind } => {
                    self.apply_proposal(&body, title.as_deref(), kind.as_deref(), true)
                }
                Pending::Trash { path } => self.trash(&path),
            },
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = match pending {
                    Pending::OverwriteSave | Pending::DiscardEdits => Screen::Edit,
                    Pending::Trash { .. } => Screen::Browse,
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
                // an empty areas list is a real answer: it clears them
                if value.is_empty() && !matches!(action, InputAction::EditAreas) {
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
            InputAction::EditAreas => {
                let areas: Vec<String> = value
                    .split(',')
                    .map(|a| a.trim().trim_matches('/').to_string())
                    .filter(|a| !a.is_empty())
                    .collect();
                if let Some(bad) = areas.iter().find(|a| !self.store.root().join(a).is_dir()) {
                    self.message = Some(format!("no group `{bad}`: areas unchanged"));
                } else {
                    self.update_selected_fields(|fm| fm.areas = areas);
                }
            }
            InputAction::AddItem { heading, pane } => {
                self.edit_selected_body(|body| {
                    crate::store::add_checklist_item(body, heading, value)
                });
                if heading == crate::store::CRITERIA_HEADING
                    && let Some(it) = self.selected_intent().cloned()
                {
                    // the owner's own criterion is seen
                    let n = crate::store::criteria(&it.body).len();
                    if n > 0 {
                        let _ = crate::store::acknowledge_criteria(&it.path, &[n - 1]);
                        self.reload_tree();
                    }
                }
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
            InputAction::AddArtifact { kind: default_kind } => {
                let (kind, target) = match value.split_once(':') {
                    Some((k, t))
                        if !k.trim().contains(' ')
                            && !t.trim().is_empty()
                            && !matches!(k.trim(), "http" | "https") =>
                    {
                        (k.trim().to_string(), t.trim().to_string())
                    }
                    _ => (default_kind.to_string(), value.to_string()),
                };
                self.edit_selected_body(|body| crate::store::add_artifact(body, &kind, &target));
                // the new line is the last one linked in its section
                let pane = ui::list_section(&kind);
                self.focus = pane;
                let linked = self
                    .selected_intent()
                    .cloned()
                    .map(|it| self.section_lists(&it, pane).0.len())
                    .unwrap_or(1);
                *self.list_selected(pane) = linked.saturating_sub(1);
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
            Screen::Interview(st) => st.input.push_str(text.trim_end_matches('\n')),
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
        self.poll_reconciles();
        // pull requests, plans and searches are checked in either layout (a
        // merge must reconcile from the tree too); agent states only matter
        // to the cards, so they poll only while the list shows
        if self.poll_ticks.is_multiple_of(18000) {
            self.request_found_prs();
        }
        if self.poll_ticks.is_multiple_of(1800) {
            // pick up lines agents appended to intent files
            if self.poll_ticks > 0 {
                self.reload_tree();
            }
            self.request_polls(true);
        } else if self.layout == Layout::List && self.poll_ticks.is_multiple_of(90) {
            self.request_polls(false);
        }
        self.poll_ticks = self.poll_ticks.wrapping_add(1);
        if matches!(self.screen, Screen::Interview(_)) {
            self.tick_interview();
            return;
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
        if let Ok(t) = self.scan() {
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
                    reference: false,
                },
                Row::Intent {
                    intent,
                    depth,
                    reference,
                } => RowView {
                    depth: *depth,
                    label: intent.fm.title.clone(),
                    is_group: false,
                    questions: intent.open_questions(),
                    reference: reference.is_some(),
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
                    reference: false,
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
            focus: pane_name(self.focus),
            open: pane_name(self.open),
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
            filter: self.filter.label(),
            interview: None,
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
            Screen::Interview(st) => {
                v.screen = "interview";
                v.turn_running = st.turn.is_some();
                v.transcript = st.transcript.clone();
                v.transcript_len = st.transcript.len();
                v.chat_input = st.input.clone();
                v.chat_scroll = st.scroll;
                if let Awaiting::Retry(reason) = &st.awaiting {
                    v.hint = Some(reason.clone());
                }
                v.interview = Some(self.interview_view(st));
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
            Screen::KindPick { .. } => {
                v.screen = "kind";
                v.hint = Some(KIND_QUESTION.to_string());
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
            running: Option<String>,
            group: String,
            worktree: Option<String>,
        }
        let now = self.fixed_now.unwrap_or_else(chrono::Local::now);
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
                // time since the intent was sent
                let running = match (&i.fm.sent_to, i.fm.state) {
                    (Some(sent), State::Sent) => Some(crate::progress::ago(sent.at, now)),
                    _ => None,
                };
                // its home group, then the other areas it affects
                let home = i
                    .path
                    .parent()
                    .and_then(|d| d.strip_prefix(self.store.root()).ok())
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                let group = std::iter::once(home)
                    .chain(
                        i.fm.areas
                            .iter()
                            .map(|a| a.trim().trim_matches('/').to_string())
                            .filter(|a| !a.is_empty()),
                    )
                    .collect::<Vec<_>>()
                    .join(" · ");
                let suggested = self.suggested_areas(i);
                let group = if suggested.is_empty() {
                    group
                } else {
                    format!("{group}   suggested: {} (+ adds)", suggested.join(", "))
                };
                // the sent agent's worktree, relative to the repo when inside it
                let worktree = match (&i.fm.sent_to, &self.agents, i.fm.state) {
                    (Some(sent), Some(agents), State::Sent) => {
                        agents.get(&sent.agent).and_then(|info| {
                            let path = info.worktree_path.as_ref()?;
                            let shown = match i
                                .fm
                                .repo
                                .as_deref()
                                .and_then(|r| path.strip_prefix(r).ok())
                            {
                                Some(rel) => rel.display().to_string(),
                                None => match dirs_home().and_then(|h| {
                                    path.strip_prefix(h).ok().map(|p| p.to_path_buf())
                                }) {
                                    Some(rel) => format!("~/{}", rel.display()),
                                    None => path.display().to_string(),
                                },
                            };
                            Some(match info.slot {
                                Some(n) => format!("{shown} │ slot {n}"),
                                None => shown,
                            })
                        })
                    }
                    _ => None,
                };
                Owned {
                    description,
                    agent,
                    running,
                    group,
                    worktree,
                }
            })
            .collect();
        let rows: Vec<ui::ListRow> = self
            .list
            .iter()
            .zip(&owned)
            .map(|(i, o)| {
                let when = match self.sort {
                    SortOrder::Captured | SortOrder::CreatedAsc => i.fm.created,
                    _ => crate::store::stamp(i),
                };
                ui::ListRow {
                    status: match crate::store::readiness(i) {
                        crate::store::Readiness::Sent => "→ sent".to_string(),
                        r => r.label(),
                    },
                    progress: crate::progress::progress(i, &self.evidence_for(i)),
                    agent: o.agent.clone(),
                    running: o.running.clone(),
                    title: &i.fm.title,
                    unread: i.fm.unread || !crate::store::new_criteria(i).is_empty(),
                    group: o.group.clone(),
                    worktree: o.worktree.clone(),
                    when: when.format("%b %d %H:%M").to_string(),
                    description: &o.description,
                }
            })
            .collect();
        let header = match self.filter {
            ListFilter::All => format!(
                "Intents · {} · sorted by {}",
                self.list.len(),
                self.sort.label()
            ),
            f => format!(
                "Intents · sorted by {} · showing {} ({} of {})",
                self.sort.label(),
                f.label(),
                self.list.len(),
                self.list_total
            ),
        };
        let empty = if self.list_total == 0 {
            "no intents yet: v back to the tree, then a adds one"
        } else {
            "nothing matches this filter: f changes it"
        };
        let hits =
            ui::render_intent_list(&rows, self.list_selected, &header, empty, list_area, buf);
        *self.card_hits.borrow_mut() = hits;
        let text = match &self.screen {
            Screen::Input { label, text, .. } => format!("{label}: {text}▏"),
            Screen::Prompt { question, .. } => question.clone(),
            _ => self.message.clone().or_else(|| self.reconcile_note()).unwrap_or_else(|| {
                "j/k move  Enter open in tree  a add  v tree  S sort  f filter  s send  g agent  D delete  r rescan  q quit"
                    .to_string()
            }),
        };
        ui::render_status(&text, status, buf);
    }

    pub fn draw(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        if matches!(
            self.screen,
            Screen::KindPick { .. }
                | Screen::Interview(_)
                | Screen::AgentPick { back: Some(_), .. }
        ) {
            self.draw_adding(area, buf);
            return;
        }
        // the card view stays on screen for the browser and for one-line
        // prompts (title, areas, confirmations), which use the status line
        if self.layout == Layout::List
            && matches!(
                self.screen,
                Screen::Browse | Screen::Input { .. } | Screen::Prompt { .. }
            )
        {
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
                    refs,
                } => ui::TreeRow {
                    depth: *depth,
                    label: name,
                    is_group: true,
                    collapsed: self.collapsed.contains(path),
                    questions: 0,
                    state: None,
                    count: *count,
                    broken: false,
                    reference: false,
                    refs: *refs,
                    unread: false,
                    complete: false,
                },
                Row::Intent {
                    intent,
                    depth,
                    reference,
                } => ui::TreeRow {
                    depth: *depth,
                    label: &intent.fm.title,
                    is_group: false,
                    collapsed: false,
                    questions: intent.open_questions(),
                    state: Some(intent.fm.state),
                    count: 0,
                    broken: false,
                    reference: reference.is_some(),
                    refs: 0,
                    unread: intent.fm.unread || !crate::store::new_criteria(intent).is_empty(),
                    complete: crate::progress::progress(intent, &self.evidence_for(intent))
                        .is_some_and(|p| p.complete),
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
                    reference: false,
                    refs: 0,
                    unread: false,
                    complete: false,
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
                    PickStep::Provider => {
                        "polish with which agent? (kept for every polish)".to_string()
                    }
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
                    let prs = self.pr_rows(intent);
                    ui::render_intent(
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
                            open: self.open,
                            prs: &prs,
                            prs_selected: self.prs_selected,
                            demos_selected: self.demos_selected,
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
                _ => ratatui::widgets::Paragraph::new("a add intent · n new group · ? help")
                    .render(right, buf),
            },
        }

        let status_text = match &self.screen {
            Screen::Input { label, text, .. } => format!("{label}: {text}▏"),
            // adding draws its own view, status line included (`draw_adding`)
            Screen::KindPick { .. } | Screen::Interview(_) => String::new(),
            Screen::Prompt { question, .. } => question.clone(),
            Screen::Edit => "Ctrl-s save · Ctrl-p polish · Ctrl-e $EDITOR · Esc back".to_string(),
            Screen::Chat(_) => "Enter send · Ctrl-a apply · Ctrl-d dismiss · PgUp/PgDn scroll · Esc cancel/back".to_string(),
            Screen::Diff { title, kind, .. } => match (title, kind) {
                (Some(t), Some(k)) => format!(
                    "Enter apply ({} · title → {t}) · Esc back · PgUp/PgDn scroll",
                    crate::config::kind_label(k)
                ),
                (Some(t), None) => format!("Enter apply (title → {t}) · Esc back · PgUp/PgDn scroll"),
                _ => "Enter apply · Esc back · PgUp/PgDn scroll".to_string(),
            },
            Screen::SendPick { .. } | Screen::AgentPick { .. } => "j/k choose · Enter select · Esc back".to_string(),
            Screen::Answer { .. } => "type the answer · Enter saves · Esc cancels".to_string(),
            Screen::Reader { .. } => "j/k · PgUp/PgDn scroll · Esc back".to_string(),
            Screen::SendConfirm { .. } => "Enter send · Tab provider · j/k scroll · Esc cancel".to_string(),
            Screen::Browse => match (&self.message, self.focus) {
                (Some(m), _) => m.clone(),
                (None, _) if self.reconcile_note().is_some() => {
                    self.reconcile_note().unwrap_or_default()
                }
                (None, Pane::Tree) => "v list  Tab panes  a add  t title  e edit  p polish  m model  Q answer  C criteria  V artifacts  s send  R ready  K kind  n group  M move  A attach  r rescan  D delete  q quit".to_string(),
                (None, Pane::Description) => "Description: j/k scroll · Enter edit · Tab next pane · Esc tree".to_string(),
                (None, Pane::Preconditions) => "Preconditions: j/k choose · Enter/space confirm · a add · Tab next pane · Esc tree".to_string(),
                (None, Pane::Criteria) => "Criteria: j/k choose · Enter/space toggle · y acknowledge a new one · Y all · a add · Tab next pane · Esc tree".to_string(),
                (None, Pane::Questions) => "Questions: j/k choose · Enter answer · a add · Tab next pane · Esc tree".to_string(),
                (None, Pane::Artifacts) => "Artifacts: j/k choose · Enter read · o open outside · L link a found one · a add · Esc tree".to_string(),
                (None, Pane::Demos) => "Demo Videos: j/k choose · Enter read the story · o play it · L link a found one · a add · Esc tree".to_string(),
                (None, Pane::PullRequests) => "Pull requests: j/k choose · Enter/o open in the browser · L link a found one · a add · Esc tree".to_string(),
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
        // a reference row is its own place in the tree, so selections and
        // reveals by the intent's path land on its home row
        Row::Intent {
            intent,
            reference: Some(group),
            ..
        } => group.join(format!(
            "↳{}",
            intent
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        )),
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

fn init_terminal() -> ratatui::DefaultTerminal {
    let t = ratatui::init();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture
    );
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
    run_with(cfg, dirs, false)
}

/// The capture popup: the full tool, starting at the add conversation and
/// closing once the intent is saved or the conversation is cancelled.
pub fn run_capture(cfg: Config, dirs: Dirs) -> anyhow::Result<()> {
    run_with(cfg, dirs, true)
}

fn run_with(cfg: Config, dirs: Dirs, capture: bool) -> anyhow::Result<()> {
    install_signal_handlers();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    let mut app = App::new(cfg, dirs)?;
    if capture {
        app.start_capture();
    }
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
            Ok(Event::Mouse(m)) => app.mouse(m),
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    })();

    // process::exit below skips destructors, and an agent left running
    // outlives this slot
    app.shutdown();
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

/// The write-up as it would be saved: its title, and its body with the
/// agent's remaining questions, any skipped question it left out, and
/// `pending` (a change asked for but not applied) under Open questions.
fn written_intent(st: &InterviewScreen, pending: Option<&str>) -> Option<(String, String)> {
    let r = st.write_up.as_ref()?;
    let title = r.title.clone()?;
    let proposal = r.proposal.clone()?;
    let mut qs = r.questions.clone();
    for qa in st.iv.qas.iter().filter(|qa| qa.answer.is_none()) {
        let q = qa.question.trim();
        if !q.is_empty() && !proposal.contains(q) && !qs.iter().any(|x| x.trim() == q) {
            qs.push(q.to_string());
        }
    }
    if let Some(p) = pending.map(str::trim).filter(|p| !p.is_empty()) {
        qs.push(format!("Not applied yet: {p}"));
    }
    Some((title, merge_questions(&proposal, &qs)))
}

/// The question that starts an add.
const KIND_QUESTION: &str = "new intent: is it a bug, a refactor, a feature or an idea?   b bug · r refactor · f feature · i idea · Esc cancel";

/// The title prompt's hint for a kind: how a good title for it reads.
/// The retitle prompt (`t`), e.g. `title (a bug: what goes wrong, and when)`.
fn title_label(kind: &str) -> String {
    format!(
        "title (a {}: {})",
        crate::config::kind_label(kind),
        title_hint(kind)
    )
}

/// What a good title says, for each kind.
fn title_hint(kind: &str) -> &'static str {
    match kind {
        "issue" => "what goes wrong, and when",
        "refactor" => "what changes, and where",
        "idea" => "the idea, in a phrase",
        _ => "who can do what",
    }
}

/// `add` mode: popup asks for a title; otherwise title from args or stdin's first line, body = rest.
/// `piped` is what arrived on stdin when it is not a terminal; the caller
/// reads it, so this never blocks on an open stdin.
pub fn add(
    cfg: Config,
    dirs: Dirs,
    title: Option<String>,
    popup: bool,
    piped: Option<String>,
) -> anyhow::Result<()> {
    let store = Store::open(&cfg.root)?;
    // the top of the workspace's group, or of the tree; there is no default group
    let group = cfg
        .scope
        .as_deref()
        .map(|scope| store.scope_group(scope))
        .unwrap_or_default();
    let repo = crate::send::capture_repo();
    let body = template(&dirs, "idea", false).unwrap_or_default();
    if popup {
        // the capture pane: the add conversation, in the full tool
        return run_capture(cfg, dirs);
    }
    if title.is_none() && piped.is_none() {
        anyhow::bail!(
            "usage: herdr-intents add <title>   (or pipe a title line and body on stdin)"
        );
    }
    let input = piped
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .replace('\r', "\n");
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
    let it = store.create(&group, t.trim(), "idea", &body_text, repo)?;
    println!("{}", it.path.display());
    Ok(())
}

/// The user's home folder, for shortening paths to `~/…`.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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

    /// Move the selected intent out of `raw` so the send gate lets it through.
    fn make_refining(app: &mut App) {
        let path = app.selected_intent().unwrap().path.clone();
        Store::update_fields(&path, |fm| fm.state = State::Refining).unwrap();
        app.rescan();
    }

    #[test]
    fn s_refuses_a_raw_intent_and_one_with_open_questions() {
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
        for _ in 0..8 {
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
    fn the_list_holds_every_intent_across_groups() {
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
    fn enter_in_the_list_reveals_a_deeply_nested_intent() {
        let mut fx = setup("agy.jsonl");
        // skoolscout/discover-schools/org/office-hours: never drawn in the tree yet
        let mut parent = PathBuf::from("skoolscout");
        for name in ["discover-schools", "org", "office-hours"] {
            fx.app.store.create_group(&parent, name).unwrap();
            parent = parent.join(name);
        }
        fx.app
            .store
            .create(&parent, "Deep one", "idea", "x\n", None)
            .unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('v'));
        sort_by(&mut fx.app, "Title");
        list_goto(&mut fx.app, "Deep one");
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.layout, "tree");
        assert!(
            v.rows[v.selected].label.contains("Deep one"),
            "Enter lands on the intent: {:?}",
            v.rows
        );
        assert_eq!(fx.app.selected_intent().unwrap().fm.title, "Deep one");
    }

    fn mouse(
        kind: crossterm::event::MouseEventKind,
        column: u16,
        row: u16,
    ) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }
    fn left_click(column: u16, row: u16) -> crossterm::event::MouseEvent {
        mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
        )
    }

    /// Draw once at `w`×`h` so the app knows where everything is.
    fn draw_at(app: &App, w: u16, h: u16) {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| app.draw(Rect::new(0, 0, w, h), f.buffer_mut()))
            .unwrap();
    }

    #[test]
    fn clicking_a_tree_row_selects_it_and_a_selected_group_toggles() {
        let mut fx = setup("agy.jsonl");
        draw_at(&fx.app, 100, 24);
        // rows: Inbox, In the inbox, skoolscout, Alpha, Beta
        fx.app.mouse(left_click(3, 4));
        assert_eq!(fx.app.selected_intent().unwrap().fm.title, "Beta");
        fx.app.mouse(left_click(3, 2));
        assert_eq!(
            fx.app.view().rows[fx.app.view().selected].label,
            "skoolscout"
        );
        let before = fx.app.view().rows.len();
        fx.app.mouse(left_click(3, 2)); // the selected group: collapse it
        assert_eq!(
            fx.app.view().rows.len(),
            before - 2,
            "{:?}",
            fx.app.view().rows
        );
        fx.app.mouse(left_click(3, 2)); // and open it again
        assert_eq!(fx.app.view().rows.len(), before);
        // the wheel moves the selection
        fx.app
            .mouse(mouse(crossterm::event::MouseEventKind::ScrollDown, 3, 2));
        assert_eq!(fx.app.selected_intent().unwrap().fm.title, "Alpha");
        // a click below the last row changes nothing
        fx.app.mouse(left_click(3, 20));
        assert_eq!(fx.app.selected_intent().unwrap().fm.title, "Alpha");
    }

    #[test]
    fn clicking_a_card_selects_it_and_clicking_it_again_opens_it() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 100, 60);
        let y = shown
            .lines()
            .position(|l| l.starts_with(" Org admins"))
            .unwrap() as u16;
        draw_at(&fx.app, 100, 60);
        fx.app.mouse(left_click(5, y));
        assert!(list_title(&fx.app).starts_with("Org admins"));
        assert_eq!(fx.app.view().layout, "list");
        fx.app.mouse(left_click(5, y + 2)); // anywhere on the selected card
        let v = fx.app.view();
        assert_eq!(v.layout, "tree", "a second click opens it");
        assert!(v.rows[v.selected].label.starts_with("Org admins"));
    }

    #[test]
    fn an_intent_appears_in_each_area_it_affects() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        Store::update_fields(&path, |fm| {
            fm.areas = vec!["Inbox".into(), "nowhere".into()]
        })
        .unwrap();
        fx.app.rescan();
        let v = fx.app.view();
        let at = |label: &str, reference: bool| {
            v.rows
                .iter()
                .position(|r| r.label == label && r.reference == reference)
        };
        let inbox = at("Inbox", false).unwrap();
        let reference = at("Alpha", true).expect("a reference row under Inbox");
        let home = at("Alpha", false).expect("the intent at home");
        let skoolscout = at("skoolscout", false).unwrap();
        assert!(inbox < reference && reference < skoolscout, "{:?}", v.rows);
        assert!(home > skoolscout);
        // an area that is no group is ignored
        assert_eq!(v.rows.iter().filter(|r| r.label == "Alpha").count(), 2);
        // the reference row acts on the real intent
        for _ in 0..10 {
            if fx.app.view().selected == reference {
                break;
            }
            key(&mut fx.app, KeyCode::Char('k'));
        }
        assert_eq!(fx.app.view().selected, reference);
        assert_eq!(fx.app.selected_intent().unwrap().path, path);
        // and the group shows its own count plus what affects it
        let mut t = Terminal::new(TestBackend::new(100, 20)).unwrap();
        t.draw(|f| fx.app.draw(Rect::new(0, 0, 100, 20), f.buffer_mut()))
            .unwrap();
        let text = format!("{:?}", t.backend().buffer());
        assert!(text.contains("Inbox (1 +1)"), "{text}");
        assert!(text.contains("↳ Alpha"), "{text}");
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c));
        }
    }
    fn clear_input(app: &mut App) {
        for _ in 0..80 {
            key(app, KeyCode::Backspace);
        }
    }

    #[test]
    fn plus_edits_the_areas_and_refuses_unknown_groups() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        key(&mut fx.app, KeyCode::Char('+'));
        assert_eq!(fx.app.view().screen, "input");
        assert_eq!(fx.app.view().input_text, "");
        type_text(&mut fx.app, "Inbox");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(Store::load(&path).unwrap().fm.areas, vec!["Inbox"]);
        assert!(
            fx.app
                .view()
                .rows
                .iter()
                .any(|r| r.reference && r.label == "Alpha")
        );
        // the input opens on the current areas (the selection stayed on Alpha)
        assert_eq!(fx.app.selected_intent().unwrap().path, path);
        key(&mut fx.app, KeyCode::Char('+'));
        assert_eq!(fx.app.view().input_text, "Inbox");
        clear_input(&mut fx.app);
        type_text(&mut fx.app, "Inbox, nowhere");
        key(&mut fx.app, KeyCode::Enter);
        assert!(
            fx.app
                .view()
                .message
                .unwrap_or_default()
                .contains("no group `nowhere`"),
            "{:?}",
            fx.app.view().message
        );
        assert_eq!(
            Store::load(&path).unwrap().fm.areas,
            vec!["Inbox"],
            "unchanged"
        );
        // an empty input clears them
        key(&mut fx.app, KeyCode::Char('+'));
        clear_input(&mut fx.app);
        key(&mut fx.app, KeyCode::Enter);
        assert!(Store::load(&path).unwrap().fm.areas.is_empty());
    }

    #[test]
    fn plus_works_in_the_list_too() {
        let mut fx = setup("agy.jsonl");
        key(&mut fx.app, KeyCode::Char('v'));
        sort_by(&mut fx.app, "Title");
        list_goto(&mut fx.app, "Beta");
        key(&mut fx.app, KeyCode::Char('+'));
        type_text(&mut fx.app, "Inbox");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().layout, "list");
        let beta = fx
            .app
            .store
            .scan()
            .unwrap()
            .groups
            .iter()
            .flat_map(|g| g.intents.iter())
            .find(|i| i.fm.title == "Beta")
            .unwrap()
            .fm
            .areas
            .clone();
        assert_eq!(beta, vec!["Inbox"]);
    }

    #[test]
    fn quick_capture_lands_in_the_workspace_group() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let mut cfg = load_or_init(&dirs).unwrap();
        cfg.scope = Some("tourneyseason".into());
        add(cfg, dirs.clone(), Some("Quick one".into()), false, None).unwrap();
        let names: Vec<String> = fs::read_dir(dirs.root.join("tourneyseason"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("-quick-one.md")),
            "{names:?}"
        );
    }

    #[test]
    fn the_tree_shows_only_the_workspace_group() {
        let mut fx = setup("agy.jsonl");
        fx.app.cfg.scope = Some("skoolscout".into());
        fx.app.reload_tree();
        let labels: Vec<String> = fx.app.view().rows.iter().map(|r| r.label.clone()).collect();
        assert!(labels.iter().any(|l| l == "Alpha"), "{labels:?}");
        assert!(
            !labels.iter().any(|l| l == "Inbox" || l == "In the inbox"),
            "another project's group is hidden: {labels:?}"
        );
    }

    #[test]
    fn quick_capture_lands_at_the_top_of_the_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        add(
            cfg.clone(),
            dirs.clone(),
            Some("Quick one".into()),
            false,
            None,
        )
        .unwrap();
        let names: Vec<String> = fs::read_dir(&dirs.root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("-quick-one.md")),
            "{names:?}"
        );
        assert!(
            !dirs.root.join("Inbox").exists(),
            "no Inbox group: {names:?}"
        );
        let app = App::new(cfg, dirs).unwrap();
        assert!(
            app.view()
                .rows
                .iter()
                .any(|r| r.label == "Quick one" && r.depth == 0),
            "{:?}",
            app.view().rows
        );
    }

    #[test]
    fn the_criteria_pane_scrolls_to_the_selected_criterion() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let mut body = String::from("## Description\nx\n\n## Acceptance criteria\n");
        // more than the open section holds
        for i in 0..30 {
            body.push_str(&format!("- [ ] criterion number {i}\n"));
        }
        let mut it = Store::load(&path).unwrap();
        it.body = body;
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('C'));
        for _ in 0..25 {
            key(&mut fx.app, KeyCode::Char('j'));
        }
        let shown = render(&fx.app, 100, 24);
        assert!(
            shown.contains("criterion number 25"),
            "the selection is in view: {shown}"
        );
        assert!(shown.contains("more (j/k)"), "says what is hidden: {shown}");
    }

    #[test]
    fn a_demo_line_opens_its_story_and_marks_the_criteria_it_shows() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let repo = fx._tmp.path().join("repo");
        fs::create_dir_all(repo.join("app-demos/stories")).unwrap();
        fs::write(
            repo.join("app-demos/stories/a.yaml"),
            "stories:\n  - id: alex-shows-alpha\n    title: Alex shows Alpha working\n    actor: alex\n    steps:\n      - signIn: alex\n",
        )
        .unwrap();
        let mut it = Store::load(&path).unwrap();
        it.fm.repo = Some(repo);
        it.body = "## Description\nx\n\n## Acceptance criteria\n- [ ] first thing\n- [ ] second thing\n\n## Artifacts\n- demo: alex-shows-alpha (criteria: 2)\n".into();
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        let shown = render(&fx.app, 100, 24);
        assert!(shown.contains("[ ] ▶ second thing"), "{shown}");
        assert!(shown.contains("[ ] first thing"), "{shown}");
        // demos have their own section
        tab_to(&mut fx.app, "demos");
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.screen, "reader", "{v:?}");
        assert_eq!(v.reader_title, "Alex shows Alpha working");
    }

    /// Alpha, sent, with one criterion from its refinement transcript and two
    /// an agent added.
    fn alpha_with_agent_added_criteria(fx: &mut Fx) -> PathBuf {
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let mut it = Store::load(&path).unwrap();
        it.fm.state = State::Sent;
        it.body = "## Description\nx\n\n## Acceptance criteria\n- [ ] owner wrote this\n- [ ] agent added one\n- [ ] agent added two\n".into();
        Store::save_force(&mut it).unwrap();
        fs::write(
            it.transcript_path(),
            "```proposal\n- [ ] owner wrote this\n```\n",
        )
        .unwrap();
        fx.app.rescan();
        path
    }

    #[test]
    fn added_criteria_are_highlighted_until_each_is_acknowledged() {
        let mut fx = setup("agy.jsonl");
        let path = alpha_with_agent_added_criteria(&mut fx);
        let shown = render(&fx.app, 110, 30);
        assert!(shown.contains("(0/3 · 2 new)"), "{shown}");
        assert!(shown.contains("[ ] + agent added one"), "{shown}");
        assert!(shown.contains("[ ] owner wrote this"), "{shown}");
        assert!(
            shown.contains("* Alpha"),
            "the intent carries * meanwhile: {shown}"
        );
        key(&mut fx.app, KeyCode::Char('C'));
        key(&mut fx.app, KeyCode::Char('j')); // agent added one
        key(&mut fx.app, KeyCode::Char('y'));
        assert_eq!(
            crate::store::new_criteria(&Store::load(&path).unwrap()),
            vec![2]
        );
        assert!(render(&fx.app, 110, 30).contains("* Alpha"), "one left");
        key(&mut fx.app, KeyCode::Char('j'));
        key(&mut fx.app, KeyCode::Char('y'));
        let shown = render(&fx.app, 110, 30);
        assert!(!shown.contains("* Alpha"), "the last one clears *: {shown}");
        assert!(!shown.contains("new)"), "{shown}");
    }

    #[test]
    fn y_capital_acknowledges_every_new_criterion() {
        let mut fx = setup("agy.jsonl");
        let path = alpha_with_agent_added_criteria(&mut fx);
        key(&mut fx.app, KeyCode::Char('C'));
        key(&mut fx.app, KeyCode::Char('Y'));
        assert!(crate::store::new_criteria(&Store::load(&path).unwrap()).is_empty());
    }

    #[test]
    fn the_owners_own_criteria_never_show_as_new() {
        let mut fx = setup("agy.jsonl");
        let path = alpha_with_agent_added_criteria(&mut fx);
        // adding one in the pane
        key(&mut fx.app, KeyCode::Char('C'));
        key(&mut fx.app, KeyCode::Char('a'));
        type_text(&mut fx.app, "owner added later");
        key(&mut fx.app, KeyCode::Enter);
        let it = Store::load(&path).unwrap();
        let cs = crate::store::criteria(&it.body);
        let fresh = cs
            .iter()
            .position(|c| c.text == "owner added later")
            .unwrap();
        assert!(
            !crate::store::new_criteria(&it).contains(&fresh),
            "added by the owner"
        );
        assert_eq!(
            crate::store::new_criteria(&it).len(),
            2,
            "the agent's two still new"
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

    /// An intent file written by hand so its timestamps are fixed.
    fn write_cap(root: &Path, dir: &str, stem: &str, front: &str, body: &str) {
        let d = root.join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join(format!("{stem}.md")),
            format!("---\n{front}---\n{body}"),
        )
        .unwrap();
    }

    /// Six intents covering every row kind, a repo with a plan and its
    /// ledger, a fake gh that knows PR #42 is open, and a fake herdr whose
    /// agent for the PR intent is blocked.
    fn list_fixture(repo_for_sent: bool) -> ListFx {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let mut cfg = load_or_init(&dirs).unwrap();
        cfg.providers.insert("fake".into(), fake_spec());
        cfg.polish_provider = "fake".into();
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
        let sent = |agent: &str, at: &str| {
            format!(
                "state: sent\n{repo_line}sent_to:\n  handoff: grill\n  workspace_id: w9\n  agent: {agent}\n  at: 2026-10-25T{at}:00-04:00\n"
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
                "title: Notify primary contact to schedule a meeting\nkind: idea\ncreated: 2026-10-25 00:34:00\nupdated: 2026-10-25 08:10:00\nareas:\n  - Inbox\n{}",
                sent("intent-notify-547e", "08:10")
            ),
            "## Description\nThe primary contact gets no prompt to book the intro meeting.\n\n## Acceptance criteria\n- [ ] gets an email\n\n## Artifacts\n- spec: docs/superpowers/specs/notify.md\n- pr: #42\n",
        );
        write_cap(
            root,
            "skoolscout/org",
            "building",
            &format!(
                "title: Counselors can reassign a student between orgs\nkind: idea\ncreated: 2026-10-21 09:00:00\nupdated: 2026-10-22 16:15:00\n{}",
                sent("intent-reassign-0001", "07:00")
            ),
            "## Description\nReassignment needs a support ticket today.\n\n## Acceptance criteria\n- [ ] moves\n- [ ] keeps history\n\n## Artifacts\n- plan: docs/superpowers/plans/reassign.md\n",
        );
        write_cap(
            root,
            "skoolscout/org",
            "done",
            &format!(
                "title: Invitees skip the join request\nkind: idea\ncreated: 2026-10-19 09:00:00\nupdated: 2026-10-20 10:30:00\n{}",
                sent("intent-invitees-0002", "06:00")
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
        // a fixed clock keeps running times stable in snapshots
        app.fixed_now = Some(
            chrono::DateTime::parse_from_rfc3339("2026-10-25T10:24:00-04:00")
                .unwrap()
                .with_timezone(&chrono::Local),
        );
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
            // the same workspace in every shell, inside Herdr or not
            ("HERDR_WORKSPACE_ID".into(), "w5".into()),
            (
                "GH_BIN_PATH".into(),
                fixtures().join("fake-gh.sh").display().to_string(),
            ),
            ("FAKE_GH_DIR".into(), prs.display().to_string()),
        ];
        app.provider_env = vec![
            (
                "FAKE_EVENTS".into(),
                fixtures()
                    .join("events")
                    .join("agy.jsonl")
                    .display()
                    .to_string(),
            ),
            (
                "FAKE_PROVIDER_LOG".into(),
                tmp.path().join("provider.log").display().to_string(),
            ),
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
    fn a_narrow_card_cuts_the_progress_label_with_an_ellipsis_and_never_the_bar() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let is_bar = |c: char| matches!(c, '█' | '▉' | '▓' | '░');
        // the Counselors card: agent working, building 1/3
        let fields_row = |screen: &str| -> Option<String> {
            screen
                .lines()
                .find(|l| l.contains("working"))
                .map(|l| l.trim_end().to_string())
        };
        let wide = fields_row(&render(&fx.app, 120, 60)).unwrap();
        assert!(wide.contains("building 1/3"), "{wide}");
        let bar_len = wide.chars().filter(|c| is_bar(*c)).count();
        assert!(bar_len > 0, "{wide}");
        for w in 52..=120u16 {
            let Some(row) = fields_row(&render(&fx.app, w, 60)) else {
                continue;
            };
            let cells = row.chars().filter(|c| is_bar(*c)).count();
            assert!(
                cells == 0 || cells == bar_len,
                "at {w} columns the bar is whole or absent: {row:?}"
            );
            if row.contains("buil") && !row.contains("building 1/3") {
                assert!(
                    row.ends_with('…'),
                    "at {w} columns a cut label ends in …: {row:?}"
                );
            }
        }
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
    fn send_from_the_list_shows_that_intent_in_the_tree() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        list_goto(&mut fx.app, "Org admins");
        key(&mut fx.app, KeyCode::Char('s'));
        let v = fx.app.view();
        assert_eq!(v.screen, "send-pick");
        assert!(
            v.rows[v.selected].label.contains("Org admins"),
            "the tree beside the picker points at the intent being sent: {:?}",
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
            shown.contains("Oct 25 08:10 │ → sent │ —"),
            "no endless loading mark: {shown}"
        );
    }

    #[test]
    fn cards_show_where_the_intent_lives() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 70);
        let lines: Vec<&str> = shown.lines().collect();
        let row = |prefix: &str| {
            lines
                .iter()
                .position(|l| l.starts_with(prefix))
                .unwrap_or_else(|| panic!("no line starting {prefix:?}: {shown}"))
        };
        // sent card: title, tree path, metadata, worktree path and slot, blank, description
        let t = row(" Notify primary contact");
        assert_eq!(
            lines[t + 1],
            " skoolscout/org · Inbox",
            "its home, then the other areas it affects"
        );
        assert!(
            lines[t + 2].starts_with(" Oct 25 08:10 │ → sent"),
            "{}",
            lines[t + 2]
        );
        assert_eq!(
            lines[t + 3],
            " .claude/worktrees/org/notify │ slot 3",
            "worktree path under the metadata row"
        );
        assert_eq!(lines[t + 4], "", "a blank line before the description");
        assert!(
            lines[t + 5].starts_with(" The primary contact"),
            "{}",
            lines[t + 5]
        );
        // unsent card: no worktree row
        let a = lines.iter().position(|l| *l == " a").unwrap();
        assert_eq!(lines[a + 1], " Inbox");
        assert!(lines[a + 2].starts_with(" Oct 25 07:30 │ raw"));
        assert_eq!(lines[a + 3], "");
        assert_eq!(lines[a + 4], " (no description yet)");
    }

    #[test]
    fn cards_show_how_long_the_agent_has_been_running() {
        let mut fx = list_fixture(true);
        // sent at 08:10 -04:00 in the fixture
        fx.app.fixed_now = Some(
            chrono::DateTime::parse_from_rfc3339("2026-10-25T10:24:00-04:00")
                .unwrap()
                .with_timezone(&chrono::Local),
        );
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 60);
        assert!(shown.contains("! needs you │ 2h ago"), "{shown}");
        assert!(shown.contains("working     │ 3h ago"), "{shown}");
        assert!(
            shown.contains("closed      │ 4h ago"),
            "time since sent, whether or not the pane is still there: {shown}"
        );
        assert!(
            shown.contains("│ raw    │ —           │ —"),
            "nothing before sending: {shown}"
        );
    }

    #[test]
    fn longest_running_and_created_asc_orders() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        sort_by(&mut fx.app, "Longest running");
        let titles = fx.app.view().list_titles;
        assert_eq!(
            &titles[..2],
            &[
                "Counselors can reassign a student between orgs".to_string(),
                "Notify primary contact to schedule a meeting".to_string(),
            ],
            "running agents first, longest first: {titles:?}"
        );
        assert_eq!(
            titles[2], "a",
            "then the rest by most recently changed: {titles:?}"
        );
        sort_by(&mut fx.app, "Created Asc");
        let titles = fx.app.view().list_titles;
        assert!(titles[0].starts_with("A very long title"), "{titles:?}");
        assert_eq!(titles[1], "Invitees skip the join request");
        assert_eq!(titles.last().unwrap(), "a", "newest last: {titles:?}");
        // the choice is remembered like the others
        let again = App::new(fx.app.cfg.clone(), fx.app.dirs.clone()).unwrap();
        assert_eq!(again.view().sort, "Created Asc");
    }

    /// Press `f` until the list is filtered by `filter`.
    fn filter_by(app: &mut App, filter: &str) {
        for _ in 0..4 {
            if app.view().filter == filter {
                return;
            }
            key(app, KeyCode::Char('f'));
        }
        panic!("no filter {filter}");
    }

    #[test]
    fn f_filters_in_progress_completed_and_all() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        assert_eq!(fx.app.view().filter, "All", "the default");
        assert_eq!(fx.app.view().list_titles.len(), 7);
        filter_by(&mut fx.app, "In progress");
        let titles = fx.app.view().list_titles;
        assert_eq!(titles.len(), 6, "{titles:?}");
        assert!(
            !titles.iter().any(|t| t.starts_with("Invitees")),
            "{titles:?}"
        );
        assert!(
            titles.iter().any(|t| t == "a"),
            "drafts count as in progress"
        );
        filter_by(&mut fx.app, "Completed");
        assert_eq!(
            fx.app.view().list_titles,
            vec!["Invitees skip the join request"]
        );
        let shown = render(&fx.app, 120, 20);
        assert!(shown.contains("showing Completed (1 of 7)"), "{shown}");
        let narrow = render(&fx.app, 80, 20);
        assert!(
            narrow.contains("showing Completed (1 of 7)"),
            "the header is never cut for hints: {narrow}"
        );
        // remembered like the sort order
        let again = App::new(fx.app.cfg.clone(), fx.app.dirs.clone()).unwrap();
        assert_eq!(again.view().filter, "Completed");
        filter_by(&mut fx.app, "All");
        assert_eq!(fx.app.view().list_titles.len(), 7);
    }

    #[test]
    fn an_empty_filter_says_so() {
        let mut fx = setup("agy.jsonl"); // nothing sent, nothing complete
        key(&mut fx.app, KeyCode::Char('v'));
        filter_by(&mut fx.app, "Completed");
        let shown = render(&fx.app, 100, 12);
        assert!(shown.contains("nothing matches this filter"), "{shown}");
    }

    #[test]
    fn prs_found_by_plan_name_move_the_card() {
        let mut fx = list_fixture(true);
        // the Counselors intent links plans/reassign.md but no pr: line
        fs::write(
            fx.tmp.path().join("prs/search-reassign.md.json"),
            r#"[{"number":77,"state":"OPEN"}]"#,
        )
        .unwrap();
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 70);
        let line = shown
            .lines()
            .skip_while(|l| !l.starts_with(" Counselors"))
            .nth(2)
            .unwrap_or_default()
            .to_string();
        assert!(line.contains("PR #77 open"), "{line}");
        assert!(line.contains("▉▉▉▓▉░░░"), "{line}");
        let it = fx
            .app
            .list
            .iter()
            .find(|i| i.fm.title.starts_with("Counselors"))
            .unwrap()
            .clone();
        let pr = crate::progress::progress(&it, &fx.app.evidence_for(&it)).unwrap();
        let kinds: String = crate::progress::stage_cells(&pr)
            .iter()
            .map(|c| match c.kind {
                crate::progress::CellKind::Done => 'D',
                crate::progress::CellKind::Todo => 'T',
                crate::progress::CellKind::Review => 'R',
            })
            .collect();
        assert_eq!(kinds, "DDDRDTTT", "the plan's own block shows it in review");
    }

    #[test]
    fn bar_cells_are_shades_with_no_styling_and_no_gap_cells() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let (w, h) = (120u16, 60u16);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| fx.app.draw(Rect::new(0, 0, w, h), f.buffer_mut()))
            .unwrap();
        let buf = t.backend().buffer().clone();
        let row = |y: u16| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>();
        // the Notify card: PR open, merged and verified still to do
        let y = (0..h)
            .find(|&y| row(y).contains("PR #42 open"))
            .expect("Notify's metadata row");
        let text = row(y);
        assert!(text.contains("▉▉▉██▉▉░░░ PR #42 open"), "{text}");
        assert!(!text.contains('▕'), "no gap cells: {text}");
        // Herdr drops colour and dim on block glyphs, so the bar carries none
        let start = text.find("▉▉▉██▉▉░░░").unwrap();
        let x0 = text[..start].chars().count() as u16;
        for x in x0..x0 + 10 {
            let st = buf[(x, y)].style();
            assert!(
                st.add_modifier.is_empty()
                    && st.fg.is_none_or(|c| c == ratatui::style::Color::Reset),
                "cell {x} is unstyled: {st:?}"
            );
        }
    }

    #[test]
    fn pr_branches_suggest_areas_that_plus_confirms() {
        let mut fx = list_fixture(true);
        fs::create_dir_all(fx.app.store.root().join("skoolscout/family")).unwrap();
        // the Counselors plan's PR was built in the family work area
        fs::write(
            fx.tmp.path().join("prs/search-reassign.md.json"),
            r#"[{"number":78,"state":"OPEN","headRefName":"feature/family--reassign-family"}]"#,
        )
        .unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 70);
        let t = shown
            .lines()
            .position(|l| l.starts_with(" Counselors"))
            .unwrap();
        let path_row = shown.lines().nth(t + 1).unwrap().to_string();
        assert!(
            path_row.contains("suggested: skoolscout/family"),
            "{path_row}"
        );
        sort_by(&mut fx.app, "Title");
        list_goto(&mut fx.app, "Counselors");
        key(&mut fx.app, KeyCode::Char('+'));
        assert_eq!(
            fx.app.view().input_text,
            "skoolscout/family",
            "suggestions prefilled"
        );
        key(&mut fx.app, KeyCode::Enter);
        let it = fx.app.selected_intent().unwrap().clone();
        assert_eq!(
            Store::load(&it.path).unwrap().fm.areas,
            vec!["skoolscout/family"]
        );
    }

    #[test]
    fn a_merged_intent_is_reconciled_marked_unread_and_read_on_open() {
        let mut fx = list_fixture(true);
        fs::write(fx.tmp.path().join("prs/42"), "MERGED").unwrap(); // Notify's only PR
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let path = fx.app.store.root().join("skoolscout/org/pr.md");
        let it = Store::load(&path).unwrap();
        assert!(
            it.body
                .contains("Parents who skip the report card folder see Pending."),
            "the agent's rewrite was applied: {}",
            it.body
        );
        assert!(it.body.contains("- Which org?"), "its questions merged in");
        assert!(it.fm.unread, "marked unread");
        assert_eq!(it.fm.reconciled_for, vec![42]);
        assert!(
            crate::reconcile::backup_path(&path).exists(),
            "previous version kept"
        );
        let shown = render(&fx.app, 120, 70);
        assert!(
            shown
                .lines()
                .any(|l| l.starts_with(" * Notify primary contact")),
            "the card shows * until it is read: {shown}"
        );
        // the same merged set is not reconciled twice
        for _ in 0..5 {
            settle(&mut fx.app);
        }
        let runs = fs::read_to_string(fx.tmp.path().join("provider.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("--prompt="))
            .count();
        assert_eq!(runs, 1, "one agent run per merged set");
        // opening it reads it
        sort_by(&mut fx.app, "Title");
        list_goto(&mut fx.app, "Notify");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().layout, "tree");
        assert!(!Store::load(&path).unwrap().fm.unread, "read once opened");
        // u puts the previous version back
        key(&mut fx.app, KeyCode::Char('u'));
        let back = Store::load(&path).unwrap();
        assert!(
            back.body.contains("The primary contact gets no prompt"),
            "{}",
            back.body
        );
        assert!(!crate::reconcile::backup_path(&path).exists());
    }

    #[test]
    fn sent_reads_as_an_arrow_and_only_complete_gets_a_check() {
        let mut fx = list_fixture(true);
        settle(&mut fx.app);
        // open every group so all intents show in the tree
        let groups: Vec<PathBuf> = fx.app.collapsed.iter().cloned().collect();
        for g in groups {
            fx.app.collapsed.remove(&g);
        }
        fx.app.rebuild_rows();
        let tree = render(&fx.app, 120, 30);
        let line = |title: &str| {
            tree.lines()
                .find(|l| l.contains(title) && !l.contains('↳'))
                .unwrap_or_else(|| panic!("no {title} in {tree}"))
                .to_string()
        };
        assert!(
            line("Notify primary contact").contains("→ Notify"),
            "sent: {tree}"
        );
        assert!(
            line("Invitees skip").contains("✓ Invitees"),
            "complete: {tree}"
        );
        assert!(
            !line("Counselors").contains('✓'),
            "sent, not complete: {tree}"
        );
        // the card's status field
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let cards = render(&fx.app, 120, 70);
        assert!(cards.contains("│ → sent │"), "{cards}");
    }

    #[test]
    fn moving_between_cards_does_not_scan_the_repo_for_found_artifacts() {
        let mut fx = list_fixture(true);
        let specs = fx.tmp.path().join("repo/docs/superpowers/specs");
        fs::create_dir_all(&specs).unwrap();
        fs::write(
            specs.join("reassign-design.md"),
            "# Counselors reassign a student between orgs\n",
        )
        .unwrap();
        key(&mut fx.app, KeyCode::Char('v'));
        assert_eq!(fx.app.view().layout, "list");
        fx.app.found_cache = None;
        for _ in 0..6 {
            key(&mut fx.app, KeyCode::Char('j'));
        }
        assert!(
            fx.app.found_cache.is_none(),
            "the list never shows found artifacts, so it does not look for them"
        );
        // the tree still finds them for the intent it shows
        key(&mut fx.app, KeyCode::Char('v'));
        select(&mut fx.app, "org");
        key(&mut fx.app, KeyCode::Char('l'));
        select(&mut fx.app, "Counselors can reassign");
        assert_eq!(fx.app.view().found_count, 1);
    }

    #[test]
    fn a_reconcile_hands_the_agent_the_suggested_areas_too() {
        let mut fx = list_fixture(true);
        fs::create_dir_all(fx.app.store.root().join("skoolscout/family")).unwrap();
        fs::write(fx.tmp.path().join("prs/42"), "MERGED").unwrap();
        fs::write(
            fx.tmp.path().join("prs/search-notify.md.json"),
            r#"[{"number":42,"state":"MERGED","headRefName":"feature/family--notify-family"}]"#,
        )
        .unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        settle(&mut fx.app);
        let log = fs::read_to_string(fx.tmp.path().join("provider.log")).unwrap_or_default();
        assert!(
            log.contains("Areas it affects besides its own group: Inbox, skoolscout/family"),
            "the reconcile prompt lists the suggested area: {log}"
        );
    }

    #[test]
    fn a_merged_intent_is_reconciled_while_the_tree_shows() {
        let mut fx = list_fixture(true);
        fs::write(fx.tmp.path().join("prs/42"), "MERGED").unwrap();
        assert_eq!(fx.app.view().layout, "tree", "never opening the list");
        settle(&mut fx.app);
        settle(&mut fx.app);
        let it = Store::load(&fx.app.store.root().join("skoolscout/org/pr.md")).unwrap();
        assert_eq!(it.fm.reconciled_for, vec![42], "reconciled from the tree");
        let herdr = fs::read_to_string(fx.tmp.path().join("herdr.log")).unwrap_or_default();
        assert!(
            !herdr.contains("agent list"),
            "agent states only poll for the cards: {herdr}"
        );
    }

    #[test]
    fn ready_and_refining_keys_leave_a_sent_intent_sent() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        Store::update_fields(&path, |fm| fm.state = State::Sent).unwrap();
        fx.app.rescan();
        for k in ['R', 'U'] {
            key(&mut fx.app, KeyCode::Char(k));
            assert_eq!(
                Store::load(&path).unwrap().fm.state,
                State::Sent,
                "{k} kept it sent"
            );
            assert!(
                fx.app
                    .view()
                    .message
                    .unwrap_or_default()
                    .contains("already sent"),
                "{:?}",
                fx.app.view().message
            );
        }
    }

    #[test]
    fn a_failed_reconcile_stays_visible_and_r_retries_it() {
        let mut fx = list_fixture(true);
        fs::write(fx.tmp.path().join("prs/42"), "MERGED").unwrap();
        let empty = fx.tmp.path().join("empty.jsonl");
        fs::write(&empty, "").unwrap();
        fx.app.provider_env[0].1 = empty.display().to_string(); // the agent says nothing useful
        settle(&mut fx.app);
        settle(&mut fx.app);
        let path = fx.app.store.root().join("skoolscout/org/pr.md");
        assert!(
            Store::load(&path).unwrap().fm.reconciled_for.is_empty(),
            "not applied"
        );
        // select the intent and press an unrelated key: the reason is still shown
        let groups: Vec<PathBuf> = fx.app.collapsed.iter().cloned().collect();
        for g in groups {
            fx.app.collapsed.remove(&g);
        }
        fx.app.rebuild_rows();
        fx.app.select_path(&path);
        key(&mut fx.app, KeyCode::Char('Q'));
        key(&mut fx.app, KeyCode::Esc);
        let status = render(&fx.app, 120, 30)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string();
        assert!(
            status.contains("reconcile") && status.contains("r retries"),
            "{status}"
        );
        // a working agent and r: it runs again
        fx.app.provider_env[0].1 = fixtures()
            .join("events")
            .join("agy.jsonl")
            .display()
            .to_string();
        key(&mut fx.app, KeyCode::Char('r'));
        settle(&mut fx.app);
        settle(&mut fx.app);
        assert_eq!(
            Store::load(&path).unwrap().fm.reconciled_for,
            vec![42],
            "retried"
        );
    }

    #[test]
    fn a_reconcile_that_runs_too_long_is_stopped_and_reported() {
        let mut fx = list_fixture(true);
        fs::write(fx.tmp.path().join("prs/42"), "MERGED").unwrap();
        let empty = fx.tmp.path().join("empty.jsonl");
        fs::write(&empty, "").unwrap();
        fx.app.provider_env[0].1 = empty.display().to_string();
        fx.app
            .provider_env
            .push(("FAKE_MODE".into(), "hang".into())); // never answers
        fx.app.reconcile_limit = Duration::from_millis(300);
        settle(&mut fx.app);
        settle(&mut fx.app);
        let path = fx.app.store.root().join("skoolscout/org/pr.md");
        assert!(Store::load(&path).unwrap().fm.reconciled_for.is_empty());
        assert!(
            fx.app
                .reconcile_failed
                .get(&path)
                .is_some_and(|m| m.contains("did not finish")),
            "{:?}",
            fx.app.reconcile_failed
        );
        assert_eq!(fx.app.polls_in_flight(), 0, "the stuck agent is gone");
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
    fn the_list_opens_on_its_first_card_from_an_empty_group() {
        let mut fx = list_fixture(true);
        fs::create_dir_all(fx.app.store.root().join("zz-empty")).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('v'));
        key(&mut fx.app, KeyCode::Char('j'));
        key(&mut fx.app, KeyCode::Char('j'));
        let moved = fx.app.view().list_selected;
        assert!(moved > 0);
        key(&mut fx.app, KeyCode::Char('v'));
        select(&mut fx.app, "zz-empty");
        key(&mut fx.app, KeyCode::Char('v'));
        assert_eq!(
            fx.app.view().list_selected,
            0,
            "no card under the group: start at the first, not where the list was"
        );
    }

    #[test]
    fn a_poll_that_reorders_the_progress_sort_keeps_the_selection() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        sort_by(&mut fx.app, "Progress");
        for _ in 0..10 {
            if list_title(&fx.app).starts_with("Counselors") {
                break;
            }
            key(&mut fx.app, KeyCode::Char('j'));
        }
        let before = fx.app.view().list_titles;
        let at = fx.app.view().list_selected;
        // the Notify card's PR closes: its progress drops below Counselors'
        fs::write(fx.tmp.path().join("prs/42"), "CLOSED").unwrap();
        fx.app.rescan();
        settle(&mut fx.app);
        settle(&mut fx.app);
        let after = fx.app.view().list_titles;
        assert_ne!(before, after, "the poll re-sorted the list");
        assert_ne!(
            after[at], before[at],
            "another card now sits where the selection was"
        );
        assert!(
            list_title(&fx.app).starts_with("Counselors"),
            "the selection follows its card: {}",
            list_title(&fx.app)
        );
    }

    #[test]
    fn a_repo_that_is_a_file_still_renders_in_the_list() {
        let mut fx = list_fixture(true);
        let file = fx.tmp.path().join("not-a-folder");
        fs::write(&file, "x").unwrap();
        let path = fx.app.store.root().join("skoolscout/org/building.md");
        Store::update_fields(&path, |fm| fm.repo = Some(file.clone())).unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('v'));
        settle(&mut fx.app);
        let shown = render(&fx.app, 120, 60);
        assert!(shown.contains("Counselors can reassign"), "{shown}");
        let row = shown
            .lines()
            .find(|l| l.contains("working"))
            .unwrap_or_else(|| panic!("its fields row: {shown}"));
        assert!(!row.contains("building 1/3"), "no plans can be read: {row}");
    }

    #[test]
    fn page_keys_move_the_list_by_a_screen() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        let total = fx.app.view().list_titles.len();
        let _ = render(&fx.app, 120, 20);
        let shown = fx.app.card_hits.borrow().len();
        assert!(
            shown >= 2 && shown < total,
            "{shown} of {total} cards on screen"
        );
        // the list opens on the tree's intent, not necessarily the first
        let start = fx.app.view().list_selected;
        key(&mut fx.app, KeyCode::PageDown);
        assert_eq!(
            fx.app.view().list_selected,
            start + shown - 1,
            "a screen less one: the last card seen stays on screen"
        );
        key(&mut fx.app, KeyCode::PageUp);
        assert_eq!(fx.app.view().list_selected, start);
    }

    #[test]
    fn a_question_added_after_s_stops_the_send() {
        let mut fx = list_fixture(true);
        key(&mut fx.app, KeyCode::Char('v'));
        sort_by(&mut fx.app, "Title");
        for _ in 0..10 {
            if list_title(&fx.app).starts_with("Org admins") {
                break;
            }
            key(&mut fx.app, KeyCode::Char('j'));
        }
        let path = fx.app.selected_intent().unwrap().path.clone();
        let repo = fx.tmp.path().join("repo");
        Store::update_fields(&path, |fm| fm.repo = Some(repo)).unwrap();
        fx.app.rescan();
        let original = Store::load(&path).unwrap().body;
        let ask = |body: &str| {
            let mut it = Store::load(&path).unwrap();
            it.body = body.to_string();
            Store::save_force(&mut it).unwrap();
        };
        let refused = |app: &App| {
            app.view().screen == "browse"
                && app
                    .view()
                    .message
                    .unwrap_or_default()
                    .contains("1 open question")
        };
        // a question appears while the handoff picker is open
        key(&mut fx.app, KeyCode::Char('s'));
        assert_eq!(fx.app.view().screen, "send-pick");
        ask(&format!("{original}- And which format?\n"));
        key(&mut fx.app, KeyCode::Enter);
        assert!(refused(&fx.app), "{:?}", fx.app.view().message);
        // and while the confirm screen is showing
        ask(&original);
        key(&mut fx.app, KeyCode::Char('s'));
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "send-confirm");
        ask(&format!("{original}- And which format?\n"));
        key(&mut fx.app, KeyCode::Enter);
        assert!(refused(&fx.app), "{:?}", fx.app.view().message);
        let calls = fs::read_to_string(fx.tmp.path().join("herdr.log")).unwrap_or_default();
        assert!(
            !calls.contains("tab create") && !calls.contains("workspace create"),
            "nothing sent: {calls}"
        );
        assert_ne!(Store::load(&path).unwrap().fm.state, State::Sent);
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
    fn a_sent_intent_without_repo_still_renders() {
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
        assert_eq!(
            app.view().screen,
            "chat",
            "p polishes at once: {:?}",
            app.view()
        );
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
            .push(("FAKE_MODE".into(), "midturn".into()));
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
            ("HERDR_WORKSPACE_ID".into(), "w5".into()),
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
        assert!(
            it.fm.acknowledged.is_some(),
            "what was sent is recorded as seen"
        );
        let sent = it.fm.sent_to.unwrap();
        assert_eq!(sent.handoff, "grill");
        assert_eq!(sent.workspace_id, "w5", "the agent runs in this workspace");
        let calls = fs::read_to_string(&log).unwrap();
        assert!(calls.contains("tab create --workspace w5"), "{calls}");
        assert!(!calls.contains("workspace create"), "{calls}");
        assert!(calls.contains("agent prompt"));
        let msg = fx.app.view().message.unwrap_or_default();
        assert!(msg.contains("new tab"), "{msg}");

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
            ("HERDR_WORKSPACE_ID".into(), "w5".into()),
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
        assert_eq!(fx.app.view().screen, "chat", "polish starts at once");
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
        key(&mut fx.app, KeyCode::Char('M'));
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
    fn m_chooses_provider_then_model_and_polish_uses_it_from_then_on() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('m'));
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
        assert_eq!(
            fx.app.view().screen,
            "browse",
            "choosing does not start a chat"
        );
        let last = last_used(&fx.app.dirs, &fx.app.cfg);
        assert_eq!(
            (last.provider.as_str(), last.model.as_deref()),
            ("fake", Some("model-b"))
        );
        // p uses it without asking
        key(&mut fx.app, KeyCode::Char('p'));
        assert_eq!(fx.app.view().screen, "chat");
        wait_until(&mut fx.app, "turn", |a| !a.view().turn_running);
        let log = fs::read_to_string(&fx.log).unwrap();
        assert!(
            log.contains("--model model-b"),
            "model reaches the provider argv: {log}"
        );
        key(&mut fx.app, KeyCode::Esc);
        // next time, the last choice is preselected
        key(&mut fx.app, KeyCode::Char('m'));
        key(&mut fx.app, KeyCode::Enter);
        let v = fx.app.view();
        assert_eq!(v.pick_items[v.pick_selected], "model-b");
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
    }

    #[test]
    fn provider_without_models_skips_the_model_step() {
        let mut fx = setup("agy.jsonl");
        fx.app.cfg.providers.get_mut("fake").unwrap().models.clear();
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('m'));
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "browse");
        let last = last_used(&fx.app.dirs, &fx.app.cfg);
        assert_eq!((last.provider.as_str(), last.model), ("fake", None));
    }

    #[test]
    fn tab_opens_one_section_at_a_time_and_the_last_stays_open() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox");
        assert_eq!(fx.app.view().open, "criteria", "criteria open to start");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "description");
        assert_eq!(
            fx.app.view().open,
            "criteria",
            "the description leaves it open"
        );
        for pane in [
            "preconditions",
            "criteria",
            "demos",
            "questions",
            "pull requests",
            "artifacts",
        ] {
            key(&mut fx.app, KeyCode::Tab);
            assert_eq!(fx.app.view().focus, pane);
            assert_eq!(fx.app.view().open, pane, "the focused section opens");
        }
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "tree");
        assert_eq!(fx.app.view().open, "artifacts", "the last one stays open");
        select(&mut fx.app, "Alpha");
        assert_eq!(
            fx.app.view().open,
            "artifacts",
            "and stays open on the next intent"
        );
        key(&mut fx.app, KeyCode::BackTab);
        assert_eq!(fx.app.view().open, "artifacts");
        key(&mut fx.app, KeyCode::BackTab);
        assert_eq!(fx.app.view().open, "pull requests");
    }

    /// Tab from the tree to the named section.
    fn tab_to(app: &mut App, pane: &str) {
        for _ in 0..8 {
            if app.view().focus == pane {
                return;
            }
            key(app, KeyCode::Tab);
        }
        panic!("never reached {pane}");
    }

    #[test]
    fn pull_requests_show_their_titles_and_l_links_a_found_one() {
        let mut fx = list_fixture(true);
        let prs = fx.tmp.path().join("prs");
        fs::write(prs.join("42.title"), "Notify the family by email").unwrap();
        fs::write(
            prs.join("search-notify.md.json"),
            r#"[{"number":43,"state":"MERGED","headRefName":"fix/org/notify--copy","title":"Fix the email copy"}]"#,
        )
        .unwrap();
        select(&mut fx.app, "Notify primary contact");
        settle(&mut fx.app);
        settle(&mut fx.app);
        tab_to(&mut fx.app, "pull requests");
        let shown = render(&fx.app, 120, 40);
        assert!(
            shown.contains("Pull requests (0/1 merged, 1 found)"),
            "{shown}"
        );
        assert!(
            shown.contains("open    #42  Notify the family by email"),
            "{shown}"
        );
        assert!(
            shown.contains("merged? #43  Fix the email copy   (found · L links)"),
            "{shown}"
        );
        key(&mut fx.app, KeyCode::Char('j'));
        key(&mut fx.app, KeyCode::Char('L'));
        let path = fx.app.selected_intent().unwrap().path.clone();
        let body = Store::load(&path).unwrap().body;
        assert!(body.contains("- pr: #43"), "{body}");
        let shown = render(&fx.app, 120, 40);
        assert!(shown.contains("Pull requests (1/2 merged)"), "{shown}");
        assert_eq!(fx.app.view().focus, "pull requests");
    }

    #[test]
    fn a_in_pull_requests_and_demos_adds_a_line_of_that_kind() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        tab_to(&mut fx.app, "pull requests");
        key(&mut fx.app, KeyCode::Char('a'));
        type_str(&mut fx.app, "#77");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().focus, "pull requests");
        tab_to(&mut fx.app, "demos");
        key(&mut fx.app, KeyCode::Char('a'));
        type_str(&mut fx.app, "story-x (criteria: 2)");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().focus, "demos");
        let body = Store::load(&path).unwrap().body;
        assert!(body.contains("- pr: #77"), "{body}");
        assert!(body.contains("- demo: story-x (criteria: 2)"), "{body}");
    }

    #[test]
    fn the_demo_videos_help_line_names_the_section() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        tab_to(&mut fx.app, "demos");
        let shown = render(&fx.app, 140, 30);
        assert!(shown.contains("Demo Videos: j/k choose"), "{shown}");
    }

    #[test]
    fn reading_a_demo_returns_to_the_demos_section() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        let repo = fx._tmp.path().join("repo");
        fs::create_dir_all(repo.join("app-demos/stories")).unwrap();
        fs::write(
            repo.join("app-demos/stories/a.yaml"),
            "stories:\n  - id: alex-shows-alpha\n    title: Alex shows Alpha working\n    actor: alex\n    steps:\n      - signIn: alex\n",
        )
        .unwrap();
        let mut it = Store::load(&path).unwrap();
        it.fm.repo = Some(repo);
        it.body = "## Description\nx\n\n## Acceptance criteria\n- [ ] first thing\n\n## Artifacts\n- spec: docs/x.md\n- demo: alex-shows-alpha (criteria: 1)\n".into();
        Store::save_force(&mut it).unwrap();
        fx.app.rescan();
        tab_to(&mut fx.app, "demos");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(
            fx.app.view().screen,
            "reader",
            "the section's first row is the demo"
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().focus, "demos");
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
        assert_eq!(fx.app.view().focus, "demos");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "questions");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "pull requests");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "artifacts");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().focus, "tree");
        key(&mut fx.app, KeyCode::BackTab);
        assert_eq!(fx.app.view().focus, "artifacts");
        key(&mut fx.app, KeyCode::BackTab);
        assert_eq!(fx.app.view().focus, "pull requests");
        key(&mut fx.app, KeyCode::Tab);
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
    fn detail_shows_every_section_title_and_opens_one() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox");
        let text = render(&fx.app, 100, 24);
        for title in [
            "Description",
            "Preconditions",
            "Acceptance criteria",
            "Open questions",
            "Artifacts",
        ] {
            assert!(text.contains(title), "pane {title} missing: {text}");
        }
        assert!(!text.contains("? a?"), "questions start collapsed: {text}");
        // tree → description → preconditions → criteria → demos → questions
        for _ in 0..5 {
            key(&mut fx.app, KeyCode::Tab);
        }
        let text = render(&fx.app, 100, 24);
        assert!(
            text.contains("? a?") && text.contains("? b?"),
            "questions listed with their open mark: {text}"
        );
    }

    #[test]
    fn t_renames_the_intent_with_the_current_title_prefilled() {
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
    fn f_and_i_choose_feature_and_idea() {
        for (k, kind) in [('f', "enhancement"), ('i', "idea")] {
            let mut fx = setup("agy.jsonl");
            select(&mut fx.app, "skoolscout");
            key(&mut fx.app, KeyCode::Char('a'));
            key(&mut fx.app, KeyCode::Char(k));
            assert_eq!(fx.app.view().interview.unwrap().save_kind, kind, "{k}");
        }
    }

    #[test]
    fn esc_at_the_kind_question_adds_nothing() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        let count = |app: &App| app.view().rows.len();
        let before = count(&fx.app);
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
        assert_eq!(count(&fx.app), before);
    }

    // ---- adding by conversation ----------------------------------------------

    const RESTATE: &str = "summary: Tenant admins are notified\n\nWhen someone accepts a team invite, every tenant admin is notified. Is that right?";
    const WORDS: &str = "tenant admins getting notifications when a user accepts a team invite";
    const WRITE_UP: &str = "bug: Tenant admins are emailed when anyone accepts a team invite\ngroup: skoolscout\n```proposal\n## Description\nx\n\n## Acceptance criteria\n- [ ] only the inviter\n\n### skoolscout/global\n- [ ] no email to others\n```\nquestions: none\n";

    /// Queue the fake agent's replies, one per turn; the last one repeats.
    fn agent_replies(fx: &mut Fx, replies: &[&str]) {
        let dir = fx
            ._tmp
            .path()
            .join(format!("replies-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        for (i, text) in replies.iter().enumerate() {
            let lines = [
                serde_json::json!({"event": "init", "conversation_id": "conv-int"}),
                serde_json::json!({"event": "step_update", "step_update": {
                    "conversation_id": "conv-int", "step_type": "agent_response", "text_delta": text}}),
                serde_json::json!({"event": "result", "result": {
                    "conversation_id": "conv-int", "status": "SUCCESS", "response": text}}),
            ];
            let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
            fs::write(dir.join(format!("{}.jsonl", i + 1)), body).unwrap();
        }
        fx.app
            .provider_env
            .retain(|(k, _)| k != "FAKE_EVENTS" && k != "FAKE_EVENTS_DIR");
        fx.app
            .provider_env
            .push(("FAKE_EVENTS_DIR".into(), dir.display().to_string()));
    }

    /// Each provider call's prompt, in order.
    fn calls(fx: &Fx) -> Vec<String> {
        let log = fs::read_to_string(&fx.log).unwrap_or_default();
        log.split("--prompt=").skip(1).map(str::to_string).collect()
    }

    fn awaiting(app: &App) -> &'static str {
        app.view().interview.map(|i| i.awaiting).unwrap_or("none")
    }

    fn wait_turn(app: &mut App) {
        wait_until(app, "the agent's turn", |a| awaiting(a) != "turn");
    }

    fn md_files(dir: &Path) -> usize {
        let mut n = 0;
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                n += md_files(&p);
            } else if name.ends_with(".md") && !name.ends_with(".chat.md") {
                n += 1;
            }
        }
        n
    }

    fn new_folders(root: &Path) -> Vec<String> {
        fs::read_dir(root.join(".polish"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.starts_with("new-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// From the tree, on group `skoolscout`: a, b, the words, Enter, wait.
    fn start_bug_in_skoolscout(fx: &mut Fx) {
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        type_str(&mut fx.app, WORDS);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
    }

    #[test]
    fn a_in_the_tree_starts_an_interview_on_that_group() {
        let mut fx = setup("agy.jsonl");
        let before = md_files(fx.app.store.root());
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        let v = fx.app.view();
        assert_eq!(v.screen, "interview");
        let iv = v.interview.unwrap();
        assert_eq!(iv.awaiting, "words");
        assert_eq!(iv.home, "skoolscout");
        assert_eq!(md_files(fx.app.store.root()), before, "nothing saved yet");
    }

    #[test]
    fn the_agent_restates_first_and_asks_nothing() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE]);
        start_bug_in_skoolscout(&mut fx);
        assert_eq!(awaiting(&fx.app), "confirm");
        let c = calls(&fx);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!(
            c[0].contains(WORDS) && c[0].contains("summary:"),
            "{}",
            c[0]
        );
        assert!(!c[0].contains("question 1 of"), "{}", c[0]);
    }

    #[test]
    fn yes_at_the_restatement_means_that_s_right() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "question: Who sees it?"]);
        start_bug_in_skoolscout(&mut fx);
        type_str(&mut fx.app, "yes");
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(
            c[1].contains("question 1 of at most 5"),
            "asks, not restates: {}",
            c[1]
        );
        assert_eq!(awaiting(&fx.app), "answer");
    }

    #[test]
    fn a_correction_gets_a_new_restatement() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE]);
        start_bug_in_skoolscout(&mut fx);
        type_str(&mut fx.app, "only the inviter");
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c[1].contains("only the inviter"), "{}", c[1]);
        assert_eq!(awaiting(&fx.app), "confirm");
    }

    #[test]
    fn questions_come_one_at_a_time_up_to_five() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "question: Q?"]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter); // that's right
        wait_turn(&mut fx.app);
        for i in 1..=5 {
            let iv = fx.app.view().interview.unwrap();
            assert_eq!((iv.awaiting, iv.question_no), ("answer", i));
            type_str(&mut fx.app, &format!("a{i}"));
            key(&mut fx.app, KeyCode::Enter);
            wait_turn(&mut fx.app);
        }
        let c = calls(&fx);
        assert_eq!(c.len(), 7, "restate, five asks, a write-up: {c:?}");
        assert!(c[6].contains(crate::interview::WRITE_UP_MARKER), "{}", c[6]);
    }

    #[test]
    fn enough_goes_straight_to_the_write_up() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "enough", WRITE_UP]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert_eq!(c.len(), 3, "{c:?}");
        assert!(c[2].contains(crate::interview::WRITE_UP_MARKER), "{}", c[2]);
        assert_eq!(awaiting(&fx.app), "review");
    }

    #[test]
    fn enter_on_empty_skips_and_ctrl_d_writes_up_now() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "question: Q?"]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        key(&mut fx.app, KeyCode::Enter); // skip question 1
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert!(c[2].contains("(skipped)"), "{}", c[2]);
        assert_eq!(fx.app.view().interview.unwrap().question_no, 2);
        fx.app
            .key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert!(
            c.last()
                .unwrap()
                .contains(crate::interview::WRITE_UP_MARKER),
            "{c:?}"
        );
    }

    #[test]
    fn the_write_up_shows_title_type_group_and_areas() {
        let mut fx = setup("agy.jsonl");
        fx.app
            .store
            .create_group(Path::new("skoolscout"), "global")
            .unwrap();
        fx.app.rescan();
        agent_replies(&mut fx, &[RESTATE, "enough", WRITE_UP]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let iv = fx.app.view().interview.unwrap();
        assert_eq!(iv.awaiting, "review");
        assert_eq!(iv.save_kind, "issue");
        assert_eq!(iv.save_group, "skoolscout");
        assert_eq!(iv.areas, ["skoolscout/global"]);
    }

    #[test]
    fn enter_while_the_agent_works_is_ignored() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE]);
        fx.app
            .provider_env
            .push(("FAKE_MODE".into(), "midturn".into()));
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        type_str(&mut fx.app, WORDS);
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(awaiting(&fx.app), "turn");
        type_str(&mut fx.app, "more"); // a non-empty box: Enter would send it
        key(&mut fx.app, KeyCode::Enter);
        std::thread::sleep(Duration::from_millis(200));
        fx.app.tick();
        assert_eq!(calls(&fx).len(), 1, "no second turn");
        assert_eq!(
            fx.app.view().transcript_len,
            2,
            "the words and the agent's turn, nothing more"
        );
        assert_eq!(awaiting(&fx.app), "turn");
    }

    #[test]
    fn a_failed_turn_keeps_the_words_and_enter_retries() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &["", RESTATE]);
        // an empty first reply: the fake prints nothing at all
        let dir = fx
            .app
            .provider_env
            .iter()
            .find(|(k, _)| k == "FAKE_EVENTS_DIR")
            .map(|(_, v)| PathBuf::from(v))
            .unwrap();
        fs::write(dir.join("1.jsonl"), "").unwrap();
        start_bug_in_skoolscout(&mut fx);
        assert_eq!(awaiting(&fx.app), "retry");
        let hint = fx.app.view().hint.unwrap_or_default();
        assert!(hint.contains("no reply"), "{hint}");
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "confirm");
        let c = calls(&fx);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c[0].contains(WORDS) && c[1].contains(WORDS));
    }

    #[test]
    fn esc_discards_and_leaves_no_file_or_folder() {
        let mut fx = setup("agy.jsonl");
        let before = md_files(fx.app.store.root());
        agent_replies(&mut fx, &[RESTATE]);
        start_bug_in_skoolscout(&mut fx);
        assert_eq!(
            new_folders(fx.app.store.root()).len(),
            1,
            "the agent's folder"
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
        assert_eq!(md_files(fx.app.store.root()), before);
        assert!(new_folders(fx.app.store.root()).is_empty());
    }

    #[test]
    fn in_the_card_view_the_home_group_is_unknown() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "question: Q?"]);
        key(&mut fx.app, KeyCode::Char('v'));
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('i'));
        assert_eq!(fx.app.view().interview.unwrap().home, "group to be decided");
        type_str(&mut fx.app, WORDS);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert!(c[1].contains("Home group: unknown"), "{}", c[1]);
    }

    #[test]
    fn adding_starts_in_its_own_view() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        let shown = render(&fx.app, 80, 20);
        assert!(
            shown
                .lines()
                .next()
                .unwrap_or("")
                .contains("new intent · in skoolscout"),
            "{shown}"
        );
        assert!(shown.contains("r refactor"), "{shown}");
        assert!(
            !shown.contains("In the inbox") && !shown.contains("Alpha"),
            "no tree: {shown}"
        );
        let mut fx = setup("agy.jsonl");
        fx.app.start_capture();
        let shown = render(&fx.app, 80, 20);
        assert!(shown.contains("r refactor"), "{shown}");
        assert!(
            !shown.contains("In the inbox") && !shown.contains("Alpha"),
            "no tree: {shown}"
        );
    }

    #[test]
    fn the_interview_has_its_own_view() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        let shown = render(&fx.app, 80, 20);
        assert!(
            shown
                .lines()
                .next()
                .unwrap_or("")
                .contains("bug · in skoolscout"),
            "{shown}"
        );
        assert!(
            !shown.contains("In the inbox") && !shown.contains("Alpha"),
            "no tree: {shown}"
        );
        let mut fx = setup("agy.jsonl");
        key(&mut fx.app, KeyCode::Char('v'));
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('i'));
        let shown = render(&fx.app, 80, 20);
        assert!(
            !shown.contains("Alpha") && !shown.contains("Beta"),
            "no cards: {shown}"
        );
    }

    #[test]
    fn interview_review_snapshot() {
        let mut fx = setup("agy.jsonl");
        fx.app
            .store
            .create_group(Path::new("skoolscout"), "global")
            .unwrap();
        fx.app.rescan();
        agent_replies(&mut fx, &[RESTATE, "enough", WRITE_UP]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "review");
        insta::assert_snapshot!(render(&fx.app, 80, 20));
    }

    #[test]
    fn capture_mode_closes_when_the_interview_is_discarded() {
        let mut fx = setup("agy.jsonl");
        fx.app.start_capture();
        assert_eq!(key(&mut fx.app, KeyCode::Char('b')), Flow::Continue);
        assert_eq!(fx.app.view().screen, "interview");
        assert_eq!(key(&mut fx.app, KeyCode::Esc), Flow::Quit);
    }

    /// After the kind is picked: the words, "that's right", then whatever the
    /// queued replies lead to (RESTATE, "enough", a write-up → review).
    fn talk_to_review(fx: &mut Fx, write_up: &str) {
        agent_replies(fx, &[RESTATE, "enough", write_up]);
        type_str(&mut fx.app, WORDS);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "review", "{:?}", fx.app.view().hint);
    }

    fn global_group(fx: &mut Fx) {
        fx.app
            .store
            .create_group(Path::new("skoolscout"), "global")
            .unwrap();
        fx.app.rescan();
    }

    #[test]
    fn enter_saves_the_write_up_as_a_refining_intent() {
        let mut fx = setup("agy.jsonl");
        global_group(&mut fx);
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        talk_to_review(&mut fx, WRITE_UP);
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.view().screen, "browse");
        let it = fx.app.selected_intent().unwrap().clone();
        assert_eq!(
            it.fm.title,
            "Tenant admins are emailed when anyone accepts a team invite"
        );
        assert_eq!(
            it.path.parent().unwrap(),
            fx.app.store.root().join("skoolscout")
        );
        assert_eq!(
            (it.fm.kind.as_str(), it.fm.state),
            ("issue", State::Refining)
        );
        assert_eq!(it.fm.areas, ["skoolscout/global"]);
        assert!(
            it.fm
                .polish_dir
                .as_deref()
                .unwrap_or("")
                .starts_with("new-"),
            "{:?}",
            it.fm.polish_dir
        );
        assert_eq!(
            it.fm.conversations.get("fake").map(String::as_str),
            Some("conv-int")
        );
        let chat = fs::read_to_string(it.transcript_path()).unwrap();
        assert!(chat.contains(WORDS), "{chat}");
        assert!(
            crate::store::new_criteria(&it).is_empty(),
            "the owner's own write-up has no unseen criteria"
        );
    }

    #[test]
    fn tab_changes_the_type_before_saving() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        talk_to_review(&mut fx, WRITE_UP);
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.view().interview.unwrap().save_kind, "refactor");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.selected_intent().unwrap().fm.kind, "refactor");
    }

    #[test]
    fn typing_at_the_write_up_refines_it() {
        let mut fx = setup("agy.jsonl");
        global_group(&mut fx);
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        talk_to_review(&mut fx, WRITE_UP);
        let moved = WRITE_UP.replace("group: skoolscout\n", "group: skoolscout/global\n");
        agent_replies(&mut fx, &[&moved]);
        type_str(&mut fx.app, "file it under skoolscout/global");
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        let last = c.last().unwrap();
        assert!(last.contains("file it under skoolscout/global"), "{last}");
        assert!(last.contains(crate::interview::WRITE_UP_MARKER), "{last}");
        assert_eq!(awaiting(&fx.app), "review");
        key(&mut fx.app, KeyCode::Enter);
        let it = fx.app.selected_intent().unwrap().clone();
        assert_eq!(
            it.path.parent().unwrap(),
            fx.app.store.root().join("skoolscout/global")
        );
        assert!(it.fm.areas.is_empty(), "{:?}", it.fm.areas);
    }

    #[test]
    fn an_unknown_group_falls_back_to_where_a_adds_today() {
        let mut fx = setup("agy.jsonl");
        key(&mut fx.app, KeyCode::Char('v'));
        list_goto(&mut fx.app, "Beta");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('i'));
        talk_to_review(
            &mut fx,
            &WRITE_UP.replace("group: skoolscout\n", "group: nowhere\n"),
        );
        key(&mut fx.app, KeyCode::Enter);
        let it = fx.app.selected_intent().unwrap().clone();
        assert_eq!(
            it.path.parent().unwrap(),
            fx.app.store.root().join("skoolscout")
        );
        assert_eq!(it.fm.kind, "idea");
    }

    #[test]
    fn p_after_saving_resumes_the_interview_conversation() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        talk_to_review(&mut fx, WRITE_UP);
        key(&mut fx.app, KeyCode::Enter);
        let it = fx.app.selected_intent().unwrap().clone();
        key(&mut fx.app, KeyCode::Char('p'));
        wait_until(&mut fx.app, "turn", |a| !a.view().turn_running);
        let log = fs::read_to_string(&fx.log).unwrap();
        let i = log.rfind("--prompt=").unwrap();
        assert!(
            log[..i].trim_end().ends_with("--resume=conv-int"),
            "p resumes the interview's conversation"
        );
        assert!(
            !fx.app.store.root().join(".polish").join(it.stem()).exists(),
            "and runs in the interview's folder"
        );
    }

    #[test]
    fn an_empty_list_falls_back_to_the_workspace_group() {
        let mut fx = setup("agy.jsonl");
        fx.app.cfg.scope = Some("tourneyseason".into());
        fx.app.reload_tree();
        key(&mut fx.app, KeyCode::Char('v'));
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('i'));
        talk_to_review(&mut fx, &WRITE_UP.replace("group: skoolscout\n", ""));
        key(&mut fx.app, KeyCode::Enter);
        let dir = fx.app.store.root().join("tourneyseason");
        assert_eq!(md_files(&dir), 1, "saved in {}", dir.display());
    }

    #[test]
    fn adding_in_a_new_workspace_creates_its_group() {
        let mut fx = setup("agy.jsonl");
        fx.app.cfg.scope = Some("tourneyseason".into());
        fx.app.reload_tree();
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('f'));
        talk_to_review(&mut fx, &WRITE_UP.replace("group: skoolscout\n", ""));
        key(&mut fx.app, KeyCode::Enter);
        let path = fx.app.selected_intent().unwrap().path.clone();
        assert!(
            path.starts_with(fx.app.store.root().join("tourneyseason")),
            "{}",
            path.display()
        );
    }

    #[test]
    fn the_popup_closes_after_saving() {
        let mut fx = setup("agy.jsonl");
        fx.app.start_capture();
        key(&mut fx.app, KeyCode::Char('b'));
        talk_to_review(&mut fx, WRITE_UP);
        assert_eq!(key(&mut fx.app, KeyCode::Enter), Flow::Quit);
    }

    /// Make reply `n` (1-based) of the queued replies an empty, failing one.
    fn reply_fails(fx: &Fx, n: usize) {
        let dir = fx
            .app
            .provider_env
            .iter()
            .find(|(k, _)| k == "FAKE_EVENTS_DIR")
            .map(|(_, v)| PathBuf::from(v))
            .unwrap();
        fs::write(dir.join(format!("{n}.jsonl")), "").unwrap();
    }

    #[test]
    fn ctrl_s_after_a_failure_saves_the_words_raw() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "question: Which notification?", ""]);
        reply_fails(&fx, 3);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter); // that's right
        wait_turn(&mut fx.app);
        type_str(&mut fx.app, "both");
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "retry");
        fx.app
            .key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(fx.app.view().screen, "browse");
        let it = fx.app.selected_intent().unwrap().clone();
        assert_eq!(it.fm.title, WORDS);
        assert_eq!((it.fm.kind.as_str(), it.fm.state), ("issue", State::Raw));
        assert!(it.body.contains(WORDS), "{}", it.body);
        assert!(it.body.contains("→ both"), "{}", it.body);
        assert_eq!(
            it.path.parent().unwrap(),
            fx.app.store.root().join("skoolscout")
        );
    }

    #[test]
    fn esc_on_a_running_turn_cancels_into_retry_and_keeps_the_words() {
        let mut fx = setup("agy.jsonl");
        let before = md_files(fx.app.store.root());
        agent_replies(&mut fx, &[RESTATE]);
        // a turn still in progress: the fake never sends its result line
        fx.app
            .provider_env
            .push(("FAKE_MODE".into(), "midturn".into()));
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        type_str(&mut fx.app, WORDS);
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(awaiting(&fx.app), "turn");
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(awaiting(&fx.app), "retry");
        assert!(
            fx.app.view().hint.unwrap_or_default().contains("cancelled"),
            "{:?}",
            fx.app.view().hint
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.view().screen, "browse");
        assert_eq!(md_files(fx.app.store.root()), before);
        assert!(new_folders(fx.app.store.root()).is_empty());
    }

    #[test]
    fn m_after_a_failure_picks_another_agent_and_returns() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &["", RESTATE]);
        reply_fails(&fx, 1);
        start_bug_in_skoolscout(&mut fx);
        assert_eq!(awaiting(&fx.app), "retry");
        key(&mut fx.app, KeyCode::Char('m'));
        assert_eq!(fx.app.view().screen, "agent-pick");
        let shown = render(&fx.app, 80, 20);
        assert!(
            !shown.contains("In the inbox") && !shown.contains("Alpha"),
            "still its own view: {shown}"
        );
        key(&mut fx.app, KeyCode::Enter); // fake, preselected
        key(&mut fx.app, KeyCode::Char('j'));
        key(&mut fx.app, KeyCode::Enter); // model-b
        assert_eq!(fx.app.view().screen, "interview");
        assert_eq!(awaiting(&fx.app), "retry");
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let log = fs::read_to_string(&fx.log).unwrap();
        let i = log.rfind("--prompt=").unwrap();
        let head = &log[..i];
        let start = head.rfind('\n').map(|j| j + 1).unwrap_or(0);
        assert!(
            head[start..].contains("--model model-b"),
            "the retry runs on the chosen model: {}",
            &head[start..]
        );
        assert_eq!(awaiting(&fx.app), "confirm");
    }

    #[test]
    fn switching_agent_after_a_turn_starts_a_fresh_conversation() {
        let mut fx = setup("agy.jsonl");
        let mut other = fake_spec();
        other.name = "fake2".into();
        fx.app.cfg.providers.insert("fake2".into(), other);
        agent_replies(&mut fx, &[RESTATE, "", RESTATE]);
        reply_fails(&fx, 2);
        start_bug_in_skoolscout(&mut fx); // conv-int from the first agent
        key(&mut fx.app, KeyCode::Enter); // that's right → the ask fails
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "retry");
        key(&mut fx.app, KeyCode::Char('m'));
        key(&mut fx.app, KeyCode::Char('j')); // fake → fake2
        key(&mut fx.app, KeyCode::Enter);
        key(&mut fx.app, KeyCode::Enter); // its first model
        assert_eq!(fx.app.view().screen, "interview");
        key(&mut fx.app, KeyCode::Enter); // retry on fake2
        wait_turn(&mut fx.app);
        let log = fs::read_to_string(&fx.log).unwrap();
        let i = log.rfind("--prompt=").unwrap();
        let head = &log[..i];
        let start = head.rfind('\n').map(|j| j + 1).unwrap_or(0);
        assert!(
            !head[start..].contains("--resume="),
            "another agent cannot resume the first one's conversation: {}",
            &head[start..]
        );
        assert!(
            calls(&fx).last().unwrap().contains(WORDS),
            "so it gets everything"
        );
    }

    #[test]
    fn the_agents_open_questions_are_kept() {
        let mut fx = setup("agy.jsonl");
        let write_up =
            WRITE_UP.replace("questions: none\n", "questions:\n- Which email template?\n");
        agent_replies(&mut fx, &[RESTATE, "question: Who gets it?", &write_up]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        fx.app
            .key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)); // skip it, write up
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "review");
        let shown = render(&fx.app, 100, 40);
        assert!(
            shown.contains("Which email template?"),
            "shown at review: {shown}"
        );
        key(&mut fx.app, KeyCode::Enter);
        let it = fx.app.selected_intent().unwrap().clone();
        assert!(it.body.contains("Which email template?"), "{}", it.body);
        assert!(
            it.body.contains("Who gets it?"),
            "the skipped one too: {}",
            it.body
        );
        assert_eq!(it.open_questions(), 2, "{}", it.body);
    }

    #[test]
    fn ctrl_s_after_a_failed_refinement_keeps_the_write_up() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        talk_to_review(&mut fx, WRITE_UP);
        agent_replies(&mut fx, &[""]);
        reply_fails(&fx, 1);
        type_str(&mut fx.app, "shorten it");
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "retry");
        fx.app
            .key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let it = fx.app.selected_intent().unwrap().clone();
        assert_eq!(
            it.fm.title,
            "Tenant admins are emailed when anyone accepts a team invite"
        );
        assert!(it.body.contains("only the inviter"), "{}", it.body);
        assert!(
            it.body.contains("shorten it"),
            "the unsent request is kept: {}",
            it.body
        );
        assert_eq!(it.fm.state, State::Refining);
    }

    #[test]
    fn quitting_mid_interview_stops_the_agent_and_leaves_no_folder() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE]);
        fx.app
            .provider_env
            .push(("FAKE_MODE".into(), "midturn".into()));
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        type_str(&mut fx.app, WORDS);
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(awaiting(&fx.app), "turn");
        assert_eq!(new_folders(fx.app.store.root()).len(), 1);
        fx.app.shutdown();
        assert!(new_folders(fx.app.store.root()).is_empty());
        if let Screen::Interview(st) = &fx.app.screen {
            assert!(st.turn.is_none(), "the agent is stopped");
        }
    }

    #[test]
    fn ctrl_d_keeps_a_typed_answer_or_correction() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "question: Who?", WRITE_UP]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        type_str(&mut fx.app, "only the inviter");
        fx.app
            .key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert!(
            c.last().unwrap().contains("only the inviter"),
            "{}",
            c.last().unwrap()
        );

        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, WRITE_UP]);
        start_bug_in_skoolscout(&mut fx);
        type_str(&mut fx.app, "only admins");
        fx.app
            .key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert!(
            c.last().unwrap().contains("only admins"),
            "{}",
            c.last().unwrap()
        );
    }

    #[test]
    fn long_words_wrap_in_the_input_box() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        let words: String =
            (0..30).map(|i| format!("word{i} ")).collect::<String>() + "the-very-end";
        type_str(&mut fx.app, &words);
        let shown = render(&fx.app, 80, 20);
        assert!(shown.contains("word0"), "{shown}");
        assert!(
            shown.contains("the-very-end"),
            "the whole text shows, wrapped: {shown}"
        );
    }

    #[test]
    fn retry_ignores_typing_so_the_next_enter_does_what_it_says() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &["", RESTATE]);
        reply_fails(&fx, 1);
        start_bug_in_skoolscout(&mut fx);
        assert_eq!(awaiting(&fx.app), "retry");
        type_str(&mut fx.app, "shorter");
        assert_eq!(
            fx.app.view().chat_input,
            "",
            "nothing to type into after a failure"
        );
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "confirm", "Enter retried");
    }

    #[test]
    fn a_save_failure_is_shown_in_the_add_view() {
        use std::os::unix::fs::PermissionsExt;
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('b'));
        talk_to_review(&mut fx, WRITE_UP);
        let dir = fx.app.store.root().join("skoolscout");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        key(&mut fx.app, KeyCode::Enter);
        let shown = render(&fx.app, 100, 20);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(awaiting(&fx.app), "review", "still at the write-up");
        let status = shown.lines().last().unwrap_or_default().to_string();
        assert!(status.contains("save failed"), "{status}");
    }

    #[test]
    fn an_empty_reply_is_a_failure_not_a_blank_turn() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[""]); // a result line, but no text
        start_bug_in_skoolscout(&mut fx);
        assert_eq!(awaiting(&fx.app), "retry");
        assert!(
            fx.app.view().hint.unwrap_or_default().contains("no reply"),
            "{:?}",
            fx.app.view().hint
        );
    }

    #[test]
    fn an_ask_reply_that_is_already_the_write_up_goes_to_review() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, WRITE_UP]);
        start_bug_in_skoolscout(&mut fx);
        key(&mut fx.app, KeyCode::Enter); // that's right → the agent jumps ahead
        wait_turn(&mut fx.app);
        assert_eq!(awaiting(&fx.app), "review");
        assert_eq!(calls(&fx).len(), 2, "no extra write-up turn");
    }

    #[test]
    fn a_retry_after_an_unknown_provider_resends_the_step_it_failed() {
        let mut fx = setup("agy.jsonl");
        agent_replies(&mut fx, &[RESTATE, "question: Q?"]);
        start_bug_in_skoolscout(&mut fx);
        let spec = fx.app.cfg.providers.remove("fake").unwrap();
        key(&mut fx.app, KeyCode::Enter); // that's right → the ask cannot start
        assert_eq!(awaiting(&fx.app), "retry");
        fx.app.cfg.providers.insert("fake".into(), spec);
        key(&mut fx.app, KeyCode::Enter);
        wait_turn(&mut fx.app);
        let c = calls(&fx);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(
            c[1].contains("question 1 of at most 5"),
            "the ask, not the restatement: {}",
            c[1]
        );
    }

    #[test]
    fn capital_d_trashes_an_unsent_intent_after_asking() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('D'));
        let v = fx.app.view();
        assert_eq!(v.screen, "prompt");
        assert!(v.hint.unwrap_or_default().contains("Delete \"Alpha\"?"));
        key(&mut fx.app, KeyCode::Char('y'));
        let v = fx.app.view();
        assert_eq!(v.screen, "browse");
        assert!(!v.rows.iter().any(|r| r.label == "Alpha"), "{:?}", v.rows);
        assert!(v.message.unwrap_or_default().contains(".trash"));
        let trashed = fs::read_dir(fx.app.store.root().join(".trash"))
            .unwrap()
            .count();
        assert_eq!(trashed, 1);
    }

    #[test]
    fn anything_but_y_keeps_the_intent() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        key(&mut fx.app, KeyCode::Char('D'));
        key(&mut fx.app, KeyCode::Char('n'));
        assert_eq!(fx.app.view().screen, "browse");
        assert!(fx.app.view().rows.iter().any(|r| r.label == "Alpha"));
        assert!(!fx.app.store.root().join(".trash").exists());
    }

    #[test]
    fn a_sent_intent_is_not_deleted() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let path = fx.app.selected_intent().unwrap().path.clone();
        Store::update_fields(&path, |fm| {
            fm.state = State::Sent;
            fm.sent_to = Some(crate::store::SentTo {
                handoff: "grill".into(),
                workspace_id: "w5".into(),
                agent: "intent-alpha-x1".into(),
                at: chrono::Local::now(),
            });
        })
        .unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('D'));
        let v = fx.app.view();
        assert_eq!(v.screen, "browse", "no question asked");
        assert!(v.message.unwrap_or_default().contains("intent-alpha-x1"));
        assert!(path.exists());
    }

    #[test]
    fn capital_d_works_in_the_card_view_too() {
        let mut fx = setup("agy.jsonl");
        key(&mut fx.app, KeyCode::Char('v'));
        list_goto(&mut fx.app, "Beta");
        key(&mut fx.app, KeyCode::Char('D'));
        assert_eq!(fx.app.view().screen, "prompt");
        key(&mut fx.app, KeyCode::Char('y'));
        assert_eq!(fx.app.view().layout, "list", "stays in the card view");
        assert!(!fx.app.list.iter().any(|i| i.fm.title == "Beta"));
    }

    #[test]
    fn capture_mode_cancelled_at_the_kind_question_closes() {
        let mut fx = setup("agy.jsonl");
        fx.app.start_capture();
        assert_eq!(key(&mut fx.app, KeyCode::Esc), Flow::Quit);
    }

    #[test]
    fn applying_a_bug_heading_files_the_intent_as_a_bug() {
        let mut fx = setup("agy.jsonl");
        let titled = fx._tmp.path().join("bug.jsonl");
        fs::write(&titled, r#"{"event":"init","conversation_id":"c3"}
{"event":"step_update","step_update":{"conversation_id":"c3","step_type":"agent_response","text_delta":"bug: Saving twice duplicates the folder when offline\n\n```proposal\n## Description\nnew body\n```\nquestions: none\n"}}
{"event":"result","result":{"conversation_id":"c3","status":"SUCCESS"}}
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
            "Saving twice duplicates the folder when offline"
        );
        assert_eq!(it.fm.kind, "issue", "polish judged it a bug");
    }

    #[test]
    fn p_runs_in_the_remembered_polish_folder() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "Alpha");
        let it = fx.app.selected_intent().unwrap().clone();
        // written through `extra` so this test compiles before the field exists
        Store::update_fields(&it.path, |fm| {
            fm.extra
                .insert("polish_dir".into(), serde_yaml_ng::Value::from("new-x"));
        })
        .unwrap();
        fx.app.rescan();
        key(&mut fx.app, KeyCode::Char('p'));
        wait_until(&mut fx.app, "turn", |a| !a.view().turn_running);
        let polish = fx.app.store.root().join(".polish");
        assert!(
            polish.join("new-x").is_dir(),
            "runs in the remembered folder"
        );
        assert!(
            !polish.join(it.stem()).exists(),
            "not in a folder named after the file"
        );
    }

    #[test]
    fn k_cycles_bug_refactor_feature_idea() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "In the inbox"); // an issue
        key(&mut fx.app, KeyCode::Char('K'));
        assert_eq!(fx.app.selected_intent().unwrap().fm.kind, "refactor");
        key(&mut fx.app, KeyCode::Char('K'));
        assert_eq!(fx.app.selected_intent().unwrap().fm.kind, "enhancement");
    }

    #[test]
    fn r_in_the_kind_question_starts_a_refactor() {
        let mut fx = setup("agy.jsonl");
        select(&mut fx.app, "skoolscout");
        key(&mut fx.app, KeyCode::Char('a'));
        key(&mut fx.app, KeyCode::Char('r'));
        let v = fx.app.view();
        assert_eq!(v.screen, "interview");
        assert_eq!(v.interview.unwrap().save_kind, "refactor");
    }

    #[test]
    fn the_title_prompt_matches_the_kind() {
        assert!(
            title_label("issue").contains("bug"),
            "{}",
            title_label("issue")
        );
        assert!(title_label("enhancement").contains("feature"));
        assert!(title_label("enhancement").contains("who can do what"));
        assert!(title_label("idea").contains("idea"));
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
