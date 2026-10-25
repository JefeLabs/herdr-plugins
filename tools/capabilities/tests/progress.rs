use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use herdr_capabilities::progress::{
    CellKind, Evidence, FoundPr, PlanTasks, Poller, PrState, Progress, Size, Update,
    agent_statuses, ago, bar, branch_area, plan_evidence, plan_size, plan_tasks, pr_number,
    pr_state, progress, prs_mentioning, stage_cells,
};

/// The bar as `(glyphs, kinds)`: D done (`█`), T to do (`░`), R in review (`▓`).
fn bar_of(p: &Progress) -> (String, String) {
    let cells = stage_cells(p);
    (
        cells.iter().map(|c| c.ch).collect(),
        cells
            .iter()
            .map(|c| match c.kind {
                CellKind::Done => 'D',
                CellKind::Todo => 'T',
                CellKind::Review => 'R',
            })
            .collect(),
    )
}

fn pair(g: &str, k: &str) -> (String, String) {
    (g.to_string(), k.to_string())
}
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
        counted: true,
        ..Default::default()
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
                counted: false,
                ..Default::default()
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
    assert_eq!(
        plan_tasks(plan, None),
        PlanTasks {
            done: 0,
            total: 3,
            readable: true,
            counted: false,
            ..Default::default()
        },
        "no ledger: the tasks are known, how many are done is not (steps are not tasks)"
    );
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
    assert_eq!(map["intent-notify-547e"].status, "blocked");
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
            readable: false,
            counted: false,
            ..Default::default()
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

#[test]
fn agent_worktree_and_slot_come_from_its_working_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let area = tmp.path().join("repo/.claude/worktrees/org/team");
    fs::create_dir_all(area.join("app-ui")).unwrap();
    fs::write(area.join(".slot"), "2\n").unwrap();
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let list = tmp.path().join("agents.json");
    fs::write(
        &list,
        format!(
            r#"{{"result":{{"agents":[{{"name":"a","agent_status":"working","foreground_cwd":"{}"}},{{"name":"b","agent_status":"idle","cwd":"{}"}}]}}}}"#,
            area.join("app-ui").display(),
            elsewhere.display()
        ),
    )
    .unwrap();
    let mut env = herdr_env(tmp.path(), false);
    env.push(("FAKE_AGENT_LIST".to_string(), list.display().to_string()));
    let map = agent_statuses(&fixtures().join("fake-herdr.sh"), &env).unwrap();
    assert_eq!(map["a"].worktree.as_deref(), Some("org/team"));
    assert_eq!(map["a"].worktree_path.as_deref(), Some(area.as_path()));
    assert_eq!(map["a"].slot, Some(2));
    assert_eq!(map["b"].worktree, None, "no .slot above it");
    assert_eq!(map["b"].worktree_path, None);
    assert_eq!(map["b"].slot, None);
}

#[test]
fn time_since_sent_reads_compactly() {
    let t0 = chrono::Local::now();
    let at = |secs: i64| t0 + chrono::Duration::seconds(secs);
    assert_eq!(ago(t0, at(30)), "<1m ago");
    assert_eq!(ago(t0, at(45 * 60)), "45m ago");
    assert_eq!(ago(t0, at(15 * 3600 + 35 * 60)), "15h ago");
    assert_eq!(ago(t0, at(3 * 86400 + 4 * 3600)), "3d ago");
    assert_eq!(ago(t0, at(-90)), "<1m ago");
}

#[test]
fn without_a_ledger_building_shows_no_count_and_no_share() {
    let uncounted = PlanTasks {
        done: 0,
        total: 5,
        readable: true,
        counted: false,
        ..Default::default()
    };
    let ev = Evidence {
        plans: vec![tasks(2, 8), uncounted],
        prs: vec![],
    };
    let (_tmp, it) = intent(State::Sent, "## Artifacts\n- plan: a.md\n- plan: b.md\n");
    let pr = progress(&it, &ev).unwrap();
    assert_eq!(pr.label, "building", "no guessed count");
    // each plan shows its own evidence: the ledgered L plan a quarter done,
    // the unledgered M plan empty
    assert_eq!(pr.plans.len(), 2);
    assert_eq!((pr.plans[0].cells, pr.plans[0].fill), (3, 0.25));
    assert_eq!((pr.plans[1].cells, pr.plans[1].fill), (2, 0.0));
}

#[test]
fn segments_follow_the_stages_reached() {
    let ev = Evidence {
        plans: vec![tasks(1, 3)],
        prs: vec![],
    };
    let (_tmp, it) = intent(
        State::Sent,
        "## Acceptance criteria\n- [x] a\n- [ ] b\n\n## Artifacts\n- spec: s.md\n- plan: p.md\n",
    );
    let pr = progress(&it, &ev).unwrap();
    assert_eq!(pr.segments, [1.0, 1.0, 1.0, 1.0 / 3.0, 0.0, 0.0, 0.5]);
    assert_eq!(
        bar_of(&pr),
        pair("▉▉▉▍░░█░", "DDDDTTDT"),
        "one S plan, a third done"
    );
    // ticks alone light only the verified block
    let (_tmp, it) = intent(State::Sent, "## Acceptance criteria\n- [x] a\n- [ ] b\n");
    let pr = progress(&it, &Evidence::default()).unwrap();
    assert_eq!(bar_of(&pr), pair("▉░░░░░░░█░", "DTTTTTTTDT"));
    // a merged PR fills every stage up to merged
    let ev = Evidence {
        plans: vec![],
        prs: vec![(1, PrState::Merged)],
    };
    let (_tmp, it) = intent(State::Sent, "## Artifacts\n- pr: #1\n");
    let pr = progress(&it, &ev).unwrap();
    assert_eq!(bar_of(&pr), pair("▉▉▉██▉▉▉░░", "DDDDDDDDTT"));
}

