//! The admin panel: `mstream-player admin [libraries|discovery|federation|backups|torrents|users]` — the
//! server's management rooms, drawn full-screen from the UI kit the way
//! the setup wizard is, against the saved session's server. And one page
//! that is not a room: `mstream-player stats`, the account's listening
//! log, which shares this hub's chrome and sign-in but needs no admin.
//!
//! Each room is a [`Screen`]. This hub owns the terminal session around
//! it — ground lease, mouse capture, pointer contract, the event loop,
//! teardown (the wizard's, minus pictures) — plus the chrome every room
//! shares (the header, the note and tips lines on the bottom edge) and
//! the gate sentences for the errors B4 warns a terminal client will hit.
//! Rooms keep their own state, worker and drawing; the loop only asks a
//! room to pump its worker, tick its timers, draw, and answer input.

mod backups;
mod torrents;
mod discovery;
mod federation;
mod libraries;
mod login;
pub(crate) mod stats;
pub(crate) mod tz;
mod users;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::style::Print;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, Paragraph};
use rust_i18n::t;

use crate::api::{ApiError, Client};
use crate::kit::theme::th;
use crate::kit::{
    GroundGuard, POINTER_RESET, Surface, accent, bold, dim, set_pointer_shape, theme, width,
};

/// How long to wait for input before redrawing anyway.
const POLL: Duration = Duration::from_millis(100);

#[derive(Args)]
pub struct AdminArgs {
    /// The server to manage — defaults to the saved session's server
    #[arg(long, global = true)]
    server: Option<String>,

    /// Auth token override (default: the saved session's token)
    #[arg(long, hide = true, global = true)]
    token: Option<String>,

    /// This terminal runs on the server's own machine: choosing a folder opens
    /// the OS folder dialog, and the paths it picks are the server's paths
    #[arg(long, global = true)]
    same_machine: bool,

    #[command(subcommand)]
    room: Option<RoomCmd>,
}

#[derive(Subcommand)]
enum RoomCmd {
    /// The server's music folders (the default room)
    Libraries,
    /// The discovery network (P2P): the mesh, the servers you follow, invites, settings
    Discovery,
    /// Federation: requests, the tickets you minted, the peers you can read
    Federation,
    /// Backups: each library's copies on other drives, their schedules and runs
    Backups,
    /// Torrents: the torrent client, its list, each library's daemon-side path, seeding, who may add
    Torrents,
    /// Users: who has an account, what each one may do, which libraries they see
    Users,
}

pub fn run(args: AdminArgs) -> i32 {
    let client = match Client::resolve(args.server.as_deref(), args.token.as_deref()) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            return 1;
        }
    };
    crate::setup::boot_language();
    let Session { client, unsaved, .. } = match ensure_session(client) {
        Ok(session) => session,
        Err(code) => return code,
    };

    let code = match args.room.unwrap_or(RoomCmd::Libraries) {
        RoomCmd::Libraries => run_tui(&mut libraries::start(client, args.same_machine)),
        RoomCmd::Discovery => run_tui(&mut discovery::start(client)),
        RoomCmd::Federation => run_tui(&mut federation::start(client)),
        RoomCmd::Backups => run_tui(&mut backups::start(client, args.same_machine)),
        RoomCmd::Torrents => run_tui(&mut torrents::start(client, args.same_machine)),
        RoomCmd::Users => run_tui(&mut users::start(client)),
    };
    if let Some(e) = unsaved {
        eprintln!("mstream-player: signed in, but the session was not saved: {e}");
    }
    code
}

#[derive(Args)]
pub struct StatsArgs {
    /// The server to read — defaults to the saved session's server
    #[arg(long)]
    server: Option<String>,

    /// Auth token override (default: the saved session's token)
    #[arg(long, hide = true)]
    token: Option<String>,
}

/// `mstream-player stats`: the account's listening log, on the hub's
/// terminal session and behind the same sign-in — any account, not an
/// admin's, since every account has a log of its own.
pub fn run_stats(args: StatsArgs) -> i32 {
    let client = match Client::resolve(args.server.as_deref(), args.token.as_deref()) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            return 1;
        }
    };
    crate::setup::boot_language();
    let Session { client, username, unsaved } = match ensure_session(client) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let code = run_tui_as(&mut stats::start(client, username), "mStream Stats");
    if let Some(e) = unsaved {
        eprintln!("mstream-player: signed in, but the session was not saved: {e}");
    }
    code
}

/// A session a page can open against: the client, who it is signed in
/// as (when the saved session or the sign-in page knows), and — signed in
/// this run but not kept — the reason to print once the page closes.
struct Session {
    client: Client,
    username: Option<String>,
    unsaved: Option<String>,
}

/// Pre-flight. Every room needs an admin session, and the rooms already
/// explain a 403 (not an admin, or an address restriction) and a 405
/// (lockAdmin) in their own words — but a 401 has an answer the hub can
/// give itself: no saved session for this server, or one it no longer
/// accepts, so ask once and keep what the server issues, the way
/// `mstream-player login` and the wizard's own account creation do.
/// Anything else the ping says (a server down; a public-mode server,
/// which needs no session at all) is the page's to show. `Err` carries
/// the exit code when the sign-in page ended the run.
fn ensure_session(client: Client) -> Result<Session, i32> {
    if !matches!(client.ping(), Err(ApiError::Unauthorized)) {
        let username = saved_username(&client.server());
        return Ok(Session { client, username, unsaved: None });
    }
    let mut page = login::start(client);
    let code = run_tui(&mut page);
    if code != 0 {
        return Err(code);
    }
    let Some((username, token)) = page.session() else {
        return Err(0); // Esc: nothing to open
    };
    let server = page.server();
    let client = match Client::new(&server) {
        Ok(fresh) => fresh.with_token(Some(token.clone())),
        Err(e) => {
            eprintln!("mstream-player: {e}");
            return Err(1);
        }
    };
    // Signed in for this run regardless; the terminal is the page's
    // until it closes, so the warning waits.
    let unsaved = login::remember(&server, &username, &token).err();
    Ok(Session { client, username: Some(username), unsaved })
}

/// Who the saved session signs in as on `server`, if the config knows.
fn saved_username(server: &str) -> Option<String> {
    let config = crate::config::load().ok()?;
    config
        .servers
        .iter()
        .find(|entry| crate::config::same_server(&entry.url, server))
        .and_then(|entry| entry.username.clone())
}

