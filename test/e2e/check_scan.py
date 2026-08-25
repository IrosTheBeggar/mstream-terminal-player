#!/usr/bin/env python3
"""Assert the scan widget's states from a captured wizard stream.

Frame-replays the stream at many byte slices (in-place line rewrites
emit only changed cells, so stream-grep lies about later states) and
requires: a percentage state, the queued suffix, and completion.
Usage: check_scan.py STREAM REPLAY_PY
"""
import re
import subprocess
import sys

stream, replay = sys.argv[1], sys.argv[2]
raw = open(stream, "rb").read()
seen = set()
for i in range(10, 1001, 10):
    cut = raw[: len(raw) * i // 1000]
    j = cut.rfind(b"\x1b[?1049l")
    if 0 < j < len(cut):
        cut = cut[:j]
    open(stream + ".cut", "wb").write(cut)
    out = subprocess.run(
        ["python3", replay, stream + ".cut", "40", "100"], capture_output=True, text=True
    ).stdout
    if re.search(r"[▰▱]+ \d+%", out) and "tracks so far" in out:
        seen.add("pct")
    if "▰" in out and "▱" in out:
        seen.add("bar")
    if "more queued" in out:
        seen.add("queued")
    if "Generating waveforms" in out or "Downloading album art" in out:
        seen.add("enrich")
    if "Library scan complete." in out:
        seen.add("complete")
need = {"pct", "queued", "complete", "bar", "enrich"}
missing = need - seen
print("scan widget states:", "ALL PASS" if not missing else f"MISSING {sorted(missing)}")
sys.exit(0 if not missing else 1)
