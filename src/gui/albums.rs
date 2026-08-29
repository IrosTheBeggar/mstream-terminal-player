//! The Albums wall: every album's cover, name and year, paged with
//! arrows — the webapp's album grid, worn terminal. Clicking an album
//! drills into its tracks through the App's own Library drill, so
//! playing, queueing and Back are the shared state machine's.
//!
//! Feasibility (2026-08-29, measured by the tests at the bottom): a page
//! of fifteen covers renders fine down both paths. The ▀-mosaic path is
//! character cells — a full page costs tens of milliseconds a frame with
//! a `CoverPane` per slot (the pane caches ONE resampled grid, so slots
//! may not share). The pixel path forks `Graphics` per slot the same way
//! (its cache holds ONE encoded picture) — but encoding is render-time
//! work on the thread the keyboard waits on, and a debug-build sixel
//! page measured two whole seconds. So encodes are PACED: each frame
//! spends at most [`ENCODE_BUDGET`] starting new encodes, the mosaic
//! stands in for slots not yet paid for, and a page turn upgrades to
//! pixels over a few frames instead of freezing on the first.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::{dim, scroll_list, table_view};
use crate::kit::theme::{legacy_conhost, th};
use crate::tui::app::{Action, Tab};
use crate::tui::art::Art;
use crate::tui::worker::LibraryNode;

use super::{Act, Gui, accent, bright_bold, put, sel};

/// How long one frame may spend STARTING cover encodes. Kitty pays ~1 ms
/// a grid-sized cover in release and the whole page lands in one frame;
/// debug-build sixel pays ~140 ms each and fills in one or two a frame —
/// which is the point: the budget bounds the frame, not the page.
const ENCODE_BUDGET: Duration = Duration::from_millis(40);

/// The cover's cells: 12x6 is square at the common 10x20 font.
const COVER_W: u16 = 12;
const COVER_H: u16 = 6;
/// Cover, name, year.
const CELL_H: u16 = COVER_H + 2;
/// Two columns of breathing room, one row.
const STRIDE_X: u16 = COVER_W + 2;
const STRIDE_Y: u16 = CELL_H + 1;

// ── State ───────────────────────────────────────────────────────────────────

/// One grid slot's drawing state: its own pixel-protocol cache and its
/// own mosaic grid, so neither thrashes when a whole page draws in one
/// frame.
struct Slot {
    graphics: crate::tui::graphics::Graphics,
    pane: crate::tui::viz::CoverPane,
    /// What this slot last drew as pixels — (art id, cell size). Matching
    /// means the protocol cache below is warm and drawing is free; a
    /// mismatch is an encode, which only the frame's budget may buy.
    key: Option<(u64, u16, u16)>,
}

impl Slot {
    fn new(graphics: crate::tui::graphics::Graphics) -> Slot {
        Slot { graphics, pane: Default::default(), key: None }
    }

    /// The cover, pixels where the terminal draws them and the budget
    /// allows, the ▀-mosaic otherwise — the card cover's two paths, paced.
    fn draw_paced(&mut self, frame: &mut Frame, rect: Rect, art: &Art, pace: &Pace) {
        let want = (art.id(), rect.width, rect.height);
        if self.key == Some(want) {
            // Warm: drawing is the cached protocol entry, microseconds.
            if self.graphics.draw(frame, rect, art) {
                return;
            }
            self.key = None;
        } else if pace.allows() {
            if self.graphics.draw(frame, rect, art) {
                self.key = Some(want);
                return;
            }
            self.key = None;
        } else {
            // Starving only happens once real encode time has been spent
            // this frame — a mosaic-only terminal never elapses the
            // budget, so this can't spin an idle session hot.
            pace.starved.set(true);
        }
        let mut canvas = crate::tui::canvas::Canvas::new(rect);
        if !canvas.is_empty() {
            self.pane.draw(&mut canvas, art);
            frame.render_widget(ratatui::widgets::Paragraph::new(canvas.into_lines()), rect);
        }
    }
}

/// One frame's encode allowance, shared by every slot it draws. A slot
/// the budget turned away marks it STARVED — the caller's cue to run the
/// next frame promptly instead of idling out the poll, so a page turn
/// finishes upgrading in tens of milliseconds of wall clock rather than
/// one encode per 100 ms tick.
struct Pace {
    start: Instant,
    budget: Duration,
    starved: std::cell::Cell<bool>,
}

impl Pace {
    fn frame() -> Pace {
        Pace { start: Instant::now(), budget: ENCODE_BUDGET, starved: std::cell::Cell::new(false) }
    }

    fn allows(&self) -> bool {
        self.start.elapsed() < self.budget
    }

    fn starved(&self) -> bool {
        self.starved.get()
    }
}

pub(crate) struct AlbumsUi {
    slots: Vec<Slot>,
    pub page: usize,
    /// The cell cursor within the page — the keyboard's hand on the grid.
    pub cursor: usize,
    /// The track view's wheel offset and reveal flag (the kit's table
    /// contract, same as Files).
    tscroll: usize,
    treveal: bool,
}

