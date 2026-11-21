//! The discovery room in character cells: what `mstream-player admin
//! discovery` draws in a terminal, and what the GUI player's Admin tab
//! draws on its grid (the hub's [`Screen::render_hosted`]). The room
//! itself (its state, its worker, its keys, its words) is the parent
//! module; this is its view, and holds what is measured in cells: the
//! state row, the tabs and their pane, the servers table and the modals,
//! on the kit's surface. It is also where the room meets the hub's loop,
//! as a [`Screen`].

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::super::{
    Screen, age_text, body_column, draw_foot, draw_header, fmt_bytes, fmt_count, frame_ground,
    host_of, iso_unix, unix_now,
};
use super::*;
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::g;

/// The tab pane: eight rows under the tabs row, whatever the tab.
const PANE_ROWS: u16 = 8;
const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
/// A held-snapshot count is "N of M" only while auto-fetch has a target.
const TAB_GAP: u16 = 2;

/// What this view keeps on the room between frames. The room holds one
/// and never looks inside it.
pub(super) struct State {
    /// The kit's interaction surface: what the last frame registered.
    ui: Surface<Act>,
    /// Where the last frame drew the tabs' pane, if it drew one: the wheel
    /// walks the log only over it, wherever a host put the room.
    pane_at: Option<Rect>,
    /// The cursor as the table last saw it: the table scrolls to the
    /// cursor only on the frame it moved.
    sel_anchor: Option<usize>,
}

impl State {
    pub(super) fn new() -> State {
        State { ui: Surface::new(), pane_at: None, sel_anchor: None }
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

    fn wheel(&mut self, up: bool, at: Position) {
        if !matches!(self.modal, Modal::None) {
            return;
        }
        // Over the Activity pane the wheel walks the log; anywhere else it
        // is the table's. Only the rows are asked: the pane spans the
        // column, and the column is all the room has across.
        let over_pane = self.cells.pane_at.is_some_and(|p| (p.y..p.bottom()).contains(&at.y));
        if self.tab == Tab::Activity && over_pane {
            self.ascroll = if up { self.ascroll.saturating_add(1) } else { self.ascroll.saturating_sub(1) };
        } else {
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

    #[cfg(feature = "window")]
    fn has_egui(&self) -> bool {
        true
    }

    /// The room's egui face (egui_view.rs), with the input method's
    /// composition and the key-hints setting the host put on the surface.
    #[cfg(feature = "window")]
    fn egui(&mut self, ui: &mut egui::Ui) -> Option<crate::admin::EguiShown> {
        let composition = self.cells.ui.composition().to_string();
        let key_hints = self.cells.ui.key_hints;
        Some(super::egui_view::show(ui, self, &composition, key_hints))
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
/// the area's last row; below its minimum it draws what fits, and the
/// table is what gets shorter.
fn draw(frame: &mut Frame, room: &mut Room, area: Rect, hosted: bool) {
    room.cells.pane_at = None;

    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.cells.ui.pointer;
    if modal_open {
        room.cells.ui.pointer = None;
    }

    if !hosted {
        draw_header(frame, area, &t!("p2p.title"), &host_of(&room.client));
    }
    let column = body_column(area, hosted);
    // Lent to the draw and put back, not copied every frame (performance
    // audit #110): nothing the page draws reads it off the room.
    let status = room.status.take();
    match &status {
        None => {}
        Some(s) if !s.enabled => draw_off(frame, room, column, s),
        Some(s) => draw_on(frame, room, column, s),
    }
    room.status = status;
    draw_foot(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room), hosted);

    if modal_open {
        room.cells.ui.pointer = live_pointer;
        room.cells.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Sheet { id, sel } => draw_sheet(frame, room, area, &id, sel),
        Modal::Federate(c) => draw_compose(frame, room, area, &c),
        Modal::Identity(d) => draw_identity(frame, room, area, &d),
        Modal::Setting(d) => draw_setting(frame, room, area, &d),
        Modal::Block(id) => draw_block(frame, room, area, &id),
        Modal::Leave => draw_leave(frame, room, area),
        Modal::Blocked(sel) => draw_blocked(frame, room, area, sel),
    }
    if let Some((target, text)) = room.cells.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// Off the network: the state line, the join card, the opt-in, the pitch.
fn draw_off(frame: &mut Frame, room: &mut Room, column: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let unavailable = !s.binary_found && !s.binary_fetchable;
    if unavailable {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.state_unavailable").to_string(), Style::default().fg(th().gold))),
            line(column.y),
        );
    } else {
        let mut state = t!("p2p.state_off").to_string();
        if !s.binary_found && s.binary_fetchable {
            state.push_str(" · ");
            state.push_str(&t!("p2p.off_will_download"));
        }
        frame.render_widget(Paragraph::new(Span::styled(state, dim())), line(column.y));
    }
    let mut y = column.y + 2;

    if !unavailable {
        // The one affirmative action, the add-folder card's shape: green,
        // hover brightens, Enter or a click joins at once.
        let card = Rect { x: column.x, y, width: column.width, height: 3 };
        let hover = room.cells.ui.pointer.is_some_and(|p| card.contains(p));
        let color = if hover { th().bright } else { th().ok };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
        let inner = block.inner(card);
        frame.render_widget(block, card);
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.off_join").to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
                .alignment(Alignment::Center),
            inner,
        );
        room.cells.ui.click(card, Act::Join);
        y += 4;

        // The opt-in: the kit's checkbox card, on by default. Space and a
        // click flip it; the border hovers like every clickable.
        let opt = Rect { x: column.x, y, width: column.width, height: 4 };
        let hover = room.cells.ui.pointer.is_some_and(|p| opt.contains(p));
        let border = if hover { th().bright } else { th().dim };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
        let inner = block.inner(opt);
        frame.render_widget(block, opt);
        let glyph = if room.accept_requests {
            Span::styled(format!("{} ", g("[✓]", "[x]")), Style::default().fg(th().ok))
        } else {
            Span::styled("[ ] ", dim())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![glyph, Span::styled(t!("p2p.off_requests").to_string(), bold())])),
            Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(1), height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.off_requests_hint").to_string(), dim())),
            Rect { x: inner.x + 5, y: inner.y + 1, width: inner.width.saturating_sub(5), height: 1 },
        );
        room.cells.ui.click(opt, Act::ToggleRequests);
        y += 5;
    }

    // The pitch — the webapp's words, wrapped to the column.
    let lines = vec![
        Line::from(t!("p2p.off_pitch_1").to_string()),
        Line::from(""),
        Line::from(Span::styled(t!("p2p.off_shared").to_string(), dim().add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::raw(t!("p2p.off_pitch_2a").to_string()),
            Span::styled(t!("p2p.off_pitch_2b").to_string(), bold()),
            Span::raw(t!("p2p.off_pitch_2c").to_string()),
        ]),
        Line::from(""),
        Line::from(t!("p2p.off_pitch_3").to_string()),
    ];
    let body = Rect { x: column.x, y, width: column.width, height: column.bottom().saturating_sub(y) };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

/// On the network: the state row, the tabs and their pane, the table.
fn draw_on(frame: &mut Frame, room: &mut Room, column: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };

    // The state row: the dot in its color, then the facts.
    let recovering = (s.recovery.attempts > 0 || s.recovery.retry_pending) && s.neighbors == 0;
    let (word, color, detail) = if !s.binary_found {
        (t!("p2p.state_unavailable").to_string(), th().gold, String::new())
    } else if s.neighbors > 0 {
        let d = if s.neighbors == 1 { t!("p2p.state_neighbors_one") } else { t!("p2p.state_neighbors", n = s.neighbors) };
        (t!("p2p.state_connected").to_string(), th().ok, d.to_string())
    } else if recovering {
        (t!("p2p.state_reconnecting").to_string(), th().gold, t!("p2p.state_reconnecting_detail", n = s.recovery.attempts).to_string())
    } else if s.joined {
        (t!("p2p.state_waiting").to_string(), th().gold, String::new())
    } else {
        (t!("p2p.state_not_joined").to_string(), th().dim, String::new())
    };
    // The facts after the state are optional, in reverse order of loss: a
    // narrow window drops the announced name first (Config has it), then
    // the sidecar figure — the state itself is never clipped.
    let mut groups: Vec<Vec<Span>> = vec![vec![
        Span::styled(word, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::raw(detail),
    ]];
    if let Some(mb) = s.watchdog.last_rss_mb {
        let mut sidecar = vec![Span::raw(t!("p2p.state_sidecar", mb = format!("{mb:.0}")).to_string())];
        if s.watchdog.restarts > 0 {
            let r = if s.watchdog.restarts == 1 { t!("p2p.state_restart_one") } else { t!("p2p.state_restarts", n = s.watchdog.restarts) };
            sidecar.push(Span::styled(r.to_string(), Style::default().fg(th().gold)));
        }
        groups.push(sidecar);
    }
    groups.push(vec![
        Span::raw(t!("p2p.state_announcing").to_string()),
        Span::styled(printable(&s.server_name, NAME_MAX), bold()),
    ]);
    let width_of = |groups: &[Vec<Span>]| -> usize {
        groups.iter().flatten().map(|sp| sp.content.chars().count()).sum()
    };
    while groups.len() > 1 && width_of(&groups) > column.width as usize {
        groups.pop();
    }
    let state_w = width_of(&groups);
    frame.render_widget(Paragraph::new(Line::from(groups.concat())), line(column.y));
    let polls = t!("p2p.polls").to_string();
    if state_w + 2 + polls.chars().count() <= column.width as usize {
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), line(column.y));
    }
    // The indeterminate "something is happening" bar: the feature is on
    // but the mesh has no neighbor yet — the kit's all-dim bar, never a
    // fake percentage.
    if s.binary_found && (!s.running || !s.joined || s.neighbors == 0) {
        let text = if recovering { t!("p2p.recovering") } else { t!("p2p.searching") };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("▱".repeat(10), dim()),
                Span::raw(" "),
                Span::styled(text.to_string(), accent()),
            ])),
            line(column.y + 1),
        );
    }

    // The tabs: the active one a filled slab, the rest text buttons.
    let tabs_y = column.y + 2;
    let mut x = column.x;
    for tab in Tab::ALL {
        let label = format!(" {} ", tab.label());
        // In cells, as the kit draws the label, and clamped to the column:
        // counted by characters a Japanese tab got a rect half its width,
        // so the label was cut there, its click covered only what was left,
        // and the next tab started inside it.
        let rect = Rect { x, y: tabs_y, width: (kit::width(&label) as u16).min(column.right().saturating_sub(x)), height: 1 };
        let hover = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
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
    if room.tab == Tab::Activity {
        let note = t!("p2p.activity_note").to_string();
        if x as usize + 2 + note.chars().count() <= column.right() as usize {
            frame.render_widget(Paragraph::new(Span::styled(note, dim())).alignment(Alignment::Right), line(tabs_y));
        }
    }

    let pane = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: PANE_ROWS };
    room.cells.pane_at = Some(pane);
    match room.tab {
        Tab::Stats => draw_stats(frame, room, pane, s),
        Tab::Activity => draw_activity(frame, room, pane),
        Tab::Invite => draw_invite(frame, room, pane, s),
        Tab::Config => draw_config(frame, room, pane, s),
    }

    let table = Rect { x: column.x, y: pane.bottom(), width: column.width, height: column.bottom().saturating_sub(pane.bottom()) };
    draw_servers(frame, room, table, s);
}

