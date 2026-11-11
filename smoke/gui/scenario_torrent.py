import json, os, re, time
# Add torrent on a pty, against whatever server the rig's gui-config names
# (docs/ux-contracts/add-torrent.md). No torrent client is needed: the
# page is all here — the two boxes, a whole paste (a magnet link, then a
# dropped file's path), the card with what the file holds, the two rows
# and their sheets, the typed picker — and a server with no client says
# why in its own words, Add standing off. Nothing is ever added: the pass
# stops short of the primary, so it is safe against a server that would
# take one.
#
# The list under the form is this player's own record (torrents.json), and
# only an add writes a row. So the pass writes two rows there itself before
# the player starts, sees them listed, takes one off with `x` — the file
# losing it — and opens the other in Files. The file is put back as it was.
#
# The rig: MSTREAM_RIG_DIR (default ~/mstream-rig) with gui-config/ naming
# one server, by its address, that has a library "Music"; the scratch
# .torrent is written there. See README.md.
R = os.environ.get("MSTREAM_RIG_DIR", os.path.expanduser("~/mstream-rig"))
BIN = os.environ.get("MSTREAM_PLAYER_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "debug", "mstream-player"))

NAME = "Northlight - Field Notes (2019) [FLAC]"
MAGNET = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=Northlight+-+Field+Notes+(2019)+%5BFLAC%5D"
# The two rows written into torrents.json: one sent to a folder (taken off
# the list), one the server already had whole, which names its library
# alone (opened in Files).
GONE, KEPT = "Sent And Gone", "Whole Library"


def paste(g, text):
    g.key(b"\x1b[200~" + text.encode() + b"\x1b[201~")


def check(cond, what):
    print(("PASS " if cond else "FAIL ") + what, flush=True)
    if not cond:
        raise SystemExit("FAIL: " + what)


def torrent(name, lengths):
    """A multi-file .torrent, bencoded by hand."""
    out = b"d8:announce17:http://t.example/4:infod5:filesl"
    for i, length in enumerate(lengths):
        out += b"d6:lengthi%de4:pathl6:%02d.mp3ee" % (length, i + 1)
    out += b"e4:name%d:%s12:piece lengthi16384e6:pieces20:" % (len(name.encode()), name.encode())
    return out + bytes(20) + b"ee"


def slot_key():
    """Where torrents.json keeps one server and account's rows
    (`config::slot_key`): the address the rig's config names its server
    by, and who signs in there."""
    url = user = None
    with open(os.path.join(R, "gui-config", "config.toml"), encoding="utf-8") as f:
        for line in f:
            m = re.match(r'\s*(url|username)\s*=\s*"([^"]*)"', line)
            if m and m.group(1) == "url" and url is None:
                url = m.group(2)
            elif m and m.group(1) == "username" and user is None:
                user = m.group(2)
    if url is None:
        raise SystemExit("FAIL: the rig's gui-config names no server")
    return url.rstrip("/") + "\n" + (user or "")


def seed(sent, key):
    """Write the two rows into this server's slot, the rest of the file
    left as it stands. Returns the file as it was, to put back."""
    before = open(sent, "rb").read() if os.path.exists(sent) else None
    try:
        held = json.loads(before) if before else {}
    except ValueError:
        held = {}
    if not isinstance(held, dict) or held.get("version") != 1 or not isinstance(held.get("servers"), dict):
        held = {"version": 1, "servers": {}}
    now = int(time.time() * 1000)
    held["servers"][key] = {"receipts": [
        {"name": GONE, "vpath": "Music", "path": "Smoke Test/" + GONE, "at": now - 5 * 60 * 1000},
        {"name": KEPT, "vpath": "Music", "path": "", "at": now - 3 * 3600 * 1000},
    ]}
    with open(sent, "w", encoding="utf-8") as f:
        json.dump(held, f)
    return before


def run(Gui):
    sent = os.path.join(R, "gui-config", "torrents.json")
    key = slot_key()
    before = seed(sent, key)
    try:
        walk(Gui, sent, key)
    finally:
        if before is None:
            os.remove(sent)
        else:
            with open(sent, "wb") as f:
                f.write(before)


def walk(Gui, sent, key):
    log = os.path.join(R, "gui.log"); open(log, "w").close()
    env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "TERM": "xterm-256color",
           "LANG": "en_US.UTF-8", "MSTREAM_PLAYER_CONFIG_DIR": os.path.join(R, "gui-config"),
           "MSTREAM_LOG": log, "MSTREAM_NO_OPEN": "1", "RUST_LOG": "info"}
    path = os.path.join(R, "Vela - Cassini (2020).torrent")
    with open(path, "wb") as f:
        f.write(torrent("Vela - Cassini (2020)", [3 * 1024 * 1024, 4 * 1024 * 1024, 5 * 1024 * 1024]))

    g = Gui([BIN, "gui"], env, os.path.join(R, "gui-torrent.raw"))
    g.wait_for("Music", 40, "the library")
    g.pump(1.0)

    # The room is the nav's, under TOOLS; it opens on the two boxes, and
    # what this player sent this server is listed under them.
    g.click_text("Add torrent", 5)
    g.wait_for("▸ Add torrent", 5, "the row lit")
    g.pump(2.0)
    g.dump("the room, opened")
    for words in ("Choose a .torrent file…", "type a path ▸", " or ", "Paste a magnet link"):
        check(g.find(words) is not None, f"the two ways in: {words}")
    check(g.find("│ Paste │") is None, "no Paste in a terminal")
    check(g.find("SOURCE") is None and g.find("MAGNET") is None, "no label column")
    for words in ("FROM TORRENTS", GONE, "Smoke Test", KEPT):
        check(g.find(words) is not None, f"what was sent, read from torrents.json: {words}")
    ready = g.find("· ready") is not None
    print("the gate:", "ready" if ready else "the server takes no torrents", flush=True)

    # A whole paste, the hand stowed: a magnet link fills the link box.
    paste(g, MAGNET)
    g.wait_for(NAME, 5, "the link's own name")
    g.pump(0.5)
    g.dump("a magnet link pasted")
    for words in ("magnet:?xt=urn:btih:c12fe1c0", "Metadata", "Northlight · Field Notes · 2019", "Destination",
                  "Music › Northlight › Field Notes", "Add torrent"):
        check(g.find(words) is not None, f"the link's form: {words}")
    check(g.find("Choose a .torrent file…") is None and g.find("Force fresh download") is None,
          "the boxes gone, and no checkbox for a link")
    check(g.find("─ Choose a .torrent file ─") is None, "its letters never opened the path sheet")
    # x clears the link: the two boxes are back.
    g.key("x"); g.pump(0.6)
    check(g.find("Choose a .torrent file…") is not None and g.find("Paste a magnet link") is not None, "x clears the link")
    g.key("\x1b"); g.pump(0.3)

    # A file dropped on a terminal arrives as its path, pasted: it loads.
    paste(g, path.replace(" ", "\\ "))
    g.wait_for("Vela - Cassini (2020).torrent", 5, "the file's card")
    g.pump(0.5)
    g.dump("a .torrent loaded")
    for words in ("3 files · 12 MB", "Open in another app", "Vela · Cassini · 2020", "Music › Vela › Cassini",
                  "[ ] Force fresh download", "FROM TORRENTS"):
        check(g.find(words) is not None, f"the file's form: {words}")
    check((g.find("Auto-detect metadata") is not None) == ready, "Auto-detect only where the server takes torrents")
    check((g.find("Add torrent ▸") is not None) == ready, "Add on only where the server takes torrents")

    # Metadata: a sheet of three boxes, edited as they are typed.
    g.key("e"); g.pump(0.8)
    g.dump("the Metadata sheet")
    check(g.find("─ Metadata ─") is not None and g.find("ARTIST") is not None and g.find("[ Done ]") is not None, "e opens Metadata")
    for ch in " Trio":
        g.key(ch)
    g.key("\r"); g.pump(0.6)
    check(g.find("─ Metadata ─") is None, "Enter closes it")
    check(g.find("Vela Trio · Cassini · 2020") is not None and g.find("Music › Vela Trio › Cassini") is not None,
          "the row and the destination follow")
    # Destination: the library first, the path's box, the rename checkbox.
    g.key("l"); g.pump(0.8)
    g.dump("the Destination sheet")
    for words in ("─ Destination · Music › Vela Trio › Cassini ─", "PATH", "/Music/Vela Trio/Cassini/‹torrent contents›",
                  "[✓] Rename the torrent's root folder", "Match the destination folder name"):
        check(g.find(words) is not None, f"l opens Destination: {words}")
    g.key("\x1b"); g.pump(0.6)
    check(g.find("─ Destination · ") is None, "Esc closes it")

    # The hand: ↓ lands on the card, ↓ again on its first way; Esc stows it.
    g.key("\x1b[B"); g.pump(0.4)
    g.key("\x1b[B"); g.pump(0.4)
    g.dump("the hand on a way")
    g.key("\x1b"); g.pump(0.4)
    # x lets the file go; t is the typed road.
    g.key("x"); g.pump(0.6)
    check(g.find("Choose a .torrent file…") is not None, "x lets the file go")
    g.key("t"); g.pump(1.0)
    g.dump("the typed picker")
    check(g.find("─ Choose a .torrent file ─") is not None, "t opens the typed picker")
    g.key("\x1b"); g.pump(0.6)
    check(g.find("─ Choose a .torrent file ─") is None, "Esc closes it")

    # The list: ↓ walks the hand past the two boxes onto its first row.
    def held(words):
        hit = g.find(words)
        cell = g.screen.buffer[hit[0]][hit[1]] if hit else None
        return cell is not None and (cell.bg != "default" or cell.reverse)

    for _ in range(6):
        if held(GONE):
            break
        g.key("\x1b[B"); g.pump(0.3)
    g.dump("the hand on the list's first row")
    check(held(GONE), "↓ walks the hand down to the list")
    # x takes the row off the list, and off the file.
    g.key("x"); g.pump(0.8)
    check(g.find(GONE) is None and g.find(KEPT) is not None, "x takes the row off the list")
    with open(sent, encoding="utf-8") as f:
        left = [row["name"] for row in json.load(f)["servers"][key]["receipts"]]
    check(left == [KEPT], f"and off torrents.json: {left}")
    check(held(KEPT), "the hand on the row left")
    # Enter opens a row's folder in Files — this one names its library alone.
    g.key("\r")
    g.wait_for("▸ Files", 5, "Files, lit in the nav")
    g.pump(1.0)
    g.dump("the row's folder, in Files")
    check(g.find("FROM TORRENTS") is None, "Enter opens the row's folder in Files")
    print("ALL PASS", flush=True)
    print("exit", g.quit(), flush=True)
