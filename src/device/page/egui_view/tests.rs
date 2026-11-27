//! The page's egui face run headless, as the window's panel runs it: a
//! context with the panel's faces, the page's area in points, and the
//! page's own fixtures — boards told on quiet channels (`Page::quiet`,
//! `Ends::tell`), the commands it sent read back (`Ends::sent`). No worker
//! runs, no port or card is opened, no native dialog comes up.

use std::cell::RefCell;
use std::rc::Rc;

use egui::epaint::Shape;
use egui::{Event, FullOutput, MouseWheelUnit, PointerButton, Vec2};

use super::super::fixtures;
use super::board::tests::{card_room_of, filled};
use super::*;
use crate::device::card::worker::{self, Fold, Link, fixtures as cards};

fn english() -> std::sync::RwLockReadGuard<'static, ()> {
    let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
    rust_i18n::set_locale("en");
    crate::kit::theme::pin_modern_terminal();
    guard
}

/// The page's area in points: in the GUI's default 100×30 window (its 28
/// rows of 8 by 16 points under the top bar and over the footer), at the
/// GUI's floor (100×24: 22 rows), and in the boards' 1440 by 900 window.
const WINDOW: Vec2 = vec2(800.0, 448.0);
const FLOOR: Vec2 = vec2(800.0, 352.0);
const BOARD: Vec2 = vec2(1440.0, 824.0);

/// A context styled as the host's, its passes run over a page's area.
struct Probe {
    ctx: egui::Context,
    size: Vec2,
}

impl Probe {
    fn new(size: Vec2) -> Probe {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        studio::style_once(&ctx, &Pal::now());
        Probe { ctx, size }
    }

    /// One pass with `events`: what egui drew, and what the page told its
    /// host over the pass's runs (a click either run acted on stands).
    fn pass(&self, page: &mut Page, events: Vec<Event>) -> (FullOutput, EguiShown) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), events, ..Default::default() };
        let mut told: Option<EguiShown> = None;
        let mut out = self.ctx.run_ui(input, |ui| {
            let next = show(ui, page, "", false);
            told = Some(match told.take() {
                Some(last) => EguiShown { outcome: last.outcome.or(next.outcome), caret: next.caret },
                None => next,
            });
        });
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        (out, told.expect("the pass ran"))
    }

    /// One pass with `events` that egui runs twice, as it does for a modal
    /// that took its body's height: the first run asks for the second, and
    /// the runs egui made come back.
    fn pass_twice(&self, page: &mut Page, events: Vec<Event>) -> u32 {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), events, ..Default::default() };
        let mut runs = 0;
        let mut out = self.ctx.run_ui(input, |ui| {
            runs += 1;
            let _ = show(ui, page, "", false);
            if runs == 1 {
                ui.ctx().request_discard("the test's second run");
            }
        });
        out.textures_delta.clear();
        runs
    }

    /// Two passes (egui sizes some things on the first), the second's.
    fn settle(&self, page: &mut Page) -> FullOutput {
        let _ = self.pass(page, Vec::new());
        self.pass(page, Vec::new()).0
    }

    /// A press and a release at `at`, a pass each after the move, as
    /// hands make them.
    fn click(&self, page: &mut Page, at: Pos2) {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        for events in [vec![Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
            let _ = self.pass(page, events);
        }
    }

    /// A click on the middle of what `id` drew on the last pass.
    fn click_id(&self, page: &mut Page, id: Id) {
        let at = self.ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect.center();
        self.click(page, at);
    }

    /// A press at `at`; `meanwhile` done to the page while the button is
    /// held, and a pass that draws it, as the window draws the worker's
    /// news; then the release where the press was.
    fn press_held_through(&self, page: &mut Page, at: Pos2, meanwhile: impl FnOnce(&mut Page)) {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let _ = self.pass(page, vec![Event::PointerMoved(at)]);
        let _ = self.pass(page, vec![button(true)]);
        meanwhile(page);
        let _ = self.pass(page, Vec::new());
        let _ = self.pass(page, vec![button(false)]);
    }
}

/// Every text a pass drew, whole, with where.
fn texts(out: &FullOutput) -> Vec<(String, Rect)> {
    out.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Text(t) => Some((t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size()))),
            _ => None,
        })
        .collect()
}

fn drew(out: &FullOutput, words: &str) -> bool {
    texts(out).iter().any(|(t, _)| t == words)
}

/// Where the pass drew `words`.
fn find(out: &FullOutput, words: &str) -> Rect {
    texts(out).into_iter().find(|(t, _)| t == words).map(|(_, r)| r).unwrap_or_else(|| panic!("{words:?} drawn: {:?}", texts(out)))
}

fn assert_drew(out: &FullOutput, words: &[&str]) {
    for w in words {
        assert!(drew(out, w), "{w:?} drawn: {:?}", texts(out).into_iter().map(|(t, _)| t).collect::<Vec<_>>());
    }
}

/// A page on quiet channels, its worker's reports read.
fn page_with(tell: impl FnOnce(&Ends)) -> (Page, Ends) {
    let (mut page, ends) = Page::quiet();
    tell(&ends);
    page.pump();
    (page, ends)
}

// ── The board card ──────────────────────────────────────────────────────

#[test]
fn a_board_a_release_behind_draws_its_head_its_chip_its_card_row_and_its_primary() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = page_with(Ends::update_available);
    let out = probe.settle(&mut page);
    assert_drew(&out, &["M5Stack Core2", "COM3", "FIRMWARE", "SD CARD", "Details", "Advanced…", "UPDATE"]);
    assert_drew(&out, &["Update available · v0.7.0 → v0.8.0", "v0.7.0 can't report the card."]);
    assert!(!drew(&out, "? v0.7.0 can't report the card."), "the cells' glyph is the kit's LED, not a word");
    assert!(!texts(&out).iter().any(|(t, _)| t.contains("COM5")), "one board: no subjects row");
    // The head's port at the card's right on the name's baseline, the
    // primary at the content's.
    let (name, port, primary) = (find(&out, "M5Stack Core2"), find(&out, "COM3"), find(&out, "UPDATE"));
    let level = (baseline(&out, "M5Stack Core2") - baseline(&out, "COM3")).abs() < 0.5;
    assert!(level && port.min.x > name.max.x + 100.0, "{name:?} {port:?}");
    assert!(primary.min.y > find(&out, "v0.7.0 can't report the card.").max.y, "the bottom row under the rows");
    assert!(ends.sent().is_empty(), "drawing asks the worker nothing");
}

