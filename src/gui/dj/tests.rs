//! The Auto DJ room's tests: the room's rows and its floor, the gates, the
//! sheets and their draft law, First song's list, the opening question and
//! Recently played, the empty queue's openers, and quick config.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use super::super::{Act, DJ_NAV, FILES_NAV, Gui, Screen, render};
use super::{Item, QuickAct, Row, layout};
use crate::api::types::{
    Genre, GenreSource, GenreSources, MoodFilter, MoodLevel, MoodScale, SonicReport, Track, TrackMetadata,
};
use crate::config::{self, Config};
use crate::dj::{self, EmptyQueueStart, GenreMode, SonicAnchor};
use crate::kit::theme::th;
use crate::tui::app::{App, Capture, DjMark, Effect, KnownServer, Origin, Queued};
use crate::tui::worker::{ApiCmd, DjNote, DjServerInfo, Event};

const HOST: &str = "http://host:3000";
const ATTIC: &str = "http://attic:3000";

/// English words under assertion: the locale lock, shared, and English
/// pinned — the locale is process-wide, and the tests that switch it take
/// the lock whole ([`every_language`]) while they draw in another language.
fn english() -> std::sync::RwLockReadGuard<'static, ()> {
    let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
    rust_i18n::set_locale("en");
    guard
}

/// The locale lock taken whole, English pinned, for a test that switches
/// the language by hand: the tests sharing it to read English wait until
/// the test is done, so none of them draws in another language.
fn every_language() -> std::sync::RwLockWriteGuard<'static, ()> {
    let guard = crate::setup::tests::LOCALE_LOCK.write().unwrap_or_else(|e| e.into_inner());
    rust_i18n::set_locale("en");
    guard
}

/// A connected session with a server to arm the DJ for.
fn session_gui() -> Gui {
    let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
    gui.app.connected = true;
    gui.app.session.server = HOST.into();
    gui.app.session.server_id = HOST.into();
    gui
}

fn known(id: &str, name: &str) -> KnownServer {
    KnownServer {
        id: id.into(),
        name: name.into(),
        token: None,
        username: None,
        self_signed: false,
        peer: None,
        pairing: None,
        dj: Default::default(),
    }
}

fn track(path: &str) -> Track {
    Track { filepath: path.into(), metadata: Default::default() }
}

fn song(path: &str, title: &str, artist: &str) -> Track {
    Track {
        filepath: path.into(),
        metadata: TrackMetadata { title: Some(title.into()), artist: Some(artist.into()), ..Default::default() },
    }
}

fn queued(path: &str, dj: Option<DjMark>) -> Queued {
    Queued { origin: Origin { server: HOST.into(), peer: None }, dj, track: track(path) }
}

/// What a current server says of itself: everything the room can do.
fn info(version: &str) -> DjServerInfo {
    DjServerInfo {
        version: Some(version.into()),
        discovery: true,
        discovery_ready: Some(true),
        libraries: vec!["Music".into()],
        mood_filters: true,
    }
}

/// The session, with the room open and its server known.
fn room_gui() -> Gui {
    let mut gui = session_gui();
    gui.act(Act::Nav(DJ_NAV));
    assert_eq!(gui.active, DJ_NAV, "the nav row opens the room");
    probed(&mut gui, info("6.33.1"));
    gui
}

/// The room's server answers its probe.
fn probed(gui: &mut Gui, info: DjServerInfo) {
    let effects = gui.app.apply_event(Event::DjProbed { identity: HOST.into(), info: Some(info) });
    gui.pend(effects);
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

fn draw(gui: &mut Gui) -> Vec<String> {
    draw_sized(gui, 100, 30)
}

/// The colour the cell at (`x`, `y`) is drawn in, at 100×30.
fn fg_at(gui: &mut Gui, x: u16, y: u16) -> ratatui::style::Color {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| render(frame, gui)).unwrap();
    terminal.backend().buffer()[(x, y)].fg
}

/// The row of the drawn screen that holds `needle` right of the nav
/// column — whose own Genres row is not the room's.
fn row_of(rows: &[String], needle: &str) -> u16 {
    rows.iter()
        .position(|row| cells(row, 17, 100).contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} on screen:\n{}", rows.join("\n"))) as u16
}

/// The room's cells of a drawn row: the 47 from column 17.
fn room(rows: &[String], y: u16) -> String {
    cells(&rows[y as usize], 17, 64)
}

/// A second library, so Play from has something to choose.
fn two_libraries(gui: &mut Gui) {
    let mut both = info("6.33.1");
    both.libraries = vec!["Music".into(), "Pods".into()];
    probed(gui, both);
}

/// Two saved servers, in the book and the config both.
fn two_servers(gui: &mut Gui) {
    gui.config.servers.push(config::ServerEntry { url: HOST.into(), ..Default::default() });
    gui.config.servers.push(config::ServerEntry { url: ATTIC.into(), ..Default::default() });
    gui.app.servers.push(known(HOST, "host"));
    gui.app.servers.push(known(ATTIC, "attic"));
}

/// The cells of `row` from column `from` to `to` (exclusive), by character.
fn cells(row: &str, from: usize, to: usize) -> String {
    row.chars().skip(from).take(to - from).collect()
}

/// The column `needle` starts at on the first row holding it.
fn find(rows: &[String], needle: &str) -> Option<(u16, u16)> {
    rows.iter().enumerate().find_map(|(y, row)| {
        let at = row.find(needle)?;
        Some((row[..at].chars().count() as u16, y as u16))
    })
}

/// The act under the first cell of `needle` on the drawn screen.
fn hit_text(gui: &Gui, rows: &[String], needle: &str) -> Option<Act> {
    let (x, y) = find(rows, needle)?;
    gui.ui.hit(Position { x, y })
}

/// Click the first cell of `needle`.
fn click(gui: &mut Gui, rows: &[String], needle: &str) {
    let act = hit_text(gui, rows, needle).unwrap_or_else(|| panic!("nothing to click at {needle:?}:\n{}", rows.join("\n")));
    gui.act(act);
}

fn key(gui: &mut Gui, code: KeyCode) {
    super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(gui: &mut Gui, text: &str) {
    for c in text.chars() {
        key(gui, KeyCode::Char(c));
    }
}

/// A drawn screen with its blanks out: a wide character takes two cells,
/// and the second is drawn as nothing — so a CJK word is found without
/// the gaps between its characters.
fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn opener_asked(gui: &Gui) -> bool {
    gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AutoDj(request)) if request.ask.opener))
}

#[test]
#[ignore = "prints the room for a look"]
fn dump_dj() {
    let _en = english();
    let mut gui = room_gui();
    for (w, h) in [(100, 30), (100, 24)] {
        println!("{}", draw_sized(&mut gui, w, h).join("\n"));
    }
    gui.app.replace_queue(vec![track("a")]);
    gui.act(Act::DjStartStop);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    println!("{}", draw(&mut gui).join("\n"));
    for row in [Row::Sonic, Row::Batch, Row::Beat, Row::Length, Row::SkipWords, Row::Rating, Row::Cooldown, Row::Genres] {
        gui.act(Act::DjRowKey(row));
        if row == Row::Genres {
            let effects = gui.app.apply_event(Event::DjGenres {
                source: GenreSource::Both,
                genres: ["Ambient", "Boom Bap", "Dub", "Techno"].iter().map(|g| Genre { name: g.to_string(), track_count: None }).collect(),
                sources: Some(GenreSources { tag: 3, model: 2 }),
            });
            gui.pend(effects);
        }
        println!("{}", draw(&mut gui).join("\n"));
        key(&mut gui, KeyCode::Esc);
    }
    gui.act(Act::DjRowKey(Row::FirstSong));
    println!("{}", draw(&mut gui).join("\n"));
    key(&mut gui, KeyCode::Esc);
    gui.act(Act::DjRowKey(Row::Smart));
    println!("{}", draw(&mut gui).join("\n"));
    key(&mut gui, KeyCode::Esc);
    gui.act(Act::DjQuick(QuickAct::Toggle));
    println!("{}", draw(&mut gui).join("\n"));
}

// ── The room ────────────────────────────────────────────────────────────────

#[test]
fn the_room_is_one_row_a_setting_on_its_twenty_three_rows() {
    let _en = english();
    let mut gui = room_gui();
    two_libraries(&mut gui);
    let rows = draw(&mut gui);
    // The canvas's "The room, off": the name and Preview, the button, how
    // the session runs, what must match, what the DJ prefers — the room's
    // 23 rows whole, the last on its last.
    assert!(room(&rows, 2).starts_with("Auto DJ") && room(&rows, 2).ends_with("Preview picks ▸"), "{:?}", room(&rows, 2));
    assert_eq!(room(&rows, 4), format!("╭{}╮", "─".repeat(45)), "the button, the rows' width");
    assert_eq!(room(&rows, 5).trim_matches(['│', ' ']), "Start ▸");
    let want = [
        (8, "Play from", "host:3000 ▸"),
        (9, "First song", "Ask ▾"),
        (10, "Batch", "4 songs ▸"),
        (13, "Sonic similarity", "55% ▸"),
        (14, "Smart filters", "Any ▸"),
        (15, "Genres", "Any ▸"),
        (16, "Rating", "Any ▸"),
        (17, "Length", "Any ▸"),
        (20, "Beat matching", "Off ▸"),
        (21, "Harmonic mixing", "[ ]  "),
        (23, "Skip words", "Off ▸"),
        (24, "Artist cooldown", "3 artists ▸"),
    ];
    for (y, label, tail) in want {
        let row = room(&rows, y);
        assert!(row.starts_with(label) && row.ends_with(tail), "row {y}: {row:?}");
    }
    assert_eq!(room(&rows, 12).trim_end(), "MUST MATCH");
    assert_eq!(room(&rows, 19).trim_end(), "PREFER");
    assert_eq!(room(&rows, 22).trim_end(), "  Next song in a key that mixes well");
    for blank in [3, 7, 11, 18] {
        assert_eq!(room(&rows, blank).trim(), "", "row {blank} is air");
    }
    // Every value ends two cells before the chevron's, whatever its length.
    assert_eq!(cells(&rows[9], 59, 64), "Ask ▾");
    assert_eq!(cells(&rows[24], 53, 64), "3 artists ▸");
    // One server and one library: Play from has nothing to choose, so it
    // is not drawn, and the rows under it stand a row higher.
    probed(&mut gui, info("6.33.1"));
    let rows = draw(&mut gui);
    assert!(!rows.join("\n").contains("Play from"));
    assert!(room(&rows, 8).starts_with("First song") && room(&rows, 23).starts_with("Artist cooldown"));
}

