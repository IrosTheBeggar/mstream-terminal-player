//! The server's log, read as it grows: the Admin tab's log tail (docs/
//! ux-contracts/admin-screen.md, clauses 21-26; docs/ui-kit.md, "Log
//! tail"). The model holds the newest thousand lines of mStream's main log
//! ring, delta-polled by sequence number from `GET api/v1/admin/logs/
//! recent`, and knows whether the view follows the newest line or stands
//! paused where the reader scrolled it. The route has no level or limit
//! parameter, so the level filter lives here, in the player.
//!
//! The poll runs on a thread of the log's own rather than on the App's api
//! worker or a room's: a slow answer from a busy server must never hold a
//! room's request behind it, nor the music's (PLAN.md risk 63). One request
//! is in flight at most; the next is due two seconds after the last answer,
//! ten after a failure, and never again after a 401, 403 or 405 until the
//! tab is opened again. The screen pumps it once a frame and the pump only
//! ever `try_recv`s, so a frame never waits on the network.
//!
//! Drawing takes any surface and a wrapper for the log's own actions, so
//! the host decides what a click on the paused word or the level control
//! becomes in its action type. The level menu is drawn in a second pass,
//! [`draw_menu`], after everything else, because it hangs over the lines
//! as a real overlay.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use super::{bright_bold, bullet_glyph, put, sel};
use crate::admin::tz::Zone;
use crate::admin::{gate_message, iso_unix, printable};
use crate::api::types::LogTail;
use crate::api::{ApiError, Client};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{Surface, dim, frame_at, width};

/// How many lines the player keeps; older ones fall off the front.
pub(crate) const RING: usize = 1000;
/// The pause between an answer and the next request.
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(2);
/// The pause after a failed request, so a server that is down is not
/// asked thirty times a minute.
pub(crate) const POLL_FAILING: Duration = Duration::from_secs(10);
/// How many characters of a message's first line are kept. The widest
/// log the layouts draw is far narrower; the cap only bounds the memory
/// a thousand 4000-character messages would take.
const KEEP_CHARS: usize = 400;

/// A line's level, most severe first, so a line shows when its level is
/// at or above the filter's (`line.level <= filter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    /// The menu's rows, in its order.
    pub(crate) const ALL: [Level; 4] = [Level::Error, Level::Warn, Level::Info, Level::Debug];

    /// Winston's seven levels folded into the four the player offers:
    /// `http`, `verbose` and `silly` are chatter, so they read as debug.
    /// A level the player has never heard of reads as info, where the
    /// default filter still shows it.
    pub(crate) fn of(raw: &str) -> Level {
        match raw.trim().to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" | "warning" => Level::Warn,
            "http" | "verbose" | "debug" | "silly" => Level::Debug,
            _ => Level::Info,
        }
    }

    pub(crate) fn label(self) -> String {
        match self {
            Level::Error => t!("gui.admin.log.level_error"),
            Level::Warn => t!("gui.admin.log.level_warn"),
            Level::Info => t!("gui.admin.log.level_info"),
            Level::Debug => t!("gui.admin.log.level_debug"),
        }
        .to_string()
    }

    fn index(self) -> usize {
        Level::ALL.iter().position(|l| *l == self).unwrap_or(2)
    }
}

/// One log line as the player keeps it: the message's first line made
/// printable, and whether anything followed it (a stack trace).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub seq: u64,
    /// Unix seconds, when the server's time parsed.
    pub at: Option<i64>,
    pub level: Level,
    pub text: String,
    pub more: bool,
}

impl Line {
    fn from_entry(entry: &crate::api::types::ActivityEntry) -> Line {
        // The first line with something on it: a message that opens on a
        // newline still says what it is about.
        let mut parts = entry.message.split('\n').filter(|l| !printable(l, 1).is_empty());
        let text = parts.next().map(|l| printable(l, KEEP_CHARS)).unwrap_or_default();
        let more = parts.next().is_some();
        Line { seq: entry.seq, at: iso_unix(&entry.t), level: Level::of(&entry.level), text, more }
    }
}

/// The lines held, where the next poll starts, and how the reader is
/// looking at them.
pub(crate) struct LogModel {
    lines: VecDeque<Line>,
    /// The last `lastSeq` the server answered: the next poll's `since`.
    cursor: u64,
    /// Lines at or below this sequence number have been on screen (or
    /// were already held when the log first answered).
    seen: u64,
    answered: bool,
    /// How many shown lines the view stands above the newest; 0 follows.
    pub scroll: usize,
    pub level: Level,
    /// The ring's size on the server, once it has answered; 0 is off.
    pub capacity: Option<u64>,
    /// The last poll's failure, in words, until a poll succeeds.
    pub error: Option<String>,
    /// The server refused the log (401, 403 or 405): polling stops.
    pub gated: bool,
}

impl Default for LogModel {
    fn default() -> Self {
        Self::new()
    }
}

impl LogModel {
    pub(crate) fn new() -> Self {
        LogModel {
            lines: VecDeque::new(),
            cursor: 0,
            seen: 0,
            answered: false,
            scroll: 0,
            level: Level::Info,
            capacity: None,
            error: None,
            gated: false,
        }
    }

    /// Fold an answer in. A `lastSeq` below the cursor is a server that
    /// restarted and answered its whole new ring: the old lines go and
    /// every new one counts as unseen. Otherwise only entries past the
    /// newest line held are appended, so an answer that repeats itself
    /// adds nothing. A paused view grows its offset by the lines that
    /// arrived under it, so what the reader is looking at stays put.
    pub(crate) fn take(&mut self, tail: LogTail) {
        if self.answered && tail.last_seq < self.cursor {
            self.lines.clear();
            self.seen = 0;
            self.scroll = 0;
        }
        let newest = self.lines.back().map(|l| l.seq);
        let mut arrived = 0;
        for entry in tail.entries.iter().filter(|e| newest.is_none_or(|n| e.seq > n)) {
            let line = Line::from_entry(entry);
            if line.level <= self.level {
                arrived += 1;
            }
            self.lines.push_back(line);
        }
        while self.lines.len() > RING {
            self.lines.pop_front();
        }
        if self.scroll > 0 {
            self.scroll += arrived;
        }
        if !self.answered {
            self.seen = tail.last_seq;
            self.answered = true;
        }
        self.cursor = tail.last_seq;
        self.capacity = Some(tail.capacity);
        self.error = None;
    }

