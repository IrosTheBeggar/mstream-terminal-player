//! The admin panel: `mstream-player admin [libraries|discovery]` — the
//! server's management rooms, drawn full-screen from the UI kit the way
//! the setup wizard is, against the saved session's server.
//!
//! Each room is a [`Screen`]. This hub owns the terminal session around
//! it — ground lease, mouse capture, pointer contract, the event loop,
//! teardown (the wizard's, minus pictures) — plus the chrome every room
//! shares (the header, the note and tips lines on the bottom edge) and
//! the gate sentences for the errors B4 warns a terminal client will hit.
//! Rooms keep their own state, worker and drawing; the loop only asks a
//! room to pump its worker, tick its timers, draw, and answer input.

mod discovery;
mod libraries;

use std::time::Duration;

use clap::{Args, Subcommand};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, Paragraph};
use rust_i18n::t;

use crate::api::{ApiError, Client};
use crate::kit::theme::th;
use crate::kit::{
    GroundGuard, POINTER_RESET, Surface, accent, bold, dim, set_pointer_shape, theme,
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

    /// This terminal runs on the server's own machine: adding a folder opens
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
    match args.room.unwrap_or(RoomCmd::Libraries) {
        RoomCmd::Libraries => run_tui(libraries::start(client, args.same_machine)),
        RoomCmd::Discovery => run_tui(discovery::start(client)),
    }
}

/// How a room's loop ended.
pub(crate) enum Outcome {
    Quit,
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

    fn render(&mut self, frame: &mut Frame);

    /// A key press (Ctrl-C is the loop's own).
    fn key(&mut self, key: KeyEvent) -> Option<Outcome>;

    /// A click, drag or hold resolved to one of the room's actions.
    fn act(&mut self, act: Self::Act) -> Option<Outcome>;

    /// The wheel over `at`: which list scrolls is the room's to say.
    fn wheel(&mut self, up: bool, at: Position);
}

/// The terminal session around a room: ground lease, mouse capture,
/// pointer contract, event loop, teardown.
fn run_tui<S: Screen>(mut screen: S) -> i32 {
    let _title = crate::tui::WindowTitle::claim("mStream Admin");

    // Claim the window background BEFORE ratatui takes the terminal — the
    // OSC 11 query runs its own raw-mode transaction on the tty.
    let claim = theme::acquire_ground();
    let ground_guard = GroundGuard;

    let mut terminal = ratatui::init();
    let mouse_on = execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    if let Some(seq) = claim {
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(seq));
    }
    set_pointer_shape(false, mouse_on);
    let outcome = event_loop(&mut terminal, &mut screen, mouse_on);
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
    terminal: &mut ratatui::DefaultTerminal,
    screen: &mut S,
    mouse_on: bool,
) -> std::io::Result<Outcome> {
    let mut hand = false;
    loop {
        screen.tick();
        terminal.draw(|frame| screen.render(frame))?;
        screen.pump();

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
                    let at = Position { x: mouse.column, y: mouse.row };
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if !screen.ui().begin_press(at) {
                                continue;
                            }
                            let hit = screen.ui().hit(at);
                            if let Some(act) = hit
                                && let Some(outcome) = screen.act(act)
                            {
                                return Ok(outcome);
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
                }
                _ => {}
            }
        }
    }
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
    let head = Rect { x: 2, y: 0, width: area.width.saturating_sub(4), height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(title.to_string(), bold())), head);
    frame.render_widget(
        Paragraph::new(Span::styled(format!("{host} · {}", t!("admin.role")), dim()))
            .alignment(Alignment::Right),
        head,
    );
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
    let line = Rect { x: 2, y: area.height.saturating_sub(2), width, height: 1 };
    if let Some((text, is_err)) = note {
        let style = if *is_err { Style::default().fg(th().gold) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(text.clone(), style)), line);
    }
    if let Some(busy) = busy {
        frame.render_widget(Paragraph::new(Span::styled(busy.to_string(), accent())), line);
    }
    let tips_rect = Rect { x: 2, y: area.height.saturating_sub(1), width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(tips.to_string(), dim())), tips_rect);
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
