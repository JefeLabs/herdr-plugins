//! Spaces: the meta repo's space file, and each person's pointers to it.

use std::fs;
use std::path::Path;

use herdr_intents::config::Dirs;
use herdr_intents::space::{meta_for, missing, read_space, remember_meta};

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
