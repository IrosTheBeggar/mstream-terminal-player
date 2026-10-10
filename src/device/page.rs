//! The MP3 Player page: one card about the board in view (mStream
//! docs/designs/mp3-tab, alternate A, "One card";
//! docs/ux-contracts/mp3-player-screen.md). It is `mstream-player device
//! flash` without `--yes`, drawn on the admin hub's terminal session, and
//! the GUI's MP3 Player tab hosts it whole under its top bar
//! (`render_hosted`).
//!
//! The worker (desk.rs) watches the ports and asks every board over USB
//! with no reset; this page draws the boards it reports — the head, the
//! firmware's verdict as one chip, the SD card as the running firmware
//! reports it, one primary that follows the verdict — and keeps only what
//! is its own: which board is in view, Details, the gate, the write's lock,
//! the time left, the log and the notes. The verdict and the card are
//! board.rs's, so this card and `device list` never disagree.
//!
//! Every write passes the gate, the kit's gold warning modal, and the
//! board's reset comes after its `y`. From then the write cannot be left;
//! the other boards can be looked at while it runs and never written. Done
//! is the card again: the worker listens for the board coming back, and
//! the chip and the card say what it runs now.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use rust_i18n::t;

use super::FlashArgs;
use super::board::{Board, Card, CardKind, CardUnknown, Count, CountWhy, Free, Heard, Primary, Probe, Tracks};
use super::board::{Verdict, Work, Written, gb};
use super::desk::{self, All, Cmd, Event, LogKind, Refusal};
use super::firmware::{Origin, Place, Target, place};
use super::flow::Phase;
use super::listen::{self, IdentifyWhy};
use super::{DeviceError, engine};
use crate::admin::{Outcome, Screen, draw_foot, draw_header_as, fmt_count, frame_ground};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{self, Surface, accent, bold, dim};

/// The standalone page's least window: the card at 68 cells with its
/// tallest content, the header above and the busy and tips rows below.
const MIN_W: u16 = 72;
const MIN_H: u16 = 24;
/// The card, border to border: today's 78-cell column.
const CARD_W: u16 = 78;
/// The border and two cells of padding, each side — one cell on a card
/// narrower than the column (the console's 68, whose bar is then 50).
const CARD_INSET: u16 = 3;
/// The label column: wide enough for French "Micrologiciel" and a gap.
const LABEL_W: u16 = 14;
/// The gate's width: the top of the kit's 52–74 range, so it covers the
/// card's words and only the card's border shows at its sides.
const GATE_W: u16 = 74;
/// The boards Update all's gate names before "and N more": six fit the
/// GUI's floor.
const GATE_ROWS: usize = 6;
/// A tall button's rows.
const BUTTON_H: u16 = 3;
/// The write's time left is said once this much of it has gone by: the
/// first chunks are not a rate.
const ESTIMATE_AFTER: Duration = Duration::from_secs(2);
/// The clock's width in a log line: `mm:ss` and two spaces.
const CLOCK_W: usize = 7;
/// How long a passing note (a board plugged in, Show on the player) stays
/// on the busy line.
const NOTE_FOR: Duration = Duration::from_secs(8);
/// The GUI's footer is drawn from x 1: a hint has 99 cells.
const HINT_W: usize = 99;
/// How long the page waits, on its way out, for the worker to let every
/// port go: a read under way finishes and restarts its board (one rung of
/// espflash's connect at most), anything else lets go at once.
pub(crate) const LET_GO: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    /// The card's primary: a gate, or Read the board.
    Primary,
    Details,
    WriteAgain,
    Count,
    Show,
    Ask,
    UpdateAll,
    /// The port tab at this place.
    Board(usize),
    /// The gate's safe choice.
    Keep,
    /// The gate's write.
    Write,
    /// The install gate's erase box.
    Erase,
}

/// What a gate writes: its title, its words and its button follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// An older release of ours.
    Update,
    /// A development build, or a version the order cannot place.
    Replace,
    /// Over other firmware, or nothing.
    Install,
    /// The very version again (a repair), or after a failed write.
    Again,
    /// The pin over a newer version.
    Back,
    /// Update all.
    All,
}

/// The gate: one function's titles over every write (contract clause 23).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Gate {
    kind: Kind,
    /// The board written; none for Update all, whose boards are `rows`.
    port: Option<String>,
    from: Option<String>,
    to: String,
    /// What an install replaces, by its app description's name.
    name: Option<String>,
    erase: bool,
    rows: Vec<GateRow>,
}

/// One board in Update all's gate.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GateRow {
    port: String,
    serial: Option<String>,
    from: String,
}

/// The write the page is locked to (contract clause 9), from the gate's
/// yes until it has ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Lock {
    /// One board's. `seen` once its board has shown the write under way, so
    /// a result it carried from before is not taken for this one's.
    One { port: String, kind: Kind, from: Option<String>, seen: bool },
    /// Update all's: its boards, the one at its turn, and the one seen
    /// under way (whose end is told on the busy line).
    All { ports: Vec<String>, at: usize, current: Option<String> },
}

/// The image the worker has in hand.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Image {
    version: String,
    origin: String,
    kind: Origin,
}

/// A line on the busy row: a result, a board plugged in, a refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Note {
    text: String,
    gold: bool,
    /// Gone after this; none stays until something replaces it.
    until: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LogLine {
    /// Since the page opened.
    at: Duration,
    /// The board it is about; none for the image and the watch.
    port: Option<String>,
    text: String,
    kind: LogKind,
}

/// The write's rate, for the time left: whose, since when, from what
/// percent.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rate {
    port: String,
    since: Instant,
    from: u8,
}

pub(crate) struct Page {
    cmds: Sender<Cmd>,
    events: Receiver<Event>,
    /// `--erase` / `--no-erase` on the standalone page: the gate's erase.
    erase_asked: Option<bool>,
    target: Option<Target>,
    image: Option<Image>,
    download: Option<(u64, Option<u64>)>,
    /// Why the image could not be had (Details' "This player" row).
    image_failed: Option<String>,
    /// Every board, in the order the OS lists their ports.
    boards: Vec<Board>,
    /// The ports of the last watch, in its order.
    order: Vec<String>,
    watched: bool,
    others: Vec<String>,
    /// The ports could not be listed at all.
    list_failed: Option<DeviceError>,
    /// The board in view, by its port.
    open: Option<String>,
    /// Details (or, with no board, "Not showing up?") open.
    details: bool,
    gate: Option<Gate>,
    lock: Option<Lock>,
    rate: Option<Rate>,
    time_left: Option<u64>,
    /// The boards Details asked `L` of, once each.
    facts_asked: HashSet<String>,
    note: Option<Note>,
    /// Told the worker to let go.
    quitting: bool,
    /// The worker said every port is free, or ended.
    released: bool,
    /// The worker told of a board: it may have a port open.
    touched: bool,
    opened: Instant,
    log: Vec<LogLine>,
    /// Lines scrolled up from the newest; 0 follows it.
    log_scroll: usize,
    /// Where the log was drawn this frame, and how many lines it shows,
    /// for the wheel.
    log_rect: Option<Rect>,
    ui: Surface<Act>,
}

pub(crate) fn run(args: FlashArgs) -> i32 {
    let engine: desk::Shared = std::sync::Arc::from(engine::from_env());
    // Not the firmware first: the boards are judged while the image comes.
    let (cmds, events) = desk::spawn(engine, args.source(), args.port.clone(), desk::Timing::REAL, false);
    let mut page = Page::with_channels(cmds, events, args.erase_asked());
    let code = crate::admin::run_tui_as(&mut page, "mStream MP3 Player");
    // Ctrl+C leaves the loop at once: a read under way still restarts its
    // board, and every listen lets its port go, before the process ends.
    page.let_go_within(LET_GO);
    code
}

/// The page the GUI's MP3 Player tab hosts (contract clauses 1 and 5):
/// `device flash` with no flags — the pinned firmware, every board
/// watched. Building it starts the worker, which only listens. Never under
/// test: a test's page rides [`Page::quiet`], since this worker downloads
/// the firmware and opens real serial ports.
#[cfg(not(test))]
pub(crate) fn hosted() -> Page {
    let engine: desk::Shared = std::sync::Arc::from(engine::from_env());
    let source = super::firmware::Source::Pinned;
    let (cmds, events) = desk::spawn(engine, source, None, desk::Timing::REAL, false);
    Page::with_channels(cmds, events, None)
}

impl Page {
    fn with_channels(cmds: Sender<Cmd>, events: Receiver<Event>, erase_asked: Option<bool>) -> Page {
        Page {
            cmds,
            events,
            erase_asked,
            target: None,
            image: None,
            download: None,
            image_failed: None,
            boards: Vec::new(),
            order: Vec::new(),
            watched: false,
            others: Vec::new(),
            list_failed: None,
            open: None,
            details: false,
            gate: None,
            lock: None,
            rate: None,
            time_left: None,
            facts_asked: HashSet::new(),
            note: None,
            quitting: false,
            released: false,
            touched: false,
            opened: Instant::now(),
            log: Vec::new(),
            log_scroll: 0,
            log_rect: None,
            ui: Surface::new(),
        }
    }

    // ── The worker's reports ────────────────────────────────────────────────

    fn apply(&mut self, event: Event) {
        self.apply_at(event, Instant::now())
    }

    fn apply_at(&mut self, event: Event, now: Instant) {
        match event {
            Event::Target(target) => self.target = Some(target),
            Event::Download { done, total } => self.download = Some((done, total)),
            Event::Firmware { version, origin, kind, .. } => {
                self.download = None;
                self.image_failed = None;
                self.image = Some(Image { version, origin, kind });
            }
            Event::FirmwareFailed(e) => {
                self.download = None;
                self.image_failed = Some(e.text());
            }
            Event::Watch { ports, others } => self.watch(ports, others, now),
            Event::Board(board) => self.board(board, now),
            Event::Gone { port } => self.gone(&port, now),
            // The worker logs what its bootloader showed and the plan.
            Event::Plan { .. } => {}
            Event::Identified { port, label, result } => self.identified(&port, &label, result, now),
            Event::All(all) => self.all(all),
            Event::Refused { port, why } => self.refused(port, why, now),
            Event::Log { port, text, kind } => self.log_at(now, port, text, kind),
            Event::Failed(e) => self.list_failed = Some(e),
            Event::Released => self.released = true,
        }
    }

    fn log_at(&mut self, now: Instant, port: Option<String>, text: String, kind: LogKind) {
        self.log.push(LogLine { at: now.saturating_duration_since(self.opened), port, text, kind });
    }

    fn say(&mut self, text: impl Into<String>, gold: bool, until: Option<Instant>) {
        self.note = Some(Note { text: text.into(), gold, until });
    }

    /// The watch looked: the boards' order, the other ports, and the
    /// boards that arrived since the last look — asked without a reset,
    /// and never taking the view (contract clause 29).
    fn watch(&mut self, ports: Vec<String>, others: Vec<String>, now: Instant) {
        if self.watched {
            let arrived: Vec<String> = ports.iter().filter(|p| !self.order.contains(p)).cloned().collect();
            for port in arrived {
                self.say(t!("dev.log_plugged", port = short(&port)), false, Some(now + NOTE_FOR));
            }
        }
        self.watched = true;
        self.order = ports;
        self.others = others;
        let order = self.order.clone();
        self.boards.sort_by_key(|b| order.iter().position(|p| p == b.port()).unwrap_or(usize::MAX));
        if self.open.is_none() {
            self.open = self.order.first().cloned();
        }
    }

    /// A board, whole, as the worker knows it now.
    fn board(&mut self, board: Board, now: Instant) {
        self.touched = true;
        let port = board.port().to_string();
        match self.boards.iter_mut().find(|b| b.port() == port) {
            Some(known) => *known = board.clone(),
            None => {
                let place = self.order.iter().position(|p| *p == port).unwrap_or(usize::MAX);
                let at = self
                    .boards
                    .iter()
                    .position(|b| self.order.iter().position(|p| p == b.port()).unwrap_or(usize::MAX) > place)
                    .unwrap_or(self.boards.len());
                self.boards.insert(at, board.clone());
            }
        }
        if self.open.is_none() {
            self.open = Some(port);
        }
        self.track(&board, now);
    }

    /// The write's progress and its end, from the board the write is on.
    fn track(&mut self, board: &Board, now: Instant) {
        let port = board.port().to_string();
        let under_way = matches!(board.work, Work::Queued | Work::Writing { .. });
        if let Work::Writing { phase, pct } = board.work {
            self.estimate(&port, phase, pct, now);
        }
        let ended = !under_way && board.written.is_some();
        match &mut self.lock {
            Some(Lock::One { port: locked, seen, kind, from }) if *locked == port => {
                if under_way {
                    *seen = true;
                } else if *seen && ended {
                    let (kind, from) = (*kind, from.clone());
                    self.lock = None;
                    self.ended(board, kind, from);
                }
            }
            Some(Lock::All { current, .. }) => {
                if under_way {
                    *current = Some(port);
                } else if ended && current.as_deref() == Some(port.as_str()) {
                    *current = None;
                    match &board.written {
                        Some(Written::Done { took, .. }) => {
                            let line = t!("dev.note_all_one", port = short(&port), secs = took.as_secs());
                            self.say(line, false, None);
                        }
                        _ => self.failed_open(&port),
                    }
                }
            }
            _ => {}
        }
    }

    /// One board's write ended: Done's one line (contract clause 24), or
    /// the board in view with Details open on its log.
    fn ended(&mut self, board: &Board, kind: Kind, from: Option<String>) {
        self.rate = None;
        self.time_left = None;
        let text = match &board.written {
            Some(Written::Done { skipped: true, .. }) => t!("dev.done_skipped").to_string(),
            Some(Written::Done { install: true, .. }) => t!("dev.note_installed").to_string(),
            Some(Written::Done { version, .. }) => match (kind, from) {
                // Only a step forward is an update. Going back from a newer
                // version, or over one the order cannot place, replaced it.
                (Kind::Update | Kind::Replace, Some(from)) if place(&from, version) == Place::Older => {
                    t!("dev.note_updated", from = from).to_string()
                }
                (Kind::Update | Kind::Replace | Kind::Back, Some(from)) => {
                    t!("dev.note_replaced", from = from).to_string()
                }
                _ => t!("dev.note_again").to_string(),
            },
            _ => return self.failed_open(board.port()),
        };
        self.say(text, false, None);
    }

    /// A write failed: its board in view, Details opened by itself on the
    /// log's last lines.
    fn failed_open(&mut self, port: &str) {
        self.open = Some(port.to_string());
        self.details = true;
        self.log_scroll = 0;
        self.note = None;
    }

    fn gone(&mut self, port: &str, now: Instant) {
        let at = self.boards.iter().position(|b| b.port() == port);
        self.boards.retain(|b| b.port() != port);
        self.order.retain(|p| p != port);
        if matches!(&self.lock, Some(Lock::One { port: locked, .. }) if locked == port) {
            self.lock = None;
        }
        if self.open.as_deref() == Some(port) {
            // The next to the right, else the left.
            let right = at.and_then(|i| self.boards.get(i));
            let next = right.or_else(|| at.and_then(|i| i.checked_sub(1)).and_then(|j| self.boards.get(j)));
            self.open = next.map(|b| b.port().to_string());
            self.log_scroll = 0;
        }
        let text = match self.boards.as_slice() {
            [left] => t!("dev.note_left_one", port = short(port), left = short(left.port())).to_string(),
            _ => t!("dev.log_unplugged", port = short(port)).to_string(),
        };
        self.say(text, false, Some(now + NOTE_FOR));
    }

    fn identified(&mut self, port: &str, label: &str, result: Result<(), IdentifyWhy>, now: Instant) {
        let (text, gold) = match result {
            Ok(()) => (t!("dev.note_shown", label = label).to_string(), false),
            Err(IdentifyWhy::Ui) => (t!("dev.identify_ui").to_string(), true),
            Err(IdentifyWhy::Screen) => (t!("dev.identify_screen").to_string(), true),
            Err(IdentifyWhy::Viz) => (t!("dev.identify_viz").to_string(), true),
            Err(_) => (t!("dev.identify_failed", port = short(port)).to_string(), true),
        };
        self.say(text, gold, Some(now + NOTE_FOR));
    }

    fn all(&mut self, all: All) {
        match all {
            All::Running { ports, at } => {
                let current = match &self.lock {
                    Some(Lock::All { current, .. }) => current.clone(),
                    _ => None,
                };
                self.lock = Some(Lock::All { ports, at, current });
            }
            All::Done { ports, passed, took } => {
                self.lock = None;
                self.rate = None;
                let n = ports.len().saturating_sub(passed.len());
                self.say(t!("dev.note_all_done", n = n, took = took_text(took)), false, None);
            }
            All::Stopped { ports, at, .. } => {
                self.lock = None;
                self.rate = None;
                let port = ports.get(at).cloned().unwrap_or_default();
                let half = self.boards.iter().any(|b| b.port() == port && b.verdict == Verdict::HalfWritten);
                let text = if half {
                    t!("dev.note_all_half", port = short(&port)).to_string()
                } else {
                    t!("dev.log_all_stopped", port = short(&port)).to_string()
                };
                self.failed_open(&port);
                self.say(text, true, None);
            }
        }
    }

    /// A command the worker would not run: nothing was touched, so a write
    /// it refused unlocks the page.
    fn refused(&mut self, port: Option<String>, why: Refusal, now: Instant) {
        let unlock = match (&self.lock, &port) {
            (Some(Lock::One { port: locked, seen: false, .. }), Some(port)) => locked == port,
            (Some(Lock::All { current: None, .. }), None) => true,
            _ => false,
        };
        if unlock {
            self.lock = None;
        }
        let name = port.as_deref().map(short).unwrap_or_default();
        let text = match why {
            Refusal::Leaving => return,
            Refusal::NoBoard => t!("dev.refused_gone", port = name),
            Refusal::OneAtATime { busy } => t!("dev.refused_one", port = short(&busy)),
            Refusal::Busy => t!("dev.refused_busy", port = name),
            Refusal::NotAnswering => t!("dev.refused_not_answering", port = name),
            Refusal::NothingToUpdate => t!("dev.refused_nothing"),
        };
        self.say(text, false, Some(now + NOTE_FOR));
    }

    /// The write's time left, from the rate since its first sample: said
    /// once a couple of seconds and some percent have gone by, to the
    /// nearest five seconds. A phase before the write starts the count
    /// over.
    fn estimate(&mut self, port: &str, phase: Phase, pct: Option<u8>, now: Instant) {
        let Some(pct) = pct.filter(|_| phase == Phase::Writing) else {
            if matches!(phase, Phase::Connecting | Phase::Comparing) {
                self.rate = None;
                self.time_left = None;
            }
            return;
        };
        match &self.rate {
            Some(rate) if rate.port == port && pct > rate.from => {
                let gone = now.saturating_duration_since(rate.since);
                if gone >= ESTIMATE_AFTER {
                    let speed = f64::from(pct - rate.from) / gone.as_secs_f64();
                    let left = f64::from(100 - pct.min(100)) / speed;
                    self.time_left = Some(((left / 5.0).round() * 5.0) as u64);
                }
            }
            Some(rate) if rate.port == port && pct == rate.from => {}
            _ if pct > 0 && pct < 100 => {
                self.rate = Some(Rate { port: port.to_string(), since: now, from: pct });
                self.time_left = None;
            }
            _ => {}
        }
    }

    // ── What the page asks ──────────────────────────────────────────────────

    fn send(&mut self, cmd: Cmd) -> bool {
        if self.cmds.send(cmd).is_ok() {
            return true;
        }
        self.worker_gone();
        false
    }

