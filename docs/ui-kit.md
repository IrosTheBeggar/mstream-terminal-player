# mStream Terminal UI Kit

The design standard for every ratatui surface in this project — the setup
wizard first, any screen after. Every value here is ratatui-renderable:
named ANSI colors, `BorderType::Rounded`, cell/row spacing, text glyphs.
The visual companion (the same standard, drawn) lives on the design
canvas: <https://claude.ai/code/artifact/8eb74e0a-b721-434c-9ff7-b6f02385aab8>.
The shipped reference implementation is `src/setup/` — when this document
and that code disagree, fix one of them in the same change.

## Palette

Two regimes, deliberately different:

- **The setup wizard ships FIXED colors.** It is the brand moment, so it
  looks the same in every terminal that can carry it. `src/setup/theme.rs`
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
render as `•` repeat. Click focuses; Tab/↓ cycle fields.

### Path input + completion
Suggestions under the input, max 6 + "… and N more". Rows: idle DarkGray,
hover Cyan, keyboard-selected bg LightBlue fg Black. Tab completes
(longest common prefix, then cycles); ↑↓ pick; Enter submits the text as
typed; suggestions are clickable. Listings come from the server (admin
file-explorer), loaded on the worker — typing must never block.

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
rule row (no vertical separators, no zebra). 1-line rows, 2-cell column
gaps, text left / numbers right, `—` fg DarkGray for missing values, `…`
truncation at the cell edge. Keyboard-selected row: bg LightBlue fg
Black; hover row fg Cyan. At most one intent-colored cell per row.
Overflow: `Scrollbar` right edge (track `│` DarkGray, thumb `█`
LightBlue, `▲▼` endcaps DarkGray), only when rows overflow. Empty state:
one DarkGray parenthesized line that says what to DO.

### Modal
Centered, `Clear` beneath (no scrim — terminals have no alpha; the
border carries the weight). Border = intent: LightBlue neutral, Yellow
warning. First line: BOLD title in the intent color. Buttons bottom-right
(1-row; the safe choice fg LightBlue BOLD). Esc always dismisses.

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
Hides the instant the pointer leaves, on any keypress, and while a modal
is open. Tip targets are a per-frame rect registry parallel to the click
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
  ashpd (default-features off) on Linux, with the in-TUI server-side
  browser as the universal fallback.
