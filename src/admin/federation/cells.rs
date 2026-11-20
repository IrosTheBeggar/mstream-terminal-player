//! The Federation room in character cells: what `mstream-player admin
//! federation` draws in a terminal, and what the GUI player's Admin tab
//! draws on its grid while it hosts the room. The room itself (its state,
//! its worker, its keys, its words) is the parent module; this is its
//! view in cells, and holds what is measured in cells: the state row,
//! the tabs, the three tables, the turn-on card and the modals, on the
//! kit's surface. It is also where the room meets the hub's loop, as a
//! [`Screen`].

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::super::{Screen, body_column, draw_foot, draw_header, fmt_bytes, frame_ground, host_of};
use super::*;
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::g;

const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
const TAB_GAP: u16 = 2;

/// What this view keeps on the room between frames. The room holds one
/// and never looks inside it.
pub(super) struct State {
    /// The kit's interaction surface: what the last frame registered.
    ui: Surface<Act>,
    /// The cursor as the table last saw it: the table scrolls to the
    /// cursor only on the frame it moved.
    sel_anchor: Option<usize>,
}

impl State {
    pub(super) fn new() -> State {
        State { ui: Surface::new(), sel_anchor: None }
    }
}

// ── The hub's view of the room ───────────────────────────────────────────────

impl Screen for Room {
    type Act = Act;

    fn ui(&mut self) -> &mut Surface<Act> {
        &mut self.cells.ui
    }

    fn absorb(&mut self) -> bool {
        Room::absorb(self)
    }

    fn pump(&mut self) {
        Room::pump(self)
    }

    fn awaiting(&self) -> bool {
        Room::awaiting(self)
    }

    fn tick(&mut self) {
        Room::tick(self)
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        Room::key(self, key)
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        Room::act(self, act)
    }

    fn wheel(&mut self, up: bool, _at: Position) {
        if matches!(self.modal, Modal::None) {
            self.tscroll = if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
        }
    }

    fn render_hosted(&mut self, frame: &mut Frame, area: Rect) {
        self.cells.ui.begin_frame();
        draw(frame, self, area, true);
    }

    fn hint(&self) -> String {
        Room::hint(self)
    }

    fn modal_open(&self) -> bool {
        Room::modal_open(self)
    }

    fn claim(&self) -> Claim {
        Room::claim(self)
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.cells.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    draw(frame, room, area, false);
}

/// The room inside `area`. Standalone it owns the window, header and tips
/// row included. Hosted, the host's bar names the server and its footer
/// carries the tips, so the room draws neither and keeps only its note on
/// the area's last row; below its minimum it draws what fits and never
/// asks for a larger window.
fn draw(frame: &mut Frame, room: &mut Room, area: Rect, hosted: bool) {
    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.cells.ui.pointer;
    if modal_open {
        room.cells.ui.pointer = None;
    }

    if !hosted {
        draw_header(frame, area, &t!("fed.title"), &host_of(&room.client));
    }
    let column = body_column(area, hosted);
    match room.params.clone() {
        None => {}
        Some(p) if !p.enabled => draw_off(frame, room, column, &p),
        Some(p) => draw_on(frame, room, column, &p),
    }
    draw_foot(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room), hosted);

    if modal_open {
        room.cells.ui.pointer = live_pointer;
        room.cells.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Form(f) => draw_form(frame, room, area, &f),
        Modal::Minted { name, ticket, libraries } => draw_minted(frame, room, area, &name, ticket.as_deref(), &libraries),
        Modal::PeerName { name, preview, .. } => draw_peer_name(frame, room, area, &name, &preview),
        Modal::Decline(id) => {
            let who = room.request(id).map(request_name).unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("fed.decline_title", name = who).to_string(),
                vec![t!("fed.decline_1").to_string(), t!("fed.decline_2").to_string()],
                (t!("fed.gate_keep").to_string(), Act::DeclineCancel),
                (t!("fed.decline_confirm").to_string(), Act::DeclineConfirm),
            );
        }
        Modal::Revoke(id) => {
            let (name, libs) = room
                .key_row(id)
                .map(|k| (printable(&k.name, NAME_MAX), k.library_names.join(", ")))
                .unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("fed.revoke_title", name = name).to_string(),
                vec![
                    t!("fed.revoke_1", name = name, libraries = libs).to_string(),
                    t!("fed.revoke_2").to_string(),
                ],
                (t!("fed.gate_keep").to_string(), Act::RevokeCancel),
                (t!("fed.revoke_confirm").to_string(), Act::RevokeConfirm),
            );
        }
        Modal::Forget(id) => {
            let name = room.peer(id).map(|p| printable(&p.name, NAME_MAX)).unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("fed.forget_title", name = name).to_string(),
                vec![t!("fed.forget_1").to_string(), t!("fed.forget_2").to_string()],
                (t!("fed.gate_keep").to_string(), Act::ForgetCancel),
                (t!("fed.forget_confirm").to_string(), Act::ForgetConfirm),
            );
        }
        Modal::TurnOff => draw_gate(
            frame,
            room,
            area,
            t!("fed.off_title").to_string(),
            vec![t!("fed.off_1").to_string(), t!("fed.off_2").to_string()],
            (t!("fed.off_stay").to_string(), Act::TurnOffCancel),
            (t!("fed.off_confirm").to_string(), Act::TurnOffConfirm),
        ),
    }
    if let Some((target, text)) = room.cells.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// Off: the state line, the turn-on card (or the gold sentence), the pitch.
fn draw_off(frame: &mut Frame, room: &mut Room, column: Rect, p: &FederationParams) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let mut y = column.y + 2;
    if p.available {
        frame.render_widget(Paragraph::new(Span::styled(t!("fed.state_off").to_string(), dim())), line(column.y));
        let card = Rect { x: column.x, y, width: column.width, height: 3 };
        let starting = room.starting;
        let hover = !starting && room.cells.ui.pointer.is_some_and(|pt| card.contains(pt));
        let color = if starting { th().dim } else if hover { th().bright } else { th().ok };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
        let inner = block.inner(card);
        frame.render_widget(block, card);
        let label = if starting { t!("fed.turning_on") } else { t!("fed.turn_on") };
        frame.render_widget(
            Paragraph::new(Span::styled(label.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
            inner,
        );
        if !starting {
            room.cells.ui.click(card, Act::TurnOn);
        }
        y += 4;
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("fed.state_unavailable").to_string(), Style::default().fg(th().gold))),
            line(column.y),
        );
    }
    let lines = vec![
        Line::from(vec![
            Span::raw(t!("fed.pitch_1a").to_string()),
            Span::styled(t!("fed.pitch_1b").to_string(), bold()),
            Span::raw(t!("fed.pitch_1c").to_string()),
        ]),
        Line::from(""),
        Line::from(Span::styled(t!("fed.pitch_credentials").to_string(), dim().add_modifier(Modifier::BOLD))),
        Line::from(t!("fed.pitch_2").to_string()),
    ];
    let body = Rect { x: column.x, y, width: column.width, height: column.bottom().saturating_sub(y) };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

