# mStream Terminal UI Kit

The design standard for every ratatui surface in this project — the setup
wizard first, any screen after. Every value here is ratatui-renderable:
named ANSI colors, `BorderType::Rounded`, cell/row spacing, text glyphs.
The visual companion (the same standard, drawn) lives on the design
canvas: <https://claude.ai/code/artifact/8eb74e0a-b721-434c-9ff7-b6f02385aab8>.
The shipped implementation is `src/kit/`: `Surface<A>` (the per-frame
click/tip/scrollbar registries plus pointer, tooltip dwell, capture and
hold-repeat, generic over each screen's action enum), the widgets as
free functions (`tall_button`, `button`, `modal_frame(_anchored)`,
`modal_close`, `scroll_list`, `draw_tooltip`, `input_display`), the
pure geometry (`table_view`, `bar_jump`, `tooltip_rect`, `caret_cell`),
the pointer contract, and `kit::theme` (the fixed palette + the OSC 11
ground lease). `src/setup/` is the reference consumer — a new screen
embeds a `Surface`, draws kit widgets, and wires its event loop to the
surface's `hit`/`arm_bars`/`motion`/`drag_action`/`hold_action`/
`dwell_tick` in a dozen lines. When this document and that code
disagree, fix one of them in the same change.

## Palette

Two regimes, deliberately different:

- **The setup wizard ships FIXED colors.** It is the brand moment, so it
  looks the same in every terminal that can carry it. `src/kit/theme.rs`
  resolves the table below once at startup through a three-tier ladder:
  **truecolor** (`COLORTERM` contains `truecolor`/`24bit` — macOS 26's
  Terminal.app advertises this) → **256-color** (`TERM` contains
  `256color`; cube/ramp indexes 16+ only — the first 16 are repainted by
  terminal themes; older Apple Terminals land here) → **named ANSI** (the
  floor: the adaptive palette, no ground painting — a 16-color console
  keeps its own background).
  `MSTREAM_SETUP_THEME=ansi|256|truecolor` overrides detection.
- **The player inherits the terminal theme.** Named ANSI only — never
  `Color::Rgb`, never `Color::Indexed` — so the user's own theme paints
  the app. Any future fixed-scheme surface opts in the wizard's way:
  module-local, through its own resolved theme, never by changing the
  player's `ui::Theme`.

| Token | Role | Truecolor | 256 | Named floor |
|---|---|---|---|---|
| TEXT | body text, values, paths | `#d8dee9` | 253 | default fg |
| GROUND | the painted background (fixed tiers only) | `#12131c` | 233 | terminal's own |
| ACCENT | actions, name chips, focus borders, selection bg | `#7aabdf` | 110 | `LightBlue` |
| BRIGHT | hover states, the active rename chip and caret | `#8fd6e8` | 117 | `Cyan` |
| DIM | hints, labels, idle borders, text buttons, table headers | `#69718f` | 60 | `DarkGray` |
| GOLD | the bottom rule, warnings/errors, the warning modal | `#e5c07b` | 179 | `Yellow` |
| OK | checked boxes, progress, success | `#98c379` | 114 | `Green` |
| DANGER | destructive hover (the row [X]); never decoration | `#e06c75` | 167 | `Red` |
| ON-ACCENT | fg on accent-filled cells (selection rows) | `#0d1017` | 232 | `Black` |

Element specs below name colors by floor name (LightBlue = ACCENT,
Cyan = BRIGHT, DarkGray = DIM, Yellow = GOLD, Green = OK, Red = DANGER,
Black = ON-ACCENT) — this table is the mapping; code goes through the
tokens.

Rules:
- Emphasis is `Modifier::BOLD`, and only BOLD. Dim text is fg DarkGray,
  not `Modifier::DIM` (uneven terminal support).
- Selection bg is ACCENT with ON-ACCENT fg. **Hover never uses the
  selection bg — hover brightens** (DIM→BRIGHT, ACCENT→BRIGHT).
- Cards and panels have **no background fill** — the ground is the only
  background (the wizard's painted GROUND on fixed tiers, the terminal's
  own on the floor).
- **The GROUND is all-or-nothing, gated on OSC 11 ownership.** Terminals
  reserve margin pixels around the cell grid painted with their DEFAULT
  background — cell fills can't reach them. At startup the wizard queries
  the terminal's default background (OSC 11; the answer doubles as
  capability detection and as the exact restore value) and only if
  answered does it set the default background — margins included — and
  paint cell grounds; the original is restored on exit, including the
  panic path. No answer → no ground anywhere and body text keeps the
  terminal's default fg. The fixed look must never sit inside a two-tone
  border of the user's own background.
