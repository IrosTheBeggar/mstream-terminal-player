//! The Torrents room drawn by egui, in the desktop window's panel: the same
//! [`Room`] the cells view draws (the parent module: its state, its worker,
//! its keys, its words), on the Studio kit (src/gui/window/studio.rs), after
//! the Admin canvas's Torrents boards. Only the drawing is new. Every click
//! is the very [`Act`] the cells view registers for the same control,
//! applied through [`Room::act`] in the pass, so the keyboard and the
//! pointer stay one model. Keys never reach egui: each text field is drawn
//! from the room's own value and caret, which its key handler edits as it
//! always has. What the room says — the state line, the tabs, the rows, the
//! notes, the modals — comes from the same `t!` keys and the same helpers
//! the cells view calls; the cells' bullets and status glyphs are the
//! desk's LEDs here, and their tick boxes the kit's.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::epaint::StrokeKind;
use egui::{Align, Align2, Color32, FontId, Id, Layout, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};

use super::*;
use crate::admin::{EguiShown, fmt_bytes};
use crate::gui::window::studio::{
    self, CELL_PAD, Col, Dialog, FIELD_H, FIELD_ROW_H, FORM_BUTTON_H, Glyph, HIT, Input as Field, Pal, Para, Run, RowCtx, SEG_H,
    Seg, TABLE_HEAD, Table, bare, body, cadence, caps, card, cell, cell_tipped, check_line, checkbox, chip, field_at,
    field_box, foot, foot_line, form_button, form_button_width, gate, ink, led, led_cell, led_mid, meter, meter_lit, mono,
    name_cell, path_cell, pointer, radio, ring_since, room_rects, segmented, small_caps, small_caps_width, state_line, strong,
    text, unkeyed, verdict, width_of, with_figures,
};

/// The room between the page's rows, and between the page and its note
/// line: the canvas's.
const GAP: f32 = studio::ROOM_GAP;
/// A line of prose.
const LINE_H: f32 = 20.0;
/// A table's headings and one row: with less room than this a table is
/// not drawn, as the cells leave off a table with under three rows.
const TABLE_MIN: f32 = TABLE_HEAD + 40.0;
/// An entry modal's width.
const ENTRY_W: f32 = 600.0;
/// The connect form's width, its wide fields' and the port's, the room
/// between its rows, and the few points more over its first row and over
/// its verdict (the canvas's).
const FORM_W: f32 = 632.0;
const WIDE_W: f32 = 480.0;
const PORT_W: f32 = 128.0;
const FORM_GAP: f32 = 20.0;
const FORM_NUDGE: f32 = 4.0;
/// A labelled field's height: its label, the room under it, its box.
const SLOT_H: f32 = FIELD_ROW_H;
/// The key column of the Client tab's saved fields.
const KEY_W: f32 = 120.0;

/// What a pass gathers for the room: the acts its clicks asked for, in
/// order, and the caret of the field that has the keyboard.
#[derive(Default)]
struct Out {
    acts: Vec<Act>,
    caret: Option<Pos2>,
}

/// One pass of the room over the whole of `ui`, the region its host gave
/// it: the page the room is on, its note line, and any modal over them.
/// `composition` is the input method's uncommitted text, drawn in the
/// field that has the keyboard and in no other. The clicks are applied
/// through [`Room::act`] before it returns; what it returns is what the
/// host must know: the focused field's caret, and an outcome a click ended
/// the room with (none of the room's clicks does).
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    let pal = Pal::now();
    let full = ui.max_rect();
    let (page_rect, note_rect) = room_rects(full);
    let modal_up = room.modal_open();
    // The filter runs over the daemon's whole list once a pass, as the
    // cells run it once a frame: the Torrents tab's rows and the note
    // line's story share it.
    let filtered = if room.phase() == Phase::Tabs && room.tab == Tab::Torrents { room.filtered() } else { Vec::new() };
    let mut out = Out::default();
    let mut page = ui.new_child(UiBuilder::new().id_salt("tor-page").max_rect(page_rect).layout(Layout::top_down(Align::Min)));
    // A radio row's hover reaches 8 points past the column, as the canvas's does.
    page.set_clip_rect(page_rect.expand2(vec2(9.0, 1.0)).intersect(ui.clip_rect()));
    draw_page(&mut page, room, &filtered, &pal, if modal_up { "" } else { composition }, &mut out);
    note(ui, note_rect, room, &filtered, &pal);
    if modal_up {
        // The room beneath a modal is inert, as the cells drop what it
        // registered: no click under it counts, and its field has not the
        // keyboard — the modal's has.
        out = Out::default();
        draw_modal(ui, room, &pal, full, composition, &mut out);
    }
    let mut outcome = None;
    for act in out.acts {
        outcome = outcome.or(room.act(act));
    }
    EguiShown { caret: out.caret, outcome }
}

// ── The page ──────────────────────────────────────────────────────────────

/// The page the room is on, top-down in `ui`: the state line, then the
/// client choice, the connect form or the tabs. `filtered` is the
/// Torrents tab's rows: the filter's pass over the list.
fn draw_page(ui: &mut Ui, room: &Room, filtered: &[usize], pal: &Pal, composition: &str, out: &mut Out) {
    state_row(ui, room, pal);
    ui.add_space(GAP);
    match room.phase() {
        Phase::Loading => {}
        Phase::Choose => choose(ui, room, pal, out),
        Phase::Connect => {
            if let Some(c) = &room.connect {
                connect(ui, c, pal, composition, out);
            }
        }
        Phase::Tabs => tabs(ui, room, filtered, pal, composition, out),
    }
}

/// The state line in parts: its head with the cells' bullet left off (an
/// LED stands in for it), whether the head is set bold, the detail after
/// it and the figures in it (the version, the host), the tone of the LED
/// and the head, and the poll note.
struct StateLine {
    head: String,
    bold: bool,
    detail: String,
    figures: Vec<String>,
    tone: Color32,
    polls: Option<String>,
}

/// The webapp's status card in one row, in the words the cells' state
/// line uses.
fn state_words(room: &Room, pal: &Pal) -> StateLine {
    let kind = room.kind();
    if kind == Kind::Disabled {
        let head = bare(&t!("tor.state_off")).to_string();
        let detail = t!("tor.state_off_detail").to_string();
        return StateLine { head, bold: false, detail, figures: Vec::new(), tone: pal.dim, polls: None };
    }
    if !room.config().is_some_and(|c| c.configured) {
        return StateLine {
            head: bare(&t!("tor.state_picked", client = kind.as_str())).to_string(),
            bold: true,
            detail: t!("tor.state_picked_detail").to_string(),
            figures: Vec::new(),
            tone: pal.gold,
            polls: None,
        };
    }
    let polls = if room.tab == Tab::Torrents && room.list_live() { t!("tor.polls") } else { t!("tor.polls_slow") };
    let (head, detail, figures, tone) = match &room.status {
        None => (
            t!("tor.state_asking", client = kind.as_str()).to_string(),
            t!("tor.state_asking_detail").to_string(),
            Vec::new(),
            pal.dim,
        ),
        Some(s) if s.connected => {
            let host = printable(&room.config().map(|c| format!("{}:{}", c.host, c.port)).unwrap_or_default(), 80);
            let version = version_words(s);
            let detail = t!(
                "tor.state_connected_detail",
                client = kind.label(),
                version = version.clone(),
                host = host.clone(),
                reachable = room.reachable(),
                total = room.libraries.len()
            );
            (t!("tor.state_connected").to_string(), detail.to_string(), vec![version, host], pal.ok)
        }
        Some(s) => (
            t!("tor.state_down").to_string(),
            t!("tor.state_down_detail", reason = printable(s.reason.as_deref().unwrap_or(""), 120)).to_string(),
            Vec::new(),
            pal.gold,
        ),
    };
    StateLine { head: bare(&head).to_string(), bold: true, detail, figures, tone, polls: Some(polls.to_string()) }
}

/// Where users add torrents from a phone: the server's host and its port,
/// as the address must be typed (the shared `host_of` names the host
/// alone, for a header).
fn phone_host(room: &Room) -> String {
    let server = room.client.server();
    url::Url::parse(&server)
        .ok()
        .and_then(|u| u.host_str().map(|h| match u.port() {
            Some(port) => format!("{h}:{port}"),
            None => h.to_string(),
        }))
        .unwrap_or(server)
}

/// The page's first row: the state line on the left behind its LED, the
/// poll note right of it when both fit, and the gold BETA chip flush right
/// — the chip alone while the room loads, and on a connect page with no
/// form yet.
fn state_row(ui: &mut Ui, room: &Room, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    let beta = chip(ui, row.right_center(), Align2::RIGHT_CENTER, &t!("tor.beta"), pal.gold);
    let paged = match room.phase() {
        Phase::Loading => false,
        Phase::Connect => room.connect.is_some(),
        Phase::Choose | Phase::Tabs => true,
    };
    if !paged {
        return;
    }
    let state = state_words(room, pal);
    let head_font = if state.bold { strong(13.5) } else { body(13.5) };
    let figures: Vec<&str> = state.figures.iter().map(String::as_str).collect();
    let mut parts = vec![(state.head.clone(), head_font, state.tone)];
    parts.extend(with_figures(&state.detail, &figures, body(13.5), mono(12.5), pal.soft));
    let polls = state.polls.map(|p| cadence(&p, pal)).unwrap_or_default();
    let line = Rect::from_min_max(row.min, pos2((beta.min.x - 16.0).max(row.min.x), row.max.y));
    state_line(ui, line, state.tone, &parts, &polls);
}

// ── Painting helpers ──────────────────────────────────────────────────────

/// One line of words, a row of the layout, cut where it runs long.
fn line(ui: &mut Ui, s: &str, font: FontId, color: Color32) {
    studio::line(ui, LINE_H, s, font, color);
}

/// A group's label, a row of the layout, and the room under it.
fn label(ui: &mut Ui, s: &str, pal: &Pal) {
    studio::label(ui, s, pal);
    ui.add_space(6.0);
}