/// On: the state row, the tabs, the active tab's body.
fn draw_on(frame: &mut Frame, room: &mut Room, column: Rect, p: &FederationParams) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let connected = p.running && p.online;
    let mut spans = if !p.available {
        vec![Span::styled(t!("fed.state_unavailable").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD))]
    } else if connected {
        vec![
            Span::styled(t!("fed.state_on").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
            Span::raw(t!("fed.state_relay").to_string()),
        ]
    } else {
        vec![
            Span::styled(t!("fed.state_on").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
            Span::raw(t!("fed.state_connecting").to_string()),
        ]
    };
    if let Some(id) = &p.endpoint_id {
        spans.push(Span::raw(t!("fed.state_endpoint", id = short_id(id)).to_string()));
    }
    spans.push(Span::raw(
        if room.accept_requests { t!("fed.state_inbox_open") } else { t!("fed.state_inbox_closed") }.to_string(),
    ));
    let state_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    frame.render_widget(Paragraph::new(Line::from(spans)), line(column.y));
    let polls = t!("fed.polls").to_string();
    if state_w + 2 + polls.chars().count() <= column.width as usize {
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), line(column.y));
    }
    if p.available && !connected {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("▱".repeat(10), dim()),
                Span::raw(" "),
                Span::styled(t!("fed.connecting_note").to_string(), accent()),
            ])),
            line(column.y + 1),
        );
    }

    // The tabs: the active one a filled slab, the rest text buttons; the
    // Requests tab carries the count waiting on an answer.
    let tabs_y = column.y + 2;
    let mut x = column.x;
    let pending = room.pending_inbound();
    for tab in Tab::ALL {
        let name = match tab {
            Tab::Requests if pending > 0 => t!("fed.tab_requests_n", n = pending).to_string(),
            Tab::Requests => t!("fed.tab_requests").to_string(),
            Tab::Tickets => t!("fed.tab_tickets").to_string(),
            Tab::Peers => t!("fed.tab_peers").to_string(),
        };
        let label = format!(" {name} ");
        // In cells, as the kit draws the label, and clamped to the column:
        // counted by characters a Japanese tab got a rect half its width,
        // so the label was cut there, its click covered only what was left,
        // and the next tab started inside it.
        let rect = Rect { x, y: tabs_y, width: (kit::width(&label) as u16).min(column.right().saturating_sub(x)), height: 1 };
        let hover = room.cells.ui.pointer.is_some_and(|pt| rect.contains(pt));
        let style = if tab == room.tab {
            Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
        } else if hover {
            Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        room.cells.ui.click(rect, Act::Tab(tab));
        x = rect.right() + TAB_GAP;
    }
    let note = match room.tab {
        Tab::Requests => t!("fed.note_requests"),
        Tab::Tickets => t!("fed.note_tickets"),
        Tab::Peers => t!("fed.note_peers"),
    }
    .to_string();
    if x as usize + 2 + note.chars().count() <= column.right() as usize {
        frame.render_widget(Paragraph::new(Span::styled(note, dim())).alignment(Alignment::Right), line(tabs_y));
    }

    let body = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: column.bottom().saturating_sub(tabs_y + 2) };
    match room.tab {
        Tab::Requests => draw_requests(frame, room, body),
        Tab::Tickets => draw_tickets(frame, room, body),
        Tab::Peers => draw_peers(frame, room, body),
    }
}

/// The table frame every tab shares: the header words at their columns,
/// the full-width rule beneath. Returns the first row's y.
fn table_head(frame: &mut Frame, at: Rect, cols: &[(u16, u16, &str, bool)]) -> u16 {
    for (x, w, word, right) in cols {
        let rect = Rect { x: *x, y: at.y, width: *w, height: 1 };
        let p = Paragraph::new(Span::styled(word.to_string(), dim()));
        frame.render_widget(if *right { p.alignment(Alignment::Right) } else { p }, rect);
    }
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(at.width as usize), dim())),
        Rect { x: at.x, y: at.y + 1, width: at.width, height: 1 },
    );
    at.y + 2
}

/// A table's rows: the viewport, the selection paint, the scrollbar; the
/// caller paints each row's cells through `draw_row`.
fn table_rows(
    frame: &mut Frame,
    room: &mut Room,
    at: Rect,
    len: usize,
    mut draw_row: impl FnMut(&mut Frame, &mut Room, usize, Rect, bool, bool),
) {
    let avail = at.height as usize;
    let sel_moved = room.sel != room.cells.sel_anchor;
    room.cells.sel_anchor = room.sel;
    let reveal = if sel_moved { room.sel } else { None };
    let (first, visible) = kit::table_view(len, reveal, room.tscroll, avail);
    room.tscroll = first;
    for (row, i) in (first..first + visible).enumerate() {
        let rect = Rect { x: at.x, y: at.y + row as u16, width: at.width, height: 1 };
        let selected = room.sel == Some(i);
        let hovered = !selected && room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        if selected {
            frame.render_widget(
                Paragraph::new(Span::styled(" ".repeat(at.width as usize), Style::default().fg(th().on_accent).bg(th().accent))),
                rect,
            );
        }
        draw_row(frame, room, i, rect, selected, hovered);
        room.cells.ui.click(rect, Act::Select(i));
    }
    let bar = Rect { x: at.x + at.width, y: at.y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut room.cells.ui, bar, len, visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);
}

fn family_style(family: Family) -> Style {
    match family {
        Family::Theirs => Style::default().fg(th().gold).add_modifier(Modifier::BOLD),
        Family::Good => Style::default().fg(th().ok),
        Family::Dead | Family::Mute => dim(),
        Family::Wait => Style::default(),
    }
}

/// A cell's style: selection paint wins, then hover, then the cell's own.
fn cell_style(selected: bool, hovered: bool, own: Style) -> Style {
    if selected {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if hovered {
        Style::default().fg(th().bright)
    } else {
        own
    }
}

fn clip(text: &str, width: u16) -> String {
    let w = width as usize;
    if text.chars().count() <= w {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(w.saturating_sub(1)).collect::<String>())
    }
}

/// Requests: the inbox checkbox card, then the table.
fn draw_requests(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let card = Rect { x: body.x, y: body.y, width: body.width, height: 4 };
    let hover = room.cells.ui.pointer.is_some_and(|p| card.contains(p));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if hover { th().bright } else { th().dim }));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    let glyph = if room.accept_requests {
        Span::styled(format!("{} ", g("[✓]", "[x]")), Style::default().fg(th().ok))
    } else {
        Span::styled("[ ] ", dim())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![glyph, Span::styled(t!("fed.inbox_label").to_string(), bold())])),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(1), height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.inbox_hint").to_string(), dim())),
        Rect { x: inner.x + 5, y: inner.y + 1, width: inner.width.saturating_sub(5), height: 1 },
    );
    room.cells.ui.click(card, Act::ToggleInbox);

    let table = Rect { x: body.x, y: body.y + 5, width: body.width, height: body.height.saturating_sub(5) };
    if table.height < 3 {
        return;
    }
    let (server_w, status_w, age_w) = (18u16, 20u16, 6u16);
    let fixed = server_w + 2 + status_w + 2 + age_w + 2;
    let msg_w = table.width.saturating_sub(fixed);
    let msg_x = table.x + server_w + 2;
    let status_x = msg_x + msg_w + 2;
    let age_x = status_x + status_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, server_w, &t!("fed.col_server"), false),
            (msg_x, msg_w, &t!("fed.col_offer"), false),
            (status_x, status_w, &t!("fed.col_status"), false),
            (age_x, age_w, &t!("fed.col_age"), true),
        ],
    );
    if room.requests.is_empty() {
        let text = if room.accept_requests { t!("fed.empty_requests") } else { t!("fed.empty_requests_closed") };
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let now = unix_now();
    let requests = room.requests.clone();
    table_rows(frame, room, rows_rect, requests.len(), |frame, room, i, rect, selected, hovered| {
        let r = &requests[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&request_name(r), server_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x, server_w),
        );
        let offer = request_offer(r);
        frame.render_widget(Paragraph::new(Span::styled(clip(&offer, msg_w), base)), cell(msg_x, msg_w));
        if offer.chars().count() > msg_w as usize {
            room.cells.ui.tip(cell(msg_x, msg_w), offer.clone());
        }
        let (words, family) = request_status(r);
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&words, status_w), cell_style(selected, hovered, family_style(family)))),
            cell(status_x, status_w),
        );
        frame.render_widget(
            Paragraph::new(Span::styled(age_of(Some(&r.created_at), now), cell_style(selected, hovered, dim()))).alignment(Alignment::Right),
            cell(age_x, age_w),
        );
    });
    // The cursor row's whole story rides the note line, when it is free.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(r) = room.selected_request()
    {
        let mut text = format!("{} · {} · {}", request_name(r), short_id(&r.peer_endpoint_id), request_offer(r));
        if let Some(reason) = r.reject_reason.as_deref().filter(|s| !s.trim().is_empty()) {
            text.push_str(&format!(" · \u{201c}{}\u{201d}", printable(reason, 200)));
        }
        if r.direction == "in" && r.state == "rejected" {
            text.push_str(&t!("fed.ignored_week"));
        }
        frame.render_widget(Paragraph::new(Span::raw(text)), line(body.bottom() + 1));
    } else if room.note.is_none() && room.busy.is_none() && room.accept_requests && room.pending_inbound() >= INBOX_FULL {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("fed.inbox_full").to_string(), Style::default().fg(th().gold))),
            line(body.bottom() + 1),
        );
    }
}

