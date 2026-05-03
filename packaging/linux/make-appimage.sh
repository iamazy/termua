#!/usr/bin/env bash
set -euo pipefail

# Build an AppImage for termua on Linux using appimagetool.
#
# Prereqs:
#   - Rust toolchain (cargo)
#   - curl
#
# Usage:
#   packaging/linux/make-appimage.sh
#   ARCH=x86_64 packaging/linux/make-appimage.sh
#   ARCH=aarch64 packaging/linux/make-appimage.sh
#   BIN=target/release/termua OUT_APPIMAGE=target/appimage/x86_64/termua-x86_64.AppImage packaging/linux/make-appimage.sh
#
# Notes:
#   - Downloads appimagetool at build time.
#   - Extracts appimagetool to avoid FUSE requirements.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"
source "$repo_root/packaging/package-version.sh"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "make-appimage.sh must run on Linux." >&2
  exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"

host_machine="$(uname -m)"
case "$host_machine" in
  x86_64) host_arch="x86_64" ;;
  aarch64|arm64) host_arch="aarch64" ;;
  *)
    echo "unsupported arch for AppImage: $host_machine" >&2
    exit 1
    ;;
esac

arch="${ARCH:-$host_arch}"
if [[ "$arch" != "$host_arch" ]]; then
  echo "ARCH=$arch does not match host arch ($host_arch). Cross-packaging AppImages is not supported by this script." >&2
  exit 1
fi

default_target=""
case "$arch" in
  x86_64) default_target="x86_64-unknown-linux-gnu" ;;
  aarch64) default_target="aarch64-unknown-linux-gnu" ;;
esac
target="${TARGET:-$default_target}"
package_version="$(get_termua_package_version "$repo_root/Cargo.toml")"

bin="${BIN:-target/$target/release/termua}"
relay_bin="${RELAY_BIN:-target/$target/release/termua-relay}"
out_appimage="${OUT_APPIMAGE:-target/appimage/$arch/termua-$package_version-linux.$arch.AppImage}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "missing cargo; install Rust toolchain first" >&2
  exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "missing curl; on Ubuntu: sudo apt-get install -y curl" >&2
  exit 1
fi

if [[ ! -f "$bin" || ! -f "$relay_bin" ]]; then
  echo "==> Building termua + termua-relay (release)"
  cargo build -p termua --release --target "$target"
  cargo build -p termua_relay --release --target "$target"
fi
if [[ ! -f "$bin" ]]; then
  echo "missing binary after build: $bin" >&2
  exit 1
fi
if [[ ! -f "$relay_bin" ]]; then
  echo "missing relay binary after build: $relay_bin" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

appdir="$work/AppDir"
mkdir -p "$appdir/usr/bin"
cp "$bin" "$appdir/usr/bin/termua"
cp "$relay_bin" "$appdir/usr/bin/termua-relay"
chmod +x "$appdir/usr/bin/termua"
chmod +x "$appdir/usr/bin/termua-relay"

# Desktop integration (appimagetool expects these at the root of AppDir)
cp packaging/linux/termua.desktop "$appdir/termua.desktop"
cp assets/logo/termua.svg "$appdir/termua.svg"

cat >"$appdir/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/termua" "$@"
EOF
chmod +x "$appdir/AppRun"

echo "==> Downloading appimagetool ($arch)"
appimagetool_url="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-${arch}.AppImage"
curl -fsSL -o "$work/appimagetool.AppImage" "$appimagetool_url"
chmod +x "$work/appimagetool.AppImage"

echo "==> Building AppImage"
(
  cd "$work"
  ./appimagetool.AppImage --appimage-extract >/dev/null
)

if [[ ! -x "$work/squashfs-root/AppRun" ]]; then
  echo "appimagetool extraction failed; expected: $work/squashfs-root/AppRun" >&2
  echo "note: --appimage-extract writes to ./squashfs-root in the current directory" >&2
  exit 1
fi

mkdir -p "$(dirname "$out_appimage")"
"$work/squashfs-root/AppRun" "$appdir" "$out_appimage"
chmod +x "$out_appimage"

echo "==> Wrote: $out_appimage"
