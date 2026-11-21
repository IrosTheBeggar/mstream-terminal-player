//! The Login screen's body and its skip gate drawn by egui, after the
//! canvas's Login and LoginSkip boards. The body sits inside the frame's
//! column (the frame, egui_view.rs, draws the top strip, the status and
//! tips lines and the bar with Back and Create Admin around it, the form's
//! problem said on the status line as the cells say it); the gate is a
//! window over the kit's scrim, the page beneath inert, the status and tips
//! lines lifted lit over the scrim as the board stands them.
//!
//! Every click is the act the cells register for the same control
//! (`draw_login`, `draw_skip_warning`): a field gives itself the keyboard
//! (Focus) and leaves its caret where it stood, Skip for now raises the
//! gate (SkipLogin), and the gate's way back closes it (SkipCancel) and its
//! other answer goes public (SkipConfirm). The keys answer both as they
//! always have: the wizard's keymap moves between the fields and edits the
//! one with the keyboard, and Enter, Esc and b answer the gate.

use egui::{Id, Rangef, Rect, Sense, Ui, pos2, vec2};
use rust_i18n::t;

use super::super::{Act, LoginField, Wizard};
use super::{Drawn, Page, lift_lines, tip};
use crate::gui::window::studio::{self, FIELD_ROW_H, HEADING_H, HIT, Para, body};

/// The form's width, centred in the column: the boards' 420 points, the
/// cells' 44 cells of their 74.
const FORM_W: f32 = 420.0;
/// The room between two fields, and between the last and Skip for now.
const FIELD_GAP: f32 = 14.0;
const SKIP_GAP: f32 = 18.0;
/// The three fields' height, and the body's: the heading, the fields and
/// Skip for now's target.
const FIELDS_H: f32 = 3.0 * FIELD_ROW_H + 2.0 * FIELD_GAP;
const BODY_H: f32 = HEADING_H + FIELDS_H + SKIP_GAP + HIT;

/// The body over the whole of `ui`, the column, the kit's scrolled body:
/// the kit's heading, then the form centred under it, its three fields as
/// the kit draws a field (the two passwords masked one dot per character,
/// the input method's text too), the one with the keyboard in the accent
/// with its caret, which is noted for the host; then Skip for now, the
/// kit's quiet button, at the form's left with the cells' tooltip. The
/// field with the keyboard is brought into view on the pass it gets it
/// where the column is too short for the form.
pub(super) fn show(ui: &mut Ui, wizard: &mut Wizard, page: &Page) -> Drawn {
    let pal = &page.pal;
    let fields = [
        (LoginField::Username, "setup-username", t!("login.field_username"), &wizard.username, false),
        (LoginField::Password, "setup-password", t!("login.field_password"), &wizard.password, true),
        (LoginField::Confirm, "setup-confirm", t!("login.field_confirm"), &wizard.confirm, true),
    ];
    let keep = fields.iter().position(|(field, ..)| *field == wizard.field).map(|k| (k, Rangef::new(field_top(k), field_top(k) + FIELD_ROW_H)));
    let mut drawn = Drawn::default();
    studio::scrolled(ui, Id::new("setup-login"), BODY_H, keep, |ui| {
        studio::heading(ui, &t!("login.title"), &t!("login.subtitle"), pal);
        let room = ui.available_width();
        let (form, _) = ui.allocate_exact_size(vec2(room, FIELDS_H), Sense::hover());
        let w = FORM_W.min(room);
        let left = form.center().x - w / 2.0;
        for (k, (field, id, label, input, masked)) in fields.iter().enumerate() {
            let slot = Rect::from_min_size(pos2(left, form.min.y + k as f32 * (FIELD_ROW_H + FIELD_GAP)), vec2(w, FIELD_ROW_H));
            let shown = studio::Input::of(input).focused(*field == wizard.field, page.composition);
            let shown = if *masked { shown.masked() } else { shown };
            let typed = studio::field_at(ui, Id::new(*id), slot, label, &shown, pal);
            // A click area, as the cells' card is: it gives the field the
            // keyboard and leaves its caret where it stood, so the hand,
            // not the I-beam of a field whose click places one.
            if studio::hand(typed.response).clicked() {
                drawn.acts.push(Act::Focus(*field));
            }
            // The caret of the field drawn with the keyboard, also on the
            // pass a click moves the keyboard on: the field it moved to notes
            // its own on the next, as the cells' next frame does, and the
            // window's input method stays on between the two rather than
            // dropping what it was composing.
            if typed.caret.is_some() {
                drawn.caret = typed.caret;
            }
        }
        ui.add_space(SKIP_GAP);
        let (row, _) = ui.allocate_exact_size(vec2(room, HIT), Sense::hover());
        let skip = studio::quiet_button(ui, Id::new("setup-skip-login"), pos2(left, row.center().y), &t!("login.skip"), w, pal);
        tip(page, &skip, &t!("login.tip_skip"));
        if skip.clicked() {
            drawn.acts.push(Act::SkipLogin);
        }
    });
    drawn
}

