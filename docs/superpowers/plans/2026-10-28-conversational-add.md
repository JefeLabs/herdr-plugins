# Conversational Add Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `a` (and the `prefix+a` popup) opens a conversation that runs:
- pick a type (bug, refactor, feature or idea);
- your own words;
- a polished restatement;
- up to 5 questions, one at a time;
- an agent-written title and intent, with criteria grouped by area.

Nothing is saved until Enter at the write-up. Ctrl-s after an agent failure saves the words raw.

**Architecture:**
- The phase logic and prompt building live in a new pure module, `src/interview.rs`, unit-tested without the TUI.
- `app.rs` gains one screen, `Screen::Interview`, that runs turns through the existing `conversation::Turn`. It drives the steps (restate, ask, write up) with a phase-specific prompt each turn, parses each reply for that phase, and saves through `Store::create`.
- Refactor becomes a fourth kind.
- Intents remember the folder their agent ran in (`polish_dir`), so `p` resumes the interview's conversation.

**Tech Stack:** Rust 2024 workspace, ratatui/crossterm, serde_yaml front matter, insta snapshots, tempfile, the fake provider and fake events in `tools/intents/tests/fixtures/`.

**Spec:** `docs/superpowers/specs/2026-10-28-conversational-add-design.md`

## Global Constraints

- All paths below are under `tools/intents/` unless they start with `docs/` or `scripts/`.
- Work on branch `feature/conversational-add` (from `main`). Commit with the repo's configured author and end each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push, open a PR or merge without the owner's word.
- **Tests:**
  - per task, run `cargo test -q --release -p herdr-intents </dev/null`; stdin must be closed, or a test can hang;
  - at the end, `cargo test -q --release --workspace </dev/null`, `cargo fmt --all`, `cargo clippy -q --release --workspace --all-targets -- -D warnings` and `sh scripts/test-install.sh`, all clean.
