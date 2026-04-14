#!/usr/bin/env bash
set -euo pipefail

# Build a .dmg for termua on macOS using hdiutil.
#
# Usage:
#   packaging/macos/make-dmg.sh
#
# Environment overrides:
#   APP_BUNDLE=path/to/termua.app     # If set, package this .app directly
#   ARCH=x86_64|aarch64               # Select packaging arch (defaults to host)
#   TARGET=<rust-target-triple>       # Overrides target triple derived from ARCH
#   BIN=target/release/termua         # Used when APP_BUNDLE is not set
#   APP_NAME=termua                   # App bundle name (termua.app)
#   BUNDLE_ID=com.iamazy.termua
#   VOLNAME=termua
#   OUT_DMG=target/dmg/<arch>/termua.dmg
#   ICON_ICNS=packaging/macos/termua.icns  # Optional; copied into .app Resources/
#   ICON_SVG=assets/logo/termua.svg         # Used to generate .icns if ICON_ICNS is not set
#
# Notes:
#   - This script does NOT codesign or notarize. For distribution outside local
#     usage, you likely want codesigning + notarization.

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "make-dmg.sh must run on macOS (Darwin)." >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

app_name="${APP_NAME:-termua}"
bundle_id="${BUNDLE_ID:-com.iamazy.termua}"
volname="${VOLNAME:-termua}"

host_machine="$(uname -m)"
case "$host_machine" in
  x86_64) host_arch="x86_64" ;;
  arm64|aarch64) host_arch="aarch64" ;;
  *)
    echo "unsupported arch: $host_machine" >&2
    exit 1
    ;;
esac

arch="${ARCH:-$host_arch}"
default_target=""
case "$arch" in
  x86_64) default_target="x86_64-apple-darwin" ;;
  aarch64) default_target="aarch64-apple-darwin" ;;
  *)
    echo "unsupported ARCH: $arch (expected x86_64 or aarch64)" >&2
    exit 1
    ;;
esac
target="${TARGET:-$default_target}"

out_dmg="${OUT_DMG:-target/dmg/$arch/termua-${arch}.dmg}"

bin="${BIN:-target/$target/release/termua}"
relay_bin="${RELAY_BIN:-target/$target/release/termua-relay}"
icon_icns="${ICON_ICNS:-}"
app_bundle="${APP_BUNDLE:-}"

workspace_version="$(
  awk '
    $0 ~ /^\[workspace\.package\]/ {in=1; next}
    in && $0 ~ /^\[/ {in=0}
    in && match($0, /^version[[:space:]]*=[[:space:]]*"([^"]+)"/, m) {print m[1]; exit}
  ' Cargo.toml 2>/dev/null || true
)"
if [[ -z "$workspace_version" ]]; then
  workspace_version="0.0.0"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

stage="$work/stage"
mkdir -p "$stage"

if [[ -z "$icon_icns" ]]; then
  icon_svg="${ICON_SVG:-assets/logo/termua.svg}"
  if [[ -f "$icon_svg" ]]; then
    generated_icns="target/icons/${arch}/termua.icns"
    echo "==> Generating .icns from: $icon_svg"
    if packaging/macos/build-icns.sh "$icon_svg" "$generated_icns" >/dev/null; then
      icon_icns="$generated_icns"
    else
      echo "warning: failed to generate .icns (continuing without icon)" >&2
    fi
  fi
fi

if [[ -n "$app_bundle" ]]; then
  if [[ ! -d "$app_bundle" ]]; then
    echo "APP_BUNDLE does not exist: $app_bundle" >&2
    exit 1
  fi
  cp -R "$app_bundle" "$stage/${app_name}.app"
else
  if ! command -v cargo >/dev/null 2>&1; then
    echo "missing cargo; install Rust toolchain first" >&2
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

  app_root="$work/${app_name}.app"
  contents="$app_root/Contents"
  macos_dir="$contents/MacOS"
  resources_dir="$contents/Resources"
  mkdir -p "$macos_dir" "$resources_dir"

  cp "$bin" "$macos_dir/termua"
  cp "$relay_bin" "$macos_dir/termua-relay"
  chmod +x "$macos_dir/termua"
  chmod +x "$macos_dir/termua-relay"

  icon_key=""
  if [[ -n "$icon_icns" ]]; then
    if [[ -f "$icon_icns" ]]; then
      cp "$icon_icns" "$resources_dir/${app_name}.icns"
      icon_key="    <key>CFBundleIconFile</key>\n    <string>${app_name}.icns</string>\n"
    else
      echo "warning: ICON_ICNS not found: $icon_icns (skipping icon)" >&2
    fi
  fi

  cat >"$contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>termua</string>
    <key>CFBundleIdentifier</key>
    <string>${bundle_id}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${app_name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${workspace_version}</string>
    <key>CFBundleVersion</key>
    <string>${workspace_version}</string>
${icon_key}    <key>LSMinimumSystemVersion</key>
    <string>10.13</string>
    <key>NSHighResolutionCapable</key>
    <true/>
  </dict>
</plist>
EOF

  cp -R "$app_root" "$stage/${app_name}.app"
fi

ln -s /Applications "$stage/Applications"

mkdir -p "$(dirname "$out_dmg")"
rm -f "$out_dmg"

echo "==> Building DMG"
hdiutil create \
  -volname "$volname" \
  -srcfolder "$stage" \
  -ov \
  -format UDZO \
  "$out_dmg" >/dev/null

echo "==> Wrote: $out_dmg"
