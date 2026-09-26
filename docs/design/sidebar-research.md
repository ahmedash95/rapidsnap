# Editor side panel — design research

> **Outcome (2026-09-25):** variant C (Minimal) was kept and made compact (256 pt wide, 22 pt controls, 4 pt rhythm,
> "More options" disclosure, text-only aspect-ratio chips); A, B and the switcher were removed. See `panel_layout.rs`.

Scope: the right-hand Style / Beautify panel of the RapidSnap v2 editor. This is a short summary of what well-regarded
inspectors do, what is wrong with the current panel, and the three variants built from it.

## What the current panel gets wrong (before screenshots, 2026-09-25)

- **Mixed type sizes.** Switch labels are 14 px, row labels 12 px, the title 14 px semibold and section headers 12 px muted.
  The labels don't read as one system.
- **Broken grid.** Sliders and segmented buttons start at the 78 px label column, but switches start at the panel edge and
  their labels land where controls should be. Values sit in a 38 px column, so `4.0` and `100%` don't line up.
- **Wrapping segmented rows.** Background kind (6 options) and aspect ratio (8) wrap onto a ragged second line. Font weight
  (6) and text background (4) don't fit either.
- **Palette overflow.** 10 swatches plus recents wrap, so a lone black dot ends up on its own line.
- **Weak hierarchy.** Section titles look like row labels, there are no dividers or grouping, and the selected state of
  segmented buttons is barely visible.
- **No way to reduce noise.** Every option is always shown, and rare settings (export defaults, per-side padding, caps,
  presets) get the same weight as color and width.

## Principles from the references

1. **A strict two-column grid.** Use a fixed label column and a control column that takes the rest of the width. All inputs,
   selects and buttons share one height, and switches sit in the same grid as every other control. Design-tool inspectors
   are typically 240–320 px wide. ([aligned label/control grid PR][grid], [Tailwind inspector layout][tw])
2. **Put the most-used controls first and cut the rest.** Figma's UI3 rethought the properties panel "to prioritize
   critical controls". It removed rarely used input fields, added backgrounds to inputs and borders to dropdowns so complex
   controls are easier to parse, and made the panel collapsible and resizable. ([Figma: behind UI3][ui3],
   [Figma: our approach to UI3][ui3b], [Fast Company][fc])
3. **Progressive disclosure.** Hide what isn't needed until it's asked for, which gives a simpler first view and faster
   task completion. Sections should be collapsible and keyboard operable. ([NN/g][nng], [IxDF][ixdf])
4. **One spacing scale.** macOS layout is built on an 8 pt grid with 20 pt window margins. Stacked regular controls get
   at least 6 pt between them, plus 12 pt around separators and 8 pt extra between groups. ([layout guidelines][mag],
   [Apple HIG][hig])
5. **Quiet, dense, aligned.** Linear's redesigns cut visual noise in panels and headers, kept everything on the same
   alignment axes and raised hierarchy and density. Small uppercase group labels and hairline dividers carry the
   structure instead of boxes. ([Linear: calmer interface][linear], [Linear: how we redesigned][linear2])
6. **Visual choices over words.** Screenshot beautifiers (Xnapper, CleanShot X) show backgrounds, gradients and
   wallpapers as thumbnails and keep the flow short: capture, beautify, share. Sketch's rewritten inspector adds scrubbable
   numeric fields and floating color panels. ([Xnapper vs CleanShot][sweetbits], [CleanShot X features][cleanshot],
   [Sketch Copenhagen tour][sketch])

## Design rules applied to all three variants

- Type scale: panel title 13 px semibold · section header 11 px semibold (uppercase in A) · labels and values 12 px.
  Nothing else.
- Spacing on 4 px multiples: 16 px panel gutter, 8 px between rows, 16–20 px between sections.
- One control height: 24 px (segmented controls, value boxes, buttons, selects). Swatches are 16 px, or 20 px in B.
- No wrapping segmented rows. Anything with more than 4 options becomes icons, glyphs ("Aa" drawn at each font weight),
  shape tiles (aspect ratios drawn to scale) or an equal-width grid.
- The palette sits in a fixed 10-column grid, with recent colors as a second row on the same columns.
- Section-level on/off (shadow, inset, watermark, window frame) lives in the section header, and the section's rows only
  appear when it is on.
- Every control comes from one shared declarative spec (`panel_spec.rs`). The variants only change the layout, so
  behavior is never duplicated.

## The variants

| | A · Inspector | B · Cards | C · Minimal |
|---|---|---|---|
| Inspired by | Apple HIG inspectors, Figma UI3 | CleanShot X, Xnapper | Linear, progressive disclosure |
| Structure | Uppercase headers + hairline dividers, collapsible sections | Rounded soft cards with icon + title | Flat list; essentials visible, the rest under "More" |
| Rows | Fixed 80 px label column, control column, value box | Label + value above a full-width slider | Label / control / muted value; advanced rows hidden |
| Choices | Segmented controls with icons or glyphs | Large icon segments, big swatches and thumbnails | Icon toolbar rows |

[grid]: https://github.com/robertsonc/topology-dojo/pull/246
[tw]: https://codefronts.com/layouts/tailwind-css-sidebar-layouts/tailwind-right-aligned-sidebar-inspector-panel/
[ui3]: https://www.figma.com/blog/behind-our-redesign-ui3/
[ui3b]: https://www.figma.com/blog/our-approach-to-designing-ui3/
[fc]: https://www.fastcompany.com/91145720/figma-update-ui-more-powerful-than-ever
[nng]: https://www.nngroup.com/articles/progressive-disclosure/
[ixdf]: https://ixdf.org/literature/topics/progressive-disclosure
[mag]: https://marioaguzman.github.io/design/layoutguidelines/
[hig]: https://developer.apple.com/design/human-interface-guidelines/designing-for-macos
[linear]: https://linear.app/now/behind-the-latest-design-refresh
[linear2]: https://linear.app/now/how-we-redesigned-the-linear-ui
[sweetbits]: https://thesweetbits.com/cleanshot-vs-xnapper/
[cleanshot]: https://cleanshot.com/features
[sketch]: https://www.sketch.com/blog/a-tour-of-copenhagen/

---
Generated with Claude AI — please review before distribution.
