# RapidSnap v2 — CONTRACTS

The single source of truth for the parallel agents. Read this fully before
touching code. Product spec: `PRODUCT.md`. v1 reference app: `~/code/tmp/rapidsnap`.

> Source files are authoritative for field-level docs (every public item has a
> doc comment). This file gives the map, the semantics that cross crate
> boundaries, the rules, and the verified GPUI recipes.

---

## 0. TL;DR for every agent

```sh
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
cd /Users/ahmed.ammar/Code/tmp/rapidsnap-v2
cp -cR target target-<yourname>          # APFS clone of the prebuilt GPUI (instant)
export CARGO_TARGET_DIR=$PWD/target-<yourname>
cargo build -p <your-crate> && cargo test -p <your-crate>
```

* (2026-09-25, integration) Per-agent `target-*` dirs were deleted; the integrated workspace builds in `target/`. Re-clone `target` as above if you need your own.
* Only edit files inside **your crate** (plus the CONTRACTS.md Changelog, §9).
* Never edit the root `Cargo.toml`, never rename/remove anything in `rs-core`.
* No `todo!()`, `unimplemented!()`, `panic!()` in non-test code; return `Err`.
* Do not `git commit` unless the orchestrator tells you to.

---

## 1. Crate map & ownership

| Crate | Path | Owner agent | Purpose |
|---|---|---|---|
| `rs-core` | `crates/rs-core` | foundation — **FROZEN** (add-only) | Pure data: Document, annotations, styles, beautify/export settings, preferences, undo history, geometry, asset paths. serde everywhere. No GPUI. |
| `rs-render` | `crates/rs-render` | **render agent** | tiny-skia renderer: `Document → RgbaImage`; layout/coordinate mapping; encoding. Single renderer for preview + export. |
| `rs-capture` | `crates/rs-capture` | **capture agent** | Screen capture (ScreenCaptureKit / CoreGraphics), permissions, display/window enumeration, clipboard, image import. |
| `rs-smart` | `crates/rs-smart` | **smart agent** | OCR (Vision), auto-redact detection, `.rsnap` project files, capture history, preferences persistence, custom background library, login item. |
| `rs-editor` | `crates/rs-editor` | **editor agent** | GPUI editor window: toolbar, canvas element (hit-testing, handles), style panel, text editing, copy/save/drag-out. |
| `rapidsnap` | `crates/rapidsnap` | **app agent** | Binary: GPUI app, tray, global hotkeys, capture overlay (region/window), welcome/permission screen, preferences window, flow glue. |

### Dependency graph (enforced — no other edges)

```
rapidsnap ──► rs-editor ──► rs-render ──► rs-core
    │                                        ▲
    ├──► rs-render                           │
    ├──► rs-capture ─────────────────────────┤
    ├──► rs-smart ───────────────────────────┘
    └──► rs-core
```

`rs-editor` must NOT depend on rs-capture / rs-smart (the app injects
behaviour via callbacks / `PreferencesGlobal`). `rs-render`, `rs-capture`,
`rs-smart` must NOT depend on GPUI.

---

## 2. Pinned toolchain & dependencies (verified building together)

