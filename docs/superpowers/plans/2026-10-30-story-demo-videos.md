# Story Demo Videos Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- Polishing a feature or bug drafts user stories in the intent.
- Once its completion check passes, the confined agent records one captioned video per story into the intent's assets.
- The developer plays, copies or reveals each video from the intent's Stories section.

**Architecture:**
- **Stories are a `## Stories` section of the intent.** `store.rs` parses it, and reconcile renumbers its `shows:` lines.
- **The run guide's `## Stories` section** names the personas stories may use, and the drafting prompts read it.
- **A take is a new tool mode in `capture_mcp.rs`.** Its tools are `take_start`, `caption` and `take_stop`, and the server itself draws the cards and captions.
- **The runner records stories** in two ways:
  - right after a passing check, inside the same app run;
  - as its own `Job::Story`, when re-recorded by hand.

**Tech Stack:** Rust 2024, ratatui, serde/serde_json, insta, tempfile, and the shell fakes in `tools/intents/tests/fixtures/`.

**Spec:** `docs/superpowers/specs/2026-10-30-story-demo-videos-design.md`. It builds on `docs/superpowers/specs/2026-10-30-spaces-and-run-guide-design.md`, and comes after `docs/superpowers/plans/2026-10-30-intent-storage.md`.

## Global Constraints

- **Kinds:** stories for `enhancement` (feature) and `issue` (bug) only; none for `refactor` or `idea`.
- **The intent's section:**
  - `## Stories` comes after `## Acceptance criteria`, with one `### <name>` per story;
  - each story has `- persona: <name>`, `- shows: <numbers>` and `- starting data: <text>`, then numbered steps;
  - `shows:` counts criteria from 1, in order across area groups, as demo lines and the check count them.
- **Drafting:** stories are drafted only when every open question has an answer. Until then each story has only its name, `persona:` and `shows:`, plus the line `drafts wait for the open questions`.
- **The guide's personas:** a `## Stories` section in the run guide's instructions, with `### Personas` lines `- <Name>: <who, and how they sign in>`.
  - With no such section, personas are `unknown`, and recording stops with `the run guide lists no personas for stories`.
  - The guide's rules win over a story.
- **When the check passes:** every current criterion has a result in `latest.json`, and none is `Fails` or `Missing`. `CantCheck` doesn't block.
- **Recording:**
  - after a passing check, each story with no kept video records;
  - each story that shows a criterion this check run looked at records again;
  - by hand, a story records only after the check has passed.
- **Captions** are the intent's own criterion text. The agent passes only a number, and the agent's own `eval` stays refused.
- **The take starts at step 1,** after sign-in and setup. The agent sets up, rehearses all the way through, sets up fresh data again, then takes.
- **A video is kept only when every criterion the story shows got its caption.** Otherwise the story reads `criterion <n> didn't show`.
- **Files:** `<stem>.assets/stories/<n>-<slug>/`, holding `video.mp4`, `steps.txt`, `captions.json`, `story.json` and `run.log`. Steps found by the rehearsal and take stay there and never enter the intent.
- **The confined agent** stays as `--restricted --permission-mode dontAsk --tools ""`, with only the `intents-capture` server's tools.
- **Tests** start no real app, browser or agent, and run with `</dev/null`. No push, merge or release without the owner's word.

## Review Focus

1. A criterion whose text holds quotes, a backtick, `</div>` or `<script>`. Expected: the caption shows that text literally and the overlay script is unchanged. Test: `a_caption_with_markup_is_shown_as_text` (Task 4).
2. A `shows:` number that names no criterion, because a criterion was deleted by hand. Expected: it's dropped from the take and the keep rule, and `story.json` notes it. Test: `shows_beyond_the_criteria_are_dropped` (Task 1).
3. Two stories with the same name. Expected: each gets its own folder (`1-…`, `2-…`) and its own video. Test: `two_stories_with_one_name_get_their_own_folders` (Task 1).
4. A cancel (`E`, trash, quitting the runner) during a take. Expected: no video is kept, and the story doesn't read as recorded. Test: `a_cancelled_take_keeps_no_video` (Task 5).
5. Recording by hand before the check has passed. Expected: refused with `the completion check hasn't passed yet`. Test: `recording_by_hand_waits_for_a_passing_check` (Task 6).

---

### Task 1: Stories in the intent

