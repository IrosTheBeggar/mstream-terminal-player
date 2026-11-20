//! The Libraries room drawn by egui, in the desktop window's panel: the
//! same [`Room`] the cells view draws (the parent module: its state, its
//! worker, its keys and its words), only the drawing is new, in pixels, on
//! the Studio kit (src/gui/window/studio.rs), after the Claude Design
//! canvas of the Admin screen (its Main and LibrariesName boards).
//!
//! Every click is one of the room's own [`Act`]s, the one the cells view
//! registers for the same control, applied through [`Room::act`] in the
//! pass. Keys never reach egui: the room's keymap still moves the cursor,
//! edits the two fields (the typed path and the name) and answers the
//! modals, and egui draws whatever the room holds. What the view keeps
//! between passes (where a list stands, the window over a field's line)
//! it keeps in egui's memory under its own ids; the room holds nothing for
//! it.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::epaint::{Shadow, StrokeKind};
use egui::style::ScrollAnimation;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Align2, Color32, CursorIcon, FontId, Id, Layout, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use unicode_segmentation::UnicodeSegmentation;

use super::*;
use crate::admin::EguiShown;
use crate::gui::window::studio::{
    self, Col, FIELD_H, FORM_BUTTON_H, Icon, Pal, SCRIM, Table, add_card, body, checkbox, field_box, form_button,
    form_button_width, icon_button, ink, led, modal, mono, note_line, ring, small_caps, square, strong, style_once, text,
    width_of,
};

/// The room's margins inside its area, as the canvas pads the room.
const PAD_X: f32 = 24.0;
const PAD_TOP: f32 = 20.0;
const PAD_BOTTOM: f32 = 14.0;
/// The room between the add card, the table and the note line.
const GAP: f32 = 18.0;
/// The note line along the room's foot.
const NOTE_H: f32 = 18.0;
/// The table's NAME and SYMLINKS columns (FOLDER takes the rest), and the
/// remove column beyond the selection area with the gap before it.
const NAME_W: f32 = 220.0;
const SYM_W: f32 = 96.0;
const REMOVE_W: f32 = 36.0;
const TRAIL: f32 = REMOVE_W + 8.0;
/// The table's rows, and a modal list's.
const ROW_H: f32 = 40.0;
const LIST_ROW_H: f32 = 30.0;
/// Where the kit's table starts a row's first cell, and how far its first
/// row stands under the table's top (the headings and the room under
/// them): the empty list says so there.
const CELL_PAD: f32 = 10.0;
const HEAD_H: f32 = 34.0;
/// The modals' widths: the canvas's Name window, and the wider one the
/// lists of folders and the typed path get.
const NARROW: f32 = 560.0;
const WIDE: f32 = 640.0;
/// The name field's box, the canvas's.
const NAME_BOX_W: f32 = 264.0;
/// The most rows a modal list shows: the cells' browser and suggestions.
const BROWSE_ROWS: usize = 11;
const SUGGEST_ROWS: usize = 6;
/// The kit's window, which the gate's copies: its padding, the room under
/// its title, and the title's row.
const MODAL_PAD: f32 = 24.0;
const MODAL_GAP: f32 = 20.0;
const TITLE_H: f32 = 20.0;

/// One pass of the room over the whole of `ui`, the room's area less its
/// first row, with `composition` the input method's uncommitted text for
/// the focused field. What it returns is the focused field's caret and a
/// click that ended the room (none can: leaving is a key's).
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    // The host has painted the ground and styled egui; styling again is a
    // no-op there, and all a bare context needs.
    let pal = Pal::now();
    style_once(ui.ctx(), &pal);
    let full = ui.max_rect();
    let column = Rect::from_min_max(full.min + vec2(PAD_X, PAD_TOP), full.max - vec2(PAD_X, PAD_BOTTOM));
    let note_rect = Rect::from_min_max(pos2(column.min.x, column.max.y - NOTE_H), column.max);
    let body_rect = Rect::from_min_max(column.min, pos2(column.max.x, (note_rect.min.y - GAP).max(column.min.y)));

    let mut acts: Vec<Act> = Vec::new();
    let mut ui_body =
        ui.new_child(UiBuilder::new().id_salt("libraries-body").max_rect(body_rect).layout(Layout::top_down(Align::Min)));
    ui_body.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
    libraries(&mut ui_body, room, &pal, &mut acts);
    note_line(ui, note_rect, room.busy.as_deref(), room.note.as_ref(), &pal);

    // A modal makes the room beneath inert: what was clicked under it this
    // pass is dropped, as the cells view drops what the room registered.
    if !matches!(room.modal, Modal::None) {
        acts.clear();
    }
    let mut caret = None;
    match &room.modal {
        Modal::None => {}
        Modal::Browser(browse) => browser(ui, full, browse, &pal, &mut acts),
        Modal::PathEntry(draft) => caret = path_entry(ui, full, draft, composition, &pal, &mut acts),
        Modal::Name(draft) => caret = name(ui, full, draft, composition, &pal, &mut acts),
        Modal::Remove(i) => remove(ui, full, room, *i, &pal, &mut acts),
    }

    let mut outcome = None;
    for act in acts {
        outcome = outcome.or(room.act(act));
    }
    // A click that closed the field's modal took the keyboard with it.
    if !matches!(room.modal, Modal::Name(_) | Modal::PathEntry(_)) {
        caret = None;
    }
    EguiShown { caret, outcome }
}

// ── The room ──────────────────────────────────────────────────────────────

