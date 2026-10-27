//! The wizard as a window's face (gui/window/face.rs): `setup --window` and
//! `qr --window`, the one [`Wizard`] the terminal drives, drawn in the
//! player's own window instead. The two halves are the terminal loop's
//! (`setup::frame`, `setup::input`); what the window adds is the cursor for
//! the pointer's hand, pictures as textures over the grid, and an input
//! method for the field with the keyboard.

use std::io;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::Event as TermEvent;
use ratatui::layout::{Position, Rect};

use super::{Done, Job, Outcome, Wizard, render, session_title};
use crate::api::Client;
use crate::gui::window::WindowTerminal;
use crate::gui::window::face::Face;
use crate::gui::{Flow, Host};
use crate::tui::graphics::{Graphics, PictureHost};

/// The wizard and the two ends of its worker, as `run_tui` holds them.
///
/// The window's input translation needs no arm for the wizard. A paste and
/// an input method's commit arrive as one key event a character
/// (window/input.rs), which `handle_key` types into the field with the
/// keyboard as a terminal's typing does, and the window pastes only while
/// [`Face::caret_at`] names such a field. Ctrl+C arrives as `c` with
/// CONTROL and quits, as in a terminal. The copy chord (Cmd+C on a Mac,
/// Ctrl+Shift+C or Ctrl+Insert elsewhere) is the host's and never reaches
/// the input half; the wizard copies nothing anywhere, so the default
/// swallows it. A resize arrives as `TermEvent::Resize`, which the wizard
/// ignores, because `render` reads the frame's area on every draw.
///
/// Every way out exits 0: each [`Outcome`] the input half returns, and the
/// close button, which ends the loop without passing through it. A Cmd-Q
/// on a Mac ends the process inside AppKit once the window's teardown
/// returns, so it never reaches [`Face::exit_code`]; that exit is 0 too.
/// The host keeps its own codes: [`NO_WINDOW`](crate::gui::window::NO_WINDOW)
/// (3) when no window opens, which a launcher takes as "use the terminal
/// route", and 1 when a frame fails.
pub(crate) struct WizardFace {
    wizard: Wizard,
    to_worker: Sender<(Arc<Client>, Job)>,
    from_worker: Receiver<Done>,
}

impl WizardFace {
    pub(super) fn new(
        wizard: Wizard,
        to_worker: Sender<(Arc<Client>, Job)>,
        from_worker: Receiver<Done>,
    ) -> Self {
        WizardFace { wizard, to_worker, from_worker }
    }
}

impl Face for WizardFace {
    /// The terminal session's title, which the window carries instead.
    fn title(&self) -> &'static str {
        session_title(self.wizard.standalone)
    }

    // The grid is the host's 100×30: the wizard needs 58×20, and the
    // hosted Done page's two columns fit the wordmark band and a code of
    // up to 15 rows in it.

    fn frame(
        &mut self,
        terminal: &mut WindowTerminal,
        host: &mut dyn Host,
    ) -> io::Result<Duration> {
        super::frame(
            terminal,
            &mut self.wizard,
            &mut |hand| host.pointer(hand),
            &self.to_worker,
            &self.from_worker,
        )
    }

    /// Matched whole, so an [`Outcome`] added later (a finished wizard told
    /// from an abandoned one) is decided here, beside [`Face::exit_code`].
    fn input(&mut self, event: TermEvent) -> Flow {
        match super::input(&mut self.wizard, event) {
            Some(Outcome::Quit) => Flow::Quit,
            None => Flow::Continue,
        }
    }

    fn caret_at(&self) -> Option<Position> {
        self.wizard.ui.caret_at()
    }

    fn set_composition(&mut self, text: &str) {
        self.wizard.ui.set_composition(text);
    }

    /// The window draws pictures, so the Done page takes its two columns
    /// from the first frame and both pictures, the wordmark and the code,
    /// are the host's to draw. Each of the wizard's modals registers its
    /// footprint with the Surface as it draws, and the watch hands it on,
    /// so the host never paints the wordmark over the directory browser;
    /// the wizard's tooltip registers none (`render` says why).
    fn host_pictures(&mut self, host: Arc<dyn PictureHost>, overlays: Box<dyn Fn(Rect) + 'static>) {
        self.wizard.adopt_graphics(Graphics::hosted(host));
        self.wizard.ui.watch_overlays(overlays);
    }

    /// The op each entry queued (the wizard's ping, the page's Quick
    /// Connect load) on the wire while the window and its renderer are
    /// built, as the GUI's connect is. Ops are single-flight, so frame 1's
    /// dispatch finds it in flight and sends nothing, and frame 1's drain
    /// folds in the answer when it is back.
    fn send_early(&mut self) {
        self.wizard.dispatch_queued(&self.to_worker);
    }

    /// Nothing to put away: the wizard writes what it keeps as it goes
    /// (the server's state over its API, the session at sign-in), and the
    /// worker thread ends once the face is dropped with its sender, as it
    /// does when `run_tui` returns.
    fn finish(&mut self) {}

    fn hit_debug(&self, at: Position) -> Option<String> {
        self.wizard.ui.hit(at).map(|act| format!("{act:?}"))
    }

    fn render_test(&mut self, frame: &mut Frame<'_>) {
        render(frame, &mut self.wizard);
    }
}

