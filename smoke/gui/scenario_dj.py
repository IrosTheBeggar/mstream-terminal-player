import os, shutil
# Auto DJ on the rig (auto-dj contract, slice A5): the room from the nav,
# Start on a one-row queue, the pick landing with its badge, Preview, and
# the server picker moving the DJ to the proxied peer (Rig A via B) with the
# next turn picked from there. The rig's servers have no discovery data, so
# the sonic row shows its reason and the picks stay random (∞, never ≈).
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def badges(g):
    return sum(line.count("∞") + line.count("≈") for line in g.text())

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
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": cfg,
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-dj.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(0.5)

    # One row in the queue, idle (a click on a track would queue the folder
    # and play from it): the cursor onto 6AM, `a` queues just that row. The
    # DJ then opens on it — a one-row idle queue is the queue's end.
    g.click_text("Boukmanflow", 10)
    g.wait_for("6AM", 20)
    # The pane's cursor is not visible in text (a painted row), and it may
    # rest on the up-link: walk down until `a` queues a row.
    for _ in range(4):
        g.key("\x1b[B"); g.pump(0.3)
        g.key("a"); g.pump(1.0)
        if g.find("1 track"):
            break
    if not g.find("1 track"):
        g.dump("after queueing one track")
        raise SystemExit("FAIL: expected a one-row queue")

    # The nav's Auto DJ row under TOOLS — the room, off, then the probe's
    # reason on the sonic row (the rig has no discovery data).
    g.wait_for("TOOLS", 10, "the nav's TOOLS group")
    g.click_text("Auto DJ", 5)
    g.wait_for("Auto DJ is off", 10, "the room")
    g.wait_for("have discovery", 15, "the sonic reason from the probe")
    g.dump("the room, off")

    # Start: armed for B, the note names the server (several are saved), and
    # with the playing row the last one a turn goes out at once.
    g.click_text("Start Auto DJ", 5)
    g.wait_for("Auto DJ is on", 10, "the room saying on")
    g.wait_for("picking from", 10, "the state line")
    g.wait_for("∞", 40, "a DJ pick with its badge in the queue panel")
    g.pump(1.0)
    g.dump("started; the first pick landed")
    # A batch: songs per fetch is 4, so four rows land at once, each badged.
    n1 = badges(g)
    print(f"badged rows: {n1}", flush=True)
    if n1 < 4:
        raise SystemExit("FAIL: expected a batch of four badged rows")
    if not g.find("5 tracks"):
        raise SystemExit("FAIL: the queue did not grow by the batch")

    # Preview: three picks under the row, none of them queued. The row is
    # at the body's end — PageDown scrolls the room.
    for _ in range(3):
        g.key("\x1b[6~"); g.pump(0.3)
    g.click_text("Preview picks", 5)
    g.pump(6.0)
    g.dump("after Preview")
    hit = g.find("Preview picks")
    if not hit:
        raise SystemExit("FAIL: the Preview row vanished")
    row = hit[0]
    lines = g.text()
    listed = 0
    for i in range(row + 1, min(row + 4, len(lines))):
        # The content column: between the nav's rule and the queue's.
        cells = lines[i].split("│")
        body = cells[1] if len(cells) > 1 else ""
        if body.strip(" ▲▼█│"):
            listed += 1
        else:
            break
    print(f"preview rows listed under the row: {listed}", flush=True)
    if listed < 3:
        raise SystemExit("FAIL: fewer than three preview picks listed")
    if not g.find("5 tracks"):
        raise SystemExit("FAIL: Preview queued something")

    # Songs per fetch down to 1 by a click on the bar's first cell, live
    # (the volume widget's cells; four of twenty-five songs fill one).
    for _ in range(3):
        g.key("\x1b[5~"); g.pump(0.3)
    hit = g.find("▰▱▱▱▱▱▱▱▱▱")
    if not hit:
        g.dump("looking for the songs-per-fetch bar")
        raise SystemExit("FAIL: no songs-per-fetch bar")
    g.click(hit[0], hit[1])
    g.wait_for("1 song", 5, "the bar set to one song")

    # The server picker: move the DJ to Rig A, the peer reached through B.
    g.click_text("3041 ▸", 5)
    g.wait_for("Auto DJ picks from", 10, "the server picker")
    g.dump("the server picker")
    g.click_text("Rig A", 5)
    g.wait_for("picking from Rig A", 15, "the state line naming Rig A")
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
    if not g.find("Auto DJ: "):
        raise SystemExit("FAIL: the pick was not announced")

    # Stop: the room says off, the badge stays with the rows.
    g.click_text("Stop Auto DJ", 5)
    g.wait_for("Auto DJ is off", 10, "off again")
    if badges(g) < 1 or not g.find("6 tracks"):
        raise SystemExit("FAIL: the rows or their badges went with the DJ")
    g.dump("stopped")
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