/// A wider room: the rows take up to a fifth more than the canvas's 47
/// cells, and past that the block stands centred — the title, the button,
/// the headers and the rows as one (the owner's call, 2026-11-10).
#[test]
fn the_block_grows_a_fifth_and_then_stands_centred() {
    let _en = english();
    let mut gui = room_gui();
    two_libraries(&mut gui);
    // 100 columns with the queue open: the room is the canvas's 47 cells,
    // and the block fills it — nothing has moved.
    let rows = draw(&mut gui);
    assert_eq!(find(&rows, "Auto DJ  ").map(|(x, _)| x), Some(17));
    assert_eq!(cells(&rows[4], 17, 64), format!("╭{}╮", "─".repeat(45)));
    // 106 columns: a room of 53 cells, and the block is all of it.
    let rows = draw_sized(&mut gui, 106, 30);
    assert_eq!(cells(&rows[4], 17, 70), format!("╭{}╮", "─".repeat(51)), "{}", rows.join("\n"));
    let batch = cells(&rows[row_of(&rows, "Batch") as usize], 17, 70);
    assert!(batch.starts_with("Batch") && batch.ends_with("4 songs ▸"), "{batch:?}");
    // 140 columns: a room of 87 cells. The block stops at 56 — a fifth past
    // 47 — and stands in the middle, 15 cells in and 16 from the far edge.
    let rows = draw_sized(&mut gui, 140, 30);
    let all = rows.join("\n");
    let x = 17 + 15;
    assert_eq!(cells(&rows[4], x, x + 56), format!("╭{}╮", "─".repeat(54)), "{all}");
    assert_eq!(cells(&rows[4], 17, x).trim(), "", "nothing at the room's left: {all}");
    let title = cells(&rows[2], x, x + 56);
    assert!(title.starts_with("Auto DJ") && title.ends_with("Preview picks ▸"), "{title:?}");
    for (label, tail) in [("Play from", "host:3000 ▸"), ("Sonic similarity", "55% ▸"), ("Artist cooldown", "3 artists ▸")] {
        let row = cells(&rows[row_of(&rows, label) as usize], x, x + 56);
        assert!(row.starts_with(label) && row.ends_with(tail), "{row:?}");
    }
    assert_eq!(cells(&rows[row_of(&rows, "MUST MATCH") as usize], x, x + 10), "MUST MATCH");
    // A row takes its click across its own cells, and no further.
    let (y, left) = (row_of(&rows, "Batch"), x as u16);
    assert_eq!(gui.ui.hit(Position { x: left, y }), Some(Act::DjRow(Row::Batch)));
    assert_eq!(gui.ui.hit(Position { x: left + 55, y }), Some(Act::DjRow(Row::Batch)));
    assert_eq!(gui.ui.hit(Position { x: left - 1, y }), None);
    assert_eq!(gui.ui.hit(Position { x: left + 56, y }), None);
    // First song's list hangs from the row where it stands now: its last
    // cell one past the row's.
    gui.act(Act::DjRow(Row::FirstSong));
    let rows = draw_sized(&mut gui, 140, 30);
    let (lx, ly) = find(&rows, "╭───────────────────╮").expect("the list");
    assert_eq!((lx + 21, ly), (left + 57, row_of(&rows, "First song") + 1), "{}", rows.join("\n"));
    key(&mut gui, KeyCode::Esc);
    // The queue folded at 100 columns leaves a room of 80: the same 56,
    // twelve cells in.
    gui.queue_open = false;
    let rows = draw(&mut gui);
    assert_eq!(cells(&rows[4], 29, 85), format!("╭{}╮", "─".repeat(54)), "{}", rows.join("\n"));
    // And the floor, 100×24 with the queue open, is the canvas's still.
    gui.queue_open = true;
    let rows = draw_sized(&mut gui, 100, 24);
    let top = cells(&rows[2], 17, 64);
    assert!(top.starts_with("Auto DJ") && top.ends_with(" Start ▸ "), "{top:?}");
}

#[test]
fn the_floor_keeps_every_row_at_a_hundred_by_twenty_four() {
    let _en = english();
    let mut gui = room_gui();
    two_libraries(&mut gui);
    let rows = draw_sized(&mut gui, 100, 24);
    // The canvas's "Minimum": Start folds to the slab on the title row,
    // the air around the button and Harmonic's line go, and all twelve
    // settings stand on the room's 17 rows.
    assert_eq!(room(&rows, 2), "Auto DJ              Preview picks ▸   Start ▸ ");
    assert_eq!(hit_text(&gui, &rows, " Start ▸ "), Some(Act::DjStartStop), "the slab is the button");
    let labels = [
        "Play from", "First song", "Batch", "", "MUST MATCH", "Sonic similarity", "Smart filters", "Genres", "Rating",
        "Length", "", "PREFER", "Beat matching", "Harmonic mixing", "Skip words", "Artist cooldown",
    ];
    for (i, label) in labels.iter().enumerate() {
        let row = room(&rows, 3 + i as u16);
        assert!(if label.is_empty() { row.trim().is_empty() } else { row.starts_with(label) }, "row {}: {row:?}", 3 + i);
    }
    assert!(!rows.join("\n").contains("mixes well"), "no room for the line");
    // Armed, the slab is Stop.
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::DjStartStop);
    assert!(room(&draw_sized(&mut gui, 100, 24), 2).ends_with(" Stop "));
}

#[test]
fn the_rows_the_height_affords_come_back_in_one_order() {
    let l = |rows, play_from| layout(rows, play_from);
    // With Play from the rows alone take 15.
    let bare = l(15, true);
    assert!(!bare.gap_must && !bare.gap_prefer && !bare.tall && !bare.hint, "the rows, nothing else");
    assert!(l(12, true) == bare, "under the floor nothing more goes");
    assert!(l(16, true).gap_must && !l(16, true).gap_prefer);
    let floor = l(17, true);
    assert!(floor.gap_must && floor.gap_prefer && !floor.tall && !floor.hint, "100×24");
    assert!(l(18, true).hint && !l(18, true).tall);
    assert!(l(19, true).hint && !l(19, true).tall);
    let tight = l(20, true);
    assert!(tight.tall && !tight.blank_button && !tight.blank_title && !tight.hint, "the frame, no air");
    assert!(l(21, true).tall && l(21, true).blank_button && !l(21, true).blank_title);
    assert!(l(22, true).blank_title && !l(22, true).hint);
    let whole = l(23, true);
    assert!(whole.tall && whole.blank_title && whole.blank_button && whole.hint && whole.gap_must && whole.gap_prefer);
    assert!(l(40, true) == whole, "a taller room adds nothing: it never scrolls, and never stretches");
    // Without Play from every step comes a row sooner.
    assert!(l(22, false) == whole && l(16, false) == floor);
}

#[test]
fn a_value_wears_its_tone() {
    let _en = english();
    let mut gui = room_gui();
    gui.app.dj.moods = MoodFilter::NONE.press(MoodScale::Valence, "happy").press(MoodScale::Energy, "relaxed");
    gui.app.dj.genre_filter = true;
    gui.app.dj.genre_mode = GenreMode::Blacklist;
    gui.app.dj.genres = vec!["chillhop".into(), "Boom Bap".into()];
    gui.app.dj.min_rating = 7;
    gui.app.dj.length = true;
    gui.app.dj.min_seconds = 120;
    gui.app.dj.max_seconds = 3750;
    gui.app.dj.bpm = true;
    gui.app.dj.harmonic = true;
    gui.app.dj.keyword_filter = true;
    gui.app.dj.keywords = vec!["live".into(), "remix".into()];
    let rows = draw(&mut gui);
    let value = |label: &str| room(&rows, row_of(&rows, label))[label.len()..].trim().to_string();
    assert_eq!(value("Smart filters"), "Happy · Relaxed ▸");
    assert_eq!(value("Genres"), "Skip Boom Bap · chillhop ▸", "sorted, whatever order they were ticked in");
    assert_eq!(value("Rating"), "3.5★+ ▸");
    assert_eq!(value("Length"), "2:00–1:02:30 ▸", "to the second, the hour from an hour up");
    assert_eq!(value("Beat matching"), "± 8 BPM ▸");
    assert_eq!(value("Harmonic mixing"), "[✓]");
    assert_eq!(value("Skip words"), "live · remix ▸");
    // DIM for how the session runs, OK while a value shapes the picks.
    let y = row_of(&rows, "First song");
    assert_eq!(fg_at(&mut gui, 61, y), th().dim, "Ask");
    let y = row_of(&rows, "Sonic similarity");
    assert_eq!(fg_at(&mut gui, 61, y), th().ok, "55%");
    let y = row_of(&rows, "Rating");
    assert_eq!(fg_at(&mut gui, 61, y), th().ok);
    // Only one bound set reads on its own.
    gui.app.dj.max_seconds = dj::LENGTH_RAIL_SECONDS;
    assert!(draw(&mut gui).join("\n").contains("2:00+ ▸"));
    gui.app.dj.min_seconds = 0;
    gui.app.dj.max_seconds = 360;
    assert!(draw(&mut gui).join("\n").contains("Under 6:00 ▸"));
    // A filter switched off is Any, whatever it still holds.
    gui.app.dj.length = false;
    gui.app.dj.keyword_filter = false;
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Length")).ends_with("Any ▸"));
    assert!(room(&rows, row_of(&rows, "Skip words")).ends_with("Off ▸"));
}

