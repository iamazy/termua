#!/usr/bin/env bash
set -euo pipefail

# Convert an SVG into an .icns file (macOS icon) by generating an .iconset and running iconutil.
#
# Usage:
#   packaging/macos/build-icns.sh assets/logo/termua.svg target/icons/termua.icns
#
# Requirements:
#   - macOS: `sips`, `iconutil` (built-in)
#   - SVG rasterizer: one of:
#       - rsvg-convert  (brew install librsvg)
#       - inkscape      (brew install inkscape)

if [[ "${1-}" == "" || "${2-}" == "" ]]; then
  echo "usage: $0 <icon.svg> <out.icns>" >&2
  exit 2
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-icns.sh must run on macOS (Darwin)." >&2
  exit 1
fi

svg="$1"
out_icns="$2"

if [[ ! -f "$svg" ]]; then
  echo "missing svg: $svg" >&2
  exit 1
fi

if ! command -v sips >/dev/null 2>&1; then
  echo "missing sips (should be available on macOS)" >&2
  exit 1
fi
if ! command -v iconutil >/dev/null 2>&1; then
  echo "missing iconutil (should be available on macOS)" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

base_png="$work/icon-1024.png"
if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w 1024 -h 1024 -o "$base_png" "$svg"
elif command -v inkscape >/dev/null 2>&1; then
  inkscape "$svg" --export-type=png --export-filename="$base_png" -w 1024 -h 1024 >/dev/null
else
  echo "no svg rasterizer found." >&2
  echo "install one of:" >&2
  echo "  brew install librsvg     # provides rsvg-convert" >&2
  echo "  brew install inkscape" >&2
  exit 1
fi

if [[ ! -f "$base_png" ]]; then
  echo "failed to rasterize svg to: $base_png" >&2
  exit 1
fi

iconset="$work/termua.iconset"
mkdir -p "$iconset"

mk_png() {
  local size="$1"
  local out="$2"
  sips -z "$size" "$size" "$base_png" --out "$out" >/dev/null
}

# Standard iconset sizes
mk_png 16   "$iconset/icon_16x16.png"
mk_png 32   "$iconset/icon_16x16@2x.png"
mk_png 32   "$iconset/icon_32x32.png"
mk_png 64   "$iconset/icon_32x32@2x.png"
mk_png 128  "$iconset/icon_128x128.png"
mk_png 256  "$iconset/icon_128x128@2x.png"
mk_png 256  "$iconset/icon_256x256.png"
mk_png 512  "$iconset/icon_256x256@2x.png"
mk_png 512  "$iconset/icon_512x512.png"
mk_png 1024 "$iconset/icon_512x512@2x.png"

mkdir -p "$(dirname "$out_icns")"
iconutil -c icns "$iconset" -o "$out_icns"

if [[ ! -f "$out_icns" ]]; then
  echo "failed to write icns: $out_icns" >&2
  exit 1
fi

echo "$out_icns"
