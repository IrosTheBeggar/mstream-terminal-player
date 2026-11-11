# Add from YouTube

| | |
|---|---|
| **Design of record** | For this surface's look and its words: the canvas "Add from YouTube — webapp & desktop" (claude.ai artifact `7Th46vjDNSJPuSAPZhPoqk`), its *Terminal player GUI* page — boards `TermMain` (T1–T4), `TermRoomStates` (A1–E3), `TermFiles` (F1–F6, C1–C4, the door's row 2 at 80 and 47 cells), `TermList` (L1–L4, F1, S1–S4, N1–N4) and `TermWhere` (W0–W5), cell-exact, each frame's Keys and Mouse lines part of the spec — and its notes `termDecisions`, `termQuestions` and `termBuild`, which win over the boards (the user answered the open question 2026-11-03). **Amended 2026-11-09**: the owner moved the way in from Settings to the left nav (TOOLS › `Add YouTube`), which supersedes the boards' Settings frames — T1, and S1–S4 — and the `◂` on every room board (see the log). **Amended 2026-11-10**: the owner approved the redesign canvas "Add from YouTube · GUI" (claude.ai artifact `JL4aG7b1Nut1YVSqLJcojQ`; boards Main, Empty, After, Min, Wide, First, Reading, Refused, Cleaned, Clash, FailedLookup, Tags, SavesTo, Format, Files — and Today, the room it replaces), which supersedes the first canvas's `TermMain`, `TermRoomStates` and `TermList` boards, the window frames of `TermFiles` (F2–F6, C1–C4) and `TermWhere`'s W0–W3: the room, the Files window, the list and the place picker are the new canvas's (see the log). The first canvas still stands for the Files door (F1 and its spot), Choose a folder (W4, W5), the bar's notes (N1–N4) and every word and rule the new one does not redraw. For the behaviour and the record's words: the mobile app, `mstream_music` @ `origin/master` `1d4e8974` (PR #231) — `lib/singletons/ytdl_hub.dart` (the job hub, #231's `priorEntries` snapshot and send-order guard included), `lib/util/ytdl_{api,errors,filename,formats,guess,layout,link}.dart`, `lib/objects/ytdl_job.dart`, `lib/screens/ytdl/*.dart`, the `ytdl*` keys of `lib/l10n/app_*.arb`. Where the phone has no word: the webapp's port of the same rules, mStream PR #1123 (`claude/ytdl-webapp` @ `190f4325`) — `webapp/alpha/ytdl-core.js`, `webapp/locales/*.json` (`ytdl.*`). |
| **Server API** | **A server with the new job list** (the ping's `ytdl >= 1`, also `features.ytdl` on `GET /api/`; mStream #1123): `GET /api/v1/ytdl/metadata?url=` → `{title, artist, album, year, thumbnail, fileStem, isLive, liveStatus, durationSec, channel}`, a failed lookup a bare 500 · `POST /api/v1/ytdl/ {directory, url, outputCodec, metadata?: {title, artist, album, year}}` → 200 `{message, id, fileName}`; **409 `{error, reason: 'exists', fileName}`** refused before anything downloads; 403 Uploading Disabled; 404 a library or folder; 500 "FFmpeg is not available yet" / "yt-dlp is not installed" · `GET /api/v1/ytdl/downloads` → the caller's jobs, newest first, `{id, pid, url, directory, outputCodec, status: downloading \| complete \| error, startTime, endTime, fileName, filePath, downloadId, error, reason}`, `reason` ∈ exists · too-big · too-long · live · unavailable · null, settled ones kept 24 h · `GET /api/v1/discovery/downloads?plugin=ytdl&limit=&before=` → the account's record of what came in (the list's songs, Show earlier). **Older servers** (no `ytdl`): 6.30–6.33 answer the record route unfiltered (the "full" tier: a global tracker keyed by pid, settled entries kept 30 s, a POST answering `{message}` alone, a name clash failing after the download); before 6.30 the record route is Express's 404 (the "older" tier: an add whose file name is already in the folder re-tags that file). **Both**: `POST /api/v1/file-explorer/mkdir {directory}` (403 for an account that can't make folders — the ping's `noMkdir`), `POST /api/v1/file-explorer {directory}` (a folder's names, for the clash and Choose a folder), the ping's `noUpload`, `transcode`, `noMkdir` and `supportedAudioFiles` (a map of format → on, or a list). |
| **Already in this repo** | The torrent room's shape — every call a one-shot `Client` on its own thread folded back between draws (`gui/torrent.rs`) — so no `ApiCmd` and no wasm worker change; Keep a copy's window, its band, its borders and its notes (`gui/keep.rs`: `window_rect`, `open_window`, `draw_borders`, `set_note`, `fit_note`), Where copies land's picker grammar and the layout engine it mirrors (`api/types.rs`: `validate_layout`, `layout_folders`, `sanitize_segment`), the kit's tall primary, line editor, `ListView` and `scroll_items`. **Built 2026-11-09, phase A** (the logic): `src/api/ytdl.rs` — the calls, `YtdlCallError` (the status, the server's words, its `reason` and `fileName`, a timeout told from a refusal and from no way there), the shapes, the tier; the ping's `noMkdir`, `supportedAudioFiles` and `ytdl` read leniently (`api/types.rs`); `ServerEntry`'s `ytdl_library`, `ytdl_layout`, `ytdl_layout_kind`, `ytdl_format` and the disposable `ytdl.json` beside `config.toml` (`config.rs`); `src/ytdl/` — the pure rules, a port of the webapp's `ytdl-core.js` and the phone's `ytdl_filename.dart` (links, the tag guess and its clean-up, layouts, presets and bracket traps, formats, yt-dlp's restricted file name, the clash, failures and why Add is off), every word a `Label` (a locale key) — and `src/ytdl/jobs.rs`, the job tracker, sans-I/O over an injected transport, clock and store (the phone's hub for the older tiers, the job id for the new one). **Phase B** (the GUI): `src/gui/ytdl.rs` (the room, its form's rows, the From YouTube list in it, the cover, the paste seam), `src/gui/ytdl_sheets.rs` (the sheet's frame, Edit tags, Format — 2026-11-10), `src/gui/ytdl_here.rs` (the Files door and its window, the Saves here sheet, the re-list), `src/gui/ytdl_where.rs` (Saves to, Choose a folder); the shell's hooks in `gui/mod.rs` (the nav's `NavId::YouTube` row — a Settings doorway until it moved, 2026-11-09 — `Host::paste`, `TermEvent::Paste`), `Face::takes_paste` in the window flavour, the panic hook's paste off (`tui/mod.rs`); the `gui.ytdl.*`, `gui.nav.add_youtube` and `gui.tips.ytdl_*` strings in the ten locales. **Rebuilt 2026-11-10** to the redesign canvas: the room's form (`form_rows`, `draw_row`, shared with the Files window), the song's card and its cover (`YtUi::clipboard` and `Work::Thumb` the two new seams: the window flavour's clipboard for `Paste`, YouTube's image host for the thumbnail), the list drawn in the room, the three sheets. |
| **Target surface** | The GUI player, both flavours: a Library nav room under TOOLS (a room off Settings until 2026-11-09), and a door on Files. **The classic TUI gets nothing**, and there is no `gui --youtube` seam in v1. |
| **Status** | Designed 2026-11-03 (the canvas, the brief's decisions, the user's answers); phase A and phase B built 2026-11-09; this contract written from the canvas notes that stood as the contract during the build; the room moved from Settings into the nav the same day (the owner's call, see the log); the room, the window, the list and the picker rebuilt to the redesign canvas 2026-11-10 (the owner's "build it"; its open decisions built as drawn — see the log). The pty smoke (`smoke/gui/scenario_ytdl.py`) passes against a new-tier server (mStream with #1123 and its fake yt-dlp) and an older one (6.32.3), before the move and after it. |

## Intent

Paste a YouTube link and the song lands in your library, tagged and in the
folder you would have put it in: the player reads the video, guesses the
tags as a person would — the title cleaned of upload noise, the artist split
off it, every cut undoable — shows where the file goes, and asks your own
server to fetch and convert it. The server does the work; the player
follows it to the end, says what came of it in the server's own words, and
keeps a list of what came in. Nothing is drawn where it cannot work.

## Entry points

1. **The nav row `Add YouTube`** — the left nav's TOOLS group, last, on
   row 19 under `Add torrent` (TOOLS label 14, Auto DJ 15, Sonic path 16,
   Discover 17, Add torrent 18, Add YouTube 19; the block rises on short
   terminals — clause 2). It opens the room as a Library nav room, the
   Auto DJ room's shape: no way back, the nav column beside it. No digit
   and no letter. Settings has no doorway (see the log).
2. **Files, on a folder below a library root** — line one of the browse
   bar carries the door word `add from YouTube` three cells after the
   crumb; it, or `y`, opens a 76-wide window that adds the song right
   there (`[ Add here ]`).
3. **Nothing else** in v1: no command-line seam, no TUI. (`Paste`, beside
   the link, reads the clipboard in the GUI's own window — clause 11.)

The row and the door stand only under the gate (clause 1).

## States & flows

The room is one form, revealed by what the link came to, over the list of
what came in: **empty** (the field has the keys; the limits; the list) → a
link typed or pasted → **reading** (8 s on, "still looking", with a way to
type the details) → **found** (the song as a card with its cover, where it
saves and as what, the primary) or **failed** (the lookup's own words, a way
to type the details, Add working without them) or **timed out** (try again)
— a late answer offering its details over tags already typed → the
**press** (`Adding…`, the form's keys waiting) → the server answers:
started, and the form **starts over** for the next link, its job heading
the list; or refused, and the form stays with the server's words over the
primary. From the press on the job is the tracker's: **adding** → **added**
(the note says so; the list keeps it) or **failed** (its row says why and
offers a way on), with **starting…**, **not sure it started**, **not sure
it finished** and **not found on the server** where the server's answers
leave it unsure. Three sheets stand over the form when asked for: **Edit
tags**, **Saves to** (the Files window's **Saves here**) and **Format**;
Choose a folder is a window of its own. The Files window is the same form
over a folder, with no list. Kept: per server in `config.toml` (library,
layout, format); the jobs in `ytdl.json`; nothing else outlives the
session.

## Behavior contract

### The gate

1. **Hidden, never greyed** (the phone's gates). The nav row and the
   Files door stand only while the session is connected to **its own
   server** — never a federated peer, which is read-only (multi-server
   clause 26) — whose ping says the account may add files (`noUpload`
   false; a ping that does not say grants nothing), that the server has
   audio conversion (`transcode`), and that names a library. The door
   also needs Files on a folder **below** a library root (a root itself
   takes no song by this door), nothing over it, the header's dropdown
   shut and the browse bar's filter not taking text (`y` types there). A
   gate that closes under a window closes it.
2. **The row's place**: `NavId::YouTube`, last in `NAV`, after
   `NavId::Torrent` — rooms past the digits are the pointer's, and a row
   that comes and goes never renumbers the others. It closes the TOOLS
   group, which packs the rooms it shows: with all five it stands on 19,
   two lines higher on a server with no sonic path and no Discover (Auto
   DJ 15, Add torrent 16, Add YouTube 17); absent, Add torrent closes the
   group and nothing stands under it (discovery-page clause 2). Where
   Settings, on the content's last row, would meet the last tool, the
   rooms rise only as far as they must — into the blank under Search,
   then over the TOOLS heading, then over the LIBRARY heading — so with
   all five the row stands on 18 at 100 × 25 with the key hints off or
   100 × 26 with them on, on 17 at 100 × 24 with them off or 100 × 25
   with them on, and on 16 at 100 × 24 with them on: never on Settings'
   row or under it. The
   label fits the nav's twelve cells in every locale (CJK two a
   character), and reads `▸ Add YouTube` while the room is up. A gate
   that closes while the room is up and the session still connected sends
   the room to Files as its row goes, the Discover room's way; a
   connection that drops closes nothing — the room says it can't reach
   the server (clause 14), its row hidden meanwhile, so no row is lit.
3. **The row carries no state** — no word, no glyph, no count: the jobs'
   news is the room's list (clause 40), the bar's note (clause 70) and the
   Files door's spot (clause 30). The Settings doorway's one word (S1–S4)
   went with it, and the title row's word with the redesign (see the log).

### The room

10. **One block, centred** (Main, Wide): everything the room draws stands
    in a block as wide as the room up to 56 cells — 47 with the queue open
    at 100 columns, 56 in the room's middle once the room is wider — whose
    right-hand words end on its last cell. Top to bottom: the title
    (clause 22), the link's band, the link's own lines, what the link came
    to, the two rows, the file's and the folder's lines, the primary, the
    list (clause 40). Short of rows (Min): the primary becomes a slab on
    the title row (clause 20), then the blanks give way for the list's
    three rows — the one under the title, the form's from the bottom up,
    the one over the list — and what still does not fit is cut at the
    room's last row, the list first.
11. **It opens with the link field holding the keys** (Empty; see the log),
    on every opening from its row — unless a press is still out, which
    keeps the form quiet (clause 20): the card ACCENT, the caret, then the
    DIM placeholder "Paste a YouTube link"; `[X]` (DIM, DANGER under the
    pointer) clears a link, the keys staying. Typing edits the link; Esc
    gives the keys back (the form's keys again); Esc once more leaves
    nothing — a nav room has no way back, the Auto DJ room's rule — and is
    the shell's. A click on the card takes the keys; ↓ leaves the field
    for the list (clause 42).
    **A paste lands in the link field, whatever has the keys.** While the
    room (or the Files window) is up and nothing stands over its form the
    terminal is in bracketed paste; a paste replaces the link — its first
    non-blank line, controls and invisible format characters left out, at
    most 4096 characters — and looks it up at once, the same link pasted
    again asking again; found, the field gives the keys back. While a
    press is out it is dropped. Over a sheet, Saves to or Choose a folder
    a paste is that surface's field's, typed; so is one while another's
    surface stands over the room — the sign-in form a lapsed token opens,
    Add a server, the header's server list, a dialog.
    **`Paste`** (Main; the window's too): a DIM tall frame one cell after
    the link's card, BRIGHT under the pointer, putting the clipboard's
    text through the same paste — drawn only where the build can read the
    clipboard, which is the GUI's own window (`YtUi::clipboard`). A
    terminal cannot, so there the card takes the band's whole width and a
    paste is the terminal's own (see the log).
12. **A typed link is looked up once the typing rests** (400 ms), or on
    Enter. A changed link starts the form over; an answer for a link no
    longer in the field is dropped; a lookup is bounded (30 s).
13. **The link's own lines**, on the row under its band — which is blank
    while it has none: GOLD refusals — "That's not a YouTube link. Paste
    one from youtube.com or youtu.be." (Refused) and "That's a playlist.
    Open one video in it and copy that link.", the field keeping the keys
    — the DIM note "Just this video, not the playlist.", and the
    live-stream warning — on an older server the phone's hedge. A failed
    lookup's words stand there too (clause 14). Every link is sent as
    `https://<host>/watch?v=<id>` (a youtu.be, shorts, live or embed link
    rewritten; the server refuses a youtu.be) — and never shown so.
    **Before any link** (Empty): the limits, DIM, wrapped — "One video per
    link · the server stops at 500 MB or after an hour · no live streams
    or playlists" — the room's alone (the window has no room for them).
14. **The lookup** (Reading): the cover's slot empty, "Reading the video…"
    ACCENT beside it; after 8 s "Still looking — YouTube can be slow."
    under it (two rows at most) and `type the details ▸` on the slot's
    last row, which opens Edit tags on an empty form. The two rows stand
    as before a link and the primary is there, off. A failed lookup (a
    bare 500, an unreadable answer; FailedLookup) reads "Couldn't read
    this video's details. It may be private, age-restricted or blocked for
    the server." in GOLD under the band, `type the details ▸` under it,
    and **Add works without them** — the file gets the video's own; a
    timeout "The server took too long — it may still be setting up its
    downloader." with `try again` and no primary; yt-dlp missing reads so
    and the primary is off; a server the player cannot reach — refused, or
    no way there — "Can't reach {server}. Check that it's running, then
    try again." with `try again`. An answer that lands after details were
    typed shows "Details found for this video" and `use them` at the row's
    end, which fills only the tags left empty.
15. **The song is a card** (Main): its cover's slot, 8 × 4 cells, and
    beside it four lines — the title BOLD with `edit tags ▸` at the row's
    end; the artist; the album and year DIM ("No album" for none);
    YouTube's own line DIM, `YouTube · {channel} · {length}`, as the
    lookup gave them. A blank title tag leaves the file the video's own
    title, which then shows DIM in the title's place. Under the card: the
    source line — "Tags from YouTube Music" (DIM), or GOLD "Artist may be
    the channel name — check it", nothing for an artist split off the
    title; where the title was cleaned, "Cleaned from “…”" — its title cut
    inside the quotes, never the closing one — with `undo` at the row's
    end, putting every cut back (Cleaned). Details typed by hand make the
    same card, without YouTube's line.
    **The cover** is YouTube's own thumbnail for the video's id — its
    `mqdefault.jpg`, asked of YouTube's image host (`i.ytimg.com`) by the
    player itself once the song shows, never by an address the server
    gave: the id's own letters make the address, nothing of the player's
    or the server's goes with the request, and a picture over 2 MB or 10 s
    late is none. The cover is the frame's middle square. Until it is
    here, while the video is read, and where there is none, the kit's
    empty slot stands (a DIM frame); under a sheet or a window it draws as
    the mosaic, as every cover does.
16. **Edit tags** (Tags; `edit tags ▸`, `type the details ▸`, a trap's way
    on; `e`, `t`) is a sheet (clause 53): TITLE over a bordered card the
    sheet's width; ARTIST, ALBUM and YEAR over three side by side — the
    card every text input wears, ACCENT with the caret while it has the
    keys, BRIGHT under the pointer, a click taking them; under them, for
    the tag with the keys, where its value came from and what a blank one
    does (a year that is none says so in GOLD); `[ Done ]`. Tab and ↓,
    Shift-Tab and ↑ walk the tags. The tags are the form's own as they are
    typed: Enter, Esc, `[ Done ]` and a click outside all only close the
    sheet, and Enter in the room adds. A year is four digits or blank; the
    tags go to the server trimmed, a blank one not at all.
17. **`Saves to`** (Main): a row — the label TEXT, its value ending two
    cells before the `▸` at the row's end, the whole row the control
    (BRIGHT under the pointer; `d`). Before a song the value is what new
    songs go by, DIM (`Music · Artist / Album`); with one it is the place
    this song gets — the crumbs TEXT between DIM `›`, the library first, a
    folder the add will make followed by a DIM `new` — giving up whole
    folders from the left behind `…`; a layout that lands nowhere is a DIM
    `—`. It opens the Saves to sheet (clause 50). An account that can't
    make folders, with a folder new or unchecked: a GOLD line under the
    two rows, and under it `save to {the deepest folder there} instead` ·
    `choose a folder ▸` (wrapped where they pass the block); Add off.
18. **The file and the folder's names** (asked of the server once the tags
    rest): on a server from 6.30 a name like one already there is a GOLD
    "Something like “One_More_Time.mp3” looks to be in this folder
    already." under the two rows, and the primary reads **Add anyway**
    (Clash) — another format, a row away, is the other way on; before 6.30
    the server would re-tag that file, so the phone's words say so and Add
    is off. A title whose letters yt-dlp drops says the name it will get
    (DIM). The name is the lookup's `fileStem`, else predicted from the
    title as yt-dlp's `--restrict-filenames` makes it.
19. **`Format`** (Main, Format): a row like Saves to, its value the format
    the song goes as, DIM (`f`). It opens the Format sheet: "Best audio
    the video has, saved as", then only what the server's
    `supportedAudioFiles` has on, in the record's order, as radios — the
    dot ACCENT on the one in the making, its name BOLD, what each is good
    for DIM beside it (wrapped in its column where a language's words are
    longer) — "Remembered for {server}" and `[ Done ]`. The dot is the
    choice in the making: ↑ ↓ or a click moves it, Enter or `[ Done ]`
    takes it — remembered per server — and Esc, `f` again or a click
    outside leaves the format as it was; a 400 on the format forgets it.
    A row's Change format and retry opens the same sheet for that row: no
    one remembers it, and taking a format adds the row again in it.
20. **One primary, Enter presses it**: `Add to library ▸`, a tall frame the
    block's width under the two rows, its label centred — the room's one
    ACCENT frame, BRIGHT under the pointer. Off it is DIM with no `▸`, no
    click, and the reason it is off stands DIM right-aligned over it —
    "Already adding to this folder" — or, for a refusal, the GOLD lines.
    Where the whole form, the list's header and one row of it do not fit
    the room's rows (Min, the 100 × 24 floor), the primary is a one-row
    slab on the title row instead, at the block's right end — ACCENT
    ground, BOLD — the title giving way to it two cells clear (cut with
    `…` where a language's words are longer). Pressed it reads `Adding…`,
    DIM, no click, no hover, and the form's keys wait for the server —
    letters either case, digits, signs, Space, so a link pasted as keys (a
    terminal with no bracketed paste) never reaches the shell's globals —
    every control of the form standing, neither clicking nor lighting; the
    list under it keeps its keys and its clicks, and the tips line names
    those alone. A nav room has no back for Esc to take, so Esc and Tab
    are the shell's, and the nav's rows are the way on; the row clicked
    again opens the room with the form still quiet.
21. **The press**: never twice for one link and folder while one is open
    (the tracker is asked first). A folder the place would make is made
    first; the add is sent **once, ever**. The server's answer:
    - **started** — the form starts over in place, its job heading the
      list (After), the note line saying "Adding “One More Time”…"
      (ACCENT);
    - **refused** — the form stays and the words stand GOLD over the
      primary: a 409's "“One_More_Time.mp3” is already in this folder, so
      nothing was added." — standing for the clash warning it confirms
      (Clash's line goes) — "The server can't convert audio right now. Try
      again later, or ask its admin." (the ping asked again); a library
      gone, uploads off, the server's own words otherwise; Enter tries
      again;
    - **no connection** — "Can't reach {server} — check that it's running,
      then try again.";
    - **no answer in time** (a timeout, a lost answer, a 504) — never
      sent again: the form goes on as though started and the job is
      looked for (clause 60), its list row `starting…` until it is found,
      `not sure it started` if two minutes find nothing.
22. **The title row**: `Add from YouTube` (BOLD) at the block's first
    cell — no `◂`, the nav column being right there, and a click on it
    does nothing. It carries no word of the jobs (the list is on the page)
    and nothing else but the primary's slab at the floor (clause 20).

### The Files door and its window

30. **The door word** (F1): `add from YouTube`, ACCENT (BRIGHT BOLD under
    the pointer, no tooltip), three cells after the crumb on line one of
    the browse bar; the count at the right as ever. Its spot carries the
    window's job once one is pressed: `adding` (ACCENT), `✓ added` (OK, 8 s,
    then the door again), `! failed` (GOLD — a click opens the room, where
    its row and its ways on are). `y` opens the window under every word, **no letter on screen**
    (see the log). At 47 cells the count goes first, then the crumb clips
    from the left (`…Music/Daft Punk/Homework`, at 12 cells at the least),
    then the word (see the log). The word steps aside while the window is
    up.
31. **The window** (Files): Keep a copy's shape and place — 76 cells wide,
    centred in the room with its band at x 19–94 (queue folded), over the
    whole frame at x 12–87 with no band when the room is narrower than 80;
    anchored as if 17 rows (row 6 at 100 × 30, row 3 at 100 × 24), growing
    down to the room's last row; "Add from YouTube" on its top border,
    "Esc close" on its bottom one; the room beneath inert (discovery-page
    clause 106). Inside it is the room's own form, in a block two cells in
    from each border: the link's band with `Paste` (clause 11), the song's
    card right under it (the blank under the band is the room's), the
    source line, `Saves here` — `Saves to` once it adds past the folder on
    screen — and `Format`, the file's lines, then the bracketed primary
    `[ Add here ]` at the bottom right, ending on the block's last cell.
    No limits and no list: those are the room's. It opens empty with the
    link field holding the keys ("a paste fills the link and looks it up ·
    typing works too"), the two rows there already. Esc closes it at any
    time.
32. **Saves here** (the row, or `s`) opens its sheet (clause 53): where
    under the folder on screen the song goes, as radios in two columns —
    `(•) Right here`, `( ) Album`, `( ) Artist / Album`, `( ) Custom` —
    the dot ACCENT, ↑ ↓ or ← → moving it, the place it makes on the
    sheet's title row; Custom's field is the bordered card with the
    variables under it (ARTIST · ALBUMARTIST · ALBUM · YEAR, a click
    putting one in at the caret), a refused layout's GOLD line under the
    card; `[ Done ]`. Enter or Esc closes it (in Custom's field they give
    the keys back first). Past Right here the primary reads `[ Add to
    library ]`. Not remembered. Edit tags and Format open over the window
    as over the room, for the window's own song; a sheet over the window
    is the window's width, on its columns.
33. **The press**: nothing moves, only the primary goes off — `[ Adding… ]`
    DIM, no click, no hover; the controls stand and do nothing; Esc closes
    and the press still goes through. The window closes once the server
    starts the job; a refusal (the 409, the server's words) stays inside
    it, and its row is never made.
34. **After** (F4): the door's spot carries the job (clause 30); once the
    song is in, the folder is listed again — its rows stay up meanwhile —
    and the cursor rests on the new row (or on the subfolder it made),
    **lit and held**: the list-cursor law's one exception, the row the add
    made. Not while a filter stands (the listing would wipe it), and not
    when Files has moved on. The note line says it (clause 70).

### The From YouTube list

40. **In the room** (Main, Empty, After): under the form, a blank between,
    the header `FROM YOUTUBE` in DIM capitals at the block's first cell,
    then the rows down to the room's last row — three of them at 100 × 30
    with a song found, every row the room has left otherwise. It has no
    window of its own and no word on the title row (see the log); while
    the room is up what lands in it is **seen** as it lands, which is what
    takes the Files door's `! failed` down. **The first time** (First) —
    nothing adding, nothing ever added, the record read — two DIM lines
    stand for the rows: "Paste a link to add a song to your library." and
    "Songs added from the web app show up here too."; until the record is
    read the header stands alone.
41. **Its items**, in order: the open adds (`↓` adding, then `◷`
    starting…), then those that need a look (`!` failed, `?` not sure),
    then the songs that came in — the server's record, newest first.
    **A job's row**: its mark, its title — the row's, up to two cells
    before the words at its right end — and where it stands: `adding · 1
    min · MP3` (ACCENT), `starting…`, `failed` (GOLD), `not sure it
    started`, `waiting for connection`. A row that needs a look carries
    its words DIM on the lines under it, two cells in, and its ways at the
    right end, TEXT with DIM `·` between (BRIGHT BOLD under the pointer) —
    Try again, Change format and retry (clause 19), Choose a folder, Top
    of the library, Check again, Add again, Dismiss — sharing the words'
    last line where both fit, wrapping onto a second row where a
    language's ways pass the block. **A song's row**: `✓` (OK), the title,
    the folder it is in — its last name, DIM — and when it came in at the
    right end, DIM (`1 h ago`, `Oct 31`), or `file missing`. The title's
    column is the block less the mark and the cells kept for the folder
    and the when: 22 of them at 47 cells and one more for every four the
    block grows (22 and 22 at 47; 29 and 24 at 56).
    **More rows than the room has** scroll under the wheel and the keys —
    an item at a time, no bar — and `Show earlier`, DIM at the right,
    comes after the last song while the record has more.
42. **Keys and pointer**: with the form's keys free ↑ ↓ pick the list's
    cursor up and walk it — the held row an ACCENT slab the block's width
    — and ↓ from the link's field goes there too; ↑ from the first row
    hands the keys back to the link, Esc puts the hand down. The cursor
    stays on its row as the rows move — a row added again lists with the
    open ones, and the cursor goes with it. Enter is the held row's first
    way (a song's: play) and never the primary's while a row is held; `x`
    dismisses a settled job; on a song `a` queues it and `m` hangs its
    list (Add to queue `a` · Show in folder — Files on the song's folder,
    the cursor resting lit and held on the song as clause 34's does, Back
    from there the parent, not an older walk's column). A click acts on
    any way; a ✓ song plays on a click, its verbs `[▸] [+] [⋯]` at the
    row's end under the pointer or the keys' hand, a right click its list
    — hung under the row from the `[⋯]`, right edges level, over the row
    where the room has no rows left under it; the room is inert beneath
    it, a click elsewhere or Esc closing it. The press out, the list keeps
    all of this (clause 20).

### Saves to, and Choose a folder

50. **The sheet** (SavesTo): the `Saves to` row (or `d`) opens it on the
    choice in force — a sheet (clause 53), ` Saves to ` on its top border
    and after it the place this song would get (the crumbs, `new`; a `—`
    for a layout that lands nowhere). Rows: a bracket trap's GOLD lines —
    "No year for this song, so the folder reads “Discovery ()”." — with
    its way on at the first one's end, `add a year ▸` or `edit tags ▸`
    (`y`, `e`), which keeps the choice for this song and opens Edit tags
    on that tag; the libraries side by side only with two or more (the dot
    OK, ← →); the layouts as radios in two columns, the dot ACCENT, the
    chosen label BOLD — Artist / Album (`{{ALBUMARTIST}}/{{ALBUM}}`),
    Artist, Top of the library, Same as Discover (only while the account
    has a Discover destination of its own), Library layout (only where the
    server has templates), Custom — ↑ ↓ walking them in that order;
    Custom's field the bordered card, opening on the layout in force, its
    GOLD refusal under it, then the variables row; `use for this song`
    (`u`) and `[ Save ]` at the bottom right. Esc or a click outside
    closes it.
51. **Save is off until something changed** (DIM, no click, its tooltip
    why: "nothing changed yet" / "fix the layout first"); it keeps the
    choice for the server in `config.toml` (`ytdl_library`, `ytdl_layout`,
    `ytdl_layout_kind`; Same as Discover folds its base into the layout).
    `use for this song` changes this song's place alone. It reads the
    account's Discover destination and **never writes it**.
52. **Choose a folder** (W4, W5; `choose a folder ▸`, `c`, a failed row's
    way): an existing folder, for an account that can't make one — 80
    wide at Where copies land's place, "Esc close" on its border; over the
    room for the room's song or a row of its list, in the Files window's
    place for the window's (which comes back when it closes): `◂ Up to …`,
    the folders, `[ Use this folder ]`; ↑ ↓ pick up the cursor, Enter goes
    in, or uses this folder while the cursor is stowed.
53. **A sheet** (Edit tags, Format, Saves to, Saves here): 66 cells wide,
    as tall as its rows, on the frame's middle rows and a room's window's
    columns (clause 31's rule) — centred in the room where the room is
    wider than it, over the frame's middle where it is not — so it covers
    the block it was opened from, whose cells never show cut beside it;
    over the Files window it is the window's own 76, on the window's
    columns. Its name is TEXT BOLD on the top border, what it holds now
    after a DIM `·` (TEXT, a blank closing it before the border goes on);
    its text starts two cells in from the border; its one bracketed button
    is ACCENT BOLD at the bottom right, ending three cells before the
    border (DIM and no click where it is off). What is beneath is inert;
    a click outside closes it as Esc does. The sheet owns every key while
    it is up, and a paste is its field's, typed (clause 11).

### Following the jobs

60. **The tracker** (`ytdl::jobs::Tracker`): one per server identity and
    account, made once the session has its facts, let go when the session
    moves to another (its calls' answers with it). On a server with the new
    job list a job is followed by the POST's `id`; on older ones by the
    phone's rules — the newest entry for the link and folder that no other
    job holds, that the prior list did not have (#231's snapshot) and that a
    read sent after the POST shows (#231's send order). It polls `GET
    /ytdl/downloads` every 3 s while an add is open, one read at a time,
    backing off to 60 s on failures ("waiting for connection" with no answer
    at all); settles from the entry, else the server's record, else the
    folder; and is never fooled by a wall clock set back.
61. **Kept in `ytdl.json`** beside `config.toml`, per server and account:
    the last 20, a failure kept 7 days; merged on write (another player's
    jobs survive); a corrupt file ignored and replaced, never repaired. A
    press still out when the player closed comes back unsure and is looked
    for.

### Notes, keys, glyphs and words

70. **The note line** (the bar's last row, 30 cells at 100 columns,
    `keep::set_note`): "Adding “One More Time”…" (ACCENT) once the server
    took it; "Added “One More Time”" (OK) when it lands, the folder
    joining — "· Daft Punk › Discovery" — only where the note is wider than
    the head (N4, 120 columns); "Couldn't add “Around the World”" (GOLD),
    clipped with `…` (N3). Once per job.
71. **Tips name a key only where it works**, composed from what the frame
    drew, top to bottom (Main: "a paste replaces the link · e edit tags ·
    d change · f format · Enter add · ↑ ↓ rows" — no Esc: a nav room has
    nothing to go back to); past the footer's 99 cells the last key words
    give way — the paste and Enter stay. With the list's hand up the line
    is the held row's ("↑ ↓ rows · Enter play · a add to queue · m more ·
    Esc deselect"); while a press is out it names the list's alone
    (clause 20).
72. **The page shows words, the tips line their keys** (the redesign's
    rule; docs/ui-kit.md, Key word): a control of the form is its words —
    and `▸` where it opens something (`edit tags ▸`, `type the details ▸`,
    `choose a folder ▸`, `add a year ▸`; not `undo`, `use them`, `try
    again`, which act in place) — DIM, BRIGHT BOLD under the pointer, a
    click doing what its key does; the two rows are controls whole. The
    letters still act — `e`, `d`, `f`, `a`, `t`, `u`, `i`, `c`, the
    window's `s`, the sheet's `y` — **only where their control is drawn**,
    and the tips line names them by their key words (`e edit tags`). The
    page's words are those key words without their key: every locale's
    opens with its letter and a blank, the letter never translated. **The
    room's capitals are its own**: with Shift or Caps Lock `d`, `t` and
    `a` arrive as `D`, `T` and `A`, the shell's Auto DJ, Stats and DJ
    toggle — while the room or any of its sheets and windows is up, the
    press out or not, a capital of the room's letters never reaches the
    shell (a drawn control's capital is that control: `D` is the Saves to
    row), and the sheets and windows keep every key. Try again is `a`,
    never `r` (the shell's Repeat, which reaches it from this room as from
    every nav room).
73. **Glyphs live in code** behind `legacy_conhost` — `▸ ◂ › ▾ ✓ ↓ ◷ •
    ▏ …` and the `! ` of a GOLD line — the locale strings carry words only
    (add-torrent's 2026-09-26 rule).
74. **No counts** (the canvas's "count-free"): not on the nav row, the
    door or the list — a row's minutes and its date are times, not
    tallies.
75. **The server's words, as they are** — a refusal's `error`, a job's
    `error` (the server strips yt-dlp's `[youtube] <id>:` itself) — inside
    the translated frame ("The server said: “…”", "the server said: “…”"
    on a row), never a bare code; a server with no words at all reads
    "Something went wrong on the server." (new) or the hedged reason (older
    tiers: "the server didn't say why — a live stream, YouTube refusing
    the server, 500 MB or an hour").
76. **Never "download" for the add**: the words are "Add from YouTube",
    "Add to library", "Add here", "Add anyway"; the server's own
    capability may be named so ("This server can't download from YouTube
    yet."), the user's act never.

## Wording

English reference; every string through `t!()` in all ten locales (de en
es fr it ja pl pt ru zh). The record's `ytdl*` keys were carried over from
the ten `.arb` files into `locales/*.yml` under `gui.ytdl.*` (`room`,
`key`, `tag`, `word`, `note`, `link`, `lookup`, `server`, `source`, `help`,
`blank`, `where`, `sort`, `format`, `file`, `no_mkdir`, `err`, `list`,
`add`, `here`, `choose`, and since the redesign `tags`, `done` and the
room's `paste_button`, `limits`, `empty_title`, `empty_web`, `saves_to`,
`format`), `gui.nav.add_youtube` and `gui.tips.ytdl_*` — each
language's own words, `{name}` written `%{name}`, the phone's ICU plurals
unneeded (the pages carry no counts). Glyphs (`▸ ◂ › ▾ ✓ ↓ ◷ • ▏ …` and a
GOLD line's `! `) are the code's (clause 73). Where the board trims a phone
sentence the translation is the phone's cut at the same sentence. A key
word is the phone's verb lower-cased behind its letter (`e edit tags` from
"Edit tags"); a row's reason is the webapp's `ytdl.reason.*` / `ytdl.row.*`
without "Failed · ", its sentences joined by " — " or " · " as the board
joins them. The webapp's words stand only where the phone has none:
`where.err.generic`, `err.exists`, Choose a folder's `failed`, the list's
`dismiss`; the layout errors are the phone's own (`discoverDestErr*`,
`ytdlErr*` — its YouTube Where sheet's), and so is the dots' Custom
(`discoverDestPresetCustom`). The limits are the ones the fit tests pin
(What the tests pin).

| String | Key |
|---|---|
| Add YouTube (the nav row, new: within the nav's 12 cells, CJK two a character; `+ YouTube`, the add sign for the verb, in de es fr it pl pt ru — open for the owner, see the log) | `gui.nav.add_youtube` |
| adding · ! failed · ✓ added (the one word on the Files door; the title row's word and `history` went with the redesign) | `gui.ytdl.word.*` |
| Paste (the button beside the link, new — the only new word of the redesign: de Einfügen, es Pegar, fr Coller, it Incolla, ja 貼り付け, pl Wklej, pt Colar, ru Вставить, zh 粘贴; in its frame it leaves the link's card sixteen cells at 47) | `gui.ytdl.room.paste_button` |
| One video per link · the server stops at 500 MB or after an hour · no live streams or playlists (the phone's `ytdlLimits`; wrapped, three rows at most at 47) | `gui.ytdl.room.limits` |
| Paste a link to add a song to your library. · Songs added from the web app show up here too. (the phone's `ytdlEmptyTitle`, `ytdlEmptyWebApp`) | `gui.ytdl.room.empty_title` · `.empty_web` |
| Saves to · Saves here (the phone's `ytdlSavesTo`, `ytdlSavesHere`) · Format (new: de Format, es Formato, fr Format, it Formato, ja 形式, pl Format, pt Formato, ru Формат, zh 格式) | `gui.ytdl.room.saves_to` · `gui.ytdl.here.saves_here` · `gui.ytdl.room.format` |
| Edit tags (the sheet's name; the phone's `ytdlEditTags`) · Done (the sheets' button; the player's own word, as the mood Filters') | `gui.ytdl.tags.title` · `gui.ytdl.done` |
| Best audio the video has, saved as · Remembered for {server} (the phone's `ytdlFormatMenuTitle`, `ytdlFormatRemembered`) | `gui.ytdl.format.best` · `.remembered` |
| add from YouTube (the Files door, new) | `gui.ytdl.door` |
| FROM YOUTUBE (the list's header: `list.title` in capitals, where the language has them) | `gui.ytdl.list.title` |
| Adding “One More Time”… · Added “One More Time” (the note's head, at most 30 cells with the boards' song) · Added “…” · Daft Punk › Discovery (where the note is wider, new) · Couldn't add “…” | `gui.ytdl.note.adding` · `.added` · `.added_in` · `.couldnt` |
| Still looking — YouTube can be slow. · Details found for this video (wrapped, never clipped: the first at most two rows beside the cover's slot — 38 cells at 47 — the second beside `use them` at 47) | `gui.ytdl.room.slow` · `gui.ytdl.room.late` |
| Fix the layout to see where files land (why Add is off; at most two lines at 47) | `gui.ytdl.where.fix_layout` |
| The server took too long — it may still be setting up its downloader. (the phone's, "Try again in a minute." cut) | `gui.ytdl.lookup.timed_out` |
| Something like “…” looks to be in this folder already. (the phone's first sentence) | `gui.ytdl.file.clash` |
| Nothing to fill this folder, so it's named “…”. · No year for this song, so the folder reads “…”. · A tag is empty, so the folder reads “…”. (each the phone's first sentence) | `gui.ytdl.where.trap.*` |
| not sure it finished (the phone's, before its " · ") · not sure it started | `gui.ytdl.list.not_sure_finished` · `.not_sure_started` |
| adding · starting… · failed · waiting for connection · not found on the server · file missing (a row's state, lower case; a job's leaves its title six cells at 47, a song's when sits in the folder's 22) | `gui.ytdl.list.*` |
| “…” landed here meanwhile · nothing was replaced · it's over the server's 500 MB limit, so nothing was added · … · the server didn't say why — a live stream, YouTube refusing the server, 500 MB or an hour (the webapp's reasons, "Failed · " dropped) | `gui.ytdl.list.landed` … `.hedged_older` |
| didn't start · the player couldn't reach the server (the record's "the phone") | `gui.ytdl.list.didnt_start` |
| Try again · Change format and retry · Choose a folder · Top of the library · Check again · Add again · Dismiss (a row's ways; each within the block's 47, a set of them on two rows at most) | `gui.ytdl.list.try_again` … `.dismiss` |
| This server doesn't offer Add from YouTube. (the feature's name — see the log) | `gui.ytdl.err.no_route` |
| Can't reach {server}. Check that it's running, then try again. (no way there too — see the log) | `gui.ytdl.server.unreachable` |
| e edit tags · d change · f change format · a try again · t type the details · u use them · u undo · i save to … instead · c choose a folder · s subfolders · u use for this song · y add a year (the tips line's key words — and, without their key, the page's controls: `edit tags ▸`, `undo`; `edit tags ▸` leaves the card's title ten cells at 47) | `gui.ytdl.key.*` |
| Right here · Album · Artist / Album · Custom (the Saves here sheet's radios; each within its column's 30) | `gui.ytdl.sort.*` |
| Add to library · Add here · Add anyway · Adding… (the primary: in the tall frame's 45, as the slab two cells clear of a title that keeps eight at 47, bracketed in the window's 72) | `gui.ytdl.add.*` |
| type or paste a link · Enter look it up · Esc give the keys back | `gui.tips.ytdl_field` |
| type · Tab ↑ ↓ tags · Enter done · Esc close (Edit tags'; reworded for the sheet from each language's own parts) | `gui.tips.ytdl_tags` |
| a paste replaces the link · e edit tags · d change · f format · Enter add · ↑ ↓ rows (composed from what the frame drew, at most the footer's 99) | `gui.tips.ytdl_paste` · `.ytdl_format` · `.ytdl_enter_add` · `.ytdl_rows` |
| ↑ ↓ rows · Enter play · a add to queue · m more · Esc deselect · x dismiss · ↑ ↓ walk · Enter picks · Esc close the list (the list's; `ytdl_stow` each language's "Esc deselect", the federation page's) | `gui.tips.ytdl_rows` · `.ytdl_play` · `.ytdl_queue` · `.ytdl_more` · `.ytdl_stow` · `.ytdl_dismiss` · `.ytdl_menu` |
| a paste fills the link and looks it up · typing works too · Esc close · ↑ ↓ or ← → move the dot · Enter add here (the window's and its sheet's) | `gui.tips.ytdl_here_field` · `.ytdl_here_dot` · `.ytdl_enter_here` · `.ytdl_enter_library` · `.ytdl_enter_anyway` · `.ytdl_close` |
| ← → library · ↑ ↓ rows · Enter save · type · Enter or Esc gives the keys back · ↑ ↓ pick up the cursor · Enter goes in, or uses this folder when stowed · Esc close (Saves to's, Custom's field's in both sheets, Choose a folder's) | `gui.tips.ytdl_where_*` · `.ytdl_choose` |
| ja 「…」の追加が完了 · ここに (cut to their cells — see the log) | `gui.ytdl.note.added` · `gui.ytdl.sort.here` |
| de Füge „…“ hinzu… (reworded to the note's 30 — see the log) | `gui.ytdl.note.adding` |

## Out of scope here

- **The classic TUI**, and a `gui --youtube <link>` seam (no OS hand-off
  exists for a YouTube link).
- **A clipboard key, and `Paste` in a terminal**: a paste is the way in
  (clause 11); the button stands only where the build reads the clipboard.
- **Stopping an add**: the server cannot stop one; Dismiss hides a row.
- **The phone's home card, its counts and its strip** of adds per folder.
- **A bar for the list**, and the list's rows' covers: the wheel and the
  keys scroll it, and its rows are words.
- **"You're offline"**: the desktop has no connectivity probe; a server
  with no way there reads "Can't reach {server}" (see the log).

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| The phone's Add from YouTube screen (a card, rows that each open a sheet, the jobs under it) | A Library nav **room** under TOOLS, the Auto DJ room's shape, beside Add torrent's (since 2026-11-09; a Settings doorway room before) — since 2026-11-10 the phone's own shape in cells: the link's card, the song's card with its cover, one row a sheet, the list under the form |
| The phone's explorer entry "Add from YouTube here" | The browse bar's **door word**, opening Keep a copy's 76-wide window |
| Material TextField with a paste button | The kit's 3-row card for the link (`[X]` DIM, DANGER under the pointer) and **bracketed paste** while the room or window is up; the window flavour's paste chord hands the clipboard over whole (`Face::takes_paste`), and its `Paste` button reads it too — a terminal has neither button nor clipboard |
| The card's cover (the lookup's thumbnail) | The kit's 8 × 4 cover slot, YouTube's own thumbnail for the video's id fetched by the player (clause 15) |
| The home card's counts, the strip, snackbars | One word on the Files door — none on the nav row or the title row (clause 3); the bar's note line |
| The jobs under the form (Adding / Needs a look / Added) | The **From YouTube** list in the room, one header, the three kinds in that order |
| Edit tags, Saves to, Format (sheets) | The three sheets (clause 53): bordered text boxes, radios (libraries' dot OK, a layout's and a format's ACCENT), one bracketed button |
| The phone's `ytdl_hub` | `ytdl::jobs::Tracker`, sans-I/O, its calls on the GUI's threads |
| Kept in `shared_preferences` | `config.toml`'s `[[server]]` keys and `ytdl.json` |

## Decisions on the open questions (2026-11-03)

1. **The Files door's key** (`termQuestions` 1) — `y`, kept as drawn, with
   no letter on screen (see the log).

## What the tests pin

`api::ytdl` against canned servers (17): the lookup's shapes on both servers,
the add's body and its 409 / 403 / 400 / 404 / 500, an add never sent twice,
reads asked once more when lost, refused vs no way there, the job list and
the record's pages, the tier's ingredients, mkdir and the listing. `ytdl`
(87): the webapp's and the phone's fixtures for links, the guess (and the
server's own readers it agrees with), layouts and traps, formats, yt-dlp's
file names, failures and why Add is off, and every label a key. `ytdl::jobs`
(33): following by id, the older tiers' claim with #231's snapshot and send
order, a lost answer never taking an earlier add, the poll's beat and
backoff, retention and the two-player merge, outcomes seen while the list
shows. `gui::ytdl` (44), `gui::ytdl_here` (18), `gui::ytdl_where` (10):
**the redesign canvas's boards cell for cell with their tokens** — Main,
Empty, First, After, Min, Wide, Reading, Refused, FailedLookup, Cleaned,
Clash, Tags, Format, SavesTo, Files — written into the tests from the
canvas's own files (the cover's picture drawn as the empty slot the tests
see; the few cells where the canvas's hand-drawn sample is off the kit's
own rules named beside their patch, see the log), with the boards the
canvas did not draw written down from the build once looked at (the Files
window empty, the Saves here sheet, Saves to as it opens and with a trap),
and the first canvas's that still stand (the door's spot, F4, the bar's
notes N1–N4, Choose a folder's W4 and W5); every control of each board a
click, its tips line; the nav row under each gate (Add torrent closing
TOOLS, no way in, the room yielding to Files and its sheets and windows
with it, a dropped connection closing nothing); `Paste` only where the
clipboard can be read (the terminal's card the band's width, the window's
face handing the clipboard in); the cover asked by the video's id once the
song shows, its address never anyone else's, a 16:9 frame's middle square,
the empty slot until it comes and where there is none; Edit tags' keys and
its four ways out, details typed by hand and a late answer; Format's dot
taken only by Done, a row's Format adding it again; a key acting only where
its control stands (and nowhere behind the mini player), Esc giving the
keys back and then leaving nothing, the capitals kept (Shift and Caps
Lock, the room and each sheet, the press out too); the list — its cursor
on its row as rows move, every kind that needs a look with its ways, a
song's verbs and its list (inside the room at its last row), its keys from
the link's field and back, the wheel and the keys scrolling it to `Show
earlier`, Show in folder's Back to the parent; the paste (bracketed on and
off with the room, whole through the loop, none while another's surface
or a sheet stands over the form); the press once and its answers (the
form's keys waiting, a link pasted as keys too, the list still working),
the form starting over with its job heading the list, the note taken down
by a wordless end, the re-list with the new row held, the Format sheet gone
with the window, a Save that wrote nothing keeping the song's choice, one
tracker per server and account (a token's reconnect keeping it); and
**every language**: the key words open with their letter and a blank (the
page's words are what follows), the words fit their cells (the tips' 99,
`Paste` and the link's card at 47, a row's label with ten cells of value,
the primary in its frame and as the slab, `edit tags ▸` beside the title,
the clean-up beside `undo`, a list row's words and its ways on two rows at
most, the sheets' labels over their cards, a format's help in two rows of
its column, the radios in theirs, the nav row's 12 cells, the door at 47,
the note's 30 for both heads, still looking, the late answer, the limits
and why Add is off within their rows), a sentence with no blanks wrapped
within its room, and the room, its sheets, the list and the window drawn
whole in each of the ten — the slab beside its title, a help wrapped in
its column, still looking, the late answer, the limits, a failed lookup,
a row that didn't start and the first time's words among them. The
shell's own (`gui`): the TOOLS group's
give-way at ten sizes from 100 × 24 to 100 × 30, the footer on and off, with
all five tools, none of the gated ones, Add YouTube without the sonic path
and Discover, and Discover without the sonic path — packed every time, the
rise only where the last tool would reach Settings; the Add rows last in
`NAV`, with no key; each Add row opening its room, from Manage servers too,
and Settings keeping TORRENTS to the torrent switch; both Add labels within
12 cells in every locale.

## Deviations log

- **2026-11-03 — The room and the window open with the link field holding
  the keys.** docs/ui-kit.md has a room open stowed ("List cursor") and an
  inline input idle until focused ("Text input"); a paste is the way in
  here, so the field takes the keys on open — a letter typed lands in it,
  `q` quits nothing — and gives them back once a song is found or on Esc.
- **2026-11-03 — The Files door's key is `y`, with no letter drawn.** The
  Key word rule ("drawn only where its key works, and the key works only
  where it is drawn"): the door word is a door, not a key word, and the
  canvas draws it letterless; `y` works wherever the word could stand
  (under every spot word, and where the give-way dropped it), nowhere else.
- **2026-11-03 — The browse bar gives way count first.** browser-top-bar
  clause 3 says the count is never dropped for another control; at 47
  cells the board drops it first for the door word, then clips the crumb
  from the left, then drops the word (`y` still works).
- **2026-11-03 — Bracketed paste, only while it is wanted.** Nothing else
  in the GUI turns `?2004h` on: it goes on while the room or the Files
  window is up, off as they close, at exit and in the panic hook. The
  window flavour has no bracketed paste: its paste chord hands the
  clipboard over whole while the room or window is up, caret or not.
- **2026-11-03 — Windows' console types a paste.** crossterm reads no
  paste event there (nor does a terminal that ignores `?2004h`), so a
  paste arrives as keys: on Windows the link field keeps the keys after a
  find (`YtUi::whole_pastes`), and the room's letters are never a pasted
  link's.
- **2026-11-03 — TORRENTS becomes ADD MUSIC** (add-torrent.md's log too):
  the key `gui.set.torrents_group` is kept, its value new copy in ten
  languages. *Superseded 2026-11-09, below: the group reads TORRENTS
  again.*
- **2026-11-09 — No "You're offline".** The phone has a connectivity
  probe; a call that never connected for want of a way there reads as one
  refused: "Can't reach {server}. Check that it's running, then try
  again." (the boards have no offline words).
- **2026-11-09 — "Doesn't offer Add from YouTube".** The phone's English
  `ytdlErrNoRoute` says "YouTube downloads"; its nine translations and the
  webapp say the feature's name, and so does this English (clause 76).
- **2026-11-09 — Two Japanese strings cut to their cells**: `note.added`
  reads 「…」の追加が完了 (the record's 「…」を {path} に追加しました cut at
  {path} is 31 cells with the boards' song, past the note's 30), and
  `sort.here` reads ここに (the record's ここに直接 pushes Custom off the
  window's row of dots).
- **2026-11-09 — The ADD MUSIC group reads one Portuguese.** The phone
  settled on Brazilian; `gui.set.tor_add_desc` read European ("ficheiro",
  "ligação") beside it, and now reads the phone's "arquivo", "link".
  *Superseded the same day, below: the row went with the doorways.*
- **2026-11-09 — A capital of a drawn word is that word.** The build note
  asks the room to swallow its capitals; one the frame drew acts as its
  lowercase (`D` is `d change`, `E` `e edit tags`), the rest do nothing —
  neither reaches the shell.
- **2026-11-09 — Where new songs land's tips give way too**: like the
  room's and the window's, its key words (`u use for this song`, a trap's
  way on) go first past the footer's 99 cells.
- **2026-11-09 — The subfolders' dot is ACCENT.** docs/ui-kit.md's
  Choices side by side draw the chosen `(•)` OK; the Files window's
  subfolders (clause 32, C1's `7:19-21:Accent`) wear the Radio group's
  ACCENT, the dot of a layout as Where new songs land's layouts wear it,
  so green never marks a place.
- **2026-11-09 — A sentence too wide for its row wraps, never clips.**
  The boards' English fits one row; in other languages still looking
  beside `t type the details`, the late answer beside `u use them` and why
  Add is off over the primary take a second, and a sentence with no blanks
  (ja, zh) — a GOLD line, a row's failure — breaks between characters
  inside its room, never past the window's border.
- **2026-11-09 — The German note's head reworded**: `note.adding` reads
  Füge „…“ hinzu… (the phone's „…“ wird hinzugefügt… is 33 cells with the
  boards' song, past the note's 30).
- **2026-11-09 — Both ways in move to the nav** (the owner's call,
  replacing the Settings doorways this build first made; add-torrent.md's
  log too). The left nav's TOOLS group gains `Add torrent` and `Add
  YouTube` after Discover, last in `NAV` — no digit, no letter (entry
  point 1, clause 2) — and the group packs the rooms it shows, the
  owner's second call the same day: with every row on its own line, a
  server with no sonic path and no Discover (most, the owner's own among
  them) showed two blank lines between Auto DJ and Add torrent. All five
  stand on 15–19; where Settings would meet the last tool shown the
  block rises a line at a time, the blank under Search first, then the
  TOOLS heading, then the LIBRARY heading (discovery-page clause 2). This
  supersedes the canvas's Settings frames — T1, and S1–S4's one word in
  the doorway's description — and the `◂` on every room board: the room
  is a nav room, the Auto DJ room's shape, its title at the room's first
  cell, Esc giving the field's keys back and then leaving nothing (clauses
  10, 22). The row carries no state (clause 3): the owner chose it without
  a glyph, and the jobs' word stays on the title row and the door. With no
  way back to keep open, the press keeps the boards' pause without Esc's
  way out: every typed key still waits — a link pasted as keys, where the
  terminal sends one so, must never reach the shell's globals (`p`, `s`,
  `/`, a digit, `q`) — and Esc and Tab are the shell's, the nav's rows the
  way on (clause 20; docs/ui-kit.md, Key word), where the boards had Esc
  the one way out. Settings is TORRENTS again over `[✓] Ask what to do
  with torrents` alone, its rows fixed as before: the dynamic rows and
  the blank above DISPLAY that gave way went with the row that needed
  them. `gui.set.yt_add`, `gui.set.yt_add_desc` and
  `gui.tips.ytdl_esc_back` are gone, `gui.set.torrents_group` reads its
  old word in each language, and `gui.nav.add_youtube` is new: "Add
  YouTube", ja YouTube 追加, zh 添加 YouTube. In de es fr it pl pt ru no
  verb fits the nav's twelve cells beside it (pl `Dodaj YouTube` is 13, de
  `YouTube hinzufügen` 18), so the GUI's add sign stands for the verb —
  `+ YouTube`, as `+ New playlist` and Discover's `+ queue all` read —
  rather than the bare `YouTube`, which beside Discover reads as a place
  to browse YouTube, not a way to add from it. The owner chose the sign
  over an abbreviated verb (it `Agg. YouTube`), 2026-11-09.
- **2026-11-10 — The redesign: the phone's shape in cells** (the owner's
  "improve the UI/UX for adding YouTube videos", then "Looks good, build
  it"). The canvas "Add from YouTube · GUI" replaces the first canvas's
  room, window frames, list window and place picker: the link in a card
  with `Paste` beside it; the found song a card with its cover; `Saves to`
  and `Format` as rows that each open one sheet; the primary tall and the
  block's width under the song; Edit tags a sheet of bordered text boxes;
  the From YouTube list in the room; the block centred and at most 56
  cells wide. With it go the calmer grammar's key words on the page (the
  letters stay as keys, named on the tips line — clause 72), `MP3 ▾` and
  its rising list, the tags as fields in the form, the From YouTube list's
  window and the title row's word (`gui.ytdl.word.history`,
  `gui.ytdl.list.esc_going`, `gui.tips.ytdl_list_key`,
  `gui.tips.ytdl_here_layout`, `gui.ytdl.room.files_go_to`,
  `gui.ytdl.here.into` and `gui.ytdl.where.title` are gone; the tracker's
  `Summary` and its `Word` with them), the Files window's radios in place
  and its `Into` row, and the name "Where new songs land", which the sheet
  that took its place does not wear.
- **2026-11-10 — The canvas's open decisions, built as drawn** (the owner
  approved the canvas with them listed): the cover is fetched by the
  player from YouTube's image host (clause 15 says how narrowly); `Paste`
  differs between the builds — there in the GUI's window, absent in a
  terminal, which cannot read the clipboard (clause 11); the list has no
  window any more; the block stops at 56 cells; Format is a sheet with a
  line for each format rather than a list; the row reads "Saves to" (the
  phone's words), the sheet too; and Saves to keeps both `use for this
  song` and `[ Save ]`.
- **2026-11-10 — Where the canvas's sample is off the kit's own rules,
  the build's cells stand** (each named beside its patch in the boards'
  generator, the tests' boards carrying the build's): the caret is the
  kit's field's own TEXT, not ACCENT; a refused link's field shows one
  more cell of its tail before `[X]`; a failed lookup's words wrap where
  the kit wraps them (two cells in from the block's 47); the Wide board's
  title column is a cell wider than drawn (the place's column is where it
  was drawn); the Cleaned board paired a title split off its artist with
  the channel's warning, which the rules never do — the title that gets
  both is cleaned of its noise alone; the SavesTo board's field wrote
  `{{YEAR}} - {{ALBUM}}` under a place with the artist's folder in it —
  the layout that makes that place stands in the field; and the After
  board trimmed its list to five songs to show `Show earlier`, which in
  the build follows the list's last song (as on the Empty board), the
  list scrolling to it.
- **2026-11-10 — The cover's empty slot is the kit's frame.** The canvas
  drew a filled block for the cover (a picture) and a darker one while
  the video is read; the build draws the picture where it has one and the
  kit's empty cover slot — a DIM rounded frame, the bar card's idiom —
  until then and where there is none: the theme has no fill for a
  placeholder, and a slot that reads as "a cover goes here" is the kit's
  own.
- **2026-11-10 — The primary is there, off, while the video is read**
  (the Reading board) — it was absent until a song was found — and **a
  video the server couldn't read no longer opens the tags by itself**:
  its words stand with `type the details ▸` under them, and Add works
  without the details (the FailedLookup board), where the tags used to
  open empty with the title holding the keys.
- **2026-11-10 — A sheet stands where a room's window does**, not always
  in the frame's middle: with the queue folded the room is wider than the
  sheet and the block is not under the frame's middle columns, so a sheet
  centred on the frame left the block's last cells showing cut beside it.
  Centred in the room it covers the block; over the Files window it takes
  the window's own columns for the same reason (clause 53). With the
  queue open at 100 columns it is where the canvas drew it.
- **2026-11-10 — The title gives way to the slab.** At the floor the
  primary's slab shares the title's row (the Min board); in German and
  the other long languages the two do not fit the block's 47 cells, so the
  title is cut with `…` two cells clear of the slab rather than the slab
  dropping its words.
- **2026-11-10 — A row's ways are TEXT.** The first canvas's list drew
  them ACCENT in its own window; in the room they are TEXT with DIM `·`
  between (the After board), the room's one ACCENT frame being its
  primary. And where a language's ways are wider than the block (German's
  "Direkt in die Bibliothek · Ordner wählen · Ausblenden") they wrap onto
  a second row rather than run off it.
- **2026-11-10 — A job's row says where it stands, a song's where it
  is.** The first list's rows carried the folder's crumbs on every row and
  a song's format beside its date; the After board draws neither on a
  job's row (its title has the row up to its state's words) and only the
  folder's last name and the date on a song's (`ytdl::record_format` went
  with the format).
- **2026-11-10 — Format's helps wrap, and the sheet grows a row for
  them.** The board's English fits one row each; French's "usually
  YouTube's own audio" line is 58 cells in a 50-cell column, so a help
  takes a second row in its column, never a `…`.
- **2026-11-10 — `Esc close` is not on a sheet's border.** Keep a copy's
  windows carry it as a button; the canvas's sheets have a bracketed
  button of their own and a plain border, so a sheet closes by Esc, its
  button, or a click outside it — the tips line says Esc.
