//! The design spec's board mocks as test fixtures, and the matcher that holds
//! a drawn [`Buffer`] to them.
//!
//! Each `boards/peer_recs/<NAME>.txt` is one `MOCK:` block of the Peer recs
//! room's board spec, copied verbatim (TERM-DECISIONS §0.5): NAME is the
//! block's tag, `M1` or `REG-STRIP`. A block holds one or more frames, each a
//! TEXT fence and a STYLE fence of equal shape. A fence starts with two ruler
//! lines (the column's tens digit, then its units digit); then each row is
//! its number right-aligned in 2 cells, 2 spaces, then exactly its cells.
//!
//! - A **full frame** (`M1` … `V4`) is a whole screen. Its row numbers are
//!   buffer rows, and its label is its caption (`ALL PEERS · IDLE`). A
//!   derived frame gives only the rows that differ from its base, and
//!   [`board`] fills every other row from the base, as the spec reads it.
//! - A **region frame** (`REG-*`) is a slice at the columns its ruler names.
//!   Each of its rows is one variant of the same screen row, so an [`Area`]
//!   places it at a buffer row (`y_at`). Its label is its width in cells:
//!   `80` (`78` for the grid), and `47` for the queue-open slice.
//!
//! The STYLE codes are read off the drawn cell with the kit's own tokens
//! ([`th`]) and the swatch from [`swatch`], so the matcher holds on any
//! colour tier. On the 256- and 16-colour tiers a swatch `1`–`9` is drawn
//! dim (peer-recs contract §0.6), and the matcher reads it so.
//!
//! | code | the drawn cell |
//! |---|---|
//! | (space) | a blank cell, not on a slab; its colour cannot show |
//! | `t` / `T` | `th().text` or `Reset`, without / with BOLD |
//! | `d` | `th().dim` (BOLD is not read) |
//! | `a` `A`, `g` `G`, `o` `O` | accent, gold, ok, without / with BOLD |
//! | `S` / `s` | the slab: on_accent on accent, BOLD / plain; a blank slab cell is `S` |
//! | `%` | a swatch on the slab: any swatch (or dim) on accent |
//! | `1`–`9` | the §0.6 tile's swatch, by its hue |
//! | `c` | under the cover art: not compared, TEXT or STYLE (pixels, the mosaic, or the slot's frame) |
//!
//! A `c` cell is skipped whole, not read as a space: the bar paints the art
//! (or its empty slot's dim frame) over those cells, so what the buffer
//! holds there never shows.
//!
//! Any other colour, a ground other than `Reset` or the kit's, and any
//! modifier but BOLD (the spec draws no underline, italic or reverse) read
//! as `?`, which no mock holds.

use std::ops::RangeInclusive;

use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier};

use super::theme::{swatch, th, truecolor};

/// One mock block: its tag and its frames, in the spec's order.
pub(crate) struct Board {
    pub name: String,
    pub frames: Vec<Frame>,
}

/// One TEXT/STYLE pair of a block.
pub(crate) struct Frame {
    /// A full frame's caption (`ALL PEERS · IDLE`), or a region frame's
    /// width in cells (`80`, `78`, `47`).
    pub label: String,
    /// The first column the ruler names: 0 for a full frame.
    pub x0: u16,
    /// `(row, text, style)`, in row order: one char per cell in both.
    pub rows: Vec<(u16, String, String)>,
    /// A full frame's size in cells, from its caption line (`100×30`).
    pub size: Option<(u16, u16)>,
    /// A derived frame's base, by label; its other rows came from there.
    pub base: Option<String>,
    /// The rows the block itself gives: all of them, unless derived.
    pub own: Vec<u16>,
    /// A region frame's caption per row (`3` → "admin, Kitchen Radio …").
    pub captions: Vec<(u16, String)>,
}

impl Frame {
    /// A whole screen, not a region slice.
    pub fn full(&self) -> bool {
        self.size.is_some()
    }

    /// The frame's width in cells, as its ruler counts them.
    pub fn width(&self) -> usize {
        self.rows.first().map_or(0, |(_, text, _)| text.chars().count())
    }
}

/// What a comparison covers. Every field left `None` is the whole frame.
#[derive(Debug, Clone, Default)]
pub(crate) struct Area {
    /// The buffer row the first compared mock row is drawn at, each later
    /// one a row lower. A region frame needs it; for a full frame `None`
    /// reads a mock row as the same buffer row, and `Some` moves the frame
    /// (the sheet sits at y 6–17 at 100×24, and at y 9–20 in M3).
    pub y_at: Option<u16>,
    /// The mock rows to compare, by their number.
    pub rows: Option<RangeInclusive<u16>>,
    /// The columns to compare, as the ruler and the buffer count them.
    pub cols: Option<RangeInclusive<u16>>,
}

impl Area {
    /// Every cell the frame gives, each at its own buffer row.
    pub(crate) fn whole() -> Area {
        Area::default()
    }

