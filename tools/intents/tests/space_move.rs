//! A space whose pointer is keyed by another name than its meta repo's moves
//! to that name once: its folder, its intents' area references, its pointer.

use std::fs;
use std::path::{Path, PathBuf};

use herdr_intents::config::Dirs;
use herdr_intents::space::{current_name, meta_for, pointers_path};
use herdr_intents::space_move::settle_spaces;

const INTENT: &str = "---
title: Office hours
kind: idea
state: sent
created: 2026-10-25T10:00:00-04:00
updated: 2026-10-25T10:00:00-04:00
repo: /Users/e/Workspaces/skoolscout/skoolscout-com
areas:
- skoolscout/discover-schools/org/candidates
- 'skoolscout/discover-schools/family/schedule-meeting'
- skoolscout-legacy/elsewhere
---
Fixes skoolscout/jefelabs-com#1856.

## Acceptance criteria

### skoolscout/discover-schools/org/office-hours

- [ ] a
- skoolscout/not-an-area-list

### skoolscout-legacy/kept

- [ ] b
";

const MOVED: &str = "---
title: Office hours
kind: idea
state: sent
created: 2026-10-25T10:00:00-04:00
updated: NOW
repo: /Users/e/Workspaces/skoolscout/skoolscout-com
areas:
- skoolscout-com/discover-schools/org/candidates
- 'skoolscout-com/discover-schools/family/schedule-meeting'
- skoolscout-legacy/elsewhere
---
Fixes skoolscout/jefelabs-com#1856.

## Acceptance criteria

### skoolscout-com/discover-schools/org/office-hours

- [ ] a
- skoolscout/not-an-area-list

### skoolscout-legacy/kept

- [ ] b
";

const UNTOUCHED: &str = "---
title: Nothing to rewrite
kind: idea
state: draft
created: 2026-10-25T10:00:00-04:00
updated: 2026-10-25T10:00:00-04:00
---
Body.
";

struct World {
    _tmp: tempfile::TempDir,
    dirs: Dirs,
    meta: PathBuf,
}

impl World {
    fn root(&self) -> &Path {
        &self.dirs.root
    }
    fn pointers(&self, text: &str) {
        fs::create_dir_all(&self.dirs.config).unwrap();
        fs::write(pointers_path(&self.dirs), text).unwrap();
    }
    fn pointers_text(&self) -> String {
        fs::read_to_string(pointers_path(&self.dirs)).unwrap()
    }
    fn settle(&self) -> Vec<String> {
        settle_spaces(&self.dirs, |_, _| false)
    }
}

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn git(repo: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// skoolscout's state before the move: its meta repo is skoolscout-com, its
/// pointer and intents still say skoolscout.
fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let meta = base.join("work/skoolscout-com");
    fs::create_dir_all(meta.join(".helmsmith/intents")).unwrap();
    git(&meta, &["init", "-q"]);
    git(
        &meta,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:skoolscout/skoolscout-com.git",
        ],
    );
    let w = World {
        dirs: Dirs {
            config: base.join("cfg"),
            root: base.join("spaces"),
        },
        meta,
        _tmp: tmp,
    };
    w.pointers(&format!(
        "[spaces.skoolscout]\nmeta = \"{}\"\n",
        w.meta.display()
    ));
    let group = w
        .root()
        .join("skoolscout/discover-schools/org/office-hours");
    put(&group.join("2026-10-25-a.md"), INTENT);
    put(
        &group.join("2026-10-25-a.chat.md"),
        "### skoolscout/x\n- skoolscout/y\n",
    );
    put(&group.join("2026-10-25-a.before-reconcile.bak"), INTENT);
    put(&group.join("2026-10-25-b.md"), UNTOUCHED);
    w
}

fn moved_group(w: &World) -> PathBuf {
    w.root()
        .join("skoolscout-com/discover-schools/org/office-hours")
}

