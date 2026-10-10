# MP3 Player screen

| | |
|---|---|
| **Design of record** | mStream `docs/designs/mp3-tab`, alternate **A · One card** (card 02) and its several-boards frames (card 05), with the grafts the set's README recommends — chosen by Paul on 2026-10-09 ("Go with alternate A, build it"). This repo's page draws it: `src/device/page.rs` over the desk worker (`src/device/desk.rs`, `board.rs`, `listen.rs`), the same page `mstream-player device flash` opens and the GUI's MP3 Player tab hosts whole. The GUI adds only the tab, the way back, and the rules that keep a write from being left and a board from being left in its bootloader. |
| **Server API** | None. No mStream session is involved: each board is a USB serial port, asked over it with no reset (the firmware repo's `docs/HOST-STATUS.md`: `@status`, `@count`, `@identify`, and the console's `L` for firmware older than them), and the firmware written is the release this player pins. |
| **Already in this repo** | The page, live as its own command on the admin hub's terminal session; the GUI's Stats tab, which showed how a hub page lives under the top bar (the stats-screen contract), and the hub's `HostedRoom`, whose per-frame call this screen uses. The kit's gold warning modal (the libraries room's remove gate), its tab row (the Discovery room), its scan widget and its tall buttons. |
| **Status** | Implemented 2026-10-04 (the tenth set's alternate A: the step line, the question, the erase box); **redrawn as alternate A of the eleventh set 2026-10-10** — this document first, then the page, the tab and the command line (Deviations log). |

## Intent

The MP3 player, one tab away from the music: plug the Core2 in, open the
tab, and see at once whether its firmware is up to date and what its SD
card holds — without the board noticing. Every write passes one gold gate
first; everything technical waits behind one Details door. The same page is
`mstream-player device flash`, so a terminal gets it too.

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
   board that says nothing within about a second is *not answering*. The
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
    `r` (ask again), `←` `→` (another board), Esc (the way back). The GUI
    keeps `q` (quit), `K` (back to the Library), `T`, `M`, `0`, `V`, the
    digits and `D`, as on the Stats tab; uppercase stays the GUI's, so the
    page's `d` is no slip of Auto DJ's `D`. With a gate up its keys are
    `y`, `n`, `e`, Enter and Esc (clause 23); the GUI's own still leave the
    tab, which cancels the gate (nothing was touched). Entering the tab
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
    text buttons, Details, the log, the gate's buttons and its erase box),
    the hub's way. With a gate up the card beneath is inert: no hover, no
    click, no tooltip. The top bar's row and the banner's row are the
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
    cells: in a long locale the lesser keys (`s`, `a`, `c`, `w`, `r`) give
    way before the primary, Details and Esc. It outranks the queue panel's
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
      Core2" for a board running mStream firmware, or one whose bootloader
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
      buttons that apply (Show on the player, Ask again), and at the right
      **one primary**, the kit's tall button, that follows the verdict:
      *Update ▸*, *Install ▸*, *Read the board ▸*, *Try again ▸*, or none.
      While another board is written it is the kit's disabled frame (dim,
      no `▸`, a tooltip saying why) under a line saying why (clause 26);
    - **Details** (`d`, `l` kept; it opens by itself when a write fails):
      a dim rule, then the facts one row each — Port (the port, the bridge,
      the serial, and "115200 baud · no reset" for a board only listened to,
      the baud its bootloader answered at once it was read), Board (its
      flash, from `L` asked when Details opens, or from a read), On the
      board (the firmware and its ELF), This player (the version it carries
      and where it came from, or why it could not be had), Card (the kind,
      the exact bytes, where its free space came from) — then the board's
      log in the page's grammar (mm:ss dim; a phase in the accent, a fact in
      the text colour, the host lines and a boot line dim, a failure gold;
      never wrapped; the wheel scrolls it), then, for mStream firmware with
      no write primary, the dim text button **Write v0.8.0 again** (`w`),
      behind the same gate. A failed write shows only the Port fact before
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
    - **! Newer than this player · v0.9.0** (gold), "This player carries
      v0.8.0; the board is fine as it is.": a newer release, or a
      development build based on the pin or past it (the real Core2's
      `v0.8.0-5-g4e94418`). No primary — installing the pin would go back;
      Details' Write again is titled *Go back*.
    - **? Version not recognised · …** (dim): a version the order cannot
      place. No primary.
    - **✗ Not mStream firmware · UIFlow** (gold — never danger red: a fact,
      not a danger) and **✗ Nothing installed** (gold), after a read:
      *Install ▸*, the install gate (its erase box on).
    - **? Not answering · may not be an MP3 player** (dim), "Its USB chip is
      a Core2's, but many ESP32 boards share it." and "Reading it restarts
      the board for a few seconds.": *Read the board ▸* (clause 22).
    - **✗ Not a Core2** (gold), under the label *Board* instead of
      *Firmware*, after a read found another chip or another flash size:
      "A Core2 is an ESP32 with 16 MB of flash. Nothing will be written to
      it." No primary, no SD card row, never Install or Try again.
    - **! In use by another program** (gold): clause 29.
    - **✗ Write failed · the board is half written** (gold), "It cannot
      start until it is written again." — or `· nothing was changed` when
      the write failed before it touched the flash: *Try again ▸*, behind
      the gate again.
    - While the board is asked: the kit's scan widget, **asking the
      player…**, and under it C's line "over USB, without restarting it —
      its music plays on". While it is read: **reading it over its
      bootloader…** and "Its screen is dark for a few seconds; it restarts
      as it was."
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
    - **unknown**, dim, saying why: being asked; during a write or a read
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
    started it: the primary (Update, Install, Try again), Details' Write
    again, and Update all. It is the kit's gold warning modal, 74 cells wide
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
    *Update 2 players to v0.8.0?*. The erase box lives only in the install
    gate, on by default (`e` or a click flips it), its cost on the line
    under it; the install gate also says, in gold, that the page cannot tell
    a Core2 from another ESP32 board with 16 MB of flash. Enter, Esc and `n`
    keep; `y` or a click on the secondary writes; a click elsewhere on the
    page does nothing; a click on another tab of the GUI leaves, which keeps
    the board as it is. The gate is the board's last untouched moment: its
    reset comes after the `y`. Updates never erase; the standalone page's
    `--erase` / `--no-erase` set the install gate's box and an update's
    erase.