/// Six tiles in two rows of three: value BOLD, label and detail DIM.
fn draw_stats(frame: &mut Frame, room: &mut Room, pane: Rect, s: &DiscoveryStatus) {
    let held: Vec<&CatalogPeer> = room.catalog.peers.iter().filter(|p| p.fetched.is_some()).collect();
    let held_tracks: u64 = held.iter().map(|p| p.payload.row_count).sum();
    let hidden = if room.show_incompatible { 0 } else { room.catalog.hidden_incompatible };
    let known = room.catalog.peers.len() as u32 + hidden;
    let held_n = held.len().to_string();
    let held_value = if s.auto_fetch_count > 0 {
        t!("p2p.tile_held_of", held = held.len(), cap = s.auto_fetch_count).to_string()
    } else {
        held_n.clone()
    };
    let memory = match s.watchdog.last_rss_mb {
        Some(mb) => t!("p2p.value_mb", n = format!("{mb:.0}")).to_string(),
        None => "—".to_string(),
    };
    let memory_detail = if s.watchdog.max_rss_mb > 0 {
        t!("p2p.tile_memory_detail", max = s.watchdog.max_rss_mb, restarts = s.watchdog.restarts).to_string()
    } else {
        t!("p2p.tile_memory_off").to_string()
    };
    let tiles: [(String, String, String); 6] = [
        (s.neighbors.to_string(), t!("p2p.tile_neighbors").to_string(), t!("p2p.tile_neighbors_detail").to_string()),
        (
            known.to_string(),
            t!("p2p.tile_known").to_string(),
            t!("p2p.tile_known_detail", hidden = hidden, blocked = s.blocked_peers.len()).to_string(),
        ),
        (
            held_value,
            t!("p2p.tile_held").to_string(),
            t!("p2p.tile_held_detail", used = fmt_bytes(room.catalog.storage.used_bytes), cap = fmt_bytes(room.catalog.storage.cap_bytes)).to_string(),
        ),
        (fmt_count(held_tracks), t!("p2p.tile_tracks").to_string(), t!("p2p.tile_tracks_detail", n = held.len()).to_string()),
        (memory, t!("p2p.tile_memory").to_string(), memory_detail),
        (held_n, t!("p2p.tile_seeding").to_string(), t!("p2p.tile_seeding_detail").to_string()),
    ];
    let tile_w = pane.width / 3;
    for (i, (value, label, detail)) in tiles.iter().enumerate() {
        let x = pane.x + (i as u16 % 3) * tile_w;
        let y = pane.y + (i as u16 / 3) * 4;
        let row = |dy: u16| Rect { x, y: y + dy, width: tile_w.saturating_sub(1), height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(value.clone(), bold())), row(0));
        frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), row(1));
        frame.render_widget(Paragraph::new(Span::styled(detail.clone(), dim())), row(2));
    }
}

/// The log, newest first, level as color: warn and error gold, debug dim.
fn draw_activity(frame: &mut Frame, room: &mut Room, pane: Rect) {
    if room.activity.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.activity_empty").to_string(), dim())),
            Rect { x: pane.x, y: pane.y, width: pane.width, height: 1 },
        );
        return;
    }
    let rows = pane.height as usize;
    let max_scroll = room.activity.len().saturating_sub(rows);
    room.ascroll = room.ascroll.min(max_scroll);
    let newest = room.activity.len() - room.ascroll;
    let first = newest.saturating_sub(rows);
    let now = unix_now();
    for (row, entry) in room.activity[first..newest].iter().rev().enumerate() {
        let style = match entry.level.as_str() {
            "warn" | "error" => Style::default().fg(th().gold),
            "debug" => dim(),
            _ => Style::default(),
        };
        let when = iso_unix(&entry.t).map(|t| age_text(now - t)).unwrap_or_else(|| "—".to_string());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{when:>9}  "), dim()),
                Span::styled(printable(&entry.message, 400), style),
            ])),
            Rect { x: pane.x, y: pane.y + row as u16, width: pane.width, height: 1 },
        );
    }
}

/// Endpoint, the ticket whole, and the befriend box — the kit's inline
/// text input, focused by click or `j`.
fn draw_invite(frame: &mut Frame, room: &mut Room, pane: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: pane.x, y, width: pane.width, height: 1 };
    let label_w = 11u16;
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.invite_endpoint").to_string(), dim())), line(pane.y));
    frame.render_widget(
        Paragraph::new(Span::raw(match &s.endpoint_id {
            Some(id) => printable(id, 64),
            None => t!("p2p.invite_no_endpoint").to_string(),
        })),
        Rect { x: pane.x + label_w, y: pane.y, width: pane.width.saturating_sub(label_w), height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(t!("p2p.invite_ticket").to_string(), dim()),
            Span::styled(t!("p2p.invite_ticket_hint").to_string(), dim()),
        ])),
        line(pane.y + 1),
    );
    if let Some(ticket) = &s.ticket {
        let copy = t!("p2p.invite_copy").to_string();
        let w = kit::width(&copy) as u16 + 4;
        kit::button(
            frame,
            &mut room.cells.ui,
            Rect { x: pane.right().saturating_sub(w), y: pane.y + 1, width: w, height: 1 },
            &copy,
            false,
            Act::CopyTicket,
        );
        frame.render_widget(
            Paragraph::new(printable(ticket, 4096)).wrap(Wrap { trim: false }),
            Rect { x: pane.x, y: pane.y + 2, width: pane.width, height: 2 },
        );
    } else {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.invite_no_endpoint").to_string(), dim())), line(pane.y + 2));
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(t!("p2p.invite_befriend").to_string(), dim()),
            Span::styled(t!("p2p.invite_befriend_hint").to_string(), dim()),
        ])),
        line(pane.y + 4),
    );
    let field = Rect { x: pane.x, y: pane.y + 5, width: pane.width, height: 3 };
    let hover = room.cells.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if room.ticket_focus { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if room.ticket_focus {
        let w = inner.width.saturating_sub(2);
        kit::field_display(&mut room.cells.ui, inner.x + 1, inner.y, room.ticket.value(), room.ticket.cursor(), w, None)
    } else {
        room.ticket.value().to_string()
    };
    frame.render_widget(
        Paragraph::new(Span::raw(shown)),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 },
    );
    room.cells.ui.click(field, Act::TicketFocus);
}

