//! Spaces: the meta repo's space file, and each person's pointers to it.

use std::fs;
use std::path::{Path, PathBuf};

use herdr_intents::config::Dirs;
use herdr_intents::space::{
    Storage, current_name, intents_space, is_meta, meta_for, missing, other_names, pointed_at,
    pointers_path, read_space, remember_meta, repo_name, repo_space, space_name, storage_block,
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

// ---- the meta repo's intents folder: the space's name ---------------------------------

#[test]
fn an_intents_folder_or_a_space_file_makes_a_meta_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("skoolscout-com");
    fs::create_dir_all(repo.join(".helmsmith/hooks")).unwrap();
    assert!(!is_meta(&repo), "hooks alone are not intents config");
    fs::create_dir_all(repo.join(".helmsmith/intents")).unwrap();
    assert!(is_meta(&repo));
    let other = tmp.path().join("meta");
    fs::create_dir_all(other.join(".helmsmith")).unwrap();
    fs::write(other.join(".helmsmith/space.toml"), "repos = []\n").unwrap();
    assert!(is_meta(&other));
}

/// A git repo at `path` whose `origin` is `url` (none when `url` is empty).
fn git_repo(path: &Path, url: &str) {
    fs::create_dir_all(path).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    if !url.is_empty() {
        git(&["remote", "add", "origin", url]);
    }
}

#[test]
fn the_space_is_named_after_the_meta_repos_origin_not_its_folder() {
    // a teammate cloned skoolscout-com into a folder called skoolscout
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("skoolscout");
    git_repo(&repo, "git@github.com:skoolscout/skoolscout-com.git");
    fs::create_dir_all(repo.join(".helmsmith/intents")).unwrap();
    assert_eq!(space_name(&repo).as_deref(), Ok("skoolscout-com"));
    assert_eq!(intents_space(&repo), Some(Ok("skoolscout-com".to_string())));
}

#[test]
fn an_origin_url_in_any_form_gives_the_repo_name_with_its_case() {
    for (url, want) in [
        (
            "git@github.com:skoolscout/skoolscout-com.git",
            "skoolscout-com",
        ),
        (
            "https://github.com/skoolscout/skoolscout-com",
            "skoolscout-com",
        ),
        (
            "ssh://git@github.com/skoolscout/skoolscout-com.git",
            "skoolscout-com",
        ),
        ("https://github.com/Acme/Shop-Meta.git/", "Shop-Meta"),
        (
            "  git@gitlab.com:group/sub/tourneyseason.git\n",
            "tourneyseason",
        ),
    ] {
        assert_eq!(repo_name(url).as_deref(), Some(want), "{url}");
    }
    assert_eq!(repo_name(""), None);
    assert_eq!(repo_name("https://github.com/"), None);
}

#[test]
fn without_an_origin_the_space_is_the_folder_name() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("tourneyseason");
    git_repo(&repo, "");
    assert_eq!(space_name(&repo).as_deref(), Ok("tourneyseason"));
    let plain = tmp.path().join("not-a-repo");
    fs::create_dir_all(&plain).unwrap();
    assert_eq!(space_name(&plain).as_deref(), Ok("not-a-repo"));
}

#[test]
fn a_remote_name_that_is_not_a_folder_name_falls_back_to_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("acme-meta");
    git_repo(&repo, "git@github.com:acme/.github.git");
    assert_eq!(space_name(&repo).as_deref(), Ok("acme-meta"));
}

#[test]
fn the_old_space_line_in_config_toml_is_no_longer_read() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("skoolscout-com");
    git_repo(&repo, "git@github.com:skoolscout/skoolscout-com.git");
    fs::create_dir_all(repo.join(".helmsmith/intents")).unwrap();
    let config = repo.join(".helmsmith/intents/config.toml");
    fs::write(&config, "space = \"skoolscout\"\n").unwrap();
    assert_eq!(space_name(&repo).as_deref(), Ok("skoolscout-com"));
    fs::write(&config, "space = [\n").unwrap();
    assert_eq!(
        intents_space(&repo),
        Some(Ok("skoolscout-com".to_string())),
        "a config.toml that does not parse no longer matters"
    );
}

#[test]
fn a_space_file_alone_names_the_space_too() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("shop-meta");
    git_repo(&repo, "git@github.com:acme/shop-meta.git");
    fs::create_dir_all(repo.join(".helmsmith")).unwrap();
    fs::write(repo.join(".helmsmith/space.toml"), "repos = []\n").unwrap();
    assert_eq!(intents_space(&repo), Some(Ok("shop-meta".to_string())));
    let plain = tmp.path().join("plain");
    git_repo(&plain, "git@github.com:acme/plain.git");
    assert_eq!(intents_space(&plain), None, "not a meta repo");
}

fn dirs_in(tmp: &Path) -> Dirs {
    Dirs {
        config: tmp.join("cfg"),
        root: tmp.join("root"),
    }
}

fn pointers(dirs: &Dirs, text: &str) {
    fs::create_dir_all(&dirs.config).unwrap();
    fs::write(pointers_path(dirs), text).unwrap();
}