#[test]
fn a_board_up_to_date_draws_its_fill_meter_its_figures_and_no_primary() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, _ends) = page_with(Ends::up_to_date);
    let out = probe.settle(&mut page);
    assert_drew(&out, &["Up to date · v0.8.0", "21.4 of 59.6 GB used · 1,284 tracks", "38.2 GB free"]);
    assert!(!drew(&out, "UPDATE"), "nothing to do: no primary");
    // The free figure on the used figures' baseline, at the column's end
    // clear of them. Both stand on whole pixels from the row's top, so on
    // the same one: within a tenth of a point, where centred on the row by
    // the face's own measures it stood a point over them in Segoe UI and
    // nearly a point and a half in Ubuntu Light.
    let (used, free) = (find(&out, "21.4 of 59.6 GB used · 1,284 tracks"), find(&out, "38.2 GB free"));
    let (on, at) = (baseline(&out, "21.4 of 59.6 GB used · 1,284 tracks"), baseline(&out, "38.2 GB free"));
    assert!((on - at).abs() < 0.1 && free.min.x > used.max.x + 16.0, "the free figure at the column's end, on {on}: {at}, {free:?} after {used:?}");
    // The meter: the share's segments lit in the accent across the value
    // column, the rest unlit, and the SD card's label, which has no words
    // beside it to stand on, centred on them.
    let pal = Pal::now();
    let lit = out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == pal.accent && r.rect.height() == 10.0)).count();
    let unlit = out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == pal.unlit && r.rect.height() == 10.0)).count();
    assert!(lit > 10 && unlit > lit, "about a third of the card lit: {lit} of {}", lit + unlit);
    let segment = out.shapes.iter().find_map(|c| match &c.shape {
        Shape::Rect(r) if r.fill == pal.accent && r.rect.height() == 10.0 => Some(r.rect),
        _ => None,
    });
    let (label, meter) = (find(&out, "SD CARD").center().y, segment.expect("the meter's segments").center().y);
    assert!((label - meter).abs() < 0.5, "the SD card's label centred on its meter: {label} {meter}");
}

#[test]
fn several_boards_draw_the_subjects_row_its_marks_its_right_edge_and_a_tab_click_switches() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, _ends) = page_with(Ends::two);
    let out = probe.settle(&mut page);
    assert_drew(&out, &["COM3", "COM5", "!", "1 of 2 needs an update"]);
    assert!(texts(&out).iter().any(|(t, _)| t.starts_with("serial ")), "with several boards the head's right is the serial");
    let pal = Pal::now();
    let slab = out.shapes.iter().any(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == pal.accent && r.rect.height() == ROW_H));
    assert!(slab, "the tab in view on the accent's slab");
    assert_eq!(page.open.as_deref(), Some("COM3"));
    probe.click_id(&mut page, tab_id("COM5"));
    assert_eq!(page.open.as_deref(), Some("COM5"), "the tab's act, the cells' Board(1)");
    let out = probe.settle(&mut page);
    assert_drew(&out, &["Update available · v0.7.0 → v0.8.0"]);
}

#[test]
fn the_text_buttons_ask_for_the_cells_acts_once_a_click() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = page_with(Ends::two);
    let out = probe.settle(&mut page);
    // Show on the player: two boards, COM3 answers, nothing under way.
    probe.click(&mut page, find(&out, "Show on the player").center());
    assert_eq!(ends.sent(), [Cmd::Identify { port: "COM3".into() }], "the cells' Show, sent once");
    // The Details door opens Details and asks the board's facts, as `d`.
    let out = probe.settle(&mut page);
    probe.click(&mut page, find(&out, "Details").center());
    assert!(page.details, "Details open");
    assert_eq!(ends.sent(), [Cmd::Facts { port: "COM3".into() }], "the facts asked, once");
    let out = probe.settle(&mut page);
    // The facts the default window has room for under the card at rest;
    // the rest are cut at the card's foot, as the cells cut them.
    assert_drew(&out, &["PORT", "BOARD", "ON THE BOARD"]);
    probe.click(&mut page, find(&out, "Details").center());
    assert!(!page.details, "and closed again");
}

#[test]
fn details_open_on_its_facts_and_its_log_pinned_to_the_newest_line_which_the_wheel_scrolls() {
    let _en = english();
    let probe = Probe::new(BOARD);
    let (mut page, ends) = page_with(Ends::up_to_date);
    let lines = (0..60).map(|i| log_line(&format!("line {i:02}")));
    ends.tell(lines);
    page.pump();
    page.act(Act::Details);
    let _ = ends.sent();
    let out = probe.settle(&mut page);
    assert_drew(&out, &["PORT", "line 59"]);
    assert!(!drew(&out, "line 00"), "the oldest lines above the pane");
    // A fact's label on its value's baseline, the port leading the value
    // as a figure: within a tenth of a point, where centred on the row a
    // label stood 2 points over its value's words in Segoe UI, and a
    // value led by a figure half a point over one led by words.
    let port = texts(&out).into_iter().map(|(t, _)| t).find(|t| t.starts_with("COM3 · ")).expect("the Port fact's value");
    let (label, value) = (baseline(&out, "PORT"), baseline(&out, &port));
    assert!((label - value).abs() < 0.1, "Port on its value's baseline: {label} {value}");
    // A notch up over the log: a line older, the follow stopped.
    let at = find(&out, "line 59").center();
    let wheel = Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, 1.0), phase: egui::TouchPhase::Move, modifiers: Default::default() };
    let _ = probe.pass(&mut page, vec![Event::PointerMoved(at)]);
    let _ = probe.pass(&mut page, vec![wheel.clone()]);
    assert_eq!(page.log_scroll, 1, "the page's own count, as the cells' wheel moves it");
    let out = probe.settle(&mut page);
    assert!(drew(&out, "line 58") && !drew(&out, "line 59"), "the pane one line up");
    // And down again: following the newest.
    let down = Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, -1.0), phase: egui::TouchPhase::Move, modifiers: Default::default() };
    let _ = probe.pass(&mut page, vec![down]);
    assert_eq!(page.log_scroll, 0);
}

#[test]
fn a_pass_egui_runs_twice_applies_its_click_and_its_wheel_once() {
    let _en = english();
    let probe = Probe::new(BOARD);
    let (mut page, ends) = page_with(Ends::two);
    let out = probe.settle(&mut page);
    let at = find(&out, "Show on the player").center();
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
    let _ = probe.pass(&mut page, vec![Event::PointerMoved(at)]);
    let _ = probe.pass(&mut page, vec![button(true)]);
    assert_eq!(probe.pass_twice(&mut page, vec![button(false)]), 2, "egui ran the release's pass twice");
    assert_eq!(ends.sent(), [Cmd::Identify { port: "COM3".into() }], "the click acted once");
    // The wheel over the log likewise: a notch is one line, not two.
    let (mut page, ends) = page_with(Ends::up_to_date);
    ends.tell((0..60).map(|i| log_line(&format!("line {i:02}"))));
    page.pump();
    page.act(Act::Details);
    let out = probe.settle(&mut page);
    let _ = probe.pass(&mut page, vec![Event::PointerMoved(find(&out, "line 59").center())]);
    let wheel = Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, 1.0), phase: egui::TouchPhase::Move, modifiers: Default::default() };
    assert_eq!(probe.pass_twice(&mut page, vec![wheel]), 2);
    assert_eq!(page.log_scroll, 1, "the notch moved the log once");
}

