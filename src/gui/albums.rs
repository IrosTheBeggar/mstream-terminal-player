//! The Albums wall: every album's cover, name and year, paged with
//! arrows — the webapp's album grid, worn terminal. Clicking an album
//! drills into its tracks through the App's own Library drill, so
//! playing, queueing and Back are the shared state machine's.
//!
//! Covers draw through `cover::Slot`, one per grid cell, paced by the
//! frame's encode budget — the feasibility story and the pacing live
//! there, measured by the tests at the bottom of this file.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::{ListView, dim, scroll_list};
use crate::kit::theme::{legacy_conhost, th};
use crate::tui::app::Action;
use crate::api::types::Album;
use crate::tui::worker::LibraryNode;

use super::cover::{Pace, Slot};
use super::{Act, Gui, List, RowVerb, accent, bright_bold, put, sel};

/// The cover's cells: 12x6 is square at the common 10x20 font.
const COVER_W: u16 = 12;
const COVER_H: u16 = 6;
/// Cover, name, year.
const CELL_H: u16 = COVER_H + 2;
/// Two columns of breathing room, one row.
const STRIDE_X: u16 = COVER_W + 2;
const STRIDE_Y: u16 = CELL_H + 1;

// ── State ───────────────────────────────────────────────────────────────────

/// A wall's own state. The Albums room keeps one and the library room
/// keeps another for its artist wall (library-rooms contract, clause 7), so
/// leaving one for the other finds each standing where it was.
#[derive(Debug, Default)]
pub(crate) struct WallState {
    pub page: usize,
    /// The cell cursor within the page — the keyboard's hand on the grid.
    pub cursor: usize,
    /// The track view's viewport (the kit's table contract, same as Files).
    pub tracks: ListView,
}

pub(crate) struct AlbumsUi {
    slots: Vec<Slot>,
    pub wall: WallState,
}

impl AlbumsUi {
    pub(crate) fn new() -> AlbumsUi {
        AlbumsUi { slots: Vec::new(), wall: WallState::default() }
    }

    /// A resize changes the cell-to-pixel mapping every slot encoded
    /// against, and kitty forgets its transmitted images with the old
    /// terminal — the card cover's rule, per slot.
    pub(crate) fn on_resize(&mut self) {
        for slot in &mut self.slots {
            slot.on_resize();
        }
    }
}

/// The page's shape in `content`: columns, rows, and where cell `i` sits.
struct GridShape {
    cols: usize,
    rows: usize,
    origin: (u16, u16),
}

impl GridShape {
    fn for_content(content: Rect) -> GridShape {
        let cols = ((content.width + 2) / STRIDE_X).max(1) as usize;
        // Rows keep the pre-bar sum: the last row spends its trailing
        // blank on the controls line, and the per-cell bottom guard
        // clips anything that genuinely would not fit.
        let rows = (content.height.saturating_sub(2) / STRIDE_Y).max(1) as usize;
        GridShape { cols, rows, origin: (content.x, content.y + 3) }
    }

    fn capacity(&self) -> usize {
        self.cols * self.rows
    }