/// Where field `k`'s row (its label and its box) starts down the body.
fn field_top(k: usize) -> f32 {
    HEADING_H + k as f32 * (FIELD_ROW_H + FIELD_GAP)
}

// ── The skip gate ─────────────────────────────────────────────────────────

/// The skip gate (`Modal::SkipWarning`) over `over`, the whole page, as the
/// LoginSkip board draws it: the kit's gold gate, no close (Esc, b and the
/// way back cover it), asking the cells' question; their sentences, the
/// upsides in the ok green and the downsides in gold, each led by its "+"
/// or "−" in a column of its own (`Para::listed`), the last dim; and the
/// two answers, the way back lit, side by side, or going public on a row of
/// its own above the way back where a translation will not fit, as the
/// cells move their confirmation up (`skip_buttons`). Enter goes public, as
/// the model has it; no answer takes egui's focus. The frame's status and
/// tips lines stand lit over the scrim where the window leaves them clear
/// (`lift_lines`).
pub(super) fn skip_gate(ui: &mut Ui, over: Rect, wizard: &mut Wizard, page: &Page) -> Drawn {
    let pal = &page.pal;
    let sentences = vec![
        Para::new(&t!("skip_modal.body"), body(14.0), pal.text),
        Para::listed(&t!("skip_modal.plus_access"), body(14.0), pal.ok),
        Para::listed(&t!("skip_modal.plus_tv"), body(14.0), pal.ok),
        Para::listed(&t!("skip_modal.minus_control"), body(14.0), pal.gold),
        Para::listed(&t!("skip_modal.minus_qc"), body(14.0), pal.gold),
        Para::new(&t!("skip_modal.later"), body(14.0), pal.dim),
    ];
    let answers = [t!("skip_modal.back"), t!("skip_modal.confirm")];
    let id = Id::new("setup-skip");
    let shown = studio::gate(ui, id, over, &t!("skip_modal.title"), sentences, [&answers[0], &answers[1]], pal);
    lift_lines(ui, id, over, shown.window.max.y, wizard, page);
    let acts = shown.inner.map(|go| if go { Act::SkipConfirm } else { Act::SkipCancel });
    Drawn { acts: acts.into_iter().collect(), caret: None }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use egui::epaint::Shape;
    use egui::{Event, Id, Pos2, Rect, pos2, vec2};
    use ratatui::crossterm::event::KeyCode;
    use rust_i18n::t;
    use tui_input::Input;

    use super::super::FLOOR;
    use super::super::tests::{BOARD, Probe, color_of, drawn, has, key, lifted, tipped, wheel, wizard};
    use super::FORM_W;
    use crate::gui::window::studio::{FORM_BUTTON_H, Pal, foot, foot_rects};
    use crate::setup::cells::footer_hint;
    use crate::setup::tests::LOCALE_LOCK;
    use crate::setup::{LANGS, LoginField, Modal, Op, Screen, Wizard};

    const FIELDS: [&str; 3] = ["setup-username", "setup-password", "setup-confirm"];
    /// The board's gate: 600 wide, 24 in, its answers 10 apart beside one
    /// another or a row apart.
    const GATE_W: f32 = 600.0;
    const PAD: f32 = 24.0;
    const ANSWER_GAP: f32 = 10.0;

    /// The gate's two answers, the kit's gate's foot's: the way back first.
    fn answer(k: usize) -> Id {
        Id::new("setup-skip").with("foot").with(k)
    }

    fn login() -> Wizard {
        let mut wizard = wizard();
        wizard.screen = Screen::Login;
        wizard
    }

    fn field(k: usize) -> Id {
        Id::new(FIELDS[k])
    }

    /// The board's moment: anna's form with the confirmation mistyped.
    fn filled(wizard: &mut Wizard) {
        wizard.username = Input::new("anna".into());
        wizard.password = Input::new("correct-horse-7".into());
        wizard.confirm = Input::new("correct-horse-8".into());
    }

    #[test]
    fn the_form_draws_the_cells_words_masks_the_passwords_and_stands_where_the_board_puts_it() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = login();
        filled(&mut wizard);
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let dots = "•".repeat(15);
        for w in [&*t!("login.title"), &*t!("login.subtitle"), "USERNAME", "PASSWORD", "CONFIRM PASSWORD", &*t!("login.skip"), "anna", &dots] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert_eq!(words.iter().filter(|w| **w == dots).count(), 2, "both passwords, one dot a character");
        assert!(!words.iter().any(|w| w.contains("horse")), "never in clear: {words:?}");
        assert_eq!(color_of(&out, &t!("login.skip")), Some(pal.dim), "Skip for now at rest");
        assert!(has(&words, &t!("hint.login")));
        // The board's form: 420 wide, centred, the fields' boxes from y 160,
        // 54 apart with 14 between, Skip for now 18 under them.
        for k in 0..3 {
            let at = probe.rect(field(k));
            assert_eq!(at, Rect::from_min_size(pos2(430.0, 160.0 + k as f32 * 68.0), vec2(FORM_W, 34.0)), "field {k}");
        }
        let skip = probe.rect(Id::new("setup-skip-login"));
        assert!((skip.min.x - 430.0).abs() < 0.01 && (skip.min.y - 348.0).abs() < 0.01 && skip.height() == 28.0, "{skip:?}");
    }

    #[test]
    fn the_field_with_the_keyboard_draws_its_caret_and_composition_and_hands_the_caret_back() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let mut probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = login();
        filled(&mut wizard);
        for (k, which) in [LoginField::Username, LoginField::Password, LoginField::Confirm].into_iter().enumerate() {
            wizard.field = which;
            let (out, shown) = probe.settle(&mut wizard);
            let caret = shown.caret.unwrap_or_else(|| panic!("{which:?}'s caret"));
            let at = probe.rect(field(k));
            assert!(at.contains(caret - vec2(0.0, 1.0)), "{which:?}: the caret's foot in its box: {caret:?} {at:?}");
            // Its box in the accent, the others at rest.
            let edges: Vec<Rect> = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Rect(r) if r.stroke.color == pal.accent && r.rect.height() == 34.0 => Some(r.rect),
                    _ => None,
                })
                .collect();
            assert_eq!(edges, [at], "{which:?} alone in the accent");
        }
        // The input method's text at the caret: in clear in the username,
        // one dot a character in a password, in no field without the keys.
        probe.composition = "りあ".into();
        wizard.field = LoginField::Username;
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(has(&words, "annaりあ") && has(&words, &"•".repeat(15)), "{words:?}");
        wizard.field = LoginField::Password;
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(has(&words, &"•".repeat(17)) && has(&words, "anna") && !words.iter().any(|w| w.contains("りあ")), "{words:?}");
        // The keys edit the model's field, and the next pass draws it.
        probe.composition.clear();
        wizard.field = LoginField::Username;
        key(&mut wizard, KeyCode::Char('!'));
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), "anna!"));
        // Under the gate no field has the keyboard, so none hands back a
        // caret; the gate's answer gives it back.
        wizard.modal = Modal::SkipWarning;
        assert!(probe.settle(&mut wizard).1.caret.is_none(), "no caret under the gate");
        key(&mut wizard, KeyCode::Esc);
        assert!(probe.settle(&mut wizard).1.caret.is_some());
    }

    #[test]
    fn a_click_on_a_field_gives_it_the_keyboard_and_leaves_its_caret_where_it_stood() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = login();
        filled(&mut wizard);
        // The password's caret two letters from its end.
        wizard.field = LoginField::Password;
        key(&mut wizard, KeyCode::Left);
        key(&mut wizard, KeyCode::Left);
        wizard.field = LoginField::Username;
        let _ = probe.settle(&mut wizard);
        let at = probe.rect(field(1)).center();
        let _ = probe.pass(&mut wizard, vec![Event::PointerMoved(at)]);
        assert!(probe.pass(&mut wizard, Vec::new()).1.hand, "a click area, as the cells' card: the hand");
        let shown = probe.click(&mut wizard, field(1));
        assert_eq!(wizard.field, LoginField::Password);
        assert_eq!(wizard.password.cursor(), 13, "the caret where it stood");
        // The pass of the click hands back the caret it drew, the
        // username's, so the input method stays on through the move, as
        // through the cells' (their caret is the last frame's until the next).
        assert!(shown.caret.is_some_and(|c| probe.rect(field(0)).contains(c - vec2(0.0, 1.0))), "{:?}", shown.caret);
        let (_, shown) = probe.settle(&mut wizard);
        assert!(shown.caret.is_some_and(|c| probe.rect(field(1)).contains(c - vec2(0.0, 1.0))), "the next pass hands back the password's");
        // A click on the field that has the keyboard keeps its caret.
        let shown = probe.click(&mut wizard, field(1));
        assert!(shown.caret.is_some_and(|c| probe.rect(field(1)).contains(c - vec2(0.0, 1.0))));
        probe.click(&mut wizard, field(2));
        assert_eq!(wizard.field, LoginField::Confirm);
        // The label row takes no click: only the box does.
        let label = probe.rect(field(0)).min - vec2(-20.0, 12.0);
        probe.press(&mut wizard, label);
        assert_eq!(wizard.field, LoginField::Confirm);
        assert!(wizard.queued.is_none() && wizard.note.is_none() && wizard.modal == Modal::None, "nothing else happened");
    }

    #[test]
    fn skip_for_now_raises_the_gate_and_carries_the_cells_tip() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = login();
        let _ = probe.settle(&mut wizard);
        let skip = probe.rect(Id::new("setup-skip-login")).center();
        let seen = probe.rest(&mut wizard, skip, 0.6);
        assert!(has(&drawn(&seen), &tipped(&t!("login.tip_skip"))), "the whole tip: no key in it to cut");
        assert_eq!(color_of(&seen, &t!("login.skip")), Some(pal.text), "the words lit under the pointer");
        let shown = probe.click(&mut wizard, Id::new("setup-skip-login"));
        assert!(shown.outcome.is_none());
        assert_eq!((wizard.modal.clone(), wizard.screen), (Modal::SkipWarning, Screen::Login), "the gate over the page, the page unchanged");
    }

    #[test]
    fn create_with_a_problem_says_it_on_the_status_line_in_gold() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = login();
        filled(&mut wizard);
        wizard.field = LoginField::Confirm;
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, Id::new("setup-forward"));
        let mismatch = t!("login_problem.mismatch").to_string();
        assert_eq!(wizard.note, Some((mismatch.clone(), true)));
        assert!(wizard.queued.is_none(), "nothing sent");
        let (out, shown) = probe.settle(&mut wizard);
        assert_eq!(color_of(&out, &mismatch), Some(pal.gold));
        assert!(shown.caret.is_some(), "the confirmation keeps the keyboard");
        // Mended, Create queues the call with its busy line.
        wizard.confirm = Input::new("correct-horse-7".into());
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, Id::new("setup-forward"));
        assert_eq!(wizard.queued, Some(Op::CreateAdmin));
    }

    #[test]
    fn the_gate_draws_the_cells_lines_over_an_inert_page_and_its_answers_do_what_the_cells_do() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = login();
        let _ = probe.settle(&mut wizard);
        let beneath = [probe.rect(field(1)).center(), probe.rect(Id::new("setup-skip-login")).center(), probe.rect(Id::new("setup-back")).center()];
        wizard.act(crate::setup::Act::SkipLogin);
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert_eq!(color_of(&out, &t!("skip_modal.title")), Some(pal.gold), "the gate's gold title");
        assert_eq!(color_of(&out, &t!("skip_modal.body")), Some(pal.text));
        // The marks in a column of their own, in their line's colour.
        let lines = [("skip_modal.plus_access", pal.ok), ("skip_modal.plus_tv", pal.ok), ("skip_modal.minus_control", pal.gold), ("skip_modal.minus_qc", pal.gold)];
        for (key, tone) in lines {
            let line = t!(key).to_string();
            let (mark, words_of) = line.split_once(' ').expect("a mark and its words");
            assert!(has(&words, words_of), "{key}'s words without their mark: {words:?}");
            assert_eq!(color_of(&out, words_of), Some(tone), "{key}");
            assert_eq!(color_of(&out, mark), Some(tone), "{key}'s mark");
        }
        assert_eq!(color_of(&out, &t!("skip_modal.later")), Some(pal.dim));
        for w in ["BACK — CREATE A LOGIN", "GO PUBLIC ANYWAY", &*footer_hint(&wizard)] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert_eq!(color_of(&out, "BACK — CREATE A LOGIN"), Some(pal.on_accent), "the way back lit");
        assert_eq!(color_of(&out, "GO PUBLIC ANYWAY"), Some(pal.dim));
        assert!(shown.caret.is_none());
        // No [X]: the gate's one close is its way back, a form button.
        assert!(probe.ctx.read_response(Id::new("setup-skip").with("close")).is_none(), "no close");
        let back = probe.rect(answer(0));
        assert_eq!(back.height(), FORM_BUTTON_H, "{back:?}");
        let go = probe.rect(answer(1));
        assert!((back.max.x + ANSWER_GAP - go.min.x).abs() < 0.01 && back.min.y == go.min.y, "side by side in English: {back:?} {go:?}");
        // Side by side at the window's right edge, 24 in.
        let window_right = BOARD.x / 2.0 + GATE_W / 2.0;
        assert!((go.max.x - (window_right - PAD)).abs() < 0.01, "{go:?}");

        // The page beneath is inert: a press on a field, Skip or Back is lost.
        for at in beneath {
            probe.press(&mut wizard, at);
        }
        assert_eq!((wizard.modal.clone(), wizard.screen, wizard.field), (Modal::SkipWarning, Screen::Login, LoginField::Username));
        // The way back closes it, the page as it was.
        let _ = probe.settle(&mut wizard);
        let shown = probe.click(&mut wizard, answer(0));
        assert_eq!((wizard.modal.clone(), wizard.screen), (Modal::None, Screen::Login));
        assert!(shown.outcome.is_none());
        // Going public: Done, public, the Quick Connect load queued.
        wizard.act(crate::setup::Act::SkipLogin);
        let _ = probe.settle(&mut wizard);
        let shown = probe.click(&mut wizard, answer(1));
        assert!(shown.outcome.is_none(), "no click ends the wizard here");
        assert_eq!((wizard.modal.clone(), wizard.screen, wizard.public), (Modal::None, Screen::Done, true));
        assert_eq!(wizard.queued, Some(Op::LoadDone));
    }

    #[test]
    fn enter_and_esc_answer_the_gate_as_the_model_has_them_and_no_answer_is_egui_focus() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = login();
        wizard.act(crate::setup::Act::SkipLogin);
        let _ = probe.settle(&mut wizard);
        assert!(probe.ctx.memory(|m| m.focused()).is_none());
        key(&mut wizard, KeyCode::Esc);
        let _ = probe.settle(&mut wizard);
        assert_eq!(wizard.modal, Modal::None, "Esc keeps");
        wizard.act(crate::setup::Act::SkipLogin);
        let _ = probe.settle(&mut wizard);
        key(&mut wizard, KeyCode::Enter);
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!((wizard.screen, wizard.public), (Screen::Done, true), "Enter goes public");
        assert!(!has(&drawn(&out), &t!("skip_modal.title")), "and the gate is gone");
    }

    /// Over the gate the status line and the tips line stand lit over the
    /// scrim, as the LoginSkip board draws them, where the window leaves
    /// them clear: the gate's hint, and the problem the form had when the
    /// gate came up. Where the window reaches them, on a page at the cells'
    /// floor, they stay under the scrim, where the frame drew them.
    #[test]
    fn the_status_and_tips_lines_stand_over_the_gates_scrim_where_its_window_leaves_them_clear() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let problem = t!("login_problem.username").to_string();
        for (size, clear) in [(BOARD, true), (FLOOR, false)] {
            let probe = Probe::new(size);
            let mut wizard = login();
            wizard.act(crate::setup::Act::CreateAdmin);
            assert_eq!(wizard.note, Some((problem.clone(), true)), "an empty form's first problem");
            wizard.act(crate::setup::Act::SkipLogin);
            let (out, _) = probe.settle(&mut wizard);
            let (words, hint) = (drawn(&out), footer_hint(&wizard));
            assert_eq!(hint, t!("hint.skip"));
            assert!(has(&words, &hint) && has(&words, &problem), "at {size:?}: {words:?}");
            assert_eq!(lifted(&out, size, &hint), clear, "at {size:?}: the hint");
            assert_eq!(lifted(&out, size, &problem), clear, "at {size:?}: the problem");
            // Lifted or not, they take no click: a press on the status line
            // leaves the gate up.
            let status = super::super::parts(Rect::from_min_size(Pos2::ZERO, size)).status;
            probe.press(&mut wizard, status.left_center() + vec2(8.0, 0.0));
            assert_eq!(wizard.modal, Modal::SkipWarning, "at {size:?}");
        }
    }

    #[test]
    fn at_the_floor_the_gate_scrolls_its_body_to_its_answers_and_comes_up_at_its_top() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(FLOOR);
        let mut wizard = login();
        wizard.modal = Modal::SkipWarning;
        let _ = probe.settle(&mut wizard);
        let id = Id::new("setup-skip");
        // How much of the way back shows: its height in view. The window
        // stays on the page whatever its body's height.
        let shows = |probe: &Probe| probe.ctx.read_response(answer(0)).map_or(0.0, |r| r.interact_rect.height().max(0.0));
        let page = Rect::from_min_size(Pos2::ZERO, FLOOR);
        let window = probe.ctx.data(|d| d.get_temp::<f32>(id.with("height"))).expect("the window took a height");
        assert!(window <= FLOOR.y - 32.0 + 0.5, "{window}");
        // Taller than the page allows with this system's face, the body
        // scrolls, its foot cut at the fold until the wheel brings it in.
        if shows(&probe) >= FORM_BUTTON_H - 0.5 {
            eprintln!("the gate's body fits the floor with this face: no scroll to test");
            return;
        }
        wheel(&probe, &mut wizard, page.center(), -2000.0);
        for _ in 0..10 {
            let _ = probe.pass_after(&mut wizard, Vec::new(), 0.1);
        }
        assert!(shows(&probe) >= FORM_BUTTON_H - 0.5, "the wheel brought the way back whole into view: {}", shows(&probe));
        probe.click(&mut wizard, answer(0));
        assert_eq!(wizard.modal, Modal::None);
        // Up again after a frame without it, it comes up at its top.
        let _ = probe.settle(&mut wizard);
        wizard.modal = Modal::SkipWarning;
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), &t!("skip_modal.title")));
        assert!(shows(&probe) < FORM_BUTTON_H - 0.5, "from its top: {}", shows(&probe));
    }

    /// Stacked or side by side by the answers' own widths, in every
    /// language, in the boards' window and at the floor, as the kit's gate
    /// lays its foot: English sits side by side in the boards' window,
    /// Russian's long way back stacks at the floor (as German, Spanish,
    /// Italian, Portuguese and Russian stack at the cells' floor), going
    /// public keeps the first row, and no answer leaves its row. Which
    /// others stack is the system face's to say.
    #[test]
    fn the_answers_stack_only_where_a_translation_will_not_fit_side_by_side() {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts_every_script(&ctx);
        let inner = |page_w: f32| GATE_W.min(page_w - 32.0) - 2.0 * PAD;
        let (wide, narrow) = (inner(BOARD.x), inner(FLOOR.x));
        let mut stacked = Vec::new();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BOARD)), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            crate::gui::window::studio::style_once(ui.ctx(), &Pal::now());
            for (code, _) in LANGS {
                let (keep, go) = (t!("skip_modal.back", locale = code), t!("skip_modal.confirm", locale = code));
                for w in [wide, narrow] {
                    let (laid, h) = foot_rects(ui, &[], &[(&keep, true), (&go, false)], w);
                    let (k, g) = (laid[0], laid[1]);
                    let side = k.width() + ANSWER_GAP + g.width() <= w;
                    assert!(k.min.x >= -0.01 && g.min.x >= -0.01 && (g.max.x - w).abs() < 0.01, "{code} at {w}: in the row, {k:?} {g:?}");
                    let keep_ends = if side { g.min.x - ANSWER_GAP } else { w };
                    assert!((k.max.x - keep_ends).abs() < 0.01, "{code} at {w}: the way back first, or under: {k:?} {g:?}");
                    assert_eq!(h == FORM_BUTTON_H, side, "{code} at {w}");
                    assert_eq!(k.min.y == g.min.y, side, "{code} at {w}: one row or two");
                    assert!(g.min.y.abs() < 0.01, "{code} at {w}: going public on the first row");
                    if !side {
                        stacked.push((code, w == wide));
                    }
                }
            }
        });
        out.textures_delta.clear();
        assert!(!stacked.contains(&("en", true)), "English side by side in the boards' window: {stacked:?}");
        assert!(stacked.contains(&("ru", false)), "Russian stacked at the floor: {stacked:?}");
    }

    /// The gate's foot as drawn, in the floor's row with Russian's answers
    /// (handed in, so no locale is switched): going public on a row above
    /// the way back, both ending at the right edge, each still taking its
    /// own click and answering as before.
    #[test]
    fn a_stacked_foot_draws_both_answers_whole_and_each_answers() {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts_every_script(&ctx);
        let id = Id::new("probe-skip");
        let (keep, go) = (t!("skip_modal.back", locale = "ru").to_string(), t!("skip_modal.confirm", locale = "ru").to_string());
        let row = Rect::from_min_size(pos2(40.0, 30.0), vec2(GATE_W.min(FLOOR.x - 32.0) - 2.0 * PAD, 200.0));
        let answered = Cell::new(None);
        let run = |events: Vec<Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BOARD)), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                crate::gui::window::studio::style_once(ui.ctx(), &Pal::now());
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::top_down(egui::Align::Min)));
                if let Some(k) = foot(&mut child, id, &[], &[(&keep, true), (&go, false)], None, &Pal::now()) {
                    answered.set(Some(k == 1));
                }
            });
            out.textures_delta.clear();
            drawn(&out)
        };
        let _ = run(Vec::new());
        let words = run(Vec::new());
        let rect = |k: usize| ctx.read_response(id.with(k)).unwrap_or_else(|| panic!("answer {k} drawn")).rect;
        let (back, ahead) = (rect(0), rect(1));
        assert!((back.max.x - row.max.x).abs() < 0.01 && (ahead.max.x - row.max.x).abs() < 0.01, "both at the right edge: {back:?} {ahead:?}");
        assert!((back.min.y - ahead.max.y - ANSWER_GAP).abs() < 0.01 && (ahead.min.y - row.min.y).abs() < 0.01, "going public above: {back:?} {ahead:?}");
        assert!(back.width() <= row.width() && ahead.width() <= row.width());
        assert!(has(&words, &keep.trim_start_matches(['◂', ' ']).to_uppercase()) && has(&words, &go.to_uppercase()), "{words:?}");
        let click = |at: Pos2| {
            let button = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
            for events in [vec![Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
                let _ = run(events);
            }
        };
        click(ahead.center());
        assert_eq!(answered.take(), Some(true), "going public goes through");
        click(back.center());
        assert_eq!(answered.take(), Some(false), "the way back keeps");
    }
}
