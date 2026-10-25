use std::fs;
use std::path::Path;

use herdr_capabilities::store::{
    Readiness, SaveError, State, Store, add_artifact, add_checklist_item, add_question,
    answer_question, artifacts, criteria, discover_artifacts, open_questions, preconditions,
    questions, readiness, send_refusal, slugify, toggle_criterion, toggle_precondition,
};

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

// I2: realistic hand-edits must still load
#[test]
fn lenient_front_matter_loads_hand_edits() {
    let (tmp, _s) = store();
    let cases: &[(&str, &str)] = &[
        ("empty-tags", "---\ntitle: T\nkind: idea\ntags:\n---\nb\n"),
        (
            "number-tags",
            "---\ntitle: T\nkind: idea\ntags: [2024, x]\n---\nb\n",
        ),
        ("no-title", "---\nkind: idea\n---\nb\n"),
        (
            "state-done",
            "---\ntitle: T\nkind: idea\nstate: done\n---\nb\n",
        ),
        (
            "state-caps",
            "---\ntitle: T\nkind: idea\nstate: Ready\n---\nb\n",
        ),
        ("crlf", "---\r\ntitle: T\r\nkind: idea\r\n---\r\nb\r\n"),
        (
            "date-only",
            "---\ntitle: T\nkind: idea\ncreated: 2026-10-24\n---\nb\n",
        ),
        (
            "space-ts",
            "---\ntitle: T\nkind: idea\ncreated: 2026-10-24 18:02:11\n---\nb\n",
        ),
    ];
    for (name, text) in cases {
        let p = tmp.path().join("root/Inbox").join(format!("{name}.md"));
        fs::write(&p, text).unwrap();
        let it = Store::load(&p).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(it.body.trim(), "b", "{name}");
    }
    let it = Store::load(&tmp.path().join("root/Inbox/number-tags.md")).unwrap();
    assert_eq!(it.fm.tags, ["2024", "x"]);
    let it = Store::load(&tmp.path().join("root/Inbox/no-title.md")).unwrap();
    assert_eq!(it.fm.title, "no-title", "title defaults to the file stem");
    assert_eq!(
        Store::load(&tmp.path().join("root/Inbox/state-caps.md"))
            .unwrap()
            .fm
            .state,
        State::Ready
    );
    assert_eq!(
        Store::load(&tmp.path().join("root/Inbox/state-done.md"))
            .unwrap()
            .fm
            .state,
        State::Raw,
        "unknown state reads as raw"
    );
}

#[test]
fn unparseable_file_shows_up_as_a_broken_entry_not_silently_dropped() {
    let (tmp, s) = store();
    fs::write(
        tmp.path().join("root/Inbox/bad.md"),
        "---\ntitle: [unclosed\n---\nb\n",
    )
    .unwrap();
    let tree = s.scan().unwrap();
    let inbox = tree.groups.iter().find(|g| g.name == "Inbox").unwrap();
    assert_eq!(inbox.broken.len(), 1, "{:?}", inbox.broken);
    assert!(inbox.broken[0].0.ends_with("bad.md"));
    assert!(!inbox.broken[0].1.is_empty(), "carries the parse error");
}

#[test]
fn missing_updated_does_not_cause_a_false_conflict() {
    let (tmp, _s) = store();
    let p = tmp.path().join("root/Inbox/old.md");
    fs::write(&p, "---\ntitle: Old\nkind: idea\n---\nb\n").unwrap();
    let mut it = Store::load(&p).unwrap();
    it.body = "c\n".into();
    Store::save(&mut it).expect("first save of a file without `updated` must not conflict");
}

// C2: field-only changes must not clobber another writer's edits
#[test]
fn update_fields_reloads_before_writing() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("Inbox"), "Shared", "idea", "a\n", None)
        .unwrap();
    let stale = Store::load(&it.path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut other = Store::load(&it.path).unwrap();
    other.body = "other wrote this\n".into();
    Store::save_force(&mut other).unwrap();
    let fresh = Store::update_fields(&stale.path, |fm| fm.state = State::Ready).unwrap();
    assert_eq!(
        fresh.body, "other wrote this\n",
        "body from disk, not from the stale copy"
    );
    assert_eq!(fresh.fm.state, State::Ready);
    assert_eq!(Store::load(&it.path).unwrap().body, "other wrote this\n");
}

#[test]
fn save_meta_does_not_bump_updated() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("Inbox"), "Meta", "idea", "a\n", None)
        .unwrap();
    let before = Store::load(&it.path).unwrap().fm.updated;
    std::thread::sleep(std::time::Duration::from_millis(1100));
    Store::update_meta(&it.path, |fm| {
        fm.conversations.insert("agy".into(), "c1".into());
    })
    .unwrap();
    let after = Store::load(&it.path).unwrap();
    assert_eq!(
        after.fm.updated.unwrap().timestamp(),
        before.unwrap().timestamp(),
        "meta writes keep `updated`"
    );
    assert_eq!(after.fm.conversations["agy"], "c1");
}

