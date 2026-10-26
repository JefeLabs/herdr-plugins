//! Intents on disk: Markdown files with YAML front matter in a directory tree.
//!
//! `<root>/<group>/.../<YYYY-MM-DD-slug>.md`, attachments in `<stem>.assets/`,
//! the refinement transcript in `<stem>.chat.md`. Groups are directories.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

const SLUG_MAX: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Raw,
    Refining,
    Ready,
    Sent,
}

impl<'de> Deserialize<'de> for State {
    /// Case-insensitive; anything unknown reads as `raw` rather than failing the file.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "refining" => State::Refining,
            "ready" => State::Ready,
            "sent" => State::Sent,
            _ => State::Raw,
        })
    }
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Raw => "raw",
            State::Refining => "refining",
            State::Ready => "ready",
            State::Sent => "sent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentTo {
    pub handoff: String,
    pub workspace_id: String,
    pub agent: String,
    pub at: DateTime<Local>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontMatter {
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub state: State,
    #[serde(default = "now", deserialize_with = "lenient_date")]
    pub created: DateTime<Local>,
    /// `None` on disk means "unknown": conflict checks fall back to the file mtime.
    #[serde(default, deserialize_with = "lenient_opt_date")]
    pub updated: Option<DateTime<Local>>,
    #[serde(default)]
    pub repo: Option<PathBuf>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "lenient_opt_date")]
    pub polished_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub sent_to: Option<SentTo>,
    #[serde(default)]
    pub conversations: BTreeMap<String, String>,
    /// Other groups this capability affects (group paths under the root,
    /// e.g. `skoolscout/org/candidates`); it lives in one, appears in each.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "one_or_many"
    )]
    pub areas: Vec<String>,
    /// Fields we do not know, preserved verbatim on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_kind() -> String {
    "idea".to_string()
}

fn now() -> DateTime<Local> {
    Local::now()
}

/// `tags: a`, `tags: [a, b]`, `tags: [2024, x]` and an empty `tags:` all load.
fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = Value::deserialize(d)?;
    fn scalar(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
    Ok(match &v {
        Value::Null => Vec::new(),
        Value::Sequence(items) => items.iter().filter_map(scalar).collect(),
        other => scalar(other).into_iter().collect(),
    })
}

/// RFC 3339, `YYYY-MM-DD HH:MM:SS`, or a bare `YYYY-MM-DD` (midnight local).
fn parse_date(text: &str) -> Option<DateTime<Local>> {
    let t = text.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(t) {
        return Some(d.with_timezone(&Local));
    }
    if let Ok(n) = chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S") {
        return n.and_local_timezone(Local).single();
    }
    if let Ok(n) = chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S") {
        return n.and_local_timezone(Local).single();
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return d
            .and_hms_opt(0, 0, 0)
            .and_then(|n| n.and_local_timezone(Local).single());
    }
    None
}

fn lenient_opt_date<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<DateTime<Local>>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::String(s) => parse_date(&s),
        Value::Null => None,
        other => parse_date(&serde_yaml_ng::to_string(&other).unwrap_or_default()),
    })
}

fn lenient_date<'de, D: serde::Deserializer<'de>>(d: D) -> Result<DateTime<Local>, D::Error> {
    Ok(lenient_opt_date(d)?.unwrap_or_else(now))
}

#[derive(Debug, Clone)]
pub struct Intent {
    pub path: PathBuf,
    pub fm: FrontMatter,
    pub body: String,
    /// `updated` as read from disk, for conflict detection on save.
    pub loaded_updated: DateTime<Local>,
}

impl Intent {
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }
    pub fn assets_dir(&self) -> PathBuf {
        self.path.with_file_name(format!("{}.assets", self.stem()))
    }
    pub fn transcript_path(&self) -> PathBuf {
        self.path.with_file_name(format!("{}.chat.md", self.stem()))
    }
    pub fn open_questions(&self) -> usize {
        open_questions(&self.body)
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    pub path: PathBuf,
    pub name: String,
    pub groups: Vec<Group>,
    pub intents: Vec<Intent>,
    /// Markdown files that failed to parse: `(path, error)`, shown rather than dropped.
    pub broken: Vec<(PathBuf, String)>,
}

