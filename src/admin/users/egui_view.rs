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

use egui::epaint::{Shape, StrokeKind};
use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Id, Layout, Pos2, Rect, Response, Sense, Stroke, Ui, UiBuilder, Vec2,
    pos2, vec2,
};

use super::super::EguiShown;
use super::*;
use crate::gui::window::studio::{
    self, Col, FIELD_H, FORM_BUTTON_H, HIT, Icon, Pal, Table, add_card, body, cell, check_line, field, form_button,
    form_button_width, icon_button, ink, led, lift, modal, note_line, ring, small_caps, small_caps_width, square,
    strong, text, width_of,
};
use crate::input::KeyModifiers;

/// The room's margin inside its region, as the canvas pads it: the sides,
/// the top and the foot.
const PAD_X: f32 = 24.0;
const PAD_TOP: f32 = 20.0;
const PAD_FOOT: f32 = 14.0;
/// The canvas's gap between the room's blocks, and between a form's.
const GAP: f32 = 18.0;
const FORM_GAP: f32 = 20.0;
/// The state line's height, and the note line's along the foot.
const STATE_H: f32 = 20.0;
const NOTE_H: f32 = 18.0;
/// The table's trail: the remove column, then the slot egui's scroll bar
/// floats in at the table's edge.
const REMOVE_W: f32 = 36.0;
const BAR_SLOT: f32 = 14.0;
const TRAIL: f32 = REMOVE_W + BAR_SLOT;
/// The headings' row and the room under it, as the kit's table lays them.
const HEAD_H: f32 = 34.0;
/// The USER column: the canvas's 180 points where the room has them, and
/// half of what the flags leave where it has not, never under 72.
const USER_W: f32 = 180.0;
const USER_MIN: f32 = 72.0;
/// The flag columns on the canvas, in the table's order: ADMIN, then
/// FOLDERS, UPLOAD, AUDIO and MODIFY.
const FLAG_W: [f32; 5] = [60.0, 72.0, 64.0, 56.0, 64.0];
/// The kit's table spacing (studio.rs): the gap between columns, and a
/// row's padding either side.
const COL_GAP: f32 = 16.0;
const CELL_PAD: f32 = 10.0;
/// A tick box's side, as the kit draws one.
const BOX: f32 = 16.0;
/// A field with its label over it, as the kit's `field` lays one out.
const FIELD_ROW_H: f32 = 14.0 + 6.0 + FIELD_H;
/// The modals' widths: the canvas's add form, the grant and the password,
/// and the remove gate.
const FORM_W: f32 = 640.0;
const SMALL_W: f32 = 480.0;
const GATE_W: f32 = 520.0;
/// A row of the grant's list.
const GRANT_ROW_H: f32 = 32.0;
/// Where each modal keeps, from pass to pass, the control its keyboard
/// was on ([`moved`]): the add form's focus, the grant's cursor, the
/// password modal's field.
const KEYS_AT: [&str; 3] = ["users-add-focus", "users-libs-cursor", "users-pw-focus"];

/// What a click in the pass asks of the room, applied once it is drawn.
enum Step {
    /// One of the room's own acts, as the cells view registers it.
    Act(Act),
    /// A key, for a control the canvas draws where the cells name only the
    /// key: the remove gate's close is its Esc.
    Key(KeyCode),
}

