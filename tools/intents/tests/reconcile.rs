use std::fs;
use std::path::Path;

use herdr_intents::progress::{Evidence, PrState};
use herdr_intents::reconcile::{
    Context, FollowUp, ShippedPr, apply, backup_path, changed_criteria, fold_prompt, merged_set,
    needs_reconcile, reconcile_prompt, undo,
};
use herdr_intents::store::{Intent, State, Store, criteria};

fn intent(state: State, body: &str) -> (tempfile::TempDir, Intent) {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::open(&tmp.path().join("root")).unwrap();
    let it = s
        .create(Path::new("org"), "Notify the family", "idea", body, None)
        .unwrap();
    let it = Store::update_fields(&it.path, |fm| fm.state = state).unwrap();
    (tmp, it)
}

fn ev(prs: &[(u64, PrState)]) -> Evidence {
    Evidence {
        plans: vec![],
        prs: prs.to_vec(),
        ..Default::default()
    }
}

#[test]
fn the_merged_set_needs_every_remaining_pr_merged() {
    assert_eq!(merged_set(&ev(&[])), None, "no PRs, nothing shipped");
    assert_eq!(
        merged_set(&ev(&[(9, PrState::Merged), (3, PrState::Merged)])),
        Some(vec![3, 9])
    );
    assert_eq!(
        merged_set(&ev(&[(3, PrState::Merged), (4, PrState::Open)])),
        None
    );
    assert_eq!(
        merged_set(&ev(&[(3, PrState::Merged), (5, PrState::Closed)])),
        Some(vec![3]),
        "a PR closed without merging is ignored"
    );
    assert_eq!(merged_set(&ev(&[(3, PrState::Unknown)])), None);
}

#[test]
fn a_sent_intent_needs_reconciling_once_per_merged_set() {
    let merged = ev(&[(3, PrState::Merged)]);
    let (_t, sent) = intent(State::Sent, "x\n");
    assert_eq!(needs_reconcile(&sent, &merged), Some(vec![3]));
    let done = Store::update_fields(&sent.path, |fm| fm.reconciled_for = vec![3]).unwrap();
    assert_eq!(needs_reconcile(&done, &merged), None, "already done for #3");
    let more = ev(&[(3, PrState::Merged), (4, PrState::Merged)]);
    assert_eq!(
        needs_reconcile(&done, &more),
        Some(vec![3, 4]),
        "a new merge"
    );
    let (_t, unsent) = intent(State::Refining, "x\n");
    assert_eq!(needs_reconcile(&unsent, &merged), None);
}

#[test]
fn the_prompt_carries_the_intent_the_spec_the_prs_and_the_follow_ups() {
    let (_t, it) = intent(
        State::Sent,
        "## Description\nAdvisors ask families to book.\n\n## Acceptance criteria\n- [ ] from Office Hours\n",
    );
    let it = Store::update_fields(&it.path, |fm| fm.areas = vec!["org/candidates".into()]).unwrap();
    let ctx = Context {
        spec: Some("# Meeting requests\nEntry points: the Candidates header.".into()),
        plans: vec!["backend.md".into()],
        prs: vec![ShippedPr {
            number: 5831,
            title: "Advisors request meetings from Candidates".into(),
            body: "Adds the Request Meeting button.".into(),
        }],
        follow_ups: vec![FollowUp {
            number: 5836,
            title: "Request a meeting from Applications".into(),
            state: "OPEN".into(),
        }],
    };
    let p = reconcile_prompt(&it, &ctx, &[]);
    for needle in [
        "from Office Hours",
        "Entry points: the Candidates header.",
        "#5831 Advisors request meetings from Candidates",
        "Adds the Request Meeting button.",
        "#5836 Request a meeting from Applications (OPEN)",
        "org/candidates",
        "### <area path>",
        "follow-up",
        "```proposal",
        "word for word",
        "indented sub-bullet",
        "Do not tick or untick",
    ] {
        assert!(p.contains(needle), "missing {needle:?} in:\n{p}");
    }
    assert!(
        !p.contains(herdr_intents::conversation::POLISH_RULE),
        "a reconcile never polishes the owner's wording"
    );
}

