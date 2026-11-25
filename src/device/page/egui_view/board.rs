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

use egui::{Align2, Color32, Id, Rect, Ui, UiBuilder, pos2, vec2};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use super::super::cells::{self, Row};
use super::super::*;
use super::{Drawn, Face, tone};
use crate::gui::window::studio::{self, HEAD_H, Lead, Pal, Para, Run, TALL_H, TEXT_BUTTON_H, body, mono, strong};

/// The cells' line, too wide to wrap in: the rows come a sentence each, and
/// the value column wraps them by words in points. The measures from here
/// on are a card's view's too (card.rs).
pub(super) const WIDE: u16 = 4000;
/// The label column: the boards' 140 points, less on a narrow card.
const LABEL_W: f32 = 140.0;
/// A value's lines, their words' size and their row: a block's first line
/// (a verdict) 14 on 20, a plain one (the figures under a meter) 13.5 on
/// 20, an explanation's dim one 13 on 18; each 4 points under the last, 6
/// under a meter.
pub(super) const FIRST: (f32, f32) = (14.0, 20.0);
pub(super) const PLAIN: (f32, f32) = (13.5, 20.0);
pub(super) const DIM: (f32, f32) = (13.0, 18.0);
pub(super) const LINE_GAP: f32 = 4.0;
pub(super) const METER_GAP: f32 = 6.0;
/// A line led by the cells' scan (asking, reading, counting, formatting):
/// the kit's scan on the row's 20 points (`studio::scan_lead`), its words
/// 13, 10 points after the segments, and after the share where there is
/// one.
pub(super) const SCAN: (f32, f32) = (13.0, 20.0);
/// A meter's row, and its segments' height in it.
pub(super) const METER_ROW: f32 = 20.0;
pub(super) const METER_H: f32 = 10.0;
/// A Details fact's row, a log line's, and the log's clock column (its
/// `mm:ss` and two spaces in the monospace).
pub(super) const FACT_H: f32 = 20.0;
const LOG_ROW: f32 = 18.0;
const CLOCK_W: f32 = 53.0;
/// Text buttons in a row, the boards' 24 points apart, and a note after
/// its button.
pub(super) const APART: f32 = 24.0;
/// Words at a row's right come no nearer the words at its left than this,
/// or are dropped whole.
pub(super) const CLEAR: f32 = 16.0;
/// The figures' one size in the card.
pub(super) const FIGURE: f32 = 12.5;

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

    pub(super) fn value_x(&self) -> f32 {
        self.x + self.label_w
    }

    pub(super) fn value_w(&self) -> f32 {
        (self.w - self.label_w).max(1.0)
    }

    pub(super) fn right(&self) -> f32 {
        self.x + self.w
    }

    /// A block's label in the label column on the line whose middle is
    /// `mid`: the kit's small capitals, dim, cut 8 points short of the
    /// values. A card's view labels its rows and its facts so too (card.rs).
    pub(super) fn label(&self, ui: &Ui, label: &str, mid: f32, pal: &Pal) {
        let room = (self.label_w - 8.0).max(1.0);
        studio::small_caps(ui, pos2(self.x, mid), label, pal.dim, room, Align2::LEFT_CENTER);
    }

    /// The cells' █░ bar on a meter's row from `top`, as the kit's LED row
    /// across the value column, lit for `permille` in `lit` as the cells
    /// fill it.
    pub(super) fn bar(&self, ui: &Ui, top: f32, permille: u32, lit: Color32, pal: &Pal) {
        let track = Rect::from_min_size(pos2(self.value_x(), top + (METER_ROW - METER_H) / 2.0), vec2(self.value_w(), METER_H));
        studio::led_fill(ui, track, permille, lit, pal.unlit);
    }
}

