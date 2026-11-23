//! An SD card in this computer's reader drawn by egui (contract clauses
//! 20a, 25, 44–58): its Card view in the column, as the board's card stands
//! there, and its gates over the page's area and the kit's scrim.
//!
//! The Card view's rows are the cells' own (cells/card.rs's `card_layout`,
//! made with a value column too wide to wrap in), set in points on the
//! board card's grid after the canvas's Card, Syncing and Synced boards:
//! the head, then the value rows 18 points apart — Holds, Space and its
//! meter, Last sync, Next sync, Converts, or a sync's Syncing and Now —
//! each label on its value's first line, a plain value in 13.5 points, a
//! verdict behind the kit's LED, an explanation dim, a scan as the kit's
//! searching shape with its share, the figures in the monospace and the
//! card's drive letter with them; then the bottom row (board.rs's
//! `Paint`), and Details with its facts, the card's own log and Sync
//! again. Short of room the rows give way as the cells give them way
//! (`give_way_rows`), measured in points: the dim explanation lines first,
//! the last first, then Converts; then the values are cut whole lines at a
//! time above the bottom row, which is never cut. The bottom row's links
//! give way as the cells' do (`give_way`): Send the plays first, then the
//! links from the end. Details' Drive fact gives way a piece at a time,
//! its serial first and then its clusters, and its Removes fact cuts its
//! path before its count, never the count.
//!
//! A card's gates are the kit's gate whose body is the page's own
//! (`studio::gate_with`), over the page's area: its title and its
//! sentences in the cells' words and groups (`gate_body`'s), the keep lit
//! with the cells' back arrow as its lead and the go as the hairline, as
//! the board's gate answers. The stronger gate (RemoveGate.dc.html) has its
//! box in its body, the kit's check line, and its go only once the box is
//! ticked, as the model holds it; a gate opened by another's answer opens
//! at its body's top, and swallows what comes within its guard
//! (`CHAIN_GUARD`) in the model, which every click here goes through.

use egui::{Align2, Color32, FontId, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use ratatui::style::Style;
use ratatui::text::Line;

use super::super::card::{CardGate, GateKind, plan_of, span};
use super::super::cells::{self, Row};
use super::super::*;
use super::board::{APART, CLEAR, DIM, FACT_H, FIGURE, FIRST as VERDICT, Grid, LINE_GAP, Lined, METER_GAP, METER_ROW, PLAIN, Paint, SCAN, WIDE, clipped, fixed_h, lead_of};
use super::{Drawn, Face, tone, unled};
use crate::device::card::worker::CardView;
use crate::gui::window::studio::{self, HEAD_H, Para, Run, TALL_H, TEXT_BUTTON_H, body, mono, strong};

/// A value's lines are the board card's (board.rs's measures: a plain
/// value 13.5 on 20, a verdict behind its LED 14 on 20, an explanation's
/// dim line 13 on 18 — on 20 where it is a row's first — a scan's words 13
/// on 20; each 4 points under the last, 6 under a meter), and the file in
/// flight's in the monospace, 12.5 on 20.
const NOW: (f32, f32) = (12.5, 20.0);

/// A gate's sentences: 14 points at the kit's 1.3 leading, a group the
/// kit's gate's 6 points under the last; a figure in them the monospace at
/// 13, as the boards set a library's count.
const SAY: f32 = 14.0;
const SAY_LINE: f32 = SAY * 1.3;
const SAY_FIGURE: f32 = 13.0;
/// A library's name and its count, the cells' two spaces apart.
const LIB_APART: f32 = 8.0;

// ── The Card view ─────────────────────────────────────────────────────────

/// The card `view` in the column's `room` (the cells' `card_view`): its
/// head, its value rows, its bottom row, and Details while open, in a
/// hairline frame as tall as its rows and no taller than the room. Its rest
/// is its height through its bottom row with every row it has, Details
/// aside, as the frame measures it; the measuring run (`face.live` false)
/// draws no control and gives nothing way.
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face, view: &CardView, room: Rect) -> Drawn {
    let s = face.space;
    let grid = Grid::of(room, face);
    let laid = Laid::of(cells::card::card_layout(page, view, WIDE));
    let drive = view.drive();
    let actions = links_give_way(ui, face, grid, laid.actions);
    let actions_h = match &actions {
        Row::Actions { primary: Some(_), .. } => TALL_H,
        _ => TEXT_BUTTON_H,
    };
    let mut values = laid.values;
    if face.live {
        // Short of room, the rows the cells give way go first, measured in
        // points with Details' rows (but its log, which takes what is left)
        // as the cells measure them.
        let details_h = if page.details { laid.details.iter().map(|r| fixed_h(r, face)).sum::<f32>() } else { 0.0 };
        let fixed = 2.0 * s.pad_y + HEAD_H + s.gap + actions_h + details_h;
        let mut probe = ui.new_child(UiBuilder::new().id_salt("mp3-card-measure").max_rect(room));
        probe.set_invisible();
        let measure = Values { ui: &probe, face, grid, floor: f32::INFINITY, drive: &drive };
        while fixed + measure.paint(0.0, &values) > room.height() + 0.5 && give_way(&mut values) {}
    }
    let card = clipped(ui, room, "mp3-card");
    let y = studio::card_head(&card, pos2(grid.x, room.min.y + s.pad_y), grid.w, &laid.name, &laid.right, face.pal);
    // The measuring run sees the card whole; the drawing cuts the values
    // short of the bottom row's place at the room's foot.
    let foot = if face.live { room.max.y - s.pad_y } else { f32::INFINITY };
    let y = Values { ui: &card, face, grid, floor: foot - actions_h - s.gap, drive: &drive }.paint(y, &values);
    let mut acts = Vec::new();
    let mut paint = Paint::new(&card, face, grid, y, foot, Id::new("mp3-card"), &mut acts);
    paint.blank();
    paint.row(page, &view.key, &actions, 0.0);
    let rest = paint.y + s.pad_y - room.min.y;
    if page.details && face.live {
        details(&mut paint, page, &card, face, grid, view, &laid.details);
    }
    let bottom = (paint.y + s.pad_y).min(room.max.y);
    studio::hairline(&card, Rect::from_min_max(room.min, pos2(room.max.x, bottom)), face.pal);
    Drawn { acts, caret: None, rest }
}

/// The cells' rows of a card's view, in the parts the egui face sets each
/// its own way: the head's name and its right edge, the value rows, the
/// bottom row, and Details' rows (none while it is shut).
struct Laid {
    name: String,
    right: String,
    values: Vec<Row>,
    actions: Row,
    details: Vec<Row>,
}

impl Laid {
    /// `rows` as `card_layout` makes them: a blank, the head, a blank, the
    /// value rows, a blank, the bottom row, Details' rows, a blank.
    fn of(mut rows: Vec<Row>) -> Laid {
        let at = rows.iter().position(|r| matches!(r, Row::Actions { .. })).unwrap_or(rows.len());
        let mut details: Vec<Row> = rows.split_off(at);
        let actions = if details.is_empty() { Row::Actions { links: Vec::new(), primary: None, covered: false } } else { details.remove(0) };
        // The card's foot padding is the frame's.
        if matches!(details.last(), Some(Row::Blank)) {
            details.pop();
        }
        let (mut name, mut right) = (String::new(), String::new());
        let mut values = Vec::new();
        for row in rows {
            match row {
                Row::Wide { left, right: words } if name.is_empty() => {
                    name = left.to_string();
                    right = words.map(|w| w.to_string()).unwrap_or_default();
                }
                Row::Blank => {}
                row => values.push(row),
            }
        }
        Laid { name, right, values, actions, details }
    }
}

/// The cells' give-way of the card's rows (cells/card.rs's
/// `give_way_rows`), one step: the last dim explanation line goes, else
/// the Converts row and the lines under it; never the head, Holds, Space
/// or the bottom row. Whether anything went.
fn give_way(rows: &mut Vec<Row>) -> bool {
    let dim = crate::kit::dim();
    let dim_line = |row: &Row| match row {
        Row::Value { label: None, left, right: None } => !left.spans.is_empty() && left.spans.iter().all(|s| s.style == dim),
        _ => false,
    };
    if let Some(at) = rows.iter().rposition(dim_line) {
        rows.remove(at);
        return true;
    }
    let converts = t!("dev.card.label_converts").to_string();
    let Some(at) = rows.iter().position(|r| matches!(r, Row::Value { label: Some(l), .. } if *l == converts)) else { return false };
    rows.remove(at);
    while matches!(rows.get(at), Some(Row::Value { label: None, right: None, .. })) {
        rows.remove(at);
    }
    true
}

/// The bottom row's links given way in points as the cells give them way
/// in cells (cells/card.rs's `give_way`): where they would run past the
/// room the primary leaves them, Send the plays goes first. `Paint` then
/// draws no link that would run under the primary, from the end.
fn links_give_way(ui: &Ui, face: &Face, grid: Grid, row: Row) -> Row {
    let Row::Actions { mut links, primary, covered } = row else { return row };
    let primary_w = primary.as_ref().map_or(0.0, |(label, enabled, _)| studio::tall_button_width(ui, label, *enabled).min(grid.w) + APART);
    let widths: f32 = links
        .iter()
        .map(|(text, _)| {
            let (lead, words) = lead_of(text);
            studio::text_button_width(ui, words, lead, face.ports)
        })
        .sum();
    let width = widths + APART * links.len().saturating_sub(1) as f32;
    if width > grid.w - primary_w
        && let Some(at) = links.iter().position(|(_, act)| *act == Act::CardPlays)
    {
        links.remove(at);
    }
    Row::Actions { links, primary, covered }
}

// ── The value rows ────────────────────────────────────────────────────────

/// What a value row's line is, read off the cells' spans.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A plain value, its words in their colours.
    Plain,
    /// Every word dim: an explanation.
    Dim,
    /// Led by a verdict's glyph (✓ ! ✗ ?), the kit's LED in its colour.
    Verdict(Color32),
    /// The cells' scan widget at its head: its lit cells of ten, and the
    /// share after them where there is one.
    Scan(usize),
}

