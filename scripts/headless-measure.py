#!/usr/bin/env python3
"""Measures the hover highlights of the account and power buttons in the shots
of scripts/headless-look.sh (standard library and ImageMagick's convert only).

  headless-measure.py avatar <shot> [picture]
      the avatar disc: its centre and size, in pixels (found by its colour:
      the initials' disc, or with "picture" the test picture's).
  headless-measure.py highlight <plain shot> <hovered shot> <cx> <cy> <radius>
      the hover highlight around (cx, cy): the bounding box of what changed
      between the two shots inside that radius, with the pixels of the picture
      itself left out (they do not change), and its centre.
  headless-measure.py centred <plain shot> <hovered shot> <cx> <cy> <radius> [picture]
      the sub-pixel offset of the highlight's centre from the picture's: both
      are centres of mass of their discs (edge pixels count by how much of
      them is covered), so a pixel of asymmetry shows as half a pixel and a
      tenth of a pixel is readable. The picture is found by its colour, the
      highlight (picture included) by what changed between the two shots.
  headless-measure.py glyph <plain shot> <cx> <cy> <radius>
      the dark glyph (the power icon) near (cx, cy): bounding box and centre.
  headless-measure.py alpha <over black> <over white> <x> <y>
      the panel's opacity at (x, y), from the same panel over a black and over
      a white backdrop: 1 - (white - black) / 255, and its colour over each.
  headless-measure.py contrast <over black> <over white> <x> <y> <r,g,b> <text alpha>
      WCAG contrast of text of that colour and opacity over the panel at
      (x, y), on the worst backdrops (black and white), and the least of the two.

All numbers are in physical pixels of the screenshot, so at scale 1.7 a
logical pixel is 1.7 of them. Centres are half-pixel exact: (min + max + 1) / 2.
"""
import subprocess
import sys

AVATAR = (0x0E, 0x74, 0x82)  # the colour TelamonAvatar picks for the name used by the runs
PICTURE = (0xE8, 0x59, 0x0C)  # the flat colour of the test picture (headless-look.sh, "picture")


def load(path):
    head = subprocess.run(["identify", "-format", "%w %h", path], check=True, capture_output=True, text=True).stdout.split()
    w, h = int(head[0]), int(head[1])
    raw = subprocess.run(["convert", path, "-alpha", "off", "-depth", "8", "rgb:-"], check=True, capture_output=True).stdout
    return w, h, raw


def px(img, x, y):
    w, _, raw = img
    i = (y * w + x) * 3
    return raw[i], raw[i + 1], raw[i + 2]


def bbox(points):
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    return min(xs), min(ys), max(xs), max(ys)


def describe(label, box):
    x0, y0, x1, y1 = box
    print(f"{label}: x {x0}..{x1} y {y0}..{y1} size {x1 - x0 + 1}x{y1 - y0 + 1} centre {(x0 + x1 + 1) / 2:.1f},{(y0 + y1 + 1) / 2:.1f}")
    return (x0 + x1 + 1) / 2, (y0 + y1 + 1) / 2


