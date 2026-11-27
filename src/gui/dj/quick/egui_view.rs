//! Quick config's egui face in the GUI's own window, hosted by the chrome
//! (src/gui/chrome.rs): the popover over the room, drawn from quick
//! config's view, its presses the cells' acts.
//!
//! The face is still to be drawn: the QuickConfig board's look (decision
//! D18) is not here yet. What stands here is an honest placeholder: each
//! line's words in the cells' own rows of the popover, a stop's line taking
//! its press, the frame swallowing a press off them and a press anywhere
//! else putting it away, as the cells' Guard and catcher do. [`READY`]
//! stays false until the face is drawn: the cells draw quick config
//! meanwhile.

use egui::{Align2, Id, Sense, Ui};
use ratatui::layout::Rect;

use super::{QStop, QuickAct, QuickLine, QuickView};
use crate::gui::Act;
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Pal};

/// Whether egui draws quick config (the host draws it in cells until it
/// does).
pub(crate) const READY: bool = false;

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
    /// The act of this pass's press: a stop's `DjQuick(Press(stop))`,
    /// nothing on its frame off them, `DjQuick(Close)` anywhere else in
    /// `live`.
    pub act: Option<Act>,
    /// What a press works where, in the order they stack (the catcher
    /// first), for the script's dumps.
    pub items: Vec<(egui::Rect, Act)>,
    /// Its frame grown by its shadow: its part for the host's cut.
    pub frame: Option<egui::Rect>,
}

/// The shadow's reach round the popover.
const SHADOW: f32 = 16.0;

/// Quick config, up: a press on a stop's line is its act, on its frame
/// nothing, anywhere else inside `live` Close. A placeholder: see the
/// module's words.
pub(crate) fn show(ui: &mut Ui, view: &QuickView, cx: &QuickCtx) -> QuickOut {
    let mut out = QuickOut::default();
    let screen = cx.cells.points;
    let frame = cx.cells.rect(view.rect);
    let grown = frame.expand(SHADOW);
    let live = |at: egui::Pos2| cx.live.iter().any(|rect| rect.contains(at));
    let pal = cx.pal;
    let id = Id::new("quick-config");
    let ctx = ui.ctx().clone();
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(screen.min).constrain(false).fade_in(false);
    area.show(&ctx, |ui| {
        let close = Act::DjQuick(QuickAct::Close);
        let catcher = ui.interact(screen, id.with("catcher"), Sense::click());
        // Over the catcher: a press on the frame off the rows is the
        // frame's, and nothing.
        let _ = ui.interact(frame, id.with("frame"), Sense::click());
        out.items.push((screen, close.clone()));
        out.items.push((frame, Act::Guard));
        let pressed_at = ui.input(|i| i.pointer.press_origin());
        if studio::pressed(&catcher) && pressed_at.is_some_and(live) {
            out.act = Some(close);
        }
        ui.set_clip_rect(grown);
        ui.painter().rect_filled(frame, studio::square(), pal.panel);
        ui.painter().rect_stroke(frame, studio::square(), egui::Stroke::new(1.0, pal.accent), egui::StrokeKind::Inside);
        for (i, line) in view.lines.iter().enumerate() {
            let cells = Rect { x: view.rect.x + 2, y: view.rect.y + 1 + i as u16, width: view.rect.width.saturating_sub(4), height: 1 };
            if cells.bottom() >= view.rect.bottom() {
                break;
            }
            let row = cx.cells.rect(cells);
            let (words, stop) = words_of(line);
            let color = if stop.is_some_and(|stop| view.hand == Some(stop)) { pal.accent } else { pal.text };
            studio::text(ui, row.left_center(), &words, studio::body(13.0), color, row.width(), Align2::LEFT_CENTER);
            let Some(stop) = stop else { continue };
            let act = Act::DjQuick(QuickAct::Press(stop));
            out.items.push((row, act.clone()));
            if !live(row.center()) {
                continue;
            }
            let response = studio::hand(ui.interact(row, id.with(("stop", i)), Sense::click()));
            if studio::pressed(&response) {
                out.act = Some(act);
            }
        }
    });
    out.frame = Some(grown);
    out
}

/// A line's words as the placeholder sets them, and the stop a press on it
/// presses.
fn words_of(line: &QuickLine) -> (String, Option<QStop>) {
    match line {
        QuickLine::Head(words) => (words.clone(), None),
        QuickLine::Server { stop, words, current, .. } => (format!("{} {words}", if *current { "(•)" } else { "( )" }), Some(*stop)),
        QuickLine::PlayFrom { stop, label, value } => (format!("{label}  {value} ▸"), Some(*stop)),
        QuickLine::Gap => (String::new(), None),
        QuickLine::Sonic { label, gate: Some(why), .. } => (format!("{label}  {why}"), None),
        QuickLine::Sonic { label, on, value, .. } => (format!("{} {label}  {value}", if *on { "[✓]" } else { "[ ]" }), Some(QStop::Sonic)),
        QuickLine::Bar(bar) => (format!("− {}{} +", "▰".repeat(usize::from(bar.filled)), "▱".repeat(usize::from(bar.cells - bar.filled.min(bar.cells)))), None),
        QuickLine::Seeds { stop, label } => (format!("{label} ▸"), Some(*stop)),
        QuickLine::Smart { label, gate: Some(why), .. } => (format!("{label}  {why}"), None),
        QuickLine::Smart { label, value, .. } => (format!("{label}  {value} ▸"), Some(QStop::Smart)),
        QuickLine::All { stop, label } => (format!("{label} ▸"), Some(*stop)),
    }
}