/// A face as a test opens it, with the worker's two ends held by the test
/// rather than a thread: the jobs the face sent, and the sender of the
/// worker's answers.
#[cfg(test)]
type Opened = (WizardFace, Receiver<(Arc<Client>, Job)>, Sender<Done>);

#[cfg(test)]
impl WizardFace {
    /// A face on a fresh wizard as its entry starts it: `setup` on the
    /// folders, `qr` on the standalone Done page.
    pub(crate) fn opened(standalone: bool) -> Opened {
        let mut wizard = Wizard::new(Client::new("http://127.0.0.1:9").expect("client"));
        if standalone {
            wizard.standalone = true;
            wizard.screen = super::Screen::Done;
        }
        let (to_worker, jobs) = std::sync::mpsc::channel();
        let (answers, from_worker) = std::sync::mpsc::channel();
        (WizardFace::new(wizard, to_worker, from_worker), jobs, answers)
    }

    /// The window's pictures handed over as `window::run` hands them. The
    /// window pins the truecolour palette before anything asks for one, so
    /// its wordmark is flattened onto that ground; a test binary may have
    /// resolved a palette with no ground from a bare environment first,
    /// and then the wordmark is built here on the window's ground, so the
    /// layout under test is the window's either way.
    pub(crate) fn host_pictures_as_the_window(
        &mut self,
        host: Arc<dyn PictureHost>,
        overlays: Box<dyn Fn(Rect) + 'static>,
    ) {
        crate::kit::theme::pin_truecolor();
        crate::kit::theme::pin_modern_glyphs();
        self.host_pictures(host, overlays);
        assert_eq!(
            self.wizard.logo_art.is_some(),
            crate::kit::theme::th().ground_rgb.is_some(),
            "a wordmark wherever there is a ground to flatten it onto"
        );
        if self.wizard.logo_art.is_none() {
            eprintln!("the palette has no ground here: the wordmark is built on the window's");
            self.wizard.logo_art = super::logo_art((0x12, 0x13, 0x1c));
        }
    }

    /// The server's directory browser over the folders screen, opened by a
    /// listing's answer as it is where the system has no folder picker.
    pub(crate) fn open_the_browser(&mut self) {
        let listing = crate::api::types::DirListing {
            path: "/music".to_string(),
            directories: vec![crate::api::types::DirEntry { name: "Albums".to_string() }],
            files: Vec::new(),
        };
        self.wizard.apply(Done::Browsed(Ok(listing)));
        assert!(matches!(self.wizard.modal, super::Modal::Browser(_)));
    }

    /// The server's Quick Connect answer, with `ticket` as its code, folded
    /// in as the frame half folds the worker's.
    pub(crate) fn answer_ticket(&mut self, ticket: &str) {
        let status = crate::api::types::IrohStatus {
            enabled: true,
            qr: Some(ticket.to_string()),
            ..Default::default()
        };
        self.wizard.apply(Done::Iroh(Ok(status)));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rust_i18n::t;

    use super::super::tests::{IoTest, LOCALE_LOCK, click_rect, row_text};
    use super::super::{Act, LOGO, LoginField, Modal, Op, POLL, Screen, done_buttons};
    use super::*;
    use crate::tui::art::Art;

    /// What the window's host is told as a frame draws, in order.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Told {
        /// A picture, placed in this band.
        Placed(Rect),
        /// Something drawn over the base layer, with this footprint.
        Over(Rect),
    }

    /// A window's picture host that only remembers what it was told: where
    /// each picture was placed and, through the watch the face is handed
    /// beside it, each overlay's footprint, in the order they came.
    #[derive(Default)]
    struct RecordingHost(Mutex<Vec<Told>>);

    impl PictureHost for RecordingHost {
        fn place(&self, area: Rect, _grid: Rect, _art: &Art) {
            self.0.lock().unwrap().push(Told::Placed(area));
        }
    }

    impl RecordingHost {
        /// Everything told since the last ask.
        fn told(&self) -> Vec<Told> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }

