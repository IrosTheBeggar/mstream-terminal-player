//! The Now Playing screen: the TUI's full-screen view, whole, under the
//! GUI's top bar (docs/ux-contracts/now-playing.md) — the facts column
//! with the cover beneath, the tabbed panel (Queue · Lyrics · Discover ·
//! Auto-DJ · Visualizer, as the session offers them), the rule, the
//! mirrored waveform band with the scrubber, the modes on the last row —
//! plus the one thing a pointer surface adds: prev · play · next in the
//! bar's own frames under the cover. The GUI's queue panel and bar stand
//! down here; the view has its own queue tab and its own seek control.
//!
//! The App's `fullscreen` flag is up while the screen is, so the App's
//! full-screen keys mean here what they mean in the TUI: Enter on the
//! queue tab plays, ↑↓ walk the tab's list, `i` picks the Queue tab, and
//! the visualizer asks for its thirty frames a second.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

use super::bar::{TallKind, play_glyphs, tall_compact};
use super::{Act, Gui, Screen, put};
use crate::tui::app::Action;
use crate::tui::ui::{NowExtras, now_regions, render_now_view};

/// What the cover yields to the transport: one row of air, then the
/// three-row frames.
const TRANSPORT_ROWS: u16 = 4;

/// What the screen keeps between frames: where the band and the cover
/// stood — a click on the band maps against the band's width, and the
/// cover draws as the mosaic when an overlay stood over it last frame.
#[derive(Debug, Default)]
pub(crate) struct NowUi {
    pub band: Rect,
    cover: Option<Rect>,
}

impl NowUi {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// The view's area: the whole frame under the top bar — a row lower while
/// a pick's banner is up — and short of the footer row while the key hints
/// are on. `now_regions` spends the area's first row on the TUI's title,
/// which is the top bar's (or the banner's) row here.
pub(crate) fn view_rect(area: Rect, banner: bool, footer: bool) -> Rect {
    let top = u16::from(banner);
    Rect {
        x: 0,
        y: top,
        width: area.width,
        height: area.height.saturating_sub(top + u16::from(footer)),
    }
}

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, view: Rect) {
    let regions = now_regions(view);
    let mosaic = gui.now.cover.is_some_and(|cover| gui.ui.covered_last_frame(cover));
    let extras = NowExtras { reserve: TRANSPORT_ROWS, mosaic, no_hints: true };
    let layout = render_now_view(frame, &regions, &mut gui.app, &extras);
    gui.now.band = layout.band;
    gui.now.cover = layout.cover;

    // The pointer's ways in, on the view's own elements (contract clauses
    // 4–6): a tab to pick, the band to seek — lit under the pointer as the
    // TUI lights it — and the cover's right click for the track's sheet.
    for (index, rect) in &layout.strip.tabs {
        gui.ui.click(*rect, Act::NowTab(*index));
    }
    if let Some((back, forward)) = layout.strip.arrows {
        gui.ui.click(back, Act::NowTabStep(-1));
        gui.ui.click(forward, Act::NowTabStep(1));
    }
    for column in 0..layout.band.width {
        let cell = Rect { x: layout.band.x + column, y: layout.band.y, width: 1, height: layout.band.height };
        gui.ui.click(cell, Act::NowSeek(column));
    }
    if let Some(cover) = layout.cover {
        gui.ui.context(cover, Act::NowMore);
    }

    // The transport under the cover (clause 3): prev, play, next in the
    // bar's frames, centred in the column — one row of air under the
    // picture, none under the card's own blank row when there is none.
    let air = u16::from(layout.cover.is_some());
    let spare = layout.spare;
    if spare.height >= air + 3 {
        let (prev, play, next) = play_glyphs(gui.bar_paused());
        let group_w: u16 = [prev, play, next].iter().map(|l| crate::kit::width(l) as u16 + 4).sum::<u16>() + 2;
        let y = spare.y + air;
        let mut x = spare.x + spare.width.saturating_sub(group_w) / 2;
        x += tall_compact(frame, &mut gui.ui, x, y, prev, TallKind::Strong, Act::Prev) + 1;
        x += tall_compact(frame, &mut gui.ui, x, y, play, TallKind::Primary, Act::PlayPause) + 1;
        tall_compact(frame, &mut gui.ui, x, y, next, TallKind::Strong, Act::Next);
    }

    // The Auto-DJ tab's pickers are the TUI's own overlay, lent from the
    // view this screen reuses: its Sources row's and its Genres row's —
    // the TUI's genre list, not the room's Genres sheet, which is a kit
    // modal like the chooser. The picker's box stands over the screen as
    // the GUI's own pop-ups do: an overlay, so the cover under it stands
    // down (clause 7) — the desktop window painted the picture over its
    // rows — and a click of either button in it goes no further, the
    // cover's right click (its sheet) included.
    if (gui.app.dj_panel.sources.is_some() || gui.app.dj_panel.genres.is_some())
        && let Some(rect) = crate::tui::ui::render_dj_picker(frame, view, &gui.app)
    {
        gui.ui.overlay(rect);
        gui.ui.click(rect, Act::Guard);
        gui.ui.context(rect, Act::Guard);
    }

    // The screen's note, where the TUI's key hints would be (clause 9): the
    // GUI names its keys on the footer, and the row's right is the modes.
    if let Some((text, style)) = gui.note_words() {
        // Up to the modes readout, whatever it is wearing: a fixed forty
        // cells let a long note run into a wide readout.
        let room = layout.modes.x.saturating_sub(regions.keys.x + 1) as usize;
        put(frame, regions.keys.x, regions.keys.y, &super::bar::clip(&text, room), style);
    }
}

