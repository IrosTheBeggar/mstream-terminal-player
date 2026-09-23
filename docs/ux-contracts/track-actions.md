# Track actions & the queue

| | |
|---|---|
| **Design of record** | `mstream_music` @ `a27da385` (2026-09-21) — `lib/widgets/track_actions_sheet.dart` (the one sheet for a song wherever it is met: header with cover, title, byline and spec line; the badge row with the interactive rating, key, tempo, lyrics; the actions Add next, Play now, Add to end, Add to playlist, Download, Find similar; a peer's track loses rating, lyrics and playlists), `lib/util/queue_actions.dart` (the verbs' exact semantics: `addNext`, `playNow`, `addToQueueEnd`, `playFromHere`, `addRowsToQueue`, `handleTrackTap`), `lib/widgets/queue_list.dart` (the queue: a grip per row, tap = skip and play, swipe or the desktop ⋮ for Song info / Download / Remove, the header's count, Auto DJ, Download all and Clear queue, "Queue is empty"), `lib/widgets/playlist_picker_sheet.dart` (the server's playlists plus "New playlist" through the shared name dialog; the add route creates a missing playlist), `lib/widgets/star_rating.dart` (0–10 on the wire drawn as five stars in halves; a tap sets a whole star, a tap on the current star clears; half stars are an opt-in long-press; optimistic with a revert on failure, the latest tap winning), `lib/screens/metadata_screen.dart` (Song Info: art, title, artist, album · year, "Track N of M", chips for length, BPM, key and genre, the file location with a copy). |
| **Server API** | `POST /api/v1/db/rate-song {filepath, rating}` — `rating` 0–10 or `null` to clear; 404 for a path the library does not hold · `POST /api/v1/playlist/add-song {playlist, song}` — appends the vpath-relative path, **creating the playlist when it does not exist** · `GET /api/v1/playlist/getall` → `[{name}]` sorted, case-insensitive · `POST /api/v1/db/metadata {filepath}` → the full block for one track · `GET /api/v1/db/rated` → the user's rated tracks (a browse list, not ported here) · `GET /api/v1/lyrics?path=` (not ported here). **Off the federation allowlist**: rate-song, every playlist route, lyrics — a peer's track has no rating, no playlist and no words to open. |
| **Already in this repo** | The queue's verbs and their meanings are the multi-server contract's clauses 30–34 (Play now, Add next, Add to end, Play from here, Add all, Remove, Reorder, Clear, shuffle and repeat, what starts an empty queue, the panel's head); `Queue::{insert_next, move_row, remove, clear, replace}` and the App's `queue_selected_next`, `remove_queue_row`, `move_queue_row`, `jump_to_playing`, `play_index`; the TUI's keys `N` `P` `d` `C` `<` `>` `i`; the GUI's hover verbs `[▸] [»] [+]` on every track row (`draw_pane_rows`) and the queue panel (`src/gui/queue.rs`: rows three tall with covers, a click plays, a hover `[x]` removes, the DJ badge, the count with the total length); `TrackMetadata` carrying `rating`, `bpm`, `musical_key`, `has_lyrics`, `format`, `bitrate`, `sample_rate`, `bit_depth`, `channels`, `file_size`, `genres`, `track_total`, `disc_total`; the client's `metadata`, `playlists`, `playlist_new`; the playlists room's name dialog. **Missing**: any per-track sheet, a `rate-song` and an `add-song` client call, reorder in the GUI (a grip, or keys), a Clear verb in the panel, keyboard reach into the panel, Song info, a way to open the sheet from the playing track. |
| **Target surface** | the GUI player — every track row (Files, the albums' tracks, the library rooms, Search, Playlists), the queue panel's rows, and the bar's playing card; the App gains the rating and add-to-playlist plumbing the TUI shares |
| **Status** | contract extracted 2026-09-21 and **implemented the same day** — the shared half in `src/tui/app/track.rs` (rating, add to playlist, the block, playlist names), the GUI's in `src/gui/actions.rs` (the sheet, the picker, Song Info, the queue's grip, clear and keys); **checked on the rig the same day** (`smoke/gui/scenario_actions.py`: the sheet from a row's verb, a four-star rating the server kept, Song Info, a playlist created by the add and listed in its room, the header's clear) |

## Intent

The same handful of things you can do to a song, wherever you meet it: put
it next, play it now, add it to the end, put it in a playlist, rate it,
see what it is. And a queue you can shape — reorder, remove, clear — with
the keys and the pointer alike, so the list that is about to play is never
something you can only watch.

## Entry points

1. **A track row's `[⋯]`** — the fourth hover verb after play now, add
   next, add to the end — on every browse pane and on the queue's rows;
   **`m`** on the highlighted row; a **right click** on a row (this
   surface's own: a terminal has one, and the record's long-press has no
   pointer equivalent).
2. **The bar's playing card**: `[⋯]` on hover, or `m` while nothing else
   claims it — the sheet for what is playing.
3. **The queue panel's header**: `clear` beside QUEUE while rows exist.
4. The verbs the sheet gathers stay where they are: the row's own hover
   verbs and keys (`P` `N` `a`), the bar's Play / Queue all / Shuffle
   (browser-top-bar contract), the panel's `[x]`.

## States & flows

Sheet closed → **the sheet** over the room, for one track → an action
runs and the sheet closes, or Esc closes it. From the sheet: **Add to
playlist** opens the picker (choose one, or New playlist through the name
dialog) → adds → says so; **Song info** opens the info modal → Esc; the
**rating** changes in place, at once, and reverts if the server refuses.
The queue: a click on a row plays it (as today); a click also hands the
panel the keys until a click lands elsewhere; a press on a row's grip
drags it; `clear` empties the queue and ends playback.

## Behavior contract

### The sheet

1. **One sheet for every song** — browse rows, queue rows, the playing
   track — with the same actions in the same order, so "the same actions
   wherever you meet a song" stays true (the record's own reason for one
   builder).
