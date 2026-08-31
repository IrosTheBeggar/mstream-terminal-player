//! The Playlists room: the server's playlists as a browsable list — open
//! one and play it, or manage the list itself (create, rename, delete).
//!
//! Contract: docs/ux-contracts/playlists.md (clause numbers below cite
//! it). The design of record is the mobile browser tab's server-playlists
//! view; every element here maps to an idiom this GUI already draws — the
//! albums room's drill, the kit's affirmative add-card, the servers
//! room's hover row verbs, the sonic save prompt's modal — which is why
//! this feature carries no canvas of its own.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{dim, input_display, modal_frame, scroll_list, table_view};
use crate::tui::app::{Action, Entry};
use crate::tui::worker::{ApiCmd, LibraryNode};

use super::{Act, Gui, accent, bright_bold, put, sel};

/// The room's own state: the name dialog, the delete gate, and the two
/// levels' wheels. The rows themselves are the App's library pane.
pub(crate) struct PlaylistsUi {
    /// The name dialog: `rename_of` is the old name when renaming, `None`
    /// when creating; `name` is the draft.
    pub dialog: Option<PlDialog>,
    /// The delete gate, holding the name it asks about (clause 30).
    pub confirm: Option<String>,
    /// List-level wheel offset + reveal, and the drilled tracks' own.
    pub lscroll: usize,
    pub lreveal: bool,
    pub tscroll: usize,
    pub treveal: bool,
}

pub(crate) struct PlDialog {
    pub rename_of: Option<String>,
    pub name: String,
}

impl PlaylistsUi {
    pub(crate) fn new() -> Self {
        PlaylistsUi {
            dialog: None,
            confirm: None,
            lscroll: 0,
            lreveal: false,
            tscroll: 0,
            treveal: false,
        }
    }
}

/// The playlist a drilled view is inside of, if any.
fn drilled(gui: &Gui) -> Option<String> {
    match gui.app.library_node() {
        LibraryNode::Playlist(name) => Some(name.clone()),
        _ => None,
    }
}

/// Whether the library pane currently holds this room's data at all — a
/// fresh nav entry fetches, and until the answer lands the pane may still
/// be another room's walk.
fn holds_playlists(gui: &Gui) -> bool {
    matches!(gui.app.library_node(), LibraryNode::Playlists | LibraryNode::Playlist(_))
}

// ── Drawing ─────────────────────────────────────────────────────────────────

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    put(
        frame,
        content.x,
        content.y,
        &t!("gui.nav.playlists"),
        Style::default().add_modifier(Modifier::BOLD),
    );
    if !gui.app.connected {
        put(frame, content.x, content.y + 2, &t!("gui.no_server"), dim());
        return;
    }
    if !holds_playlists(gui) {
        put(frame, content.x, content.y + 2, &t!("busy.listing"), accent());
        return;
    }
    match drilled(gui) {
        Some(name) => draw_tracks(frame, gui, content, &name),
        None => draw_list(frame, gui, content),
    }
}