/// The identity line, then the settings rows: key, label, value, hint.
fn draw_config(frame: &mut Frame, room: &mut Room, pane: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: pane.x, y, width: pane.width, height: 1 };
    let mut spans = vec![
        Span::styled(t!("p2p.config_announcing").to_string(), dim()),
        Span::raw("  "),
        Span::styled(printable(&s.server_name, NAME_MAX), bold()),
    ];
    if !s.server_description.trim().is_empty() {
        spans.push(Span::raw(format!(" — {}", printable(&s.server_description, DESCRIPTION_MAX))));
    }
    let edit = t!("p2p.config_edit").to_string();
    let edit_w = kit::width(&edit) as u16 + 4;
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { x: pane.x, y: pane.y, width: pane.width.saturating_sub(edit_w + 1), height: 1 },
    );
    kit::button(
        frame,
        &mut room.cells.ui,
        Rect { x: pane.right().saturating_sub(edit_w), y: pane.y, width: edit_w, height: 1 },
        &edit,
        false,
        Act::IdentityOpen,
    );

    let days = |n: u32| if n > 0 { t!("p2p.value_days", n = n).to_string() } else { t!("p2p.value_never").to_string() };
    let rows: [(char, String, String, String, Option<DiscoverySetting>); 6] = [
        ('1', t!("p2p.config_storage").to_string(), t!("p2p.value_mb", n = s.max_peer_db_storage_mb).to_string(), t!("p2p.config_storage_hint").to_string(), Some(DiscoverySetting::MaxStorageMb)),
        (
            '2',
            t!("p2p.config_retention").to_string(),
            if s.peer_retention_days > 0 { t!("p2p.value_days_silence", n = s.peer_retention_days).to_string() } else { t!("p2p.value_never").to_string() },
            t!("p2p.config_retention_hint").to_string(),
            Some(DiscoverySetting::PeerRetentionDays),
        ),
        (
            '3',
            t!("p2p.config_autofetch").to_string(),
            if !room.catalog.auto_fetch {
                t!("p2p.value_off").to_string()
            } else if s.auto_fetch_count == 0 {
                t!("p2p.value_paused").to_string()
            } else {
                t!("p2p.value_servers", n = s.auto_fetch_count).to_string()
            },
            t!("p2p.config_autofetch_hint").to_string(),
            Some(DiscoverySetting::AutoFetchCount),
        ),
        ('4', t!("p2p.config_rotation").to_string(), days(s.rotation_days), t!("p2p.config_rotation_hint").to_string(), Some(DiscoverySetting::RotationDays)),
        (
            '5',
            t!("p2p.config_memory").to_string(),
            if s.watchdog.max_rss_mb > 0 { t!("p2p.value_mb", n = s.watchdog.max_rss_mb).to_string() } else { t!("p2p.tile_memory_off").to_string() },
            t!("p2p.config_memory_hint").to_string(),
            Some(DiscoverySetting::SidecarMaxRssMb),
        ),
        (' ', t!("p2p.config_seeds").to_string(), if s.community_seeds { t!("p2p.value_on") } else { t!("p2p.value_off") }.to_string(), t!("p2p.config_seeds_hint").to_string(), None),
    ];
    let label_x = pane.x + 3;
    let value_x = pane.x + 27;
    let hint_x = pane.x + 53;
    for (i, (key, label, value, hint, setting)) in rows.iter().enumerate() {
        let y = pane.y + 2 + i as u16;
        let rect = line(y);
        let hover = setting.is_some() && room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let key_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else { bold() };
        frame.render_widget(Paragraph::new(Span::styled(key.to_string(), key_style)), Rect { x: pane.x, y, width: 1, height: 1 });
        frame.render_widget(
            Paragraph::new(Span::styled(label.clone(), if hover { Style::default().fg(th().bright) } else { dim() })),
            Rect { x: label_x, y, width: value_x.saturating_sub(label_x + 1), height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(value.clone(), bold())),
            Rect { x: value_x, y, width: hint_x.saturating_sub(value_x + 1), height: 1 },
        );
        if pane.right() > hint_x + 4 {
            frame.render_widget(
                Paragraph::new(Span::styled(hint.clone(), dim())),
                Rect { x: hint_x, y, width: pane.right().saturating_sub(hint_x), height: 1 },
            );
        }
        if let Some(which) = setting {
            room.cells.ui.click(rect, Act::SettingOpen(*which));
        }
    }
}

