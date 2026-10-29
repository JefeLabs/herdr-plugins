# Evidence Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A bug added by conversation can carry evidence:
- after the last question, `s` screenshot or `v` video starts a background capture: a seeded local stack, then a claude agent restricted to `agent-browser`, which reproduces the bug and records it;
- after every fix PR merges, the same steps run again for the "after";
- an Evidence section opens and copies the files.

**Architecture:**
- **A new `src/capture.rs` holds the pure parts:** the capture guide, choosing the area, parsing the agent's outcome and steps, capture records and the agent's prompt. It also holds the runner: one background thread per capture that runs `work start` → seed → the claude agent → `work finish`, and sends updates over a channel.
- **`app.rs` adds** the prompt in the add view, the Evidence pane, the `E` key, and the lifecycle hooks: persist when the issue exists, re-run after merge, find PRs by issue number.
- **Files:** a capture writes into the intent's agent folder (`.polish/<polish_name>/evidence/`) and moves to `<stem>.assets/` once an `issue:` line exists.

**Tech Stack:** Rust 2024, ratatui; shell fixtures for `work`, `make`, `claude` (stream-json) and `agent-browser`; insta; tempfile.

**Spec:** `docs/superpowers/specs/2026-10-29-intent-evidence-capture-design.md`

## Global Constraints

- Paths are under `tools/intents/` unless they start with `docs/` or `scripts/`. Branch `feature/evidence-capture` from `main`, with local commits ending `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No push, PR or merge without the owner's word.
- **Tests:**
  - per task, `cargo test -q --release -p herdr-intents </dev/null`;
  - at the end, the workspace tests, `cargo fmt --all --check`, `cargo clippy -q --release --workspace --all-targets -- -D warnings` and `sh scripts/test-install.sh`.
- **The capture agent's only tool is Bash restricted to `agent-browser`.** That means `--tools Bash --allowedTools "Bash(agent-browser:*)"`, no other tool, and never `--dangerously-skip-permissions`. It runs in the evidence folder, never in a repo.
- **workbrokr:**
  - every capture is its own task effort, `work start --area <a> --task evidence-<n> --type fix --mode fullstack`, with `WORK_SESSION=capture-<stem>`;
  - it never passes `--join` or `--into-area`, and never sets `SLOT`;
  - a capture that started an effort always runs `work finish --area <a> --task evidence-<n>`, on success, failure and shutdown alike.
- **Tool binaries are found via env first, then PATH:** `WORK_BIN_PATH`, `MAKE_BIN_PATH`, `CLAUDE_BIN_PATH`, `AGENT_BROWSER_BIN_PATH`, `OPEN_BIN_PATH`, `OSASCRIPT_BIN_PATH`, `PBCOPY_BIN_PATH`. Use `progress::tool_bin(env, var, default)` as the existing `gh`/`herdr` lookups do.
- **No credential is written anywhere.** Sign-in is the Cmd+U persona picker; dev codes are email 123456, SMS 654321.
- **Only bugs (kind `issue`)** get the capture prompt and the `E` key.

## Review Focus

These are the inputs most likely to bite that the tasks' main tests don't cover, with the test each gets:

1. The tool quits, or the intent is trashed, while a capture runs. The agent is stopped and its effort finished; no slot stays leased. Test: `shutdown_finishes_a_running_capture` (Task 3).
2. `E` is pressed while a capture is already running for that intent. It's refused ("capturing already"); no second effort starts. Test: `e_refuses_a_second_capture_while_one_runs` (Task 4).
3. The agent finishes without an outcome line. That's a failure ("no outcome"), never "reproduced". Test: `a_reply_without_an_outcome_is_a_failure` (Task 1).
4. The intent's repo has no `.helmsmith/capture.md`. The prompt says "no capture guide in <repo>" and offers only Enter; nothing starts. Test: `no_capture_guide_means_no_capture` (Task 3).
5. An evidence path containing spaces. `open`, `osascript` and `pbcopy` get it as one argument. Test: `evidence_paths_with_spaces_reach_the_opener_whole` (Task 4).

---

### Task 1: The pure parts of capture

**Files:**
- Create: `src/capture.rs`.
- Modify: `src/lib.rs` (`pub mod capture;`).
- Test: unit tests in `src/capture.rs`.

**Interfaces (produces, exact):**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum Shot { Screenshot, Video }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum Phase { Before, After }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome { Running(String), Reproduced(String), NotReproduced(String), Fixed(String), StillBroken(String), Failed(String) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureGuide { pub default_area: String, pub seed: Vec<String>, pub url: String, pub body: String }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRecord { pub phase: Phase, pub shot: Shot, pub outcome: Outcome, pub file: Option<String>,
    pub steps: Vec<String>, pub viewport: (u16, u16), pub at: chrono::DateTime<chrono::Local> }
pub const VIEWPORT: (u16, u16) = (1280, 800);
pub fn read_guide(repo: &Path) -> anyhow::Result<CaptureGuide>;          // <repo>/.helmsmith/capture.md
pub fn area_candidates(group_rel: Option<&str>, guide: &CaptureGuide) -> Vec<String>;
pub fn issue_numbers(body: &str) -> Vec<u64>;                            // from `issue:` artifact lines
pub fn evidence_dir(polish_folder: &Path) -> PathBuf;                    // <folder>/evidence
pub fn write_record(dir: &Path, rec: &CaptureRecord) -> anyhow::Result<()>;   // <dir>/<phase>.json
pub fn read_records(dir: &Path) -> Vec<CaptureRecord>;                   // before first, then after
pub fn capture_prompt(guide: &CaptureGuide, phase: Phase, shot: Shot, intent: &str,
    base_url: &str, out_file: &Path, before: Option<&CaptureRecord>) -> String;
pub fn parse_outcome(phase: Phase, text: &str) -> Outcome;
pub fn steps_from_stream(line: &str) -> Option<String>;                  // one claude stream-json line
```

