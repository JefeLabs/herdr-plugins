# Toolbar Setup Walk-through Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `install.sh` walks a teammate through choosing from the owner's toolbar tools, installs the missing programs (Homebrew, then mise, then curl), and writes `tools.toml`.

**Architecture:** The walk-through is POSIX `sh` inside `install.sh`, which already detects the OS and architecture and owns the pinned downloads. It runs after the plugins are installed and after `--tools`.

The menu is a catalog shipped in the release archive (`toolbar/setup/`):
- a tool list;
- a program list;
- one `[[tool]]` block per tool, with placeholders.

Questions read from `/dev/tty`, or from a file named by `HERDR_PLUGINS_TTY` for tests. `HERDR_PLUGINS_TTY=none` forces the no-terminal path.

**Tech Stack:** POSIX `sh` (`set -eu`), `scripts/test-install.sh` (sh, offline, fake `herdr`/`brew`/`mise`/`gh`), Rust workspace unchanged.

**Spec:** `docs/superpowers/specs/2026-10-30-toolbar-setup-walkthrough-design.md`

## Global Constraints

- POSIX `sh` with `set -eu`. It must run on macOS and Linux (glibc and musl), arm64 and x86_64.
- **No sudo.** Write only under `$HOME`, plus whatever Homebrew does in its own prefix.
- **Pinning:** the installer's own downloads are pinned, and each must match its SHA-256 or it isn't installed. Homebrew, mise, `gh extension install` and terminal-browser's own script are each tool's own channel, and are not pinned by us.
- **Never touch an existing `tools.toml`** without `--setup`. `--setup` backs the old file up to `tools.toml.bak-<YYYYmmdd-HHMMSS>` first.
- **No terminal** (`HERDR_PLUGINS_TTY=none`, or no readable `/dev/tty`): no questions, no installs. Write the ticked tools whose programs are present.
- **Signing in is deferred (owner).**
  - GitHub issues needs `gh auth status` to succeed; otherwise it is left off with `GitHub issues: run gh auth login, then install.sh --setup to add it`.
  - Jira needs a jira-cli config file; otherwise it is left off with `Jira: run jira init, then install.sh --setup to add it`.
- **Settings and order are the owner's:**
  - `prefix = "ctrl-]"` and `bar = "large"`;
  - catalog order: intents, issues, db, files, jira, web, editor, browser, usage, terminal.
- **Ticked by default:** everything except `jira` and `web`.
- **Icons:** the spec's icon table, written into `tools.toml` as TOML escape text. Nerd glyphs are pre-selected when `TERM` is `xterm-ghostty` or `xterm-kitty`, when `TERM_PROGRAM` is `ghostty`, or when `GHOSTTY_RESOURCES_DIR` or `KITTY_WINDOW_ID` is set.
- **Nothing personal anywhere in the catalog or example** (`JIRA_PROJECT`, `SKOOL`, `/Users/`):
  - Intents uses `$dir/target/release/herdr-intents`;
  - the Usage disk pane uses `HERDR_PLUGINS_REPOS`, else `$HOME`;
  - the Usage process pane uses `Virtualization` on macOS and `containerd` on Linux.
- **`--tools`:** still installs its six pinned programs, and no longer appends tools to `tools.toml`.
- **Tests:** `sh scripts/test-install.sh </dev/null` never calls the real Homebrew, mise, gh or network. The existing `run()` sets `HERDR_PLUGINS_TTY=none`.
- **Escapes:** write files that hold icon escapes (`toolbar/setup/tools`, `toolbar/tools.example.toml`) with a script, not an editor tool that decodes `\u` escapes. Check them with `grep -c '\\u'`.
- Commit per task. No push.

## Review Focus