    /// A worker that ended without a last word: only a bug does that. It
    /// holds no port; the page says so instead of hanging.
    fn worker_gone(&mut self) {
        self.released = true;
        self.lock = None;
        self.gate = None;
        if !self.quitting {
            self.say(t!("note.worker_gone"), true, None);
        }
    }

    /// The board in view.
    fn shown(&self) -> Option<&Board> {
        let open = self.open.as_deref()?;
        self.boards.iter().find(|b| b.port() == open)
    }

    fn shown_index(&self) -> Option<usize> {
        let open = self.open.as_deref()?;
        self.boards.iter().position(|b| b.port() == open)
    }

    /// The board being written, while the page is locked.
    fn writing_port(&self) -> Option<&str> {
        match self.lock.as_ref()? {
            Lock::One { port, .. } => Some(port),
            Lock::All { current: Some(port), .. } => Some(port),
            Lock::All { ports, at, .. } => ports.get(*at).map(String::as_str),
        }
    }

    /// The card's primary for `board`: its verdict's, while nothing else
    /// is under way on it.
    fn primary_of(&self, board: &Board) -> Option<Primary> {
        if board.work != Work::Idle || self.target.is_none() || self.written_now(board) {
            return None;
        }
        board.primary()
    }

    /// The board is the one the page is locked to: its write is under way,
    /// whether or not the worker has said so yet.
    fn written_now(&self, board: &Board) -> bool {
        matches!(board.work, Work::Writing { .. } | Work::Queued) || self.writing_port() == Some(board.port())
    }

    fn can_write_again(&self, board: &Board) -> bool {
        self.details
            && self.lock.is_none()
            && self.target.is_some()
            && board.work == Work::Idle
            && board.ours()
            && matches!(board.verdict, Verdict::UpToDate | Verdict::Newer | Verdict::Unplaced)
            && !matches!(board.written, Some(Written::Failed { .. }))
    }

    fn can_count(&self, board: &Board) -> bool {
        self.lock.is_none()
            && board.work == Work::Idle
            && board.answers_status()
            && matches!(board.card(), Card::Fat { free: Free::NotCounted, .. })
    }

    fn can_show(&self, board: &Board) -> bool {
        self.boards.len() >= 2 && self.lock.is_none() && board.work == Work::Idle && board.answers_status()
    }

    /// Ask again, named on the card where a board could not be asked.
    fn offers_ask(&self, board: &Board) -> bool {
        self.lock.is_none()
            && board.work == Work::Idle
            && matches!(board.heard, Heard::InUse { .. } | Heard::Failed(_))
    }

    fn needing(&self) -> Vec<&Board> {
        self.boards.iter().filter(|b| b.needs_update()).collect()
    }

    fn can_update_all(&self) -> bool {
        self.lock.is_none() && self.target.is_some() && self.needing().len() >= 2
    }

    /// The gate for `board`: its primary's (`again` false) or Details'
    /// Write again.
    fn gate_for(&self, board: &Board, again: bool) -> Option<Gate> {
        let to = self.target.as_ref()?.version.clone();
        let from = board.version().map(str::to_string);
        let read_name = match &board.probe {
            Some(Ok(Probe { on_board: Some(desc), .. })) if !desc.is_ours() => Some(desc.project.clone()),
            _ => None,
        };
        let kind = if again {
            match board.verdict {
                Verdict::Newer => Kind::Back,
                Verdict::Unplaced if from.is_some() => Kind::Replace,
                _ => Kind::Again,
            }
        } else if let Some(Written::Failed { half, .. }) = &board.written {
            // Half written: writing again fixes it, whatever it was. A write
            // that failed before it touched the flash is the same write as
            // before.
            match (&board.verdict, half) {
                (Verdict::Update, false) => Kind::Update,
                (Verdict::DevUpdate, false) => Kind::Replace,
                (Verdict::Other { .. } | Verdict::Blank, false) => Kind::Install,
                _ if board.ours() => Kind::Again,
                _ => Kind::Install,
            }
        } else {
            match board.verdict {
                Verdict::Update => Kind::Update,
                Verdict::DevUpdate => Kind::Replace,
                Verdict::Other { .. } | Verdict::Blank => Kind::Install,
                _ => return None,
            }
        };
        let erase = match kind {
            Kind::Install => self.erase_asked.unwrap_or(true),
            _ => self.erase_asked.unwrap_or(false),
        };
        let name = match (&board.verdict, read_name) {
            (Verdict::Other { name }, _) => Some(name.clone()),
            (_, name) => name,
        };
        Some(Gate { kind, port: Some(board.port().to_string()), from, to, name, erase, rows: Vec::new() })
    }

    /// Update all's gate: every board that needs it, named.
    fn gate_all(&self) -> Option<Gate> {
        let to = self.target.as_ref()?.version.clone();
        let rows: Vec<GateRow> = self
            .needing()
            .into_iter()
            .map(|b| GateRow {
                port: b.port().to_string(),
                serial: b.serial().map(str::to_string),
                from: b.version().unwrap_or("?").to_string(),
            })
            .collect();
        let gate = Gate { kind: Kind::All, port: None, from: None, to, name: None, erase: false, rows };
        (gate.rows.len() >= 2).then_some(gate)
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Primary => {
                let board = self.shown().cloned()?;
                if self.lock.is_some() {
                    return None;
                }
                match self.primary_of(&board) {
                    // A read writes nothing: no gate, its cost said on the card.
                    Some(Primary::Read) => {
                        self.send(Cmd::Read { port: board.port().to_string() });
                    }
                    Some(_) => self.gate = self.gate_for(&board, false),
                    None => {}
                }
            }
            Act::Details => {
                self.details = !self.details;
                self.log_scroll = 0;
                if self.details {
                    self.ask_facts();
                }
            }
            Act::WriteAgain => {
                if let Some(board) = self.shown().cloned()
                    && self.can_write_again(&board)
                {
                    self.gate = self.gate_for(&board, true);
                }
            }
            Act::Count => {
                if let Some(board) = self.shown().cloned()
                    && self.can_count(&board)
                {
                    self.send(Cmd::Count { port: board.port().to_string() });
                }
            }
            Act::Show => {
                if let Some(board) = self.shown().cloned()
                    && self.can_show(&board)
                {
                    self.send(Cmd::Identify { port: board.port().to_string() });
                }
            }
            Act::Ask => {
                if let Some(board) = self.shown().cloned()
                    && self.lock.is_none()
                    && board.work == Work::Idle
                {
                    self.send(Cmd::Listen { port: board.port().to_string() });
                }
            }
            Act::UpdateAll => {
                if self.can_update_all() {
                    self.gate = self.gate_all();
                }
            }
            Act::Board(i) => {
                if let Some(board) = self.boards.get(i) {
                    self.open = Some(board.port().to_string());
                    self.log_scroll = 0;
                    if self.details {
                        self.ask_facts();
                    }
                }
            }
            Act::Keep => self.gate = None,
            Act::Write => self.gate_yes(),
            Act::Erase => {
                if let Some(gate) = self.gate.as_mut()
                    && gate.kind == Kind::Install
                {
                    gate.erase = !gate.erase;
                }
            }
        }
        None
    }

    /// The gate's yes: the write sent, the page locked from now on.
    fn gate_yes(&mut self) {
        let Some(gate) = self.gate.take() else { return };
        self.note = None;
        self.rate = None;
        self.time_left = None;
        if gate.kind == Kind::All {
            let ports: Vec<String> = gate.rows.iter().map(|r| r.port.clone()).collect();
            if self.send(Cmd::UpdateAll { ports: ports.clone() }) {
                self.lock = Some(Lock::All { ports, at: 0, current: None });
            }
        } else if let Some(port) = gate.port
            && self.send(Cmd::Write { port: port.clone(), erase: Some(gate.erase) })
        {
            self.lock = Some(Lock::One { port, kind: gate.kind, from: gate.from, seen: false });
        }
    }

    /// Details' Board row wants the flash size, which only `L` says without
    /// a reset: asked once per board, when Details opens on it.
    fn ask_facts(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let Some(board) = self.shown() else { return };
        let answers = matches!(board.heard, Heard::Status(_) | Heard::Old { .. });
        if !answers || board.work != Work::Idle || board.flash_mb.is_some() {
            return;
        }
        let port = board.port().to_string();
        if self.facts_asked.insert(port.clone()) {
            self.send(Cmd::Facts { port });
        }
    }

    fn switch(&mut self, step: isize) {
        let n = self.boards.len();
        if n < 2 {
            return;
        }
        let at = self.shown_index().unwrap_or(0) as isize;
        let next = (at + step).rem_euclid(n as isize) as usize;
        self.act(Act::Board(next));
    }

    /// Esc, or the host letting the page go: the worker is told to let
    /// every port go, and the page ends once it has (contract clause 7). A
    /// write is never told anything.
    fn leave(&mut self) {
        if self.lock.is_some() || self.quitting {
            return;
        }
        self.gate = None;
        self.quitting = true;
        if self.cmds.send(Cmd::Quit).is_err() {
            self.released = true;
        }
    }

    /// The write is under way (contract clause 9): from the gate's yes
    /// until the board's write has ended, and for Update all its last
    /// board's. Nothing may leave it but the process's exit.
    pub(crate) fn writing(&self) -> bool {
        self.lock.is_some()
    }

    /// The worker was told to let go and has not yet said every port is
    /// free, and it told of a board, so it may have one open (contract
    /// clauses 8 and 11). A page that never saw a board holds nothing.
    pub(crate) fn holds_board(&self) -> bool {
        self.quitting && !self.released && self.touched
    }

    /// The host is done with the page — the GUI's tab left, or the player
    /// quitting. Asks the worker to let go Esc's way, then reads its
    /// reports, so [`Page::holds_board`] is the truth: false, and the page
    /// may go; true, and the host waits, reading on. Never a write.
    pub(crate) fn release(&mut self) {
        Screen::pump(self);
        if self.writing() {
            return;
        }
        self.leave();
        Screen::pump(self);
    }

    /// The standalone page's way out: let go, then wait for the worker,
    /// at most `within`.
    fn let_go_within(&mut self, within: Duration) {
        self.release();
        let t0 = Instant::now();
        while self.holds_board() && t0.elapsed() < within {
            std::thread::sleep(Duration::from_millis(20));
            Screen::pump(self);
        }
    }

    /// What a page that is letting go says meanwhile — and what the GUI's
    /// tab says while a visit waits for it.
    pub(crate) fn parting_words(&self) -> String {
        if self.boards.iter().any(|b| b.work == Work::Reading) {
            Phase::Restarting.text()
        } else {
            t!("dev.letting_ports_go").to_string()
        }
    }

    /// The write's words for `board`: its phase, and while it writes the
    /// version, the percent and the time left.
    fn write_words(&self, board: &Board) -> String {
        let to = self.target.as_ref().map_or("?", |t| t.version.as_str());
        match board.work {
            Work::Writing { phase: Phase::Writing, pct } => {
                let mut text = t!("dev.fw_writing", version = to).to_string();
                if let Some(pct) = pct {
                    text.push_str(&format!(" {pct}%"));
                    let mine = self.rate.as_ref().is_some_and(|r| r.port == board.port());
                    if let Some(secs) = self.time_left.filter(|_| mine && pct < 100) {
                        text.push_str(&format!(" · {}", time_left_text(secs)));
                    }
                }
                text
            }
            Work::Writing { phase, .. } => phase.text(),
            Work::Queued if self.image.is_none() => {
                let mut text = t!("dev.fw_waiting").to_string();
                if let Some((done, Some(total))) = self.download.filter(|(_, t)| t.is_some_and(|t| t > 0)) {
                    text.push_str(&format!(" {}%", (done * 100 / total).min(100)));
                }
                text
            }
            _ => Phase::Connecting.text(),
        }
    }

    /// The busy row: a page letting go; the write of a board not in view;
    /// the image's download. A note shows where there is none.
    fn busy(&self) -> Option<String> {
        if self.holds_board() {
            return Some(self.parting_words());
        }
        if let Some(port) = self.writing_port()
            && self.boards.len() >= 2
            && self.open.as_deref() != Some(port)
            && let Some(board) = self.boards.iter().find(|b| b.port() == port)
        {
            return Some(format!("{} · {}", short(port), self.write_words(board)));
        }
        if self.note.is_none()
            && let Some((done, Some(total))) = self.download
            && total > 0
        {
            return Some(format!("{} {}%", Phase::Firmware.text(), (done * 100 / total).min(100)));
        }
        None
    }

    // ── The footer ──────────────────────────────────────────────────────────

    /// The hint, in pieces and their place in the order they give way (0
    /// never): hosted, Esc leads back to the Library; on its own, out.
    fn hint_parts(&self, hosted: bool) -> Vec<(String, u8)> {
        let mut parts: Vec<(String, u8)> = Vec::new();
        let switch = |word: String| format!("{} {word}", glyphs().switch);
        if let Some(gate) = &self.gate {
            let yes = match gate.kind {
                Kind::Update | Kind::Replace => "dev.hint_gate_update",
                Kind::Install => "dev.hint_gate_install",
                Kind::Again => "dev.hint_gate_again",
                Kind::Back => "dev.hint_gate_back",
                Kind::All => "dev.hint_gate_all",
            };
            parts.push((t!(yes).to_string(), 0));
            if gate.kind == Kind::Install {
                parts.push((t!("dev.hint_gate_erase").to_string(), 0));
            }
            parts.push((t!("dev.hint_gate_cancel").to_string(), 0));
            return parts;
        }
        if let Some(port) = self.writing_port() {
            // The warning never gives way; the console's narrow footer
            // drops the keys after it.
            if self.boards.len() >= 2 {
                parts.push((t!("dev.hint_working_port", port = short(port)).to_string(), 0));
                parts.push((switch(t!("dev.hint_look").to_string()), 1));
            } else {
                parts.push((t!("dev.hint_working").to_string(), 0));
            }
            parts.push((t!("dev.hint_details").to_string(), 1));
            return parts;
        }
        let back = t!(if hosted { "dev.hint_library" } else { "dev.hint_leave" }).to_string();
        let Some(board) = self.shown() else {
            parts.push((t!("dev.hint_help").to_string(), 0));
            parts.push((back, 0));
            return parts;
        };
        if self.boards.len() >= 2 {
            parts.push((switch(t!("dev.hint_player").to_string()), 0));
        }
        let enter = match self.primary_of(board) {
            Some(Primary::Update) => Some("dev.hint_enter_update"),
            Some(Primary::Install) => Some("dev.hint_enter_install"),
            Some(Primary::Read) => Some("dev.hint_enter_read"),
            Some(Primary::TryAgain) => Some("dev.hint_enter_retry"),
            None => None,
        };
        if let Some(key) = enter {
            parts.push((t!(key).to_string(), 0));
        }
        if self.can_show(board) {
            parts.push((t!("dev.hint_show").to_string(), 2));
        }
        if self.can_update_all() {
            parts.push((t!("dev.hint_all").to_string(), 2));
        }
        if self.can_count(board) {
            parts.push((t!("dev.hint_count").to_string(), 1));
        }
        if self.can_write_again(board) {
            parts.push((t!("dev.hint_write").to_string(), 1));
        }
        if self.offers_ask(board) {
            parts.push((t!("dev.hint_ask").to_string(), 1));
        }
        parts.push((t!("dev.hint_details").to_string(), 0));
        parts.push((back, 0));
        parts
    }

    /// The hint in `max` cells: the lesser keys give way, the last first,
    /// before the primary, Details and Esc (contract clause 15).
    fn hint_line(&self, hosted: bool, max: usize) -> String {
        let mut parts = self.hint_parts(hosted);
        loop {
            let line = parts.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(" · ");
            if kit::width(&line) <= max {
                return line;
            }
            let worst = parts.iter().map(|(_, rank)| *rank).max().unwrap_or(0);
            if worst == 0 {
                return line;
            }
            let at = parts.iter().rposition(|(_, rank)| *rank == worst).expect("a part of that rank");
            parts.remove(at);
        }
    }

    /// The log lines about `port`, and the image's and the watch's.
    fn log_for(&self, port: &str) -> Vec<&LogLine> {
        self.log.iter().filter(|l| l.port.as_deref().is_none_or(|p| p == port)).collect()
    }
}

/// `about 20 s left`, or `a few seconds left` under five.
fn time_left_text(secs: u64) -> String {
    if secs < 5 { t!("dev.time_left_few").to_string() } else { t!("dev.time_left", secs = secs).to_string() }
}

/// `1 min 31 s`, or `47 s`.
fn took_text(took: Duration) -> String {
    let secs = took.as_secs();
    if secs < 60 {
        t!("dev.dur_s", s = secs).to_string()
    } else {
        t!("dev.dur_min", m = secs / 60, s = secs % 60).to_string()
    }
}

/// A port's short name — `COM5`, `ttyACM0`, `cu.usbmodem14101` — where
/// the whole path would crowd a tab or a sentence.
fn short(port: &str) -> String {
    listen::label_for(port)
}

impl Screen for Page {
    type Act = Act;

    fn ui(&mut self) -> &mut Surface<Act> {
        &mut self.ui
    }

    fn pump(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(event) => self.apply(event),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    if !self.released {
                        self.worker_gone();
                    }
                    return;
                }
            }
        }
    }

    /// A passing note goes once its time is up.
    fn tick(&mut self) {
        if self.note.as_ref().and_then(|n| n.until).is_some_and(|until| Instant::now() >= until) {
            self.note = None;
        }
    }

    fn finished(&self) -> Option<Outcome> {
        (self.quitting && (self.released || !self.touched)).then_some(Outcome::Quit)
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn render_hosted(&mut self, frame: &mut Frame, area: Rect) {
        render_hosted(frame, self, area)
    }

    fn hint(&self) -> String {
        self.hint_line(true, HINT_W)
    }

    fn modal_open(&self) -> bool {
        self.gate.is_some()
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        // The gate is a modal: its keys and nothing else (the kit's gates:
        // Enter, Esc and n keep, y writes).
        if self.gate.is_some() {
            let act = match key.code {
                KeyCode::Char('y') => Some(Act::Write),
                KeyCode::Char('e') => Some(Act::Erase),
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('n') => Some(Act::Keep),
                _ => None,
            };
            return act.and_then(|act| self.act(act));
        }
        let act = match key.code {
            KeyCode::Esc => {
                self.leave();
                None
            }
            KeyCode::Enter => Some(Act::Primary),
            KeyCode::Char('d' | 'l') => Some(Act::Details),
            KeyCode::Char('w') => Some(Act::WriteAgain),
            KeyCode::Char('c') => Some(Act::Count),
            KeyCode::Char('s') => Some(Act::Show),
            KeyCode::Char('a') => Some(Act::UpdateAll),
            KeyCode::Char('r') => Some(Act::Ask),
            KeyCode::Left => {
                self.switch(-1);
                None
            }
            KeyCode::Right => {
                self.switch(1);
                None
            }
            _ => None,
        };
        act.and_then(|act| self.act(act))
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        Page::act(self, act)
    }

    /// Over the log, the wheel scrolls it (up stops the follow, back down
    /// to the newest line resumes it).
    fn wheel(&mut self, up: bool, at: Position) {
        let Some(rect) = self.log_rect.filter(|r| r.contains(at)) else { return };
        let lines = self.open.as_deref().map_or(0, |port| self.log_for(port).len());
        let most = lines.saturating_sub(usize::from(rect.height));
        self.log_scroll = if up { (self.log_scroll + 1).min(most) } else { self.log_scroll.saturating_sub(1) };
    }
}

// ── Glyphs ──────────────────────────────────────────────────────────────────