/// The catalog as a table: the section line (filter, hidden, blocked),
/// the header and rule, then the rows — Enter (or a click) opens a sheet.
fn draw_servers(frame: &mut Frame, room: &mut Room, table: Rect, s: &DiscoveryStatus) {
    if table.height < 4 {
        return;
    }
    let mut y = table.y;
    let line = |y: u16| Rect { x: table.x, y, width: table.width, height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.section_servers").to_string(), dim().add_modifier(Modifier::BOLD))),
        line(y),
    );
    // The right side of the section line: the filter (live text while it
    // is on), the hidden-incompatible toggle and the blocked count.
    let mut parts: Vec<Span> = Vec::new();
    let mut caret = None;
    if room.filter_on {
        let shown = if room.filter_focus {
            let (line, at) = kit::field_line(&room.cells.ui, room.filter.value(), room.filter.cursor(), 30, None);
            caret = Some(at);
            line
        } else {
            room.filter.value().to_string()
        };
        parts.push(Span::styled(format!("/ {shown}"), if room.filter_focus { accent() } else { Style::default() }));
    } else {
        parts.push(Span::styled(t!("p2p.filter_key").to_string(), dim()));
    }
    let hidden = room.catalog.hidden_incompatible;
    if hidden > 0 || room.show_incompatible {
        parts.push(Span::styled(" · ", dim()));
        if room.show_incompatible {
            parts.push(Span::styled(t!("p2p.hidden_showing").to_string(), dim()));
        } else if hidden == 1 {
            parts.push(Span::styled(t!("p2p.hidden_one").to_string(), dim()));
        } else {
            parts.push(Span::styled(t!("p2p.hidden_many", n = hidden).to_string(), dim()));
        }
        parts.push(Span::styled(" · ", dim()));
        parts.push(Span::styled(if room.show_incompatible { t!("p2p.hidden_hide") } else { t!("p2p.hidden_show") }.to_string(), dim()));
    }
    if !s.blocked_peers.is_empty() {
        parts.push(Span::styled(" · ", dim()));
        parts.push(Span::styled(t!("p2p.blocked_n", n = s.blocked_peers.len()).to_string(), dim()));
        parts.push(Span::styled(" · ", dim()));
        parts.push(Span::styled(t!("p2p.blocked_manage").to_string(), dim()));
    }
    // Measured in cells, as the line is drawn: a composition in the
    // filter, or a translation, of wide characters takes two each.
    let right_w: usize = parts.iter().map(|p| kit::width(&p.content)).sum();
    let right = Rect {
        x: table.right().saturating_sub(right_w as u16),
        y,
        width: (right_w as u16).min(table.width),
        height: 1,
    };
    frame.render_widget(Paragraph::new(Line::from(parts)), right);
    room.cells.ui.click(right, Act::FilterOpen);
    // The filter's caret, now that its line has a place: after `/ `.
    if let Some(at) = caret {
        room.cells.ui.note_caret(Position { x: right.x.saturating_add(2 + at), y });
    }
    y += 1;

    // Columns: the fixed ones from the right, SERVER takes the rest —
    // and the narrowest windows give up seeders, then tracks.
    let (tracks_w, seeders_w, online_w, snap_w, fed_w) = (7u16, 7u16, 13u16, 19u16, 14u16);
    let mut show_seeders = true;
    let mut show_tracks = true;
    let fixed = |seeders: bool, tracks: bool| {
        online_w + 2 + snap_w + 2 + fed_w + if seeders { seeders_w + 2 } else { 0 } + if tracks { tracks_w + 2 } else { 0 }
    };
    if table.width.saturating_sub(fixed(true, true)) < 16 {
        show_seeders = false;
    }
    if table.width.saturating_sub(fixed(false, true)) < 16 {
        show_tracks = false;
    }
    let server_w = table.width.saturating_sub(fixed(show_seeders, show_tracks));
    let mut x = table.x + server_w;
    let tracks_x = x;
    if show_tracks {
        x += tracks_w + 2;
    }
    let seeders_x = x;
    if show_seeders {
        x += seeders_w + 2;
    }
    let online_x = x;
    let snap_x = online_x + online_w + 2;
    let fed_x = snap_x + snap_w + 2;

    let head = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_server").to_string(), dim())), head(table.x, server_w));
    if show_tracks {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_tracks").to_string(), dim())).alignment(Alignment::Right), head(tracks_x, tracks_w));
    }
    if show_seeders {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_seeders").to_string(), dim())).alignment(Alignment::Right), head(seeders_x, seeders_w));
    }
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_online").to_string(), dim())), head(online_x, online_w));
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_snapshot").to_string(), dim())), head(snap_x, snap_w));
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_federation").to_string(), dim())), head(fed_x, fed_w));
    y += 1;
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(table.width as usize), dim())), line(y));
    y += 1;

    // Places in the catalog, not copies of its peers: each row on screen
    // is read where it lives (performance audit #110).
    let rows = room.row_indices();
    if rows.is_empty() {
        let text = if room.filter_on && !room.catalog.peers.is_empty() { t!("p2p.empty_filter") } else { t!("p2p.empty_servers") };
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), dim())), line(y));
        return;
    }
    let avail = table.bottom().saturating_sub(y) as usize;
    let sel_moved = room.sel != room.cells.sel_anchor;
    room.cells.sel_anchor = room.sel;
    let reveal = if sel_moved { room.sel } else { None };
    let (first, visible) = kit::table_view(rows.len(), reveal, room.tscroll, avail);
    room.tscroll = first;
    if visible == 0 {
        return;
    }
    let rows_y = y;
    let now = unix_now();
    for (i, &at) in rows.iter().enumerate().skip(first).take(visible) {
        let peer = &room.catalog.peers[at];
        let selected = room.sel == Some(i);
        let rect = line(y);
        let hovered = !selected && room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let row_bg = if selected { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default() };
        frame.render_widget(Paragraph::new(Span::styled(" ".repeat(table.width as usize), row_bg)), rect);
        let base = if selected { row_bg } else if hovered { Style::default().fg(th().bright) } else { Style::default() };
        let faint = if selected { row_bg } else if hovered { Style::default().fg(th().bright) } else { dim() };
        let cell = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(display_name(peer), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(table.x, server_w.saturating_sub(2)),
        );
        if show_tracks {
            frame.render_widget(Paragraph::new(Span::styled(fmt_count(peer.payload.row_count), base)).alignment(Alignment::Right), cell(tracks_x, tracks_w));
        }
        if show_seeders {
            frame.render_widget(Paragraph::new(Span::styled(peer.seeders.to_string(), base)).alignment(Alignment::Right), cell(seeders_x, seeders_w));
        }
        let online = if peer.online {
            (t!("p2p.online").to_string(), base)
        } else {
            let age = iso_unix(&peer.updated_at).map(|t| age_text(now - t));
            (
                match age {
                    Some(age) => t!("p2p.offline_age", age = age).to_string(),
                    None => t!("p2p.offline").to_string(),
                },
                faint,
            )
        };
        frame.render_widget(Paragraph::new(Span::styled(online.0, online.1)), cell(online_x, online_w));
        let snap = match &peer.fetched {
            None => ("—".to_string(), faint),
            Some(h) => (
                match (h.stale, h.pinned) {
                    (false, false) => t!("p2p.snap_downloaded"),
                    (false, true) => t!("p2p.snap_pinned"),
                    (true, false) => t!("p2p.snap_update"),
                    (true, true) => t!("p2p.snap_update_pinned"),
                }
                .to_string(),
                base,
            ),
        };
        frame.render_widget(Paragraph::new(Span::styled(snap.0, snap.1)), cell(snap_x, snap_w));
        let fed = match room.relation(&peer.from) {
            Relation::None => ("—".to_string(), faint),
            Relation::Federated => (t!("p2p.fed_federated").to_string(), if selected || hovered { base } else { Style::default().fg(th().ok) }),
            Relation::Theirs => (t!("p2p.fed_theirs").to_string(), if selected || hovered { base } else { Style::default().fg(th().gold) }),
            Relation::Sent => (t!("p2p.fed_sent").to_string(), faint),
        };
        frame.render_widget(Paragraph::new(Span::styled(fed.0, fed.1)), cell(fed_x, fed_w));
        room.cells.ui.click(rect, Act::OpenSheet(i));
        if !peer.payload.description.trim().is_empty() {
            room.cells.ui.tip(rect, printable(&peer.payload.description, DESCRIPTION_MAX));
        }
        y += 1;
    }
    // The selected row's description rides the note line, when nothing
    // else is on it.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(peer) = room.sel.and_then(|s| rows.get(s)).map(|&at| &room.catalog.peers[at])
        && !peer.payload.description.trim().is_empty()
    {
        let text = format!("{} · {}", display_name(peer), printable(&peer.payload.description, DESCRIPTION_MAX));
        frame.render_widget(
            Paragraph::new(Span::raw(text)),
            Rect { x: table.x, y: table.bottom() + 1, width: table.width, height: 1 },
        );
    }
    let bar = Rect { x: table.x + table.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut room.cells.ui, bar, rows.len(), visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);
}

// ── Modals ───────────────────────────────────────────────────────────────────

/// Enter on a row: the server's facts, then its actions as a list picker.
fn draw_sheet(frame: &mut Frame, room: &mut Room, area: Rect, id: &str, sel: usize) {
    let Some(peer) = room.peer(id).cloned() else {
        room.modal = Modal::None;
        return;
    };
    let menu = room.sheet_menu(&peer);
    let height = 10 + menu.len() as u16;
    let inner = kit::modal_frame(frame, area, 68, height, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(display_name(&peer), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::SheetClose);
    frame.render_widget(
        Paragraph::new(Span::raw(printable(&peer.payload.description, DESCRIPTION_MAX))),
        line(inner.y + 1),
    );
    let now = unix_now();
    let label_w = 12u16;
    let fact = |frame: &mut Frame, y: u16, label: String, value: String, style: Style| {
        frame.render_widget(Paragraph::new(Span::styled(label, dim())), line(y));
        frame.render_widget(
            Paragraph::new(Span::styled(value, style)),
            Rect { x: inner.x + 1 + label_w, y, width: inner.width.saturating_sub(2 + label_w), height: 1 },
        );
    };
    let seen = iso_unix(&peer.updated_at).map(|t| t!("p2p.sheet_seen", age = age_text(now - t)).to_string()).unwrap_or_default();
    fact(frame, inner.y + 3, t!("p2p.sheet_endpoint").to_string(), format!("{}{seen}", short_id(&peer.from)), Style::default());
    fact(
        frame,
        inner.y + 4,
        t!("p2p.sheet_online").to_string(),
        format!(
            "{} · {}",
            if peer.online { t!("p2p.online") } else { t!("p2p.offline") },
            t!("p2p.sheet_tracks", tracks = fmt_count(peer.payload.row_count), seeders = peer.seeders)
        ),
        Style::default(),
    );
    let snapshot = match &peer.fetched {
        None => t!("p2p.sheet_snapshot_none").to_string(),
        Some(h) => {
            let age = iso_unix(&h.fetched_at).map(|t| age_text(now - t)).unwrap_or_else(|| "—".to_string());
            let mut s = t!("p2p.sheet_snapshot_held", age = age, size = fmt_bytes(h.size_bytes)).to_string();
            if h.stale {
                s.push_str(&t!("p2p.sheet_snapshot_stale"));
            }
            if h.pinned {
                s.push_str(&t!("p2p.sheet_snapshot_pinned"));
            }
            s
        }
    };
    fact(frame, inner.y + 5, t!("p2p.sheet_snapshot").to_string(), snapshot, Style::default());
    let (fed, fed_style) = match room.relation(&peer.from) {
        Relation::None => ("—".to_string(), dim()),
        Relation::Federated => (t!("p2p.sheet_fed_federated").to_string(), Style::default().fg(th().ok)),
        Relation::Theirs => (t!("p2p.sheet_fed_theirs").to_string(), Style::default().fg(th().gold)),
        Relation::Sent => (t!("p2p.sheet_fed_sent").to_string(), Style::default()),
    };
    fact(frame, inner.y + 6, t!("p2p.sheet_federation").to_string(), fed, fed_style);

    let list_y = inner.y + 8;
    for (i, (key, label, _)) in menu.iter().enumerate() {
        let rect = line(list_y + i as u16);
        let selected = i == sel;
        let hovered = !selected && room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {key}  "), style.add_modifier(Modifier::BOLD)),
                Span::styled(label.clone(), style),
            ]))
            .style(style),
            rect,
        );
        // A click on the cursor row runs it; elsewhere it moves the cursor.
        room.cells.ui.click(rect, if selected { Act::SheetRun } else { Act::SheetPick(i) });
    }
}

