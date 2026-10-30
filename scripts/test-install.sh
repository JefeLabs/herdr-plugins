#!/bin/sh
# Tests install.sh offline: a fake `herdr`, a fake plugin release and fake
# tool archives, installed into a throwaway HOME. Needs no network.
# Run from anywhere: sh scripts/test-install.sh
set -u
cd "$(dirname "$0")/.." || exit 1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in Linux) os=linux ;; Darwin) os=macos ;; esac
case "$(uname -m)" in aarch64 | arm64) arch=arm64 ;; *) arch=x86_64 ;; esac
platform="$os-$arch"
docker_word=$([ "$os" = linux ] && echo containerd || echo Virtualization)

failures=0
pass() { printf 'ok    %s\n' "$1"; }
flunk() { printf 'FAIL  %s\n      %s\n' "$1" "$2"; failures=$((failures + 1)); }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1; else shasum -a 256 "$1" | cut -d ' ' -f 1; fi; }

# --- a fake herdr: remembers links, answers config-dir under $HOME
mkdir -p "$work/fakebin"
cat > "$work/fakebin/herdr" <<'EOF'
#!/bin/sh
echo "$*" >> "$HOME/.fake-herdr-calls"
case "$1 $2" in
  "plugin list") cat "$HOME/.fake-herdr-links" 2>/dev/null ;;
  "plugin link") echo "[local:$3]" >> "$HOME/.fake-herdr-links" ;;
  "plugin unlink")
    grep -v "^- $3 " "$HOME/.fake-herdr-links" > "$HOME/.fake-herdr-links.tmp" 2>/dev/null
    mv "$HOME/.fake-herdr-links.tmp" "$HOME/.fake-herdr-links" ;;
  "plugin config-dir") echo "$HOME/.config/herdr/plugins/config/$3" ;;
esac
exit 0
EOF
chmod +x "$work/fakebin/herdr"

# --- a fake plugin release for this machine
rel="$work/release"
stage="$work/stage/herdr-plugins"
mkdir -p "$rel" "$stage/toolbar" "$stage/tools/intents" "$stage/target/release"
cp toolbar/tools.example.toml "$stage/toolbar/"
cp -R toolbar/setup "$stage/toolbar/"
echo 'id = "jefelabs.toolbar"' > "$stage/toolbar/herdr-plugin.toml"
echo 'id = "jefelabs.intents"' > "$stage/tools/intents/herdr-plugin.toml"
# the real toolbar binary: install.sh runs `herdr-toolbar unzip` where no unzip
# or python3 exists
[ -x target/release/herdr-toolbar ] || cargo build -q --release -p herdr-toolbar || exit 1
cp target/release/herdr-toolbar "$stage/target/release/herdr-toolbar"
printf '#!/bin/sh\n' > "$stage/target/release/herdr-intents"
asset="herdr-plugins-$platform.tar.gz"
tar -czf "$rel/$asset" -C "$work/stage" herdr-plugins
echo "$(sha "$rel/$asset")  $asset" > "$rel/SHA256SUMS"
cp install.sh "$rel/"

# --- fake tool archives, laid out like the real ones, and a manifest
fake_bin() { # <path> <name>
  mkdir -p "$(dirname "$1")"
  printf '#!/bin/sh\necho "%s fake"\n' "$2" > "$1"
  chmod +x "$1"
}
zip_of() { # <zip> <dir to zip>
  python3 - "$1" "$2" <<'EOF'
import os, sys, zipfile
out, top = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w") as z:
    for d, _, fs in os.walk(top):
        for f in fs:
            p = os.path.join(d, f)
            info = zipfile.ZipInfo(os.path.relpath(p, top))
            info.external_attr = 0o755 << 16
            z.writestr(info, open(p, "rb").read())
EOF
}
t="$work/tools"
mkdir -p "$t/procs" "$t/oxker" "$t/dua" "$t/yazi" "$t/lazysql" "$t/nvim"
fake_bin "$t/procs/procs" procs && zip_of "$rel/procs-fake.zip" "$t/procs"
fake_bin "$t/oxker/oxker" oxker && tar -czf "$rel/oxker-fake.tar.gz" -C "$t/oxker" oxker
fake_bin "$t/dua/dua-fake/dua" dua && tar -czf "$rel/dua-fake.tar.gz" -C "$t/dua" dua-fake
fake_bin "$t/yazi/yazi-fake/yazi" yazi && fake_bin "$t/yazi/yazi-fake/ya" ya && zip_of "$rel/yazi-fake.zip" "$t/yazi"
fake_bin "$t/lazysql/lazysql" lazysql && tar -czf "$rel/lazysql-fake.tar.gz" -C "$t/lazysql" lazysql
# a folder tool, like Neovim's release: the program plus its runtime files
fake_bin "$t/nvim/nvim-fake/bin/nvim" nvim
mkdir -p "$t/nvim/nvim-fake/share/nvim/runtime"
echo "-- runtime" > "$t/nvim/nvim-fake/share/nvim/runtime/filetype.lua"
tar -czf "$rel/nvim-fake.tar.gz" -C "$t/nvim" nvim-fake
mkdir -p "$t/gh" "$t/jira"
fake_bin "$t/gh/gh-fake/bin/gh" gh && tar -czf "$rel/gh-fake.tar.gz" -C "$t/gh" gh-fake
fake_bin "$t/jira/jira-fake/bin/jira" jira && tar -czf "$rel/jira-fake.tar.gz" -C "$t/jira" jira-fake
write_manifest() { # <file> <bad tool or -> <missing tool or ->
  bad=$2 missing=$3
  {
    echo "# name platform url sha256 member..."
    while read -r name file members; do
      [ "$name" = "$missing" ] && continue
      sum=$(sha "$rel/$file")
      [ "$name" = "$bad" ] && sum=0000000000000000000000000000000000000000000000000000000000000000
      echo "$name $platform https://example.invalid/dl/$file $sum $members"
    done <<'ROWS'
procs procs-fake.zip procs
oxker oxker-fake.tar.gz oxker
dua dua-fake.tar.gz dua-fake/dua
yazi yazi-fake.zip yazi-fake/yazi yazi-fake/ya
lazysql lazysql-fake.tar.gz lazysql
nvim nvim-fake.tar.gz dir=nvim-fake:bin/nvim needs=glibc
gh gh-fake.tar.gz gh-fake/bin/gh
jira jira-fake.tar.gz jira-fake/bin/jira
ROWS
  } > "$1"
}

