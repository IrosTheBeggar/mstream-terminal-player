//! The Torrents room in character cells: what `mstream-player admin
//! torrents` draws in a terminal, and what the GUI player's Admin tab
//! draws on its grid when it hosts the room ([`Screen::render_hosted`]).
//! The room itself (its state, its worker, its keys, its words) is the
//! parent module; this is its view in cells, and holds what is measured
//! in cells: the layout, the setup page's radio groups and form, the
//! tabs' tables and the modals, on the kit's surface, and the chips'
//! glyphs and colours. It is also where the room meets the hub's loop,
//! as a [`Screen`].

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::super::{Screen, body_column, draw_chip, draw_foot, draw_header, fmt_bytes, frame_ground, host_of};
use super::*;
use crate::kit::theme::th;
use crate::kit::{self, Surface, bold, dim};
use crate::setup::g;

const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
const TAB_GAP: u16 = 2;

/// What this view keeps on the room between frames. The room holds one
/// and never looks inside it.
pub(super) struct State {
    /// The kit's interaction surface: what the last frame registered.
    ui: Surface<Act>,
}

impl State {
    pub(super) fn new() -> State {
        State { ui: Surface::new() }
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

// ── The chips' glyphs and colours ────────────────────────────────────────────

/// The chip colours in the kit's: downloading accent, seeding green, the
/// idle states dim, the worrying ones gold.
fn status_style(status: &str) -> Style {
    match status {
        "downloading" => Style::default().fg(th().accent),
        "seeding" => Style::default().fg(th().ok),
        "verifying" | "error" => Style::default().fg(th().gold),
        _ => dim(),
    }
}

fn confidence_glyph(c: &str) -> &'static str {
    match c {
        "verified" => g("✓", "+"),
        "inferred" => "~",
        "pending" => g("⟳", "~"),
        _ => g("✗", "x"),
    }
}

fn confidence_style(c: &str) -> Style {
    match c {
        "verified" => Style::default().fg(th().ok),
        "inferred" => Style::default().fg(th().gold),
        "pending" => Style::default().fg(th().accent),
        _ => Style::default().fg(th().gold),
    }
}

/// The seed-existing outcome as the webapp's chip: glyph, word, style.
pub(crate) fn outcome_words(o: &str) -> (String, Style) {
    let (glyph, word, style) = match o {
        "seeded" => (g("✓", "+"), t!("tor.out_seeded"), Style::default().fg(th().ok)),
        "match_unmapped" => ("!", t!("tor.out_match_unmapped"), Style::default().fg(th().gold)),
        "pad_files_missing" => ("!", t!("tor.out_pad_files_missing"), Style::default().fg(th().gold)),
        "partial_match" => ("~", t!("tor.out_partial_match"), Style::default().fg(th().gold)),
        "already_in_daemon" => (g("⊝", "="), t!("tor.out_already_in_daemon"), dim()),
        "no_match" => (g("✗", "x"), t!("tor.out_no_match"), dim()),
        "invalid_torrent" => (g("✗", "x"), t!("tor.out_invalid_torrent"), Style::default().fg(th().gold)),
        "daemon_error" => (g("✗", "x"), t!("tor.out_daemon_error"), Style::default().fg(th().gold)),
        other => ("", std::borrow::Cow::Owned(other.to_string()), dim()),
    };
    (format!("{glyph} {word}").trim().to_string(), style)
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.cells.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    draw(frame, room, area, false);
}

/// The room inside `area`. Standalone it owns the window: the header with
/// the beta chip beside the title, and the tips row. Hosted, the host's bar
/// names the server and its footer carries the tips, so the chip moves to
/// the right of the body's first row (the state line stops short of it)
/// and the room keeps only its note on the area's last row. Below its
/// minimum it draws what fits and never asks for a larger window.
fn draw(frame: &mut Frame, room: &mut Room, area: Rect, hosted: bool) {
    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.cells.ui.pointer;
    if modal_open {
        room.cells.ui.pointer = None;
    }

    let beta = t!("tor.beta").to_string();
    if !hosted {
        let title = t!("tor.title").to_string();
        draw_header(frame, area, &title, &host_of(&room.client));
        // In cells, so a Japanese title is not drawn over.
        let chip_x = area.x + 2 + kit::width(&title) as u16 + 1;
        frame.render_widget(
            Paragraph::new(Span::styled(beta.clone(), Style::default().fg(th().gold))),
            Rect { x: chip_x, y: area.y, width: area.width.saturating_sub(chip_x + 30), height: 1 },
        );
    }
    let column = body_column(area, hosted);
    // The state line heads every page that draws (not the loading one,
    // nor a connect page with no form yet). Hosted, it leaves the chip its
    // cells and a gap of two.
    let paged = match room.phase() {
        Phase::Loading => false,
        Phase::Connect => room.connect.is_some(),
        Phase::Choose | Phase::Tabs => true,
    };
    if paged {
        let reserve = if hosted { kit::width(&beta) as u16 + 2 } else { 0 };
        draw_state(frame, room, Rect { width: column.width.saturating_sub(reserve), ..column });
    }
    if hosted {
        draw_chip(frame, column, &beta);
    }
    match room.phase() {
        Phase::Loading => {}
        Phase::Choose => draw_choose(frame, room, column),
        Phase::Connect => {
            if let Some(c) = room.connect.clone() {
                draw_connect(frame, room, column, &c);
            }
        }
        Phase::Tabs => draw_tabs(frame, room, column),
    }
    draw_foot(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room), hosted);

    if modal_open {
        room.cells.ui.pointer = live_pointer;
        room.cells.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Mapping { vpath, path, error } => {
            let client = room.kind().label();
            let body = vec![Line::from(Span::styled(t!("tor.mapping_hint").to_string(), dim()))];
            draw_entry_modal(
                frame,
                room,
                area,
                t!("tor.mapping_title", name = printable(&vpath, NAME_MAX), client = client.clone()).to_string(),
                t!("tor.mapping_field", client = client.to_uppercase()).to_string(),
                &path,
                None,
                body,
                error.as_deref(),
                vec![(t!("tor.mapping_save").to_string(), true, Act::ModalSubmit)],
            );
        }
        Modal::Template { vpath, template, error } => {
            let vars = room.templates.supported_vars.iter().map(|v| format!("{{{{{v}}}}}")).collect::<Vec<_>>().join(" ");
            let preview = match preview_template(template.value(), &room.templates.sample_metadata) {
                None => (t!("tor.template_preview_none").to_string(), dim()),
                Some(p) if p.is_empty() => (t!("tor.template_preview_empty").to_string(), dim()),
                Some(p) => (p, bold()),
            };
            let body = vec![
                Line::from(vec![Span::styled(format!("{:<11}", t!("tor.template_vars")), dim()), Span::raw(vars)]),
                Line::from(Span::styled(t!("tor.template_vars_hint").to_string(), dim())),
                Line::from(""),
                Line::from(vec![Span::styled(format!("{:<11}", t!("tor.template_preview")), dim()), Span::styled(preview.0, preview.1)]),
                Line::from(Span::styled(t!("tor.template_preview_hint").to_string(), dim())),
            ];
            draw_entry_modal(
                frame,
                room,
                area,
                t!("tor.template_title", name = printable(&vpath, NAME_MAX)).to_string(),
                t!("tor.template_field").to_string(),
                &template,
                None,
                body,
                error.as_deref(),
                vec![
                    (t!("tor.template_suggested").to_string(), false, Act::TemplateSuggest),
                    (t!("tor.template_clear").to_string(), false, Act::TemplateClear),
                    (t!("tor.template_save").to_string(), true, Act::ModalSubmit),
                ],
            );
        }
        Modal::SeedPath { path, matches, error, asked } => {
            let mut body = vec![Line::from(Span::styled(t!("tor.seed_path_hint").to_string(), dim()))];
            if asked.is_some() && matches.is_empty() {
                body.push(Line::from(Span::styled(format!("  {}", t!("busy.listing")), dim())));
            }
            for m in &matches {
                body.push(Line::from(Span::styled(format!("  {} {}", g("▸", "►"), clip(m, 64)), dim())));
            }
            draw_entry_modal(
                frame,
                room,
                area,
                t!("tor.seed_path_title").to_string(),
                t!("tor.seed_path_field").to_string(),
                &path,
                Some("~/Downloads/album.torrent"),
                body,
                error.as_deref(),
                vec![(t!("tor.seed_path_button").to_string(), true, Act::ModalSubmit)],
            );
        }
        Modal::Remove(hash) => {
            let name = room.torrent(&hash).map(|t| printable(&t.name, NAME_MAX)).unwrap_or_else(|| short_id(&hash));
            draw_gate(
                frame,
                room,
                area,
                t!("tor.remove_title", name = name, client = room.kind().label()).to_string(),
                vec![t!("tor.remove_1").to_string(), t!("tor.remove_2").to_string()],
                (t!("tor.remove_keep").to_string(), Act::RemoveCancel),
                (t!("tor.remove_confirm").to_string(), Act::RemoveConfirm),
            );
        }
        Modal::Disconnect => draw_gate(
            frame,
            room,
            area,
            t!("tor.disconnect_title", client = room.kind().label()).to_string(),
            vec![t!("tor.disconnect_1").to_string(), t!("tor.disconnect_2").to_string()],
            (t!("tor.disconnect_keep").to_string(), Act::DisconnectCancel),
            (t!("tor.disconnect_confirm").to_string(), Act::DisconnectConfirm),
        ),
    }
    if let Some((target, text)) = room.cells.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// The state line for every page: the webapp's status card in one row.
fn state_spans(room: &Room) -> (Vec<Span<'static>>, Option<String>) {
    let kind = room.kind();
    if kind == Kind::Disabled {
        return (vec![Span::styled(t!("tor.state_off").to_string(), dim()), Span::raw(t!("tor.state_off_detail").to_string())], None);
    }
    let label = kind.label();
    if !room.config().is_some_and(|c| c.configured) {
        return (
            vec![
                Span::styled(t!("tor.state_picked", client = kind.as_str()).to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                Span::raw(t!("tor.state_picked_detail").to_string()),
            ],
            None,
        );
    }
    let polls = Some(if room.tab == Tab::Torrents && room.list_live() { t!("tor.polls") } else { t!("tor.polls_slow") }.to_string());
    match &room.status {
        None => (
            vec![
                Span::styled(t!("tor.state_asking", client = kind.as_str()).to_string(), dim().add_modifier(Modifier::BOLD)),
                Span::raw(t!("tor.state_asking_detail").to_string()),
            ],
            polls,
        ),
        Some(s) if s.connected => {
            let host = room.config().map(|c| format!("{}:{}", c.host, c.port)).unwrap_or_default();
            (
                vec![
                    Span::styled(t!("tor.state_connected").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
                    Span::raw(
                        t!(
                            "tor.state_connected_detail",
                            client = label,
                            version = version_words(s),
                            host = printable(&host, 80),
                            reachable = room.reachable(),
                            total = room.libraries.len()
                        )
                        .to_string(),
                    ),
                ],
                polls,
            )
        }
        Some(s) => (
            vec![
                Span::styled(t!("tor.state_down").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                Span::raw(t!("tor.state_down_detail", reason = printable(s.reason.as_deref().unwrap_or(""), 120)).to_string()),
            ],
            polls,
        ),
    }
}

/// The state line on `column`'s first row, its poll note right-aligned
/// when both fit. [`draw`] calls it for every page, so a hosted room can
/// hand it a column that stops short of the chip.
fn draw_state(frame: &mut Frame, room: &Room, column: Rect) {
    let (spans, polls) = state_spans(room);
    let state_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let line = Rect { x: column.x, y: column.y, width: column.width, height: 1 };
    frame.render_widget(Paragraph::new(Line::from(spans)), line);
    if let Some(polls) = polls
        && state_w + 2 + polls.chars().count() <= column.width as usize
    {
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), line);
    }
}

/// Radio rows: `(•) name — description`, one per row; returns the rows used.
fn radio_rows(frame: &mut Frame, room: &mut Room, at: Rect, options: &[(String, String)], chosen: usize, focused: bool, act: impl Fn(usize) -> Act) -> u16 {
    for (i, (name, desc)) in options.iter().enumerate() {
        if i as u16 >= at.height {
            break;
        }
        let rect = Rect { x: at.x, y: at.y + i as u16, width: at.width, height: 1 };
        let on = i == chosen;
        let hover = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on && focused {
            Style::default().fg(th().accent)
        } else if on {
            Style::default()
        } else {
            dim()
        };
        let name_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if on { g("(•)", "(*)") } else { "( )" }, glyph_style),
                Span::raw(" "),
                Span::styled(name.clone(), name_style),
                Span::styled(format!(" — {desc}"), dim()),
            ])),
            rect,
        );
        room.cells.ui.click(rect, act(i));
    }
    options.len() as u16
}

