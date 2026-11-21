//! The Extras screen's body drawn by egui, after the canvas's Extras board,
//! inside the frame's column (the frame, egui_view.rs, draws the top strip,
//! the status and tips lines and the bar with Back, Continue and the scan
//! around it): the heading, then the four opt-ins as the kit's tick cards
//! in the cells' order, each card's box carrying its flag.
//!
//! A click on a card flips it (Toggle), the act the cells register for the
//! card (`draw_extras`). The keyboard's cursor over the cards
//! (`extras_sel`) is the keys' alone, as in the cells: absent until ↑ or ↓,
//! moved by no click, never egui's focus. Its card wears the kit's
//! keyboard look, and is brought into view on the pass the cursor moves to
//! it where the column is too short for the four (the kit's scrolled body,
//! which Login's form shares).

use egui::{Id, Rangef, Ui};
use rust_i18n::t;

use super::super::{Act, Wizard};
use super::{Drawn, Page};
use crate::gui::window::studio::{self, HEADING_H};

/// A card, the kit's tick card's 60 points, and the boards' 8 between two.
const CARD_H: f32 = 60.0;
const CARD_GAP: f32 = 8.0;
/// The body's height: the heading and the four cards.
const BODY_H: f32 = HEADING_H + 4.0 * CARD_H + 3.0 * CARD_GAP;

/// The body over the whole of `ui`, the column: the kit's heading, then the
/// four opt-ins as the kit's tick cards, the column wide and 8 points
/// apart, each flipped by a click, the keyboard's cursor on one of them in
/// the kit's keyboard look, which wins over the pointer's. Nothing here has
/// a field, so it notes no caret.
pub(super) fn show(ui: &mut Ui, wizard: &mut Wizard, page: &Page) -> Drawn {
    let pal = &page.pal;
    let cards = [
        (t!("extras.updates_label"), t!("extras.updates_desc")),
        (t!("extras.discovery_label"), t!("extras.discovery_desc")),
        (t!("extras.federation_label"), t!("extras.federation_desc")),
        (t!("extras.audio_label"), t!("extras.audio_desc")),
    ];
    let cursor = wizard.extras_sel;
    let keep = cursor.map(|i| (i, Rangef::new(card_top(i), card_top(i) + CARD_H)));
    let acts = studio::scrolled(ui, Id::new("setup-extras"), BODY_H, keep, |ui| {
        studio::heading(ui, &t!("extras.title"), &t!("extras.subtitle"), pal);
        let mut acts = Vec::new();
        for (i, ((label, hint), on)) in cards.iter().zip(wizard.extras).enumerate() {
            if i > 0 {
                ui.add_space(CARD_GAP);
            }
            if studio::tick_card(ui, Id::new("setup-extra").with(i), on, label, hint, cursor == Some(i), pal).clicked() {
                acts.push(Act::Toggle(i));
            }
        }
        acts
    });
    Drawn { acts, caret: None }
}

