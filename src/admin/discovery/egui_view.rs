//! The discovery room drawn by egui, in the desktop window's panel: the
//! room's second view, beside `cells` (cells.rs). The room is the parent
//! module (its state, its worker, its keys and every word it says); this
//! draws it after the Claude Design canvas of the Admin screen (the
//! Discovery board and "a server's sheet", 2026-11-20) in the Studio kit's
//! look (src/gui/window/studio.rs): the state row behind its LED, the tabs
//! as one segmented control over a pane that keeps its height, the servers
//! you follow as the kit's table, the note line at the foot, and the modals
//! over the room's area. Off the network it is the join page.
//!
//! A click here is the act the cells' click on the same thing is, applied
//! through the room's own [`Room::act`] once the pass is drawn, so the two
//! views cannot disagree about what the room does. A few controls the
//! canvas draws stand for a key the cells' words name (the hidden servers'
//! `h`, the blocked list's `u`, a gate's close for Esc): they hand the room
//! that key, as the keyboard would. Keys never reach egui: every field is
//! drawn from the room's own value and caret, which its key handler edits
//! as it always has, and the focused one says where its caret stands.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::epaint::{CornerRadius, StrokeKind};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Id, LayerId, Layout, Order, Pos2, Rect, Response, ScrollArea, Sense, Shape,
    Stroke, Ui, UiBuilder, pos2, vec2,
};

use super::super::{EguiShown, age_text, fmt_bytes, fmt_count, iso_unix, unix_now};
use super::*;
use crate::gui::window::studio::{
    self, Col, FIELD_H, FORM_BUTTON_H, Pal, RowCtx, SEG_H, Seg, Table, add_card, body, caps, caps_width, cell, check_line,
    field, field_box, form_button, form_button_width, ink, led, meter, modal, mono, note_line, ring, segmented,
    segmented_width, small_caps, small_caps_width, square, strong, text, width_of,
};
use crate::input::KeyModifiers;

/// The note line's height along the room's foot: the canvas's 18 points,
/// a full gap under the servers you follow.
const NOTE_H: f32 = 18.0;
/// Where a line's words start after the LED that leads it.
const LED_W: f32 = 14.0;
/// The table's headings and the room under them, as the kit draws them.
const HEAD_H: f32 = 34.0;
/// The filter's box, at its widest and its narrowest.
const FILTER_W: f32 = 220.0;
const FILTER_MIN: f32 = 120.0;
/// The column the canvas gives a label before its value (Invite, the sheet).
const LABEL_COL: f32 = 112.0;
const FACT_COL: f32 = 124.0;
/// The modals' widths: the canvas's sheet, and the blocked list's room for
/// a whole endpoint id.
const MODAL_W: f32 = 560.0;
const BLOCKED_W: f32 = 620.0;
/// The setting's number box, and the pitch's measure off the network.
const NUMBER_W: f32 = 200.0;
const PITCH_W: f32 = 760.0;
/// The Activity log's age column.
const AGE_W: f32 = 48.0;
/// From this tall a room wears the canvas's measures.
const ROOMY: f32 = 560.0;

/// The room's measures. The canvas's are for a room about as tall as its
/// board's; in a shorter one (the window's opening size) the same parts are
/// drawn tighter and the tabs ride the state row's right end, so the
/// servers table still has rows under the tab pane.
struct Look {
    compact: bool,
    /// The room's margin either side, above and below.
    side: f32,
    top: f32,
    bottom: f32,
    /// Between the room's parts.
    gap: f32,
    /// A readout's height on the Stats tab: the pane is two of them.
    tile_h: f32,
    /// A readout's padding, its label's size, its figure's and its detail's.
    tile_pad: f32,
    label: f32,
    figure: f32,
    detail: f32,
    /// A row of the servers table, a line of the Activity log.
    row_h: f32,
    line_h: f32,
    /// The section line, and the small buttons and boxes in the room's rows.
    section_h: f32,
    button_h: f32,
    box_h: f32,
}

impl Look {
    fn for_height(h: f32) -> Look {
        if h >= ROOMY {
            Look {
                compact: false,
                side: 24.0,
                top: 20.0,
                bottom: 14.0,
                gap: 18.0,
                tile_h: 82.0,
                tile_pad: 9.0,
                label: 11.5,
                figure: 24.0,
                detail: 12.0,
                row_h: 40.0,
                line_h: 20.0,
                section_h: 34.0,
                button_h: 28.0,
                box_h: FIELD_H,
            }
        } else {
            Look {
                compact: true,
                side: 16.0,
                top: 10.0,
                bottom: 8.0,
                gap: 10.0,
                tile_h: 62.0,
                tile_pad: 5.0,
                label: 10.5,
                figure: 18.0,
                detail: 11.0,
                row_h: 30.0,
                line_h: 17.0,
                section_h: 30.0,
                button_h: 24.0,
                box_h: 28.0,
            }
        }
    }

    /// The tab pane's height, whatever the tab: the Stats tab's two rows of
    /// readouts and the hairlines that join them.
    fn pane_h(&self) -> f32 {
        2.0 * self.tile_h + 3.0
    }
}

/// One thing a click asks of the room, applied once the pass is drawn.
enum Do {
    /// The act the cells' click on the same control is.
    Act(Act),
    /// A key, for a control the canvas draws in a key's stead (the cells'
    /// words name the key).
    Key(KeyCode),
}

/// What a pass gathers: what its clicks ask, in order, and the caret of
/// the field that has the keyboard.
#[derive(Default)]
struct Pass {
    does: Vec<Do>,
    caret: Option<Pos2>,
}

impl Pass {
    fn act(&mut self, act: Act) {
        self.does.push(Do::Act(act));
    }
}

/// What the room's fields draw: the input method's uncommitted text, for
/// the focused one only, and whether the room's own fields may have the
/// keyboard at all (not while a modal is over them).
#[derive(Clone, Copy)]
struct Typing<'a> {
    composition: &'a str,
    live: bool,
}

impl<'a> Typing<'a> {
    /// The composition, for a field that has the keyboard.
    fn for_field(self, focused: bool) -> &'a str {
        if focused { self.composition } else { "" }
    }
}

/// A run of a line: its words, its face and its colour.
type Run = (String, FontId, Color32);

/// One pass of the room over the whole of `ui`: the room's area less its
/// first row, which is the host's. The host has styled egui and painted the
/// ground. `composition` is the input method's uncommitted text, which only
/// the focused field draws; `key_hints` is whether a tooltip may name the
/// key that does the same. What it returns is the focused field's caret and
/// what a click's act said; no click here ends the room, as none in the
/// cells does.
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str, key_hints: bool) -> EguiShown {
    let pal = Pal::now();
    let full = ui.max_rect();
    let look = Look::for_height(full.height());
    let column =
        Rect::from_min_max(pos2(full.min.x + look.side, full.min.y + look.top), pos2(full.max.x - look.side, full.max.y - look.bottom));
    let foot = Rect::from_min_max(pos2(column.min.x, column.max.y - NOTE_H), column.max);
    let body_rect = Rect::from_min_max(column.min, pos2(column.max.x, foot.min.y - look.gap));

    // The sheet of a server that left the catalog closes, as the cells'
    // draw closes it.
    if let Modal::Sheet { id, .. } = &room.modal
        && room.peer(id).is_none()
    {
        room.modal = Modal::None;
    }
    let live = !room.modal_open();
    let typing = Typing { composition, live };
    let mut pass = Pass::default();
    let shown: &Room = room;
    let mut described = None;
    {
        let mut inner = ui.new_child(UiBuilder::new().id_salt("disc-body").max_rect(body_rect).layout(Layout::top_down(Align::Min)));
        inner.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
        match &shown.status {
            None => {}
            Some(s) if !s.enabled => off_network(&mut inner, shown, s, &look, &pal, &mut pass),
            Some(s) => described = on_network(&mut inner, shown, s, &look, &pal, typing, &mut pass),
        }
    }
    foot_line(ui, foot, shown, described.as_deref(), &pal);

    // A modal makes the room beneath inert: what was clicked under it this
    // pass is dropped, as the cells drop what the room registered, and no
    // field of the room's has the keyboard.
    if !live {
        pass = Pass::default();
    }
    modals(ui, shown, full, &pal, typing, key_hints, &mut pass);

    let mut outcome = None;
    for d in pass.does {
        let out = match d {
            Do::Act(act) => room.act(act),
            Do::Key(code) => room.key(KeyEvent::new(code, KeyModifiers::NONE)),
        };
        outcome = outcome.or(out);
    }
    EguiShown { caret: pass.caret, outcome }
}

// ── Text ──────────────────────────────────────────────────────────────────

/// A state word less its bullet: the canvas draws the bullet as the LED
/// before the word.
fn unbulleted(s: &str) -> &str {
    s.trim_start_matches('•').trim_start()
}

/// `parts` as one job, each run in its own face and colour.
fn run_job(parts: &[Run]) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (s, font, color) in parts {
        if !s.is_empty() {
            job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *color, ..Default::default() });
        }
    }
    job
}

/// How wide `parts` lay out in one line.
fn runs_width(ui: &Ui, parts: &[Run]) -> f32 {
    ui.painter().layout_job(run_job(parts)).size().x
}

/// One line of runs, each in its own face and colour (the state word in
/// its tone, the facts plain, the announced name in semibold), cut with an
/// ellipsis where it would pass `max_w`, anchored at `pos` by `align`.
/// Returns where it was drawn.
fn runs(ui: &Ui, pos: Pos2, parts: &[Run], max_w: f32, align: Align2) -> Rect {
    let mut job = run_job(parts);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.painter().layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, Color32::WHITE);
    rect
}

/// Runs laid out as one paragraph wrapped at `w`, a row of `ui`.
fn para(ui: &mut Ui, w: f32, parts: &[Run]) -> Rect {
    let mut job = run_job(parts);
    job.wrap.max_width = w.max(1.0);
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(rect.min, galley, Color32::WHITE);
    rect
}

/// One line of text, a row of `ui` `h` tall, cut to the row.
fn line(ui: &mut Ui, h: f32, s: &str, font: FontId, color: Color32) -> Rect {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    text(ui, row.left_center(), s, font, color, w, Align2::LEFT_CENTER)
}

/// A label in dim small capitals, a row of `ui` `h` tall.
fn label_row(ui: &mut Ui, h: f32, s: &str, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    small_caps(ui, row.left_center(), s, pal.dim, w, Align2::LEFT_CENTER);
}

/// A label in small capitals and, after it, its dim hint (which carries its
/// own " · "), both cut to `max_w` from `at`, the line's left middle.
fn head_line(ui: &Ui, at: Pos2, label: &str, hint: &str, max_w: f32, pal: &Pal) {
    let drawn = small_caps(ui, at, label, pal.dim, max_w, Align2::LEFT_CENTER);
    let room = at.x + max_w - drawn.max.x;
    if room > 20.0 && !hint.is_empty() {
        text(ui, pos2(drawn.max.x, at.y), hint, body(13.0), pal.dim, room, Align2::LEFT_CENTER);
    }
}

/// How wide a small button in a room's row is for `label`: its capitals in
/// the form button's size and the canvas's ten points either side.
fn small_button_width(ui: &Ui, label: &str) -> f32 {
    caps_width(ui, label, 11.5) + 22.0
}

