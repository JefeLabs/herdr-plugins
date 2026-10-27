#!/bin/sh
# Installs the Herdr toolbar and capabilities plugins from a GitHub release:
#
#   curl -fsSL https://github.com/JefeLabs/herdr-plugins/releases/latest/download/install.sh | sh
#
# On a machine without internet access, copy over install.sh, SHA256SUMS and
# the archive for that machine (herdr-plugins-<os>-<arch>.tar.gz), then run
#
#   sh install.sh --from <folder holding them>
#
# Add --tools to also install the command-line tools the toolbar uses
# (procs, oxker, dua, yazi, lazysql) and a Usage tool (one icon, three panes):
#
#   curl -fsSL https://github.com/JefeLabs/herdr-plugins/releases/latest/download/install.sh | sh -s -- --tools
#
# Tools come from their own GitHub releases at pinned versions, and each
# download must match the SHA-256 written below or it is not installed. Tools
# already on PATH are left alone. Offline, put their archives in the --from
# folder too.
#
# No sudo, Rust or compiler needed. Settings (environment variables):
#   HERDR_PLUGINS_VERSION   a release tag such as v0.1.0 (default: the latest)
#   HERDR_PLUGINS_DIR       where the plugins go (default: ~/.local/share/herdr-plugins)
#   HERDR_PLUGINS_TOOLS     1 does what --tools does
#   HERDR_PLUGINS_BIN       where tools go (default: ~/.local/bin)
#   HERDR_PLUGINS_REPOS     the folder the Usage tool's disk pane opens (default: ~)
#   HERDR_PLUGINS_BASE_URL  download from here instead of GitHub (for testing)
#   HERDR_PLUGINS_TOOLS_MANIFEST  a tools table to use instead of the one below (for testing)
set -eu

from="${HERDR_PLUGINS_FROM:-}"
tools="${HERDR_PLUGINS_TOOLS:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --from)
      [ -n "${2:-}" ] || { echo "herdr-plugins: --from needs a folder" >&2; exit 1; }
      from=$2
      shift 2
      ;;
    --tools)
      tools=1
      shift
      ;;
    *)
      echo "herdr-plugins: unknown option $1" >&2
      exit 1
      ;;
  esac
done

repo="JefeLabs/herdr-plugins"
version="${HERDR_PLUGINS_VERSION:-latest}"
dir="${HERDR_PLUGINS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/herdr-plugins}"
base="${HERDR_PLUGINS_BASE_URL:-}"
if [ -z "$base" ]; then
  if [ "$version" = latest ]; then
    base="https://github.com/$repo/releases/latest/download"
  else
    base="https://github.com/$repo/releases/download/$version"
  fi
fi

say() { printf '%s\n' "$*"; }
fail() { printf 'herdr-plugins: %s\n' "$*" >&2; exit 1; }

command -v herdr >/dev/null 2>&1 || fail "Herdr is not installed: install it first (https://herdr.dev)"
case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) fail "no build for $(uname -s)" ;;
esac
case "$(uname -m)" in
  aarch64 | arm64) arch=arm64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) fail "no build for $(uname -m)" ;;
esac
asset="herdr-plugins-$os-$arch.tar.gz"

fetch() {
  if [ -n "$from" ]; then
    cp "$from/$(basename "$1")" "$2"
  elif command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  else
    wget -qO "$2" "$1"
  fi
}
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "$from" ]; then
  [ -d "$from" ] || fail "no folder $from"
  say "Installing $asset from $from"
else
  say "Downloading $asset ($version)"
fi
fetch "$base/$asset" "$tmp/$asset" || fail "could not get $asset${from:+ from $from}"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "could not get SHA256SUMS${from:+ from $from}"
want=$(grep " $asset\$" "$tmp/SHA256SUMS" | cut -d ' ' -f 1)
[ -n "$want" ] || fail "SHA256SUMS has no line for $asset"
[ "$(sha256 "$tmp/$asset")" = "$want" ] || fail "checksum mismatch for $asset: not installing"

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
# replace the files but keep the folder: Herdr links to it
rm -rf "$dir/toolbar" "$dir/tools" "$dir/target"
cp -R "$tmp/herdr-plugins/." "$dir/"
say "Installed into $dir"