24. **Done is the card again.** There is no Done page and no Close. The
    board restarts, the worker listens again (the boot line, then the
    status), and the card comes back with the new verdict (`✓ Up to date ·
    v0.8.0 · just written`) and its card as the firmware reports it; the
    busy line says it in one dim line — "Updated from v0.7.0 just now. The
    SD card was not touched.", after a first install "Installed just now.
    Next: your music in /music on a FAT32 card." The board's own first
    line, the plan and the checksum are in the log.

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
    or more need an update. One gate names every board — port, serial,
    from → to (six at most, then how many more) — and the worker writes
    them one after another, never at once: one board dark at a time. It
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
    every port go. `--firmware` and `--release` pick another image (a file's
    version is known once it is read), `--port` keeps the page to that one
    board, and `--erase` / `--no-erase` set the gate's erase (clause 23).
32. **`--yes` keeps its printed steps.** One board: today's lines — the
    firmware, the board as its bootloader read it, the plan, the percents,
    Done and the board's first line — with no gate (the flags answer it). A
    port another program holds is said on stderr with the serial-monitor
    hint, exit 1. Several boards with no `--port`: a refusal listing each
    board with its verdict and card, exit 1 — unless `--all`, which updates
    every board running an older release of ours, one after another, with
    the steps under each port, stopping at the first failure (exit 1), and
    names the boards it left alone (`updated 2 of 2 · skipped: COM9 (not
    answering)`). `--all` needs `--yes` and refuses `--port`, `--erase` and
    `--firmware`. `device list` asks every board at once with no reset and
    prints today's line, then its verdict and its card; `--ports` prints
    today's lines and opens nothing; exit 0 with boards, 1 with none, 2 when
    the ports cannot be listed.

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

`dev.hint_working_unseen` is the mini player's line while the page writes
out of sight (it wraps, so it has no footer's width to keep);
`dev.letting_go` is the quit's stderr line (clause 11). The no-player
card's hints (`dev.no_device_*`, `dev.ports_seen*`) are today's, unchanged.
The `--yes` lines and `device list` are plain English, like the `plan:`
line: they are lines a script reads.

## Out of scope here

- **A check of GitHub for a release newer than the pin** (the set's Q2):
  the page knows the pin and nothing past it, and touches no network but
  the pinned image's download.
- **Formatting a card** (Q3): the SD card row names the fix and its cost;
  the firmware formats nothing.
- **Bytes of music** (Q5): the firmware reports `music=?`; the card shows
  tracks only.
- **The battery and what is playing** beyond the identity line of several
  boards (Q6).
- **The DIO image** (Q8) for a Core2 that keeps restarting on the pinned
  QIO image: still `--firmware` with the release's `-dio-full.bin`.
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
  its `written` again — or, for Update all, when its end is told; a
  refusal clears it too. *Holds the board* is "told to let go, not yet
  `Released`, and a board was seen": a page that never saw a board holds
  nothing. `Page::release` is the host's one way to let a page go — Esc's
  way, then the worker's reports read again.
- Under test the tab builds its page on channels the test holds both ends
  of: no worker is ever spawned, so no firmware is downloaded and no
  serial port is opened by `cargo test` (a Core2 may well be plugged in).
  The worker's own tests run it on the fake engine (`MSTREAM_DEVICE_FAKE`'s
  grammar: `status:`, `old:`, `silent:`, `chip`, `busy`, `/in=`, `/out=`,
  `/held=`, and any `@status` field), whose trace says what was done to
  each board.
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