/// `text` with its `updated:` line's value replaced by `NOW`.
fn without_updated(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.starts_with("updated: ") {
                "updated: NOW"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn a_space_moves_to_its_meta_repos_name_and_says_so_once() {
    let w = world();
    let notices = w.settle();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("moved space skoolscout to skoolscout-com"),
        "{notices:?}"
    );
    assert!(moved_group(&w).join("2026-10-25-a.md").is_file());
    let link = w.root().join("skoolscout");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("skoolscout-com"));
    assert_eq!(current_name(&w.dirs, "skoolscout"), "skoolscout-com");
    assert_eq!(meta_for(&w.dirs, "skoolscout-com"), Some(w.meta.clone()));
    let pointers = w.pointers_text();
    assert!(!pointers.contains("[spaces.skoolscout]"), "{pointers}");

    let before = fs::read_to_string(moved_group(&w).join("2026-10-25-a.md")).unwrap();
    assert_eq!(w.settle(), Vec::<String>::new(), "only once");
    let after = fs::read_to_string(moved_group(&w).join("2026-10-25-a.md")).unwrap();
    assert_eq!(before, after);
}

#[test]
fn the_move_rewrites_only_the_areas_and_criteria_headings_of_its_intents() {
    let w = world();
    w.settle();
    let g = moved_group(&w);
    let a = fs::read_to_string(g.join("2026-10-25-a.md")).unwrap();
    assert_eq!(without_updated(&a), MOVED);
    let updated = a.lines().find(|l| l.starts_with("updated: ")).unwrap();
    assert_ne!(
        updated, "updated: 2026-10-25T10:00:00-04:00",
        "a rewritten intent is stamped, so a stale copy cannot save over it"
    );
    assert_eq!(
        fs::read_to_string(g.join("2026-10-25-b.md")).unwrap(),
        UNTOUCHED
    );
    assert_eq!(
        fs::read_to_string(g.join("2026-10-25-a.chat.md")).unwrap(),
        "### skoolscout/x\n- skoolscout/y\n"
    );
    assert_eq!(
        fs::read_to_string(g.join("2026-10-25-a.before-reconcile.bak")).unwrap(),
        INTENT
    );
}

#[test]
fn a_run_in_the_space_holds_the_move_back_until_a_later_start() {
    let w = world();
    let asked = std::cell::RefCell::new(Vec::new());
    let notices = settle_spaces(&w.dirs, |space, meta| {
        asked
            .borrow_mut()
            .push((space.to_path_buf(), meta.to_path_buf()));
        true
    });
    assert_eq!(
        asked.into_inner(),
        vec![(w.root().join("skoolscout"), w.meta.clone())]
    );
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0]
            .contains("skoolscout will become skoolscout-com once its check or recording ends"),
        "{notices:?}"
    );
    assert!(w.root().join("skoolscout/discover-schools").is_dir());
    assert!(!w.root().join("skoolscout-com").exists());
    assert_eq!(current_name(&w.dirs, "skoolscout"), "skoolscout");
    // the run ended: the next start moves it
    assert_eq!(w.settle().len(), 1);
    assert!(moved_group(&w).is_dir());
}

