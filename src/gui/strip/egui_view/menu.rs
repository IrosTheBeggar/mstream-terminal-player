//! The server menu's egui face, open over whatever is beneath: its rows
//! from the menu's view, its catcher over the whole window, as the
//! ServerMenu board hangs it (decision D8): the kit's hung list from the
//! strip's foot, its right edge a cell in from the window's, where the
//! cells' border ends; a row a server, grouped as the view orders them, the
//! session's own in amber and the semibold behind the kit's pointer mark,
//! the default's gold star at the row's right, a peer's way in dim after
//! its name, the add row last and dim; the row under the pointer raised,
//! its words in the text colour and the semibold.
//!
//! What a press works is the cells' menu's, on the press: a row switches
//! to its server, the add row opens the add form, and anywhere else the
//! catcher puts the menu away. The keys and the wheel stay the cells': Esc
//! closes it, any other key closes it and goes on, and the wheel anywhere
//! moves the view's first row, which the menu holds to its room and hands
//! back. Past its room the kit's thumb shows where the rows stand, and takes
//! nothing; the gutter it stands in is its row's, as the cells' row runs the
//! menu's whole width.

use egui::{Align2, Color32, FontId, Id, Pos2, Response, Sense, Ui, pos2, vec2};
use ratatui::layout::Rect;

use crate::gui::Act;
use crate::gui::servers::{MenuRow, MenuView};
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Pal, Pick, Run, body, strong};

/// What the menu's face is handed besides the view.
pub(crate) struct MenuCtx<'a> {
    pub cells: Cells,
    /// Where the menu holds the pointer, in points: the window less what
    /// the cells stacked over it (a sheet a right press opened, the quit's
    /// gate). Nothing lights, shows the hand or takes a press outside it.
    pub live: &'a [egui::Rect],
    pub pal: &'a Pal,
}

/// What the menu's face did this pass.
#[derive(Default)]
pub(crate) struct MenuOut {
    /// The act of this pass's press: a row's `SrvSwitch`, the add row's
    /// `SrvAdd`, anywhere else in `live` `SrvCloseDrop`, as the cells'
    /// catcher takes it.
    pub act: Option<Act>,
    /// The first row shown, clamped to the menu's room: the host writes it
    /// back to the wheel's scroll.
    pub scroll: Option<usize>,
    /// What a press works where, in the order they stack (the catcher
    /// first), for the script's dumps.
    pub items: Vec<(egui::Rect, Act)>,
    /// The menu's frame grown by its shadow: its part for the host's cut.
    pub frame: Option<egui::Rect>,
}

/// The rows' words: a text button's size.
const SIZE: f32 = 13.5;
/// The room before every row's words where the session's server wears the
/// pointer mark (a text button's lead), so the names stand in one column.
const LEAD: f32 = 16.0;
/// The default's star (the kit's, [`studio::STAR_R`] twice across), and the
/// air before it.
const STAR: f32 = 2.0 * studio::STAR_R;
const STAR_GAP: f32 = 12.0;
/// The menu's narrowest and widest, in cells, as the cells bound theirs.
const LEAST: f32 = 24.0;
const MOST: f32 = 60.0;

/// The server menu, open over whatever is beneath; every primary press
/// inside `live` is its own: on a row its act, anywhere else the catcher's.
pub(crate) fn menu(ui: &mut Ui, view: &MenuView, cx: &MenuCtx) -> MenuOut {
    let mut out = MenuOut::default();
    let (screen, grid, cell) = (cx.cells.points, cx.cells.grid, cx.cells.cell());
    let lines: Vec<Line> = view.rows.iter().map(Line::server).chain([Line::add(&view.add)]).collect();
    let len = lines.len();
    // From the strip's foot, no taller than the cells' menu may be: the
    // window's rows less eight.
    let top = cx.cells.rect(Rect { x: grid.x, y: grid.y, width: 1, height: 1 }).max.y;
    let bottom = top + f32::from(grid.height.saturating_sub(8)) * cell.y;
    let n = studio::hung_fits(top, bottom).clamp(1, len);
    let first = view.scroll.min(len - n);
    let words_w = lines.iter().map(|line| line.width(ui)).fold(0.0_f32, f32::max);
    let right = cx.cells.rect(Rect { x: (grid.x + grid.width).saturating_sub(1), y: grid.y, width: 1, height: 1 }).min.x;
    let w = studio::hung_width(words_w, n, len).max(LEAST * cell.x).min(MOST * cell.x).min(right - screen.min.x - cell.x).max(1.0);
    let frame = studio::hung_frame(pos2(right - w, top), w, n);
    let grown = studio::lifted_reach(frame);
    // The rows as the kit hangs them: the frame's room round them, each the
    // frame's width less it, the thumb's gutter among it.
    let inset = (frame.height() - n as f32 * studio::HUNG_ROW_H) / 2.0;
    let row_at = |k: usize| {
        let top = frame.min.y + inset + (k - first) as f32 * studio::HUNG_ROW_H;
        egui::Rect::from_min_size(pos2(frame.min.x + inset, top), vec2((frame.width() - 2.0 * inset).max(1.0), studio::HUNG_ROW_H))
    };
    let gutter = if n < len { studio::HUNG_GUTTER } else { 0.0 };
    let live = |at: Pos2| cx.live.iter().any(|rect| rect.contains(at));
    let open = !cx.live.is_empty();
    let act_of = |k: usize| view.rows.get(k).map_or(Act::SrvAdd, |row| Act::SrvSwitch(row.index));
    // A row's id is its server's (the add row's its own): the row the
    // pointer is on stays it as the wheel moves the view.
    let id_of = |k: usize| view.rows.get(k).map_or(Id::new("srv-add-row"), |row| Id::new(("srv-row", row.index)));
    let id = Id::new("server-menu");
    let ctx = ui.ctx().clone();
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(screen.min).constrain(false).fade_in(false);
    area.show(&ctx, |ui| {
        // Where the press came down, a tap's among them.
        let origin = studio::pass_press(ui);
        let took = |response: &Response| studio::pressed(response) && origin.is_some_and(live);
        // The catcher first, under the rows: a press off them closes it,
        // and the hand shows over it, as over the cells' catcher.
        let catcher = ui.interact(screen, id.with("catcher"), Sense::click());
        let catcher = if open { studio::hand(catcher) } else { catcher };
        out.items.push((screen, Act::SrvCloseDrop));
        if took(&catcher) {
            out.act = Some(Act::SrvCloseDrop);
        }
        ui.set_clip_rect(grown);
        // The gutter beside a row is the row's.
        let mut beside = vec![false; len];
        if gutter > 0.0 && open {
            for k in first..first + n {
                let row = row_at(k);
                let at = egui::Rect::from_min_max(pos2(row.max.x - gutter, row.min.y), row.max);
                let response = studio::hand(ui.interact(at, id_of(k).with("gutter"), Sense::click()));
                beside[k] = response.hovered();
                if took(&response) {
                    out.act = Some(act_of(k));
                }
            }
        }
        // A row lights as the kit raises it: by its own response, which
        // the kit made before it paints the row's words.
        let paint = |ui: &Ui, k: usize, room: egui::Rect, _cursor: bool| {
            if beside[k] {
                studio::raised_row(ui, room.expand2(vec2(studio::HUNG_PAD, 0.0)), cx.pal);
            }
            let hovered = ui.ctx().read_response(id_of(k)).is_some_and(|response| response.hovered());
            lines[k].paint(ui, room, open && (beside[k] || hovered), cx.pal);
        };
        let ids = |k: usize| open.then(|| id_of(k));
        let picked = studio::hung_list_by(ui, frame, first, n, len, None, ids, Pick::Press, cx.pal, paint);
        if let Some(k) = picked.filter(|_| origin.is_some_and(live)) {
            out.act = Some(act_of(k));
        }
    });
    out.items.extend((first..first + n).map(|k| (row_at(k), act_of(k))));
    out.scroll = Some(first);
    out.frame = Some(grown);
    out
}

