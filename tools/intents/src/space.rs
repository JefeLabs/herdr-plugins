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

/// Each person's pointers, `<config dir>/spaces.toml`: `[spaces.<name>] meta = "<path>"`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Pointers {
    #[serde(default)]
    spaces: std::collections::BTreeMap<String, Pointer>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Pointer {
    meta: String,
}

/// Where this person's pointers are kept.
pub fn pointers_path(dirs: &Dirs) -> PathBuf {
    dirs.config.join("spaces.toml")
}

fn read_pointers(dirs: &Dirs) -> Pointers {
    std::fs::read_to_string(pointers_path(dirs))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

/// This person's meta repo for the space named `name` (`~` allowed).
pub fn meta_for(dirs: &Dirs, name: &str) -> Option<PathBuf> {
    read_pointers(dirs)
        .spaces
        .get(name)
        .map(|p| PathBuf::from(shellexpand::tilde(&p.meta).into_owned()))
}

/// Records `meta` as the meta repo of the space named `name`.
pub fn remember_meta(dirs: &Dirs, name: &str, meta: &Path) -> anyhow::Result<()> {
    let mut pointers = read_pointers(dirs);
    pointers.spaces.insert(
        name.to_string(),
        Pointer {
            meta: meta.display().to_string(),
        },
    );
    std::fs::create_dir_all(&dirs.config)?;
    std::fs::write(pointers_path(dirs), toml::to_string(&pointers)?)?;
    Ok(())
}
