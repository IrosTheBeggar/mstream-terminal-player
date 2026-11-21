//! The Done page drawn by egui, the wizard's last step and the Quick
//! Connect page alike: its whole page, since it owns its frame (no rule and
//! no bar; two columns where the cells draw pictures). The frame's own
//! pieces are there to share (egui_view.rs): where each part stands
//! (`parts`), the top strip with the step (`strip`), the status line and
//! the tips line.
//!
//! Still to be drawn, after the canvas's Done, DoneRest and QuickConnect
//! boards: a placeholder that shows the top strip, the page's heading in
//! the cells' words, and the status and tips lines. The keys answer the
//! page as they always have (f, q and Esc end it; there is no Finish to
//! click, as in the cells). It takes the root `ui`, the wizard and the
//! frame's [`Page`], and hands back what its clicks asked for.

use egui::{Align2, Ui, pos2};
use rust_i18n::t;

use super::super::Wizard;
use super::{Drawn, Page, parts, status_line, strip, tips_line};
use crate::gui::window::studio::{self, strong};

/// The whole page. Still to be drawn: the strip, the heading, the status
/// and tips lines.
pub(super) fn show(ui: &mut Ui, wizard: &mut Wizard, page: &Page) -> Drawn {
    let at = parts(ui.max_rect());
    let mut drawn = Drawn::default();
    strip(ui, at.strip, wizard, page, &mut drawn.acts);
    let heading = if wizard.standalone { t!("done.heading_qr") } else { t!("done.heading_wizard") };
    let top = pos2(at.column.min.x, at.column.min.y + 14.0);
    studio::text(ui, top, &heading, strong(22.0), page.pal.text, at.column.width(), Align2::LEFT_CENTER);
    status_line(ui, at.status, wizard, &page.pal);
    tips_line(ui, at.tips, wizard, &page.pal);
    drawn
}
