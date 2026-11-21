//! The Login screen's body and its skip gate drawn by egui. The body sits
//! inside the frame's column (the frame, egui_view.rs, draws the top strip,
//! the status and tips lines and the bar with Back and Create Admin around
//! it); the gate is a window over the kit's scrim, the page beneath inert.
//!
//! Still to be drawn, after the canvas's Login and LoginSkip boards: each
//! is a placeholder that shows its title in the cells' words, the gate's
//! close doing what the cells' [X] does. The keys answer both as they
//! always have. The body takes the column's `ui`, the gate the root `ui`
//! and `over` (the whole page, the scrim's reach); both take the wizard and
//! the frame's [`Page`], and hand back what their clicks asked for and the
//! caret of their field with the keyboard.

use egui::{Id, Rect, Ui};
use rust_i18n::t;

use super::super::{Act, Wizard};
use super::{Drawn, Page, tip_keyed};
use crate::gui::window::studio::{self, Dialog, strong};

/// The skip gate's window until it is drawn: the kit's gates'.
const GATE_W: f32 = 600.0;

/// The screen's body. Still to be drawn: its title. Its fields, one of
/// which always has the keyboard, are not drawn yet, so it notes no caret.
pub(super) fn show(ui: &mut Ui, _wizard: &mut Wizard, page: &Page) -> Drawn {
    studio::line(ui, 28.0, &t!("login.title"), strong(22.0), page.pal.text);
    Drawn::default()
}

/// The skip gate (`Modal::SkipWarning`). Still to be drawn: its title, and
/// a close that is the cells' [X] (the way back).
pub(super) fn skip_gate(ui: &mut Ui, over: Rect, _wizard: &mut Wizard, page: &Page) -> Drawn {
    let id = Id::new("setup-skip");
    let shown = Dialog::new(id, over, GATE_W, &t!("skip_modal.title")).show(ui, &page.pal, |_| {});
    if let Some(response) = ui.ctx().read_response(id.with("close")) {
        tip_keyed(page, &response, &t!("path_modal.tip_close"));
    }
    Drawn { acts: if shown.closed { vec![Act::SkipCancel] } else { Vec::new() }, caret: None }
}