#[test]
fn a_setting_the_server_cannot_do_keeps_its_place_and_says_why() {
    let _en = english();
    let mut gui = room_gui();
    probed(
        &mut gui,
        DjServerInfo {
            version: Some("6.6.0".into()),
            discovery: false,
            discovery_ready: None,
            libraries: vec!["Music".into()],
            mood_filters: false,
        },
    );
    let rows = draw(&mut gui);
    let line = |label: &str| room(&rows, row_of(&rows, label)).trim_end().to_string();
    // The canvas's "One older server": the reason where the value would
    // be, ending at the values' own column, and no chevron.
    assert_eq!(cells(&rows[row_of(&rows, "Batch") as usize], 43, 64), " Needs server 6.26+  ");
    assert!(line("Sonic similarity").ends_with("Needs server 6.15.2+"));
    assert!(line("Smart filters").ends_with("No mood data"));
    assert!(line("Genres").ends_with("Needs server 6.7.1+"));
    assert!(line("Length").ends_with("Needs server 6.25+"));
    assert!(line("Beat matching").ends_with("Needs server 6.7.1+"));
    assert!(line("Harmonic mixing").ends_with("Needs server 6.7.1+"));
    // The rows did not move: the same lines as on a server that can.
    assert_eq!(row_of(&rows, "Artist cooldown"), 23);
    // DIM whole, and nothing to press.
    let y = row_of(&rows, "Genres");
    assert_eq!(fg_at(&mut gui, 17, y), th().dim, "the name");
    assert!(gui.ui.hit(Position { x: 17, y }).is_none(), "no click on a row the server can't do");
    gui.act(Act::DjRowKey(Row::Genres));
    assert!(gui.dj.sheet.is_none(), "and no sheet");
    // The keyboard walks past them.
    for _ in 0..12 {
        key(&mut gui, KeyCode::Down);
    }
    let mut seen = Vec::new();
    for _ in 0..12 {
        seen.push(gui.dj.cursor);
        key(&mut gui, KeyCode::Up);
    }
    assert!(seen.contains(&Some(Item::Row(Row::Rating))) && seen.contains(&Some(Item::Row(Row::SkipWords))));
    assert!(!seen.contains(&Some(Item::Row(Row::Genres))) && !seen.contains(&Some(Item::Row(Row::Sonic))));

    // The sonic row's other two reasons, and the scan's word once it is
    // under way.
    let mut scanning = info("6.33.1");
    scanning.discovery_ready = Some(false);
    probed(&mut gui, scanning);
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Sonic similarity")).trim_end().ends_with("Still scanning"));
    let mut off = info("6.33.1");
    off.discovery = false;
    probed(&mut gui, off);
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Sonic similarity")).trim_end().ends_with("Discovery off"));
    assert!(room(&rows, row_of(&rows, "Batch")).ends_with("4 songs ▸"), "the rest came back");
}

#[test]
fn a_peer_has_no_stars_and_no_moods_to_ask_for() {
    let _en = english();
    let mut gui = session_gui();
    let parent = "http://parent:3000";
    let pid = config::peer_identity(parent, 7);
    gui.app.servers.push(known(parent, "parent"));
    gui.app.servers.push(KnownServer { peer: Some((parent.into(), 7)), ..known(&pid, "Attic") });
    gui.app.dj_home = Some(pid.clone());
    gui.act(Act::Nav(DJ_NAV));
    let effects = gui.app.apply_event(Event::DjProbed { identity: pid, info: Some(info("6.33.1")) });
    gui.pend(effects);
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Rating")).trim_end().ends_with("Not on peers"));
    assert!(room(&rows, row_of(&rows, "Smart filters")).trim_end().ends_with("Not on peers"));
    assert!(room(&rows, row_of(&rows, "Genres")).ends_with("Any ▸"), "the rest of a peer's library is its own");
}

#[test]
fn a_running_session_that_dropped_the_pool_says_so_in_gold() {
    let _en = english();
    let mut gui = room_gui();
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::DjStartStop);
    gui.app.lane.sonic_drop = Some(DjNote::SonicRange);
    let rows = draw(&mut gui);
    let y = row_of(&rows, "Sonic similarity");
    assert_eq!(cells(&rows[y as usize], 49, 64), " ! Too strict ▸");
    assert_eq!(fg_at(&mut gui, 61, y), th().gold);
    // The sheet opens on it all the same — where the user goes to loosen
    // it — and a change there gives the pool another go.
    gui.act(Act::DjRowKey(Row::Sonic));
    assert!(draw(&mut gui).join("\n").contains("Sonic similarity · ! Too strict"), "the border says it too");
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Left);
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj.sonic_min_similarity, 0.54);
    assert!(gui.app.lane.sonic_drop.is_none(), "another go");
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Sonic similarity")).ends_with("54% ▸"));
    // Off, nothing is dropped: there is no session to have dropped it.
    gui.app.lane.sonic_drop = Some(DjNote::SonicUnscanned);
    assert!(draw(&mut gui).join("\n").contains("! Still scanning"));
    gui.act(Act::DjStartStop);
    assert!(!draw(&mut gui).join("\n").contains("! "));
}

#[test]
fn the_button_is_the_status_and_stop_always_stops() {
    let _en = english();
    let mut gui = room_gui();
    two_servers(&mut gui);
    gui.app.queue.items = vec![queued("a.mp3", None)];
    let rows = draw(&mut gui);
    assert_eq!(hit_text(&gui, &rows, "Start ▸"), Some(Act::DjStartStop));
    assert_eq!(fg_at(&mut gui, 17, 4), th().accent, "Start in the accent");
    gui.act(Act::DjStartStop);
    assert_eq!(gui.app.dj_server.as_deref(), Some(HOST), "started through the toggle");
    let rows = draw(&mut gui);
    assert_eq!(room(&rows, 5).trim_matches(['│', ' ']), "Stop");
    assert_eq!(fg_at(&mut gui, 17, 4), th().ok, "Stop lit, never the destructive colour");
    assert!(rows.join("\n").contains("Auto DJ •"), "and the nav row wears its dot");
    // Browsing another server, Stop still stops.
    gui.app.session.server = ATTIC.into();
    gui.app.session.server_id = ATTIC.into();
    gui.act(Act::DjStartStop);
    assert!(gui.app.dj_server.is_none(), "it used to move the DJ here");
    // And Start goes back to where it ran: Play from says so.
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Play from")).ends_with(" host ▸"));
}

#[test]
fn the_nav_row_opens_the_room_and_the_keyboard_walks_its_rows() {
    let _en = english();
    let mut gui = session_gui();
    let rows = draw(&mut gui);
    let nav = hit_text(&gui, &rows, "Auto DJ").expect("the nav row");
    assert_eq!(nav, Act::Nav(DJ_NAV));
    gui.act(nav);
    probed(&mut gui, info("6.33.1"));
    assert!(gui.dj.cursor.is_none(), "the room opens with the hand stowed");
    // ↓ from rest takes the button; ↑ from it, Preview on the title row.
    key(&mut gui, KeyCode::Down);
    assert_eq!(gui.dj.cursor, Some(Item::Toggle));
    key(&mut gui, KeyCode::Up);
    assert_eq!(gui.dj.cursor, Some(Item::Preview));
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::FirstSong)));
    // The hand is the accent slab, the row's width.
    let rows = draw(&mut gui);
    let y = row_of(&rows, "First song");
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| render(frame, &mut gui)).unwrap();
    let buffer = terminal.backend().buffer();
    assert!((17..64).all(|x| buffer[(x, y)].bg == th().accent), "the slab");
    assert_ne!(buffer[(64, y)].bg, th().accent);
    // Esc only stows the hand: a nav room has no way back.
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.cursor.is_none());
    assert_eq!((gui.screen, gui.active), (Screen::Library, DJ_NAV));
    // ↑ from rest takes the last row; Space on Harmonic flips the one
    // switch on the page.
    key(&mut gui, KeyCode::Up);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Cooldown)));
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Up);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Harmonic)));
    key(&mut gui, KeyCode::Char(' '));
    assert!(gui.app.dj.harmonic);
    // And a click on its row does the same, the hand staying where it was.
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Harmonic mixing");
    assert!(!gui.app.dj.harmonic);
    // D opens the room from anywhere.
    gui.act(Act::Nav(FILES_NAV));
    key(&mut gui, KeyCode::Char('D'));
    assert_eq!(gui.active, DJ_NAV);
}

// ── First song, the opening question, and the empty queue ───────────────────

