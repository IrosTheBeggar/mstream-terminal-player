# Admin screen

| | |
|---|---|
| **Design of record** | The "Admin GUI Hybrid" canvas — the Main (176×46), Middle (136×40), Band (136×52), Narrow and Minimum boards — for the hallway, the placements of the log and the footer; and this repo's own admin rooms, `src/admin/*` (Libraries, Users, Backups, Discovery, Federation, Torrents) at commit `2a3b4a3` (2026-10-02), hosted whole. The GUI adds the tab, the hallway, the log and the way back. |
| **Server API** | The six rooms' routes, unchanged, plus `GET /api/v1/admin/logs/recent?since=<seq>` — the server's main log ring (mStream `src/api/admin.js` → `logger.getRecentLogs` → `LogRing.read`): `{entries: [{seq, t, level, message}], lastSeq, capacity}`, oldest first, every entry with `seq > since`. No level parameter, no limit parameter. And `GET /api/v1/admin/logs/download` (mStream `admin.js`): the server's log files as a zip, streamed as archiver builds it (clause 32). |
| **Already in this repo** | The six rooms, live as `mstream-player admin <room>` on the admin hub's terminal session; the GUI's Stats tab, which showed how a hub page lives under the top bar (the stats-screen contract). |
| **Status** | Implemented 2026-10-02 — this document first, then the hub's hosting seams, the rooms, the log and the tab, on one branch. |

## Intent

The server, one tab away from the music: the admin panel's six rooms on the
session the GUI already holds — no second sign-in, no second terminal — in a
hallway that names them all, with the server's own log running beside the
room when the window has room for it, and a room of its own when it does
not.

## Entry points

1. The top bar's **Admin** tab, third after Library and Stats; the
   **Library** tab is the way back.
2. **`M`** from the Library, Stats and Now Playing; **`M`** or **Esc** from
   the hallway back to the Library.
3. From the hallway, a nav digit or `D` leaves for that room of the
   Library, `T` for Stats, `0` for Now Playing, `V` for the visualizer.

## States & flows

- **No session**: the tab opens on one dim sentence where the room would
  be — "no session — connect to a server and its admin rooms open here",
  on a second row where the window is too narrow for it (most
  translations, at the floor) — and the hallway marks no room open; Esc,
  `M` or the Library tab leave.
- **A server that cannot be reached right now** (a tunnel down): the
  reach's own reason on that line, as on the Stats tab.
- **A room**: loading on entry — every read of the room's, exactly as
  `mstream-player admin <room>` makes them — then the room as it is.
- **Not allowed**: 401 is the hub's sentence ("not signed in — run
  `mstream-player login` first"), 403 and 405 the room's own (not an admin
  here, or restricted by address; the admin panel locked). The log says the same through its own line and stops
  polling until the tab is opened again.
- **The session changes** while the tab is up (a switch, a pasted code's
  dial): the room and the log are rebuilt on the new session's server; the
  room shown and the log's placement are kept. A switch drops both the
  moment it goes out, and the tab says there is no session until the new
  server answers, so a switch that never lands leaves nothing reading or
  changing the old server under the new one's name.
- **Leaving** the tab drops the room and stops the log; coming back builds
  both again and reloads.

## Behavior contract

### The tab

1. **Admin is the third tab**, after Library and Stats, and wears the slab
   while the screen is up, as the other tabs do. `M` opens it from the
   Library, from Stats and from Now Playing; `M` or Esc in the hallway
   leads back to the Library.
2. **The bar and the GUI's note stay**, as on the Library; the queue panel
   stands down, as on Stats.

### The hallway

3. **The hallway is a column at the left**, the Library's nav column's
   idiom: the groups dim at x 1 — SERVER on row 2, NETWORK on row 7,
   WATCH on row 12 — and the rooms at x 3: Libraries, Users and Backups on
   rows 3–5, Discovery, Federation and Torrents on rows 8–10, Log on row
   13. A dim `│` at x 16 runs from row 2 to the row above the bar and
   turns the accent while the room (or the Log room) has the focus. Room
   labels are clipped to 13 cells and group labels to 15. The open row is
   `▸ label` at x 1 in the accent, BOLD — none while there is no session;
   the others are dim at x 3, BRIGHT + BOLD under the pointer. While the
   hallway has the focus, its cursor row wears the slab across x 1–15.
   WATCH and Log show only while a session exists and the log is not on
   screen beside the room — in the Room placement, or when `L` has hidden
   it.
