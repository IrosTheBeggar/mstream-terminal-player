//! The Peer recs room's grid (peer-recs contract §1.7): the shown tab's leads in two columns,
//! column-major in screen blocks, under the heads and the room's rule.
//!
//! The list is the content less its scrollbar lane, and each column is `cw = (list − 2) / 2`
//! cells wide with a 2-cell gap between them: 38 at 100 columns, 21 with the queue open, where
//! ARTIST drops. A block holds twice the visible rows; column A takes `ceil(m / 2)` of a block's
//! `m` leads and column B the rest, so 12 leads read 6 | 6 and 5 read 3 | 2 at every size, and
//! 32 leads at the 100×24 floor read 11 | 11, then 5 | 5 under the scrollbar (the web's
//! `columnBlocks`).
//!
//! A cell is the lead's swatch `▌` in its tile's hue, TRACK, ARTIST dim and the play slot. The
//! slot follows the room's one clip (peer-recs contract §1.8): `▸` accent at rest, `▮▮` while it
//! sounds, `▸` accent BOLD while paused, `◌` gold while it is found or waits out a 429, and `▸`
//! dim when the plug-in found nothing this visit. The live lead's title is accent BOLD. The held
//! cell is the kit's slab across the whole cell, the swatch keeping its hue on it.
//!
//! The cursor is a lead index (build plan D22); the room maps it to a cell and back through
//! [`cell_of`] and [`index_at`], and walks it with [`walk`].

use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::TasteLead;
use crate::kit::theme::{busy_glyph, swatch, swatch_glyph, th};
use crate::kit::{Surface, dim, scroll_list};
use crate::tui::app::App;
use crate::tui::app::peer_recs::previews::Slot;
use crate::tui::app::peer_recs::rules::peer_text;

use super::super::bar::{clip, play_glyphs};
use super::super::discover::dash;
use super::super::{Act, List, accent, bright_bold, put, sel};
use super::PeerAct;

/// A column's width at 100 columns, and the narrowest that still holds ARTIST.
const CELL_W: u16 = 38;
/// The swatch and the blank after it, before TRACK.
const LEAD: u16 = 2;
/// Between TRACK and ARTIST.
const GAP: u16 = 2;
/// The play slot's cells, at the cell's end, a blank before it.
const SLOT_W: u16 = 2;

/// Where the grid stands this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GridGeom {
    /// The leads' rect: the content less its scrollbar lane and the blank before it.
    pub list: Rect,
    /// A column's width.
    pub cw: u16,
    /// Columns A and B's first cells.
    pub ax: u16,
    pub bx: u16,
    /// Too narrow for ARTIST (the queue open): TRACK and the slot only.
    pub narrow: bool,
    /// The play slot is drawn: a preview plug-in and an audio device.
    pub slot: bool,
    /// The visible rows: a screen block holds twice as many leads.
    pub rows: u16,
}

/// The grid in `list`, the slot drawn when `slot`.
pub(super) fn geom(list: Rect, slot: bool) -> GridGeom {
    let cw = list.width.saturating_sub(2) / 2;
    GridGeom {
        list,
        cw,
        ax: list.x,
        bx: list.x.saturating_add(cw + 2),
        narrow: cw < CELL_W,
        slot,
        rows: list.height,
    }
}

/// A cell's parts, as offsets from its first cell: TRACK's and ARTIST's place and width, and
/// the slot's place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Parts {
    track: (u16, u16),
    artist: Option<(u16, u16)>,
    slot: Option<u16>,
}

/// TRACK at +2; ARTIST at +19 (15 cells of TRACK and a 2-cell gap at 38), to the slot's blank
/// or, with no slot, to the cell's end; the slot at the cell's last 2 cells. A narrow cell has
/// no ARTIST, and TRACK runs to the slot's blank or the cell's end (peer-recs contract §1.7).
fn parts(geom: &GridGeom) -> Parts {
    let cw = geom.cw;
    let tail = if geom.slot { 1 + SLOT_W } else { 0 };
    let slot = geom.slot.then(|| cw.saturating_sub(SLOT_W));
    if geom.narrow {
        return Parts { track: (LEAD, cw.saturating_sub(LEAD + tail)), artist: None, slot };
    }
    let track_w = cw.saturating_sub(LEAD + GAP + 1 + SLOT_W) / 2;
    let artist_x = LEAD + track_w + GAP;
    Parts { track: (LEAD, track_w), artist: Some((artist_x, cw.saturating_sub(artist_x + tail))), slot }
}

/// Where TRACK and ARTIST start in a cell, as offsets (ARTIST `None` in a narrow cell): the
/// heads' places, which the panes' skeleton runs stand under.
pub(super) fn head_offsets(geom: &GridGeom) -> (u16, Option<u16>) {
    let parts = parts(geom);
    (parts.track.0, parts.artist.map(|(x, _)| x))
}

// ── Blocks: the lead ↔ cell maps ─────────────────────────────────────────────

/// The visible rows, at least one, so a block is never empty.
fn per_column(rows: u16) -> usize {
    usize::from(rows.max(1))
}

