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
  - FAKE_LIBRARY=1: a made-up library for the SD-card transfer — the paged
    sync manifest with its ETag and 304, metadata/batch, /media with Range
    and HEAD, /transcode, album art, embeddings; /_library lists what was
    asked. Off by default, and the other routes stay as they are.

Usage: fake_mstream.py PORT
"""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])
LOOP = os.environ.get("FAKE_LOOP", "") not in ("", "0")
# FAKE_AUTH=1: the real wall's shape for the admin hub's sign-in leg — once
# a user exists, /ping and /admin/* want the issued token (401 otherwise),
# and /auth/login only knows the users PUT there. Off by default: the
# wizard legs run on the zero-account window this would close.
AUTH = os.environ.get("FAKE_AUTH", "") not in ("", "0")


# FAKE_LIBRARY=1: a small made-up library for the SD-card transfer's routes
# (the terminal's `device card-sync`): the paged sync manifest with its ETag
# and 304, `db/metadata/batch`, `/media` with Range and HEAD, `/transcode`
# (an Ogg Opus or an MP3 of the track's length), album art, and embeddings.
# The audio is generated here: silent MP3 frames and Ogg Opus pages, enough
# for a length check and a tag reader, never for a speaker. Off by default:
# every other leg sees the routes exactly as before.
LIBRARY = os.environ.get("FAKE_LIBRARY", "") not in ("", "0")


def _ogg_crc(data):
    crc = 0
    for b in data:
        crc ^= b << 24
        for _ in range(8):
            crc = ((crc << 1) ^ 0x04C11DB7) if crc & 0x80000000 else crc << 1
            crc &= 0xFFFFFFFF
    return crc


def _ogg_page(flags, granule, seq, packets):
    head = b"OggS\0" + bytes([flags]) + granule.to_bytes(8, "little", signed=True)
    head += (0x5EED).to_bytes(4, "little") + seq.to_bytes(4, "little") + b"\0\0\0\0"
    head += bytes([len(packets)]) + bytes(len(p) for p in packets)
    page = bytearray(head + b"".join(packets))
    page[22:26] = _ogg_crc(page).to_bytes(4, "little")
    return bytes(page)


def opus_stream(ms):
    """An Ogg Opus stream of `ms` by its granule, pre-skip 312, two channels."""
    head = b"OpusHead" + bytes([1, 2]) + (312).to_bytes(2, "little") + (48000).to_bytes(4, "little") + b"\0\0\0"
    tags = b"OpusTags" + (4).to_bytes(4, "little") + b"fake" + (0).to_bytes(4, "little")
    out = _ogg_page(0x02, 0, 0, [head]) + _ogg_page(0, 0, 1, [tags])
    pages = max(1, -(-ms // 1000))
    samples = ms * 48 + 312
    for n in range(1, pages + 1):
        flags = 0x04 if n == pages else 0
        out += _ogg_page(flags, samples * n // pages, 1 + n, [b"\xf8\xff\xfe"])
    return out


def mp3_stream(ms):
    """Silent MPEG-1 Layer III frames at 128 kbit/s, 44.1 kHz: `ms` long."""
    frame = b"\xff\xfb\x90\x40" + bytes(417 - 4)
    return frame * max(1, round(ms / 1000 * 44100 / 1152))


def _png(width, height, rgb):
    import struct
    import zlib

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\0" + bytes(rgb) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def _track(path, tid, data, seconds, art, bpm_source):
    return {"path": path, "id": tid, "bytes": data, "seconds": seconds, "art": art, "bpm-source": bpm_source}


TRACKS = [
    _track("Rock/Artist A/Album B/01.mp3", 1, mp3_stream(181500), 181.5, "b.png", "essentia"),
    _track("Rock/Artist A/Album B/02.mp3", 2, mp3_stream(95000), 95.0, "b.png", "tag"),
    # A wave the player can't play: the transfer asks for a conversion.
    _track("Rock/Artist C/Album D/01.wav", 3, b"RIFF a made-up wave", 120.0, "d.png", "tag"),
    _track("Jazz/Artist E/Album F/01.opus", 4, opus_stream(60000), 60.0, None, "tag"),
]
library = {
    "revision": "1:4:1790000000004:3",
    "tracks": TRACKS,
    "images": {"Rock/Artist A/Album B/cover.jpg": _png(64, 64, (200, 40, 90))},
    "art": {"b.png": _png(32, 32, (10, 120, 200)), "d.png": _png(48, 36, (240, 200, 0))},
    "asked": [],
}


def _md5(data):
    import hashlib

    return hashlib.md5(data).hexdigest()


def _entry(t):
    return {
        "filepath": t["path"], "id": t["id"], "file-size": len(t["bytes"]),
        "modified": 1790000000000 + t["id"], "hash": _md5(t["bytes"]),
        "audio-hash": _md5(b"audio:" + t["bytes"]), "hash-v": 2,
        "format": t["path"].rsplit(".", 1)[-1], "album-id": 1, "artist-id": 1,
        "created-at": "2026-10-01 12:00:00",
        "metadata": {"title": t["path"].rsplit("/", 1)[-1], "artist": "Artist", "album": "Album",
                     "duration": t["seconds"], "album-art": t["art"], "bpm": 124,
                     "musical-key": "A minor"},
    }


def _vector(path):
    import base64
    import hashlib
    import struct

    h = hashlib.md5(path.encode()).digest()
    return base64.b64encode(b"".join(struct.pack("<f", b / 255.0 - 0.5) for b in h[:8])).decode()

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
state = {"directories": {}, "users": []}


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

    def _bytes(self, status, data, kind="application/octet-stream", extra=(), head=False):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        for name, value in extra:
            self.send_header(name, value)
        self.end_headers()
        if not head:
            self.wfile.write(data)

    def _chunked(self, kind, data):
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        for at in range(0, len(data), 4096):
            part = data[at:at + 4096]
            self.wfile.write(b"%x\r\n" % len(part) + part + b"\r\n")
        self.wfile.write(b"0\r\n\r\n")

    def _media(self, rel, head=False):
        """`/media`: the file's bytes, a Range of them (206), or HEAD."""
        track = next((t for t in library["tracks"] if t["path"] == rel), None)
        data = track["bytes"] if track else library["images"].get(rel)
        if data is None:
            return self._bytes(404, b'{"error":"Not Found"}', "application/json")
        tag = 'W/"%x-%s"' % (len(data), _md5(data)[:8])
        rng = self.headers.get("Range", "")
        if rng.startswith("bytes=") and not head:
            first, _, last = rng[6:].partition("-")
            start = int(first or 0)
            end = min(int(last) + 1, len(data)) if last else len(data)
            if start >= len(data):
                return self._bytes(416, b"", extra=[("Content-Range", "bytes */%d" % len(data))])
            span = "bytes %d-%d/%d" % (start, end - 1, len(data))
            return self._bytes(206, data[start:end], extra=[("Content-Range", span), ("ETag", tag)])
        return self._bytes(200, data, extra=[("ETag", tag), ("Accept-Ranges", "bytes")], head=head)

    def _library_get(self):
        """A route of the library's, answered: True; else False."""
        from urllib.parse import parse_qs, unquote, urlsplit

        url = urlsplit(self.path)
        path = unquote(url.path)
        library["asked"].append("GET " + path)
        if path == "/api/":
            vpaths = sorted({t["path"].split("/", 1)[0] for t in library["tracks"]})
            self._json({
                "server": "6.31.1",
                "features": {"sync": True, "discovery": True,
                             "transcode": {"defaultCodec": "opus", "defaultBitrate": "96k"}},
                "user": {"vpaths": vpaths, "admin": False},
            })
        elif path.startswith("/media/"):
            self._media(path[len("/media/"):])
        elif path.startswith("/transcode/"):
            rel = path[len("/transcode/"):]
            track = next((t for t in library["tracks"] if t["path"] == rel), None)
            if track is None:
                self._bytes(404, b'{"error":"Not Found"}', "application/json")
            elif parse_qs(url.query).get("codec", ["opus"])[0] == "mp3":
                self._chunked("audio/mpeg", mp3_stream(int(track["seconds"] * 1000)))
            else:
                self._chunked("audio/ogg", opus_stream(int(track["seconds"] * 1000)))
        elif path.startswith("/album-art/"):
            data = library["art"].get(path[len("/album-art/"):])
            if data is None:
                self._bytes(404, b'{"error":"Not Found"}', "application/json")
            else:
                self._bytes(200, data, "image/png")
        elif path == "/_library":
            self._json({"revision": library["revision"], "asked": library["asked"]})
        else:
            return False
        return True

    def _library_post(self, body):
        library["asked"].append("POST " + self.path)
        if self.path == "/api/v1/sync/manifest":
            quoted = '"%s"' % library["revision"]
            if "cursor" not in body and self.headers.get("If-None-Match") == quoted:
                self.send_response(304)
                self.send_header("ETag", quoted)
                self.end_headers()
                return True
            ignore = body.get("ignoreVPaths") or []
            cursor = body.get("cursor") or 0
            limit = int(body.get("limit") or 2000)
            tracks = sorted(
                (t for t in library["tracks"]
                 if t["id"] > cursor and t["path"].split("/", 1)[0] not in ignore),
                key=lambda t: t["id"],
            )
            page = tracks[:limit]
            nxt = page[-1]["id"] if len(tracks) > limit else None
            data = json.dumps({"revision": library["revision"], "scanning": False, "next": nxt,
                               "entries": [_entry(t) for t in page]}).encode()
            self._bytes(200, data, "application/json", extra=[("ETag", quoted)])
            return True
        if self.path == "/api/v1/db/metadata/batch":
            out = {}
            for p in body if isinstance(body, list) else []:
                t = next((t for t in library["tracks"] if t["path"] == p), None)
                out[p] = None if t is None else {"filepath": p, "metadata": dict(
                    _entry(t)["metadata"], **{"sample-rate": 44100, "channels": 2,
                                              "bitrate": 320000, "bpm-source": t["bpm-source"]})}
            self._json(out)
            return True
        if self.path == "/api/v1/discovery/local/embeddings":
            paths = body.get("filePaths") or []
            known = {t["path"] for t in library["tracks"]}
            if any(p not in known for p in paths):
                self._bytes(404, b'{"error":"Track not found"}', "application/json")
                return True
            tracks = [{"filePath": p, "audioHash": None, "notAnalyzed": False, "embedding": _vector(p)}
                      for p in paths]
            self._json({"model": {"id": "effnet-discogs", "version": "1"}, "dim": 8, "tracks": tracks})
            return True
        return False

    def do_HEAD(self):
        if not LIBRARY:
            return self.send_error(501)
        from urllib.parse import unquote, urlsplit

        path = unquote(urlsplit(self.path).path)
        library["asked"].append("HEAD " + path)
        if path.startswith("/media/"):
            return self._media(path[len("/media/"):], head=True)
        return self._bytes(404, b"", head=True)

    def do_GET(self):
        if self._walled():
            return self._deny()
        if LIBRARY and self._library_get():
            return None
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
            return self._json(
                {"vpaths": sorted(state["directories"]), "noFileModify": False, "noUpload": False}
            )
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
        if self.path == "/_state":
            return self._json(state)
        return self._json({})

    def do_POST(self):
        body = self._body()
        if LIBRARY and self._library_post(body):
            return None
        if self.path == "/api/v1/auth/login":
            if AUTH and body.get("username") not in state["users"]:
                return self._deny()
            return self._json(
                {"token": "fake-token", "vpaths": sorted(state["directories"])}
            )
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
