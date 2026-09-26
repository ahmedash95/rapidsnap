#!/usr/bin/env bash
# Build a distributable DMG from dist/RapidSnap.app (run scripts/bundle.sh first).
#
#   scripts/make_dmg.sh                      # → dist/RapidSnap-<version>.dmg + sha256
#   RAPIDSNAP_DIST_DIR=<dir> scripts/make_dmg.sh
#
# The DMG holds RapidSnap.app and an /Applications symlink (drag to install).
# It prints the SHA-256 to paste into packaging/homebrew/Casks/rapidsnap.rb.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="${RAPIDSNAP_DIST_DIR:-$ROOT/dist}"
APP="$DIST/RapidSnap.app"
[[ -d "$APP" ]] || { echo "error: $APP not found (run scripts/bundle.sh first)" >&2; exit 1; }

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
DMG="$DIST/RapidSnap-$VERSION.dmg"
VOLNAME="RapidSnap $VERSION"

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/rapidsnap-dmg.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

# ditto keeps the code signature, symlinks and extended attributes intact.
ditto "$APP" "$STAGE/RapidSnap.app"
ln -s /Applications "$STAGE/Applications"

rm -f "$DMG"
hdiutil create -quiet -volname "$VOLNAME" -srcfolder "$STAGE" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"
hdiutil verify -quiet "$DMG"

SHA="$(shasum -a 256 "$DMG" | awk '{print $1}')"
SIZE="$(du -h "$DMG" | awk '{print $1}')"
echo "built   $DMG ($SIZE)"
echo "version $VERSION"
echo "sha256  $SHA"
echo
echo "Next: upload it as a GitHub release asset (tag v$VERSION), then set"
echo "  version \"$VERSION\""
echo "  sha256 \"$SHA\""
echo "in packaging/homebrew/Casks/rapidsnap.rb. The website serves the same file at /rapidsnap.dmg:"
echo "  cp \"$DMG\" website/public/rapidsnap.dmg"
