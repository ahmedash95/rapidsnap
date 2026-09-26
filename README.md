<p align="center"><img src="assets/icons/app-icon-256.png" width="128" alt="RapidSnap icon"></p>

<h1 align="center">RapidSnap</h1>

<p align="center">Capture, annotate, beautify and share screenshots from the macOS menu bar, in seconds.</p>

<p align="center">macOS 14+ · Rust · <a href="https://github.com/zed-industries/zed/tree/main/crates/gpui">GPUI</a> · offline, no account, no telemetry</p>

## What it is

RapidSnap is a menu-bar screenshot tool: capture a region, window or screen, annotate it in a vector editor, give it
a styled background, then copy, save or drag it out. v2 is a Rust rewrite on GPUI (via gpui-component). One tiny-skia
renderer draws both the preview and the export, so what you see is what you get.

## Features

**Capture**
* Region overlay on every display: crosshair, live size, pixel loupe with hex color, arrow-key nudging, keyboard selection.
* Window capture (hover to highlight, optional shadow and transparent background), full screen, repeat last region.
* Multi-display and mixed-DPI correct, including selections that span displays.
* Delayed capture (3 / 5 / 10 s) with a countdown next to the menu-bar icon.
* Open images with ⌘O, Finder "Open With", the Dock icon or the clipboard (⇧⌘V): PNG, JPEG, TIFF, WebP, HEIC.

**Annotate**: every annotation stays an editable vector object until export, and can go anywhere on the canvas,
including over the padding and background.

| Key | Tool | Key | Tool | Key | Tool |
|---|---|---|---|---|---|
| `V` | Select | `P` | Pen | `B` | Blur |
| `R` | Rectangle | `H` | Highlighter | `X` | Pixelate |
| `O` | Ellipse | `N` | Step counter | `D` | Solid redact |
| `L` | Line | `C` | Callout | `T` | Text |
| `A` | Arrow (straight, curved, double) | `S` | Spotlight | `K` | Crop and rotate |
| | | `M` | Magnifier | | |

* Handles, multi-select, undo/redo, duplicate, z-order, copy/paste between screenshots.
* Snapping with guides (hold ⌘ to disable); ⇧ constrains, ⌥ draws from the center.
* Per-tool styles, saved presets, recent colors and an eyedropper.
* Text: wrapping boxes, IME, system fonts, alignment, pill/translucent/outline backgrounds, Title/Label/Note/Code presets.
* Blur, pixelate and redact are destructive on export: hidden pixels can't be recovered from the file.

**Beautify**
* Backgrounds: 11 bundled wallpapers, solid colors, gradients, your own images, or the current desktop wallpaper.
* Padding, edge-color inset, rounded corners, drop shadow, auto-balance and aspect presets (1:1, 16:9, X, LinkedIn…).
* macOS or browser window chrome, and a text or image watermark.
* One compact side panel (Style and Beautify tabs) with advanced rows behind "More".

**Output and smart tools**
* Copy as PNG + TIFF with Retina DPI; save as PNG, JPEG or WebP at 1× or 2×; drag out into any app.
* Instant copy mode skips the editor entirely.
* OCR copy text (⇧⌘T) and auto-redact (⇧⌘R) for emails, API keys, card numbers, IPs and phone numbers.
* Recent captures in the menu-bar menu, and `.rsnap` project files that reopen fully editable.

## Install and first run

```sh
scripts/bundle.sh                        # release build → dist/RapidSnap.app (ad-hoc signed)
cp -R dist/RapidSnap.app /Applications/
open /Applications/RapidSnap.app
```

Then grant Screen Recording:

1. Press a capture hotkey or use the menu-bar icon. The welcome window opens
   **System Settings → Privacy & Security → Screen & System Audio Recording**.
2. Turn on **RapidSnap**.
3. Click **Quit & Reopen**. macOS only applies the grant to a freshly started process.

> **Replaces v1.** v2 uses RapidSnap v1's bundle ID (`io.github.ahmedash95.rapidsnap`) at version 2.0.0, so installing
> it over `/Applications/RapidSnap.app` upgrades v1. Quit v1 first and grant Screen Recording again (the signature
> differs). v2 doesn't read v1's sandbox container. Both use the same default hotkeys, so only one should run.

## Keyboard shortcuts

### Global (change in Preferences → Capture)

| Action | Default |
|---|---|
| Capture region | ⇧⌘2 |
| Capture window | ⌥⇧⌘2 |
| Capture full screen | ⇧⌘1 |
| Repeat last region | Unassigned |
| Open image / from clipboard | ⌘O / ⇧⌘V |
| Preferences / quit | ⌘, / ⌘Q |

### Capture overlay

