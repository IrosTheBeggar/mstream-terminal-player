#!/usr/bin/env python3
"""Assert Keep a copy's states from a captured GUI stream (the
gui-discover-keep leg; docs/ux-contracts/discovery-page.md, clauses
100-141).

Frame-replays the stream at every synchronized-update end (ratatui's diff
renderer emits only changed cells, so stream-grep lies about anything that
repaints in place) and requires, in order: the Keep a copy modal naming
where the copy lands; the room with the copy live — the tile's artist row
a live mark and the verbs line's indicator; the copy kept — the tile's
"✓ kept" and the row's ✓ LEAD; the Copies modal listing it. No frame of
the room may show a number for the copy (clause 126).
Usage: check_keep.py STREAM REPLAY_PY ROWS COLS
"""
import subprocess
import sys

stream, replay, rows, cols = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
raw = open(stream, "rb").read()
END = b"\x1b[?2026l"
ends = []
at = raw.find(END)
while at >= 0:
    ends.append(at + len(END))
    at = raw.find(END, at + 1)
ends.append(len(raw))


def frame(n):
    cut = raw[:n]
    j = cut.rfind(b"\x1b[?1049l")
    if 0 < j < len(cut):
        cut = cut[:j]
    open(stream + ".cut", "wb").write(cut)
    out = subprocess.run(["python3", replay, stream + ".cut", rows, cols], capture_output=True, text=True).stdout
    return out.split("\n")


def tile_word(lines):
    """The tile's artist row: the line under the strip's "Lanterns"."""
    for y, line in enumerate(lines[:-1]):
        if line[17:].startswith("Lanterns"):
            return lines[y + 1][17:29].strip()
    return None


steps = ["modal", "live", "kept", "copies"]
seen = []
numbers = []
for n in ends:
    lines = frame(n)
    text = "\n".join(lines)
    modal = "LANDS IN" in text or "Copies" in text and "TITLE" in text
    if "LANDS IN  copies › Attic Band › Lanterns" in text and "[ Add album ]" in text:
        seen.append("modal")
    word = tile_word(lines)
    verbs = next((l for l in lines if "▸ play · + queue all" in l), "")
    if not modal and word in ("◷ waiting", "copying…") and verbs.rstrip().endswith("copying…"):
        seen.append("live")
    if not modal and word == "✓ kept" and any(l[17:].startswith("✓ Ember") for l in lines):
        seen.append("kept")
    copy_row = next((y for y, l in enumerate(lines) if "Lanterns" in l and "Attic" in l and "✓ in your collection" in l), None)
    if "TITLE" in text and "FROM" in text and "STATE" in text and copy_row is not None:
        if "│   album · 2 copied" in lines[copy_row + 1]:
            seen.append("copies")
    if not modal and word is not None and any(c.isdigit() for c in word):
        numbers.append(word)
# In order: each step after the one before it.
order = []
for step in seen:
    if not order or order[-1] != step:
        order.append(step)
it = iter(order)
missing = [s for s in steps if s not in it]
problems = []
if missing:
    problems.append(f"MISSING {missing} (seen in order: {order})")
if numbers:
    problems.append(f"NUMBERS on the tile: {numbers}")
print("keep a copy states:", "ALL PASS" if not problems else "; ".join(problems))
sys.exit(0 if not problems else 1)
