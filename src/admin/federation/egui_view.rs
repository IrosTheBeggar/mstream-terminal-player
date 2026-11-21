//! The Federation room drawn by egui, in the desktop window's panel: the
//! same [`Room`] the cells view draws (the parent module: its state, its
//! worker, its keys and every word it says); only the drawing is new. The
//! look is the Studio kit's (src/gui/window/studio.rs), laid out after the
//! Claude Design canvas of the Admin screen (its Federation board and its
//! "mint a ticket" board, 2026-11-20): the state line led by its LED, the
//! tabs as one segmented group, each tab's card over its table, the room's
//! note along its foot, and the forms and the gates as the kit's modals
//! over the room.
//!
//! Every click is one of the room's own [`Act`]s — the one the cells view
//! registers where the click lands — applied through [`Room::act`] in the
//! pass. A gate has no close, as the cells draw it: it is answered with its
//! buttons or its keys. Keys never reach egui: the room's key handler edits
//! its fields as it always has, and each field is drawn from the room's own
//! value and caret. Where the cells draw a glyph — the state line's bullet,
//! the claimed tick, a peer's status dot, the checkboxes, the connecting
//! strip, a button's arrow — the desk draws its LED, its tick box, its
//! meter or its button instead.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::{Align, Align2, Color32, Id, Layout, Pos2, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use super::*;
use crate::admin::{EguiShown, fmt_bytes};
use crate::gui::window::studio::{
    self, Col, Dialog, FIELD_H, FIELD_ROW_H, Glyph, Pal, Para, Run, SEG_H, Seg, TICK_H, Table, Typed, bare, body, cadence, caps,
    card, cell, cell_tipped, checkbox, field_at, field_box, fitted, foot, foot_line, gate, ink, label_at, led, led_cell, led_mid,
    mono, name_cell, reveal, room_rects, searching, segmented, state_line, strong, text, tick_card, tick_grid, tick_row, unkeyed,
    with_figures,
};

/// The room between the room's parts: the canvas's 18.
const GAP: f32 = studio::ROOM_GAP;
/// A table's headings and the room under them, and its rows.
const TABLE_HEAD: f32 = studio::TABLE_HEAD;
const ROW_H: f32 = 40.0;
/// The forms' window, the canvas's.
const FORM_W: f32 = 640.0;
/// The room between a modal's parts: the canvas's 22.
const SECTION: f32 = 22.0;
/// The gap between the forms' columns (their fields).
const FORM_GAP: f32 = 16.0;
/// The least a table's flexible column keeps where the default window is
/// narrower than the canvas, and the least share its fixed ones narrow to.
const FLEX_LEAST: f32 = 120.0;
const NARROWEST: f32 = 0.4;

/// What a pass gathers as it draws: the room's acts its clicks asked for,
/// and the foot of the focused field's caret.
struct Gathered<'a> {
    acts: Vec<Act>,
    caret: Option<Pos2>,
    /// The input method's composition, for the field with the keyboard.
    composition: &'a str,
}

/// What a modal's body gathers: its acts and its caret.
#[derive(Default)]
struct Out {
    acts: Vec<Act>,
    caret: Option<Pos2>,
}

/// One pass of the room over the whole of `ui` — the room's area less its
/// first row — with the input method composing `composition`: the room
/// drawn from its state, each click turned into the room's act and applied,
/// and what the host must know handed back.
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    let pal = Pal::now();
    let full = ui.max_rect();
    let (body_rect, note_rect) = room_rects(full);
    let modal_open = room.modal_open();
    let mut got = Gathered { acts: Vec::new(), caret: None, composition: if modal_open { "" } else { composition } };

    let mut ui_body = ui.new_child(UiBuilder::new().max_rect(body_rect).layout(Layout::top_down(Align::Min)));
    ui_body.set_clip_rect(body_rect.expand(1.0));
    match room.params.clone() {
        None => {}
        Some(p) if !p.enabled => off(&mut ui_body, room, &p, &pal, &mut got),
        Some(p) => on(&mut ui_body, room, &p, &pal, &mut got),
    }
    note(ui, note_rect, room, &pal);

    // A modal makes the room beneath inert: what was clicked under it this
    // pass is dropped, as the cells drop the room's registrations, and the
    // keyboard is the modal's.
    if modal_open {
        got.acts.clear();
        got.caret = None;
        got.composition = composition;
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Form(f) => form(ui, &f, full, &pal, &mut got),
        Modal::Minted { name, ticket, libraries } => minted(ui, &name, ticket.as_deref(), &libraries, full, &pal, &mut got),
        Modal::PeerName { name, preview, .. } => peer_name(ui, &name, &preview, full, &pal, &mut got),
        Modal::Decline(id) => {
            let who = room.request(id).map(request_name).unwrap_or_default();
            let lines = [t!("fed.decline_1").to_string(), t!("fed.decline_2").to_string()];
            let safe = (t!("fed.gate_keep").to_string(), Act::DeclineCancel);
            let go = (t!("fed.decline_confirm").to_string(), Act::DeclineConfirm);
            warn(ui, Id::new("fed-gate").with("decline"), full, &t!("fed.decline_title", name = who), &lines, safe, go, &pal, &mut got);
        }
        Modal::Revoke(id) => {
            let (name, libs) =
                room.key_row(id).map(|k| (printable(&k.name, NAME_MAX), k.library_names.join(", "))).unwrap_or_default();
            let lines = [t!("fed.revoke_1", name = name, libraries = libs).to_string(), t!("fed.revoke_2").to_string()];
            let safe = (t!("fed.gate_keep").to_string(), Act::RevokeCancel);
            let go = (t!("fed.revoke_confirm").to_string(), Act::RevokeConfirm);
            warn(ui, Id::new("fed-gate").with("revoke"), full, &t!("fed.revoke_title", name = name), &lines, safe, go, &pal, &mut got);
        }
        Modal::Forget(id) => {
            let name = room.peer(id).map(|p| printable(&p.name, NAME_MAX)).unwrap_or_default();
            let lines = [t!("fed.forget_1").to_string(), t!("fed.forget_2").to_string()];
            let safe = (t!("fed.gate_keep").to_string(), Act::ForgetCancel);
            let go = (t!("fed.forget_confirm").to_string(), Act::ForgetConfirm);
            warn(ui, Id::new("fed-gate").with("forget"), full, &t!("fed.forget_title", name = name), &lines, safe, go, &pal, &mut got);
        }
        Modal::TurnOff => {
            let lines = [t!("fed.off_1").to_string(), t!("fed.off_2").to_string()];
            let safe = (t!("fed.off_stay").to_string(), Act::TurnOffCancel);
            let go = (t!("fed.off_confirm").to_string(), Act::TurnOffConfirm);
            warn(ui, Id::new("fed-gate").with("turn-off"), full, &t!("fed.off_title"), &lines, safe, go, &pal, &mut got);
        }
    }

    let mut outcome = None;
    for act in got.acts {
        outcome = outcome.or(room.act(act));
    }
    EguiShown { caret: got.caret, outcome }
}

// ── Off ───────────────────────────────────────────────────────────────────

/// Off: the state line, the card that turns the endpoint on (dim and deaf
/// while the start is on its way; none where Iroh is unavailable), and the
/// pitch.
fn off(ui: &mut Ui, room: &Room, p: &FederationParams, pal: &Pal, got: &mut Gathered) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    if p.available {
        state_line(ui, row, pal.unlit, &[(bare(&t!("fed.state_off")).to_string(), body(14.0), pal.dim)], &[]);
    } else {
        state_line(ui, row, pal.gold, &[(bare(&t!("fed.state_unavailable")).to_string(), body(14.0), pal.gold)], &[]);
    }
    ui.add_space(GAP);
    if p.available {
        let label = if room.starting { t!("fed.turning_on") } else { t!("fed.turn_on") };
        if card(ui, Id::new("fed-turn-on"), &label, Glyph::None, pal.ok, !room.starting, pal).clicked() {
            got.acts.push(Act::TurnOn);
        }
        ui.add_space(GAP);
    }
    let pitch = [
        (t!("fed.pitch_1a").to_string(), body(14.0), pal.text),
        (t!("fed.pitch_1b").to_string(), strong(14.0), pal.text),
        (t!("fed.pitch_1c").to_string(), body(14.0), pal.text),
    ];
    Para::runs(&pitch).show(ui);
    ui.add_space(GAP);
    let (head, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
    label_at(ui, head.left_center(), &t!("fed.pitch_credentials"), w, pal);
    ui.add_space(4.0);
    Para::new(&t!("fed.pitch_2"), body(14.0), pal.text).show(ui);
}

