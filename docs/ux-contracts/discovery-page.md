# Discover — the history-seeded page

| | |
|---|---|
| **Design of record** | mStream's plan, `docs/discovery-page-plan.md` @ `a6076a7a` (2026-10-05, plan v2 after its adversarial review) — §1 (the three sources and the verbs a row carries on every client), §2 (the decisions: seed rule 2.1, window and widening 2.2, pins and remembered exclusions 2.3, the library filter 2.5, albums before songs 2.6, source stamping 2.7, the random fallback 2.8, placement 2.9, flag-never-probe 2.10, naming 2.12, **2.13: where the plan departs from the canvas — the plan wins**), §3.1–§3.3 (the server contract), §4.3 (this surface). The canvas "mStream Discovery Page" (https://claude.ai/artifact/SgZVPShtC92MFhkrkJbLcA) — boards `Tui` (Library tab, pointer on a row), `TuiNetwork` (the `[⋯]` menu open), `TuiPeers` (keyboard cursor), `TuiEmpty` (nothing finished yet) and the notes `n_tui` / `n_tui_ill` — is the record **for visuals only**: layout, wording and order; where it and the plan disagree, the plan is followed and the difference is logged below. **Amended 2026-10-11: the PLAY-NOW page** — plan §2.13's 2026-10-11 lines (as built on the webapp, mStream `6b33af36` / `9a3e492f`) and the design canvas https://claude.ai/artifact/AyZ2yH6431ajvmWgxjZuxo: only songs you can play now, no counts, no percentages, no p2p network (see the log). **Amended 2026-10-14: the stateless page** — mStream `8dcfad0e` (the plan's §1, §2, §3.1 and §2.13's dated note): every call draws a new random batch of seeds, nothing is stored on the server, the library filter and the prefs routes are gone (see the log). **Amended 2026-10-19: Keep a copy, Open elsewhere and the Copies modal** — the design canvas "Discover plug-ins · phone & terminal" (artifact `46efba8d-dd1b-40be-b9aa-b5805a63cbcb`; boards `TermRoom`, `TermEntryPoints`, `TermKeepModal`, `TermKeepStates`, `TermCopies` and the shared model `Main`, approved by the user 2026-10-19: "the designs look good, start the work"), the webapp's side as built (mStream `71663032`: `webapp/alpha/discover-jobs.js` `jobRowState`, `discovery-page.js` `jobBadges` / `peerAlbumBadges` / `jobForAlbum`) and the user's decisions of that day (the room shows no counts or percentages; a compilation's album copy takes the whole album) — clauses 100–141 (see the log). |
| **Server API** | **`POST /api/v1/discovery/taste`** (§3.1), stateless since 2026-10-14 — nothing is stored or cached between calls — `{window? (7 \| 30 \| 90 \| 0, default 30), pin? (≤ 12 — `PIN_CAP`, the seed cap, since 2026-10-14 — seed first), exclude? (seed hashes left out of THIS call only, ≤ 200), sources?, limit?, newArtistsOnly?, ignoreVPaths?}` (`remember`, `filter` and `staleDays` are a 400) → `{window, widenedFrom, seedSource, fallback, seeds: [{hash, filepath, metadata, outcome, lastPlayed, plays, pinned, analyzed}], excluded: [{hash, filepath, metadata}] (this call's `exclude` hashes that resolve), dropped, notAnalyzed, model, library: {tracks: [{filepath, similarity, seedHash, metadata, genreTags, playCount, lastPlayed}], albums: [{…Album, similarity, scopedTracks, playedTracks, lastPlayed, tracks: [path]}], capped}, network: {results: [p2p row + {source: 'p2p', seedHash}], searched: {peers, tracks}} \| null, peers: {peers: [{id, name, reachable, mismatched, lastSeen, results: [federation row + {source: 'federation', seedHash: null}]}], searched: {peers, unreachable, mismatched}} \| null}` — an account only (403 for a federation key or a guest), 403 `Discovery is disabled` when discovery is off or the index is unavailable; `network`/`peers` are `null` when the source is not advertised for the caller or a random seed is in use. The seeds are the pins, then a **random batch** of the account's finished songs in the window — a different batch on every call — widening window → all time → counters → one random library song (drawn afresh on every call, 2026-10-14; never one in `exclude`). The library half is the fixed rule "songs you haven't played lately" (never, or more than 90 days ago). *(`GET` / `PUT …/taste/prefs` are gone since 2026-10-14 — a 404.)* **The plug-in routes** (since 2026-10-19, clauses 100–141; mStream's `docs/openapi.yaml`), every one asked of the **user's own server** — never a connected server's, through the parent's proxy or over its own tunnel: `GET /api/v1/discovery/plugins` → `{plugins: [{name, title, description, capabilities, scope, scopes, settings, enabled, available}], jobs: {allowed}}`; `GET /api/v1/discovery/collection/destination` → `{destination: {vpath, base, layout, source: user \| default} \| null, saved, libraries: [{vpath, template}], defaultLayout, variables}` — asked with the gate and again with every Keep a copy (clause 105); `PUT /api/v1/discovery/collection/destination {destination: {vpath, base, layout} \| null}` (since 2026-10-21, clause 144: per account; `null` back to the library default) → the same view, a place the server will not take a 400 in its words (`destination layout: Unknown variable {{ARTST}}. Supported: …`); the picker's folders by `POST /api/v1/file-explorer {directory: "/{library}/{folder}"}` → `{directories: [{name}]}`; `POST /api/v1/discovery/plugins/federation-copy/jobs {recommendation, scope?, destination?}` (mStream 95f2205a: `destination` the place the client showed, `{vpath, base, layout}` or `null`, clause 145) → `202 {job, created: true}` or `200 {job, created: false}` (a live job already covers it — whatever place was shown), the job pinning `params.destination` at start and naming it in `result.destination`; **409 `{error: "destination_changed", message, destination}`** when the place shown is no longer the one in force — nothing starts, `destination` the one that is (or `null`, nowhere); refused otherwise with the server's words: 400 (a scope the row cannot fill), 403 (jobs not allowed, "Uploading Disabled"), 404 (the plug-in gone), 409 (another account's live job — its own words), 429 (the queue cap, 20); `GET /api/v1/discovery/plugin-jobs` → `{jobs, runner}`; `POST …/plugin-jobs/{id}/cancel` → `{job, outcome: cancelled \| requested}`, **409 = it had already finished**; `POST …/plugin-jobs/clear` → `{removed}`; `POST …/plugin-jobs/lookup/batch {recommendations}` (1–100 a call; chunked) → `{results: [{key, keys: {song, album, artist, artist-missing, folder}, jobs}]}` in input order; `POST /api/v1/discovery/plugins/links/resolve {recommendation}` → `{result: {links: [{id, label, url, kind: canonical \| search}]}}`; and, for the Keep modal's sizes, `POST /api/v1/db/album-songs {album, artist? \| album_artist?, year?}` and `POST /api/v1/db/artists-albums {artist}` on the connected server (through its reach) and on the user's own. A job is `{id, plugin, key, recommendation, params: {scope?, destination?}, state: queued \| running \| done \| failed \| cancelled, progress 0..1 \| null, statusText, result, error, cancelRequested, createdAt…}`; a song copy's `result` is `{copied: {vpath, filepath, title, artist, album, …}}` or `{skipped: owned \| exists, existing?, filepath?}`, an album or artist copy's `{scope, songs: {total, copied[], skipped[], failed[]} (each capped at 500), counts: {total, copied, skipped, failed}, albums?, skippedAlbums?: [{name, why: owned \| appearance}], stopped: null \| cancelled \| quota \| busy \| peer \| refused, peer, destination}`. The routes the player already spoke stay: the federation byte proxy `GET /api/v1/federation/peers/{id}/stream/…` and `POST /api/v1/stats/plays` with `peerId`, both through the queue as it is. **Gate**: the ping's `discovery === true && discoveryTaste >= 1` (§2.10) — here `capabilities.discovery && capabilities.discovery_taste >= 1`; the Connected servers source rides `federationDiscovery`; `discoveryPlugins` and `noUpload` gate the plug-ins (clause 101); `discoveryP2p` (the Network source) is parsed but read by nothing here — the later Explore room's. |
| **Already in this repo** | The App/worker pair every room rides: `Effect::Api(ApiCmd::…)` drained onto the api thread, answered by `worker::answer` (and `src/web/api_worker.rs` in the browser build) as an `Event` that `App::apply_event` folds in, stale replies dropped by seq / node / session-generation guards. The **Sonic room's own row model** — `List::Sonic` maps to no App pane (`List::tab()` → `None`), its rows live in App state and its verbs call App methods that build the queue rows themselves (`src/gui/sonic.rs`), with `sonic::meter_glyph` for the one-cell match meter. The kit's **Tabs** (first consumer: the admin Discovery room), **Dropdown** (the header's server switcher, the stats page's period), **Table** (`table_view`, `scroll_list`, `ListView`), a list hung from a control (`frame_at` + `Surface::overlay`, the dropdown's own grammar) and **modals** (`modal_frame_on`). `Client::stats_history_async` (the one outcome-bearing history read — named so nobody reaches for it: the server picks the seeds, clause 10). `Client::random_song_async`; `gui::opener::open_target` (honours `MSTREAM_NO_OPEN`); `kit::clipboard::copy`; the track-actions sheet (`actions::Sheet { origin, track }`); the capture banner (`Capture`); `App::origin()` and `App::reach(&Origin)` for a peer origin (multi-server clauses 27, 30); `ApiCmd::DiscoveryProbe`; `GuiPrefs` (`[gui]`, unknown keys kept). **Missing**: `Ping.discovery_taste` / `discovery_plugins` and their `Capabilities` mirrors, the taste types and client calls, `ApiCmd::Taste` / `Event::Taste`, the App's taste state, `NavId::Discover`, `src/gui/discover.rs`, a capture for a seed pick, the `gui.discover.*` strings. **Added 2026-10-19 for clauses 100–141**: the plug-in types and their pure halves (`src/api/types/plugins.rs`: the listing, `PluginJob` and its `outcome()` — the webapp's `jobRowState` — `JobMark`, `CopyResult` / `ManyCounts`, `JobLookup`, the destination with `resolve_layout` (replaced 2026-10-21 by `Landing::of` / `layout_folders`), `PluginLink`, `ArtistAlbumRow`, `name_key`, `artist_ownership`, `job_for_album`), `Capabilities.uploads` (the ping's `noUpload` false; absent grants nothing), `TasteLead.album_artist`, the client calls (`Client::discovery_plugins_async` … `plugin_jobs_lookup_batch_async`, refused for a peer's client), `PluginFailure` (the server's words), the `ApiCmd`/`Event` pairs `KeepGate`, `JobsLookup`, `Jobs`, `JobStart`/`JobStarted`, `JobCancel`/`JobCancelled`, `JobsClear`/`JobsCleared`, `PluginLinks`, `KeepFacts` with their shapers (`worker::keep_gate` …, both workers), and the App's half (`src/tui/app/keep.rs`: the gate, the rows' jobs and marks, the poll, the Keep flow and its `KeepView`, the Copies rows, the links list). **Built 2026-10-20**: the drawing of the boards `TermRoom`, `TermEntryPoints`, `TermKeepModal` and `TermKeepStates` — `src/gui/keep.rs` (the Keep a copy modal in every state, the marks' glyphs and words, the status notes, the Copies modal's frame), the room's marks, indicator, `c` / `C`, the tile's list and tips in `src/gui/discover.rs`, the sheet's two rows and its badge row in `src/gui/actions.rs`, `MessageKind::Busy` / `Success` for the notes' tokens, and the `gui.act.*`, `gui.discover.keep.*` / `mark.*` / `copies.*` / `note.*` and `gui.tips.discover_*` strings in the ten locales. **Built 2026-10-20 (the second pass, the board `TermCopies`)**: the Copies modal's list (`gui::keep::draw_copies` over `App::copies_rows`, `CopiesUi` — the held copy by its job's id and a `ListView` in copies — `copies_key`, `copies_tips`, `copies_wheel`, the `DiscAct::Copies*` acts, `App::copies_action`: Copy again only where a gap is left), Open elsewhere's list (`gui::keep::draw_links`, `LinksUi` — its stop, viewport, its row's anchor from the sheet, and the opener's answers on a channel from `gui::opener::open_target`'s thread — `links_walk` / `links_enter` / `links_tips`, `DiscAct::Link*`), `MessageKind::Plain` (TEXT) for "opened …", the `gui.discover.copies.*` / `links.*` and `gui.tips.discover_copies_*` / `discover_links*` strings, and the e2e leg `test/e2e/legs/gui-discover-keep.exp` with `test/e2e/check_keep.py` (Scenario K). The temporary dead-code allows of the data layer are gone with it. **Built 2026-10-21 (Where files land, clauses 142–148)**: the destination's view, its save and the library's folders (`Client::collection_destination_save_async`, `ApiCmd::DestinationRead` / `DestinationSave` / `DestinationFolders` and their events, `App::keep_dest_*`, the view stamped by the ask that wrote it), the start's `destination` and its 409 (`JobStartAnswer`, `PluginFailure::DestinationChanged`, the job's `params.destination` pin, a server from before the key asked again without it), `Landing` / `layout_folders` / `CopyResult::landed` for LANDS IN and the PREVIEW, `KeepView::again` (the place Retry and Copy again send), the picker `gui::keep::DestUi` / `draw_dest` / `dest_key`, the `gui.discover.dest.*` and `gui.tips.discover_dest_*` strings in the ten locales, and the e2e legs `gui-discover-dest.exp` with `check_dest.py` (Scenario W) and Scenario K2 (`FAKE_NO_PIN`). **Built 2026-10-22 (a variable each song fills, clause 105)**: `SongTag`, `LayoutTags::slots`, `Run` (a landing's runs: words, a refused variable, a slot), `Landing::words` / `slots` / `one_song`, the server's `~` / `$HOME` / `${HOME}` fold in `sanitize_segment`, `TrackMetadata::artist_display` (the listing's `artist-display`), `KeepSong` on `KeepFacts::album_songs` (each song's ARTIST tag as written, its year, its one listed genre), `KeepView::album_songs` and the per-scope rule in `KeepView::scope_tags`, the slots and their note drawn by `gui::keep` (`slot_words`, `slot_note`, `path_tip`), the kit's tooltip wrap and box measured in cells (`wrap_tip`, `draw_tooltip`), the `gui.discover.dest.slot_*` strings in the ten locales. |
| **Target surface** | the GUI player (`mstream-player gui`, both flavours — the window host draws the same `render`) as a **TOOLS nav room**. **The TUI changes nothing**: its Discover tab (`Tab::Discover`, the explicit-seed lookup), the Now Playing panel's neighbours (`App.now_discover`) and `Capture::Discover` stay exactly as they are. The one shared change is a title: the Now Playing view's Discover tab becomes **Similar** (plan §2.12; docs/ux-contracts/now-playing.md) |
| **Status** | contract extracted 2026-10-05; built 2026-10-05; reworked into the play-now page 2026-10-11; the album strip 2026-10-11; the verbs line over the table and save as playlist 2026-10-16; the header's controls — no state line, no seeds line — 2026-10-16; Keep a copy, Open elsewhere and the Copies modal contracted 2026-10-19, their data layer built the same day; the room's marks and indicator, the ways in, the Keep a copy modal and the notes drawn 2026-10-20; the Copies modal's list, Open elsewhere's list and the Keep a copy e2e leg the same day; Where files land — the picker, the destination read with every Keep a copy, the start's pinned destination — contracted and built 2026-10-21, after the webapp's redesign (mStream 95f2205a, a7d4459f) (the log) |

## Intent

A room that knows what you like because you finished it, and offers only
what you can **play now**. The server takes a random batch of the songs
you played to the end (or most of the way) in a window you choose, never
the ones you skipped — a new batch every visit — and answers with songs and albums in **your own library** that
sound like them and that you have not played lately, and songs on your
**connected servers** you can play through your own server. The seeds are
yours to edit behind **more options**, a new batch is one click away
(**↻ new seeds**), the room says in words what it is based on —
never a count, a percentage or a "never played" — and every row plays and
queues. The room never goes blank: with nothing finished yet it starts
from a random pick and says so. The p2p network's leads are not here: they
cannot play, and a later Explore room draws them.

## Entry points

1. **The nav row `Discover`** — the TOOLS group's third room, row **16**
   under Sonic path (TOOLS label 13, Auto DJ 14, Sonic path 15, Discover
   16; Settings stays on the content's last row — clause 2). No digit —
   the digits end at 9 (library-rooms entry point 1) and never renumber
   (the sonic-path log, 2026-08-31) — so **`G`** from the Library
   screen. Drawn only while the ping carries the gate and the session is
   not a peer's (clause 1).
2. **Nothing else in v1.** No "discover from this song" on the track
   sheet, no hand-off from the Now Playing screen: that screen's per-song
   lens is retitled **Similar** (plan §2.12) and stays the TUI's view, and
   this room is the history page alone.

## States & flows

One room, one answer, two tabs. The answer arrives in two halves
(clause 30): **the library**, then **the connected servers**, asked once
the library's answer lands and pinned to its batch of seeds — so the
Library never waits on a 4 s server deadline, and both tabs and the seeds
modal come from one batch.

- **Asking** — the first ask: the header's refresh reads `↻ asking…` and
  each tab's strip and pane hold DIM skeleton tiles and rows where they
  will stand — no "asking…" sentence in the pane; a re-ask keeps the rows
  on screen until the new ones land, the refresh saying it is asking.
- **Answered** — header `Discover` with its
  **controls** at its right (clause 17): the **window dropdown**
  (`Last 30 days ▾`), **more options** (the **seeds modal**) and **↻ new
  seeds** (ask again); the **tabs** `Library · Connected servers`, names
  only (absent sources not drawn), with the DIM tagline `based on songs
  you finished` at their row's right (clause 11); the tab's **album strip** — a row of
  album tiles with their covers that scrolls sideways — at every height
  the GUI draws (clauses 45–49); the **verbs line** with the tab's
  whole-list verbs, right over its table (clause 64); the tab's table, down
  to the room's last row — there is no bottom row (2026-10-16). No state
  line and no seeds line (2026-10-16): the seeds are reached only through
  more options, as on the webapp.
- **Nothing finished yet** (`fallback: true`, the server's random seed) —
  the fallback card above the tabs: what the room needs, the random
  pick it started from, **another pick** and **choose a song…**; the
  connected servers are not asked for a random seed and say so.
- **Scan pending** (`notAnalyzed: true`, or a 403 whose re-ping still
  carries the gate) — the seeds are listed; every pane says the scan has
  not run, with **Check again**.
- **Switched off** (a 403 whose re-ping lost the gate) — the sentence, no
  retry; the nav row leaves with the next ping and the room yields to
  Files.
- **A source that answers nothing** — one DIM line where its rows would
  be; the tab stays.
- **Failed** — the half that failed says "couldn't reach this source"
  with Try again; the other half stands.
- **Keep a copy** (2026-10-19, clauses 100–141) — on the Connected servers
  tab, where the copy gate holds: `c` (or the sheet's row, or the tile's
  list) opens the **Keep a copy** modal on a song or an album; a copy runs
  on the user's own server and outlives the modal; the room marks each
  row and tile with its copy's state in a glyph or a word — never a
  number — and the verbs line's **indicator** (`copying… · ! failed`)
  opens the **Copies** modal, where the numbers live. **Open elsewhere…**
  lists a song's links. Nothing of it is drawn while the gate is closed.

What persists, and what asks again:

| Kept | For how long | Asks again when |
|---|---|---|
| The answer (both halves) | on screen until the next one lands — **never reused**: every visit asks | every visit; the nav row, `G` or **↻ new seeds** while the room is up; a window change; a seeds-modal change; a pin; another pick; Try again / Check again |
| Pins, dropped seeds, random picks passed over | **the visit** — a fresh entry into the room, a server switch or quit lets them go (clauses 13, 14, 21); never stored anywhere | — |
| The window | `GuiPrefs` (`[gui] discover_window`, beside the tab; default 30) | — |
| The tab in front | `GuiPrefs` (`[gui] discover_tab`) | — |
| The plug-in gate (the listing, the destination) | the session — dropped with the answer on a server switch | every visit (clause 101) |
| The rows' copy marks | the answer they were looked up for — a new peers answer drops them at once | every peers answer; a copy ending; Clear finished (clause 111) |
| Copies | the server's (a copy outlives every modal and the room) | the poll while one is live and shown (clause 112); the Copies modal's opening |

Nothing is the server's: the route keeps nothing between calls, and its
random batch is what makes each answer new. A server switch drops the
answer and the visit (the session generation, clause 31).

## Behavior contract

### The row

1. **Gated, absent is absent.** The row is drawn only while
   `capabilities.discovery && capabilities.discovery_taste >= 1` and
   `session.peer` is `None` — a peer's guest has no history on the peer
   (library-rooms clause 23's reason). No placeholder, no grey row; it
   goes with the next ping that drops the flag. When the row goes while
   the room is up (a peer session, a ping without the flag), the room
   yields to Files, as a peer's hidden rooms do. With no session up the
   room shows the offline text every browse room shows.
2. **Its place.** `NavId::Discover` is the **last** of `NAV` (index 12):
   rooms past the digits are the pointer's. The TOOLS group reads TOOLS
   (row 13) · Auto DJ (14) · Sonic path (15) · **Discover (16)**, and
   Settings stays on the content's last row — 17 at 100×24 with the key
   hints on, 18 with them off — so at the 24-row floor Discover and
   Settings never share a row (the canvas's row 17 would). With Sonic path
   absent its row stays empty; rows never close up.
3. **`G` opens it** wherever the Library screen takes keys; the hosted
   screens' key tables (Now Playing, Stats, Admin) are not touched — this
   is a room, not a screen (plan §4.3). `G` does nothing while the row is
   absent, as digit 9 goes dead with the sonic row. **Every visit asks**
   (clause 30) — no earlier answer is reused, and the old rows stand until
   the new land (clause 32). Entering the room afresh lets the last
   visit's pins, drops and passed-over picks go; `G`, a click on the row
   or the header's **↻ new seeds** (clause 17) while the room is up asks
   again keeping them, as does the way back from a pick (clause 14).
4. **The label fits twelve cells** in every locale (the nav column's
   width).

### Seeds — what the server decides, what the room shows

10. **The server picks the seeds** (plan §2.1–§2.2): a **random batch**
    of the distinct songs the user finished, or stopped after half their
    length, counted plays only, in the window, never a song whose latest
    play in the window was a skip — a different batch on every call
    (2026-10-14: "that's actually a feature"); widening to all time, then
    to the play counters, then to one random song from the user's own
    libraries. The room reads no history of its own and sends only the
    window, the pins and the visit's exclusions.
11. **The room says what the answer is based on**, by `seedSource`:
    `history` → "based on songs you finished" (no count — the 2026-10-11
    log); `counters` → "based on
    songs you've played before"; `pinned` → "based on the songs you
    picked"; `random` → "nothing finished yet" — the room's DIM tagline,
    right-aligned on the **tabs row** (since 2026-10-16: the header's
    right is its controls', and beside them the phrase did not fit at the
    design size's 80 cells — see the 2026-10-16 log's review). When
    `widenedFrom` is set the tagline adds "nothing in the last n days —
    all time instead" as its second fact (since 2026-10-16, when the
    state line that carried it left; for `counters` the phrase already
    says where the seeds came from, and the old "from your play counts"
    fact went); the window dropdown names the widening at every width
    (clause 15) and its tooltip leads with the fact in full. The
    tagline's facts drop whole, from the right, never closer than two
    cells to the last tab: at 80 and 100 cells English shows "based on
    songs you finished" (the widened fact beside it from 108 cells, 128 columns), at
    the queue panel's 47 cells it drops.
12. *(The seeds line — "seeds  Title · Artist … and more  edit", the
    `random` marker — left the room on 2026-10-16, the user's "make the
    seeds hidden but discoverable like the webapp": the seeds are shown
    only in the seeds modal, reached through the header's **more
    options**, as the webapp's "More options" reveals its seed picker. The
    modal keeps its `picked` marker; `random`, "and more" and `edit` went
    with the line.)*
13. **The seeds modal** (the header's **more options**, `e`) — a kit modal titled "Based on":
    **PINNED** (each with an unpin), **FROM YOUR PLAYS** (the history seeds,
    checked: the relative last play, "finished" or "listened to" by
    `outcome` — no play count — "not analyzed yet" when `analyzed` is
    false) — the batch the answer was built from — and **DROPPED** (this
    visit's drops that the answer's `excluded` resolved, unchecked; a
    random pick passed over, clause 21, is not one). Space, Enter or a
    click toggles a row; `x` unpins. **Changes apply when the modal
    closes** (Esc, or Done), then one ask: a seed unchecked joins the
    visit's drop list, sent as `exclude` on every ask of the visit (this
    call only on the server — never stored); a dropped one checked again
    ("use again") leaves it. A modal closed with nothing changed asks
    nothing. A fresh visit starts with no drops.
14. **Pins are explicit** — the room never pins on its own. The modal
    offers **Pin the playing song** (only while the playing track is the
    session server's own, not a peer's, not already a seed, and fewer than
    8 pins stand — otherwise hidden, never grey) and **Pin a song…**, a
    capture pick in the sonic room's grammar (its clauses 10–13: the banner
    above the room, a pick consumed, a pick from another server rejected
    with "Pick a track on {server}", Esc returns, re-entry disarms). A pin
    asks when the modal closes, or when the pick lands; unpinning asks
    without it. Pins last for **the visit** and are sent with every ask of
    it; the server never stores them. A changed playing track never
    re-asks.
15. **The window** is the header's dropdown: Last 7 days · Last 30 days ·
    Last 90 days · All time, the current one wearing `•`. It shows the
    window **asked for** (`widenedFrom` when set, else `window`; the list's
    `•` too) — and, where the server widened it, where it went: `Last 30
    days → All time ▾` (`->` on the legacy console), or, short of width
    (clause 17), the window the answer came from, `All time ▾` — never a
    window the answer did not use; while a new choice is out, that choice
    alone. The value stands in a slot as wide as the widest of the four
    windows, so choosing one moves nothing (a widened value may be wider:
    a new answer, not a choice, moves it). `w` opens
    it, `[` `]` step it without the list. A change asks both halves with
    it, and the choice is **remembered locally** in `[gui]
    discover_window` (7, 30, 90 or 0; unset or any other number asks the
    default 30). Every ask sends the window. It is the first of the
    header's controls (clause 17).
16. **Nothing is the server's** (2026-10-14). The route keeps nothing
    between calls: no prefs are read or written, no `remember`, `filter`
    or `staleDays` is sent (they are a 400). The room's memories of its
    own are the window (clause 15) and the tab (clause 40), both in
    `[gui]`.
17. **The header's controls** (2026-10-16 — the webapp's configuration
    bar, mStream `m.js` `discoveryHeadHtml`: "Based on the last [30 days]",
    "More options", the ⟳ "New seeds" button). The header row carries, after
    the room's name (BOLD) — and nothing else: the tagline is the tabs
    row's (clause 11) — right-aligned
    in this order, two cells apart: the **window dropdown** `Last 30 days
    ▾` (clause 15, `w`); **more options**, a DIM text button that opens the
    seeds modal (clause 13, `e`); and **↻ new seeds**, the accent's text
    button — the header's one primary — that asks again plainly, the
    visit's window, pins and drops kept (clause 3, `G`). The `↻` is gated
    like the kit's other glyphs (none on the legacy console, where the
    words always stand). Hover brightens each (BRIGHT + BOLD), a click acts
    on it, the click rects are their labels' cells, and each tooltip names
    its key ("The plays your seeds come from — w", "Choose what this room
    starts from — e", "Ask again around a new batch of seeds — G"). **While
    an ask is out** (either half, or a 403's probe) the refresh reads
    `↻ asking…` in the getting-there colour (GOLD), in a slot as wide as
    the wider of its two words, drawn from the slot's left so its glyph
    stays under the pointer — the state line's old "asking…" (clause 32).
    Where the refresh's words dropped it reads `↻ …`, the glyph keeping
    the two cells after it for the mark at rest too: an ask is never
    shown by the colour alone, and nothing moves.
    **The drop order**, when the row is short of cells (the tagline is
    not on it): the refresh's words first (its glyph stays — the queue
    panel's 47 cells in English), then "more options" shortens to
    "options" (German, French and Russian at 47 cells; Chinese keeps all
    three whole there; the legacy console, with no glyph to keep, goes
    straight from the words to "options"), then a widened window's mark
    gives way to the window the answer came from, `All time` (clause 15;
    every locale but Chinese at 47), then the room's name — whole, never
    drawn over — which the nav row names already (the legacy console at
    47 in German, Spanish, French, Japanese, Portuguese and Russian). The
    three controls never leave the row and never move for anything but
    the width (and a widened answer's wider value): they stay reachable
    at 47 cells in every locale, on both glyph tiers.

### Nothing finished yet

20. **The fallback card** stands between the header and the tabs
    while the server has no history to seed from — learned from the last
    answer asked without pins (`fallback: true`) and kept until an answer
    carries history seeds; the tabs and the Library's rows stay under it,
    around the random pick. It reads, in order: "Finish a few songs and
    this room learns your taste" (BOLD); "Songs you play to the end — or
    most of the way — become seeds. Skips never count."; "Until then,
    around a random pick: {title · artist}" ("around your pick" once a
    pick replaced it); then **another pick** and **choose a song…**. (The
    state line's `• no seeds` · "suggestions around a random pick until you
    finish a song" left with the line on 2026-10-16: the card says it.)
21. **Another pick** (`r`, only while the card stands) re-asks with
    **every random pick the card showed in this visit** in `exclude` — a
    visit-local list beside the drops, never listed under DROPPED: every
    random pick the card showed joins it as it lands, whatever ask drew
    it (`G`, a window change and Retry draw afresh too, but pass nothing
    over themselves), so an earlier pick never comes back, even when a
    chosen song now stands in the card's place — and
    any pin the card stands around is let go (pins alone make a page; the
    server draws a random song only when none stands). The server draws
    its random seed afresh on every call (2026-10-14), never one in
    `exclude`. A fresh entry into the room empties the list. **No pick
    left**: when an answer comes back with no seed — every song the
    server could draw was shown, so it answers `notAnalyzed` with nothing
    to show; pinned or not, since a chosen song the server cannot resolve
    falls back to the random draw too — the room empties the list and
    asks once more (the old rows
    and card standing meanwhile), so the card starts over instead of
    stranding on a false scan-pending card; an empty list asks nothing
    more. **Choose a song…** is clause 14's capture pick; its answer is
    `pinned`, so the connected servers are asked from then on.
22. **A random seed searches the library only** (§2.8): the connected
    servers are not asked (clause 30 — the server would answer `peers:
    null`), and the Connected servers tab holds one DIM line — "Asked once
    you've finished a song — a random pick only searches your library."
    (no `–`: the tabs carry no counts).

### Asking

30. **Two asks, one batch**: **A** `sources: ['library']` — never
    `'network'`, even advertised (the 2026-10-11 log) — with the window,
    the pins and the visit's exclusions and no `limit`, so the server's
    defaults apply; then **B** `sources: ['peers']` (only while
    `federation_discovery` is) — §3.1's "SHOULD", so the library renders
    without the peers' 4 s deadline. Every call draws its own random
    batch (2026-10-14), so B goes out **once A's answer lands**, pinned
    to that answer's seeds alone (the user's pins that resolved are among
    them; one that did not is not sent, so it cannot push a seed out at
    the cap; at most 12, the server's `PIN_CAP`) with `limit: {seeds: n}` their count, so the
    server adds no seed of its own: the Connected servers tab is built
    from the batch the seeds modal shows. The same window and
    exclusions ride it. While A is out, B's tab reads asking. **No B**
    for an A whose seed is the random pick (clause 22), an A with no
    seed at all, an A that failed (B's pane fails with it — clause 83)
    or was refused (clause 82), or an A that is not the latest ask
    (clause 31).
31. **Last ask wins.** Each ask takes a new seq and the session's
    generation; each half's reply carries both back and fills nothing
    under an older seq or another generation (a reply from the server the
    session has left). Each half lands on its own; only the latest A,
    landed once, sends B.
32. **While asking**: the first ask shows the header's refresh as
    `↻ asking…` in the getting-there colour (clause 17) and, in each tab's
    pane, **four skeleton rows** — DIM `░` runs of varied lengths where
    TRACK, ARTIST, ALBUM (ON) and LENGTH will stand (the webapp's four
    skeleton rows) — and no sentence: the refresh is the only "asking…". A
    re-ask leaves the rows standing until the new ones land; only the
    refresh says it is asking.

### Tabs

40. **The tabs** are the kit's, two of them, **names only — no counts**:
    **Library** always; **Connected servers** while `federation_discovery`
    is advertised — a source the ping does not advertise is not drawn at
    all. A `null` half for a non-random seed (the server says this caller
    lacks the source) hides the tab until the next answer, as does an
    answer that searched no servers at all (`searched.peers` 0 and no
    groups: nothing is connected). `←` `→` (while the keyboard's hand is
    not on the album strip, clause 47) and clicks switch; switching asks
    nothing; the pane under the tabs keeps one height whatever the tab,
    save the album strip, which stands only over a tab that has albums
    (clause 45). The tab in front is remembered in `GuiPrefs` (`[gui]
    discover_tab`: `library` · `servers`; a saved `peers`, the tab's old
    name, reads as `servers`, a saved `network` as no memory); a
    remembered tab whose source is gone falls back to Library.
41. *(The state line — `• ready to play`, `• nothing new`, `• asking…`,
    `• waiting for the scan`, `• no answer`, `• switched off`, `• no
    seeds`, "streams through your server" — left the room on 2026-10-16,
    the user's "remove the 'ready to play' line". What it alone said moved
    to the table region's own states: the skeleton while the first ask is
    out (clause 32), the empty lines (clause 80), the scan-pending and
    switched-off sentences (clauses 81–82), the failure line with Try
    again (clause 83); "asking…" to the header's refresh (clause 17); the
    widened fact to the tagline at the tabs row's right and to the window
    dropdown's value and tooltip (clauses 11, 15); the fallback's
    "no seeds" fact to the card, which says it (clause 20). "Streams
    through your server" is **dropped**: a Connected servers row's `[▸]`
    tooltip already reads "Listen through your server — P" (clause 62).
    "No answer" stays only as a cleared wait's failure words, the failure
    line's tooltip (clause 83).)*
42. **The tab row carries no note** (the old "not in your library" / "you
    don't own these" went with the busy page).

### The album strip

45. **A strip of album tiles under the tabs**, above the tab's
    table — the webapp's "Albums from your library" / "Albums from
    {server}" scrollers, without their labels (the tab names the source).
    **Library**: the answer's `library.albums`, best match first (by
    `similarity`; ties keep the server's order). **Connected servers**:
    the tab's playable rows (clause 53) grouped by server + album +
    artist — album and artist trimmed and folded to lower case, a row
    with no album tag joining none — best match first by each album's
    best row, the webapp's `peerAlbums`. Both are capped at **12** (the
    webapp's `ALBUM_STRIP_MAX`; the server sends eight library albums at
    most when no `limit` is asked). Where the strip stands, **the album
    container rows leave the table** (clause 51); a tab with no album to
    show draws no strip and its table takes the rows. While the first ask
    is out the strip holds **four skeleton tiles** — the empty cover
    frame and DIM `░` runs where the title and the artist will stand —
    over the table's skeleton rows (clause 32). A sentence pane (clauses
    22, 80–83) draws no strip. A library answer of **albums and no
    songs** draws the strip and **no table under it** — no header, no
    rule, no empty rows: the strip holds the whole answer.
46. **A tile is 12 cells wide**: the cover 12 cells × 5 rows, then the
    album's title (one line, clipped with the house `…`) and its artist
    (DIM; the album's credit — `album_artist`, else the first of
    `artists` — or the rows' artist on a connected server's album); one
    cell between tiles. **A 12 × 5 box is not square** at the terminal's
    1:2 cell: the picture is fitted inside it the way every cover is
    (`tui::graphics` keeps the art's aspect — a square cover is 10 cells ×
    5 rows, centred, one blank cell either side), while the mosaic fills
    the box (the shared `CoverPane`, as the Now Playing and queue
    mosaics do), a square cover's mosaic about a fifth wider than its
    picture. So a tile whose cover swaps between picture and mosaic (an
    overlay passing over it, the cut tile beside whole ones) changes its
    cover's width, never its tile's: the title and artist stand at the
    tile's left edge either way. The brief fixed the 12 × 5 box; the
    album wall's square 12 × 6 is the alternative should this read badly
    on a pixel terminal. **The cover is real pixels** through the player's
    graphics probe (`tui::graphics` — kitty, sixel, iTerm2) on the very
    path the album wall and the queue panel draw theirs (`cover::Slot`,
    one per cover on view keyed by the art's id, paced by the frame's
    encode budget, the slots refreshed on a resize): the same claim and
    cache as the Now Playing cover (`App::fetch_art_file` — a library
    album's `album_art_file` from `/album-art/<file>`; a connected
    server's album only when one of its rows names art — the row's
    **`albumArt`** (mStream 2026-10-16: the peer's own `album-art` file
    name, `null` for a track with none, absent from an older server; the
    older guessed spellings `art`, `album-art` and `album_art_file` are
    still read after it, harmlessly), taken only as one plain file name
    (`TasteLead::album_art`, the server's `peerArtName` rule) — the
    first row best match first that names one, and then from that server
    by the queue panel's peer reach: the parent's federation art proxy,
    `/api/v1/federation/peers/{id}/art/<file>?compress=l`, or, while the
    peer's own tunnel is up, the peer's `/album-art/<file>?compress=l` —
    the peer's 256 px rendition either way, the original from a peer
    without one — else no art). Where pixels are not drawn the cover is the
    same **▀-mosaic** the Now Playing cover falls back to; an album with
    no art (or none decoded yet) is the **note tile**: the empty cover
    slot's DIM rounded frame with `♪` at its centre (the frame alone on
    the legacy console). The kit's picture rules hold: a cover an overlay
    touched last frame (the window's list, the seeds modal, a tooltip)
    draws as the mosaic for that frame (Over pixels); pictures are
    placed with the frame and go with it — a scrolled strip moves each
    cover with its tile (an encoded picture draws anywhere for free) and
    the cells a tile left are repainted by the next frame, so nothing is
    left behind; sixel's lack of alpha costs nothing because a cover is
    opaque and fitted inside its own box (the owned-ground rule is the
    flattened logo's, whose alpha needs a known ground — covers draw on
    any ground, as the wall's and the queue's do).
47. **The strip scrolls sideways.** The **wheel** over it scrolls by one
    tile a notch, whatever the notch, never past the last view of whole
    tiles; the wheel over the table scrolls the table. While the strip
    overflows, **the last tile shown is cut at the right edge** — its
    cover cut (always the mosaic: a picture cannot be cut), its words
    clipped — so the cut says there is more, as the webapp's strip does.
    Where the whole tiles would end exactly at the edge (the content
    width mod 13 is 0 or 12 — 110 or 111 columns with the queue folded,
    104 or 105 with it open), the last whole tile gives its place up to a
    **half tile** (six cells) so a cut always stands while tiles follow;
    at the strip's end nothing follows and nothing is cut.
    **The strip is a cursor stop above the table** (the list-cursor law,
    clause 57): ↓ or PageDown from rest lands on the first tile in view,
    `←` `→` move the hand one tile and scroll to keep it whole, ↓ or
    PageDown goes to the table's first row, ↑ or PageUp from the table's
    first row comes back to the tile it left (and from rest, with no rows
    under the strip); ↑ on the strip does nothing; Esc stows. Nothing is
    lit on arrival; the keyboard's tile wears the slab on its **title
    line alone** (its artist stays DIM on the ground); the pointer never
    lights a tile — hover brightens its title to BRIGHT + BOLD and shows
    its verbs (clause 48). The lit
    tile is always whole in view: a narrowed room (the queue panel
    opening, a resize) scrolls the strip to keep it, so Enter and `+`
    never act on an album off screen. **`←` `→` switch tabs whenever the
    hand is not on the strip** (clause 40); the tips say which (`← →
    albums` on the strip; `← → tabs` at rest — "↓ pick an album" where the
    strip stands — and on a song or album row). A tab switch starts its
    strip from the left.
48. **A tile's verbs.** A click or Enter on a **library album** opens it
    in the Albums room, its row's way (clause 61); a hovered whole tile
    (not cut, not the keyboard's) shows its verbs at its right edge —
    `[+]` at the right of its title line, the title giving way — and
    `[+]`, `+` or `a` with the hand on it **queues the album**
    (`ApiCmd::TasteAlbum`, clause 61). A hovered **connected server's**
    tile shows a second, `[⋯]` under the `[+]` at the right of its artist
    line (the rows' own `[⋯]` glyph, `[.]` on the legacy console; DIM,
    BRIGHT + BOLD under the hand), the artist — or the copy's state word
    in its place (clause 114) — giving way as the title does: both lines
    clip to the tile's width less four while the verbs show, so nothing
    runs under either. A click on it opens **the tile's list** (clause
    103) — registered after the tile's own click, so it wins; the rest of
    the tile still plays. A library tile has no list and no `[⋯]`. A **connected server's
    album** has no drill here to open into (the room cannot browse a
    peer), so its click or Enter **plays** it — the queue replaced, the
    first song started — and `[+]` / `+` / `a` queue it; either way its
    songs are asked **of that server** through the parent's federation
    proxy (`album-songs` by name and the rows' track artist, a client
    aimed by the command's peer reach), stamped with `Origin { server,
    peer: Some(id) }` (clause 62); a server that will not list the album
    leaves **the rows that matched** (the webapp's
    `discoveryQueuePeerAlbum`), and nothing at all says "couldn't load
    the album". **A compilation** — rows that carry an album artist
    (`albumArtist`, mStream 2026-10-19) — is **one tile** whoever sings
    each song: the strip groups by server + album + the album artist (the
    track artist where the rows carry none), the tile's artist line is the
    album artist, and its songs are asked by the album artist alone
    (`album-songs {album, album_artist}`), so every song of it comes back
    (the 2026-10-19 log). `N` and `P` do nothing on a tile; on a
    **connected server's** tile, `m`, a right click and a click on its
    `[⋯]` open **the tile's list** (clause 103; 2026-10-19 — they did
    nothing before; the `[⋯]` 2026-10-20, for a terminal that reports no
    right click — Apple's), and `c` opens Keep a copy in album focus where
    the copy gate holds. The tile's title has a tooltip ("Open the album —
    Enter"; "Play the album from {server} — Enter"), its `[+]` "Add the
    album to the queue — a", its `[⋯]` the list's verbs as they stand and
    the other ways in ("More: Play album · Queue album · Keep a copy… (or
    right-click the album) — m"; `gui.discover.tile_more_tip`, the ` — m`
    tail with the key hints alone). A library album
    is opened and queued **by name alone**, as its row always was (clause
    61): two albums of one name by different artists (two "Greatest
    Hits") open and queue the songs of both — the strip, showing both
    credits side by side, makes this visible (the log).
49. **The height gate.** The strip's seven rows and the gap under them
    stand only where the table keeps **at least three rows** under the
    strip. Above it stand the header and the tabs, and the blank under
    each **as the height allows** (2026-10-16, the rows the state line, the
    seeds line and their blanks gave back): both blanks from 18 content
    rows, the header's alone at 17, neither at 16 — by the height alone,
    never the tab, so a tab switch moves nothing; under it the verbs line
    and the table's header and rule. So the strip stands from a content
    area of **16 rows** with no card — 2 + 8 + 3 + 3 — which is **every
    height the GUI draws** (its floor is 100×24): the **100×24 floor**
    with the footer on (16 rows: header, tabs, the strip, three songs) and
    off (17: the header's blank back, three songs), and the installer's
    100×30 window (22 rows with the footer on, 23 off: seven or eight rows
    under the strip). The floor trades four of its song rows for the
    album tiles. The fallback card comes first (clause 20): with it whole
    the strip wants **26 rows** (100×33 with the footer off), so under the
    card at the usual sizes the albums are the table's first rows — the
    one place the album container rows (clause 51) still show. With the
    queue panel open (47 cells) the strip shows three whole tiles and the
    fourth cut. **A resize across the gate keeps the keyboard's hand on
    what it was on**:
    a song row stays that song as the album rows come or go above it, an
    album row becomes its tile where the strip stands and the tile its row
    again where it does not (under the card, short of 26 rows); what has
    no place on the other side (a connected server's tile) puts the hand
    down.

### The table

50. **One kit table per tab**, in the server's order (most similar
    first — no rank number, **no MATCH meter, no percentage, no LAST
    PLAYED**): Library **TRACK · ARTIST · ALBUM · LENGTH**; Connected
    servers **TRACK · ARTIST · ALBUM · ON · LENGTH**. TRACK starts two
    cells in, where the playing row's `▸` stands (clause 55).
51. **Library rows, albums first** (§2.6): where the album strip does not
    stand (clause 49 — under the fallback card short of 26 rows, since the
    strip stands at the floor from 2026-10-16), the server's albums,
    then its songs; where it stands, the songs alone (clause 45). An album row's TRACK is the album's name with a DIM `album`
    beside it, its ARTIST the album's credit, its ALBUM and LENGTH empty —
    no song count, no "never played". A song row's TRACK is the title (the
    file name when there is none); ARTIST, ALBUM; LENGTH is m:ss. A
    missing value is the kit's DIM `—`.
52. *(Network rows — left the room on 2026-10-11; the later Explore room
    draws the leads.)*
53. **Connected servers rows are one list**, in the server's order, no
    group lines: only rows that play now — from a server that answered
    (`reachable`) on the same sonic model (not `mismatched`) and that
    carry a file. ON is the server's name. An offline or another-model
    server's rows are simply not shown.
54. **Narrow widths**: TRACK keeps at least 24 cells; ARTIST and ALBUM
    take a share each (TRACK keeps half beside both, two thirds beside
    one) and ON its name's width up to 16. When TRACK would get fewer
    than 24, **ALBUM goes first, then ON, then ARTIST**; LENGTH never
    goes. With the queue panel open (47 cells) the Library table is
    TRACK · ARTIST · LENGTH and the Connected servers table TRACK · ARTIST
    · LENGTH. Widths are cells (`kit::width`; a CJK title is two cells a
    glyph).
55. **The playing track's row is marked** as in every room. On a
    connected server's row the same two-cell LEAD also carries its copy's
    mark (clause 113, 2026-10-19); the playing `▸` wins the cell.
56. **A row names its seed**: a Library or Connected servers row whose
    `seedHash` is one of the answer's seeds has a tooltip "sounds like
    {title · artist}" — a pointer enhancement; nothing lives only there.
57. **The list-cursor law** (docs/ui-kit.md): the room opens with no row
    lit; ↓ ↑ PageUp PageDown pick the cursor up (at the top on ↓ and
    PageDown — the album strip where it stands, clause 47 — the bottom on
    ↑ and PageUp; the room has
    no resting place of its own) and walk it;
    every line is a row; Esc stows it; a click acts on its row and
    stows it; Enter, `a`, `N`, `P`, `m` do nothing while it is stowed.
    Hover brightens, never selects.
58. **The wheel scrolls the list under the pointer** — the table, the
    album strip (one tile a notch, clause 47), the seeds modal's rows — and the kit scrollbar stands at the table's right
    while the rows overflow. The window dropdown's four choices always
    show whole, so the wheel over it moves nothing, and not the table
    under it either.

### Verbs

60. **Library song rows** play and queue like any track the session
    browses, with the session's origin (`App::origin()`): a click or
    Enter plays the row and queues the tab's songs after it (play from
    here, library-rooms clause 14); the hover verbs `[▸]` play now, `[»]`
    add next, `[+]` add to the end, `[⋯]` the track-actions sheet (its
    contract's verbs); `P`, `N`, `a`, `m` on the keyboard; a right click is
    the sheet.
61. **Library album rows are containers** (library-rooms clause 5) with
    one hover verb, their play control: `[+]` (and `a`) **queues the
    album** — its songs asked by name, as the drill asks them
    (`ApiCmd::TasteAlbum`), appended when they land with the session's
    origin, queue all's way (say how many, start an empty idle queue); a
    failed ask says "couldn't load the album". A click or Enter opens the
    album's tracks in the Albums room (as does its strip tile, clause 48) — the wall's drilled album view, its
    Back leading to the wall — and `G` brings this room back, asking again
    with the old rows standing until the new land (clause 3).
62. **Connected servers rows play through your server**, as library rows
    play: `[▸]` Listen (play now), `[»]` add next, `[+]` add to the end,
    `[⋯]` the track-actions sheet; a click or Enter plays the row and
    queues the tab's rows after it; `P`, `N`, `a`, `m`; a right click is
    the sheet. The queue
    row's origin is `Origin { server: <the session's server>, peer:
    Some(<the row's peer id>) }`, so it reaches its bytes the multi-server
    contract's way (clause 27: the parent's federation proxy,
    `/api/v1/federation/peers/{id}/stream/…`, or the peer's own tunnel
    when a direct ticket is up), and its play is reported to the parent
    with `peerId` and the snapshot (play-reporting clause 7). The sheet
    opens with that same peer origin — the sheet a queued peer row opens —
    so it offers only what a peer's track can do: no stars, no playlist
    row (a peer path has no rating or playlist here, plan §1); its **Keep a
    copy…** and **Open elsewhere…** rows are clause 102's (2026-10-19).
63. *(Network rows and their lead menu — FIND ELSEWHERE through the
    `links` plug-ins, THIS LEAD's copy — left the room on 2026-10-11: they
    cannot play now. The later Explore room takes them; git history keeps
    the code.)*
64. **The whole-list verbs** stand on the **verbs line**, from the room's
    left edge, right over the tab's table — under the album strip where it
    stands, under the tabs (and their blank) where it does not — the webapp's
    verbs under its songs' header, ahead of the rows (mStream `141e1a9c`,
    the 2026-10-16 log). **Library**: **▸ play · + queue all · ⇄ shuffle ·
    save as playlist…**; **Connected servers**: **▸ play · + queue all ·
    ⇄ shuffle** (the webapp's servers column offers Queue all alone; play
    and shuffle stay, built here before it, and there is no save: a
    connected server's songs are not this server's). Play, queue all and
    shuffle are the browse bar's three with its meanings (browser-top-bar
    clauses 10–12: replace and play, append and say how many, shuffle the
    order once) over the tab's songs (albums are containers) or its rows
    in order; **save as playlist** opens the house name prompt — the
    sonic room's (`App.sonic_playlist_name`, drawn by `sonic::draw_modals`,
    typed through the App's line editor), empty as the webapp's modal
    opens — and Enter saves the Library tab's songs, in the server's
    order, as a playlist on this server (`ApiCmd::SavePlaylist`, the
    sonic path's save; the files ride `App.playlist_save_files`); Esc
    cancels; an empty name saves nothing. The line wears the bar's
    grammar: **play the one accent**, the rest DIM text buttons, each
    BRIGHT + BOLD under the pointer and acting on a click, `⇄` gated on
    the legacy console, the key in each tooltip; squeezed, it drops verbs
    from the tail (save first, play last). Verbs appear only while the tab
    has rows (the bar's law); the line keeps its row while they are
    hidden, so nothing moves. Keys `p`, `A`, `S`, as in the browse rooms,
    and **`s`** for save as playlist — the sonic room's save key — only
    where the tab's songs can be saved; elsewhere `s` keeps its global
    meaning (shuffle mode). The line stands under the strip, not over it,
    as the webapp's does: its album strips draw first and its songs block
    — label, verbs, rows — after them (`m.js` `discoveryLibrarySongsHtml`),
    so the verbs sit with the songs they act on.
65. *(The library filter's checkbox, "Include songs I've played lately",
    and its `f` left on 2026-10-14: the library half is the server's fixed
    rule — songs you haven't played lately, never or more than 90 days
    ago — and the bottom row kept the whole-list verbs alone, until they
    moved to the verbs line on 2026-10-16 and the bottom row went.)*
66. **Plays started here are ordinary plays** — `manual`, `shuffle` or
    `autodj` as the play-reporting contract decides; there is no
    `discovery` source in v1 (plan §6, question 8).
67. **The room never touches the TUI's Discover state** — `App.discover`,
    its drill and seed, `App.now_discover` and `Capture::Discover` read
    the same after any verb here.

### Connected servers, not grouped

70. *(The per-server group lines — status, online / not answering, song
    counts, last seen, "another sonic model" — left on 2026-10-11: rows
    from an offline or another-model server are simply not shown, clause
    53.)*
71. *(Group lines are gone, so nothing to skip.)*
72. **Nothing connected** (`searched.peers` 0, no groups): the tab is not
    drawn (clause 40).

### When a source says nothing, or the server says no

80. **Each source explains itself in one DIM line** where its first row
    would be, under the table's header and rule — no number; the tab
    never disappears for being empty (clause 72 aside). Library: "Nothing
    new in your library sounds like these songs." (no button since
    2026-10-14). Connected servers: "Nothing from your connected servers
    right now."
81. **Scan pending** — `notAnalyzed: true`: the seeds modal lists the
    seeds; every tab's pane reads "This server hasn't analyzed any
    music yet — suggestions appear once the discovery scan has run." with
    **Check again** (asks again).
82. **A 403 re-pings before it names a reason** (the sonic-path contract's
    clause 40, same probe, reading this room's gate): the gate still on →
    clause 81's sentence and Check again (the index is not there yet); the
    gate gone → "Sonic discovery has been switched off on this server."
    in the pane (no state line since 2026-10-16), no retry, and the row
    goes with the next ping (clause 1). A failed probe leaves no "asking…" standing. Entering
    the room while the probe is out starts the visit afresh but asks
    nothing: the probe's verdict lands first.
83. **Transport and server errors** say "couldn't reach this source" (the
    webapp's words) in the warning colour, with **Try again**, in the
    panes of the half that failed — the line alone says so since the
    state line left (2026-10-16) — and the other half's rows stand, as do
    the old rows of the half that failed.
    A failed A fails B too (B waited on it, clause 30); Try again asks
    both.
    The server's own words (a status code among them) are the line's
    tooltip, never on the page; a wait an error cleared reads "no answer"
    there.

### The floor

90. **At 100×24**, footer on or off, the nav column holds every row
    (clause 2) and the room holds, top to bottom (2026-10-16): the header
    with its controls, a blank with the footer off (none with it on), the
    tabs, the album strip and its gap where the tab has albums (clause 49),
    the verbs line, the table's header and rule, the rows down to the
    room's last row — three under the strip, footer on or off; eleven
    where the tab has no album (both blanks give way at 16 rows, one at
    17) — and no bottom row. **No state line, no seeds line.** The card
    (clause 20) pushes the table down and stands whole at the floor (the
    header, the card and its blank, the tabs, the verbs line, the table's
    header, rule and three rows at 16); only in a room shorter than the
    GUI's floor does it give up its blank line and its body sentence first
    (whole from 14 rows; the title, the pick and its two actions stay), so
    the table keeps at least its header, rule and one row, and scrolls.
91. **With the queue panel open** (47 cells at 100 columns) the tabs
    row's tagline drops (clause 11), the header's refresh keeps its glyph
    alone — "options" for "more options" in the longest locales, `All
    time` for a widened window's mark (clause 17) — the tabs themselves
    have nothing to drop (clause 42), the table narrows by clause 54, and the verbs line
    drops its verbs from the tail by the bar's rules (save as playlist
    first: English keeps play · queue all · shuffle there) — no control
    moves for anything but the width.

### Keep a copy, Open elsewhere and the Copies modal (2026-10-19)

The canvas "Discover plug-ins · phone & terminal" draws every surface
here cell for cell (boards `TermRoom`, `TermEntryPoints`, `TermKeepModal`,
`TermKeepStates`, `TermCopies`; the vocabulary on `Main`); these clauses
are its rules. The sample data throughout is the stage's: Stage Peer B
holds Boukmanflow's *Borrowed Tapes* (2019) — 01 Rooftop Cypher, 02 Late
Train Verse, 03 Corner Store Loop — Stage Peer C holds Rooftop Cypher of
it, the library three other Boukmanflow albums, the destination `copies`
with `{{ARTIST}}/{{ALBUM}}`.

**What it is, and whose**

100. **Keep a copy copies a connected server's song or album into a
     library on the user's OWN server** — the `federation-copy` plug-in,
     run there as a job; **Open elsewhere** lists the song's links — the
     `links` plug-in. Both act on **Connected servers rows and tiles
     only**: never a Library row (it is already yours), never the Network
     (not in this room). Every plug-in call goes to the session's own
     server, never to a connected one through the parent's proxy or over
     its own tunnel (`Client::send_plugin` refuses a peer's client), and
     sends the row's **raw taste JSON exactly as it came**
     (`TasteLead.raw`): the server's schema strips what it does not take
     (`albumArt`, `similarity`, `seedHash`, `genreTags`). **No previews**
     (the song already plays in full) and **never Folder** (a row's path
     names a file, and a folder key built from it never matches).
101. **The copy gate — hidden, never grey.** Every Keep a copy entry
     point (the sheet's row, `c`, the tile's list's row, the tips' "c keep
     a copy", the indicator and `C`) is drawn only while **all five**
     hold: (1) the ping's `discoveryPlugins`; (2) `GET
     /api/v1/discovery/plugins` lists `federation-copy` with `acquire`
     and the scope (`scopes`; an older server that lists none runs the
     song alone); (3) its `jobs.allowed`; (4) the account may upload (the
     ping's `noUpload` false — `Capabilities.uploads`; a ping that does not
     say grants nothing); (5) `GET …/collection/destination` names one
     (`destination` non-null: a library the account may put files into).
     A peer session asks nothing (the room is hidden there, clause 1).
     **Asked with every visit** (`App::taste_open`): the listing, then —
     only behind the copy plug-in with jobs allowed — the destination
     (`ApiCmd::KeepGate`), read again with every Keep a copy that opens
     (clause 105); a server switch drops both with the answer; a failed
     ask hides everything until the next visit asks again — and a read
     or a start's 409 that finds nowhere closes the gate (clause 147). **Open
     elsewhere… has its own gate**: the flag and a plug-in with the `links`
     capability listed — no jobs, upload or destination right.

**Ways in**

102. **On a connected server's row.** The track-actions sheet (`[⋯]`,
     `m`, a right click; clause 62) gains, for a connected server's song
     only: **Keep a copy…** — with a DIM `c` after its label, its key from
     anywhere in the sheet — in the slot Add to playlist holds on your own
     songs (after Add to end of queue), while the copy gate holds; and
     **Open elsewhere…**, the sheet's last row (it leaves the app), while
     the links gate holds. The sheet's badge row for such a song reads
     "from Stage Peer B   • online" (the dot OK, the words DIM; no stars —
     a rating is your own server's). **`c`** on the held row — else the
     hovered one — opens Keep a copy on that song. The sheet is 66 × 14,
     centred; ↑ ↓ rows, Enter runs the row, Esc closes; hover BRIGHT +
     BOLD; covers the sheet touches draw as the mosaic, its own 6 × 3
     cover stays pixels. **The sheet pins its row** — its server and its
     file (`KeepPin`), never its place: a peers answer can land while the
     sheet is open (the old answer stands until the new one does, clause
     3), and Keep a copy…, Open elsewhere… and the badge row still act on
     and name the sheet's song wherever it now stands; once the new answer
     leaves it out, both rows go (hidden, never grey — what they would
     send is gone), except a links list already asked, which hangs on, its
     Try again asking again for the row it was first asked for.
103. **On a connected server's tile.** A click on the hovered tile's
     `[⋯]` (clause 48 — the left click's way in, for a terminal that
     reports no right click, Apple's), a right click on the tile, or `m`
     on the lit tile, hangs **the tile's list** from the tile's first
     cell, one row under it — right under the `[⋯]`; opened by the
     pointer no row is lit, by `m` the first — the kit's context list (docs/ui-kit.md,
     Context list): an ACCENT `frame_at`, `Clear` beneath,
     overlay-registered, no title, no `[X]`, 24 cells wide, kept inside
     the room — with **Play album** (Enter or a click on the tile does the
     same), **Queue album  a**, **Keep a copy…  c** (album focus) and
     **Open on {server}** (the album as that server holds it: the session
     moves to that connected server — its saved entry, Settings › Servers'
     own switch — the Albums room opens, and the album opens there once
     that server answers; hidden while the server is not saved or is
     parked). ↑ ↓ rows
     (the accent slab), Enter picks, `a` and `c` act from the list too;
     Esc, `m` again or a click elsewhere closes and keeps; hover BRIGHT +
     BOLD. While the list hangs the strip's tooltips stand down: the hand
     that clicked the `[⋯]` (or right-clicked the tile) rests where it was,
     and its tip, drawn over everything, would cover the list it opened. The list keeps Play album, Queue album and Open on {server}
     whatever the gate; Keep a copy… stands only while it holds and the
     tile offers a choice (clause 104 — a tile with no artist has none).
     **`c`** on the lit or hovered tile goes straight to Keep a copy in
     album focus. The list **pins its album** (its server, album and
     credit): a new answer moves the list with its tile, and folds it once
     the album left — no verb of the list ever acts on another album. The
     tile's hover verbs are `[+]` (queue) and `[⋯]` (this list); a click
     anywhere else on it plays (clause 48). **In Apple Terminal** a click
     arrives as an instant pair and the physical release sends a second
     pair, the **release echo**, which the player swallows whole for every
     click (docs/ui-kit.md, behavioral rules): the `[⋯]`'s click opens the
     list and it stays — the echo neither closes it on its own catcher nor
     picks the row under the hand — as a row's `[⋯]` opens its sheet and
     the sheet stays (clause 102), and a click on the tile plays it once.
     **What each sends**:
     a row's job sends its row; a tile's sends its **best row** (the first
     of its rows, best match first) with scope `album` — or the artist
     scopes — and **never a song job** (the server would take it and
     fail).

**What it copies**

104. **The choices** (`app::keep::keep_choices`), each only where the copy
     plug-in lists its scope: **This song** — on a row; a tile is album
     focus and has none. **Album** — the row names its album and its credit
     (`albumArtist`, else the track artist): with neither the peer's
     album-songs answers every album of that name by anyone. **Every album
     by {artist}** — the row names its track artist; never from a
     compilation's tile (its credit is not the track artist's, so the job
     would copy someone else's albums). **Only the albums you don't have**
     is the artist choice's checkbox (clause 107). **Compilations** (the
     user's 2026-10-19 decision): a row carrying `albumArtist` sends it
     unchanged, and the server copies the whole compilation. Never Folder.
105. **Where it lands — shown before the first copy, and changed here.**
     The LANDS IN line reads the destination's library, its base folder's
     names and the layout rendered from the subject's tags, `›` between —
     `copies › Boukmanflow › Borrowed Tapes` (`KeepView::landing`,
     `Landing::of`, the server's `resolveLayout` mirrored: a variable with
     no value drops its segment) — the library BOLD, the names TEXT, `›`
     DIM — wrapped at a crumb onto a second row where it outgrows its 52
     cells (` ›` ending the first; the second clipped at the right, the
     whole path then the rows' tooltip; never cut from the left, so the
     library always shows) — rendered per scope as the webapp's
     `scopeTarget` does: the song
     and the album by the song's tags (ALBUMARTIST the album's credit, so
     a compilation stays in one folder); **an artist copy** with the track
     artist as ARTIST and ALBUMARTIST, the first folder that names
     `{{ALBUM}}` or `{{YEAR}}` and every folder after it becoming **one
     folder per album** in DIM — a hint, not a folder (a layout with no
     album folder adds nothing); only-missing taking exactly one album
     names that album instead, with its year (the listing's `year`, else
     its `year_min`). **A variable each song fills is a slot** (the
     2026-10-22 rule, the same on the webapp and the phone): the server
     files every song by its own file's tags (destination.js
     `tagsForLayout`) — `{{ARTIST}}`, `{{YEAR}}` and `{{GENRE}}` per
     song, `{{ALBUM}}`, `{{ALBUMARTIST}}` (the album's credit, its
     albumArtist else its artist) and `{{PEER}}` the same for every song
     of an album — so LANDS IN never promises one folder the songs will
     not share (`KeepView::scope_tags`, `LayoutTags::slots`,
     `Run::Slot`). Each song of the album's listing
     (`KeepFacts::album_songs`, a `KeepSong` each) is read as the copy
     files it: its **artist the ARTIST tag as written** — the listing's
     `artist-display` ("Boukmanflow feat. ALM"), else its `artist` (the
     primary artist alone, mStream V73) — its year, and **its genre only
     where the listing names exactly one** (the peer's scanner splits a
     tag on `,` `;` `/`, the copy files by the tag as the file has it:
     a "Hip-Hop/Rap" lists as two and lands in "Hip-Hop-Rap"; the
     webapp's `songGenre`). **The song**: its own values — ARTIST its
     listing row's (found by its path) where the listing has it, else
     the row's; GENRE that row's genre, else a slot. **The album**:
     ARTIST and YEAR the value only where every song of the listing
     files under the same one (cleaned as the server cleans a folder
     name; a song with no artist under the album's credit — the
     listing's `fallbackArtist`; a song with no year under none, its
     folder dropped, never the row's year: the server's album songs
     carry none to fall back to) — before the listing answers (or where
     it failed, or is empty) only where the row is no compilation (no
     albumArtist, or the row's artist by `name_key`), else a slot; GENRE
     a slot unless every song names the same one; ALBUMARTIST the
     credit (albumArtist, else the row's artist) whatever the songs'
     artists. **The artist scopes**: as above, GENRE before the cut a
     slot. A slot stands **inside its folder's own words** — DIM
     `‹artist›`, `‹year›`, `‹genre›` (the locale's word, `< >` on the
     legacy console), never trimmed, dropped or merged with its
     neighbour — and the path's tooltip carries **one note**, the
     webapp's and the phone's words: "‹artist› comes from each song's
     own tags, so these songs can land in more than one folder." (the
     slots it shows in the path's order, ", " between, "come" for more
     than one; with a genre "…more than one folder (a song with none
     leaves that folder out)."; a song's copy alone "‹genre› comes from
     the song's own tags (a song with none leaves that folder out).") —
     the 66-cell LANDS IN has no row for it, so the rows' tooltip holds
     it, after the whole path and " — " where the rows clip it. The same everywhere a place is drawn before its
     files land: LANDS IN while choosing, starting (the place sent) and
     live (the pin), "retry adds to" / "copy again adds to", the
     picker's PREVIEW; an ended copy's folder its files landed in is
     real and carries none. Under it the **SOURCE** row: DIM "saved
     for your account" (`source: user`) or "the library's default", and
     **`[ Change… ]`** right-aligned (cols 53–63; DIM, hover BRIGHT +
     BOLD, the keyboard's stop the selection slab BOLD; tooltip "Where
     files land — d"; legacy `[ Change... ]`), which — or `d` from
     anywhere the choice stands, another server's copy beside it
     included, Enter or Space on the stop, a click — opens **Where files
     land** (clauses 142–148). **The destination is read again with every
     Keep a copy that opens** (`App::keep_dest_read`, not once a visit):
     the last place known draws at once and the answer replaces it; a
     failed read keeps what is drawn. **In a job's stages** LANDS IN reads
     the place the copy is pinned to — the start's while it is out, the
     job's `params.destination` while it is live (a job from before the
     pin: the account's; pinned to nowhere: no row) — and once it ended
     the folder its files landed in (the webapp's `landedPath`: the folder
     the copied files share, else where the library already had them),
     else the place it ran with (`result.destination`, else its pin) —
     **never the account's place now** (the 2026-10-21 incident: a window
     showing a stale place put five stage test songs into the user's real
     library). No SOURCE row and no Change… there. **Retry and Copy again
     land at the account's place now** (clause 145): where that is not
     the place the copy ran with (its `result.destination`, else its pin)
     or no such place is known, the row under LANDS IN says so first —
     DIM "retry adds to" / "copy again adds to" and the place, drawn and
     wrapped as LANDS IN (the webapp's `retryPath`).

**The Keep a copy modal**

106. **The modal** — the kit's, ACCENT: 66 cells wide (at least 44, at
     most the area less 2), centred, 16 rows (17 in its tallest common
     state; it anchors as if 17 so its top holds — row 6 at 100 × 30,
     row 3 at 100 × 24 — and it grows downward), under the 22-row cap at
     100 × 24. Rows: the BOLD ACCENT title **Keep a copy**
     with `[X]` (DIM, hover BRIGHT + BOLD, "Close — Esc"); a blank; the
     header — a 6 × 3 pixel cover, the title BOLD (the song, or the album
     in album focus), a DIM byline ("Boukmanflow · Borrowed Tapes · 2019 ·
     from Stage Peer B"; a tile's "Boukmanflow · 2019 · from Stage Peer
     B") and "• online"; a blank; the DIM label **WHAT** and the radios
     with their DIM **sizes** (`ChoiceSize`): "0:08 · mp3" (the row's
     length; the format by the file's extension), "3 songs" (the album's
     songs on the connected server, asked with the copy's own selectors —
     a compilation by its album artist, else by its track artist and
     year), "1 album · you have 3" (the connected server's albums of the
     artist, the library's) — DIM `…` while they are asked (`Fact::Asking`),
     each read optional (`ApiCmd::KeepFacts`); the checkbox (clause 107); a
     blank; **LANDS IN** and its **SOURCE** row (clause 105); a blank (or
     a refusal's GOLD rows, clause 108); the buttons bottom-right —
     a DIM `[ Cancel ]` (it closes; nothing is copied before Add) and the
     ACCENT BOLD primary, whose verb follows the choice: **Add song · Add
     album · Add artist · Add 1 album** (`KeepVerb`). Focus opens on the
     preselected radio — This song from a row, Album in album focus. **The
     keyboard's stop**: a radio's or the checkbox's label ACCENT BOLD (the
     chosen radio's `(•)` OK and its label BOLD where the hand is
     elsewhere), a button the selection slab; where a job's outcome stands
     the hand rests stowed and Enter means the primary. Keys:
     ↑ ↓ walk the radios, the checkbox and the buttons (← → between the
     buttons); Space or Enter picks a radio or ticks the checkbox; Enter on
     the primary starts; `d` opens Where files land; Esc, `[X]` or Cancel
     closes. The stops walk radios › checkbox › Change… › Cancel ›
     primary. A locale whose WHAT
     or LANDS IN outgrows the canvas's ten cells moves the values' column
     right, two cells past the wider label. Mouse: click a radio, the checkbox or a button; hover BRIGHT
     + BOLD. The room beneath is inert; the strip's covers the frame
     touches redraw as the mosaic, the header's cover stays pixels.
107. **"Only the albums you don't have"** (`[ ]` / `[✓]`) stands under
     Every album by when the plug-in runs `artist-missing`, the library
     holds some of the artist's albums and the connected server lists one
     of the artist's own **that the library lacks by NAME** (folded as the
     server folds them, `name_key`; an album counts as owned when any of
     its songs is here; an album they only appear on is never theirs —
     `artist_ownership`), never by comparing counts; or while such a copy
     still wants the user (live, failed, stopped). Hidden otherwise —
     including while the sizes are asked. Ticking it picks Every album by
     (scope `artist-missing`) and the primary reads **Add 1 album**;
     picking This song or Album unticks it. A partly kept album is
     Album's to finish: it skips the songs already here.
108. **Starting, and refused at start.** Add sends the job
     (`ApiCmd::JobStart`) with **the place LANDS IN drew** (clause 145):
     the WHAT block folds to one line ("WHAT  This
     song · 0:08 · mp3"), the step row reads **starting…** (ACCENT) and no
     button stands until it answers; the frame keeps its height. A refusal
     brings the choice back with **"couldn't start: {the server's words}"**
     in GOLD in the row above the buttons ("Uploading Disabled"; 409 "someone
     else on this server is already getting this — try again in a
     moment" and 429 "you have 20 jobs queued or running — wait for some
     to finish (the limit is 20)" wrap to a second row); the choice stays
     editable and Add stays (`KeepStage::Refused`). **A place no longer in
     force** (409 `destination_changed`, clause 146) starts nothing:
     LANDS IN redraws from the answer at once and one GOLD line reads
     "where files land changed · check the path above and add again".
     **Another server's
     live copy** of the same album or artist (the start's `200 created:
     false` naming another server, or one already held): the radios stay,
     the primary is hidden — never grey — and two rows read "Stage Peer C
     is already copying this album — you can add Stage Peer B's once it
     ends"; the primary returns in their place when that copy ends
     (`KeepStage::Choose { other }`).
109. **Following a copy** (`KeepStage::Job`). Once a job exists the
     choice folds to one WHAT line (the scope in words and its size: "This
     song · 0:08 · mp3", "Album · 3 songs", "Every album by Boukmanflow",
     for only the missing ones "… · 1 album") and LANDS IN reads the
     place the copy is pinned to, then where it landed (clause 105); then
     the kit's
     **three-line scan widget**: **queued** — "waiting for a free slot",
     an all-DIM bar `▱▱▱▱▱▱▱▱▱▱` with no percentage (never a fake 0%),
     "starts on its own when the copy ahead ends"; **running** — the
     plug-in's step (`statusText`, "song 2 of 3 · Late Train Verse",
     TEXT), the ten-cell bar (`▰` OK / `▱` DIM) with the percentage DIM
     beside it, the DIM counts ("1 copied · 0 failed", from
     `result.counts`, never the capped lists); a DIM **[ Cancel copy ]**
     (stopping is not the way forward) — `x` or a click cancels
     (`ApiCmd::JobCancel`); **stopping** — "stopping…", the bar holds, no
     button until the job ends (a 409 on the cancel means it had
     finished: the outcome replaces this, clause 133). **Esc or `[X]`
     closes and the copy keeps running** on the server; the verbs line's
     indicator opens it again. Polled about every 1.5 s while the modal is
     open (clause 112). The modal opens straight onto this when the
     preselected scope's copy from **this** server already stands (a
     reopened row reads its copy's real state); another server's finished
     copy is not this one's — the choice, idle. **A wider copy speaks too**
     (the user's 2026-10-20 report: only-missing had copied the album and
     Album still offered Add album): with no copy of its own that is live
     or newer (finish, else update time; else id), the newest finished
     wider copy from this server opens the choice on what it recorded for
     it (`opening_phase`, `covered_view`, `KeepPhase::Covered`) — **Album**:
     a done artist or only-missing copy read through `job_for_album`, and
     only where that view marks the row ("in your collection · 1 copied",
     "already yours"; nothing for a guest spot, an album never walked, a
     failed or cancelled run); **This song**: a done album copy, or a done
     artist copy's walk of the row's album, whose lists name the song's
     file on its server (`from`) — copied (its landed file, Play your
     copy), skipped `owned` (already yours), `exists` (a file in the way);
     a song the lists do not name (they stop at 500), or one a failed or
     cancelled run names, is never claimed.
110. **How it ended** — "done" is not "kept" (clause 120); each with its
     buttons, the primary on the right:
     - **one song copied** — "✓ in your collection" (OK) and the new
       file's path (clipped from the left) · `[ Add to queue ] [ Play
       your copy ]` — the user's own file, the session's origin;
     - **an album or artist copied** — "✓ in your collection", the DIM
       counts ("1 copied · 1 already yours · 1 failed · 2 albums left
       out"; the failed part GOLD) and "Copy again takes only the gaps" ·
       `[ Close ] [ Copy again ]`;
     - **already yours** (nothing copied, all already here) — "✓ already
       yours" (DIM) · "your library already has this song · nothing was
       copied" · `[ Close ]`;
     - **a file in the way** (`skipped: exists`) — "skipped · a file is
       already at that path", the path, "nothing was overwritten" ·
       `[ Close ] [ Retry ]`;
     - **stopped** — the reason in GOLD ("Stage Peer B's transfer limit
       stopped it" · "Stage Peer B is serving too many streams right now"
       · "Stage Peer B went away") and DIM counts ("1 copied · 2 not
       copied · try again later" / "… in a moment") · `[ Close ] [ Retry ]`
       — Retry takes only the rest;
     - **refused** — "Stage Peer B does not allow copies with your
       server's key" (GOLD) · "nothing was copied · playing it still
       works" · `[ Close ]` — no Retry: it would meet the same refusal;
     - **failed** — "copy failed: {job.error}" (GOLD) · "nothing was
       copied" · `[ Close ] [ Retry ]`;
     - **cancelled** — "cancelled · 1 copied" (DIM) · "the other 2 stay on
       Stage Peer B" · `[ Close ] [ Start again ]` — Start again brings the
       radios back;
     - **nothing to copy** — "✓ nothing to copy · you have it all" ·
       `[ Close ]`; an artist who only appears on the connected server's
       albums: "nothing of theirs to copy" · "Boukmanflow only appears on
       other artists' albums there" · `[ Close ]`.
     Retry and Copy again start the same copy again — the job's own
     recommendation and scope (`App::keep_again`); where a wider copy
     speaks for the choice (clause 109), the choice's own copy of the
     modal's subject. Keys: Enter the
     primary, ↑ ↓ the buttons, Esc closes.

**Marks, lookups and the poll**

111. **Lookups** (`POST …/plugin-jobs/lookup/batch`, `ApiCmd::JobsLookup`)
     — every Connected servers row's raw JSON, unchanged, a hundred a call
     (`LOOKUP_BATCH_MAX`, put back together in order) — after **every
     peers answer** (Try again's included) once the gate's listing says
     copies run here (the copy plug-in listed, jobs allowed), when that
     listing lands over rows already on screen, **after a copy ends** and
     after **Clear finished**. A row keeps the keys its lookup answered —
     the song's, the album's, the artist's, the only-missing one; **never
     the folder's** — and the newest copy per scope under them
     (`RowJobs`); the poll's, a start's and a cancel's jobs are folded in
     by key, the newer replacing. A new peers answer drops the old rows'
     marks at once (they are other songs); a lookup made for an answer
     since replaced fills nothing. Library rows are never looked up.
112. **The poll** (`GET …/plugin-jobs`, `ApiCmd::Jobs`) runs **only while
     a copy is live** (queued, running, a start out): about every **1.5
     s** while the copy's own surface shows it — the Copies modal, or a
     Keep a copy modal whose own copy moves (its start out, the job it
     follows or one of its subject's slots live: `App::keep_flow_live`; a
     modal still choosing over someone else's copy keeps the room's beat)
     — every **4 s** while the room alone shows it (`KeepWatch`, set by
     the GUI each frame),
     never away from the room; one ask at a time; each failure doubles the
     wait, up to **60 s**, and an answer forgives them (`keep_backoff`).
     The list is also asked once when the gate opens (the indicator) and
     when the Copies modal opens. A copy seen live and then finished is
     news for the status line (clause 118) — a page load is not.
113. **The row's mark** — one BOLD glyph in the two-cell **LEAD** before
     TRACK (`App::keep_row_mark` → `JobMark`), LENGTH untouched (clause
     54): **◷** waiting (DIM) · **↓** copying (ACCENT) · **↓** stopping
     (DIM) · **✓** in your collection (OK) · **✓** already yours (DIM) ·
     **!** failed, stopped or refused (GOLD) · **×** cancelled (DIM) ·
     nothing for "nothing to copy" and for none. The **playing `▸` wins**
     the cell; the mark comes back when it stops. A **held row** keeps its
     glyph in the selection colours (ON-ACCENT on the slab); its token
     comes back when the hand leaves. **Precedence**, when several copies
     speak for one row (its song copy, and its server's album and artist
     copies): live (copying › waiting › stopping) › failed / stopped /
     refused › in your collection › already yours › cancelled, the newest
     job deciding a tie. A kept row stays listed for about a minute (the
     server's novelty cache), wearing ✓, until the next ask drops it. **No
     number** — no %, no count — on a row; a row's tooltip may say the
     state in words, and nothing lives only there.
114. **The tile's mark** — the tile's DIM artist row (12 cells) becomes
     the state word (`App::keep_tile_mark`): **◷ waiting** (DIM) ·
     **copying…** (ACCENT) · **stopping…** (DIM) · **✓ kept** (OK) · **✓
     yours** (DIM) · **! failed** · **! stopped** · **! refused** (GOLD) ·
     **× cancelled** (DIM); none leaves the artist. Never on the cover's
     pixels (Over pixels). The lit tile's slab is its title row's alone,
     so the word keeps its token; a cut tile (8 cells, always the mosaic)
     clips a longer word with `…` like any tile text ("! failed" stands
     whole). A tile wears only its album's and its artist's copies — never
     a song copy (one song is not the album).
115. **Per server, per album.** Album and artist copies are keyed on
     names, not servers, so a lookup can hand Stage Peer B's row Stage Peer
     C's copy of the same album: such a copy **never marks** this server's
     rows or tiles — running or done — and stays in the row's jobs for the
     modal (clause 108). A **song** copy is the recording wherever it came
     from and marks the row. A finished **artist** copy speaks for an
     album only as its walk did (`job_for_album`): the album's own account
     where it was walked, nothing where its listing failed (unless the run
     stopped there), already yours where only-missing left it out, nothing
     for an album the artist only guests on or one the walk never reached.
116. **The jobs indicator** — the right edge of the verbs line (it ends at
     the room's right edge less its gutter: cell 96 at 100 columns), in
     **words, never a count** (the 2026-10-19 log): **copying…** (ACCENT)
     while a copy is live (queued, running, stopping), **· ! failed** (` ·
     ` DIM, `! failed` GOLD) while one failed — its state failed, or it
     copied nothing and something failed, and no newer job of it stands
     (a stopped copy is done: its row and tile say "! stopped") — and
     hidden when neither, or while the copy gate is closed; on both tabs.
     **The squeeze**: the 47-cell room — the room whose table gives ALBUM
     up, the queue open — draws the indicator as its glyphs in **every**
     state, whatever the words would fit: **↓** while live, **↓ · !**
     live and failed, **!** failed only (legacy `v`, `v · !`, `!`); a
     wider room draws the words, squeezed too where the verbs would leave
     them too little; never clipped; the verbs give way from their tail
     only if even the glyphs do not fit. Click it, or **`C`** anywhere in
     the room while the copy gate holds, to open the Copies modal (`C`
     stays the queue panel's clear while the queue has the keys); hover
     brightens it whole; its tooltip "Copies — C" (squeezed: "copying… · !
     failed — Copies — C"). `App::keep_indicator` → `JobsIndicator`.

**The Copies modal**

117. **Copies** — the terminal's jobs tray (`App::copies_rows`): the kit's
     neutral modal, **76 cells wide**, centred over the whole frame, its
     height following its rows up to **22** (it fits 100 × 24 with the tab
     bar above and the tips below); the list scrolls in a `ListView` with
     `▲█▼` on its right border, the header, the rule and Clear finished
     never scrolling. Title **Copies** and `[X]`; the kit table **TITLE**
     31 · **FROM** 12 · **STATE** 20 · `[X]` 3. **Each copy takes two
     rows**: TITLE (the song; the album; "every album by {artist}" —
     `CopyTitle`), FROM (the server it copies from), STATE — the ten-cell
     bar and % while running (all DIM, no %, before an estimate), else one
     word: ◷ waiting · stopping… · ! failed · ! stopped · ! refused · ✓ in
     your collection · ✓ already yours · skipped · nothing to copy · ×
     cancelled — and `[X]` on a live row (DIM, DANGER on hover, "Cancel
     this copy — x"; none while stopping); then a DIM second row opening
     with "song" or "album" ("only the albums you don't have" for an
     only-missing copy; an artist copy opens with neither), then the step,
     the reason or the outcome, and at its right the row's action in
     ACCENT text: **Retry · Copy again · Start again · Play your copy** —
     none after a refusal. **Order**: running, then waiting in slot order,
     then the GOLD rows (failed, stopped, refused), then the finished;
     newest first in each; a finished copy that a newer one of the same
     plug-in, key, scope and server supersedes leaves (a retried failure
     is old news); the same album from two servers is two rows, FROM
     telling them apart. **Only Discover's copies**: the copy plug-in's
     jobs in Discover's own scopes — a folder copy (the webapp's peer
     browser, listed by the same `GET …/plugin-jobs`) or a scope this
     client does not know never lists, lights the indicator, keeps the
     poll going or words a note: nothing here could retry it (never
     Folder, clause 104), and no action stands that would do nothing. **Clear finished** — a DIM text button
     bottom-right, hidden while no row is finished — drops the finished
     rows on the server (`ApiCmd::JobsClear`); live rows stay. **Keys**: ↑ ↓
     rows (the wheel scrolls), Enter the held row's action, `x` cancels
     the held running or waiting copy, `X` clears finished, Esc closes —
     copies keep running. **States**: asking — "asking your server for
     copies…" (ACCENT) under the header; the first ask failed —
     "couldn't load copies: your server didn't answer" (GOLD) with `[ Try
     again ]` (Enter); a refresh failed — the last rows stay, "couldn't
     refresh: your server didn't answer · next try in 8 s" (GOLD;
     `App::keep_next_try`) with Try again beside Clear finished; empty —
     "Nothing copying." / "Start one with c on a connected server's song
     or album."; **a cancel that lost the race** — the list refreshes, the
     row shows its outcome and a TEXT note bottom-left says "Borrowed Tapes
     had already finished, nothing to cancel" (`KeepNote::TooLate`), gone
     on the next key.
     **As built** (`gui::keep::draw_copies`, 2026-10-20): the title's
     `[X]` stands over the rows' `[X]` column, a cell in from the border;
     the header and its rule DIM; a running copy's STATE is the kit's bar
     (▰ OK, ▱ and the % DIM; the legacy console says "52% done" / "no
     estimate yet"), a copy whose cancel was asked — the server's
     `cancelRequested` or this player's ask still out — "stopping…" with
     no `[X]` and its second row "stopping after {step}"; a copy's second
     row by its outcome — the step (or "copying…"), "waiting for a free
     slot", where a song landed by its folder ("copies › Color Out › Peer
     Sessions"), a many-song copy's counts from `result.counts` (copied ·
     already yours · failed · albums left out, DIM), "you had it, nothing
     copied" ("you had them all, …" for many), "a file already sits at
     that path", "nothing of theirs to copy" / "you have it all", the
     server's error words or "{n} songs failed, none copied" in GOLD, the
     stop's reason in GOLD ("{server}'s transfer limit stopped it", "…
     was too busy to send more", "… went away") then "· {n} copied" DIM,
     the refusal in GOLD, "cancelled · {n} copied"; the action words
     right-aligned to the table's 72nd cell. **Copy again** stands only
     where a gap is left — a song failed, an album's listing failed, songs
     not reached (`App::copies_action`); a many-song copy that took
     everything offers none. The held copy wears the selection slab across
     both rows and every word in it; ↓ or ↑ picks the hand up on the first
     copy and the list reveals it; the wheel and the bar's arrows scroll a
     copy a notch; a click on an action or an `[X]` acts and the room
     beneath is inert. Enter with nothing held after a failed ask asks
     again. A refused Retry, a cancel or a clear the server refused leave
     their words GOLD in the same bottom-left slot ("couldn't start: …",
     "couldn't cancel: …", "couldn't clear: …"). "· next try in N s"
     follows a refresh's error only while a copy is live — the poll runs
     only then.

**Notes and tips**

118. **Status notes** — the App's message on the bar's bottom row (the
     window's width less 70 cells: 30 at 100 columns), for a copy that
     changes state **while its own Keep modal is closed** (the open modal
     speaks for itself; `App::keep_take_news`): "copying Borrowed Tapes…"
     (ACCENT, busy) · "copied Late Train Verse" (OK) · "album copy
     finished" (OK) · "album copy: some songs failed" (GOLD) · "already
     yours" (DIM) · "nothing to copy" (DIM) · "copy failed: {the server's
     words}" (GOLD) · "copy stopped: transfer limit" (GOLD) · "Stage Peer B
     refuses copies" (GOLD) · "copy cancelled" (DIM). The tail ("from
     {server}", "into {library}", "· {title}") is added only when the
     whole note fits; server words that still overflow clip with `…` (a
     trailing space trimmed first) and stay whole in the Copies list and
     the Keep modal. The next note replaces it, as every App message does.
119. **Tips** name a key only where it works: `c` only on a connected
     server's row, tile or sheet while the copy gate holds, `C` only while
     the indicator stands. Held connected-server row: "Enter play from
     here · a queue · N next · P play now · c keep a copy · m actions ·
     Esc done" ("← → tabs" gives way, so the line stays inside 99 cells;
     ← → still switch tabs). Lit connected-server tile: "← → albums · Enter
     play the album · + a queue · c keep a copy · m more · ↓ songs · Esc
     done" — with the copy gate closed the same without "c keep a copy"
     (the tile's list stands whatever the gate). The sheet on a connected
     server's song: "↑ ↓ rows · Enter
     choose · c keep a copy · Esc close" (no rating keys). The tile's list:
     "↑ ↓ rows · Enter pick · a queue · c keep a copy · Esc close". At rest
     with the indicator standing: "↓ pick an album · ← → tabs · w window ·
     e more options · G new seeds · C copies". The Keep modal: "↑ ↓ rows ·
     Space or Enter pick · Enter on Add song starts · d where files land ·
     Esc close" (the choice; the verb follows it; where a locale's line
     outgrows 99 cells, "· d where files land" gives way first) · "Esc close" (starting, stopping, a
     refusal, nothing to copy) · "x cancel the copy · Esc close — it stays
     queued" / "… — the copy keeps running" · "Enter play your copy · ↑ ↓
     buttons · Esc close" · "Enter copy again · …" · "Enter retry · …" ·
     "Enter start again · …" · "Enter close · Esc close" (already yours;
     nowhere, clause 147) · "↑ ↓ rows · Space or Enter pick · d where files
     land · Esc close" (another server copying). Where files land's, by
     the hand: clause 143.
     The Copies modal, by the held row: "↑ ↓ rows · x cancel · X clear
     finished · Esc close" (running or waiting) · "↑ ↓ rows · Enter retry ·
     X clear finished · Esc close" (failed or stopped) · "↑ ↓ rows · Enter
     play your copy · X clear finished · Esc close" (a kept song) · "↑ ↓
     rows · X clear finished · Esc close" (refused) · "Esc close"
     (asking, an error, empty) — composed of its parts (`gui.tips.
     discover_copies_*`), the verb only for the held row's action and "X
     clear finished" only while a finished copy is listed. Open
     elsewhere's list: "↑ ↓ rows · Enter open · Esc back" over links,
     "Enter try again · Esc back" after a failure, "Esc back" while it
     asks or has none. With the copy gate closed every tip reads as
     before.

**Rules every surface here keeps**

120. **"Done" is not "kept"** (`PluginJob::outcome` → `JobOutcome`, the
     webapp's `jobRowState` with its 2026-10-19 fixes). In your collection
     only when something was copied: a song's `result.copied`, or an album
     or artist copy's `counts.copied > 0` with `stopped` null. A done many-
     song copy with `stopped` quota / busy / peer is **stopped** with its
     reason (Retry), with `refused` **refused** (no Retry), with
     `cancelled` (the cancel that landed as the run ended) **cancelled**
     (Start again), and with a word this client does not know **stopped**
     in neutral words ("the copy stopped before it finished", Retry) —
     **any** stop word keeps it out of your collection; **copied 0 with
     a failure is failed**, never "already yours"; copied 0 with skips (or
     its albums left out as owned) is **already yours**; nothing at all is
     **nothing to copy** — an artist who only guests there says so. A song
     copy that failed "does not allow copies" is **refused**. Every number
     comes from `result.counts`, never the per-song lists (capped at 500).
121. **Another server's copy** — clause 115's: album and artist keys
     ignore the server; the room and the modal never take one server's
     album or artist copy for another's.
122. **`created: false`** — a start the server answers with a live job
     already covering it: that job is followed; another server's live copy
     reads as clause 108 says, and the primary comes back once it ends.
123. **A kept song or album stays** on the Connected servers tab for up to
     a minute (the server's novelty cache), marked ✓ (clause 113), until
     an ask no longer lists it.
124. **Glyphs** live in code behind `legacy_conhost`, the words in the ten
     locales. On the legacy console: the LEAD's `>` for `▸`, `v` for `↓`,
     `~` for `◷`, `+` for `✓`, `x` for `×`, `»` for a clipped cell's `…`
     and "..." for a word's; the tile words "~ waiting", "copying...",
     "stopping...", "+ kept", "+ yours", "x cancelled"; the indicator keeps
     its words ("copying... · ! failed", squeezed "v · !"); the verbs
     line's `⇄` drops; the modals' `+ - |` frame, `(*)` radios, `[x]`
     checked, `>` for `›`, `*` for `•`, the header cover as the ▀ mosaic,
     and the bar row in words ("52% done"; "no estimate yet" while
     queued).
125. **Over pixels**: every new overlay — the Keep and Copies modals, the
     tile's list, the links list — registers its footprint, so a cover it
     touches draws as the mosaic (docs/ui-kit.md, Modal).
126. **No numbers on the page**: no percentage and no count on a row, a
     tile, the indicator or the tabs (the user's 2026-10-11 "no counts
     anywhere", confirmed 2026-10-19); the Keep and Copies modals carry
     them.
128. **A peer re-paired** — a copy that failed "no longer paired": Retry
     asks the room again (`taste_ask_again`) rather than the same job —
     the new rows carry the server's new id.
129. **A peer gone after its answer**: a copy is accepted, holds the copy
     plug-in's single slot about 15 s and fails ("… is unreachable"); the
     copies behind it read "waiting for a free slot".
130. **The server's own words** — for 400, 403, 404, 409 and 429 at start,
     and `job.error` on failure — are shown as they are, after "<what
     failed>: ", never a bare code (`PluginFailure::Refused { words }`; the
     plug-in routes keep a 404's words). A server that did not answer reads
     "your server didn't answer" (`PluginFailure::Unreachable`).
133. **A cancel can lose its race** — the server's 409 is its word that the
     job had already finished (`CancelAnswer::AlreadyFinished`): the list
     is asked again, the outcome shows, and the Copies modal says so once
     (clause 117).

**Open elsewhere**

140. **The links list** — asked only when it opens (`ApiCmd::PluginLinks`,
     the `links` plug-in's resolve), never prefetched; no row at all
     without a links plug-in. A `frame_at` list, ACCENT border, hung
     beside the sheet's **Open elsewhere…** row (the rows below are short,
     so it rises to sit level with the row, clear of `[X]` and the sheet's
     borders); the row's label turns ACCENT BOLD while its list is open.
     **CANONICAL** (the recording's own MusicBrainz ids) then **SEARCH**
     (MusicBrainz, Deezer, Apple Music, YouTube, Bandcamp, Discogs) — DIM
     UPPERCASE heads, items indented, the plug-in's labels, **https only**
     (`PluginLink::is_web`), never a URL. States: **asking…** (ACCENT);
     the list; **no links for this song** (DIM); **couldn't get links:**
     and the server's words (GOLD) with **Try again** held (Enter asks
     again). Keys: ↑ ↓ to the row, Enter or → opens the list; ↑ ↓ rows;
     Enter opens the held link; Esc or ← closes the list. Mouse: click the
     row, then a link; hover BRIGHT, the held link in selection colours;
     more links than fit scroll. Tips: "↑ ↓ rows · Enter open · Esc back".
     **As built** (`gui::keep::draw_links`, 2026-10-20): 30 cells inside
     (wider for a longer label, never over the row's label), its right
     edge three cells in from the sheet's, its bottom border one row under
     the row, rising to at most the sheet's title row and scrolling past
     that (the kit's bar on its right border, the wheel over it); the
     heads DIM and uppercased in code, the links three cells in, the
     answer's https links only (`PluginLink::is_web`; a list of none of
     them reads "no links for this song"), CANONICAL first. Enter or → on
     the row opens the list with its first link held; a click on the row
     opens it with nothing held (the kit's list-cursor law), a click on a
     link opens that link. While the list hangs, the row gives up its
     selection slab — the list holds the keyboard's — and wears ACCENT
     BOLD; ↑ ↓ and Enter are the list's; Try again stands held after a
     failure, the server's words wrapped GOLD above it.
141. **Opening a link** — through `gui::opener::open_target` (honouring
     `MSTREAM_NO_OPEN`): "opening Deezer…" (ACCENT) while the hand-off
     runs; "opened Deezer" (TEXT) once the system took it — the list stays
     open, so another can be tried; "Deezer: no app took it" (GOLD) when
     nothing handled it. **As built**: the hand-off runs on a thread and
     answers on `LinksUi`'s channel, worded once a frame
     (`gui::keep::frame_tick`); "opened" is `MessageKind::Plain`; an
     opener that could not start reads "no app took it" too; under
     `MSTREAM_NO_OPEN` (the harness's seam) nothing opens and the note
     says "Deezer · {its url}", as the QR page's browser opens do.

**Where files land (2026-10-21)**

The canvas's `TermDestination` board (after the webapp's `RecDestination`
and its SPEC §9) draws every state here cell for cell; the sample is the
board's: the account may upload to **music** (the library default, its
template ARTIST / YEAR - ALBUM) and **copies** (saved, Album artist /
Album). Built in `gui::keep` (`DestUi`, `draw_dest`, `dest_key`) over the
App's `keep_dest_*` (the view, the save, the folders).

142. **The picker** — `[ Change… ]` or `d` (clause 105) opens **Where
     files land** on the place in force: its own kit modal, ACCENT, 76
     cells wide (at most the area less 4), centred over the whole frame,
     anchored as if 22 rows (its top on row 4 at 100 × 30, row 1 at 100 ×
     24) and growing downward. Keep a copy is not drawn while it is open;
     the room beneath is inert, and the modal registers as an overlay, so
     the covers it touches draw as the mosaic. Esc (the folder list
     closed), `[X]` (DIM, "Back — Esc") or Cancel go back to Keep a copy —
     nothing saved, the hand on `[ Change… ]`.
143. **Its rows**, from its left border — labels col 2, values col 12
     (two past a wider locale's label), the last content cell col 73:
     the title **Where files land** ACCENT BOLD; a blank; **LIBRARY** —
     `◂ copies ▸` (the arrows DIM, the name BOLD; ← → or a click step
     through the server's order, a click on the name steps on, a click on
     the label takes the hand), three cells, DIM "only libraries you may
     upload to"; with one library its name BOLD and DIM "your only
     library", no arrows and no stop (its label takes no click);
     **FOLDER** — a line editor (tui-input) at cols 12–73: empty and
     without the hand DIM "library root"; with the hand its label ACCENT
     BOLD, the caret `▏` (legacy `│`), tail-scrolled behind a `…`; under
     it 0–3 folders of the open list (DIM, the held one the selection
     slab across cols 11–73 — not bold —, hover BRIGHT, the kit's bar
     past three); the **hint** row: "in copies › from-peers · 3 folders"
     while the list is open, GOLD "files would land in X › X" where the
     base folder's last name is the layout's first rendered folder (case
     folded), "not there yet · the first file creates it" where a typed
     name is absent from its parent's listing, GOLD "couldn't list
     {folder}: {the server's words}" for the folder being completed, else
     "inside copies · leave it empty for the library root"; a blank;
     **LAYOUT** — `(•)` OK / `( )` DIM at col 12, labels col 16: "Album
     artist / Album" (`{{ALBUMARTIST}}/{{ALBUM}}`), "Artist / Album"
     (`{{ARTIST}}/{{ALBUM}}`), "Server / Album artist / Album"
     (`{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}`), "{library}'s template" only
     where that library's template is none of the three, and "Custom",
     whose field (cols 24–73) always shows the layout in force — DIM
     unless Custom is chosen, TEXT with the caret under the hand. The
     chosen radio is the preset the layout is, character for character,
     else Custom (and Custom once picked). With the hand in the field the
     **variables** row (col 24 — right-aligned to col 73 where a wider
     locale's Custom label would push the six past it, so none is lost)
     lists the view's variables joined " · ", DIM text buttons — a click
     inserts `{{NAME}}` at the caret; a blank;
     **PREVIEW** — the copy Keep a copy has chosen rendered with the
     fields as typed (clause 105's rules, its slots DIM included; the
     library BOLD), a variable the view does not list drawn as typed in
     GOLD, one row clipped from the left behind `…` (the whole path in
     its tooltip, then a slot's note — the note alone where nothing is
     clipped); then a song's
     file name DIM (blank for an album or an artist); a blank; DIM
     "Applies to every copy and download on your account" — a refusal's
     GOLD rows in its place (clause 144) — and the buttons right-aligned
     to col 73: DIM `[ Use the library default ]` only while the fields
     differ from the default (the first library, no folder, its template
     else the default layout; it refills the fields only, the hand on
     LIBRARY), DIM `[ Cancel ]`, ACCENT BOLD `[ Save ]`. 18 rows, a row
     more for a library's template, the variables row or each listed
     folder, two for a long refusal — the list and the variables never
     stand together and every edit clears a refusal, so it never passes
     22. **Keys**: ↑ ↓ walk LIBRARY › FOLDER › each radio › Custom's field
     (only while Custom is chosen) › Use the library default › Cancel ›
     Save; the hand opens on LIBRARY (FOLDER with one library). ← → step
     the library and move between the buttons; Space or Enter pick a
     radio (Custom's also moving the hand into its field) and press a
     button, as Keep a copy's do; in a field
     every printable key edits (Space too), ← → Home End and the ctrl
     word ops move the caret. FOLDER: **Tab** opens the list on the
     folder being completed (an empty field lists the library root),
     extends the typed name to the names' common start, then cycles the
     held folder; Shift-Tab cycles back; ↓ walks into the list, ↑ from its
     top back to the field; Enter on a held folder fills "{folder}/{name}/"
     and lists inside it; Esc closes the list. Enter in a field (the list
     closed or nothing held) or on LIBRARY saves. A hand whose stop leaves
     (a re-read drops a library's template) goes to the first stop, never
     nowhere. Mouse: every stop, the arrows and the name, a folder, a
     variable, a button; hover BRIGHT + BOLD on the radios, the arrows and
     the name, a variable and a button, BRIGHT on a listed folder — a
     field's row and the LIBRARY label take the hand without a hover
     style; Apple Terminal's release echo swallowed as everywhere. **Folders** come from the user's own
     server's file explorer (`/{library}/{folder}`), asked on the worker
     one at a time, each once while the picker is open
     (`App::keep_dest_list`). **Tips** by the hand: "↑ ↓ rows · ← →
     library · Enter save · Esc back" · "type a folder · Tab folders · ↑ ↓
     rows · Enter save · Esc back" · "↑ ↓ folders · Enter open · Tab
     complete · Esc close the list" · "↑ ↓ rows · Space or Enter pick ·
     Esc back" · "type a layout · ↑ ↓ rows · Enter save · Esc back" · "← →
     buttons · Enter save|cancel|use the library default · Esc back" ·
     "Esc back" while saving.
144. **Save** — `PUT …/collection/destination` (`ApiCmd::DestinationSave`)
     with `{destination: {vpath, base (as typed, trimmed), layout}}`, or
     `{destination: null}` where Use the library default was pressed while
     the picker was open and the fields are the default at Save — a step
     away and back, or a radio picked again, keeps it (the webapp's
     `reset`). While it is out "saving…" (ACCENT) stands right-aligned in
     the button row, no buttons, the hand down, the frame its height; Esc
     still goes back and the answer still folds in. **The answer is its
     own picker's** (by the save's seq, as the webapp's `m.picker === k`):
     a picker opened again meanwhile is no "saving…", its keys work, its
     Save supersedes the one out, and the old answer never closes or fills
     it; a refusal whose picker is gone is the status line's, Error,
     "couldn't save: {the server's words}" fitted to its slot. **Order**:
     every answer lands on its own thread, so the gate's view is stamped
     by the ask that wrote it — a read, the gate's ask or a start's 409
     sent before a save that answered draws nothing. **200**: its view
     replaces the gate's (`Event::DestinationSaved`, whichever picker sent
     it) and its picker closes onto Keep a copy — the new LANDS IN, "saved
     for your account", the hand on `[ Change… ]`. **A refusal** (400, 403,
     any other): GOLD "couldn't save: {the server's words}" — "destination
     layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, …" —
     wrapped at 72 cells, three rows at most, then `…`; no answer:
     "couldn't save: your server didn't answer"; the fields, the hand (on
     Save where it was nowhere) and Save stay. A base folder the server
     refuses has its words too: "destination base folder: Resolved path
     cannot contain .. segments". **No layout is checked here**: the
     server's words decide.
145. **A start sends the place it showed** — every `ApiCmd::JobStart`
     carries `destination` (`{vpath, base, layout}`, its `source`
     dropped): Keep a copy's Add the place LANDS IN drew; the modal's
     Retry and Copy again the account's place now, which the modal shows
     first where it is not where the copy ran ("retry adds to …", clause
     105 — the webapp's Retry and Add again); the Copies list's Retry,
     Copy again and Start again, which show no place, the place the copy
     ran with — its `params.destination`, else (a job from before the
     pin) its `result.destination`, else the account's place — so a place
     moved since is the server's 409, never a quiet landing somewhere the
     list never showed. Never no place. The server pins it on the job; a
     covering live job still answers `200 created: false` and is
     followed. **A server from before the key** (mStream before
     2026-10-21, its jobs route's strict body: Joi's 400 '"destination"
     is not allowed') is asked once more without it; it runs the copy
     unpinned, and the job reads as from before the pin.
146. **The place moved** — the start's **409 `destination_changed`**
     (`PluginFailure::DestinationChanged`, its `destination` the one in
     force) starts nothing: the gate's destination becomes the answer's
     at once (unless an answer to a later ask is drawn) and the view is
     read again — a body with no `destination`, or one this client cannot
     read, keeps what is drawn until the read answers (only an explicit
     `null` is nowhere, clause 147); Keep a copy comes back on the
     choice (`KeepStage::Refused`) with LANDS IN redrawn and one GOLD line
     above the buttons, "where files land changed · check the path above
     and add again"; Add stays, the hand back on the chosen radio. From
     the Copies list the note reads "couldn't start: where files land
     changed — retry it from Keep a copy" (on the spacer row while the
     buttons would cut it) — where the modal's Retry says the place it
     sends and starts at once. **Another account's 409** keeps its own words
     (clause 108).
147. **Nowhere** — a re-read or a 409 whose destination is `null`: Keep a
     copy's choice gives way to TEXT "your account can't add files to
     this server · copies are off", a blank and `[ Close ]` (ACCENT BOLD,
     the primary) — 11 rows; tips "Enter close · Esc close"; the copy
     gate closes (clause 101), so every way in is hidden.
148. **The legacy console**: the `+ - |` frame, `< >` for `◂ ▸`, `(*)`,
     `>` for `›`, the caret `│`, "..." for `…` ("saving...", "[ Change...
     ]"); covers as the mosaic.

## Wording

English reference; every string through `t!()` in all ten locales (de en
es fr it ja pl pt ru zh). Glyphs (`▸ » ⋯ • ▾ ✓ — ↻`) are drawn by the code
beside the words, never in the strings (docs/ui-kit.md, Glyphs). Keys
ending `_*` are plural families — `_one`, `_few`, `_many`, `_other`, chosen
the way the DJ room's `songs_words` chooses (English and the one-plural
locales repeat the other form); `%{count}` is their placeholder. Since
2026-10-11 the only families left are the seeds modal's ages: the room
itself shows no count. Since 2026-10-19 the Keep a copy and Copies
modals carry theirs (clause 126: numbers live only there).

| String | Key |
|---|---|
| Discover (the nav row and the room's heading) | `gui.nav.discover` — at most 12 cells in every locale |
| base tips: … · D Auto DJ · **G discover** · T stats · … | `gui.tips.base` (amended) |
| ↓ pick a row · ← → tabs · w window · e more options · G new seeds (2026-10-16; German, Spanish, French, Japanese and Russian name `e` by the short "options" to fit the 99-cell footer with the save words) | `gui.tips.discover` |
| ↓ pick an album · ← → tabs · w window · e more options · G new seeds (at rest where the album strip stands) | `gui.tips.discover_strip` |
| s save as playlist (appended to every rest tip — `discover`, `discover_strip` and the fallback card's `discover_fallback` — after ` · `, while the Library tab has songs; 2026-10-16) | `gui.tips.discover_save` |
| Enter play from here · a queue · N next · P play now · m actions · ← → tabs · Esc done (a library song's row and a connected server's) | `gui.tips.discover_row` (one key since 2026-10-11) |
| Enter open the album · a queue · ← → tabs · Esc done | `gui.tips.discover_album` |
| ← → albums · Enter open the album · + a queue · ↓ songs · Esc done (the hand on a library album's tile) | `gui.tips.discover_tile` |
| ← → albums · Enter play the album · + a queue · ↓ songs · Esc done (the hand on a connected server's album's tile) | `gui.tips.discover_peer_tile` |
| r another pick · e more options · ← → tabs | `gui.tips.discover_fallback` |
| ↑ ↓ rows · Space keep or drop · x unpin · Esc apply | `gui.tips.discover_seeds` |
| ↑ ↓ rows · Enter pick · Esc close (the window's list) | `gui.tips.discover_menu` |
| based on songs you finished | `gui.discover.based_on_history` |
| based on songs you've played before | `gui.discover.based_on_counts` |
| based on the songs you picked | `gui.discover.based_on_picked` |
| nothing finished yet | `gui.discover.nothing_finished` |
| Last %{days} days · All time (the window; days are 7, 30, 90) | `gui.discover.window_days` · `gui.discover.window_all` |
| The plays your seeds come from — w | `gui.discover.window_tip` |
| nothing in the last %{days} days — all time instead (the tagline's second fact; the window dropdown's tooltip leads with it) | `gui.discover.widened` |
| asking… (the header's refresh while an ask is out) | `gui.discover.asking` |
| no answer (a cleared wait's failure words: the failure line's tooltip) | `gui.discover.state_failed` |
| Library · Connected servers | `gui.discover.tab_library` · `tab_servers` |
| more options · options (the header's control, and its short form at the narrowest, clause 17) | `gui.discover.more_options` · `options_short` |
| new seeds (the header's refresh, after the gated `↻`) | `gui.discover.new_seeds` |
| Ask again around a new batch of seeds — G (the refresh's tooltip) | `gui.discover.new_seeds_tip` |
| Choose what this room starts from — e (more options' tooltip) | `gui.discover.seeds_tip` |
| picked (a pin's marker in the seeds modal) | `gui.discover.seed_pinned` |
| *(gone 2026-10-16 with the state line and the seeds line: `from_counts`, `state_ready`, `state_empty`, `state_pending`, `state_off`, `no_seeds`, `no_seeds_fact`, `through_server`, `seeds`, `seeds_more`, `seeds_edit`, `seed_random`)* | — |
| Based on (the modal's title) | `gui.discover.seeds_title` |
| songs you finished or mostly heard · skips never count | `gui.discover.seeds_sub` |
| PINNED · FROM YOUR PLAYS · DROPPED | `gui.discover.sec_pinned` · `sec_seeds` · `sec_dropped` |
| finished · listened to · not analyzed yet | `gui.discover.outcome_completed` · `outcome_stopped` · `not_analyzed` |
| Pin the playing song · Pin a song… · Done | `gui.discover.pin_playing` · `pin_song` · `seeds_done` |
| Unpin — x | `gui.discover.unpin_tip` |
| Up to 8 songs can be picked — unpin one first | `gui.discover.pins_full` |
| Pick a song to start from — click a track anywhere in the library · Esc cancels | `gui.discover.pick_banner` |
| Pick a track on %{server} | `gui.discover.pick_other_server` |
| Finish a few songs and this room learns your taste | `gui.discover.card_title` |
| Songs you play to the end — or most of the way — become seeds. Skips never count. | `gui.discover.card_body` |
| Until then, around a random pick: %{track} · Until then, around your pick: %{track} | `gui.discover.card_around_random` · `card_around_pick` |
| another pick · choose a song… | `gui.discover.another` · `choose` |
| Another random pick — r | `gui.discover.another_tip` |
| Asked once you've finished a song — a random pick only searches your library. | `gui.discover.not_asked` |
| TRACK · ARTIST · ALBUM · ON · LENGTH | `gui.discover.col_track` · `col_artist` · `col_album` · `col_on` · `col_length` |
| today · yesterday (the seeds modal) | `gui.discover.today` · `yesterday` |
| %{count} days ago · %{count} months ago · %{count} years ago (the seeds modal) | `gui.discover.days_ago_*` · `months_ago_*` · `years_ago_*` |
| album (an album row's marker) | `gui.discover.album` |
| Open the album — Enter (an album row's title, a library tile's) | `gui.discover.album_tip` |
| Play the album from %{server} — Enter (a connected server's tile) | `gui.discover.peer_album_tip` |
| Add the album to the queue — a (an album row's `[+]`, a tile's) | `gui.discover.album_queue_tip` |
| couldn't load the album | `gui.discover.album_failed` |
| sounds like %{track} | `gui.discover.seed_tip` |
| Listen through your server — P | `gui.discover.listen_tip` |
| Add next — N · Add to the queue — a · Play now — P · More actions | reused: `gui.files.next_tip` · `gui.files.queue_tip` · `gui.files.now_tip` · `gui.act.more_tip` |
| play · queue all · shuffle (the verbs line) | reused: `gui.bar.play` · `gui.bar.queue` · `gui.shuffle_word` |
| save as playlist… (the verbs line, Library tab) | `gui.discover.save_playlist` |
| Save as playlist · enter saves · esc cancels · Save (the name prompt) · type a name · Enter saves · Esc cancels (its tips) | reused: the sonic room's `gui.sonic.save_title` · `save_hint` · `save_ok` · `gui.tips.sonic_save` |
| Nothing new in your library sounds like these songs. | `gui.discover.empty_library` |
| Nothing from your connected servers right now. | `gui.discover.empty_servers` |
| couldn't reach this source · Try again | `gui.discover.failed` · `gui.discover.retry` |
| This server hasn't analyzed any music yet — suggestions appear once the discovery scan has run. · Check again | `gui.discover.scan_pending` · `gui.discover.check_again` |
| Sonic discovery has been switched off on this server. | `gui.discover.turned_off` — the sonic-path contract's `discoverTurnedOff` words |

**Keep a copy, the Copies modal, Open elsewhere** (2026-10-19, clauses
100–141 — the canvas's words; the webapp's `discover.job.*` and
`discover.modal.*` carry the same meanings in its ten locales, so their
translations can be carried over). Plural families end `_*`.

| String | Key |
|---|---|
| Keep a copy… · Open elsewhere… (the sheet's rows; the DIM `c` after the first is code) · from %{server} · online (its badge row, the Keep modal's header) | `gui.act.keep_copy` · `open_elsewhere` · `from_server` · `online` |
| Play album · Queue album · Open on %{server} (the tile's list; Keep a copy… is `gui.act.keep_copy`) | `gui.discover.tile_play` · `tile_queue` · `tile_open_on` |
| Keep a copy (the modal's title) · What · Lands in (uppercased in code) | `gui.discover.keep.title` · `what` · `lands_in` |
| This song · Album · Every album by %{artist} · Only the albums you don't have | `gui.discover.keep.song` · `album` · `artist` · `only_missing` |
| %{count} songs · %{count} albums · you have %{count} (the sizes) | `gui.discover.keep.songs_*` · `albums_*` · `you_have_*` |
| Add song · Add album · Add artist · Add %{count} albums · Cancel · Close · Cancel copy · Retry · Copy again · Start again · Add to queue · Play your copy | `gui.discover.keep.add_song` · `add_album` · `add_artist` · `add_missing_*` · `cancel` · `close` · `cancel_copy` · `retry` · `copy_again` · `start_again` · `add_to_queue` · `play_copy` |
| starting… · couldn't start: %{words} · your server didn't answer (a start's words when it did not) | `gui.discover.keep.starting` · `refused_start` · `no_answer` |
| %{other} is already copying this album — you can add %{server}'s once it ends · … these albums … (an artist copy) | `gui.discover.keep.other_album` · `other_artist` |
| waiting for a free slot · starts on its own when the copy ahead ends · copying… (a running copy's step when the plug-in names none) · stopping… · no estimate yet · %{pct}% done (the legacy console's bar row) | `gui.discover.keep.waiting` · `waiting_sub` · `copying` · `stopping` · `no_estimate` · `pct_done` |
| %{count} copied · %{count} already yours · %{count} failed · %{count} not copied · %{count} albums left out | `gui.discover.keep.copied_*` · `owned_*` · `failed_*` · `not_copied_*` · `left_out_*` |
| in your collection · Copy again takes only the gaps · already yours · your library already has this song · … these songs · nothing was copied | `gui.discover.keep.kept` · `gaps` · `yours` · `yours_song` · `yours_many` · `nothing_copied` |
| skipped · a file is already at that path · nothing was overwritten | `gui.discover.keep.exists` · `exists_sub` |
| %{server}'s transfer limit stopped it · %{server} is serving too many streams right now · %{server} went away · the copy stopped before it finished (a stop word this client does not know, clause 120) · try again later · try again in a moment | `gui.discover.keep.stopped_quota` · `stopped_busy` · `stopped_peer` · `stopped_other` · `later` · `moment` |
| %{server} does not allow copies with your server's key · playing it still works | `gui.discover.keep.refused` · `refused_sub` |
| copy failed: %{words} · the copy failed (a copy that failed in no words) | `gui.discover.keep.failed` · `failed_plain` |
| cancelled · the other %{count} stay on %{server} | `gui.discover.keep.cancelled` · `stay_*` |
| nothing to copy · you have it all · nothing of theirs to copy · %{artist} only appears on other artists' albums there · the copy finished (a result this client cannot read) | `gui.discover.keep.nothing` · `all_yours` · `nothing_theirs` · `appearances` · `finished` |
| waiting · copying… · stopping… · kept · yours · failed · stopped · refused · cancelled (the tile's state words and the LEAD's tooltip; the glyphs are code) | `gui.discover.mark.waiting` · `copying` · `stopping` · `kept` · `yours` · `failed` · `stopped` · `refused` · `cancelled` |
| Copies (the modal's title) · copying… · failed (the indicator's words) · Copies — C (its tooltip) | `gui.discover.copies.title` · `live` · `failed` · `tip` |
| Title · From · State (the header, uppercased in code) · every album by %{artist} · song · album · only the albums you don't have (a second row's opening word) · in your collection · skipped · nothing to copy · finished (the STATE words beside the marks' `waiting` · `failed` · `stopped` · `refused` · `cancelled` and the Keep modal's `yours` · `stopping`) · stopping after %{step} · you had it, nothing copied · you had them all, nothing copied · a file already sits at that path · %{count} songs failed, none copied · %{server} was too busy to send more (second rows, beside the Keep modal's `waiting` · `copying` · `copied_*` · `owned_*` · `failed_*` · `left_out_*` · `stopped_quota` · `stopped_peer` · `refused` · `cancelled` · `nothing_theirs` · `all_yours` · `failed_plain` · `finished`) · asking your server for copies… · couldn't load copies: %{words} · couldn't refresh: %{words} · next try in %{secs} s · Nothing copying. · Start one with c on a connected server's song or album. · Clear finished · Cancel this copy — x (the `[X]`'s tooltip) · %{title} had already finished, nothing to cancel · it had already finished, nothing to cancel · couldn't cancel: %{words} · couldn't clear: %{words} (a start refused from the list is the Keep modal's `refused_start`; Try again is `gui.discover.retry`; your server didn't answer is `gui.discover.keep.no_answer`) | `gui.discover.copies.col_title` · `col_from` · `col_state` · `every_album` · `kind_song` · `kind_album` · `kind_missing` · `kept` · `skipped` · `nothing` · `done` · `stopping_after` · `had_it` · `had_them` · `exists` · `failed_none_*` · `stopped_busy` · `asking` · `load_failed` · `refresh_failed` · `next_try` · `empty` · `empty_sub` · `clear` · `cancel_tip` · `too_late` · `too_late_plain` · `cancel_failed` · `clear_failed` |
| Canonical · Search (the heads, uppercased in code) · no links for this song · couldn't get links: · opening %{label}… · opened %{label} · %{label}: no app took it (Open elsewhere's list and its notes; asking… is `gui.discover.asking`, Try again `gui.discover.retry`) | `gui.discover.links.canonical` · `search` · `none` · `failed` · `opening` · `opened` · `no_app` |
| copying %{title}… · copying %{title} from %{server}… · copied %{title} · copied %{title} into %{library} · album copy finished · album copy: some songs failed · already yours · copy skipped: a file is in the way · nothing to copy · copy failed: %{words} · copy stopped: %{why} · transfer limit · %{server} reached its transfer limit · server too busy · %{server} is too busy right now · server went away · %{server} went away · ended early (an unknown stop word's) · %{server} refuses copies · copy cancelled (the status notes, clause 118: the short and the long form each — the long where the whole note fits) | `gui.discover.note.copying` · `copying_from` · `copied` · `copied_into` · `album_done` · `album_failed` · `yours` · `exists` · `nothing` · `failed` · `stopped` · `why_quota` · `why_quota_long` · `why_busy` · `why_busy_long` · `why_peer` · `why_peer_long` · `why_other` · `refuses` · `cancelled` |
| Where files land (the picker's title) · Library · Folder · Layout · Preview (uppercased in code) · Back (its `[X]`'s tooltip) · only libraries you may upload to · your only library · library root · inside %{library} · leave it empty for the library root · in %{path} · %{count} folders · not there yet · the first file creates it · couldn't list %{path}: %{words} · files would land in %{path} (`›` between the names in code) · Album artist / Album · Artist / Album · Server / Album artist / Album · %{library}'s template · Custom · Applies to every copy and download on your account · Use the library default · Save · saving (`…` in code) · couldn't save: %{words} · save · cancel · use the library default (the button tip's verbs) | `gui.discover.dest.title` · `library` · `folder` · `layout` · `preview` · `back` · `lib_hint` · `only_library` · `root` · `base_hint` · `listing_*` · `not_there` · `list_failed` · `nested` · `preset_albumartist` · `preset_artist` · `preset_server` · `preset_library` · `custom` · `applies` · `use_default` · `save` · `saving` · `save_failed` · `verb_save` · `verb_cancel` · `verb_default` |
| saved for your account · the library's default · Change (`…` in code; Keep a copy's SOURCE row) · where files land changed · check the path above and add again · where files land changed — retry it from Keep a copy (the Copies note) · your account can't add files to this server · copies are off · one folder per album (an artist copy's LANDS IN; it replaced `keep.each_album`) · retry adds to · copy again adds to (the row under LANDS IN where the account's place moved, clause 105) | `gui.discover.dest.source_user` · `source_default` · `change` · `changed` · `changed_retry` · `nowhere` · `per_album` · `retry_adds_to` · `again_adds_to` |
| artist · year · genre (a slot's word, `‹ ›` in code) · %{slots} comes from each song's own tags, so these songs can land in more than one folder. · %{slots} come from … (more than one slot, ", " between in code, in the path's order) · …more than one folder (a song with none leaves that folder out). (a genre among them) · %{slots} comes from the song's own tags (a song with none leaves that folder out). (a song's copy alone) — the note in LANDS IN's and the PREVIEW's tooltip, clause 105; the phone's words in every locale | `gui.discover.dest.slot_artist` · `slot_year` · `slot_genre` · `slot_note_*` · `slot_note_genre_*` (the `_one` … `_other` family) · `slot_note_song` |
| the tips of clause 119 | `gui.tips.discover_row_keep` · `discover_peer_tile_keep` · `discover_peer_tile_more` (a connected server's lit tile with the gate closed: its list stands) · `discover_sheet` (a connected server's song's sheet, no rating keys) · `discover_sheet_keep` · `discover_tile_list` · `discover_tile_list_keep` · `discover_copies` ("C copies", appended to a rest tip while the indicator stands and the line has the cells) · `discover_keep_choose` (`%{verb}` the primary's words) · `discover_keep_close` · `discover_keep_queued` · `discover_keep_running` · `discover_keep_play` · `discover_keep_again` · `discover_keep_retry` · `discover_keep_start_again` · `discover_keep_yours` · `discover_keep_other` (both with "d where files land", whose words `discover_keep_d` are what gives way where a line outgrows 99 cells) · the picker's `discover_dest_library` · `discover_dest_folder` · `discover_dest_list` · `discover_dest_radio` · `discover_dest_custom` · `discover_dest_button` (`%{verb}`) · `discover_dest_saving`; the Copies modal's parts, joined with ` · ` — `discover_copies_rows` (↑ ↓ rows) · `discover_copies_cancel` (x cancel) · `discover_copies_retry` · `discover_copies_again` · `discover_copies_start_again` · `discover_copies_play` (Enter …) · `discover_copies_clear` (X clear finished) · `discover_copies_esc` (Esc close); the links list's `discover_links` · `discover_links_failed` · `discover_links_wait` |

Data is never translated: titles, artists, album and server names.

## Out of scope here

- **The p2p network** (since 2026-10-11): its leads, their FIND ELSEWHERE
  links (the `links` plug-ins' resolve) and THIS LEAD's copy. A later
  **Explore** room draws them (the webapp's `discoveryPanel({ page:
  'explore' })`); the answer's `network` half stays in the data model
  (`TasteResponse.network`), never asked for here.
- **Opening a connected server's album** (the webapp's peer-browse view):
  the room has no way into a peer's library, so that album's tile plays
  it instead (clause 48). The strips' section labels ("Albums from your
  library", "Albums from {server}") are the tab's name here.
- *(**"Save as playlist"** was out of scope here until 2026-10-16; it is
  clause 64's now — the log.)*
- *(**Keep a copy**, the job lookups and a copy's progress, **Open
  elsewhere** and the **Copies** list were Phase B here until
  2026-10-19; they are clauses 100–141 now — the log.)* Still out: the
  30 s **preview** (a connected server's song plays in full; the engine
  plays no external URL), **Get it** (the YouTube job — a later Explore
  room's), **Folder** copies (a row names a file), a STATE column (the
  marks ride the LEAD and the tile's artist row, clauses 113–114), a
  row's own `[copy]` verb (Keep a copy is behind `c`, the sheet and the
  tile's list), "copies allowed" / "this key refuses copies" up front (a
  connected server's key says so only when a copy runs: the refusal is
  the job's outcome, clause 110), and **Browse this server** from the
  row (the header's server switcher reaches it). *(**Changing the
  destination** was out of scope here — read-only, the webapp's —
  until 2026-10-21; it is clauses 142–148 now.)* Still out of the picker:
  the webapp's **New folder…** (a folder typed that is not there yet is
  made by the first file) and its local layout checks (the server's
  words decide, clause 144).
- **The peer cache** — "n songs kept from the last visit", "plays when
  it's back" (§2.13): an unreachable server's rows are not shown.
- **The album under the cursor** (the TuiPeers board's ownership facts,
  "play album · add the 7 you're missing", "view artist") (§2.13).
- **New artists only** (the TuiNetwork board's `[✓]`): the room sends
  `newArtistsOnly: false`, so a connected server's song may be by an
  artist you already have. The plan keeps the switch client-local (§3.2); it
  ports with a `GuiPrefs` key when wanted.
- **"browse this server ▸"**: the header's server switcher reaches a
  connected server.
- **"browse the library"** on the fallback card: the Files row is one
  click away; "choose a song…" is the card's way into the library.
- **The `limit` caps** (the server's defaults stand).
- **A text filter** over the rows (the browse bar's), and the webapp's
  "Show more" (sixteen rows a column): the table scrolls, so every row
  the server sent is in it.
- **The TUI** — no tab, no panel, no key; its Discover tab stays the
  explicit-seed lookup it is.

## Translation notes (terminal GUI)

| Record (plan · canvas) | Here |
|---|---|
| A TOOLS room "under Sonic path" (§2.9; canvas row 17) | `NavId::Discover` last in `NAV`; `draw_nav` gates it like the sonic row and hides it for a peer like the play lists; rows TOOLS 13 · Auto DJ 14 · Sonic 15 · Discover 16 · Settings on the content's last row; `G` in the Library screen's global table, recorded in `gui.tips.base`; `render`'s match over `NAV` and `Gui::wheel` get their arms |
| The room's own rows | **Its own row model** (`App.taste`), the sonic room's precedent: `List::Discover` maps to no pane (`List::tab()` → `None`), and the verbs call App methods that build the queue rows themselves — play from here, play now, add next, add to the end, play / queue / shuffle all — stamping `self.origin()` on Library rows and the peer origin on Connected servers rows (`TasteState::peer_lines`, the rows that play now). The TUI's `Tab::Discover` pane is never written |
| A federation row | Flat (`artist`, `title`, `album`, `year`, `duration`, `filepath` = the **peer's** path, `peer: {id, name}`), not the `{filepath, metadata}` envelope: the App builds the queue row's `Track` metadata from those fields, which is also the play report's snapshot |
| Two calls (§3.1) | `ApiCmd::Taste { seq, generation, half, body }` → `Event::Taste { seq, generation, half, answer }` — one command, a half on it (the D7 habit: a destination on the command, not a second command); arms in `worker::answer`, `web/api_worker.rs` and `App::apply_event` (the prefs command and the "another pick" `random-songs` command left on 2026-10-14) |
| The webapp's re-ping on 403 | `ApiCmd::DiscoveryProbe`, reading `discovery && discovery_taste >= 1` for this room (the sonic room's probe reads its own flag) |
| Segmented switch (the play-now page's two sources) | The kit's Tabs, Library · Connected servers, names only; `[gui] discover_tab` in `GuiPrefs` |
| The configuration bar: "Based on the last [window]", "More options", ⟳ "New seeds" (mStream `m.js` `discoveryHeadHtml`) | The header's controls (clause 17): the kit's Dropdown (`Last 30 days ▾`), remembered in `[gui] discover_window` (the webapp's localStorage); **more options**, a text button that opens the seeds modal — not an inline reveal: the room has no rows to unfold a picker above the table, and the modal was there; **↻ new seeds**, the accent's text button, `G`'s ask |
| The songs' whole-list verbs under each column's header, ahead of the rows (mStream `141e1a9c`) | The verbs line over the tab's table (clause 64): the Library's four, the Connected servers' three; the bottom row is gone |
| Album cards (the strips' scrollers), the two song columns | The album strip over each tab's table — 12 × 5 covers through `cover::Slot`, real pixels or the mosaic — at every height the GUI draws (16 content rows since 2026-10-16); album rows ahead of the songs only under the fallback card short of 26 rows (clauses 45–49, the log); the songs in one table per tab |
| A row's art tile, title and "Artist · Album" | TRACK · ARTIST · ALBUM (· ON) · LENGTH columns: a terminal has no art tile in chrome, and columns read faster than a joined line at 80 cells |
| The seeds sheet (PhoneSeeds) | A kit modal (`modal_frame_on`) with `[✓]` / `[ ]` rows and a modal primary Done; the window stays the header's dropdown, not repeated in the modal |
| "Start from a song…" (§2.8) | A capture of its own — a new `Capture` variant beside `Sonic` and `DjSeed`, never `Capture::Discover` (the TUI's) — with the banner over the room |
| Relative ages ("8 months ago") | The seeds modal's only: computed on the local clock from the seed's ISO `lastPlayed`; plural families (Wording) |
| Section skeletons while asking | Four DIM `░` rows in the table's own columns (clause 32) |
| Glyphs | `[▸] [»] [+] [⋯]` → `[>] [^] [+] [.]`, `•` → `*`, `▾` → `v`, the skeleton's `░` → `·`, `[✓]` → `[x]`, `—` → `-`, the refresh's `↻` → nothing (its words stand) on `legacy_conhost()` |
| The webapp's recommendation window for a connected server's row, its Keep card and its album focus (mStream `vp.js` `dmKeepChoices` / `dmKeepScope` / `dmKeepStart`, `dmLoadPeerFacts`, `dmArtistOwnership`) | The **Keep a copy** modal (clauses 106–110): `App::keep_open(KeepTarget)` freezes its subject (`KeepSubject`: the row's raw JSON or the tile's best row), asks the sizes (`ApiCmd::KeepFacts`) and, when its rows had no lookup yet, a lookup of its own; `App::keep_view()` works out everything the modal draws (`KeepView`: the choices and their `ChoiceSize`, the checkbox, the scope, the destination, the `KeepStage`); `keep_pick`, `keep_set_missing`, `keep_start`, `keep_cancel`, `keep_again`, `keep_close` are its keys |
| The webapp's `DISCOVERJOBS.jobRowState` / `manyCounts` (discover-jobs.js) | `PluginJob::outcome()` → `JobOutcome` with `action()`, `mark()`, `kept()`, `troubled()`; `CopyResult` read one field at a time; `ManyCounts` from `result.counts` (src/api/types/plugins.rs), with the webapp's cases as its tests |
| The play page's badges (`DISCOVERYPAGE.jobBadges`, `badgeOf`, `peerAlbumBadges`, `jobForAlbum`, `otherPeerJob`, `nameKey`) | Glyphs and words, no numbers: `RowJobs` (a row's keys and newest copy per scope), `row_mark` / `tile_mark` → `JobMark` with the canvas's precedence (clauses 113–115); `job_for_album`, `PluginJob::other_peer`, `name_key` ported with their cases |
| The webapp's batch lookup (`lookupChunks`) and its strip's poll (1.5 s / 4 s, ×2 to 60 s on failures) | `Client::plugin_jobs_lookup_batch_async` (chunks of 100, padded back to the rows' order); `App::keep_tick` on the App's tick, its beat from `KeepWatch`, which the GUI sets each frame (clauses 111–112) |
| The webapp's downloads strip (`trayRows`, `traySummary`: live and failed only) | The **Copies** modal (clause 117) lists every copy, finished ones too (Clear finished drops them), superseded by plug-in + key + scope + server; the indicator says words, never `traySummary`'s counts (clause 116) |
| The webapp's "Open elsewhere" section (the `links` resolve) | A `frame_at` list hung from the sheet's row (clause 140) — the lead menu's grammar the Network tab had until 2026-10-11 |

Open at implementation (decide, then log):

- **The fallback card's memory** (clause 20) is the room's: the server
  says `fallback` only for its own random seed, so once a pin stands the
  room keeps "no history yet" from the last pinless answer. Confirm what
  `seedSource` reads for pins plus history (the plan names the values,
  not the mix).

### What the tests pin

**Keep a copy, Open elsewhere, Copies (2026-10-19), the data layer:**
`api::types::plugins` — the listing's gate (the copy plug-in, its
scopes, an older server's song alone, jobs refused or unsaid, an
unavailable plug-in), a job read tolerantly (string ids, unknown
scopes and states, the server by `peer.id`, another server's album or
artist copy and not a song's), every outcome of the webapp's
`jobRowState` cases (queued and running with and without an estimate,
stopping, copied, owned, exists, done unread, failed with words,
refused by a song's error or a stop, cancelled with its counts, copied
many, already yours, copied 0 with failures failed — beside a skip too —
an artist's failed listings, counts over capped lists, counts alone,
nothing to copy and guest spots, stopped early over what landed), the
marks' precedence, a lookup's keys without the folder's, the layout
rendered as the server renders it (reserved and control characters,
blanks, dots, a missing variable, ALBUMARTIST's fallback, PEER, unknown
and malformed tokens), the destination's crumbs, https-only links,
`name_key` against the server's cases, "what you don't have" by name,
`job_for_album` album by album. `api::tests` against a canned server —
the start's body exactly the row with no scope for a song, `created:
false`, 403 / 409 / 429 / 404 keeping their words, an unreachable server,
the plug-in routes refused for a peer's client before any request, the
lookups a hundred at a time back in order and padded, the list's query,
the cancel's answer and its 409, clear, the destination, the links'
resolve body. `tui::worker` — the gate asking the destination only
behind the copy plug-in, a 401 the session's and a 500 the room's, the
sizes asked with the copy's own selectors (a compilation by its album
artist), a compilation's album queued by its album artist, every plug-in
command on the session's client. `tui::app::keep` and the App's tests —
a row's newest copy per slot and the merge by key, the marks per server
(another server's album copy never marking, a song copy from anywhere
marking, a tile wearing only its album's and artist's copies, the walk
per album), the poll's back-off; the gate's five halves each hiding every
entry point and a failed listing asked again next visit, a stale gate
reply dropped; the rows looked up unchanged once the gate opens and after
every peers answer, the old marks dropped at once and a stale lookup
filling nothing; a compilation one tile by its album artist and queued by
it; a row's start sending its row unchanged (song, album), a tile's its
best row as the album and never a song, a compilation tile's no artist
copy; a refused start keeping the choice with the server's words, a
start followed and marking its row at once, another server's live copy
hiding the primary until it ends; the modal opening on its scope's own
copy (Play your copy queuing the user's own file with the session's
origin; Start again bringing the choice back; Retry starting the job's own
recommendation), another server's finished copy leaving it idle; the
sizes and "only the albums you don't have" by name; the poll only while
live and shown (4 s, 1.5 s, the back-off and its next try), a copy ending
as news once and the rows looked up again, a page load no news; a cancel
once at a time and its 409 as a note and a refresh; the Copies list's
order, supersession, titles and servers, Clear finished, the indicator's
two halves and its gate; Retry from the list, a refused restart as a
note, an unpaired failure asking the room again; Open elsewhere asked on
open with the row unchanged, without a links plug-in nothing; a server
switch and an error clearing it all. The drawing's render tests,
locales and e2e leg come with the drawing.

Render tests at TestBackend 100×30 and 100×24, footer on and off,
injecting `Event::Taste`: the row's gate (absent without either flag,
absent in a peer session, gone with a ping that drops it, the room
yielding to Files); TOOLS at 13, Auto DJ 14, Sonic 15, Discover 16, and
Discover and Settings never on one row; `G` opens it; the tabs row's
tagline per `seedSource` and the widened fact; **no count, no meter, no
percentage and no "never played" on the page** — the answered tabs, the
queue-open view, the first ask's skeletons, the fallback card, scan
pending, a failure (no status code) and both empty states — where only a
row's m:ss and the window's days carry digits (the seeds modal behind
more options keeps its relative ages; the fallback card's "Skips never count."
is the webapp's own sentence); the two tabs, names only, and
the remembered tab (`peers` → servers, `network` → Library); the table's
columns per tab and the narrow-width order (ALBUM, then ON); albums first
with one hover verb that queues the album by name (under the fallback
card); **the album strip** (clauses 45–49, the pixel path off unless
said): at 100×30 and at the 100×24 floor (footer on and off) the tiles
with their mosaic or note covers, titles and artists over the songs and
no album row, three songs under the strip at the floor, the album rows
under the fallback card at 100×30, the layout's 16-row gate (the blanks
under the header and the tabs by height: none at 16, one at 17, both
from 18) and the card coming first (26 rows), the first ask's four
skeleton tiles at 100×30 and at the floor; best match first and the cut last tile,
**a cut standing at every width while tiles follow** (110 and 111
columns, where whole tiles would end at the edge, cut a half tile; every
content width from 25 to 170 cells keeps a cut inside the edge) and
nothing cut at the strip's end; an answer of albums alone drawing no
empty table; the wheel one tile a notch, clamped, the table's wheel
apart; `←` `→` walking the tiles and scrolling to keep the hand's tile
whole, and switching tabs off the strip, the tips saying which (the rest
tip "↓ pick an album" over the strip, `← → tabs` on a row); the queue
panel opening with the hand on a far tile scrolling it back into view;
↓ or PageDown from rest onto the strip, ↓ to the first song, ↑ or PageUp
back, nothing lit on arrival, a click stowing; a resize across the
height gate (under the card: 100×34 and 100×30) keeping the hand on its
song, and carrying an album row to its tile and back; hover brightening the title BOLD with `[+]` at its
corner and never selecting; a library tile opening in the Albums room by
click and by Enter, and `[+]`, `+`, `a` queueing it by name; a connected
server's tile playing through its server by click and by Enter (the hand
staying) (`TasteAlbum` with the peer's reach, the songs stamped with the
peer's origin, the stream through the proxy) and its `[+]` falling back
to the matched rows; the note tile keeping the owned ground under it;
the covers claimed once, a connected server's through the proxy; with a
forced kitty, sixel and iTerm2 picker the whole tiles' covers encoded as
pictures, the cut tile the mosaic, a scroll encoding only the newly
whole cover; end to end, the gui-discover-room leg (120×34, pixels off,
the queue open: the room's 67 cells) gating on the header's "more
options" with the first frame, on "new seeds" and the tabs row's "based
on songs you finished" once the answer lands, then
on the fake answer's album tile above the songs and the fake
counting the cover's `/album-art/` fetch, and its last body carrying the
window and no `remember` / `filter` / `staleDays` with no prefs call (the
fake 404s the prefs route and 400s the gone keys); no checkbox and no `f`
anywhere; the window dropdown asking both parts and saved in `[gui]
discover_window`, a remembered window asked by a new shell and an
unknown number reading as 30; every visit asking with the old rows
standing; the seeds modal's drops sent once, listed under DROPPED only
once dropped in the visit, "use again" asking without, a fresh visit
letting them go; the fallback card; the scan-pending, off and failure
lines; the Connected servers rows with an offline and a mismatched
server's left out, Enter playing through the federation proxy, and `[⋯]`,
`m` and a right click opening the sheet with the peer's origin; the
list-cursor law (nothing lit on arrival, a click stows, verbs need the
cursor up); the wheel; the glyph fallbacks; cells in French and Japanese;
**the verbs line** (2026-10-16) at 100×30 and at 100×24 under the strip
and over the table's header, footer on and off, with no bottom row and
the table reaching the room's last row; the layout's numbers (the strip
from 16 content rows with three rows under it, eleven rows at the floor
where no strip stands, the card whole at the floor and from 14 rows);
**the header's controls** (2026-10-16): the window dropdown, more options
and ↻ new seeds right-aligned in that order on the header row at 100×30
and 100×24, both tabs, queue open and closed, footer on and off; the drop
order — the header holding the controls alone, the tabs row's tagline
whole at 80 and 100 cells and dropped (never clipped) at 47, the
refresh's words gone at the queue's 47 (`↻ …` there while an ask is
out), "options" for "more options" in German there and on the legacy
console (whose refresh keeps its words and has no `↻`); in every locale
at 47 cells, on both glyph tiers and for a widened answer too, the three
controls standing as click targets two cells clear of the room's name
wherever it stands; a widened answer at 100×30 and 100×24 reading `Last
30 days → All time ▾` with the tagline whole, its tooltip leading with
the fact, `All time` at 47 and a new choice out reading alone; the
window's slot as wide as the widest window (`Last 7 days  ▾`); the refresh the accent, more
options DIM, hover brightening both, the tooltips naming `w`, `e`, `G`;
more options opening the seeds modal by click and by `e`, its flows
unchanged (nothing asked while it is up, a close with nothing changed
asking nothing); ↻ new seeds asking again by click exactly as `G` does —
one ask, the window, the pins and the drops kept, a fresh visit still
letting the pins and drops go — reading `↻ asking…` in GOLD while the
ask is out and the old rows stand; **no state line and no seeds line**:
no `•` state, no "ready to play", "nothing new", "no answer", "waiting
for the scan", "no seeds", "streams through your server", no seed's
words, "and more" or `edit` anywhere in a frame — the answered tabs, the
first ask, the fallback card, scan pending, switched off, a failure and
both empty states;
play the one accent, hover brightening; each verb by click and by key on
both tabs (`p` `A` `S`, and `s` on the Library tab only — the Connected
servers tab has no save, by click, key or act); save as playlist opening
the sonic room's prompt empty, typed letters staying letters, Enter
sending `SavePlaylist` with the Library's songs in order and no album,
Esc forgetting them; save dropping first with the queue open; the rest
tips naming `s` on the Library tab only, every rest tip fitting the
99-cell footer in all ten locales; the e2e leg gating on the verbs line's
"playlist" between the strip and the rows.
App tests: the two asks and their bodies (`library` alone, never
`network`; the window always, 30 by default; nothing but the taste asks
— no prefs); the peers ask following the Main answer, pinned to its
batch (its seeds alone, an unresolved pin left out, capped at 12) with `limit.seeds` their count, and
none for a stale Main, a Main landing twice, a random Main, a Main with
no seed, a failed or refused Main, or federation discovery gone; every visit asks, `G` keeping
the visit's pins and drops and a fresh visit letting them go, the window
kept; the exclusions capped at the server's 200, newest kept; a stale
seq and another generation fill nothing; a Library row
queues with the session's origin; **a Connected servers row queues with
`Origin { server, peer: Some(id) }`, its stream URL is the federation
proxy path, and its play report carries `peerId`**; activating
any row leaves `App.discover`, `App.now_discover` and the TUI's capture
unchanged; another pick re-asks with every random pick shown in the
visit passed over, whatever ask drew it (never a pin, never a dropped
seed, forgotten by a fresh visit; `G` passing nothing over), the one
shown before a chosen song included, and an answer with no pick left —
pinned or not — emptying the list and asking once more — once only,
the old card standing meanwhile; a window picked while
another is still asked for is asked, not ignored; a visit begun while
a 403's probe is out waits for the verdict;
the seeds modal's close sends the visit's drops once and "use again"
takes one back; the strips' order and grouping (folded server +
album + artist, a row with no album, no file, or from a mismatched or
unreachable server joining none, the best row's spelling and first art,
the cap of twelve, ties in the server's order) and a connected server's
album asked with its peer's reach and falling back to its matched rows.
The locale parity test. Expect legs
(`test/e2e/legs/gui-discover.exp`, `MSTREAM_GUI_DEMO=1`; and
`gui-discover-room.exp` against the fake's taste route: the window
dropdown, more options, the header's words, a row; first-paint gates, drain-not-sleep). `cargo test --features desktop`; `cargo check --target
wasm32-unknown-unknown`.
**Keep a copy's drawing (2026-10-20)**, render tests in `src/gui/keep.rs`
over the canvas's sample (Stage Peer B's Borrowed Tapes and Peer
Sessions, Stage Peer C's Rooftop Cypher, Brass District and Static
Lullaby): each row's LEAD glyph and token (▸ wins over a kept album; ✓
OK, ↓ ACCENT, ◷ DIM, ! GOLD; another server's album copy marks nothing)
and its words under the pointer, the held row's glyph in the selection
colours, the tiles' artist rows ("copying…" ACCENT, "✓ kept" OK, "!
failed" GOLD, the artist where none speaks) and no number anywhere on the
page; the indicator's words ending at cell 96, its tokens, its click and
`C` opening the Copies modal with its first ask, its squeeze to "↓ · !"
at 47 cells with the verbs standing, its tooltips, hidden with nothing
live or failed; the rest tip's "C copies"; the legacy console's `v ~ + !
>` LEAD, "copying..." and "+ kept" words and "v · !"; `c` on the held
row, else the hovered row or tile, the lit tile's `c` and `m`; the copy
gate closed hiding every way in — no indicator, no `c`, no `C`, no sheet
row, no list row — with Open elsewhere's own gate apart; the sheet's six
rows in the canvas's order and place (66 × 14 at row 8), `c` DIM after
Keep a copy…, the badge row "from Stage Peer B   • online" in its tokens,
no stars, its tips, Open elsewhere asking the links plug-in with its row
ACCENT and Esc folding it, `c` handing the sheet to the modal; the tile's
list hung at (16, 13), 24 wide, its rows and keys, a click elsewhere
closing it, `a` queueing from it, and Open on {server} — shown only for a
saved server — moving the session there with the album waiting; the
modal's rows from a row and from a tile cell for cell against
`TermKeepStates` (loading facts, idle, starting, refused 403 and 409
wrapped and an unreachable server's words, queued, running, stopping,
one song done, many done, already yours, a file in the way, failed,
stopped by the transfer limit, too busy, a server gone and refused,
cancelled, nothing to copy, nothing of theirs) with each state's tips; its
place (row 7 at 100 × 30, rows 4–19 at 100 × 24, anchored as if 16), its
tokens, its click targets and the inert room; the keys walking the choice,
the primary following it, ticking only-missing picking Every album by
and a radio unticking it, Add sending the row exactly as it came; a tile's
album focus sending its best row with scope album; a compilation's tile
credited to its album artist, offering Album alone and sending the row
unchanged; another server's live copy hiding the primary behind its two
ACCENT rows; the legacy console's `+ - |` frame, `(*)`, `[x]`, `>`, `*`
and "52% done"; the status notes — none while the copy's modal is open,
"copying …" ACCENT on closing it, "copied …" OK once it lands, their
tails only where they fit, server words clipped with the space trimmed;
a server's name kept whole when the words wrap; every new tips line, the
choice's tip with each verb, the tile's words and the rest tip with "C
copies" fitting in all ten locales.
**The Copies modal and Open elsewhere (2026-10-20)**, render tests in
`src/gui/keep.rs` against `TermCopies` cell for cell: the canvas's eight
copies at 100 × 30 — 76 × 22 from row 4, seven of them shown, `▲█▼` on
the right border — every row, column and token (the bar OK and DIM, ◷
DIM, ! GOLD with the server's words GOLD after a DIM "song ·", ✓ OK,
the actions ACCENT, Clear finished DIM), the held copy's two rows in the
selection colours, the pointer's DANGER `[X]`, every click target and the
inert room; at the 100 × 24 floor rows 1–22 with the bar, the wheel a
copy a notch, the head and Clear finished standing, ↓ revealing; every
state — asking, the first ask failed with `[ Try again ]` and Enter
asking again, empty, running, no estimate yet, waiting with the held row,
failed / stopped / refused (quota, busy, a server gone, a many-song
refusal), finished (Copy again only with a gap, already yours, skipped,
nothing of theirs, cancelled), a refresh failed with its rows kept and
"next try in", a cancel that lost its race with its note gone on the
next key; the keys — `x` cancelling the held live copy once and its row
turning "stopping…", Enter retrying with the server's recommendation
unchanged, Enter playing a kept song from the user's own server, nothing
for a refusal, `X` clearing, Esc closing — and the tips by the held row;
the legacy console's `+ - |`, "52% done", `~ + x >`. Open elsewhere's
list hung from the sheet's row as `TermCopies` G draws it — asking…,
CANONICAL then SEARCH with the first link held, no URL and no
non-https link, hover BRIGHT, the row ACCENT BOLD without its slab —
none, failed with Try again held and Enter asking again, the pointer
opening it unheld and a link's click opening that link; the notes
"opening …" ACCENT, "opened …" TEXT, "…: no app took it" GOLD, the
harness's URL note; the legacy `+ - |` list; the Copies and links words
fitting their cells (STATE 20, FROM 12, the table's 72, the list's 28)
and every composed tips line inside 99 cells in all ten locales. **The
e2e leg** `gui-discover-keep.exp` (run.sh's Scenario K, `FAKE_TASTE=1
FAKE_PLUGINS=1`): the real binary at 100 × 30 with the queue folded —
`G`, →, ↓, `c`, ↓ ↓ ↓ ↓ (the hand walks Change… since 2026-10-21), Enter,
Esc, `C` — with `check_keep.py` replaying
the stream frame by frame for the Keep modal's LANDS IN line, the tile's
live mark with the verbs line's "copying…", the tile's "✓ kept" with the
row's ✓ LEAD, and the Copies modal's row; the fake's state for the one
start (the row unchanged, scope album) and the lookups (≤ 100 a call,
again once the copy ended).

**The review's fixes (2026-10-20)**, tests in `src/gui/keep.rs` and
`src/api/types/plugins.rs`: the 47-cell room's indicator as glyphs in
every state (`↓` live only, `!` failed only, flush right, ACCENT / GOLD,
the words in its tip, BRIGHT + BOLD under the pointer) while the 80-cell
room says the words; LANDS IN's stand-in DIM (now "one folder per
album", clause 105), the library BOLD and the names TEXT; the held link plain ON-ACCENT on ACCENT and the hover
BRIGHT, neither bold; the held copy's action plain in its slab; the
sheet and the tile's list acting on their own row and album when a new
peers answer lands under them (Keep a copy's row sent unchanged, Open
elsewhere's ask, `a` queueing the pinned album at its new tile), and
offering neither once that answer left it out (a links list already
asked retrying its own row); a folder copy listed nowhere, lighting
nothing, polled for nothing and worded by no note; the quick beat only
over a copy's own surface; any stop word on a done row a stop — a late
`cancelled` cancelled, an unknown word "the copy stopped before it
finished" with Retry — never in your collection.

**Where files land (2026-10-21)**, clauses 105 and 142–148. The data
layer (`api::types::plugins`, `api`): a job's pin read absent, `null`, a
place or an odd shape (the job whole); `landedPath`'s cases (one song,
already yours, a file in the way, the shared folder of many, none shared,
skips only without a failure, copied with no list); the scope's
rendering (the album folder and after as "one folder per album", none
without an album folder, a refused variable as typed and flagged); the
library default (the first library, its template else the default
layout), the same place as the jobs route reckons it, the place as sent;
the PUT's body (the place, `null`) and a 400's words; the start's body
(the place, its `source` dropped; `null`) and the 409 `destination_changed`
an answer with the place in force — no `destination`, or one that will
not read, never nowhere; another account's 409 still its words; a server
from before the key (Joi's 400) asked once more without it, any other
400 kept; a missing album's year (`year`, else `year_min`, 0 none); the
worker's shapers (the failure the room's, folders by name).
The App (`tui::app::tests`, `keep`): the destination read with every
open, a failed read keeping what is drawn, an older one filling
nothing, nowhere closing the gate; a start sending the place shown, the
409 refusing with LANDS IN from the answer and a read again, nowhere on
`null`, another account's 409 not read again; LANDS IN on a running
copy's pin, no row for a pin to nowhere, an ended copy's landed folder,
else where it ran — never the account's place now; the modal's Retry
sending the account's place now, said first where the copy ran
elsewhere or nowhere known (`KeepView::again`), and nothing said where
it did not move or the copy runs; the Copies list's restart sending
where the copy ran (its pin, a pre-pin job's `result.destination`, else
the account's place); the save's answer by its seq, a later save
superseding it, its view the gate's; a read or the gate's ask sent
before a save that answered drawing nothing, a read sent after it
drawing; the lone missing album named with its year under
`{{YEAR}}`; the folders asked once each, one at a time, forgotten with
the picker. The drawing (`gui::keep`, against `TermDestination`): F1's
SOURCE row and `[ Change… ]` (cols 53–63, the slab under the hand), F2
cell for cell with its tokens, clicks and the inert room, Esc back on
Change…, `d`; P1's template radio, P2's refill and the `null` save, a
preset after it saving the place; F3 (the Custom field, the variables,
the GOLD variable in the preview, "saving…", the server's words in three
GOLD rows, an edit clearing them) and FL at the floor (rows 1–21); F4's
folder list from the server (one ask at a time, the held slab, Enter in,
Tab's common start and cycling, Shift-Tab, Esc, a click); P3's one
library on FOLDER and "not there yet", the nested GOLD hint; F5a/F5b;
F7's 409 (the start's body, LANDS IN redrawn, the GOLD line, the hand on
the radio, Add again sending the new place); F6's nowhere (11 rows, the
gate closed); F8's legacy glyphs; P4's tile, one folder per album, no
file row, no answer; a variable clicked in at the caret, the list's bar
and wheel; the Copies list's Retry sending the job's pin and its note;
LANDS IN wrapped at a crumb (the library kept, ` ›` ending the first
row, the second clipped at the right with the whole path its rows'
tooltip); "retry adds to" and "copy again adds to" under it, DIM, the
library BOLD, and the Retry sending that place; the variables row whole
and inside the border in all ten locales; Space on the picker's
buttons; one library's label no stop, a hand whose stop left on the
first stop; a picker whose view went giving way to Keep a copy, Esc
never swallowed; Use the library default standing until Save; a save's
answer its own picker's (a reopened picker neither "saving…" nor closed
by it, a refusal after its picker went the status line's, the base
folder's refusal in two GOLD rows). The tips inside 99 cells in all ten
locales ("d where files land" giving way first, never in English). **The e2e leg** `gui-discover-dest.exp`
(run.sh's Scenario W): Keep a copy on the default, `d`, ↓, "fr", Tab, Esc,
Enter — the folder completed from the fake's listing and saved —, the
place moved behind the client's back (`/_move_destination`), ↓ ↓, Enter —
the 409 — with `check_dest.py` replaying the four states and the fake's
state for the one save, the reads (exactly three: the gate, the open,
the 409's), the listing and the one start answered
`destination_changed`, nothing started. **Scenario K2** runs the Keep a
copy leg against `FAKE_NO_PIN` (a server from before the key): the same
states, the start asked twice — with the place (400), then without it
(202) — and the copy done. Each headline behaviour
was mutated once (the `null` save, the read on open, the start's place,
the 409's redraw, another account's 409, the gate without a library, a
running copy's pin, the 400's words, the server's completion) and its
test failed; every mutation restored exactly. **The slots** (2026-10-22):
`sanitize_segment`'s home fold and `Landing`'s slot runs (kept inside a
folder's words, never trimmed, dropped or merged — `{{ARTIST}}{{ARTIST}}`
two slots — a refused variable still GOLD, an artist copy's before its
cut, the slots in the path's order) in plugins.rs; `KeepSong` read off
the peer's listing (the ARTIST tag as written over the primary artist, a
genre only where one is listed, blanks none) in the worker; the rule per
scope in `tui::app::keep` (the Night Market Sampler before and after its
listing, a row that is no compilation, agreement folded as the server
folds, a song with no year a slot and no song with one no folder, a
song with no artist under the credit, a featured song's own folder, a
genre every song must name and a split tag none, the credit whatever
the songs' artists, an empty listing for a compilation and for an album
that is none, the song's artist and genre from its listing row, a
song's landing marked for its note, the artist scopes); through the stages in the app
tests (idle, the listing, starting, queued, every song by one artist,
"retry adds to", an ended copy's landed folder with none); drawn at
100 × 30 (LANDS IN `copies › From friends › ‹artist›` with the slot
DIM, the note its tooltip, the PREVIEW the same, starting and queued,
one artist's folder with no note; "retry adds to copies › ‹genre› ›
Boukmanflow - 2019" with the genre's addition, the pinned row with no
note, the legacy `<genre>`; a clipped LANDS IN and PREVIEW tipping the
whole path, " — ", then the note — the song's own words for This song,
the album's for Album); the kit's tooltip breaking a Japanese sentence
by cells and sizing its box by them. One mutation per headline rule (the song's
genre, agreement, a compilation before its listing, a genre every song
names, the artist scopes' genre, the slot's DIM, the note in the
tooltip, a slot keeping its folder, the listing carried to the view, the
home fold) failed its tests; each restored exactly.

## Deviations log

- **2026-10-05 — Extracted.** The record is the plan; the canvas is its
  picture, and these are the places this surface parts from the
  pictures, each for the reason given:
  - **Row 16, not 17** (plan §2.13): the canvas's row 17 is Settings' row
    at 100×24 with the footer on. The TOOLS label moves up to the gap row
    13, Auto DJ and Sonic path one row up with it.
  - **`G`** opens the room (the canvas named no key; plan §4.3), on the
    Library screen only.
  - **Network rows carry links and Copy only** (plan §1, §4.3): no preview,
    no Get it, no lookup, no STATE column; the menu's LISTEN and GET IT
    sections and "not for me" are not drawn.
  - **Peers rows carry `[▸] [»] [+]` and no `[⋯]`** (the canvas note gives
    them `[⋯]`; the plan's peer ⋯ is phase B's), and no STATE column — its
    every v1 value ("listen only", "copying", "plays when it's back") is
    phase B's or the cache's. FOUND ON names one server, not "n servers":
    the network answer keeps one row per lead, deduped across peers by
    its recommendation key (§2.7), and carries that row's one `peer`.
  - **No peer cache, no album under the cursor** (plan §2.13): an
    unreachable peer is a dim group line with no rows.
  - **Album rows, not album cards**: twelve-by-six covers want nine rows,
    and at the 24-row floor the room has sixteen. Albums lead the Library
    table as container rows that open in the Albums room. They are kept
    because the server leaves an album's songs out of `library.tracks`
    (§2.6) — dropping albums would drop those songs.
  - **The filter opens on "Not in 90 days"** (`fresh`, plan §2.5), not the
    canvas's "Never played".
  - **The filter's home is the bottom row's left**, not the tab row's
    right: with the queue panel open the room is 47 cells wide, the tabs
    take about 32 and the control up to 16, and a dropdown has one home
    that does not move with the width (docs/ui-kit.md, Dropdown). The
    canvas's bottom-left "7 of 18" is not drawn — the kit's "n of m"
    means a filtered list (browser-top-bar clause 3) — and the scrollbar
    shows the position.
  - **The fallback card's copy follows the seed rule** (plan §2.1): "Songs
    you play to the end — or most of the way — become seeds", since a
    stopped play past half its length seeds. Its "browse the library" is
    left out; "press r for another" lives in the tips and the tooltip,
    not inline (pointer first; key hints are off by default).
  - **No auto-pin of the playing song** (the canvas's model note pins it
    first): the room is the history page, a pin is the user's explicit
    choice in the seeds modal, and so no "seeds changed · refresh" is
    needed (plan §2.3 allows both).
  - **The seeds modal opens from `edit` and `e`**, not from Enter on the
    seeds line: the line is not a list row (the list-cursor law).
  - **"New artists only" and the "new artist" marker are not ported**
    (Out of scope).
  - **Now Playing's Discover tab is retitled Similar** (plan §2.12), in the
    now-playing contract's log.
  - **The Library footer gives up `-/+ volume`** to name `G discover`: the
    base tips line is held to 99 cells in all ten locales, and volume is
    the one hint the bar already shows by its own control (the keys still
    work). Two locales shorten a word instead as well (ru "админ", es
    "estad.").
  - **An album row opens by its name alone**, as the Albums room asks:
    `album-songs`' `artist` key filters by TRACK artist, and the row's
    credit is the ALBUM artist (or the first of several), so sent it would
    open a "Various Artists" compilation empty and a two-credit album
    half-full (mStream's db.js album-songs).
  - **Another pick draws again, up to three times, when its song is not
    analyzed** (clause 21): `random-songs` has no "analyzed only" ask, and
    an unanalyzed pin's answer says `notAnalyzed`, which the room would
    read as the whole server's scan not run (clause 81). A server-side
    "analyzed only" random pick would retire the re-draw.
  - **The fallback card's title wraps into the body's first row** when it
    does not fit one line (the queue open: 47 cells), the body giving that
    row up — the BOLD lead sentence is the card's point; compact, it
    clips.
- **2026-10-11 — The play-now page.** The user approved the webapp's
  rework (mStream `6b33af36`, `9a3e492f`; plan §2.13's 2026-10-11 lines;
  design canvas https://claude.ai/artifact/AyZ2yH6431ajvmWgxjZuxo) and
  asked for the same UI here. Their words on the first page: it was "too
  busy" — the "never played" and "matched by sound" text, every
  percentage, every count ("the user doesn't care there are exactly 32
  suggestions") — and "doing too much", the p2p network's leads crammed
  in with songs you can play. So the room now shows only what plays now:
  - **Two tabs, Library and Connected servers** (Peers renamed), names
    only. The **Network tab, its lead menu, the `links` plug-in resolve
    and the copy** leave the room, and the taste call no longer asks for
    the network half (clauses 30, 52, 63; `ApiCmd`/`Event::TasteLinks`,
    the plug-in client calls and types removed — git history keeps them
    for the later Explore room; `TasteResponse.network` stays in the data
    model).
  - **No counts anywhere**: the header says "based on songs you finished",
    the state line `• ready to play`, the tabs their names, the seeds line
    "and more", an album row no song count, the seeds modal no play count
    (clauses 11–13, 40–41, 51). The tab-row notes go (clause 42).
  - **Columns TRACK · ARTIST · ALBUM · LENGTH** on Library and **TRACK ·
    ARTIST · ALBUM · ON · LENGTH** on Connected servers; the MATCH meter
    and LAST PLAYED are gone (clause 50). At the queue panel's 47 cells
    five columns do not fit with TRACK's 24: **ALBUM goes first, then
    ON** (then ARTIST), so both tabs read TRACK · ARTIST · LENGTH there
    (clause 54). ARTIST and ALBUM take a share of the width whatever their
    words, so a hovered row's verbs cover ALBUM and LENGTH, not ARTIST.
  - **Connected servers rows are one list** — no per-server group lines,
    no online/offline, no counts, no last seen; an offline or
    another-model server's rows are simply not shown (clauses 53, 70–72);
    with nothing connected the tab is not drawn. Its rows behave like
    library rows: Enter or `[▸]` plays through your own server (the Listen
    verb's way, the federation proxy), `[»]` and `[+]` queue.
  - **The library filter is one checkbox**, "Include songs I've played
    lately" (off `fresh`, on `all`), at the bottom row's left where the
    dropdown stood — the kit has no inline checkbox element, so the
    settings rows' `[✓] label` form stands in for the 4-row opt-in card.
    "Never played" is gone; a saved `unplayed` reads as unticked and is
    asked again once as `fresh` (clause 65). The empty library offers the
    same words as a button that ticks it (clause 80); the empty-state
    sentences are the webapp's.
  - **Kept as they were**: the window dropdown, the state line (in words),
    the seeds line and modal, the fallback card, the 403 latch, the album
    rows ahead of the songs (cards still want nine rows of sixteen), the
    whole-list verbs (play · queue all · shuffle; the webapp's "Save as
    playlist" not ported). The tips name the checkbox's `f`.
  - **From the review of the port** (same day): a connected server's row
    gains `[⋯]`, `m` and the right click — the sheet with the peer's
    origin, as a queued peer row opens it (clause 62; the 2026-10-05 "no
    `[⋯]`" departure retires, the design of record gives the row "more");
    an album row's play control is `[+]`/`a`, queueing the album by name
    (clause 61); the first ask draws four skeleton rows and no "asking…"
    sentence in the pane (clause 32) — the state line keeps its one word
    `• asking…`, because the kit's state line names every state the room
    is in, as the webapp's busy page marks itself; a failure reads
    "couldn't reach this source" with Try again and keeps the server's
    words for the tooltip (clause 83); "streams through your server"
    stands only beside rows (clause 41).
- **2026-10-11 — The album strip.** The user asked for the webapp's
  horizontal album scroller here too — its "Albums from your library" /
  "Albums from {server}" strips of album cards with real covers (mStream's
  `discoveryPlayHtml`) — and approved this design (clauses 45–49):
  - **A strip of album tiles over each tab's table** — Library the
    answer's albums, Connected servers their playable rows grouped by
    server + album + artist, both best match first and capped at twelve,
    the webapp's `peerAlbums` and `ALBUM_STRIP_MAX` — and the album
    container rows leave the table where it stands. The 2026-10-05
    "album rows, not album cards" holds only below the strip's height:
    the floor's sixteen rows still cannot hold seven rows of tiles and a
    table, so there the albums stay rows (clause 49, the gate measured
    from the layout: the table keeps three rows under the strip from a
    20-row content area; the installer's 100×30 window has 22 or 23).
  - **Tiles 12 cells wide, covers 12 × 5**, one cell apart: the webapp's
    cards worn terminal. The covers are real pixels on kitty, sixel and
    iTerm2 through the album wall's and the queue panel's `cover::Slot`
    (paced encodes, a slot per cover keyed by its art, the Over-pixels
    mosaic under an overlay), the ▀-mosaic where pixels are not drawn,
    the note tile (the empty slot frame with `♪`) where an album has no
    art. A cut tile is always the mosaic: a picture cannot be cut, and a
    squeezed one would read as a smaller album, not as more to come.
  - **The verb at the corner sits beside the title, not over the cover**:
    the webapp's play button floats over the art, but a picture's cells
    are skipped by the terminal writer, so text over a cover would not
    land (or would turn the cover to the mosaic for as long as the
    pointer rests). The title gives its tail to `[+]` instead.
  - **A connected server's tile plays where the webapp's opens**: the
    webapp follows the card into a peer-browse view, which this room has
    no way into (the header's server switcher reaches a peer, and the
    room is hidden in a peer session, clause 1). So its click and Enter
    play the album and `[+]` queues it, its songs asked of that server
    through the parent's federation proxy with the rows' track artist —
    `ApiCmd::TasteAlbum` gains the peer, its reach and `play` — and a
    server that will not list it leaves the rows that matched, as the
    webapp's queue button does. The browser build, with one client,
    queues the matched rows at once.
  - **`+` queues on a tile** as the brief asked, beside `a`: it is the
    volume key everywhere else and stays so off the strip. `←` `→` walk
    the tiles while the hand is on the strip and switch tabs everywhere
    else; the tips say which (`gui.tips.discover_tile` /
    `discover_peer_tile`, ten locales).
  - **No labels over the strips**: the tab in front names the source, and
    a label row would cost the floor-adjacent heights a table row.
  - **After review, the same day**: a cut tile stands at every width
    while tiles follow — where whole tiles would end exactly at the edge
    the last one gives its place up to a half tile (clause 47); the lit
    tile is kept whole in view when the room narrows; a resize across the
    height gate carries the keyboard's hand to the same song, album row
    or tile (clause 49); PageDown and PageUp stop on the strip like ↓ and
    ↑; the slab sits on the tile's title line alone; an answer of albums
    alone draws no empty table; the rest tip over a strip reads "↓ pick
    an album" (`gui.tips.discover_strip`) and the row tips name `← →
    tabs` (ten locales); the note tile keeps the owned ground under it.
    **Known limitation — library albums are drilled and queued by name
    alone** (clause 48), inherited from the container rows and the Albums
    room: mStream's `album-songs` has taken an `album_artist` selector
    since `a63f44a0` (2026-09-08) and the webapp's library card sends it,
    but the player's `album_songs` and `LibraryNode::Album` cannot carry
    it yet (its `artist` is the TRACK-artist filter, the 2026-10-05 log).
    Two same-named albums by different artists open and queue together;
    the strip, showing both credits side by side, makes that visible. The
    fix is the drill's, not the strip's: plumb `album_artist` through
    `LibraryNode::Album` and `album_songs` for the Albums room and here
    at once. The 12 × 5 cover box's letterbox and stretch (clause 46) are
    likewise recorded rather than changed: the brief fixed the box.
- **2026-10-14 — The stateless page.** The user's decision, in their
  words: "This feature should be simple: pull in song history; pick a
  batch of random seeds and pull in discovery suggestions. It doesn't
  matter if the seeds are different every time the page opens, that's
  actually a feature. Getting new results every time is the expected
  behaviour." The seed picker stays ("a nice power feature for users who
  want to make targeted discovery queries"), as does the window. The
  server went first (mStream `8dcfad0e`: `POST …/taste` stateless —
  `remember`, `filter` and `staleDays` a 400 — and `GET`/`PUT …/prefs`
  gone, a 404); every client follows the same rules:
  - **No prefs**: `ApiCmd`/`Event::TastePrefs`, the `TastePrefs` /
    `TastePrefsPatch` types, the client calls, the once-a-session read,
    the restore's `PUT` and its "couldn't restore the seed" note are
    removed (clauses 13, 16). The request carries the window, the pins
    and the visit's exclusions only.
  - **The window is remembered locally**, in `[gui] discover_window`
    beside `discover_tab` (default 30; clause 15), the webapp's
    localStorage and the phone's settings store being the others'.
  - **The library filter's checkbox goes**, with its `f`, its tooltip,
    the empty library's "include played" button and the
    `TasteFilter` type and `TastePart::Library` ask (clauses 65, 80); the
    bottom row keeps play · queue all · shuffle, and the empty-library
    line stays.
  - **Pins and drops are the visit's** (clauses 3, 13, 14): a dropped
    seed joins a visit-local exclude list, "use again" takes it back,
    both re-asking; a fresh entry into the room clears pins, drops and
    passed-over picks; `G` with the room up and the way back from a pick
    keep them. DROPPED lists the answer's `excluded` rows for this visit's
    drops.
  - **No reuse** (clause 3): the 5-minute reuse clock (`TASTE_REUSE`,
    `arrived`, `taste_open_at`) is gone — every visit asks, and the old
    answer stays drawn until the new one lands. `G ask again` is the same
    plain ask; the server's random batch is what makes it new.
  - **Another pick** (clause 21) — settled the same day for every
    client: the server's random fallback, salted by user + UTC day, gave
    the same song all day, so a plain re-ask could not give another pick;
    it is now drawn **afresh on every call**, honouring `exclude`. Every
    client's another pick re-asks with the random picks shown in this
    visit in `exclude` — a visit-local list beside the drops (here
    `skipped`, fed by `shown_random`, which gathers every random pick
    the card showed, whatever ask drew it; not listed as a drop),
    emptied on a fresh entry — and, when the answer comes back with no
    pick left (no seed, `notAnalyzed` with nothing to show — pinned or
    not, as the webapp's `noPickLeft`), empties the
    list and asks once more, so the card starts over instead of
    stranding on a false scan-pending card. The card's pin, if any, is
    let go, as built. The `random-songs` draw
    (`ApiCmd`/`Event::TasteRandom`), the random pin and the 2026-10-05
    "draws again up to three times when not analyzed" re-draw retire —
    the server's random seed is analyzed by construction.
  - **One batch per visit across the halves** (clause 30) — settled the
    same day for every client: each call draws its own batch, so a peers
    ask sent in parallel with Main was built from a different batch than
    the one the seeds line shows. The peers half now goes out **after**
    the Main answer lands, pinned to all of its seeds and nothing else
    (as the webapp's `peersRequest`: a resolved pin is a seed already, an
    unresolved one would push a seed out at the cap) with `limit.seeds`
    their count; the server's `PIN_CAP` rose
    8 → 12 (= the seed cap) so a whole batch fits. A random Main sends no
    peers ask (the server asks no connected server for a random seed),
    nor does a stale Main (the seq guard), a failed or refused one, or
    one with no seed. The user still picks at most 8 songs (clause 14);
    `TasteRequest` gains `limit` (`TasteLimit { seeds }`), sent only on
    the peers ask.
- **2026-10-16 — The verbs line; save as playlist.** The user's ask, in
  their words: "make the terminal app reflect the webapp". The webapp
  (mStream `141e1a9c`, "Discover's whole-list verbs sit under the column
  headers") moved "Queue all · Shuffle · Save as playlist" under "Songs
  from your library" and "Queue all" under "Songs from {server}" —
  directly under each songs column's header, ahead of its rows, "so the
  actions are where the eye lands first" — and (`c01a8327`) keeps its
  configuration bar fixed while the page scrolls. Here:
  - **The verbs line** (clause 64) takes the bottom row's verbs to a line
    of their own right over the tab's table: under the album strip where
    it stands — the webapp's songs header follows its album strips, so the
    verbs sit with the songs they act on, not with the seeds line's
    configuration — and under the seeds line's blank at the floor. It
    starts at the room's left edge, as the webapp's do, so no control
    moves for the width. **The bottom row goes**; the table runs to the
    room's last row. The arithmetic keeps every number the room had: the
    verbs line costs the row the bottom row gave back, so the strip still
    stands from a 20-row content area with at least three rows under it
    (100×30: five rows with the footer on, six off), the card stands whole
    from 18 rows and with the strip from 28, and the floor keeps seven
    rows with the footer on (clauses 49, 90).
  - **Save as playlist is ported** on the Library tab (clause 64; it was
    out of scope since 2026-10-05): the house's name prompt — the sonic
    room's save (`App.sonic_playlist_name`, its modal and its keys), not a
    new one; the Playlists room's `PlNew` dialog creates an empty
    playlist and is not a save — opened empty as the webapp's
    `openSavePlaylistModal` opens, saving the tab's listed songs in the
    server's order (the webapp's `allTracks`; the album rows are
    containers and are not saved). The App's prompt learned whose songs
    it saves: `App.playlist_save_files`, set by `taste_save_playlist`,
    taken on Enter, dropped on Esc and whenever the sonic path opens the
    prompt, so the sonic save still saves its stops. Key **`s`**, the
    sonic room's save key, only where the Library tab has songs (the
    room's `p` / `A` / `S` shadow the transport the same way); every rest
    tip gains "s save as playlist" there, the fallback card's included
    (its Library tab saves too).
  - **The Connected servers tab keeps play · queue all · shuffle**, though
    the webapp's servers column offers Queue all alone: they stood here
    first and do what they say through the federation proxy. It has **no
    save**: the webapp's servers column has none, and a connected server's
    songs are not this server's to save.
  - **The configuration bar** needed nothing: the header, the state line,
    the tabs and the seeds line have always stood fixed above the table,
    which scrolls alone — the webapp's sticky bar is this room's shape
    already.
- **2026-10-16 — The header's controls; no state line, no seeds line.**
  The user's words: "The terminal/desktop needs some work. First, there is
  no obvious refresh. Remove the 'ready to play' line. Make the seeds
  hidden but discoverable like the webapp." The webapp's reference is its
  configuration bar (mStream `m.js` `discoveryHeadHtml`): "Based on the
  last [30 days]", a "More options" toggle that reveals the seed picker,
  and a ⟳ "New seeds" button — the seeds are never on the page itself.
  Here:
  - **The header carries the controls** (clause 17): after the room's
    name, right-aligned, the window dropdown (unchanged,
    `w`), **more options** (a DIM text button that opens the existing
    seeds modal — seeds on or off for the visit, pins, "Pin a song…",
    "Pin the playing song"; `e`, its key since 2026-10-05, renamed in the
    tips) and **↻ new seeds** (the accent's text button, the header's
    one primary: `G`'s plain ask, the visit's window, pins and drops kept
    — `G` still works). The refresh is the obvious one the user asked
    for; the transport's repeat button wears `↻` too, in the bar's
    own frame, a different place and shape, and the header's always
    carries its words but at the queue's width. Short of width the
    refresh's words drop first (the glyph stays), then "more options"
    shortens to "options", then a widened window's mark, then the room's
    name: at the queue panel's 47 cells English keeps "more options  ↻",
    German, French and Russian read "options  ↻", Chinese keeps all three
    whole.
  - **Review (same day): the tagline moves to the tabs row.** Beside the
    three controls the header had 29 cells left at the design size's 80:
    "· based on songs you finished" was one short, the `counters` and
    `pinned` phrases further, and the widened fact — which only the state
    line had carried — first stood at 160 columns, while the dropdown
    still read "Last 30 days" over an all-time answer. So the tagline is
    the tabs row's, right-aligned (clause 11: whole at 80 and 100 cells,
    dropped at 47); the dropdown says a widening itself, `Last 30 days →
    All time ▾`, or `All time ▾` short of width, its tooltip leading
    with the fact (clause 15), in a slot as wide as the widest window so
    choosing one moves nothing; where the refresh's words dropped an ask
    reads `↻ …`, never the colour alone; and on the legacy console, where
    six locales' controls would have drawn over the room's name at 47
    cells, the name gives way, last (clause 17). The e2e leg is back at
    120 columns, gating on "more options" and "new seeds".
  - **The state line is removed** (clause 41). What it alone said moved:
    "asking…" to the refresh, which reads `↻ asking…` in GOLD while an ask
    is out (the old rows standing, as before); the widened fact to the
    tagline and the window dropdown (review, above); "no seeds" to the fallback card, which already said it; the
    scan-pending, switched-off, failure and empty states were the table
    region's own lines already. "Streams through your server" is
    **dropped**: a connected server's `[▸]` tooltip says "Listen through
    your server". "From your play counts" is dropped (the `counters`
    phrase says it).
  - **The seeds line is removed** (clause 12): the seeds are reached only
    through more options, as on the webapp; its `edit`, "and more" and the
    `random` marker went with it (the modal keeps `picked`).
  - **The freed rows go to the strip and the table** (clauses 49, 90):
    with the header and the tabs alone above it — and the blank under
    each as the height allows (both from 18 content rows, the header's at
    17, neither at 16) — the strip stands from 16 content rows with three
    rows under it: **at the 100×24 floor, footer on (16) and off (17)**,
    three songs under the strip, and at 100×30 seven or eight (five or six
    before). The floor gives four song rows for the album tiles; where a
    tab has no album the floor's table has eleven rows (seven or eight
    before). The fallback card stands whole at the floor now, and with the
    strip under it from 26 rows (28 before); under the card at the usual
    sizes the album container rows are the table's first rows — the one
    place they still show. The verbs line (2026-10-16, above) stays right
    over the table.
  - **Ten locales** for `more_options`, `options_short`, `new_seeds`,
    `new_seeds_tip` and the amended rest tips; twelve keys left
    (Wording). Five locales name `e` by the short "options" in the rest
    tips so every rest tip with the save words fits the 99-cell footer;
    French says "autres graines" ("other seeds") on the button and in the
    tips for the same reason.
- **2026-10-16 — Connected servers' covers.** The user's ask, in their
  words: "add the album art from federated servers for all apps".
  mStream's federation discovery responder now names each row's cover as
  `albumArt` (the peer's own `album-art` file name; `null` without one),
  the parent passes it through, and the taste route's peers rows carry
  it (mStream's plan §3.1 "Peer covers", §2.13's 2026-10-16 note). Here:
  - `TasteLead` reads it as `album_art` under the server's own
    `peerArtName` rule (`^[a-zA-Z0-9_-]{1,100}\.[a-zA-Z0-9]{1,7}$`, as the
    webapp and the phone hold it; anything else is no cover), the older
    spellings after it; the strip's peer
    tile takes its album's first row that names one (clause 46) and
    claims it once through the federation art proxy; no name keeps the
    note tile and asks nothing.
  - A connected server's row queued from the room carries the name as
    its track's `album-art`, so the queue panel and Now Playing draw its
    cover from that server the way a peer browse row's is drawn, and the
    play report's snapshot names it (`artFile`).
  - **Full size, no `?compress=s`** (superseded 2026-10-17, below: the
    player asks `l`). The webapp asks the proxy's small
    (92 px) rendition only for its 40 px song rows; its album cards ask
    full size, or `l` (256 px) with compressed album art on. The player
    caches a cover once per file name and shares it with Now Playing and
    the queue panel, and scales it to terminal cells itself, so it asks
    the one full-size file.
  - **A failed cover waits before it is asked again.** An unanswered
    fetch (a dropped connection, the proxy's 502/504 when the peer went
    away after the taste answer, a 401) gives its cache slot back, as
    before, but the file is not asked again for 15 s (`ART_RETRY`): the
    GUI claims covers on every draw, ten a second, and without the pause
    every visible Connected-servers tile re-asked the failing hop on
    each frame. A 404 or undecodable bytes still settle as no cover.
  - e2e: the fake advertises `federationDiscovery`, answers the peers
    ask with one server's row carrying `albumArt` and serves the proxy
    route; the leg switches to Connected servers and run.sh asserts one
    proxied fetch of exactly that file. The leg's answer words are gated
    in order (header, strip, verbs line, rows) with the refresh's "new
    seeds" allowed anywhere among them — the peers half lands after Main,
    so "new seeds" may paint after Main's rows.
- **2026-10-17 — Connected servers' covers ask the compressed rendition.**
  The user's ask, in their words: "Use the compressed covers for the
  album cards and terminal". Every cover of a federated peer now asks
  `?compress=l` — the peer's `zl-` file, at most 256 px a side, the one
  the webapp's album cards ask for (`urls::PEER_ART_COMPRESS`) —
  however the peer is reached:
  - **Through the parent's art proxy** (`urls::peer_art_url`). The
    proxy forwards exactly the `compress` query (mStream's
    federation-browse.js `FORWARD_QUERY_ART`).
  - **Over the peer's own tunnel** (multi-server clause 27), its own
    `/album-art/<file>?compress=l` (`urls::direct_peer_art_url`): a
    reach for a peer origin carries `Reach::compressed_art`, and a peer
    session's client is marked by its peer identity (the direct Connect
    and Retarget carry no `peer`). Without it the same peer's covers grew
    to full size the moment its tunnel came up — right after queueing an
    album from the strip, since tunnels follow the queue (clause 38).
  - The peer's `/album-art` route serves the original when it holds no
    `zl-` file, so an older or uncompressed peer still answers with a
    cover. Through a tunnel bridge the loopback token joins the query
    after it (`…?compress=l&__lt=…`).
  - **Scope.** Every cover a peer serves: the Connected-servers strip,
    the queue panel and Now Playing for a peer's track, and — when the
    browsed server is itself a peer (multi-server clauses 26–27) — that
    session's whole Library, album wall, queue and Now Playing. Only a
    server of the user's own (a saved server, a Quick Connect, the
    bundled server) still serves its `/album-art/<file>` full size.
  - **The accepted trade-off.** The strip (12x5 cells), the album wall's
    12-cell tiles and the queue panel's 6x3 thumbnail fit within 256 px,
    and the ▀-mosaic (and the cover visualizer's grid) is drawn from the
    128 px thumbnail anyway. The larger pixel surfaces are not: a peer
    track's Now Playing cover and the mini player's, drawn in pixels,
    re-decode the kept source (`Art::source`: `tui::graphics` on kitty,
    sixel and iTerm2, the window GUI's textures) and now scale a 256 px
    rendition up where they drew the original before; a peer's cover
    there can look softer on a HiDPI screen.
  - **One cache slot per file name.** mStream names a cover by its
    content hash, so federated servers holding the same rips share
    names, and the cache (keyed by name alone) holds whichever copy was
    asked first: the session's own Now Playing can show a peer's 256 px
    rendition for the rest of the session when the peer's was fetched
    first, and a peer's tile the session's original the other way
    round. Kept, not keyed by (file, rendition): same name, same
    picture, one fetch.
  - e2e: run.sh also asserts that every proxied fetch the fake recorded
    carries `compress=l`. The direct-tunnel URL is pinned by unit tests
    (`a_direct_peers_strip_cover_is_the_compressed_rendition_too`, the
    worker's `a_peer_sessions_covers_ask_the_compressed_rendition_over_either_transport`).
- **2026-10-19 — Keep a copy, Open elsewhere and the Copies modal.** The
  user approved the design canvas "Discover plug-ins · phone & terminal"
  ("the designs look good, start the work") with three decisions: the
  room shows **no counts or percentages** — words and glyphs only, the
  numbers in the Keep and Copies modals; **compilations**: connected-server
  rows may carry `albumArtist` (mStream adds it server-side the same day),
  the strip groups by server + album + (album artist, else track artist),
  a compilation tile's artist line is its album artist, Album is offered
  when the row names its album and a credit, and the album job sends the
  row unchanged — the server copies the whole compilation (the plan's
  shared decision had accepted the per-track-artist copy; the user chose
  the whole album); the active tab stays the kit's filled slab, as
  shipped. Clauses 100–141 describe the canvas; Phase B's entry under Out
  of scope retires. Where this surface parts from the briefs and the
  webapp, and why:
  - **The row's mark rides the LEAD** (the brief's Option 1): no STATE
    column fits at 47 cells, and LENGTH never goes (clause 54) — so
    Option 2 (LENGTH swapped for a state) is not taken. The LEAD was the
    playing `▸`'s alone (clauses 50, 55); it is shared now, and the
    playing `▸` wins it (clause 113).
  - **The indicator says words, not "copying 2 · 1 failed"** (brief §5
    F): the shared decision and the user's "no counts anywhere" win;
    "how many" is the Copies modal's (clause 116). Its GOLD part follows
    the room board's legend — a failed copy (its state failed, or nothing
    copied with failures) — not every GOLD row of the Copies list (the
    Copies board's note), since a stopped copy is done and its row and
    tile say so.
  - **Keep a copy… takes Add to playlist's slot in the sheet** (a peer's
    song has no playlist row, clause 62), Open elsewhere… goes last; the
    sheet's badge row names the server and "• online", no stars.
  - **`c`** keeps a copy (free in the GUI and the TUI keymaps); **`C`**
    opens Copies while the room has the keys — the queue panel's `C`
    clears the queue only while the queue has them; **`m` and a right
    click on a connected server's tile** open the tile's list (clause 48
    said they did nothing). The tile's list is a new kit piece (the
    context list, docs/ui-kit.md).
  - **The destination is read-only** in v1 (clause 105): the webapp keeps
    the picker. On the stage the default is `demo`, the user's checkout, so
    no Keep flow is pressed there.
  - **Sizes**: the format is the file's extension ("mp3"), not the
    connected server's metadata read the webapp makes; "you have 3" is the
    library's albums of the artist — the canvas's sample — where the
    webapp's "you have" counts the connected server's albums the library
    holds by name; the checkbox stands under Every album by whatever radio
    is picked (the canvas), where the webapp shows it with the artist
    choice alone. "Only the albums you don't have" is still reckoned by
    name, as the webapp's 2026-10-19 fix does.
  - **A tile of a compilation offers no "Every album by"**: its rows'
    track artists are not its credit, and the job would copy someone
    else's albums.
  - **The modal opens on its preselected scope's copy from this server**
    when one stands (a reopened row reads its real state, the webapp's
    `dmJobRow`); picking another radio never folds the choice — the
    server's `created: false` answers a start that a live copy covers.
  - **Retry of a copy whose server is "no longer paired"** asks the room
    again instead (the row's new id; clause 128).
  - **The Copies list lists finished copies too** (the webapp's strip
    holds live and failed ones; its Downloads view the rest): the
    canvas's finished, cancelled and race states need them, and Clear
    finished empties them. A finished copy a newer one of the same
    plug-in, key, scope and server supersedes leaves — by server too,
    since the same album from two servers is two rows.
  - **"✓ nothing to copy · you have it all"** is the canvas's line for a
    done copy with nothing copied, skipped or failed; the webapp avoids
    "you have it all" there ("nothing came back is not you have it all").
    Today's server cannot end a copy that way — it fails an album with no
    songs and an artist with no albums, and an artist who only guests
    there reads "nothing of theirs to copy" — so the canvas's words are
    kept; revisit if a server learns to.
  - **The status notes are worded by the GUI** from the App's news of a
    copy seen live and then finished (`App::keep_take_news`), only while
    that copy's Keep modal is closed (clause 118).
  - **Lookups and the poll ride the listing** (the copy plug-in listed,
    jobs allowed), not the full gate: marks of copies made before the
    account lost its upload right still read true; nothing is asked of a
    server without the flag.
  - **A compilation tile's album is queued and played by its album
    artist** (`album-songs {album, album_artist}`), where it was asked by
    a track artist (clause 48) — the tile is the whole album now.
  - **The browser build** has one client, the session's: the Keep modal's
    connected-server sizes are unknown there (`KeepFacts` with the peer's
    halves `None`); every plug-in call works as natively.
  - **Built the same day, the data layer only** (types, client, worker,
    App state, tests); the drawing — sheet rows, the tile's list, the
    modals, marks, indicator, links list, notes, tips and the ten locales —
    is next, and the contract's "What the tests pin" grows with it.
- **2026-10-20 — Keep a copy drawn** (the boards `TermRoom`,
  `TermEntryPoints`, `TermKeepModal`, `TermKeepStates`; the Copies list and
  Open elsewhere's list are the next pass's). Where it parts from the
  canvas or fills what it leaves open:
  - **"Open on {server}"** — left open by the canvas ("the album as that
    server holds it") — is the session's own switch to that saved
    connected server, the Albums room, and the album opened once that
    server answers; the row is hidden while the server is not saved or is
    parked. It leaves the room, as Settings › Servers' switch does.
  - **A lit connected-server tile names "m more" with the copy gate
    closed too** (`gui.tips.discover_peer_tile_more`): its list stands
    whatever the gate (clause 103), so the clause-119 rule "every tip
    reads as before" gives way to "name a key where it works".
  - **The keyboard's stop on a button** wears the selection slab; on a
    radio or the checkbox, the canvas's ACCENT BOLD label. The canvas
    draws no focused button; in a job's outcome the hand rests stowed and
    Enter means the primary, so its drawn states read exactly.
  - **The WHAT and LANDS IN labels' column** moves right for a locale
    whose label is wider than the canvas's ten cells (French
    "DESTINATION", Italian "FINISCE IN"); the radios, the folded line and
    the crumbs follow it.
  - **The folded line of an only-missing copy** reads "Every album by
    {artist} · 1 album" (its size), the canvas drawing only the plain
    artist copy.
  - **The status notes** carry their kit tokens — busy ACCENT, landed OK,
    an outcome DIM, an error GOLD — as App messages of two new kinds
    (`MessageKind::Busy`, `Success`), so the next message replaces them as
    clause 118 says; a note is fitted to the window it lands in and is not
    re-fitted on a resize. The busy note lands when a Keep modal closes on
    a copy still moving.
  - **"C copies"** rides a rest tip only where the whole line still fits
    the 99-cell footer (the Library tab's "s save as playlist" comes
    first).
  - **The Copies modal opens as its frame** (title, `[X]`, Esc), asking
    the list; its rows are the next pass's (`TermCopies`). Open
    elsewhere… asks the links plug-in and wears its open state; its list
    is the next pass's too.
  - **The tile's state word** for a cancelled copy is German "Abbruch",
    and the other locales' words fit the 12-cell tile whole.
- **2026-10-20 — The Copies modal and Open elsewhere drawn** (the board
  `TermCopies`; the first pass's "next pass" items). Where it parts from
  the canvas or fills what it leaves open:
  - **Copy again stands only where a gap is left** (`App::copies_action`:
    a song failed, an album's listing failed, songs not reached) — the
    canvas draws Peer Sessions' "2 copied · 1 already yours" with no
    action and the artist copy with a failure with one, and its note says
    Copy again "takes only the gaps". The Keep a copy modal keeps offering
    it after every many-song copy (`TermKeepStates`, unchanged).
  - **The race panel's order**: the canvas draws the finished Borrowed
    Tapes above the running copy; the build keeps clause 117's order
    (running first), the note still bottom-left.
  - **A stopping row's step**: "stopping after {step}" with the plug-in's
    whole step ("stopping after song 2 of 3 · Late Train Verse"); the
    canvas trims it to "song 2 of 3", a word the client cannot cut
    reliably from the plug-in's own line.
  - **Words the canvas leaves open**: "finished" for a done copy whose
    result this client cannot read; "you had them all, nothing copied"
    for an owned album or artist copy (the canvas draws the song's);
    "couldn't cancel / clear / start: …" in GOLD in the race note's slot
    when the server refused those.
  - **"next try in N s"** follows a refresh's error only while a copy is
    live: the poll runs only then (clause 112), so with nothing live
    there is no next try to count down.
  - **The held running row's bar** draws in the selection's colours with
    the rest of its row; ↓ or ↑ with nothing held picks up the first copy.
  - **The title's `[X]`** stands a cell in from the border, over the rows'
    `[X]` column, as the canvas draws it; the kit's other modals keep it
    flush.
  - **Two locales' STATE words** are shorter forms so "✓ …" fits the
    20-cell column: Italian "nella collezione", Japanese "コレクションに追加"
    and "コピー対象なし" (the Keep modal keeps its longer words); Spanish,
    French and Portuguese links words are short forms for the list's 28
    cells.
  - **Open elsewhere's list geometry** (the canvas draws one size): 30
    cells inside, wider for a longer label but never over the row's
    label; it rises from one row under its row to at most the sheet's
    title row and scrolls past that. A click on the row opens it with
    nothing held; the keys (Enter or →) hold the first link. The row
    gives up its selection slab while the list hangs.
  - **"opened Deezer"** is a new `MessageKind::Plain` (TEXT); an opener
    that could not start reads "no app took it" like one that found
    nothing; under `MSTREAM_NO_OPEN` the note names the link's URL
    ("Deezer · https://…"), the harness's seam (test/e2e/README.md).
  - **The legacy console's scrollbar** keeps the kit's `▲ █ ▼ │` (the
    console's code page has them) beside the modals' `+ - |` frames.
  - **The fake's copies walk on the wall clock** (queued 1 s, running
    6 s) instead of one step per list ask, so the e2e leg sees a copy
    live in the room between its 4 s polls.
  - **Dead code**: `JobMark::live` went (nothing draws by it);
    `JobOutcome::kept` is the tests' (clause 120 in one word) — the
    drawing matches the two kept outcomes by name.
- **2026-10-20 — The review's fixes** (a review of both passes against
  the boards). What changed, and what now reads differently:
  - **The indicator's squeeze follows the room's layout**, not whether
    its words fit: the room whose table gives ALBUM up (the queue-open
    47-cell room) draws `↓`, `↓ · !` or `!` in every state, as
    `TermRoom`'s "every state" table draws it; before, a live copy alone
    read "copying…" there (8 cells fit) and a failure alone "! failed".
    A wider room still squeezes where the verbs would leave the words
    too little.
  - **"each album" is DIM** in LANDS IN, as `TermKeepStates` draws it
    (the stand-in is a hint, not a folder) — matched in whichever crumb
    the layout puts it.
  - **The held link and the held copy's action** draw in the plain
    selection colours, the hovered link BRIGHT without bold, as
    `TermCopies` draws them (the kit's lists elsewhere keep BRIGHT +
    BOLD hover and a BOLD held label).
  - **Folder copies leave Discover entirely** (clause 117): the server's
    list carries the webapp's peer-browser folder copies too; they were
    listed as "song" rows with a Retry that did nothing (a retry may
    never offer Folder). A scope this client does not know goes the same
    way.
  - **The sheet and the tile's list pin their row and album**
    (`KeepPin`, clauses 102–103): the positions they kept named another
    song once a new peers answer landed under them, so Keep a copy sent
    another row's JSON and Open elsewhere asked links for another song.
    The links list's Try again sends the row it first asked for
    (`App::links_retry`).
  - **Any stop word is a stop** (clause 120): a newer server's word
    reads "the copy stopped before it finished" with Retry (new strings
    `gui.discover.keep.stopped_other` and `gui.discover.note.why_other`
    in the ten locales), a late `cancelled` on a done row is cancelled.
  - **The quick beat** (clause 112) holds only while a copy's own
    surface shows it — the Copies modal, or a Keep a copy modal whose own
    copy moves; a modal still choosing beside another copy keeps the
    room's 4 s.
- **2026-10-21 — Where files land in the terminal** (the user's
  2026-10-20 "In the terminal UI I do not see a way to edit the folder
  it will go to", built after the webapp's redesign so the two match;
  the `TermDestination` board, mStream 95f2205a + a7d4459f). Clause 105
  is no longer read-only; clauses 142–148 are the picker, its save, the
  start's place and the 409, nowhere and the legacy console. Where the
  build parts from the kit or the board:
  - **A fifth radio** where a library's own template is none of the
    three presets ("music's template") — the kit says 2–4 radios, then a
    list picker; the presets are what the user picks between, and the
    template is the library's own default, so it stays a radio.
  - **Folder completion comes from the server** (the file explorer on
    the user's own server, one ask at a time, on the worker) — the kit's
    Path input completes LOCALLY (docs/ui-kit.md): the destination is a
    library on the server, never this machine's.
  - **No layout is checked here**: the webapp validates as it is typed;
    the terminal shows the server's 400 words in GOLD (clause 144), the
    preview flagging a variable the view does not list.
  - **A job's LANDS IN** reads its pin, then where it landed — never the
    account's place now, which is what the modal drew before (clause
    105); a copy that landed nothing reads where it ran, keeping the
    frame's row, where the webapp draws no "In" line.
  - **"d where files land"** joins the choice's tips and the other
    server's (`d` works there too); a locale's line past 99 cells loses
    that part first (German's "Song hinzufügen" line does).
  - **The Copies note** for a Retry the 409 refused takes the spacer
    row above the footer when Clear finished would cut it.
  - **The review's pass** (the same day): the modal's Retry and Copy
    again send the account's place now and say it first ("retry adds
    to …"), as the webapp's Retry and Add again do — where the build
    had sent the job's pin and let the 409 bring the choice back, two
    refusals for one move; the Copies list, which shows no place, still
    sends where the copy ran (a pre-pin job's `result.destination`
    before the account's), so its 409 sends the user to that Retry.
    **LANDS IN wraps** onto a second row instead of clipping at the
    right (the webapp's path bar never ellipsises, SPEC §13), the
    library always shown; the picker's PREVIEW keeps the board's one
    row cut from the left. A refused save whose picker is gone is a
    status note, never lost.
- **2026-10-22 — A variable each song fills is a slot** (found live the
  same day — the phone's card said "Files go to copies › From friends ›
  Boukmanflow" for an album copy of the compilation Night Market Sampler,
  whose three songs the server files under Color Out, Boukmanflow and
  ALM; the user said "fix it"; the rule is the three clients' own,
  clause 105). Where the terminal parts from the other two:
  - **The note is LANDS IN's and the PREVIEW's tooltip**, not a row
    under the path: the 66-cell modal and the picker's one PREVIEW row
    have none to give; the tooltip already carried the whole path where
    the rows clip it, and now carries the note after it (the note alone
    where nothing is clipped).
  - **The song's genre** is read from the album's listing (the row
    whose path is the song's), the only place this client sees a genre;
    a listing that names none, or has not answered, is the slot — the
    listing cannot tell a song without a genre from a server that sent
    none.
  - **`Landing::crumbs`** is the tests' words now (`‹artist›` in
    English); the surfaces word a slot in their locale
    (`Landing::words`).
  - **The CJK notes are written as the phone writes them**, no blanks
    inside: the kit's tooltip now hard-breaks a word wider than its 40
    cells by cells, between graphemes (it counted characters, so a
    Japanese line ran to 80 cells), and sizes its box by cells — every
    CJK tooltip gains.
  - **Review, the same day** (each found by reading the server, each
    pinned by a test): a featured song's folder is its ARTIST tag as
    written (`artist-display`), not the primary artist; an album song
    with no year never borrows the row's (the first cut did, and
    promised "… › 2024" for a song that lands a folder up); a song
    with no artist files under the album's credit, not the row's track
    artist; a genre is known only where the listing names exactly one;
    the note's genre words stand inside the sentence, its slots in the
    path's order, and a song's copy has its own words — the webapp's
    and the phone's.
