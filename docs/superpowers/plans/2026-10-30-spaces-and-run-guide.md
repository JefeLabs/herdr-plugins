# Spaces and the Run Guide Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Captures and completion checks run on any project. A space's meta repo says which repos it has (`.helmsmith/space.toml`) and how to run its app (`.helmsmith/run.md`). Skoolscout's `work`-based way keeps working unchanged.

**Architecture:**
- **New `space.rs`:** reads the shared space file, and each person's space → meta repo pointers.
- **`capture.rs`:** the guide gains a commands mode (`start`, `url {port}`, `ready`, `seed`, `stop`) beside today's `work` mode (`default_area`). The detached runner starts the app's process group, waits for it to answer, seeds, runs the agent, then stops it, even on a cancel.
- **`app.rs`:** looks for the guide in the space's meta repo, then the intent's repo. Automatic after-merge checks in a commands-mode space wait until the intent's repo has the merged commits.

**Tech Stack:** Rust 2024 (tools/intents), serde/toml/serde_yaml_ng, `std::process` process groups (nix-style via libc already used), tests in `tests/capture.rs`, `tests/check.rs`, app unit tests, fake programs in `tests/fixtures/`.

**Spec:** `docs/superpowers/specs/2026-10-30-spaces-and-run-guide-design.md`

## Global Constraints

- **Skoolscout keeps working exactly as today.** A guide whose front matter has `default_area` uses `work start`/`env`/`finish`, `make e2e-story` seeds, the port from `work env`, and logs relative to the slot's worktree. Its existing tests stay green, unchanged.
- **Guide file:** `.helmsmith/run.md`, falling back to `.helmsmith/capture.md`.
- **Guide lookup order:**
  1. the space's meta repo, when one is known;
  2. the intent's repo;
  3. an error: `no run guide: set [spaces.<name>] meta in <config path>, or add .helmsmith/run.md to <repo>`.
- **Commands mode:**
  - every command runs with `sh -c` from the meta repo, with `PORT` set when `url` has `{port}`;
  - `start` runs in its own process group;
  - `ready` (default: the url itself) is polled until it answers, or `ready_timeout` seconds pass (default 600);
  - `seed` commands run in order and need exit 0;
  - `stop` (optional) runs afterwards, then `start`'s group is ended, always, including on a cancel and on failure.
- **Ports:** a free port is picked per run when `url` has `{port}`. Without `{port}`, only one run at a time per space; a second is refused with the address in use.
- **Logs (commands mode):** paths relative to the meta repo. No symlinks, and they must be canonically inside the meta repo's parent folder.
- **Space file:** `<meta>/.helmsmith/space.toml` is `repos = [{ path, git }]`, with paths relative to the meta repo. A missing repo is named with `git clone <git> <path>`. The tool never clones.
- **Pointers:** `<config dir>/spaces.toml`, `[spaces.<name>] meta = "<path>"`, where `<name>` is the Herdr workspace scope. When the tool starts in a workspace with no pointer and the workspace folder has `.helmsmith/space.toml`, it is recorded and a message says so.
- **Waiting for the pull (owner):** in a commands-mode space, the automatic after-merge check starts only when the intent's repo's checked-out `HEAD` contains every merge commit of that merged set (`git merge-base --is-ancestor`). It never fetches or pulls. Until then the set isn't spent, and the card reads `waiting for a pull`. `X` by hand is not gated.
- **Unchanged:** the confined agent. Guide commands run in the runner, never in the agent.
- **Tests:**
  - stdin from `/dev/null`;
  - no real app, network, `work`, `make` or `claude`: fake programs only;
  - commands-mode tests use an in-test `TcpListener` as "the app", or a fake `start` script.
- Commit per task. No push.

## Review Focus

1. **A cancelled or crashed run leaves no `start` process running.** Test: `a_cancelled_run_stops_the_apps_process_group` (Task 2).
2. **`start` exits at once** (a typo in the guide): the run fails fast with `start` exited, plus the last lines of its output. It doesn't wait out the 10-minute timeout. Test: `a_start_that_exits_fails_fast_with_its_output` (Task 2).
3. **Both `run.md` and `capture.md` present:** `run.md` wins. A `capture.md` with `default_area` behaves exactly as today. Test: `run_md_wins_and_capture_md_still_works` (Task 1).
4. **A second run in a space without `{port}`** is refused with a clear message rather than colliding on the port. Test: `a_second_run_without_a_port_placeholder_waits_its_turn` (Task 2).
5. **A guide log path that leads out of the space** (`../../etc/passwd`, or a symlink) is refused before the agent starts. Test: `a_log_outside_the_space_is_refused` (Task 2).

