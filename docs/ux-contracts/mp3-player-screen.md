# MP3 Player screen

| | |
|---|---|
| **Design of record** | mStream `docs/designs/mp3-tab`, alternate **A · One card** (card 02) and its several-boards frames (card 05), with the grafts the set's README recommends — chosen by Paul on 2026-10-09 ("Go with alternate A, build it"); and for the Advanced options, alternate **A · An options sheet** (card 07) with the grafts of cards 08 and 09 the set's README recommends — chosen by Paul on 2026-10-10 ("Go with alternate A, build it"). This repo's page draws it: `src/device/page.rs` over the desk worker (`src/device/desk.rs`, `board.rs`, `listen.rs`), the same page `mstream-player device flash` opens and the GUI's MP3 Player tab hosts whole. The GUI adds only the tab, the way back, and the rules that keep a write from being left and a board from being left in its bootloader. |
| **Design of record, the SD card transfer** | **The card-transfer design set (twelfth set), alternates per the user's picks of 2026-10-19.** **Provisional, with no commit pinned:** it is in no repo yet and will live in mStream `docs/designs/card-transfer`; its commit is pinned here once it lands there, in a commit of its own touching this header (the README's re-extraction rule). Until then the citations below ("card 07 §7, choice 6" and the like) can't be followed from a repo. It draws the placement chosen on 2026-10-18 (Q2, one tab row per subject) whole in its card 02, with the Card view's states (card 03), the sync (card 04), the card's Settings (card 05, alternate **A**, two tabs), the gates and formatting in the Core2 (card 06, the player's second step as alternate **B**, the hold), and play history, sets and the command line (card 07, whose section 7 holds the twelve choices and the picks). This repo's page is to draw it beside the board card (`src/device/page/card.rs`), over a transfer worker that lives at the GUI level (`src/device/card/worker.rs`; part B4). |
| **Server API** | **For a board, none.** No mStream session is involved: each board is a USB serial port, asked over it with no reset (the firmware repo's `docs/HOST-STATUS.md`: `@status` with `cid=` and `clock=`, `@count`, `@identify`, `@time`, `@format`, and the console's `L` for firmware older than them), and the firmware written is the release this player pins — or, for one write, what the Advanced options sheet chose: the pin's other build, another release (GitHub's releases API, asked only when the sheet lists them), or a local build. **For a card in a reader, its own server**: the one the card names (its URL key and nickname in `/.mstream/state.json`), reached through this computer's saved sign-in for it, never necessarily the GUI's session; a peer's library through its parent (the multi-server contract). The calls: `GET /api/` (the server's `instanceId`, mStream B11, and `features.federationProxy`, whether its peer stream proxy forwards the copy header); `POST /api/v1/sync/manifest` (paged, ETag/304) and `POST /api/v1/db/metadata/batch`; `GET /media/…` with Range, and `GET /transcode/…`; `/album-art/…` and `discovery/local/embeddings`; `POST /api/v1/stats/plays`; for peers, `federation/peers`, `…/peers/:id/access` and `file-explorer/recursive`, and the key's limits where the server offers them (mStream B10); beside the server on this computer, `admin/directories` and `admin/transcode`, to read its folders from disk (part A9). |
| **Already in this repo** | The page, live as its own command on the admin hub's terminal session; the GUI's Stats tab, which showed how a hub page lives under the top bar (the stats-screen contract), and the hub's `HostedRoom`, whose per-frame call this screen uses. The kit's gold warning modal (the libraries room's remove gate), its tab row (the Discovery room), its scan widget and its tall buttons. For the SD card transfer: the card contract's kit, names, tag reader and Auto DJ writer (`src/device/card/{contract,names.rs,tags.rs,autodj.rs}`); cards in readers held to the drive-safety rule (`card/volume/`); set-up, the card's state file and this computer's `cards.json` (`setup.rs`, `state.rs`, `cards.rs`); the sync engine with its plan, covers, dry run, the peers' budget and sets (`engine/`, `sync.rs`, `plan.rs`, `covers.rs`, `dry.rs`, `budget.rs`, `span.rs`); the play log read and posted (`history.rs`); the transfer client (`src/api/transfer.rs`); `cid=`, `clock=` and `@time` in `listen.rs`; the hidden commands that drive them for the real-card and board checks (`device card-probe`, `card-setup`, `card-sync`, `card-set`, `card-plays`, `time`). The play outbox and `report_plays_async` (the play-reporting contract), and `session_reach`/`reach_client` (`src/gui/stats.rs`). |
| **Status** | Implemented 2026-10-04 (the tenth set's alternate A: the step line, the question, the erase box); **redrawn as alternate A of the eleventh set 2026-10-10** — this document first, then the page, the tab and the command line (Deviations log); **Advanced options (card 07) 2026-10-10** — the worker and the command line first, then this document's clauses 33–43, then the sheet; **the SD card transfer (the twelfth set) 2026-10-19** — this document's clauses 3a, 20a, 25a and 44–67 with the user's picks (Deviations log); the card's engine, set-up, sync, sets, peers and plays are built behind hidden commands (parts A2–A9, A12, A13, B1, B2, B7, B9, B15); the transfer worker, the Card view and the public commands (B4), the Format route with `device format` (B6), and the tab's half of the clock (clause 5, B13), are built (Deviations log). |

## Intent

The MP3 player, one tab away from the music: plug the Core2 in, open the
tab, and see at once whether its firmware is up to date and what its SD
card holds — without the board noticing. Every write passes one gold gate
first; everything technical waits behind one Details door, and every
choice about what is written — the flash mode, another release, a local
build — behind one *Advanced…* sheet. The same page is `mstream-player
device flash`, so a terminal gets it too.

The tab also holds the SD card itself, when it sits in this computer's
card reader. Put the card in and see at once what it holds, how full it
is, when it was last synced and what the next sync would do. One key fills
it from your mStream server and keeps it in step. Every change that
removes music or rewrites the card passes a gold gate first; a format is
started here and confirmed on the player. The sync belongs to the whole
app, not to the tab, so leaving the tab or listening to music meanwhile
never stops it. The same is `mstream-player device card …`, so a
terminal gets it too.

## Entry points

1. The top bar's **MP3 Player** tab, fourth after Library, Stats and Admin,
   before the Visualizer item; the **Library** tab is the way back.
2. **`K`** from the Library, Now Playing, Stats and the Admin hallway;
   **`K`** on the tab back to the Library.
3. From the tab, `T` for Stats, `M` for Admin, a nav digit or `D` for that
   room of the Library, `0` for Now Playing, `V` for the visualizer — but
   never while the page writes (clause 9).
4. **`mstream-player device flash`**, on its own (clause 31), and its line
   mode `--yes` (clause 32); **`device list`** and **`device format`**
   (clauses 32, 66).
5. **`mstream-player device cards`** and **`device card <drive>
   status|setup|sync|plays|eject [--yes]`**, the cards' line mode (clauses
   64–67).

## States & flows

The page is one card about the board on view (clause 18): its name, its
firmware's verdict, its SD card, one action. Under it the worker watches
the ports and asks every board over USB with no reset; the firmware image
comes from the cache, or a download, beside them.

- **Entering** the tab builds the page and starts its worker, which only
  listens (clause 5) — unless the page the tab was last left with is still
  letting its ports go: then the tab says so and builds its page the frame
  after that one is done (clause 8).
- **At rest** the card shows the verdict and the card; a board that needs
  something wears one primary (Update, Install, Read the board, Try again);
  one that does not wears none.
- **A choice** is the Advanced options sheet's (clauses 33–36): Apply
  leaves one *Next write* line on the board's card (clause 38), and the
  primary and the gate follow it; the write, Reset or leaving ends it.
- **A write** is a gate's yes away (clause 23): from it until the board is
  back — the reset, the compare, the write, the check, the restart — the
  tab cannot be left (clause 9), and the card stays the subject: its
  Firmware row becomes the write's progress, its SD card row says the card
  is untouched. **Done is the card again** (clause 24).
- **Leaving** the tab tells the worker to let go, as Esc does; a page whose
  worker may still have a port is kept aside until it has (clause 8).
- **Coming back** builds a fresh page: every board is asked again, the
  image comes from the cache.
- **The session** has nothing to do with a board: with no server
  configured the board side works the same, and a server switch or a
  reconnect leaves it alone. **A card uses its own server** (clause 3a):
  it reads the same whichever server the Library is on, and a card whose
  server isn't saved here says so.
- **A card in a reader** is a subject like a board. It has a tab on the
  same row (clause 25) and its own view under it, the **Card view** (clause
  44). Its tab appears when the computer mounts it, with nothing written,
  and goes when the card is taken out, except during its sync (clause 53).
- **A sync** runs on the transfer worker, which the GUI owns (clause 55).
  It starts from the Card view's Sync, after any gate the plan needs
  (clause 58). The Card view follows it; leaving the tab, another subject
  or another screen never stops it; Done is the Card view again (clause
  54).
- **Formatting** is the board's. A card the player can't read is offered
  Format on the board's SD row, and a card it reads in Details (clause 59).
  The tab asks; the player confirms on its own screen and formats; the PC
  never writes the card's file system.

## Behavior contract

### The screen

1. **It is the firmware page, whole, and the cards' view beside it** —
   `mstream-player device flash` with no flags (the firmware this player
   pins, every Core2-shaped port watched, the erase decided in the gate),
   plus every card in this computer's readers (clauses 44–63). It is
   drawn under the top bar in the rows the page spends on its own header:
   its first row blank under the bar, no header of its own (the top bar is
   the screen's heading; the firmware's version and origin are in
   Details), no tips row of its own (the GUI's footer carries the page's
   hint, clause 15), the busy and note line on the page's last row. The
   page's worker, keys, buttons and words are the command's; the cards'
   are the GUI's alone (clause 31).
2. **The GUI's queue panel, bar and note row stand down**, as on Stats.
   The top bar stays.
3. **No session is involved for a board.** The board side works with no
   server saved. A server switch, a reconnect or a session that drops does
   nothing to a board's card, and the page is never rebuilt for one.

3a. **A card uses its own server.** A card in a reader names the server
   that filled it: a URL key and a nickname in its `state.json`, never an
   address, token or username.
   - The Card view reaches that server through this computer's saved
     sign-in for the saved server with that key, whatever the GUI's
     session is on. A peer's rows go through its parent.
   - No saved server has that key: the card says so and offers to add one
     (clause 45).
   - A saved sign-in that no longer works: the card offers to sign in
     again, to that server only, and the Library stays where it is.
   - A new card takes the session's server, changeable in its Settings
     (clause 48).
   - With no server saved on this computer at all, set-up still works
     (clause 50): it writes the structure, the first commit and the name,
     and leaves the card's server empty. The Card view then says "No server
     saved on this computer yet" with *Add the server ▸* (the GUI's
     add-server form, `[+]`), and the card takes the first server saved,
     changeable in Settings. Nothing is synced until it has one.
   - A server switch in the GUI changes nothing on a card's view or its
     sync.

4. **It fits the GUI's floor.** At 100×24 the page has 22 rows — the
   footer is always drawn on this tab (clause 15) — and in every locale the
   tallest card (an action, Details closed, the port tabs and the identity
   line of several boards above and in it) is drawn whole with its primary,
   and every gate (update, install, write again, go back, Update all with
   six boards listed) with both of its buttons. Details opens in the rows
   that are left and its log windows to them; a gate short of rows drops its
   blank lines first. Nothing the page draws lands outside its area (no
   resize line, no spill onto the footer); with a pick's banner up the page
   has one row less and cuts itself inside it. Below the floor the GUI is
   the mini player; the page keeps running unseen and takes no key and no
   click there (clauses 12 and 14); while it writes, the mini player's line
   says so — that unplugging the board or quitting would leave it half
   written — in place of asking for room.

   With cards, every Card view at rest is still drawn whole at 100×24 in
   every locale, its primary included. With the tab row the card has 16
   rows inside its border, and the tallest states take all 16 in German: a
   card with no `device.txt` yet (its assumed player wraps to two lines)
   and a card whose server isn't saved here (the set's card 03 §8, which
   draws them with the third tallest, new with music of its own, 15). A
   set's card takes 15, the board card with its card line (clause 20a) 15,
   the ready card 14. The tallest run state, Done with skipped files and a
   refused source, takes 14 (card 04).

   Should a locale run longer, the Card view gives way in this order: its
   dim explanation lines, the last first; then the Converts row; never the
   head, Holds, Space or the bottom row with its primary. The board card's
   card line (clause 20a) gives way before any of the board's own rows. The
   card's Settings sheet fits the floor whole with a one-line fit (clause
   49); a longer fit line gives way the help first, then blank lines. Every
   gate fits with its buttons, its blank lines going first.

### The boards

5. **Opening the tab only listens.** The worker lists the ports (nothing
   is opened to list them) and asks each Core2-shaped board what it runs
   over its USB serial port opened with DTR and RTS held low, which does not
   reset it: its screen stays on and its music plays on. A board running
   firmware with the status query (`@status`, firmware 0.9.0) says its
   version and its SD card; an older mStream firmware answers `@err 7
   status`, and the console's `L`, sent on its own, gives its version; a
   board that says nothing within about a second is *not answering*. Unless
   it showed itself ours: the firmware serves its console only from its main
   loop, and lists the card's library before that loop runs — about 20 s
   for 20,000 tracks on v0.8.0 and later after another version used the
   card, 90 s at every start on v0.7.0 — while its other tasks print. A
   board that printed the firmware's boot line, or two lines or more in its
   own log's shape (`[bt] reconnect: …`: a lowercase tag in brackets, a
   space, words — never the Arduino core's `[ 11267][W][…]`; one rule,
   `listen::sign`), is *starting up* (clause 19): it is asked again every
   5 s on the port it holds, for 3 minutes at most from the first sign
   (`listen::STARTING_UP`), and only then called not answering. A board
   that prints lines but none of ours is listened to for 8 s before it is
   called not answering; one that prints nothing, after the first second,
   as before. The
   bootloader is reached only after a gate's yes (clause 23), or by **Read
   the board** on a board that does not answer (clause 22). Selecting the
   tab while it is up does nothing; the page is built once per visit. A
   visit that comes while the page last left still has a port (clause 8)
   waits for it — the tab says "letting the USB ports go…", or "restarting
   the board…" when a read was under way — and starts its worker once
   every port is free: a new worker never meets the old one's port in use.

   **The one write a listen makes is the clock** (the firmware repo's
   `docs/HOST-STATUS.md`, "@time", as at its commits 09874f5 and 79a7438).
   `@status` says only whether the board has a time (`clock=ok` or
   `clock=unset`); the time itself is asked by a bare `@time`, answered
   `@time <unix-ms>` or `@time unset`, at once and changing nothing.
   - A board whose `@status` has a `clock=` key is asked a bare `@time`
     once per listen, after its status, on the port the listen holds; a
     clock set is not read back. This computer's time is taken halfway
     between the question and its answer.
   - When the answer is `@time unset`, or a time more than 2 s from this
     computer's (`listen::CLOCK_SLACK`: a restart alone leaves the board's
     clock within 1.5 s of the time set), it is sent one `@time <unix-ms>`
     (this computer's `SystemTime::now()`, in ms since 1970, UTC), and the
     firmware answers `@time ok`. The log says "the player's clock was
     unset: set it (@time)" or "the player's clock was 3.2 s off: set it
     (@time)", and Details' *Clock* fact says when (clause 61).
   - This computer's own clock outside 2026–2099 (the range the board
     takes) sends nothing; `@err 5 time`, and `@err 4 time rtc` (the
     board's clock chip didn't take it), leave the board as it was. The
     log says so once, in gold, and the listen goes on; the next listen
     tries again. So does a board that answers no `@time` within the
     reply's wait ("the player didn't answer @time"), and any other
     answer, word for word.
   - The page's listens do this, the tab's and `device flash`'s alike;
     `device flash --yes`'s lines, `device list` and `device format` only
     listen. The hidden `device time` reads and sets one board's clock the
     same way, and reads it once more after a set (its `after`), for the
     device checks.
   - A listen told to stop (the tab left, a write asked for) sends no
     `@time <unix-ms>`. A set the board answered `@time ok` before the
     stop is still said and kept for Details, and a refusal it gave is
     still said; only a silence the stop cut short, or a port that failed
     meanwhile, is said by none.
   - A board whose `@status` has no `clock=` (before 0.12.0) is never
     asked: absence is how the page tells, never the version. Should one
     answer `@err 7 time` anyway, it is left as it is.
   - Nothing else is written and no reset follows. Neither form of `@time`
     is sent during a write, a read, a count or a format.

   **The readers are watched beside the ports, every 2 s**, by the
   transfer worker (clause 55). A card the computer has mounted is read
   (its root, `state.json`, `device.txt`, the play log) with nothing
   written; its server is then asked once what changed (one manifest
   request, 304 when unchanged).
6. **Every frame while the page exists**, the host does what the hub's
   loop does: the worker's reports are folded in (the worker watches the
   ports every two seconds by itself), a held button steps, the tooltip
   clock runs, a note past its time goes — and then the page is asked
   whether it has ended. A page kept aside (clause 8) has its reports read
   every frame too.
7. **The page's own end is the way back.** Esc — outside a gate and a
   write — tells the worker to let go and ends the page once it says every
   port is free: at once for a page that never saw a board, a moment later
   for listens (they let their ports go at once), after the board's restart
   for a read under way. The screen then goes to the Library and lets the
   page go. A sync is not the page's: Esc during a sync goes back to the
   Library and the sync goes on (clause 55).
8. **Leaving the tab any other way lets the boards go** — another tab,
   `K`, a nav digit, `D`, `0`, or any screen change from elsewhere. Before
   or after a write that is safe: the tab tells the worker to let go as Esc
   does. Every listen lets its port go at once; a count or Show on the
   player stops (the count runs on on the board, which is harmless); a read
   finishes and restarts the board it reset; the download finishes into the
   cache. A page that has seen a board is kept aside and read every frame
   until the worker says every port is free (`Released`) — at most eight
   seconds — then dropped; a page that never saw one is dropped at once. No
   board is left in its bootloader by leaving, and no visit meets its own
   port in use. Leaving never stops a sync or a format: the sync is the
   GUI's worker's, and a format runs on the player (clause 59). A page kept
   aside lets its boards' ports go as before; the transfer worker holds no
   serial port.
