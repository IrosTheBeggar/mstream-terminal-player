import os, shutil
# Auto DJ on the rig (auto-dj contract, clauses 40–58; the room rebuilt
# 2026-11-10): the room from the nav — one row a setting, the server's
# reasons standing where a value would — Start on an empty queue through
# the First song question and Recently played, a batch landing with its
# badges, Preview's sheet, the Batch, Length, Genres and Skip words sheets
# each ending in the row's value, First song's list, quick config over the
# bar, Play from moving the DJ to the proxied peer (Rig A via B) with the
# next turn picked from there, Stop, and the bar's button starting the DJ
# on Play from's home. The rig's servers have no discovery data and no mood
# scores, so those two rows are greyed and the picks stay random (∞, never
# ≈).
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

# The room's columns on the 120-cell pty: between the nav's rule and the
# queue panel's. The rows stand centred in them — 56 cells from column 22
# here — and a sheet stands over them and a little past.
ROOM = (17, 86)

def badges(g):
    return sum(line.count("∞") + line.count("≈") for line in g.text())

def room_find(g, needle):
    """A needle inside the room's columns — the nav has a Genres of its own."""
    for i, line in enumerate(g.text()):
        j = line.find(needle, ROOM[0])
        if 0 <= j <= ROOM[1]:
            return i, j
    return None

def room_wait(g, needle, timeout, what=None):
    for _ in range(int(timeout / 0.2) + 1):
        hit = room_find(g, needle)
        if hit:
            return hit
        g.pump(0.2)
    g.dump(f"timeout waiting for {what or needle!r} in the room")
    raise SystemExit(f"FAIL: never saw {needle!r} in the room")

def row_reads(g, name, value, timeout=5):
    """The room's row `name` showing `value` (a row is its name, then its
    value ending at one column)."""
    for _ in range(int(timeout / 0.2) + 1):
        hit = room_find(g, name)
        if hit and value in g.text()[hit[0]][hit[1]:ROOM[1] + 1]:
            return hit
        g.pump(0.2)
    g.dump(f"the {name!r} row never read {value!r}")
    raise SystemExit(f"FAIL: the {name} row never read {value!r}")

def press_row(g, name):
    row, col = room_wait(g, name, 5)
    g.click(row, col)
    g.pump(0.4)

def done(g):
    g.click_text("[ Done ]", 5, offset=2)
    g.pump(0.5)
    if g.find("[ Done ]"):
        g.dump("the sheet did not close")
        raise SystemExit("FAIL: [ Done ] did not close the sheet")

def clear(g):
    """The sheet's own `clear`, on its foot beside [ Done ] — the queue
    header has a `clear` too, higher up the screen."""
    foot = g.wait_for("[ Done ]", 5)
    col = g.text()[foot[0]].find("clear")
    if col < 0:
        g.dump("the sheet shows no clear")
        raise SystemExit("FAIL: nothing to clear in the sheet")
    g.click(foot[0], col + 1)
    g.pump(0.3)

