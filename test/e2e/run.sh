#!/usr/bin/env bash
# The wizard's e2e battery, fully self-contained: every leg runs against
# the stateful fake server (test/e2e/fake_mstream.py) — no Node, no real
# mStream, no audio files. Requires expect(1) and python3.
#
#   BIN=path/to/mstream-player bash test/e2e/run.sh
#   bash test/e2e/run.sh K W           # only the scenarios named
#   E2E_JOBS=1 bash test/e2e/run.sh    # one scenario at a time
#
# Without BIN it builds and uses target/debug/mstream-player. Legs are
# grouped into scenarios; each scenario gets a fresh fake (fresh state, on
# a port of its own) and a fresh HOME, so the scenarios run side by side,
# E2E_JOBS at a time (8 unless set), longest first — the scheduler at the
# end. Their legs mostly wait (the drains, the fake's wall-clock jobs, the
# wizard's scan poll), so the battery takes about as long as its longest
# scenario rather than all of them end to end. Each scenario's lines print
# whole when it ends. The frame oracle is replay.py: in-place cell rewrites
# make raw-stream greps lie, so anything mid-run is asserted by replaying
# the captured stream (check_scan.py slices it through time).
set -u
cd "$(dirname "$0")/../.."
E2E=test/e2e
BIN="${BIN:-target/debug/mstream-player}"

# The scenarios, by name (each is a function below); a name asked for
# that is none of them stops the run before anything is built.
ALL="A B C D E F G H T K K2 W M KM I J"
for s in "$@"; do
  case " $ALL " in *" $s "*) ;; *) echo "no scenario $s (the battery has $ALL)"; exit 2 ;; esac
