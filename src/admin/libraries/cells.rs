//! The Libraries room in character cells: what `mstream-player admin`
//! draws in a terminal, and what the GUI player's Admin tab draws on its
//! grid ([`Screen::render_hosted`]). The room itself (its state, its
//! worker, its keys, its words) is the parent module; this is its view in
//! cells, and holds what is measured in cells: the add card, the table and
//! its scrollbar, and the four modals, on the kit's surface. It is also
//! where the room meets the hub's loop, as a [`Screen`].

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::super::{Screen, body_column, draw_foot, draw_header, frame_ground, host_of};
use super::*;
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::{ellipsize_path_start, g};

/// The table's NAME column, the wizard's width — the vpath is the point.
const NAME_W: u16 = 16;
/// The ` [X]` column, outside the selection area.
const REMOVE_W: u16 = 4;
/// The SYMLINKS column: the header word, and a `[✓]`/`[ ]` cell under it.
const SYM_W: u16 = 8;

/// What this view keeps on the room between frames. The room holds one
/// and never looks inside it.
pub(super) struct State {
    /// The kit's interaction surface: click/tip/bar registries, pointer,
    /// tooltip dwell, scrollbar capture and hold-repeat.
    ui: Surface<Act>,
    /// Last frame's selection — a change yanks the view to the selection.
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
        match &mut self.modal {
            Modal::PathEntry(draft) => {
                draft.scroll =
                    if up { draft.scroll.saturating_sub(1) } else { draft.scroll.saturating_add(1) };
            }
            Modal::None => {
                self.tscroll =
                    if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
            }
            _ => {}
        }
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.cells.ui.begin_frame();
    let Some(area) = frame_ground(frame, 58, 12) else { return };
    draw(frame, room, area, false);
}