/// The add card, then the table: NAME · FOLDER · SYMLINKS, the remove
/// button in a column of its own past the selection area. Rows take no
/// click as a whole, only the tick box and the remove; the cursor's ring
/// is the keys'.
fn libraries(ui: &mut Ui, room: &mut Room, pal: &Pal, acts: &mut Vec<Act>) {
    if add_card(ui, Id::new("libraries-add"), &t!("folders.add_card"), pal).clicked() {
        acts.push(Act::Add);
    }
    ui.add_space(GAP);

    let cols = [
        Col::left(t!("folders.col_name"), NAME_W),
        Col::left(t!("folders.col_folder"), 0.0),
        Col::center(t!("admin.col_symlinks"), SYM_W),
    ];
    let top = ui.cursor().min;
    // A fresh add's row is brought into view, not selected; the cells view
    // takes the same note when it draws. egui moves the table on the pass
    // after this one and shows the move on the pass after that, and asks
    // its host for both.
    let reveal = room.reveal.take();
    let mut revealed = false;
    let libs = &room.libs;
    let table = Table::new(Id::new("libraries-table"), &cols, libs.len()).cursor(room.sel).row_height(ROW_H).trail(TRAIL).inert();
    table.show(ui, pal, |ui, i, row| {
        if let Some(j) = reveal
            && !revealed
        {
            revealed = true;
            let to = row.rect.translate(vec2(0.0, (j as f32 - i as f32) * ROW_H));
            ui.scroll_to_rect_animation(to, None, ScrollAnimation::none());
        }
        let lib = &libs[i];
        let y = row.rect.center().y;
        // The vpath, plain: the server has no rename, so nothing here
        // offers one. A long one is cut at its column with no ellipsis, as
        // the canvas and the cells cut it.
        let font = if row.selected { strong(14.0) } else { body(14.0) };
        clipped(ui, row.cell(0), &lib.name, font, ink(row, pal.text, pal));
        // The folder keeps its leaf; the whole path rides a tip when cut.
        let (x, w) = row.cells[1];
        let (shown, cut) = cut_start(ui, &lib.root, mono(12.5), w);
        text(ui, pos2(x, y), &shown, mono(12.5), ink(row, pal.soft, pal), w, Align2::LEFT_CENTER);
        if cut {
            let _ = ui.interact(row.cell(1), Id::new("libraries-root").with(i), Sense::hover()).on_hover_text(lib.root.as_str());
        }
        if checkbox(ui, Id::new("libraries-symlinks").with(i), row.cell(2).center(), lib.follow_symlinks, pal).clicked() {
            acts.push(Act::ToggleSymlinks(i));
        }
        let at = pos2(row.trail.max.x - REMOVE_W / 2.0, y);
        if icon_button(ui, Id::new("libraries-remove").with(i), at, Icon::Remove, pal.dim, pal).clicked() {
            acts.push(Act::RemoveAt(i));
        }
    });
    // The table's headings stay; an empty list says so where the first row
    // would be.
    if libs.is_empty() {
        let at = pos2(top.x + CELL_PAD, top.y + HEAD_H + ROW_H / 2.0);
        text(ui, at, &t!("folders.nothing_yet"), body(14.0), pal.dim, ui.max_rect().width() - CELL_PAD, Align2::LEFT_CENTER);
    }
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The server's folders: the listed path, its folders (a click moves the
/// cursor, as in the cells), and the four acts — up and open on the left,
/// close and the add on the right, the add lit.
fn browser(ui: &Ui, over: Rect, browse: &Browse, pal: &Pal, acts: &mut Vec<Act>) {
    let id = Id::new("libraries-browse");
    let shown = modal(ui, id, over, WIDE, &t!("browse.title"), pal, |ui| {
        let w = ui.available_width();
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        let (path, _) = cut_start(ui, &browse.path, mono(12.5), w);
        text(ui, line.left_center(), &path, mono(12.5), pal.dim, w, Align2::LEFT_CENTER);
        ui.add_space(10.0);
        let rows = browse.dirs.len().clamp(1, BROWSE_ROWS) as f32;
        let h = (rows * LIST_ROW_H).min(ui.available_height() - 16.0 - FORM_BUTTON_H).max(LIST_ROW_H);
        let mut act = None;
        if browse.dirs.is_empty() {
            let (region, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            let at = pos2(region.min.x + CELL_PAD, region.min.y + LIST_ROW_H / 2.0);
            text(ui, at, &t!("browse.empty"), body(14.0), pal.dim, w - CELL_PAD, Align2::LEFT_CENTER);
        } else {
            let content = Id::new(&browse.path);
            let picked = list(ui, id.with("list"), h, browse.dirs.len(), Some(browse.sel), content, pal, |ui, i, rect, on, _| {
                folder_row(ui, rect, &browse.dirs[i], on, pal.text, pal);
            });
            act = picked.map(Act::BrowseRow);
        }
        ui.add_space(16.0);
        let start = [(t!("browse.up").to_string(), false), (t!("browse.open").to_string(), false)];
        let end = [(t!("browse.close").to_string(), false), (t!("browse.add").to_string(), true)];
        let meant = [Act::BrowseUp, Act::BrowseEnter, Act::BrowseCancel, Act::BrowseAdd];
        foot(ui, id.with("foot"), &start, &end, pal).map(|k| meant[k].clone()).or(act)
    });
    if shown.closed {
        acts.push(Act::BrowseCancel);
    }
    acts.extend(shown.inner);
}

/// The typed path: the field (the room's, its value and caret, never
/// edited here) and under it the server's suggestions, a click taking
/// one, or the gold reason the directory could not be listed. The list's
/// room is kept whether or not there is anything in it, so the window
/// holds still as the suggestions come and go (the cells anchor theirs).
fn path_entry(ui: &Ui, over: Rect, draft: &PathDraft, composition: &str, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    let id = Id::new("libraries-path");
    let shown = modal(ui, id, over, WIDE, &t!("path_modal.title"), pal, |ui| {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
        let input = studio::Input::new(draft.text.value(), draft.text.cursor()).focused(true, composition).mono();
        let typed = field_box(ui, id.with("field"), rect, &input, pal);
        ui.add_space(12.0);
        let h = (SUGGEST_ROWS as f32 * LIST_ROW_H).min(ui.available_height()).max(LIST_ROW_H);
        let mut picked = None;
        if let Some(err) = &draft.error {
            let (region, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            let at = pos2(region.min.x, region.min.y + LIST_ROW_H / 2.0);
            text(ui, at, err, body(13.0), pal.gold, w, Align2::LEFT_CENTER);
        } else {
            // The suggestions are the listing filtered: walked once a pass
            // for the rows on screen, never collected whole (a listing can
            // run to thousands).
            let (mut from, mut window): (usize, Vec<&str>) = (0, Vec::new());
            let content = Id::new(draft.text.value());
            picked = list(ui, id.with("list"), h, draft.suggestion_count(), draft.sel, content, pal, |ui, i, rect, on, hovered| {
                if i < from || i >= from + window.len() {
                    from = i;
                    window = draft.suggestions().skip(i).take(4 * SUGGEST_ROWS).collect();
                }
                // Dim, the pointer's brighter, as the cells draw them.
                if let Some(entry) = window.get(i - from) {
                    folder_row(ui, rect, entry, on, if hovered { pal.text } else { pal.dim }, pal);
                }
            });
        }
        (typed.caret, picked)
    });
    if shown.closed {
        acts.push(Act::PathCancel);
    }
    let (caret, picked) = shown.inner;
    acts.extend(picked.map(Act::PathSuggest));
    caret
}

/// The Name window every way of adding ends in: the folder, the name field
/// with the charset beside it, the permanence in gold, a refusal's words,
/// and the add.
fn name(ui: &Ui, over: Rect, draft: &NameDraft, composition: &str, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    let id = Id::new("libraries-name");
    let shown = modal(ui, id, over, NARROW, &t!("admin.name_title"), pal, |ui| {
        let w = ui.available_width();
        ui.add_space(2.0);
        label(ui, &t!("admin.name_folder"), pal);
        ui.add_space(6.0);
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        let (folder, _) = cut_start(ui, &draft.directory, mono(12.5), w);
        text(ui, line.left_center(), &folder, mono(12.5), pal.text, w, Align2::LEFT_CENTER);
        ui.add_space(18.0);

        label(ui, &t!("admin.name_label"), pal);
        ui.add_space(6.0);
        let charset = t!("admin.name_charset").to_string();
        let charset_w = width_of(ui, &charset, body(13.0));
        let box_w = NAME_BOX_W.min(w);
        let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
        let input = studio::Input::new(draft.name.value(), draft.name.cursor()).focused(true, composition);
        let typed = field_box(ui, id.with("field"), Rect::from_min_size(row.min, vec2(box_w, FIELD_H)), &input, pal);
        // Beside the box where it fits, as the canvas sets it; under it
        // where it does not.
        if box_w + 16.0 + charset_w <= w {
            let at = pos2(row.min.x + box_w + 16.0, row.center().y);
            text(ui, at, &charset, body(13.0), pal.dim, charset_w + 1.0, Align2::LEFT_CENTER);
        } else {
            ui.add_space(6.0);
            let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
            text(ui, line.left_center(), &charset, body(13.0), pal.dim, w, Align2::LEFT_CENTER);
        }
        ui.add_space(18.0);

        para(ui, &t!("admin.name_permanent_1"), 13.0, pal.gold);
        para(ui, &t!("admin.name_permanent_2"), 13.0, pal.gold);
        ui.add_space(12.0);
        // The refusal's line is kept empty until there is one, so the
        // window does not grow under the hand that pressed Add.
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        if let Some(err) = &draft.error {
            text(ui, line.left_center(), err, body(13.0), pal.gold, w, Align2::LEFT_CENTER);
        }
        ui.add_space(12.0);
        let add = foot(ui, id.with("foot"), &[], &[(t!("admin.name_add").to_string(), true)], pal).is_some();
        (typed.caret, add)
    });
    if shown.closed {
        acts.push(Act::NameCancel);
    }
    let (caret, add) = shown.inner;
    if add {
        acts.push(Act::NameAdd);
    }
    caret
}

/// The warning gate before a remove, in its gold window with no close (a
/// gate is answered, as the cells draw it): what stays and what goes, then
/// Keep (the safe choice, lit) and Remove.
fn remove(ui: &Ui, over: Rect, room: &Room, i: usize, pal: &Pal, acts: &mut Vec<Act>) {
    let id = Id::new("libraries-gate");
    let name = room.libs.get(i).map(|l| l.name.clone()).unwrap_or_default();
    let picked = gate(ui, id, over, NARROW, &t!("admin.remove_title", name = name), pal, |ui| {
        para(ui, &t!("admin.remove_files_stay"), 13.0, pal.ok);
        ui.add_space(6.0);
        para(ui, &t!("admin.remove_users", name = name), 13.0, pal.gold);
        para(ui, &t!("admin.remove_tracks"), 13.0, pal.gold);
        ui.add_space(20.0);
        let end = [(t!("admin.remove_keep").to_string(), true), (t!("admin.remove_confirm").to_string(), false)];
        match foot(ui, id.with("foot"), &[], &end, pal) {
            Some(0) => Some(Act::RemoveCancel),
            Some(_) => Some(Act::RemoveConfirm),
            None => None,
        }
    });
    acts.extend(picked);
}

/// The kit's [`modal`] as a warning gate draws it: the same scrim
/// swallowing presses beside the window, the same window sized to its
/// body, but no close (the cells draw a gate no [X]: it forces an explicit
/// choice, its keys and its two buttons), and its title and frame in the
/// warning's gold, as the cells frame it. What the body returned.
fn gate<R>(ui: &Ui, id: Id, over: Rect, width: f32, title: &str, pal: &Pal, body: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let width = width.min(over.width() - 32.0).max(over.width().min(160.0));
    let max_h = (over.height() - 32.0).max(2.0 * MODAL_PAD + TITLE_H);
    let size_key = id.with("height");
    let known: Option<f32> = ctx.data(|d| d.get_temp(size_key));
    let window = Rect::from_center_size(over.center(), vec2(width, known.unwrap_or(max_h).min(max_h)));
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(over.min).constrain(false).fade_in(false);
    let shown = area.show(&ctx, |ui| {
        let _scrim = ui.allocate_rect(over, Sense::click_and_drag());
        ui.painter().rect_filled(over, 0.0, SCRIM);
        let shadow = Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(115) };
        ui.painter().add(shadow.as_shape(window, square()));
        ui.painter().rect_filled(window, square(), pal.panel);
        ui.painter().rect_stroke(window, square(), Stroke::new(1.0, pal.gold), StrokeKind::Inside);
        let inner = window.shrink(MODAL_PAD);
        let title_row = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_H));
        text(ui, title_row.left_center(), title, strong(16.0), pal.gold, inner.width(), Align2::LEFT_CENTER);
        let budget = (max_h - 2.0 * MODAL_PAD - TITLE_H - MODAL_GAP).max(0.0);
        let body_rect = Rect::from_min_size(pos2(inner.min.x, title_row.max.y + MODAL_GAP), vec2(inner.width(), budget));
        let mut child =
            ui.new_child(UiBuilder::new().id_salt(id.with("body")).max_rect(body_rect).layout(Layout::top_down(Align::Min)));
        let out = ScrollArea::vertical().id_salt(id.with("scroll")).max_height(budget).auto_shrink([false, true]).show(&mut child, body);
        let wanted = 2.0 * MODAL_PAD + TITLE_H + MODAL_GAP + out.content_size.y.min(budget);
        (out.inner, wanted.min(max_h))
    });
    let (inner, wanted) = shown.inner;
    if known.is_none_or(|h| (h - wanted).abs() > 0.5) {
        ctx.data_mut(|d| d.insert_temp(size_key, wanted));
        ctx.request_discard("a gate took its body's height");
    }
    inner
}

