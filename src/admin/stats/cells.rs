//! The stats page in character cells: what `mstream-player stats` draws
//! in a terminal, and what the GUI's Stats screen draws on its grid when
//! no egui panel takes the page ([`render_hosted`]). The page itself (its
//! state, its worker, its keys, its words) is the parent module; this is
//! one of its two views, beside `egui_view`, and holds what is measured in
//! cells: the layout, the charts' eighth-blocks, the tables and the two
//! modals, on the kit's surface. It is also where the page meets the hub's
//! loop, as a [`Screen`].

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::super::{Screen, draw_bottom, draw_header_as, frame_ground, host_of};
use super::*;
use crate::kit::theme::th;
use crate::kit::{self, Surface, bold, dim};
use crate::setup::g;

const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
const TAB_GAP: u16 = 2;
/// Both charts are eighth-block columns: three rows on a short terminal,
/// up to eight as the screen grows (stats-screen contract, clause 10).
const CHART_ROWS_MIN: u16 = 3;
const CHART_ROWS_MAX: u16 = 8;
/// The rows a chart spends around its columns: the title above, the
/// baseline and the labels below.
const CHART_FRAME: u16 = 3;
/// A tile's card: the frame's two rows around the value, the label and
/// the detail (stats-screen contract, clause 13).
const CARD_ROWS: u16 = 5;
/// The share bar's cells.
const SHARE_W: usize = 10;
/// The fewest cells the split's bar beside the hours chart shrinks to
/// before the split gives way (stats-screen contract, clause 12).
const SHARE_MIN: usize = 4;

/// What this view keeps on the page between frames. The page holds one
/// and never looks inside it.
pub(super) struct State {
    /// The kit's interaction surface: what the last frame registered.
    ui: Surface<Act>,
    /// Where the period's name was drawn this frame: the list drops from
    /// under it.
    period_at: Option<Rect>,
    /// The cursor as the table last saw it: the table scrolls to the
    /// cursor only on the frame it moved.
    sel_anchor: Option<usize>,
}

impl State {
    pub(super) fn new() -> State {
        State { ui: Surface::new(), period_at: None, sel_anchor: None }
    }
}

// ── The hub's view of the page ────────────────────────────────────────────

impl Screen for Page {
    type Act = Act;

    fn ui(&mut self) -> &mut Surface<Act> {
        &mut self.cells.ui
    }

    fn absorb(&mut self) -> bool {
        Page::absorb(self)
    }

    fn pump(&mut self) {
        Page::pump(self)
    }

    fn awaiting(&self) -> bool {
        Page::awaiting(self)
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        Page::key(self, key)
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        Page::act(self, act)
    }

    fn wheel(&mut self, up: bool, _at: Position) {
        match &mut self.modal {
            Modal::None => {
                self.tscroll = if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
            }
            Modal::Period(cursor) => {
                let n = if self.options.is_empty() { 2 } else { self.options.len() };
                *cursor = if up { cursor.saturating_sub(1) } else { (*cursor + 1).min(n - 1) };
            }
            Modal::Forget(_) => {}
        }
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, page: &mut Page) {
    page.cells.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    let host = host_of(&page.client);
    let right = match &page.username {
        Some(user) => format!("{host} · {}", printable(user, 64)),
        None => host,
    };
    draw_header_as(frame, area, &t!("sta.title"), &right);
    draw_page(frame, page, area);
}

/// The page inside another shell's area (the GUI's Stats screen): no
/// header — the host's top bar names the server — and no tips row — the
/// host's footer carries [`footer_hint`] — on the ground the host painted.
/// The area's first row stays blank, as the page's own header row does,
/// and the note takes the last.
pub(crate) fn render_hosted(frame: &mut Frame, page: &mut Page, area: Rect) {
    page.cells.ui.begin_frame();
    draw_page(frame, page, area);
}

fn draw_page(frame: &mut Frame, page: &mut Page, area: Rect) {
    // A modal makes the page beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(page.modal, Modal::None);
    let live_pointer = page.cells.ui.pointer;
    if modal_open {
        page.cells.ui.pointer = None;
    }
    page.cells.period_at = None;

    // The tabs are the body's first row — no state line: the tiles carry
    // the period's totals (stats-screen contract, clause 15). The bottom
    // edge is the note and the tips; the page keeps the row above them,
    // which the rooms leave blank — a chart's axis lands there. Hosted,
    // the header row and the tips row are the host's: one blank row under
    // its bar, the note on the area's last row.
    let (top, bottom) = if page.hosted { (1, 1) } else { (2, 2) };
    let column = Rect {
        x: area.x + 2,
        y: area.y + top,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(top + bottom),
    };
    draw_body(frame, page, column);

    // The cursor row's own line, when nothing louder holds the note line.
    let row_note = match (&page.note, &page.busy, page.sel) {
        (None, None, Some(i)) if !modal_open => row_words(page, i).map(|w| (w, false)),
        _ => None,
    };
    let note = page.note.clone().or(row_note);
    if page.hosted {
        let line = Rect { x: area.x + 2, y: area.bottom().saturating_sub(1), width: area.width.saturating_sub(4), height: 1 };
        if let Some((text, is_err)) = &note {
            let style = if *is_err { Style::default().fg(th().gold) } else { dim() };
            frame.render_widget(Paragraph::new(Span::styled(text.clone(), style)), line);
        }
        if let Some(busy) = &page.busy {
            frame.render_widget(Paragraph::new(Span::styled(busy.clone(), crate::kit::accent())), line);
        }
    } else {
        draw_bottom(frame, area, note.as_ref(), page.busy.as_deref(), &footer_hint(page));
    }

    if modal_open {
        page.cells.ui.pointer = live_pointer;
        page.cells.ui.clear_registries();
    }
    match page.modal.clone() {
        Modal::None => {}
        Modal::Period(cursor) => draw_period_list(frame, page, area, cursor),
        Modal::Forget(row) => draw_forget(frame, page, area, row),
    }
}

/// The tabs, the tab's note, the tab's body — or, on a server without
/// the API, its one gold sentence where the body would be.
fn draw_body(frame: &mut Frame, page: &mut Page, column: Rect) {
    let tabs_y = column.y;
    let mut x = column.x;
    for tab in Tab::ALL {
        let label = format!(" {} ", tab.name());
        // In cells, as the kit draws the label, and clamped to the column:
        // counted by characters a Japanese tab got a rect half its width,
        // so the label was cut there, its click covered only what was left,
        // and the next tab started inside it.
        let rect = Rect { x, y: tabs_y, width: (kit::width(&label) as u16).min(column.right().saturating_sub(x)), height: 1 };
        let hover = page.cells.ui.pointer.is_some_and(|pt| rect.contains(pt));
        let style = if tab == page.tab {
            Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
        } else if hover {
            Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        page.cells.ui.click(rect, Act::Tab(tab));
        x = rect.right() + TAB_GAP;
    }
    draw_tab_note(frame, page, Rect { x: x + 2, y: tabs_y, width: column.right().saturating_sub(x + 2), height: 1 });

    let body = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: column.bottom().saturating_sub(tabs_y + 2) };
    if body.height == 0 {
        return;
    }
    if page.no_api {
        gold_line(frame, Rect { height: 1, ..body }, &t!("sta.noapi_state"), &t!("sta.noapi_detail"));
        return;
    }
    match page.tab {
        Tab::Overview => draw_overview(frame, page, body),
        Tab::Top => draw_top(frame, page, body),
        Tab::Recent => draw_recent(frame, page, body),
    }
}

/// The tab row's right edge: the pane's note, and on the tabs with no
/// period control of their own the period's name before it, as the
/// control — `This month ▾ · the log, newest first` — so every tab names
/// the range it shows and can change it (stats-screen contract, clause
/// 14). The log's note ends in "times in UTC" on a machine without a
/// zone. Parts drop from the right when the row is short of room.
fn draw_tab_note(frame: &mut Frame, page: &mut Page, at: Rect) {
    let control = page.tab != Tab::Overview && page.data.is_some() && !page.no_api && !page.no_plays_ever();
    let name = control.then(|| format!("{}{}", page.period.name, g(" ▾", " v")));
    let mut parts: Vec<String> = name.iter().cloned().collect();
    parts.extend(page.tab.note());
    if page.tab == Tab::Recent && page.zone.is_none() {
        parts.push(t!("sta.zone_utc").to_string());
    }
    // In cells, as period_control draws the name that leads it: counted by
    // characters a Japanese note was right-anchored too far right, and the
    // control, wider than counted, pushed the rest past the row's end.
    while !parts.is_empty() && kit::width(&parts.join(" · ")) > at.width as usize {
        parts.pop();
    }
    if parts.is_empty() {
        return;
    }
    let text = parts.join(" · ");
    let mut x = at.right().saturating_sub(kit::width(&text) as u16);
    if name.is_some() {
        x = period_control(frame, page, x, at.y);
        parts.remove(0);
        if parts.is_empty() {
            return;
        }
        let rest = format!(" · {}", parts.join(" · "));
        frame.render_widget(Paragraph::new(Span::styled(rest, dim())), Rect { x, y: at.y, width: at.right().saturating_sub(x), height: 1 });
    } else {
        frame.render_widget(Paragraph::new(Span::styled(text, dim())), Rect { x, y: at.y, width: at.right().saturating_sub(x), height: 1 });
    }
}

/// One gold sentence — `• head` bold, the tail after it — for a state the
/// page has nothing else to show: no Stats API, no plays yet.
fn gold_line(frame: &mut Frame, at: Rect, head: &str, tail: &str) {
    let gold = Style::default().fg(th().gold);
    let spans = vec![Span::styled(head.to_string(), gold.add_modifier(Modifier::BOLD)), Span::styled(tail.to_string(), gold)];
    frame.render_widget(Paragraph::new(Line::from(spans)), at);
}

/// The period control, `This month ▾`: the name bold, the chevron dim,
/// both bright under the pointer; a click drops the list under it, or
/// closes it. Returns the x past it.
fn period_control(frame: &mut Frame, page: &mut Page, x: u16, y: u16) -> u16 {
    let name = page.period.name.clone();
    let chevron = g(" ▾", " v");
    // In cells, as the origin row beside it measures: its x follows this.
    let rect = Rect { x, y, width: (kit::width(&name) + kit::width(chevron)) as u16, height: 1 };
    let hover = page.cells.ui.hovers(rect);
    let (name_style, chevron_style) = if hover {
        let bright = Style::default().fg(th().bright).add_modifier(Modifier::BOLD);
        (bright, bright)
    } else {
        (bold(), dim())
    };
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(name, name_style), Span::styled(chevron, chevron_style)])), rect);
    page.cells.ui.click(rect, Act::PeriodList);
    page.cells.period_at = Some(rect);
    rect.right()
}