/// The page's marks, drawn beside the words and never inside them: on the
/// bare Windows console, which draws a missing glyph as a question mark,
/// their ASCII stand-ins (the kit's rule, docs/ui-kit.md "Glyphs").
struct Glyphs {
    ok: &'static str,
    warn: &'static str,
    no: &'static str,
    unknown: &'static str,
    forward: &'static str,
    open: &'static str,
    back: &'static str,
    arrow: &'static str,
    filled: &'static str,
    empty: &'static str,
    bullet: &'static str,
    switch: &'static str,
    busy: &'static str,
    rule: &'static str,
}

fn glyphs() -> Glyphs {
    if legacy_conhost() {
        Glyphs {
            ok: "+",
            warn: "!",
            no: "x",
            unknown: "?",
            forward: ">",
            open: "v",
            back: "<",
            arrow: "->",
            filled: "#",
            empty: "-",
            bullet: "*",
            switch: "<>",
            busy: "...",
            rule: "-",
        }
    } else {
        Glyphs {
            ok: "✓",
            warn: "!",
            no: "✗",
            unknown: "?",
            forward: "▸",
            open: "▾",
            back: "◂",
            arrow: "→",
            filled: "▰",
            empty: "▱",
            bullet: "•",
            switch: "←→",
            busy: "…",
            rule: "─",
        }
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, page: &mut Page) {
    page.ui.begin_frame();
    page.log_rect = None;
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    draw(frame, page, area, false);
}

/// The page inside another shell's `area` (the GUI's MP3 Player tab,
/// contract clauses 1 and 4): no ground, no header and no tips row — the
/// host's top bar and footer carry those — and the area's first row left
/// blank for the host. Nothing lands outside the area: the card stops
/// above the area's last row, which is the busy line's, and what does not
/// fit is cut inside the card's border rather than spilled
/// (docs/ui-kit.md, "Hosted rooms").
fn render_hosted(frame: &mut Frame, page: &mut Page, area: Rect) {
    page.ui.begin_frame();
    page.log_rect = None;
    // One row is the host's blank one and nothing more: not even the busy
    // line has a row of its own.
    if area.width == 0 || area.height < 2 {
        return;
    }
    draw(frame, page, area, true);
}

/// The page in `area`: on its own, under its header with the tips on the
/// last row; hosted, under a blank row with only the busy line below it.
fn draw(frame: &mut Frame, page: &mut Page, area: Rect, hosted: bool) {
    if !hosted {
        let right = match &page.image {
            Some(image) => {
                let word = t!(match image.kind {
                    Origin::Release => "dev.kind_release",
                    Origin::Cached => "dev.kind_cached",
                    Origin::File => "dev.kind_file",
                });
                t!("dev.head_firmware", version = image.version, kind = word).to_string()
            }
            None => String::new(),
        };
        draw_header_as(frame, area, &t!("dev.title"), &right);
    }
    let width = area.width.saturating_sub(4).min(CARD_W);
    let x = area.x + (area.width - width) / 2;
    let top = area.y + if hosted { 1 } else { 2 };
    // The first row the card may not reach: the busy line's (and on its
    // own, the tips' under it).
    let floor = if hosted { area.bottom() - 1 } else { area.bottom().saturating_sub(2) };
    // A gate makes the card beneath inert (the kit's modal rule): drawn
    // with no pointer, its clicks and tips dropped before the gate draws.
    let pointer = if page.gate.is_some() { page.ui.pointer.take() } else { None };
    let mut y = top;
    if page.boards.len() >= 2 && y < floor {
        tab_row(frame, page, Rect { x, y, width, height: 1 });
        y += 2;
    }
    if y < floor {
        let room = Rect { x, y, width, height: floor - y };
        match page.shown().cloned() {
            Some(board) => card(frame, page, &board, room),
            None => empty_card(frame, page, room),
        }
    }
    let busy = page.busy();
    let note = page.note.as_ref().map(|n| (n.text.clone(), n.gold));
    let tips = page.hint_line(hosted, usize::from(area.width.saturating_sub(4)));
    draw_foot(frame, area, note.as_ref(), busy.as_deref(), &tips, hosted);
    if page.gate.is_some() {
        page.ui.clear_registries();
        page.ui.pointer = pointer;
        draw_gate(frame, page, area);
    }
    if let Some((target, text)) = page.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// What the GUI's MP3 Player tab draws in `area` while the page it was
/// left with lets its ports go and the next waits to be built (contract
/// clause 5): `words`, dim, on the page's first row under the host's
/// blank one.
pub(crate) fn draw_waiting(frame: &mut Frame, area: Rect, words: &str) {
    if area.width == 0 || area.height < 3 {
        return;
    }
    let width = area.width.saturating_sub(4).min(CARD_W);
    let x = area.x + (area.width - width) / 2;
    let rect = Rect { x, y: area.y + 1, width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(words.to_string(), dim())), rect);
}

// ── The port tabs ───────────────────────────────────────────────────────────

/// A board's mark on its tab: ✓ green, ! and ✗ gold, ? and … dim, a
/// write's percentage in the accent, `in use` dim.
fn mark(page: &Page, board: &Board) -> (String, Color) {
    let g = glyphs();
    if let Work::Writing { pct, .. } = board.work {
        return (pct.map_or_else(|| g.busy.to_string(), |p| format!("{p}%")), th().accent);
    }
    if page.written_now(board) {
        return (g.busy.to_string(), th().accent);
    }
    if matches!(board.work, Work::Queued | Work::Reading) || board.verdict == Verdict::Asking {
        return (g.busy.to_string(), th().dim);
    }
    if matches!(board.written, Some(Written::Failed { .. })) {
        return (g.no.to_string(), th().gold);
    }
    match board.verdict {
        Verdict::UpToDate => (g.ok.to_string(), th().ok),
        Verdict::Update | Verdict::DevUpdate | Verdict::Newer => (g.warn.to_string(), th().gold),
        Verdict::InUse => (t!("dev.tab_in_use").to_string(), th().dim),
        Verdict::Other { .. }
        | Verdict::Blank
        | Verdict::NotCore2 { .. }
        | Verdict::HalfWritten
        | Verdict::Unreadable => (g.no.to_string(), th().gold),
        Verdict::Silent | Verdict::Unplaced | Verdict::Asking => (g.unknown.to_string(), th().dim),
    }
}

/// The kit's tab row of ports (contract clause 25): the open one the slab,
/// the others dim text that brightens under the pointer, each wearing its
/// board's mark; the count, or Update all, at the right edge.
fn tab_row(frame: &mut Frame, page: &mut Page, row: Rect) {
    let mut x = row.x;
    let boards = page.boards.clone();
    for (i, board) in boards.iter().enumerate() {
        let name = short(board.port());
        let (mark, color) = mark(page, board);
        let label = format!(" {name} {mark} ");
        let cells = kit::width(&label) as u16;
        if x + cells > row.right() {
            break;
        }
        let rect = Rect { x, y: row.y, width: cells, height: 1 };
        let open = page.open.as_deref() == Some(board.port());
        let line = if open {
            let slab = Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD);
            Line::from(Span::styled(label, slab))
        } else {
            let name_style = if page.ui.hovers(rect) { hover() } else { dim() };
            Line::from(vec![
                Span::styled(format!(" {name} "), name_style),
                Span::styled(mark, Style::default().fg(color)),
                Span::raw(" "),
            ])
        };
        frame.render_widget(Paragraph::new(line), rect);
        page.ui.click(rect, Act::Board(i));
        x = rect.right() + 1;
    }
    let (text, style, act) = tabs_right(page);
    let cells = kit::width(&text) as u16;
    if text.is_empty() || x + 2 + cells > row.right() {
        return;
    }
    let rect = Rect { x: row.right() - cells, y: row.y, width: cells, height: 1 };
    let style = match &act {
        Some(_) if page.ui.hovers(rect) => hover(),
        _ => style,
    };
    frame.render_widget(Paragraph::new(Span::styled(text, style)), rect);
    if let Some(act) = act {
        page.ui.click(rect, act);
    }
}

/// The tab row's right edge: Update all as it goes, the board being
/// written, Update all to offer, C's count of the boards that need an
/// update, or what is plugged in.
fn tabs_right(page: &Page) -> (String, Style, Option<Act>) {
    match &page.lock {
        Some(Lock::All { ports, at, .. }) => {
            let text = t!("dev.tabs_all_at", i = (at + 1).min(ports.len()), n = ports.len());
            return (text.to_string(), dim(), None);
        }
        Some(Lock::One { port, .. }) => {
            return (t!("dev.tabs_writing", port = short(port)).to_string(), accent(), None);
        }
        None => {}
    }
    let needing = page.needing().len();
    if page.can_update_all() {
        return (t!("dev.tabs_all", n = needing).to_string(), dim(), Some(Act::UpdateAll));
    }
    let n = page.boards.len();
    if needing == 1 {
        return (t!("dev.tabs_needs_one", n = n).to_string(), Style::default().fg(th().gold), None);
    }
    // Each board in one place: in use, being asked, a player (it answered
    // as mStream firmware), or another board.
    let (mut players, mut others, mut in_use) = (0, 0, 0);
    let mut asking: Vec<&Board> = Vec::new();
    for board in &page.boards {
        match board.verdict {
            Verdict::InUse => in_use += 1,
            Verdict::Asking => asking.push(board),
            _ if board.ours() => players += 1,
            _ => others += 1,
        }
    }
    if players == n {
        return (t!("dev.tabs_plugged", n = n).to_string(), dim(), None);
    }
    let mut parts = Vec::new();
    if players > 0 {
        let words = if players == 1 { t!("dev.tabs_player_one") } else { t!("dev.tabs_players", n = players) };
        parts.push(words.to_string());
    }
    if others > 0 {
        let words = if others == 1 { t!("dev.tabs_other_one") } else { t!("dev.tabs_others", n = others) };
        parts.push(words.to_string());
    }
    if in_use > 0 {
        parts.push(t!("dev.tabs_in_use", n = in_use).to_string());
    }
    if let Some(board) = asking.first() {
        parts.push(t!("dev.tabs_asking", port = short(board.port())).to_string());
    }
    (parts.join(" · "), dim(), None)
}

// ── The card ────────────────────────────────────────────────────────────────

/// One row of the card, laid out before it is drawn so the card knows its
/// height: every row is one cell high but the actions (three with a
/// primary) and the log (whatever is left).
enum Row {
    Blank,
    /// Across the content: a line at the left and one at the right, the
    /// right dropped where it would not fit.
    Wide { left: Line<'static>, right: Option<Line<'static>> },
    /// The label column (on a row's first line), then the value column.
    Value { label: Option<String>, left: Line<'static>, right: Option<Line<'static>> },
    /// A █░ bar across the value column: `permille` filled, in `color`.
    Bar { label: Option<String>, permille: u32, color: Color },
    /// A dim text button, in the value column or across the content, with
    /// a dim note after it.
    Link { wide: bool, text: String, act: Act, note: Option<String> },
    /// The bottom row: text buttons at the left, the primary at the right
    /// (enabled, or the kit's disabled frame with why in a tooltip).
    Actions { links: Vec<(String, Act)>, primary: Option<(String, bool, Option<String>)> },
    /// Details' dim rule.
    Rule,
    /// A Details fact: its label, its value cut to the column.
    Fact { label: String, value: String, style: Style },
    /// The log: a blank row and the newest lines, in the rows left.
    Log,
}

impl Row {
    /// The rows it takes; the log's are the card's to give.
    fn height(&self) -> u16 {
        match self {
            Row::Actions { primary: Some(_), .. } => BUTTON_H,
            Row::Log => 0,
            _ => 1,
        }
    }
}

/// `pieces` word-wrapped in `width` cells, each word in its piece's style;
/// a word wider than the line (a Japanese or Chinese sentence, which has
/// no spaces) breaks at the cell. Pieces with no space between them make
/// one word.
fn wrap_spans(pieces: &[(String, Style)], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    // Words, each a run of (text, style).
    let mut words: Vec<Vec<(String, Style)>> = Vec::new();
    let mut in_word = false;
    for (text, style) in pieces {
        for c in text.chars() {
            if c == ' ' {
                in_word = false;
                continue;
            }
            if !in_word {
                words.push(Vec::new());
                in_word = true;
            }
            let runs = words.last_mut().expect("a word");
            match runs.last_mut() {
                Some((run, s)) if s == style => run.push(c),
                _ => runs.push((c.to_string(), *style)),
            }
        }
    }
    let mut lines: Vec<Vec<(String, Style)>> = vec![Vec::new()];
    let mut used = 0usize;
    for runs in words {
        let cells: usize = runs.iter().map(|(t, _)| kit::width(t)).sum();
        // Japanese and Chinese break between any two characters: a word of
        // theirs fills the line it starts on rather than leave it short.
        let wide = runs.iter().any(|(t, _)| t.chars().any(|c| kit::width(c.encode_utf8(&mut [0u8; 4])) > 1));
        if used > 0 && used + 1 + cells > width && !(wide && used + 3 < width) {
            lines.push(Vec::new());
            used = 0;
        } else if used > 0 {
            lines.last_mut().expect("a line").push((" ".to_string(), Style::default()));
            used += 1;
        }
        for (text, style) in runs {
            for c in text.chars() {
                let w = kit::width(c.encode_utf8(&mut [0u8; 4]));
                if used + w > width && used > 0 {
                    lines.push(Vec::new());
                    used = 0;
                }
                let line = lines.last_mut().expect("a line");
                match line.last_mut() {
                    Some((run, s)) if *s == style => run.push(c),
                    _ => line.push((c.to_string(), style)),
                }
                used += w;
            }
        }
    }
    lines
        .into_iter()
        .filter(|l| !l.is_empty())
        .map(|l| Line::from(l.into_iter().map(|(t, s)| Span::styled(t, s)).collect::<Vec<_>>()))
        .collect()
}

/// Value rows for `pieces` wrapped in the value column, the label on the
/// first.
fn value_rows(label: Option<String>, pieces: &[(String, Style)], width: u16) -> Vec<Row> {
    wrap_spans(pieces, usize::from(width))
        .into_iter()
        .enumerate()
        .map(|(i, line)| Row::Value { label: if i == 0 { label.clone() } else { None }, left: line, right: None })
        .collect()
}

/// Dim value rows, under a label or not.
fn dim_rows(label: Option<String>, text: &str, width: u16) -> Vec<Row> {
    value_rows(label, &[(text.to_string(), dim())], width)
}

/// Wide rows for `pieces` across the content.
fn wide_rows(pieces: &[(String, Style)], width: u16) -> Vec<Row> {
    wrap_spans(pieces, usize::from(width)).into_iter().map(|left| Row::Wide { left, right: None }).collect()
}

/// The chip: the glyph and the verdict's first words in its colour, the
/// words bold, and the facts after ` · ` plain.
fn chip(glyph: &str, color: Color, words: &str, facts: &[String]) -> Vec<(String, Style)> {
    let tone = Style::default().fg(color);
    let mut pieces = vec![(format!("{glyph} "), tone), (words.to_string(), tone.add_modifier(Modifier::BOLD))];
    for fact in facts {
        pieces.push((format!(" · {fact}"), Style::default()));
    }
    pieces
}

/// The kit's scan widget on one line: ten cells (▰ green filled to the
/// percent, ▱ dim; all dim with no estimate), the percent dim, then the
/// present-progressive words in the accent.
fn scan(pct: Option<u8>, words: &str) -> Vec<(String, Style)> {
    let g = glyphs();
    let filled = pct.map_or(0, |p| usize::from(p.min(100)) / 10);
    let mut pieces = Vec::new();
    if filled > 0 {
        pieces.push((g.filled.repeat(filled), Style::default().fg(th().ok)));
    }
    let mut empty = g.empty.repeat(10 - filled);
    if let Some(p) = pct {
        empty.push_str(&format!(" {p}%"));
    }
    pieces.push((empty, dim()));
    pieces.push((format!(" {words}"), accent()));
    pieces
}

/// The board's name on the card's head (contract clause 18).
fn board_name(board: &Board) -> String {
    if board.ours() {
        return t!("dev.name_core2").to_string();
    }
    match (&board.verdict, &board.probe) {
        (Verdict::NotCore2 { .. }, Some(Err(DeviceError::WrongFlash { .. }))) => t!("dev.name_esp32").to_string(),
        (Verdict::NotCore2 { .. }, _) | (Verdict::Silent, _) => t!("dev.name_unknown").to_string(),
        (Verdict::InUse | Verdict::Unreadable, _) => t!("dev.name_on_port", port = short(board.port())).to_string(),
        _ => t!("dev.name_core2").to_string(),
    }
}

/// The head's right edge: the port with one board; with several (the port
/// is on the tab) the USB serial, the bridge before it for a board that
/// has not answered as ours.
fn head_right(page: &Page, board: &Board) -> String {
    if page.boards.len() < 2 {
        return board.port().to_string();
    }
    let bridge = board.candidate.bridge;
    match (board.serial(), board.ours()) {
        (Some(serial), true) => t!("dev.serial", serial = serial).to_string(),
        (Some(serial), false) => format!("{bridge} · {}", t!("dev.serial", serial = serial)),
        (None, _) => bridge.to_string(),
    }
}

/// The identity line with several boards: what a person can check on the
/// board itself — its battery, its headphones (contract clause 18).
fn identity(page: &Page, board: &Board) -> Option<String> {
    if page.boards.len() < 2 {
        return None;
    }
    let status = board.status()?;
    let mut parts = Vec::new();
    if let Some(bat) = status.bat {
        parts.push(t!("dev.id_battery", pct = bat).to_string());
    }
    parts.push(match &status.bt {
        Some(name) => t!("dev.id_paired", name = crate::admin::printable(name, 40)).to_string(),
        None => t!("dev.id_unpaired").to_string(),
    });
    Some(parts.join(" · "))
}

