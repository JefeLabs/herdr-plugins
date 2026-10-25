use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use herdr_capabilities::progress::{
    Evidence, PlanTasks, Poller, PrState, Update, agent_statuses, bar, plan_evidence, plan_tasks,
    pr_number, pr_state, progress,
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
fn an_unknown_pr_state_shows_no_state_word() {
    let ev = Evidence {
        plans: vec![],
        prs: vec![(7, PrState::Unknown)],
    };
    let (fill, label, _) = p("## Artifacts\n- pr: #7\n", &ev);
    assert!(close(fill, 7.0), "still past planning: {fill}");
    assert_eq!(label, "PR #7");
    let ev = Evidence {
        plans: vec![],
        prs: vec![(1, PrState::Merged), (2, PrState::Unknown)],
    };
    let (fill, label, _) = p("## Artifacts\n- pr: #1\n- pr: #2\n", &ev);
    assert!(close(fill, 7.0));
    assert_eq!(label, "2 PRs", "no merged count while one is unknown");
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

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?} in {}", dir.display());
}

/// Live-shaped `herdr agent list`: one named blocked agent, two unnamed.
const AGENT_LIST: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"working","pane_id":"w5:p25"},{"agent":"claude","agent_status":"blocked","name":"intent-notify-547e","pane_id":"w9:p1"},{"agent":"claude","agent_status":"idle","pane_id":"w5:p9"}],"type":"agent_list"}}"#;

fn herdr_env(tmp: &Path, with_list: bool) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "HERDR_BIN_PATH".to_string(),
            fixtures().join("fake-herdr.sh").display().to_string(),
        ),
        (
            "FAKE_HERDR_LOG".to_string(),
            tmp.join("herdr.log").display().to_string(),
        ),
    ];
    if with_list {
        let list = tmp.join("agents.json");
        fs::write(&list, AGENT_LIST).unwrap();
        env.push(("FAKE_AGENT_LIST".to_string(), list.display().to_string()));
    }
    env
}

#[test]
fn agents_without_names_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let env = herdr_env(tmp.path(), true);
    let map = agent_statuses(&fixtures().join("fake-herdr.sh"), &env).unwrap();
    assert_eq!(map.len(), 1, "{map:?}");
    assert_eq!(map["intent-notify-547e"], "blocked");
}

#[test]
fn a_failing_herdr_gives_none() {
    let tmp = tempfile::tempdir().unwrap();
    let env = herdr_env(tmp.path(), false);
    assert!(agent_statuses(&fixtures().join("fake-herdr.sh"), &env).is_none());
    assert!(agent_statuses(&tmp.path().join("no-such-herdr"), &env).is_none());
}

#[test]
fn pr_state_reads_gh_and_degrades() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("prs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("7"), "MERGED").unwrap();
    fs::write(dir.join("9"), "OPEN").unwrap();
    let env = vec![("FAKE_GH_DIR".to_string(), dir.display().to_string())];
    let gh = fixtures().join("fake-gh.sh");
    assert_eq!(pr_state(&gh, tmp.path(), "#7", &env), PrState::Merged);
    assert_eq!(pr_state(&gh, tmp.path(), "#9", &env), PrState::Open);
    assert_eq!(pr_state(&gh, tmp.path(), "#8", &env), PrState::Unknown);
    assert_eq!(
        pr_state(&tmp.path().join("no-gh"), tmp.path(), "#7", &env),
        PrState::Unknown
    );
}

fn sent_with(body: &str) -> (tempfile::TempDir, herdr_capabilities::store::Intent) {
    intent(State::Sent, body)
}

#[test]
fn plan_evidence_finds_the_ledger_in_a_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("main");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let wt = tmp.path().join("wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    fs::create_dir_all(wt.join("docs/superpowers/plans")).unwrap();
    fs::write(
        wt.join("docs/superpowers/plans/p.md"),
        "### Task 1: a\n### Task 2: b\n",
    )
    .unwrap();
    let sdd = wt.join(".superpowers/sdd/p");
    fs::create_dir_all(&sdd).unwrap();
    fs::write(
        sdd.join("progress.md"),
        "# SDD ledger — plan: docs/superpowers/plans/p.md\nTask 1: complete (x)\n",
    )
    .unwrap();
    // a ledger for another plan must not be used
    let other = wt.join(".superpowers/sdd/q");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("progress.md"),
        "# SDD ledger — plan: docs/superpowers/plans/q.md\nTask 1: complete\nTask 2: complete\n",
    )
    .unwrap();
    let (_t, it) = sent_with("## Artifacts\n- plan: docs/superpowers/plans/p.md\n");
    assert_eq!(plan_evidence(Some(&repo), &it), vec![tasks(1, 2)]);
}

#[test]
fn plan_evidence_without_repo_marks_unreadable() {
    let (_t, it) = sent_with("## Artifacts\n- plan: docs/superpowers/plans/p.md\n");
    assert_eq!(
        plan_evidence(None, &it),
        vec![PlanTasks {
            done: 0,
            total: 0,
            readable: false
        }]
    );
}

#[test]
fn poller_reports_on_a_channel_without_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let mut poller = Poller::new(herdr_env(tmp.path(), true));
    poller.request_agents();
    poller.request_agents(); // already in flight: not run twice
    let start = Instant::now();
    let mut updates = Vec::new();
    while updates.is_empty() && start.elapsed() < Duration::from_secs(5) {
        updates.extend(poller.drain());
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(200));
    updates.extend(poller.drain());
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert!(matches!(&updates[0], Update::Agents(Some(m)) if m.len() == 1));
    let calls = fs::read_to_string(tmp.path().join("herdr.log")).unwrap();
    assert_eq!(calls.matches("agent list").count(), 1);
}

#[test]
fn pr_state_asks_gh_about_the_pr_as_linked() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("prs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("1850"), "MERGED").unwrap();
    fs::write(dir.join("9"), "OPEN").unwrap();
    let log = tmp.path().join("gh.log");
    let env = vec![
        ("FAKE_GH_DIR".to_string(), dir.display().to_string()),
        ("FAKE_GH_LOG".to_string(), log.display().to_string()),
    ];
    let gh = fixtures().join("fake-gh.sh");
    let url = "https://github.com/skoolscout/jefelabs-com/pull/1850";
    assert_eq!(pr_state(&gh, tmp.path(), url, &env), PrState::Merged);
    assert_eq!(pr_state(&gh, tmp.path(), "o/other#9", &env), PrState::Open);
    let calls = fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains(&format!("pr view {url}")),
        "the URL itself, not its number: {calls}"
    );
    assert!(
        calls.contains("-R o/other"),
        "owner/repo#n names its repo: {calls}"
    );
    assert_eq!(pr_number("o/other#9"), Some(9));
}
