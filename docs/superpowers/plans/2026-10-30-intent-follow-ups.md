# Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a refinement after an intent is done becomes a linked follow-up intent. When the follow-up ships, it is folded into its original's criteria, and the original re-proves the changed ones.

**Architecture:**
- **Links:** stems in the front matter, as missing items use; the original's `follow_ups` and the follow-up's `follow_up_of`.
- **Creating one:** `F` opens the existing add conversation, with the original passed as context.
- **The fold:** a reconcile with its own prompt. It goes through the poller, `reconcile_jobs` and `reconcile::apply`, so it gets backup, `u` and `*` for free. It records `last_fold`, and then starts a completion check with `fold: true` on the criteria it changed. In such a check a failure becomes a `can't check` note, and nothing is filed.
- **Display:** a Follow-ups section after Missing items / Bugs, and a `· N follow-up(s)` note on the card.

**Tech Stack:** Rust 2024 (crate `herdr-intents`), ratatui, serde, insta, tempfile, shell-script fakes.

**Spec:** `docs/superpowers/specs/2026-10-30-intent-follow-ups-design.md`

## Global Constraints

- **`F`:** on a sent intent only (unsent is refused). On a follow-up it links to the first original. The follow-up is filed in the original's group, as refining, and is never sent by the tool.
- **The card:** `· N follow-up(s)` while any is open (one follow-up, two follow-ups). A follow-up never takes away the original's `✓`.
- **When a fold runs:**
  - the follow-up reads `✓ complete`;
  - it isn't folded or declined;
  - the original has no open pull requests and its own reconcile has been applied for its merged set;
  - no reconcile or fold of that original is running.

  One fold per original at a time, in order of completion, and each follow-up folds once.
- **What a fold changes:**
  - it touches only what the follow-up changed;
  - it copies criteria that still hold word for word with their checkboxes; new or changed ones are unticked;
  - it keeps Description, Preconditions and Artifacts;
  - the original's `reconciled_for` is unchanged;
  - the follow-up's ticks never move.
- **After a fold:** a completion check of the changed or new criteria, with `fold: true`. A `fails` or `missing` becomes `can't check: the fold may describe this wrongly: …`, with no missing item and no untick.
- **`u` after a fold:** it restores the previous version and records `fold_declined` on that follow-up.
- **Pull requests:** those linked to or found for a follow-up stay out of the original's set, unless the original has a `pr:` line for it.
- **Front matter:** new fields are skipped when empty. Links use stems.
- **Repo rules:** tests run with `</dev/null`; no push or release without the owner's word.

## Review Focus

1. **A follow-up is deleted before it completes.** Its link is dropped from the original, and nothing folds. Test: `deleting_a_follow_up_drops_its_link` (Task 3).
2. **The original's own pull requests merge again after a fold.** Its regular reconcile must keep the folded criteria, so the reconcile prompt names them as shipped by follow-ups. Test: `a_later_reconcile_keeps_folded_criteria` (Task 4).
3. **The fold agent fails or returns no proposal.** Nothing changes, the link isn't marked folded, the reason shows, and `r` retries it. Test: `a_failed_fold_stays_visible_and_r_retries` (Task 4).
4. **`u` after a fold, then later polls.** The follow-up never folds again on its own. Test: `u_after_a_fold_declines_that_follow_up` (Task 5).
5. **The original's spec name turns up in a follow-up's pull request.** That PR is left out of the original's set: the original stays `✓` and starts no reconcile. Test: `a_follow_ups_pr_stays_out_of_the_original` (Task 3).

---

### Task 1: Links and the fold's pure parts

**Files:**
- Modify: `tools/intents/src/store.rs` (FrontMatter fields)
- Modify: `tools/intents/src/reconcile.rs` (`fold_prompt`, `changed_criteria`, the reconcile prompt keeps folded criteria)
- Test: `tools/intents/tests/reconcile.rs`

**Interfaces:**
- Produces, in `store::FrontMatter`:
  - `follow_up_of: Option<String>`, the first original's stem;
  - `follow_ups: Vec<FollowUpLink>`, with `pub struct FollowUpLink { pub stem: String, #[serde(default)] pub folded: bool }`;
  - `fold_declined: bool` (on a follow-up);
  - `last_fold: Option<String>` (on an original: the stem the last fold came from).
