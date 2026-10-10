//! The MP3 Player screen: the firmware page of `mstream-player device
//! flash` (src/device/page.rs) hosted whole under the GUI's top bar
//! (docs/ux-contracts/mp3-player-screen.md) — the same page, worker and
//! keys, with no flags: the pinned firmware, the port found by itself, the
//! erase the board decides. No session is involved, so nothing here reads
//! the App's.
//!
//! The page is built on entering the tab and let go of on leaving it: a
//! worker that holds the board, or is reaching it, is told to let it go
//! and its page kept aside, read every frame until the board is back in
//! its firmware, so the next visit never meets its own port in use
//! (contract clause 8). From Go until Done or Failed the tab holds the
//! GUI — every key is the page's, the top bar is inert, no screen change
//! is honoured (clause 9); Ctrl+C, the window's close button and, on
//! macOS, Cmd-Q or the app menu's Quit still quit, and cut the write
//! short (clause 10). A quit with the board held before the write lets it
//! go first (clause 11). The page keeps its own surface; the screen hands
//! it the keys and the pointer where it is drawn, runs its frame duties,
//! and puts its hint on the GUI's footer.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use rust_i18n::t;

use super::{Act, DJ_NAV, Gui, SONIC_NAV, Screen};
use crate::admin::{HostedRoom, Screen as Hosted, drive_pointer};
use crate::device::Page;
use crate::tui::app::{Action, Capture, Focus, Tab};

/// How long the worker may take to let a board go once told: what is
/// left of one rung of the baud ladder — an espflash connect that never
/// syncs gives up after about seven seconds — and the reset after it. At
/// the question the reset is all there is, a fraction of a second. A quit
/// waits this long at most (contract clause 11), and so does the next
/// visit for a page left behind (clause 8).
const LET_GO: Duration = Duration::from_secs(8);
/// A quit that waits longer than this says why on stderr: the terminal is
/// the shell's again by then, and a pause with no word looks like a hang.
const SAY_AFTER: Duration = Duration::from_secs(1);

/// The screen's state: the page while the tab is up (or writing), the one
/// it was left with while that one lets the board go, and the page's area
/// in the last frame.
#[derive(Default)]
pub(crate) struct DeviceUi {
    page: Option<Page>,
    /// A page the tab was left with while its worker held the board or was
    /// reaching it, told to let go, and since when: read every frame until
    /// the board is back in its firmware (or the wait's bound), then
    /// dropped. A visit meanwhile builds no page of its own.
    parting: Option<(Page, Instant)>,
    /// A press began on the page: its drag and release are the page's.
    pressed: bool,
    /// Where the page was drawn this frame — None in a frame drawn as the
    /// mini player, whose page's surface still holds the last frame's
    /// targets and must take neither a click nor a key.
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
/// The Library's hands are let go first: the queue panel's keys (its
/// focus stows, as a nav row stows it) and an armed pick (as the banner's
/// [X] lets it go, home to the room that asked) — on this tab Enter and
/// Esc are the page's (clause 12), and a pick or a queue left holding them
/// would name keys that start a write instead.
pub(crate) fn open(gui: &mut Gui) {
    if gui.device.page.is_some() {
        return;
    }
    gui.device.pressed = false;
    gui.app.focus = Focus::Browser;
    gui.queue_view.stow();
    if gui.app.capture.is_some() {
        let sonic = matches!(gui.app.capture, Some(Capture::Sonic(_)));
        gui.forward(Action::Cancel);
        if sonic {
            gui.active = SONIC_NAV;
            gui.app.tab = Tab::SonicPath;
        }
    }
    // The page left a moment ago may still be letting the board go: a
    // worker started now would find the port in use. `frame` builds this
    // visit's page once that one has gone.
    if gui.device.parting.is_none() {
        gui.device.page = Some(build(gui));
    }
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

/// Leaving the tab (clause 8) — never while the page writes, which nothing
/// leaves. The worker is told to let go the page's own way, as Esc tells
/// it; a page whose worker holds the board, or was reaching it, is kept
/// aside until the board is back in its firmware, and any other is
/// dropped at once.
pub(crate) fn close(gui: &mut Gui) {
    if writing(gui) {
        return;
    }
    gui.device.pressed = false;
    let Some(mut page) = gui.device.page.take() else { return };
    page.release();
    // At most one page parts at a time: `open` builds none while one does.
    if page.holds_board() && gui.device.parting.is_none() {
        gui.device.parting = Some((page, Instant::now()));
    }
}

/// `view` is the Now Playing screen's rect, which keeps the top bar's row
/// for the TUI view's own title; the page wants the rows under the bar,
/// and leaves its own first row blank under it (clause 1).
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, view: Rect) {
    let area = Rect { y: view.y + 1, height: view.height.saturating_sub(1), ..view };
    let hints = gui.ui.key_hints;
    let Some(page) = gui.device.page.as_mut() else {
        // A visit waiting for the last page to let the board go.
        if gui.device.parting.is_some() {
            crate::device::draw_waiting(frame, area);
        }
        return;
    };
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
            KeyCode::Char('K') => return gui.act(Act::Screen(Screen::Library)),
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
        // The visit waits for the last page: Esc is still the way back.
        if key.code == KeyCode::Esc {
            return gui.act(Act::Screen(Screen::Library));
        }
        return false;
    };
    // The page's keys reach it only where it was drawn: in the mini player
    // it is out of sight, and Enter there would start a write nobody sees.
    if gui.device.at.is_none() {
        return false;
    }
    // Esc, Enter on Done and the rest end the page through `finished`,
    // sometimes only once the worker has answered (clause 7).
    HostedRoom::press(page, key);
    end_if_finished(gui);
    false
}

