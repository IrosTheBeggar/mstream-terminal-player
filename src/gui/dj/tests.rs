//! The Auto DJ room's tests: the room's rows and its floor, the gates, the
//! sheets and their draft law, First song's list, the opening question and
//! Recently played, the empty queue's openers, and quick config.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use super::super::{Act, DJ_NAV, FILES_NAV, Gui, List, RowVerb, SEARCH_NAV, Screen, render};
use super::{Item, Layout, QuickAct, Row, layout};
use crate::api::types::{
    Genre, GenreSource, GenreSources, MoodFilter, MoodLevel, MoodScale, Ping, SonicReport, Track, TrackMetadata,
};
use crate::config::{self, Config};
use crate::dj::{self, EmptyQueueStart, GenreMode, SonicAnchor};
use crate::kit::theme::th;
use crate::tui::app::{
    App, Capture, DjEdit, DjMark, Effect, Entry, KnownServer, Origin, Queued, SeedFrom, SeedKey, SeedList,
};
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
        auto_dj: Some(1),
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

/// A seed of the room's server.
fn seed_key(path: &str) -> SeedKey {
    SeedKey::new(&Origin { server: HOST.into(), peer: None }, path)
}

/// The lists the A boards drew, newest first: More like — SkyWalk
/// (pinned), Crystalstair, Fayz, Better Days (pinned), Looking Back; Less
/// like — Circles, Heartbeat (pinned). Built with the model's own moves:
/// the DJ's picks, the user's adds, a learned skip.
fn a_board_lists(gui: &mut Gui) {
    let now = crate::clock::Instant::now();
    let seeds = &mut gui.app.seeds;
    let (b, c) = ("Boukmanflow", "Color Out");
    let pick = |seeds: &mut crate::tui::app::Seeds, path: &str, title: &str, artist: &str| {
        seeds.push_pick(seed_key(path), song(path, title, artist), SeedFrom::Pick, now);
    };
    let add = |seeds: &mut crate::tui::app::Seeds, list: SeedList, path: &str, title: &str, artist: &str| {
        seeds.add_manual(list, seed_key(path), song(path, title, artist), now);
    };
    pick(seeds, "lib/looking-back.mp3", "Looking Back", b);
    add(seeds, SeedList::MoreLike, "lib/better-days.mp3", "Better Days", b);
    pick(seeds, "lib/fayz.mp3", "Fayz", "ALM");
    pick(seeds, "lib/crystalstair.mp3", "Crystalstair", b);
    add(seeds, SeedList::MoreLike, "lib/skywalk.mp3", "SkyWalk", b);
    add(seeds, SeedList::LessLike, "lib/heartbeat.mp3", "Heartbeat", c);
    seeds.quick_skip(&seed_key("lib/circles.mp3"), &song("lib/circles.mp3", "Circles", c), true, now);
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
    key(&mut gui, KeyCode::Esc);
    // The Seeds sheet as the A boards drew it: Drift, a row's menu, Stay
    // close.
    a_board_lists(&mut gui);
    gui.act(Act::DjRowKey(Row::Seeds));
    println!("{}", draw(&mut gui).join("\n"));
    let rows = draw(&mut gui);
    let (x, y) = find(&rows, "Looking Back").expect("the row");
    let menu = gui.ui.hit(Position { x: x + 57, y }).expect("its [⋯]");
    gui.act(menu);
    println!("{}", draw(&mut gui).join("\n"));
    key(&mut gui, KeyCode::Esc);
    super::edit(&mut gui, DjEdit::SeedMode(SonicAnchor::Locked));
    println!("{}", draw(&mut gui).join("\n"));
    println!("{}", draw_sized(&mut gui, 100, 24).join("\n"));
}

// ── The room ────────────────────────────────────────────────────────────────

/// E-AutoDJ, the room's cells 17–63 on rows 2–24 at 100×30 (the DJ armed,
/// as the board drew it).
const E_AUTODJ: [&str; 23] = [
    "Auto DJ                         Preview picks ▸",
    "╭─────────────────────────────────────────────╮",
    "│                    Stop                     │",
    "╰─────────────────────────────────────────────╯",
    "First song                                Ask ▾",
    "Batch                                 4 songs ▸",
    "Artist cooldown                     3 artists ▸",
    "                                               ",
    "FILTERS                                        ",
    "Smart filters                             Any ▸",
    "Genres                                    Any ▸",
    "Rating                                    Any ▸",
    "Length                                    Any ▸",
    "Skip words                                Off ▸",
    "                                               ",
    "MATCH                                          ",
    "Sonic similarity                          55% ▸",
    "  Learn from skips                        [ ]  ",
    "    Skipping a song will prevent similar songs ",
    "    from being selected                        ",
    "  Seeds                                       ▸",
    "Beat matching                             Off ▸",
    "Harmonic mixing                           [ ]  ",
];

/// Whether a cell is drawn in the TEXT colour: the terminal's own, or the
/// theme's where it owns the ground.
fn is_text(color: ratatui::style::Color) -> bool {
    color == ratatui::style::Color::Reset || color == th().text
}

/// The frame drawn at 100×30, with its styles.
fn buffer(gui: &mut Gui) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| render(frame, gui)).unwrap();
    terminal.backend().buffer().clone()
}

fn bold(buffer: &ratatui::buffer::Buffer, x: u16, y: u16) -> bool {
    buffer[(x, y)].modifier.contains(ratatui::style::Modifier::BOLD)
}

#[test]
fn the_room_is_one_row_a_setting_on_its_twenty_three_rows() {
    let _en = english();
    let mut gui = room_gui();
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::DjStartStop);
    let rows = draw(&mut gui);
    // E-AutoDJ: the name and Preview, the button, how the session runs,
    // FILTERS and MATCH — Learn from skips and Seeds indented under Sonic
    // similarity, Learn's line under its name — the room's 23 rows whole,
    // the last on its last. No Harmonic line: the room needs every row.
    for (i, want) in E_AUTODJ.iter().enumerate() {
        let y = 2 + i as u16;
        assert_eq!(room(&rows, y), *want, "row {y}:\n{}", rows.join("\n"));
    }
    // Its colours: the sub-rows' names TEXT two cells in, the switch and the
    // chevron DIM, Learn's line DIM four cells in, the headers DIM.
    let b = buffer(&mut gui);
    assert!(is_text(b[(19, 19)].fg) && is_text(b[(19, 22)].fg), "the sub-rows' names");
    assert_eq!((b[(59, 19)].fg, b[(21, 20)].fg, b[(21, 21)].fg, b[(63, 22)].fg), (th().dim, th().dim, th().dim, th().dim));
    assert_eq!((b[(17, 10)].fg, b[(17, 17)].fg), (th().dim, th().dim), "FILTERS and MATCH");
    assert_eq!(b[(59, 18)].fg, th().ok, "55%");
    // Each row takes its click across its own cells; Learn's line takes none.
    assert_eq!(gui.ui.hit(Position { x: 17, y: 19 }), Some(Act::DjRow(Row::Learn)));
    assert_eq!(gui.ui.hit(Position { x: 63, y: 22 }), Some(Act::DjRow(Row::Seeds)));
    assert_eq!(gui.ui.hit(Position { x: 30, y: 20 }), None, "the line is no control");

    // With Play from: Learn's line, the frame and one blank row — the one
    // before FILTERS — on the same 23 rows.
    two_libraries(&mut gui);
    let rows = draw(&mut gui);
    let want = [
        (6, "Play from", "host:3000 ▸"),
        (7, "First song", "Ask ▾"),
        (8, "Batch", "4 songs ▸"),
        (9, "Artist cooldown", "3 artists ▸"),
        (12, "Smart filters", "Any ▸"),
        (16, "Skip words", "Off ▸"),
        (18, "Sonic similarity", "55% ▸"),
        (19, "  Learn from skips", "[ ]  "),
        (22, "  Seeds", "▸"),
        (23, "Beat matching", "Off ▸"),
        (24, "Harmonic mixing", "[ ]  "),
    ];
    for (y, label, tail) in want {
        let row = room(&rows, y);
        assert!(row.starts_with(label) && row.ends_with(tail), "row {y}: {row:?}\n{}", rows.join("\n"));
    }
    assert_eq!(room(&rows, 3), format!("╭{}╮", "─".repeat(45)), "the frame, no air over it");
    assert_eq!((room(&rows, 10).trim(), room(&rows, 11).trim_end()), ("", "FILTERS"));
    assert_eq!(room(&rows, 17).trim_end(), "MATCH", "no air before it");
    assert_eq!(room(&rows, 20).trim_end(), "    Skipping a song will prevent similar songs");
    // Every value ends two cells before the chevron's, whatever its length.
    assert_eq!(cells(&rows[7], 59, 64), "Ask ▾");
    assert_eq!(cells(&rows[9], 53, 64), "3 artists ▸");
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
    assert_eq!(cells(&rows[3], 17, 64), format!("╭{}╮", "─".repeat(45)));
    // 106 columns: a room of 53 cells, and the block is all of it.
    let rows = draw_sized(&mut gui, 106, 30);
    assert_eq!(cells(&rows[3], 17, 70), format!("╭{}╮", "─".repeat(51)), "{}", rows.join("\n"));
    let batch = cells(&rows[row_of(&rows, "Batch") as usize], 17, 70);
    assert!(batch.starts_with("Batch") && batch.ends_with("4 songs ▸"), "{batch:?}");
    // 140 columns: a room of 87 cells. The block stops at 56 — a fifth past
    // 47 — and stands in the middle, 15 cells in and 16 from the far edge.
    let rows = draw_sized(&mut gui, 140, 30);
    let all = rows.join("\n");
    let x = 17 + 15;
    assert_eq!(cells(&rows[3], x, x + 56), format!("╭{}╮", "─".repeat(54)), "{all}");
    assert_eq!(cells(&rows[3], 17, x).trim(), "", "nothing at the room's left: {all}");
    let title = cells(&rows[2], x, x + 56);
    assert!(title.starts_with("Auto DJ") && title.ends_with("Preview picks ▸"), "{title:?}");
    for (label, tail) in [("Play from", "host:3000 ▸"), ("Sonic similarity", "55% ▸"), ("Artist cooldown", "3 artists ▸")] {
        let row = cells(&rows[row_of(&rows, label) as usize], x, x + 56);
        assert!(row.starts_with(label) && row.ends_with(tail), "{row:?}");
    }
    assert_eq!(cells(&rows[row_of(&rows, "FILTERS") as usize], x, x + 7), "FILTERS");
    // The sub-rows keep their indent and their columns in the wider block.
    let seeds = cells(&rows[row_of(&rows, "  Seeds") as usize], x, x + 56);
    assert!(seeds.starts_with("  Seeds ") && seeds.ends_with('▸'), "{seeds:?}");
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
    assert_eq!(cells(&rows[3], 29, 85), format!("╭{}╮", "─".repeat(54)), "{}", rows.join("\n"));
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
    // The canvas's "Minimum": Start folds to the slab on the title row, the
    // air and Learn's line go, and all fourteen settings stand on the
    // room's 17 rows, the headers with them.
    assert_eq!(room(&rows, 2), "Auto DJ              Preview picks ▸   Start ▸ ");
    assert_eq!(hit_text(&gui, &rows, " Start ▸ "), Some(Act::DjStartStop), "the slab is the button");
    let labels = [
        "Play from", "First song", "Batch", "Artist cooldown", "FILTERS", "Smart filters", "Genres", "Rating", "Length",
        "Skip words", "MATCH", "Sonic similarity", "  Learn from skips", "  Seeds", "Beat matching", "Harmonic mixing",
    ];
    for (i, label) in labels.iter().enumerate() {
        let row = room(&rows, 3 + i as u16);
        assert!(row.starts_with(label), "row {}: {row:?}\n{}", 3 + i, rows.join("\n"));
    }
    assert!(!rows.join("\n").contains("Skipping a song"), "no room for the line");
    // Without Play from the one row to spare is the blank before FILTERS.
    probed(&mut gui, info("6.33.1"));
    let rows = draw_sized(&mut gui, 100, 24);
    let labels = [
        "First song", "Batch", "Artist cooldown", "", "FILTERS", "Smart filters", "Genres", "Rating", "Length",
        "Skip words", "MATCH", "Sonic similarity", "  Learn from skips", "  Seeds", "Beat matching", "Harmonic mixing",
    ];
    for (i, label) in labels.iter().enumerate() {
        let row = room(&rows, 3 + i as u16);
        assert!(if label.is_empty() { row.trim().is_empty() } else { row.starts_with(label) }, "row {}: {row:?}", 3 + i);
    }
    // Armed, the slab is Stop.
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::DjStartStop);
    assert!(room(&draw_sized(&mut gui, 100, 24), 2).ends_with(" Stop "));
}