/// How a room's loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Quit,
}

/// What a hosted room holds of the keyboard right now (admin-screen
/// contract, clause 13): the host routes a key to the room or keeps it by
/// asking this first, so a modal or a text field never loses a letter to
/// the host's own keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Claim {
    /// The room takes its own keys; the host keeps Tab and its own letters.
    #[default]
    Open,
    /// Tab moves the room's own focus, so the host leaves Tab alone too.
    OwnTab,
    /// A modal or a text field is up: every key is the room's.
    All,
}

/// What the terminal session asks of a room. A room keeps its own worker
/// channels, state and drawing; the loop calls these in a fixed order —
/// tick, draw, pump, then input — so a busy note queued by a key is on
/// screen the frame before its call blocks the worker.
pub(crate) trait Screen {
    type Act: Clone;

    /// The kit's interaction surface: the loop drives hover, presses,
    /// drags, hold-repeat and tooltip dwell through it.
    fn ui(&mut self) -> &mut Surface<Self::Act>;

    /// Fold in whatever the worker finished, then hand it the next queued
    /// op. Called once per frame, right after the draw.
    fn pump(&mut self);

    /// Once per loop turn, before the draw: a room's own timers (a poll
    /// cadence, say). Nothing by default.
    fn tick(&mut self) {}

    /// Asked right after the pump: a screen whose job is done (the sign-in
    /// page, once the server answered) ends the loop from here — the rooms
    /// only ever end on a key or a click, and never override it.
    fn finished(&self) -> Option<Outcome> {
        None
    }

    fn render(&mut self, frame: &mut Frame);

    /// A key press (Ctrl-C is the loop's own).
    fn key(&mut self, key: KeyEvent) -> Option<Outcome>;

    /// A click, drag or hold resolved to one of the room's actions.
    fn act(&mut self, act: Self::Act) -> Option<Outcome>;

    /// The wheel over `at`: which list scrolls is the room's to say.
    fn wheel(&mut self, up: bool, at: Position);

    /// Draw inside another shell's `area` (the GUI player's Admin tab):
    /// no header, since the host's bar names the server, and no tips row,
    /// since the host's footer carries [`Screen::hint`], on the ground the
    /// host painted. A page that is never hosted draws nothing here.
    fn render_hosted(&mut self, _frame: &mut Frame, _area: Rect) {}

    /// The keyboard tips a host's footer shows while this page is hosted,
    /// in the words the page's own tips row would use.
    fn hint(&self) -> String {
        String::new()
    }

    /// Whether one of the page's own modals is up.
    fn modal_open(&self) -> bool {
        false
    }

    /// How much of the keyboard the page holds right now: every key while
    /// a modal is up. A page with a text field or a Tab of its own says
    /// more.
    fn claim(&self) -> Claim {
        if self.modal_open() { Claim::All } else { Claim::Open }
    }
}

/// The terminal session around a room: ground lease, mouse capture,
/// pointer contract, event loop, teardown.
fn run_tui<S: Screen>(screen: &mut S) -> i32 {
    run_tui_as(screen, "mStream Admin")
}

/// [`run_tui`] under a window title of the page's own. Crate-visible for
/// the pages that borrow this session without being rooms: the stats page
/// here, the MP3 player's flash page (src/device/page.rs).
pub(crate) fn run_tui_as<S: Screen>(screen: &mut S, title: &str) -> i32 {
    let _title = crate::tui::WindowTitle::claim(title);

    // Claim the window background BEFORE ratatui takes the terminal — the
    // OSC 11 query runs its own raw-mode transaction on the tty.
    let claim = theme::acquire_ground();
    let ground_guard = GroundGuard;

    // The room's palette is its interface: not subject to NO_COLOR.
    crate::console::keep_colors();
    // Frames written whole, like the player's (`kit::frames`).
    let mut terminal = crate::kit::frames::init();
    let mouse_on = execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    if let Some(seq) = claim {
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(seq));
    }
    set_pointer_shape(false, mouse_on);
    let outcome = event_loop(&mut terminal, screen, mouse_on);
    if mouse_on {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(POINTER_RESET));
    }
    ratatui::restore();
    drop(ground_guard);

    match outcome {
        Ok(Outcome::Quit) => 0,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            1
        }
    }
}

fn event_loop<S: Screen>(
    terminal: &mut crate::kit::frames::PageTerminal,
    screen: &mut S,
    mouse_on: bool,
) -> std::io::Result<Outcome> {
    let mut hand = false;
    loop {
        screen.tick();
        terminal.draw(|frame| screen.render(frame))?;
        screen.pump();
        if let Some(outcome) = screen.finished() {
            return Ok(outcome);
        }

        let over = screen.ui().hovering_clickable();
        if over != hand {
            hand = over;
            set_pointer_shape(hand, mouse_on);
        }
        let held = screen.ui().hold_action();
        if let Some(act) = held {
            screen.act(act);
        }
        screen.ui().dwell_tick();

        if !event::poll(POLL)? {
            continue;
        }
        // Drain everything queued before the next draw: a sweep of the
        // pointer is one event per cell crossed.
        let mut inputs = vec![event::read()?];
        while event::poll(Duration::ZERO)? {
            inputs.push(event::read()?);
        }
        for input in inputs {
            match input {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    screen.ui().dismiss_tooltip();
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        return Ok(Outcome::Quit);
                    }
                    if let Some(outcome) = screen.key(key) {
                        return Ok(outcome);
                    }
                }
                TermEvent::Mouse(mouse) => {
                    if let Some(outcome) = drive_pointer(screen, mouse) {
                        return Ok(outcome);
                    }
                }
                _ => {}
            }
        }
    }
}