/// The footer's line (clause 15): the page's hint, in its hosted words —
/// none while a visit waits for the last page to let the board go.
pub(crate) fn tips(gui: &Gui) -> String {
    gui.device.page.as_ref().map(Hosted::hint).unwrap_or_default()
}

/// The mini player's line while the page writes out of sight: the write
/// is under way and must not be cut — in place of the mini player's own
/// request for a larger window.
pub(crate) fn unseen_line(gui: &Gui) -> Option<String> {
    writing(gui).then(|| t!("dev.hint_working_unseen").to_string())
}

/// The pointer (clauses 9 and 14). Outside a write, the page's area is the
/// page's, on its own surface, and so are the drag and the release of a
/// press that began there, wherever they land; a GUI modal or the server
/// menu owns the pointer while open. During a write every event is the
/// screen's: inside the area the page's, outside it nobody's — the top
/// bar's clicks do nothing and nothing on the GUI's surface lights, the
/// mini player's buttons included. True when the event was the screen's
/// to take.
pub(crate) fn pointer(gui: &mut Gui, mouse: MouseEvent) -> bool {
    if gui.screen != Screen::Device {
        return false;
    }
    let at = Position { x: mouse.column, y: mouse.row };
    let held =
        gui.device.pressed && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_));
    // A release ends the press wherever it lands, whoever takes it.
    if matches!(mouse.kind, MouseEventKind::Up(_)) {
        gui.device.pressed = false;
    }
    let locked = writing(gui);
    // A frame drawn as the mini player drew no page: its own buttons answer,
    // but not during a write.
    let Some(area) = gui.device.at else {
        if locked {
            gui.ui.pointer = None;
            gui.ui.dismiss_tooltip();
        }
        return locked;
    };
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
/// ended. A page left behind is read too, until its board is let go; then
/// a visit that waited for it gets its own page. Returns whether the
/// pointer is over one of the page's clickables, for the hand.
pub(crate) fn frame(gui: &mut Gui) -> bool {
    // A page a screen change left behind without passing through `close`
    // goes now; a writing one stays, and the screen change was refused.
    if gui.screen != Screen::Device {
        close(gui);
    }
    if let Some((page, since)) = gui.device.parting.as_mut() {
        Hosted::pump(page);
        // Past the bound a worker that never answers is let be: a port it
        // may still hold is the next page's failure to say, not a tab that
        // never comes back.
        if !page.holds_board() || since.elapsed() >= LET_GO {
            gui.device.parting = None;
        }
    }
    if gui.screen == Screen::Device && gui.device.page.is_none() && gui.device.parting.is_none() {
        gui.device.pressed = false;
        gui.device.page = Some(build(gui));
    }
    let Some(page) = gui.device.page.as_mut() else { return false };
    let over = HostedRoom::after_frame(page);
    end_if_finished(gui);
    over && gui.screen == Screen::Device && gui.device.at.is_some()
}

