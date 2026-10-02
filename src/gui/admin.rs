//! The Admin screen: the admin panel's six rooms (`src/admin/*`, what
//! `mstream-player admin` opens) hosted under the GUI's top bar, with a
//! hallway that names them all and the server's own log beside the room
//! when the window has room for it (docs/ux-contracts/admin-screen.md).
//! The rooms are the hub's, whole: each draws into the area this screen
//! hands it through the hub's `HostedRoom` face, on a client built from
//! the App's reach, so a tunnel or a peer's parent serves them as it
//! serves the queue. One room is alive at a time, and the log polls on a
//! thread of its own (`server_log`).
//!
//! Three things can hold the keyboard here: the hallway, the room and the
//! log. A focused room keeps every key but the two the screen needs to
//! move between them, Tab and `L`, and those only while the room holds
//! nothing that wants them; a room that holds every key has the focus,
//! wherever it stood. The pointer is the room's inside its area, and under
//! the top bar while its modal is up, and the GUI's everywhere else. The
//! geometry, the key routing and the focus cycle are pure functions of
//! their inputs, so the tests pin them by window size and by key before
//! any frame is drawn.

use std::time::Instant;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use rust_i18n::t;

use super::server_log::{self, LogAct, LogKey, LogUi, Look};
use super::stats::{reach_client, session_reach};
use super::{Act, DJ_NAV, Gui, Screen, accent, bar, bright_bold, forward_glyph, put, sel};
use crate::admin::{Claim, HostedRoom, Outcome, RoomId, open_room};
use crate::kit::{Grip, blank, dim, width, wrap_words};
use crate::tui::app::{App, Reach};

/// The hallway's rule, and the first column of the room's area after it.
const RULE_X: u16 = 16;
const ROOM_X: u16 = 17;
/// The room's width while the log is docked beside it as a column.
const DOCKED_W: u16 = 100;
/// From this many columns the log docks beside the room; narrower, from
/// this many rows, it is a band under the room; smaller still, a room of
/// its own on the hallway's Log row.
const COLUMN_FROM: u16 = 160;
const BAND_FROM: u16 = 48;
/// The band's rows: its rule, the log's header and seven lines.
const BAND_ROWS: u16 = 9;
/// The cells a hallway label may take before the rule: a room's from x 3,
/// a group's from x 1.
const ROOM_LABEL: usize = 13;
const GROUP_LABEL: usize = 15;

/// A hallway row: one of the rooms, or the log as a room of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hall {
    Room(RoomId),
    Log,
}

/// What holds the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Hall,
    Room,
    Log,
}

/// Where the log stands: docked beside the room, a band under it, or a
/// room of its own on the hallway's Log row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    Column,
    Band,
    Room,
}

/// One frame's geometry. `natural` is the placement the window size picks;
/// `placement` is what is drawn, which is the Room placement while `L` has
/// hidden a docked log. `log` is where the log draws this frame, if it
/// does; `rule` is the line between it and the room; `log_row` says
/// whether the hallway offers WATCH and its Log row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub placement: Placement,
    pub natural: Placement,
    pub room: Rect,
    pub log: Option<Rect>,
    pub spaced: bool,
    pub rule: Option<Rect>,
    pub log_row: bool,
    pub bar_top: u16,
}

/// The screen's geometry for a `width`×`height` window: the room right of
/// the hallway from row 1 to the row above the bar (`footer` is whether
/// the tips row is under the bar), and the log where the size puts it
/// (admin-screen contract, clauses 7 and 21). The log's and the band's
/// right edges stop at the third column from the window's edge, where a
/// stretched room's body ends.
pub(crate) fn layout(width: u16, height: u16, footer: bool, log_hidden: bool, showing: Hall) -> Layout {
    let bar_top = height.saturating_sub(bar::BAR_ROWS + u16::from(footer));
    let natural = if width >= COLUMN_FROM {
        Placement::Column
    } else if height >= BAND_FROM {
        Placement::Band
    } else {
        Placement::Room
    };
    let placement = if log_hidden { Placement::Room } else { natural };
    // From x 19, the room's body column, to the third column from the right.
    let span = width.saturating_sub(21);
    let base = Layout {
        placement,
        natural,
        room: Rect { x: ROOM_X, y: 1, width: width.saturating_sub(ROOM_X), height: bar_top.saturating_sub(1) },
        log: None,
        spaced: true,
        rule: None,
        log_row: false,
        bar_top,
    };
    match placement {
        Placement::Column => {
            let rule_x = ROOM_X + DOCKED_W + 1;
            let log_x = rule_x + 2;
            Layout {
                room: Rect { width: DOCKED_W, ..base.room },
                rule: Some(Rect { x: rule_x, y: 2, width: 1, height: bar_top.saturating_sub(2) }),
                log: Some(Rect { x: log_x, y: 2, width: width.saturating_sub(log_x + 2), height: bar_top.saturating_sub(2) }),
                ..base
            }
        }
        Placement::Band => Layout {
            room: Rect { height: bar_top.saturating_sub(1 + BAND_ROWS), ..base.room },
            rule: Some(Rect { x: 19, y: bar_top.saturating_sub(BAND_ROWS), width: span, height: 1 }),
            log: Some(Rect { x: 19, y: bar_top.saturating_sub(BAND_ROWS - 1), width: span, height: BAND_ROWS - 1 }),
            spaced: false,
            ..base
        },
        Placement::Room => Layout {
            log: (showing == Hall::Log).then_some(Rect { x: 19, y: 2, width: span, height: bar_top.saturating_sub(2) }),
            log_row: true,
            ..base
        },
    }
}

/// What a click on the screen's own surface means. The room's clicks are
/// the room's, on its own surface.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AdmAct {
    /// A hallway row: show it and hand it the keys.
    Open(Hall),
    /// A column taking the keys: the hallway's empty cells, the log.
    Focus(Focus),
    /// The log's own controls: the level, its menu, the paused word, the
    /// copy and the download, and a press-drag on its lines.
    Log(LogAct),
}

/// The screen's state. The room and the log's poll live while the screen
/// is up; what was shown, where the log stands and the hallway's cursor
/// are kept for as long as the player runs.
pub(crate) struct AdminUi {
    pub(super) room: Option<Box<dyn HostedRoom>>,
    /// Which room `room` is.
    pub(super) room_id: Option<RoomId>,
    pub(super) showing: Hall,
    /// The room the Log room hands back to, and the tab opens on.
    pub(super) last_room: RoomId,
    /// The hallway's keyboard cursor, an index into its rows.
    pub(super) cursor: usize,
    pub(super) focus: Focus,
    /// `L` hid a docked log: the room takes its rows or columns.
    pub(super) log_hidden: bool,
    pub(super) log: LogUi,
    /// How the session's server is reached, while there is a session.
    reach: Option<Reach>,
    /// Whether the rooms may pick the server's paths with the OS dialog.
    same_machine: bool,
    /// Why there is no room, in words, when there is none.
    why: Option<String>,
    /// Where the room takes the pointer this frame, less its first row,
    /// and where the log stood; both reset at the top of every frame.
    pub(super) room_at: Rect,
    pub(super) log_at: Option<Rect>,
    /// A press began in the room: its drag and release are the room's.
    pressed: bool,
}

impl AdminUi {
    pub(super) fn new() -> Self {
        AdminUi {
            room: None,
            room_id: None,
            showing: Hall::Room(RoomId::Libraries),
            last_room: RoomId::Libraries,
            cursor: 0,
            focus: Focus::Hall,
            log_hidden: false,
            log: LogUi::new(crate::admin::tz::local()),
            reach: None,
            same_machine: false,
            why: None,
            room_at: Rect::default(),
            log_at: None,
            pressed: false,
        }
    }
}

/// Whether a server reached this way runs on this machine, so a room's
/// folder picker may open the OS dialog and its paths mean the same on
/// both sides (contract clause 9): a loopback host (`localhost`,
/// `*.localhost`, 127.0.0.0/8, `::1`) reached directly. A tunnel's bridge
/// listens on loopback too, but the server is elsewhere, and so is a peer.
pub(super) fn same_machine(reach: &Reach) -> bool {
    if reach.local_token.is_some() || reach.peer.is_some() {
        return false;
    }
    let Ok(url) = url::Url::parse(&reach.base) else { return false };
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Open the screen on the session's server: the log starts polling and
/// the room last shown opens, with the keys in the hallway. With no
/// session, or a server that cannot be reached right now, the one
/// sentence saying why.
pub(super) fn open(gui: &mut Gui) {
    close(gui);
    gui.admin.focus = Focus::Hall;
    if !gui.app.connected {
        gui.admin.why = Some(t!("gui.admin.no_session").to_string());
        return;
    }
    let reach = match session_reach(&gui.app) {
        Ok(reach) => reach,
        Err(why) => {
            gui.admin.why = Some(why);
            return;
        }
    };
    let client = match reach_client(&reach) {
        Ok(client) => client,
        Err(why) => {
            gui.admin.why = Some(why);
            return;
        }
    };
    // A peer session reaches its parent, which is not this machine's
    // server however it is reached.
    gui.admin.same_machine = same_machine(&reach) && gui.app.session.peer.is_none();
    gui.admin.log.start(client);
    gui.admin.log.set_label(&log_server(&gui.app));
    gui.admin.reach = Some(reach);
    let showing = gui.admin.showing;
    show(gui, showing);
    gui.admin.cursor = row_of(showing);
}

/// The server a saved log file is named after: the one the screen's
/// rooms and log reach (a peer session's parent, else the session's own
/// origin, a tunnel by its identity), never the loopback bridge a tunnel
/// is reached through.
fn log_server(app: &App) -> String {
    match &app.session.peer {
        Some((parent, _)) => parent.clone(),
        None => app.origin().server,
    }
}

/// Leaving drops the room and stops the log's poll. What was shown, the
/// log's placement and the cursor are kept for the next visit.
pub(super) fn close(gui: &mut Gui) {
    let admin = &mut gui.admin;
    admin.room = None;
    admin.room_id = None;
    admin.reach = None;
    admin.why = None;
    admin.pressed = false;
    admin.log.stop();
    admin.room_at = Rect::default();
    admin.log_at = None;
}

/// The session changed under the screen: the room and the log are the new
/// server's.
pub(super) fn reopen(gui: &mut Gui) {
    if gui.screen == Screen::Admin {
        open(gui);
    }
}

/// Show a hallway row. A room other than the live one replaces it, so one
/// room is alive at a time; the Log room keeps the live room alive but
/// not drawn, its hover let go of.
fn show(gui: &mut Gui, hall: Hall) {
    let admin = &mut gui.admin;
    admin.pressed = false;
    match hall {
        Hall::Room(id) => {
            admin.last_room = id;
            if admin.room_id != Some(id) {
                admin.room = None;
                admin.room_id = None;
                if let Some(reach) = &admin.reach {
                    match reach_client(reach) {
                        Ok(client) => {
                            admin.room = Some(open_room(id, client, admin.same_machine));
                            admin.room_id = Some(id);
                        }
                        Err(why) => admin.why = Some(why),
                    }
                }
            }
        }
        Hall::Log => {
            if let Some(room) = admin.room.as_mut() {
                room.leave();
            }
        }
    }
    admin.showing = hall;
}

/// The hallway's rows in order, and the screen row each stands on: the
/// SERVER rooms under their group on row 2, the NETWORK rooms under
/// theirs on row 7, and the Log under WATCH on row 12 when it is offered.
fn hall_rows(log_row: bool) -> Vec<(Hall, u16)> {
    let mut rows: Vec<(Hall, u16)> = RoomId::ALL
        .iter()
        .enumerate()
        .map(|(i, id)| (Hall::Room(*id), if i < 3 { 3 + i as u16 } else { 5 + i as u16 }))
        .collect();
    if log_row {
        rows.push((Hall::Log, 13));
    }
    rows
}

/// A row's index in the hallway: the rooms in their order, then the Log.
fn row_of(hall: Hall) -> usize {
    match hall {
        Hall::Room(id) => RoomId::ALL.iter().position(|r| *r == id).unwrap_or(0),
        Hall::Log => RoomId::ALL.len(),
    }
}

fn hall_label(hall: Hall) -> String {
    match hall {
        Hall::Room(RoomId::Libraries) => t!("gui.admin.room_libraries"),
        Hall::Room(RoomId::Users) => t!("gui.admin.room_users"),
        Hall::Room(RoomId::Backups) => t!("gui.admin.room_backups"),
        Hall::Room(RoomId::Discovery) => t!("gui.admin.room_discovery"),
        Hall::Room(RoomId::Federation) => t!("gui.admin.room_federation"),
        Hall::Room(RoomId::Torrents) => t!("gui.admin.room_torrents"),
        Hall::Log => t!("gui.admin.room_log"),
    }
    .to_string()
}

/// Whether a room is drawn this frame (not the Log room, not before a
/// session).
fn room_up(admin: &AdminUi) -> bool {
    admin.room.is_some() && matches!(admin.showing, Hall::Room(_))
}

/// What the drawn room holds of the keyboard, whatever has the focus;
/// Open while no room is drawn.
fn room_claim(admin: &AdminUi) -> Claim {
    match admin.room.as_ref() {
        Some(room) if room_up(admin) => room.claims(),
        _ => Claim::Open,
    }
}

/// The layout for this window, with what is shown and what has the focus
/// made to agree with it: a window that grew past the Room placement, or
/// an `L` that docked the log again, hands the Log room back to the last
/// room (the log keeps the focus, now beside it, and the hallway's cursor
/// moves to the room's row); a room that holds every key — a modal that
/// opened while the hallway or the log had the keys, a text field —
/// takes the focus; a focus on something not on screen falls back to the
/// room, or the hallway.
fn settle(gui: &mut Gui, width: u16, height: u16) -> Layout {
    let footer = gui.footer();
    let mut lay = layout(width, height, footer, gui.admin.log_hidden, gui.admin.showing);
    if gui.admin.showing == Hall::Log && lay.placement != Placement::Room {
        let last = gui.admin.last_room;
        show(gui, Hall::Room(last));
        gui.admin.cursor = row_of(Hall::Room(last));
        lay = layout(width, height, footer, gui.admin.log_hidden, gui.admin.showing);
    }
    let admin = &mut gui.admin;
    let session = admin.reach.is_some();
    let room = room_up(admin);
    let log = session && lay.log.is_some();
    if room_claim(admin) == Claim::All {
        admin.focus = Focus::Room;
    }
    admin.focus = match admin.focus {
        Focus::Room if !room => Focus::Hall,
        Focus::Log if !log => {
            if room {
                Focus::Room
            } else {
                Focus::Hall
            }
        }
        focus => focus,
    };
    let rows = hall_rows(lay.log_row && session).len();
    admin.cursor = admin.cursor.min(rows - 1);
    // A log off the screen (hidden by `L`, or the Log room folded back)
    // lets go of a press held on it, so a drag that outlives it moves
    // nothing.
    if !log {
        admin.log.menu = None;
        admin.log.let_go();
    }
    lay
}

// ── Drawing ─────────────────────────────────────────────────────────────────

fn wrap_log(act: LogAct) -> Act {
    Act::Adm(AdmAct::Log(act))
}

/// The screen under the top bar: the room in its area, then the spill
/// guard over whatever it drew past that area, the hallway and its rule,
/// and the log with its rule (contract clauses 3, 6–8, 21). The pick
/// banner, the note, the bar and the footer are the GUI's, drawn after.
pub(super) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let lay = settle(gui, area.width, area.height);
    // A room's modal owns the pointer under the top bar (clause 18), and
    // swallows the clicks there: the GUI's surface lets go of it, so the
    // hallway, the log and the bar neither light nor show the hand under
    // a pointer they cannot have. The top bar's row stays the GUI's.
    let modal = room_up(&gui.admin) && gui.admin.room.as_ref().is_some_and(|room| room.modal_up());
    if modal && gui.ui.pointer.is_some_and(|at| at.y > 0) {
        gui.ui.pointer = None;
    }
    let key_hints = gui.config.gui.key_hints;
    let session = gui.admin.reach.is_some();
    let log_on = session && lay.log.is_some();
    // The log on screen has been seen, before the hallway counts it.
    if log_on {
        gui.admin.log.model.mark_seen();
    }

    // The room's field is the window's field (clause 28): the input
    // method's composition goes down to the room before it draws, and the
    // caret its focused field noted comes up to the GUI's surface after,
    // where the window reads that a field has the keyboard (its paste and
    // its input method on) and floats the candidates by the caret. A GUI
    // modal drawn over the room later lays itself over the note, as over
    // a GUI field's; the composition while one is up is its own field's,
    // so the room's beneath does not draw it too.
    let composition = if gui.modal_open() { "" } else { gui.ui.composition() };
    let admin = &mut gui.admin;
    let mut drew_room = false;
    if let (Hall::Room(_), Some(room)) = (admin.showing, admin.room.as_mut()) {
        room.set_key_hints(key_hints);
        room.set_composition(composition);
        room.draw_in(frame, lay.room);
        if let Some(at) = room.caret_at() {
            gui.ui.note_caret(at);
        }
        // The first row is the pick banner's, so its [X] stays the GUI's.
        admin.room_at = Rect { y: lay.room.y + 1, height: lay.room.height.saturating_sub(1), ..lay.room };
        drew_room = true;
    } else if !session {
        // The sentence where the room would be, on a second row where the
        // window is too narrow for it (most translations, at the floor);
        // what two rows cannot hold is cut at the second's end.
        let why = admin.why.clone().unwrap_or_default();
        let cells = area.width.saturating_sub(21) as usize;
        let mut lines = wrap_words(&why, cells).into_iter();
        if let Some(first) = lines.next() {
            put(frame, 19, 2, &bar::clip(&first, cells), dim());
        }
        let rest = lines.collect::<Vec<_>>().join(" ");
        if !rest.is_empty() {
            put(frame, 19, 3, &bar::clip(&rest, cells), dim());
        }
    }

    // The spill guard: whatever the room wrote below its area (a form
    // taller than the area, at the floor) and, beside the docked log,
    // right of it, goes before the hallway, the log and the bar draw.
    if drew_room {
        let below = lay.room.bottom();
        blank(frame, Rect { x: ROOM_X, y: below, width: area.width.saturating_sub(ROOM_X), height: area.height.saturating_sub(below) });
        if lay.placement == Placement::Column {
            let right = lay.room.right();
            blank(frame, Rect { x: right, y: 1, width: area.width.saturating_sub(right), height: area.height.saturating_sub(1) });
        }
    }

    draw_hallway(frame, gui, &lay, session);
    if log_on {
        draw_log(frame, gui, &lay, key_hints);
    }
}

