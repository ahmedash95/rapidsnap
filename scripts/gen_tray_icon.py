#!/usr/bin/env python3
"""Generate the monochrome menu-bar template icon (black + alpha).

Design: "Snap Mark", the same glyph as the app icon (assets/logo-concepts/claude/snap-mark-white/v2).
It has four rounded crop brackets and a hand-drawn marker loop whose width varies like a pen stroke.
The vector master is assets/logo-concepts/claude/snap-mark-white/v2/menubar-template.svg (22-unit design grid),
and its generator is assets/logo-concepts/claude/_source/gen.py (mark_menubar).

tray-icon draws the status-item image at a fixed height of 18 pt. So we write 18x18 (1x) and 36x36 (@2x)
PNGs, and the @2x image maps to Retina pixels 2:1. The glyph is cropped from the 22-unit grid to a
19-unit window (1.5..20.5), which makes it about 16 pt tall with a hairline of breathing room.

Run: python3 scripts/gen_tray_icon.py  (writes assets/icons/tray-template{,@2x}.png)
"""
import math, os, struct, zlib

# ---- geometry (22-unit design grid, identical to gen.py mark_menubar) ----
SW = 2.0                      # bracket stroke width
TL, BR, ARM, RR = 3.4, 18.6, 5.3, 2.4
WIN0, WIN = 1.5, 19.0         # crop window


def bracket_pts(x, y, sx, sy):
    pts = [(x, y + sy * ARM), (x, y + sy * RR)]
    for i in range(1, 25):     # quadratic corner (x, y+sy*RR) -> (x+sx*RR, y), control (x, y)
        t = i / 24
        p0, p1, p2 = (x, y + sy * RR), (x, y), (x + sx * RR, y)
        pts.append(((1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0],
                    (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1]))
    pts.append((x + sx * ARM, y))
    return pts


BRACKETS = [bracket_pts(TL, TL, 1, 1), bracket_pts(BR, BR, -1, -1), bracket_pts(BR, TL, -1, 1), bracket_pts(TL, BR, 1, -1)]


def loop_centerline(cx=11.0, cy=11.05, rx=4.7, ry=3.6, rot=-16, start=260, sweep=420, shrink=-0.14, lw=2.3,
                    steps=240, w0=0.6, taper_from=0.78, w1=0.55):
    out, r = [], math.radians(rot)
    for i in range(steps + 1):
        t = i / steps
        th = math.radians(start + sweep * t)
        k = 1 - shrink * t
        x, y = rx * k * math.cos(th), ry * k * math.sin(th)
        w = min(1.0, w0 + (1 - w0) * (t / 0.35)) if t < 0.35 else 1.0
        if t > taper_from:
            u = (t - taper_from) / (1 - taper_from)
            w *= 1 - (1 - w1) * (u * u * (3 - 2 * u))
        out.append((cx + x * math.cos(r) - y * math.sin(r), cy + x * math.sin(r) + y * math.cos(r), lw * w / 2))
    return out


LOOP = loop_centerline()


def seg_dist(px, py, a, b):
    ax, ay = a; bx, by = b
    dx, dy = bx - ax, by - ay
    L = dx * dx + dy * dy
    t = 0 if L == 0 else max(0, min(1, ((px - ax) * dx + (py - ay) * dy) / L))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def inside(u, v):
    for pts in BRACKETS:
        for a, b in zip(pts, pts[1:]):
            if seg_dist(u, v, a, b) <= SW / 2:
                return True
    for (x0, y0, r0), (x1, y1, r1) in zip(LOOP, LOOP[1:]):
        if seg_dist(u, v, (x0, y0), (x1, y1)) <= (r0 + r1) / 2:
            return True
    return False


def render(size, ss=6):
    s = size / WIN
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            c = 0
            for j in range(ss):
                for i in range(ss):
                    if inside(WIN0 + (x + (i + 0.5) / ss) / s, WIN0 + (y + (j + 0.5) / ss) / s):
                        c += 1
            row += bytes([0, 0, 0, round(255 * c / (ss * ss))])
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "icons")
for size, name in [(18, "tray-template.png"), (36, "tray-template@2x.png")]:
    with open(os.path.join(out, name), "wb") as f:
        f.write(render(size))
    print("wrote", name, f"{size}x{size}")
