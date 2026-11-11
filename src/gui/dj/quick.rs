//! Quick config (the record's split Auto DJ button, #226): the bar's
//! `auto-dj` word starts and stops as it always has, and the chevron beside
//! it opens this — Play from (the servers inline up to four, one row to the
//! sheet past that), Sonic similarity with Drift · Stay close, Smart
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

use crate::dj::SonicAnchor;
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{dim, width};
use crate::tui::app::DjEdit;

use super::super::{Act, DJ_NAV, Gui, accent, bright_bold, put, sel};
use super::{PickServer, Sheet, Tone};

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
    /// Drift · Stay close.
    Anchor(u8),
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
}

/// A line of the popover. Its groups stand a blank row apart, as the
/// canvas drew them.
enum Line {
    Head,
    Server(usize),
    PlayFrom,
    Gap,
    Sonic,
    Anchor,
    Smart,
    All,
}

/// What it shows right now: the servers where there is a choice of them,
/// the anchor only while sonic similarity is on and the server can do it.
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
    if gui.app.dj.sonic && gui.app.dj_sonic_gate().is_none() {
        out.push(Line::Anchor);
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
            Line::Anchor => out.extend([QStop::Anchor(0), QStop::Anchor(1)]),
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

/// The popover, drawn and registered over the room: a click anywhere else
/// puts it away (the bar's chevron among them).
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(quick) = gui.dj.quick else { return };
    if !bar_stands(gui) {
        gui.dj.quick = None;
        return;
    }
    let servers = super::pickable_servers(gui);
    let lines = lines(gui, &servers);
    let bottom = super::super::bar::top(area, gui.footer());
    let h = (lines.len() as u16 + 2).min(bottom.saturating_sub(1));
    let w = QUICK_W.min(area.width.saturating_sub(2));
    let rect = Rect { x: QUICK_X.min(area.width.saturating_sub(w)), y: bottom.saturating_sub(h), width: w, height: h };
    gui.ui.click(area, Act::DjQuick(QuickAct::Close));
    gui.ui.overlay(rect);
    let inner = crate::kit::frame_at(frame, rect, th().accent);
    gui.ui.click(rect, Act::Guard);
    let (x0, text_w) = (inner.x + 1, inner.width.saturating_sub(2));
    let right = x0 + text_w;
    let hand = |stop: QStop| quick.shown && quick.hand == stop;
    let (check_on, check_off) = super::super::check_glyphs();
    let (radio_on, radio_off) = if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") };
    let forward = super::super::forward_glyph();
    let branch = if legacy_conhost() { "+" } else { "└" };
    for (i, line) in lines.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let row = Rect { x: x0, y, width: text_w, height: 1 };
        match line {
            Line::Gap => {}
            Line::Head => put(frame, x0, y, &t!("gui.dj.play_from").to_uppercase(), dim()),
            Line::Server(n) => {
                let Some(server) = servers.get(*n) else { continue };
                let stop = QStop::Server(*n);
                let word = if server.peer { format!("{branch} {}", server.label) } else { server.label.clone() };
                let word = super::super::bar::clip(&word, text_w.saturating_sub(4) as usize).into_owned();
                choice(frame, gui, row, (radio_on, radio_off), server.current, accent(), &word, hand(stop), stop);
            }
            Line::PlayFrom => {
                let stop = QStop::PlayFrom;
                value_row(frame, gui, row, &t!("gui.dj.play_from"), (super::target_name(gui), Tone::Neutral), hand(stop), Some(stop));
            }
            Line::Sonic => {
                let label = t!("gui.dj.sonic").to_string();
                match gui.app.dj_sonic_gate() {
                    Some(gate) => {
                        // The reason whole; the name gives way to it.
                        let why = super::gate_words(gate);
                        let room = (text_w as usize).saturating_sub(4 + width(&why) + 2);
                        put(frame, x0, y, check_off, dim());
                        put(frame, x0 + 4, y, &super::super::bar::clip(&label, room), dim());
                        put(frame, right.saturating_sub(width(&why) as u16), y, &why, dim());
                    }
                    None => {
                        let stop = QStop::Sonic;
                        let s = &gui.app.dj;
                        let (on, pct) = (s.sonic, (s.sonic_min_similarity * 100.0).round() as u32);
                        let (value, tone) = match super::sonic_drop(gui) {
                            Some(why) => (format!("! {why}"), Tone::Warning),
                            None if on => super::sonic_value(on, pct),
                            None => (String::new(), Tone::Neutral),
                        };
                        let ok = Style::default().fg(th().ok);
                        choice(frame, gui, row, (check_on, check_off), on, ok, &label, hand(stop), stop);
                        put(frame, right.saturating_sub(width(&value) as u16), y, &value, tone.style());
                    }
                }
            }
            Line::Anchor => {
                let now = gui.app.dj.sonic_anchor;
                let words = [t!("gui.dj.drift").to_string(), t!("gui.dj.stay_close").to_string()];
                let mut x = x0 + 4;
                for (n, (kind, word)) in [SonicAnchor::Rolling, SonicAnchor::Locked].into_iter().zip(&words).enumerate() {
                    let stop = QStop::Anchor(n as u8);
                    let cells = Rect { x, y, width: 4 + width(word) as u16, height: 1 };
                    choice(frame, gui, cells, (radio_on, radio_off), now == kind, accent(), word, hand(stop), stop);
                    x += cells.width + 3;
                }
            }
            Line::Smart => {
                let label = t!("gui.dj.smart").to_string();
                match gui.app.dj_mood_gate() {
                    Some(gate) => {
                        let why = super::gate_words(gate);
                        let room = (text_w as usize).saturating_sub(width(&why) + 4);
                        put(frame, x0, y, &super::super::bar::clip(&label, room), dim());
                        put(frame, right.saturating_sub(width(&why) as u16 + 2), y, &why, dim());
                    }
                    None => {
                        let stop = QStop::Smart;
                        value_row(frame, gui, row, &label, super::moods_value(gui.app.dj.moods), hand(stop), Some(stop));
                    }
                }
            }
            Line::All => {
                let stop = QStop::All;
                let label = format!("{} {forward}", t!("gui.dj.all_settings"));
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

/// `stop` pressed: applied at once.
fn press(gui: &mut Gui, stop: QStop) {
    match stop {
        QStop::Server(i) => {
            let Some(server) = super::pickable_servers(gui).into_iter().nth(i) else { return };
            let effects = gui.app.dj_play_from_set(server.id);
            gui.pend(effects);
            gui.save_now();
        }
        QStop::PlayFrom => super::sheets::open(gui, Sheet::PlayFrom, false),
        QStop::Sonic | QStop::Anchor(_) => {
            let s = &gui.app.dj;
            let (mut on, floor, mut anchor) = (s.sonic, (s.sonic_min_similarity * 100.0).round() as u32, s.sonic_anchor);
            match stop {
                QStop::Sonic => on = !on,
                QStop::Anchor(0) => anchor = SonicAnchor::Rolling,
                _ => anchor = SonicAnchor::Locked,
            }
            super::edit(gui, DjEdit::Sonic { on, floor, anchor });
        }
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

/// The hand off a stop that went: the anchor's with its switch.
fn settle(gui: &mut Gui) {
    let Some(mut quick) = gui.dj.quick else { return };
    let stops = stops(gui, &super::pickable_servers(gui));
    if !stops.contains(&quick.hand) {
        quick.hand = match quick.hand {
            QStop::Anchor(_) => QStop::Sonic,
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
            press(gui, stop);
            settle(gui);
        }
    }
}

/// Its keys while it is up: ↑ ↓ and Tab walk the stops, ← → the anchor's
/// two; Space and Enter press; Esc puts it away. `None` while it is down.
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
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
            // A stowed hand is only shown by the first walking key.
            if quick.shown && !stops.is_empty() {
                let back = matches!(code, KeyCode::Up | KeyCode::Left | KeyCode::BackTab);
                let next = if back { at.saturating_sub(1) } else { (at + 1).min(last) };
                // ↑ ↓ pass the anchor's second choice: it is beside the first.
                let next = match (code, stops[at], stops[next]) {
                    (KeyCode::Down, QStop::Anchor(0), QStop::Anchor(1)) => (next + 1).min(last),
                    (KeyCode::Up, _, QStop::Anchor(1)) if !matches!(stops[at], QStop::Anchor(_)) => next.saturating_sub(1),
                    _ => next,
                };
                quick.hand = stops[next];
            }
            quick.shown = true;
            gui.dj.quick = Some(quick);
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if quick.shown {
                press(gui, quick.hand);
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
