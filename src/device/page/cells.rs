//! The MP3 Player page in character cells: what `mstream-player device
//! flash` draws in a terminal, and what the GUI's MP3 Player tab draws on
//! its grid ([`render_hosted`]). The page itself (its state, the worker's
//! reports, its keys, its hints and its gates' words) is the parent
//! module, page.rs; this is its view in cells, and holds what is measured
//! in cells: the header and the foot, the tab row of subjects, the board's
//! card laid out in rows, Details and its log, the card with no board, a
//! card's view and the gate, on the kit's surface. A control is one of the
//! page's [`Act`]s, registered as it is drawn; the page's [`Screen`] impl
//! hands this a frame, and does what a click says.
//!
//! Its two children are the drawing halves of the page's two: card.rs
//! draws an SD card's view, its gates and its Settings sheet (page/card.rs
//! is their model), sheet.rs the Advanced options sheet (page/sheet.rs).

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::*;
use crate::admin::{draw_foot, draw_header_as, frame_ground};
use crate::device::board::{CardRoot, CardUnknown, Count, CountWhy, NextState, gb};
use crate::device::firmware::{Check, clock_at};
use crate::kit::theme::th;
use crate::kit::{accent, bold, dim};

/// An SD card's view, its gates and its Settings sheet in cells: the
/// drawing half of page/card.rs.
pub(super) mod card;
/// The Advanced options sheet in cells: the drawing half of page/sheet.rs.
pub(super) mod sheet;

/// The standalone page's least window: the card at 68 cells with its
/// tallest content, the header above and the busy and tips rows below.
pub(super) const MIN_W: u16 = 72;
const MIN_H: u16 = 24;
/// The card, border to border: today's 78-cell column.
pub(super) const CARD_W: u16 = 78;
/// The border and two cells of padding, each side — one cell on a card
/// narrower than the column (the console's 68, whose bar is then 50).
const CARD_INSET: u16 = 3;
/// The label column: wide enough for French "Micrologiciel" and a gap.
pub(super) const LABEL_W: u16 = 14;
/// The gate's width: the top of the kit's 52–74 range, so it covers the
/// card's words and only the card's border shows at its sides.
pub(super) const GATE_W: u16 = 74;
/// The boards Update all's gate names before "and N more": six fit the
/// GUI's floor.
const GATE_ROWS: usize = 6;
/// A tall button's rows.
const BUTTON_H: u16 = 3;
/// The clock's width in a log line: `mm:ss` and two spaces.
const CLOCK_W: usize = 7;

// ── Drawing ─────────────────────────────────────────────────────────────────

pub(super) fn render(frame: &mut Frame, page: &mut Page) {
    page.ui.begin_frame();
    page.log_rect = None;
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    draw(frame, page, area, false);
}

/// The page inside another shell's `area` (the GUI's MP3 Player tab,
/// contract clauses 1 and 4): no ground, no header and no tips row — the
/// host's top bar and footer carry those — and the area's first row left
/// blank for the host. Nothing lands outside the area: the card stops
/// above the area's last row, which is the busy line's, and what does not
/// fit is cut inside the card's border rather than spilled
/// (docs/ui-kit.md, "Hosted rooms").
pub(super) fn render_hosted(frame: &mut Frame, page: &mut Page, area: Rect) {
    page.ui.begin_frame();
    page.log_rect = None;
    // One row is the host's blank one and nothing more: not even the busy
    // line has a row of its own.
    if area.width == 0 || area.height < 2 {
        return;
    }
    draw(frame, page, area, true);
}

/// The page in `area`: on its own, under its header with the tips on the
/// last row; hosted, under a blank row with only the busy line below it.
fn draw(frame: &mut Frame, page: &mut Page, area: Rect, hosted: bool) {
    if !hosted {
        // The image the page writes, its build named where it is DIO
        // (clause 43): `firmware v0.7.0 · DIO · release`.
        let right = match &page.image {
            Some(image) => {
                let word = t!(match image.kind {
                    Origin::Release => "dev.kind_release",
                    Origin::Cached => "dev.kind_cached",
                    Origin::File => "dev.kind_file",
                });
                let version = match image.mode {
                    Some(Mode::Dio) => format!("{} · {}", image.version, Mode::Dio.word()),
                    _ => image.version.clone(),
                };
                t!("dev.head_firmware", version = version, kind = word).to_string()
            }
            None => String::new(),
        };
        draw_header_as(frame, area, &t!("dev.title"), &right);
    }
    let width = area.width.saturating_sub(4).min(CARD_W);
    let x = area.x + (area.width - width) / 2;
    let top = area.y + if hosted { 1 } else { 2 };
    // The first row the card may not reach: the busy line's (and on its
    // own, the tips' under it).
    let floor = if hosted { area.bottom() - 1 } else { area.bottom().saturating_sub(2) };
    // A gate or the sheet makes the card beneath inert (the kit's modal
    // rule): drawn with no pointer, its clicks and tips dropped before the
    // modal draws.
    let modal = page.modal_open();
    let pointer = if modal { page.ui.pointer.take() } else { None };
    let mut y = top;
    if page.boards.len() + page.card_count() >= 2 && y < floor {
        tab_row(frame, page, Rect { x, y, width, height: 1 });
        y += 2;
    }
    if y < floor {
        let room = Rect { x, y, width, height: floor - y };
        match (page.open_card(), page.shown().cloned()) {
            (Some(view), _) => card_view(frame, page, &view, room),
            (None, Some(board)) => card(frame, page, &board, room),
            (None, None) => empty_card(frame, page, room),
        }
    }
    let busy = page.busy();
    let note = page.note.as_ref().map(|n| (n.text.clone(), n.gold));
    let tips = page.hint_line(hosted, usize::from(area.width.saturating_sub(4)));
    draw_foot(frame, area, note.as_ref(), busy.as_deref(), &tips, hosted);
    if modal {
        page.ui.clear_registries();
        page.ui.pointer = pointer;
        if page.gate.is_some() {
            draw_gate(frame, page, area);
        } else if page.sheet.is_some() {
            sheet::draw_sheet(frame, page, area);
        } else if page.card.gate.is_some() {
            card::draw_card_gate(frame, page, area);
        } else {
            card::draw_card_sheet(frame, page, area);
        }
    }
    if let Some((target, text)) = page.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// What the GUI's MP3 Player tab draws in `area` while the page it was
/// left with lets its ports go and the next waits to be built (contract
/// clause 5): `words`, dim, on the page's first row under the host's
/// blank one.
pub(crate) fn draw_waiting(frame: &mut Frame, area: Rect, words: &str) {
    if area.width == 0 || area.height < 3 {
        return;
    }
    let width = area.width.saturating_sub(4).min(CARD_W);
    let x = area.x + (area.width - width) / 2;
    let rect = Rect { x, y: area.y + 1, width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(words.to_string(), dim())), rect);
}

// ── The port tabs ───────────────────────────────────────────────────────────

/// A board's mark on its tab: ✓ green, ! and ✗ gold, ? and … dim, a
/// write's percentage in the accent, `in use` dim.
fn mark(page: &Page, board: &Board) -> (String, Color) {
    let g = glyphs();
    if let Work::Writing { pct, .. } = board.work {
        return (pct.map_or_else(|| g.busy.to_string(), |p| format!("{p}%")), th().accent);
    }
    if page.written_now(board) {
        return (g.busy.to_string(), th().accent);
    }
    // Being asked, or starting up and asked again: the verdict is coming.
    let coming = matches!(board.verdict, Verdict::Asking | Verdict::Starting);
    if matches!(board.work, Work::Queued | Work::Reading) || coming {
        return (g.busy.to_string(), th().dim);
    }
    if matches!(board.written, Some(Written::Failed { .. })) {
        return (g.no.to_string(), th().gold);
    }
    match board.verdict {
        Verdict::UpToDate | Verdict::Newer => (g.ok.to_string(), th().ok),
        Verdict::Update | Verdict::DevUpdate => (g.warn.to_string(), th().gold),
        Verdict::InUse => (t!("dev.tab_in_use").to_string(), th().dim),
        Verdict::Other { .. }
        | Verdict::Blank
        | Verdict::NotCore2 { .. }
        | Verdict::HalfWritten
        | Verdict::Unreadable => (g.no.to_string(), th().gold),
        Verdict::Silent | Verdict::Unplaced | Verdict::Asking | Verdict::Starting => {
            (g.unknown.to_string(), th().dim)
        }
    }
}