2. **The header names the track by sight**: the cover when the track has
   one (the empty slot frame otherwise, only when text can balance it),
   the title (the file name when untagged), the byline `artist · album`
   with either half missing, and the spec line `FLAC · 320 kbps ·
   44.1 kHz · 4:12` with each part independent and the line absent when
   empty (bitrate rounds to kbps; the sample rate drops a whole kHz's
   decimal).
3. **The badge row** under the header: the **rating** (clause 10), the
   **key**, the **tempo** when above zero (zero is "not measured"), and
   **Lyrics** when the server holds words — a fact here, not a door (the
   lyrics view is the Now Playing contract's). A peer's track shows no
   rating and no lyrics badge.
4. **The actions, in order**: **Play now** · **Add next** · **Add to end**
   · **Add to playlist** · **Song info** · and, on a queue row, **Remove
   from queue**. Each closes the sheet first. *Absent on this surface*:
   Download (the player streams), Find similar (until the Discover room
   lands — it re-enters then, gated on discovery as the record gates it).
5. **What the verbs do** is the multi-server contract's clause 32; the
   sheet adds no fourth meaning. Play now says nothing (the sound is the
   answer); Add next and Add to end say "1 song added to queue"; a queue
   row's Remove says nothing and the row leaves.
6. **Add to end is the row's `[+]`** — one verb, one meaning: it appends,
   and an empty, idle queue starts on it (the multi-server contract's
   clause 33, a plain add-to-queue activation). The record hides Add to
   end when its tap behaviour already appends and never starts on it;
   this surface has no tap behaviour, so the sheet's verb is the row's
   verb. Logged below.
7. **Add to playlist is offered for a track on the user's own server
   only** — a peer's path would store what the parent cannot resolve, and
   the routes are off the allowlist. The picker (clause 12) belongs to the
   track's server, which in a mixed queue is not necessarily the
   session's.
8. **The sheet reads the row's own metadata** and asks the track's server
   for the full block on open (`/db/metadata`), so a row a listing
   described thinly — a file explorer row, a search hit — fills in while
   the sheet is up; the header and badges update in place.
9. **Esc closes**; a click outside closes; the sheet owns the keyboard
   while it is up (the kit's modal law).

### Rating

10. **Five stars in halves** on the badge row — `★★★½☆` reads the wire's
    0–10 as the record draws it. A click on a star sets that whole star;
    a click on the star that is the current rating **clears** it; `←`/`→`
    step a whole star down or up from the current, `0` clears. Half stars
    are displayed, never set (the record's half-star input is an opt-in
    long-press; its setting is out of scope).
11. **Optimistic, with a revert**: the row, the queue's copy of the track
    and the sheet change at once; the write goes to the track's own
    server (multi-server clause 31); a refusal puts the previous value
    back and says "Could not save rating" — unless a newer change has
    since gone out, in which case the newer one stands (the record's
    latest-tap-wins).

### Add to playlist

12. **The picker** is a kit modal listing the track's server's playlists
    (`getall`), **New playlist** leading it; empty reads "No playlists yet
    — New playlist creates one". Choosing one adds the track and says
    "Added to {name}"; New playlist opens the playlists contract's name
    dialog and then adds — the add route creates the playlist, so there
    is no separate create. Esc leaves without adding.
13. **A failure is one sentence**: "Couldn't add to the playlist." — the
    playlists contract's habit is followed where the server said
    something: `Couldn't add to the playlist — <the server's words>`.
14. **The playlists room learns of a new playlist** without a restart:
    the room's list refreshes after an add that named a playlist it did
    not know.

### Song info

15. **A kit modal**: the cover, the title, the artist, `album · year`,
    `Track N of M · Disc N of M` where the server reports them, then the
    facts as label–value rows: length, BPM, key, genre(s), format,
    bitrate, sample rate, bit depth, channels, file size, play count,
    rating, and the file path last — every row present only when the
    fact is. Asked of the server on open like the sheet (clause 8).
16. **The path is shown, not copied**: the record's copy button has no
    terminal equivalent this surface promises (OSC 52 is a later kit
    question); the path is plain text the terminal can select.

### The queue panel

17. **A row's verbs on hover**: `[x]` removes (as today); `[⋯]` opens the
    sheet; a **grip** `≡` at the row's edge takes a drag. A click anywhere
    else on the row plays it (the record's skip-and-play).
18. **Reorder by drag**: press on the grip, move, release — the row
    follows the pointer between its neighbours as it goes, and the playing
    row keeps its identity wherever it lands (`move_row`). The DJ's badge
    travels with the row.
19. **Reorder by key**: a click on a row gives the panel the keyboard (the
    App's focus goes to the queue, the TUI's rule); then `<` and `>` move
    the highlighted row, `↑ ↓` walk the rows, `Enter` plays the row, `d`
    removes it, `C` clears, `i` jumps to the playing row, `m` opens its
    sheet. A click in a browse room, or Esc, hands the keys back. The
    footer names these keys while the panel has them.
20. **Clear**: the header's `clear` empties the queue, ends playback, and
    ends a DJ session with it — no confirmation (the record has none; the
    verb is a small dim word, and the key is `C`, deliberate enough).
21. **Removing the playing row stops playback** (this player's standing
    rule: nothing plays that is no longer queued); removing any other row
    leaves playback alone.
22. **Persistence**: the queue's order and badges are saved as today;
    a reorder or a removal is a save.

## Wording

| String | Key |
|---|---|
| Play now · Add next · Add to end of queue | `gui.files.now_tip` / `next_tip` / `queue_tip` (exist, with their keys); the sheet's rows `gui.act.play_now` · `gui.act.add_next` · `gui.act.add_end` — the record's `queuePlayNow` · `queueAddNext` · `queueAddToEnd` |
| Add to playlist | `gui.act.add_playlist` — `trackAddToPlaylist` |
| Song info | `gui.act.info` — `songInfoTitle` |
| Remove from queue | `gui.act.remove` — the record's desktop literal; `gui.queue.remove_tip` exists for the `[x]` |
| More actions (the `[⋯]` tip) | `gui.act.more_tip` — `browserMoreActions` |
| 1 song added to queue · {n} songs added to queue | `gui.act.added_one` · `gui.act.added_many` — `browserSongsAdded` |
| Rate · Could not save rating | `gui.act.rate` · `gui.act.rating_failed` — `ratingTitle` · `ratingFailed` |
| New playlist · No playlists yet — New playlist creates one ⚑ · Added to {name} · Couldn't add to the playlist. | `gui.pl.new` (exists) · `gui.act.no_playlists` · `gui.act.added_to` · `gui.act.add_failed` — `playlistsNew` · `addToPlaylistEmpty` · `addedToPlaylist` · `trackAddToPlaylistFailed` |
| Lyrics (the badge) | `gui.act.lyrics` — `lyricsTitle` |
| Track N of M · Disc N of M · Length · BPM · Key · Genre · Format · Bitrate · Sample rate · Bit depth · Channels · File size · Play count · Path | `gui.info.*` — the record's chips are self-labelling; the labels here are this surface's |
| clear (the header verb) · Clear queue (its tip) | `gui.queue.clear` · `gui.queue.clear_tip` — `mainClearQueue` |
| Drag to reorder (the grip's tip) | `gui.queue.grip_tip` |

## Out of scope here

- **Download** and **Download all** (offline copies): the player streams;
  a persistent track cache is the plan's Phase 7.
- **Find similar**: the Discover room's contract; the row returns to the
  sheet with it.
- **Lyrics**: the Now Playing contract's (the TUI's tab is a placeholder
  today); the badge is a fact only.
- **Share**: the record's share subsystem, absent here.
- **The tap-behaviour setting** (Add to queue · Play from here · Add and
  play) and the **half-star** setting: this surface offers every verb on
  every row and sets whole stars.
- **The Rated list** (`/db/rated`): a browse room, the library rooms'
  contract once ratings exist here.
- **Local files**: no device library on this surface.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| Long-press → bottom sheet | `[⋯]` as the fourth hover verb, `m`, and a right click; a kit modal (the torrent chooser's shape) with the header block, the badge row, and one row per action; Esc or a click outside closes |
| The rating badge + dialog | The badge row's `★★★½☆` with clicks on the stars, `←`/`→`/`0`; the App's `rate_track(origin, filepath, rating)` writes through `client_for(reach)` and patches every copy of the track (the queue's rows, the panes' rows, the playing track); `Event::Rated { origin, filepath, rating, ok }` reverts on `!ok` when no newer rating is pending |
| The playlist picker sheet + name dialog | A kit modal list from `ApiCmd::Playlists { reach }`; New playlist reuses `playlists.rs`'s dialog; `ApiCmd::AddToPlaylist { reach, playlist, song }` → `Event::AddedToPlaylist { playlist, ok, why }`; the playlists room refreshes on a created name |
| Song Info screen | A kit modal of label–value rows from the track's block, refreshed by `ApiCmd::TrackInfo { reach, filepath }` |
| The queue row's grip and swipe | A hover `≡` on the row's first line and `[⋯]` beside it; the grip's press arms a row drag the event loop feeds (press, drag, release) — `move_row` on every crossing; the swipe's Info/Remove are the sheet's |
| The queue header's Clear / Download all | `clear` as a dim text button beside QUEUE; Download all absent |
| Focus | The App's `Focus::Queue` set by a click on a row; the panel draws the highlighted row; the GUI routes `< > d C i m Enter ↑ ↓` to the App while the queue has focus; Esc or a browse click returns to `Focus::Browser` |
| The playing card | The bar's card gets a hover `[⋯]` and `m` opens the sheet for `now_playing` |
| Notes | The note line, the record's snackbar words |

### What the tests pin

The `[⋯]` on a Files row, an album's row, a queue row and the playing
card opens the sheet naming that track; the actions send the App's
verbs; Add to playlist is absent for a peer's track and present for the
own server's; the picker lists the playlists, New playlist adds through
the dialog, a created name reaches the room; a star sets, the current
star clears, a refusal reverts, a newer rating survives an older
refusal; Song info lists only the facts present; the grip drag moves a
row and keeps the playing row; the keys move, remove, clear and play
while the queue has focus and stop when a browse click takes it back;
`clear` empties and stops.

## Deviations log

- **2026-09-21 — Implemented**, with these differences from the wording
  above: the notes name the song — "queued {title}" and "{title} — next",
  the App's standing words — rather than counting ("1 song added to
  queue"), because a note that says which song did what is worth more
  than one that counts to one; the record's "Song Info" keeps its
  capital; a queue row's grip and `[⋯]` share the row's first line and
  the `[x]` keeps the last; a pane row's own verbs and a click in a browse
  room both hand the keys back from the queue.
- **2026-09-21 — Extracted.** Add to end is the row's `[+]` (an empty
  idle queue starts on it; the record's Add to end never starts and is
  hidden under two of its three tap behaviours). Whole stars only. Right
  click and `m` open the sheet. Download, Find similar, Lyrics and Share
  stay out; the Rated list waits on ratings landing. The queue gains
  keyboard reach through the App's own focus, which the TUI already has.
- **2026-09-22 — The card's hover `[⋯]` removed** (entry point 2, and the
  translation table's playing-card row). The bar's design pass (the
  "Player bar options" canvas, I) took the verb off the card so the title
  keeps its whole width; the card's ways to the sheet are the right click
  and `m`, which the contract already names.
