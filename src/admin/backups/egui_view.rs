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
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

use super::super::{EguiShown, age_text, iso_unix, unix_now};
use super::*;
use crate::api::types::ActiveBackup;
use crate::gui::window::studio::{
    self, BROWSER_W, Col, Dialog, FIELD_H, Glyph, Pal, Para, Run, Table, add_card, bare, body, cadence, cell, checkbox,
    chip, field_box, fitted, folder_browser, foot, foot_line, form_button, gate, hand, ink, label, led_cell, line, meter,
    meter_lit, mono, name_cell, path_cell, radio, reveal, ring, ring_since, room_rects, small_caps, square, state_line,
    strong, text, unkeyed, verdict, width_of, with_figures,
};

/// The canvas's room between the room's blocks, the note line's among them.
const GAP: f32 = studio::ROOM_GAP;
/// The form's room between its blocks, and between a label and its field.
const FORM_GAP: f32 = 20.0;
const LABEL_GAP: f32 = 8.0;
/// The modals' widths: the canvas's form, and the history's wider one.
const FORM_W: f32 = 640.0;
const HISTORY_W: f32 = 760.0;
/// The history's rows, and how many show at once (the cells' eight).
const RUN_ROW: f32 = 32.0;
const RUNS_SHOWN: usize = 8;
/// A number field's box, as the canvas draws it.
const NUMBER_W: f32 = 120.0;

/// One pass of the room over the whole of `ui`, the region its host hands
/// it, `composition` the input method's uncommitted text for the field that
/// has the keyboard. The pass's clicks are applied after it through
/// [`Room::act`], and what the host must know comes back: the focused
/// field's caret, and an outcome a click brought, which none in this room
/// does.
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    let pal = Pal::now();
    let full = ui.max_rect();
    let (body_rect, note_rect) = room_rects(full);

    let mut acts: Vec<Act> = Vec::new();
    let mut base = ui.new_child(
        egui::UiBuilder::new().id_salt("backups-body").max_rect(body_rect).layout(egui::Layout::top_down(egui::Align::Min)),
    );
    base.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
    state(&mut base, room, &pal);
    if room.loaded {
        base.add_space(GAP);
        if add_card(&mut base, Id::new("backups-add"), &t!("bak.add_card"), Glyph::Tray, &pal).clicked() {
            acts.push(Act::Add);
        }
        base.add_space(GAP);
        table(&mut base, room, &pal, &mut acts);
    }
    note(ui, note_rect, room, &pal);

    // A modal makes the room beneath inert: what was clicked under it this
    // pass is dropped, as the cells view drops its registrations.
    let mut caret = None;
    if !matches!(room.modal, Modal::None) {
        acts.clear();
    }
    match &room.modal {
        Modal::None => {}
        Modal::Form(f) => caret = form(ui, room, f, full, composition, &pal, &mut acts),
        Modal::Browser { browse, .. } => browser(ui, browse, full, &pal, &mut acts),
        Modal::History { dest, runs, loaded, sel } => history(ui, dest, runs, *loaded, *sel, full, &pal, &mut acts),
        &Modal::Remove(id) => remove_gate(ui, room, id, full, &pal, &mut acts),
    }

    let acted = !acts.is_empty();
    let mut outcome = None;
    for act in acts {
        if let Some(out) = room.act(act) {
            outcome.get_or_insert(out);
        }
    }
    if acted {
        // The room changed after this pass drew it: the next pass shows it.
        ui.ctx().request_repaint();
        // The caret is the one this pass drew, the field's that had the keys
        // then. A click that closed the form, or gave its keys to a choice
        // (the library, the trigger), leaves no field with the keyboard now;
        // one that gave them to another field keeps a caret until the next
        // pass draws that field's, so the input method stays on between the
        // two rather than dropping what it was composing.
        let typing = matches!(&room.modal, Modal::Form(f) if !matches!(f.focused(), Field::Library | Field::Trigger));
        if !typing {
            caret = None;
        }
    }
    EguiShown { caret, outcome }
}

// ── The state line ────────────────────────────────────────────────────────