    /// The cells in columns `x0..=x1` and rows `y0..=y1` of a full frame,
    /// for example the lead sheet's x 17–82, y 9–20.
    pub(crate) fn rect(x0: u16, x1: u16, y0: u16, y1: u16) -> Area {
        Area { y_at: None, rows: Some(y0..=y1), cols: Some(x0..=x1) }
    }

    /// One variant of a region frame, its mock row `row`, at buffer row `y`.
    pub(crate) fn at(y: u16, row: u16) -> Area {
        Area { y_at: Some(y), rows: Some(row..=row), cols: None }
    }
}

/// The Peer recs room's boards, by tag (TERM-DECISIONS §3–§7, and the
/// region mocks of §1.4–§1.8).
const PEER_RECS: [(&str, &str); 25] = [
    ("REG-HEADER", include_str!("boards/peer_recs/REG-HEADER.txt")),
    ("REG-STRIP", include_str!("boards/peer_recs/REG-STRIP.txt")),
    ("REG-GRID", include_str!("boards/peer_recs/REG-GRID.txt")),
    ("REG-BAND", include_str!("boards/peer_recs/REG-BAND.txt")),
    ("M1", include_str!("boards/peer_recs/M1.txt")),
    ("M2", include_str!("boards/peer_recs/M2.txt")),
    ("M3", include_str!("boards/peer_recs/M3.txt")),
    ("M4", include_str!("boards/peer_recs/M4.txt")),
    ("W1", include_str!("boards/peer_recs/W1.txt")),
    ("W2", include_str!("boards/peer_recs/W2.txt")),
    ("W3", include_str!("boards/peer_recs/W3.txt")),
    ("L1", include_str!("boards/peer_recs/L1.txt")),
    ("L2", include_str!("boards/peer_recs/L2.txt")),
    ("L3", include_str!("boards/peer_recs/L3.txt")),
    ("L4", include_str!("boards/peer_recs/L4.txt")),
    ("S1", include_str!("boards/peer_recs/S1.txt")),
    ("S2", include_str!("boards/peer_recs/S2.txt")),
    ("S3", include_str!("boards/peer_recs/S3.txt")),
    ("S4", include_str!("boards/peer_recs/S4.txt")),
    ("S5", include_str!("boards/peer_recs/S5.txt")),
    ("S6", include_str!("boards/peer_recs/S6.txt")),
    ("V1", include_str!("boards/peer_recs/V1.txt")),
    ("V2", include_str!("boards/peer_recs/V2.txt")),
    ("V3", include_str!("boards/peer_recs/V3.txt")),
    ("V4", include_str!("boards/peer_recs/V4.txt")),
];

/// Every code a STYLE row may hold.
const CODES: &str = " tTdaAgGoOSs%c123456789";

/// A board by its tag, its derived frames filled from their base. Panics on
/// an unknown tag or a malformed block: a fixture that does not parse is a
/// broken test.
pub(crate) fn board(name: &str) -> Board {
    let mut board = parse(name, source(name));
    for frame in &mut board.frames {
        if let Some(base) = frame.base.clone() {
            fill_from(name, frame, &base);
        }
    }
    board
}

/// DS §0.6's swatch codes: `1`–`9`, each a tile's server-stamped hue.
pub(crate) fn ds_hue(code: char) -> Option<u16> {
    match code {
        '1' => Some(203),
        '2' => Some(39),
        '3' => Some(4),
        '4' => Some(352),
        '5' => Some(78),
        '6' => Some(154),
        '7' => Some(353),
        '8' => Some(53),
        '9' => Some(81),
        _ => None,
    }
}

/// The STYLE code a drawn cell reads as (the module's table). `hue_of` maps
/// a swatch code to its hue: [`ds_hue`] for the spec's boards. A swatch on
/// the slab (`%`) is any of those hues, dim, or `hue_of('%')` when given.
pub(crate) fn style_code(cell: &Cell, hue_of: &dyn Fn(char) -> Option<u16>) -> char {
    let t = th();
    if !cell.modifier.difference(Modifier::BOLD).is_empty() {
        return '?';
    }
    let bold = cell.modifier.contains(Modifier::BOLD);
    let blank = cell.symbol().trim().is_empty();
    // Off truecolor every swatch is dim, and reads as `d`.
    let swatch_code = |fg: Color| {
        truecolor()
            .then(|| ('1'..='9').find(|&code| hue_of(code).is_some_and(|h| swatch(Some(h)) == fg)))
            .flatten()
    };
    if cell.bg == t.accent {
        // A blank slab cell shows neither its colour nor its BOLD.
        return if blank {
            'S'
        } else if cell.fg == t.on_accent {
            if bold { 'S' } else { 's' }
        } else if cell.fg == swatch(None)
            || swatch_code(cell.fg).is_some()
            || hue_of('%').is_some_and(|h| swatch(Some(h)) == cell.fg)
        {
            '%'
        } else {
            '?'
        };
    }
    if cell.bg != Color::Reset && Some(cell.bg) != t.ground {
        return '?';
    }
    if blank {
        return ' ';
    }
    let weight = |plain, strong| if bold { strong } else { plain };
    match cell.fg {
        fg if fg == t.text || fg == Color::Reset => weight('t', 'T'),
        fg if fg == t.dim => 'd',
        fg if fg == t.accent => weight('a', 'A'),
        fg if fg == t.gold => weight('g', 'G'),
        fg if fg == t.ok => weight('o', 'O'),
        fg => swatch_code(fg).unwrap_or('?'),
    }
}

