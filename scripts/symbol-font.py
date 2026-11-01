#!/usr/bin/env python3
"""Regenerate the window's bundled symbol face, assets/fonts/mStreamSymbols-Regular.ttf.

    python3 scripts/symbol-font.py

The GUI draws a few glyphs that Hack, the window's face, does not have (the
census test in src/gui/window/mod.rs keeps the list: the rating stars, the
checkbox tick and the MP3 Player tab's ballot cross). A terminal borrows them from its own font fallback; the window
used to borrow them from whatever the system had (Menlo, DejaVu, Segoe UI
Symbol), so they looked different on every OS and were boxes where none was
found. This face draws them itself, on Hack's metrics (2048 units to the em,
an advance of 1233, ascender 1901, descender -483), so they sit on Hack's
grid at Hack's size wherever the window runs.

The glyphs are plain geometry: a regular five-pointed star, the same star
hollowed by an inward offset, a two-stroke tick and a two-stroke cross, each
at a regular and a heavy weight where Unicode has both. No outline is taken
from another font. Standard library only; the output is reproducible (no
timestamps: head's dates are fixed), so rerunning it writes the same bytes.
"""

import math
import os
import struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "assets", "fonts", "mStreamSymbols-Regular.ttf")

# Hack's metrics, so the face's cell is Hack's cell (the backend sizes a
# face by its ascender-to-descender line, and measures a face's width by its
# advance).
UPEM = 2048
ADVANCE = 1233
ASCENDER = 1901
DESCENDER = -483
X_HEIGHT = 1120
CAP_HEIGHT = 1493
FAMILY = "mStream Symbols"
VERSION = "Version 1.000"
# 2026-10-02 in the format's seconds since 1904, fixed so the bytes are too.
WHEN = 3842294400

# A regular stroke the weight of Hack's stems, and a heavy one.
STROKE = 150
HEAVY = 250
# The white star's outline. A star's edges pass close to its centre (181
# units here), so an outline as heavy as a stem would leave only a speck of
# a hole; Menlo's white star is a hairline for the same reason.
OUTLINE = 60


def star(cx, cy, outer, inner, start=90.0):
    """The ten vertices of a five-pointed star, clockwise from its top point."""
    points = []
    for k in range(10):
        radius = outer if k % 2 == 0 else inner
        angle = math.radians(start - 36.0 * k)
        points.append((cx + radius * math.cos(angle), cy + radius * math.sin(angle)))
    return points


def edge_distance(points, cx, cy):
    """How far the star's edges are from its centre. Every edge of a regular
    star is the same distance away, so an inward offset of the outline is
    the star scaled about its centre."""
    (x1, y1), (x2, y2) = points[0], points[1]
    return abs((x2 - x1) * (y1 - cy) - (x1 - cx) * (y2 - y1)) / math.hypot(x2 - x1, y2 - y1)


def intersect(p, d, q, e):
    """Where the line through p along d meets the line through q along e."""
    det = d[0] * e[1] - d[1] * e[0]
    t = ((q[0] - p[0]) * e[1] - (q[1] - p[1]) * e[0]) / det
    return (p[0] + t * d[0], p[1] + t * d[1])


def stroke(points, width):
    """A polyline drawn with a pen of `width`: square-cut ends, mitred joins,
    as one closed outline (the left side out, the right side back)."""
    half = width / 2.0
    dirs = []
    for (x1, y1), (x2, y2) in zip(points, points[1:]):
        length = math.hypot(x2 - x1, y2 - y1)
        dirs.append(((x2 - x1) / length, (y2 - y1) / length))
    normals = [(-dy, dx) for dx, dy in dirs]

    def side(sign):
        out = []
        p0, n0 = points[0], normals[0]
        out.append((p0[0] + sign * half * n0[0], p0[1] + sign * half * n0[1]))
        for i in range(1, len(points) - 1):
            a, b = normals[i - 1], normals[i]
            pa = (points[i][0] + sign * half * a[0], points[i][1] + sign * half * a[1])
            pb = (points[i][0] + sign * half * b[0], points[i][1] + sign * half * b[1])
            out.append(intersect(pa, dirs[i - 1], pb, dirs[i]))
        pn, nn = points[-1], normals[-1]
        out.append((pn[0] + sign * half * nn[0], pn[1] + sign * half * nn[1]))
        return out

    return side(1) + side(-1)[::-1]


