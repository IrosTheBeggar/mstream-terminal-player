//! The top strip's egui face in the GUI's own window, hosted by the
//! chrome (src/gui/chrome.rs) in row 0's cells: the tabs, the Visualizer
//! item, the server label and [+], their two tooltips, and the server menu
//! (`menu`), drawn from the strip's view and the menu's, their presses the
//! cells' acts.
//!
//! The face is still to be drawn: the Studio look of the canvas's TopStates,
//! StripHeight, TopTips, TopLocales and ServerMenu boards (decisions D1-D10)
//! is not here yet. What stands here is an honest placeholder, every item's
//! words as plain labels in the cells' own slots, each taking the press the
//! way the cells do, so the hosting runs whole and its tests have something
//! to press. [`READY`] stays false until the face is drawn: the cells draw
//! the strip and the menu in the window meanwhile.

use std::time::Duration;

use egui::{Align2, Color32, Id, Response, Sense, Ui};
use ratatui::layout::Rect;

use super::{StripView, header_label, top_rects, top_strip_end_with};
use crate::gui::Act;
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Icon, Pal};

mod menu;

pub(crate) use menu::{MenuCtx, menu};

/// Whether egui draws the strip, its tooltips and the server menu (the
/// host draws the cells strip until it does).
pub(crate) const READY: bool = false;

/// What the strip's face is handed besides the view.
pub(crate) struct StripCtx<'a> {
    /// Where the grid's cells stand in points.
    pub cells: Cells,
    /// Where a press on row 0 would act this frame, in points: the cells
    /// the registry left the strip. Empty while it is inert (an MP3 write,
    /// a layer that dropped the cells' registries). Nothing lights, shows
    /// the hand or takes a press outside it.
    pub live: &'a [egui::Rect],
    /// The pointer stood as the cells would have drawn the strip (no modal
    /// stashed it), and no popup holds it: hover may light, a tooltip may
    /// show.
    pub hover: bool,
    /// The time since the last key, which hides a tooltip for the dwell.
    pub since_key: Option<Duration>,
    pub pal: &'a Pal,
}

/// What the strip's face did this pass.
#[derive(Default)]
pub(crate) struct StripOut {
    /// The act of this pass's press, where it pressed a strip item: taken
    /// on the press, as the cells take it.
    pub act: Option<Act>,
    /// Every item where it stands and what a press on it does, for the
    /// script's dumps.
    pub items: Vec<(egui::Rect, Act)>,
    /// The tooltip's box, while one shows: it cuts no hole and takes none.
    pub tip: Option<egui::Rect>,
}

/// The strip in row 0, on `ui` (its child over row 0, clipped to it, the
/// ground painted). A placeholder: see the module's words.
pub(crate) fn show(ui: &mut Ui, view: &StripView, cx: &StripCtx) -> StripOut {
    let mut out = StripOut::default();
    let mp3 = match &view.mark {
        Some(mark) => format!("{} {}", view.tabs[3].label, mark.words()),
        None => view.tabs[3].label.clone(),
    };
    let rects = top_rects([&view.tabs[0].label, &view.tabs[1].label, &view.tabs[2].label, &mp3, &view.viz.label]);
    for (tab, rect) in view.tabs.iter().zip(rects) {
        let words = if tab.screen == crate::gui::Screen::Device { &mp3 } else { &tab.label };
        let color = if tab.up { cx.pal.text } else { cx.pal.dim };
        let id = Id::new(("strip-tab", format!("{:?}", tab.screen)));
        item(ui, cx, &mut out, rect, words, color, id, Act::Screen(tab.screen), view.live);
    }
    let viz = rects[4];
    let color = if view.viz.lit { cx.pal.accent } else { cx.pal.dim };
    if let Some(response) = item(ui, cx, &mut out, viz, &view.viz.label, color, Id::new("strip-viz"), Act::VizWindow, view.live) {
        let tip = if view.key_hints { view.viz.tip.as_str() } else { view.viz.tip.rsplit_once(" — ").map_or(view.viz.tip.as_str(), |(words, _)| words) };
        tip_of(&mut out, &response, tip, cx);
    }
    let width = cx.cells.grid.width;
    let plus = Rect { x: width.saturating_sub(5), y: 0, width: 3, height: 1 };
    let at = cx.cells.rect(plus);
    studio::icon(ui, at.center(), Icon::Add, cx.pal.dim);
    if let Some(response) = take(ui, cx, &mut out, at, Id::new("server-add"), Act::SrvAdd, true) {
        tip_of(&mut out, &response, &view.add_tip, cx);
    }
    if let Some(server) = &view.server {
        let mark = if server.menu { " ▾" } else { "" };
        let mark_words = view.mark.map(|mark| mark.words()).unwrap_or_default();
        if let Some((x, label)) = header_label(&server.name, mark, top_strip_end_with(&mark_words), plus.x) {
            let rect = Rect { x, y: 0, width: crate::kit::width(&label) as u16, height: 1 };
            let at = cx.cells.rect(rect);
            studio::text(ui, at.left_center(), &label, studio::body(13.0), cx.pal.dim, at.width(), Align2::LEFT_CENTER);
            if server.menu {
                take(ui, cx, &mut out, at, Id::new("server-label"), Act::SrvMenu, true);
            }
        }
    }
    out
}

/// A labelled item in its cells: its words centred in them, and the press
/// it takes ([`take`]).
#[allow(clippy::too_many_arguments)]
fn item(ui: &Ui, cx: &StripCtx, out: &mut StripOut, cells: Rect, words: &str, color: Color32, id: Id, act: Act, live: bool) -> Option<Response> {
    let at = cx.cells.rect(cells);
    studio::text(ui, at.center(), words, studio::body(13.0), color, at.width(), Align2::CENTER_CENTER);
    take(ui, cx, out, at, id, act, live)
}

/// What a press on `at` does, where the strip is live there: taken on the
/// press, the hand over it; listed for the dumps either way.
#[allow(clippy::too_many_arguments)]
fn take(ui: &Ui, cx: &StripCtx, out: &mut StripOut, at: egui::Rect, id: Id, act: Act, live: bool) -> Option<Response> {
    let live = live && cx.live.iter().any(|rect| rect.contains(at.center()));
    if !live {
        return None;
    }
    out.items.push((at, act.clone()));
    let response = studio::hand(ui.interact(at, id, Sense::click()));
    if studio::pressed(&response) {
        out.act = Some(act);
    }
    Some(response)
}

/// The tooltip of an item, while the pointer may ripen one.
fn tip_of(out: &mut StripOut, response: &Response, words: &str, cx: &StripCtx) {
    if cx.hover && out.tip.is_none() {
        out.tip = studio::tip_under(response, words, cx.since_key, cx.pal);
    }
}
