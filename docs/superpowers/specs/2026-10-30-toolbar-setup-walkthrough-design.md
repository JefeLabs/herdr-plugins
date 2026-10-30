# Toolbar setup: a walk-through in the installer

Status: draft for the owner's review. Decisions marked **(owner)** are the owner's; **(my call)** are mine and open to correction.

## Why

The owner has tuned a toolbar they like:
- a large bar, Nerd Font icons, and their own tool order;
- Intents, GitHub issues, Database, Files, Jira, the browser (in a pane and in the toolbar), Editor, Usage and a Terminal.

A teammate who installs herdr-plugins gets none of that. They get `toolbar/tools.example.toml`, with a small bar, text icons and six tools, and they would have to hand-edit TOML and work out which command-line programs each tool needs.

The owner wants teammates walked through choosing from the owner's tools when they install, with the missing programs installed for them.

## Who and when (owner)

Teammates run the install script on their own machines (`curl … install.sh | sh`, with or without `--tools`). Most are on Macs using Ghostty. The owner also installs on Linux x64, where they use mise and curl rather than Homebrew.

## What it does

### The walk-through

During an interactive install, after the plugins are linked and before `tools.toml` is written, the installer shows the owner's tools as one list **(owner, agreed with the mock-up below)**:

```text
Toolbar setup: which tools do you want on your bar?
  [x] 1  Intents              capture and send intents
  [x] 2  GitHub issues        gh dash              needs: gh, gh-dash
  [x] 3  Database             lazysql
  [x] 4  Files                yazi
  [ ] 5  Jira                 jira                 needs: jira-cli
  [x] 6  Browser              terminal-browser     needs: terminal-browser
  [ ] 7  Browser in toolbar   (experimental)
  [x] 8  Editor               Neovim
  [x] 9  Usage                procs, oxker, dua    needs: procs, oxker
  [x] 10 Terminal             a shell in a pane
Type numbers to switch them on or off, Enter when done:
```

- Every tool starts ticked except Jira, which needs a Jira site and a `jira init`, and the experimental in-toolbar browser **(my call)**. A teammate who just presses Enter gets the owner's bar.
- `needs:` lists only the programs that are missing on this machine.
- After the list comes one question for everything missing **(owner: "offer to install it, skip if they say no"; one question, agreed)**:

  ```text
  Install what's missing with Homebrew? gh gh-dash terminal-browser procs oxker [Y/n]
  ```

  - **Yes:** it installs them. A tool whose program still can't be found afterwards (a failed install) is left off the bar, with the reason printed.
  - **No:** the tools that need them are left off. A teammate never gets a button that can't start.