/// A modal's error line in gold, a row of the layout: the kit's verdict,
/// the cells' dash as its mark, then the words.
fn error_row(ui: &mut Ui, words: &str, pal: &Pal) {
    let w = ui.available_width();
    let at = ui.cursor().min;
    let drawn = verdict(ui, at, w, words, &[], pal.gold, Some(false));
    ui.allocate_exact_size(vec2(w, drawn.height().max(18.0)), Sense::hover());
}

/// A button's label bare of the key the cells lead it with, the key left
/// to the room's tips row: the canvas's buttons carry the word alone.
fn word_of(label: &str) -> &str {
    unkeyed(label).0
}

/// A flag that is information, not a lever, as an LED: lit `on` in `tone`,
/// or, off, the LED's outline in the dim, so an off flag still shows.
fn flag_led(ui: &Ui, at: Pos2, on: bool, tone: Color32, pal: &Pal) {
    if on {
        led(ui, at, tone);
    } else {
        let rect = Rect::from_center_size(at, vec2(6.0, 6.0));
        ui.painter().rect_stroke(rect, egui::CornerRadius::same(1), Stroke::new(1.0, pal.dim), StrokeKind::Inside);
    }
}

// ── The client page ───────────────────────────────────────────────────────

/// The client page: the pitch, the client radio group, the card that
/// applies the pick, and — with someone to whitelist, where there is room
/// — who may add torrents.
fn choose(ui: &mut Ui, room: &Room, pal: &Pal, out: &mut Out) {
    line(ui, &t!("tor.pitch_1"), body(14.0), pal.text);
    line(ui, &t!("tor.pitch_2"), body(14.0), pal.text);
    line(ui, &t!("tor.mobile_note", host = phone_host(room)), body(13.0), pal.dim);
    ui.add_space(GAP);
    let groups = room.many_users();
    label(ui, &t!("tor.client_label"), pal);
    let top = ui.cursor().min.y;
    for (i, kind) in Kind::ALL.iter().enumerate() {
        let id = Id::new("tor-client").with(i);
        if radio(ui, id, i == room.client_pick, &kind.label(), &kind.description(), pal).clicked() {
            out.acts.push(Act::PickClient(i));
        }
    }
    if groups && room.group == 0 {
        ring_since(ui, top, pal);
    }
    ui.add_space(14.0);
    let pick = Kind::ALL[room.client_pick.min(Kind::ALL.len() - 1)];
    let offer = if pick != Kind::Disabled {
        Some((t!("tor.connect_card", client = pick.label()).to_string(), pal.ok))
    } else if room.kind() != Kind::Disabled {
        Some((t!("tor.off_card").to_string(), pal.gold))
    } else {
        None
    };
    if let Some((words, tone)) = offer {
        if card(ui, Id::new("tor-card"), &words, Glyph::None, tone, true, pal).clicked() {
            out.acts.push(Act::ApplyClient);
        }
        ui.add_space(GAP);
    }
    if groups && ui.available_height() >= 20.0 + 60.0 {
        label(ui, &t!("tor.policy_label"), pal);
        let top = ui.cursor().min.y;
        let whitelist = room.policy_whitelist();
        let policy = [
            (t!("tor.policy_all"), t!("tor.policy_all_desc")),
            (t!("tor.policy_whitelist"), t!("tor.policy_whitelist_desc")),
        ];
        for (i, (name, desc)) in policy.iter().enumerate() {
            if radio(ui, Id::new("tor-policy").with(i), (i == 1) == whitelist, name, desc, pal).clicked() {
                out.acts.push(Act::PickPolicy(i));
            }
        }
        if room.group == 1 {
            ring_since(ui, top, pal);
        }
    }
}

// ── The connect form ──────────────────────────────────────────────────────

/// The connect form: the chosen client's fields in the cells' order (which
/// is also the keys' order), the HTTPS tick, the verdict, and the test and
/// connect buttons on the page's last row.
fn connect(ui: &mut Ui, c: &Connect, pal: &Pal, composition: &str, out: &mut Out) {
    let area = ui.available_rect_before_wrap();
    let x = area.min.x;
    let form_w = area.width().min(FORM_W);
    let wide = (form_w - PORT_W - 24.0).clamp(80.0, WIDE_W);
    let half = ((form_w - 24.0) / 2.0).max(80.0);
    let fields = c.fields();
    let slot = |x: f32, y: f32, w: f32| Rect::from_min_size(pos2(x, y), vec2(w, SLOT_H));
    let mut y = area.min.y + FORM_NUDGE;
    place(ui, c, CField::Host, slot(x, y, wide), &fields, composition, pal, out);
    place(ui, c, CField::Port, slot(x + wide + 24.0, y, PORT_W), &fields, composition, pal, out);
    y += SLOT_H + FORM_GAP;
    if c.kind.has_username() {
        place(ui, c, CField::Username, slot(x, y, half), &fields, composition, pal, out);
        place(ui, c, CField::Password, slot(x + half + 24.0, y, half), &fields, composition, pal, out);
    } else {
        place(ui, c, CField::Password, slot(x, y, half), &fields, composition, pal, out);
    }
    y += SLOT_H + FORM_GAP;
    let https_at = if c.kind.has_rpc_path() {
        place(ui, c, CField::RpcPath, slot(x, y, wide), &fields, composition, pal, out);
        // On the RPC box's middle, beside it.
        let left = x + wide + 24.0;
        let at = Rect::from_min_size(pos2(left, y + SLOT_H - FIELD_H / 2.0 - HIT / 2.0), vec2((area.max.x - left).max(HIT), HIT));
        y += SLOT_H + FORM_GAP;
        at
    } else {
        let at = Rect::from_min_size(pos2(x, y), vec2(form_w, HIT));
        y += HIT + FORM_GAP;
        at
    };
    https(ui, c, https_at, pal, out);

    // The form's own complaint, the probe's answer, or qBittorrent's caveat.
    // The version and the RPC number the probe found are figures.
    let said = if let Some(e) = &c.error {
        Some((e.clone(), Vec::new(), pal.gold, Some(false)))
    } else {
        match &c.probe {
            Some(Ok((version, rpc))) => {
                let rpc_words = if rpc.is_empty() { String::new() } else { t!("tor.probe_rpc", n = rpc).to_string() };
                let words = t!("tor.probe_ok", client = c.kind.label(), version = version, rpc = rpc_words);
                Some((words.to_string(), vec![version.clone(), rpc.clone()], pal.ok, Some(true)))
            }
            Some(Err(e)) => Some((t!("tor.probe_failed", err = e).to_string(), Vec::new(), pal.gold, Some(false))),
            None if c.kind == Kind::Qbittorrent => Some((t!("tor.csrf_note").to_string(), Vec::new(), pal.gold, None)),
            None => None,
        }
    };
    // Left off when the page has no room left for it, as in the cells.
    if let Some((words, figures, tone, marked)) = said
        && y + FORM_NUDGE < area.max.y
    {
        let figures: Vec<&str> = figures.iter().map(String::as_str).collect();
        y = verdict(ui, pos2(x, y + FORM_NUDGE), form_w, &words, &figures, tone, marked).max.y + FORM_GAP;
    }

    // The page's last row, unless the form above leaves the buttons none:
    // the cells leave them off rather than draw them past the body.
    let by = (area.max.y - FORM_BUTTON_H).max(y);
    if by + FORM_BUTTON_H > area.max.y {
        return;
    }
    let connect = t!("tor.connect_button");
    let connect_w = form_button_width(ui, &connect);
    let connect_at = Rect::from_min_size(pos2(area.max.x - connect_w, by), vec2(connect_w, FORM_BUTTON_H));
    let test = t!("tor.test_button");
    let test = word_of(&test);
    let test_w = form_button_width(ui, test);
    let test_at = Rect::from_min_size(pos2(connect_at.min.x - 10.0 - test_w, by), vec2(test_w, FORM_BUTTON_H));
    if form_button(ui, Id::new("tor-test"), test_at, test, false, pal).clicked() {
        out.acts.push(Act::ConnectTest);
    }
    if form_button(ui, Id::new("tor-connect"), connect_at, &connect, true, pal).clicked() {
        out.acts.push(Act::ConnectSubmit);
    }
}

/// One text field of the connect form in `slot`, drawn from the draft's
/// own value and caret, masked where the cells mask it; a click on its box
/// gives it the keys, as in the cells.
#[allow(clippy::too_many_arguments)]
fn place(ui: &Ui, c: &Connect, field: CField, slot: Rect, fields: &[CField], composition: &str, pal: &Pal, out: &mut Out) {
    let (label, input) = match field {
        CField::Host => (t!("tor.field_host"), &c.host),
        CField::Port => (t!("tor.field_port"), &c.port),
        CField::Username => (t!("tor.field_username"), &c.username),
        CField::Password => (t!("tor.field_password"), &c.password),
        CField::RpcPath => (t!("tor.field_rpc"), &c.rpc_path),
        CField::Https => return,
    };
    let index = fields.iter().position(|f| *f == field).unwrap_or(0);
    let on = c.focused() == field;
    let mut drawn = Field::of(input).focused(on, composition);
    if matches!(field, CField::Host | CField::Port | CField::RpcPath) {
        drawn = drawn.mono();
    }
    if field == CField::Password {
        drawn = drawn.masked();
    }
    // The cells' placeholder, shown while the host is empty and has not the keys.
    if field == CField::Host && !on {
        drawn = drawn.placeholder("127.0.0.1");
    }
    let typed = field_at(ui, Id::new("tor-connect-field").with(index), slot, &label, &drawn, pal);
    if typed.response.clicked() {
        out.acts.push(Act::ConnectFocus(index));
    }
    if on {
        out.caret = typed.caret;
    }
}

