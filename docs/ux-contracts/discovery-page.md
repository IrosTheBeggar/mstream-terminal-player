# Discover — the history-seeded page

| | |
|---|---|
| **Design of record** | mStream's plan, `docs/discovery-page-plan.md` @ `a6076a7a` (2026-10-05, plan v2 after its adversarial review) — §1 (the three sources and the verbs a row carries on every client), §2 (the decisions: seed rule 2.1, window and widening 2.2, pins and remembered exclusions 2.3, the library filter 2.5, albums before songs 2.6, source stamping 2.7, the random fallback 2.8, placement 2.9, flag-never-probe 2.10, naming 2.12, **2.13: where the plan departs from the canvas — the plan wins**), §3.1–§3.3 (the server contract), §4.3 (this surface). The canvas "mStream Discovery Page" (https://claude.ai/artifact/SgZVPShtC92MFhkrkJbLcA) — boards `Tui` (Library tab, pointer on a row), `TuiNetwork` (the `[⋯]` menu open), `TuiPeers` (keyboard cursor), `TuiEmpty` (nothing finished yet) and the notes `n_tui` / `n_tui_ill` — is the record **for visuals only**: layout, wording and order; where it and the plan disagree, the plan is followed and the difference is logged below. **Amended 2026-10-11: the PLAY-NOW page** — plan §2.13's 2026-10-11 lines (as built on the webapp, mStream `6b33af36` / `9a3e492f`) and the design canvas https://claude.ai/artifact/AyZ2yH6431ajvmWgxjZuxo: only songs you can play now, no counts, no percentages, no p2p network (see the log). **Amended 2026-10-14: the stateless page** — mStream `8dcfad0e` (the plan's §1, §2, §3.1 and §2.13's dated note): every call draws a new random batch of seeds, nothing is stored on the server, the library filter and the prefs routes are gone (see the log). |
| **Server API** | **`POST /api/v1/discovery/taste`** (§3.1), stateless since 2026-10-14 — nothing is stored or cached between calls — `{window? (7 \| 30 \| 90 \| 0, default 30), pin? (≤ 12 — `PIN_CAP`, the seed cap, since 2026-10-14 — seed first), exclude? (seed hashes left out of THIS call only, ≤ 200), sources?, limit?, newArtistsOnly?, ignoreVPaths?}` (`remember`, `filter` and `staleDays` are a 400) → `{window, widenedFrom, seedSource, fallback, seeds: [{hash, filepath, metadata, outcome, lastPlayed, plays, pinned, analyzed}], excluded: [{hash, filepath, metadata}] (this call's `exclude` hashes that resolve), dropped, notAnalyzed, model, library: {tracks: [{filepath, similarity, seedHash, metadata, genreTags, playCount, lastPlayed}], albums: [{…Album, similarity, scopedTracks, playedTracks, lastPlayed, tracks: [path]}], capped}, network: {results: [p2p row + {source: 'p2p', seedHash}], searched: {peers, tracks}} \| null, peers: {peers: [{id, name, reachable, mismatched, lastSeen, results: [federation row + {source: 'federation', seedHash: null}]}], searched: {peers, unreachable, mismatched}} \| null}` — an account only (403 for a federation key or a guest), 403 `Discovery is disabled` when discovery is off or the index is unavailable; `network`/`peers` are `null` when the source is not advertised for the caller or a random seed is in use. The seeds are the pins, then a **random batch** of the account's finished songs in the window — a different batch on every call — widening window → all time → counters → one random library song (drawn afresh on every call, 2026-10-14; never one in `exclude`). The library half is the fixed rule "songs you haven't played lately" (never, or more than 90 days ago). *(`GET` / `PUT …/taste/prefs` are gone since 2026-10-14 — a 404.)* **`POST /api/v1/discovery/plugin-jobs/lookup/batch`** (§3.3) — phase B; **v1 calls only the taste route** (the `links` plug-in's resolve left with the Network tab on 2026-10-11 — the later Explore room's), plus routes the player already speaks: the federation byte proxy `GET /api/v1/federation/peers/{id}/stream/…` and `POST /api/v1/stats/plays` with `peerId`, both through the queue as it is. **Gate**: the ping's `discovery === true && discoveryTaste >= 1` (§2.10) — here `capabilities.discovery && capabilities.discovery_taste >= 1`; the Connected servers source rides `federationDiscovery`; `discoveryP2p` (the Network source) and `discoveryPlugins` are parsed but read by nothing here since 2026-10-11 — the later Explore room's. |
| **Already in this repo** | The App/worker pair every room rides: `Effect::Api(ApiCmd::…)` drained onto the api thread, answered by `worker::answer` (and `src/web/api_worker.rs` in the browser build) as an `Event` that `App::apply_event` folds in, stale replies dropped by seq / node / session-generation guards. The **Sonic room's own row model** — `List::Sonic` maps to no App pane (`List::tab()` → `None`), its rows live in App state and its verbs call App methods that build the queue rows themselves (`src/gui/sonic.rs`), with `sonic::meter_glyph` for the one-cell match meter. The kit's **Tabs** (first consumer: the admin Discovery room), **Dropdown** (the header's server switcher, the stats page's period), **Table** (`table_view`, `scroll_list`, `ListView`), a list hung from a control (`frame_at` + `Surface::overlay`, the dropdown's own grammar) and **modals** (`modal_frame_on`). `Client::stats_history_async` (the one outcome-bearing history read — named so nobody reaches for it: the server picks the seeds, clause 10). `Client::random_song_async`; `gui::opener::open_target` (honours `MSTREAM_NO_OPEN`); `kit::clipboard::copy`; the track-actions sheet (`actions::Sheet { origin, track }`); the capture banner (`Capture`); `App::origin()` and `App::reach(&Origin)` for a peer origin (multi-server clauses 27, 30); `ApiCmd::DiscoveryProbe`; `GuiPrefs` (`[gui]`, unknown keys kept). **Missing**: `Ping.discovery_taste` / `discovery_plugins` and their `Capabilities` mirrors, the taste types and client calls, `ApiCmd::Taste` / `Event::Taste`, the App's taste state, `NavId::Discover`, `src/gui/discover.rs`, a capture for a seed pick, the `gui.discover.*` strings. |
| **Target surface** | the GUI player (`mstream-player gui`, both flavours — the window host draws the same `render`) as a **TOOLS nav room**. **The TUI changes nothing**: its Discover tab (`Tab::Discover`, the explicit-seed lookup), the Now Playing panel's neighbours (`App.now_discover`) and `Capture::Discover` stay exactly as they are. The one shared change is a title: the Now Playing view's Discover tab becomes **Similar** (plan §2.12; docs/ux-contracts/now-playing.md) |
| **Status** | contract extracted 2026-10-05; built 2026-10-05; reworked into the play-now page 2026-10-11; the album strip 2026-10-11 (the log) |

## Intent

A room that knows what you like because you finished it, and offers only
what you can **play now**. The server takes a random batch of the songs
you played to the end (or most of the way) in a window you choose, never
the ones you skipped — a new batch every visit — and answers with songs and albums in **your own library** that
sound like them and that you have not played lately, and songs on your
**connected servers** you can play through your own server. The seeds are
visible and yours to edit, the room says in words what it is based on —
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
line come from one batch.

- **Asking** — the first ask: the state line reads `• asking…` and each
  tab's pane holds four DIM skeleton rows where its columns will stand —
  no "asking…" sentence in the pane; a re-ask keeps the rows on screen
  until the new ones land.
- **Answered** — header `Discover · based on songs you finished` with
  the **window dropdown** at its right (`Last 30 days ▾`); the state line
  for the tab in front, in words (`• ready to play`); the **tabs**
  `Library · Connected servers`, names only (absent sources not drawn);
  the **seeds line** (`seeds  Title · Artist … and more  edit`) whose
  `edit` opens the **seeds modal**; the tab's **album strip** — a row of
  album tiles with their covers that scrolls sideways — where the room is
  20 rows tall or more (clauses 45–49); the tab's table; the bottom row with
  the whole-list verbs.
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

What persists, and what asks again:

| Kept | For how long | Asks again when |
|---|---|---|
| The answer (both halves) | on screen until the next one lands — **never reused**: every visit asks | every visit; the nav row or `G` while the room is up; a window change; a seeds-modal change; a pin; another pick; Try again / Check again |
| Pins, dropped seeds, random picks passed over | **the visit** — a fresh entry into the room, a server switch or quit lets them go (clauses 13, 14, 21); never stored anywhere | — |
| The window | `GuiPrefs` (`[gui] discover_window`, beside the tab; default 30) | — |
| The tab in front | `GuiPrefs` (`[gui] discover_tab`) | — |

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
   visit's pins, drops and passed-over picks go; `G` or a click on the row
   while the room is up asks again keeping them, as does the way back
   from a pick (clause 14).
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
11. **The header says what the answer is based on**, by `seedSource`:
    `history` → "based on songs you finished" (no count — the 2026-10-11
    log); `counters` → "based on
    songs you've played before"; `pinned` → "based on the songs you
    picked"; `random` → "nothing finished yet". When `widenedFrom` is set
    the state line adds "nothing in the last n days — all time instead"
    (or "from your play counts" for `counters`).
12. **The seeds line** shows the seeds that fit — the title (the file name
    when there is none) and the artist — pins first, then a DIM "and more"
    for the rest (never `+n`), then the `edit` text button. The server's random seed wears a
    DIM `random`; a pinned one `picked`. It never wraps; it never scrolls.
13. **The seeds modal** (`edit`, `e`) — a kit modal titled "Based on":
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
    window **asked for** (`widenedFrom` when set, else `window`). `w` opens
    it, `[` `]` step it without the list. A change asks both halves with
    it, and the choice is **remembered locally** in `[gui]
    discover_window` (7, 30, 90 or 0; unset or any other number asks the
    default 30). Every ask sends the window.
16. **Nothing is the server's** (2026-10-14). The route keeps nothing
    between calls: no prefs are read or written, no `remember`, `filter`
    or `staleDays` is sent (they are a 400). The room's memories of its
    own are the window (clause 15) and the tab (clause 40), both in
    `[gui]`.

### Nothing finished yet

20. **The fallback card** stands between the state line and the tabs
    while the server has no history to seed from — learned from the last
    answer asked without pins (`fallback: true`) and kept until an answer
    carries history seeds; the tabs and the Library's rows stay under it,
    around the random pick. It reads, in order: "Finish a few songs and
    this room learns your taste" (BOLD); "Songs you play to the end — or
    most of the way — become seeds. Skips never count."; "Until then,
    around a random pick: {title · artist}" ("around your pick" once a
    pick replaced it); then **another pick** and **choose a song…**. The
    state line reads `• no seeds` (DarkGray) · "suggestions around a
    random pick until you finish a song".
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
    from the batch the seeds line and modal show. The same window and
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
32. **While asking**: the first ask shows the state line `• asking…` in
    the getting-there colour and, in each tab's pane, **four skeleton
    rows** — DIM `░` runs of varied lengths where TRACK, ARTIST, ALBUM
    (ON) and LENGTH will stand (the webapp's four skeleton rows) — and no
    sentence: the state word is the only "asking…". A re-ask leaves the
    rows standing until the new ones land; only the state line says it is
    asking.

### Tabs and the state line

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
41. **The state line follows the tab in front** (the kit's state line),
    **in words — never a count**: Green `• ready to play` when the tab
    holds rows (Connected servers adds "streams through your server"),
    DarkGray `• nothing new` when it holds none, Yellow while asking
    (`• asking…`), waiting for the scan (`• waiting for the scan`) or
    after a failure (`• no answer`), DarkGray `• switched off` (clause
    82); the widened fact (clause 11) after; the facts drop from the
    right, the state never clips.
42. **The tab row carries no note** (the old "not in your library" / "you
    don't own these" went with the busy page).

### The album strip

45. **A strip of album tiles under each tab's seeds line**, above its
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
    server's album only when one of its rows names art (`art`,
    `albumArt`, `album-art` or `album_art_file`, the webapp's
    `peerArtOf`) and then through the parent's federation art proxy,
    `/api/v1/federation/peers/{id}/art/<file>`, the queue panel's peer
    reach — else no art). Where pixels are not drawn the cover is the
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
    lights a tile — hover brightens its title to BRIGHT + BOLD. The lit
    tile is always whole in view: a narrowed room (the queue panel
    opening, a resize) scrolls the strip to keep it, so Enter and `+`
    never act on an album off screen. **`←` `→` switch tabs whenever the
    hand is not on the strip** (clause 40); the tips say which (`← →
    albums` on the strip; `← → tabs` at rest — "↓ pick an album" where the
    strip stands — and on a song or album row). A tab switch starts its
    strip from the left.
48. **A tile's verbs.** A click or Enter on a **library album** opens it
    in the Albums room, its row's way (clause 61); a hovered tile shows
    one verb at its corner — `[+]` at the right of its title line, the
    title giving way — and `[+]`, `+` or `a` with the hand on it **queues
    the album** (`ApiCmd::TasteAlbum`, clause 61). A **connected server's
    album** has no drill here to open into (the room cannot browse a
    peer), so its click or Enter **plays** it — the queue replaced, the
    first song started — and `[+]` / `+` / `a` queue it; either way its
    songs are asked **of that server** through the parent's federation
    proxy (`album-songs` by name and the rows' track artist, a client
    aimed by the command's peer reach), stamped with `Origin { server,
    peer: Some(id) }` (clause 62); a server that will not list the album
    leaves **the rows that matched** (the webapp's
    `discoveryQueuePeerAlbum`), and nothing at all says "couldn't load
    the album". `N`, `P`, `m` do nothing on a tile. The tile's title has
    a tooltip ("Open the album — Enter"; "Play the album from {server} —
    Enter"), its `[+]` "Add the album to the queue — a". A library album
    is opened and queued **by name alone**, as its row always was (clause
    61): two albums of one name by different artists (two "Greatest
    Hits") open and queue the songs of both — the strip, showing both
    credits side by side, makes this visible (the log).
49. **The height gate.** The strip's seven rows and the gap under them
    stand only where the table keeps **at least three rows** under the
    strip: the room's six rows above it (header, state, blank, tabs,
    seeds, blank), the table's header and rule and the bottom row mean a
    content area of **20 rows** with no card. The installer's default
    100×30 window qualifies (22 rows with the footer on, 23 off: five or
    six rows under the strip); the **100×24 floor** (16 or 17) does not
    and keeps today's album container rows in the table, so the floor
    never loses its songs. The fallback card comes first (clause 20): with
    it whole the strip wants 28 rows. With the queue panel open (47
    cells) the strip shows three whole tiles and the fourth cut. **A
    resize across the gate keeps the keyboard's hand on what it was on**:
    a song row stays that song as the album rows come or go above it, an
    album row becomes its tile where the strip stands and the tile its row
    again at the floor; what has no place on the other side (a connected
    server's tile at the floor) puts the hand down.

### The table

50. **One kit table per tab**, in the server's order (most similar
    first — no rank number, **no MATCH meter, no percentage, no LAST
    PLAYED**): Library **TRACK · ARTIST · ALBUM · LENGTH**; Connected
    servers **TRACK · ARTIST · ALBUM · ON · LENGTH**. TRACK starts two
    cells in, where the playing row's `▸` stands (clause 55).
51. **Library rows, albums first** (§2.6): where the album strip does not
    stand (clause 49 — the floor, the card's height), the server's albums,
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
55. **The playing track's row is marked** as in every room.
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
    row (a peer path has no rating or playlist here, plan §1); its Keep a
    copy is phase B's.
63. *(Network rows and their lead menu — FIND ELSEWHERE through the
    `links` plug-ins, THIS LEAD's copy — left the room on 2026-10-11: they
    cannot play now. The later Explore room takes them; git history keeps
    the code.)*
64. **The whole-list verbs** stand at the right of the room's bottom row
    — **play · queue all · shuffle**, the browse bar's three with its
    meanings (browser-top-bar clauses 10–12: replace and play, append and
    say how many, shuffle the order once) — on the Library tab (its
    songs; albums are containers) and the Connected servers tab (its rows
    in order), only while the tab has rows (the bar's law: verbs appear
    only when they would do something). Keys `p`, `A`, `S`, as in the
    browse rooms.
65. *(The library filter's checkbox, "Include songs I've played lately",
    and its `f` left on 2026-10-14: the library half is the server's fixed
    rule — songs you haven't played lately, never or more than 90 days
    ago — and the bottom row keeps the whole-list verbs alone.)*
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
81. **Scan pending** — `notAnalyzed: true`: the seeds line and modal list
    the seeds; every tab's pane reads "This server hasn't analyzed any
    music yet — suggestions appear once the discovery scan has run." with
    **Check again** (asks again).
82. **A 403 re-pings before it names a reason** (the sonic-path contract's
    clause 40, same probe, reading this room's gate): the gate still on →
    clause 81's sentence and Check again (the index is not there yet); the
    gate gone → `• switched off` and "Sonic discovery has been switched
    off on this server.", no retry, and the row goes with the next ping
    (clause 1). A failed probe leaves no "asking…" standing. Entering
    the room while the probe is out starts the visit afresh but asks
    nothing: the probe's verdict lands first.
83. **Transport and server errors** say "couldn't reach this source" (the
    webapp's words) in the warning colour, with **Try again**, in the
    panes of the half that failed — the state line `• no answer` — and the
    other half's rows stand, as do the old rows of the half that failed.
    A failed A fails B too (B waited on it, clause 30); Try again asks
    both.
    The server's own words (a status code among them) are the line's
    tooltip, never on the page.

### The floor

90. **At 100×24**, footer on or off, the nav column holds every row
    (clause 2) and the room holds, top to bottom: the header, the state
    line, a blank, the tabs, the seeds line, a blank, the table's header
    and rule, the rows (seven with the footer on), the bottom row. The
    card (clause 20) pushes the table down; when the room is short the
    card gives up its blank line and its body sentence first (the title,
    the pick and its two actions stay), so the table keeps at least its
    header, rule and one row, and scrolls. The album strip does not stand
    at the floor (clause 49): the albums are the table's first rows.
91. **With the queue panel open** (47 cells at 100 columns) the header's
    "based on…" clips before the window control, the tab row
    has none to drop (clause 42), the table narrows by clause 54, and the
    bottom row drops the verbs from the tail by the bar's rules — no
    control moves for the width.

## Wording

English reference; every string through `t!()` in all ten locales (de en
es fr it ja pl pt ru zh). Glyphs (`▸ » ⋯ • ▾ ✓ —`) are drawn by the code
beside the words, never in the strings (docs/ui-kit.md, Glyphs). Keys
ending `_*` are plural families — `_one`, `_few`, `_many`, `_other`, chosen
the way the DJ room's `songs_words` chooses (English and the one-plural
locales repeat the other form); `%{count}` is their placeholder. Since
2026-10-11 the only families left are the seeds modal's ages: the room
itself shows no count.

| String | Key |
|---|---|
| Discover (the nav row and the room's heading) | `gui.nav.discover` — at most 12 cells in every locale |
| base tips: … · D Auto DJ · **G discover** · T stats · … | `gui.tips.base` (amended) |
| ↓ pick a row · ← → tabs · w window · e seeds · G ask again | `gui.tips.discover` |
| ↓ pick an album · ← → tabs · w window · e seeds · G ask again (at rest where the album strip stands) | `gui.tips.discover_strip` |
| Enter play from here · a queue · N next · P play now · m actions · ← → tabs · Esc done (a library song's row and a connected server's) | `gui.tips.discover_row` (one key since 2026-10-11) |
| Enter open the album · a queue · ← → tabs · Esc done | `gui.tips.discover_album` |
| ← → albums · Enter open the album · + a queue · ↓ songs · Esc done (the hand on a library album's tile) | `gui.tips.discover_tile` |
| ← → albums · Enter play the album · + a queue · ↓ songs · Esc done (the hand on a connected server's album's tile) | `gui.tips.discover_peer_tile` |
| r another pick · e seeds · ← → tabs | `gui.tips.discover_fallback` |
| ↑ ↓ rows · Space keep or drop · x unpin · Esc apply | `gui.tips.discover_seeds` |
| ↑ ↓ rows · Enter pick · Esc close (the window's list) | `gui.tips.discover_menu` |
| based on songs you finished | `gui.discover.based_on_history` |
| based on songs you've played before | `gui.discover.based_on_counts` |
| based on the songs you picked | `gui.discover.based_on_picked` |
| nothing finished yet | `gui.discover.nothing_finished` |
| Last %{days} days · All time (the window; days are 7, 30, 90) | `gui.discover.window_days` · `gui.discover.window_all` |
| The plays your seeds come from — w | `gui.discover.window_tip` |
| nothing in the last %{days} days — all time instead | `gui.discover.widened` |
| from your play counts | `gui.discover.from_counts` |
| asking… · ready to play · nothing new · no answer · waiting for the scan · switched off · no seeds (state words) | `gui.discover.asking` · `state_ready` · `state_empty` · `state_failed` · `state_pending` · `state_off` · `no_seeds` |
| suggestions around a random pick until you finish a song | `gui.discover.no_seeds_fact` |
| streams through your server | `gui.discover.through_server` |
| Library · Connected servers | `gui.discover.tab_library` · `tab_servers` |
| seeds · and more · edit | `gui.discover.seeds` · `seeds_more` · `seeds_edit` |
| Choose what this room starts from — e | `gui.discover.seeds_tip` |
| random · picked (a seed's marker) | `gui.discover.seed_random` · `seed_pinned` |
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
| play · queue all · shuffle (the bottom row) | reused: `gui.bar.play` · `gui.bar.queue` · `gui.shuffle_word` |
| Nothing new in your library sounds like these songs. | `gui.discover.empty_library` |
| Nothing from your connected servers right now. | `gui.discover.empty_servers` |
| couldn't reach this source · Try again | `gui.discover.failed` · `gui.discover.retry` |
| This server hasn't analyzed any music yet — suggestions appear once the discovery scan has run. · Check again | `gui.discover.scan_pending` · `gui.discover.check_again` |
| Sonic discovery has been switched off on this server. | `gui.discover.turned_off` — the sonic-path contract's `discoverTurnedOff` words |

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
- **"Save as playlist"** under the library's songs (the webapp's third
  footer verb): the bottom row keeps the browse bar's three.
- **Phase B** (plan §4.3): the 30 s **preview**, **Get it** (the YouTube
  job), **Keep a copy** (the federation-copy job), lookups, and every
  job's progress — the canvas's "getting it · 64%", "copying", the STATE
  columns, a connected server's row's `[copy]` and its own ⋯ (Keep a copy
  · Browse this server · Open elsewhere), "copies allowed" / "this key
  refuses copies". The
  batch job lookup (§3.3) is phase B's read; v1 never calls it.
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
| "Based on the last [window]" | The kit's Dropdown at the header's right (`Last 30 days ▾`), remembered in `[gui] discover_window` (the webapp's localStorage) |
| Album cards (the strips' scrollers), the two song columns | The album strip over each tab's table — 12 × 5 covers through `cover::Slot`, real pixels or the mosaic — where the room has 20 rows; album rows ahead of the songs below that (clauses 45–49, the log); the songs in one table per tab |
| A row's art tile, title and "Artist · Album" | TRACK · ARTIST · ALBUM (· ON) · LENGTH columns: a terminal has no art tile in chrome, and columns read faster than a joined line at 80 cells |
| The seeds sheet (PhoneSeeds) | A kit modal (`modal_frame_on`) with `[✓]` / `[ ]` rows and a modal primary Done; the window stays the header's dropdown, not repeated in the modal |
| "Start from a song…" (§2.8) | A capture of its own — a new `Capture` variant beside `Sonic` and `DjSeed`, never `Capture::Discover` (the TUI's) — with the banner over the room |
| Relative ages ("8 months ago") | The seeds modal's only: computed on the local clock from the seed's ISO `lastPlayed`; plural families (Wording) |
| Section skeletons while asking | Four DIM `░` rows in the table's own columns (clause 32) |
| Glyphs | `[▸] [»] [+] [⋯]` → `[>] [^] [+] [.]`, `•` → `*`, `▾` → `v`, the skeleton's `░` → `·`, `[✓]` → `[x]`, `—` → `-` on `legacy_conhost()` |

Open at implementation (decide, then log):

- **The fallback card's memory** (clause 20) is the room's: the server
  says `fallback` only for its own random seed, so once a pin stands the
  room keeps "no history yet" from the last pinless answer. Confirm what
  `seedSource` reads for pins plus history (the plan names the values,
  not the mix).

### What the tests pin

Render tests at TestBackend 100×30 and 100×24, footer on and off,
injecting `Event::Taste`: the row's gate (absent without either flag,
absent in a peer session, gone with a ping that drops it, the room
yielding to Files); TOOLS at 13, Auto DJ 14, Sonic 15, Discover 16, and
Discover and Settings never on one row; `G` opens it; the header's phrase
per `seedSource` and the widened fact; **no count, no meter, no
percentage and no "never played" on the page** — the answered tabs, the
queue-open view, the first ask's skeletons, the fallback card, scan
pending, a failure (no status code) and both empty states — where only a
row's m:ss and the window's days carry digits (the seeds modal behind
`edit` keeps its relative ages; the fallback card's "Skips never count."
is the webapp's own sentence); the two tabs, names only, and
the remembered tab (`peers` → servers, `network` → Library); the table's
columns per tab and the narrow-width order (ALBUM, then ON); albums first
with one hover verb that queues the album by name (at the floor); **the
album strip** (clauses 45–49, the pixel path off unless said): at 100×30
(footer on and off) the tiles with their mosaic or note covers, titles
and artists over the songs and no album row, at 100×24 the album rows
and no tile, the layout's 20-row gate and the card coming first, the
first ask's four skeleton tiles; best match first and the cut last tile,
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
height gate keeping the hand on its song, and carrying an album row to
its tile and back; hover brightening the title BOLD with `[+]` at its
corner and never selecting; a library tile opening in the Albums room by
click and by Enter, and `[+]`, `+`, `a` queueing it by name; a connected
server's tile playing through its server by click and by Enter (the hand
staying) (`TasteAlbum` with the peer's reach, the songs stamped with the
peer's origin, the stream through the proxy) and its `[+]` falling back
to the matched rows; the note tile keeping the owned ground under it;
the covers claimed once, a connected server's through the proxy; with a
forced kitty, sixel and iTerm2 picker the whole tiles' covers encoded as
pictures, the cut tile the mosaic, a scroll encoding only the newly
whole cover; end to end, the gui-discover-room leg (120×34, pixels off)
gating on the fake answer's album tile above the songs and the fake
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
cursor up); the wheel; the glyph fallbacks; cells in French and Japanese.
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
dropdown, the header's words, a row; first-paint gates, drain-not-sleep). `cargo test --features desktop`; `cargo check --target
wasm32-unknown-unknown`.

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