    /// A poll failed: the hub's sentence for a refusal, else the log's
    /// own with the reason. A refusal stops the polling for good.
    pub(crate) fn fail(&mut self, e: &ApiError) {
        self.error = Some(gate_message(e, &t!("gui.admin.log.failed")));
        self.gated |= matches!(
            e,
            ApiError::Unauthorized | ApiError::Forbidden(_) | ApiError::Server { status: 405, .. }
        );
    }

    /// The lines that pass the level, oldest first.
    pub(crate) fn shown(&self) -> Vec<&Line> {
        self.lines.iter().filter(|l| l.level <= self.level).collect()
    }

    /// Whether the view rides the newest line. Derived from the offset,
    /// so the header's word and the view can never disagree.
    pub(crate) fn following(&self) -> bool {
        self.scroll == 0
    }

    /// Move the view `older` lines back (negative: toward the newest).
    /// Reaching the newest line follows again.
    pub(crate) fn scroll_by(&mut self, older: i32) {
        let most = self.shown().len().saturating_sub(1) as i64;
        self.scroll = (self.scroll as i64 + older as i64).clamp(0, most.max(0)) as usize;
    }

    /// Ride the newest line again.
    pub(crate) fn follow(&mut self) {
        self.scroll = 0;
    }

    /// A new filter, and the view follows: the old offset counted lines
    /// of a different list.
    pub(crate) fn set_level(&mut self, level: Level) {
        self.level = level;
        self.follow();
    }

    /// Shown lines that arrived since the log was last on screen.
    pub(crate) fn unseen(&self) -> usize {
        self.lines.iter().filter(|l| l.level <= self.level && l.seq > self.seen).count()
    }

    /// The log is on screen: what it holds has been seen.
    pub(crate) fn mark_seen(&mut self) {
        self.seen = self.lines.back().map_or(self.cursor, |l| l.seq.max(self.cursor));
    }

    /// Keep the offset where `rows` lines still fill the view.
    fn clamp(&mut self, rows: usize) {
        self.scroll = self.scroll.min(self.shown().len().saturating_sub(rows));
    }
}

/// Whether the next request may go: never while one is out, never after
/// a refusal, and otherwise once the pause since the last answer (longer
/// after a failure) has passed. The first request goes at once.
pub(crate) fn due(last: Option<Instant>, now: Instant, failing: bool, in_flight: bool, gated: bool) -> bool {
    if gated || in_flight {
        return false;
    }
    let pause = if failing { POLL_FAILING } else { POLL_EVERY };
    last.is_none_or(|t| now.saturating_duration_since(t) >= pause)
}

/// `HH:MM:SS` on the machine's clock, or UTC on a machine without a zone;
/// dashes when the server's time did not parse.
pub(crate) fn clock(at: Option<i64>, zone: Option<&Zone>) -> String {
    let Some(t) = at else { return "--:--:--".to_string() };
    let local = t + zone.map_or(0, |z| z.offset_at(t) as i64);
    let s = local.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// What a click in the log means. The host wraps these in its own action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LogAct {
    /// The level control: open the menu, or close it when open.
    Menu,
    /// A click anywhere off the menu's rows.
    MenuClose,
    Pick(Level),
    /// The paused word.
    Follow,
}

/// What became of a key the focused log was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogKey {
    Taken,
    /// Esc or `q`: the focus goes back to the hallway.
    Leave,
}

/// How the host wants the log drawn: a blank row under the header (the
/// column and the Log room), and the dim hint at the header's right.
pub(crate) struct Look {
    pub spaced: bool,
    pub hint: Option<String>,
}

/// The poll thread's two ends, and when it last answered.
struct Worker {
    jobs: Sender<u64>,
    done: Receiver<Result<LogTail, ApiError>>,
    in_flight: bool,
    last: Option<Instant>,
}

/// The log as a screen holds it: the model, the level menu's cursor
/// while it is open, the poll thread, and where the last frame put it.
pub(crate) struct LogUi {
    pub model: LogModel,
    pub menu: Option<usize>,
    zone: Option<Zone>,
    worker: Option<Worker>,
    /// The level control's cells, which the menu hangs from.
    level_at: Option<Rect>,
    /// The area the log was last drawn in, which the menu stays inside.
    drawn_in: Option<Rect>,
    /// How many line rows the last frame had, for the wheel's clamp.
    rows: usize,
}

impl LogUi {
    pub(crate) fn new(zone: Option<Zone>) -> Self {
        LogUi { model: LogModel::new(), menu: None, zone, worker: None, level_at: None, drawn_in: None, rows: 0 }
    }

    /// Start reading `client`'s log from the beginning of its ring, on a
    /// thread of its own: it takes a `since`, makes the one blocking call,
    /// and sends the answer back, until the screen lets go of it.
    pub(crate) fn start(&mut self, client: Client) {
        self.stop();
        self.model = LogModel::new();
        self.menu = None;
        let (jobs, job_rx) = channel::<u64>();
        let (done_tx, done) = channel::<Result<LogTail, ApiError>>();
        let spawned = std::thread::Builder::new().name("server log".into()).spawn(move || {
            while let Ok(since) = job_rx.recv() {
                if done_tx.send(client.admin_logs_recent(since)).is_err() {
                    break;
                }
            }
        });
        if spawned.is_ok() {
            self.worker = Some(Worker { jobs, done, in_flight: false, last: None });
        }
    }