/// The HTTPS tick in `at`, in the keyboard's look while it has the keys,
/// as every form's focused tick: a click flips it.
fn https(ui: &mut Ui, c: &Connect, at: Rect, pal: &Pal, out: &mut Out) {
    let mut row = ui.new_child(UiBuilder::new().id_salt("tor-https-row").max_rect(at).layout(Layout::top_down(Align::Min)));
    let focused = c.focused() == CField::Https;
    if check_line(&mut row, Id::new("tor-https"), c.https, &t!("tor.https"), "", focused, pal).clicked() {
        out.acts.push(Act::ConnectHttps);
    }
}

// ── The tabs ──────────────────────────────────────────────────────────────

fn tab_name(room: &Room, tab: Tab) -> String {
    match tab {
        Tab::Torrents if !room.torrents.is_empty() => t!("tor.tab_torrents_n", n = room.torrents.len()).to_string(),
        Tab::Torrents => t!("tor.tab_torrents").to_string(),
        Tab::Libraries => t!("tor.tab_libraries").to_string(),
        Tab::Seeding => t!("tor.tab_seeding").to_string(),
        Tab::Access => t!("tor.tab_access").to_string(),
        Tab::Client => t!("tor.tab_client").to_string(),
    }
}

fn tab_caption(tab: Tab) -> String {
    match tab {
        Tab::Torrents => t!("tor.note_torrents"),
        Tab::Libraries => t!("tor.note_libraries"),
        Tab::Seeding => t!("tor.note_seeding"),
        Tab::Access => t!("tor.note_access"),
        Tab::Client => t!("tor.note_client"),
    }
    .to_string()
}

/// Connected: the tab strip as one segmented group with the open tab's
/// caption right of it where it fits, then the open tab.
fn tabs(ui: &mut Ui, room: &Room, filtered: &[usize], pal: &Pal, composition: &str, out: &mut Out) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, SEG_H), Sense::hover());
    let tabs = room.tabs();
    let labels: Vec<String> = tabs.iter().map(|t| tab_name(room, *t)).collect();
    let chosen = tabs.iter().position(|t| *t == room.tab).unwrap_or(0);
    let (strip, clicked) = segmented(ui, Id::new("tor-tabs"), row.min.x, row.center().y, &labels, chosen, Seg::Lit, pal);
    if let Some(i) = clicked {
        out.acts.push(Act::Tab(tabs[i]));
    }
    let caption = tab_caption(room.tab);
    let caption_w = width_of(ui, &caption, body(12.5));
    if strip.max.x + 16.0 + caption_w <= row.max.x {
        text(ui, row.right_center(), &caption, body(12.5), pal.dim, caption_w + 1.0, Align2::RIGHT_CENTER);
    }
    ui.add_space(GAP);
    match room.tab {
        Tab::Torrents => torrents(ui, room, filtered, pal, composition, out),
        Tab::Libraries => libraries(ui, room, pal, out),
        Tab::Seeding => seeding(ui, room, pal, out),
        Tab::Access => access(ui, room, pal, out),
        Tab::Client => client(ui, room, pal, out),
    }
}

// ── Torrents ──────────────────────────────────────────────────────────────

/// Torrents: the filter and the count, then the daemon's list, `filtered`
/// the rows the filter lets through.
fn torrents(ui: &mut Ui, room: &Room, filtered: &[usize], pal: &Pal, composition: &str, out: &mut Out) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
    let count_w = 144.0;
    let field_at = Rect::from_min_max(row.min, pos2((row.max.x - count_w - 16.0).max(row.min.x + 80.0), row.max.y));
    let focused = room.filter_focus;
    // The hint stands while the filter is empty and has not the caret, as in the cells.
    let hint = if focused { String::new() } else { t!("tor.filter_hint").to_string() };
    let input = Field::of(&room.filter).focused(focused, composition).placeholder(&hint).search();
    let typed = field_box(ui, Id::new("tor-filter"), field_at, &input, pal);
    if typed.response.clicked() {
        out.acts.push(Act::FilterFocus);
    }
    if focused {
        out.caret = typed.caret;
    }
    let listed = room.list_loaded && room.list_error.is_none();
    if listed {
        let count = if !room.filter.value().trim().is_empty() {
            t!("tor.count_match", n = filtered.len(), total = room.torrents.len()).to_string()
        } else if room.torrents.len() == 1 {
            t!("tor.count_one").to_string()
        } else {
            t!("tor.count", n = room.torrents.len()).to_string()
        };
        text(ui, row.right_center(), &count, mono(12.0), pal.dim, count_w, Align2::RIGHT_CENTER);
    }
    ui.add_space(14.0);
    if ui.available_height() < TABLE_MIN {
        return;
    }
    let cols = [
        Col::left(t!("tor.col_name"), 0.0),
        Col::left(t!("tor.col_status"), 124.0),
        Col::left(t!("tor.col_progress"), 152.0),
        Col::right(t!("tor.col_down"), 88.0),
        Col::right(t!("tor.col_size"), 88.0),
        Col::left(t!("tor.col_by"), 120.0),
    ];
    let len = if listed { filtered.len() } else { 0 };
    let top = ui.cursor().min;
    let empty = if room.list_error.is_some() || !room.list_loaded {
        String::new()
    } else if room.torrents.is_empty() {
        t!("tor.empty_list").to_string()
    } else {
        t!("tor.empty_filter").to_string()
    };
    let table = Table::new(Id::new("tor-torrents"), &cols, len).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| torrent_row(ui, &room.torrents[filtered[i]], i, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
    if let Some(err) = &room.list_error {
        // Where the rows would be, in two lines at most.
        let words = t!("tor.list_error", err = printable(err, 200));
        Para::new(&words, body(13.5), pal.gold).rows(2).paint(ui, pos2(top.x + CELL_PAD, top.y + TABLE_HEAD + 10.0), w - 2.0 * CELL_PAD);
    }
}

/// The webapp's status colours: downloading in the accent, seeding green,
/// a check or an error gold, the idle states dim.
fn status_tone(status: &str, pal: &Pal) -> Color32 {
    match status {
        "downloading" => pal.accent,
        "seeding" => pal.ok,
        "verifying" | "error" => pal.gold,
        _ => pal.dim,
    }
}

/// One torrent: its name (cut, with the name and hash in a tip), its
/// status behind its LED, its progress as ten LED segments and a percent,
/// its rate, its size and who added it.
fn torrent_row(ui: &Ui, t: &Torrent, i: usize, row: &RowCtx, pal: &Pal) {
    let mid = row.rect.center().y;
    let name = printable(&t.name, 300);
    let font = if row.selected { strong(14.0) } else { body(14.0) };
    let tip = format!("{name} · {}", t.info_hash);
    cell_tipped(ui, row, 0, &name, &tip, font, ink(row, pal.text, pal), Id::new("tor-tip-name").with(i));
    let tone = status_tone(&t.status, pal);
    led_cell(ui, row, 1, Some(tone), &status_word(&t.status), body(13.0), tone);
    let pct = (t.percent * 100.0).round().clamp(0.0, 100.0) as u64;
    let lit = if t.status == "seeding" { pal.ok } else { pal.accent };
    let (x, w) = row.cells[2];
    let drawn = meter(ui, pos2(x, mid), 10, meter_lit(pct, 100, 10), vec2(8.0, 12.0), 2.0, lit, pal);
    text(ui, pos2((drawn.max.x + 46.0).min(x + w), mid), &format!("{pct}%"), mono(12.0), pal.dim, 36.0, Align2::RIGHT_CENTER);
    let down = if t.rate_download > 0.0 { pal.soft } else { pal.dim };
    cell(ui, row, 3, &rate_words(t.rate_download), mono(12.5), ink(row, down, pal), true);
    cell(ui, row, 4, &fmt_bytes(t.size_bytes), mono(12.5), ink(row, pal.soft, pal), true);
    let (by, tone) = match (t.managed_by_mstream, &t.managed_by) {
        (true, Some(u)) => (printable(u, NAME_MAX), pal.soft),
        (true, None) => ("mStream".to_string(), pal.soft),
        (false, _) => (t!("tor.by_external").to_string(), pal.dim),
    };
    cell(ui, row, 5, &by, body(13.5), ink(row, tone, pal), false);
}

// ── Libraries ─────────────────────────────────────────────────────────────

