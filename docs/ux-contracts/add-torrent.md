# Add torrent

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`0f8b23f`, merged 2026-08-28) — `lib/screens/add_torrent_screen.dart` (the "smart" panel), `lib/native/torrent_channel.dart` + `MainActivity` (both directions of the OS hand-off), the intent chooser in `lib/main.dart`, the two settings rows in `lib/screens/settings_screen.dart`, `lib/util/torrent_meta.dart`. The screen is itself a port of the webapp's standalone panel (`webapp/alpha/m.js setupAddTorrentPanel`; `webapp/torrent/` is the reduced mobile-web sibling) — where the two disagree, this contract says so. |
| **Server API** | `GET /api/v1/torrent/preflight?path=` (active · clientType · displayName · noUpload · userAllowed · reason) · `GET /api/v1/torrent/path-templates` · `POST /api/v1/torrent/auto-detect` (multipart) · `POST /api/v1/torrent/seed-existing` (multipart) · `POST /api/v1/torrent/add` (multipart: vpath · subPath · directoryName · renameRoot · exactly one of magnet / torrentFile) |
| **Already in this repo** | nothing before this slice — no torrent client surface existed; all five client calls, the multipart body and the metadata parser are new (`src/api/mod.rs`, `src/gui/torrent_meta.rs`). The kit brought the modal, the line editor, the wizard's path-completion machinery and the servers room's thread-and-reply pattern |
| **Target surface** | the GUI player — a Library nav room under TOOLS (a room off Settings until 2026-11-09), plus `mstream-player gui --torrent <file-or-magnet>` for torrents arriving from the OS (`src/gui/torrent.rs`) |
| **Design** | the canvas "Add torrent · GUI" (2026-11-10): Add from YouTube's shapes for a torrent — one centred block, the two ways in as two boxes, the file a card, `Metadata` and `Destination` rows that each open a sheet, the primary under the form, what was sent under that. Its six open questions are settled below as its boards drew them |
| **Status** | contract extracted 2026-08-31; the five open questions settled and the room implemented 2026-09-01 (decisions in the deviations log); the room moved from Settings into the nav 2026-11-09 (decision 5 amended); the room rebuilt to its canvas 2026-11-10 (the canvas's decisions and the log's entries of that day) |

## Intent

Hand the server a torrent — a `.torrent` file or a magnet link — to
download into a chosen library at a music-shaped path, with the client
doing the smart part: read the torrent's own name, guess artist/album/year,
resolve the library's path template, and check whether the files are
already on disk before downloading anything. And because a torrent can
turn out not to be for the server at all, the flow can hand it onward to a
real torrent client instead. Being the system's handler for `.torrent`
files and `magnet:` links is what makes the receiving half exist.

## Entry points

1. **In-app navigation** — the screen on its own. *Here: the nav row
   `Add torrent`, the TOOLS group's after Discover (row 18 with all five
   tools; the group packs the rooms it shows, so it stands higher where
   the sonic path or Discover is absent, and on short terminals it rises
   with the block — discovery-page clause 2), drawn
   whenever the Library is: no ping flag exists, the room is the gate. A
   Library nav room, the Auto DJ room's shape: the title row the name in
   BOLD at the room's first cell and the gate's standing at the right, no
   `◂`; ↓ picks up the cursor, Esc stows it, and Esc once more leaves
   nothing — never to Settings. Each opening stows the cursor and asks
   the gate where this server has not answered it. (Since 2026-11-09; the
   Settings room's `Add a torrent ▸` doorway, the Manage-servers shape,
   before.)*
2. **Opened WITH a torrent** — the OS delivers a `.torrent` (file manager,
   browser download, share) or a `magnet:` link to the app. *Here: the
   `--torrent` seam, which the installers' file associations will
   launch; whichever way the torrent comes into the screen — straight in
   with asking off, the chooser's Add, our own file coming back — it
   opens the nav room the nav's own way: the Library screen brought back
   from Now Playing, Stats, Admin or the MP3 Player screen, Manage servers
   let go, then the form filled.* What happens next is the **ask-me
   setting**'s call:
   - *ask* (default): a chooser — "add it here, or hand it on?" — with a
     don't-ask-again checkbox (checking it is what flips the setting;
     the Settings switch is the way back).
   - *skip*: straight into the screen, source pre-filled.
   Delivery is one-shot (cold start and already-running both drain the
   same slot, so nothing double-delivers), and every arrival is logged —
   "I opened a torrent and nothing happened" is untriageable otherwise.

