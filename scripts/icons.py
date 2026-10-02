#!/usr/bin/env python3
"""Regenerate the desktop app's icons from assets/mstream-logo.ico.

    python3 scripts/icons.py

Writes the freedesktop PNG set (assets/icons/hicolor/<n>x<n>/apps/
io.mstream.player.png at 16, 32, 48, 64, 128, 256 and 512 px) and the
macOS icon (assets/icons/io.mstream.player.icns). The .ico stays the
Windows source: build.rs embeds it in the exe as it is.

Runs on a Mac with nothing installed: the ICO's frames are decoded here with
the standard library alone (they are 32-bit BMPs, no PNG frame among them),
`sips` scales the two sizes the ICO lacks and `iconutil` packs the iconset.
The ICO's largest frame is 128 px, so 256 and 512 are that frame upscaled;
every smaller size is the ICO's own frame, drawn for that size, not a
downscale. Replace the .ico with one carrying a 256 px (or larger) frame and
this script takes the largest frame as the upscale source on its own.
"""

import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICO = os.path.join(ROOT, "assets", "mstream-logo.ico")
ICONS = os.path.join(ROOT, "assets", "icons")
APP_ID = "io.mstream.player"  # src/identity.rs's APP_ID; a test keeps them equal.
SIZES = (16, 32, 48, 64, 128, 256, 512)
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


def frames(data):
    """The ICO's frames as {size: png bytes}, each decoded to an RGBA PNG."""
    reserved, kind, count = struct.unpack_from("<HHH", data, 0)
    if reserved != 0 or kind != 1:
        sys.exit(f"{ICO} is not an icon file")
    out = {}
    for i in range(count):
        w, h, _, _, _, _, size, offset = struct.unpack_from("<BBBBHHII", data, 6 + 16 * i)
        w, h = w or 256, h or 256
        body = data[offset : offset + size]
        if w != h:
            continue
        out[w] = body if body.startswith(PNG_MAGIC) else bmp_to_png(body)
    return out


def bmp_to_png(body):
    """A 32-bit ICO BMP frame (BITMAPINFOHEADER, height doubled for the AND
    mask, rows bottom-up, BGRA) as a PNG. The alpha channel is the frame's
    own; the 1-bit AND mask after the pixels only matters below 32 bits."""
    header, width, height2, _, bpp, compression = struct.unpack_from("<IiiHHI", body, 0)
    if bpp != 32 or compression != 0:
        sys.exit(f"an ICO frame is {bpp}-bit (compression {compression}); only 32-bit is handled")
    height = height2 // 2
    pixels = body[header : header + width * height * 4]
    rows = []
    for y in range(height):
        start = (height - 1 - y) * width * 4
        bgra = pixels[start : start + width * 4]
        rgba = bytearray(len(bgra))
        rgba[0::4], rgba[1::4], rgba[2::4], rgba[3::4] = bgra[2::4], bgra[1::4], bgra[0::4], bgra[3::4]
        rows.append(b"\0" + bytes(rgba))
    return png(width, height, b"".join(rows))


def png(width, height, raw):
    def chunk(kind, payload):
        crc = zlib.crc32(kind + payload) & 0xFFFFFFFF
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return PNG_MAGIC + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def scaled(source, size, path):
    """`source` (a PNG path) resampled by sips to size x size at `path`."""
    subprocess.run(
        ["sips", "-z", str(size), str(size), source, "--out", path],
        check=True,
        stdout=subprocess.DEVNULL,
    )


def main():
    for tool in ("sips", "iconutil"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found: this script runs on macOS")
    with open(ICO, "rb") as f:
        found = frames(f.read())
    largest = max(found)
    with tempfile.TemporaryDirectory() as tmp:
        source = os.path.join(tmp, f"source-{largest}.png")
        with open(source, "wb") as f:
            f.write(found[largest])
        made = {}
        for size in SIZES:
            target = os.path.join(ICONS, "hicolor", f"{size}x{size}", "apps", f"{APP_ID}.png")
            os.makedirs(os.path.dirname(target), exist_ok=True)
            if size in found:
                with open(target, "wb") as f:
                    f.write(found[size])
                how = "the ICO's own frame"
            else:
                scaled(source, size, target)
                how = f"{'upscaled' if size > largest else 'scaled'} from {largest} px"
            made[size] = target
            print(f"{size:>4} px  {how}")
        # The iconset names iconutil wants: icon_<n>x<n>.png and its @2x,
        # which is the 2n px image. 512@2x (1024 px) is left out: the ICO
        # has nothing near it, and iconutil packs what is there.
        iconset = os.path.join(tmp, f"{APP_ID}.iconset")
        os.makedirs(iconset)
        for points in (16, 32, 128, 256, 512):
            shutil.copy(made[points], os.path.join(iconset, f"icon_{points}x{points}.png"))
            if points * 2 in made:
                shutil.copy(made[points * 2], os.path.join(iconset, f"icon_{points}x{points}@2x.png"))
        icns = os.path.join(ICONS, f"{APP_ID}.icns")
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", icns], check=True)
        print(f"icns     {os.path.relpath(icns, ROOT)}")


if __name__ == "__main__":
    main()
