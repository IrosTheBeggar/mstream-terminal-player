import os, subprocess, time
# The visualizer window against Rig B alone (visualizer-window contract): a
# row playing, `V` spawns the child `mstream-player viz-window` and the top
# bar's Visualizer item lights; a second `V` is a raise (the child stays
# one); `q` closes the player and the child goes with it within a moment.
# The window itself is a real window on this machine's display — the
# scenario proves the process and the pipe, not the pixels (screenshots are
# the manual step: the child's window is titled "mStream Visualizer — …").
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def children():
    # Anchored to the child's own command line: a shell whose script merely
    # mentions the subcommand must not count as a child.
    out = subprocess.run(["pgrep", "-f", r"/mstream-player viz-window$"], capture_output=True, text=True).stdout.split()
    return [pid for pid in out if pid.strip()]

def run(Gui):
    if children():
        raise SystemExit("FAIL: a viz-window child is already running; close it first")
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-vizwindow.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(1.0)
    g.click_text("Boukmanflow", 10)
    g.wait_for("6AM", 20)
    g.key("A"); g.pump(2.0)
    g.key(" "); g.pump(3.0)
    if not g.find("▮▮"):
        g.dump("after Space"); raise SystemExit("FAIL: the first row did not start playing")

    g.key("V"); g.pump(4.0)
    g.dump("after V")
    kids = children()
    if len(kids) != 1:
        raise SystemExit(f"FAIL: expected one viz-window child, found {kids}")
    g.key("V"); g.pump(1.5)
    if len(children()) != 1:
        raise SystemExit("FAIL: a second V spawned another window instead of raising the one open")

    g.quit()
    for _ in range(40):
        if not children():
            break
        time.sleep(0.1)
    if children():
        raise SystemExit("FAIL: the viz-window child outlived the player")
