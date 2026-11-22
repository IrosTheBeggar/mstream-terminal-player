//! The Users room in character cells: what `mstream-player admin users`
//! draws in a terminal, and what the GUI player's Admin tab draws on its
//! grid ([`Screen::render_hosted`]). The room itself (its state, its
//! worker, its keys, its words) is the parent module; this is its view in
//! cells: the state line, the add card, the table of checkbox cells and
//! the four modals, on the kit's surface. It is also where the room meets
//! the hub's loop, as a [`Screen`].

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::super::{Screen, body_column, draw_foot, draw_header, frame_ground, host_of};
use super::*;
use crate::kit::theme::th;
use crate::kit::{self, Surface, bold, dim};
use crate::setup::g;

const MIN_W: u16 = 80;
const MIN_H: u16 = 22;
/// The USER column, the wizard's NAME width.
const USER_W: u16 = 16;
/// The ` [X]` column, outside the selection area.
const REMOVE_W: u16 = 4;

/// What this view keeps on the room between frames. The room holds one
/// and never looks inside it.
pub(super) struct State {
    /// The kit's interaction surface: what the last frame registered.
    ui: Surface<Act>,
    /// The cursor as the table last saw it: the table scrolls to the
    /// cursor only on the frame it moved.
    sel_anchor: Option<usize>,
}

