# RapidSnap v2

A fast macOS menu-bar screenshot tool: capture a region, window or screen, annotate it with a full vector editor, give
it an Xnapper-style background, and copy or save it in seconds. It is built in Rust with
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (through gpui-component). Rendering uses
tiny-skia, one renderer for both the preview and the export, so what you see is what you get. Capture uses
ScreenCaptureKit and OCR uses Apple's Vision framework. It works fully offline, with no account and no telemetry.

* Product spec: [`PRODUCT.md`](PRODUCT.md)
* Architecture, crate contracts and changelog: [`CONTRACTS.md`](CONTRACTS.md)
* Feature audit and manual test checklist: [`QA.md`](QA.md)

## Features

**Capture**
* Region (custom overlay on every display: crosshair, live size, pixel loupe with hex color, arrow-key nudging,
  keyboard selection), window (hover to highlight, optional shadow and transparent background), full screen, and
  repeat last region.
* Multi-display and mixed-DPI (Retina) correct, including selections that span displays.
* Delayed capture (3 / 5 / 10 s) with a countdown next to the menu-bar icon.
* Open an image with ⌘O, Finder "Open With", or by dropping it on the Dock icon (PNG, JPEG, TIFF, WebP, HEIC), or
  paste one from the clipboard (⇧⌘V).

**Annotate**: every annotation stays an editable vector object until export. The editor has 15 tools, plus Select (`V`):

| Key | Tool | Key | Tool | Key | Tool |
|---|---|---|---|---|---|
| `R` | Rectangle | `H` | Highlighter | `B` | Blur |
| `O` | Ellipse | `N` | Step counter | `X` | Pixelate |
| `L` | Line | `C` | Callout | `D` | Solid redact |
| `A` | Arrow (straight, curved, double) | `S` | Spotlight | `T` | Text |
| `P` | Pen | `M` | Magnifier | `K` | Crop and rotate |

* Move and resize with handles, multi-select (⇧-click or marquee), undo/redo, duplicate, z-order, and copy/paste
  between screenshots.
* Snapping to objects and image edges with guides (hold ⌘ to disable). ⇧ constrains, ⌥ draws from the center.
* Style panel with stroke, fill, width, opacity, corner radius and shadow. Each tool remembers its style, and you can
  save your own presets. Recent colors and an eyedropper are included.
* Annotations can go anywhere on the canvas, including over the padding and background, not only on the screenshot.
* Blur, pixelate and redact are destructive on export: the hidden pixels can't be recovered from the file.

**Text**
* Click to place, or drag to make a wrapping box. Inline editing with caret, selection, IME and copy/paste.
* Multi-line text, alignment, system font families, size, weight and italic.
* Backgrounds: none, pill, translucent, or a true glyph outline. Optional drop shadow.
* Presets: Title, Label, Note, Code. Callouts and step counters use the same text engine.

**Beautify**
* Backgrounds: 11 bundled wallpapers, solid colors, linear and radial gradients, your own image library, or the
  current desktop wallpaper (with blur and brightness).
* Padding (uniform or per side), edge-color inset, rounded corners, and drop shadow (size, opacity, X/Y offset).
* Auto-balance, and aspect-ratio presets: Free, 1:1, 4:3, 16:9, X, LinkedIn, YouTube, Instagram.
* macOS or browser window chrome, and a text or image watermark.
* A single compact side panel (Style and Beautify tabs). Advanced rows sit behind "More". The toolbar collapses
  secondary actions into a "…" menu on narrow windows (the minimum is 720 × 500 pt).

**Output**
* Copy (⌘C) to the clipboard as PNG and TIFF with Retina DPI. You can close the editor automatically after a copy.
* Save (⌘S) as PNG, JPEG (adjustable quality) or WebP at 1× or 2×. You set the default folder and a filename template.
* Drag the result out of the editor into any app.
* Instant copy mode: a capture goes straight to the clipboard and skips the editor.

**Smart**
* Copy text from the image with OCR (⇧⌘T).
* Auto-redact (⇧⌘R): detects emails, API keys, card numbers, IP addresses and phone numbers, then shows a review list
  where you pick solid, blur or pixelate for each item.
* Recent captures history in the menu-bar menu.
* `.rsnap` project files (⇧⌘S) reopen with every annotation still editable.

## Requirements

* macOS 14 or later, Apple Silicon or Intel.
* Rust stable, 1.90 or later (edition 2024). This repo is built and tested with the rustup
  `stable-aarch64-apple-darwin` toolchain, which `scripts/bundle.sh` puts on `PATH` itself. For manual `cargo`
  commands:

  ```sh
  export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
  ```

