//! The Sonic path room: pick two songs, and the server fills the road
//! between them — the mobile app's screen (the design of record), worn
//! terminal on the shared `App` state machine the TUI's tab already drives.
//!
//! Contract: docs/ux-contracts/sonic-path.md (clause numbers below cite
//! it); boards: the "Sonic Path Room" canvas. Three things are done this
//! surface's way rather than the record's:
//!
//! * **Pick methods live on the card** as text buttons (no search sheet
//!   here — contract §5), and the keyboard reaches them through a small
//!   anchored menu (Enter on a card, `e`/`E` on the results chips).
//! * **Play sits rightmost** as the kit's one primary; the record pins it
//!   leftmost-expanded.
//! * **Endpoint edits in results rebuild immediately** (clause 22) for the
//!   in-place methods; a browse pick rides the capture round-trip back to
//!   setup, exactly the record's own split.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{dim, input_display, modal_close, modal_frame, scroll_list, table_view, tall_button, tall_secondary};
use crate::tui::app::{Action, Capture, SonicEmpty, SonicSide, SonicView, Tab};
use crate::tui::worker::ApiCmd;

use super::{Act, Gui, accent, bright_bold, put, sel};

/// The setup stage's keyboard rows, top to bottom.
const ROW_START: usize = 0;
const ROW_END: usize = 1;
const ROW_LENGTH: usize = 2;
const ROW_BUILD: usize = 3;
const SETUP_ROWS: usize = 4;

/// The length bar's cells — the TUI slider's width, drawn the gold way.
const LENGTH_CELLS: u32 = 10;

/// The room's own state: the pick-methods menu, the setup cursor, and the
/// results list's wheel — everything else lives on the shared App.
pub(crate) struct SonicUi {
    /// The pick-methods menu, when open, and which end it serves.
    pub menu: Option<SonicSide>,
    pub menu_row: usize,
    /// Setup keyboard cursor over [`ROW_START`]..[`ROW_BUILD`]. None is the
    /// kit's resting state — ↓ picks it up, Esc stows it.
    pub cursor: Option<usize>,
    /// Results list cursor + wheel, the Files pane's contract.
    pub rcursor: Option<usize>,
    pub scroll: usize,
    pub reveal: bool,
}

impl SonicUi {
    pub(crate) fn new() -> Self {
        SonicUi { menu: None, menu_row: 0, cursor: None, rcursor: None, scroll: 0, reveal: false }
    }
}

// ── Pure geometry ───────────────────────────────────────────────────────────

/// The meter's one cell: ⌈similarity × 8⌉ eighth-blocks — the record's
/// linear MatchMeter, worn as a glyph. `None` on the conhost floor, where
/// the bare percentage carries the reading alone.
pub(crate) fn meter_glyph(similarity: f64) -> Option<char> {
    if legacy_conhost() {
        return None;
    }
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let step = (similarity * 8.0).ceil().clamp(1.0, 8.0) as usize;
    Some(BLOCKS[step - 1])
}

/// What length a click on bar cell `i` means: the band's ends on the end
/// cells, proportional between.
pub(crate) fn cell_length(i: u32) -> u32 {
    use crate::api::types::{JOURNEY_MAX_LENGTH, JOURNEY_MIN_LENGTH};
    let span = JOURNEY_MAX_LENGTH - JOURNEY_MIN_LENGTH;
    JOURNEY_MIN_LENGTH + (i * span + (LENGTH_CELLS - 1) / 2) / (LENGTH_CELLS - 1)
}

/// How many bar cells `length` fills — the TUI slider's own sum.
pub(crate) fn filled_cells(length: u32) -> u32 {
    use crate::api::types::{JOURNEY_MAX_LENGTH, JOURNEY_MIN_LENGTH};
    let span = JOURNEY_MAX_LENGTH - JOURNEY_MIN_LENGTH;
    ((length - JOURNEY_MIN_LENGTH) * LENGTH_CELLS).div_ceil(span)
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The room. Dispatches on the shared App's view; the capture round-trip
/// never lands here (arming jumps to Files), so this draws Setup, Results,
/// or the disconnected note.
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    put(
        frame,
        content.x,
        content.y,
        &t!("gui.nav.sonic"),
        Style::default().add_modifier(Modifier::BOLD),
    );
    if !gui.app.connected {
        put(frame, content.x, content.y + 2, &t!("gui.no_server"), dim());
        return;
    }
    match gui.app.sonic.view {
        SonicView::Setup => draw_setup(frame, gui, content),
        SonicView::Results => draw_results(frame, gui, content),
    }
}

