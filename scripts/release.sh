#!/bin/sh
# Builds the release dmg, target/dist/AccRoster-<version>.dmg, and checks it.
#
# Usage:
#   scripts/release.sh --dry-run [--version <version>]
#   scripts/release.sh --developer-id --version <version>
#
#   --dry-run       ad-hoc signature, no notarization, no secrets: what every
#                   pull request runs to prove the release build still works.
#   --developer-id  Developer ID signature, notarization and stapling, then the
#                   Gatekeeper checks. Needs these environment variables:
#                     DEVELOPER_ID_NAME   the certificate's common name, in a
#                                         keychain on the search list
#                     NOTARY_KEY_P8       path to the App Store Connect API key
#                     NOTARY_KEY_ID       that key's id
#                     NOTARY_ISSUER_ID    that key's issuer id
#   --version       the version being released, `X.Y.Z` or the tag `vX.Y.Z`;
#                   it must equal workspace.package.version in Cargo.toml.
#                   Optional with --dry-run (defaults to Cargo.toml's).
#
# Needs rsvg-convert (brew install librsvg) for the app icon, and the
# aarch64-apple-darwin and x86_64-apple-darwin targets (rust-toolchain.toml).
set -eu

repo="$(cd "$(dirname "$0")/.." && pwd)"
bundle_id=io.github.jbmanllr.accroster
entitlements="$repo/bundle/AccRoster.entitlements"
dist="$repo/target/dist"

fail() {
  echo "release: $*" >&2
  exit 1
}

mode=""
expected=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) mode=dry-run ;;
    --developer-id) mode=developer-id ;;
    --version)
      [ $# -ge 2 ] || fail "--version needs a value"
      expected="${2#v}"
      shift
      ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -n "$mode" ] || fail "pass --dry-run or --developer-id"

# --- Guards: everything that can be checked before building -----------------

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -n 1)"
[ -n "$version" ] || fail "no version found in Cargo.toml"
if [ -z "$expected" ]; then
  [ "$mode" = dry-run ] || fail "--developer-id needs --version (the tag being released)"
  expected="$version"
fi
[ "$expected" = "$version" ] \
  || fail "version mismatch: releasing $expected but Cargo.toml says $version; bump workspace.package.version or fix the tag"

if [ "$mode" = developer-id ]; then
  for secret in DEVELOPER_ID_NAME NOTARY_KEY_P8 NOTARY_KEY_ID NOTARY_ISSUER_ID; do
    eval "value=\${$secret:-}"
    [ -n "$value" ] || fail "--developer-id needs $secret set (see docs/RELEASING.md)"
  done
  [ -f "$NOTARY_KEY_P8" ] || fail "NOTARY_KEY_P8 names no file: $NOTARY_KEY_P8"
  security find-identity -v -p codesigning | grep -q "\"$DEVELOPER_ID_NAME\"" \
    || fail "no valid code signing identity named \"$DEVELOPER_ID_NAME\" in the keychain search list"
fi
command -v rsvg-convert > /dev/null 2>&1 \
  || fail "rsvg-convert not found: install it (brew install librsvg) to build the app icon"

app="$dist/AccRoster.app"
dmg="$dist/AccRoster-$version.dmg"

# --- Build ------------------------------------------------------------------

rm -rf "$dist"
mkdir -p "$dist"
for target in x86_64-apple-darwin aarch64-apple-darwin; do
  cargo build --release --locked --manifest-path "$repo/Cargo.toml" -p accroster --target "$target"
done
lipo -create -output "$dist/accroster" \
  "$repo/target/x86_64-apple-darwin/release/accroster" \
  "$repo/target/aarch64-apple-darwin/release/accroster"
"$repo/scripts/assemble-app.sh" "$dist/accroster" "$app" --require-icon
rm "$dist/accroster"

if [ "$mode" = developer-id ]; then
  identity="$DEVELOPER_ID_NAME"
  timestamp=--timestamp
else
  identity=-
  timestamp=""
fi
# shellcheck disable=SC2086 # $timestamp is one flag or none.
codesign --force --options runtime $timestamp --entitlements "$entitlements" \
  --identifier "$bundle_id" --sign "$identity" "$app"

