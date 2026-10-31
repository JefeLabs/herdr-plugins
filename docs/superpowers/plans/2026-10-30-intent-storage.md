# Where Intents Are Stored Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Intents live in `~/.helmsmith/intents/spaces/<space>/` by default, today's `~/intents/` moves there once, and each space has a `storage` setting whose `meta` and `remote` values are placeholders.

**Architecture:**
- A new `storage.rs` decides the root at start-up: the new default, plus the one-time move from `~/intents`.
- `space.rs`, which already owns each person's `spaces.toml`, gains the `storage` setting.
- `main.rs` puts both results on `Config`. The app shows a blocked space as empty, says why, and refuses to create anything in it.

**Tech Stack:** Rust 2024, ratatui, serde/toml, nix, tempfile, insta.

**Spec:** `docs/superpowers/specs/2026-10-30-intent-storage-design.md`

## Global Constraints

- Profile storage path: `~/.helmsmith/intents/spaces/<space>/` (owner). A later meta storage would use `<meta repo>/.helmsmith/intents/` (owner: "let keep intents consistent").
- `storage` values: `profile` (the default, also when the line is missing), and `meta` and `remote`. The last two are placeholders that show "meta storage is not built yet" / "remote storage is not built yet" and never fall back to profile.
- `INTENTS_ROOT`, or a `root =` that is neither `~/intents` nor `~/.helmsmith/intents/spaces`, wins, and is never moved.
- Tests use temporary homes and never touch the real `~/intents` or `~/.helmsmith`. Run `cargo test` with `</dev/null`.
- No push, merge or release without the owner's word.

## Review Focus

1. A coding agent sent before the move appends `## Artifacts` lines to the intent's old absolute path, while a check runner writes to its old check folder. Expected: both land in the moved intent through the link at `~/intents`. Test: `the_old_path_still_reaches_the_intents` (Task 1).
2. `INTENTS_ROOT` or a custom `root =` with an old `~/intents` beside it. Expected: nothing moves. Test: `a_custom_root_is_never_moved` (Task 1).
3. Recording the meta repo for a space whose `spaces.toml` entry already has `storage` (and maybe no `meta`). Expected: the storage line survives. Test: `recording_the_meta_repo_keeps_the_storage_line` (Task 2).
4. A `spaces.toml` that doesn't parse. Expected: the space is blocked with the file named, and recording a meta repo never overwrites it. Tests: `a_spaces_file_that_does_not_parse_blocks_and_is_kept` (Task 2).
5. A space name written in a different case in `spaces.toml` from Herdr's workspace (`[spaces.SkoolScout]`). Expected: it still applies, as scoping matches groups case-insensitively. Test: `storage_matches_the_space_name_in_any_case` (Task 2).

---

### Task 1: The new default root and the one-time move

**Files:**
- Create: `tools/intents/src/storage.rs`, `tools/intents/tests/storage.rs`
- Modify: `tools/intents/src/lib.rs` (add `pub mod storage;`), `tools/intents/src/config.rs:88-110` (`resolve_dirs` default root and doc comment), `tools/intents/src/config.rs:489-492` (the old-default check), `tools/intents/src/config.rs:294` (`Config` gains `notice`), `tools/intents/src/main.rs:11-37` (`with_config`), `tools/intents/src/app.rs` (`App::new` shows `cfg.notice`), `tools/intents/defaults/intents.toml:4`, `tools/intents/README.md` (lines 22, 219, 379)

**Interfaces:**
- Produces:
  - `pub const DEFAULT_ROOT: &str = "~/.helmsmith/intents/spaces";` and `pub const OLD_ROOT: &str = "~/intents";` in `storage.rs`.
  - `pub struct Settled { pub root: PathBuf, pub notice: Option<String> }`.
  - `pub fn settle_root(home: &Path, root: &Path) -> Settled`.
  - `Config.notice: Option<String>`, the one-time start-up message.

- [ ] **Step 1: Write the failing tests** in `tests/storage.rs`. Each test builds a temporary `home`, with `new = home/.helmsmith/intents/spaces` and `old = home/intents`.

