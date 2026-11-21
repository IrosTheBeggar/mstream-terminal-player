//! The Extras screen's body drawn by egui, after the canvas's Extras board,
//! inside the frame's column (the frame, egui_view.rs, draws the top strip,
//! the status and tips lines and the bar with Back, Continue and the scan
//! around it): the heading, then the four opt-ins as the kit's tick cards
//! in the cells' order, each card's box carrying its flag.
//!
//! A click on a card flips it (Toggle), the act the cells register for the
//! card (`draw_extras`). The keyboard's cursor over the cards
//! (`extras_sel`) is the keys' alone, as in the cells: absent until ↑ or ↓,
//! moved by no click, never egui's focus. Its card wears the kit's ring, and
//! is brought into view on the pass the cursor moves to it where the column
//! is too short for the four.
//!
//! Extras and Login lay out alike, a heading over controls of fixed heights
//! in a column that may be too short for them: the two share the heading
//! and the scrolled body, which live here, Extras being the first of them.

use egui::epaint::StrokeKind;
use egui::{Align2, Id, Rangef, Rect, Response, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use rust_i18n::t;

use super::super::{Act, Wizard};
use super::{Drawn, Page};
use crate::gui::window::studio::{self, BAR_ROOM, BOX, Pal, body, strong};

/// The heading's two lines and the room under them, the boards': the
/// title's 28-point row, 6, the subtitle's 18-point row, and 24 to the
/// first control.
const TITLE_H: f32 = 28.0;
const SUB_GAP: f32 = 6.0;
const SUB_H: f32 = 18.0;
const HEAD_GAP: f32 = 24.0;
/// How far down the body its first control stands: the heading's height.
pub(super) const HEAD: f32 = TITLE_H + SUB_GAP + SUB_H + HEAD_GAP;
/// A card, the kit's tick card's 60 points, and the boards' 8 between two.
const CARD_H: f32 = 60.0;
const CARD_GAP: f32 = 8.0;
/// The body's height: the heading and the four cards.
const BODY_H: f32 = HEAD + 4.0 * CARD_H + 3.0 * CARD_GAP;

/// The body over the whole of `ui`, the column: the heading, then the four
/// opt-ins, the column wide and 8 points apart, each flipped by a click,
/// the keyboard's cursor on one of them in the kit's ring. Nothing here has
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
    let acts = scrolled(ui, Id::new("setup-extras"), BODY_H, keep, |ui| {
        heading(ui, &t!("extras.title"), &t!("extras.subtitle"), pal);
        let mut acts = Vec::new();
        for (i, ((label, hint), on)) in cards.iter().zip(wizard.extras).enumerate() {
            if i > 0 {
                ui.add_space(CARD_GAP);
            }
            if tick_card(ui, Id::new("setup-extra").with(i), on, label, hint, cursor == Some(i), pal).clicked() {
                acts.push(Act::Toggle(i));
            }
        }
        acts
    });
    Drawn { acts, caret: None }
}

/// Where card `i` starts down the body.
fn card_top(i: usize) -> f32 {
    HEAD + i as f32 * (CARD_H + CARD_GAP)
}

/// A screen's heading, rows of the layout the column wide, as the boards
/// set it and the cells draw it at the column's left: `title` in the
/// 22-point semibold, `subtitle` dim under it, each cut with an ellipsis
/// where it runs long, then the room before the first control ([`HEAD`]
/// in all).
pub(super) fn heading(ui: &mut Ui, title: &str, subtitle: &str, pal: &Pal) {
    studio::line(ui, TITLE_H, title, strong(22.0), pal.text);
    ui.add_space(SUB_GAP);
    studio::line(ui, SUB_H, subtitle, body(14.0), pal.dim);
    ui.add_space(HEAD_GAP);
}