/// A three-row affirmative card with a centred label.
fn card(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, color: ratatui::style::Color, act: Act) {
    let hover = room.cells.ui.pointer.is_some_and(|p| at.contains(p));
    let color = if hover { th().bright } else { color };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
    let inner = block.inner(at);
    frame.render_widget(block, at);
    frame.render_widget(
        Paragraph::new(Span::styled(label.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        inner,
    );
    room.cells.ui.click(at, act);
}

/// The client page: the pitch, the client radio group, the card, the policy.
fn draw_choose(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    frame.render_widget(Paragraph::new(t!("tor.pitch_1").to_string()), line(column.y + 2));
    frame.render_widget(Paragraph::new(t!("tor.pitch_2").to_string()), line(column.y + 3));
    frame.render_widget(
        Paragraph::new(Span::styled(t!("tor.mobile_note", host = host_of(&room.client)).to_string(), dim())),
        line(column.y + 4),
    );
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.client_label").to_string(), dim())), line(column.y + 6));
    let options: Vec<(String, String)> = Kind::ALL.iter().map(|k| (k.label(), k.description())).collect();
    let rows = radio_rows(
        frame,
        room,
        Rect { x: column.x + 2, y: column.y + 7, width: column.width.saturating_sub(2), height: 4 },
        &options,
        room.client_pick,
        room.group == 0,
        Act::PickClient,
    );
    let mut y = column.y + 7 + rows + 1;
    let pick = Kind::ALL[room.client_pick.min(Kind::ALL.len() - 1)];
    if pick != Kind::Disabled {
        card(frame, room, Rect { x: column.x, y, width: column.width, height: 3 }, &t!("tor.connect_card", client = pick.label()), th().ok, Act::ApplyClient);
        y += 4;
    } else if room.kind() != Kind::Disabled {
        card(frame, room, Rect { x: column.x, y, width: column.width, height: 3 }, &t!("tor.off_card"), th().gold, Act::ApplyClient);
        y += 4;
    }
    if room.many_users() && y + 3 <= column.bottom() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.policy_label").to_string(), dim())), line(y));
        let policy = vec![
            (t!("tor.policy_all").to_string(), t!("tor.policy_all_desc").to_string()),
            (t!("tor.policy_whitelist").to_string(), t!("tor.policy_whitelist_desc").to_string()),
        ];
        let chosen = if room.policy_whitelist() { 1 } else { 0 };
        radio_rows(
            frame,
            room,
            Rect { x: column.x + 2, y: y + 1, width: column.width.saturating_sub(2), height: 2 },
            &policy,
            chosen,
            room.group == 1,
            Act::PickPolicy,
        );
    }
}

/// A labelled field: the label row, then a three-row box; the value may be
/// masked (a password) and may show a placeholder while empty.
#[allow(clippy::too_many_arguments)]
fn field_box(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, masked: bool, placeholder: Option<&str>, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = room.cells.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = block.inner(field);
    frame.render_widget(block, field);
    let w = inner.width.saturating_sub(2);
    let value = if masked { "•".repeat(input.value().chars().count()) } else { input.value().to_string() };
    let (shown, style) = if value.is_empty() && !focused {
        (placeholder.map(|p| clip(p, w)).unwrap_or_default(), dim())
    } else if focused {
        let mask = masked.then_some('•');
        (kit::field_display(&mut room.cells.ui, inner.x + 1, inner.y, &value, input.cursor(), w, mask), Style::default())
    } else {
        (clip(&value, w), Style::default())
    };
    frame.render_widget(Paragraph::new(Span::styled(shown, style)), Rect { x: inner.x + 1, y: inner.y, width: w, height: 1 });
    room.cells.ui.click(field, act);
}

/// The connect page: the form, the probe's answer, the two buttons.
fn draw_connect(frame: &mut Frame, room: &mut Room, column: Rect, c: &Connect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let fields = c.fields();
    let index_of = |f: CField| fields.iter().position(|x| *x == f).unwrap_or(0);
    let focused = c.focused();
    let x = column.x;
    let wide = column.width.saturating_sub(20).min(56);
    let half = column.width.saturating_sub(4).min(44).min(column.width.saturating_sub(4) / 2);
    let mut y = column.y + 2;
    field_box(frame, room, Rect { x, y, width: wide, height: 4 }, &t!("tor.field_host"), &c.host, focused == CField::Host, false, Some("127.0.0.1"), Act::ConnectFocus(index_of(CField::Host)));
    field_box(frame, room, Rect { x: x + wide + 4, y, width: 16.min(column.width.saturating_sub(wide + 4)), height: 4 }, &t!("tor.field_port"), &c.port, focused == CField::Port, false, None, Act::ConnectFocus(index_of(CField::Port)));
    y += 5;
    if c.kind.has_username() {
        field_box(frame, room, Rect { x, y, width: half, height: 4 }, &t!("tor.field_username"), &c.username, focused == CField::Username, false, None, Act::ConnectFocus(index_of(CField::Username)));
        field_box(frame, room, Rect { x: x + half + 4, y, width: half, height: 4 }, &t!("tor.field_password"), &c.password, focused == CField::Password, true, None, Act::ConnectFocus(index_of(CField::Password)));
    } else {
        field_box(frame, room, Rect { x, y, width: half, height: 4 }, &t!("tor.field_password"), &c.password, focused == CField::Password, true, None, Act::ConnectFocus(index_of(CField::Password)));
    }
    y += 5;
    let https_at;
    if c.kind.has_rpc_path() {
        field_box(frame, room, Rect { x, y, width: wide, height: 4 }, &t!("tor.field_rpc"), &c.rpc_path, focused == CField::RpcPath, false, None, Act::ConnectFocus(index_of(CField::RpcPath)));
        https_at = Rect { x: x + wide + 4, y: y + 2, width: column.width.saturating_sub(wide + 4), height: 1 };
        y += 5;
    } else {
        https_at = Rect { x, y, width: column.width, height: 1 };
        y += 2;
    }
    let glyph_style = if focused == CField::Https {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if c.https {
        Style::default().fg(th().ok)
    } else {
        dim()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(if c.https { g("[✓]", "[x]") } else { "[ ]" }, glyph_style),
            Span::raw(format!(" {}", t!("tor.https"))),
        ])),
        https_at,
    );
    room.cells.ui.click(https_at, Act::ConnectHttps);

    // The probe's answer, the form's own complaint, or qBittorrent's caveat.
    let verdict: Option<(String, Style)> = if let Some(e) = &c.error {
        Some((format!("{} {e}", g("−", "-")), Style::default().fg(th().gold)))
    } else {
        match &c.probe {
            Some(Ok((version, rpc))) => Some((
                format!(
                    "{} {}",
                    g("✓", "+"),
                    t!(
                        "tor.probe_ok",
                        client = c.kind.label(),
                        version = version,
                        rpc = if rpc.is_empty() { String::new() } else { t!("tor.probe_rpc", n = rpc).to_string() }
                    )
                ),
                Style::default().fg(th().ok),
            )),
            Some(Err(e)) => Some((format!("{} {}", g("−", "-"), t!("tor.probe_failed", err = e)), Style::default().fg(th().gold))),
            None if c.kind == Kind::Qbittorrent => Some((t!("tor.csrf_note").to_string(), Style::default().fg(th().gold))),
            None => None,
        }
    };
    if let Some((text, style)) = verdict
        && y < column.bottom()
    {
        frame.render_widget(Paragraph::new(Span::styled(text, style)).wrap(Wrap { trim: true }), Rect { x, y, width: column.width, height: 2.min(column.bottom() - y) });
    }
    let by = column.bottom().saturating_sub(1).max(y + 2);
    if by < column.bottom() + 1 {
        let connect = t!("tor.connect_button").to_string();
        let connect_w = kit::width(&connect) as u16 + 4;
        let connect_rect = kit::button(frame, &mut room.cells.ui, Rect { x: column.right().saturating_sub(connect_w), y: by, width: connect_w, height: 1 }, &connect, true, Act::ConnectSubmit);
        let test = t!("tor.test_button").to_string();
        let test_w = kit::width(&test) as u16 + 4;
        kit::button(frame, &mut room.cells.ui, Rect { x: connect_rect.x.saturating_sub(test_w + 2), y: by, width: test_w, height: 1 }, &test, false, Act::ConnectTest);
    }
    let _ = line;
}

