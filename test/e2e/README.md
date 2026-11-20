# The wizard's e2e battery

`bash test/e2e/run.sh` drives the real binary through the real event
loop with expect(1), against a **stateful fake mStream**
(`fake_mstream.py`) — no Node, no real server, no audio files. Its
scenarios each run on a fresh fake (on a port the OS picks) and a fresh
`$HOME`, so they run side by side: `E2E_JOBS` at a time (8 unless set;
`E2E_JOBS=1` is one by one), the longest first, each one's lines printed
whole when it ends. `bash test/e2e/run.sh K W` runs only the scenarios
named. (T, the Discover room against the taste route, is described in
run.sh):

- **A** — the full English walk (folders → extras → login → Done), then
  the standalone `qr` pages on the session it saved: the half-block
  fallback at two heights, the forced-iTerm2 pixel path (asserted by its
  `1337;File=` escape actually leaving the app), and the Done button
  list from the keyboard.
- **B** — the same walk auf Deutsch (`MSTREAM_SETUP_LANG=de`), gating on
  German strings.
- **C** — quit before the login, reopen, finish: the boot-time seeding
  round-trip, asserted against the fake's real state (both folders
  survive, the user exists).
- **D** — public mode (Ctrl+S → Go public anyway) still renders the
  Quick Connect code on Done.
- **E** — click-to-rename via injected SGR mouse reports.
- **F** — the scan widget's whole choreography: file scan with a queued
  sibling, the enrichment passes (waveforms with an estimate, album art
  without), completion — `check_scan.py` verifies every state.
- **G** — the admin hub's sign-in: a fake with its auth wall up
  (`FAKE_AUTH=1`) and one account, `mstream-player admin` on a fresh
  `$HOME` gets the sign-in page, a good name + password opens the
  Libraries room, the session lands in `credentials.toml`, and the
  second run goes straight to the room.