    /// Let go of the poll thread. It finishes the call it is in, finds
    /// nobody to answer, and ends.
    pub(crate) fn stop(&mut self) {
        self.worker = None;
        self.menu = None;
    }

    pub(crate) fn running(&self) -> bool {
        self.worker.is_some()
    }

    /// Take whatever answers have arrived, then send the next request
    /// when one is due. Never blocks.
    pub(crate) fn pump(&mut self, now: Instant) {
        let Some(worker) = self.worker.as_mut() else { return };
        let mut lost = false;
        loop {
            match worker.done.try_recv() {
                Ok(answer) => {
                    worker.in_flight = false;
                    worker.last = Some(now);
                    match answer {
                        Ok(tail) => self.model.take(tail),
                        Err(e) => self.model.fail(&e),
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    lost = true;
                    break;
                }
            }
        }
        if !lost && due(worker.last, now, self.model.error.is_some(), worker.in_flight, self.model.gated) {
            if worker.jobs.send(self.model.cursor).is_ok() {
                worker.in_flight = true;
            } else {
                lost = true;
            }
        }
        if lost {
            self.worker = None;
        }
    }

    /// A key while the log has the focus; `rows` is how many lines it
    /// shows. Every key is the log's except the two that leave it.
    pub(crate) fn key(&mut self, key: KeyEvent, rows: usize) -> LogKey {
        let page = rows.max(1) as i32;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return LogKey::Leave,
            KeyCode::Up => self.model.scroll_by(1),
            KeyCode::Down => self.model.scroll_by(-1),
            KeyCode::PageUp => self.model.scroll_by(page),
            KeyCode::PageDown => self.model.scroll_by(-page),
            KeyCode::Home => self.model.scroll = usize::MAX,
            KeyCode::End | KeyCode::Char('f') => self.model.follow(),
            KeyCode::Enter => self.menu = Some(self.model.level.index()),
            _ => {}
        }
        self.model.clamp(rows);
        LogKey::Taken
    }

    /// A key while the level menu is open: it takes every one.
    pub(crate) fn menu_key(&mut self, key: KeyEvent) {
        let Some(cursor) = self.menu else { return };
        match key.code {
            KeyCode::Up => self.menu = Some(cursor.saturating_sub(1)),
            KeyCode::Down => self.menu = Some((cursor + 1).min(Level::ALL.len() - 1)),
            KeyCode::Enter => self.act(LogAct::Pick(Level::ALL[cursor])),
            KeyCode::Esc => self.menu = None,
            _ => {}
        }
    }

    /// One notch of the wheel over the log: up reads older lines, which
    /// pauses it; down comes back, and following resumes at the bottom.
    pub(crate) fn wheel(&mut self, up: bool) {
        self.model.scroll_by(if up { 1 } else { -1 });
        if self.rows > 0 {
            self.model.clamp(self.rows);
        }
    }

    pub(crate) fn act(&mut self, act: LogAct) {
        match act {
            LogAct::Menu => {
                self.menu = if self.menu.is_some() { None } else { Some(self.model.level.index()) };
            }
            LogAct::MenuClose => self.menu = None,
            LogAct::Pick(level) => {
                self.model.set_level(level);
                self.menu = None;
            }
            LogAct::Follow => self.model.follow(),
        }
    }

    /// The same log on UTC, so a test's clock reads the same anywhere.
    #[cfg(test)]
    pub(crate) fn utc(mut self) -> Self {
        self.zone = None;
        self
    }
}

/// `text` at (x, y), clipped by cells so it ends before `right`. Returns
/// the cells it took.
fn span(frame: &mut Frame, x: u16, y: u16, right: u16, text: &str, style: Style) -> u16 {
    let room = right.saturating_sub(x) as usize;
    if room == 0 {
        return 0;
    }
    let shown = super::bar::clip(text, room);
    put(frame, x, y, &shown, style);
    width(&shown) as u16
}

