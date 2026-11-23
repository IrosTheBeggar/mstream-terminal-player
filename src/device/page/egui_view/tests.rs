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
    // The head's port at the card's right, the primary at the content's.
    let (name, port, primary) = (find(&out, "M5Stack Core2"), find(&out, "COM3"), find(&out, "UPDATE"));
    assert!((name.center().y - port.center().y).abs() < 2.0 && port.min.x > name.max.x + 100.0, "{name:?} {port:?}");
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
    assert!(find(&out, "COM3").center().y < ROOMY.top + ROW_H / 2.0 - 4.0, "the tighter measures: the row nearer the top");
    let primary = find(&out, "WRITE");
    let busy_top = FLOOR.y - TIGHT.under_busy - studio::NOTE_H;
    assert!(primary.center().y + studio::TALL_H / 2.0 <= busy_top + 0.5, "the primary whole over the busy line: {primary:?}");
    assert_drew(&out, &["COM3", "COM5"]);
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