/// Connected: under the state line, the tabs and the active tab's body.
fn draw_tabs(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let tabs_y = column.y + 2;
    let mut x = column.x;
    for tab in room.tabs() {
        let name = match tab {
            Tab::Torrents if !room.torrents.is_empty() => t!("tor.tab_torrents_n", n = room.torrents.len()).to_string(),
            Tab::Torrents => t!("tor.tab_torrents").to_string(),
            Tab::Libraries => t!("tor.tab_libraries").to_string(),
            Tab::Seeding => t!("tor.tab_seeding").to_string(),
            Tab::Access => t!("tor.tab_access").to_string(),
            Tab::Client => t!("tor.tab_client").to_string(),
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
        Tab::Torrents => t!("tor.note_torrents"),
        Tab::Libraries => t!("tor.note_libraries"),
        Tab::Seeding => t!("tor.note_seeding"),
        Tab::Access => t!("tor.note_access"),
        Tab::Client => t!("tor.note_client"),
    }
    .to_string();
    if x as usize + 2 + note.chars().count() <= column.right() as usize {
        frame.render_widget(Paragraph::new(Span::styled(note, dim())).alignment(Alignment::Right), line(tabs_y));
    }
    let body = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: column.bottom().saturating_sub(tabs_y + 2) };
    match room.tab {
        Tab::Torrents => draw_torrents(frame, room, body),
        Tab::Libraries => draw_libraries(frame, room, body),
        Tab::Seeding => draw_seeding(frame, room, body),
        Tab::Access => draw_access(frame, room, body),
        Tab::Client => draw_client(frame, room, body),
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
    let sel_moved = room.sel != room.sel_anchor;
    room.sel_anchor = room.sel;
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

/// Torrents: the filter line, then the daemon's list.
fn draw_torrents(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    // The filter: an inline input, dim words until it has something.
    let filter_rect = Rect { x: body.x, y: body.y, width: body.width.saturating_sub(20), height: 1 };
    let hover = room.cells.ui.pointer.is_some_and(|p| filter_rect.contains(p));
    let slash_style = if room.filter_focus { Style::default().fg(th().accent).add_modifier(Modifier::BOLD) } else if hover { Style::default().fg(th().bright) } else { dim() };
    let text = if room.filter_focus {
        // After the `/ ` mark's two cells.
        let (x, w) = (filter_rect.x + 2, filter_rect.width.saturating_sub(2));
        Span::raw(kit::field_display(&mut room.cells.ui, x, filter_rect.y, room.filter.value(), room.filter.cursor(), w, None))
    } else if room.filter.value().is_empty() {
        Span::styled(t!("tor.filter_hint").to_string(), dim())
    } else {
        Span::raw(clip(room.filter.value(), filter_rect.width.saturating_sub(2)))
    };
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("/ ", slash_style), text])), filter_rect);
    room.cells.ui.click(filter_rect, Act::FilterFocus);
    let filtered = room.filtered();
    let count = if !room.filter.value().trim().is_empty() {
        t!("tor.count_match", n = filtered.len(), total = room.torrents.len()).to_string()
    } else if room.torrents.len() == 1 {
        t!("tor.count_one").to_string()
    } else {
        t!("tor.count", n = room.torrents.len()).to_string()
    };
    if room.list_loaded && room.list_error.is_none() {
        frame.render_widget(Paragraph::new(Span::styled(count, dim())).alignment(Alignment::Right), line(body.y));
    }

    let table = Rect { x: body.x, y: body.y + 2, width: body.width, height: body.height.saturating_sub(2) };
    if table.height < 3 {
        return;
    }
    let (status_w, prog_w, down_w, size_w, by_w) = (11u16, 15u16, 10u16, 8u16, 13u16);
    let fixed = status_w + 2 + prog_w + 2 + down_w + 2 + size_w + 2 + by_w + 2;
    let name_w = table.width.saturating_sub(fixed).max(12);
    let status_x = table.x + name_w + 2;
    let prog_x = status_x + status_w + 2;
    let down_x = prog_x + prog_w + 2;
    let size_x = down_x + down_w + 2;
    let by_x = size_x + size_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, name_w, &t!("tor.col_name"), false),
            (status_x, status_w, &t!("tor.col_status"), false),
            (prog_x, prog_w, &t!("tor.col_progress"), false),
            (down_x, down_w, &t!("tor.col_down"), true),
            (size_x, size_w, &t!("tor.col_size"), true),
            (by_x, by_w, &t!("tor.col_by"), false),
        ],
    );
    if let Some(err) = &room.list_error {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("tor.list_error", err = printable(err, 200)).to_string(), Style::default().fg(th().gold))).wrap(Wrap { trim: true }),
            Rect { x: table.x, y: rows_y, width: table.width, height: 2.min(table.bottom().saturating_sub(rows_y)) },
        );
        return;
    }
    if !room.list_loaded {
        return;
    }
    if room.torrents.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_list").to_string(), dim())), line(rows_y));
        return;
    }
    if filtered.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_filter").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    // The list is lent to the rows and put back, not copied for them every
    // frame — it is the daemon's whole list, and a row needs the room only
    // for its tooltip (performance audit #110).
    let torrents = std::mem::take(&mut room.torrents);
    table_rows(frame, room, rows_rect, filtered.len(), |frame, room, i, rect, selected, hovered| {
        let t = &torrents[filtered[i]];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let name = printable(&t.name, 300);
        frame.render_widget(Paragraph::new(Span::styled(clip(&name, name_w), base)), cell(rect.x, name_w));
        if name.chars().count() > name_w as usize {
            room.cells.ui.tip(cell(rect.x, name_w), format!("{name} · {}", t.info_hash));
        }
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&status_word(&t.status), status_w), cell_style(selected, hovered, status_style(&t.status)))),
            cell(status_x, status_w),
        );
        let pct = (t.percent * 100.0).round().clamp(0.0, 100.0) as u16;
        let filled = (pct / 10) as usize;
        let bar_style = if t.status == "seeding" { Style::default().fg(th().ok) } else { Style::default().fg(th().accent) };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("▰".repeat(filled), cell_style(selected, hovered, bar_style)),
                Span::styled("▱".repeat(10 - filled), cell_style(selected, hovered, dim())),
                Span::styled(format!(" {pct:>3}%"), cell_style(selected, hovered, dim())),
            ])),
            cell(prog_x, prog_w),
        );
        let down = rate_words(t.rate_download);
        frame.render_widget(
            Paragraph::new(Span::styled(down.clone(), cell_style(selected, hovered, if down == "—" { dim() } else { Style::default() }))).alignment(Alignment::Right),
            cell(down_x, down_w),
        );
        frame.render_widget(Paragraph::new(Span::styled(fmt_bytes(t.size_bytes), base)).alignment(Alignment::Right), cell(size_x, size_w));
        let (by, own) = match (t.managed_by_mstream, &t.managed_by) {
            (true, Some(u)) => (printable(u, NAME_MAX), Style::default()),
            (true, None) => ("mStream".to_string(), Style::default()),
            (false, _) => (t!("tor.by_external").to_string(), dim()),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&by, by_w), cell_style(selected, hovered, own))), cell(by_x, by_w));
    });
    room.torrents = torrents;
    // The cursor row's hash, error and origin ride the note line — found
    // through this frame's filter, not a second run of it.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(t) = room.sel.and_then(|s| filtered.get(s)).and_then(|&i| room.torrents.get(i))
    {
        let mut spans = vec![Span::styled(short_id(&t.info_hash), dim())];
        if !t.error_message.trim().is_empty() {
            spans.push(Span::styled(" · ", dim()));
            spans.push(Span::styled(printable(&t.error_message, 200), Style::default().fg(th().gold)));
        }
        if let Some(u) = t.managed_by.as_deref().filter(|_| t.managed_by_mstream) {
            spans.push(Span::styled(format!(" · {}", t!("tor.note_added_by", user = printable(u, NAME_MAX))), dim()));
        }
        if t.added_at > 0.0 {
            spans.push(Span::styled(format!(" · {}", ago_words("added", unix_now() - t.added_at as i64)), dim()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), line(body.bottom() + 1));
    }
}

/// Libraries: the daemon's view of each library and its template.
fn draw_libraries(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let client = room.kind().label();
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.intro_1", client = client.clone()).to_string(), dim())), line(body.y));
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.intro_2").to_string(), dim())), line(body.y + 1));
    let table = Rect { x: body.x, y: body.y + 3, width: body.width, height: body.height.saturating_sub(3) };
    if table.height < 3 {
        return;
    }
    let (lib_w, access_w, tmpl_w) = (12u16, 14u16, 34u16);
    let fixed = lib_w + 2 + access_w + 2 + tmpl_w + 2;
    let path_w = table.width.saturating_sub(fixed).max(12);
    let access_x = table.x + lib_w + 2;
    let path_x = access_x + access_w + 2;
    let tmpl_x = path_x + path_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, lib_w, &t!("tor.col_library"), false),
            (access_x, access_w, &t!("tor.col_access"), false),
            (path_x, path_w, &t!("tor.col_path", client = client.to_uppercase()), false),
            (tmpl_x, tmpl_w, &t!("tor.col_template"), false),
        ],
    );
    if room.libraries.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_libraries").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let libraries = room.libraries.clone();
    let access = room.access.clone();
    let templates = room.templates.vpaths.clone();
    table_rows(frame, room, rows_rect, libraries.len(), |frame, room, i, rect, selected, hovered| {
        let name = &libraries[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(name, NAME_MAX), lib_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x, lib_w),
        );
        let row = access.get(name);
        let confidence = row.map(|a| a.confidence.as_str()).unwrap_or("unconfirmed");
        frame.render_widget(
            Paragraph::new(Span::styled(
                clip(&format!("{} {}", confidence_glyph(confidence), confidence_words(confidence)), access_w),
                cell_style(selected, hovered, confidence_style(confidence)),
            )),
            cell(access_x, access_w),
        );
        let (path, own) = match row.and_then(|a| a.daemon_path.clone()).filter(|p| !p.is_empty()) {
            Some(p) if confidence != "pending" => (p, Style::default()),
            _ if confidence == "pending" => (t!("tor.path_probing").to_string(), dim()),
            _ => (t!("tor.path_none").to_string(), dim()),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(&path, 300), path_w), cell_style(selected, hovered, own))), cell(path_x, path_w));
        if path.chars().count() > path_w as usize {
            room.cells.ui.tip(cell(path_x, path_w), path.clone());
        }
        let (tmpl, own) = match templates.get(name).and_then(|t| t.template.clone()).filter(|t| !t.trim().is_empty()) {
            Some(t) => (t, Style::default()),
            None => (t!("tor.tmpl_none").to_string(), dim()),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(&tmpl, 300), tmpl_w), cell_style(selected, hovered, own))), cell(tmpl_x, tmpl_w));
        if tmpl.chars().count() > tmpl_w as usize {
            room.cells.ui.tip(cell(tmpl_x, tmpl_w), tmpl.clone());
        }
    });
    // The cursor row's probe story rides the note line.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(name) = room.selected_library()
    {
        let now = unix_now();
        let mut spans = vec![Span::styled(printable(name, NAME_MAX), bold())];
        match room.access.get(name) {
            Some(a) => {
                let mut parts: Vec<(String, Style)> = Vec::new();
                if let Some(e) = a.last_error.as_deref().filter(|e| !e.trim().is_empty()) {
                    parts.push((printable(e, 200), Style::default().fg(th().gold)));
                } else if let Some(m) = a.method.as_deref().filter(|_| a.confidence == "verified" || a.confidence == "inferred") {
                    parts.push((t!("tor.note_via", method = printable(m, 60)).to_string(), dim()));
                }
                if a.source.as_deref() == Some("manual") {
                    parts.push((t!("tor.note_manual").to_string(), dim()));
                }
                if let Some(secs) = a.last_probed_at.as_ref().and_then(|v| probed_secs(v, now)) {
                    parts.push((ago_words("probed", secs), dim()));
                }
                for (text, style) in parts {
                    spans.push(Span::styled(" — ", dim()));
                    spans.push(Span::styled(text, style));
                    if let Some(last) = spans.last_mut()
                        && last.content.is_empty()
                    {
                        spans.pop();
                    }
                }
            }
            None => {
                spans.push(Span::styled(" — ", dim()));
                spans.push(Span::styled(t!("tor.path_none").to_string(), dim()));
            }
        }
        // Later separators read as " · ".
        let mut seen = 0;
        for s in spans.iter_mut() {
            if s.content == " — " {
                seen += 1;
                if seen > 1 {
                    s.content = " · ".into();
                }
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), line(body.bottom() + 1));
    }
}

