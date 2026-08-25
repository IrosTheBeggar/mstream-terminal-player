# The wizard's e2e battery

`bash test/e2e/run.sh` drives the real binary through the real event
loop with expect(1), against a **stateful fake mStream**
(`fake_mstream.py`) — no Node, no real server, no audio files. Six
scenarios, each on a fresh fake and a fresh `$HOME`:

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