/// Where even the rows cannot all stand — the floor with the tips line and
/// Play from, sixteen rows — the two headers fold first, and their gaps
/// with them: every setting still stands, Harmonic mixing on the last.
#[test]
fn the_headers_fold_where_even_the_rows_cannot_stand() {
    let _en = english();
    let mut gui = room_gui();
    two_libraries(&mut gui);
    gui.config.gui.key_hints = true;
    let rows = draw_sized(&mut gui, 100, 24);
    let all = rows.join("\n");
    assert!(!all.contains("FILTERS") && !all.contains("MATCH"), "{all}");
    let labels = [
        "Play from", "First song", "Batch", "Artist cooldown", "Smart filters", "Genres", "Rating", "Length",
        "Skip words", "Sonic similarity", "  Learn from skips", "  Seeds", "Beat matching", "Harmonic mixing",
    ];
    for (i, label) in labels.iter().enumerate() {
        let row = room(&rows, 3 + i as u16);
        assert!(row.starts_with(label), "row {}: {row:?}\n{all}", 3 + i);
    }
    assert_eq!(gui.ui.hit(Position { x: 30, y: 16 }), Some(Act::DjRow(Row::Harmonic)), "and it takes its click");
}

#[test]
fn the_rows_the_height_affords_come_back_in_one_order() {
    // Plan D28's table — English, so Learn from skips' line is two rows,
    // and no Play from: the spare rows come back by priority, each whole or
    // not at all, skipping what does not fit — Learn's line, the frame, the
    // blank before FILTERS, then before MATCH, a blank under the frame,
    // then over it.
    let flags = |l: Layout| [l.learn_lines == 2, l.tall, l.gap_filters, l.gap_match, l.blank_button, l.blank_title];
    let table: [(u16, [bool; 6]); 10] = [
        (16, [false, false, false, false, false, false]),
        (17, [false, false, true, false, false, false]),
        (18, [true, false, false, false, false, false]),
        (19, [true, false, true, false, false, false]),
        (20, [true, false, true, true, false, false]),
        (21, [true, true, false, false, false, false]),
        (22, [true, true, true, false, false, false]),
        (23, [true, true, true, true, false, false]),
        (24, [true, true, true, true, true, false]),
        (25, [true, true, true, true, true, true]),
    ];
    for (rows, want) in table {
        let l = layout(rows, false, 2);
        assert_eq!(flags(l), want, "{rows} rows: {l:?}");
        assert!(l.headers, "{rows} rows: the headers stand");
    }
    assert_eq!(layout(40, false, 2), layout(25, false, 2), "a taller room adds nothing: it never scrolls, and never stretches");
    // With Play from every step comes a row later: at 100×30 it keeps
    // Learn's line, the frame and one blank row.
    assert_eq!(layout(17, true, 2), Layout { headers: true, ..Layout::default() });
    assert_eq!(flags(layout(23, true, 2)), [true, true, true, false, false, false]);
    // A one-line Learn — ja, zh, or the reason — comes back sooner.
    assert_eq!(layout(17, false, 1), Layout { headers: true, learn_lines: 1, ..Layout::default() });
    assert_eq!(layout(20, false, 1), Layout { headers: true, learn_lines: 1, tall: true, ..Layout::default() });
    // Where even the rows cannot all stand — 100×24 with the tips line and
    // Play from — the headers fold first, and their gaps with them.
    let folded = layout(16, true, 2);
    assert!(!folded.headers && !folded.gap_filters && !folded.gap_match && !folded.tall, "{folded:?}");
    assert_eq!(layout(16, true, 1).learn_lines, 1, "the one row to spare is a one-line Learn's");
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
            auto_dj: Some(0),
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
    // The rows did not move: the same lines as on a server that can, E-AutoDJ's.
    assert_eq!((row_of(&rows, "Artist cooldown"), row_of(&rows, "  Seeds"), row_of(&rows, "Harmonic mixing")), (8, 22, 24));
    // DIM whole, and nothing to press.
    let y = row_of(&rows, "Genres");
    assert_eq!(fg_at(&mut gui, 17, y), th().dim, "the name");
    assert!(gui.ui.hit(Position { x: 17, y }).is_none(), "no click on a row the server can't do");
    gui.act(Act::DjRowKey(Row::Genres));
    assert!(gui.dj.sheet.is_none(), "and no sheet");
    // Sonic similarity blocked, its sub-rows rest under it, DIM with no
    // reason of their own — Learn's usual line: the server's word about
    // negatives counts only once sonic can run (clause 59).
    assert!(line("  Learn from skips").ends_with("[ ]") && line("  Seeds").ends_with('▸'));
    assert!(rows[20].contains("Skipping a song will prevent similar songs"), "{}", rows[20]);
    assert!(!rows.join("\n").contains("Needs a newer server"));
    assert_eq!((fg_at(&mut gui, 19, 19), fg_at(&mut gui, 19, 22)), (th().dim, th().dim));
    assert!(gui.ui.hit(Position { x: 19, y: 22 }).is_none());
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
    assert!(!seen.contains(&Some(Item::Row(Row::Learn))) && !seen.contains(&Some(Item::Row(Row::Seeds))));

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
    assert_eq!(fg_at(&mut gui, 17, 3), th().accent, "Start in the accent");
    gui.act(Act::DjStartStop);
    assert_eq!(gui.app.dj_server.as_deref(), Some(HOST), "started through the toggle");
    let rows = draw(&mut gui);
    assert_eq!(room(&rows, 4).trim_matches(['│', ' ']), "Stop");
    assert_eq!(fg_at(&mut gui, 17, 3), th().ok, "Stop lit, never the destructive colour");
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
    // ↑ from rest takes the last row, Harmonic mixing: Space flips its
    // switch; up the column, Beat matching, then Sonic similarity's two.
    key(&mut gui, KeyCode::Up);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Harmonic)));
    key(&mut gui, KeyCode::Char(' '));
    assert!(gui.app.dj.harmonic);
    let mut walked = Vec::new();
    for _ in 0..4 {
        key(&mut gui, KeyCode::Up);
        walked.push(gui.dj.cursor);
    }
    let want = [Row::Beat, Row::Seeds, Row::Learn, Row::Sonic].map(|row| Some(Item::Row(row)));
    assert_eq!(walked, want, "the sub-rows are stops of their own");
    for _ in 0..4 {
        key(&mut gui, KeyCode::Down);
    }
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
    // E-Sonic: the sheet over the room, rows 10 to 19 — 66 cells, the row's
    // name and its value on the top border, the switch, the bar, the old
    // room's help line inside, and [ Done ] at the foot — no [X], and no
    // Drift · Stay close: the mode is the Seeds sheet's now.
    let (x, y) = find(&rows, "╭─ Sonic similarity · 55% ─").expect("the title and the value on the border");
    assert_eq!((x, y), (17, 10));
    assert_eq!(cells(&rows[y as usize], 17, 83).chars().count(), 66);
    assert_eq!(cells(&rows[y as usize], 82, 83), "╮");
    assert_eq!(cells(&rows[11], 19, 25), "[✓] On");
    assert_eq!(cells(&rows[13], 19, 54), "- ▰▰▰▰▰▰▰▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱ +   55%");
    assert_eq!(cells(&rows[15], 19, 73).trim_end(), "Only pick songs that sound like the session, using the");
    assert_eq!(cells(&rows[16], 19, 73).trim_end(), "server's audio analysis.");
    assert_eq!(cells(&rows[18], 73, 81), "[ Done ]");
    assert!(cells(&rows[19], 17, 18) == "╰" && rows[20].contains("Skipping a song"), "ten rows, the room under");
    for blank in [12, 14, 17] {
        assert_eq!(cells(&rows[blank], 18, 82).trim(), "", "row {blank}");
    }
    assert!(!all.contains("Drift") && !all.contains("Stay close"), "{all}");
    assert!(all.contains("[ Done ]") && !all.contains("[X]"));
    assert!(!all.contains("clear"), "nothing here to clear");
    // Two points a cell across the band: 55% lights thirteen of 25.
    let bar = rows.iter().find(|r| r.contains("▰")).expect("the bar");
    assert_eq!((bar.matches('▰').count(), bar.matches('▱').count()), (13, 12), "{bar}");
    // ↓ from the bar goes to the foot: no stops are left between.
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Down);
    assert!(format!("{:?}", gui.dj.sheet.as_ref().unwrap()).contains("hand: Done"));
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Up);
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

    // By the pointer: the switch back on, a cell of the bar, Done. The mode
    // rides through as it stands.
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
    super::edit(&mut gui, DjEdit::SeedMode(SonicAnchor::Locked));
    click(&mut gui, &rows, "[ Done ]");
    assert!(gui.dj.sheet.is_none());
    assert!(gui.app.dj.sonic);
    assert_eq!(gui.app.dj.sonic_min_similarity, 0.7);
    assert_eq!(gui.app.dj.sonic_anchor, SonicAnchor::Locked, "the sheet leaves the mode as it found it");
    assert!(gui.app.seeds.frozen());

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
    assert!(all.contains("▰▰▰▰▰▰▰▰▰▰▰▰▰▱") && all.contains("│     Seeds"), "the match bar and the Seeds row: {all}");
    assert!(!all.contains("Drift") && !all.contains("Stay close"), "the mode is the Seeds sheet's: {all}");
    assert!(all.contains("Smart filters") && all.contains("All Auto DJ settings ▸"), "{all}");
    assert!(all.contains("│ auto-dj │ ▾ │"), "the chevron points down to it");
    // Every change applies at once: no draft, no Done.
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[✓] Sonic similarity");
    assert!(!gui.app.dj.sonic);
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(!all.contains("│     - ") && !all.contains("Seeds"), "the bar and Seeds go with its switch: {all}");
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
    // ↓ to the match bar: ← → step it a point, at once; ↓ again walks on.
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Left);
    assert_eq!(gui.app.dj.sonic_min_similarity, 0.56);
    assert!(gui.prefs_unsaved.is_some(), "saved once the steps rest");
    // Seeds, under it: Enter opens the Seeds sheet, its hand up, and quick
    // config goes.
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Enter);
    assert!(gui.dj.quick.is_none() && gui.dj.seeds.is_some());
    assert!(format!("{:?}", gui.dj.seeds).contains("shown: true"), "{:?}", gui.dj.seeds);
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.seeds.is_none());
    gui.act(Act::DjQuick(QuickAct::Toggle));
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