1. **`curl … | sh` is interactive.** Every question must read the tty's file descriptor (`/dev/tty`), never stdin, which is the script itself. Tests use `HERDR_PLUGINS_TTY`, so only reading the code proves this. The expected result: questions work when the script is piped.
2. **A program installed outside the installing shell's `PATH`** (mise shims, `/usr/local/bin` Homebrew on Intel) still counts as present. The tool is added, and the `PATH` line to add is printed. It is not left off. Test: `a_mise_tool_off_path_is_added_with_a_path_hint` (Task 3).
3. **Junk answers in the list** (`x`, `42`, `1 1`) are ignored with a one-line note, the list is shown again, and an empty line ends it. Test: `junk_answers_are_ignored` (Task 2).
4. **One failed install** (`brew` exits 1): that tool is left off with the reason, the rest carry on, `tools.toml` is still written, and the install doesn't fail for it. Test: `a_failed_brew_install_leaves_that_tool_off` (Task 3).
5. **`--setup` twice in one minute** never overwrites an earlier backup. The backup name has seconds, plus a counter if that name is taken. Test: `setup_twice_keeps_both_backups` (Task 2).

---

### Task 1: The catalog and the no-terminal setup

**Files:**
- Create: `toolbar/setup/tools`, `toolbar/setup/programs`, `toolbar/setup/<id>.toml` (10 files)
- Modify: `install.sh` (remove the first-config write at "a first toolbar config" and the Usage/Editor appends under `--tools`; add `--setup` parsing, `setup_toolbar`, called after the `--tools` block)
- Modify: `scripts/dist.sh` (copy `toolbar/setup/` into the archive), `toolbar/tools.example.toml`
- Test: `scripts/test-install.sh`

**Interfaces:**
- Produces:
  - **`toolbar/setup/tools`:** one line per tool in catalog order, `id|name|summary|ticked(1/0)|needs (space-separated program names)|nerd escape|text icon`. Lines starting with `#` are comments.
  - **`toolbar/setup/programs`:** `program|check|brew|mise|curl`.
    - `check` is the command looked for (`gh-dash` is checked with `gh extension list`).
    - `brew` is a formula, or `-`.
    - `mise` is a mise tool name or `github:<owner>/<repo>`, or `-`.
    - `curl` is `pinned` (a row in `builtin_tools`), `script:<url>`, `gh-ext:<owner>/<repo>`, or `-`.
  - **`toolbar/setup/<id>.toml`:** that tool's `[[tool]]` block with `@ICON@`, `@INTENTS@`, `@REPOS@` and `@VM@`.
  - **`setup_toolbar`** in `install.sh`. It reads the catalog from `$dir/toolbar/setup`, and `$config/tools.toml` is its output.

- [ ] **Step 1: Write the failing tests** in `scripts/test-install.sh`:
  - **Stage the catalog:** `cp -R toolbar/setup "$stage/toolbar/"`.
  - **`run()`:** passes `HERDR_PLUGINS_TTY="${TTY:-none}"`, plus `TERM`, `GHOSTTY_RESOURCES_DIR` and `KITTY_WINDOW_ID` when the caller sets them.
  - **Test 17, `no terminal: the default bar from the catalog`.** Fresh home, `--tools`, `TTY=none`. The `tools.toml` has:
    - `bar = "large"` and `prefix = "ctrl-]"`;
    - `order` equal to `["intents", "db", "files", "editor", "usage", "terminal"]` (the ticked tools whose programs `--tools` installed);
    - no `id = "jira"`, no `id = "web"`, no `id = "issues"`;
    - Intents' command pointing at `$h/.local/share/herdr-plugins/target/release/herdr-intents`.

    The output contains `left off: GitHub issues (needs gh, gh-dash)`.
  - **Test 18, `no terminal: Nerd icons under Ghostty`.** Same, with `TERM=xterm-ghostty`. The `tools.toml` has Intents' Nerd escape as text: `grep -F` for the escape from `toolbar/setup/tools`. Without `TERM`, it has `✎`.
  - **Test 19, `nothing personal in the catalog`.** `! grep -rqE 'JIRA_PROJECT|SKOOL|/Users/' toolbar/setup toolbar/tools.example.toml`.

  Existing tests 1, 2, 5, 6, 9, 10 and 11 keep their assertions. Usage and Editor now come from the catalog, with the same OS word, the repos folder and `exec nvim --listen`.