def cross(cx, cy, size, width):
    """A saltire: two bars of `width` crossing at (cx, cy), as one outline (no
    overlapping contours, so any fill rule draws it the same)."""
    half = width / 2.0
    corners = []
    # The bars' four arms, each a pair of edge points at its end and the
    # inner corner where it meets the next arm.
    arms = [(1, 1), (1, -1), (-1, -1), (-1, 1)]
    for i, (sx, sy) in enumerate(arms):
        d = (sx / math.sqrt(2), sy / math.sqrt(2))
        n = (-d[1], d[0])
        tip = (cx + sx * size / 2.0, cy + sy * size / 2.0)
        left = (tip[0] + half * n[0], tip[1] + half * n[1])
        right = (tip[0] - half * n[0], tip[1] - half * n[1])
        corners.append((left, right, d, n))
    outline = []
    for i in range(4):
        left, right, d, n = corners[i]
        nleft, _, nd, nn = corners[(i + 1) % 4]
        outline += [left, right]
        # The inner corner between this arm's right edge and the next arm's
        # left edge.
        outline.append(intersect(right, d, nleft, nd))
    return outline


def area(points):
    return sum(x1 * y2 - x2 * y1 for (x1, y1), (x2, y2) in zip(points, points[1:] + points[:1])) / 2


def clockwise(points):
    """TrueType fills clockwise contours and cuts holes with counter-clockwise
    ones (y up)."""
    return points if area(points) < 0 else points[::-1]


def anticlockwise(points):
    return points[::-1] if area(points) < 0 else points


def glyphs():
    """Each glyph's code point and contours, in font units."""
    # The star fills the advance less Menlo's side bearings (58 each side) and
    # stands on the baseline, as Menlo's and DejaVu's do.
    outer = (ADVANCE - 2 * 58) / (2 * math.sin(math.radians(72)))
    inner = outer * math.sin(math.radians(18)) / math.sin(math.radians(54))
    cx = ADVANCE / 2.0
    cy = 15 + outer * math.cos(math.radians(36))
    solid = star(cx, cy, outer, inner)
    hollow = star(cx, cy, outer, inner)
    h = edge_distance(hollow, cx, cy)
    k = (h - OUTLINE) / h
    hole = [(cx + (x - cx) * k, cy + (y - cy) * k) for x, y in hollow]
    # The tick: a short arm down to the left of centre, a long arm up to the
    # cap height, in the box Menlo's takes (x 153..1080, y 192..1148).
    tick = [(200, 640), (470, 300), (1040, 1100)]
    out = {
        0x2605: [clockwise(solid)],  # ★ BLACK STAR
        0x2606: [clockwise(hollow), anticlockwise(hole)],  # ☆ WHITE STAR
        0x2713: [clockwise(stroke(tick, STROKE))],  # ✓ CHECK MARK
        0x2714: [clockwise(stroke(tick, HEAVY))],  # ✔ HEAVY CHECK MARK
        0x2717: [clockwise(cross(cx, 620, 860, STROKE))],  # ✗ BALLOT X
        0x2718: [clockwise(cross(cx, 620, 860, HEAVY))],  # ✘ HEAVY BALLOT X
    }
    return {
        cp: [[(round(x), round(y)) for x, y in c] for c in contours]
        for cp, contours in out.items()
    }


def glyf_entry(contours):
    """One simple glyph: every point on the curve, coordinates as words."""
    if not contours:
        return b""
    xs = [x for c in contours for x, _ in c]
    ys = [y for c in contours for _, y in c]
    data = struct.pack(">hhhhh", len(contours), min(xs), min(ys), max(xs), max(ys))
    end = -1
    for c in contours:
        end += len(c)
        data += struct.pack(">H", end)
    data += struct.pack(">H", 0)  # no instructions
    points = [p for c in contours for p in c]
    data += bytes([0x01] * len(points))  # on curve; x and y are signed words
    last = 0
    for x, _ in points:
        data += struct.pack(">h", x - last)
        last = x
    last = 0
    for _, y in points:
        data += struct.pack(">h", y - last)
        last = y
    return data + b"\0" * (-len(data) % 4)


def name_table():
    records = [
        (0, "Copyright 2026 the mStream Terminal Player authors"),
        (1, FAMILY),
        (2, "Regular"),
        (3, FAMILY + " Regular 1.000"),
        (4, FAMILY + " Regular"),
        (5, VERSION),
        (6, "mStreamSymbols-Regular"),
        (13, "This Font Software is licensed under the SIL Open Font License, Version 1.1."),
        (14, "https://openfontlicense.org"),
    ]
    strings = b""
    entries = b""
    for name_id, text in records:
        encoded = text.encode("utf-16-be")
        entries += struct.pack(">HHHHHH", 3, 1, 0x409, name_id, len(encoded), len(strings))
        strings += encoded
    header = struct.pack(">HHH", 0, len(records), 6 + 12 * len(records))
    return header + entries + strings


