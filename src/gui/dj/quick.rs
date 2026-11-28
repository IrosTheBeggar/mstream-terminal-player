//! Quick config (the record's split Auto DJ button, #226): the bar's
//! `auto-dj` word starts and stops as it always has, and the chevron beside
//! it opens this — Play from (the servers inline up to four, one row to the
//! sheet past that), Sonic similarity with its match bar and the way to
//! the Seeds sheet while it is on (auto-dj clause 58, E-Quick), Smart
//! filters, and the way to the room. Every change applies at once: there is
//! no draft and no Done. It rises from the rule between the button's
//! halves — over the nav's rule, never over its rows — and never covers
//! the bar.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;
use rust_i18n::t;

use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{dim, width};
use crate::tui::app::{DjEdit, DjRow};

use super::super::{Act, DJ_NAV, Gui, accent, bright_bold, put, sel};
use super::{PickServer, Sheet, Tone};

/// Quick config drawn by egui in the GUI's own window (src/gui/chrome.rs).
#[cfg(feature = "window")]
pub(crate) mod egui_view;

/// Its cells across, and where it stands: its first column the rule
/// between the button's halves.
const QUICK_W: u16 = 42;
const QUICK_X: u16 = super::super::bar::DJ_X + 10;
/// The most servers it lists itself.
const INLINE: usize = 4;

/// One keyboard stop — and what a click on it presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QStop {
    /// One of Play from's servers, inline.
    Server(usize),
    /// Play from's row, where the servers are too many to list.
    PlayFrom,
    /// Sonic similarity's switch.
    Sonic,
    /// Its match bar: ← → step it, a click on a cell sets it.
    Bar,
    /// The way to the Seeds sheet.
    Seeds,
    Smart,
    /// `All Auto DJ settings ▸`.
    All,
}

/// Quick config, up: the keyboard's hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Quick {
    hand: QStop,
    shown: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum QuickAct {
    /// The bar's chevron: up, or away.
    Toggle,
    Press(QStop),
    Close,
    /// The match bar's `-` or `+`: a point down or up.
    Step(i32),
    /// A cell of the match bar: the floor it sets, in hundredths.
    Set(u32),
}

/// A line of the popover. Its groups stand a blank row apart, as the
/// canvas drew them.
enum Line {
    Head,
    Server(usize),
    PlayFrom,
    Gap,
    Sonic,
    /// Sonic similarity's match bar, under its switch.
    Bar,
    /// `Seeds ▸`, under the bar.
    Seeds,
    Smart,
    All,
}

/// Whether sonic similarity's own lines stand: switched on, and the server
/// can do it (E-Quick: hidden otherwise, as Drift · Stay close was).
fn sonic_lines(gui: &Gui) -> bool {
    gui.app.dj.sonic && gui.app.dj_sonic_gate().is_none()
}

/// What it shows right now: the servers where there is a choice of them,
/// the match bar and the Seeds row only while sonic similarity is on and
/// the server can do it.
fn lines(gui: &Gui, servers: &[PickServer]) -> Vec<Line> {
    let mut out = Vec::new();
    if servers.len() > 1 {
        if servers.len() <= INLINE {
            out.push(Line::Head);
            out.extend((0..servers.len()).map(Line::Server));
        } else {
            out.push(Line::PlayFrom);
        }
        out.push(Line::Gap);
    }
    out.push(Line::Sonic);
    if sonic_lines(gui) {
        out.extend([Line::Bar, Line::Seeds]);
    }
    out.extend([Line::Gap, Line::Smart, Line::Gap, Line::All]);
    out
}

/// The stops in their order.
fn stops(gui: &Gui, servers: &[PickServer]) -> Vec<QStop> {
    let mut out = Vec::new();
    for line in lines(gui, servers) {
        match line {
            Line::Server(i) => out.push(QStop::Server(i)),
            Line::PlayFrom => out.push(QStop::PlayFrom),
            Line::Sonic if gui.app.dj_sonic_gate().is_none() => out.push(QStop::Sonic),
            Line::Bar => out.push(QStop::Bar),
            Line::Seeds => out.push(QStop::Seeds),
            Line::Smart if gui.app.dj_mood_gate().is_none() => out.push(QStop::Smart),
            Line::All => out.push(QStop::All),
            _ => {}
        }
    }
    out
}

