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

use crate::kit::{ListView, STRIP_BUCKETS, dim, scroll_list};
use crate::kit::theme::{legacy_conhost, th};
use crate::tui::app::{Action, App};
use crate::api::types::Album;
use crate::tui::worker::LibraryNode;

use super::cover::{Pace, Slot};
use super::{Act, Gui, List, accent, bright_bold, put, sel};

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
    /// Whether the keyboard holds that hand: the cell is lit only then
    /// (the kit's list-cursor law); a card's click stows it.
    pub held: bool,
    /// The artist this wall is, when it is an artist's: another artist's
    /// wall starts on its own first page, not the last one's.
    pub of: Option<String>,
    /// The track view's viewport (the kit's table contract, same as Files).
    pub tracks: ListView,
    /// Which albums the wall shows, kept between frames — see [`WallView`].
    pub view: WallView,
}

impl WallState {
    /// The pointer acted, or a room opened: the keyboard's hand is down on
    /// the cells and the track list alike.
    pub(crate) fn stow(&mut self) {
        self.held = false;
        self.tracks.stow();
    }
}

/// The wall's view of its album list: which albums it shows, in order, and
/// the strip's index over their names.
///
/// A pure function of the list and the filter's needle, and it was being
/// rebuilt every frame and again for every key — while a filter stood, one
/// label formatted and lowercased per album: 5 ms a frame and three scans a
/// page turn at 20,000 albums (performance audit #103). Rebuilt only when
/// the key moves: a new list (the App's revision, and its length) or a new
/// needle. Every reader goes through [`visible_albums`], which checks the
/// key first — events land between a frame and the next key, and a view
/// trusted from the last draw could open the wrong album.
#[derive(Debug, Default)]
pub(crate) struct WallView {
    key: Option<(u64, usize, String)>,
    /// The shown albums' indices into the list; `None` while no filter
    /// stands — every album, in order, with nothing allocated to say so.
    shown: Option<Vec<usize>>,
    len: usize,
    /// The strip's (present, first_of) over the shown albums' names
    /// (library-rooms contract, clause 10).
    strip: ([bool; STRIP_BUCKETS], [usize; STRIP_BUCKETS]),
}

impl WallView {
    fn build(albums: &[Album], key: (u64, usize, String)) -> WallView {
        let needle = key.2.as_str();
        // The same match the pane's rows use (the bar contract's clause 13:
        // what you see is what the wall shows).
        let shown: Option<Vec<usize>> = (!needle.is_empty()).then(|| {
            albums
                .iter()
                .enumerate()
                .filter(|(_, album)| {
                    crate::tui::app::entries::album_label(album).to_lowercase().contains(needle)
                })
                .map(|(i, _)| i)
                .collect()
        });
        let len = shown.as_ref().map_or(albums.len(), Vec::len);
        let name = |at: usize| albums.get(at).and_then(|a| a.name.as_deref()).unwrap_or("");
        let strip = match &shown {
            Some(shown) => crate::kit::letter_index(shown.iter().map(|&at| name(at))),
            None => crate::kit::letter_index((0..albums.len()).map(name)),
        };
        WallView { key: Some(key), shown, len, strip }
    }

    fn len(&self) -> usize {
        self.len
    }

    /// The album shown at position `at`, as its index into the list.
    fn get(&self, at: usize) -> Option<usize> {
        match &self.shown {
            Some(shown) => shown.get(at).copied(),
            None => (at < self.len).then_some(at),
        }
    }