**Rulings:**
- **The guide's front matter** is YAML (`default_area`, `seed` as a list of story ids, `url` with a `{port}` placeholder). The body is the agent's instructions.
- **`area_candidates`** returns the group path's last two segments when there are at least two (`skoolscout/discover-schools/org/team` → `org/team`), then `default_area`, without duplicates. `work start` decides validity, so there is no config parsing.
- **Outcome lines are matched case-insensitively** anywhere in the text, the last one winning:
  - for the "before": `reproduced:` / `couldn't reproduce:` (also "could not reproduce:");
  - for the "after": `fixed:` / `still broken:`;
  - no line gives `Failed("no outcome in the agent's reply")`.
- **`steps_from_stream`** returns the `command` of an assistant `tool_use` named `Bash` whose command starts with `agent-browser`.
- **The prompt contains:**
  - the guide body;
  - the base URL;
  - "first run `agent-browser set viewport 1280 800`";
  - for a video, `agent-browser record start <out_file>` … `record stop`; for a screenshot, `agent-browser screenshot <out_file>` at the moment the bug shows;
  - the intent text;
  - for the "after", the "before"'s steps and outcome, with "repeat these steps exactly";
  - the outcome-line contract.

- [ ] **Step 1: Write the failing tests**
  - `the_guide_is_read_from_helmsmith_capture_md`: front matter `default_area: org/social`, `seed: [kathia-signup]`, `url: https://demo-org.skoolscout.com.local:{port}`, plus a body. The fields parse, and the body excludes the front matter. A missing file is an error that names the path.
  - `area_candidates_try_the_intents_area_then_the_default`:
    - `Some("skoolscout/discover-schools/org/team")` gives `["org/team", "org/social"]`;
    - `Some("skoolscout")` gives `["org/social"]`;
    - `None` gives `["org/social"]`;
    - `Some(".../org/social")` gives `["org/social"]`, with no duplicate.
  - `issue_numbers_come_from_issue_lines`: a body with `- issue: #123`, `- issue: https://github.com/o/r/issues/456`, `- issue: .issues/x.md` and `- pr: #9` gives `[123, 456]`.
  - `outcome_lines_are_read_per_phase`: Before with `"…\nreproduced: the email goes to every admin"` gives `Reproduced("the email goes to every admin")`. Before with `"Couldn't reproduce: no invite button"` gives `NotReproduced`. After with `still broken:` gives `StillBroken`, and with `fixed:` gives `Fixed`.
  - `a_reply_without_an_outcome_is_a_failure` (Review Focus 3).
  - `steps_are_the_agent_browser_commands_it_ran`:
    - a stream line `{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"agent-browser open https://x"}}]}}` gives `Some("agent-browser open https://x")`;
    - a Bash `ls` gives `None`;
    - a text delta gives `None`.
  - `records_round_trip_before_then_after`: write After, then Before; `read_records` returns Before then After.
  - `the_prompt_carries_viewport_output_and_contract`: a video Before prompt contains `set viewport 1280 800`, `record start <out_file>`, `reproduced:`, the guide body and the base URL. An After prompt contains each "before" step and `fixed:`.