/// The list level: the affirmative New card above the rows it feeds (the
/// kit's own words for it), then the playlists — hover reveals rename and
/// the [X] remove, the servers room's idiom (clauses 1–4).
fn draw_list(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let card_w = content.width.min(60);
    let card = Rect { x: content.x, y: content.y + 2, width: card_w, height: 3 };
    let hover = gui.ui.pointer.is_some_and(|p| card.contains(p));
    let color = if hover { th().bright } else { th().ok };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(color));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    let label = format!("+ {}", t!("gui.pl.new"));
    let x = inner.x + (inner.width.saturating_sub(label.chars().count() as u16)) / 2;
    put(frame, x, inner.y, &label, Style::default().fg(color).add_modifier(Modifier::BOLD));
    gui.ui.click(card, Act::PlNew);

    // entries[0] is the pane's Parent row — the way back to a Library root
    // this room's nav replaces, so the list starts at 1.
    let entries = &gui.app.library.entries;
    let names: Vec<(usize, String)> = entries
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(i, entry)| match entry {
            Entry::Node { label, .. } => Some((i, label.clone())),
            _ => None,
        })
        .collect();

    let list = Rect {
        x: content.x,
        y: card.y + 4,
        width: content.width.saturating_sub(2),
        height: content.height.saturating_sub(7),
    };
    if names.is_empty() {
        let words = if gui.app.library.loading {
            t!("busy.listing").to_string()
        } else {
            t!("gui.pl.empty").to_string()
        };
        let style = if gui.app.library.loading { accent() } else { dim() };
        put(frame, list.x, list.y, &words, style);
        return;
    }

    let selected = gui.app.library.state.selected();
    let reveal = gui
        .playlists
        .lreveal
        .then_some(selected)
        .flatten()
        .and_then(|sel| names.iter().position(|(i, _)| *i == sel));
    gui.playlists.lreveal = false;
    let (first, visible) = table_view(names.len(), reveal, gui.playlists.lscroll, list.height as usize);
    gui.playlists.lscroll = first;

    let rename_label = t!("gui.pl.rename_verb").to_string();
    for (row, (index, name)) in names.iter().skip(first).take(visible).enumerate() {
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        let is_sel = *index == selected.unwrap_or(usize::MAX);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let style = match (is_sel, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        let verbs_w = rename_label.chars().count() as u16 + 6;
        put(
            frame,
            list.x,
            y,
            &super::bar::clip(name, list.width.saturating_sub(verbs_w + 1) as usize),
            style,
        );
        gui.ui.click(rect, Act::PlRow(*index));
        if hover {
            // The record's ⋮ menu, worn as the row's own verbs: rename
            // dim-to-bright, the [X] wearing the destructive color only
            // under the pointer (clause 3; the kit's per-row remove).
            let rx = rect.right().saturating_sub(verbs_w);
            let rename_rect =
                Rect { x: rx, y, width: rename_label.chars().count() as u16, height: 1 };
            let rhover = gui.ui.pointer.is_some_and(|p| rename_rect.contains(p));
            put(frame, rx, y, &rename_label, if rhover { bright_bold() } else { dim() });
            gui.ui.click(rename_rect, Act::PlRename(*index));
            let x_rect = Rect { x: rect.right().saturating_sub(4), y, width: 3, height: 1 };
            let xhover = gui.ui.pointer.is_some_and(|p| x_rect.contains(p));
            let xstyle = if xhover {
                Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
            } else {
                dim()
            };
            put(frame, x_rect.x, y, "[X]", xstyle);
            gui.ui.click(x_rect, Act::PlDelete(*index));
        }
    }
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right().saturating_sub(1), y: list.y, width: 1, height: list.height },
        names.len(),
        visible,
        first,
        Act::PlScrollBy(-1),
        Act::PlScrollBy(1),
        Act::PlScrollTo,
    );
}

/// Inside a playlist: the tracks, the shared pane rows — activate plays,
/// the hover [+] queues one, Parent walks back (clauses 40–41). The
/// albums room's drilled shape, name for name.
fn draw_tracks(frame: &mut Frame, gui: &mut Gui, content: Rect, name: &str) {
    let crumb = format!(
        "{} {} {}",
        t!("gui.nav.playlists"),
        if legacy_conhost() { ">" } else { "▸" },
        name
    );
    put(
        frame,
        content.x,
        content.y,
        &super::bar::clip(&crumb, content.width as usize),
        dim(),
    );

    let entries = &gui.app.library.entries;
    if entries.len() <= 1 && gui.app.library.loading {
        put(frame, content.x, content.y + 2, &t!("busy.listing"), accent());
        return;
    }
    let list = Rect {
        x: content.x,
        y: content.y + 2,
        width: content.width.saturating_sub(2),
        height: content.height.saturating_sub(2),
    };
    let selected = gui.app.library.state.selected();
    let reveal = gui.playlists.treveal.then_some(selected).flatten();
    gui.playlists.treveal = false;
    let (first, visible) = table_view(entries.len(), reveal, gui.playlists.tscroll, list.height as usize);
    gui.playlists.tscroll = first;
    let rows: Vec<(usize, &Entry)> =
        entries.iter().enumerate().skip(first).take(visible).collect();
    let len = entries.len();
    let playing = gui.app.now_playing.as_ref().map(|t| t.filepath.as_str());
    super::draw_pane_rows(
        frame,
        &mut gui.ui,
        playing,
        &rows,
        list,
        selected,
        Act::PlTrackRow,
        Act::PlTrackQueue,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right().saturating_sub(1), y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::PlScrollBy(-1),
        Act::PlScrollBy(1),
        Act::PlScrollTo,
    );
}

