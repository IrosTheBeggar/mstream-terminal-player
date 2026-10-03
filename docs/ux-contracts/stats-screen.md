# Stats screen

| | |
|---|---|
| **Design of record** | This repo's own stats page — `src/admin/stats.rs`, `mstream-player stats`, built from the "Player Stats" canvas: the state line, Overview · Top · Recent, the periods, the forget gate — at commit `492f362` (2026-09-27), hosted whole. The GUI adds only the tab and the way back. |
| **Server API** | The Stats API v2 the page already speaks (`/api/v1/stats/*`, mStream 6.27 and up). Nothing new. |
| **Already in this repo** | The page, live as its own command on the admin hub's terminal session; the GUI's Now Playing screen, which showed how a shared view lives under the top bar (the now-playing contract). |
| **Status** | Implemented 2026-09-27, this document and the code in one commit. |

## Intent

Your listening, one tab away from the music: the same page
`mstream-player stats` opens, on the session the GUI already holds — no
second sign-in, no second terminal — and a Quick Connect tunnel or a
federated peer's parent serves it as it serves the queue.

## Entry points

1. The top bar's **Stats** tab, second after Library (the Now Playing tab
   was hidden 2026-09-27; `0` opens that screen); the **Library** tab
   beside it is the way back.
2. **`T`** from any screen; **`T`** or **Esc** back to the Library; **`M`**
   on to the Admin tab (the admin-screen contract).
3. A nav digit or `D` leaves for that room; `0` for Now Playing.

## States & flows

- **No session**: the tab opens on one dim sentence — "no session —
  connect to a server and its listening log opens here"; Esc, `T` or the
  Library tab leave.
- **A server that cannot be reached right now** (a tunnel down): the
  reach's own reason on that line.
- **The page**: loading on entry — every read of the page's, exactly as
  `mstream-player stats` makes them — then the page as it is: the tabs,
  the period control, the log, the forget gate. A server older than 6.27
  says "no Stats API" the page's way.
