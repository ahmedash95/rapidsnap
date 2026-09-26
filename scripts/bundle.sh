#!/usr/bin/env bash
# Build a release RapidSnap.app into dist/.
#
#   scripts/bundle.sh            # release build + bundle + ad-hoc sign
#   SKIP_BUILD=1 scripts/bundle.sh   # reuse existing target/release/rapidsnap
#   RAPIDSNAP_SIGN_IDENTITY="RapidSnap Dev" scripts/bundle.sh
#                                # sign with a stable identity (keeps the Screen
#                                # Recording grant across rebuilds; see README.md)
#   RAPIDSNAP_DIST_DIR=/tmp/out scripts/bundle.sh   # output dir (default: dist/)
#   CARGO_TARGET_DIR=...         # honoured (binary taken from $CARGO_TARGET_DIR/release)
#
# Layout:
#   dist/RapidSnap.app/Contents/Info.plist
#   dist/RapidSnap.app/Contents/MacOS/RapidSnap      (cargo bin `rapidsnap`, renamed like v1)
#   dist/RapidSnap.app/Contents/Resources/AppIcon.icns
#   dist/RapidSnap.app/Contents/Resources/assets/...   (rs_core::assets::assets_dir())
set -euo pipefail

# Identity = RapidSnap v1's (v2 ships as its successor / upgrade).
BUNDLE_ID="io.github.ahmedash95.rapidsnap"
EXECUTABLE="RapidSnap"
PROJECT_UTI="$BUNDLE_ID.project"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"

# App version from crates/rapidsnap/Cargo.toml (must stay > v1's 1.5.1).
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/rapidsnap/Cargo.toml | head -1)"
[[ -n "$VERSION" ]] || { echo "could not read version from crates/rapidsnap/Cargo.toml" >&2; exit 1; }
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
DIST_DIR="${RAPIDSNAP_DIST_DIR:-$ROOT/dist}"
APP="$DIST_DIR/RapidSnap.app"
SIGN_IDENTITY="${RAPIDSNAP_SIGN_IDENTITY:-}"

if [[ -z "${SKIP_BUILD:-}" ]]; then
  cargo build --release -p rapidsnap
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$TARGET_DIR/release/rapidsnap" "$APP/Contents/MacOS/$EXECUTABLE"
# The release profile doesn't strip (rustc's rust-objcopy is broken in this
# toolchain) → strip local + debug symbols with the system tool (~20% smaller).
strip -x "$APP/Contents/MacOS/$EXECUTABLE"
# Only the assets the app loads at runtime (rs_core::assets): wallpapers, the
# tray template and the 256 px icon (welcome / about). The .icns goes to
# Contents/Resources; the other icon PNGs are source files for it.
mkdir -p "$APP/Contents/Resources/assets/icons"
cp -R "$ROOT/assets/wallpapers" "$APP/Contents/Resources/assets/wallpapers"
for f in tray-template.png tray-template@2x.png app-icon-256.png; do
  cp "$ROOT/assets/icons/$f" "$APP/Contents/Resources/assets/icons/$f"
done
find "$APP/Contents/Resources/assets" -name '.DS_Store' -delete
# Sanity: everything rs_core::assets / the app loads at runtime must be there.
for f in icons/tray-template@2x.png icons/app-icon-256.png wallpapers/wallpaper-01.jpg wallpapers/wallpaper-11.jpg; do
  [[ -f "$APP/Contents/Resources/assets/$f" ]] || { echo "missing asset: $f" >&2; exit 1; }
done
cp "$ROOT/assets/icons/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>RapidSnap</string>
  <key>CFBundleDisplayName</key><string>RapidSnap</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>${EXECUTABLE}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>RapidSnap</string>
  <!-- Screen Recording has no Info.plist usage key; TCC shows its own prompt.
       These strings are shown for related permissions / future use. -->
  <key>NSScreenCaptureUsageDescription</key>
  <string>RapidSnap needs Screen Recording access to capture screenshots.</string>
  <key>NSAppleEventsUsageDescription</key>
  <string>RapidSnap uses Apple Events to read the desktop wallpaper for backgrounds.</string>
  <key>NSDesktopFolderUsageDescription</key>
  <string>RapidSnap saves screenshots to your Desktop by default.</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Image</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.png</string>
        <string>public.jpeg</string>
        <string>public.tiff</string>
        <string>org.webmproject.webp</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>RapidSnap Project</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>CFBundleTypeIconFile</key><string>AppIcon</string>
      <key>LSItemContentTypes</key>
      <array><string>${PROJECT_UTI}</string></array>
    </dict>
  </array>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>${PROJECT_UTI}</string>
      <key>UTTypeDescription</key><string>RapidSnap Project</string>
      <key>UTTypeConformsTo</key>
      <array><string>public.data</string><string>public.zip-archive</string></array>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key><array><string>rsnap</string></array>
      </dict>
    </dict>
  </array>
</dict>
</plist>
PLIST

plutil -lint "$APP/Contents/Info.plist" >/dev/null
if [[ -n "$SIGN_IDENTITY" ]]; then
  # Stable identity: TCC (Screen Recording) keys the grant on the signing
  # certificate + bundle id, so rebuilt bundles keep the permission.
  if ! security find-identity -p codesigning | grep -Fq "\"$SIGN_IDENTITY\""; then
    echo "warning: code-signing identity \"$SIGN_IDENTITY\" not listed by 'security find-identity -p codesigning'; trying anyway" >&2
  fi
  codesign --force --deep -s "$SIGN_IDENTITY" -i "$BUNDLE_ID" "$APP"
  echo "Signed with identity: $SIGN_IDENTITY"
else
  # Ad-hoc: every rebuild has a new code hash → macOS forgets the Screen
  # Recording grant (re-grant after each bundle, or set RAPIDSNAP_SIGN_IDENTITY).
  codesign --force --deep -s - -i "$BUNDLE_ID" "$APP"
  echo "Signed ad-hoc (set RAPIDSNAP_SIGN_IDENTITY for a stable signature)"
fi
codesign --verify --deep "$APP"
du -sh "$APP"
echo "Built $APP ($BUNDLE_ID v$VERSION)"
