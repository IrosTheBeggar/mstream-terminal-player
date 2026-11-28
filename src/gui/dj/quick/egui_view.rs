//! Quick config's egui face in the GUI's own window, hosted by the chrome
//! (src/gui/chrome.rs): the QuickConfig board's popover (decision D18),
//! drawn from quick config's view line by line, its presses the cells'
//! acts.
//!
//! It hangs as the kit hangs a list: the panel's fill in the accent's
//! hairline with the canvas's shadow, rising from the split's divider as
//! wide as the cells' rect (42 cells from the 14th), its foot 4 points over
//! the seek row; its rows 20 points tall, the gaps between its groups 8 and
//! its padding 10, so its tallest case (four servers, sonic on) stands whole
//! under the strip at the floor with the tips row on. Its shadow stops at
//! the room's edges: nothing of it reaches the bar or the strip.
//!
//! The Gui's state stays the model. The keyboard's hand is the view's, worn
//! as the Admin canvas wears it (the raised fill and the accent's ring) and
//! never egui's focus, and every key stays the cells'. A press works what
//! the cells' registry works there, on the press as the cells act: a stop's
//! `DjQuick(Press(stop))` on its mark and words (a server's, the switch's,
//! All's) or on its whole row (a value row's), the match bar's ends a step
//! down or up and each of its segments the floor it sets; nothing on the
//! frame off them, the cells' Guard; and anywhere else `Close`, the bar's
//! own controls among them, as the cells' catcher over the whole window
//! takes them. A row the server cannot do rests dim and takes nothing.
//! Nothing lights, shows the hand or takes a press outside where quick
//! config holds the pointer.

use egui::epaint::Shadow;
use egui::{Align, Align2, Color32, CornerRadius, FontId, Id, Order, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::{QStop, QuickAct, QuickBar, QuickLine, QuickView, Tone};
use crate::gui::Act;
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Pal};

/// Whether egui draws quick config (the host draws it in cells until it
/// does).
pub(crate) const READY: bool = true;

/// What quick config's face is handed besides the view.
pub(crate) struct QuickCtx<'a> {
    pub cells: Cells,
    /// Where quick config holds the pointer, in points: the window less
    /// what the cells stacked over it. Nothing lights, shows the hand or
    /// takes a press outside it.
    pub live: &'a [egui::Rect],
    pub pal: &'a Pal,
}

/// What quick config's face did this pass.
#[derive(Default)]
pub(crate) struct QuickOut {
    /// The act of this pass's press: a stop's `DjQuick(Press(stop))`, the
    /// match bar's `Step` or `Set`, nothing on its frame off them,
    /// `DjQuick(Close)` anywhere else in `live`.
    pub act: Option<Act>,
    /// What a press works where, in the order they stack (the catcher
    /// first), for the script's dumps.
    pub items: Vec<(egui::Rect, Act)>,
    /// Its frame grown by its shadow, kept to the room: its part for the
    /// host's cut.
    pub frame: Option<egui::Rect>,
}

/// The canvas's shadow under what floats over a page, as the kit lays it
/// under a list that hangs (the kit keeps its own to itself).
const LIFTED: Shadow = Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(115) };

/// The board's measures, in points: a row, a gap between groups, the
/// frame's padding, and how far over the seek row its foot stands.
const ROW_H: f32 = 20.0;
const GAP_H: f32 = 8.0;
const PAD: f32 = 10.0;
const FOOT: f32 = 4.0;
/// How far a control's look reaches past its mark and words either side.
const REACH: f32 = 6.0;
/// Where a choice's name stands past the content's left edge: past its
/// 16-point mark and the 10 after it.
const NAMES: f32 = studio::BOX + 10.0;
/// The words' size, and the figures' in the monospace.
const SIZE: f32 = 12.5;
const FIGURE: f32 = 12.0;
/// A value row's value ends this far before the row's end, where its
/// pointer stands; and the least room between a name and its value.
const VALUE_END: f32 = 16.0;
const VALUE_GAP: f32 = 10.0;
/// The room the way to the room's pointer takes after its words: the 8
/// before it and its 6.
const LEAD: f32 = 8.0 + 6.0;
/// The match bar: its quiet ends' box, the segments' width, height and the
/// room between them, the room from the minus's box to the first segment,
/// and from the last segment to the plus.
const END_W: f32 = 8.0;
const SEG_W: f32 = 6.0;
const SEG_H: f32 = 10.0;
const SEG_GAP: f32 = 2.0;
const SEG_FROM: f32 = 8.0;
const SEG_TO: f32 = 10.0;

