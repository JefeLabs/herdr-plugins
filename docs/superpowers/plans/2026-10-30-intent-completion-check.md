# Completion Check Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a sent intent's work is checked against what shipped. Every PR must be merged, and each acceptance criterion is tried in the running app or its logs by the confined capture agent. The check ticks what it proves, files a linked "Missing item/Bug" for each gap, and re-checks the original once those are closed.

**Architecture:** a check is a new kind of run on evidence capture's detached runner (`herdr-intents capture-run`).
- **The run:** same `work` stack and seed, same confined `claude`. Its MCP server gains a per-criterion `screenshot` and a read-only `logs` tool. The runner parses one verdict line per criterion and applies the results to the intent file itself: ticks, front-matter records, missing items. So a check completes even with no tool open.
- **The TUI:** it adds the `X` key, the automatic trigger after a reconcile, the re-check loop, and the display: result marks in Criteria, proof in Evidence, and a new Missing items / Bugs section.

**Tech Stack:** Rust 2024 (crate `herdr-intents`), ratatui, serde/serde_json/serde_yaml_ng, nix, insta, tempfile, shell-script fakes in `tools/intents/tests/fixtures/`.

**Spec:** `docs/superpowers/specs/2026-10-30-intent-completion-check-design.md`

## Global Constraints

- **The agent gets no shell:** the check runs `claude --restricted --permission-mode dontAsk --tools ""` with only the `intents-capture` MCP server's tools. An older claude fails closed before any stack starts. All of this is unchanged from capture.
- **The `logs` tool:**
  - reads only files the guide lists under `logs:`, resolved under the check stack's worktree; an absolute path or one with `..` is refused;
  - returns only lines written after the agent started;
  - is read-only.
- **No tick without proof:** a `confirmed` without `checks/<n>.png` or `checks/<n>.log.txt` written in that run counts as `can't check: no proof kept`.
- **Ticks:**
  - a check never unticks an owner's tick, and unticks only ticks a check made;
  - once the owner unticks a check-made tick, no later check ticks that criterion again.
- **Wording:** criterion wording is never changed. Results attach to criteria by `store::criterion_key(text)`.
- **Complete** means: every criterion is ticked, every PR has merged, no missing item is open, and every criterion is acknowledged.
- **Automatic check:** it runs once per merged set (`checked_for`), only after that set's reconcile was applied, one at a time per repo.
- **Upgrade:** on the first start with this feature, every sent intent with a non-empty `reconciled_for` gets `checked_for = reconciled_for` (owner: no backlog).
- **Missing items:**
  - a `fails` verdict files a bug (`issue`), a `missing` verdict a feature (`enhancement`);
  - each is created as refining in the original's group, and never sent by the tool;
  - an open item gets later proof instead of a duplicate;
  - a criterion that fails again after its item completed gets a new item;
  - deleting an item (`D`) dismisses that gap for that criterion.
- **Re-check:** when an item closes (complete, or its criterion ticked by the owner), the original re-checks only that criterion, automatically.
- **Display:**
  - Criteria shows `✓` / `✗` / `?` with the finding and the date, and `o` opens the proof;
  - Evidence lists the proof files (Enter / `c` / `Y` as today);
  - Missing items / Bugs comes right after Pull requests;
  - Demo Videos is untouched.
- **Key:** `X`, in the tree, on a card and in Criteria. It is refused while that intent's check or capture runs.
- **Tests:** always run with `</dev/null`. Never commit `.herdr-toolbar.toml`. No push without the owner's word.

## Review Focus

1. **A reconcile rewords a criterion while its check runs.** The result for the old wording must attach to nothing, and must never tick a different criterion. Test: `results_for_a_reworded_criterion_tick_nothing` (Task 4).
2. **The owner edits the intent while a check finishes.** The runner applies its results to the file as it is on disk (reload, then write); the owner's edit survives. Test: `applying_keeps_an_edit_made_during_the_check` (Task 4).
3. **A log file is shorter than its recorded offset** (rotated or truncated during the run). The tool reads it from the start, without panicking or reading another file. Test: `a_truncated_log_is_read_from_the_start` (Task 2).
4. **The agent reports a criterion number out of range, twice, or not at all.** Out of range is ignored, the last line wins, and a missing answer is `can't check: no answer`. Test: `verdict_lines_out_of_range_repeated_or_missing` (Task 1).
5. **The original intent was deleted while its missing item stays open.** Closing the item triggers nothing and does not fail. Test: `an_item_whose_original_is_gone_closes_quietly` (Task 6).

