# Add from YouTube

| | |
|---|---|
| **Design of record** | For this surface's look and its words: the canvas "Add from YouTube — webapp & desktop" (claude.ai artifact `7Th46vjDNSJPuSAPZhPoqk`), its *Terminal player GUI* page — boards `TermMain` (T1–T4), `TermRoomStates` (A1–E3), `TermFiles` (F1–F6, C1–C4, the door's row 2 at 80 and 47 cells), `TermList` (L1–L4, F1, S1–S4, N1–N4) and `TermWhere` (W0–W5), cell-exact, each frame's Keys and Mouse lines part of the spec — and its notes `termDecisions`, `termQuestions` and `termBuild`, which win over the boards (the user answered the open question 2026-11-03). **Amended 2026-11-09**: the owner moved the way in from Settings to the left nav (TOOLS › `Add YouTube`), which supersedes the boards' Settings frames — T1, and S1–S4 — and the `◂` on every room board (see the log). For the behaviour and the record's words: the mobile app, `mstream_music` @ `origin/master` `1d4e8974` (PR #231) — `lib/singletons/ytdl_hub.dart` (the job hub, #231's `priorEntries` snapshot and send-order guard included), `lib/util/ytdl_{api,errors,filename,formats,guess,layout,link}.dart`, `lib/objects/ytdl_job.dart`, `lib/screens/ytdl/*.dart`, the `ytdl*` keys of `lib/l10n/app_*.arb`. Where the phone has no word: the webapp's port of the same rules, mStream PR #1123 (`claude/ytdl-webapp` @ `190f4325`) — `webapp/alpha/ytdl-core.js`, `webapp/locales/*.json` (`ytdl.*`). |
| **Server API** | **A server with the new job list** (the ping's `ytdl >= 1`, also `features.ytdl` on `GET /api/`; mStream #1123): `GET /api/v1/ytdl/metadata?url=` → `{title, artist, album, year, thumbnail, fileStem, isLive, liveStatus, durationSec, channel}`, a failed lookup a bare 500 · `POST /api/v1/ytdl/ {directory, url, outputCodec, metadata?: {title, artist, album, year}}` → 200 `{message, id, fileName}`; **409 `{error, reason: 'exists', fileName}`** refused before anything downloads; 403 Uploading Disabled; 404 a library or folder; 500 "FFmpeg is not available yet" / "yt-dlp is not installed" · `GET /api/v1/ytdl/downloads` → the caller's jobs, newest first, `{id, pid, url, directory, outputCodec, status: downloading \| complete \| error, startTime, endTime, fileName, filePath, downloadId, error, reason}`, `reason` ∈ exists · too-big · too-long · live · unavailable · null, settled ones kept 24 h · `GET /api/v1/discovery/downloads?plugin=ytdl&limit=&before=` → the account's record of what came in (the list's songs, Show earlier). **Older servers** (no `ytdl`): 6.30–6.33 answer the record route unfiltered (the "full" tier: a global tracker keyed by pid, settled entries kept 30 s, a POST answering `{message}` alone, a name clash failing after the download); before 6.30 the record route is Express's 404 (the "older" tier: an add whose file name is already in the folder re-tags that file). **Both**: `POST /api/v1/file-explorer/mkdir {directory}` (403 for an account that can't make folders — the ping's `noMkdir`), `POST /api/v1/file-explorer {directory}` (a folder's names, for the clash and Choose a folder), the ping's `noUpload`, `transcode`, `noMkdir` and `supportedAudioFiles` (a map of format → on, or a list). |
| **Already in this repo** | The torrent room's shape — every call a one-shot `Client` on its own thread folded back between draws (`gui/torrent.rs`) — so no `ApiCmd` and no wasm worker change; Keep a copy's window, its band, its borders and its notes (`gui/keep.rs`: `window_rect`, `open_window`, `draw_borders`, `set_note`, `fit_note`), Where copies land's picker grammar and the layout engine it mirrors (`api/types.rs`: `validate_layout`, `layout_folders`, `sanitize_segment`), the kit's tall primary, line editor, `ListView` and `scroll_items`. **Built 2026-11-09, phase A** (the logic): `src/api/ytdl.rs` — the calls, `YtdlCallError` (the status, the server's words, its `reason` and `fileName`, a timeout told from a refusal and from no way there), the shapes, the tier; the ping's `noMkdir`, `supportedAudioFiles` and `ytdl` read leniently (`api/types.rs`); `ServerEntry`'s `ytdl_library`, `ytdl_layout`, `ytdl_layout_kind`, `ytdl_format` and the disposable `ytdl.json` beside `config.toml` (`config.rs`); `src/ytdl/` — the pure rules, a port of the webapp's `ytdl-core.js` and the phone's `ytdl_filename.dart` (links, the tag guess and its clean-up, layouts, presets and bracket traps, formats, yt-dlp's restricted file name, the clash, failures and why Add is off), every word a `Label` (a locale key) — and `src/ytdl/jobs.rs`, the job tracker, sans-I/O over an injected transport, clock and store (the phone's hub for the older tiers, the job id for the new one). **Phase B** (the GUI): `src/gui/ytdl.rs` (the room, the From YouTube list, the format list, the paste seam), `src/gui/ytdl_here.rs` (the Files door and its window, the re-list), `src/gui/ytdl_where.rs` (Where new songs land, Choose a folder); the shell's hooks in `gui/mod.rs` (the nav's `NavId::YouTube` row — a Settings doorway until it moved, 2026-11-09 — `Host::paste`, `TermEvent::Paste`), `Face::takes_paste` in the window flavour, the panic hook's paste off (`tui/mod.rs`); the `gui.ytdl.*`, `gui.nav.add_youtube` and `gui.tips.ytdl_*` strings in the ten locales. |
| **Target surface** | The GUI player, both flavours: a Library nav room under TOOLS (a room off Settings until 2026-11-09), and a door on Files. **The classic TUI gets nothing**, and there is no `gui --youtube` seam in v1. |
| **Status** | Designed 2026-11-03 (the canvas, the brief's decisions, the user's answers); phase A and phase B built 2026-11-09; this contract written from the canvas notes that stood as the contract during the build; the room moved from Settings into the nav the same day (the owner's call, see the log). The pty smoke (`smoke/gui/scenario_ytdl.py`) passes against a new-tier server (mStream with #1123 and its fake yt-dlp) and an older one (6.32.3), before the move and after it. |

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
3. **Nothing else** in v1: no clipboard key, no command-line seam, no TUI.

The row and the door stand only under the gate (clause 1).

## States & flows

The room is one form, revealed by what the link came to: **empty** (the
field has the keys) → a link typed or pasted → **reading** (8 s on, "still
looking", with a way to type the tags) → **found** (the song, its tags, the
place) or **failed** (the lookup's own words; the four tags open empty) or
**timed out** (try again) — a late answer offering its details over tags
already typed → the **press** (`Adding…`, every typed key waiting) → the
server answers: started, and the form **resets** for the next link; or
refused, and the form stays with the server's words above the primary.
From the press on the job is the tracker's: **adding** → **added** (the
note says so; the list keeps it) or **failed** (the list says why and
offers a way on), with **starting…**, **not sure it started**, **not sure
it finished** and **not found on the server** where the server's answers
leave it unsure. The Files window is the same form over a folder; Where new
songs land and Choose a folder change where the room's song goes. Kept:
per server in `config.toml` (library, layout, format); the jobs in
`ytdl.json`; nothing else outlives the session.

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
   news is the title row's word (clause 22) and the Files door's spot
   (clause 30). The Settings doorway's one word (S1–S4) went with it (see
   the log).

### The room

10. **It opens with the link field holding the keys** (A1; see the log),
    on every opening from its row — unless a press is still out, which
    keeps the form quiet (clause 20): the caret, then the DIM placeholder
    "Paste a YouTube link". Typing edits the link; Esc gives the keys back
    (a room's keys again); Esc once more leaves nothing — a nav room has
    no way back, the Auto DJ room's rule — and is the shell's: it sends an
    armed Sonic path or Discover pick home, as anywhere, and otherwise
    does nothing; never to Settings. A click on the field takes the keys.
11. **A paste lands in the link field, whatever has the keys.** While the
    room (or the Files window) is up the terminal is in bracketed paste;
    a paste replaces the link — its first non-blank line, controls and
    invisible format characters left out, at most 4096 characters — and
    looks it up at once, the same link pasted again asking again. Once a
    song is found the field gives the keys back, so `e`, `d`, `f` and
    Enter work. Nothing wants a paste while the From YouTube list, Where
    new songs land or Choose a folder stands over the form, or while a
    press is out: it is dropped. While another's surface stands over the
    room or Files — the sign-in form a lapsed token opens, Add a server,
    the header's server list, the sheet, a dialog — bracketed paste is off
    and the paste is that surface's, typed into its field as before.
12. **A typed link is looked up once the typing rests** (400 ms), or on
    Enter. A changed link starts the form over; an answer for a link no
    longer in the field is dropped; a lookup is bounded (30 s).
13. **The link's own lines**, under the card: GOLD refusals — "That's not
    a YouTube link. Paste one from youtube.com or youtu.be." and "That's a
    playlist. Open one video in it and copy that link." (A2, A3, the field
    keeping the keys) — the DIM note "Just this video, not the playlist."
    (A4), and the live-stream warning (A5) — on an older server the
    phone's hedge, "This server doesn't stop live streams. Add this only if
    the stream has ended." Every link is sent as
    `https://<host>/watch?v=<id>` (a youtu.be, shorts, live or embed link
    rewritten; the server refuses a youtu.be) — and never shown so.
14. **The lookup**: "Reading the video…" with a DIM bar (B1); after 8 s
    "Still looking — YouTube can be slow." and `t type the details`, which
    opens the four tags (B2). A failed lookup (a bare 500, an unreadable
    answer) reads "Couldn't read this video's details. It may be private,
    age-restricted or blocked for the server." and the tags open empty
    (B4); a timeout "The server took too long — it may still be setting up
    its downloader." with `a try again` (B5); yt-dlp missing reads so and
    Add stays off (D1); a server the player cannot reach — refused, or no
    way there — "Can't reach {server}. Check that it's running, then try
    again." with `a try again` (D2). An answer that lands after tags were
    typed shows "Details found for this video" and `u use them`, which
    fills only the tags left empty (B3).
15. **The song found** (T2): the title; the facts DIM (artist · album ·
    year, "No album" for none) with `e edit tags` at the right; the
    source line — "Tags from YouTube Music" (DIM), or GOLD "Artist may be
    the channel name — check it" (C3), nothing for an artist split off the
    title; where the title was cleaned, "Cleaned from “…”" with `u undo`
    (C2), putting every cut back.
16. **Edit tags** (C1): TITLE, ARTIST, ALBUM, YEAR — the one label column
    — as 1-row fields, ↑ ↓ between them, a click taking the keys; under
    the focused one its one help line (where the value came from, what a
    blank does). A year is four digits or blank; the tags go to the server
    trimmed, a blank one not at all.
17. **Where it lands**: "Files go to Music › Daft Punk › Discovery new"
    (the library TEXT, a folder the add will make followed by a DIM
    `new`) with `d change` at the right; narrow, the crumbs give up whole
    folders from the left behind `…` (T3). An account that can't make
    folders, with a folder new or unchecked (C5): a GOLD line in the
    place's row, `i save to {the deepest folder there} instead` and `c
    choose a folder`; Add off.
18. **The file and the folder's names** (asked of the server once the tags
    rest): on a server from 6.30 a name like one already there is a GOLD
    "Something like “One_More_Time.mp3” looks to be in this folder
    already." with `f change format`, and the primary reads **Add anyway**
    (C4); before 6.30 the server would re-tag that file, so the phone's
    words say so and Add is off. A title whose letters yt-dlp drops says
    the name it will get. The name is the lookup's `fileStem`, else
    predicted from the title as yt-dlp's `--restrict-filenames` makes it.
19. **The format**: `MP3 ▾` on the bottom band, its list rising from its
    first cell (`f`), offering only what the server's `supportedAudioFiles`
    has on, in the record's order; remembered per server; a 400 on the
    format forgets it.
20. **One primary, Enter presses it**: the tall `Add to library ▸` at the
    bottom right (at 47 cells x 42–63). Off it is DIM, its width kept, no
    click, and the reason it is off stands DIM right-aligned above it —
    "Already adding to this folder" (D4) — or, for a refusal, the GOLD
    lines. Pressed it reads `Adding…` (13 cells), no click, no hover, and
    every typed key waits for the server (D5) — letters either case,
    digits, signs, Space, so a link pasted as keys (a terminal with no
    bracketed paste, clause 11) never reaches the shell's globals — and ↑
    ↓ too; every control stands — the key words, the title row's word, the
    link card and `MP3 ▾` — neither clicking nor lighting, and the tips
    line names no key. A nav room has no back for Esc to take, so Esc and
    Tab are the shell's, and the nav's rows are the way on; the row
    clicked again opens the room with the form still quiet.
21. **The press**: never twice for one link and folder while one is open
    (the tracker is asked first). A folder the place would make is made
    first; the add is sent **once, ever**. The server's answer:
    - **started** — the form resets in place (E1), the note line says
      "Adding “One More Time”…" (ACCENT);
    - **refused** — the form stays and the words stand GOLD above the
      primary (D3): a 409's "“One_More_Time.mp3” is already in this
      folder, so nothing was added." with `f change format` — standing for
      the clash warning it confirms (C4's line goes, one `f change format`
      left); "The server
      can't convert audio right now. Try again later, or ask its admin."
      (the ping asked again); a library gone, uploads off, the server's
      own words otherwise; Enter tries again;
    - **no connection** — "Can't reach {server} — check that it's running,
      then try again.";
    - **no answer in time** (a timeout, a lost answer, a 504) — never
      sent again: the form goes on as though started and the job is
      looked for (clause 60), its list row `starting…` until it is found,
      `not sure it started` if two minutes find nothing.
22. **The title row**: `Add from YouTube` (BOLD) at the room's first
    cell — no `◂`, the nav column being right there, and a click on it
    does nothing — and its word at the right, ending one cell in:
    `adding` (ACCENT), `! failed` (GOLD), `✓ added` (OK, an hour), else
    `added songs` (DIM) once the list has anything; none before. The
    word, or `l`, opens the From YouTube list (E1–E3).

### The Files door and its window

30. **The door word** (F1): `add from YouTube`, ACCENT (BRIGHT BOLD under
    the pointer, no tooltip), three cells after the crumb on line one of
    the browse bar; the count at the right as ever. Its spot carries the
    window's job once one is pressed: `adding` (ACCENT), `✓ added` (OK, 8 s,
    then the door again), `! failed` (GOLD — a click opens the From YouTube
    list). `y` opens the window under every word, **no letter on screen**
    (see the log). At 47 cells the count goes first, then the crumb clips
    from the left (`…Music/Daft Punk/Homework`, at 12 cells at the least),
    then the word (see the log). The word steps aside while the window is
    up.
31. **The window** (F2–F6): Keep a copy's shape and place — 76 cells wide,
    centred in the room with its band at x 19–94 (queue folded), over the
    whole frame at x 12–87 with no band when the room is narrower than 80;
    anchored as if 17 rows (row 6 at 100 × 30, row 3 at 100 × 24), growing
    down to the room's last row; "Add from YouTube" on its top border,
    "Esc close" on its bottom one; the room beneath inert (discovery-page
    clause 106). It opens empty with the link field holding the keys (F2;
    "a paste fills the link and looks it up · typing works too"); what the
    link comes to reads as in the room (clauses 13–18); the place is
    "Into Music › Daft Punk" — the folder on screen — with `s subfolders`
    at the right; the action row `MP3 ▾` and the bracketed primary `[ Add
    here ]` at the right. Esc closes it at any time.
32. **Subfolders** (C1, C2): `s` opens the radios in place — `(•) Right
    here   ( ) Album   ( ) Artist / Album   ( ) Custom`, the dot ACCENT,
    ↑ ↓ or ← → moving it; Custom opens its line editor and the variables
    row (ARTIST · ALBUMARTIST · ALBUM · YEAR, a click putting one in at
    the caret). Past Right here the primary reads `[ Add to library ]`.
    Not remembered.
33. **The press** (C4): nothing moves, only the primary goes off — `[
    Adding… ]` DIM, no click, no hover; the key words stand and do
    nothing; Esc closes and the press still goes through. The window
    closes once the server starts the job; a refusal (the 409, the
    server's words) stays inside it, and its row is never made.
34. **After** (F4): the door's spot carries the job (clause 30); once the
    song is in, the folder is listed again — its rows stay up meanwhile —
    and the cursor rests on the new row (or on the subfolder it made),
    **lit and held**: the list-cursor law's one exception, the row the add
    made. Not while a filter stands (the listing would wipe it), and not
    when Files has moved on. The note line says it (clause 70).

### The From YouTube list

40. **The window** (L1–L4, F1): Copies' place and width — 76 cells, the
    same anchor and band as Keep a copy — "From YouTube" on its top border,
    "Esc close · it keeps going" on its bottom one while an add runs, else
    "Esc close"; growing with its items, then scrolling them (`▲█▼` on its
    right border); over a 47-cell room centred on the frame (F1). The room
    beneath is inert.
41. **Its items**, in order: the open adds (`↓` adding, then `◷`
    starting…), then those that need a look (`!` failed, `?` not sure),
    then the songs that came in — the server's record, newest first, with
    "Show earlier" at the bottom right while it has more. A row: its mark,
    the title (20 cells), the folder as crumbs (clipped from the left),
    and at the right where it stands — `adding · 2 min · MP3`,
    `starting…`, `failed`, `not sure it started`, `waiting for connection`,
    a song's `MP3 · 3 min ago` / `Oct 31` or `file missing`. A row that
    needs a look carries its words on the line under it and its ways at
    the right — Try again, Change format and retry (the format list hung
    from its first cell), Choose a folder, Top of the library, Check
    again, Add again, Dismiss — sharing the words' last line where both
    fit.
42. **Keys and pointer**: ↑ ↓ pick up and walk the rows (the list-cursor
    law), the cursor staying on its row as the rows move — a row added
    again lists with the open ones, and the cursor goes with it; Enter the
    held row's first way (a song's: play); `x` dismisses a
    settled job; on a song `a` queues it and `m` opens its list (Add to
    queue · Show in folder — Files on the song's folder, the cursor
    resting lit and held on the song as clause 34's does, Back from there
    the parent, not an older walk's column); a click acts on any way; a ✓
    song plays on a
    click, its verbs `[▸] [+] [⋯]` under the pointer, a right click its
    list; the wheel scrolls. Esc closes the song's list first, then the
    list. What the list has shown is **seen**: the title row's `! failed`
    and `✓ added` go once the list showed them.

### Where new songs land

50. **The picker** (W0–W3): `d change` opens it on the choice in force —
    80 cells wide at Where copies land's place (centred over the frame, its
    top on row 4 at 100 × 30), "Where new songs land" on its top border,
    "Esc close" on its bottom one. Rows: "Files go to …" first (the place
    this song would get); a bracket trap's GOLD line — "No year for this
    song, so the folder reads “Discovery ()”." — with its way on, `y add a
    year` or `e edit tags`; the libraries side by side only with two or
    more (the dot OK, ← →); the layouts as radios, the dot ACCENT — Artist
    / Album (`{{ALBUMARTIST}}/{{ALBUM}}`), Artist, Top of the library, Same
    as Discover (only while the account has a Discover destination of its
    own), Library layout (only where the server has templates), Custom
    (its line editor, its GOLD refusal, the variables row); `u use for
    this song` at the bottom left and `[ Save ]` at the right.
51. **Save is off until something changed** (DIM, no click, its tooltip
    why: "nothing changed yet" / "fix the layout first"); it keeps the
    choice for the server in `config.toml` (`ytdl_library`, `ytdl_layout`,
    `ytdl_layout_kind`; Same as Discover folds its base into the layout).
    `u use for this song` changes this song's place alone. It reads the
    account's Discover destination and **never writes it**.
52. **Choose a folder** (W4, W5; `c`, a failed row's way): an existing
    folder, for an account that can't make one — it takes the place of the
    window it was opened from, which comes back when it closes: `◂ Up to
    …`, the folders, `[ Use this folder ]`; ↑ ↓ pick up the cursor, Enter
    goes in, or uses this folder while the cursor is stowed.

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
    drew (T2: "a paste replaces the link · e edit tags · d change · f
    format · l list · Enter add" — no Esc: a nav room has nothing to go
    back to); past the footer's 99 cells the last key words give way —
    the paste and Enter stay. While a press is out the line names none
    (clause 20).
72. **Key words** (docs/ui-kit.md, Key word): drawn only where their key
    works, the key working only where its word stood, the letter never
    translated, DIM, BRIGHT BOLD under the pointer, a click doing what the
    key does. **The room's capitals are its own**: with Shift or Caps Lock
    `d`, `t` and `a` arrive as `D`, `T` and `A`, the shell's Auto DJ, Stats
    and DJ toggle — while the room or any of its windows is up, the press
    out or not, a capital of the room's letters never reaches the shell
    (a drawn word's capital is that word: `D` is `d change`), and the
    windows keep every key. Try again is `a`, never `r` (the shell's
    Repeat, which reaches it from this room as from every nav room).
73. **Glyphs live in code** behind `legacy_conhost` — `▸ ◂ › ▾ ✓ ↓ ◷ •
    ▏ …` and the `! ` of a GOLD line — the locale strings carry words only
    (add-torrent's 2026-09-26 rule).
74. **No counts** (the canvas's "count-free"): not on the nav row, the
    door, the title row's word or the list — a row's minutes and its date
    are times, not tallies.
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
`add`, `here`, `choose`), `gui.nav.add_youtube` and `gui.tips.ytdl_*` — each
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
| adding · ! failed · ✓ added · added songs (the one word on the title row and the door — at most 20 cells at 47, and beside the title; `history` new) | `gui.ytdl.word.*` |
| add from YouTube (the Files door, new) | `gui.ytdl.door` |
| From YouTube · Esc close · it keeps going (the list's borders, new; the footer at most 50 cells) | `gui.ytdl.list.title` · `gui.ytdl.list.esc_going` |
| Adding “One More Time”… · Added “One More Time” (the note's head, at most 30 cells with the boards' song) · Added “…” · Daft Punk › Discovery (where the note is wider, new) · Couldn't add “…” | `gui.ytdl.note.adding` · `.added` · `.added_in` · `.couldnt` |
| Still looking — YouTube can be slow. · Details found for this video (beside `t type the details` and `u use them`, wrapped, never clipped: the first at most two lines in the room's 80 and the window's 72, the second at 47) | `gui.ytdl.room.slow` · `gui.ytdl.room.late` |
| Fix the layout to see where files land (why Add is off; at most two lines at 47) | `gui.ytdl.where.fix_layout` |
| The server took too long — it may still be setting up its downloader. (the phone's, "Try again in a minute." cut) | `gui.ytdl.lookup.timed_out` |
| Something like “…” looks to be in this folder already. (the phone's first sentence) | `gui.ytdl.file.clash` |
| Nothing to fill this folder, so it's named “…”. · No year for this song, so the folder reads “…”. · A tag is empty, so the folder reads “…”. (each the phone's first sentence) | `gui.ytdl.where.trap.*` |
| not sure it finished (the phone's, before its " · ") · not sure it started | `gui.ytdl.list.not_sure_finished` · `.not_sure_started` |
| adding · starting… · failed · waiting for connection · not found on the server · file missing (a row's state, lower case, new; at most 37 cells beside the place) | `gui.ytdl.list.*` |
| “…” landed here meanwhile · nothing was replaced · it's over the server's 500 MB limit, so nothing was added · … · the server didn't say why — a live stream, YouTube refusing the server, 500 MB or an hour (the webapp's reasons, "Failed · " dropped) | `gui.ytdl.list.landed` … `.hedged_older` |
| didn't start · the player couldn't reach the server (the record's "the phone") | `gui.ytdl.list.didnt_start` |
| Try again · Change format and retry · Choose a folder · Top of the library · Check again · Add again · Dismiss (a row's ways; each row of them within 69 cells) | `gui.ytdl.list.try_again` … `.dismiss` |
| This server doesn't offer Add from YouTube. (the feature's name — see the log) | `gui.ytdl.err.no_route` |
| Can't reach {server}. Check that it's running, then try again. (no way there too — see the log) | `gui.ytdl.server.unreachable` |
| e edit tags · d change · f change format · a try again · t type the details · u use them · u undo · i save to … instead · c choose a folder · s subfolders · u use for this song · y add a year | `gui.ytdl.key.*` |
| Right here · Album · Artist / Album · Custom (the window's dots; the four and their gaps within 72) | `gui.ytdl.sort.*` |
| Add to library · Add here · Add anyway · Adding… (the primary; tall beside `OGG Vorbis ▾` within 47, bracketed in the window's 72) | `gui.ytdl.add.*` |
| type or paste a link · Enter look it up · Esc give the keys back | `gui.tips.ytdl_field` |
| type · ↑ ↓ tags · Enter add · Esc give the keys back | `gui.tips.ytdl_tags` |
| a paste replaces the link · e edit tags · d change · f format · l list · Enter add (composed from what the frame drew, at most the footer's 99) | `gui.tips.ytdl_paste` · `.ytdl_format` · `.ytdl_list_key` · `.ytdl_enter_add` |
| ↑ ↓ rows · Enter play · a add to queue · m more · Esc close · x dismiss · ↑ ↓ walk · Enter picks · Esc close the list (the list's) | `gui.tips.ytdl_rows` · `.ytdl_play` · `.ytdl_queue` · `.ytdl_more` · `.ytdl_close` · `.ytdl_dismiss` · `.ytdl_menu` |
| a paste fills the link and looks it up · typing works too · Esc close · ↑ ↓ or ← → move the dot · Enter add here (the window's) | `gui.tips.ytdl_here_*` · `.ytdl_enter_here` · `.ytdl_enter_library` · `.ytdl_enter_anyway` |
| ← → library · ↑ ↓ rows · Enter save · ↑ ↓ pick up the cursor · Enter goes in, or uses this folder when stowed · Esc close (Where new songs land's, Choose a folder's) | `gui.tips.ytdl_where_*` · `.ytdl_choose` |
| ja 「…」の追加が完了 · ここに (cut to their cells — see the log) | `gui.ytdl.note.added` · `gui.ytdl.sort.here` |
| de Füge „…“ hinzu… (reworded to the note's 30 — see the log) | `gui.ytdl.note.adding` |

## Out of scope here

- **The classic TUI**, and a `gui --youtube <link>` seam (no OS hand-off
  exists for a YouTube link).
- **A clipboard key**: a paste is the way in (clause 11).
- **Stopping an add**: the server cannot stop one; Dismiss hides a row.
- **The phone's home card, its counts and its strip** of adds per folder;
  the cover and thumbnail (the calmer grammar has no fills and no covers).
- **"You're offline"**: the desktop has no connectivity probe; a server
  with no way there reads "Can't reach {server}" (see the log).

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| The phone's Add from YouTube screen (a sheet over a card) | A Library nav **room** under TOOLS, the Auto DJ room's shape, beside Add torrent's (since 2026-11-09; a Settings doorway room before) — one form, no labels but the tags', the calmer grammar |
| The phone's explorer entry "Add from YouTube here" | The browse bar's **door word**, opening Keep a copy's 76-wide window |
| Material TextField with a paste button | The kit's 3-row card for the link (`[X]` DIM, DANGER under the pointer) and **bracketed paste** while the room or window is up; the window flavour's paste chord hands the clipboard over whole (`Face::takes_paste`) |
| The home card's counts, the strip, snackbars | One word on the title row and the door — none on the nav row (clause 3); the bar's note line |
| The jobs sheet (Adding / Needs a look / Added) | The **From YouTube** list window, Copies' shape, two-row items |
| Where new songs land (a sheet) | Where copies land's picker (keep.rs) with Add from YouTube's presets; libraries' dot OK, the layout's ACCENT |
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
(88): the webapp's and the phone's fixtures for links, the guess (and the
server's own readers it agrees with), layouts and traps, formats, yt-dlp's
file names, failures and why Add is off, and every label a key. `ytdl::jobs`
(33): following by id, the older tiers' claim with #231's snapshot and send
order, a lost answer never taking an earlier add, the poll's beat and
backoff, retention and the two-player merge. `gui::ytdl` (44),
`gui::ytdl_here` (17), `gui::ytdl_where` (9): every board frame cell for cell
with its tokens (T2–T4, A1–E3, F1–F6, C1–C4, the door's spot, L1–L4, N1–N4,
W0–W5; the room's drawn with no `◂`), the nav row under each gate (Add
torrent closing TOOLS, no way in, the room yielding to Files and its windows and
lists with it, a dropped connection closing nothing), a key acting only
where its word stands (and nowhere behind the mini player), the arrows kept the room's, Esc giving the keys back and
then leaving nothing, the capitals kept (Shift and Caps Lock, the room and each
window, the press out too), the paste (bracketed on and off with the room,
the window's whole paste, none while another's surface or the list stands
over the form), the press once and its answers (every typed key waiting, a
link pasted as keys too, Esc and the nav's rows the shell's), the note
taken down by a wordless end, the re-list with the new row held, Show in folder's Back to the parent, the list's cursor on its
row, the format list gone with the window, a Save that wrote nothing keeping
the song's choice, one tracker per server and account (a token's reconnect
keeping it); and **every language**: the key words open with their letter,
the words fit their cells (the tips' 99, the tall primary beside
`OGG Vorbis ▾` at 47, the window's action row and its four dots in 72, the
nav row's 12 cells, the title row at 47, the door at 47, the list's
right-hand words and ways, the note's 30 for both heads, still looking, the
late answer and why Add is off in two lines), a sentence with no blanks
wrapped within its room, and the room, the list and the window drawn whole in
each of the ten — still looking, the late answer, a failed lookup and a row
that didn't start among them. The shell's own (`gui`): the TOOLS group's
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
