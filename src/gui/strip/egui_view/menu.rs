//! The server menu's egui face, open over whatever is beneath: its rows
//! from the menu's view, its catcher over the whole window. Still to be
//! drawn: the ServerMenu board's look (decision D8) is not here yet, only
//! a placeholder hung as the kit hangs a list, picking on the press as the
//! cells' menu picks.

use egui::{Align2, Id, Sense, Ui, pos2};

use crate::gui::Act;
use crate::gui::servers::MenuView;
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Pal, Pick};

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

/// The shadow's reach round a hung list's frame.
const SHADOW: f32 = 16.0;

/// The server menu, open over whatever is beneath; every primary press
/// inside `live` is its own. A placeholder: see the module's words.
pub(crate) fn menu(ui: &mut Ui, view: &MenuView, cx: &MenuCtx) -> MenuOut {
    let mut out = MenuOut::default();
    let screen = cx.cells.points;
    let cell = cx.cells.cell();
    let len = view.rows.len() + 1;
    let top = screen.min.y + cell.y;
    let n = studio::hung_fits(top, screen.max.y - 8.0 * cell.y).clamp(1, len);
    let first = view.scroll.min(len - n);
    let words = |k: usize| match view.rows.get(k) {
        Some(row) => row.name.clone(),
        None => format!("+ {}", view.add),
    };
    let font = studio::body(13.0);
    let words_w = (0..len).map(|k| studio::width_of(ui, &words(k), font.clone())).fold(0.0_f32, f32::max);
    let w = studio::hung_width(words_w, n, len).min(screen.width() - 16.0).max(1.0);
    let frame = studio::hung_frame(pos2(screen.max.x - 8.0 - w, top), w, n);
    let grown = frame.expand(SHADOW);
    let live = |at: egui::Pos2| cx.live.iter().any(|rect| rect.contains(at));
    let act_of = |k: usize| match view.rows.get(k) {
        Some(row) => Act::SrvSwitch(row.index),
        None => Act::SrvAdd,
    };
    let id = Id::new("server-menu");
    let ctx = ui.ctx().clone();
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(screen.min).constrain(false).fade_in(false);
    area.show(&ctx, |ui| {
        // The catcher first, under the rows: a press off them closes it.
        let catcher = ui.interact(screen, id.with("catcher"), Sense::click());
        out.items.push((screen, Act::SrvCloseDrop));
        let pressed_at = ui.input(|i| i.pointer.press_origin());
        if studio::pressed(&catcher) && pressed_at.is_some_and(live) {
            out.act = Some(Act::SrvCloseDrop);
        }
        ui.set_clip_rect(grown);
        let open = live(frame.center());
        let id_of = |k: usize| open.then(|| Id::new(("srv-row", k)));
        let paint = |ui: &Ui, k: usize, room: egui::Rect, _on: bool| {
            studio::text(ui, room.left_center(), &words(k), font.clone(), cx.pal.text, room.width(), Align2::LEFT_CENTER);
        };
        if let Some(k) = studio::hung_list_by(ui, frame, first, n, len, None, id_of, Pick::Press, cx.pal, paint) {
            out.act = Some(act_of(k));
        }
    });
    for k in first..first + n {
        let row = egui::Rect::from_min_size(pos2(frame.min.x, frame.min.y + 2.0 + (k - first) as f32 * studio::HUNG_ROW_H), egui::vec2(frame.width(), studio::HUNG_ROW_H));
        out.items.push((row, act_of(k)));
    }
    out.scroll = Some(first);
    out.frame = Some(grown);
    out
}
