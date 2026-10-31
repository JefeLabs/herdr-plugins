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

/// A follow-up filed for an intent (by the follow-up's stem, which `M` keeps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpLink {
    pub stem: String,
    /// Its shipped change was folded into this intent's criteria.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub folded: bool,
}

/// A missing item filed for one of an intent's criteria (by the item's stem,
/// which `M` keeps, so the link survives moves).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingLink {
    pub stem: String,
    /// The criterion's `criterion_key`.
    pub key: String,
    /// The original re-checked this criterion once the item closed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rechecked: bool,
}

/// On a missing item: what it was filed for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingFor {
    pub stem: String,
    pub key: String,
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
    /// Other groups this intent affects (group paths under the root,
    /// e.g. `skoolscout/org/candidates`); it lives in one, appears in each.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "one_or_many"
    )]
    pub areas: Vec<String>,
    /// The merged pull requests the last reconcile covered (sorted), so each
    /// merged set is reconciled once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconciled_for: Vec<u64>,
    /// Updated by a reconcile and not yet read by the owner (shown as `*`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
    /// Keys (`criterion_key`) of the acceptance criteria the owner has seen.
    /// `None` until first recorded; see `new_criteria` for how that is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<Vec<String>>,
    /// The folder under `<root>/.polish/` its agent conversation runs in,
    /// when that is not the file's stem (an intent added by conversation is
    /// named only at the end; claude resumes only from the folder it began in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polish_dir: Option<String>,
    /// The merged set of pull requests a completion check last ran for, so
    /// each merged set is checked once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checked_for: Vec<u64>,
    /// Keys (`criterion_key`) of the criteria a check ticked (with proof).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check_ticks: Vec<String>,
    /// Keys of check ticks the owner took back: no check ticks them again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owner_unticked: Vec<String>,
    /// The missing items / bugs a check filed for this intent's criteria.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<MissingLink>,
    /// Keys of gaps the owner dismissed (by deleting their item).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissed: Vec<String>,
    /// On a missing item: the intent and criterion it was filed for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_for: Option<MissingFor>,
    /// On a follow-up: the stem of the (first) intent it refines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_of: Option<String>,
    /// The follow-ups filed for this intent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<FollowUpLink>,
    /// On a follow-up: the owner undid its fold, so it never folds again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fold_declined: bool,
    /// On an original: the follow-up the last fold came from (for `u`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fold: Option<String>,
    /// On an original: the criteria a fold changed, still to be checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fold_check: Vec<String>,
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
    /// The folder name under `<root>/.polish/` this intent's agent runs in.
    /// `polish_dir` counts only as one plain folder name: anything that could
    /// point outside `.polish/` (a path, `..`, an absolute path) is ignored.
    pub fn polish_name(&self) -> String {
        match self.fm.polish_dir.as_deref() {
            Some(d) if is_plain_folder(d) => d.to_string(),
            _ => self.stem(),
        }
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

impl Group {
    /// The tree a workspace sees: only the top-level group named `scope`
    /// (matched whatever its case), or an empty one at `<root>/<scope>` until
    /// something is captured there. Nothing else at the top level shows.
    /// `None` keeps the whole tree.
    pub fn scoped(self, scope: Option<&str>) -> Group {
        let Some(name) = scope else {
            return self;
        };
        let mut groups: Vec<Group> = self
            .groups
            .into_iter()
            .filter(|g| g.name.eq_ignore_ascii_case(name))
            .take(1)
            .collect();
        if groups.is_empty() {
            groups.push(Group {
                path: self.path.join(name),
                name: name.to_string(),
                groups: Vec::new(),
                intents: Vec::new(),
                broken: Vec::new(),
            });
        }
        Group {
            path: self.path,
            name: self.name,
            groups,
            intents: Vec::new(),
            broken: Vec::new(),
        }
    }
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

    /// The group (relative to the root) captures for workspace `scope` go
    /// to: an existing top-level folder of that name in any case, so they
    /// land where the tree shows them, else `scope` as written.
    pub fn scope_group(&self, scope: &str) -> PathBuf {
        std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .find(|n| n.eq_ignore_ascii_case(scope))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(scope))
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
            reconciled_for: Vec::new(),
            unread: false,
            acknowledged: None,
            polish_dir: None,
            checked_for: Vec::new(),
            check_ticks: Vec::new(),
            owner_unticked: Vec::new(),
            missing: Vec::new(),
            dismissed: Vec::new(),
            missing_for: None,
            follow_up_of: None,
            follow_ups: Vec::new(),
            fold_declined: false,
            last_fold: None,
            fold_check: Vec::new(),
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

    /// Moves an intent and everything it owns (its file, conversation,
    /// attachments and agent folder) into `<root>/.trash/<stamp>-<stem>/`,
    /// with `from.txt` naming the group it came from, so it can be put back.
    /// `.trash` starts with a dot, so the tree never shows it.
    pub fn trash(&self, intent: &Intent) -> anyhow::Result<PathBuf> {
        let base = self.root().join(".trash");
        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        let stem = intent.stem();
        let mut dest = base.join(format!("{stamp}-{stem}"));
        let mut n = 2;
        while dest.exists() {
            dest = base.join(format!("{stamp}-{stem}-{n}"));
            n += 1;
        }
        std::fs::create_dir_all(&dest)?;
        let from = intent
            .path
            .parent()
            .and_then(|p| p.strip_prefix(self.root()).ok())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        std::fs::write(dest.join("from.txt"), format!("{from}\n"))?;
        for src in [
            intent.path.clone(),
            intent.transcript_path(),
            intent.assets_dir(),
        ] {
            if let (true, Some(name)) = (src.exists(), src.file_name()) {
                std::fs::rename(&src, dest.join(name))?;
            }
        }
        let polish = self.root().join(".polish").join(intent.polish_name());
        if polish.is_dir() {
            std::fs::rename(&polish, dest.join("polish"))?;
        }
        Ok(dest)
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

/// One ordinary folder name: no separators, not `.` or `..`, not empty.
fn is_plain_folder(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    !name.contains(['/', '\\'])
        && matches!(parts.next(), Some(std::path::Component::Normal(_)))
        && parts.next().is_none()
}

/// The change stamp used for conflict detection: `updated` when present, else
/// the file's mtime (hand-written files often lack `updated`).
/// When the intent last changed: its `updated`, else the file's mtime.
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

/// Where an intent stands before it is handed off.
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

/// Why `s` may not send this intent, or `None` when it may: it must be
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
    "## Stories",
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

/// Something generated for the intent elsewhere: a spec, plan, ADR,
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

pub(crate) fn significant_words(text: &str) -> Vec<String> {
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
        .fold(Vec::new(), |mut out: Vec<String>, w| {
            // a word the text repeats counts once
            if !out.iter().any(|o| o == w) {
                out.push(w.to_string());
            }
            out
        })
}

/// Two words match when they share a stem: plurals (contacts, families,
/// boxes), -ing and -ed (meeting, skipped) and a final e (schedule,
/// scheduling) fold away. A word is not matched by every word that merely
/// starts with it ("specific" is not "specification").
pub(crate) fn word_matches(a: &str, b: &str) -> bool {
    stem(a) == stem(b)
}

fn stem(word: &str) -> String {
    let mut w = word.to_string();
    if w.len() > 4 && w.ends_with("ies") {
        w.truncate(w.len() - 3);
        w.push('y');
    } else if ["sses", "xes", "zes", "ches", "shes"]
        .iter()
        .any(|s| w.ends_with(s))
    {
        w.truncate(w.len() - 2);
    } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
        w.pop();
    }
    for suffix in ["ing", "ed"] {
        if w.ends_with(suffix) && w.len() - suffix.len() >= 3 {
            w.truncate(w.len() - suffix.len());
            // skipped -> skip, but install, pass, buzz keep their double
            let b = w.as_bytes();
            if b.len() >= 2 && b[b.len() - 1] == b[b.len() - 2] && !b"lsz".contains(&b[b.len() - 1])
            {
                w.pop();
            }
            break;
        }
    }
    if w.len() > 3 && w.ends_with('e') {
        w.pop();
    }
    w
}

/// Files under the known artifact folders of `repo` whose name and first
/// heading share at least two significant words with `title`, one of them in
/// the file name, minus the targets already linked. Paths are relative to
/// the repo.
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
            let name = significant_words(&e.file_name().to_string_lossy());
            let haystack: Vec<String> =
                significant_words(&format!("{} {heading}", e.file_name().to_string_lossy()));
            let hits: Vec<&String> = wanted
                .iter()
                .filter(|w| haystack.iter().any(|h| word_matches(w, h)))
                .collect();
            // the name must share a word too: a heading alone matches too much
            let in_name = hits.iter().any(|w| name.iter().any(|n| word_matches(w, n)));
            if hits.len() >= needed && in_name {
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

/// The `### ` sub-headings inside Acceptance criteria (one per area, say),
/// each with the index of the first criterion under it.
pub fn criteria_groups(body: &str) -> Vec<(usize, String)> {
    let mut groups = Vec::new();
    let mut count = 0usize;
    for (_, line) in section_lines(body, CRITERIA_HEADING) {
        let t = line.trim_start();
        if let Some(h) = t.strip_prefix("### ") {
            groups.push((count, h.trim().to_string()));
        } else if checkbox_text(t).is_some_and(|r| !r.trim().is_empty()) {
            count += 1;
        }
    }
    groups
}

/// A stable key for a criterion's wording (FNV-1a over its words), so a
/// reworded criterion counts as new while ticking it does not.
pub fn criterion_key(text: &str) -> String {
    let normal = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in normal.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The criteria keys the owner has seen. With no record yet: before sending,
/// every criterion is the owner's own; once sent, those whose wording appears
/// in the refinement transcript (the proposals the owner worked through), or
/// all of them when there is no transcript to tell.
fn seen_keys(it: &Intent) -> Vec<String> {
    if let Some(keys) = &it.fm.acknowledged {
        return keys.clone();
    }
    let all = criteria(&it.body);
    if it.fm.state != State::Sent {
        return all.iter().map(|c| criterion_key(&c.text)).collect();
    }
    // without a transcript nothing tells who wrote what: count them all as seen
    let Ok(transcript) = std::fs::read_to_string(it.transcript_path()) else {
        return all.iter().map(|c| criterion_key(&c.text)).collect();
    };
    let transcript = transcript.split_whitespace().collect::<Vec<_>>().join(" ");
    all.iter()
        .filter(|c| {
            let words = c.text.split_whitespace().collect::<Vec<_>>().join(" ");
            !words.is_empty() && transcript.contains(&words)
        })
        .map(|c| criterion_key(&c.text))
        .collect()
}

/// Acceptance criteria (0-based) added or reworded by someone else (an
/// agent, a reconcile) and not yet acknowledged by the owner.
pub fn new_criteria(it: &Intent) -> Vec<usize> {
    let seen = seen_keys(it);
    criteria(&it.body)
        .iter()
        .enumerate()
        .filter(|(_, c)| !seen.contains(&criterion_key(&c.text)))
        .map(|(i, _)| i)
        .collect()
}

/// Record criteria `indexes` (0-based) as seen, keeping what was already
/// seen, without counting as an edit (`updated` is not bumped).
pub fn acknowledge_criteria(path: &Path, indexes: &[usize]) -> anyhow::Result<Intent> {
    let it = Store::load(path)?;
    let mut seen = seen_keys(&it);
    let all = criteria(&it.body);
    for i in indexes {
        if let Some(c) = all.get(*i) {
            let k = criterion_key(&c.text);
            if !seen.contains(&k) {
                seen.push(k);
            }
        }
    }
    Store::update_meta(path, |fm| fm.acknowledged = Some(seen))
}

/// Record every current criterion as seen (the owner authored or accepted them).
pub fn acknowledge_all_criteria(path: &Path) -> anyhow::Result<Intent> {
    let n = criteria(&Store::load(path)?.body).len();
    acknowledge_criteria(path, &(0..n).collect::<Vec<_>>())
}
pub const PRECONDITIONS_HEADING: &str = "## Preconditions";

/// Checks that prove the intent is done.
pub fn criteria(body: &str) -> Vec<Criterion> {
    checklist(body, CRITERIA_HEADING)
}

pub const STORIES_HEADING: &str = "## Stories";
/// The line a story carries until every open question has its answer.
pub const STORIES_WAIT: &str = "drafts wait for the open questions";

/// One story under `## Stories`: who acts, what must exist first, what they
/// do, and which acceptance criteria its demo video shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentStory {
    /// From 1, in the order the section lists them.
    pub number: usize,
    pub name: String,
    pub persona: Option<String>,
    /// Criteria numbers, counted from 1 across area groups.
    pub shows: Vec<usize>,
    pub starting_data: Option<String>,
    pub steps: Vec<String>,
    /// Drafted only once every open question has its answer.
    pub waits: bool,
}

/// `1, 3` → `[1, 3]`; anything that is not a number is skipped.
fn numbers(list: &str) -> Vec<usize> {
    list.split(',')
        .filter_map(|n| n.trim().parse::<usize>().ok())
        .collect()
}

/// The stories under `## Stories`, each `### <name>` with its `- persona:`,
/// `- shows:` and `- starting data:` lines and numbered steps.
pub fn stories(body: &str) -> Vec<IntentStory> {
    let mut out: Vec<IntentStory> = Vec::new();
    for (_, line) in section_lines(body, STORIES_HEADING) {
        let t = line.trim();
        if let Some(name) = t.strip_prefix("### ") {
            out.push(IntentStory {
                number: out.len() + 1,
                name: name.trim().to_string(),
                persona: None,
                shows: Vec::new(),
                starting_data: None,
                steps: Vec::new(),
                waits: false,
            });
            continue;
        }
        let Some(s) = out.last_mut() else { continue };
        if let Some(v) = t.strip_prefix("- persona:") {
            s.persona = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        } else if let Some(v) = t.strip_prefix("- shows:") {
            s.shows = numbers(v);
        } else if let Some(v) = t.strip_prefix("- starting data:") {
            s.starting_data = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        } else if t.eq_ignore_ascii_case(STORIES_WAIT) {
            s.waits = true;
        } else if let Some((n, step)) = t.split_once(". ")
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            s.steps.push(step.trim().to_string());
        }
    }
    out
}

/// The story's folder name, `<n>-<slug>`: its number keeps two stories of
/// one name apart; the slug is its name's words, lower case, up to 48 letters.
pub fn story_folder(s: &IntentStory) -> String {
    let mut slug = String::new();
    for word in s
        .name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let word = word.to_lowercase();
        if !slug.is_empty() && slug.len() + 1 + word.len() > 48 {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    format!("{}-{slug}", s.number)
}

/// The criteria a story shows that exist, with their text, and the numbers
/// it names that match no criterion.
pub fn shown_criteria(body: &str, s: &IntentStory) -> (Vec<(usize, String)>, Vec<usize>) {
    let all = criteria(body);
    let mut shown = Vec::new();
    let mut dropped = Vec::new();
    for &n in &s.shows {
        match n.checked_sub(1).and_then(|i| all.get(i)) {
            Some(c) => shown.push((n, c.text.clone())),
            None => dropped.push(n),
        }
    }
    (shown, dropped)
}

/// `body` with its Stories section replaced by the one in `proposal` (or the
/// proposal itself, given without its heading), added after the acceptance
/// criteria when there was none. Every other section stays as it is, and any
/// other section the proposal holds is ignored.
pub fn replace_stories(body: &str, proposal: &str) -> String {
    let heading = |l: &str| l.starts_with("## ");
    let is_stories = |l: &str| l.trim().eq_ignore_ascii_case(STORIES_HEADING);
    // the new section's lines
    let plines: Vec<&str> = proposal.lines().collect();
    let from = plines
        .iter()
        .position(|l| is_stories(l))
        .map_or(0, |i| i + 1);
    let to = plines[from..]
        .iter()
        .position(|l| heading(l))
        .map_or(plines.len(), |i| from + i);
    let inner = plines[from..to].join("\n");
    let section = format!("{STORIES_HEADING}\n{}", inner.trim());
    // the body without its old section
    let mut lines: Vec<&str> = body.lines().collect();
    if let Some(start) = lines.iter().position(|l| is_stories(l)) {
        let end = lines[start + 1..]
            .iter()
            .position(|l| heading(l))
            .map_or(lines.len(), |i| start + 1 + i);
        lines.drain(start..end);
    }
    // where it goes: after the criteria, else before the open questions, else last
    let after_criteria = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(CRITERIA_HEADING))
        .map(|c| {
            lines[c + 1..]
                .iter()
                .position(|l| heading(l))
                .map_or(lines.len(), |i| c + 1 + i)
        });
    let at = after_criteria
        .or_else(|| {
            lines
                .iter()
                .position(|l| l.trim().eq_ignore_ascii_case(QUESTIONS_HEADING))
        })
        .unwrap_or(lines.len());
    let before = lines[..at].join("\n");
    let after = lines[at..].join("\n");
    let mut out = before.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&section);
    out.push('\n');
    if !after.trim().is_empty() {
        out.push('\n');
        out.push_str(after.trim_start());
        out.push('\n');
    }
    out
}

/// The criteria (from 1) no story shows.
pub fn unshown_criteria(body: &str) -> Vec<usize> {
    let shown: Vec<usize> = stories(body).iter().flat_map(|s| s.shows.clone()).collect();
    (1..=criteria(body).len())
        .filter(|n| !shown.contains(n))
        .collect()
}

/// Set each acceptance criterion's checkbox from `ticked(text)`, keeping
/// everything else (indentation, sub-headings, sub-bullets) as it is.
pub fn set_criteria_ticks(body: &str, ticked: impl Fn(&str) -> bool) -> String {
    let lines: Vec<usize> = section_lines(body, CRITERIA_HEADING)
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    let mut out: Vec<String> = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let t = line.trim_start();
        match checkbox_text(t) {
            Some(rest) if lines.contains(&i) && !rest.trim().is_empty() => {
                let indent = &line[..line.len() - t.len()];
                let mark = if ticked(rest.trim()) { "x" } else { " " };
                out.push(format!("{indent}- [{mark}] {rest}"));
            }
            _ => out.push(line.to_string()),
        }
    }
    let mut text = out.join("\n");
    if body.ends_with('\n') {
        text.push('\n');
    }
    text
}

pub fn toggle_criterion(body: &str, index: usize) -> Option<String> {
    toggle_checklist(body, CRITERIA_HEADING, index)
}

/// Assumptions that must hold for the intent to be worth building;
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