/// Whether the screen on show has the bar it rises from.
fn bar_stands(gui: &Gui) -> bool {
    matches!(gui.screen, super::super::Screen::Library | super::super::Screen::Admin)
}

/// Quick config while it is up, line by line, for either face to draw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuickView {
    /// Where the cells set it: from [`QUICK_X`], [`QUICK_W`] wide, its foot
    /// on the row above the bar's seek line.
    pub rect: Rect,
    /// The seek line's row, which nothing of it covers.
    pub bar_top: u16,
    /// The keyboard's hand, while it shows.
    pub hand: Option<QStop>,
    pub lines: Vec<QuickLine>,
}

/// A line of quick config: what it says, and the stop a press on it
/// presses (`Act::DjQuick(QuickAct::Press(stop))`), where it has one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QuickLine {
    /// Play from's head, in capitals as the cells set it.
    Head(String),
    /// One of Play from's servers, a peer behind a branch.
    Server { stop: QStop, words: String, peer: bool, current: bool },
    /// Play from's row, past the servers it lists inline.
    PlayFrom { stop: QStop, label: String, value: String },
    /// A blank row between groups.
    Gap,
    /// Sonic similarity's switch: gated, its reason and no stop; else on or
    /// off, with its value in its tone (the floor, or "! why" where a
    /// running session had to drop it).
    Sonic { label: String, gate: Option<String>, on: bool, value: String, tone: Tone },
    /// Its match bar.
    Bar(QuickBar),
    /// The way to the Seeds sheet.
    Seeds { stop: QStop, label: String },
    /// Smart filters: gated, its reason and no stop; else its value.
    Smart { label: String, gate: Option<String>, value: String, tone: Tone },
    /// The way to the room, its words (the cells set ▸ after them).
    All { stop: QStop, label: String },
}

/// The match bar: `cells` cells, `filled` of them lit. A press on cell `k`
/// is `Act::DjQuick(QuickAct::Set(first + k * per))`, on its ends
/// `Step(-1)` and `Step(1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuickBar {
    pub cells: u16,
    pub filled: u16,
    pub first: u32,
    pub per: u32,
}

/// Quick config as the Gui stands, in `area`, the frame's; None while it
/// is down or the screen has no bar.
pub(crate) fn view(gui: &Gui, area: Rect) -> Option<QuickView> {
    let quick = gui.dj.quick?;
    if !bar_stands(gui) {
        return None;
    }
    let servers = super::pickable_servers(gui);
    let lines = lines(gui, &servers);
    let bottom = super::super::bar::top(area, gui.footer());
    let h = (lines.len() as u16 + 2).min(bottom.saturating_sub(1));
    let w = QUICK_W.min(area.width.saturating_sub(2));
    let rect = Rect { x: QUICK_X.min(area.width.saturating_sub(w)), y: bottom.saturating_sub(h), width: w, height: h };
    let lines = lines
        .iter()
        .filter_map(|line| {
            Some(match line {
                Line::Gap => QuickLine::Gap,
                Line::Head => QuickLine::Head(t!("gui.dj.play_from").to_uppercase()),
                Line::Server(n) => {
                    let server = servers.get(*n)?;
                    QuickLine::Server { stop: QStop::Server(*n), words: server.label.clone(), peer: server.peer, current: server.current }
                }
                Line::PlayFrom => QuickLine::PlayFrom { stop: QStop::PlayFrom, label: t!("gui.dj.play_from").to_string(), value: super::target_name(gui) },
                Line::Sonic => {
                    let label = t!("gui.dj.sonic").to_string();
                    let s = &gui.app.dj;
                    match gui.app.dj_sonic_gate() {
                        Some(gate) => QuickLine::Sonic { label, gate: Some(super::gate_words(gate)), on: s.sonic, value: String::new(), tone: Tone::Neutral },
                        None => {
                            let (on, pct) = (s.sonic, (s.sonic_min_similarity * 100.0).round() as u32);
                            let (value, tone) = match super::sonic_drop(gui) {
                                Some(why) => (format!("! {why}"), Tone::Warning),
                                None if on => super::sonic_value(on, pct),
                                None => (String::new(), Tone::Neutral),
                            };
                            QuickLine::Sonic { label, gate: None, on, value, tone }
                        }
                    }
                }
                Line::Bar => QuickLine::Bar(super::sheets::sonic_bar(floor_of(gui)).quick()),
                Line::Seeds => QuickLine::Seeds { stop: QStop::Seeds, label: t!("gui.dj.seeds").to_string() },
                Line::Smart => {
                    let label = t!("gui.dj.smart").to_string();
                    match gui.app.dj_mood_gate() {
                        Some(gate) => QuickLine::Smart { label, gate: Some(super::gate_words(gate)), value: String::new(), tone: Tone::Neutral },
                        None => {
                            let (value, tone) = super::moods_value(gui.app.dj.moods);
                            QuickLine::Smart { label, gate: None, value, tone }
                        }
                    }
                }
                Line::All => QuickLine::All { stop: QStop::All, label: t!("gui.dj.all_settings").to_string() },
            })
        })
        .collect();
    Some(QuickView { rect, bar_top: bottom, hand: quick.shown.then_some(quick.hand), lines })
}