/// The kit's tab row of subjects (contract clause 25): the boards, then the
/// cards in this computer's readers; the open one the slab, the others dim
/// text that brightens under the pointer, each wearing its mark; the
/// count, or Update all, at the right edge. Short of room, the cards'
/// names give way first, down to six cells; then the right edge's text;
/// then the last tabs.
fn tab_row(frame: &mut Frame, page: &mut Page, row: Rect) {
    let mut x = row.x;
    let boards = page.boards.clone();
    let cards = page.cards();
    let open_card = page.card_open.clone();
    // Each tab's name, its mark and its act, in order.
    let mut tabs: Vec<(String, (String, Color), bool, Act)> = Vec::new();
    for (i, board) in boards.iter().enumerate() {
        let open = open_card.is_none() && page.open.as_deref() == Some(board.port());
        tabs.push((short(board.port()), mark(page, board), open, Act::Board(i)));
    }
    let (right_text, _, _) = tabs_right(page);
    let fixed: usize = tabs.iter().map(|(n, (m, _), ..)| kit::width(n) + kit::width(m) + 3).sum();
    let names = |room: usize| -> Vec<String> { cards.iter().map(|c| card::tab_name(c, room)).collect() };
    let width_of = |names: &[String]| -> usize {
        let marks: usize = cards.iter().map(|c| kit::width(&card::mark(c).0) + 3).sum();
        fixed + marks + names.iter().map(|n| kit::width(n)).sum::<usize>() + tabs.len() + cards.len()
    };
    let room_all = usize::from(row.width).saturating_sub(kit::width(&right_text) + 2);
    let mut room = 64;
    let mut card_names = names(room);
    while width_of(&card_names) > room_all && room > 6 {
        room -= 1;
        card_names = names(room);
    }
    for (i, (card, name)) in cards.iter().zip(card_names).enumerate() {
        tabs.push((name, card::mark(card), open_card.as_deref() == Some(card.key.as_str()), Act::Card(i)));
    }
    for (name, (mark, color), open, act) in tabs {
        let label = format!(" {name} {mark} ");
        let cells = kit::width(&label) as u16;
        if x + cells > row.right() {
            break;
        }
        let rect = Rect { x, y: row.y, width: cells, height: 1 };
        let line = if open {
            let slab = Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD);
            Line::from(Span::styled(label, slab))
        } else {
            let name_style = if page.ui.hovers(rect) { hover() } else { dim() };
            Line::from(vec![
                Span::styled(format!(" {name} "), name_style),
                Span::styled(mark, Style::default().fg(color)),
                Span::raw(" "),
            ])
        };
        frame.render_widget(Paragraph::new(line), rect);
        page.ui.click(rect, act);
        x = rect.right() + 1;
    }
    let (text, style, act) = tabs_right(page);
    let cells = kit::width(&text) as u16;
    if text.is_empty() || x + 2 + cells > row.right() {
        return;
    }
    let rect = Rect { x: row.right() - cells, y: row.y, width: cells, height: 1 };
    let style = match &act {
        Some(_) if page.ui.hovers(rect) => hover(),
        _ => style,
    };
    frame.render_widget(Paragraph::new(Span::styled(text, style)), rect);
    if let Some(act) = act {
        page.ui.click(rect, act);
    }
}

/// The tab row's right edge: Update all as it goes, the board being
/// written, Update all to offer, C's count of the boards that need an
/// update, or what is plugged in.
fn tabs_right(page: &Page) -> (String, Style, Option<Act>) {
    match &page.lock {
        Some(Lock::All { ports, at, .. }) => {
            let text = t!("dev.tabs_all_at", i = (at + 1).min(ports.len()), n = ports.len());
            return (text.to_string(), dim(), None);
        }
        Some(Lock::One { port, .. }) => {
            return (t!("dev.tabs_writing", port = short(port)).to_string(), accent(), None);
        }
        None => {}
    }
    // A sync, then a card being read (clause 25).
    let cards = page.cards();
    if let Some(card) = cards.iter().find(|c| c.syncing()) {
        return (t!("dev.card.tabs_syncing", card = super::card::card_name(card)).to_string(), accent(), None);
    }
    if let Some(card) = cards.iter().find(|c| c.read == crate::device::card::worker::Read::Reading) {
        return (t!("dev.card.tabs_reading", drive = card.drive()).to_string(), dim(), None);
    }
    let card_words = match cards.len() {
        0 => None,
        1 => Some(t!("dev.card.tabs_card_one").to_string()),
        n => Some(t!("dev.card.tabs_cards", n = n).to_string()),
    };
    let with_cards = |words: String| match &card_words {
        Some(cards) if words.is_empty() => cards.clone(),
        Some(cards) => format!("{words} · {cards}"),
        None => words,
    };
    let needing = page.needing().len();
    if page.can_update_all() {
        // It counts only the boards it writes (clause 42).
        let n = page.writing_all().len();
        return (t!("dev.tabs_all", n = n).to_string(), dim(), Some(Act::UpdateAll));
    }
    let n = page.boards.len();
    let gold = Style::default().fg(th().gold);
    match needing {
        1 => return (t!("dev.tabs_needs_one", n = n).to_string(), gold, None),
        // Two or more, kept out of Update all by their own next writes.
        k if k >= 2 => return (t!("dev.tabs_needs", k = k, n = n).to_string(), gold, None),
        _ => {}
    }
    // Each board in one place: in use, being asked, a player (it answered
    // as mStream firmware), or another board.
    let (mut players, mut others, mut in_use) = (0, 0, 0);
    let mut asking: Vec<&Board> = Vec::new();
    for board in &page.boards {
        match board.verdict {
            Verdict::InUse => in_use += 1,
            Verdict::Asking => asking.push(board),
            _ if board.ours() => players += 1,
            _ => others += 1,
        }
    }
    if players == n && card_words.is_none() {
        return (t!("dev.tabs_plugged", n = n).to_string(), dim(), None);
    }
    let mut parts = Vec::new();
    if players > 0 {
        let words = if players == 1 { t!("dev.tabs_player_one") } else { t!("dev.tabs_players", n = players) };
        parts.push(words.to_string());
    }
    if others > 0 {
        let words = if others == 1 { t!("dev.tabs_other_one") } else { t!("dev.tabs_others", n = others) };
        parts.push(words.to_string());
    }
    if in_use > 0 {
        parts.push(t!("dev.tabs_in_use", n = in_use).to_string());
    }
    if let Some(board) = asking.first() {
        parts.push(t!("dev.tabs_asking", port = short(board.port())).to_string());
    }
    (with_cards(parts.join(" · ")), dim(), None)
}

// ── The card ────────────────────────────────────────────────────────────────