def cmap_table(mapping):
    """Format 4: one segment per code point, then the closing 0xFFFF."""
    codes = sorted(mapping)
    ends = codes + [0xFFFF]
    starts = codes + [0xFFFF]
    deltas = [(mapping[c] - c) & 0xFFFF for c in codes] + [1]
    seg = len(ends)
    search = 2 ** int(math.log2(seg))
    sub = struct.pack(
        ">HHHHHHH", 4, 0, 0, seg * 2, search * 2, int(math.log2(search)), seg * 2 - search * 2
    )
    sub += struct.pack(f">{seg}H", *ends) + b"\0\0" + struct.pack(f">{seg}H", *starts)
    sub += struct.pack(f">{seg}H", *deltas) + struct.pack(f">{seg}H", *([0] * seg))
    sub = sub[:2] + struct.pack(">H", len(sub)) + sub[4:]
    return struct.pack(">HHHHI", 0, 1, 3, 1, 12) + sub


def checksum(data):
    data += b"\0" * (-len(data) % 4)
    return sum(struct.unpack(f">{len(data) // 4}I", data)) & 0xFFFFFFFF


def build():
    shapes = glyphs()
    codes = sorted(shapes)
    order = [[]] + [shapes[c] for c in codes]  # glyph 0 is .notdef, empty
    mapping = {c: i + 1 for i, c in enumerate(codes)}

    glyf = b""
    loca = []
    for contours in order:
        loca.append(len(glyf))
        glyf += glyf_entry(contours)
    loca.append(len(glyf))
    xs = [x for g in order for c in g for x, _ in c]
    ys = [y for g in order for c in g for _, y in c]
    bbox = (min(xs), min(ys), max(xs), max(ys))
    points = max(sum(len(c) for c in g) for g in order)
    contours = max(len(g) for g in order)

    head = struct.pack(
        ">IIIIHHQQhhhhHHhhh",
        0x00010000, 0x00010000, 0, 0x5F0F3CF5, 0b1011, UPEM, WHEN, WHEN, *bbox,
        0, 8, 2, 1, 0,
    )
    hhea = struct.pack(
        ">IhhhHhhhhhhhhhhhH",
        0x00010000, ASCENDER, DESCENDER, 0, ADVANCE, 0, 0, ADVANCE, 1, 0, 0,
        0, 0, 0, 0, 0, len(order),
    )
    # Every glyph, .notdef too, advances one cell: a face's width is read
    # from its advance (`Font::new` looks for an `m`, and falls back to
    # .notdef's), so this face's cell is Hack's.
    hmtx = b"".join(
        struct.pack(">Hh", ADVANCE, min((x for c in g for x, _ in c), default=0)) for g in order
    )
    maxp = struct.pack(">IHHHHHHHHHHHHHH", 0x00010000, len(order), points, contours,
                       0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0)
    os2 = struct.pack(
        ">HhHHHhhhhhhhhhhh10sIIIIIIIIHHHhhhHHIIhhHHH",
        4, ADVANCE, 400, 5, 0,
        650, 700, 0, 140, 650, 700, 0, 480, 100, 500, 0,
        bytes([2, 11, 6, 9, 3, 2, 2, 2, 2, 4]),  # PANOSE: monospaced
        0, 0, 0, 0, b"mStr"[0] << 24 | b"mStr"[1] << 16 | b"mStr"[2] << 8 | b"mStr"[3],
        0, 0, 0,
        0x40, codes[0], codes[-1],
        ASCENDER, DESCENDER, 0, ASCENDER, -DESCENDER,
        1, 0,
        X_HEIGHT, CAP_HEIGHT, 0, 0x20, 1,
    )
    post = struct.pack(">IIhhIIIII", 0x00030000, 0, -205, 102, 1, 0, 0, 0, 0)

    tables = {
        "OS/2": os2,
        "cmap": cmap_table(mapping),
        "glyf": glyf,
        "head": head,
        "hhea": hhea,
        "hmtx": hmtx,
        "loca": struct.pack(f">{len(loca)}I", *loca),
        "maxp": maxp,
        "name": name_table(),
        "post": post,
    }
    tags = sorted(tables)
    count = len(tags)
    search = 2 ** int(math.log2(count))
    header = struct.pack(">IHHHH", 0x00010000, count, search * 16, int(math.log2(search)),
                         count * 16 - search * 16)
    offset = 12 + 16 * count
    directory = b""
    body = b""
    for tag in tags:
        data = tables[tag]
        record = (tag.encode(), checksum(data), offset + len(body), len(data))
        directory += struct.pack(">4sIII", *record)
        body += data + b"\0" * (-len(data) % 4)
    font = bytearray(header + directory + body)
    # head's checkSumAdjustment, over the whole font with it zero.
    at = 12 + 16 * tags.index("head")
    head_offset = struct.unpack_from(">I", font, at + 8)[0]
    struct.pack_into(">I", font, head_offset + 8, (0xB1B0AFBA - checksum(bytes(font))) & 0xFFFFFFFF)
    return bytes(font)


if __name__ == "__main__":
    font = build()
    with open(OUT, "wb") as f:
        f.write(font)
    print(f"{OUT}: {len(font)} bytes")
