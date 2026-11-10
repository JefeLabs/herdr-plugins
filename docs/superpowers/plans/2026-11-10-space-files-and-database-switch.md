# Space files library and the database switch: implementation plan (1 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the code that reads and writes the space files into a shared library. Then make `database = false` in a space's `space.toml` hide the Database tool on that space's toolbar, including on machines that already have a `tools.toml`.

**Architecture:**
- A new library crate, `herdr-space` (`lib/space`), takes over the space-file code from `tools/intents/src/space.rs`. Pointer functions take the config folder (`&Path`) instead of Intents' `Dirs`. Intents' `space.rs` becomes thin wrappers, so none of its callers change.
- Writes to `spaces.toml` move to `toml_edit`, so comments and unknown keys survive.
- `herdr-intents context` gains a `database` value (`"on"`/`"off"`).
- The toolbar gains a per-tool `when = "<name>"`: a tool whose named context value is `"off"` moves out of the bar into a hidden list, and moves back when the value changes.
- The installer adds `when = "database"` to the Database entry of an existing `tools.toml`.

**Tech Stack:** Rust 2024 workspace (ratatui 0.30, serde, toml 1, toml_edit, anyhow, shellexpand), POSIX sh for `install.sh` and `scripts/test-install.sh`.

**Spec:** `docs/superpowers/specs/2026-11-10-herdr-spaces-design.md`. This plan covers these sections: "The database switch", "A shared library for the space files", "Hiding the Database tool when the switch is off", and the database half of "An existing `tools.toml` is patched in place". Plan 2 (the `herdr-spaces` tool, local changes and push mode) and plan 3 (new GitHub repos and PR mode) build on it.

## Global Constraints

- Work in a git worktree of herdr-plugins on branch `feature/space-files-database`, cut from `origin/main`. Never work in the owner's main checkout: it's behind `origin/main` and holds untracked specs.
- Before every commit: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` all pass. Shell changes also pass `sh scripts/test-install.sh` and `shellcheck install.sh scripts/test-install.sh`.
- Commit only the paths each task names. Never `--no-verify`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- No push, PR, version bump or release without the owner's word. The owner merges.
- `herdr-intents`' behaviour is unchanged apart from the new `database` context value. Its existing tests stay as they are and pass.
- Copy follows the repo's voice: plain sentences, and every refusal or error says why.
- Spec values used here, verbatim:
  - The key in `space.toml` is `database = true/false`.
  - With no `database` key, the database is on when `run.md` has `db:`, off otherwise.
  - The tool field is `when = "database"`. A missing key means shown.
  - The installer keeps a `.bak`, prints what it changed, and touches nothing else.
  - It runs once per change, so a second install changes nothing.

## Review Focus

1. **A `space.toml` with only `database = false` and no `repos`.** Today `repos` is required, so the whole file fails to parse. Expected: it reads as a space with no repos and the database off. Pinned by Task 3 `database_only_file_reads_with_no_repos`.
2. **A `space.toml` that doesn't parse** (for example `database = "no"`). Expected: `context` still prints, with `database` `"on"` (the Database tool stays) and a `database_error` saying why. Pinned by Task 4 `broken_space_file_keeps_the_database_tool_and_says_why`.
3. **A new toolbar with an older `herdr-intents`** that prints no `database` value. Expected: the Database tool shows. Pinned by Task 5 `a_tool_whose_value_is_missing_is_shown`.
4. **The selected tool is hidden by a workspace switch.** Expected:
   - The selection moves to the first shown tool, and the hidden tool's process keeps running.
   - When the value comes back `"on"`, the tool returns to its place in the bar.
   - Pinned by Task 5 `hiding_the_selected_tool_moves_the_selection_and_keeps_it_running`.
5. **An existing `tools.toml` whose Database entry already has `when`, or that has no Database entry.** Expected: the file is untouched and no `.bak` is made. Pinned by Task 6 cases 4 and 5.

---

### Task 1: The `herdr-space` library takes over the space-file code

**Files:**
- Create: `lib/space/Cargo.toml`, `lib/space/src/lib.rs` (the space file and naming), `lib/space/src/pointers.rs` (`spaces.toml`)
- Modify: `Cargo.toml` (workspace `members` adds `"lib/space"`), `tools/intents/Cargo.toml` (`herdr-space = { path = "../../lib/space" }`), `tools/intents/src/space.rs` (becomes wrappers), `tools/intents/src/scope.rs` (`group_name` re-exported from the library)
- Test: `lib/space/src/lib.rs` and `lib/space/src/pointers.rs` (`#[cfg(test)] mod tests`, with `tempfile` as a dev-dependency)