| Key | Action |
|---|---|
| Drag | Select a region; releasing captures |
| Space | Toggle region / window mode (hold while dragging to move the selection) |
| Enter | Start or confirm a keyboard selection; in window mode, capture the hovered window |
| Arrows (⇧ = 10 px) | Nudge the cursor or resize the keyboard selection |
| Esc / right-click | Cancel |

### Editor

| Key | Action |
|---|---|
| Tool letters | See the tools table above (disabled while typing text) |
| ⌘C | Copy the image; with Select active and annotations selected, copy those annotations |
| ⌘X / ⌘V | Cut / paste annotations, also between editor windows |
| ⌘S / ⇧⌘S | Save image / save `.rsnap` project |
| ⌘Z / ⇧⌘Z | Undo / redo |
| ⌘D / ⌘A / ⌫ | Duplicate / select all / delete |
| ⌘] / ⌘[ / ⌥⌘] / ⌥⌘[ | Forward / backward / to front / to back |
| Arrows (⇧ = ×10) | Nudge the selection |
| Enter | Apply crop, or edit the selected text or callout |
| Esc | Cancel drag or crop, then deselect, then return to Select |
| ⌘= / ⌘- / ⌘0 / ⌘1 | Zoom in / out / fit / actual size |
| ⇧⌘T / ⇧⌘R | OCR copy text / auto-redact |
| ⌘W | Close the editor |

While editing text, Enter inserts a newline; ⌘Enter, Esc or clicking outside commits. Standard macOS text keys work.

## Preferences

Open from the menu-bar icon or with ⌘,. Editor tool and beautify settings are remembered automatically.

| Tab | Settings |
|---|---|
| General | Launch at login (bundle only), instant copy, close after copy, history size, show welcome screen |
| Capture | Global shortcuts, window shadow, transparent window background, default delay |
| Output | Format, JPEG quality, scale, default folder, filename template with live preview |
| About | Version and links |

## Building from source

**Requirements**
* macOS 14 or later, Apple Silicon or Intel.
* Rust stable 1.90+ (edition 2024). `scripts/bundle.sh` puts the rustup `stable-aarch64-apple-darwin` toolchain on
  `PATH` itself; for manual commands run
  `export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"`.
* Xcode Command Line Tools (`xcode-select --install`). Full Xcode isn't needed: GPUI uses `runtime_shaders`.

**Build, run, test, bundle**

```sh
cargo run -p rapidsnap                   # dev build; the menu-bar icon appears
cargo run -p rapidsnap -- image.png      # open a file straight into the editor
cargo test --workspace
cargo clippy --workspace --all-targets
scripts/bundle.sh                        # → dist/RapidSnap.app
```

| `bundle.sh` variable | Effect |
|---|---|
| `SKIP_BUILD=1` | Re-bundle the existing release binary |
| `RAPIDSNAP_DIST_DIR=<dir>` | Output folder (default `dist/`) |
| `RAPIDSNAP_SIGN_IDENTITY=<name>` | Sign with a stable identity instead of ad-hoc |
| `CARGO_TARGET_DIR` | Honored when locating the release binary |

A bare `cargo run` binary can't use Launch at Login, and its Screen Recording grant belongs to your terminal.

**Optional: keep Screen Recording across rebuilds.** Ad-hoc signatures change every build, so macOS forgets the
grant. Sign with a stable self-signed certificate instead:

1. Keychain Access → Certificate Assistant → Create a Certificate… Name `RapidSnap Dev`, Identity Type
   **Self Signed Root**, Certificate Type **Code Signing**.
2. Check `security find-identity -p codesigning` lists it (`CSSMERR_TP_NOT_TRUSTED` is fine for local use).
3. Bundle with `RAPIDSNAP_SIGN_IDENTITY="RapidSnap Dev" scripts/bundle.sh` (choose **Always Allow** if prompted).
4. Remove any stale ad-hoc entry from the Screen Recording list, then grant once more.

## Development

### Self-tests and examples

```sh
# App end-to-end self-test (isolated data dir); exit 0 = all checks passed
RAPIDSNAP_DATA_DIR=$(mktemp -d) RAPIDSNAP_SELFTEST=1 target/debug/rapidsnap; echo "exit $?"

# Same against the bundle
open -n -W --env RAPIDSNAP_SELFTEST=1 --env RAPIDSNAP_DATA_DIR=/tmp/rs-selftest \
     --stdout /tmp/rs-selftest.log --stderr /tmp/rs-selftest.log dist/RapidSnap.app
grep SELFTEST /tmp/rs-selftest.log

# Editor self-test: drives real GPUI events, then quits
RAPIDSNAP_EDITOR_SELFTEST=1 RAPIDSNAP_EDITOR_SELFTEST_DIR=/tmp/x cargo run -p rs-editor --example editor_demo

cargo run -p rs-render --example gallery --release -- /tmp/gallery   # render every feature + 4K benchmark
cargo run -p rs-capture --example probe                              # capture diagnostics
cargo run -p rs-smart --example ocr_live -- /tmp/ocr                 # live OCR check
```

The app self-test covers the welcome and preferences windows, the region overlay on every display (synthetic frames
without Screen Recording), drag → editor, the clipboard round-trip (your clipboard is restored), hotkeys, Esc
routing and file open. Quit other RapidSnap copies while testing, since they share the default hotkeys.

### Environment variables

| Variable | Effect |
|---|---|
| `RAPIDSNAP_DATA_DIR=<dir>` | Folder for preferences, history, backgrounds and the single-instance lock |
| `RAPIDSNAP_SELFTEST=1` | Run the app self-test, then exit |
| `RAPIDSNAP_EDITOR_SELFTEST=1` | Run the editor self-test when an editor opens |
| `RAPIDSNAP_EDITOR_SELFTEST_DIR=<dir>` | Output folder for the editor self-test |
| `RAPIDSNAP_EDITOR_SELFTEST_SHOT=1` | Also save window and side-panel screenshots (`RAPIDSNAP_PANEL_SHOT_DIR`, `RAPIDSNAP_PANEL_DARK=1`) |
| `RAPIDSNAP_EDITOR_SELFTEST_KEEP_OPEN=1` | Keep the editor open after the self-test |
| `RAPIDSNAP_NO_WELCOME=1` | Skip the welcome window at startup |
| `RAPIDSNAP_ASSETS_DIR=<dir>` | Override the assets folder |
| `RS_CAPTURE_SKIP_PREFLIGHT=1` | Skip the Screen Recording preflight check |
| `RUST_LOG=debug` | Verbose logging |

### Data locations

| What | Where (or under `$RAPIDSNAP_DATA_DIR`) |
|---|---|
| Preferences | `~/Library/Application Support/io.github.ahmedash95.rapidsnap/preferences.json` |
| Capture history | `~/Library/Application Support/io.github.ahmedash95.rapidsnap/history/` |
| Custom backgrounds | `~/Library/Application Support/io.github.ahmedash95.rapidsnap/backgrounds/` |
| Drag-out temp files | `$TMPDIR/rapidsnap-drag/` |

Data from pre-release builds (`~/Library/Application Support/com.rapidsnap.v2/`) is moved over on first start if the
new folder has no `preferences.json`. Nothing is deleted, and the migration is skipped when `RAPIDSNAP_DATA_DIR` is set.

### Project layout

| Crate / path | Contents |
|---|---|
| `crates/rs-core` | Data model: document, annotations, styles, beautify/export settings, preferences, undo. No GPUI |
| `crates/rs-render` | tiny-skia renderer, layout and coordinate mapping, cosmic-text text engine, encoding |
| `crates/rs-capture` | ScreenCaptureKit / CoreGraphics capture, permissions, displays and windows, clipboard, import |
| `crates/rs-smart` | Vision OCR, auto-redact, `.rsnap` projects, history, preferences persistence, login item |
| `crates/rs-editor` | GPUI editor: toolbar, canvas, side panel, inline text editing, copy/save/drag-out |
| `crates/rapidsnap` | App binary: menu-bar icon, global hotkeys, capture overlay, welcome and preferences, self-test |
| `assets/` | Wallpapers, app and tray icons |
| `scripts/` | `bundle.sh` (build the app), `gen_tray_icon.py` (menu-bar template, no dependencies) |

## Icon

The app icon is **Snap Mark · Graphite**: four crop brackets and a coral hand-drawn loop on a white body, drawn on
Apple's macOS 1024 template so it matches other Dock icons. The PNGs live in `assets/icons/app-icon-<N>.png`, and
`AppIcon.icns` is built from them with `iconutil -c icns AppIcon.iconset -o assets/icons/AppIcon.icns`. The menu-bar
template glyph is generated with `python3 scripts/gen_tray_icon.py`.

## Known limitations

* Dropping an image onto the menu-bar icon isn't supported; use the Dock icon, "Open With", ⌘O or the clipboard.
* WebP exports carry no DPI metadata (PNG and JPEG do).
* Idle memory shows about 80 MB RSS in `ps`, mostly shared framework pages; the app's own footprint is about 18 MB.
* Builds are ad-hoc signed and not notarized. Developer ID signing, auto-update and Windows are out of scope for now.

## Docs

* [`PRODUCT.md`](PRODUCT.md): product spec
* [`CONTRACTS.md`](CONTRACTS.md): architecture, crate contracts and changelog
* [`QA.md`](QA.md): feature audit and manual test checklist
* [`docs/design/sidebar-research.md`](docs/design/sidebar-research.md): research behind the side-panel design

---

*Generated with Claude AI — please review before distribution.*
