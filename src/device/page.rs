//! The flash page: `mstream-player device flash` without `--yes`, drawn on
//! the admin hub's terminal session (the kit's surface, the hub's loop,
//! its chrome). One worker thread (flow.rs) does everything slow; this
//! page shows what it reports, asks the one question — the board as read,
//! the firmware to put on it, erase first or not — and keeps the board's
//! fate honest: the write cannot be left, and leaving before it restarts
//! the board into what it had.
//!
//! Around the question, the page says where the run is (a step line:
//! Firmware · Board · Write · Restart), how long the write has left, and
//! what to do once it is done (music on the card, by hand). It keeps a
//! log of every report with a clock behind `l` — including the details
//! the busy line never says, which the worker sends as `Event::Log` —
//! and opens it by itself when something fails. With no board plugged
//! in it watches the ports every couple of seconds.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use rust_i18n::t;

use super::FlashArgs;
use super::engine::{self, DeviceInfo};
use super::firmware::{AppDesc, Origin};
use super::flow::{self, Cmd, Event, Kind, Phase, Plan};
use super::ports::Candidate;
use crate::admin::{Outcome, Screen, draw_bottom, draw_header_as, fmt_bytes, frame_ground};
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};

/// The card's five rows, the step line, the erase line, two tall buttons,
/// the logs toggle, the bottom lines — and the longest label the rows
/// carry.
const MIN_W: u16 = 72;
const MIN_H: u16 = 24;
const COLUMN_W: u16 = 78;
const LABEL_W: u16 = 15;
/// How often the page looks again while no board — or several — is
/// plugged in.
const WATCH: Duration = Duration::from_secs(2);
/// The write's time left is said once this much of it has gone by: the
/// first chunks are not a rate.
const ESTIMATE_AFTER: Duration = Duration::from_secs(2);
/// The clock's width in a log line: `mm:ss` and two spaces.
const CLOCK_W: usize = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    Go,
    Cancel,
    ToggleErase,
    Pick(usize),
    Rescan,
    Retry,
    Close,
    ToggleLogs,
}

/// Where the page is, in the order the worker moves it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// The firmware and the board are being found.
    Preparing,
    NoDevice,
    Several { list: Vec<Candidate>, cursor: usize },
    /// The board is being reached and read.
    Probing,
    /// The question.
    Confirm { on_board: Option<AppDesc>, plan: Plan, erase: bool },
    Working,
    /// Esc with the board held: the worker restarts it, then says so.
    Cancelling,
    Done { version: String, skipped: bool, boot: Option<String> },
    Failed { text: String, hint: Option<String> },
    Leaving,
}

/// The step line: one mark per thing that happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Done,
    Current,
    Todo,
    Failed,
}

/// How a log line is drawn: the phases in the accent, the facts plain, a
/// failure in gold, the quiet ones (a hint, the boot line) dim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Phase,
    Fact,
    Fail,
    Quiet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LogLine {
    /// Since the page opened.
    at: Duration,
    text: String,
    tone: Tone,
}

/// A fresh worker, for a retry.
type Respawn = Box<dyn Fn() -> (Sender<Cmd>, Receiver<Event>)>;

pub(crate) struct Page {
    cmds: Sender<Cmd>,
    events: Receiver<Event>,
    respawn: Respawn,
    step: Step,
    /// (version, origin) once the firmware is found; `kind` is the word
    /// the header puts beside the version.
    firmware: Option<(String, String)>,
    kind: Option<Origin>,
    download: Option<(u64, Option<u64>)>,
    board: Option<DeviceInfo>,
    phase: Option<Phase>,
    percent: Option<u8>,
    /// The write's first progress sample — when, at what percent — for
    /// the rate the time left is worked out from.
    write_from: Option<(Instant, u8)>,
    time_left: Option<u64>,
    /// The serial ports that are there when no Core2 is.
    others: Vec<String>,
    /// Which step the run failed in (the step line's ✗).
    failed_at: Option<usize>,
    note: Option<(String, bool)>,
    /// When the page opened: the log's clock.
    opened: Instant,
    log: Vec<LogLine>,
    logs_open: bool,
    /// Lines scrolled up from the newest; 0 follows it.
    log_scroll: usize,
    /// Where the log was drawn this frame, for the wheel.
    log_rect: Option<Rect>,
    /// The last `Progress` logged, so the log gets the tens only.
    last_logged_pct: Option<u8>,
    /// When the ports were last looked at, while the page waits for one.
    last_scan: Instant,
    ui: Surface<Act>,
}

pub(crate) fn run(args: FlashArgs) -> i32 {
    let respawn: Respawn = Box::new(move || {
        flow::spawn(Box::new(engine::from_env), args.source(), args.port.clone(), args.erase_asked())
    });
    let mut page = Page::new(respawn);
    crate::admin::run_tui_as(&mut page, "mStream MP3 Player")
}

impl Page {
    fn new(respawn: Respawn) -> Page {
        let (cmds, events) = respawn();
        Page::with_channels(cmds, events, respawn)
    }

    fn with_channels(cmds: Sender<Cmd>, events: Receiver<Event>, respawn: Respawn) -> Page {
        let now = Instant::now();
        Page {
            cmds,
            events,
            respawn,
            step: Step::Preparing,
            firmware: None,
            kind: None,
            download: None,
            board: None,
            phase: None,
            percent: None,
            write_from: None,
            time_left: None,
            others: Vec::new(),
            failed_at: None,
            note: None,
            opened: now,
            log: Vec::new(),
            logs_open: false,
            log_scroll: 0,
            log_rect: None,
            last_logged_pct: None,
            last_scan: now,
            ui: Surface::new(),
        }
    }

    /// Fold one of the worker's reports into the page.
    fn apply(&mut self, event: Event) {
        self.apply_at(event, Instant::now())
    }