---

### Task 1: The space file, the pointers, and finding the guide

**Files:**
- Create: `tools/intents/src/space.rs` (and `pub mod space;` in `lib.rs`)
- Modify: `tools/intents/src/capture.rs` (`read_guide`: `run.md` then `capture.md`; `GuideFront`/`CaptureGuide` gain the commands-mode fields and a `mode`)
- Modify: `tools/intents/src/app.rs` (`guide_repo_for`; `capture_block_for`, `start_check` and `start_capture_job` use it; record a pointer on start)
- Test: `tools/intents/tests/space.rs` (new), `tests/capture.rs`, app tests

**Interfaces:**
- Produces:
  - `space::read_space(meta: &Path) -> anyhow::Result<Space>`, where `Space { meta: PathBuf, repos: Vec<SpaceRepo { path: PathBuf /* absolute */, git: String }> }`
  - `space::missing(&Space) -> Vec<&SpaceRepo>`
  - `space::meta_for(dirs: &Dirs, name: &str) -> Option<PathBuf>`
  - `space::remember_meta(dirs: &Dirs, name: &str, meta: &Path) -> anyhow::Result<()>`
  - `capture::GuideMode::{Work { default_area: String }, Commands { start: String, ready: Option<String>, ready_timeout: u64, stop: Option<String> }}`, with `CaptureGuide.mode`. `seed` stays `Vec<String>`: e2e stories in work mode, commands in commands mode.
  - `App::guide_repo_for(&self, intent_repo: Option<PathBuf>) -> Result<PathBuf, String>`

- [ ] **Step 1: Write the failing tests:**
  - `tests/space.rs`:
    - `a_space_lists_its_repos_beside_the_meta_repo`: `.` and `../api` resolve to absolute paths; a missing `../web` is reported with `git clone <git> <path>`.
    - `pointers_are_remembered_per_space`.
  - `tests/capture.rs`:
    - `run_md_wins_and_capture_md_still_works`: both files are present and `run.md`'s url is used; with `capture.md` alone and `default_area` set, `mode` is `Work` with today's fields.
    - `a_commands_guide_is_read`: `start`, `ready`, `ready_timeout` (default 600) and `stop` are read.
    - `a_guide_with_neither_start_nor_default_area_is_refused`.
  - App tests:
    - `the_guide_comes_from_the_spaces_meta_repo`;
    - `without_a_space_the_intents_repo_guide_is_used`;
    - `no_guide_anywhere_names_both_places`;
    - `a_meta_repo_is_remembered_when_the_tool_starts_in_it`.
- [ ] **Step 2: Run** `cargo test -q --release -p herdr-intents </dev/null`. Expected: the new tests fail; the rest pass.
- [ ] **Step 3: Implement** to the interfaces above. `space.toml` is parsed with `toml`; pointers are written to `<config dir>/spaces.toml`. The app uses `guide_repo_for` wherever it read the guide from the intent's or current repo, and passes that repo as `CaptureRequest.repo`.
- [ ] **Step 4: Run** the whole intents suite. Expected: PASS, including every existing capture and check test unchanged.
- [ ] **Step 5: Commit** `feat(intents): spaces: the meta repo's run guide and repo list`.

### Task 2: Running the app from the guide's commands

**Files:** Modify `tools/intents/src/capture.rs` (`run`: a commands-mode branch beside the `work` one; teardown in `finish_once` and on cancel; logs containment), `tools/intents/src/check.rs` (log paths relative to the meta repo in commands mode). Test: `tests/capture.rs`, `tests/check.rs`.

**Interfaces:**
- Consumes: Task 1's `GuideMode::Commands` and `CaptureRequest.repo` (the meta repo).
- Produces: `capture::free_port() -> u16`. The run's own record of `start`'s process group and output file sits beside the run lock (`.app.pgid`, `.app.log` in `run_dir()`), so `cancel_running` and a crashed runner's next start can end it.

- [ ] **Step 1: Write the failing tests.** "The app" is a `TcpListener` the test owns; `url` points at its port, and `start` is a script that logs and sleeps. For `{port}`, `start` runs `python3 -m http.server $PORT`, and the test is skipped when `python3` is absent.
  - `a_commands_guide_starts_seeds_runs_and_stops`: `start`, then each `seed`, then the agent (the fake claude answers), then `stop`, all logged in that order; `start`'s group is gone afterwards.
  - `the_port_placeholder_gets_a_free_port_as_PORT`.
  - `a_start_that_exits_fails_fast_with_its_output`: fails within 5 s, with the outcome naming `start exited` and its last line.
  - `an_app_that_never_answers_times_out`: `ready_timeout: 2`.
  - `a_failing_seed_ends_the_run_and_stops_the_app`.
  - `a_cancelled_run_stops_the_apps_process_group`.
  - `a_second_run_without_a_port_placeholder_waits_its_turn`: the second is refused with `another run is using <url>`.
  - `a_missing_sibling_repo_is_named_with_its_clone_command`: `space.toml` lists `../web`, which doesn't exist. The run fails before `start`, naming `../web` and `git clone <git> ../web`.
  - `a_log_outside_the_space_is_refused` (in `tests/check.rs`).
  - `a_check_reads_logs_relative_to_the_meta_repo` (in `tests/check.rs`).