        /// What was placed since the last ask.
        fn take(&self) -> Vec<Rect> {
            let placed = |told| if let Told::Placed(rect) = told { Some(rect) } else { None };
            self.told().into_iter().filter_map(placed).collect()
        }
    }

    /// The face drawn as the fidelity dump draws it, into a `TestBackend`
    /// of the window's grid.
    fn draw(face: &mut WizardFace) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| face.render_test(frame)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height).map(|y| row_text(buf, y)).collect()
    }

    fn on_a_recording_host(face: &mut WizardFace) -> Arc<RecordingHost> {
        let host = Arc::new(RecordingHost::default());
        let watching = host.clone();
        let overlays = move |rect| watching.0.lock().unwrap().push(Told::Over(rect));
        face.host_pictures_as_the_window(host.clone(), Box::new(overlays));
        host
    }

    /// The window's grid is 100×30, where a terminal's half-block code
    /// (39 rows for a real ticket) never fits and the stacked page says so
    /// in `done.too_short`. A window draws pictures, so the Quick Connect
    /// page is two columns from its first frame, before the server has
    /// answered, and the two pictures are placed with the host, the
    /// wordmark's band and then the code's, both in the left column: 30
    /// cells from the page's left edge, the right column's buttons beside
    /// them. The wizard's folders screen places its wordmark the same way
    /// and draws no figlet under it.
    #[test]
    fn the_hosted_quick_connect_page_is_two_column_at_the_windows_grid() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut face, _jobs, _answers) = WizardFace::opened(true);
        let host = on_a_recording_host(&mut face);
        assert!(face.wizard.done_two_column(), "a hosted page is two columns before any data");
        assert!(face.wizard.qr.is_none() && face.wizard.qr_art.is_none());

        // The Done page's column starts two cells in; its left column is
        // 30 wide at this grid (`draw_done`).
        let (left, left_end) = (2u16, 2u16 + 30);
        let in_left =
            |rect: &Rect| rect.x >= left && rect.right() <= left_end && rect.bottom() <= 30;
        draw(&mut face);
        let placed = host.take();
        assert_eq!(placed.len(), 1, "the wordmark alone until the code arrives: {placed:?}");
        assert_eq!(placed[0], Rect { x: left, y: 2, width: 30, height: 4 });

        face.answer_ticket("iroh-ticket-abc123");
        assert!(face.wizard.qr_art.is_some());
        let buf = draw(&mut face);
        let placed = host.take();
        assert_eq!(placed.len(), 2, "the wordmark, then the code: {placed:?}");
        assert!(placed.iter().all(in_left), "both in the left column: {placed:?}");
        let (wordmark, code) = (placed[0], placed[1]);
        assert_eq!(wordmark.height, 4);
        assert_eq!(code.y, wordmark.bottom() + 1, "the code under the wordmark");
        assert_eq!(code.height, 15, "the code's band is as tall as the page lets it be");
        let text = rows(&buf);
        let too_short = t!("done.too_short").to_string();
        assert!(text.iter().all(|row| !row.contains(&too_short)), "the stacked page's apology");
        for (label, _) in done_buttons() {
            let row = text.iter().find(|row| row.contains(&label)).expect("a button");
            let x = row.find(&label).unwrap();
            let column = crate::kit::width(&row[..x]);
            assert!(column >= usize::from(left_end), "{label} is in the right column");
        }
        // The picture's cells are left blank for the texture.
        for at in code.positions() {
            assert_eq!(buf[at].symbol(), " ", "{at:?} under the code is not blank");
        }

        // The wizard's own first screen: the wordmark's band, no figlet.
        let (mut face, _jobs, _answers) = WizardFace::opened(false);
        let host = on_a_recording_host(&mut face);
        let buf = draw(&mut face);
        let placed = host.take();
        assert_eq!(placed.len(), 1, "{placed:?}");
        assert_eq!(placed[0].height, LOGO.len() as u16, "the figlet's band");
        let text = rows(&buf);
        for line in LOGO {
            assert!(text.iter().all(|row| !row.contains(line.trim())), "figlet drawn: {line}");
        }
    }

    /// What the host reads of the face between frames is the last frame's:
    /// the login field with the keyboard notes its caret, so the window
    /// turns its input method on there and lets a paste in; a composition
    /// the input method hands back is drawn in that field; and a modal
    /// with no field (the language list) clears the note, so the input
    /// method goes off and a paste does nothing.
    #[test]
    fn the_window_face_reports_its_fields() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut face, _jobs, _answers) = WizardFace::opened(false);
        face.wizard.screen = Screen::Login;
        face.wizard.field = LoginField::Username;
        draw(&mut face);
        let card = click_rect(&face.wizard, Act::Focus(LoginField::Username));
        let field = Position { x: card.x + 1, y: card.y + 1 };
        assert_eq!(face.caret_at(), Some(field), "an empty field's caret is its first cell");

        face.set_composition("にほ");
        let buf = draw(&mut face);
        let row = row_text(&buf, field.y);
        assert!(row.contains("にほ"), "the composition is drawn in the field: {row}");
        let after = Position { x: field.x + 4, y: field.y };
        assert_eq!(face.caret_at(), Some(after), "the caret after its four cells");
        face.set_composition("");

        face.wizard.modal = Modal::Language(0);
        draw(&mut face);
        assert_eq!(face.caret_at(), None, "the language list has the keys and no field");
    }

    /// The window's input half is the wizard's: a key types into the field
    /// with the keyboard, a resize asks nothing, Ctrl+C and the page's Esc
    /// end the window, and a press is reported for the script's dumps as
    /// what the last frame registered under it.
    #[test]
    fn the_window_faces_input_is_the_wizards() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let key = |code, modifiers| TermEvent::Key(KeyEvent::new(code, modifiers));
        let (mut face, _jobs, _answers) = WizardFace::opened(false);
        face.wizard.screen = Screen::Login;
        assert_eq!(face.input(key(KeyCode::Char('a'), KeyModifiers::NONE)), Flow::Continue);
        assert_eq!(face.wizard.username.value(), "a");
        assert_eq!(face.input(TermEvent::Resize(120, 40)), Flow::Continue);
        draw(&mut face);
        let skip = click_rect(&face.wizard, Act::SkipLogin);
        assert_eq!(face.hit_debug(skip.as_position()).as_deref(), Some("SkipLogin"));
        assert_eq!(face.hit_debug(Position { x: 0, y: 29 }), None);
        assert_eq!(face.input(key(KeyCode::Char('c'), KeyModifiers::CONTROL)), Flow::Quit);

        let (mut face, _jobs, _answers) = WizardFace::opened(true);
        assert_eq!(face.input(key(KeyCode::Esc, KeyModifiers::NONE)), Flow::Quit);
        assert_eq!(face.exit_code(), 0, "every way out is 0");
        assert!(!face.copy(), "nothing to copy: the chord is swallowed");
    }

    /// Every modal the wizard draws tells the window its footprint, the
    /// whole frame it drew, after the page beneath has placed its pictures:
    /// the window stands down a picture placed before an overlay that
    /// touches it (`Board::overlay`). Each footprint is where that modal's
    /// rounded corners are on the grid.
    #[test]
    fn every_wizard_modal_tells_the_window_its_footprint() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // What opens the modal over the page.
        type Open = fn(&mut WizardFace);
        let opens: [(Screen, Open); 4] = [
            (Screen::Login, |face| face.wizard.modal = Modal::SkipWarning),
            (Screen::Folders, |face| face.wizard.modal = Modal::Language(0)),
            (Screen::Folders, |face| face.wizard.modal = Modal::PathEntry(Default::default())),
            (Screen::Folders, WizardFace::open_the_browser),
        ];
        for (screen, open) in opens {
            let (mut face, _jobs, _answers) = WizardFace::opened(false);
            let host = on_a_recording_host(&mut face);
            face.wizard.screen = screen;
            open(&mut face);
            let buf = draw(&mut face);
            let told = host.told();
            let modal = &face.wizard.modal;
            let Some(&Told::Over(rect)) = told.last() else { panic!("{modal:?} told {told:?}") };
            let overs = told.iter().filter(|told| matches!(told, Told::Over(_))).count();
            assert_eq!(overs, 1, "{modal:?}: {told:?}");
            let corner = (rect.right() - 1, rect.bottom() - 1);
            assert_eq!(buf[(rect.x, rect.y)].symbol(), "╭", "{modal:?} at {rect:?}");
            assert_eq!(buf[corner].symbol(), "╯", "{modal:?} at {rect:?}");
        }
    }

    /// The directory browser opens over the folders wordmark's last row at
    /// the window's grid. On the frame it opens the wordmark is placed, by
    /// last frame's footprints, and the browser's footprint is told after
    /// it, so the window leaves the picture unpainted, and the frame half
    /// asks for the next frame at once; that frame places no picture and
    /// draws the figlet above the browser instead. Once the browser closes
    /// the figlet stays for the one frame drawn by its footprint, again
    /// asked for at once, and the frame after places the picture again.
    #[test]
    fn a_modal_over_the_wordmark_stands_it_down_and_the_figlet_stands_in() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut face, _jobs, _answers) = WizardFace::opened(false);
        let host = on_a_recording_host(&mut face);
        let mut terminal = Terminal::new(IoTest(TestBackend::new(100, 30))).unwrap();
        let mut frame = |face: &mut WizardFace| {
            let (to_worker, from_worker) = (&face.to_worker, &face.from_worker);
            let wizard = &mut face.wizard;
            let wait =
                super::super::frame(&mut terminal, wizard, &mut |_| {}, to_worker, from_worker);
            (wait.unwrap(), terminal.backend().0.buffer().clone())
        };

        let (wait, buf) = frame(&mut face);
        let told = host.told();
        let [Told::Placed(band)] = told[..] else { panic!("the wordmark alone: {told:?}") };
        assert_eq!(wait, POLL);
        let figlet = |buf: &Buffer, rows: u16| {
            (0..rows).all(|i| row_text(buf, band.y + i).contains(LOGO[usize::from(i)].trim()))
        };
        assert!(!figlet(&buf, 1), "a picture, not the figlet");

        face.open_the_browser();
        let (wait, _) = frame(&mut face);
        let told = host.told();
        let [Told::Placed(placed), Told::Over(browser)] = told[..] else { panic!("{told:?}") };
        assert_eq!(placed, band);
        assert!(browser.intersects(band), "the browser crosses the band: {browser:?} {band:?}");
        assert!(browser.y > band.y, "the browser leaves the band's top rows: {browser:?}");
        assert_eq!(wait, Duration::ZERO, "the frame drawn by the browser's footprint, now");

        let (wait, buf) = frame(&mut face);
        assert_eq!(host.told(), [Told::Over(browser)], "no picture under the browser");
        assert!(figlet(&buf, browser.y - band.y), "the figlet stands in: {:#?}", rows(&buf));
        assert_eq!(wait, POLL);

        let esc = TermEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(face.input(esc), Flow::Continue);
        assert_eq!(face.wizard.modal, Modal::None);
        let (wait, buf) = frame(&mut face);
        assert_eq!(host.told(), [], "drawn by the browser's footprint: no picture yet");
        assert!(figlet(&buf, LOGO.len() as u16), "the whole figlet: {:#?}", rows(&buf));
        assert_eq!(wait, Duration::ZERO, "the frame drawn by no footprint, now");

        let (wait, buf) = frame(&mut face);
        assert_eq!(host.told(), [Told::Placed(band)], "the picture again");
        assert!(!figlet(&buf, 1));
        assert_eq!(wait, POLL);
    }

    /// The window's title is the terminal session's, which a launcher and
    /// anyone scripting against window titles already know: English, and
    /// one for each entry. Both open at the host's own grid.
    #[test]
    fn the_faces_titles_are_the_sessions() {
        let (wizard, _jobs, _answers) = WizardFace::opened(false);
        assert_eq!(wizard.title(), session_title(false));
        assert_eq!(wizard.title(), "mStream Setup");
        let (page, _jobs, _answers) = WizardFace::opened(true);
        assert_eq!(page.title(), session_title(true));
        assert_eq!(page.title(), "mStream Quick Connect");
        assert_eq!((wizard.grid(), page.grid()), ((100, 30), (100, 30)));
    }

    /// The op an entry queued goes to the worker before the first frame:
    /// once, since a second early send and the first frame's dispatch find
    /// it in flight, and the answer it brings back is folded in by that
    /// first frame.
    #[test]
    fn send_early_puts_the_queued_op_on_the_wire() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut face, jobs, answers) = WizardFace::opened(true);
        face.wizard.queue(Op::LoadDone, "fetching");
        assert!(jobs.try_recv().is_err(), "nothing sent before the host asks");
        face.send_early();
        let (_, job) = jobs.try_recv().expect("the op is on the wire");
        assert!(matches!(job, Job::Plain(Op::LoadDone)));
        face.send_early();
        assert!(jobs.try_recv().is_err(), "single-flight: sent once");

        let status = crate::api::types::IrohStatus {
            enabled: true,
            qr: Some("ticket".to_string()),
            ..Default::default()
        };
        answers.send(Done::Iroh(Ok(status))).unwrap();
        let mut terminal = Terminal::new(IoTest(TestBackend::new(100, 30))).unwrap();
        let (to_worker, from_worker) = (&face.to_worker, &face.from_worker);
        super::super::frame(&mut terminal, &mut face.wizard, &mut |_| {}, to_worker, from_worker)
            .unwrap();
        assert!(face.wizard.qr.is_some(), "the first frame folded the answer in");
        assert!(jobs.try_recv().is_err(), "and sent nothing again");
    }
}
