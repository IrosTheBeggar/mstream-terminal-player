//! The Advanced options sheet drawn by egui (contract clauses 33–43): the
//! kit's Dialog over the page's area and its scrim, the page inert beneath,
//! for the board in view.
//!
//! Its face is still to be drawn. What stands here is the sheet's title,
//! from the cells' own key for it (`dev.adv_title`), its close, and the
//! path field while it has the keyboard — the window's field, drawn from
//! the page's own value and caret, its caret handed to the host for the
//! input method — so the sheet can still be closed and typed in: the
//! firmware's, the flash mode's and the erase's choices, the release list,
//! the local build's choosers and what was read of it, the help line, and
//! Use defaults and Apply are the sheet's next face (Advanced.dc.html).

use egui::{Id, Sense, Ui, vec2};

use super::super::*;
use super::{Drawn, Face};
use crate::gui::window::studio::{self, Dialog, FIELD_H};

/// The sheet's window: the canvas's forms'.
const SHEET_W: f32 = 640.0;

/// The Advanced options sheet over `face.full`. Its close asks for the
/// cells' [`Act::SheetClose`]; the path field, while it has the keyboard,
/// draws the input method's composition and gives back its caret.
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(sheet) = page.sheet.as_ref() else { return drawn };
    let title = t!("dev.adv_title", port = short(&sheet.port)).to_string();
    let id = Id::new("mp3-sheet");
    let shown = Dialog::new(id, face.full, SHEET_W, &title).show(ui, face.pal, |ui| {
        let field = sheet.field.as_ref()?;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_H), Sense::hover());
        let input = studio::Input::of(field).mono().focused(true, face.composition);
        studio::field_box(ui, id.with("path"), rect, &input, face.pal).caret
    });
    if shown.closed {
        drawn.acts.push(Act::SheetClose);
    }
    drawn.caret = shown.inner;
    drawn
}