impl AlbumsUi {
    pub(crate) fn new() -> AlbumsUi {
        AlbumsUi { slots: Vec::new(), page: 0, cursor: 0, tscroll: 0, treveal: false }
    }

    /// A resize changes the cell-to-pixel mapping every slot encoded
    /// against, and kitty forgets its transmitted images with the old
    /// terminal — the card cover's rule, per slot.
    pub(crate) fn on_resize(&mut self) {
        for slot in &mut self.slots {
            slot.graphics.refresh();
            slot.key = None;
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
        let rows = (content.height.saturating_sub(2) / STRIDE_Y).max(1) as usize;
        GridShape { cols, rows, origin: (content.x, content.y + 2) }
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

/// The grid geometry the last frame drew with — recomputed from the same
/// inputs, so key handling agrees with the pointer about where cells are.
fn shape(gui: &Gui) -> GridShape {
    GridShape::for_content(super::content_rect(gui.last_width, gui.last_height, gui.queue_open))
}

fn turn_page(gui: &mut Gui, delta: i32) {
    let Some(albums) = gui.app.albums.as_ref() else { return };
    let pages = page_count(albums.len(), shape(gui).capacity());
    let page = gui.albums.page as i32 + delta;
    gui.albums.page = page.clamp(0, pages as i32 - 1) as usize;
    gui.albums.cursor = 0;
}

fn open_album(gui: &mut Gui, index_on_page: usize) {
    let capacity = shape(gui).capacity();
    let at = gui.albums.page * capacity + index_on_page;
    let Some(album) = gui.app.albums.as_ref().and_then(|a| a.get(at)).cloned() else { return };
    gui.albums.cursor = index_on_page;
    gui.albums.tscroll = 0;
    gui.albums.treveal = false;
    let node = LibraryNode::Album {
        name: album.name.clone().unwrap_or_default(),
        artist: album.artist.clone(),
    };
    let effects = gui.app.open_library_node(node, false);
    gui.pend(effects);
}

/// The albums side of [`Gui::act`]. Returns true when the act was ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act {
        Act::AlbPage(delta) => turn_page(gui, *delta),
        Act::AlbCell(i) => open_album(gui, *i),
        Act::AlbTrackRow(i) => {
            gui.app.tab = Tab::Library;
            gui.app.library.state.select(Some(*i));
            gui.albums.treveal = true;
            gui.forward(Action::Activate);
        }
        Act::AlbTrackQueue(i) => {
            gui.app.tab = Tab::Library;
            gui.app.library.state.select(Some(*i));
            gui.forward(Action::AddToQueue);
        }
        Act::AlbScrollBy(delta) => {
            gui.albums.tscroll = if *delta < 0 {
                gui.albums.tscroll.saturating_sub(1)
            } else {
                gui.albums.tscroll + 1
            };
        }
        Act::AlbScrollTo(first) => gui.albums.tscroll = *first,
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
                gui.albums.treveal = true;
                gui.forward(Action::Down);
            }
            KeyCode::Up => {
                gui.albums.treveal = true;
                gui.forward(Action::Up);
            }
            KeyCode::PageDown => {
                gui.albums.treveal = true;
                gui.forward(Action::PageDown);
            }
            KeyCode::PageUp => {
                gui.albums.treveal = true;
                gui.forward(Action::PageUp);
            }
            KeyCode::Enter => gui.forward(Action::Activate),
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
                gui.albums.cursor = (gui.albums.cursor + cols).min(on_page - 1);
            }
        }
        KeyCode::Up => gui.albums.cursor = gui.albums.cursor.saturating_sub(cols),
        KeyCode::Enter => {
            let cursor = gui.albums.cursor;
            open_album(gui, cursor);
        }
        _ => return None,
    }
    Some(false)
}

/// The wheel turns the wall a page at a time, and scrolls the track list.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if drilled_album(gui).is_some() {
        let _ = act(gui, &Act::AlbScrollBy(delta));
    } else {
        turn_page(gui, delta);
    }
}

/// How many albums the current page actually shows.
fn page_len(gui: &Gui, capacity: usize) -> usize {
    let total = gui.app.albums.as_ref().map_or(0, Vec::len);
    total.saturating_sub(gui.albums.page * capacity).min(capacity)
}

// ── Drawing ─────────────────────────────────────────────────────────────────

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !gui.app.connected {
        let text = if gui.app.connecting {
            (t!("busy.reaching").to_string(), accent())
        } else {
            (t!("gui.no_server").to_string(), dim())
        };
        put(frame, content.x, content.y, &super::bar::clip(&text.0, content.width as usize), text.1);
        return;
    }
    if let Some((name, artist)) = drilled_album(gui) {
        draw_tracks(frame, gui, content, &name, artist.as_deref());
    } else {
        draw_wall(frame, gui, content);
    }
}

