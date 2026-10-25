# herdr-toolbar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Herdr plugin that opens a right-hand pane with a vertical, single-select activity bar of tools, each hosted in its own pseudo-terminal that keeps running while another is shown.

**Architecture:** One Rust binary, `herdr-toolbar`, with three modes: the default TUI (ratatui + tui-term + portable-pty), `toggle` (opens or closes the workspace's toolbar pane through the Herdr CLI), and `select <tool>` (tells the running toolbar to switch, over a Unix socket in the plugin state dir). Config is TOML merged from a global file and an optional per-worktree file.

**Tech Stack:** Rust 1.97 (2024 edition), `ratatui 0.30`, `crossterm 0.29`, `tui-term 0.3.4` (default `vt100` feature, re-exports `vt100 0.16`), `portable-pty 0.9`, `serde` + `toml`, `anyhow`, `shellexpand`, `serde_json`. Tests: `cargo test`, `insta` for buffer snapshots.

**Spec:** `docs/superpowers/specs/2026-10-24-herdr-toolbar-design.md` (same repo). Read it first; this plan argues from it.

## Global Constraints

- Repo is `jefelabs/herdr-plugins`, a Cargo workspace; this plugin lives at `toolbar/`, crate and binary both named `herdr-toolbar`.
- Plugin id `jefelabs.toolbar`, `min_herdr_version = "0.9.0"`, `platforms = ["macos", "linux"]`. Windows out of scope.
- A workspace build puts binaries in `<repo>/target/release/`, not `toolbar/target/`. The manifest's `command` paths must reflect where Herdr runs the command from (plugin dir = `toolbar/`), so they are `../target/release/herdr-toolbar`.
- Tools are never written here. The toolbar runs argv commands from config.
- Hosted tools inherit `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_ENV`, `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID`, `HERDR_PANE_ID` and get `HERDR_TOOLBAR_TOOL_ID=<id>`. Their `TERM` is `xterm-256color`; `TERM_PROGRAM`, `KITTY_WINDOW_ID` and `WEZTERM_*` are removed (text-only v1).
- Tools start in `workspace_cwd` from `HERDR_PLUGIN_CONTEXT_JSON`, unless the entry sets `cwd`.
- Activity bar is exactly 3 columns wide on the right edge; content area is the rest.
- Default in-pane prefix `Ctrl-]`; defaults after prefix: `j`/`k` next/previous, `1`..`9` nth, `r` reload, `x` restart, `q` quit, `Ctrl-]` literal.
- Lazy start: a tool's process starts the first time it is selected.
- Invalid config never kills a running toolbar; the error shows in the content area and the previous config stays.
- No orphans: on quit, SIGHUP/SIGTERM or pane close, every child gets SIGHUP, then SIGKILL after 500 ms.
- Commit after every task with a conventional-commit message; never commit `target/`.

## Review Focus

Five spec-implied conditions no single task's happy path covers. Each has its test pinned to the owning task below.

1. **Missing tool binary** (`command = ["definitely-not-installed"]`): the content area shows "cannot start `<name>`: <os error>" and the other tools still work. Task 4.
2. **Config reload with a broken file**: after `r` with a TOML syntax error, the bar keeps the old tools and the content area shows the parse error with line number. Task 2 (merge) and Task 6 (reload path).
3. **Prefix pressed twice** sends one literal `0x1d` to the tool and leaves prefix mode. Task 3.
4. **Resize while a background tool is running**: the hidden tool's pty and parser both get the new size, so it is correct when shown. Task 4.
5. **Tool exits while hidden**: its icon dims; selecting it shows the last screen plus the footer; Enter restarts it in the same cwd. Task 6.

---

## File Structure

```
herdr-plugins/
  Cargo.toml                      workspace: members = ["toolbar"]
  .gitignore                      target/, .superpowers/
  README.md                       what lives here, how to link a plugin
  docs/superpowers/{specs,plans}/
  toolbar/
    Cargo.toml                    package herdr-toolbar
    herdr-plugin.toml             Herdr manifest
    README.md                     install, config.toml keybinding, tools.toml example
    tools.example.toml            the spec's five-tool example
    src/
      main.rs                     arg parsing: (none) | toggle | select <tool>
      config.rs                   ToolConfig, Config, load_merged()
      keys.rs                     Prefix state machine + key_to_bytes()
      pty.rs                      ToolProcess: spawn/read/resize/kill, ToolState
      ui.rs                       layout + render(bar, content, footer)
      app.rs                      App: event loop, selection, reload, shutdown
      herdr.rs                    HerdrContext from env, toggle(), control socket
    tests/
      spawn.rs                    real pty integration test
      toggle.rs                   toggle against a fake `herdr` script
```

---

### Task 1: Repository, workspace and crate skeleton

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `README.md`, `toolbar/Cargo.toml`, `toolbar/herdr-plugin.toml`, `toolbar/src/main.rs`
- Move in: `docs/superpowers/specs/2026-10-24-herdr-toolbar-design.md` and this plan already exist in the folder; commit them.

**Interfaces:**
- Produces: binary `herdr-toolbar` that prints `herdr-toolbar 0.1.0` for `--version` and exits 2 with usage for an unknown subcommand.

- [ ] **Step 1: Initialise the repo**

```bash
cd ~/Development/Workspaces/jefelabs/herdr-plugins
git init -b main
```

- [ ] **Step 2: Write the workspace `Cargo.toml`**

`[workspace] members = ["toolbar"]`, `resolver = "3"`, and `[workspace.package]` with `edition = "2024"`, `license = "MIT"`, `repository = "https://github.com/jefelabs/herdr-plugins"`.

- [ ] **Step 3: Write `toolbar/Cargo.toml`**

Package `herdr-toolbar`, version `0.1.0`, inherits workspace fields. Dependencies: `ratatui = "0.30"`, `crossterm = "0.29"`, `tui-term = "0.3.4"`, `portable-pty = "0.9"`, `serde = { version = "1", features = ["derive"] }`, `toml = "0.9"`, `serde_json = "1"`, `anyhow = "1"`, `shellexpand = "3"`, `nix = { version = "0.30", features = ["signal"] }`. Dev-dependencies: `insta = "1"`, `tempfile = "3"`.

- [ ] **Step 4: Write the failing test for argument handling**

In `toolbar/src/main.rs`, module `tests`:

```rust
#[test]
fn parse_mode_from_args() {
    assert_eq!(parse_mode(&[]), Mode::Tui);
    assert_eq!(parse_mode(&["toggle".into()]), Mode::Toggle);
    assert_eq!(parse_mode(&["select".into(), "db".into()]), Mode::Select("db".into()));
    assert!(matches!(parse_mode(&["bogus".into()]), Mode::Usage(_)));
}
```

- [ ] **Step 5: Run it and watch it fail**

Run: `cargo test -p herdr-toolbar parse_mode_from_args`
Expected: compile error, `parse_mode` not found.

- [ ] **Step 6: Implement `enum Mode { Tui, Toggle, Select(String), Usage(String) }` and `fn parse_mode(args: &[String]) -> Mode`**

`main` matches on the mode; `Tui`, `Toggle` and `Select` call stubs that return `anyhow::Result<()>` and currently `todo!()`-free `Ok(())`. `--version` prints `herdr-toolbar <CARGO_PKG_VERSION>`.

- [ ] **Step 7: Run tests and build**

Run: `cargo test -p herdr-toolbar && cargo build --release -p herdr-toolbar && ../target/release/herdr-toolbar --version` from `toolbar/`
Expected: tests pass; prints `herdr-toolbar 0.1.0`.

- [ ] **Step 8: Write `toolbar/herdr-plugin.toml`**

Exactly the manifest in the spec's "Herdr manifest" section with these corrections: `[[build]] command = ["cargo", "build", "--release", "-p", "herdr-toolbar"]`, pane and action `command` paths `["../target/release/herdr-toolbar", ...]`, pane `placement = "split"`.

- [ ] **Step 9: Write `.gitignore` (`target/`, `.superpowers/`) and root `README.md`** (three lines: what the repo is, `herdr plugin link <checkout>/toolbar`, pointer to `toolbar/README.md`).

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore README.md toolbar docs
git commit -m "feat(toolbar): workspace, crate skeleton and Herdr manifest"
```

---

### Task 2: Config loading and merge

**Files:**
- Create: `toolbar/src/config.rs`, `toolbar/tools.example.toml`
- Modify: `toolbar/src/main.rs` (add `mod config;`)

**Interfaces:**
- Produces:
  ```rust
  pub struct ToolConfig { pub id: String, pub name: String, pub icon: String,
      pub command: Vec<String>, pub cwd: Option<String>, pub env: BTreeMap<String, String> }
  pub struct Config { pub prefix: String, pub tools: Vec<ToolConfig> }   // tools already ordered
  pub fn load_merged(global: &Path, workspace_root: &Path) -> anyhow::Result<Config>
  pub fn parse(text: &str) -> anyhow::Result<RawConfig>                   // one file
  ```
  `Config::default()` has `prefix = "ctrl-]"` and no tools.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn parses_the_spec_example() {
    let raw = parse(include_str!("../tools.example.toml")).unwrap();
    assert_eq!(raw.tool.len(), 5);
    assert_eq!(raw.tool[2].id, "db");
    assert_eq!(raw.tool[4].env["JIRA_PROJECT"], "SKOOL");
    assert_eq!(raw.order.as_deref(), Some(&["issues","todo","db","files","jira"].map(String::from)[..]));
}
#[test] fn workspace_entry_replaces_global_by_id_and_keeps_position() { /* global 5 tools; workspace redefines "db" with a different command; merged[2].command changed, len still 5 */ }
#[test] fn workspace_order_wins() { /* workspace file has order = ["files","db"]; merged ids start with files, db, then the rest in global order */ }
#[test] fn missing_workspace_file_is_fine() { /* only global present → same as global */ }
#[test] fn syntax_error_reports_line() { let e = parse("[[tool]\nid=1").unwrap_err().to_string(); assert!(e.contains("line 1")); }
#[test] fn tool_without_command_is_an_error_naming_the_id() { /* "tool `todo` has no command" */ }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p herdr-toolbar config::`
Expected: compile errors for missing items.

- [ ] **Step 3: Implement `config.rs`**

`RawConfig { prefix: Option<String>, order: Option<Vec<String>>, tool: Vec<ToolConfig> }` with serde. Merge: start with global tools in order; for each workspace tool, replace the same-id entry in place or append; apply `order` (workspace's if present, else global's): listed ids first in that order, unlisted after in existing order; unknown ids in `order` are an error naming the id. `cwd` is `shellexpand::tilde`d at load. Workspace file name is `.herdr-toolbar.toml`.

- [ ] **Step 4: Write `toolbar/tools.example.toml`** with the spec's five entries verbatim.

- [ ] **Step 5: Run tests**

Run: `cargo test -p herdr-toolbar config::`
Expected: 6 passed.

- [ ] **Step 6: Commit**

```bash
git add toolbar/src/config.rs toolbar/src/main.rs toolbar/tools.example.toml
git commit -m "feat(toolbar): load and merge global and workspace tool config"
```

---

### Task 3: Key prefix state machine and key encoding

**Files:**
- Create: `toolbar/src/keys.rs`
- Modify: `toolbar/src/main.rs` (`mod keys;`)

**Interfaces:**
- Produces:
  ```rust
  pub enum Action { Forward(Vec<u8>), Select(usize), Next, Prev, Reload, Restart, Quit, Ignore }
  pub struct Prefix { armed: bool, chord: KeyChord }
  impl Prefix { pub fn new(prefix: &str) -> anyhow::Result<Self>; pub fn handle(&mut self, key: KeyEvent) -> Action; pub fn armed(&self) -> bool }
  pub fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>>
  ```
  `KeyChord` parses `"ctrl-]"`, `"ctrl-b"`, `"alt-t"`; anything else is an error.

- [ ] **Step 1: Write the failing tests**

```rust
fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent { KeyEvent::new(code, m) }
#[test] fn plain_keys_forward() { assert!(matches!(p().handle(k(Char('a'), NONE)), Action::Forward(b) if b == b"a")); }
#[test] fn prefix_then_digit_selects_zero_based() { let mut p = p(); p.handle(k(Char(']'), CONTROL)); assert!(matches!(p.handle(k(Char('3'), NONE)), Action::Select(2))); assert!(!p.armed()); }
#[test] fn prefix_twice_sends_literal_0x1d() { let mut p = p(); p.handle(k(Char(']'), CONTROL)); assert!(matches!(p.handle(k(Char(']'), CONTROL)), Action::Forward(b) if b == vec![0x1d])); }
#[test] fn prefix_then_unknown_is_ignored_and_disarms() { /* Action::Ignore, armed() == false */ }
#[test] fn jk_r_x_q_map() { /* Next, Prev, Reload, Restart, Quit */ }
#[test] fn encodes_arrows_and_ctrl_chars() { assert_eq!(key_to_bytes(&k(Up, NONE)), Some(vec![27,91,65])); assert_eq!(key_to_bytes(&k(Char('c'), CONTROL)), Some(vec![3])); assert_eq!(key_to_bytes(&k(Char('x'), ALT)), Some(vec![27, b'x'])); assert_eq!(key_to_bytes(&k(F(5), NONE)), Some(b"\x1b[15~".to_vec())); }
#[test] fn bad_prefix_string_is_an_error() { assert!(Prefix::new("super-]").is_err()); }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p herdr-toolbar keys::` — Expected: compile errors.

- [ ] **Step 3: Implement `keys.rs`**

Encoding table follows the tui-term `smux.rs` example for chars, Ctrl, arrows, Home/End/PgUp/PgDn/Tab/BackTab/Delete/Insert/Esc/Backspace (`0x7f` for Backspace, not 8), plus Alt as ESC-prefix, F1–F12 (`\x1bOP`..`\x1bOS`, `\x1b[15~`..`\x1b[24~`), and Enter as `\r`. Unicode chars encode as UTF-8.

- [ ] **Step 4: Run tests** — Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add toolbar/src/keys.rs toolbar/src/main.rs
git commit -m "feat(toolbar): prefix chords and key-to-bytes encoding"
```

---

### Task 4: Pseudo-terminal host for one tool

**Files:**
- Create: `toolbar/src/pty.rs`, `toolbar/tests/spawn.rs`
- Modify: `toolbar/src/main.rs` (`mod pty;`)

**Interfaces:**
- Consumes: `ToolConfig` (Task 2).
- Produces:
  ```rust
  pub struct Size { pub rows: u16, pub cols: u16 }
  pub enum ToolState { NotStarted, Running, Exited { code: Option<i32> }, Failed(String) }
  pub struct ToolProcess { /* parser: Arc<RwLock<vt100::Parser>>, master, writer, child handle, exited flag */ }
  pub struct SpawnEnv { pub cwd: PathBuf, pub inherit: Vec<(String, String)>, pub tool_id: String }
  impl ToolProcess {
      pub fn spawn(cfg: &ToolConfig, env: &SpawnEnv, size: Size, wake: Sender<()>) -> Result<Self, String>;
      pub fn screen(&self) -> impl Deref<Target = vt100::Screen> + '_;   // read lock
      pub fn write(&self, bytes: &[u8]) -> anyhow::Result<()>;
      pub fn resize(&self, size: Size);
      pub fn state(&self) -> ToolState;
      pub fn kill(&mut self);          // SIGHUP, wait 500 ms, SIGKILL
  }
  ```
  `wake` is sent `()` after every chunk of pty output so the UI redraws.

- [ ] **Step 1: Write the failing integration tests** in `toolbar/tests/spawn.rs`

```rust
fn cfg(cmd: &[&str]) -> ToolConfig { /* id "t", name "T", icon "x", command, cwd None, env {} */ }
fn wait_until(f: impl Fn() -> bool) { /* poll 10 ms up to 3 s, panic on timeout */ }
#[test] fn renders_output_into_the_screen() {
    let (tx, _rx) = channel();
    let p = ToolProcess::spawn(&cfg(&["sh","-c","printf 'hello toolbar'; sleep 30"]), &env(), Size{rows:5,cols:40}, tx).unwrap();
    wait_until(|| p.screen().contents().contains("hello toolbar"));
}
#[test] fn exit_code_is_captured() { /* sh -c 'exit 3' → state() == Exited{code:Some(3)} */ }
#[test] fn missing_binary_is_failed_with_os_error() { let e = ToolProcess::spawn(&cfg(&["definitely-not-installed-xyz"]), ...).unwrap_err(); assert!(e.contains("definitely-not-installed-xyz")); }
#[test] fn resize_reaches_pty_and_parser() { /* sh -c 'stty size; sleep 30' after resize(Size{rows:7,cols:33}) shows "7 33"; screen().size() == (7,33) */ }
#[test] fn env_passthrough_and_tool_id() { /* sh -c 'echo $HERDR_TOOLBAR_TOOL_ID $HERDR_BIN_PATH $TERM; sleep 30' with inherit [("HERDR_BIN_PATH","/x/herdr")] → "t /x/herdr xterm-256color" */ }
#[test] fn kill_leaves_no_child() { /* spawn sleep 60; kill(); the child's pid no longer exists (kill(pid,0) fails) within 1 s */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p herdr-toolbar --test spawn` — Expected: compile errors.