/// One row of the card, laid out before it is drawn so the card knows its
/// height: every row is one cell high but the actions (three with a
/// primary) and the log (whatever is left).
pub(super) enum Row {
    Blank,
    /// Across the content: a line at the left and one at the right, the
    /// right dropped where it would not fit.
    Wide { left: Line<'static>, right: Option<Line<'static>> },
    /// The label column (on a row's first line), then the value column.
    Value { label: Option<String>, left: Line<'static>, right: Option<Line<'static>> },
    /// A █░ bar across the value column: `permille` filled, in `color`.
    Bar { label: Option<String>, permille: u32, color: Color },
    /// A dim text button, in the value column or across the content, with
    /// a dim note after it.
    Link { wide: bool, text: String, act: Act, note: Option<String> },
    /// A line of the Next write in the value column, the first with Reset
    /// at the column's right edge (clause 38).
    Next { line: Line<'static>, reset: bool },
    /// The bottom row: text buttons at the left, the primary at the right
    /// (enabled, or the kit's disabled frame with why in a tooltip). Under
    /// a gate or a sheet (`covered`) the primary keeps its rows and is not
    /// drawn: the card beneath keeps its height, and a modal shorter than
    /// the card shows no half a button.
    Actions { links: Vec<(String, Act)>, primary: Option<(String, bool, Option<String>)>, covered: bool },
    /// Details' dim rule.
    Rule,
    /// A Details fact: its label, its value cut to the column.
    Fact { label: String, value: String, style: Style },
    /// The log: a blank row and the newest lines, in the rows left.
    Log,
}

impl Row {
    /// The rows it takes; the log's are the card's to give.
    fn height(&self) -> u16 {
        match self {
            Row::Actions { primary: Some(_), .. } => BUTTON_H,
            Row::Log => 0,
            _ => 1,
        }
    }
}

/// `pieces` word-wrapped in `width` cells, each word in its piece's style;
/// a word wider than the line (a Japanese or Chinese sentence, which has
/// no spaces) breaks at the cell. Pieces with no space between them make
/// one word.
pub(super) fn wrap_spans(pieces: &[(String, Style)], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    // Words, each a run of (text, style).
    let mut words: Vec<Vec<(String, Style)>> = Vec::new();
    let mut in_word = false;
    for (text, style) in pieces {
        for c in text.chars() {
            if c == ' ' {
                in_word = false;
                continue;
            }
            if !in_word {
                words.push(Vec::new());
                in_word = true;
            }
            let runs = words.last_mut().expect("a word");
            match runs.last_mut() {
                Some((run, s)) if s == style => run.push(c),
                _ => runs.push((c.to_string(), *style)),
            }
        }
    }
    let mut lines: Vec<Vec<(String, Style)>> = vec![Vec::new()];
    let mut used = 0usize;
    for runs in words {
        let cells: usize = runs.iter().map(|(t, _)| kit::width(t)).sum();
        // Japanese and Chinese break between any two characters: a word of
        // theirs fills the line it starts on rather than leave it short.
        let wide = runs.iter().any(|(t, _)| t.chars().any(|c| kit::width(c.encode_utf8(&mut [0u8; 4])) > 1));
        if used > 0 && used + 1 + cells > width && !(wide && used + 3 < width) {
            lines.push(Vec::new());
            used = 0;
        } else if used > 0 {
            lines.last_mut().expect("a line").push((" ".to_string(), Style::default()));
            used += 1;
        }
        for (text, style) in runs {
            for c in text.chars() {
                let w = kit::width(c.encode_utf8(&mut [0u8; 4]));
                if used + w > width && used > 0 {
                    lines.push(Vec::new());
                    used = 0;
                }
                let line = lines.last_mut().expect("a line");
                match line.last_mut() {
                    Some((run, s)) if *s == style => run.push(c),
                    _ => line.push((c.to_string(), style)),
                }
                used += w;
            }
        }
    }
    lines
        .into_iter()
        .filter(|l| !l.is_empty())
        .map(|l| Line::from(l.into_iter().map(|(t, s)| Span::styled(t, s)).collect::<Vec<_>>()))
        .collect()
}

/// Value rows for `pieces` wrapped in the value column, the label on the
/// first.
fn value_rows(label: Option<String>, pieces: &[(String, Style)], width: u16) -> Vec<Row> {
    wrap_spans(pieces, usize::from(width))
        .into_iter()
        .enumerate()
        .map(|(i, line)| Row::Value { label: if i == 0 { label.clone() } else { None }, left: line, right: None })
        .collect()
}

/// Dim value rows, under a label or not.
fn dim_rows(label: Option<String>, text: &str, width: u16) -> Vec<Row> {
    value_rows(label, &[(text.to_string(), dim())], width)
}

/// Wide rows for `pieces` across the content.
fn wide_rows(pieces: &[(String, Style)], width: u16) -> Vec<Row> {
    wrap_spans(pieces, usize::from(width)).into_iter().map(|left| Row::Wide { left, right: None }).collect()
}

/// The chip: the glyph and the verdict's first words in its colour, the
/// words bold, and the facts after ` · ` plain.
fn chip(glyph: &str, color: Color, words: &str, facts: &[String]) -> Vec<(String, Style)> {
    let tone = Style::default().fg(color);
    let mut pieces = vec![(format!("{glyph} "), tone), (words.to_string(), tone.add_modifier(Modifier::BOLD))];
    for fact in facts {
        pieces.push((format!(" · {fact}"), Style::default()));
    }
    pieces
}

/// The kit's scan widget on one line: ten cells (▰ green filled to the
/// percent, ▱ dim; all dim with no estimate), the percent dim, then the
/// present-progressive words in the accent.
fn scan(pct: Option<u8>, words: &str) -> Vec<(String, Style)> {
    let g = glyphs();
    let filled = pct.map_or(0, |p| usize::from(p.min(100)) / 10);
    let mut pieces = Vec::new();
    if filled > 0 {
        pieces.push((g.filled.repeat(filled), Style::default().fg(th().ok)));
    }
    let mut empty = g.empty.repeat(10 - filled);
    if let Some(p) = pct {
        empty.push_str(&format!(" {p}%"));
    }
    pieces.push((empty, dim()));
    pieces.push((format!(" {words}"), accent()));
    pieces
}

/// The board's name on the card's head (contract clause 18).
fn board_name(board: &Board) -> String {
    if board.ours() {
        return t!("dev.name_core2").to_string();
    }
    match (&board.verdict, &board.probe) {
        (Verdict::NotCore2 { .. }, Some(Err(DeviceError::WrongFlash { .. }))) => t!("dev.name_esp32").to_string(),
        (Verdict::NotCore2 { .. }, _) | (Verdict::Silent, _) => t!("dev.name_unknown").to_string(),
        (Verdict::InUse | Verdict::Unreadable, _) => t!("dev.name_on_port", port = short(board.port())).to_string(),
        _ => t!("dev.name_core2").to_string(),
    }
}

/// The head's right edge: the port with one board; with several (the port
/// is on the tab) the USB serial, the bridge before it for a board that
/// has not answered as ours.
fn head_right(page: &Page, board: &Board) -> String {
    if page.boards.len() < 2 {
        return board.port().to_string();
    }
    let bridge = board.candidate.bridge;
    match (board.serial(), board.ours()) {
        (Some(serial), true) => t!("dev.serial", serial = serial).to_string(),
        (Some(serial), false) => format!("{bridge} · {}", t!("dev.serial", serial = serial)),
        (None, _) => bridge.to_string(),
    }
}

/// The identity line with several boards: what a person can check on the
/// board itself — its battery, its headphones (contract clause 18).
fn identity(page: &Page, board: &Board) -> Option<String> {
    if page.boards.len() < 2 {
        return None;
    }
    let status = board.status()?;
    let mut parts = Vec::new();
    if let Some(bat) = status.bat {
        parts.push(t!("dev.id_battery", pct = bat).to_string());
    }
    parts.push(match &status.bt {
        Some(name) => t!("dev.id_paired", name = crate::admin::printable(name, 40)).to_string(),
        None => t!("dev.id_unpaired").to_string(),
    });
    Some(parts.join(" · "))
}

/// The Firmware row (contract clause 19): the chip and what follows from
/// it, the write itself while it runs, or the board being asked or read.
fn firmware_rows(page: &Page, board: &Board, w: u16) -> Vec<Row> {
    let g = glyphs();
    let label = Some(t!("dev.label_firmware").to_string());
    let gold = th().gold;
    let mut rows = Vec::new();
    let to = page.target.as_ref().map(|t| t.version.clone());
    let pin = to.clone().unwrap_or_else(|| "?".to_string());
    let version = board.version().unwrap_or("?").to_string();
    // The write under way, on this board: its words, then the bar.
    if page.written_now(board) {
        rows.extend(value_rows(label, &[(page.write_words(board), accent())], w));
        let permille = match board.work {
            Work::Writing { phase: Phase::Writing, pct } => u32::from(pct.unwrap_or(0).min(100)) * 10,
            Work::Writing { phase: Phase::Verifying | Phase::Restarting, .. } => 1000,
            _ => 0,
        };
        rows.push(Row::Bar { label: None, permille, color: th().accent });
        return rows;
    }
    if board.work == Work::Reading {
        rows.extend(value_rows(label, &scan(None, &t!("dev.fw_reading")), w));
        rows.extend(dim_rows(None, &t!("dev.fw_reading_2"), w));
        return rows;
    }
    let asking = board.verdict == Verdict::Asking || (to.is_none() && board.verdict == Verdict::Unplaced);
    if asking {
        rows.extend(value_rows(label, &scan(None, &t!("dev.fw_asking")), w));
        rows.extend(dim_rows(None, &t!("dev.fw_asking_2"), w));
        return rows;
    }
    let mut second: Vec<String> = Vec::new();
    let pieces = if let Some(Written::Failed { error, half, .. }) = &board.written {
        if *half {
            second.push(t!("dev.fw_half_2").to_string());
            chip(g.no, gold, &t!("dev.fw_failed"), &[t!("dev.fw_half").to_string()])
        } else {
            second.push(error.text());
            chip(g.no, gold, &t!("dev.fw_failed"), &[t!("dev.fw_not_written").to_string()])
        }
    } else if let (Some(looping), Some(Written::Done { image, .. })) = (board.looping(), &board.written) {
        // The restart loop (clause 40): what went on, what probably
        // happened, what fixes it.
        let (n, secs) = (looping.restarts, looping.secs);
        second.push(t!("dev.fw_looping_2", n = n, secs = secs).to_string());
        let mode = image.mode.map_or("?", Mode::word);
        let went_on = t!("dev.next_in", what = image.version, mode = mode).to_string();
        chip(g.no, gold, &t!("dev.fw_looping"), &[went_on])
    } else {
        match &board.verdict {
            Verdict::UpToDate => {
                let mut facts = vec![version.clone()];
                match &board.written {
                    Some(Written::Done { install: true, .. }) => facts.push(t!("dev.fw_just_installed").to_string()),
                    Some(Written::Done { .. }) => facts.push(t!("dev.fw_just_written").to_string()),
                    _ => {}
                }
                chip(g.ok, th().ok, &t!("dev.fw_up_to_date"), &facts)
            }
            Verdict::Update => chip(g.warn, gold, &t!("dev.fw_update"), &[format!("{version} {} {pin}", g.arrow)]),
            Verdict::DevUpdate => chip(g.warn, gold, &t!("dev.fw_dev"), &[format!("{version} {} {pin}", g.arrow)]),
            // Past the pin is nothing to do, so it wears the up-to-date chip;
            // the second line says why the version is not the pin's. A gold !
            // here read as a warning the next line took back (Paul, 10 Oct).
            Verdict::Newer => {
                second.push(t!("dev.fw_newer_2", pin = pin).to_string());
                chip(g.ok, th().ok, &t!("dev.fw_up_to_date"), std::slice::from_ref(&version))
            }
            Verdict::Unplaced if board.version().is_some() => {
                second.push(t!("dev.fw_unplaced_2", pin = pin).to_string());
                chip(g.unknown, th().dim, &t!("dev.fw_unplaced"), std::slice::from_ref(&version))
            }
            Verdict::Unplaced => {
                second.push(t!("dev.fw_unknown_version_2").to_string());
                chip(g.unknown, th().dim, &t!("dev.fw_unknown_version"), &[])
            }
            Verdict::Other { name } => chip(g.no, gold, &t!("dev.fw_other"), std::slice::from_ref(name)),
            Verdict::Blank => chip(g.no, gold, &t!("dev.fw_blank"), &[]),
            // Ours, busy listing its library: nothing to read or write yet,
            // and nothing that says it may not be an MP3 player.
            Verdict::Starting => {
                let mut facts: Vec<String> = board.version().map(str::to_string).into_iter().collect();
                facts.push(t!("dev.fw_starting_why").to_string());
                chip(g.unknown, th().dim, &t!("dev.fw_starting"), &facts)
            }
            Verdict::Silent => {
                second.push(t!("dev.fw_silent_2").to_string());
                second.push(t!("dev.fw_read_cost").to_string());
                chip(g.unknown, th().dim, &t!("dev.fw_silent"), &[t!("dev.fw_silent_maybe").to_string()])
            }
            Verdict::NotCore2 { found } => {
                second.push(t!("dev.fw_not_core2_2").to_string());
                let label = Some(t!("dev.board").to_string());
                let pieces = chip(g.no, gold, &t!("dev.fw_not_core2"), std::slice::from_ref(found));
                rows.extend(value_rows(label, &pieces, w));
                for line in &second {
                    rows.extend(dim_rows(None, line, w));
                }
                return rows;
            }
            Verdict::InUse => {
                second.push(t!("dev.fw_in_use_2").to_string());
                second.push(t!("dev.fw_in_use_3").to_string());
                chip(g.warn, gold, &t!("dev.fw_in_use"), &[])
            }
            Verdict::Unreadable => {
                let why = match (&board.heard, &board.probe) {
                    (Heard::Failed(e), _) | (_, Some(Err(e))) => e.text(),
                    _ => String::new(),
                };
                if !why.is_empty() {
                    second.push(why);
                }
                chip(g.no, gold, &t!("dev.fw_unreadable"), &[])
            }
            // Folded above: a half-written board carries its failed write.
            Verdict::HalfWritten => {
                second.push(t!("dev.fw_half_2").to_string());
                chip(g.no, gold, &t!("dev.fw_failed"), &[t!("dev.fw_half").to_string()])
            }
            Verdict::Asking => Vec::new(),
        }
    };
    rows.extend(value_rows(label, &pieces, w));
    for line in &second {
        rows.extend(dim_rows(None, line, w));
    }
    match board.pending() {
        Some(next) => rows.extend(next_rows(page, board, next, w)),
        // A board on DIO by its own choice says so once; QIO is quiet
        // (clause 37).
        None if board.looping().is_none()
            && board.default_mode() == Mode::Dio
            && !matches!(board.written, Some(Written::Failed { .. })) =>
        {
            rows.extend(dim_rows(None, &t!("dev.fw_in_dio"), w));
        }
        None => {}
    }
    // Look, don't write: a board whose primary would act waits its turn.
    if let Some(port) = page.writing_port()
        && port != board.port()
        && board.primary().is_some()
    {
        rows.extend(dim_rows(None, &t!("dev.fw_one_at_a_time", port = short(port)), w));
    }
    rows
}

/// The board's next write under its chip (clause 38): "Next write:" dim,
/// the choice in the accent — the selection's colour, not a warning — and
/// Reset at the value column's right edge; the image's percent while it is
/// fetched; where the flags set it, which; one that cannot be had, why.
fn next_rows(page: &Page, board: &Board, next: &Next, w: u16) -> Vec<Row> {
    let mut words = t!("dev.next_in", what = next_what(next), mode = mode_words(next.mode())).to_string();
    if next.erase {
        words.push_str(&t!("dev.next_erasing"));
    }
    let pieces = [(format!("{} ", t!("dev.next_write")), dim()), (words, accent())];
    let mut lines = wrap_spans(&pieces, usize::from(w));
    if let NextState::Getting { done, total } = &next.state {
        let mut getting = t!("dev.next_getting").to_string();
        if let Some(total) = total.filter(|t| *t > 0) {
            getting.push_str(&format!(" {}%", (done * 100 / total).min(100)));
        }
        lines.extend(wrap_spans(&[(getting, dim())], usize::from(w)));
    }
    if next.by == By::Flags
        && let Some(flags) = &page.flags
    {
        lines.extend(wrap_spans(&[(t!("dev.next_set_by", flags = flags).to_string(), dim())], usize::from(w)));
    }
    // Reset at the value column's right edge, on the first of the lines
    // with room for it, else on a row of its own.
    let reset = page.offers_reset(board);
    let cells = kit::width(&t!("dev.link_reset")) as u16 + 2;
    let at = lines.iter().position(|line| line.width() as u16 + cells <= w);
    let mut rows: Vec<Row> =
        lines.into_iter().enumerate().map(|(i, line)| Row::Next { line, reset: reset && at == Some(i) }).collect();
    if reset && at.is_none() {
        rows.push(Row::Next { line: Line::default(), reset: true });
    }
    if let NextState::Failed(e) = &next.state {
        rows.extend(value_rows(None, &[(e.text(), Style::default().fg(th().gold))], w));
    }
    rows
}

/// `1,284 tracks`, `1 track`, `no tracks`, or being indexed.
fn tracks_words(tracks: &Tracks) -> Option<String> {
    Some(match tracks {
        Tracks::Count(0) => t!("dev.card_no_tracks").to_string(),
        Tracks::Count(1) => t!("dev.card_track_one").to_string(),
        Tracks::Count(n) => t!("dev.card_tracks", n = fmt_count(*n)).to_string(),
        Tracks::Building => t!("dev.card_tracks_building").to_string(),
        Tracks::NoCard | Tracks::Unknown => return None,
    })
}

/// The SD card row (contract clause 20): only what the running firmware
/// just reported.
fn card_rows(page: &Page, board: &Board, w: u16) -> Vec<Row> {
    let g = glyphs();
    let label = Some(t!("dev.sd_card").to_string());
    let gold = Style::default().fg(th().gold);
    let problem = |words: String, why: Option<String>, fix: String| -> Vec<Row> {
        let mut pieces = vec![(format!("{} ", g.warn), gold), (words, gold.add_modifier(Modifier::BOLD))];
        if let Some(why) = why {
            pieces.push((format!(" · {why}"), gold));
        }
        let mut rows = value_rows(label.clone(), &pieces, w);
        rows.extend(dim_rows(None, &fix, w));
        rows
    };
    let unknown = |words: String| -> Vec<Row> { dim_rows(label.clone(), &format!("{} {words}", g.unknown), w) };
    // The format under way is the row (clause 59): the question on the
    // player's screen, then its restart and its percent.
    match board.format {
        Format::Asking => return value_rows(label, &scan(None, &t!("dev.fw_asking")), w),
        Format::Waiting => {
            let mut rows = value_rows(label, &scan(None, &t!("dev.card.fmt_waiting")), w);
            rows.extend(dim_rows(None, &t!("dev.card.fmt_waiting_2"), w));
            return rows;
        }
        Format::Restarting | Format::Running { .. } => {
            let pct = match board.format {
                Format::Running { pct } => Some(pct),
                _ => None,
            };
            let mut rows = value_rows(label, &scan(pct, &t!("dev.card.fmt_running")), w);
            let left = super::card::span(page.format_left(board));
            rows.extend(dim_rows(None, &t!("dev.card.fmt_left", t = left), w));
            return rows;
        }
        _ => {}
    }
    // Firmware that can't format, said where the format would be offered:
    // "Updating lets it." only while the pin can (firmware.rs,
    // PINNED_ANSWERS_FORMAT), as "Updating shows it." above.
    let cant_format = || {
        let mut words = t!("dev.card.fmt_old", version = board.version().unwrap_or("?")).to_string();
        if page.target.as_ref().is_some_and(|t| t.answers_format) {
            words.push(' ');
            words.push_str(&t!("dev.card.fmt_old_update"));
        }
        words
    };
    // Nothing can read the card while its board is written.
    let card = if page.written_now(board) { Card::Unknown(CardUnknown::Writing) } else { board.card() };
    match card {
        Card::Unknown(why) => match why {
            CardUnknown::Asking => dim_rows(label, &t!("dev.card_asking"), w),
            CardUnknown::Writing => dim_rows(label, &t!("dev.card_untouched"), w),
            CardUnknown::HalfWritten | CardUnknown::NotRunning => unknown(t!("dev.card_unknown_runs").to_string()),
            CardUnknown::Starting => unknown(t!("dev.card_starting").to_string()),
            CardUnknown::OldFirmware => {
                let mut words = match board.version() {
                    Some(version) => t!("dev.card_old", version = version).to_string(),
                    None => t!("dev.card_old_this").to_string(),
                };
                // Only a pin that answers the status query can keep this
                // promise (firmware.rs, PINNED_ANSWERS_STATUS): v0.9.0 and
                // later answer it — the pin does from v0.10.0 on; v0.8.0 did
                // not. A board this old is behind any pin that answers, so
                // its card has Update ▸ beside the promise.
                if page.target.as_ref().is_some_and(|t| t.answers_status) {
                    words.push(' ');
                    words.push_str(&t!("dev.card_old_update"));
                }
                unknown(words)
            }
            CardUnknown::NotOurs => unknown(t!("dev.card_not_ours").to_string()),
            CardUnknown::InUse => unknown(t!("dev.card_in_use").to_string()),
            CardUnknown::Unreadable => unknown(t!("dev.card_unreadable_board").to_string()),
        },
        Card::Empty => problem(t!("dev.card_none").to_string(), None, t!("dev.card_put_in").to_string()),
        // A card the player can't read (clauses 20, 59, 60). On a board that
        // can format it, the fix is the player's: Format as FAT32 ▸ when
        // nothing else is to be done, else its dim link here; a damaged card
        // goes to a computer's disk check first, the format its dim second
        // step. On one that can't, today's words, on a computer (the `_pc`
        // keys), and that it can't — or, while the pin can, that updating
        // lets it, in the fix line's place.
        Card::Foreign { kind, .. } => {
            let damaged = !matches!(kind, CardKind::ExFat | CardKind::Ntfs | CardKind::Gpt);
            let (words, why) = match kind {
                CardKind::ExFat | CardKind::Ntfs => {
                    let fs = if kind == CardKind::ExFat { "dev.card_exfat" } else { "dev.card_ntfs" };
                    (t!(fs), Some(t!("dev.card_fat32_only")))
                }
                CardKind::Gpt => (t!("dev.card_gpt"), Some(t!("dev.card_mbr_only"))),
                _ => (t!("dev.card_unreadable"), None),
            };
            let (gpt, offered) = (kind == CardKind::Gpt, board.format_offer().is_some());
            let pin_formats = page.target.as_ref().is_some_and(|t| t.answers_format);
            let fix = match (damaged, offered, gpt) {
                (true, true, _) => t!("dev.card.board_unreadable_2").to_string(),
                (true, false, _) => t!("dev.card_try_computer").to_string(),
                (false, true, false) => t!("dev.card_fix_format").to_string(),
                (false, true, true) => t!("dev.card_fix_mbr").to_string(),
                (false, false, _) if pin_formats && !board.can_format() => cant_format(),
                (false, false, false) => t!("dev.card_fix_format_pc").to_string(),
                (false, false, true) => t!("dev.card_fix_mbr_pc").to_string(),
            };
            let mut rows = problem(words.to_string(), why.map(|w| w.to_string()), fix);
            if !board.can_format() && (damaged || !pin_formats) {
                rows.extend(dim_rows(None, &cant_format(), w));
            }
            if page.can_format(board) && page.primary_of(board) != Some(Primary::Format) {
                let (text, note) = if damaged {
                    let note = t!("dev.card.link_format_anyway_note").to_string();
                    (t!("dev.card.link_format_anyway"), Some(note))
                } else {
                    (t!("dev.card.link_format"), None)
                };
                rows.push(Row::Link { wide: false, text: text.to_string(), act: Act::Format, note });
            }
            rows
        }
        Card::Fat { size, free, tracks, .. } => {
            let tracks_text = tracks_words(&tracks);
            let size_text = size.map(|s| t!("dev.card_size", size = gb(s)).to_string());
            let mut rows = Vec::new();
            match (&free, size) {
                (Free::Bytes(free), Some(size)) => {
                    let card = board.card();
                    let full = card.nearly_full();
                    let used = card.used().unwrap_or_else(|| size.saturating_sub(*free));
                    let mut permille = (u128::from(used) * 1000 / u128::from(size.max(1))) as u32;
                    // A sliver used still shows a cell; a card not quite full
                    // never shows a full bar.
                    if used > 0 {
                        permille = permille.max(1);
                    }
                    let color = if full { th().gold } else { th().accent };
                    rows.push(Row::Bar { label: label.clone(), permille, color });
                    let mut left = t!("dev.card_used", used = gb(used), size = gb(size)).to_string();
                    if let Some(tracks) = &tracks_text {
                        left.push_str(&format!(" · {tracks}"));
                    }
                    let free_text = t!("dev.card_free", free = gb(*free)).to_string();
                    let right = if full {
                        Line::from(Span::styled(format!("{} {free_text}", g.warn), Style::default().fg(th().gold)))
                    } else {
                        Line::from(Span::raw(free_text))
                    };
                    rows.push(Row::Value { label: None, left: Line::from(left), right: Some(right) });
                }
                (Free::Bytes(free), None) => {
                    let mut left = t!("dev.card_free", free = gb(*free)).to_string();
                    if let Some(tracks) = &tracks_text {
                        left.push_str(&format!(" · {tracks}"));
                    }
                    rows.extend(value_rows(label.clone(), &[(left, Style::default())], w));
                }
                (Free::NotCounted, _) => {
                    let mut words: Vec<String> = size_text.iter().cloned().collect();
                    words.extend(tracks_text.clone());
                    words.push(t!("dev.card_not_counted").to_string());
                    rows.extend(value_rows(label.clone(), &[(words.join(" · "), Style::default())], w));
                    if let Count::Refused { why, .. } = &board.count {
                        let (text, style) = match why {
                            CountWhy::Playing => (t!("dev.count_playing"), gold),
                            CountWhy::Library => (t!("dev.count_library"), dim()),
                            _ => (t!("dev.count_failed"), dim()),
                        };
                        rows.extend(value_rows(None, &[(text.to_string(), style)], w));
                    }
                    if page.can_count(board) {
                        let note = Some(t!("dev.link_count_note").to_string());
                        let text = t!("dev.link_count").to_string();
                        rows.push(Row::Link { wide: false, text, act: Act::Count, note });
                    }
                }
                (Free::Counting { pct }, _) => {
                    rows.extend(value_rows(label.clone(), &scan(*pct, &t!("dev.card_counting")), w));
                    let mut words: Vec<String> = size_text.iter().cloned().collect();
                    words.extend(tracks_text.clone());
                    if !words.is_empty() {
                        rows.extend(value_rows(None, &[(words.join(" · "), Style::default())], w));
                    }
                }
            }
            // The card it holds, by its `cid=` (clause 20a): its line gives
            // way to nothing but the floor. A card not filled yet says so in
            // place of where the music goes — one the player just formatted,
            // that it was (clause 59).
            let fresh = matches!(board.format, Format::Done(_))
                && matches!(tracks, Tracks::Count(0))
                && board.status().is_some_and(|s| !matches!(s.cid, CardRoot::Loaded(_)));
            let named =
                if fresh { vec![t!("dev.card.fmt_done").to_string()] } else { card::board_lines(page, board) };
            if matches!(tracks, Tracks::Count(0)) && named.is_empty() {
                rows.extend(dim_rows(None, &t!("dev.card_music_hint"), w));
            }
            for line in &named {
                rows.extend(dim_rows(None, line, w));
            }
            rows
        }
    }
}

/// Details (contract clause 18): the facts, the log, Write again.
fn details_rows(page: &Page, board: &Board) -> Vec<Row> {
    let mut rows = vec![Row::Rule];
    let fact = |label: &str, value: String| {
        Row::Fact { label: t!(label).to_string(), value, style: Style::default() }
    };
    let mut port = format!("{} · {}", board.port(), board.candidate.bridge);
    if let Some(serial) = board.serial() {
        port.push_str(&format!(" · {}", t!("dev.serial", serial = serial)));
    }
    let listened =
        matches!(board.heard, Heard::Status(_) | Heard::Old { .. } | Heard::Starting { .. } | Heard::Silent);
    match &board.probe {
        Some(Ok(probe)) => port.push_str(&format!(" · {} baud", probe.info.baud)),
        _ if listened => {
            port.push_str(&format!(" · {} baud · {}", engine::CONSOLE_BAUD, t!("dev.no_reset")));
        }
        _ => {}
    }
    rows.push(fact("dev.port", port));
    // A failed write: the log's last lines are what Details opens on.
    if !matches!(board.written, Some(Written::Failed { .. })) {
        let name = board_name(board);
        let flash = match &board.probe {
            Some(Ok(probe)) => {
                let rev = probe.info.revision.map(|(a, b)| format!("{a}.{b}")).unwrap_or_else(|| "?".to_string());
                let flash = probe.info.flash_mb.or(board.flash_mb);
                let flash = flash.map_or_else(|| "?".to_string(), |mb| mb.to_string());
                let chip = format!("{} rev {rev}", probe.info.chip);
                Some(format!("{name} · {chip} · {}", t!("dev.fact_flash", flash = flash)))
            }
            _ => board.flash_mb.map(|mb| format!("{name} · {}", t!("dev.fact_flash", flash = mb))),
        };
        rows.push(fact("dev.board", flash.unwrap_or(name)));
        let on_board = match (&board.verdict, board.version()) {
            (Verdict::Other { name }, _) => t!("dev.on_board_other", name = name).to_string(),
            (Verdict::Blank, _) => t!("dev.on_board_unknown").to_string(),
            (_, Some(version)) => {
                let mut text = t!("dev.on_board_ours", version = version).to_string();
                // Its mode where its ELF tells it, never a guess (clause 41).
                if let Some(elf) = board.elf().filter(|e| !e.is_empty()) {
                    let mode = board.mode().map(|m| m.mode.word().to_string());
                    let mode = mode.unwrap_or_else(|| t!("dev.mode_unknown").to_string());
                    text.push_str(&format!(" · {mode} · ELF {elf}"));
                }
                text
            }
            (_, None) => t!("dev.fact_unknown").to_string(),
        };
        rows.push(fact("dev.on_board", on_board));
        let carries = t!("dev.fact_carries", version = page.target.as_ref().map_or("?", |t| t.version.as_str()));
        let (player, style) = match (&page.image, &page.image_failed) {
            (Some(image), _) => (format!("{carries} · {}", image.origin), Style::default()),
            (None, Some(why)) => (why.clone(), Style::default().fg(th().gold)),
            (None, None) => {
                let mut getting = format!("{carries} · {}", t!("dev.fact_getting"));
                if let Some((done, Some(total))) = page.download.filter(|(_, t)| t.is_some_and(|t| t > 0)) {
                    getting.push_str(&format!(" {}%", (done * 100 / total).min(100)));
                }
                (getting, Style::default())
            }
        };
        rows.push(Row::Fact { label: t!("dev.label_player").to_string(), value: player, style });
        // What the last write this visit put on, read-only (clause 41).
        if let Some(Written::Done { image, .. }) = &board.written {
            let mode = image.mode.map_or_else(|| t!("dev.mode_unknown").to_string(), |m| m.word().into());
            let (version, tag) = (&image.version, image.image.tag().unwrap_or("?"));
            let value = match image.check {
                Check::Pinned => t!("dev.fact_written_pin", version = version, mode = mode),
                Check::Sums => t!("dev.fact_written_sums", version = version, mode = mode, tag = tag),
                Check::Description => t!("dev.fact_written_local", version = version, mode = mode),
            };
            rows.push(fact("dev.label_written", value.to_string()));
        }
        if let Some(status) = board.status() {
            let kind = match &status.card {
                CardKind::None => None,
                CardKind::Fat32 => Some("FAT32".to_string()),
                CardKind::Fat16 => Some("FAT16".to_string()),
                CardKind::ExFat => Some("exFAT".to_string()),
                CardKind::Ntfs => Some("NTFS".to_string()),
                CardKind::Gpt => Some("GPT".to_string()),
                CardKind::Other | CardKind::Unreadable => Some(t!("dev.card_unreadable").to_string()),
                CardKind::Unknown(word) => Some(word.clone()),
            };
            let mut card = match kind {
                Some(kind) => vec![kind],
                None => vec![t!("dev.card_none").to_string()],
            };
            if let Some(size) = status.size {
                card.push(t!("dev.fact_bytes", n = fmt_count(size)).to_string());
            }
            match (&board.count, &status.free) {
                (Count::Done { .. }, _) => card.push(t!("dev.fact_free_counted").to_string()),
                (_, Free::Bytes(_)) => card.push(t!("dev.fact_free_fsinfo").to_string()),
                (_, Free::NotCounted) if status.card.readable() => card.push(t!("dev.card_not_counted").to_string()),
                _ => {}
            }
            rows.push(fact("dev.label_card_fact", card.join(" · ")));
        }
        // The clock as a listen of this visit set it (clauses 5 and 61).
        if let Some(set) = &board.clock_set
            && let Some(ms) = set.sent
        {
            let time = clock_at(ms / 1000);
            let value = match set.off_ms {
                None => t!("dev.card.board_clock_set", time = time),
                Some(off) => t!("dev.card.board_clock_off", time = time, s = desk::seconds_off(off)),
            };
            rows.push(fact("dev.card.board_clock", value.to_string()));
        }
    }
    rows.push(Row::Log);
    let again = page.can_write_again(board);
    if again {
        let to = page.target.as_ref().map_or("?", |t| t.version.as_str());
        rows.push(Row::Blank);
        let text = t!("dev.link_write", version = to).to_string();
        rows.push(Row::Link { wide: true, text, act: Act::WriteAgain, note: None });
    }
    // A card the player reads is offered the format here only (clause 59).
    if page.can_format(board) && board.format_offer() == Some(FormatOffer::Readable) {
        if !again {
            rows.push(Row::Blank);
        }
        let text = t!("dev.card.link_format").to_string();
        let note = Some(t!("dev.card.link_format_anyway_note").to_string());
        rows.push(Row::Link { wide: true, text, act: Act::Format, note });
    }
    rows
}

/// Every row of `board`'s card, top to bottom.
fn card_layout(page: &Page, board: &Board, content_w: u16) -> Vec<Row> {
    let value_w = content_w.saturating_sub(LABEL_W);
    let mut rows = vec![Row::Blank];
    let right = Line::from(Span::styled(head_right(page, board), dim()));
    rows.push(Row::Wide { left: Line::from(Span::styled(board_name(board), bold())), right: Some(right) });
    if let Some(line) = identity(page, board) {
        rows.extend(wide_rows(&[(line, dim())], content_w));
    }
    rows.push(Row::Blank);
    rows.extend(firmware_rows(page, board, value_w));
    if !matches!(board.verdict, Verdict::NotCore2 { .. }) {
        rows.push(Row::Blank);
        rows.extend(card_rows(page, board, value_w));
    }
    rows.push(Row::Blank);
    let g = glyphs();
    let details = format!("{} {}", if page.details { g.open } else { g.forward }, t!("dev.details"));
    let mut links = vec![(details, Act::Details)];
    if page.can_show(board) {
        links.push((t!("dev.link_show").to_string(), Act::Show));
    }
    if page.offers_ask(board) {
        links.push((t!("dev.link_ask").to_string(), Act::Ask));
    }
    if page.can_advance(board) {
        links.push((t!("dev.link_advanced").to_string(), Act::Advanced));
    }
    let primary = page.primary_of(board).map(|p| {
        let word = t!(match p {
            Primary::Update => "dev.btn_update",
            Primary::Install => "dev.btn_install",
            Primary::Read => "dev.btn_read",
            Primary::TryAgain => "dev.retry",
            Primary::Write => "dev.btn_write",
            Primary::Back => "dev.btn_back",
            Primary::Dio => "dev.btn_dio",
            Primary::Format => "dev.card.btn_format",
        });
        match page.writing_port() {
            // The kit's disabled frame: dim, no ▸, why in its tooltip.
            Some(port) => (word.to_string(), false, Some(t!("dev.fw_one_at_a_time", port = short(port)).to_string())),
            None => (format!("{word} {}", g.forward), true, None),
        }
    });
    rows.push(Row::Actions { links, primary, covered: false });
    if page.details {
        rows.extend(details_rows(page, board));
    }
    rows.push(Row::Blank);
    rows
}

/// The card in `room`: its rows laid out, its height what they need (the
/// log taking the rows left), cut to the room inside its border.
fn card(frame: &mut Frame, page: &mut Page, board: &Board, room: Rect) {
    if room.height < 3 || room.width < CARD_INSET * 2 + 2 {
        return;
    }
    let content_w = room.width - inset(room.width) * 2;
    let rows = card_layout(page, board, content_w);
    draw_card(frame, page, board.port(), &rows, room);
}

/// The card's inset at its width: the border and the padding.
pub(super) fn inset(width: u16) -> u16 {
    if width >= CARD_W { CARD_INSET } else { CARD_INSET - 1 }
}

/// The rows in a dim rounded card at the top of `room`.
fn draw_card(frame: &mut Frame, page: &mut Page, port: &str, rows: &[Row], room: Rect) {
    let content_w = room.width - inset(room.width) * 2;
    let fixed: u16 = rows.iter().map(Row::height).sum();
    let inside = room.height - 2;
    let lines = page.log_for(port).len() as u16;
    let has_log = rows.iter().any(|r| matches!(r, Row::Log));
    // The log takes what is left, under a blank row, and no more than it
    // has: the card is as tall as its rows.
    let spare = inside.saturating_sub(fixed);
    let log_rows = if has_log && spare >= 2 { (spare - 1).min(lines) } else { 0 };
    let height = (fixed + log_rows + u16::from(log_rows > 0)).min(inside);
    let rect = Rect { height: height + 2, ..room };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(dim());
    frame.render_widget(block, rect);
    let content = Rect { x: rect.x + inset(room.width), y: rect.y + 1, width: content_w, height };
    draw_rows(frame, page, port, rows, content, log_rows);
}

fn draw_rows(frame: &mut Frame, page: &mut Page, port: &str, rows: &[Row], content: Rect, log_rows: u16) {
    let label_w = LABEL_W.min(content.width);
    let value = |y: u16| Rect { x: content.x + label_w, y, width: content.width - label_w, height: 1 };
    let whole = |y: u16| Rect { x: content.x, y, width: content.width, height: 1 };
    let bottom = content.bottom();
    let mut y = content.y;
    for row in rows {
        if y >= bottom {
            break;
        }
        match row {
            Row::Blank => {}
            Row::Wide { left, right } => {
                frame.render_widget(Paragraph::new(left.clone()), whole(y));
                put_right(frame, whole(y), left, right.as_ref());
            }
            Row::Value { label, left, right } => {
                if let Some(label) = label {
                    let at = Rect { width: label_w, ..whole(y) };
                    frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), at);
                }
                frame.render_widget(Paragraph::new(left.clone()), value(y));
                put_right(frame, value(y), left, right.as_ref());
            }
            Row::Bar { label, permille, color } => {
                if let Some(label) = label {
                    let at = Rect { width: label_w, ..whole(y) };
                    frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), at);
                }
                bar(frame, value(y), *permille, *color);
            }
            Row::Link { wide, text, act, note } => {
                let at = if *wide { whole(y) } else { value(y) };
                let rect = text_button(frame, page, at, text, act.clone());
                if let Some(note) = note {
                    let x = rect.right() + 3;
                    if x < at.right() {
                        let rest = Rect { x, width: at.right() - x, ..at };
                        frame.render_widget(Paragraph::new(Span::styled(note.clone(), dim())), rest);
                    }
                }
            }
            Row::Next { line, reset } => {
                frame.render_widget(Paragraph::new(line.clone()), value(y));
                if *reset {
                    let words = t!("dev.link_reset").to_string();
                    let cells = (kit::width(&words) as u16).min(value(y).width);
                    let at = Rect { x: value(y).right() - cells, width: cells, ..value(y) };
                    text_button(frame, page, at, &words, Act::Reset);
                }
            }
            Row::Actions { links, primary, covered } => {
                let tall = primary.is_some();
                // Half a frame is not a button: short of rows, the primary
                // is not drawn and Enter still does what it would.
                let whole_button = tall && bottom - y >= BUTTON_H && !covered;
                let mid = if tall { y + 1 } else { y };
                // The links stop short of the primary: one that would run
                // under it is not drawn, its key still works.
                let primary_w = primary.as_ref().map_or(0, |(label, ..)| kit::tall_width(label) + 2);
                let links_end = content.right().saturating_sub(primary_w);
                if mid < bottom {
                    let mut x = content.x;
                    for (i, (text, act)) in links.iter().enumerate() {
                        let cells = kit::width(text) as u16;
                        if x >= links_end || (i > 0 && x + cells > links_end) {
                            break;
                        }
                        let at = Rect { x, y: mid, width: links_end.saturating_sub(x).max(1), height: 1 };
                        let rect = text_button(frame, page, at, text, act.clone());
                        x = rect.right() + 3;
                    }
                }
                if let Some((label, enabled, tip)) = primary
                    && whole_button
                {
                    let w = kit::tall_width(label).min(content.width);
                    let at = Rect { x: content.right() - w, y, width: w, height: BUTTON_H };
                    let rect = kit::tall_button(frame, &mut page.ui, at, label, *enabled, Act::Primary);
                    if let Some(tip) = tip {
                        page.ui.tip(rect, tip.clone());
                    }
                }
                y += if tall { BUTTON_H } else { 1 };
                continue;
            }
            Row::Rule => {
                let rule = glyphs().rule.repeat(usize::from(content.width));
                frame.render_widget(Paragraph::new(Span::styled(rule, dim())), whole(y));
            }
            Row::Fact { label, value: text, style } => {
                let at = Rect { width: label_w, ..whole(y) };
                frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), at);
                let cut = fit(text, usize::from(content.width - label_w));
                frame.render_widget(Paragraph::new(Span::styled(cut, *style)), value(y));
            }
            Row::Log => {
                if log_rows > 0 {
                    let rows = log_rows.min(bottom.saturating_sub(y + 1));
                    if rows > 0 {
                        let pane = Rect { x: content.x, y: y + 1, width: content.width, height: rows };
                        draw_log(frame, page, port, pane);
                    }
                    y += log_rows + 1;
                }
                continue;
            }
        }
        y += 1;
    }
}

