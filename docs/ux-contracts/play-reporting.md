# Play reporting

| | |
|---|---|
| **Design of record** | mStream @ `ccf78c82` (2026-09-21) — `webapp/assets/js/mstream.play-session.js` (the fold: one session per song start, listened time from forward position deltas, pauses, the outcome, the too-short rule, the outbox and its checkpoint) and the web player that feeds it (`webapp/assets/js/mstream.player.js`); the server's write side `src/api/stats.js` (`POST /api/v1/stats/plays`, its `playSchema` and `requireAccount`), `src/stats/ingest.js` (`decideCounted`, `MAX_BATCH`) and `src/stats/store.js` (`OUTCOMES`, `SOURCES`). The mobile app in this checkout reports no plays (the webapp's comment says its fold mirrors the mobile app's; that app is newer than the one here). |
| **Server API** | `POST /api/v1/stats/plays` `{client: {name, version?, instanceId?}, plays: [{id, filePath, peerId?, startedAt, endedAt?, playedMs, durationMs?, outcome, source?, sessionId?, pauseCount, track?}]}` (≤ 200 plays; instants as ISO 8601 or epoch ms) → `{accepted: [id…], duplicates: [id…], rejected: [{id, reason}…]}`; a caller with no account — a federation key, a guest — is refused 403. Outcomes `completed · skipped · stopped`; sources `manual · shuffle · autodj · playlist · smart-playlist · auto · carplay · cast · other` (`legacy` is the server's own). **A play counts** when `playedMs ≥ 30 000` or ≥ half a known length (`decideCounted`, the operator's `stats` config); the verdict is stored, so history never rewrites. A peer's track is posted to the PARENT with `peerId` and a `track` snapshot (`title, artist, album, durationMs, hash, artFile`). The ping's `stats` (a version number) says the server has this API; without it the legacy `POST /api/v1/lastfm/scrobble-by-filepath {filePath}` counts a play by decree, thirty seconds in. |
| **Already in this repo** | The engine's status ticks (`PlayerStatus`: position, paused, duration, source) and its end-of-track events (`TrackEnded`, `HandedOver`, `PlaybackFailed`), the App's one door for every Play (`play_index`) and its hook on outgoing audio commands (`note_pending`), `Reach` / `reach()` for a row's own server or a peer's parent, the queue file's saver (`QueueSaver`, `queue.json`) as the persistence pattern, `clock::Instant` for both targets. **Missing** before 2026-09-23: everything else — no session, no post, no outbox; Last played and Most played (library-rooms clauses 21–24) showed other clients' listening only. |
| **Target surface** | the shared App, so both shells report; no UI beyond `[stats]` log lines |
| **Status** | contract extracted and **implemented 2026-09-23** (`src/tui/app/stats.rs`, the worker's `ReportPlays` / `Scrobble`, `stats.json`); checked on the rig the same day (`smoke/gui/scenario_stats.py`: a track played past thirty seconds and skipped appears in Last played and Most played) |

## Intent

What you play here counts where you keep your library: a song you listen
to is one play on the server, the way a song played in the webapp is, so
Last played, Most played and the server's own stats tell the truth about
your listening whichever player you used. The player says exactly what
happened — how long you listened, whether you let the song end, skipped
it or stopped — and the server decides what counts.

## Entry points

1. **Every song start** in either shell — a click, Enter, the queue
   advancing, a crossfade hand-over, a restored queue resumed — opens a
   session. Nothing is asked of the user.
2. **Launch**: the outbox and a checkpointed session come back from
   `stats.json`; what was owed goes out once a server can be reached.

## States & flows

A **session** opens when a queue row is told to play and folds the
engine's status ticks while it is the current source. It **closes** when
that row stops being the one playing: the engine ran out of it, blended
into the next, the user moved on, playback stopped or failed. A closed
session becomes a **play** in the **outbox** — or nothing, when too short
— and the outbox is posted to the plays' own servers, one batch in flight
at a time, retried while the server keeps saying no for reasons that may
pass. The outbox and the open session are **checkpointed** to disk, so a
crash still yields a `stopped` play at the next launch.

## Behavior contract

### The session

1. **One session per song start**, tied to the queue row's filepath and
   origin, opened by the same Play that starts the engine — a click, the
   queue advancing, a hand-over adopted, a resume. A new Play closes the
   session before it.
2. **Listened time** is the sum of forward position steps of at most 3 s
   per status while playing and not paused; a bigger step, or a backward
   one, is a seek and adds nothing (`SEEK_JUMP_S`). The furthest position
   reached is kept.
3. **Pauses are counted**, one per pause edge.
4. **The length** is the row's metadata duration, else the engine's once
   it knows one.
5. **The outcome**: `completed` when the engine ran out of the track or
   blended out of it, or when the furthest position came within 2 s of a
   known length however the song was left (`END_SLACK_S`: a skip during
   the fade-out is not a skip); otherwise `skipped` when the user moved
   on (next, previous to another track, a click on another row) and
   `stopped` for a stop, a failure, a cleared queue, a quit.
6. **Too short is not a play**: under 1 000 ms listened the session is
   dropped, never posted (`MIN_POST_MS`) — a mis-click, a failed load.
7. **The play** carries an id unique to this install (the instance's id,
   the wall clock and a counter), the filepath as the row's server knows
   it, `startedAt` / `endedAt` in epoch milliseconds, `playedMs`,
   `durationMs` when known, the outcome, the source — `autodj` for a
   DJ-picked row, `shuffle` while shuffle is on, else `manual` — one
   `sessionId` per launch, and `pauseCount`. A peer row's play carries
   `peerId` and the track snapshot from its metadata.

### The outbox

8. **Where a play goes**: its row's own server through the reach its
   stream used, with the client named (`mstream-terminal-player/<version>`
   and the instance id); a peer row's play goes to the PARENT, never to
   the peer. One batch (≤ 200) in flight at a time; every id the server
   names — accepted, duplicate or rejected — is settled. A batch the
   server calls malformed (400), one it has no route for (404) or one it
   will never take from this caller (403: a guest, a key) is dropped and
   logged; anything else — no network, a server error, an expired token —
   is kept for the next try, a minute later. A finished play, a connect
   and the timer each try again.
9. **The outbox persists** in `stats.json` beside `queue.json` (at most
   500 plays, oldest out) with the open session checkpointed on the way
   — a write soon after a change and every ten seconds while a session is
   open and its listening moves (a paused session is written once more,
   then not again until it plays, seeks or ends), a flush on the way out
   — so a crash, a kill or a closed terminal yields a `stopped` play at
   the next launch, listened to the last checkpoint. The instance id lives in the file too, so it is one per
   install.
10. **A server without the Stats API** (the ping's `stats` absent) gets
    the legacy scrobble instead: once per session, thirty seconds in, for
    the session's own server; its plays never enter the outbox, which
    would only be refused. Another server in the queue is asked through
    the Stats API and dropped on a 404.
11. **A peer session** — the user browsing a peer, with a guest token or
    through the parent — never posts to the peer: its rows' plays wait in
    the outbox for the parent's own reach (a saved server) and go there
    with `peerId`.
12. **The log is the second voice**: a posted batch, a kept one and a
    dropped one are each a `[stats]` line at info with the server's word.

## Wording

None on screen. The `[stats]` log lines are the player's own.

## Out of scope here

- **Now-playing notices** (`POST /api/v1/stats/now-playing`, the server's
  live view) — a second, unrelated protocol; revisit with the Now Playing
  screen.
- **Last.fm** — the server forwards counted plays itself for a linked
  account.
- **A per-tab `sessionId`** — one player process is one session here.
- **The web shell's outbox** — the browser build keeps its outbox in
  memory for the session (no file); a closed tab loses an unposted play.

## Translation notes (shared App)

| Record | Here |
|---|---|
| `createSession` / `tick` / `pause` / `resume` / `withDuration` / `classify` / `finish` | `stats::Session` and its methods, pure, in `src/tui/app/stats.rs`; the App feeds them from `Event::Status`, `play_index`, `HandedOver`, `TrackEnded`, `PlaybackFailed` and the `Stop` it sends |
| The outbox in `localStorage`, `checkpoint` / `recover` | `stats::Stats` on the App (the outbox, the batch in flight, the retry clock, the pending outcome) and `StatsSnapshot` in `stats.json`, written by the queue file's saver grown a second file |
| `post({client, plays})` and the settle | `ApiCmd::ReportPlays { reach, body, ids }` answered by `Event::PlaysReported { ids, outcome }` — `Settled`, `Dropped`, `Kept` — shaped once in the worker for both shells |
| The 30-second timer for a server without the Stats API | `ApiCmd::Scrobble { reach, filepath }` from the session fold at `LEGACY_SCROBBLE_MS`; `Event::Scrobbled` is only logged |
| The ping's `stats` flag | `Ping.stats: Option<u32>` from `/api/v1/ping` and the layered `/api/`'s `features.stats`; `Capabilities.stats` |
| `crypto.randomUUID` | ids from the standard library's random hasher seed: `<instance>-<epoch ms>-<n>` |

### What the tests pin

App (pure): a play that runs out is posted `completed` with the seconds
listened, its length, `manual`; a skip is `skipped`, a seek adds no time,
a pause is counted; under a second is never posted; a kept batch waits a
minute and a settled or dropped one clears; a peer row's play goes to the
parent with `peerId` and a snapshot; a server without the Stats API gets
one legacy scrobble at thirty seconds and no batch; the outbox and a
checkpointed session survive a restart, the session as a `stopped` play;
a paused session is checkpointed once, then not while it stays paused.
Worker: the server's answers mapped onto settled / dropped / kept. Rig: a
track played past thirty seconds and skipped appears in Last played and
Most played.

## Deviations log

- **2026-09-23 — Extracted and implemented.** One `sessionId` per launch
  (the record's is per tab). The browser build's outbox is in memory. A
  404 from a server the ping never described drops the batch (the record
  keeps everything but a 400; a server without the route would keep it
  forever).
- **2026-09-26 — The review's fixes.** A session knows its stream: a status
  from another source — the tail of a track that failed to give way, which
  the engine keeps sounding — folds nothing (it was reported as the new
  row's play, completed). Gapless repeat-one's laps reach this side as a
  position falling from the end to the start; each lap is a play of its
  own (clause 1). A server whose batch was kept backs off alone, so the
  other servers' plays go out (clause 8 per server; one unreachable
  server held every play behind it until the cap dropped them — the trim
  is logged now). The browser build flushes its outbox each frame (it
  never posted). `stats.json` always keeps the install's id (clause 9).
  A blend into a queue edited underneath closes the old track as completed;
  a row parked for its tunnel closes the one it left as a skip (clause 5).
  The GUI's direct calls run the App's effect hook, so a click's Play arms
  the starting gate and its Stop closes the session as the TUI's do. Left
  for later: an owed play is posted with whichever account holds the
  server's token at flush time (clause 8's "the reach its stream used"
  on a shared machine) — stamping the play with its user is the fix.
- **2026-09-26 — The follow-up: a play knows its account** (clause 8). A
  session records the account signed in at its server when it opens, the
  owed play inherits it (kept in `stats.json`), and the flush posts a play
  only under that account — held for its return, never posted as another
  user's listening. A play from before the stamp carries none and posts
  as before.
- **2026-09-28 — A paused session stops being rewritten** (clause 9;
  performance audit #107). The ten-second checkpoint ran for as long as a
  session was open, so a player left paused rewrote and flushed
  `stats.json` 360 times an hour with nothing new but the checkpoint's
  time. It now writes only when something a recovery reads has moved —
  the time listened, the pauses, the furthest point, the length — which
  is every checkpoint while playing and one after a pause. A crash during
  a long pause recovers the same play, listened to the same second; its
  end time is the pause's (within ten seconds) rather than the crash's,
  which is what the record's webapp gives (its checkpoint rides
  `timeupdate`, silent while paused).