/// Quick config rises from the bar, and a screen without one puts it down:
/// run on every frame, before the layers draw, whichever face draws it.
pub(crate) fn settle_screen(gui: &mut Gui) {
    if gui.dj.quick.is_some() && !bar_stands(gui) {
        gui.dj.quick = None;
    }
}

/// The popover, drawn and registered over the room: a click anywhere else
/// puts it away (the bar's chevron among them).
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(view) = view(gui, area) else { return };
    let rect = view.rect;
    gui.ui.click(area, Act::DjQuick(QuickAct::Close));
    gui.ui.overlay(rect);
    let inner = crate::kit::frame_at(frame, rect, th().accent);
    gui.ui.click(rect, Act::Guard);
    let (x0, text_w) = (inner.x + 1, inner.width.saturating_sub(2));
    let right = x0 + text_w;
    let hand = |stop: QStop| view.hand == Some(stop);
    let (check_on, check_off) = super::super::check_glyphs();
    let (radio_on, radio_off) = if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") };
    let forward = super::super::forward_glyph();
    let branch = if legacy_conhost() { "+" } else { "└" };
    for (i, line) in view.lines.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let row = Rect { x: x0, y, width: text_w, height: 1 };
        match line {
            QuickLine::Gap => {}
            QuickLine::Head(words) => put(frame, x0, y, words, dim()),
            QuickLine::Server { stop, words, peer, current } => {
                let word = if *peer { format!("{branch} {words}") } else { words.clone() };
                let word = super::super::bar::clip(&word, text_w.saturating_sub(4) as usize).into_owned();
                choice(frame, gui, row, (radio_on, radio_off), *current, accent(), &word, hand(*stop), *stop);
            }
            QuickLine::PlayFrom { stop, label, value } => {
                value_row(frame, gui, row, label, (value.clone(), Tone::Neutral), hand(*stop), Some(*stop));
            }
            QuickLine::Sonic { label, gate: Some(why), .. } => {
                // The reason whole; the name gives way to it.
                let room = (text_w as usize).saturating_sub(4 + width(why) + 2);
                put(frame, x0, y, check_off, dim());
                put(frame, x0 + 4, y, &super::super::bar::clip(label, room), dim());
                put(frame, right.saturating_sub(width(why) as u16), y, why, dim());
            }
            QuickLine::Sonic { label, gate: None, on, value, tone } => {
                let stop = QStop::Sonic;
                let ok = Style::default().fg(th().ok);
                choice(frame, gui, row, (check_on, check_off), *on, ok, label, hand(stop), stop);
                put(frame, right.saturating_sub(width(value) as u16), y, value, tone.style());
            }
            // Under the switch's name: `-`, the cells, `+` — the value is
            // the line above's.
            QuickLine::Bar(_) => {
                let spec = super::sheets::sonic_bar(floor_of(gui));
                let step = |delta: i32| Act::DjQuick(QuickAct::Step(delta));
                let set = |value: u32| Act::DjQuick(QuickAct::Set(value));
                super::sheets::draw_bar(frame, gui, x0 + 4, y, &spec, hand(QStop::Bar), true, step, set, false);
            }
            // Under the switch's name too: the way to the Seeds sheet.
            QuickLine::Seeds { stop, label } => {
                let row = Rect { x: x0 + 4, width: text_w.saturating_sub(4), ..row };
                value_row(frame, gui, row, label, (String::new(), Tone::Neutral), hand(*stop), Some(*stop));
            }
            QuickLine::Smart { label, gate: Some(why), .. } => {
                let room = (text_w as usize).saturating_sub(width(why) + 4);
                put(frame, x0, y, &super::super::bar::clip(label, room), dim());
                put(frame, right.saturating_sub(width(why) as u16 + 2), y, why, dim());
            }
            QuickLine::Smart { label, gate: None, value, tone } => {
                let stop = QStop::Smart;
                value_row(frame, gui, row, label, (value.clone(), *tone), hand(stop), Some(stop));
            }
            QuickLine::All { stop, label } => {
                let stop = *stop;
                let label = format!("{label} {forward}");
                let cells = Rect { x: x0, y, width: width(&label) as u16, height: 1 };
                let style = match (hand(stop), gui.ui.hovers(cells)) {
                    (true, _) => sel().add_modifier(Modifier::BOLD),
                    (false, true) => bright_bold(),
                    (false, false) => accent(),
                };
                put(frame, x0, y, &label, style);
                gui.ui.click(cells, Act::DjQuick(QuickAct::Press(stop)));
            }
        }
    }
}

