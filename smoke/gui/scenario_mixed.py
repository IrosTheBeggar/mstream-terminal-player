import os, time, json, subprocess
# The rig directory (the two servers' configs and logs, the seed config) and
# the player binary come from the environment; see README.md.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def tail(path, pattern, n=12):
    out = subprocess.run(["grep", "-E", pattern, path], capture_output=True, text=True).stdout.splitlines()
    return out[-n:]

def open_dropdown(g):
    # The header's server name is the click target; it ends with " ▾".
    for i in range(3):
        line = g.screen.display[0]
        j = line.rfind(" ▾")
        if j > 0:
            g.click(0, j - 2)
            g.pump(0.4)
            return
        g.pump(0.5)
    raise SystemExit("no header dropdown marker")

def hover(g, row, col):
    os.write(g.fd, f"\x1b[<35;{col+1};{row+1}M".encode()); g.pump(0.3)

def run(Gui):
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    code = json.load(open(os.path.join(R, "qc-code.json")))["code"]
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-mixed.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(1.0)

    # 1. Pair B a second time, as a Quick Connect server.
    open_dropdown(g)
    g.click_text("Add a server", 10)
    g.wait_for("Quick Connect", 10, "the add chooser")
    g.dump("add chooser")
    g.click_text("Quick Connect", 5)
    g.pump(1.0)
    g.dump("quick connect page")
    g.key(code); g.pump(0.5)
    g.key("\r")
    g.wait_for("quick connect ·", 60, "the header naming the tunnel session")
    g.pump(2.0)
    g.dump("connected over the tunnel")

    # 2. Queue a track from B over the tunnel.
    g.wait_for("Boukmanflow", 20)
    g.click_text("Boukmanflow", 10)
    g.wait_for("6AM", 20)
    g.key("\x1b[B"); g.pump(0.2)   # Down: past '..'
    g.key("a"); g.pump(1.0)
    g.dump("one row from the tunnel server queued")

    # 3. Switch to Rig A (through the standard parent), let it go direct, queue a track.
    open_dropdown(g)
    g.wait_for("Rig A", 10)
    g.dump("dropdown with both parents")
    # Two parents list the peer now; take it through the standard one.
    g.click_text("Rig A · via http", 5)
    g.wait_for("Rig A via http", 30, "the peer through the standard parent")
    g.pump(12.0)
    g.wait_for("Boukmanflow", 20)
    g.click_text("Boukmanflow", 10)
    g.wait_for("Been a While", 20)
    g.key("\x1b[B"); g.pump(0.2); g.key("\x1b[B"); g.pump(0.2)
    g.key("a"); g.pump(1.0)
    g.dump("two rows queued, from two servers")

    # 4. Switch to the standard server: both tunnels stay for the queue.
    open_dropdown(g)
    g.wait_for("Add a server", 10, "the dropdown")
    # The standard server's own row: the dropdown line naming it plainly,
    # not the header and not a peer's "via" line.
    target = None
    for i in range(1, 12):
        line = g.screen.display[i]
        if "http://127.0.0.1:3041" in line and "via" not in line and "quick connect" not in line:
            target = (i, line.find("http://127.0.0.1:3041")); break
    if not target:
        g.dump("no plain row"); raise SystemExit("no standard-server row in the dropdown")
    g.click(*target)
    g.wait_for("connected to http://127.0.0.1:3041", 30, "back on the standard server")
    g.pump(14.0)
    print("── after the switch and the grace:", flush=True)
    for line in tail(log, "tunnel", 20): print("  " + line[24:200], flush=True)
    g.click_text("Settings", 5); g.click_text("Manage servers", 10); g.pump(1.0)
    g.dump("the room after the switch")
    g.key("\x1b"); g.pump(0.3); g.key("\x1b"); g.pump(0.3); g.key("1"); g.pump(0.5)

    # 5. Play the queue: the first row plays from the tunnel server's loopback.
    g.key(" "); g.pump(6.0)
    g.dump("playing the first queued row")

    # 6. Remove both rows: hover a queue row, click its [x]; wait past the grace.
    for _ in range(2):
        hit = g.find("▸ ") if False else None
        # The queue rows sit under the QUEUE header; hover the first row.
        qrow, qcol = g.wait_for("QUEUE", 5)
        hover(g, qrow + 2, qcol + 10)
        g.dump("hovering the first queue row")
        x = g.find("[x]", qrow + 1)
        if not x:
            raise SystemExit("no [x] on hover")
        g.click(x[0], x[1] + 1); g.pump(1.0)
    g.dump("queue emptied")
    g.pump(14.0)
    print("── after the rows left and the grace:", flush=True)
    for line in tail(log, "tunnel", 30): print("  " + line[24:200], flush=True)
    print("── A: guest connections", flush=True)
    for line in tail(os.path.join(R, "a", "server.log"), "guest connection|authorized \\(guest", 6): print("  " + line[:160], flush=True)
    print("── B: iroh tunnel connections", flush=True)
    for line in tail(os.path.join(R, "b", "server.log"), "\\[iroh\\]", 6): print("  " + line[:160], flush=True)
    g.click_text("Settings", 5); g.click_text("Manage servers", 10); g.pump(1.0)
    g.dump("the room at the end")
    print("quit status", g.quit(), flush=True)
