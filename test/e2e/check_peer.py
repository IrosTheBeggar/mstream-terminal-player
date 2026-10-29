#!/usr/bin/env python3
"""Assert Keep a copy while browsing a connected server, from a captured GUI
stream (the gui-peer-files and gui-peer-album legs; the keep-a-copy spec's
frames, docs/ux-contracts/discovery-page.md, clauses 150–172).

Each leg writes a mark into the stream after each step has been drained
(@@PF:word@@ … for the Files leg, @@PA:word@@ … for the album leg). The
marks are cut out and the stream is replayed at each (ratatui's diff
renderer emits only changed cells, so stream-grep lies about anything that
repaints in place). The frames are the spec's, cell for cell where it
draws them: the header line's word and its state in the same cells,
the album page's credit, the window's folder, song, album and artist
focus, the Into a folder chooser, the queue item's sheet row, the copy
states on the header, the rows, the cards and the Copies list.
Usage: check_peer.py files|album STREAM REPLAY_PY ROWS COLS [CAPTURES_DIR]
  CAPTURES_DIR, when given, receives each mark's decoded frame as
  <leg>-<mark>.txt.
"""
import os
import re
import subprocess
import sys

leg, stream, replay, rows, cols = sys.argv[1:6]
captures = sys.argv[6] if len(sys.argv) > 6 else ""
TAG = {"files": "PF", "album": "PA"}[leg]
NAMES = {
    "files": ["word", "mirror", "mirror-live", "mirror-room", "held", "into", "chooser", "typed", "into-place",
              "tags", "copies", "kept"],
    "album": ["word", "held", "song", "narrow", "queue-c", "sheet", "queue-song", "folded", "album", "album-live",
              "album-kept", "state", "wall", "artist", "artist-unticked", "copies"],
}[leg]
raw = open(stream, "rb").read()

# Cut the marks (and the newline puts adds) out, remembering where each was.
clean = b""
marks = {}
rest = raw
for name in NAMES:
    mark = f"@@{TAG}:{name}@@".encode()
    at = rest.find(mark)
    if at < 0:
        print(f"keep from a connected server ({leg}): MISSING mark {mark.decode()}")
        sys.exit(1)
    clean += rest[:at]
    marks[name] = len(clean)
    rest = rest[at + len(mark):]
    if rest.startswith(b"\r\n"):
        rest = rest[2:]
    elif rest.startswith(b"\n"):
        rest = rest[1:]
clean += rest

frames = {}


def frame(name):
    if name not in frames:
        cut = clean[:marks[name]]
        j = cut.rfind(b"\x1b[?1049l")
        if 0 < j < len(cut):
            cut = cut[:j]
        open(stream + ".cut", "wb").write(cut)
        out = subprocess.run(["python3", replay, stream + ".cut", rows, cols], capture_output=True, text=True).stdout
        frames[name] = (out.split("\n") + [""] * int(rows))[: int(rows)]
        if captures:
            os.makedirs(captures, exist_ok=True)
            with open(os.path.join(captures, f"gui-peer-{leg}-{name}.txt"), "w") as f:
                f.write("\n".join(f"{y:2d} {line}" for y, line in enumerate(frames[name])) + "\n")
    return frames[name]


def window(lines, title):
    """A modal's rows between its borders — the title on its top border —
    each as its inner cells (the cell after the left border on), trailing
    blanks dropped; [] when it is not up."""
    for y, line in enumerate(lines):
        at = line.find(f"╭─ {title} ─")
        if at < 0:
            continue
        for z in range(y + 1, len(lines)):
            if lines[z][at:at + 1] == "╰":
                return [l[at + 1:].rstrip().removesuffix("│").rstrip() for l in lines[y + 1:z]]
    return []


def copies(lines):
    """The Copies list's rows (its title on its first inner row)."""
    for y, line in enumerate(lines):
        if "│ Copies " in line:
            return [l.rstrip() for l in lines[y:]]
    return []


problems = []


def need(name, ok, why):
    if not ok:
        problems.append(f"{name}: {why}:\n" + "\n".join(f"{y:2d} {l}" for y, l in enumerate(frame(name))))


def same(name, got, want, why):
    """The window's rows exactly — each inner row, a leading blank cell and
    the words, the right-aligned key words and primary in their cells."""
    got = [r.replace("▏", "").rstrip() for r in got]  # the caret blinks
    want = [r.replace("▏", "").rstrip() for r in want]
    if got != want:
        diff = "\n".join(f"  want {w!r}\n  got  {g!r}" for w, g in zip(want + [""] * len(got), got + [""] * len(want))
                         if w != g)
        problems.append(f"{name}: {why}:\n{diff}")


