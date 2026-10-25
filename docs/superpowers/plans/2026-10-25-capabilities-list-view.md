# capabilities list view Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A full-screen list of every capability showing readiness before sending and a delivery progress bar after, with sending gated on every open question being answered.

**Architecture:** Readiness and the send gate live in `store.rs`/`app.rs`. Progress is a new pure module `progress.rs` (signals in, bar out) plus a gatherer that reads plan files and ledgers across worktrees and polls `gh` and `herdr` on background threads. The list is a second layout of `Screen::Browse`, toggled with `v`, rendered by a new `ui::render_list`.

**Tech Stack:** Rust 2024, ratatui 0.30, crossterm 0.29, serde/serde_json/toml, insta, tempfile. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-25-capabilities-list-view-design.md`

## Global Constraints

- Crate `herdr-capabilities` at `tools/capabilities/`; test command `cargo test -p herdr-capabilities`; `cargo clippy -p herdr-capabilities --all-targets -- -D warnings` clean; `cargo fmt --all`.
- The UI thread never waits on `herdr`, `gh` or `git` while the list shows. A failing tool degrades a column, never the view.
- Never `eprintln!` in TUI paths. Every string drawn from a file or a command goes through `ui::sanitize`.
- The tool never edits the owner's seeded templates in the config directory.
- Sentences the spec fixes are copied verbatim (FORMAT_REMINDER rule, hand-off preamble, `pr:` instruction).
- `herdr` is found through `HERDR_BIN_PATH` (env or `App::herdr_env`), as `send.rs` does today. `gh` is found through `GH_BIN_PATH` (env or `App::herdr_env`), default `gh`.
- Commit after each task: `git add <named paths>` then a conventional message ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No push.

## Review Focus

1. **A capability whose `repo` is missing, not a directory, or not a git repo** must render in the list with a bar from file signals only (Task 4 and Task 6 tests).
2. **`herdr agent list` entries without a `name` field** (most live agents have none) must be skipped, not fail the parse (Task 5 test).
3. **A plan linked by an absolute or `~` path** must resolve the same way the Artifacts reader resolves it (Task 3 test).
4. **Blank template placeholders** (`- ` under Open questions, `- [ ] ` under criteria) must not count as items, or a fresh capability can never be sent and never completes (Task 1 test).
5. **Long or wide titles and control characters** must truncate to the pane width without panicking or wrapping into the next row (Task 7 snapshot at 80 columns).

---

### Task 1: Readiness, blank placeholders and the send gate

**Files:**
- Modify: `tools/capabilities/src/store.rs` (questions, answer_question, checklist, toggle_checklist, stamp, new Readiness)
- Modify: `tools/capabilities/src/app.rs` (`s` handler in `key_browse`; existing send tests)
- Test: `tools/capabilities/tests/store.rs`, `tools/capabilities/src/app.rs` tests

**Interfaces:**
- Produces:
  - `pub enum Readiness { Raw, Open(usize), Ready, Sent }` (derive Debug, Clone, Copy, PartialEq, Eq)
  - `impl Readiness { pub fn label(self) -> String }` → `"raw"`, `"?2"`, `"ready"`, `"sent"`
  - `pub fn readiness(it: &Intent) -> Readiness`
  - `pub fn stamp(intent: &Intent) -> DateTime<Local>` (was private)
  - `pub fn send_refusal(it: &Intent) -> Option<String>` — `None` when sendable, else the message

- [ ] **Step 1: Write the failing tests** in `tests/store.rs`:

```rust
#[test]
fn blank_template_placeholders_are_not_items() {
    let body = "## Acceptance criteria\n\n- [ ] \n- [ ] real\n\n## Open questions\n\n- \n- who?\n";
    assert_eq!(questions(body).len(), 1);
    assert_eq!(open_questions(body), 1);
    assert_eq!(criteria(body).len(), 1);
    // index 0 is the first real item, not the placeholder
    let answered = answer_question(body, 0, "owners").unwrap();
    assert!(answered.contains("- who?\n  → owners"));
    let ticked = toggle_criterion(body, 0).unwrap();
    assert!(ticked.contains("- [x] real"));
    assert!(ticked.contains("- [ ] \n"), "placeholder untouched");
}

