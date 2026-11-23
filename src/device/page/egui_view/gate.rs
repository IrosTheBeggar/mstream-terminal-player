//! The board's gate drawn by egui (contract clauses 4, 23, 39, 42 and 59),
//! after the canvas's UpdateGate and Format boards: the kit's gold gate over
//! the page's area and its scrim, the page inert beneath, and no close — a
//! gate is answered, never dismissed.
//!
//! Its words are the cells' with the same keys and arguments: the title
//! (cells.rs's `gate_title`), the answers (`gate_buttons`), and the body
//! [`body_of`] makes as their `gate_lines` makes it, in its order and its
//! tones, a sentence a paragraph where the cells give it a line a row —
//! what a chosen write puts on and the check it passed, what goes on and
//! how long, what stays in the green, what goes and what may go wrong in the
//! gold, Update all's boards, the format's four sentences, and the
//! install's erase box with its cost under it. The cells' blank lines are
//! the paragraphs' gaps.
//!
//! The answers are the cells' two at the window's foot, the kit's gate's
//! ([`studio::answers_at`]): the safe one first and lit, named for what
//! stays and led by the cells' ◂ as the kit's pointer turned back, its
//! figure in the monospace in its own case; then the write as the dim
//! secondary. Keys never reach egui, so Enter, Esc and `n` keep and `y`
//! writes through the page's own keymap as in the cells, and nothing here
//! can make the write the default. A click asks for the cells' act:
//! [`Act::Keep`], [`Act::Write`], or [`Act::Erase`] on the install's box
//! (the kit's check line).
//!
//! The gate never scrolls and its answers stand whole at every size the
//! page is drawn at (clause 4): short of room its paragraphs' gaps give way
//! first, from the end, as the cells drop their blank lines; then its frame
//! tightens; then its last lines are left out whole. So it lays its window
//! out here, in the kit's gate look ([`studio::modal_window`] in the gold,
//! [`studio::modal_title`]), rather than in the kit's Dialog, whose body
//! scrolls.

use egui::{Color32, Id, Order, Pos2, Rect, Ui, UiBuilder, pos2, vec2};

use super::super::cells;
use super::super::*;
use super::{Drawn, Face, unled};
use crate::device::board::gb;
use crate::gui::window::studio::{self, Answers, HIT, Pal, Para, Title, body, mono};

/// The sentences' words, and their figures a point under them (the
/// Format board's 13 under its 14).
const WORDS: f32 = 14.0;
const FIGURES: f32 = 13.0;
/// A sentence's line: the kit's 1.3 leading on the words' size.
const LINE: f32 = WORDS * 1.3;
/// The erase box's cost under it: dim words, hung under the box's.
const COST: (f32, f32) = (13.0, 18.0);
/// Update all's boards and its left-out ones share six lines, the cells'
/// six: the most the GUI's floor holds.
const GATE_ROWS: usize = 6;

/// The window's measures: the room it leaves the area round it (both
/// sides', both ends'), its padding, the room under its title, between two
/// paragraphs, and over its answers.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Measures {
    clear: f32,
    pad: f32,
    under_title: f32,
    apart: f32,
    over_foot: f32,
}

/// The kit's gate's measures: 16 points clear of the area, 24 in, 20
/// under the title and over the answers, sentences 6 apart.
const ROOMY: Measures = Measures { clear: 32.0, pad: 24.0, under_title: 20.0, apart: studio::GATE_APART, over_foot: studio::GATE_OVER_FOOT };
/// Where the page's area is too short for those even with no gaps between
/// the paragraphs (Update all's six boards at the GUI's floor).
const TIGHT: Measures = Measures { clear: 16.0, pad: 16.0, under_title: 10.0, apart: 0.0, over_foot: 12.0 };

/// One piece of the gate's body, as the cells' `gate_lines` gives it.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    /// A sentence in its colour; `line` one line cut with an ellipsis, as
    /// the cells fit Update all's boards to their width.
    Words { text: String, tone: Color32, line: bool },
    /// The install's erase box, and its cost under it.
    Erase { on: bool, cost: String },
}

/// The body's pieces in order, each with whether a gap comes before it —
/// the cells' blank line.
struct Body {
    pieces: Vec<(bool, Piece)>,
    gap: bool,
}

impl Body {
    fn put(&mut self, piece: Piece) {
        let gap = self.gap && !self.pieces.is_empty();
        self.pieces.push((gap, piece));
        self.gap = false;
    }

    fn say(&mut self, text: String, tone: Color32) {
        self.put(Piece::Words { text, tone, line: false });
    }

    fn line(&mut self, text: String, tone: Color32) {
        self.put(Piece::Words { text, tone, line: true });
    }

    fn blank(&mut self) {
        self.gap = true;
    }
}

