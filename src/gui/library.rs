//! The Library rooms — Artists, Genres, Recent, Rated (docs/ux-contracts/
//! library-rooms.md): the shared App's Library pane under the browse top
//! bar, with the letter strip on the alphabetical lists, an artist opening
//! as a wall of its albums (the Albums room's wall, generalized), and a
//! genre, Recent or Rated listing tracks the drilled album's way.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::{GenreSource, MoodFilter};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, STRIP_MIN_ROWS, dim, letter_index, letter_strip, scroll_list};
use crate::tui::app::{Action, App, Entry, Tab};
use crate::tui::worker::LibraryNode;

use super::albums::{self, WallState};
use super::{
    ARTISTS_NAV, Act, GENRES_NAV, Gui, LAST_PLAYED_NAV, List, MOST_PLAYED_NAV, RATED_NAV, RECENT_NAV, accent,
    bright_bold, moods, put,
};

// ── State ───────────────────────────────────────────────────────────────────

/// What the three rooms keep for themselves: the list's wheel offset and
/// reveal flag (the kit's table contract, as Files), and the artist wall's
/// own state (library-rooms contract, clause 7).
#[derive(Debug, Default)]
pub(crate) struct LibraryUi {
    pub view: ListView,
    pub wall: WallState,
}

impl LibraryUi {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// The root node a nav row opens, when the row is one of ours.
pub(crate) fn root_of(active: usize) -> Option<LibraryNode> {
    match active {
        // The rooms' mood filters, and the Genres room's kind, are put in
        // by [`open`].
        ARTISTS_NAV => Some(LibraryNode::Artists(MoodFilter::NONE)),
        GENRES_NAV => Some(LibraryNode::Genres(GenreSource::default())),
        RECENT_NAV => Some(LibraryNode::Recent(MoodFilter::NONE)),
        RATED_NAV => Some(LibraryNode::Rated(MoodFilter::NONE)),
        LAST_PLAYED_NAV => Some(LibraryNode::RecentlyPlayed),
        MOST_PLAYED_NAV => Some(LibraryNode::MostPlayed),
        _ => None,
    }
}

fn is_room(gui: &Gui) -> bool {
    root_of(gui.active).is_some()
}

/// The nav row chosen (entry point 1): the root list, fresh — the rooms
/// share one pane, and "recent" means now (clauses 4 and the flows).
pub(crate) fn open(gui: &mut Gui, root: LibraryNode) {
    gui.library.view.scroll = 0;
    gui.library.view.reveal = false;
    // Fresh, and with the cursor stowed (the kit's list-cursor law).
    gui.library.view.stow();
    gui.library.wall = WallState::default();
    // The Genres room opens on the kind it was left on (clause 26), the
    // Artists, Recent and Rated rooms under their mood filters where the
    // server offers them (clause 30) — and a song list in its Sort's order.
    let root = match root {
        LibraryNode::Genres(_) => LibraryNode::Genres(remembered_source(gui)),
        LibraryNode::Artists(_) => LibraryNode::Artists(moods::asked(gui, ARTISTS_NAV)),
        LibraryNode::Recent(_) => {
            gui.app.songs_by_match = moods::by_match(gui, RECENT_NAV);
            LibraryNode::Recent(moods::asked(gui, RECENT_NAV))
        }
        LibraryNode::Rated(_) => {
            gui.app.songs_by_match = moods::by_match(gui, RATED_NAV);
            LibraryNode::Rated(moods::asked(gui, RATED_NAV))
        }
        other => other,
    };
    let effects = gui.app.open_library_node(root, true);
    gui.pend(effects);
}

/// The kind `[gui] genre_source` keeps — tags when it keeps none.
fn remembered_source(gui: &Gui) -> GenreSource {
    GenreSource::from_name(gui.config.gui.genre_source.as_deref())
}

/// The kind the Genres switch lights, when the switch is offered: the
/// list's own — the other one, when the asked kind had none — or, while a
/// list of another kind is on its way, the kind asked for (clause 27).
fn switch_on(gui: &Gui) -> Option<GenreSource> {
    let LibraryNode::Genres(asked) = *gui.app.library_stack.here() else { return None };
    let (kept, list) = gui.app.genre_list.as_ref()?;
    list.offers_choice().then_some(if *kept == asked { list.shown } else { asked })
}

/// Whether the drill stands on a wall or inside an album — the views the
/// Albums room's code draws and drives.
fn wall_view(gui: &Gui) -> bool {
    matches!(gui.app.library_stack.here(), LibraryNode::Artist(_) | LibraryNode::Album { .. })
}

/// Whether the view on screen is a root list, whose `..` row leads nowhere.
fn at_root(gui: &Gui) -> bool {
    matches!(
        gui.app.library_stack.here(),
        LibraryNode::Artists(_)
            | LibraryNode::Genres(_)
            | LibraryNode::Recent(_)
            | LibraryNode::Rated(_)
            | LibraryNode::RecentlyPlayed
            | LibraryNode::MostPlayed
    )
}

/// The rows the list draws: everything but the `..` row at a root (it
/// would climb into the TUI's mode menu), and how many rows that passed
/// over — a drawn row's pane index is its position plus that. Borrows the
/// App alone, so the shell's own state stays writable beside it.
///
/// A slice past the first row rather than a filtered copy: `..` only ever
/// leads a listing (every listing is built with it first, and a filter
/// keeps the order), and the copy was a 20,000-artist Vec every frame to
/// drop one row (performance audit #109).
fn rows_of(app: &App, skip_parent: bool) -> (usize, &[Entry]) {
    let entries = &app.library.entries;
    let skip = usize::from(skip_parent && matches!(entries.first(), Some(Entry::Parent)));
    (skip, &entries[skip..])
}

/// A root list's cursor never rests on the hidden `..`.
pub(super) fn keep_off_parent(gui: &mut Gui) {
    if at_root(gui)
        && gui.app.library.state.selected() == Some(0)
        && gui.app.library.entries.len() > 1
    {
        gui.app.library.state.select(Some(1));
    }
}

/// Whether the list on screen is alphabetical — Artists and Genres are; Recent
/// is newest first and a track list is an album's or a genre's order
/// (clauses 10 and 12).
fn alphabetical(gui: &Gui) -> bool {
    matches!(gui.app.library_stack.here(), LibraryNode::Artists(_) | LibraryNode::Genres(_))
}

// ── Drawing ─────────────────────────────────────────────────────────────────

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !gui.app.connected {
        super::servers::draw_disconnected(frame, gui, content, 0);
        return;
    }
    if wall_view(gui) {
        // Another artist's wall starts on its own first page with the cursor
        // stowed: the page and the cell were the last artist's.
        if let LibraryNode::Artist(name) = gui.app.library_stack.here()
            && gui.library.wall.of.as_deref() != Some(name.as_str())
        {
            let name = name.clone();
            gui.library.wall = WallState { of: Some(name), ..WallState::default() };
        }
        albums::draw(frame, gui, content);
        return;
    }
    draw_list(frame, gui, content);
}

/// The room's header: the room's name at a root; the way back and the
/// genre's name inside one. Returns where the count may start.
fn draw_heading(frame: &mut Frame, gui: &mut Gui, content: Rect) -> u16 {
    let node = gui.app.library_stack.here().clone();
    match node {
        LibraryNode::Genre(genre, _) => {
            let (back_glyph, forward) = if legacy_conhost() { ("<", ">") } else { ("◂", "▸") };
            let back_label = format!("{back_glyph} {}", t!("gui.nav.genres"));
            let back =
                Rect { x: content.x, y: content.y, width: back_label.chars().count() as u16, height: 1 };
            let hover = gui.ui.hovers(back);
            put(frame, content.x, content.y, &back_label, if hover { bright_bold() } else { dim() });
            gui.ui.click(back, Act::LibBack);
            let title = format!("{forward} {genre}");
            let x = back.right() + 1;
            let shown = super::bar::clip(&title, content.width.saturating_sub(back.width + 18) as usize);
            put(frame, x, content.y, &shown, Style::default().add_modifier(Modifier::BOLD));
            x + shown.chars().count() as u16 + 2
        }
        _ => {
            let mut heading = super::NAV[gui.active].label().to_string();
            // A peer is read-only, and the crumb says so (multi-server
            // clause 26, library-rooms clause 16) — as the Files bar does.
            if gui.app.session.peer.is_some() {
                heading = format!("{heading} · {}", t!("gui.srv.read_only"));
            }
            put(frame, content.x, content.y, &heading, Style::default().add_modifier(Modifier::BOLD));
            content.x + crate::kit::width(&heading) as u16 + 2
        }
    }
}

