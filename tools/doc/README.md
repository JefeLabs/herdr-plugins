# herdr-doc

The toolbar's Editor, with documents. It is one Neovim session in the active agent's worktree, where an agent can show you a file formatted (Markdown rendered, JSON indented, CSV in columns, images as block pictures) and you can switch to editing it with one key. Because the formatted view and the editing are buffers of the same Neovim, an agent opening a file never costs you unsaved work.

## Showing a file

An agent shows you a file with:

```sh
herdr-doc open docs/plan.md
```

The path is resolved against the agent's own folder. The toolbar of that workspace switches to the Editor (starting it if needed) and shows the file formatted. Tell your agents about it, for example in `CLAUDE.md`:

> To show the user a file, run `herdr-doc open <file>`. It opens in the toolbar, formatted, and the user can edit it from there.

## Keys

| Where | Key | What it does |
|---|---|---|
| the formatted view (`less`) | `v` | edit the file in this Neovim (view-only files say so) |
| | `/`, `n`, `N`, `g`, `G` | search and move, as in `less` |
| | `q` | close the view |
| the file's buffer (Neovim) | `:Doc` or `<leader>v` | back to the formatted view, re-rendered with what you saved |
| anywhere in Neovim | `:Doc <file>` | view any file formatted |

`<leader>v` is only mapped when your config hasn't already taken it.

## File types

| Kind | Extensions | Shown by | `v` edits |
|---|---|---|---|
| Markdown | `.md`, `.markdown` | `mdcat --ansi --columns <width>` | yes |
| JSON | `.json`, `.jsonc`, `.geojson` | `jaq -C .` | yes |
| CSV | `.csv` | `tv -a` | yes |
| TSV | `.tsv` | `tv -a -s <tab>` | yes |
| Images | `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp` | `viu -b -w <width>` | no |
| SVG | `.svg` | `resvg` to a temporary picture, then `viu` (the picture is removed) | no |
| PDF | `.pdf` | `pdftotext -layout` | no |
| anything else | | shown as it is | no |

A renderer that isn't installed shows the file as it is rather than failing. Install them with:

```sh
cargo install mdcat jaq tidy-viewer viu resvg
brew install poppler   # pdftotext (apt: poppler-utils)
```

## In the toolbar

```toml
[[tool]]
id        = "editor"
name      = "Editor"
icon      = "◫"
group     = "agent"
command   = ["/absolute/path/to/herdr-plugins/target/release/herdr-doc"]
on_switch = ["sh", "-c", "p=$(printf %s \"$1\" | sed \"s/'/''/g\"); exec nvim --server \"${XDG_CACHE_HOME:-$HOME/.cache}/nvim-editor-${HERDR_WORKSPACE_ID:-local}.sock\" --remote-expr \"chdir('$p')\" >/dev/null", "sh", "{agent}"]
```

`herdr-doc open` asks the toolbar for the tool named `editor`. Set `HERDR_DOC_TOOL` if yours has another id. Neovim listens on `${XDG_CACHE_HOME:-~/.cache}/nvim-editor-<workspace>.sock`, the Editor's usual socket, so `nvim --server … --remote <file>` and the `on_switch` above keep working. The installer links `herdr-doc` into `~/.local/bin` when the Editor is on your bar, so agents can run it.

## Modes

- `herdr-doc [files…]`: the pane, which is Neovim with the document view.
- `herdr-doc open <file>`: show a file, as an agent does.
- `herdr-doc view <file>`: the formatted view (`less -R` over `render`).
- `herdr-doc render <file>`: print the formatted view. It's what `less` runs (`LESSOPEN`). The width comes from `HERDR_DOC_COLUMNS`, else the terminal, else `COLUMNS`, else 100.
- `herdr-doc edit [+line] <file>`: what `v` runs (`VISUAL`).

## Safety

A file's name never becomes code. It reaches Neovim as a quoted string through `v:lua` calls, and Neovim opens it with filename and bar magic off. `less` runs the renderer and `v` with the name quoted. A name with a control character, such as a newline, is refused.