4. **The Log row counts what you have not seen**: a dim ` · 12 new` after
   the label while lines at or above the chosen level have arrived since
   the log was last on screen. Lines the server already held when the tab
   opened, and anything before the log's first answer, do not count;
   after a server restart, the new ring's lines do. The count goes first
   when the row is short of cells.
5. **The tab lands on Libraries** with the focus in the hallway. The room
   last shown is remembered for as long as the player runs, and the tab
   opens on it next time. When the Log room gives way to a docked log
   (the window grew, or `L`), the hallway's cursor moves to the room
   shown.

### The room

6. **The room is hosted whole** in the area right of the hallway, from
   row 1: its first row is left blank (the pick banner's row), its note is
   on its last row, and it draws no header (the top bar names the server)
   and no tips row (the GUI's footer carries the room's hint). A room's
   BETA chip stands at the right of the body's first row. Nothing of the
   room's behaviour changes: the same tables, tabs, modals, gates and
   words as the hub.
7. **Its width is 100 cells while the log is docked as a column**, and
   otherwise runs to the window's right edge; its height runs to the row
   above the bar (above the log's band when the band is up).
8. **Below a room's own minimum it draws what fits** and never asks for a
   bigger window: its tables get shorter. Its modals centre in its area,
   not in the window. Whatever a room draws below its area — and, while
   the log is docked as a column, right of it — is blanked before the
   hallway, the log and the bar are drawn, so a tall form never lands on
   them. At the GUI's 100×24 floor the room has 17 rows.
9. **The session's server serves the room**: the client is built from the
   App's reach for the session's origin — a peer session's parent — with
   the session's token, its trust and, over a tunnel, the bridge's
   loopback token, as on the Stats tab. A room's same-machine affordances
   (the OS folder dialog) apply only when the host is loopback
   (`localhost`, `*.localhost`, `127.0.0.0/8`, `::1`), no tunnel token is
   in play and the session is not a peer's.
10. **Refusals are the hub's**: 401 shows the hub's sentence; 403 and 405
    show the rooms' own.
11. **One room is alive at a time.** Opening another drops the one before.
    The Log room keeps the last room alive but not drawn — still polled,
    its hover cleared — and `L` back, or a click on its row, shows it
    again exactly as it was. Leaving the tab drops the room and stops the
    log's poll; a session change rebuilds both.

### Focus and keys

12. **Three things can hold the focus** — the hallway, the room and the
    log — and Tab and BackTab cycle them, hallway → room → log → hallway.
    The log is in the cycle only while it stands beside the room (Column
    or Band); while the Log room shows, the cycle is hallway ⇄ log; with
    no room (no session) the focus stays in the hallway. Torrents' Choose
    page keeps Tab for itself, as it does in the hub. A drawn room that
    holds every key — a modal or a text field up, Torrents' Connect page —
    has the focus wherever it stood: a modal that opens while the hallway
    or the log has the keys (a folder listing answering after they moved
    on) takes them, and the focus stays with the room until it lets go.
13. **A focused room keeps every key.** The host takes only Ctrl-C (as on
    every GUI screen), Tab and BackTab while the room holds nothing that
    wants them, and `L` unless the room has a modal or a text field open
    — then every key is the room's, whichever of the three had the focus
    (clause 12). The host never takes a digit, `q`, Esc, `M`, `T`, `V` or
    `D` from a focused room: they are the room's flags, tabs and verbs.
14. **Esc at a room's base, or `q`, moves the focus to the hallway** and
    closes nothing, with the hallway's cursor on the room's row. `q` in
    the hallway quits the player, as on every GUI screen.
15. **The hallway's keys**: `↑` `↓` move the cursor; Enter or `→` opens the
    row and gives it the focus (the room, or the log for the Log row);
    Esc or `M` go to the Library, `T` to Stats, `0` to Now Playing, `V` to
    the visualizer; `1`–`9` and `D` to the Library's rooms; `q` quits.
    The transport keys work as on the Library: Space, `p`, `n`, `s`, `r`,
    `A`, `-`, `+` and `=`.
16. **The log's keys**: `↑` `↓` a line, PgUp and PgDn a page, Home the
    oldest line, End or `f` back to following, Enter opens the level
    menu, `y` copies (clause 30), `d` downloads the server's log files
    (clause 32), `o` shows the file the last download saved (clause 33).
    Esc clears a highlight that stands (clause 29); otherwise Esc or `q`
    hand the focus back to the hallway. In the level menu: `↑` `↓` move
    its cursor, Enter picks, Esc closes and keeps the level; every other
    key is swallowed while it is open.
17. **`L` moves the log.** In a window wide or tall enough for the log
    beside the room (clause 21), `L` hides it — the room takes its rows or
    columns and WATCH · Log comes back to the hallway — and shows it
    again; a log that had the focus hands it to the room (or, with no
    room, to the hallway). In a window that has room only for the room,
    `L` opens the Log room with the focus on the log, and from there goes
    back to the last room with the focus on the room.

### The pointer

18. **The room takes the pointer inside its area**, less its first row
    (the pick banner's `[X]` lives there): hover, click, hold, drag and the
    wheel on its own controls, the hub's way. While its modal is up it
    takes every event under the top bar — its own catchers decide, and a
    click elsewhere is swallowed, so a half-filled form is never lost to
    a hallway click — and nothing of the GUI's under the top bar (the
    hallway, the log, the bar) lights or shows the hand meanwhile. A press
    that started in the room keeps the pointer until it is released, even
    on the top bar's row, where a thumb dragged to the top lands. When the
    pointer leaves the room, or the room is hidden (the Log room), the
    room's hover and tooltip are cleared and any press it held is let go,
    so a held arrow never steps on unseen.
19. **The hallway and the log answer on the GUI's surface.** A click on
    the hallway column gives it the focus; a click on a row opens it. A
    click in the log gives it the focus; its level control opens the menu,
    the paused word resumes following, and the header's copy and download
    do what `y` and `d` do. A press on the log's lines, dragged across
    them, highlights them (clause 29): the drag keeps the pointer until
    its release, wherever that lands — over the room, the hallway or the
    bar — so nothing else lights, takes the release or keeps a press of
    its own meanwhile, and what lies under the release lights at once.
    The drag never outlives the log: leaving the tab, `L`, the Log room
    folding back or the mini player lets go of it. A press while it
    holds means the terminal lost the release (some drop it when the
    focus leaves mid-drag): the drag ends at that press's cell, and the
    press then acts as any press. The hand cursor follows whichever
    surface reports a clickable under the pointer.
20. **A GUI modal, the header's server menu, or the log's level menu owns
    the pointer whole while open**, as elsewhere in the GUI. The level
    menu keeps every key too until it closes, even when a room has come
    to hold every key under it.

### The log

21. **Three placements, chosen by the window**:
    - **Column**, 160 columns and wider: the room is 100 wide; a `│` at
      x 118 runs from row 2 to the row above the bar; the log stands at
      x 120 from row 2 — its header, a blank row, then its lines.
    - **Band**, narrower but 48 rows and taller: the room keeps the
      window's width and stops nine rows short; a `─` row from x 19 to the
      third column from the right edge, then the log's header and seven
      lines.
    - **Room**, anything smaller: the log is a room of its own on the
      hallway's Log row, drawn at x 19 from row 2 with a blank row under
      its header.

    The rules beside and above the log turn the accent while it has the
    focus. At 136×52 with the footer off the band's rows are the room's
    note on 37, the rule on 38, the header on 39 and lines on 40–46; at
    176×46 with the footer on, the room's note is on row 39, the rule at
    x 118, the header at (120, 2) and lines from row 4.
22. **The header**: `•` in the ok colour and `following` in the ok colour,
    BOLD — or `• paused` dim, the word a target that resumes following —
    then a dim ` · ` and the level with a dim `▾` (`v` on the legacy
    console), BRIGHT + BOLD under the pointer, a target that opens the
    level menu. The menu is the kit's Dropdown, hanging from the level's
    first cell and kept inside the log: error · warn · info · debug, the
    current one wearing `•`, the cursor the slab. After the level, a dim
    ` · ` and `copy`, then a dim ` · ` and `download`, each dim and
    BRIGHT + BOLD under the pointer with a tooltip, and each drawn whole
    or not at all: the first that does not fit ends the row. While a
    download runs, `download` reads `downloading…` in the accent (the
    kit's busy word) and is no target. At the header's right, dim and only
    while keyboard hints are on, what `L` does here: "L undocks"
    (Column), "L hides the band" (Band), "L back to the room" (the Log
    room); it is the first thing to give way, before the copy and the
    download.
23. **Lines**: newest at the bottom; each is the local time `HH:MM:SS` in
    dim (UTC on a machine without a zone, `--:--:--` when the server's
    time does not parse), two spaces, then the message clipped by cells —
    its first line that is not blank, ` …` after it (` »` on the legacy
    console) when another non-blank line follows (a stack trace).
    Info and debug are in the text colour, warn in gold, error in the
    danger colour; debug is not dimmed. With nothing to show, one
    sentence: "waiting for the server's log…" (dim) before the first
    answer; "this server keeps no live log — its logBufferSize is 0" (dim)
    when the ring is off; "no lines at this level yet" (dim) when no line
    passes the level; the failure (gold) when the last poll failed.
24. **Following and paused.** The log follows the newest line until it is
    scrolled up — a key or the wheel — which pauses it and holds the view
    still while new lines arrive under it. End, `f`, a click on the paused
    word, or scrolling back down to the newest line resume following.
25. **Levels are filtered in the player**, since the route has no level
    parameter: a line shows when its level is at or above the one chosen.
    `http`, `verbose` and `silly` count as debug, an unknown level as
    info; the default is info.
26. **Polling**: every 2 s while the tab is up, in every placement — the
    Log row's count needs it — on a worker thread of the log's own, so a
    slow answer never holds a room's requests behind it, and never two
    requests at once. After a failure the next try waits 10 s; after 401,
    403 or 405 the log stops until the tab is opened again. The cursor is
    the last `lastSeq`; an answer whose `lastSeq` is below the cursor is a
    restarted server, and the log starts over from the new ring. The
    player keeps the newest 1000 lines.

### The footer

27. **The footer follows the focus**: with no room, "Esc library"; in the
    hallway, its keys; in the room, the room's own hint followed by
    ` · Tab focus · L log` — only `L log` while the room keeps Tab, and
    nothing while it holds a modal or a text field — the tail only when
    the whole line fits the window, otherwise the room's hint alone; in
    the log, its keys — `y` copy and `d` download among them — or the
    menu's while the menu is open. Every line of the tab's own fits 99
    cells in every locale. A focused room's own hint is the room's,
    written for the hub's full width, and may not: the tail goes first,
    then the window's edge cuts the hint.

### The window

28. **The room's field is the window's field.** In the desktop window
    (`gui --window`, or a bare `mstream-player` in the desktop flavour) a
    room's field that has the keyboard is a field as the GUI's own are:
    the paste chord (Cmd+V on a Mac, Ctrl+V elsewhere) types the
    clipboard into it, the input method is on while it has the keyboard
    and off once it lets go, the method's candidate window floats at its
    caret, and what is being composed is drawn in the field before the
    caret until it is committed. Only while the room is drawn: not under
    the Log room, not with no session. A GUI modal laid over the room
    (the add-server form, which opens on its chooser) and the log's level
    menu take the keys as keys, never a paste or a composition, and a GUI
    modal's own field composes in itself, never in the room's beneath;
    the header's server menu passes every key but Esc on to the field,
    closing as it goes, as it does over the GUI's own fields. A terminal
    changes nothing here: it types a paste as keys and composes in its
    own UI. Hosted, a room's caret holds steady rather than blink with
    the GUI's own fields' (the deviations log, 2026-10-02).

### Copying and saving the log

29. **A press-drag across the log's lines highlights them**: whole
    lines, from the line the press landed on to the line under the
    pointer, in every placement. A highlighted line is filled across the
    log's width in the selection colours (the accent behind, the
    on-accent text), its clock and message in them rather than its
    level's colour. The highlight is held by the lines' sequence numbers,
    so arrivals and the ring's drops leave it on the same lines, and it
    goes once its last line has left the ring. A drag above the first
    line drawn or below the last stops at that line; the log never
    scrolls by itself at an edge, but the wheel scrolls during a drag
    and the highlight's end follows the line under the still pointer.
    A plain click on the lines (a press released without leaving its
    cell), Esc, a level picked, a restarted server's ring, a new session
    and the tab opened again clear it; `L`, Tab and `q` keep it.
    Following carries on under it. The drag region is the line rows and
    the empty rows under them, never the header or the failure's row.
    Apple Terminal reports a press as an instant click pair (ui-kit.md,
    the phantom-release dialect), so there no drag highlights; `y` still
    copies every line shown. The highlight is a run of text to copy,
    not the list cursor (ui-kit.md, List cursor): no row verb acts on
    it and no key moves it.
30. **`y`, or the header's copy, copies** the highlighted lines, or every
    line shown at the chosen level when none is highlighted, oldest
    first. Each line is its local time `HH:MM:SS`, two spaces, `warn` or
    `error` and two spaces for those levels (winston's own words, never
    translated), then the whole message — every line of it, not only
    the first the row shows, its control and bidi characters dropped,
    tabs as four spaces, cut at mStream's own 4000 characters — with its
    later lines indented ten spaces, so a stack trace reads under its
    time. Lines are joined by newlines, none after the last. The note
    says which went — "the highlighted lines are on the clipboard" or
    "every line shown is on the clipboard" — and carries no count; a
    copy handed to the terminal says it may not have arrived, a copy no
    route took says so in gold, both pointing at `d`; with nothing shown,
    "no lines to copy yet".
31. **Where a copy goes is the kit's** (ui-kit.md, "The clipboard is the
    kit's"), by where the player runs: in the desktop window, the
    system pasteboard, then the platform's tool, and never OSC 52, which
    would land in whatever shell launched the window; in a terminal over
    SSH (`SSH_CONNECTION`, `SSH_CLIENT` or `SSH_TTY` set), OSC 52 alone,
    since the machine's own clipboard is not the user's; in a local
    terminal, the pasteboard where the build links one (the desktop
    flavour), then the tool, then OSC 52. The tools are `pbcopy` on a
    Mac, `clip.exe` on Windows, and elsewhere `wl-copy` under Wayland,
    then `xclip` and `xsel` under X11; one that fails or takes more than
    a second is passed over. Text over 1 MiB is never written as OSC 52.
    The Discovery and Federation rooms' `y` goes the same way: "ticket
    copied" when the clipboard took it, their sentence about the
    terminal when it went as OSC 52, their failure sentence when nothing
    did.
32. **`d`, or the header's download, saves the server's log files.** It
    asks the session's server for `GET /api/v1/admin/logs/download`, the
    zip mStream builds as it streams it, on a one-shot thread of its own
    with the log's client — never the poll's, so a slow zip never holds
    the tail — and waits up to the client's ten-minute ceiling. One at a
    time: while it runs, `d` does nothing, the header reads
    `downloading…` and the note "downloading the server's logs…". A whole
    zip is saved as `mstream-logs-<server>-<YYYYMMDD-HHMMSS>.zip`: the
    session's server's host without its port (a peer session's parent; a
    Quick Connect tunnel as `quickconnect-` and the first twelve
    characters of its id), then the machine's local time. It lands in
    the Downloads folder — `~/Downloads` on a Mac, the profile's
    `Downloads` on Windows, elsewhere `XDG_DOWNLOAD_DIR`, then the
    `XDG_DOWNLOAD_DIR` line of `user-dirs.dirs`, then `~/Downloads` —
    or, with none of them there, the player's config folder. A file
    already there is never replaced: the name gains `-2`, `-3`, up to
    `-99`; with all of them taken the note names the file and saves
    nothing. The note says it saved and that `o` shows it, then where,
    `~/` for the home folder off Windows. The path comes last in every
    language, and where the note's cells (the window less 70) cannot
    hold it all, it is clipped at its front to what is left after the
    words, so the words stay whole and the file's own name stays in
    sight (ui-kit.md, Table: paths truncate with a leading `…`). A
    server that writes no log files (an empty zip: mStream's
    `writeLogs` is off by default) and one with no download route (404,
    or an answer that is not a zip) still leave a file: the lines
    shown, written as the copy writes them, saved as `.txt` under the
    same name, with a note that says why; with no lines shown either,
    the note says nothing was saved. A zip cut short (no
    end-of-central-directory record) is never saved, and the note says
    `d` tries again. A refusal speaks the hub's gate sentences (401, 403,
    405) and touches no folder; any other failure says "could not
    download the server's logs" with the client's words. A download
    still running when the tab is left saves its file all the same, and
    its note shows when the tab is next up.
33. **`o` shows the file the last download saved** in the file manager
    rather than opening it, since opening the zip on a Mac would unpack
    it into Downloads: Finder selects it (`open -R`), Explorer selects it
    (`/select,`), and elsewhere the freedesktop opener opens its folder.
    With nothing saved yet, the note says so and points at `d`. With
    `MSTREAM_NO_OPEN` set, as on a machine with no desktop, nothing is
    launched and the note gives the file's full path, clipped at its
    front like the download's only where the note cannot hold it; an
    opener that fails says so in gold, with the path, the same way.
34. **The window's copy chord is the log's `y`.** In the desktop window,
    Cmd+C on a Mac (not with Shift), and Ctrl+Shift+C or Ctrl+Insert
    elsewhere — never plain Ctrl+C, never with Alt — copies as `y` does
    while the log is on screen and either has the focus or holds a
    highlight, and nothing laid over the screen holds the keys: no GUI
    modal, no level menu, no room's modal. The C is the layout's letter,
    or the key at C's place on a layout without Latin letters, as the
    paste chord's V is (clause 28). The window takes the chord whether
    or not the log copies, so off a Mac Ctrl+Shift+C no longer reaches
    the GUI as Ctrl+C, which quits. To the header's open server menu the
    chord is a key like any other, on every screen: it closes the menu
    and goes on. A terminal changes nothing here: its own copy chord
    copies the terminal's text, and `y` copies the log's.

## Wording

| Key | English |
|---|---|
| `gui.top.admin` | Admin |
| `gui.admin.no_session` | no session — connect to a server and its admin rooms open here |
| `gui.admin.group_server` / `group_network` / `group_watch` | SERVER / NETWORK / WATCH |
| `gui.admin.room_libraries` / `room_users` / `room_backups` | Libraries / Users / Backups |
| `gui.admin.room_discovery` / `room_federation` / `room_torrents` / `room_log` | Discovery / Federation / Torrents / Log |
| `gui.admin.log_new` | · %{n} new |
| `gui.admin.tips_hall` | ↑ ↓ rooms · Enter open · Tab focus · L log · Esc library · q quit |
| `gui.admin.tips_focus` / `tips_log_key` | Tab focus / L log |
| `gui.admin.tips_log` | ↑ ↓ scroll · End follow · Enter level · y copy · d download · Tab focus · L log · Esc hallway |
| `gui.admin.tips_log_menu` | ↑ ↓ level · Enter choose · Esc close |
| `gui.admin.log.following` / `paused` | following / paused |
| `gui.admin.log.level_error` / `level_warn` / `level_info` / `level_debug` | error / warn / info / debug |
| `gui.admin.log.waiting` | waiting for the server's log… |
| `gui.admin.log.empty` | no lines at this level yet |
| `gui.admin.log.off` | this server keeps no live log — its logBufferSize is 0 |
| `gui.admin.log.failed` | could not read the server's log |
| `gui.admin.log.hint_undock` / `hint_hide` / `hint_room` | L undocks / L hides the band / L back to the room |
| `gui.admin.log.copy` / `download` / `downloading` | copy / download / downloading… |
| `gui.admin.log.tip_copy` | Copy the highlighted lines, or every line shown — y |
| `gui.admin.log.tip_download` | Save the server's log files to your Downloads folder — d |
| `gui.admin.log.copied_highlight` | the highlighted lines are on the clipboard |
| `gui.admin.log.copied_all` | every line shown is on the clipboard |
| `gui.admin.log.copied_terminal` | handed to the terminal's clipboard, if it allows it — d saves the log to a file |
| `gui.admin.log.copy_failed` | could not reach the clipboard — d saves the log to a file |
| `gui.admin.log.copy_nothing` | no lines to copy yet |
| `gui.admin.log.fetching` | downloading the server's logs… |
| `gui.admin.log.saved` | saved · o shows it · %{path} |
| `gui.admin.log.saved_lines` | saved the lines shown — this server offers no log download · o shows it · %{path} |
| `gui.admin.log.saved_no_files` | saved the lines shown — this server writes no log files · o shows it · %{path} |
| `gui.admin.log.zip_cut` | the server's zip arrived cut short — d tries again |
| `gui.admin.log.save_nothing` | this server sent no log files, and there are no lines here to save |
| `gui.admin.log.download_failed` | could not download the server's logs |
| `gui.admin.log.save_failed` | could not save the logs: %{err} |
| `gui.admin.log.names_taken` | could not save the logs: %{name} and 98 numbered copies are already there |
| `gui.admin.log.no_folder` | could not find a folder to save the logs in |
| `gui.admin.log.file_at` | the file is at %{path} |
| `gui.admin.log.show_failed` | could not show the file — it is at %{path} |
| `gui.admin.log.nothing_saved` | nothing saved yet — d saves the server's logs |
| `p2p.copied_clipboard` / `fed.copied_clipboard` | ticket copied |
| `admin.hint_empty_hosted` | b browse · t type a path · Esc back |
| `admin.hint_rows_hosted` | ↑ ↓ select · b browse · t type a path · Esc back |
| `usr.hint_empty_hosted` | a add the first user · Esc back |
| `usr.hint_rows_hosted` | ↑ ↓ select · a add a user · Esc back |
| `usr.public_3_hosted` | This room signs in as the user it creates; the player keeps its own session — sign it in as that user in Manage servers, or the other rooms are refused. |

Changed: `gui.tips.base` names `M` ("· M admin" before "· q quit"), and
`gui.admin.tips_log` names `y` and `d` (2026-10-02). The rooms' own
strings (`admin.*`, `usr.*`, `bak.*`, `p2p.*`, `fed.*`, `tor.*`) are
unchanged but for the two `copied_clipboard` keys; the hub's 401 sentence
is the hub's.

## Out of scope here

- **The Overview**, the canvas's first hallway row (rooms and network at a
  glance): a phase of its own. It will sit on row 2 and move the groups
  down two rows, which is where the canvas draws them.