- While the ground is owned, body text takes its fg from the ground fill
  — never the terminal default, which may be invisible on a painted
  ground. Anything that `Clear`s (modals) repaints the ground behind
  itself.

## Spacing — cells and rows

- One centered content column per screen, `min(width − 4, 74)` cells.
- Cards: 3 rows (border + 1 content row); 4 rows with a description line.
- **Primary buttons are 3-row Rounded frames, no fill** — the frame
  color is the emphasis. Text buttons are 1 row. Modal buttons are 1 row
  (modals are compact surfaces). Never two primaries in one row group.
- Section gaps: 1–2 blank rows.

## Glyphs — text only, no emoji

`▸` forward affordance · `[X]` per-row remove (right of the selection area) · `▏` text caret · `◂` back ·
`[✓]`/`[ ]` checkboxes · `(•)`/`( )` radios · `•` secret mask · `▰▱`
progress · `·` hint separator · `─` rules · rounded box-drawing via
`BorderType::Rounded` · `┴`/`┬` tooltip caret stems (merged into the
border, pointing at the target) · `█▀▄` half-blocks (QR only).

- Checkmark is ✓ U+2713; on restricted glyph sets (the player's Glyphs
  trust system) fall back to `[x]` — same width, same colors.
- Radio dot is • U+2022 — never ● (ambiguous-width; double-wide in CJK
  terminals).
- `ratatui-image` is reserved for album-art surfaces only — never chrome,
  never icons — so screens render identically over SSH and in ratzilla.

## Elements

### Buttons

**The terminal's button space** — a cell carries one glyph, one fg, one
bg, so exactly three shapes exist: a 1-row filled slab (square corners),
a rounded frame (box-drawing corners, no fill), and a fat fill with
quarter-cell soft corners (block elements `▗▄▖`/`▝▀▘` in the button
color). A filled block with truly rounded corners does not exist, and
smooth pill ends need private-use Powerline glyphs (patched fonts) —
banned.

- **Primary — THE STANDARD**: 3-row `BorderType::Rounded` frame, no
  fill; border and `  Label ▸  ` label both fg LightBlue BOLD — the
  frame color carries the emphasis. Hover: border and label brighten to
  Cyan together. Exactly one per screen, bottom-right (in the bottom bar
  where the screen has one).
- **Alternatives** (sanctioned, deliberate, never a third style on one
  screen): the 1-row filled slab for dense contexts; the fat soft-corner
  fill for high-impact moments.
