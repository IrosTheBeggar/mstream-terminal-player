//! The board's gate drawn by egui (contract clauses 23 and 59): the kit's
//! gold gate over the page's area and its scrim, the page inert beneath, no
//! close — a gate is answered, never dismissed.
//!
//! Its face is still to be drawn. What stands here is the gate's title,
//! from the cells' own words for it (cells.rs's `gate_title`), and its two
//! answers as the kit's gate sets them, so a click can still keep or write:
//! the body (what goes on and how long, what stays, the one danger, the
//! sheet's chosen build), the install's erase box, Update all's boards and
//! the format's words, and the answers in the boards' look, are the gate's
//! next face (UpdateGate.dc.html, Format.dc.html).

use egui::{Id, Ui};

use super::super::cells;
use super::super::*;
use super::{Drawn, Face};
use crate::gui::window::studio;

/// The board's gate over `face.full`: its title and its answers, the safe
/// one first and lit, as the cells' keys answer it (Enter, Esc and `n`
/// keep, `y` writes). A click asks for the cells' act: [`Act::Keep`] or
/// [`Act::Write`].
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(gate) = page.gate.as_ref() else { return drawn };
    let title = cells::gate_title(gate);
    let (keep, write) = cells::gate_buttons(gate);
    let shown = studio::gate(ui, Id::new("mp3-gate"), face.full, &title, Vec::new(), [&keep, &write], face.pal);
    match shown.inner {
        Some(true) => drawn.acts.push(Act::Write),
        Some(false) => drawn.acts.push(Act::Keep),
        None => {}
    }
    drawn
}
