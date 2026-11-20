#!/usr/bin/env python3
"""Assert the GUI's Peer recs room from a captured stream: the admin leg
(gui-peer-recs, scenario R, Apple Terminal's mouse dialect) or the member
leg (gui-peer-recs-member, scenario R2), against the fake's taste
generation 3 (FAKE_P2P; docs/ux-contracts/peer-recommendations.md).

    check_peer_recs.py admin|member STREAM REPLAY_PY ROWS COLS HOME [CAPTURES]

The leg writes marks into the stream as it goes (@@PR:open@@, …), each with
the fake's /_state at that moment kept beside it as HOME/state-<mark>.json.
The marks are cut out and the stream is replayed at every synchronized
update's end and at each mark (README law 1: the diff renderer emits only
changed cells, so a grep of the raw stream lies about anything repainted in
place). The run's log, HOME/mstream.log, says whether the machine opened an
output device (`output opened on`): with one the previews are asserted,
without one their absence is.

With CAPTURES (run.sh passes E2E_CAPTURES), every mark's decoded frame is
kept there as <mode>-<mark>.txt.

Where a state is told by colour alone (the tabs stop's slab, an idle slot
against a muted one), the frame is replayed once more with each cell's
colours, by `styled` below.
"""
import base64
import json
import os
import re
import sys

mode, stream, replay, rows, cols, home = sys.argv[1:7]
captures = sys.argv[7] if len(sys.argv) > 7 else ""
ROWS, COLS = int(rows), int(cols)
sys.dont_write_bytecode = True  # no __pycache__ left in the tree
sys.path.insert(0, os.path.dirname(os.path.abspath(replay)))
from replay import frames  # noqa: E402

FIXTURES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fixtures", "peer_recs")


def fixture(name):
    with open(os.path.join(FIXTURES, name + ".json"), encoding="utf-8") as f:
        return json.load(f)


TASTE_ALL = fixture("taste_all")
LEADS = TASTE_ALL["network"]["results"]
PEERS = {p["label"]: p["endpointId"] for p in fixture("peers_admin")["peers"]}
BATCH_PINS = [s["filepath"] for s in TASTE_ALL["seeds"]]
R1 = {"window": 30, "sources": ["network"], "newArtistsOnly": False,
      "limit": {"seeds": 12, "network": 32}, "networkOnRandomSeed": True}

MARKS = {
    "admin": ["before", "open", "copied", "kitchen", "sheet", "held", "dead", "dead2", "clip",
              "stop", "adding", "inert", "landed"],
    "member": ["open", "end", "tab"],
}[mode]

# The layout at 100 × 30, hints off, the queue folded (peer-recs contract
# §1.3 and §1.7): the content from x 17, the header on row 2, the strip on
# row 4, row 6 the verbs line or the band, the heads on 7, the rule on 8,
# the grid from row 9; columns A and B at x 17 and 57, 38 cells each: the
# swatch at +0, the title at +2, the artist at +19, the slot at +36.
X0, HEADER, STRIP, BAND, HEADS, RULE, GRID = 17, 2, 4, 6, 7, 8, 9
COL = (17, 57)
BAR = range(25, 30)

raw = open(stream, "rb").read()

# Cut the marks (and the newline puts adds) out, remembering where each was.
clean = b""
at_mark = {}
rest = raw
for name in MARKS:
    token = b"@@PR:" + name.encode() + b"@@"
    at = rest.find(token)
    if at < 0:
        print(f"peer recs ({mode}): MISSING mark @@PR:{name}@@")
        sys.exit(1)
    clean += rest[:at]
    at_mark[name] = len(clean)
    rest = rest[at + len(token):]
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
FRAMES = frames(clean, ends + list(at_mark.values()), ROWS, COLS)


def frame(name):
    return FRAMES[at_mark[name]]


if captures:
    os.makedirs(captures, exist_ok=True)
    for name in MARKS:
        with open(os.path.join(captures, f"{mode}-{name}.txt"), "w", encoding="utf-8") as f:
            f.write("\n".join(frame(name)))


def between(a, b):
    """Every frame after mark `a` (the start, when None) up to mark `b`,
    the mark's own included."""
    lo = at_mark[a] if a else 0
    hi = at_mark[b]
    return [FRAMES[n] for n in ends if lo < n <= hi] + [FRAMES[hi]]


def state(name):
    with open(os.path.join(home, f"state-{name}.json"), encoding="utf-8") as f:
        return json.load(f)


def row(lines, y):
    return lines[y] if y < len(lines) else ""


