//! `RAPIDSNAP_EDITOR_SELFTEST=1`: drive the editor with *real* GPUI input
//! events (keystrokes + mouse through `Window::dispatch_event`), check the
//! document, undo/redo, export a PNG and quit.
//!
//! Output dir: `$RAPIDSNAP_EDITOR_SELFTEST_DIR` or `$TMPDIR/rapidsnap-editor-selftest`.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::*;
use rs_core::Point as CPoint;

use crate::view::EditorView;

fn out_dir() -> PathBuf {
    std::env::var_os("RAPIDSNAP_EDITOR_SELFTEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rapidsnap-editor-selftest"))
}

pub(crate) fn start(window: &mut Window, cx: &mut Context<EditorView>) {
    cx.spawn_in(window, async move |this: WeakEntity<EditorView>, cx: &mut AsyncWindowContext| {
        let exec = cx.background_executor().clone();
        let sleep = |ms: u64| exec.timer(Duration::from_millis(ms));
        // Wait for the first rendered frame.
        for _ in 0..100 {
            sleep(100).await;
            if this.update(cx, |v, _| v.frame_generation().is_some()).unwrap_or(false) {
                break;
            }
        }
        log::info!("selftest: first frame ready");
        let size = this.update(cx, |v, _| v.document().content_size()).unwrap_or_default();
        let (w, h) = (size.width, size.height);
        let count = |cx: &mut AsyncWindowContext| this.update(cx, |v, _| v.document().annotations.len()).unwrap_or(0);
        let start_count = count(cx);

        macro_rules! key {
            ($k:expr) => {{
                let _ = cx.update(|window, cx| {
                    if let Ok(ks) = Keystroke::parse($k) {
                        window.dispatch_keystroke(ks, cx);
                    }
                });
                sleep(30).await;
            }};
        }
        let to_win = |cx: &mut AsyncWindowContext, x: f32, y: f32| {
            this.update(cx, |v, _| v.content_to_window(CPoint::new(x, y))).unwrap_or_default()
        };
        macro_rules! mouse {
            (move $p:expr, $pressed:expr) => {{
                let p = $p;
                let _ = cx.update(|window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: $pressed, modifiers: Modifiers::default() }),
                        cx,
                    );
                });
            }};
            (down $p:expr, $clicks:expr) => {{
                let p = $p;
                let _ = cx.update(|window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Left,
                            position: p,
                            modifiers: Modifiers::default(),
                            click_count: $clicks,
                            first_mouse: false,
                        }),
                        cx,
                    );
                });
            }};
            (up $p:expr) => {{
                let p = $p;
                let _ = cx.update(|window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Left,
                            position: p,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
                });
            }};
        }
        let shots = std::env::var("RAPIDSNAP_EDITOR_SELFTEST_SHOT").is_ok();
        macro_rules! shot {
            ($name:expr) => {{
                shot!($name, 700)
            }};
            ($name:expr, $delay:expr) => {{
                if shots {
                    sleep($delay).await;
                    let p = out_dir().join(format!("{}.png", $name));
                    let _ = cx.update(|window, _| match crate::debug_shot::capture_window(window, &p) {
                        Ok(()) => log::info!("selftest: window shot {}", p.display()),
                        Err(e) => log::warn!("selftest: window shot failed: {e}"),
                    });
                }
            }};
        }
        shot!("shot0-initial");

        // --- Fit zoom: initial zoom == fit rule, ⌘= leaves fit, ⌘0 returns.
        let fit_state = |cx: &mut AsyncWindowContext| {
            this.update_in(cx, |v, window, _| {
                let vp = v.canvas_viewport();
                (v.zoom(), v.fit_zoom(window), v.is_fit(), vp, v.canvas_display_size())
            })
            .unwrap_or_default()
        };
        let (z0, expect0, fit0, vp, disp) = fit_state(cx);
        let sf = this.update(cx, |v, _| v.document().metadata.scale_factor).unwrap_or(1.0);
        let m = crate::fit::FIT_MARGIN;
        let fills = disp.0 >= vp.0 - 2.0 * m - 1.5 || disp.1 >= vp.1 - 2.0 * m - 1.5;
        let at_cap = (z0 - crate::fit::fit_zoom_cap(sf)).abs() < 1e-3;
        let mut ok_fit = fit0 && (z0 - expect0).abs() < 1e-3 && (fills || at_cap);
        log::info!(
            "selftest: fit zoom {z0:.3} (expected {expect0:.3}, cap {:.1}) viewport {vp:?} canvas shown {disp:?} fills={fills}",
            crate::fit::fit_zoom_cap(sf)
        );
        key!("cmd-=");
        sleep(50).await;
        let (z1, _, fit1, _, _) = fit_state(cx);
        key!("cmd-0");
        sleep(50).await;
        let (z2, expect2, fit2, _, _) = fit_state(cx);
        ok_fit &= !fit1 && (z1 - z0 * 1.25).abs() < 1e-3 && fit2 && (z2 - expect2).abs() < 1e-3;
        log::info!("selftest: zoom in → {z1:.3} (fit {fit1}), ⌘0 → {z2:.3} (fit {fit2}); fit ok={ok_fit}");
        macro_rules! drag {
            ($x0:expr, $y0:expr, $x1:expr, $y1:expr) => {{
                let a = to_win(cx, $x0, $y0);
                let b = to_win(cx, $x1, $y1);
                mouse!(move a, None);
                sleep(20).await;
                mouse!(down a, 1);
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    mouse!(move point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t), Some(MouseButton::Left));
                    sleep(10).await;
                }
                mouse!(up b);
                sleep(60).await;
            }};
        }
        macro_rules! click {
            ($x:expr, $y:expr) => {{
                let a = to_win(cx, $x, $y);
                mouse!(move a, None);
                sleep(20).await;
                mouse!(down a, 1);
                mouse!(up a);
                sleep(80).await;
            }};
        }

        key!("r");
        drag!(w * 0.06, h * 0.08, w * 0.30, h * 0.30);
        key!("a");
        drag!(w * 0.45, h * 0.15, w * 0.75, h * 0.40);
        key!("t");
        click!(w * 0.08, h * 0.62);
        sleep(150).await; // let the text editor paint + register the input handler
        for ch in ["H", "e", "l", "l", "o"] {
            key!(&ch.to_lowercase().replace('h', "shift-h"));
        }
        key!("enter");
        for ch in ["w", "o", "r", "l", "d"] {
            key!(ch);
        }
        sleep(150).await;
        // Live-edit geometry (rs-render caret rects mapped to the window) for
        // every caret stop, plus the text box.
        let live = this
            .update(cx, |v, _| {
                let l = v.edit_layout.borrow().clone()?;
                let t = l.ann.content.clone();
                let carets: Vec<(usize, Bounds<Pixels>)> =
                    (0..=t.len()).filter(|i| t.is_char_boundary(*i)).map(|i| (i, l.caret(i))).collect();
                Some((t, carets, l.box_bounds()))
            })
            .ok()
            .flatten();
        // Blink: with no input the caret toggles every ~530 ms.
        let mut phases = Vec::new();
        for _ in 0..6 {
            sleep(260).await;
            phases.push(this.update(cx, |v, _| (v.caret_on(), v.blinking())).unwrap_or((true, false)));
        }
        let ok_blink = phases.iter().any(|p| p.0) && phases.iter().any(|p| !p.0) && phases.iter().all(|p| p.1);
        log::info!("selftest: caret blink phases {phases:?} ok={ok_blink}");
        key!("left"); // input resets the blink → caret visible
        let reset_visible = this.update(cx, |v, _| v.caret_on()).unwrap_or(false);
        // Inside the first (visible) blink half-period after the reset.
        shot!("shot1-typing", 150);
        key!("cmd-enter");
        sleep(100).await;
        // Committed geometry: rs-render caret rects of the committed text
        // annotation, mapped through the same content→window transform.
        let committed = this
            .update(cx, |v, _| {
                let t = v.document().annotations.iter().rev().find_map(|a| match &a.kind {
                    rs_core::AnnotationKind::Text(t) => Some(t.clone()),
                    _ => None,
                })?;
                let map = |r: rs_core::Rect| {
                    let a = v.content_to_window(r.origin());
                    let b = v.content_to_window(CPoint::new(r.right(), r.bottom()));
                    Bounds::from_corners(a, b)
                };
                let carets: Vec<(usize, Bounds<Pixels>)> = (0..=t.content.len())
                    .filter(|i| t.content.is_char_boundary(*i))
                    .map(|i| (i, map(rs_render::caret_rect(&t, i))))
                    .collect();
                Some((t.content.clone(), carets, map(rs_render::text_box(&t)), v.blinking()))
            })
            .ok()
            .flatten();
        let ok_text_geom = match (&live, &committed) {
            (Some((lt, lc, lb)), Some((ct, cc, cb, blinking))) => {
                let d = |a: &Bounds<Pixels>, b: &Bounds<Pixels>| {
                    [a.origin.x - b.origin.x, a.origin.y - b.origin.y, a.size.width - b.size.width, a.size.height - b.size.height]
                        .into_iter()
                        .map(|v| f32::from(v).abs())
                        .fold(0.0f32, f32::max)
                };
                let max_caret = lc.iter().zip(cc.iter()).map(|(a, b)| if a.0 == b.0 { d(&a.1, &b.1) } else { f32::MAX }).fold(0.0, f32::max);
                let box_d = d(lb, cb);
                log::info!(
                    "selftest: live vs committed text geometry: {} carets, max caret Δ {max_caret:.3} px, box Δ {box_d:.3} px, blink after commit {blinking}",
                    lc.len()
                );
                lt == ct && lc.len() == cc.len() && max_caret < 0.01 && box_d < 0.01 && !*blinking
            }
            _ => false,
        };
        log::info!("selftest: text geometry ok={ok_text_geom}, caret visible after input={reset_visible}");
        key!("n");
        click!(w * 0.60, h * 0.75);
        click!(w * 0.70, h * 0.75);
        key!("v");
        sleep(200).await;
        // Select the arrow (click its middle) to show handles + its style panel.
        click!(w * 0.60, h * 0.275);
        shot!("shot2-selected");
        let _ = this.update(cx, |v, cx| {
            v.panel.tab = 1;
            cx.notify();
        });
        shot!("shot3-beautify");
        let _ = this.update(cx, |v, cx| {
            v.panel.tab = 0;
            cx.notify();
        });

        let after = count(cx);
        let text = this
            .update(cx, |v, _| {
                v.document().annotations.iter().rev().find_map(|a| match &a.kind {
                    rs_core::AnnotationKind::Text(t) => Some(t.content.clone()),
                    _ => None,
                })
            })
            .ok()
            .flatten()
            .unwrap_or_default();
        log::info!("selftest: annotations {start_count} → {after} (expected +5); last text {text:?}");
        key!("cmd-z");
        let undone = count(cx);
        key!("cmd-shift-z");
        let redone = count(cx);
        log::info!("selftest: undo → {undone}, redo → {redone}");

        // --- Multi-selection resize: the new rect + arrow, drag the
        // bounding box's bottom-right handle; one undo step.
        let ids: Vec<rs_core::AnnotationId> = this
            .update(cx, |v, _| v.document().annotations.iter().skip(start_count).take(2).map(|a| a.id).collect())
            .unwrap_or_default();
        let _ = this.update(cx, |v, cx| {
            v.model_mut().set_tool(rs_core::ToolKind::Select);
            v.model_mut().selection = ids.clone();
            cx.notify();
        });
        sleep(150).await;
        let before = this
            .update(cx, |v, _| {
                let originals: Vec<rs_core::Annotation> =
                    ids.iter().filter_map(|id| v.document().get(*id).cloned()).collect();
                let (u, _) = v.model_mut().multi_handles()?;
                Some((originals, u, v.model().history.undo_len()))
            })
            .ok()
            .flatten();
        let mut ok_multi = false;
        if let Some((originals, u, undo0)) = before {
            let br = crate::geom::RectHandle::BottomRight.position(u);
            drag!(br.x, br.y, br.x + u.width * 0.25, br.y + u.height * 0.25);
            sleep(150).await;
            shot!("shot4-multiresize");
            let res = this
                .update(cx, |v, _| {
                    let (u2, _) = v.model_mut().multi_handles()?;
                    let now: Vec<rs_core::Annotation> = ids.iter().filter_map(|id| v.document().get(*id).cloned()).collect();
                    Some((u2, now, v.model().history.undo_len()))
                })
                .ok()
                .flatten();
            if let Some((u2, now, undo1)) = res {
                // Each annotation == proportional scaling of its original.
                let mut max_d = 0.0f32;
                for (o, n) in originals.iter().zip(now.iter()) {
                    let e = crate::hit::scale_annotation(o, u, u2, true).geometric_bounds();
                    let g = n.geometric_bounds();
                    for d in [e.x - g.x, e.y - g.y, e.width - g.width, e.height - g.height] {
                        max_d = max_d.max(d.abs());
                    }
                }
                let grew = u2.width > u.width * 1.15 && u2.height > u.height * 1.15;
                log::info!(
                    "selftest: multi-resize union {u:?} → {u2:?}, max Δ vs proportional {max_d:.2} px, undo steps +{}",
                    undo1 - undo0
                );
                key!("cmd-z");
                let restored = this
                    .update(cx, |v, _| ids.iter().filter_map(|id| v.document().get(*id).cloned()).collect::<Vec<_>>())
                    .unwrap_or_default();
                ok_multi = grew && max_d < 3.0 && undo1 == undo0 + 1 && restored == originals;
            }
        }
        log::info!("selftest: multi-resize ok={ok_multi}");

        // --- Drawing over the beautify padding (whole-canvas annotations):
        // rect + typed text in the padding via real events, move a step from
        // the image into the padding, undo/redo, hover + eyedropper there,
        // export and check the pixels at the padding locations.
        let bg = rs_core::Color::rgb(20, 160, 90);
        let _ = this.update(cx, |v, cx| {
            let mut b = v.document().beautify.clone();
            b.enabled = true;
            b.background = rs_core::Background::Solid { color: bg };
            b.padding = rs_core::Padding::uniform(60.0);
            b.auto_balance = false;
            b.aspect_ratio = rs_core::AspectRatio::Free;
            b.inset.enabled = false;
            b.chrome = rs_core::WindowChrome::None;
            b.shadow.enabled = false;
            b.corner_radius = 16.0;
            b.watermark = None;
            let m = v.model_mut();
            m.commit_text();
            // The demo's spotlight dims the whole canvas (padding included);
            // remove it so the padding shows the plain background color.
            m.selection = m
                .doc
                .annotations
                .iter()
                .filter(|a| matches!(a.kind, rs_core::AnnotationKind::Spotlight(_)))
                .map(|a| a.id)
                .collect();
            m.delete_selection();
            m.selection.clear();
            m.set_tool(rs_core::ToolKind::Select);
            m.set_beautify(b, None);
            v.changed(cx);
        });
        let wait_frame = |cx: &mut AsyncWindowContext| {
            this.update(cx, |v, _| v.frame_generation() == Some(v.model().generation)).unwrap_or(false)
        };
        for _ in 0..50 {
            sleep(100).await;
            if wait_frame(cx) {
                break;
            }
        }
        let ok_padding = padding_checks(&this, cx, &sleep, h, w, bg, shots).await;
        log::info!("selftest: padding drawing ok={ok_padding}");
        // Side-panel layout variants (A/B/C) + per-variant window shots.
        let ok_panel = crate::panel_selftest::run(&this, cx, out_dir()).await;

        let ok = after == start_count + 5
            && undone + 1 == after
            && redone == after
            && text.to_lowercase() == "hello\nworld"
            && ok_fit
            && ok_text_geom
            && ok_blink
            && reset_visible
            && ok_multi
            && ok_padding
            && ok_panel;
        // Let the preview catch up so a screenshot (if any) shows the result.
        sleep(800).await;
        let dir = out_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("selftest.png");
        let p2 = path.clone();
        let _ = this.update_in(cx, move |v, window, cx| {
            v.export_to_path(p2, window, cx, move |r, _, cx| {
                match r {
                    Ok(()) => log::info!("selftest: exported {}", path.display()),
                    Err(e) => log::error!("selftest: export failed: {e}"),
                }
                if ok {
                    log::info!("SELFTEST PASS");
                } else {
                    log::error!("SELFTEST FAIL");
                }
                if std::env::var("RAPIDSNAP_EDITOR_SELFTEST_KEEP_OPEN").is_err() {
                    cx.quit();
                }
            });
        });
    })
    .detach();
}