    fn cell(&self, i: usize) -> Rect {
        let (col, row) = (i % self.cols, i / self.cols);
        Rect {
            x: self.origin.0 + col as u16 * STRIDE_X,
            y: self.origin.1 + row as u16 * STRIDE_Y,
            width: COVER_W,
            height: CELL_H,
        }
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// Whether the wall on screen is the Albums room's own (the library room's
/// artist wall otherwise).
fn on_root_wall(gui: &Gui) -> bool {
    gui.active == super::ALBUMS_NAV
}

pub(super) fn wall(gui: &mut Gui) -> &mut WallState {
    if gui.active == super::ALBUMS_NAV { &mut gui.albums.wall } else { &mut gui.library.wall }
}

fn wall_ref(gui: &Gui) -> &WallState {
    if gui.active == super::ALBUMS_NAV { &gui.albums.wall } else { &gui.library.wall }
}

/// The albums the wall on screen shows: the Albums room's list, or the
/// drilled artist's, which the App keeps beside it.
pub(crate) fn wall_albums(gui: &Gui) -> Option<&Vec<Album>> {
    if on_root_wall(gui) {
        return gui.app.albums.as_ref();
    }
    match gui.app.library_stack.here() {
        LibraryNode::Artist(artist) => {
            gui.app.artist_albums.as_ref().filter(|(name, _)| name == artist).map(|(_, albums)| albums)
        }
        _ => None,
    }
}

/// Whether the Library drill is standing inside one album — the track
/// view. Anything else shows the wall.
fn drilled_album(gui: &Gui) -> Option<(String, Option<String>)> {
    match gui.app.library_stack.here() {
        LibraryNode::Album { name, artist } => Some((name.clone(), artist.clone())),
        _ => None,
    }
}

fn page_count(albums: usize, capacity: usize) -> usize {
    albums.div_ceil(capacity).max(1)
}

/// The wall's view of the album list: every index — or, while the pane's
/// filter stands, the indices whose label matches it, the same match the
/// pane's own rows use (the bar contract's clause 13: what you see is
/// what the wall shows).
fn visible_albums(gui: &Gui) -> Option<Vec<usize>> {
    let albums = wall_albums(gui)?;
    let needle = gui.app.library.filter.trim().to_lowercase();
    Some(if needle.is_empty() {
        (0..albums.len()).collect()
    } else {
        albums
            .iter()
            .enumerate()
            .filter(|(_, album)| {
                crate::tui::app::entries::album_label(album)
                    .to_lowercase()
                    .contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    })
}

/// The grid geometry the last frame drew with — recomputed from the same
/// inputs, so key handling agrees with the pointer about where cells are.
fn shape(gui: &Gui) -> GridShape {
    GridShape::for_content(super::content_rect(gui.last_width, gui.last_height, gui.queue_open, gui.footer()))
}

fn turn_page(gui: &mut Gui, delta: i32) {
    let Some(visible) = visible_albums(gui) else { return };
    let pages = page_count(visible.len(), shape(gui).capacity());
    let w = wall(gui);
    let page = w.page as i32 + delta;
    w.page = page.clamp(0, pages as i32 - 1) as usize;
    w.cursor = 0;
}

/// The strip's jump (library-rooms contract, clause 10): the page that
/// holds the visible album at `at`, with the cursor on it.
fn jump_wall(gui: &mut Gui, at: usize) {
    let capacity = shape(gui).capacity().max(1);
    let total = visible_albums(gui).map_or(0, |v| v.len());
    if at >= total {
        return;
    }
    let w = wall(gui);
    w.page = at / capacity;
    w.cursor = at % capacity;
}

fn open_album(gui: &mut Gui, index_on_page: usize) {
    let capacity = shape(gui).capacity();
    let at = wall_ref(gui).page * capacity + index_on_page;
    let Some(album) = visible_albums(gui)
        .as_deref()
        .and_then(|v| v.get(at))
        .and_then(|&i| wall_albums(gui).and_then(|a| a.get(i)))
        .cloned()
    else {
        return;
    };
    {
        let w = wall(gui);
        w.cursor = index_on_page;
        w.tracks.scroll = 0;
        w.tracks.reveal = false;
    }
    let node = LibraryNode::Album {
        name: album.name.clone().unwrap_or_default(),
        artist: album.artist.clone(),
    };
    // The App's door drills through the pane's own row when the pane lists
    // this album, so Back restores the wall's list from the trail without
    // asking again (library-rooms contract, clause 6).
    let effects = gui.app.open_library_node(node, false);
    gui.pend(effects);
}

/// The albums side of [`Gui::act`]. Returns true when the act was ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act {
        Act::AlbPage(delta) => turn_page(gui, *delta),
        Act::AlbCell(i) => open_album(gui, *i),
        Act::AlbJump(at) => jump_wall(gui, *at),
        _ => return false,
    }
    true
}

/// The albums side of the key handler: Some(quit) when consumed, None to
/// fall through to the global keys. Only called when the Albums room is
/// the active section.
pub(crate) fn handle_key(gui: &mut Gui, key: ratatui::crossterm::event::KeyEvent) -> Option<bool> {
    use ratatui::crossterm::event::KeyCode;

    if drilled_album(gui).is_some() {
        match key.code {
            KeyCode::Down => {
                wall(gui).tracks.reveal = true;
                gui.forward(Action::Down);
            }
            KeyCode::Up => {
                wall(gui).tracks.reveal = true;
                gui.forward(Action::Up);
            }
            KeyCode::PageDown => {
                wall(gui).tracks.reveal = true;
                gui.forward(Action::PageDown);
            }
            KeyCode::PageUp => {
                wall(gui).tracks.reveal = true;
                gui.forward(Action::PageUp);
            }
            KeyCode::Enter => gui.forward_capturing(Action::Activate),
            KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Esc => {
                gui.forward(Action::Back);
            }
            KeyCode::Char('a') => gui.forward(Action::AddToQueue),
            _ => return None,
        }
        return Some(false);
    }

    let (capacity, cols) = {
        let shape = shape(gui);
        (shape.capacity(), shape.cols)
    };
    let on_page = page_len(gui, capacity);
    match key.code {
        KeyCode::Left | KeyCode::PageUp => turn_page(gui, -1),
        KeyCode::Right | KeyCode::PageDown => turn_page(gui, 1),
        KeyCode::Down => {
            if on_page > 0 {
                let w = wall(gui);
                w.cursor = (w.cursor + cols).min(on_page - 1);
            }
        }
        KeyCode::Up => {
            let w = wall(gui);
            w.cursor = w.cursor.saturating_sub(cols);
        }
        KeyCode::Enter => {
            let cursor = wall_ref(gui).cursor;
            open_album(gui, cursor);
        }
        // The artist wall climbs back to the artists; the root wall has
        // nowhere to go.
        KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Esc if !on_root_wall(gui) => {
            gui.forward(Action::Back);
        }
        _ => return None,
    }
    Some(false)
}

/// The wheel turns the wall a page at a time, and scrolls the track list.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if drilled_album(gui).is_some() {
        gui.act(Act::ScrollBy(List::AlbumTracks, delta));
    } else {
        turn_page(gui, delta);
    }
}