/// The state line: what the room is doing behind its LED (a run in flight,
/// tasks queued, or idle), the cadence where it fits, and the gold BETA
/// chip at the right, which stands there while the room loads too; under a
/// run in flight, its progress.
fn state(ui: &mut Ui, room: &Room, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    let beta = chip(ui, row.right_center(), Align2::RIGHT_CENTER, &t!("bak.beta"), pal.gold);
    if !room.loaded {
        return;
    }
    let now = unix_now();
    // The word in the semibold face and its colour while something runs or
    // waits, as the cells set it bold; idle, plain and dim.
    // The detail's figures (the path, the time it has run) in the grid's
    // monospace, and the cadence's, as the canvas sets them.
    let (word, detail, tone, lamp, figures, polls) = match &room.status.active {
        Some(a) => {
            let elapsed = a.started_at.as_deref().and_then(iso_unix).map(|t| elapsed_text(now - t)).unwrap_or_default();
            let trigger = a.trigger_reason.as_deref().map(trigger_words).unwrap_or_default();
            let path = a.dest_path.clone().unwrap_or_default();
            let detail = t!(
                "bak.state_running_detail",
                library = printable(a.library_name.as_deref().unwrap_or(""), 64),
                path = path.clone(),
                trigger = trigger,
                elapsed = elapsed.clone()
            );
            (t!("bak.state_running"), detail, pal.ok, pal.ok, vec![path, elapsed], t!("bak.polls_2"))
        }
        None if room.status.queue_length > 0 => {
            let n = room.status.queue_length;
            let word = if n == 1 { t!("bak.state_queued_one") } else { t!("bak.state_queued", n = n) };
            (word, t!("bak.state_queued_detail"), pal.gold, pal.gold, Vec::new(), t!("bak.polls_2"))
        }
        None => {
            let on = room.dests.iter().filter(|d| d.enabled).count();
            let detail = t!("bak.state_idle_detail", on = on, total = room.dests.len());
            (t!("bak.state_idle"), detail, pal.dim, pal.unlit, Vec::new(), t!("bak.polls_5"))
        }
    };
    let font = if room.status.active.is_some() || room.status.queue_length > 0 { strong(14.0) } else { body(14.0) };
    let figures: Vec<&str> = figures.iter().map(String::as_str).collect();
    let mut parts = vec![(bare(&word).to_string(), font, tone)];
    parts.extend(with_figures(&detail, &figures, body(14.0), mono(12.5), pal.text));
    // The kit's line, short of the chip, the cadence only where the whole
    // state fits beside it, as the cells show it.
    let line = Rect::from_min_max(row.min, pos2((beta.min.x - 16.0).max(row.min.x), row.max.y));
    state_line(ui, line, lamp, &parts, &cadence(&polls, pal));
    if let Some(a) = &room.status.active {
        progress(ui, a, pal);
    }
}

