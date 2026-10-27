#!/bin/sh
# Installs the Herdr toolbar and capabilities plugins from a GitHub release:
#
#   curl -fsSL https://github.com/JefeLabs/herdr-plugins/releases/latest/download/install.sh | sh
#
# No sudo, Rust or compiler needed. Settings (environment variables):
#   HERDR_PLUGINS_VERSION   a release tag such as v0.1.0 (default: the latest)
#   HERDR_PLUGINS_DIR       where the plugins go (default: ~/.local/share/herdr-plugins)
#   HERDR_PLUGINS_BASE_URL  download from here instead of GitHub (for testing)
set -eu

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
  if command -v curl >/dev/null 2>&1; then
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
say "Downloading $asset ($version)"
fetch "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "could not download $base/SHA256SUMS"
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
