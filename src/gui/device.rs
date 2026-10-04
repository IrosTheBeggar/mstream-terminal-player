//! The MP3 Player screen: the firmware page of `mstream-player device
//! flash` (src/device/page.rs) hosted whole under the GUI's top bar
//! (docs/ux-contracts/mp3-player-screen.md) — the same page, worker and
//! keys, with no flags: the pinned firmware, the port found by itself, the
//! erase the board decides. No session is involved, so nothing here reads
//! the App's.
//!
//! The page is built on entering the tab and dropped on leaving it, except
//! while it writes: from Go until Done or Failed the tab holds the GUI —
//! every key is the page's, the top bar is inert, no screen change is
//! honoured (contract clause 9) — and a quit with the board held before
//! the write lets the board go first (clause 11). The page keeps its own
//! surface; the screen hands it the keys and the pointer, runs its frame
//! duties, and puts its hint on the GUI's footer.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{Act, DJ_NAV, Gui, Screen};
use crate::admin::{HostedRoom, Screen as Hosted, drive_pointer};
use crate::device::Page;

/// How long a quit waits for the worker to restart a board it holds
/// (contract clause 11): a reset is the first thing the restart does, so
/// this covers it with room to spare, and a worker still syncing with the
/// board is not worth a longer hang on the way out.
const LET_GO: Duration = Duration::from_secs(2);

/// The screen's state: the page while the tab is up (or writing), and the
/// page's area in the last frame.
#[derive(Default)]
pub(crate) struct DeviceUi {
    page: Option<Page>,
    /// A press began on the page: its drag and release are the page's.
    pressed: bool,
    /// Where the page was drawn this frame — None in a frame drawn as the
    /// mini player, whose page's surface still holds the last frame's
    /// targets and must not take a click.
    pub(crate) at: Option<Rect>,
    /// Under test, the far ends of the page's channels: the test is the
    /// worker.
    #[cfg(test)]
    pub(crate) ends: Option<crate::device::Ends>,
}

/// The page is writing (contract clause 9): the GUI cannot leave the tab.
pub(crate) fn writing(gui: &Gui) -> bool {
    gui.device.page.as_ref().is_some_and(Page::writing)
}

/// Open the screen: the page, and its worker with it (clause 5). A page
/// already up — the tab selected again, or one kept by a write — stays.
pub(crate) fn open(gui: &mut Gui) {
    if gui.device.page.is_some() {
        return;
    }
    gui.device.pressed = false;
    gui.device.page = Some(build(gui));
}

#[cfg(not(test))]
fn build(_gui: &mut Gui) -> Page {
    crate::device::hosted()
}

/// Under test the page rides channels the test holds: no worker, so no
/// download and no serial port (the vizwin stand-in's way).
#[cfg(test)]
fn build(gui: &mut Gui) -> Page {
    let (page, ends) = Page::quiet();
    gui.device.ends = Some(ends);
    page
}

/// Leaving drops the page, and its worker lets go of the board as its
/// channel closes (clause 8) — never while it writes, which nothing leaves.
pub(crate) fn close(gui: &mut Gui) {
    if writing(gui) {
        return;
    }
    gui.device.page = None;
    gui.device.pressed = false;
}

/// `view` is the Now Playing screen's rect, which keeps the top bar's row
/// for the TUI view's own title; the page wants the rows under the bar,
/// and leaves its own first row blank under it (clause 1).
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, view: Rect) {
    let area = Rect { y: view.y + 1, height: view.height.saturating_sub(1), ..view };
    let hints = gui.ui.key_hints;
    let Some(page) = gui.device.page.as_mut() else { return };
    // The erase box's tooltip names its key as the GUI's own tooltips do.
    HostedRoom::set_key_hints(page, hints);
    HostedRoom::draw_in(page, frame, area);
    gui.device.at = Some(area);
}

