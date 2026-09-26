# RapidSnap v2 — Product & Feature Spec

> Status: Draft v0.1 · Stack: Rust + GPUI · Last updated: 2026-09-25

## 1. Vision

A fast, native-feeling screenshot tool that captures, annotates and beautifies a
screenshot in seconds. It should feel as light as RapidSnap (tiny, instant, low
memory) and produce results as polished as Xnapper (backgrounds, padding,
shadows, redaction), with **best-in-class annotation and text tools** as the
main differentiator.

## 2. Target

### Users
- **Developers & technical writers** — bug reports, docs, PR descriptions, code snippets.
- **Product / design / support teams** — feedback, walkthroughs, step-by-step guides.
- **Creators / marketers** — social-ready screenshots (X, LinkedIn, YouTube).

### Platforms
| Platform | Priority | Notes |
|---|---|---|
| macOS 14+ (Apple Silicon + Intel) | **P0** | Primary target; ScreenCaptureKit, Metal via GPUI |
| Windows 11 | P2 | GPUI supports it; capture via `xcap` |
| Linux (Wayland/X11) | P3 | Best effort; capture via portals |

### Product targets (non-functional)
| Metric | Target |
|---|---|
| Hotkey → selection overlay visible | < 100 ms |
| Capture → editor open | < 200 ms |
| Idle memory (menu bar only) | < 50 MB |
| Editor interaction | 120 fps on ProMotion, never below 60 fps |
| Export 4K PNG with effects | < 500 ms |
| App bundle size | < 20 MB |
| Works fully offline | Yes — no account, no telemetry by default |

## 3. Baseline — what RapidSnap v1 does today

Source of truth: `~/code/tmp/rapidsnap` (SwiftUI, macOS 13+, ~1.9k LOC).
v2 must reach **full parity with v1 first**, then match Xnapper.