/// Tickets: the mint card, then the table.
fn draw_tickets(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let card = Rect { x: body.x, y: body.y, width: body.width, height: 3 };
    let hover = room.cells.ui.pointer.is_some_and(|p| card.contains(p));
    let color = if hover { th().bright } else { th().ok };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.mint_card").to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
            .alignment(Alignment::Center),
        inner,
    );
    room.cells.ui.click(card, Act::Mint);

    let table = Rect { x: body.x, y: body.y + 4, width: body.width, height: body.height.saturating_sub(4) };
    if table.height < 3 {
        return;
    }
    let (name_w, libs_w, today_w, last_w, claimed_w) = (16u16, 16u16, 6u16, 9u16, 9u16);
    let fixed = name_w + 2 + libs_w + 2 + today_w + 2 + last_w + 2 + claimed_w + 2;
    let limits_w = table.width.saturating_sub(fixed).max(8);
    let libs_x = table.x + name_w + 2;
    let limits_x = libs_x + libs_w + 2;
    let today_x = limits_x + limits_w + 2;
    let last_x = today_x + today_w + 2;
    let claimed_x = last_x + last_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, name_w, &t!("fed.col_name"), false),
            (libs_x, libs_w, &t!("fed.col_libraries"), false),
            (limits_x, limits_w, &t!("fed.col_limits"), false),
            (today_x, today_w, &t!("fed.col_today"), true),
            (last_x, last_w, &t!("fed.col_last_used"), false),
            (claimed_x, claimed_w, &t!("fed.col_claimed"), false),
        ],
    );
    if room.keys.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("fed.empty_tickets").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let now = unix_now();
    let keys = room.keys.clone();
    table_rows(frame, room, rows_rect, keys.len(), |frame, room, i, rect, selected, hovered| {
        let k = &keys[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let faint = cell_style(selected, hovered, dim());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(&k.name, NAME_MAX), name_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x, name_w),
        );
        let libs = k.library_names.join(", ");
        frame.render_widget(Paragraph::new(Span::styled(clip(&libs, libs_w), base)), cell(libs_x, libs_w));
        if libs.chars().count() > libs_w as usize {
            room.cells.ui.tip(cell(libs_x, libs_w), libs.clone());
        }
        let (limits, limits_style) = if k.expired {
            (t!("fed.exp_expired").to_string(), cell_style(selected, hovered, Style::default().fg(th().gold)))
        } else {
            (limits_words(k), base)
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&limits, limits_w), limits_style)), cell(limits_x, limits_w));
        // The webapp's fmtBytes drops a trailing .0 ("218 MB", not "218.0 MB").
        let today = if k.usage_today_bytes > 0 { fmt_bytes(k.usage_today_bytes).replace(".0 ", " ") } else { "—".to_string() };
        frame.render_widget(
            Paragraph::new(Span::styled(today, if k.usage_today_bytes > 0 { base } else { faint })).alignment(Alignment::Right),
            cell(today_x, today_w),
        );
        let last = match k.last_used.as_deref() {
            Some(ts) => (age_of(Some(ts), now), base),
            None => (t!("fed.never").to_string(), faint),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&last.0, last_w), last.1)), cell(last_x, last_w));
        let claimed = if k.bound_endpoint_id.is_some() {
            (t!("fed.claimed").to_string(), cell_style(selected, hovered, Style::default().fg(th().ok)))
        } else {
            (t!("fed.not_claimed").to_string(), faint)
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&claimed.0, claimed_w), claimed.1)), cell(claimed_x, claimed_w));
    });
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(k) = room.selected_key()
    {
        let claimed = match &k.bound_endpoint_id {
            Some(id) => t!("fed.claimed_by", id = short_id(id), age = age_of(k.bound_at.as_deref(), now)).to_string(),
            None => t!("fed.not_claimed_yet").to_string(),
        };
        let text = format!(
            "{} · {} · {} · {}",
            printable(&k.name, NAME_MAX),
            k.library_names.join(", "),
            expiry_words(k),
            claimed
        );
        frame.render_widget(Paragraph::new(Span::raw(text)), line(body.bottom() + 1));
    }
}

