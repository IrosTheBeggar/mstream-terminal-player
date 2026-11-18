//! The Auto DJ Seeds sheet (auto-dj contract, clauses 23, 64–66; option A
//! of the desktop canvas, A-Drift and A-StayClose): Drift · Stay close and
//! the mode's line at its top, then More like and Less like — a song a row,
//! its pin and its `[⋯]` menu, and Add song under each list, which arms the
//! library pick (clause 65) — then Clear pins when the queue is cleared,
//! and Done. It wears the room's sheets' shell (the mood Filters' modal),
//! but it has no draft: every press applies at once through the App, and
//! Done only closes (clause 51's exception).
//!
//! The lists are live while it is up — a pick starting in Drift, a Less
//! like row running out, a skip — so the hand and the menu are keyed by
//! the seed, never by a row's place, and the lists' region scrolls once
//! they outgrow the room.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;
use rust_i18n::t;

use crate::dj::SonicAnchor;
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, scroll_list, width, wrap_whole};
use crate::tui::app::{Capture, DjEdit, Seed, SeedKey, SeedList, SeedRefusal, Tab};

use super::super::{Act, DJ_NAV, FILES_NAV, Gui, List, Screen, accent, bright_bold, put, sel};
use super::sheets::{COL2, TEXT_W, done_word, draw_choice, radio_glyphs, walk};

/// The rows that are not the lists': the frame's two, the mode, its two
/// lines and a blank under them; a blank, the checkbox and the foot under
/// the lists (decision 34, clause 64).
const FIXED: u16 = 9;
/// The rows of the mode's line, whichever mode: the lists stand still when
/// it changes.
const SUB_ROWS: usize = 2;
/// The lists, top to bottom.
const LISTS: [SeedList; 2] = [SeedList::MoreLike, SeedList::LessLike];

// ── State ───────────────────────────────────────────────────────────────────

/// The sheet, up: the keyboard's hand, whether it shows, the row menu.
#[derive(Debug, Clone)]
pub(crate) struct SeedsUi {
    hand: SeedStop,
    /// Whether the hand is drawn: the sheet opened by its key, or a walking
    /// key pressed since a click stowed it (the kit's list-cursor law).
    shown: bool,
    menu: Option<SeedMenu>,
    /// The grid's row and column the hand last stood on: a stop that goes
    /// leaves the hand where it was.
    at: (usize, usize),
}

/// One keyboard stop — and what a click on it presses. A row's are keyed
/// by its seed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SeedStop {
    /// Drift (0) · Stay close (1).
    Mode(u8),
    /// A row's pin.
    Pin(SeedList, SeedKey),
    /// A row's `[⋯]`: its menu.
    Menu(SeedList, SeedKey),
    /// A list's `+ Add song`.
    Add(SeedList),
    /// Clear pins when the queue is cleared.
    Check,
    Done,
}

/// A row's menu, down: the row it hangs from, and its cursor — `None` is
/// stowed (opened by the pointer).
#[derive(Debug, Clone, PartialEq)]
struct SeedMenu {
    list: SeedList,
    key: SeedKey,
    cursor: Option<usize>,
}

/// The pointer's acts inside the sheet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SeedsAct {
    Press(SeedStop),
    /// A verb of the row menu, by its place.
    Verb(usize),
    /// A click off the menu: it folds, and the sheet stays.
    MenuClose,
}

/// What a press leaves the sheet to do — the caller's to carry out, so a
/// press never writes the sheet back over its own close.
enum After {
    Stay,
    Close,
    /// The sheet goes, and the library pick is armed for this list.
    Pick(SeedList),
}

/// A row menu's verbs, in their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    /// To the other list, pinned: Less like this on a More like row, More
    /// like this on a Less like row.
    Move,
    Remove,
}

const VERBS: [Verb; 2] = [Verb::Move, Verb::Remove];

// ── What answers ────────────────────────────────────────────────────────────

/// Whether the DJ's server takes negatives (clause 23): Less like's own
/// controls answer only then.
fn negatives(gui: &Gui) -> bool {
    gui.app.dj_takes_negatives()
}