// ── On ────────────────────────────────────────────────────────────────────

/// On: the state line (and the connecting strip while the endpoint comes
/// up), the tabs, the active tab's card and table.
fn on(ui: &mut Ui, room: &Room, p: &FederationParams, pal: &Pal, got: &mut Gathered) {
    state(ui, room, p, pal);
    if p.available && !(p.running && p.online) {
        ui.add_space(4.0);
        // The cells' ▱▱▱▱▱▱▱▱▱▱: the desk's meter, every segment unlit.
        searching(ui, &t!("fed.connecting_note"), pal);
    }
    ui.add_space(GAP);
    tabs(ui, room, pal, got);
    ui.add_space(GAP);
    match room.tab {
        Tab::Requests => requests(ui, room, pal, got),
        Tab::Tickets => tickets(ui, room, pal, got),
        Tab::Peers => peers(ui, room, pal, got),
    }
}

/// The state row: its LED lit green while connected, gold while the relay
/// connects or Iroh is unavailable; the cells' words after it, the endpoint
/// in figures; "polls every 30 s" dim at the end when it fits.
fn state(ui: &mut Ui, room: &Room, p: &FederationParams, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    let connected = p.running && p.online;
    let (lit, mut parts) = if !p.available {
        (pal.gold, vec![(bare(&t!("fed.state_unavailable")).to_string(), strong(14.0), pal.gold)])
    } else if connected {
        let on = (bare(&t!("fed.state_on")).to_string(), strong(14.0), pal.ok);
        (pal.ok, vec![on, (t!("fed.state_relay").to_string(), body(14.0), pal.text)])
    } else {
        let on = (bare(&t!("fed.state_on")).to_string(), strong(14.0), pal.gold);
        (pal.gold, vec![on, (t!("fed.state_connecting").to_string(), body(14.0), pal.text)])
    };
    if let Some(id) = &p.endpoint_id {
        let short = short_id(id);
        parts.extend(with_figures(&t!("fed.state_endpoint", id = short), &[&short], body(14.0), mono(12.5), pal.text));
    }
    let inbox = if room.accept_requests { t!("fed.state_inbox_open") } else { t!("fed.state_inbox_closed") };
    parts.push((inbox.to_string(), body(14.0), pal.text));
    state_line(ui, row, lit, &parts, &cadence(&t!("fed.polls"), pal));
}

/// The tabs, one segmented group with the open one lit (Requests carrying
/// the count waiting on an answer), and the open tab's note dim at the
/// row's end.
fn tabs(ui: &mut Ui, room: &Room, pal: &Pal, got: &mut Gathered) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, SEG_H + 2.0), Sense::hover());
    let pending = room.pending_inbound();
    let labels: Vec<String> = Tab::ALL
        .iter()
        .map(|tab| match tab {
            Tab::Requests if pending > 0 => t!("fed.tab_requests_n", n = pending).to_string(),
            Tab::Requests => t!("fed.tab_requests").to_string(),
            Tab::Tickets => t!("fed.tab_tickets").to_string(),
            Tab::Peers => t!("fed.tab_peers").to_string(),
        })
        .collect();
    let chosen = Tab::ALL.iter().position(|t| *t == room.tab).unwrap_or(0);
    let (seg, clicked) = segmented(ui, Id::new("fed-tabs"), row.min.x, row.center().y, &labels, chosen, Seg::Lit, pal);
    if let Some(i) = clicked {
        got.acts.push(Act::Tab(Tab::ALL[i]));
    }
    let note = match room.tab {
        Tab::Requests => t!("fed.note_requests"),
        Tab::Tickets => t!("fed.note_tickets"),
        Tab::Peers => t!("fed.note_peers"),
    };
    let room_w = row.max.x - seg.max.x - 16.0;
    if room_w > 60.0 {
        text(ui, pos2(row.max.x, row.center().y), &note, body(13.0), pal.dim, room_w, Align2::RIGHT_CENTER);
    }
}

// ── The tabs' bodies ──────────────────────────────────────────────────────

/// Requests: the inbox's card, then the table. Accept, decline, withdraw
/// and dismiss are keys only, as in the cells: no row carries a button.
fn requests(ui: &mut Ui, room: &Room, pal: &Pal, got: &mut Gathered) {
    if tick_card(ui, Id::new("fed-inbox"), room.accept_requests, &t!("fed.inbox_label"), &t!("fed.inbox_hint"), pal).clicked() {
        got.acts.push(Act::ToggleInbox);
    }
    ui.add_space(GAP);
    if ui.available_height() < TABLE_HEAD + ROW_H {
        return;
    }
    let cols = fitted(
        ui.available_width(),
        vec![
            Col::left(t!("fed.col_server"), 170.0),
            Col::left(t!("fed.col_offer"), 0.0),
            Col::left(t!("fed.col_status"), 170.0),
            Col::right(t!("fed.col_age"), 56.0),
        ],
        FLEX_LEAST,
        NARROWEST,
    );
    let empty = if room.accept_requests { t!("fed.empty_requests") } else { t!("fed.empty_requests_closed") };
    let now = unix_now();
    let table = Table::new(Id::new("fed-requests"), &cols, room.requests.len()).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| {
        let r = &room.requests[i];
        name_cell(ui, row, 0, &request_name(r), pal);
        let offer = request_offer(r);
        cell_tipped(ui, row, 1, &offer, &offer, body(13.5), ink(row, pal.soft, pal), Id::new("fed-offer-tip").with(i));
        let (words, family) = request_status(r);
        let (font, color) = match family {
            Family::Theirs => (strong(13.0), pal.gold),
            Family::Good => (body(13.0), pal.ok),
            Family::Dead | Family::Mute => (body(13.0), pal.dim),
            Family::Wait => (body(13.0), pal.text),
        };
        cell(ui, row, 2, &words, font, ink(row, color, pal), false);
        cell(ui, row, 3, &age_of(Some(&r.created_at), now), mono(12.5), ink(row, pal.dim, pal), true);
    });
    if let Some(i) = shown.row {
        got.acts.push(Act::Select(i));
    }
}

/// Tickets: the Mint card, then the table. Copy, limits, reset and revoke
/// are keys only, as in the cells.
fn tickets(ui: &mut Ui, room: &Room, pal: &Pal, got: &mut Gathered) {
    if card(ui, Id::new("fed-mint"), &t!("fed.mint_card"), Glyph::Ticket, pal.ok, true, pal).clicked() {
        got.acts.push(Act::Mint);
    }
    ui.add_space(GAP);
    if ui.available_height() < TABLE_HEAD + ROW_H {
        return;
    }
    let cols = fitted(
        ui.available_width(),
        vec![
            Col::left(t!("fed.col_name"), 190.0),
            Col::left(t!("fed.col_libraries"), 190.0),
            Col::left(t!("fed.col_limits"), 0.0),
            Col::right(t!("fed.col_today"), 84.0),
            Col::left(t!("fed.col_last_used"), 96.0),
            Col::left(t!("fed.col_claimed"), 112.0),
        ],
        FLEX_LEAST,
        NARROWEST,
    );
    let empty = t!("fed.empty_tickets");
    let now = unix_now();
    let table = Table::new(Id::new("fed-tickets"), &cols, room.keys.len()).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| {
        let k = &room.keys[i];
        name_cell(ui, row, 0, &printable(&k.name, NAME_MAX), pal);
        let libs = k.library_names.join(", ");
        cell_tipped(ui, row, 1, &libs, &libs, body(13.5), ink(row, pal.soft, pal), Id::new("fed-libs-tip").with(i));
        let (limits, color) = if k.expired { (t!("fed.exp_expired").to_string(), pal.gold) } else { (limits_words(k), pal.text) };
        cell(ui, row, 2, &limits, mono(12.5), ink(row, color, pal), false);
        // The webapp's fmtBytes drops a trailing .0 ("218 MB", not "218.0 MB").
        let (today, color) = if k.usage_today_bytes > 0 {
            (fmt_bytes(k.usage_today_bytes).replace(".0 ", " "), pal.text)
        } else {
            ("—".to_string(), pal.dim)
        };
        cell(ui, row, 3, &today, mono(12.5), ink(row, color, pal), true);
        let (last, color) = match k.last_used.as_deref() {
            Some(ts) => (age_of(Some(ts), now), pal.text),
            None => (t!("fed.never").to_string(), pal.dim),
        };
        cell(ui, row, 4, &last, mono(12.5), ink(row, color, pal), false);
        // The cells' ✓ is the desk's LED, lit while a server holds the binding.
        let (word, color, lit) = if k.bound_endpoint_id.is_some() {
            (bare(&t!("fed.claimed")).to_string(), pal.ok, pal.ok)
        } else {
            (t!("fed.not_claimed").to_string(), pal.dim, pal.unlit)
        };
        led_cell(ui, row, 5, Some(lit), &word, body(13.0), ink(row, color, pal));
    });
    if let Some(i) = shown.row {
        got.acts.push(Act::Select(i));
    }
}

