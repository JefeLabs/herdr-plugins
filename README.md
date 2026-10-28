# herdr-plugins

Jefelabs' plugins for [Herdr](https://herdr.dev), one folder per plugin, in a single Cargo workspace.

## Install

On macOS or Linux (arm64 or x86_64), with Herdr already installed:

```sh
curl -fsSL https://github.com/JefeLabs/herdr-plugins/releases/latest/download/install.sh | sh
```

It downloads the build for your machine from the latest release, checks it against the release's `SHA256SUMS`, puts both plugins in `~/.local/share/herdr-plugins`, links them with `herdr plugin link`, writes a first toolbar `tools.toml` if you have none, and prints the Herdr key bindings to add. No sudo, Rust or compiler is needed: the Linux builds are static. Run it again to update; `HERDR_PLUGINS_VERSION=v0.1.0` installs a given release.

To also install the command-line tools the toolbar uses, add `--tools`:

```sh
curl -fsSL https://github.com/JefeLabs/herdr-plugins/releases/latest/download/install.sh | sh -s -- --tools
```

That installs procs, oxker, dua, yazi, lazysql and Neovim into `~/.local/bin` (`HERDR_PLUGINS_BIN` changes it) from their own GitHub releases, at versions pinned in `install.sh`. Each download must match the SHA-256 written there, or that tool is not installed and the run fails. Tools already on your `PATH` are left alone. It then adds a Usage tool (▤) to the toolbar: one press opens three panes stacked beside the work pane (processes, Docker containers, disk use), the next press closes them. The disk pane opens `HERDR_PLUGINS_REPOS`, or your home folder. Neovim's release is a folder (the program and its runtime files): it is unpacked into `~/.local/share/herdr-plugins/vendor/nvim` and `nvim` is linked into `~/.local/bin`. Its Linux build needs glibc, so on Alpine or another musl system it is skipped with a hint to use the package manager. With Neovim in place the installer also adds an Editor tool (◫): Neovim in a pane beside the work pane, listening on `~/.cache/nvim-editor-<workspace id>.sock`, so `nvim --server <that socket> --remote <file>` opens a file in it from anywhere. Your own Neovim configuration is not copied. For gh dash, jira and terminal-browser it prints how to install them. Offline, put the tools' archives in the `--from` folder as well. `sh scripts/test-install.sh` tests the installer offline.

Without internet access (a closed network), copy `install.sh`, `SHA256SUMS` and the archive for that machine from a release (or from `dist/`) into one folder there, and run `sh install.sh --from <that folder>`. It checks and installs the same way and makes no network calls.

## Develop

Link one for development with `herdr plugin link <this checkout>/<plugin>` after `cargo build --release`.

## Release

`scripts/dist.sh` builds both plugins for linux-arm64, linux-x86_64, macos-arm64 and macos-x86_64 and packages them, with `SHA256SUMS` and `install.sh`, into `dist/`. Linux builds are linked with Rust's own `rust-lld` against musl, so no cross compiler is needed. Upload `dist/*` as the assets of a GitHub release.

## Plugins

- [`toolbar/`](toolbar/README.md): a right-hand activity bar of tools, each hosted in its own terminal.
- [`tools/intents/`](tools/intents/README.md): capture, refine (with an agent) and send intents to agents.