/// The gate's body under its title, as the cells' `gate_lines` says it:
/// what a chosen write puts on, then the kind's own words — the format's,
/// Update all's, the install's with its erase box, or every other write's
/// — each line's tone as the cells colour it in `pal`.
fn body_of(gate: &Gate, pal: &Pal) -> Vec<(bool, Piece)> {
    let g = glyphs();
    let mut b = Body { pieces: Vec::new(), gap: false };
    let tone = |how: Tone| if how == Tone::Gold { pal.gold } else { pal.text };
    // Update all's rows name the version as its title does.
    let to = match gate.title_mode {
        Some(mode) => format!("{} · {}", gate.to, mode.word()),
        None => gate.to.clone(),
    };
    for (words, how) in &gate.lines {
        b.say(words.clone(), tone(*how));
    }
    if !gate.lines.is_empty() {
        b.blank();
    }
    let old = gate.name.clone().unwrap_or_else(|| t!("dev.gate_old_firmware").to_string());
    match gate.kind {
        Kind::Format => {
            let ask = gate.format.clone().unwrap_or(FormatAsk { size: None, lost: String::new() });
            let size = ask.size.map_or_else(|| "?".to_string(), gb);
            let took = super::super::card::span(format_secs(ask.size.unwrap_or(0)));
            b.say(t!("dev.card.gate_fmt_body", size = size, t = took).to_string(), pal.text);
            b.blank();
            b.say(t!("dev.card.gate_fmt_confirm").to_string(), pal.text);
            b.blank();
            b.say(t!("dev.card.gate_fmt_lost", what = ask.lost).to_string(), pal.gold);
            b.blank();
            b.say(t!("dev.card.gate_fmt_txt").to_string(), pal.text);
        }
        Kind::All => {
            b.say(t!("dev.gate_all_time").to_string(), pal.text);
            b.blank();
            // Six lines at most, the boards left out among them.
            let left = gate.left_out.len().min(GATE_ROWS.saturating_sub(gate.rows.len().min(GATE_ROWS)));
            let shown = GATE_ROWS - left;
            for row in gate.rows.iter().take(shown) {
                let mut words = short(&row.port);
                if let Some(serial) = &row.serial {
                    words.push_str(&format!(" · {}", t!("dev.serial", serial = serial)));
                }
                let to = match row.dio {
                    true => t!("dev.next_in", what = to, mode = Mode::Dio.word()).to_string(),
                    false => to.clone(),
                };
                words.push_str(&format!(" · {} {} {to}", row.from, g.arrow));
                b.line(words, pal.text);
            }
            if gate.rows.len() > shown {
                b.line(t!("dev.gate_all_more", n = gate.rows.len() - shown).to_string(), pal.dim);
            }
            for (port, what) in gate.left_out.iter().take(left) {
                b.line(t!("dev.gate_all_left_out", port = short(port), what = what).to_string(), pal.dim);
            }
            b.blank();
            b.say(t!("dev.gate_all_stays").to_string(), pal.ok);
            b.blank();
            b.say(t!("dev.gate_all_plugged").to_string(), pal.text);
        }
        Kind::Install => {
            let time = match &gate.name {
                Some(name) => t!("dev.gate_install_time", name = name),
                None => t!("dev.gate_blank_time"),
            };
            b.say(time.to_string(), pal.text);
            b.blank();
            b.say(t!("dev.gate_unknown").to_string(), pal.gold);
            b.blank();
            let cost = if gate.erase { t!("dev.gate_erase_on", name = old) } else { t!("dev.gate_erase_off", name = old) };
            b.put(Piece::Erase { on: gate.erase, cost: cost.to_string() });
            b.blank();
            b.say(t!("dev.gate_card_untouched").to_string(), pal.ok);
            b.blank();
            b.say(t!("dev.gate_plugged_install").to_string(), pal.text);
        }
        _ => {
            b.say(t!("dev.gate_time").to_string(), pal.text);
            b.blank();
            if gate.erase_any {
                // The sheet's Erase: what goes in gold, what stays in green.
                b.say(t!("dev.gate_erase_any").to_string(), pal.gold);
                b.say(t!("dev.gate_card_stays").to_string(), pal.ok);
            } else {
                b.say(t!("dev.gate_stays").to_string(), pal.ok);
            }
            for (words, how) in &gate.after {
                b.say(words.clone(), tone(*how));
            }
            b.blank();
            let plugged = if gate.kind == Kind::Local { "dev.gate_plugged_local" } else { "dev.gate_plugged" };
            b.say(t!(plugged).to_string(), pal.text);
        }
    }
    b.pieces
}

/// The board's gate over `face.full`, the page's area: its title, its body
/// and its two answers in the kit's gold window, laid out for the room the
/// area has. A click asks for the cells' act: the safe answer
/// [`Act::Keep`], the write [`Act::Write`], the install's box
/// [`Act::Erase`].
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(gate) = page.gate.clone().filter(|_| face.live) else { return drawn };
    let pal = face.pal;
    let title = cells::gate_title(&gate);
    let (keep, write) = cells::gate_buttons(&gate);
    let (words, lead) = unled(&keep);
    let answers = Answers { keep: words, lead, go: Some(&write), also: face.ports };
    let pieces = body_of(&gate, pal);
    let laid = lay_out(ui, face, &title, &pieces, &answers);
    let id = Id::new("mp3-gate");
    let over = face.full;
    let ctx = ui.ctx().clone();
    let area = egui::Area::new(id).order(Order::Foreground).fixed_pos(over.min).constrain(false).fade_in(false);
    let shown = area.show(&ctx, |ui| {
        // The scrim takes every press outside the window: the page under
        // it is inert.
        studio::modal_window(ui, over, laid.window, pal.gold, pal);
        let inner = laid.window.shrink(laid.m.pad);
        laid.title.paint(ui, inner.min);
        let mut act = None;
        for ((_, piece), top) in pieces.iter().zip(&laid.tops) {
            let Some(top) = *top else { continue };
            let at = pos2(inner.min.x, top);
            match piece {
                Piece::Words { .. } => {
                    words_para(piece, face).floor(laid.floor).paint(ui, at, inner.width());
                }
                Piece::Erase { on, cost } => {
                    let row = Rect::from_min_size(at, vec2(inner.width(), HIT));
                    let words = t!("dev.gate_erase").to_string();
                    if studio::check_line_at(ui, id.with("erase"), row, *on, &words, "", false, pal).clicked() {
                        act = Some(Act::Erase);
                    }
                    let hung = pos2(at.x + ERASE_HANG, at.y + HIT);
                    cost_para(cost, face).floor(laid.floor).paint(ui, hung, (inner.width() - ERASE_HANG).max(1.0));
                }
            }
        }
        match studio::answers_at(ui, id.with("foot"), pos2(inner.min.x, laid.foot_top), &answers, inner.width(), pal) {
            Some(true) => Some(Act::Write),
            Some(false) => Some(Act::Keep),
            None => act,
        }
    });
    drawn.acts.extend(shown.inner);
    drawn
}