#[test]
fn readiness_follows_state_and_open_questions() {
    // build intents with state Raw / Refining+2 open / Refining+0 open / Ready+0 / Sent
    assert_eq!(readiness(&raw), Readiness::Raw);
    assert_eq!(readiness(&refining_two_open), Readiness::Open(2));
    assert_eq!(readiness(&refining_answered), Readiness::Ready);
    assert_eq!(readiness(&ready), Readiness::Ready);
    assert_eq!(readiness(&sent_with_open), Readiness::Sent);
    assert_eq!(Readiness::Open(2).label(), "?2");
    assert!(send_refusal(&raw).unwrap().contains("raw"));
    assert!(send_refusal(&refining_two_open).unwrap().contains("2 open questions"));
    assert_eq!(send_refusal(&refining_answered), None);
}
```

And in `src/app.rs` tests:

```rust
#[test]
fn s_refuses_a_raw_capability_and_one_with_open_questions() {
    let mut fx = setup("agy.jsonl");
    select(&mut fx.app, "Alpha"); // raw
    key(&mut fx.app, KeyCode::Char('s'));
    assert_eq!(fx.app.view().screen, "browse");
    assert!(fx.app.view().message.unwrap().contains("raw"));
    select(&mut fx.app, "In the inbox"); // two open questions
    make_refining(&mut fx.app);
    key(&mut fx.app, KeyCode::Char('s'));
    assert_eq!(fx.app.view().screen, "browse");
    assert!(fx.app.view().message.unwrap().contains("2 open questions"));
}
```

`make_refining(app)` is a new test helper: `Store::update_fields(path, |fm| fm.state = State::Refining)` then `app.rescan()`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p herdr-capabilities blank_template readiness_follows s_refuses`
Expected: compile errors for `Readiness`, `readiness`, `send_refusal`; then the placeholder test fails on counts.

- [ ] **Step 3: Implement.** In `questions`, `checklist`, `answer_question` and `toggle_checklist`, skip items whose text after the marker is empty after `trim()`, so counting and indexing walk the same sequence. `readiness`: `Sent` if state is Sent, `Raw` if state is Raw, `Open(n)` if `open_questions > 0`, else `Ready`. `send_refusal` messages: `"still raw: refine it (p) or mark it ready (R) before sending"` and `"{n} open question{s} still unanswered: answer them before sending (Q)"`. In the `s` handler, call `send_refusal` first and set `self.message` instead of opening `SendPick`.

- [ ] **Step 4: Fix the existing send tests.** The five tests that press `s` on a raw fixture (`grep -n "Char('s'))" src/app.rs`) call `make_refining` before `s`.

- [ ] **Step 5: Run the suite**

Run: `cargo test -p herdr-capabilities`
Expected: all pass.

- [ ] **Step 6: Commit** `feat(capabilities): readiness, blank placeholders and the send gate`

---

### Task 2: Happy-path questions and the hand-off preambles

**Files:**
- Modify: `tools/capabilities/src/conversation.rs` (`FORMAT_REMINDER`)
- Modify: `tools/capabilities/defaults/{idea,issue,enhancement}.polish.md`
- Modify: `tools/capabilities/src/send.rs` (`artifacts_preamble`, new `EDGE_CASES_PREAMBLE`, `render_prompt`)
- Test: `tools/capabilities/tests/conversation.rs`, `tools/capabilities/tests/send.rs`

