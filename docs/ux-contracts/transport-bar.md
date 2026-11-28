# The transport bar

| | |
|---|---|
| **Design of record** | The GUI's own, with no record elsewhere in the family (the webapp's and the phone's players are their own): `src/gui/bar.rs` at `eabeeded` (2026-11-26), after the "Player bar options" canvas (A′ for the rows, I for the gold play frame; 2026-08-29, when the waveform seek style was retired). Its egui face (clauses 15–20) is the "Top Bar and Transport in egui" canvas (2026-11-27): Main.dc.html's BAR, SEEK ROW, AUTO-DJ SPLIT, TRANSPORT GROUP, VOLUME, NOTE SLOT and CARD patterns, the TransportStates, TransportParts and NoteBanner boards, and the board plan's decisions D11–D17 and D22–D24; `src/gui/bar/egui_view.rs`, hosted by `src/gui/chrome.rs`. |
| **Server API** | None of its own: its acts are the App's (play, pause, seek, the queue's steps and modes, the volume, the DJ's switch), and the cover is the one the card already fetched for the playing track. |
| **Already in this repo** | All of it: this contract is extracted from the code, its egui face with it. The mini player reuses its card, its seek line and three of its frames (mini-player.md); quick config hangs from its split (auto-dj.md clause 58). |
| **Status** | Extracted and its egui face built 2026-11-28, this document with the chrome's egui face. |

## Intent

What is playing and where it has got to, and every control a listener
reaches for, in the same five rows on every screen that browses: the gold
rule that is the seek line, the transport under it, the volume, a line
for the player's news, and the card that says what the song is and
opens the queue.

## Entry points

1. **The Library and the Admin screen** draw it at their foot. Stats, the
   MP3 Player, Now Playing and the mini player draw no bar (the mini
   player borrows its parts, mini-player.md).
2. **The keys** it names on the tips line: Space play/pause, `p` previous,
   `n` next, `s` shuffle, `r` repeat, `A` the DJ, `-` and `+` (`=`) the
   volume, Tab the queue, `m` the playing track's sheet — the Library's
   last resort, which a room, a layer or a typed query takes first.

## States & flows

- **Playing or paused**: the seek line runs, the play slot wears the pause
  bars or the play mark, the card names the song.
- **Nothing loaded**: the seek line is the gold rule across the row, the
  card says "(nothing playing)", the play slot still wears the pause bars.
- **Quick config up**: its popover rises from the split's rule, the
  split's chevron turned down; a press anywhere off it puts it away.
- **The queue open**: the card's chevron turns down; on the Library the
  queue panel stands over the rooms' right.
- **Key hints on**: the tips line under the bar, which then stands a row
  higher.
- **A quit waiting for a sync's save**: its countdown writes the window's
  last row, over the bar's bottom row while key hints are off.

## Behavior contract

### The rows

1. **Five rows**, from the bar's top: the seek line; three rows of tall
   frames; the bottom row, which holds the volume, the note and the card's
   fourth line. With key hints on the tips line is the row under them.
2. **The seek line** spans the row less a column each side: the elapsed
   time, the track's cells, the total. Both times sit in slots as wide as
   the total ("4:12", a track past a hundred minutes "100:00"), the elapsed
   one at its slot's right; the cells start after its slot. The played
   span is gold (`━`), the playhead a gold `█`, the rest dim (`─`); the span
   always agrees with the times (the playhead's cell is the one played
   through). A press on a cell seeks to that cell's share of the track —
   the first cell its start, the last its end — on the press. Under the
   pointer the cell brightens and the elapsed slot shows the time a press
   there would land at. An unknown length reads "0:00" and plays none of
   the track; its elapsed time past ten minutes is written whole from its
   slot's left.
3. **Idle**, with nothing loaded, the seek line is the gold rule across the
   whole row, the setup wizard's, with no times and nothing to press.