/// One pass of the room over the whole of `ui`: the room's area less its
/// first row, which is the host's pick banner's. `composition` is the
/// input method's uncommitted text, for the focused field alone. Returns
/// the focused field's caret, if a field has the keyboard, and what a
/// click asked to end (nothing here can).
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    let pal = Pal::now();
    let full = ui.max_rect();
    let column = Rect::from_min_max(full.min + vec2(PAD_X, PAD_TOP), (full.max - vec2(PAD_X, PAD_FOOT)).max(full.min));
    let note_rect = Rect::from_min_max(pos2(column.min.x, (column.max.y - NOTE_H).max(column.min.y)), column.max);
    let body_rect = Rect::from_min_max(column.min, pos2(column.max.x, (note_rect.min.y - GAP).max(column.min.y)));

    let mut steps = Vec::new();
    let mut base = ui.new_child(UiBuilder::new().id_salt("users-base").max_rect(body_rect).layout(Layout::top_down(Align::Min)));
    base.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
    draw_base(&mut base, room, &pal, &mut steps);
    note(ui, note_rect, room, &pal);

    let mut caret = None;
    if matches!(room.modal, Modal::None) {
        // The next modal to open scrolls its keyboard's control into view,
        // wherever its body stood when it last closed.
        ui.ctx().data_mut(|d| KEYS_AT.iter().for_each(|at| d.remove::<usize>(Id::new(*at))));
    } else {
        // A modal makes the room beneath inert: what was clicked under it
        // this pass is dropped, as the cells view drops what it registered.
        steps.clear();
        caret = draw_modal(ui, room, composition, full, &pal, &mut steps);
    }
    let mut outcome = None;
    for step in steps {
        let ended = match step {
            Step::Act(act) => room.act(act),
            Step::Key(code) => room.key(KeyEvent::new(code, KeyModifiers::NONE)),
        };
        outcome = outcome.or(ended);
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
fn draw_base(ui: &mut Ui, room: &Room, pal: &Pal, steps: &mut Vec<Step>) {
    state_line(ui, room, pal);
    ui.add_space(GAP);
    let label = if room.loaded && room.users.is_empty() { t!("usr.add_first_card") } else { t!("usr.add_card") };
    if add_card(ui, Id::new("users-add-card"), bare(&label), pal).clicked() {
        steps.push(Step::Act(Act::Add));
    }
    ui.add_space(GAP);
    table(ui, room, pal, steps);
}

/// Public mode in gold, else the count — or, once the room signed itself
/// in, who it is signed in as — after the state's LED, and the reload's
/// cadence dim at the right. Blank until the first load answers.
fn state_line(ui: &mut Ui, room: &Room, pal: &Pal) {
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
    let mid = row.center().y;
    let mut right = row.max.x;
    if !public {
        let polls = text(ui, row.right_center(), &t!("usr.polls"), body(13.0), pal.dim, w * 0.4, Align2::RIGHT_CENTER);
        right = polls.min.x - 16.0;
    }
    led(ui, pos2(row.min.x + 3.0, mid), lamp);
    let x = row.min.x + 16.0;
    let drawn = text(ui, pos2(x, mid), unbullet(&head), strong(14.0), head_color, (right - x).max(1.0), Align2::LEFT_CENTER);
    if right - drawn.max.x > 12.0 {
        text(ui, pos2(drawn.max.x, mid), &tail, body(14.0), tail_color, right - drawn.max.x, Align2::LEFT_CENTER);
    }
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
/// ringed; a row's five flags as tick boxes, and its remove in the trail.
/// A click on a row selects it, its boxes included, as in the cells; the
/// cross opens the remove gate.
fn table(ui: &mut Ui, room: &Room, pal: &Pal, steps: &mut Vec<Step>) {
    let w = ui.available_width();
    let cols = columns(ui, w);
    let id = Id::new("users-table");
    if room.users.is_empty() {
        // The headings and their rule over no rows, as the cells draw them
        // in every state.
        let (head, _) = ui.allocate_exact_size(vec2(w, HEAD_H), Sense::hover());
        let mut child = ui.new_child(UiBuilder::new().max_rect(head).layout(Layout::top_down(Align::Min)));
        let _ = Table::new(id, &cols, 0).trail(TRAIL).show(&mut child, pal, |_, _, _| {});
        if room.loaded {
            public(ui, pal);
        }
        return;
    }
    let rows: Vec<(&String, &AdminUser)> = room.users.iter().collect();
    let mut trails: Vec<Rect> = Vec::new();
    let flags = [(1, Flag::Admin), (3, Flag::Mkdir), (4, Flag::Upload), (5, Flag::Audio), (6, Flag::Modify)];
    let shown = Table::new(id, &cols, rows.len()).cursor(room.sel).trail(TRAIL).show(ui, pal, |ui, i, row| {
        let (name, u) = rows[i];
        let font = if row.selected { strong(14.0) } else { body(14.0) };
        cell(ui, row, 0, &printable(name, NAME_MAX), font, ink(row, pal.text, pal), false);
        let (libs, own) = if u.vpaths.is_empty() { ("—".to_string(), pal.dim) } else { (u.vpaths.join(" · "), pal.soft) };
        cell(ui, row, 2, &libs, body(13.5), ink(row, own, pal), false);
        for (at, flag) in flags {
            let mark = Rect::from_center_size(row.cell(at).center(), Vec2::splat(BOX));
            tick_box(ui, mark, flag.get(u), false, pal.panel, pal);
        }
        let at = pos2(row.trail.min.x + REMOVE_W / 2.0, row.rect.center().y);
        if icon_button(ui, Id::new("users-remove").with(i), at, Icon::Remove, pal.dim, pal).clicked() {
            steps.push(Step::Act(Act::Remove(name.clone())));
        }
        trails.push(row.trail);
    });
    // The trail is outside the selection, as the cells' remove column is:
    // a click there beside the cross selects nothing.
    if let Some(i) = shown.row {
        let at = ui.ctx().input(|input| input.pointer.interact_pos());
        if !at.is_some_and(|p| trails.iter().any(|t| t.contains(p))) {
            steps.push(Step::Act(Act::Select(i)));
        }
    }
}

/// Public mode's sentences under the empty table: the empty list, the two
/// warnings in gold, and what the player must do as well, since the
/// session the room makes is its own. Where the room is too short for
/// them, the lines with no room left are left out, as in the cells.
fn public(ui: &mut Ui, pal: &Pal) {
    // The cells' rhythm (the empty line under the rule, two rows to the
    // warnings, one to the last sentence) tight enough that all of it fits
    // the GUI's default window, as it does in cells.
    let floor = ui.clip_rect().max.y;
    ui.add_space(6.0);
    lines_above(ui, &t!("usr.empty_users"), body(14.0), pal.dim, floor);
    ui.add_space(18.0);
    lines_above(ui, &t!("usr.public_1"), body(14.0), pal.gold, floor);
    ui.add_space(2.0);
    lines_above(ui, &t!("usr.public_2"), body(14.0), pal.gold, floor);
    ui.add_space(10.0);
    lines_above(ui, &t!("usr.public_3_hosted"), body(14.0), pal.dim, floor);
}

/// The room's foot: what it is busy with, its note, or — with a row under
/// the cursor and no modal up — what that user sees, as the cells word it.
fn note(ui: &Ui, rect: Rect, room: &Room, pal: &Pal) {
    let row_note = match (&room.note, &room.busy, room.selected_name()) {
        (None, None, Some(name)) if matches!(room.modal, Modal::None) => room.users.get(&name).map(|u| {
            let user = printable(&name, NAME_MAX);
            let text = if u.vpaths.is_empty() {
                t!("usr.note_sees_none", user = user).to_string()
            } else {
                t!("usr.note_sees", user = user, libs = u.vpaths.join(" · ")).to_string()
            };
            (format!("{text}{}", t!("usr.note_flags")), false)
        }),
        _ => None,
    };
    let note = room.note.clone().or(row_note);
    note_line(ui, rect, room.busy.as_deref(), note.as_ref(), pal);
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The room's modal over `over`, the room's whole rect. Returns the caret
/// of its focused field, if it has one.
fn draw_modal(ui: &Ui, room: &Room, composition: &str, over: Rect, pal: &Pal, steps: &mut Vec<Step>) -> Option<Pos2> {
    match &room.modal {
        Modal::None => None,
        Modal::Add(f) => add_form(ui, f, composition, over, pal, steps),
        Modal::Libraries(d) => {
            grant(ui, d, over, pal, steps);
            None
        }
        Modal::Password(d) => password(ui, d, composition, over, pal, steps),
        Modal::Remove(name) => {
            gate(ui, room, name, over, pal, steps);
            None
        }
    }
}

/// The kit's field for one of the room's inputs: its value and caret, the
/// keyboard and the composition when `on`, masked where the cells mask.
fn entry<'a>(input: &'a Input, on: bool, composition: &'a str, masked: bool) -> studio::Input<'a> {
    let field = studio::Input::new(input.value(), input.cursor()).focused(on, if on { composition } else { "" });
    if masked { field.masked() } else { field }
}

/// The add form: the two fields side by side, the libraries in one row of
/// ticks (those with no room left off, as the cells leave them), the four
/// permissions, the name's note and the error, and the one button. The
/// control with the keyboard is ringed, and scrolled into view when the
/// keyboard moved to it.
fn add_form(ui: &Ui, f: &AddForm, composition: &str, over: Rect, pal: &Pal, steps: &mut Vec<Step>) -> Option<Pos2> {
    let fields = f.fields();
    let index_of = |field: AField| fields.iter().position(|x| *x == field).unwrap_or(0);
    let focused = f.focused();
    let shown = modal(ui, Id::new("users-add-form"), over, FORM_W, &t!("usr.form_title"), pal, |ui| {
        let mut caret = None;
        let jump = moved(ui, Id::new(KEYS_AT[0]), f.focus);
        let w = ui.available_width();

        let half = ((w - 24.0) / 2.0).max(0.0);
        let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_ROW_H), Sense::hover());
        let texts = [
            (AField::Username, t!("usr.field_username"), &f.username, false),
            (AField::Password, t!("usr.field_password"), &f.password, true),
        ];
        for (k, (which, label, value, masked)) in texts.into_iter().enumerate() {
            let rect = Rect::from_min_size(pos2(row.min.x + k as f32 * (half + 24.0), row.min.y), vec2(half, row.height()));
            let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
            let on = focused == which;
            let typed = field(&mut child, Id::new("users-add-field").with(k), &label, &entry(value, on, composition, masked), pal);
            if typed.response.clicked() {
                steps.push(Step::Act(Act::FormFocus(index_of(which))));
            }
            if on {
                caret = typed.caret;
                if jump {
                    // The label with its box: the field, not its box alone.
                    ui.scroll_to_rect(rect, None);
                }
            }
        }

        ui.add_space(FORM_GAP);
        let (head, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
        let hint = text(ui, head.right_center(), &t!("usr.form_libraries_hint"), body(13.0), pal.dim, w * 0.6, Align2::RIGHT_CENTER);
        small_caps(ui, head.left_center(), &t!("usr.form_libraries"), pal.dim, (hint.min.x - 16.0 - head.min.x).max(1.0), Align2::LEFT_CENTER);
        ui.add_space(8.0);
        let (line, _) = ui.allocate_exact_size(vec2(w, HIT), Sense::hover());
        if f.libraries.is_empty() {
            text(ui, line.left_center(), &t!("usr.form_no_libraries"), body(13.5), pal.dim, w, Align2::LEFT_CENTER);
        }
        let mut x = line.min.x;
        for (i, (name, on)) in f.libraries.iter().enumerate() {
            // The first is drawn however long, cut at the row's end; a
            // later one with no room left is left off, as in the cells.
            let label = printable(name, NAME_MAX);
            let want = BOX + 8.0 + width_of(ui, &label, body(14.0));
            if x + want > line.max.x && x > line.min.x {
                break;
            }
            let rect = Rect::from_min_max(pos2(x, line.min.y), pos2((x + want).min(line.max.x), line.max.y));
            let which = AField::Lib(i);
            let response = inline_tick(ui, Id::new("users-add-lib").with(i), rect, *on, &label, focused == which, pal);
            if response.clicked() {
                steps.push(Step::Act(Act::FormToggle(index_of(which))));
            }
            if focused == which && jump {
                response.scroll_to_me(None);
            }
            x = rect.max.x + 20.0;
        }

        ui.add_space(FORM_GAP);
        let (head, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
        small_caps(ui, head.left_center(), &t!("usr.form_permissions"), pal.dim, w, Align2::LEFT_CENTER);
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
            let response = check_line(ui, Id::new("users-add-perm").with(k), f.checked(which), &label, &desc, pal);
            if response.clicked() {
                steps.push(Step::Act(Act::FormToggle(index_of(which))));
            }
            if focused == which {
                ring(ui, response.rect.expand2(vec2(4.0, 0.0)), pal);
                if jump {
                    response.scroll_to_me(None);
                }
            }
        }

        ui.add_space(FORM_GAP);
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        text(ui, line.left_center(), &t!("usr.form_permanent"), body(13.0), pal.dim, w, Align2::LEFT_CENTER);
        ui.add_space(4.0);
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        if let Some(err) = &f.error {
            text(ui, line.left_center(), err, body(13.0), pal.gold, w, Align2::LEFT_CENTER);
        }
        ui.add_space(FORM_GAP);
        if button_row(ui, Id::new("users-add-submit"), &t!("usr.form_add"), true, pal).clicked() {
            steps.push(Step::Act(Act::FormSubmit));
        }
        caret
    });
    if shown.closed {
        steps.push(Step::Act(Act::FormCancel));
    }
    shown.inner
}

/// The library grant: a row a library, ticked where the user holds it,
/// the keyboard's cursor ringed; the two notes; Save.
fn grant(ui: &Ui, d: &LibsDraft, over: Rect, pal: &Pal, steps: &mut Vec<Step>) {
    let title = t!("usr.libs_title", user = printable(&d.username, NAME_MAX));
    let shown = modal(ui, Id::new("users-libraries"), over, SMALL_W, &title, pal, |ui| {
        let w = ui.available_width();
        if d.libraries.is_empty() {
            let (line, _) = ui.allocate_exact_size(vec2(w, GRANT_ROW_H), Sense::hover());
            text(ui, line.left_center(), &t!("usr.form_no_libraries"), body(13.5), pal.dim, w, Align2::LEFT_CENTER);
        }
        let jump = moved(ui, Id::new(KEYS_AT[1]), d.cursor);
        for (i, (name, on)) in d.libraries.iter().enumerate() {
            let cursor = i == d.cursor;
            let response = grant_row(ui, Id::new("users-libs-row").with(i), *on, &printable(name, NAME_MAX), cursor, pal);
            if response.clicked() {
                steps.push(Step::Act(Act::LibsRow(i)));
            }
            if cursor && jump {
                response.scroll_to_me(None);
            }
        }
        ui.add_space(16.0);
        paragraph(ui, &t!("usr.libs_note_1"), body(13.0), pal.dim);
        ui.add_space(2.0);
        paragraph(ui, &t!("usr.libs_note_2"), body(13.0), pal.dim);
        ui.add_space(FORM_GAP);
        if button_row(ui, Id::new("users-libs-save"), &t!("usr.libs_save"), true, pal).clicked() {
            steps.push(Step::Act(Act::LibsSubmit));
        }
    });
    if shown.closed {
        steps.push(Step::Act(Act::LibsCancel));
    }
}

/// A new password, typed twice, both masked; the error at the foot's left
/// and the button at its right, lit only while the two agree, as the
/// cells' waits, though a click is the submit either way.
fn password(ui: &Ui, d: &PwDraft, composition: &str, over: Rect, pal: &Pal, steps: &mut Vec<Step>) -> Option<Pos2> {
    let title = t!("usr.pw_title", user = printable(&d.username, NAME_MAX));
    let shown = modal(ui, Id::new("users-password"), over, SMALL_W, &title, pal, |ui| {
        let mut caret = None;
        let jump = moved(ui, Id::new(KEYS_AT[2]), d.focus);
        let pairs = [(t!("usr.field_password"), &d.password), (t!("usr.field_again"), &d.again)];
        for (k, (label, value)) in pairs.into_iter().enumerate() {
            if k > 0 {
                ui.add_space(16.0);
            }
            let on = d.focus == k;
            let top = ui.cursor().min.y;
            let typed = field(ui, Id::new("users-pw-field").with(k), &label, &entry(value, on, composition, true), pal);
            if typed.response.clicked() {
                steps.push(Step::Act(Act::PwFocus(k)));
            }
            if on {
                caret = typed.caret;
                if jump {
                    // The label with its box: the field, not its box alone.
                    ui.scroll_to_rect(typed.response.rect.with_min_y(top), None);
                }
            }
        }
        ui.add_space(FORM_GAP);
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
        let label = t!("usr.pw_set");
        let label = bare(&label);
        let at = Rect::from_min_max(pos2(row.max.x - form_button_width(ui, label).min(w), row.min.y), row.max);
        if let Some(err) = &d.error {
            text(ui, row.left_center(), err, body(13.0), pal.gold, (at.min.x - 16.0 - row.min.x).max(1.0), Align2::LEFT_CENTER);
        }
        if form_button(ui, Id::new("users-pw-set"), at, label, d.ready(), pal).clicked() {
            steps.push(Step::Act(Act::PwSubmit));
        }
        caret
    });
    if shown.closed {
        steps.push(Step::Act(Act::PwCancel));
    }
    shown.inner
}

/// The remove gate: what goes with the user in gold, what stays in green,
/// the last admin's warning where it applies; Keep lit, Remove not.
fn gate(ui: &Ui, room: &Room, name: &str, over: Rect, pal: &Pal, steps: &mut Vec<Step>) {
    let user = printable(name, NAME_MAX);
    let title = t!("usr.remove_title", user = user.clone());
    let last_admin = room.last_admin(name);
    let shown = modal(ui, Id::new("users-remove-gate"), over, GATE_W, &title, pal, |ui| {
        paragraph(ui, &t!("usr.remove_1"), body(14.0), pal.gold);
        ui.add_space(4.0);
        paragraph(ui, &t!("usr.remove_2"), body(14.0), pal.ok);
        if last_admin {
            ui.add_space(12.0);
            paragraph(ui, &t!("usr.remove_last_admin", user = user.clone()), strong(14.0), pal.gold);
        }
        ui.add_space(FORM_GAP);
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
        let (keep, go) = (t!("usr.remove_keep"), t!("usr.remove_confirm"));
        let (keep, go) = (bare(&keep), bare(&go));
        let go_at = Rect::from_min_max(pos2(row.max.x - form_button_width(ui, go), row.min.y), row.max);
        let keep_left = go_at.min.x - 10.0 - form_button_width(ui, keep);
        let keep_at = Rect::from_min_max(pos2(keep_left, row.min.y), pos2(go_at.min.x - 10.0, row.max.y));
        if form_button(ui, Id::new("users-remove-keep"), keep_at, keep, true, pal).clicked() {
            steps.push(Step::Act(Act::RemoveCancel));
        }
        if form_button(ui, Id::new("users-remove-go"), go_at, go, false, pal).clicked() {
            steps.push(Step::Act(Act::RemoveConfirm));
        }
    });
    // The cells' gate draws no close, and its tips name Esc for Keep; the
    // kit's window always has one, so it is that Esc.
    if shown.closed {
        steps.push(Step::Key(KeyCode::Esc));
    }
}

// ── Pieces the kit has no widget for ─────────────────────────────────────

/// A label bare of the arrow the cells mark a button with, at its end (or
/// its start): the kit's button is its own mark.
fn bare(s: &str) -> &str {
    s.trim().trim_end_matches('▸').trim_start_matches('◂').trim()
}

/// A state's words bare of the bullet the cells lead them with: the kit
/// draws it as the state's LED.
fn unbullet(s: &str) -> &str {
    s.trim_start().trim_start_matches('•').trim_start()
}

/// Whether `now` differs from what `id` held on the last pass; it holds
/// `now` from here on. The keyboard's control is scrolled into view on the
/// pass it moved, as the kit's table follows its cursor.
fn moved(ui: &Ui, id: Id, now: usize) -> bool {
    let last = ui.ctx().data(|d| d.get_temp::<usize>(id));
    ui.ctx().data_mut(|d| d.insert_temp(id, now));
    last != Some(now)
}

/// `s` wrapped at the layout's width, a row of the layout as tall as its
/// lines.
fn paragraph(ui: &mut Ui, s: &str, font: FontId, color: Color32) {
    lines_above(ui, s, font, color, f32::INFINITY);
}

/// [`paragraph`], its lines drawn only where they end above `floor`: a line
/// with no room left for it is left out rather than cut through, as the
/// cells leave out a row past the column's end.
fn lines_above(ui: &mut Ui, s: &str, font: FontId, color: Color32, floor: f32) {
    let w = ui.available_width();
    let galley = ui.painter().layout(s.to_string(), font, color, w.max(1.0));
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    let fits = galley.rows.iter().map(|row| rect.min.y + row.rect().max.y).filter(|foot| *foot <= floor).fold(rect.min.y, f32::max);
    if fits > rect.min.y {
        ui.painter().with_clip_rect(rect.with_max_y(fits).intersect(ui.clip_rect())).galley(rect.min, galley, color);
    }
}

/// A tick box at `rect` on `fill`, as the kit draws one: a dim edge when
/// off, the ok green's edge and check when on, each brighter under the
/// pointer. Alone it takes no click: a table's flags show their state, and
/// the row is what a click there selects.
fn tick_box(ui: &Ui, rect: Rect, on: bool, hovered: bool, fill: Color32, pal: &Pal) {
    let edge = if on { pal.ok } else { pal.dim };
    let edge = if hovered { lift(edge, 40) } else { edge };
    ui.painter().rect_filled(rect, square(), fill);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, edge), StrokeKind::Inside);
    if on {
        // The canvas's check in its 10-point box: M1.5 5.2 l2.4 2.4 L8.6 2.6.
        let o = rect.center() - vec2(5.0, 5.0);
        let points = vec![o + vec2(1.5, 5.2), o + vec2(3.9, 7.6), o + vec2(8.6, 2.6)];
        let check = if hovered { lift(pal.ok, 40) } else { pal.ok };
        ui.painter().add(Shape::line(points, Stroke::new(1.7, check)));
    }
}