/// The room inside `area`. Standalone it sits under its own header and
/// over its tips row. Hosted (the GUI player's Admin tab) the host's bar
/// is the header and its footer carries the tips, so the area's last row
/// is the status line alone. Modals and the tooltip centre in `area`
/// either way.
fn draw(frame: &mut Frame, room: &mut Room, area: Rect, hosted: bool) {
    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.cells.ui.pointer;
    if modal_open {
        room.cells.ui.pointer = None;
    }

    if !hosted {
        draw_header(frame, area, &t!("admin.title"), &host_of(&room.client));
    }

    // Full display: two-cell margins, the whole height above the bottom
    // lines (a table room, not a form — the kit's 74-cell column is
    // deliberately not applied).
    let column = body_column(area, hosted);
    draw_libraries(frame, room, column);

    let tips = footer_hint_as(room, hosted);
    draw_foot(frame, area, room.note.as_ref(), room.busy.as_deref(), &tips, hosted);

    if modal_open {
        room.cells.ui.pointer = live_pointer;
        room.cells.ui.clear_registries();
    }
    // The folder browser and the path modal each hold a server listing —
    // thousands of names at a music root — so they are drawn from where
    // they live, not from a copy made every frame (performance audit #110).
    match &mut room.modal {
        Modal::None => {}
        Modal::Browser(_) => {
            // Lent to its draw, which never touches the modal, and put back.
            let modal = std::mem::replace(&mut room.modal, Modal::None);
            if let Modal::Browser(browse) = &modal {
                draw_browser(frame, room, area, browse);
            }
            room.modal = modal;
        }
        Modal::PathEntry(draft) => {
            // The draft stays in place — its draw keeps the list's scroll
            // there — and lends the frame only its listing.
            let entries = std::mem::take(&mut draft.entries);
            let lent = PathDraft { entries, ..draft.clone() };
            draw_path_entry(frame, room, area, &lent);
            if let Modal::PathEntry(draft) = &mut room.modal {
                draft.entries = lent.entries;
            }
        }
        Modal::Name(draft) => {
            let draft = draft.clone();
            draw_name(frame, room, area, &draft);
        }
        Modal::Remove(i) => {
            let i = *i;
            draw_remove(frame, room, area, i);
        }
    }

    // The tooltip draws last — over everything, once the dwell matures.
    if let Some((target, text)) = room.cells.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// The room's body: the add card, then the table.
fn draw_libraries(frame: &mut Frame, room: &mut Room, column: Rect) {
    let mut y = column.y;

    // The add card — the screen's one add affordance (typing a path is the
    // `t` shortcut, in the tips line) — sits ABOVE the table so it holds
    // one spot as libraries come and go. Green: the affirmative add.
    let add_rect = Rect { x: column.x, y, width: column.width, height: 3 };
    let add_hover = room.cells.ui.pointer.is_some_and(|p| add_rect.contains(p));
    let add_color = if add_hover { th().bright } else { th().ok };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(add_color));
    let inner = block.inner(add_rect);
    frame.render_widget(block, add_rect);
    frame.render_widget(
        Paragraph::new(Span::styled(
            t!("folders.add_card").to_string(),
            Style::default().fg(add_color).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        inner,
    );
    room.cells.ui.click(add_rect, Act::Add);
    y += 4;

    // The table: NAME first — the vpath is the point. The [X] remove
    // control sits to the RIGHT of the selection area, not inside it.
    let sel_width = column.width.saturating_sub(REMOVE_W);
    let header = Rect { x: column.x, y, width: sel_width, height: 1 };
    // The name heading is padded to NAME_W in cells, where the rows start
    // their paths: `{:<16}` pads characters, which set the folder heading
    // two cells right of its column under a Japanese "名前" (the wizard's
    // table measures it the same way).
    let name_col = t!("folders.col_name").to_string();
    let name_pad = " ".repeat(usize::from(NAME_W).saturating_sub(kit::width(&name_col)));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{name_col}{name_pad}"), dim()),
            Span::styled(t!("folders.col_folder").to_string(), dim()),
        ])),
        header,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.col_symlinks").to_string(), dim()))
            .alignment(Alignment::Right),
        header,
    );
    y += 1;
    // The rule spans the FULL row — selection area and the [X] column.
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(column.width as usize), dim())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;

    // The table frame is ALWAYS on screen; an empty list says so where
    // the first row would be.
    if room.libs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("folders.nothing_yet").to_string(), dim())),
            Rect { x: column.x, y, width: sel_width, height: 1 },
        );
        return;
    }
    let avail = (column.y + column.height).saturating_sub(y) as usize;
    let sel_moved = room.sel != room.cells.sel_anchor;
    room.cells.sel_anchor = room.sel;
    let reveal = room.reveal.take().or(if sel_moved { room.sel } else { None });
    let (first, visible) = kit::table_view(room.libs.len(), reveal, room.tscroll, avail);
    room.tscroll = first;
    if visible == 0 {
        return;
    }
    let rows_y = y;
    let path_w = sel_width.saturating_sub(NAME_W + SYM_W + 2);
    for i in first..first + visible {
        let selected = room.sel == Some(i);
        let (name, root, follow) = {
            let lib = &room.libs[i];
            (lib.name.clone(), lib.root.clone(), lib.follow_symlinks)
        };
        let row_bg = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else {
            Style::default()
        };
        let rect = Rect { x: column.x, y, width: sel_width, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(" ".repeat(sel_width as usize), row_bg)),
            rect,
        );
        // Names are plain text, not chips: the server has no rename, so
        // nothing here pretends to offer one.
        let name_style = if selected { row_bg.add_modifier(Modifier::BOLD) } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Span::styled(name, name_style)),
            Rect { x: column.x, y, width: NAME_W.min(sel_width), height: 1 },
        );
        // Long paths keep their LEAF visible: a leading … and the tail. The
        // full path rides the tooltip whenever the cell clipped.
        let path_rect = Rect { x: column.x + NAME_W, y, width: path_w, height: 1 };
        let (shown, clipped) = ellipsize_path_start(&root, path_rect.width as usize);
        frame.render_widget(Paragraph::new(Span::styled(shown, row_bg)), path_rect);
        if clipped {
            let (tip, _) = ellipsize_path_start(&root, kit::TIP_WRAP * 3);
            room.cells.ui.tip(path_rect, tip);
        }
        // The SYMLINKS cell: a checkbox glyph wearing its state, clickable
        // like every other control, `s` on the cursor row by key.
        let sym_rect = Rect { x: column.x + sel_width - SYM_W, y, width: SYM_W, height: 1 };
        let sym_hover = room.cells.ui.pointer.is_some_and(|p| sym_rect.contains(p));
        let glyph = if follow { g("[✓]", "[x]") } else { "[ ]" };
        let sym_style = match (selected, sym_hover, follow) {
            (true, _, _) => row_bg,
            (false, true, _) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
            (false, false, true) => Style::default().fg(th().ok),
            (false, false, false) => dim(),
        };
        frame.render_widget(Paragraph::new(Span::styled(glyph, sym_style)), sym_rect);
        room.cells.ui.click(sym_rect, Act::ToggleSymlinks(i));
        // The [X]: DIM until the pointer arrives, then the destructive red.
        let x_rect = Rect { x: column.x + sel_width + 1, y, width: 3, height: 1 };
        let x_hover = room.cells.ui.pointer.is_some_and(|p| x_rect.contains(p));
        let x_style = if x_hover {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled("[X]", x_style)), x_rect);
        room.cells.ui.click(x_rect, Act::RemoveAt(i));
        y += 1;
    }

    // Overflow → the kit's scrollbar, in the margin cell right of the [X].
    let bar = Rect { x: column.x + column.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(
        frame,
        &mut room.cells.ui,
        bar,
        room.libs.len(),
        visible,
        first,
        Act::TableScroll(-1),
        Act::TableScroll(1),
        Act::TableScrollTo,
    );
}

