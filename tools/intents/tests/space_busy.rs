//! A space is busy while a check, capture or recording runs in it: the move
//! to its meta repo's name waits for it.

use std::fs;
use std::path::Path;

use herdr_intents::capture::{space_busy, space_lock_path};
use herdr_intents::store::Store;

fn lock(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A pid that is surely not running: a child that has already ended.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

struct World {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    meta: std::path::PathBuf,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("spaces");
    let meta = tmp.path().join("skoolscout-com");
    fs::create_dir_all(&meta).unwrap();
    let store = Store::open(&root).unwrap();
    store
        .create(
            Path::new("skoolscout/org"),
            "Office hours",
            "idea",
            "x\n",
            None,
        )
        .unwrap();
    World {
        _tmp: tmp,
        root,
        meta,
    }
}

impl World {
    fn busy(&self) -> bool {
        space_busy(&self.root, &self.root.join("skoolscout"), &self.meta)
    }
    fn assets(&self) -> std::path::PathBuf {
        let md = fs::read_dir(self.root.join("skoolscout/org"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "md"))
            .unwrap();
        md.with_extension("assets")
    }
}

#[test]
fn a_quiet_space_is_not_busy() {
    let w = world();
    assert!(!w.busy());
}

#[test]
fn a_check_or_recording_running_in_the_space_makes_it_busy() {
    let w = world();
    let check = w.assets().join("checks/1/.capture.lock");
    lock(&check, &dead_pid().to_string());
    assert!(!w.busy(), "a lock left by a run that ended");
    lock(&check, &std::process::id().to_string());
    assert!(w.busy());
}

#[test]
fn evidence_being_captured_for_one_of_its_intents_makes_it_busy() {
    let w = world();
    let stem = w
        .assets()
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let evidence = w
        .root
        .join(".polish")
        .join(&stem)
        .join("evidence/.capture.lock");
    lock(&evidence, &std::process::id().to_string());
    assert!(w.busy());
    // another space's intent capturing evidence does not hold this one back
    fs::remove_file(&evidence).unwrap();
    lock(
        &w.root
            .join(".polish/2026-01-01-elsewhere/evidence/.capture.lock"),
        &std::process::id().to_string(),
    );
    assert!(!w.busy());
}

#[test]
fn a_run_holding_the_meta_repos_lock_makes_it_busy() {
    let w = world();
    let held = space_lock_path(&w.meta);
    lock(
        &held,
        &format!("{} http://localhost:3000", std::process::id()),
    );
    let busy = w.busy();
    lock(&held, &format!("{} http://localhost:3000", dead_pid()));
    let stale = w.busy();
    let _ = fs::remove_file(&held);
    assert!(busy);
    assert!(!stale, "a lock left by a run that ended");
}