#[test]
fn first_song_is_a_list_and_a_pick_applies() {
    let _en = english();
    let mut gui = room_gui();
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "First song");
    assert_eq!(gui.dj.first, Some(0), "down, on the answer in force");
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("• Ask") && all.contains("Random") && all.contains("Choose") && all.contains("Recently played"), "{all}");
    // Hanging under its row, its last cell under the chevron's.
    let (x, y) = find(&rows, "╭───────────────────╮").expect("the list's frame");
    assert_eq!((x + 20, y), (64, row_of(&rows, "First song") + 1));
    assert_eq!(hit_text(&gui, &rows, "Random"), Some(Act::DjFirst(EmptyQueueStart::Random)));
    // A click anywhere else closes and keeps.
    assert_eq!(gui.ui.hit(Position { x: 2, y: 2 }), Some(Act::DjFirstClose));
    gui.act(Act::DjFirstClose);
    assert!(gui.dj.first.is_none());
    assert_eq!(gui.app.dj.empty_queue, EmptyQueueStart::Ask);
    // By the keyboard: Enter on the row, ↓ ↓ ↓, Enter.
    gui.act(Act::DjRowKey(Row::FirstSong));
    for _ in 0..3 {
        key(&mut gui, KeyCode::Down);
    }
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj.empty_queue, EmptyQueueStart::Recent);
    assert!(gui.dj.first.is_none(), "a pick applies and closes");
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "First song")).ends_with("Recently played ▾"));
    // Esc closes and keeps.
    gui.act(Act::DjRowKey(Row::FirstSong));
    assert_eq!(gui.dj.first, Some(3));
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Esc);
    assert_eq!(gui.app.dj.empty_queue, EmptyQueueStart::Recent);
}

/// The server's answer to what was played last on it.
fn recents(gui: &mut Gui) {
    let effects = gui.app.apply_event(Event::DjRecent {
        identity: HOST.into(),
        tracks: vec![
            song("lib/one.mp3", "Anything But Everything", "Color Out"),
            song("lib/two.mp3", "High Hopes", "Color Out"),
            song("lib/three.mp3", "Manor", "ALM"),
        ],
    });
    gui.pend(effects);
}

#[test]
fn the_toggle_on_an_empty_queue_asks_under_the_rows_own_name() {
    let _en = english();
    let mut gui = session_gui();
    gui.act(Act::AutoDj);
    assert!(gui.app.dj_chooser.is_some(), "the opening question");
    assert!(
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DjRecent { identity, .. }) if identity == HOST)),
        "and what was played last is asked for"
    );
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("─ First song ─"), "its name on the border: {all}");
    assert!(all.contains("Random") && all.contains("Choose") && all.contains("[ ] Remember"), "{all}");
    assert!(!all.contains("Recently played"), "left out while there is nothing to offer");
    // The bar beneath is inert while it is up.
    let (x, y) = find(&rows, "auto-dj").expect("the bar");
    assert_eq!(gui.ui.hit(Position { x, y }), Some(Act::DjCancel), "a click anywhere else leaves the DJ off");

    // The box, then the first answer: the opener goes out and the answer
    // is remembered.
    click(&mut gui, &rows, "Remember");
    assert!(gui.app.dj_chooser.as_ref().is_some_and(|c| c.remember));
    let rows = draw(&mut gui);
    assert!(rows.join("\n").contains("[✓] Remember"));
    click(&mut gui, &rows, "Random");
    assert!(gui.app.dj_chooser.is_none());
    assert!(opener_asked(&gui), "the filtered opener");
    assert_eq!(gui.app.dj.empty_queue, EmptyQueueStart::Random);

    // Esc dismisses and the DJ stays off.
    let mut gui = session_gui();
    gui.act(Act::AutoDj);
    key(&mut gui, KeyCode::Esc);
    assert!(gui.app.dj_chooser.is_none() && !gui.app.dj_armed());
    // Choose hands off to the library under its banner.
    gui.act(Act::AutoDj);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.capture, Some(Capture::DjSeed));
    assert!(draw(&mut gui).join("\n").contains("Pick the opening song"));
}

#[test]
fn recently_played_joins_the_question_and_a_song_of_it_opens_the_session() {
    let _en = english();
    let mut gui = session_gui();
    gui.act(Act::AutoDj);
    recents(&mut gui);
    let rows = draw(&mut gui);
    assert!(rows.join("\n").contains("Recently played ▾"), "the third answer, once it has songs");
    assert!(row_of(&rows, "Remember") == row_of(&rows, "Recently played ▾") + 2, "the box under a blank row");
    // ↓ ↓ to it, Enter lets its list down: the songs newest first, the
    // artist beside the title.
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Enter);
    assert!(gui.app.dj_chooser.as_ref().is_some_and(|c| c.recent_open));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("Anything But Everything · Color Out") && all.contains("Manor · ALM"), "{all}");
    assert!(all.contains("[ ] Remember"), "the box stays in sight beside the list");
    assert_eq!(hit_text(&gui, &rows, "High Hopes"), Some(Act::DjRecentPick(1)));
    // Esc folds the list, not the question.
    key(&mut gui, KeyCode::Esc);
    assert!(gui.app.dj_chooser.as_ref().is_some_and(|c| !c.recent_open));
    // Down again, the second song, with the box ticked: that song opens
    // the session, and what is remembered is the way to start.
    key(&mut gui, KeyCode::Enter);
    key(&mut gui, KeyCode::Char(' '));
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Enter);
    assert!(gui.app.dj_chooser.is_none());
    assert_eq!(gui.app.dj_server.as_deref(), Some(HOST));
    assert_eq!(gui.app.queue.items.len(), 1);
    assert_eq!(gui.app.queue.items[0].filepath, "lib/two.mp3");
    assert!(gui.app.queue.items[0].dj.is_none(), "the user chose it");
    assert_eq!(gui.app.dj.empty_queue, EmptyQueueStart::Recent);

    // Remembered, the question opens with its list already down once the
    // songs are in hand — and a list folded since stays folded.
    gui.act(Act::DjStartStop);
    gui.app.queue.items.clear();
    gui.act(Act::AutoDj);
    let chooser = gui.app.dj_chooser.clone().expect("it asks every time: a song has to be named");
    assert!(chooser.recent_open);
    let rows = draw(&mut gui);
    assert!(rows.join("\n").contains("Anything But Everything"), "the list kept from before stands while the new one is out");
    key(&mut gui, KeyCode::Esc);
    recents(&mut gui);
    draw(&mut gui);
    assert!(gui.app.dj_chooser.as_ref().is_some_and(|c| !c.recent_open), "folded stays folded");
}

#[test]
fn the_empty_queue_offers_the_openers_only_while_the_dj_is_armed() {
    let _en = english();
    let mut gui = session_gui();
    let all = draw(&mut gui).join("\n");
    assert!(all.contains("(nothing queued yet)") && !all.contains("Pick a random song"), "off: just the label");
    // Armed, with an empty queue (a cleared one).
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::AutoDj);
    gui.app.queue.items.clear();
    gui.pend(Vec::new());
    assert!(
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DjRecent { .. }))),
        "the queue ran out: what was played last is asked for, once"
    );
    let asks = |gui: &Gui| gui.pending.iter().filter(|e| matches!(e, Effect::Api(ApiCmd::DjRecent { .. }))).count();
    gui.pend(Vec::new());
    assert_eq!(asks(&gui), 1, "once for this emptiness");
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("Auto DJ is on and needs an") && all.contains("opening song."), "the hint, wrapped: {all}");
    assert_eq!(hit_text(&gui, &rows, "Pick a random song"), Some(Act::DjSurprise));
    assert_eq!(hit_text(&gui, &rows, "Choose a song"), Some(Act::DjPick));
    assert!(!all.contains("Recently played"), "nothing to offer yet");
    gui.act(Act::DjSurprise);
    assert!(opener_asked(&gui));

    // The third opener, once the server has answered with songs: its list
    // hangs under it, the panel's width, and a song of it opens the queue.
    recents(&mut gui);
    let rows = draw(&mut gui);
    assert_eq!(hit_text(&gui, &rows, "Recently played ▾"), Some(Act::DjRecentOpen));
    click(&mut gui, &rows, "Recently played ▾");
    let rows = draw(&mut gui);
    assert_eq!(hit_text(&gui, &rows, "Manor"), Some(Act::DjRecentPick(2)));
    let (x, _) = find(&rows, "Manor").expect("the list");
    assert_eq!(x, 70, "inside the panel's frame");
    click(&mut gui, &rows, "Manor");
    assert_eq!(gui.app.queue.items.len(), 1);
    assert_eq!(gui.app.queue.items[0].filepath, "lib/three.mp3");
    assert!(gui.dj.recent.is_none());
    // Switched off, the buttons leave with it.
    gui.app.queue.items.clear();
    gui.act(Act::AutoDj);
    assert!(!draw(&mut gui).join("\n").contains("Pick a random song"));
}

#[test]
fn dj_picked_rows_wear_the_badge_and_the_road_wears_its_banner() {
    let _en = english();
    let mut gui = session_gui();
    gui.app.queue.items = vec![
        queued("lib/user.mp3", None),
        queued("lib/random.mp3", Some(DjMark { sonic: false })),
        queued("lib/sonic.mp3", Some(DjMark { sonic: true })),
    ];
    let all = draw(&mut gui).join("\n");
    assert!(all.contains("∞ random.mp3") && all.contains("≈ sonic.mp3"), "{all}");
    assert!(!all.contains("∞ user.mp3") && !all.contains("≈ user.mp3"), "a user row wears none");
}

// ── The sheets ──────────────────────────────────────────────────────────────

/// The sheet's rows as drawn: its frame's lines.
fn sheet_text(gui: &mut Gui) -> String {
    draw(gui).into_iter().filter(|row| cells(row, 17, 18) == "│" || cells(row, 17, 18) == "╭").collect::<Vec<_>>().join("\n")
}