- **The rooms' one-line summaries** in the hallway, which the canvas
  sketches beside the labels.
- **A sign-in page.** A 401 says the hub's sentence; signing in is the
  Servers form's or `mstream-player login`'s.
- **The folded hallway and the compact bar** the canvas offers as options.
- **Server-side levels and limits** for the log: the route has neither, so
  the player filters and caps.
- **Highlighting part of a line, and highlighting from the keyboard**:
  the highlight is whole lines, made with the pointer; `y` with nothing
  highlighted copies every line shown.
- **Scrolling by itself at an edge** during a drag: the drag stops at
  the first or last line drawn, and the wheel scrolls under it.
- **A drag in Apple Terminal**, which reports no held button (clause 29).
- **Choosing the folder a download lands in**: it is the Downloads
  folder, and `o` shows where.
- **The Users room's first-user session**: in the hub, the first user
  added to a public server becomes the hub's session; the GUI does not
  adopt it, so the room reloads as the hub would and the GUI keeps its own
  session. Hosted, the room's public-mode note says so: sign the player in
  as that user in Manage servers, or the other rooms are refused.

## Translation notes

- **The GUI's own surface.** The tab is `src/gui/admin.rs` on the
  `Gui`'s loop, modelled on `src/gui/stats.rs`; the log is
  `src/gui/server_log.rs`; the rooms are hosted through the hub's
  `HostedRoom` face (`src/admin/mod.rs`), each room drawing into an area
  it is given rather than the whole frame.