/// The log in `area`: the header on its first row, then the lines, the
/// newest at the bottom — or the one sentence when there are none to
/// show. A failure while lines are held takes the last row, so a log
/// that stopped never passes for one that is following.
pub(crate) fn draw<A: Clone>(
    frame: &mut Frame,
    ui: &mut Surface<A>,
    log: &mut LogUi,
    area: Rect,
    look: &Look,
    wrap: fn(LogAct) -> A,
) {
    log.drawn_in = Some(area);
    log.level_at = None;
    log.rows = 0;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let right = area.right();
    let y = area.y;

    // The rows the lines get, and the offset clamped to them before the
    // header says whether the view follows: a paused view whose lines all
    // fit is following, and says so on this frame, not the next.
    let top = y + if look.spaced { 2 } else { 1 };
    let below = area.bottom().saturating_sub(top) as usize;
    let held = log.model.shown().len();
    let error_row = log.model.error.is_some() && held > 0 && below > 0;
    let rows = below - usize::from(error_row);
    log.rows = rows;
    log.model.scroll = log.model.scroll.min(held.saturating_sub(rows));

    // The header: the follow state, the level control, the hint.
    let mut x = area.x;
    let bullet = bullet_glyph();
    if log.model.following() {
        let ok = Style::default().fg(th().ok);
        x += span(frame, x, y, right, &format!("{bullet} "), ok);
        x += span(frame, x, y, right, &t!("gui.admin.log.following"), ok.add_modifier(Modifier::BOLD));
    } else {
        let text = format!("{bullet} {}", t!("gui.admin.log.paused"));
        let rect = Rect { x, y, width: (width(&text) as u16).min(right.saturating_sub(x)), height: 1 };
        let style = if ui.hovers(rect) { bright_bold() } else { dim() };
        x += span(frame, x, y, right, &text, style);
        ui.click(rect, wrap(LogAct::Follow));
    }
    x += span(frame, x, y, right, " · ", dim());
    let chevron = if legacy_conhost() { " v" } else { " ▾" };
    let control = format!("{}{chevron}", log.model.level.label());
    let rect = Rect { x, y, width: (width(&control) as u16).min(right.saturating_sub(x)), height: 1 };
    if rect.width > 0 {
        let style = if ui.hovers(rect) { bright_bold() } else { dim() };
        x += span(frame, x, y, right, &control, style);
        ui.click(rect, wrap(LogAct::Menu));
        log.level_at = Some(rect);
    }
    if let Some(hint) = &look.hint {
        let w = width(hint) as u16;
        if x + 2 + w <= right {
            put(frame, right - w, y, hint, dim());
        }
    }
    if below == 0 {
        return;
    }

    // With nothing to show, one sentence; a failure while lines are held
    // takes the last row under them.
    if let Some(error) = &log.model.error {
        let at = if error_row { top + rows as u16 } else { top };
        span(frame, area.x, at, right, error, Style::default().fg(th().gold));
    } else if held == 0 {
        let words = if !log.model.answered {
            t!("gui.admin.log.waiting")
        } else if log.model.capacity == Some(0) {
            t!("gui.admin.log.off")
        } else {
            t!("gui.admin.log.empty")
        };
        span(frame, area.x, top, right, &words, dim());
    }

    // The lines, the newest at the bottom of the window onto them.
    let shown = log.model.shown();
    let end = held - log.model.scroll;
    let first = end.saturating_sub(rows);
    let more = if legacy_conhost() { " »" } else { " …" };
    for (row, line) in shown[first..end].iter().enumerate() {
        let ly = top + row as u16;
        let mut lx = area.x;
        lx += span(frame, lx, ly, right, &clock(line.at, log.zone.as_ref()), dim());
        lx += span(frame, lx, ly, right, "  ", Style::default());
        let color = match line.level {
            Level::Error => th().danger,
            Level::Warn => th().gold,
            Level::Info | Level::Debug => th().text,
        };
        let text = if line.more { format!("{}{more}", line.text) } else { line.text.clone() };
        span(frame, lx, ly, right, &text, Style::default().fg(color));
    }
}

