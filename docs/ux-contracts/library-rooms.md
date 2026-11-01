# Library rooms — Artists, Genres, Recent, Rated, Last played, Most played

| | |
|---|---|
| **Design of record** | `mstream_music` @ `a27da385` (2026-09-21) — `lib/singletons/api.dart` (`getArtists`, `getArtistAlbums`, `getRecentlyAdded`: how each answer becomes rows, which lists are pushed *alphabetical*, the `SINGLES` bucket), `lib/singletons/browser_list.dart` (the list stack and its alphabetical cache), `lib/screens/browser.dart` (a list that is all albums becomes the grid; the strip's gutter; the row builders), `lib/widgets/letter_strip.dart` (the `#` + A–Z index, `showsFor` and its threshold, `scrubbableCount`, snapping to the nearest present letter), `lib/widgets/album_grid.dart` (cards, columns by width), `lib/objects/display_item.dart` (`getText`: the title, else the file name; `getSubText`: the artist). **Genres**: the mobile app does not browse them (its genres call feeds Auto DJ only), so the record for that room is the webapp — mStream @ `ccf78c82`, `webapp/alpha/m.js` (`renderGenre` "name (count)", `getAllGenres`, `getGenreSongsList`, `getRecentlyAdded` with its 100). **Last played · Most played** (2026-09-23): the webapp again — `webapp/index.html` (the side-nav entries "Recently Played" / "Most Played", `local-only`), `webapp/alpha/m.js` (`getRecentlyPlayed` / `getMostPlayed`: a "Get last [100] songs" box, then the rows as files — the title, else the file name, the artist under it — through the same `createMusicFileHtml` as Recently Added). The mobile app has neither list. |
| **Server API** | `GET/POST /api/v1/db/artists` → `{artists: [name…]}`, the server's order (A–Z, case-insensitive) · `POST /api/v1/db/artists-albums {artist}` → `{albums: [{name, artist, year, album_art_file}]}` in the server's order — every album the artist is on, compilations and features included (V17), plus one **name-less bucket** for the artist's loose tracks · `GET/POST /api/v1/db/genres` → `{genres: [{name, track_count}]}` — since mStream #716 (2026-10-28) `{source?}` in (`tag` · `model`, absent for both) and `sources: {tag, model}` out, the genre count of each kind · `POST /api/v1/db/genre-songs {genre, source?, limit?, offset?}` → tracks (everything when unpaged; `source` keeps the songs linked from that kind) · `POST /api/v1/db/recent/added {limit}` (required) → tracks newest first (`created_at DESC`), each carrying `created-at` · `POST /api/v1/db/album-songs {album, artist}` — `album: null` is the singles bucket (the webapp sends `album: null, artist: null`). All take `ignoreVPaths`; all answer through a federated peer's parent (the webapp's `peerReq` variants). **The play lists** (2026-09-23): `POST /api/v1/db/stats/recently-played {limit}` (required; `ignoreVPaths` optional) → tracks by `last_played DESC` · `POST /api/v1/db/stats/most-played {limit}` → tracks by `play_count DESC` where `play_count > 0`; both read the signed-in user's own `user_metadata`, render like every listing, and answer `[]` for a caller with no user (a federation key, a guest). A play is what the server has counted — the webapp's play sessions (`/api/v1/stats/plays`, 6.27) or the legacy 30-second scrobble; this player reports neither yet. **Rated** (2026-10-31): `GET /api/v1/db/rated` → a bare array of the signed-in user's rated tracks, highest rating first; `POST` with `{moods}` filtered; off the federation allowlist. **The mood filters** (mStream #1092, #1099): `moods` `{valence, energy, voice, tags[], threshold?}` in the POST body of `db/albums`, `db/artists`, `db/recent/added` (beside `limit`) and `db/rated`; a filtered song list's rows carry `moodMatch` (0–1, three places, or null) and keep the list's own order. |
| **Already in this repo** | `LibraryNode::{Artists, Artist, Genres, Genre, Recent}` and `load_library` (`src/tui/worker.rs`, `RECENT_LIMIT = 100`); `entries_from_library` (`src/tui/app/entries.rs`: an artist row is its name, an album row `album_label` "Artist — Name (year)", a genre row `genre_label` "Name (count)", a track row `display_name`); the Library pane (`App.library`) with its `Drill` stack and the pane's **trail** — Back restores the parent listing and its cursor from the trail without asking again; `open_library_node`; the TUI's Library tab, which walks all five nodes today; the GUI's album wall (`src/gui/albums.rs`: cells, pages, per-slot covers, the drilled album's track list through `draw_pane_rows`) with `App.albums` as its cache; the browse top bar (docs/ux-contracts/browser-top-bar.md); the GUI nav rows Artists · Genres · Recent, placeholders that say the browse slice is coming. |
| **Target surface** | the GUI player — three left-nav rooms (digits 3, 4, 5) over the shared App's Library pane; the artist's albums as the wall; the TUI changes nothing. **2026-09-23**: Last played and Most played as two more rows of the LIBRARY group over the same pane — no digit (the digits end at 9), the pointer's rooms; the TUI's Library root menu gains the two nodes. **2026-10-31**: **Rated** as one more LIBRARY row after Recent, no digit, hidden for a peer; the TUI's menu gains no row |
| **Status** | contract extracted 2026-09-21 and **implemented the same day** in the GUI (`src/gui/library.rs`, the wall generalized in `src/gui/albums.rs`, the kit's `letter_strip`); **checked on the rig the same day** (`smoke/gui/scenario_library.py`: the artists list, Boukmanflow's wall with its Singles card, an album with the artist as Back, Esc up twice, chillhop's five tracks under the verbs, Recent's hundred). **2026-09-23**: Last played and Most played added (clauses 21–24), Recent's grammar over the same pane; the rig run opens both. **2026-10-31**: the mood filters redrawn as an overlay modal, and the Rated room (clauses 30–38), implemented and pinned by tests; not yet checked on the rig |

## Intent

The library by its tags: every artist, every genre, and what arrived
lately — three ways into the same songs the Files room reaches by folder.
Each is a list you can narrow, jump through by letter, and play from,
drilling down to albums and tracks without leaving the room, with the
browse bar's verbs appearing exactly when the list holds something to
play.

## Entry points

1. The left nav: **Artists** (`3`), **Genres** (`4`), **Recent** (`5`),
   since 2026-10-31 **Rated** after it, and since 2026-09-23 **Last
   played** and **Most played** — Rated and the play lists without a
   digit (the digits end at 9, `D` is the DJ's): the pointer's rows.
   Rated hides for a peer, as the play lists do (clause 37).
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
tracks. **Recent**: the newest 100 tracks, no drill. **Rated**: the songs
you rated, highest first, no drill. **Last played**: the
last 100 you played, newest first, no drill. **Most played**: the 100 you
have played most, most first, no drill. Every step down is a
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
3. **Counts** read `n items` in the header (the wall keeps `n albums`),
   `1 item` and `1 album` in their plural forms (clause 38); under the
   text filter, `n of m`.
4. **Recent holds the newest 100** (the record's limit); a smaller library
   shows what it has, and the count says so. Recent asks again on every
   visit — "recent" means now, and the ask is one hundred rows.
5. **Container rows carry no hover verbs**: an artist or a genre is not
   playable until it is opened. The record's long-press "more actions" on
   such a row is the track-actions contract's.
6. **Activate** on an artist opens its albums; on a genre, its tracks; on
   a track, clause 14. **Back** climbs one level, restoring the parent
   from the pane's trail — cursor, scroll and all — without asking the
   server again. The cursor is drawn only while the keyboard holds it
   (the kit's list-cursor law): a room opens with no row lit, ↓ shows the
   cursor where the pane rests it, a click on a row stows it.

### The artist's albums

7. **An artist opens as a wall** (the record switches to the grid whenever
   a list is all albums): the same cards, pages, covers and keys as the
   Albums wall; the header reads `◂ Artists ▸ {artist}` with `n albums`.
   The order is the server's, not alphabetical, so no strip (the record
   pushes an artist's albums without the alphabetical flag).
8. **The singles bucket**: the server's name-less album — the artist's
   loose tracks — is one card labelled **Singles**, no year, its art the
   server's pick; opening it asks album-songs with no album name and the
   artist (the webapp's `album: null`; this client sends `album: ""`, which
   the server reads the same way). The record labels the bucket
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
    room's verbs (`a` queues, `h` climbs, `f` filters). A jump seats the
    pane's cursor on the row it brought to the top without lighting it,
    so a ↓ after it picks the cursor up there.

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

### The play lists (2026-09-23)

21. **Last played and Most played are Recent's shape**: the Library pane
    under the bar, track rows exactly as Recent's (clause 2) with the
    verbs (clause 1), `n items` in the header (clause 3), the hundred
    asked on every visit (clause 4) — "played" means now — and no drill.
22. **The server's order stands**: last played newest first, most played
    by count, most first. Neither is an alphabet: no strip (clause 12).
23. **Whose plays**: the signed-in user's own — the server keeps a play
    count and a last-played time per user. A peer session's caller (a
    federation key, a guest) has no user there and is answered with an
    empty list, so the rooms hide for a peer as Playlists do (multi-server
    clause 26), and their rows go nowhere.
24. **What counts as a play** is the server's: the webapp's play sessions
    (6.27) and the legacy 30-second scrobble write the same stats. This
    player reports play sessions since 2026-09-23 (its own contract:
    docs/ux-contracts/play-reporting.md), so what is played here counts
    here too.

### The genre source (2026-10-28)

25. **Two kinds of genre** (mStream #716): a song is linked to a genre by
    its own tags, or by the discovery model's prediction. The server says
    how many genres of each kind it has (`sources`), and the Genres list
    shows one kind at a time, or both together.
26. **The kind is remembered** in `[gui] genre_source` — tags until one is
    chosen — and the room opens on it. A kind with no genres on this server
    gives way to the other (an untagged library lists its generated
    genres); a server that predates the filter lists every genre, as
    before.
27. **The switch**: `Tags · Generated · Both` sits at the controls line's
    right, where a list of tracks wears its verbs — a list of genres has
    none — and the filter leads the line as in every room. The kind on
    screen is in the ok colour, the others dim, each a click (a pointer
    control, as the strip is). Choosing asks
    for the list again in place — no new level, so Back climbs where it did
    — and keeps the choice; the kind already up asks nothing. Drawn only
    while the server has both kinds, never over a genre's songs, and not
    while the filter's field holds the line.
28. **A genre's songs follow its list**: opened from the generated list, a
    genre lists the songs the model linked to it — never one that carries
    it only in its tags — and the other way round; from Both, or from a
    server without the filter, every song.
29. **The TUI keeps every genre**: its Library menu lists both kinds
    together, as it always did, and has no switch.

### The mood filters (2026-10-29; the overlay, 2026-10-31)

30. **Filters by mood** (mStream #1092, #1099): a server with mood scores
    — the ping's `moodFilters` — filters the Albums wall, the Artists list,
    Recent and Rated by mood: Happy · Neutral · Sad, Relaxed · Neutral ·
    Aggressive, Instrumental · Vocals, any of Acoustic, Party, Danceable,
    and how sure every group must be: Strict · Standard · Loose. An album
    or artist stays when most of its songs match, a song when it matches.
    Offered only on the user's own server — never on a peer's lists, and
    a peer's session offers none — at each room's root: not on Genres, the
    play lists, Playlists, an artist's wall or an album. The choice rides
    the list's POST body as `moods`, the level inside it as `threshold` —
    Strict 0.7, Loose 0.3, Standard none — and only beside at least one
    mood or tag: a level alone filters nothing and is never sent. A
    filtered song list's rows carry `moodMatch` (clause 36). Each room
    keeps its own moods, level and Sort while the app runs; nothing is
    saved. Where a room does not offer the filters now (a peer's session,
    a server without scores) its kept choice stays, nothing of it is sent,
    and no opener, strip or key stands.
31. **The opener**: `mood` on line one, a DIM text button — BRIGHT BOLD
    under the pointer, a click opens the modal (clause 32) — with no dot,
    no chevron and no tooltip. Its column is fixed, so the count never
    moves it: on the lists two cells after the room's name (Rated x24,
    Recent x25, Artists x26 at the queue's 47 cells); on the wall ending
    thirteen cells in from the content's last (x51 at 47 cells, x84 at
    80), clear of a two-digit pager — a wider pager pulls it left to end
    three cells before its back arrow, the only time it moves. **`o`**
    opens the modal wherever the opener is drawn, and is a typed letter
    while the filter's field takes text; `O` stays unbound (the house rule
    against a capital whose lowercase is a key — `F` is out, `f` being the
    filter). The opener stands wherever the room offers the filters,
    whatever line two holds, the `/ filter` field included, and the count
    is never dropped for it: it ends two cells before the opener, and
    where a pager of three digits or more, or a five-digit count in a long
    locale, leaves it less room, its words are clipped by cells with the
    kit's mark, its digits whole. With key hints on, the room's resting tips
    line ends ` · o mood` where the whole line fits the footer. The wall's
    copy serves only the filter it was asked under — a return under
    another asks again; the session keeps only the whole Artists list, and
    a filtered one always asks.
32. **The modal**: 66 cells wide in every locale and every room, 10 rows
    tall, 11 on Recent and Rated with their SORT row. It stands where the
    kit's modal stands over the whole frame — x17–x82, r10–r19 (r9–r19
    with SORT) at 100×30, r7–r16 (r6–r16) at 100×24 — or, with the queue
    folded, centred in the room's 80 cells (x24–x89) with the room's width
    cleared on its rows (the kit's band); its bottom never passes the
    room's last row. A rounded ACCENT border (`+ - |` on the legacy
    console) carries the title `Filters` (TEXT BOLD) on its top border:
    no title row, no `[X]`, nothing on the bottom border — `[ Done ]` is
    the way out. **Its rows**: the three scales; the tags; a blank; the
    level; on a song list `SORT` (DIM, in the first column, no stop) with
    the list's own order — Rating on Rated, Newest on Recent — and Match;
    a blank; and the foot, `[ Done ]` (ACCENT BOLD) ending three cells in
    from the right corner and `clear` (a DIM text button) three cells left
    of it, drawn only while a mood, a tag or a level other than Standard
    is set. **Its columns** are the same box in every room: each as wide
    as a glyph, a space and its widest word in the locale across every
    row, the Sort words included, the first at least as wide as SORT, the
    slack shared between the two gaps. **Glyphs**: `( )` `(•)` for the
    scales, the level and Sort, `[ ]` `[✓]` for the tags (`(*)` `[x]` on
    the legacy console), a space, then the word; a stop's click is its
    glyph through its word. **Colours**: an unchosen glyph DIM and its word
    TEXT; a chosen scale value or tag OK, its word OK BOLD — green means
    only "a mood is on"; the chosen level and Sort the Radio group's ACCENT
    `(•)`, the word TEXT BOLD. The pointer brightens a word alone (BRIGHT
    BOLD); the glyph keeps its state's colour. **Match rests** while no
    mood is set — DIM, no click, no hover, no stop, no tooltip — and the
    own order shows pressed; Match stays the choice and wakes with the
    first mood. **Choosing**: a scale's value picks it, or lets it go when
    it is the one chosen (none chosen is allowed); a tag toggles; a level
    or Sort radio picks it, the pressed one doing nothing; `clear` lets
    every scale and tag go and puts the level on Standard, the Sort kept,
    the modal open. **The keyboard** has every key while the modal is up
    (Ctrl-C still quits): ↑ ↓ the row above or below in the hand's column
    — clamped to a row's last stop, SORT's column landing on the stop to
    its right, a resting Match on the own order — ↓ from the last choice
    row to `[ Done ]` and ↑ from the foot back to the remembered column,
    no wrap; ← → the stop beside, clamped, on the foot between `clear` and
    `[ Done ]`; Tab and Shift-Tab the reading order, wrapping; Space
    presses the held stop; Enter is Done from anywhere, pressing `clear`
    only while the hand shows on it; Esc and `o` close, as Done. **The
    hand**: `o` opens the modal with it up on Happy; a click-open — the
    opener, the strip's words — stows it on Happy, and the first walking
    key or Space only shows it; a click on a stop presses it and stows the
    hand there. Under the hand a radio's, a checkbox's or `clear`'s word
    is ACCENT BOLD, its glyph kept, and `[ Done ]` wears the selection
    slab; where the hand and the pointer meet, the hand wins. A stop that
    goes takes the hand along: a Match gone to rest to the own order, a
    `clear` gone to `[ Done ]`. **The room is inert** beneath (the kit's
    modal): its registries dropped and the frame a `Guard`, the hand
    cursor only over the stops, `clear` and `[ Done ]`; a click outside
    does nothing, nor do a right click and the wheel; no tooltips inside.
    With key hints on the tips line reads "↑ ↓ ← → move · Space choose ·
    Enter or Esc done".
33. **The TUI keeps every album, artist and song**: its Library menu lists
    them unfiltered, as it always did — no filters, no Sort, and no Rated
    row.
34. **Applying** (the seeds modal's law, discovery-page clause 13): the
    modal edits a draft, and nothing changes until it closes, whichever
    way. Then the room's choice becomes the draft and at most one thing
    happens. Where what is asked changed — the moods, and a threshold
    beside them — the room **asks once**: the wall from its first page, a
    list from its top, the cursor stowed; the rows on screen stand until
    the answer lands, and a list asked under the old choice is dropped
    when it lands. An ask that fails, or a drill from the rows still
    standing, puts the room's choice back to the one those rows carry: the
    strip never names a filter they lack, Back comes back to them under
    their own, and Done with the new choice asks again. Where only a song list's shown order changed, it
    **re-orders in hand** — at its top, stowed, nothing asked (clause 36).
    Otherwise **nothing**: a level picked with no mood set changes nothing
    on screen and asks nothing, and is kept for the first mood; the same
    choice made again asks nothing. An ask that also changes the shown
    order lands in the new one. A standing text filter goes when the new
    list lands (the bar contract's clause 24: a filter describes the list
    it was typed against).
35. **The strip**: while the room offers the filters and its kept choice
    holds a mood or a tag, the row under the controls — r4, the letter
    strip's — names it: the scales' values, then the tags in their order,
    then the level's word when it is not Standard, ` · ` (DIM) between, the
    moods and tags OK and the level TEXT, and `[X]` (DIM) one cell after.
    The words are one click that opens the modal with the hand stowed,
    lit BRIGHT BOLD together under the pointer; `[X]` (BRIGHT BOLD under
    the pointer) lets every scale and tag go and the level back to
    Standard, the Sort kept, and the room asks once. Neither has a tip.
    The run ends a cell in from the content's last (x62 at 47 cells, x95 at
    80); where it would pass that the words clip with `…` by cells, `[X]`
    stays, and only then is the whole run the words' tooltip. The open
    modal does not change it. The strip takes a free row, so nothing below
    it moves — except where an alphabetical list also has the letter strip
    (clause 10): the mood strip comes first, the letter strip a row lower,
    and the rows or the grid a row lower with it.
36. **Sort** (Recent and Rated): the list's own order — Rated by rating,
    highest first; Recent newest first — or Match. The client sorts: Match
    is a stable sort by `moodMatch`, best first, a song with none last,
    ties in the server's order; computed by key, so switching back and
    forth is exact, and the rows a text filter hides re-order with the
    shown. The rows do not show the match. The order shown is the choice
    while a mood is set, the own order otherwise.
37. **Rated**: one more LIBRARY row after Recent, `Rated` (the phone's
    `browserRated`), without a digit — the fourteenth room, last in `NAV`.
    Recent's shape: the Library pane under the bar, track rows with their
    length, the bar's verbs (clause 1), `n items`, no letter strip and no
    drill, asked on every visit — ratings change.
    `GET /api/v1/db/rated`, or `POST` with `moods` while filtered, a bare
    array either way, highest rating first. Hidden for a peer, as
    Playlists and the play lists are (`db/rated` is off the federation
    allowlist, and a guest has no ratings there); while hidden its row and
    its `Act::Nav` go nowhere. **The nav at 30 rows**: Files 2 · Search 3 ·
    LIBRARY 5 · Albums 6 · Artists 7 · Genres 8 · Recent 9 · Rated 10 ·
    Playlists 11 · Last played 12 · Most played 13 · TOOLS 14 · Auto DJ
    15 · Sonic path 16 · Discover 17 · Settings on the content's last row.
    At 100×24 with the footer on that last row is 17: there, and only
    there, the LIBRARY heading through Discover rise one row into the
    blank under Search (LIBRARY 4 … Rated 9 … Discover 16), so Settings
    keeps r17 alone. At 24 rows with the footer off, and at 30, nothing
    moves.
38. **Empty, and the counts**: an empty answer under a mood filter reads
    `(nothing here)`, as the text filter's miss does — never "(no albums
    in the library)" or "Nothing here yet", which would call the library
    empty. The opener and the strip are drawn before any empty or loading
    words, so the way back is always on screen (the wall's first-visit
    "loading albums…" alone comes first). The counts read their plural
    families in every browse room — the wall and a drilled album, the
    lists, Files and Playlists: `1 album`, `1 item`, and ru's and pl's few
    and many forms. Under a mood filter the count is the filtered list's
    own; `n of m` belongs to the text filter alone.

## Wording

| String | Key |
|---|---|
| Artists · Genres · Recent (the nav rows, exist) | `gui.nav.artists` · `gui.nav.genres` · `gui.nav.recent` — the record's `browserArtists` / `browserRecent` |
| Recently Played · Most Played (the webapp's nav entries) | `gui.nav.last_played` "Last played" · `gui.nav.most_played` "Most played" — the nav column is twelve cells wide, so the record's words shorten in every locale; the TUI's root menu keeps the record's "Recently Played" / "Most Played" |
| n items · n albums · n of m | `gui.files.items_*` · `gui.alb.count_*` (plural families, `%{count}`; 2026-10-31, the one-form `gui.files.items` and `gui.alb.count` retired) · the bar contract's |
| (nothing here) (a filter's empty answer, exists) | `gui.files.empty` |
| listing… (exists) | `busy.listing` |
| Nothing here yet | `gui.lib.empty` — the record's `browserEmptyList` |
| Singles | `gui.lib.singles` — the record's literal `SINGLES`, in a word |
| Jump to %{letter} (the strip's tip) | `gui.lib.jump_tip` |
| Tags · Generated · Both | `gui.lib.genre_tags` · `gui.lib.genre_generated` · `gui.lib.genre_both` — the phone's `genreSourceTags` · `genreSourceGenerated` · `genreSourceBoth`, the same words in every locale |
| Genre source (the switch's tip) | `gui.lib.genre_source_tip` — the phone's `genreSourceLabel` |
| mood (the opener) · Filters (the modal's title) | `gui.moods.mood` — a mood noun, lower-case as the text buttons are; wants a translator's look · `gui.moods.title` — the phone's `moodFilters` (2026-10-31: `gui.moods.filters` and `gui.moods.tip` retired) |
| Happy · Neutral · Sad · Relaxed · Aggressive · Instrumental · Vocals · Acoustic · Party · Danceable | `gui.moods.happy` … `gui.moods.danceable` — the phone's `moodHappy` … `moodDanceable` |
| clear | `gui.moods.clear` — a text button, lower-case as "more options" is |
| Strict · Standard · Loose | `gui.moods.strict` · `gui.moods.standard` · `gui.moods.loose` — the phone's `moodStrict` · `moodStandard` · `moodLoose` |
| SORT · Rating · Newest · Match | `gui.moods.sort` — the phone's `moodSort`, stored upper-case as the kit's group labels are · `gui.moods.sort_rating` · `gui.moods.sort_newest` · `gui.moods.sort_match` — `moodSortRating` · `moodSortNewest` · `moodSortMatch` |
| Done (`[ Done ]`) | `gui.moods.done` — the phone's `done` |
| Rated (the nav row) | `gui.nav.rated` — the phone's `browserRated`, twelve cells in every locale |
| ↑ ↓ ← → move · Space choose · Enter or Esc done (the modal's tips) · o mood (the rooms' tail) | `gui.tips.moods` · `gui.tips.mood_key` |

## Out of scope here

- **Local Files** and downloads, the **file explorer** (the Files room).
- **Rating a song from the GUI**: the Rated room lists the server's
  ratings; giving one is the track-actions contract's.
- The **tap-behaviour** setting (clause 14) and the strip's **threshold
  and side** settings (clause 10).
- **More actions** on a row, **go to artist / album**, ratings — the
  track-actions contract.
- **Disc headers** in the drilled album (the record's album detail groups
  a multi-disc album under "Disc n") — the Albums room's, not these
  rooms'; logged so it is not forgotten.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| The drawer's Library entries | The nav rows Artists · Genres · Recent; each `Act::Nav` opens the root node fresh (`open_library_node(node, true)`) and points the App's tab at Library, the wall's way. Artists and Genres keep their answered list for the session (`App.artists`, `App.genre_list`, beside `App.albums`) and seat a return from it; the row chosen again while its room is up asks afresh |
| The webapp's side-nav entries Recently Played / Most Played | Two more rows at the LIBRARY group's end, Last played · Most played (2026-09-23), hidden for a peer like Playlists; no digit, so the pointer's; the heading is the nav label; the "Get last [n]" box is not ported — a fixed hundred, as Recent. The column grew to fourteen rows, so Files and Search now stand together at its top — fifteen since 2026-10-05, the Discover room (the log) |
| A list of DisplayItems | The App's Library pane drawn by `draw_pane_rows` (rows, the cursor while the keyboard holds it, hover verbs, kit scrollbar), under `draw_bar_controls` — the Files room's grammar with the pane swapped |
| The album grid for an artist | `albums.rs` generalized: the wall draws whichever album list the App holds for the view on screen — `App.albums` for the root wall as today, and the artist's albums kept beside it for the `Artist` node — with the cells, pages, cover slots and keys shared |
| The vertical letter strip | A one-line `# A B … Z` row under the bar's controls (a 22-row pane cannot stand 27 letters), a kit widget: each letter a click target, dim when absent, the row's tip "Jump to X"; a jump sets the list's first visible row (the pane's scroll) or the wall's page |
| `getSubText` (the artist under a title) | The track row's `display_name` already reads "Artist - Title"; no second line |
| The singles bucket | `album_songs` grows an `Option<&str>` album so a name-less album asks with an empty `album` (`""` — the server reads it as the webapp's `null`) |
| Loading / empty | `busy.listing` / `gui.lib.empty` |
| The phone's mood sheet (mstream_music #213, #218: the B bottom sheet with its Strictness and Sort, the grey strip with Clear) | A kit modal over the room (direction C, 2026-10-31): the sheet's choices on a three-column grid, its draft applied on close — Done, Enter, Esc or `o` alike, where the sheet's dismissal keeps nothing — then one ask, as the seeds modal; the strip under the controls is the phone's grey strip, its Clear the `[X]`. `LibraryNode::{Albums, Artists, Recent, Rated}(MoodFilter)` carry the wire filter, so an answer asked under another is dropped by the drill's stale-reply rule; the App keeps each song's own and Match places beside the pane (`song_ranks`) and re-orders in hand (`sort_songs`) |
| The phone's Tags · Generated · Both `SegmentedButton` before search (mstream_music #211) | Three toggle words right-aligned on the controls line where a track list's verbs go, the Search room's chip grammar (the lit one in the ok colour); the filter keeps the line's lead. `LibraryNode::Genres(GenreSource)` and `Genre(name, GenreSource)` carry the kind, so a list of the other kind landing late is dropped by the drill's stale-reply rule; `App.genre_list` keeps the kind it was asked with beside the list, and `load_genres` makes the fall back to the other kind one round trip more |

### What the tests pin

Each room fetches its root on entry and draws the pane — Artists and
Genres on their first, a return seating the session's copy at the root
with no request, the room chosen again asking afresh; the genres row
shows its count; Recent asks with the limit and shows track rows with the
verbs; an artist row opens a wall of its albums with the artist's header,
and a card opens the tracks with the artist as Back; the singles card;
the strip appears at 25 rows and not at 24, dims absent letters, jumps
the list, snaps from a dim letter, follows the filter, and is absent on
Recent; the bar's verbs are absent on artists and genres and present on
tracks. The play lists ask with the limit, draw track rows with the verbs
and no strip, and hide for a peer. The genre source: the switch sits at the
controls line's right with the filter leading it, and lights the kind on
screen; a choice asks again in place, is remembered, rides into a genre's
songs, and comes back with the room; one kind, a server without the filter,
a genre's songs and an open filter draw no switch; a reply of the kind left
behind is dropped. The mood filters: the wall's opener ends at x51 and the
lists' stands two cells after the name, the count ending at x62 (x95 at 80
cells, the wall's opener at x84); `o` opens the modal with the hand up on
Happy, a click with it stowed, and the first arrow only shows it; the box
at x17–x82, r10–r19 (r9–r19 with SORT; r6–r16 at the 24-row floor, clear
of the bar), banded at x24–x89 with the queue folded, the title on its
border, the columns at x19, x45 and x67; Done asks once under the draft
with the threshold inside `moods`, the rows standing drawn until it lands,
a late answer under the old filter is dropped, a failed ask puts the choice
back and Done asks again, a lone level asks nothing and rides with the first mood, a Sort
alone re-orders in hand, `clear` keeps the Sort and moves the hand to
Done; the pointer lights a word and leaves its glyph's colour, the hand
winning; the room is inert under it, Ctrl-C still quitting; the strip takes
r4, its `[X]` lets everything go, and the letter strip goes under it; an
empty answer reads `(nothing here)` with the opener and strip standing; a
peer or a server without scores gets no opener, strip or key, its choice
kept; Rated sits at r10 after Recent (r9 under the floor's lift), lists the
rated songs, asks on every visit and hides for a peer; under Match a
filtered list lands best first. In every locale the modal fits its 66
cells, the board's strip its 46, line one keeps two cells between its
halves (a four-digit album count under a pager up to four digits,
five-digit lists), Rated fits the nav's
twelve cells and the tips lines their 99; the counts read their plural
forms (ru 2, 5, 21; pl 1, 2, 5; ja one form).

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
- **2026-09-23 — Last played and Most played**: the webapp's two stats
  lists, ported as Recent's siblings (clauses 21–24). Its "Get last [n]"
  box is not ported (the fixed hundred, as Recent); the labels shorten to
  the nav column's twelve cells; both hide for a peer, whose guest has no
  play counts on the peer. The reporting the lists feed on — a play
  session posted from the player — landed the same day (clause 24,
  docs/ux-contracts/play-reporting.md).
- **2026-09-26 — No row lit until the keyboard walks** (clauses 6 and 13,
  the list row of the translation table): the rooms lit their first row
  on arrival and a click lit the row it opened or queued, since the
  shared pane's cursor was drawn as is. The cursor now shows only while
  the keyboard holds it — the kit's list-cursor law (docs/ui-kit.md) —
  and the wall's cell cursor follows the same law: no cell lit until
  ↑ ↓ ← → pick it up, a card's click stows it.
- **2026-09-26 — The review's fixes.** A room open through a switch or a
  late connect asks the new server (its opener had run against the old
  one), and a peer's hidden rooms yield to Files (clause 23). The Albums
  wall stands over its own drill on a return from another room, seated
  from the cache (its verbs, count and Back ran on the other room's list).
  The wall's keys and clicks use the frame's own geometry, a row lower
  under a pick's banner (a click opened an album a page away). Another
  artist's wall starts on its own first page. The drilled rooms take `N`
  and `P` (clause 14), read "Nothing here yet" after a failed listing
  instead of "listing…" forever, and their `◂` is Back itself. A peer's
  heading says read-only (clause 16). Back reveals the parent's cursor
  (clause 6). The art cache holds a wall's page plus the queue's rows
  (ninety covers over a cap of sixty-four re-asked for every frame).
  Left for later: a reply from the previous server can still land on the
  new session's same-named list — a session generation on the library
  and search commands is the fix; the search query is cleared on a switch
  meanwhile.
- **2026-09-26 — The follow-up: a session generation.** The library and
  search asks carry the App's session generation and their replies echo
  it; a reply under another generation — the server the session has since
  left — fills nothing, whatever same-named list the new server has. A
  switch also drops the submitted-search marker, so no stale search is
  wanted either. The art cache lets go of its oldest cover that neither
  the playing track nor a queued row needs (clause 6), instead of clearing
  the lot at the cap.
- **2026-09-28 — Artists and Genres are kept for the session** (entry
  point 1; performance audit #99). Every return to either room asked for
  the whole list again — 0.25-0.6 MB and up to a second over a relay for
  a big library's artists — though it changes only with a rescan. The
  answered list is now kept beside the Albums wall's and a return seats
  it: still a fresh drill at the root, as the flows say, without the
  request. Choosing the room already up asks again, the way to see a
  rescan (the wall has none yet). Recent and the play lists ask every
  time; a new server forgets both, and so does another account signing
  in on the same one, which may see other libraries.
- **2026-09-28 — The art cache spares what is on screen** (performance
  audit #91): the playing track's cover, the wall's page, the queue
  panel's rows and the open action sheet's cover, where it spared every
  queued row. The queue keeps its played rows and Auto DJ only appends,
  so a long session grew the cache past its cap one album at a time. A
  queued row scrolled back into view after its cover went is asked for
  once more.
- **2026-10-05 — Fifteen rows: the Discover room joins TOOLS**
  (docs/ux-contracts/discovery-page.md, clause 2). *Superseded 2026-10-31:
  the map is clause 37's, sixteen rows with Rated.* The column's map was
  Files 2 · Search 3 · LIBRARY 5 · Albums 6 · Artists 7 · Genres 8 · Recent
  9 · Playlists 10 · Last played 11 · Most played 12 · TOOLS 13 · Auto DJ
  14 · Sonic path 15 · Discover 16 · Settings on the content's last row (17
  at 100×24 with the footer on, 18 with it off). The TOOLS group moved up a
  row into the old gap at 13, so the floor still fits the column with
  nothing shared. Discover hides for a peer as the play lists do (clause
  23's reason: a guest has no history there), and is the thirteenth room —
  last in `NAV`, no digit, `G`.
- **2026-10-28 — The genre source** (clauses 25–29; mStream #716,
  mstream_music #211). The switch is right-aligned on the controls line
  (the user's call), where a track list's verbs go — a list of genres
  never has any — and the filter keeps the line's lead, as in every room.
  The list is asked for with `POST /db/genres` and the kind in the body,
  where it was a GET: a peer's proxy forwards a body but no query string.
- **2026-10-31 — The mood filters as an overlay; Rated** (clauses 30–38,
  mStream #1099; direction C, the user's pick of three, with both judges'
  fixes). The inline panel under the controls line — the `filters ▾`
  toggle with its dot and chevron, the list pushed down by the panel's
  rows — gives way to a modal over the room that edits a draft and asks
  once on close. The opener is the noun `mood`, not `filters`, which sat
  over line two's `/ filter` (in zh the same word, 筛选), and has no
  tooltip; its key is `o`, not the direction's `F` (the house rule against
  a capital whose lowercase is a key, as the MP3 tab moved `F → K`). It
  stands at a fixed column — two cells after a list's name, ending x51 on
  the wall — so a changing count never slides it under the pointer, and
  the count is never dropped (browser-top-bar clause 3): where a pager of
  three digits or more, or a five-digit count in a long locale, pulls the
  two together, the count's words are clipped by cells, its digits whole,
  two cells before the opener. The
  level (Strict · Standard · Loose) and, on Recent and Rated, a Sort by
  `moodMatch` join the choices; a level alone sends nothing. The strip
  under the controls names a standing filter and lets it go with `[X]`;
  green means only "a mood is on", so the level's word and the level and
  Sort radios wear TEXT and ACCENT. The modal's title is on its border and
  nothing is on the bottom border (minimal text), the resting Match has no
  tooltip. **Rated** joins the LIBRARY group, so the nav runs sixteen rows
  and, at 100×24 with the footer on only, the block from LIBRARY to
  Discover rises a row (clause 37). The lists' counts end at x62, a cell
  in from the content's last like every right-hand item of line one, and
  are measured in cells, not chars (a CJK count ran a cell long); they
  read their plural forms (`1 album`, not `1 albums`). An empty answer
  under a filter reads `(nothing here)`. The rows stand, drawn, until a
  filtered answer lands (#62 emptied the pane first), so the verbs act on
  the rows on screen; an ask that fails, or a drill from those rows, puts
  the room's choice back to theirs. #62's latent leak is fixed: a
  stored filter was sent to a peer's lists; what is sent is now always
  gated. The key-hint tail ` · o mood` keeps the kit's hint law with no
  tooltip; it is the one piece the user may veto.