- [ ] **Step 2: Run** `sh scripts/test-install.sh </dev/null`. Expected: 17, 18 and 19 fail (no catalog yet); the rest as before.
- [ ] **Step 3: Implement.**
  - Write the catalog from the owner's `tools.toml` and the spec's tables (script-written, escapes as text). Jira has no `env`. Terminal's text icon is `▣`, its command `["clear"]`, `open = "pane"`.
  - `setup_toolbar` (no-terminal path):
    1. Read the tools.
    2. Keep the ticked ones whose `needs` are all present: `command -v`, or the file exists in `$HERDR_PLUGINS_BIN` / `~/.local/bin`, or `gh extension list` lists `gh-dash`.
    3. Print `left off: <name> (needs <missing>)` for the rest.
    4. Pick the icon mode by the detection rule.
    5. Write the header and each block with placeholders replaced, using `sed` with `|` delimiters and escaped values.
  - Call it only when `$config/tools.toml` doesn't exist.
  - `dist.sh` copies `toolbar/setup`.
  - `tools.example.toml` becomes the catalog's ticked set with text icons, `bar = "large"`, the owner's order, Terminal added, and no `JIRA_PROJECT`.
- [ ] **Step 4: Run** `sh scripts/test-install.sh </dev/null`. Expected: `all install tests passed`.
- [ ] **Step 5: Commit** `feat(install): write the toolbar from a catalog of the owner's tools`.

### Task 2: The walk-through: list, icons, `--setup`

**Files:** Modify `install.sh`. Test: `scripts/test-install.sh`.

**Interfaces:**
- Consumes: Task 1's catalog and `setup_toolbar`.
- Produces:
  - `ask` reads one line from the tty, or from `$HERDR_PLUGINS_TTY` (a file read line by line across questions, through one file descriptor opened once).
  - `--setup` is a flag.

- [ ] **Step 1: Write the failing tests** (answers in a file, `TTY=<file>`):
  - **Test 20, `Enter takes the defaults`.** Answers: an empty line (list), then an empty line (icons). There's no install question until Task 3. The result is the same as test 17's order. The output shows the list with `[x] 1  Intents` and `[ ] 5  Jira`.
  - **Test 21, `numbers switch tools`.** Answers `4`, then an empty line, and so on. Files is unticked, so `order` has no `files`, and the list is shown again with `[ ] 4  Files`.
  - **Test 22, `junk_answers_are_ignored`.** Answers `x`, `42`, `1 1`, then an empty line. Each gets `ignored: <answer>`. The result is the defaults (`1 1` switches Intents off and on again).
  - **Test 23, `the icon answer wins`.** `TERM=xterm-ghostty`, icons answered `n`: the text icons are written.
  - **Test 24, `an existing tools.toml is left alone`.** Run without `--setup` on a home that already has `tools.toml`. The file is byte-identical (`cmp`), and no `which tools` prompt appears in the output.
  - **Test 25, `setup_twice_keeps_both_backups`.** Two `--setup` runs: two distinct `tools.toml.bak-*` files exist, the first holding the original content.
  - **Test 26, `--setup needs a terminal`.** `--setup` with `TTY=none` exits 1 with `--setup needs a terminal to ask in`.
- [ ] **Step 2: Run.** Expected: 20–26 fail.
- [ ] **Step 3: Implement.**
  - **The list:**
    - print `[x]`/`[ ]`, the number, the name, the summary, and `needs:` with the missing programs only;
    - read numbers separated by spaces and toggle each;
    - ignore anything that isn't a number 1–N, with a note;
    - repeat until an empty line.
  - **Then:** the install question (Task 3 fills it in: for now, with nothing missing, ask nothing), then `Nerd Font icons? [Y/n]` with the detected default (`[y/N]` when not detected), then write.
  - **`--setup`:** an interactive check first, then the backup (seconds, plus `-2`, `-3`… if the name is taken).