#[test]
fn reset_count_write_again_and_update_all_ask_for_the_cells_acts() {
    let _en = english();
    // Reset on a next write: the cells' Reset, the choice let go.
    let probe = Probe::new(BOARD);
    let (mut page, ends) = page_with(Ends::next_write);
    let out = probe.settle(&mut page);
    probe.click(&mut page, find(&out, "Reset").center());
    assert_eq!(ends.sent(), [Cmd::Choose { port: "COM3".into(), choice: None }]);
    // Count free space on a card not counted: the cells' Count.
    let probe = Probe::new(BOARD);
    let uncounted = fixtures::answering_with("COM3", "v0.8.0", |s| s.free = crate::device::board::Free::NotCounted);
    let (mut page, ends) = page_with(|e| e.boards(vec![uncounted]));
    let out = probe.settle(&mut page);
    probe.click(&mut page, find(&out, "Count free space").center());
    assert_eq!(ends.sent(), [Cmd::Count { port: "COM3".into() }]);
    // Write again, in Details: the cells' WriteAgain, its gate.
    let probe = Probe::new(BOARD);
    let (mut page, _ends) = page_with(Ends::up_to_date);
    page.act(Act::Details);
    let out = probe.settle(&mut page);
    probe.click(&mut page, find(&out, "Write v0.8.0 again").center());
    assert!(page.gate.as_ref().is_some_and(|g| g.kind == Kind::Again), "the cells' WriteAgain: its gate");
    // Update all at the row's right edge: its gate over the boards it
    // writes, nothing written before its yes.
    let probe = Probe::new(BOARD);
    let boards = vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0"), fixtures::answering("COM9", "v0.8.0")];
    let (mut page, ends) = page_with(|e| e.boards(boards));
    let out = probe.settle(&mut page);
    probe.click(&mut page, find(&out, "Update all (2)").center());
    assert!(page.gate.as_ref().is_some_and(|g| g.kind == Kind::All), "the cells' UpdateAll: its gate");
    assert!(ends.sent().is_empty());
}

#[test]
fn under_the_write_the_tabs_still_switch_as_the_cells_let_them_and_nothing_else_is_offered() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = page_with(Ends::two);
    page.act(Act::Board(1));
    page.act(Act::Primary);
    page.act(Act::Write);
    assert!(page.writing() && matches!(ends.sent().as_slice(), [Cmd::Write { port, .. }] if port == "COM5"));
    let out = probe.settle(&mut page);
    assert_drew(&out, &["writing COM5…"]);
    assert!(!drew(&out, "Advanced…"), "nothing that writes or chooses under the lock");
    // COM3's tab: the cells register it under the lock, and the view moves
    // to the board not being written, which offers nothing either.
    probe.click_id(&mut page, tab_id("COM3"));
    assert_eq!(page.open.as_deref(), Some("COM3"));
    let out = probe.settle(&mut page);
    assert!(!drew(&out, "Show on the player") && !drew(&out, "Advanced…"), "{:?}", texts(&out));
    assert!(ends.sent().is_empty(), "and the lock sends nothing");
}

#[test]
fn a_press_on_details_held_while_the_primary_comes_in_opens_details_never_the_gate() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    // COM3 listing its library, its bottom row the door alone; it answers
    // on v0.7.0 while the button is held on Details, and UPDATE comes in at
    // the row's end: the release opens Details, never the update's gate.
    let (mut page, ends) = page_with(|e| e.boards(vec![fixtures::starting("COM3", Some("v0.7.0"))]));
    let out = probe.settle(&mut page);
    assert!(!drew(&out, "UPDATE"), "no primary while it lists its library");
    probe.press_held_through(&mut page, find(&out, "Details").center(), |page| {
        ends.tell([crate::device::desk::Event::Board(fixtures::old("COM3", "v0.7.0"))]);
        page.pump();
    });
    assert!(page.details && page.gate.is_none(), "the press on Details opened Details, not the gate");
    assert!(drew(&probe.settle(&mut page), "UPDATE"), "the primary came in under the held button");
}

#[test]
fn a_press_on_a_tab_held_while_a_board_is_plugged_in_before_it_opens_its_own_board() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    // Two boards, the press on COM5's tab, and COM4 plugged in before it
    // while the button is held: the release opens COM5, never COM4.
    let (mut page, ends) = page_with(Ends::two);
    let out = probe.settle(&mut page);
    let tab = find(&out, "COM5");
    assert!(tab.max.y < ROOMY.top + ROW_H, "COM5's tab on the subjects row: {tab:?}");
    probe.press_held_through(&mut page, tab.center(), |page| {
        let ports = ["COM3", "COM4", "COM5"].map(String::from).to_vec();
        let plugged = fixtures::answering("COM4", "v0.8.0");
        ends.tell([crate::device::desk::Event::Board(plugged), crate::device::desk::Event::Watch { ports, others: Vec::new() }]);
        page.pump();
    });
    assert_eq!(page.boards.iter().map(|b| b.port()).collect::<Vec<_>>(), ["COM3", "COM4", "COM5"], "COM4 came in before COM5");
    assert_eq!(page.open.as_deref(), Some("COM5"), "the press on COM5's tab opened COM5");
}

#[test]
fn a_press_on_a_primary_held_while_its_board_is_unplugged_asks_nothing_of_the_next() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    // Two boards a release behind, the press on COM3's UPDATE, and COM3
    // unplugged while the button is held, the view going to COM5: the
    // release asks nothing of COM5.
    let (mut page, ends) = page_with(|e| e.boards(vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0")]));
    let out = probe.settle(&mut page);
    probe.press_held_through(&mut page, find(&out, "UPDATE").center(), |page| {
        ends.tell([crate::device::desk::Event::Gone { port: "COM3".into() }]);
        page.pump();
    });
    assert_eq!(page.open.as_deref(), Some("COM5"), "the view went to the board left");
    assert!(page.gate.is_none(), "a press on a board gone opens no gate on another");
    assert!(ends.sent().is_empty());
}

/// A log line about COM3, as the worker tells one.
fn log_line(text: &str) -> crate::device::desk::Event {
    crate::device::desk::Event::Log { port: Some("COM3".into()), text: text.into(), kind: LogKind::Fact }
}

#[test]
fn nothing_plugged_in_draws_the_empty_card_and_its_door_opens_the_hints() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, _ends) = page_with(Ends::no_board);
    let out = probe.settle(&mut page);
    assert_drew(&out, &["Nothing plugged in", "watching the USB ports and card readers…", "Not showing up?"]);
    assert!(!drew(&out, "a USB-C cable that carries data — some only charge"), "the hints behind the door");
    probe.click(&mut page, find(&out, "Not showing up?").center());
    assert!(page.details, "the door is Details' flag, as `d`");
    let out = probe.settle(&mut page);
    assert_drew(&out, &["a USB-C cable that carries data — some only charge", "•"]);
    assert_drew(&out, &["Serial ports seen: COM1 — not a Core2's bridge"]);
}

#[test]
fn the_gate_leaves_the_page_inert_and_answers_with_the_cells_acts() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = page_with(Ends::update_available);
    let out = probe.settle(&mut page);
    let (primary, details) = (find(&out, "UPDATE").center(), find(&out, "Details").center());
    probe.click(&mut page, primary);
    assert!(page.gate.is_some(), "the primary's act, the cells' Primary: the gate");
    assert!(ends.sent().is_empty(), "nothing is written before the gate's yes");
    let out = probe.settle(&mut page);
    assert_drew(&out, &["Update from v0.7.0 to v0.8.0?"]);
    // Beneath it the page takes no click.
    probe.click(&mut page, details);
    assert!(!page.details && page.gate.is_some(), "the page under the gate is inert");
    // The safe answer keeps.
    let _ = probe.settle(&mut page);
    probe.click_id(&mut page, Id::new("mp3-gate").with("foot").with(0usize));
    assert!(page.gate.is_none() && ends.sent().is_empty(), "keep: nothing sent");
    // The other writes, and the page is locked from the yes.
    let out = probe.settle(&mut page);
    probe.click(&mut page, find(&out, "UPDATE").center());
    let _ = probe.settle(&mut page);
    probe.click_id(&mut page, Id::new("mp3-gate").with("foot").with(1usize));
    assert!(matches!(ends.sent().as_slice(), [Cmd::Write { port, .. }] if port == "COM3"), "the cells' Write");
    assert!(page.writing());
}