    fn apply_at(&mut self, event: Event, now: Instant) {
        match event {
            Event::Phase(phase) => {
                self.phase = Some(phase);
                // The watch asks every couple of seconds; its scans are
                // not news.
                let watching = matches!(self.step, Step::NoDevice | Step::Several { .. });
                if !(phase == Phase::Scanning && watching) {
                    self.log_at(now, phase.text(), Tone::Phase);
                }
                if phase == Phase::Writing || phase == Phase::Comparing {
                    self.percent = None;
                    self.write_from = None;
                    self.time_left = None;
                    self.last_logged_pct = None;
                }
                let waiting = matches!(self.step, Step::Preparing | Step::NoDevice | Step::Several { .. });
                if waiting && matches!(phase, Phase::Connecting | Phase::Reading) {
                    self.step = Step::Probing;
                }
            }
            Event::Download { done, total } => self.download = Some((done, total)),
            Event::Firmware { version, origin, bytes, kind } => {
                self.download = None;
                self.log_at(now, format!("firmware: {version} ({origin}, {} KB)", bytes / 1024), Tone::Fact);
                self.firmware = Some((version, origin));
                self.kind = Some(kind);
            }
            Event::NoDevice { others } => {
                if self.board.is_some() || matches!(self.step, Step::Leaving | Step::Failed { .. } | Step::Done { .. }) {
                    return;
                }
                if self.step != Step::NoDevice {
                    let mut line = t!("dev.no_device_title").to_string();
                    if !others.is_empty() {
                        line.push_str(&format!(" — {} {}", t!("dev.ports_seen"), others.join(", ")));
                    }
                    self.log_at(now, line, Tone::Fact);
                    self.last_scan = now;
                }
                self.others = others;
                self.step = Step::NoDevice;
            }
            Event::Several(list) => {
                if self.board.is_some() || matches!(self.step, Step::Leaving | Step::Failed { .. } | Step::Done { .. }) {
                    return;
                }
                if let Step::Several { list: shown, cursor } = &mut self.step {
                    // The list follows the ports; the cursor stays where
                    // it was unless its row went away.
                    if *shown != list {
                        *cursor = (*cursor).min(list.len().saturating_sub(1));
                        *shown = list;
                    }
                } else {
                    let names: Vec<String> = list.iter().map(Candidate::describe).collect();
                    self.log_at(now, format!("{} {}", t!("dev.several_title"), names.join("; ")), Tone::Fact);
                    self.last_scan = now;
                    self.step = Step::Several { list, cursor: 0 };
                }
            }
            Event::Board(info) => {
                self.log_at(now, format!("board: {} · {} baud", info.describe(), info.baud), Tone::Fact);
                self.board = Some(info);
                if matches!(self.step, Step::Preparing | Step::NoDevice | Step::Several { .. }) {
                    self.step = Step::Probing;
                }
            }
            Event::Probed { on_board, plan } => {
                self.log_at(now, format!("{}: {}", t!("dev.on_board"), flow::on_board_text(on_board.as_ref())), Tone::Fact);
                self.log_at(now, format!("plan: {}", plan.describe()), Tone::Fact);
                // Esc while the board was being reached: the worker is now
                // waiting for the answer it will get — Quit.
                if self.step == Step::Cancelling {
                    let _ = self.cmds.send(Cmd::Quit);
                    return;
                }
                let erase = plan.erase;
                self.step = Step::Confirm { on_board, plan, erase };
            }
            Event::Progress(pct) => {
                self.percent = Some(pct);
                self.estimate(pct, now);
                if pct.is_multiple_of(10) && self.last_logged_pct != Some(pct) {
                    self.last_logged_pct = Some(pct);
                    let mut line = format!("{pct}%");
                    if let Some(secs) = self.time_left.filter(|_| pct < 100) {
                        line.push_str(&format!(" · {}", time_left_text(secs)));
                    }
                    self.log_at(now, line, Tone::Fact);
                }
            }
            Event::Done { version, skipped, boot } => {
                self.phase = None;
                if skipped {
                    self.log_at(now, t!("dev.done_skipped").to_string(), Tone::Fact);
                }
                self.log_at(now, t!("dev.done_body", version = version).to_string(), Tone::Fact);
                if let Some(line) = &boot {
                    self.log_at(now, t!("dev.done_booted", line = line).to_string(), Tone::Quiet);
                }
                self.step = if self.step == Step::Cancelling {
                    Step::Leaving
                } else {
                    Step::Done { version, skipped, boot }
                };
            }
            Event::Cancelled => {
                self.log_at(now, t!("dev.cancelled").to_string(), Tone::Fact);
                self.step = Step::Leaving;
            }
            Event::Failed(e) => {
                self.phase = None;
                self.failed_at = Some(self.position());
                self.log_at(now, e.text(), Tone::Fail);
                if let Some(hint) = e.hint() {
                    self.log_at(now, hint, Tone::Quiet);
                }
                // The log is why it failed; nobody should have to find the key.
                self.logs_open = true;
                self.step = if self.step == Step::Cancelling {
                    Step::Leaving
                } else {
                    Step::Failed { text: e.text(), hint: e.hint() }
                };
            }
            Event::Log(text) => self.log_at(now, text, Tone::Fact),
        }
    }

    fn log_at(&mut self, now: Instant, text: String, tone: Tone) {
        self.log.push(LogLine { at: now.saturating_duration_since(self.opened), text, tone });
    }

    /// The write's time left, from the rate since the first sample: said
    /// once a couple of seconds and some percent have gone by, to the
    /// nearest five seconds.
    fn estimate(&mut self, pct: u8, now: Instant) {
        match self.write_from {
            None if pct > 0 && pct < 100 => self.write_from = Some((now, pct)),
            Some((since, from)) if pct > from => {
                let gone = now.saturating_duration_since(since);
                if gone >= ESTIMATE_AFTER {
                    let rate = f64::from(pct - from) / gone.as_secs_f64();
                    let left = f64::from(100 - pct) / rate;
                    self.time_left = Some(((left / 5.0).round() * 5.0) as u64);
                }
            }
            _ => {}
        }
    }

    /// Which of the four things the run is at: the step line's ▸, and the
    /// ✗ when it fails.
    fn position(&self) -> usize {
        match (&self.step, self.phase) {
            (Step::Done { .. }, _) => 3,
            (Step::Working, Some(Phase::Restarting)) | (Step::Cancelling, _) => 3,
            (Step::Working, _) | (Step::Confirm { .. }, _) => 2,
            (Step::NoDevice, _) | (Step::Several { .. }, _) | (Step::Probing, _) => 1,
            (Step::Preparing, Some(Phase::Scanning | Phase::Connecting | Phase::Reading)) => 1,
            _ => 0,
        }
    }

