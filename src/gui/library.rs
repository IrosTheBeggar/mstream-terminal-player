//! The Library rooms — Artists, Genres, Recent (docs/ux-contracts/
//! library-rooms.md): the shared App's Library pane under the browse top
//! bar, with the letter strip on the alphabetical lists, an artist opening
//! as a wall of its albums (the Albums room's wall, generalized), and a
//! genre or Recent listing tracks the drilled album's way.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::legacy_conhost;
use crate::kit::{STRIP_BUCKETS, STRIP_MIN_ROWS, dim, letter_bucket, letter_strip, scroll_list, table_view};
use crate::tui::app::{Action, App, Entry, Tab};
use crate::tui::worker::LibraryNode;

use super::albums::{self, WallState};
use super::{ARTISTS_NAV, Act, GENRES_NAV, Gui, RECENT_NAV, accent, bright_bold, put};

// ── State ───────────────────────────────────────────────────────────────────

/// What the three rooms keep for themselves: the list's wheel offset and
/// reveal flag (the kit's table contract, as Files), and the artist wall's
/// own state (library-rooms contract, clause 7).
#[derive(Debug, Default)]
pub(crate) struct LibraryUi {
    pub scroll: usize,
    pub reveal: bool,
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
        ARTISTS_NAV => Some(LibraryNode::Artists),
        GENRES_NAV => Some(LibraryNode::Genres),
        RECENT_NAV => Some(LibraryNode::Recent),
        _ => None,
    }
}

fn is_room(gui: &Gui) -> bool {
    root_of(gui.active).is_some()
}

/// The nav row chosen (entry point 1): the root list, fresh — the rooms
/// share one pane, and "recent" means now (clauses 4 and the flows).
pub(crate) fn open(gui: &mut Gui, root: LibraryNode) {
    gui.library.scroll = 0;
    gui.library.reveal = false;
    gui.library.wall = WallState::default();
    // A fresh root has no way back: a trail left by another room's drill
    // would be an orphan under it.
    gui.app.library.trail.clear();
    let effects = gui.app.open_library_node(root, true);
    gui.pend(effects);
}

/// Whether the drill stands on a wall or inside an album — the views the
/// Albums room's code draws and drives.
fn wall_view(gui: &Gui) -> bool {
    matches!(gui.app.library_stack.here(), LibraryNode::Artist(_) | LibraryNode::Album { .. })
}

/// Whether the view on screen is a root list, whose `..` row leads nowhere.
fn at_root(gui: &Gui) -> bool {
    matches!(gui.app.library_stack.here(), LibraryNode::Artists | LibraryNode::Genres | LibraryNode::Recent)
}

/// The rows the list draws, with their pane indices: everything but the
/// `..` row at a root (it would climb into the TUI's mode menu). Borrows
/// the App alone, so the shell's own state stays writable beside it.
fn rows_of(app: &App, skip_parent: bool) -> Vec<(usize, &Entry)> {
    app.library
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !(skip_parent && matches!(e, Entry::Parent)))
        .collect()
}

fn rows(gui: &Gui) -> Vec<(usize, &Entry)> {
    rows_of(&gui.app, at_root(gui))
}

/// A root list's cursor never rests on the hidden `..`.
fn keep_off_parent(gui: &mut Gui) {
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
    matches!(gui.app.library_stack.here(), LibraryNode::Artists | LibraryNode::Genres)
}

/// The list's letters (clause 11: the narrowed list's), and the first row of
/// each — as positions in the drawn rows.
fn letters(rows: &[(usize, &Entry)]) -> ([bool; STRIP_BUCKETS], [usize; STRIP_BUCKETS]) {
    let mut present = [false; STRIP_BUCKETS];
    let mut first_of = [0usize; STRIP_BUCKETS];
    for (pos, (_, entry)) in rows.iter().enumerate() {
        let bucket = letter_bucket(entry.label());
        if !present[bucket] {
            present[bucket] = true;
            first_of[bucket] = pos;
        }
    }
    (present, first_of)
}