/// Libraries: the two lines on what the paths are, then the daemon's view
/// of each library and its template.
fn libraries(ui: &mut Ui, room: &Room, pal: &Pal, out: &mut Out) {
    let client = room.kind().label();
    line(ui, &t!("tor.intro_1", client = client.clone()), body(13.0), pal.dim);
    line(ui, &t!("tor.intro_2"), body(13.0), pal.dim);
    ui.add_space(14.0);
    if ui.available_height() < TABLE_MIN {
        return;
    }
    let cols = [
        Col::left(t!("tor.col_library"), 140.0),
        Col::left(t!("tor.col_access"), 140.0),
        Col::left(t!("tor.col_path", client = client.to_uppercase()), 0.0),
        Col::left(t!("tor.col_template"), 260.0),
    ];
    let empty = t!("tor.empty_libraries");
    let table = Table::new(Id::new("tor-libraries"), &cols, room.libraries.len()).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| library_row(ui, room, i, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
}

/// The confidence ladder's colours: verified green, probing in the accent,
/// the rest gold.
fn confidence_tone(confidence: &str, pal: &Pal) -> Color32 {
    match confidence {
        "verified" => pal.ok,
        "pending" => pal.accent,
        _ => pal.gold,
    }
}

/// One library: its name, its rung on the ladder behind its LED, the path
/// the daemon sees and its template, each cut with a tip.
fn library_row(ui: &Ui, room: &Room, i: usize, row: &RowCtx, pal: &Pal) {
    let name = &room.libraries[i];
    name_cell(ui, row, 0, &printable(name, NAME_MAX), pal);
    let access = room.access.get(name);
    let confidence = access.map(|a| a.confidence.as_str()).unwrap_or("unconfirmed");
    let tone = confidence_tone(confidence, pal);
    led_cell(ui, row, 1, Some(tone), &confidence_words(confidence), body(13.0), tone);
    match access.and_then(|a| a.daemon_path.clone()).filter(|p| !p.is_empty()) {
        Some(path) if confidence != "pending" => {
            // Cut at its start, so its leaf stays, as the canvas cuts paths.
            path_cell(ui, row, 2, &printable(&path, 300), mono(12.5), ink(row, pal.text, pal), Id::new("tor-tip-path").with(i));
        }
        _ if confidence == "pending" => {
            cell(ui, row, 2, &t!("tor.path_probing"), body(13.0), ink(row, pal.dim, pal), false);
        }
        _ => {
            cell(ui, row, 2, &t!("tor.path_none"), body(13.0), ink(row, pal.dim, pal), false);
        }
    }
    match room.templates.vpaths.get(name).and_then(|t| t.template.clone()).filter(|t| !t.trim().is_empty()) {
        Some(template) => {
            let template = printable(&template, 300);
            cell_tipped(ui, row, 3, &template, &template, mono(12.5), ink(row, pal.text, pal), Id::new("tor-tip-template").with(i));
        }
        None => {
            cell(ui, row, 3, &t!("tor.tmpl_none"), body(13.0), ink(row, pal.dim, pal), false);
        }
    }
}

// ── Seeding ───────────────────────────────────────────────────────────────

/// Seeding: the pitch, the add card, the SEARCH IN ticks, the results.
fn seeding(ui: &mut Ui, room: &Room, pal: &Pal, out: &mut Out) {
    line(ui, &t!("tor.seed_intro_1"), body(13.0), pal.dim);
    line(ui, &t!("tor.seed_intro_2"), body(13.0), pal.dim);
    ui.add_space(14.0);
    if card(ui, Id::new("tor-seed-card"), &t!("tor.seed_card"), Glyph::None, pal.ok, true, pal).clicked() {
        out.acts.push(Act::SeedAdd);
    }
    ui.add_space(14.0);
    ticks(ui, room, pal, out);
    ui.add_space(14.0);
    if ui.available_height() < TABLE_MIN {
        return;
    }
    let cols = [
        Col::left(t!("tor.col_file"), 220.0),
        Col::left(t!("tor.col_outcome"), 160.0),
        Col::left(t!("tor.col_details"), 0.0),
    ];
    let client = room.kind().label();
    let empty = t!("tor.seed_empty");
    let table = Table::new(Id::new("tor-seeding"), &cols, room.seeds.len()).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| seed_row(ui, &room.seeds[i], &client, i, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
}

/// SEARCH IN and a tick per library, a row of the layout: the first tick
/// always drawn, cut to the row, and a later one that does not fit left
/// off, as the cells do; with none ticked, what that means on the right
/// where it fits. A click on a tick or its name flips it.
fn ticks(ui: &mut Ui, room: &Room, pal: &Pal, out: &mut Out) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, HIT), Sense::hover());
    let mid = row.center().y;
    let search = t!("tor.seed_search");
    small_caps(ui, row.left_center(), &search, pal.dim, w, Align2::LEFT_CENTER);
    let mut x = row.min.x + small_caps_width(ui, &search) + 16.0;
    let first = x;
    for (i, (name, on)) in room.seed_ticks.iter().enumerate() {
        let name = printable(name, NAME_MAX);
        let tick_w = 24.0 + width_of(ui, &name, body(13.5));
        if x + tick_w > row.max.x && x > first {
            break;
        }
        let id = Id::new("tor-tick").with(i);
        let tick = checkbox(ui, id, pos2(x + 8.0, mid), *on, pal);
        let words_at = Rect::from_min_max(pos2(x + 22.0, row.min.y), pos2((x + tick_w).min(row.max.x).max(x + 23.0), row.max.y));
        let words = studio::hand(ui.interact(words_at, id.with("name"), Sense::click()));
        let tone = if tick.hovered() || words.hovered() {
            pal.text
        } else if *on {
            pal.ok
        } else {
            pal.dim
        };
        text(ui, pos2(x + 24.0, mid), &name, body(13.5), tone, (row.max.x - x - 24.0).max(1.0), Align2::LEFT_CENTER);
        if tick.clicked() || words.clicked() {
            out.acts.push(Act::SeedTick(i));
        }
        x += tick_w + 24.0;
    }
    if room.seed_ticks.iter().all(|(_, on)| !*on) {
        let all = t!("tor.seed_all");
        let all_w = width_of(ui, &all, body(13.0));
        if x - 8.0 + all_w <= row.max.x {
            text(ui, row.right_center(), &all, body(13.0), pal.dim, all_w + 1.0, Align2::RIGHT_CENTER);
        }
    }
}

/// A seed outcome as the webapp's chip: its word and tone, and whether an
/// LED stands for the cells' glyph before it (a word the server named
/// that the room does not know has none).
fn outcome_look(outcome: &str, pal: &Pal) -> (String, Color32, bool) {
    let (word, tone) = match outcome {
        "seeded" => (t!("tor.out_seeded"), pal.ok),
        "match_unmapped" => (t!("tor.out_match_unmapped"), pal.gold),
        "pad_files_missing" => (t!("tor.out_pad_files_missing"), pal.gold),
        "partial_match" => (t!("tor.out_partial_match"), pal.gold),
        "already_in_daemon" => (t!("tor.out_already_in_daemon"), pal.dim),
        "no_match" => (t!("tor.out_no_match"), pal.dim),
        "invalid_torrent" => (t!("tor.out_invalid_torrent"), pal.gold),
        "daemon_error" => (t!("tor.out_daemon_error"), pal.gold),
        other => return (other.to_string(), pal.dim, false),
    };
    (word.to_string(), tone, true)
}

/// One `.torrent` handed in: its file, its outcome behind its LED (in the
/// accent while it is checked), and the details sentence, cut with a tip.
fn seed_row(ui: &Ui, s: &SeedRow, client: &str, i: usize, row: &RowCtx, pal: &Pal) {
    name_cell(ui, row, 0, &printable(&s.file, 200), pal);
    let (word, tone, lit) = match (&s.outcome, &s.error) {
        (Some(o), _) => outcome_look(&o.outcome, pal),
        (None, Some(_)) => outcome_look("daemon_error", pal),
        (None, None) => (t!("tor.seed_checking").to_string(), pal.accent, true),
    };
    led_cell(ui, row, 1, lit.then_some(tone), &word, body(13.0), tone);
    let details = outcome_details(s, client);
    let quiet = s.outcome.as_ref().is_some_and(|o| matches!(o.outcome.as_str(), "no_match" | "already_in_daemon"));
    let color = ink(row, if quiet { pal.dim } else { pal.text }, pal);
    cell_tipped(ui, row, 2, &details, &details, body(13.0), color, Id::new("tor-tip-details").with(i));
}

// ── Access ────────────────────────────────────────────────────────────────

