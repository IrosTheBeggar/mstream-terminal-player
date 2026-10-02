//! The Stats screen: the stats page (`src/admin/stats.rs`, `mstream-player
//! stats`) hosted whole under the GUI's top bar (docs/ux-contracts/
//! stats-screen.md) — the same page, the same keys, on the session the GUI
//! already holds. Its client is built from the App's reach, so a Quick
//! Connect tunnel or a peer's parent serves it as it serves the queue. The
//! page keeps its own surface; the screen hands it the keys and the pointer
//! below the bar, pumps its worker each frame, and puts its hint on the
//! GUI's footer.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use rust_i18n::t;

use super::{Act, DJ_NAV, Gui, Screen, put};
use crate::admin::stats::{self as page, Page};
use crate::admin::{Outcome, Screen as Hosted, drive_pointer};
use crate::api::Client;
use crate::kit::dim;
use crate::tui::app::{App, Origin, Reach};

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
    let client = match session_reach(&gui.app).and_then(|reach| reach_client(&reach)) {
        Ok(client) => client,
        Err(why) => {
            gui.stats.why = Some(why);
            return;
        }
    };
    gui.stats.page = Some(page::start(client, gui.app.session.username.clone()).hosted());
}

/// How the session's server is reached for a page of the hub's: the
/// session's origin, or a peer session's parent, which keeps the plays
/// and the admin rooms a peer does not have. `Err` carries the reason in
/// words (a tunnel that is down, say). The Admin tab reaches its rooms
/// and its log the same way.
pub(super) fn session_reach(app: &App) -> Result<Reach, String> {
    let target = match &app.session.peer {
        Some((parent, _)) => Origin { server: parent.clone(), peer: None },
        None => app.origin(),
    };
    app.reach(&target)
}

/// A client for `reach`: its base, its trust, its token and, over a
/// tunnel, the bridge's loopback token. A `Client` is built per page,
/// since each page's worker owns its own.
pub(super) fn reach_client(reach: &Reach) -> Result<Client, String> {
    Client::new_with(&reach.base, reach.self_signed)
        .map(|client| client.with_token(reach.token.clone()).with_local_token(reach.local_token.clone()))
        .map_err(|e| e.to_string())
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
            // The Admin tab, as from the Library (admin-screen contract,
            // clause 1).
            KeyCode::Char('M') => return gui.act(Act::Screen(Screen::Admin)),
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
/// own routine, so the two cannot drift apart (contract clause 5). A GUI
/// modal or the header's server menu owns the pointer while it is open,
/// and the page lets go of it. True when the event was the screen's to
/// take, whether or not a page was up to take it.
pub(crate) fn pointer(gui: &mut Gui, mouse: MouseEvent) -> bool {
    if gui.screen != Screen::Stats || mouse.row == 0 || gui.modal_open() || gui.servers.drop_open {
        return false;
    }
    let Some(page) = gui.stats.page.as_mut() else { return true };
    if matches!(drive_pointer(page, mouse), Some(Outcome::Quit)) {
        gui.act(Act::Screen(Screen::Library));
    }
    true
}

/// The page's own per-frame duties, after the draw: its worker's answers,
/// a held control, the tooltip clock — and whether the pointer is over one
/// of its clickables, for the hand.
pub(crate) fn frame(gui: &mut Gui) -> bool {
    let on_screen = gui.screen == Screen::Stats;
    let Some(page) = gui.stats.page.as_mut() else { return false };
    Hosted::pump(page);
    if let Some(act) = page.ui().hold_action() {
        <Page as Hosted>::act(page, act);
    }
    page.ui().dwell_tick();
    on_screen && page.ui().hovering_clickable()
}