/// Hold `buf` to a board's frame inside `area`: every cell the mock gives
/// there, TEXT and STYLE. `label` is the frame's label, or `""` for the
/// board's first frame. On a failure it prints, for every row that
/// differs, the mock's and the drawn TEXT and STYLE rows, with the first
/// differing cell marked.
#[track_caller]
pub(crate) fn assert_frame(buf: &Buffer, board: &str, label: &str, area: Area) {
    let board = self::board(board);
    let frame = pick(&board, label);
    if let Some(mismatch) = diff(buf, &board, frame, &area) {
        panic!("{}", mismatch.report);
    }
}

// ── Parsing ──────────────────────────────────────────────────────────────────

fn source(name: &str) -> &'static str {
    match PEER_RECS.iter().find(|(tag, _)| *tag == name) {
        Some((_, src)) => *src,
        None => {
            let tags: Vec<&str> = PEER_RECS.iter().map(|(tag, _)| *tag).collect();
            panic!("no board {name:?}; the boards are {tags:?}")
        }
    }
}

fn pick<'a>(board: &'a Board, label: &str) -> &'a Frame {
    let found = board.frames.iter().find(|f| label.is_empty() || f.label == label);
    found.unwrap_or_else(|| {
        let labels: Vec<&str> = board.frames.iter().map(|f| f.label.as_str()).collect();
        panic!("board {} has no frame {label:?}; its frames are {labels:?}", board.name)
    })
}

/// A fence's rows, before it is paired with its twin.
struct Fence {
    x0: u16,
    width: usize,
    rows: Vec<(u16, String)>,
}

fn parse(name: &str, src: &str) -> Board {
    let mut frames = Vec::new();
    let mut caption: Option<(String, (u16, u16))> = None;
    let mut base: Option<(String, Vec<u16>)> = None;
    let mut captions = Vec::new();
    let mut heading: Option<&str> = None;
    let mut fence: Option<Vec<&str>> = None;
    let mut text: Option<Fence> = None;
    for line in src.lines() {
        if let Some(lines) = fence.as_mut() {
            if !line.starts_with("```") {
                lines.push(line);
                continue;
            }
            let block = fence_rows(name, &fence.take().unwrap_or_default());
            match heading.take() {
                Some("TEXT") => text = Some(block),
                Some("STYLE") => {
                    let Some(text) = text.take() else {
                        panic!("board {name}: a STYLE fence before its TEXT fence");
                    };
                    let parts = Parts {
                        caption: caption.as_ref(),
                        base: base.as_ref(),
                        captions: std::mem::take(&mut captions),
                    };
                    frames.push(pair(name, text, block, parts));
                }
                _ => panic!("board {name}: a fence under no TEXT or STYLE heading"),
            }
            continue;
        }
        if line.starts_with("```") {
            fence = Some(Vec::new());
        } else if line == "TEXT" || line == "STYLE" {
            heading = Some(line);
        } else if line.starts_with('*') {
            caption = Some(caption_line(name, line));
        } else if let Some(rest) = line.strip_prefix("Rows that differ from ") {
            base = Some(base_line(name, rest));
        } else if let Some(rest) = line.strip_prefix("- `") {
            let (number, words) = rest.split_once("` ").unwrap_or((rest, ""));
            let Ok(number) = number.parse::<u16>() else {
                panic!("board {name}: a caption line without a row number: {line:?}");
            };
            captions.push((number, words.to_string()));
        }
    }
    assert!(fence.is_none(), "board {name}: an unclosed fence");
    assert!(text.is_none(), "board {name}: a TEXT fence without its STYLE fence");
    assert!(!frames.is_empty(), "board {name}: no frames");
    Board { name: name.to_string(), frames }
}

/// `*ALL PEERS · IDLE · 100×30 · terminal window, …*`: the caption runs to
/// the first part that is a size.
fn caption_line(name: &str, line: &str) -> (String, (u16, u16)) {
    let parts: Vec<&str> = line.trim_matches('*').split(" · ").collect();
    let Some(at) = parts.iter().position(|part| size(part).is_some()) else {
        panic!("board {name}: a caption line without a size: {line:?}");
    };
    (parts[..at].join(" · "), size(parts[at]).unwrap_or_default())
}