#[derive(Debug)]
pub enum SaveError {
    /// The file's `updated` on disk differs from the one we loaded.
    Conflict,
    Io(anyhow::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Conflict => write!(f, "file changed on disk since it was loaded"),
            SaveError::Io(e) => write!(f, "{e:#}"),
        }
    }
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open (creating) the root and its `Inbox`.
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn abs(&self, group: &Path) -> PathBuf {
        if group.as_os_str().is_empty() {
            self.root.clone()
        } else {
            self.root.join(group)
        }
    }

    pub fn scan(&self) -> anyhow::Result<Group> {
        self.scan_dir(&self.root, String::new())
    }

    fn scan_dir(&self, dir: &Path, name: String) -> anyhow::Result<Group> {
        let mut groups = Vec::new();
        let mut intents = Vec::new();
        let mut broken = Vec::new();
        for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if fname.ends_with(".assets") {
                    continue;
                }
                groups.push(self.scan_dir(&path, fname)?);
            } else if fname.ends_with(".md") && !fname.ends_with(".chat.md") {
                match Store::load(&path) {
                    Ok(it) => intents.push(it),
                    Err(e) => broken.push((path.clone(), format!("{e:#}"))),
                }
            }
        }
        groups.sort_by_key(|g| g.name.to_lowercase());
        intents.sort_by_key(|i| i.fm.title.to_lowercase());
        broken.sort();
        Ok(Group {
            path: dir.to_path_buf(),
            name,
            groups,
            intents,
            broken,
        })
    }

    pub fn create(
        &self,
        group: &Path,
        title: &str,
        kind: &str,
        body: &str,
        repo: Option<PathBuf>,
    ) -> anyhow::Result<Intent> {
        let dir = self.abs(group);
        std::fs::create_dir_all(&dir)?;
        let date = Local::now().format("%Y-%m-%d");
        let mut stem = format!("{date}-{}", slugify(title));
        let mut path = dir.join(format!("{stem}.md"));
        let mut n = 2;
        while path.exists() {
            stem = format!("{date}-{}-{n}", slugify(title));
            path = dir.join(format!("{stem}.md"));
            n += 1;
        }
        let ts = Local::now();
        let fm = FrontMatter {
            title: title.to_string(),
            kind: kind.to_string(),
            state: State::Raw,
            created: ts,
            updated: Some(ts),
            repo,
            tags: Vec::new(),
            polished_at: None,
            sent_to: None,
            conversations: BTreeMap::new(),
            areas: Vec::new(),
            extra: BTreeMap::new(),
        };
        let mut it = Intent {
            path,
            fm,
            body: body.to_string(),
            loaded_updated: ts,
        };
        write_intent(&it)?;
        it.loaded_updated = stamp(&it);
        Ok(it)
    }

    pub fn load(path: &Path) -> anyhow::Result<Intent> {
        let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let text = String::from_utf8_lossy(&raw)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let (fm_text, body) = split_front_matter(&text)
            .with_context(|| format!("{}: no front matter", path.display()))?;
        let mut fm: FrontMatter = serde_yaml_ng::from_str(fm_text)
            .with_context(|| format!("{}: front matter", path.display()))?;
        if fm.title.trim().is_empty() {
            fm.title = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "untitled".into());
        }
        let mut it = Intent {
            path: path.to_path_buf(),
            loaded_updated: Local::now(),
            fm,
            body: body.to_string(),
        };
        it.loaded_updated = stamp(&it);
        Ok(it)
    }

    /// Save, refusing if the file changed on disk since it was loaded.
    pub fn save(intent: &mut Intent) -> Result<(), SaveError> {
        if let Ok(on_disk) = Store::load(&intent.path)
            && stamp(&on_disk).timestamp() != intent.loaded_updated.timestamp()
        {
            return Err(SaveError::Conflict);
        }
        Store::save_force(intent).map_err(SaveError::Io)
    }

    pub fn save_force(intent: &mut Intent) -> anyhow::Result<()> {
        intent.fm.updated = Some(Local::now());
        write_intent(intent)?;
        intent.loaded_updated = stamp(intent);
        Ok(())
    }

    /// Reload the file, change front matter fields, save (bumping `updated`).
    /// The way every field-only change (state, kind, sent_to) must be written,
    /// so a stale in-memory copy never clobbers another writer's body.
    pub fn update_fields(path: &Path, f: impl FnOnce(&mut FrontMatter)) -> anyhow::Result<Intent> {
        let mut it = Store::load(path)?;
        f(&mut it.fm);
        Store::save_force(&mut it)?;
        Ok(it)
    }

    /// Like `update_fields` but without bumping `updated`: for bookkeeping such
    /// as a conversation id, which must not look like an edit to other writers.
    pub fn update_meta(path: &Path, f: impl FnOnce(&mut FrontMatter)) -> anyhow::Result<Intent> {
        let mut it = Store::load(path)?;
        f(&mut it.fm);
        write_intent(&it)?;
        it.loaded_updated = stamp(&it);
        Ok(it)
    }

    pub fn move_intent(&self, intent: &Intent, to_group: &Path) -> anyhow::Result<Intent> {
        let dir = self.abs(to_group);
        std::fs::create_dir_all(&dir)?;
        let stem = intent.stem();
        let dest = dir.join(format!("{stem}.md"));
        if dest.exists() {
            bail!("{} already exists", dest.display());
        }
        std::fs::rename(&intent.path, &dest)?;
        let assets = intent.assets_dir();
        if assets.is_dir() {
            std::fs::rename(&assets, dir.join(format!("{stem}.assets")))?;
        }
        let chat = intent.transcript_path();
        if chat.is_file() {
            std::fs::rename(&chat, dir.join(format!("{stem}.chat.md")))?;
        }
        Store::load(&dest)
    }

    pub fn create_group(&self, parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
        let dir = self.abs(parent).join(name);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn move_group(&self, group: &Path, to_parent: &Path) -> anyhow::Result<PathBuf> {
        let from = self.abs(group);
        let name = from.file_name().context("group has no name")?.to_owned();
        let dest = self.abs(to_parent).join(name);
        if dest.exists() {
            bail!("{} already exists", dest.display());
        }
        std::fs::rename(&from, &dest)?;
        Ok(dest)
    }

    /// Copy a file into the intent's assets folder; returns the copy's path.
    pub fn attach(intent: &Intent, file: &Path) -> anyhow::Result<PathBuf> {
        let dir = intent.assets_dir();
        std::fs::create_dir_all(&dir)?;
        let name = file.file_name().context("attachment has no file name")?;
        let dest = dir.join(name);
        std::fs::copy(file, &dest).with_context(|| format!("copying {}", file.display()))?;
        Ok(dest)
    }

    pub fn attachments(intent: &Intent) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(intent.assets_dir()) {
            for e in rd.flatten() {
                if let Ok(meta) = e.metadata()
                    && meta.is_file()
                {
                    out.push((e.file_name().to_string_lossy().to_string(), meta.len()));
                }
            }
        }
        out.sort();
        out
    }

    pub fn append_transcript(intent: &Intent, role: &str, text: &str) -> anyhow::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(intent.transcript_path())?;
        writeln!(
            f,
            "### {role} {}\n\n{}\n",
            Local::now().format("%Y-%m-%d %H:%M"),
            text.trim_end()
        )?;
        Ok(())
    }
}

