use std::fs;
use std::path::Path;

use herdr_capabilities::progress::{Evidence, PrState};
use herdr_capabilities::reconcile::{
    Context, FollowUp, ShippedPr, apply, backup_path, merged_set, needs_reconcile,
    reconcile_prompt, undo,
};
use herdr_capabilities::store::{Intent, State, Store, criteria};

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
fn a_sent_capability_needs_reconciling_once_per_merged_set() {
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
fn the_prompt_carries_the_capability_the_spec_the_prs_and_the_follow_ups() {
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
    let p = reconcile_prompt(&it, &ctx);
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
    // the backup is not mistaken for a capability
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