fn size(part: &str) -> Option<(u16, u16)> {
    let (w, h) = part.split_once('×')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// `ALL PEERS · IDLE: 6, 10, 11, 27. Every other row is as ALL PEERS · IDLE.`
fn base_line(name: &str, rest: &str) -> (String, Vec<u16>) {
    let parsed = rest.split_once(": ").and_then(|(base, rows)| {
        let rows = rows.split_once('.')?.0;
        let rows: Option<Vec<u16>> = rows.split(", ").map(|row| row.parse().ok()).collect();
        Some((base.to_string(), rows?))
    });
    parsed.unwrap_or_else(|| panic!("board {name}: a malformed base line: {rest:?}"))
}

fn fence_rows(name: &str, lines: &[&str]) -> Fence {
    let [tens, units, rows @ ..] = lines else {
        panic!("board {name}: a fence without its two ruler lines");
    };
    let digits = |ruler: &str| -> Vec<u16> {
        let Some(digits) = ruler.strip_prefix("    ") else {
            panic!("board {name}: a ruler line without its 4-cell margin: {ruler:?}");
        };
        let digit = |c: char| c.to_digit(10).map(|d| d as u16);
        let all: Option<Vec<u16>> = digits.chars().map(digit).collect();
        all.unwrap_or_else(|| panic!("board {name}: a ruler of other than digits: {ruler:?}"))
    };
    let (tens, units) = (digits(tens), digits(units));
    assert!(!tens.is_empty() && tens.len() == units.len(), "board {name}: unequal ruler lines");
    let x0 = tens[0] * 10 + units[0];
    for (i, (t, u)) in tens.iter().zip(&units).enumerate() {
        assert_eq!(t * 10 + u, x0 + i as u16, "board {name}: the ruler skips at its cell {i}");
    }
    let width = tens.len();
    let rows = rows
        .iter()
        .map(|line| {
            let (Some(head), Some(cells)) = (line.get(..4), line.get(4..)) else {
                panic!("board {name}: a row line too short: {line:?}");
            };
            let number = head[..2].trim().parse::<u16>();
            let (Ok(number), "  ") = (number, &head[2..]) else {
                panic!("board {name}: a row line without its number and 2 spaces: {line:?}");
            };
            let count = cells.chars().count();
            let what = format!("board {name}: row {number} holds {count} cells, its ruler {width}");
            assert_eq!(count, width, "{what}");
            (number, cells.to_string())
        })
        .collect();
    Fence { x0, width, rows }
}

/// What a frame takes from the lines around its fences.
struct Parts<'a> {
    caption: Option<&'a (String, (u16, u16))>,
    base: Option<&'a (String, Vec<u16>)>,
    captions: Vec<(u16, String)>,
}

fn pair(name: &str, text: Fence, style: Fence, parts: Parts) -> Frame {
    assert_eq!((text.x0, text.width), (style.x0, style.width), "board {name}: unequal rulers");
    let numbers = |fence: &Fence| fence.rows.iter().map(|(n, _)| *n).collect::<Vec<_>>();
    let own = numbers(&text);
    assert_eq!(own, numbers(&style), "board {name}: TEXT and STYLE give other rows");
    let rows: Vec<(u16, String, String)> = text
        .rows
        .into_iter()
        .zip(style.rows)
        .map(|((number, text), (_, style))| {
            if let Some(code) = style.chars().find(|c| !CODES.contains(*c)) {
                panic!("board {name}: row {number} holds the unknown code {code:?}");
            }
            (number, text, style)
        })
        .collect();
    let base = parts.base.map(|(label, listed)| {
        assert_eq!(listed, &own, "board {name}: the rows it says differ are not the rows it gives");
        label.clone()
    });
    let (label, size) = match parts.caption {
        Some((caption, size)) => (caption.clone(), Some(*size)),
        None => (text.width.to_string(), None),
    };
    Frame { label, x0: text.x0, rows, size, base, own, captions: parts.captions }
}

/// A derived frame's other rows, from the first frame of its board.
fn fill_from(name: &str, frame: &mut Frame, base: &str) {
    let found = PEER_RECS
        .iter()
        .flat_map(|(tag, src)| parse(tag, src).frames)
        .find(|f| f.full() && f.base.is_none() && f.label == base);
    let Some(found) = found else {
        panic!("board {name}: no full frame {base:?} to fill its other rows from");
    };
    for row in found.rows {
        if !frame.rows.iter().any(|(n, _, _)| *n == row.0) {
            frame.rows.push(row);
        }
    }
    frame.rows.sort_by_key(|(n, _, _)| *n);
}

// ── Matching ─────────────────────────────────────────────────────────────────

/// The code the drawn cell must read as, or `None` when it is not compared.
fn expected(style: char, text: char) -> Option<char> {
    match style {
        'c' => None,
        '1'..='9' if !truecolor() => Some('d'),
        's' if text == ' ' => Some('S'),
        code => Some(code),
    }
}