/// Peers: the paste box with its label and its preview, then the table.
/// Test and forget are keys only, as in the cells; the discovery box is
/// the one control a row draws.
fn peers(ui: &mut Ui, room: &Room, pal: &Pal, got: &mut Gathered) {
    let w = ui.available_width();
    // The label and its hint a space apart, as every label's.
    studio::label(ui, &format!("{}{}", t!("fed.paste_label"), t!("fed.paste_hint")), pal);
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
    let focused = room.ticket_focus && !room.modal_open();
    let typed = field_box(ui, Id::new("fed-ticket"), rect, &studio::Input::of(&room.ticket).focused(focused, got.composition).mono(), pal);
    if typed.response.clicked() {
        got.acts.push(Act::TicketFocus);
    }
    if focused {
        got.caret = typed.caret;
    }
    // The preview: who and what, before the server is asked.
    let typed_words = room.ticket.value().trim();
    if !typed_words.is_empty() {
        ui.add_space(6.0);
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        let (words, color) = match decode_ticket(typed_words) {
            Some(p) => (preview_words(&p), pal.ok),
            None => (t!("fed.ticket_invalid").to_string(), pal.gold),
        };
        text(ui, line.left_center(), &words, body(13.0), color, w, Align2::LEFT_CENTER);
    }
    ui.add_space(GAP);
    if ui.available_height() < TABLE_HEAD + ROW_H {
        return;
    }
    let cols = fitted(
        ui.available_width(),
        vec![
            Col::left("", 8.0),
            Col::left(t!("fed.col_server"), 200.0),
            Col::left(t!("fed.col_status"), 0.0),
            Col::left(t!("fed.col_last_seen"), 100.0),
            Col::left(t!("fed.col_discovery"), 100.0),
        ],
        FLEX_LEAST,
        NARROWEST,
    );
    let empty = t!("fed.empty_peers");
    let now = unix_now();
    let mut picked = None;
    let table = Table::new(Id::new("fed-peers"), &cols, room.peers.len()).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| {
        let p = &room.peers[i];
        let (status, color, lit) = match p.last_status.as_deref() {
            None => (t!("fed.never_tested").to_string(), pal.dim, pal.unlit),
            Some("ok") => (t!("fed.status_ok").to_string(), pal.ok, pal.ok),
            Some(other) => (printable(other, 200), pal.gold, pal.gold),
        };
        // The cells' • is the desk's LED, in the status words' colour,
        // centred on the name it leads.
        led(ui, pos2(row.cell(0).center().x, led_mid(ui, row.rect.center().y, &body(14.0), false)), lit);
        name_cell(ui, row, 1, &printable(&p.name, NAME_MAX), pal);
        cell_tipped(ui, row, 2, &status, &status, body(13.0), ink(row, color, pal), Id::new("fed-status-tip").with(i));
        let (seen, color) = match p.last_seen.as_deref() {
            Some(ts) => (age_of(Some(ts), now), pal.text),
            None => (t!("fed.never").to_string(), pal.dim),
        };
        cell(ui, row, 3, &seen, mono(12.5), ink(row, color, pal), false);
        // The cells register the flip on the box, but the row's select after
        // it, which wins the hit: a click on the box selects its row, and
        // `s` flips it.
        let at = row.cell(4);
        if checkbox(ui, Id::new("fed-disc").with(i), pos2(at.min.x + 8.0, at.center().y), p.use_discovery, pal).clicked() {
            picked = Some(i);
        }
    });
    if let Some(i) = shown.row.or(picked) {
        got.acts.push(Act::Select(i));
    }
}

// ── The note line ─────────────────────────────────────────────────────────

/// The room's foot: what it is busy with in the accent, else its note; with
/// neither, the cursor row's story — or, on Requests with no cursor, the
/// full inbox in gold.
fn note(ui: &Ui, rect: Rect, room: &Room, pal: &Pal) {
    let parts: Vec<Run> = match story(room, pal) {
        Some((words, figure, color)) => with_figures(&words, &[figure.as_deref().unwrap_or("")], body(13.0), mono(12.0), color),
        None => Vec::new(),
    };
    foot_line(ui, rect, room.busy.as_deref(), room.note.as_ref(), &parts, pal);
}