- [ ] **Step 4: Run.** Expected: `all install tests passed`.
- [ ] **Step 5: Commit** `feat(install): walk teammates through choosing their toolbar tools`.

### Task 3: Installing what's missing: Homebrew, then mise, then pinned downloads

**Files:** Modify `install.sh` (`install_pinned`, made from the `--tools` loop body; `gh` and `jira` rows in `builtin_tools`; the install question), `toolbar/setup/programs`. Test: `scripts/test-install.sh`.

**Interfaces:**
- Consumes: Task 2's `ask` and the list result.
- Produces: `install_pinned <name>`, which returns 0 when installed. The `--tools` loop uses it unchanged.

- [ ] **Step 1: Write the failing tests.** First, insert the install answer `n` after the list answers in tests 20–23's answer files. They tick tools whose programs are missing (GitHub issues, Browser), so the install question now comes before the icons question. The fakes are scripts in a per-test folder put first on `PATH`:
  - **fake `brew`:** logs `$*` to `$HOME/.brew-log`. For `install <formula>` it writes a fake program (named by the formula's `check` from the programs list) into a folder that is on `PATH`, and exits `$FAKE_BREW_STATUS` (default 0).
  - **fake `mise`:** logs, and for `use -g <x>` writes the program into `$HOME/.local/share/mise/shims/`.
  - **fake `gh`:** `auth status` exits `$FAKE_GH_AUTH` (default 1). `extension install` logs and records `gh-dash`; `extension list` prints it.

  The tests:
  - **Test 27, `Homebrew installs what is missing`.** With brew present, tick Jira (`5`). The answer to `Install what's missing with Homebrew? …` is `y`. `.brew-log` has `install ankitpokhrel/jira-cli/jira-cli`, and `jira` has a config file in the test home, so Jira is added.
  - **Test 28, `no means left off`.** The same with `n`: nothing in `.brew-log`, and no `id = "jira"`.
  - **Test 29, `a_failed_brew_install_leaves_that_tool_off`.** `FAKE_BREW_STATUS=1`. The output says `left off: Jira (jira-cli did not install)`, `tools.toml` is written, and the exit status is 0.
  - **Test 30, `mise when there is no Homebrew`.** `.mise-log` has `use -g github:ankitpokhrel/jira-cli`, or the confirmed mise name.
  - **Test 31, `a_mise_tool_off_path_is_added_with_a_path_hint`.** The shims folder isn't on `PATH`. Jira is still added, and the output has `$HOME/.local/share/mise/shims is not on your PATH`.
  - **Test 32, `pinned download when neither is there`.** Fake `gh` and `jira` archives in the manifest. `$h/.local/bin/jira` is installed from the manifest row, and a wrong SHA-256 refuses it (reusing `write_manifest`'s bad-row option).
- [ ] **Step 2: Run.** Expected: 27–32 fail.
- [ ] **Step 3: Implement.**
  1. Extract `install_pinned` from the `--tools` loop, which then calls it.
  2. Add `gh` and `jira` rows for `linux-x86_64`, `linux-arm64`, `macos-arm64` and `macos-x86_64`. Pin the current stable releases of `cli/cli` and `ankitpokhrel/jira-cli`, and take each asset's SHA-256 from GitHub's recorded digest (`gh api repos/<owner>/<repo>/releases/tags/<tag> --jq '.assets[] | [.name, .digest] | @tsv'`). Ledger the versions.
  3. **The installer:**
     - Homebrew if `command -v brew`, else mise if `command -v mise`, else curl.
     - Ask once: `Install what's missing with <Homebrew|mise|downloads>? <programs> [Y/n]`.
     - Install each with that tool's command (or `install_pinned`).
     - Recheck, counting a program as present on `PATH`, in `~/.local/bin`, in mise's shims, or in `brew --prefix`'s `bin`.
     - Print the `PATH` line for any folder not on `PATH`.
     - Leave off with the reason any tool whose program is still missing.
  4. **mise names:** verify with `mise registry` on a machine that has mise. If none is available, use `github:` for all of them and ledger that the names are unverified.
- [ ] **Step 4: Run.** Expected: `all install tests passed`.
- [ ] **Step 5: Commit** `feat(install): install a chosen tool's programs with Homebrew, mise or a pinned download`.

### Task 4: gh dash, Jira and terminal-browser

**Files:** Modify `install.sh`, `toolbar/setup/programs`. Test: `scripts/test-install.sh`.

- [ ] **Step 1: Check two facts** and ledger them:
  - Download `https://terminal-browser.sh/install` into `target/probe/` and read it (don't run it). Record its shell (`bash` or `sh`), whether it uses sudo, where it writes, and whether it supports Linux x86_64.
  - Run `brew info --json=v2 terminal-browser` and compare its homepage and tap with the script's project. If it's the same app, put its formula in the programs list; otherwise `-`.
  - Also confirm jira-cli's config path, `${JIRA_CONFIG_FILE:-$HOME/.config/.jira/.config.yml}`, from its README.
- [ ] **Step 2: Write the failing tests:**
  - **Test 33, `GitHub issues waits for sign-in`.** Fake `gh` present, `FAKE_GH_AUTH=1`. There is no `id = "issues"`; the output has `GitHub issues: run gh auth login, then install.sh --setup to add it`; and `.gh-log` has no `extension install`.
  - **Test 34, `signed in, gh dash is installed`.** `FAKE_GH_AUTH=0`, install answered `y`. `.gh-log` has `extension install dlvhdr/gh-dash`, and `id = "issues"` is written.
  - **Test 35, `Jira waits for jira init`.** Fake `jira` present, no config file. There is no `id = "jira"`, and the output has `Jira: run jira init, then install.sh --setup to add it`.
  - **Test 36, `terminal-browser from its own script`.** No brew formula (or brew absent). `HERDR_PLUGINS_TB_INSTALL=file://<fake script>`, where the fake writes `$HOME/.local/bin/terminal-browser`. `id = "browser"` is written. With `--from` and no override, the output says `terminal-browser: needs the network (its own installer)` and Browser is left off.
- [ ] **Step 3: Run.** Expected: 33–36 fail.
- [ ] **Step 4: Implement:**
  - the `gh auth status` gate before `gh extension install dlvhdr/gh-dash`;
  - the Jira config check;
  - the script install: `curl -fsSL "${HERDR_PLUGINS_TB_INSTALL:-https://terminal-browser.sh/install}" | <the shell step 1 found>`, skipped under `--from` unless the override is set;
  - the next-steps lines at the end.
- [ ] **Step 5: Run.** Expected: `all install tests passed`.
- [ ] **Step 6: Commit** `feat(install): gh dash, Jira and terminal-browser in the walk-through`.

### Task 5: README and the whole suite

**Files:** `README.md` (the install section), `toolbar/README.md` (first run).

- [ ] **Step 1: Rewrite the install section.**
  - Cover the walk-through, Homebrew then mise then downloads, `--setup`, the no-terminal path and sign-in later.
  - `--tools` now only installs programs.
  - Remove the "adds a Usage tool / Editor tool" sentences.
  - `toolbar/README.md`: the installer writes `tools.toml` through the walk-through, and the example is for setting up by hand.
- [ ] **Step 2: Run** `sh scripts/test-install.sh </dev/null && cargo test -q --release --workspace </dev/null && cargo fmt --all --check && cargo clippy -q --release --workspace --all-targets -- -D warnings`, plus `shellcheck install.sh scripts/test-install.sh` if `shellcheck` is installed. Expected: all clean.
- [ ] **Step 3: Commit** `docs: the toolbar setup walk-through`.