/// One endpoint card: 4 rows, the label in the top border, the pick
/// methods inside while empty, the pick (with its [X]) once filled.
/// The LightBlue border is the KEYBOARD cursor, the kit's law.
fn draw_card(frame: &mut Frame, gui: &mut Gui, at: Rect, side: SonicSide) {
    let has_cursor = gui.sonic.cursor
        == Some(match side {
            SonicSide::Start => ROW_START,
            SonicSide::End => ROW_END,
        });
    let border = if has_cursor { accent() } else { dim() };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(border)
        .title(ratatui::text::Span::styled(
            format!(" {} ", label_for(side).to_uppercase()),
            dim(),
        ));
    let inner = block.inner(at);
    frame.render_widget(block, at);

    match gui.app.sonic.side(side).cloned() {
        Some(track) => {
            let name_width = inner.width.saturating_sub(8) as usize;
            let title = track
                .metadata
                .title
                .clone()
                .unwrap_or_else(|| track.display_name());
            put(
                frame,
                inner.x + 1,
                inner.y,
                &super::bar::clip(&title, name_width),
                Style::default().add_modifier(Modifier::BOLD),
            );
            if let Some(artist) = track.metadata.artist.as_deref() {
                put(frame, inner.x + 1, inner.y + 1, &super::bar::clip(artist, name_width), dim());
            }
            // The per-row remove, the kit's [X]: dim at rest, danger red
            // under the pointer — clearing is destructive to the choice.
            let x_rect = Rect { x: inner.right().saturating_sub(4), y: inner.y, width: 3, height: 1 };
            let hover = gui.ui.pointer.is_some_and(|p| x_rect.contains(p));
            let style = if hover {
                Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
            } else {
                dim()
            };
            put(frame, x_rect.x, x_rect.y, "[X]", style);
            gui.ui.click(x_rect, Act::SonClear(side));
            // The card body re-opens the methods — a filled card hides the
            // buttons (a settled decision, webapp parity), the ✕ or another
            // pick is how it changes.
            let body = Rect { x: at.x, y: at.y, width: at.width.saturating_sub(5), height: at.height };
            gui.ui.click(body, Act::SonMenu(side));
        }
        None => {
            put(frame, inner.x + 1, inner.y, &t!("gui.sonic.not_set"), dim());
            // The three ways in, as 1-row text buttons (clauses 1, 2, 4 —
            // clause 3's search sheet is not ported, §5). When the queue
            // squeezes the room below the row's width, the card offers the
            // menu instead — the same three ways, with room to breathe.
            let labels = [
                (t!("gui.sonic.use_playing").to_string(), Act::SonUse(side)),
                (t!("gui.sonic.random").to_string(), Act::SonRandom(side)),
                (t!("gui.sonic.browse").to_string(), Act::SonBrowse(side)),
            ];
            let dot = " · ";
            let need: usize = labels.iter().map(|(l, _)| l.chars().count()).sum::<usize>()
                + 2 * dot.chars().count()
                + 2;
            if need > inner.width as usize {
                put(frame, inner.x + 1, inner.y + 1, &t!("gui.sonic.choose"), dim());
                gui.ui.click(at, Act::SonMenu(side));
                return;
            }
            let mut x = inner.x + 1;
            for (label, act) in labels {
                if x > inner.x + 1 {
                    put(frame, x, inner.y + 1, dot, dim());
                    x += dot.chars().count() as u16;
                }
                let rect =
                    Rect { x, y: inner.y + 1, width: label.chars().count() as u16, height: 1 };
                let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
                put(frame, x, inner.y + 1, &label, if hover { bright_bold() } else { dim() });
                gui.ui.click(rect, act);
                x += rect.width;
            }
        }
    }
}

fn label_for(side: SonicSide) -> String {
    match side {
        SonicSide::Start => t!("gui.sonic.start_song").to_string(),
        SonicSide::End => t!("gui.sonic.end_song").to_string(),
    }
}