/// Whether a list's pins answer: never while Stay close locks them, and
/// Less like's only while the server takes negatives.
fn pin_live(gui: &Gui, list: SeedList) -> bool {
    !gui.app.seeds.frozen() && (list == SeedList::MoreLike || negatives(gui))
}

/// Whether a list's Add song answers: the list can take a row, the
/// library on screen is the room's server — a pick from any other would
/// only be refused (decision 28) — and Less like, the server's negatives.
fn add_live(gui: &Gui, list: SeedList) -> bool {
    gui.app.seeds.can_take(list) && gui.app.seeds_pickable() && (list == SeedList::MoreLike || negatives(gui))
}

/// Whether a verb is on offer on a row of `list` right now: a move needs
/// room in the other list (and, into Less like, the server's negatives).
fn verb_live(gui: &Gui, list: SeedList, verb: Verb) -> bool {
    match verb {
        Verb::Remove => true,
        Verb::Move => {
            let to = list.other();
            gui.app.seeds.can_take(to) && (to == SeedList::MoreLike || negatives(gui))
        }
    }
}

fn verb_word(list: SeedList, verb: Verb) -> String {
    match (verb, list) {
        (Verb::Move, SeedList::MoreLike) => t!("gui.dj.less_like_this"),
        (Verb::Move, SeedList::LessLike) => t!("gui.dj.more_like_this"),
        (Verb::Remove, _) => t!("gui.dj.seed_remove"),
    }
    .to_string()
}

/// The first verb on offer, for a cursor shown on opening.
fn first_verb(gui: &Gui, list: SeedList) -> Option<usize> {
    (0..VERBS.len()).find(|&i| verb_live(gui, list, VERBS[i]))
}

fn seed_of<'a>(gui: &'a Gui, list: SeedList, key: &SeedKey) -> Option<&'a Seed> {
    gui.app.seeds.list(list).iter().find(|seed| seed.key == *key)
}

// ── Glyphs ──────────────────────────────────────────────────────────────────

/// A pin: pinned, unpinned — the legacy console's stand-ins (ui-kit.md).
fn pin_glyphs() -> (&'static str, &'static str) {
    if legacy_conhost() { ("*", "o") } else { ("◉", "◌") }
}

/// A row's menu: the queue's `[⋯]`, and its legacy stand-in.
fn menu_glyph() -> &'static str {
    if legacy_conhost() { "[.]" } else { "[⋯]" }
}

// ── The grid ────────────────────────────────────────────────────────────────

/// A line of the lists' region — the part of the sheet that scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Head(SeedList),
    /// A row, by its place in its list as this frame has it.
    Row(SeedList, usize),
    Add(SeedList),
    Gap,
}

/// The region's lines: More like's header, rows and Add song; a blank;
/// Less like's.
fn region(gui: &Gui) -> Vec<Line> {
    let mut lines = Vec::new();
    for list in LISTS {
        if list == SeedList::LessLike {
            lines.push(Line::Gap);
        }
        lines.push(Line::Head(list));
        lines.extend((0..gui.app.seeds.list(list).len()).map(|i| Line::Row(list, i)));
        lines.push(Line::Add(list));
    }
    lines
}

/// The stops by row — what ↑ ↓ ← → walk: the mode's two; a row's pin and
/// menu (its menu alone while its pin rests); an Add song that answers;
/// the checkbox; Done.
fn stop_rows(gui: &Gui) -> Vec<Vec<SeedStop>> {
    let mut rows = vec![vec![SeedStop::Mode(0), SeedStop::Mode(1)]];
    for list in LISTS {
        let pins = pin_live(gui, list);
        for seed in gui.app.seeds.list(list) {
            let menu = SeedStop::Menu(list, seed.key.clone());
            rows.push(if pins { vec![SeedStop::Pin(list, seed.key.clone()), menu] } else { vec![menu] });
        }
        if add_live(gui, list) {
            rows.push(vec![SeedStop::Add(list)]);
        }
    }
    rows.push(vec![SeedStop::Check]);
    rows.push(vec![SeedStop::Done]);
    rows
}

