import os, shutil
# Track actions on the rig (track-actions contract): a Files row's [⋯] opens
# the sheet naming the track; a star rates it and the server keeps it; Add
# to playlist through New playlist lands "Added to …" and the Playlists
# room lists it; Song Info opens; the queue's clear empties. Needs B alone.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def need(g, needle, what):
    if not g.find(needle):
        g.dump(f"looking for {what}")
        raise SystemExit(f"FAIL: no {what!r} ({needle!r})")

def run(Gui):
    cfg = os.path.join(R, "gui-config-actions")
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
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-actions.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(0.5)
    g.click_text("Boukmanflow", 10)
    g.wait_for("Been a While", 20)
    g.pump(0.3)

    # Hover the second track: the fourth verb opens the sheet.
    row, col = g.find("Been a While")
    os.write(g.fd, f"\x1b[<35;{col + 5};{row + 1}M".encode()); g.pump(0.4)   # SGR motion: hover
    g.dump("hovering a track row")
    need(g, "[⋯]", "the fourth hover verb")
    g.click_text("[⋯]", 5)
    g.wait_for("Play now", 10, "the sheet")
    g.pump(1.0)
    g.dump("the sheet")
    need(g, "Been a While", "the sheet naming the track")
    need(g, "Add to playlist", "the playlist row")
    need(g, "Song Info", "the info row")
    need(g, "☆", "the stars")

    # Rate it four stars: the badge changes at once and no refusal follows.
    # The rig keeps ratings between runs, and a click on the star that IS
    # the rating clears it (clause 10) — so a track already at four stars
    # clears on the first click and takes the second.
    srow, scol = g.find("☆") or g.find("★")
    star_row = g.screen.display[srow]
    first = star_row.find("★") if "★" in star_row else star_row.find("☆")
    g.click(srow, first + 3); g.pump(2.0)
    if g.find("☆☆☆☆☆"):
        g.dump("cleared a standing rating first")
        g.click(srow, first + 3); g.pump(2.0)
    g.dump("rated")
    need(g, "★★★★☆ 4", "four stars")
    if g.find("Could not save rating"):
        raise SystemExit("FAIL: the server refused the rating")

    # Song Info lists the facts, Esc returns to the sheet.
    g.click_text("Song Info", 5)
    g.wait_for("Bitrate", 10, "Song Info")
    g.dump("Song Info")
    need(g, "Path", "the path row")
    g.key("\x1b"); g.pump(0.5)
    need(g, "Add to playlist", "the sheet again")

    # Add to a new playlist by name.
    g.click_text("Add to playlist", 5)
    g.wait_for("New playlist", 10, "the picker")
    g.click_text("New playlist", 5)
    g.pump(0.4)
    g.key("Rig mix"); g.pump(0.3)
    g.key("\r")
    g.wait_for("Added to Rig mix", 15, "the add's note")
    g.dump("added")

    # The Playlists room knows the new name.
    g.key("6")
    g.wait_for("Rig mix", 15, "the playlists room listing it")
    g.dump("Playlists")

    # Back to Files: queue the folder, then the header's clear empties it.
    g.key("1"); g.pump(0.5)
    g.key("A"); g.pump(2.0)
    need(g, "tracks", "a queued folder")
    g.click_text("clear", 5); g.pump(1.0)
    g.dump("cleared")
    need(g, "nothing queued yet", "the empty queue")
    g.quit()