/// One tick of a row of them at `rect` (the add form's libraries): the box
/// on the ground's fill and `label` after it, the whole rect the click's,
/// ringed while it has the keyboard.
fn inline_tick(ui: &Ui, id: Id, rect: Rect, on: bool, label: &str, focused: bool, pal: &Pal) -> Response {
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let mark = Rect::from_min_size(pos2(rect.min.x, rect.center().y - BOX / 2.0), Vec2::splat(BOX));
    tick_box(ui, mark, on, response.hovered(), pal.ground, pal);
    let x = mark.max.x + 8.0;
    text(ui, pos2(x, rect.center().y), label, body(14.0), pal.text, (rect.max.x - x).max(1.0), Align2::LEFT_CENTER);
    if focused {
        ring(ui, rect.expand2(vec2(4.0, 0.0)), pal);
    }
    response
}

/// A row of the grant's list, the layout's width: a tick box on the
/// ground's fill and the library's name, the keyboard's cursor raised and
/// ringed as a table's row is. The whole row takes the click; as in the
/// cells, it has no look of its own under the pointer.
fn grant_row(ui: &mut Ui, id: Id, on: bool, name: &str, cursor: bool, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, GRANT_ROW_H), Sense::hover());
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if cursor {
        ui.painter().rect_filled(rect, square(), pal.raised);
        ring(ui, rect, pal);
    }
    let mark = Rect::from_center_size(pos2(rect.min.x + 10.0 + BOX / 2.0, rect.center().y), Vec2::splat(BOX));
    tick_box(ui, mark, on, false, pal.ground, pal);
    let x = mark.max.x + 10.0;
    let font = if cursor { strong(14.0) } else { body(14.0) };
    text(ui, pos2(x, rect.center().y), name, font, pal.text, (rect.max.x - 10.0 - x).max(1.0), Align2::LEFT_CENTER);
    response
}