fn locate(rows: &[Vec<SeedStop>], stop: &SeedStop) -> Option<(usize, usize)> {
    rows.iter().enumerate().find_map(|(r, row)| row.iter().position(|s| s == stop).map(|c| (r, c)))
}

/// The hand off a stop that went — a row removed, moved or run out, a pin
/// that Stay close locked, an Add song a full list put to rest: the stop
/// at its place in the grid. The menu folds when its row leaves.
fn settle(ui: &mut SeedsUi, gui: &Gui) {
    let rows = stop_rows(gui);
    match locate(&rows, &ui.hand) {
        Some(at) => ui.at = at,
        None => {
            let r = ui.at.0.min(rows.len() - 1);
            let c = ui.at.1.min(rows[r].len() - 1);
            ui.hand = rows[r][c].clone();
            ui.at = (r, c);
        }
    }
    if ui.menu.as_ref().is_some_and(|menu| !gui.app.seeds.holds(menu.list, &menu.key)) {
        ui.menu = None;
    }
}

/// The region line the hand stands on, when it is one of the region's.
fn hand_line(gui: &Gui, lines: &[Line], hand: &SeedStop) -> Option<usize> {
    let want = match hand {
        SeedStop::Pin(list, key) | SeedStop::Menu(list, key) => {
            let i = gui.app.seeds.list(*list).iter().position(|seed| seed.key == *key)?;
            Line::Row(*list, i)
        }
        SeedStop::Add(list) => Line::Add(*list),
        _ => return None,
    };
    lines.iter().position(|line| *line == want)
}

/// The sheet's rows: the fixed ones and the region's lines, as many as the
/// room affords — an over-tall sheet stands from row 1, never over the bar
/// (clause 64: 24 rows at 100×30, 18 at 100×24).
fn height(gui: &Gui, lines: usize) -> u16 {
    let cap = gui.room_rect().bottom().saturating_sub(1);
    (FIXED + lines as u16).min(cap)
}

// ── Opening and coming back ─────────────────────────────────────────────────

/// Open the sheet over the room (the room's Seeds row, quick config's):
/// every other layer of the DJ's goes, and the hand stands on the mode in
/// force — up when the keyboard opened it.
pub(crate) fn open(gui: &mut Gui, key: bool) {
    gui.dj.quick = None;
    gui.dj.first = None;
    gui.dj.recent = None;
    gui.dj.sheet = None;
    gui.dj.list = ListView::default();
    let hand = SeedStop::Mode(u8::from(gui.app.dj.sonic_anchor == SonicAnchor::Locked));
    let mut ui = SeedsUi { hand, shown: key, menu: None, at: (0, 0) };
    settle(&mut ui, gui);
    gui.dj.seeds = Some(ui);
}

/// Add song's pick is gone — answered, or let go by Esc or the banner's
/// `[X]` (decision 33: never while it still stands): home to the Auto DJ room,
/// whatever room or screen asked, the sheet up again over it with the hand
/// on that list's Add song where it was shown; a pick the lists refused
/// says why once (decision 28).
pub(crate) fn returned(gui: &mut Gui, list: SeedList) {
    to_library(gui);
    gui.active = DJ_NAV;
    let shown = gui.dj.seeds_back.take().unwrap_or(false);
    open(gui, shown);
    let add = SeedStop::Add(list);
    let stands = stop_rows(gui).iter().flatten().any(|stop| *stop == add);
    if let Some(mut ui) = gui.dj.seeds.take() {
        if stands {
            ui.hand = add;
        }
        settle(&mut ui, gui);
        gui.dj.seeds = Some(ui);
        gui.dj.list.reveal = true;
    }
    if let Some(SeedRefusal::OtherServer(server)) = gui.app.seeds_refused.take() {
        gui.note = Some((t!("gui.discover.pick_other_server", server = server).to_string(), true));
    }
    gui.save_soon();
}

