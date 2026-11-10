import os, json, time
# Add from YouTube against one mStream server with the scripted yt-dlp
# (docs/ux-contracts/add-from-youtube.md). The rig: a server on
# $MSTREAM_YT_PORT (default 3140) booted with MSTREAM_YTDLP_BIN pointing at
# mStream's test/helpers/fake-yt-dlp.mjs, MSTREAM_FAKE_YTDLP_SCRIPT at a
# script naming the ids below and MSTREAM_FAKE_YTDLP_FIXTURE at an mp3; a
# library "Music" holding Daft Punk/Discovery/Aerodynamic.mp3. See README.md.
#
# MSTREAM_YT_STEPS picks the passes (comma list, default all):
#   room   — the nav's Add YouTube row under TOOLS: paste, the found tags,
#            Enter, the title-row word, the From YouTube list, the note
#   refuse — the same link again (409 before downloading), a live stream,
#            YouTube's bot check (the details typed by hand, then the add fails)
#   caps   — D, T and A in the room never reach Auto DJ, Stats or the DJ
#   door   — Files › Music › Daft Punk › Discovery: the door word, y, the
#            window, a paste, Add here, the door's spot, the re-list
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))
MUSIC = os.environ.get("MSTREAM_YT_MUSIC", os.path.join(R, "music", "Music"))
STEPS = set(os.environ.get("MSTREAM_YT_STEPS", "room,refuse,caps,door").split(","))
# A server before mStream #1123 (no `ytdl` on the ping): the clash fails after
# the download, a live stream is the hedged reason, jobs are claimed by link
# and folder (the phone's rules).
OLDER = os.environ.get("MSTREAM_YT_OLDER") == "1"

ONE_MORE_TIME = "https://youtu.be/FGBhQbmPwH8"
LIVE = "https://www.youtube.com/watch?v=liveStream1"
BOT = "https://www.youtube.com/watch?v=botCheck001"
HARDER = "https://www.youtube.com/watch?v=harderBettr&list=PL123"

def paste(g, text):
    g.key(b"\x1b[200~" + text.encode() + b"\x1b[201~")

def hover(g, row, col):
    os.write(g.fd, f"\x1b[<35;{col+1};{row+1}M".encode()); g.pump(0.3)

def check(cond, what):
    print(("PASS " if cond else "FAIL ") + what, flush=True)
    if not cond:
        raise SystemExit("FAIL: " + what)

def songs():
    out = []
    for root, _, files in os.walk(MUSIC):
        for f in files:
            out.append(os.path.relpath(os.path.join(root, f), MUSIC))
    return sorted(out)