// ── On the network ────────────────────────────────────────────────────────

/// The room on the network: the state row and the tabs, the tab's pane,
/// then the servers you follow. Returns the cursor row's server and its
/// description in words, for the foot, when the table drew it.
fn on_network(ui: &mut Ui, room: &Room, s: &DiscoveryStatus, look: &Look, pal: &Pal, typing: Typing, pass: &mut Pass) -> Option<String> {
    let w = ui.available_width();
    if look.compact {
        // The tabs at the state row's right end: one row, not two.
        let (row, _) = ui.allocate_exact_size(vec2(w, SEG_H), Sense::hover());
        let labels: Vec<String> = Tab::ALL.iter().map(|t| t.label()).collect();
        let tabs_w = segmented_width(ui, &labels);
        tabs(ui, row.max.x - tabs_w, row.center().y, room, pal, pass);
        state_line(ui, Rect::from_min_max(row.min, pos2(row.max.x - tabs_w - 16.0, row.max.y)), s, false, pal);
    } else {
        let (row, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
        state_line(ui, row, s, true, pal);
    }
    // The indeterminate bar: on, but no neighbour yet. All unlit, never a
    // share of anything.
    if s.binary_found && (!s.running || !s.joined || s.neighbors == 0) {
        ui.add_space(6.0);
        let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        let bar = meter(ui, row.left_center(), 10, 0, vec2(5.0, 10.0), 2.0, pal.accent, pal);
        let words = if recovering(s) { t!("p2p.recovering") } else { t!("p2p.searching") };
        text(ui, pos2(bar.max.x + 10.0, row.center().y), &words, body(13.0), pal.accent, row.max.x - bar.max.x - 10.0, Align2::LEFT_CENTER);
    }
    ui.add_space(look.gap);
    if !look.compact {
        let (row, _) = ui.allocate_exact_size(vec2(w, SEG_H), Sense::hover());
        let drawn = tabs(ui, row.min.x, row.center().y, room, pal, pass);
        if room.tab == Tab::Activity {
            let note = t!("p2p.activity_note").to_string();
            let room_w = row.max.x - drawn.max.x - 16.0;
            if width_of(ui, &note, body(13.0)) <= room_w {
                text(ui, row.right_center(), &note, body(13.0), pal.dim, room_w, Align2::RIGHT_CENTER);
            }
        }
        ui.add_space(look.gap);
    }
    let (pane, _) = ui.allocate_exact_size(vec2(w, look.pane_h()), Sense::hover());
    match room.tab {
        Tab::Stats => stats_pane(ui, pane, room, s, look, pal),
        Tab::Activity => activity_pane(ui, pane, room, look, pal),
        Tab::Invite => invite_pane(ui, pane, room, s, look, pal, typing, pass),
        Tab::Config => config_pane(ui, pane, room, s, look, pal, pass),
    }
    ui.add_space(look.gap);
    servers(ui, room, s, look, pal, typing, pass)
}

/// Crash recovery is replaying the stack, with no neighbour yet.
fn recovering(s: &DiscoveryStatus) -> bool {
    (s.recovery.attempts > 0 || s.recovery.retry_pending) && s.neighbors == 0
}

/// The state row in `row`: the state's LED and its word in its tone, then
/// the facts, the announced name in semibold, and (when `polls` and it
/// fits) the poll's cadence dim at the right. A narrow room drops the facts
/// from the end, as the cells do: the name first, then the sidecar's
/// figure; the state itself is only ever cut.
fn state_line(ui: &Ui, row: Rect, s: &DiscoveryStatus, polls: bool, pal: &Pal) {
    let (word, tone, detail) = if !s.binary_found {
        (t!("p2p.state_unavailable").to_string(), pal.gold, String::new())
    } else if s.neighbors > 0 {
        let d = if s.neighbors == 1 { t!("p2p.state_neighbors_one") } else { t!("p2p.state_neighbors", n = s.neighbors) };
        (t!("p2p.state_connected").to_string(), pal.ok, d.to_string())
    } else if recovering(s) {
        (t!("p2p.state_reconnecting").to_string(), pal.gold, t!("p2p.state_reconnecting_detail", n = s.recovery.attempts).to_string())
    } else if s.joined {
        (t!("p2p.state_waiting").to_string(), pal.gold, String::new())
    } else {
        (t!("p2p.state_not_joined").to_string(), pal.dim, String::new())
    };
    let lamp = if tone == pal.dim { pal.unlit } else { tone };
    let mut groups: Vec<Vec<Run>> = vec![vec![(unbulleted(&word).to_string(), strong(14.0), tone), (detail, body(14.0), pal.text)]];
    if let Some(mb) = s.watchdog.last_rss_mb {
        let mut sidecar = vec![(t!("p2p.state_sidecar", mb = format!("{mb:.0}")).to_string(), body(14.0), pal.text)];
        if s.watchdog.restarts > 0 {
            let r = if s.watchdog.restarts == 1 { t!("p2p.state_restart_one") } else { t!("p2p.state_restarts", n = s.watchdog.restarts) };
            sidecar.push((r.to_string(), body(14.0), pal.gold));
        }
        groups.push(sidecar);
    }
    groups.push(vec![
        (t!("p2p.state_announcing").to_string(), body(14.0), pal.text),
        (printable(&s.server_name, NAME_MAX), strong(14.0), pal.text),
    ]);
    let room_w = row.width() - LED_W;
    while groups.len() > 1 && runs_width(ui, &groups.concat()) > room_w {
        groups.pop();
    }
    led(ui, pos2(row.min.x + 3.0, row.center().y), lamp);
    let drawn = runs(ui, pos2(row.min.x + LED_W, row.center().y), &groups.concat(), room_w, Align2::LEFT_CENTER);
    let cadence = t!("p2p.polls").to_string();
    let cadence_w = width_of(ui, &cadence, mono(12.0));
    if polls && drawn.max.x + 16.0 + cadence_w <= row.max.x {
        text(ui, row.right_center(), &cadence, mono(12.0), pal.dim, cadence_w + 1.0, Align2::RIGHT_CENTER);
    }
}

/// The four tabs as one segmented control from `left`, the open one lit.
/// Returns where it was drawn.
fn tabs(ui: &Ui, left: f32, mid_y: f32, room: &Room, pal: &Pal, pass: &mut Pass) -> Rect {
    let labels: Vec<String> = Tab::ALL.iter().map(|t| t.label()).collect();
    let chosen = Tab::ALL.iter().position(|t| *t == room.tab).unwrap_or(0);
    let id = Id::new("disc-tabs");
    let (drawn, clicked) = segmented(ui, id, left, mid_y, &labels, chosen, Seg::Lit, pal);
    match clicked {
        Some(i) => pass.act(Act::Tab(Tab::ALL[i])),
        // The open tab takes its click too, as the cells' does: it lets go
        // of the befriend box, and on Activity fetches the log's news.
        None if ui.ctx().read_response(id.with(chosen)).is_some_and(|r| r.clicked()) => pass.act(Act::Tab(room.tab)),
        None => {}
    }
    drawn
}

/// The Stats tab: six readouts in one block of two rows of three, joined by
/// hairlines, as the Stats page draws its tiles.
fn stats_pane(ui: &Ui, pane: Rect, room: &Room, s: &DiscoveryStatus, look: &Look, pal: &Pal) {
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
    ui.painter().rect_filled(pane, square(), pal.edge);
    let cell_w = (pane.width() - 4.0) / 3.0;
    for (i, (value, label, detail)) in tiles.iter().enumerate() {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        let rect = Rect::from_min_size(
            pos2(pane.min.x + 1.0 + col * (cell_w + 1.0), pane.min.y + 1.0 + row * (look.tile_h + 1.0)),
            vec2(cell_w, look.tile_h),
        );
        ui.painter().rect_filled(rect, CornerRadius::same(2), pal.panel);
        let inner = rect.shrink2(vec2(14.0, look.tile_pad));
        caps(ui, inner.min, label, look.label, pal.dim, inner.width(), Align2::LEFT_TOP);
        // The canvas's label line and the 3 points under it, where the Stats
        // page sets its figure (17 points down at the label's 11.5).
        let figure_y = inner.min.y + look.label + 5.5;
        let figure = text(ui, pos2(inner.min.x, figure_y), value, mono(look.figure), pal.text, inner.width(), Align2::LEFT_TOP);
        text(ui, pos2(inner.min.x, figure.max.y + 1.0), detail, mono(look.detail), pal.dim, inner.width(), Align2::LEFT_TOP);
    }
}

/// The Activity tab: the discovery log, newest first, the age dim and the
/// line in its level's tone (warn and error gold, debug dim), scrolled by
/// the wheel.
fn activity_pane(ui: &mut Ui, pane: Rect, room: &Room, look: &Look, pal: &Pal) {
    if room.activity.is_empty() {
        let at = pos2(pane.min.x, pane.min.y + look.line_h / 2.0);
        text(ui, at, &t!("p2p.activity_empty"), body(13.0), pal.dim, pane.width(), Align2::LEFT_CENTER);
        return;
    }
    let mut inner = ui.new_child(UiBuilder::new().id_salt("disc-activity").max_rect(pane).layout(Layout::top_down(Align::Min)));
    let now = unix_now();
    let n = room.activity.len();
    ScrollArea::vertical().id_salt("disc-activity-log").auto_shrink([false, false]).show_rows(&mut inner, look.line_h, n, |ui, range| {
        for i in range {
            let entry = &room.activity[n - 1 - i];
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), look.line_h), Sense::hover());
            let tone = match entry.level.as_str() {
                "warn" | "error" => pal.gold,
                "debug" => pal.dim,
                _ => pal.text,
            };
            let when = iso_unix(&entry.t).map(|t| age_text(now - t)).unwrap_or_else(|| "—".to_string());
            text(ui, pos2(row.min.x + AGE_W, row.center().y), &when, mono(12.0), pal.dim, AGE_W, Align2::RIGHT_CENTER);
            let x = row.min.x + AGE_W + 14.0;
            text(ui, pos2(x, row.center().y), &printable(&entry.message, 400), body(13.0), tone, row.max.x - x, Align2::LEFT_CENTER);
        }
    });
}