- [ ] **Step 3: Implement `pty.rs`**

`native_pty_system().openpty(PtySize{rows, cols, pixel_width:0, pixel_height:0})`; `CommandBuilder::new(argv[0])` + `args`, `cwd`, `env_clear()` then re-add `PATH`, `HOME`, `USER`, `SHELL`, `LANG`, `LC_*`, the `inherit` list, the config `env`, `TERM=xterm-256color`, `HERDR_TOOLBAR_TOOL_ID`; never set `TERM_PROGRAM`, `KITTY_WINDOW_ID`, `WEZTERM_*`. Reader thread: `try_clone_reader()`, 8 KiB buffer, `parser.write().process(&buf[..n])`, `wake.send(())`. Waiter thread: `child.wait()` → store exit code. `vt100::Parser::new(rows, cols, 0)`. Spawn errors map to `Err(format!("cannot start `{}`: {}", cfg.name, e))`. `kill` uses `child.process_id()` and `nix::sys::signal::kill`.

- [ ] **Step 4: Run tests** — Expected: 6 passed. (If `stty size` is flaky under CI, accept the parser size assertion alone and note it.)

- [ ] **Step 5: Commit**

```bash
git add toolbar/src/pty.rs toolbar/src/main.rs toolbar/tests/spawn.rs
git commit -m "feat(toolbar): host a tool in a pseudo-terminal with resize and clean kill"
```

