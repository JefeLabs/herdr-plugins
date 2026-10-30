#!/bin/sh
# Installs the Herdr toolbar and intents plugins from a GitHub release:
#
#   curl -fsSL https://github.com/JefeLabs/herdr-plugins/releases/latest/download/install.sh | sh
#
# On a machine without internet access, copy over install.sh, SHA256SUMS and
# the archive for that machine (herdr-plugins-<os>-<arch>.tar.gz), then run
#
#   sh install.sh --from <folder holding them>
#
# Add --tools to also install the command-line tools the toolbar uses
# (procs, oxker, dua, yazi, lazysql, Neovim), a Usage tool (one icon, three
# panes) and an Editor tool (Neovim in a pane, reachable for remote opens):
#
#   curl -fsSL https://github.com/JefeLabs/herdr-plugins/releases/latest/download/install.sh | sh -s -- --tools
#
# Tools come from their own GitHub releases at pinned versions, and each
# download must match the SHA-256 written below or it is not installed. Tools
# already on PATH are left alone. Offline, put their archives in the --from
# folder too.
#
# On a terminal it then walks you through the toolbar's tools: tick the ones
# you want, and it writes the toolbar's tools.toml. A tools.toml you already
# have is left alone; add --setup to be walked through again (the old file is
# kept as tools.toml.bak-<date>).
#
# No sudo, Rust or compiler needed. Settings (environment variables):
#   HERDR_PLUGINS_VERSION   a release tag such as v0.1.0 (default: the latest)
#   HERDR_PLUGINS_DIR       where the plugins go (default: ~/.local/share/herdr-plugins)
#   HERDR_PLUGINS_TOOLS     1 does what --tools does
#   HERDR_PLUGINS_BIN       where tools go (default: ~/.local/bin)
#   HERDR_PLUGINS_REPOS     the folder the Usage tool's disk pane opens (default: ~)
#   HERDR_PLUGINS_BASE_URL  download from here instead of GitHub (for testing)
#   HERDR_PLUGINS_TOOLS_MANIFEST  a tools table to use instead of the one below (for testing)
#   HERDR_PLUGINS_LIBC      glibc or musl, instead of detecting it (for testing)
#   HERDR_PLUGINS_TTY       a file of answers for the walk-through, or none to
#                           skip it (for testing; default: the terminal)
set -eu

from="${HERDR_PLUGINS_FROM:-}"
tools="${HERDR_PLUGINS_TOOLS:-}"
setup=""
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
    --setup)
      setup=1
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

# Where the walk-through reads its answers: the terminal (when piped from
# curl, stdin is this script, so never stdin), a file for tests, or nowhere.
answers_from=""
case "${HERDR_PLUGINS_TTY:-}" in
  none) ;;
  "") if { : </dev/tty; } 2>/dev/null; then answers_from=/dev/tty; fi ;;
  *) answers_from=$HERDR_PLUGINS_TTY ;;
esac
[ -z "$setup" ] || [ -n "$answers_from" ] || fail "--setup needs a terminal to ask in"

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

for plugin in toolbar tools/intents; do
  if herdr plugin list 2>/dev/null | grep -q "local:$dir/$plugin\]"; then
    say "  $plugin: already linked, updated in place"
  elif herdr plugin link "$dir/$plugin" >/dev/null 2>&1; then
    say "  $plugin: linked"
  else
    say "  $plugin: could not link (another copy may be linked; see 'herdr plugin list')"
  fi
done

# the toolbar's config folder; its tools.toml is written after the tools
config=$(herdr plugin config-dir jefelabs.toolbar 2>/dev/null || true)

# The intents plugin used to be jefelabs.capabilities (folder tools/capabilities,
# program herdr-capabilities). Unlink the old plugin, whose folder is gone now,
# and point the toolbar and Herdr's key binding at the new names, keeping a
# `.before-intents` copy of each file changed. The plugin carries its own
# settings over the first time it runs.
if herdr plugin list 2>/dev/null | grep -q 'jefelabs\.capabilities'; then
  if herdr plugin unlink jefelabs.capabilities >/dev/null 2>&1; then
    say "  Unlinked the old jefelabs.capabilities plugin (it is jefelabs.intents now)"
  fi