* rustc/cargo **1.98.1** stable aarch64 (not on PATH — see §0). Edition **2024**, resolver 2.
* **GPUI**: `gpui = { package = "gpui-pre", version = "=0.3.6" }` (crates.io snapshot of Zed's GPUI, published 2026‑09‑21).
  Zed no longer needs a git pin: gpui-component pins the exact snapshot and we match it.
* **gpui-component / gpui-kit**: git `https://github.com/longbridge/gpui-component`, **rev `9765ae2c9a5eccfa13891248a445991e6f6a09d8` (tag v0.6.6)**, which requires `gpui-pre =0.3.6`.
  Use `gpui-kit` (the facade: `use gpui_kit::*` = GPUI, `gpui_kit::component::*` = widgets).
* **Metal shaders**: only Command Line Tools are installed (no `metal` compiler). `gpui-pre-platform` feature
  **`runtime_shaders`** is enabled (by gpui-kit's own workspace dep and again in our root `Cargo.toml`), so
  shaders compile at runtime. Verified: clean build, no `xcrun metal` invoked, windows render.
* Others (root `[workspace.dependencies]`, all resolved in `Cargo.lock`):
  serde 1 (+derive,rc), serde_json 1, anyhow 1, thiserror 2, uuid 1 (v4, serde), log 0.4, env_logger 0.11,
  dirs 7, chrono 0.4, base64 0.23, zip 8 (deflate), regex 1, smol 2, futures 0.3,
  tiny-skia **0.12**, cosmic-text **0.19**, image 0.25 (png,jpeg,tiff,webp,bmp,gif), imageproc **0.27** (no default features),
  tray-icon **0.25**, global-hotkey **0.8**, arboard 3.6 (image-data), xcap 0.9, rfd 0.17, raw-window-handle 0.6,
  objc2 0.6, block2 0.6, dispatch2 0.3, objc2-foundation / -app-kit / -core-foundation / -core-graphics / -vision / -screen-capture-kit / -image-io 0.3 with **`default-features = false`** (+ `std`): each crate lists the header features it uses in its own Cargo.toml (`objc2-app-kit = { workspace = true, features = ["NSPasteboard", …] }`). A missing type/method → add the header named in the item's `#[cfg(feature = …)]` (usually the class / header name).
* `[profile.dev.package."*"] opt-level = 2` — deps are optimized in dev (GPUI is unusable at -O0). Your own crate stays -O0 for fast rebuilds.

Adding dependencies: only in **your crate's** `Cargo.toml`, `foo.workspace = true` when declared above;
otherwise a plain version (`foo = "1.2"`) in your crate and note it in the Changelog. `Cargo.lock` is shared:
if a concurrent agent's resolution drops your new entry, cargo re-adds it on your next build (self-healing) — just rebuild.

---

## 3. Coordinate spaces & units (READ THIS)

| Space | Origin / axes | Unit | Used by |
|---|---|---|---|
| **Base image px** | top-left of `Document.base_image`, y down | physical pixel | `ImageTransform.crop` (`IRect`) only |
| **Content space** | top-left of the base image **after crop, then clockwise rotation**, y down | base-image pixel | **all annotation geometry and all annotation style lengths** (stroke width, font size, radius, blur sigma, …) |
| **Logical points (pt)** | — | px / `DocumentMetadata.scale_factor` | `BeautifySettings` lengths; `ToolStyle`/presets in Preferences; `ExportScale` |
| **Canvas px** | top-left of the rendered output image | output pixel | `rs_render::CanvasLayout` rects; renderer output |
| **Global screen pt** | top-left of *main* display, y down (CG global) | point | rs-capture frames/regions, `CaptureRegion` |
| **GPUI window px** | GPUI `Pixels` (logical) inside a window | logical px | editor UI; multiply by `window.scale_factor()` for physical |

* `DocumentMetadata.scale_factor` = physical px per pt of the capture (2.0 Retina, 1.0 files/clipboard).
* Converting a remembered `ToolStyle` (pt) into a new annotation: `prefs.tool_style(t).to_content_px(doc.metadata.scale_factor).create(t, from, to)`.
* Content size: `doc.content_size()` (px), `doc.content_size_pt()`.
* Crop/rotate: `ImageTransform::{base_to_content, content_to_base}`; applying `Command::SetImageTransform` remaps every annotation automatically (exact for 90° steps).

### Canvas layout (beautify → canvas) — `rs_render::compute_layout(doc, scale) -> CanvasLayout`

`scale` = output px per content px. `k = scale_factor * scale` = output px per pt.

```
canvas  ⊃ padding(pt·k, or auto-balance) ⊃ frame [shadow, corner_radius·k clip]
frame   = chrome title bar (height_pt·k, top)  +  inset band (inset.size·k, edge color)  +  content (content_size·scale)
aspect ratio: canvas grown (never shrunk) along one axis; extra split equally; frame origin rounded to whole px.
beautify.enabled == false → canvas == content rect exactly.
```

`CanvasLayout` API (all canvas px): `canvas_width/height: u32`, `scale`, `px_per_pt`, `frame_rect`, `chrome_rect: Option<Rect>`,
`inset_rect`, `content_rect`, `corner_radius`, and mappings `image_to_canvas(Point)`, `canvas_to_image(Point)`,
`image_rect_to_canvas(Rect)`, `canvas_rect_to_image(Rect)`, `len_to_canvas(f32)`, `canvas_size()`.
**The editor must hit-test by mapping the mouse into content space with `canvas_to_image`** (after undoing its own zoom/pan:
GPUI px → canvas px = `(mouse - canvas_origin) * (canvas_px / displayed_px)`).

### Annotations cover the whole canvas (canvas agent, 2026-09-25)
* Annotation geometry stays in **content space** (no new space; `.rsnap` files and history unchanged), but it is **not confined
  to the image**: coordinates may be negative or exceed `content_size()` — that is how an annotation sits over the padding,
  inset, chrome or background. Annotations therefore stay attached to the screenshot when padding / aspect / inset change.
* Canvas area in content space = `layout.canvas_rect_to_image((0,0,canvas_w,canvas_h))` (editor: `EditorModel::canvas_rect()`,
  `None` when canvas == image). The layout (padding, auto-balance, aspect) **never depends on annotations**.
* Render: background → frame shadow → chrome / inset → image (rounded frame clip applies to the image only) → annotations at
  canvas scale over the **whole canvas** (only the pixmap edge clips) → watermark. Blur / pixelate / redact / magnifier sample
  the composed canvas below them (background + frame + image + lower annotations) anywhere on it and stay destructive;
  spotlight dims the whole canvas except its holes. Beautify off → canvas == image, outside parts clip at the image edge.
* Crop / rotate (`SetImageTransform`) remaps outside annotations like any other (they stay attached to the same base-pixel
  positions, which may now be off-canvas). OCR / auto-redact still read only `content_image`.
* Sanitize: `Annotation::sanitize(bounds)` keeps the ±max(4·side, 4096) px window; `Document::sanitize` widens it to
  `Document::annotation_margin()` (whole beautify canvas size, incl. 160 pt auto-balance, aspect growth); NaN/inf/absurd
  values are still clamped. Editor paste uses the same margin and keeps pasted groups on the canvas (not the image).
* Editor: pointer area = whole canvas element; create / pick / move / resize / nudge / marquee / text / hover / cursor work
  over the padding; snapping adds canvas edges + center (`SnapTargets::with_canvas`), guides span the canvas; the eyedropper
  samples the composed preview bitmap (`EditorModel::canvas_sample`, base image fallback).

### Scale semantics
* Preview: editor picks `scale = zoom * window.scale_factor() / doc.scale_factor` (cap at 1.0 for big images) and displays the bitmap at `canvas_px / window.scale_factor()` logical px × any extra zoom.
* Export: `RenderOptions::for_export(doc, &export)` → `scale = export.scale.factor() / doc.scale_factor`.
  `ExportScale::X2` on a Retina capture = native pixels; `X1` halves it; on a 1× capture X2 upscales.
* Aspect-ratio exact values: Free=none, Square 1:1, FourThree 4:3, SixteenNine 16:9, Twitter 16:9 (1600×900), LinkedIn **1.91:1** (1200×627), YouTube 16:9 (1280×720), Instagram **4:5** (1080×1350). `AspectRatio::ratio()` returns w/h.
* Auto-balance padding (pt) = `clamp(0.08 × max(content_w_pt, content_h_pt), 16, 160)` — `rs_render::auto_balance_padding_pt`.

---

## 4. rs-core (FROZEN, add-only) — `use rs_core::*;`

Modules re-exported at the crate root: `geometry, color, style, text, annotation, tool, beautify, export, prefs, document, history` (+ `assets`, `test_support` as modules).

### geometry
* `Point { x: f32, y: f32 }` — `new, translate, distance, lerp`, `+ - *f32`, `Point::ZERO`.
* `Size { width, height }` — `new, scale, is_empty`.
* `Rect { x, y, width, height }` (normalized: w,h ≥ 0) — `new, from_origin_size, from_points(a,b), bounding(&[Point]) -> Option, normalized, origin, size, left/top/right/bottom, center, is_empty, contains (half-open), intersects, intersection -> Option, union, translate, outset(d), scale(s), corners() [tl,tr,br,bl]`.
* `IRect { x, y, width, height: u32 }` — `new, to_rect, clamp_to(w,h) -> Option`.

### color
* `Color { r, g, b, a: u8 }` **straight alpha**, serde as `"#RRGGBBAA"` (accepts `#RGB`, `#RRGGBB`). Consts `TRANSPARENT, BLACK, WHITE, RED (#FF3B30 default), YELLOW, GREEN, BLUE, PURPLE, ORANGE, GRAY`. Methods `rgb, rgba, with_alpha(f32), alpha_f32, to_f32, to_hex, from_hex, luminance`. `DEFAULT_PALETTE: [Color; 10]`.

### style (lengths in content px inside a Document)
* `LineCap { Butt, Round (default), Square }`
* `DashStyle { Solid, Dashed, Dotted }` — `dash_array(width) -> Option<[on, off]>` (Dashed 3w/2w, Dotted ~0/2w + round cap).
* `Stroke { color, width, dash, cap }` default red 4px solid round. `Stroke::new(color, width)`.
* `FillMode { Outline (default), Filled, OutlineAndFill }` — `has_stroke(), has_fill()`.
* `ShapeStyle { mode: FillMode, stroke: Stroke, fill: Color }`.
* `Shadow { color (alpha = opacity), blur (≈2σ px), offset_x, offset_y }` — `scaled(s)`.

### text
* `FontFamily { System (default), Monospace, Serif, Named(String) }` (serde `{"type":..,"name":..}`).
* `FontWeight(pub u16)` 100..900, consts `THIN..BLACK`, `REGULAR` default.
* `TextAlign { Left, Center, Right }`.
* `TextBackground { None, Pill{color, corner_radius, padding_x, padding_y}, SemiTransparent{same fields}, Outline{width, color} }` — Outline is a **true glyph stroke drawn behind the fill** (fixes v1 fake stroke). Pill/semi padding extends *outside* the text box.
* `TextPreset { Title, Label, Note, Code }` — `ALL, name(), style() -> TextStyle` (in pt).
* `TextStyle { family, size (px), weight, italic, color, align, line_height (× size, 1.2), background, shadow: Option<Shadow> }` — `scaled(s)`. Text shadow: if `None`, renderers use `Annotation.shadow`.

### annotation — PRODUCT.md §5 complete
`Annotation { id: AnnotationId, opacity: f32 (group opacity), shadow: Option<Shadow>, hidden: bool, locked: bool, kind: AnnotationKind }`
* `AnnotationId(pub Uuid)` — `new()`. Z-order = index in `Document.annotations` (0 = bottom).
* Methods: `new(kind)`, `with_shadow`, `with_opacity`, `duplicate()` (new id), `tool()`, `is_obfuscation()`, `geometric_bounds()` (approx; excludes stroke/shadow; text estimated), `translate(dx,dy)`, `map_points(&dyn Fn(Point)->Point)`.
* `AnnotationKind` (serde internally tagged `"type"`, snake_case):

| Variant | Struct fields | Semantics |
|---|---|---|
| `Rect` | `rect, style: ShapeStyle, corner_radius` | outline/filled/both, dashed via stroke |
| `Ellipse` | `rect, style` | inscribed in rect |
| `Line` | `start, end, stroke` | caps/dash from stroke |
| `Arrow` | `start (tail), end (tip), control: Option<Point>, stroke, head: ArrowHead, double_headed, tapered, head_scale` | `control` = quadratic Bézier control point (curved). `ArrowHead { FilledTriangle, Open, Round, None }`. `head_length() = max(width*4,10)*head_scale`, `head_width() = 0.75*head_length`. Tapered: width 0 at start → stroke.width at head base, filled polygon. |
| `Pen` | `points, stroke` | raw samples; renderer smooths (Catmull‑Rom) |
| `Highlighter` | `points, color, width` | **multiply** blend, flat caps; 2 points = straight |
| `Step` | `center, radius, value (1-based), label: StepLabel, fill, text_color, border: Option<Stroke>, font_family, font_weight` | `StepLabel { Number, UpperLetter, LowerLetter }`, `format(v)` (27 → "AA"); `label_text()`; next value: `Document::next_step_value(label)` |
| `Callout` | `rect, tail, corner_radius, style, text, text_style, padding` | tail tip at `tail` (none if inside rect); base on nearest edge, width min(24px·scale, edge/3); text wraps at `rect.width − 2·padding` |
| `Spotlight` | `rect, shape: SpotlightShape{Rect,Ellipse}, corner_radius, dim: Color` | one dim layer for all spotlights (at lowest spotlight's z), color = topmost spotlight's `dim`, holes for each |
| `Magnifier` | `source_center, source_radius, target_center, zoom (2–4), border: Stroke` | loupe radius `target_radius() = source_radius*zoom` |
| `Blur` | `rect, strength (σ px), corner_radius` | blurs everything *below* it; destructive |
| `Pixelate` | `rect, block_size` | blocks aligned to rect top-left, average color; destructive |
| `Redact` | `rect, color, corner_radius` | alpha forced to 255 |
| `Text` | `position (box top-left), width: Option<f32> (fixed wrap width), content ('\n' breaks), style: TextStyle, preset: Option<TextPreset>` | |

Annotations are **clipped to the content rect** when rendered.

### tool
* `ToolKind { Select, Rect, Ellipse, Line, Arrow, Pen, Highlighter, Step, Callout, Spotlight, Magnifier, Blur, Pixelate, Redact, Text, Crop }` — `ALL`, `shortcut()` (v r o l a p h n c s m b x d t k), `from_shortcut(char)`, `name()`, `creates_annotation()`.
* `ToolStyle` — flat bag of all style knobs (**pt**): `stroke, fill_mode, fill, corner_radius, opacity, shadow, text, arrow_head, arrow_double, arrow_tapered, arrow_curved, highlighter_color, highlighter_width, step_label, step_radius, step_text_color, spotlight_shape, spotlight_dim, magnifier_zoom, blur_strength, pixelate_block, redact_color, callout_padding`. `default_for(tool)`, `to_content_px(scale)`, `create(tool, from, to) -> Option<Annotation>` (drag semantics in doc comment).
* `StylePreset { id: Uuid, name, tool: Option<ToolKind>, style: ToolStyle }`.

### beautify (all lengths **pt**)
* `BeautifySettings { enabled, background, padding: Padding, inset: Inset, corner_radius, shadow: FrameShadow, aspect_ratio, auto_balance, chrome: WindowChrome, watermark: Option<Watermark> }` — default = v1 (padding 50, radius 20, shadow 20, wallpaper-01). `disabled()`.
* `Background { None, Solid{color}, LinearGradient{stops, angle_deg (0=→, 90=↓)}, RadialGradient{stops, center:[f32;2] normalized, radius (fraction of half-diagonal)}, Wallpaper{name}, CustomImage{path}, DesktopWallpaper{blur (σ pt), brightness (×)} }`. Images are **cover-fit, centered**. `GradientStop { offset, color }`, `gradient_presets() -> Vec<(&str, Background)>`.
* `Padding { top, right, bottom, left, linked }` — `uniform(v)`, `horizontal()`, `vertical()`.
* `Inset { enabled, size, color: InsetColor{Auto, Custom(Color)} }` — Auto = `rs_render::dominant_edge_color` (whole 1‑px border ring, 5‑bit buckets, most frequent, averaged).
* `FrameShadow { enabled, size (blur ≈2σ), opacity, offset_x, offset_y, color }`.
* `AspectRatio { Free, Square, FourThree, SixteenNine, Twitter, LinkedIn, YouTube, Instagram }` — `ALL, ratio(), label()`.
* `WindowChrome { None, MacOs{dark, title}, Browser{dark, url} }` — `height_pt()` 0/28/40.
* `Watermark { content: WatermarkContent{Text{text, style}, Image{path, width}}, position: Corner{TopLeft,TopRight,BottomLeft,BottomRight}, opacity, margin }` — drawn in canvas corner.
* **Crop & rotation are NOT in BeautifySettings** (deliberate): they change content space and must not be remembered in `last_beautify`. They live in `Document.image_transform`.

### export
* `ExportFormat { Png, Jpeg{quality 1..=100}, WebP (lossless) }` — `extension(), mime()`. JPEG flattens alpha on white.
* `ExportScale { X1, X2 (default) }` — `factor()`.
* `ExportSettings { format, scale, default_folder: Option<PathBuf> (None → ~/Desktop), filename_template, counter }` default template `"RapidSnap {date} at {time}"`.
* `render_filename(template, &FilenameContext) -> String` — tokens `{date}`=YYYY-MM-DD, `{time}`=HH.MM.SS, `{n}`=counter, `{w}` `{h}`=px, `{app}`; unknown tokens kept; `/` `:` → `-`. `FilenameContext { year, month, day, hour, minute, second, counter, width, height, app_name }` (caller supplies local time).

### prefs (`#[serde(default)]` everywhere → tolerant of missing/unknown fields)
* `Modifiers { cmd, shift, alt, ctrl }`, `Hotkey { modifiers, key }` — `key` = W3C `KeyboardEvent.code` (`"Digit2"`, `"KeyA"`, `"F5"`) = what `global_hotkey::hotkey::Code::from_str` parses. `display()` → `"⇧⌘2"`.
* `Hotkeys { region (⇧⌘2), window (⌥⇧⌘2), fullscreen (⇧⌘1), repeat_last (None) }`.
* `CaptureDelay { None, S3, S5, S10 }` — `seconds()`. `CaptureRegion { display_id: Option<u32>, rect (global pt) }`.
* `Preferences { version, hotkeys, launch_at_login, auto_close_on_copy (true), instant_copy, capture_delay, window_capture_shadow, window_capture_transparent (NEW), export, tool_styles: BTreeMap<ToolKind, ToolStyle>, style_presets, recent_colors (≤ MAX_RECENT_COLORS=16), last_beautify, last_capture_region, last_tool, onboarding_done, history_limit (50) }` — `tool_style(t)`, `remember_tool_style(t, s)`, `push_recent_color(c)`.

### document
* `RgbaImage { width, height, pixels: Arc<Vec<u8>> }` — straight RGBA8, tight rows. **serde writes only width/height** (pixels skipped); `is_loaded()` false after plain JSON deserialize. `new(w,h,Vec) -> Option`, `filled(w,h,rgba)`, `size()`, `pixel(x,y)`.
* `Rotation { Deg0, Deg90, Deg180, Deg270 }` (clockwise) — `degrees, cw, ccw, swaps_axes`.
* `ImageTransform { crop: Option<IRect> (base px), rotation }` — crop first, then rotate. `crop_rect(w,h)`, `content_size(w,h)`, `base_to_content`, `content_to_base`. `remap_annotations(&mut [Annotation], before, after, w, h)`.
* `CaptureSource { Unknown, Region, Window{app_name, title}, Fullscreen{display_id}, File{path}, Clipboard }`.
* `DocumentMetadata { created_at_ms, source, scale_factor, title, project_path }`.
* `DocumentId(pub Uuid)`, `DOCUMENT_FORMAT_VERSION = 1`.
* `Document { format_version, id, base_image, image_transform, beautify, annotations: Vec<Annotation>, metadata }` — `new(img, meta)`, `with_beautify`, `content_size`, `content_size_pt`, `index_of(id)`, `get`, `get_mut`, `next_step_value(label)`.

### history (command pattern; unit-tested)
* `Command` (serde-able) — `Add(Vec<Annotation>)` (append on top), `Insert(Vec<(usize, Annotation)>)`, `Remove(Vec<AnnotationId>)` (multi), `Update{before, after}` (move/resize/restyle/text edit; `Command::update(b, a)`), `Reorder{before, after}` (full id orders), `SetBeautify{before, after}` (`Command::set_beautify`), `SetImageTransform{before, after}` (remaps annotations), `Batch(Vec<Command>)`. `is_noop()`.
* `History` — `new()`, `apply(&mut doc, cmd)`, `apply_coalescing(&mut doc, cmd, key)` (merge a drag/slider/typing run into one step; `break_coalescing()` on mouse‑up/Release), `undo(&mut doc) -> Result<bool>`, `redo`, `can_undo`, `can_redo`, `clear`, `mark_saved`, `is_dirty`, `revision()` (bumps on every change — use to invalidate preview), `undo_len`, `redo_len`. No-op commands are not recorded. On error the doc is unchanged (batches roll back).
* `HistoryError { NotFound, DuplicateId, IndexOutOfBounds, InvalidReorder, IdMismatch }`.
* `apply_command(&mut doc, &cmd) -> Result<inverse>` (low level).
* `zorder::{bring_to_front, send_to_back, bring_forward, send_backward}(&doc, &[ids]) -> Command`.
* Copy/paste between screenshots: `AnnotationClipboard::to_json(&[Annotation]) -> String` (plain-text clipboard payload
  `{"rapidsnap_annotations":[...]}`) and `AnnotationClipboard::from_json(&str) -> Option<Vec<Annotation>>` (fresh ids, ready for `Command::Add`).

### assets
* `BUNDLED_WALLPAPERS: [&str; 11]` = `"wallpaper-01"…"wallpaper-11"` (v1 order); file `assets/wallpapers/<name>.jpg` (6000×4000 JPEG — downscale on load!).
* `assets_dir()` resolution: `$RAPIDSNAP_ASSETS_DIR` → `<exe>/../Resources/assets` (inside `.app`) → `<repo>/assets` (compile-time `CARGO_MANIFEST_DIR` of rs-core). Plus `bundle_resources_dir()`, `dev_assets_dir()`, `asset_path(rel)`, `wallpaper_path(name) -> Option`. `ASSETS_DIR_ENV`.
* Asset files: `assets/icons/AppIcon.icns`, `app-icon-{16,32,64,128,256,512,1024}.png`, `tray-template.png` (18×18), `tray-template@2x.png` (36×36; monochrome black+alpha "Snap Mark" glyph, tray-icon draws it at a fixed 18 pt height; regenerate with `scripts/gen_tray_icon.py`). App icon = "Snap Mark · Graphite", source `assets/logo-concepts/claude/snap-mark-white/v2/` (vector masters, generator in `assets/logo-concepts/claude/_source/`).

### test_support
* `all_kinds() -> Vec<AnnotationKind>` (every variant, incl. curved arrow & code text), `sample_document()` (200×100, scale 2, one of each).

---

## 5. rs-render (render agent) — public API (signatures frozen, bodies yours)

```rust
pub const MAX_CANVAS_SIDE: u32 = 16_384;
pub enum RenderError { ImageNotLoaded, TooLarge(u32,u32), Alloc(u32,u32), Image(image::ImageError), Io(std::io::Error) }
pub enum Quality { Fast, High }                       // preview vs export
pub struct RenderOptions { pub scale: f32, pub draw_annotations: bool, pub exclude: Vec<AnnotationId>, pub quality: Quality }
impl RenderOptions { fn preview(scale) -> Self; fn for_export(&Document, &ExportSettings) -> Self }
pub struct RenderOutput { pub image: RgbaImage /* straight RGBA */, pub layout: CanvasLayout }
pub struct Renderer;  // caches (decoded backgrounds, fonts, blur layers). One per editor window. !Sync.
impl Renderer {
    fn new() -> Self;
    fn set_desktop_wallpaper(&mut self, Option<PathBuf>);
    fn clear_caches(&mut self);
    fn render(&mut self, &Document, &RenderOptions) -> Result<RenderOutput, RenderError>;
    fn annotation_bounds(&mut self, &Annotation) -> Rect;              // content space, incl. stroke & exact text
    fn measure_text(&mut self, text: &str, &TextStyle, width: Option<f32>) -> Size;  // content px
}
pub fn compute_layout(&Document, scale: f32) -> CanvasLayout;          // §3 — implemented & tested
pub fn auto_balance_padding_pt(content_pt: Size) -> f32;
pub fn export(&mut Renderer, &Document, &ExportSettings) -> Result<(Vec<u8>, RgbaImage), RenderError>;
pub fn encode(&RgbaImage, ExportFormat) -> Result<Vec<u8>, RenderError>;  // implemented
pub fn content_image(&Document) -> Result<RgbaImage, RenderError>;       // crop+rotate applied; implemented
pub fn dominant_edge_color(&RgbaImage) -> Color;                          // implemented
pub fn rgba_to_pixmap(&RgbaImage) -> Option<tiny_skia::Pixmap>;           // premultiply
pub fn pixmap_to_rgba(&tiny_skia::Pixmap) -> RgbaImage;                   // demultiply
```

Status (render agent): **everything implemented** — all 14 annotation kinds, every beautify option (backgrounds incl.
desktop wallpaper blur+brightness, per-side padding, inset, radius, frame shadow, aspect, auto-balance, macOS/browser
chrome light+dark, text/image watermark), crop+rotate, group opacity + drop shadows, destructive obfuscation, PNG/JPEG/WebP.
Rendering order: background → frame shadow → frame (chrome, inset) → content image → annotations (z-order, over the whole canvas — see §3 "Annotations cover the whole canvas") → watermark.

**NEW public API (additive; render agent)** — all content space unless noted:
```rust
impl Renderer {
    fn preload(&mut self, &Document);                              // decode background image ahead of time (wallpapers are 6000×4000)
    fn hit_test(&mut self, &Annotation, p: Point, tolerance: f32) -> bool;   // precise: strokes ±w/2+tol, filled interiors, text box
}
pub fn annotation_bounds(&Annotation) -> Rect;                     // == Renderer::annotation_bounds (strokes, heads, exact text, pill pad)
pub fn hit_test(&Annotation, Point, tolerance: f32) -> bool;       // == Renderer::hit_test
// Text engine (process-wide FontSystem behind OnceLock<Mutex>; layouts cached; thread-safe, call from any thread)
pub fn warm_up_fonts();                                            // load system fonts on a background thread (call at app start)
pub fn font_families() -> Vec<String>;                             // installed families for a picker → FontFamily::Named
pub fn layout_text(&str, &TextStyle, width: Option<f32>) -> Arc<TextLayout>;
pub fn text_layout(&TextAnnotation) -> Arc<TextLayout>;
pub fn text_box(&TextAnnotation) -> Rect;                          // position + laid-out size (the wrap/align box)
pub fn measure_text_annotation(&TextAnnotation) -> Rect;           // everything painted: box ∪ ink ∪ pill padding ∪ outline
pub fn text_hit_index(&TextAnnotation, Point) -> usize;            // UTF-8 byte index into `content` nearest to the point (caret placement)
pub fn caret_rect(&TextAnnotation, byte_index: usize) -> Rect;     // caret (w ≈ size/16, full line height); soft-wrap boundary → next line
pub fn selection_rects(&TextAnnotation, Range<usize>) -> Vec<Rect>;// one rect per visual line
pub fn callout_text(&CalloutAnnotation) -> TextAnnotation;         // the exact text box drawn inside a callout → use with caret/hit fns
pub struct TextLayout { pub size: Size, pub font_size: f32, pub line_height: f32, pub lines: Vec<TextLine>, pub ink_bounds: Option<Rect>, .. }
impl TextLayout { fn hit_index(&self, local: Point) -> usize; fn caret_rect(&self, usize) -> Rect; fn selection_rects(&self, Range<usize>) -> Vec<Rect>; fn visual_bounds(&self, &TextStyle) -> Rect }  // box-local
pub struct TextLine { pub top, pub height, pub baseline, pub x, pub width: f32, pub range: Range<usize>, pub hard_line: usize }
// `Quality` now also derives Hash.
```
Rendering semantics the editor should know:
* Text: glyphs are drawn as vector outlines (Outline background = real stroke of the glyph path, 2×width, round joins, behind the fill);
  color-bitmap fonts (Apple Color Emoji) are rasterized. Auto width = widest line; alignment within the box. Text shadow = `style.shadow`, else `Annotation.shadow`.
* Callout: text wraps at `rect.width − 2·padding` and is **vertically centered** in the padded rect; tail base = min(24 px·scale_factor, edge/3), spliced into the outline (no seam).
* Step: label font auto-fitted to the circle (size ≈ 1.15·r, shrunk for long labels), centered on its ink bounds.
* Magnifier: samples what is rendered below it; when the loupe is moved off its source, a thin ring marks the source (included in bounds/hit-test).
* Obfuscation ignores opacity/shadow and always fully replaces pixels (min blur σ 1 px, min pixelate block 2 px in output px). Highlighter: multiply, opacity folded into color, no shadow.
* Group opacity/shadow render through an offscreen layer (correct for overlapping fill+stroke).
* Caching: base layer (everything below annotations) per (image, transform, beautify, scale, quality); blur results keyed by a signature of
  the base + overlapping lower annotations (no pixel read-back on hit); decoded/downsampled images; text layouts. An annotation drag only
  re-renders the vector layer. Use `RenderOptions::exclude` for the object being edited inline.
* Perf (M-series, `--release`, 3840×2160 retina doc, wallpaper + chrome + inset + shadow, 3 blurs incl. 3840×760, pixelate, redact,
  2 spotlights, magnifier, text, arrow; X2 export → 4088×2464): cold export incl. wallpaper decode + PNG ≈ 200 ms; warm render ≈ 12 ms +
  PNG encode ≈ 19 ms (JPEG q90 ≈ 66 ms, WebP ≈ 22 ms); re-render after moving an annotation ≈ 25 ms; preview @0.5 drag ≈ 6 ms/frame,
  @0.25 ≈ 2 ms/frame; preview @0.5 base rebuild (e.g. padding slider) ≈ 34 ms. `cargo run -p rs-render --example gallery --release -- OUT_DIR`.

---

## 6. rs-capture (capture agent)

Conventions: frames/regions in **global screen points** (CG global, origin top-left of main display, y down);
images in physical px; `Captured.scale_factor` = px/pt. All functions blocking; call from `cx.background_executor()`.

```rust
pub enum CaptureError { PermissionDenied, Cancelled, NotFound, NotImplemented(&'static str), Unsupported(String),
                        Clipboard(String), NoClipboardImage, Platform(String), Image(..), Io(..) }
pub type Result<T, E = CaptureError>;
pub struct DisplayInfo { id: u32 /* CGDirectDisplayID == u32::from(gpui DisplayId) */, name, frame: Rect, scale_factor: f32, is_main: bool }
pub struct WindowInfo { id: u32 /* CGWindowID */, owner_pid: i32, app_name, title, frame: Rect, layer: i32, display_id: Option<u32> }
pub struct WindowCaptureOptions { include_shadow: bool, transparent_background: bool }
pub struct Captured { image: RgbaImage, scale_factor: f32, source: CaptureSource, frame: Option<Rect> }
impl Captured { fn into_document(self, BeautifySettings) -> Document }
fn has_screen_capture_permission() -> bool;          // CGPreflightScreenCaptureAccess (cached per process: relaunch after grant)
fn request_screen_capture_permission() -> bool;      // CGRequestScreenCaptureAccess (prompts once, adds app to the list)
fn open_screen_recording_settings() -> Result<()>;
fn list_displays() -> Result<Vec<DisplayInfo>>;      // CG; main first; per-display scale; names refreshed when called on main thread
fn list_windows() -> Result<Vec<WindowInfo>>;        // CG; Vec order = z-order front-to-back; layer 0, excl. own pid / alpha 0 / <40pt / off-display / Dock, WindowServer…
fn capture_display(id: u32) -> Result<Captured>;     // SCK, own windows excluded
fn capture_all_displays() -> Result<Vec<(DisplayInfo, Captured)>>;  // freeze-frame: all displays concurrently, cached SCK snapshot
fn capture_region(rect: Rect, display_id: Option<u32>) -> Result<Captured>;  // may SPAN displays (None); snapped to px grid; CLI fallback
fn capture_window(window_id: u32, WindowCaptureOptions) -> Result<Captured>; // alpha/shadow → SCK desktopIndependentWindow; else on-screen pixels w/o windows above
fn crop_to_region(&Captured, &DisplayInfo, rect: Rect) -> Result<Captured>;  // single display
fn capture_interactive_system(window: bool) -> Result<Captured>;  // /usr/sbin/screencapture -i fallback; scale from pHYs (default 2.0)
fn desktop_wallpaper_path(display_id: Option<u32>) -> Result<PathBuf>;  // ANY thread (hops to main queue, 2 s timeout); HEIC/.madesktop → cached JPEG
const SUPPORTED_IMPORT_EXTENSIONS: &[&str];          // png jpg jpeg tif tiff webp bmp gif heic heif
fn load_image_file(&Path) -> Result<Captured>;       // EXIF orientation; ImageIO fallback (HEIC); scale from PNG pHYs
fn load_image_bytes(&[u8]) -> Result<Captured>;
fn clipboard_image() -> Result<Captured>;            // NSPasteboard: PNG, TIFF, JPEG/HEIC/WebP/GIF, or copied image file (source File)
fn copy_image_to_clipboard(&RgbaImage) -> Result<()>;// NSPasteboard PNG + TIFF (72 dpi)
fn copy_text_to_clipboard(&str) -> Result<()>;
// ---- NEW (capture agent) ----
const SKIP_PREFLIGHT_ENV: &str = "RS_CAPTURE_SKIP_PREFLIGHT";  // debug: bypass permission gate
const MAX_IMPORT_SIDE: u32 = 32_768;
fn prepare_capture() -> Result<()>;                  // warm SCK + cache snapshot (call once at startup, background thread)
struct RegionPart { display_id, source_rect: Rect /* display-local pt */, source_scale: f32, dest: IRect /* output px */ }
struct RegionPlan { rect: Rect /* snapped global pt */, scale_factor /* max of covered displays */, width, height, parts: Vec<RegionPart> }
fn plan_region(rect, &[DisplayInfo], display_id: Option<u32>) -> Result<RegionPlan>;  // pure math
fn crop_region_from_displays(&[(DisplayInfo, Captured)], rect: Rect) -> Result<Captured>; // crop freeze-frame, spanning + mixed DPI
fn display_at_point(&[DisplayInfo], Point) -> Option<&DisplayInfo>;
fn window_at_point(&[WindowInfo], Point) -> Option<&WindowInfo>;       // front-most under cursor (hover highlight)
fn point_to_pixel(&Captured, &DisplayInfo, Point) -> Option<(u32, u32)>;
fn sample_pixel(&[(DisplayInfo, Captured)], Point) -> Option<Color>;   // loupe colour readout
fn sample_grid(&Captured, &DisplayInfo, Point, radius: u32) -> RgbaImage; // (2r+1)² loupe pixels, outside = transparent
fn encode_png(&RgbaImage, scale_factor: f32) -> Result<Vec<u8>>;       // fast PNG + pHYs (scale×72 dpi)
fn png_dpi(&[u8]) -> Option<f64>;  fn dpi_to_scale(f64) -> f32;
fn copy_image_to_clipboard_with_scale(&RgbaImage, scale_factor: f32) -> Result<()>; // Retina pastes at point size
fn copy_png_to_clipboard(&[u8]) -> Result<()>;       // already-encoded PNG (+ TIFF derived)
fn copy_file_to_clipboard(&Path) -> Result<()>;      // file URL on pasteboard
struct ClipboardSnapshot { items };  fn take() -> Self; fn restore(&self) -> Result<()>;  // NEW (app-polish): save/restore whole clipboard
```
**Permission gate:** every capture fn returns `Err(PermissionDenied)` immediately when the preflight is false (macOS would
otherwise return wallpaper-only frames). All ScreenCaptureKit completions are bridged with 6 s timeouts → `Err(Platform)`,
never a hang. SCK errors `-3801` map to `PermissionDenied`.
**Threading:** everything is blocking and callable from any thread; run captures on `cx.background_executor()`.
`desktop_wallpaper_path` off-main needs the main run loop alive (GPUI: yes) — never call it while blocking the main thread
on its result. Images are converted to **sRGB** straight-alpha RGBA. Retina frames are big (6720×3780 = 100 MB RGBA per 4K
display at "looks like 3360×1890"); drop freeze-frames once the overlay closes.
Screen Recording: the dev terminal has **no** Screen Recording permission, so live captures can only be tested from a
bundled/signed app (or a granted terminal): `cargo run -p rs-capture --example probe` prints everything and writes PNGs.

---

## 7. rs-smart (smart agent) — all implemented

Data dir: `$RAPIDSNAP_DATA_DIR` or `~/Library/Application Support/io.github.ahmedash95.rapidsnap/` (`paths::{APP_ID, LEGACY_APP_ID, DATA_DIR_ENV, LOCK_FILE_NAME, data_dir, preferences_path, backgrounds_dir, history_dir, migrate_legacy_data_dir, migrate_legacy_data_dir_from, Migration}`; the app calls `migrate_legacy_data_dir()` first thing in `main`, which moves the pre-release `com.rapidsnap.v2/` contents over once). Tests should set `RAPIDSNAP_DATA_DIR` or use explicit paths.

**Threading:** every rs-smart function is blocking. OCR / detection (≈0.2–1 s), history add (PNG encode), project save/load,
background add → `cx.background_executor().spawn(..)`. Prefs load/save and login-item calls are fast (fine on main thread; debounce saves).
All types are `Send`; `HistoryStore` / `BackgroundLibrary` are cheap `Clone` handles and serialize their index writes with an
in-process mutex, so concurrent background tasks are safe. All file writes are atomic (`fsutil::atomic_write`: unique temp + fsync + rename).

```rust
pub enum SmartError { Io, Json, Zip, Image, InvalidProject(String), UnsupportedVersion(u32), NotFound, NotImplemented(&'static str), Platform(String) }
// prefs — tolerant: missing/unknown fields OK (serde default); corrupt → defaults + preferences.json.bak
prefs::load_preferences() -> Preferences              // never fails
prefs::load_preferences_from(&Path) -> Preferences
prefs::save_preferences(&Preferences) -> Result<()>   // atomic
prefs::save_preferences_to(&Preferences, &Path) -> Result<()>
// project .rsnap — zip: manifest.json {format:"rsnap",version:1,app_version}, document.json, base.png, [thumbnail.png]
//   forward compat: unknown fields/entries ignored, unknown annotation kinds skipped (logged), newer version → UnsupportedVersion
project::{PROJECT_VERSION=1, PROJECT_EXTENSION="rsnap"}
project::project_to_bytes(&Document, thumbnail: Option<&RgbaImage>) -> Result<Vec<u8>>
project::project_from_bytes(&[u8]) -> Result<Document>
project::save_project(&Document, &Path, thumbnail: Option<&RgbaImage>) -> Result<()>  // atomic; doesn't mutate doc
project::load_project(&Path) -> Result<Document>      // sets metadata.project_path
project::load_project_thumbnail(&Path) -> Result<RgbaImage>   // Err(NotFound) if none embedded
project::is_project_path(&Path) -> bool               // NEW
// custom backgrounds (copied into <data>/backgrounds/<uuid>.<ext>, index.json array)
backgrounds::SUPPORTED_EXTENSIONS                      // NEW
backgrounds::BackgroundItem { id: Uuid, name, path }
backgrounds::BackgroundLibrary::{open_default(), open(dir), dir(), list(), add_from_file(&Path), remove(Uuid),
    get(Uuid), contains_path(&Path), add_from_bytes(&[u8], name), rename(Uuid, &str)}   // last four NEW
// history (<data>/history: index.json {"version":1,"entries":[..]} newest first, <uuid>.rsnap, <uuid>.thumb.png ≤512px)
//   missing/corrupt index is rebuilt from the .rsnap files
history::{THUMBNAIL_MAX_SIDE=512, DEFAULT_HISTORY_LIMIT=50}   // NEW
history::HistoryEntry { id, created_at_ms, project_path, thumbnail_path, width, height, source }
history::HistoryStore::{open_default(), open(dir), dir(),
    list() -> newest first,
    add(&Document, thumbnail: &RgbaImage /* any size, downscaled; empty → uses base image */, limit /* prefs.history_limit */) -> HistoryEntry,
    load(Uuid) -> Document /* project_path cleared */, remove(Uuid) /* NotFound */, clear(),
    get(Uuid), load_thumbnail(Uuid), prune(limit)}      // last three NEW
history::make_thumbnail(&RgbaImage, max_side) -> RgbaImage    // NEW
// OCR (macOS Vision, VNRecognizeTextRequest) — input = rs_render::content_image(doc); rects in content px (top-left, y down)
ocr::OcrOptions { languages, accurate, language_correction }  // default: accurate, NO language correction
ocr::OcrOptions::{for_copy_text() /* + correction */, for_redaction()}   // NEW
ocr::OcrWord { text, range (byte range in line), bounds }     // per whitespace token via boundingBoxForRange
ocr::OcrLine { text, bounds, confidence, words }  + OcrLine::range_bounds(byte range) -> Option<Rect>  // NEW: tight box for a substring
ocr::recognize_text(&RgbaImage, &OcrOptions) -> Result<Vec<OcrLine>>   // reading order (rows top→bottom, left→right)
ocr::recognize_plain_text(&RgbaImage, &OcrOptions) -> Result<String>
ocr::copy_text_from_image(&RgbaImage) -> Result<String>   // NEW: "Copy text" menu item; same-row lines joined by 2 spaces
ocr::join_rows(&[OcrLine]) -> String                       // NEW
// auto-redact
redact::SensitiveKind { Email, ApiKey, CreditCard, IpAddress, Phone } + ALL + label()   // label NEW
redact::Detection { id, kind, text, bounds (padded), selected (true) } + masked_text()  // masked_text NEW — show this in the review list
redact::RedactStyle { Solid(Color), Blur(f32), Pixelate(u32) }
redact::detect_sensitive(&[OcrLine], &[SensitiveKind]) -> Vec<Detection>   // pure
redact::detect_in_image(&RgbaImage, &[SensitiveKind]) -> Result<Vec<Detection>>   // NEW: OCR + detect (blocking)
redact::find_in_text(&str, &[SensitiveKind]) -> Vec<TextMatch{kind, range}>        // NEW: pure text matcher
redact::luhn_valid(&str) -> bool                                                  // NEW
redact::detections_to_annotations(&[Detection], RedactStyle) -> Vec<Annotation>   // selected only; apply via Command::Add
// login item (SMAppService.mainAppService via objc2 msg_send; ServiceManagement linked)
login_item::set_launch_at_login(bool) -> Result<()>   // idempotent; Err(Platform) when not running from a .app bundle
login_item::is_launch_at_login_enabled() -> bool
login_item::LoginItemStatus { NotRegistered, Enabled, RequiresApproval, NotFound, Unavailable }   // NEW
login_item::{login_item_status(), open_login_items_settings(), is_running_from_app_bundle()}      // NEW
// output (NEW module) — save location + filename template with local clock
output::default_save_folder(&ExportSettings) -> PathBuf        // default_folder if it exists, else ~/Desktop
output::filename_context_now(counter, w, h, app_name: Option<String>) -> FilenameContext
output::app_name_for_source(&CaptureSource) -> Option<String>
output::suggested_file_name(&ExportSettings, &FilenameContext) -> String   // template + format extension
output::unique_path(dir, file_name) -> PathBuf                 // "name 2.png", "name 3.png"…
output::suggested_save_path(&ExportSettings, &FilenameContext) -> PathBuf
// fsutil (NEW): atomic_write(&Path, &[u8]), remove_file_if_exists(&Path)
```

Auto-redact detection rules (unit-tested incl. false-positive guards): emails (asset names like `icon@2x.png` rejected);
API keys: AWS `AKIA/ASIA…`, GitHub `ghp_/gho_/ghs_/github_pat_`, GitLab `glpat-`, Slack `xox?-` + webhooks, Stripe `sk_/pk_/rk_live|test_`,
`whsec_`, OpenAI/Anthropic `sk-(proj-|ant-…)`, Google `AIza…`, Twilio, SendGrid, npm, PyPI, DigitalOcean, Shopify, JWTs, PEM private-key
headers, `password=/api_key:/Bearer …` values, generic high-entropy tokens (≥20 chars mixed case+digits, entropy ≥3.6; hex ≥32) — UUIDs,
identifiers, paths, numbers rejected; cards: 13–19 digits with spaces/dashes, Luhn + IIN prefix; IPv4 (+CIDR, not versions `1.2.3.4.5`),
IPv6 (std parser; not times/MACs/`std::fs`); phones: `+` international, `(area)`, or ≥9 digits in ≥3 groups (not dates/versions/amounts).
Overlaps resolved by priority card > key > email > IP > phone. Boxes are tight to the matched substring (partial tokens trimmed by
character share). Live-verified with Vision on a rendered test image (`cargo run -p rs-smart --example ocr_live -- <out_dir>`).

**Suggested app flow:** after capture → `HistoryStore::add(&doc, &rendered_or_empty, prefs.history_limit)` on the background
executor; "Copy Text" → `ocr::copy_text_from_image(&rs_render::content_image(&doc)?)` then `rs_capture::copy_text_to_clipboard`;
Auto-redact → `redact::detect_in_image` → review list → `detections_to_annotations` → `Command::Add`. Because rs-editor must not
depend on rs-smart, the app should inject these as callbacks (e.g. via `EditorOptions` fields).

## 8. rs-editor & rapidsnap (GPUI) contracts

### rs-editor public API (frozen)
```rust
pub const KEY_CONTEXT: &str = "RsEditor";
gpui_kit::actions!(rs_editor, [Copy, Save, Undo, Redo, CloseEditor]);   // add more freely
pub struct PreferencesGlobal(pub Preferences);  impl Global              // app sets at startup
pub enum EditorEvent { Copied{document}, Saved{document, path}, Closed{document} }
pub type EditorEventHandler = Rc<dyn Fn(&EditorEvent, &mut App)>;
pub struct EditorOptions { pub on_event: Option<EditorEventHandler> }    // Clone + Default; add fields freely
pub fn init(cx: &mut App);                                               // keybindings; after gpui_kit::init
pub fn open_editor(cx: &mut App, doc: Document, options: EditorOptions) -> anyhow::Result<WindowHandle<Root>>;
pub fn rgba_to_render_image(&RgbaImage) -> Arc<RenderImage>;
pub struct EditorView { .. }  // fn new(doc, options, window, cx); fn document(&self) -> &Document
```
* Preferences flow: editor reads/writes `cx.global::<PreferencesGlobal>()` / `cx.global_mut` (tool style memory, recent colors,
  `last_beautify`, export counter). The app observes the global and persists (debounce is the app's job).
* The editor copies through `EditorServices.copy_image` (app: rs-capture NSPasteboard PNG + TIFF, pHYs = export px/pt) and only
  falls back to GPUI's PNG-only clipboard when no service is injected; saves via `cx.prompt_for_new_path` (format from the chosen
  extension; missing/unknown extension → preferred format's extension appended). `auto_close_on_copy` → `window.remove_window()`.
* Foundation editor = preview + padding slider + beautify switch + Copy/Save/Undo/Redo/Close. Replace freely behind the same API.

### rs-editor additions (editor agent, 2026-09-25) — all additive, frozen API unchanged
```rust
pub struct EditorOptions { pub on_event: Option<EditorEventHandler>, pub services: EditorServices }   // NEW field
pub struct EditorServices {                       // rs_editor::services; Clone + Default (= none; UI hides the feature)
    pub recognize_text:    Option<Arc<dyn Fn(&RgbaImage) -> Result<Vec<OcrTextLine>, String> + Send + Sync>>,  // bg thread
    pub detect_sensitive:  Option<Arc<dyn Fn(&RgbaImage) -> Result<Vec<SensitiveMatch>, String> + Send + Sync>>, // bg thread
    pub save_project:      Option<Arc<dyn Fn(&Document, &Path, Option<&RgbaImage>) -> Result<(), String> + Send + Sync>>,
    pub load_project:      Option<Arc<dyn Fn(&Path) -> Result<Document, String> + Send + Sync>>,
    pub add_to_history:    Option<Arc<dyn Fn(&Document, &RgbaImage) -> Result<(), String> + Send + Sync>>, // called on copy & save
    pub copy_image:        Option<Arc<dyn Fn(&RgbaImage, &[u8] /*png*/, f32 /*px per pt*/) -> Result<(), String> + Send + Sync>>, // bg thread (integration)
    pub list_backgrounds:  Option<Rc<dyn Fn() -> Vec<CustomBackground>>>,           // main thread
    pub add_background:    Option<Rc<dyn Fn(&Path) -> Result<CustomBackground, String>>>,
    pub remove_background: Option<Rc<dyn Fn(&str /*id*/) -> Result<(), String>>>,
    pub desktop_wallpaper: Option<PathBuf>,                                         // for Background::DesktopWallpaper
}
pub struct OcrTextLine { text, bounds: Rect }  pub struct SensitiveMatch { kind: String, text, bounds: Rect }  // content px
pub struct CustomBackground { id: String, name, path }
pub const CANVAS_CONTEXT = "RsEditorCanvas"; pub const TEXT_CONTEXT = "RsEditorText";  // key contexts (see init())
// more actions: SaveProject ⇧⌘S, CopyText ⇧⌘T, AutoRedact ⇧⌘R, ZoomIn/Out/Fit/Actual ⌘=/⌘-/⌘0/⌘1, Tool* (letters),
// Cancel, Confirm, DeleteSelection, Duplicate ⌘D, SelectAll, Nudge*, BringForward ⌘], SendBackward ⌘[, BringToFront ⌥⌘],
// SendToBack ⌥⌘[, CanvasCopy/Cut/Paste (annotations via AnnotationClipboard; ⌘C with nothing selected copies the image), Text*.
pub mod model;   // EditorModel: pure state machine (doc/history/selection/tools/text sessions/crop) — usable headless
pub mod geom; pub mod hit; pub mod snap; pub mod style_map; pub mod text_buffer; pub mod render_worker; pub mod dragout;
```
* Prefs written by the editor: `tool_styles` (per-tool memory), `style_presets`, `recent_colors`, `last_beautify` (every
  beautify change), `last_tool` (initial tool of new editors), `export.{format, scale}` (Beautify tab → Export), `export.counter`
  (+1 per save).
* Text editing UX: Enter = newline, ⌘Enter / Esc / click outside = commit; empty text removed; hint in the status bar.
* Rendering: one render thread per editor window (latest-wins previews, FIFO exports) — the editor never renders on the main
  thread. Live overlays (selection, handles, guides, marquee, crop, caret/IME text) are GPUI quads/paths.
* Drag-out: native `NSDraggingSession` (file URL of a temp PNG in `$TMPDIR/rapidsnap-drag/`); click on the handle = Reveal in Finder.
* Debug: `RAPIDSNAP_EDITOR_SELFTEST=1` (+ `_DIR=<dir>`, `_SHOT=1` window PNGs, `_KEEP_OPEN=1`) drives real GPUI events, then quits.
  Demo: `cargo run -p rs-editor --example editor_demo`.

### rapidsnap (app) status (app agent, 2026-09-25)
Modules: `main.rs` (startup, single-instance flock `<data dir>/rapidsnap.lock`, `on_open_urls`/argv → editor, app menu + ⌘O/⌘⇧V/⌘,/⌘Q,
prefs observer: debounced save 400 ms + hotkey re-register + tray sync), `tray.rs` (`AppEvent`, `TrayState`: menu incl. Delayed Capture ▸,
Recent Captures ▸, Instant Copy check item, accelerators = hotkeys; per-mode registration `errors`; `suspend/resume` for the recorder),
`capture.rs` (flow: delay → overlay / fullscreen / repeat-last → crop from freeze-frame → `deliver` = history + editor or instant copy;
every `CaptureError` → HUD notice, `PermissionDenied`/preflight false → welcome window; freeze-frame failure → `screencapture -i` fallback),
`overlay.rs` (`OverlaySession` entity shared by one `OverlayView` per display; input API `pointer_move/down/up`, `key_down/up`, `cancel`,
`toggle_mode` — used by mouse, keys and the self-test), `hud.rs` (click-through notice + countdown, `TransientWindows`),
`welcome.rs`, `prefs_window.rs`, `services.rs` (`EditorServices` from rs-smart/rs-capture + shared `HistoryIndex` so an editor copy
replaces the capture's history entry), `keys.rs` (GPUI keystroke → W3C code), `platform.rs`, `selftest.rs`.
* Overlay windows open **before** the freeze-frame arrives and stay alpha-1/255 until then (SCK excludes own windows anyway), so
  hotkey→overlay ≈ 15–45 ms (debug). Keys reach the overlay because it keeps `NonactivatingPanel` (key window without activating the app).
* `RAPIDSNAP_SELFTEST=1` (dev or `open --env RAPIDSNAP_SELFTEST=1 dist/RapidSnap.app`) runs 23 checks on a two-display setup (22 with one display; +1 "Esc posted key event" when the process may post events) and exits 0/2.

---

## 9. GPUI recipes verified at gpui-pre 0.3.6 + gpui-kit v0.6.6 (all compiled & ran here)

**App skeleton / accessory app**
```rust
use gpui_kit::*;                       // GPUI API
gpui_kit::application().run(|cx: &mut App| {
    gpui_kit::init(cx);                // REQUIRED before any gpui-component widget (theme, Root, inputs…)
    rs_editor::init(cx);
    platform::set_activation_policy(ActivationPolicy::Accessory);  // GPUI forces Regular in didFinishLaunching → override here
    // objc2: NSApplication::sharedApplication(MainThreadMarker::new()?).setActivationPolicy(NSApplicationActivationPolicy::Accessory)
});
```
`LSUIElement=true` in Info.plist alone is **not** enough (GPUI resets to Regular). Switch to Regular while editor/prefs windows are
open (Dock icon + menu + ⌘-Tab), back to Accessory in `cx.on_window_closed(|cx, _| if cx.windows().is_empty() {..}).detach()`.

**Opening a window with gpui-component**
```rust
let handle = cx.open_window(WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1200.), px(800.)), cx))),
    titlebar: Some(TitlebarOptions { title: Some("RapidSnap".into()), appears_transparent: true,
                                     traffic_light_position: Some(point(px(12.), px(12.))) }),
    focus: true, show: true, kind: WindowKind::Normal,
    window_min_size: Some(size(px(640.), px(400.))), ..Default::default()
}, |window, cx| {
    let view = cx.new(|cx| MyView::new(window, cx));
    cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))  // Root required for dialogs/menus/notifications/popovers
})?;
cx.activate(true);                                   // bring app to front (needed for accessory apps & file panels)
handle.update(cx, |_, window, _| window.activate_window())?;   // make key + front
```
With `appears_transparent`, reserve ~28px top padding for the traffic lights.

**Borderless transparent always-on-top overlay (verified: CG reports layer 1000, bounds = full display incl. menu bar)**
```rust
WindowOptions { window_bounds: Some(WindowBounds::Windowed(display.bounds())), titlebar: None, kind: WindowKind::PopUp,
                window_background: WindowBackgroundAppearance::Transparent, is_movable: false, is_resizable: false,
                is_minimizable: false, display_id: Some(display.id()), focus: true, show: true, ..Default::default() }
// then in the build closure: platform::make_overlay_window(window)  (raw-window-handle → NSView.window →
//   Borderless style, !opaque, no shadow, clear bg, level 1000, CanJoinAllSpaces|FullScreenAuxiliary|Stationary|IgnoresCycle,
//   setFrame(screen.frame) — DEFERRED via dispatch2::DispatchQueue::main().exec_async)
```
GPUI alone gives: `titlebar: None` → Titled|FullSizeContentView (not truly borderless, constrained below the menu bar);
`PopUp` → NSPanel, NonactivatingPanel, level 101, all Spaces. Hence the native tweak. One overlay per display (`cx.displays()`).
Body: `div().size_full().bg(rgba(0x00000066))` — alpha works with `Transparent`.

**⚠ Never call AppKit methods that fire delegate callbacks (setStyleMask, setFrame, makeKeyAndOrderFront, …) synchronously
from inside a GPUI callback** — GPUI's App `RefCell` is borrowed and you get `ERROR RefCell already borrowed` (lost events).
Defer them with `dispatch2::DispatchQueue::main().exec_async(..)` (wrap the `Retained<NSWindow>` in a `Send` newtype and
bind the whole wrapper inside the closure — edition 2024 disjoint capture otherwise captures the non-Send field).

**Showing an RGBA buffer (preview)**
```rust
// GPUI RenderImage = BGRA, straight alpha, frames of image::Frame.
let mut bgra = rgba.pixels.as_ref().clone(); for p in bgra.chunks_exact_mut(4) { p.swap(0, 2); }
let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, bgra).unwrap());
let render_image: Arc<RenderImage> = Arc::new(RenderImage::new(vec![frame]));   // = rs_editor::rgba_to_render_image
div().child(img(render_image.clone()).w(px(w_logical)).h(px(h_logical)))      // ImageSource: From<Arc<RenderImage>>
// When replacing: window.drop_image(old_arc) to free the sprite-atlas texture.
```
For a custom canvas element use `canvas(prepaint, paint)` and `window.paint_image(bounds, corners, render_image, frame_ix, grayscale)`.

**Actions & keybindings**
```rust
gpui_kit::actions!(rs_editor, [Copy, Save]);        // gpui_kit's macro (GPUI's own needs crate name `gpui`)
cx.bind_keys([KeyBinding::new("cmd-c", Copy, Some("RsEditor")), KeyBinding::new("cmd-shift-z", Redo, Some("RsEditor"))]);
div().key_context("RsEditor").track_focus(&self.focus_handle)
     .on_action(cx.listener(|this, _: &Copy, window, cx| ...))
// focus: self.focus_handle.focus(window, cx); impl Focusable for the view.
// single-letter tool keys: KeyBinding::new("r", SelectRect, Some("RsEditorCanvas")) on a canvas context that is NOT active while a text input is focused.
```

**Async / background work**
```rust
let task = cx.background_executor().spawn(async move { heavy_blocking() });      // any thread
cx.spawn(async move |cx: &mut AsyncApp| { let r = task.await; cx.update(|cx| use_result(r, cx)); }).detach();
// inside a view (Context<T>):
cx.spawn(async move |this: WeakEntity<T>, cx: &mut AsyncApp| { let _ = this.update(cx, |this, cx| { ...; cx.notify(); }); }).detach();
cx.background_executor().timer(Duration::from_millis(16)).await;   // timers
```
`AsyncApp::update` returns `R` directly at this rev; `WeakEntity::update` / `WindowHandle::update` return `Result`.

**File dialogs & clipboard**: `cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Open".into()) })`
and `cx.prompt_for_new_path(&dir, Some("name.png"))` → oneshot receivers (`rx.await` → `Result<Result<Option<..>>>`). Call `cx.activate(true)` first
in an accessory app. Clipboard: `cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, bytes)))`, `cx.read_from_clipboard()`.

**Globals**: `impl Global for T {}`, `cx.set_global(t)`, `cx.global::<T>()`, `cx.global_mut::<T>()`, `cx.try_global::<T>()`,
`cx.update_global::<T, _>(|g, cx| ..)`, `cx.observe_global::<T>(|cx| ..).detach()`.

**Tray icon + global hotkeys with GPUI's run loop (verified)**
* GPUI owns the AppKit run loop; tray-icon / muda / global-hotkey post their events from it on the main thread — no extra loop.
* Create `TrayIconBuilder…build()` and `GlobalHotKeyManager::new()` **inside `run`** (after launch) and keep them alive in a Global.
* `MenuEvent::set_event_handler` / `GlobalHotKeyEvent::set_event_handler` (can be set **once**, `Send+Sync`) → push to a
  `smol::channel::unbounded` sender → `cx.spawn(async move |cx| while let Ok(e) = rx.recv().await { cx.update(|cx| handle(e, cx)); })`.
  Zero polling, no added latency.
* Verified: 3 hotkeys registered OK (⇧⌘2, ⌥⇧⌘2, ⇧⌘1); a synthetic ⇧⌘1 posted with `CGEventPost` (self-test,
  `platform::post_key_combo`) arrived as `event: CaptureFullscreen` on the GPUI main thread → **hotkey path verified end-to-end**.
  (Posting synthetic events needs Accessibility for the responsible process; `CGPreflightPostEventAccess()` is logged.)
  Status-bar item created — on macOS 26 status items are hosted by **Control Centre** (layer‑25 windows owned by
  Control Centre's pid, not ours); layer‑25 count went 34 → 36 (one item per display menu bar). Tray *menu clicks* use the
  identical channel mechanism but were not clicked automatically (no UI scripting permission); manual check: click the
  tray icon → menu → "Open File…".
* Tray icon: 44×44 black+alpha PNG, `.with_icon_as_template(true)`; tray-icon sizes it to 18pt height.

**gpui-component widgets (import paths at v0.6.6; all compiled in rs-editor unless noted)**
```rust
use gpui_kit::component::{
    Root, ActiveTheme as _ /* cx.theme().background/.border/.foreground… */, Sizable as _, Size, StyledExt as _,
    h_flex, v_flex, Icon, IconName, IndexPath, WindowExt as _,
    button::{Button, ButtonVariants as _, ButtonGroup, Toggle},        // Button::new("id").primary().label("..").icon(IconName::X).tooltip("..").on_click(|ev,window,cx| ..)
    slider::{Slider, SliderState, SliderEvent},                         // cx.new(|_| SliderState::new().min(0.).max(1.).step(0.01).default_value(0.5)); Slider::new(&state)
                                                                        // events: SliderEvent::Change(v)/Release(v), v.start()/v.end(); state.set_value(v, window, cx)
    switch::Switch,                                                     // Switch::new("id").label("..").checked(b).on_click(|checked: &bool, window, cx| ..)
    color_picker::{ColorPicker, ColorPickerState, ColorPickerEvent},    // cx.new(|cx| ColorPickerState::new(window, cx).default_value(hsla)); ColorPickerEvent::Change(Option<Hsla>)
    input::{Input, InputState, InputEvent, Textarea, TextareaState, NumberInput}, // cx.new(|cx| InputState::new(window, cx).placeholder("..").default_value("..")); Input::new(&state)
    select::{Select, SelectState},                                      // cx.new(|cx| SelectState::new(vec![..], Some(IndexPath::default()), window, cx)); Select::new(&state)
    tab::{Tab, TabBar},                                                 // TabBar::new("id").selected_index(ix).on_click(cx.listener(|this, ix: &usize, window, cx| ..)).child(Tab::new().label(".."))
    checkbox::Checkbox, radio::Radio, popover::Popover, tooltip::Tooltip, menu::{PopupMenu, ContextMenuExt},
    dialog, notification::Notification, separator::Separator, label::Label, kbd::Kbd, scroll, form::{field, v_form},
};
use gpui_kit::prelude::FluentBuilder as _;   // .when(), .when_some()
```
All of the above except `checkbox/radio/popover/tooltip/menu/dialog/notification/kbd/scroll/form` are exercised in
**`crates/rs-editor/src/gallery.rs`** (compile-checked; `rs_editor::gallery::open_gallery(cx)` opens it; the self-test opens it).
Extra verified details: `Button::new(id).ghost().small()`, `ButtonGroup::new(id).child(..)`, `Toggle::new(id).label(..).checked(b)`,
`Checkbox::new(id).label(..).checked(b)`, `Radio::new(id).label(..).checked(b)`, `NumberInput::new(&input_state)`,
`Separator::horizontal()`, `Label::new(..)`, `IconName::Check`, `SelectState<Vec<String>>`, `ColorPickerEvent::Change(Option<Hsla>)`
(single variant — use `let ColorPickerEvent::Change(c) = ev;`), `InputEvent::Change` + `state.read(cx).value()`.
Dialogs/sheets/toasts via `use gpui_kit::component::WindowExt as _;` (needs `Root`): `window.open_dialog(cx, |dialog: Dialog, window, cx| dialog.title(..).child(..))`,
`window.open_sheet(cx, |sheet: Sheet, window, cx| sheet..)`, `window.push_notification("Copied", cx)` (`impl Into<Notification>`).
Subscribe to widget entities with `cx.subscribe_in(&entity, window, |this, _entity, ev: &Event, window, cx| ..)` and keep the
`Subscription`s in the view. Only Button/Slider/Switch/Root/h_flex/v_flex/ActiveTheme are exercised by foundation code; the
rest are copied from gpui-component's story crate at this rev (`~/.cargo/git/checkouts/gpui-component-*/9765ae2/crates/story/src/stories/*`) — look there for usage.

**Dev-machine quirks**: Amethyst (tiling WM) is running and resizes normal windows to fill the screen (editor opened at
1300×851, then tiled to 3360×1800). Not a bug. The overlay (level 1000, borderless) is unaffected.

---

## 10. Build, run, bundle

* Build/test one crate: `cargo build -p rs-render`, `cargo test -p rs-render` (with your own `CARGO_TARGET_DIR`, see §0).
* Full check (foundation / orchestrator only): `cargo build --workspace && cargo test --workspace`.
* Run app: `cargo run -p rapidsnap` (or `target/debug/rapidsnap`); `RAPIDSNAP_SELFTEST=1` runs the self-test;
  `RAPIDSNAP_DATA_DIR=/tmp/x` isolates prefs; `RUST_LOG=debug`.
* Bundle: `scripts/bundle.sh` → `dist/RapidSnap.app` (release build, Info.plist: `io.github.ahmedash95.rapidsnap` = v1's
  bundle id, executable `Contents/MacOS/RapidSnap`, version from `crates/rapidsnap/Cargo.toml` (2.0.0 > v1 1.5.1), `LSUIElement`,
  `LSMinimumSystemVersion 14.0`, document types png/jpeg/tiff/webp + `io.github.ahmedash95.rapidsnap.project` (.rsnap) UTI, AppIcon.icns,
  `Contents/Resources/assets/`, ad-hoc `codesign --force --deep -s - -i io.github.ahmedash95.rapidsnap`). `SKIP_BUILD=1` reuses the binary.
* Assets at runtime: always via `rs_core::assets::*` (bundle → `Contents/Resources/assets`, dev → repo `assets/`).
* Measured (integration, 2026-09-25): release binary 14.9 MB stripped, **bundle 19 MB** (was 38 MB; 19 MB as of 2026-09-26) — see Changelog.
  Release profile: `opt-level="s"` + `-O3` for the pixel pipeline crates, fat LTO, 1 CGU, `panic="abort"`, `strip -x` in bundle.sh.
* Measured (foundation, 2026-09-25): release binary 25 MB, bundle 32 MB (target < 20 MB → M6: `opt-level="z"`, trim
  gpui-component features/assets); idle RSS 57 MB release / 58 MB debug (target < 50 MB); cold `cargo build --workspace`
  ≈ 2.5 min; incremental crate rebuild seconds. Wallpapers are 6000×4000 JPEGs — decoding one costs ~100 MB transiently.

---

## 11. Changelog (append-only; every addition to rs-core or cross-crate contract goes here)

| Date | Agent | Change |
|---|---|---|
| 2026-09-25 | foundation | Initial contracts, workspace, stubs, spikes. |
| 2026-09-25 | smart | rs-smart fully implemented (OCR via Vision, auto-redact detection, history store, robust project/prefs/backgrounds, SMAppService login item). No rs-core changes. Additive API only (marked NEW in §7): `ocr::{OcrOptions::for_copy_text/for_redaction, OcrLine::range_bounds, copy_text_from_image, join_rows}`, `redact::{SensitiveKind::label, Detection::masked_text, TextMatch, find_in_text, detect_in_image, luhn_valid}`, `history::{THUMBNAIL_MAX_SIDE, DEFAULT_HISTORY_LIMIT, make_thumbnail, HistoryStore::{get, load_thumbnail, prune}}`, `backgrounds::{SUPPORTED_EXTENSIONS, get, contains_path, add_from_bytes, rename}`, `project::is_project_path`, `login_item::{LoginItemStatus, login_item_status, open_login_items_settings, is_running_from_app_bundle}`, new modules `output` and `fsutil`. Deps added to rs-smart: `chrono` (workspace); dev: `cosmic-text` (workspace). Links ServiceManagement.framework. |
| 2026-09-25 | capture | rs-capture fully implemented (CG enumeration, ScreenCaptureKit SCScreenshotManager captures incl. spanning/mixed-DPI regions and single-window alpha+shadow, NSPasteboard PNG+TIFF clipboard, ImageIO/HEIC import, EXIF orientation, PNG pHYs scale, desktop wallpaper incl. HEIC/.madesktop). No signature changes; additions marked NEW in §6. No rs-core changes. Dep added to rs-capture: `objc2-image-io = "0.3"` (already in Cargo.lock); links Accelerate.framework (vImage). |
| 2026-09-25 | render | rs-render fully implemented (all annotation kinds, all beautify options, cosmic-text engine with true glyph outlines, destructive cached blur/pixelate/redact, base-layer caching). No signature changes; no rs-core changes. Additive API (see §5 NEW): `Renderer::{preload, hit_test}`, free `annotation_bounds`, `hit_test`, text engine `warm_up_fonts, font_families, layout_text, text_layout, text_box, measure_text_annotation, text_hit_index, caret_rect, selection_rects, callout_text, TextLayout, TextLine`; `Quality: Hash`. Deps added to rs-render: `serde_json` (workspace), `fontdb = { version = "0.23", default-features = false }` (same crate cosmic-text uses). PNG export now uses fast deflate. Example `gallery` (renders every feature + 4K bench). |
| 2026-09-25 | editor | rs-editor completed (toolbar, canvas with handles/snapping/marquee/crop, inline IME text editor, style & beautify panels, presets, eyedropper, auto-redact review, OCR copy, .rsnap save/open, drag-out, background render thread, self-test). Additive API only (§8 "rs-editor additions"): `EditorOptions.services: EditorServices` + service types, more actions/key contexts, public pure modules. No rs-core changes. Deps added to rs-editor: `smol`, `uuid`, `raw-window-handle`, `objc2`, `objc2-foundation`, `objc2-app-kit`, `objc2-core-graphics`, `objc2-core-foundation`, `dispatch2` (workspace), `unicode-segmentation = "1"` (already in Cargo.lock); dev: `env_logger`. No Requests to rs-render: live text editing is drawn with GPUI's text system, committed text is rendered by rs-render. |
| 2026-09-25 | app | rapidsnap app shell implemented (see §8 status). No rs-core changes. Deps added to rapidsnap: `uuid`, `chrono` (workspace), `libc = "0.2"` (flock; already in Cargo.lock). Consumes rs-editor `EditorOptions.services`. `scripts/bundle.sh`: strips .DS_Store, asserts required assets, `codesign -i com.rapidsnap.v2`. |
| 2026-09-25 | integration | Whole workspace built together for the first time (`target/`): build/test/clippy clean (only external `block v0.1.6` future-incompat note). **Root Cargo.toml now owned by integration** — profiles: dev `rs-render`/`rs-capture` opt-level 2 (+ deps 2); release `opt-level="s"` with the pixel pipeline (rs-render, rs-capture, tiny-skia(-path), image, imageproc, png, fdeflate, miniz_oxide, zune-jpeg, image-webp, simd-adler32, crc32fast, zeno, swash) at 3, fat LTO, 1 CGU, `panic="abort"` (nothing uses catch_unwind; GPUI tasks/AppKit callbacks already abort through `extern "C"` trampolines), `strip="none"` because this toolchain's `rust-objcopy` is broken (missing libLLVM.dylib) → `scripts/bundle.sh` runs `strip -x` and bundles only runtime assets (wallpapers, tray templates, app-icon-256; .icns only in Contents/Resources). **Bundle 38 MB → 18 MB** (binary 31 → 14.9 MB); 4K X2 export cold ≈ 200 ms (unchanged vs -O3). Wiring fixes: editor Copy now goes through new `EditorServices.copy_image` (app → `rs_capture::copy_image_to_clipboard_with_scale`, NSPasteboard PNG+TIFF, pHYs = export px/pt) instead of GPUI's PNG-only clipboard; Save appends the format extension when the chosen name has none/unknown one (bytes always match the extension) and writes off the main thread; `rs_render::warm_up_fonts()` + `rs_capture::prepare_capture()` at startup on a background thread; welcome window offers **Quit & Reopen** after granting (relaunch waits for our pid to exit and forwards `RAPIDSNAP_DATA_DIR`/`RUST_LOG` through `open --env`); repeat-last region clipped to current displays. rs-capture: `arboard` only on non-macOS; `copy_text_to_clipboard` uses NSPasteboard on macOS. Hardening (panic audit): rs-render caps every gaussian sigma at `effects::MAX_SIGMA` (1024 px; NaN → 0), saturating `PxRect` math, blur extent check, magnifier shadow mask limited to the canvas (regression test `absurd_effect_sizes_are_clamped`); rs-editor `nearest_point_on_rect` normalizes/validates rects, text rows sorted for RTL wrap boundaries; rs-core `next_step_value` + export counter saturating. App self-test +4 checks (freeze-frames dropped after overlay, document scale_factor = display scale, NSPasteboard copy round-trip at 2×, JPEG bytes). |
| 2026-09-25 | app-polish | **rs-core add-only: `rs_core::sanitize` module.** `Annotation::sanitize(&mut self, bounds: Size /* content px */)` (finite floats, normalized rects, coords clamped to bounds ± max(4·side, 4096), stroke ≤512, radii ≤10k, blur σ ≤256, pixelate 1..=1024, font 1..=2048, line height 0.5..5, zoom 1..8, head_scale 0.1..10, opacity 0..1, non-finite pen points dropped; id untouched), `Annotation::sanitize_all(&mut [Annotation], bounds, &mut HashSet<AnnotationId>)` (**use for pasted annotations**: sanitizes + fresh id for any id already taken or repeated), `Document::sanitize()` (crop clamped, scale_factor 0.25..8 else 1, beautify, annotations, duplicate ids → fresh UUID), `BeautifySettings/Background/Padding/Inset/FrameShadow/Watermark/GradientStop/TextStyle/TextBackground/Stroke/ShapeStyle/Shadow/ToolStyle/ExportSettings/Hotkeys/Preferences::sanitize()`, `FontWeight::sanitized()`, `CaptureRegion::sanitized() -> Option`, `Hotkey::{is_valid, is_function_key}`, `sanitize::{limits::*, clamp_f32}`. All idempotent; values the UI produces pass unchanged. rs-smart calls them on `.rsnap` load (`project_from_bytes`, so history load too) and preferences load. **New pref `Preferences.window_capture_transparent: bool`** (default true; `window_capture_shadow` now means shadow only; rs-smart migrates a missing value from `window_capture_shadow`) — resolves the §12 request. |
| 2026-09-25 | editor-polish | **rs-render (additive):** `callout_bubble_rect(&CalloutAnnotation) -> Rect` (bubble grows vertically to fit wrapped text + padding, away from the tail; used for drawing, bounds, hit-test, `callout_text`); `encode_with_density(&RgbaImage, ExportFormat, px_per_pt)` + `dpi_for_px_per_pt` — PNG `pHYs` / JPEG JFIF dpi (WebP: none); `export()` now embeds density = `export.scale.factor()` (X2 → 144 dpi); `encode()` unchanged (no density). Text layout splits hard lines exactly like cosmic-text (`\r`, `\r\n`, `\n\r`) and every caret/hit/selection index is a char boundary (fixes lone-CR mid-char offsets). Magnifiers above a spotlight sample the undimmed canvas (content + annotations below), drawn into both. **rs-editor (additive):** `pub mod fit` (fit zoom ≤ `scale_factor.clamp(1,2)`, 24 pt margin; initial window = canvas at actual size + chrome, ≥ 1100×700, ≤ 90 % of the visible screen); live text editing is rendered by rs-render (no exclude) with caret/selection/IME overlays from rs-render geometry (`EditGeom`), 530 ms caret blink; multi-selection bounding-box handles (`Interaction::MultiResize`, `hit::scale_annotation`, one undo step); pasted annotations go through `Annotation::sanitize_all` + shifted into the image. No rs-core changes. |
| 2026-09-25 | app-polish | App/capture polish (no signature changes). **rs-capture additive:** `ClipboardSnapshot { items: Vec<Vec<(String /*UTI*/, Vec<u8>)>> }::{take, restore, byte_len}` (every pasteboard item/type; single item restored eagerly via `setData:forType:`); `capture_window` with `include_shadow && !transparent_background` now flattens the shadowed window onto the on-screen pixels behind it (was: transparent). **App:** window-capture prefs split (Shadow / Transparent background switches); Preferences window observes `PreferencesGlobal` (format/scale/delay/history selects + filename template follow editor/tray changes) and refreshes the login-item status on open, after toggling and on window activation (RequiresApproval → switch on + "Approve…" hint; NotFound → move-to-/Applications hint; Unavailable → "bundle only" hint); self-test saves/restores the user's clipboard around the copy round-trip, never posts global hotkeys while another RapidSnap (v1 / a user copy of v2) runs, checks prefs-window sync and the overlay hint; overlay hint bar is state-aware (idle / mouse drag / keyboard selection / window mode) and jumps to the top when the selection or cursor is near the bottom — overlay-only, captures are cropped from the freeze-frame; `rs_render::warm_up_fonts()` and the desktop-wallpaper resolve moved from startup to capture start / document open; `RAPIDSNAP_NO_WELCOME=1` debug knob. Idle (release bundle, tray-only, 12 s): phys footprint **≈19 MB** (before 18.7–19.6 MB; fonts are mmap'd so the saving is small), `ps` RSS ≈80 MB both before/after (dominated by shared-cache framework pages: AppKit/WebKit/SwiftUI/CoreML… pulled in by linked frameworks); with welcome window 47 MB footprint / 104 MB RSS. `scripts/bundle.sh`: `RAPIDSNAP_SIGN_IDENTITY` (stable signature), `RAPIDSNAP_DIST_DIR`. New root `README.md`. |
| 2026-09-25 | QA | Integration of the two parallel polish passes: workspace build/test/clippy clean. **objc2 framework crates: default features off** in `[workspace.dependencies]`, per-crate header features (rs-capture, rs-smart, rs-editor, rapidsnap); `objc2-image-io` moved to the workspace. Load commands drop CoreML, AVFoundation, UniformTypeIdentifiers (our own crates); CloudKit / CoreData / CoreImage (gpui-base / gpui-component use objc2-app-kit with defaults) and CoreLocation (gpui → objc2-user-notifications defaults) stay — a vendored `[patch.crates-io]` removing them was measured and **not kept**: 1102 dyld images at launch either way and identical RSS/footprint (AppKit/Vision load them transitively). **rs-editor:** ⌘C copies the image unless the Select tool is active with annotations selected (a freshly drawn shape stays selected — ⌘C used to put annotation JSON on the clipboard); new action `CopySelection` (context-menu Copy = selected annotations); filename template `{app}` now filled from window captures (`default_file_stem(.., app)`). **App:** `platform::bring_to_front` (deferred activate + `orderFrontRegardless` + key) for editor / preferences / welcome windows; prefs flushed on every quit path (`on_app_quit`); tray accelerators shown only for hotkeys we actually registered, ⇧⌘V shown on Open from Clipboard; Preferences → Output: JPEG quality; self-test never posts events without `CGPreflightPostEventAccess` and posts Esc only while our overlay is the key window. See `QA.md`. |
| 2026-09-25 | design | **rs-core add-only: `Preferences.panel_variant: u8`** (serde default 0; 0 = A Inspector, 1 = B Cards, 2 = C Minimal; values > 2 are treated as 2 by the editor). The editor reads it when a window opens and writes it when the user clicks the floating "Panel A·B·C" switcher at the bottom-left of the canvas. That switcher is a GPUI overlay only and never appears in rs-render output or exports. rs-editor side panel rebuilt: `panel.rs` (state, handlers, declarative `Spec` builders = the single source of behavior), `panel_spec.rs` (spec types + shared primitives/tokens), `panel_variants.rs` (the 3 layouts + switcher), `panel_icons.rs` (inline panel icons), `panel_selftest.rs` (self-test: checks each variant persists, JPEG quality survives format switches, and a picker drag records a single recent color; `RAPIDSNAP_EDITOR_SELFTEST_SHOT=1` + optional `RAPIDSNAP_PANEL_SHOT_DIR`, `RAPIDSNAP_PANEL_DARK=1` capture per-variant shots). Panel fixes from QA: JPEG keeps its quality (plus a Quality slider), image/signature watermark (Type Text/Image, file picker, width), beautify shadow Offset X, recent colors recorded only after the picker settles (600 ms). Research notes in `docs/design/sidebar-research.md`. |
| 2026-09-25 | canvas | **Annotations anywhere on the canvas** (padding / inset / chrome / background) — see §3. **rs-render:** annotations no longer clipped to the content rect ∩ rounded frame (clip mask + `intersect_masks` removed); drawable bounds = whole canvas for shapes, text, group layers, blur/pixelate/redact, spotlight dim and magnifier sampling; base-layer key (also the blur-cache signature) now includes the content identity (fixes a stale blur after a size-preserving 180° rotation). No signature changes. **rs-core add-only:** `Annotation::sanitize_with_margin`, `Annotation::sanitize_all_with_margin`, `Document::annotation_margin()`; `Document::sanitize` uses the margin (no behavior change for existing values). **rs-editor:** `EditorModel::{canvas_rect, drawable_rect, canvas_sample, last_picked}`, `SnapTargets::with_canvas`, `PreviewFrame.rgba`; paste keeps groups on the canvas; snap guides span the canvas; eyedropper samples the composed canvas. Tests: rs-render (padding export 1×/2×, not clipped by rounded frame, destructive obfuscation over background, spotlight/magnifier in padding, crop/rotate remap, beautify-off clip, preview==export with padding annotations), rs-core (sanitize keeps outside coords / rejects garbage), rs-editor (create/pick/move/resize/marquee/text/paste/eyedropper in padding, canvas snapping); editor self-test "padding" section (real events: rect + typed text in the padding, step moved image→padding, undo/redo, hover, eyedropper, PNG export pixel checks). |
| 2026-09-25 | final-panel | **Single side panel (was variant C) — A, B and the floating "Panel A·B·C" switcher removed.** rs-core: `Preferences.panel_variant` kept (add-only) but documented as deprecated/unused; the editor no longer reads or writes it. rs-editor: `panel_variants.rs` → `panel_layout.rs` (one compact layout); `panel_spec.rs` trimmed (no per-variant looks, `Vis::Aspect`, `Head.icon`; `Opt::tip`; `Spec.master` is a `Tog` shown at the right of the tab bar). Compact geometry: panel **256 pt** (was 300), 12 pt gutters, 22 pt controls, 4 pt row gap, 12 pt section gap, 30 pt tab bar, 6-per-row background thumbnails, slider rows with inline value; advanced rows behind each section's "More", advanced sections behind one "More options" disclosure. Aspect ratio = text-only two-row segmented chips (Free · 1:1 · 4:3 · 16:9 · X · LinkedIn · YouTube · Instagram) with exact-ratio tooltips. Window: `rs_editor::fit::MIN_WINDOW` = 720×500 (was 760×480), initial size never below it; toolbar degrades by width (secondary actions → "…" menu, then Save icon-only, then Copy icon-only with tighter tool spacing) so nothing clips at 720 pt. Panel self-test: measures panel content height per view (default content must fit 500 pt; logged), shots `light|dark-{style-arrow,style-text,style-rect,style-step,style-blur,style-empty,style-arrow-more,beautify,beautify-more,window-min-style,window-min-beautify,window-normal}.png`. No other cross-crate changes. |
| 2026-09-26 | identity | **App identity = RapidSnap v1's (v2 ships as its successor).** Bundle id `com.rapidsnap.v2` → `io.github.ahmedash95.rapidsnap`, executable `Contents/MacOS/rapidsnap` → `RapidSnap` (cargo bin stays `rapidsnap`), version 2.0.0 (`crates/rapidsnap/Cargo.toml` now has its own version; `scripts/bundle.sh` reads it), `codesign -i io.github.ahmedash95.rapidsnap`, project UTI `com.rapidsnap.project` → `io.github.ahmedash95.rapidsnap.project` (.rsnap unchanged). **rs-smart:** `paths::APP_ID` = `io.github.ahmedash95.rapidsnap` (data dir `~/Library/Application Support/io.github.ahmedash95.rapidsnap/`); additive `paths::{LEGACY_APP_ID, LOCK_FILE_NAME, Migration, migrate_legacy_data_dir, migrate_legacy_data_dir_from}` — one-time move of the pre-release `com.rapidsnap.v2/` contents when the new dir has no `preferences.json` (skips the lock file and names already present, deletes nothing, off when `RAPIDSNAP_DATA_DIR` is set; unit-tested with temp dirs). **App:** calls the migration before the instance lock; `platform::is_other_rapidsnap` (pure, unit-tested) — self-test other-instance detection compares by pid only, so it never counts itself although it shares v1's id/name. v1's sandbox container is not migrated. |
| 2026-09-26 | icon | **New app icon "Snap Mark · Graphite"** (source `assets/logo-concepts/claude/snap-mark-white/v2/`, generator in `assets/logo-concepts/claude/_source/`). `assets/icons/app-icon-{16..1024}.png` were replaced; they are now on Apple's 1024 template (824 body + shadow margin) instead of the old full-bleed square, and 16/32 px come from tuned masters. `AppIcon.icns` was rebuilt with `iconutil` (all 10 sizes). The tray template is now the Snap Mark glyph at **18×18 / 36×36** (was 22/44), because tray-icon 0.25 always draws the status-item image 18 pt high, so the @2x image maps 2:1 and stays crisp. `with_icon_as_template(true)` is unchanged. `scripts/gen_tray_icon.py` was rewritten to reproduce the new glyph (still no dependencies). Runtime/bundle paths are unchanged (`icons/tray-template@2x.png`, `icons/app-icon-256.png`, `AppIcon.icns`/`CFBundleIconFile`). Build, tests, bundle and the app self-test (22/22) all pass. |

## 12. Requests (non-blocking)

| From | To | Request |
|---|---|---|
| app | rs-core | ~~Optional add-only pref `window_capture_transparent: bool`~~ — **done** (app-polish, see Changelog). |

---

*Generated with Claude AI — please review before distribution.*