/// Where the erase box's cost hangs under it: under the box's words, the
/// box's 16 points and the 10 after it.
const ERASE_HANG: f32 = studio::BOX + 10.0;

/// A sentence of the body as the kit's prose: the boards' 14 points in its
/// tone at the kit's leading, its figures (a version, a size, a port) in
/// the monospace a point under them, wrapped by words — or, a `line`, one
/// line cut with an ellipsis. A sentence that starts with a figure (Update
/// all's port) keeps the words' leading.
fn words_para(piece: &Piece, face: &Face) -> Para {
    let Piece::Words { text, tone, line } = piece else { return Para::new("", body(WORDS), face.pal.text) };
    let runs = studio::figured(text, face.ports, body(WORDS), mono(FIGURES), *tone);
    let para = Para::runs(&runs).spaced(LINE);
    if *line { para.rows(1) } else { para }
}

/// The erase box's cost: dim, 13 points on 18, its figures (a version in
/// the old firmware's name) in the monospace a point under its words, as
/// every sentence of the gate sets them.
fn cost_para(cost: &str, face: &Face) -> Para {
    let runs = studio::figured(cost, face.ports, body(COST.0), mono(COST.0 - 1.0), face.pal.dim);
    Para::runs(&runs).spaced(COST.1)
}

/// The gate laid out for its room: its measures, its window, its title set,
/// each piece's top (none for one left out), the floor no line of the body
/// passes, and where its answers' foot starts.
struct Laid {
    m: Measures,
    window: Rect,
    title: Title,
    tops: Vec<Option<f32>>,
    floor: f32,
    foot_top: f32,
}

/// The gate measured with one set of [`Measures`] in a window `width`
/// wide: its title set, each piece's height, and its answers' foot's
/// height.
struct Fit {
    m: Measures,
    title: Title,
    heights: Vec<f32>,
    foot_h: f32,
}

impl Fit {
    fn of(ui: &mut Ui, face: &Face, m: Measures, width: f32, gate: (&str, &[(bool, Piece)], &Answers)) -> Fit {
        let (title, pieces, answers) = gate;
        let inner_w = (width - 2.0 * m.pad).max(1.0);
        let title = studio::modal_title(ui, title, inner_w, face.pal.gold);
        let heights = measure(ui, face, pieces, inner_w);
        let foot_h = studio::answers_rects(ui, answers, inner_w).1;
        Fit { m, title, heights, foot_h }
    }

    /// How tall the window stands with the first `kept` of the body's gaps.
    fn height(&self, pieces: &[(bool, Piece)], kept: usize) -> f32 {
        let gaps = pieces.iter().filter(|(gap, _)| *gap).count().min(kept);
        let body_h = self.heights.iter().sum::<f32>() + gaps as f32 * self.m.apart;
        2.0 * self.m.pad + self.title.h + self.m.under_title + body_h + self.m.over_foot + self.foot_h
    }
}

/// How the gate stands in `face.full`: centred, the kit's gate's width (16
/// points clear of the area's sides where it is narrower), as tall as its
/// title, its body and its answers with the kit's measures and every gap —
/// or, short of room, with its paragraphs' gaps given way from the end,
/// then with the tighter measures, no gaps and half the clearance, and its
/// last lines left out whole over its answers, which stand whole.
fn lay_out(ui: &mut Ui, face: &Face, title: &str, pieces: &[(bool, Piece)], answers: &Answers) -> Laid {
    let over = face.full;
    // The window's width and its most height with `m`'s clearance.
    let room = |m: Measures| (studio::GATE_W.min(over.width() - m.clear).max(over.width().min(160.0)), (over.height() - m.clear).max(0.0));
    let gaps = pieces.iter().filter(|(gap, _)| *gap).count();
    let (width, most) = room(ROOMY);
    let roomy = Fit::of(ui, face, ROOMY, width, (title, pieces, answers));
    let (fit, kept, width, most) = match (0..=gaps).rev().find(|kept| roomy.height(pieces, *kept) <= most + 0.5) {
        Some(kept) => (roomy, kept, width, most),
        None => {
            let (width, most) = room(TIGHT);
            (Fit::of(ui, face, TIGHT, width, (title, pieces, answers)), 0, width, most)
        }
    };
    let m = fit.m;
    // The search above finds room up to half a point over the area's, and
    // the window takes that half point: held to `most`, its floor would
    // stand over the body's last line by as much, and the line would be
    // left out.
    let want = fit.height(pieces, kept);
    let h = if want <= most + 0.5 { want } else { most.max(2.0 * m.pad + fit.title.h + fit.foot_h) };
    let window = Rect::from_center_size(over.center(), vec2(width, h));
    let inner = window.shrink(m.pad);
    let foot_top = inner.max.y - fit.foot_h;
    let floor = foot_top - m.over_foot;
    let mut y = inner.min.y + fit.title.h + m.under_title;
    let mut seen = 0;
    let mut tops: Vec<Option<f32>> = Vec::new();
    for ((gap, piece), h) in pieces.iter().zip(&fit.heights) {
        if *gap {
            seen += 1;
            if seen <= kept {
                y += m.apart;
            }
        }
        // A piece whose first line would pass the floor is left out, and
        // every piece after it.
        let first = match piece {
            Piece::Erase { .. } => HIT,
            Piece::Words { .. } => LINE,
        };
        let fits = tops.last().is_none_or(Option::is_some) && y + first.min(*h) <= floor + 0.01;
        tops.push(fits.then_some(y));
        y += h;
    }
    Laid { m, window, title: fit.title, tops, floor, foot_top }
}