def header(name):
    return frame(name)[2].rstrip()


# The spec's frames (§6.2–§6.7): each window row's inner cells, from the cell
# after the left border, trailing blanks dropped.
def rows_of(text):
    return [l[1:].rstrip().removesuffix("│").rstrip() for l in text.strip("\n").split("\n")[1:-1]]


if leg == "files":
    # §4.5 / M4: the word 3 cells after the crumb, "read-only" alone, the
    # count right-aligned; no "playlists and ratings" anywhere.
    need("word", header("word") == " ▸ Files       │ ◂ Files ▸ peerlib/Boukmanflow · read-only   keep a copy                  3 items",
         "the Files crumb's word")
    need("word", all(frame("word")[y][17:19] == "  " for y in (6, 7, 8)), "folder rows unmarked")
    need("word", not any("playlists and ratings" in l for l in frame("word")), "the cut clause drawn")
    # M5: the open folder, As on Stage Peer B, counted by one recursive listing.
    same("mirror", window(frame("mirror"), "Keep a copy"), rows_of("""
╭─ Keep a copy ─╮
│ Boukmanflow                                                              │
│ from Stage Peer B                                                        │
│                                                                          │
│ (•) As on Stage Peer B                                                   │
│ ( ) Into a folder…                                                       │
│ ( ) By your layout                                                       │
│                                                                          │
│ Files go to copies › Boukmanflow                                         │
│                                                                          │
│                                                 [ Add folder · 9 songs ] │
╰─╯
"""), "folder focus on the open folder (M5)")
    need("mirror", "keep a copy" not in header("mirror"), "the word drawn under the window")
    live = window(frame("mirror-live"), "Keep a copy")
    need("mirror-live", any(l.startswith(" Copying") or l.startswith(" Waiting") for l in live), "the window follows the copy")
    need("mirror-live", " Into copies › Boukmanflow" in live, "its landing: the folder's own path (mirror)")
    need("mirror-live", any(l.startswith(" x cancel copy") and l.endswith("[ Close ]") for l in live), "its action row")
    need("mirror-room", window(frame("mirror-room"), "Keep a copy") == [], "the window closed")
    need("mirror-room", header("mirror-room")[61:].split("   ")[0].strip() in ("◷ waiting", "copying…"),
         "the open folder's live state in the word's cells (x 61)")
    need("mirror-room", "copying Boukmanflow…" in frame("mirror-room")[29], "the start's note")
    need("held", "keep a copy" not in header("held") and header("held").endswith("3 items"), "the word hidden while a row is held")
    # §6.5.1: a held folder row, Into a folder… chosen — `d change` under it.
    same("into", window(frame("into"), "Keep a copy"), rows_of("""
╭─ Keep a copy ─╮
│ Borrowed Tapes                                                           │
│ from Stage Peer B                                                        │
│                                                                          │
│ ( ) As on Stage Peer B                                                   │
│ (•) Into a folder…                                                       │
│ ( ) By your layout                                                       │
│                                                                          │
│ Files go to copies › Borrowed Tapes                             d change │
│                                                                          │
│                                                 [ Add folder · 3 songs ] │
╰─╯
"""), "folder focus on the held row, Into a folder… chosen (§6.5.1)")
    # §6.6: the chooser's default, and a new folder typed inside From Sam.
    same("chooser", window(frame("chooser"), "Into a folder"), rows_of("""
╭─ Into a folder ─╮
│ Files go to copies › Borrowed Tapes                                          │
│ “Borrowed Tapes” isn't there yet · the first file creates it                 │
│                                                                              │
│ (•) copies   ( ) music                                                       │
│                                                                              │
│ Borrowed Tapes                                                               │
│   Boukmanflow                                                                │
│   Color Out                                                                  │
│   From Sam                                                                   │
│                                                                              │
╰─╯
"""), "the chooser on its default (§6.6)")
    need("chooser", window(frame("chooser"), "Keep a copy") == [], "Keep a copy drawn under the chooser")
    same("typed", window(frame("typed"), "Into a folder"), rows_of("""
╭─ Into a folder ─╮
│ Files go to copies › From Sam › Borrowed Tapes                               │
│ “Borrowed Tapes” isn't there yet · the first file creates it                 │
│                                                                              │
│ (•) copies   ( ) music                                                       │
│                                                                              │
│ From Sam/Borrowed Tapes▏                                                     │
│   Up to copies                                                               │
│   No folders in here                                                         │
│                                                                              │
╰─╯
"""), "a new folder typed inside From Sam: its nearest folder listed (§6.6)")
    place = window(frame("into-place"), "Keep a copy")
    need("into-place", " (•) Into a folder…" in place, "Esc kept the choice")
    need("into-place", " Files go to copies › From Sam › Borrowed Tapes                  d change" in place,
         "the place line from the chooser's last valid preview")
    # §6.5.1 / §6.5.3: By your layout, through the account's layout.
    same("tags", window(frame("tags"), "Keep a copy"), rows_of("""
╭─ Keep a copy ─╮
│ Harbour Lights                                                           │
│ from Stage Peer B                                                        │
│                                                                          │
│ ( ) As on Stage Peer B                                                   │
│ ( ) Into a folder…                                                       │
│ (•) By your layout                                                       │
│                                                                          │
│ Files go to copies › Boukmanflow › Harbour Lights               d change │
│                                                                          │
│                                                 [ Add folder · 3 songs ] │
╰─╯
"""), "By your layout on the second held row")
    # §6.7: the Copies list, opened outside Discover (Q1), its kind "folder".
    listed = copies(frame("copies"))
    titles = [l for l in listed if "Stage Peer B" in l]
    need("copies", len(titles) == 3 and all(any(t in l for l in titles) for t in ("Boukmanflow", "Borrowed Tapes", "Harbour Lights")),
         "the three folder copies listed")
    need("copies", sum("│   folder · " in l for l in listed) == 3, "each one's kind: folder")
    need("copies", "TITLE" in "\n".join(listed) and "FROM" in "\n".join(listed), "the Copies list's header")
    # §5.1 / §5.4: the open folder kept, its kept folder rows' ✓.
    need("kept", header("kept")[61:].split("   ")[0].strip() == "✓ kept", "the open folder's ✓ kept in the word's cells")
    need("kept", frame("kept")[6][17:] == "✓ Borrowed Tapes" and frame("kept")[7][17:] == "✓ Harbour Lights"
         and frame("kept")[8][17:] == "  Second Shift", "the folder rows' marks: their own folder copies only")