/// A form's foot: its one button at the right end of a row of the layout,
/// `label` bare of its arrow.
fn button_row(ui: &mut Ui, id: Id, label: &str, primary: bool, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
    let label = bare(label);
    let at = Rect::from_min_max(pos2(row.max.x - form_button_width(ui, label).min(w), row.min.y), row.max);
    form_button(ui, id, at, label, primary, pal)
}

#[cfg(test)]
mod tests {
    use egui::{Event, FullOutput, Modifiers, PointerButton};

    use super::super::tests::{english, libs, loaded, press, room, type_text};
    use super::*;

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
            assert!(has("3 users"), "the count, its bullet an LED: {words:?}");
            assert!(has(&t!("usr.state_admins_one")) && has(&t!("usr.polls")), "{words:?}");
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
        assert!(has("public") && has(&t!("usr.state_public_detail")), "{words:?}");
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
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-add-submit"));
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
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-libs-save"));
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
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-pw-set"));
        let Modal::Password(d) = &r.modal else { panic!("the password modal") };
        assert_eq!((d.error.as_deref(), d.focus), (Some("a password is needed"), 0), "refused as on Enter");
        let words = drawn(&pass(&ctx, &mut r, LARGE).out);
        assert!(words.iter().any(|w| w == "a password is needed"), "{words:?}");
        type_text(&mut r, "pw");
        press(&mut r, KeyCode::Tab);
        type_text(&mut r, "pw");
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-pw-set"));
        assert_eq!(r.queued, Some(Op::Password { username: "ben".into(), password: "pw".into() }));
        r.queued = None;
        let _ = pass(&ctx, &mut r, LARGE);
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-password").with("close"));
        assert!(matches!(r.modal, Modal::None), "the close is the modal's cancel");
    }

    #[test]
    fn the_remove_gate_keeps_removes_and_its_close_is_esc() {
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
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-remove-keep"));
        assert!(matches!(r.modal, Modal::None) && r.queued.is_none(), "Keep keeps");

        r.act(Act::Remove("ben".into()));
        let words = drawn(&settle(&ctx, &mut r, LARGE).out);
        assert!(!words.iter().any(|w| w.contains("only admin")), "{words:?}");
        let _ = click_on(&ctx, &mut r, LARGE, Id::new("users-remove-gate").with("close"));
        assert!(matches!(r.modal, Modal::None) && r.queued.is_none(), "the close is Esc, which keeps");

        r.act(Act::Remove("ben".into()));
        let _ = settle(&ctx, &mut r, LARGE);
        let shown = click_on(&ctx, &mut r, LARGE, Id::new("users-remove-go"));
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
}