#[test]
fn an_old_name_finds_the_space_it_became() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    pointers(
        &dirs,
        "[spaces.skoolscout-com]\nmeta = \"/code/skoolscout-com\"\nstorage = \"meta\"\nold_names = [\"skoolscout\"]\n",
    );
    assert_eq!(current_name(&dirs, "skoolscout"), "skoolscout-com");
    assert_eq!(current_name(&dirs, "SkoolScout"), "skoolscout-com");
    assert_eq!(current_name(&dirs, "skoolscout-com"), "skoolscout-com");
    assert_eq!(current_name(&dirs, "tourneyseason"), "tourneyseason");
    assert_eq!(
        meta_for(&dirs, "skoolscout").as_deref(),
        Some(Path::new("/code/skoolscout-com"))
    );
    assert_eq!(storage_for(&dirs, "skoolscout"), Ok(Storage::Meta));
}

#[test]
fn recording_a_meta_repo_keeps_the_spaces_old_names() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    pointers(
        &dirs,
        "[spaces.skoolscout-com]\nmeta = \"/code/skoolscout-com\"\nold_names = [\"skoolscout\"]\n",
    );
    remember_meta(
        &dirs,
        "skoolscout-com",
        Path::new("/elsewhere/skoolscout-com"),
    )
    .unwrap();
    assert_eq!(current_name(&dirs, "skoolscout"), "skoolscout-com");
    assert_eq!(
        meta_for(&dirs, "skoolscout").as_deref(),
        Some(Path::new("/elsewhere/skoolscout-com"))
    );
}

#[test]
fn the_pointer_that_already_points_at_a_meta_repo_names_its_space() {
    // before the move, skoolscout-com's own pointer is still keyed skoolscout
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let meta = tmp.path().join("work/skoolscout-com");
    fs::create_dir_all(&meta).unwrap();
    fs::create_dir_all(tmp.path().join("work/other")).unwrap();
    pointers(
        &dirs,
        &format!(
            "[spaces.tourneyseason]\nmeta = \"{t}\"\n\n[spaces.skoolscout]\nmeta = \"{m}\"\n",
            t = tmp.path().join("work/other").display(),
            // written through a `..`, as a person might
            m = tmp.path().join("work/other/../skoolscout-com").display()
        ),
    );
    assert_eq!(pointed_at(&dirs, &meta).as_deref(), Some("skoolscout"));
    let unknown = tmp.path().join("work/new-meta");
    fs::create_dir_all(&unknown).unwrap();
    assert_eq!(pointed_at(&dirs, &unknown), None);
}

#[test]
fn a_meta_repos_space_goes_by_its_old_names_before_and_after_the_move() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let meta = tmp.path().join("work/skoolscout-com");
    let other = tmp.path().join("work/tourneyseason");
    fs::create_dir_all(&meta).unwrap();
    fs::create_dir_all(&other).unwrap();
    // before: the pointer still has the old name
    pointers(
        &dirs,
        &format!(
            "[spaces.skoolscout]\nmeta = \"{}\"\n\n[spaces.tourney]\nmeta = \"{}\"\n",
            meta.display(),
            other.display()
        ),
    );
    assert_eq!(
        other_names(&dirs, &meta, "skoolscout-com"),
        vec!["skoolscout"]
    );
    // after: the old name is kept on the renamed pointer
    pointers(
        &dirs,
        &format!(
            "[spaces.skoolscout-com]\nmeta = \"{}\"\nold_names = [\"skoolscout\", \"skool\"]\n\n[spaces.tourney]\nmeta = \"{}\"\n",
            meta.display(),
            other.display()
        ),
    );
    assert_eq!(
        other_names(&dirs, &meta, "skoolscout-com"),
        vec!["skoolscout", "skool"]
    );
    assert_eq!(other_names(&dirs, &other, "tourneyseason"), vec!["tourney"]);
}

#[test]
fn until_its_move_a_meta_repos_space_keeps_its_pointers_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let meta = tmp.path().join("skoolscout-com");
    git_repo(&meta, "git@github.com:skoolscout/skoolscout-com.git");
    fs::create_dir_all(meta.join(".helmsmith/intents")).unwrap();
    // a teammate with no pointer yet: the name its origin gives
    assert_eq!(
        repo_space(&dirs, &meta),
        Some(Ok("skoolscout-com".to_string()))
    );
    // not moved yet (a run held it back): still the pointer's name
    pointers(
        &dirs,
        &format!("[spaces.skoolscout]\nmeta = \"{}\"\n", meta.display()),
    );
    assert_eq!(repo_space(&dirs, &meta), Some(Ok("skoolscout".to_string())));
    // moved
    pointers(
        &dirs,
        &format!(
            "[spaces.skoolscout-com]\nmeta = \"{}\"\nold_names = [\"skoolscout\"]\n",
            meta.display()
        ),
    );
    assert_eq!(
        repo_space(&dirs, &meta),
        Some(Ok("skoolscout-com".to_string()))
    );
    let plain = tmp.path().join("plain");
    git_repo(&plain, "");
    assert_eq!(repo_space(&dirs, &plain), None, "not a meta repo");
}
