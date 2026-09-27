import os, time, subprocess
# The rig directory (the two servers' configs and logs, the seed config) and
# the player binary come from the environment; see README.md.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def tail(path, pattern, n=12):
    try:
        out = subprocess.run(["grep", "-E", pattern, path], capture_output=True, text=True).stdout.splitlines()
        return out[-n:]
    except Exception as e:
        return [str(e)]

def run(Gui):
    log = os.path.join(R, "gui.log")
    open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui.raw"))
    t0 = time.time()
    g.wait_for("demo", 40, "B's library in the files pane")
    g.pump(1.0)
    g.dump(f"connected to B after {time.time()-t0:.1f}s")

    # The header dropdown: switch to the peer.
    row, col = g.find("127.0.0.1:3041") or (None, None)
    print(f"header server text at row {row} col {col}", flush=True)
    g.click(row, col)
    g.wait_for("Rig A", 15, "the peer in the dropdown")
    g.dump("dropdown open")
    g.click_text("Rig A")
    g.wait_for("Rig A via", 30, "the header naming the peer")
    g.pump(1.0)
    g.dump("switched to Rig A (through the proxy at first)")

    # The access ask, the dial and the retarget happen on the ticks; give
    # them a moment, then look for the mark in the dropdown.
    g.pump(12.0)
    row, col = g.find("Rig A via")
    g.click(row, col)
    g.wait_for("direct ·", 20, "the direct mark")
    g.dump("dropdown shows the peer as direct")
    g.key("\x1b")  # close the dropdown
    g.pump(0.5)
    # The room too.
    g.click_text("Settings", 5)
    g.click_text("Manage servers", 10)
    g.wait_for("direct ·", 10, "the mark in the room")
    g.dump("the servers room")
    g.key("\x1b"); g.pump(0.3); g.key("\x1b"); g.pump(0.3)
    g.key("1"); g.pump(0.5)  # back to Files

    # Browse A and play a track: the peer's root lists the shared library's
    # folders directly.
    g.wait_for("Boukmanflow", 20)
    g.click_text("Boukmanflow", 10)
    g.wait_for("6AM", 20)
    g.click_text("6AM", 10)
    g.pump(6.0)
    g.dump("playing from Rig A")
    bar = " ".join(g.text()[29:33])
    print("BAR:", " ".join(bar.split())[:200], flush=True)

    print("── gui log: tunnel lines", flush=True)
    for line in tail(log, "tunnel|Renew|direct|Retarget|guest", 20): print("  " + line[:180], flush=True)
    print("── A: federation handshakes", flush=True)
    for line in tail(os.path.join(R, "a", "server.log"), r"\[federation\] handshake|guest", 8): print("  " + line[:200], flush=True)
    print("── B: access route / guest mint", flush=True)
    for line in tail(os.path.join(R, "b", "server.log"), r"guest|access", 6): print("  " + line[:200], flush=True)
    status = g.quit()
    print(f"quit status {status}", flush=True)