---

### Task 1: The check's pure parts

**Files:**
- Create: `tools/intents/src/check.rs`
- Modify: `tools/intents/src/lib.rs` (`pub mod check;`)
- Modify: `tools/intents/src/capture.rs` (`CaptureGuide.logs`, `GuideFront.logs`)

**Interfaces:**
- Produces, in `check`:
  - `pub enum Verdict { Confirmed(String), Fails(String), Missing(String), CantCheck(String) }`, serde-tagged `{state, text}`, snake_case;
  - `pub struct CheckItem { pub number: usize, pub key: String, pub text: String }`;
  - `pub fn items(body: &str, only: Option<&[String]>) -> Vec<CheckItem>`: numbers from 1 across area groups, as demos count; `only` holds keys to keep;
  - `pub fn check_prompt(guide: &CaptureGuide, items: &[CheckItem], intent: &str, base_url: &str, after: Option<&CaptureRecord>) -> String`;
  - `pub fn parse_verdicts(items: &[CheckItem], text: &str) -> Vec<(usize, Verdict)>`;
  - `pub fn require_proof(verdicts: Vec<(usize, Verdict)>, check_dir: &Path, since: std::time::SystemTime) -> Vec<(usize, Verdict)>`;
  - `pub struct CriterionResult { pub number: usize, pub key: String, pub text: String, pub verdict: Verdict, pub proof: Option<String> }`;
  - `pub struct CheckRecord { pub at: DateTime<Local>, pub merged: Vec<u64>, pub results: Vec<CriterionResult>, pub warning: Option<String> }`;
  - `pub fn checks_dir(it: &Intent) -> PathBuf` (`<stem>.assets/checks`);
  - `pub fn read_check(dir) -> Option<CheckRecord>` and `pub fn write_check(dir, &CheckRecord)` (`latest.json`);
  - `pub fn merge_check(old: Option<CheckRecord>, new: CheckRecord) -> CheckRecord`: a partial re-check replaces only its criteria;
  - `pub fn log_paths(worktree: &Path, listed: &[String]) -> Result<Vec<PathBuf>, String>`.
- Produces, in `capture`: `CaptureGuide.logs: Vec<String>` (front matter `logs:`, default empty).

- [ ] **Step 1: Write the failing tests** in `check.rs`'s test module:
  - `items_number_across_area_groups`: a body with two ungrouped criteria, then `### org/team` with one, gives numbers 1, 2, 3; `only` with the second's key gives just number 2.
  - `the_prompt_lists_numbered_criteria_and_the_reply_format`: the prompt contains `1. <text>`, the four line forms `<n>: confirmed:` / `fails:` / `missing:` / `can't check:`, the `screenshot` and `logs` tool names, the guide body, and the reporter text fenced as data. With `after: Some(before)` it also contains `record_start` and `fixed:`.
  - `verdict_lines_out_of_range_repeated_or_missing` (Review Focus 4): `"9: confirmed: x"` is ignored for 3 items; two lines for 2 means the last wins; no line for 3 means `CantCheck("no answer")`.
  - `a_confirmed_without_proof_is_cant_check`: `Confirmed` for 1 with no `1.png` / `1.log.txt` newer than `since` becomes `CantCheck("no proof kept")`; with `1.png` written after `since` it stays.
  - `a_partial_recheck_replaces_only_its_criteria`.
  - `log_paths_stay_under_the_worktree`: `.logs/backend.log` resolves; `/etc/passwd`, `../x.log` and `a/../../x` are refused.
  - In `capture.rs` tests: `the_guide_lists_logs` (`logs: [.logs/backend.log]` parses; a missing `logs:` gives empty).