/// The hallway: the groups dim at x 1, the rooms at x 3, the open row
/// `▸ label` in the accent, the cursor row on the slab while the hallway
/// has the keys, and the rule at x 16, lit while the room or the Log room
/// has them (contract clauses 3–4).
fn draw_hallway(frame: &mut Frame, gui: &mut Gui, lay: &Layout, session: bool) {
    let log_row = lay.log_row && session;
    // The column's own target first, so a click between the rows still
    // hands the hallway the keys.
    let column = Rect { x: 0, y: 2, width: RULE_X, height: lay.bar_top.saturating_sub(2) };
    gui.ui.click(column, Act::Adm(AdmAct::Focus(Focus::Hall)));

    let mut groups = vec![(2, t!("gui.admin.group_server")), (7, t!("gui.admin.group_network"))];
    if log_row {
        groups.push((12, t!("gui.admin.group_watch")));
    }
    for (y, label) in groups {
        if y < lay.bar_top {
            put(frame, 1, y, &bar::clip(&label, GROUP_LABEL), dim());
        }
    }

    let hall_keys = gui.admin.focus == Focus::Hall;
    let unseen = gui.admin.log.model.unseen();
    for (i, (hall, y)) in hall_rows(log_row).into_iter().enumerate() {
        if y >= lay.bar_top {
            break;
        }
        let rect = Rect { x: 1, y, width: RULE_X - 1, height: 1 };
        // With no session nothing is open, whatever was shown last.
        let open = session && hall == gui.admin.showing;
        let cursor = hall_keys && i == gui.admin.cursor;
        let style = if cursor {
            sel().add_modifier(Modifier::BOLD)
        } else if open {
            accent().add_modifier(Modifier::BOLD)
        } else if gui.ui.hovers(rect) {
            bright_bold()
        } else {
            dim()
        };
        if cursor {
            put(frame, rect.x, y, &" ".repeat(rect.width as usize), sel());
        }
        if open {
            put(frame, 1, y, &format!("{} ", forward_glyph()), style);
        }
        let label = bar::clip(&hall_label(hall), ROOM_LABEL).into_owned();
        put(frame, 3, y, &label, style);
        if hall == Hall::Log && unseen > 0 {
            let x = 3 + width(&label) as u16;
            if let Some(count) = log_count(x, &t!("gui.admin.log_new", n = unseen), unseen) {
                put(frame, x, y, &count, if cursor { sel() } else { dim() });
            }
        }
        gui.ui.click(rect, Act::Adm(AdmAct::Open(hall)));
    }

    let lit = match gui.admin.focus {
        Focus::Room => true,
        Focus::Log => gui.admin.showing == Hall::Log,
        Focus::Hall => false,
    };
    let style = if lit { accent() } else { dim() };
    for y in 2..lay.bar_top {
        put(frame, RULE_X, y, "│", style);
    }
}

/// The Log row's count, drawn from column `x` up to the rule: the whole
/// phrase (`words`, "· 12 new") where it fits, else the bare number where
/// a long translation of "Log" leaves no room for the words, else nothing,
/// so the label keeps its cells and the count is what goes.
fn log_count(x: u16, words: &str, unseen: usize) -> Option<String> {
    let room = RULE_X.saturating_sub(x) as usize;
    [format!(" {words}"), format!(" · {unseen}")].into_iter().find(|count| width(count) <= room)
}

/// The log where the layout puts it, behind its rule — `│` at x 118 when
/// docked, a `─` row over the band — lit while the log has the keys. The
/// header's hint says what `L` does here, while the key names are shown.
fn draw_log(frame: &mut Frame, gui: &mut Gui, lay: &Layout, key_hints: bool) {
    let Some(rect) = lay.log else { return };
    let style = if gui.admin.focus == Focus::Log { accent() } else { dim() };
    if let Some(rule) = lay.rule {
        match lay.placement {
            Placement::Column => {
                for y in rule.top()..rule.bottom() {
                    put(frame, rule.x, y, "│", style);
                }
            }
            Placement::Band => put(frame, rule.x, rule.y, &"─".repeat(rule.width as usize), style),
            Placement::Room => {}
        }
    }
    let hint = key_hints.then(|| {
        match lay.placement {
            Placement::Column => t!("gui.admin.log.hint_undock"),
            Placement::Band => t!("gui.admin.log.hint_hide"),
            Placement::Room => t!("gui.admin.log.hint_room"),
        }
        .to_string()
    });
    // Under the log's own controls: a click anywhere in it takes the keys.
    gui.ui.click(rect, Act::Adm(AdmAct::Focus(Focus::Log)));
    let look = Look { spaced: lay.spaced, hint };
    server_log::draw(frame, &mut gui.ui, &mut gui.admin.log, rect, &look, wrap_log);
    gui.admin.log_at = Some(rect);
    gui.admin.log.model.mark_seen();
}

/// The log's level menu, in the overlay pass, so it hangs over the lines
/// and owns the pointer while it is open. It owns every key too (clause
/// 20), so a room's field beneath stops counting as having the keyboard:
/// a chooser takes its keys as keys, never a paste or a composition.
pub(super) fn draw_overlays(frame: &mut Frame, gui: &mut Gui) {
    if gui.screen != Screen::Admin || gui.admin.log_at.is_none() {
        return;
    }
    if gui.admin.log.menu.is_some() {
        gui.ui.modal_over();
    }
    server_log::draw_menu(frame, &mut gui.ui, &mut gui.admin.log, wrap_log);
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// Where a key goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Route {
    /// The log's level menu, while it is open: every key.
    Menu,
    Room,
    Log,
    Hall,
    /// Tab or BackTab: the focus moves on, or back.
    Cycle { back: bool },
    /// `L`: the log moves.
    ToggleLog,
}

/// Which of the screen's three holders a key is for (contract clause 13),
/// by the focus and the drawn room's `claim`. A room that holds every key
/// (a modal or a text field up) gets every key, wherever the focus stood.
/// Otherwise the host keeps only Tab and BackTab, while the focused room's
/// claim is Open, and `L`; every other key a focused room gets, digits,
/// `q`, Esc and the GUI's capitals included.
pub(super) fn route(focus: Focus, claim: Claim, menu_open: bool, key: KeyEvent) -> Route {
    if menu_open {
        return Route::Menu;
    }
    if claim == Claim::All {
        return Route::Room;
    }
    let tab = matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let log = key.code == KeyCode::Char('L') && key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
    let cycle = Route::Cycle { back: key.code == KeyCode::BackTab };
    match focus {
        Focus::Room => match claim {
            Claim::Open if tab => cycle,
            _ if log => Route::ToggleLog,
            _ => Route::Room,
        },
        Focus::Log if tab => cycle,
        Focus::Log if log => Route::ToggleLog,
        Focus::Log => Route::Log,
        Focus::Hall if tab => cycle,
        Focus::Hall if log => Route::ToggleLog,
        Focus::Hall => Route::Hall,
    }
}

/// The focus after a Tab (or BackTab, `back`): around what is on screen,
/// hallway, room, log. The log is in the ring while it stands beside the
/// room; in the Log room the ring is the hallway and the log; with no
/// room the hallway keeps it (contract clause 12).
pub(super) fn next_focus(focus: Focus, log_beside: bool, log_room: bool, has_room: bool, back: bool) -> Focus {
    let mut ring = vec![Focus::Hall];
    if has_room && !log_room {
        ring.push(Focus::Room);
    }
    if log_beside || log_room {
        ring.push(Focus::Log);
    }
    let at = ring.iter().position(|f| *f == focus).unwrap_or(0);
    let n = ring.len();
    ring[if back { (at + n - 1) % n } else { (at + 1) % n }]
}

/// The screen's keys. Ctrl-C and the GUI's modals were asked first, as on
/// every screen. Returns true to quit.
pub(super) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    let lay = settle(gui, gui.last_width, gui.last_height);
    let claim = room_claim(&gui.admin);
    let menu_open = gui.admin.log.menu.is_some();
    match route(gui.admin.focus, claim, menu_open, key) {
        Route::Menu => gui.admin.log.menu_key(key),
        Route::Room => room_key(gui, key),
        Route::Log => {
            let rows = lay.log.map_or(0, |r| r.height.saturating_sub(if lay.spaced { 2 } else { 1 })) as usize;
            if gui.admin.log.key(key, rows) == LogKey::Leave {
                gui.admin.focus = Focus::Hall;
                gui.admin.cursor = row_of(gui.admin.showing);
            }
            take_log_note(gui);
        }
        Route::Cycle { back } => {
            let admin = &mut gui.admin;
            let log_beside = admin.reach.is_some() && lay.log.is_some() && lay.placement != Placement::Room;
            let log_room = admin.showing == Hall::Log;
            admin.focus = next_focus(admin.focus, log_beside, log_room, admin.room.is_some(), back);
        }
        Route::ToggleLog => toggle_log(gui, &lay),
        Route::Hall => return hall_key(gui, key, &lay),
    }
    false
}

/// A key for the focused room. Esc at its base and `q` come back as Quit,
/// which here hands the keys to the hallway and closes nothing (contract
/// clause 14).
fn room_key(gui: &mut Gui, key: KeyEvent) {
    let admin = &mut gui.admin;
    let Some(room) = admin.room.as_mut() else {
        admin.focus = Focus::Hall;
        return;
    };
    if matches!(room.press(key), Some(Outcome::Quit)) {
        admin.focus = Focus::Hall;
        admin.cursor = row_of(admin.showing);
    }
}

/// `L` (contract clause 17): where the window has room for the log beside
/// the room, it hides and shows it, and a log that had the keys hands
/// them to the room; where it has room only for the room, it opens the
/// Log room with the keys on the log, and from there goes back to the
/// last room with the keys on the room.
fn toggle_log(gui: &mut Gui, lay: &Layout) {
    gui.admin.log.menu = None;
    if lay.natural != Placement::Room {
        let admin = &mut gui.admin;
        admin.log_hidden = !admin.log_hidden;
        if admin.log_hidden && admin.focus == Focus::Log {
            admin.focus = if room_up(admin) { Focus::Room } else { Focus::Hall };
        }
        return;
    }
    if gui.admin.showing == Hall::Log {
        let last = gui.admin.last_room;
        show(gui, Hall::Room(last));
        gui.admin.focus = if room_up(&gui.admin) { Focus::Room } else { Focus::Hall };
    } else if gui.admin.reach.is_some() {
        show(gui, Hall::Log);
        gui.admin.focus = Focus::Log;
    }
    gui.admin.cursor = row_of(gui.admin.showing);
}