/// Quick config, up: the popover over the room, a press on a control its
/// act, on its frame off them nothing, anywhere else inside `live` Close.
pub(crate) fn show(ui: &mut Ui, view: &QuickView, cx: &QuickCtx) -> QuickOut {
    let mut out = QuickOut::default();
    let placed = place(view, cx.cells);
    let screen = cx.cells.points;
    let live = |at: &Pos2| cx.live.iter().any(|rect| rect.contains(*at));
    let id = Id::new("quick-config");
    let ctx = ui.ctx().clone();
    let area = egui::Area::new(id).order(Order::Foreground).fixed_pos(screen.min).constrain(false).fade_in(false);
    area.show(&ctx, |ui| {
        let seat = Seat::of(ui, &live);
        // Under everything: the catcher, which takes a press anywhere off
        // the popover and shows the hand as the cells' does, then the
        // frame, which takes one off its controls and shows none.
        let close = Act::DjQuick(QuickAct::Close);
        let catcher = ui.interact(screen, id.with("catcher"), Sense::click());
        let _ = ui.interact(placed.frame, id.with("frame"), Sense::click());
        out.items.push((screen, close.clone()));
        out.items.push((placed.frame, Act::Guard));
        if worked(catcher, &seat).1 {
            out.act = Some(close);
        }
        ui.set_clip_rect(placed.part);
        let pal = cx.pal;
        ui.painter().add(LIFTED.as_shape(placed.frame, studio::square()));
        ui.painter().rect_filled(placed.frame, studio::square(), pal.panel);
        ui.painter().rect_stroke(placed.frame, studio::square(), Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        let mut pen = Pen { ui, id, seat, hand: view.hand, pal, out: &mut out };
        let (left, right) = (placed.frame.min.x + PAD, placed.frame.max.x - PAD);
        for (line, rect) in view.lines.iter().zip(&placed.rows) {
            let Some(rect) = *rect else { break };
            pen.line(Row { rect, left, right }, line);
        }
    });
    out.frame = Some(placed.part);
    out
}

/// Where the popover stands, in points.
struct Placed {
    /// From the cells' left edge to their right, its foot [`FOOT`] over the
    /// seek row, as tall as its lines and its padding, but never over row 1.
    frame: Rect,
    /// The frame grown by its shadow and kept to the room, from row 1 to the
    /// seek row: its part of the paint.
    part: Rect,
    /// Each line's row, top to foot, while it stands whole inside the
    /// frame's padding: a line that would not is left out, as the cells
    /// leave out a line past their rect.
    rows: Vec<Option<Rect>>,
}

/// How tall a line stands.
fn height(line: &QuickLine) -> f32 {
    if *line == QuickLine::Gap { GAP_H } else { ROW_H }
}

/// Where `view` stands over `cells`: its sides the cells' rect's, its foot
/// on the seek row's top less [`FOOT`], its top as high as its lines want
/// and no higher than row 1's top.
fn place(view: &QuickView, cells: Cells) -> Placed {
    let grid = cells.grid;
    let band = |y: u16| cells.rect(ratatui::layout::Rect { x: grid.x, y, width: grid.width, height: 1 });
    let across = cells.rect(view.rect);
    let seek = band(view.bar_top).min.y;
    let room = band(grid.y + 1).min.y.min(seek);
    let foot = (seek - FOOT).max(room);
    let want = 2.0 * PAD + view.lines.iter().map(height).sum::<f32>();
    let top = (foot - want).max(room);
    let frame = Rect::from_min_max(pos2(across.min.x, top), pos2(across.max.x, foot));
    let bounds = Rect::from_min_max(pos2(cells.points.min.x, room), pos2(cells.points.max.x, seek));
    let part = (frame + LIFTED.margin()).intersect(bounds);
    let mut y = top + PAD;
    let rows = view
        .lines
        .iter()
        .map(|line| {
            let rect = Rect::from_min_size(pos2(frame.min.x, y), vec2(frame.width(), height(line)));
            y = rect.max.y;
            (rect.max.y <= foot - PAD + 0.01).then_some(rect)
        })
        .collect();
    Placed { frame, part, rows }
}

/// Where the pointer is for the popover this pass, each only inside where
/// quick config holds it: where it hovers, and where a primary press came
/// (read off the pass's events, as a tap's press and release may come in
/// one pass; a pass egui runs again hears none).
struct Seat {
    hover: Option<Pos2>,
    press: Option<Pos2>,
}

impl Seat {
    fn of(ui: &Ui, live: &impl Fn(&Pos2) -> bool) -> Seat {
        ui.input(|i| {
            let press = i.events.iter().rev().find_map(|event| match *event {
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } => Some(pos),
                _ => None,
            });
            Seat { hover: i.pointer.hover_pos().filter(live), press: press.filter(live) }
        })
    }
}