/// The Genres list's Tags · Generated · Both (clauses 25–29), right-aligned
/// on the controls line where a list of tracks wears its verbs — a list of
/// genres has none: the kind on screen in the ok colour, the others dim,
/// each a click. Only while the server has both kinds, and not while the
/// filter's field holds the line (the bar contract, clause 20).
fn draw_genre_switch(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let Some(on) = switch_on(gui).filter(|_| !gui.app.filtering) else { return };
    let sep = " · ";
    let sep_width = crate::kit::width(sep) as u16;
    let labels = GenreSource::ALL.map(source_label);
    let width = labels.iter().map(|label| crate::kit::width(label) as u16).sum::<u16>()
        + sep_width * (labels.len() as u16 - 1);
    // The verbs' right edge: a cell in from the count's.
    let mut x = content.right().saturating_sub(width + 1);
    for (i, (source, label)) in GenreSource::ALL.into_iter().zip(&labels).enumerate() {
        if i > 0 {
            put(frame, x, y, sep, dim());
            x += sep_width;
        }
        let rect = Rect { x, y, width: crate::kit::width(label) as u16, height: 1 };
        let style = if gui.ui.hovers(rect) {
            bright_bold()
        } else if source == on {
            Style::default().fg(th().ok).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        put(frame, x, y, label, style);
        gui.ui.click(rect, Act::GenreSource(source));
        gui.ui.tip(rect, t!("gui.lib.genre_source_tip").to_string());
        x += rect.width;
    }
}

fn source_label(source: GenreSource) -> std::borrow::Cow<'static, str> {
    match source {
        GenreSource::Tag => t!("gui.lib.genre_tags"),
        GenreSource::Model => t!("gui.lib.genre_generated"),
        GenreSource::Both => t!("gui.lib.genre_both"),
    }
}

fn draw_list(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let count_x = draw_heading(frame, gui, content);
    let count = super::bar_count(gui, super::discover::plural("gui.files.items", gui.app.library.counts().1 as u64));
    put(frame, super::right_at(content, &count).max(count_x), content.y, &count, dim());
    // The mood opener two cells after the room's name, a fixed column the
    // count never moves (clause 31), wherever the room offers the filters —
    // whatever the controls line holds.
    if moods::offered_here(gui) {
        let name = crate::kit::width(&super::NAV[gui.active].label()) as u16;
        let w = crate::kit::width(&moods::opener_word(&rust_i18n::locale())) as u16;
        moods::draw_opener(frame, gui, moods::opener_x(content, moods::OpenerAt::AfterName(name), w), content.y);
    }
    super::draw_bar_controls(frame, gui, content, content.y + 1);
    // The Genres switch at the controls line's right (clause 27).
    draw_genre_switch(frame, gui, content, content.y + 1);
    // The mood strip on the row under the controls (clause 35), before any
    // wait or empty wording: the way back is always on screen.
    let mooded = moods::strip_stands(gui);
    moods::draw_strip(frame, gui, content, content.y + 2);

    let root = at_root(gui);
    let alpha = alphabetical(gui);
    let (skip, rows) = rows_of(&gui.app, root);
    if rows.is_empty() {
        // The wait, while there are no rows to stand in for the answer — a
        // mood's re-ask leaves its list's on screen until it lands (clause
        // 34), the album's way.
        if gui.app.library.loading {
            put(frame, content.x, content.y + 3, &t!("busy.listing"), accent());
            return;
        }
        // An empty answer, or a filter that matched nothing — the text
        // filter or a mood's (the bar contract's rule: the way back is one
        // key; clause 38).
        let words = if gui.app.library.filter.is_empty() && !mooded {
            t!("gui.lib.empty")
        } else {
            t!("gui.files.empty")
        };
        put(frame, content.x, content.y + 3, &words, dim());
        return;
    }

    // The strip on an alphabetical list of 25 or more (clauses 10–12) —
    // under the mood strip where that stands, the list a row lower.
    let lettered = alpha && rows.len() >= STRIP_MIN_ROWS;
    let shift = u16::from(mooded && lettered);
    let body = Rect { y: content.y + shift, height: content.height.saturating_sub(shift), ..content };
    if lettered {
        // The narrowed list's letters (clause 11), by drawn position.
        let (present, first_of) = letter_index(rows.iter().map(Entry::label));
        let strip = Rect { x: content.x, y: body.y + 2, width: content.width, height: 1 };
        letter_strip(frame, &mut gui.ui, strip, &present, move |bucket| Act::LibJump(first_of[bucket]));
    }

    let list = Rect {
        x: content.x,
        y: body.y + 3,
        width: content.width - 2,
        height: body.height.saturating_sub(3),
    };
    let selected = gui.app.library.state.selected();
    // Off the drawn rows when it rests on the hidden `..`.
    let sel_pos = selected.and_then(|s| s.checked_sub(skip)).filter(|&pos| pos < rows.len());
    let (first, visible) = gui.library.view.window(rows.len(), sel_pos, list.height as usize);
    // Lit only while the keyboard holds it (the kit's list-cursor law).
    let lit = gui.library.view.shown(selected);

    let len = rows.len();
    let shown: Vec<(usize, &Entry)> =
        rows.iter().enumerate().skip(first).take(visible).map(|(pos, entry)| (skip + pos, entry)).collect();
    let playing = gui.app.now_playing.as_ref().map(|t| t.filepath.as_str());
    super::draw_pane_rows(
        frame,
        &mut gui.ui,
        playing,
        &shown,
        list,
        lit,
        List::Library,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::ScrollBy(List::Library, -1),
        Act::ScrollBy(List::Library, 1),
        |first| Act::ScrollTo(List::Library, first),
    );
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// The strip's jump (clause 10): the row at `pos` (in the drawn rows) comes
/// to the top and takes the cursor — unlit, the strip being a pointer tool
/// (clause 13, the kit's list-cursor law); ↓ picks it up there.
fn jump_list(gui: &mut Gui, pos: usize) {
    let (skip, rows) = rows_of(&gui.app, at_root(gui));
    let target = (pos < rows.len()).then_some(skip + pos);
    if let Some(index) = target {
        gui.library.view.scroll = pos;
        gui.library.view.stow();
        gui.app.tab = Tab::Library;
        gui.app.library.state.select(Some(index));
    }
}

/// The rooms' side of [`Gui::act`]. True when the act was ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act {
        Act::LibJump(pos) => jump_list(gui, *pos),
        Act::LibBack => {
            gui.app.tab = Tab::Library;
            gui.forward(Action::Back);
            // The parent comes back with its cursor (clause 6): in view.
            gui.library.view.reveal = true;
        }
        Act::GenreSource(source) => switch_source(gui, *source),
        _ => return false,
    }
    true
}

/// A kind chosen on the Genres switch (clause 27): kept in `[gui]
/// genre_source`, and the list asked again in place — the kind already on
/// screen asks nothing.
fn switch_source(gui: &mut Gui, source: GenreSource) {
    if gui.config.gui.genre_source.as_deref() != Some(source.as_str()) {
        gui.config.gui.genre_source = Some(source.as_str().to_string());
        gui.save_now();
    }
    if switch_on(gui).is_none_or(|on| on == source) {
        return;
    }
    gui.library.view.scroll = 0;
    gui.library.view.reveal = false;
    gui.library.view.stow();
    gui.app.tab = Tab::Library;
    let effects = gui.app.switch_genre_source(source);
    gui.pend(effects);
}

/// The rooms' keys: the wall's and the drilled album's are the Albums
/// room's; a list takes the Files room's. `None` when the room is not up.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !is_room(gui) || !gui.app.connected {
        return None;
    }
    if wall_view(gui) {
        return albums::handle_key(gui, key);
    }
    // A walking key picks the cursor up, then walks it; the row verbs want
    // it up; Esc stows it before it climbs (the kit's list-cursor law).
    match key.code {
        KeyCode::Down => gui.walk(List::Library, Action::Down),
        KeyCode::Up => {
            gui.walk(List::Library, Action::Up);
            keep_off_parent(gui);
        }
        KeyCode::PageDown => gui.walk(List::Library, Action::PageDown),
        KeyCode::PageUp => {
            gui.walk(List::Library, Action::PageUp);
            keep_off_parent(gui);
        }
        KeyCode::Enter => gui.row_verb(List::Library, Action::Activate),
        KeyCode::Esc if gui.library.view.held => gui.library.view.stow(),
        // Back climbs a level; a root has none, and says nothing.
        KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Esc => {
            if !at_root(gui) {
                gui.forward(Action::Back);
                // The parent's cursor, restored from the trail, in view.
                gui.library.view.reveal = true;
            }
        }
        KeyCode::Char('a') => gui.row_verb(List::Library, Action::AddToQueue),
        KeyCode::Char('N') => gui.row_verb(List::Library, Action::AddNext),
        KeyCode::Char('P') => gui.row_verb(List::Library, Action::PlayNow),
        _ => return None,
    }
    Some(false)
}

/// The footer's keys for whichever view is up: the wall's, the drilled
/// album's, or a list's — the Files room's line, since the keys are its.
pub(crate) fn tips(gui: &Gui) -> std::borrow::Cow<'static, str> {
    match gui.app.library_stack.here() {
        LibraryNode::Artist(_) => t!("gui.tips.albums"),
        LibraryNode::Album { .. } => albums::tracks_tips(gui),
        _ if gui.app.filtering => t!("gui.tips.filter"),
        // Stowed, how to pick the cursor up and what works without a row
        // (the kit's list-cursor law).
        _ if gui.library.view.held => t!("gui.tips.files"),
        _ => t!("gui.tips.files_stowed"),
    }
}