fi
rename_in() { # <file> <grep pattern of old names> <sed program>
  if [ -f "$1" ] && grep -q "$2" "$1"; then
    cp "$1" "$1.before-intents"
    sed -e "$3" "$1" > "$1.tmp" && mv "$1.tmp" "$1"
    say "  Updated $1 for the new name (old copy: $1.before-intents)"
  fi
}
if [ -n "$config" ]; then
  rename_in "$config/tools.toml" 'herdr-capabilities\|^id *= *"capabilities"' \
    's#/herdr-capabilities"#/herdr-intents"#g
     s/^\(id[[:space:]]*=[[:space:]]*\)"capabilities"/\1"intents"/
     s/^\(name[[:space:]]*=[[:space:]]*\)"Capabilities"/\1"Intents"/
     /^order/s/"capabilities"/"intents"/'
fi
rename_in "${HERDR_CONFIG_PATH:-$HOME/.config/herdr/config.toml}" 'jefelabs\.capabilities\.add' \
  's/jefelabs\.capabilities\.add/jefelabs.intents.add/g'

# Third-party tools for --tools: name, platform, download, the SHA-256 it
# must match (GitHub's recorded digest of that upload), and the programs
# inside the archive. `dir=<folder>:<program>` unpacks the whole folder (a
# program with runtime files, like Neovim) and links the program into the bin
# folder; `needs=glibc` skips the row on musl systems such as Alpine, where
# that build cannot run. oxker has no Intel Mac build.
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
nvim linux-x86_64 https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-linux-x86_64.tar.gz 474430d53e6264f6d6dd18db42d6dc9df3a1b56ca9e88a325bbf860e1a811d87 dir=nvim-linux-x86_64:bin/nvim needs=glibc
nvim linux-arm64 https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-linux-arm64.tar.gz 8f1f64a0bdb97247034038c3823c6cbad5bdf9ecd5751b85494b71c3ee04c815 dir=nvim-linux-arm64:bin/nvim needs=glibc
nvim macos-arm64 https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-macos-arm64.tar.gz 1dbd148222b051ba6c307c4d17a318ed083df018a4f6a5a0ce46955ffc9ab87b dir=nvim-macos-arm64:bin/nvim
nvim macos-x86_64 https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-macos-x86_64.tar.gz 481424f1dbc85f637d57e47d27d042aa9c883e5a4fe9ee64fc4ee9573b221998 dir=nvim-macos-x86_64:bin/nvim
TOOLS
}
tool_rows() {
  if [ -n "${HERDR_PLUGINS_TOOLS_MANIFEST:-}" ]; then
    cat "$HERDR_PLUGINS_TOOLS_MANIFEST"
  else
    builtin_tools
  fi
}
# procs and yazi ship only as .zip: unzip, else python3, else the toolbar
# binary this script just installed (`herdr-toolbar unzip`), so a minimal
# system with neither still gets them
unzip_member() { # <zip> <member> <folder>
  if command -v unzip >/dev/null 2>&1; then
    unzip -o -q "$1" "$2" -d "$3"
  elif command -v python3 >/dev/null 2>&1; then
    python3 -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extract(sys.argv[2], sys.argv[3])' "$1" "$2" "$3"
  elif [ -x "$dir/target/release/herdr-toolbar" ]; then
    "$dir/target/release/herdr-toolbar" unzip "$1" "$2" "$3"
  else
    return 1
  fi
}

# glibc unless this is a musl system (Alpine): release builds for glibc do not run there
libc="${HERDR_PLUGINS_LIBC:-}"
if [ -z "$libc" ]; then
  if [ "$os" = linux ] && ls /lib/ld-musl-* >/dev/null 2>&1; then libc=musl; else libc=glibc; fi
fi