#[test]
fn two_folders_that_both_hold_intents_are_left_for_the_owner() {
    let w = world();
    put(&w.root().join("skoolscout-com/stray.md"), UNTOUCHED);
    let notices = w.settle();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("skoolscout and skoolscout-com both hold intents"),
        "{notices:?}"
    );
    assert!(w.root().join("skoolscout/discover-schools").is_dir());
    assert!(
        !fs::symlink_metadata(w.root().join("skoolscout"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(current_name(&w.dirs, "skoolscout"), "skoolscout");
}

#[test]
fn a_move_cut_off_after_the_folder_rename_finishes_at_the_next_start() {
    let w = world();
    fs::rename(w.root().join("skoolscout"), w.root().join("skoolscout-com")).unwrap();
    let notices = w.settle();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("moved space skoolscout"), "{notices:?}");
    let a = fs::read_to_string(moved_group(&w).join("2026-10-25-a.md")).unwrap();
    assert_eq!(without_updated(&a), MOVED);
    assert!(
        fs::symlink_metadata(w.root().join("skoolscout"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(current_name(&w.dirs, "skoolscout"), "skoolscout-com");
}

#[test]
fn a_pointer_already_under_the_new_name_for_the_same_repo_is_merged() {
    // an old build, reading a config.toml without the space line, added one
    let w = world();
    w.pointers(&format!(
        "[spaces.skoolscout]\nmeta = \"{m}\"\nurl = \"https://intents.example\"\n\n[spaces.skoolscout-com]\nmeta = \"{m}\"\nstorage = \"profile\"\n",
        m = w.meta.display()
    ));
    assert_eq!(w.settle().len(), 1);
    let pointers = w.pointers_text();
    assert!(!pointers.contains("[spaces.skoolscout]"), "{pointers}");
    assert!(
        pointers.contains("url = \"https://intents.example\""),
        "{pointers}"
    );
    assert!(pointers.contains("storage = \"profile\""), "{pointers}");
    assert_eq!(current_name(&w.dirs, "skoolscout"), "skoolscout-com");
}

#[test]
fn a_pointer_under_the_new_name_for_another_repo_is_refused() {
    let w = world();
    // another owner's repo with the same name
    let other = w.meta.parent().unwrap().join("theirs/skoolscout-com");
    fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    git(
        &other,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:someone-else/skoolscout-com.git",
        ],
    );
    w.pointers(&format!(
        "[spaces.skoolscout]\nmeta = \"{}\"\n\n[spaces.skoolscout-com]\nmeta = \"{}\"\n",
        w.meta.display(),
        other.display()
    ));
    let notices = w.settle();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains(&other.display().to_string()),
        "{notices:?}"
    );
    assert!(w.root().join("skoolscout/discover-schools").is_dir());
    assert_eq!(meta_for(&w.dirs, "skoolscout"), Some(w.meta.clone()));
}

#[test]
fn a_space_with_no_intents_yet_just_takes_the_new_name() {
    let w = world();
    fs::remove_dir_all(w.root()).unwrap();
    assert_eq!(w.settle().len(), 1);
    assert!(
        fs::symlink_metadata(w.root().join("skoolscout")).is_err(),
        "no link to nothing"
    );
    assert_eq!(current_name(&w.dirs, "skoolscout"), "skoolscout-com");
}

#[test]
fn a_space_already_named_after_its_meta_repo_is_left_alone() {
    let w = world();
    for key in ["skoolscout-com", "SkoolScout-Com"] {
        let text = format!("[spaces.{key}]\nmeta = \"{}\"\n", w.meta.display());
        w.pointers(&text);
        assert_eq!(w.settle(), Vec::<String>::new(), "{key}");
        assert_eq!(w.pointers_text(), text);
    }
    assert!(w.root().join("skoolscout/discover-schools").is_dir());
}

#[test]
fn a_pointer_to_a_meta_repo_that_is_gone_is_left_alone() {
    let w = world();
    fs::remove_dir_all(&w.meta).unwrap();
    assert_eq!(w.settle(), Vec::<String>::new());
    assert!(w.root().join("skoolscout/discover-schools").is_dir());
}

#[test]
fn the_notice_says_when_config_toml_still_names_the_old_space() {
    let w = world();
    let config = w.meta.join(".helmsmith/intents/config.toml");
    fs::write(&config, "space = \"skoolscout\"\n").unwrap();
    let notices = w.settle();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains(&format!(
            "space = \"skoolscout\" in {} is no longer read: remove it",
            config.display()
        )),
        "{notices:?}"
    );
}

#[test]
fn a_move_another_intents_is_making_is_left_to_it() {
    let w = world();
    put(&w.dirs.config.join("spaces.toml.moving"), "");
    assert_eq!(w.settle(), Vec::<String>::new());
    assert!(w.root().join("skoolscout/discover-schools").is_dir());
    assert_eq!(current_name(&w.dirs, "skoolscout"), "skoolscout");
}
