//! The top bar's strip, row 0 of every screen but the mini player: the
//! four tabs (Library, Stats, Admin, MP3 Player) with a sync's mark on
//! the last, the Visualizer item, and at the right the server label, a
//! menu's trigger while two servers or more can be picked, and the [+]
//! that adds one. Its cells face, moved here whole from the shell and the
//! servers module.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::config;
use crate::kit::dim;
use crate::kit::theme::{legacy_conhost, th};

use super::{Act, Gui, Screen, bright_bold, device, put, sel, vizwin};

/// The strip in cells: the tabs and the Visualizer item, then the server
/// label and [+].
pub(super) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    draw_top_tabs(frame, gui);
    draw_header(frame, gui, area);
}

/// The top bar's tabs at the left — Library, Stats, Admin, MP3 Player: the
/// kit's tab slab for the screen that is up, dim text for the others,
/// bright under the pointer. Now Playing has no tab (it opens on `0`), so
/// while it is up no tab wears the slab. While the MP3 Player tab's page
/// writes, the strip is drawn inert — nothing lights, nothing answers a
/// click — since the tab cannot be left (mp3-player-screen contract,
/// clause 9).
fn draw_top_tabs(frame: &mut Frame, gui: &mut Gui) {
    // While a sync runs the MP3 Player tab wears its percentage after its
    // name, in the accent, dim while paused (mp3-player-screen contract,
    // clause 16; the pick of 2026-10-19 over a chip at the bar's right).
    let mark = device::sync_mark(gui);
    let mp3 = match &mark {
        Some((_, pct, _)) => format!("{} {pct}%", t!("gui.top.mp3")),
        None => t!("gui.top.mp3").to_string(),
    };
    let tabs = [
        (Screen::Library, t!("gui.top.library").to_string()),
        (Screen::Stats, t!("sta.title").to_string()),
        (Screen::Admin, t!("gui.top.admin").to_string()),
        (Screen::Device, mp3),
    ];
    let viz = t!("gui.top.viz");
    let rects = top_rects([&tabs[0].1, &tabs[1].1, &tabs[2].1, &tabs[3].1, &viz]);
    let live = !device::writing(gui);
    for ((screen, label), rect) in tabs.into_iter().zip(rects) {
        let style = if gui.screen == screen {
            sel().add_modifier(Modifier::BOLD)
        } else if live && gui.ui.hovers(rect) {
            bright_bold()
        } else {
            dim()
        };
        put(frame, rect.x, 0, &format!(" {label} "), style);
        if let Some((_, pct, paused)) = mark.as_ref().filter(|_| screen == Screen::Device && gui.screen != screen) {
            let words = format!("{pct}%");
            let x = rect.right().saturating_sub(crate::kit::width(&words) as u16 + 1);
            let tone = if *paused { dim() } else { Style::default().fg(th().accent) };
            put(frame, x, 0, &words, tone);
        }
        if live {
            gui.ui.click(rect, Act::Screen(screen));
        }
    }
    // The Visualizer item: not a screen but a window (visualizer-window
    // contract, entry 1) — lit while one is open, bright under the pointer.
    let rect = rects[4];
    let style = if vizwin::is_open(gui) {
        Style::default().fg(th().accent).add_modifier(Modifier::BOLD)
    } else if live && gui.ui.hovers(rect) {
        bright_bold()
    } else {
        dim()
    };
    put(frame, rect.x, 0, &format!(" {viz} "), style);
    if live {
        gui.ui.click(rect, Act::VizWindow);
        gui.ui.tip_keyed(rect, t!("gui.top.viz_tip").to_string());
    }
}

/// Where the top bar's items stand, left to right from x 1 with a cell
/// between them: each its label's cells and a space either side. Cells,
/// not characters: a Japanese or Chinese label is two cells a character,
/// and counting characters left the Admin tab's right half dead under the
/// Visualizer item's rect.
pub(super) fn top_rects<const N: usize>(labels: [&str; N]) -> [Rect; N] {
    let mut x = 1;
    labels.map(|label| {
        let rect = Rect { x, y: 0, width: crate::kit::width(label) as u16 + 2, height: 1 };
        x += rect.width + 1;
        rect
    })
}