4. **The auto-dj split**, four columns in: one rounded frame round two
   halves and a rule between them. The word's half, ` auto-dj `, starts
   and stops the DJ (`A` the same; with an empty queue and First song =
   Ask it asks the First song question, auto-dj.md clause 2); dim off, the
   ok green and bold while the DJ is armed, the frame and the rule in the
   same colour. The chevron's half opens and closes quick config
   (auto-dj.md clause 58): `▴` while it is down, `▾` and bright while it is
   up. Each half brightens under the pointer on its own; the word's
   brightens the whole frame.
5. **The transport group** — repeat, previous, play/pause, next, shuffle —
   is centred in the span between the split and the card, at any width,
   each a three-row frame with one cell of padding. Play/pause is the one
   primary: a thick gold frame, bold, `▮▮` whenever the player is not
   paused, nothing loaded included, and `▸` while it is, in a slot of one
   width so next never moves. Previous `◂◂` and next `▸▸` in the text
   colour, bold. Repeat `↻` and shuffle `⇄` dim off, the ok green and bold
   on; repeat One wears All's look (the note says which). Under the
   pointer a frame brightens whole. Play/pause with nothing loaded and an
   empty queue does nothing.
6. **The volume** on the bottom row, from the first column: `-`, ten cells,
   `+` and the readout. `-` and `+` step five points, cell *i* sets
   (*i*+1)×10 %, and the readout (`{:3}%`) takes no press. As many cells
   fill (`▰`, the accent; `▱` dim) as the volume rounds to in tenths, so 5 %
   steps fill as the rounding falls. `-` and `+` brighten under the
   pointer; the cells do not.
7. **The note** on the bottom row from column 24, W−70 columns wide (30 at
   100): one line, cut with the kit's mark. The shell's own note outranks
   the App's news; the shell's is dim, its error gold; the App's Info dim,
   Busy in the accent, a landing's Success in the ok green, Plain in the
   text colour (discovery-page clause 118). Now Playing, which has no bar,
   writes its note on a row of its own.