/// Peers: the paste box with its preview, then the table.
fn draw_peers(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(t!("fed.paste_label").to_string(), dim()),
            Span::styled(t!("fed.paste_hint").to_string(), dim()),
        ])),
        line(body.y),
    );
    let field = Rect { x: body.x, y: body.y + 1, width: body.width, height: 3 };
    let hover = room.cells.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if room.ticket_focus { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let w = inner.width.saturating_sub(2);
    let shown = if room.ticket_focus {
        kit::field_display(&mut room.cells.ui, inner.x + 1, inner.y, room.ticket.value(), room.ticket.cursor(), w, None)
    } else {
        clip(room.ticket.value(), w)
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 });
    room.cells.ui.click(field, Act::TicketFocus);
    // The preview: who and what, before the server is asked.
    let typed = room.ticket.value().trim();
    if !typed.is_empty() {
        let (text, style) = match decode_ticket(typed) {
            Some(p) => (preview_words(&p), Style::default().fg(th().ok)),
            None => (t!("fed.ticket_invalid").to_string(), Style::default().fg(th().gold)),
        };
        frame.render_widget(Paragraph::new(Span::styled(text, style)), line(body.y + 4));
    }

    let table = Rect { x: body.x, y: body.y + 6, width: body.width, height: body.height.saturating_sub(6) };
    if table.height < 3 {
        return;
    }
    let (server_w, seen_w, disc_w) = (22u16, 12u16, 10u16);
    let fixed = server_w + 2 + seen_w + 2 + disc_w + 2;
    let status_w = table.width.saturating_sub(fixed).max(10);
    let status_x = table.x + server_w + 2;
    let seen_x = status_x + status_w + 2;
    let disc_x = seen_x + seen_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x + 2, server_w - 2, &t!("fed.col_server"), false),
            (status_x, status_w, &t!("fed.col_status"), false),
            (seen_x, seen_w, &t!("fed.col_last_seen"), false),
            (disc_x, disc_w, &t!("fed.col_discovery"), false),
        ],
    );
    if room.peers.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("fed.empty_peers").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let now = unix_now();
    let peers = room.peers.clone();
    table_rows(frame, room, rows_rect, peers.len(), |frame, room, i, rect, selected, hovered| {
        let p = &peers[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let (status, own) = match p.last_status.as_deref() {
            None => (t!("fed.never_tested").to_string(), dim()),
            Some("ok") => (t!("fed.status_ok").to_string(), Style::default().fg(th().ok)),
            Some(other) => (printable(other, 200), Style::default().fg(th().gold)),
        };
        frame.render_widget(Paragraph::new(Span::styled("•", cell_style(selected, hovered, own))), cell(rect.x, 1));
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(&p.name, NAME_MAX), server_w - 2), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x + 2, server_w - 2),
        );
        frame.render_widget(Paragraph::new(Span::styled(clip(&status, status_w), cell_style(selected, hovered, own))), cell(status_x, status_w));
        if status.chars().count() > status_w as usize {
            room.cells.ui.tip(cell(status_x, status_w), status.clone());
        }
        let seen = match p.last_seen.as_deref() {
            Some(ts) => (age_of(Some(ts), now), base),
            None => (t!("fed.never").to_string(), cell_style(selected, hovered, dim())),
        };
        frame.render_widget(Paragraph::new(Span::styled(seen.0, seen.1)), cell(seen_x, seen_w));
        // The discovery opt-out: a checkbox wearing its state, clickable.
        let disc = cell(disc_x, 3);
        let disc_hover = room.cells.ui.pointer.is_some_and(|pt| disc.contains(pt));
        let glyph = if p.use_discovery { g("[✓]", "[x]") } else { "[ ]" };
        let disc_style = match (selected, disc_hover, p.use_discovery) {
            (true, _, _) => base,
            (false, true, _) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
            (false, false, true) => Style::default().fg(th().ok),
            (false, false, false) => dim(),
        };
        frame.render_widget(Paragraph::new(Span::styled(glyph, disc_style)), disc);
        room.cells.ui.click(disc, Act::ToggleDiscovery(p.id));
    });
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(p) = room.selected_peer()
    {
        let text = match room.shares.get(&p.id) {
            Some(libs) if !libs.is_empty() => t!("fed.peer_shares", name = printable(&p.name, NAME_MAX), libraries = libs.join(", ")).to_string(),
            Some(_) => t!("fed.peer_shares", name = printable(&p.name, NAME_MAX), libraries = t!("fed.shares_nothing")).to_string(),
            None => t!("fed.peer_untested_note", name = printable(&p.name, NAME_MAX)).to_string(),
        };
        frame.render_widget(Paragraph::new(Span::raw(text)), line(body.bottom() + 1));
    }
}

// ── Modals ───────────────────────────────────────────────────────────────────

/// A labelled 3-row input inside a modal.
fn modal_field(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = room.cells.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let w = inner.width.saturating_sub(2);
    let shown = if focused {
        kit::field_display(&mut room.cells.ui, inner.x + 1, inner.y, input.value(), input.cursor(), w, None)
    } else {
        clip(input.value(), w)
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 });
    room.cells.ui.click(field, act);
}

/// Checkbox rows in three columns; returns the rows used.
fn checkbox_grid(frame: &mut Frame, room: &mut Room, at: Rect, items: &[(String, bool)], focused: Option<usize>, field_index: impl Fn(usize) -> usize) -> u16 {
    let col_w = at.width / 3;
    for (i, (name, on)) in items.iter().enumerate() {
        let rect = Rect { x: at.x + (i as u16 % 3) * col_w, y: at.y + i as u16 / 3, width: col_w.saturating_sub(1), height: 1 };
        let is_focus = focused == Some(i);
        let hovered = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let box_style = if is_focus {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if *on {
            Style::default().fg(th().ok)
        } else {
            dim()
        };
        let glyph = if *on { format!("{} ", g("[✓]", "[x]")) } else { "[ ] ".to_string() };
        let name_style = if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if is_focus { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(glyph, box_style), Span::styled(clip(name, col_w.saturating_sub(5)), name_style)])),
            rect,
        );
        room.cells.ui.click(rect, Act::FormToggle(field_index(i)));
    }
    items.len().div_ceil(3) as u16
}

/// The mint, accept and limits forms, one drawing.
fn draw_form(frame: &mut Frame, room: &mut Room, area: Rect, f: &Form) {
    let fields = f.fields();
    let focused = f.focused();
    let index_of = |field: Field| fields.iter().position(|x| *x == field).unwrap_or(0);
    let lib_rows = if f.kind == FormKind::Limits { 0 } else { f.libraries.len().div_ceil(3).max(1) as u16 };
    // Outer heights: the rows each layout below takes, plus the frame.
    let height = match f.kind {
        FormKind::Mint => 19 + lib_rows,
        FormKind::Accept => 15 + lib_rows + u16::from(!f.their_offer.is_empty()),
        FormKind::Limits => 18,
    };
    let inner = kit::modal_frame(frame, area, 68, height, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    let title = match f.kind {
        FormKind::Mint => t!("fed.mint_title"),
        FormKind::Accept => t!("fed.accept_title", name = f.about),
        FormKind::Limits => t!("fed.limits_title", name = f.about),
    };
    frame.render_widget(
        Paragraph::new(Span::styled(title.to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::FormCancel);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let mut y = inner.y + 1;
    match f.kind {
        FormKind::Accept => {
            let mut spans = vec![Span::styled(format!("{} · ", short_id(&f.endpoint_id)), dim())];
            if f.message.is_empty() {
                spans.push(Span::styled(t!("fed.no_message").to_string(), dim()));
            } else {
                spans.push(Span::raw(format!("\u{201c}{}\u{201d}", f.message)));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), line(y));
            y += 2;
        }
        FormKind::Limits => {
            frame.render_widget(Paragraph::new(Span::styled(t!("fed.limits_note").to_string(), dim())), line(y));
            y += 2;
        }
        FormKind::Mint => {
            y += 1;
            modal_field(frame, room, Rect { x, y, width: 32.min(w), height: 4 }, &t!("fed.mint_name"), &f.name, focused == Field::Name, Act::FormFocus(index_of(Field::Name)));
            y += 5;
        }
    }
    if f.kind != FormKind::Limits {
        frame.render_widget(
            Paragraph::new(Span::styled(
                if f.kind == FormKind::Mint { t!("fed.mint_libraries") } else { t!("fed.accept_libraries") }.to_string(),
                dim(),
            )),
            line(y),
        );
        y += 1;
        if f.libraries.is_empty() {
            frame.render_widget(Paragraph::new(Span::styled(t!("fed.no_libraries").to_string(), dim())), line(y));
            y += 1;
        } else {
            let focus_lib = if let Field::Library(i) = focused { Some(i) } else { None };
            let lib_offset = index_of(Field::Library(0));
            y += checkbox_grid(frame, room, Rect { x, y, width: w, height: lib_rows }, &f.libraries, focus_lib, |i| lib_offset + i);
        }
        y += 1;
    }
    frame.render_widget(Paragraph::new(Span::styled(t!("fed.limits_label").to_string(), dim())), line(y));
    y += 1;
    let fields_row: Vec<(Field, String, &Input)> = {
        let mut v = vec![
            (Field::StreamKbps, t!("fed.field_kbps").to_string(), &f.stream_kbps),
            (Field::DailyMb, t!("fed.field_daily").to_string(), &f.daily_mb),
            (Field::MaxStreams, t!("fed.field_streams").to_string(), &f.max_streams),
        ];
        if f.kind != FormKind::Limits {
            v.push((Field::Expires, t!("fed.field_expires").to_string(), &f.expires));
        }
        v
    };
    for (i, (field, label, input)) in fields_row.iter().enumerate() {
        let fx = x + i as u16 * 16;
        modal_field(frame, room, Rect { x: fx, y, width: 14.min(w.saturating_sub(i as u16 * 16)), height: 4 }, label, input, focused == *field, Act::FormFocus(index_of(*field)));
    }
    y += 5;
    match f.kind {
        FormKind::Limits => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(t!("fed.expiry_current").to_string(), dim()),
                    Span::raw(f.current_expiry.clone()),
                ])),
                line(y),
            );
            y += 1;
            modal_field(frame, room, Rect { x, y, width: 14.min(w), height: 4 }, &t!("fed.field_new_expiry"), &f.expires, focused == Field::Expires, Act::FormFocus(index_of(Field::Expires)));
            frame.render_widget(
                Paragraph::new(Span::styled(t!("fed.expiry_hint").to_string(), dim())),
                Rect { x: x + 16, y: y + 2, width: w.saturating_sub(16), height: 1 },
            );
        }
        FormKind::Accept if !f.their_offer.is_empty() => {
            let rect = line(y);
            let is_focus = focused == Field::TheirOffer;
            let glyph = if f.accept_their_offer { format!("{} ", g("[✓]", "[x]")) } else { "[ ] ".to_string() };
            let box_style = if is_focus { Style::default().fg(th().on_accent).bg(th().accent) } else if f.accept_their_offer { Style::default().fg(th().ok) } else { dim() };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(glyph, box_style),
                    Span::styled(t!("fed.accept_their_offer", libraries = f.their_offer.join(", ")).to_string(), if is_focus { bold() } else { Style::default() }),
                ])),
                rect,
            );
            room.cells.ui.click(rect, Act::FormToggle(index_of(Field::TheirOffer)));
        }
        _ => {}
    }
    if let Some(err) = &f.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            line(inner.bottom().saturating_sub(2)),
        );
    }
    let label = match f.kind {
        FormKind::Mint => t!("fed.mint_submit"),
        FormKind::Accept => t!("fed.accept_submit"),
        FormKind::Limits => t!("fed.limits_submit"),
    }
    .to_string();
    let bx = inner.right().saturating_sub(kit::width(&label) as u16 + 4);
    kit::button(frame, &mut room.cells.ui, Rect { x: bx, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, f.can_submit(), Act::FormSubmit);
}

