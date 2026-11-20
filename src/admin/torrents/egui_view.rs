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

use egui::epaint::{Shape, StrokeKind};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align, Align2, Color32, CursorIcon, FontId, Id, Layout, Pos2, Rect, Response, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};

use super::*;
use crate::admin::{EguiShown, fmt_bytes, host_of};
use crate::gui::window::studio::{
    self, Col, FIELD_H, FORM_BUTTON_H, HIT, Input as Field, Pal, RowCtx, SEG_H, Seg, Table, Typed, add_card, begin, body, caps,
    cell, check_line, checkbox, chip, field_box, form_button, form_button_width, ink, led, meter, meter_lit, modal, mono,
    note_line, radio, ring, segmented, small_caps, small_caps_width, square, strong, text, width_of,
};

/// The room's margin inside its region, in points: the canvas's sides and
/// foot (above, the host's first row stands in for most of its top).
const PAD: Vec2 = vec2(24.0, 14.0);
/// The room between the page's rows, and between the page and its note
/// line: the canvas's.
const GAP: f32 = 18.0;
/// The note line along the region's foot: the canvas's.
const NOTE_H: f32 = 18.0;
/// A line of prose.
const LINE_H: f32 = 20.0;
/// A table's headings and one row: with less room than this a table is
/// not drawn, as the cells leave off a table with under three rows.
const TABLE_MIN: f32 = 74.0;
/// How far under a table's top its first row's middle stands: the
/// headings' 30 points, the 4 under them, half a 40-point row.
const FIRST_ROW: f32 = 54.0;
/// The modals' widths: an entry's, a gate's.
const ENTRY_W: f32 = 600.0;
const GATE_W: f32 = 560.0;
/// The connect form's width, its wide fields' and the port's, the room
/// between its rows, and the few points more over its first row and over
/// its verdict (the canvas's).
const FORM_W: f32 = 632.0;
const WIDE_W: f32 = 480.0;
const PORT_W: f32 = 128.0;
const FORM_GAP: f32 = 20.0;
const FORM_NUDGE: f32 = 4.0;
/// A labelled field's height: its label, the room under it, its box.
const SLOT_H: f32 = 20.0 + FIELD_H;
/// The key column of the Client tab's saved fields.
const KEY_W: f32 = 120.0;

/// One run of a line of words: its text, its face, its tone.
type Part = (String, FontId, Color32);

/// What a pass gathers for the room: the acts its clicks asked for, in
/// order; the keys a control stands in for (a gate's close is its Esc);
/// and the caret of the field that has the keyboard.
#[derive(Default)]
struct Out {
    acts: Vec<Act>,
    keys: Vec<KeyEvent>,
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
    let pal = begin(ui);
    let full = ui.max_rect();
    let column = full.shrink2(PAD);
    let note_rect = Rect::from_min_max(pos2(column.min.x, column.max.y - NOTE_H), column.max);
    let page_rect = Rect::from_min_max(column.min, pos2(column.max.x, note_rect.min.y - GAP));
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
    foot(ui, note_rect, room, &filtered, &pal);
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
    for key in out.keys {
        outcome = outcome.or(room.key(key));
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
/// it, the tone of the LED and the head, and the poll note.
struct StateLine {
    head: String,
    bold: bool,
    detail: String,
    tone: Color32,
    polls: Option<String>,
}

/// The webapp's status card in one row, in the words the cells' state
/// line uses.
fn state_line(room: &Room, pal: &Pal) -> StateLine {
    let kind = room.kind();
    if kind == Kind::Disabled {
        let head = unbullet(&t!("tor.state_off"));
        return StateLine { head, bold: false, detail: t!("tor.state_off_detail").to_string(), tone: pal.dim, polls: None };
    }
    if !room.config().is_some_and(|c| c.configured) {
        return StateLine {
            head: unbullet(&t!("tor.state_picked", client = kind.as_str())),
            bold: true,
            detail: t!("tor.state_picked_detail").to_string(),
            tone: pal.gold,
            polls: None,
        };
    }
    let polls = if room.tab == Tab::Torrents && room.list_live() { t!("tor.polls") } else { t!("tor.polls_slow") };
    let (head, detail, tone) = match &room.status {
        None => (
            t!("tor.state_asking", client = kind.as_str()).to_string(),
            t!("tor.state_asking_detail").to_string(),
            pal.dim,
        ),
        Some(s) if s.connected => {
            let host = room.config().map(|c| format!("{}:{}", c.host, c.port)).unwrap_or_default();
            let detail = t!(
                "tor.state_connected_detail",
                client = kind.label(),
                version = version_words(s),
                host = printable(&host, 80),
                reachable = room.reachable(),
                total = room.libraries.len()
            );
            (t!("tor.state_connected").to_string(), detail.to_string(), pal.ok)
        }
        Some(s) => (
            t!("tor.state_down").to_string(),
            t!("tor.state_down_detail", reason = printable(s.reason.as_deref().unwrap_or(""), 120)).to_string(),
            pal.gold,
        ),
    };
    StateLine { head: unbullet(&head), bold: true, detail, tone, polls: Some(polls.to_string()) }
}

/// `s` without the cells' leading bullet: the LED drawn before it is the
/// mark.
fn unbullet(s: &str) -> String {
    s.trim_start_matches(['•', '●']).trim_start().to_string()
}

/// The page's first row: the state line on the left behind its LED, the
/// poll note right of it when both fit, and the gold BETA chip flush right
/// — the chip alone while the room loads, and on a connect page with no
/// form yet.
fn state_row(ui: &mut Ui, room: &Room, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    let mid = row.center().y;
    let beta = chip(ui, row.right_center(), Align2::RIGHT_CENTER, &t!("tor.beta"), pal.gold);
    let paged = match room.phase() {
        Phase::Loading => false,
        Phase::Connect => room.connect.is_some(),
        Phase::Choose | Phase::Tabs => true,
    };
    if !paged {
        return;
    }
    let state = state_line(room, pal);
    let head_font = if state.bold { strong(13.5) } else { body(13.5) };
    let mut right = beta.min.x - 16.0;
    if let Some(polls) = &state.polls {
        let wanted = 14.0 + width_of(ui, &state.head, head_font.clone()) + width_of(ui, &state.detail, body(13.5));
        let polls_w = width_of(ui, polls, body(12.5));
        if row.min.x + wanted + 16.0 + polls_w <= right {
            text(ui, pos2(right, mid), polls, body(12.5), pal.dim, polls_w + 1.0, Align2::RIGHT_CENTER);
            right -= polls_w + 16.0;
        }
    }
    led(ui, pos2(row.min.x + 3.0, mid), state.tone);
    let x = row.min.x + 14.0;
    let head = text(ui, pos2(x, mid), &state.head, head_font, state.tone, (right - x).max(1.0), Align2::LEFT_CENTER);
    text(ui, pos2(head.max.x, mid), &state.detail, body(13.5), pal.soft, (right - head.max.x).max(1.0), Align2::LEFT_CENTER);
}

// ── Painting helpers ──────────────────────────────────────────────────────

/// One line of words, a row of the layout, cut where it runs long.
fn line(ui: &mut Ui, s: &str, font: FontId, color: Color32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, LINE_H), Sense::hover());
    text(ui, rect.left_center(), s, font, color, w, Align2::LEFT_CENTER);
}