/// The Invite tab: the endpoint, the ticket with its copy, and the
/// befriend box, a field drawn from the room's own.
#[allow(clippy::too_many_arguments)]
fn invite_pane(ui: &Ui, pane: Rect, room: &Room, s: &DiscoveryStatus, look: &Look, pal: &Pal, typing: Typing, pass: &mut Pass) {
    let (x0, w) = (pane.min.x, pane.width());
    let head_h = if look.compact { 18.0 } else { 22.0 };
    let y = pane.min.y + head_h / 2.0;
    small_caps(ui, pos2(x0, y), &t!("p2p.invite_endpoint"), pal.dim, LABEL_COL - 12.0, Align2::LEFT_CENTER);
    match &s.endpoint_id {
        Some(id) => text(ui, pos2(x0 + LABEL_COL, y), &printable(id, 64), mono(12.5), pal.text, w - LABEL_COL, Align2::LEFT_CENTER),
        None => text(ui, pos2(x0 + LABEL_COL, y), &t!("p2p.invite_no_endpoint"), body(13.5), pal.text, w - LABEL_COL, Align2::LEFT_CENTER),
    };

    let head = Rect::from_min_size(pos2(x0, pane.min.y + head_h + 4.0), vec2(w, look.button_h));
    let mut right = head.max.x;
    if s.ticket.is_some() {
        let copy = t!("p2p.invite_copy").to_string();
        let at = Rect::from_min_max(pos2(right - small_button_width(ui, &copy), head.min.y), head.max);
        if form_button(ui, Id::new("disc-copy"), at, &copy, false, pal).clicked() {
            pass.act(Act::CopyTicket);
        }
        right = at.min.x - 12.0;
    }
    head_line(ui, head.left_center(), &t!("p2p.invite_ticket"), &t!("p2p.invite_ticket_hint"), right - x0, pal);

    let field_rect = Rect::from_min_max(pos2(x0, pane.max.y - look.box_h), pane.max);
    let befriend_y = field_rect.min.y - 6.0 - head_h / 2.0;
    let ticket_top = head.max.y + 4.0;
    match &s.ticket {
        Some(ticket) => {
            // Over two lines at most, as the cells wrap it.
            let room_h = befriend_y - head_h / 2.0 - 6.0 - ticket_top;
            let rows = ((room_h + 2.0) / 15.0).floor().clamp(1.0, 2.0) as usize;
            let format = TextFormat { font_id: mono(12.5), color: pal.text, ..Default::default() };
            let mut job = LayoutJob::single_section(printable(ticket, 4096), format);
            job.wrap = TextWrapping { max_width: w, max_rows: rows, break_anywhere: true, overflow_character: Some('…') };
            let galley = ui.painter().layout_job(job);
            ui.painter().galley(pos2(x0, ticket_top), galley, pal.text);
        }
        None => {
            let at = pos2(x0, ticket_top + 8.0);
            text(ui, at, &t!("p2p.invite_no_endpoint"), body(13.5), pal.dim, w, Align2::LEFT_CENTER);
        }
    }
    head_line(ui, pos2(x0, befriend_y), &t!("p2p.invite_befriend"), &t!("p2p.invite_befriend_hint"), w, pal);
    let focused = room.ticket_focus && typing.live;
    let input = studio::Input::new(room.ticket.value(), room.ticket.cursor()).focused(focused, typing.for_field(focused)).mono();
    let typed = field_box(ui, Id::new("disc-ticket"), field_rect, &input, pal);
    if typed.response.clicked() {
        pass.act(Act::TicketFocus);
    }
    if focused {
        pass.caret = typed.caret;
    }
}

/// The settings the Config tab lists, in the cells' order: the key that
/// opens each, its label, its value in words, its hint, and the setting.
/// Community seeds has no key: the config file sets it.
fn settings(room: &Room, s: &DiscoveryStatus) -> [(char, String, String, String, Option<DiscoverySetting>); 6] {
    let days = |n: u32| if n > 0 { t!("p2p.value_days", n = n).to_string() } else { t!("p2p.value_never").to_string() };
    [
        (
            '1',
            t!("p2p.config_storage").to_string(),
            t!("p2p.value_mb", n = s.max_peer_db_storage_mb).to_string(),
            t!("p2p.config_storage_hint").to_string(),
            Some(DiscoverySetting::MaxStorageMb),
        ),
        (
            '2',
            t!("p2p.config_retention").to_string(),
            if s.peer_retention_days > 0 {
                t!("p2p.value_days_silence", n = s.peer_retention_days).to_string()
            } else {
                t!("p2p.value_never").to_string()
            },
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
        (
            '4',
            t!("p2p.config_rotation").to_string(),
            days(s.rotation_days),
            t!("p2p.config_rotation_hint").to_string(),
            Some(DiscoverySetting::RotationDays),
        ),
        (
            '5',
            t!("p2p.config_memory").to_string(),
            if s.watchdog.max_rss_mb > 0 {
                t!("p2p.value_mb", n = s.watchdog.max_rss_mb).to_string()
            } else {
                t!("p2p.tile_memory_off").to_string()
            },
            t!("p2p.config_memory_hint").to_string(),
            Some(DiscoverySetting::SidecarMaxRssMb),
        ),
        (
            ' ',
            t!("p2p.config_seeds").to_string(),
            if s.community_seeds { t!("p2p.value_on") } else { t!("p2p.value_off") }.to_string(),
            t!("p2p.config_seeds_hint").to_string(),
            None,
        ),
    ]
}

/// The Config tab: the announced identity with its edit, then the
/// settings, each a row a click opens (all but the config file's).
fn config_pane(ui: &Ui, pane: Rect, room: &Room, s: &DiscoveryStatus, look: &Look, pal: &Pal, pass: &mut Pass) {
    let head = Rect::from_min_size(pane.min, vec2(pane.width(), look.button_h));
    let edit = t!("p2p.config_edit").to_string();
    let at = Rect::from_min_max(pos2(head.max.x - small_button_width(ui, &edit), head.min.y), head.max);
    if form_button(ui, Id::new("disc-edit"), at, &edit, false, pal).clicked() {
        pass.act(Act::IdentityOpen);
    }
    let label = small_caps(ui, head.left_center(), &t!("p2p.config_announcing"), pal.dim, at.min.x - head.min.x, Align2::LEFT_CENTER);
    let mut who = vec![(printable(&s.server_name, NAME_MAX), strong(14.0), pal.text)];
    if !s.server_description.trim().is_empty() {
        who.push((format!(" — {}", printable(&s.server_description, DESCRIPTION_MAX)), body(14.0), pal.soft));
    }
    let x = label.max.x + 12.0;
    runs(ui, pos2(x, head.center().y), &who, at.min.x - 12.0 - x, Align2::LEFT_CENTER);

    let top = head.max.y + 6.0;
    let row_h = ((pane.max.y - top) / 6.0).floor();
    let (label_col, value_col) = if look.compact { (180.0, 180.0) } else { (220.0, 240.0) };
    let key_x = pane.min.x + 8.0;
    let label_x = pane.min.x + 34.0;
    let value_x = label_x + label_col;
    let hint_x = value_x + value_col;
    for (i, (key, label, value, hint, setting)) in settings(room, s).iter().enumerate() {
        let rect = Rect::from_min_size(pos2(pane.min.x, top + i as f32 * row_h), vec2(pane.width(), row_h));
        let hovered = match setting {
            Some(which) => {
                let response = ui.interact(rect, Id::new("disc-setting").with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                if response.clicked() {
                    pass.act(Act::SettingOpen(*which));
                }
                response.hovered()
            }
            None => false,
        };
        if hovered {
            ui.painter().rect_filled(rect, square(), pal.raised);
        }
        let cy = rect.center().y;
        if *key != ' ' {
            text(ui, pos2(key_x, cy), &key.to_string(), mono(12.5), if hovered { pal.accent } else { pal.text }, 20.0, Align2::LEFT_CENTER);
        }
        let label_tone = if hovered { pal.text } else { pal.dim };
        text(ui, pos2(label_x, cy), label, body(13.5), label_tone, label_col - 12.0, Align2::LEFT_CENTER);
        text(ui, pos2(value_x, cy), value, strong(13.5), pal.text, value_col - 12.0, Align2::LEFT_CENTER);
        if pane.max.x - hint_x >= 120.0 {
            text(ui, pos2(hint_x, cy), hint, body(12.5), pal.dim, pane.max.x - hint_x - 8.0, Align2::LEFT_CENTER);
        }
    }
}

/// The servers table's columns at a width: the canvas's in a wide room,
/// tighter ones in a narrow one; and as the cells' do, SEEDERS goes first
/// where SERVER would be cramped, then TRACKS. Each optional column's
/// place, and the fixed ones'.
struct Columns {
    cols: Vec<Col>,
    tracks: Option<usize>,
    seeders: Option<usize>,
    online: usize,
    snapshot: usize,
    federation: usize,
}

fn columns(w: f32) -> Columns {
    let (num, online, snapshot, federation) =
        if w >= 1000.0 { (84.0, 150.0, 220.0, 150.0) } else { (60.0, 96.0, 140.0, 104.0) };
    let server_w = |numbers: usize| w - 20.0 - (online + snapshot + federation) - numbers as f32 * (num + 16.0) - 3.0 * 16.0;
    let numbers = if server_w(2) >= 140.0 {
        2
    } else if server_w(1) >= 140.0 {
        1
    } else {
        0
    };
    let mut cols = vec![Col::left(t!("p2p.col_server"), 0.0)];
    let place = |cols: &mut Vec<Col>, col: Col| {
        cols.push(col);
        cols.len() - 1
    };
    let tracks = (numbers >= 1).then(|| place(&mut cols, Col::right(t!("p2p.col_tracks"), num)));
    let seeders = (numbers >= 2).then(|| place(&mut cols, Col::right(t!("p2p.col_seeders"), num)));
    let online = place(&mut cols, Col::left(t!("p2p.col_online"), online));
    let snapshot = place(&mut cols, Col::left(t!("p2p.col_snapshot"), snapshot));
    let federation = place(&mut cols, Col::left(t!("p2p.col_federation"), federation));
    Columns { cols, tracks, seeders, online, snapshot, federation }
}

/// The servers you follow: the section's line, then the kit's table, the
/// keyboard's cursor row ringed and kept in view, a click on a row opening
/// its sheet. Not drawn at all where it would have no row: the table gives
/// way before the pane, as the cells' does. Returns the cursor row's server
/// and its description in words, when it has one.
fn servers(ui: &mut Ui, room: &Room, s: &DiscoveryStatus, look: &Look, pal: &Pal, typing: Typing, pass: &mut Pass) -> Option<String> {
    if ui.available_height() < look.section_h + 4.0 + HEAD_H + look.row_h {
        return None;
    }
    section_line(ui, room, s, look, pal, typing, pass);
    ui.add_space(if look.compact { 4.0 } else { 8.0 });
    let w = ui.available_width();
    let cols = columns(w);
    let rows = room.row_indices();
    if rows.is_empty() {
        let words = if room.filter_on && !room.catalog.peers.is_empty() { t!("p2p.empty_filter") } else { t!("p2p.empty_servers") };
        let rect = Rect::from_min_size(ui.cursor().min, vec2(w, HEAD_H + look.row_h));
        let mut heads = ui.new_child(UiBuilder::new().id_salt("disc-servers-none").max_rect(rect).layout(Layout::top_down(Align::Min)));
        Table::new(Id::new("disc-servers"), &cols.cols, 0).inert().show(&mut heads, pal, |_, _, _| {});
        let at = pos2(rect.min.x + 10.0, rect.max.y - look.row_h / 2.0);
        text(ui, at, &words, body(13.5), pal.dim, w - 20.0, Align2::LEFT_CENTER);
        return None;
    }
    let now = unix_now();
    let table = Table::new(Id::new("disc-servers"), &cols.cols, rows.len()).cursor(room.sel).row_height(look.row_h);
    let drawn = table.show(ui, pal, |ui, i, row| {
        let peer = &room.catalog.peers[rows[i]];
        server_row(ui, room, peer, &cols, row, now, pal);
        // The server's own words under the pointer, as the cells' tip.
        if typing.live && !peer.payload.description.trim().is_empty() {
            let _ = ui
                .interact(row.rect, Id::new("disc-row-tip").with(i), Sense::hover())
                .on_hover_text(printable(&peer.payload.description, DESCRIPTION_MAX));
        }
    });
    if let Some(i) = drawn.row {
        pass.act(Act::OpenSheet(i));
    }
    let peer = room.sel.and_then(|at| rows.get(at)).map(|&at| &room.catalog.peers[at])?;
    let description = printable(&peer.payload.description, DESCRIPTION_MAX);
    (!description.trim().is_empty()).then(|| format!("{} · {description}", display_name(peer)))
}

/// One server's row: its name (semibold on the cursor's row), its counts in
/// the figures' face, whether it is online behind its LED, what is held of
/// its snapshot, and what federation says of it in its tone.
fn server_row(ui: &Ui, room: &Room, peer: &CatalogPeer, cols: &Columns, row: &RowCtx, now: i64, pal: &Pal) {
    let name_font = if row.selected { strong(14.0) } else { body(14.0) };
    cell(ui, row, 0, &display_name(peer), name_font, ink(row, pal.text, pal), false);
    if let Some(at) = cols.tracks {
        cell(ui, row, at, &fmt_count(peer.payload.row_count), mono(12.5), ink(row, pal.soft, pal), true);
    }
    if let Some(at) = cols.seeders {
        cell(ui, row, at, &peer.seeders.to_string(), mono(12.5), ink(row, pal.soft, pal), true);
    }
    let (online, tone) = if peer.online {
        (t!("p2p.online").to_string(), pal.soft)
    } else {
        let words = match iso_unix(&peer.updated_at).map(|t| age_text(now - t)) {
            Some(age) => t!("p2p.offline_age", age = age).to_string(),
            None => t!("p2p.offline").to_string(),
        };
        (words, pal.dim)
    };
    let (x, cw) = row.cells[cols.online];
    let cy = row.rect.center().y;
    led(ui, pos2(x + 3.0, cy), if peer.online { pal.ok } else { pal.unlit });
    text(ui, pos2(x + LED_W, cy), &online, body(13.0), ink(row, tone, pal), cw - LED_W, Align2::LEFT_CENTER);
    let (snapshot, tone) = match &peer.fetched {
        None => ("—".to_string(), pal.dim),
        Some(h) => {
            let words = match (h.stale, h.pinned) {
                (false, false) => t!("p2p.snap_downloaded"),
                (false, true) => t!("p2p.snap_pinned"),
                (true, false) => t!("p2p.snap_update"),
                (true, true) => t!("p2p.snap_update_pinned"),
            };
            (words.to_string(), pal.soft)
        }
    };
    cell(ui, row, cols.snapshot, &snapshot, body(13.0), ink(row, tone, pal), false);
    let (federation, tone) = match room.relation(&peer.from) {
        Relation::None => ("—".to_string(), pal.dim),
        Relation::Federated => (t!("p2p.fed_federated").to_string(), pal.ok),
        Relation::Theirs => (t!("p2p.fed_theirs").to_string(), pal.gold),
        Relation::Sent => (t!("p2p.fed_sent").to_string(), pal.dim),
    };
    cell(ui, row, cols.federation, &federation, body(13.0), ink(row, tone, pal), false);
}

/// The section's line: its name at the left; at the right the filter's
/// box (a field drawn from the room's own; a click on it is the cells' `/`
/// filter), and when any are hidden or blocked, their counts, each with
/// the button for its key, as the cells' words name it. A narrow room drops
/// the counts' words first, then narrows the box, then drops the name.
#[allow(clippy::too_many_arguments)]
fn section_line(ui: &mut Ui, room: &Room, s: &DiscoveryStatus, look: &Look, pal: &Pal, typing: Typing, pass: &mut Pass) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, look.section_h), Sense::hover());
    let cy = row.center().y;
    let title = t!("p2p.section_servers").to_string();
    // Each count: its words, its key's button and the key.
    let mut counts: Vec<(String, String, char)> = Vec::new();
    let hidden = room.catalog.hidden_incompatible;
    if hidden > 0 || room.show_incompatible {
        let words = if room.show_incompatible {
            t!("p2p.hidden_showing")
        } else if hidden == 1 {
            t!("p2p.hidden_one")
        } else {
            t!("p2p.hidden_many", n = hidden)
        };
        let label = if room.show_incompatible { t!("p2p.hidden_hide") } else { t!("p2p.hidden_show") };
        counts.push((words.to_string(), label.to_string(), 'h'));
    }
    if !s.blocked_peers.is_empty() {
        counts.push((t!("p2p.blocked_n", n = s.blocked_peers.len()).to_string(), t!("p2p.blocked_manage").to_string(), 'u'));
    }
    let font = body(13.0);
    let dot = "·";
    let dot_w = width_of(ui, dot, font.clone()) + 24.0;
    let buttons_w: f32 = counts.iter().map(|(_, label, _)| small_button_width(ui, label)).sum();
    let words_w: f32 = counts.iter().map(|(words, _, _)| width_of(ui, words, font.clone()) + 8.0).sum();
    let title_w = small_caps_width(ui, &title) + 16.0;
    let fixed = |words: bool, title: bool| {
        buttons_w + dot_w * counts.len() as f32 + if words { words_w } else { 0.0 } + if title { title_w } else { 0.0 }
    };
    let with_words = w - fixed(true, true) >= FILTER_MIN;
    let with_title = w - fixed(false, true) >= FILTER_MIN;
    let filter_w = (w - fixed(with_words, with_title)).clamp(FILTER_MIN.min(w), FILTER_W);

    let mut right = row.max.x;
    // A key the cells' words name reaches the room only where the room
    // would take it as that key: not while a field has the keyboard.
    let open = room.claim() == Claim::Open;
    for (words, label, key) in counts.iter().rev() {
        let half = look.button_h / 2.0;
        let at = Rect::from_min_max(pos2(right - small_button_width(ui, label), cy - half), pos2(right, cy + half));
        if form_button(ui, Id::new("disc-key").with(*key), at, label, false, pal).clicked() && open {
            pass.does.push(Do::Key(KeyCode::Char(*key)));
        }
        right = at.min.x - 8.0;
        if with_words {
            right = text(ui, pos2(right, cy), words, font.clone(), pal.dim, right - row.min.x, Align2::RIGHT_CENTER).min.x;
        }
        right -= 12.0;
        right = text(ui, pos2(right, cy), dot, font.clone(), pal.dim, 20.0, Align2::RIGHT_CENTER).min.x - 12.0;
    }
    let focused = room.filter_focus && typing.live;
    let (value, caret) = if room.filter_on { (room.filter.value(), room.filter.cursor()) } else { ("", 0) };
    let placeholder = t!("p2p.filter_key").to_string();
    let input = studio::Input::new(value, caret).focused(focused, typing.for_field(focused)).placeholder(&placeholder);
    let half = look.box_h.min(look.section_h) / 2.0;
    let rect = Rect::from_min_max(pos2(right - filter_w, cy - half), pos2(right, cy + half));
    let typed = field_box(ui, Id::new("disc-filter"), rect, &input, pal);
    if typed.response.clicked() {
        pass.act(Act::FilterOpen);
    }
    if focused {
        pass.caret = typed.caret;
    }
    if with_title {
        small_caps(ui, row.left_center(), &title, pal.dim, rect.min.x - row.min.x - 16.0, Align2::LEFT_CENTER);
    }
}