---

### Task 5: Rendering the bar and the content area

**Files:**
- Create: `toolbar/src/ui.rs`, snapshots under `toolbar/src/snapshots/`
- Modify: `toolbar/src/main.rs` (`mod ui;`)

**Interfaces:**
- Consumes: `ToolConfig`, `ToolState`, `vt100::Screen`.
- Produces:
  ```rust
  pub struct BarItem<'a> { pub icon: &'a str, pub name: &'a str, pub state: &'a ToolState, pub selected: bool }
  pub struct Frame<'a> { pub items: Vec<BarItem<'a>>, pub screen: Option<&'a vt100::Screen>, pub message: Option<&'a str>, pub footer: Option<&'a str>, pub prefix_armed: bool }
  pub fn split(area: Rect) -> (Rect, Rect)                  // (content, bar); bar.width == 3, bar.x == area.right()-3
  pub fn render(frame: &Frame, area: Rect, buf: &mut Buffer)
  ```

- [ ] **Step 1: Write the failing tests** (ratatui `TestBackend` 40x8, `insta::assert_snapshot!(terminal.backend())`)

```rust
#[test] fn split_reserves_three_columns_on_the_right() { let (c,b) = split(Rect::new(0,0,40,8)); assert_eq!(b, Rect::new(37,0,3,8)); assert_eq!(c, Rect::new(0,0,37,8)); }
#[test] fn bar_shows_icons_selected_highlighted_exited_dimmed() { /* 3 items, second selected, third Exited; snapshot; also assert the cell at (38,1) has Modifier::REVERSED and (38,2) has Modifier::DIM */ }
#[test] fn message_replaces_screen() { /* screen None, message "cannot start `Todo`: No such file" → snapshot contains it, wrapped */ }
#[test] fn footer_overlays_last_content_row() { /* footer "exited (code 3), enter to restart" appears on row 7 cols 0..37 */ }
#[test] fn prefix_armed_marks_bar_bottom() { /* a "^]" marker in the bar's last row when prefix_armed */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p herdr-toolbar ui::` — Expected: compile errors.