// ── Drawing ─────────────────────────────────────────────────────────────────

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !gui.app.connected {
        super::servers::draw_disconnected(frame, gui, content, 0);
        return;
    }
    if wall_view(gui) {
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
        LibraryNode::Genre(genre) => {
            let (back_glyph, forward) = if legacy_conhost() { ("<", ">") } else { ("◂", "▸") };
            let back_label = format!("{back_glyph} {}", t!("gui.nav.genres"));
            let back =
                Rect { x: content.x, y: content.y, width: back_label.chars().count() as u16, height: 1 };
            let hover = gui.ui.pointer.is_some_and(|p| back.contains(p));
            put(frame, content.x, content.y, &back_label, if hover { bright_bold() } else { dim() });
            gui.ui.click(back, Act::LibBack);
            let title = format!("{forward} {genre}");
            let x = back.right() + 1;
            let shown = super::bar::clip(&title, content.width.saturating_sub(back.width + 18) as usize);
            put(frame, x, content.y, &shown, Style::default().add_modifier(Modifier::BOLD));
            x + shown.chars().count() as u16 + 2
        }
        _ => {
            let heading = super::NAV[gui.active].label();
            put(frame, content.x, content.y, &heading, Style::default().add_modifier(Modifier::BOLD));
            content.x + heading.chars().count() as u16 + 2
        }
    }
}

fn draw_list(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let count_x = draw_heading(frame, gui, content);
    let count = super::bar_count(gui, t!("gui.files.items", count = gui.app.library.counts().1).to_string());
    let x = content.right().saturating_sub(count.chars().count() as u16).max(count_x);
    put(frame, x, content.y, &count, dim());
    super::draw_bar_controls(frame, gui, content, content.y + 1);

    if gui.app.library.loading {
        put(frame, content.x, content.y + 3, &t!("busy.listing"), accent());
        return;
    }
    let root = at_root(gui);
    let alpha = alphabetical(gui);
    let rows = rows_of(&gui.app, root);
    if rows.is_empty() {
        // An empty answer, or a filter that matched nothing (the bar
        // contract's rule: the way back is one key).
        let words = if gui.app.library.filter.is_empty() { t!("gui.lib.empty") } else { t!("gui.files.empty") };
        put(frame, content.x, content.y + 3, &words, dim());
        return;
    }

    // The strip on an alphabetical list of 25 or more (clauses 10–12).
    if alpha && rows.len() >= STRIP_MIN_ROWS {
        let (present, first_of) = letters(&rows);
        let strip = Rect { x: content.x, y: content.y + 2, width: content.width, height: 1 };
        letter_strip(frame, &mut gui.ui, strip, &present, move |bucket| Act::LibJump(first_of[bucket]));
    }

    let list = Rect {
        x: content.x,
        y: content.y + 3,
        width: content.width - 2,
        height: content.height.saturating_sub(3),
    };
    let selected = gui.app.library.state.selected();
    let sel_pos = selected.and_then(|s| rows.iter().position(|(i, _)| *i == s));
    let reveal = gui.library.reveal.then_some(sel_pos).flatten();
    gui.library.reveal = false;
    let (first, visible) = table_view(rows.len(), reveal, gui.library.scroll, list.height as usize);
    gui.library.scroll = first;

    let len = rows.len();
    let shown: Vec<(usize, &Entry)> = rows.into_iter().skip(first).take(visible).collect();
    let playing = gui.app.now_playing.as_ref().map(|t| t.filepath.as_str());
    super::draw_pane_rows(
        frame,
        &mut gui.ui,
        playing,
        &shown,
        list,
        selected,
        Act::LibRow,
        Act::LibQueue,
        Act::LibNext,
        Act::LibNow,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::LibScrollBy(-1),
        Act::LibScrollBy(1),
        Act::LibScrollTo,
    );
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// A row chosen by the pointer: the App's cursor moves there first, so the
/// forwarded action lands on it.
fn select_row(gui: &mut Gui, index: usize) {
    gui.app.tab = Tab::Library;
    gui.app.library.state.select(Some(index));
    gui.library.reveal = true;
}

/// The strip's jump (clause 10): the row at `pos` (in the drawn rows) comes
/// to the top and takes the cursor.
fn jump_list(gui: &mut Gui, pos: usize) {
    let target = rows(gui).get(pos).map(|(i, _)| *i);
    if let Some(index) = target {
        gui.library.scroll = pos;
        gui.app.tab = Tab::Library;
        gui.app.library.state.select(Some(index));
    }
}

/// The rooms' side of [`Gui::act`]. True when the act was ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act {
        Act::LibRow(i) => {
            select_row(gui, *i);
            gui.forward_capturing(Action::Activate);
        }
        Act::LibQueue(i) => {
            select_row(gui, *i);
            gui.forward(Action::AddToQueue);
        }
        Act::LibNext(i) => {
            select_row(gui, *i);
            gui.forward(Action::AddNext);
        }
        Act::LibNow(i) => {
            select_row(gui, *i);
            gui.forward(Action::PlayNow);
        }
        Act::LibScrollBy(delta) => {
            gui.library.scroll =
                if *delta < 0 { gui.library.scroll.saturating_sub(1) } else { gui.library.scroll + 1 };
        }
        Act::LibScrollTo(first) => gui.library.scroll = *first,
        Act::LibJump(pos) => jump_list(gui, *pos),
        Act::LibBack => {
            gui.app.tab = Tab::Library;
            gui.forward(Action::Back);
        }
        _ => return false,
    }
    true
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
    match key.code {
        KeyCode::Down => {
            gui.library.reveal = true;
            gui.forward(Action::Down);
        }
        KeyCode::Up => {
            gui.library.reveal = true;
            gui.forward(Action::Up);
            keep_off_parent(gui);
        }
        KeyCode::PageDown => {
            gui.library.reveal = true;
            gui.forward(Action::PageDown);
        }
        KeyCode::PageUp => {
            gui.library.reveal = true;
            gui.forward(Action::PageUp);
            keep_off_parent(gui);
        }
        KeyCode::Enter => gui.forward_capturing(Action::Activate),
        // Back climbs a level; a root has none, and says nothing.
        KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Esc => {
            if !at_root(gui) {
                gui.forward(Action::Back);
            }
        }
        KeyCode::Char('a') => gui.forward(Action::AddToQueue),
        KeyCode::Char('N') => gui.forward(Action::AddNext),
        KeyCode::Char('P') => gui.forward(Action::PlayNow),
        _ => return None,
    }
    Some(false)
}

