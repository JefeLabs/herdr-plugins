# Document pane (herdr-doc)

Status: built from the owner's draft spec ("herdr Document Pane — Spec", v0.1), and adapted during the conversation of 2026-11-01. **(owner)** marks the owner's decisions. **(my call)** marks a choice of mine, open to correction. The owner said "skip the questions, just build it".

## Why

The owner works in the agent's chat, in the main pane, and wants to see files the agent talks about, formatted, without leaving the toolbar: "when i ask agent to see a file it can open them in a view mode and i can toggle it for edit if i want" **(owner)**.

## What was decided

- **The agent opens files (owner).** An agent shows the user a file, in view mode, with one command. The draft's "external control (v2)" is the main way in, so it is v1 here: `herdr-doc open <file>`, resolved against the agent's own folder.
- **The toolbar switches right away (owner)** to the document when an agent opens one.
- **Opening it yourself gives Neovim (owner).** Launched from the rail, the tool is a Neovim session. Files an agent opens start in the formatted view.
- **It follows the active agent and works in its worktree (owner):** agent group, with the Editor's `chdir` on switch.
- **One tool with the Editor (my call).** The document view and editing are buffers of the same Neovim, so an agent opening a file never costs unsaved work:
  - the view is a terminal buffer running `less -R` over `herdr-doc render`;
  - `v` (less's `VISUAL`) opens the file's own buffer through `$NVIM`;
  - `:Doc`, or `<leader>v` when free, goes back to the view, re-rendered;
  - `q` closes the view.

  The tool keeps the Editor's id and socket (`nvim-editor-<workspace>.sock`), so `nvim --server … --remote` and `on_switch` keep working.
- **Rust, not bash scripts (my call).** macOS ships bash 3.2, so the draft's `${f,,}` and GNU `mktemp --suffix` would fail. `herdr-doc` is a new crate (`tools/doc`) with the modes `[files…]` (the pane), `open`, `view`, `render` and `edit`. The renderers stay external CLIs, as in the draft.
- **File types and editing: as in the draft (owner).**
  - Markdown, JSON, CSV and TSV are rendered and editable.
  - Images, SVG (through resvg to a temporary PNG, always removed) and PDF are view-only.
  - Anything else is shown raw and view-only.
  - A missing renderer gives the raw view.
  - `Kind::editable` is the single source of truth.
- **How `open` reaches the toolbar (my call):** the toolbar's control socket (`select editor`, or `HERDR_DOC_TOOL`), at `${XDG_STATE_HOME:-~/.local/state}/herdr/plugins/jefelabs.toolbar/<workspace>.sock`. Herdr's `plugin action invoke` takes no workspace or argument. `open` then waits up to 10 seconds for the tool's Neovim.
- **Any agent's open switches the toolbar (my call).** My earlier guess was "only the active agent's", but that needs the toolbar to compare panes. v1 keeps it simple.
- **Installing (my call).**
  - The release builds and ships `herdr-doc`.
  - The catalog's Editor runs it.
  - The installer links it into `~/.local/bin` when the Editor is on the bar, so agents can run it.
  - Renderers are not installed by the installer (no pinned builds yet). The README gives `cargo install mdcat jaq tidy-viewer viu resvg` and poppler.
- **Width (my call):** `HERDR_DOC_COLUMNS`, else `stty size` on `/dev/tty` (less pipes the renderer's output), else `COLUMNS`, else 100. This answers the draft's question about `tput cols` in a pane.
- **Safety (my call, after the #49 review):**
  - Paths reach Neovim as quoted strings through `v:lua` calls, never as Ex command lines or keystrokes.
  - Neovim opens files with filename and bar magic off.
  - Commands that `less` runs get shell-quoted paths.
  - A path with a control character is refused.

## Non-goals (v1)

- Live preview while editing.
- High-resolution images.
- HTML, rst, org, AsciiDoc and Mermaid.
- Mouse.
- Agents on other machines.
- Installing the renderers.

## Open (from the draft)

- **SVG, `.txt` and `.toml`:** should they be editable? They're view-only for now, as the draft lists.

## Testing

- **Unit:** the file-type matrix, what's editable, socket paths, and Vim quoting.
- **Render:** through the binary against stand-in renderers. Each kind's command and width, a missing renderer printing nothing, and the SVG picture removed afterwards.
- **Real Neovim (headless):**
  - an agent's `open` shows a terminal-buffer view and sends `select editor` to the toolbar;
  - `v` edits the file in the same Neovim, and `:Doc` goes back;
  - a view-only file says so;
  - a file named `it's|!touch PWNED.md` stays a name;
  - a missing file is reported.
- **Install:** the Editor runs `herdr-doc` and is linked onto `PATH`.
- **Real toolbar probe:** the toolbar switched from Intents to the Editor on an agent's `open` and showed the file, `v` edited it, `:Doc` came back, and `q` closed the view.
