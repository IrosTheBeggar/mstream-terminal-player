//! The Backups room drawn by egui, on the window's panel over the cells the
//! room's hosted draw would fill. It is the same [`Room`] the cells view
//! draws (the parent module: its state, its worker, its keys, its words);
//! only the drawing is new, in pixels, in the Studio kit's look
//! (src/gui/window/studio.rs) after the canvas's Backups boards. Every
//! click is one of the room's own [`Act`]s, the very one the cells view
//! registers for the same control, handed to the room after the pass; the
//! keys still come to [`Room::key`], so a field is drawn from the room's own
//! value and caret, and the two faces cannot disagree about what the room
//! does. What the room says in words is the parent's, called here with the
//! keys the cells view calls, never copied.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::epaint::StrokeKind;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Id, LayerId, Layout, Order, Pos2, Rect, ScrollArea, Sense, Shape, Stroke,
    Ui, UiBuilder, pos2, vec2,
};

use super::super::{EguiShown, age_text, iso_unix, unix_now};
use super::*;
use crate::api::types::ActiveBackup;
use crate::gui::window::studio::{
    self, Col, FIELD_H, FORM_BUTTON_H, Pal, Table, add_card, begin, body, cell, checkbox, chip, field_box, form_button,
    form_button_width, ink, led, meter, meter_lit, modal, mono, note_line, radio, ring, small_caps, square, strong, text,
    width_of,
};

/// The room's margins inside its region, in points: the canvas's 24 at the
/// sides and 14 at the foot; at the head less than its 20, the host's
/// banner row standing above the region already.
const PAD_X: f32 = 24.0;
const PAD_TOP: f32 = 10.0;
const PAD_FOOT: f32 = 14.0;
/// The canvas's room between the room's blocks, the note line's among them.
const GAP: f32 = 18.0;
/// The note line along the room's foot.
const NOTE_H: f32 = 18.0;
/// The form's room between its blocks, and between a label and its field.
const FORM_GAP: f32 = 20.0;
const LABEL_GAP: f32 = 8.0;
/// The modals' widths: the canvas's form, and the others to sit with it.
const FORM_W: f32 = 640.0;
const BROWSER_W: f32 = 560.0;
const HISTORY_W: f32 = 760.0;
const GATE_W: f32 = 560.0;
/// The history's rows, and how many show at once (the cells' eight).
const RUN_ROW: f32 = 32.0;
const RUNS_SHOWN: usize = 8;
/// The folder browser's rows, and how many show at once (the cells' eleven).
const DIR_ROW: f32 = 30.0;
const DIRS_SHOWN: usize = 11;
/// A number field's box, as the canvas draws it.
const NUMBER_W: f32 = 120.0;

/// One pass of the room over the whole of `ui`, the region its host hands
/// it, `composition` the input method's uncommitted text for the field that
/// has the keyboard. The pass's clicks are applied after it through
/// [`Room::act`] (the remove gate's close as the Esc it stands for), and
/// what the host must know comes back: the focused field's caret, and an
/// outcome a click brought, which none in this room does.
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    let pal = begin(ui);
    let full = ui.max_rect();
    let column = Rect::from_min_max(pos2(full.min.x + PAD_X, full.min.y + PAD_TOP), pos2(full.max.x - PAD_X, full.max.y - PAD_FOOT));
    let note_rect = Rect::from_min_max(pos2(column.min.x, column.max.y - NOTE_H), column.max);
    let body_rect = Rect::from_min_max(column.min, pos2(column.max.x, note_rect.min.y - GAP));

    let mut acts: Vec<Act> = Vec::new();
    let mut base = ui.new_child(UiBuilder::new().id_salt("backups-body").max_rect(body_rect).layout(Layout::top_down(Align::Min)));
    base.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
    state(&mut base, room, &pal);
    if room.loaded {
        base.add_space(GAP);
        if add_card(&mut base, Id::new("backups-add"), &t!("bak.add_card"), &pal).clicked() {
            acts.push(Act::Add);
        }
        base.add_space(GAP);
        table(&mut base, room, &pal, &mut acts);
    }
    foot(ui, note_rect, room, &pal);

    // A modal makes the room beneath inert: what was clicked under it this
    // pass is dropped, as the cells view drops its registrations.
    let mut keys: Vec<KeyCode> = Vec::new();
    let mut caret = None;
    if !matches!(room.modal, Modal::None) {
        acts.clear();
    }
    match &room.modal {
        Modal::None => {}
        Modal::Form(f) => caret = form(ui, room, f, full, composition, &pal, &mut acts),
        Modal::Browser { browse, .. } => browser(ui, browse, full, &pal, &mut acts),
        Modal::History { dest, runs, loaded, sel } => history(ui, dest, runs, *loaded, *sel, full, &pal, &mut acts),
        &Modal::Remove(id) => gate(ui, room, id, full, &pal, &mut acts, &mut keys),
    }

    let acted = !acts.is_empty() || !keys.is_empty();
    let mut outcome = None;
    for act in acts {
        if let Some(out) = room.act(act) {
            outcome.get_or_insert(out);
        }
    }
    for code in keys {
        if let Some(out) = room.key(KeyEvent::new(code, KeyModifiers::NONE)) {
            outcome.get_or_insert(out);
        }
    }
    if acted {
        // The room changed after this pass drew it: the next pass shows it,
        // and a form a click closed leaves no field with the keyboard.
        ui.ctx().request_repaint();
        if !matches!(room.modal, Modal::Form(_)) {
            caret = None;
        }
    }
    EguiShown { caret, outcome }
}

// ── The state line ────────────────────────────────────────────────────────

/// A state's word without the bullet the cells set before it: the LED
/// stands there instead.
fn unbullet(word: &str) -> &str {
    word.trim_start_matches('•').trim_start()
}

/// The state line: what the room is doing behind its LED (a run in flight,
/// tasks queued, or idle), the cadence where it fits, and the gold BETA
/// chip at the right, which stands there while the room loads too; under a
/// run in flight, its progress.
fn state(ui: &mut Ui, room: &Room, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    let y = row.center().y;
    let beta = chip(ui, row.right_center(), Align2::RIGHT_CENTER, &t!("bak.beta"), pal.gold);
    if !room.loaded {
        return;
    }
    let now = unix_now();
    // The word in the semibold face and its colour while something runs or
    // waits, as the cells set it bold; idle, plain and dim.
    let (word, detail, tone, lamp, polls) = match &room.status.active {
        Some(a) => {
            let elapsed = a.started_at.as_deref().and_then(iso_unix).map(|t| elapsed_text(now - t)).unwrap_or_default();
            let trigger = a.trigger_reason.as_deref().map(trigger_words).unwrap_or_default();
            let detail = t!(
                "bak.state_running_detail",
                library = printable(a.library_name.as_deref().unwrap_or(""), 64),
                path = a.dest_path.clone().unwrap_or_default(),
                trigger = trigger,
                elapsed = elapsed
            );
            (t!("bak.state_running"), detail, pal.ok, pal.ok, t!("bak.polls_2"))
        }
        None if room.status.queue_length > 0 => {
            let n = room.status.queue_length;
            let word = if n == 1 { t!("bak.state_queued_one") } else { t!("bak.state_queued", n = n) };
            (word, t!("bak.state_queued_detail"), pal.gold, pal.gold, t!("bak.polls_2"))
        }
        None => {
            let on = room.dests.iter().filter(|d| d.enabled).count();
            let detail = t!("bak.state_idle_detail", on = on, total = room.dests.len());
            (t!("bak.state_idle"), detail, pal.dim, pal.unlit, t!("bak.polls_5"))
        }
    };
    let (word, detail) = (unbullet(&word), detail.trim_start());
    let font = if room.status.active.is_some() || room.status.queue_length > 0 { strong(14.0) } else { body(14.0) };
    let x = row.min.x + 16.0;
    let state_w = 16.0 + width_of(ui, word, font.clone()) + 10.0 + width_of(ui, detail, body(14.0));
    let mut right = beta.min.x - 16.0;
    // The cadence only where the whole state fits beside it, as the cells
    // show it.
    let polls_w = width_of(ui, &polls, body(13.0));
    if state_w + 16.0 + polls_w <= right - row.min.x {
        text(ui, pos2(right, y), &polls, body(13.0), pal.dim, polls_w + 1.0, Align2::RIGHT_CENTER);
        right -= polls_w + 16.0;
    }
    led(ui, pos2(row.min.x + 3.0, y), lamp);
    let said = text(ui, pos2(x, y), word, font, tone, (right - x).max(1.0), Align2::LEFT_CENTER);
    let from = said.max.x + 10.0;
    if from < right {
        text(ui, pos2(from, y), detail, body(14.0), pal.text, right - from, Align2::LEFT_CENTER);
    }
    if let Some(a) = &room.status.active {
        progress(ui, a, pal);
    }
}

