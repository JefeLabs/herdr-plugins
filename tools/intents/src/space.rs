//! Spaces: a project's repos, anchored by a meta repo whose `.helmsmith/`
//! holds the shared `space.toml` (the repos) and `run.md` (how to run the app),
//! plus each person's pointer from a space's name to its meta repo.

use std::path::{Path, PathBuf};

use crate::config::Dirs;

/// A space, as its meta repo's `.helmsmith/space.toml` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub meta: PathBuf,
    pub repos: Vec<SpaceRepo>,
}

/// One of a space's repos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRepo {
    /// Where it is: the meta repo's folder joined with `rel`, without `.`/`..`.
    pub path: PathBuf,
    /// The path as the space file writes it, relative to the meta repo.
    pub rel: String,
    /// Where to clone it from.
    pub git: String,
}

impl SpaceRepo {
    /// The command that brings a missing repo in, from the meta repo.
    pub fn clone_hint(&self) -> String {
        format!("git clone {} {}", self.git, self.rel)
    }
}

#[derive(serde::Deserialize)]
struct SpaceFile {
    repos: Vec<RepoEntry>,
}

#[derive(serde::Deserialize)]
struct RepoEntry {
    path: String,
    git: String,
}

/// `path` without `.` and `..` (worked out on the names; the folders need not exist).
pub(crate) fn plain(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `owner/repo` (lower case) from a git URL: `git@github.com:o/r.git`,
/// `https://github.com/o/r(.git)`, `ssh://git@github.com/o/r.git`.
pub fn git_slug(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let mut parts = url.rsplit(['/', ':']);
    let repo = parts.next().filter(|s| !s.is_empty())?;
    let owner = parts.next().filter(|s| !s.is_empty())?;
    Some(format!("{owner}/{repo}").to_lowercase())
}

/// `owner/repo` from a `pr:` target in another repo (`owner/repo#12`, or a
/// pull request's URL); `None` for `#12`, which is the intent's own repo.
pub fn target_slug(target: &str) -> Option<String> {
    let t = target.trim();
    if t.starts_with('#') {
        return None;
    }
    if let Some((before, _)) = t.split_once("/pull/") {
        return git_slug(before);
    }
    let (slug, _) = t.split_once('#')?;
    git_slug(slug)
}

/// `owner/repo` of a clone's `origin`, as `git` says.
pub fn origin_slug(repo: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| git_slug(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

/// Reads `<meta>/.helmsmith/space.toml`.
pub fn read_space(meta: &Path) -> anyhow::Result<Space> {
    use anyhow::Context;
    let file = meta.join(".helmsmith").join("space.toml");
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let parsed: SpaceFile = toml::from_str(&text).with_context(|| format!("{}", file.display()))?;
    let repos = parsed
        .repos
        .into_iter()
        .map(|r| SpaceRepo {
            path: plain(&meta.join(&r.path)),
            rel: r.path,
            git: r.git,
        })
        .collect();
    Ok(Space {
        meta: meta.to_path_buf(),
        repos,
    })
}

/// The listed repos that are not there.
pub fn missing(space: &Space) -> Vec<&SpaceRepo> {
    space.repos.iter().filter(|r| !r.path.is_dir()).collect()
}

/// Each person's pointers, `<config dir>/spaces.toml`:
/// `[spaces.<name>] meta = "<path>"`, `storage = "profile"`, `url = "…"`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Pointers {
    #[serde(default)]
    spaces: std::collections::BTreeMap<String, Pointer>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Pointer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    meta: Option<String>,
    /// `profile` (the default), or the placeholders `meta` and `remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    storage: Option<String>,
    /// A remote storage's address: kept, not used yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

/// Where this person's pointers are kept.
pub fn pointers_path(dirs: &Dirs) -> PathBuf {
    dirs.config.join("spaces.toml")
}

/// The pointers; no file reads as none, a file that does not parse is an error.
fn read_pointers(dirs: &Dirs) -> Result<Pointers, String> {
    let path = pointers_path(dirs);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Pointers::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The entry for the space named `name`, matched in any case.
fn pointer<'a>(pointers: &'a Pointers, name: &str) -> Option<&'a Pointer> {
    pointers
        .spaces
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, p)| p)
}

/// This person's meta repo for the space named `name` (`~` allowed).
pub fn meta_for(dirs: &Dirs, name: &str) -> Option<PathBuf> {
    let pointers = read_pointers(dirs).ok()?;
    let meta = pointer(&pointers, name)?.meta.as_ref()?;
    Some(PathBuf::from(shellexpand::tilde(meta).into_owned()))
}

/// Records `meta` as the meta repo of the space named `name`, keeping the
/// entry's other settings. A file that does not parse is left as it is.
pub fn remember_meta(dirs: &Dirs, name: &str, meta: &Path) -> anyhow::Result<()> {
    let mut pointers = read_pointers(dirs).map_err(|e| anyhow::anyhow!(e))?;
    let key = pointers
        .spaces
        .keys()
        .find(|k| k.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| name.to_string());
    pointers.spaces.entry(key).or_default().meta = Some(meta.display().to_string());
    std::fs::create_dir_all(&dirs.config)?;
    std::fs::write(pointers_path(dirs), toml::to_string(&pointers)?)?;
    Ok(())
}

/// Where a space keeps its intents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// This person's own folder, `~/.helmsmith/intents/spaces/<space>/`.
    Profile,
    /// The meta repo's `.helmsmith/intents/`: not built yet.
    Meta,
    /// A REST API: not built yet.
    Remote,
}

/// The storage the space named `name` uses: `profile` unless its entry says otherwise.
pub fn storage_for(dirs: &Dirs, name: &str) -> Result<Storage, String> {
    let pointers = read_pointers(dirs)?;
    let Some(word) = pointer(&pointers, name).and_then(|p| p.storage.as_deref()) else {
        return Ok(Storage::Profile);
    };
    match word.trim().to_ascii_lowercase().as_str() {
        "profile" => Ok(Storage::Profile),
        "meta" => Ok(Storage::Meta),
        "remote" => Ok(Storage::Remote),
        _ => Err(format!(
            "{}: storage = \"{word}\" for space {name}: use profile, meta or remote",
            pointers_path(dirs).display()
        )),
    }
}

/// Why the space named `name` shows no intents, when its storage is not one
/// the tool can use yet (or its setting is broken).
pub fn storage_block(dirs: &Dirs, name: &str) -> Option<String> {
    let placeholder = |kind: &str| {
        Some(format!(
            "{kind} storage is not built yet (storage = \"{kind}\" for space {name} in {})",
            pointers_path(dirs).display()
        ))
    };
    match storage_for(dirs, name) {
        Ok(Storage::Profile) => None,
        Ok(Storage::Meta) => placeholder("meta"),
        Ok(Storage::Remote) => placeholder("remote"),
        Err(e) => Some(e),
    }
}
