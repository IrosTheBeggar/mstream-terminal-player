#!/usr/bin/env python3
"""Replay a raw pty stream into a virtual cell grid; print the final frame."""
import re, sys

rows, cols = int(sys.argv[2]), int(sys.argv[3])
raw = open(sys.argv[1], "rb").read().decode("utf-8", "replace")
grid = [[" "] * cols for _ in range(rows)]
r = c = 0
i = 0
pat = re.compile(
    r"\x1b\[(?P<p>[0-9;?]*)(?P<f>[A-Za-z])|\x1b\](?P<osc>[^\x07\x1b]*)(\x07|\x1b\\)|(?P<esc>\x1b.)|(?P<ch>[^\x1b])",
    re.S,
)
for m in pat.finditer(raw):
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
for line in grid:
    print("".join(line).rstrip())