**Interfaces:**
- Produces: `pub const HAPPY_PATH_RULE: &str` in `conversation.rs`; `pub const EDGE_CASES_PREAMBLE: &str` in `send.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
// tests/conversation.rs
#[test]
fn every_turn_carries_the_happy_path_rule() {
    assert!(FORMAT_REMINDER.contains(HAPPY_PATH_RULE));
    assert_eq!(HAPPY_PATH_RULE, "Open questions are only what you need to know to build the main flow: who, what, where, and what success looks like. Do not ask about edge cases or failure modes; those are handled after hand-off.");
    assert!(seed_prompt(/* existing args */).contains(HAPPY_PATH_RULE));
}

// tests/send.rs
#[test]
fn every_handoff_says_edge_cases_are_the_agents_and_asks_for_pr_lines() {
    for h in builtin_handoffs() {
        let p = render_prompt(&h, &intent, &[], false);
        assert!(p.contains(EDGE_CASES_PREAMBLE.trim_end()));
        assert!(p.contains("`- pr: #<n>` when you open a pull request"));
    }
    assert!(EDGE_CASES_PREAMBLE.starts_with("The happy path is settled: every open question below has its answer. Edge cases are now yours to find. List the ones you see, ask about those that change what gets built, and record each decision as you go."));
}
```

Also a test reading each bundled `defaults/*.polish.md` with `include_str!` and asserting it contains `HAPPY_PATH_RULE`.

- [ ] **Step 2: Run to verify they fail** — `cargo test -p herdr-capabilities happy_path edge_cases` → missing constants.

- [ ] **Step 3: Implement.** Append ` {HAPPY_PATH_RULE}` inside `FORMAT_REMINDER` (keep it one `&str`; build it with `concat!` or a literal that repeats the sentence, and the test pins equality). Confirm `seed_prompt` already appends `FORMAT_REMINDER`; if not, append it. Add the sentence to the three bundled polish templates. In `artifacts_preamble`, extend the kinds list with `` `- pr: #<n>` when you open a pull request ``. Prepend `EDGE_CASES_PREAMBLE` (ending `\n\n`) in `render_prompt` for every handoff, before the artifacts preamble.

- [ ] **Step 4: Run the suite** — `cargo test -p herdr-capabilities` → all pass.

- [ ] **Step 5: Commit** `feat(capabilities): happy-path questions in refinement, edge cases after hand-off`

---

### Task 3: Resolve repo paths across worktrees

**Files:**
- Modify: `tools/capabilities/src/send.rs` (new `worktrees`)
- Modify: `tools/capabilities/src/app.rs` (`resolve_artifact_path`)
- Test: `tools/capabilities/tests/send.rs`

**Interfaces:**
- Produces:
  - `pub fn worktrees(repo: &Path) -> Vec<PathBuf>` — every `worktree <path>` from `git -C <repo> worktree list --porcelain`, main checkout first; `vec![repo]` if git fails.
  - `pub fn resolve_in_repo(repo: &Path, target: &str) -> Option<PathBuf>` — `~`/absolute targets as-is if they exist; otherwise the first `<root>/<target>` that exists over `worktrees(repo)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_file_only_in_a_worktree_resolves() {
    // git init repo; commit one file; git worktree add ../wt -b feature;
    // write wt/docs/superpowers/plans/p.md only in the worktree.
    // git calls use -c user.name=t -c user.email=t@t so CI without identity passes.
    assert_eq!(worktrees(&repo)[0], repo.canonicalize().unwrap());
    assert_eq!(resolve_in_repo(&repo, "docs/superpowers/plans/p.md").unwrap(), wt.join("docs/superpowers/plans/p.md").canonicalize().unwrap());
    assert_eq!(resolve_in_repo(&repo, "docs/nope.md"), None);
    assert_eq!(resolve_in_repo(&tmp.path().join("not-a-repo"), "x.md"), None);
    // absolute target
    assert_eq!(resolve_in_repo(&repo, abs.to_str().unwrap()), Some(abs.clone()));
}
```

Compare canonicalized paths (macOS `/private/var` vs `/var`).

- [ ] **Step 2: Run to verify it fails** — missing functions.

- [ ] **Step 3: Implement**, then make `App::resolve_artifact_path` try `resolve_in_repo` first and fall back to its current behaviour.

- [ ] **Step 4: Run the suite** — all pass, including the existing reader tests.

- [ ] **Step 5: Commit** `feat(capabilities): resolve specs and plans in worktrees`

---

### Task 4: Progress core

**Files:**
- Create: `tools/capabilities/src/progress.rs` (register in `lib.rs`)
- Test: `tools/capabilities/tests/progress.rs`

**Interfaces:**
- Consumes: `store::{Intent, State, artifacts, criteria}`.
- Produces:
  - `pub enum PrState { Open, Merged, Closed, Unknown }`
  - `pub struct PlanTasks { pub done: usize, pub total: usize, pub readable: bool }`
  - `pub struct Evidence { pub plans: Vec<PlanTasks>, pub prs: Vec<(u64, PrState)> }` (Default)
  - `pub struct Progress { pub fill: f32 /* 0.0..=10.0 */, pub label: String, pub complete: bool }`
  - `pub fn progress(it: &Intent, ev: &Evidence) -> Option<Progress>` — `None` unless sent
  - `pub fn plan_tasks(plan: &str, ledger: Option<&str>) -> PlanTasks`
  - `pub fn pr_number(target: &str) -> Option<u64>` — `#5830` or `https://github.com/o/r/pull/5830`
  - `pub fn bar(fill: f32, cells: usize) -> String` — full `█`, partial eighth blocks `▏▎▍▌▋▊▉`, empty `░`

