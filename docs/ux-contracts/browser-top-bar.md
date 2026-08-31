# Browser top bar

| | |
|---|---|
| **Design of record** | `mstream_music` @ `137dd27` — `lib/widgets/browser_toolbar.dart` (the context-aware AppBar-bottom bar) and `lib/widgets/local_search_bar.dart` (the live list filter); action gating and demotion rules from the toolbar's own comments |
| **Server API** | none — every verb here acts on the list already fetched |
| **Already in this repo** | the whole filter machinery: `Action::StartFilter`, `App.filtering`, `Pane::apply_filter`/`clear_filter` with live narrowing, Submit-keeps / Cancel-clears / backspace-past-empty-leaves, and `Pane::counts()` = (shown, total); `Pane::tracks_with_offset` for the playable rows; `play_index`; the queue |
| **Target surface** | the GUI player's browse rooms: **Files**, **Albums** (wall and drilled album), **Playlists** (list and drilled playlist) |
| **Status** | implemented in the GUI, 2026-08-31; extended to all four browse pages the same day |

## Intent

One bar above the browse list that adapts to what the list holds: a live
**filter** for finding a row in a long listing without leaving it, and the
whole-list verbs — **play**, **queue all**, **shuffle** — exactly when the
list has something playable, never as dead chrome.

## Scope

The record's bar serves five contexts; four of them port here as the
browse rooms' shared header — the normal list (Files), album detail (the
drilled album), the open playlist, and the open filter — with the albums
WALL added as this surface's own context (the record has no grid). The
home "search the whole server" field (with its category picker) stays the
Search room's. One law travels with the bar everywhere: *verbs appear only
when they would do something*.

## Behavior contract

### The bar (resting) — two lines

1. **Line one is the room's own header, unchanged**: back (`◂`,
   clickable) when there is somewhere to go — above the root it
   disappears, not dims — the crumb or title, leading-clipped so the leaf
   survives, and the count at the right edge.
2. **Line two carries the controls.** The **filter** affordance leads it
   on the left — always there; finding a folder or an album by name is as
   real as finding a track. The verbs sit right: **Play, Queue all and
   Shuffle appear only while the list holds playable rows** (the record's
   rule: lists of containers — the albums wall, the playlists — keep a
   clean line, and there is never a play button with nothing to play).
3. The count reads `n items` (the wall keeps its `n albums`); under an
   active filter it reads **`n of m`** — the narrowed view never
   impersonates the whole.
4. On the **albums wall** the filter narrows the grid itself: pages,
   cells, the keyboard cursor and the cover prefetch all follow the
   narrowed view, and a click on a filtered cell opens the album it
   shows, never what lived at that index unfiltered.

### The verbs

10. **Play** replaces the queue with the list's playable rows, in order,
    and plays from the first — the same road a click on the first track
    takes.
11. **Queue all** appends the list's playable rows to the queue and says
    how many (`queued n`); when the queue was empty and nothing was
    playing, playback starts — the house rule every queue-add follows
    here.
12. **Shuffle** is Play with the order shuffled **once** — the record
    shuffles the rows before enqueueing and leaves the shuffle *mode*
    untouched, and so does this bar.
13. Under an active filter the verbs act on the **narrowed view** — what
    you see is what plays. *(Deviation: the record's actions read the
    unfiltered list; see the log.)*

### The filter

20. Opening the filter (the bar affordance, or its key) turns the bar into
    the field: a close affordance, the query with the caret, and the
    live `n of m`.
21. **Narrowing happens on every keystroke** — there is nothing to submit,
    only somewhere to stop typing (the App's own words). Matching is the
    pane's, case-insensitive.
22. **Enter keeps the narrowed list** and returns the keys to it — the
    narrowed list is the point. The bar keeps showing the query and the
    count, with the clear affordance, until the filter is cleared.
23. **Esc (or the close affordance) clears and closes**; backspacing past
    an empty query also leaves — the way out when nothing was meant.
24. Navigation clears the filter: a filter describes the list it was typed
    against (the App already enforces this on drill and back).

### Empty results

30. A filter that matches nothing shows the empty words under the bar and
    `0 of m` in it — the way back (clause 23) is one key.

## Wording

The record's bar is icons + tooltips; this surface writes the verbs out.
English reference:

| String | Source |
|---|---|
| play · queue all · shuffle · filter | the record's Play / Add all / Shuffle / "Search this list" tooltips, worn as kit text buttons |
| queued %{count} | browserSongsAdded, house-shortened |
| %{shown} of %{total} | this surface (the record filters silently) |
| %{count} items | already shipped (gui.files.items) |

## Out of scope here

- **Download all**: this player has no downloads subsystem.
- **The home whole-server search + category picker**: the Search room's
  contract when its top bar is revisited.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| AppBar bottom slot, 50 px | The Files room's crumb row, one cell row |
| Icon buttons + tooltips | Kit 1-row text buttons, dim → bright on hover |
| Filled accent Play button | The `▸`-led verb, accent; the bar's one emphasis |
| The filter TextField | The kit line input drawn in the bar (`input_display`), the App's `filtering` state |
| Overflow ⋮ menu | Nothing demoted — 80 columns hold every verb inline, so the demotion rules collapse |
| Snackbar counts | The note line (`queued n`) |

Already shared: everything stateful. The filter is `Action::StartFilter` +
the App's `filtering` prompt (live narrowing, Submit/Cancel/backspace-out
all in place, used by the TUI everywhere); counts are `Pane::counts()`;
the playable rows are `Pane::tracks_with_offset()`. New here: only the
drawing, the keys, and the three verbs composed from public App pieces
(`queue.replace`/`push` + `play_index` + a one-shot `fastrand` shuffle).

## Deviations log

- **2026-08-31 — Verbs act on the narrowed view** (clause 13): the record's
  add-all/download read the unfiltered list even while the body filters.
  On this surface the pane *is* the filtered list — what you see is what
  plays — and that is both the App's existing shape and the honest one.
- **2026-08-31 — Nothing is demoted to an overflow**: the record demotes
  verbs to a ⋮ menu to fit a phone bar; at a minimum of 100 columns every
  verb fits inline, so the record's demotion choreography (search trades
  places with Add all, Shuffle moves to the menu) collapses to "all
  visible, gated by playability".
- **2026-08-31 — No design canvas**: one bar row of existing kit
  vocabulary — text buttons, the line input, dim counts. The translation
  table is the design.
- **2026-08-31 — Two lines, filter left** (review): one line was messy —
  the crumb, four verbs and the count elbowing each other. The header row
  stays what each room always drew; the controls line beneath leads with
  the filter and right-aligns the verbs.
- **2026-08-31 — Extended to Albums and Playlists**: the drilled views
  reuse the pane machinery wholesale; the wall needed an index map so the
  grid, its paging and its clicks follow the filter. The grid keeps its
  row count by spending the last row's trailing blank on the controls
  line.
