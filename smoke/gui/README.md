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

```sh
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py direct
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py mixed
```

The worker-level leg of the same rig is the ignored test in `src/tui/worker.rs`:
`MSTREAM_RIG_PARENT=http://127.0.0.1:3041 cargo test -- --ignored rig`.