/// The Firmware row (contract clause 19): the chip and what follows from
/// it, the write itself while it runs, or the board being asked or read.
fn firmware_rows(page: &Page, board: &Board, w: u16) -> Vec<Row> {
    let g = glyphs();
    let label = Some(t!("dev.label_firmware").to_string());
    let gold = th().gold;
    let mut rows = Vec::new();
    let to = page.target.as_ref().map(|t| t.version.clone());
    let pin = to.clone().unwrap_or_else(|| "?".to_string());
    let version = board.version().unwrap_or("?").to_string();
    // The write under way, on this board: its words, then the bar.
    if page.written_now(board) {
        rows.extend(value_rows(label, &[(page.write_words(board), accent())], w));
        let permille = match board.work {
            Work::Writing { phase: Phase::Writing, pct } => u32::from(pct.unwrap_or(0).min(100)) * 10,
            Work::Writing { phase: Phase::Verifying | Phase::Restarting, .. } => 1000,
            _ => 0,
        };
        rows.push(Row::Bar { label: None, permille, color: th().accent });
        return rows;
    }
    if board.work == Work::Reading {
        rows.extend(value_rows(label, &scan(None, &t!("dev.fw_reading")), w));
        rows.extend(dim_rows(None, &t!("dev.fw_reading_2"), w));
        return rows;
    }
    let asking = board.verdict == Verdict::Asking || (to.is_none() && board.verdict == Verdict::Unplaced);
    if asking {
        rows.extend(value_rows(label, &scan(None, &t!("dev.fw_asking")), w));
        rows.extend(dim_rows(None, &t!("dev.fw_asking_2"), w));
        return rows;
    }
    let mut second: Vec<String> = Vec::new();
    let pieces = if let Some(Written::Failed { error, half, .. }) = &board.written {
        if *half {
            second.push(t!("dev.fw_half_2").to_string());
            chip(g.no, gold, &t!("dev.fw_failed"), &[t!("dev.fw_half").to_string()])
        } else {
            second.push(error.text());
            chip(g.no, gold, &t!("dev.fw_failed"), &[t!("dev.fw_not_written").to_string()])
        }
    } else {
        match &board.verdict {
            Verdict::UpToDate => {
                let mut facts = vec![version.clone()];
                match &board.written {
                    Some(Written::Done { install: true, .. }) => facts.push(t!("dev.fw_just_installed").to_string()),
                    Some(Written::Done { .. }) => facts.push(t!("dev.fw_just_written").to_string()),
                    _ => {}
                }
                chip(g.ok, th().ok, &t!("dev.fw_up_to_date"), &facts)
            }
            Verdict::Update => chip(g.warn, gold, &t!("dev.fw_update"), &[format!("{version} {} {pin}", g.arrow)]),
            Verdict::DevUpdate => chip(g.warn, gold, &t!("dev.fw_dev"), &[format!("{version} {} {pin}", g.arrow)]),
            Verdict::Newer => {
                second.push(t!("dev.fw_newer_2", pin = pin).to_string());
                chip(g.warn, gold, &t!("dev.fw_newer"), std::slice::from_ref(&version))
            }
            Verdict::Unplaced if board.version().is_some() => {
                second.push(t!("dev.fw_unplaced_2", pin = pin).to_string());
                chip(g.unknown, th().dim, &t!("dev.fw_unplaced"), std::slice::from_ref(&version))
            }
            Verdict::Unplaced => {
                second.push(t!("dev.fw_unknown_version_2").to_string());
                chip(g.unknown, th().dim, &t!("dev.fw_unknown_version"), &[])
            }
            Verdict::Other { name } => chip(g.no, gold, &t!("dev.fw_other"), std::slice::from_ref(name)),
            Verdict::Blank => chip(g.no, gold, &t!("dev.fw_blank"), &[]),
            Verdict::Silent => {
                second.push(t!("dev.fw_silent_2").to_string());
                second.push(t!("dev.fw_read_cost").to_string());
                chip(g.unknown, th().dim, &t!("dev.fw_silent"), &[t!("dev.fw_silent_maybe").to_string()])
            }
            Verdict::NotCore2 { found } => {
                second.push(t!("dev.fw_not_core2_2").to_string());
                let label = Some(t!("dev.board").to_string());
                let pieces = chip(g.no, gold, &t!("dev.fw_not_core2"), std::slice::from_ref(found));
                rows.extend(value_rows(label, &pieces, w));
                for line in &second {
                    rows.extend(dim_rows(None, line, w));
                }
                return rows;
            }
            Verdict::InUse => {
                second.push(t!("dev.fw_in_use_2").to_string());
                second.push(t!("dev.fw_in_use_3").to_string());
                chip(g.warn, gold, &t!("dev.fw_in_use"), &[])
            }
            Verdict::Unreadable => {
                let why = match (&board.heard, &board.probe) {
                    (Heard::Failed(e), _) | (_, Some(Err(e))) => e.text(),
                    _ => String::new(),
                };
                if !why.is_empty() {
                    second.push(why);
                }
                chip(g.no, gold, &t!("dev.fw_unreadable"), &[])
            }
            // Folded above: a half-written board carries its failed write.
            Verdict::HalfWritten => {
                second.push(t!("dev.fw_half_2").to_string());
                chip(g.no, gold, &t!("dev.fw_failed"), &[t!("dev.fw_half").to_string()])
            }
            Verdict::Asking => Vec::new(),
        }
    };
    rows.extend(value_rows(label, &pieces, w));
    for line in &second {
        rows.extend(dim_rows(None, line, w));
    }
    // Look, don't write: a board whose primary would act waits its turn.
    if let Some(port) = page.writing_port()
        && port != board.port()
        && board.primary().is_some()
    {
        rows.extend(dim_rows(None, &t!("dev.fw_one_at_a_time", port = short(port)), w));
    }
    rows
}

/// `1,284 tracks`, `1 track`, `no tracks`, or being indexed.
fn tracks_words(tracks: &Tracks) -> Option<String> {
    Some(match tracks {
        Tracks::Count(0) => t!("dev.card_no_tracks").to_string(),
        Tracks::Count(1) => t!("dev.card_track_one").to_string(),
        Tracks::Count(n) => t!("dev.card_tracks", n = fmt_count(*n)).to_string(),
        Tracks::Building => t!("dev.card_tracks_building").to_string(),
        Tracks::NoCard | Tracks::Unknown => return None,
    })
}

/// The SD card row (contract clause 20): only what the running firmware
/// just reported.
fn card_rows(page: &Page, board: &Board, w: u16) -> Vec<Row> {
    let g = glyphs();
    let label = Some(t!("dev.sd_card").to_string());
    let gold = Style::default().fg(th().gold);
    let problem = |words: String, why: Option<String>, fix: String| -> Vec<Row> {
        let mut pieces = vec![(format!("{} ", g.warn), gold), (words, gold.add_modifier(Modifier::BOLD))];
        if let Some(why) = why {
            pieces.push((format!(" · {why}"), gold));
        }
        let mut rows = value_rows(label.clone(), &pieces, w);
        rows.extend(dim_rows(None, &fix, w));
        rows
    };
    let unknown = |words: String| -> Vec<Row> { dim_rows(label.clone(), &format!("{} {words}", g.unknown), w) };
    // Nothing can read the card while its board is written.
    let card = if page.written_now(board) { Card::Unknown(CardUnknown::Writing) } else { board.card() };
    match card {
        Card::Unknown(why) => match why {
            CardUnknown::Asking => dim_rows(label, &t!("dev.card_asking"), w),
            CardUnknown::Writing => dim_rows(label, &t!("dev.card_untouched"), w),
            CardUnknown::HalfWritten | CardUnknown::NotRunning => unknown(t!("dev.card_unknown_runs").to_string()),
            CardUnknown::OldFirmware => {
                let mut words = match board.version() {
                    Some(version) => t!("dev.card_old", version = version).to_string(),
                    None => t!("dev.card_old_this").to_string(),
                };
                // Only a pin that answers the status query can keep this
                // promise; v0.8.0 does not (firmware.rs, PINNED_ANSWERS_STATUS).
                if page.target.as_ref().is_some_and(|t| t.answers_status) {
                    words.push(' ');
                    words.push_str(&t!("dev.card_old_update"));
                }
                unknown(words)
            }
            CardUnknown::NotOurs => unknown(t!("dev.card_not_ours").to_string()),
            CardUnknown::InUse => unknown(t!("dev.card_in_use").to_string()),
            CardUnknown::Unreadable => unknown(t!("dev.card_unreadable_board").to_string()),
        },
        Card::Empty => problem(t!("dev.card_none").to_string(), None, t!("dev.card_put_in").to_string()),
        Card::Foreign { kind, .. } => {
            let (words, why, fix) = match kind {
                CardKind::ExFat => ("dev.card_exfat", Some("dev.card_fat32_only"), "dev.card_fix_format"),
                CardKind::Ntfs => ("dev.card_ntfs", Some("dev.card_fat32_only"), "dev.card_fix_format"),
                CardKind::Gpt => ("dev.card_gpt", Some("dev.card_mbr_only"), "dev.card_fix_mbr"),
                _ => ("dev.card_unreadable", None, "dev.card_try_computer"),
            };
            problem(t!(words).to_string(), why.map(|k| t!(k).to_string()), t!(fix).to_string())
        }
        Card::Fat { size, free, tracks, .. } => {
            let tracks_text = tracks_words(&tracks);
            let size_text = size.map(|s| t!("dev.card_size", size = gb(s)).to_string());
            let mut rows = Vec::new();
            match (&free, size) {
                (Free::Bytes(free), Some(size)) => {
                    let card = board.card();
                    let full = card.nearly_full();
                    let used = card.used().unwrap_or_else(|| size.saturating_sub(*free));
                    let mut permille = (u128::from(used) * 1000 / u128::from(size.max(1))) as u32;
                    // A sliver used still shows a cell; a card not quite full
                    // never shows a full bar.
                    if used > 0 {
                        permille = permille.max(1);
                    }
                    let color = if full { th().gold } else { th().accent };
                    rows.push(Row::Bar { label: label.clone(), permille, color });
                    let mut left = t!("dev.card_used", used = gb(used), size = gb(size)).to_string();
                    if let Some(tracks) = &tracks_text {
                        left.push_str(&format!(" · {tracks}"));
                    }
                    let free_text = t!("dev.card_free", free = gb(*free)).to_string();
                    let right = if full {
                        Line::from(Span::styled(format!("{} {free_text}", g.warn), Style::default().fg(th().gold)))
                    } else {
                        Line::from(Span::raw(free_text))
                    };
                    rows.push(Row::Value { label: None, left: Line::from(left), right: Some(right) });
                }
                (Free::Bytes(free), None) => {
                    let mut left = t!("dev.card_free", free = gb(*free)).to_string();
                    if let Some(tracks) = &tracks_text {
                        left.push_str(&format!(" · {tracks}"));
                    }
                    rows.extend(value_rows(label.clone(), &[(left, Style::default())], w));
                }
                (Free::NotCounted, _) => {
                    let mut words: Vec<String> = size_text.iter().cloned().collect();
                    words.extend(tracks_text.clone());
                    words.push(t!("dev.card_not_counted").to_string());
                    rows.extend(value_rows(label.clone(), &[(words.join(" · "), Style::default())], w));
                    if let Count::Refused { why, .. } = &board.count {
                        let (text, style) = match why {
                            CountWhy::Playing => (t!("dev.count_playing"), gold),
                            CountWhy::Library => (t!("dev.count_library"), dim()),
                            _ => (t!("dev.count_failed"), dim()),
                        };
                        rows.extend(value_rows(None, &[(text.to_string(), style)], w));
                    }
                    if page.can_count(board) {
                        let note = Some(t!("dev.link_count_note").to_string());
                        let text = t!("dev.link_count").to_string();
                        rows.push(Row::Link { wide: false, text, act: Act::Count, note });
                    }
                }
                (Free::Counting { pct }, _) => {
                    rows.extend(value_rows(label.clone(), &scan(*pct, &t!("dev.card_counting")), w));
                    let mut words: Vec<String> = size_text.iter().cloned().collect();
                    words.extend(tracks_text.clone());
                    if !words.is_empty() {
                        rows.extend(value_rows(None, &[(words.join(" · "), Style::default())], w));
                    }
                }
            }
            // No tracks: where the music goes is the next step.
            if matches!(tracks, Tracks::Count(0)) {
                rows.extend(dim_rows(None, &t!("dev.card_music_hint"), w));
            }
            rows
        }
    }
}

/// Details (contract clause 18): the facts, the log, Write again.
fn details_rows(page: &Page, board: &Board) -> Vec<Row> {
    let mut rows = vec![Row::Rule];
    let fact = |label: &str, value: String| {
        Row::Fact { label: t!(label).to_string(), value, style: Style::default() }
    };
    let mut port = format!("{} · {}", board.port(), board.candidate.bridge);
    if let Some(serial) = board.serial() {
        port.push_str(&format!(" · {}", t!("dev.serial", serial = serial)));
    }
    match &board.probe {
        Some(Ok(probe)) => port.push_str(&format!(" · {} baud", probe.info.baud)),
        _ if matches!(board.heard, Heard::Status(_) | Heard::Old { .. } | Heard::Silent) => {
            port.push_str(&format!(" · {} baud · {}", engine::CONSOLE_BAUD, t!("dev.no_reset")));
        }
        _ => {}
    }
    rows.push(fact("dev.port", port));
    // A failed write: the log's last lines are what Details opens on.
    if !matches!(board.written, Some(Written::Failed { .. })) {
        let name = board_name(board);
        let flash = match &board.probe {
            Some(Ok(probe)) => {
                let rev = probe.info.revision.map(|(a, b)| format!("{a}.{b}")).unwrap_or_else(|| "?".to_string());
                let flash = probe.info.flash_mb.or(board.flash_mb);
                let flash = flash.map_or_else(|| "?".to_string(), |mb| mb.to_string());
                let chip = format!("{} rev {rev}", probe.info.chip);
                Some(format!("{name} · {chip} · {}", t!("dev.fact_flash", flash = flash)))
            }
            _ => board.flash_mb.map(|mb| format!("{name} · {}", t!("dev.fact_flash", flash = mb))),
        };
        rows.push(fact("dev.board", flash.unwrap_or(name)));
        let on_board = match (&board.verdict, board.version()) {
            (Verdict::Other { name }, _) => t!("dev.on_board_other", name = name).to_string(),
            (Verdict::Blank, _) => t!("dev.on_board_unknown").to_string(),
            (_, Some(version)) => {
                let mut text = t!("dev.on_board_ours", version = version).to_string();
                if let Some(elf) = board.elf().filter(|e| !e.is_empty()) {
                    text.push_str(&format!(" · ELF {elf}"));
                }
                text
            }
            (_, None) => t!("dev.fact_unknown").to_string(),
        };
        rows.push(fact("dev.on_board", on_board));
        let carries = t!("dev.fact_carries", version = page.target.as_ref().map_or("?", |t| t.version.as_str()));
        let (player, style) = match (&page.image, &page.image_failed) {
            (Some(image), _) => (format!("{carries} · {}", image.origin), Style::default()),
            (None, Some(why)) => (why.clone(), Style::default().fg(th().gold)),
            (None, None) => {
                let mut getting = format!("{carries} · {}", t!("dev.fact_getting"));
                if let Some((done, Some(total))) = page.download.filter(|(_, t)| t.is_some_and(|t| t > 0)) {
                    getting.push_str(&format!(" {}%", (done * 100 / total).min(100)));
                }
                (getting, Style::default())
            }
        };
        rows.push(Row::Fact { label: t!("dev.label_player").to_string(), value: player, style });
        if let Some(status) = board.status() {
            let kind = match &status.card {
                CardKind::None => None,
                CardKind::Fat32 => Some("FAT32".to_string()),
                CardKind::Fat16 => Some("FAT16".to_string()),
                CardKind::ExFat => Some("exFAT".to_string()),
                CardKind::Ntfs => Some("NTFS".to_string()),
                CardKind::Gpt => Some("GPT".to_string()),
                CardKind::Other | CardKind::Unreadable => Some(t!("dev.card_unreadable").to_string()),
                CardKind::Unknown(word) => Some(word.clone()),
            };
            let mut card = match kind {
                Some(kind) => vec![kind],
                None => vec![t!("dev.card_none").to_string()],
            };
            if let Some(size) = status.size {
                card.push(t!("dev.fact_bytes", n = fmt_count(size)).to_string());
            }
            match (&board.count, &status.free) {
                (Count::Done { .. }, _) => card.push(t!("dev.fact_free_counted").to_string()),
                (_, Free::Bytes(_)) => card.push(t!("dev.fact_free_fsinfo").to_string()),
                (_, Free::NotCounted) if status.card.readable() => card.push(t!("dev.card_not_counted").to_string()),
                _ => {}
            }
            rows.push(fact("dev.label_card_fact", card.join(" · ")));
        }
    }
    rows.push(Row::Log);
    if page.can_write_again(board) {
        let to = page.target.as_ref().map_or("?", |t| t.version.as_str());
        rows.push(Row::Blank);
        let text = t!("dev.link_write", version = to).to_string();
        rows.push(Row::Link { wide: true, text, act: Act::WriteAgain, note: None });
    }
    rows
}

/// Every row of `board`'s card, top to bottom.
fn card_layout(page: &Page, board: &Board, content_w: u16) -> Vec<Row> {
    let value_w = content_w.saturating_sub(LABEL_W);
    let mut rows = vec![Row::Blank];
    let right = Line::from(Span::styled(head_right(page, board), dim()));
    rows.push(Row::Wide { left: Line::from(Span::styled(board_name(board), bold())), right: Some(right) });
    if let Some(line) = identity(page, board) {
        rows.extend(wide_rows(&[(line, dim())], content_w));
    }
    rows.push(Row::Blank);
    rows.extend(firmware_rows(page, board, value_w));
    if !matches!(board.verdict, Verdict::NotCore2 { .. }) {
        rows.push(Row::Blank);
        rows.extend(card_rows(page, board, value_w));
    }
    rows.push(Row::Blank);
    let g = glyphs();
    let details = format!("{} {}", if page.details { g.open } else { g.forward }, t!("dev.details"));
    let mut links = vec![(details, Act::Details)];
    if page.can_show(board) {
        links.push((t!("dev.link_show").to_string(), Act::Show));
    }
    if page.offers_ask(board) {
        links.push((t!("dev.link_ask").to_string(), Act::Ask));
    }
    let primary = page.primary_of(board).map(|p| {
        let word = t!(match p {
            Primary::Update => "dev.btn_update",
            Primary::Install => "dev.btn_install",
            Primary::Read => "dev.btn_read",
            Primary::TryAgain => "dev.retry",
        });
        match page.writing_port() {
            // The kit's disabled frame: dim, no ▸, why in its tooltip.
            Some(port) => (word.to_string(), false, Some(t!("dev.fw_one_at_a_time", port = short(port)).to_string())),
            None => (format!("{word} {}", g.forward), true, None),
        }
    });
    rows.push(Row::Actions { links, primary });
    if page.details {
        rows.extend(details_rows(page, board));
    }
    rows.push(Row::Blank);
    rows
}

/// The card in `room`: its rows laid out, its height what they need (the
/// log taking the rows left), cut to the room inside its border.
fn card(frame: &mut Frame, page: &mut Page, board: &Board, room: Rect) {
    if room.height < 3 || room.width < CARD_INSET * 2 + 2 {
        return;
    }
    let content_w = room.width - inset(room.width) * 2;
    let rows = card_layout(page, board, content_w);
    draw_card(frame, page, board.port(), &rows, room);
}

/// The card's inset at its width: the border and the padding.
fn inset(width: u16) -> u16 {
    if width >= CARD_W { CARD_INSET } else { CARD_INSET - 1 }
}

/// The rows in a dim rounded card at the top of `room`.
fn draw_card(frame: &mut Frame, page: &mut Page, port: &str, rows: &[Row], room: Rect) {
    let content_w = room.width - inset(room.width) * 2;
    let fixed: u16 = rows.iter().map(Row::height).sum();
    let inside = room.height - 2;
    let lines = page.log_for(port).len() as u16;
    let has_log = rows.iter().any(|r| matches!(r, Row::Log));
    // The log takes what is left, under a blank row, and no more than it
    // has: the card is as tall as its rows.
    let spare = inside.saturating_sub(fixed);
    let log_rows = if has_log && spare >= 2 { (spare - 1).min(lines) } else { 0 };
    let height = (fixed + log_rows + u16::from(log_rows > 0)).min(inside);
    let rect = Rect { height: height + 2, ..room };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(dim());
    frame.render_widget(block, rect);
    let content = Rect { x: rect.x + inset(room.width), y: rect.y + 1, width: content_w, height };
    draw_rows(frame, page, port, rows, content, log_rows);
}