/// Where card `i` starts down the body.
fn card_top(i: usize) -> f32 {
    HEADING_H + i as f32 * (CARD_H + CARD_GAP)
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{FullOutput, Id, Pos2, Rect, vec2};
    use ratatui::crossterm::event::KeyCode;
    use rust_i18n::t;

    use super::super::tests::{BOARD, Probe, color_of, drawn, has, key, wizard};
    use super::super::{FLOOR, parts};
    use crate::gui::window::studio::{BAR_ROOM, Pal};
    use crate::setup::cells::footer_hint;
    use crate::setup::tests::LOCALE_LOCK;
    use crate::setup::{Screen, Wizard};

    fn card(i: usize) -> Id {
        Id::new("setup-extra").with(i)
    }

    fn extras() -> Wizard {
        let mut wizard = wizard();
        wizard.screen = Screen::Extras;
        wizard
    }

    /// The tick boxes drawn with their flag on: a box's ok-green edge.
    fn ticked(out: &FullOutput, pal: &Pal) -> usize {
        out.shapes
            .iter()
            .filter(|c| match &c.shape {
                Shape::Rect(r) => r.stroke.color == pal.ok && (r.rect.width() - 16.0).abs() < 0.01 && (r.rect.height() - 16.0).abs() < 0.01,
                _ => false,
            })
            .count()
    }

    /// Whether `rect` wears the keyboard's ring: the accent's hairline round it.
    fn ringed(out: &FullOutput, rect: Rect, pal: &Pal) -> bool {
        out.shapes.iter().any(|c| match &c.shape {
            Shape::Rect(r) => r.stroke.color == pal.accent && r.rect == rect,
            _ => false,
        })
    }

    #[test]
    fn the_four_opt_ins_draw_in_the_cells_words_and_their_boxes_carry_their_flags() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = extras();
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let keys = [
            "extras.title", "extras.subtitle", "extras.updates_label", "extras.updates_desc", "extras.discovery_label", "extras.discovery_desc",
            "extras.federation_label", "extras.federation_desc", "extras.audio_label", "extras.audio_desc",
        ];
        for key in keys {
            assert!(has(&words, &t!(key)), "{key} drawn: {words:?}");
        }
        assert!(has(&words, &t!("hint.extras")), "no cursor yet: {words:?}");
        assert_eq!(color_of(&out, &t!("extras.subtitle")), Some(pal.dim));
        assert_eq!(color_of(&out, &t!("extras.audio_desc")), Some(pal.dim));
        assert_eq!(ticked(&out, &pal), 1, "the defaults: updates on, the rest off");
        assert!(shown.caret.is_none() && shown.outcome.is_none(), "no field, nothing ended");
        // Where the board stands them: the column wide from y 140, 60 tall
        // and 8 apart.
        for i in 0..4 {
            let at = probe.rect(card(i));
            assert_eq!(at, Rect::from_min_size(Pos2::new(280.0, 140.0 + i as f32 * 68.0), vec2(720.0, 60.0)), "card {i}");
        }
        wizard.extras = [true, false, true, true];
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(ticked(&out, &pal), 3);
    }

    #[test]
    fn a_click_flips_its_card_and_never_moves_the_keyboards_cursor() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = extras();
        let _ = probe.settle(&mut wizard);
        let audio = probe.rect(card(3)).center();
        let _ = probe.pass(&mut wizard, vec![egui::Event::PointerMoved(audio)]);
        assert!(probe.pass(&mut wizard, Vec::new()).1.hand, "the hand over a card");
        let shown = probe.click(&mut wizard, card(3));
        assert!(shown.outcome.is_none());
        assert_eq!(wizard.extras, [true, false, false, true], "Server-side audio on");
        assert_eq!(wizard.extras_sel, None, "and no cursor picked up");
        assert!(wizard.queued.is_none() && wizard.note.is_none(), "nothing sent before Continue");
        probe.click(&mut wizard, card(0));
        assert_eq!(wizard.extras, [false, false, false, true], "the default-on updates off");
        // With the keys' cursor out, a click elsewhere flips that card and
        // leaves the cursor where it was.
        key(&mut wizard, KeyCode::Down);
        key(&mut wizard, KeyCode::Down);
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, card(3));
        assert_eq!((wizard.extras, wizard.extras_sel), ([false, false, false, false], Some(1)));
        assert!(probe.ctx.memory(|m| m.focused()).is_none(), "the cursor is never egui's focus");
    }

    #[test]
    fn the_keyboards_cursor_wears_the_ring_and_the_tips_line_says_so() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = extras();
        let (out, _) = probe.settle(&mut wizard);
        assert!((0..4).all(|i| !ringed(&out, probe.rect(card(i)), &pal)), "no ring before an arrow");
        // ↑ first picks up the last card, as the cells' keys do.
        key(&mut wizard, KeyCode::Up);
        let (out, _) = probe.settle(&mut wizard);
        assert!(ringed(&out, probe.rect(card(3)), &pal), "the cursor's card ringed");
        assert!((0..3).all(|i| !ringed(&out, probe.rect(card(i)), &pal)), "and it alone");
        let hint = footer_hint(&wizard);
        assert_eq!(hint, t!("hint.extras_selected"));
        assert!(has(&drawn(&out), &hint));
        // Space flips the cursor's card through the model's keys.
        key(&mut wizard, KeyCode::Char(' '));
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(ticked(&out, &pal), 2, "updates and audio");
        // The pointer on the cursor's card leaves the ring on it.
        let at = probe.rect(card(3)).center();
        let _ = probe.pass(&mut wizard, vec![egui::Event::PointerMoved(at)]);
        let (out, _) = probe.pass(&mut wizard, Vec::new());
        assert!(ringed(&out, probe.rect(card(3)), &pal), "the cursor's look wins over the pointer's");
    }

    #[test]
    fn a_short_column_scrolls_and_brings_the_cursors_card_into_view() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        // The window's own floor: the four cards are taller than its column.
        let probe = Probe::new(FLOOR);
        let column = parts(Rect::from_min_size(Pos2::ZERO, FLOOR)).column;
        let mut wizard = extras();
        let _ = probe.settle(&mut wizard);
        assert!(probe.rect(card(0)).min.y >= column.min.y - 0.5, "from the body's top");
        let last = probe.ctx.read_response(card(3)).map(|r| r.interact_rect);
        assert!(last.is_none_or(|r| r.height() < 1.0), "the last card below the fold: {last:?}");
        // The cards leave the thumb its room.
        assert!(probe.rect(card(0)).max.x <= column.max.x - BAR_ROOM + 0.5);
        key(&mut wizard, KeyCode::Up);
        let _ = probe.settle(&mut wizard);
        let at = probe.rect(card(3));
        assert!(at.min.y >= column.min.y - 0.5 && at.max.y <= column.max.y + 0.5, "the cursor's card whole in view: {at:?} in {column:?}");
        // A click on it flips it where it now stands.
        probe.click(&mut wizard, card(3));
        assert!(wizard.extras[3]);
        // Back up to the first card: in view again.
        for _ in 0..3 {
            key(&mut wizard, KeyCode::Up);
        }
        let _ = probe.settle(&mut wizard);
        let at = probe.rect(card(0));
        assert!(at.min.y >= column.min.y - 0.5 && at.max.y <= column.max.y + 0.5, "{at:?} in {column:?}");
    }
}