/// The board in view, its card in the column's `room` (cells.rs's `card`):
/// its head, its value blocks, its bottom row, and Details while open. The
/// card is as tall as its rows and no taller than the room; short of room
/// it keeps what [`keep`] keeps, in its order — the bottom row whole with
/// its primary at every size the page is drawn at, the block that says what
/// the primary will do ([`says`]: the one the primary acts on, Firmware or
/// the SD card), Details' rule and its links, its log's newest lines, its
/// facts, then the other value rows, whole lines from the foot — and its
/// log takes the room left, down to the room's foot. Its rest is its height
/// with every row and its log at [`LOG_REST`] lines, which the frame
/// measures.
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face, board: &Board, room: Rect) -> Drawn {
    let s = face.space;
    let grid = Grid::of(room, face);
    let mut card = clipped(ui, room, "mp3-board");
    let mut y = room.min.y + s.pad_y;
    y = studio::card_head(&card, pos2(grid.x, y), grid.w, &cells::board_name(board), &cells::head_right(page, board), face.pal);
    if let Some(words) = cells::identity(page, board) {
        y = studio::dim_line(&card, pos2(grid.x, y + LINE_GAP), grid.w, &words, face.ports, face.pal);
    }
    let actions = cells::actions_row(page, board);
    let actions_h = match &actions {
        Row::Actions { primary: Some(_), .. } => TALL_H,
        _ => TEXT_BUTTON_H,
    };
    let blocks = cells::value_blocks(page, board, WIDE);
    let details = if page.details { cells::details_rows(page, board) } else { Vec::new() };
    let port = board.port();
    // The value rows a row at a time, each with its block; the blocks that
    // say what the primary will do, the one it acts on among them.
    let flat: Vec<(usize, &Row)> = blocks.iter().enumerate().flat_map(|(b, rows)| rows.iter().map(move |r| (b, r))).collect();
    let acts_on = match page.primary_of(board) {
        Some(Primary::Format) => Some(1),
        Some(_) => Some(0),
        None => None,
    };
    let said: Vec<bool> = blocks.iter().enumerate().map(|(b, rows)| acts_on == Some(b) || says(&rows.iter().collect::<Vec<_>>(), face)).collect();
    let says_rows: Vec<usize> = (0..flat.len()).filter(|&i| said[flat[i].0]).collect();
    // The other rows go a line at a time from the foot: the board has no
    // give-way of its own (contract clauses 4 and 20a).
    let give_way: Vec<Vec<usize>> = (0..flat.len()).rev().filter(|&i| !said[flat[i].0]).map(|i| vec![i]).collect();
    let want = Want::of(page, port, face, room, y - room.min.y + s.gap + actions_h + s.pad_y, flat.len(), give_way, says_rows, &details);
    let salt = Id::new("mp3-board").with(port);
    // The value rows `keep` asks after, measured as they are drawn: blocks
    // a block's gap apart, under the head.
    let quiet = Face { live: false, ..*face };
    let mut probe = card.new_child(UiBuilder::new().id_salt("mp3-board-measure").max_rect(room));
    probe.set_invisible();
    let kept = keep(&want, face, |mask| {
        let mut none = Vec::new();
        let mut paint = Paint::new(&probe, &quiet, grid, y, f32::INFINITY, salt, &mut none);
        values(&mut paint, &flat, mask);
        paint.y - y
    });
    // The drawing holds to the room's foot whatever the measures say.
    let foot = if face.live { room.max.y - s.pad_y } else { f32::INFINITY };
    let mut acts = Vec::new();
    let mut paint = Paint::new(&card, face, grid, y, foot - actions_h - s.gap, salt, &mut acts);
    values(&mut paint, &flat, &kept.values);
    paint.floor = foot;
    paint.cut = false;
    paint.blank();
    paint.row(page, port, &actions);
    if face.live {
        paint.log = kept.log();
        for row in kept.details(&details) {
            if paint.cut {
                break;
            }
            paint.row(page, port, row);
        }
    }
    // Its rest: every row, Details' too while it is open — a failed write
    // opens it by itself on the log's newest lines (the Failed board) — so
    // the frame closes up to show them rather than leave them under the
    // card's foot.
    let rest = want.fixed + kept.height;
    let bottom = (paint.y + s.pad_y).min(room.max.y);
    studio::hairline(&card, Rect::from_min_max(room.min, pos2(room.max.x, bottom)), face.pal);
    Drawn { acts, caret: None, rest }
}

