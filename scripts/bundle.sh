#!/bin/sh
# Builds target/release/AccRoster.app and signs it with the stable local
# identity, so Keychain approvals and notification permission survive rebuilds.
#
# Usage: scripts/bundle.sh [--ad-hoc]
#   --ad-hoc   sign ad hoc instead (a new identity on every build); for
#              machines without the local identity, such as CI.
#
# Create the local identity once with scripts/create-signing-identity.sh.
# ACCROSTER_SIGNING_IDENTITY overrides its name.
set -eu

repo="$(cd "$(dirname "$0")/.." && pwd)"
identity="${ACCROSTER_SIGNING_IDENTITY:-AccRoster Local Signing}"
ad_hoc=false
for arg in "$@"; do
  case "$arg" in
    --ad-hoc) ad_hoc=true ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if [ "$ad_hoc" = false ] && ! security find-identity -p codesigning | grep -q "\"$identity\""; then
  echo "No code signing identity named \"$identity\"." >&2
  echo "Create it once with scripts/create-signing-identity.sh, or pass --ad-hoc." >&2
  exit 1
fi

cargo build --release --manifest-path "$repo/Cargo.toml" -p accroster

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -n 1)"
app="$repo/target/release/AccRoster.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$repo/target/release/accroster" "$app/Contents/MacOS/accroster"
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
else
  echo "rsvg-convert not found: building without an app icon." >&2
fi

if [ "$ad_hoc" = true ]; then
  echo "Signing ad hoc: Keychain approvals won't survive the next rebuild." >&2
  codesign --force --sign - --identifier io.github.jbmanllr.accroster "$app"
else
  codesign --force --sign "$identity" --identifier io.github.jbmanllr.accroster "$app"
fi
codesign --verify --strict "$app"
echo "$app"
