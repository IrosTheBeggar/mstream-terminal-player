# Discover — the history-seeded page

| | |
|---|---|
| **Design of record** | mStream's plan, `docs/discovery-page-plan.md` @ `a6076a7a` (2026-10-05, plan v2 after its adversarial review) — §1 (the three sources and the verbs a row carries on every client), §2 (the decisions: seed rule 2.1, window and widening 2.2, pins and remembered exclusions 2.3, the library filter 2.5, albums before songs 2.6, source stamping 2.7, the random fallback 2.8, placement 2.9, flag-never-probe 2.10, naming 2.12, **2.13: where the plan departs from the canvas — the plan wins**), §3.1–§3.3 (the server contract), §4.3 (this surface). The canvas "mStream Discovery Page" (https://claude.ai/artifact/SgZVPShtC92MFhkrkJbLcA) — boards `Tui` (Library tab, pointer on a row), `TuiNetwork` (the `[⋯]` menu open), `TuiPeers` (keyboard cursor), `TuiEmpty` (nothing finished yet) and the notes `n_tui` / `n_tui_ill` — is the record **for visuals only**: layout, wording and order; where it and the plan disagree, the plan is followed and the difference is logged below. |
| **Server API** | **`POST /api/v1/discovery/taste`** (§3.1) `{window?, pin?, exclude?, remember?, filter?, staleDays?, sources?, limit?, newArtistsOnly?, ignoreVPaths?}` → `{window, widenedFrom, seedSource, fallback, seeds: [{hash, filepath, metadata, outcome, lastPlayed, plays, pinned, analyzed}], excluded: [{hash, filepath, metadata}], dropped, notAnalyzed, model, library: {filter, tracks: [{filepath, similarity, seedHash, metadata, genreTags, playCount, lastPlayed}], albums: [{…Album, similarity, scopedTracks, playedTracks, lastPlayed, tracks: [path]}], capped}, network: {results: [p2p row + {source: 'p2p', seedHash}], searched: {peers, tracks}} \| null, peers: {peers: [{id, name, reachable, mismatched, lastSeen, results: [federation row + {source: 'federation', seedHash: null}]}], searched: {peers, unreachable, mismatched}} \| null}` — an account only (403 for a federation key or a guest), 403 `Discovery is disabled` when discovery is off or the index is unavailable; `network`/`peers` are `null` when the source is not advertised for the caller or a random seed is in use. **`GET` / `PUT /api/v1/discovery/taste/prefs`** (§3.2) `{window, exclude: [hash], filter, staleDays}`, the user's `discovery:page` settings. **`POST /api/v1/discovery/plugin-jobs/lookup/batch`** (§3.3) — phase B; **v1 calls only the taste route, its prefs, and the `links` plug-in's resolve** (`GET /api/v1/discovery/plugins`, `POST /api/v1/discovery/plugins/{name}/resolve`), plus routes the player already speaks: `POST /api/v1/db/random-songs` (another pick), the federation byte proxy `GET /api/v1/federation/peers/{id}/stream/…` and `POST /api/v1/stats/plays` with `peerId`, both through the queue as it is. **Gate**: the ping's `discovery === true && discoveryTaste >= 1` (§2.10) — here `capabilities.discovery && capabilities.discovery_taste >= 1`; the Network source rides `discoveryP2p`, Peers rides `federationDiscovery`, the plug-ins `discoveryPlugins` (all three already advertised; the player reads only the first two today). |
| **Already in this repo** | The App/worker pair every room rides: `Effect::Api(ApiCmd::…)` drained onto the api thread, answered by `worker::answer` (and `src/web/api_worker.rs` in the browser build) as an `Event` that `App::apply_event` folds in, stale replies dropped by seq / node / session-generation guards. The **Sonic room's own row model** — `List::Sonic` maps to no App pane (`List::tab()` → `None`), its rows live in App state and its verbs call App methods that build the queue rows themselves (`src/gui/sonic.rs`), with `sonic::meter_glyph` for the one-cell match meter. The kit's **Tabs** (first consumer: the admin Discovery room), **Dropdown** (the header's server switcher, the stats page's period), **Table** (`table_view`, `scroll_list`, `ListView`), a list hung from a control (`frame_at` + `Surface::overlay`, the dropdown's own grammar) and **modals** (`modal_frame_on`). `Client::stats_history_async` (the one outcome-bearing history read — named so nobody reaches for it: the server picks the seeds, clause 10). `Client::random_song_async`; `gui::opener::open_target` (honours `MSTREAM_NO_OPEN`); `kit::clipboard::copy`; the track-actions sheet (`actions::Sheet { origin, track }`); the capture banner (`Capture`); `App::origin()` and `App::reach(&Origin)` for a peer origin (multi-server clauses 27, 30); `ApiCmd::DiscoveryProbe`; `GuiPrefs` (`[gui]`, unknown keys kept). **Missing**: `Ping.discovery_taste` / `discovery_plugins` and their `Capabilities` mirrors, the taste types and client calls, `ApiCmd::Taste` / `Event::Taste`, the App's taste state, `NavId::Discover`, `src/gui/discover.rs`, a capture for a seed pick, the `gui.discover.*` strings. |
| **Target surface** | the GUI player (`mstream-player gui`, both flavours — the window host draws the same `render`) as a **TOOLS nav room**. **The TUI changes nothing**: its Discover tab (`Tab::Discover`, the explicit-seed lookup), the Now Playing panel's neighbours (`App.now_discover`) and `Capture::Discover` stay exactly as they are. The one shared change is a title: the Now Playing view's Discover tab becomes **Similar** (plan §2.12; docs/ux-contracts/now-playing.md) |
| **Status** | contract extracted 2026-10-05; not implemented yet |

## Intent

A room that knows what you like because you finished it. The server takes
the songs you played to the end (or most of the way) in a window you
choose, never the ones you skipped, and answers three ways at once: songs
and albums in **your own library** that sound like them and that you have
never played or forgotten; **leads** from the discovery network that your
library does not have; and songs on your **federated peers** you can
listen to through your own server. The seeds are visible and yours to
edit, the room says what it is based on, and every row does what a row
from that source can do — play and queue a song you own, listen to a
peer's, find a lead elsewhere — without the room ever going blank: with
nothing finished yet it starts from a random pick and says so.

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

One room, one answer, three tabs. The answer arrives in two halves asked
in parallel (clause 30): **library + network**, then **peers**, so the
Library never waits on a 4 s peer deadline.

- **Asking** — the first ask: the state line reads `• asking…` and each
  tab's pane holds the kit's scan bar (all-dim, no estimate) over a
  present-progressive sentence; a re-ask keeps the rows on screen until
  the new ones land.
- **Answered** — header `Discover · based on n songs you finished` with
  the **window dropdown** at its right (`Last 30 days ▾`); the state line
  for the tab in front; the **tabs** `Library n · Network n · Peers n`
  (absent sources not drawn); the **seeds line** (`seeds  Title · Artist
  … +n  edit`) whose `edit` opens the **seeds modal**; the tab's table;
  the bottom row with the **library filter dropdown** (Library tab) and
  the whole-list verbs.
- **Nothing finished yet** (`fallback: true`, the server's random seed) —
  the fallback card above the tabs: what the room needs, the random
  pick it started from, **another pick** and **choose a song…**; Network
  and Peers are not asked for a random seed and say so.
- **Scan pending** (`notAnalyzed: true`, or a 403 whose re-ping still
  carries the gate) — the seeds are listed; every pane says the scan has
  not run, with **Check again**.
- **Switched off** (a 403 whose re-ping lost the gate) — the sentence, no
  retry; the nav row leaves with the next ping and the room yields to
  Files.
- **A source that answers nothing** — one DIM line where its rows would
  be; the tab stays.
- **Failed** — the half that failed says so with Retry; the other half
  stands.

What persists, and what asks again:

| Kept | For how long | Asks again when |
|---|---|---|
| The answer (both halves) | the session; reused on re-entry within **5 minutes** of its arrival (the server's own cache window) | re-entry after 5 minutes; the nav row or `G` while the room is up; a window change; a seeds-modal change; a pin |
| The library half alone | — | a filter change (clause 65) |
| Pins | the session — a server switch or quit lets them go (clause 14) | — |
| Window, filter, dropped seeds | **the server's** prefs: every client opens on the same taste | — |
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
    `history` → "based on n songs you finished"; `counters` → "based on
    songs you've played before"; `pinned` → "based on the songs you
    picked"; `random` → "nothing finished yet". When `widenedFrom` is set
    the state line adds "nothing in the last n days — all time instead"
    (or "from your play counts" for `counters`).
12. **The seeds line** shows the seeds that fit — the title (the file name
    when there is none) and the artist — pins first, then `+n` for the
    rest, then the `edit` text button. The server's random seed wears a
    DIM `random`; a pinned one `picked`. It never wraps; it never scrolls.
13. **The seeds modal** (`edit`, `e`) — a kit modal titled "Based on":
    **PINNED** (each with an unpin), **FROM YOUR PLAYS** (the history seeds,
    checked: the relative last play, the play count, "finished" or
    "listened to" by `outcome`, "not analyzed yet" when `analyzed` is
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
    `GET …/prefs` once per session for `staleDays` (the filter's words,
    clause 65). Its only memory of its own is the tab (clause 40).

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
    14's capture pick. Either answer is `pinned`, so Network and Peers are
    asked from then on.
22. **A random seed searches the library only** (§2.8): `network` and
    `peers` come back `null`, the two tabs show `–` for a count and one
    DIM line — "Asked once you've finished a song — a random pick only
    searches your library."

### Asking

30. **Two asks in parallel**, never one: **A** `sources: ['library',
    'network']` (`'network'` only while `discovery_p2p` is advertised),
    **B** `sources: ['peers']` (only while `federation_discovery` is) —
    §3.1's "SHOULD", so the library renders without the peers' 4 s
    deadline. Both carry the same pins and exclusions; only A carries
    `remember`. No `limit`, so the server's defaults apply.
31. **Last ask wins.** Each ask takes a new seq and the session's
    generation; each half's reply carries both back and fills nothing
    under an older seq or another generation (a reply from the server the
    session has left). Each half lands on its own.
32. **While asking**: the first ask shows the state line `• asking…` in
    the getting-there colour and, in each tab's pane, the scan bar and
    "asking the server…" (Library, Network) or "asking your peers… up to
    4 s each" (Peers). A re-ask leaves the rows standing until the new
    ones land; only the state line says it is asking.

### Tabs and the state line

40. **The tabs** are the kit's: Library always; Network while
    `discovery_p2p` is advertised; Peers while `federation_discovery` is —
    a source the ping does not advertise is not drawn at all. A `null`
    half for a non-random seed (the server says this caller lacks the
    source) hides that tab until the next answer. Each tab carries its
    count: Library = albums + songs, Network = leads, Peers = playable
    songs; no count while its half is out, `–` when it was not asked.
    `←` `→` and clicks switch; switching asks nothing; the pane under the
    tabs keeps one height whatever the tab. The tab in front is
    remembered in `GuiPrefs` (`[gui] discover_tab`); a remembered tab
    whose source is gone falls back to Library.
41. **The state line follows the tab in front** (the kit's state line):
    Library `• n similar`; Network `• n leads · n servers searched`;
    Peers `• n playable · n of m peers answered · streams through your
    server` — the state Green when the tab holds rows, DarkGray `• nothing
    new` when it holds none, Yellow while asking (`• asking…`), waiting
    for the scan (`• waiting for the scan`) or after a failure (`• no
    answer`), DarkGray `• switched off` (clause 82); the facts drop from
    the right, the state never clips.
42. **The tab row's right edge** carries the tab's note — Network "not in
    your library", Peers "you don't own these" — dropped when the row is
    too narrow.

### The table

50. **One kit table per tab**, in the server's order (most similar
    first — no rank number; the percentage leads, PLAN D8):
    Library **MATCH · TRACK · LAST PLAYED · LENGTH**; Network **MATCH ·
    LEAD · FOUND ON · LENGTH**; Peers **MATCH · TRACK · ON · LENGTH**. MATCH
    is the sonic room's one-cell meter and the bare percentage (`▇ 83%`;
    the percentage alone on the legacy console).
51. **Library rows, albums first** (§2.6): the server's albums, then its
    songs. A song row's TRACK is the title (the file name when there is
    none) · the artist; an album row's is the album · the artist with a
    DIM `album` beside it. LAST PLAYED reads "never" for a song never
    played (an album none of whose songs were), else the relative age of
    `lastPlayed` — today, yesterday, n days / months / years ago. LENGTH
    is m:ss; an album's is its song count in your libraries
    (`scopedTracks`); a missing value is the kit's DIM `—`.
52. **Network rows**: LEAD is the title · the artist; FOUND ON the server
    that answered (the row's `peer.name`); LENGTH from `duration` when the
    lead has one.
53. **Peers rows** are grouped by peer (clause 70); ON is the peer's name.
54. **Narrow widths**: when TRACK (or LEAD) would get fewer than 24 cells,
    LAST PLAYED goes first on Library, FOUND ON on Network, ON on Peers;
    MATCH and LENGTH never go. Widths are cells (`kit::width`; a CJK title
    is two cells a glyph).
55. **The playing track's row is marked** as in every room.
56. **A row names its seed**: a Library or Network row whose `seedHash`
    is one of the answer's seeds has a tooltip "sounds like {title ·
    artist}" — a pointer enhancement; nothing lives only there.
57. **The list-cursor law** (docs/ui-kit.md): the room opens with no row
    lit; ↓ ↑ PageUp PageDown pick the cursor up (at the top on ↓, the
    bottom on ↑ — the room has no resting place of its own) and walk it;
    group lines are skipped; Esc stows it; a click acts on its row and
    stows it; Enter, `a`, `N`, `P`, `m` do nothing while it is stowed.
    Hover brightens, never selects.
58. **The wheel scrolls the list under the pointer** — the table, the
    seeds modal's rows, the menu (which scrolls to the keyboard's cursor
    too, when plug-ins list more links than its frame holds) — and the kit
    scrollbar stands at the table's right while the rows overflow. An open
    dropdown's three or four choices always show whole, so the wheel over
    one moves nothing, and not the table under it either.

### Verbs

60. **Library song rows** play and queue like any track the session
    browses, with the session's origin (`App::origin()`): a click or
    Enter plays the row and queues the tab's songs after it (play from
    here, library-rooms clause 14); the hover verbs `[▸]` play now, `[»]`
    add next, `[+]` add to the end, `[⋯]` the track-actions sheet (its
    contract's verbs); `P`, `N`, `a`, `m` on the keyboard; a right click is
    the sheet.
61. **Library album rows are containers** (library-rooms clause 5): no
    hover verbs. A click or Enter opens the album's tracks in the Albums
    room — the wall's drilled album view, its Back leading to the wall —
    and `G` brings this room back as it was left (clause 3's reuse).
62. **Peers rows listen through your server**: `[▸]` Listen (play now),
    `[»]` add next, `[+]` add to the end; a click or Enter plays the row
    and queues the tab's playable rows after it; `P`, `N`, `a`. The queue
    row's origin is `Origin { server: <the session's server>, peer:
    Some(<the row's peer id>) }`, so it reaches its bytes the multi-server
    contract's way (clause 27: the parent's federation proxy,
    `/api/v1/federation/peers/{id}/stream/…`, or the peer's own tunnel
    when a direct ticket is up), and its play is reported to the parent
    with `peerId` and the snapshot (play-reporting clause 7). No `[⋯]`,
    no sheet, no right click in v1 — a peer path has no rating, playlist
    or album to go to (plan §1), and its own ⋯ is phase B's.
63. **Network rows have no inline verbs** (§1: leads without a file
    cannot play). A click, Enter, the hover `[⋯]` or a right click opens
    the row's **menu**, hung from the row in the kit dropdown's grammar
    (a Rounded accent frame, `Clear` beneath, overlay-registered, the
    keyboard cursor the accent slab, hover BRIGHT + BOLD, a click picks
    and closes, Esc or a click elsewhere closes). In v1 it holds:
    - **FIND ELSEWHERE** — the links of every listed plug-in with the
      `links` capability that is enabled and available, canonical links
      first, then searches, in the plug-in's order and words. They are
      resolved when the menu first opens, by forwarding the row verbatim
      as the `recommendation` (the server strips what it does not know),
      and kept with the answer; "asking…" while out; "couldn't get the
      links: {err}" in the warning colour on a failure (a 502, or a 429
      passed through). Only `http` and `https` links are offered — a link
      is handed to the operating system's opener, which would run any
      other scheme. A pick goes to `gui::opener::open_target`: "{label} is
      open in your browser", or under `MSTREAM_NO_OPEN` "{label}: {url}".
    - **THIS LEAD** — **Copy "Artist – Title"** (`kit::clipboard::copy`),
      confirmed on the note line.

    The plug-in list is read once per session, and only while
    `discovery_plugins` is advertised; without it, or without a `links`
    plug-in, the menu holds THIS LEAD alone. Preview, Get it and lookup
    are phase B.
64. **The whole-list verbs** stand at the right of the room's bottom row
    — **play · queue all · shuffle**, the browse bar's three with its
    meanings (browser-top-bar clauses 10–12: replace and play, append and
    say how many, shuffle the order once) — on the Library tab (its
    songs; albums are containers) and the Peers tab (its playable rows in
    order); never on Network, which has nothing to play (the bar's law:
    verbs appear only when they would do something). Keys `p`, `A`, `S`,
    as in the browse rooms.
65. **The library filter** is a dropdown at the left of the bottom row,
    on the Library tab only: **Never played** (`unplayed`) · **Not in n
    days** (`fresh`, n the prefs' `staleDays`, 90 by default — the default
    filter, §2.5) · **Everything** (`all`). `f` opens it. A change asks
    the library half alone (`sources: ['library']`, `remember: true`);
    the Network and Peers rows stand.
66. **Plays started here are ordinary plays** — `manual`, `shuffle` or
    `autodj` as the play-reporting contract decides; there is no
    `discovery` source in v1 (plan §6, question 8).
67. **The room never touches the TUI's Discover state** — `App.discover`,
    its drill and seed, `App.now_discover` and `Capture::Discover` read
    the same after any verb here.

### Peers, grouped

70. **One group per peer**, in the server's order: a group line, then
    its rows. The line is the kit's state grammar — `•` and the peer's
    name, then facts: reachable → Green, "online · n songs" ("online ·
    nothing new" with none); not reachable → DarkGray and **the whole
    group dim**: "not answering · last seen {age}" (from `lastSeen`; the
    age left out when there is none) and **no rows** (the kept-from-last-
    visit cache is out of scope, §2.13); `mismatched` → Yellow, "another
    sonic model — can't compare".
71. **A group line is not a row**: the cursor skips it, a click on it
    does nothing, it carries no verbs (no "browse this server" in v1).
72. **No peers at all** (`searched.peers` is 0): one DIM line, "You're not
    federated with anyone yet."

### When a source says nothing, or the server says no

80. **Each source explains itself in one DIM line** where its first row
    would be, under the table's header and rule; the tab never
    disappears for being empty. Library: "You've played everything that
    sounds like this — try another filter, or a wider window." Network
    with no catalogues (`searched.peers` 0): "The discovery network is
    still warming up — no other servers' catalogues yet."; with some:
    "Nothing new out there for these seeds — every match is already in
    your library." Peers: clause 72, or each group's own line.
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
83. **Transport and server errors** say "couldn't ask for suggestions:
    {err}" in the warning colour, with **Retry**, in the panes of the
    half that failed — the state line `• no answer` — and the other half's
    rows stand, as do the old rows of the half that failed.

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
    "based on…" clips before the window control, the tab-row note goes
    (clause 42), the table narrows by clause 54, and the bottom row keeps
    the filter control whole and clips the verbs by the bar's rules —
    no control moves for the width.

## Wording

English reference; every string through `t!()` in all ten locales (de en
es fr it ja pl pt ru zh). Glyphs (`▸ » ⋯ • ▾ – ▇`) are drawn by the code
beside the words, never in the strings (docs/ui-kit.md, Glyphs). Keys
ending `_*` are plural families — `_one`, `_few`, `_many`, `_other`, chosen
the way the DJ room's `songs_words` chooses (English and the one-plural
locales repeat the other form); `%{count}` is their placeholder.

| String | Key |
|---|---|
| Discover (the nav row and the room's heading) | `gui.nav.discover` — at most 12 cells in every locale |
| base tips: … · D Auto DJ · **G discover** · T stats · … | `gui.tips.base` (amended) |
| ↓ pick a row · ← → tabs · w window · e seeds · G ask again | `gui.tips.discover` |
| Enter play from here · a queue · N next · P play now · m actions · Esc done | `gui.tips.discover_row` |
| Enter where else to find it · Esc done | `gui.tips.discover_lead` |
| r another pick · e seeds · ← → tabs | `gui.tips.discover_fallback` |
| ↑ ↓ rows · Space keep or drop · x unpin · Esc apply | `gui.tips.discover_seeds` |
| ↑ ↓ rows · Enter open · Esc close | `gui.tips.discover_menu` |
| based on %{count} songs you finished | `gui.discover.based_on_*` |
| based on songs you've played before | `gui.discover.based_on_counts` |
| based on the songs you picked | `gui.discover.based_on_picked` |
| nothing finished yet | `gui.discover.nothing_finished` |
| Last %{days} days · All time (the window; days are 7, 30, 90) | `gui.discover.window_days` · `gui.discover.window_all` |
| The plays your seeds come from — w | `gui.discover.window_tip` |
| nothing in the last %{days} days — all time instead | `gui.discover.widened` |
| from your play counts | `gui.discover.from_counts` |
| asking… · nothing new · no answer · waiting for the scan · switched off · no seeds (state words) | `gui.discover.asking` · `state_empty` · `state_failed` · `state_pending` · `state_off` · `no_seeds` |
| suggestions around a random pick until you finish a song | `gui.discover.no_seeds_fact` |
| %{count} similar | `gui.discover.similar_*` |
| %{count} leads | `gui.discover.leads_*` |
| %{count} playable | `gui.discover.playable_*` |
| %{count} servers searched | `gui.discover.searched_*` |
| %{answered} of %{count} peers answered | `gui.discover.answered_*` |
| streams through your server | `gui.discover.through_server` |
| Library · Network · Peers | `gui.discover.tab_library` · `tab_network` · `tab_peers` |
| not in your library · you don't own these (tab-row notes) | `gui.discover.note_network` · `note_peers` |
| seeds · +%{count} · edit | `gui.discover.seeds` · `seeds_more` · `seeds_edit` |
| Choose what this room starts from — e | `gui.discover.seeds_tip` |
| random · picked (a seed's marker) | `gui.discover.seed_random` · `seed_pinned` |
| Based on (the modal's title) | `gui.discover.seeds_title` |
| songs you finished or mostly heard · skips never count | `gui.discover.seeds_sub` |
| PINNED · FROM YOUR PLAYS · DROPPED | `gui.discover.sec_pinned` · `sec_seeds` · `sec_dropped` |
| finished · listened to · not analyzed yet | `gui.discover.outcome_completed` · `outcome_stopped` · `not_analyzed` |
| %{count} plays | `gui.discover.plays_*` |
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
| MATCH · TRACK · LAST PLAYED · LENGTH · LEAD · FOUND ON · ON | `gui.discover.col_match` · `col_track` · `col_last` · `col_length` · `col_lead` · `col_found` · `col_on` |
| never · today · yesterday | `gui.discover.never` · `today` · `yesterday` |
| %{count} days ago · %{count} months ago · %{count} years ago | `gui.discover.days_ago_*` · `months_ago_*` · `years_ago_*` |
| album (an album row's marker) | `gui.discover.album` |
| %{count} songs (an album's LENGTH, a peer group's fact) | `gui.discover.songs_*` |
| Open the album — Enter | `gui.discover.album_tip` |
| sounds like %{track} | `gui.discover.seed_tip` |
| Listen through your server — P | `gui.discover.listen_tip` |
| Add next — N · Add to the queue — a · Play now — P · More actions | reused: `gui.files.next_tip` · `gui.files.queue_tip` · `gui.files.now_tip` · `gui.act.more_tip` |
| Never played · Not in %{count} days · Everything | `gui.discover.filter_unplayed` · `filter_fresh_*` · `filter_all` |
| Which of your songs to show — f | `gui.discover.filter_tip` |
| play · queue all · shuffle (the bottom row) | reused: `gui.bar.play` · `gui.bar.queue` · `gui.shuffle_word` |
| online · not answering · last seen %{age} · another sonic model — can't compare | `gui.discover.peer_online` · `peer_unreachable` · `peer_last_seen` · `peer_mismatched` |
| You're not federated with anyone yet. | `gui.discover.no_peers` |
| You've played everything that sounds like this — try another filter, or a wider window. | `gui.discover.empty_library` |
| The discovery network is still warming up — no other servers' catalogues yet. | `gui.discover.empty_network_warming` |
| Nothing new out there for these seeds — every match is already in your library. | `gui.discover.empty_network` |
| asking the server… · asking your peers… up to 4 s each | `gui.discover.asking_library` · `asking_peers` |
| FIND ELSEWHERE · THIS LEAD (the menu's sections) | `gui.discover.menu_links` · `menu_lead` |
| Copy "%{lead}" · copied "%{lead}" | `gui.discover.copy` · `copied` |
| couldn't get the links: %{err} | `gui.discover.links_failed` |
| %{label} is open in your browser · %{label}: %{url} | reused: `note.link_opened` · `note.link_headless` |
| couldn't open the link: %{err} | `gui.discover.open_failed` |
| couldn't ask for suggestions: %{err} · Retry | `gui.discover.failed` · `gui.discover.retry` |
| This server hasn't analyzed any music yet — suggestions appear once the discovery scan has run. · Check again | `gui.discover.scan_pending` · `gui.discover.check_again` |
| Sonic discovery has been switched off on this server. | `gui.discover.turned_off` — the sonic-path contract's `discoverTurnedOff` words |

Data is never translated: titles, artists, album and peer names, and the
`links` plug-in's own labels ("Search Bandcamp" — the server's words).

## Out of scope here

- **Phase B** (plan §4.3): the 30 s **preview**, **Get it** (the YouTube
  job), **Keep a copy** (the federation-copy job), lookups, and every
  job's progress — the canvas's "getting it · 64%", "copying", the
  Network and Peers STATE columns, the menu's LISTEN and GET IT sections,
  a peer row's `[copy]` and its own ⋯ (Keep a copy · Browse this server ·
  Open elsewhere), "copies allowed" / "this key refuses copies". The
  batch job lookup (§3.3) is phase B's read; v1 never calls it.
- **The peer cache** — "n songs kept from the last visit", "plays when
  it's back" (§2.13): an unreachable peer shows its state and no rows.
- **The album under the cursor** (the TuiPeers board's ownership facts,
  "play album · add the 7 you're missing", "view artist") (§2.13).
- **New artists only** (the TuiNetwork board's `[✓]`): the room sends
  `newArtistsOnly: false`, so a lead or a peer's song may be by an artist
  you already have. The plan keeps the switch client-local (§3.2); it
  ports with a `GuiPrefs` key when wanted.
- **The "new artist" marker** on a lead: the answer carries no such fact.
- **"not for me · hides it"** on a lead: there is no route to remember it.
- **"browse this server ▸"** on a peer's group line: the header's server
  switcher reaches a peer.
- **"browse the library"** on the fallback card: the Files row is one
  click away; "choose a song…" is the card's way into the library.
- **The stale-days setting** (`staleDays` is read, never written here)
  and the `limit` caps (the server's defaults stand).
- **A text filter** over the rows (the browse bar's): an answer is at most
  a few dozen rows per tab.
- **The TUI** — no tab, no panel, no key; its Discover tab stays the
  explicit-seed lookup it is.

## Translation notes (terminal GUI)

| Record (plan · canvas) | Here |
|---|---|
| A TOOLS room "under Sonic path" (§2.9; canvas row 17) | `NavId::Discover` last in `NAV`; `draw_nav` gates it like the sonic row and hides it for a peer like the play lists; rows TOOLS 13 · Auto DJ 14 · Sonic 15 · Discover 16 · Settings on the content's last row; `G` in the Library screen's global table, recorded in `gui.tips.base`; `render`'s match over `NAV` and `Gui::wheel` get their arms |
| The room's own rows | **Its own row model** (`App.taste`), the sonic room's precedent: `List::Discover` maps to no pane (`List::tab()` → `None`), and the verbs call App methods that build the queue rows themselves — play from here, play now, add next, add to the end, play / queue / shuffle all — stamping `self.origin()` on Library rows and the peer origin on Peers rows. The TUI's `Tab::Discover` pane is never written |
| A federation row | Flat (`artist`, `title`, `album`, `year`, `duration`, `filepath` = the **peer's** path, `peer: {id, name}`), not the `{filepath, metadata}` envelope: the App builds the queue row's `Track` metadata from those fields, which is also the play report's snapshot |
| Two calls (§3.1) | `ApiCmd::Taste { seq, generation, half, body }` → `Event::Taste { seq, generation, half, answer }` — one command, a half on it (the D7 habit: a destination on the command, not a second command); arms in `worker::answer`, `web/api_worker.rs` and `App::apply_event`; the prefs read and write, the plug-in list and the resolve ride their own small commands with the same guards |
| The webapp's re-ping on 403 | `ApiCmd::DiscoveryProbe`, reading `discovery && discovery_taste >= 1` for this room (the sonic room's probe reads its own flag) |
| Segmented switch | The kit's Tabs, counts in the labels; `[gui] discover_tab` in `GuiPrefs` |
| "Last 30 days ▾", "Never played ▾" | The kit's Dropdown — the header's right for the window, the bottom row's left for the filter (see the log) |
| Album cards (12 × 6 covers) | Album rows ahead of the songs in the same table (see the log) |
| The `[⋯]` menu (TuiNetwork) | A list hung from the row in the dropdown's grammar (`frame_at` + `Surface::overlay`), cursor and hover per the kit; its links through `gui::opener::open_target`, copy through `kit::clipboard::copy` |
| The seeds sheet (PhoneSeeds) | A kit modal (`modal_frame_on`) with `[✓]` / `[ ]` rows and a modal primary Done; the window stays the header's dropdown, not repeated in the modal |
| "Start from a song…" (§2.8) | A capture of its own — a new `Capture` variant beside `Sonic` and `DjSeed`, never `Capture::Discover` (the TUI's) — with the banner over the room |
| Relative ages ("8 months ago") | Computed on the local clock from the answer's ISO `lastPlayed`; plural families (Wording) |
| Glyphs | `[▸] [»] [+] [⋯]` → `[>] [^] [+] [.]`, `•` → `*`, `▾` → `v`, `▰▱` → `■·`, `–` → `-`, the meter → nothing (the percentage alone) on `legacy_conhost()` |

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
per `seedSource` and the widened fact; tabs drawn by the flags with their
counts and `–`; the remembered tab; the table's columns per tab and the
narrow-width order; albums first with no hover verbs; "never" and the
ages; the fallback card; the scan-pending, off and failure lines; the
peer groups with an unreachable one dimmed and rowless; the list-cursor
law (nothing lit on arrival, a click stows, verbs need the cursor up);
the wheel; the glyph fallbacks. App tests: the two asks and their bodies
(network and peers only when advertised; `remember` on A alone; a filter
change asks the library alone); a stale seq and another generation fill
nothing; a Library row queues with the session's origin; **a Peers row
queues with `Origin { server, peer: Some(id) }`, its stream URL is the
federation proxy path, and its play report carries `peerId`**; activating
any row leaves `App.discover`, `App.now_discover` and the TUI's capture
unchanged; another pick pins a random song; the seeds modal's close sends
the exclusions once. The locale parity test. An expect leg
(`test/e2e/legs/gui-discover.exp`, `MSTREAM_GUI_DEMO=1`, first-paint gates,
drain-not-sleep). `cargo test --features desktop`; `cargo check --target
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
