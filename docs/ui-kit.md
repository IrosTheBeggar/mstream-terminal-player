# mStream Terminal UI Kit

The design standard for every ratatui surface in this project — the setup
wizard first, any screen after. Every value here is ratatui-renderable:
named ANSI colors, `BorderType::Rounded`, cell/row spacing, text glyphs.
The visual companion (the same standard, drawn) lives on the design
canvas: <https://claude.ai/code/artifact/8eb74e0a-b721-434c-9ff7-b6f02385aab8>.
The shipped reference implementation is `src/setup/` — when this document
and that code disagree, fix one of them in the same change.

## Palette — named ANSI only

Never `Color::Rgb`, never `Color::Indexed`: named colors only, so every
screen inherits the user's own terminal theme (the player's rule too).

| Token | Role |
|---|---|
| default fg | body text, values, paths |
| `Color::LightBlue` | ACCENT — actions, name chips, focus borders, selection bg |
| `Color::Cyan` | BRIGHT — hover states, the active rename chip and caret |
| `Color::DarkGray` | DIM — hints, labels, idle borders, text buttons, table headers |
| `Color::Yellow` | GOLD — the bottom rule, warnings/errors, the warning modal |
| `Color::Green` | OK — checked boxes, progress, success |
| `Color::Red` | DANGER — destructive hover (the row ✕); never decoration |

Rules:
- Emphasis is `Modifier::BOLD`, and only BOLD. Dim text is fg DarkGray,
  not `Modifier::DIM` (uneven terminal support).
- Selection bg is LightBlue with black fg. **Hover never uses the
  selection bg — hover brightens** (DarkGray→Cyan, LightBlue→Cyan).
- Cards and panels have **no background fill** — the terminal ground is
  the only background.

## Spacing — cells and rows

- One centered content column per screen, `min(width − 4, 74)` cells.
- Cards: 3 rows (border + 1 content row); 4 rows with a description line.
- **Primary buttons are 3-row Rounded frames, no fill** — the frame
  color is the emphasis. Text buttons are 1 row. Modal buttons are 1 row
  (modals are compact surfaces). Never two primaries in one row group.
- Section gaps: 1–2 blank rows.

## Glyphs — text only, no emoji

`▸` forward affordance · `✕` per-row remove · `▏` text caret · `◂` back ·
`[✓]`/`[ ]` checkboxes · `(•)`/`( )` radios · `•` secret mask · `▰▱`
progress · `·` hint separator · `─` rules · rounded box-drawing via
`BorderType::Rounded` · `█▀▄` half-blocks (QR only).

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
  `▸`, no click rect, no hover, no hand cursor. **Text buttons are never
  disabled — hide them instead** (a gray text button is indistinguishable
  from an idle one in 16 colors).
- Every clickable registers a click rect, participates in hover, and
  joins the OSC 22 hand-cursor set.

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
- **The UI never blocks.** Network calls and native dialogs run on a
  worker thread; results fold back between draws (see `src/setup/mod.rs`,
  the Job/Done pattern).
- Native pickers: osascript `choose folder` on macOS (an in-process
  NSOpenPanel never fronts from a terminal process), rfd on Windows,
  ashpd (default-features off) on Linux, with the in-TUI server-side
  browser as the universal fallback.
