#!/bin/sh
# Assembles an unsigned AccRoster.app around a built executable: the
# Info.plist from bundle/ with the version filled in, and the app icon.
# Shared by scripts/bundle.sh (local builds) and scripts/release.sh.
#
# Usage: scripts/assemble-app.sh <executable> <app> [--require-icon]
#   --require-icon  fail when rsvg-convert is missing instead of building
#                   without an icon (releases always ship the icon).
set -eu

repo="$(cd "$(dirname "$0")/.." && pwd)"
[ $# -ge 2 ] || { echo "usage: $0 <executable> <app> [--require-icon]" >&2; exit 2; }
executable="$1"
app="$2"
require_icon=false
[ "${3:-}" = --require-icon ] && require_icon=true

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -n 1)"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$executable" "$app/Contents/MacOS/accroster"
sed "s/@VERSION@/$version/g" "$repo/bundle/Info.plist" > "$app/Contents/Info.plist"

# App icon from the SVG, when rsvg-convert is installed (brew install librsvg).
if command -v rsvg-convert > /dev/null 2>&1; then
  iconset="$(mktemp -d)/AccRoster.iconset"
  mkdir -p "$iconset"
  for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" "$repo/assets/icon/accroster.svg" -o "$iconset/icon_${size}x${size}.png"
    double=$((size * 2))
    rsvg-convert -w "$double" -h "$double" "$repo/assets/icon/accroster.svg" -o "$iconset/icon_${size}x${size}@2x.png"
  done
  iconutil -c icns "$iconset" -o "$app/Contents/Resources/AccRoster.icns"
  rm -rf "$(dirname "$iconset")"
elif [ "$require_icon" = true ]; then
  echo "rsvg-convert not found: install it (brew install librsvg) to build the app icon." >&2
  exit 1
else
  echo "rsvg-convert not found: building without an app icon." >&2
fi