- **Affirmative card** (the wizard's add-folder picker): a 3-row Rounded
  card whose border AND BOLD centered label are fg Green — the add
  action wears the affirmative color. Hover brightens both to Cyan like
  everything else. It sits ABOVE the table it feeds, so it holds one
  spot as rows come and go.
- **Secondary (tall)**: the backward/neutral action beside a primary —
  the same 3-row Rounded frame, border and label DarkGray; hover
  brightens both to Cyan (label BOLD). "◂ Back" is the canonical use.
  Never two primaries in a row group; this is how the second tall
  control stays honest.
- **Text**: 1 row, fg DarkGray. Hover: fg Cyan + BOLD.
- **Modal primary**: 1 row, label fg LightBlue BOLD (no frame — modals
  are compact); the SAFE choice gets it.
- **Disabled**: the primary frame with border and label fg DarkGray, no
  `▸`, no click rect, no hover, no hand cursor — but it DOES register a
  tooltip whose text says why it's disabled ("Add a folder first"), the
  one exception to disabled inertness. **Text buttons are never
  disabled — hide them instead** (a gray text button is indistinguishable
  from an idle one in 16 colors).
- Every clickable registers a click rect, participates in hover, and
  joins the OSC 22 hand-cursor set (see the pointer contract under
  Behavioral rules).

### Text input
Label above (fg DarkGray, UPPERCASE, small). 3-row Rounded card: idle
border DarkGray; focused border LightBlue with the `▏` caret after the
value (Cyan is reserved for hover and the active rename chip). Secrets
render as `•` repeat (the mask maps 1:1, so the cursor shows truly).
Fields run on the SAME line editor as the path field. Click focuses;
Tab/↓ cycle fields (the editor never sees them).
The name chip brightens under the pointer like every clickable —
hover fg Cyan + BOLD (a selected row keeps its bg; only the fg
brightens). Inline chip edits run on the SAME line editor as the path
field (tui-input: mid-line cursor, Home/End, ctrl word ops; the chip
windows around the cursor), with the vpath charset enforced at the
event gate — a-z 0-9 dash, uppercase folds, everything else typed is
dropped — so the draft is never illegal. They commit on BLUR: Enter
commits, Esc cancels, and a click anywhere outside the active chip
commits too — then the click proceeds as normal. Re-clicking the chip
being edited never clobbers the draft.

### Path input + completion
Suggestions under the input, max 6 visible — the list WINDOWS around the
keyboard cursor (the same viewport the folders table uses), with the
kit's scrollbar on overflow (wheel and ▲▼ clicks scroll; typing resets
the window). Rows: idle DarkGray, hover Cyan, keyboard-selected bg
LightBlue fg Black — the selected row is always in view. Tab completes
(longest common prefix, then cycles; Shift-Tab cycles back); ↑↓ pick;
Enter submits the text as typed; suggestions are clickable. Listings
are read locally (std::fs), loaded on the worker —
typing must never block. **Completion is LOCAL** — the wizard is a
same-machine first-run tool, and its primary affordance (the native
picker) already speaks the local filesystem, so every path affordance
agrees: typed completion, the fallback TUI browser, and the picker all
browse the wizard's machine, and `~` is the USER's home (expanded
synchronously as typed; a bare `~` gains its trailing separator, so
the preview lands INSIDE the home, not in its parent). The server validates every folder at commit —
the honest failure point for docker-from-host or remote-server setups,
where local paths may not exist server-side (a caveat the native picker
always had). Listings still run on the worker: a dead network mount can
hang read_dir. The input line is a REAL line editor
(tui-input): ←/→ move the cursor, insertion is mid-line, Home/End and
the ctrl word ops work; the display windows around the cursor (clipped
edges render `…`). Tab always completes; Right completes only from the
END of the line — anywhere else it is the editor's cursor key. An EMPTY
input suggests nothing (bare Tab must not fill in home entries). The input tail-scrolls: a value wider
than the field renders as `…<tail>` so the typed end and caret stay
visible. A listing failure for the CURRENT dir-part
renders in the modal in error gold ("could not list <dir>: <the
server's words>") — silent emptiness reads as "no autocomplete";
failures for dirs already typed past stay quiet.

### Links
fg LightBlue + `Modifier::UNDERLINED`; hover fg Cyan + BOLD. Click opens
the browser (the `open` crate) through an ordinary click rect. Label the
action; never print long URLs (short host in DarkGray parens when the
destination matters). Real OSC 8 hyperlink escapes are NOT part of the
standard — ratatui cells cannot carry them.

### Checkbox / opt-in row
4-row card: `[✓]` fg Green / `[ ]` fg DarkGray · label BOLD ·
description fg DarkGray indented 4 cells. Selected card border LightBlue.
Space/Enter/click toggles. No tri-state.

### Radio group
One card per group, DarkGray UPPERCASE group label, one row per option:
`(•)` fg **LightBlue** on the chosen row (choice semantics — Green means
on/affirmative), `( )` fg DarkGray otherwise; chosen label BOLD; inline
`— description` fg DarkGray. Exactly one chosen, always; choosing is not
toggling (clicking the chosen row does nothing). Radios for 2–4 options;
5+ becomes a list picker.

### Table
ratatui's `Table`: header row fg DarkGray UPPERCASE over a single `─`
rule row (no vertical separators, no zebra). The rule spans the FULL
row — the selection area and any trailing control column ([X]) alike. 1-line rows, 2-cell column
gaps, text left / numbers right, `—` fg DarkGray for missing values, `…`
truncation at the cell edge. Keyboard-selected row: bg LightBlue fg
Black; hover row fg Cyan. At most one intent-colored cell per row.
Overflow: `Scrollbar` right edge (track `│` DarkGray, thumb `█`
LightBlue, `▲▼` endcaps DarkGray), only when rows overflow. Empty
state: the header and rule stay on screen, with one DarkGray
parenthesized line where the first row would be — "(nothing added
yet)". **The whole
bar is live**: endcaps step one row and HOLD-REPEAT while pressed
(400ms pause, then ~60ms cadence, until mouse-up), track cells JUMP
proportionally to the clicked position, a track press arms a THUMB
DRAG (ridden on mouse-drag, released on mouse-up), the wheel scrolls,
and the bar brightens under the pointer (thumb and endcaps → Cyan)
like every clickable. **A bar interaction CAPTURES the pointer**:
while an arrow is held or the thumb dragged, sub-cell hand tremor must
not retarget hover onto whatever sits beside the 1-cell bar — and
terminals differ on whether mid-press motion arrives as Drag or plain
Moved, so BOTH honor the capture. Hover resumes on release.
**The phantom-release dialect** (Apple Terminal, probed 470.2): a press
reports as an INSTANT click pair, motion-while-held arrives as plain
Moved, and the physical release emits a SECOND click pair at wherever
the hand ended — holds are invisible, so hold-repeat and thumb-drag
cannot exist there (endcap steps and track jumps still work). A release
within 150ms of arming downgrades the capture to a SOFT one: hover
stays pinned within 2 cells of the press until the pointer genuinely
travels away or a real press lands, and an off-bar press inside that
radius is swallowed as the release re-click. Honest terminals release
late and never enter the soft path. Empty state:
**Every added folder is validated LOCALLY, on the worker** (a stat can
hang on a dead mount): exists / is a folder / readable, plus
canonicalization for truth-telling — another SPELLING of a chosen
folder (trailing slash, symlink, case) is removed with a note, and a
folder INSIDE another choice is marked (the server would scan it
twice). Problems paint the row's path GOLD with a tooltip saying why
and a one-time note — advisory only: warnings never block Continue,
the server has final say at commit.
**Selection is the KEYBOARD cursor, nothing more.** No row is selected
by default and mouse actions never need one — every row action is
directly clickable (the name chip, the [X]). Only ↑/↓ pick the cursor
up (↓ from the top, ↑ from the bottom); Esc stows it. A fresh add
scrolls its row into view but does not select it.
**The tips line names only what works right now**: no rows → just the
add actions; rows with the cursor stowed → how to pick it up, plus
continue; a row under the cursor → the full set.