- [ ] **Step 2: Run** `cd tools/intents && cargo test -q --release -p herdr-intents --lib check:: </dev/null`. Expected: FAIL (the module's functions are stubs).
- [ ] **Step 3: Implement.** Verdict lines use the same trimming as `capture::parse_outcome` (`*`, backticks, `-`, curly apostrophe).
- [ ] **Step 4: Run** the same command, plus `capture::tests::the_guide_lists_logs`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): completion check — criteria, prompt, verdicts, proof rule, records`.

### Task 2: The check's tools

**Files:**
- Modify: `tools/intents/src/capture_mcp.rs`
- Test: `tools/intents/tests/capture_mcp.rs`

**Interfaces:**
- Consumes: `check::checks_dir` layout (`<n>.png`, `<n>.log.txt`).
- Produces:
  - `McpConfig` gains `#[serde(default)] pub mode: Mode` (`Capture` default | `Check`), `#[serde(default)] pub check_dir: Option<PathBuf>` and `#[serde(default)] pub logs: Vec<LogFile>`, where `pub struct LogFile { pub path: PathBuf, pub from: u64 }` holds the byte offset at agent start;
  - `pub fn tool_names(cfg: &McpConfig) -> Vec<&'static str>` replaces `tool_names(shot)`, and its caller in `capture.rs` (the allowed-tools list) is updated. Check mode gives `browser`, `screenshot` and, when `logs` is non-empty, `logs`; plus `record_start` / `record_stop` when a check also records a bug's after (`shot == Video && phase == After`).
- **Tool shapes:**
  - check-mode `screenshot` takes `{criterion: number}` and writes `<check_dir>/<n>.png`;
  - `logs` takes `{criterion: number, pattern: string}`. It does a case-insensitive substring search, line by line, from each file's `from` offset (from 0 when the file is now shorter), returning each match with the 3 lines after it. It returns at most 40 matches, with lines cut at 400 chars, and appends what it returned to `<check_dir>/<n>.log.txt`.

- [ ] **Step 1: Write the failing tests** in `tests/capture_mcp.rs` (server spawned as today):
  - `a_check_gets_per_criterion_screenshots_and_logs`: in check mode the tools are `["browser", "screenshot", "logs"]`. `screenshot {criterion: 2}` runs `[--session][S][screenshot][<check_dir>/2.png]`. `logs {criterion: 2, pattern: "To: kathia"}` returns the email block written after `from`, not one written before it, and writes `2.log.txt`.
  - `a_truncated_log_is_read_from_the_start` (Review Focus 3): the offset is larger than the file, and the match is still found.
  - `logs_only_reads_the_listed_files`: no tool argument names a path, so a `pattern` like `../../etc/passwd` just searches the listed files.
  - `a_check_with_no_logs_has_no_logs_tool`.
  - The existing capture tests still pass, since mode defaults to Capture.
- [ ] **Step 2: Run** `cd tools/intents && cargo test -q --release -p herdr-intents --test capture_mcp </dev/null`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): check tools — a screenshot and log lines per criterion`.

### Task 3: The runner runs a check

**Files:**
- Modify: `tools/intents/src/capture.rs`
- Modify: `tools/intents/tests/fixtures/fake-claude.sh` (`FAKE_CLAUDE_PROOF=<n,n…>` writes `<n>.png` into `$FAKE_CHECK_DIR`)
- Test: `tools/intents/tests/capture.rs`

**Interfaces:**
- Consumes: Task 1 (`items`, `check_prompt`, `parse_verdicts`, `require_proof`, `CheckRecord`, `write_check`, `merge_check`, `log_paths`) and Task 2 (`McpConfig.mode/check_dir/logs`).
- Produces:
  - `CaptureRequest` gains `#[serde(default)] pub job: Job`, where `pub enum Job { Capture, Check { keys: Option<Vec<String>>, check_dir: PathBuf, intent_path: PathBuf, root: PathBuf } }`, and `keys` holds the criteria to re-check (`None` means all);
  - `CaptureUpdate::Checked(CheckRecord)`, sent before `Done`.