* Xcode Command Line Tools only (`xcode-select --install`), for `codesign`, `strip` and `plutil`. You don't need the
  full Xcode, because GPUI is built with `runtime_shaders`, so there is no `xcrun metal` step.

## Quick start

```sh
cargo run -p rapidsnap                   # dev build and run; the menu-bar icon appears
cargo run -p rapidsnap -- image.png      # open a file straight into the editor

scripts/bundle.sh                        # release build → dist/RapidSnap.app (ad-hoc signed)
cp -R dist/RapidSnap.app /Applications/  # install (Launch at Login needs the bundle)
open /Applications/RapidSnap.app
```

`scripts/bundle.sh` options: `SKIP_BUILD=1` re-bundles the existing release binary, `RAPIDSNAP_DIST_DIR=/tmp/out`
changes the output folder, `RAPIDSNAP_SIGN_IDENTITY="RapidSnap Dev"` signs with a stable identity (see below), and
`CARGO_TARGET_DIR` is honored. The bundle uses RapidSnap v1's identity, because v2 ships as v1's successor: the
bundle ID `io.github.ahmedash95.rapidsnap`, the executable `RapidSnap` (the cargo binary `rapidsnap`, renamed), and
the version from `crates/rapidsnap/Cargo.toml` (`2.0.0`, higher than v1's 1.5.1). It registers the `.rsnap` project
type (`io.github.ahmedash95.rapidsnap.project`).

Because v1 and v2 share the bundle ID, copying the bundle to `/Applications` replaces v1. Quit v1 first. macOS
addresses both by the same ID, so `open -b` and AppleScript can reach either one. v2 doesn't read v1's sandbox
container (`~/Library/Containers/io.github.ahmedash95.rapidsnap`). v2 is signed differently from v1, so grant Screen
Recording again after you install it.

A bare `cargo run` binary can't use Launch at Login, and its Screen Recording entry is usually your terminal's. For
real use, run the bundle.

### Grant Screen Recording

1. Press a capture hotkey or use the menu-bar icon. The welcome window explains the permission and opens
   **System Settings → Privacy & Security → Screen & System Audio Recording**.
2. Turn on **RapidSnap**.
3. Click **Quit & Reopen** in the welcome window. macOS only applies the grant to a freshly started process.

Until you grant the permission, captures open the welcome window instead.

### Keep the grant across rebuilds (stable code signing)

macOS ties the grant to the app's code signature. The default ad-hoc signature (`codesign -s -`) changes with every
build, so each rebuild loses the grant. To keep it, sign every build with the same self-signed certificate:

1. Open **Keychain Access**, then **Keychain Access → Certificate Assistant → Create a Certificate…**
2. Set Name to `RapidSnap Dev`, Identity Type to **Self Signed Root** and Certificate Type to **Code Signing**, then
   click **Create**. The certificate and its key go into your login keychain.
3. Check it: `security find-identity -p codesigning` should list `"RapidSnap Dev"`. It shows as not trusted
   (`CSSMERR_TP_NOT_TRUSTED`), which is fine for local signing.
4. Bundle with `RAPIDSNAP_SIGN_IDENTITY="RapidSnap Dev" scripts/bundle.sh`. If `codesign` asks for keychain access,
   choose **Always Allow**.
5. Grant Screen Recording once more. After that, rebuilds signed with the same identity keep the grant.

If you granted an older ad-hoc build, first remove its stale entry from the Screen Recording list (the "–" button).

## Keyboard shortcuts

**Global hotkeys** (defaults; change them in **Preferences → Capture**):

| Action | Default |
|---|---|
| Capture region | ⇧⌘2 |
| Capture window | ⌥⇧⌘2 |
| Capture full screen | ⇧⌘1 |
| Repeat last region | unassigned |

> **Note:** RapidSnap v1 uses the same default hotkeys. If v1 (or another copy of v2) is running, macOS delivers the
> shortcut to only one of them, and Preferences shows a registration warning. Quit the other app, or assign different
> shortcuts.

**Capture overlay**:

| Key | Action |
|---|---|
| Drag | Select a region. Releasing captures immediately |
| Space (not dragging) | Toggle region / window mode |
| Hold Space while dragging | Move the selection |
| Enter | Start a keyboard selection at the cursor, or confirm it. In window mode: capture the hovered window |
| ↑ ↓ ← → (⇧ = 10 px) | Nudge the cursor, or resize the keyboard selection |
| Esc / right-click | Cancel |

**App**: ⌘O open image · ⇧⌘V open from clipboard · ⌘, Preferences · ⌘Q quit.

**Editor**:

| Key | Action |
|---|---|
| `V` `R` `O` `L` `A` `P` `H` `N` `C` `S` `M` `B` `X` `D` `T` `K` | Tools (see [Annotate](#features)); disabled while typing text |
| ⌘C | Copy the **image**. With the Select tool active and annotations selected, it copies those annotations instead. Right-click → Copy always copies the selected annotations |
| ⌘X / ⌘V | Cut / paste annotations (also between editor windows) |
| ⌘S / ⇧⌘S | Save image / save `.rsnap` project |
| ⌘Z / ⇧⌘Z | Undo / redo |
| ⌘D / ⌘A / ⌫ | Duplicate / select all / delete selection |
| ⌘] / ⌘[ / ⌥⌘] / ⌥⌘[ | Bring forward / send backward / bring to front / send to back |
| ↑ ↓ ← → (⇧ = ×10) | Nudge the selected annotations |
| Enter | Apply crop, or edit the selected text or callout |
| Esc | Cancel the current drag or crop, then deselect, then return to Select |
| ⌘= / ⌘- / ⌘0 / ⌘1 | Zoom in / out / fit / actual size (pinch or ⌘-scroll zooms too) |
| ⇧⌘T / ⇧⌘R | Copy text (OCR) / auto-redact |
| ⌘W | Close the editor |

**While editing text**: Enter or ⇧Enter inserts a newline. ⌘Enter, Esc or a click outside commits, and empty text is
removed. Standard macOS text keys work (⌥/⌘ arrows, ⌘A, ⌘C/⌘X/⌘V, ⌘Z/⇧⌘Z, ⌃⌘Space for the character palette).

## Preferences

Open from the menu-bar icon or with ⌘,. Every tool and beautify setting you change in the editor is remembered
automatically.

* **General**: launch at login (bundle only; move it to /Applications first), instant copy mode, close the editor
  after copying, history size (off / 10 / 25 / 50 / 100), and a button to show the welcome screen.
* **Capture**: record or remove the global shortcuts, include the window shadow, transparent window background, and
  the default capture delay.
* **Output**: default format (PNG / JPEG / WebP), JPEG quality, scale (1× / 2×), default folder, and filename
  template with a live preview.
* **About**: version and links.

## Project layout

| Path | Contents |
|---|---|
| `crates/rs-core` | Pure data model: Document, annotations, styles, beautify/export settings, preferences, undo history, sanitizing. No GPUI. |
| `crates/rs-render` | tiny-skia renderer (`Document → image`), layout and coordinate mapping, text engine (cosmic-text), encoding. |
| `crates/rs-capture` | ScreenCaptureKit / CoreGraphics capture, permissions, display and window lists, NSPasteboard clipboard, image import. |
| `crates/rs-smart` | Vision OCR, auto-redact detection, `.rsnap` projects, history, preferences persistence, background library, login item. |
| `crates/rs-editor` | GPUI editor window: toolbar, canvas (hit-testing, handles, snapping), side panel, inline text editing, copy/save/drag-out. |
| `crates/rapidsnap` | The app binary: menu-bar icon, global hotkeys, capture overlay, HUD, welcome and preferences windows, self-test. |
| `assets/` | Wallpapers, tray and app icons, logo concepts. |
| `scripts/` | `bundle.sh` (build `dist/RapidSnap.app`), `gen_tray_icon.py` (menu-bar template generator, no dependencies). |
| `docs/design/sidebar-research.md` | Research notes behind the side-panel design. |

## Development

```sh
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets
RUST_LOG=debug cargo run -p rapidsnap                                # verbose logging
cargo run -p rs-editor --example editor_demo                         # editor with every annotation kind
cargo run -p rs-render --example gallery --release -- /tmp/gallery   # render every feature + 4K benchmark
cargo run -p rs-capture --example probe                              # capture diagnostics
cargo run -p rs-smart --example ocr_live -- /tmp/ocr                 # live OCR check
```

**Self-tests**

```sh
# App end-to-end self-test (debug binary, isolated data dir); exit code 0 = all checks passed
RAPIDSNAP_DATA_DIR=$(mktemp -d) RAPIDSNAP_SELFTEST=1 target/debug/rapidsnap; echo "exit $?"

# Same against the bundle
open -n -W --env RAPIDSNAP_SELFTEST=1 --env RAPIDSNAP_DATA_DIR=/tmp/rs-selftest \
     --stdout /tmp/rs-selftest.log --stderr /tmp/rs-selftest.log dist/RapidSnap.app
grep SELFTEST /tmp/rs-selftest.log

# Editor self-test: drives real GPUI events, then quits
RAPIDSNAP_EDITOR_SELFTEST=1 RAPIDSNAP_EDITOR_SELFTEST_DIR=/tmp/x cargo run -p rs-editor --example editor_demo
```

The app self-test opens the welcome and preferences windows and runs the region overlay on every display (with
synthetic frames if Screen Recording isn't granted). It also covers a drag → editor, the clipboard copy round-trip
(your clipboard is saved and restored), hotkey re-registration, Esc routing and file open. If another RapidSnap is
running, it calls the capture handler directly and doesn't post global hotkeys.

| Variable | Effect |
|---|---|
| `RAPIDSNAP_DATA_DIR=<dir>` | Use this folder for preferences, history, backgrounds and the single-instance lock (recommended for testing) |
| `RAPIDSNAP_SELFTEST=1` | Run the app self-test, then exit |
| `RAPIDSNAP_EDITOR_SELFTEST=1` | Run the editor self-test when an editor opens |
| `RAPIDSNAP_EDITOR_SELFTEST_DIR=<dir>` | Output folder for the editor self-test |
| `RAPIDSNAP_EDITOR_SELFTEST_SHOT=1` | Also save window and side-panel screenshots (`RAPIDSNAP_PANEL_SHOT_DIR`, `RAPIDSNAP_PANEL_DARK=1` for dark mode) |
| `RAPIDSNAP_EDITOR_SELFTEST_KEEP_OPEN=1` | Keep the editor open after the self-test |
| `RAPIDSNAP_NO_WELCOME=1` | Don't open the welcome window at startup (for example, to measure idle memory) |
| `RAPIDSNAP_ASSETS_DIR=<dir>` | Override the assets folder |
| `RS_CAPTURE_SKIP_PREFLIGHT=1` | Skip the Screen Recording preflight check (debugging capture error paths) |
| `RUST_LOG=debug` | Verbose logging |

**Data locations** (or under `$RAPIDSNAP_DATA_DIR`):

| What | Where |
|---|---|
| Preferences | `~/Library/Application Support/io.github.ahmedash95.rapidsnap/preferences.json` |
| Capture history | `~/Library/Application Support/io.github.ahmedash95.rapidsnap/history/` |
| Custom backgrounds | `~/Library/Application Support/io.github.ahmedash95.rapidsnap/backgrounds/` |
| Drag-out temp files | `$TMPDIR/rapidsnap-drag/` |

Pre-release v2 builds stored data in `~/Library/Application Support/com.rapidsnap.v2/`. On first start, if the new
folder has no `preferences.json`, RapidSnap moves the old folder's contents into it and logs what it moved. It skips
the lock file and any name the new folder already has, and deletes nothing. The migration doesn't run when
`RAPIDSNAP_DATA_DIR` is set.

Quit RapidSnap v1 (and any other copy of v2) while you test, because they share the default hotkeys.

### Icon

The app icon is **Snap Mark · Graphite**: four crop brackets and a coral hand-drawn loop on a white body. It is drawn
on Apple's macOS 1024 template (an 824 px continuous-corner body with the shadow in the margin), so it sits at the
same size as other Dock icons. The source is `assets/logo-concepts/claude/snap-mark-white/v2/`, which has vector
masters for default, dark and tinted, separate 16 and 32 px masters, and layers for Icon Composer. The generator is in
`assets/logo-concepts/claude/_source/`.

To regenerate:

```sh
# 1. SVG masters + PNGs: build the resvg renderer in _source/renderer, then run gen.py / gen_white.py
#    and render each SVG (see assets/logo-concepts/claude/README.md).
# 2. App icon PNGs: copy snap-mark-white/v2/icon-{16,32,64,128,256,512,1024}.png to assets/icons/app-icon-<N>.png
#    (16 and 32 come from the micro/small masters).
# 3. AppIcon.icns: put those PNGs in an AppIcon.iconset (icon_16x16 … icon_512x512@2x), then
iconutil -c icns AppIcon.iconset -o assets/icons/AppIcon.icns
# 4. Menu-bar template (18x18 / 36x36, black + alpha; tray-icon draws it at 18 pt):
python3 scripts/gen_tray_icon.py
```

## Known limitations

* Dropping an image onto the **menu-bar icon** isn't supported. Use the Dock icon, Finder "Open With", ⌘O or the
  clipboard instead.
* WebP exports carry no DPI metadata (PNG and JPEG do).
* Idle memory: `ps` shows about 80 MB RSS, but most of that is shared system-framework pages. The app's own physical
  footprint is about 18 MB.
* The release bundle is about 19 MB.
* Builds are ad-hoc signed by default, so macOS forgets the Screen Recording grant after a rebuild unless you use the
  stable-signing setup above. There is no notarization.
* Out of scope for this release (milestone M6): Developer ID signing, notarization, auto-update and a Windows build.
  Also not planned: screen recording/GIF, cloud upload, scrolling capture and mobile apps.

---

*Generated with Claude AI — please review before distribution.*