base_path="$work/fakebin:/usr/bin:/bin"
run() { # <home> <extra PATH or -> <manifest> <args...>; output in $out, status in $status
  h=$1 extra=$2 m=$3
  shift 3
  p=$base_path
  [ "$extra" = - ] || p="$extra:$p"
  # TTY: answers for the walk-through (a file), none by default; XTERM,
  # XGHOSTTY, XKITTY: the terminal it is told it runs in (never the runner's)
  out=$(env -i HOME="$h" PATH="$p" HERDR_PLUGINS_TOOLS_MANIFEST="$m" ${REPOS:+HERDR_PLUGINS_REPOS="$REPOS"} ${LIBC:+HERDR_PLUGINS_LIBC="$LIBC"} \
    HERDR_PLUGINS_TTY="${TTY:-none}" ${XTERM:+TERM="$XTERM"} ${XGHOSTTY:+GHOSTTY_RESOURCES_DIR="$XGHOSTTY"} ${XKITTY:+KITTY_WINDOW_ID="$XKITTY"} \
    sh "$rel/install.sh" --from "$rel" "$@" 2>&1)
  status=$?
}
toml() { echo "$1/.config/herdr/plugins/config/jefelabs.toolbar/tools.toml"; }

write_manifest "$work/good" - -

# 1. --tools installs every tool for this machine and adds the Usage tool
h="$work/h1"
mkdir -p "$h"
run "$h" - "$work/good" --tools
if [ $status -ne 0 ]; then
  flunk "tools install" "exit $status: $out"
else
  missing=""
  for b in procs oxker dua yazi ya lazysql; do
    [ -x "$h/.local/bin/$b" ] || missing="$missing $b"
  done
  if [ -n "$missing" ]; then
    flunk "tools install" "not installed:$missing"
  elif [ "$("$h/.local/bin/dua")" != "dua fake" ]; then
    flunk "tools install" "dua does not run: $("$h/.local/bin/dua" 2>&1)"
  else
    pass "tools install"
  fi
fi
if grep -q '^id *= *"usage"' "$(toml "$h")" 2>/dev/null &&
  grep -q "\"$docker_word\"" "$(toml "$h")" &&
  grep -q "\"dua\", \"i\", \"$h\"" "$(toml "$h")"; then
  pass "usage tool added, for this OS and the home folder"
else
  flunk "usage tool added, for this OS and the home folder" "$(cat "$(toml "$h")" 2>&1)"
fi
case "$out" in
  *"gh extension install dlvhdr/gh-dash"*"https://github.com/ankitpokhrel/jira-cli"*"curl -fsSL https://terminal-browser.sh/install | bash"*)
    pass "hints for the tools it does not install" ;;
  *) flunk "hints for the tools it does not install" "$out" ;;
esac
case "$out" in
  *"$h/.local/bin is not on your PATH"*) pass "says when the bin folder is not on PATH" ;;
  *) flunk "says when the bin folder is not on PATH" "$out" ;;
esac

# 2. a second run keeps what is there and adds the Usage tool once
run "$h" "$h/.local/bin" "$work/good" --tools
n=$(grep -c '^id *= *"usage"' "$(toml "$h")")
case "$status:$n:$out" in
  0:1:*"procs: already installed"*) pass "second run changes nothing" ;;
  *) flunk "second run changes nothing" "exit $status, usage blocks $n: $out" ;;
esac

# 3. a download whose checksum is wrong is refused; the rest still install
h="$work/h3"
mkdir -p "$h"
write_manifest "$work/bad-oxker" oxker -
run "$h" - "$work/bad-oxker" --tools
if [ $status -ne 0 ] && [ ! -e "$h/.local/bin/oxker" ] && [ -x "$h/.local/bin/procs" ]; then
  case "$out" in
    *"checksum mismatch for oxker"*) pass "tampered download refused" ;;
    *) flunk "tampered download refused" "no mismatch message: $out" ;;
  esac
else
  flunk "tampered download refused" "exit $status; oxker present: $([ -e "$h/.local/bin/oxker" ] && echo yes || echo no): $out"
fi
if grep -q '^id *= *"usage"' "$(toml "$h")" 2>/dev/null; then
  flunk "no Usage tool without its three tools" "added although oxker is missing"
else
  case "$out" in
    *"left off: Usage (needs oxker)"*) pass "no Usage tool without its three tools" ;;
    *) flunk "no Usage tool without its three tools" "no message: $out" ;;
  esac
fi

