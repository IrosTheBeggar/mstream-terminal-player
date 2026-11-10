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

Keys, since 2026-09-26: a list's highlighted row is the keyboard cursor alone
(docs/ui-kit.md, "List cursor") — nothing is lit until ↓ or ↑, the FIRST press
only picks the cursor up where the pane rests it, and `a`, Enter and the queue
panel's `d` do nothing until it is up. The scenarios walk Down + `a` until the
queue's header counts a row, which absorbs both the pick-up and a cursor on `..`.

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
- `now` — the Now Playing screen on B (now-playing contract): `0` brings the
  TUI's full-screen view under the top bar — the facts column with the
  track's title, the tab strip `[1:Queue]`, the band's `position / total` —
  with prev · play · next under the cover; `▸▸` steps to the next row, a
  click on the band seeks, a click on `2:Auto-DJ` opens that tab, `0` goes
  back to the Library. The GUI's bar (its `auto-dj` frame) is absent while
  the screen is up.
- `vizwindow` — the visualizer window on B (visualizer-window contract):
  a row playing, `V` spawns the `viz-window` child and the top bar's item
  lights, a second `V` raises rather than spawns, and quitting the player
  takes the child with it. Proves the process and the pipe; the window's
  pixels are a manual look (the window is titled `mStream Visualizer — …`).
- `statspage` — the Stats screen on B (stats-screen contract): the top
  bar's third tab hosts the stats page whole under the bar, loading the
  period's totals on entry; `→` walks the page's tabs, Esc and `T` go
  back to the Library, `T` opens it again; the GUI's bar and nav are
  absent while the screen is up.
- `actions` — track actions on B (track-actions contract): a hovered Files
  row's `[⋯]` opens the sheet naming the track, a star rates it and the
  server keeps it (Song Info shows the rating back), Song Info lists the
  facts, Add to playlist through New playlist says "Added to Rig mix" and
  the Playlists room lists it, and the queue header's `clear` empties a
  queued folder.
- `stats` — play reporting (play-reporting contract): one row queued and
  played past thirty seconds, then skipped; the `[stats]` log line says the
  play was posted, and Last played and Most played list the track.
- `dj` — Auto DJ on the rig (auto-dj contract, clauses 40–58; the room
  rebuilt 2026-11-10): the nav's `Auto DJ` row under TOOLS, the room off — one
  row a setting, `Discovery off` and `No mood data` standing where those two
  values would (the rig has neither), and a greyed row taking no click;
  Start on an empty queue asks for a First song, Recently played's list
  hangs from its row with B's own history, and a song of it opens the
  session — the DJ armed, a batch of four badged rows after it (5 tracks);
  Preview's sheet lists three picks and queues none; Batch's sheet sets "1
  song" by a click on its bar's first cell, the border following the draft
  and the row waiting for Done; Length takes a typed `1:30` ("1:30+") and its
  `clear`; Genres lists B's own genres, one ticked then cleared; First song's
  list applies on a pick; the bar's chevron opens quick config over the bar
  and Esc puts it away; Play from's sheet moves the DJ to Rig A (the peer
  through B) — the rating row says "Not on peers" — and the next turn is
  picked from there; Stop leaves the rows and their badges; and the bar's
  `auto-dj` word, with B still browsed, starts the DJ on Play from's home
  (Rig A) and stops it. Needs B to have played something (the `stats` or
  `now` scenario, once). Prints the run's `[dj]` log lines at the end — none
  on a clean run: the DJ's log speaks only for a dropped key, a learned
  rejection, a degraded constraint, a deferred pick or a failed fetch.

- `ytdl` — Add from YouTube (add-from-youtube contract) on its own rig, not
  the two-server one: ONE server with mStream's scripted yt-dlp. Boot it from
  an mStream checkout with `MSTREAM_YTDLP_BIN=<checkout>/test/helpers/fake-yt-dlp.mjs`,
  `MSTREAM_FAKE_YTDLP_SCRIPT=<a JSON script>` and `MSTREAM_FAKE_YTDLP_FIXTURE=<an mp3>`
  set, `"transcode": {"ffmpegDirectory": "<checkout>/bin/ffmpeg"}` in its
  config, and a library `"Music"` holding `Daft Punk/Discovery/Aerodynamic.mp3`
  (`MSTREAM_YT_MUSIC` names that folder on disk if it isn't
  `$MSTREAM_RIG_DIR/music/Music`). The script's `details` name four ids:
  `FGBhQbmPwH8` ("Daft Punk - One More Time (Official Video)", channel Daft
  Punk), `liveStream1` (`is_live: true`, "Lofi radio - beats to relax"),
  `botCheck001` (`fail`: "Sign in to confirm you're not a bot. …" — the fake
  adds yt-dlp's `ERROR: [youtube] <id>:` itself) and `harderBettr` ("Daft
  Punk - Harder, Better, Faster, Stronger"); `"download": {"slowMs": 4000}`
  keeps `adding` on screen. Seed `gui-config/config.toml` with that server
  alone, and between runs delete `gui-config/ytdl.json` and every song but
  Aerodynamic (the title row's `✓ added` lasts an hour; a song left behind is
  a clash). The passes (`MSTREAM_YT_STEPS`, default all): `room` — Settings
  with no doorway (TORRENTS over its switch alone), then the nav's `Add
  YouTube` row under TOOLS, lit `▸ Add YouTube`, the title with no `◂`, a
  bracketed paste, the cleaned tags, Enter, the title row's `adding` → `✓
  added`, the note, the file on disk, Esc then `l` for the From YouTube
  list; `refuse` — the same link again (warned of, then the 409 standing
  alone), a live stream's warning, the bot check's failed lookup with a
  title typed and the add failing in the server's words (`Try again ·
  Dismiss`); `caps` — an Esc leaves nothing (a nav room has no way back),
  then `D` `T` `A` with nothing holding the keys stay in the room; `door`
  — no door at a library root, the door word on Discovery, `y`, a paste,
  `[ Add here ]`, the door's spot, the re-list with the new song under the
  cursor. With `MSTREAM_YT_OLDER=1`
  against a server before mStream #1123 (6.30–6.33: no `ytdl` on the ping),
  `refuse` checks the older tier instead: the clash fails after the download
  (`Change format and retry · Dismiss`) and a live stream has no warning
  ahead of time.

```sh
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py direct
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py mixed
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py dj
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py library
MSTREAM_RIG_DIR=/path/to/rig python3 -u smoke/gui/ptygui.py actions
MSTREAM_RIG_DIR=/path/to/yt-rig python3 -u smoke/gui/ptygui.py ytdl
MSTREAM_RIG_DIR=/path/to/old-yt-rig MSTREAM_YT_OLDER=1 python3 -u smoke/gui/ptygui.py ytdl
```

The worker-level leg of the same rig is the ignored test in `src/tui/worker.rs`:
`MSTREAM_RIG_PARENT=http://127.0.0.1:3041 cargo test -- --ignored rig`.