**Interfaces:**
- Produces, in `herdr_space` (moved as they are, with the same doc comments):
  - Types: `Space { meta: PathBuf, repos: Vec<SpaceRepo> }`, `SpaceRepo { path, rel, git }` with `clone_hint()`, `INTENTS_DIR`.
  - Path and naming: `plain(&Path) -> PathBuf`, `git_slug(&str) -> Option<String>`, `target_slug(&str) -> Option<String>`, `repo_name(&str) -> Option<String>`, `origin_slug(&Path) -> Option<String>`, `group_name(&str) -> Option<String>` (moved from `scope.rs`).
  - Reading: `read_space(&Path) -> anyhow::Result<Space>`, `missing(&Space) -> Vec<&SpaceRepo>`, `is_meta(&Path) -> bool`, `space_name(&Path) -> Result<String, String>`, `intents_space(&Path) -> Option<Result<String, String>>`.
- Produces, in `herdr_space::pointers`, each taking `config: &Path` where Intents took `&Dirs`:
  - `pointers_path(config) -> PathBuf`, `current_name(config, name) -> String`, `pointed_at(config, meta) -> Option<String>`, `meta_for(config, name) -> Option<PathBuf>`.
  - `remember_meta(config, name, meta) -> anyhow::Result<()>`, `rename_pointer(config, from, to) -> anyhow::Result<()>`.
  - `enum Storage { Profile, Meta, Remote }`, `storage_for(config, name) -> Result<Storage, String>`.
  - `pointer_metas(config) -> Vec<(String, PathBuf)>`, `other_names(config, meta, name) -> Vec<String>`.
- Stays in `tools/intents/src/space.rs`, with today's `&Dirs` signatures, each calling the library with `&dirs.config`: every function above, plus `storage_block` and `repo_space`, which keep their bodies. `pub use herdr_space::{Space, SpaceRepo, INTENTS_DIR, Storage, …}` keeps `crate::space::X` paths working.

- [ ] **Step 1: Write the library's failing tests.** In `lib/space/src/lib.rs`:
  - `git_slug_reads_every_github_url_form`: `git@github.com:o/R.git`, `https://github.com/o/R`, `ssh://git@github.com/o/R.git` → `Some("o/r")`.
  - `repo_name_keeps_case`: `git@github.com:o/Shop-Meta.git` → `Some("Shop-Meta")`.
  - `space_name_falls_back_to_the_folder_without_origin`: `git init` a temp folder `shop-meta` with no remote → `Ok("shop-meta")`.
  - `read_space_resolves_paths_from_the_meta_repo`: `repos = [{ path = "../api", git = "g" }]` in `<tmp>/m/.helmsmith/space.toml` → `repos[0].path == <tmp>/api` and `rel == "../api"`.

  In `pointers.rs`: `remember_then_meta_for_round_trips`. It calls `remember_meta(cfg, "shop", "/x/shop-meta")`; then `meta_for(cfg, "SHOP")` is `Some("/x/shop-meta")`; and `pointers_path(cfg) == cfg.join("spaces.toml")`.