# 4. a tool already on PATH is left alone
h="$work/h4"
mkdir -p "$h" "$work/mine"
printf '#!/bin/sh\necho mine\n' > "$work/mine/procs"
chmod +x "$work/mine/procs"
run "$h" "$work/mine" "$work/good" --tools
if [ $status -eq 0 ] && [ ! -e "$h/.local/bin/procs" ] && [ -x "$h/.local/bin/oxker" ] &&
  printf '%s' "$out" | grep -q "procs: already installed"; then
  pass "an installed tool is kept"
else
  flunk "an installed tool is kept" "exit $status: $out"
fi

# 5. without --tools only the plugins install
h="$work/h5"
mkdir -p "$h"
run "$h" - "$work/good"
if [ $status -eq 0 ] && [ ! -e "$h/.local/bin/procs" ] && ! grep -q '^id *= *"usage"' "$(toml "$h")"; then
  pass "plugins only without --tools"
else
  flunk "plugins only without --tools" "exit $status: $out"
fi

# 6. HERDR_PLUGINS_REPOS chooses the folder the disk pane opens
h="$work/h6"
mkdir -p "$h"
REPOS=/srv/code run "$h" - "$work/good" --tools
if grep -q '"dua", "i", "/srv/code"' "$(toml "$h")" 2>/dev/null; then
  pass "repos folder from HERDR_PLUGINS_REPOS"
else
  flunk "repos folder from HERDR_PLUGINS_REPOS" "$(cat "$(toml "$h")" 2>&1)"
fi

# 7. a tool with no build for this machine is skipped with a hint
h="$work/h7"
mkdir -p "$h"
write_manifest "$work/no-oxker" - oxker
run "$h" - "$work/no-oxker" --tools
case "$status:$out" in
  0:*"oxker: no build for $platform"*) pass "no build: skipped with a hint" ;;
  *) flunk "no build: skipped with a hint" "exit $status: $out" ;;
esac

# 8. an unknown option is an error
h="$work/h8"
mkdir -p "$h"
run "$h" - "$work/good" --bogus
case "$status:$out" in
  1:*"unknown option --bogus"*) pass "unknown option refused" ;;
  *) flunk "unknown option refused" "exit $status: $out" ;;
esac

# 9. a folder tool (Neovim) unpacks whole and is linked into the bin folder
h="$work/h9"
mkdir -p "$h"
run "$h" - "$work/good" --tools
vendor="$h/.local/share/herdr-plugins/vendor/nvim/nvim-fake"
if [ $status -eq 0 ] && [ -L "$h/.local/bin/nvim" ] && [ "$("$h/.local/bin/nvim")" = "nvim fake" ] &&
  [ -f "$vendor/share/nvim/runtime/filetype.lua" ]; then
  pass "nvim installs as a folder, linked into the bin folder"
else
  flunk "nvim installs as a folder, linked into the bin folder" "exit $status; link: $(ls -l "$h/.local/bin/nvim" 2>&1): $out"
fi

# 10. the Editor tool is added, listening so files can be opened in it
t10=$(toml "$h")
if grep -q '^id *= *"editor"' "$t10" 2>/dev/null && grep -q 'exec nvim --listen' "$t10" &&
  sed -n '/^id *= *"editor"/,/^\[\[tool\]\]/p' "$t10" | grep -q '^open *= *"pane"'; then
  pass "Editor tool added, Neovim listening in a pane"
else
  flunk "Editor tool added, Neovim listening in a pane" "$(cat "$t10" 2>&1)"
fi

# 11. a second run keeps Neovim and adds the Editor once
run "$h" "$h/.local/bin" "$work/good" --tools
n=$(grep -c '^id *= *"editor"' "$(toml "$h")")
case "$status:$n:$out" in
  0:1:*"nvim: already installed"*) pass "second run keeps Neovim and one Editor" ;;
  *) flunk "second run keeps Neovim and one Editor" "exit $status, editor blocks $n: $out" ;;
esac

# 12. reinstalling the plugins keeps the unpacked Neovim
run "$h" - "$work/good"
if [ "$("$h/.local/bin/nvim" 2>&1)" = "nvim fake" ]; then
  pass "plugin reinstall keeps the Neovim folder"
else
  flunk "plugin reinstall keeps the Neovim folder" "$(ls -l "$h/.local/bin/nvim" 2>&1)"
fi

# 13. on a musl system (Alpine) Neovim's glibc build is skipped with a hint
h="$work/h13"
mkdir -p "$h"
LIBC=musl run "$h" - "$work/good" --tools
if [ $status -eq 0 ] && [ ! -e "$h/.local/bin/nvim" ] && [ -x "$h/.local/bin/procs" ] &&
  ! grep -q '^id *= *"editor"' "$(toml "$h")"; then
  case "$out" in
    *"nvim: "*"needs glibc"*) pass "musl: Neovim skipped with a hint" ;;
    *) flunk "musl: Neovim skipped with a hint" "no hint: $out" ;;
  esac
else
  flunk "musl: Neovim skipped with a hint" "exit $status: $out"
fi

# 14. an install from before the rename (jefelabs.capabilities) is migrated
h="$work/h14"
tb="$h/.config/herdr/plugins/config/jefelabs.toolbar"
mkdir -p "$tb"
echo "- jefelabs.capabilities (Capabilities) enabled [local:$h/.local/share/herdr-plugins/tools/capabilities]" > "$h/.fake-herdr-links"
cat > "$tb/tools.toml" <<EOF
order  = ["capabilities", "files"]

[[tool]]
id      = "capabilities"
name    = "Capabilities"
icon    = "✎"
command = ["$h/.local/share/herdr-plugins/target/release/herdr-capabilities"]