/// The kept value rows of `flat` (each with its block) through `paint`,
/// `mask` saying which, each block with a row kept a block's gap under the
/// last.
fn values(paint: &mut Paint, flat: &[(usize, &Row)], mask: &[bool]) {
    let mut block = None;
    for (i, (b, row)) in flat.iter().enumerate() {
        if !mask[i] || paint.cut {
            continue;
        }
        if block != Some(*b) {
            paint.blank();
            block = Some(*b);
        }
        paint.value(row);
    }
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
        let drawn = Para::new(&e.text(), strong(16.0), pal.gold).spaced(HEAD_H).floor(floor).paint(&card, pos2(grid.x, y), grid.w);
        y += drawn.height();
        if let Some(hint) = e.hint() {
            let drawn = Para::new(&hint, body(DIM.0), pal.dim).spaced(DIM.1).floor(floor).paint(&card, pos2(grid.x, y + LINE_GAP), grid.w);
            y += LINE_GAP + drawn.height();
        }
    } else if !page.watched {
        studio::searching_at(&card, pos2(grid.x, y + studio::NOTE_H / 2.0), &Phase::Scanning.text(), grid.w, pal);
        y += studio::NOTE_H;
    } else {
        studio::text(&card, pos2(grid.x, y + HEAD_H / 2.0), &t!("dev.none_title"), strong(16.0), pal.text, grid.w, Align2::LEFT_CENTER);
        y += HEAD_H + s.gap;
        let words = t!("dev.none_body");
        let drawn = Para::new(&words, body(FIRST.0), pal.text).spaced(FIRST.1).floor(floor).paint(&card, pos2(grid.x, y), grid.w);
        y += drawn.height() + METER_GAP;
        studio::searching_at(&card, pos2(grid.x, y + studio::NOTE_H / 2.0), &t!("dev.none_watching"), grid.w, pal);
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
    // The hints count at rest while they are open, as Details does on a
    // board's card: the measuring run sets them too.
    if page.details {
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
            let para = Para::new(hint, body(DIM.0), pal.dim).spaced(DIM.1).lead(g.bullet).floor(floor);
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
        y += Para::runs(&seen).spaced(DIM.1).floor(floor).paint(&card, pos2(grid.x, y), grid.w).height();
    }
    let rest = y + s.pad_y - room.min.y;
    let bottom = (y + s.pad_y).min(room.max.y);
    studio::hairline(&card, Rect::from_min_max(room.min, pos2(room.max.x, bottom)), face.pal);
    Drawn { acts, caret: None, rest }
}

/// A child of `ui` over `room`, clipped to it: nothing a card draws spills
/// onto the busy line.
pub(super) fn clipped(ui: &mut Ui, room: Rect, salt: &str) -> Ui {
    let mut child = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(room));
    child.set_clip_rect(room.expand(1.0).intersect(ui.clip_rect()));
    child
}

// ── The cells' rows in points ─────────────────────────────────────────────

/// A cells' line as the canvas sets it, the board card's and a card's view's
/// alike (card.rs reads its rows through it): the glyph the cells lead it
/// with (a verdict's ✓ ! ✗ ?) as the kit's LED in its colour, their scan
/// widget at its head (its lit cells and the share after them) as the kit's
/// meter, and the rest, each piece in its colour, the bold ones semibold.
pub(super) struct Lined {
    pub(super) lamp: Option<Color32>,
    pub(super) meter: Option<(usize, Option<String>)>,
    pub(super) spans: Vec<(String, Color32, bool)>,
}

