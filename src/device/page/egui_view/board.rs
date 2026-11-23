//! The board card drawn by egui (contract clauses 17–21 and 30), and the
//! card with no board. The board in view stands in its hairline card: the
//! head, the Firmware and SD card rows, the bottom row with the Details
//! door, the text buttons and the one primary, and Details open on its
//! facts and its log. Its rows are the cells' own (cells.rs's [`Row`]s,
//! made by the same functions with a value column too wide to wrap in),
//! each set in points on the canvas's grid — the label column and the value
//! column wrapping by words, the kit's LED where the cells lead a line with
//! a glyph, the fill meter, the text buttons and the tall primary — so the
//! two faces say the same words under the same conditions, and a click
//! here asks for the act the cells register for the same control, only
//! where they register it.
//!
//! The write's, a write done's, a failure's and a format's rows are drawn in
//! the same grammar from what the model holds for them: the write's words
//! over its bar, the chip and its lines after it, Details opened on the
//! failure's log, the format's scan in the SD card row. [`Paint`] is the
//! grammar, for a card's view too (card.rs).

use egui::epaint::StrokeKind;
use egui::{Align2, Color32, Id, Rect, Stroke, Ui, UiBuilder, pos2, vec2};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use super::super::cells::{self, Row};
use super::super::*;
use super::{Drawn, Face, tone};
use crate::gui::window::studio::{self, Lead, Para, Run, TALL_H, TEXT_BUTTON_H, body, mono, strong};

/// The cells' line, too wide to wrap in: the rows come a sentence each, and
/// the value column wraps them by words in points.
const WIDE: u16 = 4000;
/// The label column: the boards' 140 points, less on a narrow card.
const LABEL_W: f32 = 140.0;
/// The head's row: the name's 16 points on 22.
const HEAD_H: f32 = 22.0;
/// A value's lines, their words' size and their row: a block's first line
/// (a verdict) 14 on 20, a plain one (the figures under a meter) 13.5 on
/// 20, an explanation's dim one 13 on 18; each 4 points under the last, 6
/// under a meter.
const FIRST: (f32, f32) = (14.0, 20.0);
const PLAIN: (f32, f32) = (13.5, 20.0);
const DIM: (f32, f32) = (13.0, 18.0);
const LINE_GAP: f32 = 4.0;
const METER_GAP: f32 = 6.0;
/// A line led by the cells' scan (asking, reading, counting, formatting):
/// the kit's searching shape on the row's 20 points, its words 13 and 10
/// points after the segments, and after the share where there is one.
const SCAN: (f32, f32) = (13.0, 20.0);
const SCAN_GAP: f32 = 10.0;
/// A meter's row, and its segments' height in it.
const METER_ROW: f32 = 20.0;
const METER_H: f32 = 10.0;
/// A Details fact's row, a log line's, and the log's clock column (its
/// `mm:ss` and two spaces in the monospace).
const FACT_H: f32 = 20.0;
const LOG_ROW: f32 = 18.0;
const CLOCK_W: f32 = 53.0;
/// Text buttons in a row, the boards' 24 points apart, and a note after
/// its button.
const APART: f32 = 24.0;
/// Words at a row's right come no nearer the words at its left than this,
/// or are dropped whole.
const CLEAR: f32 = 16.0;
/// The figures' one size in the card.
const FIGURE: f32 = 12.5;

/// The card's grid in points: its content's left and width, and the label
/// column's width.
#[derive(Clone, Copy, Debug)]
pub(super) struct Grid {
    pub(super) x: f32,
    pub(super) w: f32,
    pub(super) label_w: f32,
}

impl Grid {
    /// The grid of a card in `room` with `face`'s padding.
    pub(super) fn of(room: Rect, face: &Face) -> Grid {
        let w = (room.width() - 2.0 * face.space.pad_x).max(1.0);
        Grid { x: room.min.x + face.space.pad_x, w, label_w: LABEL_W.min(w * 0.3) }
    }

    fn value_x(&self) -> f32 {
        self.x + self.label_w
    }

    fn value_w(&self) -> f32 {
        (self.w - self.label_w).max(1.0)
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }
}

/// The board in view, its card in the column's `room` (cells.rs's `card`):
/// its head, its value blocks, its bottom row, and Details while open. The
/// card is as tall as its rows and no taller than the room, cut inside its
/// frame from the foot; the bottom row is never cut — the value rows give
/// way above it, whole lines at a time — so its primary stands at every
/// size the page is drawn at. Details takes what is left under it, its log
/// the rows its other rows leave.
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face, board: &Board, room: Rect) -> Drawn {
    let s = face.space;
    let grid = Grid::of(room, face);
    let card = clipped(ui, room, "mp3-board");
    let mut y = room.min.y + s.pad_y;
    y = head(&card, face, grid, y, &cells::board_name(board), &cells::head_right(page, board));
    if let Some(words) = cells::identity(page, board) {
        y = dim_line(&card, face, grid, y + LINE_GAP, &words);
    }
    let actions = cells::actions_row(page, board);
    let actions_h = match &actions {
        Row::Actions { primary: Some(_), .. } => TALL_H,
        _ => TEXT_BUTTON_H,
    };
    // The measuring run sees the card whole; the drawing cuts the values
    // short of the bottom row's place at the room's foot.
    let foot = if face.live { room.max.y - s.pad_y } else { f32::INFINITY };
    let mut acts = Vec::new();
    let mut paint = Paint::new(&card, face, grid, y, foot - actions_h - s.gap, Id::new("mp3-board"), &mut acts);
    for block in cells::value_blocks(page, board, WIDE) {
        paint.blank();
        paint.rows(page, board.port(), &block);
    }
    paint.floor = foot;
    paint.cut = false;
    paint.blank();
    paint.row(page, board.port(), &actions, 0.0);
    let mut rest = paint.y + s.pad_y - room.min.y;
    // A failed write opens Details by itself on the log's newest lines
    // (the Failed board; the cells keep three of them at the GUI's floor):
    // there Details counts at rest — its rule, the port, which is all its
    // facts after a failure, and two lines of the log — so the frame takes
    // the tighter measures rather than leave the failure under the card's
    // foot.
    if page.details && matches!(board.written, Some(Written::Failed { .. })) {
        rest += 2.0 * s.gap + 1.0 + FACT_H + s.gap + 2.0 * LOG_ROW;
    }
    if page.details && face.live {
        paint.rows(page, board.port(), &cells::details_rows(page, board));
    }
    let bottom = (paint.y + s.pad_y).min(room.max.y);
    hairline(&card, face, Rect::from_min_max(room.min, pos2(room.max.x, bottom)));
    Drawn { acts, caret: None, rest }
}

