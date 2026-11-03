# mise Install + `herdr-toolbar setup` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Installing with mise takes one `github:` entry plus one command. After `mise install`, running `herdr-toolbar setup` links both plugins, writes `tools.toml` and prints the key bindings, exactly as the curl installer does.

**Architecture:** `install.sh` gets `--tree <folder>`: install from an already-unpacked release (mise's copy) instead of downloading one. Everything after the download stays the same: copy into `~/.local/share/herdr-plugins`, link, migrate, walk through the tools, print the keys. `herdr-toolbar setup [args]` finds the release folder it runs from and runs `sh <release>/install.sh --tree <release> [args]`. Each archive now carries `install.sh` so that file is there. The setup steps stay in one shell script, not two implementations.

**Tech Stack:** POSIX `sh` (`install.sh`, `scripts/test-install.sh`, `scripts/dist.sh`), Rust (`toolbar` crate, `anyhow`, `tempfile` in tests).

**Spec:** none. Requirements were agreed with the owner on 2026-11-03, after checking what installing through mise needs:
- the `github:` entry needs `strip_components = 1, bin_path = "target/release"`;
- mise can't run `herdr plugin link` or write `tools.toml`;
- nothing documents installing with mise.

## Global Constraints

- Without `--tree`, `install.sh` behaves exactly as before. Every existing check in `sh scripts/test-install.sh` still passes.
- `--tree` never changes or deletes anything in the folder it copies from.
- Herdr links the plugins in `$dir` (default `~/.local/share/herdr-plugins`), never in mise's folder. So `mise upgrade` and `mise prune` can't break a link, and `tools.toml` paths stay stable.
- POSIX `sh` with `set -eu`, macOS and Linux, no sudo (as the rest of `install.sh`).
- `herdr-toolbar setup` needs no Herdr pane environment (it is run from a shell), and never uses `eprintln!` (see `main.rs`: it panics on a dead pty).
- Exact usage line: `usage: herdr-toolbar [toggle | focus | select <tool-id|index> | trust | setup [--tools] [--setup] | --version]`
- Exact messages:
  - `install.sh`:
    - `--tree needs a folder`
    - `--tree and --from cannot be used together`
    - `<tree> is not an unpacked herdr-plugins release (no target/release/herdr-toolbar)`
    - `Copied <tree> into <dir>`
    - `<dir> is already in place`
  - Rust:
    - `herdr-toolbar setup runs from an unpacked release (<folder>/target/release/herdr-toolbar), not from <exe>`
    - `<root> is a development checkout: link it with 'herdr plugin link <root>/toolbar' instead`
    - `<root> has no install.sh`
- This lands in the next release. Version numbers and the release itself are not part of this plan.

## Review Focus

1. **`setup` run from the installed copy itself.** `~/.local/share/herdr-plugins/target/release/herdr-toolbar setup` makes `--tree` the same folder as `$dir`. The old `rm -rf "$dir/toolbar"…` would delete the source. Expected: the copy is skipped with `<dir> is already in place`, then linking and the rest go on. Test: Task 1, "--tree on the install folder itself".
2. **Re-running after `mise upgrade`.** Expected: the new files replace the old ones in `$dir`, both plugins say `already linked, updated in place`, and nothing is linked twice. Test: Task 1, "--tree again after an upgrade".
3. **A path with a space** (`$HOME` or mise's data folder). Expected: it works. Test: Task 1's tree lives in `"$work/tree with space/…"`. Task 2's binary test uses `"my tree"`.
4. **`setup` from a development checkout** (`cargo build --release`, then `target/release/herdr-toolbar setup`). Expected: refused with the "development checkout" message, and the repo's `target/`, which can be several GB, is not copied. Test: Task 2, `refuses_a_development_checkout`.
5. **`--tree` and `--from` together, or `--tree` on a wrong folder.** Expected: a clear refusal with exit 1, and nothing changed. Test: Task 1, the two refusal checks.

---

### Task 1: `install.sh --tree <folder>`

**Files:**
- Modify: `install.sh`: the header comment, option parsing (lines 42–64), and the download-and-copy block (lines 117–140)
- Test: `scripts/test-install.sh`: the fake release, plus a new section before `# --- review fixes`

**Interfaces:**
- Produces: `sh install.sh --tree <folder> [--tools] [--setup]`. `<folder>` is an unpacked release (`toolbar/`, `tools/`, `target/release/herdr-toolbar`, `install.sh`). Task 2 runs it exactly that way.

- [ ] **Step 1: Put `install.sh` into the fake release, as `dist.sh` will (Task 2)**

In `scripts/test-install.sh`, after the `cp target/release/herdr-toolbar …` line in the fake-release block, add `cp install.sh "$stage/"`.

- [ ] **Step 2: Write the failing tests**

Add a new section `# --- --tree: an unpacked release (mise's copy), no download` before `# --- review fixes`. It has a helper:

```sh
run_tree() { # <home> <args...>: the unpacked release's own install.sh; a download would fail
  h=$1; shift
  out=$(env -i HOME="$h" PATH="$base_path" HERDR_PLUGINS_TTY=none \
    HERDR_PLUGINS_BASE_URL="file://$work/no-such-release" sh "$tree/install.sh" "$@" 2>&1)
  status=$?
}
tree="$work/tree with space/herdr-plugins"
mkdir -p "$tree" && cp -R "$stage/." "$tree/"
h="$work/ht"; mkdir -p "$h"; d="$h/.local/share/herdr-plugins"
```

Then add these checks, each a `pass`/`flunk` pair named as quoted:
- **"--tree installs an unpacked release without downloading":** `run_tree "$h" --tree "$tree"` gives status 0. `$d/target/release/herdr-toolbar` is executable. `$h/.fake-herdr-links` holds `[local:$d/toolbar]` and `[local:$d/tools/intents]`. `$(toml "$h")` exists. `$out` contains `Copied $tree into $d`.
- **"--tree again after an upgrade updates the copy, links stay":** write `#!/bin/sh\n# v2\n` to `$tree/target/release/herdr-intents`. Then `run_tree "$h" --tree "$tree"` gives status 0. `grep -q '# v2' "$d/target/release/herdr-intents"`. `$out` contains `toolbar: already linked, updated in place` and `tools/intents: already linked, updated in place`. `grep -c local: "$h/.fake-herdr-links"` is 2.
- **"--tree on the install folder itself":** `run_tree "$h" --tree "$d"`, running `"$tree/install.sh"` as before, gives status 0. `$out` contains `$d is already in place`. `$d/target/release/herdr-toolbar` is still executable.
- **"--tree and --from together are refused":** `run_tree "$h" --tree "$tree" --from "$rel"` gives status 1, and `$out` contains `--tree and --from cannot be used together`.
- **"--tree on a folder that is not a release is refused":** `run_tree "$h" --tree "$work/fakebin"` gives status 1, and `$out` contains `is not an unpacked herdr-plugins release`.
- **"--tree leaves its source alone":** after the checks above, `$tree/install.sh`, `$tree/toolbar/herdr-plugin.toml` and `$tree/target/release/herdr-toolbar` still exist.

- [ ] **Step 3: Run the tests and check the new ones fail**

Run: `sh scripts/test-install.sh 2>&1 | grep -E '^(ok|FAIL)'`
Expected: the six new checks print `FAIL` (`unknown option --tree`), and every earlier check prints `ok`.

- [ ] **Step 4: Implement `--tree` in `install.sh`**

- **Option parsing:** add `tree=""` next to `setup=""`. Add a `--tree)` case shaped like `--from)` that fails with `--tree needs a folder`. After the loop: `[ -z "$tree" ] || [ -z "$from" ] || fail "--tree and --from cannot be used together"`.
- **With `--tree` set,** skip the download, the checksum check and the extract (the `tmp=…` through `tar -xzf` lines). Instead:
  - fail with the "not an unpacked release" message unless `$tree/target/release/herdr-toolbar` is executable;
  - `mkdir -p "$dir"`;
  - compare `$(cd "$tree" && pwd -P)` with `$(cd "$dir" && pwd -P)`. When they are equal, `say "$dir is already in place"`;
  - otherwise `rm -rf "$dir/toolbar" "$dir/tools" "$dir/target"`, then copy each of `toolbar tools target install.sh` that exists in `$tree` into `$dir` (`cp -R`), then `say "Copied $tree into $dir"`.
- **Without `--tree`,** the existing lines run unchanged.
- **Header comment:** add a paragraph on `--tree <folder>` (an unpacked release, as mise installs it; what `herdr-toolbar setup` runs).

- [ ] **Step 5: Run the tests and check they all pass**

Run: `sh scripts/test-install.sh 2>&1 | tail -3`
Expected: no `FAIL` lines, and the script exits 0.

- [ ] **Step 6: Commit**

```bash
git add install.sh scripts/test-install.sh
git commit -m "feat(install): --tree installs from an unpacked release, as mise leaves it"
```

### Task 2: `herdr-toolbar setup`, and `install.sh` inside every archive

**Files:**
- Create: `toolbar/src/setup.rs`
- Modify: `toolbar/src/lib.rs` (`pub mod setup;`), `toolbar/src/main.rs` (`Mode`, `USAGE`, `parse_mode`, `main`, the `parse_mode_from_args` test), `scripts/dist.sh` (`build()`)
- Test: `toolbar/src/setup.rs` (`#[cfg(test)]`), `toolbar/tests/setup.rs` (new)

**Interfaces:**
- Consumes: Task 1's `sh <root>/install.sh --tree <root> [args]`.
- Produces, in `herdr_toolbar::setup`:
  - `pub fn release_root(exe: &std::path::Path) -> anyhow::Result<std::path::PathBuf>`: the canonical folder above `target/release/`.
  - `pub fn command(root: &std::path::Path, args: &[String]) -> std::process::Command`: `sh <root>/install.sh --tree <root> <args…>`.
  - `pub fn run(args: &[String]) -> anyhow::Result<u8>`: `release_root(current_exe)`, then `command(…).status()`; exit code, or 1 if killed by a signal.
  - `Mode::Setup(Vec<String>)` in `main.rs`.

- [ ] **Step 1: Write the failing unit tests in `toolbar/src/setup.rs`**

Each test builds a tree in a `tempfile::tempdir()`: `target/release/herdr-toolbar` (any file), plus `install.sh` where needed. The program path each test passes must exist, because `release_root` canonicalizes it.
- `root_of_an_unpacked_release`: with `install.sh`, `release_root(&tree.join("target/release/herdr-toolbar")).unwrap() == tree.canonicalize().unwrap()`.
- `refuses_a_development_checkout`: also create `.git/`. The error text contains `is a development checkout`.
- `refuses_a_tree_without_install_sh`: the error text contains `has no install.sh`.
- `refuses_a_binary_outside_target_release`: create and pass `tree.join("bin/herdr-toolbar")`. The error text contains `runs from an unpacked release`.
- `command_runs_install_sh_on_the_tree`: `command(root, &["--tools".into()])` has `get_program() == "sh"`, and `get_args()` collects to `[root/install.sh, "--tree", root, "--tools"]` as `OsStr`s.

In `main.rs`, extend `parse_mode_from_args` with:
- `parse_mode(&["setup".into()]) == Mode::Setup(vec![])`;
- `parse_mode(&["setup".into(), "--tools".into(), "--setup".into()]) == Mode::Setup(vec!["--tools".into(), "--setup".into()])`.

- [ ] **Step 2: Write the failing binary test `toolbar/tests/setup.rs`**

`setup_runs_the_trees_install_sh_and_passes_its_exit_code`:
- **Arrange:** `tree = tmp.join("my tree")`. Copy `env!("CARGO_BIN_EXE_herdr-toolbar")` to `tree/target/release/herdr-toolbar`, keeping it executable. Write `tree/install.sh`: `printf '%s\n' "$@" > "$(dirname "$0")/args"; exit 7`.
- **Act:** `std::process::Command::new(<the copy>).args(["setup", "--tools"]).status()`.
- **Assert:** `code() == Some(7)`, and `tree/args` reads as the lines `--tree`, `<tree.canonicalize()>`, `--tools`.

- [ ] **Step 3: Run them and check they fail**

Run: `cargo test -p herdr-toolbar setup 2>&1 | tail -5`
Expected: a compile error (`unresolved module setup` / no `Mode::Setup`).

- [ ] **Step 4: Implement**

- **`setup.rs`:** the three functions above.
  - `release_root`: canonicalize `exe`. Require that its parent's name is `release` and its grandparent's is `target`. The root is the folder above `target`. Then check `.git` before `install.sh`, using the exact messages from Global Constraints.
  - `run`: `std::env::current_exe()`, then `release_root`, then `command(…).status()`.
- **`main.rs`:**
  - add `[a, rest @ ..] if a == "setup" => Mode::Setup(rest.to_vec())`;
  - dispatch like `Mode::Wrap`: `Ok(code) => return ExitCode::from(code)`;
  - set the new `USAGE` text.
- **`lib.rs`:** `pub mod setup;`

- [ ] **Step 5: Ship `install.sh` inside every archive**

In `scripts/dist.sh` `build()`, add `cp install.sh "$root/"` before `pack`. After `check_clean`, add a guard: `tar -tzf "dist/herdr-plugins-$name.tar.gz" | grep -qx 'herdr-plugins/install.sh' || { echo "dist: $name has no install.sh" >&2; exit 1; }`. Keep `cp install.sh dist/` as well: the curl installer is still a release asset.

- [ ] **Step 6: Run everything and check it passes**

Run: `cargo test -p herdr-toolbar && cargo clippy -p herdr-toolbar --all-targets -- -D warnings && sh scripts/test-install.sh | tail -1`
Expected: all tests pass, clippy is clean, and test-install has no `FAIL`.
Run: `sh scripts/dist.sh && tar -tzf dist/herdr-plugins-macos-arm64.tar.gz | grep -x herdr-plugins/install.sh`
Expected: `dist/ is ready for v…`, then `herdr-plugins/install.sh`.

- [ ] **Step 7: Commit**

```bash
git add toolbar/src/setup.rs toolbar/src/lib.rs toolbar/src/main.rs toolbar/tests/setup.rs scripts/dist.sh
git commit -m "feat(toolbar): herdr-toolbar setup runs the release's install.sh on the folder it came from"
```

### Task 3: README: installing with mise

**Files:**
- Modify: `README.md`: a new `### With mise` subsection at the end of `## Install` (before `### Choosing the toolbar's tools`), and one sentence in `## Release`.

- [ ] **Step 1: Write `### With mise`**

It contains, in this order:
1. The config block, with the version left as the latest release tag at the time of writing:
   ```toml
   [tools]
   "github:herdrdev/herdr" = "latest"
   "github:JefeLabs/herdr-plugins" = { version = "<latest>", strip_components = 1, bin_path = "target/release" }
   ```
2. Why the two options: the archive keeps the programs in `target/release/`, where each plugin's manifest looks for them.
3. Why the version is pinned: mise's list of releases can lag a new release by hours.
4. `mise install`, then `herdr-toolbar setup`. Setup runs the release's own `install.sh` on mise's copy. It copies the plugins into `~/.local/share/herdr-plugins` and links them there, so `mise upgrade` and `mise prune` never break Herdr's links. It walks you through `tools.toml` and prints the key bindings. `--tools` and `--setup` work as with the curl installer.
5. To update: change the version, run `mise install`, then `herdr-toolbar setup` again.
6. With mise, `herdr-intents` and `herdr-doc` are on your `PATH` too.

- [ ] **Step 2: Update `## Release`**

Add one sentence: each archive also carries `install.sh`, which `herdr-toolbar setup` runs.

- [ ] **Step 3: Check**

Run: `grep -n -E 'herdr-toolbar setup|strip_components' README.md`
Expected: both appear under `### With mise`.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: install with mise, then herdr-toolbar setup"
```

## After merge (not tasks: the owner's word first)

- **Release:** version bump PR, `scripts/dist.sh`, GitHub release.
- **Check on a real machine:**
  - in a throwaway `HOME`, using the "probe real Herdr safely" recipe, run `mise install` from the README entry;
  - run `herdr-toolbar setup`;
  - check that `herdr plugin list` shows both plugins `[local:~/.local/share/herdr-plugins/…]`, and that the toolbar opens.
- **Dotfiles:** the draft mise config's `[tasks.setup]` becomes `herdr-toolbar setup`, plus `gh auth login` and `terminal-browser setup`.