/// The top bar's labels, left to right: the four tabs, then the
/// Visualizer item.
pub(super) const TOP_KEYS: [&str; 5] =
    ["gui.top.library", "sta.title", "gui.top.admin", "gui.top.mp3", "gui.top.viz"];

/// Where the top bar's strip ends — the cell after the Visualizer item —
/// in the language on screen: the header's server label starts a blank
/// cell past it, cut short where it would not (servers::draw_header;
/// mp3-player-screen contract, clause 16).
#[cfg(test)]
pub(super) fn top_strip_end() -> u16 {
    top_strip_end_with("")
}

/// [`top_strip_end`] with the MP3 Player tab wearing `mark` (a sync's
/// percentage, clause 16): the server label's cut measures the strip with
/// it.
pub(super) fn top_strip_end_with(mark: &str) -> u16 {
    let mut labels = TOP_KEYS.map(|key| t!(key).to_string());
    if !mark.is_empty() {
        labels[3] = format!("{} {mark}", labels[3]);
    }
    top_rects([&labels[0], &labels[1], &labels[2], &labels[3], &labels[4]])[4].right()
}

/// The header's right corner: the server label (a dropdown trigger once a
/// second server is saved) and the [+] that adds one.
pub(crate) fn draw_header(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let plus = Rect { x: area.width.saturating_sub(5), y: 0, width: 3, height: 1 };
    let plus_hover = gui.ui.hovers(plus);
    put(frame, plus.x, 0, "[+]", if plus_hover { bright_bold() } else { dim() });
    gui.ui.click(plus, Act::SrvAdd);
    gui.ui.tip(plus, t!("gui.srv.add").to_string());

    // The label names the session's server whether or not it answered: a
    // server that would not connect keeps its place here, and the picker
    // with it, so there is always a way to another (contract clause 13).
    if gui.app.session.server_id.is_empty() && gui.app.session.server.is_empty() {
        return;
    }
    let server = gui.app.server_display();
    // Shown only while more than one server can be picked: parked and
    // missing peers do not count (contract clause 12).
    let many = gui.config.servers.iter().filter(|e| config::selectable(e)).count() > 1;
    let chevron = if legacy_conhost() { " v" } else { " ▾" };
    let mark = if many { chevron } else { "" };
    let strip_end = top_strip_end_with(&super::device::strip_mark(gui));
    let Some((x, label)) = header_label(&server, mark, strip_end, plus.x) else { return };
    let width = crate::kit::width(&label) as u16;
    let rect = Rect { x, y: 0, width, height: 1 };
    let hover = many && gui.ui.hovers(rect);
    put(frame, x, 0, &label, if hover { bright_bold() } else { dim() });
    if many {
        // No dwell tooltip here: it matured right where the dropdown
        // opens and sat on top of the first rows.
        gui.ui.click(rect, Act::SrvMenu);
    }
}

/// Where the header's server label stands, and what it says: right-aligned
/// a blank cell short of `[+]` at `plus`, and starting at least a blank cell
/// past the top bar's strip, which ends at `strip_end` — the label must
/// never cover the strip, whose clicks it would take (mp3-player-screen
/// contract, clause 16). A name longer than the room between is cut short
/// with the kit's mark, the menu `mark` kept after it. Cells, not
/// characters. None when not even two cells of the name fit.
pub(super) fn header_label(
    server: &str,
    mark: &str,
    strip_end: u16,
    plus: u16,
) -> Option<(u16, String)> {
    let room = plus.saturating_sub(1).saturating_sub(strip_end + 1) as usize;
    let fits = room.saturating_sub(crate::kit::width(mark));
    if fits < 2 {
        return None;
    }
    let label = format!("{}{mark}", super::bar::clip(server, fits));
    let width = crate::kit::width(&label) as u16;
    Some((plus.saturating_sub(width + 1), label))
}