- **How the runner does it:**
  - the stack part (start, env, seed, finish) is shared with capture, factored into one function;
  - just before claude starts, it records each listed log's byte length as `from`;
  - a check of a bug with a reproduced before also records the after into the evidence home, writing `after.json` as captures do;
  - `run_order` writes the merged `latest.json`, then calls Task 4's `check::apply(&Store::open(&root)?, &intent_path, &record)`. Until Task 4 lands, `apply` is a stub that returns an empty `Applied`.

- [ ] **Step 1: Write the failing tests** in `tests/capture.rs` (in-process `CaptureJob`, fakes as today):
  - `a_check_runs_on_the_same_stack_and_records_each_criterion`: the work log order is start, env, finish. The claude argv has `mcp__intents-capture__screenshot` and `mcp__intents-capture__logs`, and its MCP config file has `"mode":"check"` and the log offsets. `FAKE_CLAUDE_EVENTS` answers `1: confirmed: ok` / `2: fails: nope` with `FAKE_CLAUDE_PROOF=1`, so `latest.json` shows `Confirmed` for 1 and `Fails` for 2.
  - `a_bugs_check_also_records_the_after`.
  - `a_recheck_checks_only_its_criteria`: the prompt holds only that criterion, and `latest.json` keeps the others' earlier results.
- [ ] **Step 2: Run** `cd tools/intents && cargo test -q --release -p herdr-intents --test capture </dev/null`. Expected: the three new tests FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): the capture runner runs completion checks`.

### Task 4: Applying a check's results

**Files:**
- Modify: `tools/intents/src/check.rs` (`apply`)
- Modify: `tools/intents/src/store.rs` (`FrontMatter` fields)
- Test: `tools/intents/tests/check.rs` (new)

**Interfaces:**
- Produces, in `store::FrontMatter`:
  - `checked_for: Vec<u64>`;
  - `check_ticks: Vec<String>` (keys);
  - `owner_unticked: Vec<String>`;
  - `missing: Vec<MissingLink>`, where `pub struct MissingLink { pub stem: String, pub key: String, #[serde(default)] pub rechecked: bool }`;
  - `dismissed: Vec<String>`;
  - `missing_for: Option<MissingFor>`, where `pub struct MissingFor { pub stem: String, pub key: String }`.

  All are `#[serde(default, skip_serializing_if = …)]`.
- Produces, in `check`: `pub fn apply(store: &Store, intent_path: &Path, record: &CheckRecord) -> anyhow::Result<Applied>`, with `pub struct Applied { pub ticked: usize, pub unticked: usize, pub created: Vec<PathBuf>, pub updated: Vec<PathBuf> }`. It reloads the file, then writes, and sets `checked_for = record.merged` when that is non-empty.
- **What `apply` does with each verdict:**
  - `Confirmed` with proof: tick it, unless the key is in `owner_unticked`, and record the key in `check_ticks`;
  - `Fails` or `Missing` on a check-made tick: untick it;
  - `Fails` or `Missing` otherwise, unless the key is `dismissed`: append the proof to the open item linked for that key, or else create an item in the original's group (title = the criterion text cut to 100 chars; kind `issue` for `Fails`, `enhancement` for `Missing`; state refining; `missing_for` set; one criterion = the original's text; proof copied into its attachments);
  - `Fails` or `Missing` whose linked item is already complete: create a new item;
  - `CantCheck`: nothing.

- [ ] **Step 1: Write the failing tests** in `tests/check.rs` (a `Store` in a tempdir):
  - `a_confirmed_criterion_is_ticked_and_recorded`.
  - `an_owners_tick_is_never_unticked` (it stays `[x]` on `Fails`, and an item is still created).
  - `a_check_tick_is_unticked_when_it_later_fails`.
  - `an_owner_untick_is_never_ticked_again`.
  - `a_failing_criterion_files_a_linked_missing_item`: kind, state, group, `missing_for`, the single criterion, the original's `missing` link, and the proof in its `.assets/`.
  - `a_missing_criterion_files_a_feature`.
  - `an_open_item_gets_the_new_proof_not_a_twin`.
  - `a_completed_item_does_not_stop_a_new_one`.
  - `a_dismissed_gap_files_nothing`.
  - `results_for_a_reworded_criterion_tick_nothing` (Review Focus 1).
  - `applying_keeps_an_edit_made_during_the_check` (Review Focus 2).
  - `checked_for_records_the_merged_set`.
  - In `tests/capture.rs`: `a_launched_check_applies_its_results` (detached `launch`): after the runner ends, criterion 1 is ticked in the intent file and the failing one has its item.
