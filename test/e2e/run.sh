#!/usr/bin/env bash
# The wizard's e2e battery, fully self-contained: every leg runs against
# the stateful fake server (test/e2e/fake_mstream.py) — no Node, no real
# mStream, no audio files. Requires expect(1) and python3.
#
#   BIN=path/to/mstream-player bash test/e2e/run.sh
#
# Without BIN it builds and uses target/debug/mstream-player. Legs are
# grouped into scenarios; each scenario gets a fresh fake (fresh state)
# and a fresh HOME. The frame oracle is replay.py: in-place cell rewrites
# make raw-stream greps lie, so anything mid-run is asserted by replaying
# the captured stream (check_scan.py slices it through time).
set -u
cd "$(dirname "$0")/../.."
E2E=test/e2e
BIN="${BIN:-target/debug/mstream-player}"

# README law 3, checked before anything runs: a braced pattern list is a
# list only as `expect`'s sole argument with a newline after the brace.
# On one line, or after -timeout/-i, it is ONE glob that never matches —
# the gate waits out its timeout and cannot fail. Nineteen did, for 390 s
# a run (performance audit #129). Comments are skipped; -brace is fine.
one_line='^[^#]*(^|[^[:alnum:]_])expect[[:space:]]+\{[[:space:]]*[^[:space:]]'
after_flag='^[^#]*(^|[^[:alnum:]_])expect([[:space:]]+-(timeout|i)[[:space:]]+[^[:space:]]+)+[[:space:]]+\{'
if grep -nE -e "$one_line" -e "$after_flag" "$E2E"/legs/*.exp; then
  echo "FAIL a braced expect above is one glob, not a pattern list: put \`expect {\` alone on its line (README law 3)"
  exit 1
fi

command -v expect >/dev/null || { echo "SKIP: expect(1) not installed"; exit 0; }
command -v python3 >/dev/null || { echo "SKIP: python3 not installed"; exit 0; }

if [ ! -x "$BIN" ]; then
  echo "building ${BIN}..."
  cargo build --quiet || exit 1
fi

WORK="$(mktemp -d)"
trap 'kill $(cat "$WORK"/fake.pid 2>/dev/null) 2>/dev/null; rm -rf "$WORK"' EXIT
FAILS=0

port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }

# Prints the port once the fake answers, or fails: callers run it in
# $(...), where an exit would only end the subshell, so each checks the
# status. Every probe is bounded — macOS drops a SYN to a port that is
# bound but not listening yet instead of refusing it, and an unbounded
# curl then hangs ~8 s a try, forty of them minutes (performance audit
# #131, which also took the ~35 s bind-to-listen stall out of the fake).
# Each fake's PID joins the file, so a scenario's pair (an own server and
# the connected server behind its proxy) stops together.
start_fake() { # $1 = name
  local p; p="$(port)"
  python3 "$E2E/fake_mstream.py" "$p" >/dev/null 2>&1 & echo $! >> "$WORK/fake.pid"
  for _ in $(seq 1 40); do
    curl -s --connect-timeout 1 -m 2 -o /dev/null "http://127.0.0.1:$p/api/v1/ping" && { echo "$p"; return 0; }
    sleep 0.25
  done
  echo "FAIL fake $1 never answered on :$p" >&2
  return 1
}

stop_fake() { kill $(cat "$WORK/fake.pid" 2>/dev/null) 2>/dev/null; rm -f "$WORK/fake.pid"; sleep 0.2; }

# A HOME whose player opens on Stage Peer B (the connected server the fake
# own server on port $2 proxies), at its peerlib/Boukmanflow: the peer as
# the most recent server, reached through its parent — the parent's entry
# and token beside it, as a switch onto the peer leaves them.
seed_peer_home() { # $1 = HOME, $2 = own server's port
  mkdir -p "$1/player"
  cat > "$1/player/config.toml" <<TOML
[[server]]
url = "mstream+peer://2@http://127.0.0.1:$2"
last_path = "/peerlib/Boukmanflow"

[server.peer]
parent = "http://127.0.0.1:$2"
id = 2
name = "Stage Peer B"

[[server]]
url = "http://127.0.0.1:$2"
username = "e2e"
TOML
  cat > "$1/player/credentials.toml" <<TOML
[[token]]
server = "http://127.0.0.1:$2"
token = "fake-token"
TOML
}

fake_state() { curl -s "http://127.0.0.1:$1/_state"; }

leg() { # $1 name, $2 exp, $3 home, $4 out, then argv…
  local name="$1" exp="$2" home="$3" out="$4"; shift 4
  mkdir -p "$home"
  if env WIZHOME="$home" "$@" expect "$E2E/legs/$exp" "$BIN" "${ARGS[@]}" > "$out" 2>&1; then
    echo "PASS $name"
  else
    echo "FAIL $name (exit $?) — tail:"; tail -3 "$out"; FAILS=$((FAILS+1))
  fi
}

assert() { # $1 label, $2 python expr over WORK/PORT env
  if python3 -c "$2"; then echo "PASS $1"; else echo "FAIL $1"; FAILS=$((FAILS+1)); fi
}

# ── Scenario A: the full English walk, then the QR pages on its session ──
PORT="$(start_fake A)" || exit 1
ARGS=(setup --server "http://127.0.0.1:$PORT")
mkdir -p "$WORK/music"
leg drive-en drive-en.exp "$WORK/h1" "$WORK/drive-en.out" env WIZMUSIC="$WORK/music"
assert "server state: user + folder" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert s['users'], 'no user created'
assert s['directories'], 'no folder committed'
"
ARGS=(qr --server "http://127.0.0.1:$PORT")
leg qr-fallback qr-fallback.exp "$WORK/h1" "$WORK/qr-fallback.out" env MSTREAM_NO_GRAPHICS=1
leg qr-short qr-short.exp "$WORK/h1" "$WORK/qr-short.out" env MSTREAM_NO_GRAPHICS=1
leg qr-pixel qr-pixel.exp "$WORK/h1" "$WORK/qr-pixel.out" env MSTREAM_GRAPHICS=iterm2
assert "pixel path emits the image escape" "
raw=open('$WORK/qr-pixel.out','rb').read()
assert b'1337;File=' in raw, 'no iTerm2 image escape in the stream'
"
leg qr-buttons qr-buttons.exp "$WORK/h1" "$WORK/qr-buttons.out" env MSTREAM_NO_GRAPHICS=1 MSTREAM_NO_OPEN=1
stop_fake

# ── Scenario B: the same walk auf Deutsch ────────────────────────────────
PORT="$(start_fake B)" || exit 1
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg drive-de drive-de.exp "$WORK/h2" "$WORK/drive-de.out" env WIZMUSIC="$WORK/music"
stop_fake

# ── Scenario C: quit before the login, reopen, finish (seeding) ─────────
PORT="$(start_fake C)" || exit 1
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg reopen-phase1 reopen.exp "$WORK/h3" "$WORK/reopen1.out" env WIZMUSIC="$WORK/music"
mkdir -p "$WORK/music2"
leg reopen-phase2 drive-en.exp "$WORK/h3" "$WORK/reopen2.out" env WIZMUSIC="$WORK/music2"
assert "reopen kept both folders and made the user" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert len(s['directories'])==2, f'directories: {s[\"directories\"]}'
assert s['users'], 'no user after reopen'
"
stop_fake

# ── Scenario D: public mode still shows the code ─────────────────────────
PORT="$(start_fake D)" || exit 1
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg skip-public skip.exp "$WORK/h4" "$WORK/skip.out" env WIZMUSIC="$WORK/music"
assert "public-mode Done renders the half-block code" "
import subprocess
raw=open('$WORK/skip.out','rb').read()
cut=raw[:raw.rfind(b'\x1b[?1049l')] if b'\x1b[?1049l' in raw else raw
open('$WORK/skip.cut','wb').write(cut)
out=subprocess.run(['python3','$E2E/replay.py','$WORK/skip.cut','56','100'],capture_output=True,text=True).stdout
assert out.count('█')>10, 'no code on the public-mode Done page'
"
stop_fake

# ── Scenario E: click-to-rename on a fresh row ───────────────────────────
PORT="$(start_fake E)" || exit 1
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg rename rename.exp "$WORK/h5" "$WORK/rename.out" env WIZMUSIC="$WORK/music"
assert "the mouse rename landed" "
import subprocess
raw=open('$WORK/rename.out','rb').read()
cut=raw[:raw.rfind(b'\x1b[?1049l')] if b'\x1b[?1049l' in raw else raw
open('$WORK/rename.cut','wb').write(cut)
out=subprocess.run(['python3','$E2E/replay.py','$WORK/rename.cut','40','100'],capture_output=True,text=True).stdout
assert 'mediax' in out, 'renamed row not found'
"
stop_fake

# ── Scenario F: the scan widget's whole choreography ─────────────────────
PORT="$(start_fake F)" || exit 1
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg scan scan.exp "$WORK/h6" "$WORK/scan.out"
if python3 "$E2E/check_scan.py" "$WORK/scan.out" "$E2E/replay.py"; then
  echo "PASS scan widget states"
else
  echo "FAIL scan widget states"; FAILS=$((FAILS+1))
fi
stop_fake

# ── Scenario G: the admin hub signs in when the server asks it to ────────
# A fake with the auth wall up and one account: the tokenless pre-flight
# ping is a 401, the sign-in page comes first, the room after; the second
# run on the same HOME rides the kept session straight into the room.
PORT="$(FAKE_AUTH=1 start_fake G)" || exit 1
curl -s -o /dev/null -X PUT -H 'Content-Type: application/json'   -d '{"username":"alice","password":"hunter2","admin":true}'   "http://127.0.0.1:$PORT/api/v1/admin/users"
ARGS=(admin --server "http://127.0.0.1:$PORT")
leg admin-signin admin-signin.exp "$WORK/h7" "$WORK/admin-signin.out"
assert "the sign-in kept its session" "
import pathlib
found=list(pathlib.Path('$WORK/h7').rglob('credentials.toml'))
assert found, 'no credentials.toml under the HOME'
assert 'fake-token' in found[0].read_text(), 'the issued token is not stored'
cfg=list(pathlib.Path('$WORK/h7').rglob('config.toml'))
assert cfg and 'alice' in cfg[0].read_text(), 'the server entry does not name the account'
"
leg admin-room admin-room.exp "$WORK/h7" "$WORK/admin-room.out"
stop_fake

# ── Scenario H: the GUI player with no server — the demo seat, the Now Playing keys
# No fake at all: the review's key-routing bug (a digit on the Now Playing
# screen drove the nav) showed with no server, and nothing automated
# launched the GUI until this leg.
ARGS=(gui)
leg gui-now gui-now.exp "$WORK/h8" "$WORK/gui-now.out"
# The Admin tab on the same seat: M opens it on its no-session sentence,
# Esc in the hallway comes back to the Library, q quits.
leg gui-admin gui-admin.exp "$WORK/h10" "$WORK/gui-admin.out"
# The Discover room's gate on the same seat: with no server the row is
# absent, G opens nothing and the Library keeps the keys.
leg gui-discover gui-discover.exp "$WORK/h11" "$WORK/gui-discover.out"

# ── Scenario T: the GUI's Discover room against the fake's taste route ───
# The gate open (the ping advertises discoveryTaste): the nav grows its
# row, G opens the room on the answer's header and rows, and Now Playing's
# per-song tab reads Similar (docs/ux-contracts/discovery-page.md). The
# fake counts the asks: the room must have asked the route itself — and
# its peers half, whose Connected servers tile fetches its cover from the
# peer through the federation art proxy by the row's `albumArt`.
PORT="$(FAKE_TASTE=1 start_fake T)"
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-room gui-discover-room.exp "$WORK/h12" "$WORK/gui-discover-room.out"
assert "the room asked the taste route" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert s['taste_asks'] >= 1, s
"
assert "the asks were stateless: the window, no prefs, no remember/filter" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
b=s['taste_body'] or {}
assert s['prefs_calls'] == 0, s
assert b.get('window') == 30, b
assert not any(k in b for k in ('remember','filter','staleDays')), b
"
assert "the strip fetched its album's cover" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert s['art_gets'] >= 1, s
"
assert "the peers half was asked and its tile fetched the peer's cover through the art proxy" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert s['peer_asks'] >= 1, s
assert s['peer_art_gets'] == 1, s  # claimed once
assert all(p.split('?')[0] == '/api/v1/federation/peers/7/art/attic-lanterns.png' for p in s['peer_art_paths']), s
"
assert "the peer's cover was asked as its compressed rendition (compress=l)" "
import json,urllib.request,urllib.parse
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert s['peer_art_paths'], s
assert all(urllib.parse.parse_qs(urllib.parse.urlsplit(p).query).get('compress') == ['l'] for p in s['peer_art_paths']), s
"
stop_fake

# ── Scenario K: Keep a copy, end to end, against the fake's plug-ins ─────
# The five-part gate open (the ping's `discoveryPlugins` and `noUpload:
# false`, `federation-copy` listed with jobs allowed, a throwaway `copies`
# destination): `c` on the Connected servers tab's lit tile opens Keep a
# copy, Enter alone presses its one primary, Add album, and starts the copy
# with the row unchanged, the tile and the verbs line say so while the
# room's poll walks it to done, `C` lists it in the Copies modal
# (docs/ux-contracts/discovery-page.md, clauses 100-141).
# check_keep.py replays the stream frame by frame for those states.
PORT="$(FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake K)"
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-keep gui-discover-keep.exp "$WORK/h13" "$WORK/gui-discover-keep.out"
if python3 "$E2E/check_keep.py" "$WORK/gui-discover-keep.out" "$E2E/replay.py" 30 100; then
  echo "PASS keep a copy: modal, live marks, kept, Copies"
else
  echo "FAIL keep a copy: modal, live marks, kept, Copies"; FAILS=$((FAILS+1))
fi
assert "the album copy sent the row unchanged, scope album, to the user's own server" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert len(s['job_starts']) == 1, s['job_starts']
start = s['job_starts'][0]
assert start['scope'] == 'album', start
assert start['destination'] == {'vpath': 'copies', 'base': '', 'layout': '{{ALBUMARTIST}}/{{ALBUM}}'}, start  # the place "Files go to" drew
assert start['recommendation'] == {'artist': 'Attic Band', 'title': 'Ember', 'album': 'Lanterns', 'duration': 199,
    'similarity': 0.81, 'filepath': 'Music/ember.mp3', 'albumArt': 'attic-lanterns.png',
    'peer': {'id': 7, 'name': 'Attic'}, 'source': 'federation'}, start
assert [j['state'] for j in s['jobs']] == ['done'], s['jobs']
"
assert "the rows were looked up in chunks of at most 100, again once the copy ended" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert len(s['lookup_sizes']) >= 2 and all(0 < n <= 100 for n in s['lookup_sizes']), s['lookup_sizes']
assert s['cancels'] == 0 and s['clears'] == 0, s
"
stop_fake

# ── Scenario K2: Keep a copy against a server from before the pin ────────
# The same leg against FAKE_NO_PIN=1: mStream before 2026-10-21, whose jobs
# route's strict body refuses `destination` (Joi's 400). The start is asked
# once more without the key and runs unpinned, as such a server runs every
# copy (discovery-page clause 145) — the same states, the same Copies.
PORT="$(FAKE_TASTE=1 FAKE_PLUGINS=1 FAKE_NO_PIN=1 start_fake K2)"
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-keep-no-pin gui-discover-keep.exp "$WORK/h16" "$WORK/gui-discover-keep-no-pin.out"
if python3 "$E2E/check_keep.py" "$WORK/gui-discover-keep-no-pin.out" "$E2E/replay.py" 30 100; then
  echo "PASS keep a copy on a server from before the pin"
else
  echo "FAIL keep a copy on a server from before the pin"; FAILS=$((FAILS+1))
fi
assert "the start was asked again without the destination, and ran" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert [('destination' in b) for b in s['job_starts']] == [True, False], s['job_starts']
assert s['job_answers'] == [400, 202], s['job_answers']
assert s['job_starts'][1]['scope'] == 'album' and s['job_starts'][1]['recommendation'] == s['job_starts'][0]['recommendation'], s['job_starts']
assert [j['state'] for j in s['jobs']] == ['done'], s['jobs']
"
stop_fake

# ── Scenario W: Where copies land, the start's pinned destination ────────
# The same gate as K, on a place seeded with a base folder (older data: the
# leg's /_move_destination before the room opens). `c` on the lit tile
# opens Keep a copy on it; `d` opens Where copies land folded into Custom
# ("From friends/{{ALBUMARTIST}}/{{ALBUM}}", nothing dirty: Enter on the
# libraries saves nothing) — trimmed since 2026-10-24: the title on its
# border, "Files go to …" its first row, no header, labels, templates or
# help — "Start from an existing folder…" lists the library's own folders
# (the file explorer; the seeded folder is not among them: the first file
# creates it), a folder picked there starts Custom's layout and Save keeps
# it (PUT, the layout with base '' — the one PUT); Keep a copy's place line
# reads the new place. The destination then moves
# behind the client's back (the fake's /_move_destination, as another tab
# would), and Enter presses Add album — which sends the place Keep a copy
# showed — answered 409 destination_changed: nothing starts, the place line
# redraws from the answer (docs/ux-contracts/discovery-page.md, clauses
# 105, 142-148).
# check_dest.py replays the stream frame by frame for those states.
PORT="$(FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake W)"
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-dest gui-discover-dest.exp "$WORK/h15" "$WORK/gui-discover-dest.out" env FAKE_URL="http://127.0.0.1:$PORT"
if python3 "$E2E/check_dest.py" "$WORK/gui-discover-dest.out" "$E2E/replay.py" 30 100; then
  echo "PASS where copies land: the seeded place, the picker folded, the list, a folder picked, saved, moved"
else
  echo "FAIL where copies land: the seeded place, the picker folded, the list, a folder picked, saved, moved"; FAILS=$((FAILS+1))
fi
assert "the picker saved the layout started in the picked folder with no base folder, once; the start showed it; the moved destination started nothing" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert s['destination_puts'] == [{'destination': {'vpath': 'copies', 'base': '', 'layout': 'from-peers/{{ALBUMARTIST}}/{{ALBUM}}'}}], s['destination_puts']  # one PUT: the fold on open was no edit (Enter on the libraries sent nothing), the seeded base folder went into the layout, never sent as a base
assert s['folder_lists'][-2:] == ['/copies', '/copies/from-peers'] and s['folder_lists'].count('/copies') == 1, s['folder_lists']  # after the Files pane's own: the root on opening the list, the picked folder, each once
assert s['destination_gets'] == 4, s['destination_gets']  # the gate on Connected, the room's visit, the open, the 409's read again — never one a frame
assert len(s['job_starts']) == 1, s['job_starts']
assert s['job_starts'][0]['destination'] == {'vpath': 'copies', 'base': '', 'layout': 'from-peers/{{ALBUMARTIST}}/{{ALBUM}}'}, s['job_starts'][0]
assert s['job_answers'] == ['destination_changed'] and s['jobs'] == [], (s['job_answers'], s['jobs'])
"
stop_fake

# ── Scenario M: Apple Terminal's mouse dialect, the release echo ─────────
# The GUI spawned with TERM_PROGRAM=Apple_Terminal and every click sent as
# that terminal sends one: the press an instant pair, the physical release
# the same pair again (docs/ui-kit.md, behavioral rules). A connected
# server's tile's `[⋯]` opens its list and the echo, landing on the list's
# catcher, leaves it open; a row's `[⋯]` opens the track-actions sheet and
# the echo, landing outside it, leaves it open and presses nothing — the
# queue empty, nothing playing (docs/ux-contracts/discovery-page.md,
# clauses 48, 102 and 103). check_echo.py replays the stream between the
# leg's marks.
PORT="$(FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake M)"
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-echo gui-discover-echo.exp "$WORK/h14" "$WORK/gui-discover-echo.out"
if python3 "$E2E/check_echo.py" "$WORK/gui-discover-echo.out" "$E2E/replay.py" 30 100; then
  echo "PASS the release echo: the tile's list and the sheet stay, nothing pressed"
else
  echo "FAIL the release echo: the tile's list and the sheet stay, nothing pressed"; FAILS=$((FAILS+1))
fi
stop_fake

# ── Scenario KM: Keep a copy by the pointer, in Apple Terminal's dialect ──
# The same dialect as M: the tile's list opens Keep a copy, and every
# control the window draws is clicked, each press pair followed by its
# echo — the choices, `d change`, the primary, "x cancel copy", Add again,
# "Esc close" — while the room beneath stays inert: no tooltip of its
# ripens over the band or the footer, its row under the window lights
# nothing (docs/ui-kit.md, Modal; docs/ux-contracts/discovery-page.md,
# clauses 106–110). check_keep_mouse.py replays the stream at the leg's
# marks; the fake says each click acted once.
PORT="$(FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake KM)"
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-keep-mouse gui-discover-keep-mouse.exp "$WORK/h17" "$WORK/gui-discover-keep-mouse.out"
if python3 "$E2E/check_keep_mouse.py" "$WORK/gui-discover-keep-mouse.out" "$E2E/replay.py" 30 100; then
  echo "PASS keep a copy by the pointer: every click once, its echo nothing, the room inert"
else
  echo "FAIL keep a copy by the pointer: every click once, its echo nothing, the room inert"; FAILS=$((FAILS+1))
fi
assert "the pointer's clicks sent one start, one cancel and no save" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
assert len(s['job_starts']) == 1 and s['job_starts'][0]['scope'] == 'album', s['job_starts']
assert s['cancels'] == 1, s['cancels']
assert s.get('destination_puts', []) == [], s.get('destination_puts')
assert [j['state'] for j in s['jobs']] == ['cancelled'], s['jobs']
"
stop_fake

# ── Scenario P: Keep a copy while browsing a connected server ────────────
# Two fakes: Stage Peer B (FAKE_ROLE=peer, the keep-a-copy spec's §2
# library) behind the own server's browse proxy (FAKE_PEER_URL, mStream's
# federation-browse.js: the exact allowlist, the method and the JSON body
# passed through), the own server's plug-ins as K's. The HOME opens on the
# peer at peerlib/Boukmanflow (seed_peer_home). The Files leg keeps the open
# folder by its header word, a held folder row Into a folder… through the
# chooser, another By your layout, and opens Copies outside Discover; the
# album leg keeps a held row, the queue item through its sheet, the album
# page by its word (its credit on the header line), and opens the artist's
# focus on his wall (docs/ux-contracts/discovery-page.md, clauses 150–172).
# check_peer.py replays each stream at its marks; E2E_CAPTURES=DIR keeps
# every mark's decoded frame there. Every copy lands in the fake's `copies`
# library — no file is written anywhere.
PEER="$(FAKE_ROLE=peer start_fake P-peer)" || exit 1
PORT="$(FAKE_TASTE=1 FAKE_PLUGINS=1 FAKE_PEER_URL="http://127.0.0.1:$PEER" start_fake P)" || exit 1
seed_peer_home "$WORK/h18" "$PORT"
ARGS=(gui)
leg gui-peer-files gui-peer-files.exp "$WORK/h18" "$WORK/gui-peer-files.out"
if python3 "$E2E/check_peer.py" files "$WORK/gui-peer-files.out" "$E2E/replay.py" 30 100 "${E2E_CAPTURES:-}"; then
  echo "PASS keep from a connected server's Files: the word, three landings, the chooser, Copies, kept"
else
  echo "FAIL keep from a connected server's Files: the word, three landings, the chooser, Copies, kept"; FAILS=$((FAILS+1))
fi
assert "three folder starts, each with its landing, to the own server's copies library" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
starts=s['job_starts']
assert [b.get('scope') for b in starts] == ['folder'] * 3, starts
peer={'id': 2, 'name': 'Stage Peer B'}
assert [b['recommendation'] for b in starts] == [
    {'filepath': 'peerlib/Boukmanflow', 'title': 'Boukmanflow', 'peer': peer, 'source': 'federation'},
    {'filepath': 'peerlib/Boukmanflow/Borrowed Tapes', 'title': 'Borrowed Tapes', 'peer': peer, 'source': 'federation'},
    {'filepath': 'peerlib/Boukmanflow/Harbour Lights', 'title': 'Harbour Lights', 'peer': peer, 'source': 'federation'}], starts
assert 'landing' not in starts[0], starts[0]  # As on Stage Peer B: the route's default
assert starts[1]['landing'] == {'vpath': 'copies', 'path': 'From Sam/Borrowed Tapes'}, starts[1]
assert starts[2]['landing'] == {'tags': True}, starts[2]
assert all(b['destination'] == {'vpath': 'copies', 'base': '', 'layout': '{{ALBUMARTIST}}/{{ALBUM}}'} for b in starts), starts
assert s['job_answers'] == [202] * 3 and [j['state'] for j in s['jobs']] == ['done'] * 3, (s['job_answers'], s['jobs'])
assert [j['result']['folder']['landed'] for j in s['jobs']] == ['copies/Boukmanflow', 'copies/From Sam/Borrowed Tapes', None], s['jobs']
"
assert "the peer was read through the proxy: one recursive listing a window, one tag batch, nothing off the allowlist" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
p=json.load(urllib.request.urlopen('http://127.0.0.1:$PEER/_state'))
assert s['proxy_refused'] == [], s['proxy_refused']
assert [b['directory'] for b in p['recursive_bodies']] == ['peerlib/Boukmanflow', 'peerlib/Boukmanflow/Borrowed Tapes', 'peerlib/Boukmanflow/Harbour Lights'], p['recursive_bodies']
assert len(p['batch_bodies']) == 1 and len(p['batch_bodies'][0]) == 3, p['batch_bodies']  # the first By your layout, at most 100 songs
assert s['folder_lists'] == ['/copies', '/copies/From Sam'], s['folder_lists']  # the chooser's list, asked of the own server
assert s['lookup_sizes'] and all(0 < n <= 100 for n in s['lookup_sizes']), s['lookup_sizes']
"
stop_fake
PEER="$(FAKE_ROLE=peer start_fake P-peer)" || exit 1
PORT="$(FAKE_TASTE=1 FAKE_PLUGINS=1 FAKE_PEER_URL="http://127.0.0.1:$PEER" start_fake P)" || exit 1
seed_peer_home "$WORK/h19" "$PORT"
leg gui-peer-album gui-peer-album.exp "$WORK/h19" "$WORK/gui-peer-album.out"
if python3 "$E2E/check_peer.py" album "$WORK/gui-peer-album.out" "$E2E/replay.py" 30 100 "${E2E_CAPTURES:-}"; then
  echo "PASS keep from a connected server's album page, queue item and artist's wall"
else
  echo "FAIL keep from a connected server's album page, queue item and artist's wall"; FAILS=$((FAILS+1))
fi
assert "the queue item's song, then the album with the page's credit, to the own server's copies library" "
import json,urllib.request
s=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/_state'))
starts=s['job_starts']
peer={'id': 2, 'name': 'Stage Peer B'}
assert [b.get('scope') for b in starts] == [None, 'album'], starts
# The queue item is no row of the album page on screen: no page credit
# rides with it (its window finds the album's own).
assert starts[0]['recommendation'] == {'filepath': 'peerlib/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3',
    'title': 'Late Train Verse', 'artist': 'Boukmanflow', 'album': 'Borrowed Tapes',
    'year': 2019, 'duration': 8.0, 'peer': peer, 'source': 'federation'}, starts[0]
assert starts[1]['recommendation'] == {'album': 'Borrowed Tapes', 'albumArtist': 'Boukmanflow', 'artist': 'Boukmanflow',
    'year': 2019, 'peer': peer, 'source': 'federation'}, starts[1]
assert all(b['destination'] == {'vpath': 'copies', 'base': '', 'layout': '{{ALBUMARTIST}}/{{ALBUM}}'} for b in starts), starts
assert [j['state'] for j in s['jobs']] == ['done', 'done'], s['jobs']
assert s['proxy_refused'] == [], s['proxy_refused']
assert s['lookup_sizes'] and all(0 < n <= 100 for n in s['lookup_sizes']), s['lookup_sizes']
"
stop_fake

# ── Scenario I: the MP3 player's flash page, against the scripted board ──
# No fake mStream at all: the page talks to a board, and the board is the
# fake behind MSTREAM_DEVICE_FAKE. The image is a merged one in miniature —
# the two image headers and the app description the page reads the version
# from (src/device/firmware.rs).
python3 - "$WORK/fw.bin" <<'PY'
import struct, sys
desc = bytearray(256)
struct.pack_into('<I', desc, 0, 0xABCD5432)
desc[16:22] = b'v0.5.0'
desc[48:66] = b'mstream-mp3-player'
img = bytearray(b'\xff' * 0x10200)
img[0x1000] = 0xE9
img[0x10000] = 0xE9
img[0x10020:0x10120] = desc
open(sys.argv[1], 'wb').write(img)
PY
ARGS=(device flash --firmware "$WORK/fw.bin")
leg device-flash device-flash.exp "$WORK/h9" "$WORK/device-flash.out" env MSTREAM_DEVICE_FAKE=ours:v0.4.0
assert "device list names the fake boards; --yes writes a fresh board; a held port is refused" "
import os, subprocess
env = dict(os.environ, HOME='$WORK/h9', MSTREAM_DEVICE_FAKE='two')
out = subprocess.run(['$BIN', 'device', 'list'], capture_output=True, text=True, env=env)
assert out.returncode == 0 and 'FAKE0' in out.stdout and 'FAKE1' in out.stdout, (out.returncode, out.stdout, out.stderr)
env['MSTREAM_DEVICE_FAKE'] = 'fresh'
out = subprocess.run(['$BIN', 'device', 'flash', '--yes', '--firmware', '$WORK/fw.bin'], capture_output=True, text=True, env=env)
assert out.returncode == 0, (out.returncode, out.stdout, out.stderr)
assert 'install, erase first' in out.stdout and 'v0.5.0 is on the board' in out.stdout, out.stdout
env['MSTREAM_DEVICE_FAKE'] = 'busy'
out = subprocess.run(['$BIN', 'device', 'flash', '--yes', '--firmware', '$WORK/fw.bin'], capture_output=True, text=True, env=env)
assert out.returncode == 1 and 'in use' in out.stderr, (out.returncode, out.stdout, out.stderr)
"

echo
if [ "$FAILS" -eq 0 ]; then echo "e2e: ALL PASS"; else echo "e2e: $FAILS FAILURE(S)"; exit 1; fi