    /// The albums shown at positions `start..start + count` — one page.
    fn page(&self, start: usize, count: usize) -> Vec<usize> {
        (start..start + count).map_while(|at| self.get(at)).collect()
    }
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

pub(super) fn wall_ref(gui: &Gui) -> &WallState {
    if gui.active == super::ALBUMS_NAV { &gui.albums.wall } else { &gui.library.wall }
}

/// The albums the wall on screen shows: the Albums room's list, or the
/// drilled artist's, which the App keeps beside it.
pub(crate) fn wall_albums(gui: &Gui) -> Option<&Vec<Album>> {
    albums_of(&gui.app, on_root_wall(gui))
}

/// [`wall_albums`] from the App alone, so the wall's own state stays
/// writable beside it.
fn albums_of(app: &App, root: bool) -> Option<&Vec<Album>> {
    if root {
        return app.albums.as_ref();
    }
    match app.library_stack.here() {
        LibraryNode::Artist(artist) => {
            app.artist_albums.as_ref().filter(|(name, _)| name == artist).map(|(_, albums)| albums)
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
/// what the wall shows). Cached on the wall and rebuilt only when the list
/// or the needle moved (see [`WallView`]); `None` while there is no list.
fn visible_albums(gui: &mut Gui) -> Option<&WallView> {
    let needle = gui.app.library.filter.trim().to_lowercase();
    let root = on_root_wall(gui);
    let Gui { app, albums: albums_ui, library, .. } = gui;
    let albums = albums_of(app, root)?;
    let wall = if root { &mut albums_ui.wall } else { &mut library.wall };
    let key = (app.albums_rev, albums.len(), needle);
    if wall.view.key.as_ref() != Some(&key) {
        wall.view = WallView::build(albums, key);
    }
    Some(&wall.view)
}

/// The grid geometry the last frame drew with — recomputed from the same
/// inputs, so key handling agrees with the pointer about where cells are.
fn shape(gui: &mut Gui) -> GridShape {
    // The same rect the frame drew the grid in — a row lower under a pick's
    // banner, and a row lower again under both strips — else a click opened
    // the album a page's worth away.
    let both = both_strips(gui);
    GridShape::for_content(below_strips(gui.room_rect(), both))
}

/// Whether the root wall wears both strips: the mood strip on the row
/// under the controls and the letter strip — alphabetical, 25 albums or
/// more — under it (library-rooms clause 35). An artist's wall has no
/// Filters.
fn both_strips(gui: &mut Gui) -> bool {
    on_root_wall(gui)
        && super::moods::strip_stands(gui)
        && visible_albums(gui).is_some_and(|view| view.len() >= crate::kit::STRIP_MIN_ROWS)
}

/// The wall's rect: the letter strip and the grid a row lower while `both`
/// strips stand; otherwise the mood strip takes the letter strip's free
/// row, and nothing moves.
fn below_strips(content: Rect, both: bool) -> Rect {
    let shift = u16::from(both);
    Rect { y: content.y + shift, height: content.height.saturating_sub(shift), ..content }
}

fn turn_page(gui: &mut Gui, delta: i32) {
    let Some(total) = visible_albums(gui).map(WallView::len) else { return };
    let pages = page_count(total, shape(gui).capacity());
    let w = wall(gui);
    let page = w.page as i32 + delta;
    w.page = page.clamp(0, pages as i32 - 1) as usize;
    w.cursor = 0;
}

/// The strip's jump (library-rooms contract, clause 10): the page that
/// holds the visible album at `at`, with the cursor on it.
fn jump_wall(gui: &mut Gui, at: usize) {
    let capacity = shape(gui).capacity().max(1);
    let total = visible_albums(gui).map_or(0, WallView::len);
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
    let index = visible_albums(gui).and_then(|view| view.get(at));
    let Some(album) = index.and_then(|i| wall_albums(gui).and_then(|a| a.get(i))).cloned() else {
        return;
    };
    {
        let w = wall(gui);
        w.cursor = index_on_page;
        w.tracks.scroll = 0;
        // The track list inherits the hand: Enter keeps the keyboard's
        // cursor up, a card's click (stowed already) opens it unlit.
        w.tracks.held = w.held;
        w.tracks.reveal = w.held;
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
        // The pointer's acts stow the cell cursor (the kit's list-cursor
        // law): a card opens, a page turns, the strip jumps — nothing lit.
        Act::AlbPage(delta) => {
            wall(gui).held = false;
            turn_page(gui, *delta);
        }
        Act::AlbCell(i) => {
            wall(gui).held = false;
            open_album(gui, *i);
        }
        Act::AlbJump(at) => {
            wall(gui).held = false;
            jump_wall(gui, *at);
        }
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
        // The kit's list-cursor law, as the Files room: a walking key picks
        // the cursor up then walks, Enter and `a` want it up, Esc stows it
        // before it climbs.
        match key.code {
            KeyCode::Down => gui.walk(List::AlbumTracks, Action::Down),
            KeyCode::Up => gui.walk(List::AlbumTracks, Action::Up),
            KeyCode::PageDown => gui.walk(List::AlbumTracks, Action::PageDown),
            KeyCode::PageUp => gui.walk(List::AlbumTracks, Action::PageUp),
            KeyCode::Enter => gui.row_verb(List::AlbumTracks, Action::Activate),
            KeyCode::Esc if wall_ref(gui).tracks.held => wall(gui).tracks.stow(),
            KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Esc => {
                gui.forward(Action::Back);
            }
            KeyCode::Char('a') => gui.row_verb(List::AlbumTracks, Action::AddToQueue),
            KeyCode::Char('N') => gui.row_verb(List::AlbumTracks, Action::AddNext),
            KeyCode::Char('P') => gui.row_verb(List::AlbumTracks, Action::PlayNow),
            _ => return None,
        }
        return Some(false);
    }

    let (capacity, cols) = {
        let shape = shape(gui);
        (shape.capacity(), shape.cols)
    };
    let on_page = page_len(gui, capacity);
    // A walking key picks the cell cursor up — the first press shows it
    // where it rests, the next moves it — Enter wants it up, and Esc stows
    // it before it climbs (the kit's list-cursor law). A page turn is its
    // own feedback, so it turns at once.
    match key.code {
        KeyCode::Left | KeyCode::PageUp => {
            turn_page(gui, -1);
            wall(gui).held = true;
        }
        KeyCode::Right | KeyCode::PageDown => {
            turn_page(gui, 1);
            wall(gui).held = true;
        }
        KeyCode::Down => {
            let w = wall(gui);
            if w.held && on_page > 0 {
                w.cursor = (w.cursor + cols).min(on_page - 1);
            }
            w.held = true;
        }
        KeyCode::Up => {
            let w = wall(gui);
            if w.held {
                w.cursor = w.cursor.saturating_sub(cols);
            }
            w.held = true;
        }
        KeyCode::Esc if wall_ref(gui).held => wall(gui).held = false,
        KeyCode::Enter => {
            let w = wall_ref(gui);
            if w.held {
                let cursor = w.cursor;
                open_album(gui, cursor);
            }
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

/// The drilled album's footer: the row keys once the cursor is up, else
/// how to pick it up (the kit's list-cursor law).
pub(crate) fn tracks_tips(gui: &Gui) -> std::borrow::Cow<'static, str> {
    if wall_ref(gui).tracks.held { t!("gui.tips.album_tracks") } else { t!("gui.tips.tracks_stowed") }
}

/// How many albums the current page actually shows.
fn page_len(gui: &mut Gui, capacity: usize) -> usize {
    let total = visible_albums(gui).map_or(0, WallView::len);
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
        return content.x + crate::kit::width(&heading) as u16 + 2;
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

    let Some(total) = visible_albums(gui).map(WallView::len) else {
        put(frame, content.x, content.y + 3, &t!("gui.alb.loading"), accent());
        return;
    };
    let all = wall_albums(gui).map_or(0, Vec::len);
    super::draw_bar_controls(frame, gui, content, content.y + 1);
    // The mood strip on the row under the controls, the letter strip's
    // (library-rooms clause 35): where both stand the letter strip and the
    // grid stand a row lower.
    let mooded = super::moods::strip_stands(gui);
    super::moods::draw_strip(frame, gui, content, content.y + 2);
    let body = below_strips(content, on_root_wall(gui) && mooded && total >= crate::kit::STRIP_MIN_ROWS);

    let shape = GridShape::for_content(body);
    let capacity = shape.capacity();
    let pages = page_count(total, capacity);
    {
        let w = wall(gui);
        w.page = w.page.min(pages - 1);
    }
    let page = wall_ref(gui).page;
    // The pager's place: two arrows around "page/pages" at the line's
    // right, drawn once there are albums to page through.
    let label = format!("{}/{}", page + 1, pages);
    let fwd_x = content.right().saturating_sub(2);
    let label_x = fwd_x.saturating_sub(crate::kit::width(&label) as u16 + 1);
    let back_x = label_x.saturating_sub(2);
    let paged = all > 0 && total > 0;
    // The opener at its fixed column, left of the pager (clause 31) — drawn
    // before any empty wording, so the way back is always on screen.
    let opener = super::moods::offered_here(gui).then(|| {
        let w = crate::kit::width(&super::moods::opener_word(&rust_i18n::locale())) as u16;
        let at = super::moods::OpenerAt::BeforePager(if paged { back_x } else { u16::MAX });
        super::moods::opener_x(content, at, w)
    });
    // The count after the name, whatever the wall holds — the honest count
    // is never dropped (browser-top-bar clause 3): under a mood filter the
    // filtered wall's own, `n of m` under the text filter alone — ending two
    // cells before the opener, its words clipped where a wide pager pulls
    // the opener onto them.
    let count = super::bar_count(gui, super::discover::plural("gui.alb.count", all as u64));
    let cells = opener.map_or(usize::MAX, |x| super::moods::count_cells(count_x, x));
    put(frame, count_x, content.y, &super::bar::clip(&count, cells), dim());
    if let Some(x) = opener {
        super::moods::draw_opener(frame, gui, x, content.y);
    }
    if all == 0 {
        // Nothing under a mood filter is the filter's doing, not an empty
        // library (clause 38).
        let words = if mooded { t!("gui.files.empty") } else { t!("gui.alb.empty") };
        put(frame, content.x, body.y + 3, &words, dim());
        return;
    }
    if total == 0 {
        // The filter matched nothing; the way back is one key (clause 30).
        put(frame, content.x, body.y + 3, &t!("gui.files.empty"), dim());
        return;
    }

    let start = page * capacity;
    let shown = total.saturating_sub(start).min(capacity);
    {
        let w = wall(gui);
        w.cursor = w.cursor.min(shown.saturating_sub(1));
    }
    // The page's albums and the strip's index, copied off the view the top
    // of this frame brought up to date: the frame writes the Gui below
    // while it reads them.
    let (visible, strip) = {
        let view = &wall_ref(gui).view;
        (view.page(start, shown), view.strip)
    };

    // The strip (library-rooms contract, clause 10): the root wall is
    // alphabetical; an artist's albums are in the server's order.
    if on_root_wall(gui) && total >= crate::kit::STRIP_MIN_ROWS {
        let (present, first_of) = strip;
        let strip = Rect { x: content.x, y: body.y + 2, width: content.width, height: 1 };
        crate::kit::letter_strip(frame, &mut gui.ui, strip, &present, move |bucket| Act::AlbJump(first_of[bucket]));
    }

    // The pager: each end dim when there is nothing further that way.
    let (back_glyph, fwd_glyph) = if legacy_conhost() { ("<", ">") } else { ("◂", "▸") };
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
    // App's own fetch, the whole page in one batch (performance audit
    // #101). The page goes on record first, and only when it moved — a
    // page standing still costs a comparison, not a vector of names —
    // because what is on screen is what the cache must not evict
    // (performance audit #91).
    let moved: Option<Vec<String>> = {
        let Some(albums) = wall_albums(gui) else { return };
        // `visible` is already this page's slice of the wall's view.
        let page = || {
            visible
                .iter()
                .filter_map(|&at| albums.get(at))
                .filter_map(|album| album.album_art_file.as_deref())
        };
        (!gui.app.wall_on_view_is(page())).then(|| page().map(str::to_string).collect())
    };
    if let Some(page) = moved {
        gui.app.set_wall_on_view(page);
    }
    let fetches = gui.app.claim_wall_art();
    gui.pend(fetches);

    // The slots, one per cell, forked from the probed answer so each has
    // its own caches.
    while gui.albums.slots.len() < capacity {
        let fork = gui.app.graphics.fork();
        gui.albums.slots.push(Slot::new(fork));
    }
    // And no more than this window's widest page — the queue panel closed —
    // could use: a terminal that shrank lets the rest go (a kitty slot's
    // picture deleted after this frame), where they used to hold the
    // largest page ever drawn for the session; a panel toggle keeps its
    // warm slots, the queue's own slack (performance audit #95).
    let window = frame.area();
    let widest = GridShape::for_content(super::content_rect(
        window.width,
        window.height,
        false,
        gui.footer(),
    ));
    gui.albums.slots.truncate(capacity.max(widest.capacity()));
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
        for (i, album) in visible.iter().filter_map(|&at| albums.get(at)).enumerate() {
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
            let selected = wall.held && wall.cursor == i;
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
    // Back itself, not the Parent row's activation: the row is not there
    // while the tracks are still being asked for.
    gui.ui.click(back, Act::LibBack);

    let title_x = back.right() + 2;
    let name = if name.is_empty() { t!("gui.lib.singles").to_string() } else { name.to_string() };
    let title = match artist {
        Some(artist) => format!("{name} — {artist}"),
        None => name,
    };
    let count =
        super::bar_count(gui, super::discover::plural("gui.files.items", gui.app.library.counts().1 as u64));
    let count_x = super::right_at(content, &count);
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
        // The wait while the tracks are asked for; after a failed or empty
        // answer, the empty words — "listing…" forever said nothing true.
        if gui.app.library.loading {
            put(frame, content.x, content.y + 3, &t!("busy.listing"), accent());
        } else {
            put(frame, content.x, content.y + 3, &t!("gui.lib.empty"), dim());
        }
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
    // Lit only while the keyboard holds it (the kit's list-cursor law).
    let shown = wall_ref(gui).tracks.shown(selected);
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
        shown,
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
    use super::super::RowVerb;
    use super::*;
    use crate::tui::app::{Tab};
    use crate::api::types::Album;
    use crate::config::Config;
    use crate::tui::app::{App, Effect};
    use crate::tui::worker::{ApiCmd, Event, LibraryData};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// English words under assertion: the locale lock, and English pinned —
    /// the locale is process-wide, and the tests that switch it hold the
    /// lock while they draw in another language.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

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
            generation: gui.app.session_gen(),
            node: LibraryNode::Albums(crate::api::types::MoodFilter::NONE),
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

    /// Every kitty transmission a frame's cells carry: the image id's low
    /// 24 bits — what a placeholder's colour names — and the first pixel.
    fn transmissions(buffer: &ratatui::buffer::Buffer) -> Vec<(u32, [u8; 3])> {
        use base64::Engine;
        let mut sent = Vec::new();
        for cell in &buffer.content {
            let symbol = cell.symbol();
            for (at, _) in symbol.match_indices("a=T,") {
                let head = &symbol[..at];
                let id = &head[head.rfind("i=").unwrap() + 2..];
                let id: u32 = id.split(',').next().unwrap().parse().unwrap();
                let data = at + symbol[at..].find(';').unwrap() + 1;
                let pixel = &symbol[data..data + 4];
                let pixel = base64::engine::general_purpose::STANDARD.decode(pixel).unwrap();
                sent.push((id & 0xFF_FFFF, [pixel[0], pixel[1], pixel[2]]));
            }
        }
        sent
    }

    /// Every picture row on screen: its first cell, and the image id its
    /// placeholders name in their foreground colour.
    fn placeholders(buffer: &ratatui::buffer::Buffer) -> Vec<(u16, u16, u32)> {
        let area = *buffer.area();
        let mut rows = Vec::new();
        for y in 0..area.height {
            for x in 0..area.width {
                let symbol = buffer[(x, y)].symbol();
                if let Some(at) = symbol.find("\x1b[38;2;")
                    && symbol.contains('\u{10EEEE}')
                {
                    let rest = &symbol[at + 7..];
                    let rgb: Vec<u32> =
                        rest[..rest.find('m').unwrap()].split(';').map(|v| v.parse().unwrap()).collect();
                    rows.push((x, y, (rgb[0] << 16) | (rgb[1] << 8) | rgb[2]));
                }
            }
        }
        rows
    }

    #[test]
    fn an_overlay_opening_as_the_page_turns_leaves_no_cover_of_the_last_page() {
        // The header dropdown opens on the frame the next page's pixels go
        // out, over the first cells they ride in. A slot keeps one kitty id
        // for life (#94), so its placeholders showed what the id held — the
        // last page's cover — until the slot drew another album. The
        // frame's check sends what it lost with the next frame that draws
        // the picture (the integration check of performance audit #95).
        use ratatui_image::picker::ProtocolType;
        use std::collections::HashMap;

        type Store = HashMap<u32, [u8; 3]>;
        fn frame(gui: &mut Gui, terminal: &mut Terminal<TestBackend>, store: &mut Store) {
            terminal.draw(|frame| super::super::render(frame, gui)).unwrap();
            store.extend(transmissions(terminal.backend().buffer()));
        }
        fn settle(gui: &mut Gui, terminal: &mut Terminal<TestBackend>, store: &mut Store) {
            for _ in 0..20 {
                frame(gui, terminal, store);
                if !gui.hot && !gui.ui.stale() {
                    break;
                }
            }
        }

        let mut gui = wall_gui(60);
        // Saved servers enough for the dropdown to reach the wall's first row.
        for i in 0..8 {
            gui.config.servers.push(crate::config::ServerEntry {
                url: format!("http://server{i}.local:3000"),
                ..Default::default()
            });
        }
        gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
        let colour =
            |i: usize| [(i * 37 % 251) as u8 + 1, (i * 91 % 241) as u8 + 1, (i * 13 % 239) as u8 + 1];
        for i in 0..60 {
            let rgb = (0..128 * 128).flat_map(|_| colour(i)).collect();
            let art = crate::tui::art::Art::from_rgb(128, 128, rgb).unwrap();
            gui.app.art.insert(format!("aa{i:02}.jpeg"), Some(art));
        }
        // What the terminal's image store holds, as the frames fill it.
        let mut store = HashMap::new();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        settle(&mut gui, &mut terminal, &mut store);

        gui.act(Act::AlbPage(1));
        gui.act(Act::SrvMenu);
        frame(&mut gui, &mut terminal, &mut store);
        gui.act(Act::SrvCloseDrop);
        settle(&mut gui, &mut terminal, &mut store);

        let shape = GridShape::for_content(gui.room_rect());
        let page = wall_ref(&gui).page;
        assert_eq!(page, 1);
        let shown = placeholders(terminal.backend().buffer());
        assert!(shown.len() >= shape.capacity(), "a page of pictures: {}", shown.len());
        for (x, y, id) in shown {
            let Some(i) = (0..shape.capacity()).find(|&i| {
                let cell = shape.cell(i);
                x == cell.x && (cell.y..cell.y + COVER_H).contains(&y)
            }) else {
                continue;
            };
            let album = page * shape.capacity() + i;
            assert_eq!(store.get(&id), Some(&colour(album)), "cell {i}, row {y}: album {album}'s");
        }
    }

    #[test]
    fn the_slots_follow_the_window_not_the_largest_page_ever_drawn() {
        // Each slot holds an encoded picture; a window that shrank used to
        // keep the big page's worth for the session (performance audit #95).
        fn slots_at(gui: &mut Gui, width: u16, height: u16) -> usize {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| super::super::render(frame, gui)).unwrap();
            gui.albums.slots.len()
        }
        let widest = |width, height| {
            GridShape::for_content(super::super::content_rect(width, height, false, false)).capacity()
        };
        let mut gui = wall_gui(300);
        assert_eq!(slots_at(&mut gui, 200, 60), widest(200, 60));
        // The queue panel takes a few columns of cells; their slots stay
        // warm for the panel closing again.
        gui.queue_open = true;
        assert_eq!(slots_at(&mut gui, 200, 60), widest(200, 60));
        // A smaller window lets the rest go.
        assert_eq!(slots_at(&mut gui, 120, 40), widest(120, 40));
        assert!(widest(120, 40) < widest(200, 60));
    }

    #[test]
    fn the_bar_filters_the_wall_and_gates_its_verbs() {
        let _en = english();
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
    fn a_standing_filter_is_matched_once_not_every_frame() {
        let _en = english();
        // Performance audit #103: the view stands between frames and keys,
        // and a new needle is what rebuilds it.
        let mut gui = wall_gui(25);
        gui.app.library.apply_filter("album 1".into());
        draw(&mut gui);
        let built = gui.albums.wall.view.shown.as_ref().map(|shown| shown.as_ptr());
        assert!(built.is_some(), "a filter stands, so the view names its albums");
        draw(&mut gui);
        turn_page(&mut gui, 1);
        assert_eq!(
            gui.albums.wall.view.shown.as_ref().map(|shown| shown.as_ptr()),
            built,
            "the next frame and the key read the same view"
        );

        gui.app.library.apply_filter("album 12".into());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Album 12"), "the new needle narrows the wall:\n{text}");
        assert!(!text.contains("Album 13"), "and the old view is gone:\n{text}");
        assert!(text.contains("1 of 25"), "the count agrees:\n{text}");

        gui.app.library.apply_filter(String::new());
        assert!(visible_albums(&mut gui).is_some_and(|view| view.shown.is_none() && view.len() == 25));
    }

    #[test]
    fn a_click_after_the_filter_moved_opens_from_the_new_view() {
        // Events land between a frame and the next key: the view the last
        // frame drew is stale by then, and the cell must map through the
        // current one.
        let mut gui = wall_gui(25);
        gui.app.library.apply_filter("album 07".into());
        draw(&mut gui);
        gui.app.library.apply_filter("album 1".into());
        gui.pending.clear();
        gui.act(Act::AlbCell(0));
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::Library { node: LibraryNode::Album { name, .. }, .. })
                    if name == "Album 10"
            )),
            "the first cell of the new view: {:?}",
            gui.pending
        );
    }

    #[test]
    fn a_new_list_of_the_same_length_re_letters_the_strip() {
        // The unfiltered view is only a length — but the strip reads the
        // names, and the server can answer again with as many albums (a
        // re-ask, a switch to a library of the same size). The App's
        // revision is what tells the lists apart.
        let mut gui = wall_gui(30);
        draw(&mut gui);
        let (a, z) = (crate::kit::letter_bucket("A"), crate::kit::letter_bucket("Z"));
        assert!(gui.albums.wall.view.strip.0[a] && !gui.albums.wall.view.strip.0[z]);

        let zebras = (0..30).map(|i| album(&format!("Zebra {i:02}"), "Herd", 2000, None)).collect();
        let effects = gui.app.apply_event(Event::Library {
            generation: gui.app.session_gen(),
            node: LibraryNode::Albums(crate::api::types::MoodFilter::NONE),
            dest: Tab::Library,
            data: LibraryData::Albums(zebras),
        });
        gui.pend(effects);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Zebra 00"), "the new list is on the wall:\n{text}");
        let present = gui.albums.wall.view.strip.0;
        assert!(present[z] && !present[a], "and the strip reads its names: {present:?}");
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
        let _en = english();
        let mut gui = wall_gui(25);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Album 00"), "page one leads with the first album");
        assert!(all.contains("2000 · Arti"), "the year and artist ride under the name");
        assert!(all.contains("1/3"), "25 albums at 10 a page is three pages");
        assert!(all.contains(&super::super::discover::plural("gui.alb.count", 25)));

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
    fn the_wall_lights_a_cell_only_for_the_keyboard() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // The kit's list-cursor law on the grid: no cell lit on arrival,
        // Enter opens nothing yet, ↓ picks the cursor up where it rests,
        // the next ↓ moves it a row, Esc stows it, a card's click stows it.
        let mut gui = wall_gui(25);
        draw(&mut gui);
        gui.pending.clear(); // the nav's own fetch and the covers are not under test
        assert!(!gui.albums.wall.held, "no cell lit on arrival");
        let key = |gui: &mut Gui, code| {
            super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
        };
        let opened = |gui: &Gui| {
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::Library { node: LibraryNode::Album { .. }, .. })
            ))
        };
        key(&mut gui, KeyCode::Enter);
        assert!(!opened(&gui), "Enter on a stowed cursor opens nothing");
        key(&mut gui, KeyCode::Down);
        assert!(gui.albums.wall.held && gui.albums.wall.cursor == 0, "↓ lights the resting cell, unmoved");
        key(&mut gui, KeyCode::Down);
        assert!(gui.albums.wall.cursor > 0, "the next ↓ moves a row down");
        key(&mut gui, KeyCode::Esc);
        assert!(!gui.albums.wall.held, "Esc stows it");
        key(&mut gui, KeyCode::Down);
        gui.act(Act::AlbCell(1));
        assert!(!gui.albums.wall.held, "a card's click stows the cursor");
        assert!(!gui.albums.wall.tracks.held, "and the tracks open unlit");
        assert!(opened(&gui), "but the album opens: {:?}", gui.pending);
    }

