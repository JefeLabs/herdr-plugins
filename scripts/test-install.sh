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
case "$1 $2" in
  "plugin list") cat "$HOME/.fake-herdr-links" 2>/dev/null ;;
  "plugin link") echo "[local:$3]" >> "$HOME/.fake-herdr-links" ;;
  "plugin config-dir") echo "$HOME/.config/herdr/plugins/config/$3" ;;
esac
exit 0
EOF
chmod +x "$work/fakebin/herdr"

# --- a fake plugin release for this machine
rel="$work/release"
stage="$work/stage/herdr-plugins"
mkdir -p "$rel" "$stage/toolbar" "$stage/tools/capabilities" "$stage/target/release"
cp toolbar/tools.example.toml "$stage/toolbar/"
echo 'id = "jefelabs.toolbar"' > "$stage/toolbar/herdr-plugin.toml"
echo 'id = "jefelabs.capabilities"' > "$stage/tools/capabilities/herdr-plugin.toml"
printf '#!/bin/sh\n' > "$stage/target/release/herdr-toolbar"
printf '#!/bin/sh\n' > "$stage/target/release/herdr-capabilities"
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
ROWS
  } > "$1"
}

base_path="$work/fakebin:/usr/bin:/bin"
run() { # <home> <extra PATH or -> <manifest> <args...>; output in $out, status in $status
  h=$1 extra=$2 m=$3
  shift 3
  p=$base_path
  [ "$extra" = - ] || p="$extra:$p"
  out=$(env -i HOME="$h" PATH="$p" HERDR_PLUGINS_TOOLS_MANIFEST="$m" ${REPOS:+HERDR_PLUGINS_REPOS="$REPOS"} ${LIBC:+HERDR_PLUGINS_LIBC="$LIBC"} \
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
    *"Usage tool not added"*) pass "no Usage tool without its three tools" ;;
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

[ $failures -eq 0 ] && echo "all install tests passed" || echo "$failures install test(s) failed"
[ $failures -eq 0 ]