/// The federate compose: message, the offer as checkboxes, Send.
fn draw_compose(frame: &mut Frame, room: &mut Room, area: Rect, c: &Compose) {
    let lib_rows = c.libraries.len().div_ceil(3).max(1) as u16;
    let inner = kit::modal_frame(frame, area, 68, 14 + lib_rows, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.compose_title", name = c.name).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::ComposeCancel);
    for (i, key) in ["p2p.compose_1", "p2p.compose_2", "p2p.compose_3"].iter().enumerate() {
        frame.render_widget(Paragraph::new(Span::styled(t!(*key).to_string(), dim())), line(inner.y + 1 + i as u16));
    }
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.compose_message", n = c.message.value().chars().count()).to_string(), dim())),
        line(inner.y + 5),
    );
    let field = Rect { x: inner.x + 1, y: inner.y + 6, width: inner.width.saturating_sub(2), height: 3 };
    let focused = c.focus.is_none();
    let hover = room.cells.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let field_inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if focused {
        let (x, w) = (field_inner.x + 1, field_inner.width.saturating_sub(2));
        kit::field_display(&mut room.cells.ui, x, field_inner.y, c.message.value(), c.message.cursor(), w, None)
    } else {
        c.message.value().to_string()
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: field_inner.x + 1, y: field_inner.y, width: field_inner.width.saturating_sub(2), height: 1 });
    room.cells.ui.click(field, Act::ComposeFocusMessage);

    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.compose_libraries").to_string(), dim())), line(inner.y + 10));
    let libs_y = inner.y + 11;
    if !c.loaded {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.compose_loading").to_string(), dim())), line(libs_y));
    } else if c.libraries.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.compose_none").to_string(), dim())), line(libs_y));
    }
    let col_w = inner.width.saturating_sub(2) / 3;
    for (i, (name, on)) in c.libraries.iter().enumerate() {
        let rect = Rect { x: inner.x + 1 + (i as u16 % 3) * col_w, y: libs_y + i as u16 / 3, width: col_w.saturating_sub(1), height: 1 };
        let focused = c.focus == Some(i);
        let hovered = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph = if *on {
            Span::styled(format!("{} ", g("[✓]", "[x]")), if focused { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default().fg(th().ok) })
        } else {
            Span::styled("[ ] ", if focused { Style::default().fg(th().on_accent).bg(th().accent) } else { dim() })
        };
        let name_style = if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if focused { bold() } else { Style::default() };
        frame.render_widget(Paragraph::new(Line::from(vec![glyph, Span::styled(name.clone(), name_style)])), rect);
        room.cells.ui.click(rect, Act::ComposeToggle(i));
    }
    if let Some(err) = &c.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            line(inner.bottom().saturating_sub(2)),
        );
    }
    let label = t!("p2p.compose_send").to_string();
    let x = inner.right().saturating_sub(kit::width(&label) as u16 + 4);
    kit::button(frame, &mut room.cells.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, c.loaded, Act::ComposeSend);
}

/// A labelled 3-row input inside a modal; returns the rect it took.
fn modal_field<A: Clone>(
    frame: &mut Frame,
    ui: &mut Surface<A>,
    at: Rect,
    label: &str,
    input: &Input,
    focused: bool,
    act: A,
) -> Rect {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if focused {
        kit::field_display(ui, inner.x + 1, inner.y, input.value(), input.cursor(), inner.width.saturating_sub(2), None)
    } else {
        let w = inner.width.saturating_sub(2) as usize;
        let v = input.value();
        if v.chars().count() > w { format!("{}…", v.chars().take(w.saturating_sub(1)).collect::<String>()) } else { v.to_string() }
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 });
    ui.click(field, act);
    Rect { x: at.x, y: at.y, width: at.width, height: 4 }
}

/// The identity modal: the two fields the network sees.
fn draw_identity(frame: &mut Frame, room: &mut Room, area: Rect, d: &IdentityDraft) {
    let inner = kit::modal_frame(frame, area, 68, 17, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.identity_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::IdentityCancel);
    let at = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 4 };
    modal_field(
        frame,
        &mut room.cells.ui,
        at(inner.y + 2),
        &t!("p2p.identity_name", n = d.name.value().chars().count()),
        &d.name,
        !d.on_description,
        Act::IdentityFocus(false),
    );
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.identity_name_hint").to_string(), dim())), line(inner.y + 6));
    modal_field(
        frame,
        &mut room.cells.ui,
        at(inner.y + 8),
        &t!("p2p.identity_description", n = d.description.value().chars().count()),
        &d.description,
        d.on_description,
        Act::IdentityFocus(true),
    );
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.identity_description_hint").to_string(), dim())), line(inner.y + 12));
    if let Some(err) = &d.error {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))), line(inner.y + 14));
    }
    let label = t!("p2p.identity_save").to_string();
    let x = inner.right().saturating_sub(kit::width(&label) as u16 + 4);
    kit::button(frame, &mut room.cells.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, true, Act::IdentitySave);
}

/// One numeric setting: its title, the webapp's helper text, one field.
fn draw_setting(frame: &mut Frame, room: &mut Room, area: Rect, d: &SettingDraft) {
    let (title, label, help) = match d.which {
        DiscoverySetting::MaxStorageMb => ("p2p.setting_storage_title", "p2p.setting_storage_label", "p2p.setting_storage_help"),
        DiscoverySetting::PeerRetentionDays => ("p2p.setting_retention_title", "p2p.setting_retention_label", "p2p.setting_retention_help"),
        DiscoverySetting::AutoFetchCount => ("p2p.setting_autofetch_title", "p2p.setting_autofetch_label", "p2p.setting_autofetch_help"),
        DiscoverySetting::RotationDays => ("p2p.setting_rotation_title", "p2p.setting_rotation_label", "p2p.setting_rotation_help"),
        DiscoverySetting::SidecarMaxRssMb => ("p2p.setting_memory_title", "p2p.setting_memory_label", "p2p.setting_memory_help"),
    };
    let inner = kit::modal_frame(frame, area, 68, 15, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!(title).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::SettingCancel);
    frame.render_widget(
        Paragraph::new(Span::styled(t!(help).to_string(), dim())).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y + 2, width: inner.width.saturating_sub(2), height: 4 },
    );
    modal_field(
        frame,
        &mut room.cells.ui,
        Rect { x: inner.x + 1, y: inner.y + 7, width: 24.min(inner.width.saturating_sub(2)), height: 4 },
        &t!(label),
        &d.value,
        true,
        Act::SettingSave,
    );
    if let Some(err) = &d.error {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))), line(inner.y + 12));
    }
    let save = t!("p2p.setting_save").to_string();
    let x = inner.right().saturating_sub(kit::width(&save) as u16 + 4);
    kit::button(frame, &mut room.cells.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &save, true, Act::SettingSave);
}