    #[test]
    fn a_return_to_the_wall_stands_over_its_own_drill() {
        use crate::tui::worker::LibraryNode;
        // The wall drew the cached albums over whatever list the shared pane
        // held from another room: its verbs, count and Back ran on that.
        let mut gui = wall_gui(5);
        let effects = gui.app.open_library_node(LibraryNode::Recent(crate::api::types::MoodFilter::NONE), true);
        gui.pend(effects);
        gui.pending.clear();
        gui.act(Act::Nav(super::super::ALBUMS_NAV));
        assert!(matches!(gui.app.library_node(), LibraryNode::Albums(_)), "the drill is the wall's again");
        assert_eq!(gui.app.library.entries.len(), 6, "seated from the cache: the parent and five albums");
        assert!(
            !gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Library { .. }))),
            "without a request: {:?}",
            gui.pending
        );
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
                    ..
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
            generation: gui.app.session_gen(),
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
                .any(|e| matches!(e, Effect::Api(ApiCmd::Library { node: LibraryNode::Albums(_), .. }))),
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
    fn the_covers_a_frame_claims_go_out_without_a_wait() {
        // The loop sends them at the top of its next pass; the wait before
        // it is none, where it was a poll after a page turn's frame
        // (performance audit #82 follow-up). Sent, and claimed once, they
        // leave the frame after waiting as before.
        use std::time::Duration;
        let mut gui = wall_gui(4);
        draw(&mut gui);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AlbumArt { .. }))));
        assert_eq!(super::super::next_wait(&gui), Duration::ZERO, "the claims go out now");
        gui.pending.clear(); // the next pass's dispatch
        draw(&mut gui);
        assert!(gui.pending.is_empty(), "claimed once");
        assert!(super::super::next_wait(&gui) > Duration::ZERO, "nothing to send: the loop waits");
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
        let _en = english();
        let mut gui = wall_gui(3);
        gui.app.albums = None;
        gui.act(Act::Nav(super::super::ALBUMS_NAV));
        assert!(
            gui.pending
                .iter()
                .any(|e| matches!(e, Effect::Api(ApiCmd::Library { node: LibraryNode::Albums(_), .. }))),
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

    // ── The mood Filters (library-rooms contract, clauses 30–36) ─────────

    use crate::api::types::{MoodFilter, MoodLevel, MoodScale};
    use super::super::moods::{MoodAct, MoodStop, is_open};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;

    /// The wall with the queue open — the content's 47 cells, x17–x63, the
    /// boards' — on a server with scores to filter by.
    fn mood_wall(n: usize) -> Gui {
        let mut gui = wall_gui(n);
        gui.queue_open = true;
        gui.app.capabilities.mood_filters = true;
        gui
    }

    fn press(gui: &mut Gui, code: KeyCode) -> bool {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// The row's cells `from..to`, one char a cell (the rows tested are
    /// narrow glyphs).
    fn cells(row: &str, from: usize, to: usize) -> String {
        row.chars().skip(from).take(to - from).collect()
    }

    /// The x a needle starts at on a drawn row, in cells.
    fn x_of(row: &str, needle: &str) -> u16 {
        let at = row.find(needle).unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        crate::kit::width(&row[..at]) as u16
    }

    fn library_asks(gui: &Gui) -> Vec<LibraryNode> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Library { node, .. }) => Some(node.clone()),
                _ => None,
            })
            .collect()
    }

    fn land_albums(gui: &mut Gui, moods: MoodFilter, names: &[&str]) {
        let effects = gui.app.apply_event(Event::Library {
            generation: gui.app.session_gen(),
            node: LibraryNode::Albums(moods),
            dest: Tab::Library,
            data: LibraryData::Albums(names.iter().map(|n| album(n, "Artist", 2001, None)).collect()),
        });
        gui.pend(effects);
    }

    #[test]
    fn the_walls_opener_ends_at_x51_and_o_opens_the_modal_with_the_hand_on_happy() {
        let _en = english();
        let mut gui = wall_gui(2);
        gui.queue_open = true;
        let all = draw(&mut gui).join("\n");
        assert!(!all.contains("mood"), "no scores to filter by, no opener: {all}");
        assert!(!press(&mut gui, KeyCode::Char('o')) && !is_open(&gui), "and `o` is dead");

        gui.app.capabilities.mood_filters = true;
        let rows = draw(&mut gui);
        // Line one: the name, the count after it, the opener at its fixed
        // column, the pager at the right — every item a cell in from x63.
        let line = &rows[2];
        assert_eq!((x_of(line, "Albums"), x_of(line, "2 albums"), x_of(line, "mood")), (17, 25, 48), "{line}");
        assert_eq!(x_of(line, "◂ 1/1 ▸"), 56, "{line}");
        assert_eq!(gui.ui.hit(Position::new(48, 2)), Some(Act::Mood(MoodAct::Open { key: false })));
        assert_eq!(gui.ui.hit(Position::new(51, 2)), Some(Act::Mood(MoodAct::Open { key: false })));

        assert!(!press(&mut gui, KeyCode::Char('o')));
        assert!(is_open(&gui));
        let draft = gui.moods.draft.unwrap();
        assert_eq!((draft.hand, draft.shown), (MoodStop::Pick(MoodScale::Valence, "happy"), true));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        // The box at x17–x82, r10–r19, the title on its top border.
        assert!(rows[10][..].contains("╭─ Filters ─"), "{all}");
        assert_eq!(x_of(&rows[10], "╭"), 17);
        assert_eq!(x_of(&rows[10], "╮"), 82);
        assert_eq!(x_of(&rows[19], "╰"), 17);
        let at = |r: usize, needle: &str| x_of(&rows[r], needle);
        assert_eq!((at(11, "( ) Happy"), at(11, "( ) Neutral"), at(11, "( ) Sad")), (19, 45, 67), "{all}");
        assert_eq!((at(12, "( ) Relaxed"), at(12, "( ) Aggressive")), (19, 67), "{all}");
        assert_eq!((at(13, "( ) Instrumental"), at(13, "( ) Vocals")), (19, 45), "{all}");
        assert_eq!((at(14, "[ ] Acoustic"), at(14, "[ ] Party"), at(14, "[ ] Danceable")), (19, 45, 67), "{all}");
        assert!(cells(&rows[15], 18, 82).trim().is_empty(), "{all}");
        assert_eq!((at(16, "( ) Strict"), at(16, "(•) Standard"), at(16, "( ) Loose")), (19, 45, 67), "{all}");
        assert!(cells(&rows[17], 18, 82).trim().is_empty(), "{all}");
        assert_eq!(at(18, "[ Done ]"), 73, "{all}");
        assert!(!all.contains("clear") && !all.contains("SORT"), "nothing to clear, no Sort on the wall: {all}");
        assert_eq!(gui.ui.hit(Position::new(20, 11)), Some(Act::Mood(MoodAct::Press(MoodStop::Pick(MoodScale::Valence, "happy")))));
        assert_eq!(gui.ui.hit(Position::new(75, 18)), Some(Act::Mood(MoodAct::Press(MoodStop::Done))));
        // `o` again closes it, as Done: nothing changed, nothing asked.
        gui.pending.clear();
        press(&mut gui, KeyCode::Char('o'));
        assert!(!is_open(&gui) && library_asks(&gui).is_empty(), "{:?}", gui.pending);
    }

    #[test]
    fn done_asks_once_under_the_draft_and_the_strip_takes_the_free_row() {
        let _en = english();
        let mut gui = mood_wall(3);
        draw(&mut gui);
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        assert!(!gui.moods.draft.unwrap().shown, "a click opens with the hand stowed");
        gui.pending.clear();
        for stop in [
            MoodStop::Pick(MoodScale::Energy, "relaxed"),
            MoodStop::Pick(MoodScale::Voice, "instrumental"),
            MoodStop::Level(MoodLevel::Strict),
        ] {
            gui.act(Act::Mood(MoodAct::Press(stop)));
        }
        assert!(library_asks(&gui).is_empty(), "nothing asked while the modal is up");
        assert!(gui.moods.albums.filter.is_empty(), "the room's choice waits for Done");
        let rows = draw(&mut gui);
        assert!(cells(&rows[4], 17, 64).trim().is_empty(), "no strip while the draft is open: {}", rows.join("\n"));
        assert!(rows[18].contains("clear   [ Done ]"), "{}", rows.join("\n"));

        press(&mut gui, KeyCode::Enter);
        assert!(!is_open(&gui));
        let wire = MoodFilter::NONE
            .press(MoodScale::Energy, "relaxed")
            .press(MoodScale::Voice, "instrumental")
            .with_level(MoodLevel::Strict);
        assert_eq!(library_asks(&gui), vec![LibraryNode::Albums(wire)], "one ask");
        assert_eq!(
            serde_json::to_value(wire).unwrap(),
            serde_json::json!({ "energy": "relaxed", "voice": "instrumental", "threshold": 0.7 })
        );
        // The old wall stands until the answer lands; one asked under the old
        // filter is dropped.
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Album 02"), "{all}");
        land_albums(&mut gui, MoodFilter::NONE, &["Old 1", "Old 2"]);
        land_albums(&mut gui, wire, &["Stim Pack"]);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Stim Pack") && !all.contains("Old 1") && !all.contains("Album 02"), "{all}");
        // The strip in the free row under the controls; the grid unmoved.
        assert_eq!(x_of(&rows[4], "Relaxed · Instrumental · Strict [X]"), 17, "{all}");
        assert_eq!(x_of(&rows[11], "Stim Pack"), 17, "the names stand on r11, the grid from r5: {all}");
        assert_eq!(gui.ui.hit(Position::new(30, 4)), Some(Act::Mood(MoodAct::Open { key: false })));
        assert_eq!(gui.ui.hit(Position::new(50, 4)), Some(Act::Mood(MoodAct::Drop)));
        // The same choice again asks nothing.
        gui.pending.clear();
        press(&mut gui, KeyCode::Char('o'));
        press(&mut gui, KeyCode::Esc);
        assert!(library_asks(&gui).is_empty(), "{:?}", gui.pending);
    }

    #[test]
    fn the_strips_x_lets_everything_go_and_asks_once_the_sort_kept() {
        let _en = english();
        let mut gui = mood_wall(3);
        let party = MoodFilter::NONE.toggle_tag("party").with_level(MoodLevel::Loose);
        let set = super::super::moods::RoomMoods { filter: party, sort: crate::api::types::MoodSort::Match };
        let _ = super::super::moods::apply(&mut gui, super::super::ALBUMS_NAV, set);
        land_albums(&mut gui, party, &["Party Album"]);
        let rows = draw(&mut gui);
        assert_eq!(x_of(&rows[4], "Party · Loose [X]"), 17, "{}", rows.join("\n"));
        gui.pending.clear();
        gui.act(Act::Mood(MoodAct::Drop));
        assert_eq!(library_asks(&gui), vec![LibraryNode::Albums(MoodFilter::NONE)]);
        assert_eq!(gui.moods.albums.filter, MoodFilter::NONE, "the level back to Standard");
        assert_eq!(gui.moods.albums.sort, crate::api::types::MoodSort::Match, "the Sort kept");
        let rows = draw(&mut gui);
        assert!(!rows[4].contains("[X]"), "{}", rows.join("\n"));
    }

    #[test]
    fn the_room_is_inert_under_the_modal() {
        let mut gui = mood_wall(3);
        gui.demo = Some(super::super::demo_now());
        draw(&mut gui);
        press(&mut gui, KeyCode::Char('o'));
        draw(&mut gui);
        let before = (gui.queue_open, gui.active, gui.demo_paused);
        for code in [KeyCode::Char(' '), KeyCode::Char('1'), KeyCode::Char('G'), KeyCode::Char('f'), KeyCode::Char('e')] {
            assert!(!press(&mut gui, code), "{code:?}");
        }
        // Tab walks the stops, it does not fold the queue; q types nothing
        // and quits nothing.
        press(&mut gui, KeyCode::Tab);
        assert!(!press(&mut gui, KeyCode::Char('q')));
        assert_eq!((gui.queue_open, gui.active, gui.demo_paused), before);
        assert!(is_open(&gui) && !gui.app.filtering);
        // The wall beneath and the nav take no click; the wheel turns nothing.
        draw(&mut gui);
        assert_eq!(gui.ui.hit(Position::new(20, 6)), Some(Act::Guard), "the wall");
        assert_eq!(gui.ui.hit(Position::new(5, 2)), Some(Act::Guard), "the nav");
        assert_eq!(gui.ui.hit(Position::new(90, 12)), Some(Act::Guard), "the queue");
        let page = gui.albums.wall.page;
        gui.wheel(Position::new(30, 6), 1);
        assert_eq!(gui.albums.wall.page, page);
        // Ctrl-C still quits.
        assert!(super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn a_level_with_no_mood_asks_nothing_and_rides_with_the_first_mood() {
        let _en = english();
        let mut gui = mood_wall(3);
        draw(&mut gui);
        gui.pending.clear();
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Level(MoodLevel::Loose))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Done)));
        assert!(library_asks(&gui).is_empty(), "a lone level filters nothing: {:?}", gui.pending);
        assert_eq!(gui.moods.albums.filter.level(), MoodLevel::Loose, "but it is kept");
        let rows = draw(&mut gui);
        assert!(!rows[4].contains("Loose"), "and the strip waits for a mood: {}", rows.join("\n"));
        // The modal opens on it; the first mood carries it.
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        let rows = draw(&mut gui);
        assert!(rows[16].contains("(•) Loose") && rows[18].contains("clear"), "{}", rows.join("\n"));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Tag("party"))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Done)));
        let wire = MoodFilter::NONE.toggle_tag("party").with_level(MoodLevel::Loose);
        assert_eq!(library_asks(&gui), vec![LibraryNode::Albums(wire)]);
        assert_eq!(serde_json::to_value(wire).unwrap(), serde_json::json!({ "tags": ["party"], "threshold": 0.3 }));
    }

    #[test]
    fn a_filtered_alphabetical_wall_puts_the_letter_strip_under_the_mood_strip() {
        let _en = english();
        let mut gui = mood_wall(30);
        let rows = draw(&mut gui);
        assert_eq!(x_of(&rows[4], "#ABC"), 17, "the letter strip on r4: {}", rows.join("\n"));
        assert_eq!(super::shape(&mut gui).cell(0).y, 5);

        let relaxed = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        let next = super::super::moods::RoomMoods { filter: relaxed, ..Default::default() };
        let _ = super::super::moods::apply(&mut gui, super::super::ALBUMS_NAV, next);
        let names: Vec<String> = (0..30).map(|i| format!("Album {i:02}")).collect();
        land_albums(&mut gui, relaxed, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert_eq!(x_of(&rows[4], "Relaxed [X]"), 17, "the mood strip first: {all}");
        assert_eq!(x_of(&rows[5], "#ABC"), 17, "the letter strip a row lower: {all}");
        assert_eq!(x_of(&rows[12], "Album 00"), 17, "the grid a row lower: {all}");
        // The keys' grid is the drawn one.
        assert_eq!(super::shape(&mut gui).cell(0).y, 6);
    }

    #[test]
    fn an_empty_answer_under_a_mood_reads_nothing_here_and_keeps_the_opener_and_strip() {
        let _en = english();
        let mut gui = mood_wall(3);
        let sad = MoodFilter::NONE.press(MoodScale::Valence, "sad");
        gui.moods.albums.filter = sad;
        gui.act(Act::Nav(super::super::FILES_NAV));
        gui.act(Act::Nav(super::super::ALBUMS_NAV));
        assert!(library_asks(&gui).contains(&LibraryNode::Albums(sad)), "{:?}", gui.pending);
        land_albums(&mut gui, sad, &[]);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains(&t!("gui.files.empty").to_string()), "{all}");
        assert!(!all.contains(&t!("gui.alb.empty").to_string()), "not an empty library: {all}");
        assert_eq!(x_of(&rows[2], "mood"), 48, "{all}");
        assert!(rows[2].contains("0 albums"), "the count stands: {all}");
        assert_eq!(x_of(&rows[4], "Sad [X]"), 17, "{all}");
    }

    #[test]
    fn at_80_cells_the_opener_ends_at_x84_the_line_at_x95_and_the_modal_is_banded() {
        let _en = english();
        // The queue folded: the room's 80 cells, x17–x96.
        let mut gui = wall_gui(1);
        gui.app.capabilities.mood_filters = true;
        let rows = draw(&mut gui);
        let line = &rows[2];
        // One album reads its own form; the opener ends 13 in, the pager
        // a cell in from the content's last.
        assert_eq!((x_of(line, "1 album "), x_of(line, "mood")), (25, 81), "{line}");
        assert_eq!(x_of(line, "◂ 1/1 ▸") + 7, 96, "the pager ends at x95: {line}");

        // Everything set: the strip ends by x95, its [X] standing.
        let all = MoodFilter::NONE
            .press(MoodScale::Valence, "happy")
            .press(MoodScale::Energy, "aggressive")
            .press(MoodScale::Voice, "instrumental")
            .toggle_tag("acoustic")
            .toggle_tag("party")
            .toggle_tag("danceable")
            .with_level(MoodLevel::Strict);
        let next = super::super::moods::RoomMoods { filter: all, ..Default::default() };
        let _ = super::super::moods::apply(&mut gui, super::super::ALBUMS_NAV, next);
        land_albums(&mut gui, all, &["Everything"]);
        let rows = draw(&mut gui);
        let all_rows = rows.join("\n");
        assert_eq!(x_of(&rows[4], "Happy · Aggressive"), 17, "{all_rows}");
        assert!(x_of(&rows[4], "[X]") + 2 <= 95, "{all_rows}");
        assert_eq!(x_of(&rows[2], "mood"), 81, "the opener never moves: {all_rows}");

        // The modal: centred in the room at x24–x89, r10–r19, and the room's
        // cells x17–x96 on its rows blank beside it; the rows above and
        // below stay the room's.
        press(&mut gui, KeyCode::Char('o'));
        let rows = draw(&mut gui);
        let all_rows = rows.join("\n");
        assert_eq!((x_of(&rows[10], "╭─ Filters"), x_of(&rows[10], "╮")), (24, 89), "{all_rows}");
        assert_eq!((x_of(&rows[19], "╰"), x_of(&rows[19], "╯")), (24, 89), "{all_rows}");
        for (r, row) in rows.iter().enumerate().take(20).skip(10) {
            assert!(cells(row, 17, 24).trim().is_empty() && cells(row, 90, 97).trim().is_empty(), "r{r}: {all_rows}");
        }
        assert_eq!(x_of(&rows[4], "Happy · Aggressive"), 17, "the strip stands over the band: {all_rows}");
        assert_eq!(gui.ui.hit(Position::new(20, 12)), Some(Act::Guard), "the band is inert");
        assert_eq!(x_of(&rows[11], "(•) Happy"), 26, "the draft opens on the choice: {all_rows}");
    }
}