// ── Modals ──────────────────────────────────────────────────────────────────

pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if let Some(dialog) = &gui.playlists.dialog {
        let (title, action) = match &dialog.rename_of {
            Some(_) => (t!("gui.pl.rename_title"), t!("gui.pl.rename_verb")),
            None => (t!("gui.pl.new"), t!("gui.pl.create")),
        };
        let name = dialog.name.clone();
        gui.ui.click(area, Act::PlCancel);
        let inner = modal_frame(frame, area, 52, 7, th().accent);
        put(frame, inner.x + 1, inner.y, &title, accent().add_modifier(Modifier::BOLD));
        crate::kit::modal_close(frame, &mut gui.ui, inner, Act::PlCancel, t!("gui.srv.close_tip").to_string());
        put(
            frame,
            inner.x + 1,
            inner.y + 2,
            &input_display(&name, name.chars().count(), inner.width.saturating_sub(2)),
            Style::default(),
        );
        put(frame, inner.x + 1, inner.y + 4, &t!("gui.pl.dialog_hint"), dim());
        let rect = Rect {
            x: inner.right().saturating_sub(action.chars().count() as u16 + 1),
            y: inner.bottom().saturating_sub(1),
            width: action.chars().count() as u16,
            height: 1,
        };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        put(
            frame,
            rect.x,
            rect.y,
            &action,
            if hover { bright_bold() } else { accent().add_modifier(Modifier::BOLD) },
        );
        gui.ui.click(rect, Act::PlOk);
    }

    if let Some(name) = gui.playlists.confirm.clone() {
        // The delete gate: a warning modal — gold border, no [X], the safe
        // choice wearing the primary color (clause 30; the kit's law).
        gui.ui.click(area, Act::PlCancel);
        let inner = modal_frame(frame, area, 52, 6, th().gold);
        put(
            frame,
            inner.x + 1,
            inner.y,
            &t!("gui.pl.confirm_title"),
            Style::default().fg(th().gold).add_modifier(Modifier::BOLD),
        );
        put(
            frame,
            inner.x + 1,
            inner.y + 2,
            &super::bar::clip(&name, inner.width.saturating_sub(2) as usize),
            Style::default().add_modifier(Modifier::BOLD),
        );
        let del = t!("gui.pl.delete").to_string();
        let cancel = t!("gui.pl.cancel").to_string();
        let del_x = inner.right().saturating_sub(del.chars().count() as u16 + 1);
        let cancel_x = del_x.saturating_sub(cancel.chars().count() as u16 + 3);
        let y = inner.bottom().saturating_sub(1);
        let cancel_rect =
            Rect { x: cancel_x, y, width: cancel.chars().count() as u16, height: 1 };
        let chover = gui.ui.pointer.is_some_and(|p| cancel_rect.contains(p));
        put(
            frame,
            cancel_x,
            y,
            &cancel,
            if chover { bright_bold() } else { accent().add_modifier(Modifier::BOLD) },
        );
        gui.ui.click(cancel_rect, Act::PlCancel);
        let del_rect = Rect { x: del_x, y, width: del.chars().count() as u16, height: 1 };
        let dhover = gui.ui.pointer.is_some_and(|p| del_rect.contains(p));
        let dstyle = if dhover {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th().danger)
        };
        put(frame, del_x, y, &del, dstyle);
        gui.ui.click(del_rect, Act::PlConfirm);
    }
}

pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.playlists.dialog.is_some() || gui.playlists.confirm.is_some()
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// The name a list index holds, when it is a playlist row.
fn name_at(gui: &Gui, index: usize) -> Option<String> {
    match gui.app.library.entries.get(index) {
        Some(Entry::Node { label, .. }) => Some(label.clone()),
        _ => None,
    }
}

pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act.clone() {
        Act::PlNew => {
            gui.playlists.dialog = Some(PlDialog { rename_of: None, name: String::new() });
        }
        Act::PlRename(index) => {
            if let Some(name) = name_at(gui, index) {
                gui.playlists.dialog =
                    Some(PlDialog { rename_of: Some(name.clone()), name });
            }
        }
        Act::PlDelete(index) => {
            gui.playlists.confirm = name_at(gui, index);
        }
        Act::PlOk => {
            let Some(dialog) = gui.playlists.dialog.take() else {
                return true;
            };
            let name = dialog.name.trim().to_string();
            match dialog.rename_of {
                // Cancel, empty and the unchanged name are no-ops — the
                // record's dialog contract (clauses 10, 20).
                None if name.is_empty() => {}
                None => gui.pend(vec![crate::tui::app::Effect::Api(
                    ApiCmd::CreatePlaylist { name },
                )]),
                Some(old) if name.is_empty() || name == old => {}
                Some(old) => gui.pend(vec![crate::tui::app::Effect::Api(
                    ApiCmd::RenamePlaylist { from: old, to: name },
                )]),
            }
        }
        Act::PlConfirm => {
            if let Some(name) = gui.playlists.confirm.take() {
                gui.pend(vec![crate::tui::app::Effect::Api(ApiCmd::DeletePlaylist { name })]);
            }
        }
        Act::PlCancel => {
            gui.playlists.dialog = None;
            gui.playlists.confirm = None;
        }
        Act::PlRow(index) => {
            gui.app.tab = crate::tui::app::Tab::Library;
            gui.app.library.state.select(Some(index));
            gui.playlists.lreveal = true;
            gui.forward(Action::Activate);
            gui.playlists.tscroll = 0;
        }
        Act::PlTrackRow(index) => {
            gui.app.tab = crate::tui::app::Tab::Library;
            gui.app.library.state.select(Some(index));
            gui.playlists.treveal = true;
            gui.forward_capturing(Action::Activate);
        }
        Act::PlTrackQueue(index) => {
            gui.app.tab = crate::tui::app::Tab::Library;
            gui.app.library.state.select(Some(index));
            gui.forward(Action::AddToQueue);
        }
        Act::PlScrollBy(delta) => {
            let scroll = if drilled(gui).is_some() {
                &mut gui.playlists.tscroll
            } else {
                &mut gui.playlists.lscroll
            };
            *scroll = if delta < 0 { scroll.saturating_sub(1) } else { *scroll + 1 };
        }
        Act::PlScrollTo(first) => {
            if drilled(gui).is_some() {
                gui.playlists.tscroll = first;
            } else {
                gui.playlists.lscroll = first;
            }
        }
        _ => return false,
    }
    true
}

/// The room's wheel: whichever level is on screen scrolls (the router's
/// exhaustive match sent us here).
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    gui.act(Act::PlScrollBy(delta));
}

// ── Keys ────────────────────────────────────────────────────────────────────

