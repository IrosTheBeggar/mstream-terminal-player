//! The admin panel: `mstream-player admin [libraries|discovery|federation|backups]` — the
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

mod backups;
mod discovery;
mod federation;
mod libraries;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
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
        RoomCmd::Federation => run_tui(federation::start(client)),
        RoomCmd::Backups => run_tui(backups::start(client, args.same_machine)),
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