    fn marks(&self) -> [Mark; 4] {
        let mut marks = [Mark::Todo; 4];
        match &self.step {
            Step::Done { .. } => return [Mark::Done; 4],
            Step::Failed { .. } => {
                let at = self.failed_at.unwrap_or(0).min(3);
                for mark in marks.iter_mut().take(at) {
                    *mark = Mark::Done;
                }
                marks[at] = Mark::Failed;
                return marks;
            }
            // The board is being restarted untouched: the write never happened.
            Step::Cancelling => return [Mark::Done, Mark::Done, Mark::Todo, Mark::Current],
            _ => {}
        }
        let at = self.position();
        for mark in marks.iter_mut().take(at) {
            *mark = Mark::Done;
        }
        marks[at] = Mark::Current;
        marks
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Go => {
                if let Step::Confirm { erase, plan, .. } = &self.step {
                    let erase = *erase;
                    let plan = Plan { kind: plan.kind.clone(), erase };
                    if self.cmds.send(Cmd::Go { erase }).is_ok() {
                        self.log_at(Instant::now(), t!("dev.log_go", plan = plan.describe()).to_string(), Tone::Fact);
                        self.step = Step::Working;
                    } else {
                        self.worker_gone();
                    }
                }
            }
            Act::Cancel => self.leave(),
            Act::ToggleErase => {
                if let Step::Confirm { erase, .. } = &mut self.step {
                    *erase = !*erase;
                }
            }
            Act::Pick(i) => {
                if let Step::Several { list, .. } = &self.step
                    && let Some(board) = list.get(i)
                {
                    let port = board.port.clone();
                    self.step = Step::Probing;
                    if self.cmds.send(Cmd::Pick(port)).is_err() {
                        self.worker_gone();
                    }
                }
            }
            Act::Rescan => {
                if matches!(self.step, Step::NoDevice | Step::Several { .. }) {
                    self.last_scan = Instant::now();
                    if self.cmds.send(Cmd::Rescan).is_err() {
                        self.worker_gone();
                    }
                }
            }
            Act::Retry => {
                if matches!(self.step, Step::Failed { .. }) {
                    let (cmds, events) = (self.respawn)();
                    self.cmds = cmds;
                    self.events = events;
                    self.log_at(Instant::now(), t!("dev.log_again").to_string(), Tone::Fact);
                    self.step = Step::Preparing;
                    self.board = None;
                    self.phase = None;
                    self.percent = None;
                    self.write_from = None;
                    self.time_left = None;
                    self.failed_at = None;
                    self.note = None;
                }
            }
            Act::Close => self.leave(),
            Act::ToggleLogs => {
                self.logs_open = !self.logs_open;
                self.log_scroll = 0;
            }
        }
        None
    }

    /// Esc, Cancel, Close. A board held in its bootloader (the question,
    /// or on the way to it) is restarted first, and the page waits for the
    /// worker to say so; a write is never left; anything else ends now.
    fn leave(&mut self) {
        match self.step {
            Step::Working => {}
            Step::Confirm { .. } | Step::Probing => {
                self.step = Step::Cancelling;
                if self.cmds.send(Cmd::Quit).is_err() {
                    self.step = Step::Leaving;
                }
            }
            Step::Cancelling | Step::Leaving => {}
            _ => {
                let _ = self.cmds.send(Cmd::Quit);
                self.step = Step::Leaving;
            }
        }
    }

    fn worker_gone(&mut self) {
        self.failed_at = Some(self.position());
        self.logs_open = true;
        self.step = Step::Failed { text: t!("note.worker_gone").to_string(), hint: None };
    }

    /// The write's words: the phase, the percent, the time left.
    fn write_words(&self) -> String {
        let mut text = Phase::Writing.text();
        if let Some(pct) = self.percent {
            text.push_str(&format!(" {pct}%"));
            if let Some(secs) = self.time_left.filter(|_| pct < 100) {
                text.push_str(&format!(" · {}", time_left_text(secs)));
            }
        }
        text
    }

    /// The busy line: the phase, with the download's or the write's
    /// percent — or, while the page watches the ports, that it does.
    fn busy(&self) -> Option<String> {
        match self.step {
            Step::NoDevice => return Some(t!("dev.watching").to_string()),
            Step::Cancelling => return Some(Phase::Restarting.text()),
            Step::Preparing | Step::Probing | Step::Working => {}
            _ => return None,
        }
        let phase = self.phase?;
        let text = match phase {
            Phase::Firmware => {
                let mut text = phase.text();
                if let Some((done, Some(total))) = self.download
                    && total > 0
                {
                    text.push_str(&format!(" {}%", (done * 100 / total).min(100)));
                }
                text
            }
            Phase::Writing => self.write_words(),
            _ => phase.text(),
        };
        Some(text)
    }

    fn tips(&self) -> String {
        let key = match self.step {
            Step::Preparing | Step::Probing | Step::Cancelling => "dev.hint_wait",
            Step::NoDevice => "dev.hint_none",
            Step::Several { .. } => "dev.hint_several",
            Step::Confirm { .. } => "dev.hint_confirm",
            Step::Working => "dev.hint_working",
            Step::Done { .. } => "dev.hint_done",
            Step::Failed { .. } => "dev.hint_failed",
            Step::Leaving => "dev.hint_wait",
        };
        t!(key).to_string()
    }
}

