use std::fs;
use std::path::Path;

use herdr_intents::store::{
    Readiness, SaveError, State, Store, acknowledge_criteria, add_artifact, add_checklist_item,
    add_question, answer_question, artifacts, criteria, criteria_groups, criterion_key,
    discover_artifacts, new_criteria, open_questions, preconditions, questions, readiness,
    send_refusal, slugify, toggle_criterion, toggle_precondition,
};

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::open(&tmp.path().join("root")).unwrap();
    (tmp, s)
}

#[test]
fn open_creates_the_root_and_no_inbox() {
    let (tmp, _s) = store();
    assert!(tmp.path().join("root").is_dir());
    assert!(!tmp.path().join("root/Inbox").exists(), "no default group");
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
    fs::create_dir_all(tmp.path().join("root/Inbox")).unwrap();
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
    fs::create_dir_all(tmp.path().join("root/Inbox")).unwrap();
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
    fs::create_dir_all(tmp.path().join("root/Inbox")).unwrap();
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
    fs::create_dir_all(tmp.path().join("root/Inbox")).unwrap();
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

#[test]
fn affected_areas_round_trip_and_stay_out_of_the_file_when_empty() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("Inbox"), "Spans areas", "idea", "x\n", None)
        .unwrap();
    assert!(
        !std::fs::read_to_string(&it.path)
            .unwrap()
            .contains("areas:")
    );
    Store::update_fields(&it.path, |fm| {
        fm.areas = vec!["skoolscout/org".into(), "skoolscout/family".into()]
    })
    .unwrap();
    let back = Store::load(&it.path).unwrap();
    assert_eq!(back.fm.areas, vec!["skoolscout/org", "skoolscout/family"]);
    assert!(
        back.fm.extra.is_empty(),
        "areas is a known field, not extra"
    );
}

#[test]
fn a_criterion_key_ignores_spacing_and_survives_ticking() {
    assert_eq!(
        criterion_key("Family gets  a notice "),
        criterion_key("Family gets a notice")
    );
    assert_ne!(
        criterion_key("Family gets a notice"),
        criterion_key("Family gets an email")
    );
    assert_eq!(criterion_key("x").len(), 16, "a stable 64-bit hex key");
}

#[test]
fn new_criteria_are_the_ones_nobody_acknowledged() {
    let (_tmp, s) = store();
    let body = "## Acceptance criteria\n- [x] original one\n- [ ] original two\n- [ ] added by the agent\n";
    let it = s
        .create(Path::new("org"), "Cap", "idea", body, None)
        .unwrap();
    // before sending, everything is the owner's own
    assert!(new_criteria(&it).is_empty());
    // sent, with no record and no transcript: nothing tells, so nothing is new
    let it = Store::update_fields(&it.path, |fm| fm.state = State::Sent).unwrap();
    assert!(new_criteria(&it).is_empty());
    // with a transcript: criteria found in it count as seen
    fs::write(
        it.transcript_path(),
        "### agent\n```proposal\n- [ ] original one\n- [ ] original two\n```\n",
    )
    .unwrap();
    assert_eq!(new_criteria(&it), vec![2]);
    // acknowledging writes the record (seed included); a reworded one is new again
    let it = acknowledge_criteria(&it.path, &[2]).unwrap();
    assert!(new_criteria(&it).is_empty());
    assert_eq!(it.fm.acknowledged.as_ref().map(Vec::len), Some(3));
    let mut changed = Store::load(&it.path).unwrap();
    changed.body = changed
        .body
        .replace("original two", "original two, reworded");
    Store::save_force(&mut changed).unwrap();
    assert_eq!(new_criteria(&Store::load(&it.path).unwrap()), vec![1]);
}

#[test]
fn criteria_groups_are_the_sub_headings_and_where_they_start() {
    let body = "## Acceptance criteria\n- [ ] own one\n- [ ] \n### org/candidates\n- [x] header button\n- [ ] modal\n### family/dashboard \n- [ ] banner\n\n## Open questions\n### not a group\n- q\n";
    assert_eq!(
        criteria_groups(body),
        vec![
            (1, "org/candidates".to_string()),
            (3, "family/dashboard".to_string())
        ],
        "an empty checkbox is not a criterion, and headings after the section are not groups"
    );
    assert!(criteria_groups("## Acceptance criteria\n- [ ] a\n").is_empty());
}

