//! The Extras screen's body drawn by egui, inside the frame's column (the
//! frame, egui_view.rs, draws the top strip, the status and tips lines and
//! the bar with Back and Continue around it).
//!
//! Still to be drawn, after the canvas's Extras board: a placeholder that
//! shows the screen's title in the cells' words. The keys answer the screen
//! as they always have. It takes the column's `ui`, the wizard and the
//! frame's [`Page`], and hands back what its clicks asked for.

use egui::Ui;
use rust_i18n::t;

use super::super::Wizard;
use super::{Drawn, Page};
use crate::gui::window::studio::{self, strong};

/// The screen's body. Still to be drawn: its title.
pub(super) fn show(ui: &mut Ui, _wizard: &mut Wizard, page: &Page) -> Drawn {
    studio::line(ui, 28.0, &t!("extras.title"), strong(22.0), page.pal.text);
    Drawn::default()
}
