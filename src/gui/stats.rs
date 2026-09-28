//! The Stats screen: the stats page (`src/admin/stats.rs`, `mstream-player
//! stats`) hosted whole under the GUI's top bar (docs/ux-contracts/
//! stats-screen.md) — the same page, the same keys, on the session the GUI
//! already holds. Its client is built from the App's reach, so a Quick
//! Connect tunnel or a peer's parent serves it as it serves the queue. The
//! page keeps its own surface; the screen hands it the keys and the pointer
//! below the bar, folds in and pumps its worker each frame, and puts its
//! hint on the GUI's footer.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use rust_i18n::t;

use super::{Act, DJ_NAV, Gui, Screen, put};
use crate::admin::stats::{self as page, Page};
use crate::admin::{Outcome, Screen as Hosted};
use crate::api::Client;
use crate::kit::dim;
use crate::tui::app::Origin;

/// The screen's state: the page while a session gives it a server, else
/// the reason there is none.
#[derive(Default)]
pub(crate) struct StatsUi {
    pub page: Option<Page>,
    why: Option<String>,
}

/// Open the screen: the page on the session's server — a peer session's
/// parent, where its plays are reported (play-reporting contract, clause
/// 8) — or the one line saying why there is no page.
pub(crate) fn open(gui: &mut Gui) {
    gui.stats.page = None;
    gui.stats.why = None;
    if !gui.app.connected {
        gui.stats.why = Some(t!("gui.stats.no_session").to_string());
        return;
    }
    let app = &gui.app;
    let target = match &app.session.peer {
        Some((parent, _)) => Origin { server: parent.clone(), peer: None },
        None => app.origin(),
    };
    let reach = match app.reach(&target) {
        Ok(reach) => reach,
        Err(why) => {
            gui.stats.why = Some(why);
            return;
        }
    };
    let client = match Client::new_with(&reach.base, reach.self_signed) {
        Ok(client) => client.with_token(reach.token).with_local_token(reach.local_token),
        Err(e) => {
            gui.stats.why = Some(e.to_string());
            return;
        }
    };
    gui.stats.page = Some(page::start(client, app.session.username.clone()).hosted());
}

/// Leaving drops the page — and its worker with it.
pub(crate) fn close(gui: &mut Gui) {
    gui.stats.page = None;
    gui.stats.why = None;
}

/// The session changed under the screen: the page is the new server's.
pub(crate) fn reopen(gui: &mut Gui) {
    if gui.screen == Screen::Stats {
        open(gui);
    }
}

/// `view` is the Now Playing screen's rect, which keeps the top bar's row
/// for the TUI view's own title; the page wants the rows under the bar.
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, view: Rect) {
    let view = Rect { y: view.y + 1, height: view.height.saturating_sub(1), ..view };
    match gui.stats.page.as_mut() {
        Some(page) => page::render_hosted(frame, page, view),
        None => {
            let why = gui.stats.why.clone().unwrap_or_default();
            put(frame, view.x + 2, view.y + 1, &why, dim());
        }
    }
}

/// The screen's keys (contract clause 4). Returns true to quit.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    let modal = gui.stats.page.as_ref().is_some_and(Page::modal_open);
    if !modal {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('T') => return gui.act(Act::Screen(Screen::Library)),
            KeyCode::Char('0') => return gui.act(Act::Screen(Screen::NowPlaying)),
            KeyCode::Char('V') => return gui.act(Act::VizWindow),
            KeyCode::Char(c @ '1'..='9') => return gui.act(Act::Nav(c as usize - '1' as usize)),
            KeyCode::Char('D') => return gui.act(Act::Nav(DJ_NAV)),
            _ => {}
        }
    }
    let Some(page) = gui.stats.page.as_mut() else {
        if key.code == KeyCode::Esc {
            return gui.act(Act::Screen(Screen::Library));
        }
        return false;
    };
    // The page first: Esc closes its modal or lets go of its row; with
    // nothing left to close the page says Quit, which here is the way back.
    if matches!(Hosted::key(page, key), Some(Outcome::Quit)) {
        return gui.act(Act::Screen(Screen::Library));
    }
    false
}