- **Key names** follow each locale's existing room hints: French writes
  Entrée and Échap, Italian Invio, the others Enter and Esc. End is left
  as the key's English name everywhere, as Tab is. Key letters never
  translate.
- **The hallway's labels** fit 13 cells (rooms) and 15 (groups), which
  some titles do not: Japanese says 連合 for Federation (フェデレーション
  is 16 cells) and ディスカバリ for Discovery; Spanish Respaldos and
  Descubrir, Polish Backupy, Russian Бэкапы and Обнаружение; German keeps
  Log for the log, as its footer has no room for Protokoll.
- **The footer's 99 cells.** `gui.tips.base` grew by `M`, so German,
  Spanish, French, Italian, Polish, Portuguese and Russian were tightened
  (a shorter word for play/pause, volume or stats) to keep it there. The
  hallway's line with the room's tail is held to the same 99: French says
  "Tab zone" for the focus key, Japanese "Tab 移動", and the hallway's Esc
  says "back" rather than "library" in French, Polish and Russian. A test
  measures every footer line this tab adds, in every locale, and another
  the hallway's labels.
- **The log's footer re-cut for `y` and `d`** (2026-10-02). With two more
  keys the log's line was cut again to stay within 99 cells: Spanish
  scrolls with "mover", French with "lignes"; French, Polish and Russian
  say "back" for Esc (retour, wstecz, назад), as their hallway lines
  already did; and `d` is the short "save" word in German (speichern),
  French (sauver) and Italian (salva) rather than "download". The
  header's `copy` and `download` are each locale's own word, measured
  only by the header's whole-or-nothing rule (clause 22).