for plugin in toolbar tools/capabilities; do
  if herdr plugin list 2>/dev/null | grep -q "local:$dir/$plugin\]"; then
    say "  $plugin: already linked, updated in place"
  elif herdr plugin link "$dir/$plugin" >/dev/null 2>&1; then
    say "  $plugin: linked"
  else
    say "  $plugin: could not link (another copy may be linked; see 'herdr plugin list')"
  fi
done

# a first toolbar config, pointing at this install's capabilities tool
config=$(herdr plugin config-dir jefelabs.toolbar 2>/dev/null || true)
if [ -n "$config" ] && [ ! -f "$config/tools.toml" ]; then
  mkdir -p "$config"
  sed "s|/absolute/path/to/herdr-plugins|$dir|" "$dir/toolbar/tools.example.toml" > "$config/tools.toml"
  say "Wrote $config/tools.toml (edit it to choose your tools)"
fi

# Third-party tools for --tools: name, platform, download, the SHA-256 it
# must match (GitHub's recorded digest of that upload), and the programs
# inside the archive. oxker has no Intel Mac build.
builtin_tools() {
  cat <<'TOOLS'
procs linux-x86_64 https://github.com/dalance/procs/releases/download/v0.14.12/procs-v0.14.12-x86_64-linux.zip 81964faf9d5cd0e9f399d5d0954fb6fe4d4d9f7d3cbec4507633df5a45b11713 procs
procs linux-arm64 https://github.com/dalance/procs/releases/download/v0.14.12/procs-v0.14.12-aarch64-linux.zip 74b4f82acc6fe553cbd5af20ba0baac420b42bef9bb0cbb38fef14b8317d9f91 procs
procs macos-arm64 https://github.com/dalance/procs/releases/download/v0.14.12/procs-v0.14.12-aarch64-mac.zip 20c7a33426ed7a43c3e13a48f2d1866ecc979a8ad46733f86ddc78ab7f12d49c procs
procs macos-x86_64 https://github.com/dalance/procs/releases/download/v0.14.12/procs-v0.14.12-x86_64-mac.zip 2c276f40c99051984942a5b135b74ce4bc9184984a1543dddfcf275bde71d8d4 procs
oxker linux-x86_64 https://github.com/mrjackwills/oxker/releases/download/v0.13.4/oxker_linux_x86_64.tar.gz e077cbf2e978a7a6c9802d453491944ba3635482097e2549e58c7e81df389643 oxker
oxker linux-arm64 https://github.com/mrjackwills/oxker/releases/download/v0.13.4/oxker_linux_aarch64.tar.gz 850bd6c772df85abb67ab08c2add1e46f663268b8788a4a7184d2ed28fce083b oxker
oxker macos-arm64 https://github.com/mrjackwills/oxker/releases/download/v0.13.4/oxker_apple_darwin_aarch64.tar.gz cc36d209a2130d24c9f1ac9760505448068dc06e39e4a808d88147a09b7f3a05 oxker
dua linux-x86_64 https://github.com/Byron/dua-cli/releases/download/v2.46.0/dua-v2.46.0-x86_64-unknown-linux-musl.tar.gz 4f284a388a063a99f938c6e0f5ea8d932868e1f9a4224622d97488f4ec671b41 dua-v2.46.0-x86_64-unknown-linux-musl/dua
dua linux-arm64 https://github.com/Byron/dua-cli/releases/download/v2.46.0/dua-v2.46.0-aarch64-unknown-linux-musl.tar.gz a7ba6bce1f1d16392b4c9f385e7439deda71627ed3c55eb09d5dfcbe6b481a58 dua-v2.46.0-aarch64-unknown-linux-musl/dua
dua macos-arm64 https://github.com/Byron/dua-cli/releases/download/v2.46.0/dua-v2.46.0-aarch64-apple-darwin.tar.gz 857af70a6be7fe3d22611a3aa082f17789435ad0b89808845ef13f85789e8e1b dua-v2.46.0-aarch64-apple-darwin/dua
dua macos-x86_64 https://github.com/Byron/dua-cli/releases/download/v2.46.0/dua-v2.46.0-x86_64-apple-darwin.tar.gz 1fe7c6bd3cc1f3d37ee78d22a9f5ec5bd6d33eb24e5c8bf5864aa7efa03f8324 dua-v2.46.0-x86_64-apple-darwin/dua
yazi linux-x86_64 https://github.com/sxyazi/yazi/releases/download/v26.9.1/yazi-x86_64-unknown-linux-musl.zip 9b9c39decccf8cb0ff53a7d637d38f8a79d93bbd0099f4ea9c619ef6bb392f5d yazi-x86_64-unknown-linux-musl/yazi yazi-x86_64-unknown-linux-musl/ya
yazi linux-arm64 https://github.com/sxyazi/yazi/releases/download/v26.9.1/yazi-aarch64-unknown-linux-musl.zip dd569daecaae914185f295634109295ccd25c1b42b02eb89a74f651970024f2e yazi-aarch64-unknown-linux-musl/yazi yazi-aarch64-unknown-linux-musl/ya
yazi macos-arm64 https://github.com/sxyazi/yazi/releases/download/v26.9.1/yazi-aarch64-apple-darwin.zip 3921182a21cceb0a505e5dac578e1487d48104caa5f114e9f8adf40b5a7289a9 yazi-aarch64-apple-darwin/yazi yazi-aarch64-apple-darwin/ya
yazi macos-x86_64 https://github.com/sxyazi/yazi/releases/download/v26.9.1/yazi-x86_64-apple-darwin.zip 36e09036fcc446488d876d139a5e303f2443b82cfb6ac7dfcb43892d6fe6fa20 yazi-x86_64-apple-darwin/yazi yazi-x86_64-apple-darwin/ya
lazysql linux-x86_64 https://github.com/jorgerojas26/lazysql/releases/download/v0.7.0/lazysql_Linux_x86_64.tar.gz ebc3edeb848c7e557f88177cf6b2ca7e492ef158e2aa1dce18eb137d1d0726fc lazysql
lazysql linux-arm64 https://github.com/jorgerojas26/lazysql/releases/download/v0.7.0/lazysql_Linux_arm64.tar.gz 2157e58f107bd9c250ad4d106e31d3f3484cfd93280b1c59f25a2dd0965e9128 lazysql
lazysql macos-arm64 https://github.com/jorgerojas26/lazysql/releases/download/v0.7.0/lazysql_Darwin_arm64.tar.gz b6e6ac70d67f737cb8cf8e88a076e697f0b171518adc31d9c88f4e06856c81a1 lazysql
lazysql macos-x86_64 https://github.com/jorgerojas26/lazysql/releases/download/v0.7.0/lazysql_Darwin_x86_64.tar.gz 9a070da390bc024ee9da14435ef3801ef7fd25c6387458fea9a3d0f549ad46b4 lazysql
TOOLS
}
tool_rows() {
  if [ -n "${HERDR_PLUGINS_TOOLS_MANIFEST:-}" ]; then
    cat "$HERDR_PLUGINS_TOOLS_MANIFEST"
  else
    builtin_tools
  fi
}
unzip_member() { # <zip> <member> <folder>
  if command -v unzip >/dev/null 2>&1; then
    unzip -o -q "$1" "$2" -d "$3"
  elif command -v python3 >/dev/null 2>&1; then
    python3 -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extract(sys.argv[2], sys.argv[3])' "$1" "$2" "$3"
  else
    return 1
  fi
}