refused=""
if [ -n "$tools" ]; then
  bin="${HERDR_PLUGINS_BIN:-$HOME/.local/bin}"
  mkdir -p "$bin"
  say ""
  say "Tools (into $bin):"
  for name in procs oxker dua yazi lazysql nvim; do
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
    members=""
    needs=""
    for field in "$@"; do
      case "$field" in
        needs=*) needs=${field#needs=} ;;
        *) members="$members $field" ;;
      esac
    done
    if [ "$needs" = glibc ] && [ "$libc" = musl ]; then
      say "  $name: its release build needs glibc, which this system lacks; install it with your package manager (e.g. apk add neovim)"
      continue
    fi
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
    for member in $members; do
      case "$member" in
        dir=*)
          # a whole folder, kept beside the plugins, with its program linked
          spec=${member#dir=}
          folder=${spec%%:*}
          program=${spec#*:}
          home="$dir/vendor/$name"
          tar -xzf "$file" -C "$unpacked" "$folder" ||
            { say "  $name: could not unpack $folder"; refused="$refused $name"; continue 2; }
          rm -rf "$home"
          mkdir -p "$home"
          mv "$unpacked/$folder" "$home/"
          ln -sf "$home/$folder/$program" "$bin/$(basename "$program")"
          ;;
        *)
          case "$file" in
            *.zip) unzip_member "$file" "$member" "$unpacked" ;;
            *) tar -xzf "$file" -C "$unpacked" "$member" ;;
          esac || {
            say "  $name: could not unpack $member (from $(basename "$file"))"
            refused="$refused $name"
            continue 2
          }
          cp "$unpacked/$member" "$bin/$(basename "$member")"
          chmod 755 "$bin/$(basename "$member")"
          ;;
      esac
    done
    say "  $name: installed"
  done
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) say "  $bin is not on your PATH: add  export PATH=\"$bin:\$PATH\"  to your shell's startup file" ;;
  esac

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

