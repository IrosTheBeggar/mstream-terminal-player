//! The Users room drawn by egui: its face in the desktop window's panel,
//! for the GUI player's Admin tab. The same [`Room`] the cells view draws
//! (the parent module: its state, its worker, its keys, its words); only
//! the drawing is new, in pixels, on the Studio kit
//! (src/gui/window/studio.rs), after the Claude Design canvas of the Admin
//! screen (its Users and Users · add a user boards).
//!
//! Every click is the act the cells view registers for the same place,
//! handed to [`Room::act`] once the pass is drawn, so the keys and the
//! pointer stay one model. The cells' hit order has a click on a flag's
//! box select its row rather than flip the flag (the row's target is
//! registered over the boxes), and so does this face: the box is drawn as
//! the flag's state, and the row takes the click. Keys never reach egui:
//! the fields are drawn from the room's own values and carets, which its
//! key handler edits as it always has. What the room says is the parent's,
//! in the same keys the cells view calls, never copied; the bullet the
//! cells lead a state with is the kit's LED here, and the arrows at a
//! button's end are left to the button.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::{Align, Id, Layout, Pos2, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

use super::super::EguiShown;
use super::*;
use crate::gui::window::studio::{
    self, BOX, CELL_PAD, COL_GAP, Col, Dialog, FIELD_ROW_H, Glyph, HIT, Icon, Pal, Para, Table, add_card, bare, body,
    cadence, cell, check_line, field, field_at, foot, foot_line, gate, icon_button, ink, label_at, line, name_cell,
    reveal, room_rects, small_caps_width, state_line, strong, text, tick_box, tick_row, width_of,
};

/// The canvas's gap between a form's blocks.
const FORM_GAP: f32 = 20.0;
/// The state line's height.
const STATE_H: f32 = 20.0;
/// The table's trail: the remove column, then the slot egui's scroll bar
/// floats in at the table's edge.
const REMOVE_W: f32 = 36.0;
const BAR_SLOT: f32 = 14.0;
const TRAIL: f32 = REMOVE_W + BAR_SLOT;
/// The USER column: the canvas's 180 points where the room has them, and
/// half of what the flags leave where it has not, never under 72.
const USER_W: f32 = 180.0;
const USER_MIN: f32 = 72.0;
/// The flag columns on the canvas, in the table's order: ADMIN, then
/// FOLDERS, UPLOAD, AUDIO and MODIFY.
const FLAG_W: [f32; 5] = [60.0, 72.0, 64.0, 56.0, 64.0];
/// The modals' widths: the canvas's add form, and the grant and the
/// password.
const FORM_W: f32 = 640.0;
const SMALL_W: f32 = 480.0;
/// A row of the grant's list.
const GRANT_ROW_H: f32 = 32.0;

/// One pass of the room over the whole of `ui`: the room's area less its
/// first row, which is the host's pick banner's. `composition` is the
/// input method's uncommitted text, for the focused field alone. Returns
/// the focused field's caret, if a field has the keyboard, and what a
/// click asked to end (nothing here can).
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    let pal = Pal::now();
    let full = ui.max_rect();
    let (body_rect, note_rect) = room_rects(full);

    let mut acts = Vec::new();
    let mut base = ui.new_child(UiBuilder::new().id_salt("users-base").max_rect(body_rect).layout(Layout::top_down(Align::Min)));
    base.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
    draw_base(&mut base, room, &pal, &mut acts);
    note(ui, note_rect, room, &pal);

    let mut caret = None;
    if !matches!(room.modal, Modal::None) {
        // A modal makes the room beneath inert: what was clicked under it
        // this pass is dropped, as the cells view drops what it registered.
        acts.clear();
        caret = draw_modal(ui, room, composition, full, &pal, &mut acts);
    }
    let mut outcome = None;
    for act in acts {
        outcome = outcome.or(room.act(act));
    }
    // A click that closed the modal, or took its focus off the fields,
    // leaves no field with the keyboard.
    if !field_has_keys(room) {
        caret = None;
    }
    EguiShown { caret, outcome }
}

/// Whether one of the room's text fields has its keyboard.
fn field_has_keys(room: &Room) -> bool {
    match &room.modal {
        Modal::Add(f) => matches!(f.focused(), AField::Username | AField::Password),
        Modal::Password(_) => true,
        _ => false,
    }
}

// ── The room ──────────────────────────────────────────────────────────────

/// The state line, the add card, the table — and on a server with no
/// users, the sentences about public mode under its headings.
fn draw_base(ui: &mut Ui, room: &Room, pal: &Pal, acts: &mut Vec<Act>) {
    state(ui, room, pal);
    ui.add_space(studio::ROOM_GAP);
    let label = if room.loaded && room.users.is_empty() { t!("usr.add_first_card") } else { t!("usr.add_card") };
    if add_card(ui, Id::new("users-add-card"), &label, Glyph::User, pal).clicked() {
        acts.push(Act::Add);
    }
    ui.add_space(studio::ROOM_GAP);
    table(ui, room, pal, acts);
}

/// Public mode in gold, else the count — or, once the room signed itself
/// in, who it is signed in as — after the state's LED, and the reload's
/// cadence dim at the right. Blank until the first load answers.
fn state(ui: &mut Ui, room: &Room, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, STATE_H), Sense::hover());
    if !room.loaded {
        return;
    }
    let public = room.users.is_empty();
    let (lamp, head, head_color, tail, tail_color) = if public {
        (pal.gold, t!("usr.state_public"), pal.gold, t!("usr.state_public_detail").to_string(), pal.gold)
    } else if let Some(user) = &room.signed_in_as {
        let user = printable(user, NAME_MAX);
        let (tail, color) = match &room.session_unsaved {
            None => (t!("usr.state_signed_in_detail", user = user).to_string(), pal.text),
            Some(err) => (t!("usr.state_signed_in_unsaved", user = user, err = err.clone()).to_string(), pal.gold),
        };
        (pal.ok, t!("usr.state_signed_in"), pal.ok, tail, color)
    } else {
        let (n, admins) = (room.users.len(), room.admins());
        let head = if n == 1 { t!("usr.state_users_one") } else { t!("usr.state_users", n = n) };
        let tail = if admins == 1 { t!("usr.state_admins_one") } else { t!("usr.state_admins", n = admins) };
        (pal.ok, head, pal.text, tail.to_string(), pal.text)
    };
    let parts = [(bare(&head).to_string(), strong(14.0), head_color), (tail, body(14.0), tail_color)];
    let polls = if public { Vec::new() } else { cadence(&t!("usr.polls"), pal) };
    state_line(ui, row, lamp, &parts, &polls);
}

