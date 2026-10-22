#!/usr/bin/env python3
"""Assert Where files land's states from a captured GUI stream (the
gui-discover-dest leg; docs/ux-contracts/discovery-page.md, clauses 105 and
142-148).

Frame-replays the stream at every synchronized-update end (ratatui's diff
renderer emits only changed cells, so stream-grep lies about anything that
repaints in place) and requires, in order: Keep a copy on the library's
default; the picker on that place; Keep a copy on the saved place, saved for
the account; the start refused for a moved destination — LANDS IN redrawn
from the answer and its one GOLD line.
Usage: check_dest.py STREAM REPLAY_PY ROWS COLS
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


steps = ["default", "picker", "saved", "moved"]
seen = []
for n in ends:
    text = "\n".join(frame(n))
    keep = "Keep a copy" in text and "LANDS IN" in text
    if keep and "LANDS IN  copies › Attic Band › Lanterns" in text and "the library's default" in text \
            and "[ Change… ]" in text and "where files land changed" not in text:
        seen.append("default")
    if "Where files land" in text and "LIBRARY   ◂ copies ▸" in text and "PREVIEW   copies › Attic Band › Lanterns" in text:
        seen.append("picker")
    if keep and "LANDS IN  copies › from-peers › Attic Band › Lanterns" in text and "saved for your account" in text:
        seen.append("saved")
    if keep and "LANDS IN  copies › Attic Band › Lanterns" in text and "the library's default" in text \
            and "where files land changed · check the path above and add again" in text and "[ Add album ]" in text:
        seen.append("moved")
order = []
for step in seen:
    if not order or order[-1] != step:
        order.append(step)
it = iter(order)
missing = [s for s in steps if s not in it]
print("where files land states:", "ALL PASS" if not missing else f"MISSING {missing} (seen in order: {order})")
sys.exit(0 if not missing else 1)
