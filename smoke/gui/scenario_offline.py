import os, shutil
# A switch to a server that will not answer (multi-server contract, clause
# 13): the header keeps the label and the picker, the room says the server
# is offline with "Try again", the queue's transport keeps working from the
# rows' own server, and the picker leads back. Needs only B; the dead
# server is a port nothing listens on.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))
DEAD = "http://127.0.0.1:3999"

def open_dropdown(g):
    for _ in range(3):
        line = g.screen.display[0]
        j = line.rfind(" ▾")
        if j > 0:
            g.click(0, j - 2); g.pump(0.4)
            return
        g.pump(0.5)
    raise SystemExit("FAIL: no header dropdown marker")

def run(Gui):
    # A copy of the seed config with the dead server added, so the other
    # scenarios' seed stays as it is.
    cfg = os.path.join(R, "gui-config-offline")
    shutil.rmtree(cfg, ignore_errors=True)
    shutil.copytree(os.path.join(R, "gui-config"), cfg)
    with open(os.path.join(cfg, "config.toml"), "a") as f:
        f.write(f'\n[[server]]\nurl = "{DEAD}"\n')
    # An empty queue, so "queue all" below starts playback (a restored queue
    # would come back paused at its spot and the add would only append).
    for stale in ("queue.json",):
        try:
            os.remove(os.path.join(cfg, stale))
        except FileNotFoundError:
            pass
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": cfg,
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-offline.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(0.5)

    # Queue B's folder so the transport has rows to drive.
    g.click_text("Boukmanflow", 10)
    g.wait_for("6AM", 20)
    g.key("A"); g.pump(2.0)

    # Pick the dead server from the header.
    open_dropdown(g)
    g.click_text("127.0.0.1:3999", 5)
    g.wait_for("is offline", 20, "the room saying the server is offline")
    g.pump(0.5)
    g.dump("the dead server picked")
    if " ▾" not in g.screen.display[0]:
        raise SystemExit("FAIL: the header lost its picker")
    if not g.find("Try again"):
        raise SystemExit("FAIL: no Try again")

    # The transport: queueing started the first row (an empty queue starts
    # playing on Add all), so Space must pause it and Space again resume it
    # — both through the App's action funnel, which used to swallow them
    # while no session was up.
    def elapsed():
        for line in g.text():
            if "━" in line and ":" in line:
                return line.split()[0]
        return None
    before = elapsed(); g.pump(2.0); moving = elapsed()
    print(f"elapsed before Space: {before} → {moving}", flush=True)
    if before is None or moving == before:
        raise SystemExit("FAIL: the queued row is not playing before the test starts")
    g.key(" "); g.pump(2.0); paused = elapsed(); g.pump(2.5); still = elapsed()
    print(f"after Space: {paused} → {still} (should hold)", flush=True)
    g.dump("Space while the session is down")
    if paused != still:
        raise SystemExit("FAIL: Space did not pause — the transport is dead while disconnected")
    g.key(" "); g.pump(3.0); resumed = elapsed()
    print(f"after Space again: {still} → {resumed} (should move)", flush=True)
    if resumed == still:
        raise SystemExit("FAIL: Space did not resume")

    # The picker leads back to the working server.
    open_dropdown(g)
    g.click_text("127.0.0.1:3041", 5)
    g.wait_for("Boukmanflow", 30, "B's library again")
    g.dump("back on B")
    g.quit()
