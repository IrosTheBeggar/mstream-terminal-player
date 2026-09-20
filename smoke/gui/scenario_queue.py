import os
# The queue panel against Rig B alone: rows two cells tall, the cover on
# the left (the ▀-mosaic on this pty — it negotiates no pixel protocol),
# the artist under the title, the length on the second line, the hover
# [x] in its place, the playing row in colour rather than marked.
# Needs only B (`node cli-boot-wrapper.js -j $R/b/config.json`).
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def hover(g, row, col):
    os.write(g.fd, f"\x1b[<35;{col+1};{row+1}M".encode()); g.pump(0.3)

def run(Gui):
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-queue.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(1.0)

    # Queue the whole folder: the bar's "queue all" is A on the keyboard.
    g.click_text("Boukmanflow", 10)
    g.wait_for("6AM", 20)
    g.key("A"); g.pump(3.0)
    g.dump("the folder queued")

    # Hover the first row: the [x] takes the length's place on its second line.
    qrow, qcol = g.wait_for("QUEUE", 5)
    hover(g, qrow + 2, qcol + 10)
    g.dump("hovering the first row")
    x = g.find("[x]", qrow + 1)
    print(f"[x] at {x} (the header is at row {qrow}; the first row's lines are {qrow+2} and {qrow+3})", flush=True)
    hover(g, qrow + 20, 10)  # away

    # Play: the playing row is told by colour, not a mark.
    g.key(" "); g.pump(5.0)
    g.dump("playing the first row")
    # What the rows wear, read off the screen: the panel's left edge is the
    # QUEUE title's column, each row two lines from two below it.
    top = qrow + 2
    firsts = [g.screen.display[i][qcol:qcol + 4] for i in range(top, len(g.screen.display) - 9, 2)]
    covers = sum(1 for cells in firsts if cells and set(cells) <= set("█▀▄"))
    frames = sum(1 for cells in firsts if cells.startswith("╭"))
    print(f"rows visible: {covers} wear a cover, {frames} the empty slot frame", flush=True)
    if covers == 0:
        raise SystemExit("FAIL: no row drew its cover")
    g.quit()
