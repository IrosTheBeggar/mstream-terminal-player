import os
# The Now Playing screen against Rig B alone (now-playing contract): `0`
# brings the TUI's full-screen view under the top bar — the facts column
# with the track's title, the tab strip "[1:Queue]", the band's
# "position / total" — with prev · play · next under the cover in the bar's
# frames; the GUI's bar (its "auto-dj" frame) is absent while the screen is
# up. `▸▸` steps to the next queued row, a click on the band seeks, a click
# on "2:Auto-DJ" opens that tab, and `0` goes back to the Library.
# Needs only B (`node cli-boot-wrapper.js -j $R/b/config.json`).
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def run(Gui):
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-now.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(1.0)

    # Queue the folder and play it: the bar's "queue all" is A on the
    # keyboard (it queues, it does not start), Space starts the first row.
    g.click_text("Boukmanflow", 10)
    g.wait_for("6AM", 20)
    g.key("A"); g.pump(2.0)
    g.key(" "); g.pump(4.0)
    if not g.find("auto-dj"):
        g.dump("the Library screen"); raise SystemExit("FAIL: the bar's auto-dj frame is missing on the Library screen")
    if not g.find("▮▮"):
        g.dump("after Space"); raise SystemExit("FAIL: the first row did not start playing")
    g.dump("playing the first row")

    # 0: the TUI's view under the top bar — the strip, the card, the band —
    # and no GUI bar.
    g.key("0"); g.pump(1.5)
    g.dump("the Now Playing screen")
    g.wait_for("[1:Queue]", 5, "the tab strip")
    if g.find("auto-dj"):
        raise SystemExit("FAIL: the GUI's bar is still drawn on the Now Playing screen")
    band = g.find(" / ")
    if not band:
        raise SystemExit("FAIL: no 'position / total' on the band")
    if not (g.find("▸▸") and g.find("◂◂")):
        raise SystemExit("FAIL: no prev · next frames under the cover")
    if not g.find("▮▮"):
        raise SystemExit("FAIL: the play frame does not show ▮▮ while playing")

    # ▸▸ plays the next row: the card's title (the facts column, at the
    # left — the queue tab at the right names the row too) changes.
    def card_title():
        # The facts card: the title stands on the row above the artist's.
        for i in range(2, 12):
            if "Boukmanflow" in g.screen.display[i][:45]:
                return g.screen.display[i - 1][:45].strip()
        return None
    before_title = card_title()
    if not before_title:
        g.dump("no card title"); raise SystemExit("FAIL: the facts column does not name the playing track")
    g.click_text("▸▸", 5, 1); g.pump(3.0)
    g.dump("after ▸▸")
    if card_title() == before_title:
        raise SystemExit("FAIL: ▸▸ did not move to the next row")

    # A click on the band seeks: the position jumps well past where it was.
    brow, bcol = g.find(" / ")
    before = g.screen.display[brow][bcol - 5:bcol].strip()
    g.click(brow, 40); g.pump(2.0)
    brow2, bcol2 = g.find(" / ")
    after = g.screen.display[brow2][bcol2 - 5:bcol2].strip()
    print(f"band: {before!r} → {after!r} after a click on column 40", flush=True)
    if after == before or after.startswith("0:0"):
        g.dump("after the seek"); raise SystemExit("FAIL: the band did not seek")

    # The strip's tabs click: Auto-DJ's rows stand in the panel.
    g.click_text("2:Auto-DJ", 5, 1); g.pump(1.0)
    g.dump("the Auto-DJ tab")
    if not g.find("[2:Auto-DJ]"):
        raise SystemExit("FAIL: the tab click did not pick Auto-DJ")

    # 0 goes back: the nav column and the bar return.
    g.key("0"); g.pump(1.0)
    g.wait_for("Albums", 5, "the nav column back")
    if not g.find("auto-dj"):
        raise SystemExit("FAIL: the bar did not come back with the Library")
    g.quit()