/// The length row: label, the ten-cell bar (clickable, clause 20), the
/// count, and either the ←→ hint (setup) or Regenerate (results).
fn draw_length(frame: &mut Frame, gui: &mut Gui, at: Rect, in_results: bool) {
    let length = gui.app.sonic.length;
    let label = t!("gui.sonic.length").to_uppercase();
    let has_cursor = !in_results && gui.sonic.cursor == Some(ROW_LENGTH);
    put(frame, at.x, at.y, &label, if has_cursor { accent() } else { dim() });
    let bar_x = at.x + label.chars().count() as u16 + 2;
    let filled = filled_cells(length);
    let (on, off) = if legacy_conhost() { ('=', '-') } else { ('━', '─') };
    for i in 0..LENGTH_CELLS {
        let cell = Rect { x: bar_x + i as u16, y: at.y, width: 1, height: 1 };
        let lit = i < filled;
        let hover = gui.ui.pointer.is_some_and(|p| cell.contains(p));
        let style = match (hover, lit) {
            (true, _) => bright_bold(),
            (false, true) => Style::default().fg(th().gold),
            (false, false) => dim(),
        };
        put(frame, cell.x, at.y, &if lit { on } else { off }.to_string(), style);
        gui.ui.click(cell, Act::SonLen(cell_length(i)));
    }
    let count = if in_results {
        t!("gui.sonic.stops", count = length).to_string()
    } else {
        t!("gui.sonic.stops_both", count = length).to_string()
    };
    let count_x = bar_x + LENGTH_CELLS as u16 + 2;
    // The trailing piece — Regenerate or the ←→ hint — owns the row's
    // right edge; the count clips first so the action never does.
    let tail = if in_results {
        t!("gui.sonic.regenerate").to_string()
    } else {
        t!("gui.sonic.adjust").to_string()
    };
    let tail_w = tail.chars().count() as u16;
    let room = at.right().saturating_sub(count_x + tail_w + 3) as usize;
    put(frame, count_x, at.y, &super::bar::clip(&count, room), Style::default());
    let x = at.right().saturating_sub(tail_w + 1);
    if in_results {
        // Regenerate, a text button — disabled while a build is in flight
        // (clause 22), and hidden is worse than dim here: the row would
        // re-seat under a resting pointer.
        let rect = Rect { x, y: at.y, width: tail_w, height: 1 };
        let busy = gui.app.sonic.pending || gui.app.sonic.probe;
        let hover = !busy && gui.ui.pointer.is_some_and(|p| rect.contains(p));
        put(frame, x, at.y, &tail, if hover { bright_bold() } else { dim() });
        if !busy {
            gui.ui.click(rect, Act::SonRegen);
        }
    } else {
        put(frame, x, at.y, &tail, dim());
    }
}

fn draw_setup(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    put(
        frame,
        content.x,
        content.y + 1,
        &super::bar::clip(&t!("gui.sonic.hint"), content.width as usize),
        dim(),
    );

    let card_w = content.width.min(60);
    // Roomy: cards at 3 with the ↓ between; tight (the 100×24 floor):
    // packed from 2 with no arrow. Both keep Build anchored bottom-right.
    let roomy = content.height >= 17;
    let (start_y, arrow, end_y, len_y) = if roomy {
        (content.y + 3, true, content.y + 8, content.y + 13)
    } else {
        (content.y + 2, false, content.y + 6, content.y + 10)
    };
    draw_card(
        frame,
        gui,
        Rect { x: content.x, y: start_y, width: card_w, height: 4 },
        SonicSide::Start,
    );
    if arrow {
        let glyph = if legacy_conhost() { "v" } else { "↓" };
        put(frame, content.x + card_w / 2, start_y + 4, glyph, dim());
    }
    draw_card(
        frame,
        gui,
        Rect { x: content.x, y: end_y, width: card_w, height: 4 },
        SonicSide::End,
    );
    draw_length(
        frame,
        gui,
        Rect { x: content.x, y: len_y, width: card_w, height: 1 },
        false,
    );

    // The kit's one primary, bottom-right. The label keeps its width in
    // both states so the frame never re-seats; disabled still tips WHY —
    // the one exception to disabled inertness.
    let ready = gui.app.sonic.ready();
    let forward = if legacy_conhost() { '>' } else { '▸' };
    let label = if ready {
        format!("{} {forward}", t!("gui.sonic.build"))
    } else {
        format!("{}  ", t!("gui.sonic.build"))
    };
    let width = label.chars().count() as u16 + 6;
    let at = Rect {
        x: content.right().saturating_sub(width),
        y: content.bottom().saturating_sub(3),
        width,
        height: 3,
    };
    let rect = tall_button(frame, &mut gui.ui, at, &label, ready, Act::SonBuild);
    if !ready {
        gui.ui.tip(rect, t!("gui.sonic.build_first").to_string());
    }
    if gui.sonic.cursor == Some(ROW_BUILD) {
        // The keyboard cursor on a frame the pointer isn't in: the kit
        // marks it the accent way without stealing the hover contract.
        frame.render_widget(
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(if ready { accent() } else { dim().add_modifier(Modifier::BOLD) }),
            rect,
        );
    }
}