/// The screen's keys (clauses 9 and 12). Returns true to quit.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    // A write holds every key: the page's own (only `l` does anything
    // then), and nothing of the GUI's.
    if !writing(gui) {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('F') => return gui.act(Act::Screen(Screen::Library)),
            KeyCode::Char('T') => return gui.act(Act::Screen(Screen::Stats)),
            KeyCode::Char('M') => return gui.act(Act::Screen(Screen::Admin)),
            KeyCode::Char('0') => return gui.act(Act::Screen(Screen::NowPlaying)),
            KeyCode::Char('V') => return gui.act(Act::VizWindow),
            KeyCode::Char(c @ '1'..='9') => return gui.act(Act::Nav(c as usize - '1' as usize)),
            KeyCode::Char('D') => return gui.act(Act::Nav(DJ_NAV)),
            _ => {}
        }
    }
    let Some(page) = gui.device.page.as_mut() else {
        if key.code == KeyCode::Esc {
            return gui.act(Act::Screen(Screen::Library));
        }
        return false;
    };
    // Esc, Enter on Done and the rest end the page through `finished`,
    // sometimes only once the worker has answered (clause 7).
    HostedRoom::press(page, key);
    end_if_finished(gui);
    false
}

/// The footer's line (clause 15): the page's hint, in its hosted words.
pub(crate) fn tips(gui: &Gui) -> String {
    gui.device.page.as_ref().map(Hosted::hint).unwrap_or_default()
}

/// The pointer (clauses 9 and 14). Outside a write, the page's area is the
/// page's, on its own surface, and so are the drag and the release of a
/// press that began there, wherever they land; a GUI modal or the server
/// menu owns the pointer while open. During a write every event is the
/// screen's: inside the area the page's, outside it nobody's — the top
/// bar's clicks do nothing and nothing on the GUI's surface lights. True
/// when the event was the screen's to take.
pub(crate) fn pointer(gui: &mut Gui, mouse: MouseEvent) -> bool {
    if gui.screen != Screen::Device {
        return false;
    }
    // A frame drawn as the mini player drew no page.
    let Some(area) = gui.device.at else { return false };
    let at = Position { x: mouse.column, y: mouse.row };
    let held =
        gui.device.pressed && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_));
    // A release ends the press wherever it lands, whoever takes it.
    if matches!(mouse.kind, MouseEventKind::Up(_)) {
        gui.device.pressed = false;
    }
    let locked = writing(gui);
    if !locked && (gui.modal_open() || gui.servers.drop_open) {
        return false;
    }
    if !area.contains(at) && !held {
        if !locked {
            return false;
        }
        gui.ui.pointer = None;
        gui.ui.dismiss_tooltip();
        return true;
    }
    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
        gui.device.pressed = true;
    }
    // The GUI's surface follows the pointer as on the Stats tab, so the
    // top bar's tabs light and dim as it passes — but not during a write.
    if locked {
        gui.ui.pointer = None;
    } else if matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) {
        gui.ui.motion(at);
    }
    if let Some(page) = gui.device.page.as_mut() {
        drive_pointer(page, mouse);
    }
    end_if_finished(gui);
    true
}

/// The page's own per-frame duties, after the draw (clause 6): its
/// worker's reports, the port watch, a held button and the tooltip clock —
/// the hub's loop, through `HostedRoom::after_frame` — then whether it has
/// ended. Returns whether the pointer is over one of its clickables, for
/// the hand.
pub(crate) fn frame(gui: &mut Gui) -> bool {
    // A page a screen change left behind without passing through `close`
    // goes now; a writing one stays, and the screen change was refused.
    if gui.screen != Screen::Device {
        close(gui);
    }
    let Some(page) = gui.device.page.as_mut() else { return false };
    let over = HostedRoom::after_frame(page);
    end_if_finished(gui);
    over && gui.screen == Screen::Device && gui.device.at.is_some()
}

/// The page ended itself — Esc at its base, Close, a cancel once the board
/// is restarted (clause 7): the screen goes back to the Library, which
/// drops the page.
fn end_if_finished(gui: &mut Gui) {
    let ended = gui.device.page.as_ref().is_some_and(|page| Hosted::finished(page).is_some());
    if !ended {
        return;
    }
    if gui.screen == Screen::Device {
        gui.act(Act::Screen(Screen::Library));
    } else {
        close(gui);
    }
}