fn draw_rows(frame: &mut Frame, page: &mut Page, port: &str, rows: &[Row], content: Rect, log_rows: u16) {
    let label_w = LABEL_W.min(content.width);
    let value = |y: u16| Rect { x: content.x + label_w, y, width: content.width - label_w, height: 1 };
    let whole = |y: u16| Rect { x: content.x, y, width: content.width, height: 1 };
    let bottom = content.bottom();
    let mut y = content.y;
    for row in rows {
        if y >= bottom {
            break;
        }
        match row {
            Row::Blank => {}
            Row::Wide { left, right } => {
                frame.render_widget(Paragraph::new(left.clone()), whole(y));
                put_right(frame, whole(y), left, right.as_ref());
            }
            Row::Value { label, left, right } => {
                if let Some(label) = label {
                    let at = Rect { width: label_w, ..whole(y) };
                    frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), at);
                }
                frame.render_widget(Paragraph::new(left.clone()), value(y));
                put_right(frame, value(y), left, right.as_ref());
            }
            Row::Bar { label, permille, color } => {
                if let Some(label) = label {
                    let at = Rect { width: label_w, ..whole(y) };
                    frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), at);
                }
                bar(frame, value(y), *permille, *color);
            }
            Row::Link { wide, text, act, note } => {
                let at = if *wide { whole(y) } else { value(y) };
                let rect = text_button(frame, page, at, text, act.clone());
                if let Some(note) = note {
                    let x = rect.right() + 3;
                    if x < at.right() {
                        let rest = Rect { x, width: at.right() - x, ..at };
                        frame.render_widget(Paragraph::new(Span::styled(note.clone(), dim())), rest);
                    }
                }
            }
            Row::Actions { links, primary } => {
                let tall = primary.is_some();
                // Half a frame is not a button: short of rows, the primary
                // is not drawn and Enter still does what it would.
                let whole_button = tall && bottom - y >= BUTTON_H;
                let mid = if tall { y + 1 } else { y };
                if mid < bottom {
                    let mut x = content.x;
                    for (text, act) in links {
                        if x >= content.right() {
                            break;
                        }
                        let at = Rect { x, y: mid, width: content.right() - x, height: 1 };
                        let rect = text_button(frame, page, at, text, act.clone());
                        x = rect.right() + 3;
                    }
                }
                if let Some((label, enabled, tip)) = primary
                    && whole_button
                {
                    let w = kit::tall_width(label).min(content.width);
                    let at = Rect { x: content.right() - w, y, width: w, height: BUTTON_H };
                    let rect = kit::tall_button(frame, &mut page.ui, at, label, *enabled, Act::Primary);
                    if let Some(tip) = tip {
                        page.ui.tip(rect, tip.clone());
                    }
                }
                y += if tall { BUTTON_H } else { 1 };
                continue;
            }
            Row::Rule => {
                let rule = glyphs().rule.repeat(usize::from(content.width));
                frame.render_widget(Paragraph::new(Span::styled(rule, dim())), whole(y));
            }
            Row::Fact { label, value: text, style } => {
                let at = Rect { width: label_w, ..whole(y) };
                frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), at);
                let cut = fit(text, usize::from(content.width - label_w));
                frame.render_widget(Paragraph::new(Span::styled(cut, *style)), value(y));
            }
            Row::Log => {
                if log_rows > 0 {
                    let rows = log_rows.min(bottom.saturating_sub(y + 1));
                    if rows > 0 {
                        let pane = Rect { x: content.x, y: y + 1, width: content.width, height: rows };
                        draw_log(frame, page, port, pane);
                    }
                    y += log_rows + 1;
                }
                continue;
            }
        }
        y += 1;
    }
}

/// `right` at the right edge of `at`, where it clears what `left` drew.
fn put_right(frame: &mut Frame, at: Rect, left: &Line, right: Option<&Line>) {
    let Some(right) = right else { return };
    let (lw, rw) = (left.width() as u16, right.width() as u16);
    if lw + 2 + rw <= at.width {
        frame.render_widget(Paragraph::new(right.clone()).alignment(Alignment::Right), at);
    }
}

/// Under the pointer: bright and bold, the kit's hover.
fn hover() -> Style {
    Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
}

/// A dim text button in `at`: bright and bold under the pointer, its click
/// over its own cells. Returns its rect.
fn text_button(frame: &mut Frame, page: &mut Page, at: Rect, text: &str, act: Act) -> Rect {
    let rect = Rect { width: (kit::width(text) as u16).min(at.width), height: 1, ..at };
    let style = if page.ui.hovers(rect) { hover() } else { dim() };
    frame.render_widget(Paragraph::new(Span::styled(text.to_string(), style)), rect);
    page.ui.click(rect, act);
    rect
}

/// A █░ bar across `at`: `permille` of it filled in `color`, the rest a
/// dim track. Drawn by hand rather than with ratatui's Gauge, whose
/// remainder swaps the colours (the player's own reason).
fn bar(frame: &mut Frame, at: Rect, permille: u32, color: Color) {
    let cells = u32::from(at.width);
    let mut filled = (cells * permille.min(1000) + 500) / 1000;
    if permille > 0 {
        filled = filled.max(1);
    }
    if permille < 1000 {
        filled = filled.min(cells.saturating_sub(1));
    }
    let filled = filled as u16;
    frame.render_widget(Paragraph::new(Span::styled("░".repeat(usize::from(at.width)), dim())), at);
    if filled > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled("█".repeat(usize::from(filled)), Style::default().fg(color))),
            Rect { width: filled, ..at },
        );
    }
}

