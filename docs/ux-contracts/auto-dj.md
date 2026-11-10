# Auto DJ

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`439b4de4`, 2026-09-19) — `lib/screens/auto_dj.dart` (the settings screen: status, Queue, Continuity, Filters, Sources, and the multi-server body this surface does not port), `lib/widgets/auto_dj_start_sheet.dart` (the empty-queue chooser), `lib/widgets/queue_list.dart` (`toggleAutoDJ`, `_seedRandom` / `_armLibraryPick`, the `_EmptyQueue` opening-song buttons, the labelled `AutoDjButton`, the queue-row badge), `lib/widgets/player_panel.dart` (the same button on the player), `lib/singletons/auto_dj_manager.dart` (the settings, their persistence, `libraryFilters`, `batchParams`), `lib/media/audio_stuff.dart` (the session: `setAutoDJ`, `startAutoDJFromSeed`, `_startAutoDJFromSeed`, `_autoDJPick`, `sonicParams`, `shouldTopUpAutoDJ`, `shouldDeferDJPick`, `_resetAutoDJSession`, `_queueAutoDJSongs`, `restoreAutoDJ`), `lib/singletons/server_capabilities.dart` + `lib/util/server_version.dart` (the capability learner and the version floors), `lib/singletons/api.dart` (`fetchAutoDjSeed`, the opener). **Re-extracted 2026-09-20** from the 2026-09-06 pin (`695f4d0`); what moved in between: songs per fetch (`limit`, mStream #966), a federated peer hosts the DJ (mStream #946), sonic waits for the discovery scan (`discoveryReady`, mStream #879), opening-song buttons under an empty armed queue, multi-server sessions with vector seeds (out of scope here), and the toasts name the server by its display name. The screen is itself a port of the webapp's panel (`webapp/velvet/app.js viewAutoDJ`; `webapp/alpha/auto-dj.js` for the anchor semantics) — where they disagree, this contract says so. **Re-extracted 2026-11-10** against `1d4e8974` (2026-11-08) for the room's rebuild: `lib/screens/auto_dj.dart` is now one row per setting, each opening one sheet (`lib/screens/auto_dj_sheets.dart`, #222) — Start / Stop is the status; Play from · First song · Batch; **Must match**: Sonic similarity, Smart filters, Genres, Rating, Length; **Prefer**: Beat matching, Harmonic mixing, Skip words; a gated row stays in place with a few-word reason. With it came Recently played as a first song (`lib/util/auto_dj_recent.dart`, `lib/widgets/auto_dj_recent_menu.dart`), Smart filters — the mood filter on every pick (`lib/objects/mood_filter.dart`, `lib/widgets/mood_filter_sheet.dart`; #220, #218), Generated · Tags · Mixed genres (`genreSource`), Length to the second, Play from any set of servers (`lib/util/auto_dj_play_from.dart`, #225 — one server here, see Out of scope), quick config behind a split Auto DJ button (`lib/widgets/auto_dj_quick_config.dart`, #226), and a toggle whose Stop always stops (`toggleAutoDJ`). The GUI's translation of it is the canvas "Auto DJ Panel · GUI" (2026-11-09). |
| **Server API** | `POST /api/v1/db/random-songs` — one call, every knob (mStream `src/api/random.js`, Joi-validated with no unknown keys): `limit` (1–25, default 1; 6.26.0) · `ignoreList` (round-trip cursor, ≤ 500 ids; the server keeps the last 50) · `ignoreVPaths` (≤ 50) · `minRating` (0–10; ignored for a caller with no user — a federation key or guest) · `genres` (≤ 200) + `genreMode` (`whitelist` default / `blacklist`) · `bpmRanges` + `bpmRangesWide` (≤ 16 windows each, 0–1000) + `requireBpm` · `musicalKeys` (≤ 24 Camelot codes) + `requireMusicalKey` · `ignoreArtists` (≤ 100) · `similarTo` (1–8 paths) + `minSimilarity` (both or neither) · `similarToVector` + `similarToModelId` (6.26.0, out of scope) · `minDuration` + `maxDuration` (seconds, ≤ 86400) + `allowUnknownDuration` (6.25.0) · `genreSource` (`tag` or `model`, beside `genres`; absent matches both — mStream #1102, 6.32.2) · `moods` (the browse lists' filter — a value per scale, `tags`, `threshold` — a base condition never relaxed; mStream #1096, for a server whose `features.moodFilters` says it has scores). Answers `{songs: [..], ignoreList, sonic?: {similarity, similarities, poolSize}}`. Its refusals, all as `{"error": "..."}`: a **schema rejection** `"<key>" is not allowed` (400 from 6.12.0, 403 before — the body is the signal); nothing left in a sonic pool → 400 `No songs within the similarity range match criteria`; an unanalysed seed → 400 `Sonic seed track has not been analyzed yet`, an unscanned library → 400 `No tracks have been analyzed yet`; discovery switched off → 403 containing `discovery is disabled`; an expired token → 401/403 without a not-allowed body. `GET /api/v1/db/genres`; `POST /api/v1/db/genres` with `source` for one kind, answering `sources` (how many of each kind there are; mStream #716). `POST /api/v1/db/stats/recently-played` (`limit`) — the account's play history, newest first; empty for a caller with no user (a key, a guest). `GET /api/` is the capability source: `server` (the version), `features.discovery`, `features.discoveryReady` (whether the scan has produced vectors — `/api/v1/ping` never carries it), `features.moodFilters`, `user.vpaths`. Version floors the record keeps: 4.6.0 `ignoreVPaths` · 6.7.1 the BPM / key / genre / cooldown block · 6.15.2 the sonic pair · 6.25.0 the length window · 6.26.0 `limit` · 6.32.2 `genreSource`. random-songs is on the federation allowlist (mStream #946): a peer session — through the parent's proxy or over the peer's own tunnel with a guest token — can run the DJ. |
| **Already in this repo** | (As found at extraction on 2026-09-06 — the names are that day's code, since reworked.) Most of a DJ, in the older three-mode shape: `src/dj.rs` (Camelot math, same/half/double windows, the perceptual sonic slider, `build_random_request`, `Settings`), `[player.dj]` prefs and `player.autodj`, `AutoDjMode` and the `A` cycle, `maybe_autodj` (the queue-end top-up), `consume_dj` (queue the pick, start it if idle, the cursor), `autodj_pick` (`Similar` via nearest neighbours, `BpmKey` via random-songs, the sonic 400 retried once without the pool), `autodj_sample` (three picks without queueing), the TUI's Auto-DJ tab (`DjPanel` / `DjRow`) with its genre picker, `Event::{AutoDjPick, AutoDjSample, Genres}`, and the GUI bar's `auto-dj` toggle. Since 2026-09-06 the multi-server contract landed and gives this one its footing: every queued row carries its `Origin` and plays from its own server (`Reach`, `reach()`), federated peers are sessions of their own (proxied or direct), tunnels follow the queue with a hold for a row whose tunnel is down, the layered `/api/` payload is parsed (`LayeredInfo`), and the GUI queue panel has rows of its own to badge. **Missing**: the on/off-plus-toggles model, sources (`ignoreVPaths`), the length window, the keyword filter, `requireBpm` / `requireMusicalKey`, the session-locked Camelot anchor, the rolling / locked sonic anchors as the record defines them, songs per fetch, the readiness gate, the one-shot seed and the start chooser, the empty-queue openers, lane resets with in-flight discards, the capability learner, the failure taxonomy, the queue badge, the armed-not-playing restore, and the room itself. |
| **Target surface** | the GUI player — the bar toggle, the queue panel (its badge and its empty state), and the **Auto DJ room** — a Library nav room under TOOLS since 2026-09-23 (Settings › `Auto DJ ▸` before); the TUI's `A` and Auto-DJ tab follow through the shared App |
| **Status** | contract extracted 2026-09-06; **re-extracted against the moved record and settled 2026-09-20** — the open questions are decided below, and decision 10 was rewritten the same day to the record's model (the DJ is armed FOR a server, the session browses where it likes); implementation began 2026-09-20 (PLAN.md, Phase 10): **slices A1–A5 landed 2026-09-20/21** — the model, the worker, the GUI room and its surfaces, the TUI's keyword entry, and the rig run (`smoke/gui/scenario_dj.py`: the room, a batch, Preview, the server picker moving the DJ to a proxied peer with the next turn picked from there). Left for a server with discovery data: the sonic path end to end (a pool answered, clause 30's degrade) and a direct-tunnel peer lane. **2026-09-23** — the room moved from Settings to the Library's nav column, a TOOLS group beside the sonic room, where the record's browse root and desktop rail keep it (decision 5 amended; the deviations log). **2026-11-10** — the room rebuilt from the re-extracted record and its canvas: one row per setting, each opening one sheet; First song with Recently played; Smart filters; the genre kind; Length to the second; Play from with a home; the bar's split button and quick config; the toggle's Stop always stops (clauses 2, 8, 16, 20, 40–58; decisions 13–19; the deviations log) |

## Intent

Keep the music going. When the queue runs low, Auto DJ asks the server for
a few more songs that fit — the libraries the user scoped, the rating
floor, the genres, the tempo and key of what is playing and, where the
server has listened to the library, the sound of the session — and drops
them in as the next rows, a batch ahead, for as long as the DJ is on.
Everything it does is visible: its rows wear a badge in the queue, the
room's state line says where it is picking from, and the moment a
constraint cannot be met it says so once and keeps playing rather than
falling silent.

## Entry points

1. **The toggle** — the bar's `auto-dj` control, lit while the DJ runs
   anywhere, and `A` in the TUI (in the GUI's browse rooms `A` is the
   bar's queue-all; the bar's control, `D`'s room and `A` elsewhere arm
   it there). It works the record's way *(re-extracted 2026-11-10)*:
   with the DJ off it arms it FOR **Play from's server** — the home
   chosen in the room, or whichever server the DJ last ran on, while it is
   still saved; else the session's — whichever server is browsed; with
   the DJ on, anywhere, it switches it off. Stop always stops. (Before, a
   DJ armed on another server was moved to the browsed one, so one button
   meant two things; moving the DJ is Play from's.) **The TUI has no Play
   from row**, and a home there would be a server nobody could see or
   change: its `A` arms the server it stands on, as it always has, and
   that one becomes the home the GUI starts on. The note line confirms
   "Auto DJ on" / "Auto DJ off" ("Auto DJ on — picking from {server}" when
   more than one server is saved).
2. **The room** — the Library's nav column, `Auto DJ` under a TOOLS
   group (the record's browse-root card and its desktop rail's TOOLS
   item), `D` from the keyboard; the row wears the DJ's live state the
   way the record's card does. Inside: Start / Stop, how the session runs
   (Play from · First song · Batch), then MUST MATCH and PREFER — one row
   a setting (clauses 40–57).
3. **The queue panel's empty state** while the DJ is armed: "Auto DJ is on
   and needs an opening song." with `Pick a random song`, `Choose a
   song` and, where the server has some, `Recently played ▾` (clauses 8,
   16, 62).
4. **Discover › "Start a sonic session"** — clears the lane and the
   queue, seeds the DJ with the track (or a random one), switches it on.
   *Out of scope here (the Discover slice), named because it reuses this
   feature's seed and lane.*
5. **Launch** — a DJ remembered as on comes back **armed, never playing**.
6. Android Auto / CarPlay "Shuffle All" — *out of scope.*
7. **Quick config** — the chevron half of the bar's split `auto-dj`
   button: Play from, Sonic similarity, Smart filters and the way to the
   room, in a popover over the bar (clause 58).

## States & flows

Off → switched on → (empty queue only) the **opening question** — a
random song / one chosen from the library / one of the songs played last
(clause 8), the way to start rememberable → **armed**, with a
session **lane**: the ignore cursor, the Camelot anchor, the sonic history
and pin, the once-per-lane warning budgets, the suppressed sonic keys.
Every way the user steers somewhere new — switching on, moving the DJ to
another server, clearing the queue, removing the DJ's server — starts a
**new lane**; re-arming on the same server does not, and browsing another
server does not either: the DJ is armed FOR a server, and the session goes
where it likes. Off ends the lane. Settings edits apply to the running lane
at once. A lane picks from ONE server: the DJ's.

## Behavior contract

### Switching on

1. With a queue the DJ **follows what is there** — nothing is replaced. On
   the last row and idle, a fetch is made and its first song played at
   once; on the last row and playing, a fetch lands as the next rows;
   anywhere else nothing happens until the queue's end.
2. With an **empty queue** the DJ needs an opening track, and **First
   song** decides *(reworded 2026-11-10)*: *Ask* (default) shows the
   question under the row's own name — **Random** · **Choose** ·
   **Recently played** · **Remember** — Random and Choose remembered skip
   it; Recently played names a song, so it asks every time too, its list
   already down once the songs are in hand. Dismissing the question
   leaves the DJ off.
3. **Random** ("Surprise me") fetches the opener with the **same library filters
   every later pick obeys** (sources, rating, genre, length) and nothing
   else — never the plain random pick, never `limit`, no continuity keys
   — plays it, then fetches followers (clause 7). Nothing matched (a 400
   under constraints) → "No songs match your Auto DJ filters — try
   loosening them"; any other failure → "Couldn't fetch a song from the
   server." The DJ stays off.
4. **Choose** ("Let me choose") hands off to the library under the banner "Pick the
   opening song — choose a track anywhere in the library"; the DJ switches
   on when a row lands, not before; the pick is track one and the sonic
   seed; the user stays where the pick was made. Esc cancels the pick and
   leaves the DJ off.
5. The opening seed is **one-shot**: it opens exactly one session and is
   consumed by the queue clear that starts it, so no later empty-queue
   start can replay it. A seed that cannot open (its track gone, its
   server unreachable) is dropped rather than left to hijack a later arm.
6. **A federated peer hosts the DJ like any server** *(changed
   2026-09-20; the 09-06 record refused it)*: random-songs is on the
   federation allowlist, its filepath seeds resolve on the peer, and its
   picks play the way the peer's browsed rows do — through the parent's
   proxy, or over the peer's own tunnel with the guest token (multi-server
   contract, clauses 27 and 30). Two things differ for a peer: `minRating`
   is not sent (a key or a guest has no stars; the server would ignore
   it) and the rating row is hidden; the sources are the libraries the
   peer shares.
7. **Followers after a seeded start**: the seed is track one; one fetch
   is made and awaited so a failure surfaces, and a second only if the
   queue is still under three deep — a batch (clause 27) already puts the
   session under way.
8. **Recently played** *(new 2026-11-10)*: the songs the account last
   played on the server the opener is for, newest first, each once,
   twenty at most — only that server's, since a path names a row in one
   library. Asked of that server when the question opens and when an armed
   queue runs out; left out while there is nothing to offer, and for a
   peer, whose key has no history. A song of it opens the session like a
   chosen one: track one and the seed, the user's own (no badge), on the
   server it was played on. With **Remember**, what is kept is the way to
   start — never a song. Its lists show as many songs as their place
   affords; ↑ ↓ and the wheel walk them, the window following the cursor,
   so each of the twenty takes a click.

### The session lane

10. A **new lane** resets the ignore cursor, the Camelot anchor, the sonic
    history and the locked pin, the warning budgets, and the sonic keys
    suppressed by clause 30. Triggers: switching on, moving the DJ to
    another server, clearing the queue, a server removal that empties the
    queue. Re-arming on the same server is not a new lane; nor is a switch
    of the browsed session.
11. A pick **in flight when the lane changes is discarded** — it must not
    repopulate a cleared queue, overwrite the fresh cursor, or seed the
    new lane's history with the dead one's tracks. (The record stamps each
    call with a lane epoch and drops a reply whose epoch is stale; a
    request is nearly always in flight during a session.)
12. A **queue clear keeps the DJ armed and adds nothing** — no chooser, no
    surprise pick; the DJ resumes from a clean slate at the next queue
    end, or when the user opens a song from the empty state (clause 16).
13. **The top-up rule**: exactly when a real playback session stands on
    the **last row** — the current index is the last, the player is not
    idle, and no failed-track skip walk is running. Idle index emissions
    on the last row never top up (a batch add onto a fresh player once
    stacked eleven picks in nine seconds); neither does a walk through
    broken streams (the walk and the DJ feed each other unboundedly); nor
    a reorder that re-points the current index without playback moving.
14. **A batch ahead**: a fetch lands as the next rows in the server's order
    (best tier first); the play-now flag belongs to the first row and only
    when the queue was empty. Overlapping fetches are dropped; every
    trigger re-fires once the in-flight one lands. A pick already in the
    queue is not queued again — nor one a restore let go of (multi-server
    clause 40: the queue comes back with the DJ's last 100 played rows,
    and remembers the paths of the rest).
15. Every DJ pick joins the **rolling sonic history** — kept even while
    sonic mode is off, so switching it on mid-session already has the
    session's recent sound to anchor on.
16. **The empty queue, armed**: the panel's empty state offers `Pick a
    random song` (the filtered opener, clause 3, then followers), `Choose
    a song` (clause 4) and — where the server has some — `Recently played
    ▾`, whose list hangs under it (clause 8); each opens a seed on the ALREADY armed
    DJ without a new lane (re-arming the same server is not one). With the
    DJ off the empty state is just its label. A seed that cannot open is
    cleared, not kept.

### The request

19. **The DJ's server answers every pick**, whichever server the session
    browses: the request rides a reach of its own — a saved server's
    address, token and trust; a tunnel server's loopback with its token; a
    peer through its parent or over its own tunnel — and the DJ's server is
    a **tunnel target while the DJ is armed** (the record's tunnel-follows-
    the-DJ), kept up beside the session's and the queue's. Its
    capabilities are its own, learned by a probe of its `/api/` when the
    DJ arms and on every return of its tunnel: `discovery`,
    `discoveryReady`, the version, the libraries.
20. **The library filters**, shared by every pick and the opener:
    `ignoreVPaths` (the sources switched OFF), `minRating` when set and
    the server is not a peer, `genres` + `genreMode` when the genre filter
    is on and non-empty — with `genreSource` beside them when the list
    matches one kind of genre only (both is the default and sends none) —
    `minDuration` / `maxDuration` when the length window is on and a bound
    is off its rail (a rail means unbounded and is not sent; "no maximum"
    is a day, 86400 s), `allowUnknownDuration` only alongside a real
    bound, and `moods` when Smart filters hold a mood and the server SAID
    it has scores (clause 55) — a server that has not answered yet is not
    sent the key.
21. **BPM continuity**: windows of ± tolerance BPM (1–20, default 8)
    around the PLAYING track's tempo at same, half and double time; the
    wide set is tolerance + 2; `requireBpm`, so the waterfall never falls
    back to untagged picks. No tempo tag on the playing track → no
    windows; the pick is free. A batch is judged against the track that
    was playing when it was fetched.
22. **Harmonic mixing**: a **session Camelot anchor**, locked on the first
    keyed DJ pick (or the playing track's key on the first call) and
    reused all lane long; `musicalKeys` = the anchor's neighbourhood;
    `requireMusicalKey` whenever the switch is on, so even the first pick
    is keyed and can lock the anchor.
23. **Sonic similarity** sends `similarTo` + `minSimilarity` (raw cosine,
    .30–.80, default .55) only when the DJ's server reports
    `discovery` on and does not report `discoveryReady` false (clause 36;
    a server that cannot report readiness holds nothing back), the switch
    is on, the keys are not suppressed this lane (clause 30), and every
    seed belongs to that server (paths are per-library). Seeds by
    **anchor mode**: *rolling* ("Follow the vibe", default) — the last 5
    DJ picks, deduped, most recent last; else the explicit seed; else the
    playing track. *Locked* ("Stay on seed") — one pin, set on the lane's
    first pick (the seed, else the playing track) and reused. A cold start
    with nothing to anchor on stays plain random until the first pick
    seeds it.
24. `ignoreList` is an opaque **round-trip cursor**: send back what came,
    unchanged (the server keeps the last 50 and caps the wire at 500).
25. **The capability filter**: before sending, drop every key the server
    is *known* (a version floor, when `/api/` gave a version) or *learned*
    (this session) not to accept. A rejection whose body reads `"<key>"
    is not allowed` — status 400 or 403; the body is the signal — teaches
    the key for the rest of the session and the request goes again
    without it, so one new parameter can never take Auto DJ down on an
    older server. The loop terminates by construction (each pass removes a
    key). Only a not-allowed body retries: an auth failure or a bad value
    falls through to clauses 30–32. The learner lives in memory only,
    keyed by server identity, so an upgrade is noticed at the next launch
    — and it forgets a server whose version changes.
26. **The keyword filter** is client-side: a song whose title, artist,
    album or filepath contains any word (case-insensitive) is refused. A
    batch keeps whatever part of it passes; only an answer blocked in
    FULL is re-asked with the updated cursor, up to 5 times; then the last
    answer is taken whole rather than stalling the queue forever.
27. **Songs per fetch**: `limit`, 1–25, default 4 — enough runway that a
    slow tunnel never leaves the queue dry between turns, small enough
    that a changed setting shows within a few songs. At 1 the key is left
    off (the pre-batch wire shape); the opener never carries it; a server
    known to predate 6.26.0 has its row hidden (clause 50) — a stored
    value above one is still sent and left to the learner (deviation
    2026-09-26); the learner covers the rest.
28. Every request is **bounded** (the record's 15 s; here the api client's
    own timeouts) so a black-hole server cannot hang a turn; a timeout is
    a network failure (clause 33).

### When a constraint cannot be met

30. Sonic fails loud server-side; the client **degrades the constraint,
    not the music**. Matched on the lowercased error body: "similarity
    range" (an empty pool), "analyzed" (an unanalysed seed or library),
    "discovery is disabled" — each drops the sonic keys for the rest of
    the lane and retries the same pick without them. The first two say so
    **once** per lane — "Auto DJ: nothing is within the similarity range,
    so it is playing without that filter. Loosen the match slider to use
    it again." / "Auto DJ: the discovery scan hasn't reached these tracks
    yet, so it is playing without sonic similarity." — the third stays
    silent (the user switched it off themselves). Logged every time.
31. A **dead locked pin** (a 404, or an "analyzed" body) is dropped so the
    next call re-pins from the playing track — locked mode never
    re-derives on its own.
32. **An auth failure** (401/403 that is not a schema rejection): "Auto DJ
    stopped — the server session expired. Sign in again in Manage
    servers." — once per lane; the DJ stays armed and the next queue end
    tries again.
33. **A network failure** is silent (an offline queue end is normal),
    logged once per outage, and the pick is remembered as **owed** and
    retried when the connection returns; the queue **parks** at its end
    instead of stopping (the multi-server contract's hold, clause 37
    there, is the same posture).
34. A working pick clears every warning budget and the owed-pick flag.
35. **The DJ's server behind a tunnel** (a Quick Connect server, a direct
    peer): a tunnel that is dialling or down means the pick is owed from
    that moment — not from when a wait gives up — and the tunnel's
    connected edge retries it; a track that ends meanwhile parks (clause
    33) rather than stopping. Logged once until the tunnel serves again.
36. **Readiness**: discovery on but the scan has produced no vectors yet
    (`discoveryReady` false) → sonic is not sent this lane, the room's
    sonic row says why ("Discovery is on but the scan hasn't produced data
    yet — picks stay random until it does."), and the log says so once per
    lane. The switch keeps its setting for when the scan lands.

### The room

*(Re-laid 2026-11-10 from the record's rebuilt screen and the canvas "Auto
DJ Panel · GUI": one row per setting, each opening one sheet. Clauses 40–53
keep their numbers and say the new room; 54–58 are new. What the rows DO —
the request, the lane, the degrades — is clauses 1–36, unchanged.)*

40. **Start / Stop is the status.** One button the rows' width: **Start
    ▸** in the accent while the DJ is off, **Stop** lit in the OK colour
    while it runs — never the destructive one. Nothing else on the page
    says whether it is on: the state line and the status sentence are
    gone (the nav row keeps its `•`). Start goes through the same toggle
    every entry point uses (a direct arm skipped the opening question once
    and replayed a stale seed); Stop stops wherever the DJ is armed.
41. **A row** is a name, its value ending at one right edge, and `▸` (it
    opens a sheet) or `▾` (it opens a list, where a pick applies at once).
    A value is DIM for Any, Off and how the session runs (Play from, First
    song, Batch); OK while it shapes the picks; GOLD behind `!` when a
    running session had to drop it (clause 43). **A setting the DJ's
    server cannot do keeps its place**: DIM whole, its reason in a few
    words where the value would be, no chevron — it takes no click and the
    keyboard passes it. Rows never move between off and on, and never
    scroll. The reasons: `Needs server {version}+` (clause 50) ·
    `Discovery off` · `Still scanning` · `No mood data` · `Not on peers`.
    A reason never loses its words: where a name and its reason do not
    both fit, the name gives way.
42. **Play from** — the server the picks come from, and its folders.
    Drawn only where there is something to choose: more than one
    selectable server saved, or more than one library on the server. Its
    sheet lists every selectable server, a peer under its parent — ONE is
    ticked (a session across several is out of scope) — and under them
    the libraries of the server the rows are about, a checkbox each; the
    last one on cannot be switched off ("At least one source is
    required."). The server ticked is Play from's **home**: a DJ running
    elsewhere starts afresh there (a new lane); one already there carries
    on; a DJ that is off stays off and will start there. The home is also
    whichever server the DJ last ran on, however it was started. The
    session's own header switch never moves the DJ. More servers and
    folders than the sheet affords stand behind its window: the wheel and
    the kit's scrollbar move it, and a walking key brings the hand's line
    into view.
43. **Sonic similarity** — `{pct}%` (OK), `Off`, or the server's reason
    (`Needs server 6.15.2+`, `Still scanning`, `Discovery off`). Its
    sheet: `[✓] On`; the floor as a bar across the band, .30–.80, two
    points a cell, with `-` `+` and ← → a point at a time; **Drift · Stay
    close** (the anchor: rolling · locked); and "Only pick songs that sound
    like the session, using the server's audio analysis." Switched off,
    the bar and the anchor rest — DIM, no click, the keyboard passes them.
    **ON by default** (safe only because clause 30 degrades instead of
    stopping). A running session that dropped the pool says why on the row
    in GOLD — `! Too strict`, `! Still scanning` — and the sheet still
    opens, since it is where the user goes to loosen it; a change there to
    the switch, the floor or the anchor gives the lane another go with
    them (the record's `retrySonic`). Switching sonic OFF clears the
    lane's anchors.
44. **Beat matching** (the record's Tempo — BPM continuity, clause 21) —
    `± {bpm} BPM` or `Off`. Its sheet: `[✓] On`, the tolerance as a bar, a
    cell a BPM (1–20), and "Prefer picks within a tempo window of the
    current song. Honours half/double-tempo equivalence." Gated `Needs
    server 6.7.1+`.
45. **Harmonic mixing** — the one switch on the page: `[✓]` / `[ ]` at
    the values' edge and no sheet; a click, Space or Enter flips it. Under
    it, where the room has the row to spare (clause 54), "Next song in a
    key that mixes well". Gated `Needs server 6.7.1+`.
46. **Rating** — `{stars}★+` or `Any`. Its sheet: five stars — a click on
    one sets it whole, a second click on it halves it; ← → step by halves
    — and "Only pick songs at or above this rating." Per library. `Not on
    peers` for a peer (clause 6).
47. **Length**, to the second — `{min}–{max}`, `{min}+`, `Under {max}`,
    or `Any`; m:ss, and h:mm:ss from an hour up. Its sheet: **Shortest**
    and **Longest**, each a typed length (`2:00`, `90`, `1:02:30`) with
    `-` `+` stepping fifteen seconds; an empty side has no limit; what
    does not read as a length shows GOLD and leaves that side as it was;
    bounds that cross change places; **Include unknown** only beside a
    real bound (off by default: an unknown length cannot satisfy a bound,
    and a half-scanned library shrinks more than intended). Both sides
    empty is Any, which is the window switched off. Gated `Needs server
    6.25+`.
48. **Genres** — the chosen genres, sorted; `Skip {genres}` when the list
    is what to leave out; or `Any`. Its sheet: **Only · Skip** (whitelist
    · blacklist — a whitelist blocks untagged tracks, a blacklist lets
    them through); **Generated · Tags · Mixed**, the kind of genre the
    list matches (`genreSource`), offered only where the server says it
    has both kinds; a search line that takes every letter typed; the list
    — the ones already chosen first — checkboxes toggling the draft, at
    most 200; "{count} selected". Per library. Nothing chosen is the
    filter off; the way it cuts and the kind are kept. Its empty states:
    "No genres found on this server." · "No genres match "{query}"." ·
    "Could not load genres". Gated `Needs server 6.7.1+`.
49. **Skip words** (the keyword filter, clause 26) — the words, or `Off`.
    Its sheet: a line that takes every letter typed (hint: e.g. "live" or
    "remix"), Enter adding the word; the words, each with its `[X]`; at
    most 50, each once whatever its case; a word still on the line when
    the sheet closes counts. No words is the filter off. Client-side, so
    it is never gated.
50. **Version gates**: a row whose setting the server is KNOWN to predate
    says `Needs server {floor}+` in its place — 6.7.1 Genres, Beat
    matching and Harmonic mixing · 6.15.2 Sonic similarity · 6.25 Length ·
    6.26 Batch — and the key stays out of the request (clause 25).
    `genreSource` (6.32.2) gates nothing on the page: the kinds are
    offered by what the server says it has, and the key stays home on a
    server known to predate it. The version is `/api/`'s `server`; a
    server that answers only the flat ping has none, and an unknown
    version or a fork gates nothing and learns from a rejection (clause
    25) instead.
51. **A sheet edits a draft, and closing it any way applies it** — Done,
    Esc and Enter alike (the mood Filters' law, library-rooms clause 32);
    one closed as it was opened writes nothing. Applied, an edit reaches
    the running lane at once — no restart. Per-library settings (folders,
    rating, the genre rule) belong to the server they were set against;
    session-wide ones (batch, sonic and its anchor, Smart filters, length,
    skip words, beat matching, harmonic, cooldown, First song) travel with
    the user. First song's list, Harmonic's switch and quick config have
    no draft: there a press applies at once.
52. **Batch** (songs per fetch, clause 27) — "{n} songs" (1–25, "1
    song"). Its sheet: a bar, a cell a song, and "How many songs Auto DJ
    queues each time it runs. Continuity filters judge the whole batch
    against the song playing when it was fetched." Gated `Needs server
    6.26+` — a stored value above one is then not sent.
53. **Preview picks** *(the player's own, decision 6)*: `Preview picks ▸`
    on the title row opens a sheet and asks three picks with the current
    settings, none of them queued, each fed into the next call's cooldown
    so the three differ — a pick's badge, title, artist and length a row —
    with the pool they were drawn from as the foot's note ("{n} tracks in
    the sonic pool"). How a setting gets judged before it is committed to.
54. **The floor**: the room's rows always stand whole, with no scroll
    bar. What the height affords beyond them comes back in one order: the
    blank row before MUST MATCH, then the one before PREFER; then Start /
    Stop as its 3-row frame — bare, then with a blank row under it, then
    with one over it too — and last Harmonic's line. Short of the frame,
    the button is the kit's one-row slab at the title row's right, Preview
    beside it. At 100×24 — the slab, the two blank rows — all twelve
    settings stand; at 100×30 everything does, on the room's 23 rows.
55. **Smart filters** — the mood filter (library-rooms clauses 30–36),
    kept to for every pick: the moods and tags chosen, the level after
    them when it is not Standard, or `Any`. Its sheet IS the mood modal,
    retitled with the row's name and its value; nothing chosen is the
    filter off, and the level last picked is kept for when a mood comes.
    Session-wide: a mood means the same on every server. `No mood data`
    on a server with no scores; `Not on peers` for a peer.
56. **First song** — what switching on with nothing queued does (clause
    2): **Ask · Random · Choose · Recently played**. A list under the row
    (the kit's Dropdown): the answer in force wears `•`, a pick applies
    and closes, a click anywhere else closes and keeps.
57. **Artist cooldown** *(the player's own, decision 9)* — "{n} artists"
    or `Off`. Its sheet: a bar, a cell an artist (0–20; none lit is Off),
    and "Keep recently played artists out of the next picks."
58. **Quick config** (the record's split Auto DJ button): the bar's
    `auto-dj` frame is two halves — the word starts and stops as it
    always has, and the chevron opens a popover rising from the rule
    between them — over the nav's rule, never over its rows, and never
    over the bar — its groups a blank row apart: **Play from** (the servers as
    one-ticked choices while there are four at most; past that one row to
    the sheet; not drawn for one server), **Sonic similarity** with its
    value and **Drift · Stay close**, **Smart filters** (a row to its
    sheet), and `All Auto DJ settings ▸`. Every change applies at once —
    no draft, no Done. A setting the server cannot do rests with its
    reason. A click anywhere else, the chevron again, or Esc puts it away.

### Transparency and restore

60. Every DJ-picked row in the queue wears a **badge** — a sonic pick and
    a classic random pick differently; user-queued rows wear none. It
    persists with the queue.
61. **Remembered as on** brings the DJ back ARMED at launch, never
    playing; it engages at the next queue end. A user toggle that beats
    the restore wins.
62. The queue panel's **empty state** is clause 16's, live: the buttons
    appear when the DJ is armed and leave when it is switched off.
63. **The log is the DJ's second voice**: every dropped key, learned
    rejection, degraded constraint, deferred pick and failed fetch is one
    `[dj]` line at info — a filter that silently does nothing is
    indistinguishable from one that works, and the log is where the
    difference lives.

## Wording

The record ships `autoDj*` keys in all ten locales — carried over at
implementation, not re-made. The player's keys are `gui.dj.<name>` for the
room, the chooser and the panel, and `dj.<name>` for the shared App's notes
(both shells show them). English reference; `⚑` marks a terminal rewording
(logged below):

| String | Record key |
|---|---|
| Auto DJ | autoDjTitle |
| Auto DJ on · Auto DJ off · Auto DJ on — picking from {server} ⚑ | autoDjEnabled · autoDjDisabled · autoDjEnabledFor |
| Auto DJ is on · Songs are picked from {server} when the queue runs low. | autoDjStatusOn · autoDjStatusOnDetail |
| Auto DJ is off · Start below. The session's server's library will be used. ⚑ | autoDjStatusOff · autoDjStatusOffDetail |
| Start Auto DJ · Stop Auto DJ | autoDjStart · autoDjStop |
| Start Auto DJ with what? · Nothing is queued, so the DJ needs an opening track. With a queue it just follows what you already have. | autoDjStartTitle · autoDjStartSubtitle |
| Surprise me · Pick a random song from the library and build outward from it. | autoDjStartRandom · autoDjStartRandomSub |
| Let me choose · Open the library and pick the opening track yourself. | autoDjStartPick · autoDjStartPickSub |
| Remember this · Skip this question next time and always start this way. | autoDjStartRemember · autoDjStartRememberSub |
| Pick the opening song — choose a track anywhere in the library ⚑ | autoDjStartPickBanner |
| Auto DJ is on and needs an opening song. · Pick a random song · Choose a song | queueEmptyDjHint · queueEmptyDjRandom · queueEmptyDjChoose |
| On an empty queue · What Auto DJ does when you switch it on with nothing queued. · Ask | autoDjOnEmptyQueue · autoDjOnEmptyQueueSub · autoDjStartAskShort |
| No songs match your Auto DJ filters — try loosening them · Couldn't fetch a song from the server. | autoDjSeedNoMatch · autoDjSonicSeedFailed |
| Queue · Continuity · Filters · Sources | autoDjSectionQueue · autoDjSectionContinuity · autoDjSectionFilters · autoDjSectionSources |
| Songs per fetch · How many songs Auto DJ queues each time it runs. Continuity filters judge the whole batch against the song playing when it was fetched. · {count} songs / 1 song | autoDjSongsPerFetchTitle · autoDjSongsPerFetchSubtitle · autoDjSongsPerFetchValue |
| At least one source is required. | autoDjOneSourceRequired |
| Sonic similarity · Only pick songs that sound like the session, using the server's audio analysis. | autoDjSonicTitle · autoDjSonicSubtitle |
| This server doesn't have discovery data — picks stay random. · Discovery is on but the scan hasn't produced data yet — picks stay random until it does. · Needs server 6.15.2 or newer | autoDjSonicUnavailable · autoDjSonicNotReady · autoDjSonicNeedsNewerServer |
| Match strictness · {pct}% or closer | autoDjSonicStrictness · autoDjSonicStrictnessValue |
| Anchor · Follow the vibe · Stay on seed · Each pick follows the session's recent sound — it can slowly evolve. · Every pick stays close to the seed song for the whole session. | autoDjSonicAnchor* |
| BPM continuity · Prefer picks within a tempo window of the current song. Honours half/double-tempo equivalence. · Tolerance · ± {bpm} BPM | autoDjBpm* · autoDjTolerance |
| Harmonic mixing · Prefer picks in keys that mix well with the locked song (Camelot wheel neighbours). | autoDjHarmonic* |
| Minimum rating · Only pick songs at or above this rating. · Any | autoDjMinRating · autoDjMinRatingSubtitle · autoDjRatingAny |
| Track length · Skip interludes and long mixes by only picking tracks in a length range · Length · Any length · Over {min} · Under {max} · {min} to {max} · Include tracks of unknown length · Tracks your server has not read a length for are skipped otherwise | autoDjDuration* |
| Genre filter · Whitelist plays only matching tracks; blacklist skips them. · Whitelist · Blacklist · Pick genres · No genres selected. Choose "Pick genres" to start. ⚑ · Search genres… · {count} selected · No genres found on this server. · No genres match "{query}". · Could not load genres | autoDjGenre* · autoDjWhitelist · autoDjBlacklist · autoDjPickGenres · autoDjNoGenres* · autoDjSearchGenres · autoDjSelectedCount |
| Keyword filter · Skip picks whose title, artist, album, or filepath contains any of these words. · No keywords. Add words below to start filtering. · e.g. "live" or "remix" | autoDjKeyword* · autoDjNoKeywords |
| BPM continuity, harmonic mixing and the genre filter need a newer server. Update to get them. | autoDjNeedsNewerServer |
| Add a server first. | autoDjAddServerFirst |
| Auto DJ: nothing is within the similarity range, so it is playing without that filter. Loosen the match slider to use it again. · Auto DJ: the discovery scan hasn't reached these tracks yet, so it is playing without sonic similarity. · Auto DJ stopped — the server session expired. Sign in again in Manage servers. ⚑ | (the record's literals in its audio handler — keys here: `dj.sonic_range`, `dj.sonic_unscanned`, `dj.auth_expired`; the rest of the App's notes are `dj.*` too: `needs_server`, `not_reachable`, `on` / `on_from` / `off`, `one_source`, `max_keywords` / `max_genres`, the three sonic reasons, `fetch_failed` / `fetch_failed_why`, `no_match` / `no_match_lane`, `picking_opener`, `nothing_new`, `picked` / `picked_more`, `preview_failed` with `fail_auth` / `fail_network` / `fail_no_match`) |
| Preview picks · {n} tracks in the sonic pool · picking… ⚑ (this surface only) | `gui.dj.preview`, `gui.dj.pool`, `gui.dj.picking` |
| TOOLS · Auto DJ (the nav group and its row) | `gui.nav.tools`, `gui.nav.dj` — the desktop rail's literal header and the record's autoDjTitle; the card's On/Off line (browserAutoDjOn/Off) is the row's `•` |

**The rebuilt room's words (2026-11-10)** — the record's, from its ARB
files at `1d4e8974`, in all ten locales; the rows above that the room no
longer says (the status sentences, the section names, the old rows' own
labels, the old chooser's title and descriptions, the anchor's hints,
Whitelist / Blacklist and theirs, "No genres selected" and "No keywords")
left the locale files with it — 73 keys. The lists' own empty states
("No genres found on this server." and its two neighbours) stay. The
App's `dj.*` notes are unchanged.

| String | Record key → `gui.dj.` |
|---|---|
| Start · Stop | start · autoDjStopShort → `start_short`, `stop_short` |
| Play from · First song · Batch | autoDjPlayFrom · autoDjFirstSong · autoDjBatch → `play_from`, `first_song`, `batch` |
| Must match · Prefer (drawn UPPERCASE, the room's headers) | autoDjMustMatch · autoDjPrefer → `must_match`, `prefer` |
| Smart filters · Genres · Rating · Length · Beat matching · Skip words | autoDjMoodTitle · autoDjGenres · autoDjRating · autoDjDurationRange · autoDjTempo · autoDjSkipWords → `smart`, `genres`, `rating_row`, `length_row`, `beat`, `skip_words` |
| Next song in a key that mixes well | autoDjHarmonicHint → `harmonic_hint` |
| Ask · Random · Choose · Recently played · Remember | autoDjStartAskShort · autoDjFirstSongRandom · autoDjFirstSongChoose · autoDjFirstSongRecent · autoDjRemember → `ask`, `first_random`, `first_choose`, `first_recent`, `remember_short` |
| Any · Off · On · {pct}% · {stars}★+ | autoDjAny · commonOff · commonOn · autoDjPercent · autoDjRatingValue → `any_value`, `value_off`, `switch_on`, `percent`, `rating_value` |
| {min}+ · Under {max} · {min}–{max} · Include unknown · Shortest · Longest | autoDjLengthOver · autoDjLengthUnder · autoDjLengthBetween · autoDjIncludeUnknown · autoDjLengthShortest · autoDjLengthLongest → `length_from`, `length_upto`, `length_window`, `include_unknown`, `shortest`, `longest` |
| Only · Skip · Skip {genres} · Generated · Tags · Mixed | autoDjGenresOnly · autoDjGenresSkip · autoDjGenresSkipValue · genreSourceGenerated · genreSourceTags · genreSourceMixed → `genres_only`, `genres_skip`, `genres_skip_value`, `source_generated`, `source_tags`, `source_mixed` |
| Drift · Stay close | autoDjDrift · autoDjStayClose → `drift`, `stay_close` |
| Needs server {version}+ · Discovery off · Still scanning · Too strict · Not on peers · No mood data | autoDjNeedsServer · autoDjDiscoveryOff · autoDjStillScanning · autoDjTooStrict · autoDjNotOnPeers · autoDjNoMoodData → `needs_server`, `discovery_off`, `still_scanning`, `too_strict`, `not_on_peers`, `no_mood_data` |
| All Auto DJ settings · {count} max | autoDjAllSettings · autoDjMax → `all_settings`, `max` |
| Type a length to the second (2:00, 90, 1:02:30) or step by 15 seconds. Empty means no limit on that side. ⚑ (this surface only: the record's wheels need no line) | `length_help` |
| The sheets' help lines — the old room's descriptions, kept: sonic, batch, tempo, cooldown, rating, skip words | `sonic_sub`, `songs_per_fetch_sub`, `bpm_sub`, `cooldown_sub`, `rating_sub`, `keyword_sub` |

## Out of scope here

- **Multi-server sessions** — the record's "Play from every server"
  switch, vector seeds (`similarToVector` + `/discovery/local/embeddings`),
  the fan-out with per-server cursors and scoring, the participants
  readout ("{n} servers are taking part", "{n} still connecting"), the
  "Settings for" per-library editor, and the tunnels it keeps up. A lane
  here picks from one server, the session's. Consequence: a queue that
  mixes servers still plays (the multi-server contract), but the DJ's
  picks all come from where the user is browsing. Revisit once the single
  lane is proven; the record's split of the form into "The session" and
  "Each library" is the shape to grow into.
- **Play from across several servers** (the record's #225): the fan-out,
  the vector seed, the sit-out reasons, All servers, and the Genres and
  Rating sheets' tab per server. Play from here ticks ONE server (clause
  42); moving it moves the DJ and starts a new lane. The canvas drew the
  several-server state ("Several servers", "Sheet — Genres, with several
  servers") as where this grows.
- **The docked sheet** (the canvas's "Alternate"): a sheet beside the rows
  with the queue folded. The modal alone is built.
- **Discover's "Start a sonic session"**: the Discover slice, which will
  reuse the seed and the lane.
- **Android Auto / CarPlay**, and the cast/reseed paths of the record's
  handler.
- **The webapp's extras** the record dropped: Last.fm similar-artists
  mode, the crossfade row, the dice.

## Translation notes (terminal GUI) — settled 2026-09-20

| Record | Here |
|---|---|
| The labelled Auto DJ button (queue header + player) | The bar's `auto-dj` toggle, lit while the DJ runs anywhere — arm here / off / move here (decision 1, entry point 1); `A` in the TUI and outside the GUI's browse rooms (where `A` is queue-all); the confirmation on the note line names the server when several are saved |
| The settings screen | The **Auto DJ room**, a Library nav room (since 2026-09-23; behind Settings › `Auto DJ ▸` with a `◂` back before): a TOOLS group under Search in the nav column — `Auto DJ`, then `Sonic path` — the row dim, bright under the pointer, the accent when active like every nav row, plus a `•` in the ok colour while the DJ is armed (the record's card line "On"/"Off"); `D` from anywhere (the tenth room: digits stop at 9 and `0` is the Now Playing screen). Inside, the sonic room's grammar — BOLD title, the state line at the right of the title row ("• on · picking from {server}" Green / "• off" DarkGray), then rows under dim UPPERCASE section labels QUEUE · CONTINUITY · FILTERS · SOURCES with a blank row between settings, the status as one line (`● Auto DJ is on`, its detail wrapped under it) with Start / Stop at the right (the canvas "Auto DJ room options", board E′, 2026-09-23); ↑↓ walk the rows, Esc stows the cursor, the tips line names the keys |
| The browse-root card's On/Off line and the desktop rail's TOOLS header | The nav row's `•` while armed; the TOOLS label over Auto DJ and Sonic path (the record's rail says TOOLS; its browse root groups the same feature cards after the library's) |
| Start / Stop button | The room's one primary (3-row Rounded frame) under STATUS: "Start Auto DJ ▸", or "Stop Auto DJ" in the destructive colour |
| Switches | `[✓]` / `[ ]` rows; Space and Enter toggle under the cursor, a click toggles without selecting. **2026-09-23**: the label in a column of its own and no description beside it — the help line under the body (a rule and two lines) explains the row under the pointer, else the cursor's; a value in words (the length window's) sits at the value column |
| Sliders (strictness, tolerance, songs per fetch) | **2026-09-23**: the volume widget's bars at the value column — `- ▰▰▰▰▰▱▱▱▱▱ +  55% or closer`, `± 8 BPM`, `4 songs` — a click on `-` / `+` steps (strictness by .05, tolerance and songs by 1), a click on a cell sets, ←→ step too; the raw cosine, the pool size and the wide set in the help line. (Before: the sonic room's `▓░` cells with the note beside them.) |
| The two-ended length range | Two rows — Shortest · Longest — on the same 0:00–20:00 bar in 15 s steps, the words summary ("Over 1:30", "1:30 to 8:00") on the switch's row; the unknown-length checkbox appears only once a bound is real |
| Segmented buttons (empty queue · anchor · whitelist/blacklist) | **2026-09-23**: `(•)` radios on ONE row, the chosen label BOLD — at the value column beside their sub-label when they fit the body, else on the row under it; ←→ walk them; the option's hint in the help line while it is under the pointer or the cursor. (Before: one row per option with `— hint` dim beside it.) |
| Chips (genres, keywords) | An inline comma list on the row with a hover `[x]` per item; the keyword input as a 1-row text field (the kit's), Enter adds, at most 50 |
| The genre picker sheet | A kit modal list with a filter line (the path modal's shape): "{count} selected" in the title row, Space toggles, Enter and Esc close — the App's `GenrePicker` already exists |
| The multi-server body's server choice | The room's Server row (armed, several servers saved) opens a kit modal list of every selectable saved server, peers under their parent and named through it, the DJ's own marked; Enter or a click moves the DJ (clause 41), re-selecting it is a no-op |
| The start sheet | A kit modal (the torrent chooser's shape): the title and subtitle, two option rows with their descriptions, the `[ ] Remember this` row, Esc dismisses and leaves the DJ off |
| "Let me choose" / "Choose a song" | The App's `Capture` machinery grown a `DjSeed` variant — the sonic pick's road, banner and Esc included; the pick lands, the DJ arms (or opens on the armed DJ), the user stays put |
| Snackbars / toasts | The note line; the once-per-lane budgets kept; every one also a `[dj]` log line |
| The queue badge | One dim glyph before the title on the row's first line: `∞` a DJ pick, `≈` a sonic pick (the record's ∞ is its Auto DJ mark); user rows wear none; the TUI's queue column wears the same glyph |
| The empty queue's buttons | Two text buttons under the panel's "Queue is empty" while armed: `Pick a random song ▸` (accent) · `Choose a song ▸`; the hint line above them |
| Server version gates | `/api/` now gives `server`: the record's floors apply when the version is known (hide, and say why); the flat-ping session has none and keeps every control, learning from rejections (clause 25) |
| Per-server vs session-wide settings | `[player.dj]` holds the session-wide fields; the per-library ones (sources off, rating, genre mode + genres) live on the server's own `[[server]]` entry when set and fall back to `[player.dj]`'s values otherwise — which is where the player's old globals already are, so nothing migrates for them |
| The DJ's server, apart from the session | `App.dj_server: Option<String>` — an identity (a URL, a tunnel id, a peer identity), persisted as `[player] autodj_server` and restored ARMED; `App.dj_info` holds the probe's answer for it (`ApiCmd::DjProbe` → `Event::DjProbed`); `tunnel_targets()` includes its transport while armed; every pick carries `reach(dj_server)` and the worker answers it through `client_for`, the cover-art way |
| The capability learner | On the api worker, keyed by server identity (the `Tunnels` registry's shape), shared by both shells; the App builds the full request and never sees a dropped key except in the log |
| The lane epoch | An `App` counter stamped on every `ApiCmd::AutoDj`; `Event::AutoDjPick` carries it back and a stale reply is dropped in `consume_dj` — the record's `_djSessionEpoch` |
| **The rebuilt screen (2026-11-10)** | *the rows below supersede the room's rows above* |
| One row per setting, a card of rows per group | Rows at the room's own width — 47 cells, no wider in a wider room — under DIM UPPERCASE headers MUST MATCH and PREFER; a name, its value ending two cells before the last, `▸` or `▾` on the last. The keyboard's hand is the accent slab the row's width; the pointer brightens the name and the chevron; ↑ ↓ walk Preview, Start / Stop and the rows the server can do; Enter or a click opens |
| The value's colour (secondary · primary · warning) | DIM · OK · GOLD behind `!` |
| A greyed row with a reason, the chevron's room kept | DIM whole, the reason ending at the values' column, no chevron, no click |
| Start, and the lit Stop — the Auto DJ button's on look, never red | The room's one primary, a 3-row Rounded frame the rows' width: "Start ▸" in the accent, "Stop" in the OK colour; the kit's one-row slab on the title row where the room is short (clause 54) |
| A bottom sheet: a handle, the title, the value, a switch, Clear / Done | The kit modal the mood Filters already use (library-rooms clause 32): 66 cells, the title on the top border and the row's value after it in its tone, following the draft; `[✓] On` as the first row of a sheet that has a switch; the old room's help line inside; `clear` beside `[ Done ]` only while something is set; no `[X]`. Tab and the arrows walk its stops, Space presses, Enter is Done (it adds the word on Skip words' line; it clears on `clear`), Esc is Done too |
| Sliders (the match, the tolerance, the batch) | Bars in the volume widget's grammar, a cell a step: 25 cells a song (Batch), 20 a BPM (Beat matching), 20 an artist (cooldown), 25 at two points a cell (Sonic, 30–80); `-` `+` and ← → step; a click on a cell sets |
| Timer wheels (hours · minutes · seconds) for the two bounds | A typed length per bound, read to the second, with `-` `+` stepping fifteen seconds; ← → step too, so the field types at its end |
| Segmented buttons (Only \| Skip · Generated \| Tags \| Mixed · Drift \| Stay close · Strict \| Standard \| Loose) | The kit's side-by-side radios in the sheet's columns |
| Chips with a remove (the skip words) | Words with `[X]`, three cells apart, wrapping to four rows; ↓ from the line reaches them, ← → walk, `x` removes |
| The First song sheet (four rows, a tap applies) | The kit's Dropdown under the row |
| The star row of the Rating sheet | Five stars a cell apart: a click sets one whole, a second click on it halves it; ← → by halves |
| Play from: a tick per server, folder switches under each ticked one | One sheet: the servers as one-ticked choices, a peer under its parent behind `└`; the folders of the rows' server as checkboxes under its name. A server just ticked shows no folders — they are its own to say, once it is the rows' server |
| The start sheet (Random · Choose · Recently played with its dropdown · Remember) | The kit modal under the row's own name, "First song", on its border: the three answers, a blank row, `[ ] Remember`; Recently played's list hangs from its row, twenty cells in, over the box's row and past the frame, a song a row — title, then the artist DIM |
| The empty queue's Recently played chip and its menu | A third text button, `Recently played ▾`, whose list hangs under it at the panel's width |
| The split Auto DJ button (the word · a chevron) and its quick settings sheet | One rounded frame in the bar with a rule between its halves — `│ auto-dj │ ▴ │` — the transport group moved right by the chevron's four cells; the popover 42 cells wide from the rule's column, its last row the one over the bar; `▾` while it is up. No key opens it yet (decision 18) |
| A spinner while a server connects | Nothing: a session here is one server, and its tunnel's state is the header's to show |
| Already shared | `dj.rs` (reshaped per decisions 1–3), `maybe_autodj` (grown clause 13's guards), `consume_dj` (a batch, the badge, the epoch), `autodj_sample` (Preview), the genre picker, `Reach` and `reach()` for a peer's or a tunnel server's request |

### `[player.dj]` after this contract

```toml
[player]
# was `autodj = "off" | "similar" | "tempo+key"` — a mode. Now the server
# the DJ is armed FOR (a URL, a tunnel id, a peer identity), absent when
# off; the lane's other choices are toggles below
autodj_server = "http://…"
# Play from's home (2026-11-10): where Start arms the DJ whichever server
# is browsed — the one ticked in the room, and whichever the DJ last ran on
autodj_home = "http://…"

[player.dj]
songs_per_fetch = 4          # 1–25 (clause 27)
sonic = true                 # clause 43: on by default
sonic_min_similarity = 0.55  # .30–.80, raw cosine
sonic_anchor = "rolling"     # | "locked"
empty_queue = "ask"          # | "random" | "pick" | "recent" (2026-11-10)
bpm = false
bpm_tolerance = 8            # 1–20 BPM (was `tempo_tolerance`, a percent)
harmonic = false             # was `key_matching` = off | compatible | strict
artist_cooldown = 3          # kept (decision 9); 0 is off
length = false
min_seconds = 0              # 0 = the rail
max_seconds = 86400          # the rail: a day, the server's cap (2026-11-10)
length_ceiling = 86400       # what "no maximum" is in THIS file — absent in
                             # one from before, where it was the bar's 1200
allow_unknown_length = false
mood_valence = ""            # Smart filters (2026-11-10): a value per scale,
mood_energy = "relaxed"      # "" for none; the tags; and how sure every group
mood_voice = ""              # must be — "strict" | "standard" | "loose".
mood_tags = ["acoustic"]     # Nothing chosen is the filter off
mood_level = "standard"
keyword_filter = false
keywords = []
# the per-library fallbacks — a [[server]] entry may carry its own
min_rating = 0
genre_filter = false         # the switch (2026-09-23; the record's autoDJGenreEnabled)
genre_mode = "whitelist"     # | "blacklist" — kept while the switch is off
genres = []
genre_source = "tag"         # | "model"; absent is both (2026-11-10)

[[server]]
url = "http://…"
dj_sources_off = ["Audiobooks"]   # vpaths switched OFF (clause 42)
dj_min_rating = 6                 # optional, overrides [player.dj]
dj_genre_filter = true
dj_genre_mode = "whitelist"
dj_genres = ["Jazz"]
dj_genre_source = "model"         # "tag" | "model" | "both" (2026-11-10)
```

Migration on load: `player.autodj` `"off"` → no `autodj_server`, anything
else → armed on the remembered session's server at the next launch (the
old mode never named one); `sonic_tightness` → dropped (the record's model is a switch
and a raw band; the default is on at .55); `tempo_tolerance` (a percent)
→ `bpm = tempo_tolerance > 0`, `bpm_tolerance = 8` (a percent cannot be
reinterpreted); `key_matching` → `harmonic = key_matching != "off"`
(strict is gone). `genre_mode = "off"` and an entry's `dj_genre_mode =
"off"` (files from before 2026-09-23, when the switch was a third value
of the mode) → the switch off, the mode whitelist. A `max_seconds` at or
past the file's `length_ceiling` — 1200 where the file has none — reads as
no maximum (2026-11-10: an older file's 20:00 was the end of the bar it
was set on, never a cap). Unknown keys survive through `extra` as they do
today.

### What the tests pin

App (pure, on the fake clock): the top-up rule's four guards; a new lane
from each trigger and an in-flight reply dropped by epoch; the request's
JSON for every toggle — `requireBpm` / `requireMusicalKey` present exactly
when their switches are, `limit` absent at 1 and on the opener,
`minRating` absent for a peer, the sonic pair absent when suppressed or
not ready, rails not sent; the keyword filter over a batch (partial
pass, full block, the fifth-try acceptance); the three degrade strings
once per lane; the auth note once; an owed pick on a network failure and
its retry; the migration table above. Worker: the not-allowed parser, the
retry loop's bound, the version pre-filter. GUI (render tests): the
nav row (its `•` while armed, `D`), every room row and its gate, the chooser's three answers, the
capture's banner and Esc, the empty state's buttons following the armed
state, the badge glyphs, the bar's on/off wording. Rig (the two servers,
discovery off): the DJ on a peer session through the proxy and direct (a
pick's `Origin` is the peer's), `discovery is disabled` degrading once,
the Preview row's three picks, `songs_per_fetch` landing a batch.

**The rebuilt room (2026-11-10).** `dj.rs`: a length typed and read to
the second; the window to a day and an older file's end of bar as no
maximum; `moods` on a pick and on the opener only where the server has
scores, and its words through the file; `genreSource` only beside a list
and only where the server takes it; the floor in hundredths; Recently
played as a fourth way to start. Worker: a lane taking its pool back
(`retrySonic`), Recently played's each-once-twenty, the Genres sheet's
answer. App: the toggle — Start on Play from's server, Stop always
stopping, the home remembered; each sheet's edit applied whole; the gates
from the DJ server's own answer; Recently played asked of the opener's
server and a song of it opening the session. GUI (`src/gui/dj/tests.rs`):
the room's 23 rows cell for cell and the floor at 100×24; the order rows
come back in as the height grows; values in their tones; every gate in
place, DIM, unclickable and passed by the keyboard; the dropped pool in
gold and its retry; each sheet's draft law — by the keyboard and by the
pointer — and the write it ends in; First song's list; the opening
question with Recently played, and the empty queue's third opener; Smart
filters as the mood modal retitled; the split button and quick config;
Play from's window where the servers outgrow the sheet, and the wheel
through the shell — a sheet's list turning, a Recently played list
walking (both fail with the shell's hand-over taken out); and, in all ten
locales, the floor drawn whole, every help line in its sheet, every tips
line in its row, and every reason whole. The TUI: `A` with no Play from
on screen starting the DJ where it stands, and the tab's first row saying
where Enter starts it in either shell.

## Decisions (2026-09-20)

The 09-06 open questions, settled on their leans unless noted; both
flagged ones were confirmed on 2026-09-20 (decision 10 rewritten).

1. **The request model** — the record's: on/off plus independent
   toggles, ONE random-songs request. `Similar` (the nearest-neighbour
   list that bypassed every filter) is retired; rolling sonic mode
   delivers the same promise with the filters honoured. The TUI's `A`
   becomes on/off (its help already says "auto-dj on / off") and its tab
   rows follow this contract.
2. **Tempo units** — the record's ± BPM, 1–20, default 8, wide = + 2;
   the percent pref lands on the default (a percent cannot be
   reinterpreted).
3. **Key anchoring** — the record's session Camelot anchor; strict is
   dropped.
4. **Sonic default and slider** — the record's ON by default with the raw
   .30–.80 band shown as "N% or closer" (confirmed 2026-09-20). Safe only
   with clause 30 in the same slice. The pool size and the raw cosine stay
   as dim details (the player's addition).
5. **Where the room lives** — Settings › `Auto DJ ▸` in a LISTEN group
   above SERVERS; the bar toggle stays the everyday control; the queue
   panel's empty state carries the openers. When the Now Playing screen
   lands the room may also be reachable as a tab there. **Amended
   2026-09-23**: the room is a Library nav room under a TOOLS group,
   beside the sonic room, where the record's browse root (a feature card
   with the DJ's On/Off line) and its desktop rail (TOOLS › Auto DJ) keep
   it; the Settings doorway and its LISTEN group are retired — one door.
   The nav row wears the DJ's state; `D` opens the room from the keyboard.
6. **Preview** — kept (clause 53), a logged deviation.
7. **The new parameters and the learner** — all ported: sources, the
   length window, the keyword filter, `requireBpm` / `requireMusicalKey`,
   songs per fetch, readiness; the learner on the worker.
8. **The badge** — yes: `∞` / `≈`.
9. **Artist cooldown** — kept (the record has none; the server accepts
   `ignoreArtists`, the row exists, and an artist repeating immediately
   reads as broken). Default 3, 0 is off. A logged deviation.
10. **The DJ is armed FOR a server, as the record has it** (rewritten
    2026-09-20; the first cut deferred this as "the DJ follows the
    session"). The seams it needs exist since the multi-server work: a
    per-request client from a `Reach` for any saved server, tunnel bridge
    or peer; `reach()` for all of them; the tunnel-target set the worker
    keeps up; the off-session probe of a server's `/api/`. So: a DJ server
    identity apart from the session (clause 19), the picker in the room
    (clause 41), the record's toggle semantics (entry point 1), the seed
    rule by origin (clause 23), disarm on removal. One consequence worth
    saying in the room: a DJ armed on a tunnel server holds that tunnel
    up while armed, with nothing queued from it.
11. **Multi-server sessions** — out of scope for this round (above).
12. **A peer hosts the DJ** — yes, the record's rule since #946; the
    rating row hides and `minRating` is not sent.

**2026-11-10 — the rebuilt room.** The owner reviewed the canvas "Auto DJ
Panel · GUI" and said to build it as drawn; its open questions are settled
the way the boards drew them, and stay easy to turn:

13. **Artist cooldown stays a row** under PREFER, with a sheet of its own
    (the canvas drew the row; the alternative was retiring it to its
    default). The record has no such row.
14. **Preview is a sheet** opened from the title row, not a row at the
    foot of the body.
15. **Sheets are modals** over the room. The docked alternate (the
    canvas's last board) is not built.
16. **Play from ticks one server**, and keeps a home (clause 42, entry
    point 1): the record's toggle — Stop always stops, Start goes to Play
    from's server — without the fan-out. Decision 11 stands.
17. **Recently played reads the server's play history** for the account
    (`stats/recently-played`) — the Last played room's source — where the
    record reads the phone's own history. A player has no history of its
    own, and the server's follows the account across devices.
18. **Quick config has no key yet.** The chevron is the pointer's; once
    up, the popover is the keyboard's too. A key is the owner's to name.
19. **The rating row is "Not on peers"**, in place, where clause 6 hid it
    — rows never move.

## Deviations log

- **2026-09-06 — No design canvas planned**: the room is settings rows
  and modals already drawn once (the torrent room's rows and chooser, the
  sonic room's bars, the servers room's modals). Revisit if discussion
  disagrees.
- **2026-09-20 — Re-extracted** against `439b4de4`: clause 6 reversed (a
  peer hosts the DJ), clauses 7, 16, 27, 28, 35, 36, 52, 53, 62, 63
  added, clause 14 rewritten for batches, clause 26 for a batch's
  partial pass, clauses 23 and 43 for readiness, clause 50 for `/api/`'s
  version. The multi-server body of the record's screen is out of scope.
- **2026-09-20 — Preview kept** (clause 53, decision 6): three picks
  without queueing and the pool size; the record has nothing like it.
- **2026-09-20 — Artist cooldown kept** (decision 9): `ignoreArtists`
  from the recent picks, default 3; the record dropped the webapp's.
- **2026-09-20 — Decision 10 rewritten** to the record's model the same
  day it was made: the DJ is armed for a server and the session browses
  freely (clauses 19, 41, entry point 1). The "follows the session"
  deviation never shipped.
- **2026-09-20 — Touch words become terminal words** (⚑ in the wording
  table): "Tap below" → "Start below", "tap a track" → "choose a track",
  "Tap \"Pick genres\"" → "Choose \"Pick genres\"", "Re-login in Manage
  Servers" → "Sign in again in Manage servers" (the room's own name
  here), and the confirmations read "Auto DJ on" / "Auto DJ off" — the
  note line's register, not a snackbar's title case.
- **2026-09-20 — The TUI follows**: `A` is on/off; the Auto-DJ tab's rows
  become this contract's (the Mode row goes, Sonic becomes a switch and a
  band, Tempo reads in BPM, Key becomes Harmonic, Genres and Preview stay,
  Sources and Length and Keywords arrive). The TUI is not the target
  surface; it shares the App, so it cannot be left on the old model. Its
  Keywords row types a word on Enter (2026-09-21): Enter adds it and
  clears for the next, Esc or Enter on nothing leaves, `x` on the row
  takes the last word back.
- **2026-09-20 — The badge glyphs**: `∞` and `≈`, dim, before the title;
  the record's icons are its own.
- **2026-09-20 — Slices A1 and A2 landed** in the shared App and the api
  worker: the record's toggle, the lane and its epoch, the DJ's own server
  with its probe and tunnel target, the composition of clauses 20–27, the
  learner and the failure taxonomy, the openers and the chooser, batches
  and the badge field, the migration. The TUI's tab carries the new rows;
  its keyword entry waits on the GUI room (the tab says so). The room, the
  bar's wording, the queue's badge glyph and empty state, and the locales
  are slice A3; the rig is A5.
- **2026-09-21 — Slice A3 landed** in the GUI shell: the chooser, the
  banner, the openers and the badge (part 1), then the Settings doorway
  and the room (part 2) — every control an edit through one `DjEdit` on
  the App, so the TUI's tab and the room change one model. Per-library
  rules (rating, genre mode and genres) are written to the **target
  server's own entry**: the DJ's server while armed, the session's while
  off (the one Start would use); a server with no saved entry falls back
  to `[player.dj]`. The room probes the session's server when opened with
  the DJ off, so the version gates and the sonic reasons apply before
  Start.
- **2026-09-21 — The sonic switch stays live under its reason** (clause 43
  says disabled): the reason shows in gold where the description would be
  and as the row's tip, and the switch still toggles — sonic is on by
  default, and a switch that could not be switched off would pin the
  strictness and anchor rows on screen for a server that cannot use them.
- **2026-09-21 — Bars set by cell**: a click on one of the ten cells sets
  one of ten values across the bar's range (a length bar snaps to 15 s);
  ←→ reach every step. The record's sliders are continuous.
- **2026-09-21 — Chips on the keyboard**: ←→ walk a chips row, `x` (or
  Delete) removes the chip under the cursor, Enter opens the genre picker
  or focuses the keyword field; the record removes by each chip's ×.
- **2026-09-21 — Sources inline**: a `[✓]` row per library under SOURCES
  rather than a picker; the TUI keeps its picker. The refusal for the last
  source is the App's note (clause 42).
- **2026-09-21 — Verified on the rig** (Rig A :3040 and Rig B :3041,
  mStream 6.29.0, no discovery data): the room opens off with "This server
  doesn't have discovery data — picks stay random." on the sonic row from
  the opening probe; Start on a one-row queue lands a batch of four `∞`
  rows and the note names the server ("Auto DJ on — picking from …", four
  servers saved); Preview lists three picks under its row and queues none;
  a click on the songs-per-fetch bar's first cell reads "1 song" at once;
  the Server row's picker lists B, Rig A via B, B's Quick Connect tunnel
  and Rig A via it; choosing Rig A via B moves the DJ ("picking from Rig
  A", a fresh probe) and the next turn's one pick comes from Rig A through
  the proxy; Stop leaves the rows and their badges. The sonic path (a pool
  answered, clause 30) and a direct-tunnel peer lane still want a server
  with discovery data.
- **2026-09-21 — The App's notes speak the ten locales**: every note the
  shared App says for the DJ is a `dj.*` key (the record's translations
  where it has them, the terminal's own for the rest), and the worker no
  longer words anything — a pick or a preview carries a `DjNote` kind
  (the range or the unscanned degrade, a preview's failure) that the App
  puts into words. The TUI's tab rows keep their own English, as the rest
  of that surface does.
- **2026-09-21 — "Could not load genres" is a state**: a failed genres
  fetch used to leave the picker on "loading genres…" until Esc; the
  worker now answers with a failure of its own (a dead session still
  travels as the session's), the picker shows the record's sentence with
  the reason under it, and reopening asks again clean.
- **2026-09-21 — The room's Stop stops** (clause 40 says the button rides
  the same toggle as every entry point): Start does — the opening question
  and the seed rule included — but the toggle's "on elsewhere → move here"
  is the bar's rule, and the rig showed Stop pressed with the DJ armed on
  Rig A while the session browsed B *moving* the DJ to B. So the button
  disarms wherever the DJ is armed; only Start goes through the toggle.
- **2026-09-21 — Preview's picks scroll into view**: they land after the
  click under the body's last row, so while the cursor is on Preview the
  room reveals them as they arrive.
- **2026-09-21 — Long sentences wrap**: the status detail, the gate
  sentence and the hints wrap to the room's width; a row's description
  clips at the cell edge (the Settings rows' rule) and comes back whole
  when the queue panel folds away; a bar's note shows whole or not at all
  (a cosine clipped mid-number would read as another number).
- **2026-09-23 — The room moves to the Library nav** (decision 5
  amended): the record puts Auto DJ among the browse root's feature cards
  and in the desktop rail's TOOLS group, never under Settings; the
  Settings › `Auto DJ ▸` doorway was this surface's own translation. It
  and its LISTEN group are retired, the room's `◂` back with them (a nav
  room has no way back; Esc stows the cursor). The nav row wears the DJ's
  state as the record's card does — a `•` in the ok colour while armed.
  The tenth room has no digit (`0` is the Now Playing screen); `D` opens
  it, the capital pairing with `A`'s toggle.
- **2026-09-23 — The genre switch and its mode become two fields**: the
  first cut folded the record's `autoDJGenreEnabled` and
  `autoDJGenreMode` into one three-way `genre_mode` (off · whitelist ·
  blacklist), and the room's switch STEPPED it — so a click on a checked
  whitelist landed on blacklist and a second click was needed to switch
  off (found in use). Now `genre_filter` is the switch and `genre_mode`
  the two-way mode, in `[player.dj]` and on a server entry alike; the
  switch never touches the mode (blacklist survives a switch off) and the
  radios never touch the switch. Old "off" values read as the switch off.
  The TUI's single Genre filter row keeps its ←→ walk off → whitelist →
  blacklist → off as a keyboard idiom (`DjEdit::GenreCycle`), and its
  picker now shows the DJ's library's own genres rather than the
  session-wide fallback.
- **2026-09-23 — The room re-laid as its canvas's E′** (a canvas after all,
  superseding the 2026-09-06 note that none was planned: "Auto DJ room
  options", seven boards, E chosen with two changes): every control at
  one value column, no description beside a row — the help line under the
  body explains the row under the pointer, else the cursor's — a blank row
  between settings, the choices `(•)` side by side on one row, the bars in
  the volume widget's `- ▰▰▱ +` grammar with a step at each end, the
  status as one line with Start / Stop at its right. Clauses 40–53 stand;
  only the room's grammar moved.
- **2026-09-26 — The review's fixes.** The top-up (clause 13) looked at
  the queue's end only when a row started, while the status was still
  idle: a last row started from a stopped player never got its turn and
  the music stopped with the DJ on. It looks again on the play's first
  status, and the queue's end with the DJ armed asks for the turn that
  never came or was owed (clauses 32–33 as written). The genre picker
  lists the genres of the library the rule is for — the DJ's server —
  not the browsed one (clauses 48, 51). A late opener joins a queue built
  meanwhile at its end instead of wiping it (clause 5's "the queue becomes
  the seed" is the empty case); arming clears a dropped opener's target.
  The capability learner retries only a key the body carried (clause 25's
  "terminates by construction" did not hold for a refusal it could not
  mend). The room's radios stack one per row rather than drop options
  that do not fit the column (the default layout lost one). The sources
  rule reads and writes the library the rules are for (clause 42), the
  room forgets the previous server's capabilities on a new connection
  while off (clause 50), the toggle with no session switches an armed DJ
  off, and no probe goes out before a session exists. Left for later:
  keys below a known version floor are still sent and left to the learner
  (clauses 25, 27); the sonic badge follows the ask, not the body that
  went out (clause 60); Preview's degrade suppresses the lane's sonic
  pool; the migration reads the old panel's defaults as a wish for BPM
  and harmonic continuity; the Esc wording on the chooser is English.
- **2026-09-26 — The follow-ups.** The ask carries the probed version and
  leaves out what the server is known to predate (clauses 25, 27, 50)
  rather than sending the key and learning it back. The queue's badge
  reads the body that went out, not the ask (clause 60). A Preview runs
  in a lane of its own, so its degrade never lets go of the running
  lane's pool (clause 53). The migration from the three-mode panel
  switches BPM and harmonic continuity on only when the old mode was
  tempo+key — the panel wrote its defaults on every quit. The picker the
  GUI lends from the TUI view and the TUI's chooser draw their key hints
  through the locale table (the TUI boots no language and stays English).
  The Auto-DJ tab's keyword row names the key the keymap binds to
  remove-from-queue instead of an unbound `x`.
- **2026-09-28 — Played picks are let go at a restore** (clause 14;
  multi-server clause 40; performance audit #106). The record never
  trims: a lean-back DJ queue grew ~15 rows an hour, and a resumed queue
  brought every one back at each launch, with every per-row cost the
  player pays growing alike. A restore with the DJ armed now keeps the
  last 100 played DJ rows (straight play only — a shuffled or wrapping
  queue still has them to come) and remembers the rest by path, 500 at
  most, so they are still not picked again. Nothing is trimmed while the
  app runs: the rows the queue panel is scrolled to, dragged or opened on
  never move under it.
- **2026-10-05 — TOOLS holds three rooms** (entry point 2; the
  translation table's "Auto DJ, then Sonic path"): the Discover room joins
  the group under Sonic path (docs/ux-contracts/discovery-page.md, clause
  2), and the group moves up a row to fit it — the TOOLS label on row 13,
  **Auto DJ on 14**, Sonic path 15, Discover 16, Settings still on the
  content's last row. The row keeps its `•` while armed and its `D`.
  Entry point 4 is unchanged: "Start a sonic session" belongs to the
  record's per-song Discover screen, and the GUI's Discover room is the
  history-seeded page, which does not carry it.
- **2026-11-09 — TOOLS holds five rooms** (entry point 2): Add torrent and
  Add YouTube leave Settings and join the group after Discover, nav rooms
  in this room's shape — no `◂`, Esc letting go of whatever holds the
  keys and then leaving nothing, never to Settings (add-torrent.md and
  add-from-youtube.md, entry point 1). Auto DJ stays on row 15; where
  Settings would meet the block it rises a line at a time
  (discovery-page clause 2) — with all five tools to 14, then 13, then 12
  at 100×24 with the footer on — and the TOOLS label gives way where
  Settings is on 18 or 17 (100×24, and 100×25 with the footer on). The
  group packs the rooms it shows (the owner's call, the same day), so the
  rise comes only where the last tool shown would reach Settings. The row
  keeps its `•` and its `D`.
- **2026-11-10 — The room rebuilt** (re-extracted against `1d4e8974`;
  the canvas "Auto DJ Panel · GUI", twenty boards, built as drawn — the
  owner's call the same day). The record's screen became one row per
  setting with a sheet behind each, and the room follows: clauses 40–53
  rewritten, 54–58 added, clauses 2, 8, 16 and 20 with them; the E′
  layout of 2026-09-23 goes — its help line under the body, its bars and
  chips beside the rows, its status sentence. The room's numbers kept
  their subjects but one: the server picker that was clause 41 is clause
  42's sheet, and 41 is the row's own grammar — an entry from before
  today that cites clause 41 means the picker. What was this surface's own
  and is gone with it: **the sonic switch stays live under its reason**
  (2026-09-21) — the row is DIM under its reason now and takes no click;
  the strictness and anchor rows a dead switch would have pinned on screen
  live inside its sheet. **The room's Stop stops** (2026-09-21) — it is
  the toggle's own rule now, at every entry point (clause 2). **Sources
  inline** — they are Play from's sheet. **Bars set by cell** — a cell is
  a step (Sonic's is two points; ← → still reach every point). **Chips on
  the keyboard** — the same keys, inside the Skip words sheet.
- **2026-11-10 — What the rebuild decided for itself** (decisions 13–19):
  Play from ticks one server where the record ticks several; Recently
  played reads the server's history where the record reads the phone's;
  Artist cooldown and Preview stay, as a row and as the title row's
  sheet; sheets are modals; quick config is the pointer's to open. And
  three the canvas did not draw: **a server just ticked in Play from
  shows no folders** until it is the rows' server (its folders are its
  own to say — the sheet asks nobody on the way); **a gated row gives its
  name's cells to the reason** where a locale's reason is longer than the
  value column ("Pas de données d'ambiance"), the reason always whole;
  and **Start leaves the rows as they stand** — the capabilities already
  probed for the server Start arms are kept, so no row blinks to a gate
  and back while the armed probe answers.
- **2026-11-10 — The wheel reaches the DJ's lists.** A modal swallows the
  wheel in the shell, and the DJ's layers are modals: the old genre
  picker's list never turned under it, and the first cut of the sheets
  inherited that. The shell now hands a wheel over any of the DJ's layers
  to it — the Genres list and Play from's lines scroll, and a Recently
  played list walks (clauses 8, 42, 48).
- **2026-11-10 — Verified on the rig** (Rig A :3040 and Rig B :3041,
  mStream 6.32.3, no discovery data and no mood scores; the GUI on a
  120×34 pty, `smoke/gui/scenario_dj.py`, exit 0 and no `[dj]` log line):
  the room opens off with `Discovery off` and `No mood data` in place
  from the opening probe, and a greyed row takes no click; the seed
  config's `max_seconds = 1200` with no ceiling of its own reads `Any`;
  Start on an empty queue asks for a First song, Recently played's list
  hangs from its row with B's own two songs, and one of them opens the
  session — armed, a batch of four `∞` rows after it ("5 tracks");
  Preview's sheet lists three picks and queues none; Batch's border reads
  "1 song" at a click on its bar's first cell while the row waits for
  Done; a typed `1:30` reads "1:30+" and `clear` takes it back to `Any`;
  the Genres sheet lists B's own genres, one ticked and cleared, and its
  list turns under the wheel; First song's list applies on a pick; quick
  config rises from the bar's chevron — four servers inline, the two
  reasons — and Esc puts it away; Play from's sheet moves the DJ to Rig A
  through B ("Auto DJ on — picking from Rig A"), the rating and Smart
  filters rows saying `Not on peers`, and the next turn's one pick comes
  from there ("6 tracks"); Stop leaves the rows and their badges; the
  bar's word, B still browsed, starts the DJ on the home (Rig A) and
  stops it. At 100×24 on the same pty: every row whole with Start as the
  slab on the title row and no hint line, Length's and Genres' sheets
  inside the screen, quick config's last row the one over the bar. And
  read-only against demo.mstream.io (6.33.1: discovery ready, mood
  scores, both kinds of genre), nothing started there: Sonic similarity
  `55%` and its sheet, Smart filters as the mood modal under the row's
  name, the Genres sheet with Generated · Tags · Mixed, Preview's three
  picks, and quick config's sonic row with Drift · Stay close under it.
  **Not seen live**: a running sonic session (a pool answered, the `≈`
  badge, the `!` drop and its retry), a mood narrowing a pick, Play
  from's folders on a server with several libraries, and a Windows
  console — each is pinned by the tests above, none by a run.
- **2026-11-10 — The request grew** (clause 20): `moods` goes with a pick
  and with the opener where the DJ's server said it has scores, and
  `genreSource` beside a genre list from 6.32.2; a sonic setting changed
  mid-lane gives the lane its pool back (`retrySonic`, clause 43). The
  length window reaches a day; a file written before today keeps its
  20:00 as "no maximum" (`length_ceiling`).
- **2026-11-10 — The TUI follows where the model forces it**: its tab
  keeps its rows. With no Play from among them, `A` starts the DJ where
  the TUI stands (entry point 1) — the home is the GUI's to follow, the
  TUI only writes it — and `A` on a DJ armed elsewhere stops it rather
  than moving it: off, then on, moves it. Longest still walks to 20:00 and one step more is "no
  maximum" (a length typed in the room between the two steps as any
  other); Empty queue names "Recently played" when the room chose it, its
  walk stays Ask → Random → Pick, and its chooser asks; Smart filters and
  the genre kind have no row there — what the room set goes with the
  TUI's picks too.