/// The player is quitting (clause 11): a board held before the write is
/// let go first, the wait bounded; then the page goes with the GUI. With no
/// page, or none holding a board, nothing waits.
pub(crate) fn quit(gui: &mut Gui) {
    if let Some(page) = gui.device.page.as_mut() {
        page.let_go(LET_GO);
    }
    gui.device.page = None;
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use crate::config::Config;
    use crate::device::{Ends, WorkerCmd};
    use crate::kit::theme::th;
    use crate::tui::app::App;

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// The GUI with no server at all: the tab needs none (contract clause
    /// 3).
    fn gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.demo = Some(super::super::demo_now());
        gui
    }

    fn press(gui: &mut Gui, code: KeyCode) -> bool {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn render_at(gui: &mut Gui, w: u16, h: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| super::super::render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn all(buf: &Buffer) -> String {
        (0..buf.area.height).map(|y| row(buf, y) + "\n").collect()
    }

    /// The column where `needle` starts on `line`, in cells (these rows are
    /// one cell a character).
    fn col(line: &str, needle: &str) -> Option<u16> {
        line.char_indices().position(|(i, _)| line[i..].starts_with(needle)).map(|c| c as u16)
    }

    /// A mouse event through the GUI's own loop, the routing the player
    /// runs.
    fn mouse(gui: &mut Gui, kind: MouseEventKind, x: u16, y: u16) {
        let event = MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
        let mut ctx = super::super::tests::quiet_ctx(gui);
        super::super::input(gui, &mut ctx, ratatui::crossterm::event::Event::Mouse(event));
    }

    fn click(gui: &mut Gui, x: u16, y: u16) {
        mouse(gui, MouseEventKind::Down(MouseButton::Left), x, y);
        mouse(gui, MouseEventKind::Up(MouseButton::Left), x, y);
    }

    /// One pass of the loop's frame half at 100×30: the draw, then the
    /// page's duties.
    fn frame(gui: &mut Gui) {
        super::super::tests::loop_frame(gui, 100, 30);
    }

    /// The test's ends of the page's channels: the test is the worker.
    fn ends(gui: &Gui) -> &Ends {
        gui.device.ends.as_ref().expect("the tab built its page on the test's channels")
    }

    /// The tab up: `F` from the Library.
    fn on_tab() -> Gui {
        let mut gui = gui();
        assert!(!press(&mut gui, KeyCode::Char('F')));
        assert_eq!(gui.screen, Screen::Device);
        assert!(gui.device.page.is_some(), "entering the tab builds the page");
        gui
    }

    /// The tab at the question, the worker's reports folded in.
    fn at_question() -> Gui {
        let mut gui = on_tab();
        ends(&gui).to_question();
        frame(&mut gui);
        gui
    }

    /// The tab with the write under way.
    fn writing_gui() -> Gui {
        let mut gui = at_question();
        assert!(!press(&mut gui, KeyCode::Enter));
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Go { erase: false }], "Enter at the question is Go");
        ends(&gui).writing();
        frame(&mut gui);
        assert!(writing(&gui));
        gui
    }

    // ── The tab ─────────────────────────────────────────────────────────────

    #[test]
    fn the_mp3_player_tab_is_fourth_and_hosts_the_page_under_the_top_bar() {
        let _en = english();
        let mut gui = gui();
        gui.config.gui.key_hints = true; // the footer row, where the page's hint goes
        let buf = render_at(&mut gui, 100, 30);
        let top = row(&buf, 0);
        let (lx, sx, ax, fx, vx) = (
            col(&top, " Library ").unwrap(),
            col(&top, " Stats ").unwrap(),
            col(&top, " Admin ").unwrap(),
            col(&top, " MP3 Player ").unwrap(),
            col(&top, " Visualizer ").unwrap(),
        );
        assert!(lx < sx && sx < ax && ax < fx && fx < vx, "Library, Stats, Admin, MP3 Player, Visualizer: {top}");
        assert_eq!(gui.ui.hit(Position::new(fx + 1, 0)), Some(Act::Screen(Screen::Device)));
        assert_ne!(buf[(fx, 0)].bg, th().accent);

        // A click on the tab opens it: the page whole under the bar.
        click(&mut gui, fx + 1, 0);
        assert_eq!(gui.screen, Screen::Device);
        assert!(!gui.app.fullscreen, "the App's full-screen flag is Now Playing's alone");
        ends(&gui).to_question();
        frame(&mut gui);
        let buf = render_at(&mut gui, 100, 30);
        let text = all(&buf);
        assert_eq!(buf[(fx, 0)].bg, th().accent, "the tab wears the slab");
        assert!(row(&buf, 1).trim().is_empty(), "a blank row under the bar, where the page's header would be:\n{text}");
        assert!(row(&buf, 2).contains("Install or update the player firmware"), "the page's first row:\n{text}");
        assert!(text.contains("Update ▸") && text.contains("COM3 · CH9102"), "the question:\n{text}");
        assert!(!text.contains("Albums") && !text.contains("auto-dj"), "the nav and the bar stand down:\n{text}");
        assert!(!text.contains("mStream MP3 Player"), "no header of the page's own:\n{text}");
        assert_eq!(row(&buf, 29).trim(), "Enter write · e erase first · l logs · Esc cancel", "the footer is the page's hint");

        // Selecting the tab again keeps the page — its channels with it.
        gui.act(Act::Screen(Screen::Device));
        assert!(!ends(&gui).page_gone(), "the same page, not a new one");
        assert!(ends(&gui).sent().is_empty(), "and nothing was said to the worker");
    }

    #[test]
    fn every_step_fits_the_guis_floor_with_the_footer_on() {
        let _en = english();
        // 100×24 with the footer: the page's 22 rows (contract clause 4).
        let mut gui = on_tab();
        gui.config.gui.key_hints = true;
        let floor = |gui: &mut Gui| {
            super::super::tests::loop_frame(gui, 100, 24);
            render_at(gui, 100, 24)
        };
        ends(&gui).no_device();
        let buf = floor(&mut gui);
        assert!(all(&buf).contains("│  Look again  │"), "{}", all(&buf));
        assert_eq!(row(&buf, 22).trim(), "looking again every 2 s…", "the busy line over the footer");
        assert_eq!(row(&buf, 23).trim(), "r look again · l logs · Esc library", "the footer, in the tab's words");
        let mut gui = on_tab();
        gui.config.gui.key_hints = true;
        ends(&gui).to_question();
        let buf = floor(&mut gui);
        assert!(all(&buf).contains("│  Update ▸  │") && all(&buf).contains("▸ Show logs"), "{}", all(&buf));
        press(&mut gui, KeyCode::Enter);
        ends(&gui).writing();
        let buf = floor(&mut gui);
        assert!(all(&buf).contains("█") && row(&buf, 22).contains("writing… 42%"), "{}", all(&buf));
        ends(&gui).done();
        let buf = floor(&mut gui);
        assert!(all(&buf).contains("│  Close  │") && all(&buf).contains("the firmware's README"), "Done whole:\n{}", all(&buf));
        assert_eq!(row(&buf, 23).trim(), "Enter close · l logs");
    }

    #[test]
    fn f_opens_the_tab_from_the_library_now_playing_stats_and_the_admin_hallway_and_leads_back() {
        let _en = english();
        let mut gui = gui();
        press(&mut gui, KeyCode::Char('F'));
        assert_eq!(gui.screen, Screen::Device, "F from the Library");
        press(&mut gui, KeyCode::Char('F'));
        assert_eq!(gui.screen, Screen::Library, "F on the tab leads back");
        assert!(gui.device.page.is_none(), "and drops the page");

        gui.act(Act::Screen(Screen::NowPlaying));
        press(&mut gui, KeyCode::Char('F'));
        assert_eq!(gui.screen, Screen::Device, "F from Now Playing");
        assert!(!gui.app.fullscreen);

        gui.act(Act::Screen(Screen::Stats));
        assert!(gui.device.page.is_none(), "another tab drops the page");
        press(&mut gui, KeyCode::Char('F'));
        assert_eq!(gui.screen, Screen::Device, "F from Stats");

        gui.act(Act::Screen(Screen::Admin));
        press(&mut gui, KeyCode::Char('F'));
        assert_eq!(gui.screen, Screen::Device, "F from the Admin hallway");

        // The GUI's own ways out from the tab, as from the Stats tab.
        press(&mut gui, KeyCode::Char('T'));
        assert_eq!(gui.screen, Screen::Stats);
        press(&mut gui, KeyCode::Char('F'));
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.screen, Screen::Admin);
        press(&mut gui, KeyCode::Char('F'));
        press(&mut gui, KeyCode::Char('0'));
        assert_eq!(gui.screen, Screen::NowPlaying);
        press(&mut gui, KeyCode::Char('F'));
        press(&mut gui, KeyCode::Char('2'));
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::ALBUMS_NAV), "a digit is the Library's room");
        assert!(gui.device.page.is_none());
        press(&mut gui, KeyCode::Char('F'));
        assert!(press(&mut gui, KeyCode::Char('q')), "q quits the player");
    }

    #[test]
    fn the_tab_works_with_no_server_and_a_session_change_leaves_its_page_alone() {
        let _en = english();
        let mut gui = at_question();
        assert!(gui.app.session.server.is_empty(), "no server saved at all");
        // What a reconnect or a server switch runs for the session's
        // screens (the frame's Connected arm, servers.rs): none is the tab's.
        gui.app.connected = true;
        gui.reopen_room();
        super::super::stats::reopen(&mut gui);
        super::super::admin::reopen(&mut gui);
        assert!(!ends(&gui).page_gone(), "the page is the one built on entry");
        assert!(ends(&gui).sent().is_empty());
        assert_eq!(gui.screen, Screen::Device);
    }

    // ── The page's own end ──────────────────────────────────────────────────

    #[test]
    fn esc_at_the_pages_base_and_its_close_lead_back_to_the_library_and_drop_the_page() {
        let _en = english();
        // Esc where nothing holds the board: the page ends at once.
        let mut gui = on_tab();
        ends(&gui).no_device();
        frame(&mut gui);
        press(&mut gui, KeyCode::Esc);
        assert_eq!(gui.screen, Screen::Library, "Esc at the page's base is the way back");
        assert!(gui.device.page.is_none());

        // Esc at the question: the board is restarted first, so the page
        // ends — and the tab with it — once the worker says so.
        press(&mut gui, KeyCode::Char('F'));
        ends(&gui).to_question();
        frame(&mut gui);
        press(&mut gui, KeyCode::Esc);
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit]);
        frame(&mut gui);
        assert_eq!(gui.screen, Screen::Device, "still restarting the board");
        let text = all(&render_at(&mut gui, 100, 30));
        assert!(text.contains("restarting the board…"), "{text}");
        ends(&gui).cancelled();
        frame(&mut gui);
        assert_eq!(gui.screen, Screen::Library, "the page's end, seen by the frame");
        assert!(gui.device.page.is_none());

        // Close, after a write: a click on the page's own button.
        let mut gui = writing_gui();
        ends(&gui).done();
        frame(&mut gui);
        let buf = render_at(&mut gui, 100, 30);
        let y = (0..30).find(|y| row(&buf, *y).contains("│  Close  │")).expect("Close under Done");
        let x = col(&row(&buf, y), "Close").unwrap();
        click(&mut gui, x, y);
        assert_eq!(gui.screen, Screen::Library, "Close is the way back");
        assert!(gui.device.page.is_none());
    }

    // ── The write ───────────────────────────────────────────────────────────

    #[test]
    fn while_the_page_writes_nothing_leaves_the_tab_but_ctrl_c() {
        let _en = english();
        let mut gui = writing_gui();
        gui.config.gui.key_hints = true;
        let queue_open = gui.queue_open;
        let codes = [
            KeyCode::Char('q'),
            KeyCode::Char('F'),
            KeyCode::Char('T'),
            KeyCode::Char('M'),
            KeyCode::Char('0'),
            KeyCode::Char('1'),
            KeyCode::Char('9'),
            KeyCode::Char('D'),
            KeyCode::Char('V'),
            KeyCode::Char('/'),
            KeyCode::Tab,
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char(' '),
            KeyCode::Char('p'),
            KeyCode::Char('n'),
            KeyCode::Char('s'),
            KeyCode::Char('r'),
            KeyCode::Char('A'),
            KeyCode::Char('-'),
            KeyCode::Char('+'),
        ];
        for code in codes {
            assert!(!press(&mut gui, code), "{code:?} does not quit");
            assert_eq!(gui.screen, Screen::Device, "{code:?} does not leave the write");
            assert!(writing(&gui), "{code:?}: the page writes on");
        }
        assert!(ends(&gui).sent().is_empty(), "and none of them reached the worker");
        assert!(!super::super::vizwin::is_open(&gui), "V opened nothing");
        assert_eq!(gui.queue_open, queue_open, "Tab toggled nothing");

        // The page's own `l` still works.
        press(&mut gui, KeyCode::Char('l'));
        let buf = render_at(&mut gui, 100, 30);
        let text = all(&buf);
        assert!(text.contains("▾ Hide logs"), "the log opens:\n{text}");
        assert!(row(&buf, 29).contains("please wait"), "the footer is the write's hint: {}", row(&buf, 29));

        // No screen change from anywhere: an act, a nav row, the banner's X.
        for act in [Act::Screen(Screen::Library), Act::Screen(Screen::Stats), Act::Nav(0), Act::CaptureCancel] {
            assert!(!gui.act(act.clone()));
            assert_eq!(gui.screen, Screen::Device, "{act:?} is refused");
        }

        // The top bar is inert: no tab answers, none lights, and a click on
        // any of its items — or the header's corner — does nothing.
        let top = row(&buf, 0);
        for label in [" Library ", " Stats ", " Admin ", " MP3 Player ", " Visualizer "] {
            let x = col(&top, label).unwrap() + 1;
            assert_eq!(gui.ui.hit(Position::new(x, 0)), None, "{label} answers nothing during the write");
            mouse(&mut gui, MouseEventKind::Moved, x, 0);
            let buf = render_at(&mut gui, 100, 30);
            assert_ne!(buf[(x, 0)].fg, th().bright, "{label} does not light");
            click(&mut gui, x, 0);
            assert_eq!(gui.screen, Screen::Device, "a click on {label} leaves nothing");
        }
        click(&mut gui, 96, 0);
        assert!(!gui.servers.drop_open && gui.servers.form.is_none(), "the header's corner opens nothing");
        frame(&mut gui);
        assert!(gui.device.page.is_some() && writing(&gui), "the page is never dropped");

        // Ctrl+C is the one way out, as everywhere.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(super::super::handle_key(&mut gui, ctrl_c), "Ctrl+C still quits");

        // Once the write is done, the tab is a tab again.
        ends(&gui).done();
        frame(&mut gui);
        assert!(!writing(&gui));
        press(&mut gui, KeyCode::Char('T'));
        assert_eq!(gui.screen, Screen::Stats);
    }

    #[test]
    fn a_failed_write_lets_the_tab_go_again() {
        let _en = english();
        let mut gui = writing_gui();
        ends(&gui).failed();
        frame(&mut gui);
        assert!(!writing(&gui));
        let text = all(&render_at(&mut gui, 100, 30));
        assert!(text.contains("That did not work") && text.contains("Try again"), "{text}");
        press(&mut gui, KeyCode::Char('F'));
        assert_eq!(gui.screen, Screen::Library);
        assert!(gui.device.page.is_none());
    }

    // ── Leaving and quitting ────────────────────────────────────────────────

    #[test]
    fn leaving_at_the_question_drops_the_page_and_its_worker_hears_it() {
        let _en = english();
        let mut gui = at_question();
        press(&mut gui, KeyCode::Char('T'));
        assert_eq!(gui.screen, Screen::Stats);
        assert!(gui.device.page.is_none(), "leaving drops the page");
        let ends = gui.device.ends.as_ref().unwrap();
        assert!(ends.page_gone(), "the worker's reports have nobody to read them: it restarts the board and ends");
        assert!(ends.sent().is_empty(), "nothing else was said; the closed channel is the word");
    }

    #[test]
    fn with_no_board_the_port_watch_looks_again_after_two_seconds_of_frames() {
        let _en = english();
        let mut gui = on_tab();
        ends(&gui).no_device();
        frame(&mut gui);
        let t0 = Instant::now();
        let mut rescan_after = None;
        while t0.elapsed() < Duration::from_secs(6) {
            frame(&mut gui);
            let sent = ends(&gui).sent();
            if !sent.is_empty() {
                assert_eq!(sent, vec![WorkerCmd::Rescan]);
                rescan_after = Some(t0.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let after = rescan_after.expect("the watch asked again");
        assert!(after >= Duration::from_millis(1800), "not before its two seconds: {after:?}");
    }

    #[test]
    fn quitting_at_the_question_lets_the_board_go_and_waits_for_the_restart() {
        let _en = english();
        let mut gui = at_question();
        let ends = gui.device.ends.take().unwrap();
        let worker = std::thread::spawn(move || {
            let heard = ends.cmds.recv_timeout(Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(200));
            ends.cancelled();
            heard
        });
        let t0 = Instant::now();
        quit(&mut gui);
        let took = t0.elapsed();
        assert_eq!(worker.join().unwrap(), Ok(WorkerCmd::Quit), "the worker was told to let go");
        assert!(took >= Duration::from_millis(150) && took < LET_GO, "it waited for the answer, no longer: {took:?}");
        assert!(gui.device.page.is_none());
    }

    #[test]
    fn quitting_while_the_board_is_being_reached_lets_it_go_too() {
        let _en = english();
        let mut gui = on_tab();
        ends(&gui).reaching();
        frame(&mut gui);
        let ends = gui.device.ends.take().unwrap();
        let worker = std::thread::spawn(move || {
            let heard = ends.cmds.recv_timeout(Duration::from_secs(5));
            ends.cancelled();
            heard
        });
        let t0 = Instant::now();
        quit(&mut gui);
        assert_eq!(worker.join().unwrap(), Ok(WorkerCmd::Quit), "the port it has open is let go");
        assert!(t0.elapsed() < LET_GO, "{:?}", t0.elapsed());
    }

    #[test]
    fn quitting_at_the_question_with_no_answer_waits_its_bound_and_no_longer() {
        let _en = english();
        let mut gui = at_question();
        let t0 = Instant::now();
        quit(&mut gui);
        let took = t0.elapsed();
        assert!(took >= LET_GO - Duration::from_millis(100), "the bound: {took:?}");
        assert!(took < LET_GO + Duration::from_secs(2), "and no longer: {took:?}");
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit]);
    }

    #[test]
    fn quitting_with_no_page_or_no_board_held_does_not_wait() {
        let _en = english();
        let t0 = Instant::now();
        // The Library, no page.
        let mut gui = gui();
        quit(&mut gui);
        // The tab watching for a board: nothing is held.
        let mut gui = on_tab();
        ends(&gui).no_device();
        frame(&mut gui);
        quit(&mut gui);
        assert!(ends(&gui).sent().is_empty(), "nothing to let go of");
        // A write: nothing can stop it but the exit itself.
        let mut gui = writing_gui();
        quit(&mut gui);
        assert!(ends(&gui).sent().is_empty(), "the write is never told to stop");
        assert!(t0.elapsed() < Duration::from_secs(1), "no wait: {:?}", t0.elapsed());
    }

    // ── The pointer ─────────────────────────────────────────────────────────

    #[test]
    fn the_wheel_on_the_tab_never_scrolls_the_hidden_library_room_or_queue() {
        let _en = english();
        let mut gui = gui();
        gui.app.connected = true;
        render_at(&mut gui, 100, 30);
        // On the Library, the wheel reaches the room and the queue.
        let (room, queue) = (Position::new(40, 10), Position::new(90, 10));
        mouse(&mut gui, MouseEventKind::ScrollDown, room.x, room.y);
        mouse(&mut gui, MouseEventKind::ScrollDown, queue.x, queue.y);
        assert!(gui.files_view.scroll > 0 && gui.queue_view.scroll > 0, "the control: the wheel scrolls them here");
        gui.files_view.scroll = 0;
        gui.queue_view.scroll = 0;

        press(&mut gui, KeyCode::Char('F'));
        ends(&gui).several();
        frame(&mut gui);
        render_at(&mut gui, 100, 30);
        for at in [room, queue, Position::new(room.x, 0), Position::new(queue.x, 0), Position::new(room.x, 29)] {
            mouse(&mut gui, MouseEventKind::ScrollDown, at.x, at.y);
            gui.wheel(at, 1);
        }
        assert_eq!((gui.files_view.scroll, gui.queue_view.scroll), (0, 0), "nothing hidden scrolled");
    }

    #[test]
    fn the_pointer_below_the_bar_is_the_pages_and_the_bar_stays_the_guis() {
        let _en = english();
        let mut gui = on_tab();
        ends(&gui).several();
        frame(&mut gui);
        let buf = render_at(&mut gui, 100, 30);
        let y = (0..30).find(|y| row(&buf, *y).contains("COM7 · CP210x")).expect("the second board");
        let x = col(&row(&buf, y), "COM7").unwrap();
        click(&mut gui, x, y);
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Pick("COM7".into())], "a click on a row is the page's pick");
        // The top bar is the GUI's: its Library tab leads back.
        let lx = col(&row(&buf, 0), " Library ").unwrap();
        click(&mut gui, lx + 1, 0);
        assert_eq!(gui.screen, Screen::Library);
        assert!(gui.device.page.is_none());
    }
}