/// E-Quick: quick config's cells 14–55 on rows 16–24, sonic on.
const E_QUICK: [&str; 9] = [
    "╭────────────────────────────────────────╮",
    "│ [✓] Sonic similarity               55% │",
    "│     - ▰▰▰▰▰▰▰▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱ +      │",
    "│     Seeds                            ▸ │",
    "│                                        │",
    "│ Smart filters                    Any ▸ │",
    "│                                        │",
    "│ All Auto DJ settings ▸                 │",
    "╰────────────────────────────────────────╯",
];

#[test]
fn quick_config_wears_the_match_bar_and_the_way_to_the_seeds() {
    let _en = english();
    let mut gui = room_gui();
    gui.act(Act::DjQuick(QuickAct::Toggle));
    let rows = draw(&mut gui);
    // E-Quick: the match bar and Seeds under Sonic similarity, where Drift
    // · Stay close stood; the popover a row taller, its foot still on row 24.
    for (i, want) in E_QUICK.iter().enumerate() {
        let y = 16 + i;
        assert_eq!(cells(&rows[y], 14, 56), *want, "row {y}:\n{}", rows.join("\n"));
    }
    let b = buffer(&mut gui);
    assert_eq!((b[(20, 18)].fg, b[(48, 18)].fg), (th().dim, th().dim), "the bar's ends");
    assert_eq!((b[(22, 18)].fg, b[(34, 18)].fg, b[(35, 18)].fg), (th().accent, th().accent, th().dim), "13 lit of 25");
    assert!(is_text(b[(20, 19)].fg) && b[(53, 19)].fg == th().dim, "Seeds TEXT, its chevron DIM");
    // The pointer's: a cell sets, the ends step a point — at once, and the
    // popover stays.
    assert_eq!(gui.ui.hit(Position { x: 22 + 19, y: 18 }), Some(Act::DjQuick(QuickAct::Set(70))));
    gui.act(Act::DjQuick(QuickAct::Set(70)));
    assert_eq!(gui.app.dj.sonic_min_similarity, 0.7);
    assert!(gui.prefs_unsaved.is_some() && gui.dj.quick.is_some());
    draw(&mut gui);
    assert_eq!(gui.ui.hit(Position { x: 20, y: 18 }), Some(Act::DjQuick(QuickAct::Step(-1))));
    assert_eq!(gui.ui.hit(Position { x: 48, y: 18 }), Some(Act::DjQuick(QuickAct::Step(1))));
    gui.act(Act::DjQuick(QuickAct::Step(-1)));
    assert_eq!(gui.app.dj.sonic_min_similarity, 0.69);
    for _ in 0..40 {
        gui.act(Act::DjQuick(QuickAct::Step(1)));
    }
    assert_eq!(gui.app.dj.sonic_min_similarity, 0.8, "held inside its band");
    assert!(draw(&mut gui).join("\n").contains("80% │"));
    // Seeds opens the Seeds sheet — quick config goes, the hand stowed.
    draw(&mut gui);
    assert_eq!(gui.ui.hit(Position { x: 20, y: 19 }), Some(Act::DjQuick(QuickAct::Press(super::quick::QStop::Seeds))));
    assert_eq!(gui.ui.hit(Position { x: 53, y: 19 }), gui.ui.hit(Position { x: 20, y: 19 }), "the row's whole width");
    gui.act(Act::DjQuick(QuickAct::Press(super::quick::QStop::Seeds)));
    assert!(gui.dj.quick.is_none() && gui.dj.seeds.is_some());
    assert!(format!("{:?}", gui.dj.seeds).contains("shown: false"));
    key(&mut gui, KeyCode::Esc);
    // Sonic off, or blocked: the bar and Seeds are not drawn. (The bar's
    // own volume cells are the only ▰ then.)
    gui.app.dj.sonic = false;
    gui.act(Act::DjQuick(QuickAct::Toggle));
    let all = draw(&mut gui).join("\n");
    assert!(all.contains("[ ] Sonic similarity") && !all.contains("│     - ") && !all.contains("Seeds"), "{all}");
    gui.app.dj.sonic = true;
    let mut off = info("6.33.1");
    off.discovery = false;
    probed(&mut gui, off);
    let all = draw(&mut gui).join("\n");
    assert!(all.contains("Discovery off") && !all.contains("│     - ") && !all.contains("Seeds"), "{all}");
}

// ── Sonic similarity's sub-rows ─────────────────────────────────────────────

/// E-SonicOff: sonic switched off — or blocked — its two sub-rows rest in
/// place, DIM whole: no click, no stop, no reason; the switch keeps its
/// state's glyph.
#[test]
fn sonic_off_rests_its_two_sub_rows_in_place() {
    let _en = english();
    let mut gui = room_gui();
    super::edit(&mut gui, DjEdit::LearnSkips(true));
    super::edit(&mut gui, DjEdit::Sonic { on: false, floor: 55, anchor: SonicAnchor::Rolling });
    let rows = draw(&mut gui);
    assert_eq!(room(&rows, 18), "Sonic similarity                          Off ▸");
    assert_eq!(room(&rows, 19), "  Learn from skips                        [✓]  ");
    for y in 20..=22u16 {
        assert_eq!(room(&rows, y), E_AUTODJ[y as usize - 2], "row {y}");
    }
    let b = buffer(&mut gui);
    assert!(is_text(b[(17, 18)].fg) && b[(59, 18)].fg == th().dim, "Sonic's name live, its Off DIM");
    for (x, y) in [(19, 19), (59, 19), (21, 20), (21, 21), (19, 22), (63, 22)] {
        assert_eq!(b[(x, y)].fg, th().dim, "({x}, {y}) rests");
    }
    for y in 19..=22u16 {
        assert_eq!(gui.ui.hit(Position { x: 30, y }), None, "row {y} takes no click");
    }
    assert_eq!(gui.ui.hit(Position { x: 30, y: 18 }), Some(Act::DjRow(Row::Sonic)), "its sheet is how it comes back on");
    // No stop: from Harmonic up, the hand passes from Beat to Sonic.
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Up);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Sonic)));
    // Pressed anyway, nothing.
    gui.act(Act::DjRowKey(Row::Seeds));
    gui.act(Act::DjRowKey(Row::Learn));
    assert!(gui.dj.seeds.is_none() && gui.app.dj.learn_from_skips);
    // Blocked rests them the same way.
    super::edit(&mut gui, DjEdit::Sonic { on: true, floor: 55, anchor: SonicAnchor::Rolling });
    let mut off = info("6.33.1");
    off.discovery = false;
    probed(&mut gui, off);
    let rows = draw(&mut gui);
    assert!(room(&rows, 18).trim_end().ends_with("Discovery off"), "{}", room(&rows, 18));
    assert_eq!((fg_at(&mut gui, 19, 22), gui.ui.hit(Position { x: 30, y: 22 })), (th().dim, None));
    // Back on, they answer again.
    probed(&mut gui, info("6.33.1"));
    draw(&mut gui);
    assert_eq!(gui.ui.hit(Position { x: 30, y: 22 }), Some(Act::DjRow(Row::Seeds)));
}

/// Learn from skips answers only where the server takes negatives, and
/// says "Needs a newer server" in place of its line only once a probe has
/// said the server predates them — never while nobody knows (decision
/// 30, clause 59): in the room, and on the Seeds sheet's LESS LIKE.
#[test]
fn learn_from_skips_says_why_only_once_the_server_has_said_so() {
    let _en = english();
    let reason = |gui: &mut Gui| draw(gui).join("\n").contains("Needs a newer server");
    // Nobody knows yet: the DJ off, Play from's server not answering.
    let mut gui = session_gui();
    gui.act(Act::Nav(DJ_NAV));
    assert!(gui.app.dj_info.is_none());
    let rows = draw(&mut gui);
    assert_eq!(room(&rows, 19), "  Learn from skips                        [ ]  ");
    assert_eq!(room(&rows, 20), E_AUTODJ[18], "its usual line");
    assert!(!reason(&mut gui));
    assert_eq!((fg_at(&mut gui, 19, 19), gui.ui.hit(Position { x: 30, y: 19 })), (th().dim, None), "at rest");
    assert_eq!(gui.ui.hit(Position { x: 30, y: 22 }), Some(Act::DjRow(Row::Seeds)), "the seeds answer");
    gui.act(Act::DjRowKey(Row::Seeds));
    let rows = draw(&mut gui);
    assert_eq!(cells(&rows[row_of(&rows, "LESS LIKE") as usize], 19, 81).trim_end(), "LESS LIKE", "no reason");
    key(&mut gui, KeyCode::Esc);
    // The provisional info: the DJ armed on the session's own server, its
    // probe still out.
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::DjStartStop);
    assert!(gui.app.dj_info.as_ref().is_some_and(|info| info.auto_dj.is_none()));
    let rows = draw(&mut gui);
    assert_eq!(room(&rows, 20), E_AUTODJ[18]);
    assert!(!reason(&mut gui) && gui.ui.hit(Position { x: 30, y: 19 }).is_none());
    // A probe that says autoDj 0: the reason in place of the line — one
    // row, so the room gives the row back above — and Learn still rests.
    probed(&mut gui, DjServerInfo { auto_dj: Some(0), ..info("6.33.1") });
    let rows = draw(&mut gui);
    let y = row_of(&rows, "Learn from skips");
    assert_eq!(room(&rows, y + 1).trim_end(), "    Needs a newer server");
    assert!(room(&rows, y + 2).starts_with("  Seeds") && !rows.join("\n").contains("Skipping a song"));
    assert_eq!((fg_at(&mut gui, 21, y + 1), gui.ui.hit(Position { x: 30, y })), (th().dim, None));
    assert_eq!(room(&rows, 24), E_AUTODJ[22], "the rows still end on the room's last");
    gui.act(Act::DjRowKey(Row::Seeds));
    let rows = draw(&mut gui);
    let head = row_of(&rows, "LESS LIKE") as usize;
    assert_eq!(cells(&rows[head], 19, 81), format!("LESS LIKE{:>53}", "Needs a newer server"), "on its header, at the row's end");
    key(&mut gui, KeyCode::Esc);
    // And a server that takes them: Learn answers, under its usual line.
    probed(&mut gui, info("6.33.1"));
    assert!(!reason(&mut gui));
    assert_eq!(gui.ui.hit(Position { x: 30, y: 19 }), Some(Act::DjRow(Row::Learn)));
}

