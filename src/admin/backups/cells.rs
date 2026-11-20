//! The Backups room in character cells: what `mstream-player admin
//! backups` draws in a terminal, and what the GUI player's Admin tab
//! draws on its grid ([`Screen::render_hosted`]). The room itself (its
//! state, its worker, its keys, its words) is the parent module; this is
//! its view in cells, and holds what is measured in cells: the state
//! line and its scan bar, the add card, the table and the four modals, on
//! the kit's surface. It is also where the room meets the hub's loop, as
//! a [`Screen`].

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::super::{Screen, age_text, body_column, draw_chip, draw_foot, draw_header, frame_ground, host_of, iso_unix, unix_now};
use super::*;
use crate::api::types::ActiveBackup;
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::{ellipsize_path_start, g};

const MIN_W: u16 = 80;
const MIN_H: u16 = 24;

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

    /// The room's own tips already end on "Esc back", so hosted they are
    /// the same words.
    fn hint(&self) -> String {
        footer_hint(self)
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
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.cells.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    draw(frame, room, area, false);
}

/// The room inside `area`. Standalone it sits under its own header, the
/// BETA chip beside the title, and over its tips row. Hosted (the GUI
/// player's Admin tab) the host's bar is the header, so the chip moves to
/// the right of the body's first row, beside the state line, and the
/// host's footer carries the tips, leaving the area's last row to the
/// status line alone. Modals and the tooltip centre in `area` either way.
fn draw(frame: &mut Frame, room: &mut Room, area: Rect, hosted: bool) {
    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.cells.ui.pointer;
    if modal_open {
        room.cells.ui.pointer = None;
    }

    let beta = t!("bak.beta").to_string();
    if !hosted {
        let title = t!("bak.title").to_string();
        draw_header(frame, area, &title, &host_of(&room.client));
        // The page's BETA banner, as a chip beside the title.
        frame.render_widget(
            Paragraph::new(Span::styled(beta.clone(), Style::default().fg(th().gold))),
            // In cells, so a Japanese title is not drawn over.
            Rect { x: area.x + 2 + kit::width(&title) as u16 + 1, y: area.y, width: 8, height: 1 },
        );
    }
    let column = body_column(area, hosted);
    // Hosted, the chip takes the right of the state line's row, so the
    // cadence keeps clear of it by the chip and a two-cell gap.
    let reserve = if hosted { kit::width(&beta) as u16 + 2 } else { 0 };
    if room.loaded {
        draw_state(frame, room, column, reserve);
        draw_table(frame, room, column);
    }
    if hosted {
        draw_chip(frame, column, &beta);
    }
    let tips = footer_hint(room);
    draw_foot(frame, area, room.note.as_ref(), room.busy.as_deref(), &tips, hosted);

    if modal_open {
        room.cells.ui.pointer = live_pointer;
        room.cells.ui.clear_registries();
    }
    // Lent to its draw and put back, not copied every frame: the folder
    // browser holds a server listing, the history up to fifty runs
    // (performance audit #110). No modal's draw touches the modal.
    let modal = std::mem::replace(&mut room.modal, Modal::None);
    match &modal {
        Modal::None => {}
        Modal::Form(f) => draw_form(frame, room, area, f),
        Modal::Browser { browse, .. } => draw_browser(frame, room, area, browse),
        Modal::History { dest, runs, loaded, sel } => draw_history(frame, room, area, dest, runs, *loaded, *sel),
        &Modal::Remove(id) => {
            let (lib, path) = room
                .dest(id)
                .map(|d| (printable(&d.library_name, 64), d.dest_path.clone()))
                .unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("bak.remove_title", library = lib, path = path).to_string(),
                vec![t!("bak.remove_1").to_string(), t!("bak.remove_2").to_string()],
                (t!("bak.remove_keep").to_string(), Act::RemoveCancel),
                (t!("bak.remove_confirm").to_string(), Act::RemoveConfirm),
            );
        }
    }
    room.modal = modal;
    if let Some((target, text)) = room.cells.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// The state line: the run in flight with the kit's scan bar, the queue,
/// or idle. The cadence on its right stops `reserve` cells short of the
/// column's edge, where a hosted room's chip sits.
fn draw_state(frame: &mut Frame, room: &mut Room, column: Rect, reserve: u16) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let now = unix_now();
    let (spans, polls) = match &room.status.active {
        Some(a) => {
            let elapsed = a.started_at.as_deref().and_then(iso_unix).map(|t| elapsed_text(now - t)).unwrap_or_default();
            let trigger = a.trigger_reason.as_deref().map(trigger_words).unwrap_or_default();
            (
                vec![
                    Span::styled(t!("bak.state_running").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
                    Span::raw(
                        t!(
                            "bak.state_running_detail",
                            library = printable(a.library_name.as_deref().unwrap_or(""), 64),
                            path = a.dest_path.clone().unwrap_or_default(),
                            trigger = trigger,
                            elapsed = elapsed
                        )
                        .to_string(),
                    ),
                ],
                t!("bak.polls_2"),
            )
        }
        None if room.status.queue_length > 0 => (
            vec![
                Span::styled(
                    if room.status.queue_length == 1 { t!("bak.state_queued_one") } else { t!("bak.state_queued", n = room.status.queue_length) }.to_string(),
                    Style::default().fg(th().gold).add_modifier(Modifier::BOLD),
                ),
                Span::raw(t!("bak.state_queued_detail").to_string()),
            ],
            t!("bak.polls_2"),
        ),
        None => {
            let on = room.dests.iter().filter(|d| d.enabled).count();
            (
                vec![
                    Span::styled(t!("bak.state_idle").to_string(), dim()),
                    Span::raw(t!("bak.state_idle_detail", on = on, total = room.dests.len()).to_string()),
                ],
                t!("bak.polls_5"),
            )
        }
    };
    let state_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    frame.render_widget(Paragraph::new(Line::from(spans)), line(column.y));
    let polls = polls.to_string();
    let room_for = column.width.saturating_sub(reserve);
    if state_w + 2 + polls.chars().count() <= room_for as usize {
        let at = Rect { width: room_for, ..line(column.y) };
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), at);
    }
    if let Some(a) = &room.status.active {
        frame.render_widget(Paragraph::new(progress_line(a, column.width as usize)), line(column.y + 1));
    }
}

/// The kit's scan bar for a run: ten cells, the percentage, the counts —
/// or the all-dim bar and "no estimate yet" on a destination's first run.
/// The count groups drop off the right when the column is narrow.
fn progress_line(a: &ActiveBackup, width: usize) -> Line<'static> {
    let done = a.files_copied + a.files_unchanged + a.files_trashed;
    let mut spans = Vec::new();
    match a.expected_files.filter(|e| *e > 0) {
        Some(expected) => {
            let pct = ((done * 100) / expected).min(100);
            let filled = (pct / 10) as usize;
            spans.push(Span::styled("▰".repeat(filled), Style::default().fg(th().ok)));
            spans.push(Span::styled("▱".repeat(10 - filled), dim()));
            spans.push(Span::styled(
                t!("bak.progress_pct", pct = pct, done = fmt_count(done), expected = fmt_count(expected)).to_string(),
                dim(),
            ));
        }
        None => {
            spans.push(Span::styled("▱".repeat(10), dim()));
            spans.push(Span::styled(t!("bak.progress_no_estimate").to_string(), dim()));
        }
    }
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
    let head: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    while !groups.is_empty() && head + groups.iter().map(|g| g.chars().count() + 3).sum::<usize>() > width {
        groups.pop();
    }
    for group in groups {
        spans.push(Span::styled(format!(" · {group}"), dim()));
    }
    Line::from(spans)
}