/// The change stamp used for conflict detection: `updated` when present, else
/// the file's mtime (hand-written files often lack `updated`).
/// When the capability last changed: its `updated`, else the file's mtime.
pub fn stamp(intent: &Intent) -> DateTime<Local> {
    if let Some(u) = intent.fm.updated {
        return u;
    }
    std::fs::metadata(&intent.path)
        .and_then(|m| m.modified())
        .map(DateTime::<Local>::from)
        .unwrap_or_else(|_| Local::now())
}

fn write_intent(intent: &Intent) -> anyhow::Result<()> {
    let yaml = serde_yaml_ng::to_string(&intent.fm)?;
    let yaml = normalise_nulls(&yaml);
    let text = format!("---\n{yaml}---\n{}", intent.body);
    std::fs::write(&intent.path, text)
        .with_context(|| format!("writing {}", intent.path.display()))?;
    Ok(())
}

/// serde_yaml_ng writes `~` for None; `null` reads better and matches the spec.
fn normalise_nulls(yaml: &str) -> String {
    yaml.lines()
        .map(|l| {
            if let Some(k) = l.strip_suffix(": ~") {
                format!("{k}: null")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn split_front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end + 1], &rest[end + 5..]))
}

pub fn slugify(title: &str) -> String {
    let mut s = slug::slugify(title);
    if s.len() > SLUG_MAX {
        s.truncate(SLUG_MAX);
        s = s.trim_end_matches('-').to_string();
    }
    if s.is_empty() {
        "untitled".to_string()
    } else {
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub text: String,
    /// The line indented under the question starting with `→`.
    pub answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Criterion {
    pub text: String,
    pub done: bool,
}

pub const ANSWER_MARK: &str = "→";

fn section_lines<'a>(body: &'a str, heading: &str) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let mut in_section = false;
    for (i, line) in body.lines().enumerate() {
        if line.starts_with("## ") {
            in_section = line.trim().eq_ignore_ascii_case(heading);
            continue;
        }
        if in_section {
            out.push((i, line));
        }
    }
    out
}