- [ ] **Step 3: Implement `ui.rs`**

Bar: one icon per row starting at row 0, centred in 3 columns; selected uses `Modifier::REVERSED`, `Exited`/`Failed` use `Modifier::DIM`; `NotStarted` plain. Content: `PseudoTerminal::new(screen)` from `tui_term::widget`, cursor hidden when `prefix_armed`; `message` as a wrapped `Paragraph` when `screen` is `None`; `footer` painted over the last content row with `Modifier::REVERSED`. Reload glyph `⟳` in the bar's last row when not armed, `^]` when armed.

- [ ] **Step 4: Run tests, accept snapshots after eyeballing** — `cargo insta test -p herdr-toolbar --accept` once, then `cargo test -p herdr-toolbar ui::` — Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add toolbar/src/ui.rs toolbar/src/snapshots toolbar/src/main.rs
git commit -m "feat(toolbar): render activity bar, hosted screen, messages and footer"
```

---

### Task 6: The app loop

**Files:**
- Create: `toolbar/src/app.rs`
- Modify: `toolbar/src/main.rs` (`mod app;`, `Mode::Tui` calls `app::run`)

**Interfaces:**
- Consumes: everything above.
- Produces:
  ```rust
  pub struct Slot { pub cfg: ToolConfig, pub proc: Option<ToolProcess>, pub last_error: Option<String> }
  pub struct App { pub slots: Vec<Slot>, pub selected: usize, pub prefix: Prefix, pub message: Option<String>, pub size: Size, env: SpawnEnv, paths: ConfigPaths, wake: (Sender<()>, Receiver<()>) }
  impl App {
      pub fn new(cfg: Config, env: SpawnEnv, paths: ConfigPaths, size: Size) -> anyhow::Result<Self>;
      pub fn select(&mut self, idx: usize);                 // lazy spawn; clamps idx
      pub fn apply_key(&mut self, key: KeyEvent) -> Flow;   // Flow::{Continue, Quit}
      pub fn reload(&mut self);                             // keeps old config on error, sets message
      pub fn restart_selected(&mut self);
      pub fn resize(&mut self, size: Size);                 // every Running slot
      pub fn frame(&self) -> ui::Frame<'_>;
      pub fn shutdown(&mut self);                           // kill all
  }
  pub fn run(paths: ConfigPaths, env: SpawnEnv) -> anyhow::Result<()>  // crossterm raw mode, event loop, SIGTERM/SIGHUP → shutdown
  pub struct ConfigPaths { pub global: PathBuf, pub workspace_root: PathBuf }
  ```

- [ ] **Step 1: Write the failing tests** (unit tests in `app.rs`, using `sh -c` tools and a temp config dir)

```rust
#[test] fn first_tool_starts_on_new_others_lazy() { /* 3 tools; after new(): slots[0].proc Some, slots[1..].proc None */ }
#[test] fn select_spawns_once_and_keeps_background_running() { /* select(1) spawns; select(0); select(1) again → same process (compare a pid or a counter), state Running */ }
#[test] fn reload_with_broken_file_keeps_tools_and_sets_message() { /* write "[[tool]" to global; reload(); slots.len unchanged; message contains "line 1" */ }
#[test] fn reload_with_new_tool_appends_without_restarting_running_ones() { /* add a 4th tool; slots[0].proc still the same pid; slots.len()==4 */ }
#[test] fn exited_hidden_tool_dims_and_enter_restarts_it() { /* slot 1 = sh -c 'exit 3'; select(1), wait Exited; select(0); frame().items[1].state is Exited; select(1); apply_key(Enter) → slot 1 Running again */ }
#[test] fn resize_fans_out_to_hidden_slots() { /* two running; resize(Size{rows:9,cols:50}); both screen().size() == (9,50) */ }
#[test] fn quit_kills_everything() { /* two running; shutdown(); both pids gone */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p herdr-toolbar app::` — Expected: compile errors.