/// The level menu, when it is open: the kit's Dropdown hanging from the
/// level control's first cell and kept inside the log's area, the four
/// levels with the current one wearing `•` and the cursor on the slab.
/// A click on a row picks it; a click anywhere else closes the menu, which
/// is why the whole frame registers the close under the rows. Drawn after
/// everything else, as an overlay.
pub(crate) fn draw_menu<A: Clone>(frame: &mut Frame, ui: &mut Surface<A>, log: &mut LogUi, wrap: fn(LogAct) -> A) {
    let Some(cursor) = log.menu else { return };
    let whole = frame.area();
    let area = log.drawn_in.unwrap_or(whole).intersection(whole);
    ui.click(whole, wrap(LogAct::MenuClose));
    let anchor = log.level_at.unwrap_or(Rect { x: area.x, y: area.y, width: 0, height: 1 });

    let labels: Vec<String> = Level::ALL.iter().map(|l| l.label()).collect();
    let widest = labels.iter().map(|l| width(l)).max().unwrap_or(5) as u16;
    // The frame, the marker's cell with a space each side, and a cell of
    // padding after the name.
    let w = (widest + 6).max(12).min(area.width);
    let y = anchor.bottom().min(area.bottom());
    let h = (Level::ALL.len() as u16 + 2).min(area.bottom().saturating_sub(y));
    if w < 3 || h < 3 {
        return;
    }
    let rect = Rect { x: anchor.x.min(area.right().saturating_sub(w)).max(area.x), y, width: w, height: h };
    ui.overlay(rect);
    let inner = frame_at(frame, rect, th().accent);
    let bullet = bullet_glyph();
    for (i, level) in Level::ALL.iter().enumerate().take(inner.height as usize) {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let on = i == cursor;
        let hover = !on && ui.hovers(row);
        if on {
            put(frame, row.x, row.y, &" ".repeat(row.width as usize), sel());
        }
        let mark = if *level == log.model.level { bullet } else { " " };
        let right = row.right();
        let mut x = row.x;
        let marker = if on { sel() } else { Style::default().fg(th().accent) };
        x += span(frame, x, row.y, right, &format!(" {mark} "), marker);
        let style = if on {
            sel().add_modifier(Modifier::BOLD)
        } else if hover {
            bright_bold()
        } else {
            Style::default()
        };
        span(frame, x, row.y, right, &labels[i], style);
        ui.click(row, wrap(LogAct::Pick(*level)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::ActivityEntry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::layout::Position;

    fn entry(seq: u64, level: &str, message: &str) -> ActivityEntry {
        ActivityEntry {
            seq,
            t: format!("2026-10-02T09:{:02}:{:02}.000Z", seq / 60 % 60, seq % 60),
            level: level.to_string(),
            message: message.to_string(),
        }
    }

    fn tail(entries: Vec<ActivityEntry>, last_seq: u64) -> LogTail {
        LogTail { entries, last_seq, capacity: 1000 }
    }

    /// A tail of `n` info lines numbered `from..from+n`.
    fn infos(from: u64, n: u64) -> LogTail {
        tail((from..from + n).map(|s| entry(s, "info", &format!("line {s}"))).collect(), from + n - 1)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn texts(model: &LogModel) -> Vec<String> {
        model.shown().iter().map(|l| l.text.clone()).collect()
    }

    fn id(act: LogAct) -> LogAct {
        act
    }

    /// Draw the log in `area` of a `w`×`h` screen, the menu pass too, and
    /// hand back the buffer and the surface the frame registered.
    fn render(log: &mut LogUi, ui: &mut Surface<LogAct>, size: (u16, u16), area: Rect, look: &Look) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        ui.begin_frame();
        terminal
            .draw(|frame| {
                draw(frame, ui, log, area, look, id);
                draw_menu(frame, ui, log, id);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn plain() -> Look {
        Look { spaced: false, hint: None }
    }

    /// The x of the first cell of `needle` on row `y`, counted in cells.
    fn find(buf: &Buffer, y: u16, needle: &str) -> Option<u16> {
        let first = needle.chars().next()?.to_string();
        (0..buf.area.width).find(|&x| {
            buf[(x, y)].symbol() == first && {
                let mut cx = x;
                needle.chars().all(|c| {
                    let ok = cx < buf.area.width && buf[(cx, y)].symbol() == c.to_string();
                    cx += crate::kit::char_width(c).max(1) as u16;
                    ok
                })
            }
        })
    }

    #[test]
    fn a_tail_appends_past_the_cursor_and_a_restart_starts_over() {
        let mut m = LogModel::new();
        m.take(infos(1, 3));
        assert_eq!(m.cursor, 3);
        assert_eq!(m.capacity, Some(1000));
        m.take(infos(4, 2));
        assert_eq!(texts(&m), ["line 1", "line 2", "line 3", "line 4", "line 5"]);
        assert_eq!(m.cursor, 5);

        // The server restarted: its new ring answers with a lastSeq below
        // the cursor, and the log starts over from it, every line new.
        m.take(infos(1, 2));
        assert_eq!(texts(&m), ["line 1", "line 2"]);
        assert_eq!(m.cursor, 2);
        assert_eq!(m.unseen(), 2, "a restarted server's lines all count");
    }

    #[test]
    fn answers_at_or_below_the_newest_held_are_dropped() {
        let mut m = LogModel::new();
        m.take(infos(1, 3));
        // The same answer again, and one overlapping it: only 4 is new.
        m.take(infos(1, 3));
        m.take(tail(vec![entry(3, "info", "line 3"), entry(4, "info", "line 4")], 4));
        assert_eq!(texts(&m), ["line 1", "line 2", "line 3", "line 4"]);
    }

    #[test]
    fn the_ring_keeps_the_newest_thousand() {
        let mut m = LogModel::new();
        m.take(infos(1, 700));
        m.take(infos(701, 700));
        let shown = m.shown();
        assert_eq!(shown.len(), RING);
        assert_eq!(shown.first().unwrap().seq, 401);
        assert_eq!(shown.last().unwrap().seq, 1400);
    }

    #[test]
    fn levels_rank_winstons_seven_into_four_and_filter_in_the_player() {
        assert_eq!(Level::of("error"), Level::Error);
        assert_eq!(Level::of("warn"), Level::Warn);
        assert_eq!(Level::of("WARNING"), Level::Warn);
        assert_eq!(Level::of("info"), Level::Info);
        for chatter in ["http", "verbose", "debug", "silly"] {
            assert_eq!(Level::of(chatter), Level::Debug, "{chatter}");
        }
        assert_eq!(Level::of("notice"), Level::Info, "an unknown level reads as info");
        assert!(Level::Error < Level::Warn && Level::Warn < Level::Info && Level::Info < Level::Debug);

        let mut m = LogModel::new();
        assert_eq!(m.level, Level::Info, "info is the default");
        m.take(tail(
            vec![
                entry(1, "error", "e"),
                entry(2, "warn", "w"),
                entry(3, "info", "i"),
                entry(4, "http", "h"),
                entry(5, "silly", "s"),
            ],
            5,
        ));
        assert_eq!(texts(&m), ["e", "w", "i"]);
        m.set_level(Level::Debug);
        assert_eq!(texts(&m), ["e", "w", "i", "h", "s"]);
        m.set_level(Level::Error);
        assert_eq!(texts(&m), ["e"]);
        assert_eq!(Level::Warn.label(), t!("gui.admin.log.level_warn"));
    }

    #[test]
    fn scrolling_up_pauses_and_the_view_holds_as_lines_arrive() {
        let mut log = LogUi::new(None);
        log.model.take(infos(1, 20));
        assert!(log.model.following());
        assert_eq!(log.key(key(KeyCode::Up), 5), LogKey::Taken);
        assert_eq!(log.key(key(KeyCode::Up), 5), LogKey::Taken);
        assert!(!log.model.following());
        assert_eq!(log.model.scroll, 2);
        let newest_in_view = |m: &LogModel| m.shown()[m.shown().len() - 1 - m.scroll].seq;
        assert_eq!(newest_in_view(&log.model), 18);

        // Three lines arrive (one below the filter): the view holds on 18.
        log.model.take(tail(
            vec![entry(21, "info", "a"), entry(22, "debug", "b"), entry(23, "warn", "c")],
            23,
        ));
        assert_eq!(newest_in_view(&log.model), 18, "the paused view holds still");
        assert!(!log.model.following());

        // The wheel pauses too: up is older.
        let mut other = LogUi::new(None);
        other.model.take(infos(1, 20));
        other.wheel(true);
        assert!(!other.model.following());
        other.wheel(false);
        assert!(other.model.following());

        // A page and Home stop at the oldest line that still fills the view.
        log.key(key(KeyCode::Home), 5);
        assert_eq!(log.model.scroll, log.model.shown().len() - 5);
        log.key(key(KeyCode::PageUp), 5);
        assert_eq!(log.model.scroll, log.model.shown().len() - 5);
    }

    #[test]
    fn end_f_and_reaching_the_newest_line_follow_again_and_a_new_level_follows() {
        let mut log = LogUi::new(None);
        log.model.take(infos(1, 20));
        log.key(key(KeyCode::PageUp), 5);
        assert!(!log.model.following());
        log.key(key(KeyCode::End), 5);
        assert!(log.model.following(), "End follows");

        log.key(key(KeyCode::Up), 5);
        log.key(key(KeyCode::Char('f')), 5);
        assert!(log.model.following(), "f follows");

        log.key(key(KeyCode::Up), 5);
        log.key(key(KeyCode::Up), 5);
        log.key(key(KeyCode::Down), 5);
        assert!(!log.model.following());
        log.key(key(KeyCode::Down), 5);
        assert!(log.model.following(), "back at the newest line, it follows");
        log.key(key(KeyCode::Down), 5);
        assert!(log.model.following(), "and stays there");

        log.key(key(KeyCode::Up), 5);
        log.act(LogAct::Follow);
        assert!(log.model.following(), "the paused word follows");

        log.key(key(KeyCode::Up), 5);
        log.act(LogAct::Pick(Level::Warn));
        assert!(log.model.following(), "a new level follows");
        assert_eq!(log.model.level, Level::Warn);

        assert_eq!(log.key(key(KeyCode::Esc), 5), LogKey::Leave);
        assert_eq!(log.key(key(KeyCode::Char('q')), 5), LogKey::Leave);
        assert_eq!(log.key(key(KeyCode::Char('7')), 5), LogKey::Taken, "every other key is the log's");
    }

    #[test]
    fn the_new_count_skips_what_was_there_at_the_first_answer_and_counts_only_shown_lines() {
        let mut m = LogModel::new();
        assert_eq!(m.unseen(), 0, "nothing before the first answer");
        m.take(infos(1, 10));
        assert_eq!(m.unseen(), 0, "what the ring held when the log opened is not new");
        m.take(tail(
            vec![entry(11, "info", "a"), entry(12, "debug", "b"), entry(13, "error", "c")],
            13,
        ));
        assert_eq!(m.unseen(), 2, "the debug line is below the filter");
        m.mark_seen();
        assert_eq!(m.unseen(), 0);
        m.take(tail(vec![entry(14, "warn", "d")], 14));
        assert_eq!(m.unseen(), 1);
    }

    #[test]
    fn a_multi_line_message_shows_its_first_line_and_an_ellipsis_and_no_control_characters() {
        let mut m = LogModel::new();
        m.take(tail(
            vec![
                entry(1, "error", "Error: ENOENT\r\n    at open (fs.js:1:1)\n    at main"),
                entry(2, "info", "tab\there \u{1b}[31mred\u{1b}[0m"),
                entry(3, "info", "\nopens on a newline"),
                entry(4, "info", "ends on one\n"),
            ],
            4,
        ));
        let lines = m.shown();
        assert_eq!(lines[0].text, "Error: ENOENT");
        assert!(lines[0].more);
        assert!(lines[1].text.chars().all(|c| !c.is_control()), "{:?}", lines[1].text);
        assert!(!lines[1].more);
        assert_eq!(lines[2].text, "opens on a newline");
        assert!(!lines[2].more);
        assert_eq!(lines[3].text, "ends on one");
        assert!(!lines[3].more, "nothing followed the newline");

        let mut log = LogUi::new(None).utc();
        log.model = m;
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (60, 6), Rect::new(0, 0, 60, 6), &plain());
        let ellipsis = if legacy_conhost() { " »" } else { " …" };
        assert!(row(&buf, 1).contains(&format!("Error: ENOENT{ellipsis}")), "{}", row(&buf, 1));
    }

    #[test]
    fn the_clock_is_local_hh_mm_ss_utc_without_a_zone_or_dashes() {
        // 2026-10-02T09:15:04Z.
        let t = iso_unix("2026-10-02T09:15:04.120Z");
        assert_eq!(clock(t, None), "09:15:04");
        assert_eq!(clock(None, None), "--:--:--");
        assert_eq!(clock(iso_unix("yesterday"), None), "--:--:--");
        assert_eq!(clock(Some(-1), None), "23:59:59", "before the epoch wraps the right way");
        // The machine's own zone, where it has one: the clock is UTC moved
        // by the zone's offset at that instant.
        if let Some(zone) = crate::admin::tz::local() {
            let t = t.unwrap();
            let moved = t + zone.offset_at(t) as i64;
            assert_eq!(clock(Some(t), Some(&zone)), clock(Some(moved), None));
        }
        let line = Line::from_entry(&entry(5, "info", "x"));
        assert_eq!(clock(line.at, None), "09:00:05");
    }

    #[test]
    fn polls_are_due_every_two_seconds_ten_after_a_failure_never_two_at_once_and_never_after_a_gate() {
        let now = Instant::now();
        assert!(due(None, now, false, false, false), "the first poll goes at once");
        let last = Some(now);
        assert!(!due(last, now + Duration::from_millis(1900), false, false, false));
        assert!(due(last, now + POLL_EVERY, false, false, false));
        assert!(!due(last, now + Duration::from_secs(9), true, false, false), "slower after a failure");
        assert!(due(last, now + POLL_FAILING, true, false, false));
        assert!(!due(None, now, false, true, false), "never two in flight");
        assert!(!due(last, now + Duration::from_secs(3600), false, true, false));
        assert!(!due(None, now, false, false, true), "never after a gate");
        assert!(!due(last, now + Duration::from_secs(3600), true, false, true));
    }

    #[test]
    fn the_worker_reads_a_canned_server() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (seen_tx, seen_rx) = channel::<String>();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let mut head = Vec::new();
            let mut byte = [0u8; 512];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&byte[..n]),
                }
            }
            let _ = seen_tx.send(String::from_utf8_lossy(&head).lines().next().unwrap_or_default().to_string());
            let body = concat!(
                r#"{"entries":["#,
                r#"{"seq":1,"t":"2026-10-02T09:15:04.120Z","level":"info","message":"server started"},"#,
                r#"{"seq":2,"t":"2026-10-02T09:15:05.000Z","level":"warn","message":"slow scan"}],"#,
                r#""lastSeq":2,"capacity":1000}"#,
            );
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes());
        });

        let client = Client::new(&format!("http://127.0.0.1:{port}")).expect("client");
        let mut log = LogUi::new(None).utc();
        log.start(client);
        assert!(log.running());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !log.model.answered && Instant::now() < deadline {
            log.pump(Instant::now());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(log.model.answered, "the canned answer arrived: {:?}", log.model.error);
        assert_eq!(texts(&log.model), ["server started", "slow scan"]);
        assert_eq!(log.model.shown()[1].level, Level::Warn);
        assert_eq!(log.model.cursor, 2);
        let request = seen_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(request.starts_with("GET /api/v1/admin/logs/recent?since=0 "), "{request}");
        log.stop();
        assert!(!log.running());
    }

    #[test]
    fn the_header_follows_in_the_ok_colour_or_pauses_dim_and_names_the_level() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 20));
        let mut ui = Surface::new();
        let area = Rect::new(4, 2, 70, 8);
        let look = Look { spaced: true, hint: Some(t!("gui.admin.log.hint_undock").to_string()) };
        let buf = render(&mut log, &mut ui, (80, 12), area, &look);

        let bullet = bullet_glyph();
        assert_eq!(buf[(4, 2)].symbol(), bullet);
        assert_eq!(buf[(4, 2)].fg, th().ok);
        let word = t!("gui.admin.log.following").to_string();
        let at = find(&buf, 2, &word).expect("following");
        assert_eq!(at, 6);
        assert_eq!(buf[(at, 2)].fg, th().ok);
        assert!(buf[(at, 2)].modifier.contains(Modifier::BOLD));
        let level = find(&buf, 2, &t!("gui.admin.log.level_info")).expect("the level");
        assert_eq!(buf[(level, 2)].fg, th().dim);
        assert_eq!(ui.hit(Position::new(level, 2)), Some(LogAct::Menu));
        let hint = t!("gui.admin.log.hint_undock").to_string();
        let hx = find(&buf, 2, &hint).expect("the hint");
        assert_eq!(hx + width(&hint) as u16, area.right(), "right-aligned in the area");
        assert_eq!(buf[(hx, 2)].fg, th().dim);
        assert_eq!(row(&buf, 3).trim(), "", "a blank row when spaced");
        assert!(row(&buf, 4).contains("line"), "lines from the third row");

        // Under the pointer the level is BRIGHT and BOLD.
        ui.pointer = Some(Position::new(level, 2));
        let buf = render(&mut log, &mut ui, (80, 12), area, &look);
        assert_eq!(buf[(level, 2)].fg, th().bright);
        assert!(buf[(level, 2)].modifier.contains(Modifier::BOLD));
        ui.pointer = None;

        // Paused: the bullet and the word dim, and the word follows again.
        log.key(key(KeyCode::Up), 5);
        let buf = render(&mut log, &mut ui, (80, 12), area, &look);
        let paused = find(&buf, 2, &t!("gui.admin.log.paused")).expect("paused");
        assert_eq!(buf[(4, 2)].fg, th().dim);
        assert_eq!(buf[(paused, 2)].fg, th().dim);
        assert_eq!(ui.hit(Position::new(paused, 2)), Some(LogAct::Follow));
        assert!(find(&buf, 2, &word).is_none());

        // Without a hint the row ends after the level.
        let buf = render(&mut log, &mut ui, (80, 12), area, &plain());
        assert!(find(&buf, 2, &hint).is_none());
        assert!(row(&buf, 3).contains("line"), "not spaced: lines from the second row");
    }

    #[test]
    fn a_paused_view_whose_lines_all_fit_says_following_on_the_same_frame() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 3));
        log.wheel(true);
        assert!(!log.model.following(), "no frame yet to clamp against");
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (60, 8), Rect::new(0, 0, 60, 8), &plain());
        assert!(log.model.following());
        assert!(find(&buf, 0, &t!("gui.admin.log.following")).is_some(), "{}", row(&buf, 0));
        log.wheel(true);
        assert!(log.model.following(), "the wheel clamps to the rows the last frame had");
    }

    #[test]
    fn lines_wear_a_dim_time_text_colour_gold_warnings_danger_errors() {
        let mut log = LogUi::new(None).utc();
        log.model.set_level(Level::Debug);
        log.model.take(tail(
            vec![
                entry(1, "info", "an info"),
                entry(2, "debug", "a debug"),
                entry(3, "warn", "a warning"),
                entry(4, "error", "an error"),
            ],
            4,
        ));
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (60, 6), Rect::new(0, 0, 60, 6), &plain());
        assert!(row(&buf, 1).starts_with("09:00:01  an info"), "{}", row(&buf, 1));
        assert_eq!(buf[(0, 1)].fg, th().dim, "the time is dim");
        for (y, color) in [(1, th().text), (2, th().text), (3, th().gold), (4, th().danger)] {
            assert_eq!(buf[(10, y)].fg, color, "row {y}: {}", row(&buf, y));
        }
        assert!(row(&buf, 4).contains("an error"), "the newest at the bottom");
    }

    #[test]
    fn the_level_menu_hangs_from_the_control_stays_inside_and_picks_with_keys_and_clicks() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 10));
        let mut ui = Surface::new();
        // The band's log: a header and seven rows.
        let area = Rect::new(19, 39, 115, 8);
        let buf = render(&mut log, &mut ui, (136, 52), area, &plain());
        let level = log.level_at.expect("the control was drawn");

        // The key opens it on the current level.
        log.key(key(KeyCode::Enter), 7);
        assert_eq!(log.menu, Some(2));
        let buf2 = render(&mut log, &mut ui, (136, 52), area, &plain());
        assert_eq!(buf2[(level.x, level.y + 1)].symbol(), "╭", "it hangs from the control's first cell");
        let bottom = level.y + 1 + 5;
        assert_eq!(buf2[(level.x, bottom)].symbol(), "╰", "four rows inside the band's eight");
        assert!(bottom < area.bottom());
        let info_row = level.y + 1 + 1 + 2;
        assert!(row(&buf2, info_row).contains(&format!("{} {}", bullet_glyph(), t!("gui.admin.log.level_info"))));
        assert_eq!(buf2[(level.x + 2, info_row)].bg, th().accent, "the cursor wears the slab");
        assert_ne!(row(&buf, info_row), row(&buf2, info_row));

        // Keys move the cursor and pick.
        log.menu_key(key(KeyCode::Down));
        log.menu_key(key(KeyCode::Char('x')));
        assert_eq!(log.menu, Some(3), "other keys are swallowed");
        log.menu_key(key(KeyCode::Enter));
        assert_eq!(log.model.level, Level::Debug);
        assert_eq!(log.menu, None);

        // Clicks: a row picks, anywhere else closes.
        log.act(LogAct::Menu);
        let _ = render(&mut log, &mut ui, (136, 52), area, &plain());
        let error_row = Position::new(level.x + 3, level.y + 2);
        assert_eq!(ui.hit(error_row), Some(LogAct::Pick(Level::Error)));
        assert_eq!(ui.hit(Position::new(0, 0)), Some(LogAct::MenuClose));
        assert_eq!(ui.hit(Position::new(level.x, level.y)), Some(LogAct::MenuClose), "the control again closes it");
        log.act(LogAct::Pick(Level::Error));
        assert_eq!(log.model.level, Level::Error);
        log.act(LogAct::Menu);
        log.menu_key(key(KeyCode::Esc));
        assert_eq!(log.menu, None);

        // Near the right edge it moves left to stay inside the log.
        let narrow = Rect::new(100, 2, 18, 20);
        log.act(LogAct::Menu);
        let buf = render(&mut log, &mut ui, (136, 52), narrow, &plain());
        let top = (0..136).find(|&x| buf[(x, 3)].symbol() == "╭").expect("the frame");
        let corner = (0..136).find(|&x| buf[(x, 3)].symbol() == "╮").expect("its corner");
        assert!(top >= narrow.x && corner < narrow.right(), "{top}..={corner} inside {narrow:?}");
    }

    #[test]
    fn capacity_zero_waiting_empty_and_failures_are_one_sentence() {
        let mut ui = Surface::new();
        let area = Rect::new(0, 0, 80, 5);
        let sentence = |log: &mut LogUi, ui: &mut Surface<LogAct>| {
            let buf = render(log, ui, (80, 5), area, &plain());
            let text = row(&buf, 1).trim_end().to_string();
            let fg = buf[(0, 1)].fg;
            assert_eq!(row(&buf, 2).trim(), "", "one sentence: {}", row(&buf, 2));
            (text, fg)
        };

        let mut log = LogUi::new(None);
        assert_eq!(sentence(&mut log, &mut ui), (t!("gui.admin.log.waiting").to_string(), th().dim));

        log.model.take(LogTail { entries: vec![], last_seq: 0, capacity: 0 });
        assert_eq!(sentence(&mut log, &mut ui), (t!("gui.admin.log.off").to_string(), th().dim));

        log.model.take(tail(vec![entry(1, "debug", "quiet")], 1));
        assert_eq!(sentence(&mut log, &mut ui), (t!("gui.admin.log.empty").to_string(), th().dim));

        log.model.fail(&ApiError::Network("connection refused".into()));
        let (text, fg) = sentence(&mut log, &mut ui);
        assert!(text.starts_with(&*t!("gui.admin.log.failed")), "{text}");
        assert_eq!(fg, th().gold);
        assert!(!log.model.gated, "a network failure only slows the poll");

        // With lines held, the failure takes the last row under them.
        log.model.set_level(Level::Debug);
        let buf = render(&mut log, &mut ui, (80, 5), area, &plain());
        assert!(row(&buf, 1).contains("quiet"));
        assert_eq!(buf[(0, 4)].fg, th().gold);
        assert!(row(&buf, 4).starts_with(&*t!("gui.admin.log.failed")));
    }

    #[test]
    fn a_gate_stops_the_poll_and_says_the_hubs_sentence() {
        for (e, gated) in [
            (ApiError::Unauthorized, true),
            (ApiError::Forbidden("admin only".into()), true),
            (ApiError::Server { status: 405, message: "locked".into() }, true),
            (ApiError::Server { status: 500, message: "boom".into() }, false),
        ] {
            let mut m = LogModel::new();
            m.fail(&e);
            assert_eq!(m.gated, gated, "{e:?}");
            assert_eq!(m.error.as_deref(), Some(&*gate_message(&e, &t!("gui.admin.log.failed"))));
        }

        // Through the pump: a refusal arrives and no request follows it,
        // however long the screen waits.
        let (jobs, job_rx) = channel::<u64>();
        let (done_tx, done) = channel::<Result<LogTail, ApiError>>();
        let mut log = LogUi::new(None);
        log.worker = Some(Worker { jobs, done, in_flight: false, last: None });
        let now = Instant::now();
        log.pump(now);
        assert_eq!(job_rx.try_recv(), Ok(0), "the first poll asks from the start");
        log.pump(now);
        assert!(job_rx.try_recv().is_err(), "never two at once");
        done_tx.send(Err(ApiError::Unauthorized)).unwrap();
        log.pump(now);
        assert!(log.model.gated);
        log.pump(now + Duration::from_secs(3600));
        assert!(job_rx.try_recv().is_err(), "the gate stops the poll");

        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (100, 4), Rect::new(0, 0, 100, 4), &plain());
        assert_eq!(row(&buf, 1).trim_end(), t!("admin.gate_unauthorized"));
    }
}