/// `PERIOD  This month ▾        ORIGIN (•) all ( ) this server ( ) peers`
/// — the period a dropdown (stats-screen contract, clause 14), the origin
/// a radio row while the log holds peer plays.
fn draw_controls(frame: &mut Frame, page: &mut Page, at: Rect) {
    let put = |frame: &mut Frame, x: u16, text: &str, style: Style| -> u16 {
        let w = kit::width(text) as u16;
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), style)), Rect { x, y: at.y, width: w, height: 1 });
        x + w
    };
    let x = put(frame, at.x, &t!("sta.ctl_period"), dim()) + 2;
    let x = period_control(frame, page, x, at.y);

    if page.peer_plays() {
        let label = t!("sta.ctl_origin").to_string();
        let options: Vec<String> = Origin::ALL.iter().map(|o| o.label()).collect();
        let need = kit::width(&label) + 2 + options.iter().map(|o| kit::width(o) + 7).sum::<usize>();
        let start = at.right().saturating_sub(need as u16).max(x + 4);
        if start as usize + need <= at.right() as usize {
            let x = put(frame, start, &label, dim()) + 2;
            radio_row(frame, page, Rect { x, y: at.y, width: at.right().saturating_sub(x), height: 1 }, &options, page.origin.index(), Act::Origin);
        }
    }
}

/// `(•) name   ( ) name   ( ) name` across one row; returns the x past it.
/// Each option's rect is its name's width in cells and the glyph's four: a
/// character count made a Japanese option's rect short by half its name,
/// so the name was cut there and its click covered only what was left; the
/// `need` each caller right-anchors the row by measures the same way.
fn radio_row(frame: &mut Frame, page: &mut Page, at: Rect, options: &[String], chosen: usize, act: impl Fn(usize) -> Act) -> u16 {
    let mut x = at.x;
    for (i, name) in options.iter().enumerate() {
        let on = i == chosen;
        let w = kit::width(name) as u16 + 4;
        if x + w > at.right() {
            break;
        }
        let rect = Rect { x, y: at.y, width: w, height: 1 };
        let hover = page.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on { Style::default().fg(th().accent) } else { dim() };
        let name_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if on { g("(•)", "(*)") } else { "( )" }, glyph_style),
                Span::raw(" "),
                Span::styled(name.clone(), name_style),
            ])),
            rect,
        );
        page.cells.ui.click(rect, act(i));
        x += w + 3;
    }
    x
}

// ── Overview ──────────────────────────────────────────────────────────────

fn draw_overview(frame: &mut Frame, page: &mut Page, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let mut y = body.y;
    if page.no_api || page.data.is_none() {
        return;
    }
    if page.no_plays_ever() {
        // The webapp's empty state, in its words, under the gold sentence.
        gold_line(frame, line(y), &t!("sta.empty_state"), &t!("sta.empty_state_detail"));
        for (i, key) in ["sta.empty_1", "sta.empty_2"].iter().enumerate() {
            if y + 2 + (i as u16) < body.bottom() {
                frame.render_widget(Paragraph::new(t!(*key).to_string()), line(y + 2 + i as u16));
            }
        }
        if y + 5 < body.bottom() {
            let zone = if page.zone.is_some() { t!("sta.zone_local") } else { t!("sta.zone_utc") };
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.empty_prov", zone = zone).to_string(), dim())), line(y + 5));
        }
        return;
    }
    draw_controls(frame, page, line(y));
    y += 1;

    // Everything under the controls is the SHOWN period's: a change keeps
    // it until the new numbers land (clause 16).
    let Some(data) = page.data.as_ref() else { return };
    if data.summary.events == 0 {
        if y + 2 < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.nothing_title", period = page.shown.name.clone()).to_string(), bold())), line(y + 2));
        }
        if y + 4 < body.bottom() {
            let begins = data
                .periods
                .as_ref()
                .and_then(|p| p.earliest.as_deref())
                .and_then(iso_unix)
                .map(|t| date_long(t, page.offset_at(t)));
            let hint = match begins {
                Some(date) => t!("sta.nothing_hint", date = date).to_string(),
                None => t!("sta.nothing_hint_plain").to_string(),
            };
            frame.render_widget(Paragraph::new(Span::styled(hint, dim())), line(y + 4));
        }
        return;
    }

    // The six tiles, two rows of three cards (stats-screen contract,
    // clause 13): the kit's rounded frame, dim, no fill; inside, the value
    // bold, the label and the detail dim. The cards fill the column, one
    // cell between, and the rows stack frame to frame.
    let versus = versus_label(&page.shown, &page.options);
    let tiles = tiles(&data.summary, data.previous.as_ref(), versus.as_deref());
    let columns = card_columns(body);
    for (i, (value, label, detail)) in tiles.iter().enumerate() {
        let (x, w) = columns[i % 3];
        let top = y + (i as u16 / 3) * CARD_ROWS;
        if top + CARD_ROWS > body.bottom() {
            break;
        }
        let card = Rect { x, y: top, width: w, height: CARD_ROWS };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(dim());
        let inner = block.inner(card);
        frame.render_widget(block, card);
        let text_w = inner.width.saturating_sub(1);
        let row = |dy: u16| Rect { x: inner.x + 1, y: inner.y + dy, width: text_w, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(clip(value, text_w), bold())), row(0));
        frame.render_widget(Paragraph::new(Span::styled(clip(label, text_w), dim())), row(1));
        frame.render_widget(Paragraph::new(Span::styled(clip(detail, text_w), dim())), row(2));
    }
    y += 2 * CARD_ROWS;

    // Plays per day (or week, or month): one eighth-block column per
    // bucket, on a scale. The two charts share the rows under the tiles.
    let rows = chart_rows(body.bottom().saturating_sub(y));
    if y + rows + CHART_FRAME > body.bottom() {
        return;
    }
    let from = match (page.shown.period.as_str(), data.periods.as_ref().and_then(|p| p.earliest.as_deref())) {
        // All time starts at the retention floor, years before the first
        // play; the chart starts where the log does.
        ("all", Some(earliest)) => earliest.to_string(),
        _ => data.summary.period.from.clone(),
    };
    let (keys, values, bucket) = calendar_series(&data.series, &from, &data.summary.period.to, |t| page.offset_at(t));
    let width = body.width as usize;
    // Buckets fold into pairs (or more) when the row is too narrow for a
    // column each. The axis takes a gutter sized by the tallest folded
    // column — which folding raises — so fold, size, and fold again while
    // the gutter grows.
    let mut gutter_w = gutter(values.iter().copied().max().unwrap_or(0), rows) as usize;
    let (values, fold) = loop {
        let fold = values.len().div_ceil(width.saturating_sub(gutter_w).max(1)).max(1);
        let folded: Vec<u64> = values.chunks(fold).map(|c| c.iter().sum()).collect();
        let wide = gutter(folded.iter().copied().max().unwrap_or(0), rows) as usize;
        if wide <= gutter_w {
            break (folded, fold);
        }
        gutter_w = wide;
    };
    let title = match (bucket, fold) {
        ("day", 1) => t!("sta.chart_days").to_string(),
        ("day", n) => t!("sta.chart_days_folded", n = n).to_string(),
        ("week", _) => t!("sta.chart_weeks").to_string(),
        _ => t!("sta.chart_months").to_string(),
    };
    frame.render_widget(Paragraph::new(Span::styled(title.clone(), dim())), line(y));
    let days_note = days_note(&data.summary);
    if kit::width(&title) + 2 + kit::width(&days_note) <= width {
        frame.render_widget(Paragraph::new(Span::styled(days_note, dim())).alignment(Alignment::Right), line(y));
    }
    let cell_w = ((width.saturating_sub(gutter_w) / values.len().max(1)) as u16).clamp(1, 3);
    let labels: Vec<(usize, String)> = match bucket {
        "day" => day_axis(&keys, fold),
        "week" => week_axis(&keys, fold),
        _ => month_axis(&keys, fold),
    };
    chart(frame, Rect { x: body.x, y: y + 1, width: body.width, height: rows + 2 }, &values, cell_w, &labels);
    y += rows + CHART_FRAME;

    // When you listen — the 24-hour profile — and, beside it, where the
    // tracks live, while the log holds peer plays.
    if y + rows + CHART_FRAME > body.bottom() {
        return;
    }
    let hours = hour_series(&data.hours);
    let hours_w: u16 = gutter(hours.iter().copied().max().unwrap_or(0), rows) + 48;
    // Beside it, where the tracks live, while the log holds peer plays and
    // the row has room for the split's words on a bar of at least
    // SHARE_MIN cells.
    let o = &data.summary.origins;
    let name_w = [t!("sta.origin_local_row"), t!("sta.origin_peers_row")].iter().map(|s| kit::width(s)).max().unwrap_or(0) + 1;
    let words = |plays: u64, ms: u64| t!("sta.plays_time", plays = plays_words(plays), time = fmt_duration(ms)).to_string();
    let (local_words, peer_words) = (words(o.local.plays, o.local.listened_ms), words(o.peers.plays, o.peers.listened_ms));
    let widest = kit::width(&local_words).max(kit::width(&peer_words));
    let right_w = body.width.saturating_sub(hours_w + 3);
    let left_w = if page.peer_plays() && right_w as usize >= name_w + SHARE_MIN + 2 + widest { hours_w } else { body.width };
    let title = t!("sta.chart_hours").to_string();
    frame.render_widget(Paragraph::new(Span::styled(title.clone(), dim())), line(y));
    if let Some(note) = hours_note(&data.summary)
        && kit::width(&title) + 2 + kit::width(&note) <= left_w as usize
    {
        // In cells, as drawn: a Japanese title counted by characters put
        // the note over its second half.
        let title_w = kit::width(&title) as u16;
        frame.render_widget(
            Paragraph::new(Span::styled(note, dim())),
            Rect { x: body.x + title_w + 2, y, width: left_w - title_w - 2, height: 1 },
        );
    }
    let hour_labels: Vec<(usize, String)> = [0, 6, 12, 18, 23].iter().map(|h| (*h, h.to_string())).collect();
    chart(frame, Rect { x: body.x, y: y + 1, width: hours_w.min(body.width), height: rows + 2 }, &hours, 2, &hour_labels);

    if left_w < body.width {
        let x = body.x + left_w + 3;
        let w = body.right().saturating_sub(x);
        let row = |dy: u16| Rect { x, y: y + dy, width: w, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(t!("sta.origins_title").to_string(), dim())), row(0));
        // The bar: ten cells, fewer when the row is short of them.
        let bar_w = (w as usize).saturating_sub(name_w + 2 + widest).min(SHARE_W);
        let total = (o.local.plays + o.peers.plays).max(1);
        let local_share = ((o.local.plays * bar_w as u64 + total / 2) / total) as usize;
        let split = |frame: &mut Frame, at: Rect, name: String, filled: usize, words: &str| {
            // Padded to name_w in cells: format!'s width counts characters,
            // so the two Japanese names' bars started at different columns.
            let pad = " ".repeat(name_w.saturating_sub(kit::width(&name)));
            let spans = vec![
                Span::raw(format!("{name}{pad}")),
                Span::styled(g("▰", "■").repeat(filled), Style::default().fg(th().accent)),
                Span::styled(g("▱", "·").repeat(bar_w - filled), dim()),
                Span::raw(format!("  {words}")),
            ];
            frame.render_widget(Paragraph::new(Line::from(spans)), at);
        };
        split(frame, row(1), t!("sta.origin_local_row").to_string(), local_share, &local_words);
        frame.render_widget(Paragraph::new(Span::styled(t!("sta.origin_local_note").to_string(), dim())), Rect { x: x + name_w as u16, ..row(2) });
        split(frame, row(3), t!("sta.origin_peers_row").to_string(), bar_w - local_share, &peer_words);
        frame.render_widget(Paragraph::new(Span::styled(t!("sta.origin_peers_note_1").to_string(), dim())), Rect { x: x + name_w as u16, ..row(4) });
        if y + 5 < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.origin_peers_note_2").to_string(), dim())), Rect { x: x + name_w as u16, ..row(5) });
        }
    }
}