```rust
#[test] fn old_intents_move_to_the_new_root_once() {
    // old/skoolscout/a.md, old/.polish/x/f, old/.trash/t/from.txt, old/skoolscout/a.assets/evidence/before.json
    let s = settle_root(&home, &new);
    assert_eq!(s.root, new);
    let n = s.notice.unwrap();
    assert!(n.contains(&old.display().to_string()) && n.contains(&new.display().to_string()), "{n}");
    assert!(new.join("skoolscout/a.md").is_file() && new.join(".polish/x/f").is_file());
    assert!(new.join(".trash/t/from.txt").is_file() && new.join("skoolscout/a.assets/evidence/before.json").is_file());
    assert!(!old.exists());
    assert_eq!(settle_root(&home, &new).notice, None, "says it once");
}
#[test] fn nothing_to_move_says_nothing() { assert_eq!(settle_root(&home, &new), Settled { root: new.clone(), notice: None }); }
#[test] fn a_custom_root_is_never_moved() {
    // old/skoolscout/a.md exists; root = home/notes
    let s = settle_root(&home, &home.join("notes"));
    assert_eq!(s, Settled { root: home.join("notes"), notice: None });
    assert!(old.join("skoolscout/a.md").is_file());
}
#[test] fn both_folders_name_both_and_use_the_new() {
    // old/skoolscout/a.md and new/skoolscout/b.md exist
    let s = settle_root(&home, &new);
    assert_eq!(s.root, new);
    let n = s.notice.unwrap();
    assert!(n.contains(&old.display().to_string()) && n.contains(&new.display().to_string()), "{n}");
    assert!(old.join("skoolscout/a.md").is_file(), "nothing moved");
}
#[test] fn the_old_path_still_reaches_the_intents() {
    // old/skoolscout/a.md, old/skoolscout/a.assets/checks/latest.json
    settle_root(&home, &new);
    assert!(fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
    fs::write(old.join("skoolscout/a.assets/checks/latest.json"), "{}").unwrap();
    assert_eq!(fs::read_to_string(new.join("skoolscout/a.assets/checks/latest.json")).unwrap(), "{}");
}
#[test] fn a_link_at_the_old_path_counts_as_moved() {
    // new/skoolscout/a.md exists; old is a symlink to new
    assert_eq!(settle_root(&home, &new), Settled { root: new.clone(), notice: None });
}
#[test] fn a_failed_move_keeps_the_old_folder() {
    // home/.helmsmith exists read-only (0o555); skip when running as root
    let s = settle_root(&home, &new);
    assert_eq!(s.root, old);
    assert!(s.notice.unwrap().contains("could not move"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --test storage </dev/null`
Expected: compile failure, because `herdr_intents::storage` doesn't exist.

- [ ] **Step 3: Implement `settle_root` in `src/storage.rs`**