/// The table's columns across `w`: USER, ADMIN, the libraries taking what
/// is left, then the other four flags. A flag is as wide as its heading
/// (never narrower than its box's target), as the cells size them, and
/// takes the canvas's air round it only once USER and LIBRARIES each have
/// the canvas's 180 points, so a large room is the board's and the GUI's
/// default window still has room for the names.
fn columns(ui: &Ui, w: f32) -> Vec<Col> {
    let flags = [Flag::Admin, Flag::Mkdir, Flag::Upload, Flag::Audio, Flag::Modify];
    let least = flags.map(|flag| small_caps_width(ui, &flag.header()).ceil().max(HIT));
    let board: Vec<f32> = least.iter().zip(FLAG_W).map(|(l, b)| l.max(b)).collect();
    let shared = w - 2.0 * CELL_PAD - TRAIL - 6.0 * COL_GAP;
    let air = board.iter().sum::<f32>() - least.iter().sum::<f32>();
    let spare = shared - least.iter().sum::<f32>() - 2.0 * USER_W;
    let share = if air > 0.0 { (spare / air).clamp(0.0, 1.0) } else { 1.0 };
    let flag_w: Vec<f32> = least.iter().zip(&board).map(|(l, b)| (l + (b - l) * share).round()).collect();
    let rest = shared - flag_w.iter().sum::<f32>();
    let user = (rest / 2.0).clamp(USER_MIN, USER_W);
    let mut cols = vec![
        Col::left(t!("usr.col_user"), user),
        Col::center(flags[0].header(), flag_w[0]),
        Col::left(t!("usr.col_libraries"), 0.0),
    ];
    cols.extend(flags.iter().zip(&flag_w).skip(1).map(|(f, w)| Col::center(f.header(), *w)));
    cols
}

/// The users, one row each in the server's order, the keyboard's cursor
/// ringed; a row's five flags as tick boxes, and its remove in the trail,
/// outside the row's target as the cells' remove column is. A click on a
/// row selects it, its boxes included, as in the cells; the cross opens
/// the remove gate. With no users the headings stand over nothing, as the
/// cells draw them in every state; once loaded, the empty list says so on
/// the first row's line as every room's does, and public mode says why
/// under it.
fn table(ui: &mut Ui, room: &Room, pal: &Pal, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    let cols = columns(ui, w);
    let rows: Vec<(&String, &AdminUser)> = room.users.iter().collect();
    let flags = [(1, Flag::Admin), (3, Flag::Mkdir), (4, Flag::Upload), (5, Flag::Audio), (6, Flag::Modify)];
    let empty = if room.loaded { t!("usr.empty_users") } else { "".into() };
    let table = Table::new(Id::new("users-table"), &cols, rows.len()).cursor(room.sel).trail(TRAIL).empty(&empty);
    let shown = table.show(ui, pal, |ui, i, row| {
        let (name, u) = rows[i];
        name_cell(ui, row, 0, &printable(name, NAME_MAX), pal);
        let (libs, own) = if u.vpaths.is_empty() { ("—".to_string(), pal.dim) } else { (u.vpaths.join(" · "), pal.soft) };
        cell(ui, row, 2, &libs, body(13.5), ink(row, own, pal), false);
        for (at, flag) in flags {
            let mark = Rect::from_center_size(row.cell(at).center(), Vec2::splat(BOX));
            tick_box(ui, mark, flag.get(u), false, pal.panel, pal);
        }
        let at = pos2(row.trail.min.x + REMOVE_W / 2.0, row.rect.center().y);
        if icon_button(ui, Id::new("users-remove").with(i), at, Icon::Remove, pal.dim, pal).clicked() {
            acts.push(Act::Remove(name.clone()));
        }
    });
    if let Some(i) = shown.row {
        acts.push(Act::Select(i));
    }
    if rows.is_empty() && room.loaded {
        public(ui, pal);
    }
}

/// Public mode's sentences under the empty table's line: the two warnings
/// in gold, and what the player must do as well, since the session the
/// room makes is its own. Where the room is too short for them, the lines
/// with no room left are left out, as in the cells.
fn public(ui: &mut Ui, pal: &Pal) {
    // The cells' rhythm (two rows from the empty line to the warnings, one
    // to the last sentence) tight enough that all of it fits the GUI's
    // default window, as it does in cells.
    let floor = ui.clip_rect().max.y;
    let said = |ui: &mut Ui, key: &str, color| Para::new(&t!(key), body(14.0), color).floor(floor).show(ui);
    ui.add_space(2.0);
    said(ui, "usr.public_1", pal.gold);
    ui.add_space(2.0);
    said(ui, "usr.public_2", pal.gold);
    ui.add_space(10.0);
    said(ui, "usr.public_3_hosted", pal.dim);
}

