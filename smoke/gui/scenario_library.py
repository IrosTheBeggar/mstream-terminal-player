import os, shutil
# The Library rooms on the rig (library-rooms contract): Artists as a list,
# an artist as a wall of its albums, a card into its tracks with the artist
# as the way back, Genres with their counts into a genre's tracks under the
# bar's verbs, and Recent as a track list. Needs B alone.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

def need(g, needle, what):
    if not g.find(needle):
        g.dump(f"looking for {what}")
        raise SystemExit(f"FAIL: no {what!r} ({needle!r})")

def run(Gui):
    cfg = os.path.join(R, "gui-config-library")
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
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-library.raw"))
    g.wait_for("Boukmanflow", 40, "B's library")
    g.pump(0.5)

    # Artists: the names, no play verb on a list of containers.
    g.key("3")
    g.wait_for("Codec Zoo", 15, "the artists list")
    g.pump(0.5)
    g.dump("Artists")
    need(g, "4 items", "the count")
    if g.find("▸ play"):
        raise SystemExit("FAIL: a list of artists offers play")

    # An artist opens as a wall; its header leads back.
    g.click_text("Boukmanflow", 5)
    g.wait_for("▸ Boukmanflow", 15, "the artist's wall header")
    g.wait_for("albums", 10, "the wall's count")
    g.pump(1.5)
    g.dump("the artist's wall")
    need(g, "◂ Artists", "the way back")

    # A card opens its tracks, with the artist as the way back.
    row, col = g.find("◂ Artists")
    # The first card's name sits under the first cover: click the cell.
    g.click(row + 3 + 6, col)
    g.wait_for("◂ Boukmanflow", 15, "the drilled album naming the artist as Back")
    g.pump(0.5)
    g.dump("the album, from the artist")
    need(g, "▸ play", "the verbs on a track list")
    g.key("\x1b"); g.pump(0.6)
    need(g, "▸ Boukmanflow", "the wall again after Esc")
    g.key("\x1b"); g.pump(0.6)
    need(g, "Codec Zoo", "the artists again after Esc")

    # Genres: counts, then a genre's tracks under the verbs.
    g.key("4")
    g.wait_for("chillhop", 15, "the genres list")
    g.pump(0.5)
    g.dump("Genres")
    need(g, "(5)", "a genre's count")
    g.click_text("chillhop", 5)
    g.wait_for("▸ chillhop", 15, "the genre's header")
    g.pump(0.8)
    g.dump("a genre's tracks")
    need(g, "◂ Genres", "the way back")
    need(g, "▸ play", "the verbs")

    # Recent: a track list, newest first, with the verbs.
    g.key("5")
    g.wait_for("items", 15, "the recent list's count")
    g.pump(1.0)
    g.dump("Recent")
    need(g, "▸ play", "the verbs")
    if g.find("# A B C"):
        raise SystemExit("FAIL: Recent wears a strip")
    g.quit()