/// One results chip: the end's name, clickable to re-open the methods
/// (clause 30 — the record re-picks via search; here the menu is the
/// remaining methods).
fn draw_chip(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, side: SonicSide, max: usize) -> u16 {
    let name = gui
        .app
        .sonic
        .side(side)
        .map(|track| track.display_name())
        .unwrap_or_else(|| t!("gui.sonic.not_set").to_string());
    let text = super::bar::clip(&name, max);
    let rect = Rect { x, y, width: text.chars().count() as u16, height: 1 };
    let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
    let style = if hover {
        bright_bold()
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    put(frame, x, y, &text, style);
    gui.ui.click(rect, Act::SonMenu(side));
    x + rect.width
}

fn draw_results(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    // Start over, a text button on the title row.
    let over = t!("gui.sonic.start_over").to_string();
    let over_x = content.right().saturating_sub(over.chars().count() as u16);
    let over_rect =
        Rect { x: over_x, y: content.y, width: over.chars().count() as u16, height: 1 };
    let hover = gui.ui.pointer.is_some_and(|p| over_rect.contains(p));
    put(frame, over_x, content.y, &over, if hover { bright_bold() } else { dim() });
    gui.ui.click(over_rect, Act::SonStartOver);

    // The chips: start → end, editable in place in every result state
    // (clause 30).
    let per = (content.width as usize).saturating_sub(7) / 2;
    let x = draw_chip(frame, gui, content.x, content.y + 1, SonicSide::Start, per);
    let arrow = if legacy_conhost() { " -> " } else { "  →  " };
    put(frame, x, content.y + 1, arrow, dim());
    draw_chip(
        frame,
        gui,
        x + arrow.chars().count() as u16,
        content.y + 1,
        SonicSide::End,
        per,
    );

    draw_length(
        frame,
        gui,
        Rect { x: content.x, y: content.y + 3, width: content.width, height: 1 },
        true,
    );

    let sonic = &gui.app.sonic;
    let body = Rect {
        x: content.x,
        y: content.y + 5,
        width: content.width,
        height: content.height.saturating_sub(5 + 4),
    };

    // The failure taxonomy (clauses 40–44): pending and probe are quiet
    // busy lines; an empty answer names itself, and Retry appears exactly
    // where a retry can change the answer.
    if sonic.pending || sonic.probe {
        let busy =
            if sonic.probe { t!("gui.sonic.probing") } else { t!("gui.sonic.plotting") };
        put(frame, body.x, body.y, &busy, accent());
        return;
    }
    if sonic.stops.is_empty() {
        let note = sonic
            .note
            .clone()
            .unwrap_or_else(|| t!("gui.sonic.no_matches").to_string());
        let style = match sonic.empty {
            SonicEmpty::TurnedOff | SonicEmpty::NotAnalyzed => Style::default().fg(th().gold),
            SonicEmpty::Plain | SonicEmpty::ScanPending => dim(),
        };
        for (i, line) in wrap(&note, body.width as usize).into_iter().enumerate() {
            if i as u16 >= body.height {
                break;
            }
            put(frame, body.x, body.y + i as u16, &line, style);
        }
        if sonic.fetched
            && matches!(sonic.empty, SonicEmpty::Plain | SonicEmpty::ScanPending)
        {
            let retry = t!("gui.sonic.retry").to_string();
            let rect = Rect {
                x: body.x,
                y: body.y + 3,
                width: retry.chars().count() as u16,
                height: 1,
            };
            let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
            put(
                frame,
                rect.x,
                rect.y,
                &retry,
                if hover {
                    bright_bold()
                } else {
                    accent().add_modifier(Modifier::BOLD)
                },
            );
            gui.ui.click(rect, Act::SonRetry);
        }
        return;
    }

    // The journey. Header + rule, the kit table's way; the MATCH column
    // shares one right edge with the meters and the seed tags below it.
    let list = Rect {
        x: body.x,
        y: body.y + 2,
        width: body.width.saturating_sub(2),
        height: body.height.saturating_sub(2),
    };
    let match_col = t!("gui.sonic.match_col").to_uppercase();
    put(frame, body.x, body.y, &t!("gui.sonic.track_col").to_uppercase(), dim());
    put(
        frame,
        list.right().saturating_sub(match_col.chars().count() as u16 + 1),
        body.y,
        &match_col,
        dim(),
    );
    put(frame, body.x, body.y + 1, &"─".repeat(list.width as usize), dim());
    let len = sonic.stops.len();
    let reveal = gui.sonic.reveal.then_some(gui.sonic.rcursor).flatten();
    gui.sonic.reveal = false;
    let (first, visible) = table_view(len, reveal, gui.sonic.scroll, list.height as usize);
    gui.sonic.scroll = first;

    let last = len.saturating_sub(1);
    for (row, index) in (first..(first + visible).min(len)).enumerate() {
        let stop = &gui.app.sonic.stops[index];
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        let is_sel = gui.sonic.rcursor == Some(index);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let seed = index == 0 || index == last;
        let style = match (is_sel, seed, hover) {
            (true, _, _) => sel().add_modifier(Modifier::BOLD),
            (false, true, false) => accent().add_modifier(Modifier::BOLD),
            (false, _, true) => bright_bold(),
            (false, false, false) => Style::default(),
        };
        let name_width = list.width.saturating_sub(12) as usize;
        put(frame, list.x, y, &super::bar::clip(&stop.metadata_display(), name_width), style);

        // The trailing column: seeds wear their tag, waypoints the meter
        // (clause 31), all flush to the MATCH header's right edge; the
        // hovered row reveals the [+] queue-add beside it.
        if hover && !is_sel {
            let plus = Rect { x: rect.right().saturating_sub(11), y, width: 3, height: 1 };
            put(frame, plus.x, y, "[+]", dim());
            gui.ui.click(rect, Act::SonRow(index));
            gui.ui.click(plus, Act::SonQueueStop(index));
            gui.ui.tip(plus, t!("gui.files.queue_tip").to_string());
        } else {
            gui.ui.click(rect, Act::SonRow(index));
        }
        if seed {
            let tag = if index == 0 {
                t!("gui.sonic.start_tag").to_string()
            } else {
                t!("gui.sonic.end_tag").to_string()
            };
            let tag_style = if is_sel { sel() } else { dim() };
            put(frame, rect.right().saturating_sub(tag.chars().count() as u16 + 1), y, &tag, tag_style);
        } else {
            let pct = format!("{:>3}", (stop.similarity * 100.0).round() as u32);
            let pct_x = rect.right().saturating_sub(4);
            match meter_glyph(stop.similarity) {
                Some(glyph) => {
                    put(
                        frame,
                        pct_x - 2,
                        y,
                        &glyph.to_string(),
                        if is_sel { sel() } else { accent() },
                    );
                    put(frame, pct_x, y, &pct, if is_sel { sel() } else { dim() });
                }
                None => put(frame, pct_x, y, &pct, if is_sel { sel() } else { dim() }),
            }
        }
    }
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right().saturating_sub(1), y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::SonScrollBy(-1),
        Act::SonScrollBy(1),
        Act::SonScrollTo,
    );

    // The pinned verbs (clause 34): Play is the kit's one primary and sits
    // rightmost — Queue all and Save keep it honest as tall secondaries.
    let y = content.bottom().saturating_sub(3);
    let forward = if legacy_conhost() { '>' } else { '▸' };
    let play = format!("{} {forward}", t!("gui.sonic.play"));
    let queue = t!("gui.sonic.queue_all").to_string();
    let save = t!("gui.sonic.save").to_string();
    let w = |label: &str| label.chars().count() as u16 + 6;
    let play_x = content.right().saturating_sub(w(&play));
    let queue_x = play_x.saturating_sub(w(&queue) + 1);
    let save_x = queue_x.saturating_sub(w(&save) + 1);
    tall_secondary(
        frame,
        &mut gui.ui,
        Rect { x: save_x, y, width: w(&save), height: 3 },
        &save,
        Act::SonSave,
    );
    tall_secondary(
        frame,
        &mut gui.ui,
        Rect { x: queue_x, y, width: w(&queue), height: 3 },
        &queue,
        Act::SonQueueAll,
    );
    tall_button(
        frame,
        &mut gui.ui,
        Rect { x: play_x, y, width: w(&play), height: 3 },
        &play,
        true,
        Act::SonPlay,
    );
}

