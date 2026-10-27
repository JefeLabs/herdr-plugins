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

[[tool]]
id      = "browser"
name    = "Browser"
icon    = "◎"
command = ["terminal-browser", "open"]
open    = "pane"                              # optional: a Herdr pane, not the slot

[[tool]]
id       = "web"
name     = "Browser (in the toolbar)"
icon     = "◍"
command  = ["terminal-browser", "open"]
graphics = true                               # experimental: show its images in the slot
```

Tools start the first time you select them.

A tool with `open = "pane"` runs in a Herdr pane instead of the toolbar's slot: selecting it opens a right split of the work pane (the toolbar's left neighbour, else the focused pane), runs its `command` there with its `cwd` and `env`, and focuses it. The pane is labelled with the tool's name. Selecting the tool again closes that pane, so the button works as a toggle; if the pane was closed some other way, the next selection opens a new one. The pane id is kept under the plugin's state folder, so this survives a toolbar restart, and a recorded pane is closed only while it still carries the tool's name (Herdr reuses pane ids after a restart). Use it for tools that need the room or draw with graphics, such as `terminal-browser`, which shows pages as images.

A pane tool can open several panes at once: give it `panes`, one command per pane, in place of `command`. The first pane opens beside the work pane as above; the others stack below it at equal heights, each running its own command with the tool's `cwd` and `env`. Every pane carries the tool's name, and selecting the tool again closes all of them, skipping any you already closed. Arguments are passed as written, so `~` is not expanded: write paths in full.

```toml
[[tool]]
id    = "usage"
name  = "Usage"
icon  = "▤"
open  = "pane"
panes = [
  ["procs", "--watch-interval", "2", "--sortd", "cpu", "--or", "herdr", "docker", "Workspaces"],
  ["oxker"],
  ["dua", "i", "/path/to/your/repos"],
]
```

`graphics = true` (experimental) lets a slot tool draw Kitty images. The slot's screen model drops them, so the toolbar cuts each image command out of the tool's output, keeps the cursor where it was printed, and after each frame writes it to Herdr placed at that cell of the slot, with replies turned off (`q=2`) so nothing comes back into the toolbar's input. It also answers what a graphics tool asks before drawing: whether images work, the device attributes (`CSI c`), and, when the toolbar's terminal reports its pixel size, the cell and text-area size (`CSI 16 t`, `CSI 14 t`, and the pty's pixel size). Switching to another tool clears the images; switching back makes the tool redraw. Every frame passes through the toolbar, so a busy page is slower than in its own pane, and the slot is only as big as the toolbar: `open = "pane"` is the faster choice for real browsing. A tool that is not installed shows a one-line "cannot start" message in its slot and the other tools keep working.

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

- **Text by default.** The embedded terminal advertises `TERM=xterm-256color` without graphics, so tools fall back to text: `yazi` lists files without thumbnails, `mdcat` prints markdown without pictures. A tool with `graphics = true` gets its Kitty images passed through (experimental, see above); the others stay text.
- **Mouse.** Clicking an icon selects that tool. Clicks, drags and the wheel inside a tool reach it only when the tool has turned mouse reporting on (as `yazi`, `lazysql` and the capabilities tool do), in the mode and encoding it asked for. Because the toolbar captures the mouse, a plain drag no longer selects text in its pane; most terminals still select with Shift held.
- One toolbar per workspace, in the tab where it was opened. Closing it stops its tools.

## Troubleshooting

- `cannot start `X`: Unable to spawn ...`: the binary is not on `PATH` as the toolbar sees it. The toolbar passes your `PATH` through unchanged.
- `untrusted .../.herdr-toolbar.toml ignored`: run `herdr-toolbar trust` in that workspace.
- `toolbar is not open in this workspace`: `select` found no running toolbar; `toggle` first.
