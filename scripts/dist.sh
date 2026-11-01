#!/bin/sh
# Builds release archives of the Herdr plugins (toolbar and intents) for
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

# An archive must not carry macOS metadata (extended attributes such as
# com.apple.provenance, or ._ AppleDouble files): Linux tar prints a warning
# for each one on install.
check_clean() { # <archive>
  if gzip -dc "$1" | grep -aq 'LIBARCHIVE\.xattr' || tar -tzf "$1" | grep -q '/\._'; then
    echo "dist: $1 carries macOS metadata" >&2
    exit 1
  fi
}

pack() { # <archive> <folder> <entry>
  if tar --version 2>/dev/null | grep -q bsdtar; then
    # macOS tar: leave the metadata out, and no ._ files
    COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata --no-acls -czf "$1" -C "$2" "$3"
  else
    tar -czf "$1" -C "$2" "$3"
  fi
}

build() { # <rust target> <asset name>
  target=$1
  name=$2
  rustup target add "$target" >/dev/null 2>&1
  case "$target" in
    *-linux-musl)
      var="CARGO_TARGET_$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')_LINKER"
      env "$var=rust-lld" RUSTFLAGS="-C linker-flavor=ld.lld" \
        cargo build --release --target "$target" -p herdr-toolbar -p herdr-intents -p herdr-doc
      ;;
    *)
      cargo build --release --target "$target" -p herdr-toolbar -p herdr-intents -p herdr-doc
      ;;
  esac
  stage=$(mktemp -d)
  root="$stage/herdr-plugins"
  mkdir -p "$root/toolbar" "$root/tools/intents" "$root/tools/doc" "$root/target/release"
  cp toolbar/herdr-plugin.toml toolbar/README.md toolbar/tools.example.toml "$root/toolbar/"
  cp -R toolbar/setup "$root/toolbar/"
  cp tools/intents/herdr-plugin.toml tools/intents/README.md "$root/tools/intents/"
  cp tools/doc/README.md "$root/tools/doc/"
  cp "target/$target/release/herdr-toolbar" "target/$target/release/herdr-intents" \
    "target/$target/release/herdr-doc" "$root/target/release/"
  pack "dist/herdr-plugins-$name.tar.gz" "$stage" herdr-plugins
  rm -rf "$stage"
  check_clean "dist/herdr-plugins-$name.tar.gz"
  echo "built dist/herdr-plugins-$name.tar.gz"
}

build aarch64-unknown-linux-musl linux-arm64
build x86_64-unknown-linux-musl linux-x86_64
build aarch64-apple-darwin macos-arm64
build x86_64-apple-darwin macos-x86_64
cp install.sh dist/
(cd dist && shasum -a 256 herdr-plugins-*.tar.gz > SHA256SUMS)
echo "dist/ is ready for v$version"