#[test]
fn learn_from_skips_is_a_switch_by_click_and_by_space() {
    let _en = english();
    let mut gui = room_gui();
    assert!(!gui.app.dj.learn_from_skips, "off by default");
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Learn from skips");
    assert!(gui.app.dj.learn_from_skips);
    assert!(gui.prefs_unsaved.is_some(), "saved once the edits rest");
    let rows = draw(&mut gui);
    assert!(room(&rows, 19).ends_with("[✓]  "));
    assert_eq!(fg_at(&mut gui, 59, 19), th().ok);
    assert!(gui.dj.cursor.is_none(), "a click seats no hand");
    // By the keyboard: up from Harmonic to it, Space flips it — the
    // switch's tips while the hand is there.
    gui.prefs_unsaved = None;
    for _ in 0..4 {
        key(&mut gui, KeyCode::Up);
    }
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Learn)));
    assert_eq!(super::tips(&gui).as_deref(), Some("Space toggle · ↑ ↓ rows · Esc done"));
    key(&mut gui, KeyCode::Char(' '));
    assert!(!gui.app.dj.learn_from_skips && gui.prefs_unsaved.is_some());
    let b = buffer(&mut gui);
    assert!((17..64).all(|x| b[(x, 19)].bg == th().accent), "the hand's slab, the row's width");
}

/// The probes of the room's server among `effects`.
fn probes_of(effects: &[Effect]) -> usize {
    effects.iter().filter(|e| matches!(e, Effect::Api(ApiCmd::DjProbe { identity, .. }) if identity == HOST)).count()
}

/// Stopped by `A` or by the bar's word as by the room's own Stop: off, the
/// rows describe Play from's server again, and it is asked what it offers —
/// else Learn from skips and Less like rested until the room was entered
/// again. While it is asked, the hand on Learn stands on Sonic similarity.
#[test]
fn a_stop_by_a_or_the_bars_word_asks_play_froms_server_again() {
    let _en = english();
    for how in ["A", "the bar's word"] {
        let mut gui = room_gui();
        gui.app.queue.items = vec![queued("a.mp3", None)];
        gui.act(Act::DjStartStop);
        probed(&mut gui, info("6.33.1"));
        // The hand on Learn from skips: up four from rest.
        for _ in 0..4 {
            key(&mut gui, KeyCode::Up);
        }
        assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Learn)), "{how}");
        gui.pending.clear();
        if how == "A" {
            key(&mut gui, KeyCode::Char('A'));
        } else {
            let rows = draw(&mut gui);
            click(&mut gui, &rows, "auto-dj");
        }
        assert!(!gui.app.dj_armed(), "{how} stops");
        assert_eq!(probes_of(&gui.pending), 1, "{how}: Play from's server is asked: {:?}", gui.pending);
        draw(&mut gui);
        assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Sonic)), "{how}: Learn rests while it is asked");
        // It answers: Learn from skips, and Less like's Add song, answer.
        probed(&mut gui, info("6.33.1"));
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Learn from skips"), Some(Act::DjRow(Row::Learn)), "{how}");
        gui.act(Act::DjRowKey(Row::Seeds));
        let rows = draw(&mut gui);
        let y = row_of(&rows, "LESS LIKE") + 1;
        let (x, _) = find(&rows, "+ Add song").expect("Add song");
        assert_eq!(gui.ui.hit(Position { x, y }), Some(add(SeedList::LessLike)), "{how}");
    }
}

/// One pass of the loop's frame half with `event` in from the workers, as
/// a session's answer comes: what it sent the API worker. What the test
/// left pending before it (the room's opening probe, answered by hand) is
/// not the pass's.
fn frame_with(gui: &mut Gui, event: Event) -> Vec<ApiCmd> {
    struct Quiet;
    impl super::super::Host for Quiet {
        fn pointer(&mut self, _hand: bool) {}
    }
    gui.pending.clear();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (audio_tx, _audio_rx) = std::sync::mpsc::channel();
    let (api_tx, api_rx) = std::sync::mpsc::channel();
    event_tx.send(event).unwrap();
    let channels = super::super::Channels { event_rx, audio_tx, api_tx, event_tx, control: None };
    let mut ctx = super::super::Ctx::new(&gui.app, channels);
    let mut terminal = Terminal::new(super::super::tests::IoTest(TestBackend::new(100, 30))).unwrap();
    super::super::frame(&mut terminal, gui, &mut ctx, &mut Quiet).unwrap();
    api_rx.try_iter().collect()
}

/// The session reconnects with the DJ off — a switch from the header, a
/// wake — and the App lets go of what the rows knew: the room up asks Play
/// from's server again, and so does a layer of the DJ's up over another
/// room; with neither up, nothing is asked.
#[test]
fn a_reconnect_under_the_room_asks_play_froms_server_again() {
    let _en = english();
    // A server that lists its peers is asked for them: the pass writes no
    // config file.
    let connected = || Event::Connected {
        server: HOST.into(),
        id: HOST.into(),
        username: None,
        token: None,
        ping: Box::new(Ping { federation_browse: true, ..Ping::default() }),
    };
    let probes = |asked: &[ApiCmd]| {
        asked.iter().filter(|cmd| matches!(cmd, ApiCmd::DjProbe { identity, .. } if identity == HOST)).count()
    };
    let mut gui = room_gui();
    let asked = frame_with(&mut gui, connected());
    assert_eq!(probes(&asked), 1, "the room asks again: {asked:?}");
    probed(&mut gui, info("6.33.1"));
    let rows = draw(&mut gui);
    assert_eq!(hit_text(&gui, &rows, "Learn from skips"), Some(Act::DjRow(Row::Learn)));
    // Quick config's Seeds sheet, up over the Files room.
    gui.act(Act::Nav(FILES_NAV));
    gui.act(Act::DjQuick(QuickAct::Toggle));
    gui.act(Act::DjQuick(QuickAct::Press(super::quick::QStop::Seeds)));
    assert!(gui.dj.seeds.is_some());
    let asked = frame_with(&mut gui, connected());
    assert_eq!(probes(&asked), 1, "the sheet asks again: {asked:?}");
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.seeds.is_none());
    let asked = frame_with(&mut gui, connected());
    assert_eq!(probes(&asked), 0, "nothing of the DJ's up: {asked:?}");
}

/// A row gone to rest under the keyboard's hand hands it to the nearest row
/// above that answers — a sub-row to Sonic similarity, its parent — its
/// slab and its tips with it, and the arrows walk on from there. Before a
/// draw has settled it, they walk from the hand's place.
#[test]
fn a_row_gone_to_rest_under_the_hand_hands_it_to_the_row_above() {
    let _en = english();
    let mut gui = room_gui();
    for _ in 0..3 {
        key(&mut gui, KeyCode::Up);
    }
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Seeds)));
    // Quick config switches sonic off under it.
    gui.act(Act::DjQuick(QuickAct::Toggle));
    gui.act(Act::DjQuick(QuickAct::Press(super::quick::QStop::Sonic)));
    gui.act(Act::DjQuick(QuickAct::Close));
    assert!(!gui.app.dj.sonic);
    let rows = draw(&mut gui);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Sonic)));
    let y = row_of(&rows, "Sonic similarity");
    let b = buffer(&mut gui);
    assert!((17..64).all(|x| b[(x, y)].bg == th().accent), "the hand's slab on Sonic similarity's row");
    assert_eq!(super::tips(&gui).as_deref(), Some("Enter open · ↑ ↓ rows · Esc done"));
    key(&mut gui, KeyCode::Down);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Beat)));
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Up);
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::SkipWords)));
    // Sonic off under a hand on Learn from skips, nothing drawn since: ↓
    // takes Beat matching, ↑ Sonic similarity — the rows either side of it.
    for (code, want) in [(KeyCode::Down, Row::Beat), (KeyCode::Up, Row::Sonic)] {
        super::edit(&mut gui, DjEdit::Sonic { on: true, floor: 55, anchor: SonicAnchor::Rolling });
        gui.dj.cursor = Some(Item::Row(Row::Learn));
        super::edit(&mut gui, DjEdit::Sonic { on: false, floor: 55, anchor: SonicAnchor::Rolling });
        key(&mut gui, code);
        assert_eq!(gui.dj.cursor, Some(Item::Row(want)), "{code:?}");
    }
}

#[test]
fn seeds_opens_its_sheet_by_click_and_by_key() {
    let _en = english();
    let mut gui = room_gui();
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "  Seeds");
    assert!(gui.dj.seeds.is_some() && super::modal_open(&gui));
    assert!(format!("{:?}", gui.dj.seeds).contains("hand: Mode(0), shown: false"), "{:?}", gui.dj.seeds);
    let rows = draw(&mut gui);
    assert!(rows.join("\n").contains("╭─ Seeds ─"));
    assert_eq!(gui.ui.hit(Position { x: 2, y: 2 }), Some(Act::Guard), "the room beneath is inert");
    assert_eq!(super::tips(&gui).as_deref(), Some("Tab ↑ ↓ move · Space press · m menu · Enter done"));
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.seeds.is_none());
    gui.act(Act::DjRowKey(Row::Seeds));
    assert!(format!("{:?}", gui.dj.seeds).contains("hand: Mode(0), shown: true"));
    assert_eq!(gui.dj.cursor, Some(Item::Row(Row::Seeds)), "the room's hand on its row");
    // In Stay close the hand opens on Stay close.
    key(&mut gui, KeyCode::Esc);
    super::edit(&mut gui, DjEdit::SeedMode(SonicAnchor::Locked));
    gui.act(Act::DjRowKey(Row::Seeds));
    assert!(format!("{:?}", gui.dj.seeds).contains("hand: Mode(1)"));
}

// ── The Seeds sheet ─────────────────────────────────────────────────────────

/// A-Drift's sheet, the board's rows 5–24 (x17–82), Looking Back's menu
/// down.
const A_DRIFT: [&str; 20] = [
    "╭─ Seeds ────────────────────────────────────────────────────────╮",
    "│ (•) Drift                 ( ) Stay close                       │",
    "│ Songs are rotated out regularly. Pin a song to keep it from    │",
    "│ being removed.                                                 │",
    "│                                                                │",
    "│ MORE LIKE                                                      │",
    "│ ◉ SkyWalk · Boukmanflow                                    [⋯] │",
    "│ ◌ Crystalstair · Boukmanflow                               [⋯] │",
    "│ ◌ Fayz · ALM                                               [⋯] │",
    "│ ◉ Better Days · Boukmanflow                                [⋯] │",
    "│ ◌ Looking Back · Boukmanflow                               [⋯] │",
    "│ + Add song                                ╭──────────────────╮ │",
    "│                                           │ Less like this   │ │",
    "│ LESS LIKE                                 │ Remove           │ │",
    "│ ◌ Circles · Color Out                     ╰──────────────────╯ │",
    "│ ◉ Heartbeat · Color Out                                    [⋯] │",
    "│ + Add song                                                     │",
    "│                                                                │",
    "│                                                       [ Done ] │",
    "╰────────────────────────────────────────────────────────────────╯",
];