def run(Gui):
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-ytdl.raw"))
    g.wait_for("Music", 40, "the library")
    g.pump(1.0)
    print("songs before:", songs(), flush=True)

    if "room" in STEPS or "refuse" in STEPS or "caps" in STEPS:
        # Settings keeps no doorway: TORRENTS holds its switch alone.
        g.click_text("Settings", 10)
        g.wait_for("TORRENTS", 10)
        g.dump("Settings")
        check(g.find("ADD MUSIC") is None and g.find("Add from YouTube") is None, "no doorway in Settings")
        # The room is the nav's: Add YouTube closes TOOLS, under Add torrent,
        # drawn only under its gates (clauses 1, 2).
        g.wait_for("Add YouTube", 10, "the nav's Add YouTube row")
        check(g.find("Add torrent") is not None, "Add torrent above it")
        g.click_text("Add YouTube", 5)
        g.wait_for("▸ Add YouTube", 5, "the row lit")
        g.pump(1.0)
        g.dump("the room, opened")
        check(g.find("Add from YouTube") is not None and g.find("◂ Add from YouTube") is None,
              "the title row, with no way back")

    if "room" in STEPS:
        paste(g, ONE_MORE_TIME)
        g.wait_for("One More Time", 15, "the found song")
        g.pump(1.5)
        g.dump("found")
        g.key("\r")
        g.pump(0.4)
        g.dump("the press out")
        g.wait_for("added", 40, "the job's word")
        g.pump(1.0)
        g.dump("added")
        print("songs after:", songs(), flush=True)
        check(any("One_More_Time" in s or "One More Time" in s for s in songs()), "the song landed in the library")
        # The form is fresh, the link field holding the keys for the next
        # paste (E2): Esc gives them back, then l opens the list.
        g.key("\x1b"); g.pump(0.4)
        g.key("l"); g.pump(1.0)
        g.dump("the From YouTube list")
        check(g.find("From YouTube") is not None, "the list window is titled From YouTube")
        g.key("\x1b"); g.pump(0.6)
        g.dump("the list closed")

    if "refuse" in STEPS and OLDER:
        # The same link into the same folder: warned of ahead of time (C4),
        # Add anyway — the older server downloads, then won't replace.
        paste(g, ONE_MORE_TIME)
        g.wait_for("One More Time", 15)
        g.wait_for("looks to be in this folder already", 10, "the clash warned of")
        g.dump("older: the same link, the clash ahead of time")
        g.key("\r")
        g.wait_for("! failed", 60, "the after-download clash")
        g.pump(0.5)
        g.dump("older: the same link failed")
        g.key("\x1b"); g.pump(0.4)
        g.key("l"); g.pump(1.0)
        g.dump("older: the list")
        check(g.find("nothing was") is not None and g.find("Change format and retry · Dismiss") is not None,
              "older: the after-download clash row, its way on a format")
        g.key("\x1b"); g.pump(0.6)
        # A live stream: no isLive on this server, so no warning ahead of
        # time. (6.32's fake yt-dlp doesn't skip live streams, so the add
        # itself proves nothing here: not pressed.)
        paste(g, LIVE)
        g.wait_for("beats to relax", 15)
        g.pump(0.5)
        check(g.find("Live streams can't be saved") is None, "older: no live warning ahead of time")

    if "refuse" in STEPS and not OLDER:
        # The same link into the same folder: warned of ahead of time (C4),
        # then the server's 409 before anything downloads — its lines stand
        # alone over the primary.
        paste(g, ONE_MORE_TIME)
        g.wait_for("One More Time", 15)
        g.wait_for("looks to be in this folder already", 10, "the clash warned of")
        g.dump("the same link: the clash ahead of time")
        g.key("\r")
        g.wait_for("so nothing was added", 15, "the 409")
        g.pump(0.5)
        g.dump("the same link: the server refused it")
        check(g.find("looks to be in this folder already") is None, "the 409 stands for the warning it confirms")
        check(sum(1 for line in g.text() if "f change format" in line) == 1, "one f change format")
        paste(g, LIVE)
        g.wait_for("Live streams can't be saved", 15, "the live warning")
        g.pump(0.5)
        g.dump("a live stream")
        # YouTube's bot check: the lookup fails, the tags open empty (B4),
        # a title typed, Enter adds — and the add fails with the server's words.
        paste(g, BOT)
        g.wait_for("Couldn't read this video's details", 20, "the failed lookup")
        g.pump(0.5)
        g.dump("the bot check's lookup")
        for ch in "Some Song":
            g.key(ch)
        g.key("\r")
        g.wait_for("! failed", 40, "the title row's failed word")
        g.pump(0.5)
        g.dump("the bot check's add failed")
        g.key("\x1b"); g.pump(0.4)
        g.key("l"); g.pump(1.0)
        g.dump("the list, with the failure")
        check(g.find("the server said: “Sign in to confirm you're not a bot.") is not None, "the failure in the server's words")
        check(g.find("Try again · Dismiss") is not None, "the failure offers Try again and Dismiss")
        g.key("\x1b"); g.pump(0.6)

    if "caps" in STEPS:
        # Nothing holds the keys: D, T and A are the room's to swallow, never
        # Auto DJ, Stats or the DJ's toggle (decision 9). An Esc first gives
        # the field's keys back if it holds them (a caps-only run), else
        # leaves nothing: a nav room has no way back (clause 10).
        g.key("\x1b"); g.pump(0.4)
        check(g.find("Add from YouTube") is not None and g.find("▸ Add YouTube") is not None,
              "the room is up after Esc, its field not holding the keys")
        before = g.text()
        for k in "DTA":
            g.key(k); g.pump(0.5)
        g.dump("after D T A")
        check(g.find("Add from YouTube") is not None and g.find("▸ Add YouTube") is not None, "still in the room after the capitals")
        check(g.text()[30:33] == before[30:33], "the transport row untouched (no DJ toggle)")

    if "door" in STEPS:
        for _ in range(3):
            g.key("\x1b"); g.pump(0.3)
        g.click_text("Files", 10)
        g.pump(1.0)
        g.click_text("Music", 10)
        g.wait_for("Daft Punk", 10)
        g.dump("Files › Music (a library root: no door)")
        check(g.find("add from YouTube") is None, "no door at a library root")
        g.click_text("Daft Punk", 10)
        g.wait_for("Discovery", 10)
        g.click_text("Discovery", 10)
        g.wait_for("Aerodynamic", 10)
        g.pump(0.5)
        g.dump("Files › Discovery")
        check(g.find("add from YouTube") is not None, "the door word on a folder below a root")
        g.key("y"); g.pump(1.0)
        g.dump("the window")
        paste(g, HARDER)
        g.wait_for("Harder", 15, "the window's found song")
        g.pump(1.0)
        g.dump("the window, found")
        g.key("\r")
        g.pump(1.0)
        g.dump("Add here pressed")
        g.wait_for("added", 40, "the door's spot")
        g.pump(2.5)
        g.dump("the door's spot, and the re-list")
        hit = g.find("Daft_Punk_-_Harder_Better")
        check(hit is not None, "the folder re-listed with the new song")
        lit = g.screen.buffer[hit[0]][hit[1]]
        print(f"the new row's cell: fg={lit.fg} bg={lit.bg} reverse={lit.reverse}", flush=True)
        check(lit.bg != "default" or lit.reverse, "the new song under the list cursor")
        print("songs after the door:", songs(), flush=True)
        check(any("Discovery" in s and "Harder" in s for s in songs()), "the song landed in Discovery")

    cfg = os.path.join(R, "gui-config")
    for name in ("config.toml", "ytdl.json"):
        p = os.path.join(cfg, name)
        if os.path.exists(p):
            print(f"───── {name} ─────\n" + open(p).read()[:3000], flush=True)
    for _ in range(3):
        g.key("\x1b"); g.pump(0.3)
    print("exit", g.quit(), flush=True)
