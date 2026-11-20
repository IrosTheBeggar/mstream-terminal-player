//! The Peer recs room's peer strip (peer-recs contract §1.5): ` All `, the shelf's peers and an
//! admin's addable ones as tabs on one row, in the server's order.
//!
//! Each tab is its words with a cell of padding either side, and Discover's `TAB_GAP` between
//! tabs. The strip scrolls by whole tabs and never cuts one: a tab that does not fit whole is
//! not drawn, and `‹` or `›` (accent, as each is drawn only when there is more that way) say
//! what lies beyond. The one exception is a single label wider than the row, which is clipped
//! with the kit's mark and drawn alone (build plan D23).
//!
//! The room holds the strip's first tab and the tabs stop by id and resolves them to item
//! indices every frame (build plan D30), so everything here takes indices into the strip's
//! items. Widths are cells (`kit::width`), never characters: a CJK label takes two a glyph.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{Surface, dim, width};
use crate::tui::app::peer_recs::StripView;
use crate::tui::app::peer_recs::rules::{StripAct, StripItem, StripKind};

use super::super::discover::TAB_GAP;
use super::super::{Act, angle_glyphs, bar, bright_bold, bullet_glyph, put};
use super::PeerAct;

/// The cells `‹` takes before the first tab, and `›` after the last: the arrow and a blank.
const ARROW_ROOM: u16 = 2;

/// The skeleton's runs before the first answer: (offset, cells).
const SKELETON: [(u16, u16); 3] = [(0, 5), (7, 16), (25, 15)];

/// One tab in view: its item's index in the strip, where it starts and its width in cells.
/// The width is the tab's own ([`tab_width`]), save for a label wider than the row, which is
/// clipped to the room there is.
pub(super) struct TabCell {
    pub item: usize,
    pub x: u16,
    pub w: u16,
}

/// The tabs in view from a first tab, and whether more lie beyond either edge.
pub(super) struct TabRun {
    pub cells: Vec<TabCell>,
    pub left: bool,
    pub right: bool,
}

impl TabRun {
    /// The item is in view.
    fn shows(&self, item: usize) -> bool {
        self.cells.iter().any(|cell| cell.item == item)
    }
}

// ── A tab's face ────────────────────────────────────────────────────────────

/// What a piece of a tab's face is, which decides its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    /// A blank: the padding either side, and the cell after `•` or `+`.
    Pad,
    /// The online dot.
    Dot,
    /// An addable tab's `+`.
    Plus,
    /// The tab's name: `All`, or the server's label verbatim.
    Name,
    /// The words of an Add state around the label: `adding…`, `no room`, `try again`.
    Words,
}

/// How a tab is worn this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wear {
    /// Its own colours (the table of peer-recs contract §1.5).
    Rest,
    /// The slab across its padded width: the selected tab, or the tab the stop holds.
    Slab,
    /// The selected tab while the stop holds another: accent BOLD, its `•` still ok.
    Picked,
    /// Under the pointer: bright BOLD, its `•` still ok.
    Hover,
}

/// A tab's face, piece by piece, padding included.
fn face(item: &StripItem) -> Vec<(String, Part)> {
    let pad = || (" ".to_string(), Part::Pad);
    let name = || (item.label.clone(), Part::Name);
    let mut out = vec![pad()];
    match item.kind {
        StripKind::All => out.push((t!("gui.peer_recs.all").to_string(), Part::Name)),
        StripKind::Shelf { online: true } => {
            out.extend([(bullet_glyph().to_string(), Part::Dot), pad(), name()]);
        }
        StripKind::Shelf { online: false } => out.push(name()),
        StripKind::Addable => out.extend([("+".to_string(), Part::Plus), pad(), name()]),
        StripKind::Adding => out.extend(worded(&t!("gui.peer_recs.tab_adding"), &item.label)),
        StripKind::NoRoom => out.extend(worded(&t!("gui.peer_recs.tab_no_room"), &item.label)),
        StripKind::TryAgain => out.extend(worded(&t!("gui.peer_recs.tab_try_again"), &item.label)),
    }
    out.push(pad());
    out
}

/// An Add state's template split at its `%{peer}`: the words before the label, the label, and
/// the words after, wherever the locale puts the placeholder. A template without one reads as
/// the label, then the words.
fn worded(template: &str, label: &str) -> Vec<(String, Part)> {
    let (before, after) = template.split_once("%{peer}").unwrap_or(("", template));
    let mut out = Vec::new();
    if !before.is_empty() {
        out.push((before.to_string(), Part::Words));
    }
    out.push((label.to_string(), Part::Name));
    if !after.is_empty() {
        out.push((after.to_string(), Part::Words));
    }
    out
}