- [ ] **Step 1: Write the failing tests**, one per row of the spec's stage table plus the rules:

```rust
#[test] fn sent_with_nothing_reads_speccing() { assert_eq!(p("", &Evidence::default()), (1.0, "speccing".into(), false)); }
#[test] fn decisions_only_read_charting() { /* two `- decision: #1` lines */ assert_eq!(label, "charting · 2 decisions"); }
#[test] fn spec_or_issue_reads_planning() { /* `- issue: #12` */ assert_eq!((fill, label), (2.0, "planning".into())); }
#[test] fn plans_sum_tasks() { /* two plan lines; plans [2/4, 1/5] */ assert_eq!(label, "building 3/9"); assert!((fill - (3.0 + 3.0 * 3.0 / 9.0)).abs() < 1e-4); }
#[test] fn a_plan_without_tasks_counts_as_planned() { /* plans [0/0] */ assert_eq!((fill, label), (3.0, "building".into())); }
#[test] fn an_unreadable_plan_shows_a_question_mark() { /* plans [readable:false, 1/2] */ assert_eq!(label, "building ?"); }
#[test] fn an_open_pr_skips_missing_plan_stages() { /* no spec, no plan; prs [(5830, Open)] */ assert_eq!((fill, label), (7.0, "PR #5830 open".into())); }
#[test] fn several_prs_read_merged_count() { /* prs [(1,Merged),(2,Open),(3,Closed)] */ assert_eq!(label, "PRs 1/2 merged"); }
#[test] fn merged_reads_verifying() { /* prs [(1,Merged)]; criteria 1 of 2 ticked */ assert_eq!((fill, label), (8.0 + 1.0, "verifying 1/2".into())); }
#[test] fn ticks_never_jump_the_bar() { /* sent, nothing else, criteria 1 of 2 ticked */ assert_eq!(fill, 1.0 + 1.0); assert_eq!(label, "speccing"); }
#[test] fn all_ticked_is_complete_even_before_merge() { /* criteria 2 of 2 ticked, no PR */ assert!(complete); assert_eq!(label, "✓ complete"); }
#[test] fn no_criteria_after_merge_says_so() { assert_eq!(label, "no criteria"); }
#[test] fn unsent_has_no_progress() { assert!(progress(&refining, &Evidence::default()).is_none()); }
#[test] fn ledger_counts_distinct_completed_tasks() {
    let plan = "### Task 1: a\n- [ ] x\n### Task 2: b\n- [x] y\n### Task 3: c\n";
    let ledger = "# SDD ledger — plan: docs/p.md\nTask 1: complete (commits a..b)\nTask 1: complete (again)\nTask 3: complete\n";
    assert_eq!(plan_tasks(plan, Some(ledger)), PlanTasks { done: 2, total: 3, readable: true });
    assert_eq!(plan_tasks(plan, None), PlanTasks { done: 1, total: 2, readable: true }); // checkbox fallback
}
#[test] fn pr_numbers_from_refs_and_urls() { assert_eq!(pr_number("#5830"), Some(5830)); assert_eq!(pr_number("https://github.com/o/r/pull/12"), Some(12)); assert_eq!(pr_number("nope"), None); }
#[test] fn bar_draws_partial_cells() { assert_eq!(bar(2.5, 10), "██▌░░░░░░░"); assert_eq!(bar(0.0, 4), "░░░░"); }
```

`p(body, ev)` is a test helper building a sent intent from `body` and returning `(fill, label, complete)`.

- [ ] **Step 2: Run to verify they fail** — `cargo test -p herdr-capabilities --test progress` → module missing.

- [ ] **Step 3: Implement `progress`.** Stage weights Sent 1, Specced 1, Planned 1, Building 3, PR open 1, Merged 1, Verified 2. Furthest reached of Sent..Merged sets the base: Sent 1, Specced 2, Planned 3, PR open 7, Merged 8; at Planned add `3 * k/N` (0 when N is 0). Then add `2 * j/M` for criteria independently. Closed PRs are dropped before evaluating; `Unknown` counts as open. Label precedence: complete → `✓ complete`; Merged → `verifying j/M` or `no criteria`; PR open → `PR #n open` or `PRs m/t merged`; Planned → `building k/N`, `building`, or `building ?`; Specced → `planning`; Sent → `charting · N decisions` if any `decision:` line, else `speccing`.