/// The add card, then the destinations table.
fn draw_table(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let card = Rect { x: column.x, y: column.y + 2, width: column.width, height: 3 };
    let hover = room.cells.ui.pointer.is_some_and(|p| card.contains(p));
    let color = if hover { th().bright } else { th().ok };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("bak.add_card").to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
            .alignment(Alignment::Center),
        inner,
    );
    room.cells.ui.click(card, Act::Add);

    let table = Rect { x: column.x, y: column.y + 6, width: column.width, height: column.height.saturating_sub(6) };
    if table.height < 3 {
        return;
    }
    let widest = |words: &[String]| words.iter().map(|w| w.chars().count() as u16).max().unwrap_or(0);
    let trig_w = widest(&[
        t!("bak.col_trigger").to_string(),
        t!("bak.trig_after_scan").to_string(),
        t!("bak.trig_daily_at", hour = "23").to_string(),
        t!("bak.trig_manual").to_string(),
    ])
    .clamp(12, 18);
    let ret_w = widest(&[t!("bak.col_retention").to_string(), t!("bak.no_trash").to_string()]).clamp(9, 14);
    let (lib_w, last_w, on_w) = (10u16, 18u16, 3u16);
    let fixed = lib_w + 2 + trig_w + 2 + ret_w + 2 + last_w + 2 + on_w + 2;
    let dest_w = table.width.saturating_sub(fixed).max(12);
    let dest_x = table.x + lib_w + 2;
    let trig_x = dest_x + dest_w + 2;
    let ret_x = trig_x + trig_w + 2;
    let last_x = ret_x + ret_w + 2;
    let on_x = last_x + last_w + 2;
    let head = |x: u16, w: u16| Rect { x, y: table.y, width: w, height: 1 };
    for (x, w, word) in [
        (table.x, lib_w, t!("bak.col_library")),
        (dest_x, dest_w, t!("bak.col_destination")),
        (trig_x, trig_w, t!("bak.col_trigger")),
        (ret_x, ret_w, t!("bak.col_retention")),
        (last_x, last_w, t!("bak.col_last_run")),
        (on_x, on_w, t!("bak.col_on")),
    ] {
        frame.render_widget(Paragraph::new(Span::styled(word.to_string(), dim())), head(x, w));
    }
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(table.width as usize), dim())), line(table.y + 1));
    let rows_y = table.y + 2;
    if room.dests.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.empty").to_string(), dim())), line(rows_y));
        return;
    }
    let avail = table.bottom().saturating_sub(rows_y) as usize;
    let sel_moved = room.sel != room.cells.sel_anchor;
    room.cells.sel_anchor = room.sel;
    let reveal = if sel_moved { room.sel } else { None };
    let (first, visible) = kit::table_view(room.dests.len(), reveal, room.tscroll, avail);
    room.tscroll = first;
    let now = unix_now();
    let running = room.running_id();
    let pct = room.status.active.as_ref().and_then(|a| {
        a.expected_files.filter(|e| *e > 0).map(|e| ((a.files_copied + a.files_unchanged + a.files_trashed) * 100 / e).min(100))
    });
    let dests = room.dests.clone();
    for (row, i) in (first..first + visible).enumerate() {
        let d = &dests[i];
        let y = rows_y + row as u16;
        let rect = line(y);
        let selected = room.sel == Some(i);
        let hovered = !selected && room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let base = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        let faint = if selected || hovered { base } else { dim() };
        if selected {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(table.width as usize), base)), rect);
        }
        let cell = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(
                clip(&printable(&d.library_name, 64), lib_w),
                if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base },
            )),
            cell(table.x, lib_w),
        );
        let (shown, clipped) = ellipsize_path_start(&d.dest_path, dest_w as usize);
        frame.render_widget(Paragraph::new(Span::styled(shown, base)), cell(dest_x, dest_w));
        if clipped {
            room.cells.ui.tip(cell(dest_x, dest_w), d.dest_path.clone());
        }
        frame.render_widget(Paragraph::new(Span::styled(clip(&trigger_cell(d), trig_w), base)), cell(trig_x, trig_w));
        let retention = if d.retention_days == 0 { (t!("bak.no_trash").to_string(), faint) } else { (t!("bak.days", n = d.retention_days).to_string(), base) };
        frame.render_widget(Paragraph::new(Span::styled(retention.0, retention.1)), cell(ret_x, ret_w));
        let (last, own) = if running == Some(d.id) {
            (
                match pct {
                    Some(p) => t!("bak.running_pct", pct = p).to_string(),
                    None => t!("bak.running").to_string(),
                },
                accent(),
            )
        } else {
            match &d.last_run {
                None => (t!("bak.never").to_string(), dim()),
                Some(run) => {
                    let age = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
                    (format!("{} · {}", status_word(&run.status), age), status_style(&run.status))
                }
            }
        };
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&last, last_w), if selected || hovered { base } else { own })),
            cell(last_x, last_w),
        );
        // ON: the checkbox glyph wearing the state, clickable like every
        // other control, s on the cursor row by key.
        let on_rect = cell(on_x, on_w);
        let on_hover = room.cells.ui.pointer.is_some_and(|p| on_rect.contains(p));
        let glyph = if d.enabled { g("[✓]", "[x]") } else { "[ ]" };
        let on_style = match (selected, on_hover, d.enabled) {
            (true, _, _) => base,
            (false, true, _) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
            (false, false, true) => Style::default().fg(th().ok),
            (false, false, false) => dim(),
        };
        frame.render_widget(Paragraph::new(Span::styled(glyph, on_style)), on_rect);
        room.cells.ui.click(on_rect, Act::ToggleEnabled(d.id));
        room.cells.ui.click(cell(table.x, on_x.saturating_sub(table.x + 1)), Act::Select(i));
    }
    let bar = Rect { x: table.x + table.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut room.cells.ui, bar, dests.len(), visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);

    // The cursor row's extras ride the note line, when it is free.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(d) = room.selected()
    {
        let mut spans = vec![Span::raw(d.dest_path.clone())];
        if d.inter_file_delay_ms > 0 {
            spans.push(Span::raw(format!(" · {}", t!("bak.note_throttle", ms = d.inter_file_delay_ms))));
        }
        let excludes = if d.exclude_globs == room.defaults {
            t!("bak.note_default_excludes").to_string()
        } else if d.exclude_globs.is_empty() {
            t!("bak.note_no_excludes").to_string()
        } else {
            t!("bak.note_patterns", n = d.exclude_globs.len()).to_string()
        };
        spans.push(Span::raw(format!(" · {excludes}")));
        if let Some(run) = &d.last_run {
            match run.status.as_str() {
                "failed" | "partial" => {
                    let why = printable(run.error_message.as_deref().unwrap_or(""), 200);
                    spans.push(Span::styled(format!(" · {}: {why}", status_word(&run.status)), Style::default().fg(th().gold)));
                }
                _ => spans.push(Span::styled(format!(" · {}", run_summary(run)), dim())),
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), line(column.bottom() + 1));
    }
}

// ── Modals ───────────────────────────────────────────────────────────────────

/// A labelled 3-row input inside a modal.
#[allow(clippy::too_many_arguments)]
fn modal_field(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, muted: bool, act: Act) {
    modal_field_labelled(frame, room, at, at.width, label, input, focused, muted, act);
}

/// A labelled field whose label may run wider than its card (the number
/// cards sit side by side with their words in the gaps).
#[allow(clippy::too_many_arguments)]
fn modal_field_labelled(frame: &mut Frame, room: &mut Room, at: Rect, label_w: u16, label: &str, input: &Input, focused: bool, muted: bool, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: label_w, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = !muted && room.cells.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let w = inner.width.saturating_sub(2);
    let shown = if focused {
        kit::field_display(&mut room.cells.ui, inner.x + 1, inner.y, input.value(), input.cursor(), w, None)
    } else {
        clip_tail(input.value(), w)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(shown, if muted { dim() } else { Style::default() })),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 },
    );
    if !muted {
        room.cells.ui.click(field, act);
    }
}