/// The cursor row's whole story in the cells' words, the short endpoint id
/// it names (drawn in figures), and its colour.
fn story(room: &Room, pal: &Pal) -> Option<(String, Option<String>, Color32)> {
    match room.tab {
        Tab::Requests => {
            if let Some(r) = room.selected_request() {
                let short = short_id(&r.peer_endpoint_id);
                let mut words = format!("{} · {} · {}", request_name(r), short, request_offer(r));
                if let Some(reason) = r.reject_reason.as_deref().filter(|s| !s.trim().is_empty()) {
                    words.push_str(&format!(" · \u{201c}{}\u{201d}", printable(reason, 200)));
                }
                if r.direction == "in" && r.state == "rejected" {
                    words.push_str(&t!("fed.ignored_week"));
                }
                Some((words, Some(short), pal.text))
            } else if room.accept_requests && room.pending_inbound() >= INBOX_FULL {
                Some((t!("fed.inbox_full").to_string(), None, pal.gold))
            } else {
                None
            }
        }
        Tab::Tickets => {
            let k = room.selected_key()?;
            let (claimed, figure) = match &k.bound_endpoint_id {
                Some(id) => {
                    let short = short_id(id);
                    (t!("fed.claimed_by", id = short, age = age_of(k.bound_at.as_deref(), unix_now())).to_string(), Some(short))
                }
                None => (t!("fed.not_claimed_yet").to_string(), None),
            };
            let words =
                format!("{} · {} · {} · {}", printable(&k.name, NAME_MAX), k.library_names.join(", "), expiry_words(k), claimed);
            Some((words, figure, pal.text))
        }
        Tab::Peers => {
            let p = room.selected_peer()?;
            let name = printable(&p.name, NAME_MAX);
            let words = match room.shares.get(&p.id) {
                Some(libs) if !libs.is_empty() => t!("fed.peer_shares", name = name, libraries = libs.join(", ")),
                Some(_) => t!("fed.peer_shares", name = name, libraries = t!("fed.shares_nothing")),
                None => t!("fed.peer_untested_note", name = name),
            };
            Some((words.to_string(), None, pal.text))
        }
    }
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The mint, accept and limits forms, one drawing, as the canvas draws the
/// mint: the name (mint) or the request's words (accept) or the limits'
/// note (limits); the libraries as tick boxes in three columns (mint,
/// accept); the limits' fields in a row of four; the expiry (limits) or
/// their offer (accept); the gold error line; the one button. A click on a
/// field focuses it and a click on a box flips it and focuses it, the
/// cells' acts; the close button cancels, as theirs does.
fn form(ui: &Ui, f: &Form, over: Rect, pal: &Pal, got: &mut Gathered) {
    let title = match f.kind {
        FormKind::Mint => t!("fed.mint_title"),
        FormKind::Accept => t!("fed.accept_title", name = f.about),
        FormKind::Limits => t!("fed.limits_title", name = f.about),
    };
    let fields = f.fields();
    let focused = f.focused();
    let index_of = |field: Field| fields.iter().position(|x| *x == field).unwrap_or(0);
    let composition = got.composition;
    // The focused field, its label with it, is brought into view at once on
    // the pass the focus moved to it, where the form is taller than the
    // room; the form opens at its top.
    let form_id = Id::new("fed-form");
    let shown = Dialog::new(form_id, over, FORM_W, &title).show(ui, pal, |ui| {
        let mut out = Out::default();
        let w = ui.available_width();
        // A text field's click focuses it, and its caret is the pass's
        // while the form's focus is on it; `whole` is the field with its
        // label.
        let take = |ui: &Ui, out: &mut Out, typed: Typed, field: Field, whole: Rect| {
            if typed.response.clicked() {
                out.acts.push(Act::FormFocus(index_of(field)));
            }
            if focused == field {
                out.caret = typed.caret;
                reveal(ui, form_id, f.focus, whole);
            }
        };
        match f.kind {
            FormKind::Mint => {
                let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_ROW_H), Sense::hover());
                let rect = Rect::from_min_size(row.min, vec2(320.0_f32.min(w), FIELD_ROW_H));
                let input = studio::Input::of(&f.name).focused(focused == Field::Name, composition);
                let id = Id::new("fed-form-field").with(index_of(Field::Name));
                let shown = section_field(ui, id, rect, &t!("fed.mint_name"), &input, pal);
                take(ui, &mut out, shown, Field::Name, rect);
            }
            FormKind::Accept => {
                // The id in the figures, the separator in the words' face.
                let said = if f.message.is_empty() {
                    (t!("fed.no_message").to_string(), body(14.0), pal.dim)
                } else {
                    (format!("\u{201c}{}\u{201d}", f.message), body(14.0), pal.text)
                };
                let parts = [(short_id(&f.endpoint_id), mono(12.5), pal.dim), (" · ".to_string(), body(14.0), pal.dim), said];
                let (row, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
                studio::runs(ui, row.left_center(), &parts, w, Align2::LEFT_CENTER);
            }
            FormKind::Limits => {
                Para::new(&t!("fed.limits_note"), body(13.5), pal.dim).show(ui);
            }
        }
        ui.add_space(SECTION);

        if f.kind != FormKind::Limits {
            let head = if f.kind == FormKind::Mint { t!("fed.mint_libraries") } else { t!("fed.accept_libraries") };
            let (row, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
            section(ui, row.left_center(), &head, None, w, pal);
            ui.add_space(6.0);
            if f.libraries.is_empty() {
                let (row, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
                text(ui, row.left_center(), &t!("fed.no_libraries"), body(14.0), pal.dim, w, Align2::LEFT_CENTER);
            } else {
                // The kit's grid of ticks, the mint board's three columns.
                let first = index_of(Field::Library(0));
                let here = match focused {
                    Field::Library(i) => Some(i),
                    _ => None,
                };
                let ticks = tick_grid(ui, Id::new("fed-form-lib"), &f.libraries, here, 3, pal);
                if let Some(rect) = here.and_then(|i| ticks.rects.get(i)) {
                    reveal(ui, form_id, f.focus, *rect);
                }
                if let Some(i) = ticks.clicked {
                    out.acts.push(Act::FormToggle(first + i));
                }
            }
            ui.add_space(SECTION);
        }

        let (row, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
        section(ui, row.left_center(), &t!("fed.limits_label"), None, w, pal);
        ui.add_space(10.0);
        let col_w = (w - 3.0 * FORM_GAP) / 4.0;
        let mut limits = vec![
            (Field::StreamKbps, t!("fed.field_kbps"), &f.stream_kbps),
            (Field::DailyMb, t!("fed.field_daily"), &f.daily_mb),
            (Field::MaxStreams, t!("fed.field_streams"), &f.max_streams),
        ];
        if f.kind != FormKind::Limits {
            limits.push((Field::Expires, t!("fed.field_expires"), &f.expires));
        }
        let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_ROW_H), Sense::hover());
        for (k, (field, label, value)) in limits.into_iter().enumerate() {
            let rect = Rect::from_min_size(pos2(row.min.x + k as f32 * (col_w + FORM_GAP), row.min.y), vec2(col_w, FIELD_ROW_H));
            let input = studio::Input::of(value).focused(focused == field, composition).mono();
            let shown = field_at(ui, Id::new("fed-form-field").with(index_of(field)), rect, &label, &input, pal);
            take(ui, &mut out, shown, field, rect);
        }

        match f.kind {
            FormKind::Limits => {
                ui.add_space(SECTION);
                let (row, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
                section(ui, row.left_center(), &t!("fed.expiry_current"), Some(&f.current_expiry), w, pal);
                ui.add_space(10.0);
                let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_ROW_H), Sense::hover());
                let rect = Rect::from_min_size(row.min, vec2(col_w, FIELD_ROW_H));
                let input = studio::Input::of(&f.expires).focused(focused == Field::Expires, composition).mono();
                let id = Id::new("fed-form-field").with(index_of(Field::Expires));
                let shown = field_at(ui, id, rect, &t!("fed.field_new_expiry"), &input, pal);
                take(ui, &mut out, shown, Field::Expires, rect);
                let x = rect.max.x + FORM_GAP;
                let mid = rect.max.y - FIELD_H / 2.0;
                text(ui, pos2(x, mid), &t!("fed.expiry_hint"), body(13.0), pal.dim, row.max.x - x, Align2::LEFT_CENTER);
            }
            FormKind::Accept if !f.their_offer.is_empty() => {
                ui.add_space(SECTION);
                let (rect, _) = ui.allocate_exact_size(vec2(w, TICK_H), Sense::hover());
                let words = t!("fed.accept_their_offer", libraries = f.their_offer.join(", "));
                let here = focused == Field::TheirOffer;
                let response = tick_row(ui, Id::new("fed-form-offer"), rect, f.accept_their_offer, &words, here, pal);
                if here {
                    reveal(ui, form_id, f.focus, rect);
                }
                if response.clicked() {
                    out.acts.push(Act::FormToggle(index_of(Field::TheirOffer)));
                }
            }
            _ => {}
        }

        ui.add_space(SECTION);
        studio::line(ui, 18.0, f.error.as_deref().unwrap_or(""), body(13.0), pal.gold);
        ui.add_space(8.0);
        let submit = match f.kind {
            FormKind::Mint => t!("fed.mint_submit"),
            FormKind::Accept => t!("fed.accept_submit"),
            FormKind::Limits => t!("fed.limits_submit"),
        };
        // Lit while the form could be sent; an outline before, which still
        // takes the click and shows why not.
        if foot(ui, Id::new("fed-form-buttons"), &[], &[(&submit, f.can_submit())], None, pal) == Some(0) {
            out.acts.push(Act::FormSubmit);
        }
        out
    });
    if shown.closed {
        got.acts.push(Act::FormCancel);
    }
    got.acts.extend(shown.inner.acts);
    got.caret = shown.inner.caret;
}

