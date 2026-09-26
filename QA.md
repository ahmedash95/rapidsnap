# RapidSnap v2 — QA checklist

Final QA pass after the two parallel polish passes (editor + render, app + capture + smart + core), 2026-09-25.
Legend: ✅ works and is reachable from the UI (how it was verified) · ◐ partial (what's missing) · ❌ missing ·
— out of scope. `file:line` refers to the state at the end of this pass. `panel.rs` lines are from before the
side-panel redesign, so they may have moved.

Verification keys: **ST-E** editor self-test (`RAPIDSNAP_EDITOR_SELFTEST=1`, real GPUI events → export), **ST-A** app
self-test (23 checks, debug and bundle), **UT** unit tests, **CR** code review of the UI hook (the control or shortcut
exists and calls the implementation), **M** needs a human (see the manual checklist at the end).

## Build / integration

| Check | Status |
|---|---|
| `cargo build --workspace --all-targets` | ✅ 0 errors / 0 warnings in our crates (only the external `block v0.1.6` future-incompat note) |
| `cargo test --workspace` | ✅ 193 passed, 1 ignored |
| `cargo clippy --workspace --all-targets` | ✅ clean |
| Editor demo self-test | ✅ `SELFTEST PASS` |
| App self-test, debug binary | ✅ all 23 checks passed. Clipboard sentinel preserved, other-instance guard skipped the posted hotkey |
| App self-test, bundle (`open -n -W --env RAPIDSNAP_SELFTEST=1 … dist/RapidSnap.app`) | ✅ all 23 checks passed. Clipboard sentinel intact, no ERROR lines |
| Bundle smoke run (throwaway data dir, 16 s) | ✅ alive (RSS 98 MB with the startup work), log clean (`tray icon + hotkeys installed` only). Only that pid was killed. The user's v1 and Preview copies were untouched |
| Stale `target-polish-*` dirs | ✅ deleted |

## Size / memory (release bundle, tray-only, onboarding done, `RAPIDSNAP_NO_WELCOME=1`, 12 s, 2 runs)

| | Before | After (`dist/RapidSnap.app`) |
|---|---|---|
| Bundle (`du -sh`) | 19 MB | 19 MB |
| Binary (stripped) | 14 997 600 B | 15 014 240 B (includes this pass's new code) |
| Linked frameworks (`otool -L`) | 31 | 28: CoreML, AVFoundation and UniformTypeIdentifiers removed |
| dyld images loaded at launch | 1102 | 1102 |
| RSS | 78.6 / 81.0 MB | 80.1 / 79.7 MB |
| Physical footprint | 18.1 / 18.8 MB | 17.7 / 17.8 MB |

All objc2 framework crates now use `default-features = false`, and each crate lists its header features. That
removes the load commands our own crates added (objc2-vision → CoreML and AVFoundation; objc2-screen-capture-kit →
AVFoundation and UTI).

CloudKit, CoreData and CoreImage stay because gpui-base and gpui-component use objc2-app-kit with default features.
CoreLocation stays because gpui uses objc2-user-notifications defaults. I tried a vendored `[patch.crates-io]` that
dropped those four as well (24 frameworks), and measured it. The process still loaded the same 1102 images, with the
same RSS (78.7 / 80.8 MB) and footprint (17.8 / 18.2 MB), because AppKit and Vision load all of them transitively. The
5 MB vendor tree bought nothing, so I didn't keep it (see the root `Cargo.toml` comment).

Takeaway: the idle RSS of about 80 MB is shared-cache framework pages. The app's own footprint is about 18 MB, well
under the 50 MB target. The bundle stays under 20 MB.

## §3 v1 parity

| Feature | Status | Where |
|---|---|---|
| Menu-bar app, no Dock icon (Accessory; Regular only while windows are open) | ✅ ST-A | `rapidsnap/src/main.rs:126`, `main.rs:146` |
| Welcome screen requests Screen Recording (+ Quit & Reopen) | ✅ ST-A | `rapidsnap/src/welcome.rs:200` |
| Region capture + global hotkey | ✅ ST-A (synthetic frames), M | `rapidsnap/src/capture.rs:126`, `tray.rs:309` |
| Open file (PNG/JPEG/TIFF/WebP/HEIC) | ✅ ST-A | `main.rs:403` (⌘O, tray), `main.rs:367` |
| Open from clipboard | ✅ CR | `main.rs:420` (⇧⌘V, tray) |
| Tools: select, arrow, rectangle, text (+ ellipse) | ✅ ST-E | `rs-editor/src/lib.rs:194` |
| Arrow color + thickness | ✅ ST-E, CR | `panel.rs:824`, `panel.rs:840` |
| Rectangle border + optional fill, thickness | ✅ CR | `panel.rs:831` |
| Text: click-to-place with hover preview, inline edit, commit/cancel, drag, double-click re-edit, size, color, outline stroke | ✅ ST-E | `model.rs:1202`, `view.rs` text scene, `panel.rs:950` |
| Colors: palette + picker | ✅ CR | `panel.rs:702` |
| Undo | ✅ ST-E | `model.rs:905` |
| Beautify: padding, corners, shadow, background toggle, 11 wallpapers, custom backgrounds (persisted, removable) | ✅ CR, UT | `panel.rs:1094`, `panel.rs:628`, `panel.rs:1199` |
| Edge-color inset ("Implicit"), overridable | ✅ CR, UT | `panel.rs:1260`, `rs-render/src/lib.rs:1193` |
| Canvas zoom 0.1×–5× (pinch, ⌘=/⌘-/⌘0/⌘1), fit for large images, scroll | ✅ ST-E | `view.rs:506`, `rs-editor/src/fit.rs:18` |
| Copy ⌘C (+ auto-close after copy) | ✅ ST-A (NSPasteboard round-trip) — **fixed** ⌘C semantics | `view.rs:600`, `view.rs:729` |
| Save ⌘S via save panel | ✅ CR | `view.rs:756` |
| Preferences: capture shortcut, launch at login, auto-close on copy, all tool/beautify settings remembered | ✅ ST-A (hotkey re-register + persisted) | `prefs_window.rs:244`, `:391`, `:427`, `view.rs:239` |

### v1 gaps

| Gap | Status | Where |
|---|---|---|
| Rect/arrow selectable, movable, restylable, deletable | ✅ ST-E | `model.rs:475`, `hit.rs:182` |
| Real undo history + redo | ✅ ST-E (undo → 25, redo → 26) | `model.rs:905` |
| True glyph outline for text stroke | ✅ UT | `rs-render/src/text.rs` |
| Single renderer (preview == export) | ✅ ST-E (live vs committed geometry Δ 0 px) | `render_worker.rs` |
| JPEG save writes JPEG | ✅ ST-A ("JPEG export is JPEG") | `view.rs:756` |
| Edge color = dominant border color | ✅ UT | `rs-render/src/lib.rs:1193` |
| Own capture overlay, window mode, loupe | ✅ ST-A, M | `overlay.rs` |

## §4 parity map

| Feature | Status | Where |
|---|---|---|
| Region (own overlay) | ✅ ST-A, M | `capture.rs:126` |
| Window / full screen | ✅ ST-A (full screen through the handler), M | `capture.rs:328`, `capture.rs:404` |
| Global hotkey | ✅ ST-A | `tray.rs:309` |
| Open file / clipboard | ✅ ST-A | `main.rs:403`, `main.rs:420` |
| Text / arrow (+ curved) / rectangle / ellipse | ✅ ST-E | `lib.rs:194` |
| Edit shapes after drawing | ✅ ST-E | `hit.rs:182` |
| Redact solid / blur / pixelate | ✅ UT (destructive) | `effects.rs:198` |
| Auto-redact (emails, keys, cards, IPs, phones) + review | ✅ CR, UT | `view.rs:980`, `view.rs:1874`, `rs-smart/src/redact.rs:120` |
| Wallpaper / custom / gradient backgrounds | ✅ CR | `panel.rs:1094`, `panel.rs:1121` |
| Padding, corners, shadow, inset | ✅ CR | `panel.rs:1225`–`1272` |
| Aspect-ratio presets | ✅ CR | `panel.rs:1286` |
| Copy / save | ✅ ST-A | `view.rs:700`, `view.rs:756` |
| Drag-out | ✅ CR, M | `view.rs:934`, `dragout.rs` |
| Share link / cloud | — non-goal | |

## §5.1 Shared annotation behaviour

| Feature | Status | Where |
|---|---|---|
| Vector objects, editable until export | ✅ ST-E | `rs-core` Document |
| Select / move / resize handles; ⇧-click + marquee multi-select; multi-resize | ✅ ST-E | `model.rs:475`, `model.rs:494` |
| Z-order: forward / backward / front / back (⌘] ⌘[ ⌥⌘] ⌥⌘[, context menu, Arrange buttons) | ✅ CR | `lib.rs:224`, `model.rs:892`, `panel.rs:1025` |
| Duplicate ⌘D, delete ⌫, copy/paste between screenshots | ✅ CR — **fixed**: ⌘C no longer copies annotation JSON by accident | `lib.rs:214`, `view.rs:600`, `model.rs:936` |
| Undo/redo ⌘Z / ⇧⌘Z (command history) | ✅ ST-E | `lib.rs:184` |
| Snapping to objects and image edges, with guides (⌘ disables) | ✅ UT | `model.rs:342`, `snap.rs` |
| ⇧ constrain / ⌥ from center | ✅ UT | `model.rs:560` |
| Style panel: stroke, fill, width, opacity, corner radius, shadow | ✅ CR | `panel.rs:785` |
| Style memory per tool + user presets | ✅ CR | `view.rs:239`, `panel.rs:990` |
| Recent colors + eyedropper | ✅ both work (`view.rs:271`, `panel.rs:741`, `model.rs:1186`) — **fixed** (design pass, CONTRACTS.md §11): a picker drag records one recent color, after the picker settles (600 ms); covered by the panel self-test | |
| Single-letter shortcut per tool, inactive while typing | ✅ CR (tool keys live in `RsEditorCanvas`, which becomes `RsEditorText` while editing; panel inputs are outside the canvas context) | `lib.rs:194`, `view.rs:1963` |

## §5.2 Shapes

| Tool | Status | Where |
|---|---|---|
| Rectangle R: outline / filled / both, corner radius, dashed | ✅ CR | `style_map.rs:519`, `panel.rs:831`, `:846` |
| Ellipse O: outline / filled, dashed | ✅ CR | `style_map.rs:526` |
| Line L: width, dashed, caps | ✅ CR | `panel.rs:852` |
| Arrow A: straight / curved (control handle) / double; heads filled / open / round; tapered | ✅ CR, UT | `panel.rs:864`, `hit.rs:196` |
| Pen P: smoothed | ✅ UT | `rs-render/src/geom.rs` |
| Highlighter H: multiply blend, ⇧ straight | ✅ CR | `draw.rs:108`, `model.rs:596` |
| Step N: auto-increment, renumber, letters, restyle | ✅ CR | `panel.rs:887`, `model.rs:998` |
| Callout C: draggable tail, text inside | ✅ ST-E | `hit.rs:201` |
| Spotlight S: rect / ellipse, dim | ✅ CR | `panel.rs:906` |
| Magnifier M: 2×–4× loupe with border | ✅ CR | `panel.rs:912`, `hit.rs:206` |

## §5.3 Obfuscation

| Tool | Status | Where |
|---|---|---|
| Blur B (strength) / Pixelate X (block) / Solid redact D (black default, color) | ✅ UT (destructive export) | `panel.rs:915`, `:918`, `style_map.rs:580` |
| Auto-redact with review list (per item, solid / blur / pixelate) | ✅ CR (⇧⌘R + toolbar) | `view.rs:980`, `view.rs:1874` |

## §5.4 Text

| Feature | Status | Where |
|---|---|---|
| Click to place; drag for a wrapping box | ✅ ST-E | `model.rs:588` |
| Inline editing: caret, selection, IME, copy/paste | ✅ ST-E | `view.rs` `EntityInputHandler` |
| Multi-line, alignment | ✅ ST-E | `panel.rs:950` |
| Font family (system fonts), size, weight, italic | ✅ CR | `panel.rs:950` |
| Color; background none / pill / translucent / outline | ✅ CR | `panel.rs:982` |
| Drop shadow | ✅ CR | `panel.rs:937` (text uses `text.shadow`) |
| Presets Title / Label / Note / Code | ✅ CR | `panel.rs:815` |
| Double-click to edit; resize re-wraps | ✅ ST-E | `model.rs:429`, `hit.rs:213` |
| Same engine in callouts and step counters | ✅ UT | `rs-render/src/text.rs` |

## §6 Capture

| Feature | Status | Where |
|---|---|---|
| Region, window (hover, Space toggles), full screen, repeat last region | ✅ ST-A, M | `overlay.rs:277`, `capture.rs:417` |
| Crosshair, live dimensions, loupe with pixel color, arrow nudging | ✅ ST-A (hint bar), M | `overlay.rs:437`, `overlay.rs:591` |
| Multi-display, mixed DPI | ✅ ST-A (spanning selection, scale factor = display scale) | `capture.rs:306` |
| Window capture: transparent background / shadow switches | ✅ CR, M | `prefs_window.rs:511`, `:519`, `capture.rs:328` |
| Delayed capture 3 / 5 / 10 s (tray submenu + default delay pref, countdown) | ✅ CR | `tray.rs:191`, `capture.rs:65`, `prefs_window.rs:528` |
| Open existing image: Dock drop / Finder "Open With" / ⌘O / clipboard | ✅ ST-A (open-file path) | `main.rs:102` |
| Open existing image: **drag onto the menu-bar icon** | ◐ not supported (tray-icon's status-item view accepts no drops; would need a custom `NSDraggingDestination` on the status bar button) | `tray.rs:180` |
| Menu-bar app, configurable hotkeys (recorder suspends hotkeys) | ✅ ST-A | `prefs_window.rs:244` |

## §7 Beautify

| Feature | Status | Where |
|---|---|---|
| 11 wallpapers, solid, linear / radial gradient presets, custom image library, desktop wallpaper (blur / brightness) | ✅ CR | `panel.rs:1094`–`1219` |
| Padding uniform / per side; inset; corners; shadow size / opacity / offset | ✅ CR — **fixed** (design pass, CONTRACTS.md §11): shadow Offset X control added next to Offset Y | `panel.rs:1225`–`1272` |
| Auto-balance | ✅ CR | `panel.rs:1226` |
| Aspect ratios Free, 1:1, 4:3, 16:9, X, LinkedIn, YouTube, Instagram | ✅ CR | `rs-core/src/beautify.rs:246` |
| Window chrome (macOS / browser, dark) | ✅ CR | `panel.rs:1307` |
| Watermark: text, corner, opacity, size, color | ✅ CR | `panel.rs:1326` |
| Watermark: **image / signature** | ✅ CR — **fixed** (design pass, CONTRACTS.md §11): Type Text/Image, file picker, width (`WatermarkContent::Image`) | `panel.rs:1326` |
| Crop and rotate | ✅ ST-E | `model.rs:1149`, `model.rs:1092`, `panel.rs:1377` |

## §8 Output

| Feature | Status | Where |
|---|---|---|
| Copy ⌘C as PNG (+ TIFF, Retina DPI) | ✅ ST-A — **fixed** ⌘C after drawing | `view.rs:600`, `services.rs:114` |
| Save PNG / JPEG / WebP, 1× / 2× | ✅ ST-A (JPEG bytes), CR | `view.rs:756`, `panel.rs:1422`, Prefs → Output |
| JPEG quality | ✅ **fixed**: Preferences → Output → JPEG quality (was not settable anywhere). ✅ **Fixed** (design pass, CONTRACTS.md §11): the panel's Export → JPEG keeps the current quality and has a Quality slider (panel self-test checks it survives format switches) | `prefs_window.rs:43` |
| Default folder (Choose… / Reset; Desktop fallback) | ✅ CR | `prefs_window.rs:294`, `view.rs:760` |
| Filename template (tokens, preview, counter) | ✅ CR — **fixed** `{app}` token (was always empty) | `view.rs:51`, `view.rs:72` |
| Drag the result out | ✅ CR, M | `view.rs:934` |
| Instant copy mode (skip editor) | ✅ CR | `tray.rs:203`, `capture.rs:467` |
| Recent captures (tray submenu, clear) | ✅ ST-A ("capture added to history") | `tray.rs:201`, `capture.rs:541` |
| Re-open `.rsnap` with editable annotations (toolbar, ⌘O, Finder) | ✅ UT (project round-trip) | `view.rs:874`, `rs-smart/src/project.rs:130` |
| Copy text from image (OCR) ⇧⌘T | ✅ CR (Vision) | `view.rs:947`, `services.rs:93` |

## Things that could easily break (checked)

| Check | Result |
|---|---|
| ⌘C image vs annotation | **Fixed.** A freshly drawn shape stays selected with its tool active, so ⌘C copied annotation JSON instead of the screenshot. ⌘C now copies the image unless the Select tool is active with a selection. Right-click → Copy always copies the annotations (`CopySelection`). |
| Tool letters while typing | OK. Canvas context switches to text while editing; panel inputs are outside the canvas context. |
| Menu hotkeys vs registered | **Fixed.** Tray accelerators are shown only for hotkeys actually registered. Open from Clipboard now shows ⇧⌘V, which matches the binding. |
| Windows not coming to front (accessory app, hotkey while another app is active) | **Fixed (needs M).** `platform::bring_to_front` (deferred activate + `orderFrontRegardless` + key) runs for the editor, Preferences and Welcome. |
| File dialogs from the accessory app | OK. Every prompt calls `cx.activate(true)` first (`main.rs:403`, `view.rs:756`, `panel.rs:628`, `prefs_window.rs:294`). |
| Quit leaving processes / unsaved prefs | **Fixed.** Prefs are flushed on every quit path (`on_app_quit`, `main.rs:136`). The self-test exits 0 and the process ends. |
| Prefs persist across relaunch | ✅ ST-A (debounced save → reload) |
| Copy auto-close, 2× export, default folder, template | ✅ (see §8) |
| Self-test safety | **Hardened.** Never posts events without `CGPreflightPostEventAccess`. Posts Esc only while our overlay is the key window. Skips the global hotkey when another RapidSnap runs. Clipboard is saved and restored (sentinel verified before and after). |

## Side panel (handed to the design agent)

Not fixed in this pass because `panel.rs` was owned by the design agent. Items 1–4 were **fixed** later by the
design pass (CONTRACTS.md §11 "design" entry):

1. ✅ **Fixed.** Export → JPEG button resets the quality to 90 (`ExportFormatChoice::format`). Keep the current quality, or add a quality control.
2. ✅ **Fixed.** No UI for an image or signature watermark (`WatermarkContent::Image`).
3. ✅ **Fixed.** Beautify shadow: no horizontal offset control (only `offset_y`).
4. ✅ **Fixed.** Color picker `Change` events call `set_color` → `push_recent_color` for every intermediate color while dragging, which floods Recent. Push on release or commit only.
5. Only a blur slider for annotation shadows (no offset or color). Acceptable for the spec, noted.

## Out of scope

— M6 signing / notarization / auto-update / Windows build. — §11 non-goals (recording, cloud, scrolling capture, mobile).

## Counts

Feature rows in §3–§8 (94, not counting the share-link non-goal):

| | ✅ | ◐ | ❌ |
|---|---|---|---|
| Before fixes | 85 | 9 | 0 |
| After fixes | 90 | 4 | 0 |
| After the design pass | 93 | 1 | 0 |

Fixed feature rows (5):

* ⌘C semantics (3 rows)
* JPEG quality
* The `{app}` filename token

Also fixed from the breakage checks (3):

* Window activation / bring to front
* Quit-time prefs flush
* Tray accelerators

Remaining ◐:

* Drag onto the menu-bar icon (needs a custom status-item drag destination)
* ~~3 side-panel items handed off: recent-colors flood, shadow X offset, watermark image~~ — fixed by the design pass
* ~~The panel's JPEG quality reset (listed on the JPEG row, also handed off)~~ — fixed by the design pass

## Manual test checklist for the user

Needs a human with Screen Recording granted to `dist/RapidSnap.app`. Quit the Preview copy and v1 first: they share
the hotkeys.

1. **Region**: ⇧⌘2 on each display. Check the crosshair, live size, loupe hex color matches the pixel, arrow-key
   nudge, Enter keyboard selection, Esc / right-click cancel. Also drag a selection across both displays.
2. **Window**: ⌥⇧⌘2 (or Space in the overlay). Check the hover highlight. Capture with Shadow on/off and Transparent
   on/off (Preferences → Capture), then paste into Preview to check the shadow and alpha.
3. **Full screen**: ⇧⌘1 on each display (Retina and non-Retina). The editor shows the right px size.
4. **Front-most**: trigger a capture while another app (e.g. Safari) is frontmost. The editor must come to the front
   and take keys (type `R`, draw). Do the same for tray → Preferences… and tray → Open File… (the dialog must be in
   front).
5. **⌘C**: draw an arrow, press ⌘C, paste into Slack or Preview. You should get the image, not text.
6. **Drag-out**: drag the grip handle next to Save into Finder, Slack and Mail.
7. **Tray menu**: click every item, including Delayed Capture ▸ 3 s (countdown next to the icon), Recent Captures ▸
   (opens the editor), Instant Copy Mode (check mark, capture goes straight to the clipboard) and Quit (process gone:
   `pgrep -f RapidSnap.app`).
8. **Launch at login**: toggle it in Preferences → General (bundle in /Applications). Log out and back in.
9. **Persistence**: change the hotkey, format, JPEG quality and default folder, quit, relaunch. The values stay.

---

*Generated with Claude AI — please review before distribution.*