8. **The card**, 44 columns at the right edge, four rows: the cover, eight
   by four cells, the playing track's picture once it is decoded and an
   empty dim frame until then; four lines from two columns past it — the
   title (bold), the artist and the year (`artist · year`, or whichever
   the track has), the spec (`FLAC · 912 kbps · 44.1 kHz`), and the
   rating's stars in gold with the key and the tempo after them (`♪ 8A ·
   120 BPM`); with nothing up, "(nothing playing)" dim, alone. Each line is
   cut with the kit's mark short of the queue's chevron, `▴` closed and
   `▾` open, in the third column from the edge on the first line. The
   whole card is one press: it shows and hides the queue. A right press on
   it while something is up opens the playing track's sheet (`m` the
   same). Under the pointer the title and the byline brighten with the
   chevron.

### The pointer

9. **Presses act on the press**, as every cell of the GUI's does. Hover
   brightens only what a press would work, and the hand shows over every
   live control.
10. **The wheel over the bar** falls through on the Library to the room or
    the queue under that column, and does nothing on the Admin screen.
11. **No tooltips.** The tips line names every key, and a bar is hovered
    too often for tooltips to earn their draw.
12. **Under a layer the bar is inert where the layer says**: a modal's
    guard, a whole-window catcher (the server menu's, quick config's) or a
    sheet's backdrop take the press before it; the Library's modals stash
    the pointer for the screen, and the bar then draws with no hover. On
    the Admin screen a modal leaves the bar's hover lit under it.
13. **During an MP3 write** the screen is held on the MP3 Player tab, so
    no bar is drawn.
14. **The quit's countdown** writes the window's last row while a quit
    waits for a sync's save: with key hints off that is the bar's bottom
    row, its volume, note and the card's fourth line overwritten.

### Drawn by egui in the window

15. **In the desktop window, egui paints the bar** in the Studio look of
    the canvas, in the very cells the cells draw it in: the cells still
    draw and register every control, then blank the bar's rows, and egui
    paints the layout they registered (`BarLayout`), so what it paints is
    what a press works there. Everything about the pointer stays the
    cells' registry's: the presses, the right press, the wheel and the
    hand (clauses 9–12). A control lights only where the registry says
    the pointer is over it, between the bar's marks: none while a chrome
    popup holds the pointer, none where a modal stashed it, none under a
    layer that dropped the registries. The quit's countdown stays the
    cells' to write, and egui's paint is cut round it (clause 14).
    `MSTREAM_WINDOW_CELLS=1`, a terminal and the frames before the window
    exists keep the bar in cells, as clauses 1–14 tell it.
16. **The ground and the seek line**: the panel's fill over all five rows
    with no hairline over them, the seek line the bar's edge. The times
    in the monospace (Hack 12), dim, in their slots; between them a
    2-point track at the row's middle, gold over the played share and the
    strong edge after it, a gold thumb 6 by 12 points at the playhead.
    Where the pointer would land a press, the thumb's outline in the text
    colour, and in the elapsed slot the time it would land at, in the text
    colour — the cells' preview, cell for cell. Idle, a 1-point gold
    hairline across the whole row.
17. **The split and the group**: each control's frame 44 points tall,
    centred in its three rows, as wide as its cells, with the desk's
    corners. The split is one frame and a hairline at the middle of the
    rule's column, in the strong edge or, armed, the ok green; the word's
    half a 6-point LED (unlit, or the ok green) and the word in spaced
    capitals, dim or the ok green, set smaller where a wide face would cut
    it; the chevron's half the kit's chevron, up while quick config is
    down and down while it is up, dim, in the text colour while it is up.
    Each half takes its own raised fill under the pointer, an idle word
    going to the text colour. In the group, prev and next wear the strong
    edge and the text colour; repeat and shuffle the strong edge and a dim
    mark off, the ok green on; play/pause a 2-point gold frame and a gold
    mark. The marks are the kit's transport marks, 14 points from the
    canvas's paths. Under the pointer, the raised fill and a step brighter
    (the strong edge to the dim, the dim to the text colour, the gold to
    the hot accent; the ok green has none).
18. **The volume and the note**: `−` and `+` as the kit's quiet steps, dim,
    in the text colour under the pointer; ten of the kit's level segments,
    6 by 10 points at the head of each of their cells, lit amber as the
    cells fill and unlit beyond, with no look of their own under the
    pointer; the readout in the monospace, dim, at its slot's right. The
    note in its slot as the kit sets a page's note: its words at 13 points,
    its figures in the monospace, in its tone (clause 7, the accent amber).
19. **The card**: the cover 64 points square at the opening size, the
    picture made into a texture once at the size the slot draws it, with
    the desk's corners; until it is decoded, a hairline square in the
    strong edge. The four lines from 16 points past it, each in its row
    and cut with an ellipsis at its cells' end: the title in the 13-point
    semibold, the text colour; the byline at 12.5, dim; the spec in the
    monospace at 11.5, dim; the rating's five stars drawn in gold (filled
    whole, by half or not at all, outlined), centred on the facts'
    capitals, and 8 points after them the facts in the monospace, dim. The
    queue's chevron, the kit's, in its cell. Under the pointer, the raised
    fill behind the whole card and the byline and the chevron in the text
    colour (the title wears it already).
20. **Faces and size**: every piece stands in its cells at any size and
    scale, the UI face the system's (Segoe UI, SF, DejaVu or Noto); the
    words measured in the face in use, the stars and the LEDs set on their
    words' measured capitals. Nothing panics at the GUI's floor or wide.

## Wording

| Key | English |
|---|---|
| `gui.nothing_playing` | (nothing playing) |
| `gui.shuffle_word` / `gui.repeat_word` | shuffle / repeat — the group's labels on a legacy console, which has no `⇄` or `↻` |

The rest is the code's own: the auto-dj word ` auto-dj `, whose nine cells
set the split's width and quick config's column; `BPM` and, on a legacy
console, `key` before the key; the times and the readout as figures. The
song's words are the track's. The notes come from the App and the shell
(their own contracts).

## Out of scope here

- **Tooltips** (clause 11): the bar has none, in either face.
- **Drag to seek**: a press seeks; a drag only moves the preview.
- **The album on the card**: the card names the artist and the year; the
  mini player and the sheet name the album.

## Translation notes

- **Cells**: the kit's tall frames (`tall_frame`, `tall_frame_bordered`), a
  rounded frame with tees for the split, `▰ ▱` for the volume and the
  cover slot's rounded frame; the art painted over the cover's cells
  after the bar, pixels where the terminal draws them and the ▀-mosaic
  elsewhere. A legacy console swaps the glyphs it lacks (`<<`, `||`, `>`,
  `=`, `■ ·`, `^ v`, `▲ ▼`, `+ | +`) and the double border for the thick.
- **egui** (`src/gui/bar/egui_view.rs`): the Studio kit's pieces
  (`window/studio.rs`) — the transport marks and `frame_button`, the seek
  track, the LED, `caps`, the chevron, `step_mark` and `level_segment`,
  the star, `note_in`, `texture` and `picture_in` — painted in the cells'
  boxes on the panel's chrome layer (panel.rs), whose one pass runs at the
  frame's end once the cells have registered all that stands over the bar.

## Open questions

The canvas's board plan left these to the owner (its rulings R13–R24);
the bar is built as the code behaves until each is ruled on:

1. **Gold for playback** (R13): the seek and the play frame stay gold,
   though Studio keeps gold for warnings.
2. **The play slot with nothing loaded** (R14) wears the pause bars, and a
   press there does nothing and says nothing.
3. **Repeat One** (R15) wears All's look.
4. **Drag to seek and one span** (R16): the idle rule spans the whole row,
   the seek line a column less each side.
5. **The volume's segments** (R17) have no hover look; no segment reaches
   0 %; at 5 %, 15 % … 75 % the fill depends on whether + or − brought it
   there.
6. **The split's hover** (R18): the cells light the whole frame from the
   word's half, egui each half on its own; the chevron reads closed/open,
   not toward the popover.
7. **The card on the Admin screen** (R19) toggles a queue that is never
   drawn there, and keeps its chevron.
8. **The card's byline** (R20) never names the album.
9. **The quit's countdown** (R21) overwrites the bottom row.
10. **The note** (R22): a long note is cut at its 30 cells; the shell's
    note hides the App's news with no timer; "reaching …" is dim, not
    busy.
11. **Keyed tooltips on the buttons** (R24), shown only with key hints on.
12. **Hard-coded English** (R7): the auto-dj word, `BPM`.
13. **The bar under an Admin room's egui modal** (the transport spec's
    Q22) still lights while the room takes every press; decision D22 asks
    for no hover there. The fix belongs to the chrome's host
    (`chrome::bar_hot`).

## Deviations log

- **2026-11-28 — Extracted, with its egui face (clauses 15–20).** The
  canvas's looks that the cells do not have, each the board plan's
  decision as built:
  - **The seek line is a track, not cells** (D11): a 2-point line with a
    thumb and a ghost thumb where the cells brighten a cell; it keeps the
    cells' cells for its presses and its preview, so the ghost lands where
    a press would.
  - **Framed controls light by a step** (D4, D12): the raised fill and a
    step brighter for frame and mark, where the cells go bright and bold.
  - **The split's halves light on their own** (D13, R18), and the armed
    word wears an LED as well as the green.
  - **The volume's cells are segments** (D14) and its steps quiet marks;
    their missing hover look is kept (R17).
  - **The cover is egui's texture** (D15) with the desk's corners, not the
    art painted over the cells after the bar; the empty slot is a
    hairline square.
  - **The card is one raised button** under the pointer (D16).
  - **The note's tones are the kit's** (D17): the accent is Studio's amber
    where the cells' is blue, as on every egui part beside cells (the
    canvas's D3; the mixed seam is ruling R12).
  - **Paint only** (D22–D24): the bar takes no egui pointer, so the
    cells' press, right press, wheel and hand are its own unchanged, and
    nothing can act twice when egui runs a pass twice. No greying under a
    modal: what is inert is drawn at rest.