/// Each piece's height at `w`, measured by drawing it where nothing shows.
fn measure(ui: &mut Ui, face: &Face, pieces: &[(bool, Piece)], w: f32) -> Vec<f32> {
    let room = Rect::from_min_size(Pos2::ZERO, vec2(w, 100_000.0));
    let mut probe = ui.new_child(UiBuilder::new().id_salt("mp3-gate-measure").max_rect(room));
    probe.set_invisible();
    pieces
        .iter()
        .map(|(_, piece)| match piece {
            Piece::Words { .. } => words_para(piece, face).paint(&probe, Pos2::ZERO, w).height(),
            Piece::Erase { cost, .. } => HIT + cost_para(cost, face).paint(&probe, Pos2::ZERO, (w - ERASE_HANG).max(1.0)).height(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! The board's gate run headless (board.rs's tests' helpers): every kind
    //! says the cells' words in their order and their tones, its answers
    //! and its box ask for the cells' acts, the page and the scrim under it
    //! take nothing, and at the GUI's floor every gate stands whole.

    use egui::epaint::Shape;
    use egui::{Event, FullOutput};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::board::tests::{BOARD, FLOOR, Headless, WINDOW, assert_drew, color_of, drew, english, find, galleys, looks, page_with, texts};
    use super::*;
    use crate::device::board::{By, CardRoot, Clock};
    use crate::device::firmware::{Image, Mode};
    use crate::device::page::fixtures;
    use crate::gui::window::studio::strong;

    /// The kit's modal title's size, its form buttons' capitals', the room
    /// after a keep's lead and between a foot's buttons.
    const TITLE: f32 = 16.0;
    const CAPS: f32 = 11.5;
    const LEAD_GAP: f32 = 8.0;
    const FOOT_GAP: f32 = 10.0;

    /// The gate's answers as its pass makes them of the cells' two.
    fn answers_of<'a>(keep: &'a str, write: &'a str) -> Answers<'a> {
        let (words, lead) = unled(keep);
        Answers { keep: words, lead, go: Some(write), also: &["COM3"] }
    }

    /// The cells' gate as they draw it over the page in a tall window, its
    /// words joined: every row from its title's to its answers', inside its
    /// frame.
    fn cells_words(page: &mut Page) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 44)).expect("a test terminal");
        let area = ratatui::layout::Rect { x: 0, y: 1, width: 100, height: 42 };
        terminal.draw(|frame| cells::render_hosted(frame, page, area)).expect("the cells drew");
        let buf = terminal.backend().buffer().clone();
        let rows: Vec<Vec<String>> = (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect();
        let title = cells::gate_title(page.gate.as_ref().expect("a gate"));
        let first: String = title.chars().take(12).collect();
        let (top, left) = rows
            .iter()
            .enumerate()
            .find_map(|(y, row)| row.concat().find(&first).map(|at| (y, row.concat()[..at].chars().count())))
            .expect("the cells' title");
        let mut words = Vec::new();
        for row in &rows[top..] {
            let inside: String = row.iter().skip(left).take(cells::GATE_W as usize - 4).map(String::as_str).collect();
            words.push(inside.trim().to_string());
            if row.concat().contains(glyphs().back) {
                break;
            }
        }
        squash(&words.join(" "))
    }

    /// `s` with its runs of spaces made one.
    fn squash(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The gate up on `page` says what the cells' says, in their order: its
    /// title, its body's every sentence (a line the cells cut, as far as
    /// they keep it), the erase box's words and cost, and its answers; and
    /// the egui pass drew each of them.
    fn says_the_cells_words(probe: &Headless, page: &mut Page) -> FullOutput {
        let gate = page.gate.clone().expect("a gate up");
        let pal = Pal::now();
        let cells = cells_words(page);
        let (keep, write) = cells::gate_buttons(&gate);
        let mut said = vec![(cells::gate_title(&gate), false)];
        for (_, piece) in body_of(&gate, &pal) {
            match piece {
                Piece::Words { text, line, .. } => said.push((text, line)),
                Piece::Erase { cost, .. } => said.extend([(t!("dev.gate_erase").to_string(), false), (cost, false)]),
            }
        }
        said.extend([(keep, false), (write, false)]);
        let mut from = 0;
        for (words, line) in &said {
            let mut words = squash(words);
            if *line {
                words = words.chars().take(48).collect();
            }
            let at = cells[from..].find(&words).unwrap_or_else(|| panic!("{words:?} after {:?} in the cells' gate: {cells}", &cells[..from]));
            from += at + words.len();
        }
        let out = probe.settle(page);
        let drawn: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        for (words, _) in &said[..said.len() - 2] {
            assert!(drawn.contains(words), "{words:?} drawn: {drawn:?}");
        }
        out
    }

    fn uiflow() -> Board {
        let mut uiflow = fixtures::ours("v1.13.2");
        uiflow.project = "UIFlow".into();
        fixtures::read("COM3", Some(uiflow))
    }

    /// COM3 behind the pin with the sheet's next write `image`, erased
    /// first where `erase`.
    fn with_next(board: Board, image: Image, erase: bool) -> Board {
        let mut board = fixtures::chosen(board, image, By::Sheet, None);
        if let Some(next) = board.next.as_mut() {
            next.erase = erase;
        }
        board
    }

    /// A board whose firmware formats, its card exFAT.
    fn exfat() -> Board {
        fixtures::answering_with("COM3", "v0.8.0", |s| (s.cid, s.clock, s.card) = (CardRoot::None, Clock::Ok, CardKind::ExFat))
    }

    /// The page with `boards` and the gate the primary opens on the board
    /// in view.
    fn gate_on(boards: Vec<Board>) -> (Page, Ends) {
        let (mut page, ends) = page_with(|e| e.boards(boards));
        page.act(Act::Primary);
        assert!(page.gate.is_some(), "the primary's gate");
        (page, ends)
    }

    #[test]
    fn every_gate_says_the_cells_words_in_their_order_and_their_tones() {
        let _en = english();
        let probe = Headless::new(BOARD);
        let pal = Pal::now();
        // An update: three sentences, the green one saying what stays.
        let (mut page, _ends) = gate_on(vec![fixtures::old("COM3", "v0.7.0")]);
        let out = says_the_cells_words(&probe, &mut page);
        assert_eq!(color_of(&out, "Update from v0.7.0 to v0.8.0?"), pal.gold, "the title in the gold");
        assert_eq!(looks(&out, "Update from v0.7.0 to v0.8.0?")[0].1, strong(TITLE));
        assert_eq!(color_of(&out, &t!("dev.gate_time")), pal.text);
        assert_eq!(color_of(&out, &t!("dev.gate_stays")), pal.ok, "what stays, green");
        assert_eq!(color_of(&out, &t!("dev.gate_plugged")), pal.text, "the one danger plain, never gold or red");
        // An install over other firmware: the gold line, the box and its
        // cost, dim; and over nothing.
        let (mut page, _ends) = gate_on(vec![uiflow()]);
        let out = says_the_cells_words(&probe, &mut page);
        assert_eq!(color_of(&out, &t!("dev.gate_unknown")), pal.gold);
        assert_eq!(color_of(&out, &t!("dev.gate_erase_on", name = "UIFlow")), pal.dim);
        assert_eq!(color_of(&out, &t!("dev.gate_card_untouched")), pal.ok);
        let (mut page, _ends) = gate_on(vec![fixtures::read("COM3", None)]);
        says_the_cells_words(&probe, &mut page);
        // Write again, from Details.
        let (mut page, _ends) = page_with(Ends::up_to_date);
        page.act(Act::Details);
        page.act(Act::WriteAgain);
        says_the_cells_words(&probe, &mut page);
        // Going back to a release the sheet chose: its source under the
        // title, and the beta's warning in gold under the green line.
        let (mut page, _ends) = gate_on(vec![with_next(fixtures::old_on("COM3", "v0.8.0", "e127a6bf"), Image::release("v0.7.0", Mode::Qio), false)]);
        assert_eq!(page.gate.as_ref().map(|g| g.kind), Some(Kind::Back));
        let out = says_the_cells_words(&probe, &mut page);
        let beta = page.gate.as_ref().and_then(|g| g.after.first()).map(|(w, _)| w.clone()).expect("the beta's line");
        assert_eq!(color_of(&out, &beta), pal.gold);
        // The other build of the same version, named in the title.
        let (mut page, _ends) = page_with(Ends::next_write);
        page.act(Act::Primary);
        assert_eq!(page.gate.as_ref().map(|g| g.kind), Some(Kind::Mode));
        says_the_cells_words(&probe, &mut page);
        // An erase the sheet chose: what goes in gold, what stays in green,
        // and the button says both verbs.
        let (mut page, _ends) = gate_on(vec![with_next(fixtures::old("COM3", "v0.7.0"), Image::Pin(Mode::Qio), true)]);
        let out = says_the_cells_words(&probe, &mut page);
        assert_eq!(color_of(&out, &t!("dev.gate_erase_any")), pal.gold);
        assert_eq!(color_of(&out, &t!("dev.gate_card_stays")), pal.ok);
        assert_drew(&out, &["ERASE AND UPDATE"]);
        // A local build over ours: its source in gold.
        let facts = fixtures::local_facts("C:/builds/core2", "v0.8.0-5-g4e94418", Mode::Qio);
        let local = fixtures::chosen(fixtures::old("COM3", "v0.8.0"), Image::Local("C:/builds/core2".into()), By::Sheet, Some(facts));
        let (mut page, _ends) = gate_on(vec![local]);
        assert_eq!(page.gate.as_ref().map(|g| g.kind), Some(Kind::Local));
        let out = says_the_cells_words(&probe, &mut page);
        let source = page.gate.as_ref().and_then(|g| g.lines.first()).map(|(w, _)| w.clone()).expect("its source");
        assert_eq!(color_of(&out, &source), pal.gold);
        // The format: four sentences, what is lost in gold.
        let (mut page, _ends) = gate_on(vec![exfat()]);
        assert_eq!(page.gate.as_ref().map(|g| g.kind), Some(Kind::Format));
        let out = says_the_cells_words(&probe, &mut page);
        let lost = t!("dev.card.gate_fmt_lost", what = t!("dev.card.gate_fmt_one", fs = "exFAT")).to_string();
        assert_eq!(color_of(&out, &lost), pal.gold);
        assert_drew(&out, &["Format the card in COM3 as FAT32?"]);
        // Update all: a line a board, the left-out board dim; and with more
        // boards than six lines, "and N more".
        let left_out = with_next(fixtures::old("COM9", "v0.6.0"), Image::release("v0.7.0", Mode::Qio), false);
        let (mut page, _ends) = page_with(|e| e.boards(vec![fixtures::old("COM3", "v0.7.0"), fixtures::old_on("COM5", "v0.7.0", "aa45f60e"), left_out]));
        page.act(Act::UpdateAll);
        let out = says_the_cells_words(&probe, &mut page);
        assert_drew(&out, &["COM5 · serial 5B1F00COM5 · v0.7.0 → v0.8.0 in DIO"]);
        let row = "COM3 · serial 5B1F00COM3 · v0.7.0 → v0.8.0";
        assert_eq!(looks(&out, row).iter().filter(|(_, f)| *f == mono(FIGURES)).count(), 4, "the port, the serial and the versions in the monospace");
        assert_eq!(color_of(&out, "COM9 is left out: its next write is v0.7.0. Write it from its tab."), pal.dim);
        assert_eq!(color_of(&out, &t!("dev.gate_all_stays")), pal.ok);
        let many: Vec<Board> = (1..=8).map(|i| fixtures::old(&format!("COM{i}"), "v0.7.0")).collect();
        let (mut page, _ends) = page_with(|e| e.boards(many));
        page.act(Act::UpdateAll);
        let out = says_the_cells_words(&probe, &mut page);
        assert_eq!(color_of(&out, &t!("dev.gate_all_more", n = 2)), pal.dim, "six lines, and 2 more");
        assert!(!drew(&out, "COM7 · serial 5B1F00COM7 · v0.7.0 → v0.8.0"));
    }

    #[test]
    fn the_safe_answer_is_lit_and_led_back_and_keeps_and_the_write_is_the_dim_secondary() {
        let _en = english();
        let probe = Headless::new(WINDOW);
        let pal = Pal::now();
        let (mut page, ends) = gate_on(vec![fixtures::old("COM3", "v0.7.0")]);
        let out = probe.settle(&mut page);
        let keep_id = Id::new("mp3-gate").with("foot").with(0usize);
        let (keep, go) = (probe.rect_of(keep_id), probe.rect_of(Id::new("mp3-gate").with("foot").with(1usize)));
        // The keep: the accent's fill, its words in the buttons' capitals,
        // its version in the monospace in its own case, led by the pointer
        // turned back.
        let lit = |out: &FullOutput, fill: Color32, at: Rect| out.shapes.iter().any(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == fill && r.rect == at));
        assert!(lit(&out, pal.accent, keep), "the safe answer lit");
        assert!(!lit(&out, pal.accent, go), "the write never lit");
        assert_eq!(looks(&out, "KEEP "), [(pal.on_accent, strong(CAPS))]);
        assert_eq!(looks(&out, "v0.7.0").last().map(|l| l.1.clone()), Some(mono(CAPS)), "the version in its own case");
        let (words, version) = (find(&out, "KEEP "), texts(&out).into_iter().filter(|(t, r)| t == "v0.7.0" && keep.contains(r.center())).count());
        assert_eq!(version, 1, "the version on the keep");
        let lead = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Path(p) if p.fill == pal.on_accent && p.points.len() == 3 => Some(p.points.clone()),
            _ => None,
        });
        let lead = lead.expect("the pointer turned back");
        let (tip, back) = (lead.iter().fold(f32::MAX, |m, p| m.min(p.x)), lead.iter().fold(f32::MIN, |m, p| m.max(p.x)));
        assert!(lead[0].x == tip && (words.min.x - back - LEAD_GAP).abs() < 0.5, "pointing left, 8 before the words: {lead:?} {words:?}");
        assert!(keep.max.x < go.min.x && (go.min.x - keep.max.x - FOOT_GAP).abs() < 0.5 && (keep.center().y - go.center().y).abs() < 0.5, "side by side, the safe one first");
        let ink = galleys(&out).into_iter().find(|(at, g)| g.text() == "UPDATE" && go.contains(Rect::from_min_size(*at, g.size()).center()));
        assert_eq!(ink.map(|(_, g)| g.job.sections[0].format.color), Some(pal.dim), "the write, the dim secondary");
        // Under the pointer the keep heats.
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(keep.center())]);
        let out = probe.settle(&mut page);
        assert!(lit(&out, pal.accent_hot, keep), "the hot accent under the pointer");
        // A gate has no field: no caret.
        let shown = probe.shown(&mut page);
        assert!(shown.caret.is_none() && shown.outcome.is_none());
        // The keep keeps; the write writes, and locks the page from the yes.
        probe.click(&mut page, keep.center());
        assert!(page.gate.is_none() && ends.sent().is_empty() && !page.writing(), "kept: nothing sent");
        page.act(Act::Primary);
        let _ = probe.settle(&mut page);
        probe.click(&mut page, go.center());
        assert!(page.gate.is_none() && page.writing());
        assert!(matches!(ends.sent().as_slice(), [Cmd::Write { port, erase: Some(false), .. }] if port == "COM3"));
        // The format's go asks the player and locks nothing; Update all's
        // writes the boards it named.
        let (mut page, ends) = gate_on(vec![exfat()]);
        let _ = probe.settle(&mut page);
        probe.click(&mut page, probe.rect_of(Id::new("mp3-gate").with("foot").with(1usize)).center());
        assert_eq!(ends.sent(), [Cmd::Format { port: "COM3".into() }]);
        assert!(page.gate.is_none() && !page.writing());
        let (mut page, ends) = page_with(|e| e.boards(vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0")]));
        page.act(Act::UpdateAll);
        let _ = probe.settle(&mut page);
        probe.click(&mut page, probe.rect_of(Id::new("mp3-gate").with("foot").with(1usize)).center());
        assert_eq!(ends.sent(), [Cmd::UpdateAll { ports: vec!["COM3".into(), "COM5".into()] }]);
        assert!(page.writing());
    }

    #[test]
    fn the_install_gates_box_is_the_cells_erase_and_its_cost_follows_it() {
        let _en = english();
        let probe = Headless::new(WINDOW);
        let (mut page, ends) = gate_on(vec![uiflow()]);
        let out = probe.settle(&mut page);
        let (on, off) = (t!("dev.gate_erase_on", name = "UIFlow").to_string(), t!("dev.gate_erase_off", name = "UIFlow").to_string());
        assert!(drew(&out, &on) && !drew(&out, &off), "on by default");
        let box_words = find(&out, &t!("dev.gate_erase"));
        probe.click(&mut page, box_words.center());
        assert_eq!(page.gate.as_ref().map(|g| g.erase), Some(false), "the cells' Erase");
        let out = probe.settle(&mut page);
        assert!(drew(&out, &off) && !drew(&out, &on), "its cost follows it");
        probe.click_id(&mut page, Id::new("mp3-gate").with("erase"));
        assert_eq!(page.gate.as_ref().map(|g| g.erase), Some(true), "and back");
        assert!(ends.sent().is_empty(), "the box writes nothing");
        // A figure in the old firmware's name stands in the monospace a
        // point under the cost's words, as in every sentence of the gate.
        if let Some(gate) = page.gate.as_mut() {
            gate.name = Some("UIFlow v1.13.2".into());
        }
        let out = probe.settle(&mut page);
        let cost = t!("dev.gate_erase_on", name = "UIFlow v1.13.2").to_string();
        let faces: Vec<egui::FontId> = looks(&out, &cost).into_iter().map(|(_, f)| f).collect();
        assert_eq!(faces, [body(COST.0), mono(COST.0 - 1.0), body(COST.0)], "{cost:?}");
        assert_eq!(color_of(&out, &cost), Pal::now().dim);
        // The install writes as the box says.
        page.act(Act::Erase);
        let _ = probe.settle(&mut page);
        probe.click_id(&mut page, Id::new("mp3-gate").with("foot").with(1usize));
        assert!(matches!(ends.sent().as_slice(), [Cmd::Write { erase: Some(false), .. }]));
    }

    #[test]
    fn under_a_gate_the_page_and_the_scrim_take_no_click() {
        let _en = english();
        let probe = Headless::new(WINDOW);
        let (mut page, ends) = page_with(|e| e.boards(vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0")]));
        let out = probe.settle(&mut page);
        let (details, tab, all) = (find(&out, "Details").center(), find(&out, "COM5").center(), find(&out, "Update all (2)").center());
        page.act(Act::Primary);
        let _ = probe.settle(&mut page);
        for at in [details, tab, all, pos2(4.0, 4.0), pos2(WINDOW.x - 4.0, WINDOW.y - 4.0)] {
            probe.click(&mut page, at);
            let kept = page.gate.as_ref().is_some_and(|g| g.kind == Kind::Update);
            assert!(kept && !page.details && page.open.as_deref() == Some("COM3"), "{at:?} under the gate takes nothing");
        }
        assert!(ends.sent().is_empty(), "and asks nothing");
    }

    /// What the gate drew: everything from its scrim on, over the page.
    fn gate_shapes(out: &FullOutput) -> &[egui::epaint::ClippedShape] {
        let scrim = out.shapes.iter().position(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == studio::SCRIM)).expect("the gate's scrim");
        &out.shapes[scrim..]
    }

    /// Where the gate's window stands: its gold frame.
    fn window_of(out: &FullOutput) -> Rect {
        let gold = Pal::now().gold;
        gate_shapes(out)
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Rect(r) if r.stroke.color == gold && r.stroke.width == 1.0 => Some(r.rect),
                _ => None,
            })
            .expect("the gate's gold frame")
    }

    #[test]
    fn at_the_guis_floor_every_gate_stands_whole_with_both_answers_under_its_words() {
        let _en = english();
        let many: Vec<Board> = (1..=8).map(|i| fixtures::old(&format!("COM{i}"), "v0.7.0")).collect();
        let mut gates: Vec<(Page, Ends)> = vec![
            gate_on(vec![uiflow()]),
            gate_on(vec![exfat()]),
            gate_on(vec![with_next(fixtures::old("COM3", "v0.7.0"), Image::Pin(Mode::Qio), true)]),
            {
                let (mut page, ends) = page_with(|e| e.boards(many));
                page.act(Act::UpdateAll);
                (page, ends)
            },
        ];
        for size in [FLOOR, vec2(360.0, 352.0), BOARD] {
            let probe = Headless::new(size);
            for (page, _ends) in gates.iter_mut() {
                let kind = page.gate.as_ref().map(|g| g.kind);
                let out = probe.settle(page);
                let window = window_of(&out);
                let area = Rect::from_min_size(Pos2::ZERO, size);
                assert!(area.shrink(7.5).contains_rect(window), "{kind:?} at {size:?}: {window:?} clear of the area's edges");
                let keep = probe.rect_of(Id::new("mp3-gate").with("foot").with(0usize));
                let go = probe.rect_of(Id::new("mp3-gate").with("foot").with(1usize));
                assert!(window.shrink(8.0).contains_rect(keep) && window.shrink(8.0).contains_rect(go), "{kind:?} at {size:?}: both answers whole");
                assert!(!keep.intersects(go), "{kind:?} at {size:?}: apart");
                // Every word of the body over the answers, the last lines
                // left out whole where it is cut.
                let top = keep.min.y.min(go.min.y);
                let mut words = 0;
                for (clip, (t, r)) in gate_shapes(&out).iter().filter_map(|c| match &c.shape {
                    Shape::Text(t) => Some((c.clip_rect, (t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size())))),
                    _ => None,
                }) {
                    let seen = r.intersect(clip);
                    if !keep.contains(r.center()) && !go.contains(r.center()) && seen.is_positive() {
                        assert!(window.contains_rect(seen), "{kind:?} at {size:?}: {t:?} at {seen:?} inside the window");
                        assert!(seen.max.y <= top - 8.0, "{kind:?} at {size:?}: {t:?} at {seen:?} over the answers at {top}");
                        words += 1;
                    }
                }
                assert!(words >= 3, "{kind:?} at {size:?}: the title and some of the body drawn");
                // At the GUI's floor the whole gate stands, as the cells' does
                // (clause 4): its last sentence drawn whole.
                if size == FLOOR {
                    let gate = page.gate.clone().expect("a gate");
                    let last = body_of(&gate, &Pal::now()).into_iter().rev().find_map(|(_, p)| match p {
                        Piece::Words { text, .. } => Some(text),
                        Piece::Erase { .. } => None,
                    });
                    let last = last.expect("a sentence");
                    let whole = gate_shapes(&out).iter().any(|c| match &c.shape {
                        Shape::Text(t) => t.galley.text() == last && c.clip_rect.contains_rect(Rect::from_min_size(t.pos, t.galley.size())),
                        _ => false,
                    });
                    assert!(whole, "{kind:?} at the floor: {last:?} whole");
                }
                if size == BOARD {
                    assert!(window.height() < size.y - 100.0, "{kind:?}: as tall as its words in the boards' window");
                }
            }
        }
    }

    #[test]
    fn short_of_room_the_gaps_give_way_from_the_end_before_the_frame_tightens() {
        let _en = english();
        let (page, _ends) = gate_on(vec![uiflow()]);
        let probe = Headless::new(BOARD);
        let gate = page.gate.clone().expect("the install gate");
        let pal = Pal::now();
        let pieces = body_of(&gate, &pal);
        let gaps = pieces.iter().filter(|(gap, _)| *gap).count();
        assert!(gaps >= 4, "the install's blank lines are its gaps: {gaps}");
        let ctx = probe.ctx.clone();
        let (keep, write) = cells::gate_buttons(&gate);
        let title = cells::gate_title(&gate);
        // Laid out over areas ever shorter: the measures stay the kit's
        // while the gaps can give way, then tighten.
        let mut last = None;
        let input = |h: f32| egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, h))), ..Default::default() };
        for h in [824.0, 400.0, 360.0, 330.0, 300.0, 240.0] {
            let mut out = ctx.run_ui(input(h), |ui| {
                let face = Face { pal: &pal, composition: "", key_hints: false, full: ui.max_rect(), space: super::super::ROOMY, ports: &["COM3"], live: true, inert: false };
                let laid = lay_out(ui, &face, &title, &pieces, &answers_of(&keep, &write));
                assert!(laid.window.height() <= (h - laid.m.clear).max(0.0) + 0.5, "{h}: {:?}", laid.window);
                if h == 824.0 {
                    assert_eq!(laid.m, ROOMY, "the boards' window: the kit's measures");
                    assert!(laid.tops.iter().all(Option::is_some), "and every piece drawn");
                }
                if let Some(was) = last.replace(laid.m) {
                    assert!(!(was == TIGHT && laid.m == ROOMY), "{h}: the frame tightens only as the room goes");
                }
            });
            out.textures_delta.clear();
        }
        assert_eq!(last, Some(TIGHT), "at 240 points the frame is the tighter one");
        // A point more than room for every gap but the last: the last gives
        // way, the others stand, and the measures stay the kit's.
        let face_in = |ui: &Ui| Face { pal: &pal, composition: "", key_hints: false, full: ui.max_rect(), space: super::super::ROOMY, ports: &["COM3"], live: true, inert: false };
        let answers = answers_of(&keep, &write);
        let said = (title.as_str(), pieces.as_slice(), &answers);
        let mut all_but_one = 0.0_f32;
        let mut out = ctx.run_ui(input(824.0), |ui| {
            let face = face_in(ui);
            all_but_one = Fit::of(ui, &face, ROOMY, studio::GATE_W, said).height(&pieces, gaps - 1);
        });
        out.textures_delta.clear();
        let mut out = ctx.run_ui(input(all_but_one + ROOMY.clear + 1.0), |ui| {
            let face = face_in(ui);
            let laid = lay_out(ui, &face, &title, &pieces, &answers_of(&keep, &write));
            assert_eq!(laid.m, ROOMY, "the kit's measures while a gap can give way");
            let heights = measure(ui, &face, &pieces, laid.window.width() - 2.0 * laid.m.pad);
            let tops: Vec<f32> = laid.tops.iter().map(|top| top.expect("every piece drawn")).collect();
            let apart: Vec<f32> = tops.windows(2).zip(&heights).map(|(two, h)| two[1] - two[0] - h).collect();
            let mut seen = 0;
            let wanted: Vec<f32> = pieces[1..]
                .iter()
                .map(|(gap, _)| {
                    seen += usize::from(*gap);
                    if *gap && seen < gaps { ROOMY.apart } else { 0.0 }
                })
                .collect();
            assert!(apart.iter().zip(&wanted).all(|(a, w)| (a - w).abs() < 0.01), "the last gap gone, the rest kept: {apart:?}, not {wanted:?}");
        });
        out.textures_delta.clear();
    }

    #[test]
    fn a_gate_the_search_finds_room_for_within_its_half_point_keeps_its_last_line() {
        let _en = english();
        let (page, _ends) = gate_on(vec![fixtures::old("COM3", "v0.7.0")]);
        let probe = Headless::new(BOARD);
        let gate = page.gate.clone().expect("the update gate");
        let pal = Pal::now();
        let pieces = body_of(&gate, &pal);
        let gaps = pieces.iter().filter(|(gap, _)| *gap).count();
        let (keep, write) = cells::gate_buttons(&gate);
        let title = cells::gate_title(&gate);
        let answers = answers_of(&keep, &write);
        let said = (title.as_str(), pieces.as_slice(), &answers);
        let ctx = probe.ctx.clone();
        let input = |h: f32| egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, h))), ..Default::default() };
        let face_in = |ui: &Ui| Face { pal: &pal, composition: "", key_hints: false, full: ui.max_rect(), space: super::super::ROOMY, ports: &["COM3"], live: true, inert: false };
        // The gate's height with the kit's measures and every gap.
        let mut whole = 0.0_f32;
        let mut out = ctx.run_ui(input(BOARD.y), |ui| {
            let face = face_in(ui);
            let width = studio::GATE_W.min(face.full.width() - ROOMY.clear);
            whole = Fit::of(ui, &face, ROOMY, width, said).height(&pieces, gaps);
        });
        out.textures_delta.clear();
        // An area a few tenths of a point short of that: room enough for
        // every gap as the search counts it, so the window stands as tall as
        // its words and its last line is drawn over its answers.
        let mut out = ctx.run_ui(input(whole + ROOMY.clear - 0.3), |ui| {
            let face = face_in(ui);
            let short = whole - (face.full.height() - ROOMY.clear);
            assert!(short > 0.01 && short < 0.5, "the area {short} points short of the gate");
            let laid = lay_out(ui, &face, &title, &pieces, &answers_of(&keep, &write));
            assert_eq!(laid.m, ROOMY, "the kit's measures");
            assert!(laid.tops.iter().all(Option::is_some), "every piece drawn: {:?}", laid.tops);
            let heights = measure(ui, &face, &pieces, laid.window.width() - 2.0 * laid.m.pad);
            let foot = laid.tops.last().copied().flatten().expect("the last piece") + heights.last().copied().unwrap_or(0.0);
            assert!(foot <= laid.floor + 0.01, "the last line's foot {foot} over the floor {}", laid.floor);
        });
        out.textures_delta.clear();
    }
}