#[test]
fn found_artifacts_need_whole_words_a_name_match_and_each_word_once() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let specs = repo.join("docs/superpowers/specs");
    fs::create_dir_all(&specs).unwrap();
    let doc = |name: &str, heading: &str| {
        fs::write(specs.join(name), format!("# {heading}\n")).unwrap();
    };
    doc(
        "2026-10-26-reminders-2-meeting-request-chases.md",
        "Meeting request reminders",
    );
    // "specific" is not the start of "specification": one shared word, not two
    doc(
        "2026-09-20-contacts-import-wizard-design.md",
        "Contacts import wizard: specification",
    );
    // two shared words, but only in the heading: the name shares none
    doc(
        "2026-08-07-wizard-topic-badge-design.md",
        "Schedule a Meeting: topic badge",
    );
    let found: Vec<String> = discover_artifacts(
        repo,
        "Advisor can request a primary contact to schedule a specific type of meeting",
        &[],
    )
    .into_iter()
    .map(|a| a.target)
    .collect();
    assert_eq!(
        found,
        vec!["docs/superpowers/specs/2026-10-26-reminders-2-meeting-request-chases.md"]
    );

    // a word the title repeats counts once
    doc(
        "2026-09-17-school-recruitment-profile-design.md",
        "School recruitment profile",
    );
    doc("2026-10-01-school-search-design.md", "School search");
    let found: Vec<String> = discover_artifacts(repo, "School search for every school", &[])
        .into_iter()
        .map(|a| a.target)
        .collect();
    assert_eq!(
        found,
        vec!["docs/superpowers/specs/2026-10-01-school-search-design.md"]
    );
}

// --- workspace scope: one Herdr workspace sees one top-level group

/// Loose intent at the top, one in skoolscout/admin, one in skoolz.
fn three_projects() -> (tempfile::TempDir, Store) {
    let (tmp, s) = store();
    s.create(Path::new(""), "Loose", "idea", "x\n", None)
        .unwrap();
    s.create(Path::new("skoolscout/admin"), "Ours", "idea", "x\n", None)
        .unwrap();
    s.create(Path::new("skoolz"), "Theirs", "idea", "x\n", None)
        .unwrap();
    (tmp, s)
}

#[test]
fn a_workspace_sees_only_its_own_top_level_group() {
    let (_tmp, s) = three_projects();
    let tree = s.scan().unwrap().scoped(Some("skoolscout"));
    let names: Vec<&str> = tree.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["skoolscout"]);
    assert!(tree.intents.is_empty(), "nothing loose at the top");
    assert_eq!(tree.groups[0].groups[0].name, "admin");
    assert_eq!(tree.groups[0].groups[0].intents[0].fm.title, "Ours");
}

#[test]
fn a_workspace_name_matches_its_group_whatever_the_case() {
    let (_tmp, s) = three_projects();
    let tree = s.scan().unwrap().scoped(Some("SkoolScout"));
    assert_eq!(tree.groups.len(), 1);
    assert_eq!(tree.groups[0].name, "skoolscout", "the folder's own name");
    assert_eq!(tree.groups[0].groups.len(), 1);
}

#[test]
fn a_workspace_without_a_group_sees_an_empty_one_to_capture_into() {
    let (_tmp, s) = three_projects();
    let tree = s.scan().unwrap().scoped(Some("tourneyseason"));
    assert_eq!(tree.groups.len(), 1);
    let g = &tree.groups[0];
    assert_eq!(g.name, "tourneyseason");
    assert_eq!(g.path, s.root().join("tourneyseason"));
    assert!(g.groups.is_empty() && g.intents.is_empty());
    assert!(
        !g.path.exists(),
        "nothing is created until something lands there"
    );
}

#[test]
fn without_a_workspace_the_whole_tree_shows() {
    let (_tmp, s) = three_projects();
    let tree = s.scan().unwrap().scoped(None);
    let names: Vec<&str> = tree.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["skoolscout", "skoolz"]);
    assert_eq!(tree.intents.len(), 1, "the loose one too");
}

#[test]
fn captures_go_into_the_workspace_folder_as_it_is_spelled_on_disk() {
    let (_tmp, s) = three_projects();
    assert_eq!(s.scope_group("SkoolScout"), Path::new("skoolscout"));
    assert_eq!(s.scope_group("tourneyseason"), Path::new("tourneyseason"));
}

#[test]
fn polish_dir_round_trips_and_names_the_polish_folder() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("g"), "Split app", "refactor", "x\n", None)
        .unwrap();
    assert!(
        !fs::read_to_string(&it.path).unwrap().contains("polish_dir"),
        "absent until set"
    );
    assert_eq!(it.polish_name(), it.stem());
    Store::update_fields(&it.path, |fm| fm.polish_dir = Some("new-ab12cd34".into())).unwrap();
    let it = Store::load(&it.path).unwrap();
    assert_eq!(it.polish_name(), "new-ab12cd34");
    assert!(
        fs::read_to_string(&it.path)
            .unwrap()
            .contains("polish_dir: new-ab12cd34")
    );
}

