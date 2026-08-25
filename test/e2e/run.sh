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

command -v expect >/dev/null || { echo "SKIP: expect(1) not installed"; exit 0; }
command -v python3 >/dev/null || { echo "SKIP: python3 not installed"; exit 0; }

if [ ! -x "$BIN" ]; then
  echo "building $BIN…"
  cargo build --quiet || exit 1
fi

WORK="$(mktemp -d)"
trap 'kill $(cat "$WORK"/fake.pid 2>/dev/null) 2>/dev/null; rm -rf "$WORK"' EXIT
FAILS=0

port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }

start_fake() { # $1 = name
  local p; p="$(port)"
  python3 "$E2E/fake_mstream.py" "$p" >/dev/null 2>&1 & echo $! > "$WORK/fake.pid"
  for _ in $(seq 1 40); do
    curl -s -o /dev/null "http://127.0.0.1:$p/api/v1/ping" && break
    sleep 0.25
  done
  echo "$p"
}

stop_fake() { kill "$(cat "$WORK/fake.pid" 2>/dev/null)" 2>/dev/null; sleep 0.2; }

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
PORT="$(start_fake A)"
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
PORT="$(start_fake B)"
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg drive-de drive-de.exp "$WORK/h2" "$WORK/drive-de.out" env WIZMUSIC="$WORK/music"
stop_fake

# ── Scenario C: quit before the login, reopen, finish (seeding) ─────────
PORT="$(start_fake C)"
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
PORT="$(start_fake D)"
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
PORT="$(start_fake E)"
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
PORT="$(start_fake F)"
ARGS=(setup --server "http://127.0.0.1:$PORT")
leg scan scan.exp "$WORK/h6" "$WORK/scan.out"
if python3 "$E2E/check_scan.py" "$WORK/scan.out" "$E2E/replay.py"; then
  echo "PASS scan widget states"
else
  echo "FAIL scan widget states"; FAILS=$((FAILS+1))
fi
stop_fake

echo
if [ "$FAILS" -eq 0 ]; then echo "e2e: ALL PASS"; else echo "e2e: $FAILS FAILURE(S)"; exit 1; fi