impl State {
    pub(super) fn new() -> State {
        State { ui: Surface::new(), sel_anchor: None }
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

    fn render_hosted(&mut self, frame: &mut Frame, area: Rect) {
        self.cells.ui.begin_frame();
        draw(frame, self, area, true);
    }

    fn hint(&self) -> String {
        footer_hint_as(self, true)
    }

    fn modal_open(&self) -> bool {
        !matches!(self.modal, Modal::None)
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

    #[cfg(feature = "window")]
    fn has_egui(&self) -> bool {
        true
    }

    /// The room's egui face (egui_view.rs), the input method's composition
    /// taken from where the host put it.
    #[cfg(feature = "window")]
    fn egui(&mut self, ui: &mut egui::Ui) -> Option<super::super::EguiShown> {
        let composition = self.cells.ui.composition().to_string();
        Some(super::egui_view::show(ui, self, &composition))
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.cells.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    draw(frame, room, area, false);
}

/// The room inside `area`. Standalone it sits under its own header and
/// over its tips row. Hosted (the GUI player's Admin tab) the host's bar
/// is the header and its footer carries the tips, so the area's last row
/// is the status line alone, the cursor row's own line included. Modals
/// and the tooltip centre in `area` either way.
fn draw(frame: &mut Frame, room: &mut Room, area: Rect, hosted: bool) {
    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.cells.ui.pointer;
    if modal_open {
        room.cells.ui.pointer = None;
    }

    if !hosted {
        draw_header(frame, area, &t!("usr.title"), &host_of(&room.client));
    }
    let column = body_column(area, hosted);
    draw_body(frame, room, column, hosted);

    // The cursor row's own line, when nothing louder holds the note line.
    let row_note = match (&room.note, &room.busy, room.selected_name()) {
        (None, None, Some(name)) if !modal_open => room.users.get(&name).map(|u| {
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
    let tips = footer_hint_as(room, hosted);
    draw_foot(frame, area, note.as_ref(), room.busy.as_deref(), &tips, hosted);

    if modal_open {
        room.cells.ui.pointer = live_pointer;
        room.cells.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Add(f) => draw_add(frame, room, area, &f),
        Modal::Libraries(d) => draw_libraries(frame, room, area, &d),
        Modal::Password(d) => draw_password(frame, room, area, &d),
        Modal::Remove(name) => draw_remove(frame, room, area, &name),
    }

    if let Some((target, text)) = room.cells.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// The state line, the add card, the table. Hosted, public mode's last
/// sentence says what the player must do as well (its session is its own).
fn draw_body(frame: &mut Frame, room: &mut Room, column: Rect, hosted: bool) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let mut y = column.y;

    // The state line: public mode in gold, else the count — and, once the
    // room signed itself in, who it is signed in as.
    if room.loaded {
        let public = room.users.is_empty();
        let spans = if public {
            vec![
                Span::styled(t!("usr.state_public").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                Span::styled(t!("usr.state_public_detail").to_string(), Style::default().fg(th().gold)),
            ]
        } else if let Some(user) = &room.signed_in_as {
            let user = printable(user, NAME_MAX);
            let detail = match &room.session_unsaved {
                None => t!("usr.state_signed_in_detail", user = user).to_string(),
                Some(err) => t!("usr.state_signed_in_unsaved", user = user, err = err.clone()).to_string(),
            };
            vec![
                Span::styled(t!("usr.state_signed_in").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
                Span::styled(detail, if room.session_unsaved.is_some() { Style::default().fg(th().gold) } else { Style::default() }),
            ]
        } else {
            let n = room.users.len();
            let admins = room.admins();
            let head = if n == 1 { t!("usr.state_users_one") } else { t!("usr.state_users", n = n) }.to_string();
            let tail = if admins == 1 { t!("usr.state_admins_one") } else { t!("usr.state_admins", n = admins) }.to_string();
            vec![Span::styled(head, bold()), Span::raw(tail)]
        };
        frame.render_widget(Paragraph::new(Line::from(spans)), line(y));
        if !public {
            frame.render_widget(Paragraph::new(Span::styled(t!("usr.polls").to_string(), dim())).alignment(Alignment::Right), line(y));
        }
    }
    y += 2;

    // The add card: the room's one add affordance, above the table.
    let add_rect = Rect { x: column.x, y, width: column.width, height: 3 };
    let add_hover = room.cells.ui.pointer.is_some_and(|p| add_rect.contains(p));
    let add_color = if add_hover { th().bright } else { th().ok };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(add_color));
    let inner = block.inner(add_rect);
    frame.render_widget(block, add_rect);
    let label = if room.loaded && room.users.is_empty() { t!("usr.add_first_card") } else { t!("usr.add_card") };
    frame.render_widget(
        Paragraph::new(Span::styled(label.to_string(), Style::default().fg(add_color).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        inner,
    );
    room.cells.ui.click(add_rect, Act::Add);
    y += 4;

    // The table. The [X] remove control sits to the RIGHT of the selection.
    let sel_width = column.width.saturating_sub(REMOVE_W);
    let sel_end = column.x + sel_width;
    let x_col = sel_end + 1;
    let user_x = column.x;
    let admin_x = user_x + USER_W + 2;
    // The flag columns are as wide as their headers in cells, as drawn: a
    // Japanese header counted by characters got half its width, so it was
    // cut and the glyphs centred under it sat off its middle.
    let admin_w = (kit::width(&Flag::Admin.header()) as u16).max(3);
    let libs_x = admin_x + admin_w + 2;
    // The four flag columns hang off the selection's right edge.
    let mut flag_cols: Vec<(Flag, u16, u16)> = Vec::new(); // (flag, x, w)
    let mut right = sel_end;
    for flag in [Flag::Modify, Flag::Audio, Flag::Upload, Flag::Mkdir] {
        let w = (kit::width(&flag.header()) as u16).max(3);
        right = right.saturating_sub(w);
        flag_cols.push((flag, right, w));
        right = right.saturating_sub(2);
    }
    flag_cols.reverse();
    let libs_w = flag_cols.first().map_or(0, |(_, x, _)| x.saturating_sub(2).saturating_sub(libs_x));
    let mut cols: Vec<(u16, u16, String, bool)> = vec![
        (user_x, USER_W, t!("usr.col_user").to_string(), false),
        (admin_x, admin_w, Flag::Admin.header(), false),
        (libs_x, libs_w, t!("usr.col_libraries").to_string(), false),
    ];
    cols.extend(flag_cols.iter().map(|(f, x, w)| (*x, *w, f.header(), false)));
    for (x, w, word, _) in &cols {
        frame.render_widget(Paragraph::new(Span::styled(clip(word, *w), dim())), Rect { x: *x, y, width: *w, height: 1 });
    }
    y += 1;
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(column.width as usize), dim())), line(y));
    y += 1;

    if room.users.is_empty() {
        if room.loaded {
            frame.render_widget(Paragraph::new(Span::styled(t!("usr.empty_users").to_string(), dim())), line(y));
            // The webapp's two warnings, for the one state they apply to,
            // then what the room does about the first user. Hosted, that
            // sentence says the player must sign in as the user too: the
            // room keeps the session it makes, the player's stays its own,
            // and the rest of the Admin tab is refused without it. Each
            // wraps at the column, 79 cells hosted in a 100-column window.
            let gold = Style::default().fg(th().gold);
            let after = if hosted { t!("usr.public_3_hosted") } else { t!("usr.public_3") };
            let sentences = [(t!("usr.public_1"), gold, 0), (t!("usr.public_2"), gold, 0), (after, dim(), 1)];
            let mut r = y + 3;
            for (text, style, gap) in sentences {
                r += gap;
                for part in kit::wrap_words(&text, column.width as usize) {
                    if r < column.bottom() {
                        frame.render_widget(Paragraph::new(Span::styled(part, style)), line(r));
                    }
                    r += 1;
                }
            }
        }
        return;
    }

    let rows_rect = Rect { x: column.x, y, width: sel_width, height: column.bottom().saturating_sub(y) };
    if rows_rect.height == 0 {
        return;
    }
    // The accounts are lent to the rows and put back, not copied for them
    // every frame: a row needs the room only for its clicks (performance
    // audit #110).
    let users = std::mem::take(&mut room.users);
    let rows: Vec<(&String, &AdminUser)> = users.iter().collect();
    // A checkbox cell's glyph — a bar on the selected row, its own colour
    // otherwise — sitting one cell in from the column's left edge.
    let glyph_x = |x: u16, w: u16| x + (w.saturating_sub(3)) / 2;
    table_rows(frame, room, rows_rect, rows.len(), |frame, room, i, rect, selected, hovered| {
        let (name, u) = rows[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(name, NAME_MAX), USER_W), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(user_x, USER_W),
        );
        let libs = if u.vpaths.is_empty() { "—".to_string() } else { u.vpaths.join(" · ") };
        frame.render_widget(Paragraph::new(Span::styled(clip(&libs, libs_w), cell_style(selected, hovered, if u.vpaths.is_empty() { dim() } else { Style::default() }))), cell(libs_x, libs_w));
        let mut boxes: Vec<(Flag, u16, u16)> = vec![(Flag::Admin, admin_x, admin_w)];
        boxes.extend(flag_cols.iter().copied());
        for (flag, x, w) in boxes {
            let on = flag.get(u);
            let glyph = if on { g("[✓]", "[x]") } else { "[ ]" };
            let own = if on { Style::default().fg(th().ok) } else { dim() };
            let glyph_rect = cell(glyph_x(x, w), 3);
            frame.render_widget(Paragraph::new(Span::styled(glyph, cell_style(selected, hovered, own))), glyph_rect);
            room.cells.ui.click(glyph_rect, Act::Flip(name.clone(), flag));
        }
        // The [X], outside the selection: dim, red under the pointer.
        let x_rect = Rect { x: x_col, y: rect.y, width: 3, height: 1 };
        let x_hover = room.cells.ui.pointer.is_some_and(|p| x_rect.contains(p));
        let x_style = if x_hover { Style::default().fg(th().danger).add_modifier(Modifier::BOLD) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled("[X]", x_style)), x_rect);
        room.cells.ui.click(x_rect, Act::Remove(name.clone()));
    });
    room.users = users;
}

fn table_rows(
    frame: &mut Frame,
    room: &mut Room,
    at: Rect,
    len: usize,
    mut draw_row: impl FnMut(&mut Frame, &mut Room, usize, Rect, bool, bool),
) {
    let avail = at.height as usize;
    let sel_moved = room.sel != room.cells.sel_anchor;
    room.cells.sel_anchor = room.sel;
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

fn cell_style(selected: bool, hovered: bool, own: Style) -> Style {
    if selected {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if hovered {
        Style::default().fg(th().bright)
    } else {
        own
    }
}

fn clip(s: &str, w: u16) -> String {
    let w = w as usize;
    if s.chars().count() <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('…');
        out
    }
}

/// A labelled three-row field: the label above, the value inside. Masked
/// for a password.
#[allow(clippy::too_many_arguments)]
fn field_box(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, masked: bool, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = room.cells.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = block.inner(field);
    frame.render_widget(block, field);
    let w = inner.width.saturating_sub(2);
    let value = if masked { "•".repeat(input.value().chars().count()) } else { input.value().to_string() };
    let shown = if focused {
        kit::field_display(&mut room.cells.ui, inner.x + 1, inner.y, &value, input.cursor(), w, masked.then_some('•'))
    } else {
        clip(&value, w)
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: w, height: 1 });
    room.cells.ui.click(field, act);
}

/// A checkbox row in a modal: the glyph wears its state, the keyboard
/// cursor is the accent bar on the glyph, hover brightens the label.
#[allow(clippy::too_many_arguments)]
fn check_row(frame: &mut Frame, room: &mut Room, at: Rect, on: bool, focused: bool, label: &str, desc: &str, act: Act) {
    let hover = room.cells.ui.pointer.is_some_and(|p| at.contains(p));
    let glyph_style = if focused {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if on {
        Style::default().fg(th().ok)
    } else {
        dim()
    };
    let label_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else { Style::default() };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(if on { g("[✓]", "[x]") } else { "[ ]" }, glyph_style),
            Span::styled(format!(" {label}"), label_style),
            Span::styled(desc.to_string(), dim()),
        ])),
        at,
    );
    room.cells.ui.click(at, act);
}

fn draw_add(frame: &mut Frame, room: &mut Room, area: Rect, f: &AddForm) {
    let inner = kit::modal_frame(frame, area, 84, 22, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    let fits = |y: u16| y < inner.bottom();
    frame.render_widget(
        Paragraph::new(Span::styled(t!("usr.form_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::FormCancel);

    let fields = f.fields();
    let index_of = |field: AField| fields.iter().position(|x| *x == field).unwrap_or(0);
    let focused = f.focused();
    let half = w.saturating_sub(4) / 2;
    let mut y = inner.y + 2;
    if fits(y + 3) {
        field_box(frame, room, Rect { x, y, width: half, height: 4 }, &t!("usr.field_username"), &f.username, focused == AField::Username, false, Act::FormFocus(index_of(AField::Username)));
        field_box(frame, room, Rect { x: x + half + 4, y, width: half, height: 4 }, &t!("usr.field_password"), &f.password, focused == AField::Password, true, Act::FormFocus(index_of(AField::Password)));
    }
    y += 5;

    if fits(y + 1) {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_libraries").to_string(), dim())), line(y));
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_libraries_hint").to_string(), dim())).alignment(Alignment::Right), line(y));
        y += 1;
        if f.libraries.is_empty() {
            frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_no_libraries").to_string(), dim())), line(y));
        } else {
            // One row of ticks, each its own click target, 3 cells apart,
            // and each as wide as it draws: the label in cells and the
            // tick's four. Counted by characters, a library named in
            // Japanese got a rect short by half its name, so the name was
            // cut there and its click covered only what was left, and the
            // row took a tick it had no room for instead of leaving it off.
            // The first tick is drawn however wide, clamped to the row so
            // its label is cut inside it, as kit::button cuts its own: in
            // cells a name of forty Japanese characters (well under
            // NAME_MAX) outgrows the 80-cell row by itself, and breaking on
            // it left the line blank, every later library hidden with it,
            // while Tab still walked onto ticks nobody could see.
            let mut lx = x;
            for (i, (name, on)) in f.libraries.iter().enumerate() {
                let label = printable(name, NAME_MAX);
                let cw = kit::width(&label) as u16 + 4;
                if lx + cw > x + w && lx > x {
                    break;
                }
                let rect = Rect { x: lx, y, width: cw.min((x + w).saturating_sub(lx)), height: 1 };
                check_row(frame, room, rect, *on, focused == AField::Lib(i), &label, "", Act::FormToggle(index_of(AField::Lib(i))));
                lx += rect.width + 3;
            }
        }
    }
    y += 2;

    if fits(y) {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_permissions").to_string(), dim())), line(y));
    }
    y += 1;
    let perms = [
        (AField::Admin, t!("usr.perm_admin").to_string(), t!("usr.perm_admin_desc").to_string()),
        (AField::Mkdir, t!("usr.perm_mkdir").to_string(), String::new()),
        (AField::Upload, t!("usr.perm_upload").to_string(), String::new()),
        (AField::Audio, t!("usr.perm_audio").to_string(), t!("usr.perm_audio_desc").to_string()),
    ];
    for (field, label, desc) in perms {
        if fits(y) {
            check_row(frame, room, line(y), f.checked(field), focused == field, &label, &desc, Act::FormToggle(index_of(field)));
        }
        y += 1;
    }
    y += 1;
    if fits(y) {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_permanent").to_string(), dim())).wrap(Wrap { trim: true }), line(y));
    }
    y += 1;
    if let Some(err) = &f.error
        && fits(y)
    {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))).wrap(Wrap { trim: true }), line(y));
    }
    let by = inner.bottom().saturating_sub(1);
    let label = t!("usr.form_add").to_string();
    let bw = kit::width(&label) as u16 + 4;
    kit::button(frame, &mut room.cells.ui, Rect { x: inner.right().saturating_sub(bw + 1), y: by, width: bw, height: 1 }, &label, true, Act::FormSubmit);
}

fn draw_libraries(frame: &mut Frame, room: &mut Room, area: Rect, d: &LibsDraft) {
    let rows = d.libraries.len().max(1) as u16;
    let inner = kit::modal_frame(frame, area, 60, rows + 9, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    let fits = |y: u16| y < inner.bottom();
    frame.render_widget(
        Paragraph::new(Span::styled(t!("usr.libs_title", user = printable(&d.username, NAME_MAX)).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::LibsCancel);
    let mut y = inner.y + 2;
    if d.libraries.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_no_libraries").to_string(), dim())), line(y));
        y += 1;
    }
    for (i, (name, on)) in d.libraries.iter().enumerate() {
        if !fits(y) {
            break;
        }
        let rect = line(y);
        let cursor = i == d.cursor;
        if cursor {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(w as usize), Style::default().fg(th().on_accent).bg(th().accent))), rect);
        }
        let glyph_style = if cursor { Style::default().fg(th().on_accent).bg(th().accent) } else if *on { Style::default().fg(th().ok) } else { dim() };
        let name_style = if cursor { Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD) } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if *on { g("[✓]", "[x]") } else { "[ ]" }, glyph_style),
                Span::styled(format!(" {}", printable(name, NAME_MAX)), name_style),
            ])),
            rect,
        );
        room.cells.ui.click(rect, Act::LibsRow(i));
        y += 1;
    }
    y += 1;
    for key in ["usr.libs_note_1", "usr.libs_note_2"] {
        if fits(y) {
            frame.render_widget(Paragraph::new(Span::styled(t!(key).to_string(), dim())), line(y));
        }
        y += 1;
    }
    let by = inner.bottom().saturating_sub(1);
    let label = t!("usr.libs_save").to_string();
    let bw = kit::width(&label) as u16 + 4;
    kit::button(frame, &mut room.cells.ui, Rect { x: inner.right().saturating_sub(bw + 1), y: by, width: bw, height: 1 }, &label, true, Act::LibsSubmit);
}