/// The log in `pane`: the newest lines about the board that fit (`mm:ss`,
/// then the line in its kind's colour), scrolled up by the wheel.
fn draw_log(frame: &mut Frame, page: &mut Page, port: &str, pane: Rect) {
    page.log_rect = Some(pane);
    let lines: Vec<LogLine> = page.log_for(port).into_iter().cloned().collect();
    let rows = usize::from(pane.height);
    let scroll = page.log_scroll.min(lines.len().saturating_sub(rows));
    let end = lines.len() - scroll;
    let start = end.saturating_sub(rows);
    let width = usize::from(pane.width).saturating_sub(CLOCK_W);
    for (i, entry) in lines[start..end].iter().enumerate() {
        let secs = entry.at.as_secs();
        let clock = format!("{:02}:{:02}  ", secs / 60, secs % 60);
        let style = match entry.kind {
            LogKind::Phase => accent(),
            LogKind::Fact => Style::default(),
            LogKind::Fail => Style::default().fg(th().gold),
            LogKind::Quiet => dim(),
        };
        let row = Rect { y: pane.y + i as u16, height: 1, ..pane };
        let line = Line::from(vec![Span::styled(clock, dim()), Span::styled(fit(&entry.text, width), style)]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

/// `text`, cut to `width` cells with an ellipsis: a log line or a fact
/// never wraps.
fn fit(text: &str, width: usize) -> String {
    if kit::width(text) <= width {
        return text.to_string();
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = kit::width(c.encode_utf8(&mut [0u8; 4]));
        if used + w + 1 > width {
            break;
        }
        cut.push(c);
        used += w;
    }
    cut.push('…');
    cut
}

/// The card with no board (contract clause 30): what to do, the watch, and
/// "Not showing up?" with today's hints and the ports seen.
fn empty_card(frame: &mut Frame, page: &mut Page, room: Rect) {
    if room.height < 3 || room.width < CARD_INSET * 2 + 2 {
        return;
    }
    let w = room.width - inset(room.width) * 2;
    let g = glyphs();
    let mut rows = vec![Row::Blank];
    if let Some(e) = &page.list_failed {
        let gold = Style::default().fg(th().gold);
        rows.extend(wide_rows(&[(e.text(), gold.add_modifier(Modifier::BOLD))], w));
        if let Some(hint) = e.hint() {
            rows.extend(wide_rows(&[(hint, dim())], w));
        }
    } else if !page.watched {
        rows.extend(wide_rows(&scan(None, &Phase::Scanning.text()), w));
    } else {
        let title = Line::from(Span::styled(t!("dev.none_title").to_string(), bold()));
        rows.push(Row::Wide { left: title, right: None });
        rows.push(Row::Blank);
        rows.extend(wide_rows(&[(t!("dev.none_body").to_string(), Style::default())], w));
        rows.extend(wide_rows(&scan(None, &t!("dev.none_watching")), w));
    }
    rows.push(Row::Blank);
    let help = format!("{} {}", if page.details { g.open } else { g.forward }, t!("dev.not_showing"));
    rows.push(Row::Actions { links: vec![(help, Act::Details)], primary: None });
    if page.details {
        rows.push(Row::Blank);
        // Today's hints, each under its bullet with its lines hung under the
        // words; the driver's second line is its bullet's.
        let bullet = |words: Vec<String>| -> Vec<Row> {
            let mut rows = Vec::new();
            for (i, text) in words.iter().enumerate() {
                let lines = wrap_spans(&[(text.clone(), dim())], usize::from(w.saturating_sub(2)));
                for (j, line) in lines.into_iter().enumerate() {
                    let lead = if i == 0 && j == 0 { format!("{} ", g.bullet) } else { "  ".to_string() };
                    let mut spans = vec![Span::styled(lead, dim())];
                    spans.extend(line.spans);
                    rows.push(Row::Wide { left: Line::from(spans), right: None });
                }
            }
            rows
        };
        rows.extend(bullet(vec![t!("dev.no_device_cable").to_string()]));
        rows.extend(bullet(vec![t!("dev.no_device_driver").to_string(), t!("dev.no_device_driver_2").to_string()]));
        rows.extend(bullet(vec![t!("dev.no_device_linux").to_string()]));
        rows.push(Row::Blank);
        let seen = if page.others.is_empty() {
            vec![(format!("{} {}", t!("dev.ports_seen"), t!("dev.ports_seen_none")), dim())]
        } else {
            vec![
                (format!("{} ", t!("dev.ports_seen")), dim()),
                (page.others.join(", "), Style::default()),
                (format!(" {}", t!("dev.ports_not_bridge")), dim()),
            ]
        };
        rows.extend(wide_rows(&seen, w));
    }
    rows.push(Row::Blank);
    draw_card(frame, page, "", &rows, room);
}

// ── The gate ────────────────────────────────────────────────────────────────

/// A line of the gate's body: text, a blank that gives way when the rows
/// run short, or the erase box.
enum GateLine {
    Text(Line<'static>),
    Blank,
    Erase(Line<'static>),
}

/// The gate's words, wrapped in `width` cells (contract clause 23): what
/// goes on and how long, what you see, what stays or goes, the one danger.
fn gate_lines(gate: &Gate, width: usize) -> Vec<GateLine> {
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let green = Style::default().fg(th().ok);
    let mut lines = Vec::new();
    let text = |lines: &mut Vec<GateLine>, words: String, style: Style| {
        for line in wrap_spans(&[(words, style)], width) {
            lines.push(GateLine::Text(line));
        }
    };
    let from = gate.from.clone().unwrap_or_else(|| "?".to_string());
    let to = gate.to.clone();
    let title = match gate.kind {
        Kind::Update => t!("dev.gate_title_update", from = from, to = to),
        Kind::Replace => t!("dev.gate_title_replace", from = from, to = to),
        Kind::Install => t!("dev.gate_title_install", to = to),
        Kind::Again => t!("dev.gate_title_again", to = to),
        Kind::Back => t!("dev.gate_title_back", from = from, to = to),
        Kind::All => t!("dev.gate_title_all", n = gate.rows.len(), to = to),
    };
    text(&mut lines, title.to_string(), gold.add_modifier(Modifier::BOLD));
    lines.push(GateLine::Blank);
    let old = gate.name.clone().unwrap_or_else(|| t!("dev.gate_old_firmware").to_string());
    match gate.kind {
        Kind::All => {
            text(&mut lines, t!("dev.gate_all_time").to_string(), Style::default());
            lines.push(GateLine::Blank);
            for row in gate.rows.iter().take(GATE_ROWS) {
                let mut words = short(&row.port);
                if let Some(serial) = &row.serial {
                    words.push_str(&format!(" · {}", t!("dev.serial", serial = serial)));
                }
                words.push_str(&format!(" · {} {} {to}", row.from, g.arrow));
                lines.push(GateLine::Text(Line::from(Span::raw(fit(&words, width)))));
            }
            if gate.rows.len() > GATE_ROWS {
                let more = t!("dev.gate_all_more", n = gate.rows.len() - GATE_ROWS).to_string();
                lines.push(GateLine::Text(Line::from(Span::styled(more, dim()))));
            }
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_all_stays").to_string(), green);
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_all_plugged").to_string(), Style::default());
        }
        Kind::Install => {
            let time = match &gate.name {
                Some(name) => t!("dev.gate_install_time", name = name),
                None => t!("dev.gate_blank_time"),
            };
            text(&mut lines, time.to_string(), Style::default());
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_unknown").to_string(), gold);
            lines.push(GateLine::Blank);
            let mark = if gate.erase { "[x]" } else { "[ ]" };
            lines.push(GateLine::Erase(Line::from(format!("{mark} {}", t!("dev.gate_erase")))));
            let note =
                if gate.erase { t!("dev.gate_erase_on", name = old) } else { t!("dev.gate_erase_off", name = old) };
            text(&mut lines, note.to_string(), dim());
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_card_untouched").to_string(), green);
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_plugged_install").to_string(), Style::default());
        }
        _ => {
            text(&mut lines, t!("dev.gate_time").to_string(), Style::default());
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_stays").to_string(), green);
            lines.push(GateLine::Blank);
            text(&mut lines, t!("dev.gate_plugged").to_string(), Style::default());
        }
    }
    lines.push(GateLine::Blank);
    lines
}

/// The gate's two buttons: the safe choice, named for what stays, then
/// the write.
fn gate_buttons(gate: &Gate) -> (String, String) {
    let back = glyphs().back;
    let keep = match gate.kind {
        Kind::All => t!("dev.gate_keep_all").to_string(),
        Kind::Install => t!("dev.gate_keep_as_is").to_string(),
        _ => match &gate.from {
            Some(from) => t!("dev.gate_keep", version = from).to_string(),
            None => t!("dev.gate_keep_as_is").to_string(),
        },
    };
    let write = t!(match gate.kind {
        Kind::Update | Kind::Replace => "dev.gate_do_update",
        Kind::Install => "dev.gate_do_install",
        Kind::Again => "dev.gate_do_again",
        Kind::Back => "dev.gate_do_back",
        Kind::All => "dev.gate_do_all",
    });
    (format!("{back} {keep}"), write.to_string())
}

/// The kit's gold warning modal over the card: no [X], the safe choice
/// first and in the modal primary, centred in the page's area.
fn draw_gate(frame: &mut Frame, page: &mut Page, area: Rect) {
    let Some(gate) = page.gate.clone() else { return };
    let width = GATE_W.min(area.width.saturating_sub(4));
    if width < 12 || area.height < 5 {
        return;
    }
    let text_w = usize::from(width.saturating_sub(4));
    let mut lines = gate_lines(&gate, text_w);
    // The frame's two rows and the buttons' row: short of rows, the blank
    // lines give way first, from the end.
    let most = usize::from(area.height.saturating_sub(2)).saturating_sub(3);
    while lines.len() > most {
        let Some(at) = lines.iter().rposition(|l| matches!(l, GateLine::Blank)) else { break };
        lines.remove(at);
    }
    let height = (lines.len() + 3) as u16;
    let inner = kit::modal_frame_on(frame, &mut page.ui, area, width, height, th().gold);
    if inner.height == 0 {
        return;
    }
    let body = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(1),
    };
    for (i, line) in lines.iter().enumerate() {
        let y = body.y + i as u16;
        if y >= body.bottom() {
            break;
        }
        let at = Rect { y, height: 1, ..body };
        match line {
            GateLine::Blank => {}
            GateLine::Text(line) => frame.render_widget(Paragraph::new(line.clone()), at),
            GateLine::Erase(line) => {
                let rect = Rect { width: (line.width() as u16).min(at.width), ..at };
                let style = if page.ui.hovers(rect) { Style::default().fg(th().bright) } else { Style::default() };
                frame.render_widget(Paragraph::new(line.clone().style(style)), rect);
                page.ui.click(rect, Act::Erase);
            }
        }
    }
    let y = inner.bottom().saturating_sub(1);
    let (keep, write) = gate_buttons(&gate);
    let keep_w = kit::width(&keep) as u16 + 4;
    let write_w = kit::width(&write) as u16 + 4;
    let keep_x = inner.right().saturating_sub(write_w + 2 + keep_w).max(inner.x);
    let at = Rect { x: keep_x, y, width: inner.right() - keep_x, height: 1 };
    let keep_rect = kit::button(frame, &mut page.ui, at, &keep, true, Act::Keep);
    let write_x = keep_rect.right() + 2;
    if write_x < inner.right() {
        let at = Rect { x: write_x, y, width: inner.right() - write_x, height: 1 };
        kit::button(frame, &mut page.ui, at, &write, false, Act::Write);
    }
}

// ── The page under test ─────────────────────────────────────────────────────

/// The far ends of a page's channels, for the tests here and the GUI's:
/// what the page told its worker, and the worker's reports to hand it.
/// Standing in for the worker, they spawn nothing — no firmware is fetched
/// and no serial port is opened (a Core2 may well be plugged in).
#[cfg(test)]
pub(crate) struct Ends {
    pub(crate) cmds: Receiver<Cmd>,
    pub(crate) events: Sender<Event>,
}

#[cfg(test)]
impl Page {
    /// The page the GUI's MP3 Player tab builds under test, in place of
    /// [`hosted`]: on channels whose far ends the test keeps.
    pub(crate) fn quiet() -> (Page, Ends) {
        use std::sync::mpsc::channel;
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        (Page::with_channels(cmd_tx, event_rx, None), Ends { cmds: cmd_rx, events: event_tx })
    }
}

/// Boards as the worker would tell them, judged against the pin.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::device::board::Status;
    use crate::device::board::tests::{candidate, status};
    use crate::device::engine::DeviceInfo;
    use crate::device::firmware::AppDesc;

    /// The pin: v0.8.0, which does not answer the status query.
    pub(crate) fn pin() -> Target {
        Target { version: "v0.8.0".into(), answers_status: false }
    }

    /// The board's verdict again, against the pin.
    pub(crate) fn judged(mut board: Board) -> Board {
        board.verdict = board.judge(Some(&pin()));
        board
    }

    /// A board on `port` that said `heard`.
    pub(crate) fn heard(port: &str, heard: Heard) -> Board {
        let mut board = Board::new(candidate(port, &format!("5B1F00{port}")));
        board.heard = heard;
        judged(board)
    }

    /// Firmware that answers the status query at `fw`, its card the
    /// cards' example: 59.6 GB, 21.4 GB used, 1,284 tracks.
    pub(crate) fn answering(port: &str, fw: &str) -> Board {
        heard(port, Heard::Status(status(fw)))
    }

    /// The same, its status changed first.
    pub(crate) fn answering_with(port: &str, fw: &str, change: impl FnOnce(&mut Status)) -> Board {
        let mut status = status(fw);
        change(&mut status);
        heard(port, Heard::Status(status))
    }

    /// Our firmware too old for the status query: `@err 7`, then `L`.
    pub(crate) fn old(port: &str, version: &str) -> Board {
        heard(port, Heard::Old { version: Some(version.into()), elf: Some("11c35a4a".into()) })
    }

    pub(crate) fn info(port: &str) -> DeviceInfo {
        DeviceInfo {
            port: port.into(),
            bridge: "CH9102".into(),
            chip: "esp32".into(),
            revision: Some((3, 1)),
            flash_mb: Some(16),
            baud: 921_600,
        }
    }

    pub(crate) fn ours(version: &str) -> AppDesc {
        AppDesc { version: version.into(), project: AppDesc::OURS.into(), idf: "v5.5.5".into(), elf8: "fa4e0000".into() }
    }

    /// A board that said nothing, then read over its bootloader.
    pub(crate) fn read(port: &str, on_board: Option<AppDesc>) -> Board {
        let mut board = heard(port, Heard::Silent);
        board.probe = Some(Ok(Probe { info: info(port), on_board }));
        judged(board)
    }
}

#[cfg(test)]
impl Ends {
    pub(crate) fn tell(&self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            // A dropped page is the test's own business, not a failure here.
            let _ = self.events.send(event);
        }
    }

    /// Every command the page has sent since the last look.
    pub(crate) fn sent(&self) -> Vec<Cmd> {
        self.cmds.try_iter().collect()
    }

    /// The page is gone: its end of the reports is dropped, so a report
    /// sent now has nobody to read it.
    pub(crate) fn page_gone(&self) -> bool {
        self.events.send(Event::Log { port: None, text: String::new(), kind: LogKind::Quiet }).is_err()
    }

    /// The pin, the image in hand, and `boards` heard, in their order.
    pub(crate) fn boards(&self, boards: Vec<Board>) {
        let ports = boards.iter().map(|b| b.port().to_string()).collect();
        self.tell([
            Event::Target(fixtures::pin()),
            Event::Firmware {
                version: "v0.8.0".into(),
                origin: "release v0.8.0, downloaded earlier".into(),
                bytes: 2_431_000,
                kind: Origin::Cached,
            },
        ]);
        self.tell(boards.into_iter().map(Event::Board));
        self.tell([Event::Watch { ports, others: Vec::new() }]);
    }

    /// One board, up to date and reporting its card.
    pub(crate) fn up_to_date(&self) {
        self.boards(vec![fixtures::answering("COM3", "v0.8.0")]);
    }

    /// One board running v0.7.0, too old to report its card.
    pub(crate) fn update_available(&self) {
        self.boards(vec![fixtures::old("COM3", "v0.7.0")]);
    }

    /// COM3 up to date, COM5 with an update.
    pub(crate) fn two(&self) {
        self.boards(vec![fixtures::answering("COM3", "v0.8.0"), fixtures::old("COM5", "v0.7.0")]);
    }

    /// No Core2-shaped port; COM1 is another serial port.
    pub(crate) fn no_board(&self) {
        self.tell([Event::Target(fixtures::pin()), Event::Watch { ports: Vec::new(), others: vec!["COM1".into()] }]);
    }

    /// COM3's write under way, at `pct`.
    pub(crate) fn writing(&self, pct: u8) {
        let mut board = fixtures::old("COM3", "v0.7.0");
        board.work = Work::Writing { phase: Phase::Writing, pct: Some(pct) };
        self.tell([Event::Board(board)]);
    }

    /// COM3 written and restarted, its new firmware being asked.
    pub(crate) fn written(&self) {
        let mut board = fixtures::heard("COM3", Heard::Nothing);
        board.probe = Some(Ok(Probe { info: fixtures::info("COM3"), on_board: Some(fixtures::ours("v0.8.0")) }));
        board.written = Some(Written::Done {
            version: "v0.8.0".into(),
            took: Duration::from_secs(41),
            skipped: false,
            install: false,
            boot: Some("mstream-mp3-player v0.8.0 (commit 4e94418, 2026-10-10), ELF 11c35a4a".into()),
        });
        board.work = Work::Listening;
        self.tell([Event::Board(fixtures::judged(board))]);
    }

    /// COM3's write died at 38%: half written.
    pub(crate) fn failed(&self) {
        let mut board = fixtures::heard("COM3", Heard::Nothing);
        board.probe = Some(Ok(Probe { info: fixtures::info("COM3"), on_board: Some(fixtures::ours("v0.7.0")) }));
        let error = DeviceError::Link("the board stopped answering".into());
        board.written = Some(Written::Failed { error, half: true, pct: Some(38) });
        self.tell([Event::Board(fixtures::judged(board))]);
    }

    /// Every port let go.
    pub(crate) fn released(&self) {
        self.tell([Event::Released]);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyModifiers;

    use super::fixtures::{answering, answering_with, heard, old, ours, read};
    use super::*;
    use crate::device::board::Status;

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A page on quiet channels, its worker's reports read.
    fn page_with(tell: impl FnOnce(&Ends)) -> (Page, Ends) {
        let (mut page, ends) = Page::quiet();
        tell(&ends);
        page.pump();
        (page, ends)
    }

    fn press(page: &mut Page, code: KeyCode) {
        Screen::key(page, key(code));
        page.pump();
    }

    /// The page hosted in `area` of a `w`×`h` frame whose every other cell
    /// holds a `#`, so a test sees whether the page drew outside its area.
    fn hosted_at(page: &mut Page, w: u16, h: u16, area: Rect) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|frame| {
                let full = frame.area();
                let buf = frame.buffer_mut();
                for y in full.top()..full.bottom() {
                    for x in full.left()..full.right() {
                        if !area.contains(Position { x, y }) {
                            buf[(x, y)].set_symbol("#");
                        }
                    }
                }
                render_hosted(frame, page, area);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_of(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn text_of(buf: &Buffer) -> String {
        (0..buf.area.height).map(|y| row_of(buf, y) + "\n").collect()
    }

    /// The GUI's window: the page in rows 1–28 of 100×30, under the top
    /// bar and over the footer.
    const WINDOW: Rect = Rect { x: 0, y: 1, width: 100, height: 28 };
    /// The GUI's floor: rows 1–22 of 100×24.
    const FLOOR: Rect = Rect { x: 0, y: 1, width: 100, height: 22 };

    fn window(page: &mut Page) -> String {
        text_of(&hosted_at(page, 100, 30, WINDOW))
    }

    /// The standalone page at `w`×`h`.
    fn alone(page: &mut Page, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| render(frame, page)).unwrap();
        text_of(terminal.backend().buffer())
    }

    /// The rows from `from`, trimmed at the right, `#` and all.
    fn rows(frame: &str, from: usize, n: usize) -> Vec<String> {
        frame.lines().skip(from).take(n).map(|l| l.trim_end().to_string()).collect()
    }

    // ── The card ────────────────────────────────────────────────────────────

    #[test]
    fn an_up_to_date_board_is_card_02s_card_with_no_primary_and_nothing_was_asked_of_it() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::up_to_date);
        let frame = window(&mut page);
        let card = [
            "           ╭────────────────────────────────────────────────────────────────────────────╮",
            "           │                                                                            │",
            "           │  M5Stack Core2                                                       COM3  │",
            "           │                                                                            │",
            "           │  Firmware      ✓ Up to date · v0.8.0                                       │",
            "           │                                                                            │",
            "           │  SD card       █████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │",
            "           │                21.4 of 59.6 GB used · 1,284 tracks           38.2 GB free  │",
            "           │                                                                            │",
            "           │  ▸ Details                                                                 │",
            "           │                                                                            │",
            "           ╰────────────────────────────────────────────────────────────────────────────╯",
        ];
        assert_eq!(rows(&frame, 2, 12), card, "card 02's frame, character for character:\n{frame}");
        assert!(rows(&frame, 1, 1)[0].trim().is_empty(), "the area's first row is the host's");
        assert!(ends.sent().is_empty(), "opening only listens: the page asked nothing");
        assert_eq!(page.hint(), "d details · Esc library", "no primary, nothing for Enter");
        press(&mut page, KeyCode::Enter);
        assert!(ends.sent().is_empty() && page.gate.is_none(), "Enter does nothing with no primary");
    }

    #[test]
    fn an_older_release_offers_update_and_enter_opens_the_gate_before_anything_is_written() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::update_available);
        let frame = window(&mut page);
        let card = [
            "           │  Firmware      ! Update available · v0.7.0 → v0.8.0                        │",
            "           │                                                                            │",
            "           │  SD card       ? v0.7.0 can't report the card.                             │",
            "           │                                                                            │",
            "           │                                                            ╭────────────╮  │",
            "           │  ▸ Details                                                 │  Update ▸  │  │",
            "           │                                                            ╰────────────╯  │",
            "           │                                                                            │",
            "           ╰────────────────────────────────────────────────────────────────────────────╯",
        ];
        assert_eq!(rows(&frame, 6, 9), card, "{frame}");
        assert_eq!(page.hint(), "Enter update · d details · Esc library");
        press(&mut page, KeyCode::Enter);
        assert!(ends.sent().is_empty(), "the gate first: nothing was reset");
        let frame = window(&mut page);
        assert!(frame.contains("│ Update from v0.7.0 to v0.8.0?"), "B's title:\n{frame}");
        assert!(frame.contains("About a minute. Your MP3 player's screen goes dark"), "{frame}");
        assert!(frame.contains("Settings, paired headphones and the SD card's music all stay."), "{frame}");
        assert!(frame.contains("halfway leaves it half written; writing again fixes that."), "{frame}");
        assert!(frame.contains("◂ Keep v0.7.0      Update  │"), "the safe choice first, named for what stays:\n{frame}");
        assert!(!frame.contains("[X]") && !frame.contains("Erase"), "no [X], and no erase box in an update:\n{frame}");
        assert_eq!(page.hint(), "y update · Enter or Esc cancel");
        assert!(Screen::modal_open(&page));
        for keep in [KeyCode::Char('n'), KeyCode::Esc, KeyCode::Enter] {
            assert!(page.gate.is_some());
            press(&mut page, keep);
            assert!(page.gate.is_none(), "{keep:?} keeps the board as it is");
            assert!(ends.sent().is_empty() && !page.writing());
            press(&mut page, KeyCode::Enter);
        }
        press(&mut page, KeyCode::Char('y'));
        assert_eq!(ends.sent(), [Cmd::Write { port: "COM3".into(), erase: Some(false) }], "y writes, never erasing an update");
        assert!(page.writing(), "locked from the yes");
    }

    #[test]
    fn the_old_firmwares_card_row_promises_the_card_only_when_the_pin_answers_the_status_query() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::update_available);
        assert!(!window(&mut page).contains("Updating shows it"), "v0.8.0 does not answer @status");
        ends.tell([Event::Target(Target { version: "v0.9.0".into(), answers_status: true })]);
        page.pump();
        assert!(window(&mut page).contains("? v0.7.0 can't report the card. Updating shows it."));
    }

    #[test]
    fn the_write_runs_on_the_card_and_done_is_the_card_again_with_one_line() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::update_available);
        press(&mut page, KeyCode::Enter);
        press(&mut page, KeyCode::Char('y'));
        ends.sent();
        ends.writing(62);
        page.pump();
        let frame = window(&mut page);
        let card = [
            "           │  Firmware      writing v0.8.0… 62%                                         │",
            "           │                ████████████████████████████████████░░░░░░░░░░░░░░░░░░░░░░  │",
            "           │                                                                            │",
            "           │  SD card       untouched · shown again once the board restarts             │",
        ];
        assert_eq!(rows(&frame, 6, 4), card, "{frame}");
        assert!(!frame.contains("Update ▸"), "no primary during the write");
        assert_eq!(page.hint(), "please wait — unplugging now would leave the board half written · d details");
        for code in [KeyCode::Esc, KeyCode::Enter, KeyCode::Char('r'), KeyCode::Char('w'), KeyCode::Char('a')] {
            press(&mut page, code);
        }
        assert!(ends.sent().is_empty() && page.writing(), "nothing leaves the write");
        page.release();
        assert!(ends.sent().is_empty(), "a write is never told to stop");
        assert!(!page.holds_board() && Screen::finished(&page).is_none());
        ends.written();
        page.pump();
        assert!(!page.writing(), "the write has ended: the board carries its result");
        let frame = window(&mut page);
        assert!(frame.contains("Firmware      ✓ Up to date · v0.8.0 · just written"), "{frame}");
        assert!(frame.contains("SD card       shown once the player answers"), "the card waits for the firmware:\n{frame}");
        assert_eq!(
            rows(&frame, 28, 1)[0].trim(),
            "Updated from v0.7.0 just now. The SD card was not touched.",
            "C's one line, on the busy row:\n{frame}"
        );
        assert!(!frame.contains("Close") && !frame.contains("Done"), "no Done page");
    }

    #[test]
    fn the_write_says_how_long_it_has_left_once_it_knows_the_rate() {
        let _en = english();
        let (mut page, _ends) = page_with(Ends::update_available);
        press(&mut page, KeyCode::Enter);
        press(&mut page, KeyCode::Char('y'));
        let at = |pct: u8| {
            let mut board = old("COM3", "v0.7.0");
            board.work = Work::Writing { phase: Phase::Writing, pct: Some(pct) };
            Event::Board(board)
        };
        let t0 = Instant::now();
        page.apply_at(at(10), t0);
        page.apply_at(at(20), t0 + Duration::from_secs(1));
        assert_eq!(page.time_left, None, "too early to say");
        // 10% → 40% in ten seconds: 3%/s, 60% to go — twenty seconds.
        page.apply_at(at(40), t0 + Duration::from_secs(10));
        assert_eq!(page.time_left, Some(20));
        assert!(window(&mut page).contains("writing v0.8.0… 40% · about 20 s left"));
        page.apply_at(at(99), t0 + Duration::from_secs(29));
        assert!(window(&mut page).contains("writing v0.8.0… 99% · a few seconds left"));
    }

    #[test]
    fn a_failed_write_opens_details_on_its_log_and_try_again_passes_the_gate_again() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::update_available);
        press(&mut page, KeyCode::Enter);
        press(&mut page, KeyCode::Char('y'));
        ends.sent();
        ends.writing(38);
        let line = "the board stopped answering at 38% — unplugged?".to_string();
        ends.tell([Event::Log { port: Some("COM3".into()), text: line, kind: LogKind::Fail }]);
        ends.failed();
        page.pump();
        assert!(!page.writing());
        assert!(page.details, "Details opened by itself");
        let frame = window(&mut page);
        assert!(frame.contains("Firmware      ✗ Write failed · the board is half written"), "{frame}");
        assert!(frame.contains("It cannot start until it is written again."), "{frame}");
        assert!(frame.contains("SD card       ? unknown until the firmware runs again"), "{frame}");
        assert!(frame.contains("▾ Details") && frame.contains("Try again ▸"), "{frame}");
        assert!(frame.contains("Port          COM3 · CH9102 · serial 5B1F00COM3 · 921600 baud"), "only the port before the log:\n{frame}");
        assert!(!frame.contains("This player"), "{frame}");
        assert!(frame.contains("the board stopped answering at 38%"), "the log's last lines:\n{frame}");
        assert_eq!(page.hint(), "Enter try again · d details · Esc library");
        press(&mut page, KeyCode::Enter);
        assert!(window(&mut page).contains("Write v0.8.0 again?"), "behind the gate again");
        press(&mut page, KeyCode::Char('y'));
        assert_eq!(ends.sent(), [Cmd::Write { port: "COM3".into(), erase: Some(false) }]);
    }

    #[test]
    fn a_board_that_does_not_answer_is_read_without_a_gate_and_its_cost_said_first() {
        let _en = english();
        let (mut page, ends) = page_with(|e| e.boards(vec![heard("COM3", Heard::Silent)]));
        let frame = window(&mut page);
        assert!(frame.contains("Unknown board"), "never called a Core2 before it answers:\n{frame}");
        assert!(frame.contains("Firmware      ? Not answering · may not be an MP3 player"), "{frame}");
        assert!(frame.contains("Its USB chip is a Core2's, but many ESP32 boards share it."), "{frame}");
        assert!(frame.contains("Reading it restarts the board for a few seconds."), "the cost, said first:\n{frame}");
        assert!(frame.contains("SD card       ? Only mStream firmware can read the card"), "{frame}");
        assert!(frame.contains("│  Read the board ▸  │"), "{frame}");
        press(&mut page, KeyCode::Enter);
        assert!(page.gate.is_none(), "a read writes nothing: no gate");
        assert_eq!(ends.sent(), [Cmd::Read { port: "COM3".into() }]);
        let mut reading = heard("COM3", Heard::Silent);
        reading.work = Work::Reading;
        ends.tell([Event::Board(reading)]);
        page.pump();
        let frame = window(&mut page);
        assert!(frame.contains("reading it over its bootloader…"), "{frame}");
        assert!(frame.contains("The screen is dark a few seconds; it restarts as it was."), "{frame}");
    }

    #[test]
    fn other_firmware_gets_the_install_gate_its_erase_box_on_and_e_flips_it() {
        let _en = english();
        let mut uiflow = ours("v1.13.2");
        uiflow.project = "UIFlow".into();
        let (mut page, ends) = page_with(|e| e.boards(vec![read("COM3", Some(uiflow))]));
        let frame = window(&mut page);
        assert!(frame.contains("M5Stack Core2"), "an ESP32 with 16 MB: maybe a Core2:\n{frame}");
        assert!(frame.contains("Firmware      ✗ Not mStream firmware · UIFlow"), "{frame}");
        assert!(frame.contains("│  Install ▸  │"), "{frame}");
        press(&mut page, KeyCode::Enter);
        let frame = window(&mut page);
        assert!(frame.contains("Install mStream firmware v0.8.0?"), "{frame}");
        assert!(frame.contains("It replaces UIFlow and takes about a minute. This Core2's screen goes"), "{frame}");
        assert!(frame.contains("This page can't tell a Core2 from another ESP32 board with 16 MB"), "{frame}");
        assert!(frame.contains("[x] Erase the whole flash first"), "on by default:\n{frame}");
        assert!(frame.contains("Right for a first install: what UIFlow kept on the board goes."), "{frame}");
        assert!(frame.contains("The SD card is not touched."), "{frame}");
        assert!(frame.contains("◂ Keep it as it is      Install  │"), "{frame}");
        assert_eq!(page.hint(), "y install · e erase first · Enter or Esc cancel");
        press(&mut page, KeyCode::Char('e'));
        let frame = window(&mut page);
        assert!(frame.contains("[ ] Erase the whole flash first"), "{frame}");
        assert!(frame.contains("Not erased: what UIFlow kept on the board stays"), "{frame}");
        press(&mut page, KeyCode::Char('y'));
        assert_eq!(ends.sent(), [Cmd::Write { port: "COM3".into(), erase: Some(false) }]);
        // A blank board: the same gate, saying so.
        let (mut page, _ends) = page_with(|e| e.boards(vec![read("COM3", None)]));
        assert!(window(&mut page).contains("✗ Nothing installed"));
        press(&mut page, KeyCode::Enter);
        let frame = window(&mut page);
        assert!(frame.contains("Nothing is installed on it yet.") && frame.contains("[x] Erase"), "{frame}");
    }

    #[test]
    fn a_board_that_is_not_a_core2_is_offered_nothing() {
        let _en = english();
        let mut chip = heard("COM9", Heard::Silent);
        chip.probe = Some(Err(DeviceError::WrongFlash { found: "4 MB".into() }));
        let (mut page, ends) = page_with(|e| e.boards(vec![fixtures::judged(chip)]));
        let frame = window(&mut page);
        assert!(frame.contains("ESP32 board"), "{frame}");
        assert!(frame.contains("Board         ✗ Not a Core2 · 4 MB"), "{frame}");
        assert!(frame.contains("A Core2 is an ESP32 with 16 MB. Nothing is written to it."), "{frame}");
        assert!(!frame.contains("SD card") && !frame.contains("▸  │"), "no card row, no primary:\n{frame}");
        press(&mut page, KeyCode::Enter);
        assert!(ends.sent().is_empty() && page.gate.is_none());
    }

    #[test]
    fn newer_firmware_is_never_an_update_and_details_offers_going_back_behind_the_gate() {
        let _en = english();
        for fw in ["v0.9.0", "v0.8.0-5-g4e94418"] {
            let (mut page, ends) = page_with(|e| e.boards(vec![answering("COM3", fw)]));
            let frame = window(&mut page);
            assert!(frame.contains(&format!("Firmware      ! Newer than this player · {fw}")), "{frame}");
            assert!(frame.contains("This player carries v0.8.0; the board is fine as it is."), "{frame}");
            assert!(!frame.contains("▸  │"), "no primary: the pin would go back:\n{frame}");
            press(&mut page, KeyCode::Enter);
            press(&mut page, KeyCode::Char('w'));
            assert!(page.gate.is_none(), "w acts only with Details open");
            press(&mut page, KeyCode::Char('d'));
            assert_eq!(ends.sent(), [Cmd::Facts { port: "COM3".into() }], "Details asks L for the flash size");
            let frame = window(&mut page);
            assert!(frame.contains("Write v0.8.0 again"), "{frame}");
            assert_eq!(page.hint(), "w write again · d details · Esc library");
            press(&mut page, KeyCode::Char('w'));
            let frame = window(&mut page);
            assert!(frame.contains(&format!("Go back from {fw} to v0.8.0?")), "{frame}");
            assert!(frame.contains(&format!("◂ Keep {fw}")) && frame.contains("Go back  │"), "{frame}");
            // Gone back, Done's one line says what was replaced — never that
            // the board was updated.
            press(&mut page, KeyCode::Char('y'));
            assert_eq!(ends.sent(), [Cmd::Write { port: "COM3".into(), erase: Some(false) }]);
            ends.writing(62);
            ends.written();
            page.pump();
            assert!(!page.writing());
            let frame = window(&mut page);
            let note = rows(&frame, 28, 1)[0].trim().to_string();
            assert_eq!(note, format!("Replaced {fw} just now. The SD card was not touched."), "{frame}");
            assert!(!frame.contains("Updated"), "a step back is never an update:\n{frame}");
        }
    }

    #[test]
    fn a_development_build_below_the_pin_is_replaced_through_the_update_gate() {
        let _en = english();
        let (mut page, ends) = page_with(|e| e.boards(vec![old("COM3", "v0.6.0-37-g221d99d")]));
        let frame = window(&mut page);
        assert!(frame.contains("Firmware      ! Development build · v0.6.0-37-g221d99d → v0.8.0"), "{frame}");
        press(&mut page, KeyCode::Enter);
        assert!(window(&mut page).contains("Replace v0.6.0-37-g221d99d with v0.8.0?"));
        press(&mut page, KeyCode::Char('y'));
        assert_eq!(ends.sent(), [Cmd::Write { port: "COM3".into(), erase: Some(false) }], "ours: never an erase");
    }

    /// The card for a board whose status `change` made.
    fn card_row(change: impl FnOnce(&mut Status)) -> (Page, Ends, String) {
        let (mut page, ends) = page_with(|e| e.boards(vec![answering_with("COM3", "v0.8.0", change)]));
        let frame = window(&mut page);
        (page, ends, frame)
    }

    #[test]
    fn the_sd_card_row_says_what_the_firmware_reports_and_never_guesses() {
        let _en = english();
        let (_, _, frame) = card_row(|s| {
            s.card = CardKind::None;
            s.size = None;
            s.free = Free::NotCounted;
            s.tracks = Tracks::NoCard;
        });
        assert!(frame.contains("SD card       ! No card"), "{frame}");
        assert!(frame.contains("Put in a FAT32 card with your music under /music."), "{frame}");
        let (_, _, frame) = card_row(|s| s.card = CardKind::ExFat);
        assert!(frame.contains("SD card       ! exFAT card · the player reads FAT32 only"), "{frame}");
        assert!(frame.contains("Fix: format it FAT32 (MBR) on a computer. That erases it."), "{frame}");
        let (_, _, frame) = card_row(|s| s.card = CardKind::Gpt);
        assert!(frame.contains("! GPT card · the player reads MBR cards only"), "{frame}");
        assert!(frame.contains("Fix: make it MBR and FAT32 on a computer. That erases it."), "{frame}");
        let (_, _, frame) = card_row(|s| {
            s.free = Free::Bytes(700_000_000);
            s.tracks = Tracks::Count(3912);
        });
        assert!(frame.contains("58.9 of 59.6 GB used · 3,912 tracks          ! 0.7 GB free"), "nearly full, gold:\n{frame}");
        assert!(frame.contains("█████████████████████████████████████████████████████████░"), "{frame}");
        let (_, _, frame) = card_row(|s| {
            s.free = Free::Bytes(59_517_918_976);
            s.tracks = Tracks::Count(0);
        });
        assert!(frame.contains("SD card       █░░░"), "a sliver used still shows a cell:\n{frame}");
        assert!(frame.contains("0.1 of 59.6 GB used · no tracks"), "{frame}");
        assert!(frame.contains("Music goes in /music/Artist/Album/ · MP3, FLAC or Opus."), "the next step:\n{frame}");
    }

    #[test]
    fn free_space_not_counted_offers_one_count_with_its_progress_and_its_refusal() {
        let _en = english();
        let (mut page, ends, frame) = card_row(|s| s.free = Free::NotCounted);
        assert!(frame.contains("SD card       59.6 GB · 1,284 tracks · free space not counted"), "no bar:\n{frame}");
        assert!(frame.contains("Count free space   a minute or more on a big card"), "{frame}");
        assert!(!frame.contains("█"), "half a bar would be a guess");
        assert_eq!(page.hint(), "c count · d details · Esc library");
        press(&mut page, KeyCode::Char('c'));
        assert_eq!(ends.sent(), [Cmd::Count { port: "COM3".into() }]);
        let mut counting = answering_with("COM3", "v0.8.0", |s| s.free = Free::NotCounted);
        counting.count = Count::Running { pct: 37 };
        counting.work = Work::Counting;
        ends.tell([Event::Board(counting)]);
        page.pump();
        let frame = window(&mut page);
        assert!(frame.contains("SD card       ▰▰▰▱▱▱▱▱▱▱ 37% counting free space…"), "{frame}");
        assert!(frame.contains("59.6 GB · 1,284 tracks"), "{frame}");
        let mut refused = answering_with("COM3", "v0.8.0", |s| s.free = Free::NotCounted);
        refused.count = Count::Refused { why: CountWhy::Playing, part_way: false };
        ends.tell([Event::Board(refused)]);
        page.pump();
        let frame = window(&mut page);
        assert!(frame.contains("Pause the music on the player first, then count."), "{frame}");
        assert!(frame.contains("Count free space"), "and the count offered again:\n{frame}");
    }

    #[test]
    fn details_shows_the_facts_and_the_log_and_asks_l_once_for_the_flash() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::up_to_date);
        let line = |port: &str, text: &str, kind: LogKind| Event::Log { port: Some(port.into()), text: text.into(), kind };
        ends.tell([
            line("COM3", "listening on COM3 — DTR and RTS low, no reset", LogKind::Phase),
            line("COM3", "@status fw=v0.8.0 elf=63ee7a2b card=fat32", LogKind::Quiet),
            line("COM9", "another board's line", LogKind::Fact),
        ]);
        page.pump();
        press(&mut page, KeyCode::Char('d'));
        assert_eq!(ends.sent(), [Cmd::Facts { port: "COM3".into() }], "L, for the flash size");
        let frame = window(&mut page);
        assert!(frame.contains("▾ Details"), "{frame}");
        assert!(frame.contains("Port          COM3 · CH9102 · serial 5B1F00COM3 · 115200 baud · no reset"), "{frame}");
        assert!(frame.contains("Board         M5Stack Core2"), "{frame}");
        assert!(frame.contains("On the board  mstream-mp3-player v0.8.0 · ELF 63ee7a2b"), "{frame}");
        assert!(frame.contains("This player   carries v0.8.0 · release v0.8.0, downloaded earlier"), "{frame}");
        assert!(frame.contains("Card          FAT32 · 59,617,918,976 bytes · free from FSINFO"), "{frame}");
        assert!(frame.contains("00:00  listening on COM3 — DTR and RTS low, no reset"), "{frame}");
        assert!(!frame.contains("another board's line"), "the log is this board's:\n{frame}");
        assert!(frame.contains("Write v0.8.0 again"), "{frame}");
        let mut known = answering("COM3", "v0.8.0");
        known.flash_mb = Some(16);
        ends.tell([Event::Board(known)]);
        page.pump();
        assert!(window(&mut page).contains("Board         M5Stack Core2 · 16 MB flash"));
        press(&mut page, KeyCode::Char('l'));
        assert!(!page.details, "l is d's twin");
        press(&mut page, KeyCode::Char('d'));
        assert!(ends.sent().is_empty(), "L asked once per board");
    }

    #[test]
    fn the_log_scrolls_under_the_wheel_and_follows_the_newest_line_otherwise() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::up_to_date);
        for i in 0..40 {
            ends.tell([Event::Log { port: Some("COM3".into()), text: format!("detail {i}"), kind: LogKind::Fact }]);
        }
        page.pump();
        press(&mut page, KeyCode::Char('d'));
        let frame = window(&mut page);
        assert!(frame.contains("detail 39") && !frame.contains("detail 0 "), "the newest lines:\n{frame}");
        let pane = page.log_rect.expect("the log was drawn");
        let inside = Position { x: pane.x + 2, y: pane.y };
        Screen::wheel(&mut page, true, inside);
        Screen::wheel(&mut page, true, inside);
        let frame = window(&mut page);
        assert!(!frame.contains("detail 39") && frame.contains("detail 37"), "scrolled up two:\n{frame}");
        for _ in 0..3 {
            Screen::wheel(&mut page, false, inside);
        }
        assert_eq!(page.log_scroll, 0, "back at the newest, and no further");
    }

    // ── Several boards ──────────────────────────────────────────────────────

    /// A click at (x, y), the hub's way.
    fn click(page: &mut Page, x: u16, y: u16) {
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
            let event = MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
            crate::admin::drive_pointer(page, event);
        }
        page.pump();
    }

    /// The column where `needle` starts on `line`, in cells (these rows are
    /// one cell a character).
    fn col(line: &str, needle: &str) -> u16 {
        line.char_indices().position(|(i, _)| line[i..].starts_with(needle)).expect("on the row") as u16
    }

    #[test]
    fn two_boards_wear_port_tabs_with_their_marks_and_the_count_and_switch_with_the_arrows() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::two);
        let frame = window(&mut page);
        let tabs = &rows(&frame, 2, 1)[0];
        assert!(tabs.starts_with("            COM3 ✓   COM5 !"), "{tabs:?}");
        assert!(tabs.ends_with("1 of 2 needs an update"), "C's count at the right edge: {tabs:?}");
        assert_eq!(tabs.chars().count(), 89, "flush with the card's right edge: {tabs:?}");
        assert!(frame.contains("M5Stack Core2                                          serial 5B1F00COM3"), "the serial on the head:\n{frame}");
        assert!(frame.contains("87 % battery · no headphones paired"), "the identity line:\n{frame}");
        assert!(frame.contains("▸ Details   Show on the player"), "{frame}");
        assert_eq!(page.hint(), "←→ player · s show on the player · d details · Esc library");
        press(&mut page, KeyCode::Right);
        let frame = window(&mut page);
        assert!(frame.contains("! Update available · v0.7.0 → v0.8.0") && frame.contains("Update ▸"), "{frame}");
        assert!(!frame.contains("battery"), "v0.7.0 reports none:\n{frame}");
        assert_eq!(page.hint(), "←→ player · Enter update · d details · Esc library");
        press(&mut page, KeyCode::Right);
        assert_eq!(page.open.as_deref(), Some("COM3"), "wrapping");
        let frame = window(&mut page);
        click(&mut page, col(&rows(&frame, 2, 1)[0], "COM5"), 2);
        assert_eq!(page.open.as_deref(), Some("COM5"), "a click on its tab");
        assert!(ends.sent().is_empty(), "switching asks nothing of a board");
    }

    #[test]
    fn show_on_the_player_asks_a_board_that_answers_and_says_what_it_did() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::two);
        press(&mut page, KeyCode::Char('s'));
        assert_eq!(ends.sent(), [Cmd::Identify { port: "COM3".into() }]);
        ends.tell([Event::Identified { port: "COM3".into(), label: "COM3".into(), result: Ok(()) }]);
        page.pump();
        let frame = window(&mut page);
        assert_eq!(rows(&frame, 28, 1)[0].trim(), "showing “This one · COM3” on its screen for 5 s, with a short buzz");
        ends.tell([Event::Identified { port: "COM3".into(), label: "COM3".into(), result: Err(IdentifyWhy::Ui) }]);
        page.pump();
        assert!(window(&mut page).contains("The player is still starting up; try again in a few seconds."));
        // v0.7.0 has no @identify: no button, no key.
        press(&mut page, KeyCode::Right);
        assert!(!window(&mut page).contains("Show on the player"));
        press(&mut page, KeyCode::Char('s'));
        assert!(ends.sent().is_empty());
        // One board alone has nothing to be told apart from.
        let (mut page, ends) = page_with(Ends::up_to_date);
        assert!(!window(&mut page).contains("Show on the player"));
        press(&mut page, KeyCode::Char('s'));
        assert!(ends.sent().is_empty());
    }

    #[test]
    fn update_all_names_every_board_in_one_gate_and_says_how_it_went() {
        let _en = english();
        let boards = vec![old("COM3", "v0.7.0"), old("COM5", "v0.6.0"), answering("COM7", "v0.8.0")];
        let (mut page, ends) = page_with(|e| e.boards(boards));
        let frame = window(&mut page);
        assert!(rows(&frame, 2, 1)[0].ends_with("Update all (2)"), "{frame}");
        assert_eq!(page.hint(), "←→ player · Enter update · a update all · d details · Esc library");
        press(&mut page, KeyCode::Char('a'));
        let frame = window(&mut page);
        assert!(frame.contains("Update 2 players to v0.8.0?"), "{frame}");
        assert!(frame.contains("COM3 · serial 5B1F00COM3 · v0.7.0 → v0.8.0"), "{frame}");
        assert!(frame.contains("COM5 · serial 5B1F00COM5 · v0.6.0 → v0.8.0"), "{frame}");
        assert!(!frame.contains("COM7 · serial"), "up to date: not in it:\n{frame}");
        assert!(frame.contains("the next is not started."), "{frame}");
        assert!(frame.contains("◂ Keep them as they are      Update all  │"), "{frame}");
        assert_eq!(page.hint(), "y update all · Enter or Esc cancel");
        press(&mut page, KeyCode::Char('y'));
        assert_eq!(ends.sent(), [Cmd::UpdateAll { ports: vec!["COM3".into(), "COM5".into()] }]);
        assert!(page.writing());
        ends.tell([Event::All(All::Running { ports: vec!["COM3".into(), "COM5".into()], at: 0 })]);
        ends.writing(41);
        page.pump();
        let frame = window(&mut page);
        assert!(rows(&frame, 2, 1)[0].contains("COM3 41%"), "{frame}");
        assert!(rows(&frame, 2, 1)[0].ends_with("Update all · 1 of 2"), "{frame}");
        ends.written();
        ends.tell([Event::All(All::Running { ports: vec!["COM3".into(), "COM5".into()], at: 1 })]);
        page.pump();
        assert!(page.writing(), "the lock lasts until the last board");
        assert_eq!(page.note.as_ref().map(|n| n.text.as_str()), Some("COM3 written and checked in 41 s"));
        let done = All::Done { ports: vec!["COM3".into(), "COM5".into()], passed: Vec::new(), took: Duration::from_secs(91) };
        ends.tell([Event::All(done)]);
        page.pump();
        assert!(!page.writing());
        let note = page.note.as_ref().map(|n| n.text.clone());
        let said = "2 players updated in 1 min 31 s, one after the other. Nothing on their SD cards changed.";
        assert_eq!(note.as_deref(), Some(said));
    }

    #[test]
    fn during_a_write_the_other_boards_are_looked_at_and_never_written() {
        let _en = english();
        let boards = vec![old("COM3", "v0.7.0"), old("COM5", "v0.7.0")];
        let (mut page, ends) = page_with(|e| e.boards(boards));
        press(&mut page, KeyCode::Enter);
        press(&mut page, KeyCode::Char('y'));
        ends.sent();
        ends.writing(62);
        page.pump();
        press(&mut page, KeyCode::Right);
        assert_eq!(page.open.as_deref(), Some("COM5"), "look");
        let frame = window(&mut page);
        assert!(rows(&frame, 2, 1)[0].contains("COM3 62%   COM5 !"), "{frame}");
        assert!(rows(&frame, 2, 1)[0].ends_with("writing COM3…"), "{frame}");
        assert!(frame.contains("One write at a time: this one can follow COM3."), "{frame}");
        assert!(frame.contains("│  Update  │") && !frame.contains("Update ▸"), "the kit's disabled frame:\n{frame}");
        assert_eq!(rows(&frame, 28, 1)[0].trim(), "COM3 · writing v0.8.0… 62%", "the writing board stays in view");
        let hint = "please wait — unplugging COM3 now would leave it half written · ←→ look · d details";
        assert_eq!(page.hint(), hint);
        for code in [KeyCode::Enter, KeyCode::Char('a'), KeyCode::Char('c'), KeyCode::Char('s'), KeyCode::Char('r'), KeyCode::Esc] {
            press(&mut page, code);
        }
        assert!(ends.sent().is_empty() && page.gate.is_none(), "don't write");
        press(&mut page, KeyCode::Char('d'));
        assert!(page.details, "Details works");
        assert!(ends.sent().is_empty(), "and asks nothing during a write");
    }

    #[test]
    fn a_board_plugged_in_never_takes_the_view_and_one_unplugged_moves_it() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::two);
        let now = Instant::now();
        page.apply_at(Event::Board(heard("COM7", Heard::Nothing)), now);
        let ports = vec!["COM3".into(), "COM5".into(), "COM7".into()];
        page.apply_at(Event::Watch { ports, others: Vec::new() }, now);
        assert_eq!(page.open.as_deref(), Some("COM3"), "the view stays");
        let frame = window(&mut page);
        assert!(rows(&frame, 2, 1)[0].contains("COM7 …"), "{frame}");
        assert!(rows(&frame, 2, 1)[0].ends_with("1 of 3 needs an update"), "{frame}");
        assert_eq!(rows(&frame, 28, 1)[0].trim(), "COM7 plugged in: asking it over USB, without a restart");
        page.apply_at(Event::Gone { port: "COM7".into() }, now);
        page.apply_at(Event::Gone { port: "COM3".into() }, now);
        assert_eq!(page.open.as_deref(), Some("COM5"), "the next, to the right");
        let frame = window(&mut page);
        assert!(!rows(&frame, 2, 1)[0].contains("COM"), "down to one: the tabs go:\n{frame}");
        assert!(frame.contains("M5Stack Core2                                                       COM5"), "card 02's head:\n{frame}");
        assert_eq!(rows(&frame, 28, 1)[0].trim(), "COM3 was unplugged. COM5 is the only one left; nothing was reset.");
        assert!(ends.sent().is_empty(), "nothing reset, nothing asked");
        // A passing note goes in its time.
        page.note.as_mut().unwrap().until = Some(Instant::now());
        Screen::tick(&mut page);
        assert!(page.note.is_none());
    }

    #[test]
    fn a_port_in_use_is_that_boards_state_and_r_asks_again() {
        let _en = english();
        let busy = heard("COM5", Heard::InUse { detail: "Access is denied".into() });
        let (mut page, ends) = page_with(|e| e.boards(vec![answering("COM3", "v0.8.0"), busy]));
        let frame = window(&mut page);
        assert!(rows(&frame, 2, 1)[0].contains("COM5 in use"), "{frame}");
        assert!(rows(&frame, 2, 1)[0].ends_with("1 player · 1 in use"), "{frame}");
        press(&mut page, KeyCode::Right);
        let frame = window(&mut page);
        assert!(frame.contains("│  Board on COM5 ") && frame.contains(" CH9102 · serial 5B1F00COM5  │"), "{frame}");
        assert!(frame.contains("Firmware      ! In use by another program"), "{frame}");
        assert!(frame.contains("A serial monitor or another mStream Player has it open."), "{frame}");
        assert!(frame.contains("SD card       ? unknown until the port is free"), "{frame}");
        assert!(frame.contains("▸ Details   Ask again"), "{frame}");
        assert_eq!(page.hint(), "←→ player · r ask again · d details · Esc library");
        press(&mut page, KeyCode::Char('r'));
        assert_eq!(ends.sent(), [Cmd::Listen { port: "COM5".into() }]);
    }

    #[test]
    fn no_board_is_the_empty_card_and_not_showing_up_holds_todays_hints() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::no_board);
        let frame = window(&mut page);
        assert!(frame.contains("│  No player plugged in"), "{frame}");
        assert!(frame.contains("Plug the M5Stack Core2 in over USB. It shows up here by itself."), "{frame}");
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ watching the USB ports…"), "{frame}");
        assert!(frame.contains("▸ Not showing up?"), "{frame}");
        assert!(!frame.contains("Look again") && !frame.contains("Close"), "the worker watches by itself");
        assert_eq!(page.hint(), "d not showing up? · Esc library");
        press(&mut page, KeyCode::Char('d'));
        let frame = window(&mut page);
        assert!(frame.contains("• a USB-C cable that carries data — some only charge"), "{frame}");
        assert!(frame.contains("• Linux: your account in the dialout (or uucp) group"), "{frame}");
        assert!(frame.contains("Serial ports seen: COM1 — not a Core2's bridge"), "{frame}");
        assert!(ends.sent().is_empty());
        // Then a board: the page goes on by itself.
        ends.tell([Event::Board(old("COM3", "v0.7.0")), Event::Watch { ports: vec!["COM3".into()], others: Vec::new() }]);
        page.pump();
        assert!(window(&mut page).contains("Update ▸"));
    }

    // ── Leaving ─────────────────────────────────────────────────────────────

    #[test]
    fn esc_lets_every_port_go_and_ends_once_the_worker_says_they_are_free() {
        let _en = english();
        let (mut page, ends) = page_with(Ends::up_to_date);
        press(&mut page, KeyCode::Esc);
        assert_eq!(ends.sent(), [Cmd::Quit]);
        assert!(page.holds_board() && Screen::finished(&page).is_none(), "the worker may still have a port");
        assert_eq!(page.busy().as_deref(), Some("letting the USB ports go…"));
        ends.released();
        page.pump();
        assert!(!page.holds_board());
        assert!(matches!(Screen::finished(&page), Some(Outcome::Quit)));
        // A page that never saw a board holds nothing: it ends at once.
        let (mut page, ends) = page_with(Ends::no_board);
        press(&mut page, KeyCode::Esc);
        assert_eq!(ends.sent(), [Cmd::Quit]);
        assert!(!page.holds_board() && Screen::finished(&page).is_some());
        // Esc with the gate up only closes it.
        let (mut page, ends) = page_with(Ends::update_available);
        press(&mut page, KeyCode::Enter);
        press(&mut page, KeyCode::Esc);
        assert!(page.gate.is_none() && ends.sent().is_empty() && Screen::finished(&page).is_none());
    }

    #[test]
    fn released_the_page_asks_the_worker_to_let_go_and_a_read_under_way_says_it_restarts_the_board() {
        let _en = english();
        let (mut page, ends) = page_with(|e| e.boards(vec![heard("COM3", Heard::Silent)]));
        press(&mut page, KeyCode::Enter);
        ends.sent();
        let mut reading = heard("COM3", Heard::Silent);
        reading.work = Work::Reading;
        ends.tell([Event::Board(reading)]);
        page.release();
        assert_eq!(ends.sent(), [Cmd::Quit]);
        assert!(page.holds_board());
        assert_eq!(page.parting_words(), "restarting the board…");
        ends.released();
        page.pump();
        assert!(!page.holds_board());
        // A worker that ended without a word holds nothing either.
        let (mut page, ends) = page_with(Ends::up_to_date);
        page.release();
        drop(ends);
        page.pump();
        assert!(!page.holds_board() && Screen::finished(&page).is_some());
    }

    #[test]
    fn the_page_over_the_real_worker_listens_first_and_resets_only_for_the_write() {
        let _en = english();
        // The desk on the fake engine (no serial port): an old board heard,
        // the gate's yes, the write, the board heard again.
        let image = crate::device::desk::tests::image("page", "v0.8.0");
        let fake = engine::fake::Fake::new("old:v0.7.0").with_pace(Duration::from_millis(60));
        let trace = fake.trace();
        let source = crate::device::firmware::Source::Local(image.clone());
        let timing = crate::device::desk::tests::QUICK;
        let (cmds, events) = desk::spawn(std::sync::Arc::new(fake), source, None, timing, false);
        let mut page = Page::with_channels(cmds, events, None);
        let until = |page: &mut Page, what: &str, done: &dyn Fn(&Page) -> bool| {
            let t0 = Instant::now();
            while !done(page) {
                assert!(t0.elapsed() < Duration::from_secs(10), "never: {what}");
                std::thread::sleep(Duration::from_millis(10));
                page.pump();
            }
        };
        until(&mut page, "the update offered", &|p| p.shown().is_some_and(|b| p.primary_of(b) == Some(Primary::Update)));
        assert_eq!(*trace.lock().unwrap(), ["listen FAKE0"], "opening only listened");
        press(&mut page, KeyCode::Enter);
        assert_eq!(*trace.lock().unwrap(), ["listen FAKE0"], "the gate is the board's last untouched moment");
        press(&mut page, KeyCode::Char('y'));
        until(&mut page, "written and heard", &|p| {
            !p.writing() && p.shown().is_some_and(|b| b.verdict == Verdict::UpToDate && b.work == Work::Idle)
        });
        let _ = std::fs::remove_file(&image);
        assert_eq!(*trace.lock().unwrap(), ["listen FAKE0", "open 921600", "restart", "listen FAKE0"]);
        assert!(window(&mut page).contains("✓ Up to date · v0.8.0 · just written"));
        page.let_go_within(Duration::from_secs(5));
        assert!(!page.holds_board() && Screen::finished(&page).is_some());
    }

    // ── The footer, the floor and the console ───────────────────────────────

    #[test]
    fn on_its_own_the_page_keeps_its_header_and_tips_at_seventy_two_columns() {
        let _en = english();
        let (mut page, _ends) = page_with(Ends::update_available);
        let frame = alone(&mut page, 72, 24);
        assert!(frame.starts_with("  mStream MP3 Player"), "{frame}");
        assert!(rows(&frame, 0, 1)[0].ends_with("firmware v0.8.0 · cached"), "{frame}");
        assert_eq!(frame.lines().last().map(str::trim), Some("Enter update · d details · Esc leave"), "{frame}");
        assert!(frame.contains("│  Update ▸  │"), "{frame}");
        let (mut page, _ends) = page_with(Ends::up_to_date);
        let frame = alone(&mut page, 72, 24);
        let card = frame.lines().find(|l| l.contains('╭')).unwrap();
        assert_eq!(card.trim().chars().count(), 68, "the card, 68 cells: {card:?}");
        let bar = frame.lines().find(|l| l.contains('█')).unwrap();
        assert_eq!(bar.chars().filter(|c| matches!(c, '█' | '░')).count(), 50, "the bar, 50: {bar:?}");
        assert!(alone(&mut page, 40, 10).contains("larger"));
    }

    /// Every state the page draws, each on quiet channels of its own: its
    /// name and the page (with its channels' far ends, kept so a page that
    /// sends reaches somebody).
    fn every_state() -> Vec<(&'static str, Page, Ends)> {
        let mut states: Vec<(&'static str, Page, Ends)> = Vec::new();
        let mut add = |name: &'static str, tell: &dyn Fn(&Ends), keys: &[KeyCode]| {
            let (mut page, ends) = page_with(tell);
            for code in keys {
                press(&mut page, *code);
            }
            states.push((name, page, ends));
        };
        add("looking", &|_| {}, &[]);
        add("no board", &Ends::no_board, &[]);
        add("no board, not showing up", &Ends::no_board, &[KeyCode::Char('d')]);
        add("up to date", &Ends::up_to_date, &[]);
        add("up to date, details", &Ends::up_to_date, &[KeyCode::Char('d')]);
        add("update", &Ends::update_available, &[]);
        add("update, the gate", &Ends::update_available, &[KeyCode::Enter]);
        add("writing", &Ends::update_available, &[KeyCode::Enter, KeyCode::Char('y')]);
        add("written", &|e| {
            e.update_available();
            e.written();
        }, &[]);
        add("failed", &|e| {
            e.update_available();
            e.failed();
        }, &[KeyCode::Char('d')]);
        let mut uiflow = ours("v1.13.2");
        uiflow.project = "UIFlow".into();
        let other = read("COM3", Some(uiflow));
        add("install", &|e| e.boards(vec![other.clone()]), &[]);
        add("install, the gate", &|e| e.boards(vec![other.clone()]), &[KeyCode::Enter]);
        add("silent", &|e| e.boards(vec![heard("COM3", Heard::Silent)]), &[]);
        add("newer", &|e| e.boards(vec![answering("COM3", "v0.9.0")]), &[]);
        let newer = || vec![answering("COM3", "v0.9.0")];
        add("go back, the gate", &|e| e.boards(newer()), &[KeyCode::Char('d'), KeyCode::Char('w')]);
        add("dev build", &|e| e.boards(vec![old("COM3", "v0.6.0-37-g221d99d")]), &[]);
        let no_card = answering_with("COM3", "v0.8.0", |s| {
            s.card = CardKind::None;
            s.size = None;
            s.tracks = Tracks::NoCard;
        });
        add("no card", &|e| e.boards(vec![no_card.clone()]), &[]);
        let exfat = answering_with("COM3", "v0.8.0", |s| s.card = CardKind::ExFat);
        add("exfat", &|e| e.boards(vec![exfat.clone()]), &[]);
        let uncounted = answering_with("COM3", "v0.8.0", |s| s.free = Free::NotCounted);
        add("not counted", &|e| e.boards(vec![uncounted.clone()]), &[]);
        let mut refused = uncounted.clone();
        refused.count = Count::Refused { why: CountWhy::Playing, part_way: false };
        add("count refused", &|e| e.boards(vec![refused.clone()]), &[]);
        add("two", &Ends::two, &[]);
        add("two, the other", &Ends::two, &[KeyCode::Right]);
        let in_use = heard("COM5", Heard::InUse { detail: "busy".into() });
        add("in use", &|e| e.boards(vec![answering("COM3", "v0.8.0"), in_use.clone()]), &[KeyCode::Right]);
        let mut chip = heard("COM9", Heard::Silent);
        chip.probe = Some(Err(DeviceError::WrongFlash { found: "4 MB".into() }));
        let chip = fixtures::judged(chip);
        add("not a core2", &|e| e.boards(vec![answering("COM3", "v0.8.0"), chip.clone()]), &[KeyCode::Right]);
        let silent_two = vec![answering("COM3", "v0.8.0"), heard("COM9", Heard::Silent)];
        add("silent, two", &|e| e.boards(silent_two.clone()), &[KeyCode::Right]);
        let seven: Vec<Board> = (0..7).map(|i| old(&format!("COM{}", i + 3), "v0.7.0")).collect();
        add("update all, the gate", &|e| e.boards(seven.clone()), &[KeyCode::Char('a')]);
        let pair = || vec![old("COM3", "v0.7.0"), old("COM5", "v0.7.0")];
        add("writing, looking", &|e| e.boards(pair()), &[KeyCode::Enter, KeyCode::Char('y'), KeyCode::Right]);
        add("writing, looking, details", &|e| e.boards(pair()), &[
            KeyCode::Enter,
            KeyCode::Char('y'),
            KeyCode::Right,
            KeyCode::Char('d'),
        ]);
        states
    }

    /// The labels the state's buttons wear — its primary and its gate's —
    /// in the locale the test runs.
    fn buttons(page: &Page) -> Vec<String> {
        if let Some(gate) = &page.gate {
            let (keep, write) = gate_buttons(gate);
            return vec![keep, write];
        }
        let Some(board) = page.shown() else { return Vec::new() };
        let Some(primary) = page.primary_of(board) else { return Vec::new() };
        let word = t!(match primary {
            Primary::Update => "dev.btn_update",
            Primary::Install => "dev.btn_install",
            Primary::Read => "dev.btn_read",
            Primary::TryAgain => "dev.retry",
        });
        vec![if page.writing() { word.to_string() } else { format!("{word} ▸") }]
    }

    #[test]
    fn at_the_guis_floor_and_on_the_console_every_state_keeps_its_buttons_whole_in_every_locale() {
        // The locale is the whole process's: drawn here in another language,
        // the page would switch it under every test running beside this
        // one. So each language runs in a process of its own — this test
        // binary again, running only the test below, its locale named in
        // the environment.
        let exe = std::env::current_exe().expect("the test binary");
        for (code, _) in crate::setup::LANGS {
            let out = std::process::Command::new(&exe)
                .args(["--exact", FLOOR_TEST, "--ignored"])
                .env(FLOOR_LOCALE, code)
                .output()
                .expect("the test binary runs again");
            let (said, err) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(out.status.success() && said.contains("1 passed"), "{code}:\n{said}{err}");
        }
    }

    /// The test below, by its full name, and the environment variable
    /// naming its locale.
    const FLOOR_TEST: &str = "device::page::tests::the_floor_in_this_processs_locale";
    const FLOOR_LOCALE: &str = "MSTREAM_TEST_FLOOR_LOCALE";

    #[test]
    #[ignore = "run by the every-locale floor test, a process per locale"]
    fn the_floor_in_this_processs_locale() {
        // The GUI's floor is 100×24: with its top bar and its footer the
        // page gets 22 rows from row 1, and 21 from row 2 under a pick's
        // banner (contract clause 4). Every state draws its frames whole
        // and its buttons whole there, nothing outside its area, and its
        // hint in 99 cells; the console page does the same at 72×24. Spaces
        // are left out of the comparison: a wide glyph's second cell is one.
        let Ok(code) = std::env::var(FLOOR_LOCALE) else { return };
        rust_i18n::set_locale(&code);
        crate::kit::theme::pin_modern_terminal();
        let banner = Rect { x: 0, y: 2, width: 100, height: 21 };
        for (name, mut page, _ends) in every_state() {
            for area in [FLOOR, banner] {
                let buf = hosted_at(&mut page, 100, 24, area);
                let all = text_of(&buf);
                let packed = all.replace(' ', "");
                let outside = (0..24u16)
                    .flat_map(|y| (0..100u16).map(move |x| Position { x, y }))
                    .filter(|at| !area.contains(*at))
                    .all(|at| buf[(at.x, at.y)].symbol() == "#");
                assert!(outside, "{code}, {name} in {area:?}: drawn outside its area:\n{all}");
                let first = row_of(&buf, area.y);
                assert!(first.trim_matches('#').trim().is_empty(), "{code}, {name}: the first row is the host's:\n{all}");
                assert_eq!(all.matches('╭').count(), all.matches('╰').count(), "{code}, {name} in {area:?}: frames whole:\n{all}");
                assert!(!all.contains("dev."), "{code}, {name}: a key with no words:\n{all}");
                for label in buttons(&page) {
                    let whole = packed.contains(&label.replace(' ', ""));
                    assert!(whole, "{code}, {name} in {area:?}: {label:?} whole:\n{all}");
                }
            }
            let hint = page.hint();
            assert!(kit::width(&hint) <= HINT_W, "{code}, {name}: the hint is {} cells: {hint}", kit::width(&hint));
            assert!(!hint.contains("dev."), "{code}, {name}: a key with no words: {hint}");
            let all = alone(&mut page, 72, 24);
            assert!(!all.contains("dev."), "{code}, {name}: a key with no words:\n{all}");
            // The console's gate is as wide as its card, and covers its corners.
            if page.gate.is_none() {
                let (tops, bottoms) = (all.matches('╭').count(), all.matches('╰').count());
                assert_eq!(tops, bottoms, "{code}, {name} at 72×24: frames whole:\n{all}");
            }
            let packed = all.replace(' ', "");
            for label in buttons(&page) {
                let whole = packed.contains(&label.replace(' ', ""));
                assert!(whole, "{code}, {name} at 72×24: {label:?} whole:\n{all}");
            }
        }
    }

    /// Not a check: every state at the GUI's floor and on the console,
    /// printed for a look against the design cards —
    /// `DUMP_LOCALE=de cargo test frames_for_a_look -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints frames for a person to read"]
    fn frames_for_a_look() {
        let code = std::env::var("DUMP_LOCALE").unwrap_or_else(|_| "en".into());
        rust_i18n::set_locale(&code);
        crate::kit::theme::pin_modern_terminal();
        for (name, mut page, _ends) in every_state() {
            println!("=== {name} (floor)");
            print!("{}", text_of(&hosted_at(&mut page, 100, 24, FLOOR)));
            println!("--- hint: {}", page.hint());
            println!("=== {name} (console)");
            print!("{}", alone(&mut page, 72, 24));
        }
    }

    #[test]
    fn hosted_short_of_room_the_page_cuts_itself_inside_its_area() {
        let _en = english();
        let areas = [
            Rect { x: 0, y: 2, width: 100, height: 21 },
            Rect { x: 0, y: 1, width: 100, height: 16 },
            Rect { x: 0, y: 1, width: 100, height: 9 },
            Rect { x: 0, y: 1, width: 100, height: 3 },
            Rect { x: 0, y: 1, width: 100, height: 2 },
            Rect { x: 0, y: 1, width: 100, height: 1 },
            Rect { x: 20, y: 1, width: 60, height: 22 },
            Rect { x: 0, y: 1, width: 12, height: 22 },
        ];
        for area in areas {
            for (name, mut page, _ends) in every_state() {
                let buf = hosted_at(&mut page, 100, 24, area);
                let all = text_of(&buf);
                let outside = (0..24u16)
                    .flat_map(|y| (0..100u16).map(move |x| Position { x, y }))
                    .filter(|at| !area.contains(*at))
                    .all(|at| buf[(at.x, at.y)].symbol() == "#");
                assert!(outside, "{name} in {area:?}: drawn outside its area:\n{all}");
                // A gate as wide as the card covers its corners.
                if page.gate.is_none() {
                    let (tops, bottoms) = (all.matches('╭').count(), all.matches('╰').count());
                    assert_eq!(tops, bottoms, "{name} in {area:?}: frames whole:\n{all}");
                }
            }
        }
    }

    #[test]
    fn a_hint_short_of_cells_drops_its_lesser_keys_first() {
        let _en = english();
        let (page, _ends) = page_with(|e| {
            let boards = vec![
                answering_with("COM3", "v0.8.0", |s| s.free = Free::NotCounted),
                old("COM5", "v0.7.0"),
                old("COM7", "v0.7.0"),
            ];
            e.boards(boards);
        });
        let full = page.hint_line(true, 200);
        assert_eq!(full, "←→ player · s show on the player · a update all · c count · d details · Esc library");
        assert_eq!(page.hint_line(true, 60), "←→ player · c count · d details · Esc library", "s and a first");
        assert_eq!(page.hint_line(true, 10), "←→ player · d details · Esc library", "never the switch, Details or Esc");
    }

    #[test]
    fn wrapping_is_by_words_in_cells_and_breaks_a_sentence_with_no_spaces() {
        let plain = Style::default();
        let lines = wrap_spans(&[("one two three".into(), plain)], 7);
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(text, ["one two", "three"]);
        let glued = wrap_spans(&[("v0.8.0".into(), plain), ("…".into(), bold())], 20);
        assert_eq!(glued[0].spans.len(), 2, "two styles, one word");
        let filled = wrap_spans(&[("MP3 プレーヤーの画面が暗く".into(), plain)], 12);
        let filled: Vec<String> = filled.iter().map(|l| l.to_string()).collect();
        assert_eq!(filled, ["MP3 プレーヤ", "ーの画面が暗", "く"], "a Japanese word fills the line it starts on");
        let cjk = wrap_spans(&[("ボードが応答しません".into(), plain)], 8);
        assert_eq!(cjk.iter().map(|l| l.to_string()).collect::<Vec<_>>(), ["ボードが", "応答しま", "せん"]);
        assert_eq!(fit("twelve chars", 8), "twelve …");
        assert_eq!(fit("short", 10), "short");
    }
}
