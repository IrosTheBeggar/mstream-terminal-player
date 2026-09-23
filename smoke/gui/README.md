# GUI smoke on the two-server rig

Drives `mstream-player gui` on a pty (pyte screen, SGR mouse clicks, keys) against
two scratch mStream servers paired over federation, and asserts the multi-server
contract's tunnel and direct-peer behaviour live (PLAN.md, T4). Needs `pyte`
(`pip install pyte`) and a debug build of the player.

## The rig

Two servers from one mStream checkout (6.28.0 or later; `@number0/iroh` in its
`node_modules`), in `$MSTREAM_RIG_DIR/{a,b}/`: **Rig A on :3040** and **Rig B on
:3041**, each with a `config.json` holding `"iroh": {"enabled": true}`,
`"federation": {"enabled": true, "serverName": "Rig A"}` (secrets self-generate),
storage dirs under the rig dir, `"updates": {"check": false}`, and a library —
`"folders": {"demo": {"root": "<a folder with mp3s that has Boukmanflow/6AM.mp3>"}}`
is what the scenarios browse. Boot each with
`node cli-boot-wrapper.js -j <config.json> > server.log 2>&1 &` and wait on
`/api/v1/ping`. Both endpoints come online in a few seconds with direct addresses,
so pairing works on one machine.

Pair them in public mode (no users: every request is an implicit admin):

```sh
TICKET=$(curl -s -X POST -H 'content-type: application/json' \
  -d '{"name":"rig-b","vpaths":["demo"]}' http://127.0.0.1:3040/api/v1/admin/federation/keys \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["ticket"])')
curl -s -X POST -H 'content-type: application/json' \
  -d "{\"ticket\":\"$TICKET\",\"name\":\"Rig A\"}" http://127.0.0.1:3041/api/v1/admin/federation/peers
curl -s http://127.0.0.1:3041/api/v1/iroh/code > $MSTREAM_RIG_DIR/qc-code.json   # B's Quick Connect code
```

Then `:3041/api/v1/ping` says `federationBrowse: true, federationDirect: true`, and
`GET :3041/api/v1/federation/peers/1/access` mints a guest ticket.

## The scenarios

Each run starts from a seed config in `$MSTREAM_RIG_DIR/gui-config/config.toml`:

```toml
default_server = "http://127.0.0.1:3041"

[player]
volume = 0.0

[[server]]
url = "http://127.0.0.1:3041"
```

- `direct` — connect to B, switch to Rig A from the header dropdown, wait for the
  peer's own tunnel (the dropdown and the room show "Rig A · direct · via …"),
  play a track from A. Prints the player's tunnel log lines and A's federation
  handshakes (`authorized (guest, key 'rig-b')`).
- `mixed` — pair B a second time over Quick Connect, queue a row from the tunnel
  server and one from Rig A, switch back to the standard server (both tunnels
  stay up), play, remove both rows by their hover `[x]`, and wait past the grace:
  `tunnel …: closing — nothing references it` in the log, `guest connection
  closed` on A, `tunnel connection closed` on B, the room's mark gone.

- `queue` — B alone: queue a folder, hover a row, play. The panel's rows are
  three cells tall — the cover (the ▀-mosaic on this pty, which negotiates no
  pixel protocol), the title over the artist over the album, the length on
  the last line where the hover `[x]` lands, the playing row in colour rather
  than marked, the scrollbar down the last column. Counts the rows wearing a
  cover and checks the bar's caps off the screen.

- `offline` — B alone, with a dead server added to a copy of the seed config:
  queue a folder (playback starts), pick the dead server from the header. The
  header keeps its label and picker, the room says "{server} is offline —
  couldn't connect" with "Try again", Space pauses and resumes the queue
  while no session is up, and the picker leads back to B (multi-server
  contract, clause 13).

- `library` — the Library rooms on B (library-rooms contract): Artists as a
  list with no play verb, an artist as a wall of its albums with the Singles
  bucket, a card into its tracks with the artist as the way back, Esc up
  twice, Genres with their counts into a genre's tracks under the bar's
  verbs, Recent as a hundred track rows without a strip.
- `actions` — track actions on B (track-actions contract): a hovered Files
  row's `[⋯]` opens the sheet naming the track, a star rates it and the
  server keeps it (Song Info shows the rating back), Song Info lists the
  facts, Add to playlist through New playlist says "Added to Rig mix" and
  the Playlists room lists it, and the queue header's `clear` empties a
  queued folder.
- `stats` — play reporting (play-reporting contract): one row queued and
  played past thirty seconds, then skipped; the `[stats]` log line says the
  play was posted, and Last played and Most played list the track.
- `dj` — Auto DJ on the rig (auto-dj contract, slice A5): one row queued
  with `a`, the nav's `Auto DJ` row under TOOLS, the room off with the probe's sonic
  reason (the rig has no discovery data), Start — the note names the server,
  a batch of four badged rows lands (songs per fetch 4), Preview lists three
  picks under its row without queueing, a click on the songs-per-fetch bar's
  first cell sets "1 song", the Server row's picker moves the DJ to Rig A (the
  peer through B) and the next turn is picked from there, Stop leaves the
  rows and their badges. Prints the run's `[dj]` log lines at the end — none
  on a clean run: the DJ's log speaks only for a dropped key, a learned
  rejection, a degraded constraint, a deferred pick or a failed fetch.

```sh
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py direct
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py mixed
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py dj
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py library
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py actions
```

The worker-level leg of the same rig is the ignored test in `src/tui/worker.rs`:
`MSTREAM_RIG_PARENT=http://127.0.0.1:3041 cargo test -- --ignored rig`.