- **A note that names a file ends with the path** in every language
  (`saved`, `saved_lines`, `saved_no_files`, `file_at`, `show_failed`;
  a setup test pins it), since the path is what gets clipped, at its
  front, when the note runs out of cells (clause 32). The saved notes
  were reordered for it: what was saved, then `o`'s hint, then the path.

## Deviations log

- **2026-10-02 — Extracted and implemented.** The deviations from the
  canvas, each deliberate:
  - **`L`, not the canvas's `t`**, moves the log, because `t` is already a
    room's: Libraries types a path on it (`libraries.rs:797`), Federation
    tests a peer (`federation.rs:1416`), Torrents tests its client
    (`torrents.rs:1658`) and the stats page switches its entity
    (`stats.rs:885`). `Char('L')` is bound nowhere.
  - **`M` for the tab**, as `T` is Stats': no screen binds it.
  - **The body starts on row 2**, not the canvas's row 3, since this
    phase has no Overview row above the groups.
  - **The hallway's rule is at x 16** where the Library's is at x 15, so
    a room label has its 13 cells (x 3–15) before it.
  - **The Log row is hidden in the band**, following the canvas's Band
    board over the brief's "not docked as a column": the band is on
    screen, and a row for it would only repeat it.
  - **No "── following · new lines land here ──" row** under the newest
    line: the header already says whether the log is following.
  - **The log header's `L` hint shows only while keyboard hints are on**,
    as every other key name on the GUI's screens does.
  - **Hosted Libraries and Users say "Esc back"** where the hub says
    "q quit", because in the tab `q` and Esc hand the focus to the
    hallway and quit nothing.
  - **Hosted Users' public-mode note** (`usr.public_3_hosted`) says the
    player must sign in as the first user too, where the hub's says the
    panel stays open: the session the room makes is not the player's, so
    after the first user the rest of the tab is refused (Out of scope).
    Its sentences wrap at the column rather than lose their ends.
  - **The footer follows the focus** (clause 27) where the canvas's Main
    board writes one line for all three holders — `Tab hallway / room /
    log · ↑↓ rooms · ←→ the room's tabs · t undock the log · Esc back`:
    one line cannot carry a room's own keys, and the canvas's `t` is `L`
    here.
  - **A room that holds every key has the focus** (clause 12), wherever
    it stood; the plan routed keys by the focus alone, which let a modal
    that opened by itself own the pointer while the hallway's keys acted
    under it.