/// Access: the policy as a radio group, then the users. A user's access
/// is an LED, lit green when the whitelist lets them add torrents and dim
/// while every user may: information, not a lever, as the cells' tick is
/// (a click on it picks the row, and s flips it).
fn access(ui: &mut Ui, room: &Room, pal: &Pal, out: &mut Out) {
    label(ui, &t!("tor.policy_label"), pal);
    let whitelist = room.policy_whitelist();
    let policy = [
        (t!("tor.policy_all"), t!("tor.policy_all_desc")),
        (t!("tor.policy_whitelist"), t!("tor.policy_whitelist_desc_here")),
    ];
    for (i, (name, desc)) in policy.iter().enumerate() {
        if radio(ui, Id::new("tor-policy").with(i), (i == 1) == whitelist, name, desc, pal).clicked() {
            out.acts.push(Act::PickPolicy(i));
        }
    }
    ui.add_space(14.0);
    if ui.available_height() < TABLE_MIN {
        return;
    }
    // The name a fixed column, as the Users board's, the flags beside it.
    let cols = [
        Col::left(t!("tor.col_user"), 180.0),
        Col::left(t!("tor.col_admin"), 120.0),
        Col::left(t!("tor.col_torrents"), 0.0),
    ];
    let users: Vec<(&String, &AdminUser)> = room.users.iter().collect();
    let empty = t!("tor.empty_users");
    let table = Table::new(Id::new("tor-access"), &cols, users.len()).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| user_row(ui, users[i].0, users[i].1, whitelist, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
}

fn user_row(ui: &Ui, name: &str, user: &AdminUser, whitelist: bool, row: &RowCtx, pal: &Pal) {
    let font = if row.selected { strong(14.0) } else { body(14.0) };
    let own = if whitelist { pal.text } else { pal.dim };
    cell(ui, row, 0, &printable(name, NAME_MAX), font, ink(row, own, pal), false);
    let admin = if user.admin { t!("tor.admin_yes").to_string() } else { "—".to_string() };
    cell(ui, row, 1, &admin, body(13.0), ink(row, pal.dim, pal), false);
    let tone = if whitelist { pal.ok } else { pal.dim };
    flag_led(ui, pos2(row.cells[2].0 + 3.0, row.rect.center().y), user.allow_torrent, tone, pal);
}

// ── Client ────────────────────────────────────────────────────────────────

/// Client: the client's name and its status behind an LED, the saved
/// fields (never the password), the other saved clients, the two lines on
/// where users add torrents and what a disconnect forgets, and the test,
/// disconnect and switch buttons on the page's last row.
fn client(ui: &mut Ui, room: &Room, pal: &Pal, out: &mut Out) {
    let kind = room.kind();
    let name = kind.label();
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    let mid = row.center().y;
    let named = caps(ui, row.left_center(), &name, 13.0, pal.text, w, Align2::LEFT_CENTER);
    let (status, tone) = match &room.status {
        Some(s) if s.connected => (t!("tor.status_connected", client = name.clone(), version = version_words(s)).to_string(), pal.ok),
        Some(s) => (t!("tor.status_down", reason = printable(s.reason.as_deref().unwrap_or(""), 120)).to_string(), pal.gold),
        None => (t!("tor.status_unknown").to_string(), pal.dim),
    };
    let x = named.max.x + 20.0;
    led(ui, pos2(x + 3.0, led_mid(ui, mid, &body(13.5), false)), tone);
    text(ui, pos2(x + 14.0, mid), bare(&status), body(13.5), tone, (row.max.x - x - 14.0).max(1.0), Align2::LEFT_CENTER);
    ui.add_space(14.0);

    let cfg = room.config().cloned().unwrap_or_default();
    let mut saved = vec![(t!("tor.field_host").to_string(), cfg.host.clone()), (t!("tor.field_port").to_string(), cfg.port.to_string())];
    // The cells' saved fields: an empty one's word dim, HTTPS a yes or a no.
    let none = |s: String| if s.is_empty() { ("(none)".to_string(), false) } else { (s, true) };
    if kind.has_username() {
        saved.push((t!("tor.field_username").to_string(), cfg.username.trim().to_string()));
    }
    if kind.has_rpc_path() {
        saved.push((t!("tor.field_rpc").to_string(), cfg.rpc_path.clone().unwrap_or_default()));
    }
    let mut rows: Vec<(String, String, bool, bool)> =
        saved.into_iter().map(|(key, value)| { let (value, set) = none(value); (key, value, set, true) }).collect();
    rows.push(("HTTPS".to_string(), if cfg.use_https { "yes" } else { "no" }.to_string(), cfg.use_https, false));
    for (key, value, set, figure) in rows {
        let (row, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        let mid = row.center().y;
        small_caps(ui, row.left_center(), &key, pal.dim, KEY_W - 8.0, Align2::LEFT_CENTER);
        let at = pos2(row.min.x + KEY_W, mid);
        let font = if set && figure { mono(12.5) } else { body(13.5) };
        text(ui, at, &printable(&value, 200), font, if set { pal.text } else { pal.dim }, w - KEY_W, Align2::LEFT_CENTER);
    }
    ui.add_space(14.0);

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
    if !others.is_empty() && ui.available_height() >= 2.0 * LINE_H + FORM_BUTTON_H {
        let at = ui.cursor().min;
        small_caps(ui, pos2(at.x, at.y + 9.0), &t!("tor.also_saved"), pal.dim, KEY_W - 8.0, Align2::LEFT_CENTER);
        let words = t!("tor.also_saved_line", clients = others.join(", "));
        let drawn = Para::new(&words, body(13.5), pal.text).rows(2).paint(ui, pos2(at.x + KEY_W, at.y), w - KEY_W);
        ui.allocate_exact_size(vec2(w, drawn.height().max(18.0)), Sense::hover());
        ui.add_space(14.0);
    }
    if ui.available_height() >= 2.0 * LINE_H + 10.0 + FORM_BUTTON_H {
        line(ui, &t!("tor.mobile_note", host = phone_host(room)), body(13.0), pal.dim);
        line(ui, &t!("tor.disconnect_note"), body(13.0), pal.dim);
    }

    let rest = ui.available_rect_before_wrap();
    let by = rest.max.y - FORM_BUTTON_H;
    if by < rest.min.y {
        return;
    }
    let buttons = [
        ("tor-client-switch", t!("tor.btn_switch"), true, Act::ClientSwitch),
        ("tor-client-disconnect", t!("tor.btn_disconnect"), false, Act::Disconnect),
        ("tor-client-test", t!("tor.btn_test"), true, Act::ClientTest),
    ];
    let mut right = rest.max.x;
    for (id, label, primary, act) in buttons {
        let label = word_of(&label);
        let bw = form_button_width(ui, label);
        let at = Rect::from_min_size(pos2(right - bw, by), vec2(bw, FORM_BUTTON_H));
        if form_button(ui, Id::new(id), at, label, primary, pal).clicked() {
            out.acts.push(act);
        }
        right = at.min.x - 10.0;
    }
}

// ── The note line ─────────────────────────────────────────────────────────

/// The region's foot: what the room is busy with in the accent, else its
/// note (an error in gold), else on the tabs the cursor row's story, as
/// the cells tell it (`filtered`, the Torrents tab's rows).
fn note(ui: &Ui, rect: Rect, room: &Room, filtered: &[usize], pal: &Pal) {
    let parts = match room.tab {
        _ if room.phase() != Phase::Tabs => Vec::new(),
        Tab::Torrents => torrent_story(room, filtered, pal),
        Tab::Libraries => library_story(room, pal),
        Tab::Seeding => seed_story(room, pal),
        Tab::Access => user_story(room, pal),
        Tab::Client => client_story(room, pal),
    };
    foot_line(ui, rect, room.busy.as_deref(), room.note.as_ref(), &parts, pal);
}

/// The cursor torrent's short hash, its error, who added it and when —
/// while the rows are drawn, as in the cells: a failed list read keeps the
/// torrents and the cursor it had, but its error stands in place of the
/// rows, and no row on screen has a story to tell.
fn torrent_story(room: &Room, filtered: &[usize], pal: &Pal) -> Vec<Run> {
    if !room.list_loaded || room.list_error.is_some() {
        return Vec::new();
    }
    let Some(t) = room.sel.and_then(|s| filtered.get(s).copied()).and_then(|i| room.torrents.get(i)) else {
        return Vec::new();
    };
    let mut parts = vec![(short_id(&t.info_hash), mono(12.0), pal.dim)];
    if !t.error_message.trim().is_empty() {
        parts.push((" · ".to_string(), body(13.0), pal.dim));
        parts.push((printable(&t.error_message, 200), body(13.0), pal.gold));
    }
    if let Some(u) = t.managed_by.as_deref().filter(|_| t.managed_by_mstream) {
        parts.push((format!(" · {}", t!("tor.note_added_by", user = printable(u, NAME_MAX))), body(13.0), pal.dim));
    }
    if t.added_at > 0.0 {
        parts.push((format!(" · {}", ago_words("added", unix_now() - t.added_at as i64)), body(13.0), pal.dim));
    }
    parts
}

/// The cursor library's probe story: its last error or how it was
/// verified, whether it was set by hand, when it was last probed.
fn library_story(room: &Room, pal: &Pal) -> Vec<Run> {
    let Some(name) = room.selected_library() else { return Vec::new() };
    let mut said: Vec<(String, Color32)> = Vec::new();
    match room.access.get(name) {
        Some(a) => {
            if let Some(e) = a.last_error.as_deref().filter(|e| !e.trim().is_empty()) {
                said.push((printable(e, 200), pal.gold));
            } else if let Some(m) = a.method.as_deref().filter(|_| a.confidence == "verified" || a.confidence == "inferred") {
                said.push((t!("tor.note_via", method = printable(m, 60)).to_string(), pal.dim));
            }
            if a.source.as_deref() == Some("manual") {
                said.push((t!("tor.note_manual").to_string(), pal.dim));
            }
            if let Some(secs) = a.last_probed_at.as_ref().and_then(|v| probed_secs(v, unix_now())) {
                said.push((ago_words("probed", secs), pal.dim));
            }
        }
        None => said.push((t!("tor.path_none").to_string(), pal.dim)),
    }
    let mut parts = vec![(printable(name, NAME_MAX), strong(13.0), pal.text)];
    for (k, (words, tone)) in said.into_iter().filter(|(words, _)| !words.is_empty()).enumerate() {
        parts.push((if k == 0 { " — " } else { " · " }.to_string(), body(13.0), pal.dim));
        parts.push((words, body(13.0), tone));
    }
    parts
}

/// The cursor file and its details sentence.
fn seed_story(room: &Room, pal: &Pal) -> Vec<Run> {
    let Some(s) = room.sel.and_then(|i| room.seeds.get(i)) else { return Vec::new() };
    let details = outcome_details(s, &room.kind().label());
    vec![(printable(&s.file, NAME_MAX), strong(13.0), pal.text), (format!(" — {details}"), body(13.0), pal.text)]
}

/// Whether the cursor user may add torrents, and how to flip it.
fn user_story(room: &Room, pal: &Pal) -> Vec<Run> {
    let Some(name) = room.selected_user() else { return Vec::new() };
    let allowed = room.users.get(name).is_some_and(|u| u.allow_torrent);
    let user = printable(name, NAME_MAX);
    let words = if allowed { t!("tor.note_user_may", user = user) } else { t!("tor.note_user_may_not", user = user) };
    vec![(words.to_string(), body(13.0), pal.text), (t!("tor.note_user_tail").to_string(), body(13.0), pal.dim)]
}

/// When the daemon last answered, or why it did not.
fn client_story(room: &Room, pal: &Pal) -> Vec<Run> {
    match (&room.status, room.status_at) {
        (Some(s), Some(at)) if s.connected => vec![(ago_words("reachable", at.elapsed().as_secs() as i64), body(13.0), pal.dim)],
        (Some(s), _) => {
            let words = t!("tor.note_unreachable", reason = printable(s.reason.as_deref().unwrap_or(""), 200));
            vec![(words.to_string(), body(13.0), pal.gold)]
        }
        _ => Vec::new(),
    }
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The modal up, over the room's region: an entry for a field (the
/// mapping, the template, a seed path), or a gate (a remove, the
/// disconnect).
fn draw_modal(ui: &Ui, room: &Room, pal: &Pal, over: Rect, composition: &str, out: &mut Out) {
    match &room.modal {
        Modal::None => {}
        Modal::Mapping { vpath, path, error } => {
            let client = room.kind().label();
            let title = t!("tor.mapping_title", name = printable(vpath, NAME_MAX), client = client.clone()).to_string();
            let field = t!("tor.mapping_field", client = client.to_uppercase()).to_string();
            let buttons = [(t!("tor.mapping_save").to_string(), true, Act::ModalSubmit)];
            let shown = Entry { id: Id::new("tor-mapping"), title: &title, label: &field, input: path, error: error.as_deref(), buttons: &buttons };
            shown.show(ui, over, composition, pal, out, |ui| line(ui, &t!("tor.mapping_hint"), body(13.0), pal.dim));
        }
        Modal::Template { vpath, template, error } => {
            let vars = room.templates.supported_vars.iter().map(|v| format!("{{{{{v}}}}}")).collect::<Vec<_>>().join(" ");
            let (preview, font, tone) = match preview_template(template.value(), &room.templates.sample_metadata) {
                None => (t!("tor.template_preview_none").to_string(), body(13.5), pal.dim),
                Some(p) if p.is_empty() => (t!("tor.template_preview_empty").to_string(), body(13.5), pal.dim),
                Some(p) => (p, strong(13.5), pal.text),
            };
            let title = t!("tor.template_title", name = printable(vpath, NAME_MAX)).to_string();
            let field = t!("tor.template_field").to_string();
            let buttons = [
                (word_of(&t!("tor.template_suggested")).to_string(), false, Act::TemplateSuggest),
                (word_of(&t!("tor.template_clear")).to_string(), false, Act::TemplateClear),
                (t!("tor.template_save").to_string(), true, Act::ModalSubmit),
            ];
            let shown = Entry { id: Id::new("tor-template"), title: &title, label: &field, input: template, error: error.as_deref(), buttons: &buttons };
            shown.show(ui, over, composition, pal, out, |ui| {
                let (vars_key, preview_key) = (t!("tor.template_vars"), t!("tor.template_preview"));
                let key_w = small_caps_width(ui, &vars_key).max(small_caps_width(ui, &preview_key)) + 16.0;
                keyed(ui, &vars_key, key_w, &vars, mono(12.5), pal.text, pal);
                line(ui, &t!("tor.template_vars_hint"), body(13.0), pal.dim);
                ui.add_space(10.0);
                keyed(ui, &preview_key, key_w, &preview, font, tone, pal);
                line(ui, &t!("tor.template_preview_hint"), body(13.0), pal.dim);
            });
        }
        Modal::SeedPath { path, matches, error, asked } => {
            let title = t!("tor.seed_path_title").to_string();
            let field = t!("tor.seed_path_field").to_string();
            let buttons = [(t!("tor.seed_path_button").to_string(), true, Act::ModalSubmit)];
            let shown = Entry { id: Id::new("tor-seed-path"), title: &title, label: &field, input: path, error: error.as_deref(), buttons: &buttons };
            shown.show(ui, over, composition, pal, out, |ui| {
                line(ui, &t!("tor.seed_path_hint"), body(13.0), pal.dim);
                if asked.is_some() && matches.is_empty() {
                    candidate(ui, &t!("busy.listing"), false, pal);
                }
                for m in matches {
                    candidate(ui, m, true, pal);
                }
            });
        }
        Modal::Remove(hash) => {
            let name = room.torrent(hash).map(|t| printable(&t.name, NAME_MAX)).unwrap_or_else(|| short_id(hash));
            let title = t!("tor.remove_title", name = name, client = room.kind().label()).to_string();
            let lines = [t!("tor.remove_1").to_string(), t!("tor.remove_2").to_string()];
            let safe = (t!("tor.remove_keep").to_string(), Act::RemoveCancel);
            let go = (t!("tor.remove_confirm").to_string(), Act::RemoveConfirm);
            warn(ui, Id::new("tor-remove"), over, &title, &lines, safe, go, pal, out);
        }
        Modal::Disconnect => {
            let title = t!("tor.disconnect_title", client = room.kind().label()).to_string();
            let lines = [t!("tor.disconnect_1").to_string(), t!("tor.disconnect_2").to_string()];
            let safe = (t!("tor.disconnect_keep").to_string(), Act::DisconnectCancel);
            let go = (t!("tor.disconnect_confirm").to_string(), Act::DisconnectConfirm);
            warn(ui, Id::new("tor-disconnect"), over, &title, &lines, safe, go, pal, out);
        }
    }
}

/// A key in dim small capitals in a column `key_w` wide and its value
/// after it, a row of the layout (the template's variables and preview).
fn keyed(ui: &mut Ui, key: &str, key_w: f32, value: &str, font: FontId, tone: Color32, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, LINE_H), Sense::hover());
    small_caps(ui, row.left_center(), key, pal.dim, key_w, Align2::LEFT_CENTER);
    text(ui, pos2(row.min.x + key_w, row.center().y), value, font, tone, (w - key_w).max(1.0), Align2::LEFT_CENTER);
}

/// One line under the seed path's field, indented: a completion behind a
/// small pointer, or the listing still on its way.
fn candidate(ui: &mut Ui, words: &str, pointed: bool, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, LINE_H), Sense::hover());
    let mid = row.center().y;
    if pointed {
        pointer(ui, pos2(row.min.x + 20.0, mid), pal.dim);
    }
    text(ui, pos2(row.min.x + 32.0, mid), words, mono(12.5), pal.dim, (w - 32.0).max(1.0), Align2::LEFT_CENTER);
}

