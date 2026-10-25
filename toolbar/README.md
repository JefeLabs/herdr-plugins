# herdr-toolbar

A right-hand activity bar for [Herdr](https://herdr.dev). One icon per tool, single-select, like the Agents and Spaces sidebar on the left. Each tool is an ordinary terminal program (`gh dash`, `lazysql`, `yazi`, `jira`, a todo app) hosted in its own pseudo-terminal, so switching icons never restarts anything: a tool is exactly where you left it when you come back.

The toolbar belongs to a workspace. Tools start in the workspace's directory, which is that agent's worktree, and they inherit the Herdr environment, so a tool of your own can create workspaces and start or prompt agents through the `herdr` CLI.

## Install

Rust toolchain required (`cargo`). Herdr 0.9.0 or later.

```sh
# from a clone of this repo
cargo build --release -p herdr-toolbar
herdr plugin link "$PWD/toolbar"
# or, once published:
herdr plugin install jefelabs/herdr-plugins/toolbar
```

On first run the toolbar seeds `tools.toml` in its config dir from [`tools.example.toml`](tools.example.toml). Find that dir with:

```sh
herdr plugin config-dir jefelabs.toolbar
```

## Keys in Herdr

A plugin cannot declare keybindings, so add these to your Herdr `config.toml` (`herdr server reload-config` afterwards):

```toml
[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "jefelabs.toolbar.toggle"

# switch tools while focus stays on your agent (one action per slot)
[[keys.command]]
key = "prefix+1"
type = "plugin_action"
command = "jefelabs.toolbar.select-1"
# ... prefix+2 .. prefix+5 with select-2 .. select-5
```

`toggle` opens the toolbar as a right split next to the focused pane without taking focus, sized to about a third of the tab, and closes it again when it is open. `focus` jumps into the toolbar, or back out to the pane on its left when the toolbar already has focus; bind it to `prefix+f`. `select-1` to `select-5` switch the running toolbar to that slot; the binary's own `select <id-or-index>` mode accepts a tool id as well.

## Keys inside the toolbar

Everything goes to the hosted tool except a two-key prefix, default `Ctrl-]`:

| Keys | Action |
|---|---|
| `Ctrl-]` `1`..`9` | select the nth tool |
| `Ctrl-]` `j` / `k` | next / previous tool |
| `Ctrl-]` `r` | reload `tools.toml` |
| `Ctrl-]` `x` | kill and restart the selected tool |
| `Ctrl-]` `q` | quit the toolbar |
| `Ctrl-]` `Ctrl-]` | send a literal `Ctrl-]` to the tool |
| `Enter` on an exited tool | restart it |
| click an icon | select that tool |

Change the prefix with `prefix = "ctrl-b"` (or `alt-t`, ...) at the top of `tools.toml`.

## tools.toml

```toml
prefix = "ctrl-]"
order  = ["issues", "todo", "db", "files", "jira"]   # optional, default is file order

[[tool]]
id      = "issues"
name    = "GitHub issues"
icon    = "◉"
command = ["gh", "dash"]

[[tool]]
id      = "db"
name    = "Database"
icon    = "▤"
command = ["lazysql"]
cwd     = "~"                                 # optional, default is the workspace directory
env     = { PGHOST = "localhost" }            # optional
```

Tools start the first time you select them. A tool that is not installed shows a one-line "cannot start" message in its slot and the other tools keep working.

### Per-worktree overrides

A worktree may carry `.herdr-toolbar.toml` at its root with the same schema. Its entries replace global ones with the same `id`, new ids are appended, and its `order`, if present, wins. For example, a repo whose Postgres runs on a numbered slot:

```toml
[[tool]]
id      = "db"
name    = "Slot DB"
icon    = "▤"
command = ["lazysql", "postgres://app@localhost:5433/app"]
```

Because a cloned repo could use this to run anything, a workspace file is **ignored until you trust it**:

```sh
herdr-toolbar trust       # inside the workspace, e.g. via a Herdr action or from the toolbar's own shell
```

Trust is recorded as the file's path plus a hash of its content, so it has to be granted again whenever the file changes. Symlinked files are never trusted. Variables that change how a binary is loaded or found (`PATH`, `SHELL`, `HOME`, `LD_*`, `DYLD_*`, ...) are dropped from workspace-supplied `env`.

## Known limits (v1)

- **Text only.** The embedded terminal advertises `TERM=xterm-256color` without graphics, so tools fall back to text: `yazi` lists files without thumbnails, `mdcat` prints markdown without pictures. Passing Kitty graphics through is the first planned follow-up.
- **Mouse.** Clicking an icon selects that tool. Clicks, drags and the wheel inside a tool reach it only when the tool has turned mouse reporting on (as `yazi`, `lazysql` and the capabilities tool do), in the mode and encoding it asked for. Because the toolbar captures the mouse, a plain drag no longer selects text in its pane; most terminals still select with Shift held.
- One toolbar per workspace, in the tab where it was opened. Closing it stops its tools.

## Troubleshooting

- `cannot start `X`: Unable to spawn ...`: the binary is not on `PATH` as the toolbar sees it. The toolbar passes your `PATH` through unchanged.
- `untrusted .../.herdr-toolbar.toml ignored`: run `herdr-toolbar trust` in that workspace.
- `toolbar is not open in this workspace`: `select` found no running toolbar; `toggle` first.