fn draw_wall(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let heading = t!("gui.nav.albums").to_string();
    put(frame, content.x, content.y, &heading, Style::default().add_modifier(Modifier::BOLD));

    let Some(total) = gui.app.albums.as_ref().map(Vec::len) else {
        put(frame, content.x, content.y + 2, &t!("gui.alb.loading"), accent());
        return;
    };
    if total == 0 {
        put(frame, content.x, content.y + 2, &t!("gui.alb.empty"), dim());
        return;
    }

    let shape = GridShape::for_content(content);
    let capacity = shape.capacity();
    let pages = page_count(total, capacity);
    gui.albums.page = gui.albums.page.min(pages - 1);
    let page = gui.albums.page;
    let start = page * capacity;
    let shown = page_len(gui, capacity);
    gui.albums.cursor = gui.albums.cursor.min(shown.saturating_sub(1));

    put(
        frame,
        content.x + heading.chars().count() as u16 + 2,
        content.y,
        &t!("gui.alb.count", count = total),
        dim(),
    );

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
        let hover = live && gui.ui.pointer.is_some_and(|p| rect.contains(p));
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
        let Some(albums) = gui.app.albums.as_ref() else { return };
        albums
            .iter()
            .skip(start)
            .take(shown)
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

    // Split borrows: the page's rows are READ from the App while each
    // slot's caches are written — disjoint fields of the one Gui. This is
    // what lets a frame draw the wall without cloning a page of albums,
    // or worse, their decoded covers (fifty kilobytes of pixels apiece,
    // ten times a second, for nothing).
    let pace = Pace::frame();
    {
        let Gui { app, albums: wall, ui, .. } = &mut *gui;
        let Some(albums) = app.albums.as_ref() else { return };
        for (i, album) in albums.iter().skip(start).take(shown).enumerate() {
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
                Some(art) => wall.slots[i].draw_paced(frame, cover, art, &pace),
                None => {
                    // No cover (yet): the empty slot frame, the card's own
                    // idiom.
                    let block = ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(dim());
                    frame.render_widget(block, cover);
                }
            }

            let name = album.name.as_deref().unwrap_or("—");
            let name_y = cell.y + COVER_H;
            let selected = wall.cursor == i;
            let cell_hover = ui.pointer.is_some_and(|p| cell.contains(p));
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

fn draw_tracks(frame: &mut Frame, gui: &mut Gui, content: Rect, name: &str, artist: Option<&str>) {
    // The way back leads the header, then the album's own name.
    let back_glyph = if legacy_conhost() { "<" } else { "◂" };
    let back_label = format!("{back_glyph} {}", t!("gui.nav.albums"));
    let back = Rect {
        x: content.x,
        y: content.y,
        width: back_label.chars().count() as u16,
        height: 1,
    };
    let hover = gui.ui.pointer.is_some_and(|p| back.contains(p));
    put(frame, content.x, content.y, &back_label, if hover { bright_bold() } else { dim() });
    gui.ui.click(back, Act::AlbTrackRow(0)); // row 0 is the Parent row: Back

    let title_x = back.right() + 2;
    let title = match artist {
        Some(artist) => format!("{name} — {artist}"),
        None => name.to_string(),
    };
    put(
        frame,
        title_x,
        content.y,
        &super::bar::clip(&title, content.right().saturating_sub(title_x) as usize - 1),
        Style::default().add_modifier(Modifier::BOLD),
    );

    let entries = &gui.app.library.entries;
    if entries.len() <= 1 {
        put(frame, content.x, content.y + 2, &t!("busy.listing"), accent());
        return;
    }

    let list = Rect {
        x: content.x,
        y: content.y + 2,
        width: content.width - 2,
        height: content.height - 2,
    };
    let selected = gui.app.library.state.selected();
    let reveal = gui.albums.treveal.then_some(selected).flatten();
    gui.albums.treveal = false;
    let (first, visible) = table_view(entries.len(), reveal, gui.albums.tscroll, list.height as usize);
    gui.albums.tscroll = first;

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
        Act::AlbTrackRow,
        Act::AlbTrackQueue,
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::AlbScrollBy(-1),
        Act::AlbScrollBy(1),
        |first| Act::AlbScrollTo(first),
    );
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(gui.albums.page, 2, "the last page is the floor of forward");
        turn_page(&mut gui, -1);
        turn_page(&mut gui, -1);
        turn_page(&mut gui, -1);
        assert_eq!(gui.albums.page, 0, "and the first of back");
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
        // back — its Activate pops the drill and re-asks for the wall.
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

        let _ = act(&mut gui, &Act::AlbTrackRow(0));
        assert!(drilled_album(&gui).is_none(), "the Parent row walks back to the wall");
        assert!(
            gui.pending
                .iter()
                .any(|e| matches!(e, Effect::Api(ApiCmd::Library { node: LibraryNode::Albums, .. }))),
            "and the wall re-asks for its list"
        );
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
        let cell = GridShape::for_content(super::super::content_rect(100, 30, false)).cell(0);
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
    use super::*;
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