[[tool]]
id      = "files"
name    = "Files"
icon    = "▭"
command = ["yazi"]
EOF
printf '[[keys.command]]\nkey = "prefix+a"\ntype = "plugin_action"\ncommand = "jefelabs.capabilities.add"\n' > "$h/.config/herdr/config.toml"
run "$h" - "$work/good"
t14="$tb/tools.toml"
if [ $status -eq 0 ] && grep -q "unlink jefelabs.capabilities" "$h/.fake-herdr-calls" &&
  grep -q '^order  = \["intents", "files"\]' "$t14" && grep -q '^id      = "intents"' "$t14" &&
  grep -q '^name    = "Intents"' "$t14" && grep -q 'target/release/herdr-intents"' "$t14" &&
  ! grep -q 'capabilit' "$t14" && grep -q 'command = "jefelabs.intents.add"' "$h/.config/herdr/config.toml" &&
  [ -f "$t14.before-intents" ] && [ -f "$h/.config/herdr/config.toml.before-intents" ]; then
  pass "an old capabilities install is migrated, with backups"
else
  flunk "an old capabilities install is migrated, with backups" "exit $status: $out
$(cat "$t14" 2>&1)
$(cat "$h/.config/herdr/config.toml" 2>&1)
calls: $(cat "$h/.fake-herdr-calls" 2>&1)"
fi

# 15. migrating twice changes nothing more
cp "$t14" "$work/t14-after-first"
run "$h" - "$work/good"
if cmp -s "$t14" "$work/t14-after-first" && [ "$(grep -c 'unlink' "$h/.fake-herdr-calls")" = 1 ]; then
  pass "a second run leaves a migrated install alone"
else
  flunk "a second run leaves a migrated install alone" "$(diff "$work/t14-after-first" "$t14" 2>&1): $out"
fi