/// The ticket to hand over: wrapped whole, y to copy, the credential
/// warning in the webapp's words.
fn draw_minted(frame: &mut Frame, room: &mut Room, area: Rect, name: &str, ticket: Option<&str>, libraries: &[String]) {
    let inner = kit::modal_frame(frame, area, 68, 17, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.minted_title", name = name).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::MintedDone);
    let body = Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: 2 };
    frame.render_widget(
        Paragraph::new(t!("fed.minted_text", libraries = libraries.join(", ")).to_string()).wrap(Wrap { trim: false }),
        body,
    );
    match ticket {
        Some(ticket) => {
            frame.render_widget(
                Paragraph::new(printable(ticket, 4096)).wrap(Wrap { trim: false }),
                Rect { x: inner.x + 1, y: inner.y + 4, width: inner.width.saturating_sub(2), height: 6 },
            );
        }
        None => {
            frame.render_widget(
                Paragraph::new(Span::styled(t!("fed.minted_no_ticket").to_string(), Style::default().fg(th().gold))).wrap(Wrap { trim: false }),
                Rect { x: inner.x + 1, y: inner.y + 4, width: inner.width.saturating_sub(2), height: 3 },
            );
        }
    }
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.minted_note").to_string(), dim())).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y + 11, width: inner.width.saturating_sub(2), height: 2 },
    );
    let y = inner.bottom().saturating_sub(1);
    let done = t!("fed.minted_done").to_string();
    let done_w = kit::width(&done) as u16 + 4;
    let done_rect = kit::button(frame, &mut room.cells.ui, Rect { x: inner.right().saturating_sub(done_w), y, width: done_w, height: 1 }, &done, true, Act::MintedDone);
    if ticket.is_some() {
        let copy = t!("fed.minted_copy").to_string();
        let copy_w = kit::width(&copy) as u16 + 4;
        kit::button(frame, &mut room.cells.ui, Rect { x: done_rect.x.saturating_sub(copy_w + 2), y, width: copy_w, height: 1 }, &copy, false, Act::MintedCopy);
    }
}

/// The pasted ticket's preview and the peer's display name, then Add.
fn draw_peer_name(frame: &mut Frame, room: &mut Room, area: Rect, name: &Input, preview: &TicketPreview) {
    let inner = kit::modal_frame(frame, area, 68, 11, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.peer_name_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::PeerNameCancel);
    frame.render_widget(Paragraph::new(Span::styled(preview_words(preview), Style::default().fg(th().ok))), line(inner.y + 2));
    modal_field(frame, room, Rect { x: inner.x + 1, y: inner.y + 4, width: 40.min(inner.width.saturating_sub(2)), height: 4 }, &t!("fed.peer_name_label"), name, true, Act::PeerNameAdd);
    let label = t!("fed.peer_name_add").to_string();
    let x = inner.right().saturating_sub(kit::width(&label) as u16 + 4);
    kit::button(frame, &mut room.cells.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, true, Act::PeerNameAdd);
}