pub(crate) fn handle_key(
    gui: &mut Gui,
    key: ratatui::crossterm::event::KeyEvent,
) -> Option<bool> {
    use ratatui::crossterm::event::KeyCode;

    // The dialogs first — a text field owns nearly every key, and the
    // delete gate answers only its own.
    if let Some(dialog) = &mut gui.playlists.dialog {
        match key.code {
            KeyCode::Enter => return Some(gui.act(Act::PlOk)),
            KeyCode::Esc => gui.playlists.dialog = None,
            KeyCode::Backspace => {
                dialog.name.pop();
            }
            KeyCode::Char(c) => dialog.name.push(c),
            _ => {}
        }
        return Some(false);
    }
    if gui.playlists.confirm.is_some() {
        match key.code {
            // x confirms — the same key that asked; Enter stays the safe
            // way out of a warning gate (the kit's primary is Cancel).
            KeyCode::Char('x') => return Some(gui.act(Act::PlConfirm)),
            KeyCode::Esc | KeyCode::Enter => return Some(gui.act(Act::PlCancel)),
            _ => {}
        }
        return Some(false);
    }
    if gui.active != super::PLAYLISTS_NAV || !gui.app.connected || !holds_playlists(gui) {
        return None;
    }

    match drilled(gui) {
        Some(_) => match key.code {
            KeyCode::Down => {
                gui.playlists.treveal = true;
                gui.forward(Action::Down);
            }
            KeyCode::Up => {
                gui.playlists.treveal = true;
                gui.forward(Action::Up);
            }
            KeyCode::Enter => gui.forward_capturing(Action::Activate),
            KeyCode::Char('h') | KeyCode::Backspace => gui.forward(Action::Back),
            KeyCode::Char('a') => gui.forward(Action::AddToQueue),
            _ => return None,
        },
        None => match key.code {
            // The list starts at row 1 — row 0 is the pane's Parent, a
            // road the nav replaces.
            KeyCode::Down => {
                let len = gui.app.library.entries.len();
                if len > 1 {
                    let next = match gui.app.library.state.selected() {
                        Some(i) => (i + 1).min(len - 1),
                        None => 1,
                    };
                    gui.app.library.state.select(Some(next.max(1)));
                    gui.playlists.lreveal = true;
                }
            }
            KeyCode::Up => {
                let len = gui.app.library.entries.len();
                if len > 1 {
                    let next = match gui.app.library.state.selected() {
                        Some(i) => i.saturating_sub(1).max(1),
                        None => len - 1,
                    };
                    gui.app.library.state.select(Some(next));
                    gui.playlists.lreveal = true;
                }
            }
            KeyCode::Esc => gui.app.library.state.select(None),
            KeyCode::Enter => {
                if let Some(i) = gui.app.library.state.selected() {
                    return Some(gui.act(Act::PlRow(i)));
                }
            }
            KeyCode::Char('n') => return Some(gui.act(Act::PlNew)),
            KeyCode::Char('e') => {
                if let Some(i) = gui.app.library.state.selected() {
                    return Some(gui.act(Act::PlRename(i)));
                }
            }
            KeyCode::Char('x') => {
                if let Some(i) = gui.app.library.state.selected() {
                    return Some(gui.act(Act::PlDelete(i)));
                }
            }
            _ => return None,
        },
    }
    Some(false)
}

