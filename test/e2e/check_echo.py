#!/usr/bin/env python3
"""Assert that Apple Terminal's release echo acts on nothing, from a
captured GUI stream (the gui-discover-echo leg; docs/ui-kit.md, behavioral
rules; docs/ux-contracts/discovery-page.md, clauses 48, 102 and 103).

The leg writes three marks into the stream as it goes (@@ECHO:list@@,
@@ECHO:sheet@@, @@ECHO:queue@@), each after the echo before it has been
sent and drained. The marks are cut out, and the stream is frame-replayed
at every synchronized-update end (ratatui's diff renderer emits only
changed cells, so stream-grep lies about anything that repaints in place)
and at each mark. It requires: the hovered tile's `[⋯]` at its artist
line's end; from the first frame that shows the tile's list to the first
mark, every frame showing it — the echo never closed it, not even for a
frame; from the first frame
that shows the sheet to the second mark, every frame showing it; at the
third mark the queue panel empty and nothing playing — the echo pressed
nothing either time.
Usage: check_echo.py STREAM REPLAY_PY ROWS COLS
"""
import subprocess
import sys

stream, replay, rows, cols = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
raw = open(stream, "rb").read()
MARKS = [b"@@ECHO:list@@", b"@@ECHO:sheet@@", b"@@ECHO:queue@@"]

# Cut the marks (and the newline puts adds) out, remembering where each was.
clean = b""
marks = []
rest = raw
for mark in MARKS:
    at = rest.find(mark)
    if at < 0:
        print(f"echo states: MISSING mark {mark.decode()}")
        sys.exit(1)
    clean += rest[:at]
    marks.append(len(clean))
    rest = rest[at + len(mark):]
    if rest.startswith(b"\r\n"):
        rest = rest[2:]
    elif rest.startswith(b"\n"):
        rest = rest[1:]
clean += rest

END = b"\x1b[?2026l"
ends = []
at = clean.find(END)
while at >= 0:
    ends.append(at + len(END))
    at = clean.find(END, at + 1)


def frame(n):
    cut = clean[:n]
    j = cut.rfind(b"\x1b[?1049l")
    if 0 < j < len(cut):
        cut = cut[:j]
    open(stream + ".cut", "wb").write(cut)
    out = subprocess.run(["python3", replay, stream + ".cut", rows, cols], capture_output=True, text=True).stdout
    return out.split("\n")


def tile_dots(lines):
    """The Lanterns tile's artist line ends in its `[⋯]`."""
    for y, line in enumerate(lines[:-1]):
        if line[17:].startswith("Lanterns"):
            return lines[y + 1][26:29] == "[⋯]"
    return False


def tile_list(lines):
    """The tile's list hangs one row under the Lanterns tile, its frame
    from x 16 and its rows inside it. The words alone are not enough: the
    `[⋯]`'s own tooltip ("More: Play album · Queue album · …") carries them
    too, and stays up while the hand rests on a `[⋯]` that opened nothing."""
    for y, line in enumerate(lines[:-4]):
        if line[17:].startswith("Lanterns"):
            return (
                lines[y + 2][16:17] == "╭"
                and lines[y + 3][16:].startswith("│ Play album")
                and lines[y + 4][16:].startswith("│ Queue album")
            )
    return False


def sheet(lines):
    text = "\n".join(lines)
    return "Play now" in text and "Add next" in text and "Add to end of queue" in text


def watch(start, stop, is_open, name, problems):
    """From the first frame in (start, stop] where `is_open` holds, every
    frame up to `stop` — the mark's own included — must hold it too. The
    first frame it holds in, or None."""
    shots = [n for n in ends if start < n <= stop] + [stop]
    opened = None
    for n in shots:
        lines = frame(n)
        if opened is None and is_open(lines):
            opened = n
        elif opened is not None and not is_open(lines):
            problems.append(f"{name} CLOSED by the echo (frame at byte {n}):\n" + "\n".join(lines))
            return None
    if opened is None:
        problems.append(f"{name} never opened")
    return opened


problems = []
if not any(tile_dots(frame(n)) for n in ends if n <= marks[0]):
    problems.append("the hovered tile showed no [⋯] at its artist line's end")
watch(0, marks[0], tile_list, "the tile's list", problems)
watch(marks[0], marks[1], sheet, "the track-actions sheet", problems)
last = "\n".join(frame(marks[2]))
if "(nothing queued yet)" not in last:
    problems.append("the queue is not empty:\n" + last)
if "(nothing playing)" not in last:
    problems.append("something plays:\n" + last)
print("echo states:", "ALL PASS" if not problems else "; ".join(problems))
sys.exit(1 if problems else 0)