- **2026-10-02 — The room's field in the window (clause 28).** The
  rooms drew their fields on their own surfaces and noted no caret on
  the GUI's, so the window counted no field as having the keyboard:
  Cmd+V into a room's field (a Federation or Discovery ticket included)
  was dropped on a Mac, Ctrl+V elsewhere reached it as a key it ignores,
  and the input method stayed off. Each room's focused field now draws
  through the kit's `field_display`, which notes its caret on the room's
  surface and draws the surface's composition; `HostedRoom` hands the
  composition down before the draw and the caret up after it, and the
  host lifts the caret onto the GUI's surface. Deliberate, each:
  - **The hosted caret holds steady**, where the GUI's own fields blink:
    the GUI times its frames by its own surface's blink clock, not a
    room's, so a blinking room caret is a follow-up of its own.
  - **The log's level menu lays itself over the note**, as a GUI modal
    does, since it takes every key (clause 20) and a room's inline field
    (a filter, a ticket box) can still be focused beneath it.
  - **No composition reaches the room while a GUI modal is up**: the
    add-server form's own fields compose in themselves, and the room's
    field beneath would otherwise draw the same text a second time.
  - **The Discovery room's section line is measured in cells**, not
    characters, so a composition (or a translation) of wide characters
    in it is not cut at its end, and the filter's caret is noted once
    the line is placed.