/// The fresh ticket to hand over: the sentence, the ticket itself in
/// figures on the ground (or, with none, the gold reason), the claim's
/// note, and the copy and Done buttons.
#[allow(clippy::too_many_arguments)]
fn minted(ui: &Ui, name: &str, ticket: Option<&str>, libraries: &[String], over: Rect, pal: &Pal, got: &mut Gathered) {
    let title = t!("fed.minted_title", name = name);
    let shown = Dialog::new(Id::new("fed-minted"), over, FORM_W, &title).show(ui, pal, |ui| {
        Para::new(&t!("fed.minted_text", libraries = libraries.join(", ")), body(14.0), pal.text).show(ui);
        ui.add_space(SECTION);
        match ticket {
            Some(ticket) => {
                // In figures on the ground in a strong-edged box, 12 points
                // in, broken anywhere: a ticket has no spaces.
                let w = ui.available_width();
                let back = ui.painter().add(egui::Shape::Noop);
                let at = ui.cursor().min + vec2(12.0, 12.0);
                let drawn = Para::new(&printable(ticket, 4096), mono(12.5), pal.text).anywhere().paint(ui, at, w - 24.0);
                let (rect, _) = ui.allocate_exact_size(vec2(w, drawn.height() + 24.0), Sense::hover());
                ui.painter().set(back, egui::Shape::rect_filled(rect, studio::square(), pal.ground));
                ui.painter().rect_stroke(rect, studio::square(), egui::Stroke::new(1.0, pal.edge_strong), egui::StrokeKind::Inside);
            }
            None => {
                Para::new(&t!("fed.minted_no_ticket"), body(14.0), pal.gold).show(ui);
            }
        }
        ui.add_space(SECTION);
        Para::new(&t!("fed.minted_note"), body(13.0), pal.dim).show(ui);
        ui.add_space(SECTION);
        let copy = t!("fed.minted_copy");
        let (copy, _) = unkeyed(&copy);
        let done = t!("fed.minted_done");
        match ticket {
            Some(_) => match foot(ui, Id::new("fed-minted-buttons"), &[], &[(copy, false), (&done, true)], None, pal) {
                Some(0) => Some(Act::MintedCopy),
                Some(_) => Some(Act::MintedDone),
                None => None,
            },
            None => foot(ui, Id::new("fed-minted-buttons"), &[], &[(&done, true)], None, pal).map(|_| Act::MintedDone),
        }
    });
    if shown.closed {
        got.acts.push(Act::MintedDone);
    }
    got.acts.extend(shown.inner);
}

/// The pasted ticket's preview and the peer's name, then Add. The field
/// always has the keyboard; a click on it adds, as the cells register it.
fn peer_name(ui: &Ui, name: &tui_input::Input, preview: &TicketPreview, over: Rect, pal: &Pal, got: &mut Gathered) {
    let composition = got.composition;
    let shown = Dialog::new(Id::new("fed-peer-name"), over, FORM_W, &t!("fed.peer_name_title")).show(ui, pal, |ui| {
        let mut out = Out::default();
        Para::new(&preview_words(preview), body(14.0), pal.ok).show(ui);
        ui.add_space(SECTION);
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_ROW_H), Sense::hover());
        let rect = Rect::from_min_size(row.min, vec2(400.0_f32.min(w), FIELD_ROW_H));
        let input = studio::Input::of(name).focused(true, composition);
        let typed = section_field(ui, Id::new("fed-peer-name-field"), rect, &t!("fed.peer_name_label"), &input, pal);
        if typed.response.clicked() {
            out.acts.push(Act::PeerNameAdd);
        }
        out.caret = typed.caret;
        ui.add_space(SECTION);
        if foot(ui, Id::new("fed-peer-name-buttons"), &[], &[(&t!("fed.peer_name_add"), true)], None, pal) == Some(0) {
            out.acts.push(Act::PeerNameAdd);
        }
        out
    });
    if shown.closed {
        got.acts.push(Act::PeerNameCancel);
    }
    got.acts.extend(shown.inner.acts);
    got.caret = shown.inner.caret;
}

/// A warning gate, the kit's: its consequences in gold before its verbs,
/// the safe choice lit and the destructive one an outline.
#[allow(clippy::too_many_arguments)]
fn warn(ui: &Ui, id: Id, over: Rect, title: &str, lines: &[String], safe: (String, Act), go: (String, Act), pal: &Pal, got: &mut Gathered) {
    let sentences = lines.iter().map(|line| Para::new(line, body(14.0), pal.gold)).collect();
    match gate(ui, id, over, title, sentences, [&safe.0, &go.0], pal) {
        Some(true) => got.acts.push(go.1),
        Some(false) => got.acts.push(safe.1),
        None => {}
    }
}

// ── Pieces the kit does not have ──────────────────────────────────────────

/// A form's section head, as the canvas's mint form sets one: the desk's
/// capitals at 11 points up to its first `·`, the rest dim in the UI face
/// 8 points after it (the canvas's "LIMITS · 0 = unlimited …"), and `tail`
/// after that in the text colour.
fn section(ui: &Ui, pos: Pos2, s: &str, tail: Option<&str>, max_w: f32, pal: &Pal) {
    let (first, rest) = match s.find('·') {
        Some(at) => (s[..at].trim_end(), &s[at..]),
        None => (s, ""),
    };
    let end = pos.x + max_w;
    let mut drawn = caps(ui, pos, first, 11.0, pal.dim, max_w, Align2::LEFT_CENTER);
    if !rest.is_empty() && end - drawn.max.x > 16.0 {
        let x = drawn.max.x + 8.0;
        drawn = text(ui, pos2(x, pos.y), rest, body(13.0), pal.dim, end - x, Align2::LEFT_CENTER);
    }
    if let Some(tail) = tail
        && end - drawn.max.x > 16.0
    {
        text(ui, pos2(drawn.max.x, pos.y), tail, body(13.0), pal.text, end - drawn.max.x, Align2::LEFT_CENTER);
    }
}