def main():
    cmd = sys.argv[1]
    if cmd == "avatar":
        img = load(sys.argv[2])
        colour = PICTURE if len(sys.argv) > 3 and sys.argv[3] == "picture" else AVATAR
        w, h, _ = img
        pts = []
        # The header sits in the top part of the screen's panel, right half.
        for y in range(h // 4, h * 3 // 5):
            for x in range(w // 2, w):
                r, g, b = px(img, x, y)
                if abs(r - colour[0]) < 14 and abs(g - colour[1]) < 14 and abs(b - colour[2]) < 14:
                    pts.append((x, y))
        if not pts:
            sys.exit("no avatar found")
        cx, cy = describe("avatar", bbox(pts))
        print(f"AVATAR {cx:.1f} {cy:.1f}")
    elif cmd == "highlight":
        a, b = load(sys.argv[2]), load(sys.argv[3])
        cx, cy, rad = float(sys.argv[4]), float(sys.argv[5]), int(sys.argv[6])
        pts = []
        for y in range(int(cy) - rad, int(cy) + rad + 1):
            for x in range(int(cx) - rad, int(cx) + rad + 1):
                p, q = px(a, x, y), px(b, x, y)
                if max(abs(p[i] - q[i]) for i in range(3)) >= 3:
                    pts.append((x, y))
        if not pts:
            sys.exit("nothing changed")
        hx, hy = describe("highlight", bbox(pts))
        print(f"OFFSET {hx - cx:+.1f} {hy - cy:+.1f}")
    elif cmd == "centred":
        a, b = load(sys.argv[2]), load(sys.argv[3])
        cx, cy, rad = float(sys.argv[4]), float(sys.argv[5]), int(sys.argv[6])
        colour = PICTURE if len(sys.argv) > 7 and sys.argv[7] == "picture" else AVATAR
        bg = px(a, int(cx) - rad, int(cy) - rad)
        span = max(1.0, sum((bg[i] - colour[i]) ** 2 for i in range(3)) ** 0.5)
        cells = []
        for y in range(int(cy) - rad, int(cy) + rad + 1):
            for x in range(int(cx) - rad, int(cx) + rad + 1):
                p, q = px(a, x, y), px(b, x, y)
                near = sum((q[i] - colour[i]) ** 2 for i in range(3)) ** 0.5
                wa = min(1.0, max(0.0, 1 - near / span))
                cells.append((x, y, wa, max(abs(p[i] - q[i]) for i in range(3))))
        diffs = sorted(c[3] for c in cells if c[2] < 0.05 and c[3] > 0)
        if not diffs:
            sys.exit("nothing changed")
        plateau = diffs[int(len(diffs) * 0.9)]
        sa = sx = sy = ha = hx = hy = 0.0
        for x, y, wa, d in cells:
            wh = 1.0 if wa >= 0.5 else min(1.0, d / plateau)
            wh = max(wh, wa)
            sa += wa; sx += wa * (x + 0.5); sy += wa * (y + 0.5)
            ha += wh; hx += wh * (x + 0.5); hy += wh * (y + 0.5)
        print(f"picture centre {sx / sa:.2f},{sy / sa:.2f} (area {sa:.0f} px), highlight centre {hx / ha:.2f},{hy / ha:.2f} (area {ha:.0f} px)")
        print(f"CENTRED {hx / ha - sx / sa:+.2f} {hy / ha - sy / sa:+.2f}")
    elif cmd == "glyph":
        a = load(sys.argv[2])
        cx, cy, rad = float(sys.argv[3]), float(sys.argv[4]), int(sys.argv[5])
        pts = []
        for y in range(int(cy) - rad, int(cy) + rad + 1):
            for x in range(int(cx) - rad, int(cx) + rad + 1):
                r, g, bl = px(a, x, y)
                if (r + g + bl) / 3 < 120:
                    pts.append((x, y))
        if not pts:
            sys.exit("no glyph")
        gx, gy = describe("glyph", bbox(pts))
        print(f"GLYPH {gx:.1f} {gy:.1f}")
    elif cmd in ("alpha", "contrast"):
        black, white = load(sys.argv[2]), load(sys.argv[3])
        x, y = int(sys.argv[4]), int(sys.argv[5])
        cb, cw = px(black, x, y), px(white, x, y)
        a = 1 - sum(cw[i] - cb[i] for i in range(3)) / (3 * 255)
        if cmd == "alpha":
            print(f"alpha {a:.3f} over black {cb} over white {cw}")
        else:
            fg = [int(v) for v in sys.argv[6].split(",")]
            ta = float(sys.argv[7])

            def lum(c):
                def lin(v):
                    v /= 255
                    return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4
                return 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])

            def ratio(bg):
                text = [ta * fg[i] + (1 - ta) * bg[i] for i in range(3)]
                hi, lo = sorted((lum(text), lum(bg)), reverse=True)
                return (hi + 0.05) / (lo + 0.05)

            rb, rw = ratio(cb), ratio(cw)
            print(f"contrast over black {rb:.2f} over white {rw:.2f} worst {min(rb, rw):.2f}")
    else:
        sys.exit(__doc__)


main()