# 16. no unzip and no python3 (minimal Debian): the zip tools still install,
# unpacked by the toolbar's own `herdr-toolbar unzip`
minbin="$work/minbin"
mkdir -p "$minbin"
for c in sh uname mktemp mkdir rm rmdir tar gzip cp mv ln chmod cat awk sed grep cut head tail \
  basename dirname ls tr sort uniq wc sha256sum shasum touch date find readlink id env sleep xargs expr; do
  p=$(command -v "$c" 2>/dev/null) || continue
  case "$p" in /*) ln -sf "$p" "$minbin/$c" ;; esac
done
h="$work/h16"
mkdir -p "$h"
out=$(env -i HOME="$h" PATH="$work/fakebin:$minbin" HERDR_PLUGINS_TOOLS_MANIFEST="$work/good" \
  HERDR_PLUGINS_TTY=none sh "$rel/install.sh" --from "$rel" --tools 2>&1)
status=$?
if [ -e "$minbin/unzip" ] || [ -e "$minbin/python3" ]; then
  flunk "zip tools without unzip or python3" "the test PATH still has unzip or python3"
elif [ $status -eq 0 ] && [ "$("$h/.local/bin/procs")" = "procs fake" ] &&
  [ "$("$h/.local/bin/yazi")" = "yazi fake" ] && [ "$("$h/.local/bin/ya")" = "ya fake" ]; then
  pass "zip tools without unzip or python3"
else
  flunk "zip tools without unzip or python3" "exit $status: $out"
fi

# 17. no terminal: the default bar from the catalog (the ticked tools whose
# programs are present), the owner's settings and order. A variable set before
# a function call outlives it in sh, so the earlier tests' settings are cleared.
unset LIBC REPOS TTY XTERM XGHOSTTY XKITTY
h="$work/h17"
mkdir -p "$h"
run "$h" - "$work/good" --tools
t17=$(toml "$h")
if [ $status -eq 0 ] && grep -q '^bar *= *"large"' "$t17" 2>/dev/null && grep -q '^prefix *= *"ctrl-\]"' "$t17" &&
  grep -q '^order *= *\["intents", "db", "files", "editor", "usage", "terminal"\]' "$t17" &&
  ! grep -qE '^id *= *"(jira|web|issues|browser)"' "$t17" &&
  grep -qF "\"$h/.local/share/herdr-plugins/target/release/herdr-intents\"" "$t17"; then
  pass "no terminal: the default bar from the catalog"
else
  flunk "no terminal: the default bar from the catalog" "exit $status: $(cat "$t17" 2>&1)"
fi
case "$out" in
  *"left off: GitHub issues (needs gh, gh-dash)"*) pass "no terminal: says what it left off" ;;
  *) flunk "no terminal: says what it left off" "$out" ;;
esac

# 18. no terminal: Nerd icons when told it runs in Ghostty, text icons otherwise
esc=$(awk -F'|' '$1 == "intents" { print $6 }' toolbar/setup/tools 2>/dev/null)
h="$work/h18"
mkdir -p "$h"
XTERM=xterm-ghostty
run "$h" - "$work/good"
unset XTERM
nerd=$(grep -cF "\"$esc\"" "$(toml "$h")" 2>/dev/null)
h="$work/h18b"
mkdir -p "$h"
XTERM=xterm-256color XGHOSTTY=/Applications/Ghostty.app/Contents/Resources/ghostty
run "$h" - "$work/good"
unset XTERM XGHOSTTY
herdr_pane=$(grep -cF "\"$esc\"" "$(toml "$h")" 2>/dev/null)
h="$work/h18c"
mkdir -p "$h"
XTERM=xterm-256color
run "$h" - "$work/good"
unset XTERM
text=$(grep -cF '"✎"' "$(toml "$h")" 2>/dev/null)
case "$esc:$nerd:$herdr_pane:$text" in
  \\*:1:1:1) pass "icons: Nerd under Ghostty (also inside Herdr), text elsewhere" ;;
  *) flunk "icons: Nerd under Ghostty (also inside Herdr), text elsewhere" "escape '$esc', nerd $nerd, herdr pane $herdr_pane, text $text" ;;
esac

# 19. nothing personal in the catalog or the example
if [ -d toolbar/setup ] && ! grep -rqE 'JIRA_PROJECT|SKOOL|/Users/' toolbar/setup toolbar/tools.example.toml; then
  pass "nothing personal in the catalog"
else
  flunk "nothing personal in the catalog" "$(grep -rnE 'JIRA_PROJECT|SKOOL|/Users/' toolbar/setup toolbar/tools.example.toml 2>&1)"
fi

# --- the walk-through: answers come from a file (TTY), one line per question
unset LIBC REPOS TTY XTERM XGHOSTTY XKITTY
answers() { # <file> <line>...: the answers, one per line
  f=$1
  shift
  : > "$f"
  for a in "$@"; do printf '%s\n' "$a" >> "$f"; done
}
default_order='^order *= *\["intents", "db", "files", "editor", "usage", "terminal"\]'

# 20. Enter takes the defaults
h="$work/h20"
mkdir -p "$h"
answers "$work/a20" "" "n" ""
TTY="$work/a20"
run "$h" - "$work/good" --tools
unset TTY
if grep -q "$default_order" "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"[x] 1  Intents"*"[ ] 5  Jira"*) pass "walk-through: Enter takes the defaults" ;;
    *) flunk "walk-through: Enter takes the defaults" "no list: $out" ;;
  esac
else
  flunk "walk-through: Enter takes the defaults" "exit $status: $(cat "$(toml "$h")" 2>&1): $out"
fi

# 21. numbers switch tools on and off, and the list is shown again
h="$work/h21"
mkdir -p "$h"
answers "$work/a21" "4" "" "n" ""
TTY="$work/a21"
run "$h" - "$work/good" --tools
unset TTY
if grep -q '^order *= *\["intents", "db", "editor", "usage", "terminal"\]' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"[ ] 4  Files"*) pass "walk-through: numbers switch tools" ;;
    *) flunk "walk-through: numbers switch tools" "list not shown again: $out" ;;
  esac
else
  flunk "walk-through: numbers switch tools" "$(cat "$(toml "$h")" 2>&1): $out"
fi

# 22. junk answers are ignored with a note; "1 1" switches Intents off and on
h="$work/h22"
mkdir -p "$h"
answers "$work/a22" "x" "42" "1 1" "*" "" "n" ""
TTY="$work/a22"
run "$h" - "$work/good" --tools
unset TTY
case "$out" in
  *"ignored: x"*"ignored: 42"*"ignored: *"*)
    if grep -q "$default_order" "$(toml "$h")" 2>/dev/null; then
      pass "walk-through: junk answers are ignored"
    else
      flunk "walk-through: junk answers are ignored" "$(cat "$(toml "$h")" 2>&1)"
    fi ;;
  *) flunk "walk-through: junk answers are ignored" "$out" ;;
esac

# 23. the icon answer wins over detection
h="$work/h23"
mkdir -p "$h"
answers "$work/a23" "" "n" "n"
TTY="$work/a23" XTERM=xterm-ghostty
run "$h" - "$work/good"
unset TTY XTERM
if grep -qF '"✎"' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"Nerd Font icons? [Y/n]"*) pass "walk-through: the icon answer wins" ;;
    *) flunk "walk-through: the icon answer wins" "no [Y/n] question: $out" ;;
  esac
else
  flunk "walk-through: the icon answer wins" "$(cat "$(toml "$h")" 2>&1)"
fi

# 24. an existing tools.toml is left alone, with no questions
h="$work/h24"
mkdir -p "$(dirname "$(toml "$h")")"
printf '# mine\nprefix = "ctrl-b"\n' > "$(toml "$h")"
cp "$(toml "$h")" "$work/h24-before.toml"
answers "$work/a24" "" ""
TTY="$work/a24"
run "$h" - "$work/good"
unset TTY
if cmp -s "$(toml "$h")" "$work/h24-before.toml"; then
  case "$out" in
    *"which tools"*) flunk "an existing tools.toml is left alone" "asked anyway: $out" ;;
    *) pass "an existing tools.toml is left alone" ;;
  esac
else
  flunk "an existing tools.toml is left alone" "changed: $(diff "$work/h24-before.toml" "$(toml "$h")" 2>&1)"
fi

# 25. --setup asks again, backing the old file up each time
answers "$work/a25" "" "n" ""
TTY="$work/a25"
run "$h" - "$work/good" --setup
first=$status
run "$h" - "$work/good" --setup
unset TTY
cfg=$(dirname "$(toml "$h")")
baks=$(find "$cfg" -name 'tools.toml.bak-*' | wc -l | tr -d ' ')
originals=$(grep -l '^# mine' "$cfg"/tools.toml.bak-* 2>/dev/null | wc -l | tr -d ' ')
# (no --tools here, so only the tools that need nothing installed)
if [ "$first:$status:$baks:$originals" = "0:0:2:1" ] && grep -q '^order *= *\["intents", "terminal"\]' "$(toml "$h")"; then
  pass "--setup twice keeps both backups"
else
  flunk "--setup twice keeps both backups" "exits $first/$status, backups $baks, with the original $originals: $(ls "$cfg"): $out"
fi

# 26. --setup needs a terminal to ask in
run "$h" - "$work/good" --setup
case "$status:$out" in
  1:*"--setup needs a terminal to ask in"*) pass "--setup needs a terminal" ;;
  *) flunk "--setup needs a terminal" "exit $status: $out" ;;
esac

# --- installing what the chosen tools need. Fake installers log their calls
# to $HOME/.<name>-log and "install" a program by writing a script.
fake_installers() { # <folder> <brew exit status, or - for no Homebrew> <mise: yes|no>
  d=$1
  mkdir -p "$d"
  if [ "$2" != - ]; then
    cat > "$d/brew" <<EOF
#!/bin/sh
echo "\$*" >> "\$HOME/.brew-log"
[ "\$1" = install ] || exit 0
[ $2 = 0 ] || exit $2
[ "\$2" != --cask ] || shift
case "\$2" in
  ankitpokhrel/jira-cli/jira-cli) prog=jira ;;
  neovim) prog=nvim ;;
  dua-cli) prog=dua ;;
  *) prog=\$2 ;;
esac
printf '#!/bin/sh\necho "%s fake"\n' "\$prog" > "$d/\$prog"
chmod +x "$d/\$prog"
EOF
    chmod +x "$d/brew"
  fi
  if [ "$3" = yes ]; then
    cat > "$d/mise" <<'EOF'
#!/bin/sh
echo "$*" >> "$HOME/.mise-log"
[ "$1 $2" = "use -g" ] || exit 0
case "$3" in
  github:ankitpokhrel/jira-cli) prog=jira ;;
  neovim) prog=nvim ;;
  *) prog=${3##*/} ;;
