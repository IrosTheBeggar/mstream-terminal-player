# MP3 Player screen

| | |
|---|---|
| **Design of record** | This repo's own firmware page — `src/device/page.rs` over the worker in `src/device/flow.rs`, `mstream-player device flash`, built from the firmware-flash design set (mStream `docs/designs/firmware-flash`, alternate A): the step line, the card, the erase box, the time left, what to do next, the port watch, the log behind `l` — at commit `30e6b5b` (2026-10-04, the v0.6.0 pin), hosted whole. The GUI adds only the tab, the way back, and the rules that keep a write from being left. |
| **Server API** | None. No mStream session is involved: the board is a USB serial port, and the firmware is the release this player pins. |
| **Already in this repo** | The page, live as its own command on the admin hub's terminal session; the GUI's Stats tab, which showed how a hub page lives under the top bar (the stats-screen contract), and the hub's `HostedRoom`, whose per-frame call this screen uses. |
| **Status** | Implemented 2026-10-04 — this document first, then the page's hosted mode and the tab. It replaces the mStream tray launcher's planned "MP3 player" menu item, which was dropped the same day: the tab is the way in. |

## Intent

The MP3 player's firmware, one tab away from the music: the same page
`mstream-player device flash` opens, in the GUI the installers launch —
plug the Core2 in, open the tab, answer the one question — with no second
terminal and no flags to know.

## Entry points

1. The top bar's **MP3 Player** tab, fourth after Library, Stats and Admin,
   before the Visualizer item; the **Library** tab is the way back.
2. **`F`** from the Library, Now Playing, Stats and the Admin hallway;
   **`F`** on the tab back to the Library.
3. From the tab, `T` for Stats, `M` for Admin, a nav digit or `D` for that
   room of the Library, `0` for Now Playing, `V` for the visualizer — but
   never while the page writes (clause 9).

## States & flows

The page's own steps, unchanged: the firmware being found (downloaded on
first use, then kept in the cache), then the board — none (the port
watch), several (the pick), or one, reached and read — then the question,
the write (erase, compare, write, verify, restart), and Done or Failed.

- **Entering** the tab builds the page and starts its worker at once.
- **Leaving** the tab drops the page and its worker with it — before the
  write; the write cannot be left (clause 9).
- **The page's own end** (Esc at its base, Close, Cancel once the board is
  back in its firmware) is the way back to the Library.
- **Coming back** builds a fresh page: the firmware comes from the cache,
  the board is read again.
- **The session** has nothing to do with the screen: with no server
  configured the tab works the same, and a server switch or a reconnect
  leaves the page alone.

## Behavior contract

### The screen