- [ ] **Step 2: Run** `cd tools/intents && cargo test -q --release -p herdr-intents --test check </dev/null` and `--test capture a_launched_check_applies_its_results`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same two commands. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): a check ticks what it proved and files missing items`.

### Task 5: What complete means

**Files:**
- Modify: `tools/intents/src/progress.rs` (`Evidence`, `progress`)
- Modify: `tools/intents/src/app.rs` (`evidence_for`)
- Test: `tools/intents/src/progress.rs` tests, `tools/intents/src/app.rs` tests

**Interfaces:**
- Produces:
  - `Evidence` gains `pub open_missing: usize`, `pub unacknowledged: usize` and `pub checking: Option<(usize, usize)>`;
  - `evidence_for` fills them: `open_missing` from the `missing` links (an item is open while it exists, is not complete, and its criterion is not ticked), `unacknowledged` from `store::new_criteria`, cached per intent stamp, and `checking` from a running check's lock plus its proof files so far;
  - `Progress.complete` = all criteria ticked, all PRs merged (vacuous with none), `open_missing == 0`, `unacknowledged == 0`;
  - labels: `checking 2/5` while running; `✓ 5/5 · 2 new to read` when all are ticked but some are unread; `· 1 missing` appended while items are open.

- [ ] **Step 1: Write the failing tests:**
  - `complete_needs_merged_prs_no_open_items_and_everything_read`: four cases, each failing one condition.
  - `the_card_says_checking_while_a_check_runs`.
  - `the_card_counts_open_missing_items`.
  - Adjust the existing `✓ complete` tests only where they relied on an open PR counting as complete; name each in the commit message.
- [ ] **Step 2: Run** `cd tools/intents && cargo test -q --release -p herdr-intents --lib progress:: </dev/null`, then the named app tests. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole intents suite: `cd tools/intents && cargo test -q --release -p herdr-intents --no-fail-fast </dev/null`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): complete means ticked, merged, nothing missing and everything read`.

### Task 6: When checks run

**Files:**
- Modify: `tools/intents/src/app.rs`
- Test: `tools/intents/src/app.rs` tests

**Interfaces:**
- Consumes: `capture::launch` with `Job::Check`; Task 4's front-matter fields.
- Produces:
  - `fn check_now(&mut self)` (the `X` key);
  - `fn check_due(&mut self)`, called from `check_reconciles` after a reconcile applies, and on each poll for intents whose `reconciled_for != checked_for`;
  - `fn recheck_closed_items(&mut self)`;
  - `fn baseline_checks(&mut self)`, run once at start when `<config>/checks-since` is absent;
  - an owner-untick hook in `toggle_selected_checklist`;
  - in `trash`: cancel a running check (`capture::cancel_running(&check::checks_dir(&it))`), and dismiss the gap when the trashed intent is a missing item.
- **What changes:** `capture_after` no longer starts an after for a bug on merge (the check records it; `E` still does). Automatic checks wait while another check of the same repo runs.

- [ ] **Step 1: Write the failing tests:**
  - `x_checks_the_selected_sent_intent`, `x_is_refused_while_a_check_or_capture_runs`, `x_is_only_for_sent_intents`, `x_without_a_guide_says_why`.
  - `trash_stops_a_running_check`: `D` on an intent cancels a running check through its lock, as for captures; the runner still finishes its effort.
  - `a_check_runs_once_after_the_reconcile_of_each_merged_set`: the list fixture's Notify with PR 42 merged reconciles, then one check starts; a repeat poll starts none.
  - `automatic_checks_take_turns_per_repo`.
  - `the_first_start_marks_reconciled_sets_checked`: no check for an intent reconciled before; `checks-since` is written; a second start changes nothing.
  - `unticking_a_check_tick_records_the_owners_veto`.
  - `deleting_a_missing_item_dismisses_its_gap`.
  - `a_closed_item_rechecks_its_criterion_once`: closed either by completion or by the owner ticking it; only that key is in the request.
  - `an_item_whose_original_is_gone_closes_quietly` (Review Focus 5).
  - `a_merged_bug_records_its_after_in_the_check`: no separate after capture starts.