/// Greedy word wrap for the failure sentences — the kit tooltip's habit,
/// simplified: these notes are one or two lines at any sane width.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let need = if line.is_empty() { word.chars().count() } else { line.chars().count() + 1 + word.chars().count() };
        if need > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// ── Modals ──────────────────────────────────────────────────────────────────

/// The pick-methods menu and the save-as-playlist prompt. Drawn (and
/// registered) after the base screen so their rects win the pointer; a
/// whole-screen guard makes the room beneath inert.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if let Some(side) = gui.sonic.menu {
        gui.ui.click(area, Act::SonMenuClose);
        let inner = modal_frame(frame, area, 44, 7, th().accent);
        put(
            frame,
            inner.x + 1,
            inner.y,
            &label_for(side),
            accent().add_modifier(Modifier::BOLD),
        );
        modal_close(frame, &mut gui.ui, inner, Act::SonMenuClose, t!("gui.srv.close_tip").to_string());
        let playing = gui
            .app
            .now_playing
            .as_ref()
            .map(|track| track.display_name())
            .unwrap_or_else(|| t!("gui.sonic.nothing_playing").to_string());
        let rows: [(String, Option<String>, Act); 3] = [
            (t!("gui.sonic.use_playing").to_string(), Some(playing), Act::SonUse(side)),
            (t!("gui.sonic.random").to_string(), None, Act::SonRandom(side)),
            (t!("gui.sonic.browse").to_string(), None, Act::SonBrowse(side)),
        ];
        for (i, (label, detail, act)) in rows.into_iter().enumerate() {
            let y = inner.y + 2 + i as u16;
            let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
            let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
            let is_sel = gui.sonic.menu_row == i;
            if is_sel {
                frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
            }
            let style = match (is_sel, hover) {
                (true, _) => sel().add_modifier(Modifier::BOLD),
                (false, true) => bright_bold(),
                (false, false) => Style::default(),
            };
            put(frame, inner.x + 1, y, &label, style);
            if let Some(detail) = detail {
                let x = inner.x + 2 + label.chars().count() as u16;
                let width = inner.width.saturating_sub(x - inner.x + 1) as usize;
                put(
                    frame,
                    x,
                    y,
                    &super::bar::clip(&detail, width),
                    if is_sel { sel() } else { dim() },
                );
            }
            gui.ui.click(rect, act);
        }
    }

    if let Some(name) = gui.app.sonic_playlist_name.clone() {
        gui.ui.click(area, Act::SonSaveCancel);
        let inner = modal_frame(frame, area, 52, 7, th().accent);
        put(
            frame,
            inner.x + 1,
            inner.y,
            &t!("gui.sonic.save_title"),
            accent().add_modifier(Modifier::BOLD),
        );
        modal_close(frame, &mut gui.ui, inner, Act::SonSaveCancel, t!("gui.srv.close_tip").to_string());
        // The prompt rides the App's own line state: keys forward to it,
        // so the TUI and GUI share one editing truth.
        put(
            frame,
            inner.x + 1,
            inner.y + 2,
            &input_display(&name, name.chars().count(), inner.width.saturating_sub(2)),
            Style::default(),
        );
        put(frame, inner.x + 1, inner.y + 4, &t!("gui.sonic.save_hint"), dim());
        let ok = t!("gui.sonic.save_ok").to_string();
        let rect = Rect {
            x: inner.right().saturating_sub(ok.chars().count() as u16 + 1),
            y: inner.bottom().saturating_sub(1),
            width: ok.chars().count() as u16,
            height: 1,
        };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        put(
            frame,
            rect.x,
            rect.y,
            &ok,
            if hover { bright_bold() } else { accent().add_modifier(Modifier::BOLD) },
        );
        gui.ui.click(rect, Act::SonSaveOk);
    }
}

pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.sonic.menu.is_some() || gui.app.sonic_playlist_name.is_some()
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// Handle the room's actions. Returns true when the act was this room's.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act.clone() {
        Act::SonMenu(side) => {
            gui.sonic.menu = Some(side);
            gui.sonic.menu_row = 0;
        }
        Act::SonMenuClose => gui.sonic.menu = None,
        Act::SonUse(side) => {
            gui.sonic.menu = None;
            let Some(track) = gui.app.now_playing.clone() else {
                gui.note = Some((t!("gui.sonic.nothing_playing").to_string(), true));
                return true;
            };
            end_edit(gui, side, track);
        }
        Act::SonRandom(side) => {
            gui.sonic.menu = None;
            // The pick lands as an event; rebuild-on-edit (clause 22) rides
            // the event loop's capture watch, same as a browse pick.
            gui.pend(vec![crate::tui::app::Effect::Api(ApiCmd::SonicRandom { side })]);
        }
        Act::SonBrowse(side) => {
            gui.sonic.menu = None;
            let effects = gui.app.arm_capture(Capture::Sonic(side));
            gui.pend(effects);
            gui.active = super::FILES_NAV;
            gui.app.tab = Tab::Files;
        }
        Act::SonClear(side) => {
            gui.app.sonic.set_side(side, None);
        }
        Act::SonLen(stops) => {
            let delta = stops as i32 - gui.app.sonic.length as i32;
            let effects = gui.app.adjust_sonic_length(delta);
            gui.pend(effects);
        }
        Act::SonBuild | Act::SonRegen | Act::SonRetry => {
            if gui.app.sonic.ready() {
                let effects = gui.app.build_sonic_path();
                gui.pend(effects);
                gui.sonic.rcursor = None;
                gui.sonic.scroll = 0;
            }
        }
        Act::SonStartOver => {
            gui.app.reset_sonic_path();
            gui.sonic.cursor = None;
            gui.sonic.rcursor = None;
            gui.sonic.scroll = 0;
        }
        Act::SonRow(index) => {
            let effects = gui.app.play_sonic_from(index);
            gui.pend(effects);
            gui.sonic.rcursor = Some(index);
        }
        Act::SonQueueStop(index) => {
            let effects = gui.app.queue_sonic_stop(index);
            gui.pend(effects);
        }
        Act::SonScrollBy(delta) => {
            let len = gui.app.sonic.stops.len();
            gui.sonic.scroll =
                gui.sonic.scroll.saturating_add_signed(delta as isize).min(len.saturating_sub(1));
        }
        Act::SonScrollTo(first) => gui.sonic.scroll = first,
        Act::SonPlay => {
            let effects = gui.app.play_sonic_from(0);
            gui.pend(effects);
        }
        Act::SonQueueAll => {
            let effects = gui.app.activate_sonic_row(crate::tui::app::SonicRow::QueueAll);
            gui.pend(effects);
        }
        Act::SonSave => {
            let effects = gui.app.activate_sonic_row(crate::tui::app::SonicRow::SavePlaylist);
            gui.pend(effects);
        }
        Act::SonSaveOk => gui.forward(Action::Submit),
        Act::SonSaveCancel => gui.forward(Action::Cancel),
        _ => return false,
    }
    true
}