**Files:**
- Modify: `tools/intents/src/store.rs` (`SECTION_ORDER` near line 934, plus the new items below), `tools/intents/src/reconcile.rs:301-307` (`apply`, plus the new `remap_shows` beside `remap_demos` at line 497)
- Create: `tools/intents/tests/stories.rs`

**Interfaces:**
- Produces, in `store.rs`:
  - `pub const STORIES_HEADING: &str = "## Stories";`
  - `pub const STORIES_WAIT: &str = "drafts wait for the open questions";`
  - `pub struct IntentStory { pub number: usize, pub name: String, pub persona: Option<String>, pub shows: Vec<usize>, pub starting_data: Option<String>, pub steps: Vec<String>, pub waits: bool }`
  - `pub fn stories(body: &str) -> Vec<IntentStory>`, with `number` counted from 1 in order;
  - `pub fn story_folder(s: &IntentStory) -> String`, giving `<n>-<slug>` where the slug is the name's lowercase words joined by `-`, cut to 40 characters;
  - `pub fn shown_criteria(body: &str, s: &IntentStory) -> (Vec<(usize, String)>, Vec<usize>)`, giving the shown criteria that exist, with their text, and the numbers that name none;
  - `pub fn unshown_criteria(body: &str) -> Vec<usize>`.
- Produces, in `reconcile.rs`: `pub fn remap_shows(before: &str, after: &str) -> String`.

- [ ] **Step 1: Write the failing tests** in `tests/stories.rs`, using the spec's example story as the body:

```rust
#[test] fn a_story_reads_its_four_parts() {
    let s = &stories(BODY)[0];
    assert_eq!((s.number, s.name.as_str()), (1, "Mark reinstates Maya's withdrawn application"));
    assert_eq!(s.persona.as_deref(), Some("Mark, a tenant admin"));
    assert_eq!(s.shows, vec![1, 3]);
    assert_eq!(s.starting_data.as_deref(), Some("Maya has a submitted application, then withdrawn"));
    assert_eq!(s.steps.len(), 3);
    assert!(!s.waits);
}
#[test] fn a_story_waiting_for_answers_has_no_steps() { /* "### X\n- persona: Mark\n- shows: 2\ndrafts wait for the open questions\n" */ assert!(stories(b)[0].waits && stories(b)[0].steps.is_empty()); }
#[test] fn shows_beyond_the_criteria_are_dropped() { /* 3 criteria, shows: 1, 7 */ assert_eq!(shown_criteria(b, &s), (vec![(1, "c one".into())], vec![7])); }
#[test] fn two_stories_with_one_name_get_their_own_folders() { assert_eq!(story_folder(&a), "1-mark-reinstates-maya-s-withdrawn-application"); assert_ne!(story_folder(&a), story_folder(&b)); }
#[test] fn criteria_no_story_shows_are_listed() { /* 4 criteria, stories show 1,3 */ assert_eq!(unshown_criteria(b), vec![2, 4]); }
#[test] fn a_reconcile_that_renumbers_criteria_renumbers_shows() {
    // before: criteria [a, b, c], shows: 1, 3   after: criteria [new, a, b, c] with the same Stories text
    assert!(remap_shows(before, after).contains("- shows: 2, 4"));
}
#[test] fn a_shows_line_the_agent_changed_stands() { /* after's shows line differs from before's */ assert!(remap_shows(before, after).contains("- shows: 2")); }
#[test] fn adding_a_criterion_keeps_stories_after_the_criteria() { /* add_checklist_item on a body with no Acceptance criteria but a Stories section */ assert!(out.find("## Acceptance criteria") < out.find("## Stories")); }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --test stories </dev/null`
Expected: compile failure, because `stories` and `remap_shows` don't exist.

- [ ] **Step 3: Implement.**
  - **`stories`:** read the lines under `## Stories`.
    - `### ` starts a story.
    - `- persona:`, `- shows:` and `- starting data:` are its fields; `shows` takes the numbers as `demo_target` parses them.
    - Lines that start with `<n>.` are steps.
    - A line equal to `STORIES_WAIT` sets `waits`.
  - **`SECTION_ORDER`:** gains `## Stories` after `## Acceptance criteria`.
  - **`remap_shows`:** follows `remap_demos`'s rule. A `- shows:` line unchanged from `before` is renumbered by criterion wording, and a criterion that's gone drops out.
  - **`reconcile::apply`:** calls `remap_shows(&it.body, &kept)` right after `remap_demos`. That covers both reconcile and the fold, which writes through `apply`.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p herdr-intents --test stories --test reconcile </dev/null`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src/store.rs tools/intents/src/reconcile.rs tools/intents/tests/stories.rs
git commit -m "feat(intents): an intent's Stories section"
```