#[test]
fn a_write_under_way_draws_its_words_and_its_bar_and_the_lock_keeps_only_what_the_cells_keep() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = page_with(Ends::update_available);
    page.act(Act::Primary);
    page.act(Act::Write);
    let _ = ends.sent();
    ends.writing(62);
    page.pump();
    let out = probe.settle(&mut page);
    assert_drew(&out, &["writing v0.8.0… 62%"]);
    assert!(!drew(&out, "UPDATE"), "no primary on the board being written");
    // The bar: the write's share of the value column's segments lit in the
    // accent, as the cells fill their bar.
    let pal = Pal::now();
    let segments = |fill: Color32| out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == fill && r.rect.height() == 10.0)).count();
    let (lit, unlit) = (segments(pal.accent), segments(pal.unlit));
    assert!(lit + unlit > 20, "the bar across the value column: {lit} lit, {unlit} not");
    assert_eq!(lit, studio::fill_lit(620, lit + unlit), "62% of {} segments lit", lit + unlit);
    // Details still opens under the lock, as `d` does.
    probe.click(&mut page, find(&out, "Details").center());
    assert!(page.details && page.writing());
    assert!(ends.sent().is_empty(), "and nothing is asked of a board being written");
}

#[test]
fn at_the_guis_floor_the_card_tightens_and_keeps_its_bottom_row_whole() {
    let _en = english();
    let probe = Probe::new(FLOOR);
    // Two boards, the one in view reporting its card and with a next
    // write chosen: a card the boards' measures leave no room for here.
    let (mut page, _ends) = page_with(|e| {
        let board = fixtures::answering("COM3", "v0.8.0");
        let chosen = fixtures::chosen(board, crate::device::firmware::Image::Pin(crate::device::firmware::Mode::Dio), crate::device::board::By::Sheet, None);
        e.boards(vec![chosen, fixtures::old("COM5", "v0.7.0")]);
    });
    let out = probe.settle(&mut page);
    // The subjects row's middle, as its tab takes the row: its words stand
    // on the face's baseline in it, as high over its middle as the face
    // leaves room under its letters.
    let row = probe.ctx.read_response(tab_id("COM3")).expect("COM3's tab").rect.center().y;
    assert!(row < ROOMY.top + ROW_H / 2.0 - 0.5 && row > TIGHT.top + ROW_H / 2.0 - 0.5, "closed up: the row nearer the top, never nearer than the tighter set's: {row}");
    // The primary whole in the card's room, as its clip says, over the
    // foot's padding (the tighter set's at the least); the room over the
    // busy line.
    let primary = find(&out, "WRITE");
    let room = card_room_of(&out, "WRITE");
    let bottom = primary.center().y + studio::TALL_H / 2.0;
    assert!(bottom <= room.max.y - TIGHT.pad_y + 0.5, "the primary whole on the card: {primary:?}, {bottom} in {room:?}");
    let busy_top = FLOOR.y - TIGHT.under_busy - studio::NOTE_H;
    assert!(room.max.y <= busy_top - TIGHT.over_busy + 0.5, "the card's room over the busy line: {room:?}");
    assert_drew(&out, &["COM3", "COM5"]);
}

#[test]
fn a_card_a_fraction_of_a_point_too_tall_for_its_room_tightens_and_keeps_its_every_line() {
    let _en = english();
    let (mut page, _ends) = page_with(Ends::up_to_date);
    let roomy = Probe::new(BOARD);
    let whole: Vec<String> = texts(&roomy.settle(&mut page)).into_iter().map(|(t, _)| t).collect();
    // The card's height at rest with the boards' measures, as the frame's
    // invisible run measures it, and the room the frame leaves it there.
    let (mut rest, mut room) = (0.0_f32, 0.0_f32);
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BOARD)), ..Default::default() };
    let mut out = roomy.ctx.run_ui(input, |ui| {
        let pal = Pal::now();
        let full = ui.max_rect();
        let face = Face { pal: &pal, composition: "", key_hints: false, full, space: ROOMY, ports: &["COM3"], live: false, inert: false };
        let at = parts(full, ROOMY, &page);
        room = at.card.height();
        rest = subject(ui, &mut page, &face, Rect::from_min_size(at.card.min, vec2(at.card.width(), 100_000.0))).rest;
    });
    out.textures_delta.clear();
    // An area that leaves the card a few tenths of a point less than that:
    // the boards' measures would stand its last value line past the floor,
    // so the frame closes up and every line stands — a point or two, as
    // far as the card needs, never the whole way to the tighter set.
    let short = Probe::new(vec2(BOARD.x, BOARD.y - room + rest - 0.3));
    let out = short.settle(&mut page);
    let drawn: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
    let lost: Vec<&String> = whole.iter().filter(|t| !drawn.contains(t)).collect();
    assert!(lost.is_empty(), "every line of the card stands: {lost:?} left out");
    let head = find(&out, "M5Stack Core2").center().y;
    assert!(head < ROOMY.top + head_mid(ROOMY) - 0.5, "closed up: {head}");
    assert!(head > TIGHT.top + head_mid(TIGHT) + 4.0, "only as far as it needs: {head}");
}

/// COM3 beside a card, its write failed past 40% with `lines` lines of its
/// log before the failure and its hint, as the Failed board has it.
fn failed_beside_a_card(lines: usize) -> Page {
    let (mut page, ends) = with_card(Ends::update_available);
    page.act(Act::Primary);
    page.act(Act::Write);
    ends.writing(38);
    ends.tell((0..lines).map(|i| log_line(&format!("wrote chunk {i:02}"))));
    let fail = |text: &str, kind| crate::device::desk::Event::Log { port: Some("COM3".into()), text: text.into(), kind };
    ends.tell([fail("the board stopped answering", LogKind::Fail), fail("Try again; if it keeps failing, use another cable.", LogKind::Quiet)]);
    ends.failed();
    page.pump();
    assert!(page.details, "a failure opens Details by itself");
    page
}

/// The log lines a pass drew, top to foot.
fn log_drawn(out: &FullOutput) -> Vec<(String, Rect)> {
    let mut lines: Vec<(String, Rect)> = texts(out).into_iter().filter(|(t, _)| t.starts_with("wrote chunk") || t.starts_with("the board stopped") || t.starts_with("Try again;")).collect();
    lines.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
    lines
}

