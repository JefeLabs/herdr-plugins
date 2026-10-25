# herdr-toolbar: a right-hand activity bar of tools for Herdr

**Date:** 2026-10-24
**Status:** draft, awaiting owner approval
**Owner:** Edwin Cruz (jefelabs)
**Home:** `jefelabs/herdr-plugins`, a monorepo of the owner's Herdr plugins,
with this plugin at `toolbar/` (decided 2026-10-24). Crate and binary name
`herdr-toolbar`.

## Intent

Herdr has a left sidebar that lists Agents and Spaces. The owner wants the
same thing on the right for *tools*: a persistent pane whose far right edge is
a narrow, single-select activity bar, one icon per tool, and whose remaining
width shows the selected tool. The tools are existing terminal programs (a
GitHub issues browser, a todo app, a database browser, a folder browser, a
Jira browser), chosen in a config file the owner edits, never in code.

The why: while an agent works in the middle pane, the owner wants to glance at
issues, the database or the file tree without leaving the Herdr window,
opening another tab, or losing where they were in each tool.

## Decisions made in the brainstorm

These are the owner's decisions and stand as written.

1. **Layout A.** Activity bar on the far right edge of the right pane, icons
   stacked vertically, exactly one selected at a time. The selected tool fills
   the pane to the bar's left. Chosen over a top tab strip (B) and a
   launcher-list-then-fullscreen pane (C).
2. **The bar stays visible while a tool runs.** Therefore the pane hosts each
   tool inside itself in a pseudo-terminal and paints that tool's screen into
   the content area. The tool never owns the whole Herdr pane.
3. **Tools keep running in the background.** Switching icons changes which
   tool is displayed. A tool you switched away from is exactly where you left
   it when you come back. Nothing restarts on switch.
4. **Bound to the workspace.** The owner's model is one agent per
   workspace, each workspace on its own worktree. The toolbar belongs to the
   workspace: one toolbar, one set of tool processes, and the workspace's
   directory (`workspace_cwd`) is where tools start and where the optional
   `.herdr-toolbar.toml` is read from. (Decided 2026-10-24 after the builder
   check.)
5. **Hosted tools can drive Herdr.** The owner intends to write their own
   issue browser later (GitHub or Jira) that selects an issue and hands it to
   a new agent in a new workspace. The toolbar therefore passes its Herdr
   environment through to every hosted tool, so a tool can call
   `herdr workspace create`, `herdr agent start` and `herdr agent prompt`
   (or `work start` in repos that use workbrokr). The issue tool itself is a
   separate project; the toolbar only has to make it possible. (Decided
   2026-10-24.)
