use std::path::Path;

use herdr_capabilities::progress::{
    Evidence, PlanTasks, PrState, bar, plan_tasks, pr_number, progress,
};
use herdr_capabilities::store::{Intent, State, Store};

fn intent(state: State, body: &str) -> (tempfile::TempDir, Intent) {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::open(&tmp.path().join("root")).unwrap();
    let it = s
        .create(Path::new("Inbox"), "Cap", "idea", body, None)
        .unwrap();
    let it = Store::update_fields(&it.path, |fm| fm.state = state).unwrap();
    (tmp, it)
}

/// `(fill, label, complete)` for a sent capability with `body`.
fn p(body: &str, ev: &Evidence) -> (f32, String, bool) {
    let (_tmp, it) = intent(State::Sent, body);
    let pr = progress(&it, ev).expect("sent capabilities have progress");
    (pr.fill, pr.label, pr.complete)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn tasks(done: usize, total: usize) -> PlanTasks {
    PlanTasks {
        done,
        total,
        readable: true,
    }
}

const TWO_CRITERIA_ONE_TICKED: &str = "## Acceptance criteria\n- [x] one\n- [ ] two\n";

#[test]
fn sent_with_nothing_reads_speccing() {
    let (fill, label, complete) = p("x\n", &Evidence::default());
    assert!(close(fill, 1.0), "{fill}");
    assert_eq!(label, "speccing");
    assert!(!complete);
}

#[test]
fn decisions_only_read_charting() {
    let (fill, label, _) = p(
        "## Artifacts\n- decision: #1\n- decision: #2\n",
        &Evidence::default(),
    );
    assert!(close(fill, 1.0));
    assert_eq!(label, "charting · 2 decisions");
}

#[test]
fn spec_or_issue_reads_planning() {
    let (fill, label, _) = p("## Artifacts\n- issue: #12\n", &Evidence::default());
    assert!(close(fill, 2.0), "{fill}");
    assert_eq!(label, "planning");
    let (fill, label, _) = p(
        "## Artifacts\n- spec: docs/superpowers/specs/a.md\n",
        &Evidence::default(),
    );
    assert!(close(fill, 2.0));
    assert_eq!(label, "planning");
}

#[test]
fn plans_sum_tasks() {
    let ev = Evidence {
        plans: vec![tasks(2, 4), tasks(1, 5)],
        prs: vec![],
    };
    let (fill, label, _) = p("## Artifacts\n- plan: a.md\n- plan: b.md\n", &ev);
    assert_eq!(label, "building 3/9");
    assert!(close(fill, 3.0 + 3.0 * 3.0 / 9.0), "{fill}");
}

#[test]
fn a_plan_without_tasks_counts_as_planned() {
    let ev = Evidence {
        plans: vec![tasks(0, 0)],
        prs: vec![],
    };
    let (fill, label, _) = p("## Artifacts\n- plan: a.md\n", &ev);
    assert!(close(fill, 3.0), "{fill}");
    assert_eq!(label, "building");
}

#[test]
fn an_unreadable_plan_shows_a_question_mark() {
    let ev = Evidence {
        plans: vec![
            PlanTasks {
                done: 0,
                total: 0,
                readable: false,
            },
            tasks(1, 2),
        ],
        prs: vec![],
    };
    let (fill, label, _) = p("## Artifacts\n- plan: a.md\n- plan: b.md\n", &ev);
    assert_eq!(label, "building ?");
    assert!(
        close(fill, 3.0 + 3.0 * 0.5),
        "other plans still count: {fill}"
    );
}

#[test]
fn an_open_pr_skips_missing_plan_stages() {
    let ev = Evidence {
        plans: vec![],
        prs: vec![(5830, PrState::Open)],
    };
    let (fill, label, _) = p("## Artifacts\n- pr: #5830\n", &ev);
    assert!(close(fill, 7.0), "{fill}");
    assert_eq!(label, "PR #5830 open");
}

#[test]
fn an_unknown_pr_state_counts_as_open() {
    let ev = Evidence {
        plans: vec![],
        prs: vec![(7, PrState::Unknown)],
    };
    let (fill, label, _) = p("## Artifacts\n- pr: #7\n", &ev);
    assert!(close(fill, 7.0));
    assert_eq!(label, "PR #7 open");
}

#[test]
fn several_prs_read_merged_count() {
    let ev = Evidence {
        plans: vec![],
        prs: vec![
            (1, PrState::Merged),
            (2, PrState::Open),
            (3, PrState::Closed),
        ],
    };
    let (fill, label, _) = p("## Artifacts\n- pr: #1\n- pr: #2\n- pr: #3\n", &ev);
    assert_eq!(label, "PRs 1/2 merged");
    assert!(close(fill, 7.0), "closed ignored, one still open: {fill}");
}

#[test]
fn only_closed_prs_count_as_none() {
    let ev = Evidence {
        plans: vec![],
        prs: vec![(3, PrState::Closed)],
    };
    let (fill, label, _) = p("## Artifacts\n- pr: #3\n", &ev);
    assert!(close(fill, 1.0));
    assert_eq!(label, "speccing");
}

#[test]
fn merged_reads_verifying() {
    let ev = Evidence {
        plans: vec![],
        prs: vec![(1, PrState::Merged)],
    };
    let body = format!("{TWO_CRITERIA_ONE_TICKED}\n## Artifacts\n- pr: #1\n");
    let (fill, label, complete) = p(&body, &ev);
    assert!(close(fill, 8.0 + 1.0), "{fill}");
    assert_eq!(label, "verifying 1/2");
    assert!(!complete);
}

#[test]
fn ticks_never_jump_the_bar() {
    let (fill, label, _) = p(TWO_CRITERIA_ONE_TICKED, &Evidence::default());
    assert!(close(fill, 1.0 + 1.0), "only the verified share: {fill}");
    assert_eq!(label, "speccing");
}

#[test]
fn all_ticked_is_complete_even_before_merge() {
    let (_, label, complete) = p(
        "## Acceptance criteria\n- [x] one\n- [x] two\n",
        &Evidence::default(),
    );
    assert!(complete);
    assert_eq!(label, "✓ complete");
}

#[test]
fn no_criteria_after_merge_says_so() {
    let ev = Evidence {
        plans: vec![],
        prs: vec![(1, PrState::Merged)],
    };
    let (fill, label, complete) = p("## Artifacts\n- pr: #1\n", &ev);
    assert!(close(fill, 8.0));
    assert_eq!(label, "no criteria");
    assert!(!complete);
}

#[test]
fn unsent_has_no_progress() {
    let (_tmp, it) = intent(State::Refining, "x\n");
    assert!(progress(&it, &Evidence::default()).is_none());
}

#[test]
fn ledger_counts_distinct_completed_tasks() {
    let plan = "### Task 1: a\n- [ ] x\n### Task 2: b\n- [x] y\n### Task 3: c\n";
    let ledger = "# SDD ledger — plan: docs/p.md\nTask 1: complete (commits a..b)\nTask 1: complete (again)\nTask 3: complete\nTask 2: Ruling: something\n";
    assert_eq!(plan_tasks(plan, Some(ledger)), tasks(2, 3));
    assert_eq!(plan_tasks(plan, None), tasks(1, 2), "checkbox fallback");
    assert_eq!(plan_tasks("# no tasks\n", None), tasks(0, 0));
}

#[test]
fn pr_numbers_from_refs_and_urls() {
    assert_eq!(pr_number("#5830"), Some(5830));
    assert_eq!(pr_number(" #12 "), Some(12));
    assert_eq!(pr_number("https://github.com/o/r/pull/12"), Some(12));
    assert_eq!(pr_number("https://github.com/o/r/pull/12/files"), Some(12));
    assert_eq!(pr_number("nope"), None);
}

#[test]
fn bar_draws_partial_cells() {
    assert_eq!(bar(2.5, 10), "██▌░░░░░░░");
    assert_eq!(bar(0.0, 4), "░░░░");
    assert_eq!(bar(10.0, 10), "██████████");
    assert_eq!(bar(12.0, 10), "██████████", "clamped");
}
