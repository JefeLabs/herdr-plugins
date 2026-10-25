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

pub const INBOX: &str = "Inbox";
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
        std::fs::create_dir_all(root.join(INBOX))
            .with_context(|| format!("creating {}", root.display()))?;
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
fn stamp(intent: &Intent) -> DateTime<Local> {
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

/// Number of `- ` items under `## Open questions`.
pub fn open_questions(body: &str) -> usize {
    let mut in_section = false;
    let mut n = 0;
    for line in body.lines() {
        if line.starts_with("## ") {
            in_section = line.trim().eq_ignore_ascii_case("## open questions");
            continue;
        }
        if in_section && line.trim_start().starts_with("- ") {
            n += 1;
        }
    }
    n
}

/// Convenience for callers that want the raw mapping (e.g. tests).
pub fn front_matter_mapping(text: &str) -> Option<Mapping> {
    let (fm, _) = split_front_matter(text)?;
    serde_yaml_ng::from_str(fm).ok()
}