const STRUCTURED: &str = "## Description\nParents who skip see Pending.\n\n## Acceptance criteria\n- [ ] Skipped folders read Skipped\n- [x] Nothing changes for uploaded folders\n\n## Open questions\n- Which org saw this?\n  → IYM, on Oct 20\n- Since when?\n";

#[test]
fn questions_parse_with_optional_answers() {
    let qs = questions(STRUCTURED);
    assert_eq!(qs.len(), 2);
    assert_eq!(qs[0].text, "Which org saw this?");
    assert_eq!(qs[0].answer.as_deref(), Some("IYM, on Oct 20"));
    assert_eq!(qs[1].text, "Since when?");
    assert!(qs[1].answer.is_none());
    assert_eq!(
        open_questions(STRUCTURED),
        1,
        "only unanswered questions are open"
    );
}

#[test]
fn answer_question_writes_the_answer_under_the_question() {
    let body = answer_question(STRUCTURED, 1, "Since the 10-20 release").unwrap();
    assert!(
        body.contains("- Since when?\n  → Since the 10-20 release\n"),
        "{body}"
    );
    assert_eq!(open_questions(&body), 0);
    // answering again replaces the old answer
    let body = answer_question(&body, 1, "actually 10-21").unwrap();
    assert!(
        body.contains("- Since when?\n  → actually 10-21\n") && !body.contains("10-20 release"),
        "{body}"
    );
    assert!(
        answer_question(STRUCTURED, 9, "x").is_none(),
        "out of range"
    );
}

#[test]
fn criteria_parse_and_toggle() {
    let cs = criteria(STRUCTURED);
    assert_eq!(cs.len(), 2);
    assert_eq!(
        (cs[0].text.as_str(), cs[0].done),
        ("Skipped folders read Skipped", false)
    );
    assert_eq!(
        (cs[1].text.as_str(), cs[1].done),
        ("Nothing changes for uploaded folders", true)
    );
    let body = toggle_criterion(STRUCTURED, 0).unwrap();
    assert!(
        body.contains("- [x] Skipped folders read Skipped"),
        "{body}"
    );
    let body = toggle_criterion(&body, 1).unwrap();
    assert!(
        body.contains("- [ ] Nothing changes for uploaded folders"),
        "{body}"
    );
}

#[test]
fn preconditions_are_their_own_checklist() {
    let body = "## Description\nx\n\n## Preconditions\n- [ ] Report cards are optional at that point\n- [x] Pending is shared across folders\n\n## Acceptance criteria\n- [ ] Skipped folders read Skipped\n";
    let pre = preconditions(body);
    assert_eq!(pre.len(), 2);
    assert!(!pre[0].done && pre[1].done);
    assert_eq!(criteria(body).len(), 1, "criteria unaffected");
    let body = toggle_precondition(body, 0).unwrap();
    assert!(body.contains("- [x] Report cards are optional"), "{body}");
    assert!(
        body.contains("- [ ] Skipped folders read Skipped"),
        "criteria untouched: {body}"
    );
}

#[test]
fn add_checklist_item_appends_under_the_heading_creating_it_if_missing() {
    let body = "## Description\nx\n\n## Preconditions\n- [ ] one\n\n## Open questions\n- q?\n";
    let out = add_checklist_item(body, "## Preconditions", "two");
    assert!(
        out.contains("## Preconditions\n- [ ] one\n- [ ] two\n"),
        "{out}"
    );
    let out = add_checklist_item(&out, "## Acceptance criteria", "a check");
    assert!(
        out.contains("## Acceptance criteria\n- [ ] a check\n"),
        "created before Open questions: {out}"
    );
    assert!(
        out.find("## Acceptance criteria").unwrap() < out.find("## Open questions").unwrap(),
        "{out}"
    );
    let out = add_question(&out, "new q?");
    assert!(out.contains("- q?\n- new q?\n"), "{out}");
    let out = add_question("## Description\nonly\n", "first?");
    assert!(out.ends_with("## Open questions\n- first?\n"), "{out}");
}

#[test]
fn artifacts_parse_kind_and_target_and_can_be_added() {
    let body = "## Description\nx\n\n## Artifacts\n- spec: docs/superpowers/specs/2026-10-24-x-design.md\n- plan: docs/superpowers/plans/2026-10-24-x.md\n- adr: docs/adr/0007-event-sourced.md\n- decision: #5813\n- issue: https://github.com/o/r/issues/5821\n- note without a kind\n";
    let a = artifacts(body);
    assert_eq!(a.len(), 6);
    assert_eq!(
        (a[0].kind.as_str(), a[0].target.as_str()),
        ("spec", "docs/superpowers/specs/2026-10-24-x-design.md")
    );
    assert_eq!(
        (a[3].kind.as_str(), a[3].target.as_str()),
        ("decision", "#5813")
    );
    assert_eq!(
        (a[5].kind.as_str(), a[5].target.as_str()),
        ("link", "note without a kind")
    );
    let out = add_artifact("## Description\nx\n", "spec", "docs/x.md");
    assert!(out.contains("## Artifacts\n- spec: docs/x.md\n"), "{out}");
    let out = add_artifact(&out, "plan", "docs/y.md");
    assert!(
        out.contains("- spec: docs/x.md\n- plan: docs/y.md\n"),
        "{out}"
    );
}