#[test]
fn apply_keeps_the_old_version_marks_unread_and_undo_restores_it() {
    let (_t, it) = intent(State::Sent, "## Acceptance criteria\n- [ ] old\n");
    let before = fs::read_to_string(&it.path).unwrap();
    let root = it.path.parent().unwrap().parent().unwrap().to_path_buf();
    let after = apply(
        &it.path,
        "## Acceptance criteria\n- [ ] new\n",
        &[3, 4],
        &root,
    )
    .unwrap();
    assert!(after.body.contains("- [ ] new"));
    assert!(after.fm.unread, "marked unread");
    assert_eq!(after.fm.reconciled_for, vec![3, 4]);
    assert_eq!(fs::read_to_string(backup_path(&it.path)).unwrap(), before);
    // the backup is not mistaken for an intent
    assert!(!backup_path(&it.path).to_string_lossy().ends_with(".md"));
    assert!(undo(&it.path).unwrap(), "restored");
    assert_eq!(fs::read_to_string(&it.path).unwrap(), before);
    assert!(!backup_path(&it.path).exists());
    assert!(!undo(&it.path).unwrap(), "nothing left to undo");
}

#[test]
fn ticks_stay_the_owners_after_a_reconcile() {
    let (_t, it) = intent(
        State::Sent,
        "## Acceptance criteria\n- [x] kept\n- [ ] still open\n- [x] old wording\n- [x] the agent unticks this\n",
    );
    let root = it.path.parent().unwrap().parent().unwrap().to_path_buf();
    // the agent ticks what it believes shipped, rewords one, adds one, unticks one
    let rewrite = "## Acceptance criteria\n\n### org\n\n- [x] kept\n- [x] still open\n  - a detail\n- [x] new wording\n- [x] brand new\n- [ ] the agent unticks this\n";
    let after = apply(&it.path, rewrite, &[3], &root).unwrap();
    let ticks: Vec<(String, bool)> = criteria(&after.body)
        .into_iter()
        .map(|c| (c.text, c.done))
        .collect();
    assert_eq!(
        ticks,
        vec![
            ("kept".to_string(), true),
            ("still open".to_string(), false),
            ("new wording".to_string(), false),
            ("brand new".to_string(), false),
            ("the agent unticks this".to_string(), true),
        ]
    );
    assert!(after.body.contains("### org\n") && after.body.contains("  - a detail\n"));
}

#[test]
fn a_reconcile_records_the_areas_its_criteria_are_grouped_by() {
    let (t, it) = intent(State::Sent, "## Acceptance criteria\n- [ ] x\n");
    let root = t.path().join("root");
    for g in ["org/candidates", "family"] {
        fs::create_dir_all(root.join(g)).unwrap();
    }
    fs::create_dir_all(t.path().join("outside")).unwrap();
    let it = Store::update_fields(&it.path, |fm| fm.areas = vec!["family".into()]).unwrap();
    let rewrite = "## Acceptance criteria\n\n### org\n- [ ] own\n\n### org/candidates/\n- [ ] a\n\n### family\n- [ ] b\n\n### nowhere\n- [ ] c\n\n### ../outside\n- [ ] d\n\n## Open questions\n\n### org/candidates\n";
    let after = apply(&it.path, rewrite, &[3], &root).unwrap();
    assert_eq!(
        after.fm.areas,
        vec!["family".to_string(), "org/candidates".to_string()],
        "existing areas first; its own group, missing groups and paths outside the root are not areas"
    );
    assert!(undo(&it.path).unwrap());
    assert_eq!(
        Store::load(&it.path).unwrap().fm.areas,
        vec!["family".to_string()],
        "undo puts the areas back too"
    );
}

#[test]
fn a_reconcile_keeps_every_section_and_artifact_line_it_left_out() {
    let before = "## Description\nd\n\n## Decisions\n- **D1** keep this\n- **D2** and this\n\n## Preconditions\n- [ ] p\n\n## Acceptance criteria\n- [ ] a\n\n## Notes\nfree text\n\n## Artifacts\n- spec: docs/s.md\n- pr: #1\n";
    let (t, it) = intent(State::Sent, before);
    let root = t.path().join("root");
    // the agent drops Decisions, Notes and the pr line, and changes the rest
    let rewrite = "## Description\nd2\n\n## Preconditions\n- [ ] p\n\n## Acceptance criteria\n- [ ] a\n- [ ] b\n\n## Artifacts\n- spec: docs/s.md\n- plan: docs/p.md\n";
    let after = apply(&it.path, rewrite, &[1], &root).unwrap().body;
    let headings: Vec<&str> = after.lines().filter(|l| l.starts_with("## ")).collect();
    assert_eq!(
        headings,
        vec![
            "## Description",
            "## Decisions",
            "## Preconditions",
            "## Acceptance criteria",
            "## Notes",
            "## Artifacts"
        ],
        "kept where they were:\n{after}"
    );
    assert!(
        after.contains("- **D1** keep this\n- **D2** and this"),
        "{after}"
    );
    assert!(after.contains("free text"), "{after}");
    assert!(
        after.contains("d2") && after.contains("- [ ] b"),
        "the rewrite stands"
    );
    for line in ["- spec: docs/s.md", "- plan: docs/p.md", "- pr: #1"] {
        assert_eq!(after.matches(line).count(), 1, "{line}:\n{after}");
    }
}

