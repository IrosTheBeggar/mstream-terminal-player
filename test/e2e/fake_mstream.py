#!/usr/bin/env python3
"""A scripted, STATEFUL mStream stand-in for the wizard's e2e legs.

Covers the whole wizard surface so the battery needs no Node, no real
server, and no audio files:

  - directories: PUT stores them, GET serves them back — the reopen
    leg's seeding round-trip is real state, not scripting.
  - users + login: PUT remembers the account, /auth/login answers with a
    token and the known vpaths.
  - Quick Connect: enabled, with a fake ticket — the Done page draws a
    code.
  - scan choreography: /scan/progress plays a file-scan sequence per
    poll; once drained, /scan/status plays the enrichment passes
    (waveforms with an estimate and a queued sibling, album art without
    one), then everything idles. FAKE_LOOP=1 loops both forever for
    interactive viewing.
  - /_state: the test oracle — directories and users as JSON.
  - FAKE_TASTE=1: the GUI's Discover room's server half — the ping
    advertises `discovery` + `discoveryTaste`, POST /discovery/taste
    answers one fixed history-seeded answer (two seeds, two songs) and
    the prefs read is empty (docs/ux-contracts/discovery-page.md; mStream's
    docs/discovery-page-plan.md §3.1–§3.3). /_state counts the asks.

Usage: fake_mstream.py PORT
"""
import json
import os
import struct
import sys
import zlib
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])
LOOP = os.environ.get("FAKE_LOOP", "") not in ("", "0")
# FAKE_AUTH=1: the real wall's shape for the admin hub's sign-in leg — once
# a user exists, /ping and /admin/* want the issued token (401 otherwise),
# and /auth/login only knows the users PUT there. Off by default: the
# wizard legs run on the zero-account window this would close.
AUTH = os.environ.get("FAKE_AUTH", "") not in ("", "0")
TASTE = os.environ.get("FAKE_TASTE", "") not in ("", "0")

# The room's answer, as the server words it (plan §3.1). The sources it was
# not asked for come back null, as the real route's do.
TASTE_ANSWER = {
    "window": 30, "widenedFrom": None, "seedSource": "history", "fallback": False,
    "seeds": [
        {"hash": "h1", "filepath": "music/manor.mp3", "metadata": {"title": "Manor", "artist": "ALM"},
         "outcome": "completed", "lastPlayed": "2026-10-01T10:00:00.000Z", "plays": 4, "pinned": False, "analyzed": True},
        {"hash": "h2", "filepath": "music/kite.mp3", "metadata": {"title": "Kite", "artist": "Boukmanflow"},
         "outcome": "completed", "plays": 1, "pinned": False, "analyzed": True},
    ],
    "excluded": [], "dropped": 0, "notAnalyzed": False,
    "library": {
        # One album with a cover: the room's strip draws its tile (the
        # 2026-10-11 strip log) and fetches the cover from /album-art/.
        "filter": "fresh", "capped": False,
        "albums": [{"name": "Nightfall", "album_artist": "Moon", "album_art_file": "nightfall.png",
                    "similarity": 0.86, "scopedTracks": 3, "playedTracks": 0}],
        "tracks": [
            {"filepath": "music/aurora.mp3", "similarity": 0.83, "seedHash": "h1",
             "metadata": {"title": "Aurora", "artist": "Ray", "duration": 251}, "playCount": 0, "lastPlayed": None},
            {"filepath": "music/borealis.mp3", "similarity": 0.8, "seedHash": "h2",
             "metadata": {"title": "Borealis", "artist": "Ray", "duration": 200}, "playCount": 2,
             "lastPlayed": "2024-01-01T00:00:00.000Z"},
        ],
    },
    "network": None,
    "peers": None,
}

STEPS = [
    [{"vpath": "media", "pct": 4, "scanned": 118}],
    [{"vpath": "media", "pct": 33, "scanned": 982}, {"vpath": "audiobooks", "pct": None, "scanned": 0}],
    [{"vpath": "media", "pct": 76, "scanned": 2261}, {"vpath": "audiobooks", "pct": None, "scanned": 4}],
    [{"vpath": "audiobooks", "pct": 88, "scanned": 197}],
    [],
]