/// Seeding: the pitch, the add card, the SEARCH IN ticks, the results.
fn draw_seeding(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.seed_intro_1").to_string(), dim())), line(body.y));
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.seed_intro_2").to_string(), dim())), line(body.y + 1));
    card(frame, room, Rect { x: body.x, y: body.y + 3, width: body.width, height: 3 }, &t!("tor.seed_card"), th().ok, Act::SeedAdd);
    let ticks_y = body.y + 7;
    let mut x = body.x;
    let label = format!("{}  ", t!("tor.seed_search"));
    // The label and each tick in cells, as drawn, the users room's add-form
    // rule: counted by characters a library named in Japanese got a rect
    // half its width, so it was cut there and its click covered only what
    // was left. The first tick is drawn however wide, clamped to the row,
    // rather than leaving the row empty; a later one that does not fit is
    // left off.
    let label_w = kit::width(&label) as u16;
    frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), Rect { x, y: ticks_y, width: label_w.min(body.width), height: 1 });
    x += label_w;
    let first_x = x;
    let ticks = room.seed_ticks.clone();
    for (i, (name, on)) in ticks.iter().enumerate() {
        let text = format!("{} {}", if *on { g("[✓]", "[x]") } else { "[ ]" }, printable(name, NAME_MAX));
        let w = kit::width(&text) as u16;
        if x + w > body.right() && x > first_x {
            break;
        }
        let rect = Rect { x, y: ticks_y, width: w.min(body.right().saturating_sub(x)), height: 1 };
        let hover = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if hover { Style::default().fg(th().bright) } else if *on { Style::default().fg(th().ok) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(text, style)), rect);
        room.cells.ui.click(rect, Act::SeedTick(i));
        x += rect.width + 3;
    }
    if ticks.iter().all(|(_, on)| !*on) {
        let hint = t!("tor.seed_all").to_string();
        if x + 2 + kit::width(&hint) as u16 <= body.right() {
            frame.render_widget(Paragraph::new(Span::styled(hint, dim())).alignment(Alignment::Right), line(ticks_y));
        }
    }
    let table = Rect { x: body.x, y: body.y + 9, width: body.width, height: body.height.saturating_sub(9) };
    if table.height < 3 {
        return;
    }
    let (file_w, outcome_w) = (26u16, 16u16);
    let outcome_x = table.x + file_w + 2;
    let details_x = outcome_x + outcome_w + 2;
    let details_w = table.right().saturating_sub(details_x);
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, file_w, &t!("tor.col_file"), false),
            (outcome_x, outcome_w, &t!("tor.col_outcome"), false),
            (details_x, details_w, &t!("tor.col_details"), false),
        ],
    );
    if room.seeds.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.seed_empty").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let seeds = room.seeds.clone();
    let client = room.kind().label();
    table_rows(frame, room, rows_rect, seeds.len(), |frame, room, i, rect, selected, hovered| {
        let s = &seeds[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(&s.file, 200), file_w), base)), cell(rect.x, file_w));
        let (word, style) = match (&s.outcome, &s.error) {
            (Some(o), _) => outcome_words(&o.outcome),
            (None, Some(_)) => outcome_words("daemon_error"),
            (None, None) => (format!("{} {}", g("⟳", "~"), t!("tor.seed_checking")), Style::default().fg(th().accent)),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&word, outcome_w), cell_style(selected, hovered, style))), cell(outcome_x, outcome_w));
        let details = outcome_details(s, &client);
        let own = if s.outcome.as_ref().is_some_and(|o| matches!(o.outcome.as_str(), "no_match" | "already_in_daemon")) { dim() } else { Style::default() };
        frame.render_widget(Paragraph::new(Span::styled(clip(&details, details_w), cell_style(selected, hovered, own))), cell(details_x, details_w));
        if details.chars().count() > details_w as usize {
            room.cells.ui.tip(cell(details_x, details_w), details.clone());
        }
    });
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(s) = room.sel.and_then(|i| room.seeds.get(i))
    {
        let details = outcome_details(s, &client);
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(printable(&s.file, NAME_MAX), bold()), Span::raw(format!(" — {}", clip(&details, body.width.saturating_sub(s.file.chars().count() as u16 + 3))))])),
            line(body.bottom() + 1),
        );
    }
}

/// Access: the policy, then the users.
fn draw_access(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.policy_label").to_string(), dim())), line(body.y));
    let policy = vec![
        (t!("tor.policy_all").to_string(), t!("tor.policy_all_desc").to_string()),
        (t!("tor.policy_whitelist").to_string(), t!("tor.policy_whitelist_desc_here").to_string()),
    ];
    let whitelist = room.policy_whitelist();
    radio_rows(frame, room, Rect { x: body.x + 2, y: body.y + 1, width: body.width.saturating_sub(2), height: 2 }, &policy, if whitelist { 1 } else { 0 }, true, Act::PickPolicy);
    let table = Rect { x: body.x, y: body.y + 4, width: body.width, height: body.height.saturating_sub(4) };
    if table.height < 3 {
        return;
    }
    let (user_w, admin_w, tor_w) = (22u16, 8u16, 10u16);
    let admin_x = table.x + user_w + 2;
    let tor_x = admin_x + admin_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, user_w, &t!("tor.col_user"), false),
            (admin_x, admin_w, &t!("tor.col_admin"), false),
            (tor_x, tor_w, &t!("tor.col_torrents"), false),
        ],
    );
    if room.users.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_users").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    // Lent to the rows and put back, not copied every frame (performance
    // audit #110): a row needs the room only for its click.
    let users = std::mem::take(&mut room.users);
    let rows: Vec<(&String, &AdminUser)> = users.iter().collect();
    table_rows(frame, room, rows_rect, rows.len(), |frame, room, i, rect, selected, hovered| {
        let (name, u) = rows[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        // With every user allowed, the ticks are information, not a lever: dim.
        let own = if whitelist { Style::default() } else { dim() };
        let base = cell_style(selected, hovered, own);
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(name, NAME_MAX), user_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })), cell(rect.x, user_w));
        let admin = if u.admin { t!("tor.admin_yes").to_string() } else { "—".to_string() };
        frame.render_widget(Paragraph::new(Span::styled(admin, cell_style(selected, hovered, dim()))), cell(admin_x, admin_w));
        let glyph = if u.allow_torrent { g("[✓]", "[x]") } else { "[ ]" };
        let glyph_style = if u.allow_torrent && whitelist { Style::default().fg(th().ok) } else { dim() };
        let glyph_rect = cell(tor_x, 3);
        frame.render_widget(Paragraph::new(Span::styled(glyph, cell_style(selected, hovered, glyph_style))), glyph_rect);
        room.cells.ui.click(glyph_rect, Act::UserToggle(name.clone()));
    });
    room.users = users;
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(name) = room.selected_user()
    {
        let allowed = room.users.get(name).is_some_and(|u| u.allow_torrent);
        let user = printable(name, NAME_MAX);
        let text = if allowed { t!("tor.note_user_may", user = user) } else { t!("tor.note_user_may_not", user = user) }.to_string();
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::raw(text), Span::styled(t!("tor.note_user_tail").to_string(), dim())])),
            line(body.bottom() + 1),
        );
    }
}

/// Client: the connection card — status, the saved fields, the buttons.
fn draw_client(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let kind = room.kind();
    let label = kind.label();
    let status = match &room.status {
        Some(s) if s.connected => Span::styled(t!("tor.status_connected", client = label.clone(), version = version_words(s)).to_string(), Style::default().fg(th().ok)),
        Some(s) => Span::styled(t!("tor.status_down", reason = printable(s.reason.as_deref().unwrap_or(""), 120)).to_string(), Style::default().fg(th().gold)),
        None => Span::styled(t!("tor.status_unknown").to_string(), dim()),
    };
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(label.to_uppercase(), bold()), Span::raw("   "), status])), line(body.y));
    let cfg = room.config().cloned().unwrap_or_default();
    let mut rows: Vec<(String, String, bool)> = vec![
        (t!("tor.field_host").to_string(), cfg.host.clone(), true),
        (t!("tor.field_port").to_string(), cfg.port.to_string(), true),
    ];
    if kind.has_username() {
        let u = cfg.username.trim().to_string();
        rows.push((t!("tor.field_username").to_string(), if u.is_empty() { "(none)".to_string() } else { u.clone() }, !u.is_empty()));
    }
    if kind.has_rpc_path() {
        rows.push((t!("tor.field_rpc").to_string(), cfg.rpc_path.clone().unwrap_or_default(), true));
    }
    rows.push(("HTTPS".to_string(), if cfg.use_https { "yes".to_string() } else { "no".to_string() }, true));
    let mut y = body.y + 2;
    for (k, v, plain) in rows {
        frame.render_widget(Paragraph::new(Span::styled(k, dim())), Rect { x: body.x, y, width: 11, height: 1 });
        frame.render_widget(Paragraph::new(Span::styled(printable(&v, 200), if plain { Style::default() } else { dim() })), Rect { x: body.x + 12, y, width: body.width.saturating_sub(12), height: 1 });
        y += 1;
    }
    y += 1;
    let others: Vec<String> = room
        .params
        .as_ref()
        .map(|p| {
            Kind::ALL
                .iter()
                .filter(|k| **k != Kind::Disabled && **k != kind && config_of(p, **k).configured)
                .map(|k| format!("{} {}:{}", k.label(), config_of(p, *k).host, config_of(p, *k).port))
                .collect()
        })
        .unwrap_or_default();
    if !others.is_empty() && y < body.bottom() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}  ", t!("tor.also_saved")), dim()),
                Span::raw(t!("tor.also_saved_line", clients = others.join(", ")).to_string()),
            ]))
            .wrap(Wrap { trim: true }),
            Rect { x: body.x, y, width: body.width, height: 2.min(body.bottom() - y) },
        );
        y += 2;
    }
    if y + 1 < body.bottom() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.mobile_note", host = host_of(&room.client)).to_string(), dim())), line(y));
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.disconnect_note").to_string(), dim())), line(y + 1));
    }
    let by = body.bottom().saturating_sub(1);
    if by > y + 1 {
        let switch = t!("tor.btn_switch").to_string();
        let switch_w = kit::width(&switch) as u16 + 4;
        let switch_rect = kit::button(frame, &mut room.cells.ui, Rect { x: body.right().saturating_sub(switch_w), y: by, width: switch_w, height: 1 }, &switch, true, Act::ClientSwitch);
        let disc = t!("tor.btn_disconnect").to_string();
        let disc_w = kit::width(&disc) as u16 + 4;
        let disc_rect = kit::button(frame, &mut room.cells.ui, Rect { x: switch_rect.x.saturating_sub(disc_w + 2), y: by, width: disc_w, height: 1 }, &disc, false, Act::Disconnect);
        let test = t!("tor.btn_test").to_string();
        let test_w = kit::width(&test) as u16 + 4;
        kit::button(frame, &mut room.cells.ui, Rect { x: disc_rect.x.saturating_sub(test_w + 2), y: by, width: test_w, height: 1 }, &test, true, Act::ClientTest);
    }
    if room.note.is_none() && room.busy.is_none() {
        let text = match (&room.status, room.status_at) {
            (Some(s), Some(at)) if s.connected => Some((ago_words("reachable", at.elapsed().as_secs() as i64), dim())),
            (Some(s), _) => Some((t!("tor.note_unreachable", reason = printable(s.reason.as_deref().unwrap_or(""), 200)).to_string(), Style::default().fg(th().gold))),
            _ => None,
        };
        if let Some((text, style)) = text {
            frame.render_widget(Paragraph::new(Span::styled(text, style)), line(body.bottom() + 1));
        }
    }
}

/// One text field in a modal, helper lines beneath, a row of buttons.
#[allow(clippy::too_many_arguments)]
fn draw_entry_modal(
    frame: &mut Frame,
    room: &mut Room,
    area: Rect,
    title: String,
    label: String,
    input: &Input,
    placeholder: Option<&str>,
    body: Vec<Line<'static>>,
    error: Option<&str>,
    buttons: Vec<(String, bool, Act)>,
) {
    let height = 10 + body.len() as u16 + if error.is_some() { 1 } else { 0 };
    let inner = kit::modal_frame(frame, area, 74, height, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(clip(&title, w.saturating_sub(4)), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))), line(inner.y));
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::ModalCancel);
    field_box(frame, room, Rect { x, y: inner.y + 2, width: w, height: 4 }, &label, input, true, false, placeholder, Act::ModalSubmit);
    let mut y = inner.y + 7;
    for l in body {
        frame.render_widget(Paragraph::new(l), line(y));
        y += 1;
    }
    if let Some(e) = error {
        frame.render_widget(Paragraph::new(Span::styled(format!("{} {}", g("−", "-"), clip(e, w.saturating_sub(2))), Style::default().fg(th().gold))), line(y));
    }
    let by = inner.bottom().saturating_sub(1);
    let mut right = inner.right().saturating_sub(1);
    for (label, primary, act) in buttons.into_iter().rev() {
        let bw = kit::width(&label) as u16 + 4;
        let rect = kit::button(frame, &mut room.cells.ui, Rect { x: right.saturating_sub(bw), y: by, width: bw, height: 1 }, &label, primary, act);
        right = rect.x.saturating_sub(2);
    }
}