- [ ] **Step 3: Implement `app.rs`**

Content size is `ui::split(area).0` minus one row when a footer is showing. `apply_key`: `Prefix::handle` → `Forward` writes to the selected `Running` process, or if the selected slot is `Exited`/`Failed` and the key is Enter, restarts it; `Select(i)`/`Next`/`Prev` call `select`; `Reload`, `Restart`, `Quit` as named. `run`: `ratatui::init()`, `crossterm::event::poll(16 ms)` interleaved with draining `wake.1`, redraw on either, `Event::Resize` → `resize`; install `nix` signal handler for SIGTERM/SIGHUP setting an `AtomicBool` the loop checks; always `ratatui::restore()` and `shutdown()` on exit, including panic (hook).

- [ ] **Step 4: Run all tests** — `cargo test -p herdr-toolbar` — Expected: all pass.

- [ ] **Step 5: Manual smoke** from `toolbar/`:

```bash
mkdir -p /tmp/tb && cp tools.example.toml /tmp/tb/tools.toml
HERDR_PLUGIN_CONFIG_DIR=/tmp/tb HERDR_PLUGIN_CONTEXT_JSON='{"workspace_cwd":"'$PWD'"}' cargo run -p herdr-toolbar
```
Expected: bar with five icons on the right; first tool (or "cannot start `GitHub issues`: ..." if `gh dash` is absent) in the content area; `Ctrl-]` `2`..`5` switch; `Ctrl-]` `q` exits and leaves no `sleep`/tool processes (`pgrep -f lazysql` empty).