/// What the pointer did to a control of the popover: whether it lights it,
/// with the hand, and whether this pass's press is its own (on the press,
/// as the cells act), each only where quick config holds the pointer.
fn worked(response: Response, seat: &Seat) -> (bool, bool) {
    let hot = seat.hover.is_some() && response.hovered();
    let pressed = seat.press.is_some() && studio::pressed(&response);
    if hot {
        let _ = studio::hand(response);
    }
    (hot, pressed)
}

/// The look a control wears in `rect`: the keyboard's hand's (the raised
/// fill and the accent's ring), else the pointer's (the raised fill), else
/// none.
fn look(ui: &Ui, rect: Rect, hand: bool, hot: bool, pal: &Pal) {
    if hand {
        studio::cursor_look(ui, rect, pal);
    } else if hot {
        ui.painter().rect_filled(rect, CornerRadius::same(2), pal.raised);
    }
}

/// A value's colour for its tone, as the cells colour it.
fn ink(tone: Tone, pal: &Pal) -> Color32 {
    match tone {
        Tone::Neutral => pal.dim,
        Tone::Active => pal.ok,
        Tone::Warning => pal.gold,
    }
}

/// The words of a choice, plain or, lit, in the semibold.
fn face(strong: bool) -> FontId {
    if strong { studio::strong(SIZE) } else { studio::body(SIZE) }
}

/// A line's row: the frame's width, and the content's edges inside its
/// padding.
#[derive(Clone, Copy)]
struct Row {
    rect: Rect,
    left: f32,
    right: f32,
}

impl Row {
    fn mid(&self) -> f32 {
        self.rect.center().y
    }

    /// Where a choice's name stands.
    fn names(&self) -> f32 {
        self.left + NAMES
    }

    /// Where the row's words stand on their baseline: a line of the UI face
    /// centred on the row, which a figure or a value at its right is stood
    /// on too.
    fn baseline(&self, ui: &Ui) -> f32 {
        self.mid() + studio::centred_baseline(ui, SIZE)
    }

    /// A control's target and its look across the row from `from` to `to`,
    /// reaching [`REACH`] past them either side.
    fn hit(&self, from: f32, to: f32) -> Rect {
        Rect::from_min_max(pos2(from - REACH, self.rect.min.y), pos2(to.max(from) + REACH, self.rect.max.y))
    }

    /// A tick box's or a choice's mark at the row's left, on its middle.
    fn mark(&self) -> Rect {
        Rect::from_min_size(pos2(self.left, self.mid() - studio::BOX / 2.0), vec2(studio::BOX, studio::BOX))
    }
}