/// Questions under `## Open questions`: `- text`, optionally followed by an
/// indented `→ answer` line.
pub fn questions(body: &str) -> Vec<Question> {
    let mut out: Vec<Question> = Vec::new();
    for (_, line) in section_lines(body, "## Open questions") {
        let trimmed = line.trim_start();
        if let Some(q) = trimmed.strip_prefix("- ") {
            // a bare `- ` is the template's placeholder, not a question
            if q.trim().is_empty() {
                continue;
            }
            out.push(Question {
                text: q.trim().to_string(),
                answer: None,
            });
        } else if let Some(a) = trimmed.strip_prefix(ANSWER_MARK)
            && line.starts_with(' ')
            && let Some(last) = out.last_mut()
        {
            let a = a.trim();
            if !a.is_empty() {
                last.answer = Some(a.to_string());
            }
        }
    }
    out
}

/// Unanswered questions.
pub fn open_questions(body: &str) -> usize {
    questions(body)
        .iter()
        .filter(|q| q.answer.is_none())
        .count()
}

/// Write (or replace) the answer under question `index`. `None` if out of range.
pub fn answer_question(body: &str, index: usize, answer: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    let mut seen = 0usize;
    let mut q_line: Option<usize> = None;
    for (i, line) in section_lines(body, "## Open questions") {
        if line
            .trim_start()
            .strip_prefix("- ")
            .is_some_and(|q| !q.trim().is_empty())
        {
            if seen == index {
                q_line = Some(i);
                break;
            }
            seen += 1;
        }
    }
    let qi = q_line?;
    let mut out: Vec<String> = lines[..=qi].iter().map(|l| l.to_string()).collect();
    out.push(format!("  {ANSWER_MARK} {}", answer.trim()));
    let mut rest = qi + 1;
    // drop an existing answer line right under the question
    if rest < lines.len()
        && lines[rest].starts_with(' ')
        && lines[rest].trim_start().starts_with(ANSWER_MARK)
    {
        rest += 1;
    }
    out.extend(lines[rest..].iter().map(|l| l.to_string()));
    let mut text = out.join("\n");
    if body.ends_with('\n') {
        text.push('\n');
    }
    Some(text)
}

/// The text after a `- [ ] ` / `- [x] ` marker, if `t` is a checklist line.
fn checkbox_text(t: &str) -> Option<&str> {
    t.strip_prefix("- [ ] ")
        .or_else(|| t.strip_prefix("- [x] "))
        .or_else(|| t.strip_prefix("- [X] "))
}

/// Checklist items (`- [ ]` / `- [x]`) under a `## Heading`.
pub fn checklist(body: &str, heading: &str) -> Vec<Criterion> {
    section_lines(body, heading)
        .into_iter()
        .filter_map(|(_, line)| {
            let t = line.trim_start();
            if checkbox_text(t).is_some_and(|r| r.trim().is_empty()) {
                // `- [ ] ` with nothing after it is the template's placeholder
                return None;
            }
            if let Some(rest) = t.strip_prefix("- [ ] ") {
                Some(Criterion {
                    text: rest.trim().to_string(),
                    done: false,
                })
            } else {
                t.strip_prefix("- [x] ")
                    .or_else(|| t.strip_prefix("- [X] "))
                    .map(|rest| Criterion {
                        text: rest.trim().to_string(),
                        done: true,
                    })
            }
        })
        .collect()
}

/// Flip the checkbox of item `index` under `heading`. `None` if out of range.
pub fn toggle_checklist(body: &str, heading: &str, index: usize) -> Option<String> {
    let mut seen = 0usize;
    let mut target: Option<usize> = None;
    for (i, line) in section_lines(body, heading) {
        let t = line.trim_start();
        if checkbox_text(t).is_some_and(|r| !r.trim().is_empty()) {
            if seen == index {
                target = Some(i);
                break;
            }
            seen += 1;
        }
    }
    let ti = target?;
    let mut lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();
    let l = &lines[ti];
    let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
    let t = l.trim_start();
    lines[ti] = if let Some(rest) = t.strip_prefix("- [ ] ") {
        format!("{indent}- [x] {rest}")
    } else {
        let rest = t
            .strip_prefix("- [x] ")
            .or_else(|| t.strip_prefix("- [X] "))
            .unwrap_or("");
        format!("{indent}- [ ] {rest}")
    };
    let mut text = lines.join("\n");
    if body.ends_with('\n') {
        text.push('\n');
    }
    Some(text)
}

