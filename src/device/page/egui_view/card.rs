//! An SD card in this computer's reader drawn by egui (contract clauses
//! 20a, 25, 44–53 and 58): its Card view in the column, as the board's card
//! stands there, and its gates over the page's area and the kit's scrim.
//!
//! Their faces are still to be drawn. What stands here is the Card view's
//! hairline frame with its head, the card's name (`card_name`, the cells'
//! words), and a gate's title and answers from the cells' own words for
//! them (cells/card.rs's `gate_title` and `gate_buttons_of`): the view's
//! rows (Holds, Space, Last sync, Next sync, Converts, the primary and
//! Details; cells/card.rs's `card_layout` makes them as `Row`s, which
//! board.rs's `Paint` sets in points), the stronger gate's box and its
//! second button, and the chained gate's guard are the card's next face
//! (Card.dc.html, Syncing.dc.html, Synced.dc.html, RemoveGate.dc.html).

use egui::{Id, Rect, Ui, pos2};

use super::super::card::card_name;
use super::super::cells;
use super::super::*;
use super::board::{Grid, clipped, hairline, head};
use super::{Drawn, Face};
use crate::device::card::worker::CardView;
use crate::gui::window::studio::{self, Dialog};

/// The gates' window: the kit's gate's.
const GATE_W: f32 = 600.0;

/// The card `view` in the column's `room`: its frame and its head. Its
/// rest is its height, as the frame measures it.
pub(super) fn show(ui: &mut Ui, _page: &mut Page, face: &Face, view: &CardView, room: Rect) -> Drawn {
    let s = face.space;
    let grid = Grid::of(room, face);
    let card = clipped(ui, room, "mp3-card");
    let y = head(&card, face, grid, room.min.y + s.pad_y, &card_name(view), "");
    let rest = y + s.pad_y - room.min.y;
    let bottom = (room.min.y + rest).min(room.max.y);
    hairline(&card, face, Rect::from_min_max(room.min, pos2(room.max.x, bottom)));
    Drawn { rest, ..Drawn::default() }
}

/// A card's gate over `face.full`: its title and its answers, the safe one
/// first and lit, the cells' [`Act::CardKeep`] and [`Act::CardGo`]; while
/// its go waits on its box (the stronger gate), the keep alone.
pub(super) fn gate(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(gate) = page.card.gate.as_ref() else { return drawn };
    let title = cells::card::gate_title(gate);
    let (keep, go) = cells::card::gate_buttons_of(gate);
    let id = Id::new("mp3-card-gate");
    let answer = match go {
        Some(go) => studio::gate(ui, id, face.full, &title, Vec::new(), [&keep, &go], face.pal).inner,
        None => {
            let shown = Dialog::new(id, face.full, GATE_W, &title).show(ui, face.pal, |ui| {
                studio::foot(ui, id.with("foot"), &[], &[(&keep, true)], None, face.pal)
            });
            (shown.closed || shown.inner.is_some()).then_some(false)
        }
    };
    match answer {
        Some(true) => drawn.acts.push(Act::CardGo),
        Some(false) => drawn.acts.push(Act::CardKeep),
        None => {}
    }
    drawn
}
