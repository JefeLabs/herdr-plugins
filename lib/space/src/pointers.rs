//! Each person's pointers, `<config dir>/spaces.toml`: from a space's name to
//! their clone of its meta repo, `[spaces.<name>] meta = "<path>"`.

use std::path::{Path, PathBuf};

use crate::plain;

/// Each person's pointers, `<config dir>/spaces.toml`:
/// `[spaces.<name>] meta = "<path>"`, `storage = "profile"`, `url = "…"` (kept in the file, not used yet).
#[derive(Default, serde::Deserialize)]
struct Pointers {
    #[serde(default)]
    spaces: std::collections::BTreeMap<String, Pointer>,
}

#[derive(Default, serde::Deserialize)]
struct Pointer {
    #[serde(default)]
    meta: Option<String>,
    /// `profile` (the default), or the placeholders `meta` and `remote`.
    #[serde(default)]
    storage: Option<String>,
    /// Names the space had before it took its meta repo's name, which still
    /// find it (a Herdr workspace, `INTENTS_SCOPE`, an agent's old path).
    #[serde(default)]
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
/// entry's other settings. A file that does not parse is left as it is. The
/// file is edited in place, so comments and keys this code does not know
/// stay as they were.
pub fn remember_meta(config: &Path, name: &str, meta: &Path) -> anyhow::Result<()> {
    let pointers = read_pointers(config).map_err(|e| anyhow::anyhow!(e))?;
    let key = entry(&pointers, name)
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| name.to_string());
    let mut doc = read_doc(config)?;
    let spaces = spaces_table(config, &mut doc)?;
    let entry = spaces
        .entry(&key)
        .or_insert(toml_edit::table())
        .as_table_like_mut()
        .ok_or_else(|| not_a_table(config, &key))?;
    entry.insert("meta", toml_edit::value(meta.display().to_string()));
    std::fs::create_dir_all(config)?;
    std::fs::write(pointers_path(config), doc.to_string())?;
    Ok(())
}

/// The pointers file as an editable document; none yet reads as empty.
fn read_doc(config: &Path) -> anyhow::Result<toml_edit::DocumentMut> {
    let path = pointers_path(config);
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .parse()
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml_edit::DocumentMut::new()),
        Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
    }
}

/// The document's `[spaces]` table, made when missing. Written inline
/// (`spaces = { … }`), it becomes `[spaces.<name>]` tables, the only
/// layout this code adds to.
fn spaces_table<'a>(
    config: &Path,
    doc: &'a mut toml_edit::DocumentMut,
) -> anyhow::Result<&'a mut toml_edit::Table> {
    let item = doc.entry("spaces").or_insert_with(|| {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        toml_edit::Item::Table(t)
    });
    if let Some(inline) = item.as_inline_table() {
        let mut t = inline.clone().into_table();
        t.set_implicit(true);
        *item = toml_edit::Item::Table(t);
    }
    item.as_table_mut().ok_or_else(|| {
        anyhow::anyhow!("{}: spaces is not a table", pointers_path(config).display())
    })
}

fn not_a_table(config: &Path, key: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{}: spaces.{key} is not a table",
        pointers_path(config).display()
    )
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
/// what it lacks from `from`'s. The file is edited in place: comments and
/// unknown keys stay, and `from`'s entry moves with its comments when there
/// is no `to` yet.
pub fn rename_pointer(config: &Path, from: &str, to: &str) -> anyhow::Result<()> {
    read_pointers(config).map_err(|e| anyhow::anyhow!(e))?;
    let mut doc = read_doc(config)?;
    let spaces = spaces_table(config, &mut doc)?;
    let Some(old) = spaces.remove(from) else {
        anyhow::bail!("{}: no space {from}", pointers_path(config).display());
    };
    let key = spaces
        .iter()
        .map(|(k, _)| k.to_string())
        .find(|k| k.eq_ignore_ascii_case(to))
        .unwrap_or_else(|| to.to_string());
    let old_names = |item: &toml_edit::Item| -> Vec<String> {
        item.get("old_names")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut names = old_names(&old);
    if !spaces.contains_key(&key) {
        spaces.insert(&key, old);
    } else {
        let new = spaces[&key]
            .as_table_like_mut()
            .ok_or_else(|| not_a_table(config, &key))?;
        if let Some(from_settings) = old.as_table_like() {
            for (k, v) in from_settings.iter() {
                if k != "old_names" && !new.contains_key(k) {
                    new.insert(k, v.clone());
                }
            }
        }
    }
    let mut all = old_names(&spaces[&key]);
    names.push(from.to_string());
    for name in names {
        if !all.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            all.push(name);
        }
    }
    let entry = spaces[&key]
        .as_table_like_mut()
        .ok_or_else(|| not_a_table(config, &key))?;
    entry.insert(
        "old_names",
        toml_edit::value(toml_edit::Array::from_iter(all)),
    );
    std::fs::write(pointers_path(config), doc.to_string())?;
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

    const MINE: &str = "# mine\n[spaces.shop]\nmeta = \"~/a\"\nmine = 1\n";

    #[test]
    fn remember_meta_keeps_comments_and_unknown_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path();
        std::fs::write(pointers_path(cfg), MINE).unwrap();
        remember_meta(cfg, "shop", Path::new("/b")).unwrap();
        let text = std::fs::read_to_string(pointers_path(cfg)).unwrap();
        assert!(text.contains("# mine"), "{text}");
        assert!(text.contains("mine = 1"), "{text}");
        assert!(text.contains("meta = \"/b\""), "{text}");
    }

    #[test]
    fn rename_pointer_keeps_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path();
        std::fs::write(pointers_path(cfg), MINE).unwrap();
        rename_pointer(cfg, "shop", "shop-meta").unwrap();
        let text = std::fs::read_to_string(pointers_path(cfg)).unwrap();
        assert!(text.contains("# mine"), "{text}");
        assert!(text.contains("[spaces.shop-meta]"), "{text}");
        let table: toml::Table = text.parse().unwrap();
        let entry = &table["spaces"]["shop-meta"];
        assert_eq!(entry["meta"].as_str(), Some("~/a"), "{text}");
        let old: Vec<&str> = entry["old_names"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(old.contains(&"shop"), "{text}");
    }
}