### Task 2: The run guide's Stories section

**Files:**
- Modify: `tools/intents/src/capture.rs` (beside `read_guide`, near line 130)
- Test: `tools/intents/tests/capture.rs`

**Interfaces:**
- Consumes: `CaptureGuide.body` (the guide's instructions).
- Produces:
  - `pub struct GuideStories { pub personas: Vec<String>, pub text: String }`;
  - `pub fn guide_stories(guide: &CaptureGuide) -> Option<GuideStories>`. It returns `None` when the body has no `## Stories` section or no persona lines. `text` is the whole section, heading included.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn a_guide_names_its_story_personas() {
    // body: "Sign in at /auth.\n\n## Stories\n### Personas\n- Mark: tenant admin, signs in as mark@example.test\n- Pat: a parent, signs in as pat@example.test\n### Setup may create\n- applications and notes, never people\n"
    let s = guide_stories(&guide).unwrap();
    assert_eq!(s.personas, vec!["Mark", "Pat"]);
    assert!(s.text.starts_with("## Stories") && s.text.contains("never people"));
}
#[test] fn a_guide_without_a_stories_section_has_no_personas() { assert_eq!(guide_stories(&plain_guide), None); }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --test capture guide_ </dev/null`
Expected: compile failure, because `guide_stories` doesn't exist.

- [ ] **Step 3: Implement `guide_stories`.** Take the section from `## Stories` to the next `## ` heading. The persona names are the text between `- ` and the first `:` on lines under `### Personas`.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p herdr-intents --test capture guide_ </dev/null`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src/capture.rs tools/intents/tests/capture.rs
git commit -m "feat(intents): the run guide's Stories section names the personas"
```

### Task 3: Stories drafted in polish

**Files:**
- Modify:
  - `tools/intents/src/conversation.rs`: `STORIES_RULE` beside `POLISH_RULE` at line 252, and `seed_prompt` at line 292;
  - `tools/intents/src/interview.rs`: `Interview` gains `personas`, and the write-up prompt changes at line 101;
  - `tools/intents/src/app.rs`: these call sites —
    - where `Interview::new` is called for an add, and `polish` for `p`;
    - `PendingChat` gains `ask`;
    - the `Q` answer handler near line 2814.
- Test: `tools/intents/src/interview.rs` tests, `tools/intents/tests/conversation.rs`, `tools/intents/src/app.rs` tests

**Interfaces:**
- Consumes: `guide_stories` (Task 2), and `STORIES_WAIT` and `stories` (Task 1). The guide is found through `App::run_repo_for(it.fm.repo)` and `capture::read_guide`; for an add, the current repo stands in for the intent's repo.
- Produces:
  - `pub fn stories_rule(kind: &str, personas: Option<&str>) -> Option<String>`. It returns `None` for `refactor` and `idea`. `personas` is the guide's Stories `text`.
  - `Interview.personas: Option<String>`.
  - `PendingChat.ask: Option<String>`, a first message the chat sends on its own.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn a_feature_write_up_asks_for_stories() {
    let p = interview("enhancement", Some(GUIDE_STORIES)).prompt(Step::WriteUp, false, "");
    assert!(p.contains("## Stories") && p.contains("- shows:") && p.contains(STORIES_WAIT));
    assert!(p.contains("- Mark: tenant admin"), "the guide's personas are given");
    assert!(p.contains("not shown by any story"));
}
#[test] fn a_refactor_write_up_has_no_stories() { assert!(!interview("refactor", Some(GUIDE_STORIES)).prompt(Step::WriteUp, false, "").contains("## Stories")); }
#[test] fn without_guide_personas_stories_say_unknown() { assert!(interview("issue", None).prompt(Step::WriteUp, false, "").contains("persona: unknown")); }
#[test] fn polish_asks_for_stories_too() { assert!(seed_prompt(TEMPLATE, BODY, None, "enhancement").contains("## Stories")); }
```

In `app.rs` tests:

```rust
#[test] fn answering_the_last_question_drafts_the_stories() {
    // an enhancement with one open question and a Stories section that waits; answer it with Q
    let log = provider_log(&fx);
    assert!(log.contains("Every open question has its answer now: draft the stories."), "{log}");
    assert!(matches!(fx.app.screen, Screen::Chat(_)), "the chat opens to review the drafts");
}
#[test] fn answering_a_question_while_others_stay_open_drafts_nothing() { /* two open, answer one */ assert!(!provider_log(&fx).contains("draft the stories")); }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --lib stories </dev/null` and `cargo test -p herdr-intents --test conversation polish_asks_for_stories_too </dev/null`
Expected: compile failures, because `personas`, `stories_rule` and `PendingChat.ask` don't exist.