/// The hallway's keys (contract clause 15): the cursor, the way into a
/// row, the ways out to the GUI's other screens, and the transport. Returns
/// true to quit.
fn hall_key(gui: &mut Gui, key: KeyEvent, lay: &Layout) -> bool {
    let rows = hall_rows(lay.log_row && gui.admin.reach.is_some());
    match key.code {
        KeyCode::Up => gui.admin.cursor = gui.admin.cursor.saturating_sub(1),
        KeyCode::Down => gui.admin.cursor = (gui.admin.cursor + 1).min(rows.len() - 1),
        KeyCode::Enter | KeyCode::Right => {
            if let Some((hall, _)) = rows.get(gui.admin.cursor) {
                enter(gui, *hall);
            }
        }
        KeyCode::Esc | KeyCode::Char('M') => return gui.act(Act::Screen(Screen::Library)),
        KeyCode::Char('T') => return gui.act(Act::Screen(Screen::Stats)),
        KeyCode::Char('0') => return gui.act(Act::Screen(Screen::NowPlaying)),
        KeyCode::Char('V') => return gui.act(Act::VizWindow),
        KeyCode::Char(c @ '1'..='9') => return gui.act(Act::Nav(c as usize - '1' as usize)),
        KeyCode::Char('D') => return gui.act(Act::Nav(DJ_NAV)),
        KeyCode::Char('q') => return true,
        code => return super::transport_key(gui, code).unwrap_or(false),
    }
    false
}

/// Open a hallway row and hand it the keys: a room once it is up, the log
/// at once.
fn enter(gui: &mut Gui, hall: Hall) {
    match hall {
        Hall::Room(_) => {
            show(gui, hall);
            if room_up(&gui.admin) {
                gui.admin.focus = Focus::Room;
            }
        }
        Hall::Log => {
            if gui.admin.reach.is_none() {
                return;
            }
            show(gui, hall);
            gui.admin.focus = Focus::Log;
        }
    }
    gui.admin.cursor = row_of(hall);
}

/// The screen's own clicks. True when `act` was one of them.
pub(super) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Adm(act) = act else { return false };
    match act {
        AdmAct::Open(hall) => enter(gui, *hall),
        AdmAct::Focus(focus) => {
            let admin = &mut gui.admin;
            let there = match focus {
                Focus::Hall => true,
                Focus::Room => room_up(admin),
                Focus::Log => admin.log_at.is_some(),
            };
            if there {
                admin.focus = *focus;
            }
        }
        AdmAct::Log(log_act) => {
            // A click on the log's own controls is a click in the log, and
            // so is a press on its lines; the moves and the release of that
            // press, wherever the hand has gone, hand nothing on.
            let elsewhere = matches!(log_act, LogAct::MenuClose | LogAct::Select(Grip::Drag | Grip::Release, _));
            if !elsewhere {
                gui.admin.focus = Focus::Log;
            }
            gui.admin.log.act(*log_act);
            take_log_note(gui);
        }
    }
    true
}

/// The log's last note (a copy, a download, a file shown), lifted into
/// the GUI's note on the bar's bottom row, a path in it fitted to that
/// note's width in this window.
fn take_log_note(gui: &mut Gui) {
    let cells = super::bar::note_width(gui.last_width) as usize;
    if let Some(note) = gui.admin.log.take_note(cells) {
        gui.note = Some(note);
    }
}

/// The window's copy chord (Cmd+C on a Mac, Ctrl+Shift+C or Ctrl+Insert
/// elsewhere) is the log's `y` while the log on screen has the keys or a
/// highlight stands, and nothing laid over the screen holds them: no GUI
/// modal, no level menu, no room's modal. To the header's server menu it
/// is a key like any other, which closes the menu and goes on (servers.rs),
/// on every screen. True when it changed anything (a copy, the menu
/// closed), so the window draws again.
#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub(super) fn copy_chord(gui: &mut Gui) -> bool {
    let closed = std::mem::take(&mut gui.servers.drop_open);
    let admin = &gui.admin;
    let held = gui.screen != Screen::Admin
        || admin.log_at.is_none()
        || gui.modal_open()
        || admin.log.menu.is_some()
        || (room_up(admin) && admin.room.as_ref().is_some_and(|room| room.modal_up()));
    if held || (admin.focus != Focus::Log && admin.log.model.highlight.is_none()) {
        return closed;
    }
    gui.admin.log.copy();
    take_log_note(gui);
    true
}

// ── Pointer, wheel, frame, footer ───────────────────────────────────────────

/// A grip on the log's lines stands only while the log does: off the
/// screen, or with the log left undrawn by the last frame (`L`, the Log
/// room folded back, the mini player), it lets go untold, and the log's
/// hold with it. Else a release the terminal never sent would hold the
/// pointer for good: hover frozen, every page owner shut out.
pub(super) fn let_go_unseen(gui: &mut Gui) {
    if gui.ui.gripping() && (gui.screen != Screen::Admin || gui.admin.log_at.is_none()) {
        gui.ui.release();
        gui.admin.log.let_go();
    }
}

/// The pointer below the top bar (contract clauses 18 and 20). The room
/// takes what lands inside its area less its first row, every event while
/// its modal is up, and the drag and release of a press that began in it,
/// even on the top bar's row, where a thumb dragged to the top overshoots:
/// a release the room never saw would leave its held arrow stepping and
/// its thumb following later drags. A press in the room hands it the keys.
/// Anything else lets go of the room's hover and answers on the GUI's
/// surface — the hallway, the log, the bar. A GUI modal, the server menu
/// or the log's level menu owns the pointer while open, and a frame that
/// drew no room (the Log room, the mini player) takes nothing. True when
/// the room took the event.
pub(super) fn pointer(gui: &mut Gui, mouse: MouseEvent) -> bool {
    if gui.screen != Screen::Admin {
        return false;
    }
    let held = gui.admin.pressed && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_));
    // A release ends the press wherever it lands, whoever takes it.
    if matches!(mouse.kind, MouseEventKind::Up(_)) {
        gui.admin.pressed = false;
    }
    if (mouse.row == 0 && !held)
        || gui.modal_open()
        || gui.servers.drop_open
        || gui.admin.log.menu.is_some()
    {
        if let Some(room) = gui.admin.room.as_mut() {
            room.leave();
        }
        return false;
    }
    let admin = &mut gui.admin;
    let drawn = !admin.room_at.is_empty() && matches!(admin.showing, Hall::Room(_));
    let Some(room) = admin.room.as_mut().filter(|_| drawn) else { return false };
    let at = Position { x: mouse.column, y: mouse.row };
    if !(admin.room_at.contains(at) || room.modal_up() || held) {
        room.leave();
        return false;
    }
    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
        admin.focus = Focus::Room;
        admin.pressed = true;
    }
    if matches!(room.mouse(mouse), Some(Outcome::Quit)) {
        admin.focus = Focus::Hall;
        admin.cursor = row_of(admin.showing);
    }
    true
}

/// The wheel over the GUI's surface: the log scrolls under it, up for
/// older lines. A wheel over the room reached the room through
/// [`pointer`].
pub(super) fn wheel(gui: &mut Gui, at: Position, delta: i32) {
    if gui.admin.log.menu.is_some() {
        return;
    }
    if gui.admin.log_at.is_some_and(|rect| rect.contains(at)) {
        gui.admin.log.wheel(delta < 0);
    }
}

/// The screen's duties after the draw: the log's answers, its next poll
/// and the note a download or a shown file left, the room's worker,
/// timers, held control and tooltip dwell (the room is pumped while the
/// Log room hides it). Returns whether the pointer rests on one of the
/// shown room's clickables, for the hand.
pub(super) fn frame(gui: &mut Gui) -> bool {
    if gui.screen != Screen::Admin {
        return false;
    }
    gui.admin.log.pump(Instant::now());
    take_log_note(gui);
    let admin = &mut gui.admin;
    let drawn = !admin.room_at.is_empty() && matches!(admin.showing, Hall::Room(_));
    admin.room.as_mut().is_some_and(|room| room.after_frame()) && drawn
}