/// The footer's line (contract clause 6): the page's own hint, then the
/// way back while nothing on the page is selected or open.
pub(crate) fn tips(gui: &Gui) -> String {
    let back = t!("gui.tips.stats_back").to_string();
    let Some(page) = gui.stats.page.as_ref() else { return back };
    let hint = page::footer_hint(page);
    if page.modal_open() || page.sel.is_some() { hint } else { format!("{hint} · {back}") }
}

/// The pointer below the top bar, on the page's own surface — the hub's
/// loop, step for step (contract clause 5). True when the event was the
/// screen's to take, whether or not a page was up to take it.
pub(crate) fn pointer(gui: &mut Gui, mouse: MouseEvent) -> bool {
    if gui.screen != Screen::Stats || mouse.row == 0 || gui.modal_open() {
        return false;
    }
    let at = Position { x: mouse.column, y: mouse.row };
    let Some(page) = gui.stats.page.as_mut() else { return true };
    let mut back = false;
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !page.ui().begin_press(at) {
                return true;
            }
            if let Some(act) = page.ui().hit(at)
                && matches!(<Page as Hosted>::act(page, act), Some(Outcome::Quit))
            {
                back = true;
            }
            page.ui().arm_bars(at);
        }
        MouseEventKind::Moved => page.ui().motion(at),
        MouseEventKind::Drag(_) => {
            page.ui().motion(at);
            if let Some(act) = page.ui().drag_action(at) {
                <Page as Hosted>::act(page, act);
            }
        }
        MouseEventKind::Up(_) => page.ui().release(),
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            page.ui().pointer = Some(at);
            Hosted::wheel(page, mouse.kind == MouseEventKind::ScrollUp, at);
        }
        _ => {}
    }
    if back {
        gui.act(Act::Screen(Screen::Library));
    }
    true
}

/// The page's worker's answers, before the draw — the hub's order, so an
/// answer is on screen the frame it lands (performance audit #82). True
/// when anything came: a frame to draw.
pub(crate) fn absorb(gui: &mut Gui) -> bool {
    gui.stats.page.as_mut().is_some_and(Hosted::absorb)
}

/// Whether the page's last frame is out of date on its clocks alone (the
/// kit's [`crate::kit::Surface::stale`]) — only while it is on screen.
pub(crate) fn stale(gui: &mut Gui) -> bool {
    let on_screen = gui.screen == Screen::Stats;
    on_screen && gui.stats.page.as_mut().is_some_and(|page| page.ui().stale())
}

/// Whether the page has a call out with its worker — the GUI's loop waits
/// briskly for the answer while it does (`kit::pace`).
pub(crate) fn awaiting(gui: &Gui) -> bool {
    gui.stats.page.as_ref().is_some_and(Hosted::awaiting)
}

/// What the page's per-frame duties found.
#[derive(Default)]
pub(crate) struct Duties {
    /// The pointer is over one of the page's clickables: the hand.
    pub over: bool,
    /// A held control stepped: a frame to draw.
    pub stepped: bool,
}

/// The page's own per-frame duties, after the draw: its worker's next
/// call, a held control, the tooltip clock.
pub(crate) fn frame(gui: &mut Gui) -> Duties {
    let on_screen = gui.screen == Screen::Stats;
    let Some(page) = gui.stats.page.as_mut() else { return Duties::default() };
    Hosted::pump(page);
    let mut stepped = false;
    if let Some(act) = page.ui().hold_action() {
        <Page as Hosted>::act(page, act);
        stepped = true;
    }
    page.ui().dwell_tick();
    Duties { over: on_screen && page.ui().hovering_clickable(), stepped }
}
