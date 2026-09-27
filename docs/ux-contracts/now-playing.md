# Now Playing

| | |
|---|---|
| **Design of record** | This repo's own TUI full-screen view — `src/tui/ui.rs`: `render_now_playing` and what it calls (`now_regions`, `now_playing_card`, `render_facts_cover`, `render_now_panel` with the tab renderers, `waveform_top_line`, `progress_line`, `mode_readout`), `src/tui/keymap.rs` (the `now-playing` bindings: digits, Tab, ↑↓, ←→, `v`, `.`, Esc), `src/tui/app.rs` (`fullscreen`, `NowTab`, `now_tabs`, `SelectNowTab`, `NowTabNext/Prev`, `NowLeft/Right`, `CycleViz`, `ToggleScatter`, `JumpToPlaying`, `drawing_audio`) — at commit `69a7f53` (2026-09-26). PLAN.md had the mobile app's player panel, lyrics screen and visualizer down as the record; the direction on 2026-09-26 was to reuse this view instead, whole, and add only a transport under the album art. |
| **Server API** | The track's waveform (`Client::waveform_async`, `ApiCmd::Waveform`, cached per path in `App.waveforms`, prefetched for the next row); the tags the pane already holds; Discover's neighbours (`App.now_discover`, the sonic-path contract's). Nothing new. |
| **Already in this repo** | All of the above, live in the TUI. The GUI had a first-cut screen (2026-09-22: the cover as large as the stage allowed with the card's four lines beneath) in `src/gui/now.rs`, drawn beside the GUI's queue panel and over its bar. |
| **Status** | Implemented 2026-09-26, this document and the code in one commit. |

## Intent

The playing track, whole, on one screen the two shells share: what it is,
what the tags know about it, its shape as the scrubber, and the panel of
things about it — the queue, the neighbours, the DJ, the picture of the
sound. A song reads the same in both shells. The GUI adds only what a
pointer needs to drive the view: the transport under the cover.

## Entry points

1. The top bar's **Now Playing** tab; the **Library** tab beside it is the
   way back.
2. **`0`** from any room; **`0`** or **Esc** back to the Library.
3. A nav row's click leaves for that room. **The digits mean the view's
   tabs while it is up** (clause 8): the rooms are one `0` away.

## States & flows

- **Nothing playing**: the card says "nothing playing", the band is a flat
  line through its own middle, the panel shows its tabs (the queue tab
  reads "nothing queued").
- **Playing without a shape** (no ffmpeg on the server, an unscanned or
  federated track, a request still out): the scrubber is a plain bar.
  **With a shape**: the band is the mirrored waveform, the played part in
  the accent.
- **Tabs**: Queue, Auto-DJ and Visualizer always; Lyrics while the track
  has them; Discover while the server has discovery. The tab kept comes
  back; a tab that left with its track falls back to the Queue.
- **The App's `fullscreen` flag is up while the screen is** and down when
  it leaves, so the App's full-screen semantics apply to every key here
  exactly as in the TUI's view.

## Behavior contract

### The view

1. **It is the TUI's view**, drawn under the GUI's top bar in the row the
   TUI spends on its own title: the facts column at the left (26–46
   cells — the title in the accent, the artist, the labelled facts as a
   ladder; the cover beneath, near-square, pinned under the facts, pixels
   where the terminal draws them and the ▀-mosaic elsewhere), the panel at
   the right (the tab strip over a rule, then the tab in front), the rule
   with its junction under the column's divider, the band (the mirrored
   half over the scrubber, `position / total` at its right), and the last
   row's modes readout at the right (`vol · repeat · shuffle · dj on`).
2. **The GUI's queue panel and bar stand down** on this screen: the view has
   its own queue tab and its own seek control. The Library screen keeps
   both. The top bar stays.
3. **Prev · play/pause · next stand under the cover**, in the bar's own
   frames — the rounded prev and next in the text colour, the thick gold
   play, `◂◂ ▸ ▸▸` (`▮▮` while playing) — centred in the facts column, one
   row of air under the picture; under the card's blank row when there is
   no cover; not at all when the column has no three rows left (the keys
   still work). The cover yields them four rows.
4. **The band seeks on click and lights under the pointer**, as the TUI's
   does: the TUI's own `seek_target` maps the column to the track, and a
   click past the bar, or on a track of unknown length, seeks nowhere.
5. **The tabs click** — each tab of the strip, or the `‹ ›` arrows when the
   strip does not fit.
6. **A right click on the cover**, or `m`, is the playing track's sheet
   (the track-actions contract).
7. **A cover an overlay touched last frame draws as the mosaic** for that
   frame — the GUI's rule for every cover, since a picture's cells are
   skipped by the terminal writer.

### Keys

8. **The TUI's own.** Digits **1–n pick the tabs** (a digit past the strip
   does nothing; the App bounds-checks), **Tab / Shift-Tab** cycle them,
   **↑ ↓ PageUp PageDown** walk the tab in front — the queue's rows,
   Discover's neighbours, the Auto-DJ rows, the lyrics — **Enter** plays
   the queue row or neighbour in hand, **← →** adjust an Auto-DJ row, **`a`**
   queues a neighbour, **`d`** removes the queue row, **`i`** goes to the
   playing row (the Queue tab), **`v`** and **`.`** are the visualizer's
   mode and dots. **Space, `p`, `n`, `s`, `r`, `A`, `-`, `+`** are the
   transport as everywhere in the GUI; **`m`** the sheet; **`D`** the
   Auto DJ room; **`0`, Esc** back to the Library.