#[test]
fn a_sheet_edits_a_draft_and_closing_it_any_way_applies_it() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjRowKey(Row::Sonic));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    // The canvas's "A sheet over the room": 66 cells, the row's name and
    // its value on the top border, the switch, the bar, the anchor, the
    // old room's help line inside, and [ Done ] at the foot — no [X].
    let (x, y) = find(&rows, "╭─ Sonic similarity · 55% ─").expect("the title and the value on the border");
    assert_eq!(x, 17);
    assert_eq!(cells(&rows[y as usize], 17, 83).chars().count(), 66);
    assert_eq!(cells(&rows[y as usize], 82, 83), "╮");
    assert!(all.contains("[✓] On") && all.contains("(•) Drift") && all.contains("( ) Stay close"), "{all}");
    assert!(all.contains("Only pick songs that sound like the session"), "{all}");
    assert!(all.contains("[ Done ]") && !all.contains("[X]"));
    assert!(!all.contains("clear"), "nothing here to clear");
    // Two points a cell across the band: 55% lights thirteen of 25.
    let bar = rows.iter().find(|r| r.contains("▰")).expect("the bar");
    assert_eq!((bar.matches('▰').count(), bar.matches('▱').count()), (13, 12), "{bar}");
    // The room beneath is inert.
    assert_eq!(gui.ui.hit(Position { x: 2, y: 2 }), Some(Act::Guard));

    // The switch, by the keyboard: off, the controls rest and the value on
    // the border follows the draft; nothing changes until it closes.
    key(&mut gui, KeyCode::Char(' '));
    let all = sheet_text(&mut gui);
    assert!(all.contains("Sonic similarity · Off") && all.contains("[ ] On"), "{all}");
    assert!(gui.app.dj.sonic, "a draft");
    key(&mut gui, KeyCode::Down);
    let before = gui.dj.sheet.clone().map(|s| format!("{s:?}"));
    key(&mut gui, KeyCode::Right);
    assert!(format!("{:?}", gui.dj.sheet.as_ref().unwrap()).contains("floor: 55"), "a resting bar takes no step");
    drop(before);
    // Esc applies, like Done.
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.sheet.is_none());
    assert!(!gui.app.dj.sonic);
    assert!(gui.prefs_unsaved.is_some(), "and the setting is saved once the edits rest");
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Sonic similarity")).ends_with("Off ▸"));

    // By the pointer: the switch back on, a cell of the bar, the other
    // anchor, Done.
    gui.prefs_unsaved = None;
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Sonic similarity");
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[ ] On");
    let rows = draw(&mut gui);
    let (bx, by) = find(&rows, "▰").or_else(|| find(&rows, "▱")).expect("the bar's first cell");
    let act = gui.ui.hit(Position { x: bx + 19, y: by }).expect("a cell sets");
    gui.act(act);
    let rows = draw(&mut gui);
    assert!(rows.join("\n").contains("Sonic similarity · 70%"), "the twentieth cell is 70%");
    click(&mut gui, &rows, "( ) Stay close");
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[ Done ]");
    assert!(gui.dj.sheet.is_none());
    assert!(gui.app.dj.sonic);
    assert_eq!(gui.app.dj.sonic_min_similarity, 0.7);
    assert_eq!(gui.app.dj.sonic_anchor, SonicAnchor::Locked);

    // A sheet closed as it was opened writes nothing.
    gui.prefs_unsaved = None;
    gui.act(Act::DjRowKey(Row::Sonic));
    key(&mut gui, KeyCode::Enter);
    assert!(gui.dj.sheet.is_none() && gui.prefs_unsaved.is_none());
}

#[test]
fn the_bars_are_a_cell_a_step() {
    let _en = english();
    let mut gui = room_gui();
    // Batch: a cell a song, 1 to 25.
    gui.act(Act::DjRowKey(Row::Batch));
    let rows = draw(&mut gui);
    let bar = rows.iter().find(|r| r.contains("▰")).expect("the bar");
    assert_eq!((bar.matches('▰').count(), bar.matches('▱').count()), (4, 21));
    assert!(rows.join("\n").contains("─ Batch · 4 songs ─"));
    assert!(rows.join("\n").contains("How many songs Auto DJ queues each time it runs."));
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Right);
    for _ in 0..9 {
        key(&mut gui, KeyCode::Left);
    }
    assert!(sheet_text(&mut gui).contains("Batch · 1 song "), "held at its floor, in the singular");
    let rows = draw(&mut gui);
    let (x, y) = find(&rows, "+ ").filter(|(x, _)| *x > 40).expect("the bar's +");
    let act = gui.ui.hit(Position { x, y }).expect("+ steps");
    gui.act(act.clone());
    gui.act(act);
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj.songs_per_fetch, 3);

    // Beat matching: switched off it opens on [ ] On and its bar rests;
    // on, a cell a BPM.
    gui.act(Act::DjRowKey(Row::Beat));
    let all = sheet_text(&mut gui);
    assert!(all.contains("Beat matching · Off") && all.contains("[ ] On") && all.contains("± 8 BPM"), "{all}");
    key(&mut gui, KeyCode::Down);
    assert!(sheet_text(&mut gui).contains("Beat matching · Off"), "↓ has nowhere to go but Done");
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Char(' '));
    key(&mut gui, KeyCode::Down);
    for _ in 0..4 {
        key(&mut gui, KeyCode::Right);
    }
    assert!(sheet_text(&mut gui).contains("Beat matching · ± 12 BPM"));
    key(&mut gui, KeyCode::Enter);
    assert!(gui.app.dj.bpm);
    assert_eq!(gui.app.dj.bpm_tolerance, 12);

    // Artist cooldown: none of its twenty cells lit is Off.
    gui.act(Act::DjRowKey(Row::Cooldown));
    for _ in 0..5 {
        key(&mut gui, KeyCode::Left);
    }
    assert!(sheet_text(&mut gui).contains("Artist cooldown · Off"));
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj.artist_cooldown, 0);
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Artist cooldown")).ends_with("Off ▸"));
}

#[test]
fn length_is_typed_to_the_second() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjRowKey(Row::Length));
    let all = sheet_text(&mut gui);
    assert!(all.contains("─ Length · Any ─") && all.contains("Shortest") && all.contains("Longest"), "{all}");
    assert!(all.contains("Type a length to the second (2:00, 90, 1:02:30)"), "{all}");
    assert!(!all.contains("clear"));
    // Typed at its end; Tab to the other bound.
    type_text(&mut gui, "2:00");
    key(&mut gui, KeyCode::Tab);
    type_text(&mut gui, "1:02:30");
    let all = sheet_text(&mut gui);
    assert!(all.contains("Length · 2:00–1:02:30") && all.contains("clear"), "{all}");
    assert!(all.contains("[ ] Include unknown"));
    // ← → step a bound by fifteen seconds; bare seconds read too.
    key(&mut gui, KeyCode::Left);
    assert!(sheet_text(&mut gui).contains("2:00–1:02:15"));
    key(&mut gui, KeyCode::BackTab);
    key(&mut gui, KeyCode::Delete);
    type_text(&mut gui, "90");
    assert!(sheet_text(&mut gui).contains("Length · 1:30–1:02:15"));
    // Include unknown, then Done.
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Char(' '));
    key(&mut gui, KeyCode::Enter);
    let s = &gui.app.dj;
    assert_eq!((s.length, s.min_seconds, s.max_seconds, s.allow_unknown_length), (true, 90, 3735, true));
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Length")).ends_with("1:30–1:02:15 ▸"));

    // What does not read as a length shows on the border as no bound, and
    // closing leaves that side as it was.
    gui.act(Act::DjRowKey(Row::Length));
    key(&mut gui, KeyCode::Delete);
    type_text(&mut gui, "9:99");
    assert!(sheet_text(&mut gui).contains("Length · Under 1:02:15"));
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj.min_seconds, 90, "left as it was");
    // The steps by the pointer, and clear: both sides empty is Any, which
    // is the window off.
    gui.act(Act::DjRowKey(Row::Length));
    let rows = draw(&mut gui);
    let y = row_of(&rows, "Shortest");
    let (x, _) = find(&rows, "Shortest").unwrap();
    let minus = gui.ui.hit(Position { x: x + 23, y }).expect("the bound's -");
    gui.act(minus);
    assert!(sheet_text(&mut gui).contains("Length · 1:15–1:02:15"));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "clear");
    assert!(sheet_text(&mut gui).contains("Length · Any"));
    key(&mut gui, KeyCode::Enter);
    assert!(!gui.app.dj.length);
    assert_eq!((gui.app.dj.min_seconds, gui.app.dj.max_seconds), (0, dj::LENGTH_RAIL_SECONDS));
    // A longest under the shortest: the bounds change places.
    gui.act(Act::DjRowKey(Row::Length));
    type_text(&mut gui, "6:00");
    key(&mut gui, KeyCode::Tab);
    type_text(&mut gui, "2:00");
    key(&mut gui, KeyCode::Enter);
    assert_eq!((gui.app.dj.min_seconds, gui.app.dj.max_seconds), (120, 360));
}

