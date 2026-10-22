# The wizard's e2e battery

`bash test/e2e/run.sh` drives the real binary through the real event
loop with expect(1), against a **stateful fake mStream**
(`fake_mstream.py`) — no Node, no real server, no audio files. Its
scenarios each run on a fresh fake and a fresh `$HOME` (T, the Discover
room against the taste route, is described in run.sh):

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
- **K** — Keep a copy, end to end (`FAKE_TASTE=1 FAKE_PLUGINS=1`: the
  ping's `discoveryPlugins`, the plug-in listing, a throwaway `copies`
  destination, jobs that walk queued → running → done on the wall clock):
  the GUI at 100 × 30 with the queue folded opens the Discover room's
  Connected servers tab, lights the peer's album, `c` opens Keep a copy,
  Add album starts the copy, Esc leaves it running, the room's poll sees
  it live and kept, `C` lists it in the Copies modal. `check_keep.py`
  replays the stream frame by frame for those states; the fake's
  `/_state` says what was sent (the row unchanged, scope album) and how
  the rows were looked up.
- **K2** — the same leg against a server from before the pin
  (`FAKE_NO_PIN=1`: the jobs route's strict body refuses `destination`
  with Joi's 400): the start is asked once more without the key and the
  copy runs unpinned — the same states, the same Copies; `/_state` says
  both bodies and their answers (400, then 202).
- **W** — Where files land (the same fake; its destination saved per
  account as the server saves it, its libraries `copies` and `music`):
  Keep a copy opens on the library's default, `d` opens the picker, Tab
  completes a folder from the library's own (the file explorer) and Enter
  saves it; Keep a copy reads the new place, saved for the account. The
  fake's `/_move_destination` then puts the default back behind the
  client's back, and Add album — which sends the place it showed — is
  answered 409 `destination_changed`: nothing starts, LANDS IN redraws
  from the answer with its GOLD line. `check_dest.py` replays the stream;
  `/_state` says what was saved, read, listed and started.

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

## The two harness laws (learned the hard way)

1. **Never grep the raw stream for anything that repaints in place.**
   ratatui's diff renderer emits only changed cells — a later
   percentage or a rewritten word never appears as a contiguous token.
   `replay.py` is the oracle: it renders a captured stream (or any
   prefix of one) to a final frame; `check_scan.py` slices a stream
   through time this way. Gate expect on FIRST-paint single words only.
2. **Drain, don't sleep.** A pty whose output buffer fills freezes the
   TUI mid-run; every wait in a leg is an `expect -timeout N timeout {}`
   drain that keeps reading.

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
