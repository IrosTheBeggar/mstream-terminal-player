#!/usr/bin/env python3
"""Assert Keep a copy by the pointer alone, from a captured GUI stream (the
gui-discover-keep-mouse leg, Apple Terminal's dialect; docs/ui-kit.md,
Modal and behavioral rules; docs/ux-contracts/discovery-page.md, clauses
106–110).

The leg writes a mark into the stream after each step has been drained
(@@KM:open@@ … @@KM:closed@@). The marks are cut out and the stream is
replayed at each (ratatui's diff renderer emits only changed cells, so
stream-grep lies about anything that repaints in place). It requires: the
window open on the album's choice, the tile's list gone; no tooltip of the
room's ripened over the band or the footer, and the room's row under the
window unlit; each click done once and its echo nothing — Every album by
chosen, then Album; the picker open on `d change`; the copy live with its
window still up (the echo landed on its `[ Close ]`); cancelled, Add again
on its action row; the choice back with no picker (the echo landed on `d
change`); and the window closed, nothing playing.
Usage: check_keep_mouse.py STREAM REPLAY_PY ROWS COLS
"""
import subprocess
import sys

stream, replay, rows, cols = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
raw = open(stream, "rb").read()
NAMES = ["open", "place", "footer", "row", "artist", "album", "picker", "live", "cancel", "again", "closed"]
MARKS = [f"@@KM:{name}@@".encode() for name in NAMES]

# Cut the marks (and the newline puts adds) out, remembering where each was.
clean = b""
marks = {}
rest = raw
for name, mark in zip(NAMES, MARKS):
    at = rest.find(mark)
    if at < 0:
        print(f"keep by the pointer: MISSING mark {mark.decode()}")
        sys.exit(1)
    clean += rest[:at]
    marks[name] = len(clean)
    rest = rest[at + len(mark):]
    if rest.startswith(b"\r\n"):
        rest = rest[2:]
    elif rest.startswith(b"\n"):
        rest = rest[1:]
clean += rest


def frame(n):
    cut = clean[:n]
    j = cut.rfind(b"\x1b[?1049l")
    if 0 < j < len(cut):
        cut = cut[:j]
    open(stream + ".cut", "wb").write(cut)
    out = subprocess.run(["python3", replay, stream + ".cut", rows, cols], capture_output=True, text=True).stdout
    return out.split("\n")


def window(lines):
    """The window's rows, its top border's title to its bottom border."""
    for y, line in enumerate(lines):
        if "─ Keep a copy ─" in line:
            for z in range(y + 1, len(lines)):
                if lines[z][19:20] == "╰":
                    return lines[y:z + 1]
    return []


problems = []


def need(name, ok, why):
    if not ok:
        problems.append(f"{name}: {why}:\n" + "\n".join(frame(marks[name])))


def at(name):
    lines = frame(marks[name])
    return lines, "\n".join(window(lines))


lines, win = at("open")
need("open", "(•) Album" in win and "[ Add album ]" in win, "the window on the album's choice")
need("open", "Files go to copies › Attic Band › Lanterns" in win, "its place line")
need("open", "│ Play album" not in "\n".join(lines), "the tile's list gone")
for y in range(6, 6 + len(window(lines))):
    need("open", lines[y][17:19].strip() == "" and lines[y][95:97].strip() == "", f"the band beside row {y}")
for name in ["place", "footer"]:
    lines, win = at(name)
    text = "\n".join(lines)
    for leak in ["shuffle", "queue all", "Add next", "Add to the queue", "Play the album"]:
        need(name, leak not in text, f"a room's tooltip ({leak!r}) over the window")
lines, win = at("row")
need("row", len(lines) > 17 and "Ember" in lines[17] and "[⋯]" not in lines[17] and "[▸]" not in lines[17], "the room's row lit under the pointer")
_, win = at("artist")
need("artist", "(•) Every album by Attic Band" in win and "( ) Album" in win, "Every album by chosen")
need("artist", "one folder per album" in win and "[ Add every album ]" in win, "its place and its primary")
_, win = at("album")
need("album", "(•) Album" in win and "[ Add album ]" in win, "Album chosen again")
lines, _ = at("picker")
need("picker", "Where copies land" in "\n".join(lines), "the picker open on d change")
_, win = at("live")
need("live", "Into copies › Attic Band › Lanterns" in win and "x cancel copy" in win, "the copy live, its window up")
need("live", "Esc close · the copy keeps going" in win, "its footer")
_, win = at("cancel")
need("cancel", "× Cancelled" in win and "[ Add again ]" in win, "cancelled, Add again")
lines, win = at("again")
need("again", "(•) Album" in win and "[ Add album ]" in win, "the choice back")
need("again", "Where copies land" not in "\n".join(lines), "the echo opened the picker")
lines, win = at("closed")
need("closed", win == "", "the window closed")
need("closed", "(nothing playing)" in "\n".join(lines), "nothing plays")
print("keep by the pointer:", "ALL PASS" if not problems else "; ".join(problems))
sys.exit(1 if problems else 0)