/// A group's label in dim small capitals, a row of the layout, and the
/// room under it.
fn label(ui: &mut Ui, s: &str, pal: &Pal) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    small_caps(ui, rect.left_center(), s, pal.dim, w, Align2::LEFT_CENTER);
    ui.add_space(6.0);
}

/// `s` wrapped to `width` from `at`, its top left, in at most `rows`
/// lines, the last cut with an ellipsis. Returns where it was drawn.
#[allow(clippy::too_many_arguments)]
fn wrapped(ui: &Ui, at: Pos2, width: f32, s: &str, font: FontId, color: Color32, rows: usize) -> Rect {
    let mut job = LayoutJob::single_section(s.to_string(), TextFormat { font_id: font, color, ..Default::default() });
    job.wrap = TextWrapping { max_width: width.max(1.0), max_rows: rows, ..Default::default() };
    let galley = ui.painter().layout_job(job);
    let rect = Rect::from_min_size(at, galley.size());
    ui.painter().galley(at, galley, color);
    rect
}

/// The runs of a line laid end to end across `rect`, each in its own face
/// and tone, cut where the line runs out.
fn run(ui: &Ui, rect: Rect, parts: &[Part]) {
    let mid = rect.center().y;
    let mut x = rect.min.x;
    for (words, font, tone) in parts {
        if x >= rect.max.x - 1.0 {
            break;
        }
        x = text(ui, pos2(x, mid), words, font.clone(), *tone, rect.max.x - x, Align2::LEFT_CENTER).max.x;
    }
}

/// The cells' tick (`ok`) or dash, drawn as a mark in `tone` centred on
/// `at`: the canvas's check, or a short rule.
fn mark(ui: &Ui, at: Pos2, ok: bool, tone: Color32) {
    let stroke = Stroke::new(1.7, tone);
    if ok {
        // The canvas's check in its 10-point box: M1.5 5.2 l2.4 2.4 L8.6 2.6.
        let o = at - vec2(5.0, 5.0);
        ui.painter().add(Shape::line(vec![o + vec2(1.5, 5.2), o + vec2(3.9, 7.6), o + vec2(8.6, 2.6)], stroke));
    } else {
        ui.painter().line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], stroke);
    }
}