- **The session changes** while the screen is up (a switch, a pasted
  code's dial): the page is rebuilt on the new session's server. A switch
  drops the page the moment it goes out, and the screen says there is no
  session until the new server answers.
- **Leaving** drops the page; coming back builds it again and reloads.

## Behavior contract

### The screen

1. **It is the stats page, whole**, drawn under the GUI's top bar in the
   rows the page spends on its own header: no header of its own (the top
   bar names the server), no tips row of its own (the GUI's footer carries
   the page's hint), the note row at the bottom as the page draws it.
   Nothing of the page's behaviour changes: the same tabs, controls,
   periods, gate and words.
2. **The GUI's queue panel and bar stand down**, as on Now Playing. The top
   bar stays, its tabs and its server menu live.
3. **The session's server serves the page**: the client is built from the
   App's reach for the session's origin — a peer session's parent, where
   its plays are reported (play-reporting contract, clause 8) — with the
   session's token, its trust and, over a tunnel, the bridge's loopback
   token. The account is the session's.
4. **Keys**: the page's own keys are the page's (`←` `→` tabs, `↑` `↓`
   rows, `[` `]` periods, `p`, `t`, `m`, `o`, `x`, and inside its modals).
   `q` quits the player, as on every GUI screen. Esc goes to the page
   first — it closes the page's modal or lets go of its row — and with
   nothing left to close, back to the Library. `T` goes back, `M` to the
   Admin tab. `0`, the digits and `D` reach the other screen and the
   rooms as from anywhere, unless a modal of the page's is open, which
   keeps every key.
5. **The pointer** below the top bar is the page's: hover, click, hold,
   drag and the wheel on its own controls, the hub's way. The top bar's
   row is the GUI's, but for the drag and the release of a press that
   began on the page, which stay the page's there (a thumb dragged to the
   top overshoots onto it). A GUI modal (the add-server form, the server
   menu, a torrent dialog) owns the pointer whole while open, as
   elsewhere.
6. **The footer** shows the page's hint line — the keys that work now, in
   the page's order — with "Esc library" after it while nothing on the
   page is selected or open.
7. **The tab wears the slab** while the screen is up, as the other tabs do.

### The charts

The page's two column charts — plays per day (or week, or month) and the
24-hour profile — in both hosts, the hub's terminal and this screen.

8. **Every column chart reads on a scale.** Down its left, the whole-number
   ticks: `┤` on a row whose upper edge is a whole number of plays, that
   number before it; `│` on the rows between; `0 ┼` at the foot. Once the
   tallest column reaches the rows' count, every row is the same whole
   number of plays and every row is labelled (4 3 2 1; 40 30 20 10 for a
   37-play peak, which then stands at 37/40 of the height). Below that the
   tallest column takes the whole height and only the rows whose edge
   falls on a whole number are labelled.
9. **A baseline under the columns**: `─`, with `┴` under every column that
   carries a label, and the labels on the row beneath, each starting under
   its tick — the days, the Mondays, the months, the hours, as before. A
   label with no room is left out, and its tick with it.
10. **The charts grow with the screen.** The two share the rows left under
    the tiles equally: three rows of eighth-block columns on a short
    terminal, eight at the most, never more. When only one chart fits, the
    day chart is drawn and the hours chart yields, as before.
11. **No gridlines.** The ticks carry the scale; the page stays quiet.
    Every axis glyph is CP437, so the legacy console draws the same axis
    around its density columns.
12. **The split beside the hours chart keeps its words.** Its share bar is
    ten cells, fewer when the row beside the chart is short of them — the
    axis's gutter costs that row its cells — and never fewer than four;
    with less room than that the split is not drawn, as before.

### The tiles

13. **The six tiles are cards**: the kit's rounded frame, dim, no fill,
    one cell of padding; inside, the value bold, the label and the detail
    dim, as before. Three across, filling the column with one cell between
    (the width's remainder goes to the first cards), the two rows stacked
    frame to frame, the top row's frame directly under the period
    controls. The frames cost the rows under the tiles one row against the
    bare tiles; the charts yield as clause 10 says.

### The period

14. **The period is a dropdown.** On the Overview's controls row, after
    PERIOD, the control is the period's name bold with a dim `▾` (`v` on
    the legacy console), bright under the pointer: `This month ▾`. A click
    on it or `p` drops the list under it — a rounded frame in the accent,
    hanging from the name's first cell, kept inside the column — with
    every period the log has data in, then All time, each with its dates
    where it has them; the current one wears the `•`, the cursor the slab,
    and the first play's date closes the list. `↑` `↓` and the wheel move
    the cursor, Enter picks it, a click on a row picks that row; Esc, `p`
    or a click anywhere else closes the list and keeps the period. `[` and
    `]` still step the period without the list. On the Top and Recent
    tabs, which have no controls row, the same control leads the tab
    row's note at the right — `This month ▾ · the log, newest first` — so
    every tab names the range it shows and can change it; the list drops
    from there.
15. **No state line.** The tabs are the body's first row; the row that
    carried "• This month — 388 plays · 31h 12m · 188 tracks · times in
    your zone" is gone, its facts being the tiles' own, and the Overview's
    tab-row note ("the period's totals") with it. What the line said that
    nothing else did moved: "no Stats API" and "no plays yet" are one gold
    sentence at the top of the body; "times in UTC" ends the Recent tab's
    note on a machine without a zone, and is not said when the zone is
    known. The two rows go to the charts (clause 10).
16. **A change keeps the page.** A new period or origin sends the reads
    and leaves everything drawn as it is — the tiles, the charts, the
    ranking, the log — under the new name, with the note row saying the
    numbers are on their way; the new ones replace the old when they
    land, and nothing blinks. Until then the tiles compare against, and
    an empty period is named by, the period the numbers belong to, not
    the one chosen. The row cursor lets go and the log's paging stops at
    what is loaded, since those rows are about to change.

## Wording

The page's own strings (`sta.*`). New: the tab is the page's title
(`sta.title`); `gui.tips.stats_back` "Esc library"; `gui.stats.no_session`
the no-session sentence; `gui.tips.base` names `T` (and `M`, since
2026-10-02).

## Deviations log

- **2026-09-27 — Extracted and implemented.** The one alternative weighed
  was opening the page in a new terminal window (`mstream-player stats
  --server …`) and it was refused: a second process cannot ride the GUI's
  Quick Connect bridge (the loopback token is the worker's) or a proxied
  peer, it needs a token already saved for the server, and opening a
  terminal window is a per-OS guess with no answer over SSH. Hosting the
  page in the GUI's own loop has none of those, and costs the page one
  flag: no header and no tips row while hosted.
- **2026-09-27 — The charts' furniture (clauses 8–11).** The two column
  charts had labels under them and nothing else: no scale, no baseline, no
  ticks, so nothing on the page said what a full column was worth. The
  terminal-charting libraries surveyed — asciigraph and its Rust port,
  ntcharts, ratatui's Chart and BarChart, termgraph, plotext — agree on
  the furniture in clauses 8 and 9. Ratatui's own Chart was weighed and
  refused: its bars are canvas markers with four levels a row against the
  eighth blocks' eight, its labels spread evenly rather than sitting under
  their dates, and the Windows console has no braille. Gridlines were
  offered and declined. The charts' growth and its cap were the user's
  call; the floor fell from four rows to three so that a 30-row terminal
  keeps both charts and a 24-row terminal keeps one, each now a row
  taller with its baseline.
- **2026-09-27 — Cards (clause 13).** Asked for once the axes were in:
  borders on the six tiles. Drawn as the kit's rounded cards, each its
  own frame, rather than as a shared-border grid, which would have cost
  no row: the grid reads as a table, the cards as the webapp's tiles. The
  row the frames cost comes out of the charts' share — a 30-row hub
  terminal keeps the day chart alone, a 24-row one the tiles alone (both
  undone by clause 15's two rows: at 30 both charts, at 24 the day chart).
- **2026-09-27 — The period dropdown, the state line, the blink (clauses
  14–16).** Asked for as four things: the state line was redundant with
  the tiles; the Overview's "the period's totals" note said nothing; the
  `‹ This month › [ ] step · p list` control should be a dropdown; and a
  period change made the page blink. The dropdown follows the GUI
  header's server switcher (the kit's one anchored list: `name ▾`, a
  frame under the name, rows that pick on click, a click elsewhere that
  closes) rather than the centred picker the page had, and lost the
  picker's title bar and `[X]` with it. Because the state line was the
  only place the Top and Recent tabs named their period, the control
  leads their tab-row notes — an addition beyond the ask, made so no tab
  shows a range it does not name. "times in your zone" was dropped rather
  than moved: the zone being known is the expectation; only UTC is worth
  a word. The blink was the page emptying itself on a change so the state
  line could not name one period over another's totals; with the line
  gone the page keeps its numbers and tracks which period they belong to.
- **2026-10-02 — The server menu over the page (clause 5).** The header's
  server menu could not be clicked while the page was up: the page took
  every press below the top bar unless a GUI modal was open, and the menu
  is not one of the GUI's modals, so its rows fell through to the page.
  The page now steps aside while the menu is open, as clause 5 always
  said. Found while the Admin tab was built on the same seam; `M`, the
  Admin tab's key, joined the page's way out the same day.
- **2026-10-02 — A release on the top bar (clause 5).** A press that began
  on the page and was let go on the top bar's row never reached it, so a
  held arrow went on stepping and a thumb kept following until the next
  click. The drag and the release of such a press are the page's now, as
  the Admin tab's rooms' are.