fn dispatch(cx: &mut AsyncWindowContext, input: PlatformInput) {
    let _ = cx.update(|window, cx| {
        window.dispatch_event(input, cx);
    });
}

fn mouse_move(cx: &mut AsyncWindowContext, p: Point<Pixels>, pressed: Option<MouseButton>) {
    dispatch(cx, PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: pressed, modifiers: Modifiers::default() }));
}

fn mouse_down(cx: &mut AsyncWindowContext, p: Point<Pixels>, clicks: usize) {
    dispatch(
        cx,
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: p,
            modifiers: Modifiers::default(),
            click_count: clicks,
            first_mouse: false,
        }),
    );
}

fn mouse_up(cx: &mut AsyncWindowContext, p: Point<Pixels>) {
    dispatch(
        cx,
        PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: p, modifiers: Modifiers::default(), click_count: 1 }),
    );
}

fn keystroke(cx: &mut AsyncWindowContext, k: &str) {
    let _ = cx.update(|window, cx| {
        if let Ok(ks) = Keystroke::parse(k) {
            window.dispatch_keystroke(ks, cx);
        }
    });
}

/// The padding section of the self-test (see the call site). `h`/`w` =
/// content size. Returns overall success; logs every sub-check.
async fn padding_checks<T: std::future::Future>(
    this: &WeakEntity<EditorView>,
    cx: &mut AsyncWindowContext,
    sleep: &impl Fn(u64) -> T,
    h: f32,
    w: f32,
    bg: rs_core::Color,
    shots: bool,
) -> bool {
    use rs_core::AnnotationKind;
    let to_win = |cx: &mut AsyncWindowContext, x: f32, y: f32| {
        this.update(cx, |v, _| v.content_to_window(CPoint::new(x, y))).unwrap_or_default()
    };
    let canvas = this.update(cx, |v, _| v.model().canvas_rect()).ok().flatten();
    let Some(canvas) = canvas else {
        log::error!("selftest: padding: beautify canvas missing");
        return false;
    };
    let pad = -canvas.x; // content px of padding on the left/top
    log::info!("selftest: padding: canvas (content space) {canvas:?}, padding {pad} px");
    let count = |cx: &mut AsyncWindowContext| this.update(cx, |v, _| v.document().annotations.len()).unwrap_or(0);
    let n0 = count(cx);
    macro_rules! drag {
        ($x0:expr, $y0:expr, $x1:expr, $y1:expr) => {{
            let a = to_win(cx, $x0, $y0);
            let b = to_win(cx, $x1, $y1);
            mouse_move(cx, a, None);
            sleep(20).await;
            mouse_down(cx, a, 1);
            for i in 1..=8 {
                let t = i as f32 / 8.0;
                mouse_move(cx, point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t), Some(MouseButton::Left));
                sleep(10).await;
            }
            mouse_up(cx, b);
            sleep(80).await;
        }};
    }
    macro_rules! key {
        ($k:expr) => {{
            keystroke(cx, $k);
            sleep(30).await;
        }};
    }

    // 1. Rectangle entirely inside the left padding.
    let (rx0, ry0, rx1, ry1) = (-pad * 0.8, h * 0.2, -pad * 0.25, h * 0.45);
    key!("r");
    drag!(rx0, ry0, rx1, ry1);
    let rect = this
        .update(cx, |v, _| v.document().annotations.last().cloned())
        .ok()
        .flatten()
        .filter(|a| matches!(a.kind, AnnotationKind::Rect(_)));
    let rect_ok = rect.as_ref().is_some_and(|a| {
        let g = a.geometric_bounds();
        g.right() < 0.0 && g.x >= canvas.x && (g.x - rx0).abs() < 2.0 && (g.bottom() - ry1).abs() < 2.0
    });
    log::info!("selftest: padding: rect {:?} ok={rect_ok}", rect.as_ref().map(|a| a.geometric_bounds()));

    // 2. Text placed in the bottom padding and typed via key events.
    key!("t");
    let tp = CPoint::new(w * 0.1, h + pad * 0.25);
    let a = to_win(cx, tp.x, tp.y);
    mouse_move(cx, a, None);
    sleep(40).await;
    let hover_ok = this.update(cx, |v, _| v.model().hover.is_some_and(|h| h.y > v.document().content_size().height)).unwrap_or(false);
    mouse_down(cx, a, 1);
    mouse_up(cx, a);
    sleep(200).await;
    for k in ["shift-p", "a", "d", "space", "n", "o", "t", "e"] {
        key!(k);
    }
    sleep(100).await;
    if shots {
        sleep(500).await;
        let p = out_dir().join("shot5-padding-typing.png");
        let _ = cx.update(|window, _| crate::debug_shot::capture_window(window, &p));
    }
    key!("cmd-enter");
    sleep(100).await;
    let text = this
        .update(cx, |v, _| {
            v.document().annotations.iter().rev().find_map(|a| match &a.kind {
                AnnotationKind::Text(t) if t.position.y > h => Some((a.clone(), t.clone())),
                _ => None,
            })
        })
        .ok()
        .flatten();
    let text_ok = text.as_ref().is_some_and(|(_, t)| t.content == "Pad note" && (t.position.x - tp.x).abs() < 1.0 && (t.position.y - tp.y).abs() < 1.0);
    log::info!("selftest: padding: text {:?} ok={text_ok} (hover preview over padding: {hover_ok})", text.as_ref().map(|t| (&t.1.content, t.1.position)));

    // 3. Move the first step counter (in the image) into the right padding.
    let step = this
        .update(cx, |v, _| {
            v.document().annotations.iter().find_map(|a| match &a.kind {
                AnnotationKind::Step(s) if s.center.x < w => Some((a.id, s.center)),
                _ => None,
            })
        })
        .ok()
        .flatten();
    let mut move_ok = false;
    let mut hovered_ok = false;
    let mut step_after = None;
    if let Some((sid, c0)) = step {
        key!("v");
        key!("escape");
        let dest = CPoint::new(w + pad * 0.5, c0.y);
        drag!(c0.x, c0.y, dest.x, dest.y);
        let center = |cx: &mut AsyncWindowContext| {
            this.update(cx, |v, _| match v.document().get(sid).map(|a| &a.kind) {
                Some(AnnotationKind::Step(s)) => Some(s.center),
                _ => None,
            })
            .ok()
            .flatten()
        };
        let moved = center(cx);
        key!("cmd-z");
        let undone = center(cx);
        key!("cmd-shift-z");
        let redone = center(cx);
        let near = |p: Option<CPoint>, q: CPoint| p.is_some_and(|p| p.distance(q) < 1.5);
        move_ok = near(moved, dest) && near(undone, c0) && near(redone, dest);
        log::info!("selftest: padding: step {c0:?} → {moved:?}, undo {undone:?}, redo {redone:?} ok={move_ok}");
        step_after = redone.map(|c| (sid, c));
        // Hover over the moved step (select tool) → hovered.
        key!("escape");
        let hp = to_win(cx, dest.x, dest.y);
        mouse_move(cx, point(hp.x + px(1.), hp.y), None);
        sleep(40).await;
        mouse_move(cx, hp, None);
        sleep(40).await;
        hovered_ok = this.update(cx, |v, _| v.model().hovered == Some(sid)).unwrap_or(false);
        log::info!("selftest: padding: hover over moved step ok={hovered_ok}");
    }
    let n1 = count(cx);

    // 4. Eyedropper over the (empty) padding samples the background.
    for _ in 0..30 {
        sleep(100).await;
        if this.update(cx, |v, _| v.frame_generation() == Some(v.model().generation)).unwrap_or(false) {
            break;
        }
    }
    let _ = this.update(cx, |v, cx| {
        v.model_mut().selection.clear();
        v.model_mut().eyedropper = true;
        cx.notify();
    });
    sleep(50).await;
    let ep = to_win(cx, canvas.x + 6.0, canvas.y + 6.0);
    mouse_move(cx, ep, None);
    mouse_down(cx, ep, 1);
    mouse_up(cx, ep);
    sleep(80).await;
    let picked = this
        .update(cx, |v, _| {
            let p = v.model().picked_color;
            (p, v.model().eyedropper, v.model().last_picked)
        })
        .unwrap_or((None, true, None));
    let eyedropper_ok = !picked.1 && picked.2.is_some_and(|c| c.r.abs_diff(bg.r) <= 2 && c.g.abs_diff(bg.g) <= 2 && c.b.abs_diff(bg.b) <= 2);
    log::info!("selftest: padding: eyedropper over padding picked {:?} ok={eyedropper_ok}", picked.2);

    if shots {
        sleep(700).await;
        let p = out_dir().join("shot6-padding.png");
        let _ = cx.update(|window, _| crate::debug_shot::capture_window(window, &p));
    }

    // 5. Export a PNG and check the pixels at the padding locations.
    let path = out_dir().join("padding-export.png");
    let _ = std::fs::create_dir_all(out_dir());
    let (tx, rx) = smol::channel::bounded::<Result<(), String>>(1);
    let p2 = path.clone();
    let _ = this.update_in(cx, move |v, window, cx| {
        v.export_to_path(p2, window, cx, move |r, _, _| {
            let _ = tx.try_send(r);
        });
    });
    let exported = rx.recv().await.map_err(|e| e.to_string()).and_then(|r| r);
    let doc = this.update(cx, |v, _| v.document().clone()).ok();
    let mut pixels_ok = false;
    if let (Ok(()), Some(doc)) = (&exported, doc) {
        match image::open(&path) {
            Ok(img) => {
                let img = img.to_rgba8();
                let opts = rs_render::RenderOptions::for_export(&doc, &rs_core::ExportSettings::default());
                let l = rs_render::compute_layout(&doc, opts.scale);
                let size_ok = (img.width(), img.height()) == (l.canvas_width, l.canvas_height);
                let at = |p: CPoint| {
                    let q = l.image_to_canvas(p);
                    img.get_pixel_checked(q.x as u32, q.y as u32).map(|p| p.0).unwrap_or([0; 4])
                };
                let is_bg = |c: [u8; 4]| c[0].abs_diff(bg.r) <= 2 && c[1].abs_diff(bg.g) <= 2 && c[2].abs_diff(bg.b) <= 2;
                let close = |c: [u8; 4], k: rs_core::Color| c[0].abs_diff(k.r) <= 6 && c[1].abs_diff(k.g) <= 6 && c[2].abs_diff(k.b) <= 6;
                // Rect stroke on its left edge (in the padding), background inside it.
                let rect_px = rect.as_ref().and_then(|a| match &a.kind {
                    AnnotationKind::Rect(r) => Some((
                        at(CPoint::new(r.rect.x, r.rect.center().y)),
                        r.style.stroke.color,
                        at(r.rect.center()),
                    )),
                    _ => None,
                });
                let rect_pix_ok = rect_px.is_some_and(|(edge, col, inside)| close(edge, col) && is_bg(inside));
                // Text: some glyph pixels inside its box in the bottom padding.
                let text_pix_ok = text.as_ref().is_some_and(|(_, t)| {
                    let b = l.image_rect_to_canvas(rs_render::text_box(t));
                    let mut ink = 0;
                    for y in b.y as u32..(b.bottom() as u32).min(img.height()) {
                        for x in b.x as u32..(b.right() as u32).min(img.width()) {
                            if !is_bg(img.get_pixel(x, y).0) {
                                ink += 1;
                            }
                        }
                    }
                    ink > 20
                });
                // Step moved into the right padding: its fill at the center.
                // (Sampled left of the centered label.)
                let step_pix_ok = step_after.is_some_and(|(sid, c)| {
                    let fill = doc.get(sid).and_then(|a| match &a.kind {
                        AnnotationKind::Step(s) => Some((s.fill, s.radius)),
                        _ => None,
                    });
                    fill.is_some_and(|(f, r)| c.x > w && close(at(CPoint::new(c.x - r * 0.7, c.y)), f))
                });
                let corner_bg = is_bg(img.get_pixel(3, 3).0);
                pixels_ok = size_ok && rect_pix_ok && text_pix_ok && step_pix_ok && corner_bg;
                log::info!(
                    "selftest: padding: export {} {}x{} size={size_ok} rect={rect_pix_ok} ({rect_px:?}) text={text_pix_ok} step={step_pix_ok} bg={corner_bg}",
                    path.display(),
                    img.width(),
                    img.height()
                );
            }
            Err(e) => log::error!("selftest: padding: reading export failed: {e}"),
        }
    } else {
        log::error!("selftest: padding: export failed: {exported:?}");
    }
    let ok = rect_ok && text_ok && hover_ok && move_ok && hovered_ok && eyedropper_ok && pixels_ok && n1 == n0 + 2;
    log::info!("selftest: padding: annotations {n0} → {n1} (expected +2)");
    ok
}