/// How many albums the current page actually shows.
fn page_len(gui: &Gui, capacity: usize) -> usize {
    let total = visible_albums(gui).map_or(0, |v| v.len());
    total.saturating_sub(wall_ref(gui).page * capacity).min(capacity)
}

// ── Drawing ─────────────────────────────────────────────────────────────────

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !gui.app.connected {
        super::servers::draw_disconnected(frame, gui, content, 0);
        return;
    }
    if let Some((name, artist)) = drilled_album(gui) {
        draw_tracks(frame, gui, content, &name, artist.as_deref());
    } else {
        draw_wall(frame, gui, content);
    }
}

/// The wall's heading: the room's name on the Albums wall; on an artist's
/// wall the way back to the artists, then the artist (library-rooms
/// contract, clause 7). Returns where the count may start.
fn draw_wall_heading(frame: &mut Frame, gui: &mut Gui, content: Rect) -> u16 {
    let artist = match gui.app.library_stack.here() {
        LibraryNode::Artist(artist) if !on_root_wall(gui) => Some(artist.clone()),
        _ => None,
    };
    let Some(artist) = artist else {
        let heading = t!("gui.nav.albums").to_string();
        put(frame, content.x, content.y, &heading, Style::default().add_modifier(Modifier::BOLD));
        return content.x + heading.chars().count() as u16 + 2;
    };
    let (back_glyph, forward) = if legacy_conhost() { ("<", ">") } else { ("◂", "▸") };
    let back_label = format!("{back_glyph} {}", t!("gui.nav.artists"));
    let back = Rect { x: content.x, y: content.y, width: back_label.chars().count() as u16, height: 1 };
    let hover = gui.ui.hovers(back);
    put(frame, content.x, content.y, &back_label, if hover { bright_bold() } else { dim() });
    gui.ui.click(back, Act::LibBack);
    let title = format!("{forward} {artist}");
    let x = back.right() + 1;
    let shown = super::bar::clip(&title, content.width.saturating_sub(back.width + 18) as usize);
    put(frame, x, content.y, &shown, Style::default().add_modifier(Modifier::BOLD));
    x + shown.chars().count() as u16 + 2
}