/// A switch or a radio: the glyph in its state's colour, the word ACCENT
/// BOLD under the keyboard's hand, BRIGHT BOLD under the pointer.
#[allow(clippy::too_many_arguments)]
fn choice(
    frame: &mut Frame,
    gui: &mut Gui,
    row: Rect,
    glyphs: (&str, &str),
    on: bool,
    on_style: Style,
    word: &str,
    hand: bool,
    stop: QStop,
) {
    let cells = Rect { width: (4 + width(word) as u16).min(row.width), ..row };
    let hover = gui.ui.hovers(cells);
    put(frame, row.x, row.y, if on { glyphs.0 } else { glyphs.1 }, if on { on_style } else { dim() });
    let style = match (hand, hover, on) {
        (true, _, _) => accent().add_modifier(Modifier::BOLD),
        (false, true, _) => bright_bold(),
        (false, false, true) => Style::default().add_modifier(Modifier::BOLD),
        (false, false, false) => Style::default(),
    };
    put(frame, row.x + 4, row.y, word, style);
    gui.ui.click(cells, Act::DjQuick(QuickAct::Press(stop)));
}

/// A row of the room, here: its name, its value in its tone, and `▸`.
fn value_row(
    frame: &mut Frame,
    gui: &mut Gui,
    row: Rect,
    label: &str,
    (value, tone): (String, Tone),
    hand: bool,
    stop: Option<QStop>,
) {
    let hover = gui.ui.hovers(row);
    if hand {
        frame.render_widget(Block::default().style(sel()), row);
    }
    let label_style = match (hand, hover) {
        (true, _) => sel().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => Style::default(),
    };
    put(frame, row.x, row.y, label, label_style);
    let room = (row.width as usize).saturating_sub(width(label) + 2 + 2);
    let shown = super::super::bar::clip(&value, room);
    let end = row.right().saturating_sub(2);
    put(frame, end.saturating_sub(width(&shown) as u16), row.y, &shown, if hand { sel() } else { tone.style() });
    let chevron_style = match (hand, hover) {
        (true, _) => sel(),
        (false, true) => bright_bold(),
        (false, false) => dim(),
    };
    put(frame, row.right().saturating_sub(1), row.y, super::super::forward_glyph(), chevron_style);
    if let Some(stop) = stop {
        gui.ui.click(row, Act::DjQuick(QuickAct::Press(stop)));
    }
}

/// Sonic similarity's floor as it stands, in hundredths.
fn floor_of(gui: &Gui) -> u32 {
    (gui.app.dj.sonic_min_similarity * 100.0).round() as u32
}

/// The match bar stepped a point, held inside its band: applied at once.
fn step_bar(gui: &mut Gui, delta: i32) {
    let value = super::sheets::sonic_bar(floor_of(gui)).stepped(delta);
    super::edit(gui, DjEdit::Set(DjRow::Strictness, value));
}