# --- the toolbar's tools.toml, from the catalog in toolbar/setup: the
# ticked tools whose programs are here (see the walk-through below)
catalog="$dir/toolbar/setup"
if [ "$os" = macos ]; then vm=Virtualization; else vm=containerd; fi
present() { # <program>: on PATH, in the bin folder, or (gh-dash) a gh extension
  check=$(awk -F'|' -v p="$1" '$1 == p { print $2 }' "$catalog/programs")
  [ -n "$check" ] || check=$1
  case "$check" in
    gh-dash) command -v gh >/dev/null 2>&1 && gh extension list 2>/dev/null | grep -q gh-dash ;;
    *) command -v "$check" >/dev/null 2>&1 || [ -x "${HERDR_PLUGINS_BIN:-$HOME/.local/bin}/$check" ] ;;
  esac
}
nerd_default() { # Ghostty or kitty, which ship Nerd Font symbols (also inside Herdr)
  case "${TERM:-}" in xterm-ghostty | xterm-kitty) return 0 ;; esac
  [ "${TERM_PROGRAM:-}" = ghostty ] && return 0
  [ -n "${GHOSTTY_RESOURCES_DIR:-}" ] || [ -n "${KITTY_WINDOW_ID:-}" ]
}
toml_text() { printf '%s' "$1" | sed 's/[\\"]/\\&/g'; }  # inside a TOML "string"
sed_text() { printf '%s' "$1" | sed 's/[\\|&]/\\&/g'; }   # a sed s|..|..| replacement
write_toolbar() { # <file> <nerd|text> <tool id>...
  file=$1 mode=$2
  shift 2
  order=""
  for id in "$@"; do order="$order${order:+, }\"$id\""; done
  if [ "$mode" = nerd ]; then field=6; else field=7; fi
  intents=$(sed_text "$(toml_text "$dir/target/release/herdr-intents")")
  repos=$(sed_text "$(toml_text "${HERDR_PLUGINS_REPOS:-$HOME}")")
  {
    printf '%s\n' "# herdr-toolbar tools, written by the herdr-plugins installer. Run it again" \
      "# with --setup to choose again, or edit this file (see the toolbar README)." "" \
      'prefix = "ctrl-]"' 'bar    = "large"' "order  = [$order]"
    for id in "$@"; do
      icon=$(sed_text "$(awk -F'|' -v i="$id" -v f="$field" '$1 == i { print $f }' "$catalog/tools")")
      printf '\n'
      sed -e "s|@ICON@|$icon|" -e "s|@INTENTS@|$intents|" -e "s|@REPOS@|$repos|" -e "s|@VM@|$vm|" \
        "$catalog/$id.toml"
    done
  } > "$file.tmp" && mv "$file.tmp" "$file"
}
ask() { # <variable>: the next answer (empty once the answers run out)
  if [ -z "${asking:-}" ]; then
    exec 3<"$answers_from"
    asking=1
  fi
  IFS= read -r "$1" <&3 || eval "$1="
  # a typed answer ends its own line; one read from a file does not
  [ "$answers_from" = /dev/tty ] || printf '\n'
}
missing_of() { awk -F'|' -v i="$1" '$1 == i { print $2 }' "$tmp/setup-missing"; }
choose_tools() { # the list: numbers switch tools on and off until an empty answer
  while :; do
    say ""
    say "Toolbar setup: which tools do you want on your bar?"
    n=0
    while IFS='|' read -r id name summary _ticked _needs _nerd _text; do
      n=$((n + 1))
      case "$ticks" in *" $id "*) mark=x ;; *) mark=" " ;; esac
      missing=$(missing_of "$id")
      printf '  [%s] %-2s %-20s %-22s%s\n' "$mark" "$n" "$name" "$summary" "${missing:+needs: $missing}"
    done < "$tmp/setup-tools"
    printf '%s ' "Type numbers to switch them on or off, Enter when done:"
    ask reply
    [ -n "$reply" ] || return 0
    set -f
    for word in $reply; do
      k=$(printf '%s' "$word" | sed 's/^0*//')
      case "$k" in
        '' | *[!0-9]*) say "  ignored: $word" ;;
        *)
          if [ "$k" -le "$count" ]; then
            id=$(sed -n "${k}p" "$tmp/setup-tools" | cut -d'|' -f1)
            case "$ticks" in
              *" $id "*) ticks=$(printf '%s' "$ticks" | sed "s/ $id / /") ;;
              *) ticks="$ticks$id " ;;
            esac
          else
            say "  ignored: $word"
          fi
          ;;
      esac
    done
    set +f
  done
}
setup_toolbar() {
  awk '!/^#/ && NF' "$catalog/tools" > "$tmp/setup-tools"
  count=$(wc -l < "$tmp/setup-tools" | tr -d ' ')
  : > "$tmp/setup-missing"
  while IFS='|' read -r id _name _summary _ticked needs _nerd _text; do
    missing=""
    for program in $needs; do
      present "$program" </dev/null || missing="$missing${missing:+, }$program"
    done
    printf '%s|%s\n' "$id" "$missing" >> "$tmp/setup-missing"
  done < "$tmp/setup-tools"
  ticks=" $(awk -F'|' '$4 == 1 { printf "%s ", $1 }' "$tmp/setup-tools")"
  [ -z "$answers_from" ] || choose_tools
  chosen=""
  while IFS='|' read -r id name _summary _ticked _needs _nerd _text; do
    case "$ticks" in *" $id "*) ;; *) continue ;; esac
    missing=$(missing_of "$id")
    if [ -n "$missing" ]; then
      say "  left off: $name (needs $missing)"
    else
      chosen="$chosen $id"
    fi
  done < "$tmp/setup-tools"
  if nerd_default; then mode=nerd; else mode=text; fi
  if [ -n "$answers_from" ]; then
    if [ "$mode" = nerd ]; then printf 'Nerd Font icons? [Y/n] '; else printf 'Nerd Font icons? [y/N] '; fi
    ask reply
    case "$reply" in [Yy]*) mode=nerd ;; [Nn]*) mode=text ;; esac
  fi
  # shellcheck disable=SC2086 # the chosen ids, one argument each
  write_toolbar "$config/tools.toml" "$mode" $chosen
  # shellcheck disable=SC2086
  set -- $chosen
  say "Wrote $config/tools.toml ($# tools, $mode icons)"
}
if [ -n "$config" ] && { [ -n "$setup" ] || [ ! -f "$config/tools.toml" ]; }; then
  if [ -d "$catalog" ]; then
    mkdir -p "$config"
    if [ -f "$config/tools.toml" ]; then
      # --setup: keep the old file; a second run in the same second gets -2, -3, ...
      bak="$config/tools.toml.bak-$(date +%Y%m%d-%H%M%S)"
      keep=$bak
      n=2
      while [ -e "$keep" ]; do
        keep="$bak-$n"
        n=$((n + 1))
      done
      cp "$config/tools.toml" "$keep"
      say "Kept your old tools.toml as $keep"
    fi
    say ""
    say "Toolbar:"
    setup_toolbar
  else
    say "No toolbar catalog in $dir: tools.toml not written"
  fi
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
command = "jefelabs.intents.add"

Run this installer again to update.
KEYS

[ -z "$refused" ] || fail "not installed:$refused"