/// An endpoint changed in place. In results the old journey is wrong the
/// moment its anchor moved, so the rebuild is immediate (clause 22); in
/// setup it just fills the card.
fn end_edit(gui: &mut Gui, side: SonicSide, track: crate::api::types::Track) {
    let was_results = gui.app.sonic.view == SonicView::Results;
    let effects = gui.app.capture_sonic_side(side, track);
    gui.pend(effects);
    if was_results && gui.app.sonic.ready() {
        let effects = gui.app.build_sonic_path();
        gui.pend(effects);
    }
}

/// A random pick landed while results were up: rebuild, the same clause-22
/// promise `end_edit` keeps for the in-place methods. Called by the event
/// loop right after the App consumes [`Event::SonicRandom`].
pub(crate) fn random_landed(gui: &mut Gui, was_results: bool) {
    if was_results && gui.app.sonic.view == SonicView::Setup && gui.app.sonic.ready() {
        let effects = gui.app.build_sonic_path();
        gui.pend(effects);
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The room's keys (and its modals', which outrank everything). Returns
/// `Some(quit)` when the key was consumed.
pub(crate) fn handle_key(
    gui: &mut Gui,
    key: ratatui::crossterm::event::KeyEvent,
) -> Option<bool> {
    use ratatui::crossterm::event::KeyCode;

    // The save prompt first: it is a text field, so almost every key is
    // its own (the App's line editor consumes them).
    if gui.app.sonic_playlist_name.is_some() {
        match key.code {
            KeyCode::Enter => gui.forward(Action::Submit),
            KeyCode::Esc => gui.forward(Action::Cancel),
            KeyCode::Backspace => gui.forward(Action::Backspace),
            KeyCode::Char(c) => gui.forward(Action::Input(c)),
            _ => {}
        }
        return Some(false);
    }
    if let Some(side) = gui.sonic.menu {
        match key.code {
            KeyCode::Esc => gui.sonic.menu = None,
            KeyCode::Up => gui.sonic.menu_row = gui.sonic.menu_row.saturating_sub(1),
            KeyCode::Down => gui.sonic.menu_row = (gui.sonic.menu_row + 1).min(2),
            KeyCode::Enter => {
                let act_for = [Act::SonUse(side), Act::SonRandom(side), Act::SonBrowse(side)];
                let chosen = act_for[gui.sonic.menu_row].clone();
                return Some(gui.act(chosen));
            }
            _ => {}
        }
        return Some(false);
    }
    if gui.active != super::SONIC_NAV {
        return None;
    }

    match gui.app.sonic.view {
        SonicView::Setup => match key.code {
            KeyCode::Down => {
                gui.sonic.cursor = Some(gui.sonic.cursor.map_or(0, |c| (c + 1).min(SETUP_ROWS - 1)));
            }
            KeyCode::Up => {
                gui.sonic.cursor =
                    Some(gui.sonic.cursor.map_or(SETUP_ROWS - 1, |c| c.saturating_sub(1)));
            }
            KeyCode::Esc => gui.sonic.cursor = None,
            KeyCode::Enter => match gui.sonic.cursor {
                Some(ROW_START) => return Some(gui.act(Act::SonMenu(SonicSide::Start))),
                Some(ROW_END) => return Some(gui.act(Act::SonMenu(SonicSide::End))),
                Some(ROW_LENGTH) => {
                    let effects = gui.app.adjust_sonic_length(1);
                    gui.pend(effects);
                }
                Some(ROW_BUILD) => return Some(gui.act(Act::SonBuild)),
                _ => {}
            },
            KeyCode::Char('x') => match gui.sonic.cursor {
                Some(ROW_START) => return Some(gui.act(Act::SonClear(SonicSide::Start))),
                Some(ROW_END) => return Some(gui.act(Act::SonClear(SonicSide::End))),
                _ => {}
            },
            KeyCode::Left if gui.sonic.cursor == Some(ROW_LENGTH) => {
                let effects = gui.app.adjust_sonic_length(-1);
                gui.pend(effects);
            }
            KeyCode::Right if gui.sonic.cursor == Some(ROW_LENGTH) => {
                let effects = gui.app.adjust_sonic_length(1);
                gui.pend(effects);
            }
            KeyCode::Char('b') => return Some(gui.act(Act::SonBuild)),
            _ => return None,
        },
        SonicView::Results => match key.code {
            KeyCode::Down => {
                let len = gui.app.sonic.stops.len();
                if len > 0 {
                    gui.sonic.rcursor =
                        Some(gui.sonic.rcursor.map_or(0, |c| (c + 1).min(len - 1)));
                    gui.sonic.reveal = true;
                }
            }
            KeyCode::Up => {
                let len = gui.app.sonic.stops.len();
                if len > 0 {
                    gui.sonic.rcursor =
                        Some(gui.sonic.rcursor.map_or(len - 1, |c| c.saturating_sub(1)));
                    gui.sonic.reveal = true;
                }
            }
            KeyCode::Esc => gui.sonic.rcursor = None,
            KeyCode::Enter => {
                if let Some(index) = gui.sonic.rcursor {
                    return Some(gui.act(Act::SonRow(index)));
                }
            }
            KeyCode::Char('a') => {
                if let Some(index) = gui.sonic.rcursor {
                    return Some(gui.act(Act::SonQueueStop(index)));
                }
            }
            KeyCode::Char('e') => return Some(gui.act(Act::SonMenu(SonicSide::End))),
            KeyCode::Char('E') => return Some(gui.act(Act::SonMenu(SonicSide::Start))),
            KeyCode::Char('r') => return Some(gui.act(Act::SonRegen)),
            KeyCode::Char('s') => return Some(gui.act(Act::SonSave)),
            KeyCode::Char('o') => return Some(gui.act(Act::SonStartOver)),
            KeyCode::Left => {
                let effects = gui.app.adjust_sonic_length(-1);
                gui.pend(effects);
            }
            KeyCode::Right => {
                let effects = gui.app.adjust_sonic_length(1);
                gui.pend(effects);
            }
            _ => return None,
        },
    }
    Some(false)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{JOURNEY_MAX_LENGTH, JOURNEY_MIN_LENGTH};

    #[test]
    fn the_meter_is_the_records_linear_fill_in_one_cell() {
        // ⌈similarity × 8⌉, the spec board's own formula.
        assert_eq!(meter_glyph(0.05), Some('▁'));
        assert_eq!(meter_glyph(0.5), Some('▄'));
        assert_eq!(meter_glyph(0.84), Some('▇'));
        assert_eq!(meter_glyph(0.95), Some('█'));
        assert_eq!(meter_glyph(1.0), Some('█'));
        assert_eq!(meter_glyph(0.0), Some('▁'), "zero clamps to the lowest block, not nothing");
    }

    #[test]
    fn length_cells_span_the_band_end_to_end() {
        assert_eq!(cell_length(0), JOURNEY_MIN_LENGTH, "the first cell is the band's floor");
        assert_eq!(
            cell_length(LENGTH_CELLS - 1),
            JOURNEY_MAX_LENGTH,
            "the last cell is the ceiling"
        );
        for i in 1..LENGTH_CELLS {
            assert!(cell_length(i) >= cell_length(i - 1), "cells read left to right");
        }
        // Round-trip: the default length lights the cells the TUI's own
        // slider sum lights.
        assert_eq!(filled_cells(crate::api::types::JOURNEY_DEFAULT_LENGTH), 4);
        assert_eq!(filled_cells(JOURNEY_MIN_LENGTH), 0);
        assert_eq!(filled_cells(JOURNEY_MAX_LENGTH), LENGTH_CELLS);
    }

    #[test]
    fn the_failure_wrap_keeps_words_whole() {
        let lines = wrap("a sentence that needs to break somewhere sensible", 20);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.chars().count() <= 20), "{lines:?}");
    }
}

/// The tips line for this room's states.
pub(crate) fn tips(gui: &Gui) -> String {
    if gui.app.sonic_playlist_name.is_some() {
        return t!("gui.tips.sonic_save").to_string();
    }
    if gui.sonic.menu.is_some() {
        return t!("gui.tips.sonic_menu").to_string();
    }
    match gui.app.sonic.view {
        SonicView::Setup => t!("gui.tips.sonic_setup").to_string(),
        SonicView::Results => t!("gui.tips.sonic_results").to_string(),
    }
}