refused=""
if [ -n "$tools" ]; then
  bin="${HERDR_PLUGINS_BIN:-$HOME/.local/bin}"
  mkdir -p "$bin"
  say ""
  say "Tools (into $bin):"
  for name in procs oxker dua yazi lazysql; do
    if command -v "$name" >/dev/null 2>&1; then
      say "  $name: already installed ($(command -v "$name"))"
      continue
    fi
    row=$(tool_rows | awk -v n="$name" -v p="$os-$arch" '$1 == n && $2 == p' | head -n 1)
    if [ -z "$row" ]; then
      say "  $name: no build for $os-$arch; install it another way (e.g. brew install $name)"
      continue
    fi
    # shellcheck disable=SC2086 # the row's fields, split on spaces
    set -- $row
    url=$3
    want=$4
    shift 4
    file="$tmp/$(basename "$url")"
    if ! fetch "$url" "$file"; then
      say "  $name: could not get $(basename "$url")${from:+ from $from}"
      refused="$refused $name"
      continue
    fi
    if [ "$(sha256 "$file")" != "$want" ]; then
      say "  $name: checksum mismatch for $name, not installing"
      refused="$refused $name"
      continue
    fi
    unpacked="$tmp/unpacked-$name"
    mkdir -p "$unpacked"
    for member in "$@"; do
      case "$file" in
        *.zip) unzip_member "$file" "$member" "$unpacked" ;;
        *) tar -xzf "$file" -C "$unpacked" "$member" ;;
      esac || { say "  $name: could not unpack $member"; refused="$refused $name"; continue 2; }
      cp "$unpacked/$member" "$bin/$(basename "$member")"
      chmod 755 "$bin/$(basename "$member")"
    done
    say "  $name: installed"
  done
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) say "  $bin is not on your PATH: add  export PATH=\"$bin:\$PATH\"  to your shell's startup file" ;;
  esac

  # the Usage tool: one icon, three panes stacked beside the work pane
  have() { command -v "$1" >/dev/null 2>&1 || [ -x "$bin/$1" ]; }
  if [ -n "$config" ] && [ -f "$config/tools.toml" ]; then
    if grep -q '^id *= *"usage"' "$config/tools.toml"; then
      :
    elif have procs && have oxker && have dua; then
      if [ "$os" = macos ]; then vm=Virtualization; else vm=containerd; fi
      repos=$(printf '%s' "${HERDR_PLUGINS_REPOS:-$HOME}" | sed 's/\\/\\\\/g; s/"/\\"/g')
      cat >> "$config/tools.toml" <<EOF