/// A-StayClose's sheet, the board's rows 5–24.
const A_STAY_CLOSE: [&str; 20] = [
    "╭─ Seeds ────────────────────────────────────────────────────────╮",
    "│ ( ) Drift                 (•) Stay close                       │",
    "│ All songs are now pinned. No new songs will be added.          │",
    "│                                                                │",
    "│                                                                │",
    "│ MORE LIKE                                                      │",
    "│ ◉ SkyWalk · Boukmanflow                                    [⋯] │",
    "│ ◉ Crystalstair · Boukmanflow                               [⋯] │",
    "│ ◉ Fayz · ALM                                               [⋯] │",
    "│ ◉ Better Days · Boukmanflow                                [⋯] │",
    "│ ◉ Looking Back · Boukmanflow                               [⋯] │",
    "│ + Add song                                                     │",
    "│                                                                │",
    "│ LESS LIKE                                                      │",
    "│ ◉ Circles · Color Out                                      [⋯] │",
    "│ ◉ Heartbeat · Color Out                                    [⋯] │",
    "│ + Add song                                                     │",
    "│                                                                │",
    "│                                                       [ Done ] │",
    "╰────────────────────────────────────────────────────────────────╯",
];

/// Where an A board's row stands in the build: the sheet is a row
/// taller for its checkbox (decision 34), so the board's rows 5–22 stand a
/// row higher, the checkbox takes row 22, and Done and the bottom border
/// keep 23 and 24.
fn build_row(board: usize) -> usize {
    if board <= 22 { board - 1 } else { board }
}

/// The checkbox's row of the build, unticked.
fn checkbox_row() -> String {
    format!("│ {:<62} │", "[ ] Clear pins when the queue is cleared")
}

/// The sheet's stop on a row: its pin or its `[⋯]`.
fn pin(list: SeedList, path: &str) -> Act {
    Act::DjSeeds(super::SeedsAct::Press(super::seeds::SeedStop::Pin(list, seed_key(path))))
}

fn dots(list: SeedList, path: &str) -> Act {
    Act::DjSeeds(super::SeedsAct::Press(super::seeds::SeedStop::Menu(list, seed_key(path))))
}

fn add(list: SeedList) -> Act {
    Act::DjSeeds(super::SeedsAct::Press(super::seeds::SeedStop::Add(list)))
}

/// `count` rows of one's own in `list`, pinned: "Song 1" … newest first.
fn pinned_rows(gui: &mut Gui, list: SeedList, tag: &str, count: usize) {
    let now = crate::clock::Instant::now();
    for n in (1..=count).rev() {
        let path = format!("lib/{tag}{n}.mp3");
        gui.app.seeds.add_manual(list, seed_key(&path), song(&path, &format!("{tag} {n}"), "Band"), now);
    }
}

#[test]
fn the_seeds_sheet_is_a_drift_cell_for_cell() {
    let _en = english();
    let mut gui = room_gui();
    a_board_lists(&mut gui);
    gui.act(Act::DjRow(Row::Seeds));
    draw(&mut gui);
    // The board shows Looking Back's menu down, opened by its [⋯].
    assert_eq!(gui.ui.hit(Position { x: 78, y: 14 }), Some(dots(SeedList::MoreLike, "lib/looking-back.mp3")));
    gui.act(gui.ui.hit(Position { x: 80, y: 14 }).expect("its last cell too"));
    let rows_open = draw(&mut gui);
    for (i, want) in A_DRIFT.iter().enumerate() {
        let y = build_row(5 + i);
        assert_eq!(cells(&rows_open[y], 17, 83), *want, "board row {} at {y}:\n{}", 5 + i, rows_open.join("\n"));
    }
    assert_eq!(cells(&rows_open[22], 17, 83), checkbox_row());
    // Its colours, the board's.
    let b = buffer(&mut gui);
    assert!(is_text(b[(20, 4)].fg) && bold(&b, 20, 4), "the title");
    assert_eq!((b[(19, 5)].fg, b[(45, 5)].fg), (th().accent, th().dim), "the radios");
    assert!(bold(&b, 23, 5) && !bold(&b, 49, 5), "Drift chosen");
    assert_eq!((b[(19, 6)].fg, b[(19, 9)].fg, b[(19, 17)].fg), (th().dim, th().dim, th().dim), "the line, the headers");
    assert_eq!((b[(19, 10)].fg, b[(19, 11)].fg, b[(19, 19)].fg), (th().ok, th().dim, th().ok), "pinned OK, unpinned DIM");
    assert!(is_text(b[(21, 10)].fg) && b[(29, 10)].fg == th().dim && b[(78, 10)].fg == th().dim, "the title, the artist, [⋯]");
    assert!(b[(21, 14)].fg == th().accent && bold(&b, 21, 14) && b[(78, 14)].fg == th().accent, "the menu's row");
    assert!(is_text(b[(19, 15)].fg), "Add song");
    assert_eq!((b[(61, 15)].fg, b[(63, 17)].fg), (th().accent, th().danger), "the menu, Remove DANGER at rest");
    assert!(is_text(b[(63, 16)].fg));
    assert!(b[(73, 23)].fg == th().accent && bold(&b, 73, 23), "[ Done ]");
    // The menu's verbs take their clicks; a click off it folds it.
    assert_eq!(gui.ui.hit(Position { x: 63, y: 16 }), Some(Act::DjSeeds(super::SeedsAct::Verb(0))));
    assert_eq!(gui.ui.hit(Position { x: 70, y: 17 }), Some(Act::DjSeeds(super::SeedsAct::Verb(1))));
    assert_eq!(gui.ui.hit(Position { x: 30, y: 20 }), Some(Act::DjSeeds(super::SeedsAct::MenuClose)));
    gui.act(Act::DjSeeds(super::SeedsAct::MenuClose));
    // Folded, the sheet is the board's under it: every row's pin and [⋯]
    // are the row's own stops, Add song and the checkbox theirs.
    let rows = draw(&mut gui);
    assert_eq!(cells(&rows[15], 17, 83), format!("│ {:<62} │", "+ Add song"));
    assert_eq!(rows[4..15], rows_open[4..15], "the rows above the menu as they were");
    assert_eq!(gui.ui.hit(Position { x: 19, y: 11 }), Some(pin(SeedList::MoreLike, "lib/crystalstair.mp3")));
    assert_eq!(gui.ui.hit(Position { x: 79, y: 18 }), Some(dots(SeedList::LessLike, "lib/circles.mp3")));
    assert_eq!(gui.ui.hit(Position { x: 19, y: 15 }), Some(add(SeedList::MoreLike)));
    assert_eq!(gui.ui.hit(Position { x: 19, y: 20 }), Some(add(SeedList::LessLike)));
    assert_eq!(gui.ui.hit(Position { x: 30, y: 10 }), Some(Act::Guard), "a title takes no click");
    // The pointer brightens a pin and a [⋯].
    gui.ui.pointer = Some(Position { x: 19, y: 11 });
    let b = buffer(&mut gui);
    assert!(b[(19, 11)].fg == th().bright && bold(&b, 19, 11));
    gui.ui.pointer = Some(Position { x: 79, y: 12 });
    let b = buffer(&mut gui);
    assert!(b[(78, 12)].fg == th().bright && bold(&b, 80, 12));
}

#[test]
fn stay_close_locks_every_pin_and_keeps_the_menus() {
    let _en = english();
    let mut gui = room_gui();
    a_board_lists(&mut gui);
    gui.act(Act::DjRow(Row::Seeds));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "( ) Stay close");
    assert_eq!(gui.app.dj.sonic_anchor, SonicAnchor::Locked, "applied at once");
    assert!(gui.app.seeds.frozen() && gui.dj.seeds.is_some());
    let rows = draw(&mut gui);
    for (i, want) in A_STAY_CLOSE.iter().enumerate() {
        let y = build_row(5 + i);
        assert_eq!(cells(&rows[y], 17, 83), *want, "board row {} at {y}:\n{}", 5 + i, rows.join("\n"));
    }
    assert_eq!(cells(&rows[22], 17, 83), checkbox_row());
    // Every pin ◉ DIM and locked; the menus and Add song still answer.
    let b = buffer(&mut gui);
    for y in [10, 11, 12, 13, 14, 18, 19] {
        assert_eq!(b[(19, y)].fg, th().dim, "row {y}'s pin");
        assert_eq!(gui.ui.hit(Position { x: 19, y }), Some(Act::Guard), "row {y}'s pin is locked");
    }
    assert!(b[(45, 5)].fg == th().accent && bold(&b, 49, 5), "Stay close chosen");
    assert_eq!(gui.ui.hit(Position { x: 78, y: 11 }), Some(dots(SeedList::MoreLike, "lib/crystalstair.mp3")));
    assert!(is_text(b[(19, 15)].fg) && gui.ui.hit(Position { x: 19, y: 15 }) == Some(add(SeedList::MoreLike)));
    // A pin pressed anyway changes nothing; the stored pins were never
    // touched, and Drift gives the user's own back.
    gui.act(pin(SeedList::MoreLike, "lib/crystalstair.mp3"));
    assert!(!gui.app.seeds.more_like[1].pinned);
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "( ) Drift");
    assert!(!gui.app.seeds.frozen());
    let b = buffer(&mut gui);
    assert_eq!((b[(19, 10)].fg, b[(19, 11)].fg), (th().ok, th().dim));
}

#[test]
fn a_seed_press_applies_at_once_and_done_only_closes() {
    let _en = english();
    let mut gui = room_gui();
    a_board_lists(&mut gui);
    gui.act(Act::DjRow(Row::Seeds));
    draw(&mut gui);
    gui.act(pin(SeedList::MoreLike, "lib/crystalstair.mp3"));
    assert!(gui.app.seeds.more_like[1].pinned, "pinned at once");
    assert!(gui.prefs_unsaved.is_some(), "and saved once the edits rest");
    assert_eq!(fg_at(&mut gui, 19, 11), th().ok);
    gui.act(pin(SeedList::LessLike, "lib/heartbeat.mp3"));
    assert!(!gui.app.seeds.less_like[1].pinned, "and let go");
    // Done writes nothing more: there is no draft.
    gui.prefs_unsaved = None;
    let before = gui.app.seeds.clone();
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[ Done ]");
    assert!(gui.dj.seeds.is_none() && gui.prefs_unsaved.is_none());
    assert_eq!(gui.app.seeds, before);
    // Enter and Esc are Done too.
    gui.act(Act::DjRowKey(Row::Seeds));
    key(&mut gui, KeyCode::Enter);
    assert!(gui.dj.seeds.is_none());
    gui.act(Act::DjRowKey(Row::Seeds));
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.seeds.is_none() && gui.prefs_unsaved.is_none());
}