/// The room's foot: what it is busy with, its note, or — with a row under
/// the cursor and no modal up — what that user sees, as the cells word it.
fn note(ui: &Ui, rect: Rect, room: &Room, pal: &Pal) {
    let story = match room.selected_name() {
        Some(name) if matches!(room.modal, Modal::None) => room.users.get(&name).map(|u| {
            let user = printable(&name, NAME_MAX);
            let text = if u.vpaths.is_empty() {
                t!("usr.note_sees_none", user = user).to_string()
            } else {
                t!("usr.note_sees", user = user, libs = u.vpaths.join(" · ")).to_string()
            };
            vec![(format!("{text}{}", t!("usr.note_flags")), body(13.0), pal.dim)]
        }),
        _ => None,
    };
    foot_line(ui, rect, room.busy.as_deref(), room.note.as_ref(), story.as_deref().unwrap_or(&[]), pal);
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The room's modal over `over`, the room's whole rect. Returns the caret
/// of its focused field, if it has one.
fn draw_modal(ui: &Ui, room: &Room, composition: &str, over: Rect, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    match &room.modal {
        Modal::None => None,
        Modal::Add(f) => add_form(ui, f, composition, over, pal, acts),
        Modal::Libraries(d) => {
            grant(ui, d, over, pal, acts);
            None
        }
        Modal::Password(d) => password(ui, d, composition, over, pal, acts),
        Modal::Remove(name) => {
            remove_gate(ui, room, name, over, pal, acts);
            None
        }
    }
}

/// The kit's field for one of the room's inputs: its value and caret, the
/// keyboard and the composition when `on`, masked where the cells mask.
fn entry<'a>(input: &'a Input, on: bool, composition: &'a str, masked: bool) -> studio::Input<'a> {
    let field = studio::Input::of(input).focused(on, composition);
    if masked { field.masked() } else { field }
}

/// The add form: the two fields side by side, the libraries in one row of
/// ticks (those with no room left off, as the cells leave them), the four
/// permissions, the name's note and the error, and the one button. The
/// control with the keyboard wears the keyboard's look, and is brought
/// into view on the pass the keyboard moved to it.
fn add_form(ui: &Ui, f: &AddForm, composition: &str, over: Rect, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    let fields = f.fields();
    let index_of = |field: AField| fields.iter().position(|x| *x == field).unwrap_or(0);
    let focused = f.focused();
    let id = Id::new("users-add-form");
    let shown = Dialog::new(id, over, FORM_W, &t!("usr.form_title")).show(ui, pal, |ui| {
        let mut caret = None;
        let w = ui.available_width();

        let half = ((w - 24.0) / 2.0).max(0.0);
        let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_ROW_H), Sense::hover());
        let texts = [
            (AField::Username, t!("usr.field_username"), &f.username, false),
            (AField::Password, t!("usr.field_password"), &f.password, true),
        ];
        for (k, (which, label, value, masked)) in texts.into_iter().enumerate() {
            let slot = Rect::from_min_size(pos2(row.min.x + k as f32 * (half + 24.0), row.min.y), vec2(half, row.height()));
            let on = focused == which;
            let typed = field_at(ui, Id::new("users-add-field").with(k), slot, &label, &entry(value, on, composition, masked), pal);
            if typed.response.clicked() {
                acts.push(Act::FormFocus(index_of(which)));
            }
            if on {
                caret = typed.caret;
                // The label with its box: the field, not its box alone.
                reveal(ui, id, f.focus, slot);
            }
        }

        ui.add_space(FORM_GAP);
        let (head, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
        let hint = text(ui, head.right_center(), &t!("usr.form_libraries_hint"), body(13.0), pal.dim, w * 0.6, egui::Align2::RIGHT_CENTER);
        label_at(ui, head.left_center(), &t!("usr.form_libraries"), (hint.min.x - 16.0 - head.min.x).max(1.0), pal);
        ui.add_space(8.0);
        let (line_rect, _) = ui.allocate_exact_size(vec2(w, HIT), Sense::hover());
        if f.libraries.is_empty() {
            text(ui, line_rect.left_center(), &t!("usr.form_no_libraries"), body(13.5), pal.dim, w, egui::Align2::LEFT_CENTER);
        }
        let mut x = line_rect.min.x - 8.0;
        for (i, (name, on)) in f.libraries.iter().enumerate() {
            // The first is drawn however long, cut at the row's end; a
            // later one with no room left is left off, as in the cells.
            let label = printable(name, NAME_MAX);
            let which = AField::Lib(i);
            let font = if focused == which { strong(14.0) } else { body(14.0) };
            let want = 8.0 + BOX + 10.0 + width_of(ui, &label, font) + 8.0;
            if x + want > line_rect.max.x + 8.0 && i > 0 {
                break;
            }
            let rect = Rect::from_min_max(pos2(x, line_rect.min.y), pos2((x + want).min(line_rect.max.x + 8.0), line_rect.max.y));
            if tick_row(ui, Id::new("users-add-lib").with(i), rect, *on, &label, focused == which, pal).clicked() {
                acts.push(Act::FormToggle(index_of(which)));
            }
            if focused == which {
                reveal(ui, id, f.focus, rect);
            }
            x = rect.max.x + 4.0;
        }

        ui.add_space(FORM_GAP);
        studio::label(ui, &t!("usr.form_permissions"), pal);
        ui.add_space(8.0);
        let perms = [
            (AField::Admin, t!("usr.perm_admin").to_string(), t!("usr.perm_admin_desc").to_string()),
            (AField::Mkdir, t!("usr.perm_mkdir").to_string(), String::new()),
            (AField::Upload, t!("usr.perm_upload").to_string(), String::new()),
            (AField::Audio, t!("usr.perm_audio").to_string(), t!("usr.perm_audio_desc").to_string()),
        ];
        for (k, (which, label, desc)) in perms.into_iter().enumerate() {
            if k > 0 {
                ui.add_space(2.0);
            }
            let response = check_line(ui, Id::new("users-add-perm").with(k), f.checked(which), &label, &desc, focused == which, pal);
            if response.clicked() {
                acts.push(Act::FormToggle(index_of(which)));
            }
            if focused == which {
                reveal(ui, id, f.focus, response.rect);
            }
        }

        ui.add_space(FORM_GAP);
        line(ui, 18.0, &t!("usr.form_permanent"), body(13.0), pal.dim);
        ui.add_space(4.0);
        line(ui, 18.0, f.error.as_deref().unwrap_or(""), body(13.0), pal.gold);
        ui.add_space(FORM_GAP);
        let add = t!("usr.form_add");
        if foot(ui, Id::new("users-add-submit"), &[], &[(add.as_ref(), true)], None, pal).is_some() {
            acts.push(Act::FormSubmit);
        }
        caret
    });
    if shown.closed {
        acts.push(Act::FormCancel);
    }
    shown.inner
}