- **H** — the GUI player (`mstream-player gui`) with no server saved and
  no fake at all: the demo seat (`MSTREAM_GUI_DEMO=1`) fills the bar,
  `0` opens the Now Playing screen (the TUI's view), a digit there picks
  one of its tabs rather than a nav room, Esc returns to the Library
  screen, `q` quits clean. The review's key-routing bug showed with no
  server; this leg would have caught it.
- **I** — the MP3 player's page (`mstream-player device flash`) against
  the scripted board (`MSTREAM_DEVICE_FAKE=ours:v0.4.0`) and a miniature
  merged image: the board is heard without a reset and its card offers
  Update, Enter opens the gate, `y` writes, the card comes back "just
  written", `d` opens Details, Esc leaves. Then, without expect, `device
  list` on two fake boards, `--yes` writing a fresh board end to end, and a
  held port refused.
- **J** — the SD-card transfer: the fake with `FAKE_LIBRARY=1` serves a
  made-up library on the routes a card sync reads (the paged sync
  manifest with its ETag and 304, `metadata/batch`, `/media` with Range
  and HEAD, `/transcode`, album art, embeddings), checked without expect;
  and the hidden `device card-sync` refuses a volume that isn't one. A
  sync itself needs a card in a reader, which CI has none of: the Rust
  tests run one end to end with a host folder as the card
  (src/device/card/sync/tests.rs).
- **K** — Keep a copy, end to end (`FAKE_TASTE=1 FAKE_PLUGINS=1`: the
  ping's `discoveryPlugins`, the plug-in listing, a throwaway `copies`
  destination, jobs that walk queued → running → done on the wall clock):
  the GUI at 100 × 30 with the queue folded opens the Discover room's
  Connected servers tab, lights the peer's album, `c` opens Keep a copy,
  Enter alone presses its one primary, Add album, and starts the copy (a
  ↓ would move the choice itself), Esc leaves it running, the room's
  poll sees it live and kept, `C` lists it in the Copies modal.
  `check_keep.py` replays the stream frame by frame for those states; the
  fake's `/_state` says what was sent (the row unchanged, scope album)
  and how the rows were looked up.
- **K2** — the same leg against a server from before the pin
  (`FAKE_NO_PIN=1`: the jobs route's strict body refuses `destination`
  with Joi's 400): the start is asked once more without the key and the
  copy runs unpinned — the same states, the same Copies; `/_state` says
  both bodies and their answers (400, then 202).
- **W** — Where copies land (the same fake; its destination saved per
  account as the server saves it, its libraries `copies` and `music`): a
  place saved with a base folder (older data) is seeded first; Keep a
  copy opens on it, `d` opens the picker folded into Custom with nothing
  dirty (Enter on the libraries saves nothing) — its title on the border,
  "Files go to …" its first row and none of the words the trim of
  2026-10-24 took out — "Start from an existing folder…" lists the
  library's own folders (the file explorer; the seeded folder is not
  among them — the first file creates it), a folder picked there starts
  Custom's layout and Save keeps it with no base folder; Keep a copy's
  place line ("Files go to …") reads the new place. The fake's `/_move_destination` then puts the default back
  behind the client's back, and Enter alone presses Add album — which
  sends the place it showed — answered 409 `destination_changed`:
  nothing starts, the place line redraws from the answer with its GOLD
  line under it. `check_dest.py` replays the stream; `/_state` says what
  was saved (once), read, listed and started.

- **M** — Apple Terminal's mouse dialect (`TERM_PROGRAM=Apple_Terminal`
  in the spawned env, `FAKE_TASTE=1 FAKE_PLUGINS=1`): every click is sent
  as that terminal sends one — the press an instant SGR pair, then, once
  the frame shows what it opened, the same pair again where the hand let
  go (the release echo; docs/ui-kit.md, behavioral rules). A connected
  server's tile's `[⋯]` opens its list and the list stays; a row's `[⋯]`
  opens the track-actions sheet and the sheet stays, the queue empty and
  nothing playing. The leg writes marks into the stream after each echo
  (`@@ECHO:list@@`, …); `check_echo.py` cuts them out and replays every
  frame between them. The same events with any other `TERM_PROGRAM` close
  the list and the sheet — the bug the dialect's filter fixes.
- **KM** — Keep a copy by the pointer alone, in the same dialect and
  against the same fake: the tile's list opens the window, and every
  control it draws is clicked, each press pair followed by its echo —
  Every album by, Album, `d change` (the picker; Esc folds it), the
  primary (the copy starts; its echo lands on the live window's
  `[ Close ]`), "x cancel copy", Add again (its echo lands on
  `d change`), "Esc close" — while the room beneath stays inert: resting
  on the band where the verbs line drew "⇄ shuffle", and on the footer,
  ripens no tooltip of the room's; its row under the window lights
  nothing. `check_keep_mouse.py` replays the stream at the leg's
  `@@KM:…@@` marks; `/_state` says each click acted once (one start, one
  cancel, no save). Every album by stands in the tile's window from its
  open (an album's window, the keep-a-copy spec §6.2); the fake answers
  Attic's albums through the browse proxy (the user's library holds
  Lanterns, so nothing is missing to tick), so its primary counts
  theirs: "[ Add 1 album ]".
- **P** — Keep a copy while browsing a connected server, on two fakes:
  Stage Peer B (`FAKE_ROLE=peer`, the keep-a-copy spec's sample library)
  behind the own server's browse proxy (`FAKE_PEER_URL`: mStream's
  federation-browse.js — the exact allowlist screened, 403 off it, the
  method and the JSON body passed through). `seed_peer_home` writes a HOME
  whose player opens on the peer at `peerlib/Boukmanflow`. The Files leg
  (`gui-peer-files`) keeps the open folder by the header's word (As on
  Stage Peer B: no landing on the wire), a held folder row Into a folder…
  through the chooser (the field emptied, "From Sam/Borrowed Tapes"
  typed, Esc keeping it: `landing: {vpath, path}`), another By your layout
  (`{tags: true}`, its tags read once), opens Copies outside Discover and
  waits for "✓ kept" on the header and the rows. The album leg
  (`gui-peer-album`) reads the album page's credit and its word, keeps a
  held row, the queue item through its sheet's "Keep a copy…" (`c` on
  the held queue row does nothing), the album by its word (`scope: album`
  with the page's credit), clicks the "✓ kept" state for its outcome, and
  opens the artist's focus on his wall. `check_peer.py` replays each
  stream at its `@@PF:…@@` / `@@PA:…@@` marks against the spec's frames;
  with `E2E_CAPTURES=DIR` it keeps every mark's decoded frame there.
  `/_state` of both fakes says what was started (each copy into the fake's
  `copies` library), what the proxy refused (nothing), the recursive
  listings (one a window) and the tag batch (one, ≤ 100 songs).
- **R** — the GUI's Peer recs room as an admin, in Apple Terminal's mouse
  dialect (`FAKE_P2P=1`: the ping's four flags, and the stage's peers,
  leads, per-tab answers and previews from `test/fixtures/peer_recs`, the
  files the Rust tests read; docs/ux-contracts/peer-recommendations.md).
  The seat is silent and stays in the leg: `seed_quiet_home` writes
  `[player] volume = 0.0` before the GUI starts, and the leg sets
  `SSH_CLIENT`, so a copy goes out as OSC 52 into the stream, never onto
  the machine's pasteboard. At 100 × 30 with the queue folded, `R` opens
  the room: Peer recs on nav row 18 under Discover, the header on row 2,
  the strip on row 4, the twelve leads 6 | 6 with their titles at x 19 and
  59, and one golden R1 body. `y` copies the twelve lines in rank order. A
  click on the Kitchen Radio tab asks for its five leads at once, pinned to
  the batch's seeds (`networkPeers`, `pin`, `limit.seeds`); its echo asks
  nothing more. A click on a row opens the lead's sheet, and its echo,
  landing on the sheet's own `[X]`, closes nothing and presses no row. The
  previews are asserted where the run's log (`MSTREAM_LOG`) says
  `output opened on`, their absence where it does not (no Preview row, no
  slot, no band, no resolve): on a machine with a device, `l` on the lead
  whose clip URL is dead notes No preview and forgets the answer, so the
  next `l` resolves once more, and its slot reads idle, not muted; `l` on
  another plays its 2 s clip in the band. The tabs stop walks to
  `+ Lighthouse Tapes`: Enter posts the Add once, the tab reads
  `adding…` and is inert, the fast poll sees it land (Lighthouse Tapes on
  the shelf, Attic Tapes gone), and one pinned re-ask follows.
  `check_peer_recs.py admin` replays the stream at the leg's `@@PR:…@@`
  marks, each with the fake's `/_state` kept beside it
  (`state-<mark>.json` in the leg's HOME), and reads the colours where only
  they tell a state (the stop's slab, an idle slot); with
  `E2E_CAPTURES=DIR` it keeps every mark's decoded frame there. `/_state`
  then says
  one Add, three network asks, no preview artwork fetched
  (`art_gets == 0`) and no Discover body carrying a generation-3 key
  (`discover_gen3_bodies == 0`).
- **R2** — the same room as a member (`FAKE_P2P=1 FAKE_MEMBER=1`): the
  strip is the shelf alone with nothing past it, the tabs stop walks to its
  last tab and selects it, no frame draws a `+`, `adding…` or
  `try again`, and the fake counts no Add (`check_peer_recs.py member`).

## The three harness laws (learned the hard way)

1. **Never grep the raw stream for anything that repaints in place.**
   ratatui's diff renderer emits only changed cells — a later
   percentage or a rewritten word never appears as a contiguous token.
   `replay.py` is the oracle: it renders a captured stream (or any
   prefix of one) to a final frame; `check_scan.py` slices a stream
   through time this way. The checkers ask `replay.frames` for every
   prefix they read at once — one pass over the stream, never a process
   and a replay from byte 0 a frame. Gate expect on FIRST-paint single
   words only: even a first paint skips the blank cell between two plain
   words with a cursor move (`Second` ESC`[9;27H` `Shift`), so "Second
   Shift" never matches.
2. **Drain, don't sleep.** A pty whose output buffer fills freezes the
   TUI mid-run; every wait in a leg is an `expect -timeout N timeout {}`
   drain that keeps reading.
3. **`expect {` ends its line.** expect reads a braced argument as a
   pattern list only when it is the sole argument and a newline comes
   before its first pattern. On one line, or after a flag, the whole
   `"X" {} timeout {…}` is ONE glob that never matches: the gate waits
   out its timeout, runs no arm, and can never fail. Nineteen gates did
   that until the performance audit (#129); `run.sh` refuses the form.

Env seams the legs rely on: `MSTREAM_SETUP_LANG` (deterministic
language), `MSTREAM_NO_GRAPHICS=1` (no pixel probe — nothing answers
under expect, and the probe's silence costs seconds),
`MSTREAM_GRAPHICS=iterm2` (force the pixel path; the escape lands in
the capture), `MSTREAM_NO_OPEN=1` (browser-opens become notes carrying
the URL).

Against a real mStream server (the full-fidelity ritual, run manually
before releases): boot one with a scratch config, then run individual
legs with `ARGS` pointed at it — everything the fake stubs (real
scans, real Quick Connect tickets, real auth) gets exercised for real.