1. **It is the firmware page, whole** — `mstream-player device flash` with
   no flags: the firmware this player pins, the port found by itself, the
   erase the board decides. It is drawn under the top bar in the rows the
   page spends on its own header: its first row blank under the bar, no
   header of its own (the top bar is the screen's heading; the firmware's
   version and origin are on the card's "To install" row and in the log),
   no tips row of its own (the GUI's footer carries the page's hint, clause
   15), the busy and note line on the page's last row. Nothing of the
   page's behaviour changes: the same steps, worker, keys, buttons and
   words.
2. **The GUI's queue panel, bar and note row stand down**, as on Stats.
   The top bar stays.
3. **No session is involved.** The tab works with no server saved; a
   server switch, a reconnect or a session that drops does nothing to the
   page, and the page is never rebuilt for one.
4. **It fits the GUI's floor.** At 100×24 with the footer on, the page has
   22 rows and every step is drawn whole — Done, the tallest, with its
   Close button. With a pick's banner up the page has one row less: it
   draws what fits and stops above its last row, and nothing it draws ever
   lands outside its area (no resize line, no spill onto the footer). Below
   the floor the GUI is the mini player; the page keeps running unseen.

### The board

5. **Entering the tab starts the worker**, exactly as the command does:
   it resolves the firmware first (a download the first time, the cache
   after), so a failed download never touches a board; then, with exactly
   one Core2-shaped port plugged in, it opens that port — which resets the
   board into its bootloader — and reads what is on it, before anything is
   asked. One reset serves both the read and the write (PLAN.md, Phase
   13). **So opening the tab with a Core2 plugged in stops the player on
   it**: the board sits in its bootloader, its screen dark, until the
   question is answered or left. Selecting the tab while it is up does
   nothing; the page is built once per visit.
6. **Every frame while the page exists**, the host does what the hub's
   loop does: the worker's reports are folded in, the port watch looks
   again every two seconds while no board (or several) is plugged in, a
   held button steps, the tooltip clock runs — and then the page is asked
   whether it has ended.
7. **The page's own end is the way back.** Esc at the page's base, Close,
   and Cancel at the question once the worker has restarted the board end
   the page; the screen then goes to the Library and drops the page.
8. **Leaving the tab any other way drops the page** — another tab, `F`, a
   nav digit, `D`, `0`, or any screen change from elsewhere. Before the
   write that is safe: the worker restarts a board it holds when the
   page's channel closes, stops a port watch, and lets a download finish
   into the cache; no board is left in its bootloader by leaving.
9. **The write cannot be left.** From the moment the question is answered
   until Done or Failed — the erase, the compare, the write, the check and
   the restart — the tab holds the GUI:
   - every key is the page's, and only `l` (the log) does anything: `q`,
     `F`, `T`, `M`, `0`, the digits, `D`, `V`, Tab, Esc and the transport
     letters do nothing;
   - the top bar's clicks do nothing — the tabs, the Visualizer item, the
     server label and `[+]` — and its tabs do not light under the pointer;
   - no screen change from anywhere is honoured, and the page is never
     dropped.
10. **Ctrl+C and the window's close button still quit**, as on every GUI
    screen and as in the standalone page: the one way to cut a write
    short, and it leaves the board half written (the page's hint says so
    while it writes).
11. **Quitting with the board held lets it go first.** When the player
    quits (any way: `q`, Ctrl+C, the window's close button) while the page
    holds the board before the write — the board being reached, the
    question, the board being restarted after a cancel — the GUI tells
    the worker to let go and waits up to two seconds, reading its
    reports, for the restart, so a quit does not leave the Core2 dark in
    its bootloader. With no page, or a page that holds no board, or a
    write under way, the quit does not wait.

### Keys and the pointer

12. **Keys**, outside a write: the page's own keys are the page's (Enter,
    `e`, `l`, `r`, `↑` `↓`, Esc). The GUI keeps `q` (quit), `F` (back to
    the Library), `T`, `M`, `0`, `V`, the digits and `D`, as on the Stats
    tab. Esc goes to the page: at the question it cancels — the board is
    restarted, then the Library — and elsewhere it is the way back.
13. **`F` is the tab's key**: from the Library, Now Playing, Stats and the
    Admin hallway it opens the tab, and on the tab it leads back to the
    Library. The base footer line does not name it (every locale's line is
    full; the top bar does).
14. **The pointer** inside the page's area is the page's: hover, click,
    hold and the wheel on its own controls, the hub's way. The top bar's
    row and the banner's row are the GUI's, but for the drag and the
    release of a press that began on the page. A GUI modal or the server
    menu owns the pointer while it is open, as elsewhere (outside a write,
    clause 9). The wheel scrolls the page's log or its pick list, and
    never the hidden Library room or queue.
15. **The footer** shows the page's hint, in its hosted words: where the
    standalone page says "Esc leave" or "Esc close", the tab says "Esc
    library", since that is where Esc goes.
16. **The strip**: Library, Stats, Admin, MP3 Player, then the Visualizer
    item. At the GUI's 100 columns the strip ends with room to spare for
    the session's server label at the right — a local server's address
    with its menu mark, `http://localhost:3000 ▾`, and a blank cell either
    side — in every locale; a translation that would not leave it is
    shortened.
17. **The tab wears the slab** while the screen is up, as the other tabs
    do.

## Wording

The page's own strings (`dev.*`), unchanged. New:

| Key | English |
|---|---|
| `gui.top.mp3` | MP3 Player |
| `dev.hint_wait_hosted` | l logs · Esc library |
| `dev.hint_none_hosted` | r look again · l logs · Esc library |
| `dev.hint_several_hosted` | ↑ ↓ pick · Enter choose · l logs · Esc library |
| `dev.hint_failed_hosted` | r try again · l logs · Esc library |

The question's, the write's and Done's hints are the page's own
(`dev.hint_confirm`, `dev.hint_working`, `dev.hint_done`): they say what
their keys do on the tab as they are.

## Out of scope here

- **The command's flags.** `--firmware`, `--release`, `--port`, `--erase`,
  `--no-erase` and `--yes` stay the command line's: the tab always writes
  the pinned release, finds the port itself and lets the board decide the
  erase. A Core2 that keeps restarting on v0.6.0's QIO image still needs
  `--firmware` with the release's `-dio-full.bin`.
- **The tray launcher's item.** mStream's launcher gets no "MP3 player"
  entry; the GUI's tab is the way in.
- **A page that outlives the tab.** Leaving drops the page (clause 8); the
  next visit starts over, from the cache.

## Translation notes

- The page grew a hosted mode the way the stats page did: `render_hosted`
  draws no header and no tips row and leaves the area's first row blank;
  the log reserves one bottom row instead of two; everything the page
  draws stops above its last row, so a short area cuts the page instead of
  spilling it. The standalone page draws exactly as before.
- The GUI keeps the concrete page (`src/gui/device.rs`) and drives it
  through the hub's `HostedRoom::after_frame` — pump, tick, the held
  button, the dwell — then `Screen::finished`, which the Stats glue never
  needed: the device page ends only through `finished`.
- The two predicates the host leans on are the page's: *writing* (the
  write's step, from Go to Done or Failed) and *holds the board* (being
  reached, the question, the restart after a cancel).
- Under test the tab builds its page on channels the test holds both ends
  of: no worker is ever spawned, so no firmware is downloaded and no
  serial port is opened by `cargo test` (a Core2 may well be plugged in).
- Machine translations for the nine non-English locales, as everywhere;
  key names are never translated.

## Deviations log

- **2026-10-04 — Extracted and implemented.** Asked for as a fourth tab
  that loads the flash page, in place of the tray launcher's menu item.
  Two alternatives were weighed and refused. Keeping the page alive
  between visits would keep a board in its bootloader behind a tab nobody
  is looking at; a Start button before the worker runs would part the tab
  from the command, whose probe-on-sight and single reset are the design
  of record. Dropping the page on leaving, refusing to leave a write, and
  letting the board go before a quit cover what hosting adds. The
  header's right edge ("firmware v0.6.0 · release") is not drawn hosted —
  the card and the log carry it — since a hosted page's first row is the
  host's.