#[test]
fn a_polish_dir_that_is_not_one_plain_folder_is_ignored() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("g"), "Split app", "refactor", "x\n", None)
        .unwrap();
    for bad in ["../../elsewhere", "/abs/path", "a/b", "..", ".", "", "x\\y"] {
        Store::update_fields(&it.path, |fm| fm.polish_dir = Some(bad.into())).unwrap();
        let it = Store::load(&it.path).unwrap();
        assert_eq!(
            it.polish_name(),
            it.stem(),
            "{bad:?} must not leave .polish/"
        );
    }
    Store::update_fields(&it.path, |fm| fm.polish_dir = Some("new-ab12cd34".into())).unwrap();
    assert_eq!(Store::load(&it.path).unwrap().polish_name(), "new-ab12cd34");
}

#[test]
fn trash_takes_the_evidence_along() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("skoolscout/admin"), "A bug", "issue", "x\n", None)
        .unwrap();
    let scratch = s
        .root()
        .join(".polish")
        .join(it.polish_name())
        .join("evidence");
    fs::create_dir_all(&scratch).unwrap();
    fs::write(scratch.join("after.json"), "{}").unwrap();
    let kept = it.assets_dir().join("evidence");
    fs::create_dir_all(&kept).unwrap();
    fs::write(kept.join("before.json"), "{}").unwrap();

    let dest = s.trash(&it).unwrap();

    assert!(dest.join("polish/evidence/after.json").is_file());
    assert!(
        dest.join(format!("{}.assets", it.stem()))
            .join("evidence/before.json")
            .is_file()
    );
}

#[test]
fn trash_moves_an_intent_and_everything_it_owns_out_of_the_tree() {
    let (tmp, s) = store();
    let it = s
        .create(
            Path::new("skoolscout/admin"),
            "Gone soon",
            "idea",
            "x\n",
            None,
        )
        .unwrap();
    Store::append_transcript(&it, "user", "hello").unwrap();
    let shot = tmp.path().join("s.png");
    fs::write(&shot, b"png").unwrap();
    Store::attach(&it, &shot).unwrap();
    let polish = s.root().join(".polish").join(it.stem());
    fs::create_dir_all(&polish).unwrap();
    fs::write(polish.join("note.txt"), "x").unwrap();

    let dest = s.trash(&it).unwrap();

    assert!(
        dest.starts_with(s.root().join(".trash")),
        "{}",
        dest.display()
    );
    assert!(!it.path.exists() && !it.transcript_path().exists() && !it.assets_dir().exists());
    assert!(!polish.exists(), "the agent's folder goes too");
    let name = |n: &str| dest.join(n);
    assert!(name(&format!("{}.md", it.stem())).is_file());
    assert!(name(&format!("{}.chat.md", it.stem())).is_file());
    assert!(
        name(&format!("{}.assets", it.stem()))
            .join("s.png")
            .is_file()
    );
    assert!(name("polish").join("note.txt").is_file());
    assert_eq!(
        fs::read_to_string(name("from.txt")).unwrap().trim(),
        "skoolscout/admin",
        "where it came from, to put it back"
    );
    let tree = s.scan().unwrap();
    let mut titles = Vec::new();
    fn walk(g: &herdr_intents::store::Group, out: &mut Vec<String>) {
        out.extend(g.intents.iter().map(|i| i.fm.title.clone()));
        for sub in &g.groups {
            walk(sub, out);
        }
    }
    walk(&tree, &mut titles);
    assert!(!titles.contains(&"Gone soon".to_string()), "{titles:?}");
    // a second trash of the same stem does not collide
    let again = s
        .create(
            Path::new("skoolscout/admin"),
            "Gone soon",
            "idea",
            "x\n",
            None,
        )
        .unwrap();
    let dest2 = s.trash(&again).unwrap();
    assert_ne!(dest, dest2);
}

// ---- the intent's id and version -------------------------------------------

