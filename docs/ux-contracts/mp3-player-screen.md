# MP3 Player screen

| | |
|---|---|
| **Design of record** | This repo's own firmware page — `src/device/page.rs` over the worker in `src/device/flow.rs`, `mstream-player device flash`, built from the firmware-flash design set (mStream `docs/designs/firmware-flash`, alternate A): the step line, the card, the erase box, the time left, what to do next, the port watch, the log behind `l` — at commit `30e6b5b` (2026-10-04, the v0.6.0 pin), hosted whole. The GUI adds only the tab, the way back, and the rules that keep a write from being left and a board from being left in its bootloader. |
| **Server API** | None. No mStream session is involved: the board is a USB serial port, and the firmware is the release this player pins. |
| **Already in this repo** | The page, live as its own command on the admin hub's terminal session; the GUI's Stats tab, which showed how a hub page lives under the top bar (the stats-screen contract), and the hub's `HostedRoom`, whose per-frame call this screen uses. |
| **Status** | Implemented 2026-10-04 — this document first, then the page's hosted mode and the tab; revised the same day after review (Deviations log). It replaces the mStream tray launcher's planned "MP3 player" menu item, which was dropped the same day: the tab is the way in. |

## Intent

The MP3 player's firmware, one tab away from the music: the same page
`mstream-player device flash` opens, in the GUI the installers launch —
plug the Core2 in, open the tab, answer the one question — with no second
terminal and no flags to know.

## Entry points

1. The top bar's **MP3 Player** tab, fourth after Library, Stats and Admin,
   before the Visualizer item; the **Library** tab is the way back.
2. **`K`** from the Library, Now Playing, Stats and the Admin hallway;
   **`K`** on the tab back to the Library.
3. From the tab, `T` for Stats, `M` for Admin, a nav digit or `D` for that
   room of the Library, `0` for Now Playing, `V` for the visualizer — but
   never while the page writes (clause 9).

## States & flows

The page's own steps, unchanged: the firmware being found (downloaded on
first use, then kept in the cache), then the board — none (the port
watch), several (the pick), or one, reached and read — then the question,
the write (erase, compare, write, verify, restart), and Done or Failed.

- **Entering** the tab builds the page and starts its worker at once —
  unless the page the tab was last left with is still letting the board
  go: then the tab says "restarting the board…" and builds its page the
  frame after that one is done (clause 5).
- **Leaving** the tab tells the worker to let go, as Esc does, and lets
  the page go with it; a page whose worker holds the board, or is
  reaching it, is kept aside until the board is back in its firmware
  (clause 8) — all before the write; the write cannot be left (clause 9).
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
   15), the busy and note line on the page's last row. The page's steps,
   worker, keys, buttons and words are the command's.
2. **The GUI's queue panel, bar and note row stand down**, as on Stats.
   The top bar stays.
3. **No session is involved.** The tab works with no server saved; a
   server switch, a reconnect or a session that drops does nothing to the
   page, and the page is never rebuilt for one.
4. **It fits the GUI's floor.** At 100×24 the page has 22 rows — the
   footer is always drawn on this tab (clause 15) — and every step is
   drawn whole in every locale: Done, the tallest, with its Close button,
   even with the board's boot line in full. Where Done runs short of rows,
   its extra words give way and Close never does: the blank row over Close
   goes first, then the next-step block's last lines (a title left alone
   goes with them). With a pick's banner up the page has one row less: it
   draws what fits and stops above its last row — Done still keeps Close
   whole — and nothing it draws ever lands outside its area (no resize
   line, no spill onto the footer). Below the floor the GUI is the mini
   player; the page keeps running unseen and takes no key and no click
   there (clauses 12 and 14); while it writes, the mini player's line says
   so — that unplugging the board or quitting would leave it half written
   — in place of asking for room.

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
   nothing; the page is built once per visit. A visit that comes while the
   page last left still holds the board (clause 8) waits for it, saying
   "restarting the board…", and starts its worker once the port is free:
   a new worker never meets the old one's port in use.
6. **Every frame while the page exists**, the host does what the hub's
   loop does: the worker's reports are folded in, the port watch looks
   again every two seconds while no board (or several) is plugged in, a
   held button steps, the tooltip clock runs — and then the page is asked
   whether it has ended. A page kept aside (clause 8) has its reports
   read every frame too.
7. **The page's own end is the way back.** Esc at the page's base, Close,
   and Cancel at the question once the worker has restarted the board end
   the page; the screen then goes to the Library and lets the page go.
8. **Leaving the tab any other way lets the board go** — another tab,
   `K`, a nav digit, `D`, `0`, or any screen change from elsewhere. Before
   the write that is safe: the tab tells the worker to let go as Esc
   does. A board it holds is restarted into its firmware and its port
   freed at once, with no listen for a boot line; a worker not yet at the
   board stops before it opens the port; a port watch stops; a download
   finishes into the cache. The worker hears the word at the earliest
   safe point: before it opens a port, between the baud ladder's rungs
   (a rung that failed had reset the board, which is let go), and as soon
   as espflash has reached the board. It says it is reaching the board
   before its last look for the word, so the host knows from what it has
   heard whether the port may still be held: a page whose worker holds
   the board, or was reaching it, is kept aside and read every frame
   until the worker says the board is back (at most eight seconds), then
   dropped; any other page is dropped at once. No board is left in its
   bootloader by leaving, and no visit meets its own port in use.
