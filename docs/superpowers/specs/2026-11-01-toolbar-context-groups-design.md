# Toolbar context groups

Status: draft for the owner's review. **(owner)** marks the owner's decisions. **(my call)** marks a choice that is mine and open to correction.

## Why

Each Herdr workspace is a space, and its toolbar hosts the tools you work with beside an agent's chat. Some tools are about the project as a whole: its intents, its Jira, its GitHub issues, its app in a browser. Others are about the agent you're working with right now: its database, its worktree's files, an editor and a terminal in that worktree. A few are about the machine. The owner wants the rail to show that split, and each tool to follow the context of its group without being told how in every tool's command:
- "there are groupings: space context, repo context, agent context, system context" **(owner)**;
- then, folding GitHub issues into the space group: "remove repo context group" **(owner)**.

#46–#48 built the pieces this rests on:
- the workspace's context, from `herdr-intents context`: `{meta}`, `{agent}`, `{db}`;
- `{name?}` values, filled in per tool;
- following the active agent (`herdr agent list`, every second), and restarting a tool whose values changed;
- rules drawn from `"---"` in `order`.

This design puts the context in the group. A tool says which group it belongs to, and the group decides where it starts and what happens to it when you switch agents.

## Goals

- **The rail shows three groups, top to bottom (owner):** space, agent, system, with a rule between each **(owner: "please put a HR between groups")**.
- **A tool's group decides what it follows,** so moving a tool to another group changes what it follows, with nothing else to edit **(my call)**.
- **Switching agents never loses work (owner):**
  - LazySql restarts on the new agent's database;
  - Terminal keeps its shell;
  - Editor keeps its buffers and moves its folder.
- **The main pane stays the agent's chat (owner),** and the agent in it is the active agent.

## Non-goals

- **Agents on other machines:** local only **(owner)**.
- **Popping a tool out into its own pane:** a separate design, brainstormed 2026-11-01.
- **Space settings for space tools beyond the browser's page,** such as which Jira project belongs to a space. Space tools start at the meta repo, so `gh dash` finds the space's repo. Jira keeps its `env`.
- **Usage in the toolbar:** it is still three stacked pane programs, and how it fits in the toolbar is open with the owner.
- **The repo group,** which the owner removed.

## Anti-goals

- **A switch that kills work:** a running command in Terminal, unsaved buffers in Editor.
- **A tool on one agent's data after the switch without saying so:** for example LazySql still on the previous agent's database.
- **A rail that reshuffles by itself.** The group order is fixed, and `order` decides within a group.
- **Breaking today's configs:** a `tools.toml` with no groups works exactly as now, including `"---"` rules.

## What it does

### Groups

A tool names its group in `tools.toml` **(my call: a field, not positions in `order`)**:

```toml
[[tool]]
id      = "db"
name    = "Database"
icon    = "▤"
group   = "agent"
command = ["lazysql", "{db?}"]
```

- **`group = "space" | "agent" | "system"`.** Any other word is a config error naming the tool **(my call)**.
- **The rail draws the groups in that fixed order (owner),** with a rule between each pair of non-empty groups **(owner)**. `order` sorts the tools within a group.
  - Pane tools still come last within their group, as today's bar keeps them below the toolbar tools **(my call)**.
- **Tools without a group** are drawn after the grouped ones, behind a rule, and keep today's behaviour **(my call)**.
- **When any tool names a group, `"---"` in `order` is ignored,** since the groups draw the rules. Without groups, `"---"` works as in #48 **(my call)**.

### What each group means