esac
shims="$HOME/.local/share/mise/shims"
mkdir -p "$shims"
printf '#!/bin/sh\necho "%s fake"\n' "$prog" > "$shims/$prog"
chmod +x "$shims/$prog"
EOF
    chmod +x "$d/mise"
  fi
}
run_online() { # <home> <extra PATH or -> <args...>: like run, from a file:// release, not --from
  h=$1 extra=$2
  shift 2
  p=$base_path
  [ "$extra" = - ] || p="$extra:$p"
  out=$(env -i HOME="$h" PATH="$p" HERDR_PLUGINS_BASE_URL="file://$rel" HERDR_PLUGINS_TTY="${TTY:-none}" \
    HERDR_PLUGINS_TOOLS_MANIFEST="$work/good" ${TB:+HERDR_PLUGINS_TB_INSTALL="$TB"} sh "$rel/install.sh" "$@" 2>&1)
  status=$?
}
jira_config() { mkdir -p "$1/.config/.jira" && echo "installation: cloud" > "$1/.config/.jira/.config.yml"; }

# 27. Homebrew installs what is missing, after one question
h="$work/h27"
mkdir -p "$h"
jira_config "$h"
fake_installers "$work/f27" 0 no
answers "$work/a27" "5" "" "y" ""
TTY="$work/a27"
run_online "$h" "$work/f27"
unset TTY
if grep -q 'install ankitpokhrel/jira-cli/jira-cli' "$h/.brew-log" 2>/dev/null && grep -q '^id *= *"jira"' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"  with Homebrew: "*"jira-cli"*) pass "Homebrew installs what is missing" ;;
    *) flunk "Homebrew installs what is missing" "no question: $out" ;;
  esac
else
  flunk "Homebrew installs what is missing" "exit $status, brew: $(cat "$h/.brew-log" 2>&1): $out"
fi

# 28. no means nothing is installed and those tools are left off
h="$work/h28"
mkdir -p "$h"
jira_config "$h"
fake_installers "$work/f28" 0 no
answers "$work/a28" "5" "" "n" ""
TTY="$work/a28"
run_online "$h" "$work/f28"
unset TTY
if ! grep -q install "$h/.brew-log" 2>/dev/null && ! grep -q '^id *= *"jira"' "$(toml "$h")" 2>/dev/null &&
  [ -f "$(toml "$h")" ]; then
  pass "no means left off"
else
  flunk "no means left off" "exit $status, brew: $(cat "$h/.brew-log" 2>&1): $out"
fi

# 29. a failed install leaves that tool off; the rest carry on
h="$work/h29"
mkdir -p "$h"
jira_config "$h"
fake_installers "$work/f29" 1 no
answers "$work/a29" "5" "" "y" ""
TTY="$work/a29"
run_online "$h" "$work/f29"
unset TTY
case "$status:$out" in
  0:*"left off: Jira (jira-cli did not install)"*)
    if [ -f "$(toml "$h")" ] && ! grep -q '^id *= *"jira"' "$(toml "$h")"; then
      pass "a failed brew install leaves that tool off"
    else
      flunk "a failed brew install leaves that tool off" "$(cat "$(toml "$h")" 2>&1)"
    fi ;;
  *) flunk "a failed brew install leaves that tool off" "exit $status: $out" ;;
esac

# 30. mise when there is no Homebrew
h="$work/h30"
mkdir -p "$h"
jira_config "$h"
fake_installers "$work/f30" - yes
answers "$work/a30" "5" "" "y" ""
TTY="$work/a30"
run_online "$h" "$work/f30:$h/.local/share/mise/shims"
unset TTY
if grep -q 'use -g github:ankitpokhrel/jira-cli' "$h/.mise-log" 2>/dev/null && grep -q '^id *= *"jira"' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"  with mise: "*"jira-cli"*) pass "mise when there is no Homebrew" ;;
    *) flunk "mise when there is no Homebrew" "no question: $out" ;;
  esac