def cell(lines, x, y, n=1):
    """The `n` cells from (x, y). replay.py trims each row's trailing
    blanks, so a cell past the row's last drawn one reads as the blank it
    is on screen: without a device, column B's empty slot at +36 sits past
    the row's end."""
    return row(lines, y)[x:x + n].ljust(n)


def show(lines):
    return "\n".join(f"{y:2} {line}" for y, line in enumerate(lines[:ROWS]))


log = ""
try:
    with open(os.path.join(home, "mstream.log"), encoding="utf-8", errors="replace") as f:
        log = f.read()
except OSError:
    pass
DEVICE = "output opened on" in log

problems = []


def need(cond, what, lines=None):
    if not cond:
        problems.append(what + ("\n" + show(lines) if lines is not None else ""))
    return bool(cond)


# ── Colours: one replay of a prefix that keeps each cell's style ────────────
CSI = re.compile(r"\x1b\[(?P<p>[0-9;?:]*)(?P<f>[A-Za-z])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b.|(?P<ch>[^\x1b])",
                 re.S)


def styled(mark):
    """The frame at `mark` as rows of (char, fg, bg, bold): replay.py's
    reading (H, J, K and text) with the SGR colours kept."""
    text = clean[:at_mark[mark]].decode("utf-8", "replace")
    blank = (" ", None, None, False)
    grid = [[blank] * COLS for _ in range(ROWS)]
    r = c = 0
    fg = bg = None
    bold = False
    for m in CSI.finditer(text):
        f = m.group("f")
        if f:
            p = m.group("p")
            nums = [int(x) for x in p.replace("?", "").replace(":", ";").split(";") if x.isdigit()]
            if f == "H":
                r = (nums[0] - 1) if nums else 0
                c = (nums[1] - 1) if len(nums) > 1 else 0
            elif f == "J" and (not nums or nums[0] in (0, 2, 3)):
                grid = [[blank] * COLS for _ in range(ROWS)]
            elif f == "K":
                for x in range(c, COLS):
                    grid[min(r, ROWS - 1)][x] = (" ", fg, bg, bold)
            elif f == "m":
                nums = nums or [0]
                i = 0
                while i < len(nums):
                    n = nums[i]
                    if n == 0:
                        fg = bg = None
                        bold = False
                    elif n == 1:
                        bold = True
                    elif n == 22:
                        bold = False
                    elif n in (38, 48) and i + 1 < len(nums):
                        kind = nums[i + 1]
                        take = 3 if kind == 5 else 5 if kind == 2 else 2
                        colour = tuple(nums[i + 1:i + take])
                        if n == 38:
                            fg = colour
                        else:
                            bg = colour
                        i += take - 1
                    elif n == 39:
                        fg = None
                    elif n == 49:
                        bg = None
                    elif 30 <= n <= 37 or 90 <= n <= 97:
                        fg = (n,)
                    elif 40 <= n <= 47 or 100 <= n <= 107:
                        bg = (n,)
                    i += 1
        elif m.group("ch") is not None:
            ch = m.group("ch")
            if ch == "\n":
                r += 1
            elif ch == "\r":
                c = 0
            elif ch >= " ":
                if r < ROWS and c < COLS:
                    grid[r][c] = (ch, fg, bg, bold)
                c += 1
    return grid


# ── What the room draws ─────────────────────────────────────────────────────
def title_at(lines, col, y):
    """The title in column `col` (0 or 1) on row `y`: the 15 cells from +2."""
    return cell(lines, COL[col] + 2, y, 15).rstrip()


def grid_reads(lines, a, b):
    """Column A's titles from row 9 are `a`, column B's `b`, each swatch at
    +0, and the rows under them empty to the grid's end."""
    for col, titles in ((0, a), (1, b)):
        for i, title in enumerate(titles):
            y = GRID + i
            if title_at(lines, col, y) != title or cell(lines, COL[col], y) != "▌":
                return False
        for y in range(GRID + len(titles), BAR.start):
            if cell(lines, COL[col], y, 38).strip():
                return False
    return True


def rank_titles(titles):
    """Column-major in one block: ceil(n / 2) in column A, the rest in B."""
    half = (len(titles) + 1) // 2
    return titles[:half], titles[half:]


def strip(lines):
    return row(lines, STRIP)[X0:]


def nav_row(lines, label):
    """The nav row (x 0–14) whose label is `label`: its row, or None."""
    for y, line in enumerate(lines[:BAR.start]):
        if line[:15].strip() in (label, "▸ " + label):
            return y
    return None


def note_shown(lines, word):
    return any(word in row(lines, y) for y in BAR)