#[test]
fn prs_are_found_by_the_file_names_they_mention() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("prs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("search-p.md.json"),
        r#"[{"number":5828,"state":"MERGED","headRefName":"feature/org/office-hours--meeting-request-backend"},{"number":5832,"state":"OPEN"}]"#,
    )
    .unwrap();
    let log = tmp.path().join("gh.log");
    let env = vec![
        ("FAKE_GH_DIR".to_string(), dir.display().to_string()),
        ("FAKE_GH_LOG".to_string(), log.display().to_string()),
    ];
    let gh = fixtures().join("fake-gh.sh");
    assert_eq!(
        prs_mentioning(&gh, tmp.path(), "p.md", &env),
        vec![
            FoundPr {
                number: 5828,
                state: PrState::Merged,
                branch: "feature/org/office-hours--meeting-request-backend".into()
            },
            FoundPr {
                number: 5832,
                state: PrState::Open,
                branch: String::new()
            }
        ]
    );
    assert!(prs_mentioning(&gh, tmp.path(), "other.md", &env).is_empty());
    assert!(prs_mentioning(&tmp.path().join("no-gh"), tmp.path(), "p.md", &env).is_empty());
    let calls = fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("pr list --state all --search \"p.md\""),
        "{calls}"
    );
}

#[test]
fn plan_size_comes_from_tasks_and_files() {
    assert_eq!(plan_size(0, 0), None);
    assert_eq!(plan_size(3, 8), Some(Size::S));
    assert_eq!(plan_size(0, 3), Some(Size::S));
    assert_eq!(plan_size(5, 15), Some(Size::M));
    assert_eq!(plan_size(8, 32), Some(Size::L));
    assert_eq!(plan_size(7, 28), Some(Size::L));
    assert_eq!(plan_size(12, 10), Some(Size::XL));
    assert_eq!(plan_size(2, 45), Some(Size::XL), "the larger scale wins");
}

#[test]
fn plan_files_are_the_distinct_paths_it_names() {
    let plan = "### Task 1: a\n**Files:**\n- Create: `src/a.rs`\n- Modify: `src/b.rs:12-20`\n- Test: `tests/t.rs`\n### Task 2: b\n- Modify: `src/a.rs`\n";
    let t = plan_tasks(plan, None);
    assert_eq!((t.total, t.files), (2, 3));
}

#[test]
fn building_holds_one_block_per_sized_plan() {
    let plan = |total, files, pr| PlanTasks {
        done: 0,
        total,
        readable: true,
        counted: false,
        files,
        pr,
    };
    // contract (no size), backend L merged, org L merged, family M in review
    let ev = Evidence {
        plans: vec![
            plan(0, 0, None),
            plan(8, 32, Some(PrState::Merged)),
            plan(7, 28, Some(PrState::Merged)),
            plan(5, 15, Some(PrState::Open)),
        ],
        prs: vec![
            (5828, PrState::Merged),
            (5831, PrState::Merged),
            (5832, PrState::Open),
        ],
    };
    let (_tmp, it) = intent(
        State::Sent,
        "## Artifacts\n- spec: s.md\n- plan: c.md\n- plan: b.md\n- plan: o.md\n- plan: f.md\n",
    );
    let pr = progress(&it, &ev).unwrap();
    assert_eq!(bar_of(&pr), pair("▉▉▉██▉██▉▓▓▉░░░", "DDDDDDDDDRRDTTT"));
    // a counted plan fills by its ledger share: M is two cells, a quarter done
    let ev = Evidence {
        plans: vec![PlanTasks {
            done: 1,
            total: 4,
            readable: true,
            counted: true,
            files: 0,
            pr: None,
        }],
        prs: vec![],
    };
    let (_tmp, it) = intent(State::Sent, "## Artifacts\n- plan: p.md\n");
    assert_eq!(
        bar_of(&progress(&it, &ev).unwrap()),
        pair("▉▉▉▌░░░░░", "DDDDTTTTT"),
        "a linked plan implies the spec stage"
    );
    // no sized plan: the building block keeps its three cells
    let (_tmp, it) = intent(State::Sent, "x\n");
    assert_eq!(
        bar_of(&progress(&it, &Evidence::default()).unwrap()),
        pair("▉░░░░░░░░░", "DTTTTTTTTT")
    );
}

#[test]
fn blocks_touch_and_borders_show_where_they_meet() {
    // no gap cells: done blocks end in a seven-eighths cell, shades divide the rest
    let ev = Evidence {
        plans: vec![],
        prs: vec![(1, PrState::Open)],
    };
    let (_tmp, it) = intent(State::Sent, "## Artifacts\n- pr: #1\n");
    let (glyphs, kinds) = bar_of(&progress(&it, &ev).unwrap());
    assert!(
        !glyphs.contains(' ') && !glyphs.contains('▕'),
        "no gap cells at all: {glyphs}"
    );
    assert_eq!(
        (glyphs.as_str(), kinds.as_str()),
        ("▉▉▉██▉▉░░░", "DDDDDDDTTT")
    );
}

#[test]
fn a_workbrokr_branch_names_its_area() {
    assert_eq!(
        branch_area("feature/org/candidates--meeting-request-org").as_deref(),
        Some("org/candidates")
    );
    assert_eq!(
        branch_area("fix/family/schedule-meeting--meeting-request-family").as_deref(),
        Some("family/schedule-meeting")
    );
    assert_eq!(branch_area("main"), None);
    assert_eq!(branch_area("feature/no-area-marker"), None);
    assert_eq!(branch_area("changeset-release/develop"), None);
}