9. **The key hints are the GUI footer's** (`gui.tips.now`, off by default
   under `[gui] key_hints`); the view's own hint row is not drawn. Its
   left half carries the screen's note instead, its right the modes.
10. **The visualizer asks for thirty frames a second** while it moves
    (`App::drawing_audio`); every other tab idles at the poll.

### The cursor

11. **The panel's lists keep the TUI's always-visible cursor** (ratatui
    lists with their state): the view is reused whole. A deliberate
    exception to the kit's list-cursor law for the GUI's own lists
    (docs/ui-kit.md, "List cursor"), logged below.

## Wording

The view's words are the TUI's, in English, whatever the locale — the tab
titles, the facts' labels, "nothing playing", the placeholders — because
the TUI is not localized. The GUI's own strings around it are: `gui.top.now`
names the tab, `gui.tips.now` the keys on the footer (ten locales).

## Out of scope here

- The mobile record's player panel (the swipe queue under a big cover),
  its lyrics screen and its More sheet: the direction was to reuse the
  TUI view. The Lyrics tab stays the TUI's placeholder.
- A volume control on the screen: the keys have it, the Library screen's
  bar has it, the modes readout shows the level.
- Clickable rows in the panel's lists: the keyboard drives them, as in the
  TUI.
- Localizing the view.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| `render_now_playing`: `Clear`, the title row, then the parts | `render_now_view` over `now_regions(view)` — the title row is the top bar's, and nothing is cleared (the GUI paints its ground) |
| The cover under the facts | The same path, four rows shorter for the transport (`NowExtras.reserve`), as the mosaic when an overlay stood over it last frame (`NowExtras.mosaic`) |
| The keys row: hints left, modes right | The modes right (`NowExtras.no_hints`); the note at the left; the GUI footer names the keys |
| Mouse: a click on the band seeks (`on_mouse`) | `Act::NowSeek(column)` per band column → `seek_target` + `App::seek_to`; `Act::NowTab(i)` / `Act::NowTabStep(±1)` on the strip; `Act::NowMore` on the cover's right click; `App::note_pointer` on every mouse event so the band lights |
| No transport (keys only) | `bar::tall_compact` × 3 under the cover: `Act::Prev`, `Act::PlayPause`, `Act::Next` |
| `fullscreen` toggled by `0`/Esc | Set by the screen: up on `Act::Screen(NowPlaying)`, down on `Act::Screen(Library)` and every `Act::Nav` |

## Deviations log

- **2026-09-26 — Extracted and implemented.** The record changed from the
  mobile app's player panel to the TUI's full-screen view on direction
  ("re-use the existing view as much as possible — the one with the
  waveform progress bar; the only change is the play/prev/next buttons
  under the album art"). With it: the GUI's queue panel and bar stand down
  on the screen; the digits pick the view's tabs while it is up; the
  panel's lists keep the TUI's visible cursor; the view's words stay
  English; there is no volume control on the screen.
- **2026-09-26 — The review's fixes.** The screen serves the App's input
  modes first: the Auto-DJ tab's keyword field takes the letters, Enter
  and Esc, and its sources picker (the TUI's overlay, lent here) takes the
  list keys — Enter on those rows opened a mode nothing could serve or
  leave, and every later action fell into it. The shell follows the App
  when it leaves its full-screen view on its own (arming a pick, the pick
  banner's [X] going home), so the keys never drive a pane off screen.
  The Settings sub-rooms are the Library screen's. The GUI boots the
  system language now, as the wizard does; the view's own words stay the
  TUI's English.