def osc52(a, b):
    """The texts the stream put on the clipboard between marks a and b."""
    span = clean[at_mark[a]:at_mark[b]]
    return [base64.b64decode(m.group(1)).decode("utf-8")
            for m in re.finditer(rb"\x1b\]52;c;([A-Za-z0-9+/=]*)(?:\x07|\x1b\\)", span)]


def sheet_on(lines, title):
    """The lead's sheet stands: its frame from x 17 on row 9, the lead's
    title BOLD at x 19 on its first inner row, and its [X] at x 79–81."""
    return (cell(lines, 17, 9) == "╭" and cell(lines, 82, 9) == "╮"
            and cell(lines, 19, 10, len(title)) == title and cell(lines, 79, 10, 3) == "[X]")


def sheet_rows(lines):
    """The sheet's row labels from inner row 6 (row 15), x 19 to the frame,
    the Preview row's glyph left out."""
    out = []
    for y in range(15, 21):
        if cell(lines, 17, y) != "│":
            break
        text = cell(lines, 18, y, 64).strip()
        if not text:
            break
        out.append(re.sub(r"^(▸|▮▮|◌)\s+", "", text))
    return out


def resolves_of(st, title):
    return [r for r in st.get("preview_resolves", []) if r.get("title") == title]


# ── The scenarios ───────────────────────────────────────────────────────────
STRIP_ADMIN = " All    • Basement Hi-Fi    • Kitchen Radio    Attic Tapes    mStream · 3f9a   ›"
STRIP_MEMBER = " All    • Basement Hi-Fi    • Kitchen Radio    Attic Tapes    mStream · 3f9a"
ALL_A, ALL_B = rank_titles([lead["title"] for lead in LEADS])
KITCHEN_A, KITCHEN_B = rank_titles([l["title"] for l in fixture("taste_kitchen")["network"]["results"]])