#[test]
fn at_the_guis_default_size_a_failures_log_takes_the_room_under_the_card_down_to_the_busy_line() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let mut page = failed_beside_a_card(40);
    let out = probe.settle(&mut page);
    // Its newest lines, as many as fit and at least a write's last rung:
    // the frame closes up for them rather than leave the busy line's room
    // blank under two.
    let lines = log_drawn(&out);
    assert!(lines.len() >= super::board::LOG_REST, "the log's newest lines: {:?}", lines.iter().map(|(t, _)| t).collect::<Vec<_>>());
    assert_eq!(lines.last().map(|(t, _)| t.as_str()), Some("Try again; if it keeps failing, use another cable."), "following the newest");
    assert_drew(&out, &["TRY AGAIN", "Write failed · the board is half written", "PORT"]);
    // The card spent down to its room's foot: no room for another line
    // left under the log.
    let (last, room) = (lines.last().expect("a line").1, card_room_of(&out, "the board stopped answering"));
    assert!(room.max.y - last.max.y < 18.0 + ROOMY.pad_y, "the log down to the card's foot: {last:?} in {room:?}");
    let busy_top = WINDOW.y - TIGHT.under_busy - studio::NOTE_H;
    assert!(room.max.y >= busy_top - ROOMY.over_busy - 0.5, "the card's room down to the busy line: {room:?}");
}

#[test]
fn at_the_guis_default_size_details_after_a_write_keeps_its_headline_its_links_its_newest_lines_and_its_facts() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = with_card(Ends::update_available);
    page.act(Act::Primary);
    page.act(Act::Write);
    ends.writing(62);
    ends.tell((0..30).map(|i| log_line(&format!("wrote chunk {i:02}"))));
    // Its firmware the one that formats (`clock=`): the format is offered.
    let mut board = fixtures::answering_with("COM3", "v0.8.0", |s| s.clock = crate::device::board::Clock::Ok);
    board.probe = Some(Ok(crate::device::board::Probe { info: fixtures::info("COM3"), on_board: Some(fixtures::ours("v0.8.0")) }));
    board.written = Some(Written::Done {
        version: "v0.8.0".into(),
        took: std::time::Duration::from_secs(41),
        skipped: false,
        install: false,
        boot: None,
        image: crate::device::firmware::tests::pin_facts("v0.8.0", crate::device::firmware::Mode::Qio),
        looping: None,
    });
    ends.tell([crate::device::desk::Event::Board(fixtures::judged(board))]);
    page.pump();
    page.act(Act::Details);
    let out = probe.settle(&mut page);
    // Its headline, the Firmware row with its verdict, still says what the
    // board is; Write again and the format, its log's newest lines and its
    // facts from the top: no control gone at the size most windows are. The
    // other value line, the SD card's, gave way first, then the last fact.
    assert_drew(&out, &["FIRMWARE", "Up to date · v0.8.0 · just written", "Write v0.8.0 again", "Format as FAT32…", "wrote chunk 29", "wrote chunk 28"]);
    assert_drew(&out, &["PORT", "BOARD", "ON THE BOARD", "THIS PLAYER", "WRITTEN"]);
    assert!(!drew(&out, "SD CARD"), "the other value line gave way first");
    assert!(find(&out, "FIRMWARE").max.y < find(&out, "Details").min.y, "the headline over the bottom row");
    let (format, room) = (find(&out, "Format as FAT32…"), card_room_of(&out, "Format as FAT32…"));
    assert!(format.max.y <= room.max.y - TIGHT.pad_y + 0.5 && room.max.y - format.max.y < 18.0 + ROOMY.pad_y + 6.0, "the card spent down to its foot: {format:?} in {room:?}");
}

#[test]
fn at_the_guis_default_size_a_cards_details_keeps_its_holds_its_space_next_sync_its_log_and_sync_again() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = with_card(Ends::up_to_date);
    page.act(Act::Card(0));
    let key = page.cards()[0].key.clone();
    let said = |text: &str| crate::device::desk::Event::Log { port: Some(key.clone()), text: text.into(), kind: LogKind::Fact };
    ends.tell([said("E: put in: Car music, revision 42"), said("asked Home: 12 new, 2 removed")]);
    page.pump();
    page.act(Act::Details);
    let out = probe.settle(&mut page);
    // Its headline, Holds, still says what the card is, and its Space, bar
    // and figures, how full it is; Next sync, what Sync will do; Details'
    // log and Sync again, its facts from the top.
    assert_drew(&out, &["HOLDS", "Rock, Jazz from Home", "SPACE", "41.2 of 63.8 GB used · 4,812 tracks", "22.6 GB free", "NEXT SYNC", "SYNC"]);
    assert_drew(&out, &["DRIVE", "E: put in: Car music, revision 42", "asked Home: 12 new, 2 removed", "Sync again"]);
    let (lit, unlit) = (filled(&out, Pal::now().accent, 10.0).len(), filled(&out, Pal::now().unlit, 10.0).len());
    assert!(lit > 0 && lit + unlit > 60, "the Space's bar across the value column: {lit} lit, {unlit} not");
    // The other value rows gave way first, then the facts from the foot.
    assert!(!drew(&out, "LAST SYNC") && !drew(&out, "CONVERTS"), "the other value rows gave way first");
    assert!(!drew(&out, "REMOVES") && !drew(&out, "PLAYS"), "then the facts from the foot");
    let again = find(&out, "Sync again");
    assert!(again.max.y <= card_room_of(&out, "Sync again").max.y - TIGHT.pad_y + 0.5, "Sync again whole on the card: {again:?}");
}

#[test]
fn a_little_over_the_guis_floor_the_room_a_cards_space_leaves_goes_to_its_facts_before_more_of_its_log() {
    let _en = english();
    let (mut page, ends) = with_card(Ends::up_to_date);
    page.act(Act::Card(0));
    let key = page.cards()[0].key.clone();
    let said = |text: String| crate::device::desk::Event::Log { port: Some(key.clone()), text, kind: LogKind::Fact };
    ends.tell((0..12).map(|i| said(format!("asked Home: step {i:02}"))));
    page.pump();
    page.act(Act::Details);
    // At 27 and 26 rows the Space, whole, has no room beside Details' rule,
    // Sync again and the log's newest two lines, and gives way; the room it
    // leaves is the facts' from the top before it is the log's older lines'.
    for h in [400.0, 384.0] {
        let out = Probe::new(vec2(WINDOW.x, h)).settle(&mut page);
        let words = || texts(&out).into_iter().map(|(t, _)| t).collect::<Vec<_>>();
        assert_drew(&out, &["HOLDS", "NEXT SYNC", "SYNC", "Sync again", "asked Home: step 11", "asked Home: step 10"]);
        assert!(!drew(&out, "SPACE"), "{h}: the Space gave way whole: {:?}", words());
        assert!(drew(&out, "DRIVE"), "{h}: the first fact kept in the room the Space left: {:?}", words());
        assert!(find(&out, "DRIVE").max.y < find(&out, "asked Home: step 10").min.y, "{h}: the fact over the log");
    }
}

/// Where the head's name stands its middle under the column's top with
/// `space`.
fn head_mid(space: Space) -> f32 {
    studio::HEAD_H / 2.0 + space.pad_y
}