### Modal
Centered, `Clear` beneath (no scrim — terminals have no alpha; the
border carries the weight). Border = intent: LightBlue neutral, Yellow
warning. First line: BOLD title in the intent color. Buttons bottom-right
(1-row; the safe choice fg LightBlue BOLD). Esc always dismisses.
**A modal makes the screen beneath INERT**: background clicks hit
nothing, background hover and tooltips sleep, the hand cursor follows
only the modal's own controls — the base draw sees no pointer and its
rect registries are dropped before the modal draws.
Dismissable pickers (path entry, the server browser) carry a `[X]`
close on the title row, right edge: DIM, hover BRIGHT + BOLD —
dismissal is neutral; red stays reserved for the destructive row
remove. Its tooltip names the keyboard path ("Close — Esc"). Warning
gates (the public-mode modal) get NO `[X]` — they force an explicit
choice.
A modal whose height varies (the suggestion list) anchors as if always
at full height: title and input hold one spot, the list grows DOWNWARD.

### Tooltip
A dwell of ~500ms on a tip target shows a floating box: a miniature of
the neutral modal — `Clear` + ground repaint beneath, Rounded border
DarkGray, 1-cell inline padding, text default fg, wrapped at 40 cells.
**Anchored to the TARGET, never the pointer**: centered under the
target's rect (above it when below would leave the frame), pulled inside
at the edges — one fixed spot however the pointer roams within the
target, so the box never jitters (and never repaints while it rests).
A box-drawing caret merged into the connecting border points at the
target's center: `┴` on the top border when the box hangs below the
target (`╭───┴───╮`), `┬` on the bottom border when it floats above
(`╰───┬───╯`) — one cell, DIM like the border, clamped off the corners,
tracking the target center even when the box is pulled sideways.
Hides the instant the pointer leaves and on any keypress. A modal
drops the base screen's tips with the rest of its rects — but the
modal's own controls may carry tips (the close control's "Close — Esc"). Tip targets are a per-frame rect registry parallel to the click
registry — a tip rect need not be clickable (the disabled-button
exception), and a clickable need not have a tip. Rules: tooltips are
mouse-only enhancement, so nothing may live ONLY in a tooltip (the tips
line and on-screen copy stay canonical); never tooltip the self-evident;
disabled controls' tips say WHY they're disabled. Harness note: the text
reaches the pty as per-word runs (the diff renderer skips unchanged
space cells) — e2e syncs must match word by word, not the whole phrase.