# Sends one file to Apple's notary service and fails unless it's accepted.
notarize() {
  submitted="$1"
  result="$(xcrun notarytool submit "$submitted" --wait --output-format json \
    --key "$NOTARY_KEY_P8" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")" \
    || fail "notarytool submit failed for $submitted"
  status="$(printf '%s' "$result" | plutil -extract status raw - 2>/dev/null || echo unknown)"
  if [ "$status" != Accepted ]; then
    id="$(printf '%s' "$result" | plutil -extract id raw - 2>/dev/null || true)"
    [ -z "$id" ] || xcrun notarytool log "$id" \
      --key "$NOTARY_KEY_P8" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" >&2 || true
    fail "notarization of $submitted ended $status"
  fi
}

# The app gets its own ticket so it opens offline after it's copied out of
# the dmg; the dmg then gets one for itself.
if [ "$mode" = developer-id ]; then
  zip="$dist/AccRoster.zip"
  ditto -c -k --keepParent "$app" "$zip"
  notarize "$zip"
  rm "$zip"
  xcrun stapler staple "$app" || fail "stapling the app failed"
fi

staging="$dist/staging"
mkdir -p "$staging"
ditto "$app" "$staging/AccRoster.app"
ln -s /Applications "$staging/Applications"
# hdiutil sometimes reports "Resource busy" on hosted runners; retry it.
attempt=1
until hdiutil create -quiet -srcfolder "$staging" -volname AccRoster -format UDZO -ov "$dmg"; do
  [ "$attempt" -lt 3 ] || fail "hdiutil create failed"
  attempt=$((attempt + 1))
  sleep 5
done
rm -rf "$staging"

if [ "$mode" = developer-id ]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
  notarize "$dmg"
  xcrun stapler staple "$dmg" || fail "stapling the dmg failed"
fi

# --- Checks: what a user would get ------------------------------------------

check_app() {
  checked="$1"
  codesign --verify --deep --strict -vvv "$checked" \
    || fail "codesign --verify failed on $checked"
  codesign -d --verbose=2 "$checked" 2>&1 | grep -q '^CodeDirectory .*flags=.*runtime' \
    || fail "$checked is not signed with the hardened runtime"
  granted="$(codesign -d --entitlements - --xml "$checked" 2>/dev/null)"
  [ "$(printf '%s' "$granted" | plutil -convert xml1 -o - - | plutil -convert json -o - -)" = '{"com.apple.security.automation.apple-events":true}' ] \
    || fail "$checked does not carry exactly the Apple Events entitlement"
  archs="$(lipo -archs "$checked/Contents/MacOS/accroster" | tr ' ' '\n' | sort | xargs)"
  [ "$archs" = "arm64 x86_64" ] \
    || fail "$checked is not universal: lipo -archs lists $archs"
  shipped="$(plutil -extract CFBundleShortVersionString raw "$checked/Contents/Info.plist")"
  [ "$shipped" = "$expected" ] \
    || fail "$checked has version $shipped, expected $expected"
}

check_dmg() {
  [ -f "$dmg" ] || fail "no dmg at $dmg"
  mount="$(mktemp -d)"
  hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mount" "$dmg" > /dev/null \
    || fail "$dmg doesn't mount"
  status=0
  (
    volume="$(diskutil info -plist "$mount" | plutil -extract VolumeName raw -)"
    [ "$volume" = AccRoster ] || fail "dmg volume is named $volume, expected AccRoster"
    [ -d "$mount/AccRoster.app" ] || fail "dmg doesn't hold AccRoster.app"
    [ "$(readlink "$mount/Applications")" = /Applications ] \
      || fail "dmg doesn't hold an Applications link to /Applications"
    for item in "$mount"/* "$mount"/.[!.]*; do
      case "${item##*/}" in
        AccRoster.app | Applications | .fseventsd | '*' | '.[!.]*') ;;
        *) fail "dmg holds an unexpected item: ${item##*/}" ;;
      esac
    done
    check_app "$mount/AccRoster.app"
  ) || status=$?
  hdiutil detach -quiet "$mount" || hdiutil detach -force -quiet "$mount"
  rmdir "$mount"
  [ "$status" -eq 0 ] || exit "$status"
}

check_app "$app"
check_dmg
if [ "$mode" = developer-id ]; then
  spctl --assess --type execute -vv "$app" || fail "Gatekeeper rejects the app"
  spctl -a -t open --context context:primary-signature -vv "$dmg" || fail "Gatekeeper rejects the dmg"
  xcrun stapler validate "$app" || fail "the app's notarization ticket isn't stapled"
  xcrun stapler validate "$dmg" || fail "the dmg's notarization ticket isn't stapled"
fi
echo "$dmg"