/// The three card columns across `body` as (x, width): one cell between
/// them, the width's remainder to the first cards, so the row's edges are
/// the column's own.
fn card_columns(body: Rect) -> [(u16, u16); 3] {
    let usable = body.width.saturating_sub(2);
    let (base, extra) = (usable / 3, usable % 3);
    let mut out = [(0, 0); 3];
    let mut x = body.x;
    for (i, slot) in out.iter_mut().enumerate() {
        let w = base + u16::from((i as u16) < extra);
        *slot = (x, w);
        x += w + 1;
    }
    out
}

/// The rows for each of the two charts, from the rows left under the
/// tiles: an equal share, three at the least — a short terminal still
/// gets a chart — and eight at the most (stats-screen contract, clause
/// 10).
fn chart_rows(avail: u16) -> u16 {
    (avail.saturating_sub(2 * CHART_FRAME) / 2).clamp(CHART_ROWS_MIN, CHART_ROWS_MAX)
}

/// The scale a chart of `rows` rows draws `vmax` on (stats-screen
/// contract, clause 8): the axis's top, and for each row from the top
/// down the value at its upper edge when that is a whole number — those
/// rows get a tick and a label. Once the tallest column reaches the rows'
/// count every row is the same whole number of plays and every row is
/// labelled; below that the tallest column takes the whole height and the
/// whole numbers fall where they fall.
fn scale(vmax: u64, rows: u16) -> (u64, Vec<Option<u64>>) {
    let rows = u64::from(rows.max(1));
    let top = if vmax >= rows { vmax.div_ceil(rows) * rows } else { vmax.max(1) };
    let ticks = (0..rows)
        .map(|r| {
            let edge = top * (rows - r);
            (edge % rows == 0).then_some(edge / rows)
        })
        .collect();
    (top, ticks)
}

/// The cells a chart's axis takes before its first column: the widest
/// tick label, a space, the axis, a space.
fn gutter(vmax: u64, rows: u16) -> u16 {
    fmt_count(scale(vmax, rows).0).chars().count() as u16 + 3
}

/// A column chart with its furniture (stats-screen contract, clauses 8
/// and 9): `values` left to right in eighth-block columns `cell_w` cells
/// wide (the last cell a gap when there is room) on all of `at`'s rows
/// but the last two, scaled to [`scale`]'s top; down the left the ticks
/// and their labels, `0` at the foot; under the columns the baseline, a
/// tick under every column that carries one of `labels`; the labels on
/// the last row, each under its tick, none overlapping the last.
fn chart(frame: &mut Frame, at: Rect, values: &[u64], cell_w: u16, labels: &[(usize, String)]) {
    let rows = at.height.saturating_sub(2);
    if rows == 0 {
        return;
    }
    let vmax = values.iter().copied().max().unwrap_or(0);
    let (top, ticks) = scale(vmax, rows);
    let label_w = fmt_count(top).chars().count();
    let plot_x = at.x + label_w as u16 + 3;
    let plot_w = at.right().saturating_sub(plot_x);
    let cell_w = cell_w.max(1);
    let bar_w = if cell_w > 1 { cell_w as usize - 1 } else { 1 };
    let n = values.len().min((plot_w / cell_w) as usize);
    let glyphs = crate::tui::ui::glyphs();
    let ink = Style::default().fg(th().accent);
    for (row, tick) in ticks.iter().enumerate() {
        let (label, axis) = match tick {
            Some(v) => (fmt_count(*v), "┤"),
            None => (String::new(), "│"),
        };
        let mut spans = vec![Span::styled(format!("{label:>label_w$} {axis} "), dim())];
        for v in &values[..n] {
            let eighths = ((*v as f64 / top as f64) * (rows as usize * 8) as f64).round() as i64;
            let level = (eighths - ((rows as usize - 1 - row) * 8) as i64).clamp(0, 8) as usize;
            spans.push(Span::styled(glyphs.eighths[level].repeat(bar_w), ink));
            if cell_w > 1 {
                spans.push(Span::raw(" "));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), Rect { x: at.x, y: at.y + row as u16, width: at.width, height: 1 });
    }
    // The labels with room, left to right; then the baseline, a tick under
    // each of them.
    let mut placed: Vec<(u16, &str)> = Vec::new();
    let mut end = plot_x;
    for (i, text) in labels {
        if *i >= n {
            continue;
        }
        let x = plot_x + *i as u16 * cell_w;
        // In cells, as drawn: a Japanese month ("1月") counted by characters
        // was a cell short, so its rect cut it and the next label crowded it.
        let w = kit::width(text) as u16;
        if x < end || x + w > at.right() {
            continue;
        }
        placed.push((x, text.as_str()));
        end = x + w + 1;
    }
    let base_y = at.y + rows;
    let mut base = format!("{:>label_w$} ┼─", fmt_count(0));
    for px in 0..(n as u16 * cell_w).min(plot_w) {
        base.push(if placed.iter().any(|(x, _)| *x == plot_x + px) { '┴' } else { '─' });
    }
    frame.render_widget(Paragraph::new(Span::styled(base, dim())), Rect { x: at.x, y: base_y, width: at.width, height: 1 });
    for (x, text) in placed {
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), dim())), Rect { x, y: base_y + 1, width: kit::width(text) as u16, height: 1 });
    }
}

// ── Top ───────────────────────────────────────────────────────────────────