#[test]
fn discover_artifacts_finds_specs_plans_and_adrs_that_match_the_title() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    for d in [
        "docs/superpowers/specs",
        "docs/superpowers/plans",
        "docs/adr",
        "docs/other",
    ] {
        fs::create_dir_all(repo.join(d)).unwrap();
    }
    fs::write(
        repo.join("docs/superpowers/specs/2026-10-24-skipped-folder-label-design.md"),
        "# Skipped folder label\n\nFamilies who skip a folder see Pending.\n",
    )
    .unwrap();
    fs::write(
        repo.join("docs/superpowers/plans/2026-10-25-skipped-folder-label.md"),
        "# Skipped folder label Implementation Plan\n",
    )
    .unwrap();
    fs::write(
        repo.join("docs/adr/0007-pending-vs-skipped.md"),
        "# ADR 7: Pending vs Skipped folder state\n",
    )
    .unwrap();
    fs::write(
        repo.join("docs/adr/0008-unrelated.md"),
        "# ADR 8: Caching\n",
    )
    .unwrap();
    fs::write(repo.join("docs/other/skipped-folder-notes.md"), "# notes\n").unwrap();
    let found = discover_artifacts(
        repo,
        "Families can skip a folder without seeing Pending",
        &["docs/superpowers/specs/2026-10-24-skipped-folder-label-design.md".to_string()],
    );
    let targets: Vec<(&str, &str)> = found
        .iter()
        .map(|a| (a.kind.as_str(), a.target.as_str()))
        .collect();
    assert!(
        targets.contains(&(
            "plan",
            "docs/superpowers/plans/2026-10-25-skipped-folder-label.md"
        )),
        "{targets:?}"
    );
    assert!(
        targets.contains(&("adr", "docs/adr/0007-pending-vs-skipped.md")),
        "{targets:?}"
    );
    assert!(
        !targets.iter().any(|(_, t)| t.contains("0008")),
        "unrelated ADR excluded: {targets:?}"
    );
    assert!(
        !targets
            .iter()
            .any(|(_, t)| t.contains("2026-10-24-skipped-folder-label-design")),
        "already linked ones are not repeated: {targets:?}"
    );
    assert!(
        !targets.iter().any(|(_, t)| t.contains("docs/other")),
        "only the known folders are scanned: {targets:?}"
    );
    assert!(discover_artifacts(Path::new("/nonexistent"), "x", &[]).is_empty());
}

#[test]
fn blank_template_placeholders_are_not_items() {
    let body = "## Acceptance criteria\n\n- [ ] \n- [ ] real\n\n## Open questions\n\n- \n- who?\n";
    assert_eq!(questions(body).len(), 1);
    assert_eq!(open_questions(body), 1);
    assert_eq!(criteria(body).len(), 1);
    // index 0 is the first real item, not the placeholder
    let answered = answer_question(body, 0, "owners").unwrap();
    assert!(answered.contains("- who?\n  → owners"), "{answered}");
    let ticked = toggle_criterion(body, 0).unwrap();
    assert!(ticked.contains("- [x] real"), "{ticked}");
    assert!(
        ticked.contains("- [ ] \n"),
        "placeholder untouched: {ticked}"
    );
}

#[test]
fn readiness_follows_state_and_open_questions() {
    let (_tmp, s) = store();
    let mk = |title: &str, state: State, body: &str| {
        let it = s
            .create(Path::new("Inbox"), title, "idea", body, None)
            .unwrap();
        Store::update_fields(&it.path, |fm| fm.state = state).unwrap()
    };
    let two_open = "## Open questions\n- a?\n- b?\n";
    let answered = "## Open questions\n- a?\n  → yes\n";
    let raw = mk("raw one", State::Raw, "x\n");
    let refining_two_open = mk("refining open", State::Refining, two_open);
    let refining_answered = mk("refining answered", State::Refining, answered);
    let ready = mk("ready one", State::Ready, answered);
    let sent_with_open = mk("sent one", State::Sent, two_open);
    assert_eq!(readiness(&raw), Readiness::Raw);
    assert_eq!(readiness(&refining_two_open), Readiness::Open(2));
    assert_eq!(readiness(&refining_answered), Readiness::Ready);
    assert_eq!(readiness(&ready), Readiness::Ready);
    assert_eq!(readiness(&sent_with_open), Readiness::Sent);
    assert_eq!(Readiness::Open(2).label(), "?2");
    assert_eq!(Readiness::Raw.label(), "raw");
    assert!(send_refusal(&raw).unwrap().contains("raw"));
    assert!(
        send_refusal(&refining_two_open)
            .unwrap()
            .contains("2 open questions")
    );
    assert_eq!(send_refusal(&refining_answered), None);
    assert_eq!(send_refusal(&ready), None);
}