fn draw_browser(frame: &mut Frame, room: &mut Room, area: Rect, browse: &Browse) {
    let inner = kit::modal_frame(frame, area, 66, 18, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("browse.title").to_string(), bold())),
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
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else {
            Style::default()
        };
        let rect = Rect { x: inner.x, y: list_top + row as u16, width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} {}", g("▸", "►"), browse.dirs[i]), style)),
            rect,
        );
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
    let open = kit::button(
        frame,
        &mut room.cells.ui,
        row(up.right() + 1),
        &t!("browse.open"),
        false,
        Act::BrowseEnter,
    );
    let add = kit::button(
        frame,
        &mut room.cells.ui,
        row(open.right() + 1),
        &t!("browse.add"),
        true,
        Act::BrowseAdd,
    );
    kit::button(
        frame,
        &mut room.cells.ui,
        row(add.right() + 1),
        &t!("browse.close"),
        false,
        Act::BrowseCancel,
    );
}

fn draw_path_entry(frame: &mut Frame, room: &mut Room, area: Rect, draft: &PathDraft) {
    let count = draft.suggestion_count();
    let shown = count.min(6) as u16;
    // Anchored as if always full: the title and input hold one spot and
    // the suggestion list grows DOWNWARD beneath them.
    let inner = kit::modal_frame_anchored(frame, area, 62, 7 + shown, 13, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("path_modal.title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::PathCancel);
    let shown = kit::field_display(
        &mut room.cells.ui,
        inner.x,
        inner.y + 2,
        draft.text.value(),
        draft.text.cursor(),
        inner.width,
        None,
    );
    frame.render_widget(
        Paragraph::new(Span::raw(shown)),
        Rect { x: inner.x, y: inner.y + 2, width: inner.width, height: 1 },
    );
    let sel_moved = draft.sel != draft.sel_anchor;
    let reveal = if sel_moved { draft.sel } else { None };
    let (first, visible) = kit::table_view(count, reveal, draft.scroll, 6);
    if let Modal::PathEntry(d) = &mut room.modal {
        d.scroll = first;
        d.sel_anchor = d.sel;
    }
    let overflow = count > visible;
    let row_width = if overflow { inner.width.saturating_sub(1) } else { inner.width };
    for (row, entry) in draft.suggestions().skip(first).take(visible).enumerate() {
        let i = first + row;
        let selected = draft.sel == Some(i);
        let rect = Rect { x: inner.x, y: inner.y + 4 + row as u16, width: row_width, height: 1 };
        let hovered = room.cells.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            dim()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} {entry}", g("▸", "►")), style)),
            rect,
        );
        room.cells.ui.click(rect, Act::PathSuggest(i));
    }
    let bar = Rect {
        x: inner.x + inner.width.saturating_sub(1),
        y: inner.y + 4,
        width: 1,
        height: visible as u16,
    };
    kit::scroll_list(
        frame,
        &mut room.cells.ui,
        bar,
        count,
        visible,
        first,
        Act::PathScroll(-1),
        Act::PathScroll(1),
        Act::PathScrollTo,
    );
    if let Some(err) = &draft.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            Rect { x: inner.x, y: inner.y + 4, width: inner.width, height: 1 },
        );
    }
}