/// The card with no board (contract clause 30; the cells' `empty_card`):
/// what went wrong listing the ports, or the watch starting, or Nothing
/// plugged in with what to do and the watch's still line; then the door,
/// Not showing up?, and open under it the hints, each under its bullet with
/// its lines hung under its words, and the serial ports seen.
pub(super) fn empty(ui: &mut Ui, page: &mut Page, face: &Face, room: Rect) -> Drawn {
    let (s, pal) = (face.space, face.pal);
    let g = glyphs();
    let grid = Grid::of(room, face);
    let card = clipped(ui, room, "mp3-empty");
    let floor = if face.live { room.max.y - s.pad_y } else { f32::INFINITY };
    let mut y = room.min.y + s.pad_y;
    if let Some(e) = &page.list_failed {
        let drawn = Para::new(&e.text(), strong(16.0), pal.gold).line(HEAD_H / 16.0).floor(floor).paint(&card, pos2(grid.x, y), grid.w);
        y += drawn.height();
        if let Some(hint) = e.hint() {
            let drawn = Para::new(&hint, body(DIM.0), pal.dim).line(DIM.1 / DIM.0).floor(floor).paint(&card, pos2(grid.x, y + LINE_GAP), grid.w);
            y += LINE_GAP + drawn.height();
        }
    } else if !page.watched {
        searching(&card, face, grid.x, y + studio::NOTE_H / 2.0, grid.w, &Phase::Scanning.text());
        y += studio::NOTE_H;
    } else {
        studio::text(&card, pos2(grid.x, y + HEAD_H / 2.0), &t!("dev.none_title"), strong(16.0), pal.text, grid.w, Align2::LEFT_CENTER);
        y += HEAD_H + s.gap;
        let words = t!("dev.none_body");
        let drawn = Para::new(&words, body(FIRST.0), pal.text).line(FIRST.1 / FIRST.0).floor(floor).paint(&card, pos2(grid.x, y), grid.w);
        y += drawn.height() + METER_GAP;
        searching(&card, face, grid.x, y + studio::NOTE_H / 2.0, grid.w, &t!("dev.none_watching"));
        y += studio::NOTE_H;
    }
    y += s.gap;
    let mut acts = Vec::new();
    if face.live {
        let lead = if page.details { Lead::Open } else { Lead::Shut };
        let at = pos2(grid.x, y + TEXT_BUTTON_H / 2.0);
        let id = Id::new("mp3-empty-door");
        let words = t!("dev.not_showing");
        if studio::text_button(&card, id, at, &words, Some(lead), face.ports, grid.w, face.takes(), pal).clicked() {
            acts.push(Act::Details);
        }
    }
    y += TEXT_BUTTON_H;
    let rest = y + s.pad_y - room.min.y;
    if page.details && face.live {
        y += s.gap;
        let mut hints = vec![t!("dev.no_device_cable").to_string()];
        hints.push(format!("{}\n{}", t!("dev.no_device_driver"), t!("dev.no_device_driver_2")));
        hints.push(t!("dev.no_device_linux").to_string());
        // The cards' two, on the GUI's tab (clause 30).
        if page.transfer.is_some() {
            hints.push(t!("dev.card.hint_card_shows").to_string());
            hints.push(t!("dev.card.hint_own_disks").to_string());
        }
        for (k, hint) in hints.iter().enumerate() {
            if k > 0 {
                y += 6.0;
            }
            let para = Para::new(hint, body(DIM.0), pal.dim).line(DIM.1 / DIM.0).lead(g.bullet).floor(floor);
            y += para.paint(&card, pos2(grid.x, y), grid.w).height();
        }
        y += s.gap;
        let seen: Vec<Run> = if page.others.is_empty() {
            vec![(format!("{} {}", t!("dev.ports_seen"), t!("dev.ports_seen_none")), body(DIM.0), pal.dim)]
        } else {
            vec![
                (format!("{} ", t!("dev.ports_seen")), body(DIM.0), pal.dim),
                (page.others.join(", "), mono(FIGURE), pal.text),
                (format!(" {}", t!("dev.ports_not_bridge")), body(DIM.0), pal.dim),
            ]
        };
        y += Para::runs(&seen).line(DIM.1 / DIM.0).floor(floor).paint(&card, pos2(grid.x, y), grid.w).height();
    }
    let bottom = (y + s.pad_y).min(room.max.y);
    hairline(&card, face, Rect::from_min_max(room.min, pos2(room.max.x, bottom)));
    Drawn { acts, caret: None, rest }
}

/// A child of `ui` over `room`, clipped to it: nothing a card draws spills
/// onto the busy line.
pub(super) fn clipped(ui: &mut Ui, room: Rect, salt: &str) -> Ui {
    let mut child = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(room));
    child.set_clip_rect(room.expand(1.0).intersect(ui.clip_rect()));
    child
}

/// A card's frame: a 1-point hairline in the edge colour round `rect`,
/// never filled and never changing (the boards' card).
pub(super) fn hairline(ui: &Ui, face: &Face, rect: Rect) {
    ui.painter().rect_stroke(rect, studio::square(), Stroke::new(1.0, face.pal.edge), StrokeKind::Inside);
}

/// The head's row from `y`: `name` in the 16-point semibold at the left
/// and, dim in the monospace at the right on the name's baseline, `right`
/// (the port, or with several boards the serial), dropped whole where it
/// would come within 16 points of the name. Returns where the next row
/// starts.
pub(super) fn head(ui: &Ui, face: &Face, grid: Grid, y: f32, name: &str, right: &str) -> f32 {
    let pal = face.pal;
    let mid = y + HEAD_H / 2.0;
    let right_w = studio::width_of(ui, right, mono(FIGURE));
    let name_w = studio::width_of(ui, name, strong(16.0));
    let keep = !right.is_empty() && name_w + CLEAR + right_w <= grid.w;
    let room = if keep { grid.w - right_w - CLEAR } else { grid.w };
    studio::text(ui, pos2(grid.x, mid), name, strong(16.0), pal.text, room, Align2::LEFT_CENTER);
    if keep {
        // A line centred on a point stands on the UI face's baseline at its
        // own size there, the smaller the higher: the figure is centred as
        // far under the name's middle as stands it on the name's.
        let under = |size: f32| {
            let m = studio::metrics(ui, &body(size));
            m.baseline - m.row / 2.0
        };
        let at = pos2(grid.right(), mid + under(16.0) - under(FIGURE));
        studio::text(ui, at, right, mono(FIGURE), pal.dim, right_w + 1.0, Align2::RIGHT_CENTER);
    }
    y + HEAD_H
}

/// One dim line across the content from `y` (the identity line under the
/// head), its figures in the monospace, cut with an ellipsis: it never
/// wraps. Returns where the next row starts.
pub(super) fn dim_line(ui: &Ui, face: &Face, grid: Grid, y: f32, words: &str) -> f32 {
    let runs = studio::figured(words, face.ports, body(DIM.0), mono(FIGURE), face.pal.dim);
    studio::runs(ui, pos2(grid.x, y + DIM.1 / 2.0), &runs, grid.w, Align2::LEFT_CENTER);
    y + DIM.1
}

/// The cells' scan with no share, as the kit's searching shape on the line
/// whose middle is `mid`: ten unlit segments and `words` in the accent 10
/// points after them.
fn searching(ui: &Ui, face: &Face, x: f32, mid: f32, w: f32, words: &str) {
    let pal = face.pal;
    let bar = studio::meter(ui, pos2(x, mid), 10, 0, vec2(5.0, 10.0), 2.0, pal.accent, pal);
    let from = bar.max.x + 10.0;
    studio::text(ui, pos2(from, mid), words, body(13.0), pal.accent, (x + w - from).max(1.0), Align2::LEFT_CENTER);
}

// ── The cells' rows in points ─────────────────────────────────────────────

/// A cells' line as the canvas sets it: the glyph the cells lead it with
/// (a verdict's ✓ ! ✗ ?) as the kit's LED in its colour, their scan widget
/// at its head (its lit cells and the share after them) as the kit's
/// meter, and the rest, each piece in its colour, the bold ones semibold.
struct Lined {
    lamp: Option<Color32>,
    meter: Option<(usize, Option<String>)>,
    spans: Vec<(String, Color32, bool)>,
}

impl Lined {
    fn of(line: &Line, face: &Face) -> Lined {
        let g = glyphs();
        // The cells' spans with a run of spaces taking its neighbour's look,
        // so a sentence in one look is one piece.
        let mut spans: Vec<(String, Style)> = Vec::new();
        for span in &line.spans {
            let words = span.content.to_string();
            match spans.last_mut() {
                Some((last, style)) if *style == span.style || words.trim().is_empty() => last.push_str(&words),
                _ => spans.push((words, span.style)),
            }
        }
        // The scan widget: ten cells, filled to the share.
        let mut meter = None;
        let text: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        let cells: Vec<char> = text.chars().take(10).collect();
        let mark = |glyph: &str, c: char| glyph.chars().count() == 1 && glyph.starts_with(c);
        if cells.len() == 10 && cells.iter().all(|c| mark(g.filled, *c) || mark(g.empty, *c)) {
            let lit = cells.iter().filter(|c| mark(g.filled, **c)).count();
            drop_chars(&mut spans, 10);
            let head: String = spans.first().map(|(s, _)| s.trim_start().to_string()).unwrap_or_default();
            let share = head.split_whitespace().next().filter(|w| w.ends_with('%') && w[..w.len() - 1].chars().all(|c| c.is_ascii_digit()));
            let share = share.map(str::to_string);
            if let (Some(share), Some((first, _))) = (&share, spans.first_mut()) {
                *first = first.trim_start()[share.len()..].to_string();
            }
            meter = Some((lit, share));
        }
        // A verdict's glyph and the space after it.
        let mut lamp = None;
        if meter.is_none()
            && let Some((first, style)) = spans.first_mut()
        {
            for glyph in [g.ok, g.warn, g.no, g.unknown] {
                if let Some(rest) = first.strip_prefix(glyph).and_then(|r| r.strip_prefix(' ')) {
                    lamp = Some(tone(*style, face.pal));
                    *first = rest.to_string();
                    break;
                }
            }
        }
        let spans = spans
            .into_iter()
            .map(|(s, style)| (s, tone(style, face.pal), style.add_modifier.contains(Modifier::BOLD)))
            .filter(|(s, ..)| !s.trim().is_empty())
            .collect::<Vec<_>>();
        Lined { lamp, meter, spans }
    }