/// A row's words as the menu sets them: the name (a peer's behind its
/// branch) and a peer's tail after it, the way it is reached; whether it is
/// the session's server, the default, or the add row.
struct Line {
    name: String,
    tail: String,
    current: bool,
    star: bool,
    add: bool,
}

impl Line {
    /// A server's row, worded as the cells word theirs: a peer's "└ name",
    /// then " · direct · via parent", its "direct" first so a cut row still
    /// shows it.
    fn server(row: &MenuRow) -> Line {
        let (name, tail) = match &row.peer {
            Some(peer) => {
                let direct = peer.direct.as_ref().map(|d| format!("{d} · ")).unwrap_or_default();
                (format!("└ {}", row.name), format!(" · {direct}{}", peer.via))
            }
            None => (row.name.clone(), String::new()),
        };
        Line { name, tail, current: row.current, star: row.default, add: false }
    }

    /// The add row, "+ " before its words as the cells set them.
    fn add(words: &str) -> Line {
        Line { name: format!("+ {words}"), tail: String::new(), current: false, star: false, add: true }
    }

    /// The row's words as runs: the name in `font` and `ink`, the tail
    /// always dim.
    fn runs(&self, font: FontId, ink: Color32, dim: Color32) -> Vec<Run> {
        vec![(self.name.clone(), font, ink), (self.tail.clone(), body(SIZE), dim)]
    }

    /// How wide the row's words lay out at their widest, in the semibold
    /// they wear under the pointer, with the lead and the star's room.
    fn width(&self, ui: &Ui) -> f32 {
        let star = if self.star { STAR_GAP + STAR } else { 0.0 };
        LEAD + studio::runs_width(ui, &self.runs(strong(SIZE), Color32::WHITE, Color32::WHITE)) + star
    }

    /// The row's words in `room`, the row less its padding: the session's
    /// server in amber and the semibold with the pointer mark in the lead,
    /// whatever the pointer does; another server in the text colour, in the
    /// semibold while `hot`; the add row dim, in the text colour and the
    /// semibold while `hot`; the tail dim; cut with an ellipsis before the
    /// default's star.
    fn paint(&self, ui: &Ui, room: egui::Rect, hot: bool, pal: &Pal) {
        let (font, ink) = match (self.current, self.add, hot) {
            (true, ..) => (strong(SIZE), pal.accent),
            (false, _, true) => (strong(SIZE), pal.text),
            (false, true, false) => (body(SIZE), pal.dim),
            (false, false, false) => (body(SIZE), pal.text),
        };
        let mid = room.center().y;
        if self.current {
            studio::pointer(ui, pos2(room.min.x + 2.5, studio::led_mid(ui, mid, &font, false)), ink);
        }
        let star = if self.star { STAR_GAP + STAR } else { 0.0 };
        let x = room.min.x + LEAD;
        studio::runs(ui, pos2(x, mid), &self.runs(font.clone(), ink, pal.dim), (room.max.x - star - x).max(1.0), Align2::LEFT_CENTER);
        if self.star {
            // The cells' "★", the kit's star whole, on the capitals'
            // middle as the bar's rating stands its stars.
            studio::star(ui, pos2(room.max.x - STAR / 2.0, studio::led_mid(ui, mid, &font, true)), 1.0, pal.gold);
        }
    }
}