/// The grid's rows for `n` leads at `rows` visible rows: `Σ ceil(m_k / 2)` over the blocks.
/// Every block but the last is full, `rows` rows each.
pub(super) fn total_rows(n: usize, rows: u16) -> usize {
    let v = per_column(rows);
    (n / (2 * v)) * v + (n % (2 * v)).div_ceil(2)
}

/// The cell of the lead at `index` (of `n`): its grid row, across blocks, and its column, 0 for
/// A and 1 for B.
pub(super) fn cell_of(index: usize, n: usize, rows: u16) -> (usize, u8) {
    let v = per_column(rows);
    let block = index / (2 * v);
    let start = block * 2 * v;
    let a = n.saturating_sub(start).min(2 * v).div_ceil(2);
    let local = index - start;
    if local < a { (block * v + local, 0) } else { (block * v + local - a, 1) }
}

/// The lead in grid row `row`, column `col`; `None` for a hole (column B's short end in an odd
/// block, rows past the last lead).
pub(super) fn index_at(row: usize, col: u8, n: usize, rows: u16) -> Option<usize> {
    let v = per_column(rows);
    let (block, local) = (row / v, row % v);
    let start = block * 2 * v;
    if start >= n {
        return None;
    }
    let m = (n - start).min(2 * v);
    let a = m.div_ceil(2);
    match col {
        0 => (local < a).then_some(start + local),
        _ => (local < m - a).then_some(start + a + local),
    }
}

/// The last grid row that holds a lead in `col`.
fn last_row(col: u8, n: usize, rows: u16) -> Option<usize> {
    (0..total_rows(n, rows)).rev().find(|&row| index_at(row, col, n, rows).is_some())
}

/// Where a walking key takes the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Walk {
    /// To this lead.
    To(usize),
    /// Up to the tabs stop.
    Tabs,
    /// Nowhere: an edge, a hole, a key the grid does not walk.
    Stay,
}