- [ ] **Step 2: Run.** Expected: they fail.
- [ ] **Step 3: Implement.**
  - In `run`, on `GuideMode::Commands`:
    1. Check `space.toml`'s repos are all there, failing with each missing one's clone command.
    2. Take the space lock unless `url` has `{port}`. The lock is a file in the system temp folder, named by a hash of the meta repo's path, so nothing is written beside the repos.
    3. Pick the port.
    4. Spawn `sh -c <start>` with `PORT`, `current_dir(meta)`, `process_group(0)`, and output to `.app.log`; record its pgid.
    5. Poll `ready` with a TCP connect, then a minimal `GET <path>` with any HTTP status as success, every 500 ms, failing fast if `start` has exited.
    6. Run the seeds.
    7. Continue into today's agent part with `base_url`.
  - Teardown always runs `stop`, then SIGTERMs and, after 5 s, SIGKILLs the group, then releases the lock.
  - The `work` branch is untouched.
- [ ] **Step 4: Run** the whole suite. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): run any project's app from its run guide`.

### Task 3: Automatic checks wait for the pull

**Files:**
- `tools/intents/src/progress.rs`:
  - `pr_info` asks for `state,title,mergeCommit` and returns the merge commit;
  - the PR search's `--json` adds `mergeCommit`;
  - `FoundPr.merge_commit: Option<String>`;
  - `Evidence.awaiting_pull: bool` and the card label.
- `tools/intents/src/app.rs`: `pr_merge_commits: HashMap<(PathBuf, u64), String>` filled from both sources; the gate in `check_due`.
- Test: `tests/fixtures/fake-gh.sh` gains `mergeCommit`, plus app tests in `tests/progress.rs`.

**Interfaces:**
- Consumes: Task 1's guide mode for the intent's space.
- Produces:
  - `progress::pr_info(...) -> (PrState, Option<String> /* title */, Option<String> /* merge commit */)`, with every caller updated;
  - `App::pulled(&self, it: &Intent, set: &[u64]) -> bool`.

- [ ] **Step 1: Write the failing tests.** Each uses a temporary git repo as the intent's repo, with a commit `c1` on a side branch:
  - `an_automatic_check_waits_until_the_merge_is_pulled`: the fake gh reports PR 42 merged with `mergeCommit = c1`, `HEAD` lacks `c1`, and the space is in commands mode. No check starts, the card reads `waiting for a pull`, and `checked_for` is unchanged. Merging `c1` into `HEAD` and polling again starts the check.
  - `a_merge_commit_not_yet_known_also_waits`.
  - `x_by_hand_is_not_gated`.
  - `work_mode_spaces_are_not_gated` (skoolscout).
- [ ] **Step 2: Run.** Expected: they fail.
- [ ] **Step 3: Implement.** In `check_due`, when the intent's guide is in commands mode and `!self.pulled(it, &it.fm.reconciled_for)`, skip it without adding to `check_tried`, and mark its evidence `awaiting_pull`. `pulled` runs `git -C <intent repo> merge-base --is-ancestor <sha> HEAD` for each merge commit; an unknown commit means not yet.
- [ ] **Step 4: Run** the whole suite. Expected: PASS.
- [ ] **Step 5: Commit** `feat(intents): in other projects an automatic check waits for the pull`.

### Task 4: README and the whole suite

**Files:** `tools/intents/README.md`: a "Spaces and the run guide" section before "Evidence for bugs", with the `space.toml` and `run.md` examples from the spec, the pointer file, the lookup order and the pull wait. The skoolscout notes in "Evidence for bugs" and "Completion check" say they are the `work` mode.

- [ ] **Step 1: Write the section.**
- [ ] **Step 2: Run** `cargo test -q --release --workspace </dev/null && cargo fmt --all --check && cargo clippy -q --release --workspace --all-targets -- -D warnings && sh scripts/test-install.sh </dev/null`. Expected: all clean.
- [ ] **Step 3: Commit** `docs(intents): spaces and the run guide`.