#[test]
fn the_busy_line_sets_its_figures_and_the_boards_ports_in_the_monospace() {
    let _en = english();
    let probe = Probe::new(BOARD);
    let (mut page, _ends) = page_with(Ends::up_to_date);
    let words = "Updated from v0.7.0 just now. COM3 answered.";
    page.say(words, false, None);
    let out = probe.settle(&mut page);
    let galley = out
        .shapes
        .iter()
        .find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == words => Some(t.galley.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{words:?} drawn: {:?}", texts(&out)));
    let runs: Vec<(String, egui::FontId)> = galley.job.sections.iter().map(|s| (galley.job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(), s.format.font_id.clone())).collect();
    let mono_runs: Vec<&str> = runs.iter().filter(|(_, f)| *f == studio::mono(12.0)).map(|(t, _)| t.as_str()).collect();
    assert_eq!(mono_runs, ["v0.7.0", "COM3"], "the version and the port in the monospace a point under the words: {runs:?}");
    assert!(runs.iter().all(|(_, f)| *f == studio::mono(12.0) || *f == body(13.0)), "the rest the words' 13 points: {runs:?}");
}

#[test]
fn a_tabs_check_stands_centred_on_the_subjects_row_as_the_boards_centre_its_box() {
    let _en = english();
    let probe = Probe::new(BOARD);
    let (mut page, _ends) = page_with(Ends::two);
    let out = probe.settle(&mut page);
    // The kit's check: a three-point stroke 1.7 wide, its ink spanning its
    // 10-point box's middle.
    let checks: Vec<Rect> = out
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Path(p) if p.points.len() == 3 && (p.stroke.width - 1.7).abs() < 0.01 => Some(Rect::from_points(&p.points)),
            _ => None,
        })
        .filter(|r| r.max.y < ROOMY.top + ROW_H)
        .collect();
    assert_eq!(checks.len(), 1, "COM3's check on its slab: {checks:?}");
    let mid = ROOMY.top + ROW_H / 2.0;
    assert!((checks[0].center().y - mid).abs() < 0.5, "centred on the row's middle {mid}: {:?}", checks[0]);
}

#[test]
fn the_measures_close_up_in_whole_points_from_the_boards_to_the_tighter_set() {
    assert_eq!(Space::at(0.0), ROOMY);
    assert_eq!(Space::at(1.0), TIGHT);
    assert_eq!(Space::at(-1.0), ROOMY, "no roomier than the boards'");
    assert_eq!(Space::at(2.0), TIGHT, "no tighter than the tighter set");
    let half = Space::at(0.5);
    for (k, v) in [half.top, half.gap, half.pad_x, half.pad_y, half.over_busy, half.under_busy].into_iter().enumerate() {
        assert_eq!(v, v.floor(), "measure {k} a whole number of points: {half:?}");
    }
    assert_eq!(half.gap, 14.0, "half way: {half:?}");
    assert!(TIGHT.top >= 16.0, "never over the kit's modal's margin: a sheet held at its highest covers the subjects row");
}

#[test]
fn the_frame_closes_up_once_for_a_window_size_and_holds_there_till_the_window_changes() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = page_with(Ends::up_to_date);
    ends.tell((0..30).map(|i| log_line(&format!("line {i:02}"))));
    page.pump();
    let head = |out: &FullOutput| find(out, "M5Stack Core2");
    let at_rest = head(&probe.settle(&mut page));
    // Details open counts at rest: the frame closes up to show it.
    page.act(Act::Details);
    let out = probe.settle(&mut page);
    let open = head(&out);
    assert!(open.center().y < at_rest.center().y - 0.5, "Details open closes the frame up: {at_rest:?} then {open:?}");
    assert_drew(&out, &["PORT", "line 29"]);
    // Shut again, at the same size, nothing moves.
    page.act(Act::Details);
    let shut = head(&probe.settle(&mut page));
    assert_eq!(shut, open, "the frame holds while the window keeps its size");
    // Another size is measured afresh, and back again from the boards'.
    let wide = Probe { ctx: probe.ctx.clone(), size: BOARD };
    let _ = wide.settle(&mut page);
    let again = head(&probe.settle(&mut page));
    assert_eq!(again, at_rest, "a window that changed opens the frame up again");
}

/// The page's areas in points from the GUI's default window to where the
/// frame no longer closes up for a write's log: 28 to 36 rows.
fn default_sizes() -> impl Iterator<Item = Vec2> {
    (28..=36).map(|rows| vec2(800.0, rows as f32 * 16.0))
}

#[test]
fn a_write_under_way_with_details_open_moves_nothing_of_the_page_as_its_log_grows() {
    let _en = english();
    for size in default_sizes() {
        let probe = Probe::new(size);
        let (mut page, ends) = with_card(Ends::update_available);
        page.act(Act::Primary);
        page.act(Act::Write);
        ends.writing(5);
        page.pump();
        page.act(Act::Details);
        let head = |out: &FullOutput| find(out, "M5Stack Core2");
        let open = head(&probe.settle(&mut page));
        // A line at a time, as the write tells them: the frame closed up
        // for its log when Details opened, and holds there.
        for i in 0..10 {
            ends.tell([log_line(&format!("wrote chunk {i:02}"))]);
            page.pump();
            let out = probe.settle(&mut page);
            assert_eq!(head(&out), open, "the head held after line {i} at {size:?}");
            assert!(drew(&out, &format!("wrote chunk {i:02}")), "the newest line drawn at {size:?}");
        }
    }
}

#[test]
fn a_sync_under_way_with_details_open_moves_nothing_of_the_page_as_its_log_grows() {
    let _en = english();
    for size in default_sizes() {
        let probe = Probe::new(size);
        let (mut page, ends) = with_card(Ends::up_to_date);
        page.act(Act::Card(0));
        let key = page.cards()[0].key.clone();
        page.transfer.as_ref().expect("the cards' worker").fold.borrow_mut().apply(worker::Event::Card(Box::new(cards::syncing("E:", now()))));
        page.act(Act::Details);
        let head = |out: &FullOutput| find(out, "Car music");
        let open = head(&probe.settle(&mut page));
        for i in 0..10 {
            ends.tell([crate::device::desk::Event::Log { port: Some(key.clone()), text: format!("copied track {i:02}"), kind: LogKind::Fact }]);
            page.pump();
            let out = probe.settle(&mut page);
            assert_eq!(head(&out), open, "the head held after line {i} at {size:?}");
            assert!(drew(&out, &format!("copied track {i:02}")), "the newest line drawn at {size:?}");
        }
    }
}

#[test]
fn the_waiting_line_says_its_words_under_the_blank_first_row() {
    let _en = english();
    let ctx = Probe::new(WINDOW).ctx;
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)), ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| waiting(ui, "letting the USB ports go…"));
    out.textures_delta.clear();
    let at = find(&out, "letting the USB ports go…");
    assert!(at.min.y >= ROOMY.top - 1.0 && at.max.y < 60.0, "{at:?}");
}

// ── The modals ──────────────────────────────────────────────────────────