/// `right` at the right edge of `at`, where it clears what `left` drew.
fn put_right(frame: &mut Frame, at: Rect, left: &Line, right: Option<&Line>) {
    let Some(right) = right else { return };
    let (lw, rw) = (left.width() as u16, right.width() as u16);
    if lw + 2 + rw <= at.width {
        frame.render_widget(Paragraph::new(right.clone()).alignment(Alignment::Right), at);
    }
}

/// Under the pointer: bright and bold, the kit's hover.
fn hover() -> Style {
    Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
}

/// A dim text button in `at`: bright and bold under the pointer, its click
/// over its own cells. Returns its rect.
fn text_button(frame: &mut Frame, page: &mut Page, at: Rect, text: &str, act: Act) -> Rect {
    let rect = Rect { width: (kit::width(text) as u16).min(at.width), height: 1, ..at };
    let style = if page.ui.hovers(rect) { hover() } else { dim() };
    frame.render_widget(Paragraph::new(Span::styled(text.to_string(), style)), rect);
    page.ui.click(rect, act);
    rect
}

/// A █░ bar across `at`: `permille` of it filled in `color`, the rest a
/// dim track. Drawn by hand rather than with ratatui's Gauge, whose
/// remainder swaps the colours (the player's own reason).
fn bar(frame: &mut Frame, at: Rect, permille: u32, color: Color) {
    let cells = u32::from(at.width);
    let mut filled = (cells * permille.min(1000) + 500) / 1000;
    if permille > 0 {
        filled = filled.max(1);
    }
    if permille < 1000 {
        filled = filled.min(cells.saturating_sub(1));
    }
    let filled = filled as u16;
    frame.render_widget(Paragraph::new(Span::styled("░".repeat(usize::from(at.width)), dim())), at);
    if filled > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled("█".repeat(usize::from(filled)), Style::default().fg(color))),
            Rect { width: filled, ..at },
        );
    }
}