    /// Every word is dim: an explanation's line.
    fn dim(&self, face: &Face) -> bool {
        self.lamp.is_none() && self.meter.is_none() && self.spans.iter().all(|(_, c, _)| *c == face.pal.dim)
    }

    /// The words as runs at `size`, the bold ones semibold and the figures
    /// in the monospace.
    fn runs(&self, size: f32, face: &Face) -> Vec<Run> {
        let font = |bold: bool| if bold { strong(size) } else { body(size) };
        let words = |k: usize, s: &str| if k == 0 { s.trim_start().to_string() } else { s.to_string() };
        let runs = self.spans.iter().enumerate().flat_map(|(k, (s, c, bold))| studio::figured(&words(k, s), face.ports, font(*bold), mono(FIGURE), *c));
        runs.collect()
    }
}

/// `spans` less their first `n` characters.
fn drop_chars(spans: &mut Vec<(String, Style)>, mut n: usize) {
    while n > 0 && !spans.is_empty() {
        let len = spans[0].0.chars().count();
        if len <= n {
            spans.remove(0);
            n -= len;
        } else {
            let at = spans[0].0.char_indices().nth(n).map_or(spans[0].0.len(), |(i, _)| i);
            spans[0].0 = spans[0].0[at..].to_string();
            n = 0;
        }
    }
}

/// Where a line centred on its middle stands that middle to share the
/// baseline of a paragraph's first line drawn from `top` in the UI face at
/// `size`: a paragraph stands its baseline its words' ascent under its top
/// (`Para`), a centred line on the UI face's baseline at its middle
/// (`studio::runs`), so the words at a row's right stand on its left's.
fn on_baseline(ui: &Ui, top: f32, size: f32) -> f32 {
    let m = studio::metrics(ui, &body(size));
    top + m.ascent - m.baseline + m.row / 2.0
}

/// The leading that sets `runs` a line `line_h` points apart: the kit's
/// prose takes its leading as a share of its first run's size, which is a
/// figure's smaller one where a line starts with a figure (the card's
/// "21.4 of 59.6 GB used"), so its lines would stand closer than its
/// words'.
fn leading(runs: &[Run], line_h: f32) -> f32 {
    let first = runs.first().map_or(line_h, |(_, font, _)| font.size);
    line_h / first.max(1.0)
}

/// The cells' door mark off a link's words, as the kit's lead: their ▸ a
/// door shut, their ▾ a door open.
fn lead_of(text: &str) -> (Option<Lead>, &str) {
    let g = glyphs();
    let after = |glyph: &str| text.strip_prefix(glyph).and_then(|r| r.strip_prefix(' '));
    match (after(g.forward), after(g.open)) {
        (Some(rest), _) => (Some(Lead::Shut), rest),
        (_, Some(rest)) => (Some(Lead::Open), rest),
        _ => (None, text),
    }
}

/// What kind of row was drawn last in a block: the next row's gap and its
/// words' size follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Line,
    Meter,
    Link,
    Fact,
    Other,
}

/// The cells' rows set in points on a card's [`Grid`], top to bottom from
/// `y`: none past `floor` (a row that would cross it is left out whole, a
/// paragraph's lines past it too, and nothing after it is drawn), what a
/// click asks for gathered in `acts`. A blank row is a block's gap; a row
/// right under another in a block stands its own gap under it (4 points, 6
/// under a meter; facts, links and log lines touch).
pub(super) struct Paint<'u, 'f, 'a> {
    ui: &'u Ui,
    face: &'f Face<'f>,
    grid: Grid,
    pub(super) y: f32,
    pub(super) floor: f32,
    /// A row did not fit: the rows after it are not drawn.
    pub(super) cut: bool,
    last: Option<Kind>,
    salt: Id,
    controls: usize,
    acts: &'a mut Vec<Act>,
}

impl<'u, 'f, 'a> Paint<'u, 'f, 'a> {
    pub(super) fn new(ui: &'u Ui, face: &'f Face<'f>, grid: Grid, y: f32, floor: f32, salt: Id, acts: &'a mut Vec<Act>) -> Self {
        Paint { ui, face, grid, y, floor, cut: false, last: None, salt, controls: 0, acts }
    }

    /// The next control's id: its place among the card's controls.
    fn id(&mut self) -> Id {
        self.controls += 1;
        self.salt.with(self.controls)
    }

    /// A block's gap: the cells' blank row.
    pub(super) fn blank(&mut self) {
        self.y += self.face.space.gap;
        self.last = None;
    }

    /// Room for a row `h` tall `gap` under the last: where it starts, or
    /// none, and nothing more is drawn.
    fn room(&mut self, gap: f32, h: f32) -> Option<f32> {
        let top = self.y + gap;
        if self.cut || top + h > self.floor + 0.01 {
            self.cut = true;
            return None;
        }
        Some(top)
    }

    /// `rows` in order; the log among them is given what the rows after it
    /// leave it. `port` is whose log.
    pub(super) fn rows(&mut self, page: &mut Page, port: &str, rows: &[Row]) {
        for (i, row) in rows.iter().enumerate() {
            if self.cut {
                return;
            }
            let tail: f32 = rows[i + 1..].iter().map(|r| self.fixed(r)).sum();
            self.row(page, port, row, tail);
        }
    }

    /// The height a row after the log takes, for the log to leave it.
    fn fixed(&self, row: &Row) -> f32 {
        match row {
            Row::Blank => self.face.space.gap,
            Row::Link { .. } => TEXT_BUTTON_H,
            Row::Fact { .. } => FACT_H,
            Row::Actions { primary: Some(_), .. } => TALL_H,
            Row::Rule => 2.0 * self.face.space.gap + 1.0,
            Row::Log => 0.0,
            _ => PLAIN.1,
        }
    }

    /// One row; `tail` is what the rows after it take, which a log leaves.
    pub(super) fn row(&mut self, page: &mut Page, port: &str, row: &Row, tail: f32) {
        match row {
            Row::Blank => self.blank(),
            Row::Wide { left, right } => self.line(None, left, right.as_ref(), false),
            Row::Value { label, left, right } => self.line(label.as_deref(), left, right.as_ref(), true),
            Row::Bar { label, permille, color } => self.bar(label.as_deref(), *permille, tone(Style::default().fg(*color), self.face.pal)),
            Row::Link { wide, text, act, note } => self.link(*wide, text, act, note.as_deref()),
            Row::Next { line, reset } => self.next(line, *reset),
            Row::Actions { links, primary, covered } => self.actions(links, primary.as_ref(), *covered),
            Row::Rule => self.rule(),
            Row::Fact { label, value, style } => self.fact(label, value, *style),
            Row::Log => self.log(page, port, tail),
        }
    }

    /// A block's label in the label column, on the line whose middle is
    /// `mid`.
    fn label(&self, label: Option<&str>, mid: f32) {
        if let Some(label) = label {
            let room = (self.grid.label_w - 8.0).max(1.0);
            studio::small_caps(self.ui, pos2(self.grid.x, mid), label, self.face.pal.dim, room, Align2::LEFT_CENTER);
        }
    }