/// A radio choice row: `(•) name` per option, wrapping to the width. Each
/// option's rect is its name's width in cells and the glyph's four, as the
/// form's `lib_rows` counts the rows this wraps to: a library named in
/// Japanese, counted by characters, got a rect short by half its name, so
/// the name was cut there, its click covered only what was left, and the
/// row wrapped later than it should have. An option wider than the room
/// left in its row (one that starts the row and still does not fit) is
/// clamped to the row, so its name is cut inside it and its click stays on
/// what is drawn, as setup's browser row and skip buttons are: measured in
/// cells, a name of thirty Japanese characters outgrows the 60-cell row,
/// and unclamped it painted over the modal's right border and past it.
/// `lib_rows` counts such an option as the one row it takes.
fn radio_row(frame: &mut Frame, room: &mut Room, at: Rect, options: &[String], chosen: usize, focused: bool, field_index: usize) -> u16 {
    let mut x = at.x;
    let mut y = at.y;
    for (i, name) in options.iter().enumerate() {
        let w = kit::width(name) as u16 + 4;
        if x + w > at.right() && x > at.x {
            x = at.x;
            y += 1;
        }
        let rect = Rect { x, y, width: w.min(at.right().saturating_sub(x)), height: 1 };
        let on = i == chosen;
        let hovered = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on && focused {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if on {
            accent()
        } else {
            dim()
        };
        let name_style = if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(if on { "(•)" } else { "( )" }, glyph_style), Span::raw(" "), Span::styled(name.clone(), name_style)])),
            rect,
        );
        room.cells.ui.click(rect, Act::FormPick(field_index, i));
        x += rect.width + 3;
    }
    y + 1 - at.y
}

/// The add and edit forms, one drawing.
fn draw_form(frame: &mut Frame, room: &mut Room, area: Rect, f: &Form) {
    let fields = f.fields();
    let focused = f.focused();
    let index_of = |field: Field| fields.iter().position(|x| *x == field).unwrap_or(0);
    let editing = f.editing.is_some();
    let library_names: Vec<String> = room.libraries.iter().map(|(n, _)| n.clone()).collect();
    // Rows the library line takes: a wrapped radio row while adding, its
    // options measured in cells as `radio_row` places them.
    let lib_rows = if editing {
        1
    } else {
        let width = 74u16.saturating_sub(4 + 10) as usize;
        let mut rows = 1;
        let mut x = 0;
        for name in &library_names {
            let w = kit::width(name) + 4;
            if x + w > width && x > 0 {
                rows += 1;
                x = 0;
            }
            x += w + 3;
        }
        rows
    };
    let height = 25 + lib_rows;
    let inner = kit::modal_frame(frame, area, 74, height, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    // A short window (the GUI's floor, or a terminal under the form's
    // height) clamps the frame, so a block draws only when its last row
    // clears the button row. At full height every block does, and the
    // error line shares the button row, left of the buttons.
    let by = inner.bottom().saturating_sub(1);
    let clear = |last: u16| last < by;
    let title = if editing {
        t!("bak.edit_title", library = f.library_name, path = f.original.as_ref().map(|d| d.dest_path.clone()).unwrap_or_default())
    } else {
        t!("bak.add_title")
    };
    frame.render_widget(
        Paragraph::new(Span::styled(clip(&title, w.saturating_sub(4)), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::FormCancel);
    let mut y = inner.y + 2;

    // LIBRARY
    let lib_shown = clear(y + lib_rows - 1);
    if lib_shown {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.field_library").to_string(), dim())), Rect { x, y, width: 10, height: 1 });
    }
    if !lib_shown {
        y += lib_rows + 1;
    } else if editing {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(f.library_name.clone(), bold()), Span::styled(t!("bak.library_fixed").to_string(), dim())])),
            Rect { x: x + 10, y, width: w.saturating_sub(10), height: 1 },
        );
        y += 2;
    } else if library_names.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.no_libraries").to_string(), dim())), Rect { x: x + 10, y, width: w.saturating_sub(10), height: 1 });
        y += 2;
    } else {
        let used = radio_row(
            frame,
            room,
            Rect { x: x + 10, y, width: w.saturating_sub(10), height: lib_rows },
            &library_names,
            f.library,
            focused == Field::Library,
            index_of(Field::Library),
        );
        y += used + 1;
    }

    // TRIGGER: the kit's radio group, one row per option with its words.
    if clear(y) {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.field_trigger").to_string(), dim())), Rect { x, y, width: 10, height: 1 });
    }
    for (i, trigger) in Trigger::ALL.iter().enumerate() {
        if !clear(y + i as u16) {
            break;
        }
        let rect = Rect { x: x + 10, y: y + i as u16, width: w.saturating_sub(10), height: 1 };
        let on = *trigger == f.trigger;
        let hovered = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on && focused == Field::Trigger {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if on {
            accent()
        } else {
            dim()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if on { "(•)" } else { "( )" }, glyph_style),
                Span::raw(" "),
                Span::styled(trigger.label(), if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() }),
                Span::styled(format!(" — {}", trigger.description()), dim()),
            ])),
            rect,
        );
        room.cells.ui.click(rect, Act::FormPick(index_of(Field::Trigger), i));
    }
    y += 4;

    // DESTINATION: the path, ^B to browse, the server's verdict beneath.
    if clear(y + 3) {
        modal_field(
            frame,
            room,
            Rect { x, y, width: w, height: 4 },
            &t!("bak.field_dest"),
            &f.dest,
            focused == Field::Dest,
            false,
            Act::FormFocus(index_of(Field::Dest)),
        );
        let browse = t!("bak.browse_button").to_string();
        let browse_w = kit::width(&browse) as u16 + 4;
        kit::button(frame, &mut room.cells.ui, Rect { x: inner.right().saturating_sub(browse_w + 1), y, width: browse_w, height: 1 }, &browse, false, Act::FormBrowse);
    }
    y += 4;
    let dest_text = f.dest.value().trim();
    let verdict: Option<(String, Style)> = if dest_text.is_empty() {
        None
    } else if f.checked_for != dest_text || f.check.is_none() {
        Some((t!("bak.checking").to_string(), dim()))
    } else if let Some(check) = &f.check {
        if let Some(err) = check.errors.first() {
            Some((format!("− {}", printable(err, 200)), Style::default().fg(th().gold)))
        } else if let Some(warn) = check.warnings.first() {
            let more = if check.warnings.len() > 1 { format!(" (+{})", check.warnings.len() - 1) } else { String::new() };
            Some((format!("− {}{more}", printable(warn, 200)), Style::default().fg(th().gold)))
        } else {
            Some((format!("{} {}", g("✓", "+"), t!("bak.check_ok")), Style::default().fg(th().ok)))
        }
    } else {
        None
    };
    // The server's sentence may run long: two rows, wrapped at words.
    if let Some((text, style)) = verdict
        && clear(y)
    {
        frame.render_widget(
            Paragraph::new(Span::styled(printable(&text, 2 * w as usize), style)).wrap(Wrap { trim: true }),
            Rect { x, y, width: w, height: 2.min(by - y) },
        );
    }
    y += 2;

    // The numbers, side by side; the hour muted unless the trigger is daily.
    let numbers: [(Field, String, String, &Input, bool); 3] = [
        (Field::Retention, t!("bak.field_retention").to_string(), t!("bak.hint_retention").to_string(), &f.retention, false),
        (Field::Throttle, t!("bak.field_throttle").to_string(), t!("bak.hint_throttle").to_string(), &f.throttle, false),
        (Field::Hour, t!("bak.field_hour").to_string(), t!("bak.hint_hour").to_string(), &f.hour, f.trigger != Trigger::Daily),
    ];
    for (i, (field, label, hint, input, muted)) in numbers.iter().enumerate() {
        if !clear(y + 3) {
            break;
        }
        let fx = x + i as u16 * 21;
        let room_w = w.saturating_sub(i as u16 * 21);
        modal_field_labelled(
            frame,
            room,
            Rect { x: fx, y, width: 12.min(room_w), height: 4 },
            20.min(room_w),
            label,
            input,
            focused == *field,
            *muted,
            Act::FormFocus(index_of(*field)),
        );
        if clear(y + 4) {
            frame.render_widget(Paragraph::new(Span::styled(hint.clone(), dim())), Rect { x: fx, y: y + 4, width: 20.min(room_w), height: 1 });
        }
    }
    y += 5;

    // EXCLUDE PATTERNS
    if clear(y + 3) {
        modal_field(
            frame,
            room,
            Rect { x, y, width: w, height: 4 },
            &t!("bak.field_excludes"),
            &f.excludes,
            focused == Field::Excludes,
            false,
            Act::FormFocus(index_of(Field::Excludes)),
        );
    }
    y += 4;
    if let Some(err) = &f.error
        && y <= by
    {
        frame.render_widget(Paragraph::new(Span::styled(clip(err, w), Style::default().fg(th().gold))), line(y));
    }
    if editing {
        kit::button(frame, &mut room.cells.ui, Rect { x, y: by, width: w, height: 1 }, &t!("bak.reset_patterns"), false, Act::FormResetExcludes);
    }
    let label = if editing { t!("bak.save") } else { t!("bak.add_submit") }.to_string();
    let bx = inner.right().saturating_sub(kit::width(&label) as u16 + 4);
    kit::button(frame, &mut room.cells.ui, Rect { x: bx, y: by, width: inner.width, height: 1 }, &label, true, Act::FormSubmit);
}

