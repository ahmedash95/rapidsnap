#!/usr/bin/env python3
"""Generate the monochrome menu-bar template icon (black + alpha) at 22x22 and 44x44.

Design: four viewfinder corner brackets + a filled center circle.
Run: python3 scripts/gen_tray_icon.py  (writes assets/icons/tray-template{,@2x}.png)
"""
import os, struct, zlib

def render(size):
    ss = 4  # supersampling
    n = size * ss
    s = n / 22.0  # design units -> supersampled px
    def inside(x, y):
        u, v = x / s, y / s  # design coords 0..22
        t = 2.0          # bracket thickness
        m, L = 2.0, 7.0  # margin, bracket arm length
        lo, hi = m, 22 - m
        def bracket(cx, cy, dx, dy):
            # corner at (cx,cy), arms going in dx,dy directions
            xs = sorted([cx, cx + dx * L]); ys = sorted([cy, cy + dy * t])
            h = xs[0] <= u <= xs[1] and ys[0] <= v <= ys[1]
            xs = sorted([cx, cx + dx * t]); ys = sorted([cy, cy + dy * L])
            w = xs[0] <= u <= xs[1] and ys[0] <= v <= ys[1]
            return h or w
        if bracket(lo, lo, 1, 1) or bracket(hi, lo, -1, 1) or bracket(lo, hi, 1, -1) or bracket(hi, hi, -1, -1):
            return True
        return (u - 11) ** 2 + (v - 11) ** 2 <= 3.6 ** 2
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            c = sum(inside(x * ss + i + 0.5, y * ss + j + 0.5) for i in range(ss) for j in range(ss))
            row += bytes([0, 0, 0, round(255 * c / (ss * ss))])
        rows.append(bytes(row))
    raw = b"".join(rows)
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "icons")
for size, name in [(22, "tray-template.png"), (44, "tray-template@2x.png")]:
    with open(os.path.join(root, name), "wb") as f:
        f.write(render(size))
print("ok")