/// The room's foot: what it is busy with in the accent, else its note (an
/// error in gold), else the cursor row's server and its description.
fn foot_line(ui: &Ui, rect: Rect, room: &Room, described: Option<&str>, pal: &Pal) {
    if room.busy.is_some() || room.note.is_some() {
        note_line(ui, rect, room.busy.as_deref(), room.note.as_ref(), pal);
    } else if let Some(words) = described {
        text(ui, rect.left_center(), words, body(13.0), pal.text, rect.width(), Align2::LEFT_CENTER);
    }
}

// ── Off the network ───────────────────────────────────────────────────────

/// The join page: the state line, the join card (the add-folder card's
/// shape: green, and a click joins at once), the federation-requests opt-in
/// as a tick on a card a click anywhere on flips, and the pitch. Where the
/// sidecar can be neither found nor fetched, the state in gold and the
/// pitch alone.
fn off_network(ui: &mut Ui, room: &Room, s: &DiscoveryStatus, look: &Look, pal: &Pal, pass: &mut Pass) {
    let unavailable = !s.binary_found && !s.binary_fetchable;
    ScrollArea::vertical().id_salt("disc-off").auto_shrink([false, false]).show(ui, |ui| {
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
        let (words, tone, lamp) = if unavailable {
            (unbulleted(&t!("p2p.state_unavailable")).to_string(), pal.gold, pal.gold)
        } else {
            let mut words = unbulleted(&t!("p2p.state_off")).to_string();
            if !s.binary_found && s.binary_fetchable {
                words.push_str(" · ");
                words.push_str(&t!("p2p.off_will_download"));
            }
            (words, pal.dim, pal.unlit)
        };
        led(ui, pos2(row.min.x + 3.0, row.center().y), lamp);
        text(ui, pos2(row.min.x + LED_W, row.center().y), &words, body(14.0), tone, w - LED_W, Align2::LEFT_CENTER);
        ui.add_space(look.gap);
        if !unavailable {
            if add_card(ui, Id::new("disc-join"), &t!("p2p.off_join"), pal).clicked() {
                pass.act(Act::Join);
            }
            ui.add_space(10.0);
            requests_card(ui, room, pal, pass);
            ui.add_space(look.gap);
        }
        let w = w.min(PITCH_W);
        para(ui, w, &[(t!("p2p.off_pitch_1").to_string(), body(14.0), pal.text)]);
        ui.add_space(14.0);
        label_row(ui, 16.0, &t!("p2p.off_shared"), pal);
        ui.add_space(4.0);
        para(
            ui,
            w,
            &[
                (t!("p2p.off_pitch_2a").to_string(), body(14.0), pal.text),
                (t!("p2p.off_pitch_2b").to_string(), strong(14.0), pal.text),
                (t!("p2p.off_pitch_2c").to_string(), body(14.0), pal.text),
            ],
        );
        ui.add_space(14.0);
        para(ui, w, &[(t!("p2p.off_pitch_3").to_string(), body(14.0), pal.text)]);
    });
}

