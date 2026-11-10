//! Spaces: a project's repos, anchored by a meta repo whose `.helmsmith/`
//! holds the shared `space.toml` (the repos) and `run.md` (how to run the app),
//! plus each person's pointer from a space's name to its meta repo
//! ([`pointers`]). A space is named after its meta repo: the repo name in its
//! `origin`. Shared by the Herdr plugins so they agree on the files.

use std::path::{Path, PathBuf};

pub mod pointers;

/// A space, as its meta repo's `.helmsmith/space.toml` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub meta: PathBuf,
    pub repos: Vec<SpaceRepo>,
    /// `database = true/false`: whether the space's app uses a database.
    /// `None` when the file doesn't say (see [`database_on`]).
    pub database: Option<bool>,
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
    #[serde(default)]
    repos: Vec<RepoEntry>,
    #[serde(default)]
    database: Option<bool>,
}

#[derive(serde::Deserialize)]
struct RepoEntry {
    path: String,
    git: String,
}

/// `path` without `.` and `..` (worked out on the names; the folders need not exist).
pub fn plain(path: &Path) -> PathBuf {
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
        database: parsed.database,
    })
}

/// Whether the space of meta repo `meta` uses a database: its space file's
/// `database`, else whether its run guide has a `db:` line (`guide_has_db`,
/// worked out by the caller, who reads the guide), so a space with no say
/// keeps today's Database tool. No space file also follows the guide. `Err`
/// says why a space file that is there can't be read.
pub fn database_on(meta: &Path, guide_has_db: bool) -> Result<bool, String> {
    Ok(database_setting(meta)?.unwrap_or(guide_has_db))
}

/// What the space file of meta repo `meta` says about a database: `None`
/// when there is no space file or it has no `database`. `Err` says why a
/// space file that is there can't be read.
pub fn database_setting(meta: &Path) -> Result<Option<bool>, String> {
    if !meta.join(".helmsmith").join("space.toml").is_file() {
        return Ok(None);
    }
    let space = read_space(meta).map_err(|e| format!("{e:#}"))?;
    Ok(space.database)
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
        .and_then(|n| group_name(&n))
    {
        return Ok(name);
    }
    meta.file_name()
        .and_then(|n| group_name(&n.to_string_lossy()))
        .ok_or_else(|| format!("{} has no name a space can take", meta.display()))
}

/// The space `repo` names when it is a meta repo (`.helmsmith/intents/` or
/// `.helmsmith/space.toml`); `Err` says why it has no usable name.
pub fn intents_space(repo: &Path) -> Option<Result<String, String>> {
    is_meta(repo).then(|| space_name(repo))
}

/// A workspace name usable as one folder name: trimmed, not empty, no path
/// separator, not `.`-led, not `~`-led (Herdr names a workspace started in
/// the home folder `~`). Anything else is no scope rather than a path.
pub fn group_name(name: &str) -> Option<String> {
    let name = name.trim();
    let one_folder = !name.is_empty()
        && !name.starts_with('.')
        && !name.starts_with('~')
        && !name.contains('/')
        && !name.contains('\\');
    one_folder.then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn git_slug_reads_every_github_url_form() {
        for url in [
            "git@github.com:o/R.git",
            "https://github.com/o/R",
            "ssh://git@github.com/o/R.git",
        ] {
            assert_eq!(git_slug(url).as_deref(), Some("o/r"), "{url}");
        }
    }

    #[test]
    fn repo_name_keeps_case() {
        assert_eq!(
            repo_name("git@github.com:o/Shop-Meta.git").as_deref(),
            Some("Shop-Meta")
        );
    }

    #[test]
    fn space_name_falls_back_to_the_folder_without_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = tmp.path().join("shop-meta");
        std::fs::create_dir(&meta).unwrap();
        git(&meta, &["init", "-q"]);
        assert_eq!(space_name(&meta), Ok("shop-meta".to_string()));
    }

    #[test]
    fn read_space_resolves_paths_from_the_meta_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = tmp.path().join("m");
        std::fs::create_dir_all(meta.join(".helmsmith")).unwrap();
        std::fs::write(
            meta.join(".helmsmith/space.toml"),
            "repos = [{ path = \"../api\", git = \"g\" }]\n",
        )
        .unwrap();
        let space = read_space(&meta).unwrap();
        assert_eq!(space.repos[0].path, tmp.path().join("api"));
        assert_eq!(space.repos[0].rel, "../api");
    }

    /// A meta repo `<tmp>/m` with `.helmsmith/intents/`, and the given space file.
    fn meta_with(tmp: &Path, space_toml: Option<&str>) -> PathBuf {
        let meta = tmp.join("m");
        std::fs::create_dir_all(meta.join(".helmsmith/intents")).unwrap();
        if let Some(text) = space_toml {
            std::fs::write(meta.join(".helmsmith/space.toml"), text).unwrap();
        }
        meta
    }

    #[test]
    fn database_only_file_reads_with_no_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_with(tmp.path(), Some("database = false\n"));
        let space = read_space(&meta).unwrap();
        assert!(space.repos.is_empty());
        assert_eq!(space.database, Some(false));
    }

    #[test]
    fn database_key_wins_over_the_guide() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_with(tmp.path(), Some("database = false\n"));
        assert_eq!(database_on(&meta, true), Ok(false));
    }

    #[test]
    fn no_key_follows_the_guide() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_with(tmp.path(), Some("repos = []\n"));
        assert_eq!(database_on(&meta, true), Ok(true));
        assert_eq!(database_on(&meta, false), Ok(false));
    }

    #[test]
    fn no_space_file_follows_the_guide() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_with(tmp.path(), None);
        assert_eq!(database_on(&meta, true), Ok(true));
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = meta_with(tmp.path(), Some("database = \"no\"\n"));
        let err = database_on(&meta, true).unwrap_err();
        assert!(err.contains("space.toml"), "{err}");
    }
}