/// A cells' line read for the canvas (board.rs's [`Lined`], the board
/// card's reader): its kind, and its words, each piece in its colour, the
/// bold ones semibold.
struct Read {
    kind: Kind,
    lined: Lined,
}

impl Read {
    fn of(line: &Line, face: &Face) -> Read {
        let lined = Lined::of(line, face);
        let kind = match (&lined.meter, lined.lamp) {
            (Some((lit, _)), _) => Kind::Scan(*lit),
            (None, Some(lamp)) => Kind::Verdict(lamp),
            (None, None) if !lined.spans.is_empty() && lined.dim(face) => Kind::Dim,
            (None, None) => Kind::Plain,
        };
        Read { kind, lined }
    }

    /// The scan's share, where there is one.
    fn share(&self) -> Option<&str> {
        self.lined.meter.as_ref().and_then(|(_, share)| share.as_deref())
    }

    /// The words as runs at `size`, the bold ones semibold and, `figures`
    /// asked, the figures in the monospace, the card's `drive` among them.
    fn runs(&self, size: f32, figures: bool, face: &Face, drive: &str) -> Vec<Run> {
        let font = |bold: bool| if bold { strong(size) } else { body(size) };
        let mut runs = Vec::new();
        for (k, (s, color, bold)) in self.lined.spans.iter().enumerate() {
            let words = if k == 0 { s.trim_start() } else { s.as_str() };
            if figures {
                runs.extend(figured(words, face.ports, drive, font(*bold), mono(FIGURE), *color));
            } else {
                runs.push((words.to_string(), font(*bold), *color));
            }
        }
        runs
    }
}

/// Whether `drive` is a drive letter (`E:`): a figure, which the boards set
/// in the monospace wherever a card's words name it (Card.dc.html's Drive
/// fact), as the subjects row sets it on the card's tab. A volume's name
/// (Linux, macOS) is a word.
fn letter(drive: &str) -> bool {
    let b = drive.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// `words` as runs in `font` and `color`, their figures in `figure` — the
/// kit's (`studio::figured`, the boards' `ports` among them) and the card's
/// `drive` where it is a [`letter`] standing as a word of its own, which
/// the kit's cannot take: its figures end before a colon.
fn figured(words: &str, ports: &[&str], drive: &str, font: FontId, figure: FontId, color: Color32) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut rest = words;
    while let Some(at) = letter(drive).then(|| word_at(rest, drive)).flatten() {
        runs.extend(studio::figured(&rest[..at], ports, font.clone(), figure.clone(), color));
        runs.push((drive.to_string(), figure.clone(), color));
        rest = &rest[at + drive.len()..];
    }
    runs.extend(studio::figured(rest, ports, font, figure, color));
    runs
}