/// One mouse event, the way the hub's loop takes it. A left press is
/// first asked whether it is real (Apple Terminal's phantom re-click is
/// swallowed whole), then dispatched to what it hit, then allowed to arm
/// a scrollbar; motion moves the hover; a drag moves the hover and the
/// thumb it holds; a release ends the capture; the wheel sets the pointer
/// before the page decides which list scrolls. The GUI player's hosted
/// pages ride the same routine, so there is one copy of it. `Some` when an
/// action ended the page.
pub(crate) fn drive_pointer<S: Screen>(screen: &mut S, mouse: MouseEvent) -> Option<Outcome> {
    let at = Position { x: mouse.column, y: mouse.row };
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !screen.ui().begin_press(at) {
                return None;
            }
            let hit = screen.ui().hit(at);
            if let Some(act) = hit
                && let Some(outcome) = screen.act(act)
            {
                return Some(outcome);
            }
            screen.ui().arm_bars(at);
        }
        MouseEventKind::Moved => screen.ui().motion(at),
        MouseEventKind::Drag(_) => {
            screen.ui().motion(at);
            let dragged = screen.ui().drag_action(at);
            if let Some(act) = dragged {
                screen.act(act);
            }
        }
        MouseEventKind::Up(_) => screen.ui().release(),
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            screen.ui().pointer = Some(at);
            screen.wheel(mouse.kind == MouseEventKind::ScrollUp, at);
        }
        _ => {}
    }
    None
}

// ── Hosting a room in another shell ──────────────────────────────────────────

/// The rooms a host can open, in the hallway's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoomId {
    Libraries,
    Users,
    Backups,
    Discovery,
    Federation,
    Torrents,
}

impl RoomId {
    pub(crate) const ALL: [RoomId; 6] = [
        RoomId::Libraries,
        RoomId::Users,
        RoomId::Backups,
        RoomId::Discovery,
        RoomId::Federation,
        RoomId::Torrents,
    ];
}

/// A room, loading, for a host to keep: the start the `admin` subcommand
/// gives it, behind the face a host can hold without naming the room's
/// own types. `same_machine` matters only to the rooms that pick server
/// paths (libraries, backups, torrents).
pub(crate) fn open_room(id: RoomId, client: Client, same_machine: bool) -> Box<dyn HostedRoom> {
    match id {
        RoomId::Libraries => Box::new(libraries::start(client, same_machine)),
        RoomId::Users => Box::new(users::start(client)),
        RoomId::Backups => Box::new(backups::start(client, same_machine)),
        RoomId::Discovery => Box::new(discovery::start(client)),
        RoomId::Federation => Box::new(federation::start(client)),
        RoomId::Torrents => Box::new(torrents::start(client, same_machine)),
    }
}

/// What a host holds of a room: [`Screen`] made object-safe (there is no
/// `Act` type to name), with the hub loop's steps folded into the calls a
/// host's own loop makes. The names differ from [`Screen`]'s, so no call
/// through either trait is ambiguous.
pub(crate) trait HostedRoom {
    /// Draw the room inside `area` ([`Screen::render_hosted`]).
    fn draw_in(&mut self, frame: &mut Frame, area: Rect);

    /// A key the host routed to the room. It dismisses the tooltip first,
    /// as a key does in the hub.
    fn press(&mut self, key: KeyEvent) -> Option<Outcome>;

    /// A mouse event over the room ([`drive_pointer`]).
    fn mouse(&mut self, mouse: MouseEvent) -> Option<Outcome>;

    /// The pointer left the room (focus moved away, the room was hidden,
    /// or it closed), so no hover, no tooltip and no press stays behind: a
    /// scrollbar arrow held as the room went from view would otherwise go
    /// on stepping, and a thumb go on following, with no release to come.
    fn leave(&mut self);

    /// The host's frame is on screen: fold in what the worker finished and
    /// hand it the next op, run the room's timers, step a held scrollbar
    /// arrow, age the tooltip dwell. Pump comes before tick so that an op
    /// a tick queues is drawn (its busy line) the frame before it is
    /// pumped, which is the hub's own tick, draw, pump order seen from
    /// after the host's draw. Returns whether the pointer rests on
    /// something clickable, for the hand cursor.
    fn after_frame(&mut self) -> bool;

    /// The keyboard tips for the host's footer ([`Screen::hint`]).
    fn tips(&self) -> String;

    /// How much of the keyboard the room holds ([`Screen::claim`]).
    fn claims(&self) -> Claim;

    /// Whether one of the room's modals is up ([`Screen::modal_open`]).
    fn modal_up(&self) -> bool;

    /// Whether the room's tooltips name their keys: the host's setting.
    fn set_key_hints(&mut self, on: bool);

    /// The cell where the room's focused field drew its caret in the last
    /// [`HostedRoom::draw_in`], if a field has the room's keyboard
    /// ([`Surface::caret_at`], noted by [`crate::kit::field_display`]).
    /// The host lifts it onto its own surface, so a shell with a paste and
    /// an input method of its own (the GUI's window) turns them on for the
    /// room's field. Taken `&mut` because the room's surface is reached
    /// through [`Screen::ui`].
    fn caret_at(&mut self) -> Option<Position>;

    /// The host's input method's uncommitted text (empty for none), for
    /// the room's focused field to draw in place: handed down before every
    /// [`HostedRoom::draw_in`], so the room's copy follows the host's from
    /// frame to frame ([`Surface::set_composition`]).
    fn set_composition(&mut self, text: &str);
}

impl<S: Screen + 'static> HostedRoom for S {
    fn draw_in(&mut self, frame: &mut Frame, area: Rect) {
        <S as Screen>::render_hosted(self, frame, area);
    }

    fn press(&mut self, key: KeyEvent) -> Option<Outcome> {
        self.ui().dismiss_tooltip();
        <S as Screen>::key(self, key)
    }

    fn mouse(&mut self, mouse: MouseEvent) -> Option<Outcome> {
        drive_pointer(self, mouse)
    }

    fn leave(&mut self) {
        let ui = self.ui();
        ui.pointer = None;
        ui.dismiss_tooltip();
        ui.release();
    }

    fn after_frame(&mut self) -> bool {
        <S as Screen>::pump(self);
        <S as Screen>::tick(self);
        let held = self.ui().hold_action();
        if let Some(act) = held {
            <S as Screen>::act(self, act);
        }
        self.ui().dwell_tick();
        self.ui().hovering_clickable()
    }

    fn tips(&self) -> String {
        <S as Screen>::hint(self)
    }

    fn claims(&self) -> Claim {
        <S as Screen>::claim(self)
    }

    fn modal_up(&self) -> bool {
        <S as Screen>::modal_open(self)
    }

    fn set_key_hints(&mut self, on: bool) {
        self.ui().key_hints = on;
    }

    fn caret_at(&mut self) -> Option<Position> {
        self.ui().caret_at()
    }

    fn set_composition(&mut self, text: &str) {
        self.ui().set_composition(text);
    }
}