pub const QUESTIONS_HEADING: &str = "## Open questions";

/// Where a capability stands before it is handed off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Raw,
    /// Refining with this many unanswered questions.
    Open(usize),
    Ready,
    Sent,
}

impl Readiness {
    pub fn label(self) -> String {
        match self {
            Readiness::Raw => "raw".into(),
            Readiness::Open(n) => format!("?{n}"),
            Readiness::Ready => "ready".into(),
            Readiness::Sent => "sent".into(),
        }
    }
}

/// Derived from the stored state and the unanswered questions.
pub fn readiness(it: &Intent) -> Readiness {
    match it.fm.state {
        State::Sent => Readiness::Sent,
        State::Raw => Readiness::Raw,
        State::Refining | State::Ready => match it.open_questions() {
            0 => Readiness::Ready,
            n => Readiness::Open(n),
        },
    }
}

/// Why `s` may not send this capability, or `None` when it may: it must be
/// past `raw` and every open question must have an answer.
pub fn send_refusal(it: &Intent) -> Option<String> {
    if it.fm.state == State::Raw {
        return Some("still raw: refine it (p) or mark it ready (R) before sending".into());
    }
    match it.open_questions() {
        0 => None,
        n => Some(format!(
            "{n} open question{} still unanswered: answer {} before sending (Q)",
            if n == 1 { "" } else { "s" },
            if n == 1 { "it" } else { "them" }
        )),
    }
}

/// Section order used when a missing section has to be created.
const SECTION_ORDER: &[&str] = &[
    "## Description",
    "## Preconditions",
    "## Acceptance criteria",
    "## Open questions",
];

/// Append `line` at the end of the `heading` section, creating the section
/// (in canonical order) when it is missing.
fn append_to_section(body: &str, heading: &str, line: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();
    let heading_idx = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(heading));
    let insert_at = match heading_idx {
        Some(h) => {
            // end of this section: just before the next heading, trimming trailing blanks
            let mut end = lines.len();
            for (i, l) in lines.iter().enumerate().skip(h + 1) {
                if l.starts_with("## ") {
                    end = i;
                    break;
                }
            }
            let mut at = end;
            while at > h + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            at
        }
        None => {
            // create the section before the first later section in canonical order
            let my_pos = SECTION_ORDER
                .iter()
                .position(|h| h.eq_ignore_ascii_case(heading))
                .unwrap_or(SECTION_ORDER.len());
            let later = lines.iter().position(|l| {
                l.starts_with("## ")
                    && SECTION_ORDER
                        .iter()
                        .position(|h| h.eq_ignore_ascii_case(l.trim()))
                        .map(|p| p > my_pos)
                        .unwrap_or(false)
            });
            let at = later.unwrap_or(lines.len());
            let mut block = vec![heading.to_string()];
            block.push(line.to_string());
            if at < lines.len() {
                block.push(String::new());
            }
            // keep a blank line before the new heading
            if at > 0 && !lines[at - 1].trim().is_empty() {
                block.insert(0, String::new());
            }
            lines.splice(at..at, block);
            let mut out = lines.join("\n");
            out.push('\n');
            return out;
        }
    };
    lines.insert(insert_at, line.to_string());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Add an unchecked item to a checklist section.
pub fn add_checklist_item(body: &str, heading: &str, text: &str) -> String {
    append_to_section(body, heading, &format!("- [ ] {}", text.trim()))
}

/// Add an open question.
pub fn add_question(body: &str, text: &str) -> String {
    append_to_section(body, QUESTIONS_HEADING, &format!("- {}", text.trim()))
}

pub const ARTIFACTS_HEADING: &str = "## Artifacts";

/// Something generated for the capability elsewhere: a spec, plan, ADR,
/// decision ticket or issue. `target` is a path (relative to the repo) or a
/// `#123` / URL reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub kind: String,
    pub target: String,
}