/// The figures of a line of counts, each run of digits with the marks that
/// join them (1,341 · 45% · 3.9) in the monospace, the words between them
/// in `font`: the canvas's progress line.
fn counts(s: &str, font: FontId, color: Color32) -> Vec<Run> {
    let mut parts: Vec<Run> = Vec::new();
    let (mut word, mut digits) = (String::new(), String::new());
    for c in s.chars() {
        let figure = c.is_ascii_digit() || (!digits.is_empty() && matches!(c, ',' | '.' | '%'));
        if figure {
            if !word.is_empty() {
                parts.push((std::mem::take(&mut word), font.clone(), color));
            }
            digits.push(c);
        } else {
            if !digits.is_empty() {
                parts.push((std::mem::take(&mut digits), mono(12.0), color));
            }
            word.push(c);
        }
    }
    if !digits.is_empty() {
        parts.push((digits, mono(12.0), color));
    }
    if !word.is_empty() {
        parts.push((word, font, color));
    }
    parts
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
    let said = |groups: &[String]| counts(&line(groups), body(13.0), pal.dim);
    while !groups.is_empty() && studio::runs_width(ui, &said(&groups)) > room_w {
        groups.pop();
    }
    studio::runs(ui, pos2(from, row.center().y), &said(&groups), room_w.max(1.0), Align2::LEFT_CENTER);
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

/// The destinations under their headings, in server order: the library,
/// the path cut from its start (the whole of it in a tooltip where it was
/// cut), the trigger, the retention, the last run behind an LED in its
/// status colour, and the ON box. A click on a row selects it; a click on
/// its box turns the destination on or off.
fn table(ui: &mut Ui, room: &Room, pal: &Pal, acts: &mut Vec<Act>) {
    // The canvas's widths, narrowed together where the column is narrow so
    // the destination keeps some room.
    let cols = fitted(
        ui.available_width(),
        vec![
            Col::left(t!("bak.col_library"), 150.0),
            Col::left(t!("bak.col_destination"), 0.0),
            Col::left(t!("bak.col_trigger"), 150.0),
            Col::left(t!("bak.col_retention"), 110.0),
            Col::left(t!("bak.col_last_run"), 170.0),
            Col::center(t!("bak.col_on"), 48.0),
        ],
        160.0,
        0.6,
    );
    let now = unix_now();
    let running = room.running_id();
    let pct = room.status.active.as_ref().and_then(|a| {
        a.expected_files.filter(|e| *e > 0).map(|e| ((a.files_copied + a.files_unchanged + a.files_trashed) * 100 / e).min(100))
    });
    let mut toggled = None;
    let empty = t!("bak.empty");
    let table = Table::new(Id::new("backups-table"), &cols, room.dests.len()).cursor(room.sel).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| {
        let d = &room.dests[i];
        name_cell(ui, row, 0, &printable(&d.library_name, 64), pal);
        path_cell(ui, row, 1, &d.dest_path, mono(12.5), ink(row, pal.soft, pal), Id::new("backups-path").with(d.id));
        // A daily run's hour is a figure, in the monospace.
        let trigger = trigger_cell(d);
        let trigger_font = if trigger.starts_with(|c: char| c.is_ascii_digit()) { mono(12.5) } else { body(13.5) };
        cell(ui, row, 2, &trigger, trigger_font, ink(row, pal.soft, pal), false);
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
        led_cell(ui, row, 4, Some(lamp), &words, body(13.5), ink(row, tone, pal));
        if checkbox(ui, Id::new("backups-on").with(d.id), row.cell(5).center(), d.enabled, pal).clicked() {
            toggled = Some(d.id);
        }
    });
    if let Some(i) = shown.row {
        acts.push(Act::Select(i));
    }
    if let Some(id) = toggled {
        acts.push(Act::ToggleEnabled(id));
    }
}

/// The room's foot: what it is busy with in the accent, else its note,
/// else the cursor row's extras (its path, its throttle, its patterns and
/// its last run's word), as the cells ride them on the note line, in one
/// run.
fn note(ui: &Ui, rect: Rect, room: &Room, pal: &Pal) {
    let story: Vec<Run> = match room.selected().filter(|_| room.loaded) {
        Some(d) => story(room, d, pal).into_iter().map(|(s, color)| (s, body(13.0), color)).collect(),
        None => Vec::new(),
    };
    foot_line(ui, rect, room.busy.as_deref(), room.note.as_ref(), &story, pal);
}

/// The cursor row's extras for the foot, in their colours.
fn story(room: &Room, d: &BackupDestination, pal: &Pal) -> Vec<(String, Color32)> {
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
    spans
}