- [ ] **Step 2: Run them and watch them fail.** Run: `cargo test -p herdr-space`. Expected: FAIL (the crate or its functions don't exist yet).
- [ ] **Step 3: Move the code.**
  - Create the crate with Intents' versions of `serde`, `toml`, `anyhow` and `shellexpand`.
  - Move the bodies listed under Produces. Replace `crate::scope::group_name` with the moved `group_name`, and `dirs.config` with `config`.
  - Turn Intents' `space.rs` into the wrappers, and add `pub use herdr_space::group_name;` in `scope.rs`.
  - No caller outside `space.rs` and `scope.rs` changes.
- [ ] **Step 4: Run the workspace's tests.** Run: `cargo test --workspace`. Expected: the new library tests pass, and every existing test passes, with the same count as on `origin/main` plus the five new ones.
- [ ] **Step 5: Commit** `Cargo.toml Cargo.lock lib/space tools/intents/Cargo.toml tools/intents/src/space.rs tools/intents/src/scope.rs` as `refactor(space): the space files move into the herdr-space library`.

### Task 2: Writing `spaces.toml` keeps comments and unknown keys

**Files:**
- Modify: `lib/space/Cargo.toml` (add `toml_edit`, in the version that pairs with the workspace's `toml 1`), `lib/space/src/pointers.rs`
- Test: `lib/space/src/pointers.rs`

**Interfaces:**
- Consumes: Task 1's `remember_meta` and `rename_pointer`.
- Produces: the same signatures and the same results as today, now written through `toml_edit::DocumentMut`. Reading stays on `toml` and serde.

- [ ] **Step 1: Write the failing tests.**
  - `remember_meta_keeps_comments_and_unknown_keys`: the file starts as `# mine\n[spaces.shop]\nmeta = "~/a"\nmine = 1\n`. After `remember_meta(cfg, "shop", "/b")`, the text contains `# mine`, `mine = 1` and `meta = "/b"`.
  - `rename_pointer_keeps_comments`: the same file with `rename_pointer(cfg, "shop", "shop-meta")`. The text contains `# mine` and `[spaces.shop-meta]`, `old_names` contains `"shop"`, and `meta = "~/a"` is unchanged.
- [ ] **Step 2: Run them and watch them fail.** Run: `cargo test -p herdr-space pointers`. Expected: FAIL (`# mine` is gone, because today's code rewrites the file with `toml::to_string`).
- [ ] **Step 3: Implement the two writers on `DocumentMut`.**
  - They keep today's rules: the key matched in any case, merged settings, `old_names` without duplicates.
  - They still fail with the file's path when it doesn't parse, and leave it as it is.
  - A new entry is appended at the end of the file.
- [ ] **Step 4: Run the tests.** Run: `cargo test --workspace`. Expected: PASS, including Intents' existing pointer and space-move tests.
- [ ] **Step 5: Commit** `Cargo.lock lib/space` as `fix(space): writing spaces.toml keeps your comments and keys`.

### Task 3: `space.toml` says whether the space uses a database

**Files:**
- Modify: `lib/space/src/lib.rs`
- Test: `lib/space/src/lib.rs`

**Interfaces:**
- Produces:
  - `Space` gains `database: Option<bool>`.
  - `repos` defaults to empty when absent (`#[serde(default)]`).
  - `pub fn database_on(meta: &Path, guide_has_db: bool) -> Result<bool, String>`:
    - with no `space.toml`, `Ok(guide_has_db)`;
    - when the file has `database`, `Ok(value)`;
    - when it has no `database` key, `Ok(guide_has_db)`;
    - when it doesn't parse, `Err("<path>: <why>")`.
  - The caller works out `guide_has_db`, because the run guide belongs to Intents.

- [ ] **Step 1: Write the failing tests.**
  - `database_only_file_reads_with_no_repos`: `database = false` alone gives `read_space` returning `repos` empty and `database == Some(false)`.
  - `database_key_wins_over_the_guide`: `database = false` with `guide_has_db = true` gives `Ok(false)`.
  - `no_key_follows_the_guide`: `repos = []` gives `Ok(true)` for `true` and `Ok(false)` for `false`.
  - `no_space_file_follows_the_guide`: a meta repo with only `.helmsmith/intents/` gives `Ok(true)` for `true`.
  - `a_file_that_does_not_parse_says_so`: `database = "no"` gives an `Err` containing `space.toml`.
- [ ] **Step 2: Run them and watch them fail.** Run: `cargo test -p herdr-space database`. Expected: FAIL (there's no `database_on` and no `database` field, and a file with no `repos` doesn't parse).
- [ ] **Step 3: Implement it** as described under Produces.
- [ ] **Step 4: Run the tests.** Run: `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `lib/space/src/lib.rs` as `feat(space): space.toml can say the space uses no database`.

### Task 4: `herdr-intents context` prints `database`

**Files:**
- Modify: `tools/intents/src/context.rs` (`gather`), `tools/intents/README.md` (the `database` key under "Spaces and the run guide", and the new value in the toolbar's context list)
- Test: `tools/intents/src/context.rs` (new `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 3's `herdr_space::database_on`, and `crate::capture::read_guide(meta)` (`guide.db.is_some()`).
- Produces: when the meta repo is known, `gather`'s map has `"database"` with the value `"on"` or `"off"` (strings, because the toolbar's `Vars` hold only strings).
  - When `database_on` returns `Err(e)`, it puts `"database": "on"` and `"database_error": e`.
  - With no meta repo, there's no `database` key (the existing `meta_error` explains why).

- [ ] **Step 1: Write the failing tests.** Each builds `<tmp>/shop-meta/.helmsmith/` (no git remote, so the space is `shop-meta`), with `Dirs { config: <tmp>/cfg, root: <tmp>/root }`, and calls `gather(&dirs, Some("shop-meta"), Some(&meta), None, DB_TIMEOUT)`.
  - `database_off_in_the_space_file`: `space.toml` is `database = false`, and `run.md` has `db: echo x` → `"off"`.
  - `no_key_and_a_db_line_is_on`: `space.toml` is `repos = []`, and `run.md` has `db: echo x` → `"on"`.
  - `no_key_and_no_db_line_is_off`: `space.toml` is `repos = []`, and there's no `run.md` → `"off"`.
  - `broken_space_file_keeps_the_database_tool_and_says_why`: `space.toml` is `database = "no"` → `"on"`, and `database_error` contains `space.toml`.
  - `no_meta_repo_no_database`: `gather(&dirs, None, None, None, …)` → there's no `database` key.
- [ ] **Step 2: Run them and watch them fail.** Run: `cargo test -p herdr-intents context::tests`. Expected: FAIL (there's no `database` key).
- [ ] **Step 3: Implement it in `gather`,** next to the existing `meta`/`db`/`browse` puts. The guide's read result is already in `guide`: a guide error counts as `guide_has_db = false`.
- [ ] **Step 4: Run the tests.** Run: `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `tools/intents/src/context.rs tools/intents/README.md` as `feat(intents): context says whether the space uses a database`.

### Task 5: A toolbar tool can be hidden by the context (`when`)

**Files:**
- Modify:
  - `toolbar/src/config.rs`: `ToolConfig` gains `#[serde(default)] pub when: Option<String>`.
  - `toolbar/src/context.rs`: add `shown`.
  - `toolbar/src/app.rs`: the slot list.
  - `toolbar/README.md`: a `when` row in the tool fields, with the Database example.
- Test: `toolbar/src/context.rs` and `toolbar/src/app.rs` (their existing test modules)

**Interfaces:**
- Produces:
  - `pub fn shown(tool: &ToolConfig, vars: &Vars) -> bool` in `toolbar/src/context.rs`. It returns `false` only when `tool.when` is `Some(name)` and `vars.get(name)` is exactly `"off"`.
  - **The app (this is the decision, not a suggestion):**
    - `App` keeps today's `slots: Vec<Slot>` as the **shown** tools, in bar order. A new `hidden: Vec<Slot>` holds the others, with their processes left running.
    - A private `fn apply_shown(&mut self)` runs at the end of `apply_context` and of `apply_loaded`. It sorts every slot into shown or hidden, in the config's tool order, keeping `selected` on the same tool id.
    - When that tool is now hidden, `selected` becomes 0.
    - All index-based code (rules, `Next`/`Prev`, clicks, `select_by`, `button_at`) stays as it is, because it only ever sees shown slots.
    - `select_by` on a hidden tool's id returns `false`.

- [ ] **Step 1: Write the failing tests.**
  - In `context.rs`:
    - `a_tool_with_when_is_hidden_only_by_off`: `when = "database"` gives `false` for `{database: "off"}`, and `true` for `{database: "on"}` and for `{}`.
    - `a_tool_whose_value_is_missing_is_shown`: `{db_error: "…"}` without `database` → `true`.
    - `a_tool_without_when_is_always_shown`.
  - In `config.rs`: `when_parses`: a `[[tool]]` with `when = "database"` → `Some("database")`.
  - In `app.rs`, using the existing test helpers, with tools `intents`, `db` (`when = "database"`) and `files`:
    - `database_off_takes_the_tool_off_the_bar`: `apply_context(vars(&[("database","off")]))` → `view().states.len() == 2`, and the names are `intents`, `files`.
    - `hiding_the_selected_tool_moves_the_selection_and_keeps_it_running`: select `db` and start it, then apply `"off"` → `view().selected == 0`, and the `db` slot's process is still alive. Then apply `"on"` → `db` is back at index 1 with the same process.
    - `select_by_a_hidden_tool_is_false`.
    - `clicking_after_a_hidden_tool_selects_the_tool_under_the_mouse`: with `db` hidden, a click on the second button selects `files`.
- [ ] **Step 2: Run them and watch them fail.** Run: `cargo test -p herdr-toolbar`. Expected: FAIL (there's no `when` field and no `shown`, and `db` stays on the bar).
- [ ] **Step 3: Implement `when`, `shown`, `hidden` and `apply_shown`** as described under Produces.
- [ ] **Step 4: Run the tests.** Run: `cargo test --workspace`. Expected: PASS, and the bar snapshot tests are unchanged.
- [ ] **Step 5: Commit** `toolbar/src/config.rs toolbar/src/context.rs toolbar/src/app.rs toolbar/README.md` as `feat(toolbar): a tool can be hidden by the space's context (when)`.

### Task 6: The Database entry carries `when`, including in existing files

**Files:**
- Modify: `toolbar/setup/db.toml` (add `when    = "database"` after `group`, and update its comment), `install.sh` (a patch step beside `rename_in`, run on `"$config/tools.toml"`), `README.md` (one line in the install section: what the installer adds to an existing `tools.toml`)
- Test: `scripts/test-install.sh` (new numbered cases after the last one)

**Interfaces:**
- Consumes: Task 5's `when` field.
- Produces: `install.sh` gains `add_when_database() { # <tools.toml> }`.
  - It acts only when the file has a `[[tool]]` block with `id = "db"` (spacing as the existing `sed` patterns allow) and that block has no `when` line.
  - Then it copies the file to `<file>.bak-$(date +%Y%m%d-%H%M%S)`, the same backup naming as `--setup`, adding `-<n>` when the name exists. It inserts `when    = "database"` right after the block's `id` line.
  - It prints `  Updated <file>: the Database tool now shows only in spaces that use a database (old copy: <bak>)`.
  - It touches nothing else.

- [ ] **Step 1: Write the failing cases in `scripts/test-install.sh`,** in its `pass`/`flunk` style, each in a fresh fake HOME:
  1. **A fresh install** writes a `tools.toml` whose `db` block contains `when    = "database"`.
  2. **An existing file whose `db` block has no `when`** gets the line, a `.bak-*` beside it equal to the old file, and the printed line. Nothing else changes: `diff` of the old and new files is exactly that one added line.
  3. **Running the install again** leaves the file byte-for-byte the same and makes no second `.bak-*`.
  4. **An existing file with no `db` entry** is untouched, with no `.bak-*`.
  5. **A `db` block that already has `when = "database"`** is untouched.
- [ ] **Step 2: Run them and watch them fail.** Run: `sh scripts/test-install.sh`. Expected: the five new cases FAIL, and the existing ones pass.
- [ ] **Step 3: Implement `add_when_database`** and the catalog line. Use awk to track the current `[[tool]]` block. Write through `<file>.tmp` and `mv`, like `rename_in`.
- [ ] **Step 4: Run the checks.** Run: `sh scripts/test-install.sh && shellcheck install.sh scripts/test-install.sh && cargo test --workspace`. Expected: every case `ok`, shellcheck silent, tests pass.
- [ ] **Step 5: Commit** `toolbar/setup/db.toml install.sh scripts/test-install.sh README.md` as `feat(install): the Database tool hides in spaces that use no database`.

---

## After the last task

Run the full gate once more on the branch: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && sh scripts/test-install.sh && shellcheck install.sh scripts/test-install.sh`. Then stop. The owner decides about the push, the PR and the release.

What the owner can try by hand once it's built:
- Add `database = false` to a meta repo's `.helmsmith/space.toml`. The Database icon leaves that space's bar within about a second: the context names the space files in `watch`, and the toolbar asks again when one changes (added in the final review's fix pass).
- Delete the line, and it comes back, because the run guide has `db:`.
