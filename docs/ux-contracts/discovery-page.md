# Discover — the history-seeded page

| | |
|---|---|
| **Design of record** | mStream's plan, `docs/discovery-page-plan.md` @ `a6076a7a` (2026-10-05, plan v2 after its adversarial review) — §1 (the three sources and the verbs a row carries on every client), §2 (the decisions: seed rule 2.1, window and widening 2.2, pins and remembered exclusions 2.3, the library filter 2.5, albums before songs 2.6, source stamping 2.7, the random fallback 2.8, placement 2.9, flag-never-probe 2.10, naming 2.12, **2.13: where the plan departs from the canvas — the plan wins**), §3.1–§3.3 (the server contract), §4.3 (this surface). The canvas "mStream Discovery Page" (https://claude.ai/artifact/SgZVPShtC92MFhkrkJbLcA) — boards `Tui` (Library tab, pointer on a row), `TuiNetwork` (the `[⋯]` menu open), `TuiPeers` (keyboard cursor), `TuiEmpty` (nothing finished yet) and the notes `n_tui` / `n_tui_ill` — is the record **for visuals only**: layout, wording and order; where it and the plan disagree, the plan is followed and the difference is logged below. **Amended 2026-10-11: the PLAY-NOW page** — plan §2.13's 2026-10-11 lines (as built on the webapp, mStream `6b33af36` / `9a3e492f`) and the design canvas https://claude.ai/artifact/AyZ2yH6431ajvmWgxjZuxo: only songs you can play now, no counts, no percentages, no p2p network (see the log). |
| **Server API** | **`POST /api/v1/discovery/taste`** (§3.1) `{window?, pin?, exclude?, remember?, filter?, staleDays?, sources?, limit?, newArtistsOnly?, ignoreVPaths?}` → `{window, widenedFrom, seedSource, fallback, seeds: [{hash, filepath, metadata, outcome, lastPlayed, plays, pinned, analyzed}], excluded: [{hash, filepath, metadata}], dropped, notAnalyzed, model, library: {filter, tracks: [{filepath, similarity, seedHash, metadata, genreTags, playCount, lastPlayed}], albums: [{…Album, similarity, scopedTracks, playedTracks, lastPlayed, tracks: [path]}], capped}, network: {results: [p2p row + {source: 'p2p', seedHash}], searched: {peers, tracks}} \| null, peers: {peers: [{id, name, reachable, mismatched, lastSeen, results: [federation row + {source: 'federation', seedHash: null}]}], searched: {peers, unreachable, mismatched}} \| null}` — an account only (403 for a federation key or a guest), 403 `Discovery is disabled` when discovery is off or the index is unavailable; `network`/`peers` are `null` when the source is not advertised for the caller or a random seed is in use. **`GET` / `PUT /api/v1/discovery/taste/prefs`** (§3.2) `{window, exclude: [hash], filter, staleDays}`, the user's `discovery:page` settings. **`POST /api/v1/discovery/plugin-jobs/lookup/batch`** (§3.3) — phase B; **v1 calls only the taste route and its prefs** (the `links` plug-in's resolve left with the Network tab on 2026-10-11 — the later Explore room's), plus routes the player already speaks: `POST /api/v1/db/random-songs` (another pick), the federation byte proxy `GET /api/v1/federation/peers/{id}/stream/…` and `POST /api/v1/stats/plays` with `peerId`, both through the queue as it is. **Gate**: the ping's `discovery === true && discoveryTaste >= 1` (§2.10) — here `capabilities.discovery && capabilities.discovery_taste >= 1`; the Connected servers source rides `federationDiscovery`; `discoveryP2p` (the Network source) and `discoveryPlugins` are parsed but read by nothing here since 2026-10-11 — the later Explore room's. |
| **Already in this repo** | The App/worker pair every room rides: `Effect::Api(ApiCmd::…)` drained onto the api thread, answered by `worker::answer` (and `src/web/api_worker.rs` in the browser build) as an `Event` that `App::apply_event` folds in, stale replies dropped by seq / node / session-generation guards. The **Sonic room's own row model** — `List::Sonic` maps to no App pane (`List::tab()` → `None`), its rows live in App state and its verbs call App methods that build the queue rows themselves (`src/gui/sonic.rs`), with `sonic::meter_glyph` for the one-cell match meter. The kit's **Tabs** (first consumer: the admin Discovery room), **Dropdown** (the header's server switcher, the stats page's period), **Table** (`table_view`, `scroll_list`, `ListView`), a list hung from a control (`frame_at` + `Surface::overlay`, the dropdown's own grammar) and **modals** (`modal_frame_on`). `Client::stats_history_async` (the one outcome-bearing history read — named so nobody reaches for it: the server picks the seeds, clause 10). `Client::random_song_async`; `gui::opener::open_target` (honours `MSTREAM_NO_OPEN`); `kit::clipboard::copy`; the track-actions sheet (`actions::Sheet { origin, track }`); the capture banner (`Capture`); `App::origin()` and `App::reach(&Origin)` for a peer origin (multi-server clauses 27, 30); `ApiCmd::DiscoveryProbe`; `GuiPrefs` (`[gui]`, unknown keys kept). **Missing**: `Ping.discovery_taste` / `discovery_plugins` and their `Capabilities` mirrors, the taste types and client calls, `ApiCmd::Taste` / `Event::Taste`, the App's taste state, `NavId::Discover`, `src/gui/discover.rs`, a capture for a seed pick, the `gui.discover.*` strings. |
| **Target surface** | the GUI player (`mstream-player gui`, both flavours — the window host draws the same `render`) as a **TOOLS nav room**. **The TUI changes nothing**: its Discover tab (`Tab::Discover`, the explicit-seed lookup), the Now Playing panel's neighbours (`App.now_discover`) and `Capture::Discover` stay exactly as they are. The one shared change is a title: the Now Playing view's Discover tab becomes **Similar** (plan §2.12; docs/ux-contracts/now-playing.md) |
| **Status** | contract extracted 2026-10-05; built 2026-10-05; reworked into the play-now page 2026-10-11 (the log) |

## Intent

A room that knows what you like because you finished it, and offers only
what you can **play now**. The server takes the songs you played to the
end (or most of the way) in a window you choose, never the ones you
skipped, and answers with songs and albums in **your own library** that
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

One room, one answer, two tabs. The answer arrives in two halves asked
in parallel (clause 30): **the library**, then **the connected servers**,
so the Library never waits on a 4 s server deadline.

- **Asking** — the first ask: the state line reads `• asking…` and each
  tab's pane holds four DIM skeleton rows where its columns will stand —
  no "asking…" sentence in the pane; a re-ask keeps the rows on screen
  until the new ones land.
- **Answered** — header `Discover · based on songs you finished` with
  the **window dropdown** at its right (`Last 30 days ▾`); the state line
  for the tab in front, in words (`• ready to play`); the **tabs**
  `Library · Connected servers`, names only (absent sources not drawn);
  the **seeds line** (`seeds  Title · Artist … and more  edit`) whose
  `edit` opens the **seeds modal**; the tab's table; the bottom row with
  the **"Include songs I've played lately" checkbox** (Library tab) and
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
  be (the library's with a button that ticks the checkbox); the tab
  stays.
- **Failed** — the half that failed says "couldn't reach this source"
  with Try again; the other half stands.

What persists, and what asks again:

| Kept | For how long | Asks again when |
|---|---|---|
| The answer (both halves) | the session; reused on re-entry within **5 minutes** of its arrival (the server's own cache window) | re-entry after 5 minutes; the nav row or `G` while the room is up; a window change; a seeds-modal change; a pin |
| The library half alone | — | the checkbox ticked or unticked (clause 65) |
| Pins | the session — a server switch or quit lets them go (clause 14) | — |
| Window, filter (the checkbox), dropped seeds | **the server's** prefs: every client opens on the same taste | — |
| The tab in front | `GuiPrefs` (`[gui] discover_tab`) | — |

A server switch drops the answer, the pins and the prefs read (the
session generation, clause 31).

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
   absent, as digit 9 goes dead with the sonic row. `G` or a click on the
   row while the room is up asks again at once (clause 30), bypassing the
   5-minute reuse.
4. **The label fits twelve cells** in every locale (the nav column's
   width).

### Seeds — what the server decides, what the room shows

10. **The server picks the seeds** (plan §2.1–§2.2): the distinct songs
    the user finished, or stopped after half their length, counted plays
    only, in the window, newest first, never a song whose latest play in
    the window was a skip; widening to all time, then to the play
    counters, then to one random song from the user's own libraries. The
    room reads no history of its own and sends only the window, the pins
    and the exclusions.
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
    false) and **DROPPED** (the server's `excluded`, unchecked). Space,
    Enter or a click toggles a row; `x` unpins. **Changes apply when the
    modal closes** (Esc, or Done): newly dropped seeds go out as `exclude`
    with `remember: true` — the server keeps them for every client
    (§2.3); a restored seed is a `PUT …/prefs` with `exclude` minus its
    hash; then one ask. A modal closed with nothing changed asks nothing.
14. **Pins are explicit** — the room never pins on its own. The modal
    offers **Pin the playing song** (only while the playing track is the
    session server's own, not a peer's, not already a seed, and fewer than
    8 pins stand — otherwise hidden, never grey) and **Pin a song…**, a
    capture pick in the sonic room's grammar (its clauses 10–13: the banner
    above the room, a pick consumed, a pick from another server rejected
    with "Pick a track on {server}", Esc returns, re-entry disarms). A pin
    asks when the modal closes, or when the pick lands. Pins last for the
    session and are sent with every ask; the server never stores them. A
    changed playing track never re-asks.
15. **The window** is the header's dropdown: Last 7 days · Last 30 days ·
    Last 90 days · All time, the current one wearing `•`. It shows the
    window **asked for** (`widenedFrom` when set, else `window`). `w` opens
    it, `[` `]` step it without the list. A change asks both halves with
    `remember: true`.
16. **Prefs are the server's.** The room opens with no window, filter or
    stale-days of its own in the body, so the saved ones apply; it reads
    `GET …/prefs` once per session, for the window and the checkbox the
    room shows before an answer says them (clauses 15, 65). Its only
    memory of its own is the tab (clause 40).

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
21. **Another pick** (`r`, only while the card stands) asks
    `random-songs` for one track of this server and pins it in place of
    the previous such pick — the server's random seed is the same all day
    (§2.8), so a new one has to come from here. **Choose a song…** is clause
    14's capture pick. Either answer is `pinned`, so the connected servers
    are asked from then on.
22. **A random seed searches the library only** (§2.8): `peers` comes back
    `null`, and the Connected servers tab holds one DIM line — "Asked once
    you've finished a song — a random pick only searches your library."
    (no `–`: the tabs carry no counts).

### Asking

30. **Two asks in parallel**, never one: **A** `sources: ['library']`
    — never `'network'`, even advertised (the 2026-10-11 log) — and **B**
    `sources: ['peers']` (only while `federation_discovery` is) —
    §3.1's "SHOULD", so the library renders without the peers' 4 s
    deadline. Both carry the same pins and exclusions; only A carries
    `remember`. No `limit`, so the server's defaults apply.
31. **Last ask wins.** Each ask takes a new seq and the session's
    generation; each half's reply carries both back and fills nothing
    under an older seq or another generation (a reply from the server the
    session has left). Each half lands on its own.
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
    groups: nothing is connected). `←` `→` and clicks switch; switching
    asks nothing; the pane under the tabs keeps one height whatever the
    tab. The tab in front is remembered in `GuiPrefs` (`[gui]
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

### The table

50. **One kit table per tab**, in the server's order (most similar
    first — no rank number, **no MATCH meter, no percentage, no LAST
    PLAYED**): Library **TRACK · ARTIST · ALBUM · LENGTH**; Connected
    servers **TRACK · ARTIST · ALBUM · ON · LENGTH**. TRACK starts two
    cells in, where the playing row's `▸` stands (clause 55).
51. **Library rows, albums first** (§2.6): the server's albums, then its
    songs. An album row's TRACK is the album's name with a DIM `album`
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
    lit; ↓ ↑ PageUp PageDown pick the cursor up (at the top on ↓, the
    bottom on ↑ — the room has no resting place of its own) and walk it;
    every line is a row; Esc stows it; a click acts on its row and
    stows it; Enter, `a`, `N`, `P`, `m` do nothing while it is stowed.
    Hover brightens, never selects.
58. **The wheel scrolls the list under the pointer** — the table, the
    seeds modal's rows — and the kit scrollbar stands at the table's right
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
    album's tracks in the Albums room — the wall's drilled album view, its
    Back leading to the wall — and `G` brings this room back as it was
    left (clause 3's reuse).
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
65. **The library filter is one checkbox**, at the left of the bottom
    row, on the Library tab only: `[ ] Include songs I've played lately`
    (`[✓]` Green when ticked, `[ ]` DarkGray; the label in the row's own
    weight; BRIGHT + BOLD under the pointer). Unticked is **`fresh`** (the
    default, §2.5: never played, or not in the prefs' `staleDays`),
    ticked **`all`**. The old "Never played" (`unplayed`) is never sent: a
    saved `unplayed` — or any word this build lacks — reads as unticked,
    and an answer that applied one to an ask that named no filter is
    asked again once as `fresh` (the prefs left alone). A click
    or `f` toggles it and asks the library half alone (`sources:
    ['library']`, `remember: true`); the connected servers' rows stand.
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
    new in your library sounds like these songs.", and under it, while the
    checkbox is unticked, an accent text button with the checkbox's own
    words — **Include songs I've played lately** — that ticks it and asks
    (clause 65). Connected servers: "Nothing from your connected servers
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
    (clause 1). A failed probe leaves no "asking…" standing.
83. **Transport and server errors** say "couldn't reach this source" (the
    webapp's words) in the warning colour, with **Try again**, in the
    panes of the half that failed — the state line `• no answer` — and the
    other half's rows stand, as do the old rows of the half that failed.
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
    header, rule and one row, and scrolls.
91. **With the queue panel open** (47 cells at 100 columns) the header's
    "based on…" clips before the window control, the tab row
    has none to drop (clause 42), the table narrows by clause 54, and the
    bottom row keeps the checkbox whole and drops the verbs from the tail
    by the bar's rules — no control moves for the width.

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
| ↓ pick a row · ← → tabs · w window · f played lately · e seeds · G ask again | `gui.tips.discover` |
| Enter play from here · a queue · N next · P play now · m actions · Esc done (a library song's row and a connected server's) | `gui.tips.discover_row` (one key since 2026-10-11) |
| Enter open the album · a queue · Esc done | `gui.tips.discover_album` |
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
| Open the album — Enter | `gui.discover.album_tip` |
| Add the album to the queue — a (an album row's `[+]`) | `gui.discover.album_queue_tip` |
| couldn't load the album | `gui.discover.album_failed` |
| sounds like %{track} | `gui.discover.seed_tip` |
| Listen through your server — P | `gui.discover.listen_tip` |
| Add next — N · Add to the queue — a · Play now — P · More actions | reused: `gui.files.next_tip` · `gui.files.queue_tip` · `gui.files.now_tip` · `gui.act.more_tip` |
| Include songs I've played lately (the checkbox, and the empty library's button) | `gui.discover.include_played` |
| Show the songs you've played lately too — f | `gui.discover.include_tip` |
| play · queue all · shuffle (the bottom row) | reused: `gui.bar.play` · `gui.bar.queue` · `gui.shuffle_word` |
| Nothing new in your library sounds like these songs. | `gui.discover.empty_library` |
| Nothing from your connected servers right now. | `gui.discover.empty_servers` |
| couldn't restore the seed: %{err} | `gui.discover.restore_failed` |
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
- **Album cards for the connected servers** (the webapp's "Albums from
  {server}"): the Connected servers table lists their songs; an album of
  theirs would open in a peer-browse view this room has no way into yet.
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
- **The stale-days setting** (`staleDays` is never written here) and the
  `limit` caps (the server's defaults stand).
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
| Two calls (§3.1) | `ApiCmd::Taste { seq, generation, half, body }` → `Event::Taste { seq, generation, half, answer }` — one command, a half on it (the D7 habit: a destination on the command, not a second command); arms in `worker::answer`, `web/api_worker.rs` and `App::apply_event`; the prefs read and write ride their own small command with the same guards |
| The webapp's re-ping on 403 | `ApiCmd::DiscoveryProbe`, reading `discovery && discovery_taste >= 1` for this room (the sonic room's probe reads its own flag) |
| Segmented switch (the play-now page's two sources) | The kit's Tabs, Library · Connected servers, names only; `[gui] discover_tab` in `GuiPrefs` |
| "Based on the last [window]" | The kit's Dropdown at the header's right (`Last 30 days ▾`) |
| "Include songs I've played lately" (the webapp's checkbox under More options) | The kit's checkbox glyphs inline at the bottom row's left (`[✓]` Green / `[ ]` DarkGray, `check_glyphs`), the settings rows' one-line form of the kit's opt-in row — a dense row has no room for its 4-row card |
| Album cards (12 × 6 covers), the two song columns | Album rows ahead of the songs in one table per tab (see the log) |
| A row's art tile, title and "Artist · Album" | TRACK · ARTIST · ALBUM (· ON) · LENGTH columns: a terminal has no art tile in chrome, and columns read faster than a joined line at 80 cells |
| The seeds sheet (PhoneSeeds) | A kit modal (`modal_frame_on`) with `[✓]` / `[ ]` rows and a modal primary Done; the window stays the header's dropdown, not repeated in the modal |
| "Start from a song…" (§2.8) | A capture of its own — a new `Capture` variant beside `Sonic` and `DjSeed`, never `Capture::Discover` (the TUI's) — with the banner over the room |
| Relative ages ("8 months ago") | The seeds modal's only: computed on the local clock from the seed's ISO `lastPlayed`; plural families (Wording) |
| Section skeletons while asking | Four DIM `░` rows in the table's own columns (clause 32) |
| Glyphs | `[▸] [»] [+] [⋯]` → `[>] [^] [+] [.]`, `•` → `*`, `▾` → `v`, the skeleton's `░` → `·`, `[✓]` → `[x]`, `—` → `-` on `legacy_conhost()` |

Open at implementation (decide, then log):

- **Restoring a dropped seed** (clause 13) assumes `PUT …/prefs` with an
  `exclude` list *replaces* the saved list; §3.2 says the PUT returns
  "the merged prefs". Confirm against the server's S5 piece — if
  `exclude` merges, the restore needs another body shape.
- **The fallback card's memory** (clause 20) is the room's: the server
  says `fallback` only for its own random seed, so once a pin stands the
  room keeps "no history yet" from the last pinless answer. Confirm what
  `seedSource` reads for pins plus history (the plan names the values,
  not the mix).
- **The 5-minute reuse** (clause 3) mirrors the server's cache window;
  an answer that failed is never reused.

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
with one hover verb that queues the album by name; the checkbox
(unticked by default and for a saved `unplayed` — the prefs alone, before
any answer — a click or `f` asks `all` then `fresh`, the empty library's
button ticks it); the fallback card; the scan-pending, off and failure
lines; the Connected servers rows with an offline and a mismatched
server's left out, Enter playing through the federation proxy, and `[⋯]`,
`m` and a right click opening the sheet with the peer's origin; the
list-cursor law (nothing lit on arrival, a click stows, verbs need the
cursor up); the wheel; the glyph fallbacks; cells in French and Japanese.
App tests: the two asks and their bodies (`library` alone, never
`network`; peers only when advertised; `remember` on A alone; the
checkbox asks the library alone; a saved `unplayed` asked again once as
`fresh`); a stale seq and another generation fill nothing; a Library row
queues with the session's origin; **a Connected servers row queues with
`Origin { server, peer: Some(id) }`, its stream URL is the federation
proxy path, and its play report carries `peerId`**; activating
any row leaves `App.discover`, `App.now_discover` and the TUI's capture
unchanged; another pick pins a random song; the seeds modal's close sends
the exclusions once. The locale parity test. Expect legs
(`test/e2e/legs/gui-discover.exp`, `MSTREAM_GUI_DEMO=1`; and
`gui-discover-room.exp` against the fake's taste route: the checkbox, the
header's words, a row; first-paint gates, drain-not-sleep). `cargo test --features desktop`; `cargo check --target
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
