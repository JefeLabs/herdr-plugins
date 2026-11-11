//! Spaces: a project's repos, anchored by a meta repo whose `.helmsmith/`
//! holds the shared `space.toml` (the repos) and `run.md` (how to run the app),
//! plus each person's pointer from a space's name to its meta repo. A space
//! is named after its meta repo: the repo name in its `origin`.

//!
//! The files themselves are read and written by the `herdr-space` library,
//! which other plugins share; these are its functions with Intents' `Dirs`.

use std::path::{Path, PathBuf};

use crate::config::Dirs;

pub use herdr_space::pointers::Storage;
pub use herdr_space::{
    INTENTS_DIR, Space, SpaceRepo, database_on, database_setting, git_slug, intents_space, is_meta,
    missing, origin_slug, plain, read_space, repo_name, space_name, target_slug,
};

/// Where this person's pointers are kept.
pub fn pointers_path(dirs: &Dirs) -> PathBuf {
    herdr_space::pointers::pointers_path(&dirs.config)
}

/// The space's name now: `name`, unless a space used to be called that.
pub fn current_name(dirs: &Dirs, name: &str) -> String {
    herdr_space::pointers::current_name(&dirs.config, name)
}

/// The space whose pointer already points at `meta`.
pub fn pointed_at(dirs: &Dirs, meta: &Path) -> Option<String> {
    herdr_space::pointers::pointed_at(&dirs.config, meta)
}

/// This person's meta repo for the space named `name`.
pub fn meta_for(dirs: &Dirs, name: &str) -> Option<PathBuf> {
    herdr_space::pointers::meta_for(&dirs.config, name)
}

/// Records `meta` as the meta repo of the space named `name`.
pub fn remember_meta(dirs: &Dirs, name: &str, meta: &Path) -> anyhow::Result<()> {
    herdr_space::pointers::remember_meta(&dirs.config, name, meta)
}

/// The storage the space named `name` uses.
pub fn storage_for(dirs: &Dirs, name: &str) -> Result<Storage, String> {
    herdr_space::pointers::storage_for(&dirs.config, name)
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

/// Every pointer: the space's key and its meta repo.
pub fn pointer_metas(dirs: &Dirs) -> Vec<(String, PathBuf)> {
    herdr_space::pointers::pointer_metas(&dirs.config)
}

/// Renames the pointer `from` to `to`, which keeps `from` as an old name.
pub fn rename_pointer(dirs: &Dirs, from: &str, to: &str) -> anyhow::Result<()> {
    herdr_space::pointers::rename_pointer(&dirs.config, from, to)
}

/// The other names the space of meta repo `meta`, now called `name`, goes by.
pub fn other_names(dirs: &Dirs, meta: &Path, name: &str) -> Vec<String> {
    herdr_space::pointers::other_names(&dirs.config, meta, name)
}

/// The space a workspace in meta repo `repo` sees: the name of the pointer
/// that already points at it, which keeps the old name until the space
/// moves (so nothing shows empty or is captured apart meanwhile); else the
/// name the repo gives it. None when `repo` is not a meta repo.
pub fn repo_space(dirs: &Dirs, repo: &Path) -> Option<Result<String, String>> {
    let found = intents_space(repo)?;
    Some(found.map(|name| pointed_at(dirs, repo).unwrap_or(name)))
}