/// The opt-in's card: the tick and its words, its hint dim under them, a
/// hairline that brightens under the pointer. A click on the card or the
/// tick flips it, as Space does.
fn requests_card(ui: &mut Ui, room: &Room, pal: &Pal, pass: &mut Pass) {
    let w = ui.available_width();
    let (card, _) = ui.allocate_exact_size(vec2(w, 72.0), Sense::hover());
    let whole = ui.interact(card, Id::new("disc-requests-card"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let edge = if whole.contains_pointer() { pal.dim } else { pal.edge_strong };
    ui.painter().rect_stroke(card, square(), Stroke::new(1.0, edge), StrokeKind::Inside);
    let inner = card.shrink2(vec2(16.0, 10.0));
    let mut tick_ui = ui.new_child(UiBuilder::new().id_salt("disc-requests-line").max_rect(inner).layout(Layout::top_down(Align::Min)));
    let tick = check_line(&mut tick_ui, Id::new("disc-requests"), room.accept_requests, &t!("p2p.off_requests"), "", pal);
    let at = pos2(inner.min.x + 26.0, inner.min.y + 28.0 + 12.0);
    text(ui, at, &t!("p2p.off_requests_hint"), body(13.0), pal.dim, inner.width() - 26.0, Align2::LEFT_CENTER);
    if whole.clicked() || tick.clicked() {
        pass.act(Act::ToggleRequests);
    }
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The room's modal, if one is up, over the whole of the room's area.
fn modals(ui: &Ui, room: &Room, over: Rect, pal: &Pal, typing: Typing, key_hints: bool, pass: &mut Pass) {
    match &room.modal {
        Modal::None => {}
        Modal::Sheet { id, sel } => sheet(ui, room, over, id, *sel, pal, key_hints, pass),
        Modal::Federate(c) => compose(ui, over, c, pal, typing.composition, pass),
        Modal::Identity(d) => identity(ui, over, d, pal, typing.composition, pass),
        Modal::Setting(d) => setting(ui, over, d, pal, typing.composition, pass),
        Modal::Block(id) => {
            let name = room.peer(id).map(display_name).unwrap_or_else(|| short_id(id));
            gate(
                ui,
                Id::new("disc-block"),
                over,
                &t!("p2p.block_title", name = name),
                [t!("p2p.block_1").to_string(), t!("p2p.block_2").to_string()],
                (t!("p2p.block_keep").to_string(), Act::BlockCancel),
                (t!("p2p.block_confirm").to_string(), Act::BlockConfirm),
                pal,
                pass,
            )
        }
        Modal::Leave => gate(
            ui,
            Id::new("disc-leave"),
            over,
            &t!("p2p.leave_title"),
            [t!("p2p.leave_1").to_string(), t!("p2p.leave_2").to_string()],
            (t!("p2p.leave_stay").to_string(), Act::LeaveCancel),
            (t!("p2p.leave_confirm").to_string(), Act::LeaveConfirm),
            pal,
            pass,
        ),
        Modal::Blocked(sel) => blocked(ui, room, over, *sel, pal, pass),
    }
}

/// One row of a modal's list, the body's width and `h` tall: raised and
/// ringed in the accent while it is the list's cursor, raised under the
/// pointer, the hand over it. The caller draws what is in it.
fn pick_row(ui: &mut Ui, id: Id, h: f32, on: bool, pal: &Pal) -> (Rect, Response) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if on || response.hovered() {
        ui.painter().rect_filled(rect, square(), pal.raised);
    }
    if on {
        ring(ui, rect, pal);
    }
    (rect, response)
}

/// A form's foot, a row the body's width: `error` in gold at its left, if
/// there is one, and its buttons at its right in order, each `(label,
/// primary)`. Returns the one clicked.
fn form_foot(ui: &mut Ui, id: Id, error: Option<&str>, buttons: &[(String, bool)], pal: &Pal) -> Option<usize> {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
    let mut right = row.max.x;
    let mut clicked = None;
    for (i, (label, primary)) in buttons.iter().enumerate().rev() {
        let at = Rect::from_min_max(pos2(right - form_button_width(ui, label), row.min.y), pos2(right, row.max.y));
        if form_button(ui, id.with(i), at, label, *primary, pal).clicked() {
            clicked = Some(i);
        }
        right = at.min.x - 10.0;
    }
    if let Some(error) = error {
        text(ui, row.left_center(), error, body(13.0), pal.gold, right - row.min.x - 6.0, Align2::LEFT_CENTER);
    }
    clicked
}

/// One of the sheet's facts: its label in the canvas's column, and its
/// value's runs, after an LED when it has one, cut to the row.
fn fact(ui: &mut Ui, label: &str, value: &[Run], lamp: Option<Color32>, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    small_caps(ui, row.left_center(), label, pal.dim, FACT_COL - 12.0, Align2::LEFT_CENTER);
    let mut x = row.min.x + FACT_COL;
    if let Some(color) = lamp {
        led(ui, pos2(x + 3.0, row.center().y), color);
        x += LED_W;
    }
    runs(ui, pos2(x, row.center().y), value, row.max.x - x, Align2::LEFT_CENTER);
    ui.add_space(8.0);
}

/// The icon the canvas gives the sheet's action under `key`, its 14-point
/// paths centred on `at`: download, pin, remove, federate, forget, block.
fn act_icon(ui: &Ui, at: Pos2, key: char, color: Color32) {
    let o = at - vec2(7.0, 7.0);
    let p = |x: f32, y: f32| o + vec2(x, y);
    let stroke = Stroke::new(1.4, color);
    let painter = ui.painter();
    let path = |points: Vec<Pos2>| {
        painter.add(Shape::line(points, stroke));
    };
    match key {
        'd' => {
            path(vec![p(7.0, 1.5), p(7.0, 9.0)]);
            path(vec![p(4.0, 6.0), p(7.0, 9.0), p(10.0, 6.0)]);
            path(vec![p(2.0, 12.5), p(12.0, 12.5)]);
        }
        'p' => {
            path(vec![p(4.5, 1.5), p(9.5, 1.5)]);
            path(vec![p(5.5, 1.5), p(5.5, 5.5), p(3.5, 8.0), p(10.5, 8.0), p(8.5, 5.5), p(8.5, 1.5)]);
            path(vec![p(7.0, 8.0), p(7.0, 12.5)]);
        }
        'r' => {
            path(vec![p(2.0, 3.5), p(12.0, 3.5)]);
            path(vec![p(5.0, 3.5), p(5.0, 2.0), p(9.0, 2.0), p(9.0, 3.5)]);
            path(vec![p(3.5, 3.5), p(4.2, 12.5), p(9.8, 12.5), p(10.5, 3.5)]);
        }
        'f' => {
            path(vec![p(2.0, 4.5), p(11.0, 4.5)]);
            path(vec![p(8.5, 2.0), p(11.0, 4.5), p(8.5, 7.0)]);
            path(vec![p(12.0, 9.5), p(3.0, 9.5)]);
            path(vec![p(5.5, 7.0), p(3.0, 9.5), p(5.5, 12.0)]);
        }
        'g' => {
            painter.circle_stroke(p(7.0, 7.0), 5.0, stroke);
            path(vec![p(4.5, 7.0), p(9.5, 7.0)]);
        }
        'b' => {
            painter.circle_stroke(p(7.0, 7.0), 5.0, stroke);
            path(vec![p(3.5, 3.5), p(10.5, 10.5)]);
        }
        _ => {}
    }
}

/// A click on a row, or Enter on the cursor's: the server's name, its
/// description, its facts, then its actions as a list (only what applies),
/// each behind its icon, the list's cursor ringed. A click on the cursor's
/// row runs it, a click on another moves the cursor there; the close is
/// the cells' [X].
#[allow(clippy::too_many_arguments)]
fn sheet(ui: &Ui, room: &Room, over: Rect, id: &str, sel: usize, pal: &Pal, key_hints: bool, pass: &mut Pass) {
    let Some(peer) = room.peer(id) else { return };
    let menu = room.sheet_menu(peer);
    let now = unix_now();
    let shown = modal(ui, Id::new("disc-sheet"), over, MODAL_W, &display_name(peer), pal, |ui| {
        let w = ui.available_width();
        let description = printable(&peer.payload.description, DESCRIPTION_MAX);
        if !description.trim().is_empty() {
            para(ui, w, &[(description, body(13.5), pal.soft)]);
            ui.add_space(20.0);
        }
        // The canvas's id in the figures' face; its words after it soft.
        let seen = iso_unix(&peer.updated_at).map(|t| t!("p2p.sheet_seen", age = age_text(now - t)).to_string()).unwrap_or_default();
        let endpoint = [(short_id(&peer.from), mono(12.5), pal.text), (seen, body(13.5), pal.soft)];
        fact(ui, &t!("p2p.sheet_endpoint"), &endpoint, None, pal);
        // Online or offline in its tone (an offline server's dim, as its
        // row's), then its counts.
        let (word, tone) = if peer.online { (t!("p2p.online"), pal.soft) } else { (t!("p2p.offline"), pal.dim) };
        let counts = t!("p2p.sheet_tracks", tracks = fmt_count(peer.payload.row_count), seeders = peer.seeders);
        let online = [(word.to_string(), body(13.5), tone), (format!(" · {counts}"), body(13.5), pal.soft)];
        fact(ui, &t!("p2p.sheet_online"), &online, Some(if peer.online { pal.ok } else { pal.unlit }), pal);
        let snapshot = match &peer.fetched {
            None => t!("p2p.sheet_snapshot_none").to_string(),
            Some(h) => {
                let age = iso_unix(&h.fetched_at).map(|t| age_text(now - t)).unwrap_or_else(|| "—".to_string());
                let mut words = t!("p2p.sheet_snapshot_held", age = age, size = fmt_bytes(h.size_bytes)).to_string();
                if h.stale {
                    words.push_str(&t!("p2p.sheet_snapshot_stale"));
                }
                if h.pinned {
                    words.push_str(&t!("p2p.sheet_snapshot_pinned"));
                }
                words
            }
        };
        fact(ui, &t!("p2p.sheet_snapshot"), &[(snapshot, body(13.5), pal.soft)], None, pal);
        let (federation, tone) = match room.relation(&peer.from) {
            Relation::None => ("—".to_string(), pal.dim),
            Relation::Federated => (t!("p2p.sheet_fed_federated").to_string(), pal.ok),
            Relation::Theirs => (t!("p2p.sheet_fed_theirs").to_string(), pal.gold),
            Relation::Sent => (t!("p2p.sheet_fed_sent").to_string(), pal.soft),
        };
        fact(ui, &t!("p2p.sheet_federation"), &[(federation, body(13.5), tone)], None, pal);
        // The canvas's 20 points under the facts (the last one's 8 and 12
        // more), then the hairline over the actions.
        ui.add_space(12.0);
        let (rule, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
        ui.painter().rect_filled(rule, 0.0, pal.edge);
        ui.add_space(14.0);
        let mut picked = None;
        for (i, (key, label, _)) in menu.iter().enumerate() {
            let on = i == sel;
            let (rect, mut response) = pick_row(ui, Id::new("disc-sheet-act").with(i), 38.0, on, pal);
            let lit = on || response.hovered();
            act_icon(ui, pos2(rect.min.x + 19.0, rect.center().y), *key, if lit { pal.text } else { pal.dim });
            let font = if on { strong(14.0) } else { body(14.0) };
            text(ui, pos2(rect.min.x + 38.0, rect.center().y), label, font, pal.text, rect.width() - 50.0, Align2::LEFT_CENTER);
            // The row's letter runs it too: the canvas's tip, while the
            // host names keys.
            if key_hints {
                response = response.on_hover_text(key.to_string());
            }
            if response.clicked() {
                picked = Some(if on { Act::SheetRun } else { Act::SheetPick(i) });
            }
            ui.add_space(2.0);
        }
        picked
    });
    if shown.closed {
        pass.act(Act::SheetClose);
    }
    if let Some(act) = shown.inner {
        pass.act(act);
    }
}

/// The federate compose: what is sent, the message (a field drawn from the
/// room's own), the libraries offered back as ticks in three columns, the
/// keyboard's one ringed, an error in gold, and Send, lit once the library
/// load has answered.
fn compose(ui: &Ui, over: Rect, c: &Compose, pal: &Pal, composition: &str, pass: &mut Pass) {
    let title = t!("p2p.compose_title", name = c.name).to_string();
    let shown = modal(ui, Id::new("disc-compose"), over, MODAL_W, &title, pal, |ui| {
        let mut does = Vec::new();
        let w = ui.available_width();
        for key in ["p2p.compose_1", "p2p.compose_2", "p2p.compose_3"] {
            para(ui, w, &[(t!(key).to_string(), body(13.5), pal.dim)]);
        }
        ui.add_space(16.0);
        let focused = c.focus.is_none();
        let input = studio::Input::new(c.message.value(), c.message.cursor()).focused(focused, if focused { composition } else { "" });
        let label = t!("p2p.compose_message", n = c.message.value().chars().count()).to_string();
        let typed = field(ui, Id::new("disc-compose-message"), &label, &input, pal);
        if typed.response.clicked() {
            does.push(Act::ComposeFocusMessage);
        }
        let caret = if focused { typed.caret } else { None };
        ui.add_space(18.0);
        label_row(ui, 14.0, &t!("p2p.compose_libraries"), pal);
        ui.add_space(6.0);
        if !c.loaded {
            line(ui, 24.0, &t!("p2p.compose_loading"), body(13.5), pal.dim);
        } else if c.libraries.is_empty() {
            line(ui, 24.0, &t!("p2p.compose_none"), body(13.5), pal.dim);
        }
        let col_w = (w / 3.0).floor();
        for (r, chunk) in c.libraries.chunks(3).enumerate() {
            let (row, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
            for (k, (name, on)) in chunk.iter().enumerate() {
                let i = r * 3 + k;
                let at = Rect::from_min_size(pos2(row.min.x + k as f32 * col_w, row.min.y), vec2(col_w - 10.0, 28.0));
                let mut one = ui.new_child(UiBuilder::new().id_salt(("disc-compose-lib-ui", i)).max_rect(at).layout(Layout::top_down(Align::Min)));
                let response = check_line(&mut one, Id::new("disc-compose-lib").with(i), *on, name, "", pal);
                if c.focus == Some(i) {
                    ring(ui, response.rect.expand2(vec2(5.0, 1.0)), pal);
                }
                if response.clicked() {
                    does.push(Act::ComposeToggle(i));
                }
            }
            ui.add_space(2.0);
        }
        if let Some(error) = &c.error {
            ui.add_space(8.0);
            para(ui, w, &[(error.clone(), body(13.5), pal.gold)]);
        }
        ui.add_space(18.0);
        if form_foot(ui, Id::new("disc-compose-send"), None, &[(t!("p2p.compose_send").to_string(), c.loaded)], pal).is_some() {
            does.push(Act::ComposeSend);
        }
        (does, caret)
    });
    if shown.closed {
        pass.act(Act::ComposeCancel);
    }
    let (does, caret) = shown.inner;
    pass.does.extend(does.into_iter().map(Do::Act));
    pass.caret = caret;
}

/// The identity modal: the name and the description the network sees, each
/// a field drawn from the room's own with its live count and its hint; the
/// error in gold beside Save.
fn identity(ui: &Ui, over: Rect, d: &IdentityDraft, pal: &Pal, composition: &str, pass: &mut Pass) {
    let shown = modal(ui, Id::new("disc-identity"), over, MODAL_W, &t!("p2p.identity_title"), pal, |ui| {
        let mut does = Vec::new();
        let mut caret = None;
        let fields = [
            (
                false,
                &d.name,
                t!("p2p.identity_name", n = d.name.value().chars().count()).to_string(),
                t!("p2p.identity_name_hint").to_string(),
                "disc-identity-name",
            ),
            (
                true,
                &d.description,
                t!("p2p.identity_description", n = d.description.value().chars().count()).to_string(),
                t!("p2p.identity_description_hint").to_string(),
                "disc-identity-description",
            ),
        ];
        for (description, value, label, hint, id) in fields {
            let focused = d.on_description == description;
            let input = studio::Input::new(value.value(), value.cursor()).focused(focused, if focused { composition } else { "" });
            let typed = field(ui, Id::new(id), &label, &input, pal);
            if typed.response.clicked() {
                does.push(Act::IdentityFocus(description));
            }
            if focused {
                caret = typed.caret;
            }
            ui.add_space(4.0);
            line(ui, 18.0, &hint, body(12.5), pal.dim);
            ui.add_space(14.0);
        }
        ui.add_space(4.0);
        let save = [(t!("p2p.identity_save").to_string(), true)];
        if form_foot(ui, Id::new("disc-identity-foot"), d.error.as_deref(), &save, pal).is_some() {
            does.push(Act::IdentitySave);
        }
        (does, caret)
    });
    if shown.closed {
        pass.act(Act::IdentityCancel);
    }
    let (does, caret) = shown.inner;
    pass.does.extend(does.into_iter().map(Do::Act));
    pass.caret = caret;
}

/// One numeric setting: its help, its number in a box that always has the
/// keyboard (a click on it saves, as a click on the cells' does), the
/// error in gold beside Save.
fn setting(ui: &Ui, over: Rect, d: &SettingDraft, pal: &Pal, composition: &str, pass: &mut Pass) {
    let (title, label, help) = match d.which {
        DiscoverySetting::MaxStorageMb => ("p2p.setting_storage_title", "p2p.setting_storage_label", "p2p.setting_storage_help"),
        DiscoverySetting::PeerRetentionDays => ("p2p.setting_retention_title", "p2p.setting_retention_label", "p2p.setting_retention_help"),
        DiscoverySetting::AutoFetchCount => ("p2p.setting_autofetch_title", "p2p.setting_autofetch_label", "p2p.setting_autofetch_help"),
        DiscoverySetting::RotationDays => ("p2p.setting_rotation_title", "p2p.setting_rotation_label", "p2p.setting_rotation_help"),
        DiscoverySetting::SidecarMaxRssMb => ("p2p.setting_memory_title", "p2p.setting_memory_label", "p2p.setting_memory_help"),
    };
    let shown = modal(ui, Id::new("disc-setting-form"), over, MODAL_W, &t!(title), pal, |ui| {
        let mut does = Vec::new();
        let w = ui.available_width();
        para(ui, w, &[(t!(help).to_string(), body(13.5), pal.dim)]);
        ui.add_space(18.0);
        label_row(ui, 14.0, &t!(label), pal);
        ui.add_space(6.0);
        let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
        let at = Rect::from_min_size(row.min, vec2(NUMBER_W.min(w), FIELD_H));
        let input = studio::Input::new(d.value.value(), d.value.cursor()).focused(true, composition).mono();
        let typed = field_box(ui, Id::new("disc-setting-value"), at, &input, pal);
        if typed.response.clicked() {
            does.push(Act::SettingSave);
        }
        ui.add_space(20.0);
        let save = [(t!("p2p.setting_save").to_string(), true)];
        if form_foot(ui, Id::new("disc-setting-foot"), d.error.as_deref(), &save, pal).is_some() {
            does.push(Act::SettingSave);
        }
        (does, typed.caret)
    });
    if shown.closed {
        pass.act(Act::SettingCancel);
    }
    let (does, caret) = shown.inner;
    pass.does.extend(does.into_iter().map(Do::Act));
    pass.caret = caret;
}

/// A warning gate: its consequences in gold, the safe choice lit as the
/// primary and the act beside it, the window's edge gold as the cells'
/// is. The cells' gate has no close; the kit's window does, and it is Esc,
/// the safe choice, which the gate's tips name.
#[allow(clippy::too_many_arguments)]
fn gate(ui: &Ui, id: Id, over: Rect, title: &str, lines: [String; 2], safe: (String, Act), go: (String, Act), pal: &Pal, pass: &mut Pass) {
    let shown = modal(ui, id, over, MODAL_W, title, pal, |ui| {
        let w = ui.available_width();
        for words in &lines {
            para(ui, w, &[(words.clone(), body(14.0), pal.gold)]);
            ui.add_space(4.0);
        }
        ui.add_space(18.0);
        form_foot(ui, id.with("foot"), None, &[(safe.0.clone(), true), (go.0.clone(), false)], pal)
    });
    let edge = ui.ctx().layer_painter(LayerId::new(Order::Foreground, id));
    edge.rect_stroke(shown.window, square(), Stroke::new(1.0, pal.gold), StrokeKind::Inside);
    if shown.closed {
        pass.does.push(Do::Key(KeyCode::Esc));
    }
    match shown.inner {
        Some(0) => pass.act(safe.1),
        Some(_) => pass.act(go.1),
        None => {}
    }
}

/// The blocked list: each endpoint id whole, the list's cursor ringed; a
/// click on the cursor's row unblocks it, on another moves the cursor.
fn blocked(ui: &Ui, room: &Room, over: Rect, sel: usize, pal: &Pal, pass: &mut Pass) {
    let ids: &[String] = room.status.as_ref().map(|s| s.blocked_peers.as_slice()).unwrap_or(&[]);
    let shown = modal(ui, Id::new("disc-blocked"), over, BLOCKED_W, &t!("p2p.blocked_title"), pal, |ui| {
        let w = ui.available_width();
        let mut picked = None;
        if ids.is_empty() {
            line(ui, 34.0, &t!("p2p.blocked_empty"), body(13.5), pal.dim);
        }
        for (i, id) in ids.iter().enumerate() {
            let on = i == sel;
            let (rect, response) = pick_row(ui, Id::new("disc-blocked-row").with(i), 34.0, on, pal);
            let at = pos2(rect.min.x + 12.0, rect.center().y);
            text(ui, at, &printable(id, 64), mono(12.5), pal.text, rect.width() - 24.0, Align2::LEFT_CENTER);
            if response.clicked() {
                picked = Some(if on { Act::BlockedRun } else { Act::BlockedPick(i) });
            }
            ui.add_space(2.0);
        }
        ui.add_space(14.0);
        let (foot, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        text(ui, foot.right_center(), &t!("p2p.blocked_unblock"), body(13.0), pal.dim, w, Align2::RIGHT_CENTER);
        picked
    });
    if shown.closed {
        pass.act(Act::BlockedClose);
    }
    if let Some(act) = shown.inner {
        pass.act(act);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton, RawInput, Vec2};

    use super::super::tests::*;
    use super::*;
    use crate::api::types::DiscoveryRecovery;
    use crate::gui::window::studio::begin;
    use crate::kit::clipboard::{Copied, catch};

    /// The room's region at the window's opening size (100×30 cells of 8×16
    /// points, less the hallway, the bars and the banner's row), and at the
    /// canvas's.
    const SMALL: Vec2 = vec2(664.0, 352.0);
    const LARGE: Vec2 = vec2(1184.0, 760.0);

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        ctx
    }

    /// One frame of the room over a region of `size` with `events`, as the
    /// host runs it: the kit's ground, then the room. egui may run the pass
    /// twice (a modal taking its body's height): an outcome either one
    /// reports stands, and the caret is the last one's.
    fn frame(ctx: &egui::Context, room: &mut Room, size: Vec2, events: Vec<Event>, composition: &str) -> (egui::FullOutput, EguiShown) {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
        let (mut caret, mut outcome) = (None, None);
        let mut out = ctx.run_ui(input, |ui| {
            let _ = begin(ui);
            let shown = show(ui, room, composition, true);
            caret = shown.caret;
            outcome = outcome.or(shown.outcome);
        });
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        (out, EguiShown { caret, outcome })
    }

    /// Two quiet frames: egui sizes a modal on the first.
    fn settle(ctx: &egui::Context, room: &mut Room, size: Vec2) -> (egui::FullOutput, EguiShown) {
        let _ = frame(ctx, room, size, Vec::new(), "");
        frame(ctx, room, size, Vec::new(), "")
    }

    /// A press and a release at `at`, a frame each after the move, as hands
    /// would make them. What the release's frame told the host.
    fn click(ctx: &egui::Context, room: &mut Room, size: Vec2, at: Pos2) -> EguiShown {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
        let _ = frame(ctx, room, size, vec![Event::PointerMoved(at)], "");
        let _ = frame(ctx, room, size, vec![button(true)], "");
        frame(ctx, room, size, vec![button(false)], "").1
    }

    /// Where the last frame drew the control `id`.
    fn spot(ctx: &egui::Context, id: Id) -> Rect {
        ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect
    }

    /// Every text the frame drew, as laid out.
    fn drawn(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Where the frame drew `words` whole, the first time.
    fn found(out: &egui::FullOutput, words: &str) -> Option<Rect> {
        out.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.text() == words => Some(text.visual_bounding_rect()),
            _ => None,
        })
    }

    fn has(words: &[String], s: &str) -> bool {
        words.iter().any(|w| w.contains(s))
    }

    #[test]
    fn on_the_network_the_room_draws_the_cells_words_at_both_sizes() {
        let _en = english();
        for size in [SMALL, LARGE] {
            let ctx = context();
            let mut room = on();
            let (out, shown) = settle(&ctx, &mut room, size);
            let words = drawn(&out);
            assert!(words.iter().any(|w| w.starts_with("connected — 3 mesh neighbors")), "{size:?}: the state, its bullet an LED: {words:?}");
            for word in ["STATS", "ACTIVITY", "INVITE", "CONFIG", "MESH NEIGHBORS", "SERVERS KNOWN", "SNAPSHOTS HELD", "SERVERS YOU FOLLOW"] {
                assert!(words.iter().any(|w| w == word), "{word} at {size:?}: {words:?}");
            }
            for word in ["3 of 6", "23.0 MB of 500.0 MB", "24,569", "1 hidden · 1 blocked", "ceiling 256 MB · 0 restarts"] {
                assert!(words.iter().any(|w| w == word), "{word} at {size:?}: {words:?}");
            }
            for word in ["SERVER", "ONLINE", "SNAPSHOT", "FEDERATION", "Basement Archive", "federated", "/ filter", "H SHOW", "U UNBLOCK"] {
                assert!(words.iter().any(|w| w == word), "{word} at {size:?}: {words:?}");
            }
            assert!(!has(&words, &t!("p2p.title")) && !has(&words, &t!("p2p.hint_rows")), "no header and no tips: the host's");
            assert_eq!(shown.caret, None, "no field has the keyboard");
            assert!(shown.outcome.is_none());
        }
        // The canvas's size has room for every fact, column and row.
        let ctx = context();
        let mut room = on();
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        for word in [
            "connected — 3 mesh neighbors · sidecar 19 MB · announcing as mStream Den",
            "polls every 10 s",
            "TRACKS",
            "SEEDERS",
            "12,204",
            "downloaded · pinned",
            "update available",
            "they asked you",
            "offline · 3d",
            "request sent",
            "jazz-corner",
            "1 hidden — incompatible model",
            "1 blocked",
        ] {
            assert!(words.iter().any(|w| w == word), "{word}: {words:?}");
        }
        // The cursor's row: its server's words ride the foot.
        handle_key(&mut room, key(KeyCode::Down));
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(has(&words, "Basement Archive · Northern soul, dub and a wall of 7-inch singles."), "{words:?}");
    }

    #[test]
    fn every_tab_and_every_state_of_the_room_draws_its_words() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        room.act(Act::Tab(Tab::Activity));
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(has(&words, &t!("p2p.activity_empty")) && has(&words, "newest first · full history in the server logs"), "{words:?}");
        let now = unix_now();
        let entry = |seq: u64, ago: i64, level: &str, message: &str| ActivityEntry { seq, t: iso_at(now - ago), level: level.into(), message: message.into() };
        let entries = vec![
            entry(1, 3 * 3600, "info", "sidecar started (pid 48211)"),
            entry(2, 61 * 60, "warn", "rotation found nothing fetchable under the cap"),
            entry(3, 3 * 60, "info", "Basement Archive announced a new snapshot (seq 40 -> 41)"),
        ];
        room.apply(Done::Activity(Ok(DiscoveryActivity { entries, last_seq: 3 })));
        let out = settle(&ctx, &mut room, LARGE).0;
        let newest = found(&out, "Basement Archive announced a new snapshot (seq 40 -> 41)").expect("the newest line");
        let oldest = found(&out, "sidecar started (pid 48211)").expect("the oldest line");
        assert!(newest.min.y < oldest.min.y, "newest first");
        let words = drawn(&out);
        assert!(["3m", "1h", "3h"].iter().all(|age| words.iter().any(|w| w == age)), "{words:?}");

        room.act(Act::Tab(Tab::Invite));
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        let ticket = room.status.as_ref().and_then(|s| s.ticket.clone()).expect("a ticket");
        let endpoint = hex("51403e7b9c");
        for word in ["ENDPOINT", "YOUR TICKET", "BEFRIEND A SERVER", "Y COPY", endpoint.as_str(), ticket.as_str()] {
            assert!(words.iter().any(|w| w == word), "{word}: {words:?}");
        }

        room.act(Act::Tab(Tab::Config));
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        for word in [
            "ANNOUNCING AS",
            "mStream Den — Vinyl, jazz and field recordings",
            "E EDIT",
            "Snapshot storage cap",
            "500 MB",
            "after 30 days of silence",
            "up to 6 servers",
            "after 7 days",
            "256 MB",
            "Community seeds",
            "on",
        ] {
            assert!(words.iter().any(|w| w == word), "{word}: {words:?}");
        }

        // Waiting for neighbours, with the searching bar; reconnecting.
        let mut s = status_on();
        s.neighbors = 0;
        room.apply(loaded(s.clone()));
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(has(&words, "joined, waiting for neighbors") && has(&words, "searching for peers — this room updates itself every 10 s"), "{words:?}");
        s.recovery = DiscoveryRecovery { attempts: 2, retry_pending: true };
        room.apply(loaded(s));
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(has(&words, "reconnecting — sidecar died, replaying (attempt 2)"), "{words:?}");
        assert!(has(&words, "crash recovery replays the stack on a widening ladder — no action needed"), "{words:?}");

        // No servers heard yet; a filter that matches nothing.
        room.catalog.peers.clear();
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(has(&words, &t!("p2p.empty_servers")) && words.iter().any(|w| w == "SERVER"), "the headings over the empty line: {words:?}");
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('/')));
        type_text(&mut room, "nothing like it");
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(has(&words, &t!("p2p.empty_filter")), "{words:?}");

        // Loading: nothing yet but the busy line at the foot; then a failed
        // load's note in its place.
        let mut first = super::super::tests::room();
        first.reload(true);
        let words = drawn(&settle(&ctx, &mut first, LARGE).0);
        assert!(has(&words, &t!("p2p.busy_loading")) && !words.iter().any(|w| w == "STATS"), "{words:?}");
        first.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
        let words = drawn(&settle(&ctx, &mut first, LARGE).0);
        assert!(has(&words, "locked"), "{words:?}");
    }

    #[test]
    fn off_the_network_the_join_page_joins_and_flips_the_opt_in() {
        let _en = english();
        let ctx = context();
        let mut room = super::super::tests::room();
        room.apply(loaded(status_off()));
        let words = drawn(&settle(&ctx, &mut room, SMALL).0);
        for word in [
            "not on the discovery network · The p2p-sidecar will be downloaded when you join.",
            "Join the discovery network",
            "Also let other servers send me federation requests",
            "Invitations to share libraries; nothing is shared until you say yes. Turns federation on.",
            "WHAT GETS SHARED",
        ] {
            assert!(words.iter().any(|w| w == word), "{word}: {words:?}");
        }
        assert!(has(&words, "Never any audio files."), "{words:?}");
        click(&ctx, &mut room, SMALL, spot(&ctx, Id::new("disc-requests")).center());
        assert!(!room.accept_requests, "the tick flips it");
        let card = spot(&ctx, Id::new("disc-requests-card"));
        click(&ctx, &mut room, SMALL, pos2(card.max.x - 20.0, card.center().y));
        assert!(room.accept_requests, "and so does the card's empty end");
        let shown = click(&ctx, &mut room, SMALL, spot(&ctx, Id::new("disc-join")).center());
        assert_eq!(room.queued, Some(Op::JoinNetwork { accept_requests: true }), "the card joins at once");
        assert!(shown.outcome.is_none());

        let mut s = status_off();
        s.binary_fetchable = false;
        room.apply(loaded(s));
        let words = drawn(&settle(&ctx, &mut room, SMALL).0);
        assert!(words.iter().any(|w| w == "unavailable — the p2p-sidecar binary was not found for this platform"), "{words:?}");
        assert!(!words.iter().any(|w| w == "Join the discovery network") && has(&words, "Never any audio files."), "the pitch alone: {words:?}");
        assert!(!has(&words, "Also let other servers send me federation requests"), "{words:?}");
    }

    #[test]
    fn a_click_on_a_row_opens_its_sheet_and_the_sheets_rows_act_as_the_cells() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let out = settle(&ctx, &mut room, LARGE).0;
        let jazz = found(&out, "jazz-corner").expect("jazz-corner's row");
        click(&ctx, &mut room, LARGE, jazz.center());
        let Modal::Sheet { id, sel: 0 } = &room.modal else { panic!("the sheet, its cursor on top: {:?}", room.modal) };
        assert_eq!(*id, hex("7e11aa93b0"));
        let (out, shown) = settle(&ctx, &mut room, LARGE);
        let words = drawn(&out);
        for word in [
            "Download the snapshot",
            "Ask to federate",
            "Forget this server",
            "Block this server",
            "ENDPOINT",
            "offline · 6,010 tracks · 1 seeders",
            "not downloaded",
            "Hard bop through spiritual jazz.",
        ] {
            assert!(words.iter().any(|w| w == word), "{word}: {words:?}");
        }
        assert_eq!(shown.caret, None, "no field in the sheet");
        // A click on another row moves the list's cursor; on the cursor's,
        // runs it.
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-sheet-act").with(2)).center());
        assert!(matches!(room.modal, Modal::Sheet { sel: 2, .. }), "{:?}", room.modal);
        let _ = settle(&ctx, &mut room, LARGE);
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-sheet-act").with(2)).center());
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(room.queued, Some(Op::Peer { action: PeerAction::Forget, id: hex("7e11aa93b0"), name: "jazz-corner".into() }));
        // A server that leaves the catalog takes its sheet with it.
        room.act(Act::OpenSheet(4));
        room.catalog.peers.pop();
        let _ = settle(&ctx, &mut room, LARGE);
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn the_tabs_and_the_panes_controls_apply_the_cells_acts() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let _ = settle(&ctx, &mut room, LARGE);
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-tabs").with(2)).center());
        assert_eq!(room.tab, Tab::Invite);
        let _ = settle(&ctx, &mut room, LARGE);
        catch(Copied::Clipboard);
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-copy")).center());
        assert_eq!(room.note, Some((t!("p2p.copied_clipboard").to_string(), false)));
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-ticket")).center());
        assert!(room.ticket_focus, "a click on the box gives it the keys");
        type_text(&mut room, "nodeab7f");
        assert_eq!(room.ticket.value(), "nodeab7f");
        // The open tab takes its click too: it lets go of the box.
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-tabs").with(2)).center());
        assert!(room.tab == Tab::Invite && !room.ticket_focus);
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-tabs").with(3)).center());
        assert_eq!(room.tab, Tab::Config);
        let _ = settle(&ctx, &mut room, LARGE);
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-setting").with(2)).center());
        let Modal::Setting(d) = &room.modal else { panic!("the setting modal: {:?}", room.modal) };
        assert_eq!(d.which, DiscoverySetting::AutoFetchCount);
        room.act(Act::SettingCancel);
        let _ = settle(&ctx, &mut room, LARGE);
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-edit")).center());
        assert!(matches!(room.modal, Modal::Identity(_)));
    }

    #[test]
    fn the_filter_box_and_the_count_buttons_do_what_the_cells_and_their_keys_do() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let _ = settle(&ctx, &mut room, LARGE);
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-filter")).center());
        assert!(room.filter_on && room.filter_focus, "the cells' click on the line opens the filter");
        type_text(&mut room, "jazz");
        assert_eq!(room.rows().len(), 1);
        // While the filter has the keys, the h button types nothing into it.
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-key").with('h')).center());
        assert_eq!((room.filter.value(), room.queued.clone()), ("jazz", None));
        handle_key(&mut room, key(KeyCode::Enter));
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(has(&words, "jazz-corner") && !has(&words, "Basement Archive"), "{words:?}");
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-key").with('h')).center());
        assert_eq!(room.queued, Some(Op::Load { include_incompatible: true, activity_since: None }), "h");
        room.queued = None;
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-key").with('u')).center());
        assert!(matches!(room.modal, Modal::Blocked(0)), "u");
        let words = drawn(&settle(&ctx, &mut room, LARGE).0);
        assert!(words.iter().any(|w| *w == hex("a1b2c3d4e5")) && has(&words, "Enter unblocks"), "{words:?}");
        click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-blocked-row").with(0)).center());
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(room.queued, Some(Op::Peer { action: PeerAction::Unblock, id: hex("a1b2c3d4e5"), name: "a1b2c3d4e5a1…".into() }));
    }

    #[test]
    fn a_focused_field_says_where_its_caret_is_and_none_does_without_the_keyboard() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let caret = |ctx: &egui::Context, room: &mut Room, composition: &str| {
            let _ = frame(ctx, room, LARGE, Vec::new(), composition);
            frame(ctx, room, LARGE, Vec::new(), composition).1.caret
        };
        let inside = |ctx: &egui::Context, id: &str, at: Option<Pos2>| {
            let at = at.unwrap_or_else(|| panic!("{id} has the keyboard: a caret"));
            assert!(spot(ctx, Id::new(id)).contains(at - vec2(0.0, 0.5)), "{id}'s caret in its box: {at:?}");
        };
        assert_eq!(caret(&ctx, &mut room, ""), None);
        handle_key(&mut room, key(KeyCode::Char('/')));
        inside(&ctx, "disc-filter", caret(&ctx, &mut room, ""));
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(caret(&ctx, &mut room, ""), None, "the filter let go of the keys");
        handle_key(&mut room, key(KeyCode::Char('j')));
        inside(&ctx, "disc-ticket", caret(&ctx, &mut room, ""));
        // The input method's text stands before the caret, in the focused
        // field only.
        let plain = caret(&ctx, &mut room, "").expect("a caret");
        let composing = caret(&ctx, &mut room, "かな").expect("a caret");
        assert!(composing.x > plain.x + 8.0, "{plain:?} {composing:?}");
        handle_key(&mut room, key(KeyCode::Esc));
        assert_eq!(caret(&ctx, &mut room, ""), None, "the box let go of the keys");

        handle_key(&mut room, key(KeyCode::Char('e')));
        inside(&ctx, "disc-identity-name", caret(&ctx, &mut room, ""));
        handle_key(&mut room, key(KeyCode::Tab));
        inside(&ctx, "disc-identity-description", caret(&ctx, &mut room, ""));
        handle_key(&mut room, key(KeyCode::Esc));
        handle_key(&mut room, key(KeyCode::Char('3')));
        inside(&ctx, "disc-setting-value", caret(&ctx, &mut room, ""));
        handle_key(&mut room, key(KeyCode::Esc));
        room.act(Act::Federate(hex("7e11aa93b0")));
        room.apply(Done::Libraries(Ok(vec!["music".into(), "vinyl".into()])));
        inside(&ctx, "disc-compose-message", caret(&ctx, &mut room, ""));
        handle_key(&mut room, key(KeyCode::Tab));
        assert_eq!(caret(&ctx, &mut room, ""), None, "a library has the keyboard's focus, not a field");
        handle_key(&mut room, key(KeyCode::Esc));

        // A modal over the room: the room's own field under it has no keys.
        handle_key(&mut room, key(KeyCode::Char('/')));
        room.act(Act::OpenSheet(0));
        assert!(room.filter_focus && room.modal_open());
        assert_eq!(caret(&ctx, &mut room, ""), None);
    }

    #[test]
    fn each_modal_draws_and_its_close_works() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let jazz = hex("7e11aa93b0");
        type Open = fn(&mut Room, &str);
        let cases: [(Open, &str, &str); 7] = [
            (|room, _| _ = room.act(Act::OpenSheet(0)), "disc-sheet", "Update the snapshot"),
            (
                |room, jazz| {
                    let _ = room.act(Act::Federate(jazz.to_string()));
                    room.apply(Done::Libraries(Ok(vec!["music".into()])));
                },
                "disc-compose",
                "Ask jazz-corner to federate",
            ),
            (|room, _| _ = room.act(Act::IdentityOpen), "disc-identity", "Server identity on the network"),
            (
                |room, _| _ = room.act(Act::SettingOpen(DiscoverySetting::RotationDays)),
                "disc-setting-form",
                "DAYS BEFORE A DOWNLOAD MAY BE SWAPPED · 0 = NEVER",
            ),
            (|room, jazz| _ = room.act(Act::Block(jazz.to_string())), "disc-block", "Block jazz-corner?"),
            (|room, _| _ = room.act(Act::Leave), "disc-leave", "Leave the discovery network?"),
            (|room, _| _ = room.act(Act::BlockedOpen), "disc-blocked", "Enter unblocks"),
        ];
        for (open, id, word) in cases {
            open(&mut room, &jazz);
            room.queued = None;
            let words = drawn(&settle(&ctx, &mut room, LARGE).0);
            assert!(words.iter().any(|w| w == word), "{id}: {word}: {words:?}");
            click(&ctx, &mut room, LARGE, spot(&ctx, Id::new(id).with("close")).center());
            assert!(matches!(room.modal, Modal::None), "{id} closed: {:?}", room.modal);
            assert!(room.queued.is_none(), "{id}: closing sends nothing");
        }
    }

    #[test]
    fn the_forms_controls_apply_the_cells_acts_and_no_click_ends_the_room() {
        let _en = english();
        let ctx = context();
        let mut room = on();
        let jazz = hex("7e11aa93b0");
        let mut outcomes = Vec::new();
        // The compose: a tick flips its library and takes the keyboard's
        // focus there; a click on the message takes it back; Send sends
        // what is ticked.
        room.act(Act::Federate(jazz.clone()));
        room.apply(Done::Libraries(Ok(vec!["music".into(), "vinyl".into()])));
        let _ = settle(&ctx, &mut room, LARGE);
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-compose-lib").with(1)).center()).outcome);
        let Modal::Federate(c) = &room.modal else { panic!("the compose") };
        assert_eq!((c.libraries[1].1, c.focus), (false, Some(1)));
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-compose-message")).center()).outcome);
        let Modal::Federate(c) = &room.modal else { panic!("the compose") };
        assert_eq!(c.focus, None);
        type_text(&mut room, "Swap?");
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-compose-send").with(0)).center()).outcome);
        let sent = Op::SendRequest { id: jazz.clone(), name: "jazz-corner".into(), offer: vec!["music".into()], message: "Swap?".into() };
        assert_eq!(room.queued, Some(sent));
        room.queued = None;
        room.modal = Modal::None;

        // The identity: a click on a field gives it the keys; Save with
        // nothing changed closes it and sends nothing.
        room.act(Act::IdentityOpen);
        let _ = settle(&ctx, &mut room, LARGE);
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-identity-description")).center()).outcome);
        assert!(matches!(&room.modal, Modal::Identity(d) if d.on_description));
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-identity-name")).center()).outcome);
        assert!(matches!(&room.modal, Modal::Identity(d) if !d.on_description));
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-identity-foot").with(0)).center()).outcome);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none());

        // The setting: a click on its box saves, as one on the cells' box
        // does; so does Save.
        room.act(Act::SettingOpen(DiscoverySetting::AutoFetchCount));
        let _ = settle(&ctx, &mut room, LARGE);
        let saved = Some(Op::Setting { which: DiscoverySetting::AutoFetchCount, value: 6 });
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-setting-value")).center()).outcome);
        assert_eq!(room.queued, saved);
        room.queued = None;
        outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new("disc-setting-foot").with(0)).center()).outcome);
        assert_eq!(room.queued, saved);
        room.queued = None;
        room.modal = Modal::None;

        // The gates: the safe choice cancels, the other acts.
        for (open, id, done) in [
            (Act::Block(jazz.clone()), "disc-block", Op::Peer { action: PeerAction::Block, id: jazz.clone(), name: "jazz-corner".into() }),
            (Act::Leave, "disc-leave", Op::LeaveNetwork),
        ] {
            room.act(open.clone());
            let _ = settle(&ctx, &mut room, LARGE);
            outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new(id).with("foot").with(0)).center()).outcome);
            assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "{id}: the safe choice");
            room.act(open);
            let _ = settle(&ctx, &mut room, LARGE);
            outcomes.push(click(&ctx, &mut room, LARGE, spot(&ctx, Id::new(id).with("foot").with(1)).center()).outcome);
            assert_eq!(room.queued, Some(done), "{id}");
            room.queued = None;
        }
        assert!(outcomes.iter().all(Option::is_none), "no click ends the room, as none in the cells does");
    }
}