pub(crate) fn draw_wall(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let count_x = draw_wall_heading(frame, gui, content);

    let Some(visible) = visible_albums(gui) else {
        put(frame, content.x, content.y + 3, &t!("gui.alb.loading"), accent());
        return;
    };
    let all = wall_albums(gui).map_or(0, Vec::len);
    super::draw_bar_controls(frame, gui, content, content.y + 1);
    if all == 0 {
        put(frame, content.x, content.y + 3, &t!("gui.alb.empty"), dim());
        return;
    }
    let total = visible.len();
    if total == 0 {
        // The filter matched nothing; the way back is one key (clause 30).
        put(frame, content.x, content.y + 3, &t!("gui.files.empty"), dim());
        put(
            frame,
            content.right().saturating_sub(
                super::bar_count(gui, String::new()).chars().count() as u16,
            ),
            content.y,
            &super::bar_count(gui, String::new()),
            dim(),
        );
        return;
    }

    let shape = GridShape::for_content(content);
    let capacity = shape.capacity();
    let pages = page_count(total, capacity);
    {
        let w = wall(gui);
        w.page = w.page.min(pages - 1);
    }
    let page = wall_ref(gui).page;
    let start = page * capacity;
    let shown = total.saturating_sub(start).min(capacity);
    {
        let w = wall(gui);
        w.cursor = w.cursor.min(shown.saturating_sub(1));
    }

    put(
        frame,
        count_x,
        content.y,
        &super::bar_count(gui, t!("gui.alb.count", count = all).to_string()),
        dim(),
    );

    // The strip (library-rooms contract, clause 10): the root wall is
    // alphabetical; an artist's albums are in the server's order.
    if on_root_wall(gui) && total >= crate::kit::STRIP_MIN_ROWS {
        let (present, first_of) = match wall_albums(gui) {
            Some(albums) => crate::kit::letter_index(
                visible.iter().map(|&at| albums.get(at).and_then(|a| a.name.as_deref()).unwrap_or("")),
            ),
            None => ([false; crate::kit::STRIP_BUCKETS], [0; crate::kit::STRIP_BUCKETS]),
        };
        let strip = Rect { x: content.x, y: content.y + 2, width: content.width, height: 1 };
        crate::kit::letter_strip(frame, &mut gui.ui, strip, &present, move |bucket| Act::AlbJump(first_of[bucket]));
    }

    // The pager: two arrows around "page/pages", each end dim when there
    // is nothing further that way.
    let (back_glyph, fwd_glyph) = if legacy_conhost() { ("<", ">") } else { ("◂", "▸") };
    let label = format!("{}/{}", page + 1, pages);
    let fwd_x = content.right().saturating_sub(2);
    let label_x = fwd_x.saturating_sub(label.chars().count() as u16 + 1);
    let back_x = label_x.saturating_sub(2);
    for (x, glyph, delta, live) in [
        (back_x, back_glyph, -1i32, page > 0),
        (fwd_x, fwd_glyph, 1, page + 1 < pages),
    ] {
        let rect = Rect { x, y: content.y, width: 1, height: 1 };
        let hover = live && gui.ui.hovers(rect);
        let style = match (live, hover) {
            (false, _) => dim(),
            (true, true) => bright_bold(),
            (true, false) => Style::default().fg(th().accent),
        };
        put(frame, x, content.y, glyph, style);
        if live {
            gui.ui.click(rect, Act::AlbPage(delta));
        }
    }
    put(frame, label_x, content.y, &label, dim());

    // The covers this page still owes the cache: claimed through the
    // App's own fetch. Only the missing ones allocate anything — after
    // the first frame of a page this whole scan is hashmap lookups.
    let missing: Vec<String> = {
        let Some(albums) = wall_albums(gui) else { return };
        visible
            .iter()
            .skip(start)
            .take(shown)
            .filter_map(|&at| albums.get(at))
            .filter_map(|album| album.album_art_file.as_deref())
            .filter(|file| !gui.app.art.contains_key(*file))
            .map(str::to_string)
            .collect()
    };
    let mut fetches = Vec::new();
    for file in missing {
        if let Some(effect) = gui.app.fetch_art_file(&file) {
            fetches.push(effect);
        }
    }
    gui.pend(fetches);

    // The slots, one per cell, forked from the probed answer so each has
    // its own caches.
    while gui.albums.slots.len() < capacity {
        let fork = gui.app.graphics.fork();
        gui.albums.slots.push(Slot::new(fork));
    }
    // A cover per cell whichever wall is up: the slots are the room's, keyed
    // by what they last drew.

    // Split borrows: the page's rows are READ from the App while each
    // slot's caches are written — disjoint fields of the one Gui. This is
    // what lets a frame draw the wall without cloning a page of albums,
    // or worse, their decoded covers (fifty kilobytes of pixels apiece,
    // ten times a second, for nothing).
    let pace = Pace::frame();
    {
        let root = on_root_wall(gui);
        let Gui { app, albums: albums_ui, library, ui, .. } = &mut *gui;
        let AlbumsUi { slots, wall: root_wall } = albums_ui;
        let wall: &WallState = if root { root_wall } else { &library.wall };
        let albums: Option<&Vec<Album>> = if root {
            app.albums.as_ref()
        } else {
            match app.library_stack.here() {
                LibraryNode::Artist(artist) => {
                    app.artist_albums.as_ref().filter(|(name, _)| name == artist).map(|(_, a)| a)
                }
                _ => None,
            }
        };
        let Some(albums) = albums else { return };
        for (i, album) in
            visible.iter().skip(start).take(shown).filter_map(|&at| albums.get(at)).enumerate()
        {
            let cell = shape.cell(i);
            if cell.bottom() > content.bottom() {
                break;
            }
            let cover = Rect { x: cell.x, y: cell.y, width: COVER_W, height: COVER_H };
            let art = album
                .album_art_file
                .as_deref()
                .and_then(|file| app.art.get(file))
                .and_then(|art| art.as_ref());
            match art {
                // Text where an overlay stood last frame (the header
                // dropdown reaches the wall's first row), pixels elsewhere.
                Some(art) if ui.covered_last_frame(cover) => {
                    slots[i].draw_mosaic(frame, cover, art)
                }
                Some(art) => slots[i].draw_paced(frame, cover, art, &pace),
                // No cover (yet): the empty slot frame, the card's own idiom.
                None => super::bar::cover_slot(frame, cover.x, cover.y, cover.width, cover.height),
            }

            // The server's name-less bucket is the artist's loose tracks
            // (library-rooms contract, clause 8).
            let name = album.name.clone().unwrap_or_else(|| t!("gui.lib.singles").to_string());
            let name = name.as_str();
            let name_y = cell.y + COVER_H;
            let selected = wall.cursor == i;
            let cell_hover = ui.hovers(cell);
            let name_rect = Rect { x: cell.x, y: name_y, width: COVER_W, height: 1 };
            if selected {
                frame.render_widget(ratatui::widgets::Block::default().style(sel()), name_rect);
            }
            let name_style = match (selected, cell_hover) {
                (true, _) => sel().add_modifier(Modifier::BOLD),
                (false, true) => bright_bold(),
                (false, false) => Style::default(),
            };
            put(frame, cell.x, name_y, &super::bar::clip(name, COVER_W as usize), name_style);

            let below = match (album.year, album.artist.as_deref()) {
                (Some(year), Some(artist)) => format!("{year} · {artist}"),
                (Some(year), None) => year.to_string(),
                (None, Some(artist)) => artist.to_string(),
                (None, None) => String::new(),
            };
            if !below.is_empty() {
                let bstyle = if selected { sel() } else { dim() };
                put(
                    frame,
                    cell.x,
                    name_y + 1,
                    &super::bar::clip(&below, COVER_W as usize),
                    bstyle,
                );
            }
            ui.click(cell, Act::AlbCell(i));
        }
    }
    // Slots the budget turned away want the very next frame, not the next
    // poll tick — the event loop shortens its wait while this stands.
    if pace.starved() {
        gui.hot = true;
    }
}