fn symbol_matches(text: char, symbol: &str) -> bool {
    if text == ' ' {
        symbol.trim().is_empty()
    } else {
        let mut chars = symbol.chars();
        chars.next() == Some(text) && chars.next().is_none()
    }
}

/// A failed comparison: its first differing cell, and the report to print.
struct Mismatch {
    x: u16,
    y: u16,
    report: String,
}

fn diff(buf: &Buffer, board: &Board, frame: &Frame, area: &Area) -> Option<Mismatch> {
    assert!(
        frame.full() || area.y_at.is_some(),
        "board {} frame {:?} is a region: its Area needs y_at",
        board.name,
        frame.label
    );
    let picked: Vec<&(u16, String, String)> = frame
        .rows
        .iter()
        .filter(|(n, _, _)| area.rows.as_ref().is_none_or(|rows| rows.contains(n)))
        .collect();
    let (name, label) = (&board.name, &frame.label);
    assert!(!picked.is_empty(), "board {name} frame {label:?}: the area holds no mock row");
    let first_row = picked[0].0;
    let mut first: Option<(u16, u16, String)> = None;
    let mut rows = String::new();
    let mut differing = 0;
    for (number, text, style) in picked {
        let y = area.y_at.map_or(*number, |at| at + (number - first_row));
        let Some((x, why)) = first_cell(buf, frame, area, y, text, style) else { continue };
        differing += 1;
        rows.push_str(&row_report(buf, frame, *number, y, text, style, x));
        if first.is_none() {
            first = Some((x, y, why));
        }
    }
    let (x, y, why) = first?;
    let head = format!(
        "board {} frame {:?}: {differing} row(s) differ; the first cell at x {x}, y {y}: {why}",
        board.name, frame.label
    );
    Some(Mismatch { x, y, report: format!("{head}\n{rows}") })
}

/// The first cell of one row that differs, and how.
fn first_cell(buf: &Buffer, frame: &Frame, area: &Area, y: u16, text: &str, style: &str)
    -> Option<(u16, String)>
{
    for (i, (want_text, want_style)) in text.chars().zip(style.chars()).enumerate() {
        let x = frame.x0 + i as u16;
        if area.cols.as_ref().is_some_and(|cols| !cols.contains(&x)) {
            continue;
        }
        let Some(want) = expected(want_style, want_text) else { continue };
        let Some(cell) = buf.cell((x, y)) else {
            return Some((x, "the cell is outside the buffer".to_string()));
        };
        let got = style_code(cell, &ds_hue);
        if !symbol_matches(want_text, cell.symbol()) || got != want {
            let why = format!(
                "mock {want_text:?} {want_style:?}, drawn {:?} {got:?} (fg {:?}, bg {:?}, {:?})",
                cell.symbol(),
                cell.fg,
                cell.bg,
                cell.modifier
            );
            return Some((x, why));
        }
    }
    None
}