/// The tips line for this room's states.
pub(crate) fn tips(gui: &Gui) -> String {
    if gui.playlists.dialog.is_some() {
        return t!("gui.tips.pl_dialog").to_string();
    }
    if gui.playlists.confirm.is_some() {
        return t!("gui.tips.pl_confirm").to_string();
    }
    if drilled(gui).is_some() {
        return t!("gui.tips.pl_tracks").to_string();
    }
    t!("gui.tips.pl_list").to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{PlaylistSummary, Track, TrackMetadata};
    use crate::config::Config;
    use crate::tui::app::{App, Effect};
    use crate::tui::app::Tab;
    use crate::tui::worker::{Event, LibraryData};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// A connected Gui on the Playlists section with `names` standing —
    /// arrived at the way a user arrives: the nav click asks, the list is
    /// the server's reply.
    fn pl_gui(names: &[&str]) -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.app.connected = true;
        gui.queue_open = false;
        gui.act(Act::Nav(super::super::PLAYLISTS_NAV));
        let playlists =
            names.iter().map(|n| PlaylistSummary { name: n.to_string() }).collect();
        let effects = gui.app.apply_event(Event::Library {
            node: LibraryNode::Playlists,
            dest: Tab::Library,
            data: LibraryData::Playlists(playlists),
        });
        gui.pend(effects);
        gui.pending.clear(); // the nav's own fetch is not under test
        gui
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| super::super::render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_list_leads_with_the_new_card_and_names_the_playlists() {
        let mut gui = pl_gui(&["Morning", "Road Trip", "Rainy Days"]);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("+ New playlist"), "the create action leads:\n{text}");
        for name in ["Morning", "Road Trip", "Rainy Days"] {
            assert!(text.contains(name), "missing {name:?}:\n{text}");
        }

        let mut gui = pl_gui(&[]);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("No playlists yet"), "the record's empty words:\n{text}");
        assert!(text.contains("+ New playlist"), "creating the first one is the point:\n{text}");
    }

    #[test]
    fn the_dialog_creates_and_respects_the_no_ops() {
        let mut gui = pl_gui(&["Morning"]);
        key(&mut gui, KeyCode::Char('n'));
        assert!(gui.playlists.dialog.is_some(), "n opens the name dialog");
        for c in "Dusk".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("New playlist"), "the dialog names its act:\n{text}");
        key(&mut gui, KeyCode::Enter);
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::CreatePlaylist { name }) if name == "Dusk"
            )),
            "create was asked for: {:?}",
            gui.pending
        );

        // Empty is a no-op (clause 10).
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('n'));
        key(&mut gui, KeyCode::Enter);
        assert!(gui.pending.is_empty(), "an empty name asks for nothing");
        assert!(gui.playlists.dialog.is_none());
    }

    #[test]
    fn rename_prefills_and_the_unchanged_name_is_a_no_op() {
        let mut gui = pl_gui(&["Morning"]);
        key(&mut gui, KeyCode::Char('e'));
        let dialog = gui.playlists.dialog.as_ref().expect("e opens rename");
        assert_eq!(dialog.name, "Morning", "prefilled with the current name");
        // Enter with the name untouched: a no-op (clause 20).
        key(&mut gui, KeyCode::Enter);
        assert!(gui.pending.is_empty(), "the unchanged name asks for nothing");

        key(&mut gui, KeyCode::Char('e'));
        key(&mut gui, KeyCode::Char('s'));
        key(&mut gui, KeyCode::Enter);
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::RenamePlaylist { from, to })
                    if from == "Morning" && to == "Mornings"
            )),
            "the rename was asked for: {:?}",
            gui.pending
        );
    }

    #[test]
    fn the_delete_gate_asks_and_x_answers() {
        // The pane arrives with its cursor on the first playlist — the
        // Files room's own resting state.
        let mut gui = pl_gui(&["Morning", "Road Trip"]);
        key(&mut gui, KeyCode::Char('x'));
        assert_eq!(gui.playlists.confirm.as_deref(), Some("Morning"));
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Confirm Delete Playlist"), "got:\n{text}");
        assert!(text.contains("Morning"), "the gate names what it asks about:\n{text}");

        // Enter is the safe way out of a warning gate.
        key(&mut gui, KeyCode::Enter);
        assert!(gui.playlists.confirm.is_none());
        assert!(gui.pending.is_empty(), "nothing deleted on the safe answer");

        // x again is the destructive yes.
        key(&mut gui, KeyCode::Char('x'));
        key(&mut gui, KeyCode::Char('x'));
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::DeletePlaylist { name }) if name == "Morning"
            )),
            "the delete was asked for: {:?}",
            gui.pending
        );
    }

    #[test]
    fn activation_drills_and_the_tracks_draw_the_shared_way() {
        let mut gui = pl_gui(&["Morning"]);
        key(&mut gui, KeyCode::Enter);
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::Library { node: LibraryNode::Playlist(name), .. })
                    if name == "Morning"
            )),
            "activation asks for the playlist's tracks: {:?}",
            gui.pending
        );
        let effects = gui.app.apply_event(Event::Library {
            node: LibraryNode::Playlist("Morning".into()),
            dest: Tab::Library,
            data: LibraryData::Tracks(vec![Track {
                filepath: "lib/one.mp3".into(),
                metadata: TrackMetadata {
                    title: Some("First Light".into()),
                    artist: Some("Vela".into()),
                    duration: Some(200.0),
                    ..TrackMetadata::default()
                },
            }]),
        });
        gui.pend(effects);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Playlists ▸ Morning"), "the crumb names the walk:\n{text}");
        assert!(text.contains("First Light"), "got:\n{text}");
        assert!(text.contains("3:20"), "durations ride the rows:\n{text}");
    }

    #[test]
    fn the_wheel_scrolls_whichever_level_is_up() {
        let names: Vec<String> = (0..30).map(|i| format!("List {i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut gui = pl_gui(&refs);
        draw(&mut gui);
        wheel(&mut gui, 1);
        wheel(&mut gui, 1);
        assert_eq!(gui.playlists.lscroll, 2, "the list level's wheel");
        assert_eq!(gui.playlists.tscroll, 0, "the drilled level stood still");
    }
}