/// The Name modal: the chosen folder, the kit's text input holding the
/// draft vpath, the one fact that matters in gold, and Add.
fn draw_name(frame: &mut Frame, room: &mut Room, area: Rect, draft: &NameDraft) {
    let inner = kit::modal_frame(frame, area, 68, 16, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(
            t!("admin.name_title").to_string(),
            Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        )),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.cells.ui, inner, Act::NameCancel);

    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_folder").to_string(), dim())),
        line(inner.y + 2),
    );
    let (shown, _) = ellipsize_path_start(&draft.directory, inner.width.saturating_sub(2) as usize);
    frame.render_widget(Paragraph::new(Span::raw(shown)), line(inner.y + 3));

    // The text input: label above, a 3-row Rounded card, focused border
    // with the caret after the value.
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_label").to_string(), dim())),
        line(inner.y + 5),
    );
    let field = Rect { x: inner.x + 1, y: inner.y + 6, width: 32.min(inner.width), height: 3 };
    let card = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(accent());
    let field_inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = kit::field_display(
        &mut room.cells.ui,
        field_inner.x + 1,
        field_inner.y,
        draft.name.value(),
        draft.name.cursor(),
        field_inner.width.saturating_sub(1),
        None,
    );
    frame.render_widget(
        Paragraph::new(Span::raw(shown)),
        Rect { x: field_inner.x + 1, y: field_inner.y, width: field_inner.width.saturating_sub(1), height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_charset").to_string(), dim())),
        Rect {
            x: field.right() + 2,
            y: inner.y + 7,
            width: inner.width.saturating_sub(field.width + 3),
            height: 1,
        },
    );

    let gold = Style::default().fg(th().gold);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_permanent_1").to_string(), gold)),
        line(inner.y + 9),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_permanent_2").to_string(), gold)),
        line(inner.y + 10),
    );
    if let Some(err) = &draft.error {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), gold)), line(inner.y + 12));
    }

    let label = t!("admin.name_add").to_string();
    let x = inner.right().saturating_sub(kit::width(&label) as u16 + 4);
    kit::button(
        frame,
        &mut room.cells.ui,
        Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 },
        &label,
        true,
        Act::NameAdd,
    );
}