/// The library grant: a row a library, the kit's tick row as the add
/// form's libraries, ticked where the user holds it, the keyboard's cursor
/// in the keyboard's look and brought into view when it moves, a row under
/// the pointer raised; the two notes; Save.
fn grant(ui: &Ui, d: &LibsDraft, over: Rect, pal: &Pal, acts: &mut Vec<Act>) {
    let title = t!("usr.libs_title", user = printable(&d.username, NAME_MAX));
    let id = Id::new("users-libraries");
    let shown = Dialog::new(id, over, SMALL_W, &title).show(ui, pal, |ui| {
        let w = ui.available_width();
        if d.libraries.is_empty() {
            line(ui, GRANT_ROW_H, &t!("usr.form_no_libraries"), body(13.5), pal.dim);
        }
        for (i, (name, on)) in d.libraries.iter().enumerate() {
            // Every row takes its room, but only the rows in view are drawn:
            // a long list costs the pass its height, not each row's words.
            let (_, rect) = ui.allocate_space(vec2(w, GRANT_ROW_H));
            let cursor = i == d.cursor;
            if cursor {
                reveal(ui, id, i, rect);
            }
            if !ui.is_rect_visible(rect) {
                continue;
            }
            if tick_row(ui, Id::new("users-libs-row").with(i), rect, *on, &printable(name, NAME_MAX), cursor, pal).clicked() {
                acts.push(Act::LibsRow(i));
            }
        }
        ui.add_space(16.0);
        Para::new(&t!("usr.libs_note_1"), body(13.0), pal.dim).show(ui);
        ui.add_space(2.0);
        Para::new(&t!("usr.libs_note_2"), body(13.0), pal.dim).show(ui);
        ui.add_space(FORM_GAP);
        let save = t!("usr.libs_save");
        if foot(ui, Id::new("users-libs-save"), &[], &[(save.as_ref(), true)], None, pal).is_some() {
            acts.push(Act::LibsSubmit);
        }
    });
    if shown.closed {
        acts.push(Act::LibsCancel);
    }
}

/// A new password, typed twice, both masked; the error at the foot's left
/// and the button at its right, lit only while the two agree, as the
/// cells' waits, though a click is the submit either way.
fn password(ui: &Ui, d: &PwDraft, composition: &str, over: Rect, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    let title = t!("usr.pw_title", user = printable(&d.username, NAME_MAX));
    let id = Id::new("users-password");
    let shown = Dialog::new(id, over, SMALL_W, &title).show(ui, pal, |ui| {
        let mut caret = None;
        let pairs = [(t!("usr.field_password"), &d.password), (t!("usr.field_again"), &d.again)];
        for (k, (label, value)) in pairs.into_iter().enumerate() {
            if k > 0 {
                ui.add_space(16.0);
            }
            let on = d.focus == k;
            let top = ui.cursor().min.y;
            let typed = field(ui, Id::new("users-pw-field").with(k), &label, &entry(value, on, composition, true), pal);
            if typed.response.clicked() {
                acts.push(Act::PwFocus(k));
            }
            if on {
                caret = typed.caret;
                // The label with its box: the field, not its box alone.
                reveal(ui, id, k, typed.response.rect.with_min_y(top));
            }
        }
        ui.add_space(FORM_GAP);
        let set = t!("usr.pw_set");
        if foot(ui, Id::new("users-pw-set"), &[], &[(set.as_ref(), d.ready())], d.error.as_deref(), pal).is_some() {
            acts.push(Act::PwSubmit);
        }
        caret
    });
    if shown.closed {
        acts.push(Act::PwCancel);
    }
    shown.inner
}