### Status, progress, notes
One status line above the keyboard tips. Busy: fg LightBlue,
present-progressive with `…`. Errors: fg Yellow, `<what failed>: <the
server's words>` — never a bare code. Progress: `▰` Green / `▱` DarkGray
plus a DarkGray label. All server calls run on the worker thread; the
busy line must be on screen the frame BEFORE the call.

## Screen chrome

Top: content starts at the top (no app header); a `N / 4` step counter
fg DarkGray sits top-right.

Bottom, in order: the status/note line · the keyboard tips (one DarkGray
line, LEFT side) · the **gold rule** (`─` × width, fg Yellow — the one
rule; screens have no top rule) · the **bottom bar** (3 rows): the scan
widget on the left (empty until a scan is actually running), the screen's
forward action as a tall primary on the right.

## Behavioral rules

- **Headless is first-class.** These screens are reached over SSH
  (docker/npm installs, launcher-less bundles). Every action keeps a key
  binding and a named hint in the tips line — hints compress, never
  disappear. Mouse (click, hover, OSC 22 hand cursor) is enhancement.
- **The pointer contract (OSC 22).** Announce the DEFAULT arrow once at
  startup (terminals keep their text beam until an app says otherwise),
  switch to the hand over clickables, emit only on state changes, and
  reset with the empty name on exit so the shell gets the terminal's own
  pointer back. Emit both name families back to back — X cursor names
  (`left_ptr`/`hand2`) then CSS names (`default`/`pointer`) — so every
  dialect lands on the same shape; unknown names are ignored. Honored by
  xterm (where OSC 22 originates), kitty (whose pointer-shapes spec names
  the CSS shapes), Ghostty and foot. NEITHER macOS terminal implements it
  — Apple Terminal 470.2 and iTerm2 3.6.11 both probed silent on every
  query dialect, and their I-beams persist even under mouse reporting —
  so their pointers cannot be changed by any escape, which is why the
  pointer is enhancement, never signal.
- **The UI never blocks.** Network calls and native dialogs run on a
  worker thread; results fold back between draws (see `src/setup/mod.rs`,
  the Job/Done pattern).
- Native pickers: osascript `choose folder` on macOS (an in-process
  NSOpenPanel never fronts from a terminal process), rfd on Windows,
  ashpd (default-features off) on Linux, with the in-TUI LOCAL browser
  as the universal fallback (same filesystem as the picker).