- [ ] **Step 4: Run** — `cargo test -p herdr-capabilities --test progress` → pass.

- [ ] **Step 5: Commit** `feat(capabilities): delivery progress from artifacts, plans, PRs and criteria`

---

### Task 5: Gathering evidence off the UI thread

**Files:**
- Modify: `tools/capabilities/src/progress.rs` (gatherers, poller)
- Modify: `tools/capabilities/tests/fixtures/fake-herdr.sh` (answer `agent list`)
- Create: `tools/capabilities/tests/fixtures/fake-gh.sh`
- Test: `tools/capabilities/tests/progress.rs`

**Interfaces:**
- Consumes: `send::{worktrees, resolve_in_repo}`, `plan_tasks`, `pr_number`.
- Produces:
  - `pub fn plan_evidence(repo: Option<&Path>, it: &Intent) -> Vec<PlanTasks>` — for each `plan:` line: resolve, read, find its ledger (a `progress.md` under `<root>/.superpowers/sdd/*/` over every worktree root whose first line ends with the plan's repo-relative path), call `plan_tasks`; unreadable → `readable: false`.
  - `pub fn pr_state(gh: &Path, repo: &Path, n: u64) -> PrState` — `gh pr view <n> --json state` in `repo`; `OPEN`/`MERGED`/`CLOSED`, anything else `Unknown`.
  - `pub fn agent_statuses(herdr: &Path, env: &[(String, String)]) -> Option<HashMap<String, String>>` — `herdr agent list`; map `name → agent_status`; entries without `name` skipped; `None` on failure.
  - `pub struct Poller` with `pub fn new(env: Vec<(String, String)>) -> Self`, `pub fn request_agents(&mut self)`, `pub fn request_pr(&mut self, repo: PathBuf, n: u64)`, `pub fn drain(&mut self) -> Vec<Update>`; `pub enum Update { Agents(Option<HashMap<String, String>>), Pr { repo: PathBuf, n: u64, state: PrState } }`. Each request runs on a spawned thread and reports over an `mpsc` channel; a request already in flight for the same key is not duplicated.

- [ ] **Step 1: Extend the fixtures.** `fake-herdr.sh`: `"agent list"` prints the contents of `$FAKE_AGENT_LIST` (exit 1 if unset). `fake-gh.sh`: logs args to `$FAKE_GH_LOG`; for `pr view <n>` prints `{"state":"<value>"}` where value is read from `$FAKE_GH_DIR/<n>`, exit 1 if that file is missing.

- [ ] **Step 2: Write the failing tests**

```rust
#[test] fn agents_without_names_are_skipped() { /* FAKE_AGENT_LIST = live-shaped JSON with one named blocked agent and two unnamed */ assert_eq!(map.len(), 1); assert_eq!(map["intent-notify-547e"], "blocked"); }
#[test] fn a_failing_herdr_gives_none() { /* FAKE_AGENT_LIST unset */ assert!(agent_statuses(&fake, &env).is_none()); }
#[test] fn pr_state_reads_gh_and_degrades() { /* FAKE_GH_DIR/7 = MERGED */ assert_eq!(pr_state(&gh, &repo, 7), PrState::Merged); assert_eq!(pr_state(&gh, &repo, 8), PrState::Unknown); }
#[test] fn plan_evidence_finds_the_ledger_in_a_worktree() { /* plan + .superpowers/sdd/p/progress.md only in a worktree */ assert_eq!(plan_evidence(Some(&repo), &it), vec![PlanTasks { done: 1, total: 2, readable: true }]); }
#[test] fn plan_evidence_without_repo_marks_unreadable() { assert_eq!(plan_evidence(None, &it_with_one_plan), vec![PlanTasks { done: 0, total: 0, readable: false }]); }
#[test] fn poller_reports_on_a_channel() { /* request_agents; loop drain() up to 2 s */ assert!(matches!(updates[0], Update::Agents(Some(_)))); }
```

- [ ] **Step 3: Run to verify they fail**, then **Step 4: implement**, then **Step 5: run the suite** — all pass.

- [ ] **Step 6: Commit** `feat(capabilities): gather plan, PR and agent evidence off the UI thread`

---

### Task 6: The list model: layout toggle, sort orders, selection, persistence

**Files:**
- Modify: `tools/capabilities/src/app.rs` (new fields, `v`/`S` keys, list key handler, `view()`)
- Modify: `tools/capabilities/src/config.rs` (`LastUsed`)
- Test: `tools/capabilities/src/app.rs` tests

**Interfaces:**
- Consumes: `store::{readiness, stamp}`, `progress::{progress, Evidence}`.
- Produces:
  - `pub enum Layout { Tree, List }`, `pub enum SortOrder { Updated, Progress, Captured, Title }` with `fn next(self) -> Self` and `fn label(self) -> &'static str` (`"Updated"`, …)
  - App fields `layout: Layout`, `sort: SortOrder`, `list: Vec<PathBuf>` (capability paths in display order), `list_selected: usize`, `evidence: HashMap<PathBuf, Evidence>`, `agents: Option<HashMap<String, String>>`
  - `View` gains `pub layout: &'static str` (`"tree"`/`"list"`), `pub list_titles: Vec<String>`, `pub list_selected: usize`, `pub sort: &'static str`
  - `LastUsed` gains `#[serde(default)] pub layout: Option<String>` and `#[serde(default)] pub sort: Option<String>`; a new `pub fn save_view(dirs: &Dirs, layout: &str, sort: &str) -> anyhow::Result<()>` that reads the file, sets only those two fields and writes it back.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn v_toggles_the_list_and_keeps_the_selection() {
    let mut fx = setup("agy.jsonl");
    select(&mut fx.app, "Beta");
    key(&mut fx.app, KeyCode::Char('v'));
    let v = fx.app.view();
    assert_eq!(v.layout, "list");
    assert_eq!(v.list_titles[v.list_selected], "Beta");
    key(&mut fx.app, KeyCode::Char('j'));
    let now = fx.app.view().list_titles[fx.app.view().list_selected].clone();
    key(&mut fx.app, KeyCode::Char('v'));
    assert_eq!(fx.app.view().layout, "tree");
    assert!(fx.app.view().rows[fx.app.view().selected].label.contains(&now));
}
#[test] fn the_list_holds_every_capability_across_groups() { /* 3 fixtures in 2 groups, one in a collapsed sub-group */ assert_eq!(v.list_titles.len(), 3); }
#[test] fn s_cycles_sort_orders() {
    // Title order
    assert_eq!(titles_after_sorting_by("Title"), vec!["Alpha", "Beta", "In the inbox"]);
    // Progress order: raw first, then ?N, then ready, then sent
    assert_eq!(first_title_by("Progress"), /* the raw one with the oldest stamp tie-broken by Updated */);
    // Captured: newest created first (set created via update_fields)
}
#[test] fn enter_in_the_list_opens_the_tree_on_that_capability() { /* v, j, Enter */ assert_eq!(v.layout, "tree"); }
#[test] fn the_layout_and_sort_are_remembered() { /* v, S; drop app; App::new with same dirs */ assert_eq!(v.layout, "list"); assert_eq!(v.sort, "Progress"); }
#[test] fn save_view_keeps_the_provider() { /* save_last_used(provider "claude"); save_view("list","Title"); last_used → provider still "claude" */ }
```

The sort key `S` is uppercase; `key()` sends `KeyCode::Char('S')`.

- [ ] **Step 2: Run to verify they fail.**

- [ ] **Step 3: Implement.** Build `list` from the scanned tree (all groups, ignoring `collapsed`) in `rebuild_rows` and on `S`. Progress order key: `(rank, -open_questions, fill, -stamp)` with rank Raw 0, Open 1, Ready 2, Sent 3, Complete 4. When `layout == List`, `key_browse` routes to `key_list`: `j`/`k`/arrows/`PgUp`/`PgDn` move, `v` back to tree, `S` next order, `Enter` → tree with that capability selected (expand its ancestors), `r` rescan, `q` quit; `s` and `g` come in Task 7. Toggling saves via `save_view`; `App::new` restores from `last_used`. `rescan` and the existing `found_cache` refresh keep working in both layouts.

- [ ] **Step 4: Run the suite** — all pass.

- [ ] **Step 5: Commit** `feat(capabilities): list layout with sort orders and remembered view`

---

### Task 7: Rendering the list, send and go-to-agent, live evidence

**Files:**
- Modify: `tools/capabilities/src/ui.rs` (new `ListRow`, `render_list`)
- Modify: `tools/capabilities/src/app.rs` (`draw`, `tick`, `key_list` `s`/`g`, evidence refresh, status line)
- Modify: `tools/capabilities/README.md`
- Test: `tools/capabilities/src/app.rs` tests (insta snapshots)

**Interfaces:**
- Consumes: everything above; `Poller`, `send_refusal`, `progress::bar`.
- Produces:
  - `pub struct ListRow<'a> { pub status: String, pub progress: Option<(f32, String, bool)>, pub flag: Option<&'a str> /* "! needs you" | "closed" */, pub title: &'a str, pub group: &'a str, pub when: String, pub description: &'a str }`
  - `pub fn render_list(rows: &[ListRow], selected: usize, header: &str, area: Rect, buf: &mut Buffer)`

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn list_snapshot_at_80_and_120_columns() {
    // fixtures: one raw, one ?2, one ready, one sent with a spec + plan (ledger 1/3) + open PR via fake gh,
    // agent blocked via fake herdr; one sent with all criteria ticked; a 200-char title with a control char.
    // drive the poller until updates land (tick() in a loop, 2 s cap), then render.
    insta::assert_snapshot!("list_80", render(&mut fx.app, 80, 20));
    insta::assert_snapshot!("list_120", render(&mut fx.app, 120, 20));
}
#[test] fn s_in_the_list_is_gated_and_sends_when_ready() { /* ?2 row: s → message; answered row: s → send-pick */ }
#[test] fn g_focuses_the_sent_agent() { /* fake herdr log contains "agent focus intent-…" */ }
#[test] fn a_sent_capability_without_repo_still_renders() { /* repo: None; row shows the bar from file signals */ }
#[test] fn pollers_only_run_while_the_list_shows() { /* tree layout: 200 ticks, fake herdr log has no "agent list" */ }
```

- [ ] **Step 2: Run to verify they fail.**

- [ ] **Step 3: Implement.** `draw`: when `screen` is `Browse` and `layout == List`, render the list over the whole area above the status line; other screens draw as today. Header: `Capabilities · <count> · sorted by <order>` left, `v tree · S sort · s send · g agent` right. Row line 1: status (6 wide), then for sent rows the 10-cell bar, label, flag; title; group dim at the right when width ≥ 100; timestamp right-aligned (`%b %d %H:%M`, created under Captured, stamp otherwise). Row line 2: first non-empty line of `ui::description_text`, dim, or `(no description yet)`. Truncate by display width with `…`. `tick`: while `layout == List`, every 90 ticks `poller.request_agents()`, every 1800 ticks (and on `r`) `request_pr` for each distinct `pr:` on a sent capability with a repo; drain updates into `agents` and `evidence`. Recompute `plan_evidence` for a capability on `r`, on returning from another screen, and after an edit to it. `s` uses `send_refusal` then the existing SendPick flow, which returns to `Browse` and so to the list. `g` runs `herdr agent focus <sent_to.agent>` on a spawned thread; no agent → message `not sent yet`.

- [ ] **Step 4: Review the snapshots** (`cargo insta review` or read the `.snap.new` files): rows match the spec's sketch, nothing wraps at 80 columns, the control character shows as `·`.

- [ ] **Step 5: README.** A "List view" section: the toggle, the columns, the status marks, the progress stages table, the keys, and that progress refreshes on `r` and on timers while the list shows.

- [ ] **Step 6: Run the suite, clippy and a release build**

Run: `cargo test -p herdr-capabilities && cargo clippy -p herdr-capabilities --all-targets -- -D warnings && cargo build --release -p herdr-capabilities`
Expected: all pass, no warnings, binary built.

- [ ] **Step 7: Commit** `feat(capabilities): render the list with progress bars, send and go-to-agent`