/// The warning gate before a remove: consequences before verbs, the safe
/// choice as the primary, no [X] — a gate forces an explicit choice.
fn draw_remove(frame: &mut Frame, room: &mut Room, area: Rect, i: usize) {
    let name = room.libs.get(i).map(|l| l.name.clone()).unwrap_or_default();
    let inner = kit::modal_frame(frame, area, 68, 11, th().gold);
    let gold = Style::default().fg(th().gold);
    let lines = vec![
        Line::from(Span::styled(
            t!("admin.remove_title", name = name).to_string(),
            gold.add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            t!("admin.remove_files_stay").to_string(),
            Style::default().fg(th().ok),
        )),
        Line::from(Span::styled(t!("admin.remove_users", name = name).to_string(), gold)),
        Line::from(Span::styled(t!("admin.remove_tracks").to_string(), gold)),
    ];
    // The body clips ABOVE the button row.
    let body = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(2),
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);

    let y = inner.bottom().saturating_sub(1);
    let keep = t!("admin.remove_keep").to_string();
    let remove = t!("admin.remove_confirm").to_string();
    let remove_w = kit::width(&remove) as u16 + 4;
    let keep_w = kit::width(&keep) as u16 + 4;
    let keep_x = inner.right().saturating_sub(remove_w + 2 + keep_w);
    let keep_rect = kit::button(
        frame,
        &mut room.cells.ui,
        Rect { x: keep_x, y, width: inner.width, height: 1 },
        &keep,
        true,
        Act::RemoveCancel,
    );
    kit::button(
        frame,
        &mut room.cells.ui,
        Rect { x: keep_rect.right() + 2, y, width: inner.width, height: 1 },
        &remove,
        false,
        Act::RemoveConfirm,
    );
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    use super::super::tests::*;
    use super::*;
    use crate::api::types::DirEntry;

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

    #[test]
    fn the_room_draws_the_kit_table_and_names_only_what_works() {
        let _en = english();
        let mut room = seeded();
        let frame = draw(&mut room);
        assert!(frame.contains("Libraries"), "{frame}");
        assert!(frame.contains("home.mstream.example · admin"), "{frame}");
        assert!(frame.contains("Add a music folder"), "{frame}");
        assert!(frame.contains("NAME") && frame.contains("FOLDER") && frame.contains("SYMLINKS"), "{frame}");
        assert!(frame.contains("music           /srv/music"), "{frame}");
        assert!(frame.contains("[✓]") && frame.contains("[ ]") && frame.contains("[X]"), "{frame}");
        assert!(frame.contains("↑ ↓ select · b browse · t type a path · q quit"), "{frame}");
        assert!(!frame.contains("Scan now") && !frame.contains("─────────────────────────────────────────────────────────────────────────────────────────────────"), "no bottom bar, no gold rule");
        handle_key(&mut room, key(KeyCode::Down));
        let frame = draw(&mut room);
        assert!(frame.contains("s symlinks · r remove · Esc deselect"), "{frame}");
        room.libs.clear();
        let frame = draw(&mut room);
        assert!(frame.contains("(nothing added yet)"), "{frame}");
        assert!(frame.contains("b browse · t type a path · q quit"), "{frame}");
    }

    #[test]
    fn the_name_and_remove_modals_draw_their_one_fact() {
        let _en = english();
        let mut room = seeded();
        room.open_name("/Volumes/NAS/Field Recordings 2024".into());
        let frame = draw(&mut room);
        assert!(frame.contains("Name this library"), "{frame}");
        assert!(frame.contains("field-recordings-2024"), "{frame}");
        assert!(frame.contains("can't be changed later"), "{frame}");
        assert!(frame.contains("Add folder ▸"), "{frame}");
        assert!(frame.contains("type to edit · Enter add · Esc cancel"), "{frame}");
        room.modal = Modal::Remove(1);
        let frame = draw(&mut room);
        assert!(frame.contains("Remove vinyl from mStream?"), "{frame}");
        assert!(frame.contains("stay on disk"), "{frame}");
        assert!(frame.contains("◂ Keep it") && frame.contains("Remove  "), "{frame}");
        assert!(frame.contains("y remove · Esc keep"), "{frame}");
    }

    #[test]
    fn the_path_modal_draws_from_its_own_draft_and_keeps_its_listing() {
        // The draft is drawn where it lives, lending the frame its listing
        // instead of being copied with it every frame (performance audit
        // #110): the listing is back after a draw, and the scroll the draw
        // keeps lands in the draft.
        let _en = english();
        let mut room = room(false);
        handle_key(&mut room, key(KeyCode::Char('t')));
        type_text(&mut room, "/srv/");
        room.queued = None;
        room.apply(Done::Completed {
            dir: "/srv/".into(),
            listing: Ok(DirListing {
                path: "/srv".into(),
                directories: (0..40).map(|i| DirEntry { name: format!("d{i:02}") }).collect(),
                files: Vec::new(),
            }),
        });
        for _ in 0..12 {
            handle_key(&mut room, key(KeyCode::Down));
        }
        let frame = draw(&mut room);
        assert!(frame.contains("▸ d11"), "{frame}");
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path modal") };
        assert_eq!(draft.entries.len(), 40, "the listing is back after the frame");
        assert_eq!((draft.sel, draft.sel_anchor), (Some(11), Some(11)), "the draw kept its anchor");
        assert!((6..=11).contains(&draft.scroll), "scrolled to the cursor: {}", draft.scroll);
        let scroll = draft.scroll;
        draw(&mut room);
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path modal") };
        assert_eq!(draft.scroll, scroll, "a still cursor keeps its scroll");
    }

    /// `cargo test --release path_modal_frame_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check; run --release with --nocapture"]
    fn path_modal_frame_cost() {
        let _en = english();
        let folders: Vec<String> = (0..5_000).map(|i| format!("Folder {i:04}")).collect();
        let mut room = room(false);
        handle_key(&mut room, key(KeyCode::Char('t')));
        type_text(&mut room, "/big/");
        room.queued = None;
        room.apply(Done::Completed {
            dir: "/big/".into(),
            listing: Ok(DirListing {
                path: "/big".into(),
                directories: folders.iter().map(|name| DirEntry { name: name.clone() }).collect(),
                files: Vec::new(),
            }),
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        for which in ["path modal", "folder browser"] {
            if which == "folder browser" {
                room.modal = Modal::Browser(Browse { path: "/big".into(), dirs: folders.clone(), sel: 0 });
            }
            let frames = 200;
            let started = std::time::Instant::now();
            for _ in 0..frames {
                terminal.draw(|frame| render(frame, &mut room)).unwrap();
            }
            let per = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            println!("libraries {which}, 5,000 folders: {per:.3} ms a frame");
        }
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

    #[test]
    fn hosted_the_room_draws_inside_its_area_with_no_header_and_its_note_last() {
        let _en = english();
        let mut room = seeded();
        room.queue(Op::Load, t!("admin.busy_loading"));
        let buf = draw_hosted(&mut room, (100, 30), WINDOW);
        let text = shown(&buf);
        assert!(outside(&buf, WINDOW, true).is_empty(), "drawn outside its area:\n{text}");
        assert!(from(&buf, WINDOW.x, WINDOW.y).chars().all(|c| c == '·'), "the area's first row is the host's:\n{text}");
        assert!(!text.contains("Libraries") && !text.contains("home.mstream.example"), "no header:\n{text}");
        assert!(text.contains("Add a music folder") && text.contains("/srv/music"), "{text}");
        assert!(from(&buf, 19, 23).starts_with("loading libraries…"), "the busy line on the last row:\n{text}");
        assert!(!text.contains("q quit") && !text.contains("b browse"), "the tips are the host's:\n{text}");
        // Docked beside the log column, nothing spills either.
        let buf = draw_hosted(&mut room, (176, 46), DOCKED);
        assert!(outside(&buf, DOCKED, true).is_empty(), "drawn outside its area:\n{}", shown(&buf));
        assert!(from(&buf, 19, DOCKED.bottom() - 1).starts_with("loading libraries…"), "{}", shown(&buf));
    }

    #[test]
    fn hosted_a_modal_centres_in_the_rooms_area() {
        let _en = english();
        let mut room = seeded();
        handle_key(&mut room, key(KeyCode::Char('t')));
        assert!(matches!(room.modal, Modal::PathEntry(_)));
        let buf = draw_hosted(&mut room, (100, 30), WINDOW);
        let at = kit::modal_rect(WINDOW, 62, 7, 13);
        assert!(framed_at(&buf, at), "the path modal at {at:?}:\n{}", shown(&buf));
        assert!(outside(&buf, WINDOW, true).is_empty(), "{}", shown(&buf));
    }

    #[test]
    fn hosted_at_the_floor_it_draws_short_and_never_says_resize() {
        let _en = english();
        let modals = [
            Modal::None,
            Modal::Browser(Browse {
                path: "/srv".into(),
                dirs: (0..30).map(|i| format!("folder-{i}")).collect(),
                sel: 0,
            }),
            Modal::PathEntry(PathDraft::default()),
            Modal::Name(NameDraft { directory: "/srv/music".into(), name: Input::new("music".into()), error: None }),
            Modal::Remove(1),
        ];
        for modal in modals {
            let mut room = seeded();
            room.libs.extend((0..20).map(|i| Lib { name: format!("lib-{i}"), root: format!("/srv/{i}"), follow_symlinks: false }));
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
    fn hosted_the_hint_says_esc_back_not_q_quit() {
        let _en = english();
        let mut room = seeded();
        assert_eq!(Screen::hint(&room), "↑ ↓ select · b browse · t type a path · Esc back");
        assert!(footer_hint_as(&room, false).ends_with("q quit"), "standalone keeps its own words");
        room.libs.clear();
        assert_eq!(Screen::hint(&room), "b browse · t type a path · Esc back");
        assert!(footer_hint_as(&room, false).ends_with("q quit"));
        // The modals' tips are the same words in either shell.
        room.modal = Modal::Remove(0);
        assert_eq!(Screen::hint(&room), footer_hint_as(&room, false));
        assert!(Screen::modal_open(&room));
        assert_eq!(Screen::claim(&room), super::super::super::Claim::All);
    }

    /// Hosted, the cell a focused field notes is where the GUI's window
    /// floats the input method's candidates, and what tells it a field has
    /// the keyboard: it must be the caret's, wherever the field's window
    /// over a long value stands.
    #[test]
    fn the_typed_path_and_the_name_note_the_cell_their_caret_is_drawn_in() {
        let _en = english();
        let mut room = seeded();
        handle_key(&mut room, key(KeyCode::Char('t')));
        type_text(&mut room, &format!("/srv/{}/tail", "deep".repeat(20)));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "deep/tail", "");
        // Four in from the end, the tail is past the window's edge: a
        // clip mark stands after the caret.
        for _ in 0..4 {
            handle_key(&mut room, key(KeyCode::Left));
        }
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "deep/", "…");
        handle_key(&mut room, key(KeyCode::Home));
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "", "/srv/deep");

        let name = Input::new(format!("{}-end", "x".repeat(40)));
        room.modal = Modal::Name(NameDraft { directory: "/srv/music".into(), name, error: None });
        super::super::super::assert_caret_between(&draw(&mut room), &room.cells.ui, "xxx-end", "");
        handle_key(&mut room, key(KeyCode::Esc));
        draw(&mut room);
        assert_eq!(room.cells.ui.caret_at(), None, "no field, no caret");
    }

    /// The table under the Name modal, hosted as the GUI's Admin tab
    /// hosts it and driven through the hub's own pointer routine: a press
    /// where the table's bar stands neither steps nor holds an arrow nor
    /// drags the thumb (the room beneath a modal is inert, its bar dropped
    /// with every rect it registered before the modal draws), and the
    /// modal stays up. With the modal gone the bar answers again.
    #[test]
    fn a_press_on_the_tables_bar_under_the_name_modal_arms_nothing() {
        use crate::admin::drive_pointer;
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let _en = english();
        let mut room = room(false);
        room.libs = (0..40)
            .map(|i| Lib { name: format!("lib{i:02}"), root: format!("/srv/{i:02}"), follow_symlinks: false })
            .collect();
        let pointer = |room: &mut Room, kind: MouseEventKind, x: u16, y: u16| {
            drive_pointer(room, MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
        };
        let down_at = MouseEventKind::Down(MouseButton::Left);
        let up_at = MouseEventKind::Up(MouseButton::Left);
        let wait = || std::thread::sleep(kit::ARROW_DELAY + std::time::Duration::from_millis(30));
        let buf = draw_hosted(&mut room, (100, 30), WINDOW);
        let (x, up) = (WINDOW.left()..WINDOW.right())
            .flat_map(|x| (WINDOW.top()..WINDOW.bottom() - 1).map(move |y| (x, y)))
            .find(|&(x, y)| buf[(x, y)].symbol() == "▲" && matches!(buf[(x, y + 1)].symbol(), "█" | "│"))
            .expect("the table's bar");
        let down = (up + 1..WINDOW.bottom()).find(|&y| buf[(x, y)].symbol() == "▼").expect("its ▼");
        let track = (up + down) / 2;
        room.tscroll = 10;

        // ▲ under the modal: nothing beneath takes the press.
        room.open_name("/srv/new".into());
        draw_hosted(&mut room, (100, 30), WINDOW);
        pointer(&mut room, down_at, x, up);
        assert!(matches!(room.modal, Modal::Name(_)), "the modal stays up");
        assert!(!room.cells.ui.holding_bar(), "and the arrow beneath held nothing");
        wait();
        assert!(room.cells.ui.hold_action().is_none(), "so nothing repeats");
        pointer(&mut room, up_at, x, up);

        // The track under the modal: no jump, no thumb.
        pointer(&mut room, down_at, x, track);
        pointer(&mut room, MouseEventKind::Drag(MouseButton::Left), x, down - 1);
        pointer(&mut room, up_at, x, down - 1);
        assert_eq!(room.tscroll, 10, "the table never moved");

        // The modal gone, ▲ steps and repeats, and the track jumps and
        // drags the thumb.
        room.act(Act::NameCancel);
        draw_hosted(&mut room, (100, 30), WINDOW);
        pointer(&mut room, down_at, x, up);
        assert_eq!(room.tscroll, 9);
        assert!(room.cells.ui.holding_bar());
        wait();
        let repeat = room.cells.ui.hold_action().expect("the repeat");
        room.act(repeat);
        assert_eq!(room.tscroll, 8);
        pointer(&mut room, up_at, x, up);
        draw_hosted(&mut room, (100, 30), WINDOW);
        pointer(&mut room, down_at, x, up + 1);
        assert_eq!(room.tscroll, 0, "the track's first cell is the top");
        pointer(&mut room, MouseEventKind::Drag(MouseButton::Left), x, down - 1);
        assert!(room.tscroll > 10, "the thumb rode the hand down");
        pointer(&mut room, up_at, x, down - 1);
    }
}