- [ ] **Step 3: Implement `stories_rule`.** Its text says:
  - the `## Stories` section goes right after `## Acceptance criteria`, with one `### <story name>` per story;
  - each story has `- persona:` (one of the personas listed, or `unknown` when none fits or none are listed) and `- shows: <criterion numbers counted from 1 across groups>`;
  - while `## Open questions` has a question without an answer, each story stops there and the section carries `drafts wait for the open questions`. Otherwise each story adds `- starting data:` and numbered steps;
  - a last line `not shown by any story: <numbers>` lists any uncovered criteria;
  - then the guide's Stories text follows, as data.

- [ ] **Step 4: Wire it in.**
  - **The write-up prompt** appends `stories_rule(kind, personas)` after `POLISH_RULE`.
  - **`seed_prompt`** appends it after the polish tail.
  - **The app** fills `personas` from the guide's Stories text when it builds an `Interview` or a polish chat.
  - **The `Q` handler,** after a saved answer, checks four things: the kind is `issue` or `enhancement`, no question is left unanswered, and the stories are missing or have a waiting one. If all hold, it calls `polish(PendingChat { ask: Some("Every open question has its answer now: draft the stories.".into()), .. })`.
  - **`polish` with `ask`** sends that message as the first turn.

- [ ] **Step 5: Run them to verify they pass**, then the whole suite.

Run: `cargo test -p herdr-intents --lib stories </dev/null`, then `cargo test --workspace </dev/null > target/probe/sv-t3.log 2>&1; tail -3 target/probe/sv-t3.log`
Expected: all passed.

- [ ] **Step 6: Commit**

```bash
git add tools/intents/src/conversation.rs tools/intents/src/interview.rs tools/intents/src/app.rs tools/intents/tests/conversation.rs
git commit -m "feat(intents): polish drafts stories once every question has its answer"
```

### Task 4: A take's tools: start, caption, stop

**Files:**
- Modify: `tools/intents/src/capture_mcp.rs` (`Mode`, `McpConfig`, `tool_names` at line 54, the tool list near line 366, `call` near line 472)
- Test: `tools/intents/src/capture_mcp.rs` tests

**Interfaces:**
- Produces:
  - `Mode::Story`.
  - `McpConfig.take: Option<TakeConfig>`, where `TakeConfig { title: String, persona: String, shows: Vec<(usize, String)>, dir: PathBuf }`.
  - For `Mode::Story`, the tools are `browser`, `take_start`, `caption` and `take_stop`. `caption` takes `{"criterion": <number>}`.
  - In `dir`, a take writes:
    - `video.mp4`, the recording;
    - `captions.json`, a JSON array of the criterion numbers captioned, in order;
    - `steps.txt`, the browser commands run between `take_start` and `take_stop`.
- The tools behave as follows:
  - **`take_start`:** starts `record start <dir>/video.mp4`, draws the opening card (title and persona) over the page, waits 3 s, and removes it.
  - **`caption`:** refused before `take_start`, after `take_stop`, and for a number not in `shows`. Otherwise it draws that criterion's text at the bottom of the page for 4 s and appends the number to `captions.json`.
  - **`take_stop`:** draws the closing card (the criteria captioned) for 3 s, then runs `record stop`.
  - **The cards and captions are the server's own `eval`,** a fixed script that takes the text as a JSON string literal (`serde_json::to_string`) and sets `textContent`, never `innerHTML`. The agent's `browser` tool still refuses `eval`.

- [ ] **Step 1: Write the failing tests**, using `fake-agent-browser.sh`, which logs every command:

```rust
#[test] fn a_take_has_four_tools() { assert_eq!(tool_names(&story_cfg()), ["browser", "take_start", "caption", "take_stop"]); }
#[test] fn a_caption_before_the_take_is_refused() { let (err, t) = server.call("caption", &json!({"criterion": 1})); assert!(err && t.contains("take_start"), "{t}"); }
#[test] fn a_caption_for_a_criterion_the_story_does_not_show_is_refused() { /* shows [1,3]; caption 2 */ assert!(err); }
#[test] fn captions_are_recorded_in_order() { /* take_start, caption 3, caption 1, take_stop */ assert_eq!(read_captions(dir), vec![3, 1]); assert!(log.contains("record start") && log.contains("video.mp4") && log.contains("record stop")); }
#[test] fn a_caption_with_markup_is_shown_as_text() {
    // criterion text: `say "hi" </div><script>alert(1)</script>`
    let script = last_eval(&log);
    assert!(script.contains("textContent") && !script.contains("innerHTML"));
    assert!(script.contains(&serde_json::to_string("say \"hi\" </div><script>alert(1)</script>").unwrap()));
}
#[test] fn the_agent_still_cannot_eval() { let (err, _) = server.call("browser", &json!({"command": "eval", "args": ["1"]})); assert!(err); }
#[test] fn only_the_take_is_in_the_steps() { /* browser open before take_start, click during */ assert_eq!(read_steps(dir), vec!["click @e1"]); }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --lib capture_mcp </dev/null`
Expected: compile failure, because `Mode::Story` and `TakeConfig` don't exist.

- [ ] **Step 3: Implement** the mode, the three tools and the overlays. The look of the card and caption is the builder's call: dark translucent panel, large text, no app styles leaking in (inline styles on a fixed-position element with a fixed id).

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p herdr-intents --lib capture_mcp </dev/null`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src/capture_mcp.rs
git commit -m "feat(intents): a story take's tools: cards, captions and the recording"
```

### Task 5: Recording the stories

**Files:**
- Modify:
  - `tools/intents/src/check.rs`: new `passes`;
  - `tools/intents/src/capture.rs`:
    - `Job::Story`;
    - `run()`: extract the agent launch from lines 1320–1440 into `run_agent`, add the story takes after the check block near line 1489, and add the `Job::Story` path;
    - `run_order`: the `Job::Story` arm;
    - the new `story_prompt`;
  - `tools/intents/tests/fixtures/fake-claude.sh`: add `FAKE_CLAUDE_CAPTIONS`.
- Test: `tools/intents/tests/check.rs`, `tools/intents/tests/capture.rs`

**Interfaces:**
- Consumes:
  - Task 1: `stories`, `story_folder` and `shown_criteria`;
  - Task 2: `guide_stories`;
  - Task 4: `Mode::Story`, `TakeConfig`, and the take files.
- Produces:
  - `pub fn passes(latest: &CheckRecord, body: &str) -> bool` in `check.rs`.
  - `Job::Story { numbers: Vec<usize>, intent_path: PathBuf, root: PathBuf, stories_dir: PathBuf }`, whose `run_dir` is `stories_dir`.
  - `pub struct StoryRecord { pub at: DateTime<Local>, pub outcome: StoryOutcome, pub shows: Vec<usize>, pub dropped: Vec<usize> }`, kept in `<story folder>/story.json`.
  - `pub enum StoryOutcome { Kept, NotShown(Vec<usize>), Failed(String) }`.
  - `pub fn read_story(dir: &Path) -> Option<StoryRecord>`.
  - `pub fn stories_dir(it: &Intent) -> PathBuf`, which is `it.assets_dir().join("stories")`.
- **Which stories a passing check records:** those with no `Kept` record, plus those whose `shows` includes a number in this run's `checked.results`.
- **The order within each story's agent run:**
  1. sign in as the persona;
  2. set up the starting data through the app, possibly as another listed persona;
  3. rehearse all the way through, unrecorded;
  4. set up fresh data again;
  5. `take_start`;
  6. follow the steps, calling `caption` as each criterion becomes visible;
  7. `take_stop`.
- **Time limit:** `story_limit() = 15 min` per story (my call).
- **Before any agent starts:**
  - if the guide has no Stories section, every story fails with `the run guide lists no personas for stories`;
  - if a story's persona isn't among the guide's names (case-insensitive), it fails with `persona <name> isn't in the run guide's Stories section`.

- [ ] **Step 1: Write the failing tests.** In `tests/check.rs`:

```rust
#[test] fn a_check_passes_when_nothing_fails_or_is_missing() { /* results: Confirmed 1, CantCheck 2 */ assert!(passes(&rec, BODY_TWO)); }
#[test] fn a_failing_or_missing_criterion_does_not_pass() { assert!(!passes(&with(Fails), BODY_TWO)); assert!(!passes(&with(Missing), BODY_TWO)); }
#[test] fn a_criterion_never_checked_does_not_pass() { /* body has 3 criteria, results cover 2 */ assert!(!passes(&rec, BODY_THREE)); }
```

In `tests/capture.rs`, in the commands mode served by `served(port)` as part 1's tests do, with `FAKE_CLAUDE_PROOF` for the check and `FAKE_CLAUDE_CAPTIONS` for the takes:

```rust
#[test] fn a_passing_check_records_each_story_in_the_same_run() {
    // one start of the app (start log has one line), then story.json Kept in 1-…/ and 2-…/
    assert_eq!(read_story(&s1).unwrap().outcome, StoryOutcome::Kept);
    assert_eq!(starts(&log), 1);
}
#[test] fn a_story_whose_criterion_did_not_show_keeps_no_video() { /* shows 1,3; captions "1" */ assert_eq!(read_story(&s1).unwrap().outcome, StoryOutcome::NotShown(vec![3])); assert!(!s1.join("video.mp4").exists()); }
#[test] fn a_failing_check_records_no_story() { assert!(!stories_dir.exists() || fs::read_dir(&stories_dir).unwrap().next().is_none()); }
#[test] fn a_re_check_records_again_only_the_stories_it_touched() { /* both Kept; re-check criterion 3: story 1 shows 3, story 2 shows 2 */ assert!(story_1_newer && !story_2_newer); }
#[test] fn a_guide_without_personas_records_nothing_and_says_why() { assert_eq!(read_story(&s1).unwrap().outcome, StoryOutcome::Failed("the run guide lists no personas for stories".into())); }
#[test] fn a_persona_the_guide_does_not_list_fails_that_story() { /* persona: Zed */ assert!(matches!(read_story(&s1).unwrap().outcome, StoryOutcome::Failed(w) if w.contains("Zed"))); }
#[test] fn a_cancelled_take_keeps_no_video() { /* FAKE_CLAUDE_HANG=1 during the story; cancel */ assert!(read_story(&s1).is_none_or(|r| r.outcome != StoryOutcome::Kept)); assert!(!s1.join("video.mp4").exists()); }
#[test] fn a_story_job_records_the_chosen_story() { /* Job::Story { numbers: [2] } */ assert!(read_story(&s2).is_some() && read_story(&s1).is_none()); }
#[test] fn the_story_prompt_orders_setup_rehearsal_and_take() {
    let p = story_prompt(&guide, &story, &intent, "http://127.0.0.1:1", &stories_text);
    let at = |s: &str| p.find(s).unwrap();
    assert!(at("sign in as Mark") < at("rehearse") && at("rehearse") < at("set up fresh") && at("set up fresh") < at("take_start"));
    assert!(p.contains("never sign in, type a password or a code after take_start"));
    assert!(p.contains("fresh records with new names"), "leftovers from an earlier run");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --test check passes </dev/null` and `cargo test -p herdr-intents --test capture stor </dev/null`
Expected: compile failures, because `passes`, `Job::Story` and `read_story` don't exist.

- [ ] **Step 3: Extend the fake claude.** `FAKE_CLAUDE_CAPTIONS=<n,…>` writes those numbers as `captions.json` in the current folder, and `video` to `video.mp4` there. The story agent runs with its story folder as its working directory, as the capture agent runs in its evidence folder.

- [ ] **Step 4: Implement.**
  - `passes`.
  - `run_agent(req, r, state, mcp_config, prompt, limit, cwd) -> String`, the existing launch moved out unchanged. The capture and check paths use it.
  - The story takes:
    - after the check block when `passes(&latest, body)`;
    - or directly for `Job::Story`, after the seeds.
    - Each story gets its own `McpConfig` (`Mode::Story`, `take`), its own agent-browser session, and `story.json`.
    - A cancel stops the take and deletes any `video.mp4` of an unfinished take.

- [ ] **Step 5: Run them to verify they pass,** then the whole suite.

Run: `cargo test -p herdr-intents --test check --test capture </dev/null`, then `cargo test --workspace </dev/null > target/probe/sv-t5.log 2>&1; tail -3 target/probe/sv-t5.log`
Expected: all passed.

