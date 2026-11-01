//! A space is named after its meta repo. A space whose pointer still has
//! another name (skoolscout, whose meta repo is skoolscout-com) moves to it
//! once, at start-up:
//!
//! 1. its folder `<root>/<old>` becomes `<root>/<new>`;
//! 2. its intents' `areas:` items and `### ` criteria headings that start
//!    with `<old>/` start with `<new>/` (nothing else in them changes);
//! 3. `<root>/<old>` is left as a link to `<new>`, so an agent holding an
//!    old path still writes to the right file (the tree skips the link);
//! 4. the pointer `[spaces.<old>]` becomes `[spaces.<new>]`, with `<old>`
//!    kept as an old name that still finds the space.
//!
//! Each step can run again, and the pointer changes last: a move cut off
//! partway finishes at the next start. It waits while a run is going in the
//! space, and leaves two folders that both hold intents to their owner.

use std::path::{Path, PathBuf};

use crate::config::Dirs;
use crate::space;

/// Moves every space whose pointer's name is not its meta repo's; returns
/// what to tell the person. `busy(space folder, meta repo)` says a check,
/// capture or recording is running in it.
pub fn settle_spaces(dirs: &Dirs, busy: impl Fn(&Path, &Path) -> bool) -> Vec<String> {
    if due(dirs).is_empty() {
        return Vec::new();
    }
    // each workspace's Intents starts on its own: one moves, the rest go on
    let Some(_lock) = Lock::take(&dirs.config) else {
        return Vec::new();
    };
    due(dirs)
        .into_iter()
        .filter_map(|d| settle_one(dirs, &d, &busy))
        .collect()
}

/// A space to move: its pointer's name, the name its meta repo gives it.
struct Due {
    old: String,
    new: String,
    meta: PathBuf,
}

fn due(dirs: &Dirs) -> Vec<Due> {
    space::pointer_metas(dirs)
        .into_iter()
        .filter(|(_, meta)| meta.is_dir())
        .filter_map(|(old, meta)| {
            let new = space::space_name(&meta).ok()?;
            (!new.eq_ignore_ascii_case(&old)).then_some(Due { old, new, meta })
        })
        .collect()
}

fn settle_one(dirs: &Dirs, d: &Due, busy: &impl Fn(&Path, &Path) -> bool) -> Option<String> {
    let Due { old, new, meta } = d;
    let (from, to) = (dirs.root.join(old), dirs.root.join(new));
    if busy(&from, meta) {
        return Some(format!(
            "{old} will become {new} once its check or recording ends"
        ));
    }
    let from_real = is_real_dir(&from);
    if from_real && std::fs::symlink_metadata(&to).is_ok() {
        return Some(format!(
            "{old} and {new} both hold intents ({} and {}): move one by hand",
            from.display(),
            to.display()
        ));
    }
    if let Some(other) = space::meta_for(dirs, new)
        && real(&other) != real(meta)
    {
        return Some(format!(
            "cannot move {old} to {new}: a space called {new} already points at {}",
            other.display()
        ));
    }
    if from_real && let Err(e) = std::fs::rename(&from, &to) {
        return Some(format!("could not move {old} to {new}: {e}"));
    }
    if to.is_dir()
        && let Err(e) = rewrite_tree(&to, old, new)
    {
        return Some(format!(
            "moving {old} to {new}: {e:#}; it carries on at the next start"
        ));
    }
    let mut notice =
        format!("moved space {old} to {new}, its meta repo's name; {old} still finds it");
    if std::fs::symlink_metadata(&from).is_err()
        && to.is_dir()
        && let Err(e) = std::os::unix::fs::symlink(new, &from)
    {
        notice.push_str(&format!(
            "; could not leave a link at {}: {e}",
            from.display()
        ));
    }
    if let Err(e) = space::rename_pointer(dirs, old, new) {
        return Some(format!(
            "moving {old} to {new}: {e:#}; it carries on at the next start"
        ));
    }
    if let Some(line) = old_space_line(meta) {
        notice.push_str(&line);
    }
    Some(notice)
}