fn draw_top(frame: &mut Frame, page: &mut Page, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let mut y = body.y;
    if page.no_api || page.data.is_none() {
        return;
    }
    // SHOW and RANK BY, both radio rows.
    let mut x = body.x;
    let put = |frame: &mut Frame, x: u16, text: &str| -> u16 {
        let w = kit::width(text) as u16;
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), dim())), Rect { x, y, width: w, height: 1 });
        x + w
    };
    x = put(frame, x, &t!("sta.ctl_show")) + 2;
    let entities: Vec<String> = Entity::ALL.iter().map(|e| e.label()).collect();
    x = radio_row(frame, page, Rect { x, y, width: body.right().saturating_sub(x), height: 1 }, &entities, page.entity.index(), Act::Entity);
    let rank = t!("sta.ctl_rank").to_string();
    let metrics = [Metric::Plays.label(), Metric::Time.label()];
    let need = kit::width(&rank) + 2 + metrics.iter().map(|m| kit::width(m) + 7).sum::<usize>();
    let start = body.right().saturating_sub(need as u16).max(x + 2);
    if start as usize + need <= body.right() as usize {
        let x = put(frame, start, &rank) + 2;
        radio_row(frame, page, Rect { x, y, width: body.right().saturating_sub(x), height: 1 }, &metrics, if page.metric == Metric::Plays { 0 } else { 1 }, Act::Metric);
    }
    y += 2;

    // The table: rank, name, [tracks], share, plays, time, [last played].
    let tracks = page.entity == Entity::Tracks;
    let right = body.right();
    let (last_x, last_w) = if tracks { (right.saturating_sub(12), 12) } else { (right, 0) };
    let time_w = 7;
    let time_x = if tracks { last_x.saturating_sub(2 + time_w) } else { right.saturating_sub(time_w) };
    let plays_w = 6;
    let plays_x = time_x.saturating_sub(2 + plays_w);
    let share_x = plays_x.saturating_sub(2 + SHARE_W as u16);
    let (tracks_x, tracks_w) = if tracks { (share_x, 0) } else { (share_x.saturating_sub(12), 10) };
    let name_x = body.x + 4;
    let name_w = tracks_x.saturating_sub(2).saturating_sub(name_x);
    let mut cols: Vec<(u16, u16, String, bool)> = vec![(body.x, 2, t!("sta.col_rank").to_string(), false), (name_x, name_w, page.entity.header(), false)];
    if !tracks {
        cols.push((tracks_x, tracks_w, t!("sta.col_tracks").to_string(), false));
    }
    cols.push((share_x, SHARE_W as u16, t!("sta.col_share").to_string(), false));
    cols.push((plays_x, plays_w, t!("sta.col_plays").to_string(), true));
    cols.push((time_x, time_w, t!("sta.col_time").to_string(), false));
    if tracks {
        cols.push((last_x, last_w, t!("sta.col_last").to_string(), false));
    }
    y = table_head(frame, line(y), &cols);
    if page.top.is_empty() {
        if y < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.nothing_here").to_string(), dim())), line(y));
        }
        return;
    }
    let rows_rect = Rect { x: body.x, y, width: body.width, height: body.bottom().saturating_sub(y) };
    if rows_rect.height == 0 {
        return;
    }
    let metric = page.metric;
    let top_value = page.top.iter().map(|i| metric.of(i)).max().unwrap_or(0).max(1);
    let now = unix_now();
    let items: Vec<(String, Option<String>, String, u64, String, String)> = page
        .top
        .iter()
        .map(|item| {
            let (name, via) = if tracks {
                let (title, artist, _, _) = track_words(item.track.as_ref());
                (join_artist(&artist, &title), (item.origin.as_deref() == Some("peer")).then(|| via_words(item.peer_name.as_deref())))
            } else {
                (item.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| t!("sta.unknown").to_string()), None)
            };
            let extra = if tracks {
                item.last_played.as_deref().and_then(iso_unix).map(|t| day_label(t, now, page.offset_at(t), page.offset_at(now))).unwrap_or_default()
            } else {
                tracks_words(item.tracks.unwrap_or(0))
            };
            let filled = ((metric.of(item) * SHARE_W as u64 + top_value / 2) / top_value) as usize;
            (printable(&name, TEXT_MAX), via, extra, item.plays, fmt_duration(item.listened_ms), g("▰", "■").repeat(filled) + &g("▱", "·").repeat(SHARE_W - filled))
        })
        .collect();
    table_rows(frame, page, rows_rect, items.len(), |frame, page, i, rect, selected, hovered| {
        let (name, via, extra, plays, time, share) = &items[i];
        let _ = page;
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(Paragraph::new(Span::styled(format!("{:>2}", i + 1), cell_style(selected, hovered, dim()))), cell(body.x, 2));
        let via_w = via.as_ref().map_or(0, |v| v.chars().count() as u16);
        let title_w = name_w.saturating_sub(via_w);
        let title = clip(name, title_w);
        let title_len = title.chars().count() as u16;
        frame.render_widget(
            Paragraph::new(Span::styled(title, if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(name_x, title_w),
        );
        if let Some(via) = via {
            frame.render_widget(Paragraph::new(Span::styled(via.clone(), cell_style(selected, hovered, dim()))), cell(name_x + title_len, via_w));
        }
        if !tracks {
            frame.render_widget(Paragraph::new(Span::styled(clip(extra, tracks_w), cell_style(selected, hovered, dim()))), cell(tracks_x, tracks_w));
        }
        frame.render_widget(Paragraph::new(Span::styled(share.clone(), cell_style(selected, hovered, Style::default().fg(th().accent)))), cell(share_x, SHARE_W as u16));
        frame.render_widget(Paragraph::new(Span::styled(fmt_count(*plays), base)).alignment(Alignment::Right), cell(plays_x, plays_w));
        frame.render_widget(Paragraph::new(Span::styled(clip(time, time_w), cell_style(selected, hovered, dim()))), cell(time_x, time_w));
        if tracks {
            frame.render_widget(Paragraph::new(Span::styled(clip(extra, last_w), cell_style(selected, hovered, dim()))), cell(last_x, last_w));
        }
    });
}

// ── Recent ────────────────────────────────────────────────────────────────

fn draw_recent(frame: &mut Frame, page: &mut Page, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    if page.no_api || page.data.is_none() {
        return;
    }
    let right = body.right();
    let client_w = 14;
    let client_x = right.saturating_sub(client_w);
    let listened_w = 12;
    let listened_x = client_x.saturating_sub(2 + listened_w);
    let outcome_w = 17;
    let outcome_x = listened_x.saturating_sub(2 + outcome_w);
    let time_x = body.x;
    let day_x = body.x + 7;
    let day_w = 12;
    let track_x = day_x + day_w + 2;
    let track_w = outcome_x.saturating_sub(2).saturating_sub(track_x);
    let cols = [
        (time_x, 5, t!("sta.col_when").to_string(), false),
        (day_x, day_w, t!("sta.col_day").to_string(), false),
        (track_x, track_w, t!("sta.col_track").to_string(), false),
        (outcome_x, outcome_w, t!("sta.col_outcome").to_string(), false),
        (listened_x, listened_w, t!("sta.col_listened").to_string(), false),
        (client_x, client_w, t!("sta.col_client").to_string(), false),
    ];
    let y = table_head(frame, line(body.y), &cols);
    if page.history.is_empty() {
        if y < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.no_rows_recent").to_string(), dim())), line(y));
        }
        return;
    }
    let rows_rect = Rect { x: body.x, y, width: body.width, height: body.bottom().saturating_sub(y) };
    if rows_rect.height == 0 {
        return;
    }
    // Only the rows on screen are put in words: the log grows fifty plays
    // at a time as the cursor walks it, and a row is some forty strings
    // (performance audit #104).
    let now = unix_now();
    table_rows(frame, page, rows_rect, page.history.len(), |frame, page, i, rect, selected, hovered| {
        let r = &recent_row(page, &page.history[i], now);
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let dimmed = cell_style(selected, hovered, dim());
        frame.render_widget(Paragraph::new(Span::styled(r.time.clone(), base)), cell(time_x, 5));
        frame.render_widget(Paragraph::new(Span::styled(clip(&r.day, day_w), dimmed)), cell(day_x, day_w));
        let via_w = r.via.as_ref().map_or(0, |v| v.chars().count() as u16);
        let title_w = track_w.saturating_sub(via_w);
        let title = clip(&r.track, title_w);
        let title_len = title.chars().count() as u16;
        frame.render_widget(
            Paragraph::new(Span::styled(title, if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(track_x, title_w),
        );
        if let Some(via) = &r.via {
            frame.render_widget(Paragraph::new(Span::styled(via.clone(), dimmed)), cell(track_x + title_len, via_w));
        }
        let outcome_style = if r.completed { Style::default().fg(th().ok) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(clip(&r.outcome, outcome_w), cell_style(selected, hovered, outcome_style))), cell(outcome_x, outcome_w));
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&r.listened, listened_w), cell_style(selected, hovered, if r.counted { Style::default() } else { dim() }))),
            cell(listened_x, listened_w),
        );
        let client = if !r.counted {
            t!("sta.not_counted").to_string()
        } else if r.client.chars().count() as u16 <= client_w {
            r.client.clone()
        } else {
            r.client_short.clone()
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&client, client_w), dimmed)), cell(client_x, client_w));
    });
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The period list, dropped under the control (stats-screen contract,
/// clause 14): every period the log has data in, then All time, the
/// current one wearing the `•`, the cursor on the slab; the first play's
/// date closes the list. A click on a row picks it; a click anywhere else
/// closes the list, as Esc does. Without a control drawn this frame the
/// list hangs from the tab row's left.
fn draw_period_list(frame: &mut Frame, page: &mut Page, area: Rect, cursor: usize) {
    let options = page.choices();
    let foot = page
        .data
        .as_ref()
        .and_then(|d| d.periods.as_ref())
        .and_then(|p| p.earliest.as_deref())
        .and_then(iso_unix)
        .map(|t| t!("sta.period_foot", date = date_long(t, page.offset_at(t))).to_string());
    // Anywhere outside the rows closes the list; the rows, registered
    // after this, win the pointer.
    page.cells.ui.click(area, Act::PeriodClose);

    let anchor = page.cells.period_at.unwrap_or(Rect { x: area.x + 2, y: area.y + if page.hosted { 1 } else { 2 }, width: 0, height: 1 });
    // In cells, as period_control above it now measures the name: counted
    // by characters a Japanese period's list was too narrow for its rows.
    let widest = options
        .iter()
        .map(|o| kit::width(&o.name) + o.detail.as_ref().map_or(0, |d| kit::width(d) + 2))
        .chain(foot.iter().map(|f| kit::width(f)))
        .max()
        .unwrap_or(10);
    // The frame, the marker's cell and a cell of padding each side.
    let width = ((widest + 6) as u16).clamp(24, 46).min(area.width.saturating_sub(4));
    let y = anchor.bottom();
    let room = area.bottom().saturating_sub(1).saturating_sub(y);
    let visible = options.len().min(room.saturating_sub(2 + u16::from(foot.is_some())).max(1) as usize);
    let rect = Rect {
        x: anchor.x.min(area.right().saturating_sub(width + 2)),
        y,
        width,
        height: visible as u16 + 2 + u16::from(foot.is_some()),
    };
    page.cells.ui.overlay(rect);
    let inner = kit::frame_at(frame, rect, th().accent);

    let (first, visible) = kit::table_view(options.len(), Some(cursor), cursor.saturating_sub(visible.saturating_sub(1)), visible);
    for (row, i) in (first..first + visible).enumerate() {
        let rect = Rect { x: inner.x, y: inner.y + row as u16, width: inner.width, height: 1 };
        let on = i == cursor;
        let hover = !on && page.cells.ui.hovers(rect);
        let base = if on {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hover {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        if on {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(rect.width as usize), base)), rect);
        }
        let option = &options[i];
        let mark = if option.is(&page.period.period, page.period.offset) { g("•", "*") } else { " " };
        let mut spans = vec![
            Span::styled(format!(" {mark} "), if on { base } else { Style::default().fg(th().accent) }),
            Span::styled(option.name.clone(), if on || hover { base.add_modifier(Modifier::BOLD) } else { base }),
        ];
        if let Some(detail) = &option.detail {
            spans.push(Span::styled(format!("  {detail}"), if on { base } else { dim() }));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
        page.cells.ui.click(rect, Act::PeriodPick(i));
    }
    if let Some(foot) = foot {
        frame.render_widget(
            Paragraph::new(Span::styled(foot, dim())),
            Rect { x: inner.x + 3, y: inner.y + visible as u16, width: inner.width.saturating_sub(3), height: 1 },
        );
    }
}

/// `x`: the forget gate, gold because it is destructive — the play in
/// words, the one fact, Keep (Enter, Esc) and Forget (y).
fn draw_forget(frame: &mut Frame, page: &mut Page, area: Rect, row: usize) {
    let words = page.history.get(row).map(|item| {
        let r = recent_row(page, item, unix_now());
        let mut w = t!("sta.row_recent", track = r.track, day = r.day, time = r.time, outcome = r.outcome).to_string();
        if !r.completed && item.duration_ms.is_some_and(|d| d > 0) {
            w.push_str(&t!("sta.row_recent_of", total = fmt_clock(item.duration_ms.unwrap_or(0))));
        }
        w
    });
    let inner = kit::modal_frame(frame, area, 70, 10, th().gold);
    let gold = Style::default().fg(th().gold);
    let lines = vec![
        Line::from(Span::styled(t!("sta.forget_title").to_string(), gold.add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::raw(words.unwrap_or_default())),
        Line::from(""),
        Line::from(Span::styled(t!("sta.forget_fact").to_string(), gold)),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) },
    );
    let y = inner.bottom().saturating_sub(1);
    let keep = t!("sta.forget_keep").to_string();
    let go = t!("sta.forget_go").to_string();
    let go_w = kit::width(&go) as u16 + 4;
    let keep_w = kit::width(&keep) as u16 + 4;
    let keep_x = inner.right().saturating_sub(go_w + 2 + keep_w);
    let keep_rect = kit::button(frame, &mut page.cells.ui, Rect { x: keep_x, y, width: inner.width, height: 1 }, &keep, true, Act::ForgetCancel);
    kit::button(frame, &mut page.cells.ui, Rect { x: keep_rect.right() + 2, y, width: inner.width, height: 1 }, &go, false, Act::ForgetConfirm);
}