// ── Pieces the kit has not got ────────────────────────────────────────────

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
        let response = hand(ui.interact(seg, id.with(i), Sense::click()));
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
    let id = Id::new("backups-form");
    // What the keys move to is brought into view on the pass they moved,
    // the form being taller than a short room.
    let shown = Dialog::new(id, over, FORM_W, &title).show(ui, pal, |ui| {
        let mut caret = None;

        // 1. The library: a choice while adding, fixed while editing.
        label(ui, &t!("bak.field_library"), pal);
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
                reveal(ui, id, f.focus, group);
            }
            if let Some(i) = picked {
                acts.push(Act::FormPick(index_of(Field::Library), i));
            }
        }
        ui.add_space(FORM_GAP);

        // 2. The trigger: three radio rows, each its description after it.
        label(ui, &t!("bak.field_trigger"), pal);
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
            let group = ring_since(ui, top + 2.0, pal);
            reveal(ui, id, f.focus, group);
        }
        ui.add_space(FORM_GAP);

        // 3. The destination, its browse button, and the server's verdict.
        label(ui, &t!("bak.field_dest"), pal);
        ui.add_space(LABEL_GAP);
        let w = ui.available_width();
        let (line, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
        // Bare of the cells' key, as the canvas draws it.
        let browse = unkeyed(&t!("bak.browse_button")).0.to_string();
        let bw = browse_button_width(ui, &browse).min(w * 0.5);
        let boxed = Rect::from_min_max(line.min, pos2(line.max.x - bw - 8.0, line.max.y));
        let input = studio::Input::of(&f.dest).mono().focused(focused == Field::Dest, composition);
        let typed = field_box(ui, id.with("dest"), boxed, &input, pal);
        if typed.response.clicked() {
            acts.push(Act::FormFocus(index_of(Field::Dest)));
        }
        if focused == Field::Dest {
            caret = typed.caret;
            reveal(ui, id, f.focus, typed.response.rect);
        }
        let at = Rect::from_min_max(pos2(line.max.x - bw, line.min.y), line.max);
        if browse_button(ui, id.with("browse"), at, &browse, pal).clicked() {
            acts.push(Act::FormBrowse);
        }
        ui.add_space(LABEL_GAP);
        dest_verdict(ui, f, pal);
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
            let input = studio::Input::of(value).mono().enabled(!*muted).focused(on, composition);
            let typed = field_box(ui, id.with("number").with(i), boxed, &input, pal);
            if !*muted && typed.response.clicked() {
                acts.push(Act::FormFocus(index_of(*field)));
            }
            if on {
                caret = typed.caret;
                reveal(ui, id, f.focus, boxed);
            }
            text(ui, pos2(x, boxed.max.y + LABEL_GAP + 8.0), hint, body(13.0), pal.dim, col_w, Align2::LEFT_CENTER);
        }
        ui.add_space(FORM_GAP);

        // 5. The exclude patterns.
        label(ui, &t!("bak.field_excludes"), pal);
        ui.add_space(LABEL_GAP);
        let (boxed, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_H), Sense::hover());
        let input = studio::Input::of(&f.excludes).mono().focused(focused == Field::Excludes, composition);
        let typed = field_box(ui, id.with("excludes"), boxed, &input, pal);
        if typed.response.clicked() {
            acts.push(Act::FormFocus(index_of(Field::Excludes)));
        }
        if focused == Field::Excludes {
            caret = typed.caret;
            reveal(ui, id, f.focus, typed.response.rect);
        }
        ui.add_space(FORM_GAP + 4.0);

        // The kit's foot: the one act at the right, and while editing the
        // patterns' reset at the left, the error then on a line of its own
        // above them (the cells drew the one over the other); while adding
        // the error sits at the row's left, as the canvas has it.
        if editing && let Some(err) = &f.error {
            studio::line(ui, 18.0, err, body(13.0), pal.gold);
            ui.add_space(LABEL_GAP);
        }
        let submit = if editing { t!("bak.save") } else { t!("bak.add_submit") }.to_string();
        let reset = unkeyed(&t!("bak.reset_patterns")).0.to_string();
        let start: &[(&str, bool)] = if editing { &[(reset.as_str(), false)] } else { &[] };
        let refusal = if editing { None } else { f.error.as_deref() };
        match foot(ui, id.with("foot"), start, &[(submit.as_str(), true)], refusal, pal) {
            Some(k) if k == start.len() => acts.push(Act::FormSubmit),
            Some(_) => acts.push(Act::FormResetExcludes),
            None => {}
        }
        caret
    });
    if shown.closed {
        acts.push(Act::FormCancel);
    }
    shown.inner
}