- [ ] **Step 6: Commit**

```bash
git add toolbar/src/app.rs toolbar/src/main.rs
git commit -m "feat(toolbar): event loop with lazy start, reload, restart and clean shutdown"
```

---

### Task 7: Herdr integration: context, toggle and select

**Files:**
- Create: `toolbar/src/herdr.rs`, `toolbar/tests/toggle.rs`, `toolbar/tests/fixtures/fake-herdr.sh`
- Modify: `toolbar/src/main.rs` (wire `Mode::Toggle`, `Mode::Select`, build `ConfigPaths`/`SpawnEnv` from `HerdrContext`), `toolbar/src/app.rs` (accept control-socket commands)

**Interfaces:**
- Produces:
  ```rust
  pub struct HerdrContext { pub bin: PathBuf, pub workspace_id: Option<String>, pub pane_id: Option<String>, pub workspace_cwd: PathBuf, pub config_dir: PathBuf, pub state_dir: PathBuf, pub inherit: Vec<(String,String)> }
  impl HerdrContext { pub fn from_env() -> anyhow::Result<Self> }   // reads HERDR_* and HERDR_PLUGIN_CONTEXT_JSON{workspace_cwd}
  pub fn pane_file(ctx: &HerdrContext) -> PathBuf        // <state_dir>/<workspace_id>.pane  (holds the toolbar's pane id)
  pub fn socket_path(ctx: &HerdrContext) -> PathBuf      // <state_dir>/<workspace_id>.sock
  pub fn toggle(ctx: &HerdrContext) -> anyhow::Result<()>
  pub fn select(ctx: &HerdrContext, tool: &str) -> anyhow::Result<()>   // writes "select <tool>\n" to the socket
  pub struct Control { /* UnixListener, non-blocking */ }
  impl Control { pub fn bind(path: &Path) -> anyhow::Result<Self>; pub fn poll(&self) -> Vec<ControlCmd> }
  pub enum ControlCmd { Select(String) }
  ```