/// An entry modal: one text field, the lines under it, its error, its
/// buttons at the foot's right in their order.
struct Entry<'a> {
    id: Id,
    title: &'a str,
    label: &'a str,
    input: &'a Input,
    error: Option<&'a str>,
    buttons: &'a [(String, bool, Act)],
}

impl Entry<'_> {
    /// The modal over `over`, its field always with the keyboard (as the
    /// cells draw it), `lines` laid out under the field. The close is the
    /// cells' cancel and a click on the field their submit (the cells
    /// register its box as the modal's Enter); the caret is the field's.
    fn show(self, ui: &Ui, over: Rect, composition: &str, pal: &Pal, out: &mut Out, lines: impl FnOnce(&mut Ui)) {
        let Entry { id, title, label, input, error, buttons } = self;
        let shown = Dialog::new(id, over, ENTRY_W, title).show(ui, pal, |ui| {
            // A path, a template of one: the figures' monospace, as the
            // canvas sets a path's field and this room its paths.
            let drawn = Field::of(input).focused(true, composition).mono();
            let typed = studio::field(ui, id.with("field"), label, &drawn, pal);
            ui.add_space(14.0);
            lines(ui);
            if let Some(e) = error {
                ui.add_space(6.0);
                error_row(ui, e, pal);
            }
            ui.add_space(20.0);
            let items: Vec<(&str, bool)> = buttons.iter().map(|(label, primary, _)| (label.as_str(), *primary)).collect();
            let clicked = foot(ui, id.with("foot"), &[], &items, None, pal).map(|k| buttons[k].2.clone());
            (typed, clicked)
        });
        let (typed, clicked) = shown.inner;
        if typed.response.clicked() {
            out.acts.push(Act::ModalSubmit);
        }
        out.acts.extend(clicked);
        if shown.closed {
            out.acts.push(Act::ModalCancel);
        }
        out.caret = typed.caret;
    }
}