else:
    # M1 with the credit (the user's 2026-10-27 call): "{album} — {credit}",
    # the word 3 cells after it, the count right-aligned.
    need("word", header("word") == "   Files       │ ◂ Albums  Borrowed Tapes — Boukmanflow   keep a copy                     3 items",
         "the album page's credit and its word")
    need("word", [l[17:].rstrip() for l in frame("word")[5:9]] == [
        "  ..", "  Boukmanflow - Rooftop Cypher                                           0:12",
        "  Boukmanflow - Late Train Verse                                         0:08",
        "  Boukmanflow - Corner Store Loop                                        0:09"], "the page's rows, unmarked")
    need("held", header("held") == "   Files       │ ◂ Albums  Borrowed Tapes — Boukmanflow                                   3 items",
         "the word hidden while a row is held")
    # §6.3: the song window from a held row — its choices appended.
    song = rows_of("""
╭─ Keep a copy ─╮
│ Late Train Verse                                                         │
│ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
│                                                                          │
│ (•) This song                                                            │
│ ( ) Album                                                                │
│ ( ) Every album by Boukmanflow                                           │
│     [✓] Only the 2 albums you don't have                                 │
│                                                                          │
│ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
│                                                                          │
│                                                             [ Add song ] │
╰─╯
""")
    same("song", window(frame("song"), "Keep a copy"), song, "the song window from a held row (§6.3)")
    # §4.1.4 at 47 cells: the count gives way, the way back shrinks to its
    # glyph; the title, the credit and the word stay.
    need("narrow", frame("narrow")[2].startswith("   Files       │ ◂  Borrowed Tapes — Boukmanflow   keep a copy    │ QUEUE"),
         "the 47-cell header line's give-way")
    need("queue-c", window(frame("queue-c"), "Keep a copy") == [], "`c` on the held queue row opened something")
    # §4.8: the queue item's sheet, Keep a copy… in Add to playlist's slot,
    # the badge row blank.
    sheet = frame("sheet")
    verbs = [l[17:].strip(" │") for l in sheet[8:22]]
    verbs = [v for v in verbs if v in ("Play now", "Add next", "Add to end of queue", "Keep a copy…", "Song Info", "Remove from queue")]
    need("sheet", verbs == ["Play now", "Add next", "Add to end of queue", "Keep a copy…", "Song Info", "Remove from queue"],
         "the queue sheet's rows")
    need("sheet", not any("from Stage Peer B" in l for l in sheet[8:22]), "a badge on the queue sheet")
    need("sheet", "keep a copy" not in sheet[2], "the header word drawn under the sheet")
    same("queue-song", window(frame("queue-song"), "Keep a copy"), song, "the queue item's song window (§4.8, W10)")
    need("folded", header("folded") == "   Files       │ ◂ Albums  Borrowed Tapes — Boukmanflow   keep a copy                     3 items",
         "the word back once the row is stowed")
    # M2 / §6.2: album focus opened by the word.
    same("album", window(frame("album"), "Keep a copy"), rows_of("""
╭─ Keep a copy ─╮
│ Borrowed Tapes                                                           │
│ Boukmanflow · 2019 · from Stage Peer B                                   │
│                                                                          │
│ (•) Album                                                                │
│ ( ) Every album by Boukmanflow                                           │
│     [✓] Only the 2 albums you don't have                                 │
│                                                                          │
│ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
│                                                                          │
│                                                  [ Add album · 3 songs ] │
╰─╯
"""), "album focus, Album chosen (M2)")
    need("album", "keep a copy" not in header("album"), "the word drawn under the window")
    # §5.1: copying…, then ✓ kept and the rows' ✓.
    need("album-live", header("album-live")[58:].split("   ")[0].strip() in ("◷ waiting", "copying…"),
         "the album's live state in the word's cells (x 58)")
    need("album-live", frame("album-live")[6][17:19] == "  " and frame("album-live")[8][17:19] == "  ",
         "rows marked by a live album copy")
    need("album-kept", header("album-kept") == "   Files       │ ◂ Albums  Borrowed Tapes — Boukmanflow   ✓ kept                          3 items",
         "the album's ✓ kept in the word's cells")
    # The queued Late Train Verse may still be playing (Linux keeps the
    # fake's empty stream at 0:00; macOS gives it up): the playing glyph
    # takes its lead cell over the ✓.
    kept = frame("album-kept")
    late = "▸ " if "Late Train Verse" in kept[26] else "✓ "
    need("album-kept", [kept[y][17:19] for y in (6, 7, 8)] == ["✓ ", late, "✓ "], "the kept album's rows' ✓")
    outcome = window(frame("state"), "Keep a copy")
    need("state", " ✓ In your collection" in outcome and " 2 copied · 1 already yours" in outcome, "the state's click: its outcome")
    need("state", " In copies › Boukmanflow › Borrowed Tapes" in outcome, "where it landed")
    need("state", any(l.startswith(" c choose again") and l.endswith("[ Play your copy ]") for l in outcome), "its action row")
    # M3: the artist's wall, the word after the count; the kept card's year
    # line wears the tile word (§5.2).
    need("wall", header("wall") == "   Files       │ ◂ Artists ▸ Boukmanflow  4 albums   keep a copy                         ◂ 1/1 ▸",
         "the artist's wall's word (M3)")
    cards = frame("wall")[11][17:]
    years = frame("wall")[12][17:]
    at = cards.find("Borrowed Ta…")
    need("wall", at >= 0 and years[at:at + 12].strip() == "✓ kept", "the kept card's year line")
    need("wall", "2023" in years and "2021" in years and "2024" in years, "the other cards' years")
    # §6.4: artist focus — the box alone, count-less; "Leaves out …".
    same("artist", window(frame("artist"), "Keep a copy"), rows_of("""
╭─ Keep a copy ─╮
│ Boukmanflow                                                              │
│ from Stage Peer B                                                        │
│                                                                          │
│ [✓] Only the albums you don't have                                       │
│ Leaves out Night Market Sampler                                          │
│                                                                          │
│ Files go to copies › Boukmanflow › one folder per album         d change │
│                                                                          │
│                                                         [ Add 2 albums ] │
╰─╯
"""), "artist focus, ticked (§6.4)")
    same("artist-unticked", window(frame("artist-unticked"), "Keep a copy"), rows_of("""
╭─ Keep a copy ─╮
│ Boukmanflow                                                              │
│ from Stage Peer B                                                        │
│                                                                          │
│ [ ] Only the albums you don't have                                       │
│ Leaves out Night Market Sampler                                          │
│                                                                          │
│ Files go to copies › Boukmanflow › one folder per album         d change │
│                                                                          │
│                                                         [ Add 3 albums ] │
╰─╯
"""), "artist focus, unticked: the primary counts theirs (§6.4)")
    listed = copies(frame("copies"))
    text = "\n".join(listed)
    need("copies", re.search(r"Borrowed Tapes +Stage Peer B +✓ in your collection", text) is not None
         and "│   album · 2 copied · 1 already yours" in text, "the album copy listed")
    need("copies", re.search(r"Late Train Verse +Stage Peer B ", text) is not None and "│   song · " in text, "the song copy listed")

print(f"keep from a connected server ({leg}):", "ALL PASS" if not problems else "\n".join(problems))
sys.exit(1 if problems else 0)