- [ ] **Step 1: Write `tests/fixtures/fake-herdr.sh`**

A shell script that appends `"$@"` to `$FAKE_HERDR_LOG`, and: for `pane get <id>` exits 0 if `$FAKE_PANE_EXISTS=1` else 1; for `plugin pane open ...` prints `{"result":{"pane":{"pane_id":"w1:p9"}}}`; otherwise exits 0.

- [ ] **Step 2: Write the failing tests** in `tests/toggle.rs`

```rust
#[test] fn toggle_opens_when_no_pane_recorded() { /* empty state dir → log contains "plugin pane open --plugin jefelabs.toolbar --entrypoint toolbar --placement split --direction right --cwd <ws>" and no "--focus"; pane file now holds "w1:p9" */ }
#[test] fn toggle_closes_when_pane_recorded_and_alive() { /* pane file "w1:p4", FAKE_PANE_EXISTS=1 → log contains "pane close w1:p4"; pane file removed */ }
#[test] fn toggle_reopens_when_recorded_pane_is_gone() { /* pane file "w1:p4", FAKE_PANE_EXISTS=0 → "plugin pane open" logged */ }
#[test] fn select_reaches_a_listening_control_socket() { /* Control::bind(tmp.sock); select(ctx,"db"); poll() == [Select("db")] */ }
#[test] fn from_env_reads_context_json() { /* HERDR_PLUGIN_CONTEXT_JSON={"workspace_cwd":"/w"} → workspace_cwd == /w; inherit contains HERDR_BIN_PATH */ }
```

- [ ] **Step 3: Run to verify failure** — `cargo test -p herdr-toolbar --test toggle` — Expected: compile errors.

- [ ] **Step 4: Implement `herdr.rs` and wire `main.rs`**

