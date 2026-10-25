use std::fs;
use std::path::Path;

use herdr_intents::store::{SaveError, State, Store, open_questions, slugify};

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::open(&tmp.path().join("root")).unwrap();
    (tmp, s)
}

#[test]
fn open_creates_root_and_inbox() {
    let (tmp, _s) = store();
    assert!(tmp.path().join("root/Inbox").is_dir());
}

#[test]
fn create_writes_front_matter_and_body() {
    let (tmp, s) = store();
    let it = s
        .create(
            Path::new("Inbox"),
            "Pending label on skipped folder",
            "issue",
            "## Observed\n\nx\n",
            None,
        )
        .unwrap();
    let name = it.path.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.ends_with("-pending-label-on-skipped-folder.md"),
        "{name}"
    );
    assert_eq!(&name[4..5], "-", "date prefix YYYY-MM-DD-: {name}");
    let text = fs::read_to_string(&it.path).unwrap();
    assert!(text.starts_with("---\n"));
    for key in [
        "title:",
        "kind: issue",
        "state: raw",
        "created:",
        "updated:",
        "tags: []",
        "polished_at: null",
        "sent_to: null",
        "conversations: {}",
    ] {
        assert!(text.contains(key), "missing {key} in:\n{text}");
    }
    assert!(text.ends_with("## Observed\n\nx\n"));
    assert_eq!(it.fm.state, State::Raw);
    assert_eq!(tmp.path().join("root/Inbox"), it.path.parent().unwrap());
}

#[test]
fn round_trip_preserves_unknown_fields_and_defaults_missing() {
    let (tmp, _s) = store();
    let p = tmp.path().join("root/Inbox/hand.md");
    fs::write(
        &p,
        "---\ntitle: Hand written\nkind: idea\ncustom: 1\ntags: a\n---\nbody\n",
    )
    .unwrap();
    let mut it = Store::load(&p).unwrap();
    assert_eq!(it.fm.state, State::Raw, "missing state defaults to raw");
    assert_eq!(it.fm.tags, ["a"], "scalar tags become a one-item list");
    it.body = "body2\n".into();
    Store::save(&mut it).unwrap();
    let text = fs::read_to_string(&p).unwrap();
    assert!(text.contains("custom: 1"), "unknown field kept:\n{text}");
    assert!(text.contains("state: raw"));
    assert!(text.ends_with("body2\n"));
}

#[test]
fn scan_builds_tree_sorted() {
    let (_tmp, s) = store();
    s.create_group(Path::new(""), "skoolscout").unwrap();
    s.create_group(Path::new("skoolscout"), "applications")
        .unwrap();
    s.create(
        Path::new("skoolscout/applications"),
        "Zed thing",
        "idea",
        "",
        None,
    )
    .unwrap();
    s.create(
        Path::new("skoolscout/applications"),
        "Alpha thing",
        "idea",
        "",
        None,
    )
    .unwrap();
    s.create(Path::new("Inbox"), "In the inbox", "issue", "", None)
        .unwrap();
    let tree = s.scan().unwrap();
    let names: Vec<&str> = tree.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Inbox", "skoolscout"]);
    let apps = &tree.groups[1].groups[0];
    assert_eq!(apps.name, "applications");
    let titles: Vec<&str> = apps.intents.iter().map(|i| i.fm.title.as_str()).collect();
    assert_eq!(titles, ["Alpha thing", "Zed thing"]);
    assert_eq!(tree.groups[0].intents.len(), 1);
}

#[test]
fn move_intent_takes_assets_and_transcript_along() {
    let (tmp, s) = store();
    s.create_group(Path::new(""), "later").unwrap();
    let it = s
        .create(Path::new("Inbox"), "Movable", "idea", "", None)
        .unwrap();
    let shot = tmp.path().join("shot.png");
    fs::write(&shot, b"png").unwrap();
    Store::attach(&it, &shot).unwrap();
    Store::append_transcript(&it, "user", "hello").unwrap();
    let moved = s.move_intent(&it, Path::new("later")).unwrap();
    assert!(moved.path.starts_with(tmp.path().join("root/later")));
    let stem = moved
        .path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(
        tmp.path()
            .join("root/later")
            .join(format!("{stem}.assets/shot.png"))
            .is_file()
    );
    assert!(
        tmp.path()
            .join("root/later")
            .join(format!("{stem}.chat.md"))
            .is_file()
    );
    assert!(!it.path.exists());
}

#[test]
fn save_detects_concurrent_change() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("Inbox"), "Shared", "idea", "a\n", None)
        .unwrap();
    let mut first = Store::load(&it.path).unwrap();
    let mut second = Store::load(&it.path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    second.body = "second\n".into();
    Store::save(&mut second).unwrap();
    first.body = "first\n".into();
    assert!(matches!(Store::save(&mut first), Err(SaveError::Conflict)));
    Store::save_force(&mut first).unwrap();
    assert_eq!(Store::load(&it.path).unwrap().body, "first\n");
}

#[test]
fn attach_copies_file_and_lists_size() {
    let (tmp, s) = store();
    let it = s
        .create(Path::new("Inbox"), "With shot", "issue", "", None)
        .unwrap();
    let shot = tmp.path().join("screen.png");
    fs::write(&shot, vec![0u8; 1234]).unwrap();
    let dest = Store::attach(&it, &shot).unwrap();
    assert!(
        dest.ends_with("with-shot.assets/screen.png")
            || dest.to_string_lossy().contains(".assets/screen.png")
    );
    assert_eq!(
        Store::attachments(&it),
        vec![("screen.png".to_string(), 1234u64)]
    );
}

#[test]
fn transcript_appends_role_blocks() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("Inbox"), "Chatty", "idea", "", None)
        .unwrap();
    Store::append_transcript(&it, "user", "make it crisper").unwrap();
    Store::append_transcript(&it, "agent", "Here you go").unwrap();
    let text = fs::read_to_string(it.path.with_extension("chat.md")).unwrap();
    assert!(text.contains("### user "), "{text}");
    assert!(text.contains("make it crisper\n"));
    assert!(text.contains("### agent "));
}

#[test]
fn open_questions_counts_only_that_section() {
    let body = "## Observed\n- not a question\n\n## Open questions\n- Which org?\n- Since when?\n\n## Later\n- nope\n";
    assert_eq!(open_questions(body), 2);
    assert_eq!(open_questions("no section"), 0);
}

#[test]
fn slugify_is_short_and_safe() {
    assert_eq!(
        slugify("Pending label on skipped folder!"),
        "pending-label-on-skipped-folder"
    );
    assert!(slugify(&"x".repeat(200)).len() <= 60);
}