def run(Gui):
    cfg = os.path.join(R, "gui-config-dj")
    shutil.rmtree(cfg, ignore_errors=True)
    shutil.copytree(os.path.join(R, "gui-config"), cfg)
    for stale in ("queue.json",):
        try:
            os.remove(os.path.join(cfg, stale))
        except FileNotFoundError:
            pass
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": cfg, "MSTREAM_SETUP_LANG": "en",
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-dj.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(0.5)

    # The nav's Auto DJ row under TOOLS — the room, off: Start, the session's
    # rows, and the two groups, the probe's answers standing where a value
    # would (the rig has no discovery data and no mood scores).
    g.wait_for("TOOLS", 10, "the nav's TOOLS group")
    g.click_text("Auto DJ", 5)
    room_wait(g, "Start ▸", 10, "the room's Start")
    row_reads(g, "Sonic similarity", "Discovery off", 15)
    row_reads(g, "Smart filters", "No mood data")
    row_reads(g, "Play from", "3041 ▸")
    row_reads(g, "First song", "Ask ▾")
    row_reads(g, "Batch", "4 songs ▸")
    # The seed config's 20:00 was the end of the bar it was set on: with no
    # ceiling of its own in the file it is no maximum, and the switch is off.
    row_reads(g, "Length", "Any ▸")
    for header in ("MUST MATCH", "PREFER", "Preview picks ▸"):
        room_wait(g, header, 5)
    g.dump("the room, off")

    # A greyed row takes no click: nothing opens over the room.
    press_row(g, "Sonic similarity")
    if g.find("[ Done ]"):
        raise SystemExit("FAIL: a greyed row opened a sheet")

    # Start with nothing queued: the First song question, with Recently
    # played's list hung from its row — B's own history — and a song of it
    # opening the session. The DJ is armed for B (the note names the server:
    # several are saved) and a batch of four follows the opener.
    press_row(g, "Start ▸")
    g.wait_for("Remember", 10, "the First song question")
    g.dump("the First song question")
    g.click_text("Recently played", 5)
    g.wait_for("6AM", 10, "B's recently played songs")
    g.dump("Recently played, open")
    g.click_text("6AM", 5)
    room_wait(g, "Stop", 15, "the room's Stop")
    g.wait_for("∞", 40, "a DJ pick with its badge in the queue panel")
    for _ in range(40):
        g.pump(0.5)
        if g.find("5 tracks"):
            break
    g.dump("started from a recently played song; the first batch landed")
    n1 = badges(g)
    print(f"badged rows: {n1}", flush=True)
    if n1 < 4:
        raise SystemExit("FAIL: expected a batch of four badged rows")
    if not g.find("5 tracks"):
        raise SystemExit("FAIL: the queue did not grow by the batch")
    row_reads(g, "Play from", "3041 ▸")

    # Preview: three picks in its sheet, none of them queued.
    press_row(g, "Preview picks ▸")
    g.wait_for("[ Done ]", 5, "Preview's sheet")
    g.pump(6.0)
    g.dump("Preview's sheet")
    top = g.find("─ Preview picks")
    foot = g.find("[ Done ]")
    if not top or not foot:
        raise SystemExit("FAIL: Preview's sheet is not up")
    listed = 0
    for i in range(top[0] + 1, foot[0]):
        # The sheet's own cells: the queue panel shows on past its border.
        inside = g.text()[i][top[1]:top[1] + 62]
        if inside.strip():
            listed += 1
    print(f"preview picks listed: {listed}", flush=True)
    if listed != 3:
        raise SystemExit("FAIL: Preview's sheet does not list three picks")
    done(g)
    if not g.find("5 tracks"):
        raise SystemExit("FAIL: Preview queued something")

    # Batch: a cell a song — a click on the bar's first cell is one song, the
    # sheet's border follows the draft, and Done writes the row.
    press_row(g, "Batch")
    g.wait_for("─ Batch · 4 songs", 5, "Batch's sheet, its value on the border")
    bar = g.find("▰▰▰▰▱")
    if not bar:
        g.dump("looking for Batch's bar")
        raise SystemExit("FAIL: no bar in Batch's sheet")
    g.click(bar[0], bar[1])
    g.wait_for("─ Batch · 1 song", 5, "the border following the draft")
    row_reads(g, "Batch", "4 songs")  # the room waits for Done
    done(g)
    row_reads(g, "Batch", "1 song ▸")

    # Length, typed to the second: the Shortest bound, then cleared again.
    press_row(g, "Length")
    g.wait_for("Shortest", 5, "Length's sheet")
    g.click_text("Shortest", 5)
    for ch in "1:30":
        g.key(ch)
    g.wait_for("─ Length · 1:30+", 5, "the typed bound on the border")
    g.dump("Length's sheet, a bound typed")
    done(g)
    row_reads(g, "Length", "1:30+ ▸")
    press_row(g, "Length")
    clear(g)
    g.wait_for("─ Length · Any", 5, "the sheet cleared")
    done(g)
    row_reads(g, "Length", "Any ▸")

    # Genres: the DJ's library's own list, a genre ticked, the row naming
    # it; then cleared.
    press_row(g, "Genres")
    g.wait_for("alternativehiphop", 15, "B's genres in the sheet")
    g.click_text("chillhop", 5)
    g.wait_for("─ Genres · chillhop", 5, "the ticked genre on the border")
    g.dump("Genres' sheet, one ticked")
    done(g)
    row_reads(g, "Genres", "chillhop ▸")
    press_row(g, "Genres")
    g.wait_for("alternativehiphop", 15)
    clear(g)
    g.wait_for("─ Genres · Any", 5, "the sheet cleared")
    done(g)
    row_reads(g, "Genres", "Any ▸")

    # Skip words: the kit's text input with `+` beside it — a word typed, a
    # click on the plus adding it, the row naming it; then cleared.
    press_row(g, "Skip words")
    g.wait_for("│ + │", 5, "Skip words' card and its plus")
    for ch in "live":
        g.key(ch)
    plus = g.find("│ + │")
    g.click(plus[0], plus[1] + 2)
    g.wait_for("─ Skip words · live", 5, "the word the plus added, on the border")
    g.dump("Skip words' sheet, a word added by the plus")
    done(g)
    row_reads(g, "Skip words", "live ▸")
    press_row(g, "Skip words")
    g.wait_for("│ + │", 5)
    clear(g)
    g.wait_for("─ Skip words · Off", 5, "the sheet cleared")
    done(g)
    row_reads(g, "Skip words", "Off ▸")

    # First song: a list under its row, a click applies.
    press_row(g, "First song")
    g.wait_for("Choose", 5, "First song's list")
    g.click_text("Random", 5)
    row_reads(g, "First song", "Random ▾")
    press_row(g, "First song")
    g.wait_for("Choose", 5)
    g.click_text("• Random", 5, offset=-20)  # anywhere else closes and keeps
    g.pump(0.4)
    row_reads(g, "First song", "Random ▾")

    # Quick config: the chevron half of the bar's button, the popover
    # standing on the bar with the room's door at its foot.
    chevron = g.find("│ ▴ │")
    if not chevron:
        g.dump("looking for the bar's chevron")
        raise SystemExit("FAIL: no chevron on the bar's Auto DJ button")
    g.click(chevron[0], chevron[1] + 2)
    g.wait_for("All Auto DJ settings", 5, "quick config")
    g.dump("quick config")
    if not g.find("│ ▾ │"):
        raise SystemExit("FAIL: the chevron did not turn while quick config is up")
    g.key("\x1b")
    g.pump(0.4)
    if g.find("All Auto DJ settings"):
        raise SystemExit("FAIL: Esc did not close quick config")

    # Play from: the servers, a peer under its parent. Rig A — the peer
    # reached through B — ticked and Done: the DJ starts afresh there, and
    # the rating row, which a peer's key has no say over, says so in place.
    press_row(g, "Play from")
    g.wait_for("└ Rig A", 5, "Play from's sheet")
    g.dump("Play from's sheet")
    g.click_text("└ Rig A", 5)
    g.wait_for("─ Play from · Rig A", 5, "the ticked server on the border")
    done(g)
    row_reads(g, "Play from", "Rig A ▸", 15)
    g.wait_for("picking from Rig A", 15, "the note naming Rig A")
    row_reads(g, "Rating", "Not on peers", 15)
    g.dump("moved to Rig A")

    # Skip to the last row: the DJ's next turn is Rig A's, through the
    # proxy — one song now.
    for _ in range(4):
        g.key("n"); g.pump(1.5)
    # The queue panel shows a window of its rows, so the badge count on
    # screen is not the queue's: the header's count is.
    for _ in range(60):
        g.pump(0.5)
        if g.find("6 tracks"):
            break
    g.pump(1.0)
    g.dump("a pick from Rig A landed")
    if not g.find("6 tracks"):
        raise SystemExit("FAIL: no pick landed from Rig A")

    # Stop: the room says Start again, the badge stays with the rows.
    press_row(g, "Stop")
    room_wait(g, "Start ▸", 10, "Start again")
    if badges(g) < 1 or not g.find("6 tracks"):
        raise SystemExit("FAIL: the rows or their badges went with the DJ")
    g.dump("stopped")

    # The bar's button, B still the server browsed: Start goes to Play
    # from's home — Rig A — and the same button stops it.
    word = g.find("│ auto-dj │")
    if not word:
        raise SystemExit("FAIL: no auto-dj button on the bar")
    g.click(word[0], word[1] + 3)
    g.wait_for("picking from Rig A", 15, "the bar's Start on Play from's home")
    room_wait(g, "Stop", 5)
    g.click(word[0], word[1] + 3)
    g.wait_for("Auto DJ off", 10, "the bar's Stop")
    room_wait(g, "Start ▸", 5)
    g.quit()
    # The DJ's second voice (clause 63) speaks only when something is
    # dropped, learned, degraded, deferred or failed — a clean run says
    # nothing.
    print("───── [dj] log lines ─────", flush=True)
    with open(log) as f:
        lines = [line.rstrip()[:200] for line in f if "[dj]" in line]
    for line in lines:
        print(line, flush=True)
    if not lines:
        print("(none — nothing was dropped, learned, degraded, deferred or failed)", flush=True)