/// Add song pressed: the sheet is gone already, and the library pick is
/// armed for `list` — the Files room on screen, its banner above it.
fn arm(gui: &mut Gui, list: SeedList, shown: bool) {
    gui.dj.seeds_back = Some(shown);
    let effects = gui.app.arm_capture(Capture::Seeds(list));
    gui.pend(effects);
    to_library(gui);
    gui.active = FILES_NAV;
    gui.app.tab = Tab::Files;
}

/// The Library screen, through the screens' own door: the one left goes
/// with it — the full-screen view's keys, the Stats page, the Admin room
/// and its log's poll (quick config stands over Admin too).
fn to_library(gui: &mut Gui) {
    if gui.screen != Screen::Library {
        gui.act(Act::Screen(Screen::Library));
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The sheet over the room, drawn and registered over everything beneath:
/// the title on the top border; Drift · Stay close and the mode's line;
/// the lists, scrolling where they outgrow it; the checkbox; the foot; the
/// row menu over all of it.
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(mut ui) = gui.dj.seeds.clone() else { return };
    settle(&mut ui, gui);
    let lines = region(gui);
    let h = height(gui, lines.len());
    let (rect, band) = super::super::moods::place(area, gui.room_rect(), h);
    gui.ui.clear_registries();
    gui.ui.click(area, Act::Guard);
    let own = band.unwrap_or(rect);
    gui.ui.overlay(own);
    gui.ui.modal_over();
    if let Some(band) = band {
        crate::kit::blank(frame, band);
    }
    super::super::keep::modal_box(frame, rect);
    gui.ui.click(own, Act::Guard);
    super::border_title_value(frame, rect, &t!("gui.dj.seeds"), "", super::Tone::Neutral);

    let (x0, top) = (rect.x + 2, rect.y + 1);
    // The menu has the keyboard while it is down: the sheet's hand rests.
    let shown = ui.shown && ui.menu.is_none();
    let hand = |stop: &SeedStop| shown && ui.hand == *stop;

    // Drift · Stay close, and what the mode in force does.
    let mode = gui.app.dj.sonic_anchor;
    let words = [t!("gui.dj.drift").to_string(), t!("gui.dj.stay_close").to_string()];
    let x2 = x0 + COL2.max(4 + width(&words[0]) as u16 + 3);
    for (i, (kind, word)) in [SonicAnchor::Rolling, SonicAnchor::Locked].into_iter().zip(&words).enumerate() {
        let stop = SeedStop::Mode(i as u8);
        let x = if i == 0 { x0 } else { x2 };
        let act = Act::DjSeeds(SeedsAct::Press(stop.clone()));
        draw_choice(frame, gui, x, top, radio_glyphs(), mode == kind, accent(), word, hand(&stop), true, act);
    }
    let sub = if mode == SonicAnchor::Locked { t!("gui.dj.seeds_stay_sub") } else { t!("gui.dj.seeds_drift_sub") };
    for (i, line) in wrap_whole(&sub, TEXT_W as usize).into_iter().take(SUB_ROWS).enumerate() {
        put(frame, x0, top + 1 + i as u16, &line, dim());
    }

    // The lists' region: the window keeps the hand's line in view.
    let region_top = top + 1 + SUB_ROWS as u16 + 1;
    let visible = h.saturating_sub(FIXED) as usize;
    let at = hand_line(gui, &lines, &ui.hand);
    let (first, _) = gui.dj.list.window(lines.len(), at, visible);
    let overflow = lines.len() > visible;
    // The scrollbar takes the region's last column; the rows end short of it.
    let end = x0 + TEXT_W - if overflow { 2 } else { 0 };
    let mut menu_at = None;
    for (n, line) in lines.iter().enumerate().skip(first).take(visible) {
        let y = region_top + (n - first) as u16;
        match *line {
            Line::Gap => {}
            Line::Head(list) => draw_head(frame, gui, x0, y, end, list),
            Line::Row(list, i) => {
                let Some(seed) = gui.app.seeds.list(list).get(i).cloned() else { continue };
                let open = ui.menu.as_ref().is_some_and(|menu| menu.list == list && menu.key == seed.key);
                let pin = hand(&SeedStop::Pin(list, seed.key.clone()));
                let dots = hand(&SeedStop::Menu(list, seed.key.clone()));
                let at = draw_seed(frame, gui, x0, y, end, list, &seed, (pin, dots), open);
                if open {
                    menu_at = Some(at);
                }
            }
            Line::Add(list) => draw_add(frame, gui, x0, y, list, hand(&SeedStop::Add(list))),
        }
    }
    if overflow {
        let bar = Rect { x: x0 + TEXT_W - 1, y: region_top, width: 1, height: visible as u16 };
        scroll_list(
            frame,
            &mut gui.ui,
            bar,
            lines.len(),
            visible,
            first,
            Act::ScrollBy(List::DjSheet, -1),
            Act::ScrollBy(List::DjSheet, 1),
            |first| Act::ScrollTo(List::DjSheet, first),
        );
    }

    // The checkbox on its own row over the foot, and Done.
    let foot = rect.bottom().saturating_sub(2);
    let (check_on, check_off) = super::super::check_glyphs();
    let ok = Style::default().fg(th().ok);
    let on = gui.app.dj.clear_pins_on_queue_clear;
    let word = t!("gui.dj.clear_pins").to_string();
    let act = Act::DjSeeds(SeedsAct::Press(SeedStop::Check));
    draw_choice(frame, gui, x0, foot - 1, (check_on, check_off), on, ok, &word, hand(&SeedStop::Check), true, act);
    let done = done_word();
    let done_w = width(&done) as u16;
    let done_rect = Rect { x: rect.right().saturating_sub(2 + done_w), y: foot, width: done_w, height: 1 };
    let style = if hand(&SeedStop::Done) {
        sel().add_modifier(Modifier::BOLD)
    } else if gui.ui.hovers(done_rect) {
        bright_bold()
    } else {
        accent().add_modifier(Modifier::BOLD)
    };
    put(frame, done_rect.x, foot, &done, style);
    gui.ui.click(done_rect, Act::DjSeeds(SeedsAct::Press(SeedStop::Done)));

    // The row's menu over it all; one whose row is out of the window folds.
    match (ui.menu.clone(), menu_at) {
        (Some(menu), Some(dots)) => draw_menu(frame, gui, area, rect, dots, &menu),
        (Some(_), None) => ui.menu = None,
        _ => {}
    }
    gui.dj.seeds = Some(ui);
}

/// A list's header, DIM; Less like's says "Needs a newer server" at the
/// row's end once the server has said it takes no negatives (decisions
/// 30 and 34) — while that is not known yet the list rests without a reason.
fn draw_head(frame: &mut Frame, gui: &Gui, x0: u16, y: u16, end: u16, list: SeedList) {
    let word = match list {
        SeedList::MoreLike => t!("gui.dj.more_like"),
        SeedList::LessLike => t!("gui.dj.less_like"),
    };
    put(frame, x0, y, &word.to_uppercase(), dim());
    if list == SeedList::LessLike && gui.app.dj_lacks_negatives() {
        let why = t!("gui.dj.needs_newer_server");
        put(frame, end.saturating_sub(width(&why) as u16), y, &why, dim());
    }
}

/// One row: the pin, the title and the artist DIM, `[⋯]` at the row's end.
/// The pin is OK while the user's own and live, DIM unpinned, and `◉` DIM
/// whole while Stay close locks it; a Less like row without the server's
/// negatives is DIM, its `[⋯]` still live. The hand is the accent slab on
/// its glyph, the pointer BRIGHT; a row whose menu is down wears the
/// accent. Returns the `[⋯]`'s cells, where the menu hangs.
#[allow(clippy::too_many_arguments)]
fn draw_seed(
    frame: &mut Frame,
    gui: &mut Gui,
    x0: u16,
    y: u16,
    end: u16,
    list: SeedList,
    seed: &Seed,
    (pin_hand, dots_hand): (bool, bool),
    open: bool,
) -> Rect {
    let live = pin_live(gui, list);
    let pin = Rect { x: x0, y, width: 1, height: 1 };
    let (pinned, unpinned) = pin_glyphs();
    let glyph = if gui.app.seeds.shown_pinned(seed) { pinned } else { unpinned };
    let style = if pin_hand {
        sel()
    } else if live && gui.ui.hovers(pin) {
        bright_bold()
    } else if live && seed.pinned {
        Style::default().fg(th().ok)
    } else {
        dim()
    };
    put(frame, x0, y, glyph, style);
    if live {
        gui.ui.click(pin, Act::DjSeeds(SeedsAct::Press(SeedStop::Pin(list, seed.key.clone()))));
    }

    // The title and the artist end a cell short of `[⋯]`.
    let dots = Rect { x: end.saturating_sub(3), y, width: 3, height: 1 };
    let room = dots.x.saturating_sub(x0 + 3) as usize;
    let (title, artist) = super::song_words(&seed.track);
    let shown = super::super::bar::clip(&title, room);
    let words_live = list == SeedList::MoreLike || negatives(gui);
    let title_style = match (open, words_live) {
        (true, _) => accent().add_modifier(Modifier::BOLD),
        (false, true) => Style::default(),
        (false, false) => dim(),
    };
    put(frame, x0 + 2, y, &shown, title_style);
    let used = width(&shown);
    if let Some(artist) = artist
        && used + 4 < room
    {
        let tail = format!(" · {artist}");
        put(frame, x0 + 2 + used as u16, y, &super::super::bar::clip(&tail, room - used), dim());
    }

    let style = if open {
        accent().add_modifier(Modifier::BOLD)
    } else if dots_hand {
        sel()
    } else if gui.ui.hovers(dots) {
        bright_bold()
    } else {
        dim()
    };
    put(frame, dots.x, y, menu_glyph(), style);
    gui.ui.click(dots, Act::DjSeeds(SeedsAct::Press(SeedStop::Menu(list, seed.key.clone()))));
    dots
}

/// `+ Add song`: TEXT, ACCENT BOLD under the hand, BRIGHT BOLD under the
/// pointer; DIM and inert while its list can take no row, the library on
/// screen is not the room's server, or — Less like's — the server takes
/// no negatives.
fn draw_add(frame: &mut Frame, gui: &mut Gui, x0: u16, y: u16, list: SeedList, hand: bool) {
    let label = format!("+ {}", t!("gui.dj.add_song"));
    let rect = Rect { x: x0, y, width: width(&label) as u16, height: 1 };
    let live = add_live(gui, list);
    let style = if !live {
        dim()
    } else if hand {
        accent().add_modifier(Modifier::BOLD)
    } else if gui.ui.hovers(rect) {
        bright_bold()
    } else {
        Style::default()
    };
    put(frame, x0, y, &label, style);
    if live {
        gui.ui.click(rect, Act::DjSeeds(SeedsAct::Press(SeedStop::Add(list))));
    }
}

/// A row's menu — the kit's context list on a song row (decision 22): hung a
/// row under its row, its right border on the `]` of `[⋯]`, flipped above
/// where it would pass the sheet's last row; a click anywhere off it folds
/// it and keeps. A verb not on offer is DIM and inert, and the cursor
/// passes it; Remove is DANGER at rest.
fn draw_menu(frame: &mut Frame, gui: &mut Gui, area: Rect, sheet: Rect, dots: Rect, menu: &SeedMenu) {
    gui.ui.click(area, Act::DjSeeds(SeedsAct::MenuClose));
    let verbs: Vec<(Verb, String, bool)> =
        VERBS.iter().map(|verb| (*verb, verb_word(menu.list, *verb), verb_live(gui, menu.list, *verb))).collect();
    let widest = verbs.iter().map(|(_, word, _)| width(word)).max().unwrap_or(0) as u16;
    let (w, h) = (widest + 6, verbs.len() as u16 + 2);
    let below = dots.y + 1;
    let y = if below + h > sheet.bottom().saturating_sub(1) { dots.y.saturating_sub(h) } else { below };
    let rect = Rect { x: dots.right().saturating_sub(w), y, width: w, height: h };
    gui.ui.overlay(rect);
    let inner = crate::kit::frame_at(frame, rect, th().accent);
    gui.ui.click(rect, Act::Guard);
    for (i, (verb, word, live)) in verbs.iter().enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let held = *live && menu.cursor == Some(i);
        let hover = *live && gui.ui.hovers(row);
        if held {
            frame.render_widget(Block::default().style(sel()), row);
        }
        let style = match (*live, held, hover) {
            (false, _, _) => dim(),
            (true, true, _) => sel().add_modifier(Modifier::BOLD),
            (true, false, true) => bright_bold(),
            (true, false, false) if *verb == Verb::Remove => Style::default().fg(th().danger),
            (true, false, false) => Style::default(),
        };
        put(frame, rect.x + 2, row.y, word, style);
        if *live {
            gui.ui.click(row, Act::DjSeeds(SeedsAct::Verb(i)));
        }
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// `stop` pressed, by the keyboard when `by_key`: applied at once.
fn press(gui: &mut Gui, ui: &mut SeedsUi, stop: SeedStop, by_key: bool) -> After {
    match stop {
        SeedStop::Mode(i) => {
            let mode = if i == 0 { SonicAnchor::Rolling } else { SonicAnchor::Locked };
            super::edit(gui, DjEdit::SeedMode(mode));
        }
        SeedStop::Pin(list, key) => {
            if !pin_live(gui, list) {
                return After::Stay;
            }
            if let Some(pinned) = seed_of(gui, list, &key).map(|seed| !seed.pinned) {
                super::edit(gui, DjEdit::SeedPin { list, key, pinned });
            }
        }
        SeedStop::Menu(list, key) => {
            if gui.app.seeds.holds(list, &key) {
                let cursor = if by_key { first_verb(gui, list) } else { None };
                ui.menu = Some(SeedMenu { list, key, cursor });
            }
        }
        SeedStop::Add(list) => {
            if add_live(gui, list) {
                return After::Pick(list);
            }
        }
        SeedStop::Check => {
            let on = !gui.app.dj.clear_pins_on_queue_clear;
            super::edit(gui, DjEdit::ClearPinsOnQueueClear(on));
        }
        SeedStop::Done => return After::Close,
    }
    After::Stay
}

/// A menu's verb, chosen: the row moved or deleted, the menu gone.
fn verb(gui: &mut Gui, menu: SeedMenu, verb: Verb) {
    if !verb_live(gui, menu.list, verb) {
        return;
    }
    let edit = match verb {
        Verb::Move => DjEdit::SeedMove { from: menu.list, key: menu.key },
        Verb::Remove => DjEdit::SeedRemove { list: menu.list, key: menu.key },
    };
    super::edit(gui, edit);
}

/// What a press said, carried out: the sheet written back while it stays,
/// let go when it closes — and for Add song let go first, so the pick is
/// armed only once the sheet is gone.
fn finish(gui: &mut Gui, mut ui: SeedsUi, after: After) {
    match after {
        After::Stay => {
            settle(&mut ui, gui);
            gui.dj.seeds = Some(ui);
        }
        After::Close => gui.dj.seeds = None,
        After::Pick(list) => {
            gui.dj.seeds = None;
            arm(gui, list, ui.shown);
        }
    }
}

/// The sheet's keys while it is up — every key but Ctrl-C, which quits
/// first: the room beneath is inert. With a row's menu down, its keys: ↑ ↓
/// its verbs, Enter and Space choose, Esc ← and `m` fold it. Else ↑ ↓ ← →
/// walk the grid, Tab and Shift-Tab its reading order; Space presses, `m`
/// opens a row's menu; Enter and Esc are Done; PageUp and PageDown move
/// the lists' window. `None` while it is down.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    let mut ui = gui.dj.seeds.clone()?;
    let code = match key.code {
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        code => code,
    };
    if let Some(mut menu) = ui.menu.take() {
        let live: Vec<usize> = (0..VERBS.len()).filter(|&i| verb_live(gui, menu.list, VERBS[i])).collect();
        match code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('m') => {}
            KeyCode::Up | KeyCode::Down => {
                // A stowed cursor is only shown by the first key.
                menu.cursor = match menu.cursor.and_then(|at| live.iter().position(|i| *i == at)) {
                    None => live.first().copied(),
                    Some(at) if code == KeyCode::Up => live.get(at.saturating_sub(1)).copied(),
                    Some(at) => live.get((at + 1).min(live.len().saturating_sub(1))).copied(),
                };
                ui.menu = Some(menu);
            }
            KeyCode::Enter | KeyCode::Char(' ') => match menu.cursor.filter(|at| live.contains(at)) {
                Some(at) => verb(gui, menu, VERBS[at]),
                None => {
                    menu.cursor = live.first().copied();
                    ui.menu = Some(menu);
                }
            },
            // Everything else is the menu's, and does nothing.
            _ => ui.menu = Some(menu),
        }
        finish(gui, ui, After::Stay);
        return Some(false);
    }
    let mut after = After::Stay;
    match code {
        KeyCode::Esc | KeyCode::Enter => after = After::Close,
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
            // A stowed hand is only shown by the first walking key.
            if ui.shown {
                ui.hand = walk(&stop_rows(gui), &ui.hand, code);
            }
            ui.shown = true;
            gui.dj.list.reveal = true;
        }
        KeyCode::PageUp | KeyCode::PageDown => {
            let page = height(gui, region(gui).len()).saturating_sub(FIXED).max(1) as i32;
            gui.dj.list.step(if code == KeyCode::PageUp { -page } else { page });
        }
        KeyCode::Char(' ') => {
            if ui.shown {
                let stop = ui.hand.clone();
                after = press(gui, &mut ui, stop, true);
            }
            ui.shown = true;
        }
        KeyCode::Char('m') => {
            if ui.shown
                && let SeedStop::Pin(list, key) | SeedStop::Menu(list, key) = ui.hand.clone()
            {
                press(gui, &mut ui, SeedStop::Menu(list, key), true);
            }
            ui.shown = true;
        }
        _ => {}
    }
    finish(gui, ui, after);
    Some(false)
}