/// A verdict or an error: its mark when it has one, then its words wrapped
/// beside the mark in up to two lines. Returns where it was drawn.
fn verdict(ui: &Ui, at: Pos2, width: f32, words: &str, tone: Color32, marked: Option<bool>) -> Rect {
    // The canvas's 12-point mark and the 8 points after it.
    let indent = if marked.is_some() { 20.0 } else { 0.0 };
    let drawn = wrapped(ui, at + vec2(indent, 0.0), width - indent, words, body(13.5), tone, 2);
    if let Some(ok) = marked {
        mark(ui, pos2(at.x + 6.0, at.y + drawn.height().min(19.0) / 2.0), ok, tone);
    }
    drawn.union(Rect::from_min_size(at, vec2(indent, 1.0)))
}

/// A modal's error line in gold, a row of the layout: the cells' dash as a
/// mark, then the words.
fn error_row(ui: &mut Ui, words: &str, pal: &Pal) {
    let w = ui.available_width();
    let at = ui.cursor().min;
    let drawn = verdict(ui, at, w, words, pal.gold, Some(false));
    ui.allocate_exact_size(vec2(w, drawn.height().max(18.0)), Sense::hover());
}

/// A tooltip with `tip` over `rect` where `shown`, drawn there in `font`,
/// was cut: the cells' tip over a clipped cell.
fn tip_if_cut(ui: &Ui, id: Id, rect: Rect, shown: &str, font: FontId, tip: String) {
    if width_of(ui, shown, font) > rect.width() {
        let _ = ui.interact(rect, id, Sense::hover()).on_hover_text(tip);
    }
}

/// `s` cut at its start to fit `max_w` in `font`, an ellipsis where it was
/// cut: a path keeps its leaf. The fewest characters are cut that let the
/// rest fit, found by halving, as the rest only narrows the more is cut.
fn clip_start(ui: &Ui, s: &str, font: &FontId, max_w: f32) -> String {
    if width_of(ui, s, font.clone()) <= max_w {
        return s.to_string();
    }
    let starts: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    let kept = |cut: usize| format!("…{}", starts.get(cut).map_or("", |&i| &s[i..]));
    let (mut lo, mut hi) = (1, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if width_of(ui, &kept(mid), font.clone()) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    kept(lo)
}

/// A cell that is a word behind its LED (a status, a confidence, an
/// outcome): the LED in the word's tone where the cells draw a glyph, the
/// word after it.
fn led_cell(ui: &Ui, row: &RowCtx, at: usize, words: &str, tone: Color32, lit: bool) {
    let (x, w) = row.cells[at];
    let mid = row.rect.center().y;
    let from = if lit {
        led(ui, pos2(x + 3.0, mid), tone);
        x + 14.0
    } else {
        x
    };
    text(ui, pos2(from, mid), words, body(13.0), tone, (x + w - from).max(1.0), Align2::LEFT_CENTER);
}

/// A full-width card that is one act, 56 points tall: a hairline in
/// `tone`, `label` in its semibold at the centre, washed in the tone under
/// the pointer — the kit's add card in a tone of the room's and without
/// its plus (the client page's Connect and Turn off cards).
fn card(ui: &mut Ui, id: Id, label: &str, tone: Color32) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 56.0), Sense::hover());
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_filled(rect, square(), tone.gamma_multiply(0.10));
    }
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, tone), StrokeKind::Inside);
    text(ui, rect.center(), label, strong(14.0), tone, (w - 32.0).max(1.0), Align2::CENTER_CENTER);
    response
}

/// The ring round a radio group that has the keys (the client page's Tab
/// walks two of them), from `top` to where the layout stands, as wide as
/// the rows' hover.
fn group_ring(ui: &Ui, top: f32, pal: &Pal) {
    let column = ui.max_rect();
    let rect = Rect::from_min_max(pos2(column.min.x - 8.0, top - 2.0), pos2(column.max.x + 8.0, ui.cursor().min.y + 2.0));
    ring(ui, rect, pal);
}

// ── The client page ───────────────────────────────────────────────────────