/// `about 20 s left`, or `a few seconds left` under five.
fn time_left_text(secs: u64) -> String {
    if secs < 5 { t!("dev.time_left_few").to_string() } else { t!("dev.time_left", secs = secs).to_string() }
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
                    // A worker that ended without a last word: only a bug
                    // does that; the page says so instead of hanging.
                    if matches!(self.step, Step::Preparing | Step::Probing | Step::Working | Step::Cancelling) {
                        self.worker_gone();
                    }
                    return;
                }
            }
        }
    }

    /// The watch: while no board (or several) is plugged in, ask the
    /// worker to look again every couple of seconds, so plugging the
    /// Core2 in — or unplugging the wrong one — is all it takes.
    fn tick(&mut self) {
        if matches!(self.step, Step::NoDevice | Step::Several { .. }) && self.last_scan.elapsed() >= WATCH {
            self.last_scan = Instant::now();
            if self.cmds.send(Cmd::Rescan).is_err() {
                self.worker_gone();
            }
        }
    }

    fn finished(&self) -> Option<Outcome> {
        (self.step == Step::Leaving).then_some(Outcome::Quit)
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        match key.code {
            KeyCode::Esc => self.leave(),
            KeyCode::Enter => {
                let act = match &self.step {
                    Step::Confirm { .. } => Some(Act::Go),
                    Step::NoDevice => Some(Act::Rescan),
                    Step::Several { cursor, .. } => Some(Act::Pick(*cursor)),
                    Step::Done { .. } | Step::Failed { .. } => Some(Act::Close),
                    _ => None,
                };
                if let Some(act) = act {
                    return self.act(act);
                }
            }
            KeyCode::Char('e') => return self.act(Act::ToggleErase),
            KeyCode::Char('l') => return self.act(Act::ToggleLogs),
            KeyCode::Char('r') => {
                let act = match self.step {
                    Step::NoDevice | Step::Several { .. } => Some(Act::Rescan),
                    Step::Failed { .. } => Some(Act::Retry),
                    _ => None,
                };
                if let Some(act) = act {
                    return self.act(act);
                }
            }
            KeyCode::Up | KeyCode::Down => {
                if let Step::Several { list, cursor } = &mut self.step {
                    let last = list.len().saturating_sub(1);
                    *cursor = if key.code == KeyCode::Up { cursor.saturating_sub(1) } else { (*cursor + 1).min(last) };
                }
            }
            _ => {}
        }
        None
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        Page::act(self, act)
    }

    /// Over the log, the wheel scrolls it (up stops the follow, back down
    /// to the newest line resumes it); elsewhere it moves the pick.
    fn wheel(&mut self, up: bool, at: Position) {
        if self.logs_open
            && let Some(rect) = self.log_rect
            && rect.contains(at)
        {
            let rows = usize::from(rect.height);
            let most = self.log.len().saturating_sub(rows);
            self.log_scroll = if up { (self.log_scroll + 1).min(most) } else { self.log_scroll.saturating_sub(1) };
            return;
        }
        if let Step::Several { list, cursor } = &mut self.step {
            let last = list.len().saturating_sub(1);
            *cursor = if up { cursor.saturating_sub(1) } else { (*cursor + 1).min(last) };
        }
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, page: &mut Page) {
    page.ui.begin_frame();
    page.log_rect = None;
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    let right = match (&page.firmware, page.kind) {
        (Some((version, _)), Some(kind)) => {
            let word = t!(match kind {
                Origin::Release => "dev.kind_release",
                Origin::Cached => "dev.kind_cached",
                Origin::File => "dev.kind_file",
            });
            t!("dev.head_firmware", version = version, kind = word).to_string()
        }
        _ => String::new(),
    };
    draw_header_as(frame, area, &t!("dev.title"), &right);

    let column_w = area.width.saturating_sub(4).min(COLUMN_W);
    let x = area.x + (area.width - column_w) / 2;
    let mut y = area.y + 2;
    y = wrapped(frame, x, y, column_w, &t!("dev.subtitle"), dim()) + 1;
    if page.step != Step::Leaving {
        frame.render_widget(Paragraph::new(steps_line(page.marks())), Rect { x, y, width: column_w, height: 1 });
        y += 2;
    }

    match page.step.clone() {
        Step::Preparing | Step::Probing | Step::Cancelling => {
            if let Some(board) = &page.board {
                y = card(frame, x, y, column_w, board, page.firmware.as_ref(), None);
            } else if page.phase == Some(Phase::Firmware)
                && let Some((done, Some(total))) = page.download
                && total > 0
            {
                // The download on the bar the write uses, its size under it.
                let pct = ((done * 100) / total).min(100) as u8;
                y = bar(frame, x, y, column_w, &format!("{} {pct}%", Phase::Firmware.text()), pct);
                let sizes = t!("dev.download_of", done = fmt_bytes(done), total = fmt_bytes(total)).to_string();
                y = line(frame, x, y, column_w, &sizes, dim());
            }
        }
        Step::NoDevice => {
            let title = t!("dev.no_device_title").to_string();
            let watch = format!(" — {}", t!("dev.no_device_watch"));
            y = wrapped_spans(
                frame,
                x,
                y,
                column_w,
                vec![Span::styled(title, gold_bold()), Span::raw(watch)],
            ) + 1;
            for key in ["dev.no_device_cable", "dev.no_device_driver"] {
                y = wrapped(frame, x, y, column_w, &format!("• {}", t!(key)), dim());
            }
            // The driver hint's second line, indented under its bullet: drawn
            // whole, since a wrap would trim the indent away.
            y = line(frame, x, y, column_w, &format!("  {}", t!("dev.no_device_driver_2")), dim());
            y = wrapped(frame, x, y, column_w, &format!("• {}", t!("dev.no_device_linux")), dim()) + 1;
            let seen = if page.others.is_empty() {
                vec![
                    Span::styled(format!("{} ", t!("dev.ports_seen")), dim()),
                    Span::styled(t!("dev.ports_seen_none").to_string(), dim()),
                ]
            } else {
                vec![
                    Span::styled(format!("{} ", t!("dev.ports_seen")), dim()),
                    Span::raw(page.others.join(", ")),
                    Span::styled(format!(" {}", t!("dev.ports_not_bridge")), dim()),
                ]
            };
            y = wrapped_spans(frame, x, y, column_w, seen) + 1;
            let close = t!("dev.close");
            y = buttons(frame, page, x, y, column_w, &t!("dev.rescan"), Act::Rescan, Some((&close, Act::Close)));
        }
        Step::Several { list, cursor } => {
            y = line(frame, x, y, column_w, &t!("dev.several_title"), bold()) + 1;
            for (i, board) in list.iter().enumerate() {
                let rect = Rect { x, y, width: column_w, height: 1 };
                let hovered = page.ui.hovers(rect);
                let style = if hovered {
                    Style::default().fg(th().bright)
                } else if i == cursor {
                    accent()
                } else {
                    Style::default()
                };
                let marker = if i == cursor { "▸ " } else { "  " };
                let text = format!("{marker}{}", board.describe());
                frame.render_widget(Paragraph::new(Span::styled(text, style)), rect);
                page.ui.click(rect, Act::Pick(i));
                y += 1;
            }
            y = wrapped(frame, x, y + 1, column_w, &t!("dev.several_follow"), dim());
        }
        Step::Confirm { on_board, plan, erase } => {
            let board = page.board.as_ref().expect("a board before the question");
            y = card(frame, x, y, column_w, board, page.firmware.as_ref(), Some(&on_board));
            y = erase_row(frame, page, x, y + 1, column_w, erase, &plan) + 1;
            let cancel = t!("dev.cancel");
            y = buttons(frame, page, x, y, column_w, &plan.verb(), Act::Go, Some((&cancel, Act::Cancel)));
        }
        Step::Working => {
            if let Some(board) = &page.board {
                y = card(frame, x, y, column_w, board, page.firmware.as_ref(), None) + 1;
            }
            let (words, filled) = match (page.phase, page.percent) {
                (Some(Phase::Writing), Some(pct)) => (page.write_words(), pct),
                (Some(Phase::Verifying | Phase::Restarting), _) => (page.phase.map(Phase::text).unwrap_or_default(), 100),
                (Some(phase), _) => (phase.text(), 0),
                (None, _) => (String::new(), 0),
            };
            y = bar(frame, x, y, column_w, &words, filled);
        }
        Step::Done { version, skipped, boot } => {
            y = line(frame, x, y, column_w, &t!("dev.done_title"), accent().add_modifier(Modifier::BOLD)) + 1;
            if skipped {
                y = wrapped(frame, x, y, column_w, &t!("dev.done_skipped"), Style::default());
            }
            y = wrapped(frame, x, y, column_w, &t!("dev.done_body", version = version), Style::default());
            if let Some(boot) = boot {
                y = wrapped(frame, x, y, column_w, &t!("dev.done_booted", line = boot), dim());
            }
            y = next_block(frame, x, y, column_w) + 1;
            y = buttons(frame, page, x, y, column_w, &t!("dev.close"), Act::Close, None);
        }
        Step::Failed { text, hint } => {
            y = line(frame, x, y, column_w, &t!("dev.failed_title"), gold_bold()) + 1;
            y = wrapped(frame, x, y, column_w, &text, Style::default().fg(th().gold));
            if let Some(hint) = hint {
                y = wrapped(frame, x, y, column_w, &hint, dim());
            }
            let close = t!("dev.close");
            y = buttons(frame, page, x, y + 1, column_w, &t!("dev.retry"), Act::Retry, Some((&close, Act::Close)));
        }
        Step::Leaving => {}
    }

    if page.step != Step::Leaving {
        logs(frame, page, area, x, y + 1, column_w);
    }

    let busy = page.busy();
    draw_bottom(frame, area, page.note.as_ref(), busy.as_deref(), &page.tips());

    if let Some((target, text)) = page.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// A title that warns: the theme's gold, bold.
fn gold_bold() -> Style {
    Style::default().fg(th().gold).add_modifier(Modifier::BOLD)
}

/// The step line: `✓ Firmware   ✓ Board   ▸ Write   Restart` — done in the
/// kit's ok colour, the current one in the accent and bold, the rest dim,
/// the one that failed in gold.
fn steps_line(marks: [Mark; 4]) -> Line<'static> {
    let names = ["dev.step_firmware", "dev.step_board", "dev.step_write", "dev.step_restart"];
    let mut spans = Vec::new();
    for (i, (mark, key)) in marks.iter().zip(names).enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        let name = t!(key).to_string();
        let (glyph, style) = match mark {
            Mark::Done => ("✓ ", Style::default().fg(th().ok)),
            Mark::Current => ("▸ ", accent().add_modifier(Modifier::BOLD)),
            Mark::Todo => ("", dim()),
            Mark::Failed => ("✗ ", gold_bold()),
        };
        spans.push(Span::styled(format!("{glyph}{name}"), style));
    }
    Line::from(spans)
}

