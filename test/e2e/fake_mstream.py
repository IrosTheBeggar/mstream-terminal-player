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
    answers one fixed history-seeded answer (two seeds, two songs). The
    route is stateless, as the real one since 2026-10-14: `remember`,
    `filter` and `staleDays` answer 400 and the old …/taste/prefs route
    404 (docs/ux-contracts/discovery-page.md; mStream's
    docs/discovery-page-plan.md §3.1). /_state counts the asks, keeps the
    last body, and counts any prefs call. The ping also advertises
    `federationDiscovery`, so the room asks its peers half (`sources:
    ["peers"]`) once Main lands: one connected server (id 7, "Attic")
    with one playable row whose `albumArt` names its cover on the peer
    (mStream 2026-10-16), served through the federation art proxy
    GET /api/v1/federation/peers/7/art/<file> — /_state counts those
    gets (`peer_art_gets`) and keeps their paths.
  - FAKE_PLUGINS=1 (with FAKE_TASTE): the room's plug-ins — Keep a copy,
    the Copies list, Open elsewhere (discovery-page clauses 100–141). The
    ping adds `discoveryPlugins`; GET /discovery/plugins lists `links` and
    `federation-copy` (every scope) with jobs allowed; the destination is a
    throwaway `copies` library; POST …/federation-copy/jobs queues a job
    (409 for a recommendation titled "Taken", 429 for "Full") that walks
    queued → running → done (a song copied, or an album's counts);
    …/cancel cancels a live one and answers 409 for a
    finished one; …/clear drops the finished; …/lookup/batch answers each
    recommendation's keys (text:/album:/artist: over its names) and the
    jobs under them; …/links/resolve two https links and one that is not.
    A job walks on the wall clock — queued for a second, running for six,
    then done — and every list, lookup, start and cancel sees it where its
    age puts it.
    /_state keeps the starts' bodies (`job_starts`), the lookups' sizes
    (`lookup_sizes`), the cancels, clears and resolves.

Usage: fake_mstream.py PORT
"""
import json
import os
import struct
import time
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
PLUGINS = TASTE and os.environ.get("FAKE_PLUGINS", "") not in ("", "0")

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
        "capped": False,
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

# The peers half's answer (plan §3.1): library and network not asked, so
# null; one connected server whose row names its cover as the peer's own
# browse rows do (`albumArt`), fetched through the parent's art proxy.
PEER_ART = "attic-lanterns.png"
TASTE_PEERS_ANSWER = dict(TASTE_ANSWER, library=None, peers={
    "peers": [
        {"id": 7, "name": "Attic", "reachable": True, "mismatched": False, "results": [
            {"artist": "Attic Band", "title": "Ember", "album": "Lanterns", "duration": 199,
             "similarity": 0.81, "filepath": "Music/ember.mp3", "albumArt": PEER_ART,
             "peer": {"id": 7, "name": "Attic"}, "source": "federation"},
        ]},
    ],
    "searched": {"peers": 1, "unreachable": 0, "mismatched": 0},
})

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
state = {"directories": {}, "users": [], "taste_asks": 0, "taste_body": None, "prefs_calls": 0, "art_gets": 0,
         "peer_asks": 0, "peer_art_gets": 0, "peer_art_paths": [],
         "jobs": [], "job_starts": [], "lookup_sizes": [], "cancels": 0, "clears": 0, "resolves": 0}

# The plug-in listing, the destination and the links (FAKE_PLUGINS).
PLUGIN_LISTING = {
    "plugins": [
        {"name": "links", "title": "Open elsewhere", "capabilities": ["links"], "scope": "server",
         "scopes": ["song"], "settings": [], "enabled": True, "available": True},
        {"name": "federation-copy", "title": "Add to your collection", "capabilities": ["acquire"], "scope": "server",
         "scopes": ["song", "album", "artist", "artist-missing", "folder"], "settings": [], "enabled": True, "available": True},
    ],
    "jobs": {"allowed": True},
}
DESTINATION = {
    "destination": {"vpath": "copies", "base": "", "layout": "{{ARTIST}}/{{ALBUM}}", "source": "default"},
    "saved": None, "libraries": [{"vpath": "copies", "template": None}],
    "defaultLayout": "{{ARTIST}}/{{ALBUM}}", "variables": ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER"],
}
LINKS = {"links": [
    {"id": "mb-search", "label": "MusicBrainz", "url": "https://musicbrainz.org/search?query=x", "kind": "search"},
    {"id": "deezer", "label": "Deezer", "url": "https://www.deezer.com/search/x", "kind": "search"},
    {"id": "odd", "label": "Odd", "url": "javascript:alert(1)", "kind": "search"},
]}


def _keys(rec):
    """A recommendation's keys over its names — the server computes its own;
    the client only ever matches them."""
    low = lambda k: str(rec.get(k) or "").strip().lower()
    credit = str(rec.get("albumArtist") or "").strip().lower() or low("artist")
    keys = {"song": "text:" + low("title")}
    if rec.get("album"):
        keys["album"] = "album:" + credit + "|" + low("album")
    if rec.get("artist"):
        keys["artist"] = "artist:" + low("artist")
        keys["artist-missing"] = "artist-missing:" + low("artist")
    return keys


# A copy's life on the wall clock: waiting for a free slot this long, then
# running this long, then done — long enough for the room's 4 s poll to
# see it live, short enough for a leg.
JOB_QUEUED_S = 1.0
JOB_RUNNING_S = 6.0


def _walk_jobs():
    """Every live job where its age puts it: queued → running → done — asked
    by the list and the lookups alike, so both say the same."""
    now = time.time()
    for job in state["jobs"]:
        age = now - job["_born"]
        if job["state"] == "queued" and age >= JOB_QUEUED_S:
            job.update(state="running", progress=0.5, statusText="song 1 of 2")
        if job["state"] == "running" and age >= JOB_QUEUED_S + JOB_RUNNING_S:
            scope = (job.get("params") or {}).get("scope")
            rec = job["recommendation"]
            if scope:
                result = {"scope": scope, "counts": {"total": 2, "copied": 2, "skipped": 0, "failed": 0},
                          "songs": {"total": 2, "copied": [{}, {}], "skipped": [], "failed": []}, "stopped": None,
                          "peer": rec.get("peer")}
            else:
                result = {"copied": {"vpath": "copies", "filepath": "copies/%s/%s/%s.mp3" % (
                    rec.get("artist"), rec.get("album"), rec.get("title")), "title": rec.get("title")}}
            job.update(state="done", progress=1, statusText=None, result=result)

def _public(job):
    """A job as the server words it: without the fake's own clock."""
    return {k: v for k, v in job.items() if not k.startswith("_")}


# Keys the stateless taste route refuses (mStream 8dcfad0e): a 400.
TASTE_GONE = ("remember", "filter", "staleDays")


def _png(w=4, h=4, rgb=(200, 120, 40)):
    """A small solid PNG: a cover the player decodes the way it decodes any."""
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


COVER = _png()
PEER_COVER = _png(rgb=(40, 120, 200))


class Handler(BaseHTTPRequestHandler):
    def _json(self, payload):
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _status(self, code, payload):
        body = json.dumps(payload).encode()
        self.send_response(code)
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
                ping.update({"discovery": True, "discoveryTaste": 1, "federationDiscovery": True})
            if PLUGINS:
                ping["discoveryPlugins"] = True
            return self._json(ping)
        if PLUGINS and self.path == "/api/v1/discovery/plugins":
            return self._json(PLUGIN_LISTING)
        if PLUGINS and self.path == "/api/v1/discovery/collection/destination":
            return self._json(DESTINATION)
        if PLUGINS and self.path.split("?")[0] == "/api/v1/discovery/plugin-jobs":
            _walk_jobs()
            return self._json({"jobs": [_public(j) for j in sorted(state["jobs"], key=lambda j: -j["id"])],
                               "runner": {"running": sum(j["state"] == "running" for j in state["jobs"]), "active": True}})
        if TASTE and self.path == "/api/v1/discovery/taste/prefs":
            state["prefs_calls"] += 1
            return self._status(404, {"error": "Not Found"})
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
        if TASTE and self.path.split("?")[0].startswith("/api/v1/federation/peers/7/art/"):
            state["peer_art_gets"] += 1
            state["peer_art_paths"].append(self.path)
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(PEER_COVER)))
            self.end_headers()
            self.wfile.write(PEER_COVER)
            return
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
        if PLUGINS and self.path == "/api/v1/discovery/plugins/federation-copy/jobs":
            rec = body.get("recommendation") or {}
            state["job_starts"].append(body)
            if rec.get("title") == "Taken":
                return self._status(409, {"error": "someone else on this server is already getting this — try again in a moment"})
            if rec.get("title") == "Full":
                return self._status(429, {"error": "you have 20 jobs queued or running — wait for some to finish (the limit is 20)"})
            scope = body.get("scope")
            key = _keys(rec)[scope or "song"] if (scope or "song") in _keys(rec) else None
            _walk_jobs()
            live = [j for j in state["jobs"] if j["key"] == key and j["state"] in ("queued", "running")]
            if live:
                return self._json({"created": False, "job": _public(live[0])})
            job = {"id": len(state["jobs"]) + 1, "plugin": "federation-copy", "key": key, "recommendation": rec,
                   "params": {"scope": scope} if scope else None, "state": "queued", "progress": None,
                   "statusText": None, "result": None, "error": None, "attempts": 0, "cancelRequested": False,
                   "createdAt": 1760000000000 + len(state["jobs"]), "_born": time.time()}
            state["jobs"].append(job)
            return self._status(202, {"created": True, "job": _public(job)})
        if PLUGINS and self.path == "/api/v1/discovery/plugin-jobs/lookup/batch":
            recs = body.get("recommendations") or []
            state["lookup_sizes"].append(len(recs))
            _walk_jobs()
            results = []
            for rec in recs:
                keys = _keys(rec)
                results.append({"key": keys["song"], "keys": keys,
                                "jobs": [_public(j) for j in sorted(state["jobs"], key=lambda j: -j["id"]) if j["key"] in keys.values()]})
            return self._json({"results": results})
        if PLUGINS and self.path.startswith("/api/v1/discovery/plugin-jobs/") and self.path.endswith("/cancel"):
            state["cancels"] += 1
            jid = int(self.path.split("/")[-2])
            job = next((j for j in state["jobs"] if j["id"] == jid), None)
            if job is None:
                return self._status(404, {"error": "No such job"})
            _walk_jobs()
            if job["state"] not in ("queued", "running"):
                return self._status(409, {"error": "job %d has already finished" % jid})
            job.update(state="cancelled")
            return self._json({"outcome": "cancelled", "job": _public(job)})
        if PLUGINS and self.path == "/api/v1/discovery/plugin-jobs/clear":
            state["clears"] += 1
            before = len(state["jobs"])
            state["jobs"] = [j for j in state["jobs"] if j["state"] in ("queued", "running")]
            return self._json({"removed": before - len(state["jobs"])})
        if PLUGINS and self.path == "/api/v1/discovery/plugins/links/resolve":
            state["resolves"] += 1
            return self._json({"plugin": "links", "result": LINKS})
        if TASTE and self.path == "/api/v1/discovery/taste":
            state["taste_asks"] += 1
            state["taste_body"] = body
            gone = [k for k in TASTE_GONE if k in body]
            if gone:
                return self._status(400, {"error": f'"{gone[0]}" is not allowed'})
            if "peers" in (body.get("sources") or []):
                state["peer_asks"] += 1
                return self._json(TASTE_PEERS_ANSWER)
            return self._json(TASTE_ANSWER)
        return self._json({})

    def do_PUT(self):
        if TASTE and self.path == "/api/v1/discovery/taste/prefs":
            state["prefs_calls"] += 1
            return self._status(404, {"error": "Not Found"})
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
