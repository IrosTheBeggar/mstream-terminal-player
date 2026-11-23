//! A card's Settings sheet drawn by egui (contract clause 48): the kit's
//! Dialog over the page's area and its scrim, the page inert beneath, for
//! the card it was opened for.
//!
//! Its face is still to be drawn. What stands here is the sheet's title,
//! from the cells' own keys for it (`dev.card.setup_title` for a new card,
//! `dev.card.set_title` for its settings), its close, and the Name field
//! while it has the keyboard — drawn from the page's own value and caret,
//! its caret handed to the host for the input method — so the sheet can
//! still be closed and named in: its tabs, its rows, the lists that hang
//! open in it (From, Into), the fit, and Use defaults or Cancel and Apply
//! or Set up are the sheet's next face (Settings.dc.html).

use egui::{Id, Sense, Ui, vec2};

use super::super::card::card_name;
use super::super::*;
use super::{Drawn, Face};
use crate::gui::window::studio::{self, Dialog, FIELD_H};

/// The sheet's window: the canvas's forms'.
const SHEET_W: f32 = 640.0;

/// The card's Settings sheet over `face.full`. Its close asks for the
/// cells' [`Act::CardSheetClose`]; the Name field, while it has the
/// keyboard, draws the input method's composition and gives back its caret.
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(sheet) = page.card.sheet.as_ref() else { return drawn };
    let Some(card) = page.card_of(&sheet.key) else { return drawn };
    let title = if sheet.setup {
        t!("dev.card.setup_title", drive = card.drive()).to_string()
    } else {
        t!("dev.card.set_title", card = card_name(&card)).to_string()
    };
    let id = Id::new("mp3-card-sheet");
    let shown = Dialog::new(id, face.full, SHEET_W, &title).show(ui, face.pal, |ui| {
        let field = sheet.field.as_ref()?;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_H), Sense::hover());
        let input = studio::Input::of(field).focused(true, face.composition);
        studio::field_box(ui, id.with("name"), rect, &input, face.pal).caret
    });
    if shown.closed {
        drawn.acts.push(Act::CardSheetClose);
    }
    drawn.caret = shown.inner;
    drawn
}