// ── Tables ────────────────────────────────────────────────────────────────

/// The header words at their columns, the full-width rule beneath.
/// Returns the first row's y.
fn table_head(frame: &mut Frame, at: Rect, cols: &[(u16, u16, String, bool)]) -> u16 {
    for (x, w, word, right) in cols {
        let mut p = Paragraph::new(Span::styled(clip(word, *w), dim()));
        if *right {
            p = p.alignment(Alignment::Right);
        }
        frame.render_widget(p, Rect { x: *x, y: at.y, width: *w, height: 1 });
    }
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(at.width as usize), dim())), Rect { y: at.y + 1, ..at });
    at.y + 2
}

fn table_rows(
    frame: &mut Frame,
    page: &mut Page,
    at: Rect,
    len: usize,
    mut draw_row: impl FnMut(&mut Frame, &mut Page, usize, Rect, bool, bool),
) {
    let avail = at.height as usize;
    let sel_moved = page.sel != page.cells.sel_anchor;
    page.cells.sel_anchor = page.sel;
    let reveal = if sel_moved { page.sel } else { None };
    let (first, visible) = kit::table_view(len, reveal, page.tscroll, avail);
    page.tscroll = first;
    for (row, i) in (first..first + visible).enumerate() {
        let rect = Rect { x: at.x, y: at.y + row as u16, width: at.width, height: 1 };
        let selected = page.sel == Some(i);
        let hovered = !selected && page.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        if selected {
            frame.render_widget(
                Paragraph::new(Span::styled(" ".repeat(at.width as usize), Style::default().fg(th().on_accent).bg(th().accent))),
                rect,
            );
        }
        draw_row(frame, page, i, rect, selected, hovered);
        page.cells.ui.click(rect, Act::Select(i));
    }
    let bar = Rect { x: at.x + at.width, y: at.y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut page.cells.ui, bar, len, visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);
}

