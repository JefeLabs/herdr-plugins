//! Spaces: the meta repo's space file, and each person's pointers to it.

use std::fs;
use std::path::{Path, PathBuf};

use herdr_intents::config::Dirs;
use herdr_intents::space::{
    Storage, meta_for, missing, pointers_path, read_space, remember_meta, storage_block,
    storage_for,
};

#[test]
fn a_space_lists_its_repos_beside_the_meta_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    fs::create_dir_all(meta.join(".helmsmith")).unwrap();
    fs::create_dir_all(tmp.path().join("api")).unwrap();
    fs::write(
        meta.join(".helmsmith/space.toml"),
        "repos = [\n  { path = \".\", git = \"git@github.com:acme/meta.git\" },\n  { path = \"../api\", git = \"git@github.com:acme/api.git\" },\n  { path = \"../web\", git = \"git@github.com:acme/web.git\" },\n]\n",
    )
    .unwrap();
    let space = read_space(&meta).unwrap();
    let paths: Vec<_> = space.repos.iter().map(|r| r.path.clone()).collect();
    assert_eq!(
        paths,
        vec![meta.clone(), tmp.path().join("api"), tmp.path().join("web")]
    );
    let gone = missing(&space);
    assert_eq!(gone.len(), 1, "{gone:?}");
    assert_eq!(
        gone[0].clone_hint(),
        "git clone git@github.com:acme/web.git ../web"
    );
}

#[test]
fn a_repo_without_a_space_file_is_no_space() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(read_space(tmp.path()).is_err());
}

#[test]
fn pointers_are_remembered_per_space() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = Dirs {
        config: tmp.path().join("cfg"),
        root: tmp.path().join("root"),
    };
    assert_eq!(meta_for(&dirs, "tourneyseason"), None);
    remember_meta(
        &dirs,
        "tourneyseason",
        Path::new("/code/tourneyseason-meta"),
    )
    .unwrap();
    remember_meta(&dirs, "skoolscout", Path::new("/code/skoolscout-com")).unwrap();
    assert_eq!(
        meta_for(&dirs, "tourneyseason").as_deref(),
        Some(Path::new("/code/tourneyseason-meta"))
    );
    assert_eq!(
        meta_for(&dirs, "skoolscout").as_deref(),
        Some(Path::new("/code/skoolscout-com"))
    );
    let text = fs::read_to_string(tmp.path().join("cfg/spaces.toml")).unwrap();
    assert!(text.contains("[spaces.tourneyseason]"), "{text}");
}

/// A config folder holding `spaces.toml` with `text` (none when `None`).
fn spaces(text: Option<&str>) -> (tempfile::TempDir, Dirs) {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = Dirs {
        config: tmp.path().join("cfg"),
        root: tmp.path().join("root"),
    };
    fs::create_dir_all(&dirs.config).unwrap();
    if let Some(t) = text {
        fs::write(pointers_path(&dirs), t).unwrap();
    }
    (tmp, dirs)
}

#[test]
fn storage_defaults_to_profile() {
    let (_t, dirs) = spaces(None);
    assert_eq!(storage_for(&dirs, "skoolscout"), Ok(Storage::Profile));
    let (_t, dirs) = spaces(Some("[spaces.skoolscout]\nmeta = \"/w/meta\"\n"));
    assert_eq!(storage_for(&dirs, "skoolscout"), Ok(Storage::Profile));
    assert_eq!(storage_block(&dirs, "skoolscout"), None);
}

#[test]
fn meta_and_remote_are_placeholders() {
    let (_t, dirs) = spaces(Some(
        "[spaces.a]\nstorage = \"meta\"\n\n[spaces.b]\nstorage = \"remote\"\nurl = \"https://intents.example.com\"\n",
    ));
    assert_eq!(storage_for(&dirs, "a"), Ok(Storage::Meta));
    assert_eq!(storage_for(&dirs, "b"), Ok(Storage::Remote));
    assert!(
        storage_block(&dirs, "a")
            .unwrap()
            .contains("meta storage is not built yet")
    );
    assert!(
        storage_block(&dirs, "b")
            .unwrap()
            .contains("remote storage is not built yet")
    );
    assert_eq!(storage_block(&dirs, "c"), None);
}

#[test]
fn an_unknown_storage_word_names_the_file() {
    let (_t, dirs) = spaces(Some("[spaces.a]\nstorage = \"cloud\"\n"));
    let e = storage_for(&dirs, "a").unwrap_err();
    assert!(
        e.contains("spaces.toml") && e.contains("cloud") && e.contains("profile, meta or remote"),
        "{e}"
    );
}

#[test]
fn recording_the_meta_repo_keeps_the_storage_line() {
    let (_t, dirs) = spaces(Some(
        "[spaces.a]\nstorage = \"remote\"\nurl = \"https://intents.example.com\"\n",
    ));
    remember_meta(&dirs, "a", Path::new("/w/a-meta")).unwrap();
    assert_eq!(meta_for(&dirs, "a"), Some(PathBuf::from("/w/a-meta")));
    assert_eq!(storage_for(&dirs, "a"), Ok(Storage::Remote));
    assert!(
        fs::read_to_string(pointers_path(&dirs))
            .unwrap()
            .contains("https://intents.example.com")
    );
}

#[test]
fn a_spaces_file_that_does_not_parse_blocks_and_is_kept() {
    let broken = "[spaces.a\nstorage = ";
    let (_t, dirs) = spaces(Some(broken));
    assert!(storage_block(&dirs, "a").unwrap().contains("spaces.toml"));
    assert!(remember_meta(&dirs, "a", Path::new("/w/x")).is_err());
    assert_eq!(fs::read_to_string(pointers_path(&dirs)).unwrap(), broken);
}

#[test]
fn storage_matches_the_space_name_in_any_case() {
    let (_t, dirs) = spaces(Some("[spaces.SkoolScout]\nstorage = \"meta\"\n"));
    assert_eq!(storage_for(&dirs, "skoolscout"), Ok(Storage::Meta));
}