impl Lined {
    pub(super) fn of(line: &Line, face: &Face) -> Lined {
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
            let share = head.split_whitespace().next().filter(|w| w.len() > 1 && w.ends_with('%') && w[..w.len() - 1].chars().all(|c| c.is_ascii_digit()));
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
    pub(super) fn dim(&self, face: &Face) -> bool {
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

/// The cells' door mark off a link's words, as the kit's lead: their ▸ a
/// door shut, their ▾ a door open.
pub(super) fn lead_of(text: &str) -> (Option<Lead>, &str) {
    let g = glyphs();
    let after = |glyph: &str| text.strip_prefix(glyph).and_then(|r| r.strip_prefix(' '));
    match (after(g.forward), after(g.open)) {
        (Some(rest), _) => (Some(Lead::Shut), rest),
        (_, Some(rest)) => (Some(Lead::Open), rest),
        _ => (None, text),
    }
}

// ── What a card keeps in its room ─────────────────────────────────────────

/// The log's lines a card counts at rest while Details is open, as many as
/// it has up to these: a write's last rung, from its 0% to its failure's
/// hint (the Failed board's log ends so), so a window short of the boards'
/// measures closes up to show them and the boards' own shows them at its
/// measures. Past them the log takes only the room left.
pub(super) const LOG_REST: usize = 7;
/// The log's newest lines Details keeps, short of room, before its facts
/// give way.
const LOG_LEAST: usize = 2;

/// What a card asks of its room, for [`keep`]: the room's height (none in
/// the measuring run, which keeps every row), what it takes whatever it
/// keeps — its padding, its head, its bottom row and the gap over it — its
/// value rows by their number, the other ones' turns to give way (a turn's
/// rows at a time) and the ones that say what the primary will do
/// ([`says`]), top to foot; and while Details is open, its facts, the log's
/// lines it counts, and what its rows after the log (its links) take.
pub(super) struct Want {
    pub(super) room: f32,
    pub(super) fixed: f32,
    values: usize,
    give_way: Vec<Vec<usize>>,
    says: Vec<usize>,
    open: bool,
    facts: usize,
    log: usize,
    links: f32,
}

impl Want {
    /// What a card in `room` asks with Details' `details` rows (the cells'
    /// own, none while it is shut) and `port`'s log: in the live run the log
    /// counts every line it has, in the measuring run [`LOG_REST`] at most.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn of(page: &Page, port: &str, face: &Face, room: Rect, fixed: f32, values: usize, give_way: Vec<Vec<usize>>, says: Vec<usize>, details: &[Row]) -> Want {
        let at = details.iter().position(|r| matches!(r, Row::Log));
        let lines = at.map_or(0, |_| page.log_for(port).len());
        let links = at.map_or(0.0, |at| details[at + 1..].iter().map(|r| fixed_h(r, face)).sum());
        Want {
            room: if face.live { room.height() } else { f32::INFINITY },
            fixed,
            values,
            give_way,
            says,
            open: !details.is_empty(),
            facts: details.iter().filter(|r| matches!(r, Row::Fact { .. })).count(),
            log: if face.live { lines } else { lines.min(LOG_REST) },
            links,
        }
    }
}

/// What a card keeps of its rows ([`keep`]): each value row, Details' rule,
/// its first facts, its log's newest lines and its links; and how tall
/// what it keeps stands under its fixed part.
pub(super) struct Kept {
    pub(super) values: Vec<bool>,
    rule: bool,
    facts: usize,
    log: usize,
    links: bool,
    pub(super) height: f32,
}

impl Kept {
    /// Details' `rows` it keeps, in their order: the rule, the first facts,
    /// the log in its lines ([`Paint::row`] gives it them), the links with
    /// the gap over them.
    pub(super) fn details<'r>(&self, rows: &'r [Row]) -> Vec<&'r Row> {
        let mut facts = 0;
        let mut after = false;
        let mut kept = Vec::new();
        for row in rows {
            let keep = match row {
                Row::Rule => self.rule,
                Row::Fact { .. } => {
                    facts += 1;
                    facts <= self.facts
                }
                Row::Log => {
                    after = true;
                    self.log > 0
                }
                _ if after => self.links,
                _ => true,
            };
            if keep {
                kept.push(row);
            }
        }
        kept
    }