#[test]
fn skip_words_are_typed_in_and_let_go_by_their_x() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjRowKey(Row::SkipWords));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("─ Skip words · Off ─") && all.contains("e.g. \"live\" or \"remix\""), "{all}");
    assert!(all.contains("Skip picks whose title, artist, album, or filepath contains"));
    // The line is the kit's text input — a 3-row card, the hint inside —
    // with `+` framed beside it: 56 cells, a gap, five, the sheet's 62.
    let (cx, cy) = find(&rows, &format!("╭{}╮ ╭───╮", "─".repeat(54))).expect("the card and the plus");
    assert_eq!(cx, 19, "from the sheet's first column of text");
    assert_eq!(cells(&rows[cy as usize + 1], 19, 81), format!("│ ▏{:<52}│ │ + │", "e.g. \"live\" or \"remix\""));
    assert_eq!(cells(&rows[cy as usize + 2], 19, 81), format!("╰{}╯ ╰───╯", "─".repeat(54)));
    // The card has the keyboard: its border is the accent. With nothing
    // typed the plus rests — DIM, and no click.
    assert_eq!(fg_at(&mut gui, cx, cy), th().accent);
    assert_eq!(fg_at(&mut gui, cx + 59, cy + 1), th().dim);
    assert_eq!(gui.ui.hit(Position { x: cx + 59, y: cy + 1 }), Some(Act::Guard));
    // Every letter is the line's; Enter adds the word and clears for the
    // next; the same word again is one word.
    type_text(&mut gui, "live");
    // A word on the line lights the plus, and a click on it adds the word
    // as Enter does — the line cleared, the keyboard still its own.
    assert_eq!(fg_at(&mut gui, cx + 59, cy + 1), th().accent);
    let rows = draw(&mut gui);
    assert_eq!(
        gui.ui.hit(Position { x: cx + 59, y: cy + 1 }),
        Some(Act::DjSheet(super::SheetAct::AddWord)),
        "{}",
        rows.join("\n")
    );
    gui.act(Act::DjSheet(super::SheetAct::AddWord));
    let all = sheet_text(&mut gui);
    assert!(all.contains("Skip words · live ─") && all.contains("live [X]"), "{all}");
    type_text(&mut gui, "x");
    key(&mut gui, KeyCode::Backspace);
    assert!(sheet_text(&mut gui).contains("│ ▏e.g."), "the line is empty again, and typed into");
    type_text(&mut gui, "LIVE");
    key(&mut gui, KeyCode::Enter);
    type_text(&mut gui, "radio edit");
    key(&mut gui, KeyCode::Enter);
    let all = sheet_text(&mut gui);
    assert!(all.contains("Skip words · live · radio edit") && all.contains("live [X]   radio edit [X]"), "{all}");
    assert!(all.contains("clear"));
    assert!(!gui.app.dj.keyword_filter, "a draft");
    // ↓ to the words, → to the second, x lets it go. (The click on the
    // plus stowed the hand, as a pointer's press does: the first ↓ only
    // shows it, on the line.)
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Char('x'));
    assert!(!sheet_text(&mut gui).contains("radio edit"));
    // A word still on the line when the sheet closes counts.
    type_text(&mut gui, "remix");
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.sheet.is_none());
    assert_eq!(gui.app.dj.keywords, vec!["live", "remix"]);
    assert!(gui.app.dj.keyword_filter);
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Skip words")).ends_with("live · remix ▸"));
    // A click on a word's [X]; the last one gone is the filter off.
    gui.act(Act::DjRowKey(Row::SkipWords));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "live [X]");
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "clear");
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[ Done ]");
    assert!(gui.app.dj.keywords.is_empty() && !gui.app.dj.keyword_filter);
    // At the floor the sheet at its tallest — the card, four rows of words,
    // the help — still stands whole over the room.
    gui.act(Act::DjRowKey(Row::SkipWords));
    for n in 0..24 {
        type_text(&mut gui, &format!("a longer word {n:02}"));
        key(&mut gui, KeyCode::Enter);
    }
    let rows = draw_sized(&mut gui, 100, 24);
    let all = rows.join("\n");
    let (_, top) = find(&rows, "─ Skip words · ").expect("the sheet's border");
    let (_, foot) = find(&rows, "[ Done ]").expect("the sheet's foot");
    assert_eq!(foot - top, 12, "the card's three rows, four of words, the help's two and the foot: {all}");
    assert!(top >= 2 && foot + 1 <= 18, "inside the room's rows: {all}");
    assert!(all.contains("╭───╮") && all.contains("│ + │") && all.contains("Skip picks whose title"), "{all}");
    key(&mut gui, KeyCode::Esc);
}

/// The Genres sheet's list lands.
fn genres_land(gui: &mut Gui, source: GenreSource, names: &[&str], sources: Option<GenreSources>) {
    let effects = gui.app.apply_event(Event::DjGenres {
        source,
        genres: names.iter().map(|name| Genre { name: name.to_string(), track_count: None }).collect(),
        sources,
    });
    gui.pend(effects);
}

#[test]
fn genres_are_ticked_in_the_sheet_and_written_to_the_dj_servers_entry() {
    let _en = english();
    let mut gui = room_gui();
    gui.app.servers.push(known(HOST, "host"));
    gui.pending.clear();
    gui.act(Act::DjRowKey(Row::Genres));
    assert!(
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DjGenres { source: GenreSource::Both, .. }))),
        "the list is asked for, both kinds: {:?}",
        gui.pending
    );
    assert!(sheet_text(&mut gui).contains("loading genres…"));
    genres_land(&mut gui, GenreSource::Both, &["Ambient", "Boom Bap", "chillhop", "Dub"], Some(GenreSources { tag: 3, model: 2 }));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("─ Genres · Any ─") && all.contains("(•) Only") && all.contains("( ) Skip"), "{all}");
    assert!(all.contains("( ) Generated") && all.contains("( ) Tags") && all.contains("(•) Mixed"), "both kinds here: {all}");
    assert!(all.contains("Search genres…") && all.contains("[ ] Ambient") && all.contains("0 selected"), "{all}");
    // Space ticks the row under the hand; a click ticks its own.
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Char(' '));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[ ] chillhop");
    let all = sheet_text(&mut gui);
    assert!(all.contains("Genres · Boom Bap · chillhop") && all.contains("2 selected") && all.contains("clear"), "{all}");
    assert!(all.contains("[✓] Boom Bap") && all.contains("[✓] chillhop"));
    // Typing narrows the list; the letters are the search line's.
    type_text(&mut gui, "du");
    let all = sheet_text(&mut gui);
    assert!(all.contains("[ ] Dub") && !all.contains("Ambient"), "{all}");
    type_text(&mut gui, "zz");
    assert!(sheet_text(&mut gui).contains("No genres match \"duzz\"."));
    for _ in 0..4 {
        key(&mut gui, KeyCode::Backspace);
    }
    // Skip, and the generated kind: the list is asked for again, and what
    // is chosen stands first in it.
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "( ) Skip");
    gui.pending.clear();
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "( ) Generated");
    assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DjGenres { source: GenreSource::Model, .. }))));
    genres_land(&mut gui, GenreSource::Both, &["stale"], None);
    assert!(!sheet_text(&mut gui).contains("stale"), "an answer under a kind no longer asked for is dropped");
    genres_land(&mut gui, GenreSource::Model, &["Electronic", "Hip-Hop"], Some(GenreSources { tag: 3, model: 2 }));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("Genres · Skip Boom Bap · chillhop"), "{all}");
    assert!(row_of(&rows, "[✓] Boom Bap") < row_of(&rows, "[ ] Electronic"), "the chosen first");
    assert!(gui.app.dj_library().genres.is_empty(), "a draft: nothing written yet");
    // Done writes the rule whole to the DJ's server's own entry.
    gui.pending.clear();
    click(&mut gui, &rows, "[ Done ]");
    assert!(gui.dj.sheet.is_none() && gui.app.dj_panel.genres.is_none());
    let saved = gui.pending.iter().find_map(|e| match e {
        Effect::SaveDjLibrary { server, overrides } => Some((server.clone(), overrides.clone())),
        _ => None,
    });
    let (server, own) = saved.expect("written at once: a library's rule is its entry's");
    assert_eq!(server, HOST);
    assert_eq!(own.genres.as_deref(), Some(&["Boom Bap".to_string(), "chillhop".to_string()][..]));
    assert_eq!((own.genre_filter, own.genre_mode.as_deref(), own.genre_source.as_deref()), (Some(true), Some("blacklist"), Some("model")));
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Genres")).ends_with("Skip Boom Bap · chillhop ▸"));

    // A server that does not say what it has of each kind offers none.
    gui.act(Act::DjRowKey(Row::Genres));
    genres_land(&mut gui, GenreSource::Model, &["Ambient"], None);
    let all = sheet_text(&mut gui);
    assert!(!all.contains("Generated") && !all.contains("Mixed"), "{all}");
    // clear, then Done: the filter off, the way it cuts kept.
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "clear");
    key(&mut gui, KeyCode::Enter);
    let library = gui.app.dj_library();
    assert!(!library.genre_filter && library.genres.is_empty());
    assert_eq!(library.genre_mode, GenreMode::Blacklist);
}

#[test]
fn a_failed_genres_load_says_so_in_the_sheet() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjRowKey(Row::Genres));
    let effects = gui.app.apply_event(Event::GenresFailed("the server said no".into()));
    gui.pend(effects);
    let all = sheet_text(&mut gui);
    assert!(all.contains("Could not load genres") && all.contains("the server said no"), "{all}");
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.sheet.is_none() && gui.app.dj_panel.genres.is_none());
}

#[test]
fn rating_is_five_stars_a_half_at_a_time() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjRowKey(Row::Rating));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("─ Rating · Any ─") && all.contains("☆ ☆ ☆ ☆ ☆"), "{all}");
    assert!(all.contains("Only pick songs at or above this rating."));
    // A click on the fourth star is four; a second click on it, three and
    // a half.
    let (x, y) = find(&rows, "☆ ☆ ☆ ☆ ☆").unwrap();
    let fourth = gui.ui.hit(Position { x: x + 6, y }).expect("a star");
    gui.act(fourth.clone());
    assert!(sheet_text(&mut gui).contains("Rating · 4★+"));
    gui.act(fourth);
    let all = sheet_text(&mut gui);
    assert!(all.contains("Rating · 3.5★+") && all.contains("★ ★ ★ ½ ☆") && all.contains("clear"), "{all}");
    // ← → step by halves, once the first walking key has shown the hand.
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Left);
    assert!(sheet_text(&mut gui).contains("Rating · 4.5★+"));
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj_library().min_rating, 9);
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Rating")).ends_with("4.5★+ ▸"));
}