- [ ] **Step 2: Run them to see them fail.** Run the per-task command. Expected: compile errors, then FAIL with stubs.
- [ ] **Step 3: Implement.** `serde_yaml_ng` is already a dependency (store.rs uses it).
- [ ] **Step 4: Run them to see them pass.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): capture guide, outcomes, steps and records`.

### Task 2: The capture runner

**Files:**
- Modify: `src/capture.rs` (runner).
- Create: `tests/fixtures/fake-work.sh`, `tests/fixtures/fake-make.sh`, `tests/fixtures/fake-claude.sh`.
- Test: `tests/capture.rs`.

**Interfaces:**

```rust
pub struct CaptureRequest { pub repo: PathBuf, pub group_rel: Option<String>, pub stem: String,
    pub phase: Phase, pub shot: Shot, pub intent: String, pub evidence: PathBuf,
    pub before: Option<CaptureRecord>, pub n: u32 }
pub enum CaptureUpdate { Progress(String), Done(CaptureRecord) }
pub struct CaptureJob { /* rx, child pid for cancel, the started effort if any */ }
impl CaptureJob {
    pub fn spawn(req: CaptureRequest, env: Vec<(String, String)>, managed_mcp: bool) -> CaptureJob;
    pub fn try_recv(&self) -> Option<CaptureUpdate>;
    pub fn cancel(&mut self);   // kills the agent's group, then `work finish` if an effort was started
}
```

**Rulings — the thread runs, in order:**
1. `read_guide(repo)`. An error gives `Done(Failed("no capture guide in <repo>"))`.
2. For each area in `area_candidates`, `work start --area A --task evidence-<n> --type fix --mode fullstack --json` in `repo`, with `WORK_SESSION=capture-<stem>`.
   - The JSON `error.code` values `AREA_UNKNOWN`, `MANIFEST_INVALID` and `SCOPE_CONFLICT` move on to the next area.
   - Any other failure gives `Failed("work start: <code or message>")`.
   - Finding no area gives `Failed("no area could start a stack: <codes>")`.
3. `work env --area A --task evidence-<n> --json`. Read the frontend port (`SS_FE_PORT`) and the worktree path.
4. For each seed story, `make -C <worktree>/app-test-e2e-runner e2e-story STORY=<s> FAST=1`. Its failure gives `Failed("seed <s>: exit <code>")`, and the effort is still finished.
5. `claude --output-format stream-json --verbose --tools Bash --allowedTools "Bash(agent-browser:*)" [--strict-mcp-config] -p <prompt>`, with cwd = `evidence`.
   - `--strict-mcp-config` is added only when `!managed_mcp`.
   - The agent's output file is `evidence/<phase>.<png|mp4>`.
   - Read stdout lines: `steps_from_stream` collects steps; the `result` line's `result` text goes to `parse_outcome`.
6. `work finish --area A --task evidence-<n>`, always once step 2 succeeded.
7. Send `Done(record)`, with `file` set when the output file exists. `Progress` messages name each stage ("starting a stack in org/team", "seeding", "the agent is reproducing it").

**The fakes:**
- `fake-work.sh` logs `"$WORK_SESSION $*"` to `$FAKE_WORK_LOG`. `FAKE_WORK_REFUSE=<area>:<code>` makes `start` for that area print `{"error":{"code":"<code>"}}` and exit 1. `env` prints `{"SS_FE_PORT":"3004","worktree":"<FAKE_WORK_TREE>"}`.
- `fake-make.sh` logs its arguments; `FAKE_MAKE_FAIL=1` exits 2.
- `fake-claude.sh` logs its arguments, prints `$FAKE_CLAUDE_EVENTS`, and writes `FAKE_CLAUDE_WRITES` (a path) when set, standing in for the recording agent-browser would make.

- [ ] **Step 1: Write the failing tests** (`tests/capture.rs`, each with a temp repo holding `.helmsmith/capture.md`)
  - `a_capture_runs_start_env_seed_agent_finish_in_order`:
    - the work log reads `capture-<stem> start --area org/team --task evidence-1 …`, then `… env …`, then `… finish --area org/team --task evidence-1`;
    - the make log has `e2e-story STORY=kathia-signup FAST=1`;
    - the claude arguments contain `--allowedTools Bash(agent-browser:*)` and never `--dangerously`;
    - the record is `Reproduced`, with its file and the two steps.
  - `a_refused_area_falls_back_to_the_guides_default`: with `FAKE_WORK_REFUSE=org/team:SCOPE_CONFLICT`, the second start is `org/social` and the finish names `org/social`.
  - `capture_never_joins_or_merges_into_the_area`: across all the cases above, no work-log line contains `--join` or `--into-area`.
  - `a_failed_seed_still_finishes_the_effort`: `FAKE_MAKE_FAIL=1` gives `Failed("seed kathia-signup: exit 2")`, with a `finish` in the log.
  - `a_start_that_fails_otherwise_ends_the_capture`: `FAKE_WORK_REFUSE=org/team:HEALTH_TIMEOUT` gives `Failed` containing `HEALTH_TIMEOUT`, with no second area and no finish.
  - `strict_mcp_follows_the_managed_config`: `managed_mcp=false` passes `--strict-mcp-config`; `true` doesn't.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** the runner (a thread, its process group like `conversation::Turn`, `cancel` as in the rulings).
- [ ] **Step 4: Run them to see them pass.**
- [ ] **Step 5: Commit** `feat(intents): background capture runner over work, seed and a browser-only agent`.

### Task 3: Asking in the add view and running in the background

**Files:**
- Modify: `src/interview.rs` (an `Awaiting::Capture` variant and the choice).
- Modify: `src/app.rs` (the prompt, starting jobs, `capture_jobs`, tick, shutdown, the Evidence data in the view).
- Test: `src/app.rs` tests.

**Interfaces:**
- `InterviewScreen.capture: Option<Shot>`.
- `App.capture_jobs: HashMap<String /* polish folder name */, (CaptureJob, CaptureRequest)>`.
- `App.capture_env: Vec<(String, String)>`: the env the runner gets, `*_BIN_PATH` included. It's empty in production; tests point it at the fakes.
- `fn start_capture_job(&mut self, req: CaptureRequest)`.
- `View.evidence: Vec<EvidenceRow>`, where `pub struct EvidenceRow { pub label: String, pub file: Option<PathBuf>, pub mark: &'static str }` (`…` running, `✓` reproduced/fixed, `✗` still broken, `!` failed, `·` not reproduced).

**Rulings:**
- **When the prompt appears:** on kind `issue`, at the moment the interview would start the write-up (budget spent, `enough`, or Ctrl-d), it first sets `Awaiting::Capture`. The status line reads `Capture evidence? s screenshot · v video · Enter skip`. Any choice then starts the WriteUp turn as before.
- **`s`/`v` start a Before job.** It runs with `repo` from `send::capture_repo()` (as save uses), `group_rel` = home or None, `stem` = the folder name, `evidence` = `evidence_dir(folder)`, and `n` = `1 + existing records`.
- **Without a guide,** the prompt reads `no capture guide in <repo> · Enter continue`, and `s`/`v` are ignored.
- **Without claude** (`CLAUDE_BIN_PATH`, else `claude` on PATH), it reads `capture needs claude, which is not installed · Enter continue`.
- **`tick` drains the jobs:** `Progress` updates a running record (written to the evidence folder so the view reads one source), and `Done` writes the final record.
- **`App::shutdown` cancels every capture job,** and `cancel` finishes its effort.
- **The view** reads records from `.polish/<polish_name>/evidence/` for unsent intents, and from `<stem>.assets/evidence/` once moved (Task 5).

- [ ] **Step 1: Write the failing tests**
  - `a_bug_is_asked_about_evidence_before_the_write_up`: a bug interview, using the existing test helpers `start_bug_in_skoolscout` and `agent_replies`, with `capture_env` pointing at the Task 2 fakes and the temp repo holding a guide, gives `RESTATE` then `enough`. Awaiting is `capture`, and the status line contains `Capture evidence?`. On `v`, the fake work log has a `start`, the next provider call contains `WRITE_UP_MARKER`, and awaiting reaches `review`.
  - `a_feature_is_never_asked`: the same with `f` goes straight to the write-up.
  - `enter_skips_capture`: no work-log lines.
  - `no_capture_guide_means_no_capture` (Review Focus 4).
  - `no_claude_means_no_capture`: with `CLAUDE_BIN_PATH` set to a missing file, the prompt names claude and `v` starts nothing.
  - `the_saved_bug_shows_its_capture_in_evidence`: after save and the job's `Done(Reproduced)`, `view().evidence[0]` is `mark "✓"`, with a label containing `before · video · the email goes to every admin`.
  - `shutdown_finishes_a_running_capture` (Review Focus 1): `FAKE_CLAUDE_HANG=1`, then `shutdown()`. The work log ends with `finish`.
- [ ] **Step 2–4: red, implement, green.**
- [ ] **Step 5: Commit** `feat(intents): ask for evidence on bugs and capture it in the background`.

### Task 4: The Evidence pane and its keys

**Files:**
- Modify: `src/ui.rs` (`Pane::Evidence`, drawing the section).
- Modify: `src/app.rs` (focus order, keys, `E`).
- Test: `src/app.rs`, `src/ui.rs` tests.

**Rulings:**
- **Evidence sits after Artifacts** in the accordion and in Tab order. Each row is `<mark> before · video · <outcome text>`, with `after · …` beneath when present.
- **Enter or a click** runs `OPEN_BIN_PATH` (default `open`) with `-a "Google Chrome" <file>` on macOS; on Linux, `xdg-open <file>`.
- **`c` copies the file itself:** on macOS, `osascript -e 'set the clipboard to (POSIX file "<file>")'`, with the path passed as an argument via `on run argv`. Over SSH (`SSH_CONNECTION` set) or off macOS, it falls back to the path, like `Y`.
- **`Y` copies the path:** `pbcopy` on macOS; elsewhere an OSC 52 sequence written to the terminal.
- **`E` on a bug intent** starts a Before capture when no successful "before" exists, else an After. It is refused while a job for the intent runs ("capturing already"), and on non-bugs ("evidence is for bugs").

- [ ] **Step 1: Write the failing tests**
  - `enter_opens_the_evidence_in_chrome`: the fake open logs `-a Google Chrome <path>`.
  - `c_copies_the_file_and_y_the_path`: the fake osascript gets the path in argv; the fake pbcopy reads the path on stdin. With `SSH_CONNECTION=x`, `c` goes to the path copier instead.
  - `evidence_paths_with_spaces_reach_the_opener_whole` (Review Focus 5).
  - `e_refuses_a_second_capture_while_one_runs` (Review Focus 2).
  - `e_is_only_for_bugs`.
  - `evidence_section_snapshot`: 80×24, with one "before" ✓ and one "after" ✗.
- [ ] **Step 2–4: red, implement, green.**
- [ ] **Step 5: Commit** `feat(intents): Evidence section — open in Chrome, copy the file or its path, capture again`.

### Task 5: Keeping, re-running and finding the fix

**Files:**
- Modify: `src/app.rs` (persist on issue, the "after" on merge, `request_found_prs` with issue numbers).
- Modify: `src/store.rs` (`trash` also moves the agent folder's evidence; already covered by the polish move — add the assertion).
- Test: `src/app.rs` tests, `tests/store.rs`.

**Rulings:**
- **Persist:** on rescan, a sent intent with an `issue:` artifact whose agent folder holds `evidence/` has it moved to `<stem>.assets/evidence/`.
- **✗ for an "after"** comes from the agent's `still broken:` line. A screenshot diff is not built here; the spec leaves it to the builder.
- **The "after":** in `check_reconciles`, when `needs_reconcile` reports a merged set and a successful "before" exists with no "after" for that merged set, start an After job (`n` = next). The record keeps the merged PR numbers so it runs once per set.
- **PR discovery:** `request_found_prs` adds `#<n>` queries for every number from `issue_numbers(body)`, beside the spec and plan names.

- [ ] **Step 1: Write the failing tests**
  - `evidence_moves_into_the_attachments_once_the_issue_exists`.
  - `the_after_runs_once_every_pr_merged`: the fake gh is already used by the list fixture. The work log has a second `start`, and `read_records` has an After.
  - `issue_numbers_join_the_pr_search`: the fake gh log contains `"#123"`.
  - `tests/store.rs::trash_takes_the_evidence_along`.
- [ ] **Step 2–4: red, implement, green.**
- [ ] **Step 5: Commit** `feat(intents): keep evidence with the issue, re-run it after the fix, find the fix by issue number`.

### Task 6: skoolscout-com — the capture guide and the issue skill

**Repo:** `~/Development/Workspaces/skoolscout/skoolscout-com`, following its CLAUDE.md:
- begin with `work start --area platform/tooling --type chore --mode <the lightest mode workbrokr accepts there>` (ledger the choice);
- never touch the main checkout;
- commit only the named paths;
- no push without the owner's word.

**Files:**
- Create: `.helmsmith/capture.md`. Its front matter is `default_area: org/social`, `seed: [kathia-signup]` (confirm in `app-test-e2e-runner/tests/activities.ts` that its chain runs `mark-signup` first, and ledger it), and `url: https://demo-org.skoolscout.com.local:{port}`. The body covers:
  - sign in at `/auth/sign-in`, Cmd+U, the persona for the role: MARK for tenant admin, KATHIA for parent, and so on from `app-ui/app/auth/sign-in/devPersonas.ts` by name only, no passwords;
  - dev codes: email 123456, SMS 654321;
  - never run an e2e story or anything with `mark-signup`, since the tool seeds before you start;
  - stay on `demo-org.skoolscout.com.local:<port>`.
- Modify: `.claude/skills/issue-report/SKILL.md`, its Evidence section. When the intent has capture attachments:
  - use `evidence/before.png` as the issue's sibling PNG;
  - list the video and tell the owner to paste it (⌘V) into the GitHub issue when it is created.

- [ ] **Step 1:** write both files; check them against the spec's "In skoolscout-com".
- [ ] **Step 2:** run `npx biome check .` from the repo root if the skill or guide paths are linted, and the repo's markdown checks if any. Expected: clean.
- [ ] **Step 3: Commit** in the worktree: `chore(tooling): capture guide for herdr-intents evidence; issue-report uses captured evidence`.

### Task 7: README and the whole suite

**Files:** `tools/intents/README.md`.

- [ ] **Step 1:** README: an "Evidence for bugs" section covering:
  - the prompt;
  - background capture;
  - what the agent may do;
  - the capture guide (`.helmsmith/capture.md`) and its fields;
  - the Evidence keys (Enter, `c`, `Y`, `E`);
  - before/after and ✗;
  - where files live.
- [ ] **Step 2:** `cargo test -q --release --workspace </dev/null && cargo fmt --all --check && cargo clippy -q --release --workspace --all-targets -- -D warnings && sh scripts/test-install.sh </dev/null`. Expected: all clean.
- [ ] **Step 3: Commit** `docs(intents): evidence for bugs`.