/// A screen's body `body_h` points tall, laid out by `add` in `ui`, the
/// column: whole where the column has the room, else scrolled by the wheel
/// under egui's thumb, its rows [`BAR_ROOM`] short of the column's right
/// edge, where the thumb stands. The keyboard's control, `keep` (the
/// screen's number for it and where it runs down the body), is brought just
/// into view on the pass the keyboard moved to it, the screen came up or
/// the column changed height, and never after, so the wheel keeps the body
/// where it puts it. The cells draw such a body on past the column's foot,
/// over the status line and the bar; the boards' column has the room.
pub(super) fn scrolled<R>(ui: &mut Ui, id: Id, body_h: f32, keep: Option<(usize, Rangef)>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let view_h = ui.available_height().max(0.0);
    let overflows = body_h > view_h + 0.5;
    let memory = id.with("view");
    let now = ui.ctx().cumulative_pass_nr();
    let (last_pass, last_keep, last_h, last_offset): (u64, Option<usize>, f32, f32) =
        ui.ctx().data(|d| d.get_temp(memory)).unwrap_or((0, None, 0.0, 0.0));
    // Up on this pass when it was not drawn on the last: from its top.
    let fresh = last_pass + 1 != now;
    let (mut offset, mut moved) = if fresh { (0.0, true) } else { (last_offset, false) };
    if let Some((at, run)) = keep
        && (fresh || last_keep != Some(at) || (last_h - view_h).abs() > 0.5)
    {
        if run.min < offset {
            (offset, moved) = (run.min, true);
        } else if run.max > offset + view_h {
            (offset, moved) = ((run.max - view_h).min(run.min), true);
        }
    }
    let mut scroll = ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if moved {
        scroll = scroll.vertical_scroll_offset(offset.max(0.0));
    }
    let output = scroll.show(ui, |ui| {
        // The rows stand where `keep` says, whatever egui's spacing: a
        // context's first pass lays out in its default (the kit styles it
        // during that pass), which sets rows 3 points apart.
        ui.spacing_mut().item_spacing.y = 0.0;
        if overflows {
            ui.set_max_width((ui.available_width() - BAR_ROOM).max(1.0));
        }
        add(ui)
    });
    let state = (now, keep.map(|(at, _)| at), view_h, output.state.offset.y);
    ui.ctx().data_mut(|d| d.insert_temp(memory, state));
    output.inner
}

/// One opt-in, a [`CARD_H`] row of the layout the column wide: the kit's
/// tick card (`studio::tick_card`, the canvas's Federation inbox) with the
/// keyboard's look the kit's lacks. The panel's fill in an edge hairline,
/// raised under the pointer with its edge brightened to the dim; under the
/// keyboard's cursor (`focused`) raised and ringed in the accent, which
/// wins over the pointer's look, as the kit's tick rows have it. The box
/// 24 points in and 20 down carries the flag, its tick in the ok green;
/// `label` semibold beside it and `hint` dim under the label, each cut with
/// an ellipsis 16 points short of the edge. The whole card is the click,
/// with the hand.
fn tick_card(ui: &mut Ui, id: Id, on: bool, label: &str, hint: &str, focused: bool, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, CARD_H), Sense::hover());
    let response = studio::hand(ui.interact(rect, id, Sense::click()));
    let hovered = response.hovered();
    ui.painter().rect_filled(rect, studio::square(), if focused || hovered { pal.raised } else { pal.panel });
    if focused {
        studio::ring(ui, rect, pal);
    } else {
        let edge = if hovered { pal.dim } else { pal.edge };
        ui.painter().rect_stroke(rect, studio::square(), Stroke::new(1.0, edge), StrokeKind::Inside);
    }
    let top = rect.min.y + 20.0;
    let mark = Rect::from_center_size(pos2(rect.min.x + 24.0, top), vec2(BOX, BOX));
    studio::tick_box(ui, mark, on, hovered, pal.ground, pal);
    let x = mark.max.x + 12.0;
    let room = (rect.max.x - x - 16.0).max(1.0);
    studio::text(ui, pos2(x, top), label, strong(14.0), pal.text, room, Align2::LEFT_CENTER);
    studio::text(ui, pos2(x, top + 21.0), hint, body(13.0), pal.dim, room, Align2::LEFT_CENTER);
    response
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{FullOutput, Id, Pos2, Rect, vec2};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rust_i18n::t;

    use super::super::tests::{BOARD, Probe, color_of, drawn, has, wizard};
    use super::super::{FLOOR, parts};
    use crate::gui::window::studio::{BAR_ROOM, Pal};
    use crate::setup::cells::footer_hint;
    use crate::setup::tests::LOCALE_LOCK;
    use crate::setup::{Screen, Wizard, handle_key};

    fn card(i: usize) -> Id {
        Id::new("setup-extra").with(i)
    }

    fn key(wizard: &mut Wizard, code: KeyCode) {
        handle_key(wizard, KeyEvent::new(code, KeyModifiers::NONE));
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