/// The wheel: the wall turns pages and the track list scrolls (the Albums
/// room's rule); a list scrolls its view.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if wall_view(gui) {
        albums::wheel(gui, delta);
    } else {
        gui.act(Act::ScrollBy(List::Library, delta));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;

    use super::super::{
        ARTISTS_NAV, Act, FILES_NAV, GENRES_NAV, Gui, LAST_PLAYED_NAV, List, MOST_PLAYED_NAV, RATED_NAV, RECENT_NAV,
        RowVerb, render,
    };
    use crate::api::types::{
        Album, Genre, GenreList, GenreSource, GenreSources, MoodFilter, MoodLevel, MoodScale, MoodSong, MoodSort, Track,
    };
    use crate::config::Config;
    use crate::tui::app::{App, Effect, Tab};
    use crate::tui::worker::{ApiCmd, Event, LibraryData, LibraryNode};

    /// English words under assertion: the locale lock, and English pinned —
    /// the locale is process-wide, and the tests that switch it hold the
    /// lock while they draw in another language.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    fn session_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some("http://host:3000".into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = "http://host:3000".into();
        gui.app.session.server_id = "http://host:3000".into();
        gui.queue_open = false;
        gui
    }

    fn track(path: &str, seconds: f64) -> Track {
        let mut track = Track { filepath: path.into(), metadata: Default::default() };
        track.metadata.duration = Some(seconds);
        track
    }

    fn album(name: Option<&str>, artist: &str, year: Option<i32>) -> Album {
        Album {
            name: name.map(str::to_string),
            artist: Some(artist.into()),
            year,
            album_art_file: None,
        }
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        draw_sized(gui, 100, 40)
    }

    fn draw_sized(gui: &mut Gui, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn hit_text(gui: &Gui, rows: &[String], needle: &str) -> Option<Act> {
        let y = rows.iter().position(|r| r.contains(needle))?;
        let x = rows[y].char_indices().position(|(i, _)| rows[y][i..].starts_with(needle))? as u16;
        gui.ui.hit(Position { x, y: y as u16 })
    }

    fn asked(gui: &Gui, node: &LibraryNode) -> bool {
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Library { node: n, .. }) if n == node))
    }

    fn land(gui: &mut Gui, node: LibraryNode, data: LibraryData) {
        let effects = gui.app.apply_event(Event::Library { generation: gui.app.session_gen(), node, dest: Tab::Library, data });
        gui.pend(effects);
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn artists_gui(names: &[&str]) -> Gui {
        let mut gui = session_gui();
        gui.act(Act::Nav(ARTISTS_NAV));
        assert!(asked(&gui, &LibraryNode::Artists(crate::api::types::MoodFilter::NONE)), "the row asks for the list");
        gui.pending.clear();
        land(&mut gui, LibraryNode::Artists(crate::api::types::MoodFilter::NONE), LibraryData::Artists(names.iter().map(|n| n.to_string()).collect()));
        gui
    }

    #[test]
    fn each_room_opens_its_root_fresh_with_the_pane_and_no_play_verbs() {
        let _en = english();
        let mut gui = artists_gui(&["Bassnectar", "Portishead"]);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Bassnectar") && all.contains("Portishead"), "{all}");
        assert!(all.contains("2 items"), "{all}");
        assert!(!all.contains("▸ play"), "containers carry no verbs: {all}");
        assert!(!rows.iter().any(|r| r.contains("  ..")), "no .. at a root: {all}");
        assert_eq!(gui.app.library.state.selected(), Some(1), "the cursor rests on the first name");

        // Down into an artist, then away: a return opens the root list
        // again — from the session's copy, with no second ask for all of it
        // (performance audit #99).
        gui.act(Act::PaneRow(List::Library, 1, RowVerb::Open));
        assert!(asked(&gui, &LibraryNode::Artist("Bassnectar".into())));
        gui.act(Act::Nav(GENRES_NAV));
        assert!(asked(&gui, &LibraryNode::Genres(GenreSource::Tag)), "tags, with no kind remembered");
        gui.act(Act::Nav(RECENT_NAV));
        assert!(asked(&gui, &LibraryNode::Recent(MoodFilter::NONE)), "Recent asks on every visit");
        gui.pending.clear();
        gui.act(Act::Nav(ARTISTS_NAV));
        assert!(matches!(gui.app.library_stack.here(), LibraryNode::Artists(_)), "a room restarts at its root");
        assert!(!asked(&gui, &LibraryNode::Artists(crate::api::types::MoodFilter::NONE)), "seated from the session's copy");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Bassnectar") && all.contains("Portishead") && all.contains("2 items"), "{all}");
        // The room chosen again while it is up asks afresh: the way to see
        // a rescan.
        gui.act(Act::Nav(ARTISTS_NAV));
        assert!(asked(&gui, &LibraryNode::Artists(crate::api::types::MoodFilter::NONE)), "a second choice reloads");
    }

    #[test]
    fn genres_show_their_counts_and_open_into_tracks_with_the_verbs() {
        let _en = english();
        let mut gui = session_gui();
        gui.act(Act::Nav(GENRES_NAV));
        gui.pending.clear();
        land(
            &mut gui,
            LibraryNode::Genres(GenreSource::Tag),
            LibraryData::Genres(GenreList {
                genres: vec![
                    Genre { name: "Ambient".into(), track_count: Some(4) },
                    Genre { name: "Techno".into(), track_count: Some(9) },
                ],
                ..GenreList::default()
            }),
        );
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Ambient (4)") && all.contains("Techno (9)"), "{all}");
        assert_eq!(hit_text(&gui, &rows, "Techno"), Some(Act::PaneRow(List::Library, 2, RowVerb::Open)));
        gui.act(Act::PaneRow(List::Library, 2, RowVerb::Open));
        let techno = LibraryNode::Genre("Techno".into(), GenreSource::Both);
        assert!(asked(&gui, &techno), "a genre opens into its tracks — all of them, from a server without the filter");
        land(&mut gui, techno, LibraryData::Tracks(vec![track("t/a.mp3", 61.0), track("t/b.mp3", 125.0)]));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("◂ Genres") && all.contains("▸ Techno"), "{all}");
        assert!(all.contains("a.mp3") && all.contains("1:01") && all.contains("2:05"), "track rows: {all}");
        assert!(all.contains("▸ play"), "tracks carry the bar's verbs: {all}");
        assert_eq!(hit_text(&gui, &rows, "◂ Genres"), Some(Act::LibBack));
        key(&mut gui, KeyCode::Char('h'));
        assert!(matches!(gui.app.library_stack.here(), LibraryNode::Genres(_)), "Back climbs to the genres");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Ambient (4)"), "the parent comes back from the trail: {all}");
    }

    /// The cell `needle` starts at on `row`.
    fn col(row: &str, needle: &str) -> usize {
        row.char_indices().position(|(i, _)| row[i..].starts_with(needle)).unwrap_or_else(|| panic!("{needle}: {row}"))
    }

    /// A Genres list of `shown`'s kind, from a server with both kinds.
    fn genre_list(shown: GenreSource, names: &[(&str, u32)]) -> LibraryData {
        LibraryData::Genres(GenreList {
            genres: names.iter().map(|(name, n)| Genre { name: (*name).into(), track_count: Some(*n) }).collect(),
            sources: Some(GenreSources { tag: 2, model: 3 }),
            shown,
        })
    }

    fn genres_gui() -> Gui {
        let mut gui = session_gui();
        gui.act(Act::Nav(GENRES_NAV));
        assert!(asked(&gui, &LibraryNode::Genres(GenreSource::Tag)));
        gui.pending.clear();
        land(&mut gui, LibraryNode::Genres(GenreSource::Tag), genre_list(GenreSource::Tag, &[("ambient", 4), ("techno", 9)]));
        gui
    }

    #[test]
    fn the_genre_switch_sits_right_on_the_controls_line_and_the_filter_leads_it() {
        let _en = english();
        let mut gui = genres_gui();
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        let y = rows.iter().position(|r| r.contains("Tags · Generated · Both")).unwrap_or_else(|| panic!("{all}"));
        let (heading, line) = (&rows[y - 1], &rows[y]);
        assert_eq!(col(line, "/ filter"), col(heading, "Genres"), "the filter leads the line, under the heading: {all}");
        assert_eq!(
            col(line, "Both") + "Both".len(),
            col(heading, "items") + "items".len(),
            "the switch ends where the bar's verbs and the count do, a cell in from the content's last: {all}"
        );
        assert_eq!(hit_text(&gui, &rows, "Generated"), Some(Act::GenreSource(GenreSource::Model)));
        assert_eq!(hit_text(&gui, &rows, "/ filter"), Some(Act::BarFilter), "the filter still opens");
        assert_eq!(super::switch_on(&gui), Some(GenreSource::Tag), "the kind on screen is lit");

        // The filter's field takes the whole line while it is open.
        gui.act(Act::BarFilter);
        let all = draw(&mut gui).join("\n");
        assert!(!all.contains("Generated"), "{all}");
    }

    #[test]
    fn a_kind_chosen_relists_in_place_is_remembered_and_rides_into_a_genres_songs() {
        let _en = english();
        let mut gui = genres_gui();
        gui.act(Act::GenreSource(GenreSource::Model));
        assert!(asked(&gui, &LibraryNode::Genres(GenreSource::Model)), "the list is asked again");
        assert_eq!(gui.app.library_stack.here(), &LibraryNode::Genres(GenreSource::Model), "in place");
        assert_eq!(gui.config.gui.genre_source.as_deref(), Some("model"), "remembered");
        assert_eq!(super::switch_on(&gui), Some(GenreSource::Model), "lit while the list is out");
        gui.pending.clear();
        land(&mut gui, LibraryNode::Genres(GenreSource::Model), genre_list(GenreSource::Model, &[("Synthwave", 2)]));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Synthwave (2)") && !all.contains("techno"), "{all}");

        // A generated genre's songs are its generated songs; Back keeps the kind.
        gui.act(Act::PaneRow(List::Library, 1, RowVerb::Open));
        let synthwave = LibraryNode::Genre("Synthwave".into(), GenreSource::Model);
        assert!(asked(&gui, &synthwave));
        land(&mut gui, synthwave, LibraryData::Tracks(vec![track("s/a.mp3", 90.0)]));
        let all = draw(&mut gui).join("\n");
        assert!(!all.contains("Generated"), "no switch over a genre's songs: {all}");
        key(&mut gui, KeyCode::Char('h'));
        assert_eq!(gui.app.library_stack.here(), &LibraryNode::Genres(GenreSource::Model));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Synthwave (2)") && all.contains("Tags · Generated · Both"), "{all}");

        // The kind already on screen asks nothing; the room comes back on
        // the remembered kind, from the session's copy.
        gui.pending.clear();
        gui.act(Act::GenreSource(GenreSource::Model));
        assert!(gui.pending.is_empty(), "{:?}", gui.pending);
        gui.act(Act::Nav(super::super::RECENT_NAV));
        gui.pending.clear();
        gui.act(Act::Nav(GENRES_NAV));
        assert_eq!(gui.app.library_stack.here(), &LibraryNode::Genres(GenreSource::Model));
        assert!(!asked(&gui, &LibraryNode::Genres(GenreSource::Model)), "seated from the session's copy");
    }

    #[test]
    fn one_kind_or_a_server_without_the_filter_draws_no_switch() {
        let _en = english();
        // Only generated genres here: the tags asked for came back as the
        // generated list, and there is nothing to choose between.
        let mut gui = session_gui();
        gui.act(Act::Nav(GENRES_NAV));
        let only_model = GenreList {
            genres: vec![Genre { name: "Dub".into(), track_count: Some(3) }],
            sources: Some(GenreSources { tag: 0, model: 1 }),
            shown: GenreSource::Model,
        };
        land(&mut gui, LibraryNode::Genres(GenreSource::Tag), LibraryData::Genres(only_model));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Dub (3)") && !all.contains("Generated"), "{all}");
        let y = rows.iter().position(|r| r.contains("/ filter")).unwrap_or_else(|| panic!("{all}"));
        assert_eq!(col(&rows[y], "/ filter"), col(&rows[y - 1], "Genres"), "the filter leads: {all}");
        gui.act(Act::PaneRow(List::Library, 1, RowVerb::Open));
        assert!(asked(&gui, &LibraryNode::Genre("Dub".into(), GenreSource::Model)), "the songs follow the list's kind");

        // A server that predates the filter: every genre, every song.
        let mut gui = session_gui();
        gui.act(Act::Nav(GENRES_NAV));
        let old = GenreList { genres: vec![Genre { name: "Dub".into(), track_count: Some(3) }], sources: None, shown: GenreSource::Both };
        land(&mut gui, LibraryNode::Genres(GenreSource::Tag), LibraryData::Genres(old));
        let all = draw(&mut gui).join("\n");
        assert!(!all.contains("Generated"), "{all}");
        gui.act(Act::PaneRow(List::Library, 1, RowVerb::Open));
        assert!(asked(&gui, &LibraryNode::Genre("Dub".into(), GenreSource::Both)));
    }

    #[test]
    fn the_footer_names_the_keys_of_the_view_that_is_up() {
        let _en = english();
        let mut gui = artists_gui(&["Bassnectar", "Portishead"]);
        // The footer is a setting on this surface, off by default.
        gui.config.gui.key_hints = true;
        // Stowed, the line says how to pick the cursor up; up, the row
        // keys (the kit's list-cursor law).
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("↓ pick a row · h back"), "a list stowed names the pick-up: {all}");
        assert!(!all.contains("a queue"), "and no row verb: {all}");
        super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("h back · a queue"), "a list has the Files keys: {all}");
        gui.act(Act::PaneRow(List::Library, 2, RowVerb::Open));
        land(&mut gui, LibraryNode::Artist("Portishead".into()), LibraryData::Albums(vec![album(Some("Dummy"), "Portishead", Some(1994))]));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("← → page"), "the wall has the wall's keys: {all}");
    }

    #[test]
    fn recent_asks_the_limit_lists_the_tracks_and_wears_no_strip() {
        let _en = english();
        let mut gui = session_gui();
        gui.act(Act::Nav(RECENT_NAV));
        assert!(asked(&gui, &LibraryNode::Recent(MoodFilter::NONE)));
        gui.pending.clear();
        let tracks: Vec<Track> = (0..30).map(|i| track(&format!("r/{}{i:02}.mp3", (b'A' + (i % 26) as u8) as char), 100.0)).collect();
        land(&mut gui, LibraryNode::Recent(MoodFilter::NONE), LibraryData::Tracks(tracks));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("30 items") && all.contains("▸ play"), "{all}");
        assert!(!rows.iter().any(|r| r.contains("# A B C")), "newest first is not an alphabet: {all}");
        assert_eq!(hit_text(&gui, &rows, "A00.mp3"), Some(Act::PaneRow(List::Library, 1, RowVerb::Open)));
    }

    #[test]
    fn the_play_lists_ask_the_limit_list_tracks_like_recent_and_hide_for_a_peer() {
        let _en = english();
        let mut gui = session_gui();
        gui.act(Act::Nav(LAST_PLAYED_NAV));
        assert!(asked(&gui, &LibraryNode::RecentlyPlayed), "asks on every visit");
        gui.pending.clear();
        let tracks: Vec<Track> = (0..3).map(|i| track(&format!("p/{i}.mp3"), 100.0)).collect();
        land(&mut gui, LibraryNode::RecentlyPlayed, LibraryData::Tracks(tracks.clone()));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("▸ Last played") && all.contains("3 items") && all.contains("▸ play"), "{all}");
        assert!(!rows.iter().any(|r| r.contains("# A B C")), "most recent first is not an alphabet: {all}");
        gui.act(Act::Nav(MOST_PLAYED_NAV));
        assert!(asked(&gui, &LibraryNode::MostPlayed));
        gui.pending.clear();
        land(&mut gui, LibraryNode::MostPlayed, LibraryData::Tracks(tracks));
        let rows = draw(&mut gui);
        assert!(rows.join("\n").contains("▸ Most played"), "{}", rows.join("\n"));
        assert_eq!(hit_text(&gui, &rows, "0.mp3"), Some(Act::PaneRow(List::Library, 1, RowVerb::Open)), "the first track, after the menu row");
        // A peer session's guest has no play counts on the peer: the rows
        // leave the nav column, and their nav index goes nowhere.
        gui.act(Act::Nav(ARTISTS_NAV));
        gui.app.session.peer = Some(("http://host:3000".into(), 3));
        let nav: Vec<String> = draw(&mut gui).iter().map(|r| r.chars().take(15).collect()).collect();
        let column = nav.join("\n");
        assert!(!column.contains("Last played") && !column.contains("Most played"), "{column}");
        gui.act(Act::Nav(LAST_PLAYED_NAV));
        assert_eq!(gui.active, ARTISTS_NAV, "a hidden room's row goes nowhere");
    }

    // ── Rated (clause 37) ─────────────────────────────────────────────────

    #[test]
    fn rated_sits_after_recent_and_lists_the_rated_songs() {
        let _en = english();
        let mut gui = session_gui();
        let rows = draw_sized(&mut gui, 100, 30);
        let nav = |y: usize| rows[y].chars().take(15).collect::<String>();
        assert!(nav(9).contains("Recent") && nav(10).contains("Rated"), "{}", rows.join("\n"));
        assert!(nav(11).contains("Playlists"), "{}", rows.join("\n"));
        assert_eq!(hit_text(&gui, &rows, "Rated"), Some(Act::Nav(RATED_NAV)));

        gui.act(Act::Nav(RATED_NAV));
        assert!(asked(&gui, &LibraryNode::Rated(MoodFilter::NONE)), "no scores: the whole list");
        gui.pending.clear();
        let tracks: Vec<Track> = (0..3).map(|i| track(&format!("r/{i}.mp3"), 100.0)).collect();
        land(&mut gui, LibraryNode::Rated(MoodFilter::NONE), LibraryData::Tracks(tracks));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("▸ Rated") && all.contains("3 items") && all.contains("▸ play"), "{all}");
        assert!(!rows.iter().any(|r| r.contains("# A B C")), "highest first is not an alphabet: {all}");
        assert_eq!(hit_text(&gui, &rows, "0.mp3"), Some(Act::PaneRow(List::Library, 1, RowVerb::Open)));

        // Ratings change: every visit asks.
        gui.act(Act::Nav(RECENT_NAV));
        gui.pending.clear();
        gui.act(Act::Nav(RATED_NAV));
        assert!(asked(&gui, &LibraryNode::Rated(MoodFilter::NONE)), "asks again");
    }

    #[test]
    fn rated_hides_for_a_peer_and_its_row_goes_nowhere() {
        let _en = english();
        let mut gui = session_gui();
        gui.act(Act::Nav(RATED_NAV));
        assert_eq!(gui.active, RATED_NAV);
        // A peer's session: db/rated is off the federation allowlist.
        gui.app.session.peer = Some(("http://host:3000".into(), 3));
        gui.reopen_room();
        assert_eq!(gui.active, FILES_NAV, "the room yields to Files");
        let column: Vec<String> = draw(&mut gui).iter().map(|r| r.chars().take(15).collect()).collect();
        let column = column.join("\n");
        assert!(!column.contains("Rated") && column.contains("Recent"), "{column}");
        gui.pending.clear();
        gui.act(Act::Nav(RATED_NAV));
        assert_eq!(gui.active, FILES_NAV, "a hidden room's row goes nowhere");
        assert!(gui.pending.is_empty(), "and asks nothing: {:?}", gui.pending);
    }

    #[test]
    fn recent_and_rated_ask_under_their_filter_only_where_offered() {
        let relaxed = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        let strict = relaxed.with_level(MoodLevel::Strict);
        let mut gui = session_gui();
        gui.moods.recent.filter = relaxed;
        gui.moods.recent.sort = MoodSort::Match;
        gui.moods.rated.filter = strict;

        // No scores to filter by: the whole lists, the choices kept.
        gui.act(Act::Nav(RECENT_NAV));
        assert!(asked(&gui, &LibraryNode::Recent(MoodFilter::NONE)));
        assert!(!gui.app.songs_by_match, "an unfiltered list has nothing to rank by");
        gui.act(Act::Nav(RATED_NAV));
        assert!(asked(&gui, &LibraryNode::Rated(MoodFilter::NONE)));
        assert_eq!((gui.moods.recent.filter, gui.moods.rated.filter), (relaxed, strict));

        // The ping says `moodFilters`: each under its own, in its order.
        gui.app.capabilities.mood_filters = true;
        gui.pending.clear();
        gui.act(Act::Nav(RECENT_NAV));
        assert!(asked(&gui, &LibraryNode::Recent(relaxed)));
        assert!(gui.app.songs_by_match, "Recent lands best first");
        gui.act(Act::Nav(RATED_NAV));
        assert!(asked(&gui, &LibraryNode::Rated(strict)), "the level rides with the mood");
        assert!(!gui.app.songs_by_match, "Rated keeps its own order");

        // A peer's session, flag or not: never the stored filter.
        gui.app.session.peer = Some(("http://host:3000".into(), 3));
        gui.pending.clear();
        gui.act(Act::Nav(RECENT_NAV));
        assert!(asked(&gui, &LibraryNode::Recent(MoodFilter::NONE)), "{:?}", gui.pending);
        assert!(!gui.app.songs_by_match);
        assert_eq!(gui.moods.recent.filter, relaxed, "kept for the user's own server");
    }

    #[test]
    fn a_choice_asks_once_where_the_wire_moved_reorders_in_hand_or_does_nothing() {
        use super::super::moods::{Applied, RoomMoods, apply};
        let relaxed = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        let lists = |gui: &Gui| gui.pending.iter().filter(|e| matches!(e, Effect::Api(ApiCmd::Library { .. }))).count();
        let order = |gui: &Gui| -> Vec<String> {
            gui.app.library.entries.iter().skip(1).map(|e| e.label().trim_end_matches(".mp3").to_string()).collect()
        };
        let mut gui = session_gui();
        gui.app.capabilities.mood_filters = true;
        gui.act(Act::Nav(RATED_NAV));
        let tracks: Vec<Track> =
            ["Swing", "Host", "Civil Attack"].iter().map(|t| track(&format!("demo/{t}.mp3"), 1.0)).collect();
        land(&mut gui, LibraryNode::Rated(MoodFilter::NONE), LibraryData::Tracks(tracks.clone()));
        gui.pending.clear();

        // A mood under Match: one ask, the rows standing until it lands in
        // the Sort's order.
        assert_eq!(apply(&mut gui, RATED_NAV, RoomMoods { filter: relaxed, sort: MoodSort::Match }), Applied::Asked);
        assert_eq!(lists(&gui), 1);
        assert!(asked(&gui, &LibraryNode::Rated(relaxed)));
        assert_eq!(order(&gui), ["Swing", "Host", "Civil Attack"], "the old rows stand");
        let matches = [Some(0.555), Some(0.865), Some(0.524)];
        let songs: Vec<MoodSong> =
            tracks.iter().zip(matches).map(|(track, mood_match)| MoodSong { track: track.clone(), mood_match }).collect();
        land(&mut gui, LibraryNode::Rated(relaxed), LibraryData::Songs(songs));
        assert_eq!(order(&gui), ["Host", "Swing", "Civil Attack"], "best first");

        // The Sort alone: in hand, from the top, no request.
        gui.pending.clear();
        gui.library.view.scroll = 2;
        assert_eq!(apply(&mut gui, RATED_NAV, RoomMoods { filter: relaxed, sort: MoodSort::Own }), Applied::Reordered);
        assert!(gui.pending.is_empty(), "{:?}", gui.pending);
        assert_eq!(order(&gui), ["Swing", "Host", "Civil Attack"]);
        assert_eq!((gui.library.view.scroll, gui.library.view.held), (0, false));
        // The same choice again: nothing.
        assert_eq!(apply(&mut gui, RATED_NAV, RoomMoods { filter: relaxed, sort: MoodSort::Own }), Applied::Nothing);
        assert!(gui.pending.is_empty());

        // Every mood let go: one ask for the whole list. Then a level with
        // no mood, or Match with nothing to rank by: nothing on screen,
        // nothing asked — but kept for the first mood.
        assert_eq!(apply(&mut gui, RATED_NAV, RoomMoods { filter: MoodFilter::NONE, sort: MoodSort::Own }), Applied::Asked);
        assert!(asked(&gui, &LibraryNode::Rated(MoodFilter::NONE)));
        gui.pending.clear();
        let lone = MoodFilter::NONE.with_level(MoodLevel::Strict);
        assert_eq!(apply(&mut gui, RATED_NAV, RoomMoods { filter: lone, sort: MoodSort::Match }), Applied::Nothing);
        assert!(gui.pending.is_empty(), "{:?}", gui.pending);
        assert_eq!(gui.app.library_node(), &LibraryNode::Rated(MoodFilter::NONE));
        let strict = relaxed.with_level(MoodLevel::Strict);
        assert_eq!(apply(&mut gui, RATED_NAV, RoomMoods { filter: strict, sort: MoodSort::Match }), Applied::Asked);
        assert!(asked(&gui, &LibraryNode::Rated(strict)), "the level rides with the first mood");
        assert!(gui.app.songs_by_match);
    }

    #[test]
    fn an_artist_opens_a_wall_of_its_albums_and_a_card_opens_its_tracks() {
        let _en = english();
        let mut gui = artists_gui(&["Bassnectar", "Portishead"]);
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Portishead"), Some(Act::PaneRow(List::Library, 2, RowVerb::Open)));
        gui.act(Act::PaneRow(List::Library, 2, RowVerb::Open));
        assert!(asked(&gui, &LibraryNode::Artist("Portishead".into())));
        gui.pending.clear();
        land(
            &mut gui,
            LibraryNode::Artist("Portishead".into()),
            LibraryData::Albums(vec![
                album(Some("Dummy"), "Portishead", Some(1994)),
                album(Some("Third"), "Portishead", Some(2008)),
                album(None, "Portishead", None),
            ]),
        );
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("◂ Artists") && all.contains("▸ Portishead"), "{all}");
        assert!(all.contains("3 albums"), "{all}");
        assert!(all.contains("Dummy") && all.contains("Third") && all.contains("Singles"), "the cards, the bucket named: {all}");
        assert_eq!(hit_text(&gui, &rows, "◂ Artists"), Some(Act::LibBack));
        assert_eq!(hit_text(&gui, &rows, "Dummy"), Some(Act::AlbCell(0)));
        gui.act(Act::AlbCell(0));
        assert!(asked(&gui, &LibraryNode::Album { name: "Dummy".into(), artist: Some("Portishead".into()) }));
        land(
            &mut gui,
            LibraryNode::Album { name: "Dummy".into(), artist: Some("Portishead".into()) },
            LibraryData::Tracks(vec![track("p/mysterons.mp3", 306.0)]),
        );
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("◂ Portishead") && all.contains("Dummy — Portishead"), "Back is the artist: {all}");
        assert!(all.contains("mysterons.mp3"));
        // Back to the wall, then to the artists — the wall stands on its state.
        key(&mut gui, KeyCode::Esc);
        assert!(matches!(gui.app.library_stack.here(), LibraryNode::Artist(_)));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Third"), "the wall again: {all}");
        key(&mut gui, KeyCode::Esc);
        assert!(matches!(gui.app.library_stack.here(), LibraryNode::Artists(_)));
        // Opening the bucket asks for the artist's name-less album.
        gui.act(Act::PaneRow(List::Library, 2, RowVerb::Open));
        assert!(asked(&gui, &LibraryNode::Artist("Portishead".into())), "the artist opens again: {:?}", gui.pending);
        land(&mut gui, LibraryNode::Artist("Portishead".into()), LibraryData::Albums(vec![album(None, "Portishead", None)]));
        draw(&mut gui);
        gui.pending.clear();
        gui.act(Act::AlbCell(0));
        assert!(
            asked(&gui, &LibraryNode::Album { name: String::new(), artist: Some("Portishead".into()) }),
            "the bucket asks with no album name: {:?} / here {:?}",
            gui.pending,
            gui.app.library_stack.here()
        );
    }

    #[test]
    fn the_rows_past_a_hidden_way_out_keep_their_pane_indices() {
        let _en = english();
        // A root's `..` is sliced off the front rather than filtered out of
        // a copy (performance audit #109): every drawn row still answers
        // with its own pane index, filtered or not.
        let mut gui = artists_gui(&["Air", "Bassnectar", "Cocteau Twins"]);
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Air"), Some(Act::PaneRow(List::Library, 1, RowVerb::Open)));
        assert_eq!(hit_text(&gui, &rows, "Cocteau Twins"), Some(Act::PaneRow(List::Library, 3, RowVerb::Open)));
        // A cursor resting on the hidden row is off the drawn ones.
        gui.app.library.state.select(Some(0));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Air") && all.contains("3 items"), "{all}");

        key(&mut gui, KeyCode::Char('f'));
        for c in "twins".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        let rows = draw(&mut gui);
        assert!(rows.join("\n").contains("1 of 3"), "{}", rows.join("\n"));
        assert_eq!(
            hit_text(&gui, &rows, "Cocteau Twins"),
            Some(Act::PaneRow(List::Library, 1, RowVerb::Open)),
            "the one match, behind the kept `..`"
        );

        // Below a root the way out is a row like any other, first.
        key(&mut gui, KeyCode::Esc);
        gui.act(Act::Nav(GENRES_NAV));
        gui.pending.clear();
        let one = GenreList { genres: vec![Genre { name: "Ambient".into(), track_count: Some(1) }], ..GenreList::default() };
        land(&mut gui, LibraryNode::Genres(GenreSource::Tag), LibraryData::Genres(one));
        gui.act(Act::PaneRow(List::Library, 1, RowVerb::Open));
        land(&mut gui, LibraryNode::Genre("Ambient".into(), GenreSource::Both), LibraryData::Tracks(vec![track("t/a.mp3", 61.0)]));
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, ".."), Some(Act::PaneRow(List::Library, 0, RowVerb::Open)));
        assert_eq!(hit_text(&gui, &rows, "a.mp3"), Some(Act::PaneRow(List::Library, 1, RowVerb::Open)));
        assert!(rows.join("\n").contains("1 item ") && !rows.join("\n").contains("1 items"), "{}", rows.join("\n"));
    }

    /// Not a check, a measurement (performance audit #109): a frame of the
    /// Artists room at 20,000 names, bare and filtered —
    /// `cargo test --release twenty_thousand_artists -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check; run --release with --nocapture"]
    fn render_twenty_thousand_artists_and_time_it() {
        let names: Vec<String> =
            (0..20_000).map(|i| format!("{}rtist {i:05}", (b'A' + (i % 26) as u8) as char)).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut gui = artists_gui(&refs);
        let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
        for filter in ["", "rtist 1"] {
            gui.app.library.apply_filter(filter.into());
            let frames = 2_000;
            let started = std::time::Instant::now();
            for _ in 0..frames {
                terminal.draw(|frame| render(frame, &mut gui)).unwrap();
            }
            let per_frame = started.elapsed() / frames;
            println!(">>> {per_frame:?} per frame at 20,000 artists, filter {filter:?}");
        }
    }

    #[test]
    fn the_strip_shows_at_25_rows_dims_absent_letters_jumps_and_follows_the_filter() {
        let _en = english();
        // Twenty-four names: no strip. A twenty-fifth: the strip.
        let letters: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect();
        let names: Vec<String> = (0..24).map(|i| format!("{}rtist {i:02}", letters[i])).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut gui = artists_gui(&refs);
        let all = draw(&mut gui).join("\n");
        assert!(!all.contains("# A B C"), "24 rows: no strip\n{all}");

        let mut names: Vec<String> = (0..24).map(|i| format!("{}rtist {i:02}", letters[i])).collect();
        names.push("9 Lives".into());
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut gui = artists_gui(&refs);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        let strip_y = rows.iter().position(|r| r.contains("# A B C")).expect("25 rows: the strip");
        // Y and Z have no artist: their cells are there but jump elsewhere.
        // (A cell column, not a byte offset: the nav rule is a wide glyph.)
        let line = &rows[strip_y];
        let sx = line.char_indices().position(|(i, _)| line[i..].starts_with("# A")).unwrap() as u16;
        let cell = |letter: usize| Position { x: sx + 2 * letter as u16, y: strip_y as u16 };
        assert_eq!(gui.ui.hit(cell(0)), Some(Act::LibJump(24)), "# is 9 Lives, the last row in the server's order");
        assert_eq!(gui.ui.hit(cell(3)), Some(Act::LibJump(2)), "C jumps to Crtist");
        assert_eq!(gui.ui.hit(cell(26)), Some(Act::LibJump(23)), "Z snaps to the nearest present letter, X");
        gui.act(Act::LibJump(20));
        assert_eq!(gui.library.view.scroll, 20, "the row comes to the top");
        assert_eq!(gui.app.library.state.selected(), Some(21), "and takes the cursor");
        // A pane short enough to scroll: the list's window clamps to its end,
        // and the jumped-to row is inside it while the first row is gone.
        let all2 = draw_sized(&mut gui, 100, 30).join("\n");
        assert!(all2.contains("Urtist 20") && !all2.contains("Artist 00"), "scrolled: {all2}");

        // The filter narrows the letters with the list.
        key(&mut gui, KeyCode::Char('f'));
        for c in "rtist 0".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        let rows = draw(&mut gui);
        let all3 = rows.join("\n");
        assert!(all3.contains("10 of 25"), "{all3}");
        assert!(!rows.iter().any(|r| r.contains("# A B C")), "ten rows: the strip stands down\n{all3}");
        let _ = all;
    }

    // ── The mood Filters (clauses 30–36) ─────────────────────────────────

    use super::super::moods::{MoodAct, MoodStop, is_open};

    /// A song with its artist and title, as the boards' rows read.
    fn song(artist: &str, title: &str, seconds: f64) -> Track {
        let mut track = track(&format!("demo/{title}.mp3"), seconds);
        track.metadata.artist = Some(artist.into());
        track.metadata.title = Some(title.into());
        track
    }

    /// The room at nav row `nav` with the queue open — the boards' 47
    /// cells — on a server with scores to filter by, its list landed.
    fn mood_room(nav: usize, data: LibraryData) -> Gui {
        let mut gui = session_gui();
        gui.queue_open = true;
        gui.app.capabilities.mood_filters = true;
        gui.act(Act::Nav(nav));
        let node = gui.app.library_stack.here().clone();
        gui.pending.clear();
        land(&mut gui, node, data);
        gui
    }

    fn board(gui: &mut Gui) -> Vec<String> {
        draw_sized(gui, 100, 30)
    }

    fn draw_buffer(gui: &mut Gui) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn x_of(row: &str, needle: &str) -> u16 {
        let at = row.find(needle).unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        crate::kit::width(&row[..at]) as u16
    }

    fn lists(gui: &Gui) -> usize {
        gui.pending.iter().filter(|e| matches!(e, Effect::Api(ApiCmd::Library { .. }))).count()
    }

    fn order(gui: &Gui) -> Vec<String> {
        gui.app.library.entries.iter().skip(1).map(|e| e.label().to_string()).collect()
    }

    fn rated_songs() -> Vec<Track> {
        vec![song("ALM", "Swing", 205.0), song("Color Out", "Host", 178.0), song("ALM", "Civil Attack", 271.0)]
    }

    #[test]
    fn the_artists_opener_stands_two_cells_after_the_name_and_the_count_ends_at_x62() {
        let _en = english();
        let mut gui = mood_room(ARTISTS_NAV, LibraryData::Artists(vec!["ALM".into(), "Color Out".into()]));
        let rows = board(&mut gui);
        let line = &rows[2];
        assert_eq!((x_of(line, "Artists"), x_of(line, "mood")), (17, 26), "{line}");
        assert_eq!(x_of(line, "2 items") + 7, 63, "the count ends at x62: {line}");
        assert_eq!(gui.ui.hit(Position::new(26, 2)), Some(Act::Mood(MoodAct::Open { key: false })));
        // The filter's field takes line two; the opener stays on line one.
        gui.act(Act::BarFilter);
        let rows = board(&mut gui);
        assert_eq!(x_of(&rows[2], "mood"), 26);
        // While the field takes the letters, `o` is one of them.
        key(&mut gui, KeyCode::Char('o'));
        assert!(!is_open(&gui) && gui.app.library.filter == "o", "{:?}", gui.app.library.filter);
        key(&mut gui, KeyCode::Esc);
        // Recent and Rated: two after their names, x25 and x24; Genres and a
        // peer's lists have none, and every list's count ends at x62.
        for (nav, x) in [(RECENT_NAV, 25), (RATED_NAV, 24)] {
            let mut gui = mood_room(nav, LibraryData::Tracks(rated_songs()));
            let rows = board(&mut gui);
            assert_eq!(x_of(&rows[2], "mood"), x, "{}", rows[2]);
            assert_eq!(x_of(&rows[2], "3 items") + 7, 63, "{}", rows[2]);
        }
        let mut gui = mood_room(GENRES_NAV, genre_list(GenreSource::Tag, &[("Techno", 2)]));
        let rows = board(&mut gui);
        assert!(!rows[2].contains("mood"), "{}", rows[2]);
        assert_eq!(x_of(&rows[2], "1 item") + 6, 63, "one item reads its own form: {}", rows[2]);
    }

    #[test]
    fn recent_opens_the_eleven_row_modal_with_newest_pressed_and_match_resting() {
        let _en = english();
        let mut gui = mood_room(RECENT_NAV, LibraryData::Tracks(rated_songs()));
        board(&mut gui);
        key(&mut gui, KeyCode::Char('o'));
        let rows = board(&mut gui);
        let all = rows.join("\n");
        // r9–r19: the SORT row under the level.
        assert_eq!((x_of(&rows[9], "╭─ Filters"), x_of(&rows[19], "╰")), (17, 17), "{all}");
        assert_eq!(
            (x_of(&rows[16], "SORT"), x_of(&rows[16], "(•) Newest"), x_of(&rows[16], "( ) Match")),
            (19, 45, 67),
            "{all}"
        );
        assert!(!all.contains("clear"), "{all}");
        // Match rests: DIM, and its cells take no click.
        let buffer = draw_buffer(&mut gui);
        assert_eq!(buffer[(71, 16)].fg, crate::kit::theme::th().dim);
        assert_eq!(gui.ui.hit(Position::new(72, 16)), Some(Act::Guard));
        assert_eq!(gui.ui.hit(Position::new(50, 16)), Some(Act::Mood(MoodAct::Press(MoodStop::Sort(MoodSort::Own)))));
        // A mood wakes it, and `clear` comes.
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Pick(MoodScale::Energy, "relaxed"))));
        let rows = board(&mut gui);
        assert!(rows[11].contains("(•) Relaxed") && rows[18].contains("clear"), "{}", rows.join("\n"));
        assert_eq!(gui.ui.hit(Position::new(72, 16)), Some(Act::Mood(MoodAct::Press(MoodStop::Sort(MoodSort::Match)))));
    }

    #[test]
    fn rated_under_match_lands_best_first_and_a_sort_alone_reorders_without_asking() {
        let _en = english();
        let mut gui = mood_room(RATED_NAV, LibraryData::Tracks(rated_songs()));
        board(&mut gui);
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Pick(MoodScale::Energy, "relaxed"))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Sort(MoodSort::Match))));
        let rows = board(&mut gui);
        assert!(rows[16].contains("( ) Rating") && rows[16].contains("(•) Match"), "{}", rows.join("\n"));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Done)));
        let relaxed = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        assert_eq!(lists(&gui), 1);
        assert!(asked(&gui, &LibraryNode::Rated(relaxed)));
        let songs: Vec<MoodSong> = rated_songs()
            .into_iter()
            .zip([Some(0.555), Some(0.865), Some(0.524)])
            .map(|(track, mood_match)| MoodSong { track, mood_match })
            .collect();
        land(&mut gui, LibraryNode::Rated(relaxed), LibraryData::Songs(songs));
        assert_eq!(order(&gui), ["Color Out - Host", "ALM - Swing", "ALM - Civil Attack"]);
        let rows = board(&mut gui);
        let all = rows.join("\n");
        assert_eq!(x_of(&rows[4], "Relaxed [X]"), 17, "{all}");
        assert!(rows[5].contains("Color Out - Host"), "the rows from r5: {all}");
        // The Sort alone: re-ordered in hand, nothing asked.
        gui.pending.clear();
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Sort(MoodSort::Own))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Done)));
        assert_eq!(lists(&gui), 0, "{:?}", gui.pending);
        assert_eq!(order(&gui), ["ALM - Swing", "Color Out - Host", "ALM - Civil Attack"]);
    }

    #[test]
    fn a_mood_ask_leaves_the_rows_on_screen_and_a_failed_one_puts_the_choice_back() {
        let _en = english();
        let mut gui = mood_room(RATED_NAV, LibraryData::Tracks(rated_songs()));
        board(&mut gui);
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Pick(MoodScale::Energy, "relaxed"))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Level(MoodLevel::Strict))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Sort(MoodSort::Match))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Done)));
        let strict = MoodFilter::NONE.press(MoodScale::Energy, "relaxed").with_level(MoodLevel::Strict);
        assert!(asked(&gui, &LibraryNode::Rated(strict)) && gui.app.library.loading);
        // While it is out the strip names it and the rows stand, drawn: the
        // verbs on line two act on the rows on screen.
        let rows = board(&mut gui);
        let all = rows.join("\n");
        assert_eq!(x_of(&rows[4], "Relaxed · Strict [X]"), 17, "{all}");
        assert!(rows[5].contains("ALM - Swing") && !all.contains(&*rust_i18n::t!("busy.listing")), "{all}");
        assert!(rows[3].contains("play"), "{all}");
        // The ask fails (a server before #1099 refuses the threshold): the
        // list and the choice go back to what the rows carry, and the strip
        // with them.
        let effects = gui.app.apply_event(Event::Error("\"threshold\" is not allowed".into()));
        gui.pend(effects);
        assert_eq!(gui.app.library_stack.here(), &LibraryNode::Rated(MoodFilter::NONE));
        assert_eq!((gui.moods.rated.filter, gui.moods.rated.sort), (MoodFilter::NONE, MoodSort::Own));
        assert!(!gui.app.songs_by_match);
        let rows = board(&mut gui);
        assert!(!rows[4].contains("[X]") && rows[5].contains("ALM - Swing"), "{}", rows.join("\n"));
        // Done with the same choice asks again, and its answer keeps it.
        gui.pending.clear();
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Pick(MoodScale::Energy, "relaxed"))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Level(MoodLevel::Strict))));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Done)));
        assert_eq!(lists(&gui), 1);
        assert!(asked(&gui, &LibraryNode::Rated(strict)));
        let songs = rated_songs().into_iter().map(|track| MoodSong { track, mood_match: Some(0.8) }).collect();
        land(&mut gui, LibraryNode::Rated(strict), LibraryData::Songs(songs));
        assert_eq!(gui.moods.rated.filter, strict);
        assert!(board(&mut gui)[4].contains("Relaxed · Strict [X]"));
    }

    #[test]
    fn a_drill_from_the_rows_a_mood_ask_left_standing_comes_back_to_them_under_their_filter() {
        let _en = english();
        let mut gui = mood_room(ARTISTS_NAV, LibraryData::Artists(vec!["ALM".into(), "Color Out".into()]));
        let relaxed = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        let set = super::super::moods::RoomMoods { filter: relaxed, sort: MoodSort::Own };
        let _ = super::super::moods::apply(&mut gui, ARTISTS_NAV, set);
        let rows = board(&mut gui);
        assert!(rows[4].contains("Relaxed [X]"), "{}", rows.join("\n"));
        assert_eq!(hit_text(&gui, &rows, "Color Out"), Some(Act::PaneRow(List::Library, 2, RowVerb::Open)));
        gui.act(Act::PaneRow(List::Library, 2, RowVerb::Open));
        assert!(asked(&gui, &LibraryNode::Artist("Color Out".into())));
        assert_eq!(gui.app.library_stack.parent(), Some(&LibraryNode::Artists(MoodFilter::NONE)));
        assert!(gui.moods.artists.filter.is_empty(), "the choice the rows carry");
        // The filtered answer, late, is dropped; Back finds the whole list.
        land(&mut gui, LibraryNode::Artists(relaxed), LibraryData::Artists(vec!["ALM".into()]));
        let host = vec![album(Some("Host"), "Color Out", None)];
        land(&mut gui, LibraryNode::Artist("Color Out".into()), LibraryData::Albums(host));
        gui.act(Act::LibBack);
        let rows = board(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("ALM") && all.contains("Color Out") && all.contains("2 items"), "{all}");
        assert!(!rows[4].contains("[X]"), "{all}");
    }

    #[test]
    fn hover_brightens_a_label_and_keeps_its_glyphs_colour() {
        let mut gui = mood_room(RATED_NAV, LibraryData::Tracks(rated_songs()));
        board(&mut gui);
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Pick(MoodScale::Energy, "relaxed"))));
        // The pointer on Relaxed (r11): the word BRIGHT, the glyph still OK.
        gui.ui.pointer = Some(Position::new(25, 11));
        let buffer = draw_buffer(&mut gui);
        let th = crate::kit::theme::th();
        assert_eq!(buffer[(20, 11)].fg, th.ok, "the glyph keeps its state");
        assert_eq!(buffer[(23, 11)].fg, th.bright, "the word lights");
        // Under the hand the word is ACCENT, and the hand wins over the pointer.
        if let Some(draft) = gui.moods.draft.as_mut() {
            draft.shown = true;
        }
        let buffer = draw_buffer(&mut gui);
        assert_eq!((buffer[(20, 11)].fg, buffer[(23, 11)].fg), (th.ok, th.accent));
    }

    #[test]
    fn a_click_presses_and_stows_the_hand_and_the_first_arrow_only_shows_it() {
        let mut gui = mood_room(RECENT_NAV, LibraryData::Tracks(rated_songs()));
        board(&mut gui);
        gui.act(Act::Mood(MoodAct::Open { key: false }));
        let tag = MoodStop::Tag("party");
        gui.act(Act::Mood(MoodAct::Press(tag)));
        let draft = gui.moods.draft.unwrap();
        assert_eq!((draft.hand, draft.shown, draft.moods.filter.has_tag("party")), (tag, false, true));
        // The first arrow shows the hand where the click left it; the next
        // walks.
        key(&mut gui, KeyCode::Right);
        let draft = gui.moods.draft.unwrap();
        assert_eq!((draft.hand, draft.shown), (tag, true));
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.moods.draft.unwrap().hand, MoodStop::Tag("danceable"));
        // Space presses the held stop; a stowed hand is only shown by it.
        key(&mut gui, KeyCode::Char(' '));
        assert!(gui.moods.draft.unwrap().moods.filter.has_tag("danceable"));
        gui.act(Act::Mood(MoodAct::Press(MoodStop::Level(MoodLevel::Strict))));
        key(&mut gui, KeyCode::Char(' '));
        let draft = gui.moods.draft.unwrap();
        assert_eq!((draft.shown, draft.moods.filter.level()), (true, MoodLevel::Strict));
        // Enter is Done from anywhere.
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert!(!is_open(&gui));
        let wire = MoodFilter::NONE.toggle_tag("party").toggle_tag("danceable").with_level(MoodLevel::Strict);
        assert!(asked(&gui, &LibraryNode::Recent(wire)), "{:?}", gui.pending);
    }

    #[test]
    fn clear_lets_the_moods_go_puts_the_level_on_standard_keeps_the_sort_and_moves_the_hand_to_done() {
        let _en = english();
        let mut gui = mood_room(RATED_NAV, LibraryData::Tracks(rated_songs()));
        let set = MoodFilter::NONE.press(MoodScale::Energy, "relaxed").with_level(MoodLevel::Loose);
        let _ = super::super::moods::apply(&mut gui, RATED_NAV, super::super::moods::RoomMoods { filter: set, sort: MoodSort::Match });
        let songs = rated_songs().into_iter().map(|track| MoodSong { track, mood_match: Some(0.5) }).collect();
        land(&mut gui, LibraryNode::Rated(set), LibraryData::Songs(songs));
        let rows = board(&mut gui);
        assert_eq!(x_of(&rows[4], "Relaxed · Loose [X]"), 17, "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Char('o'));
        // Shift-Tab from the first stop wraps to Done, then ← is clear.
        key(&mut gui, KeyCode::BackTab);
        assert_eq!(gui.moods.draft.unwrap().hand, MoodStop::Done);
        key(&mut gui, KeyCode::Left);
        assert_eq!(gui.moods.draft.unwrap().hand, MoodStop::Clear);
        let rows = board(&mut gui);
        assert!(rows[18].contains("clear   [ Done ]"), "{}", rows.join("\n"));
        // Enter on a shown clear presses it: the modal stays, the hand on Done.
        key(&mut gui, KeyCode::Enter);
        let draft = gui.moods.draft.unwrap();
        assert!(is_open(&gui));
        assert_eq!((draft.moods.filter, draft.moods.sort, draft.hand), (MoodFilter::NONE, MoodSort::Match, MoodStop::Done));
        let rows = board(&mut gui);
        assert!(!rows[18].contains("clear") && rows[16].contains("(•) Rating"), "{}", rows.join("\n"));
        // Done: the whole list asked, the Sort kept for the next mood.
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert!(asked(&gui, &LibraryNode::Rated(MoodFilter::NONE)));
        assert_eq!(gui.moods.rated.sort, MoodSort::Match);
        assert!(!board(&mut gui)[4].contains("[X]"));
    }

    #[test]
    fn a_peer_or_a_server_without_scores_gets_no_opener_no_strip_and_no_key() {
        let mut gui = mood_room(ARTISTS_NAV, LibraryData::Artists(vec!["ALM".into()]));
        gui.moods.artists.filter = MoodFilter::NONE.press(MoodScale::Valence, "happy");
        gui.app.capabilities.mood_filters = false;
        let rows = board(&mut gui);
        assert!(!rows[2].contains("mood") && !rows[4].contains("[X]"), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Char('o'));
        assert!(!is_open(&gui));
        assert_eq!(gui.moods.artists.filter.value_of(MoodScale::Valence), Some("happy"), "the choice kept");
    }

    /// Every scale and every tag set, and a level: the longest strip.
    fn everything() -> MoodFilter {
        MoodFilter::NONE
            .press(MoodScale::Valence, "happy")
            .press(MoodScale::Energy, "aggressive")
            .press(MoodScale::Voice, "instrumental")
            .toggle_tag("acoustic")
            .toggle_tag("party")
            .toggle_tag("danceable")
            .with_level(MoodLevel::Loose)
    }

    #[test]
    fn at_80_cells_the_opener_stays_after_the_name_and_the_line_ends_at_x95() {
        let _en = english();
        // The queue folded: the room's 80 cells, x17–x96.
        let mut gui = mood_room(ARTISTS_NAV, LibraryData::Artists(vec!["ALM".into(), "Color Out".into()]));
        gui.queue_open = false;
        let rows = board(&mut gui);
        assert_eq!((x_of(&rows[2], "Artists"), x_of(&rows[2], "mood")), (17, 26), "{}", rows[2]);
        assert_eq!(x_of(&rows[2], "2 items") + 7, 96, "the count ends at x95: {}", rows[2]);

        let mut gui = mood_room(RATED_NAV, LibraryData::Tracks(rated_songs()));
        gui.queue_open = false;
        let set = super::super::moods::RoomMoods { filter: everything(), sort: MoodSort::Own };
        let _ = super::super::moods::apply(&mut gui, RATED_NAV, set);
        let songs = rated_songs().into_iter().map(|track| MoodSong { track, mood_match: Some(0.5) }).collect();
        land(&mut gui, LibraryNode::Rated(everything()), LibraryData::Songs(songs));
        let rows = board(&mut gui);
        let all = rows.join("\n");
        assert_eq!(x_of(&rows[2], "mood"), 24, "{all}");
        assert_eq!(x_of(&rows[2], "3 items") + 7, 96, "{all}");
        // The verbs end where the count does; the strip by x95, [X] kept.
        assert!(rows[3].trim_end().ends_with("shuffle") && rows[3].trim_end().chars().count() == 96, "{all}");
        assert_eq!(x_of(&rows[4], "Happy · Aggressive"), 17, "{all}");
        assert!(x_of(&rows[4], "[X]") + 2 <= 95, "{all}");

        // The eleven-row modal centred in the room, x24–x89, r9–r19, the
        // room's x17–x96 blank beside it on its rows.
        key(&mut gui, KeyCode::Char('o'));
        let rows = board(&mut gui);
        let all = rows.join("\n");
        assert_eq!((x_of(&rows[9], "╭─ Filters"), x_of(&rows[9], "╮")), (24, 89), "{all}");
        assert_eq!((x_of(&rows[19], "╰"), x_of(&rows[19], "╯")), (24, 89), "{all}");
        for (r, row) in rows.iter().enumerate().take(20).skip(9) {
            let beside = |from: usize, to: usize| row.chars().skip(from).take(to - from).collect::<String>();
            assert!(beside(17, 24).trim().is_empty() && beside(90, 97).trim().is_empty(), "r{r}: {all}");
        }
        assert_eq!(x_of(&rows[16], "SORT"), 26, "{all}");
    }

    #[test]
    fn at_the_24_row_floor_the_modal_sits_clear_of_the_bar_and_the_nav_keeps_rated() {
        let _en = english();
        // 100×24 with key hints on: the content's last row is r17.
        let mut gui = mood_room(RECENT_NAV, LibraryData::Tracks(rated_songs()));
        gui.set_key_hints(true);
        key(&mut gui, KeyCode::Char('o'));
        let rows = draw_sized(&mut gui, 100, 24);
        let all = rows.join("\n");
        assert_eq!((x_of(&rows[6], "╭─ Filters"), x_of(&rows[16], "╰")), (17, 17), "r6–r16: {all}");
        assert!(!rows[17].contains('╰') && !rows[5].contains('╭'), "{all}");
        let bar = super::super::bar::top(ratatui::layout::Rect::new(0, 0, 100, 24), true);
        assert!(16 < bar, "the bar from r{bar}: {all}");
        // The modal's own tips line while it is up.
        assert_eq!(rows[23].trim(), rust_i18n::t!("gui.tips.moods"), "{all}");
        // The nav risen a row into the blank under Search: Rated at r9,
        // Settings alone on r17.
        assert_eq!(x_of(&rows[9], "Rated"), 3, "{all}");
        assert_eq!(x_of(&rows[17], "Settings"), 3, "{all}");
    }
}
