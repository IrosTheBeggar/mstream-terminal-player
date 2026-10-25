#!/usr/bin/env python3
"""Assert Where copies land's states from a captured GUI stream (the
gui-discover-dest leg; docs/ux-contracts/discovery-page.md, clauses 105 and
142-148).

Frame-replays the stream at every synchronized-update end (ratatui's diff
renderer emits only changed cells, so stream-grep lies about anything that
repaints in place) and requires, in order: Keep a copy's place line on the
place seeded with a base folder (older data), "d change" at its end; the
picker on that place folded into Custom — its title on the top border,
"Esc close" on the bottom, its first row Keep a copy's words for the same
line ("Files go to …", the path as the server holds it), the libraries as
choices with no label, "Same folders as the server" under the Server
preset (the fake's variables list SERVERFOLDER), "Start from an existing
folder…" above the field, and none of the words the trim of 2026-10-24
took out (the header's tag and sub, "lands at", LIBRARY and LAYOUT, the
presets' layouts, "Type your own path", the help line); the list open on
the library root, the folder the layout starts in not there yet, by its
name; a folder picked from the server's own list starting Custom's
layout, the list gone inside it; Keep a copy's place line on the saved
place; the start refused for a moved destination — the place line redrawn
from the answer, its one GOLD line under it and Add album still the
primary.
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


def below(text, upper, lower):
    """`lower` on the row right under `upper`'s."""
    rows = text.split("\n")
    return any(upper in row and lower in under for row, under in zip(rows, rows[1:]))


def picker_box(text):
    """The picker's own cells (x 10-89), from its top border to its bottom."""
    rows = text.split("\n")
    top = next((y for y, row in enumerate(rows) if "─ Where copies land ─" in row), None)
    if top is None:
        return ""
    bottom = next((y for y in range(top + 1, len(rows)) if rows[y][10:11] == "╰"), len(rows) - 1)
    return "\n".join(row[10:90] for row in rows[top:bottom + 1])


def beside(text, left, right):
    """`right` on `left`'s own row, after it."""
    return any(left in row and right in row[row.index(left) + len(left):] for row in text.split("\n"))


steps = ["seeded", "picker", "listed", "picked", "saved", "moved"]
# The words the trim took out of the picker (docs/ux-contracts/
# discovery-page.md, deviations 2026-10-24, "Where copies land, trimmed").
CUT = ["· yours", "· library default", "[X]", "One setting for your account", "lands at", "LIBRARY", "LAYOUT",
       "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}", "{{SERVERFOLDER}}", "Type your own path", "Use / between folders",
       "· 1 folder", "does not exist yet"]
seen = []
for n in ends:
    text = "\n".join(frame(n))
    keep = "─ Keep a copy ─" in text and "Files go to" in text
    if keep and beside(text, "Files go to copies › From friends › Attic Band › Lanterns", "d change") \
            and "Where files land changed" not in text:
        seen.append("seeded")
    picker = "─ Where copies land ─" in text and beside(text, "(•) copies", "( ) music") and "─ Esc close ─╯" in text
    if picker and below(text, "─ Where copies land ─", "│ Files go to copies › From friends › Attic Band › Lanterns") \
            and below(text, "( ) Server / Artist / Album", "( ) Same folders as the server") \
            and below(text, "(•) Custom", "Start from an existing folder…") \
            and below(text, "Start from an existing folder…", "From friends/{{ALBUMARTIST}}/{{ALBUM}}") \
            and not any(words in picker_box(text) for words in CUT):
        seen.append("picker")
    if picker and "Hide folders" in text and below(text, "│   copies   ", "│   from-peers   ") \
            and below(text, "from-peers", "“From friends” isn't there yet · the first file creates it") \
            and "From friends/{{ALBUMARTIST}}/{{ALBUM}}" in text:
        seen.append("listed")
    if picker and "(•) Custom" in text and "from-peers/{{ALBUMARTIST}}/{{ALBUM}}" in text and "Hide folders" in text \
            and "Files go to copies › from-peers › Attic Band › Lanterns" in text and "Up to copies" in text:
        seen.append("picked")
    if keep and beside(text, "Files go to copies › from-peers › Attic Band › Lanterns", "d change"):
        seen.append("saved")
    if keep and below(text, "Files go to copies › Attic Band › Lanterns", "! Where files land changed · check the path above and add again") \
            and "[ Add album ]" in text:
        seen.append("moved")
order = []
for step in seen:
    if not order or order[-1] != step:
        order.append(step)
it = iter(order)
missing = [s for s in steps if s not in it]
print("where copies land states:", "ALL PASS" if not missing else f"MISSING {missing} (seen in order: {order})")
sys.exit(0 if not missing else 1)