/// `; space = "…" in <config.toml> is no longer read: remove it`, when the
/// meta repo still names a space the old way.
fn old_space_line(meta: &Path) -> Option<String> {
    let file = meta.join(space::INTENTS_DIR).join("config.toml");
    let table: toml::Table = std::fs::read_to_string(&file).ok()?.parse().ok()?;
    let name = table.get("space")?.as_str()?;
    Some(format!(
        "; space = \"{name}\" in {} is no longer read: remove it",
        file.display()
    ))
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| space::plain(path))
}

/// Rewrites the area references in every intent under `dir`: the files the
/// tree loads (`.md`, not `.chat.md`, not in `.assets` or dot folders).
fn rewrite_tree(dir: &Path, old: &str, new: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::AutoSi, false);
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type()?;
        let path = entry.path();
        if name.starts_with('.') {
            continue;
        }
        if kind.is_dir() {
            if !name.ends_with(".assets") {
                rewrite_tree(&path, old, new)?;
            }
        } else if kind.is_file() && name.ends_with(".md") && !name.ends_with(".chat.md") {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            if let Some(moved) = rewrite_refs(&text, old, new, &now) {
                std::fs::write(&path, moved)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
        }
    }
    Ok(())
}

/// `text` with its area references to the space `from` naming `to`: the
/// front matter's `areas:` items and the body's `### ` headings that are
/// `from` or start with `from/`. A changed file gets `updated: <now>`, so
/// an Intents still holding the old text meets a conflict, not a silent
/// write-back. `None` when nothing refers to `from`.
pub fn rewrite_refs(text: &str, from: &str, to: &str, now: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let fence = |l: &str| l.trim_end() == "---";
    let front_end = lines
        .first()
        .is_some_and(|l| fence(l))
        .then(|| lines.iter().skip(1).position(|l| fence(l)).map(|i| i + 1))
        .flatten();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut changed = false;
    let mut in_areas = false;
    let mut updated_at = None;
    for (i, line) in lines.iter().enumerate() {
        let in_front = front_end.is_some_and(|end| i > 0 && i < end);
        let in_body = front_end.is_none_or(|end| i > end);
        let rewritten = if in_front {
            if line.starts_with("updated:") {
                updated_at = Some(i);
            }
            if !line.starts_with([' ', '\t', '-']) {
                in_areas = line.trim_end() == "areas:";
                None
            } else if in_areas {
                area_item(line, from, to)
            } else {
                None
            }
        } else if in_body {
            line.strip_prefix("### ")
                .and_then(|v| swap_prefix(v, from, to))
                .map(|v| format!("### {v}"))
        } else {
            None
        };
        changed |= rewritten.is_some();
        out.push(rewritten.unwrap_or_else(|| line.to_string()));
    }
    if !changed {
        return None;
    }
    if let Some(i) = updated_at {
        out[i] = format!("updated: {now}\n");
    }
    Some(out.concat())
}

/// A front-matter list item (`- x`, `  - 'x'`) with its value rewritten.
fn area_item(line: &str, from: &str, to: &str) -> Option<String> {
    let indent = &line[..line.len() - line.trim_start().len()];
    let rest = line[indent.len()..].strip_prefix("- ")?;
    let quote = if rest.starts_with(['\'', '"']) {
        &rest[..1]
    } else {
        ""
    };
    let value = swap_prefix(&rest[quote.len()..], from, to)?;
    Some(format!("{indent}- {quote}{value}"))
}

/// `value` with a leading `from` (any case) replaced by `to`, when `from` is
/// the whole first path segment: `from`, `from/…`, `from'`; not `from-x/…`.
fn swap_prefix(value: &str, from: &str, to: &str) -> Option<String> {
    let head = value.get(..from.len())?;
    let rest = &value[from.len()..];
    let whole = rest.is_empty() || rest.starts_with(['/', '\'', '"', '\n', '\r']);
    (head.eq_ignore_ascii_case(from) && whole).then(|| format!("{to}{rest}"))
}

/// Only one Intents moves spaces at a time: `<config>/spaces.toml.moving`,
/// made new or not at all, removed when done. One left by a crash is taken
/// over after a while.
struct Lock(PathBuf);

impl Lock {
    fn take(config: &Path) -> Option<Lock> {
        let path = config.join("spaces.toml.moving");
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Some(Lock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > std::time::Duration::from_secs(120));
                    if !stale {
                        return None;
                    }
                    let _ = std::fs::remove_file(&path);
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