else
  flunk "mise when there is no Homebrew" "exit $status, mise: $(cat "$h/.mise-log" 2>&1): $out"
fi

# 31. a program mise put outside PATH still counts, with the PATH line to add
h="$work/h31"
mkdir -p "$h"
jira_config "$h"
fake_installers "$work/f31" - yes
answers "$work/a31" "5" "" "y" ""
TTY="$work/a31"
run_online "$h" "$work/f31"
unset TTY
if grep -q '^id *= *"jira"' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"$h/.local/share/mise/shims is not on your PATH"*) pass "a mise tool off PATH is added with a PATH hint" ;;
    *) flunk "a mise tool off PATH is added with a PATH hint" "no hint: $out" ;;
  esac
else
  flunk "a mise tool off PATH is added with a PATH hint" "exit $status: $out"
fi

# 32. with neither (and offline, where Homebrew is not used): a pinned download,
# refused when its checksum is wrong
h="$work/h32"
mkdir -p "$h"
jira_config "$h"
fake_installers "$work/f32" 0 no
answers "$work/a32" "5" "" "y" ""
TTY="$work/a32"
run "$h" "$work/f32" "$work/good"
if [ $status -eq 0 ] && [ "$("$h/.local/bin/jira" 2>&1)" = "jira fake" ] && grep -q '^id *= *"jira"' "$(toml "$h")" &&
  ! grep -q install "$h/.brew-log" 2>/dev/null; then
  pass "pinned download when neither is there"
else
  flunk "pinned download when neither is there" "exit $status, brew: $(cat "$h/.brew-log" 2>&1): $out"
fi
h="$work/h32b"
mkdir -p "$h"
jira_config "$h"
write_manifest "$work/bad-jira" jira -
run "$h" - "$work/bad-jira"
unset TTY
case "$status:$out" in
  1:*"checksum mismatch for jira"*)
    if grep -q '^id *= *"jira"' "$(toml "$h")" 2>/dev/null; then
      flunk "a tampered jira download is refused" "Jira added anyway"
    else
      pass "a tampered jira download is refused"
    fi ;;
  *) flunk "a tampered jira download is refused" "exit $status: $out" ;;
esac

# --- gh dash, Jira and terminal-browser
fake_gh() { # <folder> <exit status of gh auth status>: a gh that logs, and installs gh-dash
  mkdir -p "$1"
  cat > "$1/gh" <<EOF
#!/bin/sh
echo "\$*" >> "\$HOME/.gh-log"
case "\$1 \$2" in
  "auth status") exit $2 ;;
  "extension install") touch "\$HOME/.gh-dash-installed" ;;
  "extension list") [ -e "\$HOME/.gh-dash-installed" ] && echo "gh dash	dlvhdr/gh-dash	v4.26.2" ;;
esac
exit 0
EOF
  chmod +x "$1/gh"
}

# 33. GitHub issues waits for a signed-in gh, and says how to add it later
h="$work/h33"
mkdir -p "$h"
fake_installers "$work/f33" 0 no
fake_gh "$work/f33" 1
answers "$work/a33" "" "y" ""
TTY="$work/a33"
run_online "$h" "$work/f33"
unset TTY
if ! grep -q '^id *= *"issues"' "$(toml "$h")" 2>/dev/null && ! grep -q 'extension install' "$h/.gh-log" 2>/dev/null; then
  case "$out" in
    *"GitHub issues: run gh auth login, then install.sh --setup to add it"*) pass "GitHub issues waits for sign-in" ;;
    *) flunk "GitHub issues waits for sign-in" "no hint: $out" ;;
  esac
else
  flunk "GitHub issues waits for sign-in" "gh: $(cat "$h/.gh-log" 2>&1): $(cat "$(toml "$h")" 2>&1)"
fi

# 34. signed in: gh dash is installed and GitHub issues is added
h="$work/h34"
mkdir -p "$h"
fake_installers "$work/f34" 0 no
fake_gh "$work/f34" 0
answers "$work/a34" "" "y" ""
TTY="$work/a34"
run_online "$h" "$work/f34"
unset TTY
if grep -q 'extension install dlvhdr/gh-dash' "$h/.gh-log" 2>/dev/null && grep -q '^id *= *"issues"' "$(toml "$h")" 2>/dev/null; then
  pass "signed in, gh dash is installed"
else
  flunk "signed in, gh dash is installed" "gh: $(cat "$h/.gh-log" 2>&1): $out"
fi
# (Homebrew's terminal-browser is a cask)
if grep -q 'install --cask terminal-browser' "$h/.brew-log" 2>/dev/null && grep -q '^id *= *"browser"' "$(toml "$h")" 2>/dev/null; then
  pass "Homebrew installs terminal-browser as a cask"
else
  flunk "Homebrew installs terminal-browser as a cask" "brew: $(cat "$h/.brew-log" 2>&1)"
fi

# 35. Jira waits for jira init
h="$work/h35"
mkdir -p "$h"
fake_installers "$work/f35" 0 no
fake_bin "$work/f35/jira" jira
answers "$work/a35" "5" "" "n" ""
TTY="$work/a35"
run_online "$h" "$work/f35"
unset TTY
if ! grep -q '^id *= *"jira"' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"Jira: run jira init, then install.sh --setup to add it"*) pass "Jira waits for jira init" ;;
    *) flunk "Jira waits for jira init" "no hint: $out" ;;
  esac
else
  flunk "Jira waits for jira init" "added without a config: $out"
fi