#[test]
fn the_checkbox_says_whether_a_cleared_queue_lets_the_pins_go() {
    let _en = english();
    let mut gui = room_gui();
    assert!(!gui.app.dj.clear_pins_on_queue_clear, "off by default");
    gui.act(Act::DjRow(Row::Seeds));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[ ] Clear pins when the queue is cleared");
    assert!(gui.app.dj.clear_pins_on_queue_clear && gui.prefs_unsaved.is_some());
    let rows = draw(&mut gui);
    let (x, y) = find(&rows, "[✓] Clear pins when the queue is cleared").expect("ticked");
    assert_eq!((x, fg_at(&mut gui, x, y)), (19, th().ok));
    // By the keyboard: ↑ from Done, Space.
    for _ in 0..12 {
        key(&mut gui, KeyCode::Down);
    }
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Char(' '));
    assert!(!gui.app.dj.clear_pins_on_queue_clear);
}

#[test]
fn a_rows_menu_moves_and_removes_and_rests_a_move_with_no_room() {
    let _en = english();
    let mut gui = room_gui();
    a_board_lists(&mut gui);
    gui.act(Act::DjRow(Row::Seeds));
    draw(&mut gui);
    // Less like this: the row goes to the top of Less like, pinned.
    gui.act(dots(SeedList::MoreLike, "lib/looking-back.mp3"));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Less like this");
    let seeds = &gui.app.seeds;
    assert_eq!(seeds.less_like[0].key.path, "lib/looking-back.mp3");
    assert!(seeds.less_like[0].pinned && !seeds.holds(SeedList::MoreLike, &seed_key("lib/looking-back.mp3")));
    assert!(gui.dj.seeds.is_some() && !draw(&mut gui).join("\n").contains("Less like this"), "the menu folds, the sheet stays");
    // More like this, from a Less like row; then Remove.
    gui.act(dots(SeedList::LessLike, "lib/circles.mp3"));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "More like this");
    assert_eq!(gui.app.seeds.more_like[0].key.path, "lib/circles.mp3");
    gui.act(dots(SeedList::MoreLike, "lib/circles.mp3"));
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "Remove");
    assert!(!gui.app.seeds.holds(SeedList::MoreLike, &seed_key("lib/circles.mp3")));
    // Less like full of pins: a More like row's Less like this rests — DIM,
    // no click, and the cursor passes it.
    pinned_rows(&mut gui, SeedList::LessLike, "Pin", 6);
    assert!(!gui.app.seeds.can_take(SeedList::LessLike));
    // (The removal left the hand, stowed, on SkyWalk's [⋯], where Circles
    // stood: ↓ shows it there.)
    gui.act(Act::DjSeeds(super::SeedsAct::MenuClose));
    key(&mut gui, KeyCode::Down);
    key(&mut gui, KeyCode::Char('m'));
    let rows = draw(&mut gui);
    let (vx, vy) = find(&rows, "Less like this").expect("the verb");
    assert_eq!(fg_at(&mut gui, vx, vy), th().dim);
    assert_eq!(gui.ui.hit(Position { x: vx, y: vy }), Some(Act::Guard));
    let b = buffer(&mut gui);
    assert!(b[(vx, vy + 1)].bg == th().accent, "the cursor shown on Remove, the one verb on offer");
    key(&mut gui, KeyCode::Up);
    assert_eq!(buffer(&mut gui)[(vx, vy + 1)].bg, th().accent, "↑ passes the resting verb");
    gui.act(Act::DjSeeds(super::SeedsAct::Verb(0)));
    assert_eq!(gui.app.seeds.more_like[0].key.path, "lib/skywalk.mp3", "a resting verb does nothing");
    key(&mut gui, KeyCode::Enter);
    assert!(!gui.app.seeds.holds(SeedList::MoreLike, &seed_key("lib/skywalk.mp3")), "Enter, Remove");
}

/// The server takes no negatives: Less like rests, DIM — its pins and
/// Add song inert, its menus live — and More like's menus rest Less like
/// this.
#[test]
fn less_like_rests_where_the_server_takes_no_negatives() {
    let _en = english();
    let mut gui = room_gui();
    probed(&mut gui, DjServerInfo { auto_dj: Some(0), ..info("6.33.1") });
    a_board_lists(&mut gui);
    gui.act(Act::DjRow(Row::Seeds));
    let rows = draw(&mut gui);
    assert_eq!(cells(&rows[17], 17, 83), format!("│ LESS LIKE{:>53} │", "Needs a newer server"));
    let b = buffer(&mut gui);
    for y in [18, 19] {
        assert_eq!((b[(19, y)].fg, b[(21, y)].fg), (th().dim, th().dim), "row {y} DIM");
        assert_eq!(gui.ui.hit(Position { x: 19, y }), Some(Act::Guard), "row {y}'s pin rests");
    }
    assert_eq!(b[(19, 20)].fg, th().dim, "its Add song rests");
    assert_eq!(gui.ui.hit(Position { x: 19, y: 20 }), Some(Act::Guard));
    assert_eq!(gui.ui.hit(Position { x: 78, y: 18 }), Some(dots(SeedList::LessLike, "lib/circles.mp3")), "its menu answers");
    assert_eq!(gui.ui.hit(Position { x: 19, y: 15 }), Some(add(SeedList::MoreLike)), "More like's answers");
    gui.act(dots(SeedList::LessLike, "lib/circles.mp3"));
    let rows = draw(&mut gui);
    let (x, y) = find(&rows, "More like this").expect("the verb");
    assert!(gui.ui.hit(Position { x, y }).is_some_and(|act| act == Act::DjSeeds(super::SeedsAct::Verb(0))));
    gui.act(dots(SeedList::MoreLike, "lib/skywalk.mp3"));
    let rows = draw(&mut gui);
    let (x, y) = find(&rows, "Less like this").expect("the verb");
    assert_eq!((fg_at(&mut gui, x, y), gui.ui.hit(Position { x, y })), (th().dim, Some(Act::Guard)));
}

#[test]
fn add_song_rests_while_its_list_is_full_or_the_library_is_another_servers() {
    let _en = english();
    let mut gui = room_gui();
    pinned_rows(&mut gui, SeedList::MoreLike, "Pin", 8);
    gui.act(Act::DjRowKey(Row::Seeds));
    let rows = draw(&mut gui);
    let (x, y) = find(&rows, "+ Add song").expect("More like's");
    assert_eq!((fg_at(&mut gui, x, y), gui.ui.hit(Position { x, y })), (th().dim, Some(Act::Guard)));
    // No stop either: Tab walks the whole sheet and never lands on it.
    let mut seen = Vec::new();
    for _ in 0..40 {
        key(&mut gui, KeyCode::Tab);
        seen.push(format!("{:?}", gui.dj.seeds));
    }
    assert!(!seen.iter().any(|ui| ui.contains("hand: Add(MoreLike)")) && seen.iter().any(|ui| ui.contains("hand: Add(LessLike)")));
    // Stay close shows every row pinned: a list of 8 can take none, even
    // one that was not all pinned.
    key(&mut gui, KeyCode::Esc);
    gui.app.seeds.more_like[3].pinned = false;
    assert!(gui.app.seeds.can_take(SeedList::MoreLike));
    super::edit(&mut gui, DjEdit::SeedMode(SonicAnchor::Locked));
    assert!(!gui.app.seeds.can_take(SeedList::MoreLike));
    // Browsing a server that is not the room's: both rest, with no reason.
    super::edit(&mut gui, DjEdit::SeedMode(SonicAnchor::Rolling));
    gui.app.seeds = Default::default();
    two_servers(&mut gui);
    gui.app.dj_home = Some(ATTIC.into());
    assert!(!gui.app.seeds_pickable());
    gui.act(Act::DjRowKey(Row::Seeds));
    let rows = draw(&mut gui);
    for y in [row_of(&rows, "MORE LIKE") + 1, row_of(&rows, "LESS LIKE") + 1] {
        assert_eq!(cells(&rows[y as usize], 19, 29), "+ Add song");
        assert_eq!((fg_at(&mut gui, 19, y), gui.ui.hit(Position { x: 19, y })), (th().dim, Some(Act::Guard)));
    }
    gui.act(add(SeedList::MoreLike));
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some(), "a press anyway arms nothing");
}

#[test]
fn the_seeds_sheet_lists_stay_closes_ninth_row() {
    // Eight of another server's rows fill More like; a move of the DJ here
    // in Stay close takes the song playing as a ninth row (auto-dj
    // decision 42), and the sheet lists it with the eight — Add song
    // resting, as with any full list.
    let _en = english();
    let mut gui = room_gui();
    // So the move's provisional answer leaves sonic usable.
    gui.app.capabilities.discovery = true;
    let far = "http://far:3000";
    gui.app.servers = vec![known(HOST, "Home"), known(far, "Far")];
    let far_origin = Origin { server: far.into(), peer: None };
    let now = crate::clock::Instant::now();
    for n in (1..=8).rev() {
        let path = format!("lib/far{n}.mp3");
        let row = song(&path, &format!("Far {n}"), "Band");
        assert!(gui.app.seeds.add_manual(SeedList::MoreLike, SeedKey::new(&far_origin, &path), row, now));
    }
    gui.app.dj_server = Some(far.into());
    super::edit(&mut gui, DjEdit::SeedMode(SonicAnchor::Locked));
    let two = Queued { origin: Origin { server: HOST.into(), peer: None }, dj: None, track: song("lib/two.mp3", "Two", "Band") };
    gui.app.queue.items = vec![two];
    gui.app.queue.current = Some(0);
    gui.app.status.source = "http://host:3000/media/two".into();
    let effects = gui.app.dj_move_to(HOST.into());
    gui.pend(effects);
    assert_eq!(gui.app.seeds.more_like.len(), 9);
    assert_eq!(gui.app.seeds.more_like[0].from, SeedFrom::Anchor);

    gui.act(Act::DjRow(Row::Seeds));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    for n in 1..=8 {
        assert!(all.contains(&format!("Far {n} · Band")), "Far {n}:\n{all}");
    }
    assert!(all.contains("Two · Band"), "the ninth row:\n{all}");
    let (x, y) = find(&rows, "+ Add song").expect("More like's");
    assert!(y > row_of(&rows, "Far 1 · Band") && y < row_of(&rows, "LESS LIKE"), "More like's, under its nine rows");
    assert_eq!((fg_at(&mut gui, x, y), gui.ui.hit(Position { x, y })), (th().dim, Some(Act::Guard)));
}