`toggle`: read pane file; if present and `<bin> pane get <id>` succeeds → `<bin> pane close <id>`, remove file; else run `plugin pane open` as in the test, parse `.result.pane.pane_id` from stdout, write the pane file. Then, when a `--ratio`/`resize` form is available on this Herdr (check `herdr pane resize --help` at build time and record the exact flag in the code comment), set the new pane to about 35% of the tab width; if no such flag exists, skip and document. The TUI on start writes its own `HERDR_PANE_ID` to the pane file (overriding the toggle's value) and binds `Control`; `App::run` polls it each loop and maps `Select(id)` to the slot index (numeric string = 1-based index). On shutdown remove the socket and the pane file. `select` with no socket prints "toolbar is not open in this workspace" and exits 1.

- [ ] **Step 5: Run all tests** — `cargo test -p herdr-toolbar` — Expected: all pass.

- [ ] **Step 6: Manual check inside Herdr** (from a Herdr pane in any workspace):

```bash
cargo build --release -p herdr-toolbar
herdr plugin link ~/Development/Workspaces/jefelabs/herdr-plugins/toolbar
cp toolbar/tools.example.toml "$(herdr plugin config-dir jefelabs.toolbar)/tools.toml"
herdr plugin action invoke --plugin jefelabs.toolbar --action toggle   # or: ../target/release/herdr-toolbar toggle
```
Expected: a split appears on the right with the bar; focus stays in the calling pane; a second toggle closes it. `herdr-toolbar select 3` from the agent pane switches the bar without moving focus.

- [ ] **Step 7: Commit**

```bash
git add toolbar/src/herdr.rs toolbar/src/app.rs toolbar/src/main.rs toolbar/tests
git commit -m "feat(toolbar): Herdr context, toggle action and select over a control socket"
```

---

### Task 8: README, manual checklist and 0.1.0

**Files:**
- Create: `toolbar/README.md`
- Modify: `toolbar/herdr-plugin.toml` (version stays 0.1.0; confirm paths), root `README.md`

- [ ] **Step 1: Write `toolbar/README.md`**

Sections: what it is (two sentences), install (`herdr plugin install jefelabs/herdr-plugins/toolbar` once public; `herdr plugin link` for now; requires `cargo`), the `config.toml` snippet from the spec binding `prefix+t` to `jefelabs.toolbar.toggle` plus `prefix+1`..`5` to `jefelabs.toolbar.select N`, `tools.toml` example (include the five entries), per-worktree `.herdr-toolbar.toml` override example, keys table, "text-only in v1: images are dropped, tools fall back to text", troubleshooting ("cannot start" = binary not on PATH).

- [ ] **Step 2: Run the manual checklist and record results in the commit message**

With `lazysql`, `yazi` and `gh dash` installed (`brew install lazysql yazi gh && gh extension install dlvhdr/gh-dash`): each renders without garbled box drawing; arrows, Enter, Esc, `Ctrl-c` reach the tool; `Ctrl-]` `]` sends a literal `Ctrl-]`; resizing the Herdr pane redraws the hidden tools correctly when shown; quitting leaves `pgrep -f 'lazysql|yazi|gh-dash'` empty.

- [ ] **Step 3: Full verification**

Run: `cargo fmt --check && cargo clippy -p herdr-toolbar -- -D warnings && cargo test -p herdr-toolbar && cargo build --release -p herdr-toolbar`
Expected: all clean.

- [ ] **Step 4: Commit and tag**

```bash
git add toolbar/README.md README.md
git commit -m "docs(toolbar): README with install, keybindings and tools.toml; 0.1.0 checklist"
git tag toolbar-v0.1.0
```

Pushing to GitHub and creating `jefelabs/herdr-plugins` is the owner's call (private vs public decides the marketplace topic).

---

## Self-review notes

- Spec coverage: layout A (T5), bar visible while tools run (T4/T5), background tools keep running (T6), workspace binding and `workspace_cwd` (T7), Herdr env passthrough (T4/T7), text-only TERM (T4), two-layer config (T2), lazy start (T6), toggle + stays in tab (T7), select without focus (T7), prefix keys (T3), exit footer and restart (T5/T6), no orphans (T4/T6), monorepo + Cargo workspace paths (T1), README keybinding instructions (T8). Width ratio after open is best-effort pending the exact Herdr flag; recorded in T7 step 4.
- Names used across tasks: `ToolConfig`, `Config`, `load_merged`, `Prefix::handle`, `Action`, `key_to_bytes`, `ToolProcess::{spawn,screen,write,resize,state,kill}`, `ToolState`, `Size`, `SpawnEnv`, `ui::{split,render,Frame,BarItem}`, `App::{select,apply_key,reload,restart_selected,resize,frame,shutdown}`, `HerdrContext`, `toggle`, `select`, `Control`, `ControlCmd`. Checked consistent.
- Review Focus items 1–5 are pinned to T4 (missing binary, resize fan-out), T2+T6 (broken reload), T3 (prefix twice), T6 (exited hidden tool).