6. **Images: text-only in v1 (designer's call, owner informed).** The
   embedded terminal advertises no graphics support, so tools fall back to
   text cleanly. Passing Kitty graphics through to the content area is the
   first planned follow-up; a markdown reader with images is the motivating
   case. Tools in plain Herdr panes are unaffected.
7. **Rust.** `ratatui` for the UI, `tui-term` for the embedded terminal widget,
   `portable-pty` for the processes. Chosen over TypeScript (OpenTUI + React,
   the owner's existing TUI stack) and Go because tui-term already solves the
   terminal-in-terminal, which is the only hard part. Single static binary,
   no runtime to install.

Verified at design time: `tui-term` 0.3.4 (released 2026-04-07) depends on
`ratatui ^0.30`, `portable-pty ^0.9`, `vt100 ^0.16`, `crossterm ^0.29`.
`ratatui` is at 0.30.2, `portable-pty` at 0.9.0.

## Calls made by the designer (overturnable)

Marked so the owner can check them while reading.

- **Config lives in two layers.** A global list in the plugin's config
  directory (`$HERDR_PLUGIN_CONFIG_DIR/tools.toml`, which
  `herdr plugin config-dir <id>` prints), plus an optional
  `.herdr-toolbar.toml` at the root of the current workspace that adds to or
  overrides entries by `id`. The skoolscout checkout can carry a database tool
  pointing at its slot's Postgres while other repos do not.
- **Tools run in `workspace_cwd`** (read from `HERDR_PLUGIN_CONTEXT_JSON`),
  each entry may override with `cwd`. Herdr also passes `focused_pane_cwd`;
  the toolbar ignores it, by the owner's decision 4. A tab that runs in a
  different worktree than its workspace is outside the owner's intended
  model and gets the workspace's tools.
- **The toolbar pane stays in the tab where it was opened.** With one agent
  per workspace there is normally one tab, so following the active tab buys
  nothing and would need tab-focus event handling. `toggle` from another tab
  of the same workspace closes the existing toolbar rather than opening a
  second one.
- **Opened and closed with one keybinding** (default suggestion `prefix+t`, bound through the plugin
  manifest's `[[keys.command]]`). Toggling closed kills the hosted tools;
  there is no hidden-but-running state in v1 (see Non-goals).
- **Lazy start.** A tool's process starts the first time its icon is selected,
  not when the pane opens. This keeps opening the pane instant and avoids
  five processes nobody asked for.
- **Icons are configured text**, one to two characters (Nerd Font glyph or a
  plain Unicode symbol), with a one-letter fallback derived from the name.
  The design does not depend on a particular font.
- **Monorepo layout (owner's decision).** `jefelabs/herdr-plugins` holds
  every Herdr plugin: `toolbar/` (this), `tools/<name>/` for the owner's
  future tools, `web-broker/` once moved in, a root `Cargo.toml` workspace for
  the Rust crates, and `docs/superpowers/` for all specs and plans. Installed
  with `herdr plugin install jefelabs/herdr-plugins/toolbar`, linked during
  development with `herdr plugin link <checkout>/toolbar`. The wayfinder map
  for the wider "agent cockpit" effort lives in this repo's issues.

## Goals

- Open a right-hand pane with the activity bar and the first configured tool
  in under a second on a warm machine.
- Switch tools with a key and have the previously shown tool resume
  instantly with its screen intact.
- Add, remove or reorder tools by editing TOML only; a reload key re-reads the
  config without restarting the pane.
- Pass keyboard input through to the hosted tool faithfully, including
  arrows, function keys, Ctrl and Alt chords, and bracketed paste, so tools
  like `lazysql`, `yazi` and `gh dash` behave as they do standalone.
- Resize correctly: when Herdr resizes the pane, every hosted pty gets the new
  content-area size and the visible tool redraws.
- Show a tool's exit clearly (exit code, "press enter to restart") instead of
  a blank area.
- Hosted tools inherit `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_ENV`,
  `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID` and `HERDR_PANE_ID` (the toolbar's own
  pane), plus `HERDR_TOOLBAR_TOOL_ID`, so a purpose-built tool can create
  workspaces and start or prompt agents through the Herdr CLI. The toolbar's
  `TERM` and related variables are set to a value without graphics support
  (builder's choice, e.g. `xterm-256color` with `TERM_PROGRAM` unset).

## Non-goals

- Writing any of the tools. The toolbar hosts third-party terminal programs;
  issue browsing, todo, database and file browsing come from `gh dash`,
  `lazysql`/`harlequin`, `yazi`/`broot`, `jira-cli` or whatever the owner
  puts in the config.
- Mouse support inside hosted tools in v1. Keyboard only; mouse may come later.
- Image passthrough in v1 (decision 6). Planned follow-up.
- Scrollback for hosted tools beyond what the tool itself provides.
- Writing the owner's own issue-to-agent tool. Enabled by decision 5, built
  separately.
- Multiple toolbar panes in one workspace, or a toolbar that follows the user
  across workspaces.
- A hidden-but-running toolbar (closing the pane keeps processes alive).
  Herdr has no native collapsible right panel; "collapsed" in v1 means closed.
- Native Herdr UI changes. This is a plugin; it uses only the documented
  plugin surface (`[[panes]]`, `[[actions]]`, `[[keys.command]]`, the socket
  API through `HERDR_BIN_PATH`).

## Anti-goals

Things that would make this a failure even if it technically works.

- Input lag or dropped keystrokes in a hosted tool. If typing in `lazysql`
  feels worse than in a plain pane, the toolbar is not worth having.
- Garbled rendering of common TUIs (box drawing, 256/true colour, cursor
  shapes). The embedded terminal must be faithful enough that the owner stops
  noticing it is embedded.
- A config format that needs the source to understand. `tools.toml` must be
  obvious from a two-entry example.
- Leaking processes: closing the pane or quitting Herdr must leave no orphaned
  tool processes.

## Constraints

- Herdr 0.9.5 plugin surface, as documented at herdr.dev `plugins.mdx`:
  - Manifest `herdr-plugin.toml` with `id`, `name`, `version`,
    `min_herdr_version`, optional `platforms`; `[[build]]`, `[[startup]]`,
    `[[actions]]`, `[[events]]`, `[[panes]]`, `[[link_handlers]]`,
    `[[keys.command]]`.
  - `[[panes]]` entries run an argv command with `placement` one of `overlay`
    (default), `popup`, `split`, `tab`, `zoomed`. Split panes become normal
    Herdr panes, so `herdr pane move`, `pane resize`, `pane zoom` apply.
    Verified on 0.9.5 during the builder check: `herdr plugin pane open`
    accepts `--direction right`, `--target-pane`, `--cwd`, `--env` and
    `--no-focus`; width is set afterwards with `pane move --ratio`,
    `pane resize`, or the socket API's `layout.set_split_ratio`.
  - Pane commands receive `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_ENV=1`,
    `HERDR_PLUGIN_ID`, `HERDR_PLUGIN_ROOT`, `HERDR_PLUGIN_CONFIG_DIR`,
    `HERDR_PLUGIN_STATE_DIR`, `HERDR_PLUGIN_CONTEXT_JSON`,
    `HERDR_PLUGIN_ENTRYPOINT_ID`, and `HERDR_WORKSPACE_ID` / `HERDR_TAB_ID` /
    `HERDR_PANE_ID` when available. The process's working directory is the
    plugin directory, not the workspace.
  - `[[build]]` runs only on `herdr plugin install` from GitHub, not on
    `plugin link`; Herdr does not install toolchains. A linked checkout is
    built by the owner (`cargo build --release`).
  - Config and state directories survive update, reinstall and uninstall.
- macOS first (the owner's machine), Linux expected to work; Windows out of
  scope.
- Hosted tools are arbitrary user-chosen binaries. The plugin must not assume
  any of them is installed; a missing binary is a clear message in the content
  area, not a crash.

## How it works

### Process model

One binary, `herdr-toolbar`, runs inside the Herdr pane. It owns:

- the ratatui terminal (crossterm backend) for the whole pane;
- a `Tool` per config entry: id, display name, icon, argv, cwd, env, and a
  runtime slot holding either `NotStarted`, `Running { pty, parser, child }`
  or `Exited { code }`;
- the selection index.

Each running tool has a `portable-pty` master/slave pair sized to the content
area, a `vt100::Parser` fed from a reader thread, and the child process. The
UI thread redraws on input, on pty output (reader threads send a wake message
over a channel), on resize, and on a slow tick for the exit state.

### Layout

```
+------------------------------------------+---+
|                                          | ◉ |  <- selected: inverse/accent
|   content area                           | ☑ |
|   (tui-term PseudoTerminal widget        | ▤ |
|    showing the selected tool)            | ▭ |
|                                          | ◆ |
|                                          |   |
|                                          | ⟳ |  <- reload config (bottom)
+------------------------------------------+---+
```

The bar is 3 columns wide (icon with one cell of padding on each side) and
spans the pane height. A tool whose process has exited shows its icon dimmed;
one with unread output while unselected may show a dot (nice to have, builder's
call).

### Input routing

All keys go to the hosted tool except a small reserved set, configurable, with
these defaults:

| Key | Action |
|---|---|
| `Ctrl-]` then `j`/`k` or `1`..`9` | select next/previous/nth tool |
| `Ctrl-]` then `r` | reload config |
| `Ctrl-]` then `x` | kill and restart the selected tool |
| `Ctrl-]` then `q` | quit the toolbar (closes the pane) |
| `Ctrl-]` then `Ctrl-]` | send a literal `Ctrl-]` to the tool |

A two-key prefix was chosen over single chords because hosted tools use most
chords themselves. The prefix is configurable. The owner's Herdr prefix is
the default `ctrl+b` (checked), so `Ctrl-]` does not clash today.

### Config

`tools.toml` (global) and `.herdr-toolbar.toml` (workspace, optional), same
schema. Entries merge by `id`; a workspace entry with the same `id` replaces
the global one; `order` in the workspace file, if present, wins.

```toml
# ~/.config/herdr/plugins/<id>/tools.toml  (path printed by `herdr plugin config-dir`)
prefix = "ctrl-]"          # optional, default shown
order  = ["issues", "todo", "db", "files", "jira"]   # optional, default = file order

[[tool]]
id      = "issues"
name    = "GitHub issues"
icon    = "◉"
command = ["gh", "dash"]

[[tool]]
id      = "todo"
name    = "Todo"
icon    = "☑"
command = ["taskwarrior-tui"]

[[tool]]
id      = "db"
name    = "Database"
icon    = "▤"
command = ["lazysql"]
cwd     = "~"             # optional; default is the workspace directory

[[tool]]
id      = "files"
name    = "Files"
icon    = "▭"
command = ["yazi"]

[[tool]]
id      = "jira"
name    = "Jira"
icon    = "◆"
command = ["jira", "issue", "list"]
env     = { JIRA_PROJECT = "SKOOL" }   # optional extra environment
```

Workspace override example, `.herdr-toolbar.toml` at a repo root:

```toml
[[tool]]
id      = "db"
name    = "Slot DB"
icon    = "▤"
command = ["lazysql", "postgres://skoolscout@localhost:5433/skoolscout"]
```

Invalid config never kills a running toolbar: the error is shown in the
content area and the previous config stays active.

### Herdr manifest

```toml
id = "jefelabs.toolbar"
name = "Toolbar"
version = "0.1.0"
min_herdr_version = "0.9.0"
description = "Right-hand activity bar of tools, each hosted in its own terminal"
platforms = ["macos", "linux"]

[[build]]
command = ["cargo", "build", "--release"]

[[panes]]
id = "toolbar"
title = "Toolbar"
placement = "split"
command = ["target/release/herdr-toolbar"]

[[actions]]
id = "toggle"
title = "Toolbar: toggle"
contexts = ["workspace"]
command = ["target/release/herdr-toolbar", "toggle"]

[[actions]]
id = "select"
title = "Toolbar: select tool"
contexts = ["workspace"]
command = ["target/release/herdr-toolbar", "select"]
```

A plugin manifest cannot declare keybindings (verified against the 0.9.5
docs). The binding goes in the user's Herdr `config.toml`, and the README
tells the owner to add it:

```toml
[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "jefelabs.toolbar.toggle"
```

**Switching without leaving the agent pane (designer's call).** The `select`
action takes a tool index or id and delivers it to the running toolbar with
`herdr pane send-keys` into the toolbar pane (the toolbar remembers its own
pane id). The owner can bind `prefix+1`..`prefix+5` to
`jefelabs.toolbar.select 1`.. in `config.toml` and change tools while focus
stays on the agent. Opening the toolbar uses `--no-focus`, so the agent pane
keeps focus. This costs one small action, nothing structural; the builder may
replace send-keys with a control socket in `HERDR_PLUGIN_STATE_DIR` if
send-keys proves unreliable.

`toggle` is a thin mode of the same binary: it asks Herdr (via `HERDR_BIN_PATH`
and the socket) whether a toolbar pane already exists in the current
workspace (by a marker it set with `herdr pane report-metadata` or by pane
title), closes it if so, otherwise runs `herdr plugin pane open --direction
right --no-focus` and sets the split ratio to about 35% of the tab width.

### Lifecycle

- Start: parse config, draw bar, select the first tool, start its process.
- Select another tool: start it if `NotStarted`, otherwise just display it.
- Resize: resize every `Running` pty to the content-area size (background tools
  included, so they are correct when shown).
- Tool exits: keep its last screen, overlay a one-line footer "exited (code N),
  enter to restart", dim its icon.
- Quit or pane closed (SIGHUP/SIGTERM): send SIGHUP to every child, wait
  briefly, SIGKILL stragglers, restore the terminal.

## Tools the owner intends to build later

Not part of this project. Listed because they shape what the toolbar must
make possible, and they set the priority of the follow-ups.

| Tool | What it does | What it needs from the toolbar |
|---|---|---|
| Issue-to-agent (GitHub, Jira) | Browse issues, pick one, start a new agent in a new workspace with it | Herdr environment passthrough (decision 5). Covered in v1. |
| Before/after code change view | Show a change as C4 diagrams of the system before and after, generated from the worktree (e.g. Structurizr or D2 rendered to images, with an ASCII fallback) | Worktree as cwd (decision 4). Diagrams as pictures need image passthrough; until then an ASCII/box-drawing rendering. |
| Spec / plan viewer | Show the spec and plan the agent in this workspace is working on, from `docs/superpowers/specs` and `plans`, with the current task highlighted | Worktree as cwd. Markdown rendering; images in specs need image passthrough. |

| terminal-browser (zenbu-labs) | A real browser in the terminal, as a slot | Paints pages as Kitty graphics, so it is blank in the bar until image passthrough; run it in a plain Herdr pane meanwhile. (Owner asked 2026-10-24.) |

Three of the four want pictures, which is why image passthrough is the first
follow-up after v1 rather than a someday item.

## Left to the builder

- Crate versions and whether to use `tui-term`'s built-in `vt100` parser or a
  custom one; the design only requires faithful rendering of common TUIs.
- Thread vs. async design for pty readers.
- How the `toggle` action discovers an existing toolbar pane (pane metadata
  vs. a pid file in `HERDR_PLUGIN_STATE_DIR`).
- Exact config directory path resolution and reload mechanics (poll vs. key
  only).
- Error presentation details, colours, and whether to honour Herdr's `accent`.
- Test strategy. Suggested minimum: unit tests for config merge and key-prefix
  state machine; an integration test that spawns `sh -c 'printf ...; sleep'`
  into a tool slot and asserts the rendered buffer; a manual checklist with
  `lazysql`, `yazi` and `gh dash`.
- Marketplace listing (GitHub topic `herdr-plugin`) and README, after v1 works
  for the owner.
- Cargo workspace mechanics: a workspace builds into one `target/` at the repo
  root, so `toolbar/herdr-plugin.toml`'s build command and binary path must
  account for that (e.g. `cargo build --release -p herdr-toolbar` and
  `../target/release/herdr-toolbar`, or a per-package target dir). Verify on
  the first GitHub install whether Herdr runs `[[build]]` in the subdirectory
  or the repo root; the docs do not say.

## Open items for the owner

Raised by the builder check on 2026-10-24; answers go here and the body is
updated to match.

1. ~~Tabs~~ and 2. ~~Working directory~~: answered by the owner, see
   decision 4 and the designer's calls on `workspace_cwd` and tab placement.
3. ~~Images in v1~~: text-only, see decision 6.
4. ~~Name and repo~~: `jefelabs/herdr-plugins`, plugin at `toolbar/`.
5. Confirm the default toggle key (`prefix+t`) and in-pane prefix (`Ctrl-]`).