#[test]
fn the_seeds_sheet_walks_by_the_keyboard() {
    let _en = english();
    let mut gui = room_gui();
    a_board_lists(&mut gui);
    gui.act(Act::DjRowKey(Row::Seeds));
    let hand = |gui: &Gui| format!("{:?}", gui.dj.seeds);
    // Tab walks the reading order and wraps: the two modes, each row's pin
    // and [⋯], the two Add songs, the checkbox and Done — twenty stops.
    for _ in 0..20 {
        key(&mut gui, KeyCode::Tab);
    }
    assert!(hand(&gui).contains("hand: Mode(0)"), "{}", hand(&gui));
    key(&mut gui, KeyCode::BackTab);
    assert!(hand(&gui).contains("hand: Done"));
    // ↓ from the modes is the first row's pin; → its [⋯]: the hand the
    // accent slab on the glyph.
    key(&mut gui, KeyCode::Tab);
    key(&mut gui, KeyCode::Down);
    let b = buffer(&mut gui);
    assert!(b[(19, 10)].bg == th().accent && b[(21, 10)].bg != th().accent, "the pin's cell");
    key(&mut gui, KeyCode::Right);
    let b = buffer(&mut gui);
    assert!((78..81).all(|x| b[(x, 10)].bg == th().accent), "[⋯]'s cells");
    // Space on [⋯] opens its menu with the cursor up; Esc folds it and the
    // sheet stays. `m` from the pin opens it too; ← folds.
    key(&mut gui, KeyCode::Char(' '));
    let b = buffer(&mut gui);
    assert!(draw(&mut gui).join("\n").contains("Less like this") && b[(63, 12)].bg == th().accent, "the cursor on its first verb");
    assert_eq!(super::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter choose · Esc close"));
    key(&mut gui, KeyCode::Esc);
    assert!(gui.dj.seeds.is_some() && !draw(&mut gui).join("\n").contains("Less like this"));
    key(&mut gui, KeyCode::Left);
    key(&mut gui, KeyCode::Char('m'));
    assert!(draw(&mut gui).join("\n").contains("Less like this"));
    key(&mut gui, KeyCode::Left);
    assert!(!draw(&mut gui).join("\n").contains("Less like this"));
    // Space on Stay close; PageDown moves nothing here — the lists fit.
    key(&mut gui, KeyCode::Up);
    key(&mut gui, KeyCode::Right);
    key(&mut gui, KeyCode::Char(' '));
    assert!(gui.app.seeds.frozen());
    // A click stows the hand: the next walking key only shows it.
    gui.act(Act::DjSeeds(super::SeedsAct::Press(super::seeds::SeedStop::Mode(0))));
    assert!(!gui.app.seeds.frozen() && hand(&gui).contains("shown: false"));
    key(&mut gui, KeyCode::Down);
    assert!(hand(&gui).contains("hand: Mode(0), shown: true"));
}

#[test]
fn eight_and_eight_rows_scroll_the_lists() {
    let _en = english();
    let mut gui = room_gui();
    pinned_rows(&mut gui, SeedList::MoreLike, "More", 8);
    pinned_rows(&mut gui, SeedList::LessLike, "Less", 8);
    gui.act(Act::DjRowKey(Row::Seeds));
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    // As tall as the room affords — from row 1, never over the bar — the
    // mode, the checkbox and Done fixed, the lists scrolling between.
    let (_, top) = find(&rows, "╭─ Seeds ─").expect("the sheet");
    assert_eq!(top, 1);
    assert_eq!(cells(&rows[24], 17, 18), "╰");
    assert!(all.contains("(•) Drift") && all.contains("[ ] Clear pins") && all.contains("[ Done ]"), "{all}");
    assert!(all.contains("More 1 · Band") && !all.contains("Less 8 · Band"), "{all}");
    assert_eq!((cells(&rows[6], 80, 81), cells(&rows[20], 80, 81)), ("▲".into(), "▼".into()), "the scrollbar");
    assert_eq!(cells(&rows[7], 76, 79), "[⋯]", "[⋯] two cells in, clear of the bar");
    // A walking key brings the hand's row into view: Less like's last, on
    // the window's last line (sixteen rows down: neither full list's Add
    // song is a stop).
    for _ in 0..16 {
        key(&mut gui, KeyCode::Down);
    }
    let rows = draw(&mut gui);
    let all = rows.join("\n");
    assert!(all.contains("Less 8 · Band") && !all.contains("MORE LIKE"), "{all}");
    let at = row_of(&rows, "Less 8 · Band");
    assert_eq!(at, 20);
    // Its menu hangs above it: below, it would pass the sheet's last row.
    key(&mut gui, KeyCode::Char('m'));
    let rows = draw(&mut gui);
    let (mx, my) = find(&rows, "╭──────────────────╮").expect("the menu");
    assert_eq!((mx, my), (59, at - 4), "its right border on [⋯]'s ], over the row:\n{}", rows.join("\n"));
    assert!(cells(&rows[at as usize - 1], 59, 79).starts_with('╰'));
    key(&mut gui, KeyCode::Esc);
    // The wheel turns the window, wherever the hand is.
    gui.wheel(Position { x: 50, y: 12 }, -40);
    let all = draw(&mut gui).join("\n");
    assert!(all.contains("MORE LIKE") && all.contains("More 1 · Band"), "{all}");
    gui.wheel(Position { x: 50, y: 12 }, 40);
    let all = draw(&mut gui).join("\n");
    assert!(all.contains("Less 8 · Band") && all.contains("+ Add song"), "the window's end: {all}");
    // PageUp moves the window a page back.
    key(&mut gui, KeyCode::PageUp);
    assert!(draw(&mut gui).join("\n").contains("More 1 · Band"));
}

#[test]
fn a_row_that_goes_under_the_hand_settles_it() {
    let _en = english();
    let mut gui = room_gui();
    a_board_lists(&mut gui);
    gui.act(Act::DjRowKey(Row::Seeds));
    // The hand on Circles' [⋯], its menu down.
    for _ in 0..7 {
        key(&mut gui, KeyCode::Down);
    }
    key(&mut gui, KeyCode::Right);
    assert!(format!("{:?}", gui.dj.seeds).contains("circles"), "{:?}", gui.dj.seeds);
    key(&mut gui, KeyCode::Char(' '));
    // Its thirty minutes run out: the row goes, the menu with it, and the
    // hand stands where the row stood — on Heartbeat's [⋯]. (The sheet,
    // a row shorter, stands a row lower: it follows its rows.)
    let later = crate::clock::Instant::now() + std::time::Duration::from_secs(31 * 60);
    assert!(gui.app.seeds.prune(later));
    let rows = draw(&mut gui);
    assert!(!rows.join("\n").contains("Circles") && !rows.join("\n").contains("Remove"));
    let y = row_of(&rows, "Heartbeat");
    assert_eq!(y, row_of(&rows, "LESS LIKE") + 1);
    let b = buffer(&mut gui);
    assert!((78..81).all(|x| b[(x, y)].bg == th().accent), "the hand on the row that took its place");
}

// ── Add song's library pick ─────────────────────────────────────────────────

/// The room's server's Files listing, to pick from: Night Drive at row 2,
/// Aurora at row 3.
fn listing(gui: &mut Gui) {
    gui.app.files.set(vec![
        Entry::Parent,
        Entry::Dir { label: "Ambient".into(), path: "music/Ambient".into() },
        Entry::Track { label: "Night Drive".into(), track: Box::new(song("music/a.mp3", "Night Drive", "Color Out")) },
        Entry::Track { label: "Aurora".into(), track: Box::new(song("music/b.mp3", "Aurora", "ALM")) },
    ]);
}

/// The Seeds sheet up by the keyboard, and More like's Add song pressed.
fn armed_by_key(gui: &mut Gui) {
    gui.act(Act::DjRowKey(Row::Seeds));
    key(gui, KeyCode::Down);
    key(gui, KeyCode::Char(' '));
    assert_eq!(gui.app.capture, Some(Capture::Seeds(SeedList::MoreLike)));
}

#[test]
fn add_song_arms_the_library_pick_and_the_song_comes_back_pinned() {
    let _en = english();
    let mut gui = room_gui();
    listing(&mut gui);
    armed_by_key(&mut gui);
    // The sheet is gone before the pick is armed: the Files room, under
    // the pick's banner, the room a row lower.
    assert!(gui.dj.seeds.is_none());
    assert_eq!((gui.screen, gui.active), (Screen::Library, FILES_NAV));
    let rows = draw(&mut gui);
    assert!(rows[2].contains("Pick a song for More like") && rows[2].contains("[X]"), "{:?}", rows[2]);
    assert!(!rows[2].contains("Esc cancels"), "the tips line is off: its tail is cut");
    assert!(rows[3].contains("Files"), "the room a row lower: {:?}", rows[3]);
    let close_x = rows[2].chars().position(|c| c == '[').unwrap() as u16;
    assert_eq!(gui.ui.hit(Position { x: close_x, y: 2 }), Some(Act::CaptureCancel));
    assert_eq!(super::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter picks the song · Esc cancels the pick"));
    // Aurora: a pinned row of More like, the sheet back over the Auto DJ
    // room with the hand on that Add song, shown — and saved soon.
    gui.prefs_unsaved = None;
    gui.act(Act::PaneRow(List::Files, 3, RowVerb::Open));
    assert!(gui.app.capture.is_none());
    assert_eq!((gui.screen, gui.active), (Screen::Library, DJ_NAV));
    let row = &gui.app.seeds.more_like[0];
    assert!(row.pinned && row.key == seed_key("music/b.mp3") && row.track.title_or_file() == "Aurora");
    assert!(gui.prefs_unsaved.is_some());
    assert!(format!("{:?}", gui.dj.seeds).contains("hand: Add(MoreLike), shown: true"), "{:?}", gui.dj.seeds);
    let rows = draw(&mut gui);
    let y = row_of(&rows, "+ Add song");
    assert!(cells(&rows[y as usize - 1], 19, 40).starts_with("◉ Aurora · ALM"));
    let b = buffer(&mut gui);
    assert!(b[(19, y)].fg == th().accent && bold(&b, 19, y), "the hand on it");
    // By the pointer: the hand comes back stowed, on Less like's Add song.
    let rows = draw(&mut gui);
    let (x, y) = find(&rows, "+ Add song").map(|(x, _)| (x, row_of(&rows, "LESS LIKE") + 1)).unwrap();
    gui.act(gui.ui.hit(Position { x, y }).expect("Less like's Add song"));
    assert!(gui.dj.seeds.is_none() && gui.app.capture == Some(Capture::Seeds(SeedList::LessLike)));
    assert!(draw(&mut gui)[2].contains("Pick a song for Less like"));
    gui.act(Act::PaneRow(List::Files, 2, RowVerb::Open));
    assert_eq!(gui.app.seeds.less_like[0].key, seed_key("music/a.mp3"));
    assert!(format!("{:?}", gui.dj.seeds).contains("hand: Add(LessLike), shown: false"));
}

#[test]
fn esc_and_the_banners_x_let_the_pick_go_back_to_the_sheet() {
    let _en = english();
    let mut gui = room_gui();
    listing(&mut gui);
    armed_by_key(&mut gui);
    key(&mut gui, KeyCode::Esc);
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some());
    assert_eq!(gui.active, DJ_NAV);
    assert!(gui.app.seeds.more_like.is_empty(), "nothing landed");
    key(&mut gui, KeyCode::Esc);
    armed_by_key(&mut gui);
    let rows = draw(&mut gui);
    click(&mut gui, &rows, "[X]");
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some() && gui.active == DJ_NAV);
}

