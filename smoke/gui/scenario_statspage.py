import os
# The Stats screen against Rig B alone (stats-screen contract): the top
# bar's third tab hosts the stats page whole under the bar — the state line
# with the period's totals, the Overview · Top · Recent strip — on the
# session the GUI holds (B's implicit account); `→` walks the page's tabs
# (the Top tab's SHOW controls), `T` and Esc go back to the Library, `T`
# opens it again from the Library. The GUI's bar (its "auto-dj" frame) and
# nav are absent while the screen is up. Needs only B.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def run(Gui):
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-statspage.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(1.0)

    # The tab: the page loads on entry and its state line names the period.
    g.click_text("Stats", 5)
    g.wait_for("Overview", 10, "the page's tab strip")
    g.wait_for("plays", 20, "the state line's totals")
    g.dump("the Stats screen")
    # The charts' furniture (clauses 8–9): the y axis's ticks and the
    # baseline's foot.
    if not (g.find(" ┤ ") and g.find("0 ┼─")):
        raise SystemExit("FAIL: the charts have no axis")
    if g.find("auto-dj") or g.find("LIBRARY"):
        raise SystemExit("FAIL: the GUI's bar or nav is still drawn on the Stats screen")
    # The footer is a config switch (key hints); when it is on it carries
    # the page's hint and the way back.
    if g.find("q quit") and not g.find("Esc library"):
        raise SystemExit("FAIL: the footer does not name the way back after the page's hint")

    # → is the page's: the Top tab and its controls.
    g.key("\x1b[C"); g.pump(2.0)
    g.wait_for("SHOW", 10, "the Top tab's controls")
    g.dump("the Top tab")

    # Esc goes back to the Library; T opens the screen again and T closes it.
    g.key("\x1b"); g.pump(1.0)
    g.wait_for("LIBRARY", 5, "the nav column, back")
    g.key("T"); g.pump(1.5)
    g.wait_for("Overview", 10, "the page again on T")
    g.key("T"); g.pump(1.0)
    g.wait_for("LIBRARY", 5, "the nav column, back again")
    g.quit()