/// A warning gate: gold, consequences before verbs, the safe choice as
/// the primary, no [X].
fn draw_gate(frame: &mut Frame, room: &mut Room, area: Rect, title: String, body: Vec<String>, safe: (String, Act), go: (String, Act)) {
    let inner = kit::modal_frame(frame, area, 68, 6 + body.len() as u16, th().gold);
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

fn draw_block(frame: &mut Frame, room: &mut Room, area: Rect, id: &str) {
    let name = room.peer(id).map(display_name).unwrap_or_else(|| short_id(id));
    draw_gate(
        frame,
        room,
        area,
        t!("p2p.block_title", name = name).to_string(),
        vec![t!("p2p.block_1").to_string(), t!("p2p.block_2").to_string()],
        (t!("p2p.block_keep").to_string(), Act::BlockCancel),
        (t!("p2p.block_confirm").to_string(), Act::BlockConfirm),
    );
}

fn draw_leave(frame: &mut Frame, room: &mut Room, area: Rect) {
    draw_gate(
        frame,
        room,
        area,
        t!("p2p.leave_title").to_string(),
        vec![t!("p2p.leave_1").to_string(), t!("p2p.leave_2").to_string()],
        (t!("p2p.leave_stay").to_string(), Act::LeaveCancel),
        (t!("p2p.leave_confirm").to_string(), Act::LeaveConfirm),
    );
}

/// The blocked list: endpoint ids, Enter unblocks the cursor row.
fn draw_blocked(frame: &mut Frame, room: &mut Room, area: Rect, sel: usize) {
    let blocked = room.status.as_ref().map(|s| s.blocked_peers.clone()).unwrap_or_default();
    let inner = kit::modal_frame(frame, area, 74, 6 + blocked.len().max(1) as u16, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.blocked_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::BlockedClose);
    if blocked.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.blocked_empty").to_string(), dim())), line(inner.y + 2));
    }
    for (i, id) in blocked.iter().enumerate() {
        let rect = line(inner.y + 2 + i as u16);
        let selected = i == sel;
        let hovered = !selected && room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(Span::styled(format!(" {}", printable(id, 64)), style)).style(style), rect);
        room.cells.ui.click(rect, if selected { Act::BlockedRun } else { Act::BlockedPick(i) });
    }
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.blocked_unblock").to_string(), dim())).alignment(Alignment::Right),
        line(inner.bottom().saturating_sub(1)),
    );
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    use super::super::tests::*;
    use super::*;
    use crate::api::types::DiscoveryRecovery;

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

    #[test]
    fn off_the_network_the_page_joins_at_once_with_the_inbox_on_by_default() {
        let _en = english();
        let mut room = room();
        room.apply(loaded(status_off()));
        let frame = draw(&mut room);
        assert!(frame.contains("• not on the discovery network"), "{frame}");
        assert!(frame.contains("Join the discovery network"), "{frame}");
        assert!(frame.contains("[✓] Also let other servers send me federation requests"), "{frame}");
        assert!(frame.contains("Never any audio files."), "{frame}");
        assert!(frame.contains("• not on the discovery network · The p2p-sidecar will be downloaded when you join."), "{frame}");
        assert!(frame.contains("Enter join the network · Space federation requests · Esc back"), "{frame}");
        // Enter joins with the inbox — no gate in between.
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::JoinNetwork { accept_requests: true }));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char(' ')));
        assert!(draw(&mut room).contains("[ ] Also let"));
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::JoinNetwork { accept_requests: false }));
        room.queued = None;
        // The answer: a note, a reload, and the identity modal once the
        // network shows as on — the webapp's straight-into-naming move.
        room.apply(Done::Joined(Ok(serde_json::json!({ "acceptRequests": true }))));
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n.contains("give the mesh a minute") && n.contains("inbox")));
        assert!(matches!(room.queued, Some(Op::Load { .. })));
        room.queued = None;
        room.apply(loaded(status_on()));
        assert!(matches!(room.modal, Modal::Identity(_)));
        // A partial failure says exactly which half failed.
        let mut room = room_after_join();
        room.apply(Done::Joined(Ok(serde_json::json!({ "federationError": "iroh has no build here" }))));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("iroh has no build here")));
    }

    #[test]
    fn the_state_row_says_the_mesh_state_in_the_webapps_words() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains("• connected — 3 mesh neighbors · sidecar 19 MB · announcing as mStream Den"), "{frame}");
        assert!(frame.contains("polls every 10 s"), "{frame}");
        assert!(!frame.contains("searching for peers"), "{frame}");

        let mut s = status_on();
        s.neighbors = 0;
        s.recovery = DiscoveryRecovery { attempts: 2, retry_pending: true };
        s.watchdog.restarts = 1;
        room.apply(loaded(s));
        let frame = draw(&mut room);
        assert!(frame.contains("• reconnecting — sidecar died, replaying (attempt 2) · sidecar 19 MB (1 watchdog restart)"), "{frame}");
        assert!(!frame.lines().nth(2).unwrap_or_default().contains("announcing"), "the name drops first when the row is too wide\n{frame}");
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ crash recovery replays the stack"), "{frame}");

        let mut s = status_on();
        s.neighbors = 0;
        room.apply(loaded(s));
        let frame = draw(&mut room);
        assert!(frame.contains("• joined, waiting for neighbors"), "{frame}");
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ searching for peers — this room updates itself every 10 s"), "{frame}");

        let mut s = status_on();
        s.neighbors = 0;
        s.joined = false;
        room.apply(loaded(s));
        assert!(draw(&mut room).contains("• not joined yet"));

        let mut s = status_on();
        s.binary_found = false;
        room.apply(loaded(s));
        assert!(draw(&mut room).contains("• unavailable — the p2p-sidecar binary was not found"));
    }

    #[test]
    fn the_table_derives_its_columns_from_the_catalog_and_the_requests() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        for word in ["SERVERS YOU FOLLOW", "SERVER", "TRACKS", "SEEDERS", "ONLINE", "SNAPSHOT", "FEDERATION"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(frame.contains("/ filter · 1 hidden — incompatible model · h show · 1 blocked · u unblock"), "{frame}");
        let row = |name: &str| frame.lines().find(|l| l.starts_with(&format!("  {name}"))).map(str::to_string).unwrap_or_default();
        let basement = row("Basement Archive");
        assert!(basement.contains("12,204") && basement.contains("online") && basement.contains("downloaded") && basement.contains("federated"), "{basement}");
        let vinyl = row("Vinyl Rips '94");
        assert!(vinyl.contains("downloaded · pinned") && vinyl.contains(" —"), "{vinyl}");
        let friend = row("friend-node");
        assert!(friend.contains("update available") && friend.contains("they asked you"), "{friend}");
        let jazz = row("jazz-corner");
        assert!(jazz.contains("offline · 3d") && jazz.contains("—"), "{jazz}");
        let plain = row("mStream ");
        assert!(plain.contains("offline · 12d") && plain.contains("request sent"), "{plain}");
        assert!(frame.contains("↑↓ select · ←→ tab · / filter · h hidden · e name · x leave · Esc back"), "{frame}");
        // The cursor row's description rides the note line.
        handle_key(&mut room, key(KeyCode::Down));
        let frame = draw(&mut room);
        assert!(frame.contains("Basement Archive · Northern soul, dub and a wall of 7-inch singles."), "{frame}");
        assert!(frame.contains("↑↓ rows · Enter details · d download"), "{frame}");
        // Relations, by the webapp's priority.
        assert_eq!(room.relation(&hex("8f31c0e2a7")), Relation::Federated);
        assert_eq!(room.relation(&hex("c4a8f0e19d")), Relation::Theirs);
        assert_eq!(room.relation(&hex("05d6e8f2ac")), Relation::Sent);
        assert_eq!(room.relation(&hex("7e11aa93b0")), Relation::None);
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
        let tabs: Vec<(Tab, String)> = Tab::ALL.iter().map(|t| (*t, format!(" {} ", t.label()))).collect();
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
    fn the_stat_tiles_add_up_and_the_tabs_switch_with_the_arrows() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains(" Stats "), "{frame}");
        for word in ["3 of 6", "snapshots held", "23.0 MB of 500.0 MB", "24,569", "peer tracks searchable", "across 3 held libraries", "6", "servers known", "1 hidden · 1 blocked", "19 MB", "ceiling 256 MB · 0 restarts", "seeding"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Activity);
        assert_eq!(room.queued, Some(Op::Activity(0)));
        room.queued = None;
        let now = unix_now();
        let entry = |seq: u64, ago: i64, level: &str, message: &str| ActivityEntry {
            seq,
            t: iso_at(now - ago),
            level: level.into(),
            message: message.into(),
        };
        room.apply(Done::Activity(Ok(DiscoveryActivity {
            entries: vec![
                entry(1, 3 * 3600, "info", "sidecar started (pid 48211)"),
                entry(2, 61 * 60, "warn", "rotation found nothing fetchable under the cap"),
                entry(3, 3 * 60, "info", "Basement Archive announced a new snapshot (seq 40 -> 41)"),
            ],
            last_seq: 3,
        })));
        let frame = draw(&mut room);
        let newest = frame.lines().position(|l| l.contains("announced a new snapshot")).unwrap();
        let oldest = frame.lines().position(|l| l.contains("sidecar started")).unwrap();
        assert!(newest < oldest, "newest first\n{frame}");
        assert!(frame.contains("       3m  Basement Archive announced"), "{frame}");
        assert!(frame.contains("newest first · full history in the server logs"), "{frame}");
        // A restarted server resets its seq: the history starts over.
        room.apply(Done::Activity(Ok(DiscoveryActivity { entries: vec![entry(1, 10, "info", "fresh boot")], last_seq: 1 })));
        assert_eq!(room.activity.len(), 1);
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Invite);
        let frame = draw(&mut room);
        assert!(frame.contains("ENDPOINT") && frame.contains(&hex("51403e7b9c")), "{frame}");
        assert!(frame.contains("YOUR TICKET") && frame.contains("nodeaa4qk3r7z2nlxwm5c6bdhpj2ev5gktr3q7hifobyq2j5ru4kiggizaaaqbxk3ewgk6wxjm"), "{frame}");
        assert!(frame.contains("BEFRIEND A SERVER") && frame.contains("y copy"), "{frame}");
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Config);
        let frame = draw(&mut room);
        assert!(frame.contains("ANNOUNCING AS  mStream Den — Vinyl, jazz and field recordings"), "{frame}");
        assert!(frame.contains("1  Snapshot storage cap    500 MB"), "{frame}");
        assert!(frame.contains("2  Forget offline servers  after 30 days of silence"), "{frame}");
        assert!(frame.contains("3  Auto-fetch snapshots    up to 6 servers"), "{frame}");
        assert!(frame.contains("4  Rotate downloads        after 7 days"), "{frame}");
        assert!(frame.contains("5  Sidecar memory ceiling  256 MB"), "{frame}");
        assert!(frame.contains("   Community seeds         on"), "{frame}");
        assert!(frame.contains("1–5 edit a setting"), "{frame}");
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Stats, "the tabs wrap");
        handle_key(&mut room, key(KeyCode::Left));
        assert_eq!(room.tab, Tab::Config);
    }

    #[test]
    fn the_sheet_lists_only_what_applies_and_its_letters_act() {
        let _en = english();
        let mut room = on();
        // Basement: held, federated, online → update, pin, remove, block.
        handle_key(&mut room, key(KeyCode::Down));
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Sheet { id, sel: 0 } = &room.modal else { panic!("the sheet, cursor on top") };
        let keys: Vec<char> = room.sheet_menu(room.peer(id).unwrap()).iter().map(|(k, _, _)| *k).collect();
        assert_eq!(keys, vec!['d', 'p', 'r', 'b']);
        let frame = draw(&mut room);
        assert!(frame.contains("Update the snapshot") && frame.contains("Pin it") && !frame.contains("Ask to federate"), "{frame}");
        assert!(frame.contains("downloaded 2d ago · 8.0 MB"), "{frame}");
        assert!(frame.contains("12,204 tracks · 3 seeders"), "{frame}");
        assert!(frame.contains("↑↓ choose · Enter act · Esc close"), "{frame}");
        // Enter on the cursor row runs it.
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None));
        assert!(matches!(room.queued, Some(Op::Fetch { .. })));
        room.queued = None;
        // jazz-corner: offline, nothing held, no relation → download,
        // federate, forget, block.
        for _ in 0..3 {
            handle_key(&mut room, key(KeyCode::Down));
        }
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Sheet { id, .. } = &room.modal else { panic!("the sheet") };
        let keys: Vec<char> = room.sheet_menu(room.peer(id).unwrap()).iter().map(|(k, _, _)| *k).collect();
        assert_eq!(keys, vec!['d', 'f', 'g', 'b']);
        // The letters work inside the sheet, and block is gated.
        handle_key(&mut room, key(KeyCode::Char('b')));
        assert!(matches!(room.modal, Modal::Block(_)));
        let frame = draw(&mut room);
        assert!(frame.contains("Block jazz-corner?") && frame.contains("◂ Keep") && frame.contains("y block · Esc keep"), "{frame}");
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        handle_key(&mut room, key(KeyCode::Char('b')));
        handle_key(&mut room, key(KeyCode::Char('y')));
        assert_eq!(
            room.queued,
            Some(Op::Peer { action: PeerAction::Block, id: hex("7e11aa93b0"), name: "jazz-corner".into() })
        );
    }

    #[test]
    fn row_keys_act_on_the_cursor_row_and_the_compose_offers_every_library() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Down)); // Basement: federated
        handle_key(&mut room, key(KeyCode::Char('f')));
        assert!(matches!(room.modal, Modal::None), "no compose for a server already federated with");
        handle_key(&mut room, key(KeyCode::Down)); // Vinyl: held, pinned
        handle_key(&mut room, key(KeyCode::Char('p')));
        assert_eq!(room.queued, Some(Op::Pin { id: hex("2b7d9e4c11"), pinned: false }));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('r')));
        assert!(matches!(room.queued, Some(Op::Peer { action: PeerAction::RemoveSnapshot, .. })));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('g')));
        assert!(room.queued.is_none(), "an online, held server cannot be forgotten");
        handle_key(&mut room, key(KeyCode::Down));
        handle_key(&mut room, key(KeyCode::Down)); // jazz-corner
        handle_key(&mut room, key(KeyCode::Char('g')));
        assert!(matches!(room.queued, Some(Op::Peer { action: PeerAction::Forget, .. })));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('f')));
        assert!(matches!(room.modal, Modal::Federate(_)));
        assert_eq!(room.queued, Some(Op::Libraries));
        room.queued = None;
        room.apply(Done::Libraries(Ok(vec!["music".into(), "vinyl".into()])));
        type_text(&mut room, "Swap jazz for field recordings?");
        let frame = draw(&mut room);
        assert!(frame.contains("Ask jazz-corner to federate"), "{frame}");
        assert!(frame.contains("MESSAGE · optional · 31 / 500"), "{frame}");
        assert!(frame.contains("[✓] music") && frame.contains("[✓] vinyl"), "{frame}");
        handle_key(&mut room, key(KeyCode::Tab));
        handle_key(&mut room, key(KeyCode::Tab));
        handle_key(&mut room, key(KeyCode::Char(' ')));
        assert!(draw(&mut room).contains("[ ] vinyl"));
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(
            room.queued,
            Some(Op::SendRequest {
                id: hex("7e11aa93b0"),
                name: "jazz-corner".into(),
                offer: vec!["music".into()],
                message: "Swap jazz for field recordings?".into(),
            })
        );
        room.queued = None;
        room.apply(Done::RequestSent {
            name: "jazz-corner".into(),
            result: Err(ApiError::Server { status: 409, message: "already asked".into() }),
        });
        let Modal::Federate(c) = &room.modal else { panic!("a refusal keeps the compose") };
        assert!(c.error.as_deref().is_some_and(|e| e.contains("already asked")));
        room.apply(Done::RequestSent { name: "jazz-corner".into(), result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("jazz-corner will see it")));
    }

    #[test]
    fn settings_open_by_digit_and_refuse_what_the_server_would() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('3')));
        let Modal::Setting(d) = &room.modal else { panic!("the setting modal") };
        assert_eq!(d.which, DiscoverySetting::AutoFetchCount);
        assert_eq!(d.value.value(), "6");
        handle_key(&mut room, key(KeyCode::Backspace));
        type_text(&mut room, "9x9");
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Setting(d) = &room.modal else { panic!("still the setting modal") };
        assert_eq!(d.value.value(), "99", "only digits reach the field");
        assert_eq!(d.error.as_deref(), Some("enter a whole number from 0 to 50"));
        assert!(room.queued.is_none());
        handle_key(&mut room, key(KeyCode::Backspace));
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::Setting { which: DiscoverySetting::AutoFetchCount, value: 9 }));
        room.queued = None;
        room.apply(Done::Setting { which: DiscoverySetting::AutoFetchCount, value: 9, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(room.status.as_ref().unwrap().auto_fetch_count, 9);
        let frame = draw(&mut room);
        assert!(frame.contains("setting saved"), "{frame}");
    }

    #[test]
    fn the_befriend_box_and_the_filter_take_the_keys_only_while_focused() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('j')));
        assert_eq!(room.tab, Tab::Invite);
        assert!(room.ticket_focus);
        type_text(&mut room, "nodeab7fj3pq2xw");
        assert!(room.sel.is_none(), "typing never moved the table cursor");
        let frame = draw(&mut room);
        assert!(frame.contains("nodeab7fj3pq2xw▏"), "{frame}");
        assert!(frame.contains("type or paste the ticket · Enter join · Esc leave the field"), "{frame}");
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::Befriend("nodeab7fj3pq2xw".into())));
        room.queued = None;
        room.apply(Done::Befriended(Ok(())));
        assert!(!room.ticket_focus && room.ticket.value().is_empty());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("within a minute")));

        handle_key(&mut room, key(KeyCode::Char('/')));
        assert!(room.filter_focus);
        type_text(&mut room, "jazz");
        assert_eq!(room.rows().len(), 1);
        assert!(draw(&mut room).contains("/ jazz▏"));
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(!room.filter_focus && room.filter_on);
        handle_key(&mut room, key(KeyCode::Down));
        assert_eq!(room.selected_peer().map(display_name).as_deref(), Some("jazz-corner"));
        handle_key(&mut room, key(KeyCode::Esc)); // stows the cursor
        handle_key(&mut room, key(KeyCode::Esc)); // clears the filter
        assert!(!room.filter_on);
        assert_eq!(room.rows().len(), 5);
        assert!(matches!(handle_key(&mut room, key(KeyCode::Esc)), Some(Outcome::Quit)));
    }

    #[test]
    fn leaving_is_gated_and_load_errors_name_the_gate_that_bit() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('x')));
        assert!(matches!(room.modal, Modal::Leave));
        let frame = draw(&mut room);
        assert!(frame.contains("Leave the discovery network?") && frame.contains("◂ Stay") && frame.contains("y leave · Esc stay"), "{frame}");
        handle_key(&mut room, key(KeyCode::Esc));
        assert!(room.queued.is_none());
        handle_key(&mut room, key(KeyCode::Char('x')));
        handle_key(&mut room, key(KeyCode::Char('y')));
        assert_eq!(room.queued, Some(Op::LeaveNetwork));
        room.queued = None;
        room.apply(Done::Left(Ok(())));
        assert!(matches!(room.queued, Some(Op::Load { .. })));
        room.apply(loaded(status_off()));
        assert!(!room.enabled() && room.sel.is_none());

        let mut room = room_after_join();
        room.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("locked")));
        room.apply(Done::Loaded(Err(ApiError::Unauthorized)));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("login")));
    }

    #[test]
    fn the_blocked_list_unblocks_and_the_hidden_toggle_reloads() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('u')));
        assert!(matches!(room.modal, Modal::Blocked(0)));
        let frame = draw(&mut room);
        assert!(frame.contains("Blocked servers") && frame.contains(&hex("a1b2c3d4e5")), "{frame}");
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(
            room.queued,
            Some(Op::Peer { action: PeerAction::Unblock, id: hex("a1b2c3d4e5"), name: "a1b2c3d4e5a1…".into() })
        );
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('h')));
        assert_eq!(room.queued, Some(Op::Load { include_incompatible: true, activity_since: None }));
        assert!(draw(&mut room).contains("showing incompatible servers · h hide"));
    }

    #[test]
    fn a_frame_reads_the_catalog_where_it_lives_and_keeps_the_status() {
        // The table draws each row from the catalog by its place, and the
        // page from the status lent for the frame — no copies made every
        // frame (performance audit #110). The filter matches whatever the
        // case, and the cursor's note is the filtered row's.
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('/')));
        type_text(&mut room, "JAZZ");
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.rows().len(), 1);
        handle_key(&mut room, key(KeyCode::Down));
        let frame = draw(&mut room);
        assert!(frame.contains("jazz-corner"), "{frame}");
        assert!(room.status.is_some(), "the status is back after the frame");
        let peer = room.selected_peer().expect("the cursor's peer");
        assert_eq!(display_name(peer), "jazz-corner");
        if !peer.payload.description.trim().is_empty() {
            assert!(frame.contains(&format!("jazz-corner · {}", printable(&peer.payload.description, DESCRIPTION_MAX))), "{frame}");
        }
    }

    // ── Hosted in the GUI player's Admin tab ─────────────────────────────

    use crate::admin::drive_pointer;
    use crate::admin::hosting::{self, DOCKED, FLOOR, WINDOW};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};

    /// The cells `from..to` of row `y`, as text.
    fn cells(buf: &Buffer, y: u16, from: u16, to: u16) -> String {
        (from..to).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The whole buffer as text, a row a line.
    fn text_of(buf: &Buffer) -> String {
        (buf.area.top()..buf.area.bottom()).map(|y| hosting::row(buf, y)).collect::<Vec<_>>().join("\n")
    }

    /// A wheel turn at (x, y), through the hub's own pointer routine.
    fn wheel_at(room: &mut Room, up: bool, x: u16, y: u16) {
        let kind = if up { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown };
        drive_pointer(room, MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    #[test]
    fn hosted_the_room_draws_inside_its_area_with_no_header_and_its_note_last() {
        let _en = english();
        for (size, area) in [((100, 30), WINDOW), ((176, 46), DOCKED)] {
            let mut room = on();
            room.note = Some(("the mesh answered".into(), false));
            let buf = hosting::draw_hosted(&mut room, size, area);
            let text = text_of(&buf);
            assert_eq!(hosting::outside(&buf, area, true), Vec::<(u16, u16)>::new(), "{size:?}\n{text}");
            assert!(!text.contains(&*t!("p2p.title")) && !text.contains("home.mstream.example"), "no header\n{text}");
            assert!(!text.contains(&*t!("p2p.hint_rows")), "the tips are the host's\n{text}");
            assert_eq!(cells(&buf, area.y, area.x, area.right()), "·".repeat(area.width as usize), "the first row is the host's");
            assert!(cells(&buf, area.y + 1, area.x + 2, area.right()).starts_with(&*t!("p2p.state_connected")), "{text}");
            assert!(cells(&buf, area.bottom() - 1, area.x + 2, area.right()).starts_with("the mesh answered"), "{text}");
            assert!(text.contains("snapshots held") && text.contains("Basement Archive"), "{text}");
        }
    }

    #[test]
    fn hosted_a_modal_centres_in_the_rooms_area() {
        let _en = english();
        let mut room = on();
        room.modal = Modal::Leave;
        let buf = hosting::draw_hosted(&mut room, (100, 30), WINDOW);
        // The leave gate is 68 by 8: a title, a blank, two lines, the buttons.
        let at = kit::modal_rect(WINDOW, 68, 8, 8);
        assert_eq!((at.x, at.y), (24, 8));
        assert_eq!(buf[(at.x, at.y)].symbol(), "╭");
        assert_eq!(buf[(at.right() - 1, at.bottom() - 1)].symbol(), "╯");
        assert_eq!(hosting::outside(&buf, WINDOW, true), Vec::<(u16, u16)>::new());
    }

    #[test]
    fn hosted_at_the_floor_it_keeps_its_pane_and_shortens_the_table() {
        let _en = english();
        let mut room = on();
        let text = text_of(&hosting::draw_hosted(&mut room, (100, 30), WINDOW));
        assert!(text.contains("Basement Archive"), "the table has rows in the window\n{text}");

        let mut room = on();
        let buf = hosting::draw_hosted(&mut room, (100, 24), FLOOR);
        let text = text_of(&buf);
        assert_eq!(hosting::outside(&buf, FLOOR, false), Vec::<(u16, u16)>::new(), "{text}");
        assert_eq!(room.cells.pane_at, Some(Rect { x: 19, y: 6, width: 79, height: PANE_ROWS }));
        assert!(text.contains("snapshots held") && text.contains("servers known"), "the pane keeps its rows\n{text}");
        assert!(!text.contains("Basement Archive"), "the table gives way first\n{text}");
        assert!(!text.contains(&*t!("resize")), "{text}");
    }

    #[test]
    fn the_activity_wheel_follows_the_drawn_pane_wherever_the_room_sits() {
        let _en = english();
        let lower = Rect { y: 9, ..WINDOW };
        for (size, area) in [((100, 30), WINDOW), ((100, 40), lower)] {
            let mut room = on();
            room.tab = Tab::Activity;
            hosting::draw_hosted(&mut room, size, area);
            let pane = room.cells.pane_at.expect("the pane was drawn");
            assert_eq!(pane.y, area.y + 5, "state row, bar, tabs, gap: {area:?}");
            wheel_at(&mut room, true, 40, pane.y);
            wheel_at(&mut room, true, 40, pane.bottom() - 1);
            assert_eq!((room.ascroll, room.tscroll), (2, 0), "{area:?}");
            wheel_at(&mut room, false, 40, pane.bottom());
            wheel_at(&mut room, false, 40, pane.y - 1);
            assert_eq!((room.ascroll, room.tscroll), (2, 2), "{area:?}");
        }
        // Sitting lower, the rows a standalone pane would hold are above
        // this one: the wheel there is the table's.
        let mut room = on();
        room.tab = Tab::Activity;
        hosting::draw_hosted(&mut room, (100, 40), lower);
        wheel_at(&mut room, false, 40, 6);
        assert_eq!((room.ascroll, room.tscroll), (0, 1));
    }

    #[test]
    fn the_wheel_over_the_activity_pane_still_walks_the_log_standalone() {
        let _en = english();
        let mut room = on();
        room.tab = Tab::Activity;
        draw(&mut room);
        assert_eq!(room.cells.pane_at, Some(Rect { x: 2, y: 6, width: 96, height: PANE_ROWS }));
        wheel_at(&mut room, true, 40, 6);
        wheel_at(&mut room, true, 40, 6 + PANE_ROWS - 1);
        assert_eq!((room.ascroll, room.tscroll), (2, 0));
        wheel_at(&mut room, false, 40, 6 + PANE_ROWS);
        assert_eq!((room.ascroll, room.tscroll), (2, 1));
        room.tab = Tab::Stats;
        wheel_at(&mut room, false, 40, 7);
        assert_eq!((room.ascroll, room.tscroll), (2, 2), "only the Activity pane walks the log");
    }

    /// The cell a focused field notes is the caret's however the field's
    /// window over a long value stands (hosted, it is where the GUI's window
    /// floats the input method's candidates, and what turns its paste on).
    /// The filter is right-aligned with what follows it, so it notes its
    /// caret only once its line is measured and placed.
    #[test]
    fn the_filter_the_ticket_box_and_the_forms_field_note_the_cell_their_caret_is_drawn_in() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('/')));
        type_text(&mut room, &format!("{}end", "z".repeat(40)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "zzzend", " · ");
        handle_key(&mut room, key(KeyCode::Home));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "/ ", "zzz");
        handle_key(&mut room, key(KeyCode::Enter));
        draw(&mut room);
        assert_eq!(room.cells.ui.caret_at(), None, "the filter let go of the keys");

        handle_key(&mut room, key(KeyCode::Char('j')));
        type_text(&mut room, &format!("node{}end", "a".repeat(120)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "aaaend", "");
        handle_key(&mut room, key(KeyCode::Esc));
        draw(&mut room);
        assert_eq!(room.cells.ui.caret_at(), None, "the box let go of the keys");

        // The name's 64 characters, past its field's 60 cells.
        handle_key(&mut room, key(KeyCode::Char('e')));
        type_text(&mut room, &format!(" {}-end", "d".repeat(48)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "ddd-end", "");
    }
}
