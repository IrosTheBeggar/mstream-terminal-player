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
    throwaway `copies` library (its default; `music` the account's other
    library, its template ARTIST / YEAR - ALBUM; the server's default layout
    ALBUMARTIST / ALBUM; its variables mStream's seven, SERVERFOLDER
    among them), saved per account by PUT …/collection/destination
    as the server saves it — `null` back to the default; a library not
    offered, a base folder that climbs out (`..`, `~`, a drive letter) or a
    variable not known (quoted as typed) a 400 in the server's words;
    POST /api/v1/file-explorer lists `copies` (from-peers)
    and `copies/from-peers` (three servers' folders) for the picker;
    POST /_move_destination {destination} saves one behind the client's
    back, as another tab would, so the next start that shows the old one
    is the server's 409 destination_changed; POST …/federation-copy/jobs
    checks the destination a start shows as the jobs route does (409
    destination_changed, with the one in force, and nothing started),
    pins the one in force on the job, then queues it (409 for a
    recommendation titled "Taken", 429 for "Full") that walks
    queued → running → done (a song copied, or an album's counts);
    …/cancel cancels a live one and answers 409 for a
    finished one; …/clear drops the finished; …/lookup/batch answers each
    recommendation's keys (text:/album:/artist: over its names) and the
    jobs under them; …/links/resolve two https links and one that is not.
    A job walks on the wall clock — queued for a second, running for six,
    then done — and every list, lookup, start and cancel sees it where its
    age puts it. FAKE_NO_PIN=1 (with FAKE_PLUGINS) is a server from before
    the start's destination (mStream before 2026-10-21): a start that
    carries the key is Joi's 400 '"destination" is not allowed', and no
    job is pinned.
    /_state keeps the starts' bodies (`job_starts`) and how each was
    answered (`job_answers`), the lookups' sizes (`lookup_sizes`), the
    cancels, clears and resolves, the destination's reads
    (`destination_gets`), saves (`destination_puts`) and listings
    (`folder_lists`). The Discover room's connected server (id 7, "Attic")
    answers its artist's albums through the browse proxy — Lanterns,
    credited to Attic Band — and the user's own library holds Lanterns by
    name, so Keep a copy's Every album by counts one of theirs with
    nothing missing to tick.
  - FAKE_LIBRARY=1: a made-up library for the SD-card transfer — the paged
    sync manifest with its ETag and 304, metadata/batch, /media with Range
    and HEAD, /transcode, album art, embeddings; /_library lists what was
    asked. Off by default, and the other routes stay as they are.
  - FAKE_ROLE=peer: this process is a CONNECTED server, "Stage Peer B" —
    the keep-a-copy spec's §2 sample library, one library `peerlib`
    (Boukmanflow's three albums, the Various Artists compilation, Peer
    Sessions, Split Series whose credits disagree, Night Owl Radio
    Archive's 1,240 untagged songs in 56 folders). It answers the
    federation allowlist's reads as mStream words them: the layered GET
    /api/ (a peer answers no ping through the proxy), the file explorer
    and its recursive listing, the albums and an artist's albums (the
    credit facts: album_artist — null where the rows disagree — artists,
    compilation, the year aggregates), album-songs (album_artist and year
    narrowing), the artists, genres, genre-songs, search (album hits with
    the listing's credit), recent/added, metadata and metadata/batch, and
    /album-art/. /_state lists every read (`asked`) and keeps the
    recursive listings' and the tag batches' bodies.
  - FAKE_PEER_URL=http://…  (with FAKE_PLUGINS): the user's OWN server
    paired with that connected server, as id 2 "Stage Peer B": the ping
    adds `federationBrowse`, GET /api/v1/federation/peers lists it, and
    ALL /api/v1/federation/peers/2/api/<path> is mStream's browse proxy
    (federation-browse.js): the route screened against the exact
    allowlist (403 "Route not available over federation" off it), then
    the method, the JSON body and the peer's answer passed through;
    …/peers/2/art/<file> its art proxy. The user's library holds Harbour
    Lights by name (the artist's albums, asked of the own server); the
    `copies` library holds Boukmanflow, Color Out and From Sam (the Into a
    folder chooser's list); the listing names `jobs.maxFolderSongs` 1,000.
    Folder jobs key on the peer and the path (jobKey), keep their
    `landing` in `params`, and end with the folder they landed in.
    /_state adds the proxied routes (`proxied`) and the refused
    (`proxy_refused`).
  - FAKE_P2P=1 (it turns on FAKE_TASTE and FAKE_PLUGINS too): the GUI's
    Peer recs room's server half, taste generation 3 (mStream
    docs/peer-recommendations-plan.md §3.1–3.6), answered from the stage's
    own answers in test/fixtures/peer_recs/*.json, the same files the Rust
    tests read (peer-recs plan D25). The ping adds `discoveryP2p`,
    `discoveryTaste: 3` and `discoveryP2pPeers: 1`, and `discoveryPath`
    as the stage's real server has it, so the GUI's nav stands as the
    peer-recs contract §1.1 draws it with every tool (Peer recs on row 18).
      GET /api/v1/discovery/p2p/peers is the strip: peers_admin's rows,
    shelf peers online first then by label, the addable ones after.
      POST /api/v1/admin/discovery/p2p/peer-dbs/add {endpointId} is the
    admin's Add, as discovery-peer-add.js answers it: 202 `adding`; the next
    poll still reads `adding`, and 2 s after that poll Lighthouse Tapes is
    on the shelf (`done`), swapping the unpinned Attic Tapes out, as the
    stage's shelf cap does; Rooftop FM fails `offline` at the same point
    (`try again`, and a press tries again); Static Garden stays `adding`
    for 120 s. A peer already held answers 200 done, one not in the
    catalog 404.
      POST /api/v1/discovery/taste with `sources: ["network"]` is checked as
    the route's Joi schema checks it (a key it does not know, `[]` for
    networkPeers, a bad id: a 400 in Joi's words), then answered by the
    tab: taste_all for the whole shelf; taste_basement, taste_kitchen,
    taste_3f9a or taste_attic (nothing new) for one shelf peer;
    taste_not_on_shelf for a peer the shelf does not hold. The seeds are
    the pins (`pinned`), else the history's two songs less `exclude`, else
    the random seed (taste_random's seed and leads, `fallback`), whose
    network part is null unless networkOnRandomSeed is true. A Discover
    body (no `network` source) carrying a generation-3 key is counted in
    `discover_gen3_bodies`: Discover's wire must never change.
      The plug-ins are links, deezer (preview), federation-copy and
    noop-acquire (Get it): POST …/deezer/resolve answers a 2 s PCM WAV at
    /_clip/<rank>.wav, the lead's rank in taste_all; Arcade Rain finds no
    preview (null); Quiet Engine answers 429 once; Long Exposure's clip URL
    is dead (404). Each preview names an artwork URL, /_artwork/<rank>.png,
    which the player must never fetch. POST …/noop-acquire/jobs starts a
    job (its destination checked as federation-copy's), running at once
    and done 2 s later ("fail" in the title fails it, as the real
    plug-in's does); GET /api/v1/discovery/plugin-jobs/<id> reads one.
      /_state adds `pings`, `peer_polls`, `adds` (the Add posts) with
    `add_bodies` and `add_answers`, `peer_adds` (each Add's state by
    label), `network_bodies` with `network_answers` (the fixture or the
    status that answered), `discover_gen3_bodies`, `preview_resolves`
    ({title, answer, recommendation}), `clip_gets`, `job_gets` and
    `art_paths`. `resolves` counts every resolve (links and previews), and
    `art_gets` counts the previews' artwork beside /album-art/.
  - FAKE_MEMBER=1 (with FAKE_P2P): the account is a member: `canAdd` is
    false, the strip has no addable rows and no Add state, and an Add is
    the admin guard's 403.

Usage: fake_mstream.py PORT [PORTFILE]

PORT 0 takes any free port; PORTFILE, when given, gets the port it is
listening on (written whole, once it listens), which is how run.sh's
scenarios, side by side, each get a port no other fake can be handed.
"""
import copy
import json
import math
import os
import re
import socketserver
import struct
import time
import sys
import urllib.error
import urllib.parse
import urllib.request
import zlib
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])
LOOP = os.environ.get("FAKE_LOOP", "") not in ("", "0")
# FAKE_AUTH=1: the real wall's shape for the admin hub's sign-in leg — once
# a user exists, /ping and /admin/* want the issued token (401 otherwise),
# and /auth/login only knows the users PUT there. Off by default: the
# wizard legs run on the zero-account window this would close.
AUTH = os.environ.get("FAKE_AUTH", "") not in ("", "0")
# The Peer recs room's server half (taste generation 3); it needs the taste
# route and the plug-ins, so it turns both on.
P2P = os.environ.get("FAKE_P2P", "") not in ("", "0")
# The Peer recs room's account is a member: no Add.
MEMBER = P2P and os.environ.get("FAKE_MEMBER", "") not in ("", "0")
TASTE = P2P or os.environ.get("FAKE_TASTE", "") not in ("", "0")
PLUGINS = TASTE and (P2P or os.environ.get("FAKE_PLUGINS", "") not in ("", "0"))
# A server from before the start's destination (mStream before 2026-10-21):
# its jobs route's strict body refuses the key, and no job is pinned.
NO_PIN = PLUGINS and os.environ.get("FAKE_NO_PIN", "") not in ("", "0")
# This process is a connected server (the keep-a-copy spec's Stage Peer B).
PEER_ROLE = os.environ.get("FAKE_ROLE", "") == "peer"
# The own server's connected server, reached through its browse proxy.
PEER_URL = os.environ.get("FAKE_PEER_URL", "").rstrip("/") if PLUGINS else ""

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
         "jobs": [], "job_starts": [], "job_answers": [], "lookup_sizes": [], "cancels": 0, "clears": 0, "resolves": 0,
         "saved_destination": None, "destination_gets": 0, "destination_puts": [], "folder_lists": [],
         "proxied": [], "proxy_refused": [], "asked": [], "recursive_bodies": [], "batch_bodies": [],
         # The Peer recs room's (FAKE_P2P); there, `resolves` counts the
         # preview resolves too, and `art_gets` the previews' artwork.
         "pings": 0, "peer_polls": 0, "adds": 0, "add_bodies": [], "add_answers": [], "peer_adds": {},
         "network_bodies": [], "network_answers": [], "discover_gen3_bodies": 0, "preview_resolves": [],
         "clip_gets": [], "job_gets": 0, "art_paths": []}

# The plug-in listing, the destination and the links (FAKE_PLUGINS). A
# folder copy past `maxFolderSongs` is refused before it starts.
PLUGIN_LISTING = {
    "plugins": [
        {"name": "links", "title": "Open elsewhere", "capabilities": ["links"], "scope": "server",
         "scopes": ["song"], "settings": [], "enabled": True, "available": True},
        {"name": "federation-copy", "title": "Add to your collection", "capabilities": ["acquire"], "scope": "server",
         "scopes": ["song", "album", "artist", "artist-missing", "folder"], "settings": [], "enabled": True, "available": True},
    ],
    "jobs": {"allowed": True, "maxFolderSongs": 1000},
}
# The libraries this account may upload to, in the server's order: the
# first is the default.
LIBRARIES = [{"vpath": "copies", "template": None}, {"vpath": "music", "template": "{{ARTIST}}/{{YEAR}} - {{ALBUM}}"}]
DEFAULT_LAYOUT = "{{ALBUMARTIST}}/{{ALBUM}}"
# The variables a layout may name, as mStream lists them since 2026-10-22:
# SERVERFOLDER (the song's folders on the connected server) last.
LAYOUT_VARS = ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER", "SERVERFOLDER"]
# The picker's folders, by the path the file explorer is asked. Paired with
# a connected server, the spec's §2 sample instead: the Into a folder
# chooser lists `copies`' three folders, From Sam with none inside.
FOLDERS = {"/copies": ["from-peers"], "/copies/from-peers": ["2025 archive", "Stage Peer B", "Stage Peer C"]}
if PEER_URL:
    FOLDERS = {"/copies": ["Boukmanflow", "Color Out", "From Sam"], "/copies/Boukmanflow": ["Harbour Lights"],
               "/copies/Color Out": [], "/copies/From Sam": []}

# The user's own library, by artist, as /db/artists-albums words it: what
# "Only the albums you don't have" is reckoned against. Attic's Lanterns
# for the Discover room's legs; Harbour Lights for Stage Peer B's.
OWN_ALBUMS = {
    "attic band": [{"name": "Lanterns", "year": None, "album_art_file": None, "album_artist": "Attic Band",
                    "artists": ["Attic Band"], "compilation": False, "year_min": None, "year_max": None,
                    "track_count": 1, "duration": 199}],
    "boukmanflow": [{"name": "Harbour Lights", "year": 2021, "album_art_file": None, "album_artist": "Boukmanflow",
                     "artists": ["Boukmanflow"], "compilation": False, "year_min": 2021, "year_max": 2021,
                     "track_count": 3, "duration": 31}],
}
# Attic (the Discover room's connected server, id 7), as its browse proxy
# answers an artist's albums: the one album its row names, credited.
ATTIC_ALBUMS = {"attic band": OWN_ALBUMS["attic band"]}

# ── Stage Peer B (FAKE_ROLE=peer): the spec's §2 sample library ──────────────
PEER_ID = 2
PEER_NAME = "Stage Peer B"
PEER_LIB = "peerlib"
# The federation allowlist's exact reads (mStream api/federation-auth.js):
# the browse proxy forwards only these.
FED_ALLOWED = {
    "GET /api", "GET /api/", "GET /api/v1/db/status", "POST /api/v1/db/metadata", "POST /api/v1/db/metadata/batch",
    "GET /api/v1/db/artists", "POST /api/v1/db/artists", "POST /api/v1/db/artists-albums", "GET /api/v1/db/albums",
    "POST /api/v1/db/albums", "GET /api/v1/db/genres", "POST /api/v1/db/genres", "POST /api/v1/db/genre-songs",
    "POST /api/v1/db/album-songs", "POST /api/v1/db/recent/added", "POST /api/v1/db/search",
    "POST /api/v1/db/random-songs", "POST /api/v1/discovery/local/embeddings", "POST /api/v1/file-explorer",
    "POST /api/v1/file-explorer/recursive", "POST /api/v1/file-explorer/m3u", "GET /api/v1/federation/health",
    "POST /api/v1/federation/discovery/similar", "POST /api/v1/federation/guest",
}


def _psong(path, title, artist, album, credit, year, track, seconds, genre="Hip-Hop"):
    return {"filepath": PEER_LIB + "/" + path, "metadata": {
        "title": title, "artist": artist, "album": album, "album-artist": credit, "year": year, "track": track,
        "duration": seconds, "genres": [genre], "format": "mp3", "bitrate": 320000, "sample-rate": 44100,
        "album-art": None}}


PEER_SONGS = [
    _psong("Boukmanflow/Borrowed Tapes/01 - Rooftop Cypher.mp3", "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes", "Boukmanflow", 2019, 1, 12),
    _psong("Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3", "Late Train Verse", "Boukmanflow", "Borrowed Tapes", "Boukmanflow", 2019, 2, 8),
    _psong("Boukmanflow/Borrowed Tapes/03 - Corner Store Loop.mp3", "Corner Store Loop", "Boukmanflow", "Borrowed Tapes", "Boukmanflow", 2019, 3, 9),
    _psong("Boukmanflow/Harbour Lights/01 - Harbour Lights.mp3", "Harbour Lights", "Boukmanflow", "Harbour Lights", "Boukmanflow", 2021, 1, 11),
    _psong("Boukmanflow/Harbour Lights/02 - Tide Tables.mp3", "Tide Tables", "Boukmanflow", "Harbour Lights", "Boukmanflow", 2021, 2, 10),
    _psong("Boukmanflow/Harbour Lights/03 - Night Ferry.mp3", "Night Ferry", "Boukmanflow", "Harbour Lights", "Boukmanflow", 2021, 3, 10),
    _psong("Boukmanflow/Second Shift/01 - Clock In.mp3", "Clock In", "Boukmanflow", "Second Shift", "Boukmanflow", 2023, 1, 9),
    _psong("Boukmanflow/Second Shift/02 - Overtime.mp3", "Overtime", "Boukmanflow", "Second Shift", "Boukmanflow", 2023, 2, 12),
    _psong("Boukmanflow/Second Shift/03 - Clock Out.mp3", "Clock Out", "Boukmanflow", "Second Shift", "Boukmanflow", 2023, 3, 8),
    _psong("Various Artists/Night Market Sampler/01 - Lantern Street.mp3", "Lantern Street", "Color Out", "Night Market Sampler", "Various Artists", 2024, 1, 12, "Electronic"),
    _psong("Various Artists/Night Market Sampler/02 - Night Market Cypher.mp3", "Night Market Cypher", "Boukmanflow", "Night Market Sampler", "Various Artists", 2024, 2, 8),
    _psong("Various Artists/Night Market Sampler/03 - Paper Lanterns.mp3", "Paper Lanterns", "ALM", "Night Market Sampler", "Various Artists", 2024, 3, 9, "Electronic"),
    _psong("Color Out/Peer Sessions/01 - Open Line.mp3", "Open Line", "Color Out", "Peer Sessions", "Color Out", 2026, 1, 10, "Electronic"),
    _psong("Color Out/Peer Sessions/02 - Echo Room.mp3", "Echo Room", "Color Out", "Peer Sessions", "Color Out", 2026, 2, 11, "Electronic"),
    _psong("ALM/Split Series/01 - Split One.mp3", "Split One", "ALM", "Split Series", "ALM", 2022, 1, 10, "Electronic"),
    _psong("ALM/Split Series/02 - Split Two.mp3", "Split Two", "Color Out", "Split Series", "Color Out", 2022, 2, 10, "Electronic"),
]
# The compilation's songs are flagged so; the albums group them by name.
for _s in PEER_SONGS:
    _s["compilation"] = _s["metadata"]["album-artist"] == "Various Artists"
# Untagged files the database never read: Night Owl Radio's two shows, and
# the archive's 1,240 songs in 56 folders (past the folder cap).
PEER_UNTAGGED = ["Night Owl Radio/Episode 1.mp3", "Night Owl Radio/Episode 2.mp3"] + [
    "Night Owl Radio Archive/Show %03d/Hour %02d.mp3" % (show, hour)
    for show in range(1, 57) for hour in range(1, (23 if show <= 8 else 22) + 1)]
PEER_FILES = [s["filepath"] for s in PEER_SONGS] + [PEER_LIB + "/" + p for p in PEER_UNTAGGED]
PEER_BY_PATH = {s["filepath"]: s for s in PEER_SONGS}


def _name_key(s):
    return re.sub(r"\s+", " ", str(s or "").strip().lower())


def _peer_path(p):
    """A path as the peer's library holds it: no leading or trailing slash."""
    return "/".join(seg for seg in str(p or "").replace("\\", "/").split("/") if seg and seg != ".")


def _peer_albums():
    """The albums listing's items (db.js albumItems), by name: a group's
    credit is its rows' album-artist when they agree, null when not."""
    groups = {}
    for s in PEER_SONGS:
        groups.setdefault(s["metadata"]["album"], []).append(s)
    out = []
    for name in sorted(groups, key=str.lower):
        rows = groups[name]
        credits = {r["metadata"]["album-artist"] for r in rows}
        years = [r["metadata"]["year"] for r in rows]
        artists = []
        for r in rows:
            if r["metadata"]["artist"] not in artists:
                artists.append(r["metadata"]["artist"])
        out.append({"name": name, "year": max(years), "album_art_file": None,
                    "album_artist": credits.pop() if len(credits) == 1 else None, "artists": artists,
                    "compilation": any(r["compilation"] for r in rows), "year_min": min(years), "year_max": max(years),
                    "track_count": len(rows), "duration": sum(r["metadata"]["duration"] for r in rows)})
    return out


def _peer_artist_albums(artist):
    """An artist's albums (db.js /db/artists-albums): every album the artist
    is on, credited or performing, newest first, ties by name."""
    key = _name_key(artist)
    names = {s["metadata"]["album"] for s in PEER_SONGS
             if key in (_name_key(s["metadata"]["artist"]), _name_key(s["metadata"]["album-artist"]))}
    albums = [a for a in _peer_albums() if a["name"] in names]
    return sorted(albums, key=lambda a: (-(a["year"] or 0), a["name"].lower()))


def _peer_listing(directory, pull):
    """The file explorer (file-explorer.js): the library at the top, else the
    folder's folders and files, each file's tags when asked (null for a file
    the database never read)."""
    path = _peer_path(directory)
    if path in ("", "~"):
        if directory == "~":
            path = PEER_LIB
        else:
            return {"path": "/", "directories": [{"name": PEER_LIB}], "files": []}
    prefix = path + "/"
    dirs, files = [], []
    for fp in PEER_FILES:
        if not fp.startswith(prefix):
            continue
        rest = fp[len(prefix):].split("/")
        if len(rest) > 1:
            if rest[0] not in dirs:
                dirs.append(rest[0])
        else:
            entry = {"type": "mp3", "name": rest[0]}
            if pull:
                song = PEER_BY_PATH.get(fp)
                entry["metadata"] = {"filepath": fp, "metadata": song["metadata"] if song else None}
            files.append(entry)
    if not dirs and not files and path != PEER_LIB:
        return None
    return {"path": "/" + path + "/", "directories": [{"name": d} for d in sorted(dirs, key=str.lower)],
            "files": sorted(files, key=lambda f: f["name"].lower())}


def _peer_search(text):
    """db.js /db/search: artists, albums (the listing's credit and year),
    titles and files."""
    q = _name_key(text)
    hit = lambda s: q and q in _name_key(s)
    artists = sorted({s["metadata"]["artist"] for s in PEER_SONGS if hit(s["metadata"]["artist"])}, key=str.lower)
    albums = [{"name": a["name"], "album_art_file": None, "album_artist": a["album_artist"], "year": a["year"],
               "filepath": False} for a in _peer_albums() if hit(a["name"])]
    titles = [{"name": "%s - %s" % (s["metadata"]["artist"], s["metadata"]["title"]), "filepath": s["filepath"],
               "album_art_file": None, "metadata": s["metadata"]} for s in PEER_SONGS if hit(s["metadata"]["title"])]
    return {"artists": [{"name": a, "album_art_file": None, "filepath": False} for a in artists], "albums": albums,
            "title": titles, "files": [], "lyrics": []}


def _base(base):
    return "/".join(p.strip() for p in str(base or "").replace("\\", "/").split("/") if p.strip())


def _effective():
    """The destination in force (destination.js destinationFor): the saved
    one, else the first library's default."""
    saved = state["saved_destination"]
    if saved:
        return dict(saved, source="user")
    lib = LIBRARIES[0]
    return {"vpath": lib["vpath"], "base": "", "layout": lib["template"] or DEFAULT_LAYOUT, "source": "default"}


def _view():
    return {"destination": _effective(), "saved": state["saved_destination"], "libraries": LIBRARIES,
            "defaultLayout": DEFAULT_LAYOUT, "variables": LAYOUT_VARS}


def _same(shown, effective):
    """The jobs route's sameDestination: null only null; the library, the
    base as normalised, the layout exactly."""
    if not shown or not effective:
        return not shown and not effective
    return (shown.get("vpath") == effective.get("vpath") and _base(shown.get("base")) == _base(effective.get("base"))
            and shown.get("layout") == effective.get("layout"))


def _path_refusal(path):
    """Why a relative path would not do (path-template.js
    validateResolvedPath), in the server's words — None when it would."""
    if re.search(r"[\x00-\x1f]", path):
        return "Resolved path cannot contain control characters"
    if path.startswith("/") or path.startswith("\\"):
        return "Resolved path cannot start with / or \\"
    if re.match(r"^[a-zA-Z]:", path):
        return "Resolved path cannot start with a drive letter"
    if "~" in path or re.search(r"\$HOME\b|\$\{HOME\}", path):
        return "Resolved path cannot contain ~ or $HOME (no shell-style expansion)"
    for seg in re.split(r"[/\\]", path):
        if seg == "..":
            return "Resolved path cannot contain .. segments"
        if re.match(r"^[a-zA-Z]:", seg):
            return "Resolved path segments cannot start with a drive letter"
    return None


def _refusal(destination):
    """Why a destination would not be kept (destination.js
    validateDestination: the library, normalizeBase, the layout's tokens),
    in the server's words — None when it would. An unknown variable is
    quoted as typed, blanks and case kept (`t.raw`)."""
    if destination.get("vpath") not in [l["vpath"] for l in LIBRARIES]:
        return "destination: you cannot put files into library '%s'" % destination.get("vpath")
    base = _base(destination.get("base"))
    words = base and _path_refusal(base)
    if words:
        return "destination base folder: %s" % words
    for m in re.finditer(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}", destination.get("layout") or ""):
        if m.group(1).upper() not in LAYOUT_VARS:
            return "destination layout: Unknown variable %s. Supported: %s" % (
                m.group(0), ", ".join("{{%s}}" % v for v in LAYOUT_VARS))
    return None
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
    # A folder job keys on the peer AND the path, case kept (jobKey): any
    # recommendation with both has a folder key, a song's included.
    peer = (rec.get("peer") or {}).get("id")
    if rec.get("filepath") and peer is not None:
        keys["folder"] = "folder:%s|%s" % (peer, _peer_path(rec.get("filepath")))
    return keys


def _landed(rec, landing, destination):
    """Where a folder copy lands (federation-copy.js landingFor), as its
    result names it: the folder's own path under the library and the
    layout's leading folders (mirror), the place picked (`{vpath, path}`),
    or nowhere one folder names (tags)."""
    destination = destination or _effective()
    if isinstance(landing, dict) and landing.get("tags"):
        return None
    if isinstance(landing, dict) and "path" in landing:
        return "/".join(p for p in [landing.get("vpath") or destination["vpath"], _peer_path(landing["path"])] if p)
    lead = []
    for seg in _base(destination.get("layout")).split("/"):
        if "{{" in seg:
            break
        lead.append(seg)
    inside = _peer_path(rec.get("filepath")).split("/")[1:]
    return "/".join(p for p in [destination["vpath"], _base(destination.get("base"))] + lead + inside if p)


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
        if job["plugin"] == NOOP_PLUGIN:
            _walk_noop(job, age)
            continue
        if job["state"] == "queued" and age >= JOB_QUEUED_S:
            job.update(state="running", progress=0.5, statusText="song 1 of 2")
        if job["state"] == "running" and age >= JOB_QUEUED_S + JOB_RUNNING_S:
            scope = (job.get("params") or {}).get("scope")
            rec = job["recommendation"]
            if scope and PEER_URL and (rec.get("peer") or {}).get("id") == PEER_ID:
                result = _peer_result(scope, rec, job.get("params") or {})
            elif scope:
                result = {"scope": scope, "counts": {"total": 2, "copied": 2, "skipped": 0, "failed": 0},
                          "songs": {"total": 2, "copied": [{}, {}], "skipped": [], "failed": []}, "stopped": None,
                          "peer": rec.get("peer")}
            else:
                result = {"copied": {"vpath": "copies", "filepath": "copies/%s/%s/%s.mp3" % (
                    rec.get("artist"), rec.get("album"), rec.get("title")), "title": rec.get("title")}}
            job.update(state="done", progress=1, statusText=None, result=result)

def _peer_result(scope, rec, params):
    """A many-song copy of Stage Peer B's, ended as federation-copy words
    it: its songs by the file each came from — the last one already yours
    (the spec's "2 copied · 1 already yours"), the rest copied — and, for a
    folder, the folder it landed in (`landed`, none for a tags landing)."""
    if scope == "folder":
        top = _peer_path(rec.get("filepath")) + "/"
        songs = [s for s in PEER_SONGS if s["filepath"].startswith(top)]
    else:
        songs = [s for s in PEER_SONGS if _name_key(s["metadata"]["album"]) == _name_key(rec.get("album"))]
    def landed_file(s):
        m = s["metadata"]
        return "copies/%s/%s/%s" % (m["album-artist"], m["album"], s["filepath"].rsplit("/", 1)[-1])
    copied = [{"from": s["filepath"], "filepath": landed_file(s), "title": s["metadata"]["title"]} for s in songs[:-1]]
    skipped = [{"from": s["filepath"], "why": "owned", "at": landed_file(s)} for s in songs[-1:]]
    result = {"scope": scope, "counts": {"total": len(songs), "copied": len(copied), "skipped": len(skipped), "failed": 0},
              "songs": {"total": len(songs), "copied": copied, "skipped": skipped, "failed": []}, "stopped": None,
              "peer": rec.get("peer")}
    if scope == "folder":
        result["folder"] = {"path": _peer_path(rec.get("filepath")), "name": rec.get("title"),
                            "landed": _landed(rec, params.get("landing"), params.get("destination"))}
        result["layout"] = (params.get("destination") or _effective()).get("layout")
    return result


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


# ── The Peer recs room (FAKE_P2P) ────────────────────────────────────────────
# The stage's answers (mStream test/stage/peer-recs-stage.mjs), read from the
# fixtures the Rust tests read, so the two never drift apart.
FIXTURES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fixtures", "peer_recs")


def _fixture(name):
    with open(os.path.join(FIXTURES, name + ".json"), encoding="utf-8") as f:
        return json.load(f)


# The taste route's schema (mStream src/api/discovery-taste.js tasteSchema,
# limitSchema): every key it knows, with its caps. Anything else is Joi's 400.
TASTE_KEYS = ("window", "pin", "exclude", "sources", "limit", "newArtistsOnly", "ignoreVPaths", "moods",
              "networkPeers", "networkOnRandomSeed")
TASTE_LIMITS = {"seeds": (1, 12), "tracks": (0, 50), "albums": (0, 24), "network": (0, 50), "peers": (0, 25)}
TASTE_WINDOWS = (7, 30, 90, 0)
TASTE_SOURCES = ("library", "network", "peers")
# The generation-3 keys Discover's own bodies must never carry.
GEN3_KEYS = ("networkPeers", "networkOnRandomSeed")
# An Add's walk: the poll after the post still reads `adding`, and the peer
# lands (or fails) this long after that poll. Static Garden's download is the
# stage's slow one.
ADD_LAND_S = 2.0
STATIC_LAND_S = 120.0
# Get it's job (noop-acquire): running at once, done this long after.
NOOP_PLUGIN = "noop-acquire"
NOOP_RUNNING_S = 2.0
NOOP_STEPS = 3
# The previews (the stage's Deezer mock): a clip this long, Arcade Rain with
# no match, Quiet Engine rate-limited once, Long Exposure's clip URL dead.
CLIP_SECONDS = 2.0
CLIP_RATE = 16000
NO_PREVIEW = "Arcade Rain"
LIMITED_ONCE = "Quiet Engine"
DEAD_CLIP = "Long Exposure"

if P2P:
    PEER_ROWS = _fixture("peers_admin")
    TASTE_ALL = _fixture("taste_all")
    TASTE_RANDOM = _fixture("taste_random")
    TASTE_NOTHING_NEW = _fixture("taste_attic")
    TASTE_NOT_ON_SHELF = _fixture("taste_not_on_shelf")
    PREVIEW_TEMPLATE = _fixture("preview_deezer")["result"]["preview"]
    PEER_BY_LABEL = {p["label"]: p["endpointId"] for p in PEER_ROWS["peers"]}
    BASEMENT = PEER_BY_LABEL["Basement Hi-Fi"]
    KITCHEN = PEER_BY_LABEL["Kitchen Radio"]
    F3A = PEER_BY_LABEL["mStream · 3f9a"]
    ATTIC = PEER_BY_LABEL["Attic Tapes"]
    LIGHTHOUSE = PEER_BY_LABEL["Lighthouse Tapes"]
    STATIC = PEER_BY_LABEL["Static Garden"]
    ROOFTOP = PEER_BY_LABEL["Rooftop FM"]
    # One shelf peer's answer, by its id; a held peer not here has only
    # songs this library owns, so its tab reads nothing new.
    TASTE_TABS = {BASEMENT: "taste_basement", KITCHEN: "taste_kitchen", F3A: "taste_3f9a", ATTIC: "taste_attic"}
    TASTE_TAB_ANSWERS = {pid: _fixture(name) for pid, name in TASTE_TABS.items()}
    # The leads, in rank order: a lead's rank names its clip.
    LEADS = TASTE_ALL["network"]["results"]
    # The seeds this library knows, by path: the history's (finished songs)
    # and the random draw's; a pin takes the pinned form.
    HISTORY_SEEDS = TASTE_ALL["seeds"]
    SEED_POOL = {s["filepath"]: s for s in HISTORY_SEEDS + TASTE_RANDOM["seeds"]}
    # The shelf as staged, and what an Add swaps out when it lands.
    p2p = {"rows": {p["endpointId"]: p for p in PEER_ROWS["peers"]},
           "shelf": [p["endpointId"] for p in PEER_ROWS["peers"] if p["onShelf"]],
           "adds": {}, "limited": set()}
    SWAPS = {LIGHTHOUSE: ATTIC}
    FAILS = {ROOFTOP: "offline"}

# The stage's listing, in its order: Discover's two plus a preview plug-in
# (the room's first with `preview`) and a Get it one (every acquire but
# federation-copy).
P2P_LISTING = {
    "plugins": [
        PLUGIN_LISTING["plugins"][0],
        {"name": "deezer", "title": "Preview on Deezer", "capabilities": ["preview"], "scope": "server",
         "scopes": ["song"], "settings": [], "enabled": True, "available": True},
        PLUGIN_LISTING["plugins"][1],
        {"name": NOOP_PLUGIN, "title": "No-op acquire (test only)", "capabilities": ["acquire"], "scope": "server",
         "scopes": ["song", "album"], "settings": ["note", "token"], "enabled": True, "available": True},
    ],
    "jobs": PLUGIN_LISTING["jobs"],
}


def _iso(t):
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(t)) + ".%03dZ" % int((t % 1) * 1000)


def _add_wire(entry):
    return {"state": entry["state"], "code": entry["code"], "at": _iso(entry["at"])}


def _note_add(pid):
    entry = p2p["adds"][pid]
    state["peer_adds"][p2p["rows"][pid]["label"]] = {"state": entry["state"], "code": entry["code"]}


def _walk_adds(now):
    """Every Add where the clock puts it: Lighthouse Tapes lands (and Attic
    Tapes leaves the shelf), Rooftop FM fails offline — each 2 s after the
    first poll that read it adding — and Static Garden lands at 120 s."""
    for pid, entry in p2p["adds"].items():
        if entry["state"] != "adding":
            continue
        if pid == STATIC:
            due = entry["posted"] + STATIC_LAND_S
        elif entry["seen"] is not None:
            due = entry["seen"] + ADD_LAND_S
        else:
            continue
        if now < due:
            continue
        if pid in FAILS:
            entry.update(state="failed", code=FAILS[pid], at=now)
        else:
            entry.update(state="done", code=None, at=now)
            gone = SWAPS.get(pid)
            if gone in p2p["shelf"]:
                p2p["shelf"].remove(gone)
            if pid not in p2p["shelf"]:
                p2p["shelf"].append(pid)
        _note_add(pid)


def _peer_view():
    """GET /api/v1/discovery/p2p/peers as buildPeerView answers it: the shelf
    online first, then by label; for an admin the catalog's online peers not
    held, by library size (an Add never reorders a row), each with its Add
    state; a member sees no addable row and no state."""
    def row(pid, on_shelf):
        # A peer an Add landed keeps its catalog row's `pinned: false`: a
        # page Add never pins.
        r = dict(p2p["rows"][pid], onShelf=on_shelf)
        entry = p2p["adds"].get(pid)
        r["add"] = _add_wire(entry) if entry and not MEMBER else None
        return r
    shelf = sorted((row(pid, True) for pid in p2p["shelf"]),
                   key=lambda r: (not r["online"], r["label"].casefold(), r["endpointId"]))
    addable = []
    if not MEMBER:
        addable = [row(pid, False) for pid in p2p["rows"]
                   if pid not in p2p["shelf"] and (p2p["rows"][pid]["online"] or pid in p2p["adds"])]
        addable.sort(key=lambda r: (-r["rowCount"], r["endpointId"]))
    held = len(p2p["shelf"])
    return {"canAdd": not MEMBER, "shelf": dict(PEER_ROWS["shelf"], held=held, searchable=held),
            "peers": shelf + addable}


def _joi_refusal(body):
    """Why the taste route's schema refuses a body, in Joi's words; None when
    it takes it."""
    if not isinstance(body, dict):
        return '"value" must be of type object'
    for key in body:
        if key not in TASTE_KEYS:
            return '"%s" is not allowed' % key
    window = body.get("window", 30)
    if window not in TASTE_WINDOWS or isinstance(window, bool):
        return '"window" must be one of [7, 30, 90, 0]'
    for key, cap in (("pin", 12), ("exclude", 200)):
        items = body.get(key, [])
        if not isinstance(items, list) or not all(isinstance(i, str) and i for i in items):
            return '"%s" must be an array of strings' % key
        if len(items) > cap:
            return '"%s" must contain less than or equal to %d items' % (key, cap)
    sources = body.get("sources", list(TASTE_SOURCES))
    if not isinstance(sources, list):
        return '"sources" must be an array'
    for i, source in enumerate(sources):
        if source not in TASTE_SOURCES:
            return '"sources[%d]" must be one of [library, network, peers]' % i
        if source in sources[:i]:
            return '"sources[%d]" contains a duplicate value' % i
    limit = body.get("limit", {})
    if not isinstance(limit, dict):
        return '"limit" must be of type object'
    for key, value in limit.items():
        if key not in TASTE_LIMITS:
            return '"limit.%s" is not allowed' % key
        low, high = TASTE_LIMITS[key]
        if not isinstance(value, int) or isinstance(value, bool):
            return '"limit.%s" must be a number' % key
        if value < low:
            return '"limit.%s" must be greater than or equal to %d' % (key, low)
        if value > high:
            return '"limit.%s" must be less than or equal to %d' % (key, high)
    for key in ("newArtistsOnly", "networkOnRandomSeed"):
        if key in body and not isinstance(body[key], bool):
            return '"%s" must be a boolean' % key
    if "networkPeers" in body:
        ids = body["networkPeers"]
        if not isinstance(ids, list):
            return '"networkPeers" must be an array'
        if not ids:
            return '"networkPeers" must contain at least 1 items'
        if len(ids) > 50:
            return '"networkPeers" must contain less than or equal to 50 items'
        seen = []
        for i, pid in enumerate(ids):
            if not isinstance(pid, str):
                return '"networkPeers[%d]" must be a string' % i
            if not re.fullmatch(r"[0-9a-fA-F]*", pid):
                return '"networkPeers[%d]" must only contain hexadecimal characters' % i
            if len(pid) != 64:
                return '"networkPeers[%d]" length must be 64 characters long' % i
            if pid.lower() in seen:
                return '"networkPeers[%d]" contains a duplicate value' % i
            seen.append(pid.lower())
    return None


def _seeds(body):
    """The seeds the route picks (collectSeeds): the pins first, as pinned
    seeds (a path it does not know is dropped), then the history's finished
    songs less `exclude`, else the random draw."""
    limit = body.get("limit", {}).get("seeds", 12)
    exclude = set(body.get("exclude", []))
    seeds, dropped, have = [], 0, set()
    for path in body.get("pin", []):
        if len(seeds) >= limit:
            break
        seed = SEED_POOL.get(path)
        if seed is None:
            dropped += 1
            continue
        if seed["hash"] in have:
            continue
        have.add(seed["hash"])
        seeds.append(dict(seed, pinned=True, outcome=None))
    out = {"seeds": seeds, "dropped": dropped, "seedSource": "pinned" if seeds else None, "fallback": False,
           "widenedFrom": None,
           "excluded": [{"hash": s["hash"], "filepath": s["filepath"], "metadata": s["metadata"]}
                        for s in SEED_POOL.values() if s["hash"] in exclude]}
    room = limit - len(seeds)
    if room > 0:
        history = [s for s in HISTORY_SEEDS if s["hash"] not in exclude and s["hash"] not in have][:room]
        if history:
            seeds.extend(history)
            out["seedSource"] = out["seedSource"] or "history"
        elif not seeds:
            # No finished song left to seed from: the window widens to all
            # time, then the random draw (taste_random, a user with no
            # plays).
            seeds.extend(TASTE_RANDOM["seeds"][:room])
            window = body.get("window", 30)
            out.update(seedSource="random", fallback=True, window=TASTE_RANDOM["window"], widenedFrom=window or None)
    return out


def _network_part(ids, random_seed):
    """The answer's network part for these networkPeers (None: the whole
    shelf), and the fixture's name that gave it. A random seed ranks the
    leads from its own vector, so its answers are taste_random's."""
    whole = TASTE_RANDOM if random_seed else TASTE_ALL
    name = "taste_random" if random_seed else "taste_all"
    if not ids:
        return dict(whole["network"], searched=dict(whole["network"]["searched"], peers=len(p2p["shelf"]))), name
    held = [pid for pid in ids if pid in p2p["shelf"]]
    if not held:
        return TASTE_NOT_ON_SHELF["network"], "taste_not_on_shelf"
    if len(ids) == 1 and not random_seed:
        tab = TASTE_TAB_ANSWERS.get(held[0])
        return (tab or TASTE_NOTHING_NEW)["network"], TASTE_TABS.get(held[0], "taste_attic")
    results = [r for r in whole["network"]["results"] if r["peer"]["endpointId"] in held]
    return {"results": results, "searched": {"peers": len(held), "tracks": len(results)},
            "reason": None if results else "nothing-new"}, name


def _network_answer(body):
    """POST /api/v1/discovery/taste with the network source (the Peer recs
    room's R1–R8): checked, then answered by the tab. A 400 is (status,
    words); an answer is (200, body, fixture)."""
    words = _joi_refusal(body)
    if words:
        return 400, {"error": words}, None
    picked = dict({"window": body.get("window", 30)}, **_seeds(body))
    random_seed = picked["seedSource"] == "random"
    ids = [pid.lower() for pid in body.get("networkPeers", [])] or None
    network, name = _network_part(ids, random_seed)
    network = copy.deepcopy(network)
    network["results"] = network["results"][:body.get("limit", {}).get("network", 12)]
    # Each lead names the seed it sits closest to: one of this answer's.
    hashes = [s["hash"] for s in picked["seeds"]]
    for lead in network["results"]:
        if hashes and lead.get("seedHash") not in hashes:
            lead["seedHash"] = hashes[0]
    if random_seed and body.get("networkOnRandomSeed") is not True:
        network, name = None, "taste_network_null"
    answer = copy.deepcopy(TASTE_ALL)
    answer.update(picked, network=network, library=None, peers=None)
    return 200, answer, name


def _lead(rec):
    """The lead a recommendation names, and its rank (1-based), by title and
    artist; (None, None) for one the shelf never gave."""
    rec = rec if isinstance(rec, dict) else {}
    for n, lead in enumerate(LEADS, 1):
        if lead["title"] == rec.get("title") and lead["artist"] == rec.get("artist"):
            return lead, n
    return None, None


def _preview(lead, n, port):
    """The Deezer mock's preview of a lead, shaped as preview_deezer's: its
    clip on this fake, an https link, and an artwork URL never to fetch."""
    base = "http://127.0.0.1:%d" % port
    return dict(PREVIEW_TEMPLATE, url="%s/_clip/%d.wav" % (base, n), title=lead["title"], artist=lead["artist"],
                album=lead.get("album"), link="https://www.deezer.com/track/%d" % (3995000 + n),
                artwork="%s/_artwork/%d.png" % (base, n))


_CLIPS = {}


def _clip(n):
    """A 2 s mono 16-bit PCM WAV: a quiet tone a pentatonic step per lead,
    so two clips never sound alike (symphonia decodes it, no ffmpeg)."""
    if n not in _CLIPS:
        steps = [0, 2, 4, 7, 9]
        freq = 220.0 * 2 ** ((steps[(n - 1) % 5] + 12 * ((n - 1) // 5)) / 12)
        frames = int(CLIP_SECONDS * CLIP_RATE)
        pcm = b"".join(struct.pack("<h", int(3000 * math.sin(2 * math.pi * freq * i / CLIP_RATE)))
                       for i in range(frames))
        fmt = struct.pack("<HHIIHH", 1, 1, CLIP_RATE, CLIP_RATE * 2, 2, 16)
        _CLIPS[n] = (b"RIFF" + struct.pack("<I", 4 + 8 + len(fmt) + 8 + len(pcm)) + b"WAVE"
                     + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(pcm)) + pcm)
    return _CLIPS[n]


def _walk_noop(job, age):
    """Get it's job (noop-acquire) where its age puts it: running from the
    start, a step a third of the way, then done — or failed for a title
    with "fail" in it, as the real plug-in refuses one."""
    if job["state"] != "running":
        return
    if age < NOOP_RUNNING_S:
        step = min(NOOP_STEPS, 1 + int(age / NOOP_RUNNING_S * NOOP_STEPS))
        job.update(progress=step / NOOP_STEPS, statusText="step %d of %d" % (step, NOOP_STEPS))
        return
    title = str(job["recommendation"].get("title") or "")
    if re.search("fail", title, re.I):
        job.update(state="failed", progress=None, statusText=None, error='noop-acquire refused "%s"' % title)
    else:
        job.update(state="done", progress=1, statusText=None,
                   result={"echoed": title, "steps": NOOP_STEPS, "scope": "song"})


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
        from urllib.parse import unquote, urlsplit

        if P2P and unquote(urlsplit(self.path).path).startswith("/_clip/"):
            return self._clip_get(unquote(urlsplit(self.path).path), head=True)
        if not LIBRARY:
            return self.send_error(501)
        path = unquote(urlsplit(self.path).path)
        library["asked"].append("HEAD " + path)
        if path.startswith("/media/"):
            return self._media(path[len("/media/"):], head=True)
        return self._bytes(404, b"", head=True)

    def _png(self, data):
        self.send_response(200)
        self.send_header("Content-Type", "image/png")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _peer(self, method, body):
        """Stage Peer B's reads, as mStream answers them (FAKE_ROLE=peer)."""
        path = urllib.parse.unquote(self.path.split("?")[0])
        if path == "/_state":
            return self._json(state)
        state["asked"].append(method + " " + path)
        body = body if body is not None else {}
        if path == "/api/v1/ping":
            return self._json({"vpaths": [PEER_LIB], "noFileModify": True, "noUpload": True})
        if path in ("/api", "/api/"):
            # The layered payload a guest reads — no ping crosses the proxy.
            return self._json({"server": "6.30.0", "features": {"discovery": False},
                               "user": {"vpaths": [PEER_LIB], "noFileModify": True, "noUpload": True}})
        if path.startswith("/album-art/"):
            return self._png(PEER_COVER)
        if path == "/api/v1/db/status":
            return self._json({"totalFileCount": len(PEER_SONGS), "locked": False})
        if path == "/api/v1/file-explorer":
            listing = _peer_listing(str(body.get("directory", "")), bool(body.get("pullMetadata")))
            if listing is None:
                return self._status(404, {"error": "Directory not found"})
            return self._json(listing)
        if path == "/api/v1/file-explorer/recursive":
            state["recursive_bodies"].append(body)
            top = _peer_path(body.get("directory")) + "/"
            return self._json([fp for fp in PEER_FILES if fp.startswith(top)])
        if path == "/api/v1/db/albums":
            return self._json({"albums": _peer_albums()})
        if path == "/api/v1/db/artists-albums":
            return self._json({"albums": _peer_artist_albums(body.get("artist"))})
        if path == "/api/v1/db/album-songs":
            def wanted(s):
                m = s["metadata"]
                if _name_key(m["album"]) != _name_key(body.get("album")):
                    return False
                if body.get("album_artist") and _name_key(m["album-artist"]) != _name_key(body["album_artist"]):
                    return False
                if body.get("artist") and _name_key(body["artist"]) not in (_name_key(m["artist"]), _name_key(m["album-artist"])):
                    return False
                return not body.get("year") or m["year"] == body["year"]
            return self._json([s for s in PEER_SONGS if wanted(s)])
        if path == "/api/v1/db/artists":
            return self._json({"artists": sorted({s["metadata"]["artist"] for s in PEER_SONGS}, key=str.lower)})
        if path == "/api/v1/db/genres":
            genres = {}
            for s in PEER_SONGS:
                for g in s["metadata"]["genres"]:
                    genres[g] = genres.get(g, 0) + 1
            return self._json({"genres": [{"name": g, "track_count": n} for g, n in sorted(genres.items())]})
        if path == "/api/v1/db/genre-songs":
            return self._json([s for s in PEER_SONGS if body.get("genre") in s["metadata"]["genres"]])
        if path == "/api/v1/db/search":
            return self._json(_peer_search(body.get("search")))
        if path == "/api/v1/db/recent/added":
            return self._json(list(reversed(PEER_SONGS))[: int(body.get("limit") or 100)])
        if path == "/api/v1/db/metadata":
            song = PEER_BY_PATH.get(_peer_path(body.get("filepath")))
            return self._json({"filepath": body.get("filepath"), "metadata": song["metadata"] if song else None})
        if path == "/api/v1/db/metadata/batch":
            state["batch_bodies"].append(body)
            paths = body if isinstance(body, list) else []
            return self._json({fp: {"filepath": fp, "metadata": PEER_BY_PATH[fp]["metadata"] if fp in PEER_BY_PATH else None}
                               for fp in paths})
        return self._status(404, {"error": "Not Found"})

    def _proxy(self, method, body):
        """mStream's browse proxy (federation-browse.js) in front of Stage
        Peer B: an API read screened against the exact allowlist before it
        is dialled — 403 off it — its JSON body re-serialized, the peer's
        status and body passed back; art as /album-art/, `compress` the one
        query kept."""
        raw, _, query = self.path.partition("?")
        rest = raw[len("/api/v1/federation/peers/%d/" % PEER_ID):]
        if rest.startswith("art/"):
            remote = "/album-art/" + rest[len("art/"):]
            kept = [kv for kv in query.split("&") if kv.split("=")[0] == "compress"]
            remote_query = "?" + "&".join(kept) if kept else ""
        elif rest.startswith("api/"):
            remote = "/" + rest[len("api/"):]
            remote_query = ""
            screened = method + " " + urllib.parse.unquote(remote)
            if screened not in FED_ALLOWED:
                state["proxy_refused"].append(screened)
                return self._status(403, {"error": "Route not available over federation"})
        else:
            return self._status(404, {"error": "Not Found"})
        state["proxied"].append(method + " " + urllib.parse.unquote(remote))
        data = json.dumps(body if body is not None else {}).encode() if method != "GET" else None
        request = urllib.request.Request(PEER_URL + remote + remote_query, data=data, method=method,
                                         headers={"Content-Type": "application/json"} if data else {})
        try:
            with urllib.request.urlopen(request, timeout=15) as answer:
                status, kind, payload = answer.status, answer.headers.get("Content-Type"), answer.read()
        except urllib.error.HTTPError as e:
            status, kind, payload = e.code, e.headers.get("Content-Type"), e.read()
        except Exception:
            return self._status(502, {"error": "Peer unreachable"})
        self.send_response(status)
        self.send_header("Content-Type", kind or "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _attic(self, body):
        """The Discover room's connected server (id 7, Attic) through the
        browse proxy: its artist's albums answered, every other read the
        empty answer it always was."""
        if self.path == "/api/v1/federation/peers/7/api/api/v1/db/artists-albums":
            return self._json({"albums": ATTIC_ALBUMS.get(_name_key((body or {}).get("artist")), [])})
        return self._json({})

    def _ranged(self, data, kind, head=False):
        """`data` whole (200), a Range of it (206), or HEAD — as a CDN serves
        a clip to a streaming reader."""
        rng = self.headers.get("Range", "")
        if rng.startswith("bytes=") and not head:
            first, _, last = rng[6:].partition("-")
            start = int(first or 0)
            end = min(int(last) + 1, len(data)) if last else len(data)
            if start >= len(data):
                return self._bytes(416, b"", extra=[("Content-Range", "bytes */%d" % len(data))])
            span = "bytes %d-%d/%d" % (start, end - 1, len(data))
            return self._bytes(206, data[start:end], kind, extra=[("Content-Range", span), ("Accept-Ranges", "bytes")])
        return self._bytes(200, data, kind, extra=[("Accept-Ranges", "bytes")], head=head)

    def _clip_get(self, path, head=False):
        """/_clip/<rank>.wav: the lead's clip; Long Exposure's is dead (404),
        and so is a rank no lead has."""
        m = re.fullmatch(r"/_clip/(\d+)\.wav", path)
        n = int(m.group(1)) if m else 0
        if not head:
            state["clip_gets"].append(n)
        if not 1 <= n <= len(LEADS) or LEADS[n - 1]["title"] == DEAD_CLIP:
            return self._bytes(404, b'{"error":"Not Found"}', "application/json", head=head)
        return self._ranged(_clip(n), "audio/wav", head=head)

    def _p2p_get(self, path):
        """The Peer recs room's reads: True when answered."""
        if path == "/api/v1/discovery/p2p/peers":
            state["peer_polls"] += 1
            now = time.time()
            _walk_adds(now)
            # This poll reads every fresh Add as adding; the walk counts
            # from it.
            for pid, entry in p2p["adds"].items():
                if entry["state"] == "adding" and entry["seen"] is None:
                    entry["seen"] = now
            self.send_response(200)
            data = json.dumps(_peer_view()).encode()
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return True
        if path.startswith("/_clip/"):
            self._clip_get(path)
            return True
        if path.startswith("/_artwork/"):
            # The previews' artwork: the room draws none, so any get here
            # is a failure the legs count.
            state["art_gets"] += 1
            state["art_paths"].append(path)
            self._png(COVER)
            return True
        m = re.fullmatch(r"/api/v1/discovery/plugin-jobs/(\d+)", path)
        if m:
            state["job_gets"] += 1
            _walk_jobs()
            job = next((j for j in state["jobs"] if j["id"] == int(m.group(1))), None)
            if job is None:
                self._status(404, {"error": "No such job"})
            else:
                self._json({"job": _public(job)})
            return True
        return False

    def _add(self, body):
        """POST …/p2p/peer-dbs/add (discovery-peer-add.js add()): the guard,
        Joi, then the peer's Add as it stands."""
        state["adds"] += 1
        state["add_bodies"].append(body)

        def answer(status, payload):
            state["add_answers"].append(status)
            return self._status(status, payload)
        if MEMBER:
            return answer(403, {"error": "Admin access required"})
        pid = body.get("endpointId") if isinstance(body, dict) else None
        if not isinstance(pid, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", pid):
            return answer(400, {"error": '"endpointId" length must be 64 characters long'})
        pid = pid.lower()
        if pid not in p2p["rows"]:
            return answer(404, {"error": "not-in-catalog"})
        now = time.time()
        _walk_adds(now)
        entry = p2p["adds"].get(pid)
        if entry and entry["state"] == "adding":
            return answer(202, dict(endpointId=pid, **_add_wire(entry)))
        if pid in p2p["shelf"]:
            done = entry if entry and entry["state"] == "done" else {"state": "done", "code": None, "at": now}
            return answer(200, dict(endpointId=pid, **_add_wire(done)))
        p2p["adds"][pid] = {"state": "adding", "code": None, "at": now, "posted": now, "seen": None}
        _note_add(pid)
        return answer(202, dict(endpointId=pid, **_add_wire(p2p["adds"][pid])))

    def _resolve_preview(self, body):
        """POST …/plugins/deezer/resolve: the stage's Deezer mock behind the
        resolve route (discovery-plugins.js), the recommendation echoed."""
        state["resolves"] += 1
        rec = body.get("recommendation") if isinstance(body, dict) else None
        if not isinstance(rec, dict):
            return self._status(400, {"error": '"recommendation" is required'})
        lead, n = _lead(rec)
        title = rec.get("title")
        note = lambda answer: state["preview_resolves"].append(
            {"title": title, "answer": answer, "recommendation": rec})
        if lead is not None and title == LIMITED_ONCE and title not in p2p["limited"]:
            p2p["limited"].add(title)
            note("429")
            return self._status(429, {"error": "plug-in deezer is rate-limited right now — try again shortly"})
        preview = None
        if lead is not None and title != NO_PREVIEW:
            preview = _preview(lead, n, self.server.server_address[1])
        note("null" if preview is None else "dead" if title == DEAD_CLIP else "preview")
        return self._json({"plugin": "deezer", "capabilities": ["preview"], "key": _keys(rec)["song"],
                           "recommendation": rec, "result": {"preview": preview}})

    def _noop_start(self, body):
        """POST …/plugins/noop-acquire/jobs: Get it's start, the destination
        the client showed checked as the jobs route checks an acquire's."""
        state["job_starts"].append(body)
        rec = body.get("recommendation") if isinstance(body, dict) else None
        if not isinstance(rec, dict):
            state["job_answers"].append(400)
            return self._status(400, {"error": '"recommendation" is required'})
        key = _keys(rec)["song"]
        _walk_jobs()
        live = [j for j in state["jobs"]
                if j["plugin"] == NOOP_PLUGIN and j["key"] == key and j["state"] in ("queued", "running")]
        effective = _effective()
        if "destination" in body and not _same(body["destination"], effective) and not live:
            state["job_answers"].append("destination_changed")
            return self._status(409, {"error": "destination_changed", "destination": effective,
                                      "message": "your collection destination has changed — check where this will land and ask again"})
        if live:
            state["job_answers"].append(200)
            return self._json({"created": False, "job": _public(live[0])})
        state["job_answers"].append(202)
        job = {"id": 1 + max([j["id"] for j in state["jobs"]] or [0]), "plugin": NOOP_PLUGIN, "key": key,
               "recommendation": rec, "params": {"destination": effective}, "state": "running",
               "progress": 1 / NOOP_STEPS, "statusText": "step 1 of %d" % NOOP_STEPS, "result": None,
               "error": None, "attempts": 1, "cancelRequested": False,
               "createdAt": 1760000000000 + len(state["jobs"]), "_born": time.time()}
        state["jobs"].append(job)
        return self._status(202, {"created": True, "job": _public(job)})

    def _p2p_post(self, path, body):
        """The Peer recs room's writes and resolves: True when answered."""
        if path == "/api/v1/admin/discovery/p2p/peer-dbs/add":
            self._add(body)
            return True
        if path == "/api/v1/discovery/plugins/deezer/resolve":
            self._resolve_preview(body)
            return True
        if path == "/api/v1/discovery/plugins/%s/jobs" % NOOP_PLUGIN:
            self._noop_start(body)
            return True
        return False

    def do_GET(self):
        if PEER_ROLE:
            return self._peer("GET", None)
        if PEER_URL and self.path.startswith("/api/v1/federation/peers/%d/" % PEER_ID):
            return self._proxy("GET", None)
        if PEER_URL and self.path == "/api/v1/federation/peers":
            return self._json({"peers": [{"id": PEER_ID, "name": PEER_NAME, "lastSeen": None, "lastStatus": None,
                                          "useDiscovery": False, "endpointId": None, "direct": None}]})
        if self._walled():
            return self._deny()
        if LIBRARY and self._library_get():
            return None
        if P2P and self._p2p_get(urllib.parse.unquote(self.path.split("?")[0])):
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
            state["pings"] += 1
            ping = {"vpaths": sorted(state["directories"]), "noFileModify": False, "noUpload": False}
            if TASTE:
                ping.update({"discovery": True, "discoveryTaste": 1, "federationDiscovery": True})
            if PLUGINS:
                ping["discoveryPlugins"] = True
            if PEER_URL:
                ping["federationBrowse"] = True
            if P2P:
                ping.update({"discoveryP2p": True, "discoveryTaste": 3, "discoveryP2pPeers": 1,
                             "discoveryPath": True})
            return self._json(ping)
        if PLUGINS and self.path == "/api/v1/discovery/plugins":
            return self._json(P2P_LISTING if P2P else PLUGIN_LISTING)
        if PLUGINS and self.path == "/api/v1/discovery/collection/destination":
            state["destination_gets"] += 1
            return self._json(_view())
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
            state["art_paths"].append(self.path)
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
        if LIBRARY and self._library_post(body):
            return None
        if PEER_ROLE:
            return self._peer("POST", body)
        if PEER_URL and self.path.startswith("/api/v1/federation/peers/%d/" % PEER_ID):
            return self._proxy("POST", body)
        if PLUGINS and self.path.startswith("/api/v1/federation/peers/7/api/"):
            return self._attic(body)
        if PLUGINS and self.path == "/api/v1/db/artists-albums":
            return self._json({"albums": OWN_ALBUMS.get(_name_key(body.get("artist")), [])})
        if P2P and self._p2p_post(self.path, body):
            return None
        if self.path == "/api/v1/auth/login":
            if AUTH and body.get("username") not in state["users"]:
                return self._deny()
            return self._json(
                {"token": "fake-token", "vpaths": sorted(state["directories"])}
            )
        if PLUGINS and self.path == "/_move_destination":
            state["saved_destination"] = body.get("destination")
            return self._json(_view())
        if PLUGINS and self.path == "/api/v1/file-explorer":
            directory = "/" + "/".join(p for p in str(body.get("directory") or "").split("/") if p)
            state["folder_lists"].append(directory)
            return self._json({"path": directory + "/", "directories": [{"name": n} for n in FOLDERS.get(directory, [])],
                               "files": []})
        if PLUGINS and self.path == "/api/v1/discovery/plugins/federation-copy/jobs":
            rec = body.get("recommendation") or {}
            state["job_starts"].append(body)
            if NO_PIN and "destination" in body:
                state["job_answers"].append(400)
                return self._status(400, {"error": '"destination" is not allowed'})
            if rec.get("title") == "Taken":
                state["job_answers"].append(409)
                return self._status(409, {"error": "someone else on this server is already getting this — try again in a moment"})
            if rec.get("title") == "Full":
                state["job_answers"].append(429)
                return self._status(429, {"error": "you have 20 jobs queued or running — wait for some to finish (the limit is 20)"})
            scope = body.get("scope")
            key = _keys(rec)[scope or "song"] if (scope or "song") in _keys(rec) else None
            _walk_jobs()
            live = [j for j in state["jobs"] if j["key"] == key and j["state"] in ("queued", "running")]
            # The destination the client showed, when it showed one: no
            # longer the one in force is a 409 and nothing starts — unless
            # a live job already covers this recommendation, or the copy
            # lands in a place picked for it (`{vpath, path}`), which no
            # saved destination decides.
            effective = _effective()
            landing = body.get("landing")
            picked = isinstance(landing, dict) and "path" in landing
            if not NO_PIN and "destination" in body and not _same(body["destination"], effective) and not live and not picked:
                state["job_answers"].append("destination_changed")
                return self._status(409, {"error": "destination_changed", "destination": effective,
                                          "message": "your collection destination has changed — check where this will land and ask again"})
            if live:
                state["job_answers"].append(200)
                return self._json({"created": False, "job": _public(live[0])})
            state["job_answers"].append(202)
            params = {} if NO_PIN else {"destination": effective}
            if scope:
                params["scope"] = scope
            if scope == "folder" and landing is not None:
                params["landing"] = landing
            job = {"id": len(state["jobs"]) + 1, "plugin": "federation-copy", "key": key, "recommendation": rec,
                   "params": params or None, "state": "queued", "progress": None,
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
        if P2P and self.path == "/api/v1/discovery/taste" and isinstance(body, dict) \
                and "network" in (body.get("sources") or []):
            # The Peer recs room's ask: its own counters, never Discover's.
            state["network_bodies"].append(body)
            status, answer, name = _network_answer(body)
            state["network_answers"].append(name or status)
            return self._status(status, answer)
        if TASTE and self.path == "/api/v1/discovery/taste":
            # Discover's own body never carries a generation-3 key.
            if isinstance(body, dict) and (any(k in body for k in GEN3_KEYS)
                                           or "network" in (body.get("limit") or {})):
                state["discover_gen3_bodies"] += 1
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
        if PLUGINS and self.path == "/api/v1/discovery/collection/destination":
            state["destination_puts"].append(body)
            destination = body.get("destination")
            if destination is None:
                state["saved_destination"] = None
                return self._json(_view())
            words = _refusal(destination)
            if words:
                return self._status(400, {"error": words})
            state["saved_destination"] = {"vpath": destination["vpath"], "base": _base(destination.get("base")),
                                          "layout": destination["layout"]}
            return self._json(_view())
        if self.path == "/api/v1/admin/directory":
            state["directories"][body.get("vpath", "?")] = body.get("directory", "?")
            return self._json({})
        if self.path == "/api/v1/admin/users":
            state["users"].append(body.get("username", "?"))
            return self._json({})
        return self._json({})

    def log_message(self, *args):
        pass


class Server(HTTPServer):
    """HTTPServer without its reverse lookup. The stock server_bind asks
    socket.getfqdn() for the host's name between bind and listen; on
    GitHub's macOS runners that lookup stalls ~35 s, and the battery
    starts seven fakes a run (performance audit #131). The name is only
    for CGI's SERVER_NAME, which nothing here reads."""

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


server = Server(("127.0.0.1", PORT), Handler)
if len(sys.argv) > 2:
    with open(sys.argv[2] + ".tmp", "w") as f:
        f.write(str(server.server_address[1]))
    os.replace(sys.argv[2] + ".tmp", sys.argv[2])
server.serve_forever()
