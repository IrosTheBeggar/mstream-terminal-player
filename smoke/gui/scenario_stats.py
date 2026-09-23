import os, shutil
# Play reporting on the rig (play-reporting contract): one row queued and
# played past thirty seconds, then skipped — the play is posted to B (the
# `[stats]` log line says so) and the track stands in Last played and Most
# played. Needs B alone; B counts plays for its implicit user.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def run(Gui):
    cfg = os.path.join(R, "gui-config-stats")
    shutil.rmtree(cfg, ignore_errors=True)
    shutil.copytree(os.path.join(R, "gui-config"), cfg)
    for stale in ("queue.json", "stats.json"):
        try:
            os.remove(os.path.join(cfg, stale))
        except FileNotFoundError:
            pass
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": cfg,
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-stats.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(0.5)

    # One row in the queue: walk into the folder and add the row under the
    # cursor until the header says so (the cursor may rest on `..`).
    g.click_text("Boukmanflow", 10)
    g.pump(1.0)
    for _ in range(4):
        g.key("\x1b[B"); g.pump(0.3)
        g.key("a"); g.pump(1.0)
        if g.find("1 track"):
            break
    if not g.find("1 track"):
        g.dump("after queueing one track")
        raise SystemExit("FAIL: expected a one-row queue")
    # Adding to an empty queue starts it; Space here would pause. Play only
    # if the bar shows the play glyph rather than the pause glyph.
    g.pump(1.0)
    if not g.find("▮▮"):
        g.key(" ")
        g.wait_for("▮▮", 15, "the bar's pause glyph, so playback is on")
    # Past the thirty seconds a play needs to count, then move on.
    g.pump(34.0)
    if not g.find("▮▮"):
        g.dump("still playing?")
        raise SystemExit("FAIL: playback did not stay on for the thirty seconds")
    g.key("n"); g.pump(3.0)
    g.dump("after the skip")
    with open(log, encoding="utf-8", errors="replace") as f:
        lines = [l for l in f if "[stats]" in l]
    if not any("posted" in l for l in lines):
        raise SystemExit("FAIL: no play was posted; the [stats] lines were: %r" % lines)

    # The lists: the track stands in both.
    for label in ("Last played", "Most played"):
        g.click_text(label, 5)
        g.wait_for("▸ " + label, 10, f"{label} active in the nav")
        g.pump(2.0)
        g.dump(label)
        if g.find("Nothing here yet") or not g.find("1 item"):
            raise SystemExit(f"FAIL: {label} does not list the play")
    g.quit()