/// What draws the lines: the popover's ui, its pointer and the keyboard's
/// hand, and what the pass reports.
struct Pen<'a> {
    ui: &'a Ui,
    id: Id,
    seat: Seat,
    hand: Option<QStop>,
    pal: &'a Pal,
    out: &'a mut QuickOut,
}

impl Pen<'_> {
    /// A control at `rect` that works `act`: listed for the dumps, its
    /// press this pass's act. Whether the pointer lights it.
    fn control(&mut self, rect: Rect, salt: impl std::hash::Hash + std::fmt::Debug, act: Act) -> bool {
        self.out.items.push((rect, act.clone()));
        let (hot, pressed) = worked(self.ui.interact(rect, self.id.with(salt), Sense::click()), &self.seat);
        if pressed {
            self.out.act = Some(act);
        }
        hot
    }

    /// Whether the keyboard's hand is on `stop`.
    fn on(&self, stop: QStop) -> bool {
        self.hand == Some(stop)
    }

    fn line(&mut self, row: Row, line: &QuickLine) {
        match line {
            QuickLine::Gap => {}
            QuickLine::Head(words) => {
                studio::small_caps(self.ui, pos2(row.left, row.mid()), words, self.pal.dim, row.right - row.left, Align2::LEFT_CENTER);
            }
            QuickLine::Server { stop, words, peer, current } => self.server(row, *stop, words, *peer, *current),
            QuickLine::PlayFrom { stop, label, value } => self.value_row(row, *stop, row.left, label, Some((value.as_str(), Tone::Neutral))),
            QuickLine::Sonic { label, gate: Some(why), .. } => self.rest(row, true, label, why, row.right),
            QuickLine::Sonic { label, gate: None, on, value, tone } => self.sonic(row, label, *on, value, *tone),
            QuickLine::Bar(bar) => self.bar(row, bar),
            QuickLine::Seeds { stop, label } => self.value_row(row, *stop, row.names(), label, None),
            QuickLine::Smart { label, gate: Some(why), .. } => self.rest(row, false, label, why, row.right - VALUE_END),
            QuickLine::Smart { label, gate: None, value, tone } => self.value_row(row, QStop::Smart, row.left, label, Some((value.as_str(), *tone))),
            QuickLine::All { stop, label } => self.all(row, *stop, label),
        }
    }

    /// One of Play from's servers: its round mark, chosen in the accent on
    /// the current one, and its name, a peer's behind its branch; the mark
    /// and the name take the press, not the row. The current one's name in
    /// the semibold, as the cells bold it, and so under the pointer or the
    /// keyboard's hand.
    fn server(&mut self, row: Row, stop: QStop, words: &str, peer: bool, current: bool) {
        let (ui, pal) = (self.ui, self.pal);
        let word = if peer { format!("└ {words}") } else { words.to_string() };
        let room = (row.right - row.names()).max(1.0);
        let wide = studio::width_of(ui, &word, face(true)).min(room);
        let hit = row.hit(row.left, row.names() + wide);
        let hand = self.on(stop);
        let hot = self.control(hit, format!("{stop:?}"), Act::DjQuick(QuickAct::Press(stop)));
        look(ui, hit, hand, hot, pal);
        studio::radio_mark(ui, row.mark().center(), current, pal.accent, pal);
        studio::text(ui, pos2(row.names(), row.mid()), &word, face(current || hand || hot), pal.text, room, Align2::LEFT_CENTER);
    }

    /// Sonic similarity's switch: the tick box, on in the ok green, and its
    /// name, which take the press; its value at the row's end in its tone
    /// (the floor, its figure in the monospace, or why a running session
    /// dropped it), the name cut short of it rather than under it.
    fn sonic(&mut self, row: Row, label: &str, on: bool, value: &str, tone: Tone) {
        let (ui, pal) = (self.ui, self.pal);
        let parts = studio::figured(value, &[], studio::body(SIZE), studio::mono(FIGURE), ink(tone, pal));
        let value_w = if value.is_empty() { 0.0 } else { studio::runs_width(ui, &parts).min(row.right - row.names()) };
        let room = (row.right - row.names() - if value_w > 0.0 { value_w + VALUE_GAP } else { 0.0 }).max(1.0);
        let wide = studio::width_of(ui, label, face(true)).min(room);
        let hit = row.hit(row.left, row.names() + wide);
        let hand = self.on(QStop::Sonic);
        let hot = self.control(hit, format!("{:?}", QStop::Sonic), Act::DjQuick(QuickAct::Press(QStop::Sonic)));
        look(ui, hit, hand, hot, pal);
        studio::tick_box(ui, row.mark(), on, hot, pal.ground, pal);
        studio::text(ui, pos2(row.names(), row.mid()), label, face(on || hand || hot), pal.text, room, Align2::LEFT_CENTER);
        if value_w > 0.0 {
            studio::runs_on(ui, row.right, row.baseline(ui), &parts, value_w + 1.0, Align::Max);
        }
    }

    /// A row the server cannot do, resting: its name dim from the content's
    /// left (behind an empty tick box where `boxed`, at the names), giving
    /// way to `why`, dim too and whole, which ends at `end`. No look, no
    /// hand, no press: the frame's, as a resting row is the cells' Guard's.
    fn rest(&self, row: Row, boxed: bool, label: &str, why: &str, end: f32) {
        let (ui, pal) = (self.ui, self.pal);
        let from = if boxed { row.names() } else { row.left };
        if boxed {
            studio::tick_box(ui, row.mark(), false, false, pal.ground, pal);
        }
        let parts = studio::figured(why, &[], studio::body(SIZE), studio::mono(FIGURE), pal.dim);
        let why_w = studio::runs_width(ui, &parts).min((end - from).max(1.0));
        studio::runs_on(ui, end, row.baseline(ui), &parts, why_w + 1.0, Align::Max);
        let room = (end - why_w - VALUE_GAP - from).max(1.0);
        studio::text(ui, pos2(from, row.mid()), label, studio::body(SIZE), pal.dim, room, Align2::LEFT_CENTER);
    }

    /// A room's row here, from `from` to the content's end, the whole row
    /// its target: its name, its value in its tone ending [`VALUE_END`]
    /// before the row's end (cut where the name leaves it short, as the
    /// cells cut it), and the kit's pointer there, dim; the name in the
    /// semibold and the pointer in the text colour under the pointer or the
    /// keyboard's hand.
    fn value_row(&mut self, row: Row, stop: QStop, from: f32, label: &str, value: Option<(&str, Tone)>) {
        let (ui, pal) = (self.ui, self.pal);
        let hit = row.hit(from, row.right);
        let hand = self.on(stop);
        let hot = self.control(hit, format!("{stop:?}"), Act::DjQuick(QuickAct::Press(stop)));
        look(ui, hit, hand, hot, pal);
        let font = face(hand || hot);
        let mid = row.mid();
        let mark = if hand || hot { pal.text } else { pal.dim };
        studio::pointer(ui, pos2(row.right - 3.5, studio::led_mid(ui, mid, &font, false)), mark);
        let end = row.right - VALUE_END;
        if let Some((value, tone)) = value.filter(|(value, _)| !value.is_empty()) {
            let room = end - (from + studio::width_of(ui, label, font.clone()) + VALUE_GAP);
            if room >= 1.0 {
                let parts = studio::figured(value, &[], studio::body(SIZE), studio::mono(FIGURE), ink(tone, pal));
                studio::runs_on(ui, end, row.baseline(ui), &parts, room, Align::Max);
            }
        }
        studio::text(ui, pos2(from, mid), label, font, pal.text, (end - from).max(1.0), Align2::LEFT_CENTER);
    }

    /// The match bar under the switch's name: a quiet minus, a segment for
    /// each cell of the cells' bar, lit in the accent up to the floor and
    /// hot while the keyboard's hand is on it or the pointer over its
    /// segments, and a quiet plus; the ends brighten under the pointer.
    /// The minus steps down a point, the plus up, a segment sets its floor.
    /// The keyboard's hand wears its look round the whole bar.
    fn bar(&mut self, row: Row, bar: &QuickBar) {
        let (ui, pal) = (self.ui, self.pal);
        let mid = row.mid();
        let minus = row.names();
        let first = minus + END_W + SEG_FROM;
        let pitch = SEG_W + SEG_GAP;
        let n = bar.cells;
        let plus = first + f32::from(n) * pitch - SEG_GAP + SEG_TO;
        let hand = self.on(QStop::Bar);
        if hand {
            studio::cursor_look(ui, row.hit(minus, plus + END_W), pal);
        }
        let end_hit = |x: f32| Rect::from_min_max(pos2(x - 4.0, row.rect.min.y), pos2(x + END_W + 4.0, row.rect.max.y));
        let minus_hot = self.control(end_hit(minus), "Step(-1)", Act::DjQuick(QuickAct::Step(-1)));
        let mut over = false;
        for k in 0..n {
            let x = first + f32::from(k) * pitch;
            let hit = Rect::from_min_max(pos2(x - SEG_GAP / 2.0, row.rect.min.y), pos2(x + SEG_W + SEG_GAP / 2.0, row.rect.max.y));
            over |= self.control(hit, ("Set", k), Act::DjQuick(QuickAct::Set(bar.first + u32::from(k) * bar.per)));
        }
        let plus_hot = self.control(end_hit(plus), "Step(1)", Act::DjQuick(QuickAct::Step(1)));
        end_mark(ui, pos2(minus + END_W / 2.0, mid), false, if minus_hot { pal.text } else { pal.dim });
        end_mark(ui, pos2(plus + END_W / 2.0, mid), true, if plus_hot { pal.text } else { pal.dim });
        let lit = if hand || over { pal.accent_hot } else { pal.accent };
        for k in 0..n {
            let seg = Rect::from_min_size(pos2(first + f32::from(k) * pitch, mid - SEG_H / 2.0), vec2(SEG_W, SEG_H));
            ui.painter().rect_filled(seg, CornerRadius::same(1), if k < bar.filled { lit } else { pal.unlit });
        }
    }

    /// The way to the room, the board's amber text button: its words and
    /// the kit's pointer 8 points after them in the accent, which take the
    /// press; the hot accent and the semibold under the pointer, the
    /// semibold under the keyboard's hand.
    fn all(&mut self, row: Row, stop: QStop, label: &str) {
        let (ui, pal) = (self.ui, self.pal);
        let room = (row.right - row.left - LEAD).max(1.0);
        let wide = studio::width_of(ui, label, face(true)).min(room);
        let hit = row.hit(row.left, row.left + wide + LEAD);
        let hand = self.on(stop);
        let hot = self.control(hit, format!("{stop:?}"), Act::DjQuick(QuickAct::Press(stop)));
        look(ui, hit, hand, hot, pal);
        let color = if hot { pal.accent_hot } else { pal.accent };
        let font = face(hot || hand);
        let mid = row.mid();
        let drawn = studio::text(ui, pos2(row.left, mid), label, font.clone(), color, room, Align2::LEFT_CENTER);
        studio::pointer(ui, pos2(drawn.max.x + 8.0 + 2.5, studio::led_mid(ui, mid, &font, false)), color);
    }
}

/// One of the match bar's quiet ends, its 8-point box centred on `at`: a
/// minus, or a plus.
fn end_mark(ui: &Ui, at: Pos2, plus: bool, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    ui.painter().line_segment([at - vec2(3.0, 0.0), at + vec2(3.0, 0.0)], stroke);
    if plus {
        ui.painter().line_segment([at - vec2(0.0, 3.0), at + vec2(0.0, 3.0)], stroke);
    }
}

#[cfg(test)]
mod tests;