/// One styled line; returns the next row.
fn line(frame: &mut Frame, x: u16, y: u16, w: u16, text: &str, style: Style) -> u16 {
    frame.render_widget(Paragraph::new(Span::styled(text.to_string(), style)), Rect { x, y, width: w, height: 1 });
    y + 1
}

/// Word-wrapped text; returns the row after it.
fn wrapped(frame: &mut Frame, x: u16, y: u16, w: u16, text: &str, style: Style) -> u16 {
    let rows = kit::wrap_words(text, w as usize).len().max(1) as u16;
    frame.render_widget(
        Paragraph::new(Span::styled(text.to_string(), style)).wrap(Wrap { trim: true }),
        Rect { x, y, width: w, height: rows },
    );
    y + rows
}

/// Word-wrapped spans (a title in one colour, the rest in another);
/// returns the row after them.
fn wrapped_spans(frame: &mut Frame, x: u16, y: u16, w: u16, spans: Vec<Span<'static>>) -> u16 {
    let plain: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let rows = kit::wrap_words(&plain, w as usize).len().max(1) as u16;
    frame.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }),
        Rect { x, y, width: w, height: rows },
    );
    y + rows
}

/// The board as read, the port, what is on it (when asked), what will go
/// on it, and the card that stays out of all this. Returns the row after.
fn card(
    frame: &mut Frame,
    x: u16,
    mut y: u16,
    w: u16,
    board: &DeviceInfo,
    firmware: Option<&(String, String)>,
    on_board: Option<&Option<AppDesc>>,
) -> u16 {
    let row = |frame: &mut Frame, y: u16, label: &str, value: &str| -> u16 {
        frame.render_widget(
            Paragraph::new(Span::styled(label.to_string(), dim())),
            Rect { x, y, width: LABEL_W, height: 1 },
        );
        wrapped(frame, x + LABEL_W, y, w.saturating_sub(LABEL_W), value, Style::default())
    };
    let rev = board.revision.map(|(a, b)| format!("{a}.{b}")).unwrap_or_else(|| "?".to_string());
    let flash = board.flash_mb.map(|mb| mb.to_string()).unwrap_or_else(|| "?".to_string());
    y = row(frame, y, &t!("dev.board"), &t!("dev.board_line", chip = board.chip, rev = rev, flash = flash));
    let mut port = format!("{} · {}", board.port, board.bridge);
    if board.baud != engine::SYNC_BAUD {
        port.push_str(&format!(" · {} baud", board.baud));
    }
    y = row(frame, y, &t!("dev.port"), &port);
    if let Some(on_board) = on_board {
        y = row(frame, y, &t!("dev.on_board"), &flow::on_board_text(on_board.as_ref()));
    }
    if let Some((version, origin)) = firmware {
        y = row(frame, y, &t!("dev.to_install"), &format!("{version} · {origin}"));
    }
    y = row(frame, y, &t!("dev.sd_card"), &t!("dev.sd_card_text"));
    y
}

/// `[x] Erase the whole flash first — …`: the checkbox, clickable, `e`.
fn erase_row(frame: &mut Frame, page: &mut Page, x: u16, y: u16, w: u16, erase: bool, plan: &Plan) -> u16 {
    let text = format!(
        "[{}] {}",
        if erase { "x" } else { " " },
        match plan.kind {
            Kind::Install { .. } => t!("dev.erase_box_install"),
            _ => t!("dev.erase_box_update"),
        }
    );
    let rows = kit::wrap_words(&text, w as usize).len().max(1) as u16;
    let rect = Rect { x, y, width: w, height: rows };
    let style = if page.ui.hovers(rect) { Style::default().fg(th().bright) } else { Style::default() };
    frame.render_widget(Paragraph::new(Span::styled(text, style)).wrap(Wrap { trim: true }), rect);
    page.ui.click(rect, Act::ToggleErase);
    page.ui.tip_keyed(rect, t!("dev.erase_tip"));
    y + rows
}

/// What to do once the board runs the firmware: music on the card, by
/// hand — the card's rules in four lines. Returns the row after.
fn next_block(frame: &mut Frame, x: u16, mut y: u16, w: u16) -> u16 {
    y = line(frame, x, y, w, &t!("dev.next_title"), bold());
    for key in ["dev.next_card", "dev.next_layout", "dev.next_cover"] {
        y = wrapped(frame, x, y, w, &t!(key), Style::default());
    }
    wrapped(frame, x, y, w, &t!("dev.next_link"), dim())
}

/// The primary and, beside it, the secondary. Always enabled: every step
/// that draws buttons is one where each of them can be pressed. Returns
/// the row after the buttons.
#[allow(clippy::too_many_arguments)]
fn buttons(
    frame: &mut Frame,
    page: &mut Page,
    x: u16,
    y: u16,
    w: u16,
    primary: &str,
    act: Act,
    secondary: Option<(&str, Act)>,
) -> u16 {
    let at = Rect { x, y, width: w, height: 3 };
    let rect = kit::tall_button(frame, &mut page.ui, at, primary, true, act);
    if let Some((label, act)) = secondary {
        let at = Rect { x: rect.x + rect.width + 2, y, width: w.saturating_sub(rect.width + 2), height: 3 };
        kit::tall_secondary(frame, &mut page.ui, at, label, act);
    }
    y + 3
}

/// A bar: its words above it, a filled share in the accent, the rest as
/// a track. Drawn by hand rather than with ratatui's Gauge, whose
/// remainder swaps the colors (the player's own reason). Returns the row
/// after the bar.
fn bar(frame: &mut Frame, x: u16, y: u16, w: u16, words: &str, percent: u8) -> u16 {
    frame.render_widget(Paragraph::new(Span::styled(words.to_string(), accent())), Rect { x, y, width: w, height: 1 });
    let filled = (u32::from(w) * u32::from(percent.min(100)) / 100) as u16;
    let track = Rect { x, y: y + 1, width: w, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled("░".repeat(w as usize), dim())), track);
    if filled > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled("█".repeat(filled as usize), accent())),
            Rect { x, y: y + 1, width: filled, height: 1 },
        );
    }
    y + 2
}

/// The logs: a text button under the content (`▸ Show logs`, dim; bright
/// under the pointer) and, open, the newest lines that fit above the
/// bottom edge — `mm:ss`, then the line in its tone. Drawn only when the
/// window has a row for the toggle; `l` works either way.
fn logs(frame: &mut Frame, page: &mut Page, area: Rect, x: u16, y: u16, w: u16) {
    let bottom = area.y + area.height.saturating_sub(2);
    if y + 1 > bottom {
        return;
    }
    let label = t!(if page.logs_open { "dev.logs_hide" } else { "dev.logs_show" }).to_string();
    let rect = Rect { x, y, width: (label.chars().count() as u16).min(w), height: 1 };
    let style = if page.ui.hovers(rect) { Style::default().fg(th().bright) } else { dim() };
    frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
    page.ui.click(rect, Act::ToggleLogs);
    if !page.logs_open {
        return;
    }
    let rows = usize::from(bottom.saturating_sub(y + 1));
    if rows == 0 {
        return;
    }
    let pane = Rect { x, y: y + 1, width: w, height: rows as u16 };
    page.log_rect = Some(pane);
    let end = page.log.len().saturating_sub(page.log_scroll);
    let start = end.saturating_sub(rows);
    let width = usize::from(w).saturating_sub(CLOCK_W);
    for (i, entry) in page.log[start..end].iter().enumerate() {
        let secs = entry.at.as_secs();
        let clock = format!("{:02}:{:02}  ", secs / 60, secs % 60);
        let text = fit(&entry.text, width);
        let style = match entry.tone {
            Tone::Phase => accent(),
            Tone::Fact => Style::default(),
            Tone::Fail => Style::default().fg(th().gold),
            Tone::Quiet => dim(),
        };
        let row = Rect { x, y: pane.y + i as u16, width: w, height: 1 };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(clock, dim()), Span::styled(text, style)])),
            row,
        );
    }
}