9. **The write cannot be left.** From a gate's yes until the board's write
   has ended — written and restarted, or failed — and for Update all until
   the last board's has, the tab holds the GUI:
   - every key is the page's, and only `d` and `l` (Details), `←` `→`
     (look at another board or card, clause 26) and `x` on a card whose
     sync runs (below; clause 57) do anything: `q`, `K`, `T`, `M`, `0`,
     the digits, `D`, `V`, Tab, Esc, Enter and the transport letters do
     nothing;
   - the top bar's clicks do nothing — the tabs, the Visualizer item, the
     server label and `[+]` — and its tabs do not light under the pointer;
     below the floor the mini player's buttons answer nothing either;
   - no screen change from anywhere is honoured, and the page is never
     dropped.

   The lock covers the whole tab, cards included. `←` `→` still switch to a
   card to look at it, and nothing on a card can start meanwhile: no Sync,
   no Settings, no gate. A sync already running goes on beside the write,
   since they share no device, and `x` on that card is still honoured: it
   stops the sync at its next save and writes nothing to any board. The
   footer stays the write's (it has no room for more); the card's own *Stop
   at the next save* link names the key.
10. **Ctrl+C, the window's close button and, on macOS, Cmd-Q and the app
    menu's (or the Dock's) Quit still quit**, as on every GUI screen and
    as in the standalone page: they are the ways to cut a write short, and
    they leave the board half written (writing it again fixes that: its ROM
    bootloader always answers). The write's hint, on the footer, warns
    against unplugging the board; the mini player's line, while the write
    runs out of sight, names quitting too (clause 4). During a write these
    quit at once even while a sync runs: the write's rule wins, no gate is
    shown (no modal outranks the page during a write), and the sync is cut
    where it stands and recovered at its next run from its last save (the
    card contract's crash table). With no write under way, a quit during a
    sync is clause 57's.
11. **Quitting lets the boards go first.** When the player quits (any way:
    `q`, Ctrl+C, the window's close button, Cmd-Q) with a page — the tab's,
    or one kept aside — that has seen a board, the GUI tells its worker to
    let go and waits, reading its reports, until every port is free: a
    moment for listens, the rest of a read and its restart when one is under
    way. The wait is bounded at eight seconds, and in the terminal a wait
    past a second says why on stderr (`dev.letting_go`). With no page, a
    page that never saw a board, or a write under way, the quit does not
    wait. The standalone page does the same on its way out. With a sync
    under way and no write, the quit is clause 57's: it waits for the
    sync's next save, at most about 30 s, after the gate where the way of
    quitting has one.

### Keys and the pointer

12. **Keys**, outside a write and a gate: the page's own keys are the
    page's — Enter (the primary), `d` (Details; `l` too), `w` (write again,
    Details open), `c` (count the card's free space, where offered), `s`
    (Show on the player, several boards), `a` (Update all, where offered),
    `r` (ask again), `o` (the Advanced options sheet, clause 33), `x`
    (reset a next write, clause 38), `←` `→` (another board), Esc (the way
    back). The GUI
    keeps `q` (quit), `K` (back to the Library), `T`, `M`, `0`, `V`, the
    digits and `D`, as on the Stats tab; uppercase stays the GUI's, so the
    page's `d` is no slip of Auto DJ's `D`. With a gate up its keys are
    `y`, `n`, `e`, Enter and Esc (clause 23); the GUI's own still leave the
    tab, which cancels the gate (nothing was touched). With the sheet up its
    keys are Tab ⇧Tab, ↑ ↓ ← →, Space, Enter and Esc (clause 34), and the
    GUI's own letters still leave the tab, which drops the sheet — but while
    its path field has the keyboard every key is the field's (clause 36), so
    a typed `q`, `K` or digit is a character of the path. Entering the tab
    takes the keys back from the Library: the queue panel's focus stows,
    and an armed Sonic Path or Auto DJ pick is let go, home to the room that
    asked, as its banner's [X] does — a pick or a queue left holding Enter
    and Esc would name keys that open a gate instead. On the tab only a GUI
    modal outranks the page, and not during a write. The page's keys reach
    it only where it was drawn last frame: below the floor, in the mini
    player, none do.

    With cards (clause 25) the page's keys are those of the subject in view
    (the user's pick of 2026-10-19, the set's card 07 §7, choice 6):
    - **on a board**, the keys above, plus `f` (Format as FAT32…, clause
      59) where the format is offered: for a card the player can't read
      whatever Details shows, for one it reads only with Details open, as
      `w`;
    - **on a card**, Enter (the primary), `d` and `l` (Details), `o`
      (Settings…), `e` (Eject), `p` (Send the plays), `x` (Stop at the next
      save) and `h` (Not a card? Hide it, clause 63);
    - `←` `→` walk every subject, boards then cards, wrapping;
    - with the card's Settings sheet up, its keys are the Advanced sheet's
      (clause 34), plus `←` `→` between its two tabs while no row has the
      keyboard;
    - with a gate up, its keys are clause 23's, `y` `n` `e` Enter Esc, in
      place of the subject's: in the stronger removal gate `e` (or a click)
      flips its box, as it flips the install gate's erase box, and is not
      Eject there (clause 58).
13. **`K` is the tab's key**: from the Library, Now Playing, Stats and the
    Admin hallway it opens the tab, and on the tab it leads back to the
    Library. Its lowercase is no key of the GUI's, so no slip of Shift or
    Caps Lock opens the tab. The base footer line does not name it (every
    locale's line is full; the top bar does).
14. **The pointer** inside the page's area is the page's: hover, click,
    hold and the wheel on its own controls (the port tabs, the primary, the
    text buttons, Details, the log, the gate's buttons and its erase box,
    the sheet's rows, buttons and release list), the hub's way. With a gate
    or the sheet up the card beneath is inert: no hover, no click, no
    tooltip. The top bar's row and the banner's row are the
    GUI's, but for the drag and the release of a press that began on the
    page. A GUI modal or the server menu owns the pointer while it is open,
    as elsewhere (outside a write, clause 9). The wheel scrolls the page's
    log, and never the hidden Library room or queue. Below the floor the
    page takes no click. With cards, the card tabs, the Card view's links
    and primary, its Settings sheet and the transfer's gates are the page's
    the same way (clauses 44, 48, 58).
15. **The footer** is always drawn on this tab, whatever the key-hints
    setting — the page's hint is part of the page, the write's warning not
    to unplug the board above all — and the page's area ends above it. It
    shows the page's hint, in its hosted words: where the standalone page
    says "Esc leave", the tab says "Esc library", since that is where Esc
    goes. The hint names the keys that do something on the board in view —
    the primary's verb first after the switch keys — and never runs past 99
    cells: in a long locale the lesser keys give way before the primary,
    Details and Esc — `s`, `a`, `o` and `f` first, then `c`, `w`, `r` and
    `x`. Where *Format as FAT32 ▸* is the primary its verb is Enter's
    (`Enter format`) and `f` is not named again. With the sheet up the
    hint is the sheet's (clause 34), its Esc and Enter never giving way. It
    outranks the queue panel's
    and an armed pick's tips; only a GUI modal's own come first, and none
    during a write. While a visit waits for the last page to let its ports
    go, the footer is empty.

    On a card the hint names that card's keys, the primary's verb first
    after the switch keys. The switch word is "switch" once a card shares
    the row; boards alone keep "player". The lesser keys give way in a long
    locale before the primary, Details and Esc: `p` first, then `h`, `o`
    and `e`. During a sync the hint is "x stop at the next save · d details
    · Esc library", none of it giving way.
16. **The strip**: Library, Stats, Admin, MP3 Player, then the Visualizer
    item. The session's server label at the right never covers it: the
    label starts at least a blank cell past the Visualizer item, cut short
    with the kit's mark (its menu mark kept) where it would not, so the
    tabs keep every cell and every click. At the GUI's 100 columns a local
    server's label, `http://localhost:3000 ▾`, stands whole in every
    locale; a quick-connect session's or a LAN address's may be cut in the
    longer ones. While any sync runs, the strip's MP3 Player tab wears its
    percentage after its name, in the accent (dim while paused): ` MP3
    Player 28% ` (the user's pick of 2026-10-19 over a chip at the top
    bar's right; card 07 §7, choice 3). The server label's cut rule
    measures the strip with it. The mark goes at the sync's end.
17. **The tab wears the slab** while the screen is up, as the other tabs
    do.

### The card

18. **One card.** The page is one dim rounded card, no fill, in the page's
    column (78 cells, centred; 68 on the standalone page's 72 columns), its
    border dim and staying dim. In it, top to bottom:
    - **the head**: the board's name in bold, and at the right, dim, its
      port — or, with several boards plugged in, its USB serial (`serial
      5B1F007751`; the port is on its tab), with the bridge before it for a
      board that has not answered as mStream firmware. The name is "M5Stack
      Core2" for a board running mStream firmware (answering, or starting
      up), or one whose bootloader
      showed an ESP32 with 16 MB of flash; "Unknown board" for one that has
      not answered; "ESP32 board" for one that turned out not to be a
      Core2; "Board on COM5" for a port that would not open;
    - with several boards, **the identity line** under the head, dim, for a
      board that reports them: its battery and its paired headphones
      (`87 % battery · paired with Paul's headphones`);
    - **Firmware** (a 14-cell label column, then a 58-cell value column):
      one chip — the verdict's glyph, its first words bold in the verdict's
      colour, the facts after ` · ` plain — and under it, dim, at most two
      lines that say what follows from it (clause 19);
    - **SD card**: the card's own row (clause 20);
    - **the bottom row**: `▸ Details` at the left, beside it the dim text
      buttons that apply (Show on the player, Ask again, *Advanced…*,
      clause 33), and at the right **one primary**, the kit's tall button,
      that follows the verdict: *Update ▸*, *Install ▸*, *Read the board ▸*,
      *Try again ▸*, or none — or, with a next write chosen, that write's
      direction (*Write ▸*, *Update ▸*, *Go back ▸*, *Install ▸*, *Write the
      DIO image ▸*; clause 38). The text buttons stop short of the primary:
      one that would run under it (a long locale, two boards, the console)
      is not drawn, and its key still works.
      While another board is written it is the kit's disabled frame (dim,
      no `▸`, a tooltip saying why) under a line saying why (clause 26);
    - **Details** (`d`, `l` kept; it opens by itself when a write fails):
      a dim rule, then the facts one row each — Port (the port, the bridge,
      the serial, and "115200 baud · no reset" for a board only listened to,
      the baud its bootloader answered at once it was read), Board (its
      flash, from `L` asked when Details opens, or from a read), On the
      board (the firmware, its flash mode where its ELF tells it, and its
      ELF; clause 41), This player (the version it carries and where it came
      from, or why it could not be had), Card (the kind, the exact bytes,
      where its free space came from), Written (after a write this visit,
      clause 41) — then the board's
      log in the page's grammar (mm:ss dim; a phase in the accent, a fact in
      the text colour, the host lines and a boot line dim, a failure gold;
      never wrapped; the wheel scrolls it), then, for mStream firmware with
      no write primary and no next write chosen, the dim text button **Write
      v0.8.0 again** (`w`), behind the same gate. A failed write shows only the Port fact before
      the log, so the log's last lines are what Details opens on.

    With cards the board card's rows are unchanged but for two things: the
    card line under the SD card row (clause 20a), and one more primary. For
    a card the player can't read (clause 59), *Format as FAT32 ▸* is the
    primary when the verdict has none and the board reports its card: ✓ Up
    to date, ahead of the pin, or ? Version not recognised (a board's
    status keys say whether it can format, never its version: clause 59).
    The other verdicts with no primary, Starting up and Not a Core2, have
    no card to format. A verdict with a primary keeps it (*Update ▸*,
    *Install ▸*, …, and a next write's direction): that primary wins, and
    the format waits for it.
    Meanwhile the format is the dim text button *Format as FAT32…* under the
    SD card row, behind the format's gate (clause 59), its key `f`. For a
    card the player reads, *Format as FAT32…* is in Details only. A damaged
    card's *Format as FAT32 anyway…* is never the primary (clause 60).
19. **The verdict** is the board's own word against the pin, never
    guessed:
    - **✓ Up to date · v0.8.0** (ok green): the board's version equals the
      pin's exactly — and nothing else. After a write, `· just written` or
      `· just installed` follows.
    - **! Update available · v0.7.0 → v0.8.0** (gold): an older release.
      *Update ▸*.
    - **! Development build · v0.6.0-37-g221d99d → v0.8.0** (gold): a build
      based below the pin. *Update ▸*, the same gate.
    - **✓ Up to date · v0.9.0** (ok green), with a second line "Ahead of
      v0.8.0, the release this player installs.": a newer release, or a
      development build based on the pin or past it (the real Core2's
      `v0.8.0-5-g4e94418`). Nothing to do, so no warning mark and no
      primary — installing the pin would go back; Details' Write again is
      titled *Go back*. Its port tab wears ✓ too, and `device list` says
      "up to date (ahead of this player's v0.8.0)".
    - **? Version not recognised · …** (dim): a version the order cannot
      place. No primary.
    - **✗ Not mStream firmware · UIFlow** (gold — never danger red: a fact,
      not a danger) and **✗ Nothing installed** (gold), after a read:
      *Install ▸*, the install gate (its erase box on).
    - **? Not answering · may not be an MP3 player** (dim), "Its USB chip is
      a Core2's, but many ESP32 boards share it." and "Reading it restarts
      the board for a few seconds.": *Read the board ▸* (clause 22).
    - **? Starting up · v0.7.0 · it answers once its library is listed**
      (dim; the version where its boot line named it): mStream firmware
      listing its library (clause 5), asked again until it answers. No
      primary — a read would only restart the listing — no *Advanced…*,
      its tab `…`. After a write or a read this visit the chip is the
      version in its flash instead (`✓ Up to date · v0.8.0 · just
      written`), and the SD card row says it is starting up (clause 24).
    - **✗ Not a Core2** (gold), under the label *Board* instead of
      *Firmware*, after a read found another chip or another flash size:
      "A Core2 is an ESP32 with 16 MB. Nothing is written to it." No
      primary, no SD card row, never Install or Try again.
    - **! In use by another program** (gold): clause 29.
    - **✗ Write failed · the board is half written** (gold), "It cannot
      start until it is written again." — or `· nothing was changed` when
      the write failed before it touched the flash: *Try again ▸*, behind
      the gate again.
    - While the board is asked: the kit's scan widget, **asking the
      player…**, and under it C's line "over USB, without restarting it —
      its music plays on". While it is read: **reading it over its
      bootloader…** and "The screen is dark a few seconds; it restarts as
      it was."
    - During its write the row is the write itself (clause 23).

    Where a verdict above has no primary and its board reports its card —
    Up to date, ahead of the pin, Version not recognised — a board whose
    card the player can't read wears *Format as FAT32 ▸* (clause 18); the
    verdict's own words are unchanged.
20. **The SD card row** shows only what the running firmware just reported
    — never a number from before a reset, never a guess — in decimal GB
    with one decimal, as the Core2's About shows them:
    - a card the player reads: a █░ bar of used space across the value
      column (accent over the dim track), and under it `21.4 of 59.6 GB used
      · 1,284 tracks` at the left and `38.2 GB free` at the right; the bar
      and the free figure turn gold at 98 % used or under 1 GB free (`! 0.7
      GB free`); with no tracks, a third line says where music goes;
    - free space not counted (FSINFO's count unset): `59.6 GB · 1,284 tracks
      · free space not counted`, no bar (half a bar would be a guess), and
      the count (clause 21);
    - no card: **! No card** (gold) and its fix; an exFAT or NTFS card, a
      GPT card, a card it cannot read: what it is, why it does not play,
      and the fix and its cost, in gold and dim;
    - **unknown**, dim, saying why: being asked; starting up ("? starting
      up — reading its library", clause 5); during a write or a read
      ("untouched · shown again once the board restarts"); a board half
      written or ours not running ("? unknown until the firmware runs
      again"); firmware too old to report it ("? v0.7.0 can't report the
      card." — and "Updating shows it." only when the pinned release
      answers the status query, which the pin says beside its checksum:
      v0.11.0 does, as every release from v0.9.0 on; v0.8.0, the pin the
      examples here were drawn with, did not); a board that is not
      mStream firmware ("? Only mStream firmware can read the card"); a
      port in use ("? unknown until the port is free").

    With formatting in the player (clause 59), the fix lines for an exFAT,
    NTFS or GPT card name the player as the formatter
    (`dev.card_fix_format`, `dev.card_fix_mbr`: "Fix: format it as FAT32 in
    the player (below). That erases the whole card.") on a board that can
    format: its status carries `cid=` and `clock=`. On one that can't, no
    line points below at a format that isn't there:
    - while the pinned release formats, clause 59's line takes the fix
      line's place, "v0.11.0 can't format a card. Updating lets it.", with
      the verdict's *Update ▸* (the set's card 06 §4);
    - while it doesn't (the pin v0.11.0), the fix lines keep today's words
      ("Fix: format it FAT32 (MBR) on a computer. That erases it." and its
      MBR twin), moved with their ten locales to `dev.card_fix_format_pc`
      and `dev.card_fix_mbr_pc`, and clause 59's line, "v0.11.0 can't
      format a card.", follows them.

    The line that says where music goes under a card with no tracks gives
    way to clause 20a's *not filled yet* line on a card whose `cid=` is
    `-`.

20a. **The board names its card.** Once the firmware reports `cid=`
   (0.12.0: `@status`'s card id and commit), the SD card row adds one dim
   line under its numbers.
   - A card this computer has synced (its id in `cards.json`): "This is Car
     music · synced 3 days ago from Home". A set's card names itself
     ("Everything 2/2").
   - Its commit older than the last one this computer wrote, which happens
     when the card was synced and put back while the board was on: a
     second line says "The player still plays the music from before the
     last sync: restart it to load the new." Nothing restarts the board
     from here.
   - An id this computer never saw: "Filled by mStream on another
     computer."
   - `cid=-` on a FAT card: "Nothing from mStream on it yet: put it in this
     computer's card reader to fill it."
   - A board whose status has no `cid=` (before 0.12.0, or the key left
     out of a long line): no line.

   The line gives way first at the floor (clause 4).

21. **The count.** Where free space is not counted, on a board that
    answers the status query, the row offers **Count free space** (`c`, a
    dim text button, "a minute or more on a big card"): one `@count`, never
    started unasked, never during a write. While it runs the row is the
    kit's scan widget with its percentage ("counting free space…"); when it
    is done the bar and the numbers take its place. The firmware refuses
    while music plays, and the row then says "Pause the music on the player
    first, then count." in gold; a count the library update or the card
    refused says so in the row.
22. **Read the board** is the one reset outside a write. It is offered
    only to a board that does not answer, the cost said on the card before
    it ("Reading it restarts the board for a few seconds."), and it has no
    gate: it writes nothing. The worker reaches the bootloader, reads the
    app description, and restarts the board at once — never holds it dark
    while anyone decides. The result is the chip's: Not mStream firmware,
    Nothing installed, Not a Core2, or our firmware that was not running
    (its version, and the card unknown until it runs); our firmware is then
    asked again over USB.
23. **The gate.** Every write passes one gate, one function, whatever
    started it: the primary (Update, Install, Try again, and a next write's
    Write, Go back and Write the DIO image), Details' Write again, and Update
    all. It is the kit's gold warning modal, 74 cells wide
    (the width the area allows on the standalone page), centred in the
    page's area, with no `[X]` and no scrim: a gold bold title, then what
    goes on and how long it takes, what you will see, what stays (one green
    line) or, with the erase box, what goes, and the one danger; its
    buttons at the bottom right, **the safe choice first and wearing the
    modal primary**, named for what stays — *◂ Keep v0.7.0*, *◂ Keep it as
    it is* (an install), *◂ Keep them as they are* (Update all) — then the
    write in the dim secondary. Its titles: *Update from v0.7.0 to v0.8.0?*
    · *Replace v0.6.0-37-g221d99d with v0.8.0?* · *Install mStream firmware
    v0.8.0?* · *Write v0.8.0 again?* · *Go back from v0.9.0 to v0.8.0?* ·
    *Update 2 players to v0.8.0?* — and with a next write chosen, clause
    39's. The erase box lives only in the install
    gate, on by default (`e` or a click flips it), its cost on the line
    under it; the install gate also says, in gold, that the page cannot tell
    a Core2 from another ESP32 board with 16 MB of flash. Enter, Esc and `n`
    keep; `y` or a click on the secondary writes; a click elsewhere on the
    page does nothing; a click on another tab of the GUI leaves, which keeps
    the board as it is. The gate is the board's last untouched moment: its
    reset comes after the `y`. Updates never erase but where the sheet's
    Erase box chose it (clause 39); the standalone page's `--erase` /
    `--no-erase` set the install gate's box and an update's erase.
24. **Done is the card again.** There is no Done page and no Close. The
    board restarts, the worker listens again (the boot line, then the
    status), and the card comes back with the new verdict (`✓ Up to date ·
    v0.8.0 · just written`) and its card as the firmware reports it. Once
    its boot line is heard the board is starting up — listing its library
    before it answers, a minute and a half for a big card on v0.7.0 — and
    the worker keeps listening, asking again every 30 s (a question it
    already holds is answered as soon as its loop runs), for 3 minutes at
    most, until it does:
    meanwhile the SD card row says "? starting up — reading its library"
    and the log "COM3 is starting up — it answers once its library is
    listed", never "said nothing". The write's lock ended with the restart
    (clause 9): the tab can be left meanwhile, which stops the asking. Past
    3 minutes the card is as for a board that never answered ("? unknown
    until the firmware runs again"), and `r` asks again. The
    busy line says the write's end in one dim line — "Updated from v0.7.0 just now. The
    SD card was not touched.", after a first install "Installed just now.
    Next: your music in /music on a FAT32 card." Only a step forward is
    "Updated": after *Go back* (or over a version the order cannot place)
    the line is "Replaced v0.9.0 just now. The SD card was not touched."
    The board's own first line, the plan and the checksum are in the log.

### Several boards, and cards in readers

25. **The tab row of subjects.** With two subjects or more — boards
    plugged in, cards in this computer's readers — the kit's tab row stands
    above the view: one tab per board in the order the OS lists the ports,
    then one per card in a reader, by drive.
    - A board's tab: its short port name (`ttyACM0`, not `/dev/ttyACM0`)
      and its mark — ✓ green, ! and ✗ gold, ? and `…` dim, a write's
      percentage in the accent, `in use` dim.
    - A card's tab, on Windows: its drive letter, then its name (its
      `state.json` name, else its volume label, else "new card"); elsewhere
      its name alone (the user's pick of 2026-10-19, card 07 §7, choice
      11). Its mark is clause 25a's.
    - The open tab is the slab. `←` `→` (wrapping) or a click switch, and
      the view is that subject's: a board's card, with its own verdict and
      primary, or a card's Card view.
    - The right edge says, in order of precedence: the write's words
      ("writing COM3…") and Update all's ("Update all · 2 of 3"); "syncing
      Car music…" while a sync runs; "reading E:…" while a card is read;
      the needs-an-update count, "1 of 2 needs an update" (gold) where one
      does and **Update all (2)** (a dim text button, `a`) where two or
      more do; else the count of players and cards ("2 players plugged
      in", "1 player · 1 card", "2 cards"), or the mix ("2 players · 1
      other board", "1 player · 1 in use", "asking COM7…"). A board counts
      as a player only once it answers as mStream firmware.
    - Short of room, the cards' names give way first, down to the drive or
      six cells of the name; then the right edge's text; then the last
      tabs (still reached by `←` `→`).
    - Down to one subject, the row goes and the view is that subject's
      alone; nothing is reset. A board plugged in never takes the view; a
      card put in takes it only from the empty card (clause 30; the user's
      pick of 2026-10-19, card 07 §7, choice 4).

25a. **A card's mark:**
   - ✓ green: in step with its server, as far as the last answer goes;
   - ! gold: something waits (new tracks, removals, plays to send, set-up,
     a sign-in);
   - ✗ gold: it can't be synced here (not FAT32, not a disk of its own,
     write-protected, a newer format, a disk check first);
   - ? or … dim: being read, or its server being asked or not answering;
   - a sync's percentage, in the accent while it runs and dim while it's
     paused;
   - `out`, dim: a card taken out during its sync (clause 53).

26. **Look, don't write.** During a write the other boards can be switched
    to and looked at; their facts stay as read, and a board plugged in
    meanwhile is asked as usual. Nothing else starts: no second write, no
    read, no count, no Show on the player, no Update all. A board whose
    primary would write wears it disabled, under "One write at a time: this
    one can follow COM3.". The writing board stays in view: its tab carries
    its percentage, the busy line its progress whichever card is open
    ("COM3 · writing v0.8.0… 62% · about 15 s left"), and the footer names
    it ("please wait — unplugging COM3 now would leave it half written"). A
    card's view during a write is looked at only (clause 9).
27. **Show on the player** (`s`, a dim text button beside Details) asks a
    board that answers the status query, with two boards or more plugged
    in, to show "This one" and its port's short name on its screen for five
    seconds, with one buzz. The busy line says so; a refusal (the player
    still starting, another screen up, a computer driving the dancer) says
    why. Boards that do not answer the query get no button and no greyed
    promise.
28. **Update all** (`a`, the tab row's right edge) appears when two boards
    or more need an update and none of their own next writes keeps them out
    (clause 42). One gate names every board — port, serial, from → to, the
    mode where it is DIO (six at most, then how many more) — and the worker
    writes them one after another, never at once: one board dark at a time,
    each in its own mode. It
    only updates: never an install, an erase or a step back, and a board
    whose bootloader shows anything but an older release of ours is left
    as it was. A board unplugged before its turn is passed over; the first
    failure stops it, and the next board is not started. As it goes, each
    board's result is the busy line ("COM3 written and checked in 44 s"),
    and at its end "2 players updated in 1 min 31 s, one after the other.
    Nothing on their SD cards changed." — or, stopped, the board that
    stopped it, with Details open on its log.
29. **Plugging in and out; a port in use.** A board plugged in is asked
    with no reset (its tab `…`, the busy line "COM7 plugged in: asking it
    over USB, without a restart") and never takes the view; one unplugged
    takes its tab with it, and if it was open the view moves to the next
    (right, else left) — down to one, "COM5 was unplugged. COM3 is the only
    one left; nothing was reset." A board on a port another program holds
    (a serial monitor, the IDE, another mStream Player) is that board's
    state, not the page's failure: "! In use by another program", who might
    hold it and the way out, the SD card unknown until the port is free,
    and **Ask again** (`r`). The page never takes a port back by itself,
    and holds a port only while it talks to its board. A card put in is
    read with its tab marked `…` and the busy line "E: put in: reading the
    card…"; once read, the busy line says once what the next sync would do.
    A card taken out takes its tab with it ("Car music was taken out of E:.
    Nothing was being written to it."), but a card in its sync keeps its
    tab (clause 53).
30. **Nothing plugged in.** With no Core2-shaped port and no card in a
    reader, the view is still there, empty:
    - "Nothing plugged in", one line on what to do ("Plug the Core2 in over
      USB, or put its SD card in this computer's card reader. Each shows up
      here by itself."), and the kit's scan widget, "watching the USB ports
      and card readers…". The workers look at both every two seconds, so
      there is no Look again and no Close.
    - Where the OS shows empty readers (Windows gives them drive letters),
      one dim line names them: "Card readers here: E: and F:, empty."
    - **▸ Not showing up?** (`d`) opens today's hints unchanged (a cable
      that carries data, M5Stack's driver, Linux's dialout group), plus
      two: a card shows up once the computer has mounted it, and the
      computer's own disks and fixed disks are never listed. Then the
      serial ports seen and the card readers seen.
    - Ports or volumes that cannot be listed at all say so in gold.

### The command line

31. **`mstream-player device flash`** is the same page on its own, at
    72×24 at least: its header (the page's title, and the firmware it would
    write at the right), the card (68 cells, the bar 50), its busy line and
    its tips row on the last two rows, Esc leaving once the worker has let
    every port go. `--firmware` and `--release` pick another image and
    `--flash-mode` its build — each board's next write, never what a board
    is measured against (clause 43) — `--port` keeps the page to that one
    board, and `--erase` / `--no-erase` set the gate's erase (clause 23).
    The standalone page shows boards only: cards are the GUI tab's and
    `device card`'s (clause 65).
32. **`--yes` keeps its printed steps.** One board: today's lines — the
    firmware, the board as its bootloader read it, the plan, the percents,
    Done and the board's first line — with no gate (the flags answer it);
    the plan calls a write over a newer version of ours `go back`, never
    `update`. A port another program holds is said on stderr with the
    serial-monitor hint, exit 1. Several boards with no `--port`: a refusal
    listing each board with its verdict and card, exit 1 — unless `--all`,
    which updates every board running an older release of ours, one after
    another, with the steps under each port, stopping at the first failure
    (exit 1), and names the boards it left alone (`updated 2 of 2 · skipped:
    COM9 (not answering)`). `--all` needs `--yes` and refuses `--port`,
    `--erase`, `--firmware` and `--release`, and writes each board in its own
    mode (or `--flash-mode`'s). `device list` asks every board at once with
    no reset and prints today's line, then its verdict and its card;
    `--ports` prints today's lines and opens nothing; exit 0 with boards, 1
    with none, 2 when the ports cannot be listed. `device list` appends a
    board's card when `cid=` names one this computer knows ("· Gym (synced
    yesterday)").

### Advanced options

33. **The way in.** A dim text button **Advanced…** joins the card's bottom
    row after ▸ Details (and after Show on the player and Ask again), three
    cells apart like the other links, and the footer gains `o advanced`.
    `o` or a click opens the sheet for the board in view; nothing else
    does — the sheet never opens by itself, and Enter still opens the gate
    for the board's own write. It is not offered while a write runs, nor
    on a board not heard yet, one starting up with nothing else known of it
    (clause 19), one that does not answer (Read the board first), one that
    is not a Core2, one whose port would not open or is in use by another
    program, nor with no board at all.
34. **The sheet** is the kit's neutral modal: the accent border, the title
    "Advanced options · COM3" in the accent and bold, `[X]` dim at its
    right (bright under the pointer), no scrim, the card beneath inert (the
    gate's modal rule). It is 74 cells wide (the console's width less four
    on the standalone page) and centred in the page's area, its top held
    where its tallest state would put it, so a row that appears under a
    choice never moves it. It is not gold: choosing is not a warning, and
    nothing in it writes or resets anything. Three radio groups in the
    kit's grammar — a dim uppercase label, one row per choice, `(•)` in the
    accent on the focused group's choice, the chosen name bold, a dim
    `— description` after it — then a help line, then its buttons:
    - **FIRMWARE**: *v0.8.0 — this player's release, recommended*; *Another
      release — listed from GitHub when you choose it* (clause 35); *A
      local build — a file or a build folder on this computer* (clause 36).
    - **FLASH MODE**: *QIO — faster* and *DIO — runs on every Core2, a
      little slower*. A row carries one mark at most, so it fits the
      console's sheet in every locale: the row the board runs now says so
      (", as COM3 runs now", from its ELF) — that is its default too, but
      for v0.5.0's one image — else the board's default says "(the
      default)"; beside a mark DIO's meaning is its short one (*DIO — runs
      on every Core2, as COM3 runs now*). A
      release with one image has its other row dim, saying why ("not in
      v0.5.0, which has one image"); a local build's mode is its own
      (clause 36).
    - **ERASE**: `[ ] Erase the whole flash first — loses the settings`
      (`[✓]` ticked, Space or a click).
    - **The help line** (the Auto DJ room's): a dim rule and at most two
      dim lines about the focused group — what the pin is and that "Up to
      date" always means it, when GitHub is asked, what DIO costs and buys,
      what a local build is checked for, what an erase loses. Short of rows
      (the GUI's floor) it is the first thing to give way.
    - **The buttons**, bottom right: *Use defaults* (dim: clears this
      board's next write and closes) and *Apply* (the modal primary; the
      kit's dim disabled look while a release is not picked or a local
      build not vetted).

    Tab and ⇧Tab move between the groups — Firmware, its sub-row when it
    has one (the release control, the local choosers), Flash mode, Erase,
    the buttons — and ↑ ↓ choose inside one, as a radio does; Space ticks
    the box, opens the release list or the focused chooser; ← → move along
    the choosers and the buttons; Enter applies and closes (on the buttons,
    the focused one; on a chooser, opens it); Esc or `[X]` closes and
    changes nothing. The footer says the keys of the focused group ("Tab
    next group · ↑↓ choose · Space tick · Enter apply · Esc close").
35. **Another release** is listed from GitHub only when chosen. Choosing it
    (↓ onto it, Space or Enter on its control, a click) opens the kit's
    dropdown under its control (a rounded accent frame over the sheet's
    lower rows, no title, no `[X]`) and sends one request — GitHub's
    releases API, unauthenticated — unless the list is in hand: the page
    keeps the answer for the visit (the worker for fifteen minutes), and a
    failure is never kept, so opening the list again asks again. Opening
    the tab never asks. While it asks, the kit's scan widget, "asking
    GitHub for the releases…". Then one row per release with a merged
    image, back to v0.5.0, newest first, the pin left out (it is the first
    choice): its tag, its date as GitHub gives it, its images ("QIO and
    DIO", "DIO only"), a pre-release marked in its own row, one newer than
    the pin "newer than this player"; then *Show 1 pre-release* when there
    are any (Enter adds them for this visit and the row becomes *Hide
    pre-releases*; each visit starts with them hidden); then, dim, "asked
    GitHub at 08:41". Offline or refused, one gold line says why ("GitHub
    did not answer: offline?", "GitHub: too many lists from this address;
    try after 09:12" from GitHub's reset time, another answer), then the
    releases already on this computer beside the SHA256SUMS they were
    checked against ("v0.7.0 · on this computer since 2026-10-07"), which
    write with no network, then *Try again*. ↑ ↓ move the keyboard cursor
    (the accent slab; the picked value wears `•`), Enter or a click picks,
    Esc or a click elsewhere closes the list and keeps what was picked.
    Picked, the control reads `v0.7.0 ▾` and one dim line under it says
    the release's date, its images and its direction against this board
    ("a step back from COM3's v0.8.0", "an update from v0.6.0", "what COM3
    runs now", "newer than this player"). Its Flash mode offers the builds
    it has. Apply waits for a pick.
36. **A local build** opens a sub-row of three dim text buttons. *Choose a
    file…* and *Choose a folder…* open the operating system's own dialogs
    (the setup wizard's and the `.torrent` picker's backends, on a thread;
    owned by the player's window when it has one), the file dialog typed to
    `.bin`. *Type a path* turns the row into the kit's path field — the
    line editor with its caret, Tab completing what is typed from the
    local disk, Enter reading it, Esc giving the keys back — prefilled with
    the last local path picked, the one thing the player keeps between
    visits (in memory, for the run). Where no dialog can open (no desktop
    session, the dialogs switched off) the row says so and the path field
    takes the keyboard. A folder means its `firmware.factory.bin`, or the
    one in its single PlatformIO environment. What is picked is read on the
    worker at once and vetted: a merged image of at most 8 MB whose app
    description names mstream-mp3-player. Vetted, the choice's row names
    the path (its tail, `…\.pio\build\core2`) and two dim lines say what it
    is — its file, its version (a `-dirty` suffix kept), its size; its mode
    from its header, and "a local build, not a release" — and Flash mode is
    the image's own: its row chosen ("the image's own (80 MHz in its
    header)"), the other dim ("not this image's: build the DIO variant for
    it"). Refused, one gold line in `--firmware`'s words; an app alone is
    refused and pointed at `device flash --firmware`, which still writes
    one. Apply waits for a vetted pick.
37. **The modes.** QIO is the default for the pin and every release; DIO
    is a choice for one board's next write. An image's mode is read from
    its own bootloader header (byte 3's low nibble: `F` the QIO build, `0`
    the DIO build), never from its name; a board's from the ELF id it
    reports, against every release's QIO/DIO pair the player carries
    (v0.5.0–v0.11.0, bumped with the pin) and the images it has had in hand
    this run. A board whose ELF says its DIO was chosen (a release that had
    both builds) says so under its chip, once, dim: "In DIO (runs on every
    Core2): updates keep it." — and its Update ▸ and Update all write the
    pin's DIO image, checked by the pin's second built-in checksum. A board
    on v0.5.0's one image (DIO, no choice) updates to QIO. The player
    stores nothing about a mode; a QIO board says nothing about its mode on
    the card. Every mode is named with its meaning — "QIO (faster)", "DIO
    (runs on every Core2)" — never alone.
38. **Apply makes a next write** for the board in view, one write long. The
    card says it on one row under the Firmware chip: "Next write:" dim, the
    choice in the accent — "v0.7.0 in QIO (faster)", "v0.8.0 in DIO (runs
    on every Core2)", "local build v0.8.0-5-g4e94418 in QIO (faster)", with
    ", erasing first" when the box was ticked — and a dim text button
    **Reset** (`x`) at the value column's right edge, on the first of its
    lines with room for it (else on a row of its own); while the image is
    fetched, a dim line under it with its percent ("getting it… 42%"); one
    that cannot be had, why, in gold, under it. Applying starts the image's
    download (or reads the build again) at once, so the gate's yes rarely
    waits. The write's own words name the version it puts on and, where
    the gate names one, the mode ("writing v0.8.0 in DIO… 41%"). The chip, the port
    tabs' marks and the count never move: up to date is the pin (clause
    19), and after going back to v0.7.0 the card offers the update. The
    primary follows the choice: *Write ▸* for the same version in another
    mode or a local build, *Update ▸* or *Go back ▸* by the version order
    (a step back is never an update), *Install ▸* over other firmware —
    and an up-to-date board, which has none, gets one. A choice ends with
    its write when it is done, with Reset, Use defaults or applying the
    defaults (the busy row then says once, dim, what the next write is
    now: "COM3's next write is back to the defaults: v0.8.0 in QIO
    (faster)."), with the board unplugged, and with the page, so leaving
    the tab ends it; a failed write keeps it for Try again.
39. **The gate names the choice.** It is the one gate (clause 23). Its
    title names the mode where the write is not QIO, or not the board's own
    — *Update from v0.7.0 to v0.8.0 · DIO?*, and for the same version in
    another build *Write v0.8.0 · DIO?* (its safe button *◂ Keep it as it
    is*, its write *Write in DIO*) — and a local build has its own, *Write a
    local build over v0.8.0?*, never an update. Under the title, whenever
    the image is not the board's default, one line says what is written
    and the check it passed: "The DIO image reads the flash on 2 lines, as
    M5Stack ships the Core2. Slower lists and decoding than QIO, but it
    starts on every Core2."; for a board on DIO by its ELF, "COM5 runs the
    DIO image now: its ELF, aa45f60e, is v0.7.0's DIO build. This update
    keeps DIO."; "This Core2 runs DIO now; this write puts QIO back.";
    "Release v0.7.0 in QIO (faster), from GitHub, checked against its own
    SHA256SUMS."; and, in gold, "A local build, not a release:
    v0.8.0-5-g4e94418 in QIO (faster), from …\core2. Checked only for being
    mStream firmware, not for working." Gold lines say what may go wrong: a
    release newer than the pin ("Newer than this player: it was made with
    v0.8.0 and has not been tried with v0.9.0."), a release that is not the
    pin ("Not this player's v0.8.0: the card will still offer that
    update."), going back ("A step back in a beta: v0.7.0 may not read
    every setting v0.8.0 saved.", under the green line, which stays true).
    With the sheet's Erase ticked the green line splits: what goes in gold
    ("The whole flash is erased first: settings, paired headphones, touch
    calibration and the resume point go.") and "The SD card's music stays."
    in green, and the button says both verbs (*Erase and update*, *Erase
    and write*). A local build's last line says it can be written over
    again ("the board's bootloader always answers"), and it is read again
    at the `y`: changed since it was picked, nothing is written, the busy
    row says so, and the card's next write names the new build for the
    gate to show. The `y` hands the worker the image the gate named, and
    no other: a board whose next write moved before the worker read the
    yes — a choice or a Reset sent a moment before, not yet on the card, or
    a mode a listen just brought — is not written; the busy row says so
    ("COM3's next write changed before the yes; nothing written — look at
    its card again.") and the card shows the board as it is, for the gate
    to be drawn again. At the defaults today's gates are unchanged.
40. **The restart loop.** After a write the worker listens 6 s for the
    board's first line. Heard the ROM's reset banner twice or more and the
    firmware never (`engine::restart_loop`, the one rule — to verify on a
    Core2 that really loops on QIO, R5.8), the board keeps restarting: the
    chip is gold **✗ Keeps restarting · v0.8.0 in QIO**, two dim lines
    under it say what probably happened and what fixes it ("Restarted 3
    times in 6 s: its flash may not run QIO. DIO starts on every Core2, and
    the settings stay."), the
    SD card is unknown until the firmware runs, Details opens on the
    banners, and the busy row says "v0.8.0 went on, then the board kept
    restarting. The SD card was not touched." The worker fills in the cure
    as the board's next write (the same version's DIO build, by the loop):
    the Next write line shows it, and the primary is **Write the DIO image
    ▸**, whose gate is *Write v0.8.0 · DIO?* with "It keeps restarting on
    the QIO image." before the DIO line. Nothing is written by itself;
    Reset takes the offer back, the card stays gold with no primary, and
    the DIO image waits in Advanced…. Heard running again, the loop and its
    offer are gone.
41. **Details** names a board's mode where its ELF tells it: *On the
    board* "mstream-mp3-player v0.8.0 · DIO · ELF 3523b80e", or "mode not
    known" for an ELF the player has never had in hand. After a write this
    visit a read-only **Written** row says what went on: its version, its
    mode and where it came from with the check it passed ("v0.8.0 · DIO ·
    this player's release, checked by its built-in checksum", "v0.7.0 ·
    QIO · release v0.7.0, checked against its SHA256SUMS", "… · a local
    build, checked only for being mStream firmware"). The log reads each
    image's mode from its header (`0x1000: E9 03 02 40 — 40 MHz, the DIO
    build`).
42. **Several boards.** A choice is the board in view's, on its card only:
    the port tabs wear no mark for it, the sheet's title names its port,
    and switching boards shows each board's own Next write line or none.
    Update all writes the pin to every board that needs it, each in its
    own mode (the mode its sheet chose, else its ELF's, else QIO), its rows
    naming DIO where it applies; a board whose next write is another
    release or a local build is left out and named in its gate ("COM9 is
    left out: its next write is v0.7.0. Write it from its tab."), and the
    count, *Update all (2)*, counts only the boards it will write. Where
    fewer than two would be written the row's right edge says how many
    need an update ("2 of 4 need an update").
43. **The command line keeps parity.** `--release` and `--firmware` are
    every board's next write, never what it is measured against: the card
    is judged against the pin, its Next write line says where the choice
    came from ("set by --release and --flash-mode"), Reset (`x`) goes back
    to the pin, and the header's right edge names the image the page
    writes, with its mode when it is DIO ("firmware v0.7.0 · DIO ·
    release"). `--flash-mode qio|dio` picks the build for the pin or
    `--release`; beside `--firmware` it must agree with the file's header,
    or the run is refused before a board is touched; without it each board
    keeps its ELF's mode. `device releases [--pre-releases]` prints the
    dropdown's rows — tag, date, images, pre-release, and what is on this
    computer — with the same one request. `device list` says `· DIO` beside
    a DIO board's version. Every choice the sheet makes the flags can make,
    and the reverse, but an app alone (the command line's only).

### Cards in a reader

44. **The Card view.** A card in a reader is shown as one dim rounded card
    in the board card's grammar: 78 cells, a 14-cell label column, the █░
    bar, one primary bottom right. Top to bottom:
    - **the head:** the card's name in bold ("Card in E:" until it has
      one), and at the right, dim, its drive and file system ("E: ·
      FAT32");
    - **Holds:** its libraries and their server ("Rock, Jazz from Home"),
      one server after another for a card with peers;
    - **Space:** a bar of used space across the value column, and under it
      "41.2 of 63.8 GB used · 4,812 tracks" at the left and "22.6 GB free"
      at the right, from the mounted volume, never the board;
    - **Last sync:** when, and "verified" when the last run's verify
      passed; a count of skipped files in gold;
    - **Next sync:** what a sync would do now, from the server's last
      answer ("12 new · 2 to remove · 37 plays to send"), or "nothing new on
      Home", or the server not answering, or the fit's verdict (clause 49);
    - **Converts:** the conversion rule ("only what the player can't play,
      to Opus 192k"); a card for someone else adds "no Auto DJ index, plays
      not sent";
    - **the bottom row:** `▸ Details`, then *Settings…*, *Send the plays*
      (whenever plays wait) and *Eject*, and the one primary. Short of room
      before the primary (a long locale), *Send the plays* gives way first,
      as `p` does on the footer (clause 15), then the links from the end;
      their keys still work;
    - **Details:** a dim rule, then the facts (Drive, Card, Server,
      Removes, Plays, Player), then the transfer's log in the page's
      grammar, then *Sync again* and *Check every file…* (the deep check).
      A fact is one line in the value column, its words whole in every
      locale with the longest values a card realistically shows (the
      Deviations log's entry of 2026-10-20 names them). Where a value of
      its own runs longer, what is the card's gives way, never the words:
      Removes' path is cut with an ellipsis before its count ("… and 1,234
      more"), and a volume's long name (Linux, macOS) takes the Drive
      fact's serial, then its cluster size, each whole. Past that, the
      line ends in an ellipsis.

    A state replaces the rows it can't fill and never moves the others.
    Under its gate or its sheet the card keeps the rows it has at rest,
    its primary's place kept and the button not drawn, so the modal
    covers the card as the set draws it (cards 05 and 06: a modal taller
    than the card shows the card's corners beside it) and one shorter
    leaves no half a button. The primary, one or none:
    - *Sync ▸* whenever something waits; none when the card is in step;
    - *Set up ▸* on a new card;
    - *Eject ▸* for a card the player can't read, and after Done;
    - *Add the server ▸* when its server isn't saved here; *Sign in ▸* when
      its sign-in lapsed;
    - *Check again ▸* after a disk check;
    - *Choose what fits ▸* when the selection doesn't fit or passes the
      track cap;
    - *Set up as card 2 ▸* when a set waits for its next card.
45. **The states at rest** are the set's card 03; each says what the card
    is and the way on, never a code:
    - new, and new with music of its own;
    - in step; being read; asking its server;
    - not FAT32 (exFAT, NTFS, FAT12, GPT), with the route to the player's
      Format: eject it, put it in the Core2, Format on its tab (clause 59);
    - not a disk of its own (another file system beside it), never
      written;
    - write-protected; Linux names that would list wrong (with the remount
      hint);
    - its server not saved here; no server saved on this computer at all
      (clause 3a); signed out, whose plays wait too ("37 plays wait for
      Home: sign in to send them"); not answering;
    - written by a newer format (an unknown major: read, never written);
    - a disk check first (a possible cut rename stops every sync until the
      card is checked);
    - doesn't fit; over the player's track cap (20,000);
    - no `device.txt` yet (the player it is written for is assumed and said
      in one dim line under Converts; Settings' Player changes it);
    - a set's card (clause 62).

    The computer's own disks and fixed disks are never listed (the
    drive-safety rule, part A7).
46. **Nothing is guessed.** The Card view's figures come from the mounted
    volume and the card's own files; *Next sync* comes from the server's
    list and is never claimed fresher than its answer (a full read every
    7th day or 10th sync, the card contract's rule). The fit is computed on
    this computer, never from the board (clause 49).

### The card's settings

47. **The card's settings live on the card,** in its private
    `/.mstream/state.json`: its name, its server's key and nickname, its
    libraries, conversion, removals, covers, Auto DJ, who it is for, the
    player it is written for, its set, its peers' budgets. No address,
    token or username. Written by the private-file rule (`state.new`,
    fsync, delete, rename). `cards.json` on this computer only caches them.
48. **The Settings sheet** (`o`, *Settings…*; *Set up ▸* and *Choose what
    fits ▸* open it on Music) is the kit's neutral modal, 74 cells wide,
    over an inert Card view. Its top is held where its tallest state puts
    it. It has **two tabs, Music and Writing** (the user's pick of
    2026-10-19, card 05 alternate A; the one scrolling list, alternate B,
    was not chosen):
    - **Music:** Name (a text field), From (a dropdown of this computer's
      saved servers; another server is clause 58's gate on Apply), the
      server's libraries as checkboxes with their size on the card and
      their tracks, and each peer's libraries under its label with its
      daily budget.
    - **Writing:** Convert (only what it can't play, or everything), Into (a
      dropdown of codecs; MP3 above 192k dim until the server offers it),
      Removals (follow the server, or never remove: Q1), Covers, Auto DJ,
      For (me, or someone else: no Auto DJ index, no plays sent), Player
      (from `device.txt`, else a dropdown defaulting to the board last seen
      or the firmware last flashed), Peer budget (the volume widget).

    A help line under the rule explains the focused row (two lines, an
    ellipsis past them). Its words keep to the two lines in every locale,
    at the GUI's floor (70 cells) and on the console's narrower sheet
    (64), with the longest names a server realistically goes by (28
    cells: an address with its scheme and port, or Quick Connect's name)
    and a long library; the ellipsis is for a longer name still. The
    buttons: *Use defaults* and *Apply*. Apply
    writes `state.json` and closes; it never syncs. Esc or `[X]` changes
    nothing. Set-up is the same sheet titled "Set up F:" with *Cancel* and
    *Set up*.
49. **The fit line,** under both tabs, is recomputed at every change. It
    counts:
    - each file rounded to the cluster;
    - conversions estimated as duration × kbps;
    - directory entries;
    - the card's own files, twice over at a checkpoint;
    - the player's reserve;
    - a 1% margin of at least 256 MB;
    - the 20,000-track cap.

    It reads "✓ Fits · 42.9 of 63.8 GB after this sync · 20.9 GB to spare",
    "! Tight …", "✗ Doesn't fit by 10.9 GB …" with its offers (*Convert
    everything (saves about 24 GB)*, *Leave out Audiobooks*, *Spread over 2
    cards…*), "✗ 23,410 tracks: the player holds 20,000 on one card", the
    scan widget while a library's list is coming, or a peer's days at its
    budget. The Card view's *Next sync* and *Choose what fits ▸* use the
    same figures.
50. **Set-up** (*Set up* in the sheet) writes `/music`, the empty first
    commit that mints the card's id, and `state.json`, then shows the ready
    card. Nothing else on the card is touched. A card with music of its own
    then asks clause 58's adoption gate. With no server saved on this
    computer, the sheet's From and its libraries are empty and *Set up*
    still writes the card, its server left empty; the Card view then asks
    for one (clause 3a).

### A sync

51. **Sync ▸** starts a run on the transfer worker for that card. The run
    lists the source, plans, passes the gates (clause 58), sends the plays,
    deletes, fetches, renames and reads back, checkpoints, writes the
    covers and Auto DJ index, commits and verifies (the card contract's
    2.12). The Card view follows it with no primary:
    - Holds stays;
    - **Syncing** is a bar of the plan's bytes, with "1,204 of 4,812 · 6.1
      of 21.3 GB" and the time left once the rate is known, then dim "12.4
      MB/s · saved 3 min ago, safe to stop";
    - **Now** is the file being fetched and what it becomes;
    - **Plays** is their result;
    - phases with no estimate use the scan widget.

    **A file the listener changed** — one the transfer wrote that is no
    longer what its ledger says — stays the listener's: never written over
    or removed. The server's copy is written beside it as "… (2)" in the
    same run, and Details' log names both ("kept 05 - Undertow.flac: it
    changed on the card since the last sync; Home's copy is beside it as
    05 - Undertow (2).flac"). This is the user's pick of 2026-10-19, and
    the engine does it already (part A8).
52. **Stopping and pauses.** *Stop at the next save* (`x`) finishes the
    files in flight and checkpoints; the plan stays on the card and the
    next Sync goes on from it. A pause is the run waiting by itself, with
    "! Paused" and why, and *Stop for now*:
    - for a peer's daily budget: the file is parked with its partial kept,
      and the run goes on after 00:00 UTC and a few minutes, the days left
      said at the current pace;
    - for a server that didn't answer: 1 min, doubling to 30, with *Try
      now*;
    - for a sign-in: *Sign in ▸*.

    While paused, the tab's percentage and the bar are dim.
53. **Errors.** An error per file never stops a run: it is counted in gold
    on Last sync, listed in Details with its reason, and tried again next
    time. An error per run stops it in gold with the way on:
    - the card is full;
    - it was taken out;
    - it is a different card (the identity triple is checked again before
      every batch of deletes, every rename, every commit and after any I/O
      error or resume);
    - it refuses writes.

    A source that refuses (a peer's 403 on copies) is said on the card and
    left out. A card taken out during its sync keeps its tab, marked `out`,
    and its view ("✗ Taken out · stopped at 1,204 of 4,812 · put it back in
    any reader to go on: nothing is lost") until it is back in any reader
    or *Give up this run…* forgets it here. The plan stays on the card
    either way. Another card in that reader is its own subject, never
    written by that run.
54. **Done is the Card view again.** The result goes on Last sync ("just
    now · verified · 37 plays sent to Home") and once on the busy line
    ("Synced Car music in 28 min: 4,822 tracks, verified. Eject it before
    you take it out."). The primary is *Eject ▸* (the user's pick of
    2026-10-19, card 07 §7, choice 12: a card pulled before the OS lets go
    can lose its last writes), which asks the OS to let the card go and
    says "Car music can come out now.", or the OS's words when it can't.
55. **The worker is the GUI's.** The transfer worker lives at the GUI
    level and outlives the tab and the page. Leaving the tab, switching
    subject or screen, or going below the floor never stops a sync. The MP3
    Player tab wears its percentage (clause 16). Below the floor, the mini
    player's line says "syncing Car music 28% · it goes on while you
    listen". Coming back opens the view on the card being synced.
56. **One sync at a time** (the user's pick of 2026-10-19, card 07 §7,
    choice 9). A second card's Sync waits its turn: "3 new · waits for Car
    music to finish (one sync at a time)", with *Don't wait*. A firmware
    write runs beside a sync under its own lock (clause 9).
57. **Quitting during a sync.** `q`, the window's close button, Cmd-Q and
    the app menu's Quit open a gold gate: "Stop syncing Car music and
    quit?", with *◂ Keep syncing* first and the primary, and *Stop and
    quit*. On the yes the GUI waits for the next save, at most about 30 s,
    the busy line counting, then quits. Ctrl+C in a terminal stops at the
    next save without asking, saying so on stderr; a second Ctrl+C quits at
    once (the user's pick of 2026-10-19, card 07 §7, choice 8). A
    force-quit, a pulled card or a power cut is recovered at the next run
    (the card contract's crash table).
    - **While a firmware write runs, the write's rule wins** (clauses
      9–11): `q` does nothing; Ctrl+C, the close button and Cmd-Q quit at
      once with no gate and no wait, leaving the board as clause 10 says;
      the sync is cut where it stands and recovered at its next run from
      its last save. The gate above is shown only when no write runs.
    - **`x` during a write** is still honoured on a card that syncs (clause
      9): the stop touches no board.

### The gates

58. **Every gate is the kit's gold modal** (clause 23's grammar): 74 cells,
    no `[X]`, the safe choice first and the modal primary, named for what
    stays. Enter, Esc and `n` keep; `y` or a click on the second button
    goes; a click on another tab leaves, with nothing written.
    - **Removals (Q1):** up to 50 need no gate (Next sync says how many;
      Details lists them). Above 50: "Remove 214 tracks from Car music?",
      the first five paths and how many more, what stays in green, and how
      to keep them (Settings' *never remove*), with *◂ Not now* and *Remove
      214 and sync*. **Above half the card's ledger,** the stronger gate:
      why that is suspicious, the count per library, and a box, "[ ] Home
      really has dropped them". Until the box is ticked the gate draws only
      *◂ Not now*: the second button isn't there and `y` does nothing, so
      nothing invites the yes. `e` or a click flips the box, the install
      gate's grammar (clause 23); ticked, *Remove 2,412 and sync* appears
      beside the keep and `y` works; `e` again unticks it and the button
      goes. The footer says "e tick the box", and once it is ticked "y
      remove and sync" first, as the install gate keeps "e erase first"
      whichever way its box is. A card set to *never remove* never shows
      either.
    - **Another server:** only on a choice (Settings' From, or the
      unknown-server card's *Fill it from … instead…*). "Make Studio this
      card's server?" with what stays, what leaves at the next sync, and
      the plays that keep waiting for the old server; *◂ Keep Home* and
      *Make Studio its server*.
    - **Adopting files** after Set up on a card with music: "Adopt 1,190
      files Home also has?" They are matched by path and size, then a quick
      fingerprint (two 4 KiB reads each), and marked ADOPTED, not VERIFIED;
      the rest stay the listener's. *◂ Leave them all as mine* and *Adopt
      1,190*.
    - **A relay**, above about 1 GB: "Sync 21.3 GB over a relay?" with the
      time both ways; *◂ Not now* and *Sync over the relay*.
    - **A parent's proxy**, once per peer, only on a parent whose mStream
      doesn't say it forwards the copy header (`GET /api/`'s
      `features.federationProxy`; before B10): *◂ Leave Lisbon out* and
      *Copy through Home*.

### Formatting

59. **Formatting is the player's (Q3).** The PC never writes a card's file
    system: the tab asks, and the player confirms on its own screen and
    formats (the firmware repo's `docs/HOST-STATUS.md`, "@format", as at
    its commit d8855d7).
    - **Where it is offered:** for a card the player can't read
      (`card=exfat`, `ntfs`, `gpt`), the board's SD row offers *Format as
      FAT32 ▸*: the primary when the verdict has none (clause 18), else the
      dim *Format as FAT32…* (`f`) under the row. For
      a card the player reads, *Format as FAT32…* is in Details only:
      `@format` may bring up the erase question for any card, the player's
      own music card included (the user's pick of 2026-10-19; the
      firmware's refusal of a card it reads, its commit d8e797b, was
      reverted by 93f84fc).
    - **Only where the firmware can:** `@format` came in 0.12.0 with
      `@status`'s `cid=` and `clock=`, and is offered only on a board whose
      status carries them: their presence is how the page tells, never the
      version (clause 5's rule). A board whose status has neither says,
      in the SD row where Format would be offered, "v0.11.0 can't format a
      card." (`fmt_old`), and so does an `@err 7 format` answer. "Updating
      lets it." (`fmt_old_update`) follows only when the pinned release
      answers `@format`, which the pin says beside its checksum as it says
      `PINNED_ANSWERS_STATUS` (a `PINNED_ANSWERS_FORMAT`, part B6), the
      rule of clause 20's "Updating shows it.". The pin v0.11.0 does not
      answer it, so today the line ends at "can't format a card." under
      today's fix line (clause 20): a v0.11.0 board is up to date with no
      Update (clause 19), and an update to the pin would not let it format.
    - **The tab's gate:** "Format the card in COM3 as FAT32?" — the player
      erases the whole card, every part of it, even parts it can't read,
      and makes one FAT32 volume with an empty `/music`; about how long,
      from the card's size; that the player confirms on its own screen in
      two steps (`gate_fmt_confirm`; the hold is named once the player
      asks, below); what is lost, in gold, named from `@status` — the
      card's kind (`card=`) and size, and on a card the player reads its
      music ("your music, 1,284 tracks, on one FAT32 volume"; while the
      board indexes it, `tracks=building`, "your music, still being
      indexed, on one FAT32 volume"); and that the player writes its
      `device.txt` at once. *◂ Keep the card as it is* and *Ask the
      player*.
    - **On the yes** the tab sends `@format`. At `@format ok` the question
      is on the player's screen, and the SD row says "waiting for you to
      confirm on the player" and "Nothing is erased until you hold the red
      button there." (`fmt_waiting_2`, in the words of the player's own
      hint). The firmware has no verb that takes the question back, so the
      tab offers no Cancel: the player's own *Cancel*, a tap on its tab bar
      or a minute with no answer ends it (`@format no`). The wait ends on
      `@format no`, `@format 0` or `@err 4 format`. At `@format 0` the
      board restarts to format (its boot lines pass by); then the progress
      ("formatting the card… 41%", from `@format <percent>`), then
      `@format done size=… free=… cluster=…`, and the row is the empty card
      as the board then reports it, with "Formatted just now: put it in
      this computer's card reader to fill it."
    - **Other outcomes**, on the busy line: `@format no` within the
      firmware's minute, "Not formatted: it was cancelled on COM3's
      screen."; past it, "Not formatted: nobody confirmed on the player
      within a minute." Three answers are not the player's no, and are told
      apart by what came before them:
      - a refusal at once, in place of `@format ok` (`@err 4 format
        ui|screen|viz|busy|card`: the player starting up, another screen
        up, a computer driving the dancer, a restart under way, no card),
        says why;
      - a refusal at the player's second answer, after `@format ok` and
        with no restart (`@err 4 format card|size|busy`: another card in
        the slot since the board started or none answering, a card FAT32
        can't take, a restart under way), says why and that nothing was
        formatted, a word this player doesn't know too ("Not formatted:
        COM3 answered …; nothing was erased."), and the row is the card as
        the board reports it;
      - a failure after the restart (`@err 4 format
        card|size|write|mkfs|check|files`) says so in gold, the row shows
        the card as the board then reports it, and Details' log has the
        reason.

      Their words are new with part B6 (`fmt_refused_*`, the second
      answer's among them, and `fmt_failed_*`): the set drew none. Each
      fits the hosted busy line, 96 cells, beside the longest port name
      the page shows (16 cells, a Mac's `cu.usbmodem14101`). Leaving the
      tab doesn't stop a format: the player formats on its own, and coming
      back asks the board again (clause 5).
    - **On the player** (part B5, as built at d8855d7): step 1, the
      firmware's question, its words by the card, with *Cancel* and
      *Format…*; step 2, "Erase the whole card?", **the hold** (the user's
      pick of 2026-10-19, card 06 alternate B): its red *Hold to erase*
      (`format.sure_button`) answers only to a finger kept on it 2 s,
      filling meanwhile, and under the body, from the start, the hint
      "Hold the red button for 2 seconds". Lifted sooner or slid off, it
      empties, the hint turns amber and nothing else happens; the question
      stays up, and no tap, double tap or run of taps passes both steps.
      *Cancel* stays the default: its button, a tap on the tab bar or each
      step's minute ends the question.
      Then the boot's "Formatting the card" over a bar, and "Card
      formatted: ready for your music". The words are the firmware's own
      (its repo's UiStrings, ten languages). Whether a 2 s hold reads well
      on the Core2's touch panel is the firmware's device check to say.
    - **Never during** a write, a read or a count of that board, and never
      twice at once.
60. **Damaged cards** (`card=other` — blank, another system's, or a FAT
    that didn't mount, which the board can't tell apart — and
    `unreadable`) are offered the computer's disk check first ("Try it in
    this computer's card reader first: a disk check there may save what's
    on it."), as the firmware's own question does. *Format as FAT32
    anyway…* is a dim second step, never the primary.

### Plays and sets

61. **Play history.** A sync reads the card's new plays first
    (`/.player/plays.jnl`) and maps each through the ledger. It posts them
    to the server that filled that track; a peer's go to its parent with
    the peer's id. Plays never go to another server, nor from a card for
    someone else. Details counts what can't be sent:
    - plays from before the player's clock was set (kept on this
      computer);
    - plays of the listener's own files (counted);
    - plays the log overwrote (counted).

    *Send the plays* (`p`) posts them without syncing music. The board's
    Details gains a *Clock* fact, and its log a line when clause 5 set the
    clock.
62. **Sets.** A selection spread over several cards (*Spread over 2
    cards…*, a sheet of its own: split by artist A to Z, or by library;
    each card's range, size and tracks; *Cancel* / *Make the set*). Each
    card is a whole library of its own, and its `state.json` holds the
    whole set.
    - **A set's card:** a *Set* row ("card 1 of 2 · card 2 (M–Z) seen
      yesterday", or "card 2 (M–Z) not set up yet: put a new card in"), and
      moves counted in Next sync.
    - **The next card:** a new card while a set waits for one offers *Set
      up as card 2 ▸*, with *Set up as a card of its own* beside it.
    - **A full card** offers to move the edge or add a card.
    - **Moves** are two-phase: an album leaves its old card only after its
      new card has committed it.
63. **Hiding a volume.** A removable volume the player can't use is listed
    with why (the user's pick of 2026-10-19, card 07 §7, choice 5), and
    *Not a card? Hide it* (`h`) keeps it off the row on this computer, by
    its volume serial, until *Not showing up?* shows it again.

### The cards' command line

64. **`device cards`** lists every card in a reader, one line each: the
    drive, the name, the size, the file system, then the server, tracks,
    last sync and next sync, or why it can't be synced. It also lists
    refused removable volumes with their reason. Exit 0 with cards, 1 with
    none (saying which readers it saw empty), 2 when the volumes can't be
    listed. It reads only; it asks each card's server once.
65. **`device card <drive> status|setup|sync|plays|eject [--yes]`.**
    `<drive>` is a drive letter, a mount path, or a card's name when one
    card has it.
    - `status` prints the Card view's facts as `key  value` lines and
      writes nothing.
    - `setup`, `sync` and `plays` print their plan and stop (exit 1)
      without `--yes`; on a terminal they ask "go on? [y/N]". With `--yes`
      they print each step and exit 0 when done, 1 on a refusal or
      failure, and 3 when paused or stopped. Exit 3 means "run it again
      later; it goes on from the last save".
    - Ctrl+C stops at the next save (clause 57).
    - **The gates are flags that name what they allow** (the user's pick
      of 2026-10-19, card 07 §7, choice 7):
      - `--allow-removals N`, with N the plan's exact count
        (`--list-removals` prints them), and the stronger gate the same;
      - `--server S --take-over`;
      - `--adopt` or `--keep-mine` (set-up on a card with music refuses
        without one);
      - `--through-parent`.

      The relay warning is printed and `--yes` goes on.
    - **The settings are flags** on `setup`: `--name`, `--server`,
      `--libraries`, `--convert needed|all`, `--to`, `--removals
      follow|never`, `--covers on|off`, `--autodj on|off`, `--for
      me|someone-else`, `--player`, `--budget`, `--set N`, `--into-set`.
      They write `state.json` only.
    - `eject` asks the OS to let the card go.
66. **`device format [--port P]`** (the user's pick of 2026-10-19, card 07
    §7, choice 10, for parity with the tab) sends `@format` to the board's
    player and prints the card as the board reports it (its kind, size and
    tracks: what is lost), the player's answer, the progress and the
    result. It has no `--yes`: the player confirms. Exit 0 formatted; 1
    declined, timed out, refused (older firmware too) or failed (clause
    59's three answers that are not the player's no), and with several
    boards and no `--port`, a refusal naming them as `device flash --yes`'s
    does; 2 no board.
67. **The command line keeps parity** (clause 43's rule for cards): every
    choice the Card view, its Settings and its gates make, the flags make,
    and the reverse. A gate's flag says what it lets through, never merely
    "yes". The lines are plain English, as `device list`'s are: lines a
    script reads.

## Wording

The page's strings are `dev.*`. The card's: the labels (`label_firmware`
"Firmware", `label_card` "SD card", `label_board` "Board", `details`
"Details", `not_showing` "Not showing up?"), the chip's words (`fw_*`), the
card row's (`card_*`), the head's (`name_*`, `serial`, `id_*`), the
primaries (`btn_*`, with `▸` beside them in code), the text buttons
(`link_*`), Details' facts (`fact_*`), the tab row's counts (`tabs_*`), the
empty card's (`none_*`), the gate's (`gate_*`), the notes (`note_*`,
`identify_*`, `refused_*`), and the footer's pieces (`hint_*`), joined with
` · ` in code. Glyphs — ✓ ! ✗ ? ▸ ▾ ◂ █ ░ ▰ ▱ ←→ and the bullets — are
drawn beside the words, never inside them (the kit's rule), and fall back to
ASCII on the bare Windows console.

| Key | English |
|---|---|
| `gui.top.mp3` | MP3 Player |
| `dev.hint_library` | Esc library |
| `dev.hint_working` | please wait — unplugging now would leave the board half written |
| `dev.hint_working_port` | please wait — unplugging %{port} now would leave it half written |
| `dev.hint_working_unseen` | writing the MP3 player's firmware — please wait: unplugging it or quitting now would leave it half written |
| `dev.letting_go` | restarting the Core2 into its firmware before quitting… |
| `dev.letting_ports_go` | letting the USB ports go… |

A board starting up (clauses 5, 19, 24) is `dev.fw_starting` "Starting
up" with `dev.fw_starting_why` "it answers once its library is listed",
`dev.card_starting` "starting up — reading its library" and the log's
`dev.log_starting` "%{port} is starting up — it answers once its library
is listed".

`dev.hint_working_unseen` is the mini player's line while the page writes
out of sight (it wraps, so it has no footer's width to keep);
`dev.letting_go` is the quit's stderr line (clause 11). The no-player
card's hints (`dev.no_device_*`, `dev.ports_seen*`) are today's, unchanged.

The Advanced options' strings (clauses 33–43) are `dev.adv_*` for the
sheet — its title, group labels (drawn uppercase in code), choices and
their descriptions, the release list's rows and states, the local build's
choosers, facts and refusals, the nine help texts, its buttons — and, on
the card, `link_advanced`, `next_*`, `link_reset`, `mode_qio` /
`mode_dio` (every mode named with its meaning), `fw_in_dio`,
`fw_looping*`; in the gate `gate_title_mode`, `gate_title_local`,
`gate_dio`, `gate_loop`, `gate_kept`, `gate_qio_back`, `gate_src_*`,
`gate_back_beta`, `gate_newer`, `gate_not_pin`, `gate_erase_any`,
`gate_card_stays`, `gate_plugged_local`, `gate_all_left_out` and the
buttons `gate_do_mode`, `gate_do_local`, `gate_do_erase_*`; the busy row's
`note_mode`, `note_back`, `note_local`, `note_looping`, `note_reset`;
Details' `label_written` and `fact_written_*`; the footer's `hint_*`
(`hint_advanced`, `hint_reset`, the sheet's and the list's). Glyphs stay
in code: `▾`, `(•)`, `[✓]`, `↑↓`, `←→` and the scan widget's cells.

| Key | English |
|---|---|
| `dev.link_advanced` | Advanced… |
| `dev.adv_title` | Advanced options · %{port} |
| `dev.next_write` | Next write: |
| `dev.mode_qio` · `dev.mode_dio` | QIO (faster) · DIO (runs on every Core2) |
| `dev.fw_in_dio` | In DIO (runs on every Core2): updates keep it. |
| `dev.fw_looping` | Keeps restarting |
| `dev.gate_title_mode` | Write %{to}? |
| `dev.gate_title_local` | Write a local build over %{from}? |

The `--yes` lines and `device list` are plain English, like the `plan:`
line: they are lines a script reads.

The cards' strings (clauses 3a, 20a, 25a and 44–67) are `dev.card.*`:
- the tab row's (`tabs_*`, `tab_out`, `tab_new`, `readers_*`), the busy
  line's (`busy_*`) and *Not showing up?*'s two new hints
  (`hint_card_shows`, `hint_own_disks`);
- the head and labels (`name_in`, `head_*`, `label_*`);
- the rows' words (`holds_*`, `last_*`, `verified`, `next_*`, `conv_*`,
  `ago_*`, `reading`);
- the primaries and links (`btn_*`, `link_*`);
- the footer's pieces (`hint_*`);
- Details' facts and the transfer's log (`fact_*`, `log_*`);
- a sync's (`sync_*`, `stopping`, `paused*`, `waiting_*`, `plays_sent`,
  `done*`, `skipped`, `skip_*`, `ejected`, `waits_for`);
- the run's errors (`err_*`) and the states at rest (`new_*`, `foreign*`,
  `not_fat`, `route_*`, `shared*`, `unknown_server*`, `no_server*`,
  `signin`, `newer*`, `check*`, `locked`, `not_utf8*`, `fit_*`,
  `plays_*`);
- sets (`set_*`), and the board's card line and clock (`board_*`,
  `log_clock_*`);
- formatting on the tab (`fmt_*`);
- the gates (`gate_*` and their `hint_gate_*`);
- quitting (`quit_*`, `mini`);
- the Settings sheet (`set_title`, `setup_title`, `tab_music`,
  `tab_writing`, `grp_*`, `row_*`, `opt_*`, `player_*`, `budget_mb`,
  `fit_*`, `help_*`, `more_below`, `btn_defaults`, `btn_apply`,
  `btn_cancel`, `btn_do_setup`, `spread_*`).

Five `dev.*` keys change their words: `card_fix_format`, `card_fix_mbr`,
`none_title`, `none_body` and `none_watching`. With the 342 new
`dev.card.*` keys that is 347 keys drawn in the set, three of them added
since the picks of 2026-10-19 (`gate_fmt_music`, `log_kept_yours` and
`fmt_old_update`), and the frames reuse 42 existing `dev.*` keys
unchanged. The set's card 07 §5 has every one with its English, and a
German draft wherever a floor frame shows it (160 of them). Three of the
347 wait for firmware that part B5 did not build, and are not added until
it is (`fmt_cancel_note`, `hint_fmt_cancel`, `gate_fmt_part`: clause 59,
Deviations log), so **the build adds 344 keys now** (339 new, 5
changed), in all ten locales. Two more keep today's fix words for a
board that can't format while the pin can't either (`card_fix_format_pc`,
`card_fix_mbr_pc`, clause 20): moved, their ten locales as they are, not
new. Part B6 adds the format's refusals and failures beside them
(`fmt_refused_*`, `fmt_failed_*`, `fmt_no_answer`, `fmt_lost`), which
the set did not draw, and `gate_fmt_unread`: 19 keys; its review adds
`fmt_refused_second_other` and `gate_fmt_music_building`, 21 in all
(Deviations log). Part B13's terminal half adds the clock's other ends,
which the set did not draw either (clause 5): `log_clock_range`,
`log_clock_rtc`, `log_clock_ours`, `log_clock_silent` and
`log_clock_failed`, 5 keys.
Not keys: the invented names, paths, sizes and versions; the command
line's lines (plain English, like `device list`'s); the firmware's own
strings for its dialogs (its repo's lib/core UiStrings, ten languages).
Glyphs stay in code: ✓ ! ✗ ? … ▸ ▾ ◂ █ ░ ▰ ▱ ←→ ↑↓, the radios, the boxes
and the caret.

| Key | English |
|---|---|
| `dev.card.tabs_cards` | %{n} cards |
| `dev.card.label_holds` · `label_space` · `label_last` · `label_next` · `label_converts` | Holds · Space · Last sync · Next sync · Converts |
| `dev.card.next_new` · `next_remove` · `next_plays` | %{n} new · %{n} to remove · %{n} plays to send |
| `dev.card.conv_needed` | only what the player can't play, to %{codec} |
| `dev.card.btn_sync` · `btn_setup` · `btn_eject` | Sync · Set up · Eject |
| `dev.card.link_settings` · `link_stop` | Settings… · Stop at the next save |
| `dev.card.board_is` | This is %{card} · synced %{when} from %{server} |
| `dev.card.log_kept_yours` | kept %{file}: it changed on the card since the last sync; %{server}'s copy is beside it as %{copy} |
| `dev.card.log_clock_unset` · `log_clock_off` | the player's clock was unset: set it (@time) · the player's clock was %{s} s off: set it (@time) |
| `dev.card.log_clock_rtc` | the player's clock wasn't set: its clock chip didn't take the time (@err 4 time rtc) |
| `dev.card.log_clock_ours` | the player's clock wasn't set: this computer's clock isn't between 2026 and 2099 |
| `dev.card.gate_remove_title` | Remove %{n} tracks from %{card}? |
| `dev.card.gate_half_box` · `hint_gate_box` | %{server} really has dropped them · e tick the box |
| `dev.card.gate_fmt_title` | Format the card in %{port} as FAT32? |
| `dev.card.gate_fmt_music` | your music, %{n} tracks, on one %{fs} volume |
| `dev.card.fmt_waiting` | waiting for you to confirm on the player |
| `dev.card.fmt_waiting_2` | Nothing is erased until you hold the red button there. |
| `dev.card.fmt_old` · `fmt_old_update` | %{version} can't format a card. · Updating lets it. |
| `dev.card.quit_title` | Stop syncing %{card} and quit? |
| `dev.card.mini` | syncing %{card} %{pct}% · it goes on while you listen |
| `dev.card_fix_format` (changed) | Fix: format it as FAT32 in the player (below). That erases the whole card. |
| `dev.none_title` (changed) | Nothing plugged in |

The cards' command-line lines are plain English, like the `plan:` line.

## Out of scope here

- **A check of GitHub on its own.** The page asks GitHub's release list
  only when the sheet's *Another release* is chosen (clause 35), never to
  tell a board that a newer release exists: up to date is the pin.
- **Bytes of music on a board's card** (Q5): the firmware still reports
  `music=?`, so the board's SD row shows tracks only. A card in a reader
  shows its bytes, read from the volume (clause 44).
- **The battery and what is playing** beyond the identity line of several
  boards (Q6).
- **A port by hand and a write speed** (card 06's R8.2, R8.3; Paul's Q8,
  2026-10-10): neither is drawn. A board on a bridge the page does not
  know stays `--port`'s; the baud ladder steps down by itself and logs
  every rung.
- **An app alone in the tab** (R7.1): it keeps the board's bootloader and
  so its mode, which the tab cannot show; `--firmware` still writes one.
- **Firmware work.** A `mode=` field in `@status` (Q10) would name a local
  build's mode on the board; the ELF pairs cover every release without it.
- **Remembering a choice.** A next write lasts one write on one board
  (Q1); only the last local path is kept, in memory, for the run.
- **`device identify`** on the command line.
- **The tray launcher's item.** mStream's launcher gets no "MP3 player"
  entry; the GUI's tab is the way in.
- **A board page that outlives the tab.** Leaving lets the boards' page
  go (clause 8): it is kept only as long as its worker takes to let its
  ports go, never shown again; the next visit starts over. Only the
  transfer worker, which holds no port, outlives the tab (clause 55).
- **A quit that waits during a write.** The write's end cannot be waited
  for on the way out (half a minute, and the user asked to quit): clause
  10 says what a quit then does.
- **Formatting on the PC.** No OS formatter, no raw writes, no elevation:
  the player formats (Q3). The transfer plan's table of the PC's
  formatters stays a note.
- **Play history over USB** (`@plays`): not planned. The card's log is
  read in a reader (clause 61).
- **exFAT cards:** the firmware reads FAT only; the Card view walks such a
  card to the player's Format.
- **Restoring a server from a card.** A card is a playable mirror, never a
  source for a server (Q1).
- **Several syncs at once** (clause 56).
- **The classic TUI:** no MP3 Player tab, as before; the commands serve
  it.

## Translation notes

- The page (`src/device/page.rs`) draws from the worker's whole boards
  (`desk::Event::Board`): the verdict, the primary, the card and its
  numbers are `board.rs`'s, so the tab's chip and `device list`'s words
  never disagree. The page keeps only what is its own: which board is in
  view, Details, the gate, the write's lock, the time left (worked out
  from the write's percent), the log and the notes.
- The card is laid out as a list of rows first, then drawn: its height is
  what its rows need, cut to the rows the area has, so Details' log takes
  what is left at the floor and nothing is drawn past the card's border.
  Wrapping is span-aware and measured in cells, and a word wider than the
  column (a Japanese or Chinese sentence) breaks at the cell.
- The gate is the page's first modal: the card beneath is drawn with no
  pointer and its click and tip registries are dropped before the gate
  draws (the kit's modal rule), and the page's `modal_open` is true while
  it is up.
- *Writing* is the page's lock: set when the gate's `y` sends the write,
  cleared when that board's write has ended — the worker's board carries
  its `written` again — or, for Update all, when its end is told; the
  worker's refusal of that write (or of Update all) clears it too, and no
  other refusal does: one of a command sent before the `y` (Details' `L`,
  a count) can name the same board after it, while the write goes on. A
  write asked for while the image still downloads waits for it, locked; a
  download that stalls fails after 30 s with no byte, failing that write
  with nothing touched. *Holds the board* is "told to let go, not yet
  `Released`, and a board was seen": a page that never saw a board holds
  nothing. `Page::release` is the host's one way to let a page go — Esc's
  way, then the worker's reports read again.
- Under test the tab builds its page on channels the test holds both ends
  of: no worker is ever spawned, so no firmware is downloaded and no
  serial port is opened by `cargo test` (a Core2 may well be plugged in).
  The worker's own tests run it on the fake engine (`MSTREAM_DEVICE_FAKE`'s
  grammar: `status:`, `old:`, `silent:`, `chip`, `busy`, `/in=`, `/out=`,
  `/held=`, `/mode=dio`, `/elf=`, `/loop=qio`, `/listing=<s>` — a board
  starting up, answering nothing for that long after each start while it
  prints `[bt]` and Arduino lines — `/format=` — how a board from 0.12.0
  on answers `@format`: formatted by default, `no`, `late`, `silent`, a
  refusal at once, `second-<why>`, `fail-<why>` — and any `@status`
  field),
  whose trace says what was done to each board, and on a shelf of images
  and a release list in GitHub's place (`firmware::tests::Shelf`): no test
  reaches GitHub either.
- The sheet is the page's second modal (`page/sheet.rs`): its state —
  the board it is for, the source, the release picked, the mode, the box,
  the focused group, the release list's cursor, the path field, the local
  pick's state — lives on the page while it is up, and the page's
  `modal_open` is true for it as for the gate. Apply, Use defaults and
  Reset are `Cmd::Choose`; a local path is read by `Cmd::Vet` and answered
  by `Event::Vetted`; the list is `Cmd::Releases` and `Event::Releases`,
  kept on the page for the visit (`Listing`). The native dialogs run on a
  thread through `setup::picker` (`pick_firmware`, `pick_build_folder`,
  which honour `MSTREAM_NO_PICKER` like the `.torrent` picker), answering
  on a channel the page reads in its pump; under test the page's dialogs
  are a stand-in the test sets, so no test opens one. The page claims the
  whole keyboard (`Claim::All`) while the path field has it, and the GUI's
  tab then keeps none of its letters; the field's caret is lifted to the
  GUI's surface, so the window turns its input method and paste on for it.
- The last local path is a static of the page's module: kept for the run,
  never written to disk.
- Machine translations for the nine non-English locales, as everywhere;
  key names are never translated, and every new key sits on the same line
  of all ten files.
- **The Card view** is `src/device/page/card.rs`, the page's third part
  beside the board card and `sheet.rs`. It reads whole cards from the
  transfer worker (`card::worker::Event::Card`) as the board card reads
  boards from the desk: the view keeps only which subject is in view,
  Details, the sheet, the gate and the notes.
- **One ring of subjects:** `page.boards` stays; a `page.cards` list
  beside it, and `page.open` names either kind (an enum of port or card
  key). The tab row draws both (`tab_row`). `hint_parts` and `card_layout`
  branch on the open subject's kind.
- **The transfer worker** is started by the GUI at launch, not by the
  page, and owned by `Gui`. The page talks to it over `Cmd`/`Event`
  channels it borrows each visit; the page dropped on leaving drops only
  its ends. The worker owns the per-volume run lock and the OS advisory
  lock (`LockFileEx`, `flock`), so a command-line run and the GUI never
  write one card at once. It watches the readers every 2 s with part A7's
  `find()`. The clock of clause 5 is the desk's listen's, over
  `listen::clock`, which the hidden `device time` drives too.
- **Under test,** the Card view rides channels the test holds, as the
  board page does (`Page::quiet`), with fake volumes: part A7's fake mount
  tables, a scratch directory standing in for a card, the card contract's
  fixtures. The transfer client talks to loopback stubs
  (`api::transfer::stub`); the board's clock and format to the fake engine
  (`MSTREAM_DEVICE_FAKE`). No test touches a real volume, a real server or
  a serial port.
- **Frames:** the set's frames are exact grids drawn by `page.rs`'s rules.
  The page's tests compare its English frames character for character at
  100×30, and draw every state, gate and sheet whole at 100×24 in every
  locale, as the board card's do.
- **Words:** the set's German drafts are a start. Every locale is checked
  at the 14-cell label column and the 58-cell value column (Details'
  facts), and at the sheet's 70 and 64 cells of help, with the longest
  values of the Deviations log's entry of 2026-10-20. German
  addresses the listener as *du* ("Deine eigenen Dateien bleiben", "Du
  bestätigst …"), the form `locales/de.yml` uses everywhere it addresses
  anyone (about 60 *du*-forms; no formal *Sie*: its capitalised
  "Sie"/"Ihre" are third person). Where the `dev` section can, it stays
  impersonal ("Erst in den Einstellungen … wählen"); the drafts do the
  same.
- **The other eight locales** were written by the build (2026-10-19), in
  each file's own terms and form of address: French *carte*, *lecteur*,
  *vous*; Spanish *tarjeta*, *reproductor*, *tú*; Italian *scheda*,
  *lettore*, *tu*; European Portuguese *cartão*, *leitor*, *você* forms;
  Polish *karta*, *odtwarzacz*, *ty*; Russian *карта*, *плеер*, *вы*;
  Japanese *カード*, *プレーヤー*; Chinese *存储卡*, *播放器*, Auto DJ kept
  in Latin letters as the file keeps it. Where a locale already says the
  word for a card for a board or a tab too, the tab is named otherwise:
  Italian *vista* ("nella vista di COM3"), Polish *zakładka*. Counts sit
  after a colon where a language's plural would bend the noun ("utworów:
  %{n}", "файлов: %{n}"), so one form serves every number. Japanese and
  Chinese say "that server" where their sentence would name it a second
  time, since the completeness test counts every placeholder. They wait
  for a native review with the German.

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
- **2026-10-09/10 — Alternate A of the eleventh set adopted.** Paul, 7 Oct:
  "Right now it's a bit confusing and has too much information" — show
  the SD card's space and usage, make it clear whether the firmware is up
  to date, and put a warning modal before the firmware is written. Paul
  chose A on 9 Oct, with these decisions: opening the tab only listens
  (the bootloader after a gate's yes, or Read the board, said plainly);
  Update all and Show on the player are in (Show only for boards that
  answer the status query); during a write, look, don't write; the
  standalone `device flash` gets A too, `--yes` keeping its printed steps;
  no GitHub check, no Format card, tracks only, the DIO image behind
  `--firmware`; the pin moves to v0.8.0, up to date is the pin exactly, and
  a development build or a newer version is never called an update. What
  went: the subtitle, the step line, the Board / Port / On the board / To
  install rows (folded into the head, the chip and Details), the erase row
  (into the install gate), Cancel and Close, the Done page and its five
  lines of "Next" (Done is the card again; the next step is the SD card
  row's own hint), the failure page (the chip, Try again, Details opened
  by itself), the pick list (port tabs, every board read with no reset),
  Look again (the worker watches by itself) and `▸ Show logs` (inside
  Details, `l` kept). Clauses 5–8 are rewritten for listening; clause 9
  gains Details and the switch keys; clauses 18–32 are new.
  **Grafts** from the set's README, all taken: the gate's safe button names
  what stays ("◂ Keep v0.7.0" for "Cancel"), and one gate function gives B's
  titles (Update from … to … · Replace … with … · Install mStream firmware
  … · Write … again · Go back from … to …), so Details' repairs pass the
  same gate as the primary; the bridge's USB serial on the card's head with
  several boards; C's first-second line ("over USB, without restarting it —
  its music plays on"); "your MP3 player" only for mStream firmware ("this
  Core2" otherwise — in the gate's words); each port tab wearing its
  board's mark; the no-player card's "▸ Not showing up?" holding today's
  hints unchanged; C's one-line Done result; C's count at the tab row's
  right edge ("1 of 2 needs an update").
  **Where the build parts from the frames.** The install gate always
  carries card 05's gold line on telling a Core2 from another ESP32 with
  16 MB of flash: after a read the page cannot tell them apart for any
  board, not only one on a shared USB chip. A board that has not answered
  is headed "Unknown board" (card 05's rule) where card 02's single-board
  frame says "M5Stack Core2", and its chip adds "may not be an MP3 player".
  The pinned release, v0.8.0, does not answer the status query, so "Updating
  shows it." under an old firmware's card row is said only when the pin's
  `PINNED_ANSWERS_STATUS` is true (the frames assumed a pin that answers).
  The footer drops its lesser keys in a long locale rather than run past
  99 cells. Show on the player appears with two boards or more, as in
  card 05's frames — a single board has nothing to be told apart from.

- **2026-10-10 — Ahead of the pin is up to date.** The design set drew a
  board past the pin as a gold **! Newer than this player** with "the board
  is fine as it is." under it; on the real Core2 Paul read the mark as a
  warning the next line took back. Nothing is to be done for such a board,
  so it now wears the ok chip, **✓ Up to date · <its version>**, and the
  second line says why its version is not the pin's ("Ahead of v0.8.0, the
  release this player installs."); its tab mark is ✓ and `device list` says
  "up to date (ahead of …)". Going back stays behind Details and the
  *Go back* gate.

- **2026-10-10 — Advanced options: card 07 adopted, with the grafts.** Paul,
  10 Oct: "I'm thinking we need to add some kind of advanced options menu
  to this UI. We can add the flash mode to it now, and we can also add an
  option for the user to choose different firmware versions and or even a
  local build of the firmware." Of the three alternates (card 07, an
  options sheet; card 08, rows inside Details; card 09, options at the
  gate) Paul chose A the same day ("Go with alternate A, build it"), with
  the set's answers to card 06's questions: a choice lasts one write on one
  board and the mode outlives it only through the board's ELF (Q1); the
  pre-releases behind a switch in the list, hidden again each visit (Q2);
  every release with a merged image back to v0.5.0, which is DIO only and
  says so (Q3); the Erase group in the sheet as drawn, the install gate
  keeping its own box (Q4); a release newer than the pin offered as an
  update this player has not been tried with (Q5); the restart-loop offer
  the card's primary (Q6); merged local images only, vetted by their app
  description (Q7); no write speed and no port by hand (Q8); `--flash-mode
  qio|dio` and `device releases` (Q9); no firmware work (Q10); the chosen
  radio in the accent, as `docs/ui-kit.md` says, not the Auto DJ room's
  green (Q11). Clauses 33–43 are new; 12, 14, 15, 18, 23, 28, 31 and 32
  gain the sheet's keys, its pointer, its footer, the *Advanced…* link and
  the Next write's primaries, the Written row, the gate's new lines, Update
  all's modes and left-out boards, and the flags as a next write. Card
  06's finding is fixed: `--release` and `--firmware` no longer move the
  verdict, the tab marks or the count.
  **Grafts**, all taken: from card 09 the restart-loop card (gold *✗ Keeps
  restarting*, two dim lines in a listener's words, Details opened on the
  banners, *Write the DIO image ▸* the primary, straight into the gate with
  DIO chosen — A's pre-filled next write underneath, so Reset undoes it),
  the gate title's mode suffix (*Update from v0.7.0 to v0.8.0 · DIO?*,
  *Write v0.8.0 · DIO?*) in place of A's "in DIO", the one-sentence DIO
  line, and a read-only Written row in Details; from card 08 a built-in
  table of every release's QIO/DIO ELF pair, v0.5.0–v0.8.0, so a board
  written elsewhere still says its mode and v0.5.0's DIO is no choice; from
  cards 07 and 08 each release's SHA256SUMS kept beside its cached image,
  so a release downloaded once writes offline.
  **Where the build parts from the frames.** The release list gives
  GitHub's dates as they come (`2026-10-07`) where the frames wrote "7
  Oct": a month name is ten more strings to translate, and the list is a
  technical place. The help under a picked release does not say the
  image's size ("2.4 MB"): the release list does not carry it, and a
  second request for it would break the one-request rule. The path field
  completes on Tab to the longest common start of what is on disk and
  shows no suggestion list under it: the sheet has no rows to give one,
  and a typed path in a firmware sheet is a paste more often than a walk.
  Tab completion reads the one folder on the key's own turn, as the
  Torrents room's seed path does. Details' Written row has no clock time
  (the log has the minute); its *This player* fact stays the pin's image,
  which is what "carries" means (clause 18). The loop chip is A's (*✗
  Keeps restarting · v0.8.0 in QIO*) with card 09's two lines under it, the
  count of restarts folded into the first. Card 07's frame 29 (*Ask COM7 as
  a Core2*) is not built (Q8). A local build over other firmware is still
  the install gate, its erase box on, with the local build's line under
  its title. Where two boards or more need an update but fewer than two
  would be written, the row's right edge says how many need one (a new
  string) rather than offer *Update all* for one board.
  **Found while building it.** The restart loop's two lines are one
  sentence pair that wraps to two rows of the value column ("Restarted 3
  times in 6 s: its flash may not run QIO. DIO starts on every Core2, and
  the settings stay."): card 09's longer words took four rows, which the
  console's card cannot spare. The Next write line's Reset sits on the
  first of its lines with room for it — at the console's 50 value cells
  "Next write: v0.8.0 in DIO (runs on every Core2)" leaves none, and Reset
  takes a row of its own — and a download's percent is a dim line of its
  own, so no "·" ends a wrapped line. The card's text buttons now stop
  short of the primary, where before they ran under it in a long locale.
  The release list's rows over the sheet are blanked to the sheet's edge,
  and a wide character left of its border is dropped, so a Japanese
  choice never breaks the frame. Choosing *Another release* with ↓ opens
  its list (card 07's rule), so ↓ ↓ from the pin stops in the list: Esc,
  then ↓, reaches *A local build*.
- **2026-10-10 — Revised after review.** `--release <tag>` with no
  `--flash-mode` took the release's QIO build on every board, a board whose
  ELF says DIO included — and `--release v0.8.0` put QIO back on a DIO
  board the pin alone would have kept on DIO — against clause 43. The
  flags' choice now follows each board's mode once its listen has read the
  ELF, where the release has that build; `--flash-mode` named is obeyed as
  named. And the gate's `y` now names its image to the worker (clause 39):
  a choice or a Reset still on its way when the gate was drawn — the
  worker can be a scan of the ports behind — could otherwise write an
  image the gate never showed.
- **2026-10-10 — A board starting up is not a silent one; the mode rows
  fit.** The real-board smoke (Windows, COM3, a 31.9 GB card with 19,410
  tracks) found three things. After the page's own write (v0.8.0 DIO →
  v0.7.0 DIO, and v0.7.0 → a local v0.8.0-5-g4e94418) the page heard the
  boot line, sent `@status` once, gave up after 12 s — "COM3 said nothing
  — other firmware, or none running" — and left the SD card row "?
  unknown until the firmware runs again" until `r`; the board answered a
  little later. Opening the tab while a v0.7.0 board still listed its
  library drew "Unknown board · ? Not answering · may not be an MP3
  player" with *Read the board ▸* and no *Advanced…*, though it printed
  `[bt]` and Bluetooth lines all along. And the sheet's DIO row ran past
  its width at 100×30: "(•) DIO — runs on every Core2, a little slower,
  as COM3 runs now (t…". The firmware serves its console only from its
  main loop, which waits for the library's listing: a minute and a half at
  every start on v0.7.0 with a big card. So a board that answers nothing
  but prints its boot line, or two lines of its own log, is now *starting
  up* (clauses 5, 19): "M5Stack Core2" with **? Starting up**, no primary,
  listened to on the port it holds, asked again every 30 s, for 3 minutes
  at most — and so is
  the board after the page's own write, once its boot line is heard
  (clause 24). One rule, `listen::sign`, decides it, conservatively: the
  Arduino core's `[ 11267][W][…]` lines never count, one line of ours is
  not enough, and a board that prints nothing is today's silent board after
  its first second. A board that prints lines but none of ours is listened
  to for 8 s before it is called not answering, since our firmware's
  Bluetooth task prints only every few seconds. Not tried on the real board
  yet: whether the queued `@status` lines (one every 30 s, at most six,
  well inside the board's serial buffer) are read through once the loop
  runs, which is what the fake does.
  Still open: a v0.5.0 board (no host lines) that prints its log at rest
  now reads as starting up for 3 minutes before *Read the board ▸*; its
  boot line, when heard, still names it at once. The sheet's mode rows
  carry one mark each, and DIO's meaning is its short one beside a mark
  (clause 34); the every-locale floor test now checks every radio row of
  the sheet is drawn whole, which also found the local build's row cut in
  seven locales, French *Another release* and Japanese "not this image's",
  all shortened.
- **2026-10-12 — The pin moves to v0.10.0, which answers the status
  query.** Firmware v0.9.0 (11 Oct) and v0.10.0 (13 Oct) answer `@status`,
  `@count` and `@identify`; the player pins v0.10.0, and
  `PINNED_ANSWERS_STATUS` is true. No clause is rewritten for it, but what
  they say plays out differently: a board on the pin reports its SD card
  (clause 20) — the row, the count (clause 21) and *Show on the player*
  (clause 27) are there after an update, where a board just written with
  v0.8.0 said "? v0.8.0 can't report the card."; an older board's row adds
  "Updating shows it.", a promise the update now keeps; and a board that
  answers `@err 7` with no version in time is judged behind the pin and
  offered *Update ▸* (clause 19) rather than left unrecognised — older
  firmware is behind any pin that answers. The ELF table (clause 37)
  gains v0.9.0's and v0.10.0's pairs. v0.10.0 is the first release whose
  minor has two digits; versions were always compared as numbers, and the
  order is now tested across that boundary. The clauses' examples keep
  v0.8.0 as the pin: the design cards drew them so, and the page's tests
  compare the cards' frames character for character against a stand-in
  pin of v0.8.0 that does not answer the status query
  (`page::fixtures::pin`), told to the page as the worker tells it its
  target — read "v0.8.0" in them as "the pin". So the release list
  (clause 35) now leaves out, and marks newer than, the pin the page was
  told, as the line under a picked release always did, rather than the
  build's constant; in a real run the two are the same release.
- **2026-10-18 — The pin moves to v0.11.0, which asks the transfer for an
  Auto DJ index.** Firmware v0.11.0 (17 Oct) is the pin; both checksums
  are its SHA256SUMS's and GitHub's digests of the assets, and the ELF
  table (clause 37) gains its pair (QIO `1b1b71a1`, DIO `0d07e24d`), from
  its notes and both images' own app descriptions. It answers the status
  query as v0.10.0 does (its HostStatus and docs/HOST-STATUS.md are
  unchanged), so `PINNED_ANSWERS_STATUS` stays true and no clause is
  rewritten; what changes is where boards fall (clause 19): a board on
  v0.10.0 is now *Update available · v0.10.0 → v0.11.0* with *Update ▸*,
  and one on v0.11.0 is up to date with no "Ahead of" line. The reason is
  the card transfer: v0.10.0 writes `read.mpdj=1` in the card's
  `/.player/device.txt` and v0.11.0 `read.mpdj=2`, the sign the transfer
  waits for before it writes an Auto DJ index, so a board written at the
  old pin would never be given one. The clauses' examples keep v0.8.0 as
  the pin, as the 2026-10-12 entry says.

- **2026-10-18/19 — The SD card transfer joins the tab (the twelfth set),
  with the user's picks.**
  - **The decisions** (2026-10-18), on the transfer plan's four questions:
    - Q1: a backup card is a playable mirror by default, removals shown
      first, a gate above 50, a stronger one above half the card, and a
      per-card *never remove*;
    - Q2: one tab row per subject on the MP3 Player tab;
    - Q3: cards are formatted in the Core2, started from the tab,
      `@format` confirmed on the player, the PC never formatting;
    - Q4: yes to a small batch of mStream changes (B10, B11).
  - **The design of record** is the card-transfer design set (twelfth
    set), alternates per the user's picks of 2026-10-19. It draws the Q2
    placement whole, with two alternates inside it. It is in no repo yet:
    it will live in mStream `docs/designs/card-transfer`.
  - **The picks** (2026-10-19), the set's card 07 §7. By name: (1) the
    card's Settings in two tabs, Music and Writing (card 05 alternate A;
    clause 48); (2) the format's second step on the Core2 a 2 s hold of the
    red button, letting go cancelling (card 06 alternate B; clause 59). The
    other ten as the set suggested them: (3) the MP3 Player tab wears the
    sync's percentage (clause 16); (4) a card put in takes the view only
    from the empty card (25); (5) removable volumes the player can't use
    are listed, with *Hide* (63); (6) the card keys `o e p x f h` (12); (7)
    a command-line flag per gate, with the count (65); (8) Ctrl+C during a
    sync stops at the next save (57); (9) one sync at a time (56); (10)
    `device format`, for parity (66); (11) a card's tab on Windows named by
    its drive and name (25); (12) Done's primary *Eject ▸* (54). And two
    the build raised: a computer's `@format` may bring up the two-step
    erase question for any card, the player's own music card included —
    the firmware's refusal of a card it reads (its commit d8e797b) is
    withdrawn, as its 93f84fc reverted it (clause 59); and a file the
    listener changed on the card is kept, the server's copy written beside
    it as "… (2)", as part A8 built it (clause 51).
  - **This supersedes the 2026-10-09/10 decision's "no Format card"** and
    the out-of-scope "Formatting a card". A card the player can't read is
    offered Format on the board's SD row, and one it reads in Details,
    behind the gold gate; the player itself confirms in two steps and
    formats (clauses 59, 60). The reason: only the player cannot format
    the wrong disk (the card is physically in it), it writes exactly what
    FatFs expects and its `device.txt` at once, and it needs no elevation
    on any OS, where Windows 10 can't make FAT32 over 32 GB at all.
  - **Clause 5 gains its one write, the clock**, as the firmware repo's
    `docs/HOST-STATUS.md` has it at 09874f5 and 79a7438: a bare `@time`
    once per listen to a board whose `@status` has `clock=`, one `@time
    <unix-ms>` when the board's clock is unset or more than 2 s off, and a
    board with no `clock=` never asked.
  - **What else changes:**
    - Clause 3 is split: boards stay session-free; a card uses its own
      server's saved sign-in (3a).
    - "A page that outlives the tab" stays out of scope for boards, while
      the transfer worker, which holds no port, outlives the tab (clause
      55).
    - Clause 25 becomes the row of every subject (25, 25a); clause 30
      watches the card readers.
    - The command line gains `device cards`, `device card …` and `device
      format`, with clause-43 parity (clauses 64–67).
    - Clauses 1, 4, 5, 7–12, 14–16, 18–20, 26, 29, 31 and 32 are amended
      (18–20 for the board card's line and its *Format as FAT32 ▸*);
      clauses 3a, 20a, 25a and 44–67 are new.
  - **The numbering.** Clauses 1–43 keep their numbers, so code that cites
    them stays right. A clause split from an old one, or added beside it,
    takes a letter (3a, 20a, 25a); the new sections are numbered on from
    44.
  - **Where the build parts from the frames**, known before it starts. The
    set's card 06 §6 proposed `@format check`, `@format cancel` and a
    partition report for part B5; the firmware built `@format` without
    them (HOST-STATUS, "@format"). So: Format is offered by `@status`'s
    0.12.0 keys, not by a `check` answer; while the player asks, the tab
    draws no Cancel (card 06's frame has one, with `fmt_cancel_note` and
    `hint_fmt_cancel`), and the player's own *Cancel*, its tab bar or its
    minute ends the question; the gate names the card as `@status` reports
    it, where the frame lists its partitions (`gate_fmt_part`); declined
    and timed out are told apart by the firmware's minute, since both
    answer `@format no`. Should the firmware gain those verbs, the frames
    come back as drawn. The player's dialogs say *Cancel* and *Format…*
    in the firmware's own words where the frames say *Keep it*, and its
    step 2's red button is *Format*, held, where the picked frame says
    *Hold to erase*; so the busy line's declined words and the SD row's
    waiting line name no button (`fmt_declined`, and `fmt_waiting_2`'s
    "hold the red button", the player's own hint's words, both reworded in
    the set). The firmware's refusals and failures get words of their own
    with part B6 (clause 59), its refusal at the second answer among them,
    which the set did not foresee. And this computer's clock outside the
    board's range sends no `@time` at all, where the set's notes had the
    board refuse it. What else the build finds is recorded here.
  - **The firmware followed both picks** on its branch, after B5's first
    build (a second tap behind a 0.6 s settle guard, and a refusal of a
    card the player reads, d8e797b): 93f84fc reverted the refusal, and
    76a10dc made step 2 the hold (HOST-STATUS, "@format": *Format* held 2
    s; a lift sooner empties it and shows "Hold the red button for 2
    seconds"; the settle guard kept). Clause 59 follows HOST-STATUS as at
    76a10dc. Not yet tried on a board: the hold on the Core2's touch panel
    (the firmware's device check 11).
  - **The pin can't format** (v0.11.0 has no `@format`), so the line a
    board with no `cid=` and no `clock=` gets is "v0.11.0 can't format a
    card." alone, and "Updating lets it." (`fmt_old_update`, split from the
    set's `fmt_old`, whose frame drew a pin of v0.12.0) follows only while
    the pin answers `@format` (`PINNED_ANSWERS_FORMAT`, part B6), as the
    2026-10-12 entry did for "Updating shows it.": at today's pin an up to
    date board would otherwise be told to update with no Update to give.
    For the same reason, while the pin can't format, a board that can't
    keeps today's fix lines (format it on a computer, under the `_pc`
    keys), where the set's new ones would point below at a Format that
    isn't there; once the pin can, clause 59's line takes the fix line's
    place, as the set draws it (clause 20).
  - **Settled in review** (2026-10-19): during a write `x` on a syncing
    card is one of the keys that work (clause 9, as clause 57 says);
    *Format as FAT32 ▸* is the primary on every verdict with none whose
    board reports its card, *Version not recognised* included, its status
    keys deciding and never its version (clauses 18, 19); `f` gives way
    with `o` on the footer, is not named when Format is Enter's, and works
    on a card the player reads only with Details open, as `w` does
    (clauses 12, 15); `@err 4 format card|size|busy` after `@format ok` is
    a refusal at the player's second answer, ending the wait (clause 59);
    `device format` exits 1 on a failed format too (clause 66); and the
    design of record's header row is provisional until the set is
    committed in mStream.
  - **Words.** 347 keys drawn: the set's 344, the two the picks add and
    `fmt_old_update`; the build adds 344 now, three waiting for firmware
    verbs (Wording). The German drafts (160) and the machine translations
    of the other eight locales wait for a native review.
- **2026-10-19 — Part B4 built: the Card view, the GUI's transfer worker
  and the cards' command line.** What the build parts from, and why:
  - **Settled by the build.** The hidden JSON card commands (`card-probe`,
    `card-setup`, `card-sync`, `card-set`, `card-plays`, `time`) stay
    hidden, as they are, for the real-card checks and for scripts that
    read every field; `device cards` and `device card` are the listener's
    (clauses 64–67), and the help names only them. The tab's gates and the
    command line's flags ask the worker which gate a plan stops at
    (`gate_ahead`, `relay_ahead`), so the two never disagree (clause 67).
  - **Not built yet**, each drawn by the set: the Spread sheet and the
    set's rows and links (*Set up as card N ▸*, *Move the edge*, *Add
    card N*; clause 62: the Set row alone is shown), so the command line
    has no `--set` or `--into-set` either; *Check every file…*; the fit
    line's offers (*Convert everything*, *Leave out*, *Spread over*);
    adding a peer in the sheet (its peers are the card's own); the Player
    row's list (the row says what `device.txt` says, and the command line
    has no `--player`); the empty readers line (the worker lists volumes,
    not empty readers, so `device cards` says "No card in a reader."
    alone); the format's route and its rows (part B6, clause 59: the
    board's fix lines stay under the `_pc` keys until then); and setting
    the clock from the tab (`@time`, part B6).
  - **Drawn otherwise.** *verified* follows a sync of this session only;
    Next sync's look leaves the peers out, and a listing shows "asking
    Home what changed…" with no count of libraries; library sizes are the
    listed libraries'; the codec list always offers MP3 past 192k (a
    server older than 6.32 gets the highest it takes); a card with no
    `device.txt` is synced for the player's first defaults
    (`conv_default`, where the set has `conv_assumed`: the sync reads no
    board); the sheet's tabs hint has no glyph; `log_asked` is two lines
    (`log_asked_same` when the list stood); `check_win` names the drive;
    *Keep* on the another-server gate shows no count of the tracks kept.
    While the card's gate or sheet is up its primary is not drawn under
    it, so the frame stays whole at the floor; at the floor a card's
    dim explanation lines give way first, then Converts.
  - **Quitting.** `q` and the GUI's quit ask (clause 57) and wait in the
    loop for the next save, at most 30 s; the window's close button and
    Cmd-Q quit at once and wait in the teardown; Ctrl+C in the terminal
    GUI waits after the screen is restored, saying so on stderr. On the
    command line Ctrl+C is caught only by `sync`.
  - **The command line's set-up** checks `--adopt` or `--keep-mine` on a
    card with music mStream didn't put there, and says it; the answer is
    the first sync's flag (`sync --adopt`), since `state.json` keeps no
    answer and the sync's adopt gate asks again.
  - **Words.** 358 keys under `dev.card` and five `dev` keys reworded
    (`card_fix_format`, `card_fix_mbr`, `none_title`, `none_body`,
    `none_watching`; today's two fix lines kept as `card_fix_format_pc` and
    `card_fix_mbr_pc`). Beyond the set: `ago_hours`, `dur_s`, `dur_min`,
    `dur_h_min`, `btn_new_id`, `hint_enter_new_id`, `copied`, `copied_2`,
    `conv_default`, `holds_none`, `not_fat_card`, `player_none`, `sync_of`,
    `help_lib_short`, `help_lib_unknown`, `help_budget`, `help_writing`,
    `help_from` and `log_asked_same`. German from the set's drafts; the
    other eight written by the build (Translation notes) — all nine wait
    for a native review. Not tried on hardware: ejecting a real card (the
    OS's eject, part V1), and a card's sync under the GUI end to end.
- **2026-10-19 — Part B4's review fixes.** The review of B4 found three
  writes before their gate and a handful of smaller gaps; fixed as follows.
  - **Nothing is written before its gate.** Settings' From set to another
    server opens the another-server gate before anything is sent: *Make
    Studio its server* writes the settings (Apply, which never syncs:
    clause 48) and the next *Sync ▸* takes the card over; *◂ Keep Home*
    writes nothing (clause 58). `device card <drive> sync --server S
    --take-over` prints the take-over's plan from a dry run (the sheet's
    fit, which plans another server as its take-over runs), and writes
    `state.json` only after `--yes` or "go on? y" (clause 65). A card's
    Next sync, and the sheet's fit line, plan a chosen server the same way.
  - **Eject ▸ on a card the player can't read** (exFAT, NTFS, GPT,
    write-protected) now asks the OS, as the route to the player's Format
    needs (clause 45): the eject asks of any removable volume the tab
    lists, never the computer's own disks or a fixed disk; `e` works where
    Enter's *Eject ▸* does, and `device card <drive> eject` too.
  - **The gates' keeps that answer.** *◂ Leave them all as mine* syncs
    adopting nothing, as `--keep-mine` does (the set's card 06 note:
    "Leave them all as mine still sets the card up"); *◂ Leave Lisbon out*
    is kept on this computer, in `cards.json` (`leftOut`, by card id), so
    that peer is left out of the card's syncs here and never asked again
    (clause 58: once per peer); the yes was already kept on the card
    (`state.json`'s `proxyCopies`). The command line has no flag for that
    no yet: `--through-parent` remains its only answer.
  - **The removals gates name their count.** A yes (the gate's, or
    `--allow-removals N`) carries its count into the run, and a plan that
    removes another number asks again, so a server that changed since the
    look never takes more than was allowed. The stronger gate counts as
    the run's own gate does, its paths (every file deleted or replaced in
    place), each library's share read off the card's ledger by the look as
    by the run; the command line's check of N against the plan's removals
    is made only where no removals gate counted it.
  - **Quitting during a sync** tells the worker to quit: the running sync
    stops at its next save, and one waiting its turn never starts in the
    wait (clause 57). The end of one run is told before the next starts.
  - **Smaller.** A card pulled and put back in its reader while its sync
    lists or plans keeps its tab, its progress and its quit gate; `p`
    during a sync does nothing (the sync sends the plays); a card taken
    out with its view open moves the view right, else left, as a board
    unplugged does (clause 29); a paused peer's budget is its own, not the
    default; a lapsed sign-in is the API's 401, never a word in an error's
    text (a 403 never asks to sign in again).
  - **Words.** `stopping` loses its count ("stopping at the next save…",
    where the set has "… 37 files to it"): the run fetches one file at a
    time, so only that file finishes before the save, and the count was
    never better than "1 files". `newer` names the card format a newer
    writer left, read off its file; a newer private file or plan names
    none, so `newer_any` says the rest of the sentence alone. Both in all
    ten locales, the nine others waiting for the native review.

- **2026-10-19 — Part B6 built: formatting from the tab, and `device
  format`.** Clause 59 as written, over the firmware's `@format` as built
  (HOST-STATUS, "@format", at its d8855d7: the question for any card, the
  second step a 2 s hold). What the build settled, and where it parts:
  - **The protocol.** `@format` goes on the port a listen opens (DTR and
    RTS low: no reset), as a job of the desk like a count
    (`listen::format`, `desk::Cmd::Format`). Its first answer waits the
    reply's 1.5 s; the listener's, 150 s (two dialogs of a minute and a
    margin); each line after `@format 0`, a count's 60 s, the board's
    restart among them. An `@format no` that came 59 s or more after
    `@format ok` is the question's minute running out (`fmt_timeout`),
    sooner a Cancel (`fmt_declined`): a Cancel in the minute's last second,
    or a step 2 left to its own minute after step 1 was answered late,
    reads by the clock alone. An `@err 4 format` is told apart by when it
    came: before `@format ok` (not asked), after it with no restart (the
    second answer's refusal), after `@format 0` (a failure); `@err 7
    format` is firmware with no format, and the SD row says so from then
    on.
  - **While it runs** the board's work is the format's: no write, count,
    read or second `@format` starts on it (the desk refuses a write too,
    which would otherwise have stopped the job), and nothing locks the tab.
    Leaving the tab or quitting lets the job go and the board goes on by
    itself; the next visit asks it again. The board is asked again after
    the format ends, after a refusal at the second answer and after a
    failure (whose card is unknown until it answers), never after a
    Cancel. Done shows the card the done line says (its size, free, no
    tracks, `cid=-`) until that listen says it again, and "Formatted just
    now: …" (`fmt_done`) in place of the board's card line while the card
    is empty and unfilled.
  - **The time** is estimated at 1 MB/s for the gap and both FATs, plus
    15 s for the restart (`board::format_secs`: 34 s for a "64 GB" card,
    263 s for "1 TB", HOST-STATUS's layouts at the slow end), in the gate
    and on the row until the percent's rate says better. The device check
    measures it.
  - **What the gate names as lost.** `@status` reports no partitions, so
    a GPT or damaged card's volumes can't be named: the gate says "what
    the player can't read" (`gate_fmt_unread`, new).
  - **Drawn beyond the frames.** On firmware that can't format, a damaged
    card's row says so too ("v0.11.0 can't format a card." under "Try it
    in a computer."), where its Format would be offered. The player's own
    card's link in Details carries "erases the whole card", as the damaged
    card's does.
  - **Words.** 19 new `dev.card` keys: the refusals at once
    (`fmt_refused_ui|screen|viz|busy|card|other`), at the second answer
    (`fmt_refused_second_card|size|busy`), the failures
    (`fmt_failed_card|size|write|mkfs|check|files|other`), `fmt_no_answer`,
    `fmt_lost` and `gate_fmt_unread`. Each fits the hosted busy line (96
    cells) in every locale, which the floor test now checks for every end
    a format can have. German and the other eight written by the build,
    all nine waiting for the native review.
  - **`device format`** (clause 66) asks the board as `device list` does,
    with no worker and no download, prints the card first and the rest as
    plain lines; the listener's no and the minute go to stdout, the other
    ends to stderr, all exit 1. A board that doesn't answer `@status` is
    refused without being asked. Several boards: the localized title
    `device flash --yes` prints, the boards' lines, then "Pass --port with
    one of them." (flash's own last line names `--all`, which format
    hasn't).
  - **The pin can't format** (`PINNED_ANSWERS_FORMAT` false at v0.11.0, a
    test holding it to the pin), so every board today keeps the `_pc` fix
    lines and "v0.11.0 can't format a card.", and no board shows the
    Format route until it runs 0.12.0.
  - **Not tried on a board:** the whole route on a Core2 (the firmware's
    device check 11, on a spare card): the port staying open across the
    board's restart, the real percent's cadence, the minute's edge.
- **2026-10-19 — Part B6's review fixes.** The review of B6 found no
  format that starts by itself and seven smaller gaps; fixed as follows.
  - **Clause 59 follows the firmware as at d8855d7**, where B6 was built:
    step 2's red button says *Hold to erase*, as the picked frame drew it,
    and its hint is under the body from the start. The decision entry's
    "*Format*, held, where the picked frame says *Hold to erase*" and the
    firmware entry's "as at 76a10dc" were true of 76a10dc; the SD row's
    and the busy line's words already named no button, so none changes.
  - **The busy line fits beside a Mac's port.** The floor test measured
    the format's ends with "COM3" (4 cells), where the page names a port
    by up to 16 (`cu.usbmodem14101`); measured with that, eight lines ran
    over 96 cells, now shortened: German `fmt_asked`, French and
    Portuguese and Russian `fmt_failed_check`, Italian `fmt_failed_write`,
    Japanese `fmt_refused_viz`, `fmt_refused_second_busy` and
    `fmt_failed_size` (the last now ends "書き込みなし。", nothing written,
    in three words). The test now measures every end, and the asked and
    done notes, beside a 16-cell port in every locale.
  - **A refusal at the second answer in a word this player doesn't know**
    says it was not formatted, as clause 59 has it and as `device format`
    already did ("Not formatted: COM3 answered …; nothing was erased.",
    `fmt_refused_second_other`, new), where it said "Couldn't format".
  - **The player's own card while it indexes** (`tracks=building`): the
    gate's gold line names the music ("your music, still being indexed,
    on one FAT32 volume", `gate_fmt_music_building`, new), where it said
    "one FAT32 volume" alone.
  - **A write forgets how the last format ended**, as it forgets the
    count: a board that answered `@err 7 format` and is then written with
    firmware that formats is offered the format on the same visit, and a
    "Formatted just now" doesn't outlive the write (a half-written board
    forgets it too).
  - **`device format` refuses an empty slot** before asking ("Not asked:
    COM3: no card answers in its slot.", exit 1), where it printed "no
    card. Formatting erases all of it…" and then asked, for the board to
    refuse.
  - **A test's timing.** The listen test of a no past the minute kept a
    margin of about 0.1 s against the scheduler; it now waits 1 s past a
    minute of 0.1 s.
  - **Words.** The two new keys in all ten locales, and the eight
    shortened lines: the nine languages other than English wait for the
    native review with the rest of B6's.
- **2026-10-19 — Part B13's terminal half built: the clock from the
  tab; and the proxy's purpose in mStream's own words.** Clause 5's one
  write, over the firmware's `@time` as built (HOST-STATUS, "@time", at
  09874f5 and 79a7438). What the build settled, and where it parts:
  - **Where it is asked.** In the listen job itself, after its status, on
    the port the listen holds (`desk::Desk::listen`, `listen::clock`): one
    bare `@time`, one `@time <unix-ms>` when due, and the bare `@time`
    again to read it back. A listen is a job of its own, so no `@time` goes
    out during the board's write, read, count or format; a listen told to
    stop partway (leaving the tab, quitting) claims nothing. The worker
    takes this computer's clock as `desk::Setup::clock`: the page's
    (`wall_ms`) for the tab and `device flash`; none for `device flash
    --yes`, whose printed steps (clause 32) stay as they were, a script's
    lines.
  - **Details' *Clock* fact** is the last set of the visit (*set by this
    computer at 14:03 (it was unset)*, or *(it was 3.2 s off)*), kept for
    the visit while the board stays plugged in; a clock found in step adds
    no fact. Its minute is `firmware::clock_at`'s, as the release list's
    *asked at* is: this computer's zone where it names one, else UTC.
  - **More gold lines than the clause named.** The clause names `@err 5
    time`, `@err 4 time rtc` and this computer's clock; a board that
    doesn't answer `@time` (`log_clock_silent`) and any other answer or a
    port that fails partway (`log_clock_failed`, the answer word for word)
    are said in gold too. `@err 7 time` from a board that said `clock=` is
    left as it is, as the clause has it, with no line past the host lines.
    5 new keys (Wording); the off is said to a tenth of a second with a
    point in every language, as the card's rates are.
  - **The proxy's purpose.** mStream's batch now says in `GET /api/` that
    its peer stream proxy forwards `X-mStream-Purpose`
    (`features.federationProxy: { purpose: true }`, the key left out,
    never `purpose: false`, when it doesn't). The transfer reads that key
    (`sync::peers::proxy_says_copies`), where wave 3 inferred it from the
    batch's `instanceId`. The id still stands for it on the batch's own
    builds from before the key, which say 6.31.1, the version the batch
    was cut from; on a later server the id with no key is a proxy that
    drops the header, and a peer only it reaches is asked about (clause
    58's gate), as behind an older parent.
  - **Words.** The five keys in all ten locales; the nine languages other
    than English were written by the build and wait for the native
    review.
  - **Not tried on a board:** the set on a 0.12.0 Core2, its log line and
    the fact (the firmware's device check 10); nor the proxy's key against
    a running mStream of the batch.
- **2026-10-19 — Part B13's review fixes.** The review of B13's terminal
  half found no reset, no `@time` outside a listen and no old board or
  old server misread, and three smaller gaps; fixed as follows. The entry
  above's "the bare `@time` again to read it back" and "claims nothing"
  were true of 36120c8.
  - **One bare `@time` per listen, as clause 5 says.** The page's listen
    no longer reads the clock back after a set (`listen::ClockTo::Set`):
    one bare `@time`, then one `@time <unix-ms>` when due. The firmware
    reads the chip back itself and answers `@err 4 time rtc` when it
    didn't take, so the read-back told the page nothing it used. The
    hidden `device time` still reads it again (`ClockTo::SetAndCheck`, its
    `after`), for the device checks.
  - **A stop drops only what it cut short.** A listen told to stop (the
    tab left, a write asked for) sends no `@time <unix-ms>`; a set the
    board answered `@time ok` just before the stop is still said and kept
    for Details' *Clock* fact, and a refusal it gave is still said
    (`desk::listen_clock`), where the whole outcome was dropped. A silence
    the stop cut short, or a port that failed meanwhile, is said by none,
    as before.
  - **`desk::format_end` has its doc comment back:** `clock_line` had
    been put between the two. Code only.
- **2026-10-19 — The terminal gate of wave 4.** The whole suite, the
  desktop flavour's check and a debug build, then the tab's states of
  the SD card transfer drawn whole by the GUI in English and German at
  100×24 and 100×30, for a look. Fixed:
  - **Details' *Drive* fact** (clause 44) was English in every language
    ("FAT32 on MBR · 63.8 GB · 32 KB clusters") while its translation sat
    unused. It is said a piece at a time in the locale's words, since a
    card may lack its size, its clusters or its serial: the head's
    `dev.card.head_on`, a new `dev.card.fact_drive_kb` (each locale's own
    words from the unused key, which went) and `dev.card_size`.
  - **Sizes in the locale's unit.** The head of a card the player can't
    use and the Settings sheet's card and library sizes said "GB" in
    every language; they are `dev.card_size` now, as the Space row's
    figures were (Go in French, ГБ in Russian).

  Found, not changed (words for the native review, or a choice):
  - **The Settings sheet's help line** keeps to two lines and an ellipsis
    (clause 48). In its 70 cells the help of the focused row runs past
    two lines in eight of the nine other languages (all but Chinese): of
    Player in all eight, of Name in six (de, es, fr, ja, pt, ru), of
    Removals in five (de, es, it, ja, pl), of Into in French. German's
    Name help, drawn, loses its last word, "Benutzername"; the other
    languages were measured by a script wrapping by words in cells.
    Shorter words, or a third line where the area has room, would keep
    them whole.
  - **Details' facts are a line each, cut with an ellipsis:** in German
    the *Card* fact loses its version ("mStream Player 0…"), the *Player*
    fact "Wiedergabeprotokoll" and *Removes* "weitere"; in French the
    *Drive* fact loses its serial.
  - **French names two facts *Lecteur*:** the drive (`fact_drive`) and the
    player (`row_player`).
- **2026-10-20 — The wave-4 gate's findings fixed.** Measured with the
  longest values a card realistically shows: a server named by 28 cells
  (`https://192.168.100.200:3000`, one word a line can't break, and Quick
  Connect's `quick connect · 0123456789ab`), a library of 23 cells and
  six digits of tracks; a 1 TB card (1000.2 GB) in 64 KB clusters on
  `E:` or under a FAT32 label's 11 cells, its revision in four digits,
  1,234 plays to send, 99 undated and 1,234 of your own, and firmware
  0.12.0 listing four codecs.
  - **The help line (clause 48)** keeps to its two lines in every locale
    at the GUI's floor (70 cells) and on the console's sheet (64), so the
    ellipsis is left for a longer name still. Shorter words for
    `help_name`, `help_removals`, `help_into` and `help_player` in de, es,
    fr, it, ja, pl, pt and ru. English changed only where it was cut
    itself: `help_player` on the console's sheet, `help_removals` past 26
    cells of server name (14 on the console) and `help_into` past 19 on
    the console. French ties its colons and semicolons to the word before
    with a no-break space, and Russian its dash, so no line starts with
    one.
  - **Details' facts (clause 44)** are whole in the 58-cell column.
    Shorter words where a fact ran over: `fact_drive_kb` (es, fr, it, ja,
    pt), `fact_card_v` (de, es, fr, it, ja), `fact_plays_v` (de, es, fr,
    it, pl), `fact_player_v` (de, es, fr, it, pl, pt, ru) and
    `fact_signed_in` (de, es, it, ja, pl, pt, ru). English's Card fact says
    "rev." (a four-digit revision ran it to 59 cells) and its Plays fact
    "undated" for the plays made before the clock was set (four-digit
    counts ran it to 63).
  - **Two rules for the values no wording can hold,** the smallest change
    found: Removes' path is cut before its count, and a volume's long name
    takes the Drive fact's serial, then its cluster size, each whole
    (clause 44). The line used to be cut at its end, which took German's
    "weitere" and French's serial.
  - **French's two *Lecteur*:** the drive's fact is *Volume* now; the
    player keeps *Lecteur*.
  - **The card under its gate or sheet.** The set draws the Card view
    beneath a modal at its height at rest, its corners beside a taller
    modal (card 05's sheets at 100×30, card 06's German gates at the
    floor). The build hid the primary and dropped its three rows with it,
    so the card stood two rows short and its corners rose into the
    modal's text. It keeps its rows now and only the button is not drawn
    (`Row::Actions`' `covered`), which supersedes part B4's entry ("its
    primary is not drawn under it"): a modal shorter than the card still
    shows no half a button, and the floor's frames stay whole.
  - **Not changed: the command line's "GB free" and "KB clusters"** (`device
    card status`; `device format`'s done line). The cards' command-line
    lines are plain English, lines a script reads (clauses 64–67 and the
    Wording section), and those commands never set a locale, so `t!`
    would print the same English; two translated fragments would only
    mix the languages of one line. Speaking the locale on the command line
    would be a change to the contract for all its lines.
  - **Tests:** every locale's help for every row at both widths (no line
    starting with a mark a space sets apart), every locale's facts with
    the values above on a drive letter and on a long volume name, and the
    card's corners where they are at rest under each of its modals, at
    the floor and at 100×30, with no primary drawn.
  - **For the native review:** the 62 rewritten values in the eight
    languages above (de 8, es 9, fr 9, it 9, ja 7, pl 7, pt 7, ru 6),
    among them "6.32+" (es, fr, pl, pt), "ab 6.32" (de) and "dalla 6.32"
    (it) for "6.32 or later", and the shorter words for the revision
    (Rev., rev., rév., 改訂).
  - **Found, not changed:** French `fact_removes_v` says "et 1 autres" for
    one (a plural that bends, against the Translation notes' rule), and
    the wrap may start a Japanese or Chinese line with "。", since it
    breaks those languages at any cell (Japanese Removals help with a
    short server name at 64 cells).