/// The log in `pane`: the newest lines about the board that fit (`mm:ss`,
/// then the line in its kind's colour), scrolled up by the wheel.
fn draw_log(frame: &mut Frame, page: &mut Page, port: &str, pane: Rect) {
    page.log_rect = Some(pane);
    let lines: Vec<LogLine> = page.log_for(port).into_iter().cloned().collect();
    let rows = usize::from(pane.height);
    let scroll = page.log_scroll.min(lines.len().saturating_sub(rows));
    let end = lines.len() - scroll;
    let start = end.saturating_sub(rows);
    let width = usize::from(pane.width).saturating_sub(CLOCK_W);
    for (i, entry) in lines[start..end].iter().enumerate() {
        let secs = entry.at.as_secs();
        let clock = format!("{:02}:{:02}  ", secs / 60, secs % 60);
        let style = match entry.kind {
            LogKind::Phase => accent(),
            LogKind::Fact => Style::default(),
            LogKind::Fail => Style::default().fg(th().gold),
            LogKind::Quiet => dim(),
        };
        let row = Rect { y: pane.y + i as u16, height: 1, ..pane };
        let line = Line::from(vec![Span::styled(clock, dim()), Span::styled(fit(&entry.text, width), style)]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

/// `text`, cut to `width` cells with an ellipsis: a log line or a fact
/// never wraps.
fn fit(text: &str, width: usize) -> String {
    if kit::width(text) <= width {
        return text.to_string();
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = kit::width(c.encode_utf8(&mut [0u8; 4]));
        if used + w + 1 > width {
            break;
        }
        cut.push(c);
        used += w;
    }
    cut.push('…');
    cut
}

/// The card with no board (contract clause 30): what to do, the watch, and
/// "Not showing up?" with today's hints and the ports seen.
fn empty_card(frame: &mut Frame, page: &mut Page, room: Rect) {
    if room.height < 3 || room.width < CARD_INSET * 2 + 2 {
        return;
    }
    let w = room.width - inset(room.width) * 2;
    let g = glyphs();
    let mut rows = vec![Row::Blank];
    if let Some(e) = &page.list_failed {
        let gold = Style::default().fg(th().gold);
        rows.extend(wide_rows(&[(e.text(), gold.add_modifier(Modifier::BOLD))], w));
        if let Some(hint) = e.hint() {
            rows.extend(wide_rows(&[(hint, dim())], w));
        }
    } else if !page.watched {
        rows.extend(wide_rows(&scan(None, &Phase::Scanning.text()), w));
    } else {
        let title = Line::from(Span::styled(t!("dev.none_title").to_string(), bold()));
        rows.push(Row::Wide { left: title, right: None });
        rows.push(Row::Blank);
        rows.extend(wide_rows(&[(t!("dev.none_body").to_string(), Style::default())], w));
        rows.extend(wide_rows(&scan(None, &t!("dev.none_watching")), w));
    }
    rows.push(Row::Blank);
    let help = format!("{} {}", if page.details { g.open } else { g.forward }, t!("dev.not_showing"));
    rows.push(Row::Actions { links: vec![(help, Act::Details)], primary: None, covered: false });
    if page.details {
        rows.push(Row::Blank);
        // Today's hints, each under its bullet with its lines hung under the
        // words; the driver's second line is its bullet's.
        let bullet = |words: Vec<String>| -> Vec<Row> {
            let mut rows = Vec::new();
            for (i, text) in words.iter().enumerate() {
                let lines = wrap_spans(&[(text.clone(), dim())], usize::from(w.saturating_sub(2)));
                for (j, line) in lines.into_iter().enumerate() {
                    let lead = if i == 0 && j == 0 { format!("{} ", g.bullet) } else { "  ".to_string() };
                    let mut spans = vec![Span::styled(lead, dim())];
                    spans.extend(line.spans);
                    rows.push(Row::Wide { left: Line::from(spans), right: None });
                }
            }
            rows
        };
        rows.extend(bullet(vec![t!("dev.no_device_cable").to_string()]));
        rows.extend(bullet(vec![t!("dev.no_device_driver").to_string(), t!("dev.no_device_driver_2").to_string()]));
        rows.extend(bullet(vec![t!("dev.no_device_linux").to_string()]));
        // The cards' two, on the GUI's tab (clause 30).
        if page.transfer.is_some() {
            rows.extend(bullet(vec![t!("dev.card.hint_card_shows").to_string()]));
            rows.extend(bullet(vec![t!("dev.card.hint_own_disks").to_string()]));
        }
        rows.push(Row::Blank);
        let seen = if page.others.is_empty() {
            vec![(format!("{} {}", t!("dev.ports_seen"), t!("dev.ports_seen_none")), dim())]
        } else {
            vec![
                (format!("{} ", t!("dev.ports_seen")), dim()),
                (page.others.join(", "), Style::default()),
                (format!(" {}", t!("dev.ports_not_bridge")), dim()),
            ]
        };
        rows.extend(wide_rows(&seen, w));
    }
    rows.push(Row::Blank);
    draw_card(frame, page, "", &rows, room);
}

/// A card's view (clause 44) in `room`, as the board's card is drawn.
fn card_view(frame: &mut Frame, page: &mut Page, view: &crate::device::card::worker::CardView, room: Rect) {
    if room.height < 3 || room.width < CARD_INSET * 2 + 2 {
        return;
    }
    let content_w = room.width - inset(room.width) * 2;
    let mut rows = card::card_layout(page, view, content_w);
    card::give_way_rows(&mut rows, room.height.saturating_sub(2));
    draw_card(frame, page, &view.key, &rows, room);
}

// ── The gate ────────────────────────────────────────────────────────────────

/// A line of the gate's body: text, a blank that gives way when the rows
/// run short, or the erase box.
enum GateLine {
    Text(Line<'static>),
    Blank,
    Erase(Line<'static>),
}

/// The gate's words, wrapped in `width` cells (contract clause 23): what
/// goes on and how long, what you see, what stays or goes, the one danger.
fn gate_lines(gate: &Gate, width: usize) -> Vec<GateLine> {
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let green = Style::default().fg(th().ok);
    let mut lines = Vec::new();
    let text = |lines: &mut Vec<GateLine>, words: String, style: Style| {
        for line in wrap_spans(&[(words, style)], width) {
            lines.push(GateLine::Text(line));
        }
    };
    let from = gate.from.clone().unwrap_or_else(|| "?".to_string());
    // The build the write puts on, after the version, where the gate names
    // it (clause 39): `Update from v0.7.0 to v0.8.0 · DIO?`.
    let to = match gate.title_mode {
        Some(mode) => format!("{} · {}", gate.to, mode.word()),
        None => gate.to.clone(),
    };
    let title = match gate.kind {
        Kind::Update => t!("dev.gate_title_update", from = from, to = to),
        Kind::Replace => t!("dev.gate_title_replace", from = from, to = to),
        Kind::Install => t!("dev.gate_title_install", to = to),
        Kind::Again => t!("dev.gate_title_again", to = to),
        Kind::Back => t!("dev.gate_title_back", from = from, to = to),
        Kind::All => t!("dev.gate_title_all", n = gate.rows.len(), to = to),
        Kind::Mode => t!("dev.gate_title_mode", to = to),
        Kind::Local => t!("dev.gate_title_local", from = from),
        Kind::Format => t!("dev.card.gate_fmt_title", port = short(gate.port.as_deref().unwrap_or("?"))),
    };
    text(&mut lines, title.to_string(), gold.add_modifier(Modifier::BOLD));
    lines.push(GateLine::Blank);
    // What a chosen write puts on and the check it passed, and what may go
    // wrong: under the title, before today's words.
    let tone = |tone: Tone| if tone == Tone::Gold { gold } else { Style::default() };
    for (words, how) in &gate.lines {
        text(&mut lines, words.clone(), tone(*how));
    }
    if !gate.lines.is_empty() {
        lines.push(GateLine::Blank);
    }
    let old = gate.name.clone().unwrap_or_else(|| t!("dev.gate_old_firmware").to_string());
    match gate.kind {
        // What the player erases and how long it takes, that it asks on its
        // own screen first, what is lost in gold, and its device.txt
        // (clause 59).
        Kind::Format => {
            let ask = gate.format.clone().unwrap_or(FormatAsk { size: None, lost: String::new() });
            let size = ask.size.map_or_else(|| "?".to_string(), gb);
            let took = super::card::span(format_secs(ask.size.unwrap_or(0)));
            text(&mut lines, t!("dev.card.gate_fmt_body", size = size, t = took).to_string(), Style::default());
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.card.gate_fmt_confirm").to_string(), Style::default());
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.card.gate_fmt_lost", what = ask.lost).to_string(), gold);
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.card.gate_fmt_txt").to_string(), Style::default());
        }
        Kind::All => {
            text(&mut lines, t!("dev.gate_all_time").to_string(), Style::default());
            lines.push(GateLine::Blank);
            // Six boards at most, the boards left out among them.
            let left = gate.left_out.len().min(GATE_ROWS.saturating_sub(gate.rows.len().min(GATE_ROWS)));
            let shown = GATE_ROWS - left;
            for row in gate.rows.iter().take(shown) {
                let mut words = short(&row.port);
                if let Some(serial) = &row.serial {
                    words.push_str(&format!(" · {}", t!("dev.serial", serial = serial)));
                }
                let to = match row.dio {
                    true => t!("dev.next_in", what = to, mode = Mode::Dio.word()).to_string(),
                    false => to.clone(),
                };
                words.push_str(&format!(" · {} {} {to}", row.from, g.arrow));
                lines.push(GateLine::Text(Line::from(Span::raw(fit(&words, width)))));
            }
            if gate.rows.len() > shown {
                let more = t!("dev.gate_all_more", n = gate.rows.len() - shown).to_string();
                lines.push(GateLine::Text(Line::from(Span::styled(more, dim()))));
            }
            for (port, what) in gate.left_out.iter().take(left) {
                let words = t!("dev.gate_all_left_out", port = short(port), what = what).to_string();
                lines.push(GateLine::Text(Line::from(Span::styled(fit(&words, width), dim()))));
            }
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_all_stays").to_string(), green);
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_all_plugged").to_string(), Style::default());
        }
        Kind::Install => {
            let time = match &gate.name {
                Some(name) => t!("dev.gate_install_time", name = name),
                None => t!("dev.gate_blank_time"),
            };
            text(&mut lines, time.to_string(), Style::default());
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_unknown").to_string(), gold);
            lines.push(GateLine::Blank);
            let mark = if gate.erase { "[x]" } else { "[ ]" };
            lines.push(GateLine::Erase(Line::from(format!("{mark} {}", t!("dev.gate_erase")))));
            let note =
                if gate.erase { t!("dev.gate_erase_on", name = old) } else { t!("dev.gate_erase_off", name = old) };
            text(&mut lines, note.to_string(), dim());
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_card_untouched").to_string(), green);
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_plugged_install").to_string(), Style::default());
        }
        _ => {
            text(&mut lines, t!("dev.gate_time").to_string(), Style::default());
            lines.push(GateLine::Blank);
            if gate.erase_any {
                // The sheet's Erase: what goes in gold, what stays in green.
                text(&mut lines, t!("dev.gate_erase_any").to_string(), gold);
                text(&mut lines, t!("dev.gate_card_stays").to_string(), green);
            } else {
                text(&mut lines, t!("dev.gate_stays").to_string(), green);
            }
            for (words, how) in &gate.after {
                text(&mut lines, words.clone(), tone(*how));
            }
            lines.push(GateLine::Blank);
            let plugged = if gate.kind == Kind::Local { "dev.gate_plugged_local" } else { "dev.gate_plugged" };
            text(&mut lines, t!(plugged).to_string(), Style::default());
        }
    }
    lines.push(GateLine::Blank);
    lines
}

/// The gate's two buttons: the safe choice, named for what stays, then
/// the write.
pub(super) fn gate_buttons(gate: &Gate) -> (String, String) {
    let back = glyphs().back;
    let keep = match gate.kind {
        Kind::All => t!("dev.gate_keep_all").to_string(),
        Kind::Format => t!("dev.card.gate_fmt_keep").to_string(),
        Kind::Install | Kind::Mode => t!("dev.gate_keep_as_is").to_string(),
        _ => match &gate.from {
            Some(from) => t!("dev.gate_keep", version = from).to_string(),
            None => t!("dev.gate_keep_as_is").to_string(),
        },
    };
    let write = match gate.kind {
        // An erase the sheet chose: the button says both verbs.
        Kind::Update | Kind::Replace if gate.erase_any => t!("dev.gate_do_erase_update"),
        _ if gate.erase_any => t!("dev.gate_do_erase_write"),
        Kind::Mode => t!("dev.gate_do_mode", mode = gate.title_mode.map_or("?", Mode::word)),
        kind => t!(match kind {
            Kind::Update | Kind::Replace => "dev.gate_do_update",
            Kind::Install => "dev.gate_do_install",
            Kind::Again => "dev.gate_do_again",
            Kind::Back => "dev.gate_do_back",
            Kind::All => "dev.gate_do_all",
            Kind::Mode | Kind::Local => "dev.gate_do_local",
            Kind::Format => "dev.card.gate_fmt_do",
        }),
    };
    (format!("{back} {keep}"), write.to_string())
}

/// The kit's gold warning modal over the card: no [X], the safe choice
/// first and in the modal primary, centred in the page's area.
fn draw_gate(frame: &mut Frame, page: &mut Page, area: Rect) {
    let Some(gate) = page.gate.clone() else { return };
    let width = GATE_W.min(area.width.saturating_sub(4));
    if width < 12 || area.height < 5 {
        return;
    }
    let text_w = usize::from(width.saturating_sub(4));
    let mut lines = gate_lines(&gate, text_w);
    // The frame's two rows and the buttons' row: short of rows, the blank
    // lines give way first, from the end.
    let most = usize::from(area.height.saturating_sub(2)).saturating_sub(3);
    while lines.len() > most {
        let Some(at) = lines.iter().rposition(|l| matches!(l, GateLine::Blank)) else { break };
        lines.remove(at);
    }
    let height = (lines.len() + 3) as u16;
    let inner = kit::modal_frame_on(frame, &mut page.ui, area, width, height, th().gold);
    if inner.height == 0 {
        return;
    }
    let body = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(1),
    };
    for (i, line) in lines.iter().enumerate() {
        let y = body.y + i as u16;
        if y >= body.bottom() {
            break;
        }
        let at = Rect { y, height: 1, ..body };
        match line {
            GateLine::Blank => {}
            GateLine::Text(line) => frame.render_widget(Paragraph::new(line.clone()), at),
            GateLine::Erase(line) => {
                let rect = Rect { width: (line.width() as u16).min(at.width), ..at };
                let style = if page.ui.hovers(rect) { Style::default().fg(th().bright) } else { Style::default() };
                frame.render_widget(Paragraph::new(line.clone().style(style)), rect);
                page.ui.click(rect, Act::Erase);
            }
        }
    }
    let y = inner.bottom().saturating_sub(1);
    let (keep, write) = gate_buttons(&gate);
    let keep_w = kit::width(&keep) as u16 + 4;
    let write_w = kit::width(&write) as u16 + 4;
    let keep_x = inner.right().saturating_sub(write_w + 2 + keep_w).max(inner.x);
    let at = Rect { x: keep_x, y, width: inner.right() - keep_x, height: 1 };
    let keep_rect = kit::button(frame, &mut page.ui, at, &keep, true, Act::Keep);
    let write_x = keep_rect.right() + 2;
    if write_x < inner.right() {
        let at = Rect { x: write_x, y, width: inner.right() - write_x, height: 1 };
        kit::button(frame, &mut page.ui, at, &write, false, Act::Write);
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_is_by_words_in_cells_and_breaks_a_sentence_with_no_spaces() {
        let plain = Style::default();
        let lines = wrap_spans(&[("one two three".into(), plain)], 7);
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(text, ["one two", "three"]);
        let glued = wrap_spans(&[("v0.8.0".into(), plain), ("…".into(), bold())], 20);
        assert_eq!(glued[0].spans.len(), 2, "two styles, one word");
        let filled = wrap_spans(&[("MP3 プレーヤーの画面が暗く".into(), plain)], 12);
        let filled: Vec<String> = filled.iter().map(|l| l.to_string()).collect();
        assert_eq!(filled, ["MP3 プレーヤ", "ーの画面が暗", "く"], "a Japanese word fills the line it starts on");
        let cjk = wrap_spans(&[("ボードが応答しません".into(), plain)], 8);
        assert_eq!(cjk.iter().map(|l| l.to_string()).collect::<Vec<_>>(), ["ボードが", "応答しま", "せん"]);
        assert_eq!(fit("twelve chars", 8), "twelve …");
        assert_eq!(fit("short", 10), "short");
    }
}