    /// The log's lines it keeps.
    pub(super) fn log(&self) -> usize {
        self.log
    }
}

/// What a card keeps where its room is short of every row (contract
/// clause 70), in the order it keeps them: its bottom row with its primary
/// (the fixed part, always); the value rows that say what the primary will
/// do; while Details is open its rule and its links — a control never
/// vanishes while the room holds it — then its log's newest two lines,
/// then its facts, cut from the foot; then the other value rows, given way
/// in their turns. What room is left the log takes, its newest lines as
/// many as fit and it has. Short of room for even the bottom row and the
/// rows that say what it will do, Details goes whole and those rows are cut
/// from the foot. `values_h` measures the value rows a mask keeps.
pub(super) fn keep(want: &Want, face: &Face, mut values_h: impl FnMut(&[bool]) -> f32) -> Kept {
    let gap = face.space.gap;
    let log_h = |n: usize| if n > 0 { gap + n as f32 * LOG_ROW } else { 0.0 };
    let details = |k: &Kept| {
        let rule = if k.rule { 2.0 * gap + 1.0 } else { 0.0 };
        rule + k.facts as f32 * FACT_H + log_h(k.log) + if k.links { want.links } else { 0.0 }
    };
    let open = want.open;
    let mut kept = Kept {
        values: vec![true; want.values],
        rule: open,
        facts: if open { want.facts } else { 0 },
        log: if open { want.log.min(LOG_LEAST) } else { 0 },
        links: open,
        height: 0.0,
    };
    let mut v = values_h(&kept.values);
    let over = |k: &Kept, v: f32| want.fixed + v + details(k) > want.room + 0.01;
    for turn in &want.give_way {
        if !over(&kept, v) {
            break;
        }
        for &i in turn {
            kept.values[i] = false;
        }
        v = values_h(&kept.values);
    }
    while over(&kept, v) && kept.facts > 0 {
        kept.facts -= 1;
    }
    while over(&kept, v) && kept.log > 0 {
        kept.log -= 1;
    }
    if over(&kept, v) {
        kept.links = false;
        kept.rule = false;
    }
    for &i in want.says.iter().rev() {
        if !over(&kept, v) {
            break;
        }
        kept.values[i] = false;
        v = values_h(&kept.values);
    }
    // The room left is the log's.
    while kept.rule && kept.log < want.log {
        kept.log += 1;
        if over(&kept, v) {
            kept.log -= 1;
            break;
        }
    }
    kept.height = v + details(&kept);
    kept
}

/// Whether a value block (a labelled row and the lines under it) says what
/// the card's primary will do or what is under way, so it is kept before
/// the other value rows (contract clause 70): it holds a control (a link,
/// the next write with its Reset), its first line is led by a gold or a
/// danger LED (a failure's or a warning's words, what the primary mends),
/// or it is work under way (its first line a scan, or its words in the
/// accent: a write, a format, a sync).
pub(super) fn says(rows: &[&Row], face: &Face) -> bool {
    if rows.iter().any(|r| matches!(r, Row::Link { .. } | Row::Next { .. })) {
        return true;
    }
    let first = match rows.first() {
        Some(Row::Value { left, .. } | Row::Wide { left, .. }) => Lined::of(left, face),
        _ => return false,
    };
    let pal = face.pal;
    let warns = first.lamp.is_some_and(|c| c == pal.gold || c == pal.danger);
    let busy = first.meter.is_some() || (!first.spans.is_empty() && first.spans.iter().all(|(_, c, _)| *c == pal.accent));
    warns || busy
}