/// Esc takes one layer at a time (decision 33): a filter or a Search query
/// being typed ends first and the pick stands; the next Esc lets it go.
/// The banner's `[X]` lets it go outright.
#[test]
fn esc_takes_a_typed_filter_first_and_the_pick_next() {
    let _en = english();
    let mut gui = room_gui();
    listing(&mut gui);
    armed_by_key(&mut gui);
    gui.act(Act::BarFilter);
    type_text(&mut gui, "au");
    assert!(gui.app.filtering);
    key(&mut gui, KeyCode::Esc);
    assert!(!gui.app.filtering && gui.app.capture == Some(Capture::Seeds(SeedList::MoreLike)));
    assert!(gui.dj.seeds.is_none() && gui.active == FILES_NAV, "still picking");
    key(&mut gui, KeyCode::Esc);
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some() && gui.active == DJ_NAV);
    // A Search query being typed, the same.
    key(&mut gui, KeyCode::Esc);
    armed_by_key(&mut gui);
    gui.act(Act::Nav(SEARCH_NAV));
    assert!(gui.app.editing_query);
    key(&mut gui, KeyCode::Esc);
    assert!(!gui.app.editing_query && gui.app.capture.is_some() && gui.dj.seeds.is_none());
    key(&mut gui, KeyCode::Esc);
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some() && gui.active == DJ_NAV);
    // [X] with a filter being typed: one click.
    key(&mut gui, KeyCode::Esc);
    armed_by_key(&mut gui);
    gui.act(Act::BarFilter);
    type_text(&mut gui, "au");
    gui.act(Act::CaptureCancel);
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some() && gui.active == DJ_NAV);
    // And with a query being typed.
    key(&mut gui, KeyCode::Esc);
    armed_by_key(&mut gui);
    gui.act(Act::Nav(SEARCH_NAV));
    assert!(gui.app.editing_query);
    gui.act(Act::CaptureCancel);
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some() && gui.active == DJ_NAV);
}

#[test]
fn a_pick_after_the_rooms_server_changed_is_refused_and_says_so() {
    let _en = english();
    let mut gui = room_gui();
    listing(&mut gui);
    two_servers(&mut gui);
    armed_by_key(&mut gui);
    // Play from moved under the pick (the control face, a removal).
    gui.app.dj_home = Some(ATTIC.into());
    gui.act(Act::PaneRow(List::Files, 3, RowVerb::Open));
    assert!(gui.app.capture.is_none() && gui.app.seeds.more_like.is_empty(), "consumed, and no row");
    assert!(gui.dj.seeds.is_some() && gui.active == DJ_NAV, "home all the same");
    assert_eq!(gui.note.as_ref().map(|(text, _)| text.as_str()), Some("Pick a track on attic"));
}

#[test]
fn the_rooms_nav_row_lets_a_standing_pick_go_and_opens_no_sheet() {
    let _en = english();
    let mut gui = room_gui();
    listing(&mut gui);
    armed_by_key(&mut gui);
    gui.act(Act::Nav(DJ_NAV));
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_none() && gui.active == DJ_NAV);
    // Also with a Search query edit left open: it takes the first Cancel.
    armed_by_key(&mut gui);
    gui.app.editing_query = true;
    gui.act(Act::Nav(DJ_NAV));
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_none());
    // Stop leaves a pick standing: the sheet works with the DJ off.
    gui.app.queue.items = vec![queued("a.mp3", None)];
    gui.act(Act::DjStartStop);
    armed_by_key(&mut gui);
    let effects = gui.app.disarm_dj();
    gui.pend(effects);
    assert_eq!(gui.app.capture, Some(Capture::Seeds(SeedList::MoreLike)));
    // The MP3 Player tab lets it go in place.
    gui.act(Act::Screen(Screen::Device));
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_none());
    assert_eq!(gui.screen, Screen::Device);
}

/// The pick leaves for the Files room, and comes home, by the screens' own
/// door: the screen left goes whole — the full-screen view's keys from Now
/// Playing, the Admin tab's room and its log's poll under quick config.
#[test]
fn the_pick_goes_and_comes_home_by_the_screens_own_door() {
    let _en = english();
    let mut gui = room_gui();
    listing(&mut gui);
    armed_by_key(&mut gui);
    // The banner's [X] on Now Playing.
    gui.act(Act::Screen(Screen::NowPlaying));
    assert!(gui.app.fullscreen);
    let rows = draw(&mut gui);
    assert_eq!(hit_text(&gui, &rows, "[X]"), Some(Act::CaptureCancel));
    click(&mut gui, &rows, "[X]");
    assert!(gui.app.capture.is_none() && gui.dj.seeds.is_some());
    assert_eq!((gui.screen, gui.active), (Screen::Library, DJ_NAV));
    assert!(!gui.app.fullscreen, "the full-screen view's keys went with it");
    key(&mut gui, KeyCode::Esc);
    // Add song from quick config's Seeds sheet over the Admin tab.
    gui.act(Act::Screen(Screen::Admin));
    assert!(gui.admin.log.running() && gui.admin.room.is_some());
    gui.act(Act::DjQuick(QuickAct::Toggle));
    gui.act(Act::DjQuick(QuickAct::Press(super::quick::QStop::Seeds)));
    gui.act(add(SeedList::MoreLike));
    assert_eq!(gui.app.capture, Some(Capture::Seeds(SeedList::MoreLike)));
    assert_eq!((gui.screen, gui.active), (Screen::Library, FILES_NAV));
    assert!(!gui.admin.log.running() && gui.admin.room.is_none(), "the Admin tab let go");
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
        // Fourteen settings, two headers and the title on the room's 17
        // rows: no name clipped, and every value clear of its name.
        for y in 2..19u16 {
            let row = room(&rows, y);
            assert!(!row.contains('…'), "{locale} row {y} is clipped: {row:?}");
        }
        let all = squash(&rows.join("\n"));
        for key in ["gui.dj.learn_skips", "gui.dj.seeds", "gui.dj.sonic", "gui.dj.harmonic", "gui.dj.cooldown"] {
            assert!(all.contains(&squash(&rust_i18n::t!(key))), "{locale}: {key} is not whole:\n{}", rows.join("\n"));
        }
        let title = room(&rows, 2);
        assert!(title.trim_end().len() < title.len() || title.ends_with(' '), "{locale}: {title:?}");
        // At 100×30 Learn from skips' line stands whole under its name, on
        // the rows the room has for it.
        probed(&mut gui, info("6.33.1"));
        let rows = draw(&mut gui);
        let name = squash(&rust_i18n::t!("gui.dj.learn_skips"));
        let learn = rows.iter().position(|row| squash(&cells(row, 17, 100)).contains(&name)).expect("Learn's row");
        let lines = crate::kit::wrap_whole(&rust_i18n::t!("gui.dj.learn_skips_sub"), 43);
        for (i, line) in lines.iter().enumerate() {
            assert!(squash(&rows[learn + 1 + i]).contains(&squash(line)), "{locale}: {line:?}\n{}", rows.join("\n"));
        }
        assert!(squash(&rows[24]).contains(&squash(&rust_i18n::t!("gui.dj.harmonic"))), "{locale}: the room's last row");
        // The Seeds sheet: every word whole but the songs' own.
        a_board_lists(&mut gui);
        gui.act(Act::DjRowKey(Row::Seeds));
        let rows = draw(&mut gui);
        let all = squash(&rows.join("\n"));
        for key in ["gui.dj.drift", "gui.dj.stay_close", "gui.dj.add_song", "gui.dj.clear_pins", "gui.moods.done"] {
            assert!(all.contains(&squash(&rust_i18n::t!(key))), "{locale}: {key} is not whole:\n{}", rows.join("\n"));
        }
        for line in crate::kit::wrap_whole(&rust_i18n::t!("gui.dj.seeds_drift_sub"), 62) {
            assert!(all.contains(&squash(&line)), "{locale}: {line:?} is not whole:\n{}", rows.join("\n"));
        }
        for key in ["gui.dj.more_like", "gui.dj.less_like"] {
            assert!(all.contains(&squash(&rust_i18n::t!(key).to_uppercase())), "{locale}: {key}");
        }
        // Its row menu's verbs, and the mode's other line.
        let menu = Act::DjSeeds(super::SeedsAct::Press(super::seeds::SeedStop::Menu(
            SeedList::MoreLike,
            seed_key("lib/skywalk.mp3"),
        )));
        gui.act(menu);
        let all = squash(&draw(&mut gui).join("\n"));
        for key in ["gui.dj.less_like_this", "gui.dj.seed_remove"] {
            assert!(all.contains(&squash(&rust_i18n::t!(key))), "{locale}: {key}");
        }
        key(&mut gui, KeyCode::Esc);
        super::edit(&mut gui, DjEdit::SeedMode(SonicAnchor::Locked));
        let all = squash(&draw(&mut gui).join("\n"));
        for line in crate::kit::wrap_whole(&rust_i18n::t!("gui.dj.seeds_stay_sub"), 62) {
            assert!(all.contains(&squash(&line)), "{locale}: {line:?}");
        }
        key(&mut gui, KeyCode::Esc);
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
            "gui.tips.dj_seeds",
            "gui.tips.dj_seed_pick",
            "gui.dj.start_keys",
        ] {
            let tip = rust_i18n::t!(key);
            assert!(crate::kit::width(&tip) <= 99, "{locale}: {key} is {} cells", crate::kit::width(&tip));
        }
        // The seeds' words in the cells the boards gave them: a
        // header in the room's 47; Learn from skips before the switch at
        // 59; its line two rows at 43, and the reason one; the mode's lines
        // two at the sheet's 62; LESS LIKE and the reason side by side; Add
        // song and the checkbox's words beside their glyphs; the menu's
        // verbs in a list the sheet can hang.
        let w = |key: &str| crate::kit::width(&rust_i18n::t!(key));
        let lines = |key: &str, at: usize| crate::kit::wrap_whole(&rust_i18n::t!(key), at).len();
        for key in ["gui.dj.group_filters", "gui.dj.group_match"] {
            assert!(crate::kit::width(&rust_i18n::t!(key).to_uppercase()) <= 47, "{locale}: {key}");
        }
        assert!(2 + w("gui.dj.learn_skips") <= 40, "{locale}: Learn from skips is {} cells", w("gui.dj.learn_skips"));
        assert!(lines("gui.dj.learn_skips_sub", 43) <= 2, "{locale}: Learn's line");
        assert!(w("gui.dj.needs_newer_server") <= 43 && w("gui.dj.seeds") <= 43, "{locale}");
        for key in ["gui.dj.seeds_drift_sub", "gui.dj.seeds_stay_sub"] {
            assert!(lines(key, 62) <= 2, "{locale}: {key} takes {} lines", lines(key, 62));
        }
        let less = crate::kit::width(&rust_i18n::t!("gui.dj.less_like").to_uppercase());
        assert!(less + 1 + w("gui.dj.needs_newer_server") <= 60, "{locale}: LESS LIKE and its reason");
        assert!(2 + w("gui.dj.add_song") <= 58 && 4 + w("gui.dj.clear_pins") <= 62, "{locale}");
        for key in ["gui.dj.more_like_this", "gui.dj.less_like_this", "gui.dj.seed_remove"] {
            assert!(w(key) + 6 <= 40, "{locale}: {key} is {} cells", w(key));
        }
        // The pick's banners keep the " · " their Esc words are cut at.
        for key in ["gui.dj.seeds_pick_more", "gui.dj.seeds_pick_less"] {
            assert!(rust_i18n::t!(key).rsplit_once(" · ").is_some(), "{locale}: {key}");
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