/// A walking key from the lead at `from` (build plan D22): ↑ ↓ walk the column across blocks,
/// ↑ from either column's first row climbing to the tabs stop; ← → go to the other column on the
/// same row, or to that column's last lead when it is shorter, and do nothing at the edges; PgUp
/// and PgDn move one screen of rows, clamped to the column. ↓ into a hole stays.
pub(super) fn walk(from: usize, key: KeyCode, n: usize, rows: u16) -> Walk {
    if from >= n {
        return Walk::Stay;
    }
    let v = per_column(rows);
    let (row, col) = cell_of(from, n, rows);
    let to = |row: usize, col: u8| index_at(row, col, n, rows).map_or(Walk::Stay, Walk::To);
    match key {
        KeyCode::Up if row == 0 => Walk::Tabs,
        KeyCode::Up => to(row - 1, col),
        KeyCode::Down => to(row + 1, col),
        KeyCode::Left if col == 1 => to(row, 0),
        KeyCode::Right if col == 0 => match index_at(row, 1, n, rows) {
            Some(index) => Walk::To(index),
            None => last_row(1, n, rows).map_or(Walk::Stay, |last| to(last, 1)),
        },
        KeyCode::PageUp if row > 0 => to(row.saturating_sub(v), col),
        KeyCode::PageDown => match last_row(col, n, rows) {
            Some(last) if last > row => to((row + v).min(last), col),
            _ => Walk::Stay,
        },
        _ => Walk::Stay,
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The heads on row `y`, TRACK and ARTIST dim over each column (TRACK alone where the cells are
/// narrow), and the room's rule on the row under them: one dim run across the whole content,
/// the scrollbar lane included, as Discover's table rule runs.
pub(super) fn draw_heads(frame: &mut Frame, content: Rect, geom: &GridGeom, y: u16) {
    let parts = parts(geom);
    let head = |key: &str| t!(key).to_uppercase();
    let (track, artist) = (head("gui.discover.col_track"), head("gui.discover.col_artist"));
    for x in [geom.ax, geom.bx] {
        let (track_x, track_w) = parts.track;
        put(frame, x + track_x, y, &clip(&track, usize::from(track_w)), dim());
        if let Some((artist_x, artist_w)) = parts.artist {
            put(frame, x + artist_x, y, &clip(&artist, usize::from(artist_w)), dim());
        }
    }
    put(frame, content.x, y + 1, &"─".repeat(usize::from(content.width)), dim());
}

/// The grid's rows from `first_row`, one screen of them, `cursor` the held lead (the room passes
/// it only while the keyboard holds it). Each cell opens the lead's sheet on a click and on the
/// context click; its slot, registered after it so it wins, previews. The scrollbar runs in the
/// lane right of the list while the rows overflow.
pub(super) fn draw(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    app: &App,
    geom: &GridGeom,
    cursor: Option<usize>,
    first_row: usize,
) {
    let leads = app.peer_recs_leads();
    let n = leads.len();
    let total = total_rows(n, geom.rows);
    let visible = total.saturating_sub(first_row).min(usize::from(geom.rows));
    let parts = parts(geom);
    for line in 0..visible {
        let (row, y) = (first_row + line, geom.list.y + line as u16);
        for (col, x) in [(0, geom.ax), (1, geom.bx)] {
            let Some(index) = index_at(row, col, n, geom.rows) else { continue };
            let cell = Rect { x, y, width: geom.cw, height: 1 };
            let slot = geom.slot.then(|| app.peer_recs_slot(index));
            draw_cell(frame, ui, cell, &parts, (index, &leads[index]), slot, cursor == Some(index));
        }
    }
    let list = geom.list;
    scroll_list(
        frame,
        ui,
        Rect { x: list.x + list.width + 1, y: list.y, width: 1, height: list.height },
        total,
        visible,
        first_row,
        Act::ScrollBy(List::PeerRecs, -1),
        Act::ScrollBy(List::PeerRecs, 1),
        |first| Act::ScrollTo(List::PeerRecs, first),
    );
}

/// One lead's cell (peer-recs contract §1.7's row states).
fn draw_cell(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    cell: Rect,
    parts: &Parts,
    (index, lead): (usize, &TasteLead),
    slot: Option<Slot>,
    held: bool,
) {
    let (x, y) = (cell.x, cell.y);
    let hover = ui.hovers(cell);
    let slab = sel().add_modifier(Modifier::BOLD);
    if held {
        frame.render_widget(ratatui::widgets::Block::default().style(slab), cell);
    }
    let live = matches!(slot, Some(Slot::Playing | Slot::Paused));
    let title_style = if held {
        slab
    } else if live {
        accent().add_modifier(Modifier::BOLD)
    } else if hover {
        bright_bold()
    } else {
        Style::default()
    };
    // The swatch keeps its hue on the slab, whose ground stays under it.
    put(frame, x, y, swatch_glyph(), Style::default().fg(swatch(lead.tile.as_ref().map(|t| t.hue))));
    // A peer's words as far as any real ones run: the cell is measured every frame.
    let words = |text: &Option<String>| {
        let text = text.as_deref().map(|s| peer_text(s).trim());
        text.filter(|s| !s.is_empty()).unwrap_or(dash()).to_string()
    };
    let (track_x, track_w) = parts.track;
    put(frame, x + track_x, y, &clip(&words(&lead.title), usize::from(track_w)), title_style);
    if let Some((artist_x, artist_w)) = parts.artist {
        let style = if held { slab } else { dim() };
        put(frame, x + artist_x, y, &clip(&words(&lead.artist), usize::from(artist_w)), style);
    }
    ui.click(cell, Act::Peer(PeerAct::Row(index)));
    ui.context(cell, Act::Peer(PeerAct::Row(index)));
    if let (Some(slot), Some(slot_x)) = (slot, parts.slot) {
        draw_slot(frame, ui, (x + slot_x, y), index, slot, held.then_some(slab));
    }
}

/// The play slot: its glyph and colour by the clip's state, its press last so it wins over the
/// cell's. A muted slot is dim and does not brighten; its press only notes `No preview`, as `l`
/// does there. `slab`: the held cell's style, which every glyph takes.
fn draw_slot(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    (x, y): (u16, u16),
    index: usize,
    slot: Slot,
    slab: Option<Style>,
) {
    let rect = Rect { x, y, width: SLOT_W, height: 1 };
    let hover = ui.hovers(rect);
    let (play, pause) = (play_glyphs(true).1, play_glyphs(false).1);
    let bold = accent().add_modifier(Modifier::BOLD);
    let (glyph, style) = match slot {
        Slot::Idle => (play, if hover { bright_bold() } else { accent() }),
        Slot::Muted => (play, dim()),
        Slot::Busy => (busy_glyph(), Style::default().fg(th().gold)),
        Slot::Playing => (pause, if hover { bright_bold() } else { bold }),
        Slot::Paused => (play, if hover { bright_bold() } else { bold }),
    };
    put(frame, x, y, glyph, slab.unwrap_or(style));
    // No tooltip: the room's only ones are `Add` and the header's (peer-recs contract §1.2.9).
    ui.click(rect, Act::Peer(PeerAct::Slot(index)));
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(super) mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;
    use serde_json::{Value, json};

    use super::super::super::discover::layout;
    use super::super::super::{Gui, content_rect};
    use super::super::kit::{english, peer_gui, plugins};
    use super::*;
    use crate::api::peer_recs_door::{PeerRecsCmd, PeerRecsEvent};
    use crate::api::types::{CollectionDestinationView, PluginPreview, TasteResponse, fixtures};
    use crate::clock::Instant;
    use crate::kit::boards::{Area, assert_frame};
    use crate::player::{AsidePhase, AsideStatus};
    use crate::tui::app::Effect;
    use crate::tui::worker::{ApiCmd, AudioCmd, Event};

    /// The room's parts at `width × height` with key hints off, as the shell lays them out: the
    /// content, the heads' row and the list under the rule.
    pub(in crate::gui::peer_recs) fn rects(width: u16, height: u16, queue_open: bool) -> (Rect, u16, Rect) {
        let content = content_rect(width, height, queue_open, false);
        let heads_y = layout(content, false, false).table_y;
        let list = Rect {
            x: content.x,
            y: heads_y + 2,
            width: content.width.saturating_sub(2),
            height: content.bottom().saturating_sub(heads_y + 2),
        };
        (content, heads_y, list)
    }

    fn land(gui: &mut Gui, event: PeerRecsEvent) -> Vec<Effect> {
        let generation = gui.app.session_gen();
        gui.app.apply_event(Event::PeerRecs { generation, event: Box::new(event) })
    }

    fn cmds(effects: &[Effect]) -> Vec<PeerRecsCmd> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::PeerRecs { cmd, .. }) => Some(cmd.clone()),
                _ => None,
            })
            .collect()
    }

    fn destination() -> CollectionDestinationView {
        serde_json::from_value(json!({"destination": {"vpath": "music", "base": "", "layout": "{{ARTIST}}"}}))
            .expect("a destination parses")
    }

    /// A visit on the stage's server with `answer` drawn: R1 answered with it, the first poll an
    /// admin's, the gate read with the stage's listing (Deezer's previews).
    pub(in crate::gui::peer_recs) fn room_with(answer: TasteResponse) -> Gui {
        let mut gui = peer_gui();
        let effects = gui.app.peer_recs_open(None);
        for cmd in cmds(&effects) {
            let event = match cmd {
                PeerRecsCmd::Taste { seq, .. } => {
                    PeerRecsEvent::Taste { seq, answer: Ok(Box::new(answer.clone())) }
                }
                PeerRecsCmd::Peers { poll } => {
                    PeerRecsEvent::Peers { poll, answer: Ok(Box::new(fixtures::peers("peers_admin"))) }
                }
                PeerRecsCmd::Gate { seq } => PeerRecsEvent::Gate {
                    seq,
                    plugins: Ok(Box::new(plugins())),
                    destination: Some(Ok(Box::new(destination()))),
                },
                _ => continue,
            };
            land(&mut gui, event);
        }
        gui
    }

    /// The stage's All batch: its 12 leads.
    pub(in crate::gui::peer_recs) fn room() -> Gui {
        let gui = room_with(fixtures::taste("taste_all"));
        assert_eq!(gui.app.peer_recs_leads().len(), 12, "the stage's All batch is drawn");
        gui
    }

    /// `n` leads, `Lead 01` on, each its own key.
    fn many(n: usize) -> TasteResponse {
        let mut v = fixtures::json("taste_all");
        let first = v["network"]["results"][0].clone();
        let rows: Vec<Value> = (1..=n)
            .map(|i| {
                let mut row = first.clone();
                row["title"] = json!(format!("Lead {i:02}"));
                row["exportId"] = json!(format!("lead:{i}"));
                row
            })
            .collect();
        v["network"]["results"] = Value::Array(rows);
        serde_json::from_value(v).expect("the answer parses")
    }

    fn resolve_ticket(effects: &[Effect]) -> u64 {
        let ticket = cmds(effects).into_iter().find_map(|c| match c {
            PeerRecsCmd::Preview { ticket, .. } => Some(ticket),
            _ => None,
        });
        ticket.unwrap_or_else(|| panic!("no resolve in {effects:?}"))
    }

    fn play_ticket(effects: &[Effect]) -> u64 {
        let ticket = effects.iter().find_map(|e| match e {
            Effect::Audio(AudioCmd::AsidePlay { ticket, .. }) => Some(*ticket),
            _ => None,
        });
        ticket.unwrap_or_else(|| panic!("no clip played in {effects:?}"))
    }

    fn clip_answer() -> PluginPreview {
        PluginPreview {
            provider: "deezer".into(),
            url: "http://127.0.0.1:3996/preview/3.mp3".into(),
            seconds: 30,
            title: None,
            artist: None,
            album: None,
            attribution: Some("Preview from Deezer".into()),
            link: None,
            match_score: None,
        }
    }

    fn press(gui: &mut Gui, index: usize) -> Vec<Effect> {
        gui.app.peer_recs_press(index, Instant::now())
    }

    /// The lead at `index` is pressed, its resolve still out.
    pub(in crate::gui::peer_recs) fn busy(gui: &mut Gui, index: usize) {
        resolve_ticket(&press(gui, index));
        assert_eq!(gui.app.peer_recs_slot(index), Slot::Busy);
    }

    /// The lead at `index` found no preview: muted for the visit.
    pub(in crate::gui::peer_recs) fn muted(gui: &mut Gui, index: usize) {
        let ticket = resolve_ticket(&press(gui, index));
        land(gui, PeerRecsEvent::Preview { ticket, answer: Ok(None) });
        assert_eq!(gui.app.peer_recs_slot(index), Slot::Muted);
    }

    /// The lead at `index`'s clip sounds, or, with `paused`, sounded and is paused.
    pub(in crate::gui::peer_recs) fn sounding(gui: &mut Gui, index: usize, paused: bool) {
        let ticket = resolve_ticket(&press(gui, index));
        let played = land(gui, PeerRecsEvent::Preview { ticket, answer: Ok(Some(Box::new(clip_answer()))) });
        let ticket = play_ticket(&played);
        let status = |phase| Some(AsideStatus { ticket, phase, position: 3.0, duration: 30.0 });
        gui.app.apply_event(Event::Aside(status(AsidePhase::Playing)));
        if paused {
            gui.app.peer_recs_clip_toggle();
            gui.app.apply_event(Event::Aside(status(AsidePhase::Paused)));
        }
        assert_eq!(gui.app.peer_recs_slot(index), if paused { Slot::Paused } else { Slot::Playing });
    }

    /// The heads and the grid at `width × height` (key hints off): `cursor` held, from grid row
    /// `first`, the slot drawn when the room offers previews.
    fn drawn(gui: &mut Gui, (width, height): (u16, u16), cursor: Option<usize>, first: usize) -> Buffer {
        let (content, heads_y, list) = rects(width, height, gui.queue_open);
        let geom = geom(list, gui.app.peer_recs_preview_offered());
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        terminal
            .draw(|frame| {
                gui.ui.begin_frame();
                draw_heads(frame, content, &geom, heads_y);
                draw(frame, &mut gui.ui, &gui.app, &geom, cursor, first);
            })
            .expect("the grid draws");
        terminal.backend().buffer().clone()
    }

    /// `width` cells of row `y` from `x`.
    pub(in crate::gui::peer_recs) fn cells(buf: &Buffer, x: u16, y: u16, width: u16) -> String {
        (x..x + width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn at(x: u16, y: u16) -> Position {
        Position { x, y }
    }

    #[test]
    fn thirty_two_leads_fill_column_major_by_screen_blocks() {
        let _locale = english();
        // 100×30: one block, 16 | 16.
        assert_eq!(total_rows(32, 16), 16);
        let cells_30: Vec<(usize, u8)> = [0, 15, 16, 31].into_iter().map(|i| cell_of(i, 32, 16)).collect();
        assert_eq!(cells_30, [(0, 0), (15, 0), (0, 1), (15, 1)]);
        // 100×24: 11 | 11, then 5 | 5.
        assert_eq!(total_rows(32, 11), 16);
        let cells_24: Vec<(usize, u8)> =
            [10, 11, 21, 22, 26, 27, 31].into_iter().map(|i| cell_of(i, 32, 11)).collect();
        assert_eq!(cells_24, [(10, 0), (0, 1), (10, 1), (11, 0), (15, 0), (11, 1), (15, 1)]);

        let mut gui = room_with(many(32));
        let buf = drawn(&mut gui, (100, 30), None, 0);
        for line in 0..16u16 {
            let y = 9 + line;
            assert_eq!(cells(&buf, 19, y, 7), format!("Lead {:02}", line + 1), "column A, row {y}");
            assert_eq!(cells(&buf, 59, y, 7), format!("Lead {:02}", line + 17), "column B, row {y}");
            assert_eq!(cells(&buf, 96, y, 1), " ", "no scrollbar at 30 rows");
            assert_eq!(gui.ui.hit(at(96, y)), None);
        }

        let buf = drawn(&mut gui, (100, 24), None, 0);
        let pair = |buf: &Buffer, y: u16| (cells(buf, 19, y, 7), cells(buf, 59, y, 7));
        assert_eq!(pair(&buf, 8), ("Lead 01".into(), "Lead 12".into()));
        assert_eq!(pair(&buf, 18), ("Lead 11".into(), "Lead 22".into()));
        assert!((8..=18).any(|y| cells(&buf, 96, y, 1) != " "), "the scrollbar runs in the lane");
        assert!(matches!(gui.ui.hit(at(96, 18)), Some(Act::ScrollBy(List::PeerRecs, 1))));
        // Scrolled to its end: the second block's 5 | 5 under the first's last rows.
        let buf = drawn(&mut gui, (100, 24), None, 5);
        assert_eq!(pair(&buf, 8), ("Lead 06".into(), "Lead 17".into()));
        assert_eq!(pair(&buf, 14), ("Lead 23".into(), "Lead 28".into()));
        assert_eq!(pair(&buf, 18), ("Lead 27".into(), "Lead 32".into()));
    }

    #[test]
    fn twelve_read_six_six_and_five_read_three_two() {
        let _locale = english();
        for rows in [16, 11, 10] {
            assert_eq!(total_rows(12, rows), 6);
            assert!((0..6).all(|row| index_at(row, 0, 12, rows) == Some(row)), "{rows} rows");
            assert!((0..6).all(|row| index_at(row, 1, 12, rows) == Some(6 + row)), "{rows} rows");
            assert_eq!(total_rows(5, rows), 3);
            let a: Vec<Option<usize>> = (0..3).map(|row| index_at(row, 0, 5, rows)).collect();
            let b: Vec<Option<usize>> = (0..3).map(|row| index_at(row, 1, 5, rows)).collect();
            assert_eq!((a, b), (vec![Some(0), Some(1), Some(2)], vec![Some(3), Some(4), None]));
        }

        let mut gui = room();
        for (size, top) in [((100, 30), 9), ((100, 24), 8)] {
            let buf = drawn(&mut gui, size, None, 0);
            assert_eq!(cells(&buf, 19, top, 9), "Salt Road", "{size:?}");
            assert_eq!(cells(&buf, 59, top, 13), "Long Exposure", "{size:?}");
            assert_eq!(cells(&buf, 19, top + 5, 12), "Quiet Engine", "{size:?}");
            assert_eq!(cells(&buf, 59, top + 5, 11), "Field Notes", "{size:?}");
            assert_eq!(cells(&buf, 17, top + 6, 78).trim(), "", "{size:?}: six rows");
        }

        let mut gui = room_with(fixtures::taste("taste_kitchen"));
        let buf = drawn(&mut gui, (100, 30), None, 0);
        let column = |x: u16, y: u16| cells(&buf, x, y, 13).trim_end().to_string();
        let a = [column(19, 9), column(19, 10), column(19, 11)];
        assert_eq!(a, ["Signal Fires", "Harbor Lights", "Long Exposure"]);
        assert_eq!([column(59, 9), column(59, 10)], ["Sunday Wire", "Field Notes"]);
        assert_eq!(cells(&buf, 57, 11, 38).trim(), "", "column B's hole");
        assert_eq!(cells(&buf, 17, 12, 78).trim(), "");
    }

    #[test]
    fn cell_and_index_maps_are_inverse() {
        for rows in 0..=17u16 {
            for n in 0..=40usize {
                for index in 0..n {
                    let (row, col) = cell_of(index, n, rows);
                    assert_eq!(index_at(row, col, n, rows), Some(index), "n {n}, rows {rows}, lead {index}");
                }
                let mut found = 0;
                for row in 0..total_rows(n, rows) + 2 {
                    for col in [0, 1] {
                        if let Some(index) = index_at(row, col, n, rows) {
                            assert_eq!(cell_of(index, n, rows), (row, col), "n {n}, rows {rows}");
                            assert!(row < total_rows(n, rows));
                            found += 1;
                        }
                    }
                }
                assert_eq!(found, n, "every lead has one cell: n {n}, rows {rows}");
            }
        }
    }

    #[test]
    fn walking_columns_crosses_blocks_climbs_at_the_top_and_minds_the_holes() {
        use KeyCode::{Down, End, Left, PageDown, PageUp, Right, Up};
        let go = |from, key, n, rows| walk(from, key, n, rows);
        // 32 at 11 rows: down column A crosses into the second block, and back.
        assert_eq!(go(10, Down, 32, 11), Walk::To(22));
        assert_eq!(go(22, Up, 32, 11), Walk::To(10));
        assert_eq!(go(21, Down, 32, 11), Walk::To(27));
        assert_eq!(go(27, Up, 32, 11), Walk::To(21));
        // Either column's first row climbs to the tabs stop.
        assert_eq!(go(0, Up, 32, 11), Walk::Tabs);
        assert_eq!(go(11, Up, 32, 11), Walk::Tabs);
        // Across: the same row, nothing at the edges.
        assert_eq!(go(3, Right, 32, 11), Walk::To(14));
        assert_eq!(go(14, Left, 32, 11), Walk::To(3));
        assert_eq!(go(3, Left, 32, 11), Walk::Stay);
        assert_eq!(go(14, Right, 32, 11), Walk::Stay);
        // A page: one screen of rows, clamped to the column.
        assert_eq!(go(0, PageDown, 32, 11), Walk::To(22));
        assert_eq!(go(22, PageDown, 32, 11), Walk::To(26));
        assert_eq!(go(26, PageDown, 32, 11), Walk::Stay);
        assert_eq!(go(26, PageUp, 32, 11), Walk::To(4));
        assert_eq!(go(4, PageUp, 32, 11), Walk::To(0));
        assert_eq!(go(0, PageUp, 32, 11), Walk::Stay);
        assert_eq!(go(0, End, 32, 11), Walk::Stay);
        // 5 read 3 | 2: ↓ into column B's hole stays; → from beside it finds B's last lead.
        assert_eq!(go(4, Down, 5, 16), Walk::Stay);
        assert_eq!(go(2, Down, 5, 16), Walk::Stay);
        assert_eq!(go(2, Right, 5, 16), Walk::To(4));
        assert_eq!(go(1, Right, 5, 16), Walk::To(4));
        assert_eq!(go(4, Left, 5, 16), Walk::To(1));
        // A last block of one: column B's last lead is the block above's.
        assert_eq!(go(22, Right, 23, 11), Walk::To(21));
        assert_eq!(go(21, Down, 23, 11), Walk::Stay);
        assert_eq!(go(10, Down, 23, 11), Walk::To(22));
        // One lead: nowhere to go but up.
        assert_eq!(go(0, Right, 1, 16), Walk::Stay);
        assert_eq!(go(0, Down, 1, 16), Walk::Stay);
        assert_eq!(go(0, Up, 1, 16), Walk::Tabs);
        // A cursor past the list walks nowhere.
        assert_eq!(go(12, Down, 12, 16), Walk::Stay);
    }

    #[test]
    fn reg_grid_frames_match() {
        let _locale = english();
        let mut gui = room();
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_frame(&buf, "REG-GRID", "78", Area::at(9, 1));
        // M1's heads, rule and grid, the whole of it at rest.
        assert_frame(&buf, "M1", "", Area::rect(17, 99, 7, 14));

        let mut gui = room();
        sounding(&mut gui, 2, false);
        assert_frame(&drawn(&mut gui, (100, 30), None, 0), "REG-GRID", "78", Area::at(11, 2));

        let mut gui = room();
        muted(&mut gui, 7);
        assert_frame(&drawn(&mut gui, (100, 30), None, 0), "REG-GRID", "78", Area::at(10, 3));

        let mut gui = room();
        busy(&mut gui, 5);
        assert_frame(&drawn(&mut gui, (100, 30), None, 0), "REG-GRID", "78", Area::at(14, 4));

        let mut gui = room();
        sounding(&mut gui, 2, true);
        assert_frame(&drawn(&mut gui, (100, 30), None, 0), "REG-GRID", "78", Area::at(11, 5));

        let mut gui = room();
        assert_frame(&drawn(&mut gui, (100, 30), Some(6), 0), "REG-GRID", "78", Area::at(9, 6));

        let mut gui = room();
        gui.app.audio_available = false;
        assert_frame(&drawn(&mut gui, (100, 30), None, 0), "REG-GRID", "78", Area::at(9, 7));

        // The queue open: 21-cell columns, TRACK and the slot.
        let mut gui = room();
        gui.queue_open = true;
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_frame(&buf, "REG-GRID", "47", Area::at(9, 1));
        assert_frame(&buf, "REG-GRID", "47", Area::at(10, 2));
        assert_frame(&buf, "V3", "", Area::rect(17, 63, 7, 14));
    }

    #[test]
    fn the_slab_spans_the_cell_and_the_swatch_keeps_its_hue() {
        let _locale = english();
        let t = th();
        let mut gui = room();
        let buf = drawn(&mut gui, (100, 30), Some(6), 0);
        assert!((57..=94).all(|x| buf[(x, 9)].bg == t.accent), "the slab spans Long Exposure's 38 cells");
        assert!(buf[(56, 9)].bg != t.accent && buf[(95, 9)].bg != t.accent, "and no further");
        assert!((17..=54).all(|x| buf[(x, 9)].bg != t.accent), "Salt Road beside it is not held");
        let swatch_cell = &buf[(57, 9)];
        let hue = swatch(Some(352));
        assert_eq!((swatch_cell.symbol(), swatch_cell.fg), ("▌", hue), "the swatch keeps its hue");
        let title = &buf[(59, 9)];
        assert_eq!(title.fg, t.on_accent);
        assert!(title.modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(76, 9)].fg, t.on_accent, "the artist is on the slab too");
        assert_eq!((buf[(93, 9)].symbol(), buf[(93, 9)].fg), ("▸", t.on_accent), "and the slot");
        // At rest the swatch is the hue alone, on the ground.
        assert_eq!(buf[(17, 9)].fg, swatch(Some(203)));
        assert_ne!(buf[(17, 9)].bg, t.accent);

        // A held muted slot is on the slab, its dim hidden (L4).
        let mut gui = room();
        muted(&mut gui, 7);
        let buf = drawn(&mut gui, (100, 30), Some(7), 0);
        let slot = &buf[(93, 10)];
        assert_eq!((slot.symbol(), slot.fg, slot.bg), ("▸", t.on_accent, t.accent));
        // A lead without a tile draws its swatch dim.
        let mut answer = fixtures::json("taste_all");
        answer["network"]["results"][0]["tile"] = Value::Null;
        let mut gui = room_with(serde_json::from_value(answer).expect("the answer parses"));
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_eq!((buf[(17, 9)].symbol(), buf[(17, 9)].fg), ("▌", swatch(None)));
    }

    #[test]
    fn no_slot_runs_artist_to_the_cell_end() {
        let _locale = english();
        let mut gui = room();
        gui.app.audio_available = false;
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_frame(&buf, "V2", "", Area::rect(17, 99, 9, 14));
        assert_eq!(gui.ui.hit(at(53, 9)), Some(Act::Peer(PeerAct::Row(0))), "no slot to press");

        // A long artist takes 19 cells without the slot, 16 with it, clipped with the kit's mark.
        let mut answer = fixtures::json("taste_all");
        answer["network"]["results"][0]["artist"] = json!("The Extraordinarily Long Band Name");
        let answer: TasteResponse = serde_json::from_value(answer).expect("the answer parses");
        let mut gui = room_with(answer.clone());
        gui.app.audio_available = false;
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_eq!(cells(&buf, 36, 9, 19), "The Extraordinaril…");
        assert_eq!(cells(&buf, 55, 9, 2), "  ", "the gap before column B");
        let mut gui = room_with(answer);
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_eq!(cells(&buf, 36, 9, 17), "The Extraordina… ");
        assert_eq!(cells(&buf, 53, 9, 1), "▸");
    }

    #[test]
    fn the_rule_spans_the_content_and_the_lane() {
        let _locale = english();
        let mut gui = room();
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_eq!(cells(&buf, 17, 8, 80), "─".repeat(80), "x 17–96, the lane included");
        assert_eq!(cells(&buf, 97, 8, 3), "   ");
        assert!((17..=96).all(|x| buf[(x, 8)].fg == th().dim));
        assert_eq!((cells(&buf, 19, 7, 5), cells(&buf, 36, 7, 6)), ("TRACK".into(), "ARTIST".into()));
        assert_eq!((cells(&buf, 59, 7, 5), cells(&buf, 76, 7, 6)), ("TRACK".into(), "ARTIST".into()));
        assert_frame(&buf, "W1", "", Area::rect(17, 99, 7, 8));

        // At the floor the heads and the rule climb with the room.
        let buf = drawn(&mut gui, (100, 24), None, 0);
        assert_eq!(cells(&buf, 17, 7, 80), "─".repeat(80));
        assert_eq!(cells(&buf, 19, 6, 5), "TRACK");

        // The queue open: 47 cells, and ARTIST drops.
        gui.queue_open = true;
        let buf = drawn(&mut gui, (100, 30), None, 0);
        assert_eq!(cells(&buf, 17, 8, 47), "─".repeat(47));
        assert_eq!(cells(&buf, 64, 8, 1), " ");
        assert_eq!((cells(&buf, 19, 7, 5), cells(&buf, 42, 7, 5)), ("TRACK".into(), "TRACK".into()));
        assert!(!cells(&buf, 17, 7, 47).contains("ARTIST"));
    }

    #[test]
    fn a_slot_click_previews_and_a_cell_click_opens() {
        let _locale = english();
        let mut gui = room();
        drawn(&mut gui, (100, 30), None, 0);
        let hit = |gui: &Gui, x, y| gui.ui.hit(at(x, y));
        let peer = |act| Some(Act::Peer(act));
        // The slot's two cells preview; the rest of the cell opens the lead's sheet.
        assert_eq!(hit(&gui, 53, 9), peer(PeerAct::Slot(0)));
        assert_eq!(hit(&gui, 54, 9), peer(PeerAct::Slot(0)));
        assert_eq!(hit(&gui, 17, 9), peer(PeerAct::Row(0)));
        assert_eq!(hit(&gui, 52, 9), peer(PeerAct::Row(0)));
        assert_eq!(hit(&gui, 55, 9), None, "the gap between the columns");
        assert_eq!(hit(&gui, 57, 9), peer(PeerAct::Row(6)));
        assert_eq!(hit(&gui, 93, 14), peer(PeerAct::Slot(11)));
        assert_eq!(hit(&gui, 60, 14), peer(PeerAct::Row(11)));
        assert_eq!(hit(&gui, 20, 15), None, "under the last row");
        // A right click opens the sheet anywhere on the cell, the slot included.
        assert_eq!(gui.ui.hit_context(at(20, 10)), peer(PeerAct::Row(1)));
        assert_eq!(gui.ui.hit_context(at(53, 10)), peer(PeerAct::Row(1)));

        // The room's act: a slot press asks the preview plug-in about its lead; a muted slot
        // presses into the note and asks nothing.
        gui.pending.clear();
        gui.act(Act::Peer(PeerAct::Slot(0)));
        assert_eq!(gui.app.peer_recs_slot(0), Slot::Busy);
        let resolves = cmds(&gui.pending).into_iter().filter(|c| matches!(c, PeerRecsCmd::Preview { .. }));
        assert_eq!(resolves.count(), 1, "{:?}", gui.pending);
        let mut gui = room();
        muted(&mut gui, 7);
        drawn(&mut gui, (100, 30), None, 0);
        assert_eq!(hit(&gui, 93, 10), peer(PeerAct::Slot(7)));
        gui.pending.clear();
        gui.act(Act::Peer(PeerAct::Slot(7)));
        assert!(cmds(&gui.pending).is_empty(), "{:?}", gui.pending);
        assert_eq!(gui.app.peer_recs_slot(7), Slot::Muted);
    }

    #[test]
    fn no_slot_or_cell_registers_a_tooltip() {
        // The room's only tooltips are `Add` on a `+` tab and the header's (peer-recs contract
        // §1.2.9): an idle, a busy, a sounding and a muted slot register none.
        let _locale = english();
        let mut gui = room();
        muted(&mut gui, 7);
        busy(&mut gui, 3);
        drawn(&mut gui, (100, 30), Some(1), 0);
        assert!(gui.ui.tips.is_empty(), "{:?}", gui.ui.tips);
        let mut gui = room();
        sounding(&mut gui, 0, false);
        drawn(&mut gui, (100, 30), Some(0), 0);
        assert!(gui.ui.tips.is_empty(), "{:?}", gui.ui.tips);
    }
}