/// A run's progress under the state line: the ten-segment meter, a segment
/// lit green for each whole tenth done (all unlit on a destination's first
/// run, which has no estimate), then the percentage and the counts, whose
/// groups drop off the right where the column is narrow.
fn progress(ui: &mut Ui, a: &ActiveBackup, pal: &Pal) {
    ui.add_space(10.0);
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    let done = a.files_copied + a.files_unchanged + a.files_trashed;
    let expected = a.expected_files.filter(|e| *e > 0);
    let lit = expected.map_or(0, |e| meter_lit(done, e, 10));
    let bar = meter(ui, pos2(row.min.x + 16.0, row.center().y), 10, lit, vec2(14.0, 11.0), 3.0, pal.ok, pal);
    let head = match expected {
        Some(e) => {
            let pct = ((done * 100) / e).min(100);
            t!("bak.progress_pct", pct = pct, done = fmt_count(done), expected = fmt_count(e)).to_string()
        }
        None => t!("bak.progress_no_estimate").to_string(),
    };
    let written = if a.bytes_copied > 0 {
        t!("bak.progress_written", bytes = fmt_bytes(a.bytes_copied)).to_string()
    } else {
        t!("bak.progress_no_bytes").to_string()
    };
    let mut groups = vec![
        t!("bak.sum_copied", n = fmt_count(a.files_copied)).to_string(),
        t!("bak.sum_unchanged", n = fmt_count(a.files_unchanged)).to_string(),
        t!("bak.sum_trashed", n = fmt_count(a.files_trashed)).to_string(),
        written,
    ];
    let line = |groups: &[String]| -> String {
        let mut s = head.trim_start().to_string();
        for group in groups {
            s.push_str(&format!(" · {group}"));
        }
        s
    };
    let from = bar.max.x + 12.0;
    let room_w = row.max.x - from;
    while !groups.is_empty() && width_of(ui, &line(&groups), body(13.0)) > room_w {
        groups.pop();
    }
    text(ui, pos2(from, row.center().y), &line(&groups), body(13.0), pal.dim, room_w.max(1.0), Align2::LEFT_CENTER);
}

// ── The destinations ──────────────────────────────────────────────────────

/// A run's status in its colour: success green; failed, partial and skipped
/// gold; running the accent; anything else dim.
fn status_tone(status: &str, pal: &Pal) -> Color32 {
    match status {
        "success" => pal.ok,
        "failed" | "partial" | "skipped" => pal.gold,
        "running" => pal.accent,
        _ => pal.dim,
    }
}

