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

Usage: fake_mstream.py PORT
"""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])
LOOP = os.environ.get("FAKE_LOOP", "") not in ("", "0")

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

    def _body(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            return json.loads(raw)
        except Exception:
            return {}

    def do_GET(self):
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
        if self.path == "/api/v1/auth/login":
            return self._json(
                {"token": "fake-token", "vpaths": sorted(state["directories"])}
            )
        return self._json({})

    def do_PUT(self):
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
