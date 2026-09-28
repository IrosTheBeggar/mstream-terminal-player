# Mini player

| | |
|---|---|
| **Design of record** | The GUI's own parts, reused whole: the bar's card — its cover and its words — and its prev · play · next frames (the "Player bar options" canvas, A′ and I; `src/gui/bar.rs`), in the space the screens leave when the window is under their 100×24. Requested 2026-09-27 in place of the one dim line the GUI drew there; the song's words asked for the same day. |
| **Server API** | None. The cover is the one the bar already fetched for the playing track. |
| **Already in this repo** | The card cover's two draw paths (pixels, the ▀-mosaic), its empty slot frame, the card's words (`bar_now`, the key and tempo facts, the stars), the tall frames and their acts, `play_glyphs`; the keys Space, `p`, `n`, `-` `+`, which already worked in a small window. |
| **Status** | Implemented 2026-09-27, this document and the code in one commit. |

## Intent

A terminal pane squeezed beside other work still plays music you can
see and steer: the cover of what is playing, what it is, and the three
controls you reach for most, until there is room for the whole player
again.

## Entry points

1. The window, or the terminal's pane, shrinks below 100 columns or 24
   rows. There is no key: the size decides.

## States & flows

- **Room for the cover**: the cover, the song's words, the three frames,
  and the line.
- **Room for the frames only**: as many of the song's words as fit, the
  frames and the line.
- **Too small for the frames**: the line alone, as the GUI drew before.
- **Nothing playing, or its cover not yet decoded**: the empty slot frame
  where the cover will stand; with nothing playing, the card's dim
  "(nothing playing)" where the song's words would be.
- **The window grows past 100×24**: the screen it left returns as it was —
  the same screen, room, list and cursor, any open modal still open.

## Behavior contract

1. **Below the screens' size, the mini player is the whole window**: no top
   bar, nav, queue panel, bar, note line, footer, modal or tooltip. At or
   above it, nothing about the screens changes.
2. **The cover** is the playing track's, drawn the card's way — real pixels
   where the terminal draws them, the ▀-mosaic elsewhere — square in cells
   (twice as wide as tall). Before it is decoded, or with nothing playing,
   the empty slot frame holds its place.
3. **The transport** is the bar's: prev, play/pause, next in the bar's
   frames — play in the thick gold one, `▮▮` while playing and `▸` while
   paused — each a click on the bar's own act, lit under the pointer.
   Nothing else in the window acts on a click.
4. **The song's words** are the bar card's, a line each, above the
   frames: the title (bold), the artist, the album and the year, the
   file's spec, and the stars with the key and the tempo. A line with
   nothing to say is left out. Each is centred in its span and cut with
   the kit's mark where it is too long — never wrapped.
5. **The line** reads "Enlarge the terminal for the full player", dim,
   centred under the frames, wrapped to the width it has — at spaces, and
   by width where a language writes without them.
6. **The parts share the window by worth**: the title and the artist
   before the cover, the cover before the album, the spec and the stars
   only in room the cover cannot use; and the row of air over the frames
   goes before a line of the song does. The cover is stacked above the
   rest in a tall or narrow window and beside it in a short wide one —
   whichever draws it larger — with a cell or a row of air at the
   window's edges and between the parts. A cover smaller than the bar
   card's 8×4 is not drawn. Everything is centred.
7. **Too small for the frames** — under 22 columns, or too few rows for the
   frames and the line — the line stands alone, centred.
8. **The keys are unchanged**: they are the screen's, as they were in a
   small window before — Space, `p`, `n`, `-` `+` and `q` do what the bar
   names; the rest act on the screen that waits.

## Wording

| Key | English |
|---|---|
| `gui.mini.enlarge` | Enlarge the terminal for the full player |
| `gui.nothing_playing` | (nothing playing) — the card's own |

Ten locales, each in the register of its own `resize` line. The song's
words are the track's; the stars, the key's `♪` and `BPM` are the card's.

## Out of scope here

- **Seeking and volume by pointer.** The keys still do volume; seeking
  waits for the full player.
- **A click on the cover.** The card's right click opens the track's
  sheet, a modal the mini player has no room for.

## Deviations log

- **2026-09-27 — Requested and implemented** without a design canvas: the
  parts are the bar's, drawn as they are drawn there. The minimum cover is
  the card's 8×4, raised from a first cut's 6×3, whose frame left a sliver
  of picture.
- **2026-09-27 — The song's words** (clauses 4 and 6), asked for once the
  first cut stood: the card's lines, with the album on a line of its own
  — the card's byline is the artist and the year, and here there is room
  for the album the card leaves to the sheet.