#[test]
fn the_advanced_sheet_draws_and_closes_and_its_field_gives_a_caret_only_with_the_keyboard() {
    let _en = english();
    let probe = Probe::new(BOARD);
    let (mut page, _ends) = page_with(Ends::up_to_date);
    page.act(Act::Advanced);
    assert!(page.sheet.is_some());
    let _ = probe.settle(&mut page);
    let (out, shown) = probe.pass(&mut page, Vec::new());
    assert_drew(&out, &["Advanced options · COM3"]);
    assert!(shown.caret.is_none(), "no field has the keyboard: no caret");
    if let Some(sheet) = page.sheet.as_mut() {
        sheet.field = Some(tui_input::Input::new("C:/builds/core2".into()));
    }
    let _ = probe.settle(&mut page);
    let (_, shown) = probe.pass(&mut page, Vec::new());
    assert!(shown.caret.is_some(), "the path field with the keyboard: its caret for the input method");
    if let Some(sheet) = page.sheet.as_mut() {
        sheet.field = None;
    }
    let _ = probe.settle(&mut page);
    probe.click_id(&mut page, Id::new("mp3-sheet").with("close"));
    assert!(page.sheet.is_none(), "its close, the cells' SheetClose");
}

/// A page with the board `tell` says and one card in a reader, on channels
/// the test holds.
fn with_card(tell: impl FnOnce(&Ends)) -> (Page, Ends) {
    let (mut page, ends) = Page::quiet();
    let (link, _inbox) = Link::pair();
    let fold = Rc::new(RefCell::new(Fold::default()));
    {
        let mut f = fold.borrow_mut();
        f.watched = true;
        f.servers = vec![cards::home()];
        f.apply(worker::Event::Card(Box::new(cards::ready("E:", now()))));
    }
    page.attach(Transfer { link, fold });
    tell(&ends);
    page.pump();
    (page, ends)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[test]
fn a_card_shares_the_subjects_row_and_its_tab_opens_its_view() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, _ends) = with_card(Ends::update_available);
    let out = probe.settle(&mut page);
    let tab = if cfg!(windows) { "E: Car music" } else { "Car music" };
    assert_drew(&out, &["COM3", tab]);
    let key = page.cards()[0].key.clone();
    probe.click_id(&mut page, tab_id(&key));
    assert_eq!(page.card_open, Some(key), "the cells' Card(0)");
    let out = probe.settle(&mut page);
    assert_drew(&out, &["Car music"]);
}

/// A page with no board and `card` in a reader, the transfer worker's
/// inbox kept to read what the page asks of it.
fn with_one_card(card: worker::CardView) -> (Page, std::sync::mpsc::Receiver<worker::In>) {
    let (page, inbox, _fold) = with_one_card_told(card);
    (page, inbox)
}

/// The same, with the fold the worker tells the page's cards through.
fn with_one_card_told(card: worker::CardView) -> (Page, std::sync::mpsc::Receiver<worker::In>, Rc<RefCell<Fold>>) {
    let (mut page, _ends) = Page::quiet();
    let (link, inbox) = Link::pair();
    let fold = Rc::new(RefCell::new(Fold::default()));
    {
        let mut f = fold.borrow_mut();
        f.watched = true;
        f.servers = vec![cards::home()];
        f.apply(worker::Event::Card(Box::new(card)));
    }
    page.attach(Transfer { link, fold: fold.clone() });
    page.pump();
    (page, inbox, fold)
}

#[test]
fn a_press_on_a_cards_link_that_goes_before_its_release_does_nothing_of_another() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    // A card syncing, the press on its stop, and the sync ending while the
    // button is held: the stop is gone, and the release asks for nothing —
    // not the Settings sheet, Details or the sync's gate in its place.
    let (mut page, inbox, fold) = with_one_card_told(cards::syncing("E:", now()));
    let out = probe.settle(&mut page);
    let stop = t!("dev.card.link_stop").to_string();
    probe.press_held_through(&mut page, find(&out, &stop).center(), |_| {
        fold.borrow_mut().apply(worker::Event::Card(Box::new(cards::ready("E:", now()))));
    });
    let out = probe.settle(&mut page);
    assert!(!drew(&out, &stop) && drew(&out, "Settings…"), "the sync ended under the held button: {:?}", texts(&out));
    assert!(page.card.sheet.is_none() && page.card.gate.is_none() && !page.details, "the press on the stop did nothing of another control's");
    assert!(worker::sent(&inbox).is_empty(), "nothing asked of the card");
}

#[test]
fn a_cards_gate_draws_over_the_inert_page_and_its_safe_answer_is_the_cells_keep() {
    let _en = english();
    let probe = Probe::new(BOARD);
    let many: Vec<String> = (0..214).map(|i| format!("Velvet Static/Harbour Songs/{i:03} - Track.flac")).collect();
    let card = cards::with(cards::ready("E:", now()), |c| {
        if let worker::Next::Plan(plan) = &mut c.next {
            plan.removals = many.clone();
        }
    });
    let (mut page, inbox) = with_one_card(card.clone());
    // The Details door's lead, at the card's content's left: the gate's
    // window, the column's middle, stands clear of it.
    let door = find(&probe.settle(&mut page), "Details");
    let beneath = pos2(door.min.x - 8.0, door.center().y);
    page.act(Act::Primary);
    assert!(page.card.gate.is_some() && worker::sent(&inbox).is_empty(), "the gate first");
    let _ = probe.settle(&mut page);
    let (out, shown) = probe.pass(&mut page, Vec::new());
    let title = "Remove 214 tracks from Car music?";
    assert_drew(&out, &[title]);
    assert!(shown.caret.is_none(), "a gate has no field");
    // The card beneath is inert: a click on its Details door, off the
    // gate's window, is the scrim's.
    let pal = Pal::now();
    let window = out
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Rect(r) if r.fill == pal.panel && r.rect.contains_rect(find(&out, title)) => Some(r.rect),
            _ => None,
        })
        .min_by(|a, b| a.area().total_cmp(&b.area()))
        .expect("the gate's window");
    assert!(!window.contains(beneath), "the door off the gate's window: {beneath:?} and {window:?}");
    probe.click(&mut page, beneath);
    assert!(page.card.gate.is_some() && !page.details, "the card's Details door takes no click under its gate");
    assert!(worker::sent(&inbox).is_empty(), "and nothing is asked of the card");
    probe.click_id(&mut page, Id::new("mp3-card-gate").with("foot").with(0usize));
    assert!(page.card.gate.is_none(), "the cells' CardKeep");
    assert_eq!(worker::sent(&inbox), [worker::Cmd::Dismiss { key: card.key.clone() }]);
    // The stronger gate, its go waiting on its box: the keep alone answers.
    let strong = cards::with(card, |c| {
        if let worker::Next::Plan(plan) = &mut c.next {
            plan.guards = vec![crate::device::card::engine::Guard::Removals { paths: many.clone(), strong: true }];
            plan.removal_libs = (0..214).map(|i| if i < 200 { "Rock" } else { "Jazz" }.to_string()).collect();
        }
    });
    let (mut page, inbox) = with_one_card(strong);
    page.act(Act::Primary);
    let half = |page: &Page| matches!(page.card.gate.as_ref().map(|g| &g.kind), Some(super::super::card::GateKind::Half { ticked: false, .. }));
    assert!(half(&page), "the stronger gate, its box not ticked");
    let _ = probe.settle(&mut page);
    probe.click_id(&mut page, Id::new("mp3-card-gate").with("foot").with(0usize));
    assert!(page.card.gate.is_none() && matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::Dismiss { .. }]));
}