/// Where `word` first stands in `s` as a word of its own: at its start or
/// after a space, and before a space, a mark or its end.
fn word_at(s: &str, word: &str) -> Option<usize> {
    s.match_indices(word).map(|(at, _)| at).find(|&at| {
        let before = s[..at].chars().next_back().is_none_or(char::is_whitespace);
        let after = s[at + word.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

/// The Card view's value rows set in points on a card's [`Grid`], none
/// past `floor`: a row that would cross it is left out whole, a
/// paragraph's lines past it too, and nothing after it is drawn. `drive`
/// is the card's, a figure where its words name it.
struct Values<'u, 'f> {
    ui: &'u Ui,
    face: &'f Face<'f>,
    grid: Grid,
    floor: f32,
    drive: &'f str,
}

/// What the last row drawn was: the next row's gap follows it.
#[derive(Clone, Copy, PartialEq)]
enum Was {
    Line,
    Meter,
}

impl Values<'_, '_> {
    /// `rows` from `y`, each labelled row a block's gap under what came
    /// before (the boards keep the board card's 18 points between the Card
    /// view's rows, which the cells set one under another), a line under
    /// its block's last 4 points, 6 under a meter. Returns where the next
    /// row starts.
    fn paint(&self, mut y: f32, rows: &[Row]) -> f32 {
        let mut last = None;
        for row in rows {
            let labelled = matches!(row, Row::Value { label: Some(_), .. } | Row::Bar { label: Some(_), .. });
            let gap = match last {
                _ if labelled => self.face.space.gap,
                None => self.face.space.gap,
                Some(Was::Meter) => METER_GAP,
                Some(Was::Line) => LINE_GAP,
            };
            let drawn = match row {
                Row::Bar { label, permille, color } => self.bar(y + gap, label.as_deref(), *permille, tone(Style::default().fg(*color), self.face.pal)),
                Row::Value { label, left, right } => self.line(y + gap, label.as_deref(), left, right.as_ref()),
                _ => continue,
            };
            let Some((bottom, was, whole)) = drawn else { break };
            y = bottom;
            last = Some(was);
            if !whole {
                break;
            }
        }
        y
    }

    fn value_x(&self) -> f32 {
        self.grid.value_x()
    }

    fn right(&self) -> f32 {
        self.grid.right()
    }

    /// A block's label in the label column, on the line whose middle is
    /// `mid`: the board card's.
    fn label(&self, label: Option<&str>, mid: f32) {
        if let Some(label) = label {
            self.grid.label(self.ui, label, mid, self.face.pal);
        }
    }

    /// The cells' █░ bar from `top` as the kit's LED row across the value
    /// column, lit for `permille` in `lit` as the cells fill it: the space
    /// used, gold nearly full; a sync's share, dim while it waits.
    fn bar(&self, top: f32, label: Option<&str>, permille: u32, lit: Color32) -> Option<(f32, Was, bool)> {
        if top + METER_ROW > self.floor + 0.01 {
            return None;
        }
        self.label(label, top + METER_ROW / 2.0);
        self.grid.bar(self.ui, top, permille, lit, self.face.pal);
        Some((top + METER_ROW, Was::Meter, true))
    }

    /// A line of words from `top` in the value column: its LED or its
    /// scan, its words wrapped by words, and its right-hand words at the
    /// column's end where they clear the left's. The file in flight (Now)
    /// is a path, all of it in the monospace, broken anywhere. Converts'
    /// codec is a format's name, set in the words' face. Returns where the
    /// row ends, and whether it was drawn whole.
    fn line(&self, top: f32, label: Option<&str>, left: &Line, right: Option<&Line>) -> Option<(f32, Was, bool)> {
        let face = self.face;
        let (x, end) = (self.value_x(), self.right());
        if label.is_some_and(|l| l == t!("dev.card.label_now")) {
            if top + NOW.1 > self.floor + 0.01 {
                return None;
            }
            self.label(label, top + NOW.1 / 2.0);
            let path = left.to_string();
            let para = Para::new(&path, mono(NOW.0), face.pal.text).spaced(NOW.1).anywhere().floor(self.floor);
            let h = para.paint(self.ui, pos2(x, top), (end - x).max(1.0)).height().max(NOW.1);
            return Some(self.ended(top + h));
        }
        let read = Read::of(left, face);
        let (size, line_h) = match read.kind {
            Kind::Scan(_) => SCAN,
            Kind::Verdict(_) => VERDICT,
            Kind::Dim if label.is_some() => (DIM.0, PLAIN.1),
            Kind::Dim => DIM,
            Kind::Plain => PLAIN,
        };
        if top + line_h > self.floor + 0.01 {
            return None;
        }
        let mid = top + line_h / 2.0;
        self.label(label, mid);
        let mut from = x;
        match read.kind {
            Kind::Verdict(lamp) => {
                let m = studio::metrics(self.ui, &body(size));
                studio::led(self.ui, pos2(from + 3.0, top + m.ascent - m.small / 2.0), lamp);
                from += studio::STATE_INDENT;
            }
            Kind::Scan(lit) => {
                // Its share on its words' baseline, as the board card sets
                // a scan's.
                let on = studio::para_mid(self.ui, top, size);
                from = studio::scan_lead(self.ui, pos2(from, mid), lit, read.share(), on, end, face.pal);
            }
            Kind::Plain | Kind::Dim => {}
        }
        let figures = label.is_none_or(|l| l != t!("dev.card.label_converts"));
        let runs = read.runs(size, figures, face, self.drive);
        let height = if runs.is_empty() {
            line_h
        } else {
            let para = Para::runs(&runs).spaced(line_h).floor(self.floor);
            para.paint(self.ui, pos2(from, top), (end - from).max(1.0)).height().max(line_h)
        };
        if let Some(right) = right
            && height <= line_h + 0.5
        {
            let left_w = from - x + studio::runs_width(self.ui, &runs);
            self.right_words(right, studio::para_mid(self.ui, top, size), x + left_w, end);
        }
        let (bottom, _, whole) = self.ended(top + height);
        Some((bottom, Was::Line, whole))
    }

    /// Where a line's row ends, `bottom` or the floor where a paragraph's
    /// lines were left out under it, and whether it was drawn whole.
    fn ended(&self, bottom: f32) -> (f32, Was, bool) {
        if bottom > self.floor + 0.01 { (self.floor, Was::Line, false) } else { (bottom, Was::Line, true) }
    }

    /// A row's right-hand words at `end` (the free figure, gold with its
    /// LED on a card nearly full; a sync's time left) where they clear the
    /// left's words, ending at `left_end`, by 16 points.
    fn right_words(&self, right: &Line, mid: f32, left_end: f32, end: f32) {
        let face = self.face;
        let read = Read::of(right, face);
        let runs = read.runs(PLAIN.0, true, face, self.drive);
        let lamp = match read.kind {
            Kind::Verdict(lamp) => Some(lamp),
            _ => None,
        };
        let lamp_w = if lamp.is_some() { studio::STATE_INDENT } else { 0.0 };
        let w = studio::runs_width(self.ui, &runs) + lamp_w;
        if left_end + CLEAR + w > end {
            return;
        }
        if let Some(lamp) = lamp {
            studio::led(self.ui, pos2(end - w + 3.0, studio::led_mid(self.ui, mid, &body(PLAIN.0), false)), lamp);
        }
        studio::runs(self.ui, pos2(end, mid), &runs, w + 1.0, Align2::RIGHT_CENTER);
    }
}

// ── Details ───────────────────────────────────────────────────────────────

/// Details' `rows` (the cells' `details_rows`) under the bottom row through
/// `paint`: the rule, the facts — Drive and Removes giving way their own
/// way — the card's own log in the rows the rest leave it, and Sync again.
fn details(paint: &mut Paint, page: &mut Page, ui: &Ui, face: &Face, grid: Grid, view: &CardView, rows: &[Row]) {
    let drive = t!("dev.card.fact_drive").to_string();
    let removes = t!("dev.card.fact_removes").to_string();
    for (i, row) in rows.iter().enumerate() {
        if paint.cut {
            return;
        }
        match row {
            Row::Fact { label, value, .. } if *label == drive => drive_fact(paint, ui, face, grid, view, label, value),
            Row::Fact { label, value, .. } if *label == removes => removes_fact(paint, ui, face, grid, view, label, value),
            row => {
                let tail: f32 = rows[i + 1..].iter().map(|r| fixed_h(r, face)).sum();
                paint.row(page, &view.key, row, tail);
            }
        }
    }
}

/// A fact's row at `paint`'s place, its label in the label column, `value`
/// drawing its words on the row's middle from the value column's left in
/// the column's width; none past the floor, and nothing after it.
fn fact_row(paint: &mut Paint, ui: &Ui, face: &Face, grid: Grid, label: &str, value: impl FnOnce(f32, f32, f32)) {
    if paint.cut || paint.y + FACT_H > paint.floor + 0.01 {
        paint.cut = true;
        return;
    }
    let mid = paint.y + FACT_H / 2.0;
    grid.label(ui, label, mid, face.pal);
    value(mid, grid.value_x(), grid.value_w());
    paint.y += FACT_H;
}

/// The Drive fact (cells/card.rs's `drive_fact`, measured in points): the
/// drive, its file system and its size whole, then as many of the pieces
/// that give way as the value column holds — its clusters, then its serial
/// — each whole; short of room for even the first three, cut with an
/// ellipsis as any fact is. Its figures are in the monospace, the drive
/// letter with them, as the Card board sets it.
fn drive_fact(paint: &mut Paint, ui: &Ui, face: &Face, grid: Grid, view: &CardView, label: &str, value: &str) {
    let v = &view.found.volume;
    let spare = usize::from(v.space.is_some_and(|s| s.cluster > 0)) + usize::from(v.serial.is_some());
    let mut pieces: Vec<&str> = value.rsplitn(spare + 1, " · ").collect();
    pieces.reverse();
    let drive = view.drive();
    let runs = |words: &str| figured(words, face.ports, &drive, body(PLAIN.0), mono(FIGURE), face.pal.text);
    fact_row(paint, ui, face, grid, label, |mid, x, w| {
        let mut keep = pieces.len();
        while keep > 1 && studio::runs_width(ui, &runs(&pieces[..keep].join(" · "))) > w {
            keep -= 1;
        }
        studio::runs(ui, pos2(x, mid), &runs(&pieces[..keep].join(" · ")), w, Align2::LEFT_CENTER);
    });
}

/// The Removes fact (cells/card.rs's `removes_fact`, measured in points):
/// the first path in the monospace, cut with an ellipsis before the count
/// of the rest, which is never cut.
fn removes_fact(paint: &mut Paint, ui: &Ui, face: &Face, grid: Grid, view: &CardView, label: &str, value: &str) {
    let path = plan_of(view).and_then(|p| p.removals.first()).cloned().unwrap_or_default();
    let at = (!path.is_empty()).then(|| value.find(&path)).flatten();
    fact_row(paint, ui, face, grid, label, |mid, x, w| {
        let figures = |words: &str| studio::figured(words, face.ports, body(PLAIN.0), mono(FIGURE), face.pal.text);
        let (before, after) = match at {
            Some(at) => (figures(&value[..at]), figures(&value[at + path.len()..])),
            None => {
                studio::runs(ui, pos2(x, mid), &figures(value), w, Align2::LEFT_CENTER);
                return;
            }
        };
        let (before_w, after_w) = (studio::runs_width(ui, &before), studio::runs_width(ui, &after));
        let mut from = x;
        if !before.is_empty() {
            from = studio::runs(ui, pos2(from, mid), &before, w, Align2::LEFT_CENTER).max.x;
        }
        let room = (w - before_w - after_w).max(1.0);
        let drawn = studio::text(ui, pos2(from, mid), &path, mono(FIGURE), face.pal.text, room, Align2::LEFT_CENTER);
        if !after.is_empty() {
            studio::runs(ui, pos2(drawn.max.x, mid), &after, (x + w - drawn.max.x).max(1.0), Align2::LEFT_CENTER);
        }
    });
}

// ── The gates ─────────────────────────────────────────────────────────────

/// A line of a card gate's body: the cells' words (cells/card.rs's
/// `gate_body`), in its order and in its groups, set the canvas's way.
#[derive(Clone, Debug, PartialEq)]
enum Said {
    /// A sentence wrapped by words, in the text colour, or in the ok green
    /// for what stays.
    Words(String, bool),
    /// A path, a row of its own in the monospace, indented, cut rather than
    /// wrapped.
    Path(String),
    /// What the paths leave unsaid, dim, indented as they are.
    More(String),
    /// A row of words indented and cut rather than wrapped: what a new
    /// server drops.
    Row(String),
    /// A library and how many of its tracks look gone.
    Library(String, String),
    /// The stronger gate's box, ticked or not, and its words.
    Box(String, bool),
}

/// The gate's body as groups of lines, the cells' blank lines between
/// them, with the same words and the same arguments as the cells'.
fn said(gate: &CardGate) -> Vec<Vec<Said>> {
    let words = |w: &str| Said::Words(w.to_string(), false);
    let green = |w: &str| Said::Words(w.to_string(), true);
    match &gate.kind {
        GateKind::Removals { server, n, first, .. } => {
            let mut why = vec![words(&t!("dev.card.gate_remove_why", server = server.clone()))];
            why.extend(first.iter().map(|path| Said::Path(path.clone())));
            if *n > first.len() {
                why.push(Said::More(t!("dev.card.gate_remove_more", n = fmt_count((n - first.len()) as u64)).to_string()));
            }
            let stays = vec![green(&t!("dev.card.gate_remove_stays", server = server.clone()))];
            vec![why, stays, vec![words(&t!("dev.card.gate_remove_never"))]]
        }
        GateKind::Half { server, libs, ticked, .. } => {
            let mut groups = vec![vec![words(&t!("dev.card.gate_half_why", server = server.clone()))]];
            if !libs.is_empty() {
                let lib = |(lib, gone, of): &(String, usize, usize)| {
                    let (n, total) = (fmt_count(*gone as u64), fmt_count(*of as u64));
                    Said::Library(lib.clone(), t!("dev.card.gate_half_lib", n = n, total = total).to_string())
                };
                groups.push(libs.iter().map(lib).collect());
            }
            let ticks = Said::Box(t!("dev.card.gate_half_box", server = server.clone()).to_string(), *ticked);
            groups.push(vec![ticks, words(&t!("dev.card.gate_half_stays"))]);
            groups
        }
        GateKind::Adopt { drive, server, n, all } => {
            let mb = (*n as u64 * 8 * 1024).div_ceil(1_000_000);
            let (every, some, size) = (fmt_count(*all as u64), fmt_count(*n as u64), fmt_count(mb));
            let body = t!("dev.card.gate_adopt_body", drive = drive.clone(), all = every, n = some, server = server.clone(), mb = size);
            let mut then = vec![words(&t!("dev.card.gate_adopt_then"))];
            if all > n {
                then.push(green(&t!("dev.card.gate_adopt_rest", n = fmt_count((all - n) as u64))));
            }
            vec![vec![words(&body)], then, vec![words(&t!("dev.card.gate_adopt_nothing"))]]
        }
        GateKind::Relay { server, bytes } => {
            // About 1.5 MB/s over the relay, 20 MB/s on the same network.
            let (slow, fast) = (span(bytes / 1_500_000), span(bytes / 20_000_000));
            let body = t!("dev.card.gate_relay_body", server = server.clone(), t = slow, t2 = fast);
            vec![vec![words(&body)], vec![green(&t!("dev.card.gate_relay_ok"))]]
        }
        GateKind::Proxy { peer, server, .. } => {
            let body = t!("dev.card.gate_proxy_body", peer = peer.clone(), server = server.clone());
            vec![vec![words(&body)], vec![words(&t!("dev.card.gate_proxy_once"))]]
        }
        GateKind::Server { card, old, new, rows, plays, .. } => {
            let body = t!("dev.card.gate_server_body", card = card.clone(), old = old.clone(), new = new.clone());
            let drop = t!("dev.card.gate_server_drop", n = fmt_count(*rows as u64), old = old.clone());
            let mut groups = vec![vec![words(&body), Said::Row(drop.to_string())]];
            if *plays > 0 {
                groups.push(vec![words(&t!("dev.card.gate_server_plays", n = fmt_count(*plays as u64), old = old.clone()))]);
            }
            groups.push(vec![words(&t!("dev.card.gate_server_old", old = old.clone()))]);
            groups
        }
    }
}

/// A card's gate over `face.full` (the cells' `draw_card_gate`): the kit's
/// gold gate, its title and its body in the cells' words, and its foot —
/// the keep lit, led by the cells' back arrow, and the go as the hairline,
/// the stronger gate's only once its box is ticked. A click on the keep
/// asks for the cells' [`Act::CardKeep`], on the go [`Act::CardGo`], on the
/// stronger gate's box or its words [`Act::CardBox`]; within a chained
/// gate's guard the model swallows each, as it swallows the keys.
pub(super) fn gate(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(gate) = page.card.gate.as_ref() else { return drawn };
    let title = cells::card::gate_title(gate);
    let (keep, go) = cells::card::gate_buttons_of(gate);
    let groups = said(gate);
    // The adopt gate names the card's drive, a figure as the card's own
    // words set it.
    let drive = match &gate.kind {
        GateKind::Adopt { drive, .. } => drive.clone(),
        _ => String::new(),
    };
    let (pal, id) = (face.pal, Id::new("mp3-card-gate"));
    let (words, lead) = unled(&keep);
    let answers = studio::Answers { keep: words, lead, go: go.as_deref(), also: face.ports };
    let shown = studio::gate_with(ui, id, face.full, &title, &answers, pal, |ui| {
        let mut ticked = false;
        for (k, group) in groups.iter().enumerate() {
            if k > 0 {
                ui.add_space(studio::GATE_APART);
            }
            for line in group {
                ticked |= said_line(ui, id, line, face, &drive);
            }
        }
        ticked
    });
    let (answer, ticked) = shown.inner;
    if ticked {
        drawn.acts.push(Act::CardBox);
    }
    match answer {
        Some(true) => drawn.acts.push(Act::CardGo),
        Some(false) => drawn.acts.push(Act::CardKeep),
        None => {}
    }
    drawn
}

/// One line of a gate's body, a row of the layout the window's body wide,
/// `drive` the card's where the gate names it. Whether the box was
/// clicked.
fn said_line(ui: &mut Ui, id: Id, line: &Said, face: &Face, drive: &str) -> bool {
    let pal = face.pal;
    let cut_row = |ui: &mut Ui, runs: &[Run]| {
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(w, SAY_LINE), Sense::hover());
        let x = row.min.x + studio::STATE_INDENT;
        studio::runs(ui, pos2(x, row.center().y), runs, (row.max.x - x).max(1.0), Align2::LEFT_CENTER)
    };
    let runs_of = |words: &str, color: Color32| figured(words, face.ports, drive, body(SAY), mono(SAY_FIGURE), color);
    match line {
        Said::Words(words, green) => {
            // The kit's 1.3 leading of the words' 14 points, a sentence
            // that opens on a figure too.
            let runs = runs_of(words, if *green { pal.ok } else { pal.text });
            Para::runs(&runs).spaced(SAY_LINE).show(ui);
        }
        Said::Path(path) => {
            cut_row(ui, &[(path.clone(), mono(SAY_FIGURE), pal.text)]);
        }
        Said::More(words) => {
            cut_row(ui, &runs_of(words, pal.dim));
        }
        Said::Row(words) => {
            cut_row(ui, &runs_of(words, pal.text));
        }
        Said::Library(lib, count) => {
            let w = ui.available_width();
            let (row, _) = ui.allocate_exact_size(vec2(w, SAY_LINE), Sense::hover());
            let x = row.min.x + studio::STATE_INDENT;
            let room = (row.max.x - x).max(1.0);
            let name = studio::text(ui, pos2(x, row.center().y), lib, body(SAY), pal.text, room, Align2::LEFT_CENTER);
            let from = name.max.x + LIB_APART;
            if from < row.max.x {
                studio::runs(ui, pos2(from, row.center().y), &runs_of(count, pal.text), row.max.x - from, Align2::LEFT_CENTER);
            }
        }
        Said::Box(words, ticked) => return studio::check_line(ui, id.with("box"), *ticked, words, "", false, pal).clicked(),
    }
    false
}

/// The card's egui face run headless, as the window's panel runs it, on
/// the page's own fixtures: cards as the transfer worker tells them
/// (worker::fixtures) on a link whose inbox the test reads, boards on
/// quiet channels. No worker runs, no volume, server or port is touched.
#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::Receiver;

    use egui::epaint::Shape;
    use egui::{Event, FontId, FullOutput, MouseWheelUnit, PointerButton, Pos2, Vec2};

    use super::super::super::card::{Opened, Transfer};
    use super::super::show as page_show;
    use super::*;
    use crate::admin::EguiShown;
    use crate::device::card::engine::Guard;
    use crate::device::card::volume::{Fs, Table};
    use crate::device::card::worker::{self, Allow, Ended, Fold, In, Link, Next, Pause, Phase as Step, fixtures as cards};
    use crate::gui::window::studio::Pal;

    fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// The page's area in points: the GUI's default window (28 rows of 8
    /// by 16 points), its floor (22 rows), the cells' least width (72
    /// columns), and the boards' 1440 by 900 window.
    const WINDOW: Vec2 = vec2(800.0, 448.0);
    const FLOOR: Vec2 = vec2(800.0, 352.0);
    const NARROW: Vec2 = vec2(576.0, 448.0);
    const BOARD: Vec2 = vec2(1440.0, 824.0);

    /// The kit's modal: its padding, the room under its title, and the
    /// title's row.
    const MODAL_PAD: f32 = 24.0;
    const MODAL_GAP: f32 = 20.0;
    const TITLE_H: f32 = 20.0;

    /// A keep's words as its button takes them: bare of the cells' back
    /// arrow.
    fn keep_words(label: &str) -> &str {
        unled(label).0
    }

    /// A context styled as the host's, its passes run over a page's area.
    struct Probe {
        ctx: egui::Context,
        size: Vec2,
    }

    impl Probe {
        fn new(size: Vec2) -> Probe {
            let ctx = egui::Context::default();
            crate::gui::window::panel::fonts(&ctx);
            studio::style_once(&ctx, &Pal::now());
            Probe { ctx, size }
        }

        /// One pass with `events`: what egui drew, and what the page told
        /// its host.
        fn pass(&self, page: &mut Page, events: Vec<Event>) -> (FullOutput, EguiShown) {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), events, ..Default::default() };
            let mut told: Option<EguiShown> = None;
            let mut out = self.ctx.run_ui(input, |ui| {
                let next = page_show(ui, page, "", false);
                told = Some(match told.take() {
                    Some(last) => EguiShown { outcome: last.outcome.or(next.outcome), caret: next.caret },
                    None => next,
                });
            });
            // No GPU here to take the font atlas: let go of it on purpose.
            out.textures_delta.clear();
            (out, told.expect("the pass ran"))
        }

        /// One pass with `events` that egui runs twice, as it does for a
        /// gate that took its body's height: the first run asks for the
        /// second, and the runs egui made come back.
        fn pass_twice(&self, page: &mut Page, events: Vec<Event>) -> u32 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), events, ..Default::default() };
            let mut runs = 0;
            let mut out = self.ctx.run_ui(input, |ui| {
                runs += 1;
                let _ = page_show(ui, page, "", false);
                if runs == 1 {
                    ui.ctx().request_discard("the test's second run");
                }
            });
            out.textures_delta.clear();
            runs
        }

        /// Two passes (egui sizes a gate on the first), the second's.
        fn settle(&self, page: &mut Page) -> FullOutput {
            let _ = self.pass(page, Vec::new());
            self.pass(page, Vec::new()).0
        }

        /// A press and a release at `at`, a pass each after the move.
        fn click(&self, page: &mut Page, at: Pos2) {
            let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            for events in [vec![Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
                let _ = self.pass(page, events);
            }
        }

        /// A click on the middle of `words` as the page draws them now.
        fn click_on(&self, page: &mut Page, words: &str) {
            let out = self.settle(page);
            self.click(page, find(&out, words).center());
        }
    }

    /// Every text a pass drew, whole, with where.
    fn texts(out: &FullOutput) -> Vec<(String, Rect)> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) => Some((t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size()))),
                _ => None,
            })
            .collect()
    }

    fn words_of(out: &FullOutput) -> Vec<String> {
        texts(out).into_iter().map(|(t, _)| t).collect()
    }

    fn drew(out: &FullOutput, words: &str) -> bool {
        texts(out).iter().any(|(t, _)| t == words)
    }

    fn find(out: &FullOutput, words: &str) -> Rect {
        texts(out).into_iter().find(|(t, _)| t == words).map(|(_, r)| r).unwrap_or_else(|| panic!("{words:?} drawn: {:?}", words_of(out)))
    }

    fn assert_drew(out: &FullOutput, words: &[&str]) {
        for w in words {
            assert!(drew(out, w), "{w:?} drawn: {:?}", words_of(out));
        }
    }

    /// The galley a pass drew with the text `words`.
    fn galley(out: &FullOutput, words: &str) -> std::sync::Arc<egui::Galley> {
        let found = out.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Text(t) if t.galley.text() == words => Some(t.galley.clone()),
            _ => None,
        });
        found.unwrap_or_else(|| panic!("{words:?} drawn: {:?}", words_of(out)))
    }

    /// The faces `words` was set in, run by run.
    fn fonts_of(out: &FullOutput, words: &str) -> Vec<FontId> {
        galley(out, words).job.sections.iter().map(|s| s.format.font_id.clone()).collect()
    }

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    /// A page with the boards `tell` says and `list` in this computer's
    /// readers, on channels the test holds: the transfer worker's inbox and
    /// its fold.
    fn with_cards(tell: impl FnOnce(&Ends), list: Vec<CardView>) -> (Page, Ends, Receiver<In>, Rc<RefCell<Fold>>) {
        let (mut page, ends) = Page::quiet();
        let (link, inbox) = Link::pair();
        let fold = Rc::new(RefCell::new(Fold::default()));
        {
            let mut f = fold.borrow_mut();
            f.watched = true;
            f.servers = vec![cards::home()];
            for card in list {
                f.apply(worker::Event::Card(Box::new(card)));
            }
        }
        page.attach(Transfer { link, fold: fold.clone() });
        tell(&ends);
        page.pump();
        (page, ends, inbox, fold)
    }

    /// A page with `card` alone in a reader and no board: its view.
    fn alone(card: CardView) -> (Page, Receiver<In>) {
        let (page, _ends, inbox, _fold) = with_cards(|_| {}, vec![card]);
        (page, inbox)
    }

    /// The ready card's key: its drive and its serial.
    const KEY: &str = "E:\\#1A2B-3C4D";

    /// The ready card with its plan changed.
    fn planned(change: impl FnOnce(&mut worker::Planned)) -> CardView {
        cards::with(cards::ready("E:", now()), |c| {
            if let Next::Plan(plan) = &mut c.next {
                change(plan);
            }
        })
    }

    /// The paths of a plan that removes `n`.
    fn paths(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Velvet Static/Harbour Songs/{i:03} - Track.flac")).collect()
    }

    /// The LED segments a pass drew 10 points tall (the fill meters), lit
    /// in `lit` and unlit.
    fn segments(out: &FullOutput, lit: Color32) -> (usize, usize) {
        let unlit = Pal::now().unlit;
        let tall = |r: &egui::epaint::RectShape| (r.rect.height() - 10.0).abs() < 0.01;
        let count = |color: Color32| out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == color && tall(r))).count();
        (count(lit), count(unlit))
    }

    // ── The Card view ─────────────────────────────────────────────────

    #[test]
    fn a_ready_card_draws_its_head_its_rows_its_bottom_row_and_its_primary() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _inbox) = alone(cards::ready("E:", now()));
        let (_, shown) = probe.pass(&mut page, Vec::new());
        let out = probe.settle(&mut page);
        assert_drew(&out, &["Car music", "E: · FAT32", "HOLDS", "Rock, Jazz from Home", "SPACE", "LAST SYNC", "NEXT SYNC", "CONVERTS"]);
        assert_drew(&out, &["41.2 of 63.8 GB used · 4,812 tracks", "22.6 GB free", "3 days ago", "12 new · 2 to remove · 37 plays to send"]);
        assert_drew(&out, &["only what the player can't play, to Opus 192k", "Details", "Settings…", "Send the plays", "Eject", "SYNC"]);
        assert!(shown.caret.is_none(), "the Card view has no field");
        // A plain value in the card's 13.5 points, its figures in the
        // monospace; Converts' codec a format's name, in the words' face.
        assert_eq!(fonts_of(&out, "Rock, Jazz from Home"), [body(13.5)]);
        assert!(fonts_of(&out, "3 days ago").contains(&mono(12.5)), "{:?}", fonts_of(&out, "3 days ago"));
        assert_eq!(fonts_of(&out, "only what the player can't play, to Opus 192k"), [body(13.5)]);
        // Each label on its value's first line, the labelled rows a block's
        // 18 points apart: Space's meter, 6, its figures, 18, Last sync.
        let mid = |words: &str| find(&out, words).center().y;
        assert!((mid("HOLDS") - mid("Rock, Jazz from Home")).abs() < 2.0, "{} {}", mid("HOLDS"), mid("Rock, Jazz from Home"));
        assert!((mid("LAST SYNC") - mid("SPACE") - (20.0 + 6.0 + 20.0 + 18.0)).abs() < 1.0, "{} {}", mid("SPACE"), mid("LAST SYNC"));
        // The space used as the kit's LED row, the cells' share of it lit.
        let (lit, unlit) = segments(&out, Pal::now().accent);
        assert_eq!(lit, studio::fill_lit(645, lit + unlit), "41.2 of 63.8 GB: {lit} of {}", lit + unlit);
        // The bottom row under the rows, the primary at its right.
        let primary = find(&out, "SYNC");
        assert!(primary.min.y > find(&out, "only what the player can't play, to Opus 192k").max.y);
        assert!(primary.min.x > find(&out, "Eject").max.x, "the links stop short of the primary");
    }

    #[test]
    fn the_card_views_text_buttons_and_its_primary_ask_for_the_cells_acts() {
        let _en = english();
        let probe = Probe::new(WINDOW);
        let (mut page, inbox) = alone(cards::ready("E:", now()));
        probe.click_on(&mut page, "Send the plays");
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Plays { key: KEY.into() }], "the cells' CardPlays");
        probe.click_on(&mut page, "Eject");
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Eject { key: KEY.into() }], "the cells' CardEject");
        // Two removals ask nothing: Sync ▸ is the sync.
        probe.click_on(&mut page, "SYNC");
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Sync { key: KEY.into(), allow: Box::default() }], "the cells' Primary");
        probe.click_on(&mut page, "Details");
        assert!(page.details, "the cells' Details");
        probe.click_on(&mut page, "Settings…");
        assert!(page.card.sheet.is_some(), "the cells' CardSettings");
        assert!(worker::sent(&inbox).is_empty());
    }

    #[test]
    fn details_open_on_the_cards_facts_its_own_log_and_sync_again_and_the_wheel_scrolls_the_log() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, inbox, fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
        for i in 0..40 {
            let item = worker::LogItem::Said(format!("line {i}"));
            fold.borrow_mut().apply(worker::Event::Log { key: KEY.into(), item });
        }
        page.pump();
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["DRIVE", "E: · FAT32 on MBR · 63.8 GB · 32 KB clusters · 1A2B-3C4D", "CARD", "id 5eedc0de0a1b2c3d · rev. 42 · mStream Player 0.14.0"]);
        assert_drew(&out, &["Home · signed in on this computer", "REMOVES", "Velvet Static/Harbour Songs/04 - Low Tide.flac", " and 1 more"]);
        assert_drew(&out, &["Core2 0.14.0: mp3,flac,opus · Auto DJ 2 · play log", "line 39", "Sync again"]);
        assert_eq!(fonts_of(&out, "Velvet Static/Harbour Songs/04 - Low Tide.flac"), [mono(12.5)], "the path in the monospace");
        assert!(!drew(&out, "line 0"), "the log's newest lines in the rows it is left");
        // The wheel over the log: a line a notch, the page's own count.
        let at = find(&out, "line 39").center();
        let wheel = Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, 1.0), phase: egui::TouchPhase::Move, modifiers: Default::default() };
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(at)]);
        let _ = probe.pass(&mut page, vec![wheel.clone(), wheel]);
        assert_eq!(page.log_scroll, 2, "scrolled up two");
        let out = probe.settle(&mut page);
        assert!(!drew(&out, "line 39") && drew(&out, "line 37"), "{:?}", words_of(&out));
        // Sync again is Sync ▸ again: two removals, no gate.
        probe.click_on(&mut page, "Sync again");
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Sync { key: KEY.into(), allow: Box::default() }], "the cells' CardSyncAgain");
    }

    #[test]
    fn details_drive_fact_gives_way_a_piece_at_a_time_and_removes_cuts_its_path_never_its_count() {
        let _en = english();
        // The boards' card in their window: every piece, the path whole.
        let probe = Probe::new(BOARD);
        let (mut page, _inbox) = alone(cards::ready("E:", now()));
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        assert!(!galley(&out, "E: · FAT32 on MBR · 63.8 GB · 32 KB clusters · 1A2B-3C4D").elided);
        assert!(!galley(&out, "Velvet Static/Harbour Songs/04 - Low Tide.flac").elided);
        // A volume's long name (Linux, macOS) and a long path at the cells'
        // least width: the Drive fact keeps the most pieces that fit, each
        // whole, the serial first to go; the path is cut, never the count.
        let drive = "MSTREAM_CAR_MUSIC_BACKUP";
        let long = "Velvet Static/Harbour Songs (Deluxe Edition)/04 - Low Tide (Extended Mix) [Remastered 2024].flac".to_string();
        let card = cards::with(cards::ready(drive, now()), |c| {
            if let Next::Plan(plan) = &mut c.next {
                plan.removals = std::iter::once(long.clone()).chain(paths(1_234)).collect();
            }
        });
        let whole = format!("{drive} · FAT32 on MBR · 63.8 GB · 32 KB clusters · 1A2B-3C4D");
        let pieces: Vec<&str> = whole.split(" · ").collect();
        let probe = Probe::new(vec2(NARROW.x, BOARD.y));
        let (mut page, _inbox) = alone(card);
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        let fact = (3..5).map(|k| pieces[..k].join(" · ")).find(|f| drew(&out, f)).unwrap_or_else(|| panic!("the Drive fact given way: {:?}", words_of(&out)));
        assert!(!galley(&out, &fact).elided, "each piece whole, never cut: {fact}");
        assert!(!fact.contains("1A2B-3C4D"), "the serial first: {fact}");
        assert!(!galley(&out, " and 1,234 more").elided, "the count never cut");
        assert!(galley(&out, &long).elided, "the path cut before its count");
        let (path, count) = (find(&out, &long), find(&out, " and 1,234 more"));
        assert!(count.min.x >= path.max.x - 0.5, "the count after the path: {path:?} {count:?}");
    }

    #[test]
    fn short_of_room_the_dim_lines_then_converts_give_way_and_the_bottom_row_stays_whole() {
        let _en = english();
        let gift = cards::with(cards::ready("E:", now()), |c| {
            cards::facts(c).state.as_mut().expect("its settings").settings.for_someone_else = true;
            cards::facts(c).device.present = false;
        });
        let dims = [t!("dev.card.conv_default").to_string(), t!("dev.card.conv_gift").to_string()];
        let probe = Probe::new(BOARD);
        let (mut page, _inbox) = alone(gift.clone());
        let out = probe.settle(&mut page);
        assert_drew(&out, &["CONVERTS", &dims[0], &dims[1], "SYNC"]);
        assert_eq!(fonts_of(&out, &dims[1]), [body(13.0)], "an explanation dim, 13 points");
        // At the GUI's floor with Details open, the rows give way as
        // give_way_rows gives them way, and the head, Holds, Space and the
        // bottom row stay, the primary whole over the busy line. Its dim
        // lines are a sentence each whose words the cells' wrap parts with
        // plain spaces, so its first step passes them by: Converts goes,
        // and the lines under it with it.
        let probe = Probe::new(FLOOR);
        let (mut page, _inbox) = alone(gift);
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        assert!(!drew(&out, "CONVERTS") && !drew(&out, &dims[0]) && !drew(&out, &dims[1]), "{:?}", words_of(&out));
        assert_drew(&out, &["Car music", "HOLDS", "SPACE", "LAST SYNC", "Details", "SYNC"]);
        let primary = find(&out, "SYNC");
        assert!(primary.max.y + 12.0 < FLOOR.y - studio::NOTE_H, "the primary over the busy line: {primary:?}");
    }

    #[test]
    fn the_give_way_takes_the_last_dim_line_then_converts_and_the_lines_under_it_as_the_cells_do() {
        let _en = english();
        let dim = crate::kit::dim();
        let value = |label: Option<&str>, spans: Vec<ratatui::text::Span<'static>>| Row::Value { label: label.map(str::to_string), left: Line::from(spans), right: None };
        let word = |w: &'static str| ratatui::text::Span::styled(w, dim);
        let converts = t!("dev.card.label_converts").to_string();
        let mut rows = vec![
            value(Some("Holds"), vec![ratatui::text::Span::raw("Rock")]),
            value(None, vec![word("first")]),
            value(Some(converts.as_str()), vec![ratatui::text::Span::raw("Opus")]),
            value(None, vec![word("under")]),
            value(None, vec![word("two"), ratatui::text::Span::raw(" "), word("words")]),
            value(Some("Last"), vec![ratatui::text::Span::raw("now")]),
        ];
        let left = |rows: &[Row]| rows.iter().map(|r| if let Row::Value { left, .. } = r { left.to_string() } else { String::new() }).collect::<Vec<_>>();
        assert!(give_way(&mut rows));
        assert_eq!(left(&rows), ["Rock", "first", "Opus", "two words", "now"], "the last dim line first");
        assert!(give_way(&mut rows));
        assert_eq!(left(&rows), ["Rock", "Opus", "two words", "now"], "then the one before it");
        // A sentence the cells' wrap parts with plain spaces is no dim line
        // to them: Converts goes, with it.
        assert!(give_way(&mut rows));
        assert_eq!(left(&rows), ["Rock", "now"]);
        assert!(!give_way(&mut rows), "never Holds or the rest");
    }

    #[test]
    fn short_of_width_send_the_plays_gives_way_first_then_the_links_from_the_end() {
        let _en = english();
        let probe = Probe::new(vec2(480.0, 448.0));
        let (mut page, inbox) = alone(cards::ready("E:", now()));
        let out = probe.settle(&mut page);
        assert!(!drew(&out, "Send the plays"), "it gives way first: {:?}", words_of(&out));
        assert_drew(&out, &["Details", "SYNC"]);
        let primary = find(&out, "SYNC");
        for (words, _) in texts(&out).into_iter().filter(|(t, _)| ["Details", "Settings…", "Eject"].contains(&t.as_str())) {
            assert!(find(&out, &words).max.x < primary.min.x, "{words} clear of the primary");
        }
        assert!(worker::sent(&inbox).is_empty());
        // Narrower, the links that would run under the primary go from the
        // end: at the first width that leaves one out, those left are the
        // row's first, in its order.
        let order = ["Details", "Settings…", "Eject"];
        let narrower = (1..40).map(|k| 480.0 - 8.0 * k as f32).find_map(|w| {
            let out = Probe::new(vec2(w, 448.0)).settle(&mut page);
            let left: Vec<bool> = order.iter().map(|words| drew(&out, words)).collect();
            (!left[2]).then_some((w, left))
        });
        let (w, left) = narrower.expect("a width that leaves a link out");
        assert!(left[0] && left.windows(2).all(|pair| pair[0] || !pair[1]), "the links from the end at {w}: {left:?}");
    }

    #[test]
    fn a_sync_in_flight_draws_its_count_its_rate_the_file_in_flight_and_stop_at_the_next_save() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, inbox, fold) = with_cards(Ends::up_to_date, vec![cards::syncing("E:", now())]);
        page.act(Act::Card(0));
        let out = probe.settle(&mut page);
        assert_drew(&out, &["HOLDS", "SYNCING", "1,204 of 4,812 · 6.1 of 21.3 GB", "about 20 min left", "12.4 MB/s · saved 3 min ago, safe to stop"]);
        assert_drew(&out, &["NOW", "Orla Finch/Paper Boats/03 - Fold.flac", "Stop at the next save"]);
        assert_eq!(fonts_of(&out, "Orla Finch/Paper Boats/03 - Fold.flac"), [mono(12.5)], "the file in flight, a path: the monospace");
        assert!(fonts_of(&out, "12.4 MB/s · saved 3 min ago, safe to stop").contains(&body(13.0)), "the rate dim, 13 points");
        assert!(!drew(&out, "SPACE") && !drew(&out, "SYNC"), "the rest rows and the primary wait for the run");
        // The run's share lit as the cells fill their bar: 28%.
        let (lit, unlit) = segments(&out, Pal::now().accent);
        assert_eq!(lit, studio::fill_lit(280, lit + unlit));
        probe.click_on(&mut page, "Stop at the next save");
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Stop { key: KEY.into() }], "the cells' CardStop");
        // Stopping: the words in the link's place, bound to Details.
        let mut card = fold.borrow().cards[0].clone();
        card.run.as_mut().expect("its run").stopping = true;
        fold.borrow_mut().apply(worker::Event::Card(Box::new(card)));
        page.pump();
        probe.click_on(&mut page, "stopping at the next save…");
        assert!(page.details && worker::sent(&inbox).is_empty(), "the cells' Details");
    }

    #[test]
    fn a_synced_card_says_so_its_look_scans_in_next_sync_and_eject_is_the_primary() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let done = cards::with(cards::ready("E:", now()), |c| {
            c.ended = Some(Ended::Done { tracks: 4_822, took: 60, skipped: Vec::new(), plays: 37 });
            c.next = Next::Asking;
        });
        let (mut page, inbox) = alone(done);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["just now · verified · 37 plays sent to Home", "asking Home what changed…", "EJECT"]);
        assert!(!drew(&out, "Eject"), "no Eject link where it is the primary");
        assert_eq!(fonts_of(&out, "asking Home what changed…"), [body(13.0)], "the scan's words at 13");
        let unlit = Pal::now().unlit;
        let scan = out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == unlit && r.rect.width() == 5.0)).count();
        assert!(scan >= 10, "the kit's searching shape: {scan}");
        probe.click_on(&mut page, "EJECT");
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Eject { key: KEY.into() }], "the cells' Primary");
    }

    #[test]
    fn the_cards_states_draw_their_words_and_their_links_ask_for_the_cells_acts() {
        let _en = english();
        let probe = Probe::new(WINDOW);
        let draw = |card: CardView| {
            let (mut page, inbox) = alone(card);
            let out = probe.settle(&mut page);
            (page, inbox, out)
        };
        let ready = || cards::ready("E:", now());
        let (_, _, out) = draw(cards::reading("E:"));
        assert_drew(&out, &["Card in E:", "reading the card…"]);
        let (_, _, out) = draw(cards::new_card("F:"));
        assert_drew(&out, &["A FAT32 card with nothing from mStream on it yet.", "SET UP"]);
        let (_, _, out) = draw(cards::with(ready(), |c| c.server = worker::Server::Unknown("Home".into())));
        assert_drew(&out, &["Filled from Home, which this computer has no sign-in for", "ADD THE SERVER", "Fill it from Home instead…"]);
        let (_, _, out) = draw(cards::with(ready(), |c| c.server = worker::Server::New(None)));
        assert_drew(&out, &["No server saved on this computer yet", "ADD THE SERVER"]);
        let (_, _, out) = draw(cards::with(ready(), |c| c.next = Next::SignIn { plays: 0 }));
        assert_drew(&out, &["Home needs you to sign in again", "SIGN IN"]);
        // Unreachable: Try now looks again.
        let (mut page, inbox, out) = draw(cards::with(ready(), |c| c.next = Next::Unreached { asked: now() - 120, why: "x".into() }));
        assert_drew(&out, &["Home didn't answer · asked 2 min ago", "Try now"]);
        probe.click(&mut page, find(&out, "Try now").center());
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Look { key: KEY.into() }], "the cells' CardTryNow");
        // A volume the player can't use: why, Eject ▸, and Hide it.
        let (mut page, inbox, out) = draw(cards::refused("G:", Fs::ExFat, Table::Mbr));
        assert_drew(&out, &["Card in G:", "G: · exFAT · 63.8 GB", "FORMAT", "exFAT card · the player reads FAT32 only", "EJECT", "Not a card? Hide it"]);
        probe.click(&mut page, find(&out, "Not a card? Hide it").center());
        assert_eq!(worker::sent(&inbox), [worker::Cmd::Hide { key: "G:\\#1A2B-3C4D".into() }], "the cells' CardHide");
        // Paused: Try now and Stop for now, no primary, the bar dim.
        let paused = cards::with(ready(), |c| {
            let pause = Pause::Server { until: now() + 179, tries: 3 };
            c.ended = Some(Ended::Paused { pause, at: 1_930, of: 4_812, bytes: (8_600_000_000, 21_300_000_000) });
        });
        let (mut page, inbox, out) = draw(paused);
        assert_drew(&out, &["1,930 of 4,812", "Try now", "Stop for now"]);
        assert!(!drew(&out, "SYNC") && segments(&out, Pal::now().dim).0 > 0, "no primary; the share lit dim");
        probe.click(&mut page, find(&out, "Stop for now").center());
        assert_eq!(worker::sent(&inbox), [worker::Cmd::StopPause { key: KEY.into() }], "the cells' CardStopPause");
        // Taken out during its sync: Give up this run….
        let (mut page, inbox, out) = draw(cards::with(ready(), |c| {
            c.out = true;
            c.ended = Some(Ended::Out { at: 1_204, of: 4_812 });
        }));
        assert_drew(&out, &["not in a reader", "Give up this run…"]);
        probe.click(&mut page, find(&out, "Give up this run…").center());
        assert_eq!(worker::sent(&inbox), [worker::Cmd::GiveUp { key: KEY.into() }], "the cells' CardGiveUp");
        // Waiting its turn: Don't wait.
        let (mut page, inbox, out) = draw(cards::with(ready(), |c| c.run = Some(worker::Run { phase: Step::Waiting, ..worker::Run::default() })));
        assert_drew(&out, &["Don't wait"]);
        probe.click(&mut page, find(&out, "Don't wait").center());
        assert_eq!(worker::sent(&inbox), [worker::Cmd::DontWait { key: KEY.into() }], "the cells' CardDontWait");
        // A disk check first; a copy of another card.
        let (_, _, out) = draw(cards::with(ready(), |c| cards::facts(c).check = true));
        assert_drew(&out, &["CHECK", "Check this card on the computer first", "CHECK AGAIN"]);
        let (_, _, out) = draw(cards::with(ready(), |c| cards::facts(c).copied = true));
        assert_drew(&out, &["A copy of another card", "GIVE IT ITS OWN IDENTITY"]);
    }

    #[test]
    fn under_the_write_the_cards_primary_waits_with_why_and_its_links_keep_to_the_cells_lock() {
        let _en = english();
        let probe = Probe::new(WINDOW);
        let (mut page, _ends, inbox, _fold) = with_cards(Ends::update_available, vec![cards::ready("E:", now())]);
        page.act(Act::Primary);
        page.act(Act::Write);
        assert!(page.lock.is_some(), "a write under way");
        page.act(Act::Card(0));
        let out = probe.settle(&mut page);
        assert_drew(&out, &["SYNC", "Settings…"]);
        // Its why in a tooltip under the pointer, the cells' tip's words.
        let why = t!("dev.fw_one_at_a_time", port = "COM3").to_string();
        assert!(!drew(&out, &why), "no tooltip yet");
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(find(&out, "SYNC").center())]);
        for _ in 0..30 {
            let _ = probe.pass(&mut page, Vec::new());
        }
        assert_drew(&probe.pass(&mut page, Vec::new()).0, &[&why]);
        probe.click_on(&mut page, "SYNC");
        probe.click_on(&mut page, "Settings…");
        assert!(worker::sent(&inbox).is_empty() && page.card.sheet.is_none(), "nothing starts during a write");
    }

    // ── The gates ─────────────────────────────────────────────────────

    #[test]
    fn the_removals_gate_draws_its_paths_over_the_inert_card_and_its_go_asks_for_the_sync_with_its_count() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, inbox) = alone(planned(|p| p.removals = paths(214)));
        let before = probe.settle(&mut page);
        let settings = find(&before, "Settings…").center();
        page.act(Act::Primary);
        let (_, shown) = probe.pass(&mut page, Vec::new());
        let out = probe.settle(&mut page);
        assert!(shown.caret.is_none(), "a gate has no field");
        let why = t!("dev.card.gate_remove_why", server = "Home").to_string();
        let stays = t!("dev.card.gate_remove_stays", server = "Home").to_string();
        assert_drew(&out, &["Remove 214 tracks from Car music?", &why, "Velvet Static/Harbour Songs/000 - Track.flac", "and 209 more", &stays]);
        assert_drew(&out, &[&t!("dev.card.gate_remove_never"), "NOT NOW", "REMOVE 214 AND SYNC"]);
        assert_eq!(fonts_of(&out, "Velvet Static/Harbour Songs/000 - Track.flac"), [mono(13.0)]);
        assert_eq!(galley(&out, &stays).job.sections[0].format.color, Pal::now().ok, "what stays in the ok green");
        // The keep led by the cells' back arrow, drawn as the kit's pointer.
        let on_accent = Pal::now().on_accent;
        let arrow = out.shapes.iter().any(|c| matches!(&c.shape, Shape::Path(p) if p.fill == on_accent && p.points.len() == 3));
        assert!(arrow, "the keep's lead");
        // The card beneath is inert: a click where Settings… stands is the
        // scrim's.
        probe.click(&mut page, settings);
        assert!(page.card.sheet.is_none() && page.card.gate.is_some() && worker::sent(&inbox).is_empty());
        probe.click_on(&mut page, "REMOVE 214 AND SYNC");
        let sent = worker::sent(&inbox);
        let [worker::Cmd::Sync { allow, .. }] = sent.as_slice() else { panic!("{sent:?}") };
        assert!(allow.answers.removals && !allow.answers.mass_removals, "the cells' CardGo");
        assert_eq!(allow.answers.removals_seen, Some(214), "the yes names its count");
        assert!(page.card.gate.is_none());
    }

    #[test]
    fn the_stronger_gate_shows_its_go_only_once_its_box_is_ticked() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let many = paths(214);
        let half = planned(|p| {
            p.removals = many.clone();
            p.guards = vec![Guard::Removals { paths: many.clone(), strong: true }];
            p.removal_libs = (0..214).map(|i| if i < 200 { "Rock" } else { "Jazz" }.to_string()).collect();
        });
        let (mut page, inbox) = alone(half);
        page.act(Act::Primary);
        let ticked = |page: &Page| match page.card.gate.as_ref().map(|g| &g.kind) {
            Some(GateKind::Half { ticked, .. }) => Some(*ticked),
            _ => None,
        };
        assert_eq!(ticked(&page), Some(false));
        let out = probe.settle(&mut page);
        let why = t!("dev.card.gate_half_why", server = "Home").to_string();
        assert_drew(&out, &["Remove 214 of Car music's 4,812 tracks?", &why, "Rock", "200 of 2,904 gone", "Jazz", "14 of 1,908 gone"]);
        assert_drew(&out, &["Home really has dropped them", &t!("dev.card.gate_half_stays"), "NOT NOW"]);
        assert!(!drew(&out, "REMOVE 214 AND SYNC"), "no second button yet");
        // A click on the box's words ticks it, the cells' CardBox.
        probe.click_on(&mut page, "Home really has dropped them");
        assert_eq!(ticked(&page), Some(true));
        let out = probe.settle(&mut page);
        assert_drew(&out, &["REMOVE 214 AND SYNC", "NOT NOW"]);
        assert!(find(&out, "NOT NOW").max.x < find(&out, "REMOVE 214 AND SYNC").min.x, "the keep first, the go last at the right");
        probe.click_on(&mut page, "Home really has dropped them");
        assert_eq!(ticked(&page), Some(false), "unticked");
        assert!(!drew(&probe.settle(&mut page), "REMOVE 214 AND SYNC"), "the go goes");
        probe.click_on(&mut page, "Home really has dropped them");
        probe.click_on(&mut page, "REMOVE 214 AND SYNC");
        let sent = worker::sent(&inbox);
        let [worker::Cmd::Sync { allow, .. }] = sent.as_slice() else { panic!("{sent:?}") };
        assert!(allow.answers.removals && allow.answers.mass_removals);
    }

    #[test]
    fn a_chained_gate_swallows_a_click_within_its_guard_as_the_model_holds_it() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, inbox) = alone(planned(|p| {
            p.removals = paths(214);
            p.guards = vec![Guard::Adopt(vec!["a.flac".into(); 1_190])];
        }));
        page.act(Act::Primary);
        probe.click_on(&mut page, "REMOVE 214 AND SYNC");
        let adopt = |page: &Page| matches!(page.card.gate.as_ref().map(|g| &g.kind), Some(GateKind::Adopt { .. }));
        assert!(adopt(&page) && page.card.chained.is_some(), "the next gate, its guard up");
        let keep = cells::card::gate_buttons_of(page.card.gate.as_ref().expect("the gate")).0;
        let keep = keep_words(&keep).to_uppercase();
        // Within the guard (held up here, whatever the test's speed), the
        // click is swallowed and the guard starts again.
        page.card.chained = Some(Instant::now() + Duration::from_secs(60));
        probe.click_on(&mut page, &keep);
        assert!(adopt(&page) && worker::sent(&inbox).is_empty(), "nothing answered under the guard");
        // The keys quiet past it: the click is the keep's.
        page.card.chained = None;
        probe.click_on(&mut page, &keep);
        let sent = worker::sent(&inbox);
        let [worker::Cmd::Sync { allow, .. }] = sent.as_slice() else { panic!("{sent:?}") };
        assert_eq!((allow.answers.removals, allow.answers.adopt), (true, Some(false)), "Leave them all as mine");
    }

    #[test]
    fn a_chained_gate_opens_at_its_top_wherever_the_last_ones_body_was_scrolled_to() {
        let _en = english();
        // A window short enough that both gates' bodies scroll.
        let size = vec2(576.0, 260.0);
        let probe = Probe::new(size);
        let (mut page, inbox) = alone(planned(|p| {
            p.removals = paths(214);
            p.guards = vec![Guard::Adopt(vec!["a.flac".into(); 1_190])];
        }));
        page.act(Act::Primary);
        let out = probe.settle(&mut page);
        let why = t!("dev.card.gate_remove_why", server = "Home").to_string();
        let top = find(&out, &why).min.y;
        // The wheel takes the body to its foot, where its go stands.
        let at = pos2(size.x / 2.0, size.y / 2.0);
        let wheel = Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, -2000.0), phase: egui::TouchPhase::Move, modifiers: Default::default() };
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(at), wheel]);
        for _ in 0..40 {
            let _ = probe.pass(&mut page, Vec::new());
        }
        let out = probe.settle(&mut page);
        assert!(find(&out, &why).min.y < top - 40.0, "scrolled to the foot: {} from {top}", find(&out, &why).min.y);
        probe.click_on(&mut page, "REMOVE 214 AND SYNC");
        let gate = page.card.gate.clone().expect("the next gate");
        assert!(matches!(gate.kind, GateKind::Adopt { .. }) && worker::sent(&inbox).is_empty(), "the adopt gate, chained");
        // Its body from its top under its title, though egui kept the last
        // body's place, and taller than its view, so that place would show.
        let out = probe.settle(&mut page);
        let title = find(&out, &cells::card::gate_title(&gate)).center().y;
        let Some(Said::Words(first, _)) = said(&gate).into_iter().flatten().next() else { panic!("the adopt's words") };
        let body_top = title + TITLE_H / 2.0 + MODAL_GAP;
        assert!(find(&out, &first).min.y >= body_top - 1.0, "at its top: {} under {body_top}", find(&out, &first).min.y);
        let keep = keep_words(&cells::card::gate_buttons_of(&gate).0).to_uppercase();
        assert!(find(&out, &keep).min.y > size.y - 16.0 - MODAL_PAD, "its foot under its view");
    }

    #[test]
    fn a_value_or_a_gates_sentence_that_opens_on_a_figure_keeps_its_leading_where_it_wraps() {
        let _en = english();
        let apart = |g: &egui::Galley| g.rows[1].rect().min.y - g.rows[0].rect().min.y;
        // Next sync opens on its count, and at the cells' least width its
        // pieces wrap: 20 points a line, as a value's words.
        let probe = Probe::new(vec2(NARROW.x, BOARD.y));
        let (mut page, _inbox) = alone(planned(|p| {
            p.new = 12_345;
            p.removals = paths(2_345);
            p.plays = 37_000;
            p.waves_missing = true;
        }));
        let out = probe.settle(&mut page);
        let next = galley(&out, "12,345 new · 2,345 to remove · 37,000 plays to send · waveforms to add");
        assert!(next.rows.len() >= 2 && next.job.sections[0].format.font_id == mono(12.5), "it wraps, from its figure");
        assert!((apart(&*next) - 20.0).abs() < 0.1, "a value's 20 points: {}", apart(&*next));
        // The take-over's plays sentence opens on its count; in a narrow
        // window it wraps on the kit's 1.3 of the gate's 14 points, as its
        // sentence that opens on words does (egui lays rows on whole
        // pixels).
        let probe = Probe::new(vec2(360.0, BOARD.y));
        let card = cards::ready("E:", now());
        let (mut page, _inbox) = alone(card.clone());
        let kind = GateKind::Server { card: "Car music".into(), old: "Home".into(), new: "Studio".into(), rows: 4_812, plays: 37, want: None };
        page.card.gate = Some(CardGate { key: card.key.clone(), kind, allow: Allow::default(), card: Opened::of(&card) });
        let out = probe.settle(&mut page);
        let plays = galley(&out, &t!("dev.card.gate_server_plays", n = "37", old = "Home"));
        let words = galley(&out, &t!("dev.card.gate_server_body", card = "Car music", old = "Home", new = "Studio"));
        assert!(plays.rows.len() >= 2 && words.rows.len() >= 2 && plays.job.sections[0].format.font_id == mono(SAY_FIGURE), "both wrap, one from its figure");
        assert!((apart(&*plays) - apart(&*words)).abs() < 0.01, "the gate's leading: {} as {}", apart(&*plays), apart(&*words));
        assert!((apart(&*words) - SAY * 1.3).abs() < 0.5, "the kit's 1.3: {}", apart(&*words));
    }

    #[test]
    fn the_cards_drive_letter_is_a_figure_in_its_drive_fact_and_in_the_words_that_name_it() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let run_of = |g: &egui::Galley, words: &str| {
            let section = g.job.sections.iter().find(|s| &g.job.text[s.byte_range.start.0..s.byte_range.end.0] == words);
            section.unwrap_or_else(|| panic!("{words:?} a run of its own in {:?}", g.job.text)).format.font_id.clone()
        };
        // Details' Drive fact, as the Card board sets it.
        let (mut page, _inbox) = alone(cards::ready("E:", now()));
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        let fact = galley(&out, "E: · FAT32 on MBR · 63.8 GB · 32 KB clusters · 1A2B-3C4D");
        assert_eq!(run_of(&*fact, "E:"), mono(12.5));
        assert_eq!(run_of(&*fact, "1A2B-3C4D"), mono(12.5));
        // A line that names it among its words: the disk check's.
        let (mut page, _inbox) = alone(cards::with(cards::ready("E:", now()), |c| cards::facts(c).check = true));
        let out = probe.settle(&mut page);
        let windows = galley(&out, &t!("dev.card.check_win", drive = "E:"));
        assert_eq!(run_of(&*windows, "E:"), mono(12.5));
        // A word that only holds it, and a volume's name, are words.
        assert!(word_at("XE: E:", "E:") == Some(4) && word_at("E:x", "E:").is_none());
        assert!(letter("E:") && !letter("MSTREAM_CAR_MUSIC") && !letter("E"));
    }

    #[test]
    fn every_gate_kind_draws_its_words_and_its_keep_is_the_cells_keep() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let card = cards::ready("E:", now());
        let kinds = [
            GateKind::Adopt { drive: "E:".into(), server: "Home".into(), n: 1_190, all: 1_204 },
            GateKind::Relay { server: "Home".into(), bytes: 1_900_000_000 },
            GateKind::Proxy { peer: "Lisbon".into(), id: 4, server: "Home".into() },
            GateKind::Server { card: "Car music".into(), old: "Home".into(), new: "Studio".into(), rows: 4_812, plays: 37, want: None },
        ];
        for kind in kinds {
            let (mut page, inbox) = alone(card.clone());
            let gate = CardGate { key: card.key.clone(), kind: kind.clone(), allow: Allow::default(), card: Opened::of(&card) };
            let (keep, go) = cells::card::gate_buttons_of(&gate);
            let title = cells::card::gate_title(&gate);
            let lines: Vec<String> = said(&gate).into_iter().flatten().map(|line| match line {
                Said::Words(w, _) | Said::Path(w) | Said::More(w) | Said::Row(w) | Said::Box(w, _) => w,
                Said::Library(lib, _) => lib,
            }).collect();
            page.card.gate = Some(gate);
            let out = probe.settle(&mut page);
            assert_drew(&out, &[&title, &keep_words(&keep).to_uppercase(), &go.expect("a go").to_uppercase()]);
            for line in &lines {
                assert!(drew(&out, line), "{kind:?}: {line:?} drawn: {:?}", words_of(&out));
            }
            probe.click_on(&mut page, &keep_words(&keep).to_uppercase());
            assert!(page.card.gate.is_none(), "{kind:?}: kept");
            let sent = worker::sent(&inbox);
            match kind {
                GateKind::Adopt { .. } => assert!(matches!(sent.as_slice(), [worker::Cmd::Sync { allow, .. }] if allow.answers.adopt == Some(false)), "{sent:?}"),
                GateKind::Proxy { .. } => assert_eq!(sent, [worker::Cmd::LeaveOut { key: KEY.into(), peer: 4 }]),
                _ => assert_eq!(sent, [worker::Cmd::Dismiss { key: KEY.into() }], "{kind:?}"),
            }
        }
        // Each gate's words are the cells': the take-over's drop and plays.
        let drop = t!("dev.card.gate_server_drop", n = "4,812", old = "Home").to_string();
        let plays = t!("dev.card.gate_server_plays", n = "37", old = "Home").to_string();
        let gate = CardGate {
            key: card.key.clone(),
            kind: GateKind::Server { card: "Car music".into(), old: "Home".into(), new: "Studio".into(), rows: 4_812, plays: 37, want: None },
            allow: Allow::default(),
            card: Opened::of(&card),
        };
        let lines: Vec<Said> = said(&gate).into_iter().flatten().collect();
        assert!(lines.contains(&Said::Row(drop)) && lines.contains(&Said::Words(plays, false)), "{lines:?}");
    }

    #[test]
    fn a_gate_at_the_guis_floor_stands_in_the_area_and_its_body_scrolls_to_its_foot() {
        let _en = english();
        let size = vec2(576.0, 352.0);
        let probe = Probe::new(size);
        let (mut page, inbox) = alone(planned(|p| p.removals = paths(214)));
        page.act(Act::Primary);
        let out = probe.settle(&mut page);
        // The window stands inside the area, its body at its top.
        let title = find(&out, "Remove 214 tracks from Car music?");
        assert!(title.min.y >= 16.0 && title.max.x <= size.x - 16.0, "{title:?}");
        let why = t!("dev.card.gate_remove_why", server = "Home").to_string();
        let body_top = title.center().y + TITLE_H / 2.0 + MODAL_GAP;
        assert!(find(&out, &why).min.y >= body_top - 1.0, "at its top: {} under {body_top}", find(&out, &why).min.y);
        // The body runs past its view, its foot's buttons with it.
        let view_end = size.y - 16.0 - MODAL_PAD;
        assert!(find(&out, "NOT NOW").max.y > view_end + 1.0, "the foot past the view: {:?}", find(&out, "NOT NOW"));
        // The wheel over the body brings the foot into the view, under the
        // title, which stays.
        let wheel = Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, -2000.0), phase: egui::TouchPhase::Move, modifiers: Default::default() };
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(pos2(size.x / 2.0, size.y / 2.0)), wheel]);
        for _ in 0..40 {
            let _ = probe.pass(&mut page, Vec::new());
        }
        let out = probe.settle(&mut page);
        let keep = find(&out, "NOT NOW");
        assert!(keep.max.y <= view_end + 0.5 && keep.min.y >= body_top, "the foot in the view: {keep:?}");
        assert!((find(&out, "Remove 214 tracks from Car music?").min.y - title.min.y).abs() < 0.5, "the title stays");
        // Its keep answers it there: the cells' CardKeep.
        probe.click(&mut page, keep.center());
        assert!(page.card.gate.is_none() && worker::sent(&inbox) == [worker::Cmd::Dismiss { key: KEY.into() }]);
    }

    #[test]
    fn a_click_on_the_stronger_gates_box_in_a_pass_egui_runs_twice_ticks_it_once() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let many = paths(214);
        let (mut page, inbox) = alone(planned(|p| {
            p.removals = many.clone();
            p.guards = vec![Guard::Removals { paths: many.clone(), strong: true }];
            p.removal_libs = (0..214).map(|i| if i < 200 { "Rock" } else { "Jazz" }.to_string()).collect();
        }));
        page.act(Act::Primary);
        let out = probe.settle(&mut page);
        let at = find(&out, "Home really has dropped them").center();
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(at)]);
        let _ = probe.pass(&mut page, vec![button(true)]);
        assert_eq!(probe.pass_twice(&mut page, vec![button(false)]), 2, "egui ran the release's pass twice");
        let ticked = matches!(page.card.gate.as_ref().map(|g| &g.kind), Some(GateKind::Half { ticked: true, .. }));
        assert!(ticked && worker::sent(&inbox).is_empty(), "the box ticked once, not ticked and back");
    }
}