    /// A line of words, in the value column (`value`) or across the
    /// content: its LED or its meter, its words wrapped by words, and its
    /// right-hand words at the column's end where they clear the left's.
    fn line(&mut self, label: Option<&str>, left: &Line, right: Option<&Line>, value: bool) {
        let face = self.face;
        let lined = Lined::of(left, face);
        let first = self.last.is_none() || label.is_some();
        let (size, line_h) = if lined.meter.is_some() {
            SCAN
        } else if first || lined.lamp.is_some() {
            FIRST
        } else if lined.dim(face) {
            DIM
        } else {
            PLAIN
        };
        let gap = match self.last {
            _ if first => 0.0,
            Some(Kind::Meter) => METER_GAP,
            _ => LINE_GAP,
        };
        let Some(top) = self.room(gap, line_h) else { return };
        let mid = top + line_h / 2.0;
        self.label(label, mid);
        let (from, end) = if value { (self.grid.value_x(), self.grid.right()) } else { (self.grid.x, self.grid.right()) };
        let mut x = from;
        if let Some(lamp) = lined.lamp {
            let face_m = studio::metrics(self.ui, &body(size));
            studio::led(self.ui, pos2(x + 3.0, top + face_m.ascent - face_m.small / 2.0), lamp);
            x += studio::STATE_INDENT;
        }
        if let Some((lit, share)) = &lined.meter {
            let bar = studio::meter(self.ui, pos2(x, mid), 10, *lit, vec2(5.0, 10.0), 2.0, face.pal.ok, face.pal);
            x = bar.max.x + SCAN_GAP;
            if let Some(share) = share {
                let drawn = studio::text(self.ui, pos2(x, mid), share, mono(FIGURE), face.pal.dim, (end - x).max(1.0), Align2::LEFT_CENTER);
                x = drawn.max.x + SCAN_GAP;
            }
        }
        let runs = lined.runs(size, face);
        let height = if runs.is_empty() {
            line_h
        } else {
            let para = Para::runs(&runs).line(leading(&runs, line_h)).floor(self.floor);
            para.paint(self.ui, pos2(x, top), (end - x).max(1.0)).height().max(line_h)
        };
        if let Some(right) = right {
            let left_w = x - from + studio::runs_width(self.ui, &runs);
            let mid = on_baseline(self.ui, top, size);
            self.right_words(right, mid, from, end, left_w, height <= line_h + 0.5);
        }
        self.end(top + height);
        self.last = Some(Kind::Line);
    }

    /// A row's right-hand words at `end` (the free figure, gold with its
    /// LED on a card nearly full), only on a one-line row whose left words,
    /// `left_w` from `from`, they clear by 16 points.
    fn right_words(&self, right: &Line, mid: f32, from: f32, end: f32, left_w: f32, one_line: bool) {
        let face = self.face;
        let lined = Lined::of(right, face);
        let runs = lined.runs(PLAIN.0, face);
        let lamp_w = if lined.lamp.is_some() { studio::STATE_INDENT } else { 0.0 };
        let w = studio::runs_width(self.ui, &runs) + lamp_w;
        if !one_line || from + left_w + CLEAR + w > end {
            return;
        }
        if let Some(lamp) = lined.lamp {
            studio::led(self.ui, pos2(end - w + 3.0, studio::led_mid(self.ui, mid, &body(PLAIN.0), false)), lamp);
        }
        studio::runs(self.ui, pos2(end, mid), &runs, w + 1.0, Align2::RIGHT_CENTER);
    }

    /// Where the next row starts, `bottom` or the floor where a
    /// paragraph's lines were left out under it.
    fn end(&mut self, bottom: f32) {
        if bottom > self.floor + 0.01 {
            self.cut = true;
            self.y = self.floor;
        } else {
            self.y = bottom;
        }
    }

    /// The cells' █░ bar as the kit's LED row across the value column, lit
    /// for `permille` in `lit` as the cells fill it.
    fn bar(&mut self, label: Option<&str>, permille: u32, lit: Color32) {
        let first = self.last.is_none() || label.is_some();
        let Some(top) = self.room(if first { 0.0 } else { LINE_GAP }, METER_ROW) else { return };
        self.label(label, top + METER_ROW / 2.0);
        let track = Rect::from_min_size(pos2(self.grid.value_x(), top + (METER_ROW - METER_H) / 2.0), vec2(self.grid.value_w(), METER_H));
        studio::led_fill(self.ui, track, permille, lit, self.face.pal.unlit);
        self.y = top + METER_ROW;
        self.last = Some(Kind::Meter);
    }

    /// A text button in the value column, or across the content (`wide`),
    /// with its dim note 24 points after it.
    fn link(&mut self, wide: bool, text: &str, act: &Act, note: Option<&str>) {
        let gap = if matches!(self.last, None | Some(Kind::Link)) { 0.0 } else { LINE_GAP / 2.0 };
        let Some(top) = self.room(gap, TEXT_BUTTON_H) else { return };
        let face = self.face;
        let x = if wide { self.grid.x } else { self.grid.value_x() };
        let room = self.grid.right() - x;
        let mid = top + TEXT_BUTTON_H / 2.0;
        let (lead, words) = lead_of(text);
        let w = studio::text_button_width(self.ui, words, lead, face.ports).min(room);
        if face.live {
            let id = self.id();
            if studio::text_button(self.ui, id, pos2(x, mid), words, lead, face.ports, room, face.takes(), face.pal).clicked() {
                self.acts.push(act.clone());
            }
        }
        if let Some(note) = note {
            let from = x + w + APART;
            if from + 16.0 < self.grid.right() {
                studio::text(self.ui, pos2(from, mid), note, body(DIM.0), face.pal.dim, self.grid.right() - from, Align2::LEFT_CENTER);
            }
        }
        self.y = top + TEXT_BUTTON_H;
        self.last = Some(Kind::Link);
    }

    /// The board's next write (clause 38): its line in the value column —
    /// "Next write:" dim, the choice in the accent — and Reset at the
    /// column's end on it where it clears the words, else on a row of its
    /// own under them.
    fn next(&mut self, line: &Line, reset: bool) {
        let face = self.face;
        let lined = Lined::of(line, face);
        // The choice's line plain; under it, the image coming and the flags
        // that set it are explanations, dim, as every such line.
        let (size, line_h) = if !lined.spans.is_empty() && lined.dim(face) { DIM } else { PLAIN };
        let gap = if self.last.is_none() { 0.0 } else { LINE_GAP };
        let Some(top) = self.room(gap, line_h) else { return };
        let (x, end) = (self.grid.value_x(), self.grid.right());
        let runs = lined.runs(size, face);
        let words = t!("dev.link_reset").to_string();
        let reset_w = studio::text_button_width(self.ui, &words, None, face.ports);
        let height = if runs.is_empty() {
            line_h
        } else {
            let room = if reset { end - x - reset_w - CLEAR } else { end - x };
            let para = Para::runs(&runs).line(leading(&runs, line_h)).floor(self.floor);
            para.paint(self.ui, pos2(x, top), room.max(1.0)).height().max(line_h)
        };
        let beside = runs.is_empty() || x + studio::runs_width(self.ui, &runs) + CLEAR + reset_w <= end;
        self.end(top + height);
        self.last = Some(Kind::Line);
        if !reset {
            return;
        }
        let mid = if beside {
            on_baseline(self.ui, top, size)
        } else {
            let Some(under) = self.room(0.0, TEXT_BUTTON_H) else { return };
            self.y = under + TEXT_BUTTON_H;
            under + TEXT_BUTTON_H / 2.0
        };
        if face.live {
            let id = self.id();
            let at = pos2(end - reset_w, mid);
            if studio::text_button(self.ui, id, at, &words, None, face.ports, reset_w, face.takes(), face.pal).clicked() {
                self.acts.push(Act::Reset);
            }
        }
    }