#[test]
fn play_from_names_the_server_and_its_folders() {
    let _en = english();
    let mut gui = room_gui();
    two_servers(&mut gui);
    two_libraries(&mut gui);
    gui.act(Act::DjRowKey(Row::PlayFrom));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    // One sheet for where the music comes from: the servers — one ticked —
    // and under them the folders of the one the rows are about.
    assert!(all.contains("─ Play from · host:3000 ─"), "{all}");
    assert!(all.contains("(•) host:3000") && all.contains("( ) attic:3000"), "{all}");
    assert!(all.contains("[✓] Music") && all.contains("[✓] Pods"), "{all}");
    // A folder switched off; the last one on stays on, and says why.
    click(&mut gui, &rows, "[✓] Pods");
    let rows = draw(&mut gui);
    assert!(rows.join("\n").contains("[ ] Pods"));
    click(&mut gui, &rows, "[✓] Music");
    assert!(sheet_text(&mut gui).contains("[✓] Music"), "refused");
    assert!(gui.note.as_ref().is_some_and(|(text, _)| text.contains("At least one source is required.")));
    assert!(gui.app.dj_sources_off().is_empty(), "a draft");
    gui.pending.clear();
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj_sources_off(), vec!["Pods"]);
    assert!(gui.pending.iter().any(|e| matches!(e, Effect::SaveDjLibrary { .. })));

    // Off, another server ticked is only where Start will go: the rows are
    // about it now, and it is asked what it offers.
    gui.pending.clear();
    gui.act(Act::DjRowKey(Row::PlayFrom));
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Char(' '));
    let all = sheet_text(&mut gui);
    assert!(all.contains("Play from · attic:3000") && !all.contains("Music"), "another server's folders are its own: {all}");
    key(&mut gui, KeyCode::Enter);
    assert!(!gui.app.dj_armed(), "off stays off");
    assert_eq!(gui.app.dj_home.as_deref(), Some(ATTIC));
    assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DjProbe { identity, .. }) if identity == ATTIC)));
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Play from")).ends_with(" attic ▸"));
    // Start starts there, whichever server is browsed.
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::DjStartStop);
    assert_eq!(gui.app.dj_server.as_deref(), Some(ATTIC));
    // Running, another server ticked moves the DJ: a new lane there.
    let epoch = gui.app.lane.epoch;
    gui.act(Act::DjRowKey(Row::PlayFrom));
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Char(' '));
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj_server.as_deref(), Some(HOST));
    assert_ne!(gui.app.lane.epoch, epoch);
    // Closed as it was opened, nothing moves.
    let epoch = gui.app.lane.epoch;
    gui.act(Act::DjRowKey(Row::PlayFrom));
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.lane.epoch, epoch, "re-selecting is a no-op");
}

/// More servers than the sheet affords stand behind its window — the
/// wheel, the kit's scrollbar and a walking key reach every one of them.
#[test]
fn play_from_scrolls_where_the_servers_outgrow_the_sheet() {
    let _en = english();
    let mut gui = room_gui();
    two_servers(&mut gui);
    for n in 0..30 {
        let url = format!("http://box{n:02}:3000");
        gui.config.servers.push(config::ServerEntry { url: url.clone(), ..Default::default() });
        gui.app.servers.push(known(&url, &format!("box{n:02}")));
    }
    gui.act(Act::DjRow(Row::PlayFrom));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("(•) host:3000") && all.contains("( ) box00:3000"), "{all}");
    assert!(!all.contains("( ) box29:3000"), "the last is behind the window: {all}");
    assert!(all.contains("[ Done ]") && all.contains("▼"), "the foot stands, and the list says there is more: {all}");
    // A sheet is a modal, and the shell hands a modal's wheel to the DJ's
    // layers: the window moves, and the last server takes a click.
    gui.wheel(Position { x: 50, y: 12 }, 40);
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("( ) box29:3000") && !all.contains("(•) host:3000"), "{all}");
    click(&mut gui, &rows, "( ) box29:3000");
    assert!(sheet_text(&mut gui).contains("(•) box29:3000"));
    // A walking key brings the hand's line into view, wherever the wheel
    // left the window.
    for _ in 0..40 {
        key(&mut gui, KeyCode::Up);
    }
    let all = sheet_text(&mut gui);
    assert!(all.contains("( ) host:3000") && !all.contains("(•) box29:3000"), "{all}");
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj_home.as_deref(), Some("http://box29:3000"));
}

/// The wheel is the lists': a sheet's turns under it, and a Recently
/// played list walks — its window following the cursor — so every song of
/// the twenty is the pointer's to reach.
#[test]
fn the_wheel_turns_a_sheets_list_and_walks_recently_played() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjRowKey(Row::Genres));
    let names: Vec<String> = (0..40).map(|n| format!("genre{n:02}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    genres_land(&mut gui, GenreSource::Both, &names, None);
    let all = sheet_text(&mut gui);
    assert!(all.contains("genre00") && !all.contains("genre39"), "{all}");
    gui.wheel(Position { x: 50, y: 15 }, 100);
    let all = sheet_text(&mut gui);
    assert!(all.contains("genre39") && !all.contains("genre00"), "{all}");
    key(&mut gui, KeyCode::Esc);

    // The opening question's list.
    let twenty = || -> Vec<Track> {
        (0..20).map(|n| song(&format!("lib/{n}.mp3"), &format!("Song {n:02}"), "Band")).collect()
    };
    let mut gui = session_gui();
    gui.act(Act::AutoDj);
    let effects = gui.app.apply_event(Event::DjRecent { identity: HOST.into(), tracks: twenty() });
    gui.pend(effects);
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Recently played ▾");
    let all = draw(&mut gui).join("\n");
    assert!(all.contains("Song 00") && !all.contains("Song 19"), "{all}");
    gui.wheel(Position { x: 50, y: 15 }, 100);
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("Song 19") && !all.contains("Song 00"), "{all}");
    gui.wheel(Position { x: 50, y: 15 }, -100);
    assert!(draw(&mut gui).join("\n").contains("Song 00"));

    // The armed empty queue's list, in the short window.
    let mut gui = session_gui();
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::AutoDj);
    gui.app.queue.items.clear();
    gui.pend(Vec::new());
    let effects = gui.app.apply_event(Event::DjRecent { identity: HOST.into(), tracks: twenty() });
    gui.pend(effects);
    let rows = draw_sized(&mut gui, 100, 24);
    click(&mut gui, &rows, "Recently played ▾");
    let all = draw_sized(&mut gui, 100, 24).join("\n");
    assert!(all.contains("Song 00") && !all.contains("Song 19"), "{all}");
    gui.wheel(Position { x: 80, y: 12 }, 100);
    let rows = draw_sized(&mut gui, 100, 24);
    assert!(rows.join("\n").contains("Song 19"), "{}", rows.join("\n"));
    click(&mut gui, &rows, "Song 19");
    assert_eq!(gui.app.queue.items[0].filepath, "lib/19.mp3");
}

#[test]
fn preview_asks_three_picks_without_queueing_and_lists_them_in_its_sheet() {
    let _en = english();
    let mut gui = room_gui();
    gui.pending.clear();
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Preview picks ▸");
    assert!(
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AutoDjSample { count: 3, .. }))),
        "three picks are asked for"
    );
    let all = sheet_text(&mut gui);
    assert!(all.contains("─ Preview picks ─") && all.contains("picking…"), "{all}");
    let mut picks = vec![song("lib/a.mp3", "Therapy", "Color Out"), song("lib/b.mp3", "High Hopes", "Color Out")];
    picks[0].metadata.duration = Some(223.0);
    let effects = gui.app.apply_event(Event::AutoDjSample {
        tracks: picks,
        pool: Some(SonicReport { similarity: Some(0.7), pool_size: 412 }),
        note: None,
    });
    gui.pend(effects);
    let all = sheet_text(&mut gui);
    assert!(all.contains("Therapy") && all.contains("Color Out") && all.contains("3:43"), "{all}");
    assert!(all.contains("412 tracks in the sonic pool") && all.contains("[ Done ]"), "{all}");
    assert!(gui.app.queue.items.is_empty(), "none of them queued");
    key(&mut gui, KeyCode::Enter);
    assert!(gui.dj.sheet.is_none());
}

#[test]
fn smart_filters_is_the_mood_modal_retitled() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjRowKey(Row::Smart));
    assert!(super::super::moods::dj_open(&gui));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("─ Smart filters · Any ─"), "the row's name and value on the border: {all}");
    assert!(all.contains("( ) Happy") && all.contains("[ ] Acoustic") && all.contains("(•) Standard"), "{all}");
    // Its own stops, its own law: a draft until it closes.
    click(&mut gui, &rows, "( ) Happy");
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "( ) Strict");
    let rows = draw(&mut gui);
    assert!(rows.join("\n").contains("─ Smart filters · Happy · Strict ─"));
    assert!(gui.app.dj.moods.is_empty(), "a draft");
    click(&mut gui, &rows, "[ Done ]");
    assert!(!super::super::moods::dj_open(&gui));
    let want = MoodFilter::NONE.press(MoodScale::Valence, "happy").with_level(MoodLevel::Strict);
    assert_eq!(gui.app.dj.moods, want);
    assert!(gui.prefs_unsaved.is_some());
    let rows = draw(&mut gui);
    assert!(room(&rows, row_of(&rows, "Smart filters")).ends_with("Happy · Strict ▸"));
    // It stands over another room too (quick config opens it from the bar).
    gui.act(Act::Nav(FILES_NAV));
    super::super::moods::open_for_dj(&mut gui, true);
    assert!(draw(&mut gui).join("\n").contains("─ Smart filters · Happy · Strict ─"));
    key(&mut gui, KeyCode::Esc);
    assert!(!super::super::moods::dj_open(&gui));
    // A server with no scores has no sheet to open.
    let mut bare = info("6.33.1");
    bare.mood_filters = false;
    probed(&mut gui, bare);
    super::super::moods::open_for_dj(&mut gui, true);
    assert!(!super::super::moods::dj_open(&gui));
}