/// A field under a [`section`] head, in `slot` (a [`FIELD_ROW_H`] rect): the
/// mint's name, the peer's.
fn section_field(ui: &Ui, id: Id, slot: Rect, label: &str, input: &studio::Input, pal: &Pal) -> Typed {
    section(ui, pos2(slot.min.x, slot.min.y + 7.0), label, None, slot.width(), pal);
    let at = Rect::from_min_size(pos2(slot.min.x, slot.max.y - FIELD_H), vec2(slot.width(), FIELD_H));
    field_box(ui, id, at, input, pal)
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{Event, Modifiers, PointerButton, Vec2};

    use super::super::tests::{english, loaded, new_room, on, params_off, params_on, press, ticket_for, type_text};
    use super::*;
    use crate::kit::clipboard::{Copied, catch, caught};

    /// The room's region in the default window, about, and in a large one.
    const SMALL: Vec2 = vec2(640.0, 420.0);
    const LARGE: Vec2 = vec2(1180.0, 800.0);

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        ctx
    }

    /// What a run told the host, and every text it drew with where, and in
    /// what colour it began.
    struct Seen {
        caret: Option<Pos2>,
        outcome: Option<Outcome>,
        texts: Vec<(String, Rect)>,
        inks: Vec<Color32>,
    }

    impl Seen {
        /// The colour the first text holding `words` was drawn in.
        fn ink(&self, words: &str) -> Color32 {
            let found = self.texts.iter().position(|(drawn, _)| drawn.contains(words)).map(|i| self.inks[i]);
            found.unwrap_or_else(|| panic!("{words:?} drawn: {:?}", self.drawn()))
        }

        fn has(&self, words: &str) -> bool {
            self.texts.iter().any(|(drawn, _)| drawn.contains(words))
        }

        /// Where `words` were drawn, a text of their own.
        fn at(&self, words: &str) -> Rect {
            let found = self.texts.iter().find(|(drawn, _)| drawn == words).map(|(_, at)| *at);
            found.unwrap_or_else(|| panic!("{words:?} drawn: {:?}", self.drawn()))
        }

        fn drawn(&self) -> Vec<&str> {
            self.texts.iter().map(|(drawn, _)| drawn.as_str()).collect()
        }

        fn shows(&self, words: &[&str]) {
            for w in words {
                assert!(self.has(w), "{w:?} drawn: {:?}", self.drawn());
            }
        }
    }

    /// One run of egui (two passes when a modal takes its height) over a
    /// region of `size` with `events`, the input method composing
    /// `composing`.
    fn pass(ctx: &egui::Context, room: &mut Room, size: Vec2, events: Vec<Event>, composing: &str) -> Seen {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
        let (mut caret, mut outcome) = (None, None);
        let mut out = ctx.run_ui(input, |ui| {
            // The host styles egui and paints the ground first.
            let _ = studio::begin(ui);
            let shown = show(ui, room, composing);
            caret = shown.caret;
            outcome = outcome.or(shown.outcome);
        });
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        let (texts, inks) = out
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) => {
                    let ink = t.galley.job.sections.first().map_or(t.fallback_color, |s| s.format.color);
                    Some(((t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size())), ink))
                }
                _ => None,
            })
            .unzip();
        Seen { caret, outcome, texts, inks }
    }

    /// Two quiet runs, the second's: egui sizes some things on the first.
    fn settle(ctx: &egui::Context, room: &mut Room, size: Vec2) -> Seen {
        let _ = pass(ctx, room, size, Vec::new(), "");
        pass(ctx, room, size, Vec::new(), "")
    }

    /// A press and a release at `at`, a run each after the move, as hands
    /// would make them: whether any ended the room.
    fn click(ctx: &egui::Context, room: &mut Room, size: Vec2, at: Pos2) -> Option<Outcome> {
        let button =
            |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
        let mut outcome = None;
        for events in [vec![Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
            outcome = outcome.or(pass(ctx, room, size, events, "").outcome);
        }
        outcome
    }

    /// Where the last run drew the control `id`.
    fn rect_of(ctx: &egui::Context, id: Id) -> Rect {
        ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} drawn")).rect
    }

    /// A click on the control `id`, where the last run drew it.
    fn click_on(ctx: &egui::Context, room: &mut Room, size: Vec2, id: Id) -> Option<Outcome> {
        let at = rect_of(ctx, id).center();
        click(ctx, room, size, at)
    }

    fn form_of(room: &Room) -> &Form {
        match &room.modal {
            Modal::Form(f) => f,
            other => panic!("a form, not {other:?}"),
        }
    }

    #[test]
    fn every_state_draws_its_words_at_the_default_window_and_a_large_one() {
        let _en = english();
        for size in [SMALL, LARGE] {
            let ctx = context();
            // Loading: the body empty, the busy line along the foot.
            let mut room = new_room();
            room.reload(true);
            let seen = settle(&ctx, &mut room, size);
            seen.shows(&["loading federation…"]);
            assert!(!seen.has("TICKETS") && !seen.has("Turn federation on"), "{:?}", seen.drawn());
            // A load that failed: its gold note.
            room.busy = None;
            room.queued = None;
            room.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
            settle(&ctx, &mut room, size).shows(&["locked"]);
            // Off: the state line, the card, the pitch.
            room.note = None;
            room.apply(loaded(params_off()));
            let seen = settle(&ctx, &mut room, size);
            seen.shows(&["off — federation is not running", "Turn federation on", "read-only", "TICKETS ARE CREDENTIALS", "send tickets over a private channel"]);
            assert!(!seen.has("• off"), "the bullet is the LED");
            // Starting: the card dim, the busy line.
            room.act(Act::TurnOn);
            settle(&ctx, &mut room, size).shows(&["Starting federation…", "starting the federation endpoint…"]);
            // Unavailable: gold, and no card.
            room.starting = false;
            room.busy = None;
            room.queued = None;
            let mut unavailable = params_off();
            unavailable.available = false;
            room.apply(loaded(unavailable));
            let seen = settle(&ctx, &mut room, size);
            seen.shows(&["unavailable — Iroh has no prebuilt binary"]);
            assert!(!seen.has("Turn federation on"));

            // On: the state line, the tabs, the Requests tab.
            let mut room = on();
            let seen = settle(&ctx, &mut room, size);
            seen.shows(&[
                "on — connected to relay · endpoint 51403e7b9c51… · inbox open",
                "REQUESTS · 1",
                "TICKETS",
                "PEERS",
                "no access changes hands until a request is accepted",
                "Accept requests from the discovery network",
                "Off by default",
                "SERVER",
                "MESSAGE / OFFER",
                "STATUS",
                "AGE",
                "friend-node",
                "\u{201c}Jazz for field recordings?\u{201d} · offers: ambient",
                "needs your answer",
                "studio-nas",
                "waiting on them",
                "sending… · retry #2",
            ]);
            if size == LARGE {
                seen.shows(&["polls every 30 s", "federated", "declined"]);
            }
            // Connecting: gold, and the strip with its note.
            let mut connecting = params_on();
            connecting.online = false;
            room.apply(loaded(connecting));
            settle(&ctx, &mut room, size).shows(&["on — connecting to the relay…", "the endpoint is coming up"]);

            room.act(Act::Tab(Tab::Tickets));
            let seen = settle(&ctx, &mut room, size);
            seen.shows(&[
                "Mint a ticket for a friend",
                "NAME",
                "LIBRARIES",
                "LIMITS",
                "TODAY",
                "LAST USED",
                "CLAIMED",
                "Ada's laptop",
                "8 Mbps · 2 GB/day · 3 streams",
                "218 MB",
                "unlimited",
                "not yet",
            ]);
            assert!(seen.texts.iter().any(|(t, _)| t == "claimed"), "the tick is the LED: {:?}", seen.drawn());
            if size == LARGE {
                seen.shows(&["expired", "4.1 MB"]);
            }

            room.act(Act::Tab(Tab::Peers));
            let seen = settle(&ctx, &mut room, size);
            seen.shows(&[
                "ADD A PEER",
                "paste a friend's federation ticket",
                "DISCOVERY",
                "LAST SEEN",
                "Basement Archive",
                "ok",
                "dial timed out after 20 s",
                "never tested",
            ]);

            // Each table empty: its headings and the cells' sentence.
            room.peers.clear();
            settle(&ctx, &mut room, size).shows(&["(no peers yet — paste a friend's ticket above)"]);
            room.act(Act::Tab(Tab::Tickets));
            room.keys.clear();
            settle(&ctx, &mut room, size).shows(&["(no tickets yet — n mints one)"]);
            room.act(Act::Tab(Tab::Requests));
            room.requests.clear();
            settle(&ctx, &mut room, size).shows(&["(no requests yet — servers on the discovery network can find you here)"]);
            // The note and the busy line along the foot, the busy line first.
            room.note = Some(("the relay answered".into(), true));
            settle(&ctx, &mut room, size).shows(&["the relay answered"]);
            room.busy = Some(t!("fed.busy_saving").to_string());
            let seen = settle(&ctx, &mut room, size);
            assert!(seen.has("saving…") && !seen.has("the relay answered"), "{:?}", seen.drawn());
        }
    }

    #[test]
    fn a_click_on_a_tab_or_a_row_is_the_cells_act_and_the_rows_story_rides_the_foot() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let _ = settle(&ctx, &mut room, SMALL);
        assert_eq!(click_on(&ctx, &mut room, SMALL, Id::new("fed-tabs").with(1)), None, "no click ends the room");
        assert_eq!(room.tab, Tab::Tickets);
        let seen = settle(&ctx, &mut room, SMALL);
        assert_eq!(click(&ctx, &mut room, SMALL, seen.at("studio-nas").center()), None);
        assert_eq!(room.sel, Some(1), "a click on a row selects it");
        settle(&ctx, &mut room, SMALL).shows(&["studio-nas · music, vinyl, jazz · expires in 24 days · not yet claimed"]);

        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-tabs").with(0));
        assert_eq!((room.tab, room.sel), (Tab::Requests, None), "a tab switch puts the cursor away");
        let seen = settle(&ctx, &mut room, SMALL);
        // The offer is cut in the default window, and carries its tooltip:
        // a click on it is still the row's.
        let _ = click(&ctx, &mut room, SMALL, seen.at("\u{201c}Jazz for field recordings?\u{201d} · offers: ambient").center());
        assert_eq!(room.sel, Some(0), "a click on a cut cell selects its row");
        room.sel = None;
        let seen = settle(&ctx, &mut room, SMALL);
        let _ = click(&ctx, &mut room, SMALL, seen.at("friend-node").center());
        assert_eq!(room.sel, Some(0));
        let seen = settle(&ctx, &mut room, SMALL);
        seen.shows(&["friend-node · c4a8f0e19dc4… · \u{201c}Jazz for field recordings?\u{201d} · offers: ambient"]);
        // The keys still walk the rows: the room's own model, whatever draws it.
        press(&mut room, KeyCode::Down);
        assert_eq!(room.sel, Some(1));
    }

    #[test]
    fn the_cards_and_the_paste_box_apply_the_cells_acts() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let _ = settle(&ctx, &mut room, SMALL);
        assert_eq!(click_on(&ctx, &mut room, SMALL, Id::new("fed-inbox")), None);
        assert_eq!(room.queued, Some(Op::SetInbox(false)), "the card flips the inbox");
        room.queued = None;
        room.busy = None;

        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-tabs").with(1));
        let _ = settle(&ctx, &mut room, SMALL);
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-mint"));
        assert_eq!(form_of(&room).kind, FormKind::Mint, "the card opens the mint form");
        room.modal = Modal::None;

        let _ = settle(&ctx, &mut room, SMALL);
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-tabs").with(2));
        press(&mut room, KeyCode::Down);
        let _ = settle(&ctx, &mut room, SMALL);
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-ticket"));
        assert!(room.ticket_focus && room.sel.is_none(), "the box takes the caret and puts the cursor away");

        let mut room = new_room();
        room.apply(loaded(params_off()));
        let _ = settle(&ctx, &mut room, SMALL);
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-turn-on"));
        assert_eq!(room.queued, Some(Op::SetEnabled(true)), "the card turns the endpoint on");
        assert!(room.starting);
        room.queued = None;
        let _ = settle(&ctx, &mut room, SMALL);
        // The room refuses a second start whatever asks: that the card
        // itself is deaf is in what it senses.
        let card = ctx.read_response(Id::new("fed-turn-on")).expect("the card drawn");
        assert!(!card.sense.senses_click(), "dim while the start is on its way, and deaf");
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-turn-on"));
        assert!(room.queued.is_none());
    }

    #[test]
    fn a_click_on_the_discovery_box_selects_its_row_as_in_the_cells() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        room.act(Act::Tab(Tab::Peers));
        let _ = settle(&ctx, &mut room, SMALL);
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-disc").with(1));
        assert_eq!(room.sel, Some(1), "the row's select wins the click, as in the cells");
        assert!(room.queued.is_none() && !room.peers[1].use_discovery, "the box is not flipped");
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::PeerDiscovery { id: 2, on: true }), "s flips it");
    }

    #[test]
    fn a_focused_field_says_where_its_caret_is_and_none_does_without_the_keyboard() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        assert_eq!(settle(&ctx, &mut room, SMALL).caret, None, "no field has the keyboard");

        press(&mut room, KeyCode::Char('j'));
        type_text(&mut room, "mstrfed1:abc");
        let seen = settle(&ctx, &mut room, SMALL);
        let caret = seen.caret.expect("the paste box has the keyboard");
        let field = rect_of(&ctx, Id::new("fed-ticket"));
        assert!(field.expand(1.0).contains(caret), "the caret's foot in the box: {caret:?} {field:?}");
        seen.shows(&["mstrfed1:abc", "that doesn't look like a federation ticket"]);
        let _ = pass(&ctx, &mut room, SMALL, Vec::new(), "かな");
        let composing = pass(&ctx, &mut room, SMALL, Vec::new(), "かな").caret.expect("still");
        assert!(composing.x > caret.x + 8.0, "the caret stands after the composition: {caret:?} {composing:?}");
        press(&mut room, KeyCode::Esc);
        assert_eq!(settle(&ctx, &mut room, SMALL).caret, None, "the box let go of the keys");

        room.act(Act::Mint);
        let caret = settle(&ctx, &mut room, SMALL).caret.expect("the name field has the keyboard");
        let name = rect_of(&ctx, Id::new("fed-form-field").with(0));
        assert!(name.expand(1.0).contains(caret), "{caret:?} {name:?}");
        press(&mut room, KeyCode::Tab);
        assert_eq!(settle(&ctx, &mut room, SMALL).caret, None, "a tick box has no caret");
        room.modal = Modal::TurnOff;
        assert_eq!(settle(&ctx, &mut room, SMALL).caret, None, "nor a gate");
    }

    #[test]
    fn the_forms_clicks_are_the_cells_acts() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        // In the large window: in the default one the form is taller than
        // the room, and its button is under the scroll's fold.
        room.act(Act::Mint);
        let _ = settle(&ctx, &mut room, LARGE);
        // A box flips and takes the focus; the fields follow the name and
        // the five libraries.
        let _ = click_on(&ctx, &mut room, LARGE, Id::new("fed-form-lib").with(0));
        let f = form_of(&room);
        assert_eq!((f.libraries[0].clone(), f.focus), (("music".to_string(), true), 1));
        let _ = click_on(&ctx, &mut room, LARGE, Id::new("fed-form-field").with(7));
        assert_eq!(form_of(&room).focused(), Field::DailyMb, "a click on a field focuses it");
        // Sent with no name: the form says why, and nothing goes.
        let _ = click_on(&ctx, &mut room, LARGE, Id::new("fed-form-buttons").with(0));
        assert_eq!(form_of(&room).error.as_deref(), Some("name it and tick at least one library"));
        assert!(room.queued.is_none());
        settle(&ctx, &mut room, LARGE).shows(&["name it and tick at least one library"]);
        let _ = click_on(&ctx, &mut room, LARGE, Id::new("fed-form-field").with(0));
        type_text(&mut room, "Bob's NAS");
        let _ = settle(&ctx, &mut room, LARGE);
        let _ = click_on(&ctx, &mut room, LARGE, Id::new("fed-form-buttons").with(0));
        assert_eq!(
            room.queued,
            Some(Op::Mint {
                name: "Bob's NAS".into(),
                vpaths: vec!["music".into()],
                limits: FederationLimits { stream_kbps: 8000, daily_mb: 2048, max_streams: 3 },
                expires_at: None,
            })
        );
        room.queued = None;
        room.busy = None;
        room.modal = Modal::None;

        // Accept: their offer is a box of its own, last in the ring.
        room.act(Act::Accept(1));
        let _ = settle(&ctx, &mut room, LARGE);
        let _ = click_on(&ctx, &mut room, LARGE, Id::new("fed-form-offer"));
        let f = form_of(&room);
        assert!(!f.accept_their_offer, "the click flips it");
        assert_eq!(f.focused(), Field::TheirOffer, "and focuses it");
    }

    #[test]
    fn a_form_taller_than_the_room_scrolls_its_focus_into_view() {
        let _en = english();
        let ctx = context();
        // A room shorter than the mint form: the expiry is under the fold.
        let short = vec2(640.0, 340.0);
        let mut room = on();
        room.act(Act::Mint);
        let _ = settle(&ctx, &mut room, short);
        let window_foot = short.y / 2.0 + (short.y - 32.0) / 2.0;
        let unscrolled = rect_of(&ctx, Id::new("fed-form-field").with(9));
        assert!(unscrolled.max.y > window_foot - 24.0, "the expiry starts out of view: {unscrolled:?}");
        for _ in 0..9 {
            press(&mut room, KeyCode::Tab);
        }
        assert_eq!(form_of(&room).focused(), Field::Expires);
        // The run that asks for the scroll draws the form unmoved; egui
        // lands it on the next, which draws it unmoved too unless run again,
        // and nothing else would come to wake the panel.
        let caret = settle(&ctx, &mut room, short).caret.expect("the expiry has the keyboard");
        assert!(caret.y <= window_foot - 24.0, "brought into view on the run after the ask: {caret:?}");
    }

    #[test]
    fn a_modal_opens_at_its_top_whatever_its_scroll_was_when_last_up() {
        let _en = english();
        let ctx = context();
        let short = vec2(640.0, 340.0);
        // Where the form's body starts: under the title row, whose middle
        // the close button stands on, and the kit's 20 points under it.
        let body_top = |ctx: &egui::Context| rect_of(ctx, Id::new("fed-form").with("close")).center().y + 20.0 / 2.0 + 20.0;
        let mut room = on();
        room.act(Act::Mint);
        let _ = settle(&ctx, &mut room, short);
        for _ in 0..9 {
            press(&mut room, KeyCode::Tab);
        }
        let seen = settle(&ctx, &mut room, short);
        assert!(seen.at("WHO IS THIS FOR?").max.y < body_top(&ctx), "scrolled down to the expiry, past the form's head");
        press(&mut room, KeyCode::Esc);
        let _ = settle(&ctx, &mut room, short);
        // Opened again while egui still holds the old offset: the new form
        // is at its top by the run after its first.
        room.act(Act::Mint);
        let seen = settle(&ctx, &mut room, short);
        let head = seen.at("WHO IS THIS FOR?");
        assert!(head.min.y >= body_top(&ctx) - 1.0, "the head in view: {head:?}, the body from {}", body_top(&ctx));
        // Moving back up, a field comes into view with its label.
        for _ in 0..9 {
            press(&mut room, KeyCode::Tab);
        }
        let _ = settle(&ctx, &mut room, short);
        for _ in 0..9 {
            press(&mut room, KeyCode::BackTab);
        }
        let seen = settle(&ctx, &mut room, short);
        assert_eq!(form_of(&room).focused(), Field::Name);
        assert!(seen.at("WHO IS THIS FOR?").min.y >= body_top(&ctx) - 1.0, "the name's label shows with it");
    }

    #[test]
    fn each_modal_draws_its_words_and_its_close_works() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let close = |modal: &str| Id::new(modal).with("close");

        room.act(Act::Mint);
        settle(&ctx, &mut room, SMALL).shows(&[
            "New federation ticket",
            "WHO IS THIS FOR?",
            "LIBRARIES THIS TICKET CAN READ",
            "music",
            "field-recordings",
            "LIMITS",
            "· 0 = unlimited · the config's defaults prefilled",
            "STREAM KBPS",
            "8000",
            "EXPIRES (DAYS)",
            "MINT TICKET",
        ]);
        assert_eq!(click_on(&ctx, &mut room, SMALL, close("fed-form")), None);
        assert!(matches!(room.modal, Modal::None), "the close cancels");

        room.act(Act::Accept(1));
        settle(&ctx, &mut room, SMALL).shows(&[
            "Accept — share libraries with friend-node",
            "c4a8f0e19dc4… · \u{201c}Jazz for field recordings?\u{201d}",
            "LIBRARIES THEY CAN READ",
            "Add their libraries too (ambient) when they share back",
            "ACCEPT & SEND TICKET",
        ]);
        let _ = click_on(&ctx, &mut room, SMALL, close("fed-form"));
        assert!(matches!(room.modal, Modal::None));

        room.act(Act::Limits(1));
        settle(&ctx, &mut room, SMALL).shows(&[
            "Limits for Ada's laptop",
            "Applies from this key's next request",
            "EXPIRY",
            "never expires",
            "NEW EXPIRY",
            "days from now · blank keeps it · 0 = never",
            "SAVE",
        ]);
        let _ = click_on(&ctx, &mut room, SMALL, close("fed-form"));
        assert!(matches!(room.modal, Modal::None));

        // The minted ticket: copy and Done are the cells' acts.
        let ticket = "mstrfed1:eyJ0IjoibmV3In0".to_string();
        let minted = Modal::Minted { name: "Bob's NAS".into(), ticket: Some(ticket.clone()), libraries: vec!["music".into()] };
        room.modal = minted.clone();
        settle(&ctx, &mut room, SMALL).shows(&["Ticket for Bob's NAS", "can read music until", &ticket, "The first server to use it", "COPY", "DONE"]);
        catch(Copied::Clipboard);
        let _ = caught();
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-minted-buttons").with(0));
        assert_eq!(caught(), [ticket.clone()]);
        assert_eq!(room.note, Some((t!("fed.copied_clipboard").to_string(), false)));
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-minted-buttons").with(1));
        assert!(matches!(room.modal, Modal::None), "Done");
        room.modal = minted;
        let _ = settle(&ctx, &mut room, SMALL);
        let _ = click_on(&ctx, &mut room, SMALL, close("fed-minted"));
        assert!(matches!(room.modal, Modal::None));
        room.modal = Modal::Minted { name: "x".into(), ticket: None, libraries: vec!["music".into()] };
        let seen = settle(&ctx, &mut room, SMALL);
        seen.shows(&["the key was minted, but no ticket"]);
        assert!(!seen.has("COPY"), "nothing to copy");
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-minted-buttons").with(0));
        assert!(matches!(room.modal, Modal::None), "Done alone");

        // Naming the peer: Add, the field (the cells register Add on it
        // too), and the close.
        let pasted = ticket_for(serde_json::json!({ "t": "node", "k": "key", "n": "mStream Den", "l": ["music", "vinyl"], "e": "2026-12-01T00:00:00.000Z" }));
        press(&mut room, KeyCode::Char('j'));
        type_text(&mut room, &pasted);
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::PeerName { .. }));
        let seen = settle(&ctx, &mut room, SMALL);
        seen.shows(&["Name this peer", "mStream Den — shares music, vinyl · valid until 2026-12-01", "NAME", "· as you'll see it here", "ADD PEER"]);
        assert!(seen.caret.is_some(), "the name always has the keyboard");
        let added = Some(Op::AddPeer { ticket: pasted.clone(), name: Some("mStream Den".into()) });
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-peer-name-buttons").with(0));
        assert_eq!(room.queued, added);
        room.queued = None;
        let _ = click_on(&ctx, &mut room, SMALL, Id::new("fed-peer-name-field"));
        assert_eq!(room.queued, added);
        room.queued = None;
        room.busy = None;
        let _ = click_on(&ctx, &mut room, SMALL, close("fed-peer-name"));
        assert!(matches!(room.modal, Modal::None));
        assert!(room.queued.is_none());
    }

    #[test]
    fn a_gate_is_answered_keep_on_its_lit_button_or_esc_and_act_on_the_other() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let gates = [
            (Modal::Decline(1), "decline", "Decline friend-node's request?", "They aren't told why.", "KEEP", Op::Reject(1)),
            (Modal::Revoke(2), "revoke", "Revoke studio-nas's ticket?", "can no longer read music, vinyl, jazz", "KEEP", Op::Revoke(2)),
            (Modal::Forget(1), "forget", "Forget Basement Archive?", "Its bridge closes", "KEEP", Op::RemovePeer(1)),
            (Modal::TurnOff, "turn-off", "Turn federation off?", "Tickets keep their grants.", "STAY ON", Op::SetEnabled(false)),
        ];
        for (gate, kind, title, line, safe, op) in gates {
            let id = Id::new("fed-gate").with(kind);
            room.modal = gate.clone();
            let seen = settle(&ctx, &mut room, SMALL);
            seen.shows(&[title, line, safe]);
            assert!(!seen.has("◂"), "{kind}: the arrow is the button's");
            let gold = Pal::now().gold;
            assert_eq!((seen.ink(title), seen.ink(line)), (gold, gold), "{kind}: the gate's title and lines are gold, as the cells'");
            let _ = click_on(&ctx, &mut room, SMALL, id.with("foot").with(0));
            assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "{kind}: the lit button keeps");
            room.modal = gate.clone();
            let _ = settle(&ctx, &mut room, SMALL);
            assert!(ctx.read_response(id.with("close")).is_none(), "{kind}: a gate has no close, as the cells draw it");
            press(&mut room, KeyCode::Esc);
            assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "{kind}: Esc keeps");
            room.modal = gate;
            let _ = settle(&ctx, &mut room, SMALL);
            let _ = click_on(&ctx, &mut room, SMALL, id.with("foot").with(1));
            assert_eq!(room.queued, Some(op), "{kind}: the other acts");
            room.queued = None;
            room.busy = None;
            room.modal = Modal::None;
        }
        // Under a gate the room is inert.
        let _ = settle(&ctx, &mut room, SMALL);
        let card = rect_of(&ctx, Id::new("fed-inbox"));
        room.modal = Modal::TurnOff;
        let _ = settle(&ctx, &mut room, SMALL);
        let _ = click(&ctx, &mut room, SMALL, pos2(card.min.x + 6.0, card.center().y));
        assert!(matches!(room.modal, Modal::TurnOff) && room.queued.is_none(), "the scrim took the press");
    }
}
