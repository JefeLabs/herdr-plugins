//! Where intents live. By default each space's intents are under
//! `~/.helmsmith/intents/spaces/<space>/`; the first start after the update
//! moves the old `~/intents` there once and leaves a link at the old path,
//! so anything still writing to the old paths (a running check, a coding
//! agent sent before the move) lands in the moved intents.

use std::path::{Path, PathBuf};

/// The default root, as written in `intents.toml`.
pub const DEFAULT_ROOT: &str = "~/.helmsmith/intents/spaces";
/// The root before the move.
pub const OLD_ROOT: &str = "~/intents";

/// The root to use, and what to tell the person once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub root: PathBuf,
    pub notice: Option<String>,
}

/// The default root under `home`.
pub fn default_root(home: &Path) -> PathBuf {
    home.join(".helmsmith/intents/spaces")
}

/// Moves `~/intents` to the new default root once. A custom root is left
/// alone; a root already in place, or an old path that is already a link,
/// needs nothing.
pub fn settle_root(home: &Path, root: &Path) -> Settled {
    let keep = |root: &Path, notice: Option<String>| Settled {
        root: root.to_path_buf(),
        notice,
    };
    if root != default_root(home) {
        return keep(root, None);
    }
    let old = home.join("intents");
    let old_is_folder = std::fs::symlink_metadata(&old)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false);
    if !old_is_folder {
        return keep(root, None);
    }
    if std::fs::symlink_metadata(root).is_ok() {
        return keep(
            root,
            Some(format!(
                "{old} and {new} both hold intents: using {new}; move anything still in {old} by hand",
                old = old.display(),
                new = root.display()
            )),
        );
    }
    let moved = root
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::rename(&old, root));
    if let Err(e) = moved {
        return keep(
            &old,
            Some(format!(
                "could not move {old} to {new}: {e}; still using {old}",
                old = old.display(),
                new = root.display()
            )),
        );
    }
    let mut notice = format!(
        "moved your intents from {old} to {new} ({old} is now a link to it)",
        old = old.display(),
        new = root.display()
    );
    if let Err(e) = std::os::unix::fs::symlink(root, &old) {
        notice.push_str(&format!(
            "; could not leave a link at {}: {e}",
            old.display()
        ));
    }
    keep(root, Some(notice))
}