/// The footer's keys for whichever view is up: the wall's, the drilled
/// album's, or a list's — the Files room's line, since the keys are its.
pub(crate) fn tips(gui: &Gui) -> std::borrow::Cow<'static, str> {
    match gui.app.library_stack.here() {
        LibraryNode::Artist(_) => t!("gui.tips.albums"),
        LibraryNode::Album { .. } => t!("gui.tips.album_tracks"),
        _ if gui.app.filtering => t!("gui.tips.filter"),
        _ => t!("gui.tips.files"),
    }
}

/// The wheel: the wall turns pages and the track list scrolls (the Albums
/// room's rule); a list scrolls its view.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if wall_view(gui) {
        albums::wheel(gui, delta);
    } else {
        gui.act(Act::LibScrollBy(delta));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;

    use super::super::{ARTISTS_NAV, Act, GENRES_NAV, Gui, RECENT_NAV, render};
    use crate::api::types::{Album, Genre, Track};
    use crate::config::Config;
    use crate::tui::app::{App, Effect, Tab};
    use crate::tui::worker::{ApiCmd, Event, LibraryData, LibraryNode};

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
        let effects = gui.app.apply_event(Event::Library { node, dest: Tab::Library, data });
        gui.pend(effects);
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn artists_gui(names: &[&str]) -> Gui {
        let mut gui = session_gui();
        gui.act(Act::Nav(ARTISTS_NAV));
        assert!(asked(&gui, &LibraryNode::Artists), "the row asks for the list");
        gui.pending.clear();
        land(&mut gui, LibraryNode::Artists, LibraryData::Artists(names.iter().map(|n| n.to_string()).collect()));
        gui
    }

    #[test]
    fn each_room_opens_its_root_fresh_with_the_pane_and_no_play_verbs() {
        let mut gui = artists_gui(&["Bassnectar", "Portishead"]);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Bassnectar") && all.contains("Portishead"), "{all}");
        assert!(all.contains("2 items"), "{all}");
        assert!(!all.contains("▸ play"), "containers carry no verbs: {all}");
        assert!(!rows.iter().any(|r| r.contains("  ..")), "no .. at a root: {all}");
        assert_eq!(gui.app.library.state.selected(), Some(1), "the cursor rests on the first name");

        gui.act(Act::Nav(GENRES_NAV));
        assert!(asked(&gui, &LibraryNode::Genres));
        gui.act(Act::Nav(RECENT_NAV));
        assert!(asked(&gui, &LibraryNode::Recent), "Recent asks on every visit");
        gui.act(Act::Nav(ARTISTS_NAV));
        assert!(asked(&gui, &LibraryNode::Artists), "a room restarts at its root");
    }

    #[test]
    fn genres_show_their_counts_and_open_into_tracks_with_the_verbs() {
        let mut gui = session_gui();
        gui.act(Act::Nav(GENRES_NAV));
        gui.pending.clear();
        land(
            &mut gui,
            LibraryNode::Genres,
            LibraryData::Genres(vec![
                Genre { name: "Ambient".into(), track_count: Some(4) },
                Genre { name: "Techno".into(), track_count: Some(9) },
            ]),
        );
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Ambient (4)") && all.contains("Techno (9)"), "{all}");
        assert_eq!(hit_text(&gui, &rows, "Techno"), Some(Act::LibRow(2)));
        gui.act(Act::LibRow(2));
        assert!(asked(&gui, &LibraryNode::Genre("Techno".into())), "a genre opens into its tracks");
        land(&mut gui, LibraryNode::Genre("Techno".into()), LibraryData::Tracks(vec![track("t/a.mp3", 61.0), track("t/b.mp3", 125.0)]));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("◂ Genres") && all.contains("▸ Techno"), "{all}");
        assert!(all.contains("a.mp3") && all.contains("1:01") && all.contains("2:05"), "track rows: {all}");
        assert!(all.contains("▸ play"), "tracks carry the bar's verbs: {all}");
        assert_eq!(hit_text(&gui, &rows, "◂ Genres"), Some(Act::LibBack));
        key(&mut gui, KeyCode::Char('h'));
        assert!(matches!(gui.app.library_stack.here(), LibraryNode::Genres), "Back climbs to the genres");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Ambient (4)"), "the parent comes back from the trail: {all}");
    }

    #[test]
    fn the_footer_names_the_keys_of_the_view_that_is_up() {
        let mut gui = artists_gui(&["Bassnectar", "Portishead"]);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("h back · a queue"), "a list has the Files keys: {all}");
        gui.act(Act::LibRow(2));
        land(&mut gui, LibraryNode::Artist("Portishead".into()), LibraryData::Albums(vec![album(Some("Dummy"), "Portishead", Some(1994))]));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("← → page"), "the wall has the wall's keys: {all}");
    }

    #[test]
    fn recent_asks_the_limit_lists_the_tracks_and_wears_no_strip() {
        let mut gui = session_gui();
        gui.act(Act::Nav(RECENT_NAV));
        assert!(asked(&gui, &LibraryNode::Recent));
        gui.pending.clear();
        let tracks: Vec<Track> = (0..30).map(|i| track(&format!("r/{}{i:02}.mp3", (b'A' + (i % 26) as u8) as char), 100.0)).collect();
        land(&mut gui, LibraryNode::Recent, LibraryData::Tracks(tracks));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("30 items") && all.contains("▸ play"), "{all}");
        assert!(!rows.iter().any(|r| r.contains("# A B C")), "newest first is not an alphabet: {all}");
        assert_eq!(hit_text(&gui, &rows, "A00.mp3"), Some(Act::LibRow(1)));
    }

    #[test]
    fn an_artist_opens_a_wall_of_its_albums_and_a_card_opens_its_tracks() {
        let mut gui = artists_gui(&["Bassnectar", "Portishead"]);
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Portishead"), Some(Act::LibRow(2)));
        gui.act(Act::LibRow(2));
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
        assert!(matches!(gui.app.library_stack.here(), LibraryNode::Artists));
        // Opening the bucket asks for the artist's name-less album.
        gui.act(Act::LibRow(2));
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
    fn the_strip_shows_at_25_rows_dims_absent_letters_jumps_and_follows_the_filter() {
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
        assert_eq!(gui.library.scroll, 20, "the row comes to the top");
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
}
