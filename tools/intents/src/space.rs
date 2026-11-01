//! Spaces: a project's repos, anchored by a meta repo whose `.helmsmith/`
//! holds the shared `space.toml` (the repos) and `run.md` (how to run the app),
//! plus each person's pointer from a space's name to its meta repo. A space
//! is named after its meta repo: the repo name in its `origin`.

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

/// The repo's name in a git URL, as the URL writes it: the last path
/// segment without `.git` (`git@github.com:o/Shop-Meta.git` → `Shop-Meta`).
/// Like `git_slug`, something must come before it, so a bare host is none.
pub fn repo_name(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let mut parts = url.rsplit(['/', ':']);
    let repo = parts.next().filter(|s| !s.is_empty())?;
    parts.next().filter(|s| !s.is_empty())?;
    Some(repo.to_string())
}

/// The URL of a clone's `origin`, as `git` says; none without one.
fn origin_url(repo: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|url| !url.is_empty())
}

/// `owner/repo` of a clone's `origin`, as `git` says.
pub fn origin_slug(repo: &Path) -> Option<String> {
    git_slug(&origin_url(repo)?)
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

/// The folder in a meta repo that holds its space's intents configuration.
pub const INTENTS_DIR: &str = ".helmsmith/intents";

/// A repo that anchors a space: its `.helmsmith/` has an `intents/` folder or
/// a `space.toml`.
pub fn is_meta(repo: &Path) -> bool {
    repo.join(INTENTS_DIR).is_dir() || repo.join(".helmsmith").join("space.toml").is_file()
}

/// The space a meta repo names: the repo name in its `origin` URL, so every
/// clone lands in the same space whatever its folder is called; without an
/// `origin`, or when that name is not one folder name, the repo's folder
/// name. `.helmsmith/intents/config.toml` is no longer read.
pub fn space_name(meta: &Path) -> Result<String, String> {
    if let Some(name) = origin_url(meta)
        .and_then(|url| repo_name(&url))
        .and_then(|n| crate::scope::group_name(&n))
    {
        return Ok(name);
    }
    meta.file_name()
        .and_then(|n| crate::scope::group_name(&n.to_string_lossy()))
        .ok_or_else(|| format!("{} has no name a space can take", meta.display()))
}

/// The space `repo` names when it is a meta repo (`.helmsmith/intents/` or
/// `.helmsmith/space.toml`); `Err` says why it has no usable name.
pub fn intents_space(repo: &Path) -> Option<Result<String, String>> {
    is_meta(repo).then(|| space_name(repo))
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
    /// Names the space had before it took its meta repo's name, which still
    /// find it (a Herdr workspace, `INTENTS_SCOPE`, an agent's old path).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    old_names: Vec<String>,
}

impl Pointer {
    /// Whether `name` is this entry's, as its key `key` or an old name, in any case.
    fn answers_to(&self, key: &str, name: &str) -> bool {
        key.eq_ignore_ascii_case(name)
            || self.old_names.iter().any(|o| o.eq_ignore_ascii_case(name))
    }
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

/// The key and entry of the space named `name`, matched in any case: its
/// own key first, then a space that used to be called that.
fn entry<'a>(pointers: &'a Pointers, name: &str) -> Option<(&'a String, &'a Pointer)> {
    let spaces = &pointers.spaces;
    spaces
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .or_else(|| spaces.iter().find(|(k, p)| p.answers_to(k, name)))
}

/// The entry for the space named `name` (or once named that).
fn pointer<'a>(pointers: &'a Pointers, name: &str) -> Option<&'a Pointer> {
    entry(pointers, name).map(|(_, p)| p)
}

/// The space's name now: `name`, unless a space used to be called that.
pub fn current_name(dirs: &Dirs, name: &str) -> String {
    read_pointers(dirs)
        .ok()
        .and_then(|p| entry(&p, name).map(|(k, _)| k.clone()))
        .unwrap_or_else(|| name.to_string())
}

/// The space whose pointer already points at `meta` (paths compared once
/// resolved): before a move it still has the old name.
pub fn pointed_at(dirs: &Dirs, meta: &Path) -> Option<String> {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| plain(p));
    let want = real(meta);
    let pointers = read_pointers(dirs).ok()?;
    pointers.spaces.into_iter().find_map(|(key, p)| {
        let path = PathBuf::from(shellexpand::tilde(p.meta.as_deref()?).into_owned());
        (real(&path) == want).then_some(key)
    })
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
    let key = entry(&pointers, name)
        .map(|(k, _)| k.clone())
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

/// Every pointer: the space's key and its meta repo (`~` expanded).
pub fn pointer_metas(dirs: &Dirs) -> Vec<(String, PathBuf)> {
    let Ok(pointers) = read_pointers(dirs) else {
        return Vec::new();
    };
    pointers
        .spaces
        .into_iter()
        .filter_map(|(key, p)| {
            let meta = PathBuf::from(shellexpand::tilde(p.meta.as_deref()?).into_owned());
            Some((key, meta))
        })
        .collect()
}

/// Renames the pointer `from` to `to`, which keeps `from` as an old name.
/// An entry already called `to` keeps its own settings and takes only
/// what it lacks from `from`'s.
pub fn rename_pointer(dirs: &Dirs, from: &str, to: &str) -> anyhow::Result<()> {
    let mut pointers = read_pointers(dirs).map_err(|e| anyhow::anyhow!(e))?;
    let Some(old) = pointers.spaces.remove(from) else {
        anyhow::bail!("{}: no space {from}", pointers_path(dirs).display());
    };
    let key = pointers
        .spaces
        .keys()
        .find(|k| k.eq_ignore_ascii_case(to))
        .cloned()
        .unwrap_or_else(|| to.to_string());
    let new = pointers.spaces.entry(key).or_default();
    new.meta = new.meta.take().or(old.meta);
    new.storage = new.storage.take().or(old.storage);
    new.url = new.url.take().or(old.url);
    for name in old.old_names.into_iter().chain([from.to_string()]) {
        if !new.old_names.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            new.old_names.push(name);
        }
    }
    std::fs::write(pointers_path(dirs), toml::to_string(&pointers)?)?;
    Ok(())
}

/// The other names the space of meta repo `meta`, now called `name`, goes
/// by: the key of a pointer to it not yet moved to `name`, and the old
/// names it keeps.
pub fn other_names(dirs: &Dirs, meta: &Path, name: &str) -> Vec<String> {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| plain(p));
    let want = real(meta);
    let Ok(pointers) = read_pointers(dirs) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for (key, p) in pointers.spaces {
        let Some(m) = p.meta.as_deref() else { continue };
        if real(Path::new(&*shellexpand::tilde(m))) != want {
            continue;
        }
        for n in std::iter::once(key).chain(p.old_names) {
            if !n.eq_ignore_ascii_case(name) && !out.iter().any(|o| o.eq_ignore_ascii_case(&n)) {
                out.push(n);
            }
        }
    }
    out
}

/// The space a workspace in meta repo `repo` sees: the name of the pointer
/// that already points at it, which keeps the old name until the space
/// moves (so nothing shows empty or is captured apart meanwhile); else the
/// name the repo gives it. None when `repo` is not a meta repo.
pub fn repo_space(dirs: &Dirs, repo: &Path) -> Option<Result<String, String>> {
    let found = intents_space(repo)?;
    Some(found.map(|name| pointed_at(dirs, repo).unwrap_or(name)))
}