def _e(pass_, state, attempted=None, total=None):
    p = {"pass": pass_, "state": state, "progress": None}
    if attempted is not None:
        p["progress"] = {"attempted": attempted, "total": total}
    return p


STATUS_STEPS = [
    [_e("waveform", "running", 40, 300), _e("albumart", "queued")],
    [_e("waveform", "running", 210, 300), _e("albumart", "queued")],
    [_e("albumart", "running", 57, None)],
    [_e("albumart", "running", 133, None)],
    [],
]

calls = {"n": 0, "s": 0}
state = {"directories": {}, "users": [], "taste_asks": 0, "art_gets": 0}


def _png(w=4, h=4, rgb=(200, 120, 40)):
    """A small solid PNG: a cover the player decodes the way it decodes any."""
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


COVER = _png()


class Handler(BaseHTTPRequestHandler):
    def _json(self, payload):
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _deny(self):
        body = json.dumps({"error": "Authentication Error"}).encode()
        self.send_response(401)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _walled(self):
        """True when this request must carry the token and does not."""
        if not AUTH or not state["users"]:
            return False
        if self.path != "/api/v1/ping" and not self.path.startswith("/api/v1/admin/"):
            return False
        return self.headers.get("x-access-token") != "fake-token"

    def _body(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            return json.loads(raw)
        except Exception:
            return {}

    def do_GET(self):
        if self._walled():
            return self._deny()
        if self.path == "/api/v1/scan/progress":
            i = calls["n"]
            calls["n"] += 1
            step = STEPS[i % len(STEPS)] if LOOP else STEPS[min(i, len(STEPS) - 1)]
            return self._json(step)
        if self.path == "/api/v1/scan/status":
            i = calls["s"]
            calls["s"] += 1
            step = (
                STATUS_STEPS[i % len(STATUS_STEPS)]
                if LOOP
                else STATUS_STEPS[min(i, len(STATUS_STEPS) - 1)]
            )
            return self._json({"enrichment": step})
        if self.path == "/api/v1/ping":
            ping = {"vpaths": sorted(state["directories"]), "noFileModify": False, "noUpload": False}
            if TASTE:
                ping.update({"discovery": True, "discoveryTaste": 1})
            return self._json(ping)
        if TASTE and self.path == "/api/v1/discovery/taste/prefs":
            return self._json({"exclude": []})
        if self.path == "/api/v1/admin/directories":
            return self._json(
                {name: {"root": root} for name, root in state["directories"].items()}
            )
        if self.path == "/api/v1/admin/iroh":
            return self._json(
                {
                    "enabled": True,
                    "available": True,
                    "running": True,
                    "online": True,
                    "qr": "mstr1:FAKE-TICKET-0123456789abcdef-for-widget-smoke-testing-only",
                }
            )
        if TASTE and self.path.split("?")[0].startswith("/album-art/"):
            state["art_gets"] += 1
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(COVER)))
            self.end_headers()
            self.wfile.write(COVER)
            return
        if self.path == "/_state":
            return self._json(state)
        return self._json({})

    def do_POST(self):
        body = self._body()
        if self.path == "/api/v1/auth/login":
            if AUTH and body.get("username") not in state["users"]:
                return self._deny()
            return self._json(
                {"token": "fake-token", "vpaths": sorted(state["directories"])}
            )
        if TASTE and self.path == "/api/v1/discovery/taste":
            state["taste_asks"] += 1
            return self._json(TASTE_ANSWER)
        return self._json({})

    def do_PUT(self):
        if self._walled():
            return self._deny()
        body = self._body()
        if self.path == "/api/v1/admin/directory":
            state["directories"][body.get("vpath", "?")] = body.get("directory", "?")
            return self._json({})
        if self.path == "/api/v1/admin/users":
            state["users"].append(body.get("username", "?"))
            return self._json({})
        return self._json({})

    def log_message(self, *args):
        pass


HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