# Added by the herdr-plugins installer: one press opens three panes stacked
# beside the work pane (processes, Docker containers, disk use); the next
# press closes them.
[[tool]]
id    = "usage"
name  = "Usage"
icon  = "▤"
open  = "pane"
panes = [
  ["procs", "--watch-interval", "2", "--sortd", "cpu", "--insert", "ReadBytes", "--insert", "WriteBytes",
   "--or", "herdr", "docker", "$vm"],
  ["oxker"],
  ["dua", "i", "$repos"],
]
EOF
      say "  Added the Usage tool (▤) to $config/tools.toml"
    else
      say "  Usage tool not added: it needs procs, oxker and dua"
    fi
  fi

  hinted=""
  hint() {
    if [ -z "$hinted" ]; then
      say ""
      say "Not installed by this script (the default toolbar also lists these):"
      hinted=1
    fi
    say "  $1"
  }
  gh extension list 2>/dev/null | grep -q gh-dash ||
    hint "gh dash:           gh extension install dlvhdr/gh-dash   (needs the GitHub CLI)"
  command -v jira >/dev/null 2>&1 ||
    hint "jira:              see https://github.com/ankitpokhrel/jira-cli"
  command -v terminal-browser >/dev/null 2>&1 ||
    hint "terminal-browser:  curl -fsSL https://terminal-browser.sh/install | bash"
fi

cat <<'KEYS'

Add these to your Herdr config.toml, then run: herdr server reload-config

[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "jefelabs.toolbar.toggle"

[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "jefelabs.toolbar.focus"

[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "jefelabs.capabilities.add"

Run this installer again to update.
KEYS

[ -z "$refused" ] || fail "not installed:$refused"