def opened(lines):
    """The room as R opens it (peer-recs contract §1.1, §1.3–§1.7): the nav
    row under Discover, the header, the twelve leads 6 | 6 under their
    heads and rule."""
    need(nav_row(lines, "Discover") == 17 and nav_row(lines, "Peer recs") == 18
         and row(lines, 18).startswith(" ▸ Peer recs"),
         "the nav: Discover on row 17, Peer recs active on row 18", lines)
    head = row(lines, HEADER)[X0:]
    need(head.startswith("Peer Recommendations") and "Last 30 days ▾" in head
         and "more options" in head and head.rstrip().endswith("↻ new seeds"),
         "the header on row 2: name, window, more options, new seeds", lines)
    need(all(cell(lines, x + d, HEADS, len(w)) == w for x in COL for d, w in ((2, "TRACK"), (19, "ARTIST"))),
         "the heads on row 7: TRACK at +2 and ARTIST at +19 per column", lines)
    need(cell(lines, X0, RULE, 80) == "─" * 80, "the rule on row 8 across the 80 cells", lines)
    need(grid_reads(lines, ALL_A, ALL_B), "the twelve leads read 6 | 6, titles at x 19 and 59", lines)
    need(title_at(lines, 0, GRID) == "Salt Road" and title_at(lines, 1, GRID) == "Long Exposure",
         "the columns' first titles at x 19 and 59", lines)
    artists = [lead["artist"] for lead in LEADS]
    need(all(cell(lines, COL[i // 6] + 19, GRID + i % 6, 16).rstrip() == artists[i][:16] for i in range(12)),
         "each lead's artist at +19", lines)
    slot = "▸" if DEVICE else " "
    need(all(cell(lines, x + 36, GRID + i) == slot for x in COL for i in range(6)),
         f"every slot at +36 reads {slot!r} ({'a device' if DEVICE else 'no device'})", lines)
    verbs = "▸ preview all · copy list" if DEVICE else "copy list"
    need(row(lines, BAND)[X0:].rstrip() == verbs, f"the verbs line on row 6 reads {verbs!r}", lines)


def admin():
    before = frame("before")
    need(nav_row(before, "Peer recs") == 18 and not row(before, 18).startswith(" ▸"),
         "before R: Peer recs on nav row 18, at rest", before)
    need(not row(before, HEADER)[X0:].startswith("Peer Recommendations"), "before R: the room is not up", before)

    lines = frame("open")
    opened(lines)
    need(strip(lines).rstrip() == STRIP_ADMIN, "the strip on row 4 is REG-STRIP 01", lines)
    st = state("open")
    need(st["network_bodies"] == [R1], f"the open asked R1 once: {st['network_bodies']}")

    # y: the list, one OSC 52 of the twelve lines in rank order.
    copies = osc52("open", "copied")
    want = "\n".join(f"{lead['artist']} - {lead['title']}" for lead in LEADS)
    need(copies == [want], f"y put the twelve lines on the clipboard once: {copies!r}")
    need(any(note_shown(f, "Copied") for f in between("open", "copied")), "y noted Copied")

    # The Kitchen Radio tab, clicked: its leads 3 | 2, asked at once, pinned.
    lines = frame("kitchen")
    need(grid_reads(lines, KITCHEN_A, KITCHEN_B), "the Kitchen Radio tab reads its five leads 3 | 2", lines)
    need(strip(lines).rstrip() == STRIP_ADMIN, "the click moved no tab", lines)
    bodies = state("kitchen")["network_bodies"]
    need(len(bodies) == 2, f"the tab click asked once, its echo nothing: {bodies}")
    if len(bodies) >= 2:
        body = bodies[1]
        need(body.get("networkPeers") == [PEERS["Kitchen Radio"]] and body.get("pin") == BATCH_PINS
             and body.get("limit") == {"seeds": len(BATCH_PINS), "network": 32}
             and body.get("sources") == ["network"] and body.get("window") == 30,
             f"the tab's ask is R2: networkPeers, the batch's pins, limit.seeds their count: {body}")

    # The row click's sheet; its echo on the sheet's [X] closes nothing.
    shots = between("kitchen", "sheet")
    first = next((i for i, f in enumerate(shots) if sheet_on(f, "Field Notes")), None)
    if need(first is not None, "the row click opened Field Notes' sheet", shots[-1]):
        gone = next((f for f in shots[first:] if not sheet_on(f, "Field Notes")), None)
        need(gone is None, "the echo closed the sheet", gone)
    lines = frame("sheet")
    rows_want = (["Preview"] if DEVICE else []) + ["Get it", "Open elsewhere…", "Copy title"]
    need(cell(lines, 19, 11, 24) == "Ada Merrow · Cartography" and cell(lines, 19, 12, 4) == "3:27"
         and row(lines, 13)[19:].startswith("from Kitchen Radio   • online"),
         "the sheet's head: byline, length, badge with the online dot", lines)
    need(sheet_rows(lines) == rows_want, f"the sheet's rows are {rows_want}", lines)
    need(not any("★" in row(lines, y) or "☆" in row(lines, y) for y in range(9, 21)),
         "a lead's sheet draws no stars", lines)
    st = state("sheet")
    need(st["job_starts"] == [] and st["preview_resolves"] == [] and len(st["network_bodies"]) == 2
         and osc52("kitchen", "sheet") == [],
         f"the echo pressed nothing: {st['job_starts']} {st['preview_resolves']}")

    held = styled("held")
    slab = held[11][COL[0] + 2][2]
    need(slab is not None and slab != held[10][COL[0] + 2][2] and slab != held[9][COL[0] + 2][2],
         "↓ ↓ ↓ holds Long Exposure (the slab on row 11 alone)", frame("held"))

    if DEVICE:
        # The dead clip (c718b2e8): No preview, the answer forgotten, so the
        # next press resolves again.
        need(any(note_shown(f, "No preview") for f in between("held", "dead")),
             "the dead clip noted No preview", frame("dead"))
        lines = frame("dead")
        need(cell(lines, COL[0] + 36, 11) == "▸" and row(lines, BAND)[X0:].startswith("▸ preview all"),
             "after the dead clip: its slot ▸ and the verbs line back", lines)
        one = resolves_of(state("dead"), "Long Exposure")
        two = resolves_of(state("dead2"), "Long Exposure")
        need(len(one) == 1 and len(two) == 2 and all(r["answer"] == "dead" for r in two),
             f"the next l resolved again, once: {len(one)} then {len(two)}")
        need(len(state("dead2")["preview_resolves"]) == 2, "no other lead was resolved")

        # Signal Fires: its clip sounds, the band on row 6.
        band = next((f for f in between("dead2", "clip")
                     if row(f, BAND)[X0:].startswith("▮▮ ▸▸ Signal Fires")), None)
        if need(band is not None, "l on Signal Fires drew the band playing", frame("clip")):
            need(cell(band, COL[0] + 36, GRID, 2) == "▮▮", "the live row's slot reads ▮▮", band)
            need(row(band, BAND).endswith("Preview from Deezer [X]")
                 or row(band, BAND).endswith("Preview from Deezer [ get it ] [X]"),
                 "the band's cluster ends in the attribution and [X] at x 96", band)
            # The Get it gate holds on this fake (an acquire plug-in, jobs
            # allowed, a destination): the band offers it (contract §1.8).
            need(row(band, BAND).endswith(" [ get it ] [X]"), "the band draws [ get it ] before its [X]", band)
        st = state("clip")
        need([r["title"] for r in st["preview_resolves"]][-1:] == ["Signal Fires"] and 3 in st["clip_gets"],
             f"Signal Fires was resolved and its clip fetched: {st['clip_gets']}")
        lines = frame("clip")
        need(row(lines, BAND)[X0:].startswith("▸ preview all"), "the clip ran out: the verbs line back", lines)
        clip = styled("clip")
        need(clip[11][COL[0] + 36][0] == "▸" and clip[11][COL[0] + 36][1] == clip[10][COL[0] + 36][1],
             "Long Exposure's slot reads idle (as Harbor Lights'), not muted", lines)
    else:
        need(not any("Preview from" in "\n".join(f) for f in between(None, "landed")),
             "without a device no band is drawn")
        need(state("landed")["preview_resolves"] == [], "without a device nothing is resolved")

    # The Add: the stop on `+ Lighthouse Tapes`, one post, inert, landed.
    lines = frame("stop")
    tabs = strip(lines)
    plus = tabs.find("+ Lighthouse Tapes")
    if need(plus >= 0, "the tabs stop walked to + Lighthouse Tapes", lines):
        stop = styled("stop")
        need(stop[STRIP][X0 + plus + 2][2] is not None and stop[STRIP][X0 + plus + 2][2] != stop[3][X0][2],
             "the stop's slab on + Lighthouse Tapes", lines)
    need(state("stop")["adds"] == 0, "the walk posted nothing")
    lines = frame("adding")
    need("Lighthouse Tapes adding…" in strip(lines), "Enter: Lighthouse Tapes adding…", lines)
    st = state("adding")
    need(st["adds"] == 1 and st["add_bodies"] == [{"endpointId": PEERS["Lighthouse Tapes"]}]
         and st["add_answers"] == [202], f"one Add posted: {st['add_bodies']} {st['add_answers']}")
    need(state("inert")["adds"] == 1, "Enter on the adding tab posted nothing")
    lines = frame("landed")
    tabs = strip(lines)
    need("• Lighthouse Tapes" in tabs and "adding" not in tabs and "Attic Tapes" not in tabs,
         "landed: Lighthouse Tapes on the shelf, Attic Tapes gone", lines)
    st = state("landed")
    need(st["peer_adds"].get("Lighthouse Tapes", {}).get("state") == "done", f"the Add is done: {st['peer_adds']}")
    bodies = st["network_bodies"]
    need(len(state("inert")["network_bodies"]) == 2 and len(bodies) == 3,
         f"one re-ask followed the landing: {len(bodies)} bodies")
    if len(bodies) == 3:
        need(bodies[2].get("pin") == BATCH_PINS and bodies[2].get("networkPeers") == [PEERS["Kitchen Radio"]],
             f"the re-ask is pinned to the batch, on the tab: {bodies[2]}")


def member():
    lines = frame("open")
    opened(lines)
    need(strip(lines).rstrip() == STRIP_MEMBER, "the member's strip is REG-STRIP 02", lines)
    for f in between(None, "tab"):
        tabs = strip(f)
        content = "\n".join(line[X0:] for line in f[:BAR.start])
        if not need("+ " not in tabs and "adding…" not in content and "try again" not in content,
                    "a member's frame drew an Add", f):
            break
    lines = frame("end")
    need(strip(lines).rstrip() == STRIP_MEMBER, "the stop walked to the strip's end; nothing lies past it", lines)
    end = styled("end")
    label = STRIP_MEMBER.find("mStream · 3f9a")
    need(end[STRIP][X0 + label][2] is not None and end[STRIP][X0 + label][2] != end[3][X0][2],
         "the stop's slab on mStream · 3f9a", lines)
    lines = frame("tab")
    need(grid_reads(lines, ["Quiet Engine"], ["Paper Moons"]), "mStream · 3f9a's two leads read 1 | 1", lines)
    st = state("tab")
    need(st["adds"] == 0 and st["network_bodies"][-1].get("networkPeers") == [PEERS["mStream · 3f9a"]],
         f"no Add, and the tab asked: {st['adds']} {st['network_bodies'][-1:]}")


admin() if mode == "admin" else member()
print(f"peer recs ({mode}, {'a device' if DEVICE else 'no device'}):",
      "ALL PASS" if not problems else "\n".join(problems))
sys.exit(1 if problems else 0)