- [ ] **Step 2: Run** the named tests: `cd tools/intents && cargo test -q --release -p herdr-intents --lib -- x_ trash_stops_a_running_check a_check_runs automatic_checks the_first_start unticking deleting_a_missing a_closed_item an_item_whose a_merged_bug </dev/null`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole intents suite: `cd tools/intents && cargo test -q --release -p herdr-intents --no-fail-fast </dev/null`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): X and automatic completion checks, re-checks and dismissals`.

### Task 7: Showing results

**Files:**
- Modify: `tools/intents/src/ui.rs` (`render_checklist` marks, `Pane::Missing`, `DetailView.missing`)
- Modify: `tools/intents/src/app.rs` (Tab order, `o` in Criteria, Evidence rows, Missing section keys, help lines)
- Test: `tools/intents/src/app.rs` and `ui.rs` tests, insta snapshots

**Interfaces:**
- Produces:
  - `render_checklist` takes `results: &[(usize, char)]` (`✓`, `✗`, `?`), drawn after the checkbox and before `▶`;
  - the selected criterion's finding and date are shown dim on the line under it;
  - `Pane::Missing` goes between `PullRequests` and `Artifacts`, with rows `<state mark> <title> (criterion <n>)`;
  - Enter on a row selects that intent in the tree;
  - Evidence rows `check · <n> · <mark> <finding>` carry the proof file;
  - `o` in Criteria opens the selected criterion's proof (the Chrome opener).

- [ ] **Step 1: Write the failing tests:**
  - `criteria_show_check_results`;
  - `o_opens_a_criterions_proof` (fake opener as in the Evidence tests);
  - `evidence_lists_check_proof`;
  - `the_missing_section_follows_pull_requests` (update the Tab-order and layout tests; the accordion-row test names the new row);
  - `enter_on_a_missing_item_selects_it`;
  - `completion_check_snapshot` at 80×24.
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole intents suite (`cd tools/intents && cargo test -q --release -p herdr-intents --no-fail-fast </dev/null`); review and accept the new snapshot. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): check results in Criteria and Evidence, and a Missing items section`.

### Task 8: The guide, the README and the whole suite

**Files:**
- **skoolscout-com** (in its worktree `.claude/worktrees/_tasks/platform/tooling--capture-guide`, branch `chore/platform/tooling--capture-guide`): modify `.helmsmith/capture.md`:
  - add `logs: [.logs/backend.log]`;
  - make the opening neutral ("records evidence for a bug or checks an intent's acceptance criteria");
  - make the not-seeded line neutral ("say it needs <name>, who is not seeded on this stack");
  - add one line that local emails are logged in full (From, To, Subject, Body) in that log.
- herdr-plugins: modify `tools/intents/README.md`, adding a "Completion check" section after "Evidence for bugs" (the key, the automatic run, the `logs:` field, verdicts, ticks and vetoes, missing items, re-checks, what complete means, where results show) and adding `X` to the browse keys.

- [ ] **Step 1:** Write both. Parse the guide's front matter, as Task 6 of the capture plan did (`logs` must read as a list).
- [ ] **Step 2:** Run the full checks: `cargo test -q --release --workspace </dev/null && cargo fmt --all --check && cargo clippy -q --release --workspace --all-targets -- -D warnings && sh scripts/test-install.sh </dev/null`. Expected: all clean.
- [ ] **Step 3: Commit:**
  - skoolscout-com: `chore(tooling): the capture guide serves completion checks and lists the backend log`;
  - herdr-plugins: `docs(intents): completion check`.