- At most 5 questions, one question per agent turn, happy path only. `conversation::HAPPY_PATH_RULE` goes in every ask prompt.
- Plain letters are always typed in an input box. Actions use Enter, Enter on an empty box, Tab, Esc or Ctrl keys. The one exception is the retry state, which has no input box, so `m` works there.
- Nothing is written to the intents tree until save. That means Enter at the write-up, or Ctrl-s after a failure. Esc discards and deletes the interview's `.polish/new-*` folder.
- Kinds are stored as `issue`, `refactor`, `enhancement` and `idea`, and shown as bug, refactor, feature and idea (`config::kind_label`).
- When a default `*.polish.md` template changes, add the outgoing file's sha256 to `old_polish_defaults(kind)`, so untouched copies upgrade.
- The agent and model are always the ones `last_used` returns (chosen with `m`).
- Reply markers the app parses, copied exactly:
  - restate: `summary: <one line>`;
  - ask: `question: <the question>`, or the single word `enough`;
  - write-up: first line `<kind heading>: <title>`, second line `group: <path>`, then a fenced ```` ```proposal ```` block, then `questions:`.
  - Kind headings: `bug:`, `refactor:`, `feature:`, `idea:`. The older `capability:` still reads as a feature.

## Review Focus

These are the inputs the spec implies most likely to bite, with the test each gets:

1. The agent puts two `question:` lines in one reply, or a question and a write-up. Only the first question is used. Test: `parse_ask_takes_one_question_or_enough` (Task 3).
2. The agent's `group:` has a trailing slash, names a group outside the workspace, or names one that doesn't exist. A trailing slash is accepted; anything else falls back. Tests: `save_group_prefers_a_valid_reply_group` (Task 3) and `an_unknown_group_falls_back_to_where_a_adds_today` (Task 5).
3. A pasted report with a 300-character first line, saved raw. The title is cut at a word with `…` and is at most 81 characters. Test: `raw_title_is_the_first_line_cut_at_a_word` (Task 3).
4. Enter pressed while a turn is still running must not start a second turn. Test: `enter_while_the_agent_works_is_ignored` (Task 4).
5. A provider that cannot resume must get the whole conversation in every prompt. Test: `a_non_resumable_prompt_repeats_the_whole_conversation` (Task 3).

---

### Task 1: Refactor as a fourth kind

**Files:**
- Modify:
  - `src/config.rs`: `KINDS`, `kind_from_heading`, `DEFAULT_FILES`, `old_polish_defaults`, `builtin_handoffs`;
  - `src/conversation.rs`: `FORMAT_REMINDER`;
  - `src/send.rs`: `BUILDING_HANDOFFS`;
  - `src/app.rs`: `key_kind_pick`, `KIND_QUESTION`, `title_hint`;
  - `src/provider.rs`: the doc comment on `Reply.kind`;
  - `defaults/issue.polish.md`, `defaults/enhancement.polish.md`, `defaults/idea.polish.md`;
  - `README.md`.
- Create: `defaults/refactor.md`, `defaults/refactor.polish.md`.
- Test: unit tests in `src/config.rs`, `src/app.rs` and `tests/provider.rs`, `tests/send.rs`.

**Interfaces:**
- Produces:
  - `KINDS == ["issue", "refactor", "enhancement", "idea"]`, so `K` cycles bug → refactor → feature → idea;
  - `kind_from_heading("refactor") == Some("refactor")`;
  - a built-in hand-off named `refactor-spec`.

- [ ] **Step 1: Write the failing tests**
  - `config::tests::kinds_include_refactor_shown_as_refactor`: `KINDS` equals the list above. `kind_label("refactor") == "refactor"` and `kind_from_heading("Refactor") == Some("refactor")`.
  - `config::tests::refactor_templates_are_seeded`: after `load_or_init`, `templates/refactor.md` and `templates/refactor.polish.md` exist. The polish template contains `refactor: <what changes> in <where>`.
  - `config::tests::refactor_spec_handoff_applies_to_refactors`: `builtin_handoffs()` has `name == "refactor-spec"`, `kinds == ["refactor"]`, and a prompt containing `enhancement-spec` and `` `refactor` label``.
  - `tests/provider.rs::a_refactor_heading_sets_kind_and_title`: `parse_reply("refactor: Split app.rs into modules in tools/intents\n\n```proposal\n## Description\nx\n```\n")` gives kind `Some("refactor")` and title `Some("Split app.rs into modules in tools/intents")`.
  - `tests/conversation.rs`: `follow_up_prompts_always_carry_the_format_reminder` also checks for `"refactor:"`.
  - `tests/send.rs::refactor_spec_gets_the_work_start_preamble_in_workbrokr_repos`: same shape as the existing enhancement-spec preamble test.
  - `app::tests::k_cycles_bug_refactor_feature_idea`: on an `issue`, `K` gives `refactor`, `K` again gives `enhancement`.
  - `app::tests::r_in_the_kind_question_starts_a_refactor`: `a` then `r` shows a label containing `new refactor in`. Task 4 replaces this assertion.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: the new tests FAIL (refactor is not a kind, no templates, no hand-off).

- [ ] **Step 3: Implement**
  - Add `refactor` to `KINDS` in the order above.
  - `kind_from_heading`: `"refactor" => Some("refactor")`.
  - `refactor.md` uses the same four sections as the others, with a Description prompt: "Which code, what is wrong with it today, what must not change."
  - `refactor.polish.md`: copy `enhancement.polish.md`'s structure and rules; its intro speaks of a refactor (code that should change shape without changing behaviour).
  - Every polish template's and `FORMAT_REMINDER`'s first-line sentence adds ``, `refactor: <what changes> in <where>` for code that should change shape``. The kind question becomes "Is this a bug, a refactor, a feature, or an idea?".
  - Record the three outgoing polish templates' sha256s in `old_polish_defaults`; `"refactor"` gets `&[]`.
  - `refactor-spec` prompt: `"Use the enhancement-spec skill to write this up as a refactor GitHub issue with the `refactor` label. Show me the draft before posting.\n\n{intent}"`. Add it to `BUILDING_HANDOFFS`.
  - `key_kind_pick`: `KeyCode::Char('r') => "refactor"`. `KIND_QUESTION`: `"new intent: is it a bug, a refactor, a feature or an idea?   b bug · r refactor · f feature · i idea · Esc cancel"`. `title_hint("refactor") == "what changes, and where"`.
  - `README.md`: kinds read bug, refactor, feature, idea; the hand-off list includes `refactor-spec`.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: PASS, apart from snapshots whose status line changed. Accept those with `INSTA_UPDATE=always` only after reading each diff: the only change should be the kind question.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src tools/intents/tests tools/intents/defaults tools/intents/README.md
git commit -m "feat(intents): refactor as a fourth kind"
```