/// A room test's check of its focused field, which is the GUI window's
/// field when the room is hosted (admin-screen contract, clause 28): the
/// cell the room's surface `ui` noted holds the caret in `frame`, a test's
/// flattened draw (a character a cell, a line a row), with `before` the
/// end of what is drawn left of it and `after` the start of what is drawn
/// right of it, wherever the field's window over a long value stands. The
/// frame comes first so a test can draw it in the call.
#[cfg(test)]
pub(crate) fn assert_caret_between<A: Clone>(frame: &str, ui: &Surface<A>, before: &str, after: &str) {
    let at = ui.caret_at().expect("a field has the keyboard and noted its caret");
    let row: Vec<char> = frame.lines().nth(usize::from(at.y)).expect("the caret's row").chars().collect();
    let line: String = row.iter().collect();
    let x = usize::from(at.x);
    assert_eq!(row.get(x), Some(&'▏'), "the noted cell {at:?} holds no caret: {line}");
    let left: String = row[..x].iter().collect();
    let right: String = row[x + 1..].iter().collect();
    assert!(left.ends_with(before), "{before:?} is not left of the caret: {line}");
    assert!(right.starts_with(after), "{after:?} is not right of the caret: {line}");
}

// ── Shared chrome ────────────────────────────────────────────────────────────

/// Start a frame: paint the fixed scheme's ground — only when the terminal
/// granted OSC 11 ownership of the whole window (see the wizard) — and
/// refuse to draw a room into a window smaller than it can hold, saying
/// so. Returns the area to draw into.
pub(crate) fn frame_ground(frame: &mut Frame, min_width: u16, min_height: u16) -> Option<Rect> {
    let area = frame.area();
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            Block::default().style(Style::default().bg(ground).fg(th().text)),
            area,
        );
    }
    if area.width < min_width || area.height < min_height {
        frame.render_widget(Paragraph::new(t!("resize").to_string()).style(dim()), area);
        return None;
    }
    Some(area)
}

/// The room's heading on the top row, and the server it manages — its
/// host and the admin role — at the right edge.
pub(crate) fn draw_header(frame: &mut Frame, area: Rect, title: &str, host: &str) {
    draw_header_as(frame, area, title, &format!("{host} · {}", t!("admin.role")));
}

/// [`draw_header`] with the right edge spelled by the caller — the stats
/// page puts the account there, not a role. The header is the area's
/// first row, two cells in from either side.
pub(crate) fn draw_header_as(frame: &mut Frame, area: Rect, title: &str, right: &str) {
    let head = Rect { x: area.x + 2, y: area.y, width: area.width.saturating_sub(4), height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(title.to_string(), bold())), head);
    frame.render_widget(Paragraph::new(Span::styled(right.to_string(), dim())).alignment(Alignment::Right), head);
}

/// The bottom edge: one status line (an error in gold, a note in dim, a
/// busy line in the accent — busy wins) above the keyboard tips.
pub(crate) fn draw_bottom(
    frame: &mut Frame,
    area: Rect,
    note: Option<&(String, bool)>,
    busy: Option<&str>,
    tips: &str,
) {
    let width = area.width.saturating_sub(4);
    let line = Rect { x: area.x + 2, y: area.y + area.height.saturating_sub(2), width, height: 1 };
    if let Some((text, is_err)) = note {
        let style = if *is_err { Style::default().fg(th().gold) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(text.clone(), style)), line);
    }
    if let Some(busy) = busy {
        frame.render_widget(Paragraph::new(Span::styled(busy.to_string(), accent())), line);
    }
    let tips_rect = Rect { x: area.x + 2, y: area.y + area.height.saturating_sub(1), width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(tips.to_string(), dim())), tips_rect);
}

/// A room's body column, two cells in from either side. Standalone, it
/// starts under the header and a blank row and stops above a blank row
/// and the two bottom lines. Hosted, the host's bar is the header, so it
/// starts one row into the area and stops above a blank row and the
/// note, the one bottom line a hosted room keeps.
pub(crate) fn body_column(area: Rect, hosted: bool) -> Rect {
    let (top, spent) = if hosted { (1, 3) } else { (2, 5) };
    Rect {
        x: area.x + 2,
        y: area.y + top,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(spent),
    }
}

/// A room's bottom edge, standalone or hosted. Standalone it is
/// [`draw_bottom`]. Hosted, the tips are the host's footer's to show, so
/// the area's last row carries the status line alone: the busy line in
/// the accent when there is one, else the note (an error in gold, a note
/// in dim).
pub(crate) fn draw_foot(
    frame: &mut Frame,
    area: Rect,
    note: Option<&(String, bool)>,
    busy: Option<&str>,
    tips: &str,
    hosted: bool,
) {
    if !hosted {
        draw_bottom(frame, area, note, busy, tips);
        return;
    }
    let line = Rect {
        x: area.x + 2,
        y: area.y + area.height.saturating_sub(1),
        width: area.width.saturating_sub(4),
        height: 1,
    };
    let span = match (busy, note) {
        (Some(busy), _) => Span::styled(busy.to_string(), accent()),
        (None, Some((text, is_err))) => {
            let style = if *is_err { Style::default().fg(th().gold) } else { dim() };
            Span::styled(text.clone(), style)
        }
        (None, None) => return,
    };
    frame.render_widget(Paragraph::new(span), line);
}

/// A hosted room's chip (the beta mark), in gold and flush with the right
/// edge of the column's first row: the header it sits beside when the
/// room stands alone is the host's.
pub(crate) fn draw_chip(frame: &mut Frame, column: Rect, text: &str) {
    let cells = (width(text) as u16).min(column.width);
    let rect = Rect { x: column.right() - cells, y: column.y, width: cells, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(text.to_string(), Style::default().fg(th().gold))), rect);
}

/// The server's host, for a header — never the whole URL.
pub(crate) fn host_of(client: &Client) -> String {
    let server = client.server();
    url::Url::parse(&server)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or(server)
}