// ── Pieces ────────────────────────────────────────────────────────────────

/// A field's label: the kit's small capitals, dim, a 14-point row.
fn label(ui: &mut Ui, s: &str, pal: &Pal) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    small_caps(ui, rect.left_center(), s, pal.dim, w, Align2::LEFT_CENTER);
}

/// Words wrapped to the column in the UI face at `size`, the canvas's
/// 1.45 lines apart, a row of the layout as tall as they run.
fn para(ui: &mut Ui, s: &str, size: f32, color: Color32) {
    let w = ui.available_width();
    let format = TextFormat { font_id: body(size), color, line_height: Some(size * 1.45), ..Default::default() };
    let mut job = LayoutJob::single_section(s.to_string(), format);
    job.wrap.max_width = w;
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

/// `s` in one line from `rect`'s left, its middle on `rect`'s, cut plainly
/// at the rect's right edge: no ellipsis, as the canvas clips a long name.
fn clipped(ui: &Ui, rect: Rect, s: &str, font: FontId, color: Color32) {
    let galley = ui.painter().layout_no_wrap(s.to_string(), font, color);
    let at = pos2(rect.min.x, rect.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).galley(at, galley, color);
}

/// A folder in a modal list: an LED, lit on the keyboard's row as the
/// hallway lights its open room's, and the name.
fn folder_row(ui: &Ui, rect: Rect, name: &str, on: bool, color: Color32, pal: &Pal) {
    led(ui, pos2(rect.min.x + 13.0, rect.center().y), if on { pal.accent } else { pal.unlit });
    let color = if on { pal.text } else { color };
    text(ui, pos2(rect.min.x + 26.0, rect.center().y), name, body(14.0), color, rect.width() - 36.0, Align2::LEFT_CENTER);
}

/// `path` cut at its start to fit `max_w` in `font`, an ellipsis standing
/// for what went, so the leaf stays in view (the canvas's paths, and the
/// cells'): the line, and whether it was cut.
fn cut_start(ui: &Ui, path: &str, font: FontId, max_w: f32) -> (String, bool) {
    if width_of(ui, path, font.clone()) <= max_w {
        return (path.to_string(), false);
    }
    // The fewest graphemes dropped from the head that let the rest fit:
    // the narrower the tail the likelier, so a binary search finds it.
    let starts: Vec<usize> = path.grapheme_indices(true).map(|(at, _)| at).collect();
    let fits = |k: usize| {
        let tail = starts.get(k).map_or("", |&at| &path[at..]);
        width_of(ui, &format!("…{tail}"), font.clone()) <= max_w
    };
    let (mut lo, mut hi) = (1, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let tail = starts.get(lo).map_or("", |&at| &path[at..]);
    (format!("…{tail}"), true)
}

/// A modal's foot: form buttons in a row, `start` laid from its left edge
/// and `end` ending at its right, 10 points apart, each its label's width.
/// The one clicked, counting `start` first.
fn foot(ui: &mut Ui, id: Id, start: &[(String, bool)], end: &[(String, bool)], pal: &Pal) -> Option<usize> {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
    let mut clicked = None;
    let mut x = row.min.x;
    for (k, (label, primary)) in start.iter().enumerate() {
        let bw = form_button_width(ui, label);
        let rect = Rect::from_min_size(pos2(x, row.min.y), vec2(bw, FORM_BUTTON_H));
        if form_button(ui, id.with(k), rect, label, *primary, pal).clicked() {
            clicked = Some(k);
        }
        x += bw + 10.0;
    }
    let mut x = row.max.x;
    for (k, (label, primary)) in end.iter().enumerate().rev() {
        let bw = form_button_width(ui, label);
        let rect = Rect::from_min_size(pos2(x - bw, row.min.y), vec2(bw, FORM_BUTTON_H));
        if form_button(ui, id.with(start.len() + k), rect, label, *primary, pal).clicked() {
            clicked = Some(start.len() + k);
        }
        x -= bw + 10.0;
    }
    clicked
}

/// A list of folders in a modal, `h` points of its body: `len` rows
/// scrolled by egui with only those on screen drawn (a listing can run to
/// thousands), the keyboard's row raised and ringed and brought into view
/// on the pass it moved, a row under the pointer raised. A new `content`
/// (another directory, another draft) starts at the top, as the cells'
/// lists do, and so does a list whose window was down on the last pass:
/// a window opened again on the listing it last showed shows its cursor,
/// not where the wheel left it. `draw` paints a row's words, told whether
/// it is the cursor's and whether the pointer is on it; returns the row a
/// click landed on.
#[allow(clippy::too_many_arguments)]
fn list(
    ui: &mut Ui,
    id: Id,
    h: f32,
    len: usize,
    cursor: Option<usize>,
    content: Id,
    pal: &Pal,
    mut draw: impl FnMut(&Ui, usize, Rect, bool, bool),
) -> Option<usize> {
    let w = ui.available_width();
    let (region, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut inner = ui.new_child(UiBuilder::new().id_salt(id).max_rect(region).layout(Layout::top_down(Align::Min)));
    let memory = id.with("view");
    let now = ui.ctx().cumulative_pass_nr();
    let (last_pass, last_content, last_cursor, last_offset): (u64, Id, Option<usize>, f32) =
        ui.ctx().data(|d| d.get_temp(memory)).unwrap_or((0, Id::NULL, None, 0.0));
    let fresh = last_content != content || last_pass + 1 != now;
    let mut offset = if fresh { 0.0 } else { last_offset };
    let mut moved = fresh;
    if let Some(s) = cursor
        && (fresh || cursor != last_cursor)
    {
        let (top, bottom) = (s as f32 * LIST_ROW_H, (s + 1) as f32 * LIST_ROW_H);
        if top < offset {
            (offset, moved) = (top, true);
        } else if bottom > offset + h {
            (offset, moved) = (bottom - h, true);
        }
    }
    let mut scroll = ScrollArea::vertical().id_salt(id.with("scroll")).auto_shrink([false, false]);
    if moved {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    let mut clicked = None;
    let out = scroll.show_rows(&mut inner, LIST_ROW_H, len, |ui, range| {
        for i in range {
            let (_, rect) = ui.allocate_space(vec2(ui.available_width(), LIST_ROW_H));
            let response = ui.interact(rect, id.with("row").with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let on = cursor == Some(i);
            let hovered = response.hovered();
            if on || hovered {
                ui.painter().rect_filled(rect, square(), pal.raised);
            }
            if on {
                ring(ui, rect, pal);
            }
            draw(ui, i, rect, on, hovered);
            if response.clicked() {
                clicked = Some(i);
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(memory, (now, content, cursor, out.state.offset.y)));
    clicked
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::Vec2;

    use super::super::tests::{english, key, room, seeded, type_text};
    use super::*;
    use crate::api::types::DirEntry;

    /// The room's region in the default 100×30 window, in points.
    const SIZE: Vec2 = vec2(800.0, 420.0);

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        ctx
    }

    /// One pass of the room with `events`, the input method composing
    /// `composition`: what egui drew, and what the room told its host.
    fn pass(ctx: &egui::Context, room: &mut Room, events: Vec<egui::Event>, composition: &str) -> (egui::FullOutput, EguiShown) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), events, ..Default::default() };
        let mut shown = None;
        let mut out = ctx.run_ui(input, |ui| {
            let this = show(ui, room, composition);
            // A discarded pass's click stands, as the host keeps it.
            let outcome = shown.as_ref().and_then(|s: &EguiShown| s.outcome).or(this.outcome);
            shown = Some(EguiShown { caret: this.caret, outcome });
        });
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        (out, shown.expect("the pass ran"))
    }

    /// Two passes (egui sizes some things on the first), the second's.
    fn settle(ctx: &egui::Context, room: &mut Room) -> (egui::FullOutput, EguiShown) {
        let _ = pass(ctx, room, Vec::new(), "");
        pass(ctx, room, Vec::new(), "")
    }

    /// A press and a release at `at`, a pass each after the move, as hands
    /// would make them: what the room told its host, the outcome over the
    /// three and the caret of the release's pass, the one the click acted in.
    fn press(ctx: &egui::Context, room: &mut Room, at: Pos2) -> EguiShown {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut told = EguiShown { caret: None, outcome: None };
        for events in [vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
            let (_, shown) = pass(ctx, room, events, "");
            told = EguiShown { caret: shown.caret, outcome: told.outcome.or(shown.outcome) };
        }
        told
    }

    /// [`press`] on the middle of what `id` drew last pass.
    fn click(ctx: &egui::Context, room: &mut Room, id: Id) -> EguiShown {
        let at = ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect.center();
        press(ctx, room, at)
    }

    /// The wheel turned over `at`, `dy` points of a trackpad's swipe (a
    /// positive one moves the content down, toward its top), a pass for the
    /// pointer's move, one for the swipe and one for its end.
    fn wheel(ctx: &egui::Context, room: &mut Room, at: Pos2, dy: f32) {
        let turn = |phase, dy| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase,
            modifiers: egui::Modifiers::default(),
        };
        let _ = pass(ctx, room, vec![egui::Event::PointerMoved(at)], "");
        let _ = pass(ctx, room, vec![turn(egui::TouchPhase::Start, 0.0), turn(egui::TouchPhase::Move, dy)], "");
        let _ = pass(ctx, room, vec![turn(egui::TouchPhase::End, 0.0)], "");
    }

    /// Row or button `k` of the control `id` names: an index, as the view
    /// numbers them (a bare literal would hash as another integer type).
    fn nth(id: Id, k: usize) -> Id {
        id.with(k)
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

    fn has(drawn: &[String], words: &str) -> bool {
        drawn.iter().any(|d| d == words)
    }

    /// The colour `words` were drawn in, where the pass drew them.
    fn color_of(out: &egui::FullOutput, words: &str) -> Option<Color32> {
        out.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.text() == words => text.galley.job.sections.first().map(|s| s.format.color),
            _ => None,
        })
    }

    fn listing(path: &str, names: &[&str]) -> DirListing {
        DirListing {
            path: path.into(),
            directories: names.iter().map(|n| DirEntry { name: (*n).into() }).collect(),
            files: Vec::new(),
        }
    }

    #[test]
    fn the_room_draws_its_card_its_table_and_its_note_and_no_header_or_tips() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        room.queue(Op::Load, t!("admin.busy_loading"));
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Add a music folder", "NAME", "FOLDER", "SYMLINKS", "music", "/srv/music", "vinyl", "/Volumes/NAS/Vinyl Rips", "loading libraries…"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "Libraries") && !words.iter().any(|w| w.contains("b browse")), "no header, no tips: {words:?}");
        assert!(shown.caret.is_none() && shown.outcome.is_none(), "no field has the keyboard");
        let pal = Pal::now();
        assert_eq!(color_of(&out, "loading libraries…"), Some(pal.accent), "the busy line in the accent");

        // Empty, the headings stay and the list says so; a gold note shows
        // once nothing is busy.
        room.libs.clear();
        room.busy = None;
        room.note = Some(("could not load libraries: could not reach server".into(), true));
        let (out, _) = settle(&ctx, &mut room);
        let words = drawn(&out);
        assert!(has(&words, "(nothing added yet)") && has(&words, "NAME"), "{words:?}");
        assert_eq!(color_of(&out, "could not load libraries: could not reach server"), Some(pal.gold), "{words:?}");
    }

    #[test]
    fn a_click_on_each_control_in_the_room_applies_the_cells_act() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        let _ = settle(&ctx, &mut room);

        // A row takes no click as a whole: the cursor is the keys'.
        let row = ctx.read_response(nth(Id::new("libraries-symlinks"), 0)).expect("row 0 drawn").rect;
        assert_eq!(press(&ctx, &mut room, pos2(PAD_X + 2.0 * CELL_PAD, row.center().y)).outcome, None);
        assert_eq!(room.sel, None, "a click on a row selects nothing");
        assert!(room.queued.is_none() && matches!(room.modal, Modal::None));

        // The add card is `b`: the server's browser, this not being its machine.
        assert_eq!(click(&ctx, &mut room, Id::new("libraries-add")).outcome, None, "no click ends the room");
        assert_eq!(room.queued, Some(Op::OpenBrowser("~".into())));
        room.queued = None;

        // The tick box flips its own row's flag, through the server.
        click(&ctx, &mut room, nth(Id::new("libraries-symlinks"), 0));
        assert_eq!(room.queued, Some(Op::Symlinks { vpath: "music".into(), on: true }));
        assert!(!room.libs[0].follow_symlinks, "not until the server agrees");
        room.queued = None;

        // The remove opens its row's gate.
        click(&ctx, &mut room, nth(Id::new("libraries-remove"), 1));
        assert!(matches!(room.modal, Modal::Remove(1)));
    }

    #[test]
    fn the_cursor_row_is_scrolled_into_view_and_a_long_folder_keeps_its_leaf() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        room.libs = (0..40)
            .map(|i| Lib { name: format!("lib{i:02}"), root: format!("/srv/{}/leaf{i:02}", "deep".repeat(40)), follow_symlinks: false })
            .collect();
        let _ = settle(&ctx, &mut room);
        // Where the table's rows stop: the note line's gap above it. A row
        // is whole in view when its foot, half a row under its tick box's
        // middle, stands no lower.
        let floor = SIZE.y - PAD_BOTTOM - NOTE_H - GAP;
        let foot = |tick: Rect| tick.center().y + ROW_H / 2.0;
        let tick = nth(Id::new("libraries-symlinks"), 30);
        assert!(ctx.read_response(tick).is_none(), "row 30 starts off screen");
        for _ in 0..31 {
            room.key(key(KeyCode::Down));
        }
        assert_eq!(room.sel, Some(30));
        let (out, _) = settle(&ctx, &mut room);
        let row = ctx.read_response(tick).expect("the cursor's row is drawn").rect;
        assert!(foot(row) <= floor + 0.5, "and whole above the note line: {row:?}");
        let words = drawn(&out);
        assert!(words.iter().any(|w| w.starts_with('…') && w.ends_with("leaf30")), "the folder keeps its leaf: {words:?}");

        // Brought into view once, on the pass it moved: the wheel takes the
        // table away from the cursor's row, and no later pass brings it back.
        wheel(&ctx, &mut room, row.center(), 4000.0);
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(tick).is_none(), "the wheel moved the table off the cursor's row");
        assert_eq!(room.sel, Some(30), "and left the cursor where it was");

        // A fresh add's row is brought into view and not taken by the cursor.
        for _ in 0..31 {
            room.key(key(KeyCode::Up));
        }
        assert_eq!(room.sel, Some(0));
        let _ = settle(&ctx, &mut room);
        room.key(key(KeyCode::Esc));
        let _ = settle(&ctx, &mut room);
        let tick = nth(Id::new("libraries-symlinks"), 36);
        assert!(ctx.read_response(tick).is_none(), "row 36 is off screen with the cursor gone home");
        // egui moves the table on the pass after the ask and shows it on
        // the pass after that, and asks the host for both.
        room.reveal = Some(36);
        for _ in 0..2 {
            let (out, _) = pass(&ctx, &mut room, Vec::new(), "");
            let again = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay);
            assert_eq!(again, Some(std::time::Duration::ZERO), "egui asks for the next pass");
        }
        let _ = pass(&ctx, &mut room, Vec::new(), "");
        let row = ctx.read_response(tick).expect("the revealed row is drawn").rect;
        assert!(foot(row) <= floor + 0.5, "and whole above the note line: {row:?}");
        assert_eq!((room.sel, room.reveal), (None, None), "revealed, not selected, and only once");
    }

    #[test]
    fn the_remove_gate_draws_its_facts_has_no_close_and_keeps_or_removes() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        let _ = settle(&ctx, &mut room);
        let card = ctx.read_response(Id::new("libraries-add")).expect("the card").rect.center();
        room.act(Act::RemoveAt(1));
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Remove vinyl from mStream?", "The folder and its files stay on disk.", "− Users who hold vinyl lose it; playlists into it break.", "◂ KEEP IT", "REMOVE"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(shown.caret.is_none(), "a gate has no field");
        let gate = Id::new("libraries-gate");
        assert!(ctx.read_response(gate.with("close")).is_none(), "a gate has no close: it is answered");

        // The room beneath is inert: a press where the card stands does nothing.
        for pressed in [true, false] {
            let press = egui::Event::PointerButton { pos: card, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
            let _ = pass(&ctx, &mut room, vec![egui::Event::PointerMoved(card), press], "");
        }
        assert!(room.queued.is_none() && matches!(room.modal, Modal::Remove(1)), "the scrim swallowed it");

        click(&ctx, &mut room, nth(gate.with("foot"), 0));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Keep keeps");
        room.act(Act::RemoveAt(1));
        let _ = settle(&ctx, &mut room);
        click(&ctx, &mut room, nth(gate.with("foot"), 1));
        assert_eq!(room.queued, Some(Op::Remove("vinyl".into())));
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn the_name_window_draws_the_rooms_field_and_its_caret_and_adds_or_cancels() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        room.open_name("/Volumes/NAS/Field Recordings 2024".into());
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Name this library", "FOLDER", "NAME", "/Volumes/NAS/Field Recordings 2024", "field-recordings-2024", "a-z, 0-9 and dashes", "ADD FOLDER ▸"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(has(&words, &t!("admin.name_permanent_1")), "the permanence, in its own words: {words:?}");
        let field = ctx.read_response(Id::new("libraries-name").with("field")).expect("the field").rect;
        let caret = shown.caret.expect("the field has the keyboard");
        assert!(field.contains(caret - vec2(0.0, 1.0)), "the caret's foot is in the box: {caret:?} {field:?}");

        // The input method's text stands at the caret; the keys still edit
        // the room's own value, through its gate.
        let (out, _) = pass(&ctx, &mut room, Vec::new(), "かな");
        assert!(has(&drawn(&out), "field-recordings-2024かな"), "{:?}", drawn(&out));
        type_text(&mut room, "X");
        let (out, _) = settle(&ctx, &mut room);
        assert!(has(&drawn(&out), "field-recordings-2024x"));

        click(&ctx, &mut room, nth(Id::new("libraries-name").with("foot"), 0));
        let directory = "/Volumes/NAS/Field Recordings 2024".to_string();
        assert_eq!(room.queued, Some(Op::Add { directory, vpath: "field-recordings-2024x".into() }));
        room.queued = None;
        room.apply(Done::Added {
            vpath: "field-recordings-2024x".into(),
            result: Err(crate::api::ApiError::Server { status: 500, message: "EACCES".into() }),
        });
        let (out, _) = settle(&ctx, &mut room);
        assert!(drawn(&out).iter().any(|w| w.starts_with("could not add") && w.contains("EACCES")), "{:?}", drawn(&out));

        // The close takes the keyboard from the field on the pass it acts
        // in: no caret is left noted for the window that is gone.
        let shown = click(&ctx, &mut room, Id::new("libraries-name").with("close"));
        assert!(matches!(room.modal, Modal::None));
        assert!(shown.caret.is_none(), "the closing pass notes no caret");
        let (_, shown) = settle(&ctx, &mut room);
        assert!(shown.caret.is_none(), "no field, no caret");
    }

    #[test]
    fn the_path_window_suggests_and_a_click_takes_one() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        room.key(key(KeyCode::Char('t')));
        type_text(&mut room, "/srv/m");
        room.queued = None;
        room.apply(Done::Completed { dir: "/srv/".into(), listing: Ok(listing("/srv", &["media", "music", "www"])) });
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Type the full path of a music folder", "/srv/m", "media", "music"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "www"), "only what the partial matches");
        assert!(shown.caret.is_some(), "the typed path has the keyboard");

        let list = Id::new("libraries-path").with("list");
        click(&ctx, &mut room, nth(list.with("row"), 1));
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path window") };
        assert_eq!(draft.text.value(), "/srv/music/", "the suggestion taken, completion going on inside it");
        assert_eq!(room.queued, Some(Op::Complete("/srv/music/".into())));
        room.queued = None;

        // The current directory refused: its reason, in gold, where the list was.
        room.apply(Done::Completed { dir: "/srv/music/".into(), listing: Err("EACCES".into()) });
        let (out, _) = settle(&ctx, &mut room);
        assert_eq!(color_of(&out, "could not list /srv/music/: EACCES"), Some(Pal::now().gold), "{:?}", drawn(&out));

        click(&ctx, &mut room, Id::new("libraries-path").with("close"));
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn the_browser_moves_opens_climbs_adds_and_closes() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        let home = || Done::Browsed(Ok(listing("/home/sherika", &["Audiobooks", "Downloads", "Music", "Videos"])));
        room.apply(home());
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Browse the server's folders", "/home/sherika", "Audiobooks", "Videos", "◂ UP", "OPEN", "CLOSE", "ADD THIS FOLDER ▸"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(shown.caret.is_none());

        let browse = Id::new("libraries-browse");
        click(&ctx, &mut room, nth(browse.with("list").with("row"), 2));
        let Modal::Browser(b) = &room.modal else { panic!("the browser") };
        assert_eq!(b.sel, 2, "a click moves the cursor and opens nothing");
        assert!(room.queued.is_none());

        let foot = browse.with("foot");
        click(&ctx, &mut room, nth(foot, 1));
        assert_eq!(room.queued, Some(Op::BrowseTo("/home/sherika/Music".into())), "open");
        room.queued = None;
        click(&ctx, &mut room, nth(foot, 0));
        assert_eq!(room.queued, Some(Op::BrowseTo("/home".into())), "up");
        room.queued = None;
        click(&ctx, &mut room, nth(foot, 3));
        let Modal::Name(draft) = &room.modal else { panic!("the add names the folder listed") };
        assert_eq!(draft.directory, "/home/sherika");

        room.apply(home());
        let _ = settle(&ctx, &mut room);
        click(&ctx, &mut room, nth(foot, 2));
        assert!(matches!(room.modal, Modal::None), "close");
        room.apply(home());
        let _ = settle(&ctx, &mut room);
        click(&ctx, &mut room, browse.with("close"));
        assert!(matches!(room.modal, Modal::None), "the window's close");

        room.apply(Done::Browsed(Ok(listing("/srv/empty", &[]))));
        let (out, _) = settle(&ctx, &mut room);
        assert!(has(&drawn(&out), "(no folders in here)"), "{:?}", drawn(&out));
    }

    #[test]
    fn the_browsers_cursor_row_is_kept_in_view_and_a_window_opened_again_shows_it() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        let names: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let srv = || Done::Browsed(Ok(listing("/srv", &names)));
        let browse = Id::new("libraries-browse");
        let rows = browse.with("list").with("row");
        // The list ends 16 points above the window's buttons.
        let list_foot = |ctx: &egui::Context| ctx.read_response(nth(browse.with("foot"), 0)).expect("the buttons").rect.min.y - 16.0;
        room.apply(srv());
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(nth(rows, 0)).is_some(), "the list opens at its top");
        assert!(ctx.read_response(nth(rows, 25)).is_none(), "with row 25 below it");

        // The keys move the cursor and the list follows it, the row whole.
        for _ in 0..25 {
            room.key(key(KeyCode::Down));
        }
        let _ = settle(&ctx, &mut room);
        let row = ctx.read_response(nth(rows, 25)).expect("the cursor's row is drawn").rect;
        assert!(row.max.y <= list_foot(&ctx) + 0.5, "and whole in the list: {row:?}");

        // Followed on the pass it moved, not every pass: the wheel takes the
        // list back to its top and leaves the cursor where it was.
        wheel(&ctx, &mut room, row.center(), 4000.0);
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(nth(rows, 25)).is_none(), "the wheel moved the list off the cursor's row");
        assert!(matches!(&room.modal, Modal::Browser(b) if b.sel == 25));

        // The window opened again on the listing it last showed shows its
        // cursor, wherever the wheel last left the list.
        room.key(key(KeyCode::Esc));
        let _ = settle(&ctx, &mut room);
        room.apply(srv());
        let _ = settle(&ctx, &mut room);
        let top = ctx.read_response(nth(rows, 0)).expect("row 0 is drawn").rect;
        wheel(&ctx, &mut room, top.center(), -4000.0);
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(nth(rows, 0)).is_none(), "the wheel took the list to its foot");
        room.key(key(KeyCode::Esc));
        let _ = settle(&ctx, &mut room);
        room.apply(srv());
        let _ = settle(&ctx, &mut room);
        assert!(matches!(&room.modal, Modal::Browser(b) if b.sel == 0));
        assert!(ctx.read_response(nth(rows, 0)).is_some(), "the cursor's row, at the top again");
    }
}