| Area | v1 behavior (from source) |
|---|---|
| App shell | Menu bar app, no dock icon; welcome screen requests Screen Recording permission |
| Capture | Region only, by shelling out to `/usr/sbin/screencapture -i -r` (temp PNG); global hotkey via `KeyboardShortcuts` |
| Import | Open file (PNG/JPEG/TIFF) or open from clipboard |
| Tools | Select, **Arrow**, **Rectangle**, **Text** (Ellipse exists in the enum but is disabled) |
| Arrow | Straight line + fixed-size filled head (20×15 px); color + thickness (1–20) |
| Rectangle | Border + optional fill (same color); thickness; shapes < 5 px discarded |
| Text | Click to place (hover preview), inline edit, Enter commits, Esc cancels/removes; drag to move; double-click to re-edit; font size 12–72; color; **outline stroke** (width 0–10 + color, faked with blur + 4 shadows); system font only; single line |
| Selection | Only text annotations are selectable/movable/deletable; rect & arrow are fixed once drawn |
| Colors | Fixed palette picker (`ColorPickerView`) + native picker for text stroke |
| Undo | Pop-last-shape only (no redo, doesn't cover moves/edits) |
| Beautify | Padding (1–500), corner radius (1–100), shadow (1–50), background toggle, 11 bundled wallpapers + custom image backgrounds (persisted; ⌫ removes) |
| "Implicit" inset | Extra frame around the screenshot filled with the image's **edge color** (first pixel, overridable), gets the rounded corners + shadow — the screenshot looks like it extends seamlessly |
| Canvas | Zoom 0.1×–5× (pinch), initial 0.7× for large images, scrollable |
| Export | Copy (⌘C, optional auto-close editor after copy), Save (⌘S) via save panel |
| Preferences | Capture shortcut, launch at login, auto-close on copy; all tool/beautify settings remembered (`@AppStorage`) |

### v1 gaps v2 should fix
- Rect/arrow can't be selected, moved, restyled or deleted after drawing.
- Undo is not a real history; no redo.
- Text stroke is an approximation (blurred shadows) — render a true glyph outline instead.
- Export renders the SwiftUI view — preview and output can diverge; single renderer in v2.
- Save panel offers JPEG, but `savePNG` always writes PNG data.
- Edge color samples only the first pixel — sample the full border and use the dominant color.
- Capture depends on the `screencapture` CLI — no window mode, no custom overlay, no loupe.

## 4. Parity map — v1 vs Xnapper vs v2

Legend: ✅ supported · ◐ partial · — not present · **v2** = target milestone

| Area | Feature | RapidSnap v1 | Xnapper | v2 |
|---|---|---|---|---|
| Capture | Region | ✅ (system UI) | ✅ | M1 (own overlay) |
| Capture | Window / full screen | — | ✅ | M1 |
| Capture | Global hotkey | ✅ | ✅ | M1 |
| Capture | Open file / clipboard | ✅ | ✅ | M1 |
| Annotate | Text | ✅ | ✅ | M2 |
| Annotate | Arrow | ✅ straight | ✅ | M2 (+ curved in M3) |
| Annotate | Rectangle | ✅ | ✅ | M2 |
| Annotate | Ellipse | — (disabled) | ✅ | M2 |
| Annotate | Edit shapes after drawing | ◐ text only | ✅ | M2 |
| Annotate | Redact (solid / blur) | — | ✅ | M3 |
| Annotate | Auto-redact (emails, keys, cards, IPs) | — | ✅ | M5 |
| Beautify | Wallpaper / custom backgrounds | ✅ | ✅ | M4 |
| Beautify | Gradient backgrounds | — (wallpaper images only; ARCHITECTURE.md is outdated) | ✅ | M4 |
| Beautify | Padding, corners, shadow | ✅ | ✅ | M4 |
| Beautify | Edge-color inset ("Implicit") | ✅ | ✅ (inset) | M4 |
| Beautify | Aspect-ratio presets | — | ✅ | M4 |
| Output | Copy / save | ✅ | ✅ | M1 |
| Output | Drag-out | — | ✅ | M4 |
| Output | Share link / cloud | — | ✅ | Non-goal (first release) |

## 5. Annotation tools (core focus)

### 5.1 Shared behavior for every annotation
- **Vector objects**, not pixels — every annotation stays editable until export.
- Select, move, resize via handles; multi-select (⇧-click, marquee).
- Z-order: bring forward / send backward / to front / to back.
- Duplicate (⌘D), delete (⌫), copy/paste between screenshots.
- Unlimited **undo/redo** (⌘Z / ⇧⌘Z) — command-based history.
- **Snapping**: to other objects' edges/centers and to image edges, with guides.
- **Modifier keys**: ⇧ constrain (square / circle / 45° angles), ⌥ draw from center.
- Style panel: stroke color, fill color, stroke width, opacity, corner radius, shadow.
- **Style memory**: each tool remembers its last-used style; user-defined presets.
- Color palette with recent colors + eyedropper sampling from the screenshot.
- Keyboard shortcut for every tool (single-letter, e.g. `A` arrow, `R` rect, `T` text).

### 5.2 Shapes
| Tool | Key | Options |
|---|---|---|
| Rectangle | `R` | Outline / filled / outline+fill, corner radius, dashed stroke |
| Ellipse | `O` | Outline / filled, dashed stroke |
| Line | `L` | Width, dashed, caps (round / square) |
| Arrow | `A` | Styles: straight, **curved** (draggable control point), double-headed; head styles (filled triangle, open, round); tapered stroke |
| Freehand pen | `P` | Smoothed strokes, pressure-insensitive width |
| Highlighter | `H` | Semi-transparent marker, multiply blend, straight-line with ⇧ |
| Step counter | `N` | Numbered circles (1, 2, 3…), auto-increment, restyle / renumber; letters option (A, B, C) |
| Callout / speech bubble | `C` | Rect bubble with draggable tail, text inside |
| Spotlight | `S` | Dim everything except the selected rect/ellipse |
| Magnifier | `M` | Circular zoomed loupe of a region (2×–4×) with border |

### 5.3 Obfuscation
| Tool | Key | Options |
|---|---|---|
| Blur | `B` | Gaussian blur, adjustable strength |
| Pixelate | `X` | Block size |
| Solid redact | `D` | Solid color bar (default black) |
| Auto-redact (M5) | — | OCR-based detection of emails, API keys, credit cards, IP addresses, phone numbers; review list before applying |

> Obfuscation must be **destructive on export** — blurred/redacted pixels are
> never recoverable from the exported file.

### 5.4 Text annotation
- Click to place; drag to create a fixed-width wrapping box.
- **Inline editing** on canvas with caret, selection, IME support, copy/paste.
- Multi-line, alignment (left / center / right).
- Font family (system fonts + bundled defaults), size, weight, italic.
- Text color; **background styles**: none, solid pill, semi-transparent,
  outline/stroke around glyphs (legible on any background).
- Optional drop shadow.
- Presets: *Title*, *Label*, *Note*, *Code* (monospace).
- Double-click an existing text to edit; resize box re-wraps text.
- Text inside callouts and step counters uses the same engine.

## 6. Capture

- Modes: **region**, **window** (hover to highlight, `Space` toggles from region), **full screen**, **repeat last region**.
- Region overlay: crosshair, live dimensions, magnifier loupe with pixel color, arrow-key nudging.
- Multi-display and mixed-DPI (Retina) correct.
- Window capture with optional transparent background + window shadow.
- Delayed capture (3 s / 5 s / 10 s).
- Open existing image (drag onto menu bar icon / dock / ⌘O) and paste from clipboard.
- Menu bar app with configurable global hotkeys.

## 7. Beautify (Xnapper parity)

- Backgrounds: bundled wallpapers (port v1's 11), solid, linear/radial gradient presets, custom image (persisted library), current desktop wallpaper (with blur/brightness).
- Padding (uniform or per-side), **edge-color inset** (v1 "Implicit"), rounded corners, drop shadow (size, opacity, offset).
- **Auto-balance**: center content and choose padding automatically.
- Aspect-ratio presets: Free, 1:1, 4:3, 16:9, X/Twitter, LinkedIn, YouTube, Instagram.
- Optional window frame chrome (macOS-style traffic lights, browser bar).
- Watermark / signature (text or image, corner position).
- Crop and rotate the underlying screenshot.

## 8. Output

- Copy to clipboard (⌘C) as PNG.
- Save (⌘S) — PNG / JPEG / WebP, 1× / 2× scale, configurable default folder + filename template.
- Drag the result out of the editor into any app.
- "Capture → copy" instant mode (skip editor).
- Recent captures history (local).
- Re-open a saved project file (`.rsnap`) with annotations still editable.

## 9. Technical approach

| Concern | Choice |
|---|---|
| UI framework | **GPUI** |
| Widgets (toolbars, panels, pickers, sliders) | **gpui-component** |
| Canvas interaction | Custom GPUI element: hit-testing, handles, selection |
| Final image rendering | **tiny-skia** — single renderer used for both preview bitmap and export, so preview == export |
| Text shaping / layout | `cosmic-text` (for export parity with tiny-skia) |
| Blur / pixelate | `image` + `imageproc` (CPU, cached per region) |
| Screen capture | `screencapturekit` (macOS), `xcap` (Windows/Linux) |
| Global hotkeys | `global-hotkey` |
| Menu bar / tray | `tray-icon` (verify GPUI event-loop integration early) |
| Clipboard | GPUI clipboard (images); `arboard` fallback |
| OCR (auto-redact, M5) | macOS Vision framework via `objc2` |
| Document model | Serializable scene graph (`serde`), command-pattern undo stack |

### Architecture sketch
```
capture ──► Document { base_image, beautify_settings, annotations: Vec<Annotation> }
                │
                ├─► Editor (GPUI): toolbar · canvas · style panel
                │        user input ─► Command ─► Document (undo stack)
                │
                └─► Renderer (tiny-skia): Document ─► Pixmap
                         ├─► preview texture (shown in GPUI canvas)
                         └─► export (PNG/JPEG/WebP, clipboard, drag-out)
```

### Early technical risks (spike in M0)
1. `tray-icon` + `global-hotkey` coexisting with GPUI's run loop.
2. Transparent, borderless, full-screen overlay window across all displays.
3. Preview performance: incremental re-render of the tiny-skia pixmap at 60+ fps on 5K images (dirty-rect rendering, cached blur layers).
4. Text editing on canvas with IME, using GPUI's text input while rendering with cosmic-text for export.

## 10. Milestones

| Milestone | Scope |
|---|---|
| **M0 — Spikes** | GPUI window + tiny-skia preview + PNG export; tray + hotkey; overlay window |
| **M1 — Capture MVP** | Menu bar app, welcome/permission screen, hotkeys, region/window/full capture, open file/clipboard, editor opens, copy/save, preferences (shortcut, launch at login, auto-close on copy) |
| **M2 — Annotations core (≥ v1 parity)** | Rect, ellipse, line, arrow, text, selection/move/resize, undo/redo, style panel |
| **M3 — Annotations advanced** | Curved arrows, pen, highlighter, step counter, callout, blur/pixelate/redact, spotlight, magnifier, snapping |
| **M4 — Beautify** | Backgrounds, padding, corners, shadow, ratio presets, auto-balance, watermark |
| **M5 — Smart** | OCR auto-redact, copy text from image (OCR), project files, history |
| **M6 — Polish & ship** | Settings, onboarding, signing/notarization, auto-update, Windows build |

## 11. Non-goals (first v2 release)
- Screen recording / GIF.
- Cloud upload, share links, accounts, team libraries.
- Scrolling capture.
- Mobile apps.

## 12. Open questions
- Ship v2 as an update to the existing RapidSnap (same bundle ID, migrate `UserDefaults` prefs + custom backgrounds) or as a new app?
- License / distribution: open source? Mac App Store (sandbox limits) vs. direct download?
- Pricing: free like RapidSnap, or paid like Xnapper?
- Minimum macOS version — v1 supports 13; v2 targets 14 for ScreenCaptureKit maturity. OK to drop 13?

---

*Generated with Claude AI — please review before distribution.*