/// The gold gate: a title, its lines, the safe choice first.
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

    fn row(frame: &str, start: &str) -> String {
        frame.lines().find(|l| l.trim_start().starts_with(start)).map(str::to_string).unwrap_or_default()
    }

    #[test]
    fn the_client_page_offers_the_daemons_and_hides_the_policy_for_one_user() {
        let _en = english();
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("disabled", false), users(1), false));
        assert_eq!(room.phase(), Phase::Choose);
        let frame = draw(&mut room);
        assert!(frame.contains("Torrents beta"), "{frame}");
        assert!(frame.contains("• off — no torrent client chosen"), "{frame}");
        assert!(frame.contains("Hand off magnet links and .torrent files to a torrent client on this host or on the LAN."), "{frame}");
        assert!(frame.contains("Users add torrents from a phone at home.mstream.example/torrent"), "{frame}");
        assert!(frame.contains("(•) Disabled — nobody can add torrents"), "{frame}");
        assert!(frame.contains("( ) Transmission — RPC on port 9091, usually at /transmission/rpc"), "{frame}");
        assert!(frame.contains("( ) qBittorrent — WebUI on port 8080 · its CSRF protection must be off"), "{frame}");
        assert!(!frame.contains("WHO CAN ADD TORRENTS"), "one user has nobody to whitelist\n{frame}");
        assert!(!frame.contains("Connect to"), "nothing to connect to yet\n{frame}");
        assert!(frame.contains("←→ or Space choose a client · Esc back"), "{frame}");
        assert!(frame.contains("beta — the torrent feature is new"), "{frame}");
        press(&mut room, KeyCode::Right);
        let frame = draw(&mut room);
        assert!(frame.contains("(•) Transmission") && frame.contains("Connect to Transmission ▸"), "{frame}");
        assert!(frame.contains("←→ or Space choose · Enter connect · Esc back"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::SetClient(Kind::Transmission)));
        room.queued = None;
        room.apply(Done::ClientSet { kind: Kind::Transmission, result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "torrent client: Transmission"));
        assert_eq!(room.queued, Some(Op::Load));
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        assert_eq!(room.phase(), Phase::Connect);
        let frame = draw(&mut room);
        assert!(frame.contains("• transmission — no credentials yet · Test saves nothing; Connect saves after a good probe"), "{frame}");
        for word in ["HOST", "PORT", "USERNAME", "PASSWORD", "RPC PATH", "[ ] use HTTPS", "9091", "/transmission/rpc", "^T test", "Connect ▸"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(frame.contains("Tab next field · Space HTTPS · ^T test · Enter connect · Esc back to the client choice"), "{frame}");
        // Esc returns to the choice; the picked client stays applied.
        press(&mut room, KeyCode::Esc);
        assert_eq!(room.phase(), Phase::Choose);
        let frame = draw(&mut room);
        assert!(frame.contains("• transmission — no credentials yet") && frame.contains("(•) Transmission"), "{frame}");
        // Picking Disabled again offers to turn the feature off.
        press(&mut room, KeyCode::Left);
        let frame = draw(&mut room);
        assert!(frame.contains("(•) Disabled") && frame.contains("Turn torrents off ▸"), "{frame}");
        assert!(frame.contains("←→ or Space choose · Enter turn off · Esc back"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::SetClient(Kind::Disabled)));
        room.queued = None;
        room.apply(Done::ClientSet { kind: Kind::Disabled, result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "torrents are off"));

        // Four users: the policy group appears, Tab reaches it, a change posts at once.
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("disabled", false), users(4), false));
        let frame = draw(&mut room);
        assert!(frame.contains("WHO CAN ADD TORRENTS") && frame.contains("(•) all users — every signed-in user can add torrents"), "{frame}");
        assert!(frame.contains("( ) whitelist — only the users you tick on the Access tab"), "{frame}");
        assert!(frame.contains("←→ or Space choose a client · Esc back"), "{frame}");
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Right);
        assert_eq!(room.queued, Some(Op::SetPolicy("whitelist".into())));
        room.queued = None;
        room.apply(Done::PolicySet { policy: "whitelist".into(), result: Ok(()) });
        assert!(room.policy_whitelist());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "policy: only the whitelist may add torrents"));
        assert!(draw(&mut room).contains("(•) whitelist"));
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn the_connect_form_probes_without_saving_then_connects() {
        let _en = english();
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        assert_eq!(room.connect.as_ref().map(|c| c.focused()), Some(CField::Host));
        press(&mut room, KeyCode::Enter);
        let frame = draw(&mut room);
        assert!(frame.contains("− a host is needed"), "{frame}");
        assert!(room.queued.is_none());
        type_text(&mut room, "127.0.0.1");
        press(&mut room, KeyCode::Tab); // port: digits only
        type_text(&mut room, "x");
        assert_eq!(room.connect.as_ref().unwrap().port.value(), "9091");
        press(&mut room, KeyCode::Tab);
        type_text(&mut room, "admin");
        press(&mut room, KeyCode::Tab);
        type_text(&mut room, "s3cret");
        let frame = draw(&mut room);
        assert!(frame.contains("••••••") && !frame.contains("s3cret"), "the password is masked\n{frame}");
        press(&mut room, KeyCode::Tab); // rpc path
        press(&mut room, KeyCode::Tab); // https
        assert_eq!(room.connect.as_ref().unwrap().focused(), CField::Https);
        press(&mut room, KeyCode::Char(' '));
        assert!(room.connect.as_ref().unwrap().https);
        ctrl(&mut room, 't');
        let creds = TorrentCreds {
            host: "127.0.0.1".into(),
            port: 9091,
            username: Some("admin".into()),
            password: "s3cret".into(),
            rpc_path: Some("/transmission/rpc".into()),
            use_https: true,
        };
        assert_eq!(room.queued, Some(Op::Probe { kind: Kind::Transmission, creds: creds.clone(), connect: false }));
        room.queued = None;
        room.apply(Done::Probed {
            kind: Kind::Transmission,
            connect: false,
            result: Ok(ProbeAnswer { ok: true, version: Some("4.0.6".into()), rpc_version: Some(serde_json::json!(17)), ..Default::default() }),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("✓ reachable — Transmission 4.0.6 · RPC 17 · nothing saved yet"), "{frame}");
        assert_eq!(room.connect.as_ref().unwrap().password.value(), "s3cret", "a test keeps the draft");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Probe { kind: Kind::Transmission, creds: creds.clone(), connect: true }));
        room.queued = None;
        room.apply(Done::Probed {
            kind: Kind::Transmission,
            connect: true,
            result: Ok(ProbeAnswer { ok: false, error: Some("connection_failed".into()), message: Some("connect ECONNREFUSED 127.0.0.1:9091".into()), ..Default::default() }),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("− connection failed: connect ECONNREFUSED 127.0.0.1:9091"), "{frame}");
        assert_eq!(room.phase(), Phase::Connect, "nothing was saved");
        press(&mut room, KeyCode::Enter);
        room.queued = None;
        room.apply(Done::Probed {
            kind: Kind::Transmission,
            connect: true,
            result: Ok(ProbeAnswer { ok: true, version: Some("4.0.6".into()), ..Default::default() }),
        });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "connected to Transmission 4.0.6"));
        assert_eq!(room.queued, Some(Op::Load));
        assert_eq!(room.connect.as_ref().unwrap().password.value(), "", "the password leaves the draft once accepted");

        // qBittorrent: no RPC path, the CSRF caveat; Deluge: no username.
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("qbittorrent", false), users(1), false));
        let frame = draw(&mut room);
        assert!(frame.contains("USERNAME") && !frame.contains("RPC PATH"), "{frame}");
        assert!(frame.contains("mStream sends no Referer — turn off qBittorrent's CSRF protection"), "{frame}");
        assert_eq!(room.connect.as_ref().unwrap().port.value(), "8080");
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("deluge", false), users(1), false));
        let frame = draw(&mut room);
        assert!(!frame.contains("USERNAME") && !frame.contains("RPC PATH") && frame.contains("PASSWORD"), "{frame}");
        assert_eq!(room.connect.as_ref().unwrap().fields(), vec![CField::Host, CField::Port, CField::Password, CField::Https]);
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
        let mut room = connected();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut room)).unwrap();
        assert!(!room.torrents.is_empty(), "the Torrents tab carries its count");
        let tabs: Vec<(Tab, String)> = room
            .tabs()
            .into_iter()
            .map(|tab| {
                let name = match tab {
                    Tab::Torrents => t!("tor.tab_torrents_n", n = room.torrents.len()).to_string(),
                    Tab::Libraries => t!("tor.tab_libraries").to_string(),
                    Tab::Seeding => t!("tor.tab_seeding").to_string(),
                    Tab::Access => t!("tor.tab_access").to_string(),
                    Tab::Client => t!("tor.tab_client").to_string(),
                };
                (tab, format!(" {name} "))
            })
            .collect();
        assert_eq!(tabs.len(), 5, "every tab, Access included");
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
    fn connected_the_torrents_tab_lists_filters_and_gates_the_remove() {
        let _en = english();
        let mut room = connected();
        assert_eq!(room.phase(), Phase::Tabs);
        let frame = draw(&mut room);
        assert!(frame.contains("• connected — Transmission 4.0.6 · 127.0.0.1:9091 · 2 of 4 libraries reachable"), "{frame}");
        assert!(frame.contains("polls every 5 s"), "{frame}");
        for word in [" Torrents · 4 ", " Libraries ", " Seeding ", " Access ", " Client ", "the daemon's list", "/ filter by name or info-hash", "4 torrents"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        for word in ["NAME", "STATUS", "PROGRESS", "DOWN", "SIZE", "ADDED BY"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let boc = row(&frame, "Boards of Canada");
        assert!(boc.contains("downloading") && boc.contains("▰▰▰▰▰▰▱▱▱▱  63%") && boc.contains("2.4 MB/s") && boc.contains("612.0 MB") && boc.contains("iros"), "{boc}");
        let linux = row(&frame, "linux-6.9.iso");
        assert!(linux.contains("seeding") && linux.contains("▰▰▰▰▰▰▰▰▰▰ 100%") && linux.contains("external"), "{linux}");
        assert!(row(&frame, "Radiohead").contains("error"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · / filter · Esc back"), "{frame}");
        // The cursor row's story on the note line.
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("e5f6e5f6e5f6… · Tracker gave HTTP response code 403 (Forbidden) · added by iros · 2d ago"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · / filter · r remove (files stay) · Esc deselect"), "{frame}");
        // The filter takes keys only while focused, and narrows the rows.
        press(&mut room, KeyCode::Char('/'));
        assert!(room.filter_focus);
        type_text(&mut room, "linux");
        let frame = draw(&mut room);
        assert!(frame.contains("1 of 4 match") && frame.contains("type to filter · Enter or Esc done"), "{frame}");
        assert!(!frame.contains("Boards of Canada"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(!room.filter_focus);
        press(&mut room, KeyCode::Down);
        assert!(draw(&mut room).contains("↑↓ rows · ←→ tab · / filter · Esc deselect"), "external rows have no r");
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::None));
        press(&mut room, KeyCode::Char('/'));
        for _ in 0..5 {
            press(&mut room, KeyCode::Backspace);
        }
        press(&mut room, KeyCode::Esc);
        assert!(draw(&mut room).contains("4 torrents"));
        // r on an mStream row: the gate, then the daemon's answer.
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Remove(_)));
        let frame = draw(&mut room);
        assert!(frame.contains("from Transmission?") && frame.contains("The files on disk are KEPT"), "{frame}");
        assert!(frame.contains("y remove · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        press(&mut room, KeyCode::Char('r'));
        press(&mut room, KeyCode::Char('y'));
        let hash: String = "5f4a".repeat(10);
        assert_eq!(room.queued, Some(Op::Remove(hash.clone())));
        room.queued = None;
        room.apply(Done::Removed { hash: hash.clone(), result: Ok(RemoveAnswer { ok: true, daemon_remove_ok: true, daemon_remove_error: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n.starts_with("removed Boards of Canada") && n.ends_with("the files on disk stay")));
        assert_eq!(room.queued, Some(Op::List));
        room.apply(Done::Removed { hash: hash.clone(), result: Err(ApiError::NotFound("x".into())) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.starts_with("mStream did not add this torrent")));
        room.apply(Done::Removed {
            hash,
            result: Ok(RemoveAnswer { ok: true, daemon_remove_ok: false, daemon_remove_error: Some("daemon offline".into()) }),
        });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("the daemon-side remove failed: daemon offline")));
        // The polls: with a download running, the list every five seconds
        // here, everything every thirty.
        room.queued = None;
        room.last_load = Some(Instant::now());
        room.last_list = Some(Instant::now() - Duration::from_secs(6));
        room.tick();
        assert_eq!(room.queued, Some(Op::List));
        room.queued = None;
        room.last_load = Some(Instant::now() - Duration::from_secs(31));
        room.tick();
        assert_eq!(room.queued, Some(Op::Load));
        room.queued = None;
        // A daemon that went away: the list's error, in gold, and the state line.
        room.apply(Done::Listed(Ok(TorrentList { torrents: Vec::new(), error: Some("connect ECONNREFUSED".into()), client_type: None })));
        room.apply(Done::Status(Ok(TorrentStatus { connected: false, configured: true, reason: Some("connect ECONNREFUSED 127.0.0.1:9091".into()), ..Default::default() })));
        let frame = draw(&mut room);
        assert!(frame.contains("• disconnected — connect ECONNREFUSED 127.0.0.1:9091 · credentials kept"), "{frame}");
        assert!(frame.contains("couldn't fetch torrents: connect ECONNREFUSED"), "{frame}");
    }

    #[test]
    fn the_libraries_tab_shows_the_ladder_and_edits_mapping_and_template() {
        let _en = english();
        let mut room = connected();
        press(&mut room, KeyCode::Right);
        assert_eq!(room.tab, Tab::Libraries);
        let frame = draw(&mut room);
        assert!(frame.contains("The paths are as Transmission sees them, not as mStream does"), "{frame}");
        assert!(frame.contains("PATH AS SEEN BY TRANSMISSION") && frame.contains("TEMPLATE"), "{frame}");
        let music = row(&frame, "music");
        assert!(music.contains("✓ verified") && music.contains("/downloads/music") && music.contains("{{ARTIST}}/{{ALBUM}} ({{YEAR}})"), "{music}");
        let vinyl = row(&frame, "vinyl");
        assert!(vinyl.contains("✗ unconfirmed") && vinyl.contains("no mapping yet"), "{vinyl}");
        let books = row(&frame, "audiobooks");
        assert!(books.contains("⟳ probing…") && books.contains("asking the daemon…") && books.contains("(none — typed by hand)"), "{books}");
        assert!(frame.contains("↑↓ select · ←→ tab · D auto-detect all · Esc back"), "{frame}");
        // audiobooks · music · podcasts · vinyl.
        for _ in 0..4 {
            press(&mut room, KeyCode::Down);
        }
        assert_eq!(room.selected_library(), Some("vinyl"));
        let frame = draw(&mut room);
        assert!(frame.contains("vinyl — daemon free-space returned -1 (path not visible to daemon) · probed 3m ago"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · d auto-detect · m map by hand · t template · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Char('d'));
        assert_eq!(room.queued, Some(Op::Detect(Some("vinyl".into()))));
        assert_eq!(room.busy.as_deref(), Some("asking the daemon about vinyl…"));
        room.queued = None;
        room.apply(Done::Detected(Ok(access())));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "auto-detect done"));
        press(&mut room, KeyCode::Char('D'));
        assert_eq!(room.queued, Some(Op::Detect(None)));
        room.queued = None;
        room.busy = None;
        // m: the daemon's path, verified on save; a 422 keeps the modal with the reason.
        press(&mut room, KeyCode::Char('m'));
        assert!(matches!(room.modal, Modal::Mapping { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Map vinyl for Transmission") && frame.contains("PATH AS SEEN BY TRANSMISSION") && frame.contains("Verify and save ▸"), "{frame}");
        type_text(&mut room, "/data/vinyl");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Map { vpath: "vinyl".into(), path: "/data/vinyl".into() }));
        room.queued = None;
        room.apply(Done::Mapped { vpath: "vinyl".into(), result: Err(ApiError::Server { status: 422, message: "daemon free-space returned -1".into() }) });
        assert!(matches!(&room.modal, Modal::Mapping { error: Some(e), .. } if e.contains("daemon free-space returned -1")));
        assert!(draw(&mut room).contains("− the daemon could not verify the path: server error 422"));
        room.queued = None;
        room.apply(Done::Mapped {
            vpath: "vinyl".into(),
            result: Ok(serde_json::json!({ "ok": true, "daemonPath": "/data/vinyl", "confidence": "verified" })),
        });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "vinyl mapped → /data/vinyl (verified)"));
        assert_eq!(room.queued, Some(Op::Load));
        room.queued = None;
        // t: the template, with the live preview, the suggestion and the clear.
        press(&mut room, KeyCode::Up);
        press(&mut room, KeyCode::Up);
        assert_eq!(room.selected_library(), Some("music"));
        room.note = None;
        press(&mut room, KeyCode::Char('t'));
        let frame = draw(&mut room);
        assert!(frame.contains("Path template — music"), "{frame}");
        assert!(frame.contains("{{ARTIST}} {{ALBUM}} {{YEAR}} {{GENRE}} {{ALBUMARTIST}}"), "{frame}");
        assert!(frame.contains("Pink Floyd/The Dark Side of the Moon (1973)"), "{frame}");
        assert!(frame.contains("^S suggested template") && frame.contains("^X clear") && frame.contains("Save ▸"), "{frame}");
        ctrl(&mut room, 'x');
        assert!(draw(&mut room).contains("(no template — the user types the path)"));
        type_text(&mut room, "{{GENRE}}/{{ARTIST}}");
        assert!(draw(&mut room).contains("Progressive Rock/Pink Floyd"));
        ctrl(&mut room, 's');
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Template { vpath: "music".into(), template: Some("{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into()) }));
        room.queued = None;
        room.apply(Done::TemplateSaved { vpath: "music".into(), result: Err(ApiError::Server { status: 400, message: "unknown variable".into() }) });
        assert!(matches!(&room.modal, Modal::Template { error: Some(e), .. } if e.contains("unknown variable")));
        room.apply(Done::TemplateSaved { vpath: "music".into(), result: Ok(TemplateSaved { ok: true, template: Some("{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into()), sample_path: None }) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "music: template saved"));
        press(&mut room, KeyCode::Char('t'));
        ctrl(&mut room, 'x');
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Template { vpath: "music".into(), template: None }));
        room.queued = None;
        room.apply(Done::TemplateSaved { vpath: "music".into(), result: Ok(TemplateSaved { ok: true, template: None, sample_path: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "music: template cleared"));
        room.note = None;
        assert!(row(&draw(&mut room), "music").contains("(none — typed by hand)"));
    }

    /// The SEARCH IN ticks measure their names in cells, as the users
    /// room's add form does: two libraries named by ten Japanese
    /// characters draw whole, 24 cells each and three apart, each click on
    /// its tick; and a first name wider than the row by itself (fifty
    /// characters, 100 cells) is still drawn, clamped to the body's right
    /// edge and cut cleanly, with the next left off, rather than leaving
    /// the row empty. The body's edges are read off the add card's click,
    /// which spans it.
    #[test]
    fn the_seeding_ticks_measure_japanese_names_in_cells_and_clamp_the_first() {
        let _en = english();
        let mut room = connected();
        room.tab = Tab::Seeding;
        let tick_text = |room: &Room, buffer: &ratatui::buffer::Buffer, i: usize| {
            let rect = room.cells.ui.clicks.iter().find(|(_, a)| *a == Act::SeedTick(i)).map(|(r, _)| *r);
            rect.map(|r| (r, crate::setup::tests::cells_text(buffer, r.y, r.x, r.right())))
        };
        let short = ["日本の音楽アーカイブ", "クラシック全集の録音"];
        room.seed_ticks = short.iter().map(|n| (n.to_string(), false)).collect();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut room)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let (a, a_text) = tick_text(&room, &buffer, 0).expect("tick 0");
        let (b, b_text) = tick_text(&room, &buffer, 1).expect("tick 1");
        assert_eq!((a.width, b.width, b.x), (24, 24, a.right() + 3), "in cells, three apart");
        assert_eq!((a_text, b_text), (format!("[ ] {}", short[0]), format!("[ ] {}", short[1])), "whole");

        let long = "日本の音楽アーカイブ".repeat(5);
        room.seed_ticks = vec![(long.clone(), false), ("music".to_string(), false)];
        terminal.draw(|frame| render(frame, &mut room)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let body = room.cells.ui.clicks.iter().find(|(_, a)| *a == Act::SeedAdd).map(|(r, _)| *r).expect("the add card");
        let (first, text) = tick_text(&room, &buffer, 0).expect("the first tick is drawn");
        assert_eq!(first.right(), body.right(), "clamped to the body: {first:?} in {body:?}");
        assert!(text.len() > "[ ] ".len() && format!("[ ] {long}").starts_with(text.trim_end()), "cut, not garbled: {text:?}");
        assert_eq!(tick_text(&room, &buffer, 1), None, "no room is left for the next");
    }

    #[test]
    fn seeding_queues_files_one_at_a_time_and_reads_the_outcomes() {
        let _en = english();
        let dir = std::env::temp_dir().join(format!("mstream-player-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("boc.torrent");
        let second = dir.join("kob.torrent");
        std::fs::write(&first, b"d8:announce0:e").unwrap();
        std::fs::write(&second, b"d8:announce0:e").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let mut room = connected();
        press(&mut room, KeyCode::Right);
        press(&mut room, KeyCode::Right);
        assert_eq!(room.tab, Tab::Seeding);
        let frame = draw(&mut room);
        assert!(frame.contains("Add a .torrent file ▸") && frame.contains("SEARCH IN") && frame.contains("[ ] music"), "{frame}");
        assert!(frame.contains("none ticked = every library") && frame.contains("(no files checked yet)"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · a add a .torrent · 1–9 tick a library · c clear results · Esc back"), "{frame}");
        press(&mut room, KeyCode::Char('2')); // audiobooks · music · …
        assert!(draw(&mut room).contains("[✓] music"));
        press(&mut room, KeyCode::Char('a'));
        assert!(matches!(room.modal, Modal::SeedPath { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Add a .torrent file") && frame.contains(".TORRENT FILE ON THIS MACHINE") && frame.contains("Check and seed ▸"), "{frame}");
        assert!(frame.contains("type · Tab complete · Enter check and seed · Esc cancel"), "{frame}");
        // Tab completes what is typed: folders and .torrent files only.
        type_text(&mut room, &format!("{}/", dir.display()));
        press(&mut room, KeyCode::Tab);
        listed(&mut room);
        let Modal::SeedPath { path, matches, .. } = &room.modal else { panic!("the path modal") };
        assert_eq!(matches, &vec!["boc.torrent".to_string(), "kob.torrent".to_string()]);
        assert!(path.value().ends_with("/"), "two candidates share no prefix beyond the folder: {}", path.value());
        type_text(&mut room, "b");
        press(&mut room, KeyCode::Tab);
        listed(&mut room);
        let Modal::SeedPath { path, .. } = &room.modal else { panic!("the path modal") };
        assert!(path.value().ends_with("/boc.torrent"), "{}", path.value());
        assert!(draw(&mut room).contains("▸ boc.torrent"));
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(room.seeds.len(), 1);
        assert_eq!(room.queued, Some(Op::Seed { path: first.clone(), vpaths: vec!["music".into()] }));
        assert!(draw(&mut room).contains("⟳ checking…"));
        // A second file waits its turn.
        press(&mut room, KeyCode::Char('a'));
        type_text(&mut room, &second.display().to_string());
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.seeds.len(), 2);
        assert_eq!(room.seed_queue.len(), 1);
        room.queued = None;
        room.apply(Done::Seeded {
            file: "boc.torrent".into(),
            result: Ok(SeedOutcome { ok: true, outcome: "seeded".into(), vpath: Some("music".into()), added_at: Some("/downloads/music/Boards of Canada".into()), ..Default::default() }),
        });
        assert_eq!(room.queued, Some(Op::Seed { path: second.clone(), vpaths: vec!["music".into()] }), "the queue advances after the list refresh is asked for");
        room.queued = None;
        room.apply(Done::Seeded {
            file: "kob.torrent".into(),
            result: Ok(SeedOutcome {
                ok: true,
                outcome: "partial_match".into(),
                vpath: Some("music".into()),
                matched: Some(9),
                total: Some(12),
                missing: vec!["10 - Flamenco Sketches (alt).flac".into(), "cover.jpg".into(), "log.txt".into(), "x.cue".into()],
                ..Default::default()
            }),
        });
        let frame = draw(&mut room);
        let boc = row(&frame, "boc.torrent");
        assert!(boc.contains("✓ seeding") && boc.contains("music → /downloads/music/Boards of Canada"), "{boc}");
        let kob = row(&frame, "kob.torrent");
        assert!(kob.contains("~ partial") && kob.contains("9/12 matched in music · missing: 10 - Flamenco Sk"), "{kob}");
        assert_eq!(outcome_details(&room.seeds[1], "Transmission"), "9/12 matched in music · missing: 10 - Flamenco Sketches (alt).flac, cover.jpg, log.txt, +1 more");
        // Other outcomes, in the webapp's words.
        let words = |o: &str, extra: SeedOutcome| {
            let row = SeedRow { file: "f".into(), outcome: Some(SeedOutcome { outcome: o.into(), ..extra }), error: None };
            (outcome_words(o).0, outcome_details(&row, "Transmission"))
        };
        assert_eq!(words("match_unmapped", SeedOutcome { vpath: Some("vinyl".into()), matched_root: Some("/Volumes/Vinyl".into()), mapping_confidence: Some("unconfirmed".into()), ..Default::default() }).1,
            "all files in vinyl at /Volumes/Vinyl — the mapping for vinyl is not confirmed: map it on the Libraries tab, then retry");
        assert_eq!(words("pad_files_missing", SeedOutcome { vpath: Some("music".into()), pad_files_present: Some(0), pad_files_total: Some(3), ..Default::default() }).0, "! needs padding");
        assert_eq!(words("no_match", SeedOutcome { checked_vpaths: vec!["music".into(), "vinyl".into()], ..Default::default() }), ("✗ not found".into(), "not in music, vinyl".into()));
        assert_eq!(words("already_in_daemon", SeedOutcome::default()).0, "⊝ already there");
        assert_eq!(words("invalid_torrent", SeedOutcome { error: Some("not a bencoded metainfo".into()), ..Default::default() }), ("✗ invalid".into(), "not a bencoded metainfo".into()));
        // A wrong extension is refused before anything is read; c clears the finished rows.
        press(&mut room, KeyCode::Char('a'));
        type_text(&mut room, &dir.join("notes.txt").display().to_string());
        press(&mut room, KeyCode::Enter);
        assert!(matches!(&room.modal, Modal::SeedPath { error: Some(e), .. } if e == "that is not a .torrent file"));
        press(&mut room, KeyCode::Esc);
        press(&mut room, KeyCode::Char('c'));
        assert!(room.seeds.is_empty());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "results cleared"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn access_flips_the_policy_and_the_users_and_leaves_with_one_user() {
        let _en = english();
        let mut room = connected();
        for _ in 0..3 {
            press(&mut room, KeyCode::Right);
        }
        assert_eq!(room.tab, Tab::Access);
        let frame = draw(&mut room);
        assert!(frame.contains("WHO CAN ADD TORRENTS") && frame.contains("(•) all users") && frame.contains("( ) whitelist — only the users ticked below"), "{frame}");
        for word in ["USER", "ADMIN", "TORRENTS"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(row(&frame, "iros").contains("admin") && row(&frame, "iros").contains("[✓]"), "{frame}");
        assert!(row(&frame, "guest").contains("[ ]"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · p policy (all ⇄ whitelist) · Esc back"), "{frame}");
        press(&mut room, KeyCode::Char('p'));
        assert_eq!(room.queued, Some(Op::SetPolicy("whitelist".into())));
        room.queued = None;
        room.apply(Done::PolicySet { policy: "whitelist".into(), result: Ok(()) });
        room.note = None;
        assert!(draw(&mut room).contains("(•) whitelist"));
        // dj-tom · guest · iros · mira
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        assert_eq!(room.selected_user(), Some("guest"));
        let frame = draw(&mut room);
        assert!(frame.contains("guest may not add torrents · s flips it, at once"), "{frame}");
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::UserAccess { user: "guest".into(), allow: true }));
        room.queued = None;
        room.apply(Done::UserAccess { user: "guest".into(), allow: true, result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "guest may add torrents"));
        assert!(row(&draw(&mut room), "guest").contains("[✓]"));
        // Down to one user: the tab is gone and the room moves off it.
        room.queued = None;
        room.apply(loaded(params("transmission", true), users(1), true));
        assert_eq!(room.tabs(), vec![Tab::Torrents, Tab::Libraries, Tab::Seeding, Tab::Client]);
        assert_eq!(room.tab, Tab::Torrents);
        assert!(!draw(&mut room).contains(" Access "));
    }

    #[test]
    fn the_client_tab_tests_switches_and_disconnects() {
        let _en = english();
        let mut room = connected();
        press(&mut room, KeyCode::Left);
        assert_eq!(room.tab, Tab::Client);
        let frame = draw(&mut room);
        assert!(frame.contains("TRANSMISSION   ● connected — Transmission 4.0.6"), "{frame}");
        for word in ["HOST", "127.0.0.1", "PORT", "9091", "(none)", "RPC PATH", "/transmission/rpc", "HTTPS", "no"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(frame.contains("Disconnecting forgets the credentials here; the daemon keeps every torrent it has."), "{frame}");
        assert!(frame.contains("Test") && frame.contains("Disconnect") && frame.contains("Switch client ▸"), "{frame}");
        assert!(frame.contains("polls every 30 s") && frame.contains("reachable just now"), "{frame}");
        assert!(frame.contains("←→ tab · t test · c switch client · x disconnect · Esc back"), "{frame}");
        press(&mut room, KeyCode::Char('t'));
        assert_eq!(room.queued, Some(Op::Status));
        room.queued = None;
        room.apply(Done::Status(Ok(TorrentStatus { connected: false, configured: true, reason: Some("connect ECONNREFUSED 127.0.0.1:9091".into()), ..Default::default() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n == "not reachable: connect ECONNREFUSED 127.0.0.1:9091"));
        let frame = draw(&mut room);
        assert!(frame.contains("• disconnected — connect ECONNREFUSED 127.0.0.1:9091 · credentials kept"), "{frame}");
        assert!(frame.contains("● disconnected — connect ECONNREFUSED 127.0.0.1:9091"), "{frame}");
        // x: the gate, Enter keeps, y goes.
        press(&mut room, KeyCode::Char('x'));
        assert!(matches!(room.modal, Modal::Disconnect));
        let frame = draw(&mut room);
        assert!(frame.contains("Disconnect from Transmission?") && frame.contains("y disconnect · Esc stay"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none());
        press(&mut room, KeyCode::Char('x'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Disconnect(Kind::Transmission)));
        room.queued = None;
        room.apply(Done::Disconnected(Ok(())));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "disconnected — the credentials are gone"));
        assert_eq!(room.queued, Some(Op::Load));
        // c: back to the client choice, with the other saved client shown; Esc returns.
        room.queued = None;
        let mut p = params("transmission", true);
        p.qbittorrent = TorrentClientConfig { host: "192.168.1.20".into(), port: 8080, configured: true, ..Default::default() };
        room.apply(loaded(p, users(4), true));
        room.tab = Tab::Client;
        room.note = None;
        let frame = draw(&mut room);
        assert!(frame.contains("ALSO SAVED  qBittorrent 192.168.1.20:8080 — c switches without retyping the password"), "{frame}");
        press(&mut room, KeyCode::Char('c'));
        assert_eq!(room.phase(), Phase::Choose);
        let frame = draw(&mut room);
        assert!(frame.contains("(•) Transmission") && frame.contains("Connect to Transmission ▸"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert_eq!(room.phase(), Phase::Tabs);
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn a_frame_lends_the_list_to_its_rows_and_hands_it_back() {
        // The rows draw from the list itself, taken for the frame and put
        // back rather than copied (performance audit #110): after a draw
        // the room holds every torrent, and the cursor's note is found
        // through the frame's own filter, whatever the case typed.
        let _en = english();
        let mut room = connected();
        press(&mut room, KeyCode::Char('/'));
        type_text(&mut room, "RAINBOWS");
        press(&mut room, KeyCode::Enter);
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("1 of 4 match") && frame.contains("Radiohead - In Rainbows"), "{frame}");
        assert!(frame.contains("e5f6e5f6e5f6… · Tracker gave HTTP response code 403"), "the cursor's note:\n{frame}");
        assert_eq!(room.torrents.len(), 4, "the list is back after the frame");
        // A hash matches too, in either case.
        press(&mut room, KeyCode::Char('/'));
        for _ in 0.."RAINBOWS".len() {
            press(&mut room, KeyCode::Backspace);
        }
        type_text(&mut room, "C3D4");
        assert_eq!(room.filtered().len(), 1);
        assert!(draw(&mut room).contains("linux-6.9.iso"));
        assert_eq!(room.torrents.len(), 4);
        // The Access tab lends the users the same way.
        press(&mut room, KeyCode::Enter);
        room.tab = Tab::Access;
        let frame = draw(&mut room);
        assert!(frame.contains("iros") && frame.contains("dj-tom"), "{frame}");
        assert_eq!(room.users.len(), 4, "the accounts are back after the frame");
    }

    /// `cargo test --release torrents_tab_frame_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check; run --release with --nocapture"]
    fn torrents_tab_frame_cost() {
        let _en = english();
        for n in [1_000, 5_000] {
            for filter in ["", "flac"] {
                let mut room = connected();
                let mut many = list();
                many.torrents = (0..n)
                    .map(|i| torrent(&format!("{i:04x}"), &format!("Some.Torrent.Name.{i:04}.FLAC-GROUP"), "seeding", 1.0, 0.0, 400_000_000, Some("iros")))
                    .collect();
                room.apply(Done::Listed(Ok(many)));
                if !filter.is_empty() {
                    room.filter = Input::new(filter.into());
                    room.sel = Some(0);
                }
                let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
                let frames = 200;
                let started = Instant::now();
                for _ in 0..frames {
                    terminal.draw(|frame| render(frame, &mut room)).unwrap();
                }
                let per = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
                println!("torrents tab, {n} torrents, filter {filter:?}: {per:.3} ms a frame");
            }
        }
    }

    #[test]
    fn a_list_at_rest_waits_for_the_thirty_second_load() {
        // A five-second look only while something moves — a download, a
        // check, a queue, or a list changed since the last look; at rest
        // the thirty-second load keeps it fresh (performance audit #100).
        let _en = english();
        let mut room = connected();
        let mut quiet = list();
        for t in &mut quiet.torrents {
            t.status = "seeding".into();
            t.percent = 1.0;
            t.rate_download = 0.0;
        }
        let look = |room: &mut Room| {
            room.queued = None;
            room.last_list = Some(Instant::now() - Duration::from_secs(6));
            room.tick();
            room.queued.take()
        };
        room.last_load = Some(Instant::now());
        // The downloads just finished: the list changed, so one more look.
        room.apply(Done::Listed(Ok(quiet.clone())));
        assert_eq!(look(&mut room), Some(Op::List));
        assert!(draw(&mut room).contains("polls every 5 s"));
        // The same again: nothing moves, nothing to look at until the load.
        room.apply(Done::Listed(Ok(quiet.clone())));
        assert_eq!(look(&mut room), None);
        assert!(draw(&mut room).contains("polls every 30 s"));
        room.last_load = Some(Instant::now() - Duration::from_secs(31));
        room.tick();
        assert_eq!(room.queued.take(), Some(Op::Load), "the load comes as ever, the list with it");
        room.last_load = Some(Instant::now());
        // One starts downloading: five seconds again, for as long as it runs.
        let mut moving = quiet.clone();
        moving.torrents[2].status = "downloading".into();
        room.apply(Done::Listed(Ok(moving.clone())));
        room.apply(Done::Listed(Ok(moving)));
        assert_eq!(look(&mut room), Some(Op::List));
        assert!(draw(&mut room).contains("polls every 5 s"));
        // Queued and checking count as moving too.
        for status in ["queued", "verifying"] {
            let mut waiting = quiet.clone();
            waiting.torrents[0].status = status.into();
            room.apply(Done::Listed(Ok(waiting.clone())));
            room.apply(Done::Listed(Ok(waiting)));
            assert_eq!(look(&mut room), Some(Op::List), "{status}");
        }
        // Off the Torrents tab the list is never polled on its own.
        room.tab = Tab::Libraries;
        assert_eq!(look(&mut room), None);
    }

    #[test]
    fn a_seed_path_tab_lists_its_folder_off_the_ui_thread() {
        // The folder is listed on a thread of its own — a dead network
        // mount can hang read_dir, and the key handler must not
        // (performance audit #85). A healthy folder answers within the
        // Tab's short wait, and completes at once as it always did.
        let _en = english();
        let dir = std::env::temp_dir().join(format!("mstream-player-tab-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("albums")).unwrap();
        std::fs::write(dir.join("boc.torrent"), b"d8:announce0:e").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        let mut room = connected();
        room.open_seed_path();
        type_text(&mut room, &format!("{}/b", dir.display()));
        room.seed_wait = Duration::from_secs(5);
        press(&mut room, KeyCode::Tab);
        let Modal::SeedPath { path, matches, asked, .. } = &room.modal else { panic!("the path modal") };
        assert_eq!(matches, &vec!["boc.torrent".to_string()]);
        assert!(path.value().ends_with("/boc.torrent") && asked.is_none(), "{}", path.value());
        assert!(room.seed_listing.is_empty());
        // A folder slower than the wait: the key returns with the listing
        // still out, the modal says so, and the completion lands with it.
        // The listing is held back by hand — the folder already being
        // listed, so this Tab starts no thread — and sent once the modal
        // has said so: a thread free to run could answer inside even a zero
        // wait on a loaded machine (this thread put off between the spawn
        // and the wait), which is the fast road above, not this one.
        room.seed_wait = Duration::ZERO;
        for _ in 0.."boc.torrent".len() {
            press(&mut room, KeyCode::Backspace);
        }
        type_text(&mut room, "b");
        let folder = format!("{}/", dir.display());
        room.seed_listing.insert(folder.clone());
        press(&mut room, KeyCode::Tab);
        assert!(
            matches!(&room.modal, Modal::SeedPath { asked: Some(_), matches, .. } if matches.is_empty()),
            "asked, not answered"
        );
        assert!(draw(&mut room).contains("listing…"));
        room.listed_tx.send((folder.clone(), list_local(&folder))).unwrap();
        listed(&mut room);
        let Modal::SeedPath { path, matches, .. } = &room.modal else { panic!("the path modal") };
        assert_eq!(matches, &vec!["boc.torrent".to_string()]);
        assert!(path.value().ends_with("/boc.torrent"), "{}", path.value());
        assert!(room.seed_listing.is_empty());
        // A Tab, then more typing before its listing lands: the completion
        // would be for an input that is gone, so it is dropped.
        for _ in 0.."boc.torrent".len() {
            press(&mut room, KeyCode::Backspace);
        }
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Char('a'));
        std::thread::sleep(Duration::from_millis(50));
        room.absorb();
        let Modal::SeedPath { path, matches, asked, .. } = &room.modal else { panic!("the path modal") };
        assert!(path.value().ends_with("/a") && matches.is_empty() && asked.is_none(), "{}", path.value());
        // Tabs at a folder already being listed ask no second time; its
        // one listing serves the latest.
        room.seed_listing.insert(format!("{}/", dir.display()));
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Tab);
        assert_eq!(room.seed_listing.len(), 1);
        room.take_listing(format!("{}/", dir.display()), vec!["albums/".into(), "boc.torrent".into()]);
        let Modal::SeedPath { path, asked, .. } = &room.modal else { panic!("the path modal") };
        assert!(path.value().ends_with("/albums/") && asked.is_none(), "{}", path.value());
        // A listing that lands after the modal closed changes nothing.
        room.act(Act::ModalCancel);
        room.take_listing(format!("{}/", dir.display()), vec!["boc.torrent".into()]);
        assert!(matches!(room.modal, Modal::None));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Hosted in the GUI player's Admin tab ─────────────────────────────

    use crate::admin::HostedRoom;
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
            let mut room = connected();
            room.note = Some(("the daemon answered".into(), false));
            let buf = hosting::draw_hosted(&mut room, size, area);
            let text = text_of(&buf);
            assert_eq!(hosting::outside(&buf, area, true), Vec::<(u16, u16)>::new(), "{size:?}\n{text}");
            assert!(!text.contains("Torrents beta") && !text.contains("home.mstream.example · admin"), "no header\n{text}");
            assert!(!text.contains(&*t!("tor.hint_torrents")), "the tips are the host's\n{text}");
            assert_eq!(cells(&buf, area.y, area.x, area.right()), "·".repeat(area.width as usize), "the first row is the host's");
            assert!(cells(&buf, area.bottom() - 1, area.x + 2, area.right()).starts_with("the daemon answered"), "{text}");
            assert!(text.contains(" Torrents · 4 ") && text.contains("Boards of C"), "{text}");
        }
    }

    #[test]
    fn hosted_the_beta_chip_rides_the_bodys_first_row() {
        let _en = english();
        let mut room = connected();
        let buf = hosting::draw_hosted(&mut room, (100, 30), WINDOW);
        let text = text_of(&buf);
        // The column runs x 19..98; the chip ends flush with it, and the
        // state line stops two cells short of the chip.
        assert_eq!(cells(&buf, 2, 94, 98), "beta", "{text}");
        assert_eq!(cells(&buf, 2, 92, 94), "··", "{text}");
        assert!(cells(&buf, 2, 19, 98).starts_with(&*t!("tor.state_connected")), "{text}");
        // Wide enough for the poll note, it sits right-aligned beside the
        // chip with the same gap.
        let wide = Rect { x: 17, y: 1, width: 143, height: 23 };
        let buf = hosting::draw_hosted(&mut room, (160, 30), wide);
        let text = text_of(&buf);
        assert_eq!(cells(&buf, 2, 154, 158), "beta", "{text}");
        let polls = t!("tor.polls").to_string();
        let start = 152 - kit::width(&polls) as u16;
        assert_eq!(cells(&buf, 2, start, 154), format!("{polls}··"), "{text}");
        // Standalone, the chip still sits beside the title.
        assert!(draw(&mut room).lines().next().unwrap().starts_with("  Torrents beta"));
    }

    #[test]
    fn hosted_at_the_floor_the_choose_and_connect_pages_draw_without_panicking() {
        let _en = english();
        for (client, users_n, phase) in [("disabled", 4, Phase::Choose), ("transmission", 1, Phase::Connect)] {
            let mut room = new_room();
            room.queued = None;
            room.apply(loaded(params(client, false), users(users_n), false));
            assert_eq!(room.phase(), phase);
            let buf = hosting::draw_hosted(&mut room, (100, 24), FLOOR);
            let text = text_of(&buf);
            assert_eq!(hosting::outside(&buf, FLOOR, false), Vec::<(u16, u16)>::new(), "{phase:?}\n{text}");
            assert!(!text.contains(&*t!("resize")), "{phase:?}\n{text}");
            assert_eq!(cells(&buf, 2, 94, 98), "beta", "{phase:?}\n{text}");
        }
    }

    #[test]
    fn hosted_seeding_digits_still_reach_the_room() {
        let _en = english();
        let mut room = connected();
        press(&mut room, KeyCode::Right);
        press(&mut room, KeyCode::Right);
        assert_eq!(room.tab, Tab::Seeding);
        assert_eq!(room.claims(), Claim::Open, "the host keeps its hands off a digit");
        assert!(HostedRoom::press(&mut room, KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)).is_none());
        assert_eq!(room.ticked_vpaths(), vec!["music".to_string()]);
        assert_eq!(room.tips(), t!("tor.hint_seeding"));
        let text = text_of(&hosting::draw_hosted(&mut room, (100, 30), WINDOW));
        assert!(text.contains("[✓] music"), "{text}");
    }

    /// The cell a focused field notes is the caret's, masked or not, however
    /// the field's window over a long value stands (hosted, it is where the
    /// GUI's window floats the input method's candidates).
    #[test]
    fn the_filter_and_the_connect_fields_note_the_cell_their_caret_is_drawn_in() {
        let _en = english();
        let mut room = connected();
        press(&mut room, KeyCode::Char('/'));
        // 77 of the filter's 80 characters, past its 74 cells.
        type_text(&mut room, &format!("{}end", "z".repeat(74)));
        crate::admin::assert_caret_between(&draw(&mut room), &room.cells.ui, "zzzend", "");
        press(&mut room, KeyCode::Home);
        crate::admin::assert_caret_between(&draw(&mut room), &room.cells.ui, "/ ", "zzz");

        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        assert_eq!(room.phase(), Phase::Connect);
        type_text(&mut room, &format!(".{}end", "h".repeat(80)));
        crate::admin::assert_caret_between(&draw(&mut room), &room.cells.ui, "hhhend", "");
        for _ in 0..3 {
            press(&mut room, KeyCode::Tab);
        }
        type_text(&mut room, &"s".repeat(60));
        crate::admin::assert_caret_between(&draw(&mut room), &room.cells.ui, "•••", "");
    }

    /// The gold gate places its two buttons by their width in cells, as the
    /// kit draws them: with Japanese labels ("◂ 残す" is four characters and
    /// six cells) a count of characters set the pair two cells short each,
    /// and the danger button, chained off the safe one, ran over the
    /// modal's right border and out past it, click rect and all. The pair
    /// ends where the English one does, each label whole. The labels are
    /// handed in, so no locale is switched under other tests.
    #[test]
    fn the_gate_keeps_a_cjk_pair_of_buttons_inside_its_modal() {
        let _en = english();
        let gate = |safe: &str, go: &str| {
            let mut room = new_room();
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    let body = vec!["one".to_string(), "two".to_string()];
                    let pair = ((safe.to_string(), Act::RemoveCancel), (go.to_string(), Act::RemoveConfirm));
                    draw_gate(frame, &mut room, area, "Remove?".to_string(), body, pair.0, pair.1);
                })
                .unwrap();
            let rect = |act: Act| room.cells.ui.clicks.iter().find(|(_, a)| *a == act).map(|(r, _)| *r).expect("the button");
            (terminal.backend().buffer().clone(), rect(Act::RemoveCancel), rect(Act::RemoveConfirm))
        };
        // A rect's text, a wide character's hidden cell left out.
        let shown = |buf: &Buffer, rect: Rect| {
            let (mut text, mut x) = (String::new(), rect.x);
            while x < rect.right() {
                text.push_str(buf[(x, rect.y)].symbol());
                x += (kit::width(buf[(x, rect.y)].symbol()) as u16).max(1);
            }
            text
        };
        let (_, _, english_go) = gate("◂ keep", "remove");
        for (safe, go) in [("◂ 残す", "削除"), ("◂ 残る", "切断"), ("◂ 保留", "移除")] {
            let (buf, safe_rect, go_rect) = gate(safe, go);
            assert_eq!(go_rect.right(), english_go.right(), "{go}: the pair ends at the modal's inner edge");
            assert_eq!(go_rect.width as usize, kit::width(go) + 4, "{go}: its whole label");
            assert_eq!(safe_rect.right() + 2, go_rect.x, "{safe}: two cells between the pair");
            assert_eq!(shown(&buf, safe_rect), format!("  {safe}  "), "{safe}: whole");
            assert_eq!(shown(&buf, go_rect), format!("  {go}  "), "{go}: whole");
        }
    }
}