- Then one question about icons **(my call)**: `Nerd Font icons? [Y/n]`. The pre-filled answer is yes when any of these says the terminal is Ghostty or kitty, both of which ship Nerd Font symbols:
  - `TERM` is `xterm-ghostty` or `xterm-kitty` (this one survives SSH);
  - `TERM_PROGRAM` is `ghostty`;
  - `GHOSTTY_RESOURCES_DIR` or `KITTY_WINDOW_ID` is set.

  Inside a Herdr pane `TERM` reads `xterm-256color`, but `GHOSTTY_RESOURCES_DIR` is still set from the Ghostty window that started Herdr (checked on the owner's Mac). So an install run from Herdr still pre-fills yes. The question is there for the cases detection misses, such as a Herdr pane on a remote.
- **Signing in is deferred (owner: "i will defer sign in to gh for now").** The walk-through never signs anyone in.
  - `gh dash` works only with a signed-in `gh`, and installing it (`gh extension install`) needs one too. When GitHub issues is ticked and `gh auth status` says no one is signed in, GitHub issues is left off, with: `GitHub issues: run gh auth login, then install.sh --setup to add it`.
  - Jira, when ticked, is treated the same way **(my call)**: with no `jira` configuration yet, it is left off with `Jira: run jira init, then install.sh --setup to add it`.
- It then writes `tools.toml` and prints what it wrote, plus those next steps.

### How missing programs are installed (owner: Homebrew, then mise, then curl)

The installer already detects the OS (macOS or Linux) and the architecture (arm64 or x86_64). The first of these that is present is used:

1. **Homebrew** (the owner's Macs; also Linux, if someone has it): `brew install <formula>`. The formulae are `gh`, `ankitpokhrel/jira-cli/jira-cli` (which taps automatically), `lazysql`, `yazi`, `neovim`, `procs`, `oxker`, `dua-cli`, and `terminal-browser` if it is the same app (see Left to the builder).
2. **mise** (the owner's Linux x64 box): `mise use -g <tool>`, which needs no sudo.
   - By short name: `gh`, `neovim`, `yazi`, `procs`.
   - Through mise's GitHub-release backend: `jira-cli`, `lazysql`, `oxker`, `dua`.
3. **curl:**
   - `terminal-browser` through its own install script (`https://terminal-browser.sh/install`).
   - With neither Homebrew nor mise, the installer's existing pinned, checksum-checked downloads into `~/.local/bin`, the ones `--tools` uses. That list gains `gh` and `jira-cli` for each platform.

`gh dash` always installs through `gh extension install dlvhdr/gh-dash` once `gh` is present.

**Which installs are pinned (my call, from the builder check):** the pinned-version-and-SHA-256 rule covers the installer's own downloads, the `~/.local/bin` list. Homebrew, mise, `gh extension install` and terminal-browser's own install script are each tool's own channel. They verify their own downloads, and the install question names the channel, so the teammate sees what will run. They are not pinned by us; this follows the owner's own way of installing ("Homebrew, then mise, then curl"). Offline (`--from`), only the installer's own downloads are possible.

On the owner's Mac, terminal-browser's script installed into `~/.local/share/terminal-browser` and linked `~/.local/bin/terminal-browser`. The builder confirms it uses no sudo and says which shell it needs. If it does need sudo, or it doesn't support Linux, the walk-through falls back to Homebrew where available, and otherwise leaves the Browser tools off with the reason.

**Reachability (my call):** the toolbar starts programs from its own `PATH`, not through a login shell. After installing, the walk-through checks that each program is on `PATH`. If one isn't, it prints the one line to add, as the installer already does for `~/.local/bin`. That covers mise's shims folder, `~/.local/share/mise/shims`.

### The tools.toml it writes

- It uses the owner's settings: `prefix = "ctrl-]"`, `bar = "large"`, and the owner's order (Intents, GitHub issues, Database, Files, Jira, Browser in toolbar, Editor, Browser, Usage, Terminal), keeping only the chosen tools.
- Icons are the owner's Nerd glyphs, or the current text icons when the teammate says no to Nerd icons:

  | Tool | Nerd glyph | Text icon |
  |---|---|---|
  | GitHub issues | `\U000F09EE` | ◉ |
  | Intents | `\uf0eb` | ✎ |
  | Database | `\ueace` | ▤ |
  | Files | `\uea83` | ▭ |
  | Jira | `\uef56` | ◆ |
  | Editor | `\uf36f` | ◫ |
  | Browser (pane) | `\uf488` | ◎ |
  | Browser in toolbar | `\ueaae` | ◍ |
  | Usage | `\uf463` | ▤ |
  | Terminal | `\uea85` | ▣ (my call) |

- Commands are the owner's, with nothing personal in them:
  - Intents points at this install's `herdr-intents`, as now.
  - The Usage disk pane opens `HERDR_PLUGINS_REPOS`, else `~`, as `--tools` does now.
  - The Usage process pane uses `Virtualization` on macOS and `containerd` on Linux, as now.
  - Jira gets no `JIRA_PROJECT`: `jira init` sets the project **(my call; the owner's `SKOOL` is in today's public example, which this removes)**.
  - Terminal is `command = ["clear"]` with `open = "pane"`.

### The list lives in one place (my call)

One catalog file in the repo describes each tool:
- its `[[tool]]` entry;
- its text icon and Nerd icon;
- whether it is ticked by default;
- the programs it needs, with each program's Homebrew formula, mise name and curl source.

Adding a tool to the menu later is a change to that file alone. `tools.example.toml` stays for people who set up by hand, and is updated to match the catalog's defaults.

### When there is no walk-through (my call)

- **A `tools.toml` already exists:** the installer leaves it alone and asks nothing, so upgrades never touch a teammate's edits. `install.sh --setup` runs the walk-through on purpose. It first backs the file up to `tools.toml.bak-<date-time>`, then writes the new one.
- **No terminal to answer in** (CI, or `sh` with no `/dev/tty`): no questions and no installs. It writes the pre-ticked tools whose programs are already present, with the icon mode from the same detection.
- **`--from` (offline):** the walk-through runs, but it can install only what `--tools` can install from the `--from` folder. Everything else is reported as missing and left off.
- **`--tools` (my call, from the builder check):** still downloads its six pinned programs (procs, oxker, dua, yazi, lazysql, Neovim) and nothing more. `gh` and `jira-cli` join the pinned list only for the walk-through's curl fallback. It no longer adds the Usage and Editor tools to `tools.toml`: what goes on the bar is the walk-through's job, and `install.sh --setup` adds tools later. Otherwise a `--tools` run could put back tools a teammate had unticked, and it would edit an existing `tools.toml`. Run together with the walk-through, its downloads go first, and the walk-through then finds those programs present. The README and the existing `--tools` tests change to match.

## Goals

- A teammate on a Mac with Ghostty presses Enter a few times and ends up with the owner's bar, with every program it needs installed.
- The same works on Linux x64 with mise, and on a machine with neither Homebrew nor mise.
- No hand-editing of TOML, and no button that can't start.

## Non-goals

- Signing in to GitHub or Jira: deferred by the owner. The walk-through prints the command and leaves the tool off until then.
- A setup screen inside the toolbar or inside Herdr. This is the installer only.
- Changing an existing `tools.toml` without `--setup`.
- Windows.

## Anti-goals

- Installing anything the teammate didn't agree to: the install question lists exactly what it will install.
- sudo, or writing outside the home folder (Homebrew's own prefix aside).
- A personal value of the owner's (paths, a Jira project) ending up in a teammate's config or in the public repo.
- Breaking `curl … | sh`: the questions read from `/dev/tty`, because the script itself arrives on stdin.

## Constraints

- `install.sh` is POSIX `sh` with `set -eu`, and must keep working on macOS and Linux (glibc and musl), arm64 and x86_64.
- `scripts/test-install.sh` must cover:
  - the walk-through with scripted answers;
  - each installer path (Homebrew, mise, curl) through fake `brew` and `mise` programs;
  - a "no" leaving tools off;
  - no terminal;
  - an existing `tools.toml` left alone;
  - `--setup` making a backup.

  Tests never call the real Homebrew, mise or network.
- Downloads keep the installer's rule: pinned versions, and a SHA-256 that must match, or the download isn't installed.
- No push or release without the owner's word.

## Left to the builder

- Whether the walk-through lives in `install.sh` or in a `herdr-toolbar setup` subcommand. The binary is already in the release archive, and the installer already uses it for unzipping. Rust would make the catalog (TOML), the list and the tests easier; `sh` keeps everything in one script. The questions must read `/dev/tty` either way.
- The catalog's file name and format.
- Whether Homebrew's `terminal-browser` (0.15.1, "Terminal-based web browser") is the same app the owner installed from `https://terminal-browser.sh/install` (0.13.4). If not, use the curl installer everywhere. Also check whether that installer supports Linux x64.
- The exact mise names and backends, and the pinned `gh` and `jira-cli` versions and their SHA-256s.
- The exact wording of the prompts and messages, within the mock-up above.
