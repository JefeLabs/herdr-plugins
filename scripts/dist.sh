#!/bin/sh
# Builds release archives of the Herdr plugins (toolbar and capabilities) for
# every supported platform, plus SHA256SUMS and install.sh, into dist/.
# Run from anywhere: scripts/dist.sh
#
# Linux builds are static (musl) and linked with the rust-lld that ships with
# Rust, so they run on any Linux and need no cross compiler here.
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' toolbar/herdr-plugin.toml | head -n 1)
rm -rf dist
mkdir -p dist

build() { # <rust target> <asset name>
  target=$1
  name=$2
  rustup target add "$target" >/dev/null 2>&1
  case "$target" in
    *-linux-musl)
      var="CARGO_TARGET_$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')_LINKER"
      env "$var=rust-lld" RUSTFLAGS="-C linker-flavor=ld.lld" \
        cargo build --release --target "$target" -p herdr-toolbar -p herdr-capabilities
      ;;
    *)
      cargo build --release --target "$target" -p herdr-toolbar -p herdr-capabilities
      ;;
  esac
  stage=$(mktemp -d)
  root="$stage/herdr-plugins"
  mkdir -p "$root/toolbar" "$root/tools/capabilities" "$root/target/release"
  cp toolbar/herdr-plugin.toml toolbar/README.md toolbar/tools.example.toml "$root/toolbar/"
  cp tools/capabilities/herdr-plugin.toml tools/capabilities/README.md "$root/tools/capabilities/"
  cp "target/$target/release/herdr-toolbar" "target/$target/release/herdr-capabilities" \
    "$root/target/release/"
  tar -czf "dist/herdr-plugins-$name.tar.gz" -C "$stage" herdr-plugins
  rm -rf "$stage"
  echo "built dist/herdr-plugins-$name.tar.gz"
}

build aarch64-unknown-linux-musl linux-arm64
build x86_64-unknown-linux-musl linux-x86_64
build aarch64-apple-darwin macos-arm64
build x86_64-apple-darwin macos-x86_64
cp install.sh dist/
(cd dist && shasum -a 256 herdr-plugins-*.tar.gz > SHA256SUMS)
echo "dist/ is ready for v$version"