/// `- <kind>: <target>` lines under `## Artifacts`; a line without a kind is a `link`.
pub fn artifacts(body: &str) -> Vec<Artifact> {
    section_lines(body, ARTIFACTS_HEADING)
        .into_iter()
        .filter_map(|(_, line)| {
            let item = line.trim_start().strip_prefix("- ")?.trim();
            if item.is_empty() {
                return None;
            }
            // "kind: target" where kind is a single word and target is not a URL scheme
            if let Some((k, t)) = item.split_once(':')
                && !k.trim().is_empty()
                && !k.trim().contains(' ')
                && !t.trim().is_empty()
                && !matches!(k.trim(), "http" | "https" | "file")
            {
                return Some(Artifact {
                    kind: k.trim().to_lowercase(),
                    target: t.trim().to_string(),
                });
            }
            Some(Artifact {
                kind: "link".into(),
                target: item.to_string(),
            })
        })
        .collect()
}

pub fn add_artifact(body: &str, kind: &str, target: &str) -> String {
    append_to_section(
        body,
        ARTIFACTS_HEADING,
        &format!("- {}: {}", kind.trim().to_lowercase(), target.trim()),
    )
}

/// Where generated artifacts live in a repo, and the kind each folder implies.
const ARTIFACT_DIRS: &[(&str, &str)] = &[
    ("docs/superpowers/specs", "spec"),
    ("docs/superpowers/plans", "plan"),
    ("docs/adr", "adr"),
];

fn significant_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| {
            w.len() > 3
                && !matches!(
                    *w,
                    "with"
                        | "without"
                        | "from"
                        | "that"
                        | "this"
                        | "into"
                        | "when"
                        | "then"
                        | "them"
                        | "they"
                        | "have"
                        | "design"
                        | "plan"
                        | "implementation"
                )
        })
        .map(str::to_string)
        .collect()
}

fn word_matches(a: &str, b: &str) -> bool {
    a == b || (a.len() >= 4 && b.starts_with(a)) || (b.len() >= 4 && a.starts_with(b))
}

/// Files under the known artifact folders of `repo` whose name or first
/// heading shares at least two significant words with `title`, minus the
/// targets already linked. Paths are relative to the repo.
pub fn discover_artifacts(repo: &Path, title: &str, already: &[String]) -> Vec<Artifact> {
    let wanted = significant_words(title);
    if wanted.is_empty() {
        return Vec::new();
    }
    // two shared words for a normal title; a one-word title matches on that word
    let needed = wanted.len().min(2);
    let mut out = Vec::new();
    for (dir, kind) in ARTIFACT_DIRS {
        let Ok(rd) = std::fs::read_dir(repo.join(dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let rel = format!("{dir}/{}", e.file_name().to_string_lossy());
            if already.iter().any(|a| a.trim() == rel) {
                continue;
            }
            let heading = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| {
                    t.lines()
                        .find(|l| l.starts_with("# "))
                        .map(|l| l.trim_start_matches('#').trim().to_string())
                })
                .unwrap_or_default();
            let haystack: Vec<String> =
                significant_words(&format!("{} {heading}", e.file_name().to_string_lossy()));
            let hits = wanted
                .iter()
                .filter(|w| haystack.iter().any(|h| word_matches(w, h)))
                .count();
            if hits >= needed {
                out.push(Artifact {
                    kind: kind.to_string(),
                    target: rel,
                });
            }
        }
    }
    out.sort_by(|a, b| a.target.cmp(&b.target));
    out
}

pub const CRITERIA_HEADING: &str = "## Acceptance criteria";
pub const PRECONDITIONS_HEADING: &str = "## Preconditions";

/// Checks that prove the capability is done.
pub fn criteria(body: &str) -> Vec<Criterion> {
    checklist(body, CRITERIA_HEADING)
}

pub fn toggle_criterion(body: &str, index: usize) -> Option<String> {
    toggle_checklist(body, CRITERIA_HEADING, index)
}

/// Assumptions that must hold for the capability to be worth building;
/// ticked once confirmed (by the owner or by the receiving agent).
pub fn preconditions(body: &str) -> Vec<Criterion> {
    checklist(body, PRECONDITIONS_HEADING)
}

pub fn toggle_precondition(body: &str, index: usize) -> Option<String> {
    toggle_checklist(body, PRECONDITIONS_HEADING, index)
}

/// Convenience for callers that want the raw mapping (e.g. tests).
pub fn front_matter_mapping(text: &str) -> Option<Mapping> {
    let (fm, _) = split_front_matter(text)?;
    serde_yaml_ng::from_str(fm).ok()
}