/// The pointer's acts.
pub(crate) fn act(gui: &mut Gui, act: SeedsAct) {
    let Some(mut ui) = gui.dj.seeds.clone() else { return };
    let mut after = After::Stay;
    match act {
        SeedsAct::MenuClose => ui.menu = None,
        // A verb not on offer takes no click; the menu stays down.
        SeedsAct::Verb(i) => {
            if let Some(menu) = ui.menu.clone()
                && let Some(chosen) = VERBS.get(i).filter(|chosen| verb_live(gui, menu.list, **chosen))
            {
                ui.menu = None;
                verb(gui, menu, *chosen);
            }
        }
        // Pressed, and the hand stowed there: the next walking key shows
        // it on this stop.
        SeedsAct::Press(stop) => {
            ui.menu = None;
            ui.shown = false;
            ui.hand = stop.clone();
            after = press(gui, &mut ui, stop, false);
        }
    }
    finish(gui, ui, after);
}

/// The wheel over the sheet: the lists' window, while no menu is down (the
/// next frame clamps the step). True while the sheet is up.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) -> bool {
    let Some(ui) = gui.dj.seeds.as_ref() else { return false };
    if ui.menu.is_none() {
        gui.dj.list.step(delta);
    }
    true
}

/// The tips line while the sheet is up: a menu's, or the sheet's.
pub(crate) fn tips(gui: &Gui) -> Option<String> {
    let ui = gui.dj.seeds.as_ref()?;
    let tip = if ui.menu.is_some() { t!("gui.tips.dj_list") } else { t!("gui.tips.dj_seeds") };
    Some(tip.to_string())
}