/// The screen goes down — a top-bar tab; the playing track's sheet's Keep
/// a copy…, which brings the Library back for its window (the cover's
/// right click opens the sheet with a mode up, wherever no picker's box
/// stands over the cover); or the App leaving its view on its own (a sonic
/// path's Random song, `Event::SonicRandom`, landing after a mode was
/// opened) — and the App's modes it serves go with it, as their Esc takes
/// them: the keyword field (a word typed and not yet added is dropped) and
/// the Auto-DJ tab's pickers (a switch already flipped stays). The TUI's
/// view keeps every key while one is up, its way out included; here the
/// screen can go down from under one, and the mode stayed up unseen, every
/// later action falling into it — the Library's queue Clear among them.
/// `Act::Nav` calls this too, as a safeguard: no nav row is drawn here,
/// and a mode takes the keys, `D` among them. The room's Genres sheet
/// keeps its own picker.
pub(crate) fn leave(gui: &mut Gui) {
    gui.app.dj_keyword = None;
    gui.app.dj_panel.sources = None;
    if gui.dj.sheet.is_none() {
        gui.app.dj_panel.genres = None;
    }
}

/// The screen's keys (contract clause 8): the TUI's full-screen keys,
/// forwarded to the App with its `fullscreen` flag up, and the GUI's own
/// transport letters. Returns true to quit.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    // The App's own input modes first, the TUI keymap's way: the Auto-DJ
    // tab's keyword field takes the letters, and its sources and genre
    // pickers (the TUI's overlay, drawn here) take the list keys — else
    // Enter on those rows left a mode nothing could serve or leave.
    if gui.app.dj_keyword.is_some() {
        match key.code {
            KeyCode::Char(c) => gui.forward(Action::Input(c)),
            KeyCode::Backspace => gui.forward(Action::Backspace),
            KeyCode::Enter => gui.forward(Action::Submit),
            KeyCode::Esc => gui.forward(Action::Cancel),
            _ => {}
        }
        return false;
    }
    if gui.app.dj_panel.sources.is_some() || gui.app.dj_panel.genres.is_some() {
        match key.code {
            KeyCode::Up => gui.forward(Action::Up),
            KeyCode::Down => gui.forward(Action::Down),
            KeyCode::Char(' ') => gui.forward(Action::PlayPause),
            KeyCode::Enter | KeyCode::Esc => gui.forward(Action::Cancel),
            _ => {}
        }
        return false;
    }
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc | KeyCode::Char('0') => return gui.act(Act::Screen(Screen::Library)),
        // The Stats screen, from anywhere (stats-screen contract, entry 2).
        KeyCode::Char('T') => return gui.act(Act::Screen(Screen::Stats)),
        // The Admin tab, from anywhere (admin-screen contract, clause 1).
        KeyCode::Char('M') => return gui.act(Act::Screen(Screen::Admin)),
        // The MP3 Player tab (mp3-player-screen contract, entry 2).
        KeyCode::Char('K') => return gui.act(Act::Screen(Screen::Device)),
        KeyCode::Char('V') => return gui.act(Act::VizWindow),
        // The strip's numbers pick its tabs — a digit past the strip does
        // nothing, the App bounds-checks — and Tab, Shift-Tab cycle them.
        KeyCode::Char(c @ '1'..='9') => gui.forward(Action::SelectNowTab(c as usize - '1' as usize)),
        KeyCode::Tab => gui.forward(Action::NowTabNext),
        KeyCode::BackTab => gui.forward(Action::NowTabPrev),
        // The tab in front takes the list keys: the queue's rows, Discover's
        // neighbours, the Auto-DJ rows, the lyrics' scroll.
        KeyCode::Up => gui.forward(Action::Up),
        KeyCode::Down => gui.forward(Action::Down),
        KeyCode::PageUp => gui.forward(Action::PageUp),
        KeyCode::PageDown => gui.forward(Action::PageDown),
        KeyCode::Enter => gui.forward(Action::Activate),
        KeyCode::Left => gui.forward(Action::NowLeft),
        KeyCode::Right => gui.forward(Action::NowRight),
        KeyCode::Char('a') => gui.forward(Action::AddToQueue),
        KeyCode::Char('d') | KeyCode::Delete => gui.forward(Action::RemoveFromQueue),
        KeyCode::Char('i') => gui.forward(Action::JumpToPlaying),
        KeyCode::Char('v') => gui.forward(Action::CycleViz),
        KeyCode::Char('.') => gui.forward(Action::ToggleScatter),
        // The Library's Auto DJ room, the GUI's own key for it.
        KeyCode::Char('D') => return gui.act(Act::Nav(super::DJ_NAV)),
        KeyCode::Char(' ') => return gui.act(Act::PlayPause),
        KeyCode::Char('p') => return gui.act(Act::Prev),
        KeyCode::Char('n') => return gui.act(Act::Next),
        KeyCode::Char('s') => return gui.act(Act::Shuffle),
        KeyCode::Char('r') => return gui.act(Act::Repeat),
        KeyCode::Char('A') => return gui.act(Act::AutoDj),
        // The playing track's sheet, as the cover's right click.
        KeyCode::Char('m') => return gui.act(Act::NowMore),
        KeyCode::Char('-') => return gui.act(Act::VolDown),
        KeyCode::Char('+') | KeyCode::Char('=') => return gui.act(Act::VolUp),
        _ => {}
    }
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::{Position, Rect};

    use super::super::{Act, FILES_NAV, Gui, Screen, handle_key, render};
    use crate::api::types::{Genre, Track};
    use crate::config::Config;
    use crate::tui::app::{Action, App, DjRow, KnownServer, NowTab};
    use crate::tui::art::Art;
    use crate::tui::worker::{DjServerInfo, Event};

    const HOST: &str = "http://host:3000";

    /// A connected session, its server saved and its probe answered with two
    /// libraries: the Auto-DJ tab has its Genres, Keywords and Sources rows,
    /// and a library switched off has an entry to be written to.
    pub(crate) fn session() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui.app.servers.push(KnownServer {
            id: HOST.into(),
            name: "host".into(),
            token: None,
            username: None,
            self_signed: false,
            peer: None,
            pairing: None,
            dj: Default::default(),
        });
        let info = DjServerInfo {
            version: Some("6.33.1".into()),
            discovery: true,
            discovery_ready: Some(true),
            libraries: vec!["Music".into(), "Pods".into()],
            mood_filters: true,
            auto_dj: Some(1),
        };
        let effects = gui.app.apply_event(Event::DjProbed { identity: HOST.into(), info: Some(info) });
        gui.pend(effects);
        gui
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// One frame at 100×30, row by row, a cell's symbol each.
    fn screen_cells(gui: &mut Gui) -> Vec<Vec<String>> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    fn screen_text(gui: &mut Gui) -> String {
        screen_cells(gui).iter().map(|row| row.concat()).collect::<Vec<_>>().join("\n")
    }

    /// The box a picker drew, found on the frame by its own lines — the top
    /// border wearing `title` after its corner, the corner across from it,
    /// the bottom corners under them — not by what the screen registered.
    fn drawn_box(cells: &[Vec<String>], title: &str) -> Option<Rect> {
        let title: Vec<String> = title.chars().map(String::from).collect();
        let (y0, at) = cells.iter().enumerate().find_map(|(y, row)| {
            row.windows(title.len()).position(|run| run == title.as_slice()).map(|at| (y, at))
        })?;
        let top = &cells[y0];
        let x0 = at.checked_sub(1).filter(|&x| top[x] == "┌")?;
        let x1 = x0 + top[x0..].iter().position(|cell| cell == "┐")?;
        let y1 = y0 + cells[y0..].iter().position(|row| row[x0] == "└")?;
        if cells[y1][x1] != "┘" {
            return None;
        }
        Some(Rect::new(x0 as u16, y0 as u16, (x1 + 1 - x0) as u16, (y1 + 1 - y0) as u16))
    }

    /// Every cell's left click and right click, as the last frame drew them.
    fn hit_maps(gui: &Gui) -> Vec<(Position, Option<Act>, Option<Act>)> {
        (0..30)
            .flat_map(|y| (0..100).map(move |x| Position { x, y }))
            .map(|at| (at, gui.ui.hit(at), gui.ui.hit_context(at)))
            .collect()
    }

    /// A mouse event through the GUI's own loop, the routing the player
    /// runs: the page owners first, then the surface.
    fn mouse(gui: &mut Gui, kind: MouseEventKind, at: Position) {
        let event = MouseEvent { kind, column: at.x, row: at.y, modifiers: KeyModifiers::NONE };
        let mut ctx = super::super::tests::quiet_ctx(gui);
        super::super::input(gui, &mut ctx, ratatui::crossterm::event::Event::Mouse(event));
    }

    /// Now Playing on its Auto-DJ tab, Enter on `row`: the mode that row opens.
    pub(crate) fn open(gui: &mut Gui, row: DjRow) {
        gui.act(Act::Screen(Screen::NowPlaying));
        for _ in 0..8 {
            if gui.app.now_tab() == NowTab::AutoDj {
                break;
            }
            key(gui, KeyCode::Tab);
        }
        assert_eq!(gui.app.now_tab(), NowTab::AutoDj);
        gui.app.dj_panel.row = gui.app.dj_panel.rows.iter().position(|r| *r == row).expect("the row is on the tab");
        key(gui, KeyCode::Enter);
    }

    /// The cover's one colour, which nothing else on the screen wears.
    pub(crate) const COVER: [u8; 3] = [255, 0, 255];

    /// A track playing, its cover a solid [`COVER`] in the art cache.
    pub(crate) fn play_with_a_cover(gui: &mut Gui) {
        let mut playing = Track { filepath: "Music/a.mp3".into(), metadata: Default::default() };
        playing.metadata.album_art = Some("cover.jpeg".into());
        gui.app.now_playing = Some(playing);
        gui.app.art.insert("cover.jpeg".into(), Art::from_rgb(4, 4, COVER.repeat(16)));
    }

    /// Whether one of the App's modes this screen serves is up.
    fn a_mode_is_up(gui: &Gui) -> bool {
        gui.app.dj_keyword.is_some() || gui.app.dj_panel.sources.is_some() || gui.app.dj_panel.genres.is_some()
    }

    #[test]
    fn the_views_modes_go_down_with_the_screen_whichever_way_it_is_left() {
        // The TUI's view keeps every key while a mode is up, its way out
        // included; here the pointer takes the screen down from under one
        // by the top bar's tabs or the playing track's sheet's Keep a copy…
        // (`Act::Screen` both; `Act::Nav` is a safeguard: no nav row is
        // drawn here), and the mode stayed up unseen, taking every action
        // after it: the Library's queue Clear did nothing.
        let ways = [Act::Screen(Screen::Library), Act::Screen(Screen::Stats), Act::Nav(FILES_NAV)];
        for row in [DjRow::Keywords, DjRow::Sources, DjRow::Genres] {
            for way in ways.clone() {
                let mut gui = session();
                gui.app.replace_queue(vec![Track { filepath: "Music/a.mp3".into(), metadata: Default::default() }]);
                open(&mut gui, row);
                assert!(a_mode_is_up(&gui), "{row:?} opens its mode");
                gui.act(way.clone());
                assert_ne!(gui.screen, Screen::NowPlaying, "{way:?} leaves the screen");
                assert!(!a_mode_is_up(&gui), "{row:?}'s mode goes down with the screen, by {way:?}");
                gui.act(Act::QueueClear);
                assert!(gui.app.queue.items.is_empty(), "{row:?}, {way:?}: the next action is the screen's own");
            }
        }
    }

    #[test]
    fn leaving_keeps_what_the_modes_hold_as_their_esc_keeps_it() {
        // A word typed and not added goes, as Esc drops it; a library
        // switched off stays off — the picker wrote it as it flipped.
        let mut gui = session();
        open(&mut gui, DjRow::Keywords);
        key(&mut gui, KeyCode::Char('l'));
        key(&mut gui, KeyCode::Char('o'));
        assert_eq!(gui.app.dj_keyword.as_deref(), Some("lo"));
        gui.act(Act::Screen(Screen::Library));
        assert!(gui.app.dj_keyword.is_none());
        assert!(gui.app.dj.keywords.is_empty(), "the unfinished word was not added");

        let mut gui = session();
        open(&mut gui, DjRow::Sources);
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.app.dj_sources_off(), ["Music"]);
        gui.act(Act::Screen(Screen::Library));
        assert!(gui.app.dj_panel.sources.is_none());
        assert_eq!(gui.app.dj_sources_off(), ["Music"], "the switch stays where it was flipped");
    }

    #[test]
    fn the_genres_row_opens_a_list_the_screen_draws_and_serves() {
        // The tab's Genres row opens the TUI's genre list, not the room's
        // Genres sheet. It was neither drawn here nor served: Space ticked
        // genres nobody could see, and Esc left the screen with the list
        // still up.
        let mut gui = session();
        open(&mut gui, DjRow::Genres);
        assert!(gui.app.dj_panel.genres.is_some());
        let text = screen_text(&mut gui);
        assert!(text.contains("Genres ·") && text.contains("loading genres"), "drawn while it loads:\n{text}");
        let effects = gui.app.apply_event(Event::Genres(vec![
            Genre { name: "Ambient".into(), track_count: Some(4) },
            Genre { name: "Techno".into(), track_count: Some(9) },
        ]));
        gui.pend(effects);
        assert!(screen_text(&mut gui).contains("Techno"));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.app.dj_library().genres, ["Techno"], "Space ticks the genre in hand");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.dj_panel.genres.is_none(), "Esc closes the list");
        assert_eq!(gui.screen, Screen::NowPlaying, "and stays on the screen");
    }

    #[test]
    fn the_app_leaving_its_view_on_its_own_takes_the_modes_with_it() {
        // However the App comes to drop its full-screen view (arming a pick
        // does), the shell follows it to the Library, and the view's modes
        // go with the view.
        let mut gui = session();
        open(&mut gui, DjRow::Keywords);
        gui.app.fullscreen = false;
        gui.forward(Action::Up);
        assert_eq!(gui.screen, Screen::Library);
        assert!(gui.app.dj_keyword.is_none());
    }

    /// Now Playing with the genre list up, long enough (24 genres, 22 rows
    /// shown) that its box reaches down over the band, and a track with a
    /// cover playing at 0:50 of 3:20: one frame drawn, and the box found on
    /// it.
    fn a_long_list_up() -> (Gui, Rect) {
        let mut gui = session();
        play_with_a_cover(&mut gui);
        gui.app.status.duration = 200.0;
        gui.app.status.position = 50.0;
        open(&mut gui, DjRow::Genres);
        let genre = |i: u32| Genre { name: format!("Genre {i:02}"), track_count: Some(1) };
        let effects = gui.app.apply_event(Event::Genres((0..24).map(genre).collect()));
        gui.pend(effects);
        let cells = screen_cells(&mut gui);
        let Some(boxed) = drawn_box(&cells, " Genres · ") else {
            let text = cells.iter().map(|row| row.concat()).collect::<Vec<_>>().join("\n");
            panic!("no box for the list:\n{text}");
        };
        (gui, boxed)
    }

    #[test]
    fn a_pickers_box_stands_over_the_screen_and_keeps_the_clicks_on_it() {
        // The pickers are the TUI's overlay, lent: the box stands over the
        // screen as the GUI's own pop-ups do, so the cover under it draws as
        // the mosaic from the next frame (clause 7) — the desktop window
        // painted the picture over the rows — and a click of either button
        // on the box goes no further: not to the transport or the band
        // beneath, nor to the cover's right click, which opened the playing
        // track's sheet over the list. The box is the one drawn, border and
        // all; off it every click is the screen's own (the top bar's tabs
        // among them), and with the list down so are the box's cells.
        let (mut gui, boxed) = a_long_list_up();
        let up = hit_maps(&gui);
        screen_text(&mut gui);
        let cover = gui.now.cover.expect("the screen draws its cover");
        assert!(gui.ui.covered_last_frame(cover), "the cover stands down under the list");
        // The overlay is the box drawn, to its border: each corner stands
        // under it, and the cell beyond each edge does not.
        let (x0, y0, x1, y1) = (boxed.x, boxed.y, boxed.right() - 1, boxed.bottom() - 1);
        let covered = |gui: &Gui, x, y| gui.ui.covered_last_frame(Rect::new(x, y, 1, 1));
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            assert!(covered(&gui, x, y), "the box's corner ({x}, {y}) is the overlay's");
        }
        for (x, y) in [(x0 - 1, y0), (x1 + 1, y1), (x1, y0 - 1), (x0, y1 + 1)] {
            assert!(!covered(&gui, x, y), "({x}, {y}), beyond the box, is no overlay's");
        }
        // A right click where the box stands over the cover.
        let over = boxed.positions().find(|at| cover.contains(*at));
        let over = over.expect("the box stands over the cover");
        mouse(&mut gui, MouseEventKind::Down(MouseButton::Right), over);
        assert!(gui.actions.sheet.is_none(), "a right click on the box opens no sheet beneath it");
        assert!(gui.app.dj_panel.genres.is_some(), "and the list stays up");

        key(&mut gui, KeyCode::Esc);
        screen_text(&mut gui);
        let down = hit_maps(&gui);
        let guard = Some(Act::Guard);
        let guarded = down.iter().filter(|(_, click, menu)| *click == guard || *menu == guard);
        assert_eq!(guarded.count(), 0, "no box with the list down");
        let (mut beneath, mut cover_menu) = (0, 0);
        for ((at, click, menu), (_, click_down, menu_down)) in up.iter().zip(&down) {
            if boxed.contains(*at) {
                assert_eq!((click, menu), (&guard, &guard), "a click on the box at {at:?}");
                beneath += usize::from(click_down.is_some());
                cover_menu += usize::from(*menu_down == Some(Act::NowMore));
            } else {
                assert_eq!((click, menu), (click_down, menu_down), "off the box, at {at:?}");
            }
        }
        assert!(beneath > 0, "the box stood over the screen's own controls");
        assert!(cover_menu > 0, "and over the cover's right click");
        screen_text(&mut gui);
        assert!(!gui.ui.covered_last_frame(cover), "and the cover is the picture again");
        mouse(&mut gui, MouseEventKind::Down(MouseButton::Right), over);
        assert!(gui.actions.sheet.is_some(), "the cover's right click is its own again");
    }

    #[test]
    fn the_band_stays_dark_under_a_pickers_box() {
        // The band lights under the pointer and marks where a click would
        // seek (clause 4). A click on a picker's box goes no further, so the
        // pointer resting on it is no one's: where a long list's box reaches
        // down over the band, as it does the mirrored band's wave row, the
        // scrubber beneath stayed lit, its caret promising a seek the box
        // takes. Off the box the band lights as ever.
        let (mut gui, boxed) = a_long_list_up();
        let band = gui.now.band;
        let scrubber = usize::from(band.bottom() - 1);
        let caret = crate::tui::ui::glyphs().caret;
        let lit = |gui: &mut Gui| screen_cells(gui)[scrubber].iter().any(|cell| cell == caret);

        let off = Position { x: boxed.x + 5, y: band.bottom() - 1 };
        mouse(&mut gui, MouseEventKind::Moved, off);
        assert_eq!(gui.app.pointer, Some(off), "off the box, on the scrubber");
        assert!(lit(&mut gui), "the scrubber lights under the pointer");

        // The box's last row, on the band's wave row.
        let on_band = Position { x: boxed.x + 5, y: boxed.bottom() - 1 };
        if band.contains(on_band) {
            mouse(&mut gui, MouseEventKind::Moved, on_band);
            assert!(!lit(&mut gui), "the scrubber stays dark under the box: no seek there");
        } else {
            // The legacy band is one row, under the rule the box ends on.
            assert!(!crate::tui::ui::glyphs().mirrored, "the box ends over a mirrored band");
        }
        // Anywhere on the box, the pointer is no one's.
        mouse(&mut gui, MouseEventKind::Moved, Position { x: boxed.x + 5, y: boxed.y + 1 });
        assert_eq!(gui.app.pointer, None, "the pointer on the box is no one's");
    }
}
