# Auto DJ

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`439b4de4`, 2026-09-19) — `lib/screens/auto_dj.dart` (the settings screen: status, Queue, Continuity, Filters, Sources, and the multi-server body this surface does not port), `lib/widgets/auto_dj_start_sheet.dart` (the empty-queue chooser), `lib/widgets/queue_list.dart` (`toggleAutoDJ`, `_seedRandom` / `_armLibraryPick`, the `_EmptyQueue` opening-song buttons, the labelled `AutoDjButton`, the queue-row badge), `lib/widgets/player_panel.dart` (the same button on the player), `lib/singletons/auto_dj_manager.dart` (the settings, their persistence, `libraryFilters`, `batchParams`), `lib/media/audio_stuff.dart` (the session: `setAutoDJ`, `startAutoDJFromSeed`, `_startAutoDJFromSeed`, `_autoDJPick`, `sonicParams`, `shouldTopUpAutoDJ`, `shouldDeferDJPick`, `_resetAutoDJSession`, `_queueAutoDJSongs`, `restoreAutoDJ`), `lib/singletons/server_capabilities.dart` + `lib/util/server_version.dart` (the capability learner and the version floors), `lib/singletons/api.dart` (`fetchAutoDjSeed`, the opener). **Re-extracted 2026-09-20** from the 2026-09-06 pin (`695f4d0`); what moved in between: songs per fetch (`limit`, mStream #966), a federated peer hosts the DJ (mStream #946), sonic waits for the discovery scan (`discoveryReady`, mStream #879), opening-song buttons under an empty armed queue, multi-server sessions with vector seeds (out of scope here), and the toasts name the server by its display name. The screen is itself a port of the webapp's panel (`webapp/velvet/app.js viewAutoDJ`; `webapp/alpha/auto-dj.js` for the anchor semantics) — where they disagree, this contract says so. **Re-extracted 2026-11-10** against `1d4e8974` (2026-11-08) for the room's rebuild: `lib/screens/auto_dj.dart` is now one row per setting, each opening one sheet (`lib/screens/auto_dj_sheets.dart`, #222) — Start / Stop is the status; Play from · First song · Batch; **Must match**: Sonic similarity, Smart filters, Genres, Rating, Length; **Prefer**: Beat matching, Harmonic mixing, Skip words; a gated row stays in place with a few-word reason. With it came Recently played as a first song (`lib/util/auto_dj_recent.dart`, `lib/widgets/auto_dj_recent_menu.dart`), Smart filters — the mood filter on every pick (`lib/objects/mood_filter.dart`, `lib/widgets/mood_filter_sheet.dart`; #220, #218), Generated · Tags · Mixed genres (`genreSource`), Length to the second, Play from any set of servers (`lib/util/auto_dj_play_from.dart`, #225 — one server here, see Out of scope), quick config behind a split Auto DJ button (`lib/widgets/auto_dj_quick_config.dart`, #226), and a toggle whose Stop always stops (`toggleAutoDJ`). The GUI's translation of it is the canvas "Auto DJ Panel · GUI" (2026-11-09). **Re-extracted 2026-11-17** for the seeds: the phone's `claude/autodj-seeds` (uncommitted, base `c1393e12`, 2026-11-15) — `lib/util/auto_dj_seeds.dart` (the two lists, their pins, Stay close's freeze, Less like's clock), `lib/screens/auto_dj_seeds.dart` (the Seeds page) and the settings screen's FILTERS and MATCH in `lib/screens/auto_dj.dart` — its translation the desktop canvas "Auto DJ Seeds · Desktop" (https://claude.ai/artifact/ERobcFkF3YUJ9dLVKqGZny — E-AutoDJ, E-SonicOff, E-Sonic, E-Quick, and option A's A-Drift and A-StayClose; A-AddSong not built), and the owner's words of the same days. **The owner's second answers** (2026-11-17, asked of the phone and this player together) are carried over with the phone's plan for them as the reference; where this player differs, the deviations log says so. **The owner's afternoon answers** (round 3, the same day) take the phone's reviewed builds of the two asked of both — its quick skip of the blended song and its ninth row, with the three rules its review added — as the reference; the other three were asked of this player alone. |
| **Server API** | `POST /api/v1/db/random-songs` — one call, every knob (mStream `src/api/random.js`, Joi-validated with no unknown keys): `limit` (1–25, default 1; 6.26.0) · `ignoreList` (round-trip cursor, ≤ 500 ids; the server keeps the last 50) · `ignoreVPaths` (≤ 50) · `minRating` (0–10; ignored for a caller with no user — a federation key or guest) · `genres` (≤ 200) + `genreMode` (`whitelist` default / `blacklist`) · `bpmRanges` + `bpmRangesWide` (≤ 16 windows each, 0–1000) + `requireBpm` · `musicalKeys` (≤ 24 Camelot codes) + `requireMusicalKey` · `ignoreArtists` (≤ 100) · `similarTo` (1–8 paths) + `minSimilarity` (both or neither) · `similarToVector` + `similarToModelId` (6.26.0, out of scope) · `minDuration` + `maxDuration` (seconds, ≤ 86400) + `allowUnknownDuration` (6.25.0) · `genreSource` (`tag` or `model`, beside `genres`; absent matches both — mStream #1102, 6.32.2) · `moods` (the browse lists' filter — a value per scale, `tags`, `threshold` — a base condition never relaxed; mStream #1096, for a server whose `features.moodFilters` says it has scores) · `lessLike` (1–8 paths in the `similarTo` form — no leading slash — of songs the session skipped; more is a 400, and `similarTo` keeps its own 8; mStream #1108, merged and not yet released — the latest release is v6.33.1): **soft**, never a filter — a song like a skip (its cosine to any one negative at least clamp(`minSimilarity` + .10, .80, .95), .80 with no sonic bar) sorts last and is still served when nothing else is left, so negatives never empty a pool and never add a 400, and a negative the server cannot use (unknown, not analysed) is ignored and counted. Answers `{songs: [..], ignoreList, sonic?: {similarity, similarities, poolSize}, lessLike?: {used, ignored, threshold, avoided: [a flag per song served]}}` — the `lessLike` echo present exactly when the body carried negatives. Its refusals, all as `{"error": "..."}`: a **schema rejection** `"<key>" is not allowed` (400 from 6.12.0, 403 before — the body is the signal); nothing left in a sonic pool → 400 `No songs within the similarity range match criteria`; an unanalysed seed → 400 `Sonic seed track has not been analyzed yet`, an unscanned library → 400 `No tracks have been analyzed yet`; discovery switched off → 403 containing `discovery is disabled`; an expired token → 401/403 without a not-allowed body. `GET /api/v1/db/genres`; `POST /api/v1/db/genres` with `source` for one kind, answering `sources` (how many of each kind there are; mStream #716). `POST /api/v1/db/stats/recently-played` (`limit`) — the account's play history, newest first; empty for a caller with no user (a key, a guest). `GET /api/` is the capability source: `server` (the version), `features.discovery`, `features.discoveryReady` (whether the scan has produced vectors — `/api/v1/ping` never carries it), `features.moodFilters`, `features.autoDj` (the picker's generation — 1: random-songs takes `lessLike` and echoes it; the server's own word is that a client sends negatives only from 1 on, and only while `discoveryReady`), `user.vpaths`. Version floors the record keeps: 4.6.0 `ignoreVPaths` · 6.7.1 the BPM / key / genre / cooldown block · 6.15.2 the sonic pair · 6.25.0 the length window · 6.26.0 `limit` · 6.32.2 `genreSource`. random-songs is on the federation allowlist (mStream #946): a peer session — through the parent's proxy or over the peer's own tunnel with a guest token — can run the DJ. |
| **Already in this repo** | (As found at extraction on 2026-09-06 — the names are that day's code, since reworked.) Most of a DJ, in the older three-mode shape: `src/dj.rs` (Camelot math, same/half/double windows, the perceptual sonic slider, `build_random_request`, `Settings`), `[player.dj]` prefs and `player.autodj`, `AutoDjMode` and the `A` cycle, `maybe_autodj` (the queue-end top-up), `consume_dj` (queue the pick, start it if idle, the cursor), `autodj_pick` (`Similar` via nearest neighbours, `BpmKey` via random-songs, the sonic 400 retried once without the pool), `autodj_sample` (three picks without queueing), the TUI's Auto-DJ tab (`DjPanel` / `DjRow`) with its genre picker, `Event::{AutoDjPick, AutoDjSample, Genres}`, and the GUI bar's `auto-dj` toggle. Since 2026-09-06 the multi-server contract landed and gives this one its footing: every queued row carries its `Origin` and plays from its own server (`Reach`, `reach()`), federated peers are sessions of their own (proxied or direct), tunnels follow the queue with a hold for a row whose tunnel is down, the layered `/api/` payload is parsed (`LayeredInfo`), and the GUI queue panel has rows of its own to badge. **Missing**: the on/off-plus-toggles model, sources (`ignoreVPaths`), the length window, the keyword filter, `requireBpm` / `requireMusicalKey`, the session-locked Camelot anchor, the rolling / locked sonic anchors as the record defines them, songs per fetch, the readiness gate, the one-shot seed and the start chooser, the empty-queue openers, lane resets with in-flight discards, the capability learner, the failure taxonomy, the queue badge, the armed-not-playing restore, and the room itself. |
| **Target surface** | the GUI player — the bar toggle, the queue panel (its badge and its empty state), and the **Auto DJ room** — a Library nav room under TOOLS since 2026-09-23 (Settings › `Auto DJ ▸` before); the TUI's `A` and Auto-DJ tab follow through the shared App |
| **Status** | contract extracted 2026-09-06; **re-extracted against the moved record and settled 2026-09-20** — the open questions are decided below, and decision 10 was rewritten the same day to the record's model (the DJ is armed FOR a server, the session browses where it likes); implementation began 2026-09-20 (PLAN.md, Phase 10): **slices A1–A5 landed 2026-09-20/21** — the model, the worker, the GUI room and its surfaces, the TUI's keyword entry, and the rig run (`smoke/gui/scenario_dj.py`: the room, a batch, Preview, the server picker moving the DJ to a proxied peer with the next turn picked from there). Left for a server with discovery data: the sonic path end to end (a pool answered, clause 30's degrade) and a direct-tunnel peer lane. **2026-09-23** — the room moved from Settings to the Library's nav column, a TOOLS group beside the sonic room, where the record's browse root and desktop rail keep it (decision 5 amended; the deviations log). **2026-11-10** — the room rebuilt from the re-extracted record and its canvas: one row per setting, each opening one sheet; First song with Recently played; Smart filters; the genre kind; Length to the second; Play from with a home; the bar's split button and quick config; the toggle's Stop always stops (clauses 2, 8, 16, 20, 40–58; decisions 13–19; the deviations log). **2026-11-17** — the seeds: two lists, More like and Less like, a pin keeping a row and Drift · Stay close moving or freezing the rest, on a Seeds sheet over the room (option A); Learn from skips and the negatives on the wire, a learned skip's re-roll, Add song through the library pick, the pins kept through new queues and restarts; the room's FILTERS and MATCH, Sonic similarity's two sub-rows (clauses 10–15, 23, 25, 30, 31, 36, 41, 43, 45, 51, 54, 57–59, 63–66; decisions 20–34; the deviations log). **2026-11-17, later** — the owner's second answers: at a lane's first pick in Drift the queue's current song joins More like beside the pins; a play that replaces the queue is a new queue for the seeds, not a new lane; Stay close keeps every row it shows through new queues, Stop and Start, and restarts, the file with it; a first song chosen in Stay close joins More like pinned (States & flows; clauses 10, 12, 15, 23, 51, 63, 64, 66; decisions 27 and 32 amended, 35–38; the deviations log). **2026-11-17, round 3** — the owner's afternoon answers: a quick skip of the user's own song a lane's start blended in takes it out of More like and learns nothing, and the lane's picks pass over it; the end of a track whose queue a clear or a removal emptied asks no pick; the TUI, which shows no Seeds sheet, keeps the seed lists' rules from before the second answers; Stay close's one song comes in past eight rows, up to sixteen, for a More like with nothing the DJ's server can use — another server's rows, or rows that server refused — and a pick never leaves a pin out for it; a play that replaces the queue gives a lane that let its sonic pool go another go, which a pick already out cannot take back; a song's tags fill its seed rows; and, from its review, a pick out at a play that replaces the queue no longer becomes the new queue's start — the row the play started is — a track's end or hand-over while a row waits for its tunnel asks nothing, and Stay close takes no song for the user's own removal of its last usable row (States & flows; clauses 10, 12, 15, 23, 26, 30, 31, 43, 63, 64, 66; decisions 27, 32, 35 and 37 amended, 39–44; the deviations log) |

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
   (Play from · First song · Batch · Artist cooldown), then FILTERS and
   MATCH — one row a setting, Sonic similarity's two sub-rows under it
   (clauses 40–59). *(Regrouped 2026-11-17; MUST MATCH and PREFER before.)*
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
   button: Play from, Sonic similarity — its match bar and the Seeds row
   while it is on — Smart filters and the way to the room, in a popover
   over the bar (clause 58).

## States & flows

Off → switched on → (empty queue only) the **opening question** — a
random song / one chosen from the library / one of the songs played last
(clause 8), the way to start rememberable → **armed**, with a
session **lane**: the ignore cursor, the Camelot anchor, this lane's own
picks, whether its first pick is still to come and the song it blended in,
the songs its picks pass over, and Stay close's one song, the
once-per-lane warning budgets, the suppressed sonic keys. The two
**seed lists** (clause 23) outlive the lane — a new one lets only the DJ's
unpinned picks go, and none while Stay close holds them in the GUI — and
what they keep outlives the run: the pins, and every row while Stay close
holds them there; the TUI, which shows no Seeds sheet, keeps the pins
alone (clause 66).
Every way the user steers somewhere new — switching on, moving the DJ to
another server, clearing the queue, removing the DJ's server — starts a
**new lane**; re-arming on the same server does not, and browsing another
server does not either: the DJ is armed FOR a server, and the session goes
where it likes. **Playing something that replaces the queue** starts the
seed lists over as a clear does, without a new lane, and gives a sonic
pool the lane let go another go (clause 12). Off ends
the lane. Settings edits apply to the running lane at once. A lane picks
from ONE server: the DJ's.

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

10. A **new lane** resets the ignore cursor, the Camelot anchor, the
    warning budgets, the sonic keys suppressed by clause 30, and the song
    its start blended in with the songs its picks pass over (clause
    23(d)), re-arms Stay close's one song (clause 23), and makes its next
    real pick the lane's first, where in Drift the queue's current song
    joins More like (clause 15). The seed lists outlive it *(changed
    2026-11-17, and again with the owner's second answers)*: in Drift More
    like's unpinned rows go — the DJ's picks, the opener, the song the lane
    opened on — and every pin, and every unpinned Less like row with its
    clock still running, stays; while Stay close holds the lists, every row
    stays in the GUI, which shows them, and in the TUI More like's pins
    alone, Less like standing as it was (clause 66, decision 41).
    Triggers: switching on, moving the DJ to another server, clearing the
    queue, a server removal that empties the queue. Re-arming on the same
    server is not a new lane; nor is a switch of the browsed session, nor
    a play that replaces the queue, which starts the seed lists over the
    same way without one (clause 12). (The record's fifth, Discover's
    "Start a sonic session", is out of scope here — entry point 4.)
11. A pick **in flight when the lane changes is discarded** — it must not
    repopulate a cleared queue, overwrite the fresh cursor, or seed the
    new lane's history with the dead one's tracks. (The record stamps each
    call with a lane epoch and drops a reply whose epoch is stale; a
    request is nearly always in flight during a session.) A pick asked
    **before a learned skip** (clause 23(f)) is stale too *(new
    2026-11-17)*: its songs, its cursor and its failure are dropped and
    the pick is asked again at once; its degrade note stands, since the
    server has already let that lane's pool go and the second ask would
    not say so.
12. A **queue clear keeps the DJ armed and adds nothing** — no chooser, no
    surprise pick, and no pick for the end of the track it threw away,
    which can still come before the clear's Stop is answered: a track that
    ends, or hands over, with no queue left asks nothing — after a removal
    that empties the queue too *(new 2026-11-17, round 3: decision 40)*,
    and while the queue's row waits for its tunnel (multi-server clause
    37), whose play sent a Stop and no Play;
    the DJ resumes from a clean slate at the next queue end, or when the
    user opens a song from the empty state (clause 16).
    **A new queue, for the seeds** *(rewritten 2026-11-17, the owner's
    second answers; the clear alone before)*, is a clear — the queue
    header's `clear`, `C`, and the control face's `/play` and
    `/queue/clear`, which clear first — or any play that replaces the
    queue: a click or Enter on a track, which plays from there
    (library-rooms clause 14); the browser bar's Play and Shuffle
    (browser-top-bar clauses 10 and 12); a sonic path's Play, or a row of
    it played (sonic-path clauses 33–34); and Discover's own — a song or a
    connected server's row played from, the verbs line's Play and Shuffle,
    Play album on a connected server's tile (discovery-page clauses 48,
    60, 62 and 64). At each, the DJ on or off: in Drift the DJ's unpinned
    More like rows go, and the queue's current song joins More like at
    the next real pick (clause 15); Stay close keeps every row it shows
    in the GUI, and in the TUI More like's pins alone beside Less like as
    it stands (clause 66, decision 41), and its one song is re-armed
    (clause 23(c)).
    With **Clear pins when the queue is cleared** ticked (clause 66), a
    new queue where the queue held a row also lets every row drawn pinned
    go: in Drift the pins of both lists, unpinned skips keeping their
    clocks; in the GUI's Stay close every row; in the TUI's, which draws
    none, the stored pins, as in Drift (decision 41). A play that replaces
    the queue is still no new lane (clause 10, decision 27): the ignore
    cursor, the Camelot anchor, the songs the lane's picks pass over
    (clause 23(d)) and a pick already out stand, and that pick lands after
    the new rows — so with one out, the new queue's first real pick is
    composed as the play starts its row, and sent nowhere: the row that
    started opens the new queue in Drift, or is Stay close's one song when
    nothing else is to stay close to, and the late pick, the queue's end
    by then, joins as any pick does *(round 3, found in review: it was the
    new queue's start, and nothing of the new music joined)*. A pool the
    lane let go (clause 30) gets another go with the new music, as a More
    like edit gives it, the room's gold reason going with it, and a
    degrade the pick already out brings does not take
    it back *(changed 2026-11-17, round 3: the owner's "Try again",
    decision 43)*; a clear needs none, its new lane never having let its
    pool go. A seeded start opens a lane of its own (clause 5), and a
    launch's restore is no new queue.
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
    and remembers the paths of the rest). The one landing that replaces
    rows is a learned skip's re-roll (clause 23(f)).
15. **A pick joins More like when it starts playing** *(rewritten
    2026-11-17; before, at queue time, the rolling sonic history)*: in
    Drift a DJ pick becomes More like's newest row (clause 23) as it
    starts — a play, or a hand-over into it — once per track, before the
    top-up builds the next request, so the turn it triggers is already
    seeded on it. Only this lane's own picks, only while the DJ is armed,
    never in Stay close; sonic on or off, so switching it on mid-session
    already has the session's recent sound to anchor on. A DJ row restored
    from an earlier run, or queued by an earlier lane, never joins as it
    starts — it can still be the song a lane opens on (below). A session's
    first song — Random's, Choose's or Recently played's — is More like's
    first row in Drift as the session opens. In Stay close *(changed
    2026-11-17, the owner's second answers; the first real pick held to it
    before)* a song the user chose — Choose's or Recently played's — joins
    More like pinned, as Add song would (clause 23(g)), where More like can
    take a row, sonic on or off — in the GUI, which shows the pin; with
    More like full and nothing there usable, the first real pick takes it
    as Stay close's one song, unpinned (clause 23(c)). The TUI, which
    shows none, keeps the rule from before and holds to it as to Random's
    *(round 3: decision 41)*. Random's is the DJ's song: nothing
    is added, and the first real pick holds to it only when there is
    nothing else to stay close to (clause 23(c)). **The song a lane opens
    on** *(new 2026-11-17, the owner's "blend in new music")*: as a lane's
    first real pick is composed — after a new lane (clause 10: Stop and
    Start, a move of the DJ and a removal that empties the queue among
    them), after a new queue (clause 12), and at a launch — the queue's
    current song, on the DJ's server, joins More like in Drift, unpinned
    and newest as a pick does, beside the pins, and rotates out like any
    pick; sonic on or off.
    Not a song either list holds — in More like it keeps its place and its
    pin, and in Less like the user's word wins — nor one of this lane's own
    picks, which join as they start. The pick is built on it at once. A
    pick is asked at the queue's end (clause 13), so after Play on an
    album the song is the album's last — unless a pick was still out as
    the album replaced the queue: that pick lands after the album, so the
    album's first pick is composed as its play starts, on the song that
    started (clause 12, decision 35). A turn owed to a tunnel that is
    not up is not that pick; the one its return asks is. Preview (clause
    53) shows the song without keeping it, and leaves the first pick to a
    real one (decision 35). A quick skip of the song while More like holds
    it unpinned takes it out *(round 3: decision 39)*: the user's own song
    teaching nothing and passed over by the lane's picks from then on, a
    DJ pick an earlier lane or run left teaching as any pick does (clause
    23(d)). A session's first song is never this song: it is in More like
    already when the lane's first pick comes.
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
23. **Sonic similarity: the seeds** *(rewritten 2026-11-17 — before, the
    last five DJ picks or one locked pin)*. The pool is asked for —
    `similarTo` + `minSimilarity` (raw cosine, .30–.80, default .55) —
    only when the DJ's server reports `discovery` on and does not report
    `discoveryReady` false (clause 36; a server that cannot report
    readiness holds nothing back), the switch is on, and the keys are not
    suppressed this lane (clause 30). What it is asked to be like, and
    unlike, are two **seed lists**:
    - (a) **The lists** — **More like** (the positives) and **Less like**
      (the negatives). A row is a song — its server, its path, its title,
      artist and album as known — whether the user pinned it, how it came
      (a pick · the opener · Stay close's one song · a skip · the user's
      own) and when. Newest first, at most 8 rows each — More like up to 16
      while Stay close's one song carries it past eight ((c)) — a song in
      one list only. More like keeps at most min(5, 8 − pinned) unpinned
      rows — the record's five, the old rolling history's — the oldest
      unpinned leaving first; Less like holds 8, the oldest unpinned
      evicted for a new one, and a skip is not added when all 8 are pinned.
      A seed is a path on the DJ's server (paths are per-library): a row of
      another server stays listed and is not sent.
    - (b) **Positives, by mode** — the anchor setting, kept: `sonic_anchor`
      *rolling* is **Drift** (the default), *locked* is **Stay close**.
      Drift sends More like's rows on the DJ's server, newest first, and
      with none the playing track there (a session's first song is a row
      already, and so is the song a lane opened on — clause 15). Stay close
      sends its frozen rows — those the DJ's server has not refused in this
      lane, eight at most ((c)). Never a song that is in Less like.
    - (c) **Stay close freezes both lists**: every row is drawn pinned and
      locked, nothing is pushed, a skip adds and removes nothing, Less
      like's clock stops. **One song only to start**: a lane with no
      usable More like row — one on the DJ's server that the server has not
      refused in this lane (clause 31) — gets the playing song on the DJ's
      server (right after a seeded start, Random's opener, or the song the
      user chose — in the GUI that one is in already, pinned, where More
      like had room for it (clause 15); from eight rows on it comes in here
      as the one song, unpinned, as in the TUI) once, on a real pick —
      Preview shows what it would hold without keeping it — and after that
      nothing is added or removed until a new lane (clause 10), a new
      queue (clause 12) or a switch into Stay close looks again *(changed
      2026-11-17, the owner's second answers)*;
      a lane that finds a usable row takes none. **Past eight** *(changed
      2026-11-17, round 3: the owner's "Let it in as a 9th row", decision
      42)*: a More like full of rows the DJ's server cannot use — eight of
      another server's, as a move of the DJ can leave them, or rows that
      server refused in this lane — takes the song as one row more, in
      front, and nothing is evicted for it: the pick sends it, and in the
      GUI the sheet lists it and the file keeps it. A lane takes one song at
      most, so More like grows past eight by one a lane, and never past 16,
      all a restart brings back (clause 66): at 16 the song is not taken,
      the picks send no positives, Preview shows none, and the log says so
      once each time the rule looks, until a row is removed and the next
      real pick takes the song. From eight rows on More like is full all
      the same — Add song, a move into it and a chosen first song find no
      room — and a pick sends eight positives at most: past eight usable
      rows the oldest row the user did not pin stays home first, so no pin
      is left out for the one song. A lane held to rows its server has
      refused since, before it took a song of its own, looks again at its
      next real pick and takes the song playing, Preview reading the same —
      never after its own song, and never for the user's own edits: a list
      the user emptied, or one whose last usable row the user removed or
      moved to Less like, rows refused before still listed *(round 3, found
      in review)*; and a song the server refused in this lane is no song
      to stay close to: the lane keeps looking, and the next song that
      plays comes in. With
      every row removed, picks send no positives (no `similarTo`, no
      `minSimilarity`) until a song is added, the mode switched, or a new
      lane or queue takes one song to start; negatives still go. Back to
      Drift, the user's own pins return (the stored flags never changed),
      Less like's clocks skip the paused span, and More like trims to
      capacity, oldest unpinned first. The DJ never evicts a row drawn
      pinned, and never takes a row out of a frozen list within a lane — a
      dead one included (clause 31). In the GUI every row Stay close shows
      outlives new lanes and new queues, Stop and Start, and restarts
      (clause 66), where a new lane let its own unpinned rows go before the
      owner's second answers; the TUI, which shows no sheet, keeps that
      older rule (decision 41): there More like keeps its pins alone
      through them, and the file the pins of both lists.
    - (d) **The quick skip and Learn from skips** (a switch, off by
      default). A quick skip is a Next on a DJ pick — on the DJ's server,
      the DJ armed — or on the user's own song the lane's start blended in
      (clause 15) while More like holds it unpinned *(round 3: the owner's
      "Take it out", decision 39)*, heard for a second or more
      (play-reporting clause 6) that does not count as a play (30 s, or
      half a known length: the server's defaults, decision 26), where the
      Next moves to another row: repeat-one, a wrap and the last row do not
      count, and a tap on a queue row, a failed track's skip walk and a
      removal are not skips. In Drift a quick skip takes the song out of
      More like unless it was pinned there, Learn on or off; with Learn on,
      sonic on and the server taking negatives, it joins Less like unpinned
      — or, already there, moves to newest with its clock restarted. The
      blended song only leaves: nothing is learned from a song the user put
      on, Learn on or off — no Less like row, no re-roll ((f)), no negative
      — and the lane's picks pass over it until the lane ends (clause 26),
      since the DJ never served it and, out of More like, it is no seed. A
      DJ pick the start blended in — one an earlier lane or run left, or
      the song served again — keeps a pick's rule, Learn included; one the
      user pinned since stays put, and its skip says nothing. An unpinned
      Less like row leaves after 30 minutes of Drift time; unpinning a row
      restarts its clock.
    - (e) **Negatives**: `lessLike` is Less like's paths on the DJ's
      server, newest first, at most 8 — sent only while sonic is on and the
      server takes them: `features.autoDj` 1 or more, discovery on, and
      `discoveryReady` reported true. That gate is the server's own flag,
      never a version floor (clause 25). The mode does not gate them —
      Stay close sends its frozen Less like — and a pick whose positives
      were dropped carries them still, with no `minSimilarity` (the server
      refuses one without a seed). Never on the opener (clause 3); Preview
      carries them. The server's echo is one `[dj]` line (clause 63).
    - (f) **The re-roll**: a quick skip that changed the set of songs
      `lessLike` will carry (a re-skip that only moves a row to newest is
      no change), with the Next moving forward — no shuffle, no wrap —
      asks for a pick at once. Its new songs replace the DJ rows after the
      playing one, nearest first, one for one, as many as it brought, and
      the rest are appended; a song it brings that is already queued — a
      replaceable row's own song included — keeps its place and counts as
      neither, so no song lands twice. With no playing row everything
      appends. User rows are never dropped, and nothing is dropped under
      shuffle. A pick asked before the skip is asked again (clause 11); a
      cleared queue or a new lane drops the re-roll with the rest.
    - (g) **The user's edits** (the Seeds sheet, clauses 64–65) make pinned
      rows, in either mode, but only into a list that can take a row — one
      with room, or, outside Stay close, an unpinned row to give up. A song
      already in the target list becomes pinned and newest; one in the
      other list moves over. Remove deletes the row; a move takes it to the
      other list, pinned. A song chosen to open a session in Stay close
      comes in the same way (clause 15).
    - (h) **Sonic off keeps the seeds** *(changed 2026-11-17; switching it
      off cleared the lane's anchors before)*: no pool is asked for, both
      lists stand as they are, picks keep joining More like (clause 15),
      and switching sonic on picks up where they stand.
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
    — and it forgets a server whose version changes. The negatives
    (clause 23(e)) are gated on the server's own flag, `features.autoDj`,
    never on a version floor *(2026-11-17)*; the learner still guards an
    older server that refuses `lessLike` anyway, as it does any key.
26. **The keyword filter** is client-side: a song whose title, artist,
    album or filepath contains any word (case-insensitive) is refused. A
    batch keeps whatever part of it passes; only an answer blocked in
    FULL is re-asked with the updated cursor, up to 5 times; then the last
    answer is taken whole rather than stalling the queue forever. **A song
    the lane passes over** — the blended song the user quick-skipped out of
    More like (clause 23(d)) — is refused the same way, known by its path
    with or without a leading slash, and the last answer taken whole
    leaves it out while anything else is left *(new 2026-11-17, round 3:
    decision 39)*. Real picks and Preview pass over it; the opener, which
    asks for a new session, does not.
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
    the lane — until a change to a sonic setting, the mode or More like
    (clauses 43, 64), or a new queue (clause 12), gives the pool another go
    — and retries the same pick without them. The first two say so
    **once** per lane — "Auto DJ: nothing is within the similarity range,
    so it is playing without that filter. Loosen the match slider to use
    it again." / "Auto DJ: the discovery scan hasn't reached these tracks
    yet, so it is playing without sonic similarity." — the third stays
    silent (the user switched it off themselves). Logged every time. A
    path seed's 404 or "analyzed" body goes through clause 31 first
    *(2026-11-17)*: the lane lets the pool go only when no seed is found
    to blame. `lessLike` is not one of the keys a degrade lets go of
    *(2026-11-17)*: the server ignores a negative it cannot use rather
    than fail on it, and a degraded pick carries the negatives still
    (clause 23(e)). A pick composed before the pool's latest retry that
    degrades after it does not take the retry back *(new 2026-11-17, round
    3: decision 43)*: its songs land, its note is not said, the room's
    reason stays away, and the lane has its pool again; a pick composed
    after the retry degrades as before.
31. **A dead seed** *(rewritten 2026-11-17 — before, a dead locked pin was
    dropped so the next call re-pinned from the playing track)*: one More
    like path the server cannot seed from fails the whole pick, and the
    server does not say which. With one seed sent, a 404 or an "analyzed"
    body marks it dead — the server refuses seed by seed; with several, a
    404 or an "analyzed" body has the pick ask once per seed alone, and
    those refused the same way are dead — once a pick. A dead seed is left
    out of the rest of that lane's pools, a retry of the pool included —
    the lane remembers it, a Preview's lane apart — and the pick goes
    again without it. When every seed it sent is dead, that pick goes
    without the pool and the lane keeps its sonic: the next pick's new
    positives are sent. Only when no seed is found to blame does the lane
    let the pool go (clause 30). The row itself stays listed, in Stay
    close too, until the user removes it: the lists never learn that a
    row is dead. The running lane knows, and Stay close reads it *(round 3:
    decision 42)*: a row its server refused in this lane is no row to stay
    close to, nor a song refused one to take (clause 23(c)); a new lane
    starts with none.
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
    lane. The switch keeps its setting for when the scan lands. The
    negatives' gate is stricter *(2026-11-17)*: they go only while
    readiness is reported true, as the server asks (clause 23(e)) — a
    server that cannot report it gets none.

### The room

*(Re-laid 2026-11-10 from the record's rebuilt screen and the canvas "Auto
DJ Panel · GUI": one row per setting, each opening one sheet. Clauses 40–53
keep their numbers and say the new room; 54–58 are new. What the rows DO —
the request, the lane, the degrades — is clauses 1–36, unchanged.)*
*(Regrouped 2026-11-17 from the canvas "Auto DJ Seeds · Desktop",
E-AutoDJ: the session rows — Play from, First song, Batch, Artist
cooldown — then **FILTERS** (Smart filters, Genres, Rating, Length, Skip
words) and **MATCH** (Sonic similarity with its two sub-rows, Beat
matching, Harmonic mixing), where MUST MATCH and PREFER stood. Clause 59
is new; the Seeds sheet and its pick are clauses 64–66.)*

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
    `Discovery off` · `Still scanning` · `No mood data` · `Not on peers`
    — and `Needs a newer server`, Learn from skips' own, which stands on
    its line under its name rather than in the value's cells, and only
    once the server has said it takes no negatives (clause 59). A reason
    never loses its words: where a name and its reason do not both fit,
    the name gives way. **A sub-row** *(new 2026-11-17)* stands under the
    row it belongs to, its name two cells in (Sonic similarity's two,
    clause 59): while that row is off or gated it rests in place, DIM
    whole and keeping its value or its switch's glyph — no click, no
    hover, and the keyboard passes it. **The block** — the title row, the
    button, the headers and the rows, as one — is the room's width up to
    56 cells, a fifth past the canvas's 47, and past that it stands
    centred in the room rather than at its left.
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
    whichever server the DJ last ran on, however it was started, and a
    server re-addressed in the servers room takes it along *(2026-11-17;
    the next save wrote the old address back before)*. The session's own
    header switch never moves the DJ. More servers and folders than the
    sheet affords stand behind its window: the wheel and the kit's
    scrollbar move it, and a walking key brings the hand's line into
    view.
43. **Sonic similarity** — `{pct}%` (OK), `Off`, or the server's reason
    (`Needs server 6.15.2+`, `Still scanning`, `Discovery off`). Its
    sheet *(changed 2026-11-17, E-Sonic)*: `[✓] On`; the floor as a bar
    across the band, .30–.80, two points a cell, with `-` `+` and ← → a
    point at a time; and "Only pick songs that sound like the session,
    using the server's audio analysis." Switched off, the bar rests —
    DIM, no click, the keyboard passes it. **Drift · Stay close** left the
    sheet for the Seeds sheet (clause 64). **ON by default** (safe only
    because clause 30 degrades instead of stopping). A running session
    that dropped the pool says why on the row in GOLD — `! Too strict`,
    `! Still scanning` — and the sheet still opens, since it is where the
    user goes to loosen it; a change there to the switch or the floor
    gives the lane another go with them (the record's `retrySonic`), as a
    change to the seeds' mode or to More like does (clause 64), and, since
    round 3, a new queue (clause 12, decision 43) — the row's GOLD reason
    goes, and a pick already out cannot bring it back (clause 30). Switching
    sonic off keeps the seeds (clause 23(h)).
44. **Beat matching** (the record's Tempo — BPM continuity, clause 21) —
    `± {bpm} BPM` or `Off`. Its sheet: `[✓] On`, the tolerance as a bar, a
    cell a BPM (1–20), and "Prefer picks within a tempo window of the
    current song. Honours half/double-tempo equivalence." Gated `Needs
    server 6.7.1+`.
45. **Harmonic mixing** — a switch: `[✓]` / `[ ]` at the values' edge
    and no sheet; a click, Space or Enter flips it. Gated `Needs server
    6.7.1+`. *(Changed 2026-11-17: no longer the one switch on the page —
    Learn from skips is another, clause 59 — and its line "Next song in a
    key that mixes well" is gone: at 100×30 the room needs all 23 rows for
    the seeds' sub-rows. The record keeps the line; logged.)*
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
    Its sheet: the kit's text input — a 3-row card that takes every letter
    typed (hint: e.g. "live" or "remix") — with `+` framed beside it;
    Enter, or a click on the `+` once a word is on the line, adds the
    word; the words stand under the card, each with its `[X]`; at
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
    session-wide ones (batch, sonic and the seeds' mode, Learn from skips,
    the seed rows kept and their checkbox, Smart filters, length, skip words,
    beat matching, harmonic, cooldown, First song) travel with the user.
    First song's list, the room's two switches (Harmonic mixing, Learn
    from skips), the Seeds sheet (clause 64) and quick config have no
    draft: there a press applies at once.
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
54. **The floor** *(rewritten 2026-11-17)*: the room's rows always stand
    whole, with no scroll bar. What the height affords beyond them comes
    back in one order, each whole or not at all, a later one taken where
    an earlier does not fit: Learn from skips' line (clause 59); Start /
    Stop as its 3-row frame; the blank row before FILTERS, then the one
    before MATCH; a blank row under the frame, then one over it too. Short
    of the frame, the button is the kit's one-row slab at the title row's
    right, Preview beside it. Where even the rows cannot all stand —
    100×24 with the tips line and Play from — the two headers fold first,
    and their blank rows with them. At 100×24 every row stands — thirteen
    settings, fourteen with Play from — on the slab; at 100×30 without
    Play from everything does, on the room's 23 rows (E-AutoDJ), and with
    Play from Learn's line, the frame and the blank row before FILTERS.
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
    and "Keep recently played artists out of the next picks." A session
    row since 2026-11-17, under Batch, where E-AutoDJ draws it (decision
    13 amended).
58. **Quick config** (the record's split Auto DJ button): the bar's
    `auto-dj` frame is two halves — the word starts and stops as it
    always has, and the chevron opens a popover rising from the rule
    between them — over the nav's rule, never over its rows, and never
    over the bar — its groups a blank row apart: **Play from** (the servers as
    one-ticked choices while there are four at most; past that one row to
    the sheet; not drawn for one server), **Sonic similarity** with its
    value and, while it is on, its match bar — 25 cells four in, a click
    on a cell setting it, `-` `+` and ← → a point — and `Seeds ▸` to the
    Seeds sheet (clause 64), both hidden while sonic is off or gated, as
    Drift · Stay close were *(changed 2026-11-17, E-Quick; Drift · Stay
    close before)*, **Smart filters** (a row to its sheet), and `All Auto
    DJ settings ▸`. Every change applies at once — no draft, no Done. A
    setting the server cannot do rests with its reason. A click anywhere
    else, the chevron again, or Esc puts it away. While it is up, as while
    any of the DJ's layers is (a sheet, the Seeds sheet, the First song
    question and its lists), the Library beneath draws without the
    pointer, as under a modal: nothing of it lights or ripens a tooltip.
59. **Learn from skips and Seeds** *(new 2026-11-17)* — Sonic
    similarity's two sub-rows, under it in MATCH (E-AutoDJ). **Learn from
    skips** is a switch, `[✓]` / `[ ]` at the values' edge, with its line
    DIM under it four cells in — "Skipping a song will prevent similar
    songs from being selected" — and a click, Space or Enter flips it at
    once (clause 23(d)). **Seeds** is `▸` with no value: it opens the
    Seeds sheet (clause 64). While sonic is off or gated both rest, DIM
    whole in place, the switch keeping its glyph (E-SonicOff). Learn also
    rests where the server takes no negatives (clause 23(e)): once a probe
    has said so (`features.autoDj` 0, or a server that answers only the
    flat ping) its line reads "Needs a newer server" instead; while
    nobody knows yet, it rests under its usual line — no reason is
    claimed before the server has given one. With the DJ off the room
    asks Play from's server as it opens (clause 50), again whenever the
    DJ stops — the room's Stop, the bar's word or `A` — and when a session
    reconnects under the room or one of the DJ's layers, so these rows
    never wait for the room to be entered again.

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
    difference lives. The seeds speak there too *(2026-11-17)*: the
    server's `lessLike` echo — how many negatives it used and ignored, its
    threshold, how many of the songs it served are like a skip — one line
    an answer; a skip learned; a stale pick asked again; a re-roll's songs
    replaced and added; Stay close's one song, or the More like of sixteen
    rows, none of them usable, that had no room for it (once each time the
    rule looks, clause 23(c)); a seed found dead — and, with the owner's
    second answers, the song a lane opens on joining More like, and a song
    chosen to start Stay close pinned there; and, with round 3, a quick
    skip of the song a lane opened on, out of More like and passed over
    from then on (clause 23(d)), a Stay close lane whose server refused
    every row it held to taking one song after all, and an answer kept out
    in full — a keyword hit, or a song the lane passes over — asked again
    (clause 26).

### The seeds

*(New 2026-11-17: the record's Seeds page as the canvas "Auto DJ Seeds ·
Desktop" translated it — option A, the owner's pick — with the library
pick in place of its Add song sheet. What the lists DO is clause 23.)*

64. **The Seeds sheet** — `Seeds ▸` in the room (clause 59) and in quick
    config (clause 58) opens it over the room, in the mood Filters' shell
    (library-rooms clause 32): 66 cells, `Seeds` on its top border with no
    value after it, no `[X]`, the room inert beneath. It opens with the DJ
    on or off.
    - **Its rows**, top to bottom: **Drift · Stay close** side by side,
      the mode in force `(•)` (clause 23(b)); the mode's line DIM under it
      — "Songs are rotated out regularly. Pin a song to keep it from being
      removed." in Drift, "All songs are now pinned. No new songs will be
      added." in Stay close — on two rows kept for it; a blank row;
      **MORE LIKE** DIM, its rows, `+ Add song`; a blank row; **LESS
      LIKE** DIM, its rows, `+ Add song`; a blank row; **Clear pins when
      the queue is cleared**, the kit's checkbox in a modal, on a row of
      its own (clause 66); `[ Done ]`.
    - **A row** is its pin — `◉` in the OK colour where the user pinned
      it, `◌` DIM where not — the title, then ` · ` and the artist DIM,
      and `[⋯]` at the row's end; no covers. A click or Space on the pin
      toggles it.
    - **No draft**: every press applies at once, and Done only closes —
      as Enter does from anywhere, and Esc.
    - **In Stay close** every pin is `◉` DIM and inert — the lists are
      locked (clause 23(c)) — and the menus still work. Every row it shows
      is kept through new queues, Stop and Start, and restarts (clause 66).
      More like can list more than eight rows there, sixteen at most,
      where its one song came in over rows the DJ's server cannot use
      (clause 23(c)) *(round 3)*; the lists scroll as ever, and nothing on
      the sheet changes for it.
    - **Without negatives** — once the server has said it takes none
      (clause 59) — Less like's rows are DIM and "Needs a newer server"
      stands DIM on its header row; its pins and its Add song are inert,
      its menus still work, and "Less like this" in More like's menus is
      DIM. While that is not known yet, Less like is drawn the same, with
      no reason.
    - **Add song** arms the library pick for its list (clause 65). It is
      DIM and inert — no click, the keyboard passes it — while its list
      can take no row (eight rows or more, all drawn pinned) or while the
      library on screen is not the room's server: the pick could only be
      refused there.
    - **The menu** is the kit's context list on a song row, hung one row
      under its row with its right border on the `[⋯]`'s `]`, rising above
      the row where it would pass the sheet's last row: "Less like this"
      on More like's rows, "More like this" on Less like's, then "Remove"
      in the danger colour. A move the other list cannot take is DIM and
      the cursor passes it. A verb applies at once and closes the menu; a
      click anywhere else closes it and keeps; it folds when its row
      leaves, or leaves the window. While it is down the sheet's hand is
      not drawn and the wheel rests.
    - **Lists that outgrow the sheet** scroll: only the lists, from MORE
      LIKE to Less like's Add song, behind the kit's scrollbar — the mode,
      its line, the checkbox and Done stay put. The sheet is as tall as
      its rows, up to 24 at 100×30 and 18 at 100×24, rising toward row 1
      only when it must; it never covers the bar. Its height follows the
      rows live and it stands centred, so a row coming or going — a pick
      starting in Drift, a Less like row running out — can move it by a
      row; the hand stays on its stop.
    - **Keys**: ↑ ↓ ← → walk the stops on their grid — the two modes; a
      row's pin and its `[⋯]`; Add song; the checkbox; Done — and Tab and
      Shift-Tab the reading order, wrapping; Space presses the stop under
      the hand; `m` on a row — its pin or its `[⋯]` — opens its menu;
      Enter is Done from anywhere,
      and so is Esc; PageUp and PageDown move the window. With a menu up,
      ↑ ↓ walk its verbs, Enter or Space picks, and Esc, ← or `m` fold it
      — the sheet stays. Opened by a click, the sheet stows the hand until
      the first key.
    - A change on the sheet to what the positives may be — the mode, a
      More like row added, moved, removed, pinned or unpinned — gives the
      lane's pool another go (the record's `retrySonic`, clause 43). The
      rows the engine brings in itself — a pick, the song a lane opens on
      (clause 15), a chosen first song — do not.
65. **The seed pick** — Add song arms the library pick for its list: the
    App's `Capture` grown a `Seeds` variant, the sonic pick's road
    (sonic-path clauses 10–13). The sheet goes, the Files room opens, and
    the banner stands a row above the room — "Pick a song for More like —
    click a track anywhere in the library · Esc cancels", or Less like —
    with `[X]` at its right and the room a row lower; the tips line names
    the pick's keys.
    - **A track activated** in the library lands in that list as a pinned
      row, newest (clause 23(g)), and the sheet comes back over the Auto
      DJ room — wherever it was opened from, quick config included — with
      the hand on that list's Add song when the keyboard pressed it.
    - **Only the room's server**: a seed is a path on the room's server —
      the DJ's while it is armed, Play from's while it is off — and the
      pick can browse only the session's library, so Add song is offered
      only while the two are one (clause 64). A pick that lands after the
      server changed anyway is taken and refused with "Pick a track on
      {server}"; a list that filled meanwhile refuses it without a word.
      Either way the sheet comes back.
    - **Esc and `[X]`** let the pick go and return the same way, once the
      pick is gone. Esc takes one layer at a time — whatever holds the
      keys first (a filter being typed, a standing filter, a Search query
      being edited), then the pick — while `[X]` cancels the pick
      outright, in one click.
    - Re-entering the Auto DJ room through the nav disarms a stale pick
      and opens the room without the sheet (sonic-path clause 13); the MP3
      Player tab lets the pick go where it stands, as every pick does; Stop
      leaves it, since the sheet works with the DJ off.
66. **Pins and their lifetime** (the owner's rule; *changed 2026-11-17,
    the owner's second answers, and for the TUI in round 3*):
    - **In Drift** a pin lasts until the user unpins or removes its row:
      through new queues and lanes, Stop and Start, a server switch and
      app restarts. The DJ's unpinned picks start over at each new lane
      (clause 10) and each new queue (clause 12); unpinned skips keep their
      clocks through those, but not through a restart.
    - **In the GUI, while Stay close holds the lists, every row it shows
      lasts** (the owner's "keep everything shown"): the pins, its one
      song — past eight rows where it came in over rows the DJ's server
      cannot use (clause 23(c)) — the picks frozen from Drift and the
      unpinned skips, their clocks stopped — through new queues and lanes,
      Stop and Start, a server switch and app restarts. Back in Drift only
      the user's own pins stay pinned (clause 23(c)).
    - **The TUI shows no Seeds sheet** *(new 2026-11-17, round 3: the
      owner's "Old rules there", decision 41)*, so its lists keep the rules
      from before the owner's second answers, and a pin nobody there could
      see or remove is never made: a song chosen to start Stay close is not
      pinned — the first real pick holds to it as the one song (clause
      23(c)); a new lane or a new queue in Stay close keeps More like's
      pins alone, Less like staying as it stands, its clocks stopped; the
      box lets the stored pins of both lists go; and the file keeps the
      pins, so an unpinned skip does not outlive a restart, and a file the
      GUI saved in Stay close opens in the TUI with its pins alone, still
      in Stay close. The pins carry over between the two. Drift is the same
      in both, the blend and its quick skip with it; so is a lane's look
      for Stay close's one song, past eight rows included, which in the
      TUI goes at the next new lane or queue with the other unpinned rows.
    - **The file.** `[player.dj]` keeps the rows beside the other DJ
      preferences — `more_like_pins` and `less_like_pins`, the names kept;
      a row its server (and peer), path, title, artist and album, the tags
      as the row knows them — a row that joined bare, as a song the remote
      queued does, takes them when they land (decision 44) — newest first,
      at most 8 a list, More like up to 16 where Stay close's one song came
      in past eight (decision 42). Saved in Drift it keeps the pins. Saved
      in Stay close by the GUI it keeps every row, with `pinned = false` on
      a row the user did not pin and, on an unpinned skip, `drift_ms`: the
      Drift time its clock had run when Stay close froze it, so its 30
      minutes resume where they stopped — the time the app was closed never
      counts (decision 36). A file that says Stay close loads frozen — in
      the GUI every row as it was saved, a More like of sixteen included;
      in the TUI the pins alone — and one that says Drift loads the pins
      alone. Past those caps, which only a hand edit passes, the oldest row
      whose stored pin is false goes first, a pin only when every row is
      one. A row without `pinned` is a pin: before the flag, the file kept
      nothing else. A row that does not read — a `pinned` that is not true
      or false, a `drift_ms` below zero — is skipped, never the file, and a
      row of a server the app no longer has is dropped at load. The GUI
      writes the file soon after what it keeps has moved, whatever moved it
      — a song's tags landing on a kept row among them (decisions 38, 44);
      the TUI writes it on the way out, as every setting.
    - **Clear pins when the queue is cleared** — the Seeds sheet's
      checkbox, kept in `[player.dj]`, off by default. Ticked, a new queue
      (clause 12) where the queue held a row lets every row drawn pinned
      go: in Drift the pins of both lists, the kept ones with them, and
      unpinned skips keep their clocks; in the GUI's Stay close every row,
      the mode staying, and the next real pick takes one song to start
      (clause 23(c)); in the TUI's, which draws none, the stored pins of
      both lists, Less like's unpinned skips staying, and the next real
      pick takes one song all the same (decision 41). A removal that
      empties the queue is a new lane, not a new queue: the box leaves it
      alone.
    - A server removed from the app drops its rows, pins included; a
      server renamed takes its rows along.

### Drawn by egui in the window

67. **In the GUI's desktop window egui draws quick config** (clause 58),
    part of the GUI's chrome with the bar it hangs from (transport-bar.md
    clauses 15–20), in the Studio look of the "Top Bar and Transport in
    egui" canvas's QuickConfig board, line by line from the cells' own
    view, so the two faces say the same words under the same conditions.
    It hangs as the kit hangs a list: the panel's fill in the accent's
    hairline with the canvas's shadow, as wide as the cells' popover (42
    cells from the 14th), rising from the split's rule with its foot 4
    points over the seek line and never over row 1, its shadow kept off
    the strip and the bar; rows 20 points tall, the gaps between groups 8
    and its padding 10, so its tallest case (four servers, sonic on)
    stands whole under the strip at the GUI's floor with key hints on. A
    line that would not stand whole inside the padding is left out, as the
    cells leave out a line past their popover.
    - **PLAY FROM** in the kit's small capitals, dim; each server the
      kit's round mark, chosen in the accent on the current one, and its
      name, a peer's behind `└`, the current one in the semibold; the mark
      and the name take the press, not the row.
    - **Sonic similarity**: the kit's tick box, on in the ok green, and its
      name take the press; its value at the row's end in its tone (the
      floor with its figure in the monospace, or why a session dropped
      it), the name cut short of the value rather than run under it. Its
      match bar under the name: the kit's quiet `−` and `+`, and a level
      segment for each of the cells' cells, lit in the accent up to the
      floor and in the hot accent while the keyboard's hand is on the bar
      or the pointer over its segments; `−` steps a point down, `+` a
      point up, a segment sets its floor; the ends brighten under the
      pointer.
    - **Seeds**, **Smart filters** and Play from's row to the sheet: the
      whole row takes the press; the name, the value in its tone ending 16
      points before the row's end, cut where the name leaves it short, and
      the kit's pointer there, dim; the name in the semibold and the
      pointer in the text colour under the pointer or the keyboard's hand.
    - **All Auto DJ settings** the amber text button with the kit's pointer
      8 points after its words; the hot accent and the semibold under the
      pointer.
    - A setting the server cannot do rests dim, its reason dim and whole
      at the row's end, and takes no hover, no hand and no press.

    The keyboard's hand is the view's, worn as the Admin canvas wears it
    (the raised fill and the accent's ring), never egui's focus; every key
    stays the cells'. A press works what the cells register there, on the
    press and once, a tap's too: a stop on its mark and words or its whole
    row, the bar's ends and segments their step and floor; nothing on the
    popover's frame off them, as the cells' guard; and anywhere else in
    the window Close, the bar's own controls, the split's chevron and the
    top strip among them, as the cells' catcher takes them. Nothing lights,
    shows the hand or takes a press where quick config does not hold the
    pointer: during an MP3 write, or under a layer the cells stack over it
    (a sheet, the Seeds sheet, the First song question, the quit's gate).
    Right presses and the wheel stay the grid's, which swallows them while
    it is up, as in cells. Opened by a press on the split's chevron, it
    shows the hand under a pointer that has not moved since, the press's
    release giving the chrome back where the pointer rests. On the Admin
    screen it floats over the room's egui face, which stays drawn beneath
    it and hears none of the pointer; on the Library the room stays drawn
    at rest (clause 58). `MSTREAM_WINDOW_CELLS=1`, a terminal and the
    frames before the window exists keep it in cells, as clause 58 tells
    it.

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

**The seeds' words (2026-11-17)** — the record's, from the phone's ARB
files on `claude/autodj-seeds` (uncommitted, base `c1393e12`), in all ten
locales; the pick's banners and the two tips lines are this surface's own,
since the record's Add song is a sheet of its own. Where a translation
names a list in running text it wears the locale's own quotes, as its
other sentences do. Gone with the regroup: `must_match` and `prefer` (the
record dropped autoDjMustMatch and autoDjPrefer) and `harmonic_hint`
(clause 45 — the record keeps autoDjHarmonicHint).

| String | Record key → `gui.dj.` |
|---|---|
| Filters · Match (drawn UPPERCASE, the room's headers) | autoDjFilters · autoDjMatch → `group_filters`, `group_match` |
| Learn from skips · Skipping a song will prevent similar songs from being selected | autoDjLearnFromSkips · autoDjLearnFromSkipsHint → `learn_skips`, `learn_skips_sub` |
| Needs a newer server | autoDjNeedsNewerServer → `needs_newer_server` (the key's new words; the first table's sentence is gone from the record) |
| Seeds | autoDjSeeds → `seeds` |
| Songs are rotated out regularly. Pin a song to keep it from being removed. · All songs are now pinned. No new songs will be added. | autoDjSeedsDriftHint · autoDjSeedsStayCloseHint → `seeds_drift_sub`, `seeds_stay_sub` |
| More like · Less like (drawn UPPERCASE, the sheet's headers) · Add song | autoDjMoreLike · autoDjLessLike · autoDjAddSong → `more_like`, `less_like`, `add_song` |
| More like this · Less like this · Remove | autoDjMoreLikeThis · autoDjLessLikeThis · mainRemove → `more_like_this`, `less_like_this`, `seed_remove` |
| Clear pins when the queue is cleared | autoDjClearPinsOnQueueClear → `clear_pins` |
| Drift · Stay close · Done | autoDjDrift · autoDjStayClose → `drift`, `stay_close`, kept; `gui.moods.done` |
| Pick a song for More like — click a track anywhere in the library · Esc cancels · Pick a song for Less like — click a track anywhere in the library · Esc cancels (this surface only) | `seeds_pick_more`, `seeds_pick_less` — the sonic pick's banners, each with its list's name |
| Tab ↑ ↓ move · Space press · m menu · Enter done · ↑ ↓ rows · Enter picks the song · Esc cancels the pick (this surface only) | `gui.tips.dj_seeds`, `gui.tips.dj_seed_pick` — the sheet's and the pick's tips lines |
| Pick a track on {server} (a pick refused) | `gui.discover.pick_other_server`, reused: the same meaning |

**The second answers (2026-11-17)** add no words: they change what the
lists do, not what the sheet says. The box's words now cover a play that
replaces the queue too, as the record's own note on
autoDjClearPinsOnQueueClear has it ("clearing the queue (or replacing it,
as playing an album does)").

**Round 3 (2026-11-17)** adds no words either, and changes none: its new
lines are the log's (clause 63), in English as every `[dj]` line is, and
the TUI's Stay on seed note — "picks stay close to the seeds as they
stand" — promises nothing its old rules (decision 41) make untrue.

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
- **The record's Add song sheet** (the canvas's A-AddSong: Up next,
  Skipped, Recently played, a search): the owner chose the library pick
  here (clause 65, decision 21), so there is no Skipped section either.
- **`lessLikeVectors`** (+ `lessLikeModelId`) **and negatives across
  servers**: a lane here picks from one server, and a Less like row of
  another server is listed but not sent (clause 23(a)).
- **Seed art**: the record's rows carry covers; the sheet's rows have
  none, as the A boards draw them.
- **The record's accessibility labels** for the pin, the row's menu and
  the mode (autoDjSeedPin, autoDjSeedMenu, autoDjSeedMode): the GUI has no
  screen reader to name them to; the tips line names the sheet's keys.

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
| One row per setting, a card of rows per group | Rows at the room's own width — 47 cells at a hundred columns, up to 56 where the room is wider, and past that the block centred in the room — under DIM UPPERCASE headers, FILTERS and MATCH since 2026-11-17 (MUST MATCH and PREFER before); a name, its value ending two cells before the last, `▸` or `▾` on the last. The keyboard's hand is the accent slab the row's width; the pointer brightens the name and the chevron; ↑ ↓ walk Preview, Start / Stop and the rows the server can do; Enter or a click opens |
| The value's colour (secondary · primary · warning) | DIM · OK · GOLD behind `!` |
| A greyed row with a reason, the chevron's room kept | DIM whole, the reason ending at the values' column, no chevron, no click |
| Start, and the lit Stop — the Auto DJ button's on look, never red | The room's one primary, a 3-row Rounded frame the rows' width: "Start ▸" in the accent, "Stop" in the OK colour; the kit's one-row slab on the title row where the room is short (clause 54) |
| A bottom sheet: a handle, the title, the value, a switch, Clear / Done | The kit modal the mood Filters already use (library-rooms clause 32): 66 cells, the title on the top border and the row's value after it in its tone, following the draft; `[✓] On` as the first row of a sheet that has a switch; the old room's help line inside; `clear` beside `[ Done ]` only while something is set; no `[X]`. Tab and the arrows walk its stops, Space presses, Enter is Done (it adds the word on Skip words' line; it clears on `clear`), Esc is Done too. The one sheet without the draft is the Seeds sheet (2026-11-17, clause 64): a press applies at once and Done only closes |
| Sliders (the match, the tolerance, the batch) | Bars in the volume widget's grammar, a cell a step: 25 cells a song (Batch), 20 a BPM (Beat matching), 20 an artist (cooldown), 25 at two points a cell (Sonic, 30–80); `-` `+` and ← → step; a click on a cell sets |
| Timer wheels (hours · minutes · seconds) for the two bounds | A typed length per bound, read to the second, with `-` `+` stepping fifteen seconds; ← → step too, so the field types at its end |
| Segmented buttons (Only \| Skip · Generated \| Tags \| Mixed · Drift \| Stay close · Strict \| Standard \| Loose) | The kit's side-by-side radios in the sheet's columns — Drift · Stay close on the Seeds sheet since 2026-11-17, where the Sonic sheet held them |
| The skip words' text field, with its add button | The kit's Text input card (three rows, 56 cells, the accent while it has the keyboard) and a tall `+` frame a cell to its right: DIM and inert with nothing typed, the accent with a word on the line, a click adding it as Enter does |
| Chips with a remove (the skip words) | Words with `[X]`, three cells apart, wrapping to four rows, right under the card; ↓ from the line reaches them, ← → walk, `x` removes |
| The First song sheet (four rows, a tap applies) | The kit's Dropdown under the row |
| The star row of the Rating sheet | Five stars a cell apart: a click sets one whole, a second click on it halves it; ← → by halves |
| Play from: a tick per server, folder switches under each ticked one | One sheet: the servers as one-ticked choices, a peer under its parent behind `└`; the folders of the rows' server as checkboxes under its name. A server just ticked shows no folders — they are its own to say, once it is the rows' server |
| The start sheet (Random · Choose · Recently played with its dropdown · Remember) | The kit modal under the row's own name, "First song", on its border: the three answers, a blank row, `[ ] Remember`; Recently played's list hangs from its row, twenty cells in, over the box's row and past the frame, a song a row — title, then the artist DIM |
| The empty queue's Recently played chip and its menu | A third text button, `Recently played ▾`, whose list hangs under it at the panel's width |
| The split Auto DJ button (the word · a chevron) and its quick settings sheet | One rounded frame in the bar with a rule between its halves — `│ auto-dj │ ▴ │` — the transport group moved right by the chevron's four cells; the popover 42 cells wide from the rule's column, its last row the one over the bar; `▾` while it is up. No key opens it yet (decision 18) |
| A spinner while a server connects | Nothing: a session here is one server, and its tunnel's state is the header's to show |
| Already shared | `dj.rs` (reshaped per decisions 1–3), `maybe_autodj` (grown clause 13's guards), `consume_dj` (a batch, the badge, the epoch), `autodj_sample` (Preview), the genre picker, `Reach` and `reach()` for a peer's or a tunnel server's request |
| **The seeds (2026-11-17)** | *the record's Seeds page, option A* |
| The settings screen's sections, Filters and Match | The room's DIM UPPERCASE headers FILTERS and MATCH; Artist cooldown, which the record has not, joins the session rows (E-AutoDJ) |
| The Learn from skips tile, its subtitle under it; the Seeds tile | Two sub-rows under Sonic similarity, their names two cells in: `[✓]` / `[ ]` at the values' edge with the line DIM four cells in; `Seeds ▸` with no value. DIM and inert in place while sonic is off (E-SonicOff) |
| The Seeds page, a screen of its own | The Seeds sheet over the room (A-Drift, A-StayClose) — the mood Filters' shell, `Seeds` on its border, no draft: a press applies, Done only closes. The canvas's B (a nav room of its own, the lists side by side) and C (a panel in the queue's column) were not chosen |
| Drift \| Stay close and its hint | The kit's side-by-side radios at the sheet's top, the mode's line DIM under them on two rows kept for it |
| The pin icon, filled or outlined | `◉` OK for the user's pin, `◌` DIM unpinned; every pin `◉` DIM and inert in Stay close; `*` / `o` on a legacy console |
| A row's ⋮ menu | `[⋯]` (`[.]` on a legacy console) at the row's end, opening the kit's context list one row under the row, its right border on the `]`: the move, then Remove in the danger colour; a move a full list cannot take DIM and passed by the cursor |
| The dimmed Less like card and its reason | Less like's rows DIM, "Needs a newer server" DIM on its header row, the `[⋯]` live; its pins and Add song inert |
| Add song's sheet (Up next · Skipped · Recently played · a search) | The library pick (clause 65): `+ Add song` arms the capture for its list — a `Capture::Seeds` beside `DjSeed` and `TasteSeed`, its banner the row above the room — and the sheet comes back when the track lands or the pick is let go |
| The "Clear pins when the queue is cleared" checkbox (after the boards) | The kit's checkbox in a modal, on a row of its own above Done: the sheet a row taller than A-Drift, so the longest locale (de, 56 cells) fits |
| Quick config's Drift \| Stay close | Its match bar four cells in and `Seeds ▸` under the sonic row (E-Quick), hidden while sonic is off |

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
sonic_anchor = "rolling"     # | "locked" — Drift · Stay close (2026-11-17)
learn_from_skips = false     # Learn from skips (clause 23(d), 2026-11-17)
clear_pins_on_queue_clear = false  # the Seeds sheet's checkbox (clause 66)
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

# The seed lists' kept rows (clause 66, 2026-11-17), newest first, at most
# 8 a list (More like up to 16 past Stay close's one song, decision 42) —
# the user's own pins and, saved in Stay close by the GUI, every row the
# sheet shows (the owner's second answers; the TUI keeps the pins alone,
# decision 41); a row that does not read is skipped
[[player.dj.more_like_pins]]
server = "http://…"
path = "Music/Boukmanflow/SkyWalk.mp3"   # the wire form: no leading '/'
title = "SkyWalk"
artist = "Boukmanflow"
# album = "…" and, for a peer's row, peer = 3 — where known

[[player.dj.less_like_pins]]
server = "http://…"
path = "Music/Color Out/Heartbeat.mp3"
title = "Heartbeat"
artist = "Color Out"

# saved in Stay close (sonic_anchor = "locked") by the GUI: a skip the user
# did not pin
[[player.dj.less_like_pins]]
server = "http://…"
path = "Music/…"
pinned = false        # absent is a pin
drift_ms = 300000     # 5 of its 30 minutes run when Stay close froze it

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
today. A file from before 2026-11-17 has no pins and no Learn: both lists
start empty, Learn off, the box unticked, and its `sonic_anchor` reads as
the mode — rolling as Drift, locked as Stay close. A row without `pinned`
— every row written before the owner's second answers — is a pin. A list
past its cap — More like past 16 in the GUI's Stay close, past 8
elsewhere, and Less like past 8 — is a hand edit's: it loses its oldest
unpinned rows at load, a pin only when every row is one (decision 42).

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
line in its row, and every reason whole; Skip words' card and its `+`
(DIM and inert, then lit, a click adding the word), the sheet at its
tallest whole at the floor; the block filling a room of up to 56 cells
and centred in a wider one, its clicks and First song's list with it.
The TUI: `A` with no Play from
on screen starting the DJ where it stands, and the tab's first row saying
where Enter starts it in either shell.

**The seeds (2026-11-17).** The model (`src/tui/app/seeds.rs`, pure, on
the fake clock): a key in wire form; a pick newest, the oldest unpinned
leaving past five, and the cap what the pins leave of eight; a Less like
song never pushed, and nothing while frozen; Stay close's one song past
the freeze, never evicting; a quick skip out of More like and, with Learn,
into Less like — a full list evicting its oldest unpinned row, eight pins
taking none; an unpinned skip's thirty minutes (29:59 kept, 30:00 gone)
and a pin's forever; the freeze pausing the clock and Drift taking it up,
the pins locked untouched and More like trimmed after; a new lane and the
ticked clear; the user's own rows, pins, moves and removals in either
mode; the wire's eight newest of one server; the stored pins through the
file, a bad row let go; a removed or renamed server; the quick skip's
thresholds (999 ms, 1 s, 30 s, half a length, repeat-one, a wrap, the
last row) and the re-roll's rows. `stats`: a play at thirty seconds or
half a known length. `dj.rs`: `lessLike` apart from the pool — kept
without `similarTo` and `minSimilarity`, eight at most — and never on the
opener; the settings' round trip with Learn and the box. `types.rs`:
`features.autoDj` read as a number and anything else as none; the
request's `lessLike`, and the echo read softly. Config: the pins beside an
unknown `[player.dj]` key through a save, a bad row skipped, a rename
followed and a removal taking them. Worker: a dead seed out of the pool,
the last taking `minSimilarity` with it; a pick finding its dead seeds and
keeping the rest of its pool and its lane's sonic, a lone unanalysed one
included. App: a pick joining
More like as it starts — once, this lane's own, a blend into it included,
the top-up's turn already holding it; a seeded start's first song in
either mode; Drift's fallback, never a Less like song; Stay close's one
song on a real turn and Preview's look at it; every way to the mode
freezing and thawing the lists; the negatives' gate and the reason's; the
quick skip through Next and each Next that is not one; a re-skip that
changes nothing the server reads; the re-roll's replacements, no song
landing twice, its appends with no playing row or under shuffle, and a
stale pick asked again; new queues and the box; sonic off keeping the
seeds; the prefs (pins, Learn and the box kept, skips not); a removed or
renamed server and the pins of one the app no longer has; a server
removal that empties the queue starting a new lane, its pick out dropped
and the box untouched; Add song's pick and its refusal; the expiry on the
tick and not in Stay close. GUI
(`src/gui/dj/tests.rs`): the room cell for cell (E-AutoDJ) and with sonic
off (E-SonicOff), its sub-rows inert and passed by the keyboard; Learn's
reason only once a probe said so; the floor's order and the headers'
fold; the Sonic sheet without the radios (E-Sonic); quick config's bar and
Seeds row (E-Quick), hidden with sonic off; the Seeds sheet cell for cell
(A-Drift, A-StayClose), its pins, its menu where it hangs and flips, its
disabled verbs and Add song, the checkbox, its keys and its scroll, a row
going under the hand; the pick's doors — the banner, a pick landing, a
pick refused, Esc a layer at a time, `[X]`, the nav's re-entry, home by
the screens' own door; Play from's server asked again after a Stop by `A`
or the bar's word and at a reconnect under the room; and every locale's
words whole. `src/gui/device.rs`: the MP3 Player tab letting the pick go
where it stands.

**The second answers (2026-11-17).** The model: the song a new queue
brings in newest beside the pins, the oldest unpinned leaving, eight pins
taking none; the first pick's rule and the chosen song's, input by input;
a new lane and the ticked box in each mode — in Drift the DJ's own rows
going at the one and the pins at the other, in Stay close every row
staying as it stood and the box emptying both lists, the freeze standing;
the file in Stay close — every row with its pin, a skip's five minutes as
`drift_ms`, every row back frozen from the launch and round again, More
like trimmed to five at the thaw, the skip kept at 24:59 after it and gone
at 25:00 — and the same file in Drift, the pin alone; a re-skip and an
unpin restarting a clock whole, the unpin from a clock that had run; the
fingerprint of what the file keeps, moved by a pin, Add song, the freeze
and Stay close's one song, and not by a Drift pick, a skip or time; Stay
close's one song refused by a full list of another server's rows as of
this one's, and read the same for Preview *(round 3: a ninth row past
eight of another server's, sixteen rows refusing it; this server's rows
are the App's to refuse)*.
Config: `pinned = false` and `drift_ms` through a save beside a pin
written as before, a row without the flag a pin, a junk flag or a clock
below zero costing its row and not the file. The App: the blend at a
lane's first pick after a replacing play, Stop and Start, a launch, a
move of the DJ and a tunnel's owed pick, with sonic off too, and not at
the lane's next; never in Stay close, never a listed song, another
server's row or the lane's own pick — one out at a replacing play landing
after the new rows and joining as it starts, so nothing blends *(round 3:
the row the play started blends instead)* — and not
staying past eight pins; Preview showing it, keeping nothing and leaving
the first pick to a real one; nine plays over the eight places a play
replaces the queue, each a new queue without a new lane, the box ticked
at one of them, and an empty queue replaced letting nothing go; a lane
that let its pool go keeping it gone through one; Stay close keeping
every row through a clear, Stop and Start and a restart, a carried row
holding the next queue's lane, its one song outliving a clear, its box
emptying it at a clear and at a replacing play and the next pick taking
one song; a chosen first song pinned in Stay close by Choose and by
Recently played, Random's not, none past eight rows drawn pinned, a Less
like song moving over, sonic off alike, and Drift's first song unpinned
as before; a move of the DJ in Stay close onto eight rows of the old
server — no song taken and none evicted, no positives on the pick or in
Preview, the log's line once and again after a new queue or a switch
into Stay close, the song taken once a row goes. The GUI
(`src/gui/mod.rs`): what the file keeps saved soon
when the engine moves it — a pin, Stay close's one song, the box at a
replacing play — and not for a Drift pick.

**Round 3 (2026-11-17).** Four of the second answers' checks now pin the
new rules: the model's full list — eight of another server's rows —
takes the one song as a ninth row, and only sixteen rows refuse it, a
list of this server's rows being the App's to refuse by its usable rows;
eight rows of the old server take the new lane's one song as a ninth
row; the refusal and its once-a-look line are sixteen rows'; and a
replacing play gives the pool another go. The review's fixes changed two
more: a pick out at a replacing play — the lane's own, and a top-up under
the box — now finds the row the play started opening the new queue, the
late pick joining beside it. The model: the blend skip's
rule input by input — the user's own blended song unpinned in More like,
its key with or without the slash, no blend, another song, a pin since and
a DJ pick — and the pass-over's paths by server, a peer's own apart; a
chosen first song's add, each of its four conditions alone; the GUI's
rules and the TUI's side by side — the TUI's file keeping the pins, its
fingerprint blind to Stay close's one song, its frozen new lane keeping
More like's pins and the skip with its clock, its box letting the stored
pins go, a file the GUI's Stay close saved read there frozen with its pins
alone — and Drift the same in both; Stay close's one song past eight rows
of another server's, in front, nothing removed, sent alone, the list full
all the same, through the file and back nine, the TUI's and Drift's loads
the pins, the thaw's caps after; sixteen rows refusing it until a row
goes; a load of seventeen keeping sixteen, its oldest unpinned gone, Less
like eight, the pins alone at eight in Drift and the TUI, a duplicate
taking no room; a pick past eight rows sending the eight pins, never a pin
left out for the stand-in, a frozen pick the oldest unpinned staying home,
the refused rows out before the cut; a song's tags filling its rows in both
lists, not another server's and not a title it has, moving the fingerprint
for a pin and not for a Drift pick. `dj.rs`: a song passed over by its
path in either form, no path none, and never sent. The worker: a
passed-over answer asked again with the fresh cursor, five answers kept
out in full ending in the last one but for it, and the answer whole when
nothing else is left. The startups: the TUI's start and the desktop's,
one loading the pins and the other every row Stay close kept. The App: a
quick skip of the blended song leaving More like with Learn on and
learning nothing — no Less like row, no re-roll, no negative — its pass-over
on the next pick; a blended DJ pick of an earlier lane learned and not
passed over; the pass-over in Preview and through a replacing play, whose
start blends a song of its own, never on the opener, gone at Stop and
Start, and both marks following a re-addressed server; a pinned blend,
Stay close and a seeded start's first song unchanged by a quick skip; a
cleared queue's last track ending or handing over, and an emptied
removal's ending, asking no pick, the next queue's first pick the new
lane's, and the track left behind ending or handing over while a
replacing play's row waits for its tunnel, on one row and on two, the
user's choice still waiting and the first pick unspent; a pick out at a
replacing play landing after the new rows while the row the play started
opens the new queue — Drift's blend beside the late pick, the TUI's Stay
close holding to it as its one song — played through each track's end;
the TUI's Stay close by its old rules beside the GUI's — a chosen
first song unpinned as the one song, a pin holding the lane with nothing
added, Stop and Start, a clear, a replacing play and a restart keeping
More like's pins alone and the skip with its clock, its one song not
outliving a clear, the box letting the pins go and the next queue taking
one song, the GUI's file opened with its pins alone; the ninth row after a
move — the pick and Preview carrying it, Add song refused, a new queue
holding to it, the file keeping all nine, the GUI loading them and the TUI
the pins, a second move a tenth, the thaw's caps — and the TUI taking it
too; sixteen rows holding to nothing, said once a look, and a row removed
taking the song; a list its server refused taking the song playing as a
fourth row and as a ninth, the refused rows still listed, Preview agreeing;
the pins sent before the stand-in; a refused song no stand-in, the next
song coming in; one song a lane though the server refuses it; a list the
user emptied staying empty, and one the user left holding refused rows
alone — by a removal or a move to Less like — taking nothing, Preview
agreeing, where a refusal of every row, or of a song added since, still
takes the song playing; a replacing play giving a degraded lane its
pool back — the gold reason gone, the next body carrying the pool, still
the lane — and Clear queue's new lane never without it; a pick out before
the retry landing without taking it back, and a turn after the retry
saying so again. The GUI: the Seeds sheet listing a ninth row with Add
song resting (`src/gui/dj/tests.rs`); its App showing the sheet
(`src/gui/mod.rs`); and a remote `/play`'s bare seed row taking its tags
when the block lands, a pin and Stay close's one song saved soon with them
(`src/gui/control.rs`). Each new rule's tests fail with its code taken
out: two batches of mutations, twenty-two in all, failed every test
written for them; the review's three fixes' tests failed, each half of
them, on the code before the fix.

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
    default). The record has no such row. *Amended 2026-11-17*: a session
    row, under Batch, where E-AutoDJ draws it (clause 57).
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

**2026-11-17 — the seeds.** The owner reviewed the canvas "Auto DJ Seeds ·
Desktop", picked option A for the Seeds page, and asked for the library
pick the sonic path uses in place of A-AddSong; the rest is settled as the
build's plan settled it, and stays easy to turn:

20. **Option A**: the Seeds sheet over the room, in the mood Filters'
    shell (A-Drift, A-StayClose) — not B, a nav room of its own with the
    lists side by side, nor C, a panel in the queue's column.
21. **The library pick, not A-AddSong** (the owner's words): Add song
    arms the capture the sonic path and the opener use, for its list
    (clause 65); the record's sheet — Up next, Skipped, Recently played, a
    search — is not built.
22. **The row's menu is the kit's context list**, as A-Drift draws it:
    hung one row under its row, its right edge on `[⋯]`; a move a full
    list cannot take DIM; Remove in the danger colour at rest — each a
    logged exception to the kit (ui-kit.md, Context list).
23. **Quick config hides the match bar and the Seeds row while sonic is
    off** (E-Quick drew sonic on), as it hid Drift · Stay close, and as
    the record does.
24. **Harmonic mixing's line is dropped**, and the floor's order is
    Learn's line, the frame, the two blank rows, then the blanks around
    the frame (clause 54): the room needs all 23 rows at 100×30, and with
    Play from the frame is worth more than a second blank row.
25. **A quick skip is a Next that moves** (clause 23(d)): this player's
    Next under repeat-one moves to the next row, so repeat-one is left out
    by hand; the heard time and the length are the playing row's own play
    session's (play-reporting clause 1), so a row parked for its tunnel,
    which has none, is never quick.
26. **"Counts as a play" mirrors the server's defaults** — 30 s, or half a
    known length (`decideCounted`) — since the player cannot see the
    operator's `stats` config; the server's own word still decides the
    play itself.
27. **A new queue for the seeds** *(rewritten 2026-11-17, the owner's
    second answers; before, Clear queue alone, and an open question)*:
    the clear — the header's `clear`, `C`, and the control face's `/play`
    and `/queue/clear`, which go through it — and every play that replaces
    the queue (clause 12). The owner's question was about playing
    something new, "an album, a playlist", so a replacing play counts, as
    every play that replaces its queue does on the phone. On this player
    it is still no new lane (clause 10's triggers): its cursor, its
    Camelot anchor, the songs its picks pass over (decision 39) and a pick
    already out are kept — the owner left the rest of the lane out of the
    question; that pick lands after the new rows, and the new queue's
    start is settled before it can become it (decision 35, *round 3*) —
    so the seeds' own state starts over: the DJ's unpinned
    picks, the lane's first pick (decision 35) with the song it blended in,
    and Stay close's one song. A pool it let go (clause 30) was kept too,
    until the owner asked for it back *(amended 2026-11-17, round 3: the
    owner's "Try again", decision 43)*: a new queue gives it another go.
    The box counts only a queue that held a row. Ticked, it lets go every
    row drawn pinned: in Drift the pins and nothing else, unpinned skips
    keeping their 30-minute clocks as the owner's default says; in the
    GUI's Stay close, where every row is drawn pinned, all of them (clause
    66); in the TUI's, which draws none, the stored pins (decision 41).
28. **Add song only while the library on screen is the room's server**
    (clause 65): a pick can browse only the session's library, so with
    the DJ or Play from on another server every pick would be refused —
    the control rests instead, with no reason (the room's Play from row
    names the server). The refusal stays as a guard for a server that
    changes mid-pick.
29. **A dead seed costs one pick, not the lane** (clause 31): every seed
    found dead — a lone seed's 404 or "analyzed" body, or a probe's — is
    left out of the lane's pools, all of them included, and the lane lets
    its pool go only when no seed is found to blame — a pinned dead or
    unanalysed row would otherwise silence every lane's sonic.
30. **The negatives' gate** (clause 23(e)): never on the opener — clause
    3's library filters and nothing else — and "Needs a newer server" only
    once a probe has said `autoDj` 0. Before a probe has answered, the
    controls that need negatives rest without a reason, so a current
    server that is slow or out of reach never wears one.
31. **A stale pick is asked again** (clause 11): its songs, cursor and
    failure go and its degrade note stays — the worker has already let the
    pool go, and the second ask would not say so.
32. **Unpinned skips are not kept across a restart** — the phone's call:
    the owner promised restarts to the pins. *Amended 2026-11-17, the
    owner's second answers*: in Drift. While Stay close holds the lists
    every row is kept, an unpinned skip with its clock paused (clause 66,
    decision 36). *Amended again in round 3*: in the GUI; the TUI keeps
    the first rule in either mode (decision 41).
33. **The pick comes home once it is gone** (clause 65): every door — an
    answer, Esc, `[X]` — returns to the room and reopens the sheet only
    after the pick is let go, so an Esc that ended a filter or a Search
    query edit leaves the pick armed where it was.
34. **The checkbox has a row of its own**, above Done, and "Needs a newer
    server" stands on LESS LIKE's header row (clause 64): the boards drew
    neither, and Done's row would leave 51 cells, which the German, French
    and Italian words do not fit.

**2026-11-17, later — the owner's second answers.** Three questions were
put to the owner about the phone and this player together, and answered
the same day — blend in new music, keep everything shown, add the chosen
song (the deviations log has the words) — and a fourth closed with them:
a play that replaces the queue is a new queue for the seeds (decision 27,
rewritten). The phone's plan for them is the reference; where this player
differs, the build settled it so:

35. **The lane's first pick** (clause 15) is its first REAL turn — a
    top-up, Start's or a move's own turn, a seeded start's followers, an
    owed turn once it goes — never the opener, which carries no seeds
    (clause 3), nor Preview. Every new lane makes the next real turn the
    first again, so does a new queue (clause 12), and a launch starts with
    one due. A turn that cannot be asked — the DJ's tunnel dialling
    (clause 35) — leaves it due; one that is asked spends it, whether or
    not a song joined. The song is the queue's current row, on the DJ's
    server alone, since a lane picks from that one server. Preview shows
    the song as the real pick would hold it and keeps nothing, as it shows
    Stay close's one song (clause 23(c)). A seeded start's first song is
    More like's first row in Drift already, so its followers' pick finds
    it there. *Amended in round 3, found in review*: a play that replaces
    the queue while a real pick is out composes the new queue's first pick
    at once, on the row it started, and sends nothing — the pick out lands
    after the new rows (decision 27), and the queue's end would be that
    late pick, which joins More like as a pick, so nothing of the new music
    would blend and Stay close would hold to the late pick. It stamps no
    turn, so the pick out stays one composed before the retry (decision
    43); as a real pick composed there would, it seeds a Camelot anchor
    the lane has none of yet from the song playing (clause 22).
36. **The pause through a restart is each skip's own Drift time** (clause
    66). The phone keeps the set's frozen moment and each row's arrival on
    the wall clock; this player's clock is monotonic and per run, so a stored
    moment would mean nothing to the next one. The file keeps the
    difference instead — `drift_ms` on an unpinned Less like row, at most
    30 minutes — and no moment for the set: `sonic_anchor = "locked"`
    beside the rows says they were frozen. How a row came (a pick, the
    opener, Stay close's one song, a skip) is not kept either; nothing on
    screen tells them apart. `pinned` is written only when false, so a
    pin's table reads as it did before the flag, and a value that does not
    read costs its row, as any bad row here does, where the phone reads it
    as a pin.
37. **"Chosen" is the user's own song** (clause 15): Choose's or Recently
    played's, never Random's, which the DJ picked. The add is Add song's —
    pinned, newest, a song in Less like moving over, and only where More
    like can take a row (in Stay close, fewer than eight) — made before
    the song plays and before the followers are asked, so their first pick
    carries it; sonic on or off. In Drift nothing changes: the first song
    joins unpinned, as the opener. *Amended in round 3*: where a Seeds
    sheet shows the pin, the GUI; in the TUI the first real pick holds to
    the song as the one song when nothing else is, as before (decision 41).
38. **What the file keeps is saved soon** (clause 66): the engine now moves
    the kept rows on its own — Stay close's one song, a chosen first song,
    the box at a new queue — so the GUI compares a fingerprint of what the
    file keeps once a pass and saves soon when it moved. A Drift pick,
    which the file does not keep, writes nothing. The TUI writes its
    settings on the way out, as before.

**2026-11-17, round 3 — the owner's afternoon answers.** Five more
questions were answered the same afternoon — two put to the owner about the
phone and this player together (a quick skip of the blended song; Stay
close's one song when More like is full), three about this player alone,
from its second answers' review (the terminal's rules, a new queue's
retry, a remote song's tags); the deviations log has the words. A check of
this player for the phone's clear race found a door of its own, and the
phone's review of the ninth row brought three rules more, built here too.
The phone's reviewed builds are the reference for the two asked of both;
where this player differs, the build settled it so:

39. **A quick skip of the blended song** (clauses 15, 23(d), 26; the
    owner's "Take it out"). The lane keeps the song its start blended into
    More like, by its key — set only when a real pick's blend put it there,
    never by Preview — and a Next on it is judged as a DJ pick's is, by
    the same thresholds, while More like holds it unpinned and the row is
    the user's own. The guard is the row's own DJ flag, never "it was
    blended": the phone's first cut switched learning off for every
    blended row, and its review found DJ picks among them — a row an
    earlier lane or run left at the queue's end, or this lane's DJ serving
    the song again — which keep a pick's rule here, Learn included. The
    user's song leaves More like and teaches nothing; the set of negatives
    is unchanged, so neither a re-roll (clause 23(f)) nor a negative
    follows. Out of More like and never served — no cursor id keeps it out
    — it could come straight back, so the lane's picks pass over it: the
    ask carries its path to the worker, never onto the wire, and the
    worker's filter keeps it out as it keeps a keyword hit, the last answer
    taken whole leaving it out while anything else is left (clause 26) —
    the phone's single-server filter; this player has no fan-out to guard.
    The App's own check still drops a song already queued. Real picks and
    Preview carry the pass-over; the opener, which opens a new session,
    does not. The pass-over lasts the lane — through a play that replaces
    the queue, which is no lane edge here (decision 27) where on the phone
    it is one — so a skipped blend stays out longer than a skipped DJ
    pick, which the server's cursor keeps out only while it is among its
    last 50 (clause 24); the blend mark goes with each new queue, whose
    start may blend a song of its own. Both follow a server re-addressed
    meanwhile. A
    seeded start's first song is never the blend: in Drift it is in More
    like already when the lane's first pick comes, so a quick skip of a
    chosen opener still does nothing and Random's still takes a pick's
    rule — the owner's "the seeded start's opener stays as built"; in Stay
    close the lists are frozen, and a quick skip changes nothing.
40. **An empty queue has no end** (clause 12; the phone's clear race,
    checked here). On the phone a clear reset the lane at once and emptied
    the queue only after awaiting the player's stop, so a pick could open
    the new lane on a row being thrown away. Here every whole-queue reset
    — Clear queue, a play that replaces the queue, a removal that empties
    it, a seeded start — drops or replaces the rows and starts the lane,
    or its first pick, over in one call on the App's thread, before any
    event is read, and the blend reads the queue's current row, which a
    clear leaves none of: that window cannot open. The teardown had a door
    of its own: the clear's Stop is answered later, and until its status
    lands the status still names the cleared track, so that track's end —
    it ran out as the user cleared — or a crossfade the engine had
    committed to passed as the queue's end, asked the new lane's first
    pick for the emptied queue, spent its blend, and played the answer
    there. A track's end or
    hand-over with no queue left is now dropped, like the end of a track
    the player has moved past; the armed DJ waits for the next queue, whose
    first pick is the new lane's. One guard covers Clear queue, the
    remote's `/queue/clear` and a removal that empties the queue. A Next
    pressed on an empty armed queue still asks a pick: that one is the
    user's own. *Found in review*: a play onto a row that waits for its
    tunnel (multi-server clause 37) — one that replaces the queue, the
    remote's `/play` among them — sends a Stop and no Play, so the status
    still names the track left behind, and that track's end or hand-over
    passed too: it skipped the row the user chose, or, on the queue's last
    row, asked the lane's first pick with no song to blend, whose answer
    then played over the waiting row. Nothing the App started plays while
    a row waits, so a track's end or hand-over then is dropped as well.
41. **The terminal keeps the old rules** (clauses 10, 12, 15, 23(c), 66;
    the owner's "Old rules there"). The owner's second answers for the
    lists hold where a Seeds sheet shows them: the GUI. Each shell says
    whether it shows one (`App.dj_shows_seeds`) before its file is read,
    since a load already made cannot be mended — the desktop builds its
    App through a start of its own (`gui::desktop_app`), and the GUI says
    so again as it opens; the TUI, the web build and the replay harness
    show none. Without a sheet, a song chosen to start Stay close is not
    pinned — the first real pick holds to it as the one song when nothing
    else is (clause 23(c)); a new lane or a new queue in Stay close keeps
    More like's pins alone; and the file keeps the pins, so a file the
    GUI's Stay close saved opens there with its pins alone, still frozen,
    and the TUI's own save keeps those alone — opening the terminal lets go
    of the unpinned rows desktop Stay close kept, as the owner's answer
    says. Two parts of "old rules there" are readings, told to the owner and
    each one predicate from the other: the box lets the stored pins of
    both lists go, frozen or not, unpinned rows staying, as before the
    second answers; and an unpinned skip Stay close froze stays through a
    new queue with its clock stopped, since a new lane never touched Less
    like, and does not outlive a restart. The same in both shells: the
    pins, Drift, the blend and decision 39's quick skip, a replacing play
    as a new queue, decisions 40, 43 and 44, and the ninth row (decision
    42), which in the TUI goes at the next new lane or queue with the other
    unpinned rows.
42. **Stay close's one song past eight** (clauses 23(a), 23(c), 31, 64,
    66; the owner's "Let it in as a 9th row", and the phone's review of
    it). The song is asked for only by a lane with nothing to stay close
    to — no More like row on the DJ's server that the server has not
    refused in this lane — and comes in front, nothing evicted. A move
    leaves that after eight of the old server's rows, the owner's case;
    and, as the phone builds it, so does a list whose every row the server
    refused (the owner is told). The refusals are the worker's memory of
    the running lane (clause 31), never a Preview's, read by the App as it
    composes; the lists still never store them. The phone's review of the
    plain rule found two holes and widened it once, and all three are
    built here:
    - **Sixteen at most** (the phone's `maxStoredPerList`): a server that
      kept refusing what each lane took in would grow the list by one a
      lane without end, and a restart's load would drop the oldest rows,
      usually the user's pins. The song stops at sixteen rows, and a More
      like the GUI's Stay close kept loads sixteen — all the live list can
      hold; past that a hand-edited file loses its oldest unpinned rows
      first, a pin only when every row is one. Less like loads eight, as the phone's
      merge keeps it: it never passes eight in a run. At sixteen the
      second answers' refusal stands, its line reworded: no positives,
      Preview showing none, one `[dj]` line each time the rule looks.
    - **No pin left out** (the request cut, the phone's `usableSeeds`):
      rows refused in one lane are usable in the next, so a lane can hold
      more than eight rows it can send. Past eight, the oldest row whose
      stored pin is false stays home first, the oldest of all only when
      every one is pinned — and the refused rows leave before the cut, or
      eight refused pins would leave the one usable song out. The wire's
      cap is unchanged.
    - **A list the server refused** (the phone's `stayCloseReleases`): a
      lane held to rows its server refuses afterwards, before it took a
      song of its own, looks again at its next real pick, Preview reading
      the same — never after its own song, one a lane even when the server
      refuses that one too, and never for the user's own edits, which the
      phone's release never runs for: a list the user emptied holds no row
      on that server, and when the user's removal, or move to Less like,
      takes the last row the lane could use, rows refused before still
      listed, the lane notes it (`DjLane.stay_emptied`) and does not look
      again until a real pick finds a usable row — a song added or moved
      in — whose refusal then releases it *(found in review: the release
      read the list alone and took the song playing)*. A song the server
      refused in this lane is no stand-in: the lane keeps looking, and the
      next song that plays comes in.

    From eight rows on More like is full as ever — Add song, a move into
    it and a chosen first song find no room — and the thaw trims it with
    Drift's caps. Drift is unchanged: it still sends rows its lane found
    refused, which the worker leaves out (clause 31); the phone falls back
    to the song playing there, which was not asked here. And the phone
    judges the release when a refusal is found, this player when the next
    pick is composed, so rows the user removes in between leave a list the
    user emptied, which stays empty where the phone would take a song. The
    other order agrees: the usable rows the user takes out after a refusal
    the lane outlived release nothing on either.
43. **A replacing play retries sonic matching** (clauses 12, 30, 43; the
    owner's "Try again"). A new queue for the seeds ends in the retry a
    More like edit makes (the record's `retrySonic`): the worker gives the
    lane its pool back and the room's gold reason goes. The rest of the
    lane stands — its epoch, its cursor, its Camelot anchor and a pick
    already out (decision 27) — and a seed found dead stays dead (decision
    29). Clear queue needs nothing more: it starts a new lane, which never
    let its pool go. A pick already out keeps its lane and lands; composed
    before the retry and degraded after it, it would let the pool go again
    under the same epoch, and its note would bring the reason back — the
    very thing the owner asked to undo. So each retry moves the pool on a
    generation (`DjLane.pool_gen`), each turn carries the one it went out
    under, and an answer from an older one says no note, sets no reason
    and gives the worker's lane its pool back again; its songs land as
    before. A turn composed after the retry degrades as before. The More
    like edits and the sonic settings had the same gap, and the
    generation closes it there too. The phone never had it: its replacing
    play is a lane edge, where a pick out is dropped.
44. **A song's tags fill its seed rows** (clause 66; the owner's "Fill the
    seed rows"). A details fetch remembers the server it asked beside the
    rating clock, and when the block lands every row with that song's key,
    in either list, takes the title, artist and album it lacks — the
    queue's own fill rule — a value it has staying. The album comes with
    the two the owner named, since the file keeps it. "Any set held aside"
    has nothing here to reach: the lists are the App's alone, the Seeds
    sheet has no draft, and Preview's copy lives inside one call. A block
    with no ask recorded fills no row, and two asks for one path from two
    servers keep the later one's server — the ambiguity the rating clock
    already accepts. The GUI's fingerprint of what the file keeps reads
    the tags, so a pin's, and those of a row Stay close keeps, are written
    soon (decision 38); the TUI writes them on the way out.
45. **A remote clear or play gets past an open picker** (clauses 10, 12,
    66; the owner's go-ahead, 2026-11-18, PR #85). The control face's
    `/queue/clear` and `/play` clear through `App::clear_queue`, which
    empties the queue without handing the clear to an open mode, then
    settles as every action does; the `C` key's arm and the direct clear
    share `empty_the_queue`. The `C` key still goes through the mode, which
    takes it. So a remote clear is a real clear whatever is open — the
    DJ's opening question, the room's Genres sheet, Now Playing's sources
    picker, genre list or skip-word field — and a remote play replaces the
    queue: a new queue for the seeds (decision 27), whose pins go with
    Clear pins when the queue is cleared ticked (clause 66). The open mode
    stays as it was: the remote changed the queue, it did not answer the
    mode. A sheet saves its draft however it closes, a closed field would
    drop what was typed, and the opening question's answers already cope
    with a queue that filled while it was up. The remote's other commands
    (`/pause`, `/resume`, `/next`, `/previous`, `/loop`) still go through
    an open mode — a session of their own.

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
- **2026-11-10 — Two changes on seeing it in the window** (the owner's
  calls, the same day, on the windowed build): **Skip words' line is the
  kit's text input** — a 3-row card — **with a `+` beside it**, so there
  is a way to add a word by a click; the canvas drew a bare line and
  Enter alone (clause 49). And **the block is centred and may grow**: the
  canvas drew one width, 47 cells, at the room's left, which left a wide
  window lopsided — the rows now take up to a fifth more (56) and the
  block stands in the middle of whatever is left (clause 41). At a
  hundred columns with the queue open the room is the canvas's still,
  cell for cell.
- **2026-11-17 — The seeds** (re-extracted for the phone's seeds; the
  canvas "Auto DJ Seeds · Desktop", option A, the owner's pick, and the
  library pick in place of its Add song sheet — decisions 20–34): clauses
  15, 23 and 31 rewritten; 10–12, 14, 25, 30, 36, 41–43, 45, 51, 54, 57,
  58 and 63 changed; 59 and 64–66 new. The room is regrouped into FILTERS
  and MATCH, with Sonic similarity's two sub-rows, and the Sonic sheet and
  quick config give Drift · Stay close up to the Seeds sheet. The old
  anchors go into the lists: the rolling history of five picks is More
  like's unpinned rows, now pushed when a pick starts rather than when it
  is queued; the locked pin is Stay close's frozen lists; and switching
  sonic off no longer clears them (clause 23(h)).
- **2026-11-17 — Three kit exceptions on the Seeds sheet** (decisions 22
  and 34; ui-kit.md, Context list and Buttons): the row's menu is a
  context list on a song row, hung under its row and right-aligned to its
  `[⋯]`, with a move it cannot make DIM rather than hidden and Remove in
  DANGER at rest; `+ Add song` is a text button that rests DIM where it
  cannot act (its list full, the library on screen another server's)
  rather than hiding, since its place in the list says where a song
  would go; and the checkbox stands on a row of its own, the sheet a row
  taller than A-Drift.
- **2026-11-17 — "Counts as a play" is a mirror** (decision 26): a quick
  skip is judged by the server's DEFAULT play threshold, 30 s or half a
  known length. An operator who changed the server's `stats` thresholds
  sees quick skips judged by the defaults; the server still decides the
  play.
- **2026-11-17 — The control face's clears count** (decision 27): the
  remote `/play` and `/queue/clear` go through Clear queue, so with the
  box ticked they let every pin go; the GUI's own Play replaces the queue
  without a clear and does not. The asymmetry is this player's lane rule
  (clause 10) meeting the record's checkbox. The remote `/next` is a Next
  like the bar's, so a quick skip counts there too (clause 23(d)).
- **2026-11-17 — A replacing play is not a new queue for the seeds here**
  (decision 27): on the phone every play that replaces the queue goes
  through its clear, so the DJ's unpinned picks start over and a ticked
  box lets the pins go; here only Clear queue does. Matching the phone
  would take a seeds-only new queue at every replacing play while the DJ
  is armed, or a change to the lane's own triggers. An open question for
  the owner; built as the player's rule stands.
- **2026-11-17 — The checkbox lets the pins go, not the lists** (decision
  27; clause 66): the owner's words name the pins ("clears pins when the
  queue is cleared") and their default keeps the skips' 30-minute timers,
  so a ticked clear starts both lists over only as far as the pins —
  unpinned skips stay, as the phone settled it. A wider wipe is one line
  away if it was meant.
- **2026-11-17 — The record cannot be pinned** (docs/ux-contracts/README.md:
  "Pin the record's commit"): the phone's seeds build is uncommitted, so
  the branch and its base (`claude/autodj-seeds` on `c1393e12`), the
  canvas, and mStream #1108 (merged, unreleased) are pinned instead.
  Re-extract against the phone's merge.
- **2026-11-17 — The TUI drives the same mode**: its Auto-DJ tab keeps its
  Anchor row — "Follow the vibe" / "Stay on seed", the older words for
  Drift and Stay close — and the row now freezes and thaws the lists as
  the Seeds sheet's radios do. Its Stay on seed note reads "picks stay
  close to the seeds as they stand" (it said "every pick stays close to
  the seed song", the old single locked pin). Its `n` is a Next like any,
  so a quick skip counts there too. It has no seeds of its own to show
  and never arms the seed pick. The GUI's Now Playing screen, which is
  the TUI's view (now-playing clause 1), shows the same row. *(Round 3,
  the owner's "Old rules there": with no Seeds sheet to show the lists,
  the TUI keeps their rules from before the second answers (decision 41);
  the rules follow the shell, not the view, so the GUI's Now Playing row
  keeps the owner's answers. The Stay on seed note promises nothing the
  old rules break, and stands.)*
- **2026-11-17 — A removal that empties the queue is a new lane**: clause
  10 named the trigger, but only a removal of the DJ's own server started
  one. Now any server removal that empties the queue does
  (`App::apply_sweep`): a pick out or owed is dropped (clause 11) and the
  DJ's unpinned More like rows go; Clear pins when the queue is cleared
  is Clear queue's alone (decision 27).
- **2026-11-17 — Not seen live**: the seeds are pinned by the tests above
  and nothing else — no window session, no rig run (the rig's servers have
  no discovery data, so `scenario_dj` sees the sub-rows rest and never
  reaches the Seeds sheet, clause 59), no running DJ against a server that
  takes negatives (mStream #1108 is not yet released), and no legacy
  console for the `*` / `o` / `[.]` stand-ins.
- **2026-11-17 — The owner's second answers** (decisions 27, 32 and
  35–38; clauses 10, 12, 15, 23, 51, 63, 64 and 66), asked of the phone
  and this player together and answered the same day:
  - **"Blend in new music"**: "At each new queue in Drift, the song playing
    joins More like unpinned, next to your pins; it rotates out like any
    DJ pick." Built at every lane's first real pick: after the phone's
    fresh starts — a new queue, Stop and Start, a move of the DJ to
    another server, a launch — and this player's other new lane, a
    removal that empties the queue (decision 35). Preview shows the song
    and keeps nothing.
  - **"Keep everything shown"**: "While the mode is Stay close, every row
    it shows survives new queues, Stop/Start and restarts; back in Drift
    only your own pins stay pinned." The file keeps each skip's Drift time
    where the phone keeps the set's frozen moment, and a flag that does
    not read costs its row where the phone reads a pin (decision 36).
  - **"Add the chosen song"**: "Choosing a first song in Stay close adds it
    to More like as a pinned row, the same as Add song; Random adds
    nothing." Choose and Recently played, the empty queue's buttons
    included (decision 37).
  - **The question left open by the first build is closed**: the owner's
    first question was about playing something new — "an album, a
    playlist" — so a play that replaces the queue is a new queue for the
    seeds (clause 12, decision 27). It is not a new lane: the rest of the
    lane was kept out of the question, and is left as it was.
  - **History.** Four passages above describe the first build and stay as
    written: the entries "The control face's clears count" (the GUI's own
    Play counts now), "A replacing play is not a new queue for the seeds
    here" and "The checkbox lets the pins go, not the lists" (in Stay
    close every row goes now), and the last sentence of "A removal that
    empties the queue is a new lane" (the box is a user's new queue's now,
    a clear or a replacing play — still never a removal's).
  - **Where the lane meets a new queue**, open for the owner. A pick
    already out when the queue is replaced still lands, after the new
    rows, and joins More like as it starts, so the lane's first pick finds
    it there and blends nothing new — rare, since a turn is out only for
    the second or two a pick takes. In Stay close with nothing left to
    stay close to — what a ticked box leaves at a new queue — that same
    pick is the queue's last row when the new queue's first real pick is
    composed, so it becomes the new queue's one song (clause 23(c)), and
    the file keeps it through Stop and Start, a move of the DJ, restarts
    and every new queue but a ticked one: the new music does not hold the
    lane there either. And a lane that let its pool go (clause 30) keeps
    it gone through a replacing play, since the worker keys the pool by
    server and lane: the song it blends is listed but not sent until a
    sonic setting or the mode changes, More like is edited on the sheet,
    or a new lane starts. One line would give a new queue the retry every
    More like edit makes. *(Round 3: the last half is answered — a new
    queue gives the pool another go, and a pick out cannot take it back,
    decision 43. The first half, found again in round 3's review, is fixed
    without a new lane: the new queue's first real pick is composed as the
    play starts its row, so that row blends in Drift or is Stay close's
    one song, and the late pick joins beside it as a pick — decision 35.)*
  - **Two gaps at the remote**, open for the owner and not changed here.
    The first is older than this round: a remote `/play` or
    `/queue/clear` that comes while one of the App's own pickers holds the
    keys — the Genres sheet, the DJ's opening question, the Now Playing
    screen's sources picker or skip-word field — is taken by that picker:
    `/queue/clear` clears nothing, and `/play` adds its song at the end
    and plays the queue's first row, so neither is a new queue. The
    second this round makes routine: a song sent from the remote is
    queued bare until its tags land, a fill that never reaches the seed
    lists. A `/play` that comes while something plays makes the new
    queue's first pick inside the play itself — the player reads as
    playing until its Stop lands — so in Drift, with the DJ on the
    session's server, the song it blends in (clause 15, decision 35) is
    bare every time, listed by its file name for as long as it stays;
    only a `/play` onto a stopped player races its tags. In Stay close
    with nothing to stay close to, the same turn takes the bare row as
    the one song. A pin, or Stay close, keeps the untitled row in the
    file (decision 36). Before the owner's second answers only Stay close's
    one song could take a bare row, and only a pin kept one in the file.
    *(Round 3, the owner's "Fill the seed rows": the second is closed — a
    bare row takes its title, artist and album when its tags land, and a
    pin, or a row the GUI's Stay close keeps, writes them soon, decision
    44. The first stays open: older than this work, it goes to a session
    of its own.)* *(2026-11-18, PR #85: the first is closed too — a remote
    clear or play gets past an open picker, decision 45.)*
  - **A full More like and Stay close's one song**, found in review. A
    move of the DJ in Stay close now keeps every row where it kept the
    pins alone, so eight rows of the old server — pins and frozen picks —
    can leave the new lane's one song no room, where before only eight
    pins could. The admission still never evicts (clause 23(c)), but the
    refusal was silent and Preview showed the song anyway. It is one
    `[dj]` line now, once each time the rule looks (clause 63), and
    Preview reads the admission's own rule (`Seeds::admits_anchor`).
    Open for the owner: whether the one song should weigh only the DJ
    server's rows, or come in past eight. *(Round 3, the owner's "Let it in
    as a 9th row": it comes in past eight, up to sixteen, and only rows the
    DJ's server can use hold a lane — decision 42; the line and Preview's
    reading speak for sixteen rows now.)*
  - **Not seen live** either: the tests pin the second answers and nothing
    else, as the entry above says of the first build.
- **2026-11-17 — The owner's afternoon answers (round 3)** (decisions 27,
  32, 35 and 37 amended, 39–44; clauses 10, 12, 15, 23, 26, 30, 31, 43, 63,
  64 and 66), five questions answered the same afternoon:
  - **"Take it out (Recommended)"**: "A quick skip removes the blended song
    from More like right away, like a skipped DJ pick. It isn't added to
    Less like and nothing is learned from it. The seeded start's opener
    stays as built." Built with the phone's follow-ups from its review: a
    blended DJ pick keeps a pick's rule, a pinned one stays, and the
    lane's picks pass over the song (decision 39). Here the pass-over is
    the worker's filter — one server, so no fan-out to guard — and it
    lasts the lane through a play that replaces the queue, which is no
    lane edge on this player; on the phone it ends at every replacing
    play.
  - **The clear's race, checked here**: the phone's window cannot open —
    one call drops the rows and starts the lane — but a cleared track's
    end still on its way asked the new lane's first pick for the emptied
    queue, and the pick played there. Closed at the event, where the phone
    holds every pick back across an awaited teardown: a track's end with
    no queue left asks nothing (decision 40).
  - **"Old rules there (Recommended)"**: "In terminal mode a chosen first
    song isn't pinned, and Stay close starts over with each new queue, as
    before. Desktop mode keeps your answers. Pins still carry over between
    modes; opening terminal mode lets go of the unpinned rows desktop Stay
    close kept." (decision 41). Two readings go to the owner with it, each
    one predicate from the other: the TUI's box lets the stored pins of
    both lists go, Less like's unpinned skips staying, as the first
    build's did; and an unpinned skip Stay close froze stays there through
    a new queue, its clock stopped, since a new lane never touched Less
    like — it does not outlive a restart. The phone has no counterpart: it
    always shows its Seeds page.
  - **"Let it in as a 9th row (Recommended)"**: "The new server's one song
    comes in even when More like is full of the old server's songs, so
    picks keep sonic similarity. Nothing is removed." (decision 42). With
    it, as on the phone, the three rules its review of the same change
    added: a list whose every row the server refused takes the song too,
    which the owner is told; sixteen rows at most, a restart's load
    sparing the pins; a pick past eight usable rows never leaving a pin
    out for the stand-in. Less like loads eight at most, as the phone's
    merge keeps it — it never passes eight in a run, so only a hand-edited
    file could tell, and sixteen there is one cap away.
  - **"Try again (Recommended)"**: "A play that replaces the queue retries
    sonic matching, as a More like edit already does. The phone already
    works this way." (decision 43). A pick already out cannot take the
    retry back here — the pool's generation — which closes the same gap
    for the More like edits and the sonic settings; the phone's replacing
    play is a lane edge, so it never had one.
  - **"Fill the seed rows (Recommended)"**: "When a song's tags arrive, its
    rows in More like and Less like get the title and artist too, and a
    saved pin keeps them." The album comes with them, as the file keeps it
    (decision 44).
  - **Not asked, and left as built**: Drift still sends More like rows its
    lane found refused, which the worker leaves out, so with every row
    refused a Drift pick goes without sonic where the phone falls back to
    the song playing; and the phone lets a Stay close lane go when a
    refusal is found, this player at the next pick, so rows the user
    removes in between leave a list emptied, which stays empty. The other
    order — the rows still usable removed after the refusal — agrees with
    the phone since the review (below).
  - **Found in review, and fixed** (decisions 35, 39, 40 and 42): a pick
    out at a play that replaces the queue landed after the new rows and
    became the new queue's start, so nothing of the new music joined More
    like, and Stay close held to the late pick — the second answers'
    "Where the lane meets a new queue"; the new queue's first pick is now
    composed as the play starts its row, the pick out still landing. A
    play onto a row that waits for its tunnel sent a Stop and no Play, so
    the end of the track left behind skipped the user's choice, or asked
    a pick that played over it; that end is dropped now. Stay close's
    release took the song playing after the user's own removal, or move,
    of the last row the lane could use, refused rows still listed, where
    the phone never releases for the user's edits; now it does not. And
    the pass-over is no longer likened to the cursor, which lets a skipped
    DJ pick come back once it leaves the server's last 50.
  - **Still open**: the first of "Two gaps at the remote" — a remote
    `/play` or `/queue/clear` an open picker swallows — older than this
    work, for a session of its own; and a pick already out that lands
    while the queue's last row waits for its tunnel still starts over that
    row, the landing's play-now test reading the idle status (clause 14) —
    older too. *(2026-11-18: the first is closed by PR #85, decision 45.)*
  - **History.** The second answers' items "Where the lane meets a new
    queue", "Two gaps at the remote" and "A full More like and Stay close's
    one song", and the entry "The TUI drives the same mode", describe the
    builds before this round; each carries a note and stays as written.
  - **Not seen live** either: the tests pin round 3 and nothing else.
- **2026-11-20 — TOOLS holds six rooms** (entry point 2): Peer recs joins
  the group under Discover where the server offers it
  (peer-recommendations.md §1.1; discovery-page clause 2). Auto DJ stays
  on row 15; with all six it rises to 14, 13, 12 and 11 as Settings comes
  up to 20, 19, 18 and 17 — the last only at 100×24 with the footer on,
  where Files and Search rise into rows 1 and 2 for a fourth step. The
  row keeps its `•` and its `D`.
- **2026-11-28 — Quick config drawn by egui in the window (clause 67)**,
  with the GUI's chrome (the "Top Bar and Transport in egui" canvas).
  Deliberate, the board plan's decision D18 as built: the kit's hung
  frame and shadow where the cells draw an accent box; 20-point rows,
  8-point gaps and 10 of padding, Studio's rows being too tall for the
  room over the bar; the match bar as level segments with quiet ends; the
  keyboard's hand the raised fill and the accent's ring; a press on the
  popover's frame off its controls takes nothing, as the cells' guard,
  the guard being the frame as drawn rather than the cells' rect. Left for
  the owner, drawn as the code behaves, except that a long Sonic label is
  cut short of its value rather than run into it (the plan's R23): the
  chosen server's mark in the accent where the kit's choices are the ok
  green; Play from hidden for one server with several libraries; Smart
  filters a press that does nothing until the probe answers; Sonic off
  showing no "Off"; the split's chevron pointing closed/open rather
  than toward the popover (R18); and the popover floating over the Admin
  screen's egui room, which stays drawn beneath it, where the cells'
  popover stands it down (R10, built as the plan's D25 recommends,
  pending the ruling).
- **2026-11-28, later — The Library rests under the DJ's layers** (clause
  58): quick config, a sheet, the Seeds sheet and the First song question
  join the modals under which the Library draws without the pointer, in
  both faces; the Files room lit its rows' verbs and ripened their
  tooltips under quick config, where a press only put it away.