/// `stop` pressed — by the keyboard when `key` — applied at once.
fn press(gui: &mut Gui, stop: QStop, key: bool) {
    match stop {
        QStop::Server(i) => {
            let Some(server) = super::pickable_servers(gui).into_iter().nth(i) else { return };
            let effects = gui.app.dj_play_from_set(server.id);
            gui.pend(effects);
            gui.save_now();
        }
        QStop::PlayFrom => super::sheets::open(gui, Sheet::PlayFrom, false),
        // The mode rides along as it stands: it is the Seeds sheet's.
        QStop::Sonic => {
            let s = &gui.app.dj;
            let (on, anchor) = (!s.sonic, s.sonic_anchor);
            let floor = floor_of(gui);
            super::edit(gui, DjEdit::Sonic { on, floor, anchor });
        }
        // The bar is ← → and the pointer's: there is nothing to press.
        QStop::Bar => {}
        // Over whatever room this is; quick config goes with it.
        QStop::Seeds => super::seeds::open(gui, key),
        QStop::Smart => {
            gui.dj.quick = None;
            super::super::moods::open_for_dj(gui, false);
        }
        QStop::All => {
            gui.dj.quick = None;
            gui.act(Act::Screen(super::super::Screen::Library));
            gui.act(Act::Nav(DJ_NAV));
        }
    }
}

/// The hand off a stop that went: the bar's with its switch.
fn settle(gui: &mut Gui) {
    let Some(mut quick) = gui.dj.quick else { return };
    let stops = stops(gui, &super::pickable_servers(gui));
    if !stops.contains(&quick.hand) {
        quick.hand = match quick.hand {
            QStop::Bar | QStop::Seeds => QStop::Sonic,
            _ => stops.first().copied().unwrap_or(QStop::All),
        };
        if !stops.contains(&quick.hand) {
            quick.hand = QStop::All;
        }
        gui.dj.quick = Some(quick);
    }
}

pub(crate) fn act(gui: &mut Gui, act: QuickAct) {
    match act {
        QuickAct::Toggle => {
            if gui.dj.quick.take().is_none() {
                gui.dj.first = None;
                let first = stops(gui, &super::pickable_servers(gui)).first().copied().unwrap_or(QStop::All);
                gui.dj.quick = Some(Quick { hand: first, shown: false });
                // The rows describe Play from's server: ask it, as the
                // room does on opening.
                let effects = gui.app.dj_room_opened();
                gui.pend(effects);
            }
        }
        QuickAct::Close => gui.dj.quick = None,
        QuickAct::Press(stop) => {
            if let Some(quick) = gui.dj.quick.as_mut() {
                quick.hand = stop;
                quick.shown = false;
            }
            press(gui, stop, false);
            settle(gui);
        }
        // The pointer on the match bar: the hand stowed there.
        QuickAct::Step(delta) => {
            if let Some(quick) = gui.dj.quick.as_mut() {
                quick.hand = QStop::Bar;
                quick.shown = false;
            }
            step_bar(gui, delta);
        }
        QuickAct::Set(value) => {
            if let Some(quick) = gui.dj.quick.as_mut() {
                quick.hand = QStop::Bar;
                quick.shown = false;
            }
            super::edit(gui, DjEdit::Set(DjRow::Strictness, value));
        }
    }
}

/// Its keys while it is up: ↑ ↓ and Tab walk the stops, and so do ← →
/// but on the match bar, which they step; Space and Enter press; Esc puts
/// it away. `None` while it is down.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    let mut quick = gui.dj.quick?;
    let stops = stops(gui, &super::pickable_servers(gui));
    let at = stops.iter().position(|stop| *stop == quick.hand).unwrap_or(0);
    let last = stops.len().saturating_sub(1);
    let code = match key.code {
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        code => code,
    };
    match code {
        KeyCode::Esc => {
            gui.dj.quick = None;
            return Some(false);
        }
        KeyCode::Left | KeyCode::Right if quick.shown && quick.hand == QStop::Bar => {
            step_bar(gui, if code == KeyCode::Left { -1 } else { 1 });
        }
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
            // A stowed hand is only shown by the first walking key.
            if quick.shown && !stops.is_empty() {
                let back = matches!(code, KeyCode::Up | KeyCode::Left | KeyCode::BackTab);
                let next = if back { at.saturating_sub(1) } else { (at + 1).min(last) };
                quick.hand = stops[next];
            }
            quick.shown = true;
            gui.dj.quick = Some(quick);
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if quick.shown {
                press(gui, quick.hand, true);
                settle(gui);
            } else {
                quick.shown = true;
                gui.dj.quick = Some(quick);
            }
        }
        _ => {}
    }
    Some(false)
}