/// The server's word on the destination under its field, the kit's
/// verdict: nothing for an empty field; checking, dim; its first error or
/// warning in gold behind the cells' dash, in at most two lines; or the
/// check mark and its "looks good" in green.
fn dest_verdict(ui: &mut Ui, f: &Form, pal: &Pal) {
    let dest = f.dest.value().trim();
    let w = ui.available_width();
    let said: Option<(String, Color32, Option<bool>)> = if dest.is_empty() {
        None
    } else if f.checked_for != dest || f.check.is_none() {
        Some((t!("bak.checking").to_string(), pal.dim, None))
    } else if let Some(check) = &f.check {
        if let Some(err) = check.errors.first() {
            Some((printable(err, 200), pal.gold, Some(false)))
        } else if let Some(warn) = check.warnings.first() {
            let more = if check.warnings.len() > 1 { format!(" (+{})", check.warnings.len() - 1) } else { String::new() };
            Some((format!("{}{more}", printable(warn, 200)), pal.gold, Some(false)))
        } else {
            Some((t!("bak.check_ok").to_string(), pal.ok, Some(true)))
        }
    } else {
        None
    };
    let at = ui.cursor().min;
    let h = said.map_or(0.0, |(words, tone, mark)| verdict(ui, at, w, &words, &[], tone, mark).height());
    let _ = ui.allocate_exact_size(vec2(w, h.max(18.0)), Sense::hover());
}

// ── The folder browser, the history, the gate ────────────────────────────