## States & flows

One screen, revealed in steps. **Availability is per-server and the screen
is the gate** — there is no ping flag for torrents; `/torrent/preflight`
answers, and a banner explains why when the server can't take one
(feature off, uploads off, user not allowed — the server's own `reason`).

Server picker (multi-server) → library (vpath) picker → **source** →
metadata → destination → options → submit. Everything below the source
stays hidden until a source is real: a picked file, or a magnet with a
valid infohash.

*Here, since 2026-11-10 (the canvas):* one block, centred in the room, 56
cells at most (47 at 100 columns with the queue open). Top to bottom:

- **The title row** — the name BOLD; at its right end the gate's standing
  (`Transmission · ready`, DIM) or, where the room is too short for the
  tall primary, the primary as a slab.
- **The gate's words**, when it has any, under the title in place of the
  blank row: that the server is being asked (ACCENT), why it can't take
  one — its own reason, whole, GOLD — or that there is no server. Four rows
  at most.
- **No source yet: the two boxes.** A tall frame the block's width,
  `Choose a .torrent file…` (the native dialog); `type a path ▸` at the
  right under it; the record's `or` between two rules; then the link's
  band — a bordered box, `Paste a magnet link`, and `Paste` beside it where
  the build can read the clipboard (the window; never a terminal).
- **A file in hand: its card** in the boxes' place — the name BOLD, `[X]`
  to let it go, and under the name what it holds (`12 files · 356 MB`,
  read from the file; left out where the list can't be read). Under the
  card its ways, TEXT with a DIM `·` between them: `Auto-detect metadata`
  (where the server takes torrents) and `Open in another app`; then how
  the last auto-detect came out.
- **A link in hand: the link's band** alone, its own name (`dn`) DIM under
  it.
- **Two rows that each open a sheet**: `Metadata` — artist · album · year,
  or GOLD `! type the details` where the name gave neither artist nor
  album — and `Destination` — the landing place as crumbs, the library
  first; it is the folders between that give way (`Music › … › Album`),
  never the library. Then, for a file, `[ ] Force fresh download`.
- **The primary**, tall and the block's width, `Add torrent ▸`; under it
  `checking for files already on the server…` while the check is out, or
  the last refusal, GOLD, whole, on as many rows as it needs.
- **What was sent**, `FROM TORRENTS`: a row for each torrent this player
  handed this server from this account, newest first — ✓, the folder, the
  folder it sits in, when. Nothing is drawn until there is one.

Short of rows the blanks give way — the one under the title, then the
form's from the bottom up, then the one over the list — until the list
has three rows.

## Behavior contract

### The source

1. **Exactly one of file / magnet.** Picking a file clears the magnet
   field; typing a magnet drops the file. Submitting with neither (or
   both) is refused in words.
2. The **file picker is typed to torrents** (extension `.torrent`, MIME
   `application/x-bittorrent`) and starts in the platform's Downloads —
   a torrent almost always arrives through the browser.
3. Picked bytes pass a **structural gate** (`isTorrentFile`: a bencoded
   dict carrying an `info` dict) — enough to reject an mp3 picked by
   mistake, named per file: "*name* is not a torrent file".
4. The **magnet field validates live** (a v1/v2 infohash in `xt`); an
   invalid one is marked in place, not at submit.
5. Once a file is loaded, the magnet field's spot shows the **file chip
   and the hand-off** instead (clause 60) — the two sources never show
   together. *Here: the file's card takes both boxes' place, the hand-off
   a way under it; a link in hand takes them the same way.*
6. *Here:* **a paste is taken whole**, wherever the hand is, while nothing
   stands over the room: a `magnet:` link fills the link box (and lets a
   loaded file go) — one that is no magnet puts the hand on the box to
   mend it; anything else is read as a path, since a file dropped on a
   terminal arrives as one, and loaded — a folder opens the typed picker
   on itself, and what is no torrent says so under the two boxes. Over a
   sheet a paste is its box's own, typed. While a check or an add is out
   a paste waits with the rest of the form.

### Metadata and the destination

10. Metadata pre-fills from the best name available: the torrent's own
    `info.name` (a byte-level bencode walk; never the whole info dict),
    the magnet's `dn` parameter, or the filename — through the loose
    music parser (artist − album, year token, junk stripped). The parse
    only pre-fills; it gates nothing.
11. **Auto-detect** (file sources, when the server offers it) asks
    `/torrent/auto-detect` and applies the answer, saying how sure it is
    ("detected" vs "best guess"); a miss says the server found nothing.
    *Here: the answer stands under the card's ways and in the Metadata
    sheet until the source changes — "Metadata detected" in the OK
    colour, a guess or a miss GOLD behind its `!`. A server that takes no
    torrents is not offered the question.*
12. The **destination path autofills** from the library's template
    (`{{ARTIST}}/{{ALBUM}}` and friends), resolved client-side with the
    server's own sanitizer mirrored — the preview must match what
    `/torrent/add` will accept. No template → legacy `Artist/Album`.
13. **Editing the path by hand is sticky**: metadata edits stop
    recomputing it. A fresh source resets the stickiness.
14. The **preview line** shows the real landing spot:
    `/<vpath>/<path>/<contents>`. *Here: in the Destination sheet, under
    the path's box; the page's row shows the same place as crumbs.*
15. **Rename root folder** is ON by default — the whole point of the
    resolved path is that the folder matches it, and a scene-release name
    rarely does. A rename that fails after a successful add is a warning,
    not a failure. *Here: a checkbox in the Destination sheet, what it does
    DIM under it.*
16. *Here:* **Metadata** is a sheet of three bordered boxes — ARTIST,
    ALBUM, YEAR on one row — with `Auto-detect metadata` at its foot where
    the server can be asked, and `[ Done ]`. **Destination** is a sheet:
    the libraries as radios where the server has more than one, the PATH's
    box, the preview, how a hand edit behaves, the rename checkbox,
    `[ Done ]`; its title wears the place. Both edit the form as they are
    used: `[ Done ]`, Enter, Esc and a click outside all only close them.

### Submit and the seed-existing check

20. File submissions run the **seed-existing check first** (unless
    *force fresh download* is ticked; magnets skip it — no file list to
    hash). The outcomes:
    - **seeded** — already in the library; the client is now seeding it.
      Done, said in words.
    - **already_in_daemon** — the torrent client already has it. Done.
    - **invalid_torrent** — refused with the server's words.
    - **daemon_error** — the *check* failed, not the add: say the check
      was skipped and fall through to a fresh download.
    - **partial_match** — some files exist elsewhere: the **match
      picker** (clause 21).
    - **no_match** (or anything unrecognized) — fall through to add.

    *Here:* what went well is the note's and a row in the list — an add,
    and a torrent now seeding from disk (the server names its library
    alone, so its row carries the torrent's own name). A torrent the
    client already had leaves no row. **A refusal keeps the form** and
    stands under the primary in the server's words, whole — the seed
    check's, the add's, a form that can't be sent — until the form
    changes or the next press goes out. While either call is out the
    primary reads `Adding…` and nothing on the form acts.
21. The **match picker** lists each candidate location
    (`vpath/relativePath`, matched-of-total, missing count) — choosing
    one adds the torrent AT that location (seed what's there, fetch only
    what's missing); the last row is always **download fresh** at the
    typed path. *Here: a sheet — each place on two rows (its crumbs, then
    how many files are there and how many would download), the fresh
    download last with the destination under it. The dot is the choice in
    the making: a click or ↑ ↓ moves it, `[ Add torrent ]` or Enter takes
    it, Esc backs out with nothing added. Five places at most.*
22. `/torrent/add` answers with the torrent's name and where files will
    land; a duplicate is its own wording ("already added"), not an error.
23. *(Record split, settled here for the webapp's side: this surface also
    words `match_unmapped` — files all present but the daemon's path
    mapping unconfirmed, "ask your admin to run auto-detect, then retry"
    — and `pad_files_missing` — a hybrid torrent this client type can't
    seed without re-downloading boundary pieces. The mobile screen falls
    through and lets `/add`'s 412 speak. See the deviations log.)*

### Receiving, and being the default

50. The app **registers for** `.torrent` (view + share) and the
    `magnet:` scheme — on most systems nothing else claims them, so a
    tapped magnet just arrives. *Here: the installers' job (Phase 8);
    the app ships the seam they launch.*
51. The **ask-me switch** in Settings decides what an arrival does
    (chooser vs straight to the screen). Stored as "skip", shown as
    "ask" — the sheet's checkbox sets it, the switch is the only way
    back.
52. ~~The **default-app row** in Settings opens the OS's own
    open-by-default screen~~ — *not ported; see the deviations log.*

### The hand-off

60. **Open with…** hands the loaded `.torrent` to a real torrent client
    (the system chooser), falling back to the share surface when none is
    installed, and saying so when nothing will take it at all.
61. The hand-off **leaves the form standing** — nothing was submitted, so
    backing out or picking a different file both still work.

## Wording

*(2026-11-10, the canvas.)* The page shows words; the tips line names
their keys. Three more of the record's keys came over from its ten
`.arb` files — `torrentMetadataLabel`, `torrentDestinationLabel` and
`torrentOr` (`gui.tor.metadata`, `gui.tor.destination`, `gui.tor.or`).
New here, in the same ten languages: `gui.tor.paste_link` ("Paste a
magnet link", each language's Add from YouTube placeholder with the
record's word for the link), `gui.tor.list_title` ("From torrents", beside
"From YouTube"), the card's `gui.tor.files_*` plural family and the tips
line's `gui.tips.tor_ready`. `Paste`, `Done`, "type the details", "Show
in folder" and the composed tips' pieces are Add from YouTube's own.
Gone, with the label column they named: `gui.tor.hint`, `gui.tor.source`,
`gui.tor.magnet` and `gui.tips.tor_library`.

The record's `torrent*` and `settingsTorrent*` keys were carried over
from all ten `.arb` files into `locales/*.yml` (`gui.tor.*`, `gui.set.*`);
the strings new to this surface — the gate's standing, the picker, the
hand-off's outcomes, the two webapp seed-check sentences, the tips —
were written here in the same ten languages. The parser, sanitizer and
validator wordings above quote the record's English. Since 2026-11-09 the
nav row's `gui.nav.add_torrent` is new — "Add torrent", ja トレント追加, zh
添加种子, and the GUI's add sign for the verb in de es fr it pl pt
(`+ Torrent`) and ru (`+ Торрент`), where no verb fits the nav's twelve
cells (fr `Ajout torrent` and pl `Dodaj torrent` are 13) and the bare
source would read as a section, not an act (Italian's Settings heading
is that same `TORRENT`) — the owner's choice over an abbreviated verb,
2026-11-09 (add-from-youtube.md's log) — and the doorway's
`gui.set.tor_add` and `gui.set.tor_add_desc` and the room's
`gui.tor.back_tip` are gone; the room's tips (`gui.tips.tor_room`) end at
"t type a path".

## Out of scope here

- **The admin side** (torrent client setup, path-mapping auto-detect,
  per-vpath templates authoring) — the webapp admin page's own feature.
- **The `/torrent` mobile-web page** — the reduced sibling, not the
  record.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| The screen's column of fields | **One centred block** (see States & flows): the two boxes, the card, two rows that open sheets, the primary under them, what was sent under that |
| System file picker, typed, starting in Downloads | The **native file dialog** first — the tall frame `Choose a .torrent file…`, its letter `b` — the wizard's own backends (`src/setup/picker.rs`) grown a file-typed sibling: osascript `choose file of type {"torrent"}` on macOS (an NSOpenPanel from a terminal process never fronts), rfd's IFileOpenDialog on Windows, the XDG portal through ashpd on Linux — each filtered to `.torrent` / `application/x-bittorrent` and started in `~/Downloads`, run on a thread so the loop stays live. Where no dialog can open (SSH, no session bus, a refused osascript) the room says so and falls back to the kit's **path modal for files** (the wizard's typed completion re-drawn): the line editor starts in `~/Downloads/`, suggestions are that folder's sub-folders and `.torrent` files only, Tab completes, Enter descends or loads — also reachable on purpose (`type a path ▸` under the frame, its letter `t`). A dropped file's path lands in the room itself: a whole paste loads it (clause 6). The structural gate stays (clause 3) whichever road the file took. Since 2026-11-10 the typed road is a sheet: the path's bordered box, the folder's sub-folders DIM behind a `▸` and its `.torrent` files under it, six at a time; it stands where it would with its list full and grows downward, so the box holds one spot |
| Magnet TextField | The link's band: the kit's bordered box, `Paste a magnet link`, with `Paste` beside it where the build can read the clipboard (the window; a terminal draws none and its box takes the band). ACCENT with the caret while it has the hand; `[X]` clears it. The live infohash mark is a GOLD line under the band (clause 4) |
| Opened-with (intents) | The **CLI seam**: `mstream-player gui --torrent <file-or-magnet>` as the `IncomingTorrent` equivalent — the chooser / ask-me flow rides it unchanged, and the arrival is logged |
| OS registration (manifest) | The **Phase-8 installers'** job: macOS document types + `magnet:` URL scheme on the .app, Windows file association, Linux `.desktop` MimeType — all launching the seam above. The app ships the seam; the installers ship the claim. |
| Android "open by default" settings screen | **Not ported** — no OS screen to open from a terminal app; the row returns with the installers (deviations) |
| Hand-off chooser (ACTION_VIEW) | `open` / `xdg-open` / `start` on the file, **staged** into the player's own temp folder — which doubles as the loop guard: an arrival from that folder is our own file coming back, i.e. this player is the default app, and the room says so instead of offering the hand-off again |
| Preflight banner | The sonic room's pattern inverted: the nav row always stands (no ping flag exists; the Settings doorway did the same); the room itself says the server's reason in gold under its title — whole, on as many rows as it needs, four at most — and the primary is off with the reason on its tip. The form stays usable: a torrent that can't go to this server can still be handed on |
| Chooser + match-picker sheets | **Sheets**, Add from YouTube's (66 cells, standing where a room's window does): Torrent received — what arrived BOLD, the question, `Add to mStream ▸` (the primary) and `Open in another app` side by side (one over the other where a language's words are wider than half the sheet), the don't-ask box; Files already exist — clause 21 |
| Snackbars | The note line for what went well; a refusal on the page, whole (clause 20) |
| Material TextFields, 56 px each | **Two rows that each open a sheet** of the kit's bordered boxes (clause 16). Until 2026-11-10 they were 1-row fields in a label column — chosen for the 24-row floor, which the rows keep: at 100×24 the title row holds the primary and the list keeps four rows |
| (none: the screen pops) | **What was sent** — the list under the form (decision 6 of the canvas's) |

## Decisions on the open questions (2026-09-01)

1. **Seed-check taxonomy split** — the webapp's fuller wordings are
   adopted for `match_unmapped` / `pad_files_missing` (they exist
   because the fall-through error confused people); the mobile's
   fall-through stays the record for the other outcomes.
2. **Delivery to a running instance** — v1 opens a second instance; the
   local socket is its own slice if it is ever wanted.
3. **The defaults row** — dropped until the installers exist; noted in
   the Phase 8 plan, which owns the registration.
4. **The hand-off** — kept, as one text verb on the file chip (mid-flow
   is exactly when you learn the torrent isn't for the server).
5. **Where the room lives** — a Settings doorway plus the `--torrent`
   seam; the nav digits are spent, and arriving from outside is the
   record's main road anyway. **Amended 2026-11-09** (the owner's call):
   the room is a Library nav room, `Add torrent` in the TOOLS group after
   Discover — past the digits, the pointer's, with no letter of its own —
   and the `--torrent` seam stays; Settings keeps TORRENTS and the ask-me
   switch alone.

## Decisions on the canvas's open questions (2026-11-10)

The canvas left six to the owner; the owner said "Build it", and each is
built as its boards drew it.

1. **The list is this player's own record.** The record has none, and the
   server keeps no list a user can read (`/admin/torrent/list` is the
   admin's). `torrents.json` in the config folder holds, per server and
   account, the last fifty torrents this player sent: the name, the
   library, the folder, when. A row's Enter or click opens its folder in
   Files; `x` takes the row off. It says what was sent, never how far the
   download is. *Open still: an admin's rows could follow the download
   from the list the Admin tab reads.*
2. **The card says how many files and how big**, read from the `.torrent`
   here (`torrent_meta::torrent_facts`: the v1 file list with its padding
   files left out, a lone file's length, or a v2 file tree). The record
   shows neither. A link has no file list: its own name stands instead.
3. **`Paste` stands in the window build only**, as in Add from YouTube. A
   file dropped on the window is still not handled by any room.
4. **`Force fresh download` stays on the page; `Rename the torrent's root
   folder` is the Destination sheet's.**
5. **The rows read `Metadata` and `Destination`**, the record's words for
   this screen (Add from YouTube's says `Saves to`).
6. **Files already exist takes a choice and a button** (clause 21): a
   click on a place only moves the dot.

Mine, where the canvas named none: the letters `e` (Metadata) and `l`
(Destination) beside the room's `b t x o d`.

## Deviations log

- **2026-08-31 — No design canvas planned**: the screen is a form — kit
  inputs, modals, a banner — every element already drawn once.
- **2026-09-01 — Clause 52 not ported**: an app cannot open a terminal
  into "System Settings › Default apps"; with nothing registered yet
  there is nothing to change either. The Phase 8 installers own the
  registration and the row comes back with them.
- **2026-09-01 — 1-row fields, not the kit's 3-row cards** (translation
  notes): the room's height budget at the 24-row floor.
- **2026-09-01 — The hand-off says so on success**: the record stays
  quiet (the system chooser is up); a terminal that just spawned `open`
  has nothing visible to show, so the note says "handed to the system's
  torrent app — the form stays as it is". "Nothing will take it" and
  "could not hand it over" keep the record's words.
- **2026-09-01 — A staged hand-off doubles as the loop guard**: the
  record's system chooser never offers the app itself; `open` may, once
  the installers register us. An arrival from the staging folder is
  named ("mStream is the default app for torrents — choose another app
  in your system settings") and taken, never bounced again.
- **2026-09-01 — A typed path beside the native picker** (review): the
  record has only the system picker; this surface keeps the typed
  modal as a named second road, because the dialog cannot exist over
  SSH and a terminal is where paths get pasted and dropped.
- **2026-09-01 — Done resets the form in place**: the record pops its
  screen; the room has nowhere to pop to, so a success (added, seeded,
  already there) clears the source and keeps the library and toggles
  for the next one.
- **2026-09-26 — The review's fixes** (the security lane). The Windows
  hand-off goes through Explorer, not `cmd /c start`, which read `&`, `|`,
  `^` and `%VAR%` in a magnet or a file name as its own syntax (a magnet's
  `&dn=…` became a command). A typed path or `--torrent` must be a regular
  file of at most 16 MiB before it is read. Staged files go under the
  user's own config directory (0700), created fresh, never through a file
  another user planted in the shared temp dir. The room reaches the
  server the session's way (a tunnel bridge's loopback token, the entry's
  trust — a bare client was dropped by the bridge), refuses a read-only
  peer (multi-server clause 26), and drops a check's or an add's answer
  that lands after a server switch.
- **2026-09-26 — The follow-up: glyphs out of the strings.** The path
  preview's angle quotes (clause 14) come from code, behind the same
  console gate as every other glyph; the locale strings carry words only
  (docs/ui-kit.md, Glyphs). The same for the DJ room's bullet and the
  rating's star.
- **2026-11-09 — The TORRENTS group becomes ADD MUSIC.** Add from YouTube
  (docs/ux-contracts/add-from-youtube.md) shares it: `Add a torrent ▸`,
  `Add from YouTube ▸` — shown only under its own gates — and `[✓] Ask
  what to do with torrents`. The key `gui.set.torrents_group` is kept,
  its value new in the ten languages; the Portuguese row's description
  reads the phone's Brazilian ("um arquivo .torrent ou um link magnet")
  beside its new neighbour. *Superseded the same day, below: the group
  reads TORRENTS again, the doorways gone.*
- **2026-11-09 — The room moves to the nav** (the owner's call; decision
  5 amended, entry point 1; add-from-youtube.md's log too). `Add torrent`
  and `Add YouTube` join the TOOLS group after Discover — rows 18 and 19
  with all five tools, higher where the group's gated rooms are absent,
  as it packs the rooms it shows — last in `NAV`, no digit, no letter.
  The Settings doorway, its
  description, the room's `◂` and its tip "Back to Settings — Esc" are
  retired, and the tips line loses "· Esc back": a nav room has no way
  back, so Esc stows the cursor and a second leaves nothing. Settings
  keeps TORRENTS — each language's word from before ADD MUSIC — over `[✓]
  Ask what to do with torrents` alone. An arrival (`--torrent`, the
  chooser's Add, our own file coming back) takes the nav's own way in:
  the Library screen back from wherever it was, Manage servers let go,
  then the form filled. The row shows wherever the doorway did — always
  — and opening it asks the gate as before. Where no verb fits the row's
  twelve cells (de es fr it pl pt ru) it reads `+ Torrent`, the GUI's add
  sign for the verb — the owner's choice (Wording).
- **2026-11-10 — The room rebuilt to its canvas** ("Add torrent · GUI").
  The label column is gone: States & flows, clauses 5, 6, 11, 14–16, 20
  and 21, the translation notes and the canvas's decisions above say what
  stands in its place. The gate, the parser, the template resolver, the
  seed check's taxonomy, the hand-off and the `--torrent` seam are
  untouched. The tests' boards are the canvas's seventeen, written from
  its own files cell for cell with their tokens; none needed a patch.
- **2026-11-10 — What the rebuild mends.** Read off the old room's own
  frames at 100×30 with the queue open: `[ ] Force fresh download` was
  not drawn at all (the second checkbox did not fit beside the first in
  the room's 47 cells); `Open in another app · Auto-detect metadata` ran
  through the queue's rule; the preview lost its start, and with it the
  library, once library and path passed 19 characters; the match picker
  drew `Download fresh anyway` on its bottom border; the gate's "asking
  whether this server takes torrents…" was cut at 32 cells beside the
  title; the two longest refusals (about 180 and 250 characters) went to
  the bar's one-row note; a link pasted in a terminal with the hand stowed
  arrived as keys, its `t` opening the typed picker; and the wheel never
  reached the typed picker's suggestions, a modal's (the hub hands it
  over now, as it does the DJ's layers').
- **2026-11-10 — The hand.** The room still opens with it stowed. ↓ picks
  it up: the file's box, `type a path ▸`, the link's box — or, a source in
  hand, its card (or its link), its ways, `Metadata`, `Destination`, the
  checkbox, the primary — then the list's rows; Enter does the stop's own
  act, Esc stows. The hand down and a source in hand, Enter is the
  primary's. The link's box takes what is typed while it has the hand;
  Enter there moves on. A letter acts where its control is drawn; the
  capitals of the room's own letters are the room's too. The queue panel
  holding the keys, the room yields every one.
- **2026-11-10 — A name that says nothing.** The canvas's NoDetails board
  shows a file whose name gives neither artist nor album, `Metadata`
  pointing at its sheet in GOLD and the primary off. By the record's
  parser a name with no artist in it is taken whole as the album, so the
  board's state is a torrent — or far more often a link — that carries
  no name at all; a named one adds under its own name, as on the phone.
- **2026-11-10 — Torrent received keeps its hand down** until a walking
  key picks it up, on the primary: the sheet opens as its board drew it,
  and Enter adds. Space ticks the don't-ask box wherever the hand is.
- **2026-11-10 — No server at all** says so under the title, DIM, on its
  two rows; beside the title the sentence was cut.
- **2026-11-10 — Where a paste arrives as keys.** A Windows console
  delivers a paste as typed keys, never whole (Add from YouTube's
  `whole_pastes`). There the room opens with the hand on the link's box
  while it has no source, so a pasted link lands in it rather than among
  the room's letters; everywhere else the hand is stowed, as the canvas
  drew it. And Enter in the link's box, on text that is no link at all,
  reads it as the path it must be and loads the file — the whole paste's
  own rule, for a path that was typed.
- **2026-11-10 — What a run has shown, and what none has.** The pty smoke
  (`smoke/gui/scenario_torrent.py`) walks the page on a real server with
  no torrent client: both whole pastes, the card's facts, the two sheets,
  the typed picker, the server's own refusal under the title — and the
  list, from two rows the pass writes into `torrents.json` itself: read
  when the room opens, one taken off the list and off the file with `x`,
  the other opened in Files. (Under test the list is memory's alone, so
  the file's own round trip is this run's to show.) The window build was
  looked at on macOS: Torrent received over Files, the room, both sheets.
  No run has reached a server that takes torrents — no rig has a client,
  and mStream's test helpers script none — so the add itself, the seed
  check's outcomes, a refusal's real words and a row arriving in the list
  are the unit tests' alone, through the answers they hand the room.