/// A piece's colour at rest (peer-recs contract §1.5): All and an offline peer dim, an online
/// peer's name text with its `•` ok, an addable `+ {label}` accent, adding gold, no room dim,
/// and try again's label gold (the GUI's failure tone) with its words accent BOLD (the GUI's
/// again button). The padding is ground.
fn rest_style(kind: StripKind, part: Part) -> Style {
    let t = th();
    match (kind, part) {
        (_, Part::Pad) => Style::default(),
        (_, Part::Dot) => Style::default().fg(t.ok),
        (StripKind::Shelf { online: true }, _) => Style::default().fg(t.text),
        (StripKind::All | StripKind::Shelf { online: false } | StripKind::NoRoom, _) => dim(),
        (StripKind::Addable, _) => Style::default().fg(t.accent),
        (StripKind::Adding, _) => Style::default().fg(t.gold),
        (StripKind::TryAgain, Part::Words) => {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        }
        (StripKind::TryAgain, _) => Style::default().fg(t.gold),
    }
}

/// A piece's colour as the tab is worn.
fn style_of(kind: StripKind, part: Part, wear: Wear) -> Style {
    let t = th();
    match (wear, part) {
        (Wear::Slab, _) => slab(),
        (Wear::Rest, _) | (_, Part::Pad | Part::Dot) => rest_style(kind, part),
        (Wear::Picked, _) => Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        (Wear::Hover, _) => bright_bold(),
    }
}

/// The kit's cursor and selection: on_accent on accent, BOLD.
fn slab() -> Style {
    Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
}

/// A tab's padded width in cells.
pub(super) fn tab_width(item: &StripItem) -> u16 {
    let cells: usize = face(item).iter().map(|(text, _)| width(text)).sum();
    u16::try_from(cells).unwrap_or(u16::MAX)
}

/// Draw a tab's face in `rect`. A face wider than the rect (the one clipped tab) keeps what
/// fits and ends in the kit's clip mark, in the colour of the piece it cut.
fn put_face(frame: &mut Frame, rect: Rect, item: &StripItem, wear: Wear) {
    if wear == Wear::Slab {
        put(frame, rect.x, rect.y, &" ".repeat(usize::from(rect.width)), slab());
    }
    let parts = face(item);
    let total: usize = parts.iter().map(|(text, _)| width(text)).sum();
    let room = usize::from(rect.width);
    let mut used = 0;
    for (k, (text, part)) in parts.iter().enumerate() {
        let style = style_of(item.kind, *part, wear);
        let x = rect.x.saturating_add(used as u16);
        let w = width(text);
        if total > room && used + w >= room {
            let rest: String = parts[k..].iter().map(|(text, _)| text.as_str()).collect();
            put(frame, x, rect.y, &bar::clip(&rest, room - used), style);
            return;
        }
        put(frame, x, rect.y, text, style);
        used += w;
    }
}

// ── The run ─────────────────────────────────────────────────────────────────

/// The tabs that fit whole in `row` from the item `first` on. With tabs hidden to the left,
/// `‹` takes the row's first cell and the run starts two cells in. When every tab from `first`
/// fits, the run may reach the row's last cell; otherwise it ends a blank before `›`, which
/// takes the last cell. A first tab that cannot fit even alone is clipped to the room there is.
pub(super) fn run(items: &[StripItem], row: Rect, first: usize) -> TabRun {
    let first = first.min(items.len().saturating_sub(1));
    let left = first > 0;
    let start = if left { row.x.saturating_add(ARROW_ROOM) } else { row.x };
    let end = row.right();
    let widths: Vec<u16> = items.iter().skip(first).map(tab_width).collect();
    let gaps = TAB_GAP.saturating_mul(widths.len().saturating_sub(1) as u16);
    let all = widths.iter().fold(gaps, |sum, w| sum.saturating_add(*w));
    let limit = if start.saturating_add(all) <= end { end } else { end.saturating_sub(ARROW_ROOM) };
    let mut cells = Vec::new();
    let mut x = start;
    for (k, w) in widths.iter().enumerate() {
        if x.saturating_add(*w) > limit {
            break;
        }
        cells.push(TabCell { item: first + k, x, w: *w });
        x = x.saturating_add(*w).saturating_add(TAB_GAP);
    }
    if cells.is_empty() && !widths.is_empty() {
        let limit = if widths.len() == 1 { end } else { end.saturating_sub(ARROW_ROOM) };
        let w = limit.saturating_sub(start);
        if w > 0 {
            cells.push(TabCell { item: first, x: start, w });
        }
    }
    let right = first + cells.len() < items.len();
    TabRun { cells, left, right }
}