/// `s` cut from its start to fit `max_w` in `font`, an ellipsis where it
/// was cut, so a path keeps its leaf in view; and whether it was cut.
fn tail_fit(ui: &Ui, s: &str, font: FontId, max_w: f32) -> (String, bool) {
    if width_of(ui, s, font.clone()) <= max_w {
        return (s.to_string(), false);
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |drop: usize| -> String { std::iter::once('…').chain(chars[drop..].iter().copied()).collect() };
    // The fewest characters dropped that make it fit.
    let (mut lo, mut hi) = (1, chars.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if width_of(ui, &cut(mid), font.clone()) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    (cut(lo), true)
}

/// The destinations under their headings, in server order: the library,
/// the path cut from its start (the whole of it in a tooltip where it was
/// cut), the trigger, the retention, the last run behind an LED in its
/// status colour, and the ON box. A click on a row selects it; a click on
/// its box turns the destination on or off.
fn table(ui: &mut Ui, room: &Room, pal: &Pal, acts: &mut Vec<Act>) {
    // The canvas's widths, narrowed together where the column is narrow so
    // the destination keeps some room; the row's padding and the gaps are
    // the kit table's.
    let w = ui.available_width() - 20.0;
    let want = [150.0, 150.0, 110.0, 170.0];
    let on_w = 48.0;
    let gaps = 5.0 * 16.0;
    let k = ((w - 160.0 - on_w - gaps) / want.iter().sum::<f32>()).clamp(0.6, 1.0);
    let cols = [
        Col::left(t!("bak.col_library"), want[0] * k),
        Col::left(t!("bak.col_destination"), 0.0),
        Col::left(t!("bak.col_trigger"), want[1] * k),
        Col::left(t!("bak.col_retention"), want[2] * k),
        Col::left(t!("bak.col_last_run"), want[3] * k),
        Col::center(t!("bak.col_on"), on_w),
    ];
    let top = ui.cursor().min;
    let now = unix_now();
    let running = room.running_id();
    let pct = room.status.active.as_ref().and_then(|a| {
        a.expected_files.filter(|e| *e > 0).map(|e| ((a.files_copied + a.files_unchanged + a.files_trashed) * 100 / e).min(100))
    });
    let mut toggled = None;
    let shown = Table::new(Id::new("backups-table"), &cols, room.dests.len()).cursor(room.sel).show(ui, pal, |ui, i, row| {
        let d = &room.dests[i];
        let y = row.rect.center().y;
        let name = if row.selected { strong(14.0) } else { body(14.0) };
        cell(ui, row, 0, &printable(&d.library_name, 64), name, ink(row, pal.text, pal), false);
        let (x, cw) = row.cells[1];
        let (path, cut) = tail_fit(ui, &d.dest_path, mono(12.5), cw);
        text(ui, pos2(x, y), &path, mono(12.5), ink(row, pal.soft, pal), cw + 1.0, Align2::LEFT_CENTER);
        if cut {
            let _ = ui.interact(row.cell(1), Id::new("backups-path").with(d.id), Sense::hover()).on_hover_text(d.dest_path.as_str());
        }
        cell(ui, row, 2, &trigger_cell(d), body(13.5), ink(row, pal.soft, pal), false);
        if d.retention_days == 0 {
            cell(ui, row, 3, &t!("bak.no_trash"), body(13.5), ink(row, pal.dim, pal), false);
        } else {
            cell(ui, row, 3, &t!("bak.days", n = d.retention_days), mono(12.5), ink(row, pal.text, pal), false);
        }
        let (words, tone, lamp) = if running == Some(d.id) {
            let words = match pct {
                Some(p) => t!("bak.running_pct", pct = p).to_string(),
                None => t!("bak.running").to_string(),
            };
            (words, pal.accent, pal.accent)
        } else {
            match &d.last_run {
                None => (t!("bak.never").to_string(), pal.dim, pal.unlit),
                Some(run) => {
                    let age = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
                    let tone = status_tone(&run.status, pal);
                    (format!("{} · {}", status_word(&run.status), age), tone, tone)
                }
            }
        };
        let (x, cw) = row.cells[4];
        led(ui, pos2(x + 3.0, y), lamp);
        text(ui, pos2(x + 14.0, y), &words, body(13.5), ink(row, tone, pal), (cw - 14.0).max(1.0), Align2::LEFT_CENTER);
        if checkbox(ui, Id::new("backups-on").with(d.id), row.cell(5).center(), d.enabled, pal).clicked() {
            toggled = Some(d.id);
        }
    });
    if room.dests.is_empty() {
        // Under the headings and their hairline, where the first row goes.
        let at = pos2(top.x + 10.0, top.y + 34.0 + 20.0);
        text(ui, at, &t!("bak.empty"), body(14.0), pal.dim, w, Align2::LEFT_CENTER);
    }
    if let Some(i) = shown.row {
        acts.push(Act::Select(i));
    }
    if let Some(id) = toggled {
        acts.push(Act::ToggleEnabled(id));
    }
}

/// The room's foot: what it is busy with in the accent, else its note,
/// else the cursor row's extras (its path, its throttle, its patterns and
/// its last run's word), as the cells ride them on the note line.
fn foot(ui: &Ui, rect: Rect, room: &Room, pal: &Pal) {
    if room.busy.is_some() || room.note.is_some() {
        note_line(ui, rect, room.busy.as_deref(), room.note.as_ref(), pal);
        return;
    }
    let Some(d) = room.selected().filter(|_| room.loaded) else { return };
    let mut spans: Vec<(String, Color32)> = vec![(d.dest_path.clone(), pal.text)];
    if d.inter_file_delay_ms > 0 {
        spans.push((format!(" · {}", t!("bak.note_throttle", ms = d.inter_file_delay_ms)), pal.text));
    }
    let excludes = if d.exclude_globs == room.defaults {
        t!("bak.note_default_excludes").to_string()
    } else if d.exclude_globs.is_empty() {
        t!("bak.note_no_excludes").to_string()
    } else {
        t!("bak.note_patterns", n = d.exclude_globs.len()).to_string()
    };
    spans.push((format!(" · {excludes}"), pal.text));
    if let Some(run) = &d.last_run {
        match run.status.as_str() {
            "failed" | "partial" => {
                let why = printable(run.error_message.as_deref().unwrap_or(""), 200);
                spans.push((format!(" · {}: {why}", status_word(&run.status)), pal.gold));
            }
            _ => spans.push((format!(" · {}", run_summary(run)), pal.dim)),
        }
    }
    let mut x = rect.min.x;
    for (words, color) in spans {
        if x >= rect.max.x {
            break;
        }
        x = text(ui, pos2(x, rect.center().y), &words, body(13.0), color, rect.max.x - x, Align2::LEFT_CENTER).max.x;
    }
}

// ── Pieces the kit has not got ────────────────────────────────────────────

/// `s` wrapped to the layout's width in at most `rows` lines, the last cut
/// with an ellipsis, laid in as a row of its own: the server's sentences.
fn prose(ui: &mut Ui, s: &str, font: FontId, color: Color32, rows: usize) {
    let w = ui.available_width();
    let mut job = LayoutJob::single_section(s.to_string(), TextFormat { font_id: font, color, ..Default::default() });
    job.wrap = TextWrapping { max_width: w, max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

/// A field's label, a 14-point row of the layout: its words up to the first
/// " · " in the desk's small capitals, the rest after them dim in the UI
/// face (the canvas's "DESTINATION · another drive, …").
fn label_row(ui: &mut Ui, s: &str, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    let (head, tail) = match s.find(" · ") {
        Some(at) => (&s[..at], s[at..].trim_start()),
        None => (s, ""),
    };
    let drawn = small_caps(ui, row.left_center(), head, pal.dim, w, Align2::LEFT_CENTER);
    let from = drawn.max.x + 6.0;
    if !tail.is_empty() && from < row.max.x {
        text(ui, pos2(from, row.center().y), tail, body(13.0), pal.dim, row.max.x - from, Align2::LEFT_CENTER);
    }
}

/// The canvas's check mark in its 10-point box, centred on `at`.
fn tick(ui: &Ui, at: Pos2, color: Color32) {
    let o = at - vec2(5.0, 5.0);
    let points = vec![o + vec2(1.5, 5.2), o + vec2(3.9, 7.6), o + vec2(8.6, 2.6)];
    ui.painter().add(Shape::line(points, Stroke::new(1.7, color)));
}

/// The canvas's folder in its 12-point box, centred on `at`: the browse
/// button's icon.
fn folder_icon(ui: &Ui, at: Pos2, color: Color32) {
    let o = at - vec2(6.0, 6.0);
    let points = vec![o + vec2(1.5, 2.5), o + vec2(4.7, 2.5), o + vec2(5.8, 3.7), o + vec2(10.5, 3.7), o + vec2(10.5, 9.5), o + vec2(1.5, 9.5)];
    ui.painter().add(Shape::closed_line(points, Stroke::new(1.3, color)));
}

/// The browse button's size of capitals, and its room either side and
/// beside its icon, as the canvas draws it.
const BROWSE_SIZE: f32 = 11.0;
const BROWSE_PAD: f32 = 12.0;
const BROWSE_ICON_GAP: f32 = 8.0;

/// How wide [`browse_button`] wants to be for `label`.
fn browse_button_width(ui: &Ui, label: &str) -> f32 {
    BROWSE_PAD + 12.0 + BROWSE_ICON_GAP + studio::caps_width(ui, label, BROWSE_SIZE) + BROWSE_PAD
}

/// The destination's browse button at `rect`: the kit's secondary form
/// button (its hairline, its raise and its hand under the pointer) with the
/// canvas's folder before its capitals, both dim and brightened under the
/// pointer as the kit's are.
fn browse_button(ui: &Ui, id: Id, rect: Rect, label: &str, pal: &Pal) -> egui::Response {
    let response = form_button(ui, id, rect, "", false, pal);
    let color = if response.hovered() { pal.text } else { pal.dim };
    let icon = pos2(rect.min.x + BROWSE_PAD + 6.0, rect.center().y);
    folder_icon(ui, icon, color);
    let from = icon.x + 6.0 + BROWSE_ICON_GAP;
    studio::caps(ui, pos2(from, rect.center().y), label, BROWSE_SIZE, color, (rect.max.x - BROWSE_PAD - from).max(1.0), Align2::LEFT_CENTER);
    response
}

/// A small solid triangle pointing right, centred on `at`: a folder's mark.
fn folder_mark(ui: &Ui, at: Pos2, color: Color32) {
    let points = vec![at + vec2(-2.5, -4.0), at + vec2(3.5, 0.0), at + vec2(-2.5, 4.0)];
    ui.painter().add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// The LIBRARY choice as the canvas draws it: the names joined in one
/// hairline group, wrapped to more rows where the column is narrow, in the
/// UI face as they are written (the kit's segmented sets its labels in
/// capitals, which a library's name is not), the chosen one lit amber and
/// the rest raised under the pointer. Returns the group's rect and the
/// name a click landed on.
fn choices(ui: &mut Ui, id: Id, names: &[String], chosen: usize, pal: &Pal) -> (Rect, Option<usize>) {
    const H: f32 = 32.0;
    let w = ui.available_width();
    let font = |i: usize| if i == chosen { strong(13.5) } else { body(13.5) };
    // Where each name goes, wrapped as a line of words wraps.
    let mut spots = Vec::with_capacity(names.len());
    let (mut x, mut row, mut widest) = (0.0_f32, 0usize, 0.0_f32);
    for (i, name) in names.iter().enumerate() {
        let nw = (width_of(ui, name, font(i)) + 36.0).min(w);
        if x > 0.0 && x + nw > w {
            x = 0.0;
            row += 1;
        }
        spots.push((x, row, nw));
        x += nw;
        widest = widest.max(x);
    }
    let (rect, _) = ui.allocate_exact_size(vec2(w, (row + 1) as f32 * H), Sense::hover());
    let group = Rect::from_min_size(rect.min, vec2(widest, rect.height()));
    ui.painter().rect_filled(group, square(), pal.panel);
    let mut clicked = None;
    for (i, (name, (x, row, nw))) in names.iter().zip(spots).enumerate() {
        let seg = Rect::from_min_size(pos2(group.min.x + x, group.min.y + row as f32 * H), vec2(nw, H));
        let response = ui.interact(seg, id.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let ink = if i == chosen {
            ui.painter().rect_filled(seg.shrink(1.0), egui::CornerRadius::same(2), pal.accent);
            pal.on_accent
        } else if response.hovered() {
            ui.painter().rect_filled(seg.shrink(1.0), egui::CornerRadius::same(2), pal.raised);
            pal.text
        } else {
            pal.dim
        };
        if x > 0.0 {
            ui.painter().line_segment([pos2(seg.min.x, seg.min.y + 1.0), pos2(seg.min.x, seg.max.y - 1.0)], Stroke::new(1.0, pal.edge_strong));
        }
        text(ui, seg.center(), name, font(i), ink, nw - 12.0, Align2::CENTER_CENTER);
        if response.clicked() {
            clicked = Some(i);
        }
    }
    ui.painter().rect_stroke(group, square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
    (group, clicked)
}

/// A list of names with no headings, [`DIR_ROW`] points a row and at most
/// [`DIRS_SHOWN`] on screen, scrolled by egui: the row the keys hold raised
/// and ringed as a table's cursor row is, and brought into view on the pass
/// it moved; a row under the pointer raised. Returns the row a click
/// landed on.
fn dirs(ui: &mut Ui, id: Id, names: &[String], sel: usize, pal: &Pal) -> Option<usize> {
    let w = ui.available_width();
    let h = names.len().min(DIRS_SHOWN) as f32 * DIR_ROW;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut child = ui.new_child(UiBuilder::new().id_salt(id).max_rect(rect).layout(Layout::top_down(Align::Min)));
    let memory = id.with("view");
    let (last, offset, height): (Option<usize>, f32, f32) = ui.ctx().data(|d| d.get_temp(memory)).unwrap_or((None, 0.0, 0.0));
    let mut scroll = ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if last != Some(sel) && height > 0.0 {
        let (top, bottom) = (sel as f32 * DIR_ROW, (sel + 1) as f32 * DIR_ROW);
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if bottom > offset + height {
            scroll = scroll.vertical_scroll_offset(bottom - height);
        }
    }
    let mut picked = None;
    let out = scroll.show_rows(&mut child, DIR_ROW, names.len(), |ui, range| {
        for i in range {
            let (row, response) = ui.allocate_exact_size(vec2(ui.available_width(), DIR_ROW), Sense::click());
            let response = response.on_hover_cursor(CursorIcon::PointingHand);
            let on = i == sel;
            if on || response.hovered() {
                ui.painter().rect_filled(row, square(), pal.raised);
            }
            if on {
                ring(ui, row, pal);
            }
            let y = row.center().y;
            folder_mark(ui, pos2(row.min.x + 14.0, y), if on { pal.accent } else { pal.dim });
            let ink = if response.hovered() && !on { Color32::WHITE } else { pal.text };
            text(ui, pos2(row.min.x + 28.0, y), &names[i], body(14.0), ink, row.width() - 38.0, Align2::LEFT_CENTER);
            if response.clicked() {
                picked = Some(i);
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(memory, (Some(sel), out.state.offset.y, out.inner_rect.height())));
    picked
}

// ── The add and edit form ─────────────────────────────────────────────────

/// The add and edit form in the kit's modal over the room: the library (a
/// choice while adding, fixed while editing), the trigger, the destination
/// with its browse button and the server's verdict, the three numbers, the
/// patterns, and the button row with the form's error. Each field is drawn
/// from the form's own value and caret, focused as the form says, and a
/// click on one focuses it, as in the cells. Returns the focused field's
/// caret, None while the keys are on a choice.
#[allow(clippy::too_many_arguments)]
fn form(ui: &Ui, room: &Room, f: &Form, over: Rect, composition: &str, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    let editing = f.editing.is_some();
    let title = if editing {
        let path = f.original.as_ref().map(|d| d.dest_path.clone()).unwrap_or_default();
        t!("bak.edit_title", library = f.library_name, path = path)
    } else {
        t!("bak.add_title")
    };
    let fields = f.fields();
    let focused = f.focused();
    let index_of = |field: Field| fields.iter().position(|x| *x == field).unwrap_or(0);
    let composing = |field: Field| if focused == field { composition } else { "" };
    let id = Id::new("backups-form");
    // The keys moved since the last pass: what they moved to is brought
    // into view, the form being taller than a short room.
    let memory = id.with("focus");
    let moved = ui.ctx().data(|d| d.get_temp::<usize>(memory)) != Some(f.focus);
    ui.ctx().data_mut(|d| d.insert_temp(memory, f.focus));

    let shown = modal(ui, id, over, FORM_W, &title, pal, |ui| {
        let mut caret = None;

        // 1. The library: a choice while adding, fixed while editing.
        label_row(ui, &t!("bak.field_library"), pal);
        ui.add_space(LABEL_GAP);
        let w = ui.available_width();
        if editing {
            let (row, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            let name = text(ui, row.left_center(), &f.library_name, strong(14.0), pal.text, w, Align2::LEFT_CENTER);
            let from = name.max.x;
            if from < row.max.x {
                text(ui, pos2(from, row.center().y), &t!("bak.library_fixed"), body(13.5), pal.dim, row.max.x - from, Align2::LEFT_CENTER);
            }
        } else if room.libraries.is_empty() {
            let (row, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            text(ui, row.left_center(), &t!("bak.no_libraries"), body(13.5), pal.dim, w, Align2::LEFT_CENTER);
        } else {
            let names: Vec<String> = room.libraries.iter().map(|(name, _)| name.clone()).collect();
            let (group, picked) = choices(ui, id.with("library"), &names, f.library, pal);
            if focused == Field::Library {
                ring(ui, group, pal);
                if moved {
                    ui.scroll_to_rect(group, None);
                }
            }
            if let Some(i) = picked {
                acts.push(Act::FormPick(index_of(Field::Library), i));
            }
        }
        ui.add_space(FORM_GAP);

        // 2. The trigger: three radio rows, each its description after it.
        label_row(ui, &t!("bak.field_trigger"), pal);
        ui.add_space(6.0);
        let top = ui.cursor().min.y;
        for (i, trigger) in Trigger::ALL.iter().enumerate() {
            if i > 0 {
                ui.add_space(2.0);
            }
            let detail = format!("— {}", trigger.description());
            if radio(ui, id.with("trigger").with(i), *trigger == f.trigger, &trigger.label(), &detail, pal).clicked() {
                acts.push(Act::FormPick(index_of(Field::Trigger), i));
            }
        }
        if focused == Field::Trigger {
            // The rows' own reach: the kit's radio takes 8 points either side.
            let group = Rect::from_min_max(pos2(ui.max_rect().min.x, top), pos2(ui.max_rect().max.x, ui.cursor().min.y)).expand2(vec2(8.0, 0.0));
            ring(ui, group, pal);
            if moved {
                ui.scroll_to_rect(group, None);
            }
        }
        ui.add_space(FORM_GAP);

        // 3. The destination, its browse button, and the server's verdict.
        label_row(ui, &t!("bak.field_dest"), pal);
        ui.add_space(LABEL_GAP);
        let w = ui.available_width();
        let (line, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
        let browse = t!("bak.browse_button").to_string();
        let bw = browse_button_width(ui, &browse).min(w * 0.5);
        let boxed = Rect::from_min_max(line.min, pos2(line.max.x - bw - 8.0, line.max.y));
        let input = studio::Input::new(f.dest.value(), f.dest.cursor()).mono().focused(focused == Field::Dest, composing(Field::Dest));
        let typed = field_box(ui, id.with("dest"), boxed, &input, pal);
        if typed.response.clicked() {
            acts.push(Act::FormFocus(index_of(Field::Dest)));
        }
        if focused == Field::Dest {
            caret = typed.caret;
            if moved {
                typed.response.scroll_to_me(None);
            }
        }
        let at = Rect::from_min_max(pos2(line.max.x - bw, line.min.y), line.max);
        if browse_button(ui, id.with("browse"), at, &browse, pal).clicked() {
            acts.push(Act::FormBrowse);
        }
        ui.add_space(LABEL_GAP);
        verdict(ui, f, pal);
        ui.add_space(FORM_GAP);

        // 4. The numbers side by side, each its hint beneath; the hour
        //    muted unless the trigger is daily.
        let numbers = [
            (Field::Retention, t!("bak.field_retention"), t!("bak.hint_retention"), &f.retention, false),
            (Field::Throttle, t!("bak.field_throttle"), t!("bak.hint_throttle"), &f.throttle, false),
            (Field::Hour, t!("bak.field_hour"), t!("bak.hint_hour"), &f.hour, f.trigger != Trigger::Daily),
        ];
        let w = ui.available_width();
        let (band, _) = ui.allocate_exact_size(vec2(w, 14.0 + LABEL_GAP + FIELD_H + LABEL_GAP + 16.0), Sense::hover());
        let col_w = (w - 32.0) / 3.0;
        for (i, (field, label, hint, value, muted)) in numbers.iter().enumerate() {
            let x = band.min.x + i as f32 * (col_w + 16.0);
            small_caps(ui, pos2(x, band.min.y + 7.0), label, pal.dim, col_w, Align2::LEFT_CENTER);
            let boxed = Rect::from_min_size(pos2(x, band.min.y + 14.0 + LABEL_GAP), vec2(NUMBER_W.min(col_w), FIELD_H));
            let on = focused == *field && !*muted;
            let input = studio::Input::new(value.value(), value.cursor()).mono().enabled(!*muted).focused(on, composing(*field));
            let typed = field_box(ui, id.with("number").with(i), boxed, &input, pal);
            if !*muted && typed.response.clicked() {
                acts.push(Act::FormFocus(index_of(*field)));
            }
            if on {
                caret = typed.caret;
                if moved {
                    ui.scroll_to_rect(boxed, None);
                }
            }
            text(ui, pos2(x, boxed.max.y + LABEL_GAP + 8.0), hint, body(13.0), pal.dim, col_w, Align2::LEFT_CENTER);
        }
        ui.add_space(FORM_GAP);

        // 5. The exclude patterns.
        label_row(ui, &t!("bak.field_excludes"), pal);
        ui.add_space(LABEL_GAP);
        let (boxed, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_H), Sense::hover());
        let input = studio::Input::new(f.excludes.value(), f.excludes.cursor()).mono().focused(focused == Field::Excludes, composing(Field::Excludes));
        let typed = field_box(ui, id.with("excludes"), boxed, &input, pal);
        if typed.response.clicked() {
            acts.push(Act::FormFocus(index_of(Field::Excludes)));
        }
        if focused == Field::Excludes {
            caret = typed.caret;
            if moved {
                typed.response.scroll_to_me(None);
            }
        }
        ui.add_space(FORM_GAP + 4.0);

        // The button row: the one act at the right, and while editing the
        // patterns' reset at the left, the error then on a line of its own
        // above them (the cells drew the one over the other); while adding
        // the error sits at the row's left, as the canvas has it.
        let w = ui.available_width();
        if editing && let Some(err) = &f.error {
            let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
            text(ui, row.left_center(), err, body(13.0), pal.gold, w, Align2::LEFT_CENTER);
            ui.add_space(LABEL_GAP);
        }
        let (line, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
        let label = if editing { t!("bak.save") } else { t!("bak.add_submit") }.to_string();
        let gw = form_button_width(ui, &label).min(w);
        let go = Rect::from_min_max(pos2(line.max.x - gw, line.min.y), line.max);
        if form_button(ui, id.with("submit"), go, &label, true, pal).clicked() {
            acts.push(Act::FormSubmit);
        }
        if editing {
            let reset = t!("bak.reset_patterns").to_string();
            let rw = form_button_width(ui, &reset).min((go.min.x - 12.0 - line.min.x).max(0.0));
            let at = Rect::from_min_size(line.min, vec2(rw, line.height()));
            if rw > 0.0 && form_button(ui, id.with("reset"), at, &reset, false, pal).clicked() {
                acts.push(Act::FormResetExcludes);
            }
        } else if let Some(err) = &f.error {
            let room_w = go.min.x - 12.0 - line.min.x;
            if room_w > 0.0 {
                text(ui, line.left_center(), err, body(13.0), pal.gold, room_w, Align2::LEFT_CENTER);
            }
        }
        caret
    });
    if shown.closed {
        acts.push(Act::FormCancel);
    }
    shown.inner
}

/// The server's word on the destination under its field: nothing for an
/// empty field; checking, dim; its first error or warning in gold, in at
/// most two lines; or the check mark and its "looks good" in green.
fn verdict(ui: &mut Ui, f: &Form, pal: &Pal) {
    let dest = f.dest.value().trim();
    let w = ui.available_width();
    let said: Option<(String, Color32, bool)> = if dest.is_empty() {
        None
    } else if f.checked_for != dest || f.check.is_none() {
        Some((t!("bak.checking").to_string(), pal.dim, false))
    } else if let Some(check) = &f.check {
        if let Some(err) = check.errors.first() {
            Some((format!("− {}", printable(err, 200)), pal.gold, false))
        } else if let Some(warn) = check.warnings.first() {
            let more = if check.warnings.len() > 1 { format!(" (+{})", check.warnings.len() - 1) } else { String::new() };
            Some((format!("− {}{more}", printable(warn, 200)), pal.gold, false))
        } else {
            Some((t!("bak.check_ok").to_string(), pal.ok, true))
        }
    } else {
        None
    };
    match said {
        None => {
            let _ = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        }
        Some((words, color, true)) => {
            let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
            tick(ui, pos2(row.min.x + 5.0, row.center().y), color);
            text(ui, pos2(row.min.x + 18.0, row.center().y), &words, body(13.0), color, w - 18.0, Align2::LEFT_CENTER);
        }
        Some((words, color, false)) => prose(ui, &words, body(13.0), color, 2),
    }
}

// ── The folder browser, the history, the gate ────────────────────────────

/// The server's folder browser, over the form that waits beneath: the path
/// listed, its folders (a click picks one), and the buttons, up, open, use
/// this folder and close.
fn browser(ui: &Ui, browse: &Browse, over: Rect, pal: &Pal, acts: &mut Vec<Act>) {
    let id = Id::new("backups-browser");
    let shown = modal(ui, id, over, BROWSER_W, &t!("bak.browser_title"), pal, |ui| {
        let w = ui.available_width();
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        let (path, _) = tail_fit(ui, &browse.path, mono(12.5), w);
        text(ui, line.left_center(), &path, mono(12.5), pal.dim, w + 1.0, Align2::LEFT_CENTER);
        ui.add_space(12.0);
        if browse.dirs.is_empty() {
            let (row, _) = ui.allocate_exact_size(vec2(w, DIR_ROW), Sense::hover());
            text(ui, row.left_center() + vec2(10.0, 0.0), &t!("browse.empty"), body(14.0), pal.dim, w - 10.0, Align2::LEFT_CENTER);
        } else if let Some(i) = dirs(ui, id.with("dirs"), &browse.dirs, browse.sel, pal) {
            acts.push(Act::BrowseRow(i));
        }
        ui.add_space(FORM_GAP);
        let (line, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
        let buttons = [
            (t!("browse.up"), false, Act::BrowseUp),
            (t!("browse.open"), false, Act::BrowseEnter),
            (t!("bak.browse_use"), true, Act::BrowseChoose),
            (t!("browse.close"), false, Act::BrowseCancel),
        ];
        let mut x = line.min.x;
        for (i, (label, primary, act)) in buttons.into_iter().enumerate() {
            if x >= line.max.x {
                break;
            }
            let bw = form_button_width(ui, &label).min(line.max.x - x);
            let at = Rect::from_min_size(pos2(x, line.min.y), vec2(bw, line.height()));
            if form_button(ui, id.with("button").with(i), at, &label, primary, pal).clicked() {
                acts.push(act);
            }
            x += bw + 8.0;
        }
    });
    if shown.closed {
        acts.push(Act::BrowseCancel);
    }
}

/// A destination's runs, newest first, in the kit's table (the selected
/// one ringed, a click picks one); under them the count it asked for and
/// the selected run's story: its age and status in its colour, its trigger
/// and how long it took, then its error, or its summary.
#[allow(clippy::too_many_arguments)]
fn history(ui: &Ui, dest: &BackupDestination, runs: &[BackupRun], loaded: bool, sel: usize, over: Rect, pal: &Pal, acts: &mut Vec<Act>) {
    let title = t!("bak.history_title", library = printable(&dest.library_name, 64), path = dest.dest_path);
    let id = Id::new("backups-history");
    let shown = modal(ui, id, over, HISTORY_W, &title, pal, |ui| {
        if !loaded {
            return;
        }
        let w = ui.available_width();
        if runs.is_empty() {
            let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
            text(ui, row.left_center(), &t!("bak.history_empty"), body(14.0), pal.dim, w, Align2::LEFT_CENTER);
            return;
        }
        let cols = [
            Col::left(t!("bak.hcol_started"), 70.0),
            Col::left(t!("bak.hcol_status"), 80.0),
            Col::left(t!("bak.hcol_trigger"), 0.0),
            Col::right(t!("bak.hcol_copied"), 80.0),
            Col::right(t!("bak.hcol_unchanged"), 90.0),
            Col::right(t!("bak.hcol_trashed"), 80.0),
            Col::right(t!("bak.hcol_bytes"), 90.0),
        ];
        let now = unix_now();
        // The table's own room, so its rows scroll inside it and not with
        // the window's body.
        let (rect, _) = ui.allocate_exact_size(vec2(w, 34.0 + runs.len().min(RUNS_SHOWN) as f32 * RUN_ROW), Sense::hover());
        let mut room = ui.new_child(UiBuilder::new().id_salt(id.with("runs")).max_rect(rect).layout(Layout::top_down(Align::Min)));
        let table = Table::new(id.with("runs"), &cols, runs.len()).cursor(Some(sel)).row_height(RUN_ROW).show(&mut room, pal, |ui, i, row| {
            let run = &runs[i];
            let started = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
            cell(ui, row, 0, &started, mono(12.5), ink(row, pal.text, pal), false);
            cell(ui, row, 1, &status_word(&run.status), body(13.5), ink(row, status_tone(&run.status, pal), pal), false);
            let trigger = trigger_words(run.trigger_reason.as_deref().unwrap_or(""));
            cell(ui, row, 2, &trigger, body(13.5), ink(row, pal.soft, pal), false);
            for (at, n) in [(3, run.files_copied), (4, run.files_unchanged), (5, run.files_trashed)] {
                cell(ui, row, at, &fmt_count(n), mono(12.5), ink(row, pal.text, pal), true);
            }
            if run.bytes_copied > 0 {
                cell(ui, row, 6, &fmt_bytes(run.bytes_copied), mono(12.5), ink(row, pal.text, pal), true);
            } else {
                cell(ui, row, 6, "—", mono(12.5), ink(row, pal.dim, pal), true);
            }
        });
        if let Some(i) = table.row {
            acts.push(Act::HistoryPick(i));
        }
        ui.add_space(12.0);
        let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        text(ui, row.left_center(), &t!("bak.history_note", n = HISTORY_LIMIT), body(13.0), pal.dim, w, Align2::LEFT_CENTER);
        let Some(run) = runs.get(sel) else { return };
        ui.add_space(14.0);
        let started = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
        let duration = match (iso_unix(&run.started_at), run.finished_at.as_deref().and_then(iso_unix)) {
            (Some(a), Some(b)) if b >= a => elapsed_text(b - a),
            _ => String::new(),
        };
        let tone = status_tone(&run.status, pal);
        let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
        let head = format!("{started} · {}", status_word(&run.status));
        let said = text(ui, row.left_center(), &head, strong(14.0), tone, w, Align2::LEFT_CENTER);
        let tail = format!(" · {} · {duration}", trigger_words(run.trigger_reason.as_deref().unwrap_or("")));
        if said.max.x < row.max.x {
            text(ui, pos2(said.max.x, row.center().y), &tail, body(14.0), pal.dim, row.max.x - said.max.x, Align2::LEFT_CENTER);
        }
        ui.add_space(6.0);
        let notes = match run.error_message.as_deref().filter(|m| !m.trim().is_empty()) {
            Some(m) => printable(m, 400),
            None => run_summary(run),
        };
        prose(ui, &notes, body(13.5), if run.status == "success" { pal.dim } else { pal.gold }, usize::MAX);
    });
    if shown.closed {
        acts.push(Act::HistoryClose);
    }
}

/// The kit modal's padding and title row (studio.rs), where the gate puts
/// its own gold title.
const MODAL_PAD: f32 = 24.0;
const TITLE_H: f32 = 20.0;

/// The remove gate, gold because it is destructive, as the cells draw it:
/// its edge and its title, then the consequences, then Keep, the safe
/// choice and the primary, and Remove after it. Its close is the Esc that
/// keeps, sent as the key.
fn gate(ui: &Ui, room: &Room, id: i64, over: Rect, pal: &Pal, acts: &mut Vec<Act>, keys: &mut Vec<KeyCode>) {
    let (library, path) = room.dest(id).map(|d| (printable(&d.library_name, 64), d.dest_path.clone())).unwrap_or_default();
    let title = t!("bak.remove_title", library = library, path = path);
    let gate_id = Id::new("backups-remove");
    // The kit's title is in the text colour: the gate's goes on after, in
    // gold, where the kit would have put it.
    let shown = modal(ui, gate_id, over, GATE_W, "", pal, |ui| {
        for key in ["bak.remove_1", "bak.remove_2"] {
            prose(ui, &t!(key), body(14.0), pal.gold, usize::MAX);
            ui.add_space(6.0);
        }
        ui.add_space(14.0);
        let w = ui.available_width();
        let (line, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
        let keep = t!("bak.remove_keep").to_string();
        let go = t!("bak.remove_confirm").to_string();
        let go_w = form_button_width(ui, &go).min(w / 2.0);
        let keep_w = form_button_width(ui, &keep).min(w / 2.0 - 12.0);
        let go_at = Rect::from_min_max(pos2(line.max.x - go_w, line.min.y), line.max);
        let keep_at = Rect::from_min_size(pos2(go_at.min.x - 12.0 - keep_w, line.min.y), vec2(keep_w, line.height()));
        if form_button(ui, gate_id.with("keep"), keep_at, &keep, true, pal).clicked() {
            acts.push(Act::RemoveCancel);
        }
        if form_button(ui, gate_id.with("go"), go_at, &go, false, pal).clicked() {
            acts.push(Act::RemoveConfirm);
        }
    });
    // The warning's gold edge over the kit's hairline, and its gold title,
    // in the window's own layer.
    let layer = ui.ctx().layer_painter(LayerId::new(Order::Foreground, gate_id));
    layer.rect_stroke(shown.window, square(), Stroke::new(1.0, pal.gold), StrokeKind::Inside);
    let inner = shown.window.shrink(MODAL_PAD);
    let row = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_H));
    let format = TextFormat { font_id: strong(16.0), color: pal.gold, ..Default::default() };
    let mut job = LayoutJob::single_section(title.to_string(), format);
    job.wrap = TextWrapping::truncate_at_width((inner.width() - 40.0).max(1.0));
    let galley = layer.layout_job(job);
    let at = Align2::LEFT_CENTER.anchor_size(row.left_center(), galley.size());
    layer.galley(at.min, galley, pal.gold);
    if shown.closed {
        keys.push(KeyCode::Esc);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Vec2};

    use super::super::tests::*;
    use super::*;
    use crate::api::types::{BackupStatus, DirEntry};

    /// The room's region in the default 100×30 window, and a large one
    /// whose modals stand whole.
    const SMALL: Vec2 = vec2(800.0, 420.0);
    const LARGE: Vec2 = vec2(1400.0, 900.0);

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        // Styled before the first pass, as the host's is.
        studio::style_once(&ctx, &Pal::now());
        ctx
    }

    /// One pass of the room over a region of `size`, with `events` and the
    /// input method composing `composition`.
    fn pass_composing(ctx: &egui::Context, room: &mut Room, size: Vec2, events: Vec<egui::Event>, composition: &str) -> (egui::FullOutput, EguiShown) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
        let mut shown = None;
        let mut out = ctx.run_ui(input, |ui| shown = Some(show(ui, room, composition)));
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        (out, shown.expect("the pass ran"))
    }

    fn pass(ctx: &egui::Context, room: &mut Room, size: Vec2) -> (egui::FullOutput, EguiShown) {
        pass_composing(ctx, room, size, Vec::new(), "")
    }

    /// A press and a release at `at`, a pass each after the move, as hands
    /// would make them; what the release's pass told the host.
    fn click(ctx: &egui::Context, room: &mut Room, size: Vec2, at: Pos2) -> EguiShown {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let _ = pass_composing(ctx, room, size, vec![egui::Event::PointerMoved(at)], "");
        let _ = pass_composing(ctx, room, size, vec![button(true)], "");
        pass_composing(ctx, room, size, vec![button(false)], "").1
    }

    /// A click on the control drawn under `id`. One quiet pass first: egui
    /// answers `read_response` from the pass before the last, which, on the
    /// run a modal opened in, is the one it discarded (the window not yet at
    /// its body's height).
    fn click_id(ctx: &egui::Context, room: &mut Room, size: Vec2, id: Id) -> EguiShown {
        let _ = pass(ctx, room, size);
        let at = ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect.center();
        click(ctx, room, size, at)
    }

    /// Every text the pass drew, as laid out.
    fn drawn(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Where the pass drew `words`.
    fn find(out: &egui::FullOutput, words: &str) -> Rect {
        out.shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(text) if text.galley.text() == words => Some(Rect::from_min_size(text.pos, text.galley.size())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{words:?} drawn: {:?}", drawn(out)))
    }

    fn assert_drew(out: &egui::FullOutput, words: &[&str]) {
        let texts = drawn(out);
        for w in words {
            assert!(texts.iter().any(|t| t == w), "{w:?} drawn: {texts:?}");
        }
    }

    fn form(room: &Room) -> &Form {
        match &room.modal {
            Modal::Form(f) => f,
            other => panic!("the form, not {other:?}"),
        }
    }

    fn loaded_room(dests: Vec<BackupDestination>, status: BackupStatus) -> Room {
        let mut room = new_room();
        room.apply(Done::Platform(Ok(defaults())));
        room.queued = None;
        room.busy = None;
        room.apply(loaded(dests, status));
        room
    }

    #[test]
    fn every_state_draws_the_rooms_own_words() {
        let _en = english();
        let ctx = context();

        // Loading: the chip, and the busy line on the foot; nothing else.
        let mut room = new_room();
        room.queue(Op::Platform, t!("bak.busy_loading"));
        let (out, shown) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["BETA", "loading backups…"]);
        let texts = drawn(&out);
        assert!(!texts.iter().any(|t| t == "LIBRARY" || t == "Add a backup destination"), "{texts:?}");
        assert!(shown.caret.is_none() && shown.outcome.is_none());

        // Empty: the idle state, the card, the headings and the empty line.
        let mut room = loaded_room(Vec::new(), BackupStatus::default());
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &[
            "idle",
            "— nothing running · 0 of 0 destinations on · trash swept hourly",
            "polls every 5 s",
            "BETA",
            "Add a backup destination",
            "LIBRARY",
            "(no destinations yet — a adds one)",
            "beta — the configuration may change in a future release; existing destinations keep working",
        ]);

        // Rows, in the cells' words.
        let mut room = idle();
        let (out, shown) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &[
            "— nothing running · 2 of 3 destinations on · trash swept hourly",
            "LIBRARY",
            "DESTINATION",
            "TRIGGER",
            "RETENTION",
            "LAST RUN",
            "ON",
            "music",
            "/Volumes/Backup/music",
            "after each scan",
            "30d",
            "success · 2h",
            "daily at 03:00",
            "failed · 24h",
            "manual only",
            "no trash",
            "never",
        ]);
        assert!(shown.caret.is_none(), "no field has the keyboard at the room's base");

        // With no note, the cursor row's extras ride the foot.
        room.note = None;
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &[
            "/Volumes/Backup/podcasts",
            " · 200 ms/file",
            " · default excludes",
            " · failed: EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'",
        ]);

        // An error note in gold, on the foot.
        room.note = Some((t!("bak.run_disabled").to_string(), true));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["this destination is off — s turns it on"]);

        // A run in flight: its state, its meter's words, its row.
        let active = ActiveBackup {
            destination_id: 2,
            history_id: 9,
            library_name: Some("podcasts".into()),
            dest_path: Some("/Volumes/Backup/podcasts".into()),
            started_at: Some(sqlite_at(unix_now() - 134)),
            trigger_reason: Some("after-scan".into()),
            files_copied: 812,
            files_unchanged: 380,
            files_trashed: 12,
            bytes_copied: 3_650_000_000,
            expected_files: Some(2930),
        };
        room.apply(loaded(dests(), BackupStatus { active: Some(active.clone()), queue_length: 1 }));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &[
            "running",
            "polls every 2 s",
            "41% · 1,204 of 2,930 · 812 copied · 380 unchanged · 12 trashed · 3.4 GB written",
            "running… 41%",
        ]);
        let texts = drawn(&out);
        assert!(texts.iter().any(|t| t.starts_with("— podcasts → /Volumes/Backup/podcasts · after each scan · 2m 1")), "{texts:?}");
        // A first run has no estimate; a narrow room drops count groups.
        let mut first = active;
        first.expected_files = None;
        first.bytes_copied = 0;
        room.apply(loaded(dests(), BackupStatus { active: Some(first), queue_length: 0 }));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["first run — no estimate yet · 812 copied · 380 unchanged · 12 trashed · no bytes written yet", "running…"]);

        // Tasks waiting.
        room.apply(loaded(dests(), BackupStatus { active: None, queue_length: 2 }));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["2 tasks queued", "— waiting for the active scan or backup to finish", "polls every 2 s"]);
    }

    #[test]
    fn a_path_too_long_for_its_column_keeps_its_leaf() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        let (out, _) = pass(&ctx, &mut room, SMALL);
        let texts = drawn(&out);
        assert!(texts.iter().any(|t| t.starts_with('…') && t.ends_with("/podcasts")), "cut from its start: {texts:?}");
        assert!(!texts.iter().any(|t| t == "/Volumes/Backup/podcasts"), "{texts:?}");
    }

    #[test]
    fn a_click_on_a_row_selects_it_and_one_on_its_box_turns_it_on() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        let (out, _) = pass(&ctx, &mut room, LARGE);
        let shown = click(&ctx, &mut room, LARGE, find(&out, "vinyl").center());
        assert_eq!(room.sel, Some(2), "Act::Select");
        assert!(room.queued.is_none() && matches!(room.modal, Modal::None), "a row's click opens nothing");
        assert!(shown.outcome.is_none());
        let shown = click_id(&ctx, &mut room, LARGE, Id::new("backups-on").with(3));
        assert_eq!(
            room.queued,
            Some(Op::Patch { id: 3, patch: BackupPatch { enabled: Some(true), ..Default::default() }, what: Did::Enabled(true) }),
            "Act::ToggleEnabled"
        );
        assert_eq!(room.sel, Some(2), "the box's click is the box's alone");
        assert!(shown.outcome.is_none(), "no click in this room ends it");
    }

    #[test]
    fn the_add_card_opens_the_form_whose_controls_are_its_own_acts() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        let _ = pass(&ctx, &mut room, LARGE);
        let _ = click_id(&ctx, &mut room, LARGE, Id::new("backups-add"));
        assert_eq!(form(&room).focused(), Field::Library, "Act::Add opened the add form");
        let (out, shown) = pass(&ctx, &mut room, LARGE);
        assert!(shown.caret.is_none(), "the keys are on the library choice, which is no field");
        assert_drew(&out, &[
            "Add a backup destination",
            "LIBRARY",
            "music",
            "podcasts",
            "vinyl",
            "after each scan",
            "— when the library finishes scanning",
            "DESTINATION",
            "· another drive, not inside the library",
            "^B BROWSE",
            "RETENTION DAYS",
            "0 = no trash",
            "HOUR (0–23)",
            "EXCLUDE PATTERNS",
            "Thumbs.db, desktop.ini, .DS_Store, ._*",
            "ADD DESTINATION ▸",
        ]);
        let id = Id::new("backups-form");

        // A library, and a trigger: Act::FormPick, the field focused too.
        let _ = click_id(&ctx, &mut room, LARGE, id.with("library").with(1));
        assert_eq!((form(&room).library_name.as_str(), form(&room).library_id), ("podcasts", 20));
        let _ = click_id(&ctx, &mut room, LARGE, id.with("trigger").with(1));
        assert_eq!((form(&room).trigger, form(&room).focused()), (Trigger::Daily, Field::Trigger));

        // A field: Act::FormFocus, and its caret goes to the host.
        let _ = click_id(&ctx, &mut room, LARGE, id.with("dest"));
        assert_eq!(form(&room).focused(), Field::Dest);
        let (_, shown) = pass(&ctx, &mut room, LARGE);
        let field = ctx.read_response(id.with("dest")).expect("the destination's box").rect;
        let caret = shown.caret.expect("the focused field's caret");
        assert!(field.contains(caret - vec2(0.0, 1.0)), "the caret's foot in its box: {caret:?} {field:?}");
        type_text(&mut room, "/Volumes/NAS");
        let (_, plain) = pass(&ctx, &mut room, LARGE);
        let (_, composed) = pass_composing(&ctx, &mut room, LARGE, Vec::new(), "かな");
        assert!(composed.caret.expect("a caret").x > plain.caret.expect("a caret").x + 8.0, "the composition stands before the caret");
        // The hour woke with the daily trigger: a click focuses it.
        let _ = click_id(&ctx, &mut room, LARGE, id.with("number").with(2));
        assert_eq!(form(&room).focused(), Field::Hour);
        room.queued = None;

        // Browse: Act::FormBrowse, the server's browser at the typed path.
        let _ = click_id(&ctx, &mut room, LARGE, id.with("browse"));
        assert_eq!(room.queued, Some(Op::Browse("/Volumes/NAS".into())));
        room.queued = None;
        room.busy = None;

        // Submit: Act::FormSubmit, refused here for its hour.
        let _ = click_id(&ctx, &mut room, LARGE, id.with("number").with(2));
        press(&mut room, KeyCode::Backspace);
        let _ = click_id(&ctx, &mut room, LARGE, id.with("submit"));
        assert_eq!(form(&room).error.as_deref(), Some("the hour is 0 to 23"));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["the hour is 0 to 23"]);

        // The close: Act::FormCancel.
        let shown = click_id(&ctx, &mut room, LARGE, id.with("close"));
        assert!(matches!(room.modal, Modal::None), "the form closed");
        assert!(shown.caret.is_none(), "a closed form leaves no field with the keys");
    }

    #[test]
    fn the_edit_forms_reset_and_save_are_its_own_acts() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('e'));
        room.queued = None;
        let (out, shown) = pass(&ctx, &mut room, LARGE);
        assert!(shown.caret.is_none(), "the keys start on the trigger");
        assert_drew(&out, &[
            "Edit — podcasts → /Volumes/Backup/podcasts",
            "podcasts",
            "  — fixed: remove and re-add to retarget",
            "^P RESET PATTERNS TO THE DEFAULTS",
            "SAVE ▸",
        ]);
        let id = Id::new("backups-form");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("excludes"));
        assert_eq!(form(&room).focused(), Field::Excludes);
        type_text(&mut room, ", *.log");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("reset"));
        let f = form(&room);
        assert!(f.excludes_reset && f.excludes.value() == "Thumbs.db, desktop.ini, .DS_Store, ._*", "Act::FormResetExcludes");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("submit"));
        assert!(
            matches!(room.queued, Some(Op::Patch { id: 2, patch: BackupPatch { exclude_globs: Some(None), .. }, .. })),
            "the save sends the reset as null: {:?}",
            room.queued
        );
    }

    #[test]
    fn the_folder_browser_walks_and_hands_its_folder_back() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        let listing = |path: &str, names: &[&str]| {
            Done::Browsed(Ok(DirListing {
                path: path.into(),
                directories: names.iter().map(|n| DirEntry { name: n.to_string() }).collect(),
                files: Vec::new(),
            }))
        };
        ctrl(&mut room, 'b');
        room.queued = None;
        room.apply(listing("/Volumes", &["Backup", "NAS"]));
        let (out, shown) = pass(&ctx, &mut room, LARGE);
        assert!(shown.caret.is_none(), "the browser has no field");
        assert_drew(&out, &["Choose the destination folder", "/Volumes", "Backup", "NAS", "◂ UP", "OPEN", "USE THIS FOLDER ▸", "CLOSE"]);
        let id = Id::new("backups-browser");

        let _ = click(&ctx, &mut room, LARGE, find(&out, "NAS").center());
        assert!(matches!(&room.modal, Modal::Browser { browse, .. } if browse.sel == 1), "Act::BrowseRow");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("button").with(1));
        assert_eq!(room.queued, Some(Op::Browse("/Volumes/NAS".into())), "Act::BrowseEnter");
        room.queued = None;
        let _ = click_id(&ctx, &mut room, LARGE, id.with("button").with(0));
        assert_eq!(room.queued, Some(Op::Browse("/".into())), "Act::BrowseUp");
        room.queued = None;
        let _ = click_id(&ctx, &mut room, LARGE, id.with("button").with(2));
        assert_eq!(form(&room).dest.value(), "/Volumes", "Act::BrowseChoose");
        assert!(matches!(room.queued, Some(Op::Check { .. })), "the chosen path is checked");

        // Its close button and its window's close: Act::BrowseCancel both.
        for close in [id.with("button").with(3), id.with("close")] {
            room.queued = None;
            room.busy = None;
            ctrl(&mut room, 'b');
            room.queued = None;
            room.apply(listing("/Volumes/Backup", &[]));
            let (out, _) = pass(&ctx, &mut room, LARGE);
            assert_drew(&out, &["(no folders in here)"]);
            let _ = click_id(&ctx, &mut room, LARGE, close);
            assert_eq!(form(&room).dest.value(), "/Volumes", "back in the form, its draft kept");
        }
    }

    #[test]
    fn the_history_lists_the_runs_and_a_click_picks_one() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Enter);
        room.queued = None;
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["History — podcasts → /Volumes/Backup/podcasts", "loading the history…"]);
        assert!(!drawn(&out).iter().any(|t| t == "STARTED"), "nothing under the title until the runs come");
        room.apply(Done::History {
            id: 2,
            result: Ok(vec![
                run("success", 3 * 3600, 12, 11_204, 3, 432_000_000, None),
                run("failed", 86_400, 0, 0, 0, 0, Some("EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'")),
                run("skipped", 3 * 86_400, 0, 0, 0, 0, None),
            ]),
        });
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &[
            "STARTED",
            "STATUS",
            "COPIED",
            "UNCHANGED",
            "TRASHED",
            "BYTES",
            "11,204",
            "412.0 MB",
            "—",
            "50 most recent · newest first · ↑↓ picks a run",
            "3h · success",
            " · after each scan · 2m 14s",
            "12 copied, 11,204 unchanged, 3 trashed · 412.0 MB",
        ]);
        let _ = click(&ctx, &mut room, LARGE, find(&out, "failed").center());
        assert!(matches!(&room.modal, Modal::History { sel: 1, .. }), "Act::HistoryPick");
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["24h · failed", "EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'"]);
        let _ = click_id(&ctx, &mut room, LARGE, Id::new("backups-history").with("close"));
        assert!(matches!(room.modal, Modal::None), "Act::HistoryClose");

        // A history with no runs says so.
        press(&mut room, KeyCode::Enter);
        room.apply(Done::History { id: 2, result: Ok(Vec::new()) });
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["(no runs recorded yet)"]);
    }

    #[test]
    fn the_remove_gate_keeps_on_its_primary_and_its_close_and_removes_on_its_word() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('r'));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &[
            "Remove music → /Volumes/Backup/music?",
            "The files on disk are NOT deleted; only the schedule and its history go.",
            "You can add the same path again later.",
            "◂ KEEP",
            "REMOVE",
        ]);
        let title = "Remove music → /Volumes/Backup/music?";
        let inks: Vec<Color32> = out
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) if text.galley.text() == title => Some(text.galley.job.sections[0].format.color),
                _ => None,
            })
            .collect();
        assert_eq!(inks, vec![Pal::now().gold], "the title once, in gold, as the cells' gate");
        let id = Id::new("backups-remove");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("keep"));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Act::RemoveCancel");

        // Its close is the Esc that keeps.
        press(&mut room, KeyCode::Char('r'));
        let _ = pass(&ctx, &mut room, LARGE);
        let _ = click_id(&ctx, &mut room, LARGE, id.with("close"));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Esc kept it");
        assert_eq!(room.sel, Some(0), "the cursor stays: Esc reached the gate, not the room's base");

        press(&mut room, KeyCode::Char('r'));
        let _ = pass(&ctx, &mut room, LARGE);
        let _ = click_id(&ctx, &mut room, LARGE, id.with("go"));
        assert_eq!(room.queued, Some(Op::Remove(1)), "Act::RemoveConfirm");
    }

    #[test]
    fn a_click_beside_a_modal_reaches_nothing_beneath() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        let (base, _) = pass(&ctx, &mut room, LARGE);
        let vinyl = find(&base, "vinyl").center();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('r'));
        let (_, _) = pass(&ctx, &mut room, LARGE);
        let _ = click(&ctx, &mut room, LARGE, vinyl);
        assert!(matches!(room.modal, Modal::Remove(1)), "the gate stays up");
        assert_eq!(room.sel, Some(0), "the row under the scrim took no click");
    }
}