/// The server's folder browser, over the form.
fn draw_browser(frame: &mut Frame, room: &mut Room, area: Rect, browse: &Browse) {
    let inner = kit::modal_frame(frame, area, 66, 18, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("bak.browser_title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::BrowseCancel);
    frame.render_widget(
        Paragraph::new(Span::styled(browse.path.clone(), dim())),
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
    );
    let list_top = inner.y + 3;
    let visible = inner.height.saturating_sub(5) as usize;
    let first = browse.sel.saturating_sub(visible.saturating_sub(1));
    for (row, i) in (first..browse.dirs.len().min(first + visible)).enumerate() {
        let selected = i == browse.sel;
        let style = if selected { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default() };
        let rect = Rect { x: inner.x, y: list_top + row as u16, width: inner.width, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(format!("{} {}", g("▸", "►"), browse.dirs[i]), style)), rect);
        room.cells.ui.click(rect, Act::BrowseRow(i));
    }
    if browse.dirs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("browse.empty").to_string(), dim())),
            Rect { x: inner.x, y: list_top, width: inner.width, height: 1 },
        );
    }
    let y = inner.bottom().saturating_sub(1);
    // Each button gets the room left before the modal's right edge, so the
    // kit cuts the last label inside the frame instead of drawing it (and
    // its click rect) over the border when a CJK row outgrows the modal.
    let row = |x: u16| Rect { x, y, width: inner.right().saturating_sub(x), height: 1 };
    let up = kit::button(frame, &mut room.cells.ui, row(inner.x), &t!("browse.up"), false, Act::BrowseUp);
    let open = kit::button(frame, &mut room.cells.ui, row(up.right() + 1), &t!("browse.open"), false, Act::BrowseEnter);
    let choose = kit::button(frame, &mut room.cells.ui, row(open.right() + 1), &t!("bak.browse_use"), true, Act::BrowseChoose);
    kit::button(frame, &mut room.cells.ui, row(choose.right() + 1), &t!("browse.close"), false, Act::BrowseCancel);
}

/// A destination's runs, newest first; the selected run's notes beneath.
fn draw_history(frame: &mut Frame, room: &mut Room, area: Rect, dest: &BackupDestination, runs: &[BackupRun], loaded: bool, sel: usize) {
    let inner = kit::modal_frame(frame, area, 84, 21, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    frame.render_widget(
        Paragraph::new(Span::styled(
            clip(&t!("bak.history_title", library = printable(&dest.library_name, 64), path = dest.dest_path), w.saturating_sub(4)),
            Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        )),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::HistoryClose);
    if !loaded {
        return;
    }
    if runs.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.history_empty").to_string(), dim())), line(inner.y + 2));
        return;
    }
    let (started_w, status_w, trig_w, count_w, bytes_w) = (9u16, 8u16, 15u16, 9u16, 9u16);
    let started_x = x;
    let status_x = started_x + started_w + 2;
    let trig_x = status_x + status_w + 2;
    let copied_x = trig_x + trig_w + 2;
    let unch_x = copied_x + count_w + 2;
    let trash_x = unch_x + count_w + 2;
    let bytes_x = trash_x + count_w + 2;
    let hy = inner.y + 2;
    let head = |x: u16, w: u16, word: String, right: bool| {
        let p = Paragraph::new(Span::styled(word, dim()));
        (Rect { x, y: hy, width: w, height: 1 }, if right { p.alignment(Alignment::Right) } else { p })
    };
    for (rect, p) in [
        head(started_x, started_w, t!("bak.hcol_started").to_string(), false),
        head(status_x, status_w, t!("bak.hcol_status").to_string(), false),
        head(trig_x, trig_w, t!("bak.hcol_trigger").to_string(), false),
        head(copied_x, count_w, t!("bak.hcol_copied").to_string(), true),
        head(unch_x, count_w, t!("bak.hcol_unchanged").to_string(), true),
        head(trash_x, count_w, t!("bak.hcol_trashed").to_string(), true),
        head(bytes_x, bytes_w, t!("bak.hcol_bytes").to_string(), true),
    ] {
        frame.render_widget(p, rect);
    }
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(w as usize), dim())), line(hy + 1));
    let rows_y = hy + 2;
    let visible = 8usize.min(runs.len());
    let first = sel.saturating_sub(visible.saturating_sub(1)).min(runs.len().saturating_sub(visible));
    let now = unix_now();
    for (row, i) in (first..first + visible).enumerate() {
        let run = &runs[i];
        let y = rows_y + row as u16;
        let selected = i == sel;
        let base = if selected { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default() };
        let rect = line(y);
        if selected {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(w as usize), base)), rect);
        }
        let cell = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
        let started = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
        frame.render_widget(Paragraph::new(Span::styled(started, base)), cell(started_x, started_w));
        frame.render_widget(
            Paragraph::new(Span::styled(status_word(&run.status), if selected { base } else { status_style(&run.status) })),
            cell(status_x, status_w),
        );
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&trigger_words(run.trigger_reason.as_deref().unwrap_or("")), trig_w), base)),
            cell(trig_x, trig_w),
        );
        for (cx, n) in [(copied_x, run.files_copied), (unch_x, run.files_unchanged), (trash_x, run.files_trashed)] {
            frame.render_widget(Paragraph::new(Span::styled(fmt_count(n), base)).alignment(Alignment::Right), cell(cx, count_w));
        }
        let bytes = if run.bytes_copied > 0 { (fmt_bytes(run.bytes_copied), base) } else { ("—".to_string(), if selected { base } else { dim() }) };
        frame.render_widget(Paragraph::new(Span::styled(bytes.0, bytes.1)).alignment(Alignment::Right), cell(bytes_x, bytes_w));
        room.cells.ui.click(rect, Act::HistoryPick(i));
    }
    let after = rows_y + visible as u16;
    frame.render_widget(Paragraph::new(Span::styled(t!("bak.history_note", n = HISTORY_LIMIT).to_string(), dim())), line(after + 1));
    if let Some(run) = runs.get(sel) {
        let started = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
        let duration = match (iso_unix(&run.started_at), run.finished_at.as_deref().and_then(iso_unix)) {
            (Some(a), Some(b)) if b >= a => elapsed_text(b - a),
            _ => String::new(),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{started} · {}", status_word(&run.status)), status_style(&run.status).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" · {} · {duration}", trigger_words(run.trigger_reason.as_deref().unwrap_or(""))), dim()),
            ])),
            line(after + 3),
        );
        let notes = match run.error_message.as_deref().filter(|m| !m.trim().is_empty()) {
            Some(m) => printable(m, 400),
            None => run_summary(run),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(notes, if run.status == "success" { dim() } else { Style::default().fg(th().gold) })).wrap(Wrap { trim: false }),
            Rect { x, y: after + 4, width: w, height: inner.bottom().saturating_sub(after + 4) },
        );
    }
}