/// What a failed load means, said plainly: the two gates B4 warns a
/// terminal client will hit (a non-admin account or an address restriction
/// → 403, `lockAdmin` → 405), the plain sign-in miss, and otherwise
/// `what` with the server's words.
pub(crate) fn gate_message(e: &ApiError, what: &str) -> String {
    match e {
        ApiError::Unauthorized => t!("admin.gate_unauthorized").to_string(),
        ApiError::Forbidden(err) => t!("admin.gate_forbidden", err = err).to_string(),
        ApiError::Server { status: 405, .. } => t!("admin.gate_locked").to_string(),
        other => format!("{what}: {other}"),
    }
}

// ── Text, time and clipboard helpers shared by the rooms ──────────────────

/// The first twelve hex digits and an ellipsis — the webapp's fingerprint.
pub(crate) fn short_id(id: &str) -> String {
    let head: String = id.chars().filter(char::is_ascii_hexdigit).take(12).collect();
    if head.is_empty() { "…".to_string() } else { format!("{head}…") }
}

/// `raw` minus every character that acts on the terminal or reorders a
/// reader instead of showing itself, trimmed, cut to `cap` characters.
pub(crate) fn printable(raw: &str, cap: usize) -> String {
    raw.chars()
        .filter(|c| !(c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')))
        .collect::<String>()
        .trim()
        .chars()
        .take(cap)
        .collect()
}

pub(crate) fn fmt_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The webapp's discoveryBytes: GB past a gigabyte, MB past a megabyte,
/// KB below.
pub(crate) fn fmt_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let n = n as f64;
    if n >= KB * KB * KB {
        format!("{:.1} GB", n / (KB * KB * KB))
    } else if n >= KB * KB {
        format!("{:.1} MB", n / (KB * KB))
    } else {
        format!("{:.0} KB", (n / KB).ceil())
    }
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` (what the server's JSON emits) or SQLite's
/// `YYYY-MM-DD HH:MM:SS` (UTC, what its rows carry) → Unix seconds.
/// Anything else, or an offset other than Z, is `None`.
pub(crate) fn iso_unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |from: usize, to: usize| s.get(from..to)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant), proleptic Gregorian.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The inverse of [`iso_unix`]: Unix seconds → `YYYY-MM-DDTHH:MM:SS.000Z`,
/// for the cutoffs a form turns into ISO for the server.
pub(crate) fn iso_at(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    // Civil from days (Howard Hinnant), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A duration as the webapp's table suffix: minutes under an hour, hours
/// under two days, days after.
pub(crate) fn age_text(secs: i64) -> String {
    let mins = secs.max(0) / 60;
    if mins < 1 {
        t!("p2p.age_now").to_string()
    } else if mins < 60 {
        t!("p2p.age_m", n = mins).to_string()
    } else if mins < 48 * 60 {
        t!("p2p.age_h", n = mins / 60).to_string()
    } else {
        t!("p2p.age_d", n = mins / (24 * 60)).to_string()
    }
}

/// OSC 52: hand the text to the terminal's clipboard, where the terminal
/// allows it (kitty, iTerm2 with the setting on, xterm, foot, Windows
/// Terminal; Apple Terminal ignores it). Best effort — the note says so.
pub(crate) fn copy_to_clipboard(text: &str) -> bool {
    use base64::Engine;
    let payload = base64::engine::general_purpose::STANDARD.encode(text);
    execute!(std::io::stdout(), Print(format!("\x1b]52;c;{payload}\x1b\\"))).is_ok()
}

/// The server user's home — the one the admin file explorer spells `~` and
/// resolves. Nothing else on the server does: the add-directory route
/// answers a tilde with a 500 and the backup routes with "must be
/// absolute", and the explorer itself only knows the bare form. A room's
/// worker learns the home once and expands a typed `~` before any route
/// sees the path, so the modal keeps what the user typed.
pub(crate) struct ServerHome(Option<String>);

impl ServerHome {
    pub(crate) fn new() -> Self {
        ServerHome(None)
    }

    /// `path` with a leading `~` — alone, or before a separator — replaced
    /// by the server's home. Anything else comes back untouched, without a
    /// request. The one request that learns the home fails like any other
    /// call, so the caller's own sentence names what could not be done.
    pub(crate) fn expand(&mut self, client: &Client, path: &str) -> Result<String, ApiError> {
        if !home_ref(path) {
            return Ok(path.to_string());
        }
        let home = match &self.0 {
            Some(home) => home.clone(),
            None => {
                let listed = client.admin_file_explorer("~")?.path;
                if listed.is_empty() {
                    return Err(ApiError::Decode {
                        endpoint: "api/v1/admin/file-explorer".into(),
                        message: "the listing of ~ names no path".into(),
                    });
                }
                self.0 = Some(listed.clone());
                listed
            }
        };
        Ok(join_home(&home, path))
    }
}

/// A shell's home reference: `~` alone, or `~` before a separator. `~user`
/// is not one — here it is a folder called `~user`.
pub(crate) fn home_ref(path: &str) -> bool {
    path == "~" || path.starts_with("~/") || path.starts_with("~\\")
}

/// A home reference (see [`home_ref`]) rooted at `home`, in the home's own
/// separator — a Windows server's `~/Music` becomes `C:\Users\me\Music`.
pub(crate) fn join_home(home: &str, path: &str) -> String {
    let rest = &path[1..];
    let trimmed = home.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        // A root as the home ("/"): it already ends in its separator.
        return format!("{home}{}", rest.trim_start_matches(['/', '\\']));
    }
    if trimmed.contains('\\') {
        format!("{trimmed}{}", rest.replace('/', "\\"))
    } else {
        format!("{trimmed}{rest}")
    }
}

/// What the room lanes' tests share: the areas the GUI player's Admin tab
/// hands a room, and a frame drawn around one so a test can see whether
/// anything landed outside it.
#[cfg(test)]
pub(crate) mod hosting {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Position, Rect};

    use super::Screen;

    /// The room's area in a 100×30 window with key hints on: right of the
    /// hallway and its rule, under the top bar, above the footer and the
    /// player bar.
    pub(crate) const WINDOW: Rect = Rect { x: 17, y: 1, width: 83, height: 23 };
    /// The same at the GUI's floor, 100×24: seventeen rows for the room.
    pub(crate) const FLOOR: Rect = Rect { x: 17, y: 1, width: 83, height: 17 };
    /// The room beside the docked log column in a 176×46 window.
    pub(crate) const DOCKED: Rect = Rect { x: 17, y: 1, width: 100, height: 39 };

    /// What every cell holds before the room draws.
    const UNTOUCHED: &str = "·";

    /// `screen` drawn hosted in `area` on a `size` buffer that was filled
    /// with `·` in the same draw, so every cell the room wrote shows.
    pub(crate) fn draw_hosted<S: Screen>(screen: &mut S, size: (u16, u16), area: Rect) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("test backend");
        terminal
            .draw(|frame| {
                let all = frame.area();
                for y in all.top()..all.bottom() {
                    for x in all.left()..all.right() {
                        frame.buffer_mut()[(x, y)].set_symbol(UNTOUCHED);
                    }
                }
                screen.render_hosted(frame, area);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    /// The cells left of, above or right of `area` that the room wrote;
    /// with `below`, the rows under it as well.
    pub(crate) fn outside(buf: &Buffer, area: Rect, below: bool) -> Vec<(u16, u16)> {
        let mut hits = Vec::new();
        for y in buf.area.top()..buf.area.bottom() {
            if y >= area.bottom() && !below {
                continue;
            }
            for x in buf.area.left()..buf.area.right() {
                if !area.contains(Position { x, y }) && buf[(x, y)].symbol() != UNTOUCHED {
                    hits.push((x, y));
                }
            }
        }
        hits
    }

    /// Row `y` of the buffer as text.
    pub(crate) fn row(buf: &Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right()).map(|x| buf[(x, y)].symbol()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyEventKind;

    use super::hosting::{DOCKED, FLOOR, WINDOW, draw_hosted, outside, row};

    const CLICK: u16 = 50;
    const QUIT: u16 = 999;
    const BACK: u16 = 1000;
    const FWD: u16 = 1001;
    /// The probe's scrollbar: twenty rows in a six-row view.
    const BAR: Rect = Rect { x: 10, y: 5, width: 1, height: 6 };

    /// A screen that only writes down what the hub asked of it.
    #[derive(Default)]
    struct Probe {
        ui: Surface<u16>,
        log: Vec<&'static str>,
        acts: Vec<u16>,
        /// Each wheel turn, and where the pointer was when it arrived.
        wheels: Vec<(bool, Option<Position>)>,
        /// Each key, and whether a tooltip was ripe when it arrived.
        keys: Vec<(KeyCode, bool)>,
        modal: bool,
        note: Option<(String, bool)>,
        busy: Option<String>,
    }

    impl Screen for Probe {
        type Act = u16;

        fn ui(&mut self) -> &mut Surface<u16> {
            &mut self.ui
        }

        fn pump(&mut self) {
            self.log.push("pump");
        }

        fn tick(&mut self) {
            self.log.push("tick");
        }

        /// A click row, a quit row and a tip on the first, a scrollbar.
        fn render(&mut self, frame: &mut Frame) {
            self.ui.begin_frame();
            self.ui.click(Rect { x: 0, y: 0, width: 5, height: 1 }, CLICK);
            self.ui.tip(Rect { x: 0, y: 0, width: 5, height: 1 }, "a tip");
            self.ui.click(Rect { x: 0, y: 1, width: 5, height: 1 }, QUIT);
            crate::kit::scroll_list(frame, &mut self.ui, BAR, 20, 6, 0, BACK, FWD, |p| p as u16);
        }

        fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
            let ripe = self.ui.ripe_tooltip().is_some();
            self.keys.push((key.code, ripe));
            None
        }

        fn act(&mut self, act: u16) -> Option<Outcome> {
            self.acts.push(act);
            (act == QUIT).then_some(Outcome::Quit)
        }

        fn wheel(&mut self, up: bool, _at: Position) {
            self.wheels.push((up, self.ui.pointer));
        }

        /// The hosted chrome alone: the beta chip and the foot.
        fn render_hosted(&mut self, frame: &mut Frame, area: Rect) {
            let column = body_column(area, true);
            draw_chip(frame, column, "beta");
            draw_foot(frame, area, self.note.as_ref(), self.busy.as_deref(), "x quit", true);
        }

        fn modal_open(&self) -> bool {
            self.modal
        }
    }

    /// The probe drawn standalone, so its rects and its bar are registered.
    fn drawn() -> Probe {
        let mut probe = Probe::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|frame| probe.render(frame)).unwrap();
        probe
    }

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }
    }

    fn draw_with(size: (u16, u16), paint: impl FnOnce(&mut Frame)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(paint).unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (buf.area.top()..buf.area.bottom()).map(|y| row(buf, y)).collect()
    }

    #[test]
    fn outcome_compares() {
        assert_eq!(Some(Outcome::Quit), Some(Outcome::Quit));
        assert_ne!(Some(Outcome::Quit), None);
    }

    #[test]
    fn the_body_column_is_todays_standalone_and_one_row_under_the_hosts_bar_hosted() {
        let window = Rect { x: 0, y: 0, width: 100, height: 30 };
        assert_eq!(body_column(window, false), Rect { x: 2, y: 2, width: 96, height: 25 }, "the column every room hard-codes");
        assert_eq!(body_column(WINDOW, true), Rect { x: 19, y: 2, width: 79, height: 20 });
        let tiny = Rect { x: 5, y: 5, width: 3, height: 2 };
        assert_eq!(body_column(tiny, false).height, 0, "no room left is an empty column, not a panic");
        assert_eq!(body_column(tiny, true).width, 0);
    }

    #[test]
    fn the_bottom_edge_and_the_header_follow_their_area() {
        let chrome = |frame: &mut Frame, area: Rect| {
            draw_header_as(frame, area, "Title", "right");
            draw_bottom(frame, area, Some(&("a note".to_string(), false)), None, "x quit");
        };
        let origin = draw_with((40, 10), |frame| chrome(frame, frame.area()));
        let rows_at_origin = rows(&origin);
        assert_eq!(rows_at_origin[0], format!("  Title{}right  ", " ".repeat(26)), "the header where it always was");
        assert_eq!(rows_at_origin[8], format!("  a note{}", " ".repeat(32)));
        assert_eq!(rows_at_origin[9], format!("  x quit{}", " ".repeat(32)));
        assert!(rows_at_origin[1..8].iter().all(|r| r.trim().is_empty()));

        let area = Rect { x: 10, y: 3, width: 40, height: 10 };
        let offset = draw_with((60, 16), |frame| chrome(frame, area));
        let shifted = rows(&offset);
        for (y, line) in rows_at_origin.iter().enumerate() {
            let got: String = shifted[3 + y].chars().skip(10).take(40).collect();
            assert_eq!(&got, line, "row {y} of the area");
        }
        let blank = |r: &String| r.trim().is_empty();
        assert!(shifted[..3].iter().all(blank) && shifted[13..].iter().all(blank), "nothing above or below the area");
        assert!(shifted.iter().all(|r| r.chars().take(10).all(|c| c == ' ') && r.chars().skip(50).all(|c| c == ' ')));
    }

    #[test]
    fn hosted_the_foot_is_the_note_alone_on_the_areas_last_row_and_busy_wins() {
        let mut probe = Probe { note: Some(("Saved.".to_string(), false)), ..Probe::default() };
        let buf = draw_hosted(&mut probe, (100, 30), WINDOW);
        assert!(row(&buf, 23).starts_with(&format!("{}Saved.·", "·".repeat(19))), "{}", row(&buf, 23));
        assert!(!rows(&buf).iter().any(|r| r.contains("x quit")), "the tips are the host's footer's");
        assert!(outside(&buf, WINDOW, true).is_empty());

        probe.busy = Some("Saving…".to_string());
        let buf = draw_hosted(&mut probe, (100, 30), WINDOW);
        assert!(row(&buf, 23).starts_with(&format!("{}Saving…·", "·".repeat(19))), "{}", row(&buf, 23));
        assert!(!rows(&buf).iter().any(|r| r.contains("Saved")), "busy wins the line outright");

        probe.busy = None;
        probe.note = None;
        let buf = draw_hosted(&mut probe, (100, 30), WINDOW);
        assert_eq!(row(&buf, 23), "·".repeat(100), "nothing to say leaves the row alone");

        let standalone = draw_with((40, 10), |frame| {
            draw_foot(frame, frame.area(), Some(&("a note".to_string(), true)), Some("busy"), "x quit", false)
        });
        let bottom = draw_with((40, 10), |frame| {
            draw_bottom(frame, frame.area(), Some(&("a note".to_string(), true)), Some("busy"), "x quit")
        });
        assert_eq!(standalone, bottom, "standalone, the foot is the bottom edge");
    }

    #[test]
    fn the_chip_sits_right_on_the_columns_first_row() {
        let mut probe = Probe::default();
        let buf = draw_hosted(&mut probe, (100, 30), WINDOW);
        // The column runs x 19..98: the chip's last cell is x 97.
        assert_eq!(row(&buf, 2), format!("{}beta{}", "·".repeat(94), "··"));
        assert_eq!(buf[(94, 2)].fg, crate::kit::theme::th().gold);
        assert!(outside(&buf, WINDOW, true).is_empty());
    }

    #[test]
    fn drive_pointer_takes_the_hubs_steps() {
        let mut probe = drawn();
        assert_eq!(drive_pointer(&mut probe, mouse(MouseEventKind::Down(MouseButton::Left), 2, 0)), None);
        assert_eq!(probe.acts, vec![CLICK], "a press dispatches what it hit");
        assert_eq!(probe.ui.pointer, Some(Position { x: 2, y: 0 }));

        drive_pointer(&mut probe, mouse(MouseEventKind::Moved, 3, 3));
        assert_eq!(probe.ui.pointer, Some(Position { x: 3, y: 3 }), "motion moves the hover");

        // A press on the track jumps and arms a thumb drag; the drag
        // follows the hand; the release ends it.
        drive_pointer(&mut probe, mouse(MouseEventKind::Down(MouseButton::Left), 10, 7));
        let jump = |y| crate::kit::bar_jump(BAR, 14, y) as u16;
        assert_eq!(probe.acts, vec![CLICK, jump(7)]);
        drive_pointer(&mut probe, mouse(MouseEventKind::Drag(MouseButton::Left), 10, 9));
        assert_eq!(probe.acts, vec![CLICK, jump(7), jump(9)], "a drag moves the thumb");
        drive_pointer(&mut probe, mouse(MouseEventKind::Up(MouseButton::Left), 10, 9));
        drive_pointer(&mut probe, mouse(MouseEventKind::Drag(MouseButton::Left), 10, 8));
        assert_eq!(probe.acts.len(), 3, "after the release a drag holds nothing");

        // That release came on the heels of the press: Apple Terminal's
        // instant click, so its re-click beside the bar is swallowed.
        assert_eq!(drive_pointer(&mut probe, mouse(MouseEventKind::Down(MouseButton::Left), 9, 8)), None);
        assert_eq!(probe.acts.len(), 3, "the phantom re-click is swallowed whole");

        drive_pointer(&mut probe, mouse(MouseEventKind::ScrollDown, 30, 9));
        assert_eq!(probe.wheels, vec![(false, Some(Position { x: 30, y: 9 }))], "the pointer is set before the wheel");

        assert_eq!(
            drive_pointer(&mut probe, mouse(MouseEventKind::Down(MouseButton::Left), 1, 1)),
            Some(Outcome::Quit),
            "an act's outcome comes back"
        );
    }

    #[test]
    fn the_host_face_dismisses_the_tooltip_before_a_key_and_leave_clears_hover() {
        let ripe = |probe: &mut Probe| {
            probe.ui.pointer = Some(Position { x: 1, y: 0 });
            probe.ui.dwell_tick();
            probe.ui.dwell_backdate(crate::kit::TIP_DELAY);
            assert!(probe.ui.ripe_tooltip().is_some());
        };
        let mut probe = drawn();
        ripe(&mut probe);
        let key = KeyEvent::new_with_kind(KeyCode::Char('x'), KeyModifiers::NONE, KeyEventKind::Press);
        assert_eq!(HostedRoom::press(&mut probe, key), None);
        assert_eq!(probe.keys, vec![(KeyCode::Char('x'), false)], "the tooltip was gone before the key arrived");

        ripe(&mut probe);
        assert_eq!(HostedRoom::mouse(&mut probe, mouse(MouseEventKind::Moved, 2, 0)), None);
        assert!(probe.ui.hovering_clickable(), "the mouse reaches the room through the host face");
        HostedRoom::leave(&mut probe);
        assert_eq!(probe.ui.pointer, None);
        assert!(probe.ui.ripe_tooltip().is_none());
        assert!(!probe.ui.hovering_clickable());
    }

    #[test]
    fn leave_lets_go_of_a_held_arrow_and_a_dragged_thumb() {
        // The host hides a room mid-press (the GUI's `L` while an arrow is
        // held): no release will reach it, so leaving ends the capture.
        let mut probe = drawn();
        drive_pointer(&mut probe, mouse(MouseEventKind::Down(MouseButton::Left), 10, 7));
        let acts = probe.acts.len();
        HostedRoom::leave(&mut probe);
        drive_pointer(&mut probe, mouse(MouseEventKind::Drag(MouseButton::Left), 10, 9));
        assert_eq!(probe.acts.len(), acts, "the thumb no longer follows the hand");

        drive_pointer(&mut probe, mouse(MouseEventKind::Down(MouseButton::Left), 10, 10));
        assert_eq!(probe.acts.last(), Some(&FWD), "the press itself steps");
        let acts = probe.acts.len();
        HostedRoom::leave(&mut probe);
        std::thread::sleep(crate::kit::ARROW_DELAY + Duration::from_millis(20));
        probe.after_frame();
        assert_eq!(probe.acts.len(), acts, "and the held arrow stops with the leave");
    }

    #[test]
    fn after_frame_pumps_then_ticks_and_reports_the_hand() {
        let mut probe = drawn();
        probe.ui.pointer = Some(Position { x: 2, y: 0 });
        assert!(probe.after_frame(), "over a click rect the hand shows");
        assert_eq!(probe.log, vec!["pump", "tick"]);
        probe.ui.dwell_backdate(crate::kit::TIP_DELAY);
        assert!(probe.ui.ripe_tooltip().is_some(), "the dwell was aged");

        probe.ui.pointer = Some(Position { x: 30, y: 9 });
        assert!(!probe.after_frame(), "over nothing it does not");

        // A held endcap steps once its delay is out.
        drive_pointer(&mut probe, mouse(MouseEventKind::Down(MouseButton::Left), 10, 10));
        assert_eq!(probe.acts, vec![FWD]);
        std::thread::sleep(crate::kit::ARROW_DELAY + Duration::from_millis(20));
        probe.after_frame();
        assert_eq!(probe.acts, vec![FWD, FWD], "the hold repeats from after_frame");
    }

    #[test]
    fn the_claim_defaults_to_all_while_a_modal_is_up() {
        let mut probe = Probe::default();
        assert_eq!(probe.claims(), Claim::Open);
        assert!(!probe.modal_up());
        assert_eq!(probe.tips(), "", "a page that never said otherwise hints nothing");
        probe.modal = true;
        assert_eq!(probe.claims(), Claim::All);
        assert!(probe.modal_up());
        assert_eq!(Claim::default(), Claim::Open);

        probe.set_key_hints(false);
        assert!(!probe.ui.key_hints);
        probe.set_key_hints(true);
        assert!(probe.ui.key_hints);

        /// A page whose Tab is its own, the way Torrents' Choose page's is.
        #[derive(Default)]
        struct Chooser {
            ui: Surface<u16>,
        }
        impl Screen for Chooser {
            type Act = u16;
            fn ui(&mut self) -> &mut Surface<u16> {
                &mut self.ui
            }
            fn pump(&mut self) {}
            fn render(&mut self, _frame: &mut Frame) {}
            fn key(&mut self, _key: KeyEvent) -> Option<Outcome> {
                None
            }
            fn act(&mut self, _act: u16) -> Option<Outcome> {
                None
            }
            fn wheel(&mut self, _up: bool, _at: Position) {}
            fn claim(&self) -> Claim {
                Claim::OwnTab
            }
        }
        let chooser: Box<dyn HostedRoom> = Box::new(Chooser::default());
        assert_eq!(chooser.claims(), Claim::OwnTab, "a page's own claim is what the host hears");
    }

    #[test]
    fn every_room_opens_through_the_factory() {
        // Drawn through the host face the way the GUI draws it: nothing
        // above, left or right of the area at the floor (the host blanks
        // what spills below), and nothing outside it at all when docked.
        fn drawn_in(room: &mut dyn HostedRoom, size: (u16, u16), area: Rect) -> Buffer {
            draw_with(size, |frame| {
                let all = frame.area();
                for y in all.top()..all.bottom() {
                    for x in all.left()..all.right() {
                        frame.buffer_mut()[(x, y)].set_symbol("·");
                    }
                }
                room.draw_in(frame, area);
            })
        }
        for id in RoomId::ALL {
            let client = Client::new("http://host.invalid:3000").expect("client");
            let mut room = open_room(id, client, false);
            assert!(!room.modal_up(), "{id:?} opens with no modal");
            assert_eq!(room.claims(), Claim::Open, "{id:?} opens holding no keys of the host's");
            room.set_key_hints(false);
            let floor = drawn_in(room.as_mut(), (100, 24), FLOOR);
            assert_eq!(outside(&floor, FLOOR, false), vec![], "{id:?} at the floor");
            let docked = drawn_in(room.as_mut(), (176, 46), DOCKED);
            assert_eq!(outside(&docked, DOCKED, true), vec![], "{id:?} docked");
            room.leave();
        }
        assert_eq!(RoomId::ALL.len(), 6);
        assert_eq!(RoomId::ALL[0], RoomId::Libraries);
    }

    #[test]
    fn a_home_reference_is_the_tilde_alone_or_before_a_separator() {
        assert!(home_ref("~"));
        assert!(home_ref("~/"));
        assert!(home_ref("~/Music"));
        assert!(home_ref("~\\Music"));
        assert!(!home_ref("~music"), "a folder called ~music");
        assert!(!home_ref("/srv/~"));
        assert!(!home_ref(""));
    }

    #[test]
    fn a_home_reference_is_rooted_at_the_servers_home_in_its_own_separator() {
        assert_eq!(join_home("/home/me", "~"), "/home/me");
        assert_eq!(join_home("/home/me", "~/"), "/home/me/");
        assert_eq!(join_home("/home/me/", "~/Music"), "/home/me/Music");
        assert_eq!(join_home("C:\\Users\\me", "~\\Music"), "C:\\Users\\me\\Music");
        assert_eq!(
            join_home("C:\\Users\\me\\", "~/Music/2024"),
            "C:\\Users\\me\\Music\\2024",
            "typed unix-style at a Windows server"
        );
        assert_eq!(join_home("/", "~/x"), "/x", "a root as the home keeps one separator");
    }
}
