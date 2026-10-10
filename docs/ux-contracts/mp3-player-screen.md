# MP3 Player screen

| | |
|---|---|
| **Design of record** | mStream `docs/designs/mp3-tab`, alternate **A · One card** (card 02) and its several-boards frames (card 05), with the grafts the set's README recommends — chosen by Paul on 2026-10-09 ("Go with alternate A, build it"); and for the Advanced options, alternate **A · An options sheet** (card 07) with the grafts of cards 08 and 09 the set's README recommends — chosen by Paul on 2026-10-10 ("Go with alternate A, build it"). This repo's page draws it: `src/device/page.rs` over the desk worker (`src/device/desk.rs`, `board.rs`, `listen.rs`), the same page `mstream-player device flash` opens and the GUI's MP3 Player tab hosts whole. The GUI adds only the tab, the way back, and the rules that keep a write from being left and a board from being left in its bootloader. |
| **Server API** | None. No mStream session is involved: each board is a USB serial port, asked over it with no reset (the firmware repo's `docs/HOST-STATUS.md`: `@status`, `@count`, `@identify`, and the console's `L` for firmware older than them), and the firmware written is the release this player pins — or, for one write, what the Advanced options sheet chose: the pin's other build, another release (GitHub's releases API, asked only when the sheet lists them), or a local build. |
| **Already in this repo** | The page, live as its own command on the admin hub's terminal session; the GUI's Stats tab, which showed how a hub page lives under the top bar (the stats-screen contract), and the hub's `HostedRoom`, whose per-frame call this screen uses. The kit's gold warning modal (the libraries room's remove gate), its tab row (the Discovery room), its scan widget and its tall buttons. |
| **Status** | Implemented 2026-10-04 (the tenth set's alternate A: the step line, the question, the erase box); **redrawn as alternate A of the eleventh set 2026-10-10** — this document first, then the page, the tab and the command line (Deviations log); **Advanced options (card 07) 2026-10-10** — the worker and the command line first, then this document's clauses 33–43, then the sheet. |

## Intent

The MP3 player, one tab away from the music: plug the Core2 in, open the
tab, and see at once whether its firmware is up to date and what its SD
card holds — without the board noticing. Every write passes one gold gate
first; everything technical waits behind one Details door, and every
choice about what is written — the flash mode, another release, a local
build — behind one *Advanced…* sheet. The same page is `mstream-player
device flash`, so a terminal gets it too.

## Entry points

1. The top bar's **MP3 Player** tab, fourth after Library, Stats and Admin,
   before the Visualizer item; the **Library** tab is the way back.
2. **`K`** from the Library, Now Playing, Stats and the Admin hallway;
   **`K`** on the tab back to the Library.
3. From the tab, `T` for Stats, `M` for Admin, a nav digit or `D` for that
   room of the Library, `0` for Now Playing, `V` for the visualizer — but
   never while the page writes (clause 9).
4. **`mstream-player device flash`**, on its own (clause 31), and its line
   mode `--yes` (clause 32).

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
- **The session** has nothing to do with the screen: with no server
  configured the tab works the same, and a server switch or a reconnect
  leaves the page alone.

## Behavior contract

### The screen

1. **It is the firmware page, whole** — `mstream-player device flash` with
   no flags: the firmware this player pins, every Core2-shaped port watched,
   the erase decided in the gate. It is drawn under the top bar in the rows
   the page spends on its own header: its first row blank under the bar, no
   header of its own (the top bar is the screen's heading; the firmware's
   version and origin are in Details), no tips row of its own (the GUI's
   footer carries the page's hint, clause 15), the busy and note line on the
   page's last row. The page's worker, keys, buttons and words are the
   command's.
2. **The GUI's queue panel, bar and note row stand down**, as on Stats.
   The top bar stays.
3. **No session is involved.** The tab works with no server saved; a
   server switch, a reconnect or a session that drops does nothing to the
   page, and the page is never rebuilt for one.
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
   page go.
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
   port in use.
9. **The write cannot be left.** From a gate's yes until the board's write
   has ended — written and restarted, or failed — and for Update all until
   the last board's has, the tab holds the GUI:
   - every key is the page's, and only `d` and `l` (Details) and `←` `→`
     (look at another board, clause 26) do anything: `q`, `K`, `T`, `M`,
     `0`, the digits, `D`, `V`, Tab, Esc, Enter and the transport letters
     do nothing;
   - the top bar's clicks do nothing — the tabs, the Visualizer item, the
     server label and `[+]` — and its tabs do not light under the pointer;
     below the floor the mini player's buttons answer nothing either;
   - no screen change from anywhere is honoured, and the page is never
     dropped.
10. **Ctrl+C, the window's close button and, on macOS, Cmd-Q and the app
    menu's (or the Dock's) Quit still quit**, as on every GUI screen and
    as in the standalone page: they are the ways to cut a write short, and
    they leave the board half written (writing it again fixes that: its ROM
    bootloader always answers). The write's hint, on the footer, warns
    against unplugging the board; the mini player's line, while the write
    runs out of sight, names quitting too (clause 4).
11. **Quitting lets the boards go first.** When the player quits (any way:
    `q`, Ctrl+C, the window's close button, Cmd-Q) with a page — the tab's,
    or one kept aside — that has seen a board, the GUI tells its worker to
    let go and waits, reading its reports, until every port is free: a
    moment for listens, the rest of a read and its restart when one is under
    way. The wait is bounded at eight seconds, and in the terminal a wait
    past a second says why on stderr (`dev.letting_go`). With no page, a
    page that never saw a board, or a write under way, the quit does not
    wait. The standalone page does the same on its way out.

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
    page takes no click.
15. **The footer** is always drawn on this tab, whatever the key-hints
    setting — the page's hint is part of the page, the write's warning not
    to unplug the board above all — and the page's area ends above it. It
    shows the page's hint, in its hosted words: where the standalone page
    says "Esc leave", the tab says "Esc library", since that is where Esc
    goes. The hint names the keys that do something on the board in view —
    the primary's verb first after the switch keys — and never runs past 99
    cells: in a long locale the lesser keys give way before the primary,
    Details and Esc — `s`, `a` and `o` first, then `c`, `w`, `r` and `x`.
    With the sheet up the hint is the sheet's (clause 34), its Esc and
    Enter never giving way. It outranks the queue panel's
    and an armed pick's tips; only a GUI modal's own come first, and none
    during a write. While a visit waits for the last page to let its ports
    go, the footer is empty.
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
      v0.8.0 does not); a board that is not mStream firmware ("? Only
      mStream firmware can read the card"); a port in use ("? unknown until
      the port is free").
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
    the worker asks it every 5 s, for 3 minutes at most, until it does:
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

### Several boards

25. **Port tabs.** With two boards or more plugged in, the kit's tab row
    stands above the card: one tab per board in the order the OS lists the
    ports, its short port name (`ttyACM0`, not `/dev/ttyACM0`) and its
    mark — ✓ green, ! and ✗ gold, ? and `…` dim, a write's percentage in
    the accent, `in use` dim — the open one the slab. `←` `→` (wrapping) or
    a click switch; the card is that board's, with its own verdict and
    primary. The row's right edge says the count: "1 of 2 needs an update"
    (gold) where one does, **Update all (2)** (a dim text button, `a`)
    where two or more do, "2 players plugged in" otherwise, or the mix
    ("2 players · 1 other board", "1 player · 1 in use", "asking COM7…");
    during a write "writing COM3…", during Update all "Update all · 2 of
    3". A board counts as a player only once it answers as mStream
    firmware. Down to one board, the row goes and the card is the single
    card again; nothing is reset.
26. **Look, don't write.** During a write the other boards can be switched
    to and looked at; their facts stay as read, and a board plugged in
    meanwhile is asked as usual. Nothing else starts: no second write, no
    read, no count, no Show on the player, no Update all. A board whose
    primary would write wears it disabled, under "One write at a time: this
    one can follow COM3.". The writing board stays in view: its tab carries
    its percentage, the busy line its progress whichever card is open
    ("COM3 · writing v0.8.0… 62% · about 15 s left"), and the footer names
    it ("please wait — unplugging COM3 now would leave it half written").
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
    and holds a port only while it talks to its board.
30. **No player.** With no Core2-shaped port the card is still there,
    empty: "No player plugged in", one line on what to do, and the kit's
    scan widget "watching the USB ports…" — the worker looks every two
    seconds, so there is no Look again and no Close. **▸ Not showing up?**
    (`d`) opens today's hints unchanged (a cable that carries data, M5Stack's
    driver, Linux's dialout group) and the serial ports seen. Ports that
    cannot be listed at all say so in gold.

### The command line

31. **`mstream-player device flash`** is the same page on its own, at
    72×24 at least: its header (the page's title, and the firmware it would
    write at the right), the card (68 cells, the bar 50), its busy line and
    its tips row on the last two rows, Esc leaving once the worker has let
    every port go. `--firmware` and `--release` pick another image and
    `--flash-mode` its build — each board's next write, never what a board
    is measured against (clause 43) — `--port` keeps the page to that one
    board, and `--erase` / `--no-erase` set the gate's erase (clause 23).
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
    with none, 2 when the ports cannot be listed.

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
    (v0.5.0–v0.8.0, bumped with the pin) and the images it has had in hand
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

## Out of scope here

- **A check of GitHub on its own.** The page asks GitHub's release list
  only when the sheet's *Another release* is chosen (clause 35), never to
  tell a board that a newer release exists: up to date is the pin.
- **Formatting a card** (Q3): the SD card row names the fix and its cost;
  the firmware formats nothing.
- **Bytes of music** (Q5): the firmware reports `music=?`; the card shows
  tracks only.
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
- **A page that outlives the tab.** Leaving lets the page go (clause 8):
  it is kept only as long as its worker takes to let its ports go, never
  shown again; the next visit starts over.
- **A quit that waits during a write.** The write's end cannot be waited
  for on the way out (half a minute, and the user asked to quit): clause
  10 says what a quit then does.

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
  prints `[bt]` and Arduino lines — and any `@status` field),
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
  asked every 5 s on the port it holds for 3 minutes at most — and so is
  the board after the page's own write, once its boot line is heard
  (clause 24). One rule, `listen::sign`, decides it, conservatively: the
  Arduino core's `[ 11267][W][…]` lines never count, one line of ours is
  not enough, and a board that prints nothing is today's silent board after
  its first second. A board that prints lines but none of ours is listened
  to for 8 s before it is called not answering, since our firmware's
  Bluetooth task prints only every few seconds. Not tried on the real board
  yet: whether the queued `@status` lines (one every 5 s, at most 36) are
  read through in a burst once the loop runs, which is what the fake does.
  Still open: a v0.5.0 board (no host lines) that prints its log at rest
  now reads as starting up for 3 minutes before *Read the board ▸*; its
  boot line, when heard, still names it at once. The sheet's mode rows
  carry one mark each, and DIO's meaning is its short one beside a mark
  (clause 34); the every-locale floor test now checks every radio row of
  the sheet is drawn whole, which also found the local build's row cut in
  seven locales, French *Another release* and Japanese "not this image's",
  all shortened.