done

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
# What is still running if the battery stops early: the scenarios'
# subshells (and their legs) and their fakes. Each file is removed once
# its process is done with, so a pid the OS has handed on is never hit.
cleanup() {
  local f
  for f in "$WORK"/*.job "$WORK"/fake-*.pid; do
    [ -f "$f" ] || continue
    pkill -P "$(cat "$f")" 2>/dev/null
    kill "$(cat "$f")" 2>/dev/null
  done
  rm -rf "$WORK"
}
trap cleanup EXIT
FAILS=0

# Starts scenario $1's fake — its switches from the caller's env
# (`FAKE_AUTH=1 start_fake G`) — and sets PORT once it answers; a fake
# that never does is a failure. The fake binds a port the OS picks and
# writes it down once it listens, so scenarios side by side are never
# handed the same one. Every probe is still bounded — macOS drops a SYN to
# a port that is bound but not listening yet instead of refusing it, and
# an unbounded curl then hangs ~8 s a try, forty of them minutes
# (performance audit #131, which also took the ~35 s bind-to-listen stall
# out of the fake).
start_fake() { # $1 = name
  local portfile="$WORK/fake-$1.port" p
  python3 "$E2E/fake_mstream.py" 0 "$portfile" >/dev/null 2>&1 & echo $! > "$WORK/fake-$1.pid"
  for _ in $(seq 1 40); do
    if [ -s "$portfile" ]; then
      p="$(cat "$portfile")"
      curl -s --connect-timeout 1 -m 2 -o /dev/null "http://127.0.0.1:$p/api/v1/ping" && { PORT="$p"; return 0; }
    fi
    sleep 0.25
  done
  echo "FAIL fake $1 never answered"; FAILS=$((FAILS+1))
  return 1
}

stop_fake() { # $1 = name
  [ -f "$WORK/fake-$1.pid" ] || return 0
  kill "$(cat "$WORK/fake-$1.pid")" 2>/dev/null
  rm -f "$WORK/fake-$1.pid"
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

# The wizard legs' music folder, shared by A to E.
mkdir -p "$WORK/music"

# ── Scenario A: the full English walk, then the QR pages on its session ──
scenario_A() {
start_fake A || return
ARGS=(setup --server "http://127.0.0.1:$PORT")
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
}

# ── Scenario B: the same walk auf Deutsch ────────────────────────────────
scenario_B() {
start_fake B || return
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg drive-de drive-de.exp "$WORK/h2" "$WORK/drive-de.out" env WIZMUSIC="$WORK/music"
}

# ── Scenario C: quit before the login, reopen, finish (seeding) ─────────
scenario_C() {
start_fake C || return
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
}

# ── Scenario D: public mode still shows the code ─────────────────────────
scenario_D() {
start_fake D || return
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
}

# ── Scenario E: click-to-rename on a fresh row ───────────────────────────
scenario_E() {
start_fake E || return
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
}

# ── Scenario F: the scan widget's whole choreography ─────────────────────
scenario_F() {
start_fake F || return
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg scan scan.exp "$WORK/h6" "$WORK/scan.out"
if python3 "$E2E/check_scan.py" "$WORK/scan.out" "$E2E/replay.py"; then
  echo "PASS scan widget states"
else
  echo "FAIL scan widget states"; FAILS=$((FAILS+1))
fi
}

# ── Scenario G: the admin hub signs in when the server asks it to ────────
# A fake with the auth wall up and one account: the tokenless pre-flight
# ping is a 401, the sign-in page comes first, the room after; the second
# run on the same HOME rides the kept session straight into the room.
scenario_G() {
FAKE_AUTH=1 start_fake G || return
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
}

# ── Scenario H: the GUI player with no server — the demo seat, the Now Playing keys
# No fake at all: the review's key-routing bug (a digit on the Now Playing
# screen drove the nav) showed with no server, and nothing automated
# launched the GUI until this leg.
scenario_H() {
ARGS=(gui)
leg gui-now gui-now.exp "$WORK/h8" "$WORK/gui-now.out"
# The Admin tab on the same seat: M opens it on its no-session sentence,
# Esc in the hallway comes back to the Library, q quits.
leg gui-admin gui-admin.exp "$WORK/h10" "$WORK/gui-admin.out"
# The Discover room's gate on the same seat: with no server the row is
# absent, G opens nothing and the Library keeps the keys.
leg gui-discover gui-discover.exp "$WORK/h11" "$WORK/gui-discover.out"
}

# ── Scenario T: the GUI's Discover room against the fake's taste route ───
# The gate open (the ping advertises discoveryTaste): the nav grows its
# row, G opens the room on the answer's header and rows, and Now Playing's
# per-song tab reads Similar (docs/ux-contracts/discovery-page.md). The
# fake counts the asks: the room must have asked the route itself — and
# its peers half, whose Connected servers tile fetches its cover from the
# peer through the federation art proxy by the row's `albumArt`.
scenario_T() {
FAKE_TASTE=1 start_fake T || return
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-room gui-discover-room.exp "$WORK/h12" "$WORK/gui-discover-room.out"
# The row itself, replayed (law 1): the ping's frame — the one that first
# paints "connected to" — draws Discover in the nav column, before G.
assert "the nav grew its Discover row with the ping" "
import subprocess
raw=open('$WORK/gui-discover-room.out','rb').read()
end=raw.find(b'\x1b[?2026l', raw.find(b'connected to'))
assert end > 0, 'no ping frame'
open('$WORK/ping.cut','wb').write(raw[:end])
out=subprocess.run(['python3','$E2E/replay.py','$WORK/ping.cut','34','120'],capture_output=True,text=True).stdout
assert any(line[:15].strip().endswith('Discover') for line in out.splitlines()), out
"
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
}

# ── Scenario K: Keep a copy, end to end, against the fake's plug-ins ─────
# The five-part gate open (the ping's `discoveryPlugins` and `noUpload:
# false`, `federation-copy` listed with jobs allowed, a throwaway `copies`
# destination): `c` on the Connected servers tab's lit tile opens Keep a
# copy, Enter alone presses its one primary, Add album, and starts the copy
# with the row unchanged, the tile and the verbs line say so while the
# room's poll walks it to done, `C` lists it in the Copies modal
# (docs/ux-contracts/discovery-page.md, clauses 100-141).
# check_keep.py replays the stream frame by frame for those states.
scenario_K() {
FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake K || return
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
}

# ── Scenario K2: Keep a copy against a server from before the pin ────────
# The same leg against FAKE_NO_PIN=1: mStream before 2026-10-21, whose jobs
# route's strict body refuses `destination` (Joi's 400). The start is asked
# once more without the key and runs unpinned, as such a server runs every
# copy (discovery-page clause 145) — the same states, the same Copies.
scenario_K2() {
FAKE_TASTE=1 FAKE_PLUGINS=1 FAKE_NO_PIN=1 start_fake K2 || return
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
}

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
scenario_W() {
FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake W || return
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
assert s['destination_gets'] == 3, s['destination_gets']  # the gate, the open, the 409's read again — never one a frame
assert len(s['job_starts']) == 1, s['job_starts']
assert s['job_starts'][0]['destination'] == {'vpath': 'copies', 'base': '', 'layout': 'from-peers/{{ALBUMARTIST}}/{{ALBUM}}'}, s['job_starts'][0]
assert s['job_answers'] == ['destination_changed'] and s['jobs'] == [], (s['job_answers'], s['jobs'])
"
}

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
scenario_M() {
FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake M || return
ARGS=(gui --server "http://127.0.0.1:$PORT" --token fake-token)
leg gui-discover-echo gui-discover-echo.exp "$WORK/h14" "$WORK/gui-discover-echo.out"
if python3 "$E2E/check_echo.py" "$WORK/gui-discover-echo.out" "$E2E/replay.py" 30 100; then
  echo "PASS the release echo: the tile's list and the sheet stay, nothing pressed"
else
  echo "FAIL the release echo: the tile's list and the sheet stay, nothing pressed"; FAILS=$((FAILS+1))
fi
}

# ── Scenario KM: Keep a copy by the pointer, in Apple Terminal's dialect ──
# The same dialect as M: the tile's list opens Keep a copy, and every
# control the window draws is clicked, each press pair followed by its
# echo — the choices, `d change`, the primary, "x cancel copy", Add again,
# "Esc close" — while the room beneath stays inert: no tooltip of its
# ripens over the band or the footer, its row under the window lights
# nothing (docs/ui-kit.md, Modal; docs/ux-contracts/discovery-page.md,
# clauses 106–110). check_keep_mouse.py replays the stream at the leg's
# marks; the fake says each click acted once.
scenario_KM() {
FAKE_TASTE=1 FAKE_PLUGINS=1 start_fake KM || return
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
}

# ── Scenario I: the MP3 player's flash page, against the scripted board ──
# No fake mStream at all: the page talks to a board, and the board is the
# fake behind MSTREAM_DEVICE_FAKE. The image is a merged one in miniature —
# the two image headers and the app description the page reads the version
# from (src/device/firmware.rs).
scenario_I() {
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
}

# ── Scenario J: the SD-card transfer's routes, and its hidden command ────
# The fake's made-up library (FAKE_LIBRARY=1) answers the routes a card sync
# reads. The sync itself needs a card in a reader, which CI has none of, so
# `device card-sync` is held only to refusing a volume that isn't one: it
# lists the volumes and opens nothing.
scenario_J() {
FAKE_LIBRARY=1 start_fake J || return
assert "the fake serves a card sync's routes" "
import json, urllib.request, urllib.error
base = 'http://127.0.0.1:$PORT'
def ask(path, body=None, headers={}):
    data = json.dumps(body).encode() if body is not None else None
    kind = {'Content-Type': 'application/json'} if data else {}
    r = urllib.request.Request(base + path, data=data, headers=dict(headers, **kind))
    try:
        with urllib.request.urlopen(r) as resp:
            return resp.status, resp.headers, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers, e.read()
st, h, b = ask('/api/v1/sync/manifest', {'limit': 5000})
page = json.loads(b)
assert st == 200 and len(page['entries']) == 4, page
st, _, _ = ask('/api/v1/sync/manifest', {'limit': 5000}, {'If-None-Match': '\"%s\"' % page['revision']})
assert st == 304, st
st, h, b = ask('/media/Rock/Artist%20A/Album%20B/01.mp3', headers={'Range': 'bytes=0-3'})
assert st == 206 and b == bytes([0xff, 0xfb, 0x90, 0x40]), (st, b)
st, h, b = ask('/transcode/Rock/Artist%20C/Album%20D/01.wav?codec=opus&bitrate=192k')
assert h['Content-Type'] == 'audio/ogg' and b[:4] == b'OggS', h
"
stop_fake J
assert "device card-sync refuses a volume that isn't one" "
import os, subprocess
env = dict(os.environ, HOME='$WORK/h18')
out = subprocess.run(['$BIN', 'device', 'card-sync', '$WORK/no-such-card', '--dry-run'], capture_output=True, text=True, env=env)
assert out.returncode == 2 and 'volume' in out.stderr, (out.returncode, out.stdout, out.stderr)
"
}

# ── The scheduler ────────────────────────────────────────────────────────
# Each scenario above runs in a subshell of its own, its lines kept in
# WORK/<name>.log; run_one stops its fake however it ended and leaves
# "<failures> <seconds>" in WORK/<name>.done. They start longest first, so
# no long one is left running alone at the end, and print as they end.
# Measured side by side (2026-11-10, Linux, 8 at a time): A 35 s, KM 29,
# W 27, K and K2 23, F 21, C 20, M 19, H 16, B 13, E 11, D and I 10, G 9,
# T 8, J 1 — 39 s in all, against 289 s one after another (J measured
# 2026-11-11, when it joined the battery).
LONGEST_FIRST="A KM W K K2 F C M H B E D I G T J"
JOBS="${E2E_JOBS:-8}"
[ "$JOBS" -ge 1 ] 2>/dev/null || JOBS=1

run_one() { # $1 = name
  FAILS=0
  local t0=$SECONDS
  "scenario_$1"
  stop_fake "$1"
  echo "$FAILS $((SECONDS - t0))" > "$WORK/$1.done.tmp" && mv "$WORK/$1.done.tmp" "$WORK/$1.done"
}

count() { echo $#; }

# Prints and counts each running scenario that has ended.
reap() {
  local s still="" fails secs
  for s in $RUNNING; do
    if [ -f "$WORK/$s.done" ]; then
      read -r fails secs < "$WORK/$s.done"
    elif kill -0 "$(cat "$WORK/$s.job")" 2>/dev/null; then
      still="$still $s"
      continue
    elif [ -f "$WORK/$s.done" ]; then
      # It wrote its summary and ended between the two looks above.
      read -r fails secs < "$WORK/$s.done"
    else
      fails=1 secs="?"
      echo "FAIL scenario $s ended before its summary" >> "$WORK/$s.log"
    fi
    wait "$(cat "$WORK/$s.job")" 2>/dev/null
    rm -f "$WORK/$s.job"
    echo "── Scenario $s (${secs}s)"
    cat "$WORK/$s.log"
    FAILS=$((FAILS + fails))
    DONE=$((DONE + 1))
  done
  RUNNING="$still"
}

WANT=" ${*:-$ALL} "
RUNNING=""
DONE=0
T0=$SECONDS
for s in $LONGEST_FIRST; do
  case "$WANT" in *" $s "*) ;; *) continue ;; esac
  while [ "$(count $RUNNING)" -ge "$JOBS" ]; do sleep 0.2; reap; done
  ( run_one "$s" ) > "$WORK/$s.log" 2>&1 &
  echo $! > "$WORK/$s.job"
  RUNNING="$RUNNING $s"
done
while [ -n "$RUNNING" ]; do sleep 0.2; reap; done

echo
if [ "$FAILS" -eq 0 ]; then echo "e2e: ALL PASS ($DONE scenarios, $((SECONDS - T0))s)"; else echo "e2e: $FAILS FAILURE(S)"; exit 1; fi
