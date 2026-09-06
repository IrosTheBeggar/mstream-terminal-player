# Sonic path

| | |
|---|---|
| **Design of record** | `mstream_music` @ `137dd27` (2026-08-26) — `lib/screens/sonic_path_screen.dart`, `lib/widgets/song_picker_sheet.dart`, `lib/singletons/sonic_path_state.dart`, `lib/singletons/track_capture.dart`, `MatchMeter` in `lib/screens/discover_screen.dart` |
| **Server API** | `POST /api/v1/discovery/local/path` (mStream #762+); pick helpers `POST /api/v1/db/random-songs`; gate rides the ping's `discoveryPathAvailable` flag |
| **Already in this repo** | API client `Client::journey_async` + `JourneyResponse`/`JourneyStop`/`NotAnalyzed` (src/api); App-level state machine + TUI tab (src/tui/app/sonic.rs) |
| **Target surface** | the GUI player (`mstream-player gui`), as a left-nav room |
| **Status** | implemented in the GUI, 2026-08-31 (18 new tests + a live-server smoke); the TUI shares the App-level additions |

## Intent

Pick a start song and an end song; the server plots the ordered journey
between them — waypoints along the arc between the two tracks' embeddings,
each snapped to a real library track — and hands it back as a playable,
queueable, saveable list. Both seeds are included in the result, so the
result *is* the queue.

## Entry points

1. **Main navigation** — shown only when the server's ping advertises
   `discoveryPathAvailable`; the entry disappears again when a later ping
   drops the flag. Opens on the setup stage, keeping any endpoints already
   picked this session (re-entry continuity).
2. **Discover's "Play a path to…"** — start is the Discover seed, the user
   picks only the destination; the screen opens directly on results and
   builds immediately (`autoBuild`).
3. **Return from browse-to-pick** — an armed library pick lands its track
   and re-opens the screen with the card filled (see rules 10–13).

## States & flows

Two stages: **Setup** (two endpoint cards, length, Build) and **Results**
(endpoint chips + length + Regenerate above the arc, pinned actions below).

Setup state **outlives the screen** — browse-to-pick tears the screen down
and re-opens it after the captured tap, and Start over returns to a setup
that still shows the same server binding. State: `server`, `start`, `end`,
`length` (defaults: unset, unset, 14).

- Entering from navigation binds the current server; **switching servers
  invalidates both endpoints** and resets length (other servers can't seed
  this one's index).
- **Start over** clears both endpoints, resets length to 14, returns to a
  pristine setup on the same server, and orphans any in-flight build.

## Behavior contract

### Picking an endpoint (both cards, identically)

An empty card reads "Not set" and offers the pick methods; a **filled card
hides the pick buttons** (settled decision, webapp parity) and shows the
pick — art thumbnail, title (filename tail as fallback), artist — with a ✕
that clears it and reopens the methods.

1. **Use playing song** — seeds from the current track. Refused with
   "Nothing is playing" when idle, and refused when the playing track
   belongs to a different server ("Pick a track on {server}") — same guard
   the webapp applies to federated tracks.
2. **Random song** — one random library track as the pick; a fetch failure
   says "Couldn't fetch a song from the server." and leaves the card as it
   was. (In the record this row lives inside the search sheet; see Out of
   scope for where it lands here.)
3. **Search** — debounced title search (350 ms, min 2 chars). *Skipped on
   this surface — see Out of scope.*
4. **Browse library** — arms app-wide capture and drops the user into the
   normal library browser with a persistent banner ("Pick the start/end
   song — tap a track anywhere in the library").

### Browse-to-pick capture

10. While armed, activating any track row anywhere in the library is
    **consumed by the capture** — it must never also play or queue.
11. A pick from the wrong server (or a non-seedable row) is consumed but
    **rejected**: a notice shows and the capture stays armed.
12. A landed pick fills the card and returns to this feature's screen;
    cancelling (Esc) disarms and also returns there — the record re-pushes
    its screen on cancel exactly as on capture. *(Corrected at
    implementation: the extraction misread this as "stays put".)*
13. Re-entering the feature through navigation **disarms any stale
    capture** — a leftover armed pick must not swallow a later browse tap.

### Length and Build

20. Length spans **4–32, default 14**, and counts total rows *including
    both seeds*. The control always shows the current number.
21. **Build** is enabled only when both endpoints are set; it switches to
    results and fetches.
22. In results, **changing length does not rebuild** — that waits for
    **Regenerate** (disabled while a build is in flight). Changing an
    **endpoint rebuilds immediately** — the old journey is wrong the moment
    its anchor moved.
23. Concurrent fetches: **last request wins**; a stale response is dropped,
    never rendered.

### Results

30. Endpoint chips stay visible and editable in every result state — a
    failed or not-analyzed journey is fixed by editing them, not by leaving.
    On the record, chip tap re-picks via search; here the chip re-opens the
    pick methods (see Out of scope).
31. The list is the journey in order. **Seed rows (first, last) are
    accented** and marked origin/destination; **waypoints wear a match
    meter** — a slim vertical fill plus the similarity as a bare percentage
    number (`similarity` 0..1 from the API).
32. Row subtitle: artist, and genre tag when known, dot-separated.
33. Row activation plays the journey from that row.
34. Pinned beneath the list, always visible while rows exist: **Play**
    (primary — plays the whole journey), **Queue all** (appends; confirms
    with "{n} songs added to queue"), **Save as playlist** (create or
    overwrite by name — picking an existing name replaces its contents, the
    family's save-flow convention; confirms "Added to {name}", failure gets
    its own notice).
35. **Metadata healing**: the cards/chips show whatever the picker knew;
    once a journey builds, seed titles/artists are backfilled from the
    server's own metadata so a filename-titled pick heals.

### Failure taxonomy

The route answers 403 for both "discovery is switched off" and "no index
yet", deliberately. The two answers live in different places — the ping
carries the config flag, the route carries the data — so:

40. On a 403, **re-ping before naming a reason** (probe inside the failure
    path — the user is never shown an explanation that then gets
    retracted). Flag still on → **scan pending**: "This server hasn't
    analyzed any music yet, so there's nothing to build a path through. It
    works once the discovery scan has run." with Retry (a scan that
    finished in the meantime resolves on the next try). Flag gone →
    **turned off**: "Sonic discovery has been switched off on this
    server.", no retry — and the nav entry goes with the next ping.
41. Transport/server errors are **transient**: "No matches found." with
    Retry. (The record notes: don't blame the library for a build the
    server never attempted — but it accepts the generic message + Retry.)
42. Per-end **not-analyzed**: name the end that's waiting ("The
    starting song hasn't been analyzed yet — wait for the discovery scan or
    pick another." / same for destination), no retry button — the fix is
    editing or waiting.
43. An empty result is "No matches found." with Retry. A **short journey is
    an answer, not an error** — the library ran out of visible tracks.
44. While building: a quiet loading state, nothing interactive lost.

## Wording

English reference; the record ships all ten of this repo's locales
(de en es fr it ja pl pt ru zh), so translations carry over. Record keys in
parentheses.

| String | (record key) |
|---|---|
| Sonic path | pathScreenTitle |
| Pick a start and an end song — the journey between them fills itself. | pathSetupHint |
| Start song / End song | pathStartSong / pathEndSong |
| Not set | pathNotSet |
| Use playing song | pathUsePlaying |
| Search | pathSearchSong |
| Browse library | pathBrowseLibrary |
| Random song | autoDjSonicSeedRandom |
| Couldn't fetch a song from the server. | autoDjSonicSeedFailed |
| Length | pathLength |
| Build the journey | pathBuild |
| Regenerate | pathRegenerate |
| Start over | pathStartOver |
| Play / Queue all / Save as playlist | play / discoverQueueAll / pathSaveAsPlaylist |
| Pick the start song — tap a track anywhere in the library | pathPickBannerStart (End variant likewise) |
| Nothing is playing | pathNothingPlaying |
| Pick a track on {server} | pathPickOnServer |
| No matches found. | discoverNothingFound |
| Sonic discovery has been switched off on this server. | discoverTurnedOff |
| This server hasn't analyzed any music yet, … | pathScanPending |
| The starting song hasn't been analyzed yet — wait for the discovery scan or pick another. | pathStartNotAnalyzed (End variant likewise) |
| {n} songs added to queue | browserSongsAdded |
| Added to {name} | addedToPlaylist |
| Retry | lyricsRetry (the record reuses its generic retry) |

"tap a track" in the banner becomes this surface's verb (click).

## Out of scope here

- **Search pick** (decision 2026-08-31): the quick-search method is not
  ported. Consequences the contract must absorb:
  - In the record, **Random lives inside the search sheet**; without the
    sheet, Random becomes a pick-method button on the card alongside Use
    playing / Browse library.
  - The results-stage **endpoint chips re-pick via search** in the record;
    here a chip tap offers the remaining methods instead (rule 30).
  - The Discover "Play a path to…" destination picker is also the search
    sheet; that entry point ports only once this surface has a Discover
    room, and may then revisit this decision.

## Translation notes (terminal GUI)

Idiom mapping, per the kit's conventions (docs/ui-kit.md, the GUI canvas):

| Record | Here |
|---|---|
| Drawer entry | Left-nav room (gated like the record's drawer entry) |
| Bottom sheets | Kit modals (`modal_frame`) |
| Snackbars/toasts | The note line above the bar |
| Capture banner | The App's existing armed-capture banner |
| Tap | Click *and* a key — every verb keyed, tips line names them |
| Slider | Kit-drawn bar, ←→ to adjust (the TUI's reading) |
| MatchMeter (4×28 px vertical fill + percent) | Cell-sized meter + percent — design in the canvas before building |
| Icon-only Save button | No icon-font equivalents; all three actions get words |

Already shared under the GUI (the App the TUI drives): the whole sonic
state machine — endpoints, length banding, Build/Regenerate, Save as
playlist, play-from-stop, capture arming/answering, the
`capabilities.discovery_path` gate, and `JourneyResponse` handling
including per-end not-analyzed wording. The GUI slice is drawing plus
mouse verbs mapped to existing `Action`s, the Files-room pattern.

Gaps in the shared App that this contract adds (verify at implementation):

- **Random pick** — API exists (`random_song`), no App action yet.
- **403 re-ping probe** (rule 40) — the TUI gates entry on capabilities at
  open; the record's in-flight distinction (scan pending vs turned off) is
  finer. Decide whether the probe lives in the App (both surfaces gain it)
  or GUI-side.
- **Edit-endpoint-in-results with immediate rebuild** (rule 22) — the TUI
  re-picks via its rows; chips-with-rebuild is the record's shape.
- **Match meters** — the TUI shows stops as plain rows; meters are new.
- **Metadata healing of the cards** (rule 35) — record behavior; check
  whether the TUI backfills.

## Deviations log

- **2026-08-31 — Search pick skipped** (decision at extraction; see Out of
  scope). Revisit when a Discover room lands.
- **2026-08-31 — Keyboard verbs added** (surface requirement): every
  contract verb gets a binding; the record can't answer which — design in
  the canvas alongside the room.
- **2026-08-31 — Save button gets a word** (no icon fonts in cells); the
  record's narrow-screen rationale doesn't apply at min 100 columns.
- **2026-08-31 — Keys settled at implementation**: digit 9 reaches the room
  (existing digits never renumber); setup ↑↓/Enter/x/←→/b; results
  Enter/a/e/E/r/s/o/←→; the pick-methods menu is the keyboard form of the
  card buttons and the chips.
- **2026-08-31 — Narrow-width fallback**: with the queue panel open the
  inline method buttons can outgrow the card; the card then reads "click
  or Enter to choose…" and opens the methods menu instead of clipping the
  row mid-word.
- **2026-08-31 — The 403 probe lives in the App** (clause 40), so the TUI's
  tab gains the scan-pending / turned-off distinction too. The probe names
  the reason only; the nav entry still leaves with the next real ping.
- **2026-08-31 — The same-server guard on "Use playing" is vacuous here**
  (clause 1): a native session plays only the connected server's tracks,
  so no guard is drawn. Revisit if cross-server playback ever lands.