/// Scroll `first` by whole tabs until the item `item` is in view: back to it when it lies to
/// the left, else one tab at a time until the run shows it.
pub(super) fn reveal(items: &[StripItem], row: Rect, first: &mut usize, item: usize) {
    let item = item.min(items.len().saturating_sub(1));
    if item < *first {
        *first = item;
        return;
    }
    while *first < item && !run(items, row, *first).shows(item) {
        *first += 1;
    }
}

/// Scroll the strip `delta` whole tabs, a `‹` `›` click being one (peer-recs contract §1.5) and
/// so a wheel notch (build plan D23). It stops at the first tab, and once the last tab is in
/// view, where `›` is no longer drawn.
pub(super) fn scroll(items: &[StripItem], row: Rect, first: usize, delta: i32) -> usize {
    let mut first = first.min(items.len().saturating_sub(1));
    for _ in 0..delta.unsigned_abs() {
        if delta > 0 {
            if !run(items, row, first).right {
                break;
            }
            first += 1;
        } else {
            let Some(back) = first.checked_sub(1) else { break };
            first = back;
        }
    }
    first
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The selected tab's index: All when `selected` is `None`, else the shelf tab of that id.
fn selected_at(view: &StripView) -> Option<usize> {
    view.items.iter().position(|item| item.selectable() && item.id == view.selected)
}

/// What a click on a tab does: select it, or POST its Add. An inert tab takes no click.
fn act_of(item: &StripItem) -> Option<Act> {
    match item.acts() {
        StripAct::Select => Some(Act::Peer(PeerAct::Tab(item.id.clone()))),
        StripAct::Add => item.id.clone().map(|id| Act::Peer(PeerAct::Add(id))),
        StripAct::Nothing => None,
    }
}

/// The strip on `row` from the item `first` on, `stop` the item the tabs stop holds (peer-recs
/// contract §1.5). The selected tab is the slab. While the stop holds another tab, that tab is
/// the slab and the selected one reads accent BOLD; with the stop on the selected tab the strip
/// reads as at rest. Each live tab registers its act (`Tab`, or `Add` for `+` and try again),
/// a `+` tab its "Add" tooltip, and `‹` `›` scroll one tab (`TabsScroll`). A strip that is not
/// drawn (before the first answer, or All alone) leaves the row blank.
pub(super) fn draw(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    row: Rect,
    view: &StripView,
    first: usize,
    stop: Option<usize>,
) {
    if !view.drawn || view.items.is_empty() {
        return;
    }
    let run = run(&view.items, row, first);
    let selected = selected_at(view);
    for cell in &run.cells {
        let item = &view.items[cell.item];
        let rect = Rect { x: cell.x, y: row.y, width: cell.w, height: 1 };
        let act = act_of(item);
        let wear = if stop == Some(cell.item) || (stop.is_none() && selected == Some(cell.item)) {
            Wear::Slab
        } else if act.is_some() && ui.hovers(rect) {
            Wear::Hover
        } else if selected == Some(cell.item) {
            Wear::Picked
        } else {
            Wear::Rest
        };
        put_face(frame, rect, item, wear);
        if let Some(act) = act {
            ui.click(rect, act);
        }
        if item.kind == StripKind::Addable {
            ui.tip(rect, t!("gui.peer_recs.add"));
        }
    }
    let (back, more) = angle_glyphs();
    if run.left {
        arrow(frame, ui, Rect { x: row.x, y: row.y, width: 1, height: 1 }, back, -1);
    }
    if run.right {
        let x = row.right().saturating_sub(1);
        arrow(frame, ui, Rect { x, y: row.y, width: 1, height: 1 }, more, 1);
    }
}

/// One of the strip's arrows: accent, bright BOLD under the pointer, a click scrolling one tab.
fn arrow(frame: &mut Frame, ui: &mut Surface<Act>, rect: Rect, glyph: &str, delta: i32) {
    let style = if ui.hovers(rect) { bright_bold() } else { Style::default().fg(th().accent) };
    put(frame, rect.x, rect.y, glyph, style);
    ui.click(rect, Act::Peer(PeerAct::TabsScroll(delta)));
}

/// The strip before the first answer: dim `░` runs of 5, 16 and 15 cells at +0, +7 and +25
/// (`·` on the legacy console), each cut at the row's end.
pub(super) fn draw_skeleton(frame: &mut Frame, row: Rect) {
    let glyph = if legacy_conhost() { "·" } else { "░" };
    for (at, cells) in SKELETON {
        let x = row.x.saturating_add(at);
        let cells = cells.min(row.right().saturating_sub(x));
        if cells > 0 {
            put(frame, x, row.y, &glyph.repeat(usize::from(cells)), dim());
        }
    }
}

// ── The tabs stop ───────────────────────────────────────────────────────────

/// Where the tabs stop lands when the keys reach the strip (peer-recs contract §1.2): the
/// selected tab, or the first tab in view when the selected one is scrolled out. The stop
/// skips inert tabs (adding…, no room), so with none live in view it falls back to the first
/// live tab of the strip, which All always is. `None` when the strip is not drawn.
pub(super) fn stop_landing(view: &StripView, run: &TabRun) -> Option<usize> {
    if !view.drawn {
        return None;
    }
    let live = |item: usize| view.items.get(item).is_some_and(|tab| !tab.inert());
    selected_at(view)
        .filter(|&item| run.shows(item))
        .or_else(|| run.cells.iter().map(|cell| cell.item).find(|&item| live(item)))
        .or_else(|| (0..view.items.len()).find(|&item| live(item)))
}

/// The tab the stop steps to from the item `at` (`dir` −1 or 1), skipping inert tabs. `None` at
/// either end, where the stop stays.
pub(super) fn stop_step(items: &[StripItem], at: usize, dir: i32) -> Option<usize> {
    let live = |item: &usize| !items[*item].inert();
    match dir.signum() {
        1 => (at.saturating_add(1)..items.len()).find(live),
        -1 => (0..at.min(items.len())).rev().find(live),
        _ => None,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;

    use super::*;
    use crate::api::types::fixtures::{self, ATTIC, KITCHEN, LIGHTHOUSE, ROOFTOP};
    use crate::kit::boards::{Area, assert_frame, style_code};
    use crate::tui::app::peer_recs::rules::strip_items;

    use super::super::kit::english;

    /// The tabs row at 100×30: row 4, from the content's x 17, 80 cells (47 with the queue open).
    fn row(width: u16) -> Rect {
        Rect { x: 17, y: 4, width, height: 1 }
    }

    /// The strip of a peers fixture, as the App builds it.
    fn items(name: &str) -> Vec<StripItem> {
        strip_items(Some(&fixtures::peers(name)), &BTreeMap::new(), 1)
    }

    fn view(name: &str, selected: Option<&str>) -> StripView {
        let items = items(name);
        let drawn = items.len() > 1;
        StripView { items, selected: selected.map(str::to_string), drawn, skeleton: false }
    }

    /// The index of the tab with `id` (`None`: All).
    fn at(items: &[StripItem], id: Option<&str>) -> usize {
        items.iter().position(|item| item.id.as_deref() == id).expect("the tab is on the strip")
    }

    /// A drawn strip of hand-made tabs.
    fn made(items: Vec<StripItem>, selected: Option<&str>) -> StripView {
        StripView { items, selected: selected.map(str::to_string), drawn: true, skeleton: false }
    }

    /// Draw the strip on a 100×30 screen.
    fn strip(view: &StripView, width: u16, first: usize, stop: Option<usize>)
        -> (Buffer, Surface<Act>)
    {
        let mut term = Terminal::new(TestBackend::new(100, 30)).expect("a test terminal");
        let mut ui = Surface::new();
        term.draw(|frame| draw(frame, &mut ui, row(width), view, first, stop)).expect("a frame");
        (term.backend().buffer().clone(), ui)
    }

    /// What a press on the tabs row at `x` hits.
    fn hit(ui: &Surface<Act>, x: u16) -> Option<Act> {
        ui.hit(Position { x, y: 4 })
    }

    fn select(id: &str) -> Option<Act> {
        Some(Act::Peer(PeerAct::Tab(Some(id.to_string()))))
    }

    fn add(id: &str) -> Option<Act> {
        Some(Act::Peer(PeerAct::Add(id.to_string())))
    }

    fn line(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    fn cells(run: &TabRun) -> Vec<(usize, u16, u16)> {
        run.cells.iter().map(|cell| (cell.item, cell.x, cell.w)).collect()
    }

    /// A hand-made tab.
    fn tab(id: &str, label: &str, kind: StripKind) -> StripItem {
        StripItem { id: Some(id.to_string()), label: label.to_string(), kind }
    }

    fn all() -> StripItem {
        StripItem { id: None, label: String::new(), kind: StripKind::All }
    }

    #[test]
    fn reg_strip_frames_match() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        // 01: admin, All selected; the `+` tabs lie beyond `›`.
        let (buf, _) = strip(&view("peers_admin", None), 80, 0, None);
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 1));
        // 02: a member: no addable tabs, so no `›`.
        let (buf, _) = strip(&view("peers_member", None), 80, 0, None);
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 2));
        // 03: Kitchen Radio selected, scrolled two tabs.
        let (buf, _) = strip(&view("peers_admin", Some(KITCHEN)), 80, 2, None);
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 3));
        // 04: Attic Tapes selected.
        let (buf, _) = strip(&view("peers_admin", Some(ATTIC)), 80, 0, None);
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 4));
        // 05: an admin's empty shelf, at rest.
        let (buf, _) = strip(&view("peers_admin_empty_shelf", None), 80, 0, None);
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 5));
        // 06: the strip's end: no room, adding… and try again.
        let add_states = view("peers_admin_add_states", None);
        let first = at(&add_states.items, Some(LIGHTHOUSE));
        let (buf, _) = strip(&add_states, 80, first, None);
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 6));
        // 07: before the first answer.
        let mut term = Terminal::new(TestBackend::new(100, 30)).expect("a test terminal");
        term.draw(|frame| draw_skeleton(frame, row(80))).expect("a frame");
        assert_frame(term.backend().buffer(), "REG-STRIP", "80", Area::at(4, 7));
        // 08: the tabs stop on `+ Lighthouse Tapes`, All still selected.
        let empty = view("peers_admin_empty_shelf", None);
        let stop = at(&empty.items, Some(LIGHTHOUSE));
        let (buf, _) = strip(&empty, 80, 0, Some(stop));
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 8));
        // The queue open: 47 cells, `›` at x 63.
        let (buf, _) = strip(&view("peers_admin", None), 47, 0, None);
        assert_frame(&buf, "REG-STRIP", "47", Area::at(4, 1));
    }

    #[test]
    fn tabs_scroll_by_whole_tabs_and_never_cut_one() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        let admin = items("peers_admin");
        // M1: 17–21, 24–41, 44–60, 63–75 and 78–93, `›` at 96.
        let m1 = run(&admin, row(80), 0);
        assert_eq!(cells(&m1), [(0, 17, 5), (1, 24, 18), (2, 44, 17), (3, 63, 13), (4, 78, 16)]);
        assert!(!m1.left && m1.right);
        // V3, the queue open: 17–21, 24–41 and 44–60, `›` at 63.
        let v3 = run(&admin, row(47), 0);
        assert_eq!(cells(&v3), [(0, 17, 5), (1, 24, 18), (2, 44, 17)]);
        assert!(!v3.left && v3.right);
        // M4: `‹` at 17, the first tab at 19–35.
        let m4 = run(&admin, row(80), 2);
        assert_eq!(cells(&m4), [(2, 19, 17), (3, 38, 13), (4, 53, 16), (5, 71, 20)]);
        assert!(m4.left && m4.right);
        // At every width and from every first tab, each tab in view is whole and clear of the
        // arrows, and the next tab is drawn only where the last one ended plus the gap.
        for name in fixtures::PEERS {
            let strip = items(name);
            for width in 47..=80 {
                for first in 0..strip.len() {
                    let rect = row(width);
                    let laid = run(&strip, rect, first);
                    let end = if laid.right { rect.right() - 2 } else { rect.right() };
                    let start = if laid.left { rect.x + 2 } else { rect.x };
                    let mut x = start;
                    for cell in &laid.cells {
                        let case = format!("{name} at {width} cells from {first}");
                        assert_eq!(cell.w, tab_width(&strip[cell.item]), "{case}: a tab cut");
                        assert_eq!(cell.x, x, "{case}: the gap");
                        assert!(cell.x + cell.w <= end, "{case}: under an arrow");
                        x = cell.x + cell.w + TAB_GAP;
                    }
                    let shown: Vec<usize> = laid.cells.iter().map(|cell| cell.item).collect();
                    let whole: Vec<usize> = (first..first + shown.len()).collect();
                    assert_eq!(shown, whole, "{name} {width} {first}: in order, none skipped");
                    assert!(!shown.is_empty(), "{name} {width} {first}: a tab in view");
                    assert_eq!(laid.left, first > 0);
                    assert_eq!(laid.right, first + shown.len() < strip.len());
                }
            }
        }
    }

    #[test]
    fn the_arrows_show_only_when_there_is_more_that_way() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        let accent = |buf: &Buffer, x: u16| {
            style_code(&buf[(x, 4)], &|_| None) == 'a' && buf[(x, 4)].fg == th().accent
        };
        // All in view: neither arrow.
        let (buf, _) = strip(&view("peers_member", None), 80, 0, None);
        assert!(!line(&buf, 4).contains('‹') && !line(&buf, 4).contains('›'), "{}", line(&buf, 4));
        // More to the right only: `›` in accent at the row's last cell.
        let (buf, _) = strip(&view("peers_admin", None), 80, 0, None);
        assert_eq!(buf[(96, 4)].symbol(), "›");
        assert!(accent(&buf, 96) && !line(&buf, 4).contains('‹'));
        // Both ways.
        let (buf, _) = strip(&view("peers_admin", None), 80, 2, None);
        assert_eq!((buf[(17, 4)].symbol(), buf[(96, 4)].symbol()), ("‹", "›"));
        assert!(accent(&buf, 17) && accent(&buf, 96));
        // The strip's end: `‹` alone.
        let add_states = view("peers_admin_add_states", None);
        let first = at(&add_states.items, Some(LIGHTHOUSE));
        let (buf, _) = strip(&add_states, 80, first, None);
        assert!(accent(&buf, 17) && !line(&buf, 4).contains('›'), "{}", line(&buf, 4));
        // Under the pointer an arrow reads bright BOLD.
        let mut term = Terminal::new(TestBackend::new(100, 30)).expect("a test terminal");
        let mut ui = Surface::new();
        ui.pointer = Some(Position { x: 96, y: 4 });
        let admin = view("peers_admin", None);
        term.draw(|frame| draw(frame, &mut ui, row(80), &admin, 0, None)).expect("a frame");
        assert_eq!(term.backend().buffer()[(96, 4)].fg, th().bright);
        // Legacy console stand-ins.
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let (buf, _) = strip(&view("peers_admin", None), 80, 2, None);
        assert_eq!((buf[(17, 4)].symbol(), buf[(96, 4)].symbol()), ("<", ">"));
    }

    #[test]
    fn arrow_clicks_scroll_one_tab_and_move_nothing_else() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        // KITCHEN RADIO TAB: Kitchen Radio selected, then two `›` clicks.
        let kitchen = view("peers_admin", Some(KITCHEN));
        let mut first = 0;
        for _ in 0..2 {
            let (_, ui) = strip(&kitchen, 80, first, None);
            let scrolls = Some(Act::Peer(PeerAct::TabsScroll(1)));
            assert_eq!(hit(&ui, 96), scrolls, "`›` scrolls, nothing else");
            first = scroll(&kitchen.items, row(80), first, 1);
        }
        assert_eq!(first, 2, "one tab a click");
        let (buf, ui) = strip(&kitchen, 80, first, None);
        assert_frame(&buf, "REG-STRIP", "80", Area::at(4, 3));
        assert_eq!(kitchen.selected.as_deref(), Some(KITCHEN), "the selection stays");
        // `‹` scrolls one tab back.
        assert_eq!(hit(&ui, 17), Some(Act::Peer(PeerAct::TabsScroll(-1))));
        assert_eq!(scroll(&kitchen.items, row(80), first, -1), 1);
        // The arrows' blanks take no click, and the wheel stops at either end.
        assert_eq!(hit(&ui, 18), None);
        assert_eq!(hit(&ui, 95), None);
        assert_eq!(scroll(&kitchen.items, row(80), 0, -3), 0);
        let last = scroll(&kitchen.items, row(80), 0, 99);
        assert!(!run(&kitchen.items, row(80), last).right, "the last tab is in view");
        assert!(run(&kitchen.items, row(80), last - 1).right, "and not one tab further");
        // Tabs take their own acts: select, or Add.
        let (_, ui) = strip(&kitchen, 80, first, None);
        assert_eq!(hit(&ui, 30), select(KITCHEN));
        assert_eq!(hit(&ui, 80), add(LIGHTHOUSE));
        assert_eq!(hit(&ui, 36), None, "the gap takes no click");
    }

    #[test]
    fn the_stop_skips_inert_tabs_and_lands_on_the_selected_or_first_visible() {
        let _en = english();
        let add_states = view("peers_admin_add_states", None);
        let items = &add_states.items;
        let lighthouse = at(items, Some(LIGHTHOUSE));
        let rooftop = at(items, Some(ROOFTOP));
        assert!(items[lighthouse].inert() && items[lighthouse + 1].inert());
        assert!(!items[rooftop].inert());
        // → from the last shelf tab skips no room and adding… to try again; ← comes back.
        assert_eq!(stop_step(items, lighthouse - 1, 1), Some(rooftop));
        assert_eq!(stop_step(items, rooftop, -1), Some(lighthouse - 1));
        // Either end holds.
        assert_eq!(stop_step(items, rooftop, 1), None);
        assert_eq!(stop_step(items, 0, -1), None);
        assert_eq!(stop_step(items, 0, 0), None);
        // The selected tab in view: the stop lands there.
        let kitchen = view("peers_admin", Some(KITCHEN));
        let landing = stop_landing(&kitchen, &run(&kitchen.items, row(80), 0));
        assert_eq!(landing, Some(at(&kitchen.items, Some(KITCHEN))));
        // Scrolled out: the first tab in view.
        let attic = at(&kitchen.items, Some(ATTIC));
        let landing = stop_landing(&kitchen, &run(&kitchen.items, row(80), attic));
        assert_eq!(landing, Some(attic));
        // The first in view inert: the first live one in view.
        let landing = stop_landing(&add_states, &run(items, row(80), lighthouse));
        assert_eq!(landing, Some(rooftop));
        // Nothing live in view: the strip's first live tab, All.
        let adding = tab("a", "Adding Tapes", StripKind::Adding);
        let lone = made(vec![all(), adding, tab("b", "Full", StripKind::NoRoom)], Some("gone"));
        let landing = stop_landing(&lone, &run(&lone.items, row(30), 1));
        assert_eq!(landing, Some(0));
        // No strip, no stop.
        let member = StripView { drawn: false, ..made(vec![all()], None) };
        assert_eq!(stop_landing(&member, &run(&member.items, row(80), 0)), None);
    }

    #[test]
    fn try_again_splits_at_the_placeholder() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        let t = th();
        // The English template: the label, then the words.
        let rooftop = tab(ROOFTOP, "Rooftop FM", StripKind::TryAgain);
        assert_eq!(
            face(&rooftop),
            [
                (" ".to_string(), Part::Pad),
                ("Rooftop FM".to_string(), Part::Name),
                (" try again".to_string(), Part::Words),
                (" ".to_string(), Part::Pad),
            ]
        );
        assert_eq!(rest_style(StripKind::TryAgain, Part::Name), Style::default().fg(t.gold));
        assert_eq!(
            rest_style(StripKind::TryAgain, Part::Words),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        );
        // A template with the label in the middle keeps both sides' words.
        assert_eq!(
            worded("retry %{peer} now", "Rooftop FM"),
            [
                ("retry ".to_string(), Part::Words),
                ("Rooftop FM".to_string(), Part::Name),
                (" now".to_string(), Part::Words),
            ]
        );
        // The label first, and a template without the placeholder.
        assert_eq!(
            worded("%{peer} 再試行", "屋上"),
            [("屋上".to_string(), Part::Name), (" 再試行".to_string(), Part::Words)]
        );
        assert_eq!(
            worded("try again", "Rooftop FM"),
            [("Rooftop FM".to_string(), Part::Name), ("try again".to_string(), Part::Words)]
        );
        // Drawn: the label gold, the words accent BOLD, the pads ground.
        let lone = made(vec![all(), rooftop], None);
        let (buf, ui) = strip(&lone, 80, 0, None);
        let x = 17 + 5 + TAB_GAP;
        assert_eq!(line(&buf, 4)[17..].trim_end(), " All    Rooftop FM try again");
        assert_eq!(style_code(&buf[(x + 1, 4)], &|_| None), 'g');
        assert_eq!(style_code(&buf[(x + 12, 4)], &|_| None), 'A');
        assert_eq!(buf[(x, 4)].bg, ratatui::style::Color::Reset);
        // It POSTs the Add again.
        assert_eq!(hit(&ui, x + 3), add(ROOFTOP));
        // Inert tabs take no click.
        let adding = tab("a", "Static Garden", StripKind::Adding);
        let inert = made(vec![all(), adding, tab("b", "Full", StripKind::NoRoom)], None);
        let (buf, ui) = strip(&inert, 80, 0, None);
        assert_eq!(line(&buf, 4)[17..].trim_end(), " All    Static Garden adding…    Full no room");
        for x in 24..=62 {
            assert_eq!(hit(&ui, x), None, "x {x}");
        }
    }

    #[test]
    fn labels_measure_by_cells_not_chars() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        let kitchen = tab("k", "台所ラジオ", StripKind::Shelf { online: false });
        // Five characters, ten cells, plus the padding.
        assert_eq!(tab_width(&kitchen), 12);
        let online = tab("o", "台所ラジオ", StripKind::Shelf { online: true });
        assert_eq!(tab_width(&online), 14);
        let plus = tab("p", "屋上", StripKind::Addable);
        assert_eq!(tab_width(&plus), 8);
        let strip_view = made(vec![all(), kitchen, plus], None);
        let laid = run(&strip_view.items, row(80), 0);
        assert_eq!(cells(&laid), [(0, 17, 5), (1, 24, 12), (2, 38, 8)]);
        let (buf, ui) = strip(&strip_view, 80, 0, None);
        assert_eq!(buf[(25, 4)].symbol(), "台");
        assert_eq!(buf[(39, 4)].symbol(), "+");
        assert_eq!(buf[(41, 4)].symbol(), "屋");
        assert_eq!(hit(&ui, 37), None, "the gap after the wide label");
        assert_eq!(hit(&ui, 38), add("p"));
        // A run of wide labels breaks where the cells run out, not the characters.
        let offline = StripKind::Shelf { online: false };
        let wide: Vec<StripItem> = std::iter::once(all())
            .chain((0..6).map(|k| tab(&k.to_string(), "台所ラジオ", offline)))
            .collect();
        let laid = run(&wide, row(47), 0);
        assert_eq!(cells(&laid), [(0, 17, 5), (1, 24, 12), (2, 38, 12)]);
        assert!(laid.right);
    }

    #[test]
    fn a_label_wider_than_the_row_is_clipped_alone() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        let long = "A Very Long Peer Name That Runs Past Every Edge Of The Strip ".repeat(2);
        assert!(width(&long) > 80);
        let wide = tab("w", long.trim_end(), StripKind::Shelf { online: true });
        let items = vec![all(), wide.clone(), tab("x", "Next", StripKind::Shelf { online: false })];
        // From All: the wide tab does not fit, and `›` says so.
        let from_all = run(&items, row(80), 0);
        assert_eq!(cells(&from_all), [(0, 17, 5)]);
        assert!(from_all.right);
        // Scrolled to it: drawn alone, clipped clear of `›`.
        let alone = run(&items, row(80), 1);
        assert_eq!(cells(&alone), [(1, 19, 76)]);
        assert!(alone.left && alone.right);
        let view = made(items.clone(), Some("w"));
        let (buf, ui) = strip(&view, 80, 1, None);
        let drawn = line(&buf, 4);
        assert!(drawn.contains("• A Very Long Peer Name"), "{drawn}");
        assert_eq!(buf[(94, 4)].symbol(), "…", "{drawn}");
        assert_eq!(buf[(95, 4)].symbol(), " ");
        assert_eq!(buf[(96, 4)].symbol(), "›");
        // The slab spans exactly the clipped width.
        assert_eq!(style_code(&buf[(19, 4)], &|_| None), 'S');
        assert_eq!(style_code(&buf[(94, 4)], &|_| None), 'S');
        assert_eq!(buf[(95, 4)].bg, ratatui::style::Color::Reset);
        assert_eq!(hit(&ui, 60), select("w"));
        // The last tab, alone: clipped at the row's end, with no `›`.
        let last = vec![all(), wide];
        let alone = run(&last, row(80), 1);
        assert_eq!(cells(&alone), [(1, 19, 78)]);
        assert!(!alone.right);
        // reveal reaches it in one step and never runs past it.
        let mut first = 0;
        reveal(&items, row(80), &mut first, 1);
        assert_eq!(first, 1);
        reveal(&items, row(80), &mut first, 2);
        assert_eq!(first, 2);
        reveal(&items, row(80), &mut first, 0);
        assert_eq!(first, 0);
    }

    #[test]
    fn a_plus_tab_carries_the_add_tooltip() {
        let _en = english();
        crate::kit::theme::pin_modern_terminal();
        let (_, ui) = strip(&view("peers_admin_empty_shelf", None), 80, 0, None);
        let lighthouse = Rect { x: 24, y: 4, width: 20, height: 1 };
        assert!(ui.tips.contains(&(lighthouse, "Add".to_string())), "{:?}", ui.tips);
        // Only the `+` tabs carry it: three in view, and none on All.
        assert_eq!(ui.tips.len(), 3, "{:?}", ui.tips);
        assert!(ui.tips.iter().all(|(rect, text)| text == "Add" && rect.x > 21));
        let (_, ui) = strip(&view("peers_member", None), 80, 0, None);
        assert!(ui.tips.is_empty(), "{:?}", ui.tips);
        let (_, ui) = strip(&view("peers_admin_add_states", None), 80, 5, None);
        assert!(ui.tips.is_empty(), "no `+` among no room, adding… and try again: {:?}", ui.tips);
    }
}