// ── Quick config ────────────────────────────────────────────────────────────

#[test]
fn the_bars_button_splits_and_its_chevron_opens_quick_config() {
    let _en = english();
    let mut gui = session_gui();
    two_servers(&mut gui);
    let rows = draw(&mut gui);
    // One frame, a rule between its halves: the word toggles as it always
    // has, the chevron opens quick config.
    let (x, y) = find(&rows, "│ auto-dj │ ▴ │").expect("the split button");
    assert_eq!(x, super::super::bar::DJ_X);
    assert_eq!(cells(&rows[y as usize - 1], 4, 19), "╭─────────┬───╮");
    assert_eq!(gui.ui.hit(Position { x: x + 3, y }), Some(Act::AutoDj));
    assert_eq!(gui.ui.hit(Position { x: x + 12, y }), Some(Act::DjQuick(QuickAct::Toggle)));
    gui.pending.clear();
    gui.act(Act::DjQuick(QuickAct::Toggle));
    assert!(
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DjProbe { .. }))),
        "Play from's server is asked what it offers, as when the room opens"
    );
    probed(&mut gui, info("6.33.1"));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    // The canvas's "Quick config": rising from the chevron, clear of the
    // nav column, its last row the one over the bar.
    let (px, py) = find(&rows, "╭────────────────────────────────────────╮").expect("the popover");
    assert_eq!(px, 14);
    let (_, bottom) = find(&rows, "╰────────────────────────────────────────╯").expect("its last row");
    assert_eq!(bottom, 24, "never over the bar");
    assert!(py > 2);
    assert!(all.contains("PLAY FROM") && all.contains("(•) host:3000") && all.contains("( ) attic:3000"), "{all}");
    assert!(all.contains("[✓] Sonic similarity") && all.contains("55%"), "{all}");
    assert!(all.contains("(•) Drift") && all.contains("( ) Stay close"), "{all}");
    assert!(all.contains("Smart filters") && all.contains("All Auto DJ settings ▸"), "{all}");
    assert!(all.contains("│ auto-dj │ ▾ │"), "the chevron points down to it");
    // Every change applies at once: no draft, no Done.
    click(&mut gui, &rows, "( ) Stay close");
    assert_eq!(gui.app.dj.sonic_anchor, SonicAnchor::Locked);
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[✓] Sonic similarity");
    assert!(!gui.app.dj.sonic);
    let rows = draw(&mut gui);
    assert!(!rows.join("\n").contains("Stay close"), "the anchor goes with its switch");
    click(&mut gui, &rows, "( ) attic:3000");
    assert_eq!(gui.app.dj_home.as_deref(), Some(ATTIC));
    assert!(gui.dj.quick.is_some(), "it stays up");
    // Smart filters opens the sheet over whatever room this is.
    probed_as(&mut gui, ATTIC, info("6.33.1"));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Smart filters");
    assert!(gui.dj.quick.is_none() && super::super::moods::dj_open(&gui));
    key(&mut gui, KeyCode::Esc);
    // The way to the room.
    gui.act(Act::DjQuick(QuickAct::Toggle));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "All Auto DJ settings ▸");
    assert_eq!((gui.screen, gui.active), (Screen::Library, DJ_NAV));
    assert!(gui.dj.quick.is_none());
    // A click anywhere else, the chevron among them, puts it away; so
    // does Esc.
    gui.act(Act::DjQuick(QuickAct::Toggle));
    draw(&mut gui);
    assert_eq!(gui.ui.hit(Position { x: 2, y: 2 }), Some(Act::DjQuick(QuickAct::Close)));
    assert_eq!(gui.ui.hit(Position { x: x + 12, y }), Some(Act::DjQuick(QuickAct::Close)));
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.quick.is_none());
}

/// A server other than the session's answers its probe.
fn probed_as(gui: &mut Gui, identity: &str, info: DjServerInfo) {
    let effects = gui.app.apply_event(Event::DjProbed { identity: identity.into(), info: Some(info) });
    gui.pend(effects);
}

#[test]
fn quick_config_walks_by_the_keyboard_and_rests_what_the_server_cannot_do() {
    let _en = english();
    let mut gui = session_gui();
    gui.act(Act::DjQuick(QuickAct::Toggle));
    probed(&mut gui, info("6.33.1"));
    let all = draw(&mut gui).join("\n");
    assert!(!all.contains("PLAY FROM"), "one server: nothing to choose there: {all}");
    // The first walking key shows the hand; Space presses.
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Char(' '));
    assert!(!gui.app.dj.sonic, "the switch, at once");
    key(&mut gui, KeyCode::Char(' '));
    assert!(gui.app.dj.sonic);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Enter);
    assert_eq!(gui.app.dj.sonic_anchor, SonicAnchor::Locked);
    // An older server: the reasons, and nothing to press on those rows.
    let mut old = info("6.10.0");
    old.discovery = false;
    old.mood_filters = false;
    probed(&mut gui, old);
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    // The reason whole; where the two do not fit, the name gives way.
    assert!(all.contains("[ ] Sonic simil…  Needs server 6.15.2+"), "{all}");
    assert!(all.contains("Smart filters") && all.contains("No mood data"), "{all}");
    let y = row_of(&rows, "No mood data");
    assert_eq!(gui.ui.hit(Position { x: 17, y }), Some(Act::Guard), "a resting row takes no click");
    // On a screen with no bar it has nowhere to stand.
    gui.act(Act::Screen(Screen::NowPlaying));
    draw(&mut gui);
    assert!(gui.dj.quick.is_none());
}

// ── Every language ──────────────────────────────────────────────────────────

const LOCALES: [&str; 10] = ["en", "de", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];

#[test]
fn every_locale_draws_the_floor_whole() {
    let _lock = every_language();
    for locale in LOCALES {
        rust_i18n::set_locale(locale);
        let mut gui = room_gui();
        two_libraries(&mut gui);
        gui.app.dj.bpm = true;
        let rows = draw_sized(&mut gui, 100, 24);
        // Twelve settings, two headers and the title: no name clipped, and
        // every value clear of its name.
        for y in 2..19u16 {
            let row = room(&rows, y);
            assert!(!row.contains('…'), "{locale} row {y} is clipped: {row:?}");
        }
        let title = room(&rows, 2);
        assert!(title.trim_end().len() < title.len() || title.ends_with(' '), "{locale}: {title:?}");
        // The opening question and a sheet's foot fit their 66 cells.
        let mut gui = session_gui();
        gui.act(Act::AutoDj);
        recents(&mut gui);
        let all = squash(&draw(&mut gui).join("\n"));
        assert!(all.contains(&squash(&rust_i18n::t!("gui.dj.remember_short"))), "{locale}: {all}");
        let mut gui = room_gui();
        gui.act(Act::DjRowKey(Row::Length));
        type_text(&mut gui, "2:00");
        let all = squash(&draw(&mut gui).join("\n"));
        for key in ["gui.moods.clear", "gui.moods.done", "gui.dj.shortest", "gui.dj.include_unknown"] {
            assert!(all.contains(&squash(&rust_i18n::t!(key))), "{locale}: {key} is not whole:\n{all}");
        }
    }
    rust_i18n::set_locale("en");
}

#[test]
fn every_locales_help_and_tips_fit_their_cells() {
    let _lock = every_language();
    for locale in LOCALES {
        rust_i18n::set_locale(locale);
        for key in [
            "gui.dj.sonic_sub",
            "gui.dj.songs_per_fetch_sub",
            "gui.dj.bpm_sub",
            "gui.dj.cooldown_sub",
            "gui.dj.rating_sub",
            "gui.dj.length_help",
            "gui.dj.keyword_sub",
        ] {
            let lines = crate::kit::wrap_whole(&rust_i18n::t!(key), 62);
            assert!(lines.len() <= 4, "{locale}: {key} takes {} lines of a sheet", lines.len());
            assert!(lines.iter().all(|line| crate::kit::width(line) <= 62), "{locale}: {key}");
        }
        for key in [
            "gui.tips.dj_room",
            "gui.tips.dj_row",
            "gui.tips.dj_list",
            "gui.tips.dj_sheet",
            "gui.tips.dj_words",
            "gui.tips.dj_length",
            "gui.tips.dj_quick",
            "gui.tips.dj_genres",
            "gui.dj.start_keys",
        ] {
            let tip = rust_i18n::t!(key);
            assert!(crate::kit::width(&tip) <= 99, "{locale}: {key} is {} cells", crate::kit::width(&tip));
        }
        // A reason never loses its words: the few it has leave the row's
        // name a dozen cells at the least, in the room and in quick config.
        // (Quick config's sonic row has its switch before the name; its
        // Smart filters row keeps the chevron's two cells.)
        for (switch, reason) in [
            (4, rust_i18n::t!("gui.dj.needs_server", version = "6.15.2").to_string()),
            (4, rust_i18n::t!("gui.dj.still_scanning").to_string()),
            (4, rust_i18n::t!("gui.dj.discovery_off").to_string()),
            (2, rust_i18n::t!("gui.dj.no_mood_data").to_string()),
            (2, rust_i18n::t!("gui.dj.not_on_peers").to_string()),
        ] {
            let w = crate::kit::width(&reason);
            assert!(w + 4 + 12 <= 47, "{locale}: {reason:?} is {w} cells of a row's 47");
            assert!(w + switch + 2 + 8 <= 38, "{locale}: {reason:?} is {w} cells of quick config's 38");
        }
    }
    rust_i18n::set_locale("en");
}