/// The page ended itself — Esc at its base, Close, a cancel once the board
/// is restarted (clause 7): the screen goes back to the Library, which
/// lets the page go.
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

/// The player is quitting (clause 11): the page, and one left behind, let
/// the board go first — the wait bounded, and said on stderr past a
/// second; then they go with the GUI. Only a board held, or being
/// reached, is waited for: no page, a page with no board, or a write
/// (which only the exit can cut short) waits for nothing.
pub(crate) fn quit(gui: &mut Gui) {
    let_go_within(gui, LET_GO);
}

fn let_go_within(gui: &mut Gui, within: Duration) {
    let parting = gui.device.parting.take().map(|(page, _)| page);
    let mut pages: Vec<Page> = gui.device.page.take().into_iter().chain(parting).collect();
    for page in &mut pages {
        page.release();
    }
    let t0 = Instant::now();
    let mut said = false;
    loop {
        pages.retain_mut(|page| {
            Hosted::pump(page);
            page.holds_board()
        });
        if pages.is_empty() || t0.elapsed() >= within {
            return;
        }
        if !said && t0.elapsed() >= SAY_AFTER {
            eprintln!("mstream-player: {}", t!("dev.letting_go"));
            said = true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use crate::config::Config;
    use crate::device::{Ends, WorkerCmd};
    use crate::kit::theme::th;
    use crate::tui::app::{App, SonicSide};

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// The GUI with no server at all: the tab needs none (contract clause
    /// 3). Key hints off, the GUI's default.
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

    /// The tab up: `K` from the Library.
    fn on_tab() -> Gui {
        let mut gui = gui();
        assert!(!press(&mut gui, KeyCode::Char('K')));
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

    /// The question's hint, the page's own words.
    const CONFIRM: &str = "Enter write · e erase first · l logs · Esc cancel";

    // ── The tab ─────────────────────────────────────────────────────────────

    #[test]
    fn the_mp3_player_tab_is_fourth_and_hosts_the_page_under_the_top_bar() {
        let _en = english();
        let mut gui = gui();
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
        assert_eq!(row(&buf, 29).trim(), CONFIRM, "the footer is the page's hint, key hints or not");

        // Selecting the tab again keeps the page — its channels with it.
        gui.act(Act::Screen(Screen::Device));
        assert!(!ends(&gui).page_gone(), "the same page, not a new one");
        assert!(ends(&gui).sent().is_empty(), "and nothing was said to the worker");
    }

    #[test]
    fn every_step_fits_the_guis_floor_whatever_the_key_hints_say() {
        let _en = english();
        // 100×24, the footer always the page's: its 22 rows (contract
        // clause 4), with key hints off and on.
        for hints in [false, true] {
            let floor = |gui: &mut Gui| {
                super::super::tests::loop_frame(gui, 100, 24);
                render_at(gui, 100, 24)
            };
            let mut gui = on_tab();
            gui.config.gui.key_hints = hints;
            ends(&gui).no_device();
            let buf = floor(&mut gui);
            assert!(all(&buf).contains("│  Look again  │"), "{}", all(&buf));
            assert_eq!(row(&buf, 22).trim(), "looking again every 2 s…", "the busy line over the footer");
            assert_eq!(row(&buf, 23).trim(), "r look again · l logs · Esc library", "the footer, in the tab's words");
            let mut gui = on_tab();
            gui.config.gui.key_hints = hints;
            ends(&gui).to_question();
            let buf = floor(&mut gui);
            assert!(all(&buf).contains("│  Update ▸  │") && all(&buf).contains("▸ Show logs"), "{}", all(&buf));
            press(&mut gui, KeyCode::Enter);
            ends(&gui).writing();
            let buf = floor(&mut gui);
            assert!(all(&buf).contains("█") && row(&buf, 22).contains("writing… 42%"), "{}", all(&buf));
            // Done, the tallest, with the board's own line in full.
            ends(&gui).done();
            let buf = floor(&mut gui);
            assert!(all(&buf).contains("│  Close  │") && all(&buf).contains("The board reports"), "Done whole:\n{}", all(&buf));
            assert_eq!(row(&buf, 23).trim(), "Enter close · l logs");
        }
    }

    #[test]
    fn with_key_hints_off_the_footer_is_still_the_pages_and_the_write_warns_against_unplugging() {
        let _en = english();
        let mut gui = writing_gui();
        assert!(!gui.config.gui.key_hints, "the GUI's default");
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(
            row(&buf, 29).trim(),
            "please wait — unplugging now would leave the board half written · l logs",
            "the write's warning, on the footer the page's area leaves it"
        );
        assert!(row(&buf, 28).contains("writing… 42%"), "the busy line over it: {}", row(&buf, 28));
        let mut gui = at_question();
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(row(&buf, 29).trim(), CONFIRM, "the question names its keys");
    }

    #[test]
    fn k_opens_the_tab_from_the_library_now_playing_stats_and_the_admin_hallway_and_leads_back() {
        let _en = english();
        let mut gui = gui();
        // `F` is no longer the tab's: a Caps Lock slip on the browse bar's
        // `f` must not reset a plugged-in Core2.
        press(&mut gui, KeyCode::Char('F'));
        assert_eq!(gui.screen, Screen::Library, "F opens nothing");
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Device, "K from the Library");
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Library, "K on the tab leads back");
        assert!(gui.device.page.is_none(), "and lets the page go");

        gui.act(Act::Screen(Screen::NowPlaying));
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Device, "K from Now Playing");
        assert!(!gui.app.fullscreen);

        gui.act(Act::Screen(Screen::Stats));
        assert!(gui.device.page.is_none(), "another tab lets the page go");
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Device, "K from Stats");

        gui.act(Act::Screen(Screen::Admin));
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Device, "K from the Admin hallway");

        // The GUI's own ways out from the tab, as from the Stats tab.
        press(&mut gui, KeyCode::Char('T'));
        assert_eq!(gui.screen, Screen::Stats);
        press(&mut gui, KeyCode::Char('K'));
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.screen, Screen::Admin);
        press(&mut gui, KeyCode::Char('K'));
        press(&mut gui, KeyCode::Char('0'));
        assert_eq!(gui.screen, Screen::NowPlaying);
        press(&mut gui, KeyCode::Char('K'));
        press(&mut gui, KeyCode::Char('2'));
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::ALBUMS_NAV), "a digit is the Library's room");
        assert!(gui.device.page.is_none());
        press(&mut gui, KeyCode::Char('K'));
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

    #[test]
    fn entering_the_tab_stows_the_queue_and_lets_an_armed_pick_go() {
        let _en = english();
        // The queue panel holding the keys: K is none of its own, and the
        // tab takes them back from it.
        let mut queue = gui();
        queue.queue_open = true;
        queue.app.focus = Focus::Queue;
        press(&mut queue, KeyCode::Char('K'));
        assert_eq!(queue.screen, Screen::Device);
        assert_eq!(queue.app.focus, Focus::Browser, "the queue's keys stowed");
        ends(&queue).to_question();
        frame(&mut queue);
        assert_eq!(row(&render_at(&mut queue, 100, 30), 29).trim(), CONFIRM, "the page's hint, not the queue's");
        press(&mut queue, KeyCode::Enter);
        assert_eq!(ends(&queue).sent(), vec![WorkerCmd::Go { erase: false }], "and Enter the page's");

        // A Sonic Path pick: let go, the Library back at the room that asked.
        let mut sonic = gui();
        sonic.app.capture = Some(Capture::Sonic(SonicSide::Start));
        press(&mut sonic, KeyCode::Char('K'));
        assert_eq!(sonic.screen, Screen::Device);
        assert_eq!(sonic.app.capture, None, "the pick is let go");
        assert_eq!(sonic.active, SONIC_NAV, "home to the room that asked");

        // An Auto DJ opening-song pick likewise.
        let mut dj = gui();
        dj.app.capture = Some(Capture::DjSeed);
        press(&mut dj, KeyCode::Char('K'));
        assert_eq!(dj.app.capture, None);
    }

    #[test]
    fn on_the_tab_enter_and_esc_are_the_pages_even_with_a_pick_armed() {
        let _en = english();
        // However a pick came to be armed with the tab up, its keys and its
        // tips are not the tab's.
        let mut gui = at_question();
        gui.app.capture = Some(Capture::DjSeed);
        assert_eq!(row(&render_at(&mut gui, 100, 30), 29).trim(), CONFIRM, "the page's hint, not the pick's");
        press(&mut gui, KeyCode::Esc);
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit], "Esc cancels the question");
        assert_eq!(gui.app.capture, Some(Capture::DjSeed), "not the pick");

        let mut gui = at_question();
        gui.app.capture = Some(Capture::Sonic(SonicSide::Start));
        gui.config.gui.key_hints = true;
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(row(&buf, 29).trim(), CONFIRM);
        assert!(row(&buf, 1).contains("Pick the start song"), "the banner: {}", row(&buf, 1));
        assert!(!row(&buf, 1).contains("Esc cancels"), "but it names no Esc here: {}", row(&buf, 1));
        press(&mut gui, KeyCode::Enter);
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Go { erase: false }], "Enter answers the question");
    }

    // ── The page's own end ──────────────────────────────────────────────────

    #[test]
    fn esc_at_the_pages_base_and_its_close_lead_back_to_the_library_and_let_the_page_go() {
        let _en = english();
        // Esc where nothing holds the board: the page ends at once.
        let mut gui = on_tab();
        ends(&gui).no_device();
        frame(&mut gui);
        press(&mut gui, KeyCode::Esc);
        assert_eq!(gui.screen, Screen::Library, "Esc at the page's base is the way back");
        assert!(gui.device.page.is_none() && gui.device.parting.is_none());

        // Esc at the question: the board is restarted first, so the page
        // ends — and the tab with it — once the worker says so.
        press(&mut gui, KeyCode::Char('K'));
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
        assert!(gui.device.page.is_none() && gui.device.parting.is_none(), "the board is back: nothing kept");

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
        let queue_open = gui.queue_open;
        let codes = [
            KeyCode::Char('q'),
            KeyCode::Char('K'),
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
        assert!(gui.device.page.is_some() && writing(&gui), "the page is never let go of");

        // Ctrl+C is the one key out, as everywhere.
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
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Library);
        assert!(gui.device.page.is_none() && gui.device.parting.is_none(), "a failed write holds no board");
    }

    // ── The mini player ─────────────────────────────────────────────────────

    #[test]
    fn in_the_mini_player_the_unseen_page_takes_no_keys() {
        let _en = english();
        let mut gui = at_question();
        // The window narrows below the GUI's floor: the mini player, the
        // page out of sight.
        let text = all(&render_at(&mut gui, 90, 20));
        assert!(text.contains("Enlarge the terminal"), "{text}");
        assert!(gui.device.at.is_none());
        for code in [KeyCode::Enter, KeyCode::Char('e'), KeyCode::Char('r'), KeyCode::Char('l'), KeyCode::Esc] {
            assert!(!press(&mut gui, code));
            assert_eq!(gui.screen, Screen::Device, "{code:?}");
        }
        assert!(ends(&gui).sent().is_empty(), "no Go, no Quit reached the worker");
        assert!(!writing(&gui), "no write began out of sight");
        // Grown back, the page is as it was, and its keys are its own again.
        let text = all(&render_at(&mut gui, 100, 30));
        assert!(text.contains("[ ] Erase") && text.contains("▸ Show logs"), "e and l reached nothing:\n{text}");
        press(&mut gui, KeyCode::Enter);
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Go { erase: false }]);
    }

    #[test]
    fn a_write_in_the_mini_player_says_so_and_holds_the_pointer() {
        let _en = english();
        let mut gui = writing_gui();
        let area = Rect { x: 0, y: 0, width: 90, height: 20 };
        let buf = render_at(&mut gui, area.width, area.height);
        let text = all(&buf);
        assert!(!text.contains("Enlarge the terminal"), "{text}");
        let plan = super::super::mini::plan(&gui, area).mini;
        let line: Vec<&str> = plan.lines.iter().map(|(_, _, piece)| piece.as_str()).collect();
        assert_eq!(
            line.join(" "),
            "writing the MP3 player's firmware — please wait: unplugging it or quitting now would leave it half written"
        );
        for (_, y, piece) in &plan.lines {
            assert!(row(&buf, *y).contains(piece.as_str()), "{piece:?} on screen:\n{text}");
        }
        // The mini player's frames answer nothing while the page writes.
        let (x, y) = plan.transport.expect("the frames");
        let paused = gui.demo_paused;
        for dx in [2, 9, 16] {
            click(&mut gui, x + dx, y + 1);
        }
        assert_eq!(gui.demo_paused, paused, "Play did nothing");
        press(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.demo_paused, paused, "nor did Space");
        assert!(writing(&gui) && ends(&gui).sent().is_empty());
        // Once it is done, the mini player is itself again.
        ends(&gui).done();
        frame(&mut gui);
        let text = all(&render_at(&mut gui, area.width, area.height));
        assert!(text.contains("Enlarge the terminal"), "{text}");
    }

    // ── Leaving and quitting ────────────────────────────────────────────────

    #[test]
    fn leaving_at_the_question_tells_the_worker_to_let_go_and_keeps_the_page_until_it_has() {
        let _en = english();
        let mut gui = at_question();
        press(&mut gui, KeyCode::Char('T'));
        assert_eq!(gui.screen, Screen::Stats);
        assert!(gui.device.page.is_none(), "the tab has no page");
        assert!(gui.device.parting.is_some(), "the one it had is kept aside while the board is held");
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit], "told to let go, the page's own way");
        for _ in 0..3 {
            frame(&mut gui);
        }
        assert!(gui.device.parting.is_some() && !ends(&gui).page_gone(), "read every frame, kept until the worker answers");
        ends(&gui).cancelled();
        frame(&mut gui);
        assert!(gui.device.parting.is_none(), "the board is back: the page goes");
        assert!(ends(&gui).page_gone());
    }

    #[test]
    fn coming_straight_back_waits_for_the_last_page_to_let_the_board_go() {
        let _en = english();
        let mut gui = at_question();
        // The worker holds the port until it hears the Quit, then takes a
        // moment to reset the board; only then is the port free and Cancelled said.
        let held = Arc::new(AtomicBool::new(true));
        let port = held.clone();
        let first = gui.device.ends.take().unwrap();
        let worker = std::thread::spawn(move || {
            let heard = first.cmds.recv_timeout(Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(300));
            port.store(false, Ordering::SeqCst);
            first.cancelled();
            heard
        });
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Library);
        press(&mut gui, KeyCode::Char('K'));
        assert_eq!(gui.screen, Screen::Device, "the tab opens");
        assert!(gui.device.page.is_none() && gui.device.ends.is_none(), "but no worker of its own while the port is held");
        let buf = render_at(&mut gui, 100, 30);
        assert!(row(&buf, 2).contains("restarting the board…"), "{}", all(&buf));
        let t0 = Instant::now();
        while gui.device.page.is_none() {
            assert!(t0.elapsed() < Duration::from_secs(5), "the visit never got its page");
            assert!(gui.device.ends.is_none(), "no second worker meets the held port");
            frame(&mut gui);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!held.load(Ordering::SeqCst), "the new page came once the port was free");
        assert_eq!(worker.join().unwrap(), Ok(WorkerCmd::Quit));
        assert!(gui.device.parting.is_none());
        ends(&gui).to_question();
        frame(&mut gui);
        assert!(all(&render_at(&mut gui, 100, 30)).contains("Update ▸"), "a fresh page, its own question");
    }

    #[test]
    fn esc_while_the_tab_waits_for_the_last_page_leads_back() {
        let _en = english();
        let mut gui = at_question();
        press(&mut gui, KeyCode::Char('K'));
        press(&mut gui, KeyCode::Char('K'));
        assert!(gui.device.page.is_none() && gui.device.parting.is_some());
        press(&mut gui, KeyCode::Esc);
        assert_eq!(gui.screen, Screen::Library);
        assert!(gui.device.parting.is_some(), "the last page still lets the board go");
    }

    #[test]
    fn leaving_before_the_board_is_reached_lets_the_page_go_at_once() {
        let _en = english();
        // Watching the ports: the worker is told to stop, holds nothing, and
        // will open no port (it says "reaching" before its last look).
        let mut gui = on_tab();
        ends(&gui).no_device();
        frame(&mut gui);
        press(&mut gui, KeyCode::Char('K'));
        assert!(gui.device.page.is_none() && gui.device.parting.is_none());
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit]);
        assert!(ends(&gui).page_gone());

        // A worker that said it was reaching the board before the page read
        // it: the page's step lagged, the worker's word holds — kept aside.
        let mut gui = on_tab();
        ends(&gui).no_device();
        frame(&mut gui);
        ends(&gui).reaching();
        press(&mut gui, KeyCode::Char('K'));
        assert!(gui.device.parting.is_some(), "the worker may be on the port");
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit]);
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
        assert!(took >= Duration::from_millis(150) && took < Duration::from_secs(2), "it waited for the answer, no longer: {took:?}");
        assert!(gui.device.page.is_none());
    }

    #[test]
    fn quitting_while_the_board_is_being_reached_waits_for_espflash_and_the_reset() {
        let _en = english();
        let mut gui = on_tab();
        ends(&gui).reaching();
        frame(&mut gui);
        let ends = gui.device.ends.take().unwrap();
        // The worker as flow.rs runs it: deaf while espflash reaches the
        // board, then it looks for a Quit, resets the board and says so.
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            let heard = ends.cmds.try_recv();
            std::thread::sleep(Duration::from_millis(100));
            ends.cancelled();
            heard
        });
        let t0 = Instant::now();
        quit(&mut gui);
        let took = t0.elapsed();
        assert_eq!(worker.join().unwrap(), Ok(WorkerCmd::Quit), "the Quit waited for it");
        assert!(took >= Duration::from_millis(650), "the quit waited out the probe and the reset: {took:?}");
        assert!(took < Duration::from_secs(3), "and no longer: {took:?}");
    }

    #[test]
    fn quitting_with_a_page_left_behind_waits_for_it_too() {
        let _en = english();
        let mut gui = at_question();
        press(&mut gui, KeyCode::Char('T'));
        assert!(gui.device.parting.is_some());
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
        assert_eq!(worker.join().unwrap(), Ok(WorkerCmd::Quit));
        assert!(took >= Duration::from_millis(150) && took < Duration::from_secs(2), "{took:?}");
        assert!(gui.device.parting.is_none());
    }

    #[test]
    fn quitting_at_the_question_with_no_answer_waits_its_bound_and_no_longer() {
        let _en = english();
        let mut gui = at_question();
        let bound = Duration::from_millis(400);
        let t0 = Instant::now();
        let_go_within(&mut gui, bound);
        let took = t0.elapsed();
        assert!(took >= bound - Duration::from_millis(50), "the bound: {took:?}");
        assert!(took < bound + Duration::from_secs(2), "and no longer: {took:?}");
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit]);
    }

    #[test]
    fn quitting_with_no_page_or_no_board_held_does_not_wait() {
        let _en = english();
        let t0 = Instant::now();
        // The Library, no page.
        let mut gui = gui();
        quit(&mut gui);
        // The tab watching for a board: the worker is told to stop before
        // it opens any port, and nothing waits for it.
        let mut gui = on_tab();
        ends(&gui).no_device();
        frame(&mut gui);
        quit(&mut gui);
        assert_eq!(ends(&gui).sent(), vec![WorkerCmd::Quit]);
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

        press(&mut gui, KeyCode::Char('K'));
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