/// A warning gate: gold, consequences before verbs, the safe choice as
/// the primary, no [X].
fn draw_gate(frame: &mut Frame, room: &mut Room, area: Rect, title: String, body: Vec<String>, safe: (String, Act), go: (String, Act)) {
    let inner = kit::modal_frame(frame, area, 68, 8 + body.len() as u16, th().gold);
    let gold = Style::default().fg(th().gold);
    let mut lines = vec![Line::from(Span::styled(title, gold.add_modifier(Modifier::BOLD))), Line::from("")];
    lines.extend(body.into_iter().map(|b| Line::from(Span::styled(b, gold))));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) },
    );
    let y = inner.bottom().saturating_sub(1);
    let go_w = kit::width(&go.0) as u16 + 4;
    let safe_w = kit::width(&safe.0) as u16 + 4;
    let safe_x = inner.right().saturating_sub(go_w + 2 + safe_w);
    let safe_rect = kit::button(frame, &mut room.cells.ui, Rect { x: safe_x, y, width: inner.width, height: 1 }, &safe.0, true, safe.1);
    kit::button(frame, &mut room.cells.ui, Rect { x: safe_rect.right() + 2, y, width: inner.width, height: 1 }, &go.0, false, go.1);
}

// ── Colours ──────────────────────────────────────────────────────────────────