fn draw_password(frame: &mut Frame, room: &mut Room, area: Rect, d: &PwDraft) {
    let inner = kit::modal_frame(frame, area, 60, 15, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("usr.pw_title", user = printable(&d.username, NAME_MAX)).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::PwCancel);
    let fw = w.min(44);
    if inner.y + 5 < inner.bottom() {
        field_box(frame, room, Rect { x, y: inner.y + 2, width: fw, height: 4 }, &t!("usr.field_password"), &d.password, d.focus == 0, true, Act::PwFocus(0));
    }
    if inner.y + 10 < inner.bottom() {
        field_box(frame, room, Rect { x, y: inner.y + 7, width: fw, height: 4 }, &t!("usr.field_again"), &d.again, d.focus == 1, true, Act::PwFocus(1));
    }
    let by = inner.bottom().saturating_sub(1);
    if let Some(err) = &d.error {
        frame.render_widget(Paragraph::new(Span::styled(format!("{} {err}", g("!", "!")), Style::default().fg(th().gold))), line(by));
    }
    let label = t!("usr.pw_set").to_string();
    let bw = kit::width(&label) as u16 + 4;
    // Dim until the two agree — the button waits, like the kit's disabled.
    kit::button(frame, &mut room.cells.ui, Rect { x: inner.right().saturating_sub(bw + 1), y: by, width: bw, height: 1 }, &label, d.ready(), Act::PwSubmit);
}