/// The height a row after a log takes as [`Paint`] sets it, for the log
/// to leave it.
fn fixed_h(row: &Row, face: &Face) -> f32 {
    match row {
        Row::Blank => face.space.gap,
        Row::Link { .. } => TEXT_BUTTON_H,
        Row::Fact { .. } => FACT_H,
        Row::Actions { primary: Some(_), .. } => TALL_H,
        Row::Rule => 2.0 * face.space.gap + 1.0,
        Row::Log => 0.0,
        _ => PLAIN.1,
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
    /// The log's lines, as many as [`keep`] gives it.
    pub(super) log: usize,
    last: Option<Kind>,
    /// The card's own: its kind and its subject, a board's port or a
    /// card's key.
    salt: Id,
    /// The controls' ids drawn so far.
    ids: Vec<Id>,
    acts: &'a mut Vec<Act>,
}

impl<'u, 'f, 'a> Paint<'u, 'f, 'a> {
    pub(super) fn new(ui: &'u Ui, face: &'f Face<'f>, grid: Grid, y: f32, floor: f32, salt: Id, acts: &'a mut Vec<Act>) -> Self {
        Paint { ui, face, grid, y, floor, cut: false, log: 0, last: None, salt, ids: Vec::new(), acts }
    }

    /// The id of the control whose words are `words` and whose act is
    /// `act`: what it is and whose, never where it falls among the card's
    /// controls. egui gives a release to the id that was under its press; a
    /// press whose control went, changed its words or its act, or belongs to
    /// another subject by its release is so dropped, as egui drops a click
    /// whose control is no longer drawn, and never given to the control that
    /// took its place in the row (the cells act on the press itself, on what
    /// they drew under it). The same words and act twice on one card are
    /// told apart by their turn.
    fn id(&mut self, words: &str, act: &Act) -> Id {
        let id = self.salt.with(words).with(format!("{act:?}"));
        let turn = self.ids.iter().filter(|seen| **seen == id).count();
        self.ids.push(id);
        if turn == 0 { id } else { id.with(turn) }
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

    /// One row; the log, `port`'s, in its [`Paint::log`] lines.
    pub(super) fn row(&mut self, page: &mut Page, port: &str, row: &Row) {
        match row {
            Row::Log => self.log(page, port),
            row => self.value(row),
        }
    }

    /// One row of those that read nothing of the page but what they hold:
    /// every row but the log.
    pub(super) fn value(&mut self, row: &Row) {
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
            Row::Log => {}
        }
    }

    /// A block's label in the label column, on the line whose middle is
    /// `mid`.
    fn label(&self, label: Option<&str>, mid: f32) {
        if let Some(label) = label {
            self.grid.label(self.ui, label, mid, self.face.pal);
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
            // Its share on its words' baseline, as the cells' one line sets
            // the two: the words are a paragraph from the row's top, so a
            // share centred on the row's middle would stand under them.
            let on = studio::para_mid(self.ui, top, size);
            x = studio::scan_lead(self.ui, pos2(x, mid), *lit, share.as_deref(), on, end, face.pal);
        }
        let runs = lined.runs(size, face);
        let height = if runs.is_empty() {
            line_h
        } else {
            let para = Para::runs(&runs).spaced(line_h).floor(self.floor);
            para.paint(self.ui, pos2(x, top), (end - x).max(1.0)).height().max(line_h)
        };
        if let Some(right) = right {
            let left_w = x - from + studio::runs_width(self.ui, &runs);
            let mid = studio::para_mid(self.ui, top, size);
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
        self.grid.bar(self.ui, top, permille, lit, self.face.pal);
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
            let id = self.id(words, act);
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
            let para = Para::runs(&runs).spaced(line_h).floor(self.floor);
            para.paint(self.ui, pos2(x, top), room.max(1.0)).height().max(line_h)
        };
        let beside = runs.is_empty() || x + studio::runs_width(self.ui, &runs) + CLEAR + reset_w <= end;
        self.end(top + height);
        self.last = Some(Kind::Line);
        if !reset {
            return;
        }
        let mid = if beside {
            studio::para_mid(self.ui, top, size)
        } else {
            let Some(under) = self.room(0.0, TEXT_BUTTON_H) else { return };
            self.y = under + TEXT_BUTTON_H;
            under + TEXT_BUTTON_H / 2.0
        };
        if face.live {
            let id = self.id(&words, &Act::Reset);
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
                let id = self.id(label, &Act::Primary);
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
                let id = self.id(words, act);
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
        studio::rule(self.ui, self.grid.x, self.grid.right(), top, self.face.pal);
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

    /// The log under a block's gap, in the [`Paint::log`] rows [`keep`]
    /// gives it, and no more than it has or the floor leaves: its newest
    /// lines, `mm:ss` dim and then the line in its kind's colour, all in the
    /// monospace, each cut with an ellipsis at the content's end, or short of
    /// the thumb's room while there are more lines than rows (the Done
    /// board's log; the Failed board's, which all fit, has none) — scrolled
    /// up by the page's own count (the wheel's over it, a line a notch),
    /// else following the newest line on the pane's foot. No room for a
    /// line: no log, and no gap for it.
    fn log(&mut self, page: &mut Page, port: &str) {
        let face = self.face;
        let gap = face.space.gap;
        let lines: Vec<LogLine> = page.log_for(port).into_iter().cloned().collect();
        let top = self.y + gap;
        let room = self.floor - top;
        let rows = (((room + 0.01) / LOG_ROW).floor().max(0.0) as usize).min(lines.len()).min(self.log);
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
            studio::list_thumb(self.ui, pane, rows, lines.len(), end - rows, face.pal);
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

    /// The room of the card `words` stand in: what clipped them, less the
    /// point the card's clip stands outside its room (`clipped`).
    pub(in super::super) fn card_room_of(out: &FullOutput, words: &str) -> Rect {
        let clip = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == words => Some(c.clip_rect),
            _ => None,
        });
        clip.unwrap_or_else(|| panic!("{words:?} drawn")).shrink(1.0)
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

    /// Where `words`' first letter stands on its baseline.
    fn baseline(out: &FullOutput, words: &str) -> f32 {
        let (pos, galley) = galleys(out).into_iter().find(|(_, g)| g.text() == words).unwrap_or_else(|| panic!("{words:?} drawn"));
        let row = &galley.rows[0];
        pos.y + row.pos.y + row.row.glyphs[0].pos.y
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
        probe.click_id(&mut page, super::super::tab_id("COM3"));
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
        // The frame closed up, so the log has rows: its newest line whole
        // in the card's room, over the foot's padding (the tighter set's at
        // the least), the primary whole too.
        let line = find(&out, fail);
        let room = card_room_of(&out, fail);
        assert!(line.max.y <= room.max.y - super::super::TIGHT.pad_y + 0.5, "the failure whole on the card: {line:?} in {room:?}");
        assert!(drew(&out, "wrote chunk 39"), "and the line before it");
        let primary = find(&out, "TRY AGAIN");
        assert!(primary.max.y < line.min.y, "under Try again");
        // Details closed, the card is the card at rest, as tall as ever.
        page.act(Act::Details);
        let out = probe.settle(&mut page);
        assert!(!drew(&out, fail) && drew(&out, "TRY AGAIN"));
    }

    #[test]
    fn at_the_guis_floor_details_keeps_its_links_and_its_newest_line_the_value_lines_giving_way_first() {
        let _en = english();
        // Two boards and the one in view just written: the subjects row,
        // the identity line and the Done card's lines leave Details no room
        // of its own here.
        let (mut page, ends) = locked(|e| e.boards(vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0")]));
        ends.writing(62);
        ends.tell([log("writing… 100%", LogKind::Phase), log("mstream-mp3-player v0.8.0", LogKind::Quiet)]);
        ends.tell([Told::Board(written_answering())]);
        page.pump();
        page.act(Act::Details);
        // At the floor and in shorter areas still, where the card at rest
        // fills its room even closed up: Write again and the format, whole
        // under its rule, and the log's newest line, the value lines giving
        // way first (nothing here is the primary's: there is none), never
        // the head or the bottom row.
        let chip = "Up to date · v0.8.0 · just written";
        for h in [FLOOR.y, 320.0, 300.0] {
            let probe = Headless::new(vec2(FLOOR.x, h));
            let out = probe.settle(&mut page);
            assert_drew(&out, &["Details", "M5Stack Core2", "Write v0.8.0 again", "Format as FAT32…"]);
            let (format, room) = (find(&out, "Format as FAT32…"), card_room_of(&out, "Format as FAT32…"));
            assert!(format.max.y <= room.max.y - super::super::TIGHT.pad_y + 0.5, "{h}: the format's link whole on the card: {format:?} in {room:?}");
            assert!(!drew(&out, chip), "{h}: the value lines gave way first");
            // Where a line of the log has room over them, its newest.
            if h >= 320.0 {
                assert!(find(&out, "mstream-mp3-player v0.8.0").max.y < find(&out, "Write v0.8.0 again").min.y, "{h}: the log's newest line over the links");
            }
        }
        // Closed, the card is the card at rest again.
        page.act(Act::Details);
        let out = Headless::new(FLOOR).settle(&mut page);
        assert_drew(&out, &[chip, "Details"]);
    }

    #[test]
    fn short_of_room_a_card_keeps_its_rows_in_the_contracts_order_and_its_log_takes_what_is_left() {
        let pal = Pal::now();
        let full = Rect::from_min_size(Pos2::ZERO, FLOOR);
        let face = Face { pal: &pal, composition: "", key_hints: false, full, space: super::super::TIGHT, ports: &[], live: true, inert: false };
        // Five value rows 30 points each; the second says what the primary
        // will do, the others go from the foot. Details: four facts, a long
        // log, its rule (21 points in the tighter set) and its links (66).
        let values_h = |mask: &[bool]| mask.iter().filter(|k| **k).count() as f32 * 30.0;
        let want = |room: f32| Want { room, fixed: 100.0, values: 5, give_way: vec![vec![4], vec![3], vec![2], vec![0]], says: vec![1], open: true, facts: 4, log: 40, links: 66.0 };
        let kept = |room: f32| {
            let k = keep(&want(room), &face, values_h);
            (k.values.iter().filter(|v| **v).count(), k.values[1], k.facts, k.log, k.links && k.rule)
        };
        // Every row, its log 2 lines (10 over them, 18 each) and the room
        // left: the log's.
        let all = 100.0 + 150.0 + 21.0 + 80.0 + 66.0;
        assert_eq!(kept(all + 46.0), (5, true, 4, 2, true));
        assert_eq!(kept(all + 46.0 + 3.0 * 18.0), (5, true, 4, 5, true), "the room left is the log's");
        // Then the other value rows from the foot, then the facts from the
        // foot, then the log's newest lines, then Details whole; the row
        // that says what the primary will do last of all.
        assert_eq!(kept(all + 46.0 - 30.0), (4, true, 4, 2, true));
        let least = 100.0 + 30.0 + 21.0 + 66.0;
        assert_eq!(kept(least + 46.0 + 80.0), (1, true, 4, 2, true));
        assert_eq!(kept(least + 46.0 + 60.0), (1, true, 3, 2, true));
        assert_eq!(kept(least + 46.0), (1, true, 0, 2, true));
        assert_eq!(kept(least + 28.0), (1, true, 0, 1, true));
        assert_eq!(kept(least), (1, true, 0, 0, true), "the rule and the links over any line of the log");
        assert_eq!(kept(least - 1.0), (1, true, 0, 0, false));
        assert_eq!(kept(120.0), (0, false, 0, 0, false));
        // The measuring run keeps every row, its log at the lines it counts.
        let k = keep(&Want { room: f32::INFINITY, log: LOG_REST, ..want(0.0) }, &face, values_h);
        assert_eq!((k.facts, k.log), (4, LOG_REST));
        assert!((100.0 + k.height - (all + 10.0 + LOG_REST as f32 * 18.0)).abs() < 0.01, "its rest: {}", k.height);
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
        let (share, words) = (baseline(&out, "41%"), baseline(&out, &running));
        assert!((share - words).abs() < 0.5, "the share on its words' baseline, as the cells' one line: {share} and {words}");
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