- Produces, in `reconcile`:
  - `pub fn fold_prompt(original: &Intent, follow_up: &Intent, prs: &[ShippedPr]) -> String`;
  - `pub fn changed_criteria(before: &str, after: &str) -> Vec<String>`, the `criterion_key`s in `after` that are not in `before`;
  - `pub fn reconcile_prompt(it: &Intent, ctx: &Context, folded: &[Intent]) -> String`, which gains a `folded` parameter. Every caller passes it; the poller passes the original's folded follow-ups.

- [ ] **Step 1: Write the failing tests** in `tests/reconcile.rs`:
  - `the_fold_prompt_carries_the_follow_up_as_data_and_its_rules`. It contains:
    - the original's body;
    - the follow-up's criteria;
    - the PR titles;
    - "only what the follow-up changed";
    - "word for word";
    - "new and changed criteria start unticked".

    Each piece of material is fenced as data, never instructions.
  - `changed_criteria_lists_only_new_wording`: a reworded criterion counts, a kept one doesn't, and a ticked-only change doesn't.
  - `a_later_reconcile_keeps_folded_criteria`: given one folded follow-up, the prompt names its criteria and says to keep them.
- [ ] **Step 2: Run** `cd tools/intents && cargo test -q --release -p herdr-intents --test reconcile </dev/null`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): follow-up links and the fold's prompt`.

### Task 2: Creating a follow-up (`F`)

**Files:**
- Modify: `tools/intents/src/app.rs` (`Screen::KindPick`, the `F` key, `save_interview`)
- Modify: `tools/intents/src/interview.rs` (`Interview.context`, `InterviewScreen.follow_up_of`)
- Test: `tools/intents/src/app.rs` and `interview.rs` tests

**Interfaces:**
- Consumes: Task 1's fields.
- Produces:
  - `Screen::KindPick { group, follow_up: Option<PathBuf> }`;
  - `Interview.context: Option<String>`, the original's title, description and criteria, given to the first prompt as data;
  - `InterviewScreen.follow_up_of: Option<PathBuf>`;
  - `save_interview` sets the new intent's `follow_up_of` to the original's stem and pushes `FollowUpLink { stem, folded: false }` onto the original.
- **`F`'s behaviour:** it works in the tree, on a card and in the detail pane. On a follow-up it links to its `follow_up_of` original. The follow-up's home is the original's group.

- [ ] **Step 1: Write the failing tests:**
  - in `interview.rs`: `the_first_prompt_carries_the_original_as_data`;
  - in `app.rs`:
    - `f_opens_the_add_view_linked_to_the_original`: run `F` through to a save, then check both links and the group;
    - `f_on_a_follow_up_links_to_the_first_original`;
    - `f_is_refused_on_an_unsent_intent`: the message says to edit it instead.
- [ ] **Step 2: Run** them: `cd tools/intents && cargo test -q --release -p herdr-intents --lib -- the_first_prompt_carries f_ </dev/null`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole intents suite: `cd tools/intents && cargo test -q --release -p herdr-intents --no-fail-fast </dev/null`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): F files a follow-up linked to its original`.

### Task 3: Follow-ups on the original

**Files:**
- Modify: `tools/intents/src/ui.rs` (`Pane::FollowUps`, `DetailView.follow_ups`)
- Modify: `tools/intents/src/app.rs` (rows, Tab order, keys, `evidence_for` PR exclusion, trash drops the link)
- Modify: `tools/intents/src/progress.rs` (`Evidence.open_follow_ups`, the label note)
- Test: `tools/intents/tests/progress.rs`, app tests, insta snapshot

**Interfaces:**
- Produces:
  - `Evidence.open_follow_ups: usize`;
  - the label appends `· 1 follow-up` / `· N follow-ups` to whatever it reads;
  - `Pane::FollowUps` sits between `Missing` and `Artifacts`, with rows `<○|→|✓> <title>`;
  - Enter selects the follow-up;
  - `fn follow_up_rows(&self, it) -> Vec<(String, String, Option<PathBuf>)>`.
- **Pull requests:** `evidence_for` leaves out of the original's found PRs those linked to or found for any of its follow-ups.
- **Trash:** deleting a follow-up removes its link from the original.

- [ ] **Step 1: Write the failing tests:**
  - in `tests/progress.rs`: `the_card_counts_open_follow_ups` (`✓ complete · 1 follow-up`, `✓ complete · 2 follow-ups`, and `complete` stays true);
  - in app tests:
    - `the_follow_ups_section_follows_missing_items` (update the Tab-order and layout tests too);
    - `enter_on_a_follow_up_selects_it`;
    - `deleting_a_follow_up_drops_its_link` (Review Focus 1);
    - `a_follow_ups_pr_stays_out_of_the_original` (Review Focus 5);
  - `follow_ups_snapshot` at 80×24.
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole intents suite; review and accept the snapshots (new and moved rows). Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): a Follow-ups section and the card's follow-up note`.