/// The client page: the pitch, the client radio group, the card that
/// applies the pick, and — with someone to whitelist, where there is room
/// — who may add torrents.
fn choose(ui: &mut Ui, room: &Room, pal: &Pal, out: &mut Out) {
    line(ui, &t!("tor.pitch_1"), body(14.0), pal.text);
    line(ui, &t!("tor.pitch_2"), body(14.0), pal.text);
    line(ui, &t!("tor.mobile_note", host = host_of(&room.client)), body(13.0), pal.dim);
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
        group_ring(ui, top, pal);
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
        if card(ui, Id::new("tor-card"), &words, tone).clicked() {
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
            group_ring(ui, top, pal);
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
    let said = if let Some(e) = &c.error {
        Some((e.clone(), pal.gold, Some(false)))
    } else {
        match &c.probe {
            Some(Ok((version, rpc))) => {
                let rpc = if rpc.is_empty() { String::new() } else { t!("tor.probe_rpc", n = rpc).to_string() };
                let words = t!("tor.probe_ok", client = c.kind.label(), version = version, rpc = rpc);
                Some((words.to_string(), pal.ok, Some(true)))
            }
            Some(Err(e)) => Some((t!("tor.probe_failed", err = e).to_string(), pal.gold, Some(false))),
            None if c.kind == Kind::Qbittorrent => Some((t!("tor.csrf_note").to_string(), pal.gold, None)),
            None => None,
        }
    };
    // Left off when the page has no room left for it, as in the cells.
    if let Some((words, tone, marked)) = said
        && y + FORM_NUDGE < area.max.y
    {
        y = verdict(ui, pos2(x, y + FORM_NUDGE), form_w, &words, tone, marked).max.y + FORM_GAP;
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
    let test_w = form_button_width(ui, &test);
    let test_at = Rect::from_min_size(pos2(connect_at.min.x - 10.0 - test_w, by), vec2(test_w, FORM_BUTTON_H));
    if form_button(ui, Id::new("tor-test"), test_at, &test, false, pal).clicked() {
        out.acts.push(Act::ConnectTest);
    }
    if form_button(ui, Id::new("tor-connect"), connect_at, &connect, true, pal).clicked() {
        out.acts.push(Act::ConnectSubmit);
    }
}

/// A field's label over its box in `slot`, the box drawn from `input`.
fn labelled(ui: &Ui, id: Id, slot: Rect, label: &str, input: &Field, pal: &Pal) -> Typed {
    small_caps(ui, pos2(slot.min.x, slot.min.y + 7.0), label, pal.dim, slot.width(), Align2::LEFT_CENTER);
    let rect = Rect::from_min_size(pos2(slot.min.x, slot.max.y - FIELD_H), vec2(slot.width(), FIELD_H));
    field_box(ui, id, rect, input, pal)
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
    let mut drawn = Field::new(input.value(), input.cursor()).focused(on, if on { composition } else { "" });
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
    let typed = labelled(ui, Id::new("tor-connect-field").with(index), slot, &label, &drawn, pal);
    if typed.response.clicked() {
        out.acts.push(Act::ConnectFocus(index));
    }
    if on {
        out.caret = typed.caret;
    }
}

/// The HTTPS tick in `at`, its box lit while it has the keys: a click
/// flips it.
fn https(ui: &mut Ui, c: &Connect, at: Rect, pal: &Pal, out: &mut Out) {
    let mut row = ui.new_child(UiBuilder::new().id_salt("tor-https-row").max_rect(at).layout(Layout::top_down(Align::Min)));
    let response = check_line(&mut row, Id::new("tor-https"), c.https, &t!("tor.https"), "", pal);
    if response.clicked() {
        out.acts.push(Act::ConnectHttps);
    }
    // With the keys its box is on-accent on the accent, as the canvas and
    // the cells mark it: over the kit's box, at the head of its line.
    if c.focused() == CField::Https {
        let line = response.rect;
        let tick = Rect::from_min_size(pos2(line.min.x, line.center().y - 8.0), vec2(16.0, 16.0));
        ui.painter().rect_filled(tick, square(), pal.accent);
        if c.https {
            mark(ui, tick.center(), true, pal.on_accent);
        }
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

/// What a tab's table says in place of rows, on its first row's line.
fn in_place_of_rows(ui: &Ui, top: Pos2, width: f32, words: &str, pal: &Pal) {
    text(ui, pos2(top.x + 10.0, top.y + FIRST_ROW), words, body(13.5), pal.dim, (width - 20.0).max(1.0), Align2::LEFT_CENTER);
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
    let input = Field::new(room.filter.value(), room.filter.cursor())
        .focused(focused, if focused { composition } else { "" })
        .placeholder(&hint);
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
    let table = Table::new(Id::new("tor-torrents"), &cols, len).cursor(room.sel);
    let shown = table.show(ui, pal, |ui, i, row| torrent_row(ui, &room.torrents[filtered[i]], i, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
    if let Some(err) = &room.list_error {
        let words = t!("tor.list_error", err = printable(err, 200));
        wrapped(ui, pos2(top.x + 10.0, top.y + FIRST_ROW - 10.0), w - 20.0, &words, body(13.5), pal.gold, 2);
    } else if !room.list_loaded {
    } else if room.torrents.is_empty() {
        in_place_of_rows(ui, top, w, &t!("tor.empty_list"), pal);
    } else if filtered.is_empty() {
        in_place_of_rows(ui, top, w, &t!("tor.empty_filter"), pal);
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
    cell(ui, row, 0, &name, font.clone(), ink(row, pal.text, pal), false);
    tip_if_cut(ui, Id::new("tor-tip-name").with(i), row.cell(0), &name, font, format!("{name} · {}", t.info_hash));
    led_cell(ui, row, 1, &status_word(&t.status), status_tone(&t.status, pal), true);
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
    let w = ui.available_width();
    let top = ui.cursor().min;
    let table = Table::new(Id::new("tor-libraries"), &cols, room.libraries.len()).cursor(room.sel);
    let shown = table.show(ui, pal, |ui, i, row| library_row(ui, room, i, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
    if room.libraries.is_empty() {
        in_place_of_rows(ui, top, w, &t!("tor.empty_libraries"), pal);
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
    let font = if row.selected || row.hovered { strong(14.0) } else { body(14.0) };
    cell(ui, row, 0, &printable(name, NAME_MAX), font, ink(row, pal.text, pal), false);
    let access = room.access.get(name);
    let confidence = access.map(|a| a.confidence.as_str()).unwrap_or("unconfirmed");
    led_cell(ui, row, 1, &confidence_words(confidence), confidence_tone(confidence, pal), true);
    match access.and_then(|a| a.daemon_path.clone()).filter(|p| !p.is_empty()) {
        Some(path) if confidence != "pending" => {
            // Cut at its start, so its leaf stays, as the canvas cuts paths.
            let path = printable(&path, 300);
            let shown = clip_start(ui, &path, &mono(12.5), row.cell(2).width());
            cell(ui, row, 2, &shown, mono(12.5), ink(row, pal.text, pal), false);
            tip_if_cut(ui, Id::new("tor-tip-path").with(i), row.cell(2), &path, mono(12.5), path.clone());
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
            cell(ui, row, 3, &template, mono(12.5), ink(row, pal.text, pal), false);
            tip_if_cut(ui, Id::new("tor-tip-template").with(i), row.cell(3), &template, mono(12.5), template.clone());
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
    if add_card(ui, Id::new("tor-seed-card"), &t!("tor.seed_card"), pal).clicked() {
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
    let w = ui.available_width();
    let top = ui.cursor().min;
    let table = Table::new(Id::new("tor-seeding"), &cols, room.seeds.len()).cursor(room.sel);
    let shown = table.show(ui, pal, |ui, i, row| seed_row(ui, &room.seeds[i], &client, i, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
    if room.seeds.is_empty() {
        in_place_of_rows(ui, top, w, &t!("tor.seed_empty"), pal);
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
        let words = ui.interact(words_at, id.with("name"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
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
    cell(ui, row, 0, &printable(&s.file, 200), body(14.0), ink(row, pal.text, pal), false);
    let (word, tone, lit) = match (&s.outcome, &s.error) {
        (Some(o), _) => outcome_look(&o.outcome, pal),
        (None, Some(_)) => outcome_look("daemon_error", pal),
        (None, None) => (t!("tor.seed_checking").to_string(), pal.accent, true),
    };
    led_cell(ui, row, 1, &word, tone, lit);
    let details = outcome_details(s, client);
    let quiet = s.outcome.as_ref().is_some_and(|o| matches!(o.outcome.as_str(), "no_match" | "already_in_daemon"));
    cell(ui, row, 2, &details, body(13.0), ink(row, if quiet { pal.dim } else { pal.text }, pal), false);
    tip_if_cut(ui, Id::new("tor-tip-details").with(i), row.cell(2), &details, body(13.0), details.clone());
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
    let cols = [
        Col::left(t!("tor.col_user"), 0.0),
        Col::left(t!("tor.col_admin"), 120.0),
        Col::left(t!("tor.col_torrents"), 120.0),
    ];
    let users: Vec<(&String, &AdminUser)> = room.users.iter().collect();
    let w = ui.available_width();
    let top = ui.cursor().min;
    let table = Table::new(Id::new("tor-access"), &cols, users.len()).cursor(room.sel);
    let shown = table.show(ui, pal, |ui, i, row| user_row(ui, users[i].0, users[i].1, whitelist, row, pal));
    if let Some(i) = shown.row {
        out.acts.push(Act::Select(i));
    }
    if users.is_empty() {
        in_place_of_rows(ui, top, w, &t!("tor.empty_users"), pal);
    }
}

fn user_row(ui: &Ui, name: &str, user: &AdminUser, whitelist: bool, row: &RowCtx, pal: &Pal) {
    let font = if row.selected || row.hovered { strong(14.0) } else { body(14.0) };
    let own = if whitelist { pal.text } else { pal.dim };
    cell(ui, row, 0, &printable(name, NAME_MAX), font, ink(row, own, pal), false);
    let admin = if user.admin { t!("tor.admin_yes").to_string() } else { "—".to_string() };
    cell(ui, row, 1, &admin, body(13.0), ink(row, pal.dim, pal), false);
    let tone = match (user.allow_torrent, whitelist) {
        (true, true) => pal.ok,
        (true, false) => pal.dim,
        (false, _) => pal.unlit,
    };
    led(ui, pos2(row.cells[2].0 + 3.0, row.rect.center().y), tone);
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
    led(ui, pos2(x + 3.0, mid), tone);
    text(ui, pos2(x + 14.0, mid), &unbullet(&status), body(13.5), tone, (row.max.x - x - 14.0).max(1.0), Align2::LEFT_CENTER);
    ui.add_space(14.0);

    let cfg = room.config().cloned().unwrap_or_default();
    let mut saved = vec![(t!("tor.field_host").to_string(), cfg.host.clone()), (t!("tor.field_port").to_string(), cfg.port.to_string())];
    if kind.has_username() {
        saved.push((t!("tor.field_username").to_string(), cfg.username.trim().to_string()));
    }
    if kind.has_rpc_path() {
        saved.push((t!("tor.field_rpc").to_string(), cfg.rpc_path.clone().unwrap_or_default()));
    }
    for (key, value) in saved {
        let (row, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        let mid = row.center().y;
        small_caps(ui, row.left_center(), &key, pal.dim, KEY_W - 8.0, Align2::LEFT_CENTER);
        let at = pos2(row.min.x + KEY_W, mid);
        if value.is_empty() {
            text(ui, at, "—", body(13.5), pal.dim, w - KEY_W, Align2::LEFT_CENTER);
        } else {
            text(ui, at, &printable(&value, 200), mono(12.5), pal.text, w - KEY_W, Align2::LEFT_CENTER);
        }
    }
    // HTTPS, lit while the saved connection uses it.
    let (row, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    let (lamp, words) = if cfg.use_https { (pal.ok, pal.text) } else { (pal.unlit, pal.dim) };
    led(ui, pos2(row.min.x + KEY_W + 3.0, row.center().y), lamp);
    text(ui, pos2(row.min.x + KEY_W + 14.0, row.center().y), &t!("tor.https"), body(13.5), words, w - KEY_W - 14.0, Align2::LEFT_CENTER);
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
        let drawn = wrapped(ui, pos2(at.x + KEY_W, at.y), w - KEY_W, &words, body(13.5), pal.text, 2);
        ui.allocate_exact_size(vec2(w, drawn.height().max(18.0)), Sense::hover());
        ui.add_space(14.0);
    }
    if ui.available_height() >= 2.0 * LINE_H + 10.0 + FORM_BUTTON_H {
        line(ui, &t!("tor.mobile_note", host = host_of(&room.client)), body(13.0), pal.dim);
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
        let bw = form_button_width(ui, &label);
        let at = Rect::from_min_size(pos2(right - bw, by), vec2(bw, FORM_BUTTON_H));
        if form_button(ui, Id::new(id), at, &label, primary, pal).clicked() {
            out.acts.push(act);
        }
        right = at.min.x - 10.0;
    }
}

// ── The note line ─────────────────────────────────────────────────────────

/// The region's foot: what the room is busy with in the accent, else its
/// note (an error in gold), else on the tabs the cursor row's story, as
/// the cells tell it (`filtered`, the Torrents tab's rows).
fn foot(ui: &Ui, rect: Rect, room: &Room, filtered: &[usize], pal: &Pal) {
    if room.busy.is_some() || room.note.is_some() {
        note_line(ui, rect, room.busy.as_deref(), room.note.as_ref(), pal);
        return;
    }
    if room.phase() != Phase::Tabs {
        return;
    }
    let parts = match room.tab {
        Tab::Torrents => torrent_story(room, filtered, pal),
        Tab::Libraries => library_story(room, pal),
        Tab::Seeding => seed_story(room, pal),
        Tab::Access => user_story(room, pal),
        Tab::Client => client_story(room, pal),
    };
    run(ui, rect, &parts);
}

/// The cursor torrent's short hash, its error, who added it and when —
/// while the rows are drawn, as in the cells: a failed list read keeps the
/// torrents and the cursor it had, but its error stands in place of the
/// rows, and no row on screen has a story to tell.
fn torrent_story(room: &Room, filtered: &[usize], pal: &Pal) -> Vec<Part> {
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
fn library_story(room: &Room, pal: &Pal) -> Vec<Part> {
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
fn seed_story(room: &Room, pal: &Pal) -> Vec<Part> {
    let Some(s) = room.sel.and_then(|i| room.seeds.get(i)) else { return Vec::new() };
    let details = outcome_details(s, &room.kind().label());
    vec![(printable(&s.file, NAME_MAX), strong(13.0), pal.text), (format!(" — {details}"), body(13.0), pal.text)]
}

/// Whether the cursor user may add torrents, and how to flip it.
fn user_story(room: &Room, pal: &Pal) -> Vec<Part> {
    let Some(name) = room.selected_user() else { return Vec::new() };
    let allowed = room.users.get(name).is_some_and(|u| u.allow_torrent);
    let user = printable(name, NAME_MAX);
    let words = if allowed { t!("tor.note_user_may", user = user) } else { t!("tor.note_user_may_not", user = user) };
    vec![(words.to_string(), body(13.0), pal.text), (t!("tor.note_user_tail").to_string(), body(13.0), pal.dim)]
}

/// When the daemon last answered, or why it did not.
fn client_story(room: &Room, pal: &Pal) -> Vec<Part> {
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
                (t!("tor.template_suggested").to_string(), false, Act::TemplateSuggest),
                (t!("tor.template_clear").to_string(), false, Act::TemplateClear),
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
            let buttons = [
                (t!("tor.remove_keep").to_string(), true, Act::RemoveCancel),
                (t!("tor.remove_confirm").to_string(), false, Act::RemoveConfirm),
            ];
            gate(ui, Id::new("tor-remove"), over, &title, &lines, &buttons, pal, out);
        }
        Modal::Disconnect => {
            let title = t!("tor.disconnect_title", client = room.kind().label()).to_string();
            let lines = [t!("tor.disconnect_1").to_string(), t!("tor.disconnect_2").to_string()];
            let buttons = [
                (t!("tor.disconnect_keep").to_string(), true, Act::DisconnectCancel),
                (t!("tor.disconnect_confirm").to_string(), false, Act::DisconnectConfirm),
            ];
            gate(ui, Id::new("tor-disconnect"), over, &title, &lines, &buttons, pal, out);
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
        let at = pos2(row.min.x + 20.0, mid);
        ui.painter().add(Shape::convex_polygon(vec![at + vec2(-2.5, -4.0), at + vec2(3.5, 0.0), at + vec2(-2.5, 4.0)], pal.dim, Stroke::NONE));
    }
    text(ui, pos2(row.min.x + 32.0, mid), words, mono(12.5), pal.dim, (w - 32.0).max(1.0), Align2::LEFT_CENTER);
}

/// A modal's buttons, right-aligned on a row of their own, 10 points apart
/// in `buttons`' order: the acts of those clicked.
fn button_row(ui: &mut Ui, id: Id, buttons: &[(String, bool, Act)], pal: &Pal) -> Vec<Act> {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
    let mut right = row.max.x;
    let mut acts = Vec::new();
    for (i, (label, primary, act)) in buttons.iter().enumerate().rev() {
        let bw = form_button_width(ui, label);
        let at = Rect::from_min_size(pos2(right - bw, row.min.y), vec2(bw, FORM_BUTTON_H));
        if form_button(ui, id.with("button").with(i), at, label, *primary, pal).clicked() {
            acts.push(act.clone());
        }
        right = at.min.x - 10.0;
    }
    acts
}

/// An entry modal: one text field, the lines under it, its error, its
/// buttons.
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
        let shown = modal(ui, id, over, ENTRY_W, title, pal, |ui| {
            // A path, a template of one: the figures' monospace, as the
            // canvas sets a path's field and this room its paths.
            let drawn = Field::new(input.value(), input.cursor()).focused(true, composition).mono();
            let typed = studio::field(ui, id.with("field"), label, &drawn, pal);
            ui.add_space(14.0);
            lines(ui);
            if let Some(e) = error {
                ui.add_space(6.0);
                error_row(ui, e, pal);
            }
            ui.add_space(20.0);
            let clicked = button_row(ui, id, buttons, pal);
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

/// A gate: its lines in gold, wrapped, then the safe choice lit and the
/// one that goes through beside it. The canvas's modal has a close, and
/// the gate's way out is Esc, which its tips name: the close sends it.
#[allow(clippy::too_many_arguments)]
fn gate(ui: &Ui, id: Id, over: Rect, title: &str, lines: &[String], buttons: &[(String, bool, Act)], pal: &Pal, out: &mut Out) {
    let shown = modal(ui, id, over, GATE_W, title, pal, |ui| {
        for words in lines {
            let words = egui::RichText::new(words).font(body(14.0)).color(pal.gold);
            ui.add(egui::Label::new(words).wrap().selectable(false));
            ui.add_space(4.0);
        }
        ui.add_space(20.0);
        button_row(ui, id, buttons, pal)
    });
    out.acts.extend(shown.inner);
    if shown.closed {
        out.keys.push(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::TextShape;

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
            let _ = begin(ui);
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
                "connected",
                " — Transmission 4.0.6 · 127.0.0.1:9091 · 2 of 4 libraries reachable",
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
                "Add a .torrent file ▸",
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
                "use HTTPS",
                "ALSO SAVED",
                "qBittorrent 192.168.1.20:8080 — c switches without retyping the password",
                "Users add torrents from a phone at home.mstream.example/torrent — the apps don't expose it.",
                "Disconnecting forgets the credentials here; the daemon keeps every torrent it has.",
                "TEST",
                "DISCONNECT",
                "SWITCH CLIENT ▸",
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
        assert_drawn(&out, &["e5f6e5f6e5f6…", " · ", "Tracker gave HTTP response code 403 (Forbidden)", " · added by iros", " · 2d ago"]);
        room.busy = Some(t!("tor.busy_loading").to_string());
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["loading…"]);
        assert!(!words(&out).iter().any(|w| w == "e5f6e5f6e5f6…"), "the busy line wins");
        room.busy = None;
        room.note = Some(("could not reach server".into(), true));
        assert_drawn(&settle(&ctx, &mut room).0, &["could not reach server"]);

        room.act(Act::Tab(Tab::Libraries));
        room.act(Act::Select(3));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["vinyl", " — ", "daemon free-space returned -1 (path not visible to daemon)", " · ", "probed 3m ago"]);
        room.act(Act::Tab(Tab::Access));
        room.act(Act::Select(1));
        assert_drawn(&settle(&ctx, &mut room).0, &["guest may not add torrents", " · s flips it, at once"]);
    }

    #[test]
    fn a_failed_list_read_stands_in_place_of_the_rows_and_tells_no_rows_story() {
        let _en = english();
        let ctx = context();
        let mut room = connected();
        room.act(Act::Select(3));
        assert_drawn(&settle(&ctx, &mut room).0, &["Radiohead - In Rainbows [MP3 V0]", "e5f6e5f6e5f6…"]);
        // What a failed poll leaves (`Done::Listed`): the list and the
        // cursor it had, and its error.
        room.list_error = Some("could not reach server".into());
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["couldn't fetch torrents: could not reach server"]);
        let drawn = words(&out);
        assert!(!drawn.iter().any(|w| w == "Radiohead - In Rainbows [MP3 V0]"), "the error stands in place of the rows: {drawn:?}");
        assert!(!drawn.iter().any(|w| w == "e5f6e5f6e5f6…"), "no row's story rides the note line: {drawn:?}");
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
                "off",
                " — no torrent client chosen",
                "Hand off magnet links and .torrent files to a torrent client on this host or on the LAN.",
                "Completed downloads are picked up by the next library scan.",
                "Users add torrents from a phone at home.mstream.example/torrent — the apps don't expose it.",
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
        assert_drawn(&settle(&ctx, &mut room).0, &["Connect to Transmission ▸"]);

        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(
            &out,
            &[
                "transmission",
                " — no credentials yet · Test saves nothing; Connect saves after a good probe",
                "HOST",
                "PORT",
                "USERNAME",
                "PASSWORD",
                "RPC PATH",
                "use HTTPS",
                "9091",
                "/transmission/rpc",
                "^T TEST",
                "CONNECT ▸",
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
    fn each_gate_draws_and_its_buttons_and_close_do_what_the_cells_and_esc_do() {
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
                "◂ KEEP",
                "REMOVE",
            ],
        );
        click_on(&ctx, &mut room, remove.with("button").with(0));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "keep is the safe choice");
        room.act(Act::Remove(hash.clone()));
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, remove.with("close"));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "the close is Esc: keep");
        room.act(Act::Remove(hash.clone()));
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, remove.with("button").with(1));
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
                "◂ STAY",
            ],
        );
        click_on(&ctx, &mut room, disconnect.with("close"));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none());
        room.act(Act::Disconnect);
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, disconnect.with("button").with(0));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "stay");
        room.act(Act::Disconnect);
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, disconnect.with("button").with(1));
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
        assert_drawn(&out, &["Map vinyl for Transmission", "the daemon is asked to see a marker mStream writes there", "VERIFY AND SAVE ▸"]);
        assert!(shown.caret.is_some(), "the entry's field has the keyboard");
        // The cells register the box as the modal's Enter.
        click_on(&ctx, &mut room, mapping.with("field"));
        assert_drawn(&settle(&ctx, &mut room).0, &["a path to a path is needed"]);
        click_on(&ctx, &mut room, mapping.with("close"));
        assert!(matches!(room.modal, Modal::None));
        room.act(Act::Mapping("music".into()));
        let _ = settle(&ctx, &mut room);
        click_on(&ctx, &mut room, mapping.with("button").with(0));
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
                "^S SUGGESTED TEMPLATE",
                "^X CLEAR",
                "SAVE ▸",
            ],
        );
        click_on(&ctx, &mut room, template.with("button").with(1));
        assert!(matches!(&room.modal, Modal::Template { template, .. } if template.value().is_empty()), "clear");
        assert_drawn(&settle(&ctx, &mut room).0, &["(no template — the user types the path)"]);
        click_on(&ctx, &mut room, template.with("button").with(0));
        let suggested = room.templates.suggested_template.clone();
        assert!(matches!(&room.modal, Modal::Template { template, .. } if template.value() == suggested), "suggested");
        click_on(&ctx, &mut room, template.with("button").with(2));
        assert_eq!(room.queued, Some(Op::Template { vpath: "music".into(), template: Some(suggested) }));
        room.queued = None;
        room.busy = None;
        room.modal = Modal::None;

        let seed = Id::new("tor-seed-path");
        room.act(Act::SeedAdd);
        let (out, _) = settle(&ctx, &mut room);
        assert_drawn(&out, &["Add a .torrent file", ".TORRENT FILE ON THIS MACHINE", "Tab completes · the file is read here and sent to the server", "CHECK AND SEED ▸"]);
        click_on(&ctx, &mut room, seed.with("button").with(0));
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