# 36. terminal-browser from its own installer (no Homebrew); offline it needs the network
cat > "$work/tb-install" <<'EOF'
#!/bin/bash
echo "skip setup: ${TERMINAL_BROWSER_SKIP_SETUP:-no}" >> "$HOME/.tb-log"
mkdir -p "$HOME/.local/bin"
printf '#!/bin/sh\necho "terminal-browser fake"\n' > "$HOME/.local/bin/terminal-browser"
chmod +x "$HOME/.local/bin/terminal-browser"
EOF
h="$work/h36"
mkdir -p "$h"
answers "$work/a36" "2 3 4 8 9" "" "y" ""
TTY="$work/a36" TB="file://$work/tb-install"
run_online "$h" -
unset TTY TB
if grep -q '^id *= *"browser"' "$(toml "$h")" 2>/dev/null && grep -q 'skip setup: 1' "$h/.tb-log" 2>/dev/null; then
  pass "terminal-browser from its own installer"
else
  flunk "terminal-browser from its own installer" "exit $status, tb: $(cat "$h/.tb-log" 2>&1): $out"
fi
h="$work/h36b"
mkdir -p "$h"
TTY="$work/a36"
run "$h" - "$work/good"
unset TTY
if ! grep -q '^id *= *"browser"' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"terminal-browser: needs the network (its own installer)"*) pass "offline, terminal-browser needs the network" ;;
    *) flunk "offline, terminal-browser needs the network" "no message: $out" ;;
  esac
else
  flunk "offline, terminal-browser needs the network" "Browser added offline: $out"
fi

# --- review fixes
# 37. no terminal at all (CI, Docker, ssh host 'curl … | sh'): no questions, and
# the install carries on, under sh and under dash (Linux's sh)
notty() { python3 -c 'import os, sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])' "$@"; }
for shell in sh dash; do
  shell_path=$(command -v "$shell") || continue
  h="$work/h37-$shell"
  mkdir -p "$h"
  out=$(notty env -i HOME="$h" PATH="$base_path" HERDR_PLUGINS_TOOLS_MANIFEST="$work/good" \
    "$shell_path" "$rel/install.sh" --from "$rel" </dev/null 2>&1)
  status=$?
  if [ $status -eq 0 ] && grep -q '^order *= *\["intents", "terminal"\]' "$(toml "$h")" 2>/dev/null; then
    pass "no terminal at all ($shell): installs without questions"
  else
    flunk "no terminal at all ($shell): installs without questions" "exit $status: $out"
  fi
done

# 38. offline, Enter offers only what the --from folder holds; nothing fails
h="$work/h38"
mkdir -p "$h"
mv "$rel/gh-fake.tar.gz" "$work/gh-fake.tar.gz.aside"
answers "$work/a38" "" "" ""
TTY="$work/a38"
run "$h" - "$work/good"
unset TTY
mv "$work/gh-fake.tar.gz.aside" "$rel/gh-fake.tar.gz"
case "$status:$out" in
  0:*"left off: GitHub issues (needs gh, gh-dash)"*)
    case "$out" in
      *"cp: "* | *"not installed:"*) flunk "offline: only what the folder holds" "an error: $out" ;;
      *) pass "offline: only what the folder holds" ;;
    esac ;;
  *) flunk "offline: only what the folder holds" "exit $status: $out" ;;
esac

# 39. the install question names how each program installs
h="$work/h39"
mkdir -p "$h"
fake_installers "$work/f39" - yes
fake_gh "$work/f39" 0
answers "$work/a39" "" "n" ""
TTY="$work/a39"
run_online "$h" "$work/f39"
unset TTY
case "$out" in
  *"  with mise: "*"  with gh extension install: gh-dash"*"  with its own installer (https://terminal-browser.sh/install): terminal-browser"*)
    pass "the install question names each channel" ;;
  *) flunk "the install question names each channel" "$out" ;;
esac

# 40. an answer that is neither yes nor no installs nothing
h="$work/h40"
mkdir -p "$h"
fake_installers "$work/f40" 0 no
answers "$work/a40" "" "skip" ""
TTY="$work/a40"
run_online "$h" "$work/f40"
unset TTY
if ! grep -q install "$h/.brew-log" 2>/dev/null; then
  case "$out" in
    *"not installing"*) pass "an unclear answer installs nothing" ;;
    *) flunk "an unclear answer installs nothing" "no note: $out" ;;
  esac
else
  flunk "an unclear answer installs nothing" "brew: $(cat "$h/.brew-log")"
fi

# 41. the list follows the agreed mock-up (Browser, then Browser in toolbar);
# the bar keeps the owner's order
h="$work/h41"
mkdir -p "$h"
mkdir -p "$work/f41"
fake_bin "$work/f41/terminal-browser" terminal-browser
answers "$work/a41" "7" "" "n" ""
TTY="$work/a41"
run "$h" "$work/f41" "$work/good" --tools
unset TTY
if grep -q '^order *= *\["intents", "db", "files", "web", "editor", "browser", "usage", "terminal"\]' "$(toml "$h")" 2>/dev/null; then
  case "$out" in
    *"[x] 6  Browser "*"[ ] 7  Browser in toolbar"*) pass "list in the mock-up's order, bar in the owner's" ;;
    *) flunk "list in the mock-up's order, bar in the owner's" "list: $out" ;;
  esac
else
  flunk "list in the mock-up's order, bar in the owner's" "$(grep '^order' "$(toml "$h")" 2>&1): $out"
fi

[ $failures -eq 0 ] && echo "all install tests passed" || echo "$failures install test(s) failed"
[ $failures -eq 0 ]
