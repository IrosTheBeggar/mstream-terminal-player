# Auto DJ

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`695f4d0`, 2026-09-06) — `lib/screens/auto_dj.dart` (the settings screen), `lib/widgets/auto_dj_start_sheet.dart` (the empty-queue chooser), `lib/widgets/queue_list.dart` (`toggleAutoDJ` + `_seedEmptyQueue`, the labelled `AutoDjButton`, the queue-row badge), `lib/widgets/player_panel.dart` (the same button on the player), `lib/singletons/auto_dj_manager.dart` (the settings, their persistence, `libraryFilters`), `lib/media/audio_stuff.dart` (the session: `setAutoDJ`, `_autoDJPick`, `sonicParams`, `shouldTopUpAutoDJ`, `_resetAutoDJSession`, `_startAutoDJFromSeed`, `restoreAutoDJ`), `lib/singletons/server_capabilities.dart` + `lib/util/server_version.dart` (the capability learner and the version floors), `lib/main.dart` (drawer entry, launch restore). The screen is itself a port of the webapp's panel (`webapp/velvet/app.js viewAutoDJ`; `webapp/alpha/auto-dj.js` for the anchor semantics) — where they disagree, this contract says so. |
| **Server API** | `POST /api/v1/db/random-songs` — one call, every knob: `ignoreList` (round-trip cursor) · `ignoreVPaths` · `minRating` · `genres` + `genreMode` · `bpmRanges` + `bpmRangesWide` + `requireBpm` · `musicalKeys` + `requireMusicalKey` · `ignoreArtists` · `similarTo` + `minSimilarity` (both or neither) · `minDuration` + `maxDuration` + `allowUnknownDuration`. `GET /api/v1/db/genres`. Ping: `vpaths`, `discovery`. Version floors the record keeps: 4.6.0 `ignoreVPaths` · 6.7.1 the BPM / key / genre / cooldown block · 6.15.2 the sonic pair · 6.25.0 the length window. |
| **Already in this repo** | Most of a DJ: `src/dj.rs` (Camelot math, same/half/double windows, the perceptual sonic slider, `build_random_request`, `Settings`), `[player.dj]` prefs, `AutoDjMode` and the `A` cycle, `maybe_autodj` (the queue-end top-up), `consume_dj` (queue the pick, start it if idle, the cursor), `autodj_pick` (`Similar` via nearest neighbours, `BpmKey` via random-songs, the sonic 400 retried once without the pool), `autodj_sample` (three picks without queueing), the TUI's Auto-DJ tab (`DjPanel` / `DjRow`) with its genre picker, `Event::{AutoDjPick, AutoDjSample, Genres}`, and the GUI bar's `auto-dj` toggle. **Missing**: sources (`ignoreVPaths`), the length window, the keyword filter, `requireBpm` / `requireMusicalKey`, the session-locked Camelot anchor, the rolling / locked sonic anchors as the record defines them, the one-shot seed and the start chooser, lane resets with in-flight discards, the capability learner, the queue badge, and the armed-not-playing restore. |
| **Target surface** | the GUI player — an Auto DJ room plus the bar toggle (the room's home is open question 5) |
| **Status** | contract extracted 2026-09-06; **open questions below — discussion before implementation** |

## Intent

Keep the music going. When the queue runs low, Auto DJ asks the server for
one more song that fits — the libraries the user scoped, the rating floor,
the genres, the tempo and key of what is playing and, where the server has
listened to the library, the sound of the session — and drops it in as the
next row, one pick ahead, for as long as the DJ is on. Everything it does
is visible: its rows wear a badge in the queue, and the moment a constraint
cannot be met it says so once and keeps playing rather than falling silent.

## Entry points

1. **The toggle** — one labelled `Auto DJ` button, in the queue header and
   on the player, lit while the DJ runs. It toggles for the current
   server; the confirmation reads "Auto DJ Enabled" / "Auto DJ Disabled"
   ("Auto DJ Enabled For {url}" with several servers).
2. **The settings screen** (drawer › Auto DJ): the status and Start/Stop
   at the top, then Server · Sources · Continuity · Filters.
3. **Discover › "Start a sonic session"** — clears the lane and the
   queue, seeds the DJ with the track (or a random one), switches it on.
   *Out of scope here (the Discover slice), named because it reuses this
   feature's seed and lane.*
4. **Launch** — a DJ remembered as on comes back **armed, never playing**.
5. Android Auto / CarPlay "Shuffle All" — *out of scope.*

## States & flows

Off → switched on → (empty queue only) the **opening question** — ask /
surprise me / let me choose, the answer rememberable → **armed**, with a
session **lane**: the ignore cursor, the Camelot anchor, the sonic history
and pin, the once-per-lane warning budgets. Every way the user steers
somewhere new — switching on, switching the DJ's server, clearing the
queue, removing the server — starts a **new lane**; re-arming on the same
server does not. Off ends the lane. Settings edits apply to the running
lane at once.

## Behavior contract

### Switching on

1. With a queue the DJ **follows what is there** — nothing is replaced. On
   the last row and idle, a pick is fetched and played at once; on the
   last row and playing, a pick is fetched as the next row; anywhere else
   nothing happens until the queue's end.
2. With an **empty queue** the DJ needs an opening track, and the **On an
   empty queue** setting decides: *ask* (default) shows the chooser —
   "Start Auto DJ with what?" · **Surprise me** · **Let me choose** ·
   **Remember this** — the other two answers skip it. Dismissing the
   chooser leaves the DJ off.
3. **Surprise me** fetches the opener with the **same library filters
   every later pick obeys** (sources, rating, genre, length) — never the
   plain random pick — plays it, then fetches two followers so the
   session feels under way. Nothing matched (a 400 under constraints) →
   "No songs match your Auto DJ filters — try loosening them"; any other
   failure → "Couldn't fetch a song from the server." The DJ stays off.
4. **Let me choose** hands off to the library under the banner "Pick the
   opening song — tap a track anywhere in the library"; the DJ switches
   on when a row lands, not before; the pick is track one and the sonic
   seed; the user stays where the pick was made.
5. The opening seed is **one-shot**: it opens exactly one session and is
   consumed by the queue clear that starts it, so no later empty-queue
   start can replay it.
6. A shared (federated) server refuses to host the DJ, in words. *Out of
   scope here — no peers on this surface.*

### The session lane

10. A **new lane** resets the ignore cursor, the Camelot anchor, the sonic
    history and the locked pin, and the warning budgets. Triggers:
    switching on, switching the DJ's server, clearing the queue, a server
    removal that empties the queue. Re-arming on the same server is not
    a new lane.
11. A pick **in flight when the lane changes is discarded** — it must not
    repopulate a cleared queue, overwrite the fresh cursor, or seed the
    new lane's history with the dead one's track.
12. A **queue clear keeps the DJ armed and adds nothing** — no chooser, no
    surprise pick; the DJ resumes from a clean slate at the next queue
    end.
13. **The top-up rule**: exactly when a real playback session stands on
    the **last row** — the current index is the last, the player is not
    idle, and no failed-track skip walk is running. Idle index emissions
    on the last row never top up (a batch add onto a fresh player once
    stacked eleven picks in nine seconds); neither does a walk through
    broken streams (the walk and the DJ feed each other unboundedly).
14. **One pick ahead**: the pick lands as the next row, played at once
    only when the queue was empty. Overlapping fetches are dropped; every
    trigger re-fires once the in-flight one lands.
15. Every DJ pick joins the **rolling sonic history** — kept even while
    sonic mode is off, so switching it on mid-session already has the
    session's recent sound to anchor on.

### The request

20. **The library filters**, shared by every pick and the opener:
    `ignoreVPaths` (the sources switched OFF), `minRating` when set,
    `genres` + `genreMode` when the genre filter is on and non-empty,
    `minDuration` / `maxDuration` when the length window is on and a bound
    is off its rail (a rail means unbounded and is not sent),
    `allowUnknownDuration` only alongside a real bound.
21. **BPM continuity**: windows of ± tolerance BPM (1–20, default 8)
    around the PLAYING track's tempo at same, half and double time; the
    wide set is tolerance + 2; `requireBpm`, so the waterfall never falls
    back to untagged picks. No tempo tag on the playing track → no
    windows; the pick is free.
22. **Harmonic mixing**: a **session Camelot anchor**, locked on the first
    keyed DJ pick (or the playing track's key on the first call) and
    reused all lane long; `musicalKeys` = the anchor's neighbourhood;
    `requireMusicalKey` whenever the switch is on, so even the first pick
    is keyed and can lock the anchor.
23. **Sonic similarity** sends `similarTo` + `minSimilarity` (raw cosine,
    .30–.80, default .55) only when the DJ's server advertises
    `discovery`, the switch is on, the constraint has not been suppressed
    this lane, and the seed belongs to that server (paths are
    per-library). Seeds by **anchor mode**: *rolling* ("Follow the vibe",
    default) — the last 5 DJ picks, deduped, most recent last; else the
    explicit seed; else the playing track. *Locked* ("Stay on seed") —
    one pin, set on the lane's first pick (the seed, else the playing
    track) and reused. A cold start with nothing to anchor on stays plain
    random until the first pick seeds it.
24. `ignoreList` is an opaque **round-trip cursor**: send back what came,
    unchanged.
25. **The capability filter**: before sending, drop every key the server
    is *known* (version floor) or *learned* (this session) not to
    accept. A rejection whose body reads `"<key>" is not allowed` —
    status 400 or 403; the body is the signal — teaches the key for the
    rest of the session and the request goes again without it, so one new
    parameter can never take Auto DJ down on an older server. The learner
    lives in memory only, so an upgrade is noticed at the next launch.
26. **The keyword filter** is client-side: a pick whose title, artist,
    album or filepath contains any word (case-insensitive) is refused and
    re-asked with the updated cursor, up to 5 times; then the last answer
    is accepted rather than stalling the queue forever.

### When a constraint cannot be met

30. Sonic fails loud server-side; the client **degrades the constraint,
    not the music**: "nothing within the similarity range", "not analyzed
    yet" and "discovery is disabled" each drop the sonic keys for the rest
    of the lane and retry the same pick without them. The first two say
    so **once** per lane — "Auto DJ: nothing is within the similarity
    range, so it is playing without that filter. Loosen the match slider
    to use it again." / "Auto DJ: the discovery scan hasn't reached these
    tracks yet, so it is playing without sonic similarity." — the third
    stays silent (the user switched it off themselves).
31. A **dead locked pin** (a 404, or "not analyzed") is dropped so the
    next call re-pins from the playing track — locked mode never
    re-derives on its own.
32. **An auth failure** (401/403 that is not a schema rejection): "Auto DJ
    stopped — the server session expired. Re-login in Manage Servers." —
    once per lane.
33. **A network failure** is silent (an offline queue end is normal),
    logged once per outage, and the pick is remembered as owed and
    retried when the connection returns; the queue **parks** at its end
    instead of stopping.
34. A working pick clears every warning budget and the owed-pick flag.

### The settings screen

40. **Status**: "Auto DJ is on" · "Songs are picked from {url} when the
    queue runs low." or "Auto DJ is off" · "Tap below to start. The
    current server's library will be used." — and one button, **Start
    Auto DJ** / **Stop Auto DJ** (the destructive color when stopping),
    through the same toggle every entry point uses (a direct arm skipped
    the opening question once and replayed a stale seed).
41. **Server** (several servers, while on): the DJ's server; re-selecting
    it is a no-op (a restart would drop the lane for nothing); peers are
    not offered.
42. **Sources** (more than one library, while on): a switch per library;
    switching the last one off is refused — "At least one source is
    required."
43. **Continuity › Sonic similarity**: switch, **ON by default** (it is
    what makes a DJ more than shuffle; safe only because clause 30
    degrades instead of stopping). Disabled with its reason when the
    server has no discovery data ("This server doesn't have discovery data
    — picks stay random.") or is older than 6.15.2 ("Needs server 6.15.2
    or newer"). While on: **Match strictness** "{pct}% or closer"
    (.30–.80); **On an empty queue** — Ask · Surprise me · Let me choose;
    **Anchor** — Follow the vibe · Stay on seed, with the mode's hint.
    Switching sonic OFF clears the lane's anchors; the explicit seed
    survives.
44. **BPM continuity**: switch, then **Tolerance** "± {bpm} BPM" (1–20).
45. **Harmonic mixing**: switch.
46. **Filters › Minimum rating**: Any, 0.5 ★ … 5.0 ★ (1–10) — "Only pick
    songs at or above this rating."
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
    6.25.0 hides the length window; the keyword filter always shows
    (client-side). An unknown version or a fork keeps every control and
    learns from a rejection (clause 25) instead.
51. **Edits apply to the running lane at once** — no restart; slider drags
    commit on release. Per-library settings (sources, rating, genre)
    belong to the server they were set against; session-wide ones (length,
    keyword, BPM, harmonic, sonic and its anchor, the empty-queue answer)
    travel with the user.

### Transparency and restore

60. Every DJ-picked row in the queue wears a **badge** — a sonic pick and
    a classic random pick differently; user-queued rows wear none.
61. **Remembered as on** brings the DJ back ARMED at launch, never
    playing; it engages at the next queue end. A user toggle that beats
    the restore wins.

## Wording

The record ships `autoDj*` keys in all ten locales — carried over at
implementation, not re-made. English reference:

| String | Key |
|---|---|
| Auto DJ | autoDjTitle |
| Auto DJ Enabled · Auto DJ Disabled · Auto DJ Enabled For {url} | autoDjEnabled · autoDjDisabled · autoDjEnabledFor |
| Auto DJ is on · Songs are picked from {url} when the queue runs low. | autoDjStatusOn · autoDjStatusOnDetail |
| Auto DJ is off · Tap below to start. The current server's library will be used. | autoDjStatusOff · autoDjStatusOffDetail |
| Start Auto DJ · Stop Auto DJ | autoDjStart · autoDjStop |
| Start Auto DJ with what? · Nothing is queued, so the DJ needs an opening track. With a queue it just follows what you already have. | autoDjStartTitle · autoDjStartSubtitle |
| Surprise me · Pick a random song from the library and build outward from it. | autoDjStartRandom · autoDjStartRandomSub |
| Let me choose · Open the library and pick the opening track yourself. | autoDjStartPick · autoDjStartPickSub |
| Remember this · Skip this question next time and always start this way. | autoDjStartRemember · autoDjStartRememberSub |
| Pick the opening song — tap a track anywhere in the library | autoDjStartPickBanner |
| On an empty queue · What Auto DJ does when you switch it on with nothing queued. · Ask | autoDjOnEmptyQueue · autoDjOnEmptyQueueSub · autoDjStartAskShort |
| No songs match your Auto DJ filters — try loosening them · Couldn't fetch a song from the server. | autoDjSeedNoMatch · autoDjSonicSeedFailed |
| Server · Sources · Continuity · Filters | autoDjSection* |
| At least one source is required. | autoDjOneSourceRequired |
| Sonic similarity · Only pick songs that sound like the session, using the server's audio analysis. | autoDjSonicTitle · autoDjSonicSubtitle |
| This server doesn't have discovery data — picks stay random. · Needs server 6.15.2 or newer | autoDjSonicUnavailable · autoDjSonicNeedsNewerServer |
| Match strictness · {pct}% or closer | autoDjSonicStrictness · autoDjSonicStrictnessValue |
| Anchor · Follow the vibe · Stay on seed · Each pick follows the session's recent sound — it can slowly evolve. · Every pick stays close to the seed song for the whole session. | autoDjSonicAnchor* |
| BPM continuity · Prefer picks within a tempo window of the current song. Honours half/double-tempo equivalence. · Tolerance · ± {bpm} BPM | autoDjBpm* · autoDjTolerance |
| Harmonic mixing · Prefer picks in keys that mix well with the locked song (Camelot wheel neighbours). | autoDjHarmonic* |
| Minimum rating · Only pick songs at or above this rating. · Any | autoDjMinRating · autoDjMinRatingSubtitle · autoDjRatingAny |
| Track length · Skip interludes and long mixes by only picking tracks in a length range · Length · Any length · Over {min} · Under {max} · {min} to {max} · Include tracks of unknown length · Tracks your server has not read a length for are skipped otherwise | autoDjDuration* |
| Genre filter · Whitelist plays only matching tracks; blacklist skips them. · Whitelist · Blacklist · Pick genres · No genres selected. Tap "Pick genres" to choose. · Search genres… · {count} selected · No genres found on this server. · No genres match "{query}". · Could not load genres | autoDjGenre* · autoDjWhitelist · autoDjBlacklist · autoDjPickGenres · autoDjNoGenres* · autoDjSearchGenres · autoDjSelectedCount |
| Keyword filter · Skip picks whose title, artist, album, or filepath contains any of these words. · No keywords. Add words below to start filtering. · e.g. "live" or "remix" | autoDjKeyword* · autoDjNoKeywords |
| BPM continuity, harmonic mixing and the genre filter need a newer server. Update to get them. | autoDjNeedsNewerServer |
| Add a server first. | autoDjAddServerFirst |

The three failure toasts (clauses 30, 32) are English literals in the
record's audio handler, not keys — they get keys here.

## Out of scope here

- **The DJ server picker** (clause 41): this player runs one live
  session, and switching servers moves the DJ with it.
- **Federated peers** (clause 6): no peer browsing on this surface.
- **Discover's "Start a sonic session"**: the Discover slice, which will
  reuse the seed and the lane.
- **Android Auto / CarPlay**, and the **tunnel park-and-resume** of clause
  33 — the player's Quick Connect handling is its own slice; the clause's
  silence-and-retry stands.
- **The webapp's extras** the record dropped: Last.fm similar-artists
  mode, the crossfade row, the dice.

## Translation notes (terminal GUI) — for discussion

| Record | Here (proposed) |
|---|---|
| The labelled Auto DJ button (queue header + player) | The bar's `auto-dj` toggle, already lit while on — becomes plain on/off rather than the three-mode cycle it is today (question 1) |
| The settings screen | An **Auto DJ room**: 1-row settings rows (the torrent room's grammar), sections as dim group labels; its home is question 5 |
| Switches | `[✓]` / `[ ]` rows, Space toggles |
| Sliders (strictness, tolerance) | The sonic room's ten-cell bars: ←→ step, a click on a cell sets |
| The two-ended length range | Two rows — Shortest · Longest — on the same 0:00–20:00 bar, or one bar with two thumbs; the words summary beside it either way |
| Segmented buttons (empty queue · anchor · whitelist/blacklist) | Radio rows `(•)` |
| Chips (genres, keywords) | Inline lists with a hover `[X]`; the keyword input as a 1-row field |
| The genre picker sheet | A kit modal list with a filter line (the path modal's shape); Space toggles, Enter closes — the App's `GenrePicker` already exists |
| The start sheet | A kit modal (the torrent chooser's shape): the two rows and the remember box |
| "Let me choose" | The App's `Capture` machinery grown a `DjSeed` variant — the sonic pick's road, banner and Esc included |
| Snackbars / toasts | The note line; the once-per-lane budget kept |
| The queue badge | One dim glyph before the title in the queue panel (the TUI's queue could wear it too) |
| Server version gates | The session carries no version here → gates ride the **capability learner** (clause 25) plus the ping flags; the "needs a newer server" sentences become the learner's note when it drops a key |
| Per-server vs session-wide settings | `[player.dj]` grows the new fields; the per-library ones (sources, rating, genre) keyed by server in the config's server entries |
| Already shared | `dj.rs` (to be reshaped per questions 1–3), the top-up in `maybe_autodj`, `consume_dj`, the sample, the genre picker |

## Open questions (to settle before implementation)

1. **The request model.** The record composes ONE random-songs request from
   independent toggles (sonic + BPM + harmonic + filters). The player has
   three MODES — `Off / Similar / BpmKey` — where `Similar` takes the
   nearest-neighbour list and bypasses every filter. My lean: adopt the
   record's model in the shared App (on/off plus toggles) and retire
   `Similar` — rolling sonic mode delivers the same promise with the
   filters honoured. This changes the TUI's `A` key to on/off as well; a
   logged deviation there.
2. **Tempo units.** ± BPM, 1–20, default 8, wide = + 2 (record) versus ± %,
   default 6, wide = × 2 (player). Lean: the record's units, migrating the
   `tempo_tolerance` pref (a percent cannot be reinterpreted; it lands on
   the default).
3. **Key anchoring.** The record locks a session Camelot anchor on the
   first keyed pick and offers no "strict"; the player re-derives from
   each playing track with compatible / strict. Lean: the record's session
   anchor, and drop strict.
4. **Sonic default and slider.** The record is ON by default with the raw
   .30–.80 band shown as "N% or closer"; the player is OFF with a
   perceptual 1–100 onto .30–.85 and a pool-size readout the record
   lacks. Lean: the record's default-on and raw band, keeping the pool
   size and the "cosine ≥" readout as logged additions.
5. **Where the room lives.** The nav digits are spent and the bar toggle
   is the everyday control. Lean: a Settings doorway (`Auto DJ ▸`, its
   own group above SERVERS) now; when the Now Playing screen lands, the
   room may move there as a tab, the TUI's arrangement.
6. **Keep the Sample row?** The player previews three picks without
   queueing; the record has nothing like it. Lean: keep it — it is how a
   setting gets judged — as a logged deviation.
7. **The new parameters and the learner.** Sources, the length window, the
   keyword filter, `requireBpm` / `requireMusicalKey`, and the capability
   learner behind them. Lean: port all of it, the learner at the worker so
   the TUI shares it and an older server never loses Auto DJ to one new
   key.
8. **The queue badge** (clause 60). Lean: yes, one glyph.

## Deviations log

- **2026-09-06 — No design canvas planned**: the room is settings rows
  and modals already drawn once (the torrent room's rows and chooser, the
  sonic room's bars, the servers room's modals). Revisit if discussion
  disagrees.
