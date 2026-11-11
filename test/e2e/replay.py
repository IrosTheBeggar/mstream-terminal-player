#!/usr/bin/env python3
"""Replay a raw pty stream into a virtual cell grid; print the final frame.

    replay.py STREAM ROWS COLS

The checkers import it instead: `frames(raw, cuts, rows, cols)` is the
frame of every prefix they ask for, from ONE pass over the stream. They
used to write each prefix to a file and run this script on it — a process
and a replay from byte 0 per frame, quadratic in the stream: a GUI leg's
few thousand frames took minutes (performance audit). A frame from
`frames` is the same list of lines that run printed, `split("\n")`.
"""
import bisect
import re
import sys

PAT = re.compile(
    r"\x1b\[(?P<p>[0-9;?]*)(?P<f>[A-Za-z])|\x1b\](?P<osc>[^\x07\x1b]*)(\x07|\x1b\\)|(?P<esc>\x1b.)|(?P<ch>[^\x1b])",
    re.S,
)
# Leaving the alternate screen: what the stream says after it is the
# shell's, not the app's, so a prefix that holds one is cut there.
LEAVE = b"\x1b[?1049l"


class Screen:
    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.grid = [[" "] * cols for _ in range(rows)]
        self.r = self.c = 0

    def copy(self):
        other = Screen.__new__(Screen)
        other.rows, other.cols, other.r, other.c = self.rows, self.cols, self.r, self.c
        other.grid = [row[:] for row in self.grid]
        return other

    def feed(self, text):
        """Draw `text`. True when it ends between whole sequences — nothing
        cut short at its end, which the rest of the stream would have
        finished — so text fed after it parses as the two would together."""
        rows, cols, grid, r, c = self.rows, self.cols, self.grid, self.r, self.c
        end, last_esc = 0, None
        for m in PAT.finditer(text):
            end = m.end()
            if text[m.start()] == "\x1b":
                last_esc = m
            if m.group("f"):
                p, f = m.group("p"), m.group("f")
                nums = [int(x) for x in p.replace("?", "").split(";") if x.isdigit()]
                if f == "H":
                    r = (nums[0] - 1) if nums else 0
                    c = (nums[1] - 1) if len(nums) > 1 else 0
                elif f == "J" and (not nums or nums[0] in (0, 2, 3)):
                    grid = [[" "] * cols for _ in range(rows)]
                elif f == "K":
                    for x in range(c, cols):
                        grid[min(r, rows - 1)][x] = " "
            elif m.group("ch"):
                ch = m.group("ch")
                if ch == "\n":
                    r += 1
                elif ch == "\r":
                    c = 0
                elif ch >= " ":
                    if r < rows and c < cols:
                        grid[r][c] = ch
                    c += 1
        self.grid, self.r, self.c = grid, r, c
        # Only the end can be cut short: a lone ESC matches nothing, and a
        # CSI or OSC missing its end parses as a bare `ESC [` / `ESC ]`.
        cut_short = last_esc is not None and last_esc.group("esc") is not None and last_esc.group("esc")[1] in "[]"
        return end == len(text) and not cut_short

    def lines(self):
        return ["".join(row).rstrip() for row in self.grid]


def render(data, rows, cols):
    """The final frame of `data` (bytes), a line a row."""
    screen = Screen(rows, cols)
    screen.feed(data.decode("utf-8", "replace"))
    return screen.lines()


def frames(raw, cuts, rows, cols):
    """{n: the frame of raw[:n]} for every n in `cuts` — cut where the
    prefix last left the alternate screen, if it has — each as the list
    replay.py's output split on "\\n" gives (the rows, then "").

    One pass: the screen is carried from cut to cut. A cut is only built on
    where the text before it ends whole (Screen.feed) and on an ASCII byte
    (no character split), so every frame is the one a replay of its whole
    prefix draws; a cut that is not such a place is drawn on a copy, from
    the last one that was."""
    leaves = []
    at = raw.find(LEAVE)
    while at >= 0:
        leaves.append(at)
        at = raw.find(LEAVE, at + 1)

    def stop(n):
        # raw[:n].rfind(LEAVE), from the list: the last leave wholly inside.
        k = bisect.bisect_right(leaves, n - len(LEAVE)) - 1
        j = leaves[k] if k >= 0 else -1
        return j if j > 0 else n

    def settled(text_end, ok):
        return ok and (text_end == 0 or raw[text_end - 1] < 0x80)

    wanted = {n: stop(n) for n in cuts}
    screen, base, drawn = Screen(rows, cols), 0, {}
    for m in sorted(set(wanted.values())):
        trial = screen.copy()
        whole = trial.feed(raw[base:m].decode("utf-8", "replace"))
        drawn[m] = trial.lines() + [""]
        if settled(m, whole):
            screen, base = trial, m
            continue
        # Not a place to build on: try the last ESC before it, where the
        # sequence that was cut short began.
        k = raw.rfind(b"\x1b", base, m)
        if k > base:
            trial = screen.copy()
            if settled(k, trial.feed(raw[base:k].decode("utf-8", "replace"))):
                screen, base = trial, k
    return {n: drawn[m] for n, m in wanted.items()}


if __name__ == "__main__":
    rows, cols = int(sys.argv[2]), int(sys.argv[3])
    for line in render(open(sys.argv[1], "rb").read(), rows, cols):
        print(line)