### Task 2: Intents remember their polish folder

**Files:**
- Modify: `src/store.rs` (`FrontMatter`, `Intent`, `Store::create`'s literal), `src/app.rs` (`start_turn`).
- Test: `src/store.rs` unit tests (or `tests/store.rs`), `src/app.rs` tests.

**Interfaces:**
- Produces:
  - `FrontMatter.polish_dir: Option<String>`, serialized only when `Some`;
  - `Intent::polish_name(&self) -> String`, which returns `polish_dir` when set, else `stem()`.
- Consumes: `config::polish_cwd(root, name)`, unchanged.

- [ ] **Step 1: Write the failing tests**
  - `polish_dir_round_trips_and_names_the_polish_folder`:
    - create an intent; its file has no `polish_dir` line and `polish_name() == stem()`;
    - `update_fields` sets `Some("new-ab12cd34")`, then reload;
    - `polish_name() == "new-ab12cd34"`, and the file contains `polish_dir: new-ab12cd34`.
  - `app::tests::p_runs_in_the_remembered_polish_folder`: set `polish_dir: new-x` on Alpha, press `p`, wait for the turn. `<root>/.polish/new-x` exists and `<root>/.polish/<Alpha's stem>` does not.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: FAIL (no field; `start_turn` uses the stem).

- [ ] **Step 3: Implement the field and `polish_name`.** `start_turn` calls `polish_cwd(self.store.root(), &intent.polish_name())`. Reconcile keeps its own path: it never resumes.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src
git commit -m "feat(intents): an intent remembers the folder its agent ran in"
```

### Task 3: The interview core

**Files:**
- Create:
  - `src/interview.rs`;
  - `defaults/issue.interview.md`, `defaults/refactor.interview.md`, `defaults/enhancement.interview.md`, `defaults/idea.interview.md`.
- Modify:
  - `src/lib.rs` (`pub mod interview;`);
  - `src/provider.rs` (`Reply.group`);
  - `src/config.rs` (`DEFAULT_FILES`, `pub fn interview_guide`).
- Test: unit tests in `src/interview.rs`; `tests/provider.rs`; `src/config.rs` tests.

**Interfaces:**
- Consumes: `conversation::{HAPPY_PATH_RULE, POLISH_RULE}`, `config::kind_label`.
- Produces (exact):

```rust
pub const MAX_QUESTIONS: usize = 5;
/// Appears only in write-up prompts; tests look for it in the provider log.
pub const WRITE_UP_MARKER: &str = "Write the intent up now.";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step { Restate, Ask, WriteUp }
#[derive(Debug, PartialEq, Eq)]
pub enum Asked { Question(String), Enough }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qa { pub question: String, pub answer: Option<String> } // None = skipped
pub struct Interview {
    pub kind: String,            // stored form: issue | refactor | enhancement | idea
    pub home: Option<String>,    // group path under the root, when known
    pub groups: Vec<String>,     // existing group paths in the workspace's tree
    pub guide: String,           // the kind's interview template
    pub words: String,
    pub corrections: Vec<String>,
    pub qas: Vec<Qa>,
}
impl Interview {
    pub fn new(kind: &str, home: Option<String>, groups: Vec<String>, guide: String) -> Self;
    pub fn budget_left(&self) -> usize;                       // MAX_QUESTIONS - qas.len(), floored at 0
    pub fn prompt(&self, step: Step, resumable: bool, message: &str) -> String;
    pub fn raw_title(&self) -> String;
    pub fn raw_body(&self) -> String;
    pub fn save_group(&self, reply_group: Option<&str>) -> Option<String>;
}
pub fn parse_ask(text: &str) -> Asked;
pub fn polish_folder_name() -> String;                        // "new-" + 8 lowercase hex chars
// provider.rs
pub struct Reply { /* existing fields */ pub group: Option<String> }
// config.rs
pub fn interview_guide(dirs: &Dirs, kind: &str) -> anyhow::Result<String>; // templates/<kind>.interview.md
```

How `prompt` works:
- `message` is the owner's latest input for this turn: the words, a correction, an answer, `"(skipped)"`, a refinement at the write-up, or `""`.
- **Not resumable, or a Restate with no corrections yet:**
  - the guide;
  - `Kind: <kind_label>`;
  - `Home group: <home or "unknown">`;
  - for Ask and WriteUp, `Existing groups:` with one path per line;
  - `The author's words:` and the words;
  - each correction;
  - each question with its answer (`(skipped)` when `None`);
  - the step rule.
- **Resumable:** the message, then the step rule; Ask and WriteUp also carry `Home group:` and the groups.

The step rules:
- **Restate:** no questions. Reply `summary: <one sentence>`, then one short polished paragraph keeping every fact, then `Is that right?`.
- **Ask:** "question N of at most 5" (N = `qas.len() + 1`). One question as `question: …`, or the single word `enough`. Then `HAPPY_PATH_RULE`. If the home group is unknown, or the agent can't tell which groups the work touches, one question may ask, offering likely groups from the list.
- **WriteUp:**
  - `WRITE_UP_MARKER`;
  - the first-line headings, as in Global Constraints;
  - the `group: <path>` line, which must be one of the existing groups; when home is known, keep it unless the author asked to move it;
  - the ```` ```proposal ```` block with `## Description`, `## Preconditions`, `## Acceptance criteria` and `## Open questions`;
  - home-group criteria first and ungrouped, then one `### <group path>` per other existing group the work touches;
  - answered questions kept with `→ answer`, skipped ones left open;
  - then `questions:`;
  - then `POLISH_RULE`.

- [ ] **Step 1: Write the failing tests** (in `src/interview.rs` unless noted)
  - `the_first_prompt_carries_the_guide_the_words_and_the_restate_rule`: `prompt(Restate, true, words)` contains the guide, the words and `summary:`. It contains neither `question:` nor `WRITE_UP_MARKER`.
  - `ask_prompts_count_down_and_carry_the_groups`: with 2 qas, `prompt(Ask, true, "a2")` contains `question 3 of at most 5`, `skoolscout/global/notifications`, `HAPPY_PATH_RULE` and `enough`.
  - `an_unknown_home_group_is_named_as_unknown`:
    - `home = None` gives `Home group: unknown`;
    - `Some("skoolscout/discover-schools/org/team")` gives that path.
  - `a_non_resumable_prompt_repeats_the_whole_conversation`: `prompt(Ask, false, "")` contains the words, a correction, `Q1?` and `a1`, and `Q2?` followed by `(skipped)`.
  - `the_write_up_prompt_asks_for_title_group_and_grouped_criteria`: contains `WRITE_UP_MARKER`, `group:`, ```` ```proposal ````, `### <group path>`, `refactor:` and `POLISH_RULE`.
  - `parse_ask_takes_one_question_or_enough`:
    - `"question: Which notification?\n"` gives `Question("Which notification?")`;
    - `"question: A?\nquestion: B?"` gives `Question("A?")`;
    - `"enough"` and `"Enough."` give `Enough`;
    - `"Which one is it?"` with no marker gives `Question("Which one is it?")`.
  - `budget_ends_at_five`: with 5 qas, `budget_left() == 0`; with 6 qas, still 0.
  - `raw_title_is_the_first_line_cut_at_a_word`:
    - words `"tenant admins getting notifications\nmore"` give `"tenant admins getting notifications"`;
    - a 300-character first line gives at most 81 characters, ending `…`, with no word cut in half.
  - `raw_body_keeps_words_and_answers`: contains `## Description`, the words verbatim, `## Open questions`, `- Which notification?\n  → both`, and `- Who?` with no arrow for a skipped question.
  - `save_group_prefers_a_valid_reply_group`: groups are `["skoolscout", "skoolscout/global/notifications"]` and home is `Some("skoolscout")`.
    - `Some("skoolscout/global/notifications/")` gives the path without the slash;
    - `Some("nope")` gives `Some("skoolscout")`;
    - `None` gives `Some("skoolscout")`;
    - with home `None` and an invalid group, `None`.
  - `polish_folder_names_are_new_and_unique`: starts with `new-`, length 12, two calls differ.
  - `tests/provider.rs::a_group_line_names_the_home_group`:
    - `parse_reply("bug: X when Y\ngroup: skoolscout/admin\n```proposal\n## Description\ngroup: kept\n```\n")` gives `group == Some("skoolscout/admin")`;
    - prose does not contain `group:`;
    - the proposal still contains `group: kept`.
  - `config::tests::interview_guides_are_seeded_for_every_kind`: for every kind in `KINDS`, `interview_guide` returns non-empty text, and the issue guide mentions `expected`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: compile errors for the missing module and items, then FAIL once stubs exist.

- [ ] **Step 3: Implement**
  - `src/interview.rs` as specified above.
  - In `parse_reply`, read `group:` only outside the proposal block, case-insensitively, trimming quotes, `*` and `_`.
  - Each guide names what that kind's questions cover:
    - bug: what happens, what was expected, where, who sees it;
    - refactor: which code, what hurts, what must not change;
    - feature: who, what they can do, where, what success looks like;
    - idea: what sparked it, who it is for, what would make it worth doing.
  - The guides are seeded once, like the body templates; they have no upgrade list yet.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src tools/intents/tests tools/intents/defaults
git commit -m "feat(intents): interview prompts, reply parsing and raw capture text"
```

### Task 4: The conversation on screen, up to the write-up

**Files:**
- Modify: `src/interview.rs` (add `InterviewScreen`, `Awaiting`), `src/app.rs` (screen, keys, tick, view, draw; remove `InputAction::AddIntent` and its handler), `src/ui.rs` (save rows).
- Test: `src/app.rs` tests; one insta snapshot.

**Interfaces:**
- Consumes: Task 3's `Interview`, `Step`, `Asked`, `parse_ask`, `polish_folder_name`, `interview_guide`; `conversation::Turn::start(spec, prompt, conv, cwd, env)`; `last_used`.
- Produces:

```rust
// interview.rs
pub enum Awaiting { Words, Turn, Confirm, Answer(String), Review, Retry(String) }
pub struct InterviewScreen {
    pub iv: Interview,
    pub awaiting: Awaiting,
    pub step: Step,                      // the running or last turn's step
    pub message: String,                 // the input that turn was sent with (a retry re-sends it)
    pub turn: Option<crate::conversation::Turn>,
    pub transcript: Vec<(String, String)>,
    pub input: String,
    pub provider: String,
    pub model: Option<String>,
    pub conv_id: Option<String>,
    pub folder: std::path::PathBuf,      // <root>/.polish/new-xxxxxxxx
    pub fallback: std::path::PathBuf,    // where `a` adds today (selected_group_dir or home_dir)
    pub write_up: Option<crate::provider::Reply>,
    pub save_kind: String,
    pub scroll: u16,
}
// app.rs
Screen::Interview(Box<InterviewScreen>)
Screen::KindPick { group: Option<PathBuf> }   // None = home group unknown
pub struct InterviewView { pub awaiting: &'static str, pub question_no: usize, pub home: String,
    pub save_kind: String, pub suggested_kind: Option<String>, pub save_group: String, pub areas: Vec<String> }
View.interview: Option<InterviewView>          // set when screen == "interview"
```

**Rulings:**
- **The home group is known** in the tree when a row is selected (a group, or an intent's parent). It is unknown in the card view, the popup and an empty tree.
- **`fallback`** is today's `selected_group_dir()` (popup: `home_dir()`).
- **Awaiting names in the view:** `words`, `turn`, `confirm`, `answer`, `review`, `retry`. `home` is the path, or `group to be decided`.
- **Reply handling in `tick`, by step:**
  - Restate: Confirm.
  - Ask: `parse_ask` on the agent's full transcript text. A question goes to `Answer(q)`; `Enough` starts the WriteUp turn at once.
  - WriteUp: a reply with a proposal and a title becomes `write_up`; awaiting goes to Review and `save_kind = iv.kind`. Otherwise it's `Retry("no write-up in this reply")`.
  - `Failed(e)` gives `Retry(e)`.
  - `ConversationId` sets `conv_id`.
- **Keys:**
  - Words: Enter on non-empty input sets `words` and starts Restate.
  - Confirm:
    - Enter on an empty box starts Ask;
    - text plus Enter pushes a correction and starts Restate;
    - Ctrl-d starts WriteUp.
  - Answer(q):
    - Enter pushes `Qa` (empty input = skipped, message `(skipped)`);
    - then Ask, or WriteUp when `budget_left() == 0`;
    - Ctrl-d pushes q as skipped and starts WriteUp.
  - Turn: Enter is ignored.
  - Retry: Enter re-sends `step` with `message`.
  - Esc in Words, Confirm, Answer, Review or Retry discards: delete `folder` and go to Browse.
  - Esc in Turn: Task 6.
- **Resumable** means `conv_id.is_some()` and the provider has a `resume` argv, as in `key_chat`.
- **Draw:** reuse the chat transcript and input rendering. The header is `<kind label> · in <home>`, plus `· question N of up to 5` while asking. At Review, `ui::render_save_rows` draws `save as:` with the four chips (the chosen one highlighted, `agent: this reads as a <label>` when the suggestion differs), `in: <group>` and `also: <areas>`.
- **Status lines, exact:**
  - words: `say it in your own words · Enter send · Esc cancel (nothing saved)`
  - turn: `waiting for the agent · Esc cancel`
  - confirm: `Enter that's right · type a correction · Ctrl-d write it up now · Esc cancel`
  - answer: `Enter answer · Enter on empty skips · Ctrl-d write it up now · Esc cancel`
  - review: `Enter save · Tab change type · type to keep refining · Esc discard`
  - retry: `<reason> · Enter retry · Esc discard` (Task 6 extends it)

- [ ] **Step 1: Write the test helper and the failing tests**
  - Helper `agent_says(fx: &mut Fx, text: &str)`:
    - writes `<tmp>/reply-<n>.jsonl` in the agy shape of `tests/fixtures/events/agy.jsonl`: an `init` line with conversation id `conv-int`, one `step_update` agent_response whose `text_delta` is `text`, and a `result` line;
    - builds the lines with `serde_json`;
    - points `FAKE_EVENTS` in `fx.app.provider_env` at that file.
  - Tests:
    - `a_in_the_tree_starts_an_interview_on_that_group`: select group `skoolscout`, then `a`, `b`. The screen is `interview`, awaiting `words`, home `skoolscout`, and no new `.md` file exists.
    - `the_agent_restates_first_and_asks_nothing`:
      - `agent_says("summary: Tenant admins are notified\n\nWhen … Is that right?")`;
      - type the words, Enter, wait;
      - awaiting is `confirm`;
      - the first line of the provider log contains the words and `summary:`, and does not contain `question N of`.
    - `a_correction_gets_a_new_restatement`: at confirm, type `only the inviter` and Enter. The second prompt contains it, and awaiting returns to `confirm`.
    - `questions_come_one_at_a_time_up_to_five`:
      - each ask reply is `question: Q?`;
      - for i in 1..=5, `question_no == i`, then answer `a{i}`;
      - the log then has exactly 7 calls (restate, 5 asks, write-up) and the last contains `WRITE_UP_MARKER`.
    - `enough_goes_straight_to_the_write_up`: the first ask reply is `enough`. With no keypress, the next call contains `WRITE_UP_MARKER`.
    - `enter_on_empty_skips_and_ctrl_d_writes_up_now`: an empty Enter at answer sends `(skipped)`. Ctrl-d at the next answer sends a prompt with `WRITE_UP_MARKER`.
    - `the_write_up_shows_title_type_group_and_areas`:
      - create the group `skoolscout/global`;
      - the write-up reply is `bug: Tenant admins are emailed when anyone accepts a team invite\ngroup: skoolscout\n```proposal\n## Description\nx\n\n## Acceptance criteria\n- [ ] only the inviter\n\n### skoolscout/global\n- [ ] no email to others\n```\nquestions: none\n`;
      - awaiting is `review`, `save_kind == "issue"`, `save_group == "skoolscout"`, `areas == ["skoolscout/global"]`.
    - `enter_while_the_agent_works_is_ignored`: with `FAKE_MODE=hang`, Enter during the turn. The log still has one call.
    - `a_failed_turn_keeps_the_words_and_enter_retries`:
      - `FAKE_EVENTS` points at an empty file;
      - awaiting is `retry` and the hint contains `no reply`;
      - after `agent_says(restate)` and Enter, awaiting is `confirm`;
      - the log has 2 calls carrying the same words.
    - `esc_discards_and_leaves_no_file_or_folder`: Esc at confirm. The screen is `browse`, no new `.md` file exists, and no `<root>/.polish/new-*` folder exists.
    - `in_the_card_view_the_home_group_is_unknown`: `v`, `a`, `i` gives home `group to be decided`. After restate and confirm, the ask prompt contains `Home group: unknown`.
    - `interview_review_snapshot`: 80×20 render at review (the spec's narrowest terminal).
    - `capture_mode_closes_when_the_interview_is_discarded` replaces `capture_mode_closes_once_back_from_the_chat`: `start_capture()`, `b`, Esc returns `Flow::Quit`.
  - Delete the old add tests for the title prompt this task removes. Task 5 covers again where intents land and the popup closing after a save:
    - `a_in_the_card_view_adds_to_the_selected_cards_group`
    - `typing_a_title_in_the_card_view_keeps_the_cards_on_screen`
    - `a_in_an_empty_card_view_adds_at_the_top`
    - `add_creates_raw_intent_in_selected_group`
    - `adding_from_an_empty_list_lands_in_the_workspace_group`

    `capture_mode_cancelled_at_the_kind_question_closes` stays as it is.
  - `r_in_the_kind_question_starts_a_refactor` (Task 1) now asserts screen `interview` with view kind `refactor`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: FAIL (no interview screen; `a` still asks for a title).

- [ ] **Step 3: Implement** `KindPick { group: Option<PathBuf> }`, then `key_kind_pick` → `open_interview(home, kind)`.
  - `open_interview` creates `polish_cwd(root, polish_folder_name())` and reads `last_used`.
  - It builds `Interview::new(kind, home, self.group_paths(), interview_guide(..))`.
  - `start_capture` passes `None`.

  The rest follows the rulings above. Remove `InputAction::AddIntent` and its handler, since `a` no longer reaches it. Keep `title_label` for `t`.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: PASS. For the new snapshot, read it before accepting.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src
git commit -m "feat(intents): add runs as a conversation up to the write-up"
```

### Task 5: Review and save

**Files:**
- Modify: `src/app.rs` (review keys, `save_interview`), `README.md`.
- Test: `src/app.rs` tests.

**Interfaces:**
- Consumes:
  - `InterviewScreen`;
  - `Interview::save_group`;
  - `reconcile::heading_areas(body, root)`;
  - `Store::create(group_rel, title, kind, body, repo)`;
  - `Store::update_fields`;
  - `Store::append_transcript`;
  - `send::capture_repo()`;
  - `FrontMatter.polish_dir`.
- Produces: `fn save_interview(&mut self, st: Box<InterviewScreen>, raw: bool)`. Task 6 calls it with `raw = true`.

**Rulings:**
- **Save target:** `iv.save_group(write_up.group)`, else `fallback`.
- **Areas:** `heading_areas(body, root)` minus the save group.
- **State:** `Refining`, or `Raw` when `raw`.
- **Front matter:** `polish_dir` is the folder's name, and `conversations[provider] = conv_id` when present.
- **Transcript:** every transcript entry is appended in order.
- **After saving,** the screen goes to Browse and the new intent is selected (tree and list, as the old `AddIntent` did). The folder is kept.
- **Review keys:**
  - Tab cycles `save_kind` through `KINDS`;
  - Enter on an empty box saves;
  - text plus Enter starts a WriteUp turn with that message;
  - Esc discards.

- [ ] **Step 1: Write the failing tests**
  - `enter_saves_the_write_up_as_a_refining_intent`: from Task 4's review state, press Enter. The file:
    - is in `skoolscout`;
    - has title `Tenant admins are emailed when anyone accepts a team invite`, kind `issue`, state `Refining`;
    - has `areas == ["skoolscout/global"]`, `polish_dir` starting `new-`, and `conversations["fake"] == "conv-int"`;
    - has a `.chat.md` containing the words.

    The screen is `browse`, and the selected intent is the new one.
  - `tab_changes_the_type_before_saving`: Tab, then `save_kind == "refactor"`; Enter saves kind `refactor`.
  - `typing_at_the_write_up_refines_it`: type `file it under skoolscout/global` and Enter. The prompt contains it, plus `WRITE_UP_MARKER`. The reply's `group: skoolscout/global` saves it there with no `areas`.
  - `an_unknown_group_falls_back_to_where_a_adds_today`: card view on Beta, write-up `group: nowhere`. It saves into `skoolscout`, Beta's group.
  - `p_after_saving_resumes_the_interview_conversation`: after saving, `p`. The last log line contains `--resume=conv-int`, and `<root>/.polish/<new stem>` does not exist.
  - `an_empty_list_falls_back_to_the_workspace_group`: scope is `tourneyseason`, the list is empty, and the write-up has no `group:` line. It saves into `<root>/tourneyseason` (the case the deleted `adding_from_an_empty_list_lands_in_the_workspace_group` covered).
  - `the_popup_closes_after_saving`: `start_capture()`, run to review; Enter returns `Flow::Quit`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: FAIL (Enter at review does nothing yet).

- [ ] **Step 3: Implement** `save_interview` and the review keys. In the README, replace the add description (`a`, the popup, the browse key list and the "Shape of an intent" lead-in) with the conversation: type first, your words, restatement, up to 5 questions, write-up with areas, save as.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src tools/intents/README.md
git commit -m "feat(intents): save the conversation's write-up as an intent"
```

### Task 6: When the agent fails

**Files:**
- Modify: `src/app.rs` (`Screen::AgentPick` gains `back: Option<Box<Screen>>`; retry keys; Esc during a turn), `README.md`.
- Test: `src/app.rs` tests.

**Interfaces:**
- Consumes:
  - `save_interview(st, true)`;
  - `Interview::raw_title()` and `raw_body()`;
  - `open_agent_pick`, which becomes `open_agent_pick(back: Option<Box<Screen>>)`.

**Rulings:**
- **The raw save** uses `raw_title()` for the title, `raw_body()` for the body, and `iv.kind` for the kind. It skips `write_up`.
- **The picker opened from a retry** returns to the interview on Esc or on a choice. A choice saves `last_used` and sets the interview's `provider`/`model`. `m` from Browse behaves as today.
- **Esc during a turn** cancels it, which gives `Retry("cancelled")`. A second Esc discards.
- **Retry status line:** `<reason> · Enter retry · m agent · Ctrl-s save raw · Esc discard`.

- [ ] **Step 1: Write the failing tests**
  - `ctrl_s_after_a_failure_saves_the_words_raw`: answer one question, then make the next turn fail, then Ctrl-s. The file:
    - has the first line of the words as its title, the chosen kind, state `Raw`;
    - has a body containing the words and `→ <answer>`.

    The screen is `browse`.
  - `esc_on_a_running_turn_cancels_into_retry_and_keeps_the_words`: with `FAKE_MODE=hang`, Esc gives awaiting `retry` with a hint containing `cancelled`. Esc again gives `browse`, with no file and no folder.
  - `m_after_a_failure_picks_another_agent_and_returns`:
    - after a failure, `m` gives screen `agent-pick`;
    - Enter (fake), `j`, Enter (model-b) gives screen `interview`, awaiting `retry`;
    - Enter retries, and the log's last line contains `--model model-b`.
  - The existing `m_chooses_provider_then_model_and_polish_uses_it_from_then_on` still passes, since the picker from Browse returns to Browse.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -q --release -p herdr-intents </dev/null`
Expected: FAIL.

- [ ] **Step 3: Implement** the rulings above. In the README, add a short "When the agent fails" paragraph covering retry, `m`, Ctrl-s raw save and Esc.

- [ ] **Step 4: Run the whole suite and the checks**

Run: `cargo test -q --release --workspace </dev/null && cargo fmt --all --check && cargo clippy -q --release --workspace --all-targets -- -D warnings && sh scripts/test-install.sh </dev/null`
Expected: every test passes, no fmt diff, no clippy warnings, and `all install tests passed`.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src tools/intents/README.md
git commit -m "feat(intents): retry, switch agent or save raw when the agent fails"
```
