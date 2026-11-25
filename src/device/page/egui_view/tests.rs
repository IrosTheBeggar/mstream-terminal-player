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
    let (used, free) = (find(&out, "21.4 of 59.6 GB used · 1,284 tracks"), find(&out, "38.2 GB free"));
    assert!((used.center().y - free.center().y).abs() < 2.0 && free.min.x > used.max.x + 16.0, "the free figure at the column's end");
    // The meter: the share's segments lit in the accent across the value
    // column, the rest unlit.
    let pal = Pal::now();
    let lit = out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == pal.accent && r.rect.height() == 10.0)).count();
    let unlit = out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == pal.unlit && r.rect.height() == 10.0)).count();
    assert!(lit > 10 && unlit > lit, "about a third of the card lit: {lit} of {}", lit + unlit);
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
    probe.click_id(&mut page, Id::new("mp3-tab").with(1usize));
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
    probe.click_id(&mut page, Id::new("mp3-tab").with(0usize));
    assert_eq!(page.open.as_deref(), Some("COM3"));
    let out = probe.settle(&mut page);
    assert!(!drew(&out, "Show on the player") && !drew(&out, "Advanced…"), "{:?}", texts(&out));
    assert!(ends.sent().is_empty(), "and the lock sends nothing");
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
    let row = find(&out, "COM3").center().y;
    assert!(row < ROOMY.top + ROW_H / 2.0 - 0.5 && row > TIGHT.top + ROW_H / 2.0 - 0.5, "closed up: the row nearer the top, never nearer than the tighter set's: {row}");
    let primary = find(&out, "WRITE");
    let busy_top = FLOOR.y - TIGHT.under_busy - studio::NOTE_H;
    assert!(primary.center().y + studio::TALL_H / 2.0 <= busy_top + 0.5, "the primary whole over the busy line: {primary:?}");
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

/// Where the head's name stands its middle under the column's top with
/// `space`.
fn head_mid(space: Space) -> f32 {
    studio::HEAD_H / 2.0 + space.pad_y
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
    probe.click_id(&mut page, Id::new("mp3-tab").with(1usize));
    assert!(page.card_open.is_some(), "the cells' Card(0)");
    let out = probe.settle(&mut page);
    assert_drew(&out, &["Car music"]);
}

/// A page with no board and `card` in a reader, the transfer worker's
/// inbox kept to read what the page asks of it.
fn with_one_card(card: worker::CardView) -> (Page, std::sync::mpsc::Receiver<worker::In>) {
    let (mut page, _ends) = Page::quiet();
    let (link, inbox) = Link::pair();
    let fold = Rc::new(RefCell::new(Fold::default()));
    {
        let mut f = fold.borrow_mut();
        f.watched = true;
        f.servers = vec![cards::home()];
        f.apply(worker::Event::Card(Box::new(card)));
    }
    page.attach(Transfer { link, fold });
    page.pump();
    (page, inbox)
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
    page.act(Act::Primary);
    assert!(page.card.gate.is_some() && worker::sent(&inbox).is_empty(), "the gate first");
    let _ = probe.settle(&mut page);
    let (out, shown) = probe.pass(&mut page, Vec::new());
    assert_drew(&out, &["Remove 214 tracks from Car music?"]);
    assert!(shown.caret.is_none(), "a gate has no field");
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
