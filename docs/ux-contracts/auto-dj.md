# Auto DJ

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`439b4de4`, 2026-09-19) — `lib/screens/auto_dj.dart` (the settings screen: status, Queue, Continuity, Filters, Sources, and the multi-server body this surface does not port), `lib/widgets/auto_dj_start_sheet.dart` (the empty-queue chooser), `lib/widgets/queue_list.dart` (`toggleAutoDJ`, `_seedRandom` / `_armLibraryPick`, the `_EmptyQueue` opening-song buttons, the labelled `AutoDjButton`, the queue-row badge), `lib/widgets/player_panel.dart` (the same button on the player), `lib/singletons/auto_dj_manager.dart` (the settings, their persistence, `libraryFilters`, `batchParams`), `lib/media/audio_stuff.dart` (the session: `setAutoDJ`, `startAutoDJFromSeed`, `_startAutoDJFromSeed`, `_autoDJPick`, `sonicParams`, `shouldTopUpAutoDJ`, `shouldDeferDJPick`, `_resetAutoDJSession`, `_queueAutoDJSongs`, `restoreAutoDJ`), `lib/singletons/server_capabilities.dart` + `lib/util/server_version.dart` (the capability learner and the version floors), `lib/singletons/api.dart` (`fetchAutoDjSeed`, the opener). **Re-extracted 2026-09-20** from the 2026-09-06 pin (`695f4d0`); what moved in between: songs per fetch (`limit`, mStream #966), a federated peer hosts the DJ (mStream #946), sonic waits for the discovery scan (`discoveryReady`, mStream #879), opening-song buttons under an empty armed queue, multi-server sessions with vector seeds (out of scope here), and the toasts name the server by its display name. The screen is itself a port of the webapp's panel (`webapp/velvet/app.js viewAutoDJ`; `webapp/alpha/auto-dj.js` for the anchor semantics) — where they disagree, this contract says so. |
| **Server API** | `POST /api/v1/db/random-songs` — one call, every knob (mStream `src/api/random.js`, Joi-validated with no unknown keys): `limit` (1–25, default 1; 6.26.0) · `ignoreList` (round-trip cursor, ≤ 500 ids; the server keeps the last 50) · `ignoreVPaths` (≤ 50) · `minRating` (0–10; ignored for a caller with no user — a federation key or guest) · `genres` (≤ 200) + `genreMode` (`whitelist` default / `blacklist`) · `bpmRanges` + `bpmRangesWide` (≤ 16 windows each, 0–1000) + `requireBpm` · `musicalKeys` (≤ 24 Camelot codes) + `requireMusicalKey` · `ignoreArtists` (≤ 100) · `similarTo` (1–8 paths) + `minSimilarity` (both or neither) · `similarToVector` + `similarToModelId` (6.26.0, out of scope) · `minDuration` + `maxDuration` (seconds, ≤ 86400) + `allowUnknownDuration` (6.25.0). Answers `{songs: [..], ignoreList, sonic?: {similarity, similarities, poolSize}}`. Its refusals, all as `{"error": "..."}`: a **schema rejection** `"<key>" is not allowed` (400 from 6.12.0, 403 before — the body is the signal); nothing left in a sonic pool → 400 `No songs within the similarity range match criteria`; an unanalysed seed → 400 `Sonic seed track has not been analyzed yet`, an unscanned library → 400 `No tracks have been analyzed yet`; discovery switched off → 403 containing `discovery is disabled`; an expired token → 401/403 without a not-allowed body. `GET /api/v1/db/genres`. `GET /api/` is the capability source: `server` (the version), `features.discovery`, `features.discoveryReady` (whether the scan has produced vectors — `/api/v1/ping` never carries it), `user.vpaths`. Version floors the record keeps: 4.6.0 `ignoreVPaths` · 6.7.1 the BPM / key / genre / cooldown block · 6.15.2 the sonic pair · 6.25.0 the length window · 6.26.0 `limit`. random-songs is on the federation allowlist (mStream #946): a peer session — through the parent's proxy or over the peer's own tunnel with a guest token — can run the DJ. |
| **Already in this repo** | Most of a DJ, in the older three-mode shape: `src/dj.rs` (Camelot math, same/half/double windows, the perceptual sonic slider, `build_random_request`, `Settings`), `[player.dj]` prefs and `player.autodj`, `AutoDjMode` and the `A` cycle, `maybe_autodj` (the queue-end top-up), `consume_dj` (queue the pick, start it if idle, the cursor), `autodj_pick` (`Similar` via nearest neighbours, `BpmKey` via random-songs, the sonic 400 retried once without the pool), `autodj_sample` (three picks without queueing), the TUI's Auto-DJ tab (`DjPanel` / `DjRow`) with its genre picker, `Event::{AutoDjPick, AutoDjSample, Genres}`, and the GUI bar's `auto-dj` toggle. Since 2026-09-06 the multi-server contract landed and gives this one its footing: every queued row carries its `Origin` and plays from its own server (`Reach`, `reach()`), federated peers are sessions of their own (proxied or direct), tunnels follow the queue with a hold for a row whose tunnel is down, the layered `/api/` payload is parsed (`LayeredInfo`), and the GUI queue panel has rows of its own to badge. **Missing**: the on/off-plus-toggles model, sources (`ignoreVPaths`), the length window, the keyword filter, `requireBpm` / `requireMusicalKey`, the session-locked Camelot anchor, the rolling / locked sonic anchors as the record defines them, songs per fetch, the readiness gate, the one-shot seed and the start chooser, the empty-queue openers, lane resets with in-flight discards, the capability learner, the failure taxonomy, the queue badge, the armed-not-playing restore, and the room itself. |
| **Target surface** | the GUI player — the bar toggle, the queue panel (its badge and its empty state), and an **Auto DJ room** under Settings; the TUI's `A` and Auto-DJ tab follow through the shared App |
| **Status** | contract extracted 2026-09-06; **re-extracted against the moved record and settled 2026-09-20** — the open questions are decided below (two flagged for confirmation) and implementation is next (PLAN.md, Phase 10, slices A1–A5) |

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

1. **The toggle** — the bar's `auto-dj` control, lit while the DJ runs,
   and `A` in both shells. It toggles for the session's server; the note
   line confirms "Auto DJ on" / "Auto DJ off" ("Auto DJ on — picking from
   {server}" when more than one server is saved).
2. **The room** — Settings › `Auto DJ ▸` (a LISTEN group above SERVERS):
   the state and Start/Stop at the top, then Queue · Continuity ·
   Filters · Sources.
3. **The queue panel's empty state** while the DJ is armed: "Auto DJ is on
   and needs an opening song." with `Pick a random song` and `Choose a
   song` (clauses 16, 62).
4. **Discover › "Start a sonic session"** — clears the lane and the
   queue, seeds the DJ with the track (or a random one), switches it on.
   *Out of scope here (the Discover slice), named because it reuses this
   feature's seed and lane.*
5. **Launch** — a DJ remembered as on comes back **armed, never playing**.
6. Android Auto / CarPlay "Shuffle All" — *out of scope.*

## States & flows

Off → switched on → (empty queue only) the **opening question** — ask /
surprise me / let me choose, the answer rememberable → **armed**, with a
session **lane**: the ignore cursor, the Camelot anchor, the sonic history
and pin, the once-per-lane warning budgets, the suppressed sonic keys.
Every way the user steers somewhere new — switching on, switching the
session's server (which moves the DJ, clause 41's deviation), clearing the
queue, removing the server — starts a **new lane**; re-arming on the same
server does not. Off ends the lane. Settings edits apply to the running
lane at once. A lane picks from ONE server: the session's.

## Behavior contract

### Switching on

1. With a queue the DJ **follows what is there** — nothing is replaced. On
   the last row and idle, a fetch is made and its first song played at
   once; on the last row and playing, a fetch lands as the next rows;
   anywhere else nothing happens until the queue's end.
2. With an **empty queue** the DJ needs an opening track, and the **On an
   empty queue** setting decides: *ask* (default) shows the chooser —
   "Start Auto DJ with what?" · **Surprise me** · **Let me choose** ·
   **Remember this** — the other two answers skip it. Dismissing the
   chooser leaves the DJ off.
3. **Surprise me** fetches the opener with the **same library filters
   every later pick obeys** (sources, rating, genre, length) and nothing
   else — never the plain random pick, never `limit`, no continuity keys
   — plays it, then fetches followers (clause 7). Nothing matched (a 400
   under constraints) → "No songs match your Auto DJ filters — try
   loosening them"; any other failure → "Couldn't fetch a song from the
   server." The DJ stays off.
4. **Let me choose** hands off to the library under the banner "Pick the
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

### The session lane

10. A **new lane** resets the ignore cursor, the Camelot anchor, the sonic
    history and the locked pin, the warning budgets, and the sonic keys
    suppressed by clause 30. Triggers: switching on, the session moving to
    another server, clearing the queue, a server removal that empties the
    queue. Re-arming on the same server is not a new lane.
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
    trigger re-fires once the in-flight one lands.
15. Every DJ pick joins the **rolling sonic history** — kept even while
    sonic mode is off, so switching it on mid-session already has the
    session's recent sound to anchor on.
16. **The empty queue, armed**: the panel's empty state offers `Pick a
    random song` (the filtered opener, clause 3, then followers) and
    `Choose a song` (clause 4); either opens a seed on the ALREADY armed
    DJ without a new lane (re-arming the same server is not one). With the
    DJ off the empty state is just its label. A seed that cannot open is
    cleared, not kept.

### The request

20. **The library filters**, shared by every pick and the opener:
    `ignoreVPaths` (the sources switched OFF), `minRating` when set and
    the server is not a peer, `genres` + `genreMode` when the genre filter
    is on and non-empty, `minDuration` / `maxDuration` when the length
    window is on and a bound is off its rail (a rail means unbounded and
    is not sent), `allowUnknownDuration` only alongside a real bound.
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
    .30–.80, default .55) only when the session's server reports
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
    known to predate 6.26.0 never sees it and its row is hidden (clause
    50); the learner covers the rest.
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

40. **Status**: the state line "• on · picking from {server}" or "• off",
    then "Auto DJ is on" · "Songs are picked from {server} when the queue
    runs low." or "Auto DJ is off" · "Start below. The session's server's
    library will be used." — and one button, **Start Auto DJ** / **Stop
    Auto DJ** (the destructive colour when stopping), through the same
    toggle every entry point uses (a direct arm skipped the opening
    question once and replayed a stale seed).
41. **Server**: the record offers a picker for the DJ's server while on;
    *here the DJ follows the session* (deviations log), so the row is not
    drawn and a switch in the header moves the DJ.
42. **Sources** (more than one library, while on): a switch per library;
    switching the last one off is refused — "At least one source is
    required."
43. **Continuity › Sonic similarity**: switch, **ON by default** (it is
    what makes a DJ more than shuffle; safe only because clause 30
    degrades instead of stopping). Disabled with its reason when the
    server has no discovery data ("This server doesn't have discovery data
    — picks stay random."), is not ready (clause 36's sentence), or is
    older than 6.15.2 ("Needs server 6.15.2 or newer"). While on: **Match
    strictness** "{pct}% or closer" (.30–.80) with the pool size the last
    answer reported and the raw cosine as a dim detail; **On an empty
    queue** — Ask · Surprise me · Let me choose; **Anchor** — Follow the
    vibe · Stay on seed, with the mode's hint. Switching sonic OFF clears
    the lane's anchors; the explicit seed survives.
44. **BPM continuity**: switch, then **Tolerance** "± {bpm} BPM" (1–20).
45. **Harmonic mixing**: switch.
46. **Filters › Minimum rating**: Any, 0.5 ★ … 5.0 ★ (1–10) — "Only pick
    songs at or above this rating." Hidden for a peer session (clause 6).
47. **Track length**: switch, then **Length** as a two-ended range
    0:00–20:00 in 15-second steps, read back in words — "Any length" ·
    "Over {min}" · "Under {max}" · "{min} to {max}" (the rails mean
    unbounded, so a bare 0:00–20:00 must never look like a constraint) —
    and, only once a bound is real, **Include tracks of unknown length**
    (off by default: an unknown length cannot satisfy a bound, and a
    half-scanned library shrinks more than intended).
48. **Genre filter**: switch, **Whitelist / Blacklist** ("Whitelist plays
    only matching tracks; blacklist skips them." — a whitelist blocks
    untagged tracks, a blacklist lets them through), the chosen genres as
    chips with a remove, **Pick genres** → a picker with a search line,
    "{count} selected", checkboxes toggling live, at most 200; its empty
    states "No genres found on this server." · "No genres match
    "{query}"." · "Could not load genres".
49. **Keyword filter**: switch, chips with a remove, an input (hint: e.g.
    "live" or "remix") plus Add; at most 50; "No keywords. Add words below
    to start filtering."
50. **Version gates**: a server KNOWN to predate 6.7.1 hides BPM, harmonic
    and genre and says "BPM continuity, harmonic mixing and the genre
    filter need a newer server. Update to get them."; one known to predate
    6.25.0 hides the length window; one known to predate 6.26.0 hides
    songs per fetch; the keyword filter always shows (client-side). The
    version is `/api/`'s `server`; a server that answers only the flat
    ping has no version, and an unknown version or a fork keeps every
    control and learns from a rejection (clause 25) instead.
51. **Edits apply to the running lane at once** — no restart; a bar
    commits on release. Per-library settings (sources, rating, genre)
    belong to the server they were set against; session-wide ones (songs
    per fetch, length, keyword, BPM, harmonic, sonic and its anchor, the
    empty-queue answer) travel with the user.
52. **Queue › Songs per fetch**: "{n} songs" (1–25, "1 song") — "How many
    songs Auto DJ queues each time it runs. Continuity filters judge the
    whole batch against the song playing when it was fetched."
53. **Preview** *(a deviation kept from the player's own panel)*: `Preview
    picks ▸` fetches three picks with the current settings without
    queueing any, feeding each into the next call's cooldown so the three
    differ, and lists them under the row with the pool size ("{n} tracks
    in the sonic pool") — how a setting gets judged before it is committed
    to.

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
| Auto DJ: nothing is within the similarity range, so it is playing without that filter. Loosen the match slider to use it again. · Auto DJ: the discovery scan hasn't reached these tracks yet, so it is playing without sonic similarity. · Auto DJ stopped — the server session expired. Sign in again in Manage servers. ⚑ | (the record's literals in its audio handler — keys here: `dj.sonic_range`, `dj.sonic_unscanned`, `dj.auth_expired`) |
| Preview picks · {n} tracks in the sonic pool · picking… ⚑ (this surface only) | `gui.dj.preview`, `gui.dj.pool`, `gui.dj.picking` |
| Keep the music going when the queue runs low. ⚑ (the Settings doorway's description) | `gui.set.dj_desc` |

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
- **The DJ server picker** (clause 41): one live session; a switch moves
  the DJ and starts a new lane.
- **Discover's "Start a sonic session"**: the Discover slice, which will
  reuse the seed and the lane.
- **Android Auto / CarPlay**, and the cast/reseed paths of the record's
  handler.
- **The webapp's extras** the record dropped: Last.fm similar-artists
  mode, the crossfade row, the dice.

## Translation notes (terminal GUI) — settled 2026-09-20

| Record | Here |
|---|---|
| The labelled Auto DJ button (queue header + player) | The bar's `auto-dj` toggle, lit while on — plain on/off (decision 1); `A` in both shells; the confirmation on the note line |
| The settings screen | The **Auto DJ room** behind Settings › `Auto DJ ▸` (a LISTEN group above SERVERS, description "Keep the music going when the queue runs low."): the torrent room's grammar — `◂` back, BOLD title, the state line at the right of the title row ("• on · picking from {server}" Green / "• off" DarkGray), then 1-row rows under dim UPPERCASE section labels STATUS · QUEUE · CONTINUITY · FILTERS · SOURCES; ↑↓ walk the rows, Esc stows the cursor then leaves, the tips line names the keys |
| Start / Stop button | The room's one primary (3-row Rounded frame) under STATUS: "Start Auto DJ ▸", or "Stop Auto DJ" in the destructive colour |
| Switches | `[✓]` / `[ ]` rows; Space and Enter toggle under the cursor, a click toggles without selecting |
| Sliders (strictness, tolerance, songs per fetch) | The sonic room's ten-cell bars with the value beside them (`▓▓▓▓░░░░░░  55% or closer`, `± 8 BPM`, `4 songs`); ←→ step (strictness by .05, tolerance and songs by 1), a click on a cell sets; the raw cosine as a dim trailing detail on the strictness row |
| The two-ended length range | Two rows — Shortest · Longest — on the same 0:00–20:00 bar in 15 s steps, the words summary ("Over 1:30", "1:30 to 8:00") on the switch's row; the unknown-length checkbox appears only once a bound is real |
| Segmented buttons (empty queue · anchor · whitelist/blacklist) | `(•)` radio rows, one per option, chosen label BOLD, `— hint` dim |
| Chips (genres, keywords) | An inline comma list on the row with a hover `[x]` per item; the keyword input as a 1-row text field (the kit's), Enter adds, at most 50 |
| The genre picker sheet | A kit modal list with a filter line (the path modal's shape): "{count} selected" in the title row, Space toggles, Enter and Esc close — the App's `GenrePicker` already exists |
| The start sheet | A kit modal (the torrent chooser's shape): the title and subtitle, two option rows with their descriptions, the `[ ] Remember this` row, Esc dismisses and leaves the DJ off |
| "Let me choose" / "Choose a song" | The App's `Capture` machinery grown a `DjSeed` variant — the sonic pick's road, banner and Esc included; the pick lands, the DJ arms (or opens on the armed DJ), the user stays put |
| Snackbars / toasts | The note line; the once-per-lane budgets kept; every one also a `[dj]` log line |
| The queue badge | One dim glyph before the title on the row's first line: `∞` a DJ pick, `≈` a sonic pick (the record's ∞ is its Auto DJ mark); user rows wear none; the TUI's queue column wears the same glyph |
| The empty queue's buttons | Two text buttons under the panel's "Queue is empty" while armed: `Pick a random song ▸` (accent) · `Choose a song ▸`; the hint line above them |
| Server version gates | `/api/` now gives `server`: the record's floors apply when the version is known (hide, and say why); the flat-ping session has none and keeps every control, learning from rejections (clause 25) |
| Per-server vs session-wide settings | `[player.dj]` holds the session-wide fields; the per-library ones (sources off, rating, genre mode + genres) live on the server's own `[[server]]` entry when set and fall back to `[player.dj]`'s values otherwise — which is where the player's old globals already are, so nothing migrates for them |
| The capability learner | On the api worker, keyed by server identity (the `Tunnels` registry's shape), shared by both shells; the App builds the full request and never sees a dropped key except in the log |
| The lane epoch | An `App` counter stamped on every `ApiCmd::AutoDj`; `Event::AutoDjPick` carries it back and a stale reply is dropped in `consume_dj` — the record's `_djSessionEpoch` |
| Already shared | `dj.rs` (reshaped per decisions 1–3), `maybe_autodj` (grown clause 13's guards), `consume_dj` (a batch, the badge, the epoch), `autodj_sample` (Preview), the genre picker, `Reach` and `reach()` for a peer's or a tunnel server's request |

### `[player.dj]` after this contract

```toml
[player]
# was `autodj = "off" | "similar" | "tempo+key"` — a mode; now a switch,
# and the lane's other choices are toggles below
autodj = false

[player.dj]
songs_per_fetch = 4          # 1–25 (clause 27)
sonic = true                 # clause 43: on by default
sonic_min_similarity = 0.55  # .30–.80, raw cosine
sonic_anchor = "rolling"     # | "locked"
empty_queue = "ask"          # | "random" | "pick"
bpm = false
bpm_tolerance = 8            # 1–20 BPM (was `tempo_tolerance`, a percent)
harmonic = false             # was `key_matching` = off | compatible | strict
artist_cooldown = 3          # kept (decision 9); 0 is off
length = false
min_seconds = 0              # 0 = the rail
max_seconds = 1200           # 1200 = the rail
allow_unknown_length = false
keyword_filter = false
keywords = []
# the per-library fallbacks — a [[server]] entry may carry its own
min_rating = 0
genre_mode = "off"           # | "whitelist" | "blacklist"
genres = []

[[server]]
url = "http://…"
dj_sources_off = ["Audiobooks"]   # vpaths switched OFF (clause 42)
dj_min_rating = 6                 # optional, overrides [player.dj]
dj_genre_mode = "whitelist"
dj_genres = ["Jazz"]
```

Migration on load: `player.autodj` `"off"` → `autodj = false`, anything
else → `true`; `sonic_tightness` → dropped (the record's model is a switch
and a raw band; the default is on at .55); `tempo_tolerance` (a percent)
→ `bpm = tempo_tolerance > 0`, `bpm_tolerance = 8` (a percent cannot be
reinterpreted); `key_matching` → `harmonic = key_matching != "off"`
(strict is gone). Unknown keys survive through `extra` as they do today.

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
doorway, every room row and its gate, the chooser's three answers, the
capture's banner and Esc, the empty state's buttons following the armed
state, the badge glyphs, the bar's on/off wording. Rig (the two servers,
discovery off): the DJ on a peer session through the proxy and direct (a
pick's `Origin` is the peer's), `discovery is disabled` degrading once,
the Preview row's three picks, `songs_per_fetch` landing a batch.

## Decisions (2026-09-20)

The 09-06 open questions, settled on their leans unless noted; two are
flagged for confirmation before code.

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
4. **Sonic default and slider** — ⚑ *confirm*: the record's ON by default
   with the raw .30–.80 band shown as "N% or closer". Safe only with
   clause 30 in the same slice. The pool size and the raw cosine stay as
   dim details (the player's addition).
5. **Where the room lives** — Settings › `Auto DJ ▸` in a LISTEN group
   above SERVERS; the bar toggle stays the everyday control; the queue
   panel's empty state carries the openers. When the Now Playing screen
   lands the room may also be reachable as a tab there.
6. **Preview** — kept (clause 53), a logged deviation.
7. **The new parameters and the learner** — all ported: sources, the
   length window, the keyword filter, `requireBpm` / `requireMusicalKey`,
   songs per fetch, readiness; the learner on the worker.
8. **The badge** — yes: `∞` / `≈`.
9. **Artist cooldown** — kept (the record has none; the server accepts
   `ignoreArtists`, the row exists, and an artist repeating immediately
   reads as broken). Default 3, 0 is off. A logged deviation.
10. **The DJ follows the session** — ⚑ *confirm*: no server picker; a
    switch in the header moves the DJ and starts a new lane. The record
    arms the DJ FOR a server and lets the user browse elsewhere. The
    player's one-session model makes the record's shape possible now (a
    `Reach` exists for every saved server), but it needs the DJ to keep
    that server's tunnel up on its own; deferred with multi-server
    sessions.
11. **Multi-server sessions** — out of scope for this round (above).
12. **A peer hosts the DJ** — yes, the record's rule since #946; the
    rating row hides and `minRating` is not sent.

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
- **2026-09-20 — The DJ follows the session** (clause 41, decision 10):
  no server picker; a header switch moves the DJ and starts a lane.
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
  surface; it shares the App, so it cannot be left on the old model.
- **2026-09-20 — The badge glyphs**: `∞` and `≈`, dim, before the title;
  the record's icons are its own.
