# Library rooms — Artists, Genres, Recent

| | |
|---|---|
| **Design of record** | `mstream_music` @ `a27da385` (2026-09-21) — `lib/singletons/api.dart` (`getArtists`, `getArtistAlbums`, `getRecentlyAdded`: how each answer becomes rows, which lists are pushed *alphabetical*, the `SINGLES` bucket), `lib/singletons/browser_list.dart` (the list stack and its alphabetical cache), `lib/screens/browser.dart` (a list that is all albums becomes the grid; the strip's gutter; the row builders), `lib/widgets/letter_strip.dart` (the `#` + A–Z index, `showsFor` and its threshold, `scrubbableCount`, snapping to the nearest present letter), `lib/widgets/album_grid.dart` (cards, columns by width), `lib/objects/display_item.dart` (`getText`: the title, else the file name; `getSubText`: the artist). **Genres**: the mobile app does not browse them (its genres call feeds Auto DJ only), so the record for that room is the webapp — mStream @ `ccf78c82`, `webapp/alpha/m.js` (`renderGenre` "name (count)", `getAllGenres`, `getGenreSongsList`, `getRecentlyAdded` with its 100). |
| **Server API** | `GET/POST /api/v1/db/artists` → `{artists: [name…]}`, the server's order (A–Z, case-insensitive) · `POST /api/v1/db/artists-albums {artist}` → `{albums: [{name, artist, year, album_art_file}]}` in the server's order — every album the artist is on, compilations and features included (V17), plus one **name-less bucket** for the artist's loose tracks · `GET/POST /api/v1/db/genres` → `{genres: [{name, track_count}]}` · `POST /api/v1/db/genre-songs {genre, limit?, offset?}` → tracks (everything when unpaged) · `POST /api/v1/db/recent/added {limit}` (required) → tracks newest first (`created_at DESC`), each carrying `created-at` · `POST /api/v1/db/album-songs {album, artist}` — `album: null` is the singles bucket (the webapp sends `album: null, artist: null`). All take `ignoreVPaths`; all answer through a federated peer's parent (the webapp's `peerReq` variants). |
| **Already in this repo** | `LibraryNode::{Artists, Artist, Genres, Genre, Recent}` and `load_library` (`src/tui/worker.rs`, `RECENT_LIMIT = 100`); `entries_from_library` (`src/tui/app/entries.rs`: an artist row is its name, an album row `album_label` "Artist — Name (year)", a genre row `genre_label` "Name (count)", a track row `display_name`); the Library pane (`App.library`) with its `Drill` stack and the pane's **trail** — Back restores the parent listing and its cursor from the trail without asking again; `open_library_node`; the TUI's Library tab, which walks all five nodes today; the GUI's album wall (`src/gui/albums.rs`: cells, pages, per-slot covers, the drilled album's track list through `draw_pane_rows`) with `App.albums` as its cache; the browse top bar (docs/ux-contracts/browser-top-bar.md); the GUI nav rows Artists · Genres · Recent, placeholders that say the browse slice is coming. |
| **Target surface** | the GUI player — three left-nav rooms (digits 3, 4, 5) over the shared App's Library pane; the artist's albums as the wall; the TUI changes nothing |
| **Status** | contract extracted 2026-09-21 and **implemented the same day** in the GUI (`src/gui/library.rs`, the wall generalized in `src/gui/albums.rs`, the kit's `letter_strip`); rig check pending |

## Intent

The library by its tags: every artist, every genre, and what arrived
lately — three ways into the same songs the Files room reaches by folder.
Each is a list you can narrow, jump through by letter, and play from,
drilling down to albums and tracks without leaving the room, with the
browse bar's verbs appearing exactly when the list holds something to
play.

## Entry points

1. The left nav: **Artists** (`3`), **Genres** (`4`), **Recent** (`5`).
   Each opens its root list; with no session up, the room shows the
   servers room's offline text like every browse room does.
2. **Search**: an artist hit drills into that artist's albums inside the
   Search room (`SearchNode::Library(Artist)`, already shared) — the
   Search room's own contract, named here because the drill is the same.
3. The TUI's Library tab reaches every node through the shared App —
   unchanged.
4. *Later, not here*: "go to artist" / "go to album" from a track's
   actions (the track-actions contract).

## States & flows

**Artists**: the list of names → an artist → its **albums as a wall** →
an album → its tracks. **Genres**: the list with counts → a genre → its
tracks. **Recent**: the newest 100 tracks, no drill. Every step down is a
click or Enter; every step up is Back — `◂` in the header, `h`,
Backspace, Esc — and the parent comes back with its cursor where it was.
The three rooms share the App's one Library pane, so choosing a library
row starts that row's list afresh, as the record's drawer entries do;
coming back to a room opens its root list again. Loading reads
"listing…"; an empty answer reads "Nothing here yet"; a filter that
matches nothing keeps the bar contract's rule (the way back is one key).

## Behavior contract

### The lists

1. **Every room is the Library pane under the browse top bar**, and the
   bar contract applies whole: the filter always; Play, Queue all and
   Shuffle only while the list holds tracks — an artists list and a genres
   list keep a clean line, Recent and a genre's tracks carry the verbs,
   the wall keeps its own.
2. **Rows.** Artists: the name alone, in the server's order. Genres: the
   name with its track count at the right, dim (the webapp's `(n)`), in
   the server's order. Recent: track rows exactly as the drilled album's —
   the title (the file name when there is none), the duration at the
   right, the playing row marked, the hover verbs — newest first.
3. **Counts** read `n items` in the header (the wall keeps `n albums`);
   under a filter, `n of m`.
4. **Recent holds the newest 100** (the record's limit); a smaller library
   shows what it has, and the count says so. Recent asks again on every
   visit — "recent" means now, and the ask is one hundred rows.
5. **Container rows carry no hover verbs**: an artist or a genre is not
   playable until it is opened. The record's long-press "more actions" on
   such a row is the track-actions contract's.
6. **Activate** on an artist opens its albums; on a genre, its tracks; on
   a track, clause 14. **Back** climbs one level, restoring the parent
   from the pane's trail — cursor, scroll and all — without asking the
   server again.

### The artist's albums

7. **An artist opens as a wall** (the record switches to the grid whenever
   a list is all albums): the same cards, pages, covers and keys as the
   Albums wall; the header reads `◂ Artists ▸ {artist}` with `n albums`.
   The order is the server's, not alphabetical, so no strip (the record
   pushes an artist's albums without the alphabetical flag).
8. **The singles bucket**: the server's name-less album — the artist's
   loose tracks — is one card labelled **Singles**, no year, its art the
   server's pick; opening it asks album-songs with no album name and the
   artist (the webapp's `album: null`). The record labels the bucket
   `SINGLES` and notes that opening it errors; here it opens.
9. **A card opens the album's tracks** — the drilled album view that
   exists — and that view's Back reads the artist's name, not "Albums",
   because that is where Back goes.

### The letter strip

10. **On an alphabetical list of 25 rows or more** — Artists, Genres, the
    Albums wall — a strip of `#` and A–Z sits under the bar: letters no
    row starts with are dim, the rest live; a click scrolls the list so
    the first row with that initial is at the top (the wall turns to the
    page that holds it). `#` is digits, punctuation and any non-Latin
    initial (the record's bucket). A click on a dim letter snaps to the
    nearest present letter, so no click is dead. Under 25 rows there is
    no strip — the record's default threshold; its setting, and the
    strip's side, are out of scope.
11. **The strip follows the filter**: a narrowed list has narrowed
    letters, and a jump lands in the narrowed list.
12. **Not on Recent** (newest first is not an alphabet), not on an
    artist's albums (clause 7), not on a track list.
13. **The strip is a pointer tool.** The keyboard has the filter and
    PageUp/PageDown; no letter key jumps, because the letters are the
    room's verbs (`a` queues, `h` climbs, `f` filters).

### Playing

14. **A click or Enter on a track** is the App's Activate: it plays the
    track and queues the list from there — the record's default tap
    behaviour ("play from here") and this repo's rule in every browse
    room. The hover verbs offer play now, add next and add to the end
    (`P`, `N`, `a` on the keyboard). The record's tap-behaviour setting
    is out of scope: this surface offers all three verbs on every track
    row instead of choosing one.
15. **The bar's Play, Queue all and Shuffle** act on the list's tracks:
    Recent's hundred, a genre's tracks, an album's tracks.
16. **A peer session browses all three** through its parent (the
    allowlist); the header crumb wears the multi-server contract's
    `· read-only`.

## Wording

| String | Key |
|---|---|
| Artists · Genres · Recent (the nav rows, exist) | `gui.nav.artists` · `gui.nav.genres` · `gui.nav.recent` — the record's `browserArtists` / `browserRecent` |
| n items · n albums · n of m (exist) | `gui.files.items` · `gui.alb.count` · the bar contract's |
| listing… (exists) | `busy.listing` |
| Nothing here yet | `gui.lib.empty` — the record's `browserEmptyList` |
| Singles | `gui.lib.singles` — the record's literal `SINGLES`, in a word |
| Jump to %{letter} (the strip's tip) | `gui.lib.jump_tip` |

## Out of scope here

- The record's **Rated** list (the player has no ratings yet), **Local
  Files** and downloads, the **file explorer** (the Files room).
- The **tap-behaviour** setting (clause 14) and the strip's **threshold
  and side** settings (clause 10).
- **More actions** on a row, **go to artist / album**, ratings — the
  track-actions contract.
- **Disc headers** in the drilled album (the record's album detail groups
  a multi-disc album under "Disc n") — the Albums room's, not these
  rooms'; logged so it is not forgotten.
- The webapp's **Recently played** and **Most played** stats lists.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| The drawer's Library entries | The nav rows Artists · Genres · Recent; each `Act::Nav` opens the root node fresh (`open_library_node(node, true)`) and points the App's tab at Library, the wall's way |
| A list of DisplayItems | The App's Library pane drawn by `draw_pane_rows` (rows, cursor, hover verbs, kit scrollbar), under `draw_bar_controls` — the Files room's grammar with the pane swapped |
| The album grid for an artist | `albums.rs` generalized: the wall draws whichever album list the App holds for the view on screen — `App.albums` for the root wall as today, and the artist's albums kept beside it for the `Artist` node — with the cells, pages, cover slots and keys shared |
| The vertical letter strip | A one-line `# A B … Z` row under the bar's controls (a 22-row pane cannot stand 27 letters), a kit widget: each letter a click target, dim when absent, the row's tip "Jump to X"; a jump sets the list's first visible row (the pane's scroll) or the wall's page |
| `getSubText` (the artist under a title) | The track row's `display_name` already reads "Artist - Title"; no second line |
| The singles bucket | `album_songs` grows an `Option<&str>` album so a name-less album asks with `album: null` |
| Loading / empty | `busy.listing` / `gui.lib.empty` |

### What the tests pin

Each room fetches its root on entry and draws the pane; the genres row
shows its count; Recent asks with the limit and shows track rows with the
verbs; an artist row opens a wall of its albums with the artist's header,
and a card opens the tracks with the artist as Back; the singles card;
the strip appears at 25 rows and not at 24, dims absent letters, jumps
the list, snaps from a dim letter, follows the filter, and is absent on
Recent; the bar's verbs are absent on artists and genres and present on
tracks.

## Deviations log

- **2026-09-21 — Implemented**: a wall's card opens through the pane's own
  row (an Activate) rather than the wall's direct door, so the Albums
  room's drill also keeps its trail now and Back restores its list without
  a request — the drilled album no longer re-asks for the wall. The Albums
  wall gained the strip with it (clause 10 names it).
- **2026-09-21 — Extracted.** Genres take the webapp as their record (the
  mobile app has no genres browse). The strip is horizontal here. The
  three rooms share one pane, so a room restarts at its root when
  returned to (the record's drawer does the same; the record keeps a
  stack per browse session, not per entry). No tap-behaviour setting:
  three verbs on every track row. The singles bucket opens instead of
  erroring.