- [ ] **Step 6: Commit**

```bash
git add tools/intents/src/check.rs tools/intents/src/capture.rs tools/intents/tests/fixtures/fake-claude.sh tools/intents/tests/check.rs tools/intents/tests/capture.rs
git commit -m "feat(intents): a passing check records each story's video"
```

### Task 6: The Stories section in the detail view

**Files:**
- Modify:
  - `tools/intents/src/app.rs`:
    - `Pane::Stories` in the pane enum near line 520, the Tab order (Criteria → Stories → Demos → Questions), and its key handler beside `Pane::Demos` near line 2519;
    - the help line near line 6119;
    - a new `record_story(&Intent, usize)` that launches `Job::Story` as `check_now` launches a check;
  - `tools/intents/src/ui.rs`: the section's rows beside `Pane::Demos` near line 1435.
- Test: `tools/intents/src/app.rs` tests, with insta snapshots for the section

**Interfaces:**
- Consumes: `stories` (Task 1), `read_story`, `stories_dir`, `passes`, `Job::Story` (Task 5), and the existing `open_in_chrome`, `copy_file` and `copy_path`.
- **Each row reads:** `<n>. <name> · <state>`, where the state is one of:
  - `video`;
  - `recording…`, while a runner holds `stories/.capture.lock`;
  - `criterion <n> didn't show`;
  - `couldn't record: <why>`;
  - `drafts wait for the open questions`;
  - `not recorded yet`.
- **Keys:**
  - `Enter` and `o` play the video;
  - `c` copies the file, as Evidence's `c` does;
  - `Y` copies the path;
  - `O` shows it in Finder (`open -R`, or `xdg-open` on its folder elsewhere);
  - `r` records it again;
  - `Esc` goes back to the tree.
- **Messages:** with no video, the play and copy keys say `no video for this story yet`. `r` before a passing check says `the completion check hasn't passed yet`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn the_stories_section_shows_each_story_and_its_state() { /* one Kept, one NotShown([3]), one waiting */ insta::assert_snapshot!(detail(&fx)); }
#[test] fn c_copies_a_storys_video() { /* OSASCRIPT_BIN_PATH fake logs */ assert!(osascript_log(&fx).contains("video.mp4")); }
#[test] fn o_plays_a_storys_video() { assert!(opened(&log).contains("video.mp4")); }
#[test] fn capital_o_shows_the_video_in_finder() { assert!(opened(&log).contains("-R") && opened(&log).contains("video.mp4")); }
#[test] fn recording_by_hand_waits_for_a_passing_check() { key(&mut fx.app, KeyCode::Char('r')); assert_eq!(fx.app.message.as_deref(), Some("the completion check hasn't passed yet")); }
#[test] fn r_records_the_story_again_after_a_passing_check() { /* latest.json passes */ assert!(order_written(&fx).contains("\"numbers\":[2]")); }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --lib stor </dev/null`
Expected: compile failure, because `Pane::Stories` doesn't exist.

- [ ] **Step 3: Implement** the pane, its rows, the keys and `record_story`. Then review the new snapshot with `cargo insta review`, or by reading the `.snap.new` file and accepting it.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p herdr-intents --lib stor </dev/null`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add tools/intents/src/app.rs tools/intents/src/ui.rs tools/intents/src/snapshots
git commit -m "feat(intents): the Stories section plays, copies and re-records each video"
```

### Task 7: README and the whole suite

**Files:**
- Modify: `tools/intents/README.md`, with a "Stories and demo videos" section after "Spaces and the run guide"

- [ ] **Step 1: Write the README section.** Cover:
  - when stories are drafted, and the `## Stories` format;
  - the run guide's `## Stories` section, with an example of `### Personas` and `### Setup may create`;
  - when videos record (a passing check), and where they live;
  - the keys;
  - that the guide's rules win.

- [ ] **Step 2: Run everything**

Run: `cargo test --workspace </dev/null > target/probe/sv-final.log 2>&1; tail -3 target/probe/sv-final.log; sh scripts/test-install.sh </dev/null | tail -1; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --check`
Expected: all passed; install tests pass; clippy and fmt clean.

- [ ] **Step 3: Commit**

```bash
git add tools/intents/README.md
git commit -m "docs(intents): stories and demo videos"
```

Out of scope here, for the owner's word: adding a parent persona to skoolscout's run guide (`chore/platform/tooling--capture-guide` in skoolscout-com).