| Group | Follows | Starts in (unless the tool sets `cwd`) | When another agent becomes active |
|---|---|---|---|
| **space** | the space: the project as a whole | the meta repo, `{meta?}` | nothing changes |
| **agent** | the active agent | the top of its worktree, `{agent?}` | the tool's `on_switch` (below) |
| **system** | the machine | as written (the workspace's folder by default) | nothing changes |

- **"Starts in" is a default.** A tool that sets `cwd` keeps it, and `{name}` values still work in `command` and `cwd` as in #46 **(my call)**.
- **The active agent** is as in #46: the focused agent of the workspace, or, while the toolbar has focus, the one active before **(owner: "the agent that is currently set as active")**.

### When the active agent changes: `on_switch`

An agent tool says what a switch does to it **(owner, tool by tool; the setting is my call)**:

| `on_switch` | What happens | For |
|---|---|---|
| `"restart"` (the default) | if its filled-in command or folder changed, it restarts right away with the new values, as #46 does today | **Database (owner: "yes, switch it right away")**, Files (**my call**: yazi has nothing to lose) |
| `"keep"` | it keeps running as it is. The next time it starts (it exited, or `Ctrl-]` `x`), it starts with the new agent's values | **Terminal (owner: "keep the shell, new terminals open in the new worktree")** |
| a command, `["…"]` | it keeps running, and the toolbar runs this command (values filled in), so the running tool can follow on its own | **Editor (owner: "yes" to moving Neovim's folder through its socket)** |

Editor's entry, with the socket its command already listens on:

```toml
group     = "agent"
on_switch = ["sh", "-c", "p=$(printf %s \"$1\" | sed \"s/'/''/g\"); exec nvim --server \"${XDG_CACHE_HOME:-$HOME/.cache}/nvim-editor-${HERDR_WORKSPACE_ID:-local}.sock\" --remote-expr \"chdir('$p')\" >/dev/null", "sh", "{agent}"]
```

- **The command runs in the background (my call).** It runs with the tool's `env`, in the new agent's folder. A failure shows on the toolbar's footer and doesn't stop the tool.
- **`on_switch` is only for agent tools.** On a space or system tool it is a config error **(my call)**.
- **A `"keep"` or command tool that was never opened** simply starts with the current values when you open it.

### The owner's rail after this

```toml
order = ["intents", "jira", "web", "issues", "db", "files", "editor", "terminal", "usage"]
```

| Group | Tools |
|---|---|
| space | Intents, Jira, Browser, GitHub issues |
| agent | Database (`restart`), Files (`restart`), Editor (command: `:cd`), Terminal (`keep`) |
| system | Usage (a pane tool) |

Intents stays the first tool, in the space group **(owner)**.

### The space's page for the browser (owner: "space context should have default browser url")

- **Where it comes from (my call):** the run guide gets `browse:`, the space's default page. `herdr-intents context` passes it on as `browse`. With no `browse:`, the guide's `url` stands in when it needs no slot's `{port}`. Otherwise `browse_error` says why.
- **How the Browser uses it:** the space-group Browser opens `terminal-browser open {browse?}`.
- **No trust needed:** it's an address, not a command.

### Settled while building (my calls, after the builder check)

- **`on_switch` only for tools in the toolbar.** A pane tool takes its values when it opens, as today. So the catalog's Editor becomes a toolbar tool, as the owner's already is, since that's where `:cd` on switch works.
- **The catalog's Terminal drops `cwd = "{meta?}"`** and follows the agent group's folder.
- **A kept tool says so (anti-goal).** While a `"keep"` tool is on the previous agent, its footer reads `still in <old worktree> · ctrl-] x starts it in the new one`. The tool gets one row less, so the note never covers its last line, and a restart clears it.
- **The first context is not a switch.** A `"keep"` tool started before the active agent was known moves into the agent's worktree when it becomes known. An `on_switch` command runs then too.
- **A replaced tool keeps its group.** A worktree's `.herdr-toolbar.toml` entry that replaces a grouped tool without naming a group keeps the global entry's group, and so its place.

## Constraints

- **Rust, test-first, in `toolbar/`,** building on #46–#48: `context::expand`, `App::apply_context`, `App::rules`.
- **Following the agent stays as in #46:** a one-second poll of `herdr agent list`, run only when some tool depends on the agent. That now means an agent-group tool, or a `{name}` value.
- **No new Herdr hooks,** so no re-link is needed.
- **The installer's catalog** (`toolbar/setup/*.toml`) gets the `group` and `on_switch` lines above, so new installs get the grouped rail. `tools.example.toml` too.

## Rollout

1. herdr-plugins PR, then the release toolbar binary.
2. The owner's `tools.toml`: add `group` (and `on_switch` for Editor and Terminal) to each tool, take the `"---"` out of `order`, and keep a backup.
3. Going live with the context values themselves (the `context` line, the Database `{db?}`, `herdr-intents trust` in skoolscout-com) is the owner's pending step from #46–#47. Without it, agent tools still start in the active agent's folder (`{agent}` comes from the toolbar), but Database has no `{db}`.

## Testing

- **Config:** `group` read, an unknown group refused, `on_switch` refused outside the agent group, `"---"` ignored once groups exist, no groups means the same behaviour as #48.
- **Rail:** fixed group order whatever `order` says, `order` within each group, a rule between non-empty groups only, ungrouped tools after them, pane tools last within their group, and clicks and hover skipping the rules.
- **Defaults:** a space tool starts at `{meta}` (the workspace's folder when there is none), an agent tool at `{agent}`, and an explicit `cwd` wins.
- **Switching:**
  - `restart` restarts only when the filled-in command or folder changed;
  - `keep` leaves the process running (same pid) and starts the next run in the new folder;
  - a command runs once per switch with `{agent}` filled in, its failure shows on the footer, and the tool's pid is unchanged;
  - space and system tools are never touched.
- **Real check:** the real toolbar in a probe with a stand-in `herdr` whose agent switches. The Terminal's pid stays the same, a stand-in Database restarts, and Neovim's `getcwd()` follows the switch through its socket.

## Left to the builder

- Field names in code, and how the rail's groups replace `rules_after`.
- How the background `on_switch` command is run and timed out (a few seconds), and how its failure reads on the footer.
- Values reach a command only as their own arguments (`"$1"`), and Neovim gets a quoted `chdir()`, not keystrokes (fixed after a security review: a folder name must never become shell or Ex commands). A value with a control character is never used.