/// One differing row, mock over drawn, TEXT then STYLE, and a caret under
/// its first differing cell.
fn row_report(buf: &Buffer, frame: &Frame, number: u16, y: u16, text: &str, style: &str, x: u16)
    -> String
{
    let cells = (0..frame.width() as u16).map(|i| buf.cell((frame.x0 + i, y)));
    let drawn_text: String = cells
        .clone()
        .map(|cell| match cell {
            Some(cell) if cell.symbol().is_empty() => " ".to_string(),
            Some(cell) => cell.symbol().to_string(),
            None => "¤".to_string(),
        })
        .collect();
    let drawn_style: String =
        cells.map(|cell| cell.map_or('¤', |cell| style_code(cell, &ds_hue))).collect();
    let caption = frame
        .captions
        .iter()
        .find(|(n, _)| *n == number)
        .map(|(_, words)| format!(" ({words})"))
        .unwrap_or_default();
    let caret = " ".repeat(usize::from(x - frame.x0));
    format!(
        "row {number} at y {y}, columns from x {x0}{caption}\n  \
         mock  TEXT  │{text}│\n  \
         drawn TEXT  │{drawn_text}│\n  \
         mock  STYLE │{style}│\n  \
         drawn STYLE │{drawn_style}│\n  \
         {pad}{caret}^ x {x}\n",
        x0 = frame.x0,
        pad = " ".repeat(13),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    /// The style a mock code stands for, from the kit's own tokens: the
    /// painter the round trip and the synthetic boards draw with.
    fn paint_style(code: char) -> Style {
        let t = th();
        let plain = Style::default();
        let bold = |style: Style| style.add_modifier(Modifier::BOLD);
        match code {
            't' => plain.fg(t.text),
            'T' => bold(plain.fg(t.text)),
            'd' => plain.fg(t.dim),
            'a' => plain.fg(t.accent),
            'A' => bold(plain.fg(t.accent)),
            'g' => plain.fg(t.gold),
            'G' => bold(plain.fg(t.gold)),
            'o' => plain.fg(t.ok),
            'O' => bold(plain.fg(t.ok)),
            'S' => bold(plain.bg(t.accent).fg(t.on_accent)),
            's' => plain.bg(t.accent).fg(t.on_accent),
            '%' => bold(plain.bg(t.accent).fg(swatch(Some(352)))),
            '1'..='9' => plain.fg(swatch(ds_hue(code))),
            _ => plain,
        }
    }

    /// Draw a frame's own cells into `buf`, mock row `n` at `y_of(n)`.
    fn paint(buf: &mut Buffer, frame: &Frame, y_of: impl Fn(u16) -> u16) {
        for (number, text, style) in &frame.rows {
            for (i, (glyph, code)) in text.chars().zip(style.chars()).enumerate() {
                let cell = &mut buf[(frame.x0 + i as u16, y_of(*number))];
                cell.set_char(glyph);
                cell.set_style(paint_style(code));
            }
        }
    }

    fn cell(symbol: &str, style: Style) -> Cell {
        let mut cell = Cell::default();
        cell.set_symbol(symbol);
        cell.set_style(style);
        cell
    }

    /// A small region board holding every code, at x 2–13.
    const SAMPLE: &str = "\
<!-- MOCK:SAMPLE -->
- `01` every colour
- `02` the slab, a cover and blanks

TEXT

```text
    000000001111
    234567890123
01  xxxxxxxxx ▌▌
02  ab c▌      z
```

STYLE

```text
    000000001111
    234567890123
01  tTdaAgGoO 12
02  SSSs%  ccc d
```
<!-- /MOCK:SAMPLE -->
";

    fn sample() -> Board {
        parse("SAMPLE", SAMPLE)
    }

    /// The sample's two rows painted at y 3 and 4 of a 16×8 buffer.
    fn painted_sample() -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 16, 8));
        paint(&mut buf, &sample().frames[0], |n| n + 2);
        buf
    }

    fn sample_diff(buf: &Buffer, area: Area) -> Option<(u16, u16)> {
        let board = sample();
        diff(buf, &board, &board.frames[0], &area).map(|m| (m.x, m.y))
    }

    #[test]
    fn every_board_parses_with_equal_text_and_style_shapes() {
        let mut tags: Vec<&str> = PEER_RECS.iter().map(|(tag, _)| *tag).collect();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), 25, "every mock block, once");
        for (tag, _) in PEER_RECS {
            let board = board(tag);
            assert_eq!(board.name, tag);
            for frame in &board.frames {
                let width = frame.width();
                let at = |n: u16| format!("{tag} {:?} row {n}", frame.label);
                for (n, text, style) in &frame.rows {
                    assert_eq!(text.chars().count(), width, "{}", at(*n));
                    assert_eq!(style.chars().count(), width, "{}", at(*n));
                    for (glyph, code) in text.chars().zip(style.chars()) {
                        if glyph == ' ' {
                            assert!(" cSs".contains(code), "{}: a space styled {code:?}", at(*n));
                        } else {
                            assert!(!" c".contains(code), "{}: {glyph:?} has no style", at(*n));
                        }
                    }
                }
                if let Some((w, h)) = frame.size {
                    assert_eq!((frame.x0, width), (0, usize::from(w)), "{tag}: a full frame");
                    let numbers: Vec<u16> = frame.rows.iter().map(|(n, _, _)| *n).collect();
                    assert_eq!(numbers, (0..h).collect::<Vec<_>>(), "{tag}: every row, once");
                    assert!(frame.own.iter().all(|n| numbers.contains(n)), "{tag}");
                    assert!(!frame.label.is_empty() && frame.captions.is_empty(), "{tag}");
                } else {
                    assert_eq!(frame.label, width.to_string(), "{tag}: a region is labelled by width");
                    assert!(usize::from(frame.x0) + width <= 100, "{tag}");
                    // A caption names a variant; the grid's narrow pair is one, of two rows.
                    assert!(!frame.captions.is_empty(), "{tag} {:?}: no caption", frame.label);
                    for (c, _) in &frame.captions {
                        let given = frame.rows.iter().any(|(n, _, _)| n == c);
                        assert!(given, "{}: a caption, no row", at(*c));
                    }
                }
            }
            let labels: Vec<&str> = board.frames.iter().map(|f| f.label.as_str()).collect();
            if tag.starts_with("REG-") {
                let wide = if tag == "REG-GRID" { "78" } else { "80" };
                assert_eq!(labels, [wide, "47"], "{tag}: the wide frame, then the queue-open slice");
            } else {
                assert_eq!(labels.len(), 1, "{tag}: one full frame");
            }
        }
        // A derived frame is its base with its own rows over it.
        let (m1, m2) = (board("M1"), board("M2"));
        let (m1, m2) = (&m1.frames[0], &m2.frames[0]);
        assert_eq!((m1.label.as_str(), m1.size), ("ALL PEERS · IDLE", Some((100, 30))));
        assert_eq!(m2.label, "PREVIEW PLAYING");
        assert_eq!(m2.base.as_deref(), Some("ALL PEERS · IDLE"));
        assert_eq!(m2.own, [6, 10, 11, 27]);
        assert_eq!(m2.rows[9], m1.rows[9], "a row M2 does not give is M1's");
        assert_ne!(m2.rows[6], m1.rows[6], "a row it gives is its own");
        assert_eq!(board("W2").frames[0].size, Some((100, 24)));
        assert_eq!(board("L3").frames[0].base.as_deref(), Some("GET IT · DOWNLOADING"));
        assert_eq!(board("V4").frames[0].base.as_deref(), Some("MEMBER ACCOUNT"));
        assert_eq!(board("S6").frames[0].base.as_deref(), Some("NO PEERS YET"));
        let strip = board("REG-STRIP");
        assert_eq!(strip.frames[0].rows.len(), 8);
        assert_eq!(strip.frames[0].x0, 17);
        assert!(strip.frames[0].captions[2].1.starts_with("admin, Kitchen Radio selected"));
    }

    /// Every board's every frame, painted from its own codes with the kit's
    /// tokens, matches itself: the parser, the painter and the matcher agree
    /// on whatever tier this process resolved.
    #[test]
    fn every_board_matches_a_buffer_painted_from_it() {
        for (tag, _) in PEER_RECS {
            for frame in &board(tag).frames {
                match frame.size {
                    Some((w, h)) => {
                        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
                        paint(&mut buf, frame, |n| n);
                        assert_frame(&buf, tag, &frame.label, Area::whole());
                    }
                    None => {
                        let first = frame.rows[0].0;
                        let height = frame.rows.len() as u16 + 4;
                        let mut buf = Buffer::empty(Rect::new(0, 0, 100, height));
                        paint(&mut buf, frame, |n| 2 + n - first);
                        let area = Area { y_at: Some(2), ..Area::whole() };
                        assert_frame(&buf, tag, &frame.label, area);
                        for (n, _, _) in &frame.rows {
                            assert_frame(&buf, tag, &frame.label, Area::at(2 + n - first, *n));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_matcher_reads_codes_from_the_theme() {
        let t = th();
        for code in "tTdaAgGoOSs".chars() {
            assert_eq!(style_code(&cell("x", paint_style(code)), &ds_hue), code, "{code:?}");
        }
        let swatch_cell = cell("▌", paint_style('4'));
        assert_eq!(style_code(&swatch_cell, &ds_hue), if truecolor() { '4' } else { 'd' });
        assert_eq!(style_code(&cell("▌", paint_style('%')), &ds_hue), '%');
        let dim_on_slab = Style::default().bg(t.accent).fg(swatch(None));
        assert_eq!(style_code(&cell("▌", dim_on_slab), &ds_hue), '%', "a lead without a tile");
        // A caller's own hue table.
        let own = |code: char| (code == '1').then_some(100);
        let hue_100 = cell("▌", Style::default().fg(swatch(Some(100))));
        assert_eq!(style_code(&hue_100, &own), if truecolor() { '1' } else { 'd' });
        // What cannot show is not read.
        assert_eq!(style_code(&cell(" ", paint_style('G')), &ds_hue), ' ', "a space's colour");
        assert_eq!(style_code(&cell(" ", paint_style('s')), &ds_hue), 'S', "a blank slab's BOLD");
        assert_eq!(style_code(&cell("x", Style::default().fg(Color::Reset)), &ds_hue), 't');
        let dim_bold = Style::default().fg(t.dim).add_modifier(Modifier::BOLD);
        assert_eq!(style_code(&cell("x", dim_bold), &ds_hue), 'd', "dim has no BOLD code");
        // What no board holds reads as `?`.
        assert_eq!(style_code(&cell("x", Style::default().fg(t.bright)), &ds_hue), '?', "hover");
        assert_eq!(style_code(&cell("x", Style::default().fg(t.danger)), &ds_hue), '?');
        let underlined = paint_style('d').add_modifier(Modifier::UNDERLINED);
        assert_eq!(style_code(&cell("x", underlined), &ds_hue), '?', "no underline anywhere");
        let on_gold = Style::default().fg(t.text).bg(t.gold);
        assert_eq!(style_code(&cell("x", on_gold), &ds_hue), '?', "no other ground");
        let gold_on_slab = Style::default().bg(t.accent).fg(t.gold);
        assert_eq!(style_code(&cell("x", gold_on_slab), &ds_hue), '?');
        // The expectation follows the tier, as the drawing does.
        assert_eq!(expected('3', '▌'), Some(if truecolor() { '3' } else { 'd' }));
        assert_eq!(expected('c', ' '), None, "under the art");
        assert_eq!(expected('s', ' '), Some('S'));
        assert_eq!(expected('s', 'x'), Some('s'));
    }

    #[test]
    fn the_matcher_reports_the_first_differing_cell() {
        let mut buf = painted_sample();
        assert_eq!(sample_diff(&buf, Area { y_at: Some(3), ..Area::whole() }), None);
        // Under the cover anything goes.
        buf[(9u16, 4u16)].set_char('#').set_style(paint_style('G'));
        assert_eq!(sample_diff(&buf, Area { y_at: Some(3), ..Area::whole() }), None);
        // A wrong glyph on row 02, then a wrong colour on row 01 further left.
        buf[(6u16, 4u16)].set_char('Z');
        buf[(7u16, 3u16)].set_style(paint_style('t'));
        let board = sample();
        let area = Area { y_at: Some(3), ..Area::whole() };
        let Some(mismatch) = diff(&buf, &board, &board.frames[0], &area) else {
            panic!("two cells differ");
        };
        assert_eq!((mismatch.x, mismatch.y), (7, 3), "the first in row order");
        let report = &mismatch.report;
        assert!(report.starts_with("board SAMPLE frame \"12\": 2 row(s) differ"), "{report}");
        assert!(report.contains("x 7, y 3: mock 'x' 'g', drawn \"x\" 't'"), "{report}");
        assert!(report.contains("  mock  TEXT  │xxxxxxxxx ▌▌│"), "{report}");
        assert!(report.contains("  mock  STYLE │tTdaAgGoO 12│"), "{report}");
        assert!(report.contains("  drawn STYLE │tTdaAtGoO "), "{report}");
        assert!(report.contains("  drawn TEXT  │ab cZ"), "{report}");
        assert!(report.contains("row 2 at y 4, columns from x 2 (the slab, a cover and blanks)"));
        let caret = format!("\n  {}{}^ x 7\n", " ".repeat(13), " ".repeat(5));
        assert!(report.contains(&caret), "the caret stands under x 7:\n{report}");
        // assert_frame panics with that report, naming the board's frame.
        let blank = Buffer::empty(Rect::new(0, 0, 100, 30));
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_frame(&blank, "M1", "", Area::whole())
        }));
        let payload = failed.expect_err("a blank screen is not M1");
        let message = payload.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(message.starts_with("board M1 frame \"ALL PEERS · IDLE\":"), "{message}");
        assert!(message.contains("the first cell at x 1, y 0"), "{message}");
    }

    #[test]
    fn an_area_limits_the_comparison_to_its_rows_and_columns() {
        let mut buf = painted_sample();
        buf[(2u16, 3u16)].set_char('?');
        buf[(12u16, 4u16)].set_style(paint_style('S'));
        assert_eq!(sample_diff(&buf, Area { y_at: Some(3), ..Area::whole() }), Some((2, 3)));
        // y_at places the first compared mock row.
        assert_eq!(sample_diff(&buf, Area::at(4, 2)), Some((12, 4)));
        assert_eq!(sample_diff(&buf, Area { cols: Some(2..=9), ..Area::at(4, 2) }), None);
        assert_eq!(sample_diff(&buf, Area { cols: Some(3..=13), ..Area::at(3, 1) }), None);
        // The same variant drawn a row lower is held where the area says.
        let mut low = Buffer::empty(Rect::new(0, 0, 16, 8));
        paint(&mut low, &sample().frames[0], |n| n + 5);
        assert_eq!(sample_diff(&low, Area::at(6, 1)), None);
        assert!(sample_diff(&low, Area::at(3, 1)).is_some());
        // A full frame: the lead sheet's rect, at 100×30 and moved up for 100×24.
        let m3 = board("M3");
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        paint(&mut buf, &m3.frames[0], |n| n);
        buf[(5u16, 3u16)].set_char('!');
        let sheet = Area::rect(17, 82, 9, 20);
        let check = |buf: &Buffer, area: &Area| {
            diff(buf, &m3, &m3.frames[0], area).map(|m| (m.x, m.y))
        };
        assert_eq!(check(&buf, &Area::whole()), Some((5, 3)));
        assert_eq!(check(&buf, &sheet), None, "the stray cell is outside the sheet");
        buf[(40u16, 15u16)].set_char('!');
        assert_eq!(check(&buf, &sheet), Some((40, 15)));
        let mut floor = Buffer::empty(Rect::new(0, 0, 100, 24));
        paint(&mut floor, &m3.frames[0], |n| if (9..=20).contains(&n) { n - 3 } else { 23 });
        assert_eq!(check(&floor, &Area { y_at: Some(6), ..sheet.clone() }), None);
        assert_eq!(check(&floor, &sheet).map(|(_, y)| y), Some(9), "unmoved, it is not there");
    }
}