pub(crate) fn draw_tracks(frame: &mut Frame, gui: &mut Gui, content: Rect, name: &str, artist: Option<&str>) {
    // The way back leads the header — named for where it goes: the artist
    // when the album was reached through one (library-rooms contract,
    // clause 9), the wall otherwise — then the album's own name.
    let back_glyph = super::back_glyph();
    let back_name = match gui.app.library_stack.parent() {
        Some(LibraryNode::Artist(artist)) => artist.clone(),
        _ => t!("gui.nav.albums").to_string(),
    };
    let back_label = format!("{back_glyph} {back_name}");
    let back = Rect {
        x: content.x,
        y: content.y,
        width: back_label.chars().count() as u16,
        height: 1,
    };
    let hover = gui.ui.hovers(back);
    put(frame, content.x, content.y, &back_label, if hover { bright_bold() } else { dim() });
    gui.ui.click(back, Act::PaneRow(List::AlbumTracks, 0, RowVerb::Open)); // row 0 is the Parent row: Back

    let title_x = back.right() + 2;
    let name = if name.is_empty() { t!("gui.lib.singles").to_string() } else { name.to_string() };
    let title = match artist {
        Some(artist) => format!("{name} — {artist}"),
        None => name,
    };
    let count =
        super::bar_count(gui, t!("gui.files.items", count = gui.app.library.counts().1).to_string());
    let count_x = content.right().saturating_sub(count.chars().count() as u16);
    put(frame, count_x, content.y, &count, dim());
    put(
        frame,
        title_x,
        content.y,
        &super::bar::clip(&title, count_x.saturating_sub(title_x + 2) as usize),
        Style::default().add_modifier(Modifier::BOLD),
    );
    super::draw_bar_controls(frame, gui, content, content.y + 1);

    let len = gui.app.library.entries.len();
    if len <= 1 {
        put(frame, content.x, content.y + 3, &t!("busy.listing"), accent());
        return;
    }

    let list = Rect {
        x: content.x,
        y: content.y + 3,
        width: content.width - 2,
        height: content.height - 3,
    };
    let selected = gui.app.library.state.selected();
    let (first, visible) = wall(gui).tracks.window(len, selected, list.height as usize);
    let entries = &gui.app.library.entries;

    let rows: Vec<(usize, &crate::tui::app::Entry)> =
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
        List::AlbumTracks,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::ScrollBy(List::AlbumTracks, -1),
        Act::ScrollBy(List::AlbumTracks, 1),
        |first| Act::ScrollTo(List::AlbumTracks, first),
    );
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Tab};
    use crate::api::types::Album;
    use crate::config::Config;
    use crate::tui::app::{App, Effect};
    use crate::tui::worker::{ApiCmd, Event, LibraryData};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn album(name: &str, artist: &str, year: i32, art: Option<&str>) -> Album {
        Album {
            name: Some(name.to_string()),
            artist: Some(artist.to_string()),
            year: Some(year),
            album_art_file: art.map(str::to_string),
        }
    }

    /// A connected Gui on the Albums section with `n` albums standing —
    /// arrived at the way a user arrives: the nav click opens the drill,
    /// and the wall is the server's reply to it.
    fn wall_gui(n: usize) -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.app.connected = true;
        gui.queue_open = false;
        gui.act(Act::Nav(super::super::ALBUMS_NAV));
        let albums = (0..n)
            .map(|i| {
                album(
                    &format!("Album {i:02}"),
                    &format!("Artist {i:02}"),
                    2000 + i as i32,
                    Some(&format!("aa{i:02}.jpeg")),
                )
            })
            .collect();
        let effects = gui.app.apply_event(Event::Library {
            node: LibraryNode::Albums,
            dest: Tab::Library,
            data: LibraryData::Albums(albums),
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

    #[test]
    fn the_bar_filters_the_wall_and_gates_its_verbs() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut gui = wall_gui(25);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("/ filter"), "the bar reaches the wall:\n{text}");
        assert!(!text.contains("▸ play"), "albums are containers — no play verb:\n{text}");

        // f opens the App's own prompt; typing narrows the wall live.
        super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(gui.app.filtering, "f reaches the bar in this room too");
        for c in "album 07".chars() {
            super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Album 07"), "the match stands:\n{text}");
        assert!(!text.contains("Album 01"), "the rest of the wall folded:\n{text}");
        assert!(text.contains("1 of 25"), "the honest count:\n{text}");

        // Enter keeps it; the one visible cell opens the matching album,
        // not whatever lived at that grid index unfiltered.
        super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        gui.pending.clear();
        gui.act(Act::AlbCell(0));
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::Library { node: LibraryNode::Album { name, .. }, .. })
                    if name == "Album 07"
            )),
            "the filtered cell maps home: {:?}",
            gui.pending
        );
    }

    #[test]
    fn typing_a_filter_never_queues_a_row() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // The letter-steal guard: the filter gate outranks the room's own
        // keys, so an 'a' typed into the prompt must not queue-add.
        let mut gui = wall_gui(3);
        super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(gui.app.library.filter, "a", "the letter reached the prompt");
        assert!(gui.app.queue.items.is_empty(), "and queued nothing");
    }

    #[test]
    fn the_wall_pages_with_arrows_and_clamps_at_the_ends() {
        let mut gui = wall_gui(25);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Album 00"), "page one leads with the first album");
        assert!(all.contains("2000 · Arti"), "the year and artist ride under the name");
        assert!(all.contains("1/3"), "25 albums at 10 a page is three pages");
        assert!(all.contains(&t!("gui.alb.count", count = 25).to_string()));

        turn_page(&mut gui, 1);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("2/3"));
        assert!(all.contains("Album 10"), "page two picks up where one left off");
        assert!(!all.contains("Album 00"));

        turn_page(&mut gui, 1);
        turn_page(&mut gui, 1);
        assert_eq!(gui.albums.wall.page, 2, "the last page is the floor of forward");
        turn_page(&mut gui, -1);
        turn_page(&mut gui, -1);
        turn_page(&mut gui, -1);
        assert_eq!(gui.albums.wall.page, 0, "and the first of back");
    }

    #[test]
    fn a_cell_opens_its_album_through_the_library_drill() {
        let mut gui = wall_gui(25);
        draw(&mut gui);
        turn_page(&mut gui, 1);
        open_album(&mut gui, 1); // second cell of page two: Album 11
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::Library {
                    node: LibraryNode::Album { name, .. },
                    dest: Tab::Library,
                }) if name == "Album 11"
            )),
            "the drill rode out as the App's own fetch: {:?}",
            gui.pending
        );
        assert!(drilled_album(&gui).is_some(), "the view is the track list now");

        // The reply lands: rows on screen, and the Parent row is the way
        // back — its Activate pops the drill and the wall's list comes back
        // from the trail, without a request (library-rooms contract,
        // clause 6).
        let effects = gui.app.apply_event(Event::Library {
            node: LibraryNode::Album { name: "Album 11".into(), artist: Some("Artist 11".into()) },
            dest: Tab::Library,
            data: LibraryData::Tracks(vec![crate::api::types::Track {
                filepath: "music/a.mp3".into(),
                metadata: crate::api::types::TrackMetadata {
                    title: Some("Opening Night".into()),
                    duration: Some(200.0),
                    ..Default::default()
                },
            }]),
        });
        gui.pend(effects);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Album 11 — Artist 11"), "the header names the album");
        assert!(all.contains("Opening Night"), "its tracks are rows");

        gui.pending.clear();
        gui.act(Act::PaneRow(List::AlbumTracks, 0, RowVerb::Open));
        assert!(drilled_album(&gui).is_none(), "the Parent row walks back to the wall");
        assert!(
            !gui.pending
                .iter()
                .any(|e| matches!(e, Effect::Api(ApiCmd::Library { node: LibraryNode::Albums, .. }))),
            "the wall's list is back from the trail, not re-asked: {:?}",
            gui.pending
        );
        assert!(gui.app.library.entries.len() > 1, "the pane holds the album rows again");
    }

    #[test]
    fn covers_are_claimed_once_however_often_the_wall_draws() {
        let mut gui = wall_gui(4);
        draw(&mut gui);
        let asked = gui
            .pending
            .iter()
            .filter(|e| matches!(e, Effect::Api(ApiCmd::AlbumArt { .. })))
            .count();
        assert_eq!(asked, 4, "every cover on the page is asked for");

        // A second frame finds every slot claimed and asks for nothing.
        draw(&mut gui);
        let asked = gui
            .pending
            .iter()
            .filter(|e| matches!(e, Effect::Api(ApiCmd::AlbumArt { .. })))
            .count();
        assert_eq!(asked, 4, "the claim keeps the wall from asking twice");
    }

    #[test]
    fn a_decoded_cover_replaces_the_empty_slot_frame() {
        let mut gui = wall_gui(1);
        let rows = draw(&mut gui);
        let cell = GridShape::for_content(super::super::content_rect(100, 30, false, false)).cell(0);
        let top: String = rows[cell.y as usize]
            .chars()
            .skip(cell.x as usize)
            .take(COVER_W as usize)
            .collect();
        assert!(top.contains('╭'), "no art yet: the slot frame holds the cells: {top:?}");

        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let art = crate::tui::art::decode(&bytes.into_inner()).unwrap();
        gui.app.art.insert("aa00.jpeg".into(), Some(art));

        let rows = draw(&mut gui);
        let mid: String = rows[cell.y as usize + 2]
            .chars()
            .skip(cell.x as usize)
            .take(COVER_W as usize)
            .collect();
        assert!(
            mid.chars().all(|c| "█▀▄".contains(c)),
            "the mosaic holds the cells: {mid:?}"
        );
    }

    #[test]
    fn the_nav_fetches_the_wall_once_and_reuses_it_after() {
        let mut gui = wall_gui(3);
        gui.app.albums = None;
        gui.act(Act::Nav(super::super::ALBUMS_NAV));
        assert!(
            gui.pending
                .iter()
                .any(|e| matches!(e, Effect::Api(ApiCmd::Library { node: LibraryNode::Albums, .. }))),
            "the first visit asks for the list"
        );
        let all = draw(&mut gui).join("\n");
        assert!(all.contains(&t!("gui.alb.loading").to_string()));

        gui.pending.clear();
        gui.app.albums = Some(vec![album("Kept", "Standing", 2020, None)]);
        gui.act(Act::Nav(super::super::FILES_NAV));
        gui.act(Act::Nav(super::super::ALBUMS_NAV));
        assert!(gui.pending.is_empty(), "a return finds the wall standing");
        assert_eq!(gui.app.tab, Tab::Library, "but the tab still points at the pane");
    }
}