/// success green; failed, partial and skipped gold; running the accent.
fn status_style(status: &str) -> Style {
    match status {
        "success" => Style::default().fg(th().ok),
        "failed" | "partial" | "skipped" => Style::default().fg(th().gold),
        "running" => accent(),
        _ => dim(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::tests::*;
    use super::*;
    use crate::api::types::PathCheckInfo;

    /// Render one frame and flatten the buffer to text for assertions.
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

    fn row(frame: &str, name: &str) -> String {
        frame.lines().find(|l| l.starts_with(&format!("  {name}"))).map(str::to_string).unwrap_or_default()
    }

    /// The add form's library radio measures its names in cells, and the
    /// form's height counts the rows it wraps to the same way: three
    /// libraries named in Japanese, ten characters (twenty cells) each, in
    /// the 60-cell row. Two options of 24 cells and a gap take 51, so the
    /// third wraps under the first; each option's click rect is its drawn
    /// width and shows its whole name, and the trigger label sits under the
    /// second row. Counted by characters all three shared one row, each cut
    /// at half its name.
    #[test]
    fn the_add_forms_library_radio_wraps_japanese_names_by_their_cells() {
        let _en = english();
        let names = ["日本の音楽アーカイブ", "クラシック全集の録音", "ジャズのレコード盤集"];
        let mut room = new_room();
        room.apply(Done::Platform(Ok(defaults())));
        room.queued = None;
        room.busy = None;
        let libraries = names.iter().zip(1..).map(|(n, id)| (n.to_string(), id * 10)).collect();
        room.apply(Done::Loaded(Ok(Box::new(Loaded { dests: Vec::new(), status: BackupStatus::default(), libraries: Some(libraries) }))));
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        let library = f.fields().iter().position(|x| *x == Field::Library).expect("the library field");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut room)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let option = |i: usize| {
            let act = Act::FormPick(library, i);
            room.cells.ui.clicks.iter().find(|(_, a)| *a == act).map(|(rect, _)| *rect).unwrap_or_else(|| panic!("option {i}"))
        };
        let rects: Vec<Rect> = (0..3).map(option).collect();
        assert_eq!(rects[1].y, rects[0].y, "the second shares the first's row");
        assert_eq!(rects[1].x, rects[0].right() + 3, "three cells past it");
        assert_eq!((rects[2].x, rects[2].y), (rects[0].x, rects[0].y + 1), "the third wraps under the first");
        for (i, rect) in rects.iter().enumerate() {
            assert_eq!(rect.width, 24, "option {i}: its name's 20 cells and the glyph's four");
            let mut text = String::new();
            let mut x = rect.x;
            while x < rect.right() {
                let symbol = buffer[(x, rect.y)].symbol();
                text.push_str(symbol);
                x += (kit::width(symbol) as u16).max(1);
            }
            let glyph = if i == 0 { "(•)" } else { "( )" };
            assert_eq!(text, format!("{glyph} {}", names[i]), "option {i}: its whole name");
        }
        // lib_rows counted the same two rows: TRIGGER starts a blank row on.
        let trigger_y = rects[2].y + 2;
        let label: String = (0..buffer.area.width).map(|x| buffer[(x, trigger_y)].symbol()).collect();
        assert!(label.contains(&*t!("bak.field_trigger")), "the trigger under the wrapped row: {label:?}");
    }

    /// An option wider than the whole row is clamped to it: a library named
    /// by thirty Japanese characters (60 cells) wants 64 of the add form's
    /// 60-cell row at 100×30, which runs from x 25 to 85, the modal's border
    /// at 86. The option starts the row and ends at its end, the padding
    /// cell and the border past it survive, its click rect is the drawn
    /// rect, and its name is cut cleanly; the next library wraps under it,
    /// and the trigger label sits where lib_rows put it. Unclamped, the
    /// option painted over the border and its click reached past the modal.
    #[test]
    fn the_add_forms_library_radio_clamps_an_option_wider_than_the_row() {
        let _en = english();
        let long = "日本の音楽アーカイブ".repeat(3);
        assert_eq!(kit::width(&long), 60);
        let mut room = new_room();
        room.apply(Done::Platform(Ok(defaults())));
        room.queued = None;
        room.busy = None;
        let libraries = vec![(long.clone(), 10), ("music".to_string(), 20)];
        room.apply(Done::Loaded(Ok(Box::new(Loaded { dests: Vec::new(), status: BackupStatus::default(), libraries: Some(libraries) }))));
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        let library = f.fields().iter().position(|x| *x == Field::Library).expect("the library field");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut room)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let option = |i: usize| {
            let act = Act::FormPick(library, i);
            room.cells.ui.clicks.iter().find(|(_, a)| *a == act).map(|(rect, _)| *rect).unwrap_or_else(|| panic!("option {i}"))
        };
        let first = option(0);
        assert_eq!((first.x, first.right()), (25, 85), "clamped to the row: {first:?}");
        assert_eq!(buffer[(85, first.y)].symbol(), " ", "the padding cell survives");
        assert_eq!(buffer[(86, first.y)].symbol(), "│", "the border survives");
        let mut text = String::new();
        let mut x = first.x;
        while x < first.right() {
            let symbol = buffer[(x, first.y)].symbol();
            text.push_str(symbol);
            x += (kit::width(symbol) as u16).max(1);
        }
        assert!(text.len() > "(•) ".len() && format!("(•) {long}").starts_with(text.trim_end()), "cut, not garbled: {text:?}");
        let second = option(1);
        assert_eq!((second.x, second.y, second.width), (25, first.y + 1, 9), "the next wraps under it, whole");
        let trigger_y = second.y + 2;
        let label: String = (0..buffer.area.width).map(|x| buffer[(x, trigger_y)].symbol()).collect();
        assert!(label.contains(&*t!("bak.field_trigger")), "the trigger under the wrapped row: {label:?}");
    }

    #[test]
    fn the_room_boots_defaults_first_and_draws_the_table() {
        let _en = english();
        let mut room = new_room();
        room.queue(Op::Platform, "x");
        room.queued = None;
        room.apply(Done::Platform(Ok(defaults())));
        assert_eq!(room.queued, Some(Op::Load), "the list follows the defaults");
        room.queued = None;
        room.busy = None;
        room.apply(loaded(Vec::new(), BackupStatus::default()));
        let frame = draw(&mut room);
        assert!(frame.contains("Backups beta"), "{frame}");
        assert!(frame.contains("• idle — nothing running · 0 of 0 destinations on · trash swept hourly"), "{frame}");
        assert!(frame.contains("Add a backup destination"), "{frame}");
        assert!(frame.contains("(no destinations yet — a adds one)"), "{frame}");
        assert!(frame.contains("beta — the configuration may change in a future release"), "{frame}");
        assert!(frame.contains("a add a destination · Esc back"), "{frame}");
        room.apply(loaded(dests(), BackupStatus::default()));
        let frame = draw(&mut room);
        for word in ["LIBRARY", "DESTINATION", "TRIGGER", "RETENTION", "LAST RUN", "ON", "polls every 5 s"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let music = row(&frame, "music");
        assert!(music.contains("/Volumes/Backup/music") && music.contains("after each scan") && music.contains("30d") && music.contains("success · 2h") && music.contains("[✓]"), "{music}");
        let podcasts = row(&frame, "podcasts");
        assert!(podcasts.contains("daily at 03:00") && podcasts.contains("failed · 24h"), "{podcasts}");
        let vinyl = row(&frame, "vinyl");
        assert!(vinyl.contains("manual only") && vinyl.contains("no trash") && vinyl.contains("never") && vinyl.contains("[ ]"), "{vinyl}");
        assert!(frame.contains("• idle — nothing running · 2 of 3 destinations on"), "{frame}");
        // The cursor row's extras on the note line, the failure in its words.
        room.note = None;
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("/Volumes/Backup/podcasts · 200 ms/file · default excludes · failed: EACCES: permission denied"), "{frame}");
        assert!(frame.contains("↑↓ rows · Enter history · n run now · e edit · s on/off · r remove · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Up);
        let frame = draw(&mut room);
        assert!(frame.contains("/Volumes/Backup/music · default excludes · 812 copied, 11,204 unchanged, 3 trashed · 3.4 GB"), "{frame}");
    }

    #[test]
    fn the_state_line_shows_the_run_the_queue_and_the_bar() {
        let _en = english();
        let mut room = idle();
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
        let frame = draw(&mut room);
        assert!(frame.contains("• running — podcasts → /Volumes/Backup/podcasts · after each scan · 2m 14s"), "{frame}");
        assert!(frame.contains("▰▰▰▰▱▱▱▱▱▱ 41% · 1,204 of 2,930 · 812 copied · 380 unchanged · 12 trashed · 3.4 GB written"), "{frame}");
        assert!(frame.contains("polls every 2 s"), "{frame}");
        assert!(row(&frame, "podcasts").contains("running… 41%"), "{frame}");
        let mut first = active.clone();
        first.expected_files = None;
        first.bytes_copied = 0;
        room.apply(loaded(dests(), BackupStatus { active: Some(first), queue_length: 0 }));
        let frame = draw(&mut room);
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ first run — no estimate yet · 812 copied · 380 unchanged · 12 trashed"), "{frame}");
        assert!(!frame.contains("no bytes writ"), "the last group drops off a narrow column\n{frame}");
        assert!(row(&frame, "podcasts").contains("running…"), "{frame}");
        room.apply(loaded(dests(), BackupStatus { active: None, queue_length: 2 }));
        let frame = draw(&mut room);
        assert!(frame.contains("• 2 tasks queued — waiting for the active scan or backup to finish"), "{frame}");
        // The poll cadence follows the state.
        room.last_load = Some(Instant::now() - Duration::from_secs(3));
        room.queued = None;
        room.tick();
        assert_eq!(room.queued, Some(Op::Load), "two seconds while tasks wait");
        room.queued = None;
        room.apply(loaded(dests(), BackupStatus::default()));
        room.last_load = Some(Instant::now() - Duration::from_secs(3));
        room.queued = None;
        room.tick();
        assert!(room.queued.is_none(), "five seconds when idle");
    }

    #[test]
    fn the_add_form_walks_its_fields_checks_the_path_and_sends_what_changed() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!((f.focused(), f.library_name.as_str(), f.library_id, f.trigger), (Field::Library, "music", 10, Trigger::AfterScan));
        assert_eq!(f.excludes.value(), "Thumbs.db, desktop.ini, .DS_Store, ._*");
        let frame = draw(&mut room);
        assert!(frame.contains("Add a backup destination") && frame.contains("(•) music") && frame.contains("( ) podcasts"), "{frame}");
        assert!(frame.contains("(•) after each scan — when the library finishes scanning"), "{frame}");
        assert!(frame.contains("DESTINATION · another drive, not inside the library") && frame.contains("^B browse"), "{frame}");
        assert!(frame.contains("RETENTION DAYS") && frame.contains("THROTTLE MS/FILE") && frame.contains("HOUR (0–23)"), "{frame}");
        assert!(frame.contains("Tab next field · ←→ or Space choose · ^B browse · Enter add · Esc cancel"), "{frame}");
        // Radio fields walk with the arrows; the library choice re-asks the server.
        press(&mut room, KeyCode::Right);
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!((f.library_name.as_str(), f.library_id), ("podcasts", 20));
        press(&mut room, KeyCode::Tab); // trigger
        press(&mut room, KeyCode::Right);
        press(&mut room, KeyCode::Right);
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!(f.trigger, Trigger::Manual);
        assert!(!f.fields().contains(&Field::Hour), "the hour is a daily thing");
        press(&mut room, KeyCode::Tab); // destination
        type_text(&mut room, "/Volumes/Backup/pods");
        assert_eq!(room.queued, Some(Op::Check { library_id: 20, dest: "/Volumes/Backup/pods".into(), exclude_dest_id: None }));
        assert!(draw(&mut room).contains("checking the path…"));
        room.queued = None;
        room.apply(Done::Checked {
            dest: "/Volumes/Backup/pods".into(),
            result: Ok(PathCheck { ok: true, errors: Vec::new(), warnings: vec!["Destination does not exist yet. It will be created on the first backup run.".into()], info: PathCheckInfo::default() }),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("− Destination does not exist yet. It will be created on the first"), "{frame}");
        assert!(frame.lines().any(|l| l.trim_matches(|c| c == ' ' || c == '│') == "backup run."), "the sentence wraps at a word\n{frame}");
        // An untouched pattern field is OMITTED so the row follows the defaults.
        press(&mut room, KeyCode::Enter);
        assert_eq!(
            room.queued,
            Some(Op::Add(NewBackupDestination {
                library_id: 20,
                dest_path: "/Volumes/Backup/pods".into(),
                trigger_type: "manual".into(),
                daily_at_hour: None,
                retention_days: 30,
                inter_file_delay_ms: 0,
                exclude_globs: None,
            }))
        );
        room.queued = None;
        room.busy = None;
        // A hard error from the check blocks the send; a refusal keeps the form.
        room.apply(Done::Checked {
            dest: "/Volumes/Backup/pods".into(),
            result: Ok(PathCheck { ok: false, errors: vec!["Destination is inside the source library".into()], ..Default::default() }),
        });
        press(&mut room, KeyCode::Enter);
        assert!(room.queued.is_none());
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!(f.error.as_deref(), Some("Destination is inside the source library"));
        room.apply(Done::Added(Err(ApiError::Server { status: 409, message: "A destination already exists for this library + path".into() })));
        let Modal::Form(f) = &room.modal else { panic!("a refusal keeps the form") };
        assert!(f.error.as_deref().is_some_and(|e| e.contains("already exists")));
        room.apply(Done::Added(Ok(())));
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "destination added"));
        assert_eq!(room.queued, Some(Op::Load));
    }

    #[test]
    fn the_edit_form_sends_only_what_changed_and_resets_patterns_to_null() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down); // podcasts: daily at 3, 200 ms
        press(&mut room, KeyCode::Char('e'));
        let Modal::Form(f) = &room.modal else { panic!("the edit form") };
        assert_eq!((f.editing, f.focused(), f.trigger), (Some(2), Field::Trigger, Trigger::Daily));
        assert_eq!(f.fields(), vec![Field::Trigger, Field::Dest, Field::Retention, Field::Throttle, Field::Hour, Field::Excludes]);
        assert_eq!(room.queued, Some(Op::Check { library_id: 20, dest: "/Volumes/Backup/podcasts".into(), exclude_dest_id: Some(2) }), "an edit checks its own path, skipping its own row");
        room.queued = None;
        let frame = draw(&mut room);
        assert!(frame.contains("Edit — podcasts → /Volumes/Backup/podcasts"), "{frame}");
        assert!(frame.contains("podcasts  — fixed: remove and re-add to retarget"), "{frame}");
        assert!(frame.contains("^P reset patterns to the defaults") && frame.contains("Save ▸"), "{frame}");
        // Nothing changed: the form just closes.
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none());
        press(&mut room, KeyCode::Char('e'));
        room.queued = None;
        for _ in 0..4 {
            press(&mut room, KeyCode::Tab); // to the hour
        }
        let Modal::Form(f) = &room.modal else { panic!("the edit form") };
        assert_eq!(f.focused(), Field::Hour);
        press(&mut room, KeyCode::Backspace);
        type_text(&mut room, "2x3");
        press(&mut room, KeyCode::Tab); // excludes
        type_text(&mut room, ", *.log");
        press(&mut room, KeyCode::Enter);
        assert_eq!(
            room.queued,
            Some(Op::Patch {
                id: 2,
                patch: BackupPatch {
                    daily_at_hour: Some(Some(23)),
                    exclude_globs: Some(Some(vec!["Thumbs.db".into(), "desktop.ini".into(), ".DS_Store".into(), "._*".into(), "*.log".into()])),
                    ..Default::default()
                },
                what: Did::Saved,
            }),
            "digits only reach the hour; only the changed fields go"
        );
        room.queued = None;
        room.busy = None;
        ctrl(&mut room, 'p');
        let Modal::Form(f) = &room.modal else { panic!("the edit form") };
        assert!(f.excludes_reset && f.excludes.value() == "Thumbs.db, desktop.ini, .DS_Store, ._*");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Patch { patch: BackupPatch { exclude_globs: Some(None), .. }, .. })), "the reset sends null");
        room.queued = None;
        room.apply(Done::Patched { id: 2, what: Did::Saved, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.starts_with("saved")));
    }

    #[test]
    fn row_keys_run_toggle_and_gate_the_remove() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down); // vinyl: off
        press(&mut room, KeyCode::Char('n'));
        assert!(room.queued.is_none());
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("this destination is off")));
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::Patch { id: 3, patch: BackupPatch { enabled: Some(true), ..Default::default() }, what: Did::Enabled(true) }));
        room.queued = None;
        room.apply(Done::Patched { id: 3, what: Did::Enabled(true), result: Ok(()) });
        assert!(room.dests[2].enabled);
        room.queued = None;
        room.busy = None;
        press(&mut room, KeyCode::Char('n'));
        assert_eq!(room.queued, Some(Op::Run(3)));
        room.queued = None;
        room.apply(Done::Ran(Ok(RunAnswer { status: "skipped".into() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("previous run is still in progress")));
        room.apply(Done::Ran(Ok(RunAnswer { status: "queued".into() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n.contains("backup started")));
        room.queued = None;
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Remove(3)));
        let frame = draw(&mut room);
        assert!(frame.contains("Remove vinyl → /mnt/usb/vinyl?") && frame.contains("NOT deleted") && frame.contains("y remove · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        press(&mut room, KeyCode::Char('r'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Remove(3)));
        room.queued = None;
        room.apply(Done::Removed(Ok(())));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("the files on disk stay")));
        assert!(press(&mut room, KeyCode::Esc).is_none());
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn the_history_modal_lists_runs_and_tells_the_selected_ones_story() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::History { loaded: false, .. }));
        assert_eq!(room.queued, Some(Op::History(2)));
        room.queued = None;
        room.apply(Done::History {
            id: 2,
            result: Ok(vec![
                run("success", 3 * 3600, 12, 11_204, 3, 432_000_000, None),
                run("failed", 86_400, 0, 0, 0, 0, Some("EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'")),
                run("skipped", 3 * 86_400, 0, 0, 0, 0, None),
            ]),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("History — podcasts → /Volumes/Backup/podcasts"), "{frame}");
        for word in ["STARTED", "STATUS", "TRIGGER", "COPIED", "UNCHANGED", "TRASHED", "BYTES", "50 most recent · newest first"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let first = frame.lines().find(|l| l.contains("3h") && l.contains("success")).map(str::to_string).unwrap_or_default();
        assert!(first.contains("after each scan") && first.contains("11,204") && first.contains("412.0 MB"), "{first}");
        assert!(frame.contains("3h · success · after each scan · 2m 14s"), "{frame}");
        assert!(frame.contains("12 copied, 11,204 unchanged, 3 trashed"), "{frame}");
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("24h · failed · daily · 2m 14s"), "{frame}");
        assert!(frame.contains("EACCES: permission denied, mkdir"), "{frame}");
        assert!(frame.contains("↑↓ runs · Esc close"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn browsing_fills_the_destination_and_returns_to_the_form() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        ctrl(&mut room, 'b');
        assert_eq!(room.queued, Some(Op::Browse("~".into())), "the server's home, as Libraries browses");
        room.queued = None;
        room.apply(Done::Browsed(Ok(DirListing {
            path: "/Volumes".into(),
            directories: vec![crate::api::types::DirEntry { name: "Backup".into() }, crate::api::types::DirEntry { name: "NAS".into() }],
            files: Vec::new(),
        })));
        assert!(matches!(room.modal, Modal::Browser { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Choose the destination folder") && frame.contains("▸ Backup") && frame.contains("Use this folder ▸"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Browse("/Volumes/Backup".into())));
        room.queued = None;
        room.apply(Done::Browsed(Ok(DirListing { path: "/Volumes/Backup".into(), directories: Vec::new(), files: Vec::new() })));
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("back in the form") };
        assert_eq!(f.dest.value(), "/Volumes/Backup");
        assert!(matches!(room.queued, Some(Op::Check { .. })), "the chosen path is checked at once");
        // Esc in the browser returns to the form with its draft intact.
        room.queued = None;
        ctrl(&mut room, 'b');
        room.queued = None;
        room.apply(Done::Browsed(Ok(DirListing { path: "/Volumes/Backup".into(), directories: Vec::new(), files: Vec::new() })));
        press(&mut room, KeyCode::Esc);
        let Modal::Form(f) = &room.modal else { panic!("back in the form") };
        assert_eq!(f.dest.value(), "/Volumes/Backup");
        // With --same-machine the OS dialog picks instead.
        let mut local = Room::new(Client::new("http://home.mstream.example:3000").unwrap(), true);
        local.apply(Done::Platform(Ok(defaults())));
        local.queued = None;
        local.apply(loaded(dests(), BackupStatus::default()));
        press(&mut local, KeyCode::Char('a'));
        ctrl(&mut local, 'b');
        assert_eq!(local.queued, Some(Op::PickNative));
        local.queued = None;
        local.apply(Done::Picked(picker::Pick::Folder("/Volumes/USB/music".into())));
        let Modal::Form(f) = &local.modal else { panic!("the form") };
        assert_eq!(f.dest.value(), "/Volumes/USB/music");
    }

    #[test]
    fn a_modal_is_lent_to_its_draw_and_handed_back() {
        // Drawn from the modal itself, not a copy made every frame
        // (performance audit #110): after the frame it is as it was.
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        ctrl(&mut room, 'b');
        room.queued = None;
        room.apply(Done::Browsed(Ok(DirListing {
            path: "/Volumes".into(),
            directories: (0..300).map(|i| crate::api::types::DirEntry { name: format!("Drive {i}") }).collect(),
            files: Vec::new(),
        })));
        let before = format!("{:?}", room.modal);
        assert!(draw(&mut room).contains("▸ Drive 0"));
        assert_eq!(format!("{:?}", room.modal), before);
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
    use super::super::super::{Claim, HostedRoom, body_column};

    #[test]
    fn hosted_the_room_draws_inside_its_area_with_no_header_and_its_note_last() {
        let _en = english();
        let mut room = idle();
        room.note = Some(("music: backup started".into(), false));
        let buf = draw_hosted(&mut room, (100, 30), WINDOW);
        let text = shown(&buf);
        assert!(outside(&buf, WINDOW, true).is_empty(), "drawn outside its area:\n{text}");
        assert!(from(&buf, WINDOW.x, WINDOW.y).chars().all(|c| c == '·'), "the area's first row is the host's:\n{text}");
        assert!(!text.contains("Backups") && !text.contains("home.mstream.example"), "no header:\n{text}");
        assert!(text.contains("Add a backup destination") && text.contains("…/Backup/music"), "{text}");
        assert!(from(&buf, 19, 23).starts_with("music: backup started"), "the note on the last row:\n{text}");
        assert!(!text.contains("a add a destination · Esc back"), "the tips are the host's:\n{text}");
        let buf = draw_hosted(&mut room, (176, 46), DOCKED);
        assert!(outside(&buf, DOCKED, true).is_empty(), "drawn outside its area:\n{}", shown(&buf));
        assert!(from(&buf, 19, DOCKED.bottom() - 1).starts_with("music: backup started"), "{}", shown(&buf));
    }

    #[test]
    fn hosted_the_beta_chip_rides_the_bodys_first_row_not_row_zero() {
        let _en = english();
        let mut room = idle();
        let buf = draw_hosted(&mut room, (100, 30), WINDOW);
        let text = shown(&buf);
        let column = body_column(WINDOW, true);
        assert_eq!(from(&buf, column.right() - 4, column.y), "beta··", "flush with the column's right edge:\n{text}");
        assert!(buf_row(&buf, 0).chars().all(|c| c == '·'), "row zero is the host's bar:\n{text}");
        assert!(from(&buf, 19, 2).starts_with("• idle — nothing running"), "{text}");
        // Where the cadence fits beside the state, it keeps clear of the chip.
        let buf = draw_hosted(&mut room, (176, 46), DOCKED);
        assert!(buf_row(&buf, 2).contains("polls every 5 s··beta··"), "{}", shown(&buf));
        // The chip is there while the room is still loading, too.
        let mut loading = new_room();
        let buf = draw_hosted(&mut loading, (100, 30), WINDOW);
        assert_eq!(from(&buf, column.right() - 4, column.y), "beta··", "{}", shown(&buf));
        // Standalone it still sits beside the title.
        assert!(draw(&mut room).lines().next().is_some_and(|l| l.starts_with("  Backups beta")));
    }

    #[test]
    fn hosted_a_modal_centres_in_the_rooms_area() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Remove(1)));
        let buf = draw_hosted(&mut room, (100, 30), WINDOW);
        let at = kit::modal_rect(WINDOW, 68, 10, 10);
        assert!(framed_at(&buf, at), "the remove gate at {at:?}:\n{}", shown(&buf));
        assert!(outside(&buf, WINDOW, true).is_empty(), "{}", shown(&buf));
    }

    #[test]
    fn hosted_at_the_floor_it_never_says_resize() {
        let _en = english();
        let form = || Box::new(Form::add(&[("music".into(), 10)], &defaults()));
        let modals = [
            Modal::None,
            Modal::Form(form()),
            Modal::Browser {
                browse: Browse { path: "/Volumes".into(), dirs: (0..30).map(|i| format!("disk-{i}")).collect(), sel: 0 },
                form: form(),
            },
            Modal::History {
                dest: Box::new(dests().remove(0)),
                runs: (0..20).map(|i| run("success", 3600 * (i + 1), 1, 2, 0, 1024, None)).collect(),
                loaded: true,
                sel: 0,
            },
            Modal::Remove(1),
        ];
        for modal in modals {
            let mut room = idle();
            room.note = Some(("a note".into(), false));
            room.modal = modal;
            let buf = draw_hosted(&mut room, (100, 24), FLOOR);
            let text = shown(&buf);
            assert!(outside(&buf, FLOOR, false).is_empty(), "{:?} drew outside its area:\n{text}", room.modal);
            assert!(!text.contains(&*t!("resize")), "{text}");
            // A window's worth of rows holds every modal whole.
            let buf = draw_hosted(&mut room, (100, 30), WINDOW);
            assert!(outside(&buf, WINDOW, true).is_empty(), "{:?} spilled:\n{}", room.modal, shown(&buf));
        }
    }

    #[test]
    fn hosted_claims_every_key_while_a_modal_is_up() {
        let _en = english();
        let mut room = idle();
        assert_eq!(room.claims(), Claim::Open);
        assert!(!room.modal_up());
        assert_eq!(room.tips(), "↑↓ select · a add a destination · Esc back", "the room's own words");
        room.press(key_press(KeyCode::Char('a')));
        assert!(room.modal_up());
        assert_eq!(room.claims(), Claim::All, "the form takes every letter");
        assert_eq!(room.tips(), t!("bak.hint_add"));
        room.press(key_press(KeyCode::Esc));
        assert_eq!(room.claims(), Claim::Open);
    }

    /// The cell a focused field notes is the caret's however the field's
    /// window over a long value stands (hosted, it is where the GUI's window
    /// floats the input method's candidates).
    #[test]
    fn the_forms_field_notes_the_cell_its_caret_is_drawn_in() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &mut room.modal else { panic!("the add form") };
        f.focus = f.fields().iter().position(|field| *field == Field::Dest).expect("a destination field");
        type_text(&mut room, &format!("/Volumes/{}/tail", "deep".repeat(20)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "deep/tail", "");
        press(&mut room, KeyCode::Home);
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "", "/Volumes/deep");
    }
}