    /// The bottom row: the text buttons from the content's left, 24 points
    /// apart, on the row's middle — a button that would run under the
    /// primary is not drawn, its key still works — and the one primary at
    /// the content's end, its why in a tooltip while it waits. Under a
    /// modal that `covered` it the primary keeps its room and is not drawn.
    fn actions(&mut self, links: &[(String, Act)], primary: Option<&(String, bool, Option<String>)>, covered: bool) {
        let face = self.face;
        let h = if primary.is_some() { TALL_H } else { TEXT_BUTTON_H };
        let Some(top) = self.room(0.0, h) else { return };
        let mid = top + h / 2.0;
        let mut links_end = self.grid.right();
        if let Some((label, enabled, why)) = primary {
            let w = studio::tall_button_width(self.ui, label, *enabled).min(self.grid.w);
            links_end -= w + APART;
            if face.live && !covered {
                let id = self.id();
                let rect = Rect::from_min_size(pos2(self.grid.right() - w, top), vec2(w, h));
                let response = studio::tall_button(self.ui, id, rect, label, *enabled, face.takes(), face.pal);
                let response = match why.as_deref().filter(|_| face.takes()) {
                    Some(why) => response.on_hover_text(why),
                    None => response,
                };
                if *enabled && response.clicked() {
                    self.acts.push(Act::Primary);
                }
            }
        }
        let mut x = self.grid.x;
        for (i, (text, act)) in links.iter().enumerate() {
            let (lead, words) = lead_of(text);
            let w = studio::text_button_width(self.ui, words, lead, face.ports);
            if x >= links_end || (i > 0 && x + w > links_end) {
                break;
            }
            if face.live {
                let id = self.id();
                let room = (links_end - x).max(1.0);
                if studio::text_button(self.ui, id, pos2(x, mid), words, lead, face.ports, room, face.takes(), face.pal).clicked() {
                    self.acts.push(act.clone());
                }
            }
            x += w + APART;
        }
        self.y = top + h;
        self.last = Some(Kind::Other);
    }

    /// Details' rule: a hairline across the content, a block's gap above
    /// and under it.
    fn rule(&mut self) {
        let gap = self.face.space.gap;
        let Some(top) = self.room(gap, 1.0) else { return };
        let (from, to) = (pos2(self.grid.x, top + 0.5), pos2(self.grid.right(), top + 0.5));
        self.ui.painter().line_segment([from, to], Stroke::new(1.0, self.face.pal.edge));
        self.y = top + 1.0 + gap;
        self.last = None;
    }

    /// A Details fact: its label, and its value on one line, its figures in
    /// the monospace, cut with an ellipsis at the column's end: it never
    /// wraps.
    fn fact(&mut self, label: &str, value: &str, style: Style) {
        let Some(top) = self.room(0.0, FACT_H) else { return };
        let mid = top + FACT_H / 2.0;
        self.label(Some(label), mid);
        let runs = studio::figured(value, self.face.ports, body(PLAIN.0), mono(FIGURE), tone(style, self.face.pal));
        studio::runs(self.ui, pos2(self.grid.value_x(), mid), &runs, self.grid.value_w(), Align2::LEFT_CENTER);
        self.y = top + FACT_H;
        self.last = Some(Kind::Fact);
    }

    /// The log under a block's gap, in the rows the card leaves it less
    /// `tail` (what the rows after it take), and no more than it has: its
    /// newest lines, `mm:ss` dim and then the line in its kind's colour, all
    /// in the monospace, each cut with an ellipsis at the content's end, or
    /// short of the thumb's room while there are more lines than rows (the
    /// Done board's log; the Failed board's, which all fit, has none) —
    /// scrolled up by the page's own count (the wheel's over it, a line a
    /// notch), else following the newest line on the pane's foot. No room
    /// for a line: no log, and no gap for it.
    fn log(&mut self, page: &mut Page, port: &str, tail: f32) {
        let face = self.face;
        let gap = face.space.gap;
        let lines: Vec<LogLine> = page.log_for(port).into_iter().cloned().collect();
        let top = self.y + gap;
        let room = self.floor - top - tail;
        let rows = (((room + 0.01) / LOG_ROW).floor().max(0.0) as usize).min(lines.len());
        if self.cut || rows == 0 {
            return;
        }
        let pane = Rect::from_min_size(pos2(self.grid.x, top), vec2(self.grid.w, rows as f32 * LOG_ROW));
        if face.takes() && self.ui.rect_contains_pointer(pane) {
            wheel(self.ui, page, rows);
        }
        let scroll = page.log_scroll.min(lines.len() - rows);
        let end = lines.len() - scroll;
        let overflows = lines.len() > rows;
        let bar = if overflows { studio::BAR_ROOM } else { 0.0 };
        let words_w = (pane.width() - CLOCK_W - bar).max(1.0);
        for (k, entry) in lines[end - rows..end].iter().enumerate() {
            let mid = pane.min.y + (k as f32 + 0.5) * LOG_ROW;
            let secs = entry.at.as_secs();
            let clock = format!("{:02}:{:02}", secs / 60, secs % 60);
            studio::text(self.ui, pos2(pane.min.x, mid), &clock, mono(FIGURE), face.pal.dim, CLOCK_W, Align2::LEFT_CENTER);
            let ink = match entry.kind {
                LogKind::Phase => face.pal.accent,
                LogKind::Fact => face.pal.text,
                LogKind::Fail => face.pal.gold,
                LogKind::Quiet => face.pal.dim,
            };
            studio::text(self.ui, pos2(pane.min.x + CLOCK_W, mid), &entry.text, mono(FIGURE), ink, words_w, Align2::LEFT_CENTER);
        }
        if overflows {
            thumb(self.ui, face, pane, rows, lines.len(), end - rows);
        }
        self.y = pane.max.y;
        self.last = Some(Kind::Other);
    }
}

/// The wheel over the log, a line a notch through the page's own count:
/// up stops the follow, down to the newest resumes it, as the cells' wheel.
/// A line's turn is a notch, as the window's grid counts one, a pixel
/// turn a log line's height of points, a page the pane's rows; what is
/// left of a notch waits in egui's memory for the next turn the same way,
/// and a turn the other way starts afresh.
fn wheel(ui: &Ui, page: &mut Page, rows: usize) {
    let turns: Vec<f32> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|event| match event {
                egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Point => delta.y / LOG_ROW,
                    egui::MouseWheelUnit::Page => delta.y * rows.max(1) as f32,
                }),
                _ => None,
            })
            .collect()
    });
    if turns.is_empty() {
        return;
    }
    let key = Id::new("mp3-log-wheel");
    let mut left: f32 = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(0.0);
    for lines in turns.into_iter().filter(|l| l.is_finite()) {
        if lines * left < 0.0 {
            left = 0.0;
        }
        left += lines;
        while left.abs() >= 1.0 {
            page.wheel_log(left > 0.0, rows);
            left -= left.signum();
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(key, left));
}