- **2026-10-02 — Copy, download and highlight (clauses 29-34).** The
  log gained the ways out the first cut left out of scope. Deliberate,
  each:
  - **The download has a thread of its own**, with the log's client,
    rather than riding the poll's worker: a zip that takes a minute
    would otherwise stall the 2 s tail behind it.
  - **Over SSH a copy is OSC 52 alone, and the window never writes it**:
    a tool or the pasteboard on the far machine fills a clipboard
    nobody in front of the screen can paste from, and a window launched
    from a shell would write the escape into that shell.
  - **`o` shows the saved file rather than opening it**, as browsers do
    after a download: opening the zip on a Mac starts Archive Utility,
    which unpacks it into Downloads.
  - **The header carries copy and download** (clause 22), where the
    rooms keep their verbs on keys: the GUI is the pointer's surface and
    its footer of keys is off by default, so `y` and `d` alone would be
    invisible to someone with a mouse.
  - **"Highlight", never "selection"**: the kit's law that the pointer
    never highlights a row governs list cursors, which verbs act on.
    The log's highlight is text to copy, and no row verb reads it.
  - **The notes carry no line count**: a count would need plural forms
    in Russian and Polish for one sentence, and saying which lines went
    (the highlighted ones, or every one shown) is what the reader needs.
  - **Ctrl+Shift+C no longer quits in the window** off a Mac: it is the
    copy chord there, and the window takes it whether or not the log
    copies (clause 34). Plain Ctrl+C still quits.
  - **The rooms' `y` goes through the kit**: Discovery's and
    Federation's ticket copies wrote OSC 52 themselves, which a window
    never reads; they now take the same routes as the log's copy, and
    say "ticket copied" when the clipboard took it.