/// `text`, cut to `width` cells with an ellipsis: a log line never wraps.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::channel;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use crate::device::DeviceError;

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// A page on channels the test holds both ends of: `cmds` reads what
    /// the page sends the worker, `events` feeds it the worker's reports.
    /// The respawn counts how often a retry asked for a fresh worker.
    struct Rig {
        page: Page,
        cmds: Receiver<Cmd>,
        events: Sender<Event>,
        respawns: Arc<AtomicUsize>,
    }

    fn rig() -> Rig {
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        let respawns = Arc::new(AtomicUsize::new(0));
        let counter = respawns.clone();
        let respawn: Respawn = Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            channel_pair()
        });
        Rig { page: Page::with_channels(cmd_tx, event_rx, respawn), cmds: cmd_rx, events: event_tx, respawns }
    }

    fn channel_pair() -> (Sender<Cmd>, Receiver<Event>) {
        let (cmd_tx, _cmd_rx) = channel();
        let (_event_tx, event_rx) = channel();
        // Both far ends dropped: a retry's worker that never speaks, which
        // the page reports as gone — enough to see the retry happened.
        (cmd_tx, event_rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn draw_at(page: &mut Page, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| render(frame, page)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn draw(page: &mut Page) -> String {
        draw_at(page, 100, 30)
    }

    fn board() -> DeviceInfo {
        DeviceInfo {
            port: "COM3".into(),
            bridge: "CH9102".into(),
            chip: "esp32".into(),
            revision: Some((3, 1)),
            flash_mb: Some(16),
            baud: 921_600,
        }
    }

    fn ours(version: &str) -> AppDesc {
        AppDesc { version: version.into(), project: AppDesc::OURS.into(), idf: "v5.5.5".into(), elf8: "00".into() }
    }

    fn firmware() -> Event {
        Event::Firmware {
            version: "v0.5.0".into(),
            origin: "release v0.5.0".into(),
            bytes: 2_289_360,
            kind: Origin::Release,
        }
    }

    fn to_question(r: &mut Rig, on_board: Option<AppDesc>) {
        for event in [
            Event::Phase(Phase::Firmware),
            firmware(),
            Event::Phase(Phase::Scanning),
            Event::Phase(Phase::Connecting),
            Event::Board(board()),
            Event::Phase(Phase::Reading),
        ] {
            r.events.send(event).unwrap();
        }
        let plan = flow::plan(on_board.as_ref(), "v0.5.0", None);
        r.events.send(Event::Probed { on_board, plan }).unwrap();
        r.page.pump();
    }

    #[test]
    fn the_page_shows_the_phases_then_the_board_and_asks_the_question() {
        let _en = english();
        let mut r = rig();
        r.events.send(Event::Phase(Phase::Firmware)).unwrap();
        r.events.send(Event::Download { done: 572_340, total: Some(2_289_360) }).unwrap();
        r.page.pump();
        assert_eq!(r.page.busy().as_deref(), Some("getting the firmware… 25%"));
        let frame = draw(&mut r.page);
        assert!(frame.contains("▸ Firmware   Board   Write   Restart"), "the step line, at the firmware:\n{frame}");
        assert!(frame.contains("getting the firmware… 25%") && frame.contains("559 KB of 2.2 MB"), "the download on the bar:\n{frame}");
        to_question(&mut r, Some(ours("v0.4.0")));
        let update = Kind::Update { from: "v0.4.0".into() };
        assert!(matches!(r.page.step, Step::Confirm { ref plan, erase: false, .. } if plan.kind == update));
        let frame = draw(&mut r.page);
        assert!(frame.contains("mStream MP3 Player"), "{frame}");
        assert!(frame.contains("firmware v0.5.0 · release"), "the header's right edge says what it is:\n{frame}");
        assert!(frame.contains("✓ Firmware   ✓ Board   ▸ Write   Restart"), "{frame}");
        assert!(frame.contains("COM3 · CH9102 · 921600 baud"), "{frame}");
        assert!(frame.contains("mstream-mp3-player v0.4.0"), "what is on the board:\n{frame}");
        assert!(frame.contains("SD card        untouched"), "the card stays out of it:\n{frame}");
        assert!(frame.contains("Update ▸") && frame.contains("Cancel"), "{frame}");
        assert!(frame.contains("[ ] "), "no erase over our own firmware:\n{frame}");
        assert!(frame.contains("▸ Show logs"), "the toggle, closed:\n{frame}");
        assert!(!frame.contains("Hide logs"), "{frame}");
        assert!(r.page.busy().is_none(), "the question is not busy");

        r.page.key(key(KeyCode::Enter));
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Go { erase: false }));
        assert_eq!(r.page.step, Step::Working);
        r.events.send(Event::Phase(Phase::Writing)).unwrap();
        r.events.send(Event::Progress(42)).unwrap();
        r.page.pump();
        assert_eq!(r.page.busy().as_deref(), Some("writing… 42%"));
        let frame = draw(&mut r.page);
        assert!(frame.contains("writing… 42%"), "{frame}");
        assert!(frame.contains("█") && frame.contains("░"), "the bar:\n{frame}");
        r.page.key(key(KeyCode::Esc));
        assert_eq!(r.page.step, Step::Working, "a write cannot be left");

        let boot = Some("mstream-mp3-player v0.5.0 (commit a, 2026-10-01), ELF 0".to_string());
        r.events.send(Event::Done { version: "v0.5.0".into(), skipped: false, boot }).unwrap();
        r.page.pump();
        let frame = draw(&mut r.page);
        assert!(frame.contains("Done") && frame.contains("v0.5.0 is on the board"), "{frame}");
        assert!(frame.contains("✓ Firmware   ✓ Board   ✓ Write   ✓ Restart"), "{frame}");
        assert!(frame.contains("The board reports"), "{frame}");
        assert!(frame.contains("Next: music on the card") && frame.contains("/music/Artist/Album/NN - Title.mp3"), "what to do next:\n{frame}");
        assert!(r.page.finished().is_none());
        r.page.key(key(KeyCode::Enter));
        assert!(matches!(r.page.finished(), Some(Outcome::Quit)));
    }

    #[test]
    fn l_opens_the_log_with_every_report_and_a_clock() {
        let _en = english();
        let mut r = rig();
        to_question(&mut r, Some(ours("v0.4.0")));
        r.events.send(Event::Log("921600 baud: the bootloader answered".into())).unwrap();
        r.page.pump();
        r.page.key(key(KeyCode::Char('l')));
        let frame = draw(&mut r.page);
        assert!(frame.contains("▾ Hide logs"), "{frame}");
        // Eight rows fit under the question at 100×30: the newest eight
        // of the nine lines so far, the first scrolled off.
        assert!(frame.contains("00:00  looking for a Core2…"), "the phases, with the clock:\n{frame}");
        assert!(!frame.contains("getting the firmware…"), "the oldest line is out of the rows that fit:\n{frame}");
        assert!(frame.contains("firmware: v0.5.0 (release v0.5.0, 2235 KB)"), "{frame}");
        assert!(frame.contains("board: COM3 · CH9102 · esp32 rev 3.1 · 16 MB · 921600 baud"), "{frame}");
        assert!(frame.contains("plan: update, no erase"), "{frame}");
        assert!(frame.contains("921600 baud: the bootloader answered"), "the worker's detail lines:\n{frame}");
        assert!(frame.contains("Update ▸"), "the question stays above the log:\n{frame}");
        r.page.key(key(KeyCode::Enter));
        r.page.pump();
        let frame = draw(&mut r.page);
        assert!(frame.contains("write: update, no erase"), "Enter is logged:\n{frame}");
        r.page.key(key(KeyCode::Char('l')));
        let frame = draw(&mut r.page);
        assert!(frame.contains("▸ Show logs") && !frame.contains("plan: update"), "closed again:\n{frame}");
    }

    #[test]
    fn the_log_scrolls_under_the_wheel_and_follows_the_newest_line_otherwise() {
        let _en = english();
        let mut r = rig();
        to_question(&mut r, None);
        for i in 0..40 {
            r.events.send(Event::Log(format!("detail {i}"))).unwrap();
        }
        r.page.pump();
        r.page.key(key(KeyCode::Char('l')));
        let frame = draw(&mut r.page);
        assert!(frame.contains("detail 39") && !frame.contains("detail 0\n"), "the newest lines:\n{frame}");
        let pane = r.page.log_rect.expect("the log was drawn");
        let inside = Position { x: pane.x + 2, y: pane.y + 1 };
        r.page.wheel(true, inside);
        r.page.wheel(true, inside);
        let frame = draw(&mut r.page);
        assert!(!frame.contains("detail 39") && frame.contains("detail 37"), "scrolled up two:\n{frame}");
        r.page.wheel(false, inside);
        r.page.wheel(false, inside);
        r.page.wheel(false, inside);
        assert_eq!(r.page.log_scroll, 0, "back at the newest, and no further");
        assert!(draw(&mut r.page).contains("detail 39"));
    }

    #[test]
    fn the_write_says_how_long_it_has_left_once_it_knows_the_rate() {
        let _en = english();
        let mut r = rig();
        to_question(&mut r, Some(ours("v0.4.0")));
        r.page.key(key(KeyCode::Enter));
        let t0 = Instant::now();
        r.page.apply_at(Event::Phase(Phase::Writing), t0);
        r.page.apply_at(Event::Progress(10), t0);
        assert_eq!(r.page.time_left, None, "one sample is not a rate");
        r.page.apply_at(Event::Progress(20), t0 + Duration::from_secs(1));
        assert_eq!(r.page.time_left, None, "too early to say");
        // 10% → 40% in ten seconds: 3%/s, 60% to go — twenty seconds.
        r.page.apply_at(Event::Progress(40), t0 + Duration::from_secs(10));
        assert_eq!(r.page.time_left, Some(20));
        assert_eq!(r.page.busy().as_deref(), Some("writing… 40% · about 20 s left"));
        let frame = draw(&mut r.page);
        assert!(frame.contains("writing… 40% · about 20 s left"), "{frame}");
        r.page.apply_at(Event::Progress(99), t0 + Duration::from_secs(29));
        assert_eq!(r.page.busy().as_deref(), Some("writing… 99% · a few seconds left"));
        r.page.apply_at(Event::Progress(100), t0 + Duration::from_secs(30));
        assert_eq!(r.page.busy().as_deref(), Some("writing… 100%"), "done counting");
    }

    #[test]
    fn e_flips_the_erase_box_and_a_stranger_erases_by_default() {
        let _en = english();
        let mut r = rig();
        to_question(&mut r, None);
        assert!(matches!(r.page.step, Step::Confirm { erase: true, .. }), "a blank board: erase first");
        let frame = draw(&mut r.page);
        assert!(frame.contains("[x] "), "{frame}");
        assert!(frame.contains("Install ▸"), "{frame}");
        assert!(frame.contains("nothing readable"), "{frame}");
        r.page.key(key(KeyCode::Char('e')));
        assert!(matches!(r.page.step, Step::Confirm { erase: false, .. }));
        r.page.key(key(KeyCode::Enter));
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Go { erase: false }));
    }

    #[test]
    fn esc_at_the_question_waits_for_the_board_to_restart() {
        let _en = english();
        let mut r = rig();
        to_question(&mut r, Some(ours("v0.5.0")));
        assert!(matches!(r.page.step, Step::Confirm { ref plan, .. } if plan.kind == Kind::Same));
        assert!(draw(&mut r.page).contains("Write again ▸"));
        r.page.key(key(KeyCode::Esc));
        assert_eq!(r.page.step, Step::Cancelling);
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Quit));
        assert_eq!(r.page.busy().as_deref(), Some("restarting the board…"));
        assert!(draw(&mut r.page).contains("✓ Firmware   ✓ Board   Write   ▸ Restart"), "the write never happened");
        assert!(r.page.finished().is_none(), "not yet: the board is being restarted");
        r.events.send(Event::Cancelled).unwrap();
        r.page.pump();
        assert!(matches!(r.page.finished(), Some(Outcome::Quit)));
    }

    #[test]
    fn esc_while_the_board_is_being_reached_quits_once_the_worker_can_hear() {
        let _en = english();
        let mut r = rig();
        r.events.send(Event::Phase(Phase::Connecting)).unwrap();
        r.page.pump();
        assert_eq!(r.page.step, Step::Probing);
        r.page.key(key(KeyCode::Esc));
        assert_eq!(r.page.step, Step::Cancelling);
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Quit), "sent at once; the worker reads it at the question");
        r.events.send(Event::Probed { on_board: None, plan: flow::plan(None, "v0.5.0", None) }).unwrap();
        r.page.pump();
        assert_eq!(r.page.step, Step::Cancelling, "the question never shows");
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Quit), "and the answer is sent again for the recv that waits");
        r.events.send(Event::Cancelled).unwrap();
        r.page.pump();
        assert!(matches!(r.page.finished(), Some(Outcome::Quit)));
    }

    #[test]
    fn no_board_watches_the_ports_and_names_the_ones_it_saw() {
        let _en = english();
        let mut r = rig();
        r.events.send(firmware()).unwrap();
        r.events.send(Event::NoDevice { others: vec!["COM1".into()] }).unwrap();
        r.page.pump();
        let frame = draw(&mut r.page);
        assert!(frame.contains("No Core2 found — watching the USB ports"), "{frame}");
        assert!(frame.contains("• a USB-C cable") && frame.contains("• Linux:"), "the hints as bullets:\n{frame}");
        assert!(frame.contains("Serial ports seen: COM1 — not a Core2's bridge"), "{frame}");
        assert!(frame.contains("Look again"), "{frame}");
        assert!(frame.contains("✓ Firmware   ▸ Board"), "{frame}");
        assert_eq!(r.page.busy().as_deref(), Some("looking again every 2 s…"));
        // The watch: nothing yet, then a rescan once two seconds have gone by.
        r.page.tick();
        assert!(r.cmds.try_recv().is_err(), "too soon");
        r.page.last_scan = Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
        r.page.tick();
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Rescan));
        // The worker looks again and finds nothing: the page stays as it is, the log says it once.
        r.events.send(Event::Phase(Phase::Scanning)).unwrap();
        r.events.send(Event::NoDevice { others: vec![] }).unwrap();
        r.page.pump();
        assert_eq!(r.page.step, Step::NoDevice);
        assert!(draw(&mut r.page).contains("Serial ports seen: none"));
        assert_eq!(r.page.log.iter().filter(|l| l.text.starts_with("No Core2 found")).count(), 1);
        assert!(!r.page.log.iter().any(|l| l.text == "looking for a Core2…" && l.tone == Tone::Phase && r.page.log.len() > 3), "the watch's scans are not logged");
        // r still works, and then the board shows up: the page goes on by itself.
        r.page.key(key(KeyCode::Char('r')));
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Rescan));
        assert_eq!(r.page.step, Step::NoDevice, "the hints stay while it looks");
        r.events.send(Event::Phase(Phase::Scanning)).unwrap();
        r.events.send(Event::Phase(Phase::Connecting)).unwrap();
        r.page.pump();
        assert_eq!(r.page.step, Step::Probing);
    }

    #[test]
    fn several_boards_follow_the_ports_and_keep_the_pick() {
        let _en = english();
        let mut r = rig();
        r.events.send(firmware()).unwrap();
        let board = |port: &str, bridge: &'static str, vid: u16, pid: u16| Candidate {
            port: port.into(),
            bridge,
            usb: serialport::UsbPortInfo { vid, pid, serial_number: None, manufacturer: None, product: None },
        };
        let two = vec![board("COM3", "CH9102", 0x1A86, 0x55D4), board("COM7", "CP210x", 0x10C4, 0xEA60)];
        r.events.send(Event::Several(two.clone())).unwrap();
        r.page.pump();
        let frame = draw(&mut r.page);
        assert!(frame.contains("which one") && frame.contains("▸ COM3"), "{frame}");
        assert!(frame.contains("COM7 · CP210x"), "{frame}");
        assert!(frame.contains("the list follows the ports"), "{frame}");
        r.page.key(key(KeyCode::Down));
        r.page.key(key(KeyCode::Down));
        assert!(matches!(r.page.step, Step::Several { cursor: 1, .. }), "the cursor stops at the last row");
        // The watch's answer, unchanged: the cursor stays.
        r.events.send(Event::Several(two.clone())).unwrap();
        r.page.pump();
        assert!(matches!(r.page.step, Step::Several { cursor: 1, .. }));
        // One unplugged: the list shrinks and the cursor comes back in range.
        r.events.send(Event::Several(vec![two[0].clone()])).unwrap();
        r.page.pump();
        assert!(matches!(&r.page.step, Step::Several { list, cursor: 0 } if list.len() == 1));
        r.page.key(key(KeyCode::Enter));
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Pick("COM3".into())));
        assert_eq!(r.page.step, Step::Probing);
        // A late answer to the watch cannot knock the page back once the board is reached.
        r.events.send(Event::Board(board_info())).unwrap();
        r.events.send(Event::Several(two)).unwrap();
        r.page.pump();
        assert_eq!(r.page.step, Step::Probing);
    }

    fn board_info() -> DeviceInfo {
        board()
    }

    #[test]
    fn a_failure_shows_its_hint_opens_the_log_and_r_starts_a_fresh_worker() {
        let _en = english();
        let mut r = rig();
        r.events.send(firmware()).unwrap();
        r.events.send(Event::Phase(Phase::Scanning)).unwrap();
        r.events.send(Event::Phase(Phase::Connecting)).unwrap();
        let busy = DeviceError::Busy { port: "COM3".into(), detail: "Access is denied".into() };
        r.events.send(Event::Failed(busy)).unwrap();
        r.page.pump();
        assert!(r.page.logs_open, "a failure opens the log by itself");
        let frame = draw(&mut r.page);
        assert!(frame.contains("That did not work"), "{frame}");
        assert!(frame.contains("COM3 is in use"), "{frame}");
        assert!(frame.contains("serial monitor"), "the hint:\n{frame}");
        assert!(frame.contains("✓ Firmware   ✗ Board   Write   Restart"), "the step that failed:\n{frame}");
        assert!(frame.contains("▾ Hide logs"), "{frame}");
        assert!(frame.contains("Try again") && frame.contains("Close"), "{frame}");
        r.page.key(key(KeyCode::Char('r')));
        assert_eq!(r.respawns.load(Ordering::SeqCst), 1);
        assert_eq!(r.page.step, Step::Preparing);
        assert!(r.page.log.iter().any(|l| l.text == "r — starting over"));
        r.page.pump();
        assert!(matches!(r.page.step, Step::Failed { .. }), "the stub worker is gone: said so");
        r.page.key(key(KeyCode::Esc));
        assert!(matches!(r.page.finished(), Some(Outcome::Quit)));
    }

    #[test]
    fn a_failure_mid_write_marks_the_write() {
        let _en = english();
        let mut r = rig();
        to_question(&mut r, Some(ours("v0.4.0")));
        r.page.key(key(KeyCode::Enter));
        r.events.send(Event::Phase(Phase::Writing)).unwrap();
        r.events.send(Event::Progress(50)).unwrap();
        r.events.send(Event::Failed(DeviceError::Link("died".into()))).unwrap();
        r.page.pump();
        assert!(draw(&mut r.page).contains("✓ Firmware   ✓ Board   ✗ Write   Restart"));
    }

    #[test]
    fn the_minimum_window_holds_every_step_and_a_smaller_one_is_asked_to_grow() {
        let _en = english();
        let mut r = rig();
        to_question(&mut r, Some(ours("v0.4.0")));
        let frame = draw_at(&mut r.page, 72, 24);
        assert!(frame.contains("Update ▸") && frame.contains("▸ Show logs"), "{frame}");
        assert!(frame.ends_with("  Enter write · e erase first · l logs · Esc cancel                     \n"), "the tips on the last row:\n{frame}");
        // Done is the tallest step: the next-step block, then Close, and
        // the tips still on the bottom row under them.
        r.page.key(key(KeyCode::Enter));
        let boot = Some("mstream-mp3-player v0.5.0 (commit a, 2026-10-01), ELF 0".to_string());
        r.events.send(Event::Done { version: "v0.5.0".into(), skipped: true, boot }).unwrap();
        r.page.pump();
        let frame = draw_at(&mut r.page, 72, 24);
        assert!(frame.contains("│  Close  │"), "{frame}");
        assert!(frame.ends_with("  Enter close · l logs                                                  \n"), "{frame}");
        assert!(!frame.contains("Show logs"), "no row left for the toggle; l still works:\n{frame}");
        let text = draw_at(&mut r.page, 40, 10);
        assert!(text.contains("larger"), "{text}");
    }

    #[test]
    fn a_log_line_is_cut_to_the_column_never_wrapped() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("twelve chars", 8), "twelve …");
    }
}