/// The log's thumb at the pane's right: as long as the share shown, where
/// the view stands (`first` of `len` lines at its top), the kit's thumb at
/// rest; the wheel moves the view, so it takes nothing itself.
fn thumb(ui: &Ui, face: &Face, pane: Rect, rows: usize, len: usize, first: usize) {
    let h = (pane.height() * rows as f32 / len as f32).max(studio::THUMB_MIN).min(pane.height());
    let travel = pane.height() - h;
    let top = pane.min.y + travel * first as f32 / (len - rows).max(1) as f32;
    let rect = Rect::from_min_size(pos2(pane.max.x - studio::THUMB_W, top), vec2(studio::THUMB_W, h));
    let rest = face.pal.dim.gamma_multiply(studio::THUMB_REST);
    ui.painter().rect_filled(rect, egui::CornerRadius::same((studio::THUMB_W / 2.0) as u8), rest);
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(super) mod tests {
    //! The write on the board card run headless, as the frame's tests run
    //! the page (egui_view/tests.rs) and on the page's own fixtures: the
    //! write's steps over its bar, the lock, Done with Details open, a
    //! failure, and a format on its own. The helpers are the gate's tests'
    //! too (gate.rs).

    use std::time::{Duration, Instant};

    use egui::epaint::Shape;
    use egui::{Event, FullOutput, PointerButton, Pos2, Vec2};

    use super::super::{COLUMN_W, ROOMY, show};
    use super::*;
    use crate::admin::{EguiShown, Screen};
    use crate::device::board::{CardRoot, Clock, Formatted};
    use crate::device::desk::Event as Told;
    use crate::device::firmware::Mode;
    use crate::device::page::{Kind as GateKind, fixtures};
    use crate::gui::window::studio::Pal;

    pub(in super::super) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// The page's area in points: in the GUI's default 100×30 window, at
    /// the GUI's floor (100×24), and in the boards' 1440 by 900 window.
    pub(in super::super) const WINDOW: Vec2 = vec2(800.0, 448.0);
    pub(in super::super) const FLOOR: Vec2 = vec2(800.0, 352.0);
    pub(in super::super) const BOARD: Vec2 = vec2(1440.0, 824.0);

    /// A context styled as the host's, its passes run over a page's area.
    pub(in super::super) struct Headless {
        pub(in super::super) ctx: egui::Context,
        size: Vec2,
    }

    impl Headless {
        pub(in super::super) fn new(size: Vec2) -> Headless {
            let ctx = egui::Context::default();
            crate::gui::window::panel::fonts(&ctx);
            studio::style_once(&ctx, &Pal::now());
            Headless { ctx, size }
        }

        /// One pass, and what the page told its host.
        pub(in super::super) fn shown(&self, page: &mut Page) -> EguiShown {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), ..Default::default() };
            let mut told = None;
            let mut out = self.ctx.run_ui(input, |ui| told = Some(show(ui, page, "", false)));
            out.textures_delta.clear();
            told.expect("the pass ran")
        }

        /// One pass with `events`: what egui drew.
        pub(in super::super) fn pass(&self, page: &mut Page, events: Vec<Event>) -> FullOutput {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), events, ..Default::default() };
            let mut out = self.ctx.run_ui(input, |ui| {
                let _ = show(ui, page, "", false);
            });
            // No GPU here to take the font atlas: let go of it on purpose, or
            // epaint's debug check fails the drop.
            out.textures_delta.clear();
            out
        }

        /// Two passes (egui sizes a modal's area on the first), the second's.
        pub(in super::super) fn settle(&self, page: &mut Page) -> FullOutput {
            let _ = self.pass(page, Vec::new());
            self.pass(page, Vec::new())
        }

        /// A press and a release at `at`, a pass each after the move.
        pub(in super::super) fn click(&self, page: &mut Page, at: Pos2) {
            let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            for events in [vec![Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
                let _ = self.pass(page, events);
            }
        }

        /// A click on the middle of what `id` drew on the last pass.
        pub(in super::super) fn click_id(&self, page: &mut Page, id: Id) {
            self.click(page, self.rect_of(id).center());
        }

        /// Where `id` was drawn on the last pass.
        pub(in super::super) fn rect_of(&self, id: Id) -> Rect {
            self.ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect
        }
    }

    /// Every text a pass drew, whole, with where.
    pub(in super::super) fn texts(out: &FullOutput) -> Vec<(String, Rect)> {
        galleys(out).into_iter().map(|(pos, g)| (g.text().to_string(), Rect::from_min_size(pos, g.size()))).collect()
    }

    /// Every galley a pass drew, with where.
    pub(in super::super) fn galleys(out: &FullOutput) -> Vec<(Pos2, std::sync::Arc<egui::Galley>)> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) => Some((t.pos, t.galley.clone())),
                _ => None,
            })
            .collect()
    }

    pub(in super::super) fn drew(out: &FullOutput, words: &str) -> bool {
        texts(out).iter().any(|(t, _)| t == words)
    }

    /// Where the pass drew `words`.
    pub(in super::super) fn find(out: &FullOutput, words: &str) -> Rect {
        let all = texts(out);
        all.iter().find(|(t, _)| t == words).map(|(_, r)| *r).unwrap_or_else(|| panic!("{words:?} drawn: {:?}", all.iter().map(|(t, _)| t).collect::<Vec<_>>()))
    }

    pub(in super::super) fn assert_drew(out: &FullOutput, words: &[&str]) {
        for w in words {
            assert!(drew(out, w), "{w:?} drawn: {:?}", texts(out).into_iter().map(|(t, _)| t).collect::<Vec<_>>());
        }
    }

    /// The colours `words` was set in, run by run, and their faces.
    pub(in super::super) fn looks(out: &FullOutput, words: &str) -> Vec<(Color32, egui::FontId)> {
        let galley = galleys(out).into_iter().find(|(_, g)| g.text() == words).unwrap_or_else(|| panic!("{words:?} drawn")).1;
        galley.job.sections.iter().map(|s| (s.format.color, s.format.font_id.clone())).collect()
    }

    /// The one colour `words` was set in.
    pub(in super::super) fn color_of(out: &FullOutput, words: &str) -> Color32 {
        let colors: Vec<Color32> = looks(out, words).into_iter().map(|(c, _)| c).collect();
        assert!(colors.windows(2).all(|w| w[0] == w[1]), "{words:?} in one colour: {colors:?}");
        colors[0]
    }

    /// The rects a pass filled with `fill`, `h` points tall: an LED row's
    /// segments.
    pub(in super::super) fn filled(out: &FullOutput, fill: Color32, h: f32) -> Vec<Rect> {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Rect(r) if r.fill == fill && (r.rect.height() - h).abs() < 0.01 => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    /// The LEDs a pass lit in `color`: the 6-point squares.
    pub(in super::super) fn leds(out: &FullOutput, color: Color32) -> Vec<Rect> {
        filled(out, color, 6.0).into_iter().filter(|r| (r.width() - 6.0).abs() < 0.01).collect()
    }

    /// A page on quiet channels, its worker's reports read.
    pub(in super::super) fn page_with(tell: impl FnOnce(&Ends)) -> (Page, Ends) {
        let (mut page, ends) = Page::quiet();
        tell(&ends);
        page.pump();
        (page, ends)
    }

    /// COM3's update said yes to, the page locked: what the worker was
    /// sent read away.
    fn locked(tell: impl FnOnce(&Ends)) -> (Page, Ends) {
        let (mut page, ends) = page_with(tell);
        page.act(Act::Primary);
        page.act(Act::Write);
        assert!(page.writing(), "the gate's yes locks the page");
        let _ = ends.sent();
        (page, ends)
    }

    /// COM3 at work `work`, as the worker tells it during the write.
    fn at_work(work: Work) -> Board {
        let mut board = fixtures::old("COM3", "v0.7.0");
        board.work = work;
        board
    }

    #[test]
    fn the_write_says_each_step_on_the_firmware_row_over_its_bar_lit_as_the_cells_fill_theirs() {
        let _en = english();
        let probe = Headless::new(WINDOW);
        let pal = Pal::now();
        let (mut page, ends) = locked(Ends::update_available);
        // Every phase the worker reports, with the share its bar shows: none
        // until the copy says its percent, whole while it is checked and the
        // board restarts.
        let steps = [
            (Work::Queued, 0),
            (Work::Writing { phase: Phase::Connecting, pct: None }, 0),
            (Work::Writing { phase: Phase::Reading, pct: None }, 0),
            (Work::Writing { phase: Phase::Erasing, pct: None }, 0),
            (Work::Writing { phase: Phase::Comparing, pct: None }, 0),
            (Work::Writing { phase: Phase::Writing, pct: None }, 0),
            (Work::Writing { phase: Phase::Writing, pct: Some(1) }, 10),
            (Work::Writing { phase: Phase::Writing, pct: Some(62) }, 620),
            (Work::Writing { phase: Phase::Writing, pct: Some(99) }, 990),
            (Work::Writing { phase: Phase::Verifying, pct: Some(100) }, 1000),
            (Work::Writing { phase: Phase::Restarting, pct: Some(100) }, 1000),
        ];
        for (work, permille) in steps {
            let board = at_work(work.clone());
            ends.tell([Told::Board(board.clone())]);
            page.pump();
            let out = probe.settle(&mut page);
            let words = page.write_words(&board);
            assert_drew(&out, &[&words, "FIRMWARE"]);
            assert!(looks(&out, &words).iter().all(|(c, _)| *c == pal.accent), "{words:?} in the accent, the cells' step words");
            assert!(looks(&out, &words).iter().all(|(_, f)| *f == body(FIRST.0) || *f == mono(FIGURE)), "regular: work, not a verdict");
            let (lit, unlit) = (filled(&out, pal.accent, METER_H).len(), filled(&out, pal.unlit, METER_H).len());
            assert!(lit + unlit > 60, "{work:?}: the bar across the value column, {lit} lit and {unlit} not");
            assert_eq!(lit, studio::fill_lit(permille, lit + unlit), "{work:?}: {permille}‰ lit as the cells fill their bar");
            // The bar 4 points under the words.
            let segment = filled(&out, pal.accent, METER_H).into_iter().chain(filled(&out, pal.unlit, METER_H)).next().expect("a segment");
            let bar_top = segment.min.y - (METER_ROW - METER_H) / 2.0;
            assert!((bar_top - (find(&out, &words).max.y + LINE_GAP)).abs() < 2.0, "{work:?}: {bar_top} under {:?}", find(&out, &words));
            // The card row untouched, dim; the bottom row the door alone.
            let untouched = t!("dev.card_untouched").to_string();
            assert_eq!(color_of(&out, &untouched), pal.dim);
            assert_drew(&out, &["Details"]);
            for never in ["UPDATE", "Advanced…", "Show on the player", "Ask again"] {
                assert!(!drew(&out, never), "{work:?}: {never:?} under the lock");
            }
        }
        // The time left, once the rate is known, at the end of the words.
        let (mut page, _ends) = locked(Ends::update_available);
        let t0 = Instant::now();
        for (pct, after) in [(10, 0), (20, 1), (40, 10)] {
            page.apply_at(Told::Board(at_work(Work::Writing { phase: Phase::Writing, pct: Some(pct) })), t0 + Duration::from_secs(after));
        }
        let out = probe.settle(&mut page);
        assert_drew(&out, &["writing v0.8.0… 40% · about 20 s left"]);
        let figures: Vec<egui::FontId> = looks(&out, "writing v0.8.0… 40% · about 20 s left").into_iter().map(|(_, f)| f).collect();
        assert_eq!(figures.iter().filter(|f| **f == mono(FIGURE)).count(), 3, "the version, the percent and the seconds in the monospace: {figures:?}");
    }

    #[test]
    fn under_the_lock_another_board_waits_its_turn_and_its_primary_says_why_under_the_pointer() {
        let _en = english();
        let probe = Headless::new(WINDOW);
        let pal = Pal::now();
        let (mut page, ends) = locked(|e| e.boards(vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0")]));
        ends.writing(62);
        page.pump();
        page.act(Act::Board(1));
        let out = probe.settle(&mut page);
        let why = t!("dev.fw_one_at_a_time", port = "COM3").to_string();
        assert_drew(&out, &[&why, "UPDATE", "Update available · v0.7.0 → v0.8.0"]);
        assert_eq!(color_of(&out, &why), pal.dim, "the one-at-a-time line, dim");
        assert_eq!(color_of(&out, "UPDATE"), pal.dim, "the kit's disabled frame: dim, no pointer");
        for never in ["Show on the player", "Advanced…", "Ask again"] {
            assert!(!drew(&out, never), "{never:?} under the lock");
        }
        // Under the pointer it says why, as the cells' tooltip does.
        let at = find(&out, "UPDATE").center();
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(at)]);
        let out = probe.settle(&mut page);
        assert_eq!(texts(&out).iter().filter(|(t, _)| *t == why).count(), 2, "the line and its tooltip");
        // And a click on it asks for nothing.
        probe.click(&mut page, at);
        assert!(page.gate.is_none() && ends.sent().is_empty(), "the primary waits its turn");
        // The footer's warning is the cells' hint, never cut.
        assert!(Screen::hint(&page).starts_with(&t!("dev.hint_working_port", port = "COM3").to_string()), "{}", Screen::hint(&page));
        // The tab of the board being written still takes the view back.
        probe.click_id(&mut page, Id::new("mp3-tab").with(0usize));
        assert_eq!(page.open.as_deref(), Some("COM3"));
        let out = probe.settle(&mut page);
        assert_drew(&out, &["writing v0.8.0… 62%"]);
        assert!(ends.sent().is_empty());
    }

    /// COM3 written the pin, checked, restarted and answering again, its
    /// firmware the one that formats (`clock=`).
    fn written_answering() -> Board {
        let mut board = fixtures::answering_with("COM3", "v0.8.0", |s| s.clock = Clock::Ok);
        board.probe = Some(Ok(crate::device::board::Probe { info: fixtures::info("COM3"), on_board: Some(fixtures::ours("v0.8.0")) }));
        board.written = Some(Written::Done {
            version: "v0.8.0".into(),
            took: Duration::from_secs(41),
            skipped: false,
            install: false,
            boot: Some("mstream-mp3-player v0.8.0 (commit 4e94418, 2026-10-10), ELF 11c35a4a".into()),
            image: crate::device::firmware::tests::pin_facts("v0.8.0", Mode::Qio),
            looping: None,
        });
        fixtures::judged(board)
    }

    /// A log line about COM3 of `kind`, as the worker tells one.
    fn log(text: &str, kind: LogKind) -> Told {
        Told::Log { port: Some("COM3".into()), text: text.into(), kind }
    }

    #[test]
    fn done_is_the_card_again_with_details_open_on_its_facts_its_log_and_its_two_links() {
        let _en = english();
        let probe = Headless::new(BOARD);
        let pal = Pal::now();
        let (mut page, ends) = locked(Ends::update_available);
        ends.writing(62);
        ends.tell([log("writing… 100%", LogKind::Phase), log("mstream-mp3-player v0.8.0", LogKind::Quiet)]);
        ends.tell([Told::Board(written_answering())]);
        page.pump();
        assert!(!page.writing(), "the lock ended with the restart");
        let out = probe.settle(&mut page);
        assert_drew(&out, &["Up to date · v0.8.0 · just written", "21.4 of 59.6 GB used · 1,284 tracks", "Details", "Advanced…"]);
        assert!(!drew(&out, "UPDATE") && !drew(&out, "TRY AGAIN"), "nothing to do: no primary");
        assert_eq!(leds(&out, pal.ok).len(), 1, "the chip's green LED");
        // Details open: the facts, the write's log, then Write again and the
        // format, as the cells offer them only with Details open.
        assert!(!drew(&out, "Write v0.8.0 again"), "Details closed: no Write again");
        probe.click(&mut page, find(&out, "Details").center());
        assert!(page.details);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["PORT", "BOARD", "ON THE BOARD", "THIS PLAYER", "WRITTEN", "CARD"]);
        assert_drew(&out, &["writing… 100%", "mstream-mp3-player v0.8.0", "Write v0.8.0 again", "Format as FAT32…", "erases the whole card"]);
        assert_eq!(color_of(&out, "writing… 100%"), pal.accent, "a phase in the accent");
        assert_eq!(color_of(&out, "mstream-mp3-player v0.8.0"), pal.dim, "the boot line dim");
        let (again, format) = (find(&out, "Write v0.8.0 again"), find(&out, "Format as FAT32…"));
        assert!(format.min.y > again.max.y && find(&out, "writing… 100%").max.y < again.min.y, "under the log, Write again then the format");
        // Each asks for the cells' act: its gate.
        probe.click(&mut page, again.center());
        assert!(page.gate.as_ref().is_some_and(|g| g.kind == GateKind::Again), "the cells' WriteAgain");
        page.act(Act::Keep);
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "Format as FAT32…").center());
        assert!(page.gate.as_ref().is_some_and(|g| g.kind == GateKind::Format), "the cells' Format");
        let sent = ends.sent();
        assert!(matches!(sent.as_slice(), [Cmd::Facts { .. }]), "only Details' facts were asked: {sent:?}");
    }

    #[test]
    fn a_failed_write_opens_details_on_its_newest_line_and_try_again_is_the_primary() {
        let _en = english();
        let probe = Headless::new(BOARD);
        let pal = Pal::now();
        let (mut page, ends) = locked(Ends::update_available);
        ends.writing(38);
        ends.tell((0..40).map(|i| log(&format!("wrote chunk {i:02}"), LogKind::Fact)));
        ends.tell([log("the board stopped answering at 38% — unplugged?", LogKind::Fail)]);
        ends.failed();
        page.pump();
        assert!(!page.writing() && page.details, "the lock ended, and Details opened by itself");
        let out = probe.settle(&mut page);
        let chip = "Write failed · the board is half written";
        assert_drew(&out, &[chip, "It cannot start until it is written again.", "unknown until the firmware runs again"]);
        assert_eq!(looks(&out, chip)[0], (pal.gold, strong(FIRST.0)), "the chip's words semibold in the gold");
        assert_eq!(color_of(&out, "It cannot start until it is written again."), pal.dim, "its line dim, never gold");
        assert_eq!(leds(&out, pal.gold).len(), 1, "the chip's gold LED");
        assert_eq!(leds(&out, pal.dim).len(), 1, "the card row's dim one");
        assert_eq!(color_of(&out, "unknown until the firmware runs again"), pal.dim);
        assert_drew(&out, &["TRY AGAIN", "PORT"]);
        assert!(!drew(&out, "BOARD") && !drew(&out, "THIS PLAYER"), "after a failure the port alone, then the log");
        // The newest line on the pane's foot, in the gold; the oldest above
        // the pane; the thumb at the foot of its travel.
        let fail = "the board stopped answering at 38% — unplugged?";
        assert_eq!(color_of(&out, fail), pal.gold);
        let lowest = texts(&out).into_iter().filter(|(t, _)| t.starts_with("wrote chunk") || t == fail).max_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
        assert_eq!(lowest.map(|(t, _)| t).as_deref(), Some(fail), "following the newest line");
        assert!(!drew(&out, "wrote chunk 00"), "the oldest lines above the pane");
        // Try again: the gate again, never a write.
        probe.click(&mut page, find(&out, "TRY AGAIN").center());
        assert!(page.gate.as_ref().is_some_and(|g| g.kind == GateKind::Again), "half written on ours: Write again's gate");
        assert!(ends.sent().is_empty(), "nothing written before the gate's yes");
    }

    #[test]
    fn at_the_guis_floor_a_failure_still_shows_its_newest_lines_under_try_again() {
        let _en = english();
        let probe = Headless::new(FLOOR);
        let (mut page, ends) = locked(Ends::update_available);
        ends.writing(38);
        ends.tell((0..40).map(|i| log(&format!("wrote chunk {i:02}"), LogKind::Fact)));
        let fail = "the board stopped answering at 38% — unplugged?";
        ends.tell([log(fail, LogKind::Fail)]);
        ends.failed();
        page.pump();
        let out = probe.settle(&mut page);
        // The tighter measures, so the log has rows: its newest line whole
        // over the busy line, the primary whole too.
        let busy_top = FLOOR.y - super::super::TIGHT.under_busy - studio::NOTE_H;
        let line = find(&out, fail);
        assert!(line.max.y <= busy_top, "the failure on the card: {line:?} over {busy_top}");
        assert!(drew(&out, "wrote chunk 39"), "and the line before it");
        let primary = find(&out, "TRY AGAIN");
        assert!(primary.max.y < line.min.y, "under Try again");
        // Details closed, the card is the card at rest, as tall as ever.
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        assert!(!drew(&out, fail) && drew(&out, "TRY AGAIN"));
    }

    #[test]
    fn a_log_that_fits_runs_to_the_cards_end_and_one_that_does_not_leaves_its_thumb_room() {
        let _en = english();
        let probe = Headless::new(BOARD);
        let long = "x".repeat(400);
        let (mut page, ends) = page_with(Ends::up_to_date);
        ends.tell([log(&long, LogKind::Fact)]);
        page.pump();
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        let cut = texts(&out).into_iter().find(|(t, _)| t.starts_with("xxx")).expect("the long line, cut").1;
        let end = BOARD.x / 2.0 + COLUMN_W / 2.0 - ROOMY.pad_x;
        let reaches = cut.max.x > end - studio::BAR_ROOM + 1.0 && cut.max.x <= end + 0.5;
        assert!(reaches, "a log with no more lines than rows runs into the thumb's room, to the content's end: {cut:?} to {end}");
        ends.tell((0..80).map(|i| log(&format!("line {i:02}"), LogKind::Fact)));
        ends.tell([log(&long, LogKind::Fact)]);
        page.pump();
        let out = probe.settle(&mut page);
        let cut = texts(&out).into_iter().filter(|(t, _)| t.starts_with("xxx")).max_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y)).expect("the newest long line").1;
        assert!(cut.max.x <= end - studio::BAR_ROOM + 1.0, "one that overflows leaves the thumb its room: {cut:?}");
    }

    /// A board whose firmware formats, its card exFAT, its format at
    /// `state` (the page's own tests' `formats` and `at`).
    fn formatting(state: Format) -> Board {
        formatting_with(state, |_| {})
    }

    /// The same, its status changed after.
    fn formatting_with(state: Format, change: impl FnOnce(&mut crate::device::board::Status)) -> Board {
        let mut board = fixtures::answering_with("COM3", "v0.8.0", |s| {
            (s.cid, s.clock, s.card) = (CardRoot::None, Clock::Ok, CardKind::ExFat);
            change(s);
        });
        let going = matches!(state, Format::Asking | Format::Waiting | Format::Restarting | Format::Running { .. });
        board.work = if going { Work::Formatting } else { Work::Idle };
        board.format = state;
        board
    }

    #[test]
    fn a_format_on_its_own_is_the_sd_rows_scan_its_share_and_its_time_left() {
        let _en = english();
        let probe = Headless::new(WINDOW);
        let pal = Pal::now();
        let (mut page, ends) = page_with(|e| e.boards(vec![formatting(Format::Idle)]));
        let out = probe.settle(&mut page);
        assert_drew(&out, &["exFAT card · the player reads FAT32 only", "FORMAT AS FAT32"]);
        // The player asked: its question, then its percent and its time left.
        ends.tell([Told::Board(formatting(Format::Waiting))]);
        page.pump();
        let out = probe.settle(&mut page);
        let waiting = t!("dev.card.fmt_waiting").to_string();
        assert_drew(&out, &[&waiting, &t!("dev.card.fmt_waiting_2")]);
        assert_eq!(color_of(&out, &waiting), pal.accent);
        assert_eq!(filled(&out, pal.ok, 2.0).len(), 0, "no share yet: no segment lit");
        for never in ["FORMAT AS FAT32", "Advanced…", "Format as FAT32…"] {
            assert!(!drew(&out, never), "{never:?} while the player formats");
        }
        ends.tell([Told::Board(formatting(Format::Restarting)), Told::Board(formatting(Format::Running { pct: 41 }))]);
        page.pump();
        let out = probe.settle(&mut page);
        let running = t!("dev.card.fmt_running").to_string();
        assert_drew(&out, &[&running, "41%"]);
        assert_eq!(looks(&out, "41%"), [(pal.dim, mono(FIGURE))], "the share dim, a figure");
        assert_eq!(color_of(&out, &running), pal.accent);
        // The scan's ten segments, four of them lit in the green (each a
        // stack of the kit's 2-point lines), and its words after the share.
        let lit_columns = |c: Color32| {
            let mut xs: Vec<i32> = filled(&out, c, 2.0).iter().filter(|r| (r.width() - 5.0).abs() < 0.01).map(|r| r.min.x as i32).collect();
            xs.dedup();
            xs.len()
        };
        assert_eq!((lit_columns(pal.ok), lit_columns(pal.unlit)), (4, 6), "41% lights four of the ten");
        assert!(find(&out, "41%").max.x < find(&out, &running).min.x, "the share before the words");
        let left = texts(&out).into_iter().find(|(t, _)| t.contains("leaving the tab")).expect("the time left").0;
        assert!(left.starts_with("about ") && left.contains(" s left"), "{left:?}");
        // Done: a FAT card's row again, and where to fill it.
        let done = Formatted { size: 59_617_918_976, free: 59_517_918_976, cluster: 32_768, took: Duration::from_secs(41) };
        let empty = formatting_with(Format::Done(done), |s| {
            (s.card, s.free, s.tracks) = (CardKind::Fat32, Free::Bytes(59_517_918_976), Tracks::Count(0));
        });
        ends.tell([Told::Board(empty)]);
        page.pump();
        let out = probe.settle(&mut page);
        assert_drew(&out, &["0.1 of 59.6 GB used · no tracks", &t!("dev.card.fmt_done")]);
        assert!(!drew(&out, "FORMAT AS FAT32"), "the player's own card: no primary");
        assert!(ends.sent().is_empty(), "drawing asks the worker nothing");
    }
}
