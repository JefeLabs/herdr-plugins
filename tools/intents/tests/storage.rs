//! Where intents live: the new default root and the one-time move from ~/intents.

use std::fs;
use std::path::{Path, PathBuf};

use herdr_intents::storage::{Settled, settle_root};

struct Home {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    new: PathBuf,
    old: PathBuf,
}

fn home() -> Home {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    Home {
        new: home.join(".helmsmith/intents/spaces"),
        old: home.join("intents"),
        home,
        _tmp: tmp,
    }
}

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn old_intents_move_to_the_new_root_once() {
    let h = home();
    put(&h.old.join("skoolscout/a.md"), "# a\n");
    put(&h.old.join(".polish/x/f"), "f");
    put(&h.old.join(".trash/t/from.txt"), "skoolscout\n");
    put(
        &h.old.join("skoolscout/a.assets/evidence/before.json"),
        "{}",
    );

    let s = settle_root(&h.home, &h.new);
    assert_eq!(s.root, h.new);
    let n = s.notice.unwrap();
    assert!(
        n.contains(&h.old.display().to_string()) && n.contains(&h.new.display().to_string()),
        "{n}"
    );
    assert!(h.new.join("skoolscout/a.md").is_file());
    assert!(h.new.join(".polish/x/f").is_file());
    assert!(h.new.join(".trash/t/from.txt").is_file());
    assert!(
        h.new
            .join("skoolscout/a.assets/evidence/before.json")
            .is_file()
    );
    assert!(
        fs::symlink_metadata(&h.old)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the old path is a link now"
    );
    assert_eq!(settle_root(&h.home, &h.new).notice, None, "says it once");
}

#[test]
fn nothing_to_move_says_nothing() {
    let h = home();
    assert_eq!(
        settle_root(&h.home, &h.new),
        Settled {
            root: h.new.clone(),
            notice: None
        }
    );
}

#[test]
fn a_custom_root_is_never_moved() {
    let h = home();
    put(&h.old.join("skoolscout/a.md"), "# a\n");
    let s = settle_root(&h.home, &h.home.join("notes"));
    assert_eq!(
        s,
        Settled {
            root: h.home.join("notes"),
            notice: None
        }
    );
    assert!(h.old.join("skoolscout/a.md").is_file());
}

#[test]
fn both_folders_name_both_and_use_the_new() {
    let h = home();
    put(&h.old.join("skoolscout/a.md"), "# a\n");
    put(&h.new.join("skoolscout/b.md"), "# b\n");
    let s = settle_root(&h.home, &h.new);
    assert_eq!(s.root, h.new);
    let n = s.notice.unwrap();
    assert!(
        n.contains(&h.old.display().to_string()) && n.contains(&h.new.display().to_string()),
        "{n}"
    );
    assert!(h.old.join("skoolscout/a.md").is_file(), "nothing moved");
}

#[test]
fn the_old_path_still_reaches_the_intents() {
    let h = home();
    put(&h.old.join("skoolscout/a.md"), "# a\n");
    put(&h.old.join("skoolscout/a.assets/checks/latest.json"), "[]");
    settle_root(&h.home, &h.new);
    assert!(
        fs::symlink_metadata(&h.old)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::write(h.old.join("skoolscout/a.assets/checks/latest.json"), "{}").unwrap();
    assert_eq!(
        fs::read_to_string(h.new.join("skoolscout/a.assets/checks/latest.json")).unwrap(),
        "{}"
    );
}

#[test]
fn a_link_at_the_old_path_counts_as_moved() {
    let h = home();
    put(&h.new.join("skoolscout/a.md"), "# a\n");
    std::os::unix::fs::symlink(&h.new, &h.old).unwrap();
    assert_eq!(
        settle_root(&h.home, &h.new),
        Settled {
            root: h.new.clone(),
            notice: None
        }
    );
}

#[test]
fn a_failed_move_keeps_the_old_folder() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { nix::libc::geteuid() } == 0 {
        return; // root writes through any mode
    }
    let h = home();
    put(&h.old.join("skoolscout/a.md"), "# a\n");
    let helmsmith = h.home.join(".helmsmith");
    fs::create_dir_all(&helmsmith).unwrap();
    fs::set_permissions(&helmsmith, fs::Permissions::from_mode(0o555)).unwrap();
    let s = settle_root(&h.home, &h.new);
    fs::set_permissions(&helmsmith, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(s.root, h.old);
    assert!(s.notice.unwrap().contains("could not move"));
    assert!(h.old.join("skoolscout/a.md").is_file());
}