/// The server's folder browser, the kit's (Libraries' too), over the form
/// that waits beneath: the path listed, its folders (a click picks one),
/// and the buttons in the cells' order from the left, up, open, use this
/// folder (lit) and close.
fn browser(ui: &Ui, browse: &Browse, over: Rect, pal: &Pal, acts: &mut Vec<Act>) {
    let id = Id::new("backups-browser");
    let shown = Dialog::new(id, over, BROWSER_W, &t!("bak.browser_title")).show(ui, pal, |ui| {
        if let Some(i) = folder_browser(ui, id.with("list"), &browse.path, &browse.dirs, browse.sel, &t!("browse.empty"), pal) {
            acts.push(Act::BrowseRow(i));
        }
        let (up, open, choose, close) = (t!("browse.up"), t!("browse.open"), t!("bak.browse_use"), t!("browse.close"));
        let start = [(up.as_ref(), false), (open.as_ref(), false), (choose.as_ref(), true), (close.as_ref(), false)];
        let meant = [Act::BrowseUp, Act::BrowseEnter, Act::BrowseChoose, Act::BrowseCancel];
        if let Some(k) = foot(ui, id.with("foot"), &start, &[], None, pal) {
            acts.push(meant[k].clone());
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
    let shown = Dialog::new(id, over, HISTORY_W, &title).show(ui, pal, |ui| {
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
        // the window's body; each destination's runs a listing of their
        // own, so one opened after another's starts on its cursor, not
        // where the wheel left the other's runs.
        let table = Table::new(id.with("runs"), &cols, runs.len())
            .cursor(Some(sel))
            .row_height(RUN_ROW)
            .height(runs.len().min(RUNS_SHOWN) as f32 * RUN_ROW)
            .content(Id::new(dest.id));
        let table = table.show(ui, pal, |ui, i, row| {
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
        line(ui, 18.0, &t!("bak.history_note", n = HISTORY_LIMIT), body(13.0), pal.dim);
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
        // A run still going has no duration yet: no separator left hanging.
        let trigger = trigger_words(run.trigger_reason.as_deref().unwrap_or(""));
        let tail = if duration.is_empty() { format!(" · {trigger}") } else { format!(" · {trigger} · {duration}") };
        if said.max.x < row.max.x {
            text(ui, pos2(said.max.x, row.center().y), &tail, body(14.0), pal.dim, row.max.x - said.max.x, Align2::LEFT_CENTER);
        }
        ui.add_space(6.0);
        let notes = match run.error_message.as_deref().filter(|m| !m.trim().is_empty()) {
            Some(m) => printable(m, 400),
            None => run_summary(run),
        };
        Para::new(&notes, body(13.5), if run.status == "success" { pal.dim } else { pal.gold }).show(ui);
    });
    if shown.closed {
        acts.push(Act::HistoryClose);
    }
}

/// The remove gate, the kit's: the consequences, then Keep, the safe
/// choice and the primary, and Remove after it.
fn remove_gate(ui: &Ui, room: &Room, id: i64, over: Rect, pal: &Pal, acts: &mut Vec<Act>) {
    let (library, path) = room.dest(id).map(|d| (printable(&d.library_name, 64), d.dest_path.clone())).unwrap_or_default();
    let title = t!("bak.remove_title", library = library, path = path);
    let sentences = ["bak.remove_1", "bak.remove_2"].map(|key| Para::new(&t!(key), body(14.0), pal.gold)).into();
    let (keep, go) = (t!("bak.remove_keep"), t!("bak.remove_confirm"));
    match gate(ui, Id::new("backups-remove"), over, &title, sentences, [&keep, &go], pal) {
        Some(true) => acts.push(Act::RemoveConfirm),
        Some(false) => acts.push(Act::RemoveCancel),
        None => {}
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

        // Empty: the idle state (the kit's state line, in one run), the
        // card, the headings and the empty line.
        let mut room = loaded_room(Vec::new(), BackupStatus::default());
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &[
            "idle — nothing running · 0 of 0 destinations on · trash swept hourly",
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
            "idle — nothing running · 2 of 3 destinations on · trash swept hourly",
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
        // In one run, as the room's other stories.
        assert_drew(&out, &[
            "/Volumes/Backup/podcasts · 200 ms/file · default excludes · failed: EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'",
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
            "polls every 2 s",
            "41% · 1,204 of 2,930 · 812 copied · 380 unchanged · 12 trashed · 3.4 GB written",
            "running… 41%",
        ]);
        let texts = drawn(&out);
        assert!(texts.iter().any(|t| t.starts_with("running — podcasts → /Volumes/Backup/podcasts · after each scan · 2m 1")), "{texts:?}");
        // A narrow room drops whole count groups off the progress line's end.
        let whole = "41% · 1,204 of 2,930 · 812 copied · 380 unchanged · 12 trashed · 3.4 GB written";
        let (out, _) = pass(&ctx, &mut room, vec2(560.0, 420.0));
        let texts = drawn(&out);
        let line = texts.iter().find(|t| t.starts_with("41% · 1,204 of 2,930")).unwrap_or_else(|| panic!("the progress line: {texts:?}"));
        assert!(line.len() < whole.len() && whole.starts_with(line.as_str()) && whole[line.len()..].starts_with(" · "), "whole groups dropped: {line:?}");
        // A first run has no estimate.
        let mut first = active;
        first.expected_files = None;
        first.bytes_copied = 0;
        room.apply(loaded(dests(), BackupStatus { active: Some(first), queue_length: 0 }));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["first run — no estimate yet · 812 copied · 380 unchanged · 12 trashed · no bytes written yet", "running…"]);

        // Tasks waiting.
        room.apply(loaded(dests(), BackupStatus { active: None, queue_length: 2 }));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["2 tasks queued — waiting for the active scan or backup to finish", "polls every 2 s"]);
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
            "BROWSE",
            "RETENTION DAYS",
            "0 = no trash",
            "HOUR (0–23)",
            "EXCLUDE PATTERNS",
            "Thumbs.db, desktop.ini, .DS_Store, ._*",
            "ADD DESTINATION",
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
        // A click that hands the keys to another field still has a caret to
        // return, so the input method stays on; one that hands them to a
        // choice has none, at once.
        let shown = click_id(&ctx, &mut room, LARGE, id.with("excludes"));
        assert_eq!(form(&room).focused(), Field::Excludes);
        assert!(shown.caret.is_some(), "the keys went from one field to another");
        let shown = click_id(&ctx, &mut room, LARGE, id.with("trigger").with(1));
        assert_eq!(form(&room).focused(), Field::Trigger);
        assert!(shown.caret.is_none(), "the keys went to the trigger, which is no field");
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
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(0));
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
            "RESET PATTERNS TO THE DEFAULTS",
            "SAVE",
        ]);
        let id = Id::new("backups-form");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("excludes"));
        assert_eq!(form(&room).focused(), Field::Excludes);
        type_text(&mut room, ", *.log");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(0));
        let f = form(&room);
        assert!(f.excludes_reset && f.excludes.value() == "Thumbs.db, desktop.ini, .DS_Store, ._*", "Act::FormResetExcludes");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(1));
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
        assert_drew(&out, &["Choose the destination folder", "/Volumes", "Backup", "NAS", "UP", "OPEN", "USE THIS FOLDER", "CLOSE"]);
        let id = Id::new("backups-browser");

        let _ = click(&ctx, &mut room, LARGE, find(&out, "NAS").center());
        assert!(matches!(&room.modal, Modal::Browser { browse, .. } if browse.sel == 1), "Act::BrowseRow");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(1));
        assert_eq!(room.queued, Some(Op::Browse("/Volumes/NAS".into())), "Act::BrowseEnter");
        room.queued = None;
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(0));
        assert_eq!(room.queued, Some(Op::Browse("/".into())), "Act::BrowseUp");
        room.queued = None;
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(2));
        assert_eq!(form(&room).dest.value(), "/Volumes", "Act::BrowseChoose");
        assert!(matches!(room.queued, Some(Op::Check { .. })), "the chosen path is checked");

        // Its close button and its window's close: Act::BrowseCancel both.
        for close in [id.with("foot").with(3), id.with("close")] {
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

    /// The wheel takes the folder list well down, its cursor left on the
    /// first row, as a person looking for a folder leaves it.
    fn wheel_down(ctx: &egui::Context, room: &mut Room, out: &egui::FullOutput, over: &str) -> egui::FullOutput {
        let at = find(out, over).center();
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -2000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::default(),
        };
        let _ = pass_composing(ctx, room, LARGE, vec![egui::Event::PointerMoved(at), wheel], "");
        // egui spreads a wheel's travel over the passes after it.
        let mut last = pass(ctx, room, LARGE).0;
        for _ in 0..90 {
            last = pass(ctx, room, LARGE).0;
        }
        last
    }

    #[test]
    fn a_new_listing_opens_on_its_cursor_and_not_where_the_wheel_left_the_last() {
        let _en = english();
        let ctx = context();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        let listing = |path: &str, stem: &str| {
            Done::Browsed(Ok(DirListing {
                path: path.into(),
                directories: (0..30).map(|i| DirEntry { name: format!("{stem}{i:02}") }).collect(),
                files: Vec::new(),
            }))
        };
        ctrl(&mut room, 'b');
        room.queued = None;
        room.apply(listing("/Volumes", "disk"));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        let out = wheel_down(&ctx, &mut room, &out, "disk05");
        let texts = drawn(&out);
        assert!(!texts.iter().any(|t| t == "disk00") && texts.iter().any(|t| t == "disk29"), "the wheel took the list down: {texts:?}");

        // Up to the parent, its cursor still on the first row: the parent's
        // listing opens there.
        let _ = click_id(&ctx, &mut room, LARGE, Id::new("backups-browser").with("foot").with(0));
        assert_eq!(room.queued, Some(Op::Browse("/".into())), "Act::BrowseUp");
        room.queued = None;
        room.apply(listing("/", "root"));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["root00"]);

        // The same listing again, the browser closed and opened between: it
        // opens on its cursor too.
        let out = wheel_down(&ctx, &mut room, &out, "root05");
        assert!(!drawn(&out).iter().any(|t| t == "root00"), "the wheel took the list down");
        let _ = click_id(&ctx, &mut room, LARGE, Id::new("backups-browser").with("foot").with(3));
        assert!(matches!(room.modal, Modal::Form(_)), "Act::BrowseCancel");
        // The frame the close brings, the form back under no browser.
        let _ = pass(&ctx, &mut room, LARGE);
        ctrl(&mut room, 'b');
        room.queued = None;
        room.apply(listing("/", "root"));
        let (out, _) = pass(&ctx, &mut room, LARGE);
        assert_drew(&out, &["root00"]);
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
    fn the_remove_gate_is_answered_keep_on_its_primary_or_esc_and_removes_on_its_word() {
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
            "KEEP",
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
        assert!(ctx.read_response(id.with("close")).is_none(), "a gate has no close, as the cells draw it");
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(0usize));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Act::RemoveCancel");

        // Esc keeps.
        press(&mut room, KeyCode::Char('r'));
        let _ = pass(&ctx, &mut room, LARGE);
        press(&mut room, KeyCode::Esc);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Esc kept it");
        assert_eq!(room.sel, Some(0), "the cursor stays: Esc reached the gate, not the room's base");

        press(&mut room, KeyCode::Char('r'));
        let _ = pass(&ctx, &mut room, LARGE);
        let _ = click_id(&ctx, &mut room, LARGE, id.with("foot").with(1usize));
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