fn cell_style(selected: bool, hovered: bool, own: Style) -> Style {
    if selected {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if hovered {
        Style::default().fg(th().bright)
    } else {
        own
    }
}

fn clip(s: &str, w: u16) -> String {
    let w = w as usize;
    if s.chars().count() <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('…');
        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::tests::*;
    use super::*;
    use crate::api::types::OriginSlice;

    #[test]
    fn hosted_the_page_draws_no_header_and_no_tips_and_puts_its_note_last() {
        // The GUI's Stats screen (stats-screen contract, clause 1): the top
        // bar is the header and the footer carries the hint, so the page
        // draws neither; its note keeps the area's last row.
        let _en = english();
        let mut p = ready().hosted();
        press(&mut p, KeyCode::Right);
        assert_eq!(p.tab, Tab::Top);
        press(&mut p, KeyCode::Down);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let area = Rect { x: 0, y: 1, width: 100, height: 28 };
        terminal.draw(|frame| render_hosted(frame, &mut p, area)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let row = |y: u16| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>();
        assert!(row(0).trim().is_empty() && row(1).trim().is_empty(), "no header, and the row under the host's bar stays blank");
        assert!(row(2).contains(" Overview ") && row(2).contains(" Top "), "the tabs are the body's first row: {}", row(2));
        assert!(row(2).trim_end().ends_with("This month ▾ · by plays, or by time on m"), "the Top tab names the period at the row's right: {}", row(2));
        assert!(row(28).contains(" · "), "the cursor row's words take the area's last row: {}", row(28));
        assert!(row(29).trim().is_empty(), "no tips row of its own: {}", row(29));
        assert!(!footer_hint(&p).is_empty(), "the host asks for the hint instead");
    }

    #[test]
    fn a_chart_reads_on_a_whole_number_scale_and_grows_with_the_screen() {
        // stats-screen contract, clause 8: once the tallest column reaches
        // the rows' count every row is the same whole number of plays…
        assert_eq!(scale(4, 4), (4, vec![Some(4), Some(3), Some(2), Some(1)]));
        assert_eq!(scale(37, 4), (40, vec![Some(40), Some(30), Some(20), Some(10)]));
        assert_eq!(scale(5, 4), (8, vec![Some(8), Some(6), Some(4), Some(2)]));
        assert_eq!(scale(41, 8).0, 48);
        // …below it the tallest column takes the whole height and only the
        // whole numbers are labelled.
        assert_eq!(scale(2, 4), (2, vec![Some(2), None, Some(1), None]));
        assert_eq!(scale(3, 4), (3, vec![Some(3), None, None, None]));
        assert_eq!(scale(0, 4), (1, vec![Some(1), None, None, None]));
        // The gutter: the widest label, a space, the axis, a space.
        assert_eq!(gutter(4, 4), 4);
        assert_eq!(gutter(37, 4), 5);
        assert_eq!(gutter(9_999, 4), 9, "10,000 with its comma");
        // Clause 10: the rows under the tiles, shared by two charts, three
        // to eight.
        assert_eq!(chart_rows(6), 3);
        assert_eq!(chart_rows(12), 3);
        assert_eq!(chart_rows(14), 4);
        assert_eq!(chart_rows(18), 6);
        assert_eq!(chart_rows(22), 8);
        assert_eq!(chart_rows(90), 8);
    }

    #[test]
    fn the_charts_wear_their_axes_and_take_the_rows_the_screen_has() {
        // stats-screen contract, clauses 8–11: whole-number ticks down the
        // left, `0 ┼` at the foot, a baseline with a tick under every
        // label, the labels under their ticks, no gridlines; three rows of
        // columns on a 30-row terminal, five on 34, eight on 46 and on 70.
        let _en = english();
        let mut p = ready();
        for (height, rows) in [(30u16, 3usize), (34, 5), (46, 8), (70, 8)] {
            let frame = draw_at(&mut p, 100, height);
            let lines: Vec<&str> = frame.lines().collect();
            let title = lines.iter().position(|l| l.contains("PLAYS PER DAY")).unwrap_or_else(|| panic!("no day chart at {height} rows:\n{frame}"));
            let base = lines.iter().position(|l| l.contains(" ┼─")).unwrap_or_else(|| panic!("no baseline at {height} rows:\n{frame}"));
            assert_eq!(base - title - 1, rows, "rows of columns at {height} rows:\n{frame}");
            let (top, ticks) = scale(41, rows as u16);
            assert!(lines[title + 1].starts_with(&format!("  {top} ┤ ")), "the top row carries the scale's top:\n{frame}");
            for (r, tick) in ticks.iter().enumerate() {
                let want = match tick {
                    Some(v) => format!("{v} ┤ "),
                    None => "   │ ".to_string(),
                };
                assert!(lines[title + 1 + r].contains(&want), "row {r} at {height} rows:\n{frame}");
            }
            assert!(lines[base].starts_with("   0 ┼─┴"), "0 at the foot, the first day's tick right after:\n{frame}");
            // Every tick has a label starting under it, and no label starts
            // anywhere else.
            let ticks_at: Vec<usize> = lines[base].chars().enumerate().filter(|(_, c)| *c == '┴').map(|(x, _)| x).collect();
            let under: Vec<char> = lines[base + 1].chars().collect();
            let starts: Vec<usize> = (0..under.len()).filter(|&i| under[i].is_ascii_digit() && (i == 0 || under[i - 1] == ' ')).collect();
            assert!(ticks_at.len() >= 6, "{frame}");
            assert_eq!(starts, ticks_at, "labels under their ticks at {height} rows:\n{frame}");
            assert!(lines[base + 1].trim().starts_with('1') && lines[base + 1].trim().ends_with("30"), "the first and last day:\n{frame}");
            // The hours chart: the same furniture, the same rows.
            let hours = lines.iter().position(|l| l.contains("WHEN YOU LISTEN")).unwrap_or_else(|| panic!("no hours chart at {height} rows:\n{frame}"));
            assert!(lines[hours + 1].starts_with(&format!("  {} ┤ ", scale(47, rows as u16).0)), "{frame}");
            assert!(lines[hours + 1 + rows].starts_with("   0 ┼─┴") && lines[hours + 2 + rows].trim_start().starts_with("0 "), "{frame}");
            assert!(lines[hours + 2 + rows].contains("23"), "the last hour:\n{frame}");
            assert!(!frame.contains('┈') && !frame.contains('╌'), "no gridlines:\n{frame}");
        }
        // Short terminals, down to the page's 24-row floor: the cards and
        // the day chart on three rows, and the hours chart yields — the
        // state line's two rows went to the body (clause 15).
        for height in [26u16, 24] {
            let small = draw_at(&mut p, 80, height);
            assert_eq!(small.matches('╭').count(), 6, "{small}");
            assert!(small.contains("PLAYS PER DAY") && small.contains("   0 ┼─┴"), "{small}");
            assert!(!small.contains("WHEN YOU LISTEN"), "{small}");
        }
    }

    fn draw(page: &mut Page) -> String {
        draw_at(page, 100, 30)
    }

    fn draw_at(page: &mut Page, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, page)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn row(frame: &str, needle: &str) -> String {
        frame.lines().find(|l| l.contains(needle)).map(|l| l.to_string()).unwrap_or_else(|| panic!("no row with {needle:?}:\n{frame}"))
    }

    /// Every tab label draws whole in Japanese at 100×30, its click rect
    /// as wide as it in cells, and each tab starts past the one before.
    /// Counted by characters ' 概要 ' got a 4-cell rect for 6 cells of text,
    /// so it drew as ' 概 ', the next tab started two cells into it, and
    /// each click covered only what was left. Drawn under the process-wide
    /// locale (the room reads its labels through `t!`), held for the test.
    #[test]
    fn every_tab_label_draws_whole_in_japanese() {
        let Some(_) = crate::setup::tests::in_own_process(&["ja"]) else { return };
        let mut p = ready();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut p)).unwrap();
        let tabs: Vec<(Tab, String)> = Tab::ALL.iter().map(|t| (*t, format!(" {} ", t.name()))).collect();
        let buffer = terminal.backend().buffer().clone();
        let mut end = 0;
        for (tab, label) in tabs {
            let rect = p.cells.ui.clicks.iter().find(|(_, a)| *a == Act::Tab(tab)).map(|(r, _)| *r).unwrap_or_else(|| panic!("{label:?}"));
            assert_eq!(crate::setup::tests::cells_text(&buffer, rect.y, rect.x, rect.right()), label, "drawn whole");
            assert_eq!(usize::from(rect.width), kit::width(&label), "{label:?}: its click covers it");
            assert!(rect.x >= end, "{label:?} starts past the tab before it");
            end = rect.right();
        }
    }

    /// The radio rows (ORIGIN, SHOW, RANK BY) measure their options in
    /// cells: the Japanese origin labels, drawn with their glyphs in a
    /// 40-cell row, each get a click rect of their width in cells and the
    /// glyph's four, three cells apart, show whole, and the x returned is
    /// past the last, which is what the callers' `need` counts. Drawn
    /// through `radio_row` with the labels passed in, so no test switches
    /// the process-wide locale.
    #[test]
    fn the_radio_rows_measure_japanese_options_in_cells() {
        crate::kit::theme::pin_modern_terminal();
        let options: Vec<String> = ["すべて", "このサーバー", "ピア"].iter().map(|s| s.to_string()).collect();
        let mut p = page();
        let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
        let mut end = 0;
        terminal
            .draw(|frame| end = radio_row(frame, &mut p, Rect { x: 0, y: 0, width: 40, height: 1 }, &options, 1, Act::Origin))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut x = 0;
        for (i, name) in options.iter().enumerate() {
            let act = Act::Origin(i);
            let rect = p.cells.ui.clicks.iter().find(|(_, a)| *a == act).map(|(r, _)| *r).unwrap_or_else(|| panic!("option {i}"));
            assert_eq!((rect.x, rect.width), (x, kit::width(name) as u16 + 4), "option {i}: its cells");
            let mut text = String::new();
            let mut cx = rect.x;
            while cx < rect.right() {
                let symbol = buffer[(cx, 0)].symbol();
                text.push_str(symbol);
                cx += (kit::width(symbol) as u16).max(1);
            }
            let glyph = if i == 1 { g("(•)", "(*)") } else { "( )" };
            assert_eq!(text, format!("{glyph} {name}"), "option {i}: whole");
            x = rect.right() + 3;
        }
        assert_eq!(end, x, "the x past the row");
        let need: usize = options.iter().map(|o| kit::width(o) + 7).sum();
        assert_eq!(usize::from(end), need, "what the callers right-anchor the row by");
    }

    #[test]
    fn boots_loading_then_draws_the_overview_with_its_tiles_and_charts() {
        let _en = english();
        let mut p = start(Client::new("http://home.mstream.example:3000").expect("client"), Some("anna".into()));
        assert!(matches!(p.queued, Some(Op::Load { .. })), "the first load is queued");
        assert_eq!(p.busy.as_deref(), Some("loading your listening…"));
        p.zone = None;
        p.queued = None;
        p.busy = None;
        let frame = draw(&mut p);
        assert!(frame.contains("Stats") && frame.contains("home.mstream.example · anna"), "{frame}");
        assert!(frame.contains(" Overview ") && !frame.contains("PERIOD"), "the tabs, and nothing else, before the load lands:\n{frame}");

        let seq = p.seq;
        p.apply(Done::Loaded { seq, result: Ok(loaded(summary(388, 425), Some(periods()))) });
        let frame = draw_at(&mut p, 100, 32);
        // No state line (stats-screen contract, clause 15): the tabs are the
        // body's first row, the Overview's tab row ends with its tabs, and
        // the period is a dropdown control (clause 14).
        assert!(!frame.contains("388 plays ·") && !frame.contains("the period's totals"), "{frame}");
        let rows: Vec<&str> = frame.lines().collect();
        assert!(rows[1].trim().is_empty() && rows[2].contains(" Overview ") && rows[2].contains(" Top ") && rows[2].trim_end().ends_with("Recent"), "{}", rows[2]);
        assert!(frame.contains("PERIOD  This month ▾") && !frame.contains('‹') && !frame.contains("p list"), "{frame}");
        assert!(frame.contains("ORIGIN  (•) all   ( ) this server   ( ) peers"), "the log holds peer plays:\n{frame}");
        // The six tiles and their details, against August by its own name.
        let tiles = [("388", "plays", "+12% vs August 2026"), ("31h 12m", "listening time", "+1h 12m vs August 2026"), ("188", "tracks", "14% of the library"),
            ("37", "skips", "8% of starts"), ("6 days", "streak", "longest 11"), ("42", "sessions", "about 44 min each")];
        for (value, label, detail) in tiles {
            assert!(frame.contains(value) && frame.contains(label) && frame.contains(detail), "tile {label}:\n{frame}");
        }
        // The tiles are cards (stats-screen contract, clause 13): six
        // rounded frames, three across filling the column edge to edge,
        // the two rows stacked frame to frame, the values a cell in.
        assert_eq!(frame.matches('╭').count(), 6, "{frame}");
        let lids: Vec<&str> = frame.lines().filter(|l| l.contains('╭')).collect();
        let lid = lids[0].trim_end();
        assert!(lid.starts_with("  ╭") && lid.ends_with('╮') && lid.chars().count() == 98, "the cards fill the column: {lid:?}");
        assert!(lid.matches('╮').count() == 3 && lid.contains("╮ ╭"), "one cell between the cards: {lid:?}");
        let values = row(&frame, "│ 388 ");
        assert!(values.contains("│ 31h 12m ") && values.contains("│ 188 ") && values.trim_end().ends_with('│'), "{values}");
        assert!(row(&frame, "│ 37 ").contains("│ 6 days ") && row(&frame, "│ 37 ").contains("│ 42 "), "{frame}");
        let lines: Vec<&str> = frame.lines().collect();
        let first_lid = lines.iter().position(|l| l.contains('╭')).unwrap();
        assert!(lines[first_lid - 1].contains("PERIOD"), "the top frame sits under the controls:\n{frame}");
        assert!(lines[first_lid + 4].contains('╰') && lines[first_lid + 5].contains('╭') && lines[first_lid + 9].contains('╰'), "{frame}");
        assert!(lines[first_lid + 10].contains("PLAYS PER DAY"), "the chart follows the second row of cards:\n{frame}");
        assert!(row(&frame, "PLAYS PER DAY").contains("most on Tue 8 · 41 plays, 2h 10m"), "{frame}");
        assert!(frame.contains("WHEN YOU LISTEN  most around 21:00, Sundays"), "{frame}");
        assert!(frame.contains("WHERE THE TRACKS LIVE"), "{frame}");
        // At 100 columns the split beside the hours chart is short of its
        // ten cells and keeps its words on a shorter bar (stats-screen
        // contract, clause 12); ten columns wider, the bar is whole.
        assert!(row(&frame, "this server   ▰").contains("▰▰▰▰▰  374 plays · 28h 40m"), "{frame}");
        assert!(row(&frame, "peers’ tracks").contains("▱▱▱▱▱  38 plays · 2h 32m"), "{frame}");
        let wide = draw_at(&mut p, 110, 32);
        assert!(row(&wide, "this server   ▰").contains("▰▰▰▰▰▰▰▰▰▱  374 plays · 28h 40m"), "{wide}");
        assert!(row(&wide, "peers’ tracks").contains("▰▱▱▱▱▱▱▱▱▱  38 plays · 2h 32m"), "{wide}");
        assert!(frame.contains("█"), "the tallest day is a full column:\n{frame}");
        assert!(frame.contains("←→ tab · [ ] period · p pick a period · o origin · q quit"), "{frame}");
        // Every period the fixture names, in the webapp's order, then All time.
        let names: Vec<&str> = p.options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["This week", "Last week", "This month", "Last month", "July 2026", "This quarter", "Last quarter", "This half-year", "This year", "Last year", "All time"]);
        assert_eq!(p.options[0].detail.as_deref(), Some("Week of 2026-09-07"));
        assert_eq!(p.options[2].detail.as_deref(), Some("September 2026"));
    }

    #[test]
    fn the_period_steps_on_brackets_and_a_change_keeps_the_page_until_the_new_numbers_land() {
        let _en = english();
        let mut p = ready();
        let seq = p.seq;
        press(&mut p, KeyCode::Char('['));
        assert_eq!(p.period.name, "Last week");
        assert!(matches!(&p.queued, Some(Op::Load { seq: s, period, .. }) if *s > seq && period.period == "week" && period.offset == -1), "{:?}", p.queued);
        assert_eq!(p.busy.as_deref(), Some("loading your listening…"));
        // stats-screen contract, clause 16: the page keeps this month's
        // numbers under the new name while the read is out — nothing
        // blinks — and the tiles still compare against the SHOWN period's
        // predecessor; only the log's paging cursor and the row cursor go.
        assert!(p.data.is_some() && !p.history.is_empty() && !p.top.is_empty(), "the old numbers stand");
        assert!(p.next.is_none() && p.sel.is_none());
        assert_eq!(p.shown.name, "This month");
        let frame = draw(&mut p);
        assert!(frame.contains("PERIOD  Last week ▾"), "{frame}");
        assert!(row(&frame, "│ 388 ").contains("│ 31h 12m "), "the tiles keep their numbers:\n{frame}");
        assert!(frame.contains("+12% vs August 2026"), "{frame}");
        assert!(frame.contains("loading your listening…"), "{frame}");

        // An answer to the older question is dropped; the live one lands.
        p.apply(Done::Loaded { seq, result: Ok(loaded(summary(1, 1), Some(periods()))) });
        assert_eq!(p.shown.name, "This month");
        assert!(row(&draw(&mut p), "│ 388 ").contains("│ 31h 12m "));
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(summary(120, 130), Some(periods()))) });
        assert_eq!(p.shown.name, "Last week");
        let frame = draw(&mut p);
        assert!(frame.contains("│ 120 ") && frame.contains("vs last week"), "{frame}");
        assert!(!frame.contains("loading your listening…"), "{frame}");

        // p drops the list under the control (clause 14): the current
        // period wears the •, the cursor starts on it, the first play's
        // date closes the list; no title bar, no [X]. Esc keeps, Enter
        // picks, p again closes.
        press(&mut p, KeyCode::Char('p'));
        assert_eq!(p.modal, Modal::Period(1));
        let frame = draw(&mut p);
        let lines: Vec<&str> = frame.lines().collect();
        let ctl = lines.iter().position(|l| l.contains("PERIOD  Last week ▾")).unwrap();
        let name_x = lines[ctl].find("Last week").unwrap();
        assert_eq!(lines[ctl + 1].chars().nth(name_x), Some('╭'), "the list hangs from the name:\n{frame}");
        assert!(lines[ctl + 2].contains("   This week  Week of 2026-09-07"), "{frame}");
        assert!(lines[ctl + 3].contains(" • Last week  Week of 2026-08-31"), "{frame}");
        assert!(!frame.contains("[X]"), "{frame}");
        assert!(frame.contains("first play 3 March 2026"), "{frame}");
        assert!(frame.contains("↑↓ pick · Enter choose · Esc keep Last week"), "{frame}");
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Esc);
        assert_eq!(p.modal, Modal::None);
        assert_eq!(p.period.name, "Last week", "Esc keeps");
        press(&mut p, KeyCode::Char('p'));
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Enter);
        assert_eq!(p.modal, Modal::None);
        assert_eq!(p.period.name, "Last month");
        assert!(matches!(&p.queued, Some(Op::Load { period, .. }) if period.is("month", -1)));
        press(&mut p, KeyCode::Char('p'));
        assert!(matches!(p.modal, Modal::Period(_)));
        press(&mut p, KeyCode::Char('p'));
        assert_eq!(p.modal, Modal::None);
        // The last option is All time; stepping past it stays there.
        for _ in 0..20 {
            press(&mut p, KeyCode::Char(']'));
        }
        assert_eq!(p.period.name, "All time");
        assert!(matches!(&p.queued, Some(Op::Load { period, .. }) if period.period == "all"));
    }

    #[test]
    fn the_period_control_and_its_list_answer_the_pointer_on_every_tab() {
        // stats-screen contract, clause 14: a click on the name drops the
        // list; a row picks; anywhere else closes. On the Top and Recent
        // tabs the name leads the tab row's note and is the same control.
        let _en = english();
        let mut p = ready();
        let frame = draw(&mut p);
        let lines: Vec<&str> = frame.lines().collect();
        let ctl_y = lines.iter().position(|l| l.contains("PERIOD  This month ▾")).unwrap() as u16;
        let name_x = lines[ctl_y as usize].find("This month").unwrap() as u16;
        assert_eq!(p.cells.ui.hit(Position { x: name_x + 3, y: ctl_y }), Some(Act::PeriodList));
        p.act(Act::PeriodList);
        assert_eq!(p.modal, Modal::Period(2));
        let frame = draw(&mut p);
        let lines: Vec<&str> = frame.lines().collect();
        assert_eq!(lines[ctl_y as usize + 1].chars().nth(name_x as usize), Some('╭'), "the list hangs from the name:\n{frame}");
        let row_y = lines.iter().position(|l| l.contains("Last month  August 2026")).unwrap() as u16;
        assert_eq!(p.cells.ui.hit(Position { x: name_x + 5, y: row_y }), Some(Act::PeriodPick(3)));
        assert_eq!(p.cells.ui.hit(Position { x: 90, y: row_y }), Some(Act::PeriodClose), "beside the list the page only closes it");
        p.act(Act::PeriodPick(3));
        assert_eq!(p.modal, Modal::None);
        assert_eq!(p.period.name, "Last month");
        assert!(matches!(&p.queued, Some(Op::Load { period, .. }) if period.is("month", -1)));

        // The Top tab: the name leads the note at the tab row's right, the
        // list drops from it and keeps inside the column.
        p.queued = None;
        p.busy = None;
        press(&mut p, KeyCode::Right);
        let frame = draw(&mut p);
        let tabs = row(&frame, " Overview ");
        assert!(tabs.trim_end().ends_with("Last month ▾ · by plays, or by time on m"), "{tabs}");
        let x = tabs.find("Last month ▾").unwrap() as u16;
        assert_eq!(p.cells.ui.hit(Position { x: x + 2, y: 2 }), Some(Act::PeriodList));
        p.act(Act::PeriodList);
        let frame = draw(&mut p);
        let lines: Vec<&str> = frame.lines().collect();
        let lid = lines[3].trim_end();
        assert!(lid.contains('╭') && lid.ends_with('╮'), "the list drops under the tab row:\n{frame}");
        assert!(lid.find('╭').unwrap() <= x as usize && lid.chars().count() <= 98, "{frame}");
        assert!(frame.contains(" • Last month  August 2026"), "{frame}");
        p.act(Act::PeriodClose);

        // The Recent tab, on a machine without a zone, says so after its note.
        press(&mut p, KeyCode::Right);
        let frame = draw(&mut p);
        assert!(row(&frame, " Overview ").trim_end().ends_with("Last month ▾ · the log, newest first · times in UTC"), "{frame}");
    }

    #[test]
    fn the_origin_cycles_on_o_only_while_the_log_holds_peer_plays() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Char('o'));
        assert_eq!(p.origin, Origin::Local);
        assert!(matches!(&p.queued, Some(Op::Load { origin: Origin::Local, .. })));
        let live = p.seq;
        let mut only_local = summary(374, 400);
        only_local.origins.peers = OriginSlice::default();
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(only_local, Some(periods()))) });
        let frame = draw(&mut p);
        assert!(frame.contains("(•) this server"), "the control stays while the filter is on:\n{frame}");
        press(&mut p, KeyCode::Char('o'));
        press(&mut p, KeyCode::Char('o'));
        assert_eq!(p.origin, Origin::All);

        let mut p = page();
        p.reload(true);
        p.queued = None;
        let mut no_peers = summary(374, 400);
        no_peers.origins.peers = OriginSlice::default();
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(no_peers, Some(periods()))) });
        let frame = draw(&mut p);
        assert!(!frame.contains("ORIGIN") && !frame.contains("WHERE THE TRACKS LIVE") && !frame.contains("o origin"), "{frame}");
        assert!(press(&mut p, KeyCode::Char('o')).is_none());
        assert_eq!(p.origin, Origin::All);
        assert!(p.queued.is_none());
    }

    #[test]
    fn the_top_tab_ranks_with_a_share_bar_and_reranks_on_t_and_m() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Right);
        assert_eq!(p.tab, Tab::Top);
        let frame = draw(&mut p);
        assert!(frame.contains("SHOW  (•) tracks   ( ) artists   ( ) albums   ( ) genres"), "{frame}");
        assert!(frame.contains("RANK BY  (•) plays   ( ) time"), "{frame}");
        assert!(frame.contains("#   TRACK") && frame.contains("SHARE") && frame.contains("PLAYS") && frame.contains("TIME") && frame.contains("LAST"), "{frame}");
        let first = row(&frame, "Boards of Canada - Roygbiv");
        assert!(first.contains("▰▰▰▰▰▰▰▰▰▰") && first.contains("38") && first.contains("2h 31m") && first.contains("today"), "{first}");
        let peer = row(&frame, "Bassnectar - Rewind The Track");
        assert!(peer.contains(" · via attic") && peer.contains("▰▰▰▰▰▱▱▱▱▱") && peer.contains("1h 24m"), "{peer}");
        assert!(frame.contains("↑↓ select · ←→ tab · t what · m rank by · [ ] period · o origin · q quit"), "{frame}");

        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        let frame = draw(&mut p);
        assert!(frame.contains("Aphex Twin - Xtal · Selected Ambient Works 85-92 (1992) · 31 plays · 2h 32m · last played today"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · t what · m rank by · [ ] period · o origin · Esc deselect"), "{frame}");

        press(&mut p, KeyCode::Char('m'));
        assert_eq!(p.metric, Metric::Time);
        assert!(matches!(&p.queued, Some(Op::Top { metric: Metric::Time, entity: Entity::Tracks, .. })));
        assert!(p.sel.is_none(), "a re-rank drops the cursor");
        p.queued = None;
        let frame = draw(&mut p);
        let first = row(&frame, "Aphex Twin - Xtal");
        assert!(first.contains("▰▰▰▰▰▰▰▰▰▰"), "the share follows the metric:\n{first}");

        press(&mut p, KeyCode::Char('t'));
        assert_eq!(p.entity, Entity::Artists);
        assert!(matches!(&p.queued, Some(Op::Top { entity: Entity::Artists, metric: Metric::Time, .. })));
        p.queued = None;
        let live = p.seq;
        let artists = StatsTop {
            entity: "artists".into(),
            metric: "time".into(),
            items: vec![top_item(1, 96, 25_320_000, None, Some("Boards of Canada"), None), top_item(2, 61, 17_880_000, None, Some("Aphex Twin"), None)],
        };
        p.apply(Done::Top { seq: live, result: Ok(artists) });
        let frame = draw(&mut p);
        assert!(frame.contains("#   ARTIST") && frame.contains("TRACKS") && !frame.contains("LAST"), "{frame}");
        assert!(row(&frame, "Boards of Canada").contains("14 tracks"), "{frame}");
        press(&mut p, KeyCode::Down);
        assert!(draw(&mut p).contains("Boards of Canada · 14 tracks · 96 plays · 7h 02m"));
    }

    #[test]
    fn the_recent_tab_words_each_play_and_pages_as_the_cursor_nears_the_end() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Left);
        assert_eq!(p.tab, Tab::Recent);
        let frame = draw(&mut p);
        assert!(frame.contains("TIME   DAY") && frame.contains("TRACK") && frame.contains("OUTCOME") && frame.contains("LISTENED") && frame.contains("CLIENT"), "{frame}");
        let done = row(&frame, "completed");
        assert!(done.contains("today") && done.contains("2:31") && done.contains("web player"), "{done}");
        let skipped = row(&frame, "skipped at 0:08");
        assert!(skipped.contains("0:08 of 8:15") && skipped.contains("not counted") && !skipped.contains("this player"), "not counted takes the client's place:\n{skipped}");
        let stopped = row(&frame, "stopped at 5:40");
        assert!(stopped.contains("yesterday") && stopped.contains("5:40 of 9:05") && stopped.contains("mStream Mobile") && !stopped.contains("2.4.1"), "the name alone where the version does not fit:\n{stopped}");
        let legacy = row(&frame, "scrobbled at 0:30");
        assert!(legacy.contains("older client"), "{legacy}");

        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        // Four rows and a cursor near the end: the next page is asked for once.
        assert!(matches!(&p.queued, Some(Op::History { before, .. }) if before == "cursor-2"), "{:?}", p.queued);
        assert!(p.next.is_none());
        assert_eq!(p.busy.as_deref(), Some("loading older plays…"));
        let frame = draw(&mut p);
        assert!(frame.contains("loading older plays…"), "the busy line wins the note line:\n{frame}");
        p.busy = None;
        let frame = draw(&mut p);
        assert!(row(&frame, "Nils Frahm - Says · today").contains("· skipped at 0:08 of 8:15 · not counted · x forgets it"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · x forget this play · [ ] period · o origin · Esc deselect"), "{frame}");
        p.queued = None;
        press(&mut p, KeyCode::Down);
        assert!(p.queued.is_none(), "no second ask while the cursor is taken");
        let live = p.seq;
        let more = StatsHistory { items: vec![play("p5", today0() - 86_400 * 12, "completed", 200_000, Some(200_000), true, Some("mstream-webapp/6.27.0"), None)], next: None };
        p.apply(Done::History { seq: live, result: Ok(more) });
        assert_eq!(p.history.len(), 5);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        assert!(p.queued.is_none(), "the log has no more pages");
    }

    #[test]
    fn the_forget_gate_keeps_on_enter_and_forgets_on_y_then_reloads() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Left);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        p.queued = None;
        p.busy = None;
        press(&mut p, KeyCode::Char('x'));
        assert_eq!(p.modal, Modal::Forget(1));
        let frame = draw(&mut p);
        assert!(frame.contains("Forget this play?"), "{frame}");
        assert!(frame.contains("Nils Frahm - Says · today") && frame.contains("skipped at 0:08 of 8:15"), "{frame}");
        assert!(frame.contains("It leaves the log and the counts. Nothing else changes."), "{frame}");
        assert!(frame.contains("◂ Keep") && frame.contains("  Forget  ") && frame.contains("y forget · Esc keep"), "{frame}");
        press(&mut p, KeyCode::Enter);
        assert_eq!(p.modal, Modal::None, "Enter is the safe choice");
        assert!(p.queued.is_none());

        press(&mut p, KeyCode::Char('x'));
        press(&mut p, KeyCode::Char('y'));
        assert!(matches!(&p.queued, Some(Op::Forget { id, row: 1 }) if id == "p2"));
        assert_eq!(p.busy.as_deref(), Some("forgetting the play…"));
        p.queued = None;
        p.apply(Done::Forgot { id: "p2".into(), row: 1, result: Ok(()) });
        assert_eq!(p.history.len(), 3);
        assert!(p.history.iter().all(|h| h.id != "p2"));
        assert_eq!(note(&p), "the play is forgotten — the log and the totals reload");
        assert!(matches!(&p.queued, Some(Op::Load { .. })), "the totals reload");
        assert!(p.busy.is_none(), "quietly");

        p.queued = None;
        press(&mut p, KeyCode::Char('x'));
        press(&mut p, KeyCode::Char('y'));
        p.apply(Done::Forgot { id: "p3".into(), row: 1, result: Err(ApiError::Server { status: 404, message: "No such play".into() }) });
        assert_eq!(note(&p), "could not forget the play: server error 404: No such play");
        assert_eq!(p.history.len(), 3);
    }

    #[test]
    fn a_log_with_nothing_in_it_and_a_period_with_nothing_in_it_say_so() {
        let _en = english();
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        let empty = StatsPeriods { earliest: None, latest: None, periods: Vec::new() };
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(StatsSummary::default(), Some(empty))) });
        let frame = draw(&mut p);
        assert!(frame.contains("• no plays yet — plays land here as you listen"), "{frame}");
        assert!(frame.contains("The web player reports each track when it ends"), "{frame}");
        assert!(frame.contains("Plays this account reports through this server · times in UTC"), "{frame}");
        assert!(!frame.contains("PERIOD"), "nothing to step through:\n{frame}");
        assert!(frame.contains("←→ tab · q quit"), "{frame}");
        assert_eq!(p.options.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["This month", "All time"]);

        // A period the log covers but nothing was started in.
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        let mut nothing = StatsSummary::default();
        nothing.period.from = "2026-08-01T00:00:00.000Z".into();
        p.period = Period { period: "month".into(), offset: -1, name: "Last month".into(), detail: Some("August 2026".into()) };
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(nothing, Some(periods()))) });
        let frame = draw(&mut p);
        assert!(frame.contains("Nothing in Last month"), "{frame}");
        assert!(frame.contains("Your log begins on 3 March 2026 — [ ] walks the periods, p lists them."), "{frame}");
        assert!(frame.contains("PERIOD  Last month ▾"), "{frame}");

        // The default period with nothing in it, on a log that has data
        // elsewhere: the webapp's move, the first period with data.
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        let only_old = StatsPeriods { earliest: Some("2025-03-03T14:00:00.000Z".into()), latest: None, periods: vec![PeriodOption { period: "year".into(), offset: -1, label: "2025".into(), from: "2025-01-01T00:00:00.000Z".into(), to: String::new() }] };
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(StatsSummary::default(), Some(only_old))) });
        assert_eq!(p.period.name, "Last year");
        assert!(matches!(&p.queued, Some(Op::Load { period, .. }) if period.is("year", -1)));
    }

    #[test]
    fn load_errors_name_the_gate_and_an_old_server_is_told_apart() {
        let _en = english();
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Err(ApiError::NotFound("api/v1/stats/summary".into())) });
        assert!(p.no_api);
        let frame = draw(&mut p);
        assert!(frame.contains("• no Stats API — this server does not have it yet; it arrived in mStream 6.27"), "{frame}");
        assert!(frame.contains("←→ tab · q quit"), "{frame}");

        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Err(ApiError::Forbidden("Forbidden".into())) });
        assert_eq!(note(&p), "this session has no account behind it — the log is per account; sign in with `mstream-player login`");
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Err(ApiError::Network("connection refused".into())) });
        assert_eq!(note(&p), "could not load your listening: could not reach server: connection refused");
        assert!(p.note.as_ref().is_some_and(|(_, err)| *err));
    }

    #[test]
    fn a_deep_log_puts_the_rows_on_screen_in_their_own_words() {
        // A row is put in words as it is drawn, not the whole log every
        // frame (performance audit #104): whatever depth the cursor has
        // walked to, each row on screen is its own play, in order.
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Left);
        assert_eq!(p.tab, Tab::Recent);
        p.history = deep_log(2_000);
        p.next = None;
        p.act(Act::Select(1_500));
        let frame = draw_at(&mut p, 120, 40);
        let at = row(&frame, "Artist 1500 - Title 1500");
        let started = today0() - 3600 * 7 * 1500;
        assert!(at.contains(&time_label(started, 0)) && at.contains(&day_label(started, unix_now(), 0, 0)), "{at}");
        // The table's rows (the note line under them names the cursor's
        // play too, between dots).
        let shown: Vec<usize> = frame
            .lines()
            .filter(|l| !l.contains(" · "))
            .filter_map(|l| l.split("Artist ").nth(1)?.split(' ').next()?.parse().ok())
            .collect();
        assert!(shown.len() > 20 && shown.contains(&1_500), "{frame}");
        assert!(shown.windows(2).all(|w| w[1] == w[0] + 1), "rows in log order: {shown:?}");
        assert!(!frame.contains("Artist 0 - "), "the log's top is scrolled away:\n{frame}");
    }

    /// `cargo test --release recent_tab_frame_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check; run --release with --nocapture"]
    fn recent_tab_frame_cost() {
        let _en = english();
        for n in [50, 2_000, 5_000] {
            let mut p = ready();
            press(&mut p, KeyCode::Left);
            p.history = deep_log(n);
            p.sel = Some(0);
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            let frames = 200;
            let started = std::time::Instant::now();
            for _ in 0..frames {
                terminal.draw(|frame| render(frame, &mut p)).unwrap();
            }
            let per = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            println!("recent tab, {n} plays: {per:.3} ms a frame");
        }
    }
}