/// The remove gate, the kit's: what goes with the user in gold, what stays
/// in green, the last admin's warning where it applies; Keep lit, Remove
/// not.
fn remove_gate(ui: &Ui, room: &Room, name: &str, over: Rect, pal: &Pal, acts: &mut Vec<Act>) {
    let user = printable(name, NAME_MAX);
    let title = t!("usr.remove_title", user = user.clone());
    let mut sentences = vec![Para::new(&t!("usr.remove_1"), body(14.0), pal.gold), Para::new(&t!("usr.remove_2"), body(14.0), pal.ok)];
    if room.last_admin(name) {
        sentences.push(Para::new(&t!("usr.remove_last_admin", user = user), strong(14.0), pal.gold));
    }
    let (keep, go) = (t!("usr.remove_keep"), t!("usr.remove_confirm"));
    match gate(ui, Id::new("users-remove-gate"), over, &title, sentences, [&keep, &go], pal) {
        Some(true) => acts.push(Act::RemoveConfirm),
        Some(false) => acts.push(Act::RemoveCancel),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{Event, FullOutput, Modifiers, PointerButton};

    use super::super::tests::{english, libs, loaded, press, room, type_text};
    use super::*;

    /// The canvas's side margin round the room.
    const PAD_X: f32 = 24.0;

    /// The room's region in the GUI's default 100×30 window (83×22 cells
    /// under the pick banner's row), and in a large one, in points.
    const SMALL: Vec2 = vec2(664.0, 352.0);
    const LARGE: Vec2 = vec2(1232.0, 760.0);

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        ctx
    }

    /// What one pass gave back.
    struct Pass {
        out: FullOutput,
        caret: Option<Pos2>,
        outcome: Option<Outcome>,
    }

    /// One pass of the room over a region of `size`, with `events` and the
    /// input method composing `composition`, the host's way: the kit's
    /// ground first, then the room.
    fn pass_with(ctx: &egui::Context, room: &mut Room, size: Vec2, events: Vec<Event>, composition: &str) -> Pass {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
        let (mut caret, mut outcome) = (None, None);
        let mut out = ctx.run_ui(input, |ui| {
            let _ = studio::begin(ui);
            let shown = show(ui, room, composition);
            caret = shown.caret;
            outcome = outcome.or(shown.outcome);
        });
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        Pass { out, caret, outcome }
    }

    fn pass(ctx: &egui::Context, room: &mut Room, size: Vec2) -> Pass {
        pass_with(ctx, room, size, Vec::new(), "")
    }

    /// A press and a release at `at`, a pass each after the move, as hands
    /// would make them. Returns the release's pass.
    fn click(ctx: &egui::Context, room: &mut Room, size: Vec2, at: Pos2) -> Pass {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
        let _ = pass_with(ctx, room, size, vec![Event::PointerMoved(at)], "");
        let _ = pass_with(ctx, room, size, vec![button(true)], "");
        pass_with(ctx, room, size, vec![button(false)], "")
    }

    /// A click on the widget `id` drew on the last pass.
    fn click_on(ctx: &egui::Context, room: &mut Room, size: Vec2, id: Id) -> Pass {
        let at = ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was not drawn")).rect.center();
        click(ctx, room, size, at)
    }

    /// Every text the pass drew, as laid out.
    fn drawn(out: &FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Where the pass drew `s`.
    fn text_at(out: &FullOutput, s: &str) -> Rect {
        out.shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(text) if text.galley.text() == s => Some(text.visual_bounding_rect()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{s:?} not drawn: {:?}", drawn(out)))
    }

    /// Two passes, as egui settles a modal's height on the first.
    fn settle(ctx: &egui::Context, room: &mut Room, size: Vec2) -> Pass {
        let _ = pass(ctx, room, size);
        pass(ctx, room, size)
    }

    #[test]
    fn the_table_draws_the_cells_words_at_the_default_window_and_a_large_one() {
        let _en = english();
        for size in [SMALL, LARGE] {
            let ctx = context();
            let mut r = loaded();
            let words = drawn(&settle(&ctx, &mut r, size).out);
            let has = |s: &str| words.iter().any(|w| w == s);
            assert!(has(&format!("3 users{}", t!("usr.state_admins_one"))), "the count, its bullet an LED, in one run: {words:?}");
            assert!(has(&t!("usr.polls")), "{words:?}");
            assert!(has("Add a user"), "the card's words, bare of their arrow: {words:?}");
            for heading in ["USER", "ADMIN", "LIBRARIES", "FOLDERS", "UPLOAD", "AUDIO", "MODIFY"] {
                assert!(has(heading), "{heading} at {size:?}: {words:?}");
            }
            for name in ["anna", "ben", "guest", "music · field-recordings", "music"] {
                assert!(has(name), "{name} at {size:?}: {words:?}");
            }
            assert!(!words.iter().any(|w| w.contains('•') || w.contains('▸')), "no cell glyphs: {words:?}");
            // The cursor's row has its own line at the foot.
            press(&mut r, KeyCode::Down);
            press(&mut r, KeyCode::Down);
            let words = drawn(&pass(&ctx, &mut r, size).out);
            assert!(words.iter().any(|w| w == "ben sees music · field-recordings · 1–5 flip a flag, saved at once"), "{words:?}");
        }
    }

    #[test]
    fn public_mode_and_the_first_load_draw_their_own_words() {
        let _en = english();
        let ctx = context();
        let mut r = room();
        r.queued = None;
        r.apply(Done::Loaded(Ok((BTreeMap::new(), libs()))));
        let words = drawn(&settle(&ctx, &mut r, LARGE).out);
        let has = |s: &str| words.iter().any(|w| w == s);
        assert!(has(&format!("public{}", t!("usr.state_public_detail"))), "{words:?}");
        assert!(!has(&t!("usr.polls")), "public mode names no cadence: {words:?}");
        assert!(has("Add the first user") && has(&t!("usr.empty_users")), "{words:?}");
        for key in ["usr.public_1", "usr.public_2", "usr.public_3_hosted"] {
            assert!(has(&t!(key)), "{key}: {words:?}");
        }
        assert!(has("USER") && has("MODIFY"), "the headings over no rows: {words:?}");

        let mut r = start(Client::new("http://home.mstream.example:3000").expect("client"));
        let words = drawn(&settle(&ctx, &mut r, SMALL).out);
        assert!(words.iter().any(|w| *w == t!("usr.busy_loading")), "the busy line: {words:?}");
        assert!(!words.iter().any(|w| *w == t!("usr.empty_users") || w.ends_with("users")), "no state yet: {words:?}");
    }

    #[test]
    fn a_click_on_a_row_or_its_boxes_selects_it_and_the_cross_opens_the_gate() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        let out = settle(&ctx, &mut r, SMALL).out;
        let guest = text_at(&out, "guest").center();
        let shown = click(&ctx, &mut r, SMALL, guest);
        assert_eq!(r.sel, Some(2), "a click on a row selects it");
        assert!(shown.outcome.is_none() && shown.caret.is_none());
        // ben's UPLOAD box: the cells' row target is over its boxes, so a
        // click there selects the row and flips nothing.
        let out = pass(&ctx, &mut r, SMALL).out;
        let at = pos2(text_at(&out, "UPLOAD").center().x, text_at(&out, "ben").center().y);
        let _ = click(&ctx, &mut r, SMALL, at);
        assert_eq!(r.sel, Some(1));
        assert!(r.queued.is_none(), "nothing sent");
        // guest's trail right of its cross: outside the row's target, as the
        // cells' remove column is, so no hand and no selection.
        let cross = ctx.read_response(Id::new("users-remove").with(2)).expect("guest's cross").rect;
        let shown = click(&ctx, &mut r, SMALL, pos2(cross.max.x + 9.0, cross.center().y));
        assert_ne!(shown.out.platform_output.cursor_icon, egui::CursorIcon::PointingHand, "no row's hand in the trail");
        assert_eq!(r.sel, Some(1), "the trail selects nothing");
        assert!(matches!(r.modal, Modal::None), "nor removes");
        let _ = click_on(&ctx, &mut r, SMALL, Id::new("users-remove").with(0));
        assert!(matches!(&r.modal, Modal::Remove(name) if name == "anna"), "{:?}", r.modal);
        assert_eq!(r.sel, Some(1), "the cross moves no cursor");
    }

    #[test]
    fn the_add_form_opens_from_its_card_and_its_controls_are_the_cells_acts() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        let out = settle(&ctx, &mut r, LARGE).out;
        let guest = text_at(&out, "guest").center();
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-add-card"));
        let Modal::Add(_) = &r.modal else { panic!("the card opens the form: {:?}", r.modal) };
        let words = drawn(&settle(&ctx, &mut r, LARGE).out);
        // The button's words in the desk's capitals, bare of their arrow.
        for s in ["Add a user", "USERNAME", "PASSWORD", "LIBRARIES", "PERMISSIONS", "ADD USER", "music", "audiobooks"] {
            assert!(words.iter().any(|w| w == s), "{s}: {words:?}");
        }
        assert!(words.iter().any(|w| *w == t!("usr.form_permanent")), "{words:?}");

        // The room beneath is inert.
        let _ = click(&ctx, &mut r, LARGE, pos2(PAD_X + 20.0, guest.y));
        assert_eq!(r.sel, None, "a click beside the window selects nothing");

        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-add-field").with(1));
        let Modal::Add(f) = &r.modal else { panic!("the form") };
        assert_eq!(f.focused(), AField::Password, "a click on a field focuses it");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-add-perm").with(3));
        let Modal::Add(f) = &r.modal else { panic!("the form") };
        assert!(f.audio && f.focused() == AField::Audio, "server audio ticked and focused");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-add-lib").with(1));
        let Modal::Add(f) = &r.modal else { panic!("the form") };
        assert!(!f.libraries[1].1 && f.focused() == AField::Lib(1), "audiobooks unticked and focused");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-add-submit").with(0usize));
        let Modal::Add(f) = &r.modal else { panic!("the form stays") };
        assert_eq!(f.error.as_deref(), Some("a username is needed"), "the button is the form's Enter");
        let words = drawn(&pass(&ctx, &mut r, LARGE).out);
        assert!(words.iter().any(|w| w == "a username is needed"), "the error drawn: {words:?}");
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-add-form").with("close"));
        assert!(matches!(r.modal, Modal::None), "the close is the form's cancel");
    }

    #[test]
    fn a_focused_field_returns_its_caret_and_draws_the_composition_and_none_without_one() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        assert!(settle(&ctx, &mut r, LARGE).caret.is_none(), "no field at the room's base");
        press(&mut r, KeyCode::Char('a'));
        type_text(&mut r, "mira");
        let _ = settle(&ctx, &mut r, LARGE);
        let shown = pass_with(&ctx, &mut r, LARGE, Vec::new(), "かな");
        let caret = shown.caret.expect("the username has the keyboard");
        let field = ctx.read_response(Id::new("users-add-field").with(0)).expect("the username's box").rect;
        assert!(field.contains(caret - vec2(0.0, 0.5)), "the caret's foot in its box: {caret:?} {field:?}");
        assert!(drawn(&shown.out).iter().any(|w| w == "miraかな"), "the composition before the caret: {:?}", drawn(&shown.out));
        // On to the password, then to the first library's tick.
        press(&mut r, KeyCode::Tab);
        let shown = pass_with(&ctx, &mut r, LARGE, Vec::new(), "x");
        let box_rect = ctx.read_response(Id::new("users-add-field").with(1)).expect("the password's box").rect;
        assert!(shown.caret.is_some_and(|c| box_rect.contains(c - vec2(0.0, 0.5))), "the password's caret");
        assert!(drawn(&shown.out).iter().any(|w| w == "•"), "a masked field's composition is masked: {:?}", drawn(&shown.out));
        assert!(!drawn(&shown.out).iter().any(|w| w.contains('x')), "and never drawn in clear");
        press(&mut r, KeyCode::Tab);
        assert!(pass(&ctx, &mut r, LARGE).caret.is_none(), "a tick has the keyboard, not a field");
        press(&mut r, KeyCode::Esc);
        r.act(Act::Password("ben".into()));
        assert!(settle(&ctx, &mut r, LARGE).caret.is_some(), "the password modal's field has it");
    }

    #[test]
    fn the_libraries_modal_ticks_a_row_saves_and_closes() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        r.act(Act::Libraries("ben".into()));
        let words = drawn(&settle(&ctx, &mut r, LARGE).out);
        for s in ["Libraries for ben", "music", "audiobooks", "field-recordings", "SAVE"] {
            assert!(words.iter().any(|w| w == s), "{s}: {words:?}");
        }
        assert!(words.iter().any(|w| *w == t!("usr.libs_note_1")) && words.iter().any(|w| *w == t!("usr.libs_note_2")));
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-libs-row").with(1));
        let Modal::Libraries(d) = &r.modal else { panic!("the grant") };
        assert!(d.libraries[1].1 && d.cursor == 1, "audiobooks ticked under the cursor");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-libs-save").with(0usize));
        assert_eq!(r.queued, Some(Op::Vpaths { username: "ben".into(), vpaths: libs() }), "the whole list");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-libraries").with("close"));
        assert!(matches!(r.modal, Modal::None), "the close is the grant's cancel");
    }

    #[test]
    fn the_password_modal_focuses_by_click_and_its_button_is_the_submit() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        r.act(Act::Password("ben".into()));
        let words = drawn(&settle(&ctx, &mut r, LARGE).out);
        for s in ["New password for ben", "PASSWORD", "AGAIN", "SET PASSWORD"] {
            assert!(words.iter().any(|w| w == s), "{s}: {words:?}");
        }
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-pw-field").with(1));
        let Modal::Password(d) = &r.modal else { panic!("the password modal") };
        assert_eq!(d.focus, 1, "a click on AGAIN focuses it");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-pw-set").with(0usize));
        let Modal::Password(d) = &r.modal else { panic!("the password modal") };
        assert_eq!((d.error.as_deref(), d.focus), (Some("a password is needed"), 0), "refused as on Enter");
        let words = drawn(&pass(&ctx, &mut r, LARGE).out);
        assert!(words.iter().any(|w| w == "a password is needed"), "{words:?}");
        type_text(&mut r, "pw");
        press(&mut r, KeyCode::Tab);
        type_text(&mut r, "pw");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-pw-set").with(0usize));
        assert_eq!(r.queued, Some(Op::Password { username: "ben".into(), password: "pw".into() }));
        r.queued = None;
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-password").with("close"));
        assert!(matches!(r.modal, Modal::None), "the close is the modal's cancel");
    }

    #[test]
    fn the_remove_gate_is_answered_keep_or_remove_with_no_close() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        r.act(Act::Remove("anna".into()));
        let words = drawn(&settle(&ctx, &mut r, LARGE).out);
        for s in ["Remove anna?", "KEEP", "REMOVE"] {
            assert!(words.iter().any(|w| w == s), "{s}: {words:?}");
        }
        for key in ["usr.remove_1", "usr.remove_2"] {
            assert!(words.iter().any(|w| *w == t!(key)), "{key}: {words:?}");
        }
        assert!(words.iter().any(|w| *w == t!("usr.remove_last_admin", user = "anna")), "the only admin: {words:?}");
        let gate = Id::new("users-remove-gate");
        assert!(ctx.read_response(gate.with("close")).is_none(), "a gate has no close, as the cells draw it");
        let _ = click_on(&ctx, &mut r, LARGE, gate.with("foot").with(0usize));
        assert!(matches!(r.modal, Modal::None) && r.queued.is_none(), "Keep keeps");

        r.act(Act::Remove("ben".into()));
        let words = drawn(&settle(&ctx, &mut r, LARGE).out);
        assert!(!words.iter().any(|w| w.contains("only admin")), "{words:?}");
        press(&mut r, KeyCode::Esc);
        assert!(matches!(r.modal, Modal::None) && r.queued.is_none(), "Esc keeps");

        r.act(Act::Remove("ben".into()));
        let _ = settle(&ctx, &mut r, LARGE);
        let shown = click_on(&ctx, &mut r, LARGE, gate.with("foot").with(1usize));

        assert_eq!(r.queued, Some(Op::Remove("ben".into())));
        assert!(matches!(r.modal, Modal::None));
        assert!(shown.outcome.is_none(), "no click ends the room");
        let words = drawn(&pass(&ctx, &mut r, LARGE).out);
        assert!(words.iter().any(|w| w == "removing ben…"), "the busy line: {words:?}");
    }

    /// The columns `columns` lays across a column `w` points wide, and the
    /// flags' headings' widths, measured in a pass.
    fn widths(ctx: &egui::Context, w: f32) -> (Vec<f32>, Vec<f32>) {
        let mut got = (Vec::new(), Vec::new());
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, LARGE)), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            got.0 = columns(ui, w).iter().map(|c| c.w).collect();
            got.1 = [Flag::Admin, Flag::Mkdir, Flag::Upload, Flag::Audio, Flag::Modify].map(|f| small_caps_width(ui, &f.header())).to_vec();
        });
        out.textures_delta.clear();
        got
    }

    #[test]
    fn the_flags_take_the_boards_widths_in_a_large_room_and_their_headings_in_the_default_one() {
        let _en = english();
        let ctx = context();
        // The canvas's room: its grid, the libraries taking what is left.
        let (large, _) = widths(&ctx, LARGE.x - 2.0 * PAD_X);
        assert_eq!(large, [USER_W, 60.0, 0.0, 72.0, 64.0, 56.0, 64.0], "the board's grid");
        // The GUI's default window: each flag as wide as its heading, so
        // the names and the libraries keep what the flags leave.
        let w = SMALL.x - 2.0 * PAD_X;
        let (small, headings) = widths(&ctx, w);
        let flags = [small[1], small[3], small[4], small[5], small[6]];
        for (k, (got, heading)) in flags.iter().zip(&headings).enumerate() {
            assert!(*got >= heading.ceil() && *got >= HIT && *got <= FLAG_W[k].max(heading.ceil()), "flag {k}: {got} for a {heading} heading");
        }
        let libraries = w - 2.0 * CELL_PAD - TRAIL - 6.0 * COL_GAP - small.iter().sum::<f32>();
        assert!(small[0] >= USER_MIN && libraries >= small[0] - 1.0, "USER {} and LIBRARIES {libraries} share the rest", small[0]);
    }

    /// Each text the pass drew, with how many of its lines are in view
    /// under its clip and how many it has; and whether a line is cut
    /// through, its top in view and its foot not.
    fn lines_in_view(out: &FullOutput) -> Vec<(String, usize, usize, bool)> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => {
                    let clip = clipped.clip_rect;
                    let rows: Vec<Rect> = text.galley.rows.iter().map(|row| row.rect().translate(text.pos.to_vec2())).collect();
                    let shown = rows.iter().filter(|r| r.max.y <= clip.max.y + 0.5).count();
                    let cut = rows.iter().any(|r| r.min.y < clip.max.y && r.max.y > clip.max.y + 0.5);
                    Some((text.galley.text().to_string(), shown, rows.len(), cut))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn public_modes_sentences_fit_the_default_window_and_a_line_with_no_room_is_left_out_not_cut() {
        let _en = english();
        // The GUI's floor, a 100×24 window: 16 rows under the pick banner's.
        const FLOOR: Vec2 = vec2(664.0, 256.0);
        let sentences = ["usr.empty_users", "usr.public_1", "usr.public_2", "usr.public_3_hosted"].map(|key| t!(key).to_string());
        for size in [SMALL, FLOOR, LARGE] {
            let ctx = context();
            let mut r = room();
            r.queued = None;
            r.apply(Done::Loaded(Ok((BTreeMap::new(), libs()))));
            let lines = lines_in_view(&settle(&ctx, &mut r, size).out);
            assert!(lines.iter().all(|(_, _, _, cut)| !cut), "a line cut through at {size:?}: {lines:?}");
            let in_view = |s: &str| lines.iter().find(|(text, ..)| text == s).map_or((0, 0), |(_, shown, all, _)| (*shown, *all));
            if size == FLOOR {
                // The cells leave the last sentence out at the floor too.
                assert_eq!(in_view(&sentences[0]), (1, 1), "the empty list's line: {lines:?}");
                assert_eq!(in_view(&sentences[3]).0, 0, "no room for the last sentence: {lines:?}");
            } else {
                for s in &sentences {
                    let (shown, all) = in_view(s);
                    assert!(all > 0 && shown == all, "every line of {s:?} at {size:?}: {lines:?}");
                }
            }
        }
    }

    #[test]
    fn a_modal_opens_with_its_keyboards_field_in_view_however_it_was_left_scrolled() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        press(&mut r, KeyCode::Char('a'));
        let _ = settle(&ctx, &mut r, SMALL);
        let field = |ctx: &egui::Context| ctx.read_response(Id::new("users-add-field").with(0)).expect("the username's box").rect;
        let top = field(&ctx).min.y;
        // The wheel takes the form to its foot, the keyboard still on USERNAME.
        let at = pos2(SMALL.x / 2.0, SMALL.y / 2.0);
        let wheel = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -2000.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::default(),
        };
        let _ = pass_with(&ctx, &mut r, SMALL, vec![Event::PointerMoved(at), wheel], "");
        for _ in 0..40 {
            let _ = pass(&ctx, &mut r, SMALL);
        }
        assert!(field(&ctx).min.y < top - 40.0, "the form scrolled: {} from {top}", field(&ctx).min.y);
        press(&mut r, KeyCode::Esc);
        let _ = pass(&ctx, &mut r, SMALL);
        press(&mut r, KeyCode::Char('a'));
        // egui eases the body there over a few passes.
        for _ in 0..40 {
            let _ = pass(&ctx, &mut r, SMALL);
        }
        assert!((field(&ctx).min.y - top).abs() < 1.0, "the form opens on its field and the field's label: {} for {top}", field(&ctx).min.y);
    }

    /// A wheel's turn of `dy` points over `at` (up the list for a positive
    /// one), and the passes egui eases the scroll over.
    fn wheel(ctx: &egui::Context, room: &mut Room, size: Vec2, at: Pos2, dy: f32) {
        let turn = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::default(),
        };
        let _ = pass_with(ctx, room, size, vec![Event::PointerMoved(at), turn], "");
        for _ in 0..40 {
            let _ = pass(ctx, room, size);
        }
    }

    #[test]
    fn a_press_the_room_took_before_a_modal_came_up_selects_nothing_at_its_release() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        let guest = text_at(&settle(&ctx, &mut r, LARGE).out, "guest").center();
        let button = |pressed| Event::PointerButton { pos: guest, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
        // egui owes the row the click it was pressed on, under the scrim or
        // not: the room beneath drops it, on the form's first pass and on a
        // later one alike.
        for passes_between in [0, 2] {
            let _ = pass_with(&ctx, &mut r, LARGE, vec![Event::PointerMoved(guest)], "");
            let _ = pass_with(&ctx, &mut r, LARGE, vec![button(true)], "");
            press(&mut r, KeyCode::Char('a'));
            for _ in 0..passes_between {
                let _ = pass(&ctx, &mut r, LARGE);
            }
            let shown = pass_with(&ctx, &mut r, LARGE, vec![button(false)], "");
            assert_eq!(r.sel, None, "a release under the form, {passes_between} passes after it came up");
            assert!(matches!(r.modal, Modal::Add(_)) && shown.outcome.is_none(), "the form stays");
            press(&mut r, KeyCode::Esc);
            let _ = settle(&ctx, &mut r, LARGE);
        }
    }

    #[test]
    fn a_long_tables_cursor_scrolls_in_when_a_key_moves_it_and_the_wheel_has_the_list_otherwise() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        for i in 0..40 {
            r.users.insert(format!("user-{i:02}"), super::super::tests::user(false, &["music"], false, false, false, false));
        }
        let shows = |out: &FullOutput, name: &str| drawn(out).iter().any(|w| w == name);
        let out = settle(&ctx, &mut r, SMALL).out;
        assert!(shows(&out, "anna") && !shows(&out, "user-39"), "the list's head, its rows in view alone: {:?}", drawn(&out));
        // Up with no cursor takes it to the last row, and the table follows.
        press(&mut r, KeyCode::Up);
        let _ = pass(&ctx, &mut r, SMALL);
        let out = pass(&ctx, &mut r, SMALL).out;
        assert!(shows(&out, "user-39") && !shows(&out, "anna"), "{:?}", drawn(&out));
        // The wheel takes the list back to its head, and the cursor, resting,
        // is left out of view: the table follows it only when it moves.
        wheel(&ctx, &mut r, SMALL, pos2(SMALL.x / 2.0, SMALL.y / 2.0), 4000.0);
        let out = pass(&ctx, &mut r, SMALL).out;
        assert_eq!(r.selected_name().as_deref(), Some("user-39"));
        assert!(shows(&out, "anna") && !shows(&out, "user-39"), "{:?}", drawn(&out));
        press(&mut r, KeyCode::Up);
        let _ = pass(&ctx, &mut r, SMALL);
        let out = pass(&ctx, &mut r, SMALL).out;
        assert!(shows(&out, "user-38") && !shows(&out, "anna"), "the key brings it back: {:?}", drawn(&out));
    }

    #[test]
    fn the_grants_long_list_draws_its_rows_in_view_alone_and_scrolls_its_cursor_in_on_a_key() {
        let _en = english();
        let ctx = context();
        let mut r = loaded();
        r.libraries = (0..60).map(|i| format!("lib-{i:02}")).collect();
        r.act(Act::Libraries("ben".into()));
        let rows = |out: &FullOutput| drawn(out).into_iter().filter(|w| w.starts_with("lib-")).collect::<Vec<_>>();
        let shown = rows(&settle(&ctx, &mut r, SMALL).out);
        assert!(shown.first().is_some_and(|w| w == "lib-00") && shown.len() < 20, "the list's head, and only what is in view: {shown:?}");
        // Up wraps the cursor to the last library, which scrolls in.
        press(&mut r, KeyCode::Up);
        for _ in 0..40 {
            let _ = pass(&ctx, &mut r, SMALL);
        }
        let shown = rows(&pass(&ctx, &mut r, SMALL).out);
        assert!(shown.iter().any(|w| w == "lib-59") && !shown.iter().any(|w| w == "lib-00"), "{shown:?}");
        let _ = click_on(&ctx, &mut r, SMALL, Id::new("users-libs-row").with(59));
        let Modal::Libraries(d) = &r.modal else { panic!("the grant") };
        assert!(d.libraries[59].1 && d.cursor == 59, "a row in view takes its click");
    }
}