/// A warning gate: gold, consequences before verbs, the safe choice as
/// the primary, no [X].
fn draw_gate(frame: &mut Frame, room: &mut Room, area: Rect, title: String, body: Vec<String>, safe: (String, Act), go: (String, Act)) {
    let inner = kit::modal_frame(frame, area, 68, 7 + body.len() as u16, th().gold);
    let gold = Style::default().fg(th().gold);
    let mut lines = vec![Line::from(Span::styled(title, gold.add_modifier(Modifier::BOLD))), Line::from("")];
    lines.extend(body.into_iter().map(|b| Line::from(Span::styled(b, gold))));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) },
    );
    let y = inner.bottom().saturating_sub(1);
    let go_w = kit::width(&go.0) as u16 + 4;
    let safe_w = kit::width(&safe.0) as u16 + 4;
    let safe_x = inner.right().saturating_sub(go_w + 2 + safe_w);
    let safe_rect = kit::button(frame, &mut room.cells.ui, Rect { x: safe_x, y, width: inner.width, height: 1 }, &safe.0, true, safe.1);
    kit::button(frame, &mut room.cells.ui, Rect { x: safe_rect.right() + 2, y, width: inner.width, height: 1 }, &go.0, false, go.1);
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::tests::*;
    use super::*;
    use crate::api::types::PeerHealth;

    /// Render one frame and flatten the buffer to text for assertions.
    fn draw(room: &mut Room) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, room)).unwrap();
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

    fn row(frame: &str, name: &str) -> String {
        frame.lines().find(|l| l.contains(name)).map(str::to_string).unwrap_or_default()
    }

    #[test]
    fn off_the_page_turns_the_endpoint_on_with_enter() {
        let _en = english();
        let mut room = new_room();
        room.apply(loaded(params_off()));
        let frame = draw(&mut room);
        assert!(frame.contains("• off — federation is not running"), "{frame}");
        assert!(frame.contains("Turn federation on"), "{frame}");
        assert!(frame.contains("TICKETS ARE CREDENTIALS"), "{frame}");
        assert!(frame.contains("Enter turn on · Esc back"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::SetEnabled(true)));
        room.queued = None;
        // While the start is on its way the card is dim, says so, and takes no second press.
        let frame = draw(&mut room);
        assert!(frame.contains("Starting federation…") && !frame.contains("Turn federation on"), "{frame}");
        assert!(frame.contains("starting the federation endpoint…"), "{frame}");
        assert!(!frame.contains("Enter turn on"), "{frame}");
        press(&mut room, KeyCode::Enter);
        room.act(Act::TurnOn);
        assert!(room.queued.is_none(), "one start at a time");
        // The platform without an Iroh binary says so, in gold.
        room.apply(Done::Enabled { on: true, result: Ok(serde_json::json!({ "enabled": true, "available": false })) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("no prebuilt binary")));
        assert_eq!(room.queued, Some(Op::Load));
        let mut unavailable = params_off();
        unavailable.available = false;
        room.apply(loaded(unavailable));
        let frame = draw(&mut room);
        assert!(frame.contains("• unavailable — Iroh has no prebuilt binary"), "{frame}");
        assert!(!frame.contains("Turn federation on") && !frame.contains("Starting federation…"), "{frame}");
        // A failed start hands the card back.
        room.apply(loaded(params_off()));
        press(&mut room, KeyCode::Enter);
        assert!(room.starting);
        room.queued = None;
        room.apply(Done::Enabled { on: true, result: Err(ApiError::Server { status: 500, message: "relay down".into() }) });
        assert!(!room.starting);
        assert!(draw(&mut room).contains("Turn federation on"));
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
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
        let mut room = on();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut room)).unwrap();
        let pending = room.pending_inbound();
        assert!(pending > 0, "the Requests tab carries its count");
        let tabs = [
            (Tab::Requests, format!(" {} ", t!("fed.tab_requests_n", n = pending))),
            (Tab::Tickets, format!(" {} ", t!("fed.tab_tickets"))),
            (Tab::Peers, format!(" {} ", t!("fed.tab_peers"))),
        ];
        let buffer = terminal.backend().buffer().clone();
        let mut end = 0;
        for (tab, label) in tabs {
            let rect = room.cells.ui.clicks.iter().find(|(_, a)| *a == Act::Tab(tab)).map(|(r, _)| *r).unwrap_or_else(|| panic!("{label:?}"));
            assert_eq!(crate::setup::tests::cells_text(&buffer, rect.y, rect.x, rect.right()), label, "drawn whole");
            assert_eq!(usize::from(rect.width), kit::width(&label), "{label:?}: its click covers it");
            assert!(rect.x >= end, "{label:?} starts past the tab before it");
            end = rect.right();
        }
    }

    #[test]
    fn the_state_row_and_the_tabs() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains("• on — connected to relay · endpoint 51403e7b9c51… · inbox open"), "{frame}");
        assert!(frame.contains("polls every 30 s"), "{frame}");
        assert!(frame.contains(" Requests · 1 ") && frame.contains(" Tickets ") && frame.contains(" Peers "), "{frame}");
        assert!(frame.contains("no access changes hands until a request is accepted"), "{frame}");
        let mut connecting = params_on();
        connecting.online = false;
        room.apply(loaded(connecting));
        let frame = draw(&mut room);
        assert!(frame.contains("• on — connecting to the relay…"), "{frame}");
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ the endpoint is coming up"), "{frame}");
        press(&mut room, KeyCode::Right);
        assert_eq!(room.tab, Tab::Tickets);
        press(&mut room, KeyCode::Left);
        press(&mut room, KeyCode::Left);
        assert_eq!(room.tab, Tab::Peers, "the tabs wrap");
        assert!(draw(&mut room).contains("servers you can read"));
    }

    #[test]
    fn requests_show_the_webapps_states_and_act_by_row() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains("[✓] Accept requests from the discovery network"), "{frame}");
        for word in ["SERVER", "MESSAGE / OFFER", "STATUS", "AGE"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let friend = row(&frame, "friend-node");
        assert!(friend.contains("\u{201c}Jazz for field recordings?\u{201d} · offers: ambient") && friend.contains("needs your answer") && friend.contains("2h"), "{friend}");
        let studio = row(&frame, "studio-nas");
        assert!(studio.contains("you offered: music, vinyl") && studio.contains("waiting on them") && studio.contains("24h"), "{studio}");
        assert!(row(&frame, "Basement Archive").contains("sending… · retry #2"), "{frame}");
        assert!(row(&frame, "jazz-corner").contains("federated"), "{frame}");
        assert!(row(&frame, "Ada's laptop").contains("declined"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · s inbox · x turn off · Esc back"), "{frame}");
        // The cursor row: only the keys that apply, and its story on the note line.
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("↑↓ rows · a accept · n decline · Esc deselect"), "{frame}");
        assert!(frame.contains("friend-node · c4a8f0e19dc4… · \u{201c}Jazz for field recordings?\u{201d} · offers: ambient"), "{frame}");
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("the accept form") };
        assert_eq!((f.kind, f.id, f.about.as_str()), (FormKind::Accept, 1, "friend-node"));
        assert_eq!(f.their_offer, vec!["ambient".to_string()]);
        assert!(f.libraries.iter().all(|(_, on)| !on), "nothing pre-checked: they initiated");
        press(&mut room, KeyCode::Esc);
        press(&mut room, KeyCode::Char('n'));
        assert!(matches!(room.modal, Modal::Decline(1)));
        let frame = draw(&mut room);
        assert!(frame.contains("Decline friend-node's request?") && frame.contains("ignored for seven days") && frame.contains("y decline · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        press(&mut room, KeyCode::Char('n'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Reject(1)));
        room.queued = None;
        room.busy = None;
        press(&mut room, KeyCode::Down); // studio-nas, out · delivered
        assert!(draw(&mut room).contains("↑↓ rows · c withdraw · Esc deselect"));
        press(&mut room, KeyCode::Char('a'));
        assert!(matches!(room.modal, Modal::None), "only an inbound, received request can be accepted");
        press(&mut room, KeyCode::Char('c'));
        assert_eq!(room.queued, Some(Op::Cancel(2)));
        room.queued = None;
        room.busy = None;
        for _ in 0..3 {
            press(&mut room, KeyCode::Down);
        }
        let frame = draw(&mut room);
        assert!(frame.contains("↑↓ rows · d dismiss · Esc deselect"), "{frame}");
        assert!(frame.contains("Ada's laptop · 05d6e8f2ac05… · you offered: music · \u{201c}not now, sorry\u{201d}"), "{frame}");
        press(&mut room, KeyCode::Char('d'));
        assert_eq!(room.queued, Some(Op::Dismiss(5)));
        room.queued = None;
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::SetInbox(false)));
        room.queued = None;
        room.apply(Done::Did { what: Did::Inbox(false), result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("inbox closed")));
    }

    #[test]
    fn the_accept_form_walks_its_fields_and_sends_what_was_ticked() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('a'));
        // Nothing ticked: refused before the server sees it.
        press(&mut room, KeyCode::Enter);
        let Modal::Form(f) = &room.modal else { panic!("the accept form") };
        assert_eq!(f.error.as_deref(), Some("name it and tick at least one library"));
        assert!(room.queued.is_none());
        let frame = draw(&mut room);
        assert!(frame.contains("Accept — share libraries with friend-node"), "{frame}");
        assert!(frame.contains("c4a8f0e19dc4… · \u{201c}Jazz for field recordings?\u{201d}"), "{frame}");
        assert!(frame.contains("LIBRARIES THEY CAN READ") && frame.contains("[ ] music") && frame.contains("STREAM KBPS"), "{frame}");
        assert!(frame.contains("[✓] Add their libraries too (ambient) when they share back"), "{frame}");
        assert!(frame.contains("Tab next field · Space toggle · Enter accept · Esc cancel"), "{frame}");
        press(&mut room, KeyCode::Char(' ')); // music
        assert!(draw(&mut room).contains("[✓] music"));
        for _ in 0..8 {
            press(&mut room, KeyCode::Tab); // past the five libraries and three caps to Expires
        }
        let Modal::Form(f) = &room.modal else { panic!("the accept form") };
        assert_eq!(f.focused(), Field::Expires);
        press(&mut room, KeyCode::Backspace);
        type_text(&mut room, "7x");
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Char(' ')); // their offer: off
        press(&mut room, KeyCode::Enter);
        match room.queued.take() {
            Some(Op::Accept { id, vpaths, limits, expires_at, accept_their_offer }) => {
                assert_eq!(id, 1);
                assert_eq!(vpaths, vec!["music".to_string()]);
                assert_eq!(limits, FederationLimits { stream_kbps: 8000, daily_mb: 2048, max_streams: 3 });
                let cutoff = expires_at.as_deref().and_then(iso_unix).expect("an ISO cutoff");
                assert!((cutoff - unix_now() - 7 * 86_400).abs() < 5, "seven days out, the x dropped");
                assert!(!accept_their_offer);
            }
            other => panic!("{other:?}"),
        }
        room.apply(Done::Did { what: Did::Accepted, result: Err(ApiError::Server { status: 409, message: "cannot accept a request in state 'accepted'".into() }) });
        let Modal::Form(f) = &room.modal else { panic!("a refusal keeps the form") };
        assert!(f.error.as_deref().is_some_and(|e| e.contains("cannot accept")));
        room.apply(Done::Did { what: Did::Accepted, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("accepted")));
        assert_eq!(room.queued, Some(Op::Load));
    }

    #[test]
    fn minting_shows_the_ticket_to_hand_over() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Right);
        press(&mut room, KeyCode::Char('n'));
        let Modal::Form(f) = &room.modal else { panic!("the mint form") };
        assert_eq!((f.kind, f.focused()), (FormKind::Mint, Field::Name));
        type_text(&mut room, "Bob's NAS|");
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Char(' '));
        let frame = draw(&mut room);
        assert!(frame.contains("New federation ticket") && frame.contains("WHO IS THIS FOR?") && frame.contains("Bob's NAS"), "{frame}");
        assert!(frame.contains("[✓] music") && frame.contains("EXPIRES (DAYS)") && frame.contains("Mint ticket ▸"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(
            room.queued,
            Some(Op::Mint {
                name: "Bob's NAS".into(),
                vpaths: vec!["music".into()],
                limits: FederationLimits { stream_kbps: 8000, daily_mb: 2048, max_streams: 3 },
                expires_at: None,
            }),
            "the pipe never reaches the name; 0 days means never"
        );
        room.queued = None;
        room.apply(Done::Minted {
            libraries: vec!["music".into()],
            result: Ok(MintedKey { id: 9, name: "Bob's NAS".into(), ticket: Some("mstrfed1:eyJ0IjoibmV3In0".into()) }),
        });
        assert!(matches!(room.modal, Modal::Minted { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Ticket for Bob's NAS") && frame.contains("mstrfed1:eyJ0IjoibmV3In0"), "{frame}");
        assert!(frame.contains("can read music until it is claimed or revoked"), "{frame}");
        assert!(frame.contains("y copy") && frame.contains("Done") && frame.contains("y copy · Esc done"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert!(matches!(room.modal, Modal::None));
        room.apply(Done::Minted { libraries: vec!["music".into()], result: Ok(MintedKey { id: 10, name: "x".into(), ticket: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("no ticket")));
    }

    #[test]
    fn tickets_show_limits_and_claims_and_act_by_row() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Right);
        let frame = draw(&mut room);
        for word in ["Mint a ticket for a friend", "NAME", "LIBRARIES", "LIMITS", "TODAY", "LAST USED", "CLAIMED"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let ada = row(&frame, "Ada's laptop");
        assert!(ada.contains("music, vinyl") && ada.contains("8 Mbps · 2 GB/day · 3 streams") && ada.contains("218 MB") && ada.contains("1h") && ada.contains("✓ claimed"), "{ada}");
        let studio = row(&frame, "studio-nas");
        assert!(studio.contains("music, vinyl, j…") && studio.contains("unlimited") && studio.contains("not yet"), "{studio}");
        assert!(row(&frame, "jazz-corner").contains("2 Mbps · 500 MB/day · 1 stream"), "{frame}");
        assert!(row(&frame, "Bob's NAS").contains("expired"), "{frame}");
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("Ada's laptop · music, vinyl · never expires · claimed by 8f31c0e2a78f… 2d ago"), "{frame}");
        assert!(frame.contains("↑↓ rows · y copy ticket · l limits · b reset binding · r revoke · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Char('l'));
        let Modal::Form(f) = &room.modal else { panic!("the limits form") };
        assert_eq!((f.kind, f.id, f.about.as_str(), f.current_expiry.as_str()), (FormKind::Limits, 1, "Ada's laptop", "never expires"));
        assert_eq!(f.stream_kbps.value(), "8000");
        let frame = draw(&mut room);
        assert!(frame.contains("Limits for Ada's laptop") && frame.contains("EXPIRY · currently never expires") && frame.contains("NEW EXPIRY"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Limits { id: 1, expiry: ExpiryChange::Keep, .. })), "blank keeps the expiry");
        room.queued = None;
        for _ in 0..3 {
            press(&mut room, KeyCode::Tab);
        }
        type_text(&mut room, "0");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Limits { expiry: ExpiryChange::Never, .. })), "0 means never");
        room.queued = None;
        press(&mut room, KeyCode::Backspace);
        type_text(&mut room, "30");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Limits { expiry: ExpiryChange::At(_), .. })), "30 means thirty days from now");
        room.queued = None;
        room.apply(Done::Did { what: Did::LimitsSaved, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("limits saved")));
        room.note = None;
        room.queued = None;
        press(&mut room, KeyCode::Char('b'));
        assert_eq!(room.queued, Some(Op::ResetBinding(1)));
        room.queued = None;
        room.busy = None;
        press(&mut room, KeyCode::Down); // studio-nas: not claimed
        press(&mut room, KeyCode::Char('b'));
        assert!(room.queued.is_none(), "nothing to reset on an unclaimed ticket");
        assert!(draw(&mut room).contains("studio-nas · music, vinyl, jazz · expires in 24 days · not yet claimed"));
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Revoke(2)));
        let frame = draw(&mut room);
        // The gate wraps its sentence to the modal's width.
        assert!(frame.contains("Revoke studio-nas's ticket?") && frame.contains("studio-nas can no longer") && frame.contains("read music, vinyl, jazz.") && frame.contains("y revoke · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Revoke(2)));
    }

    #[test]
    fn peers_preview_the_pasted_ticket_and_act_by_row() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Char('j'));
        assert_eq!(room.tab, Tab::Peers);
        assert!(room.ticket_focus);
        type_text(&mut room, "garbage");
        let frame = draw(&mut room);
        assert!(frame.contains("that doesn't look like a federation ticket"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("doesn't look like")));
        for _ in 0.."garbage".len() {
            press(&mut room, KeyCode::Backspace);
        }
        let ticket = ticket_for(serde_json::json!({
            "t": "nodeaa4qk3r7z2nlxwm5c6bdhpj2ev5gktr3q7hifobyq2j5ru4kiggizaaaq",
            "k": "f34b91c0d2e7a89b1c2d3e4f5a6b7c8d",
            "n": "mStream Den",
            "l": ["music", "vinyl"],
            "e": "2026-12-01T00:00:00.000Z",
        }));
        type_text(&mut room, &ticket);
        let frame = draw(&mut room);
        assert!(frame.contains("mStream Den — shares music, vinyl · valid until 2026-12-01"), "{frame}");
        assert!(frame.contains("type or paste the ticket · Enter add · Esc leave the field"), "{frame}");
        press(&mut room, KeyCode::Enter);
        let Modal::PeerName { name, .. } = &room.modal else { panic!("the name modal") };
        assert_eq!(name.value(), "mStream Den");
        let frame = draw(&mut room);
        assert!(frame.contains("Name this peer") && frame.contains("Add peer ▸"), "{frame}");
        type_text(&mut room, " 2");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::AddPeer { ticket: ticket.clone(), name: Some("mStream Den 2".into()) }));
        room.queued = None;
        room.apply(Done::Did { what: Did::PeerAdded, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None) && room.ticket.value().is_empty());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("peer added")));

        room.note = None;
        let frame = draw(&mut room);
        let basement = row(&frame, "Basement Archive");
        assert!(basement.starts_with("  • Basement Archive") && basement.contains(" ok ") && basement.contains("[✓]"), "{basement}");
        assert!(row(&frame, "studio-nas").contains("[ ]"), "{frame}");
        let jazz = row(&frame, "jazz-corner");
        assert!(jazz.contains("dial timed out after 20 s") && jazz.contains("3d"), "{jazz}");
        let ada = row(&frame, "Ada's laptop");
        assert!(ada.contains("never tested") && ada.contains("never"), "{ada}");
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("Basement Archive · t tests the connection"), "{frame}");
        assert!(frame.contains("↑↓ rows · t test · s discovery · r forget · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Char('t'));
        assert_eq!(room.queued, Some(Op::Test(1)));
        room.queued = None;
        room.apply(Done::Tested {
            id: 1,
            result: Ok(PeerTest { ok: true, error: None, health: Some(PeerHealth { libraries: vec!["music".into(), "vinyl".into()] }) }),
        });
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n == "Basement Archive shares: music, vinyl"));
        room.note = None;
        assert!(draw(&mut room).contains("Basement Archive shares: music, vinyl"), "the answer stays on the note line");
        room.apply(Done::Tested { id: 1, result: Ok(PeerTest { ok: false, error: Some("dial timed out".into()), health: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("unreachable: dial timed out")));
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::PeerDiscovery { id: 1, on: false }));
        room.queued = None;
        room.apply(Done::Did { what: Did::PeerDiscovery(1, false), result: Ok(()) });
        assert!(!room.peers[0].use_discovery);
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Forget(1)));
        assert!(draw(&mut room).contains("Forget Basement Archive?"));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::RemovePeer(1)));
    }

    #[test]
    fn turning_off_is_gated_and_the_poll_is_quiet() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Char('x'));
        assert!(matches!(room.modal, Modal::TurnOff));
        let frame = draw(&mut room);
        assert!(frame.contains("Turn federation off?") && frame.contains("◂ Stay on") && frame.contains("y turn off · Esc stay on"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert!(room.queued.is_none());
        press(&mut room, KeyCode::Char('x'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::SetEnabled(false)));
        room.queued = None;
        room.apply(loaded(params_off()));
        assert!(room.requests.is_empty() && room.keys.is_empty() && room.peers.is_empty());

        let mut room = new_room();
        room.tick();
        assert!(room.queued.is_none(), "nothing before the first load answers");
        room.apply(loaded(params_off()));
        room.tick();
        assert!(room.queued.is_none(), "off, no poll");
        room.apply(loaded(params_on()));
        room.tick();
        assert_eq!(room.queued, Some(Op::Load));
        assert!(room.busy.is_none(), "the poll shows no busy line");
        room.last_load = Some(Instant::now());
        room.queued = None;
        room.tick();
        assert!(room.queued.is_none(), "not again within the cadence");
        room.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("locked")));
    }

    // ── Hosted in the GUI player's Admin tab ─────────────────────────────

    use crate::admin::hosting::{self, DOCKED, FLOOR, WINDOW};
    use ratatui::buffer::Buffer;

    /// The cells `from..to` of row `y`, as text.
    fn cells(buf: &Buffer, y: u16, from: u16, to: u16) -> String {
        (from..to).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The whole buffer as text, a row a line.
    fn text_of(buf: &Buffer) -> String {
        (buf.area.top()..buf.area.bottom()).map(|y| hosting::row(buf, y)).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn hosted_the_room_draws_inside_its_area_with_no_header_and_its_note_last() {
        let _en = english();
        for (size, area) in [((100, 30), WINDOW), ((176, 46), DOCKED)] {
            let mut room = on();
            room.note = Some(("the relay answered".into(), true));
            let buf = hosting::draw_hosted(&mut room, size, area);
            let text = text_of(&buf);
            assert_eq!(hosting::outside(&buf, area, true), Vec::<(u16, u16)>::new(), "{size:?}\n{text}");
            assert!(!text.contains("home.mstream.example"), "no header\n{text}");
            assert!(!text.contains(&*t!("fed.hint_requests")), "the tips are the host's\n{text}");
            assert_eq!(cells(&buf, area.y, area.x, area.right()), "·".repeat(area.width as usize), "the first row is the host's");
            assert!(cells(&buf, area.y + 1, area.x + 2, area.right()).starts_with("• on — connected to relay"), "{text}");
            assert!(text.contains(" Requests · 1 ") && text.contains(" Peers "), "{text}");
            assert!(cells(&buf, area.bottom() - 1, area.x + 2, area.right()).starts_with("the relay answered"), "{text}");
        }
    }

    #[test]
    fn hosted_a_modal_centres_in_the_rooms_area() {
        let _en = english();
        let mut room = on();
        room.modal = Modal::TurnOff;
        let buf = hosting::draw_hosted(&mut room, (100, 30), WINDOW);
        // The turn-off gate is 68 by 9: a title, a blank, two lines, a
        // blank, the buttons.
        let at = kit::modal_rect(WINDOW, 68, 9, 9);
        assert_eq!((at.x, at.y), (24, 8));
        assert_eq!(buf[(at.x, at.y)].symbol(), "╭");
        assert_eq!(buf[(at.right() - 1, at.bottom() - 1)].symbol(), "╯");
        assert_eq!(hosting::outside(&buf, WINDOW, true), Vec::<(u16, u16)>::new());
    }

    #[test]
    fn hosted_at_the_floor_it_never_says_resize() {
        let _en = english();
        for params in [params_on(), params_off()] {
            let mut room = new_room();
            room.apply(loaded(params));
            room.note = Some(("a note".into(), false));
            let buf = hosting::draw_hosted(&mut room, (100, 24), FLOOR);
            let text = text_of(&buf);
            assert_eq!(hosting::outside(&buf, FLOOR, false), Vec::<(u16, u16)>::new(), "{text}");
            assert!(!text.contains(&*t!("resize")), "{text}");
            assert!(cells(&buf, FLOOR.bottom() - 1, FLOOR.x + 2, FLOOR.right()).starts_with("a note"), "{text}");
        }
    }

    /// The cell a focused field notes is the caret's however the field's
    /// window over a long value stands (hosted, it is where the GUI's window
    /// floats the input method's candidates, and what turns its paste on).
    #[test]
    fn the_ticket_box_and_the_forms_field_note_the_cell_their_caret_is_drawn_in() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Char('j'));
        type_text(&mut room, &format!("mstrfed1:{}end", "q".repeat(120)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "qqqend", "");
        press(&mut room, KeyCode::Home);
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "", "mstrfed1:");
        press(&mut room, KeyCode::Esc);
        draw(&mut room);
        assert_eq!(room.cells.ui.caret_at(), None, "the box let go of the keys");

        room.act(Act::Mint);
        type_text(&mut room, &format!("{}-end", "n".repeat(40)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "nnn-end", "");
    }
}
