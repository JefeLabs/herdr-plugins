//! Each person's pointers, `<config dir>/spaces.toml`: from a space's name to
//! their clone of its meta repo, `[spaces.<name>] meta = "<path>"`.

use std::path::{Path, PathBuf};

use crate::plain;

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
pub fn pointers_path(config: &Path) -> PathBuf {
    config.join("spaces.toml")
}

/// The pointers; no file reads as none, a file that does not parse is an error.
fn read_pointers(config: &Path) -> Result<Pointers, String> {
    let path = pointers_path(config);
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
pub fn current_name(config: &Path, name: &str) -> String {
    read_pointers(config)
        .ok()
        .and_then(|p| entry(&p, name).map(|(k, _)| k.clone()))
        .unwrap_or_else(|| name.to_string())
}

/// The space whose pointer already points at `meta` (paths compared once
/// resolved): before a move it still has the old name.
pub fn pointed_at(config: &Path, meta: &Path) -> Option<String> {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| plain(p));
    let want = real(meta);
    let pointers = read_pointers(config).ok()?;
    pointers.spaces.into_iter().find_map(|(key, p)| {
        let path = PathBuf::from(shellexpand::tilde(p.meta.as_deref()?).into_owned());
        (real(&path) == want).then_some(key)
    })
}

/// This person's meta repo for the space named `name` (`~` allowed).
pub fn meta_for(config: &Path, name: &str) -> Option<PathBuf> {
    let pointers = read_pointers(config).ok()?;
    let meta = pointer(&pointers, name)?.meta.as_ref()?;
    Some(PathBuf::from(shellexpand::tilde(meta).into_owned()))
}

/// Records `meta` as the meta repo of the space named `name`, keeping the
/// entry's other settings. A file that does not parse is left as it is.
pub fn remember_meta(config: &Path, name: &str, meta: &Path) -> anyhow::Result<()> {
    let mut pointers = read_pointers(config).map_err(|e| anyhow::anyhow!(e))?;
    let key = entry(&pointers, name)
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| name.to_string());
    pointers.spaces.entry(key).or_default().meta = Some(meta.display().to_string());
    std::fs::create_dir_all(config)?;
    std::fs::write(pointers_path(config), toml::to_string(&pointers)?)?;
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
pub fn storage_for(config: &Path, name: &str) -> Result<Storage, String> {
    let pointers = read_pointers(config)?;
    let Some(word) = pointer(&pointers, name).and_then(|p| p.storage.as_deref()) else {
        return Ok(Storage::Profile);
    };
    match word.trim().to_ascii_lowercase().as_str() {
        "profile" => Ok(Storage::Profile),
        "meta" => Ok(Storage::Meta),
        "remote" => Ok(Storage::Remote),
        _ => Err(format!(
            "{}: storage = \"{word}\" for space {name}: use profile, meta or remote",
            pointers_path(config).display()
        )),
    }
}

/// Every pointer: the space's key and its meta repo (`~` expanded).
pub fn pointer_metas(config: &Path) -> Vec<(String, PathBuf)> {
    let Ok(pointers) = read_pointers(config) else {
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
pub fn rename_pointer(config: &Path, from: &str, to: &str) -> anyhow::Result<()> {
    let mut pointers = read_pointers(config).map_err(|e| anyhow::anyhow!(e))?;
    let Some(old) = pointers.spaces.remove(from) else {
        anyhow::bail!("{}: no space {from}", pointers_path(config).display());
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
    std::fs::write(pointers_path(config), toml::to_string(&pointers)?)?;
    Ok(())
}

/// The other names the space of meta repo `meta`, now called `name`, goes
/// by: the key of a pointer to it not yet moved to `name`, and the old
/// names it keeps.
pub fn other_names(config: &Path, meta: &Path, name: &str) -> Vec<String> {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| plain(p));
    let want = real(meta);
    let Ok(pointers) = read_pointers(config) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_then_meta_for_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path();
        remember_meta(cfg, "shop", Path::new("/x/shop-meta")).unwrap();
        assert_eq!(meta_for(cfg, "SHOP"), Some(PathBuf::from("/x/shop-meta")));
        assert_eq!(pointers_path(cfg), cfg.join("spaces.toml"));
    }
}