/// The footer's line by focus (contract clause 27): with no session the
/// way back; in the hallway its keys; in the log its keys, or the menu's;
/// in the room the room's own hint, then the host's keys it leaves free
/// (Tab and `L` while its claim is Open, `L` alone while it keeps Tab,
/// nothing while it holds every key), the tail only when the whole line
/// fits the footer.
pub(super) fn tips(gui: &Gui) -> String {
    let admin = &gui.admin;
    if admin.reach.is_none() {
        return t!("gui.tips.stats_back").to_string();
    }
    if admin.log.menu.is_some() {
        return t!("gui.admin.tips_log_menu").to_string();
    }
    match admin.focus {
        Focus::Hall => t!("gui.admin.tips_hall").to_string(),
        Focus::Log => t!("gui.admin.tips_log").to_string(),
        Focus::Room => {
            let Some(room) = admin.room.as_ref().filter(|_| room_up(admin)) else {
                return t!("gui.admin.tips_hall").to_string();
            };
            let hint = room.tips();
            let tail = match room.claims() {
                Claim::Open => format!("{} · {}", t!("gui.admin.tips_focus"), t!("gui.admin.tips_log_key")),
                Claim::OwnTab => t!("gui.admin.tips_log_key").to_string(),
                Claim::All => return hint,
            };
            let line = if hint.is_empty() { tail } else { format!("{hint} · {tail}") };
            // The footer is drawn from x 1 and stops a cell short of the edge.
            if width(&line) <= gui.last_width.saturating_sub(2) as usize { line } else { hint }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Style;

    use super::*;
    use crate::api::types::{ActivityEntry, LogTail};
    use crate::config::Config;
    use crate::kit::theme::th;
    use crate::tui::app::App;

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// The GUI with no server at all, the demo seat in the bar.
    fn gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.demo = Some(super::super::demo_now());
        gui
    }

    /// A session on a server nobody answers for: the rooms open and queue
    /// their loads, and nothing a test does pumps them.
    fn session_gui() -> Gui {
        let mut gui = gui();
        gui.app.connected = true;
        gui.app.session.server = "http://host.invalid:3000".into();
        gui.app.session.server_id = "http://host.invalid:3000".into();
        gui
    }

    /// The Admin tab up on a session, its log on UTC with no poll behind it.
    fn admin_gui() -> Gui {
        let mut gui = session_gui();
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.screen, Screen::Admin);
        gui.admin.log = LogUi::new(None);
        gui
    }

    /// What a probe room was asked, shared with the test that boxed it,
    /// and what it answers.
    #[derive(Default)]
    struct Record {
        keys: Vec<KeyCode>,
        mice: Vec<(MouseEventKind, u16, u16)>,
        left: usize,
        frames: usize,
        drawn: Vec<Rect>,
        hints: Option<bool>,
        claim: Claim,
        modal: bool,
        tips: String,
        /// Write past the area's bottom and right edges, as a form taller
        /// than the area would.
        spill: bool,
        /// Where its focused field's caret is, as a room's surface notes it.
        caret: Option<Position>,
        /// Each composition the host handed down, in order.
        compositions: Vec<String>,
    }

    /// A room that only writes down what the host asked of it.
    #[derive(Clone, Default)]
    struct Probe(Rc<RefCell<Record>>);

    impl HostedRoom for Probe {
        fn draw_in(&mut self, frame: &mut Frame, area: Rect) {
            let mut record = self.0.borrow_mut();
            record.drawn.push(area);
            put(frame, area.x + 2, area.y + 1, "probe body", Style::default());
            if record.spill {
                put(frame, area.x + 2, area.bottom(), "SPILL BELOW", Style::default());
                put(frame, area.right(), area.y + 3, "SPILL RIGHT", Style::default());
            }
        }

        fn press(&mut self, key: KeyEvent) -> Option<Outcome> {
            self.0.borrow_mut().keys.push(key.code);
            matches!(key.code, KeyCode::Esc | KeyCode::Char('q')).then_some(Outcome::Quit)
        }

        fn mouse(&mut self, mouse: MouseEvent) -> Option<Outcome> {
            self.0.borrow_mut().mice.push((mouse.kind, mouse.column, mouse.row));
            None
        }

        fn leave(&mut self) {
            self.0.borrow_mut().left += 1;
        }

        fn after_frame(&mut self) -> bool {
            self.0.borrow_mut().frames += 1;
            true
        }

        fn tips(&self) -> String {
            self.0.borrow().tips.clone()
        }

        fn claims(&self) -> Claim {
            let record = self.0.borrow();
            if record.modal { Claim::All } else { record.claim }
        }

        fn modal_up(&self) -> bool {
            self.0.borrow().modal
        }

        fn set_key_hints(&mut self, on: bool) {
            self.0.borrow_mut().hints = Some(on);
        }

        fn caret_at(&mut self) -> Option<Position> {
            self.0.borrow().caret
        }

        fn set_composition(&mut self, text: &str) {
            self.0.borrow_mut().compositions.push(text.to_string());
        }
    }

    /// The Admin tab with `probe` standing in for the Libraries room.
    fn hosting(probe: &Probe) -> Gui {
        let mut gui = admin_gui();
        gui.admin.room = Some(Box::new(probe.clone()));
        gui.admin.room_id = Some(RoomId::Libraries);
        gui
    }

    fn render_at(gui: &mut Gui, w: u16, h: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| super::super::render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// Row `y` from column `x` on, as text.
    fn from(buf: &Buffer, x: u16, y: u16) -> String {
        (x..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The column where `needle` starts on `line`, counted in cells (the
    /// rows these tests read are one cell a character).
    fn col(line: &str, needle: &str) -> Option<u16> {
        line.char_indices().position(|(i, _)| line[i..].starts_with(needle)).map(|c| c as u16)
    }

    fn press_with(gui: &mut Gui, code: KeyCode, modifiers: KeyModifiers) -> bool {
        super::super::handle_key(gui, KeyEvent::new(code, modifiers))
    }

    fn press(gui: &mut Gui, code: KeyCode) -> bool {
        press_with(gui, code, KeyModifiers::NONE)
    }

    /// A mouse event through the GUI's own loop, so the routing a test
    /// drives can never drift from the one the player runs: the grip, the
    /// Stats page and the Admin room first, then the GUI's own surface.
    fn mouse(gui: &mut Gui, kind: MouseEventKind, x: u16, y: u16) {
        let event = MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
        let mut ctx = super::super::tests::quiet_ctx(gui);
        super::super::input(gui, &mut ctx, ratatui::crossterm::event::Event::Mouse(event));
    }

    fn down(gui: &mut Gui, x: u16, y: u16) {
        mouse(gui, MouseEventKind::Down(MouseButton::Left), x, y);
    }

    fn entry(seq: u64, level: &str, message: &str) -> ActivityEntry {
        ActivityEntry {
            seq,
            t: format!("2026-10-02T09:00:{:02}.000Z", seq % 60),
            level: level.to_string(),
            message: message.to_string(),
        }
    }

    /// `n` info lines numbered from `from`.
    fn lines(from: u64, n: u64) -> LogTail {
        LogTail {
            entries: (from..from + n).map(|s| entry(s, "info", &format!("line {s}"))).collect(),
            last_seq: from + n - 1,
            capacity: 1000,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // ── The pure parts ──────────────────────────────────────────────────────

    #[test]
    fn layout_math_by_size() {
        let libraries = Hall::Room(RoomId::Libraries);
        let at = |w, h, footer, hidden, showing| layout(w, h, footer, hidden, showing);

        // The floor: seventeen rows for the room, the Log row offered.
        let floor = at(100, 24, true, false, libraries);
        assert_eq!(floor.placement, Placement::Room);
        assert_eq!(floor.bar_top, 18);
        assert_eq!(floor.room, Rect::new(17, 1, 83, 17));
        assert_eq!((floor.log, floor.rule, floor.log_row), (None, None, true));

        assert_eq!(at(100, 30, false, false, libraries).room, Rect::new(17, 1, 83, 24));

        let middle = at(136, 40, false, false, libraries);
        assert_eq!((middle.placement, middle.room, middle.log_row), (Placement::Room, Rect::new(17, 1, 119, 34), true));
        let log_room = at(136, 40, false, false, Hall::Log);
        assert_eq!(log_room.log, Some(Rect::new(19, 2, 115, 33)), "the Log room, under its own blank row");
        assert!(log_room.spaced);

        let band = at(136, 52, false, false, libraries);
        assert_eq!(band.placement, Placement::Band);
        assert_eq!(band.room, Rect::new(17, 1, 119, 37), "the room's note on row 37");
        assert_eq!(band.rule, Some(Rect::new(19, 38, 115, 1)));
        assert_eq!(band.rule.unwrap().right() - 1, 133, "the rule ends at the third column from the edge");
        assert_eq!(band.log, Some(Rect::new(19, 39, 115, 8)), "a header and seven lines");
        assert!(!band.spaced && !band.log_row);

        let main = at(176, 46, true, false, libraries);
        assert_eq!(main.placement, Placement::Column);
        assert_eq!(main.room, Rect::new(17, 1, 100, 39), "the room's note on row 39");
        assert_eq!(main.rule, Some(Rect::new(118, 2, 1, 38)));
        assert_eq!(main.log, Some(Rect::new(120, 2, 54, 38)));
        assert!(main.spaced && !main.log_row);

        let hidden = at(176, 46, true, true, libraries);
        assert_eq!((hidden.placement, hidden.natural), (Placement::Room, Placement::Column));
        assert_eq!(hidden.room, Rect::new(17, 1, 159, 39));
        assert!(hidden.log_row, "a hidden log is back on the hallway");

        assert_eq!(at(160, 30, false, false, libraries).placement, Placement::Column);
        assert_eq!(at(159, 48, false, false, libraries).placement, Placement::Band);
        assert_eq!(at(159, 47, false, false, libraries).placement, Placement::Room);
    }

    #[test]
    fn route_never_steals_digits_q_esc_or_capitals_from_a_room() {
        for code in [
            KeyCode::Char('1'),
            KeyCode::Char('9'),
            KeyCode::Char('0'),
            KeyCode::Char('q'),
            KeyCode::Esc,
            KeyCode::Char('M'),
            KeyCode::Char('T'),
            KeyCode::Char('V'),
            KeyCode::Char('D'),
            KeyCode::Char('t'),
            KeyCode::Char('l'),
            KeyCode::Char(' '),
        ] {
            assert_eq!(route(Focus::Room, Claim::Open, false, key(code)), Route::Room, "{code:?}");
        }
        assert_eq!(route(Focus::Room, Claim::Open, false, key(KeyCode::Tab)), Route::Cycle { back: false });
        let back = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(route(Focus::Room, Claim::Open, false, back), Route::Cycle { back: true });
        assert_eq!(route(Focus::Room, Claim::Open, false, key(KeyCode::Char('L'))), Route::ToggleLog);
        let shifted = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT);
        assert_eq!(route(Focus::Room, Claim::Open, false, shifted), Route::ToggleLog);
        let ctrl_l = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(route(Focus::Room, Claim::Open, false, ctrl_l), Route::Room, "a chord is the room's");
        let ctrl_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL);
        assert_eq!(route(Focus::Room, Claim::Open, false, ctrl_tab), Route::Room);

        // The hallway and the log keep their own keys and share the two.
        assert_eq!(route(Focus::Hall, Claim::Open, false, key(KeyCode::Char('1'))), Route::Hall);
        assert_eq!(route(Focus::Hall, Claim::Open, false, key(KeyCode::Tab)), Route::Cycle { back: false });
        assert_eq!(route(Focus::Hall, Claim::Open, false, key(KeyCode::Char('L'))), Route::ToggleLog);
        assert_eq!(route(Focus::Log, Claim::Open, false, key(KeyCode::Char('q'))), Route::Log);
        assert_eq!(route(Focus::Log, Claim::Open, false, key(KeyCode::Tab)), Route::Cycle { back: false });
        assert_eq!(route(Focus::Log, Claim::Open, false, key(KeyCode::Char('L'))), Route::ToggleLog);
        // The open level menu takes every key, whoever had the focus.
        for focus in [Focus::Hall, Focus::Room, Focus::Log] {
            assert_eq!(route(focus, Claim::Open, true, key(KeyCode::Tab)), Route::Menu);
            assert_eq!(route(focus, Claim::Open, true, key(KeyCode::Char('L'))), Route::Menu);
        }
    }

    #[test]
    fn route_hands_every_key_to_a_room_that_claims_them_all() {
        let back = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let keys = [key(KeyCode::Tab), back, key(KeyCode::Char('L')), key(KeyCode::Char('q')), key(KeyCode::Esc), key(KeyCode::Down)];
        // Wherever the focus stood: a modal that opened while the hallway or
        // the log had the keys is the room's to answer.
        for focus in [Focus::Room, Focus::Hall, Focus::Log] {
            for event in keys {
                assert_eq!(route(focus, Claim::All, false, event), Route::Room, "{focus:?} {event:?}");
            }
        }
        // The log's open level menu still takes every key until it closes.
        assert_eq!(route(Focus::Hall, Claim::All, true, key(KeyCode::Esc)), Route::Menu);
    }

    #[test]
    fn route_keeps_tab_for_a_room_that_owns_it() {
        let back = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(route(Focus::Room, Claim::OwnTab, false, key(KeyCode::Tab)), Route::Room);
        assert_eq!(route(Focus::Room, Claim::OwnTab, false, back), Route::Room);
        assert_eq!(route(Focus::Room, Claim::OwnTab, false, key(KeyCode::Char('L'))), Route::ToggleLog);
        assert_eq!(route(Focus::Room, Claim::OwnTab, false, key(KeyCode::Char('1'))), Route::Room);
    }

    #[test]
    fn focus_cycles_through_what_is_on_screen() {
        use Focus::{Hall as H, Log as L, Room as R};
        // Beside the room: hallway, room, log, and round; back the other way.
        assert_eq!(next_focus(H, true, false, true, false), R);
        assert_eq!(next_focus(R, true, false, true, false), L);
        assert_eq!(next_focus(L, true, false, true, false), H);
        assert_eq!(next_focus(H, true, false, true, true), L);
        assert_eq!(next_focus(L, true, false, true, true), R);
        assert_eq!(next_focus(R, true, false, true, true), H);
        // The log not on screen: hallway and room.
        assert_eq!(next_focus(H, false, false, true, false), R);
        assert_eq!(next_focus(R, false, false, true, false), H);
        // The Log room: hallway and log, the hidden room out of the ring.
        assert_eq!(next_focus(H, false, true, true, false), L);
        assert_eq!(next_focus(L, false, true, true, false), H);
        assert_eq!(next_focus(H, false, true, true, true), L);
        // No session, no room: the hallway keeps it.
        assert_eq!(next_focus(H, false, false, false, false), H);
        assert_eq!(next_focus(H, false, false, false, true), H);
    }

    #[test]
    fn same_machine_is_a_loopback_server_reached_directly_never_a_tunnel_or_a_peer() {
        let reach = |base: &str| Reach {
            base: base.to_string(),
            token: Some("token".into()),
            self_signed: false,
            peer: None,
            local_token: None,
        };
        for base in [
            "http://localhost:3000",
            "http://LOCALHOST:3000",
            "http://music.localhost:3000",
            "http://127.0.0.1:3000",
            "https://127.8.9.10",
            "http://[::1]:3000",
        ] {
            assert!(same_machine(&reach(base)), "{base}");
        }
        for base in [
            "http://192.168.1.20:3000",
            "https://music.example.com",
            "http://localhost.example.com:3000",
            "http://[2001:db8::1]:3000",
            "not a url",
        ] {
            assert!(!same_machine(&reach(base)), "{base}");
        }
        let tunnel = Reach { local_token: Some("lt".into()), ..reach("http://127.0.0.1:51234") };
        assert!(!same_machine(&tunnel), "a tunnel's bridge listens on loopback, its server does not");
        let peer = Reach { peer: Some(7), ..reach("http://127.0.0.1:3000") };
        assert!(!same_machine(&peer), "a peer is another server");

        // Through the tab: the session's reach decides.
        let mut gui = session_gui();
        gui.app.session.server = "http://127.0.0.1:3000".into();
        gui.app.session.server_id = "http://127.0.0.1:3000".into();
        gui.act(Act::Screen(Screen::Admin));
        assert!(gui.admin.same_machine);
        let mut far = session_gui();
        far.act(Act::Screen(Screen::Admin));
        assert!(!far.admin.same_machine);
    }

    // ── The tab ─────────────────────────────────────────────────────────────

    #[test]
    fn the_admin_tab_is_third_and_m_opens_it_from_every_screen() {
        let _en = english();
        let mut gui = gui();
        let buf = render_at(&mut gui, 100, 30);
        let top = row(&buf, 0);
        let (lx, sx, ax) = (col(&top, " Library ").unwrap(), col(&top, " Stats ").unwrap(), col(&top, " Admin ").unwrap());
        assert!(lx < sx && sx < ax, "Library, Stats, Admin: {top}");
        assert_eq!(gui.ui.hit(Position::new(ax + 1, 0)), Some(Act::Screen(Screen::Admin)));
        assert_ne!(buf[(ax, 0)].bg, th().accent);

        assert!(!press(&mut gui, KeyCode::Char('M')));
        assert_eq!(gui.screen, Screen::Admin, "M from the Library");
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(buf[(ax, 0)].bg, th().accent, "the Admin tab wears the slab");
        assert_ne!(buf[(lx, 0)].bg, th().accent);
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.screen, Screen::Library, "M in the hallway leads back");

        gui.act(Act::Screen(Screen::Stats));
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.screen, Screen::Admin, "M from Stats");
        gui.act(Act::Screen(Screen::NowPlaying));
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.screen, Screen::Admin, "M from Now Playing");
        assert!(!gui.app.fullscreen);
        press(&mut gui, KeyCode::Esc);
        assert_eq!(gui.screen, Screen::Library, "Esc in the hallway leads back");
    }

    #[test]
    fn at_100x30_the_hallway_stands_beside_the_room() {
        let _en = english();
        let mut gui = admin_gui();
        let buf = render_at(&mut gui, 100, 30);
        assert!(from(&buf, 1, 2).starts_with("SERVER"), "{}", row(&buf, 2));
        assert!(from(&buf, 1, 3).starts_with("▸ Libraries"), "{}", row(&buf, 3));
        assert!(from(&buf, 3, 4).starts_with("Users"));
        assert!(from(&buf, 3, 5).starts_with("Backups"));
        assert!(from(&buf, 1, 7).starts_with("NETWORK"));
        assert!(from(&buf, 3, 8).starts_with("Discovery"));
        assert!(from(&buf, 3, 9).starts_with("Federation"));
        assert!(from(&buf, 3, 10).starts_with("Torrents"));
        assert!(from(&buf, 1, 12).starts_with("WATCH"));
        assert!(from(&buf, 3, 13).starts_with("Log"));
        for y in 2..25 {
            assert_eq!(buf[(16, y)].symbol(), "│", "the rule on row {y}");
        }
        assert_ne!(buf[(16, 25)].symbol(), "│", "the rule stops above the bar");
        assert_eq!(from(&buf, 17, 1).trim(), "", "the room's first row is blank");
        assert!(from(&buf, 19, 24).starts_with("loading libraries…"), "the room's busy line: {}", row(&buf, 24));
        assert_eq!(buf[(19, 24)].fg, th().accent);
        assert_eq!(gui.admin.room_at, Rect::new(17, 2, 83, 23));
        assert!(!row(&buf, 2).contains("no session"));
    }

    #[test]
    fn at_136x40_the_room_stretches_and_the_log_is_a_room_with_its_count() {
        let _en = english();
        let mut gui = admin_gui();
        press(&mut gui, KeyCode::Down);
        press(&mut gui, KeyCode::Enter);
        assert_eq!((gui.admin.room_id, gui.admin.focus), (Some(RoomId::Users), Focus::Room));
        gui.admin.log.model.take(lines(1, 3));
        gui.admin.log.model.take(lines(4, 2));
        let buf = render_at(&mut gui, 136, 40);
        assert!(from(&buf, 1, 4).starts_with("▸ Users"));
        assert!(from(&buf, 3, 13).starts_with("Log · 2 new"), "{}", row(&buf, 13));
        assert_eq!(buf[(6, 13)].fg, th().dim, "the count is dim");
        assert_eq!(gui.admin.room_at, Rect::new(17, 2, 119, 33), "to the window's right edge");
        assert!(from(&buf, 19, 34).starts_with("loading users…"), "{}", row(&buf, 34));

        // L: the Log room, the keys on the log.
        press(&mut gui, KeyCode::Char('L'));
        assert_eq!((gui.admin.showing, gui.admin.focus), (Hall::Log, Focus::Log));
        let buf = render_at(&mut gui, 136, 40);
        assert!(from(&buf, 19, 2).starts_with("• following · info ▾"), "{}", row(&buf, 2));
        assert_eq!(from(&buf, 19, 3).trim(), "", "a blank row under the header");
        assert!(from(&buf, 19, 4).starts_with("09:00:01  line 1"), "{}", row(&buf, 4));
        assert!(from(&buf, 19, 8).starts_with("09:00:05  line 5"));
        assert!(from(&buf, 1, 13).starts_with("▸ Log"), "{}", row(&buf, 13));
        assert!(!row(&buf, 13).contains("new"), "the log on screen has been seen");
        assert_eq!(gui.admin.room_at, Rect::default(), "the hidden room takes no pointer");
        assert!(gui.admin.room.is_some(), "but stays alive");
    }

    #[test]
    fn at_136x52_the_log_is_a_nine_row_band_under_the_room() {
        let _en = english();
        let mut gui = admin_gui();
        gui.act(Act::Adm(AdmAct::Open(Hall::Room(RoomId::Backups))));
        gui.admin.log.model.take(lines(1, 10));
        let buf = render_at(&mut gui, 136, 52);
        assert!(from(&buf, 19, 37).starts_with("loading backups…"), "the room's note: {}", row(&buf, 37));
        assert_eq!(buf[(19, 38)].symbol(), "─");
        assert_eq!(buf[(133, 38)].symbol(), "─");
        assert_eq!(buf[(134, 38)].symbol(), " ", "the rule stops at the third column from the edge");
        assert!(from(&buf, 19, 39).starts_with("• following · info ▾"), "{}", row(&buf, 39));
        for (i, y) in (40..=46).enumerate() {
            let n = i + 4;
            assert!(from(&buf, 19, y).starts_with(&format!("09:00:{n:02}  line {n}")), "row {y}: {}", row(&buf, y));
        }
        assert!(!(0..52).any(|y| row(&buf, y).contains("WATCH")), "no Log row while the band shows it");
        assert_eq!(from(&buf, 130, 2), "beta  ", "the beta chip at the right of the body's first row");
        assert_eq!(buf[(130, 2)].fg, th().gold);
    }

    #[test]
    fn from_160_columns_the_log_docks_beside_a_100_cell_room() {
        let _en = english();
        let mut gui = admin_gui();
        gui.config.gui.key_hints = true;
        gui.act(Act::Adm(AdmAct::Open(Hall::Room(RoomId::Discovery))));
        gui.admin.log.model.take(lines(1, 50));
        let buf = render_at(&mut gui, 176, 46);
        for y in 2..=39 {
            assert_eq!(buf[(118, y)].symbol(), "│", "the log's rule on row {y}");
        }
        assert_ne!(buf[(118, 40)].symbol(), "│");
        assert!(from(&buf, 120, 2).starts_with("• following · info ▾"), "{}", row(&buf, 2));
        let hint = t!("gui.admin.log.hint_undock").to_string();
        assert_eq!(col(&row(&buf, 2), &hint).map(|x| x + width(&hint) as u16), Some(174), "the hint ends where the log does");
        assert_eq!(from(&buf, 120, 3).trim(), "");
        assert!(from(&buf, 120, 4).starts_with("09:00:"), "{}", row(&buf, 4));
        assert!(from(&buf, 19, 39).starts_with("loading the discovery network…"), "the room's note: {}", row(&buf, 39));
        // Nothing of the room right of its hundred cells.
        assert_eq!(from(&buf, 117, 1).trim(), "");
        for y in 1..40 {
            assert_eq!(buf[(117, y)].symbol(), " ", "row {y}");
            assert_eq!(buf[(119, y)].symbol(), " ", "row {y}");
        }
    }

    #[test]
    fn the_floor_gives_the_room_17_rows_and_no_resize_line() {
        let _en = english();
        let mut gui = admin_gui();
        gui.config.gui.key_hints = true;
        let resize = t!("resize").to_string();
        for id in RoomId::ALL {
            gui.act(Act::Adm(AdmAct::Open(Hall::Room(id))));
            let buf = render_at(&mut gui, 100, 24);
            assert_eq!(gui.admin.room_at, Rect::new(17, 2, 83, 16), "{id:?}");
            let all: Vec<String> = (0..24).map(|y| row(&buf, y)).collect();
            assert!(!all.iter().any(|r| r.contains(&resize)), "{id:?} asked for a bigger window:\n{}", all.join("\n"));
            assert_ne!(from(&buf, 19, 17).trim(), "", "{id:?} keeps its note row:\n{}", all.join("\n"));
            assert_eq!(from(&buf, 0, 1).trim(), "", "{id:?}: nothing above the room");
            for y in 2..18 {
                assert_eq!(buf[(16, y)].symbol(), "│", "{id:?}: nothing on the hallway's rule");
            }
        }
    }

    #[test]
    fn the_spill_guard_blanks_a_form_taller_than_the_room() {
        let _en = english();
        // The Users room's add form at the floor: the bar's rows are the
        // bar's, with the form up as without it.
        let mut gui = admin_gui();
        gui.config.gui.key_hints = true;
        gui.act(Act::Adm(AdmAct::Open(Hall::Room(RoomId::Users))));
        let plain = render_at(&mut gui, 100, 24);
        press(&mut gui, KeyCode::Char('a'));
        assert!(gui.admin.room.as_ref().unwrap().modal_up(), "the add form is up");
        let form = render_at(&mut gui, 100, 24);
        for y in 18..23 {
            assert_eq!(row(&form, y), row(&plain, y), "the bar's row {y}");
        }

        // A room that writes past its area: below it at the floor, right of
        // it beside the docked log. Neither reaches the screen.
        let probe = Probe::default();
        probe.0.borrow_mut().spill = true;
        let mut gui = hosting(&probe);
        gui.config.gui.key_hints = true;
        for (w, h) in [(100, 24), (176, 46)] {
            let buf = render_at(&mut gui, w, h);
            let all: Vec<String> = (0..h).map(|y| row(&buf, y)).collect();
            assert!(all.iter().any(|r| r.contains("probe body")), "the probe drew");
            assert!(!all.iter().any(|r| r.contains("SPILL")), "{w}×{h}:\n{}", all.join("\n"));
        }
    }

    // ── Keys ────────────────────────────────────────────────────────────────

    #[test]
    fn digits_and_q_reach_a_focused_room_before_the_host() {
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        let active = gui.active;
        render_at(&mut gui, 100, 30);
        press(&mut gui, KeyCode::Tab);
        assert_eq!(gui.admin.focus, Focus::Room);
        for c in ['1', '9', 'T', 'M', 'V', 'D', '0', ' '] {
            assert!(!press(&mut gui, KeyCode::Char(c)));
        }
        assert_eq!(
            probe.0.borrow().keys,
            ['1', '9', 'T', 'M', 'V', 'D', '0', ' '].map(KeyCode::Char).to_vec(),
            "every one of them reached the room"
        );
        assert_eq!((gui.screen, gui.active), (Screen::Admin, active), "and none of them moved the GUI");
        assert!(!super::super::vizwin::is_open(&gui));

        assert!(!press(&mut gui, KeyCode::Char('q')), "q is the room's, not the player's quit");
        assert_eq!(probe.0.borrow().keys.last(), Some(&KeyCode::Char('q')));
        assert_eq!(gui.admin.focus, Focus::Hall, "the room's Quit hands the keys to the hallway");
        assert_eq!(gui.admin.cursor, 0, "on the room's row");
        assert!(gui.admin.room.is_some());
    }

    #[test]
    fn esc_at_a_rooms_base_hands_the_keys_to_the_hallway_and_closes_nothing() {
        let mut gui = admin_gui();
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.admin.room_id, Some(RoomId::Libraries), "the tab lands on Libraries");
        assert_eq!(gui.admin.focus, Focus::Hall, "with the keys in the hallway");
        press(&mut gui, KeyCode::Tab);
        assert_eq!(gui.admin.focus, Focus::Room);
        assert!(!press(&mut gui, KeyCode::Esc));
        assert_eq!(gui.admin.focus, Focus::Hall);
        assert_eq!(gui.screen, Screen::Admin);
        assert_eq!(gui.admin.room_id, Some(RoomId::Libraries));
        assert!(gui.admin.room.is_some(), "the room is still up");
        assert_eq!(gui.admin.cursor, 0);
    }

    #[test]
    fn the_hallway_walks_opens_and_leads_out() {
        let mut gui = admin_gui();
        render_at(&mut gui, 100, 30);
        press(&mut gui, KeyCode::Up);
        assert_eq!(gui.admin.cursor, 0, "the cursor stops at the top");
        press(&mut gui, KeyCode::Down);
        press(&mut gui, KeyCode::Down);
        assert_eq!(gui.admin.cursor, 2);
        for _ in 0..10 {
            press(&mut gui, KeyCode::Down);
        }
        assert_eq!(gui.admin.cursor, 6, "the Log row is the last");
        press(&mut gui, KeyCode::Up);
        press(&mut gui, KeyCode::Enter);
        assert_eq!((gui.admin.room_id, gui.admin.focus), (Some(RoomId::Torrents), Focus::Room), "Enter opens with the keys");
        assert_eq!(gui.admin.showing, Hall::Room(RoomId::Torrents));
        press(&mut gui, KeyCode::Tab);
        assert_eq!(gui.admin.focus, Focus::Hall);
        press(&mut gui, KeyCode::Up);
        press(&mut gui, KeyCode::Right);
        assert_eq!((gui.admin.room_id, gui.admin.focus), (Some(RoomId::Federation), Focus::Room), "→ too");
        press(&mut gui, KeyCode::BackTab);
        assert_eq!(gui.admin.focus, Focus::Hall);
        // Enter on the Log row: the Log room, the keys on the log.
        gui.admin.cursor = 6;
        press(&mut gui, KeyCode::Enter);
        assert_eq!((gui.admin.showing, gui.admin.focus), (Hall::Log, Focus::Log));
        press(&mut gui, KeyCode::Esc);
        assert_eq!(gui.admin.focus, Focus::Hall, "Esc in the log hands the keys back");
        assert_eq!(gui.admin.cursor, 6);

        // The transport, as on the Library.
        let paused = gui.demo_paused;
        assert!(!press(&mut gui, KeyCode::Char(' ')));
        assert_ne!(gui.demo_paused, paused, "Space is play/pause");
        let volume = gui.app.volume;
        press(&mut gui, KeyCode::Char('-'));
        assert!(gui.app.volume < volume);

        // The ways out.
        press(&mut gui, KeyCode::Char('2'));
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::ALBUMS_NAV), "a digit is the Library's room");
        assert!(gui.admin.room.is_none(), "leaving drops the room");
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.admin.showing, Hall::Log, "the tab comes back to what it showed");
        press(&mut gui, KeyCode::Char('0'));
        assert_eq!(gui.screen, Screen::NowPlaying);
        press(&mut gui, KeyCode::Char('M'));
        press(&mut gui, KeyCode::Char('T'));
        assert_eq!(gui.screen, Screen::Stats);
        press(&mut gui, KeyCode::Char('M'));
        press(&mut gui, KeyCode::Char('D'));
        assert_eq!((gui.screen, gui.active), (Screen::Library, DJ_NAV));
        press(&mut gui, KeyCode::Char('M'));
        press(&mut gui, KeyCode::Esc);
        assert_eq!(gui.screen, Screen::Library);
        press(&mut gui, KeyCode::Char('M'));
        assert!(press(&mut gui, KeyCode::Char('q')), "q in the hallway quits the player");
    }

    #[test]
    fn l_toggles_the_log_by_placement() {
        let mut gui = admin_gui();

        // Column: L undocks the log into the hallway and docks it again.
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.admin.log_at, Some(Rect::new(120, 2, 54, 39)));
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.admin.log_at, None);
        assert_eq!(gui.admin.room_at.width, 159, "the room takes the log's columns");
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.admin.log_at, Some(Rect::new(120, 2, 54, 39)));
        // A log that had the keys hands them to the room as it goes.
        press(&mut gui, KeyCode::Tab);
        press(&mut gui, KeyCode::Tab);
        assert_eq!(gui.admin.focus, Focus::Log);
        press(&mut gui, KeyCode::Char('L'));
        assert_eq!(gui.admin.focus, Focus::Room);
        press(&mut gui, KeyCode::Char('L'));

        // Band: L hides the band and the room takes its rows.
        render_at(&mut gui, 136, 52);
        assert_eq!(gui.admin.log_at, Some(Rect::new(19, 39, 115, 8)));
        assert_eq!(gui.admin.room_at.height, 36);
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 136, 52);
        assert_eq!(gui.admin.log_at, None);
        assert_eq!(gui.admin.room_at.height, 45, "the room takes the band's rows");
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 136, 52);
        assert!(gui.admin.log_at.is_some());

        // Room: L opens the Log room, and goes back to the last room.
        render_at(&mut gui, 136, 40);
        assert_eq!(gui.admin.focus, Focus::Room);
        press(&mut gui, KeyCode::Char('L'));
        assert_eq!((gui.admin.showing, gui.admin.focus), (Hall::Log, Focus::Log));
        render_at(&mut gui, 136, 40);
        assert_eq!(gui.admin.log_at, Some(Rect::new(19, 2, 115, 33)));
        press(&mut gui, KeyCode::Char('L'));
        assert_eq!((gui.admin.showing, gui.admin.focus), (Hall::Room(RoomId::Libraries), Focus::Room));

        // Widening with the Log room open hands it back to the last room,
        // the log keeping the keys beside it.
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 176, 46);
        assert_eq!((gui.admin.showing, gui.admin.focus), (Hall::Room(RoomId::Libraries), Focus::Log));
        assert!(gui.admin.log_at.is_some());
    }

    #[test]
    fn the_log_room_folding_back_puts_the_hallway_cursor_on_the_room_it_shows() {
        let mut gui = admin_gui();
        gui.act(Act::Adm(AdmAct::Open(Hall::Room(RoomId::Federation))));
        render_at(&mut gui, 136, 40);
        press(&mut gui, KeyCode::Char('L'));
        assert_eq!((gui.admin.showing, gui.admin.cursor), (Hall::Log, 6));
        // Wide enough to dock: the Log room folds back into Federation.
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.admin.showing, Hall::Room(RoomId::Federation));
        assert_eq!(gui.admin.cursor, 4, "Federation's row, not where the Log's index falls");
        press(&mut gui, KeyCode::Tab);
        assert_eq!(gui.admin.focus, Focus::Hall);
        press(&mut gui, KeyCode::Enter);
        assert_eq!((gui.admin.showing, gui.admin.focus), (Hall::Room(RoomId::Federation), Focus::Room), "Enter opens the room shown");
    }

    #[test]
    fn a_room_that_claims_every_key_takes_the_focus_from_the_hallway_and_the_log() {
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.admin.focus, Focus::Hall);

        // A modal opens on its own while the hallway has the keys — a folder
        // listing answering after the keys moved on.
        probe.0.borrow_mut().modal = true;
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.admin.focus, Focus::Room, "the modal takes the focus");
        for code in [KeyCode::Down, KeyCode::Enter, KeyCode::Char('2'), KeyCode::Char('M'), KeyCode::Tab, KeyCode::Char('L')] {
            assert!(!press(&mut gui, code));
        }
        let keys = probe.0.borrow().keys.clone();
        assert_eq!(keys.len(), 6, "every key reached the room: {keys:?}");
        assert_eq!((gui.screen, gui.admin.cursor, gui.admin.log_hidden), (Screen::Admin, 0, false), "and none moved the host");

        // The same from the log, and for a text field (no modal) as well.
        probe.0.borrow_mut().modal = false;
        press(&mut gui, KeyCode::Tab);
        assert_eq!(gui.admin.focus, Focus::Log);
        probe.0.borrow_mut().claim = Claim::All;
        assert!(!press(&mut gui, KeyCode::Down));
        assert_eq!(gui.admin.focus, Focus::Room);
        assert_eq!(probe.0.borrow().keys.last(), Some(&KeyCode::Down), "the log's ↓ was the room's");
        assert!(gui.admin.log.model.following(), "and the log did not scroll");
    }

    // ── The pointer ─────────────────────────────────────────────────────────

    #[test]
    fn the_pointer_reaches_the_room_inside_its_area_and_lets_go_outside() {
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        render_at(&mut gui, 100, 30);
        let mice = |probe: &Probe| probe.0.borrow().mice.clone();

        down(&mut gui, 30, 5);
        assert_eq!(mice(&probe), vec![(MouseEventKind::Down(MouseButton::Left), 30, 5)]);
        assert_eq!(gui.admin.focus, Focus::Room, "a press in the room hands it the keys");
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), 30, 5);
        mouse(&mut gui, MouseEventKind::Moved, 40, 6);
        assert_eq!(mice(&probe).len(), 3);

        let left = probe.0.borrow().left;
        mouse(&mut gui, MouseEventKind::Moved, 5, 6);
        assert_eq!(mice(&probe).len(), 3, "the hallway's motion is not the room's");
        assert_eq!(probe.0.borrow().left, left + 1, "and lets go of its hover");
        assert_eq!(gui.ui.pointer, Some(Position::new(5, 6)), "the GUI's surface follows it");

        // A press that began in the room keeps its drag and its release.
        down(&mut gui, 30, 6);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 5, 6);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), 5, 7);
        let all = mice(&probe);
        assert_eq!(&all[all.len() - 2..], [(MouseEventKind::Drag(MouseButton::Left), 5, 6), (MouseEventKind::Up(MouseButton::Left), 5, 7)]);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 5, 8);
        assert_eq!(mice(&probe).len(), all.len(), "after the release a drag outside is not the room's");

        // The room's first row is the pick banner's.
        down(&mut gui, 30, 1);
        assert_eq!(mice(&probe).len(), all.len());

        // The wheel over the room is the room's.
        mouse(&mut gui, MouseEventKind::ScrollDown, 50, 10);
        assert_eq!(mice(&probe).last(), Some(&(MouseEventKind::ScrollDown, 50, 10)));

        // The hand follows the room's clickables while it is drawn.
        assert!(frame(&mut gui));
        assert!(probe.0.borrow().frames > 0);
    }

    #[test]
    fn a_press_in_the_room_released_on_the_top_bar_is_still_the_rooms() {
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        render_at(&mut gui, 100, 30);
        let mice = |probe: &Probe| probe.0.borrow().mice.clone();

        // A thumb dragged to the top overshoots onto the bar's row: the
        // drag and the release there are the room's, so its held arrow or
        // thumb lets go with the button.
        down(&mut gui, 30, 5);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 30, 0);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), 30, 0);
        let all = mice(&probe);
        assert_eq!(
            &all[all.len() - 2..],
            [(MouseEventKind::Drag(MouseButton::Left), 30, 0), (MouseEventKind::Up(MouseButton::Left), 30, 0)]
        );

        // Released: a press and a drag on the hallway's empty cells are not
        // the room's.
        down(&mut gui, 5, 16);
        assert_eq!(gui.admin.focus, Focus::Hall);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 5, 10);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), 5, 10);
        assert_eq!(mice(&probe).len(), all.len(), "a hallway drag after the release is not the room's");

        // With no button held, the bar's row is the GUI's, and the room
        // lets go of its hover there.
        mouse(&mut gui, MouseEventKind::Moved, 30, 5);
        let left = probe.0.borrow().left;
        mouse(&mut gui, MouseEventKind::Moved, 30, 0);
        assert_eq!(mice(&probe).len(), all.len() + 1, "only the move inside the room reached it");
        assert_eq!(probe.0.borrow().left, left + 1, "the room was told the pointer left");
    }

    #[test]
    fn a_room_modal_swallows_clicks_on_the_hallway() {
        let _en = english();
        let probe = Probe::default();
        probe.0.borrow_mut().modal = true;
        let mut gui = hosting(&probe);
        let buf = render_at(&mut gui, 100, 30);
        down(&mut gui, 5, 4);
        assert_eq!(probe.0.borrow().mice.last(), Some(&(MouseEventKind::Down(MouseButton::Left), 5, 4)));
        assert_eq!(gui.admin.room_id, Some(RoomId::Libraries), "the hallway's Users row did not open");
        assert_eq!(gui.admin.showing, Hall::Room(RoomId::Libraries));
        down(&mut gui, 50, 27);
        assert_eq!(probe.0.borrow().mice.len(), 2, "nor did the bar take it");

        // The top bar stays the GUI's.
        let lx = col(&row(&buf, 0), " Library ").unwrap();
        down(&mut gui, lx + 1, 0);
        assert_eq!(probe.0.borrow().mice.len(), 2);
        assert_eq!(gui.screen, Screen::Library);
    }

    #[test]
    fn under_a_room_modal_nothing_of_the_guis_lights_or_shows_the_hand() {
        let _en = english();
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        let buf = render_at(&mut gui, 100, 30);
        let lx = col(&row(&buf, 0), " Library ").unwrap();
        let bar = gui.ui.clicks.iter().find(|(rect, _)| rect.y >= 25).map(|(rect, _)| Position::new(rect.x, rect.y)).expect("a bar control");

        // Without a modal the hallway's row and the bar's control light.
        mouse(&mut gui, MouseEventKind::Moved, 5, 4);
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(buf[(3, 4)].fg, th().bright, "the Users row under the pointer");
        assert!(gui.ui.hovering_clickable());

        // The modal opens by a key with the pointer resting there.
        probe.0.borrow_mut().modal = true;
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(buf[(3, 4)].fg, dim().fg.unwrap(), "the row stays dim under the modal");
        assert!(!gui.ui.hovering_clickable(), "and shows no hand");
        mouse(&mut gui, MouseEventKind::Moved, bar.x, bar.y);
        render_at(&mut gui, 100, 30);
        assert!(!gui.ui.hovering_clickable(), "nor does the bar's control the modal swallows");

        // The top bar's row stays the GUI's.
        mouse(&mut gui, MouseEventKind::Moved, lx + 1, 0);
        render_at(&mut gui, 100, 30);
        assert!(gui.ui.hovering_clickable(), "the Library tab answers");
    }

    #[test]
    fn the_server_menu_and_the_log_menu_own_the_pointer_over_the_room() {
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        gui.servers.drop_open = true;
        render_at(&mut gui, 176, 46);
        down(&mut gui, 30, 5);
        assert!(probe.0.borrow().mice.is_empty(), "the server menu's catcher took it");
        assert!(!gui.servers.drop_open, "and closed the menu");

        render_at(&mut gui, 176, 46);
        gui.admin.log.act(LogAct::Menu);
        render_at(&mut gui, 176, 46);
        mouse(&mut gui, MouseEventKind::Moved, 30, 6);
        down(&mut gui, 30, 5);
        assert!(probe.0.borrow().mice.is_empty(), "the level menu's catcher took it");
        assert_eq!(gui.admin.log.menu, None, "and closed the menu");
        assert_eq!(gui.screen, Screen::Admin);
    }

    #[test]
    fn a_click_on_a_hallway_row_opens_that_room_with_the_keys() {
        let _en = english();
        let mut gui = admin_gui();
        render_at(&mut gui, 100, 30);
        down(&mut gui, 5, 4);
        assert_eq!(gui.admin.room_id, Some(RoomId::Users));
        assert_eq!((gui.admin.showing, gui.admin.focus, gui.admin.cursor), (Hall::Room(RoomId::Users), Focus::Room, 1));
        let buf = render_at(&mut gui, 100, 30);
        assert!(from(&buf, 1, 4).starts_with("▸ Users"));
        assert_eq!(buf[(3, 4)].fg, th().accent);
        // Between the rows the click still hands the hallway the keys.
        down(&mut gui, 5, 15);
        assert_eq!(gui.admin.focus, Focus::Hall);
        assert_eq!(gui.admin.room_id, Some(RoomId::Users));
        // The Log row opens the Log room with the keys on the log.
        down(&mut gui, 5, 13);
        assert_eq!((gui.admin.showing, gui.admin.focus), (Hall::Log, Focus::Log));
    }

    #[test]
    fn the_log_row_counts_lines_since_the_log_was_last_on_screen() {
        let _en = english();
        let mut gui = admin_gui();
        // The row from the label to the rule.
        let count = |gui: &mut Gui| {
            let buf = render_at(gui, 136, 40);
            (3..16).map(|x| buf[(x, 13)].symbol()).collect::<String>().trim_end().to_string()
        };
        assert_eq!(count(&mut gui), "Log", "nothing before the first answer");
        gui.admin.log.model.take(lines(1, 5));
        assert_eq!(count(&mut gui), "Log", "what the ring held when the tab opened is not new");
        gui.admin.log.model.take(lines(6, 3));
        assert_eq!(count(&mut gui), "Log · 3 new");
        gui.admin.log.model.take(LogTail { entries: vec![entry(9, "debug", "quiet")], last_seq: 9, capacity: 1000 });
        assert_eq!(count(&mut gui), "Log · 3 new", "a line below the level does not count");

        // On screen in the Log room, they are seen.
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 136, 40);
        press(&mut gui, KeyCode::Char('L'));
        assert_eq!(count(&mut gui), "Log");
        gui.admin.log.model.take(lines(10, 1));
        assert_eq!(count(&mut gui), "Log · 1 new");
        // Docked in a wider window it is on screen too.
        render_at(&mut gui, 176, 46);
        assert_eq!(count(&mut gui), "Log");

        // Short of cells, the words go before the number, and the number
        // before the label: Spanish's "Registro" leaves five.
        assert_eq!(log_count(6, "· 12 new", 12).as_deref(), Some(" · 12 new"));
        assert_eq!(log_count(11, "· 12 nuevas", 12).as_deref(), Some(" · 12"));
        assert_eq!(log_count(11, "· 123 nuevas", 123), None);
        assert_eq!(log_count(16, "· 1 new", 1), None);
    }

    #[test]
    fn the_focused_column_wears_the_accent_rule_and_the_hallway_its_cursor_slab() {
        let _en = english();
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        let dim_fg = dim().fg.unwrap();

        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(buf[(1, 3)].bg, th().accent, "the cursor row wears the slab");
        assert_eq!(buf[(15, 3)].bg, th().accent, "across to the rule");
        assert_ne!(buf[(1, 4)].bg, th().accent);
        assert_eq!(buf[(16, 5)].fg, dim_fg);
        assert_eq!(buf[(118, 5)].fg, dim_fg);

        press(&mut gui, KeyCode::Tab);
        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(buf[(16, 5)].fg, th().accent, "the room has the keys: the hallway's rule lights");
        assert_ne!(buf[(1, 3)].bg, th().accent, "and the slab is gone");
        assert_eq!(buf[(1, 3)].fg, th().accent, "the open row keeps its accent");
        assert_eq!(buf[(118, 5)].fg, dim_fg);

        press(&mut gui, KeyCode::Tab);
        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(buf[(118, 5)].fg, th().accent, "the log has them: its rule lights");
        assert_eq!(buf[(16, 5)].fg, dim_fg);

        let buf = render_at(&mut gui, 136, 52);
        assert_eq!(gui.admin.focus, Focus::Log);
        assert_eq!(buf[(19, 38)].fg, th().accent, "the band's rule");

        // The Log room lights the hallway's rule.
        render_at(&mut gui, 136, 40);
        assert_eq!(gui.admin.focus, Focus::Room, "the log left the screen: the keys go to the room");
        press(&mut gui, KeyCode::Char('L'));
        assert_eq!(gui.admin.focus, Focus::Log);
        let buf = render_at(&mut gui, 136, 40);
        assert_eq!(buf[(16, 5)].fg, th().accent, "the Log room has the keys: the hallway's rule lights");
    }

    // ── The footer and the rest ─────────────────────────────────────────────

    #[test]
    fn the_footer_carries_the_room_hint_and_the_host_tail_only_when_it_fits() {
        let _en = english();
        let probe = Probe::default();
        probe.0.borrow_mut().tips = "x probe".into();
        let mut gui = hosting(&probe);
        gui.config.gui.key_hints = true;
        let footer = |gui: &mut Gui| from(&render_at(gui, 100, 30), 1, 29).trim_end().to_string();

        assert_eq!(footer(&mut gui), t!("gui.admin.tips_hall"), "the hallway's keys");
        press(&mut gui, KeyCode::Tab);
        assert_eq!(footer(&mut gui), "x probe · Tab focus · L log");
        probe.0.borrow_mut().claim = Claim::OwnTab;
        assert_eq!(footer(&mut gui), "x probe · L log", "a room that keeps Tab");
        probe.0.borrow_mut().claim = Claim::All;
        assert_eq!(footer(&mut gui), "x probe", "a room that holds every key");
        probe.0.borrow_mut().claim = Claim::Open;

        // The tail only when the whole line fits the footer's 98 cells.
        let tail = " · Tab focus · L log";
        probe.0.borrow_mut().tips = "y".repeat(98 - width(tail));
        assert!(footer(&mut gui).ends_with(tail), "it fits exactly");
        probe.0.borrow_mut().tips = "y".repeat(99 - width(tail));
        assert_eq!(footer(&mut gui), "y".repeat(99 - width(tail)), "one cell over: the room's hint alone");

        // The log's keys, and its menu's.
        let buf = render_at(&mut gui, 176, 46);
        assert!(from(&buf, 1, 45).trim_end().ends_with(tail), "a wider window has room for the tail");
        press(&mut gui, KeyCode::Tab);
        assert_eq!(gui.admin.focus, Focus::Log);
        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(from(&buf, 1, 45).trim_end(), t!("gui.admin.tips_log"));
        press(&mut gui, KeyCode::Enter);
        assert!(gui.admin.log.menu.is_some(), "Enter opens the level menu");
        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(from(&buf, 1, 45).trim_end(), t!("gui.admin.tips_log_menu"));
        press(&mut gui, KeyCode::Down);
        press(&mut gui, KeyCode::Enter);
        assert_eq!(gui.admin.log.model.level, server_log::Level::Debug, "the menu took the keys");
        assert_eq!(gui.admin.focus, Focus::Log);
    }

    #[test]
    fn no_session_is_one_sentence_and_esc_leads_out() {
        let _en = english();
        let mut gui = gui();
        gui.config.gui.key_hints = true;
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.screen, Screen::Admin);
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(from(&buf, 19, 2).trim_end(), t!("gui.admin.no_session"));
        assert!(from(&buf, 1, 2).starts_with("SERVER"), "the hallway still names the rooms");
        assert!(!(0..30).any(|y| row(&buf, y).contains("WATCH")), "no log without a session");
        assert_eq!(from(&buf, 1, 29).trim_end(), t!("gui.tips.stats_back"));
        assert_eq!(from(&buf, 19, 3).trim(), "", "one sentence");
        let hallway = (2..25).map(|y| (0..16).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(!hallway.contains(forward_glyph()), "no room is open:\n{hallway}");

        // Nothing to move the keys to, nothing for L to show.
        press(&mut gui, KeyCode::Tab);
        press(&mut gui, KeyCode::Char('L'));
        press(&mut gui, KeyCode::Enter);
        assert_eq!(gui.admin.focus, Focus::Hall);
        assert!(gui.admin.room.is_none());
        press(&mut gui, KeyCode::Esc);
        assert_eq!(gui.screen, Screen::Library);
    }

    #[test]
    fn the_no_session_sentence_wraps_onto_a_second_row_rather_than_be_cut() {
        let _en = english();
        // Six translations outrun the floor's 79 cells; the sentence is set
        // in each one's words without moving the process's locale.
        let mut gui = gui();
        press(&mut gui, KeyCode::Char('M'));
        for locale in ["de", "es", "fr", "it", "pl", "pt"] {
            let sentence = t!("gui.admin.no_session", locale = locale).to_string();
            gui.admin.why = Some(sentence.clone());
            let buf = render_at(&mut gui, 100, 30);
            let (first, second) = (from(&buf, 19, 2).trim_end().to_string(), from(&buf, 19, 3).trim_end().to_string());
            assert!(!second.is_empty(), "{locale}: two rows");
            assert_eq!(format!("{first} {second}"), sentence, "{locale}: every word, none cut");
            assert!(width(&first) <= 79 && width(&second) <= 79, "{locale}: {first} / {second}");
        }
        // What two rows cannot hold is cut at the second's end.
        gui.admin.why = Some("word ".repeat(40).trim_end().to_string());
        let buf = render_at(&mut gui, 100, 30);
        assert!(from(&buf, 19, 3).trim_end().ends_with('…'));
        assert_eq!(from(&buf, 19, 4).trim(), "");
    }

    #[test]
    fn the_room_surface_follows_the_key_hints_setting() {
        let _en = english();
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(probe.0.borrow().hints, Some(false), "hints off: the room's tooltips name no keys");
        let hint = t!("gui.admin.log.hint_undock").to_string();
        assert!(!row(&buf, 2).contains(&hint), "nor does the log's header");

        gui.config.gui.key_hints = true;
        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(probe.0.borrow().hints, Some(true));
        assert!(row(&buf, 2).contains(&hint));
        // With the footer on, the band's header is on row 38.
        let buf = render_at(&mut gui, 136, 52);
        assert!(row(&buf, 38).contains(&*t!("gui.admin.log.hint_hide")), "{}", row(&buf, 38));
        press(&mut gui, KeyCode::Char('L'));
        let buf = render_at(&mut gui, 136, 52);
        assert!(!(0..52).any(|y| row(&buf, y).contains(&*t!("gui.admin.log.hint_hide"))), "the band is hidden");
    }

    #[test]
    fn the_mini_player_takes_no_room_clicks() {
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        render_at(&mut gui, 100, 30);
        assert!(!gui.admin.room_at.is_empty());
        render_at(&mut gui, 90, 20);
        assert_eq!(gui.admin.room_at, Rect::default(), "the mini player drew no room");
        assert_eq!(gui.admin.log_at, None);
        down(&mut gui, 30, 5);
        mouse(&mut gui, MouseEventKind::ScrollDown, 30, 5);
        assert!(probe.0.borrow().mice.is_empty());
        assert!(!frame(&mut gui), "no hand for a room not on screen");
        assert!(probe.0.borrow().frames > 0, "though it is still pumped");
    }

    #[test]
    fn the_wheel_scrolls_the_log_under_it_and_pauses_it() {
        let _en = english();
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 60));
        render_at(&mut gui, 176, 46);
        assert!(gui.admin.log.model.following());
        mouse(&mut gui, MouseEventKind::ScrollUp, 140, 20);
        assert!(!gui.admin.log.model.following(), "up reads older lines");
        let buf = render_at(&mut gui, 176, 46);
        assert!(from(&buf, 120, 2).starts_with("• paused"), "{}", row(&buf, 2));
        // The paused word follows again.
        down(&mut gui, 122, 2);
        assert!(gui.admin.log.model.following());
        assert_eq!(gui.admin.focus, Focus::Log, "a click in the log hands it the keys");
        mouse(&mut gui, MouseEventKind::ScrollUp, 5, 20);
        assert!(gui.admin.log.model.following(), "the hallway scrolls nothing");
    }

    #[test]
    fn a_session_change_rebuilds_the_room_and_keeps_what_was_shown() {
        // The probe stands in for a Federation room on the old session.
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        gui.admin.room_id = Some(RoomId::Federation);
        gui.admin.showing = Hall::Room(RoomId::Federation);
        render_at(&mut gui, 100, 30);
        assert_eq!(Rc::strong_count(&probe.0), 2);
        reopen(&mut gui);
        assert_eq!(Rc::strong_count(&probe.0), 1, "the old session's room is gone");
        assert!(gui.admin.room.is_some(), "a new room on the new session");
        assert_eq!(gui.admin.room_id, Some(RoomId::Federation), "the room shown is kept");
        assert!(gui.admin.log.running(), "and a new poll");
        gui.act(Act::Nav(0));
        assert!(gui.admin.room.is_none() && !gui.admin.log.running(), "leaving drops both");
        reopen(&mut gui);
        assert!(gui.admin.room.is_none(), "a session change off the tab opens nothing");
    }

    // ── The room's field in the window ──────────────────────────────────────

    #[test]
    fn a_rooms_focused_field_notes_its_caret_on_the_guis_surface_where_it_draws() {
        // The window asks the GUI's surface whether a field has the
        // keyboard, to turn its paste and its input method on, and where
        // the caret is, to float the candidates there (clause 28): the
        // typed-path field of the real Libraries room answers through the
        // host, at the cell its caret is drawn in.
        let _english = english();
        let mut gui = admin_gui();
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), None, "the room has no field open");
        press(&mut gui, KeyCode::Enter);
        assert_eq!(gui.admin.focus, Focus::Room);
        press(&mut gui, KeyCode::Char('t'));
        let buf = render_at(&mut gui, 100, 30);
        let at = gui.ui.caret_at().expect("the typed-path field has the keyboard");
        assert_eq!(buf[(at.x, at.y)].symbol(), "▏", "{}", row(&buf, at.y));
        for c in ['a', 'b', 'c'] {
            press(&mut gui, KeyCode::Char(c));
        }
        let typed = Position { x: at.x + 3, y: at.y };
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), Some(typed), "after what was typed");
        assert!(from(&buf, at.x, at.y).starts_with("abc▏"), "{}", row(&buf, at.y));

        // The Log room hides the room, field and all.
        show(&mut gui, Hall::Log);
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), None, "the Log room shows: no field has the keyboard");
        show(&mut gui, Hall::Room(RoomId::Libraries));
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), Some(typed), "the room back, and its field with it");

        press(&mut gui, KeyCode::Esc);
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), None, "the field went with its modal");
    }

    #[test]
    fn the_input_methods_composition_draws_in_the_rooms_field_before_the_caret() {
        let _english = english();
        let mut gui = admin_gui();
        press(&mut gui, KeyCode::Enter);
        press(&mut gui, KeyCode::Char('t'));
        press(&mut gui, KeyCode::Char('a'));
        press(&mut gui, KeyCode::Char('b'));
        render_at(&mut gui, 100, 30);
        let at = gui.ui.caret_at().expect("the typed-path field has the keyboard");
        // The window sets the composition on the GUI's surface; the host
        // hands it down before every draw, so it stays in the room's field
        // from frame to frame until the window lets it go.
        gui.ui.set_composition("にほ");
        for frame in 0..2 {
            let buf = render_at(&mut gui, 100, 30);
            let cells = [(at.x - 2, "a"), (at.x - 1, "b"), (at.x, "に"), (at.x + 2, "ほ"), (at.x + 4, "▏")];
            for (x, want) in cells {
                assert_eq!(buf[(x, at.y)].symbol(), want, "frame {frame}: {}", row(&buf, at.y));
            }
            let after = Position { x: at.x + 4, y: at.y };
            assert_eq!(gui.ui.caret_at(), Some(after), "the caret after the kana's four cells");
        }
        gui.ui.set_composition("");
        let buf = render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), Some(at), "let go, the value is as it was");
        assert_eq!(buf[(at.x, at.y)].symbol(), "▏");
    }

    #[test]
    fn the_host_lifts_a_rooms_caret_only_while_the_room_is_drawn_and_has_the_keys() {
        let probe = Probe::default();
        let noted = Position { x: 40, y: 9 };
        probe.0.borrow_mut().caret = Some(noted);
        let mut gui = hosting(&probe);
        gui.ui.set_composition("にほ");
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), Some(noted), "the room's caret, on the GUI's surface");
        assert_eq!(probe.0.borrow().compositions, ["にほ"], "the composition went down before the draw");

        show(&mut gui, Hall::Log);
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), None, "the Log room draws no room, so no caret comes up");
        assert_eq!(probe.0.borrow().compositions.len(), 1, "and no composition goes down");

        // The add-server form opens on its chooser, which has no field:
        // laid over the room's, it takes the keys as keys.
        show(&mut gui, Hall::Room(RoomId::Libraries));
        super::super::servers::open_add(&mut gui);
        render_at(&mut gui, 100, 30);
        assert_eq!(gui.ui.caret_at(), None, "the GUI's modal laid itself over the note");
        let handed = probe.0.borrow().compositions.last().cloned();
        assert_eq!(handed.as_deref(), Some(""), "a composition under a GUI modal is its own field's");

        // The log's level menu takes every key (clause 20), the note too.
        gui.servers.form = None;
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.ui.caret_at(), Some(noted));
        gui.admin.log.act(LogAct::Menu);
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.ui.caret_at(), None, "the level menu is up over the room");

        // No session: no room, no caret.
        let mut lone = self::gui();
        press(&mut lone, KeyCode::Char('M'));
        render_at(&mut lone, 100, 30);
        assert!(lone.admin.room.is_none());
        assert_eq!(lone.ui.caret_at(), None);
    }

    // ── Copying and saving the log ──────────────────────────────────────────

    /// The first row of the log's lines this frame, read off the buffer.
    fn first_line_row(buf: &Buffer, log: Rect) -> u16 {
        (log.y..log.bottom()).find(|&y| from(buf, log.x, y).starts_with("09:")).expect("a line on screen")
    }

    /// Whether row `y` of the log wears the selection colours.
    fn lit(buf: &Buffer, log: Rect, y: u16) -> bool {
        let cell = &buf[(log.x + 20, y)];
        cell.bg == th().accent && cell.fg == th().on_accent
    }

    #[test]
    fn a_drag_over_the_logs_lines_highlights_them_in_every_placement() {
        let _en = english();
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 60));
        for (w, h, log_room) in [(176, 46, false), (136, 52, false), (100, 30, true)] {
            gui.admin.log.model.clear_highlight();
            gui.admin.focus = Focus::Hall;
            if log_room {
                render_at(&mut gui, w, h);
                press(&mut gui, KeyCode::Char('L'));
                assert_eq!(gui.admin.showing, Hall::Log);
                gui.admin.focus = Focus::Hall;
            }
            let buf = render_at(&mut gui, w, h);
            let log = gui.admin.log_at.expect("the log is on screen");
            let top = first_line_row(&buf, log);
            down(&mut gui, log.x + 3, top + 1);
            assert!(gui.ui.gripping(), "{w}×{h}: the press took the lines");
            mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), log.x + 9, top + 2);
            mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), log.x + 9, top + 3);
            mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), log.x + 9, top + 3);
            assert!(!gui.ui.gripping());
            assert_eq!(gui.admin.focus, Focus::Log, "{w}×{h}: the log has the keys");
            assert_eq!(gui.admin.log.model.highlighted().len(), 3, "{w}×{h}");
            let buf = render_at(&mut gui, w, h);
            for y in top + 1..=top + 3 {
                assert!(lit(&buf, log, y), "{w}×{h}: row {y}: {}", row(&buf, y));
            }
            for y in [top, top + 4] {
                assert!(!lit(&buf, log, y), "{w}×{h}: row {y} beside the run");
            }
        }
    }

    #[test]
    fn a_drag_from_the_log_lights_nothing_else_and_a_release_over_the_room_ends_it() {
        let _en = english();
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        gui.admin.log.model.take(lines(1, 5));
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let top = first_line_row(&buf, log);
        let press = Position::new(log.x + 3, top + 1);
        down(&mut gui, press.x, press.y);
        assert!(gui.ui.gripping());

        // Across the hallway's Users row: nothing there lights.
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 5, 4);
        assert_eq!(gui.ui.pointer, Some(press), "the hand is held at the press");
        let buf = render_at(&mut gui, 176, 46);
        assert_ne!(buf[(3, 4)].fg, th().bright, "the Users row stays as it was");

        // Over the room, and let go there: the room is told nothing, and
        // the grip ends.
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 30, 20);
        mouse(&mut gui, MouseEventKind::Moved, 31, 20);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), 31, 20);
        assert!(probe.0.borrow().mice.is_empty(), "{:?}", probe.0.borrow().mice);
        assert!(!gui.ui.gripping(), "the release over the room ended the grip");
        assert_eq!(gui.admin.log.model.highlight, Some((2, 5)), "clamped to the last line drawn");
        assert_eq!(gui.admin.focus, Focus::Log, "the release handed the room nothing");

        // Free again, the room has the pointer inside it.
        mouse(&mut gui, MouseEventKind::Moved, 32, 20);
        assert_eq!(probe.0.borrow().mice, [(MouseEventKind::Moved, 32, 20)]);
    }

    #[test]
    fn a_drag_released_over_a_hallway_row_lights_it_at_once() {
        let _en = english();
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 20));
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let top = first_line_row(&buf, log);
        down(&mut gui, log.x + 3, top + 1);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 5, 4);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), 5, 4);
        assert_eq!(gui.ui.pointer, Some(Position::new(5, 4)), "hover at the release, with no move after");
        let buf = render_at(&mut gui, 176, 46);
        assert_eq!(buf[(3, 4)].fg, th().bright, "the Users row lights: {}", row(&buf, 4));
    }

    #[test]
    fn a_press_after_a_lost_release_ends_the_grip_and_acts_as_any_press() {
        let _en = english();
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 60));
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let top = first_line_row(&buf, log);
        down(&mut gui, log.x + 3, top + 1);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), log.x + 3, top + 3);
        assert!(gui.ui.gripping());

        // The terminal never sent the Up (the focus left mid-drag). The
        // next press, on the hallway's Users row, ends the grip there and
        // opens the room, as it would have with the release seen.
        down(&mut gui, 5, 4);
        assert!(!gui.ui.gripping(), "the press ended the grip");
        assert_eq!(gui.admin.room_id, Some(RoomId::Users), "and was a press like any other");
        assert_eq!(gui.admin.focus, Focus::Room);
        let highlight = gui.admin.log.model.highlight;
        assert!(highlight.is_some(), "the dragged run stays highlighted");
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), 5, 4);

        // Free again: a bare move over the lines goes where the hand goes
        // and extends nothing.
        render_at(&mut gui, 176, 46);
        mouse(&mut gui, MouseEventKind::Moved, log.x + 3, top + 9);
        assert_eq!(gui.ui.pointer, Some(Position::new(log.x + 3, top + 9)));
        assert_eq!(gui.admin.log.model.highlight, highlight);
    }

    #[test]
    fn leaving_the_screen_mid_drag_lets_go_of_the_grip() {
        let _en = english();
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 60));
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let top = first_line_row(&buf, log);
        down(&mut gui, log.x + 3, top + 1);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), log.x + 3, top + 3);
        let held = gui.admin.log.model.highlight;
        assert!(gui.ui.gripping());

        // The Library comes up with the Up never sent: the first event
        // after finds no log to hold the pointer for.
        gui.act(Act::Screen(Screen::Library));
        render_at(&mut gui, 176, 46);
        mouse(&mut gui, MouseEventKind::Moved, 40, 10);
        assert!(!gui.ui.gripping(), "the grip went with the screen");
        assert_eq!(gui.ui.pointer, Some(Position::new(40, 10)), "hover follows the hand again");
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), 40, 12);
        assert_eq!(gui.admin.log.model.highlight, held, "and a drag there moves nothing of the log's");
    }

    #[test]
    fn a_plain_click_on_the_lines_clears_the_highlight_and_gives_the_log_the_keys() {
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 20));
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let top = first_line_row(&buf, log);
        gui.admin.log.model.highlight = Some((5, 8));
        assert_eq!(gui.admin.focus, Focus::Hall);

        // A click elsewhere in the log (its header) keeps it.
        down(&mut gui, log.x + 40, log.y);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), log.x + 40, log.y);
        assert_eq!(gui.admin.log.model.highlight, Some((5, 8)));
        gui.admin.focus = Focus::Hall;

        down(&mut gui, log.x + 3, top + 2);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), log.x + 3, top + 2);
        assert_eq!(gui.admin.log.model.highlight, None);
        assert_eq!(gui.admin.focus, Focus::Log);
        assert!(!gui.ui.gripping());
    }

    #[test]
    fn a_room_modal_keeps_a_press_on_the_log_the_rooms() {
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        gui.admin.log.model.take(lines(1, 20));
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let top = first_line_row(&buf, log);
        probe.0.borrow_mut().modal = true;
        render_at(&mut gui, 176, 46);
        down(&mut gui, log.x + 3, top + 1);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), log.x + 3, top + 4);
        assert_eq!(probe.0.borrow().mice.first(), Some(&(MouseEventKind::Down(MouseButton::Left), log.x + 3, top + 1)));
        assert!(!gui.ui.gripping(), "nothing grips under the room's modal");
        assert_eq!(gui.admin.log.model.highlight, None);
        assert_eq!(gui.admin.focus, Focus::Room);
    }

    #[test]
    fn hiding_the_log_mid_drag_lets_go_of_the_hold() {
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 60));
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let top = first_line_row(&buf, log);
        down(&mut gui, log.x + 3, top + 1);
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), log.x + 3, top + 3);
        let held = gui.admin.log.model.highlight;
        assert!(held.is_some());

        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 176, 46);
        assert_eq!(gui.admin.log_at, None, "L hid it mid-drag");
        mouse(&mut gui, MouseEventKind::Drag(MouseButton::Left), log.x + 3, top + 9);
        assert_eq!(gui.admin.log.model.highlight, held, "a later drag moves nothing");
        assert!(!gui.ui.gripping(), "the grip went with the log");
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Left), log.x + 3, top + 9);
        assert_eq!(gui.admin.log.model.highlight, held, "L keeps the highlight");
        assert_eq!(gui.admin.focus, Focus::Room, "L handed the log's keys to the room, and the release took nothing back");
    }

    #[test]
    fn y_in_the_log_copies_and_the_note_says_how() {
        let _en = english();
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 3));
        render_at(&mut gui, 176, 46);
        gui.admin.focus = Focus::Log;
        crate::kit::clipboard::catch(crate::kit::clipboard::Copied::Clipboard);
        assert!(!press(&mut gui, KeyCode::Char('y')));
        assert_eq!(crate::kit::clipboard::caught(), ["09:00:01  line 1\n09:00:02  line 2\n09:00:03  line 3"]);
        assert_eq!(gui.note, Some((t!("gui.admin.log.copied_all").to_string(), false)));
        let buf = render_at(&mut gui, 176, 46);
        assert!((0..46).any(|y| row(&buf, y).contains(&*t!("gui.admin.log.copied_all"))), "the note is on screen");

        crate::kit::clipboard::catch(crate::kit::clipboard::Copied::Failed);
        gui.admin.log.model.highlight = Some((2, 2));
        press(&mut gui, KeyCode::Char('y'));
        assert_eq!(crate::kit::clipboard::caught(), ["09:00:02  line 2"]);
        assert_eq!(gui.note, Some((t!("gui.admin.log.copy_failed").to_string(), true)));

        // The header's control, by the pointer.
        crate::kit::clipboard::catch(crate::kit::clipboard::Copied::Terminal);
        let buf = render_at(&mut gui, 176, 46);
        let log = gui.admin.log_at.unwrap();
        let copy = col(&from(&buf, log.x, log.y), "copy").expect("the copy control") + log.x;
        down(&mut gui, copy, log.y);
        assert_eq!(gui.note, Some((t!("gui.admin.log.copied_terminal").to_string(), false)));
        assert_eq!(crate::kit::clipboard::caught().len(), 1);
        crate::kit::clipboard::catch(crate::kit::clipboard::Copied::Clipboard);
    }

    #[test]
    fn d_in_the_log_says_downloading_then_the_failure() {
        let _en = english();
        let dir = std::env::temp_dir().join(format!("mstream-admin-log-d-{}", std::process::id()));
        let mut gui = session_gui();
        press(&mut gui, KeyCode::Char('M'));
        let log = std::mem::replace(&mut gui.admin.log, LogUi::new(None));
        gui.admin.log = log.utc().saving_into(dir.clone());
        render_at(&mut gui, 176, 46);
        gui.admin.focus = Focus::Log;
        press(&mut gui, KeyCode::Char('d'));
        assert_eq!(gui.note, Some((t!("gui.admin.log.fetching").to_string(), false)));
        let buf = render_at(&mut gui, 176, 46);
        let header = from(&buf, 120, 2);
        let busy = col(&header, "downloading…").expect(&header) + 120;
        assert_eq!(buf[(busy, 2)].fg, th().accent);

        let failed = t!("gui.admin.log.download_failed").to_string();
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while !gui.note.as_ref().is_some_and(|(text, _)| text.starts_with(&failed)) && Instant::now() < deadline {
            frame(&mut gui);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let (text, is_err) = gui.note.clone().expect("a note");
        assert!(text.starts_with(&failed), "{text}");
        assert!(is_err, "a failure");
        let buf = render_at(&mut gui, 176, 46);
        let y = (0..46).find(|&y| row(&buf, y).contains(&failed)).expect("the note on screen");
        let x = col(&row(&buf, y), &failed).unwrap();
        assert_eq!(buf[(x, y)].fg, th().gold, "in gold");
        assert!(row(&buf, 2).contains("· download"), "the control is back: {}", row(&buf, 2));
        assert!(!dir.exists(), "a failed download writes nothing");
    }

    #[test]
    fn the_copy_chord_acts_only_with_a_highlight_or_the_logs_focus() {
        let _en = english();
        let probe = Probe::default();
        let mut gui = hosting(&probe);
        gui.admin.log.model.take(lines(1, 3));
        render_at(&mut gui, 176, 46);
        let chord = |gui: &mut Gui| {
            let acted = copy_chord(gui);
            assert_eq!(crate::kit::clipboard::caught().len(), usize::from(acted), "a copy exactly when it acted");
            acted
        };

        assert!(!chord(&mut gui), "the hallway's keys and no highlight");
        gui.admin.focus = Focus::Log;
        assert!(chord(&mut gui), "the log has the keys");
        assert_eq!(gui.note, Some((t!("gui.admin.log.copied_all").to_string(), false)));
        gui.admin.focus = Focus::Hall;
        gui.admin.log.model.highlight = Some((1, 2));
        assert!(chord(&mut gui), "a highlight stands");
        assert_eq!(gui.note, Some((t!("gui.admin.log.copied_highlight").to_string(), false)));

        // Something over the screen holds the keys.
        super::super::servers::open_add(&mut gui);
        assert!(!chord(&mut gui), "a GUI modal");
        gui.servers.form = None;
        gui.admin.log.act(LogAct::Menu);
        assert!(!chord(&mut gui), "the level menu");
        gui.admin.log.act(LogAct::MenuClose);
        probe.0.borrow_mut().modal = true;
        assert!(!chord(&mut gui), "the room's modal");
        probe.0.borrow_mut().modal = false;
        assert!(chord(&mut gui));

        // The log not on screen, or another screen.
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 176, 46);
        assert!(!chord(&mut gui), "L hid the log");
        press(&mut gui, KeyCode::Char('L'));
        render_at(&mut gui, 176, 46);
        assert!(chord(&mut gui));
        gui.act(Act::Screen(Screen::Library));
        assert!(!chord(&mut gui), "another screen");
    }

    #[test]
    fn the_copy_chord_closes_the_server_menu_as_any_key_does() {
        let _en = english();
        let mut gui = admin_gui();
        gui.admin.log.model.take(lines(1, 3));
        render_at(&mut gui, 176, 46);

        // Nothing to copy (the hallway's keys, no highlight): the menu
        // closes all the same, and the window is told to draw it gone.
        gui.servers.drop_open = true;
        assert!(copy_chord(&mut gui));
        assert!(!gui.servers.drop_open);
        assert!(crate::kit::clipboard::caught().is_empty());

        // With the log's keys it closes and copies, as `y` does through
        // the open menu.
        gui.admin.focus = Focus::Log;
        gui.servers.drop_open = true;
        assert!(copy_chord(&mut gui));
        assert!(!gui.servers.drop_open);
        assert_eq!(crate::kit::clipboard::caught().len(), 1);
        gui.servers.drop_open = true;
        press(&mut gui, KeyCode::Char('y'));
        assert!(!gui.servers.drop_open);
        assert_eq!(crate::kit::clipboard::caught().len(), 1, "the key does the same");

        // On every screen, and nothing changes with no menu open.
        gui.act(Act::Screen(Screen::Library));
        gui.servers.drop_open = true;
        assert!(copy_chord(&mut gui));
        assert!(!gui.servers.drop_open);
        assert!(!copy_chord(&mut gui));
        assert!(crate::kit::clipboard::caught().is_empty());
    }

    #[test]
    fn the_log_label_is_the_sessions_server_not_the_tunnels_loopback() {
        let mut gui = session_gui();
        press(&mut gui, KeyCode::Char('M'));
        assert_eq!(gui.admin.log.label(), "host.invalid", "a URL's host");

        // A tunnel session is reached through a loopback bridge; its file
        // is named after the tunnel.
        let id = "mstream+iroh://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst";
        let mut tunnel = gui_with_session("http://127.0.0.1:51234", id);
        press(&mut tunnel, KeyCode::Char('M'));
        assert!(tunnel.admin.log.running(), "the tab opened on the session");
        assert_eq!(tunnel.admin.log.label(), "quickconnect-abcdefghijkl");

        // A peer session's pages reach its parent.
        let mut peer = gui_with_session("http://127.0.0.1:51234", id);
        peer.app.session.peer = Some(("https://parent.example.com:8443".into(), 7));
        assert_eq!(log_server(&peer.app), "https://parent.example.com:8443");
        assert_eq!(super::super::log_file::file_label(&log_server(&peer.app)), "parent.example.com");
    }

    /// A session on `server`, known by `id`.
    fn gui_with_session(server: &str, id: &str) -> Gui {
        let mut gui = gui();
        gui.app.connected = true;
        gui.app.session.server = server.into();
        gui.app.session.server_id = id.into();
        gui
    }
}