// ── Feasibility ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod feasibility {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::gui::cover::ENCODE_BUDGET;
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
            // encoder how long it will take before starting it. So what is
            // checked is when each encode began, not how long a frame took:
            // a debug-build sixel encode is ~50 ms on the dev Mac and ~750
            // ms on a shared CI runner with the suite running beside it, so
            // no fixed ceiling on the frame fits both (a 700 ms one failed
            // there, 2026-09-27 twice, while a whole page encoded at once
            // here stays under it).
            let mut frames = 0;
            let mut worst = Duration::ZERO;
            while slots.iter().any(|slot| slot.key.is_none()) {
                frames += 1;
                assert!(frames <= 30, "{protocol:?}: covers never all upgraded");
                let start = Instant::now();
                let mut late = Vec::new();
                terminal
                    .draw(|frame| {
                        let pace = Pace::frame();
                        // No earlier than the pace's own start, so an
                        // encode the pace allowed began before this clock
                        // reached the budget too.
                        let opened = Instant::now();
                        for ((slot, art), rect) in
                            slots.iter_mut().zip(&arts).zip(grid_rects())
                        {
                            let (encodes, at) = (slot.encodes(), opened.elapsed());
                            slot.draw_paced(frame, rect, art, &pace);
                            if slot.encodes() > encodes && at >= ENCODE_BUDGET {
                                late.push(at);
                            }
                        }
                    })
                    .unwrap();
                worst = worst.max(start.elapsed());
                assert!(late.is_empty(), "{protocol:?}: frame {frames} began encodes at {late:?}");
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
            assert!(cached < worst, "{protocol:?}: the per-slot caches saved nothing");
        }
    }
}