/// A gate, the kit's (its title wrapped where a torrent's long name runs
/// on): its lines in gold, each its own paragraph, then the safe choice
/// lit and the one that goes through beside it.
#[allow(clippy::too_many_arguments)]
fn warn(ui: &Ui, id: Id, over: Rect, title: &str, lines: &[String], safe: (String, Act), go: (String, Act), pal: &Pal, out: &mut Out) {
    let sentences = lines.iter().map(|words| Para::new(words, body(14.0), pal.gold)).collect();
    match gate(ui, id, over, title, sentences, [&safe.0, &go.0], pal) {
        Some(true) => out.acts.push(go.1),
        Some(false) => out.acts.push(safe.1),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use egui::Vec2;
    use egui::epaint::{Shape, TextShape};

    use super::super::tests::{connected, english, loaded, new_room, params, type_text, users};
    use super::*;

    /// A roomy region of the panel, in points.
    const SIZE: Vec2 = vec2(1100.0, 660.0);

    /// A context styled for the desk before the room's first pass, as its
    /// host styles it: a style set in a pass lays out the passes after it,
    /// and a control's rect read after a run can be the run before's.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = studio::begin(ui);
        });
        out.textures_delta.clear();
        ctx
    }

    /// One pass of the room over the region with `events`, the input
    /// method composing `composition`: what egui drew and what the room
    /// told its host.
    fn pass_with(ctx: &egui::Context, room: &mut Room, events: Vec<egui::Event>, composition: &str) -> (egui::FullOutput, EguiShown) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), events, ..Default::default() };
        let mut shown = None;
        let mut out = ctx.run_ui(input, |ui| shown = Some(show(ui, room, composition)));
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        (out, shown.expect("the pass ran"))
    }

    /// Two quiet passes, as a frame settles (a modal takes its body's
    /// height on the first).
    fn settle(ctx: &egui::Context, room: &mut Room) -> (egui::FullOutput, EguiShown) {
        let _ = pass_with(ctx, room, Vec::new(), "");
        pass_with(ctx, room, Vec::new(), "")
    }

    /// A press and a release at `at`, a pass each after the move, as hands
    /// would make them: the outcome any of them returned.
    fn click(ctx: &egui::Context, room: &mut Room, at: Pos2) -> Option<Outcome> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut outcome = None;
        for events in [vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
            outcome = outcome.or(pass_with(ctx, room, events, "").1.outcome);
        }
        outcome
    }

    /// A click on the control `id` the last pass drew, settled after.
    fn click_on(ctx: &egui::Context, room: &mut Room, id: Id) {
        let at = ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect.center();
        assert_eq!(click(ctx, room, at), None, "no click ends the room");
        let _ = settle(ctx, room);
    }

    /// Every text the pass drew.
    fn texts(out: &egui::FullOutput) -> Vec<&TextShape> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    fn words(out: &egui::FullOutput) -> Vec<String> {
        texts(out).iter().map(|t| t.galley.text().to_string()).collect()
    }

    fn assert_drawn(out: &egui::FullOutput, wanted: &[&str]) {
        let drawn = words(out);
        for word in wanted {
            assert!(drawn.iter().any(|d| d == word), "{word:?} drawn: {drawn:?}");
        }
    }

    /// Where the text `s` was drawn.
    fn where_drawn(out: &egui::FullOutput, s: &str) -> Rect {
        texts(out).iter().find(|t| t.galley.text() == s).map(|t| t.visual_bounding_rect()).unwrap_or_else(|| panic!("{s:?} drawn"))
    }

    #[test]
    fn every_tab_draws_in_the_rooms_words() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "connected — Transmission 4.0.6 · 127.0.0.1:9091 · 2 of 4 libraries reachable",
                "polls every 5 s",
                "BETA",
                "TORRENTS · 4",
                "LIBRARIES",
                "SEEDING",
                "ACCESS",
                "CLIENT",
                "the daemon's list",
                "filter by name or info-hash",
                "4 torrents",
                "NAME",
                "STATUS",
                "PROGRESS",
                "DOWN",
                "SIZE",
                "ADDED BY",
                "Boards of Canada - Music Has the Right to Children (1998) [FLAC]",
                "downloading",
                "63%",
                "2.4 MB/s",
                "612.0 MB",
                "iros",
                "linux-6.9.iso",
                "seeding",
                "100%",
                "—",
                "external",
                "error",
            ],
        );

        room.act(Act::Tab(Tab::Libraries));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "polls every 30 s",
                "paths and templates per library",
                "The paths are as Transmission sees them, not as mStream does — in Docker they usually differ.",
                "The daemon confirmed the verified rows; an unconfirmed library refuses torrents until mapped.",
                "LIBRARY",
                "PATH AS SEEN BY TRANSMISSION",
                "TEMPLATE",
                "audiobooks",
                "vinyl",
                "verified",
                "probing…",
                "unconfirmed",
                "/downloads/music",
                "asking the daemon…",
                "no mapping yet",
                "{{ARTIST}}/{{ALBUM}} ({{YEAR}})",
                "(none — typed by hand)",
            ],
        );

        room.act(Act::Tab(Tab::Seeding));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                ".torrents for content on disk",
                "Hand mStream a .torrent whose content is already on disk. Its files are checked against your",
                "libraries and, when all match, the daemon gets the torrent and seeds without downloading a byte.",
                "Add a .torrent file",
                "SEARCH IN",
                "music",
                "podcasts",
                "none ticked = every library",
                "FILE",
                "OUTCOME",
                "DETAILS",
                "(no files checked yet)",
            ],
        );

        room.act(Act::Tab(Tab::Access));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "who may add torrents",
                "WHO CAN ADD TORRENTS",
                "all users",
                "every signed-in user can add torrents",
                "whitelist",
                "only the users ticked below",
                "USER",
                "ADMIN",
                "TORRENTS",
                "dj-tom",
                "guest",
                "mira",
                "admin",
            ],
        );

        if let Some(p) = room.params.as_mut() {
            p.qbittorrent = TorrentClientConfig { host: "192.168.1.20".into(), port: 8080, configured: true, ..Default::default() };
        }
        room.act(Act::Tab(Tab::Client));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "the daemon mStream talks to",
                "TRANSMISSION",
                "connected — Transmission 4.0.6",
                "HOST",
                "PORT",
                "USERNAME",
                "RPC PATH",
                "127.0.0.1",
                "9091",
                "/transmission/rpc",
                "HTTPS",
                "no",
                "ALSO SAVED",
                "qBittorrent 192.168.1.20:8080 — c switches without retyping the password",
                "Users add torrents from a phone at home.mstream.example:3000/torrent — the apps don't expose it.",
                "Disconnecting forgets the credentials here; the daemon keeps every torrent it has.",
                "TEST",
                "DISCONNECT",
                "SWITCH CLIENT",
            ],
        );
    }

    #[test]
    fn a_long_daemon_path_is_cut_at_its_start_so_its_leaf_stays() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        let long = format!("/srv/{}/music/Boards of Canada", "deep".repeat(60));
        if let Some(row) = room.access.get_mut("music") {
            row.daemon_path = Some(long.clone());
        }
        room.act(Act::Tab(Tab::Libraries));
        let (out, _) = settle(&ctx, &mut room);
        let drawn = words(&out);
        let cut = drawn.iter().find(|w| w.starts_with('…') && w.ends_with("/music/Boards of Canada")).unwrap_or_else(|| panic!("the leaf drawn: {drawn:?}"));
        assert!(cut.chars().count() < long.chars().count(), "cut: {cut}");
        assert!(!drawn.iter().any(|w| w == &long), "never drawn whole");
    }

    #[test]
    fn the_note_line_says_what_the_cells_say() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        room.act(Act::Select(3));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["e5f6e5f6e5f6… · Tracker gave HTTP response code 403 (Forbidden) · added by iros · 2d ago"]);
        room.busy = Some(t!("tor.busy_loading").to_string());
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["loading…"]);
        assert!(!words(&out).iter().any(|w| w.starts_with("e5f6e5f6e5f6…")), "the busy line wins");
        room.busy = None;
        room.note = Some(("could not reach server".into(), true));
        assert_drawn(&settle(&ctx, &mut room).0, &["could not reach server"]);

        room.act(Act::Tab(Tab::Libraries));
        room.act(Act::Select(3));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["vinyl — daemon free-space returned -1 (path not visible to daemon) · probed 3m ago"]);
        room.act(Act::Tab(Tab::Access));
        room.act(Act::Select(1));
        assert_drawn(&settle(&ctx, &mut room).0, &["guest may not add torrents · s flips it, at once"]);
    }

    #[test]
    fn a_failed_list_read_stands_in_place_of_the_rows_and_tells_no_rows_story() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        room.act(Act::Select(3));
        assert_drawn(&settle(&ctx, &mut room).0, &["Radiohead - In Rainbows [MP3 V0]", "e5f6e5f6e5f6… · Tracker gave HTTP response code 403 (Forbidden) · added by iros · 2d ago"]);
        // What a failed poll leaves (`Done::Listed`): the list and the
        // cursor it had, and its error.
        room.list_error = Some("could not reach server".into());
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["couldn't fetch torrents: could not reach server"]);
        let drawn = words(&out);
        assert!(!drawn.iter().any(|w| w == "Radiohead - In Rainbows [MP3 V0]"), "the error stands in place of the rows: {drawn:?}");
        assert!(!drawn.iter().any(|w| w.starts_with("e5f6e5f6e5f6…")), "no row's story rides the note line: {drawn:?}");
        assert!(!drawn.iter().any(|w| w == "4 torrents"), "nor is the list counted: {drawn:?}");
    }

    #[test]
    fn the_client_page_and_the_connect_form_draw_their_words() {
        let _en = english();
        let ctx = context();
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("disabled", false), users(4), false));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "off — no torrent client chosen",
                "Hand off magnet links and .torrent files to a torrent client on this host or on the LAN.",
                "Completed downloads are picked up by the next library scan.",
                "Users add torrents from a phone at home.mstream.example:3000/torrent — the apps don't expose it.",
                "CLIENT",
                "Disabled",
                "nobody can add torrents",
                "Transmission",
                "RPC on port 9091, usually at /transmission/rpc",
                "qBittorrent",
                "Deluge",
                "WHO CAN ADD TORRENTS",
                "all users",
                "only the users you tick on the Access tab",
                "beta — the torrent feature is new; report issues at github.com/IrosTheBeggar/mStream/issues",
            ],
        );
        assert!(!words(&out).iter().any(|w| w.ends_with('▸')), "nothing to apply while off stays off");
        room.act(Act::PickClient(1));
        assert_drawn(&settle(&ctx, &mut room).0, &["Connect to Transmission"]);

        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "transmission — no credentials yet · Test saves nothing; Connect saves after a good probe",
                "HOST",
                "PORT",
                "USERNAME",
                "PASSWORD",
                "RPC PATH",
                "use HTTPS",
                "9091",
                "/transmission/rpc",
                "TEST",
                "CONNECT",
            ],
        );
        room.act(Act::ConnectSubmit);
        assert_drawn(&settle(&ctx, &mut room).0, &["a host is needed"]);
        type_text(&mut room, "127.0.0.1");
        if let Some(c) = room.connect.as_mut() {
            c.password = Input::new("s3cret".into());
        }
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["127.0.0.1", "••••••"]);
        assert!(!words(&out).iter().any(|w| w.contains("s3cret")), "the password is masked");
        room.apply(Done::Probed {
            kind: Kind::Transmission,
            connect: false,
            result: Ok(ProbeAnswer { ok: true, version: Some("4.0.6".into()), rpc_version: Some(serde_json::json!(17)), ..Default::default() }),
        });
        assert_drawn(&settle(&ctx, &mut room).0, &["reachable — Transmission 4.0.6 · RPC 17 · nothing saved yet"]);

        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("qbittorrent", false), users(1), false));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["USERNAME", "mStream sends no Referer — turn off qBittorrent's CSRF protection, or bypass auth for localhost"]);
        assert!(!words(&out).iter().any(|w| w == "RPC PATH"));
    }

    #[test]
    fn a_click_on_the_tabs_page_applies_the_act_the_cells_click_applies() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-tabs").with(1));
        assert_eq!(room.tab, Tab::Libraries);
        click_on(&ctx, &mut room, Id::new("tor-tabs").with(0));
        assert_eq!(room.tab, Tab::Torrents);
        let (out, _) = settle(&ctx, &mut room);
        assert_eq!(click(&ctx, &mut room, where_drawn(&out, "linux-6.9.iso").center()), None);
        assert_eq!(room.sel, Some(2), "a click on a row selects it");
        click_on(&ctx, &mut room, Id::new("tor-filter"));
        assert!(room.filter_focus, "a click on the filter gives it the caret");
        room.filter_focus = false;

        room.act(Act::Tab(Tab::Seeding));
        let (out, _) = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-tick").with(1));
        assert_eq!(room.seed_ticks[1], ("music".to_string(), true), "a click on a tick flips it");
        assert_eq!(click(&ctx, &mut room, where_drawn(&out, "vinyl").center()), None);
        assert_eq!(room.seed_ticks[3], ("vinyl".to_string(), true), "and so does a click on its name");
        click_on(&ctx, &mut room, Id::new("tor-seed-card"));
        assert!(matches!(room.modal, Modal::SeedPath { .. }), "the card adds a file: the path modal off this machine");
        room.modal = Modal::None;

        room.act(Act::Tab(Tab::Access));
        let (out, _) = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-policy").with(1));
        assert_eq!(room.queued, Some(Op::SetPolicy("whitelist".into())));
        room.queued = None;
        room.busy = None;
        assert_eq!(click(&ctx, &mut room, where_drawn(&out, "guest").center()), None);
        assert_eq!(room.sel, Some(1));
        assert!(room.queued.is_none(), "a click on a user selects it; s flips the access");

        room.act(Act::Tab(Tab::Client));
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-client-test"));
        assert_eq!(room.queued, Some(Op::Status));
        room.queued = None;
        room.busy = None;
        click_on(&ctx, &mut room, Id::new("tor-client-disconnect"));
        assert!(matches!(room.modal, Modal::Disconnect));
        room.modal = Modal::None;
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-client-switch"));
        assert!(room.choosing && room.phase() == Phase::Choose, "switch opens the client page");
    }

    #[test]
    fn a_click_on_the_client_page_and_the_form_applies_the_act_the_cells_click_applies() {
        let _en = english();
        let ctx = context();
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("disabled", false), users(4), false));
        room.note = None;
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-client").with(1));
        assert_eq!(room.client_pick, 1);
        click_on(&ctx, &mut room, Id::new("tor-card"));
        assert_eq!(room.queued, Some(Op::SetClient(Kind::Transmission)));
        room.queued = None;
        room.busy = None;
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-policy").with(1));
        assert_eq!(room.queued, Some(Op::SetPolicy("whitelist".into())));

        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-connect-field").with(1));
        assert_eq!(room.connect.as_ref().map(Connect::focused), Some(CField::Port));
        click_on(&ctx, &mut room, Id::new("tor-https"));
        assert!(room.connect.as_ref().is_some_and(|c| c.https), "a click flips HTTPS");
        click_on(&ctx, &mut room, Id::new("tor-connect-field").with(0));
        type_text(&mut room, "127.0.0.1");
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, Id::new("tor-test"));
        assert!(matches!(&room.queued, Some(Op::Probe { connect: false, creds, .. }) if creds.host == "127.0.0.1" && creds.use_https));
        room.queued = None;
        room.busy = None;
        click_on(&ctx, &mut room, Id::new("tor-connect"));
        assert!(matches!(&room.queued, Some(Op::Probe { connect: true, .. })));
    }

    #[test]
    fn each_gate_draws_has_no_close_and_its_buttons_and_esc_do_what_the_cells_do() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        let hash = "5f4a".repeat(10);
        let remove = Id::new("tor-remove");
        room.act(Act::Remove(hash.clone()));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "Remove Boards of Canada - Music Has the Right to Children (1998) [FLAC] from Transmission?",
                "The files on disk are KEPT — only the daemon's record of the torrent goes.",
                "Use the daemon's own UI to delete the files.",
                "KEEP",
                "REMOVE",
            ],
        );
        click_on(&ctx, &mut room, remove.with("foot").with(0));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "keep is the safe choice");
        room.act(Act::Remove(hash.clone()));
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(remove.with("close")).is_none(), "a gate has no close, as the cells draw it");
        room.key(KeyEvent::new(KeyCode::Esc, crate::input::KeyModifiers::NONE));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Esc keeps");
        room.act(Act::Remove(hash.clone()));
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, remove.with("foot").with(1));
        assert_eq!(room.queued, Some(Op::Remove(hash)));
        room.queued = None;
        room.busy = None;

        let disconnect = Id::new("tor-disconnect");
        room.act(Act::Tab(Tab::Client));
        room.act(Act::Disconnect);
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "Disconnect from Transmission?",
                "mStream forgets the saved credentials; the daemon keeps every torrent and its files.",
                "Torrents added through mStream cannot be removed from here until you reconnect.",
                "STAY",
            ],
        );
        room.key(KeyEvent::new(KeyCode::Esc, crate::input::KeyModifiers::NONE));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Esc stays");
        room.act(Act::Disconnect);
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, disconnect.with("foot").with(0));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "stay");
        room.act(Act::Disconnect);
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, disconnect.with("foot").with(1));
        assert_eq!(room.queued, Some(Op::Disconnect(Kind::Transmission)));
    }

    #[test]
    fn each_entry_draws_its_field_and_its_buttons_and_close_do_what_the_cells_do() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        room.act(Act::Tab(Tab::Libraries));
        let mapping = Id::new("tor-mapping");
        room.act(Act::Mapping("vinyl".into()));
        let (out, shown) = settle(&ctx, &mut room);
        assert_drawn(&out, &["Map vinyl for Transmission", "the daemon is asked to see a marker mStream writes there", "VERIFY AND SAVE"]);
        assert!(shown.caret.is_some(), "the entry's field has the keyboard");
        // The cells register the box as the modal's Enter.
        click_on(&ctx, &mut room, mapping.with("field"));
        assert_drawn(&settle(&ctx, &mut room).0, &["a path to a path is needed"]);
        click_on(&ctx, &mut room, mapping.with("close"));
        assert!(matches!(room.modal, Modal::None));
        room.act(Act::Mapping("music".into()));
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, mapping.with("foot").with(0));
        assert_eq!(room.queued, Some(Op::Map { vpath: "music".into(), path: "/downloads/music".into() }));
        room.queued = None;
        room.busy = None;
        room.modal = Modal::None;

        let template = Id::new("tor-template");
        room.act(Act::Template("music".into()));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "Path template — music",
                "VARIABLES",
                "{{ARTIST}} {{ALBUM}} {{YEAR}} {{GENRE}} {{ALBUMARTIST}}",
                "empty variables drop their segment · slashes in a value become -",
                "PREVIEW",
                "Pink Floyd/The Dark Side of the Moon (1973)",
                "resolved from the sample metadata; the server re-checks on save",
                "SUGGESTED TEMPLATE",
                "CLEAR",
                "SAVE",
            ],
        );
        click_on(&ctx, &mut room, template.with("foot").with(1));
        assert!(matches!(&room.modal, Modal::Template { template, .. } if template.value().is_empty()), "clear");
        assert_drawn(&settle(&ctx, &mut room).0, &["(no template — the user types the path)"]);
        click_on(&ctx, &mut room, template.with("foot").with(0));
        let suggested = room.templates.suggested_template.clone();
        assert!(matches!(&room.modal, Modal::Template { template, .. } if template.value() == suggested), "suggested");
        click_on(&ctx, &mut room, template.with("foot").with(2));
        assert_eq!(room.queued, Some(Op::Template { vpath: "music".into(), template: Some(suggested) }));
        room.queued = None;
        room.busy = None;
        room.modal = Modal::None;

        let seed = Id::new("tor-seed-path");
        room.act(Act::SeedAdd);
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["Add a .torrent file", ".TORRENT FILE ON THIS MACHINE", "Tab completes · the file is read here and sent to the server", "CHECK AND SEED"]);
        click_on(&ctx, &mut room, seed.with("foot").with(0));
        assert_drawn(&settle(&ctx, &mut room).0, &["a path to a .torrent file is needed"]);
        if let Modal::SeedPath { asked, .. } = &mut room.modal {
            *asked = Some("/nowhere/".into());
        }
        assert_drawn(&settle(&ctx, &mut room).0, &["listing…"]);
        if let Modal::SeedPath { matches, asked, .. } = &mut room.modal {
            *asked = None;
            *matches = vec!["albums/".into(), "boc.torrent".into()];
        }
        assert_drawn(&settle(&ctx, &mut room).0, &["albums/", "boc.torrent"]);
        click_on(&ctx, &mut room, seed.with("close"));
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn the_room_beneath_a_modal_takes_no_click() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        room.act(Act::Disconnect);
        let _ = settle(&ctx, &mut room);
        let tab = ctx.read_response(Id::new("tor-tabs").with(1)).expect("the strip drew under the scrim").rect.center();
        let _ = click(&ctx, &mut room, tab);
        assert_eq!(room.tab, Tab::Torrents, "the tab under the scrim did not open");
        assert!(matches!(room.modal, Modal::Disconnect));
    }

    #[test]
    fn a_focused_field_says_where_its_caret_is_and_none_without_one() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        assert_eq!(settle(&ctx, &mut room).1.caret, None, "no field has the keyboard");
        let (out, _) = pass_with(&ctx, &mut room, Vec::new(), "にほ");
        assert!(!words(&out).iter().any(|w| w.contains("にほ")), "a composition goes to the focused field only");

        room.act(Act::FilterFocus);
        let caret = settle(&ctx, &mut room).1.caret.expect("the filter has the keyboard");
        let filter = ctx.read_response(Id::new("tor-filter")).expect("the filter was drawn").rect;
        assert!(filter.x_range().contains(caret.x) && caret.y > filter.center().y && caret.y <= filter.max.y, "{caret:?} in {filter:?}");
        let _ = pass_with(&ctx, &mut room, Vec::new(), "にほ");
        let (out, shown) = pass_with(&ctx, &mut room, Vec::new(), "にほ");
        assert!(words(&out).iter().any(|w| w == "にほ"), "the composition is drawn in the filter");
        assert!(shown.caret.is_some_and(|at| at.x > caret.x + 8.0), "the caret stands after it");

        // A modal's field has the keyboard over the filter's.
        room.act(Act::Tab(Tab::Libraries));
        room.act(Act::Template("music".into()));
        let caret = settle(&ctx, &mut room).1.caret.expect("the entry's field has the keyboard");
        let field = ctx.read_response(Id::new("tor-template").with("field")).expect("the field was drawn").rect;
        assert!(field.contains(caret - vec2(0.0, 0.5)), "{caret:?} in {field:?}");

        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        let caret = settle(&ctx, &mut room).1.caret.expect("the host has the keyboard");
        let host = ctx.read_response(Id::new("tor-connect-field").with(0)).expect("the host was drawn").rect;
        assert!(host.contains(caret - vec2(0.0, 0.5)), "{caret:?} in {host:?}");
        if let Some(c) = room.connect.as_mut() {
            c.focus = c.fields().len() - 1;
        }
        assert_eq!(settle(&ctx, &mut room).1.caret, None, "the HTTPS tick has the keys: no caret");
    }
}