fn draw_remove(frame: &mut Frame, room: &mut Room, area: Rect, name: &str) {
    let last_admin = room.last_admin(name);
    let inner = kit::modal_frame(frame, area, 70, 11, th().gold);
    let gold = Style::default().fg(th().gold);
    let user = printable(name, NAME_MAX);
    let mut lines = vec![
        Line::from(Span::styled(t!("usr.remove_title", user = user.clone()).to_string(), gold.add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(t!("usr.remove_1").to_string(), gold)),
        Line::from(Span::styled(t!("usr.remove_2").to_string(), Style::default().fg(th().ok))),
    ];
    if last_admin {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(t!("usr.remove_last_admin", user = user).to_string(), gold.add_modifier(Modifier::BOLD))));
    }
    let body = Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);

    let y = inner.bottom().saturating_sub(1);
    let keep = t!("usr.remove_keep").to_string();
    let remove = t!("usr.remove_confirm").to_string();
    let remove_w = kit::width(&remove) as u16 + 4;
    let keep_w = kit::width(&keep) as u16 + 4;
    let keep_x = inner.right().saturating_sub(remove_w + 2 + keep_w);
    let keep_rect = kit::button(frame, &mut room.cells.ui, Rect { x: keep_x, y, width: inner.width, height: 1 }, &keep, true, Act::RemoveCancel);
    kit::button(frame, &mut room.cells.ui, Rect { x: keep_rect.right() + 2, y, width: inner.width, height: 1 }, &remove, false, Act::RemoveConfirm);
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::tests::*;
    use super::*;

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

    fn row(frame: &str, needle: &str) -> String {
        frame.lines().find(|l| l.contains(needle)).map(|l| l.to_string()).unwrap_or_else(|| panic!("no row with {needle:?}:\n{frame}"))
    }

    /// The add form's library ticks measure their names in cells: four
    /// libraries named in Japanese, ten characters (twenty cells) each, at
    /// 100×30. Three ticks of 24 cells and their gaps fill 78 of the row's
    /// 80, so the fourth is left off; each tick's click rect is its drawn
    /// width and shows its whole name. Counted by characters the row took
    /// all four, each cut at half its name.
    #[test]
    fn the_add_forms_library_ticks_measure_japanese_names_in_cells() {
        let _en = english();
        let names = ["日本の音楽アーカイブ", "クラシック全集の録音", "ジャズのレコード盤集", "映画音楽のサウンド集"];
        let mut r = room();
        r.queued = None;
        r.apply(Done::Loaded(Ok((BTreeMap::new(), names.iter().map(|s| s.to_string()).collect()))));
        press(&mut r, KeyCode::Char('a'));
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        let fields = f.fields();
        let index_of = |field: AField| fields.iter().position(|x| *x == field).expect("a field");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut r)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let tick = |i: usize| {
            let act = Act::FormToggle(index_of(AField::Lib(i)));
            r.cells.ui.clicks.iter().find(|(_, a)| *a == act).map(|(rect, _)| *rect)
        };
        let ticks: Vec<Rect> = (0..3).map(|i| tick(i).unwrap_or_else(|| panic!("tick {i}"))).collect();
        assert_eq!(tick(3), None, "the fourth has no room, so it is left off");
        for (i, rect) in ticks.iter().enumerate() {
            assert_eq!(rect.width, 24, "tick {i}: its name's 20 cells and the tick's four");
            if i > 0 {
                assert_eq!(rect.x, ticks[i - 1].right() + 3, "tick {i}: three cells past the last");
            }
            let mut text = String::new();
            let mut x = rect.x;
            while x < rect.right() {
                let symbol = buffer[(x, rect.y)].symbol();
                text.push_str(symbol);
                x += (kit::width(symbol) as u16).max(1);
            }
            assert_eq!(text, format!("[✓] {}", names[i]), "tick {i}: its whole name");
        }
    }

    /// A first library whose name alone outgrows the row is still drawn:
    /// forty Japanese characters are 80 cells, so its tick wants 84 of the
    /// row's 80 at 100×30. The tick starts the row, is clamped to its end
    /// (the border cell past it survives), shows the start of its name cut
    /// cleanly, and its click rect is the drawn rect; the second library,
    /// with no room left, is left off. Breaking on the first tick, as the
    /// row once did, left the whole line blank.
    #[test]
    fn the_add_forms_first_library_tick_is_drawn_clamped_when_its_name_outgrows_the_row() {
        let _en = english();
        let long = "日本の音楽アーカイブ".repeat(4);
        assert_eq!(kit::width(&long), 80);
        let mut r = room();
        r.queued = None;
        r.apply(Done::Loaded(Ok((BTreeMap::new(), vec![long.clone(), "music".to_string()]))));
        press(&mut r, KeyCode::Char('a'));
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        let fields = f.fields();
        let index_of = |field: AField| fields.iter().position(|x| *x == field).expect("a field");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut r)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let tick = |i: usize| {
            let act = Act::FormToggle(index_of(AField::Lib(i)));
            r.cells.ui.clicks.iter().find(|(_, a)| *a == act).map(|(rect, _)| *rect)
        };
        let first = tick(0).expect("the first tick is drawn");
        assert_eq!(tick(1), None, "no room is left for the second");
        // The modal is 84 wide at x 8, so the row runs from 10 to 90 and the
        // border sits at 91.
        assert_eq!((first.x, first.right()), (10, 90), "clamped to the row: {first:?}");
        assert_eq!(buffer[(first.right() + 1, first.y)].symbol(), "│", "the border survives");
        let mut text = String::new();
        let mut x = first.x;
        while x < first.right() {
            let symbol = buffer[(x, first.y)].symbol();
            text.push_str(symbol);
            x += (kit::width(symbol) as u16).max(1);
        }
        let whole = format!("[✓] {long}");
        assert!(text.len() > "[✓] ".len() && whole.starts_with(text.trim_end()), "cut, not garbled: {text:?}");
    }

    #[test]
    fn boots_loading_then_draws_the_table_with_its_flags_and_the_cursor_note() {
        let _en = english();
        let mut r = start(Client::new("http://home.mstream.example:3000").expect("client"));
        assert_eq!(r.queued, Some(Op::Load));
        r.queued = None;
        let mut users = BTreeMap::new();
        users.insert("anna".to_string(), user(true, &["music", "audiobooks", "field-recordings"], true, true, true, true));
        users.insert("ben".to_string(), user(false, &["music", "field-recordings"], true, false, false, true));
        users.insert("guest".to_string(), user(false, &["music"], false, false, false, false));
        r.apply(Done::Loaded(Ok((users, libs()))));
        assert!(r.loaded);
        let frame = draw(&mut r);
        assert!(frame.contains("Users"), "{frame}");
        assert!(frame.contains("• 3 users — 1 admin · logins on"), "{frame}");
        assert!(frame.contains("reloads every 30 s"), "{frame}");
        assert!(frame.contains("Add a user ▸"), "{frame}");
        for word in ["USER", "ADMIN", "LIBRARIES", "FOLDERS", "UPLOAD", "AUDIO", "MODIFY"] {
            assert!(frame.contains(word), "{word} missing:\n{frame}");
        }
        let anna = row(&frame, "anna");
        assert_eq!(anna.matches("[✓]").count(), 5, "every flag on: {anna}");
        assert!(anna.contains("music · audiobooks · field-recordi…") && anna.contains("[X]"), "the column clips, the note line does not: {anna}");
        let guest = row(&frame, "guest");
        assert_eq!(guest.matches("[ ]").count(), 5, "every flag off: {guest}");
        assert!(frame.contains("↑ ↓ select · a add a user · q quit"), "{frame}");
        // The cursor picks up rows and the note line says what they see.
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Down);
        assert_eq!(r.selected_name().as_deref(), Some("ben"));
        let frame = draw(&mut r);
        assert!(frame.contains("ben sees music · field-recordings · 1–5 flip a flag, saved at once"), "{frame}");
        assert!(frame.contains("r remove · Esc deselect"), "{frame}");
        assert!(press(&mut r, KeyCode::Esc).is_none() && r.sel.is_none(), "Esc stows the cursor");
        assert!(matches!(press(&mut r, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn public_mode_says_so_and_the_first_user_signs_the_room_in() {
        let _en = english();
        let scratch = crate::config::testing::Scratch::new("admin-users-first-signin");
        let _ = &scratch;
        let mut r = room();
        r.queued = None;
        r.apply(Done::Loaded(Ok((BTreeMap::new(), libs()))));
        let frame = draw(&mut r);
        assert!(frame.contains("• public — no users, so no logins"), "{frame}");
        assert!(frame.contains("Add the first user ▸") && frame.contains("(no users yet — a adds the first)"), "{frame}");
        assert!(frame.contains("The first user turns logins on") && frame.contains("Make it an admin"), "{frame}");
        assert!(frame.contains(&*t!("usr.public_3")), "standalone, the room's own session keeps the panel open:\n{frame}");
        assert!(frame.contains("a add the first user · q quit"), "{frame}");

        press(&mut r, KeyCode::Char('a'));
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        assert!(f.admin, "admin is pre-ticked only on a server with no users");
        assert!(f.mkdir && f.upload && !f.audio, "the webapp's defaults");
        assert!(f.libraries.iter().all(|(_, on)| *on), "every library ticked");
        type_text(&mut r, "mira");
        press(&mut r, KeyCode::Tab);
        type_text(&mut r, "hunter2");
        let frame = draw(&mut r);
        assert!(frame.contains("Add a user") && frame.contains("mira") && frame.contains("•••••••"), "{frame}");
        assert!(!frame.contains("hunter2"), "masked:\n{frame}");
        assert!(frame.contains("[✓] admin") && frame.contains("[✓] create folders") && frame.contains("[ ] server audio"), "{frame}");
        press(&mut r, KeyCode::Enter);
        assert_eq!(
            r.queued,
            Some(Op::Add(NewUser {
                username: "mira".into(),
                password: "hunter2".into(),
                vpaths: libs(),
                admin: true,
                allow_mkdir: true,
                allow_upload: true,
                allow_server_audio: false,
            }))
        );
        r.queued = None;
        // The server said yes: logins are on now, so the room signs in as mira.
        r.apply(Done::Added { username: "mira".into(), result: Ok(()) });
        assert!(matches!(r.modal, Modal::None));
        assert_eq!(r.queued, Some(Op::SignIn { username: "mira".into(), password: "hunter2".into() }));
        r.queued = None;
        r.apply(Done::SignedIn { username: "mira".into(), result: Ok(LoginResponse { token: "tok".into(), vpaths: vec![] }) });
        assert_eq!(r.signed_in_as.as_deref(), Some("mira"));
        assert!(note(&r).contains("signed in as mira") && note(&r).contains("saved"), "{}", note(&r));
        assert_eq!(r.queued, Some(Op::Load), "and reloads with the session");
        let creds = crate::config::load_credentials().unwrap();
        assert_eq!(crate::config::token_for(&creds, "http://home.mstream.example:3000"), Some("tok".to_string()));
        r.queued = None;
        let mut users = BTreeMap::new();
        users.insert("mira".to_string(), user(true, &["music"], true, true, false, true));
        r.apply(Done::Loaded(Ok((users, libs()))));
        r.note = None;
        let frame = draw(&mut r);
        assert!(frame.contains("• logins on — signed in as mira · the session is saved"), "{frame}");
    }

    #[test]
    fn the_libraries_modal_ticks_and_saves_the_whole_list() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Down); // ben
        press(&mut r, KeyCode::Char('l'));
        let frame = draw(&mut r);
        assert!(frame.contains("Libraries for ben"), "{frame}");
        assert!(frame.contains("[✓] music") && frame.contains("[ ] audiobooks") && frame.contains("[✓] field-recordings"), "{frame}");
        assert!(frame.contains("Admins see only these too"), "{frame}");
        assert!(frame.contains("Save ▸") && frame.contains("Space tick · Enter save"), "{frame}");
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Char(' '));
        press(&mut r, KeyCode::Enter);
        assert_eq!(r.queued, Some(Op::Vpaths { username: "ben".into(), vpaths: libs() }), "the whole list, in library order");
        r.queued = None;
        r.apply(Done::VpathsSet { username: "ben".into(), result: Ok(()) });
        assert!(matches!(r.modal, Modal::None));
        assert_eq!(note(&r), "ben: libraries saved");
        assert_eq!(r.queued, Some(Op::Load));
    }

    #[test]
    fn the_password_modal_refuses_a_mismatch_then_sends_masked_all_the_way() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Down); // ben
        press(&mut r, KeyCode::Char('p'));
        press(&mut r, KeyCode::Enter);
        let Modal::Password(d) = &r.modal else { panic!("the password modal") };
        assert_eq!(d.error.as_deref(), Some("a password is needed"));
        type_text(&mut r, "hunter2");
        press(&mut r, KeyCode::Tab);
        type_text(&mut r, "hunter3");
        press(&mut r, KeyCode::Enter);
        let Modal::Password(d) = &r.modal else { panic!("the password modal") };
        assert_eq!(d.error.as_deref(), Some("the two do not match"));
        assert!(r.queued.is_none());
        let frame = draw(&mut r);
        assert!(frame.contains("New password for ben") && frame.contains("the two do not match"), "{frame}");
        assert!(frame.contains("•••••••") && !frame.contains("hunter"), "masked:\n{frame}");
        press(&mut r, KeyCode::Backspace);
        type_text(&mut r, "2");
        press(&mut r, KeyCode::Enter);
        assert_eq!(r.queued, Some(Op::Password { username: "ben".into(), password: "hunter2".into() }));
        r.queued = None;
        r.apply(Done::PasswordSet { username: "ben".into(), result: Ok(()) });
        assert!(matches!(r.modal, Modal::None));
        assert_eq!(note(&r), "ben: password set");
    }

    #[test]
    fn the_remove_gate_names_the_cascade_and_the_last_admin_and_y_confirms() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Down); // anna
        press(&mut r, KeyCode::Char('r'));
        let frame = draw(&mut r);
        assert!(frame.contains("Remove anna?"), "{frame}");
        assert!(frame.contains("playlists, play history and library grants") && frame.contains("The music files stay"), "{frame}");
        assert!(frame.contains("anna is the only admin"), "{frame}");
        assert!(frame.contains("◂ Keep") && frame.contains("y remove · Esc keep"), "{frame}");
        assert!(press(&mut r, KeyCode::Enter).is_none() && matches!(r.modal, Modal::None), "Enter keeps");
        press(&mut r, KeyCode::Down); // ben
        press(&mut r, KeyCode::Char('r'));
        let frame = draw(&mut r);
        assert!(frame.contains("Remove ben?") && !frame.contains("only admin"), "{frame}");
        press(&mut r, KeyCode::Char('y'));
        assert_eq!(r.queued, Some(Op::Remove("ben".into())));
        assert!(matches!(r.modal, Modal::None));
        r.queued = None;
        r.apply(Done::Removed { username: "ben".into(), result: Ok(()) });
        assert_eq!(note(&r), "ben removed");
        assert_eq!(r.queued, Some(Op::Load));
    }

    #[test]
    fn a_frame_lends_the_accounts_to_the_table_and_hands_them_back() {
        // Taken for the rows and put back rather than copied every frame
        // (performance audit #110): the table and the cursor's note still
        // read them.
        let _en = english();
        let mut r = loaded();
        let before = r.users.clone();
        handle_key(&mut r, key(KeyCode::Down));
        let frame = draw(&mut r);
        assert!(row(&frame, "anna").contains("[✓]") && frame.contains("guest"), "{frame}");
        assert_eq!(r.users, before, "the accounts are back after the frame");
    }

    /// The buffer as text, one line per row, for failure messages.
    fn shown(buf: &ratatui::buffer::Buffer) -> String {
        (buf.area.top()..buf.area.bottom()).map(|y| buf_row(buf, y) + "\n").collect()
    }

    /// Row `y` from column `x` on.
    fn from(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> String {
        buf_row(buf, y).chars().skip(x as usize).collect()
    }

    /// Whether a modal frame's rounded corners sit exactly on `rect`.
    fn framed_at(buf: &ratatui::buffer::Buffer, rect: Rect) -> bool {
        buf[(rect.x, rect.y)].symbol() == "╭" && buf[(rect.right() - 1, rect.bottom() - 1)].symbol() == "╯"
    }

    use super::super::super::hosting::{DOCKED, FLOOR, WINDOW, draw_hosted, outside, row as buf_row};
    use super::super::super::{Claim, HostedRoom};

    #[test]
    fn hosted_the_room_draws_inside_its_area_and_its_row_note_takes_the_last_row() {
        let _en = english();
        let mut r = loaded();
        r.note = None;
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Down);
        let buf = draw_hosted(&mut r, (100, 30), WINDOW);
        let text = shown(&buf);
        assert!(outside(&buf, WINDOW, true).is_empty(), "drawn outside its area:\n{text}");
        assert!(from(&buf, WINDOW.x, WINDOW.y).chars().all(|c| c == '·'), "the area's first row is the host's:\n{text}");
        assert!(!text.contains("home.mstream.example"), "no header:\n{text}");
        assert!(from(&buf, 19, 2).starts_with("• 3 users — 1 admin · logins on"), "the state line opens the body:\n{text}");
        assert!(
            from(&buf, 19, 23).starts_with("ben sees music · field-recordings · 1–5 flip a flag, saved at once"),
            "the cursor row's note on the last row:\n{text}"
        );
        assert!(!text.contains("Esc deselect"), "the tips are the host's:\n{text}");
        let buf = draw_hosted(&mut r, (176, 46), DOCKED);
        assert!(outside(&buf, DOCKED, true).is_empty(), "drawn outside its area:\n{}", shown(&buf));
        assert!(from(&buf, 19, DOCKED.bottom() - 1).starts_with("ben sees music"), "{}", shown(&buf));
    }

    #[test]
    fn hosted_public_mode_wraps_its_sentences_and_says_the_player_signs_in_too() {
        let _en = english();
        let mut r = room();
        r.queued = None;
        r.apply(Done::Loaded(Ok((BTreeMap::new(), libs()))));
        // At 100 columns the column is 79 cells: the last sentence takes two
        // rows rather than lose its end, and says what the player must do,
        // since the session this room makes is not the player's.
        let buf = draw_hosted(&mut r, (100, 30), WINDOW);
        let text = shown(&buf);
        assert!(outside(&buf, WINDOW, true).is_empty(), "{text}");
        let rows: Vec<String> = (13..22).map(|y| from(&buf, 19, y).trim_end_matches('·').trim_end().to_string()).collect();
        assert_eq!(rows[0], t!("usr.public_1"), "{text}");
        assert_eq!(rows[1], t!("usr.public_2"), "{text}");
        assert_eq!(rows[2], "", "{text}");
        let last: Vec<&str> = rows[3..].iter().map(String::as_str).take_while(|row| !row.is_empty()).collect();
        assert_eq!(last.len(), 2, "{text}");
        assert_eq!(last.join(" "), t!("usr.public_3_hosted"), "every word:\n{text}");
        assert!(rows.iter().all(|row| kit::width(row) <= 79), "{text}");
        assert!(!text.contains(&*t!("usr.public_3")), "not the hub's promise that the panel stays open:\n{text}");
    }

    #[test]
    fn hosted_the_add_form_centres_in_the_area_and_clamps_at_the_floor() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Char('a'));
        let buf = draw_hosted(&mut r, (100, 30), WINDOW);
        let at = kit::modal_rect(WINDOW, 84, 22, 22);
        assert!(framed_at(&buf, at), "the add form at {at:?}:\n{}", shown(&buf));
        assert!(outside(&buf, WINDOW, true).is_empty(), "{}", shown(&buf));
        // Seventeen rows: the form shrinks to the area and its rows that no
        // longer fit are left out, not drawn past its frame.
        let buf = draw_hosted(&mut r, (100, 24), FLOOR);
        let at = kit::modal_rect(FLOOR, 84, 22, 22);
        assert_eq!(at.height, FLOOR.height - 2);
        assert!(framed_at(&buf, at), "the add form at {at:?}:\n{}", shown(&buf));
        assert!(outside(&buf, FLOOR, true).is_empty(), "{}", shown(&buf));
        assert!(!shown(&buf).contains(&*t!("resize")));
        // The room's other modals stay inside the area at both sizes too.
        for act in [Act::Libraries("ben".into()), Act::Password("ben".into()), Act::Remove("ben".into())] {
            let mut r = loaded();
            r.libraries.extend((0..20).map(|i| format!("lib-{i}")));
            r.act(act);
            for (size, area) in [((100, 30), WINDOW), ((100, 24), FLOOR)] {
                let buf = draw_hosted(&mut r, size, area);
                assert!(outside(&buf, area, area == WINDOW).is_empty(), "{:?} spilled:\n{}", r.modal, shown(&buf));
            }
        }
    }

    #[test]
    fn hosted_digits_still_flip_flags() {
        let _en = english();
        let mut r = loaded();
        r.note = None;
        assert_eq!(r.claims(), Claim::Open, "the host routes the room's keys to it");
        r.press(key(KeyCode::Down));
        r.press(key(KeyCode::Down)); // ben
        assert!(r.press(key(KeyCode::Char('1'))).is_none());
        assert_eq!(
            r.queued,
            Some(Op::Access {
                username: "ben".into(),
                access: UserAccess { admin: true, allow_mkdir: true, allow_upload: false, allow_file_modify: true, allow_server_audio: false },
            })
        );
        // Esc stows the cursor, then hands the keyboard back.
        assert!(r.press(key(KeyCode::Esc)).is_none());
        assert_eq!(r.press(key(KeyCode::Esc)), Some(Outcome::Quit));
    }

    #[test]
    fn hosted_the_hint_says_esc_back_not_q_quit() {
        let _en = english();
        let mut r = loaded();
        assert_eq!(r.tips(), "↑ ↓ select · a add a user · Esc back");
        assert!(footer_hint_as(&r, false).ends_with("q quit"), "standalone keeps its own words");
        r.users.clear();
        assert_eq!(r.tips(), "a add the first user · Esc back");
        assert!(footer_hint_as(&r, false).ends_with("q quit"));
        press(&mut r, KeyCode::Char('a'));
        assert!(r.modal_up());
        assert_eq!(r.claims(), Claim::All, "a form holds every key");
        assert_eq!(r.tips(), t!("usr.hint_add"));
    }

    /// The cell a focused field notes is the caret's, masked or not, however
    /// the field's window over a long value stands (hosted, it is where the
    /// GUI's window floats the input method's candidates).
    #[test]
    fn the_password_and_the_username_note_the_cell_their_caret_is_drawn_in() {
        let _en = english();
        let mut room = loaded();
        room.act(Act::Password("ben".into()));
        type_text(&mut room, &"s".repeat(60));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "•••", "");
        press(&mut room, KeyCode::Home);
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "", "•••");
        press(&mut room, KeyCode::Esc);

        press(&mut room, KeyCode::Char('a'));
        type_text(&mut room, &format!("{}-end", "u".repeat(50)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "uuu-end", "");
    }
}