### Task 4: The fold

**Files:**
- Modify: `tools/intents/src/progress.rs` (`Poller::request_fold`, `Update::Fold`)
- Modify: `tools/intents/src/app.rs` (`fold_due`; `reconcile_jobs` carries the fold; `poll_reconciles` applies it)
- Test: app tests (list fixture with the fake provider)

**Interfaces:**
- Consumes: Task 1's `fold_prompt` and `reconcile_prompt(…, folded)`; Task 3's rows.
- Produces:
  - `Poller::request_fold(original: Intent, follow_up: Intent)`, which gathers the follow-up's merged PRs (titles and bodies, as `gather` does);
  - `Update::Fold { path, follow_up: String, prompt }`;
  - `reconcile_jobs` values gain `fold: Option<String>`.
- **On a successful fold:**
  - `reconcile::apply(path, &body, &original.fm.reconciled_for, root)` is called with the set unchanged;
  - `last_fold = Some(stem)` and `link.folded = true`. Task 5 adds the check that follows.
- **On failure:** it is kept in `reconcile_failed`, so `r` retries it.
- **`fn fold_due(&mut self)`** runs from `check_reconciles` and `rescan`, under the Global Constraints' conditions.

- [ ] **Step 1: Write the failing tests:**
  - `a_complete_follow_up_folds_into_the_original`: the original's criteria come from the fake proposal, and it has a backup, `*`, `last_fold`, the link folded, and `reconciled_for` unchanged;
  - `a_fold_waits_for_the_original_to_finish` (an open PR on the original: no fold);
  - `a_follow_up_folds_once`;
  - `folds_take_turns_per_original`;
  - `a_failed_fold_stays_visible_and_r_retries` (Review Focus 3).
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole intents suite. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): a completed follow-up folds into its original's criteria`.

### Task 5: After a fold — the check and `u`

**Files:**
- Modify: `tools/intents/src/capture.rs` (`Job::Check` gains `#[serde(default)] fold: bool`; `run()` turns a fold check's `fails`/`missing` into `can't check: the fold may describe this wrongly: …`)
- Modify: `tools/intents/src/app.rs` (start the fold's check with `keys = changed_criteria`; `undo_reconcile` declines the folded follow-up)
- Test: `tools/intents/tests/capture.rs`, app tests

**Interfaces:**
- Consumes: Task 1's `changed_criteria` and Task 4's apply path.
- Produces:
  - `start_check(…)` gains a `fold: bool` argument;
  - the fold's apply path in `poll_reconciles` starts the check with `keys = changed_criteria(before, after)`;
  - `u` on an original with `last_fold` sets `fold_declined` on that follow-up and says so.

- [ ] **Step 1: Write the failing tests:**
  - in `tests/capture.rs`: `a_failure_after_a_fold_files_nothing` (`2: fails: …` becomes `CantCheck("the fold may describe this wrongly: …")`, and after `apply` no item exists);
  - in app tests:
    - `the_fold_starts_a_check_of_the_changed_criteria` (keys are only the changed ones, and the order says `fold: true`);
    - `u_after_a_fold_declines_that_follow_up` (Review Focus 4: after `u` and several polls, no second fold).
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole intents suite. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): a fold re-proves its criteria, and u declines it`.

### Task 6: README and the whole suite

**Files:** `tools/intents/README.md`, with a "Follow-ups" section after "Completion check" and `F` in the browse keys.

- [ ] **Step 1:** Write the section. It covers:
  - `F`, including on a follow-up;
  - the section and the card note;
  - when a fold runs;
  - what it changes;
  - the check after it, and its "may describe this wrongly" note;
  - `u`;
  - pull requests staying with their intent.
- [ ] **Step 2:** Run `cargo test -q --release --workspace </dev/null && cargo fmt --all --check && cargo clippy -q --release --workspace --all-targets -- -D warnings && sh scripts/test-install.sh </dev/null`. Expected: all clean.
- [ ] **Step 3: Commit** `docs(intents): follow-ups`.