9. **The write cannot be left.** From the moment the question is answered
   until Done or Failed — the erase, the compare, the write, the check and
   the restart — the tab holds the GUI:
   - every key is the page's, and only `l` (the log) does anything: `q`,
     `K`, `T`, `M`, `0`, the digits, `D`, `V`, Tab, Esc and the transport
     letters do nothing;
   - the top bar's clicks do nothing — the tabs, the Visualizer item, the
     server label and `[+]` — and its tabs do not light under the pointer;
     below the floor the mini player's buttons answer nothing either;
   - no screen change from anywhere is honoured, and the page is never
     dropped.
10. **Ctrl+C, the window's close button and, on macOS, Cmd-Q and the app
    menu's (or the Dock's) Quit still quit**, as on every GUI screen and
    as in the standalone page: they are the ways to cut a write short, and
    they leave the board half written. The write's hint, on the footer,
    warns against unplugging the board; the mini player's line, while the
    write runs out of sight, names quitting too (clause 4).
11. **Quitting with the board held lets it go first.** When the player
    quits (any way: `q`, Ctrl+C, the window's close button, Cmd-Q) while
    a page — the tab's, or one kept aside — holds the board before the
    write or is reaching it, the GUI tells its worker to let go and waits,
    reading its reports, until the board is back in its firmware, so a
    quit does not leave the Core2 dark in its bootloader. At the question
    that is the reset, well under a second; while espflash is still
    reaching the board it is the rest of that connect — up to one rung of
    the baud ladder — and the reset. The wait is bounded at eight seconds,
    and in the terminal a wait past a second says why on stderr
    (`dev.letting_go`). With no page, a page that holds no board, or a
    write under way, the quit does not wait.

### Keys and the pointer

12. **Keys**, outside a write: the page's own keys are the page's (Enter,
    `e`, `l`, `r`, `↑` `↓`, Esc). The GUI keeps `q` (quit), `K` (back to
    the Library), `T`, `M`, `0`, `V`, the digits and `D`, as on the Stats
    tab. Esc goes to the page: at the question it cancels — the board is
    restarted, then the Library — and elsewhere it is the way back.
    Entering the tab takes the keys back from the Library: the queue
    panel's focus stows, and an armed Sonic Path or Auto DJ pick is let go,
    home to the room that asked, as its banner's [X] does — a pick or a
    queue left holding Enter and Esc would name keys that start a write
    instead. On the tab only a GUI modal outranks the page, and not during
    a write. The page's keys reach it only where it was drawn last frame:
    below the floor, in the mini player, none do.
13. **`K` is the tab's key**: from the Library, Now Playing, Stats and the
    Admin hallway it opens the tab, and on the tab it leads back to the
    Library. Its lowercase is no key of the GUI's, so no slip of Shift or
    Caps Lock opens the tab — and opening it resets a plugged-in Core2
    (clause 5). The base footer line does not name it (every locale's line
    is full; the top bar does).
14. **The pointer** inside the page's area is the page's: hover, click,
    hold and the wheel on its own controls, the hub's way. The top bar's
    row and the banner's row are the GUI's, but for the drag and the
    release of a press that began on the page. A GUI modal or the server
    menu owns the pointer while it is open, as elsewhere (outside a write,
    clause 9). The wheel scrolls the page's log or its pick list, and
    never the hidden Library room or queue. Below the floor the page
    takes no click.
15. **The footer** is always drawn on this tab, whatever the key-hints
    setting — the page's hint is part of the page, the write's warning not
    to unplug the board above all — and the page's area ends above it. It
    shows the page's hint, in its hosted words: where the standalone page
    says "Esc leave" or "Esc close", the tab says "Esc library", since that
    is where Esc goes. It outranks the queue panel's and an armed pick's
    tips; only a GUI modal's own come first, and none during a write.
    While a visit waits for the last page to let the board go, the footer
    is empty.
16. **The strip**: Library, Stats, Admin, MP3 Player, then the Visualizer
    item. The session's server label at the right never covers it: the
    label starts at least a blank cell past the Visualizer item, cut short
    with the kit's mark (its menu mark kept) where it would not, so the
    tabs keep every cell and every click. At the GUI's 100 columns a local
    server's label, `http://localhost:3000 ▾`, stands whole in every
    locale; a quick-connect session's or a LAN address's may be cut in the
    longer ones.
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
| `dev.hint_working_unseen` | writing the MP3 player's firmware — please wait: unplugging it or quitting now would leave it half written |
| `dev.letting_go` | restarting the Core2 into its firmware before quitting… |

The question's, the write's and Done's hints are the page's own
(`dev.hint_confirm`, `dev.hint_working`, `dev.hint_done`): they say what
their keys do on the tab as they are. `dev.hint_working_unseen` is the
mini player's line while the page writes out of sight (it wraps, so it has
no footer's width to keep); `dev.letting_go` is the quit's stderr line
(clause 11). The "restarting the board…" a visit shows while it waits is
the page's own `dev.phase_restarting`.

## Out of scope here

- **The command's flags.** `--firmware`, `--release`, `--port`, `--erase`,
  `--no-erase` and `--yes` stay the command line's: the tab always writes
  the pinned release, finds the port itself and lets the board decide the
  erase. A Core2 that keeps restarting on the pinned QIO image (v0.8.0's)
  still needs `--firmware` with the release's `-dio-full.bin`.
- **The tray launcher's item.** mStream's launcher gets no "MP3 player"
  entry; the GUI's tab is the way in.
- **A page that outlives the tab.** Leaving lets the page go (clause 8):
  it is kept only as long as its worker takes to let the board go, never
  shown again; the next visit starts over, from the cache.
- **A quit that waits during a write.** The write's end cannot be waited
  for on the way out (half a minute, and the user asked to quit): clause
  10 says what a quit then does.

## Translation notes

- The page grew a hosted mode the way the stats page did: `render_hosted`
  draws no header and no tips row and leaves the area's first row blank;
  the log reserves one bottom row instead of two; everything the page
  draws stops above its last row, so a short area cuts the page instead of
  spilling it, and Done gives its extra words way before Close (clause 4).
  The standalone page draws as before — it has no floor, so Done is drawn
  whole — but for the logs toggle, whose rect is now measured in cells: in
  Japanese and Chinese, '▸ ログを表示' and '▸ 显示日志' are drawn whole,
  with a click target to match.
- The GUI keeps the concrete page (`src/gui/device.rs`) and drives it
  through the hub's `HostedRoom::after_frame` — pump, tick, the held
  button, the dwell — then `Screen::finished`, which the Stats glue never
  needed: the device page ends only through `finished`.
- The predicates the host leans on are the page's: *writing* (the write's
  step, from Go to Done or Failed) and *holds the board* — the worker's
  word, not the page's step, which lags it: the worker has said it is
  reaching the board and not yet that the board is let go (Cancelled,
  Done, Failed) or ended. `Page::release` is the host's one way to let a
  page go — Esc's way, then the worker's reports read again, so *holds
  the board* is then the truth.
- The worker (`flow.rs`) hears Quit, or the page's end of its channel
  dropped, before it opens a port, between the baud ladder's rungs and as
  soon as the board is reached; it tells "reaching" before that last look,
  which is what lets a host drop a page that never heard it. Every run
  that ends without a write lets the board go through `Link::let_go` (a
  reset and the port freed, no listen), or `Engine::let_go` for a board a
  failed rung reset; only the write's own restart listens for the boot
  line. This is the command's too: Esc at the question ends at once now,
  where it waited out the six-second listen.
- Under test the tab builds its page on channels the test holds both ends
  of: no worker is ever spawned, so no firmware is downloaded and no
  serial port is opened by `cargo test` (a Core2 may well be plugged in).
  The worker's own tests run it on the fake engine, whose `open` can take
  a while to answer as espflash's connect does, and whose trace says what
  was done to the board — a local image, no port.
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
- **2026-10-04 — Revised after review.** Leaving dropped the page and
  not its worker: a quick return met COM3 "in use by another program"
  (the player itself), and a quit while the board was being reached
  found no page to wait for and left the Core2 dark. Leaving now tells
  the worker to let go and keeps the page aside until it has (clause 8),
  a visit waits for it (clause 5), and the quit waits for both, its bound
  raised from two seconds to eight to cover the rest of a connect
  (clause 11). The worker hears the word early and lets the board go with
  no boot-line listen — which also ends the command's Esc at the question
  at once. The footer was drawn only with key hints on (the GUI's
  default is off), so the write's warning not to unplug was hidden: the
  tab now always draws it (clause 15), ahead of the queue's and a pick's
  tips, and entering the tab lets the queue's focus and an armed pick go
  (clause 12). The mini player passed keys to the page it hid — Enter
  could start an unseen write — and its buttons answered during a write:
  neither now (clauses 4, 9, 12, 14). Done lost its Close at the floor in
  German, Spanish and French with a real boot line: its extra words now
  give way (clause 4). The fourth tab pushed the strip under a quick-
  connect or LAN server label in the long locales at the window's 100
  columns, the label taking the tabs' clicks: the label is now cut to
  keep off the strip (clause 16). The tab's key moved from `F` to `K`:
  lowercase `f` is the browse bar's filter key, so Shift or Caps Lock
  turned "filter" into opening the tab, which resets a plugged-in Core2;
  `K` and `k` are bound nowhere in the GUI. Clause 10 named only Ctrl+C
  and the close button, and said the write's hint warned against
  quitting; it now names Cmd-Q and the app menu's Quit, and says the hint
  warns against unplugging.