#[test]
fn a_cards_sheet_draws_and_closes_and_its_name_field_gives_a_caret_only_with_the_keyboard() {
    let _en = english();
    let probe = Probe::new(BOARD);
    let (mut page, inbox) = with_one_card(cards::new_card("F:"));
    page.act(Act::Primary);
    assert!(page.card.sheet.is_some(), "a new card's primary: its set-up sheet");
    let _ = probe.settle(&mut page);
    let (out, shown) = probe.pass(&mut page, Vec::new());
    assert_drew(&out, &["Set up F:"]);
    assert!(shown.caret.is_none(), "no field has the keyboard: no caret");
    if let Some(sheet) = page.card.sheet.as_mut() {
        sheet.field = Some(tui_input::Input::new("Gym".into()));
    }
    let _ = probe.settle(&mut page);
    let (_, shown) = probe.pass(&mut page, Vec::new());
    assert!(shown.caret.is_some(), "the Name field with the keyboard: its caret for the input method");
    if let Some(sheet) = page.card.sheet.as_mut() {
        sheet.field = None;
    }
    let _ = probe.settle(&mut page);
    probe.click_id(&mut page, Id::new("mp3-card-sheet").with("close"));
    assert!(page.card.sheet.is_none(), "its close, the cells' CardSheetClose");
    assert!(worker::sent(&inbox).is_empty(), "a sheet closed writes nothing");
}

// ── The boards' type for the cells' marks and figures ───────────────────

/// The galley a pass drew with the text `words`, within `rows` of the
/// area's height, and where.
fn galley_in(out: &FullOutput, words: &str, rows: std::ops::Range<f32>) -> (Pos2, std::sync::Arc<egui::Galley>) {
    out.shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(t) if t.galley.text() == words && rows.contains(&t.pos.y) => Some((t.pos, t.galley.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{words:?} drawn: {:?}", texts(out)))
}

/// The faces `words` was set in, run by run.
fn fonts_of(out: &FullOutput, words: &str) -> Vec<FontId> {
    galley_in(out, words, f32::MIN..f32::MAX).1.job.sections.iter().map(|s| s.format.font_id.clone()).collect()
}

/// Where `words`' first letter stands on its baseline.
fn baseline(out: &FullOutput, words: &str) -> f32 {
    let (pos, galley) = galley_in(out, words, f32::MIN..f32::MAX);
    let row = &galley.rows[0];
    pos.y + row.pos.y + row.row.glyphs[0].pos.y
}

#[test]
fn a_failed_boards_tab_wears_the_kits_cross_and_a_writes_tab_its_percent_in_the_monospace() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, ends) = page_with(|e| e.boards(vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0")]));
    ends.failed();
    page.pump();
    page.act(Act::Board(1));
    let out = probe.settle(&mut page);
    let no = glyphs().no;
    assert!(!texts(&out).iter().any(|(t, _)| t.contains(no)), "no ✗ set as words: the panel's faces have no such glyph");
    let gold = Pal::now().gold;
    let cross = out.shapes.iter().filter(|c| matches!(&c.shape, Shape::LineSegment { stroke, .. } if stroke.color == gold && stroke.width == 1.6));
    assert_eq!(cross.count(), 2, "COM3's mark, the kit's cross in the gold");
    // A write's percentage is a figure.
    let (mut page, ends) = page_with(Ends::two);
    page.act(Act::Board(1));
    page.act(Act::Primary);
    page.act(Act::Write);
    let mut board = fixtures::old("COM5", "v0.7.0");
    board.work = Work::Writing { phase: Phase::Writing, pct: Some(62) };
    ends.tell([crate::device::desk::Event::Board(board)]);
    page.pump();
    let out = probe.settle(&mut page);
    assert_eq!(fonts_of(&out, "62%"), [mono(13.0)], "the monospace at its one weight, never bold");
}

#[test]
fn the_slab_sets_a_cards_name_in_the_semibold_as_the_cells_bold_it() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, _ends) = with_card(Ends::update_available);
    let tab = if cfg!(windows) { "E: Car music" } else { "Car music" };
    let row = 0.0..ROOMY.top + ROW_H;
    let name_font = |out: &FullOutput| galley_in(out, tab, row.clone()).1.job.sections.last().map(|s| s.format.font_id.clone());
    let out = probe.settle(&mut page);
    assert_eq!(name_font(&out), Some(body(13.5)), "a closed tab's name at rest");
    page.act(Act::Card(0));
    let out = probe.settle(&mut page);
    assert_eq!(name_font(&out), Some(strong(13.5)), "on the slab, the semibold");
    if cfg!(windows) {
        assert_eq!(galley_in(&out, tab, row).1.job.sections[0].format.font_id, mono(13.0), "the drive stays regular");
    }
}

#[test]
fn update_all_at_the_rows_right_edge_is_set_at_the_slots_13_points_flush_with_the_column() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let boards = vec![fixtures::old("COM3", "v0.7.0"), fixtures::old("COM5", "v0.7.0"), fixtures::answering("COM9", "v0.8.0")];
    let (mut page, _ends) = page_with(|e| e.boards(boards));
    let words = "Update all (2)";
    let end = WINDOW.x / 2.0 + COLUMN_W / 2.0;
    let out = probe.settle(&mut page);
    assert!((find(&out, words).max.x - end).abs() < 1.0, "flush at rest: {:?} to {end}", find(&out, words));
    assert!(fonts_of(&out, words).contains(&body(13.0)), "{:?}", fonts_of(&out, words));
    // Under the pointer, the semibold, still flush.
    let _ = probe.pass(&mut page, vec![Event::PointerMoved(find(&out, words).center())]);
    let out = probe.settle(&mut page);
    assert!((find(&out, words).max.x - end).abs() < 1.0, "flush under the pointer: {:?}", find(&out, words));
    assert!(fonts_of(&out, words).contains(&strong(13.0)), "{:?}", fonts_of(&out, words));
}

#[test]
fn the_heads_right_stands_on_the_names_baseline() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, _ends) = page_with(Ends::two);
    let out = probe.settle(&mut page);
    let serial = texts(&out).into_iter().find(|(t, _)| t.starts_with("serial ")).expect("the serial at the head's right").0;
    let (name, right) = (baseline(&out, "M5Stack Core2"), baseline(&out, &serial));
    assert!((name - right).abs() < 0.5, "the name on {name}, the serial on {right}");
}

#[test]
fn a_scan_is_the_kits_searching_shape_its_words_at_13_points_10_after_the_segments() {
    let _en = english();
    let probe = Probe::new(WINDOW);
    let (mut page, _ends) = page_with(|e| e.boards(vec![fixtures::heard("COM3", Heard::Nothing)]));
    let out = probe.settle(&mut page);
    let words = t!("dev.fw_asking").to_string();
    assert_eq!(fonts_of(&out, &words), [body(13.0)]);
    let unlit = Pal::now().unlit;
    let segments = out.shapes.iter().filter_map(|c| match &c.shape {
        Shape::Rect(r) if r.fill == unlit && r.rect.width() == 5.0 => Some(r.rect.max.x),
        _ => None,
    });
    let end = segments.fold(f32::MIN, f32::max);
    assert!((find(&out, &words).min.x - (end + 10.0)).abs() < 0.5, "{:?} after {end}", find(&out, &words));
}