fn is_id(id: &str) -> bool {
    id.len() == 4
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[test]
fn a_new_intent_gets_a_four_character_id_and_version_1() {
    let (_tmp, s) = store();
    let it = s
        .create(
            Path::new("g"),
            "Reinstate",
            "feature",
            "## Description\nx\n",
            None,
        )
        .unwrap();
    let id = it.fm.id.clone().unwrap();
    assert!(is_id(&id), "{id}");
    assert_eq!(it.fm.version, Some(1));
    let back = Store::load(&it.path).unwrap();
    assert_eq!(back.fm.id.as_deref(), Some(id.as_str()), "saved");
    assert_eq!(back.fm.version, Some(1));
}

#[test]
fn a_drawn_id_is_four_lowercase_letters_or_digits() {
    for _ in 0..200 {
        let id = herdr_intents::store::draw_id();
        assert!(is_id(&id), "{id}");
    }
}

#[test]
fn an_id_already_taken_is_drawn_again() {
    let taken: std::collections::HashSet<String> = ["aaaa".to_string()].into();
    let mut draws = vec!["bbbb".to_string(), "aaaa".to_string()];
    let id = herdr_intents::store::unique_id(&taken, || draws.pop().unwrap());
    assert_eq!(id, "bbbb");
}

#[test]
fn ensure_id_gives_an_intent_without_one_or_with_a_twin_its_own() {
    let (_tmp, s) = store();
    let a = s
        .create(Path::new("g"), "One", "feature", "x\n", None)
        .unwrap();
    let b = s
        .create(Path::new("g"), "Two", "feature", "x\n", None)
        .unwrap();
    // an intent from before ids: none
    Store::update_meta(&b.path, |fm| fm.id = None).unwrap();
    let b = s.ensure_id(&b.path).unwrap();
    let b_id = b.fm.id.clone().unwrap();
    assert!(is_id(&b_id));
    assert_ne!(Some(b_id.clone()), a.fm.id);
    // a copied file: the same id as another intent
    Store::update_meta(&b.path, |fm| fm.id = a.fm.id.clone()).unwrap();
    let b = s.ensure_id(&b.path).unwrap();
    assert_ne!(b.fm.id, a.fm.id);
    // a unique id stays
    let again = s.ensure_id(&b.path).unwrap();
    assert_eq!(again.fm.id, b.fm.id);
}

#[test]
fn the_version_counts_text_changes_only() {
    let (_tmp, s) = store();
    let body = "## Description\nx\n\n## Acceptance criteria\n- [ ] one\n";
    let it = s
        .create(Path::new("g"), "Versioned", "feature", body, None)
        .unwrap();
    let v = |p: &Path| Store::load(p).unwrap().fm.version;
    assert_eq!(v(&it.path), Some(1));
    // bookkeeping: a state change, a tick, a link, trailing spaces
    Store::update_fields(&it.path, |fm| fm.state = State::Sent).unwrap();
    let mut x = Store::load(&it.path).unwrap();
    x.body = format!(
        "{}\n## Artifacts\n- pr: #12\n",
        x.body.replace("- [ ] one", "- [x] one  ")
    );
    Store::save_force(&mut x).unwrap();
    assert_eq!(v(&it.path), Some(1), "nothing the video proves changed");
    // the text: the description
    let mut x = Store::load(&it.path).unwrap();
    x.body = x.body.replace("x\n", "the admin reinstates it\n");
    Store::save_force(&mut x).unwrap();
    assert_eq!(v(&it.path), Some(2));
    // an edit in the editor counts the next time the tool writes the file
    let text = fs::read_to_string(&it.path).unwrap();
    fs::write(&it.path, text.replace("- [x] one", "- [x] one, by email")).unwrap();
    Store::update_meta(&it.path, |fm| fm.unread = true).unwrap();
    assert_eq!(v(&it.path), Some(3));
}

#[test]
fn an_intent_from_before_versions_starts_at_1() {
    let (_tmp, s) = store();
    let it = s
        .create(Path::new("g"), "Old", "feature", "x\n", None)
        .unwrap();
    Store::update_meta(&it.path, |fm| {
        fm.version = None;
        fm.version_of = None;
    })
    .unwrap();
    // update_meta itself starts the count
    assert_eq!(Store::load(&it.path).unwrap().fm.version, Some(1));
}

#[test]
fn a_run_stamps_an_id_and_counts_an_edit_made_in_the_editor() {
    let (_tmp, s) = store();
    let it = s
        .create(
            Path::new("g"),
            "Stamped",
            "feature",
            "## Description\nx\n",
            None,
        )
        .unwrap();
    Store::update_meta(&it.path, |fm| fm.id = None).unwrap();
    let text = fs::read_to_string(&it.path).unwrap();
    fs::write(&it.path, text.replace("x\n", "the admin reinstates it\n")).unwrap();
    let stamped = s.stamp_for_run(&it.path).unwrap();
    assert!(stamped.fm.id.as_deref().is_some_and(is_id));
    assert_eq!(
        stamped.fm.version,
        Some(2),
        "the editor's change is counted"
    );
    // nothing new: the file is left as it is
    let before = fs::read_to_string(&it.path).unwrap();
    let again = s.stamp_for_run(&it.path).unwrap();
    assert_eq!(again.fm.version, Some(2));
    assert_eq!(fs::read_to_string(&it.path).unwrap(), before);
}