// ── Feasibility ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod feasibility {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::tui::art::Art;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// A distinct cover per album, with enough detail to exercise the
    /// samplers (a flat fill would flatter every path).
    fn a_cover(seed: u32) -> Art {
        let mut pixels = image::RgbImage::new(128, 128);
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            let v = ((x * (7 + seed)) ^ (y * (13 + seed))) as u8;
            *pixel = image::Rgb([v, v.wrapping_mul(3).wrapping_add(seed as u8), 255 - v]);
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        pixels.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        crate::tui::art::decode(&bytes.into_inner()).unwrap()
    }

    /// The page geometry the real wall uses with the queue folded away on
    /// a 100x30 window — every cell the page can hold.
    fn grid_rects() -> Vec<Rect> {
        let shape = GridShape::for_content(Rect { x: 2, y: 0, width: 80, height: 20 });
        (0..shape.capacity())
            .map(|i| {
                let cell = shape.cell(i);
                Rect { x: cell.x, y: cell.y, width: COVER_W, height: COVER_H }
            })
            .collect()
    }

    fn mosaic_slots(n: usize) -> Vec<Slot> {
        (0..n).map(|_| Slot::new(crate::tui::graphics::Graphics::disabled())).collect()
    }

    #[test]
    fn a_page_of_mosaic_covers_renders_in_one_frame() {
        let rects = grid_rects();
        let arts: Vec<Art> = (0..rects.len() as u32).map(a_cover).collect();
        let mut slots = mosaic_slots(rects.len());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        let start = Instant::now();
        terminal
            .draw(|frame| {
                let pace = Pace::frame();
                for ((slot, art), rect) in slots.iter_mut().zip(&arts).zip(grid_rects()) {
                    slot.draw_paced(frame, rect, art, &pace);
                }
            })
            .unwrap();
        let first = start.elapsed();

        // Every slot painted its whole box with mosaic cells.
        let buffer = terminal.backend().buffer();
        for rect in grid_rects() {
            let mut painted = 0;
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    if buffer[(x, y)].symbol() != " " {
                        painted += 1;
                    }
                }
            }
            assert_eq!(
                painted,
                usize::from(rect.width) * usize::from(rect.height),
                "a slot at {rect:?} left blank cells"
            );
        }

        let start = Instant::now();
        terminal
            .draw(|frame| {
                let pace = Pace::frame();
                for ((slot, art), rect) in slots.iter_mut().zip(&arts).zip(grid_rects()) {
                    slot.draw_paced(frame, rect, art, &pace);
                }
            })
            .unwrap();
        let second = start.elapsed();
        eprintln!("mosaic grid: {first:?} first frame, {second:?} cached");
        // Generous ceilings — a debug build on shared hardware. What they
        // guard is the shape of the cost: a page must render inside a
        // frame's budget, not a second's.
        assert!(first.as_millis() < 250, "first mosaic frame took {first:?}");
        assert!(second.as_millis() < 100, "cached mosaic frame took {second:?}");
    }

    #[test]
    fn pixel_covers_upgrade_within_the_budget_not_all_at_once() {
        use ratatui_image::picker::ProtocolType;

        for protocol in [ProtocolType::Kitty, ProtocolType::Sixel, ProtocolType::Iterm2] {
            let rects = grid_rects();
            let arts: Vec<Art> = (0..rects.len() as u32).map(a_cover).collect();
            let mut slots: Vec<Slot> = (0..rects.len())
                .map(|_| Slot::new(crate::tui::graphics::Graphics::forced(protocol)))
                .collect();
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

            // Frames run until every slot has upgraded to pixels. Each one
            // is bounded by the budget plus at most ONE encode that began
            // inside it — that overshoot is the price of not asking the
            // encoder how long it will take before starting it.
            let mut frames = 0;
            let mut worst = Duration::ZERO;
            while slots.iter().any(|slot| slot.key.is_none()) {
                frames += 1;
                assert!(frames <= 30, "{protocol:?}: covers never all upgraded");
                let start = Instant::now();
                terminal
                    .draw(|frame| {
                        let pace = Pace::frame();
                        for ((slot, art), rect) in
                            slots.iter_mut().zip(&arts).zip(grid_rects())
                        {
                            slot.draw_paced(frame, rect, art, &pace);
                        }
                    })
                    .unwrap();
                worst = worst.max(start.elapsed());
            }

            let start = Instant::now();
            terminal
                .draw(|frame| {
                    let pace = Pace::frame();
                    for ((slot, art), rect) in slots.iter_mut().zip(&arts).zip(grid_rects()) {
                        slot.draw_paced(frame, rect, art, &pace);
                    }
                })
                .unwrap();
            let cached = start.elapsed();
            eprintln!(
                "{protocol:?} grid: {frames} frames to upgrade, worst {worst:?}, cached {cached:?}"
            );
            // The budget plus one debug-build sixel encode, with headroom
            // for shared hardware: the old single-frame cost was 2 s.
            assert!(worst.as_millis() < 700, "{protocol:?}: a frame took {worst:?}");
            assert!(cached < worst, "{protocol:?}: the per-slot caches saved nothing");
        }
    }
}