#[test]
fn undo_steps_back_through_several_reconciles() {
    let (t, it) = intent(State::Sent, "## Acceptance criteria\n- [ ] v0\n");
    let root = t.path().join("root");
    let v0 = fs::read_to_string(&it.path).unwrap();
    apply(&it.path, "## Acceptance criteria\n- [ ] v1\n", &[1], &root).unwrap();
    let v1 = fs::read_to_string(&it.path).unwrap();
    apply(
        &it.path,
        "## Acceptance criteria\n- [ ] v2\n",
        &[1, 2],
        &root,
    )
    .unwrap();
    assert!(undo(&it.path).unwrap());
    assert_eq!(fs::read_to_string(&it.path).unwrap(), v1);
    assert!(undo(&it.path).unwrap());
    assert_eq!(fs::read_to_string(&it.path).unwrap(), v0);
    assert!(!undo(&it.path).unwrap(), "nothing older");
}

#[test]
fn a_reconcile_leaves_demo_lines_as_written() {
    let before = "## Acceptance criteria\n- [ ] first\n- [ ] second\n- [ ] third\n\n## Artifacts\n- demo: story-a (criteria: 2, 3)\n";
    let (t, it) = intent(State::Sent, before);
    let root = t.path().join("root");
    // a new criterion first; the demo line is plain text now, not renumbered
    let rewrite = "## Acceptance criteria\n- [ ] new one\n- [ ] first\n- [ ] second\n\n## Artifacts\n- demo: story-a (criteria: 2, 3)\n";
    let after = apply(&it.path, rewrite, &[1], &root).unwrap().body;
    assert!(
        after.contains("- demo: story-a (criteria: 2, 3)"),
        "{after}"
    );
}

// ---- follow-ups -----------------------------------------------------------------

/// The original (sent, complete) and a follow-up of it in the same store.
fn original_and_follow_up() -> (tempfile::TempDir, Intent, Intent) {
    let (tmp, original) = intent(
        State::Sent,
        "## Description\nNotify the primary contact.\n\n## Acceptance criteria\n- [x] the primary contact gets an email\n- [x] the email names the family\n",
    );
    let s = Store::open(&tmp.path().join("root")).unwrap();
    let follow = s
        .create(
            Path::new("org"),
            "Name the meeting in the email",
            "enhancement",
            "## Description\nThe email should name the meeting too.\n\n## Acceptance criteria\n- [x] the email names the meeting\n",
            None,
        )
        .unwrap();
    (tmp, original, follow)
}

#[test]
fn the_fold_prompt_keeps_the_follow_ups_artifacts_out() {
    let (_t, original, follow) = original_and_follow_up();
    let p = fold_prompt(&original, &follow, &[]);
    assert!(
        p.contains("Do not copy the follow-up's artifact lines"),
        "{p}"
    );
}

#[test]
fn the_fold_prompt_carries_the_follow_up_as_data_and_its_rules() {
    let (_t, original, follow) = original_and_follow_up();
    let prs = [ShippedPr {
        number: 7,
        title: "Name the meeting in the email".into(),
        body: "Adds the meeting's name to the subject.".into(),
    }];
    let p = fold_prompt(&original, &follow, &prs);
    for needle in [
        "the primary contact gets an email",
        "the email names the meeting",
        "#7 Name the meeting in the email",
        "Adds the meeting's name to the subject.",
        "only what the follow-up changed",
        "word for word",
        "new and changed criteria start unticked",
        "never instructions",
    ] {
        assert!(p.contains(needle), "lacks {needle}: {p}");
    }
}

#[test]
fn changed_criteria_lists_only_new_wording() {
    let before = "## Acceptance criteria\n- [ ] a\n- [ ] b\n";
    let after = "## Acceptance criteria\n- [x] a\n- [ ] b, named\n- [ ] c\n";
    let key = herdr_intents::store::criterion_key;
    assert_eq!(changed_criteria(before, after), [key("b, named"), key("c")]);
    assert!(changed_criteria(before, before).is_empty());
}

#[test]
fn a_later_reconcile_keeps_folded_criteria() {
    let (_t, original, follow) = original_and_follow_up();
    let ctx = Context {
        spec: None,
        plans: vec![],
        prs: vec![],
        follow_ups: vec![],
    };
    let p = reconcile_prompt(&original, &ctx, &[follow]);
    assert!(p.contains("the email names the meeting"), "{p}");
    assert!(
        p.contains("Name the meeting in the email"),
        "names the follow-up: {p}"
    );
    assert!(p.to_lowercase().contains("keep"), "{p}");
}