Order:
1. If `root != home.join(".helmsmith/intents/spaces")`, return `Settled { root, notice: None }`.
2. If `old` is a symlink, or isn't a directory, return the new root with no notice. Use `symlink_metadata`, not `is_dir`, which follows the link.
3. If `new` exists, return the new root with a notice naming both: `"<old> and <new> both hold intents: using <new>; move anything still in <old> by hand"`.
4. Otherwise run `create_dir_all(new.parent())`, then `std::fs::rename(old, new)`, then `std::os::unix::fs::symlink(new, old)`.
   - On success, return the new root with `"moved your intents from <old> to <new> (<old> is now a link to it)"`.
   - If only the link fails, the notice ends `"; could not leave a link at <old>: <error>"`.
   - If the rename fails, return the old root with `"could not move <old> to <new>: <error>; still using <old>"`.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p herdr-intents --test storage </dev/null`
Expected: 7 passed.

- [ ] **Step 5: Change the default root.**
  - **`resolve_dirs`:** the default root becomes `home.join(".helmsmith/intents/spaces")`. Update the doc comment.
  - **`Config` load:** treat `root = "~/intents"` and `root = "~/.helmsmith/intents/spaces"` as the default. Both mean `dirs.root`.
  - **`defaults/intents.toml` line 4:** `root = "~/.helmsmith/intents/spaces"`.
  - **Existing tests:** update the root assertions in `dirs_prefer_env_then_plugin_then_home` and the test near `config.rs:1336` to the new path. Add to that test that `Some("~/.helmsmith/intents/spaces")` also resolves to the default.

- [ ] **Step 6: Wire it in, writing the test first.** In `main.rs` `with_config`, after `load_or_init`:

  ```rust
  let settled = storage::settle_root(&home, &cfg.root);
  cfg.root = settled.root;
  cfg.notice = settled.notice;
  ```

  - `App::new` sets `message` from `cfg.notice` when no other message is set.
  - **The test, written first:** `the_start_up_notice_is_shown` in `app.rs` tests. Set `cfg.notice = Some("moved your intents …")` before `App::new`, then assert `app.message` contains it. Watch it fail, then implement.

- [ ] **Step 7: Update the README.** Replace `~/intents` at lines 22, 219 and 379 with `~/.helmsmith/intents/spaces`. Add one paragraph: "Moved from `~/intents`: the first start after the update moves the folder once and leaves `~/intents` as a link to it, so running checks and agents sent before the move keep working; delete the link when nothing uses it."

- [ ] **Step 8: Run the whole suite**

Run: `cargo test --workspace </dev/null > target/probe/storage-t1.log 2>&1; tail -5 target/probe/storage-t1.log`
Expected: all passed, 0 failed.

- [ ] **Step 9: Commit**

```bash
git add tools/intents/src/storage.rs tools/intents/tests/storage.rs tools/intents/src/lib.rs tools/intents/src/config.rs tools/intents/src/main.rs tools/intents/src/app.rs tools/intents/defaults/intents.toml tools/intents/README.md
git commit -m "feat(intents): intents live in ~/.helmsmith/intents/spaces, moved once from ~/intents"
```

### Task 2: A conversation that can't resume starts fresh

`.polish/` moved, and claude remembers a conversation by the folder it ran in, so conversations started before the move fail to resume.

**Files:**
- Modify: `tools/intents/src/app.rs` (`InterviewScreen` gains `resumed: bool`; `start_interview_turn` near line 3108; the turn-finished handler near line 4392), `tools/intents/tests/fixtures/fake-provider.sh`
- Test: `tools/intents/src/app.rs` tests

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: nothing later tasks use.

- [ ] **Step 1: Give the fake a failing resume.** In `fake-provider.sh`, when `FAKE_RESUME_FAILS=1` and an argument starts with `--resume=`, print `no conversation found` to stderr and exit 1.

- [ ] **Step 2: Write the failing test** in `app.rs` tests, beside `conversation_id_persists_and_resume_argv_is_used_on_second_turn`:

```rust
#[test]
fn a_conversation_that_cannot_resume_starts_fresh_with_its_context() {
    // an interview with conv_id = Some("gone"), the provider env has FAKE_RESUME_FAILS=1;
    // answer the next question and wait for the turn
    let log = provider_log(&fx);
    assert_eq!(log.matches("--resume=gone").count(), 1, "tried once: {log}");
    let fresh = &log[log.rfind("--resume=gone").unwrap()..];
    assert!(fresh.contains("--prompt=") && !fresh.contains("--resume="), "then fresh: {log}");
    assert!(fresh.contains("The author's words:"), "with the whole context: {log}");
    assert!(!matches!(interview(&fx.app).awaiting, Awaiting::Retry(_)), "no error shown");
    assert_eq!(interview(&fx.app).transcript.iter().filter(|(who, t)| who == "agent" && t.is_empty()).count(), 0);
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p herdr-intents --lib cannot_resume </dev/null`
Expected: FAIL, because the interview stops at `Awaiting::Retry("no conversation found…")`.

- [ ] **Step 4: Implement.**
  - `start_interview_turn` sets `st.resumed = resumable`.
  - In the turn-finished handler, a `TurnMsg::Failed` while `st.resumed` does the following:
    1. clears `st.conv_id`;
    2. pops the empty agent entry the failed turn pushed;
    3. calls `start_interview_turn` again with `st.step` and `st.message`, without showing the message twice.
  - The retry isn't resumable, so it can't loop. A second failure shows `Awaiting::Retry` as today.

- [ ] **Step 5: Run it to verify it passes**

Run: `cargo test -p herdr-intents --lib cannot_resume </dev/null`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/intents/src/app.rs tools/intents/tests/fixtures/fake-provider.sh
git commit -m "fix(intents): a conversation that cannot resume starts fresh with its context"
```

### Task 3: The `storage` setting per space, with meta and remote as placeholders

**Files:**
- Modify: `tools/intents/src/space.rs:128-172` (`Pointer`, `remember_meta`, new `Storage`, `storage_for`, `storage_block`), `tools/intents/src/config.rs:294` (`Config.storage_block`), `tools/intents/src/main.rs` (`with_config`), `tools/intents/src/app.rs` (`App::new`, `scan`, the card view's empty text near line 5817, the `a` and `n` keys near line 2180, `app::add`), `tools/intents/README.md` (the Spaces section)
- Test: `tools/intents/tests/space.rs`, `tools/intents/src/app.rs` tests

**Interfaces:**
- Consumes: `Dirs` and `pointers_path(dirs)` from part 1.
- Produces:
  - `pub enum Storage { Profile, Meta, Remote }`.
  - `pub fn storage_for(dirs: &Dirs, name: &str) -> Result<Storage, String>`.
  - `pub fn storage_block(dirs: &Dirs, name: &str) -> Option<String>`.
  - `Config.storage_block: Option<String>`.

- [ ] **Step 1: Write the failing tests** in `tests/space.rs`:

```rust
#[test] fn storage_defaults_to_profile() { /* no spaces.toml; then an entry with only meta */ assert_eq!(storage_for(&dirs, "skoolscout"), Ok(Storage::Profile)); }
#[test] fn meta_and_remote_are_placeholders() {
    // [spaces.a] storage = "meta"   [spaces.b] storage = "remote"  url = "https://intents.example.com"
    assert_eq!(storage_for(&dirs, "a"), Ok(Storage::Meta));
    assert!(storage_block(&dirs, "a").unwrap().contains("meta storage is not built yet"));
    assert!(storage_block(&dirs, "b").unwrap().contains("remote storage is not built yet"));
    assert_eq!(storage_block(&dirs, "c"), None);
}
#[test] fn an_unknown_storage_word_names_the_file() {
    // [spaces.a] storage = "cloud"
    let e = storage_for(&dirs, "a").unwrap_err();
    assert!(e.contains("spaces.toml") && e.contains("cloud") && e.contains("profile, meta or remote"), "{e}");
}
#[test] fn recording_the_meta_repo_keeps_the_storage_line() {
    // [spaces.a] storage = "remote"  url = "https://intents.example.com"   (no meta)
    remember_meta(&dirs, "a", Path::new("/w/a-meta")).unwrap();
    assert_eq!(meta_for(&dirs, "a"), Some(PathBuf::from("/w/a-meta")));
    assert_eq!(storage_for(&dirs, "a"), Ok(Storage::Remote));
    assert!(fs::read_to_string(pointers_path(&dirs)).unwrap().contains("https://intents.example.com"));
}
#[test] fn a_spaces_file_that_does_not_parse_blocks_and_is_kept() {
    // spaces.toml = "[spaces.a\nstorage = "
    assert!(storage_block(&dirs, "a").unwrap().contains("spaces.toml"));
    assert!(remember_meta(&dirs, "a", Path::new("/w/x")).is_err());
    assert_eq!(fs::read_to_string(pointers_path(&dirs)).unwrap(), "[spaces.a\nstorage = ");
}
#[test] fn storage_matches_the_space_name_in_any_case() {
    // [spaces.SkoolScout] storage = "meta"
    assert_eq!(storage_for(&dirs, "skoolscout"), Ok(Storage::Meta));
}
```

In `app.rs` tests, a fixture variant sets `cfg.scope = Some("skoolscout")` and `cfg.storage_block = Some("meta storage is not built yet …")` before `App::new`, with an intent at `root/skoolscout/x.md`:

```rust
#[test] fn a_blocked_space_shows_why_and_no_intents() {
    assert!(app.message.as_deref().unwrap().contains("meta storage is not built yet"));
    assert!(intents_in(&app.tree).is_empty());
    // card view drawn to a buffer shows the block text where "no intents yet" would be
}
#[test] fn a_blocked_space_creates_nothing() {
    // `a` then typing and saving, and `n` then a group name: no new file or folder under root
    assert_eq!(md_files(app.store.root()), before);
    assert!(app.message.as_deref().unwrap().contains("not built yet"));
}
#[test] fn the_whole_tree_leaves_out_a_blocked_space() {
    // cfg.scope = None; spaces.toml [spaces.alpha] storage = "remote"; groups alpha/ and beta/ hold intents
    assert!(top_level_names(&app.tree).iter().all(|n| n != "alpha"));
    assert!(top_level_names(&app.tree).contains(&"beta".to_string()));
    assert!(app.message.as_deref().unwrap().contains("alpha"));
}
#[test] fn add_from_the_popup_is_refused_in_a_blocked_space() {
    let e = app::add(cfg_blocked, dirs, Some("x".into()), true, None).unwrap_err();
    assert!(format!("{e:#}").contains("not built yet"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-intents --test space </dev/null` and `cargo test -p herdr-intents --lib blocked </dev/null`
Expected: compile failures, because `Storage`, `storage_for` and `Config.storage_block` don't exist.

- [ ] **Step 3: Implement in `space.rs`.**
  - **`Pointer`:** becomes `{ meta: Option<String>, storage: Option<String>, url: Option<String> }`, each `#[serde(default, skip_serializing_if = "Option::is_none")]`.
  - **Reading the file:** `read_pointers` returns `Result<Pointers, String>`. A missing file is `Ok(default)`, and a parse error is `Err("<path>: <error>")`. `meta_for` keeps returning `None` on an error.
  - **`remember_meta`:** returns the parse error instead of overwriting the file, and changes only the `meta` field of an existing entry.
  - **`storage_for`:** finds the entry by `eq_ignore_ascii_case` on the name. A missing line means `Profile`. An unknown word errors with `"<path>: storage = \"<word>\" for space <name>: use profile, meta or remote"`.
  - **`storage_block`:** for `Meta` returns `"meta storage is not built yet (storage = \"meta\" for space <name> in <path>)"`, for `Remote` the same with "remote", and for `Err(e)` returns `Some(e)`.

- [ ] **Step 4: Implement the wiring.**
  - **`main.rs`:** sets `cfg.storage_block = cfg.scope.as_deref().and_then(|s| space::storage_block(&dirs, s))`.
  - **`App::new` and `App::scan`:** when blocked, use the scoped group of an empty tree, never `Store::scan`. The message is `cfg.storage_block`, which takes precedence over `cfg.notice`.
  - **The whole tree (`cfg.scope` is `None`):** `scan` drops each top-level group whose `space::storage_block(&self.dirs, &group.name)` is `Some`, and sets the message to `"left out: <names>: storage not built yet"`.
  - **The card view's empty text** is the block when there is one.
  - **`a`, `n` and `app::add`:** stop with the block as the message (or the error) before anything is written.

- [ ] **Step 5: Run them to verify they pass**

Run: `cargo test -p herdr-intents --test space </dev/null` and `cargo test -p herdr-intents --lib blocked </dev/null`
Expected: all passed.

- [ ] **Step 6: Update the README.** Under the Spaces section, show `storage = "profile"` beside `meta = …`, and say that `meta` and `remote` are placeholders and what they show.

- [ ] **Step 7: Run the whole suite and the checks**

Run: `cargo test --workspace </dev/null > target/probe/storage-t2.log 2>&1; tail -5 target/probe/storage-t2.log; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --check`
Expected: all passed; clippy and fmt clean.

- [ ] **Step 8: Commit**

```bash
git add tools/intents/src/space.rs tools/intents/src/config.rs tools/intents/src/main.rs tools/intents/src/app.rs tools/intents/tests/space.rs tools/intents/README.md
git commit -m "feat(intents): a storage setting per space; meta and remote are placeholders"
```
