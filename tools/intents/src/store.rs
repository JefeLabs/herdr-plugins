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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Raw,
    Refining,
    Ready,
    Sent,
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
    pub title: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub state: State,
    #[serde(default = "now")]
    pub created: DateTime<Local>,
    #[serde(default = "now")]
    pub updated: DateTime<Local>,
    #[serde(default)]
    pub repo: Option<PathBuf>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub tags: Vec<String>,
    #[serde(default)]
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

/// `tags: a` and `tags: [a, b]` both load.
fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
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
                    Err(_) => continue, // not an intent file; leave it alone
                }
            }
        }
        groups.sort_by_key(|g| g.name.to_lowercase());
        intents.sort_by_key(|i| i.fm.title.to_lowercase());
        Ok(Group {
            path: dir.to_path_buf(),
            name,
            groups,
            intents,
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
            updated: ts,
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
        it.loaded_updated = it.fm.updated;
        Ok(it)
    }

    pub fn load(path: &Path) -> anyhow::Result<Intent> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let (fm_text, body) = split_front_matter(&text)
            .with_context(|| format!("{}: no front matter", path.display()))?;
        let fm: FrontMatter = serde_yaml_ng::from_str(fm_text)
            .with_context(|| format!("{}: front matter", path.display()))?;
        Ok(Intent {
            path: path.to_path_buf(),
            loaded_updated: fm.updated,
            fm,
            body: body.to_string(),
        })
    }

    /// Save, refusing if the file changed on disk since it was loaded.
    pub fn save(intent: &mut Intent) -> Result<(), SaveError> {
        if let Ok(on_disk) = Store::load(&intent.path)
            && on_disk.fm.updated.timestamp() != intent.loaded_updated.timestamp()
        {
            return Err(SaveError::Conflict);
        }
        Store::save_force(intent).map_err(SaveError::Io)
    }

    pub fn save_force(intent: &mut Intent) -> anyhow::Result<()> {
        intent.fm.updated = Local::now();
        write_intent(intent)?;
        intent.loaded_updated = intent.fm.updated;
        Ok(())
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
