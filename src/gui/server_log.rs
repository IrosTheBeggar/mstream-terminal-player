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
//!
//! The lines can leave the player two ways (clauses 29-33). A press-drag
//! across them highlights a run of whole lines, anchored by sequence
//! number so it stays on the same lines as new ones arrive, and `y` copies
//! the highlight, or every line shown, through the kit's clipboard. `d`
//! downloads the server's own log files on a one-shot thread of its own,
//! never the poll's, so a zip that takes a minute never holds up the tail,
//! and `o` shows the file it saved. Their answers come back as a note the
//! screen lifts into the GUI's.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use super::log_file::{self, Fallback, SaveError, Saved};
use super::opener::{self, HandOff};
use super::{bright_bold, bullet_glyph, put, sel};
use crate::admin::tz::Zone;
use crate::admin::{gate_message, iso_unix, printable};
use crate::api::types::LogTail;
use crate::api::{ApiError, Client};
use crate::kit::clipboard::{self, Copied};
use crate::kit::os::{Os, process_var};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{Grip, Surface, dim, frame_at, width};

/// How many lines the player keeps; older ones fall off the front.
pub(crate) const RING: usize = 1000;
/// The pause between an answer and the next request.
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(2);
/// The pause after a failed request, so a server that is down is not
/// asked thirty times a minute.
pub(crate) const POLL_FAILING: Duration = Duration::from_secs(10);
/// How many characters of a message's first line the row keeps. The
/// widest log the layouts draw is far narrower; the whole message, which
/// a copy takes, is kept apart in [`Line::whole`].
const KEEP_CHARS: usize = 400;
/// The longest message kept whole, for a copy: what mStream's own logger
/// cuts a message to, so nothing the server holds is lost.
const MESSAGE_CHARS: usize = 4000;
/// What a copy puts before a message's later lines, so a stack trace
/// reads under its time stamp: the width of `HH:MM:SS` and its gap.
const LATER_LINES: &str = "          ";

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
/// printable, whether anything followed it (a stack trace), and the whole
/// message made printable for a copy.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub seq: u64,
    /// Unix seconds, when the server's time parsed.
    pub at: Option<i64>,
    pub level: Level,
    pub text: String,
    pub more: bool,
    /// Every line of the message (see [`whole_message`]).
    pub whole: String,
}

impl Line {
    fn from_entry(entry: &crate::api::types::ActivityEntry) -> Line {
        // The first line with something on it: a message that opens on a
        // newline still says what it is about.
        let mut parts = entry.message.split('\n').filter(|l| !printable(l, 1).is_empty());
        let text = parts.next().map(|l| printable(l, KEEP_CHARS)).unwrap_or_default();
        let more = parts.next().is_some();
        let whole = whole_message(&entry.message);
        Line { seq: entry.seq, at: iso_unix(&entry.t), level: Level::of(&entry.level), text, more, whole }
    }
}

/// A message as a copy carries it: every line, each made printable the
/// way a row is (no character that acts on a terminal or reorders a
/// reader, a tab as four spaces) and trimmed at its end only, so a stack
/// trace keeps its indentation. Blank lines at either end go; the whole is
/// cut at [`MESSAGE_CHARS`].
fn whole_message(raw: &str) -> String {
    let lines: Vec<String> = raw
        .split('\n')
        .map(|line| {
            line.replace('\t', "    ")
                .chars()
                .filter(|c| !(c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    let Some(first) = lines.iter().position(|l| !l.is_empty()) else { return String::new() };
    let last = lines.iter().rposition(|l| !l.is_empty()).unwrap_or(first);
    let whole: String = lines[first..=last].join("\n").chars().take(MESSAGE_CHARS).collect();
    whole.trim_end().to_string()
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
    /// The highlighted run: the sequence numbers of the line a drag began
    /// on and the one it reached, in either order. Anchored by line rather
    /// than by row, so arrivals and the ring's drops leave it where it was;
    /// it goes when its last line leaves the ring.
    pub highlight: Option<(u64, u64)>,
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
            highlight: None,
        }
    }

    /// Fold an answer in. A `lastSeq` below the cursor is a server that
    /// restarted and answered its whole new ring: the old lines go and
    /// every new one counts as unseen. Otherwise only entries past the
    /// newest line held are appended, so an answer that repeats itself
    /// adds nothing. A paused view grows its offset by the lines that
    /// arrived under it, so what the reader is looking at stays put. A
    /// highlight goes with a restarted ring, and once the ring has dropped
    /// every line of it; the view keeps following under one.
    pub(crate) fn take(&mut self, tail: LogTail) {
        if self.answered && tail.last_seq < self.cursor {
            self.lines.clear();
            self.seen = 0;
            self.scroll = 0;
            self.highlight = None;
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
        if let Some((anchor, head)) = self.highlight
            && self.lines.front().is_none_or(|front| anchor.max(head) < front.seq)
        {
            self.highlight = None;
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
    /// of a different list, and the highlight lines the reader no longer
    /// sees.
    pub(crate) fn set_level(&mut self, level: Level) {
        self.level = level;
        self.highlight = None;
        self.follow();
    }

    /// Whether the line numbered `seq` is in the highlight.
    fn lit(&self, seq: u64) -> bool {
        self.highlight.is_some_and(|(a, h)| (a.min(h)..=a.max(h)).contains(&seq))
    }

    /// The shown lines in the highlight, oldest first.
    pub(crate) fn highlighted(&self) -> Vec<&Line> {
        self.lines.iter().filter(|l| l.level <= self.level && self.lit(l.seq)).collect()
    }

    /// What `y` copies: the highlight, or every line shown when nothing is
    /// highlighted.
    pub(crate) fn to_copy(&self) -> Vec<&Line> {
        let lit = self.highlighted();
        if lit.is_empty() { self.shown() } else { lit }
    }

    pub(crate) fn clear_highlight(&mut self) {
        self.highlight = None;
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

/// One line as a copy writes it: the clock, two spaces, the level for a
/// warning or an error in winston's own word (what the server wrote, so
/// it is never translated), and the whole message, its later lines set
/// under the message's first by [`LATER_LINES`]. A blank later line stays
/// blank rather than carry the indent as trailing spaces.
pub(crate) fn copy_line(line: &Line, zone: Option<&Zone>) -> String {
    let mut out = clock(line.at, zone);
    out.push_str("  ");
    match line.level {
        Level::Error => out.push_str("error  "),
        Level::Warn => out.push_str("warn  "),
        Level::Info | Level::Debug => {}
    }
    let mut parts = line.whole.split('\n');
    out.push_str(parts.next().unwrap_or_default());
    for part in parts {
        out.push('\n');
        if !part.is_empty() {
            out.push_str(LATER_LINES);
            out.push_str(part);
        }
    }
    if line.whole.is_empty() {
        out.truncate(out.trim_end().len());
    }
    out
}

/// Lines as a copy carries them: one [`copy_line`] each, oldest first,
/// joined by newlines with none after the last.
pub(crate) fn as_text(lines: &[&Line], zone: Option<&Zone>) -> String {
    lines.iter().map(|line| copy_line(line, zone)).collect::<Vec<_>>().join("\n")
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
    /// The lines' drag region: a press, a move of the held button, or its
    /// release, wherever the pointer is.
    Select(Grip, Position),
    /// The header's copy control.
    CopyLines,
    /// The header's download control.
    Download,
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

/// A press on the lines, held: the line it began on, where the pointer
/// is now and where it pressed, and whether it has moved off that cell,
/// which is what tells a drag from a plain click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hold {
    anchor: u64,
    at: Position,
    press: Position,
    moved: bool,
}

/// What the log's one-shot threads send back: a download saved (or why
/// not), and a saved file shown (or why not).
enum Side {
    Saved(Result<Saved, SaveError>),
    Shown(PathBuf, Result<HandOff, String>),
}

/// The log as a screen holds it: the model, the level menu's cursor
/// while it is open, the poll thread, where the last frame put it, and
/// what the copy and the download need and say.
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
    /// The session's client, shared with the poll thread, for a download.
    client: Option<Arc<Client>>,
    /// The server's part of a saved file's name.
    label: String,
    /// A press on the lines, until its release or until the log leaves
    /// the screen.
    hold: Option<Hold>,
    /// The line rows the last frame drew (the rows under the lines too)
    /// and the sequence number on each drawn row, top first.
    rows_at: Option<(Rect, Vec<u64>)>,
    /// The one-shot threads' channel. It outlives a stop, so a download
    /// still running when the tab is left lands on the next visit.
    side: (Sender<Side>, Receiver<Side>),
    downloading: bool,
    /// The file the last download saved, for `o`.
    saved: Option<PathBuf>,
    /// What the last copy, download or show said, and whether it is a
    /// failure, until the screen takes it.
    note: Option<(String, bool)>,
    /// Where a download saves instead of the Downloads folder: a test's
    /// own temporary folder.
    save_dir: Option<PathBuf>,
}

impl LogUi {
    pub(crate) fn new(zone: Option<Zone>) -> Self {
        LogUi {
            model: LogModel::new(),
            menu: None,
            zone,
            worker: None,
            level_at: None,
            drawn_in: None,
            rows: 0,
            client: None,
            // What a server with no name is called until one is set.
            label: log_file::file_label(""),
            hold: None,
            rows_at: None,
            side: channel(),
            downloading: false,
            saved: None,
            note: None,
            save_dir: None,
        }
    }

    /// Start reading `client`'s log from the beginning of its ring, on a
    /// thread of its own: it takes a `since`, makes the one blocking call,
    /// and sends the answer back, until the screen lets go of it. The
    /// client is kept too, for a download.
    pub(crate) fn start(&mut self, client: Client) {
        self.stop();
        self.model = LogModel::new();
        self.menu = None;
        let client = Arc::new(client);
        let polling = Arc::clone(&client);
        let (jobs, job_rx) = channel::<u64>();
        let (done_tx, done) = channel::<Result<LogTail, ApiError>>();
        let spawned = std::thread::Builder::new().name("server log".into()).spawn(move || {
            while let Ok(since) = job_rx.recv() {
                if done_tx.send(polling.admin_logs_recent(since)).is_err() {
                    break;
                }
            }
        });
        if spawned.is_ok() {
            self.worker = Some(Worker { jobs, done, in_flight: false, last: None });
        }
        self.client = Some(client);
    }

    /// Let go of the poll thread. It finishes the call it is in, finds
    /// nobody to answer, and ends. A download in flight keeps its thread
    /// and its way back.
    pub(crate) fn stop(&mut self) {
        self.worker = None;
        self.menu = None;
        self.client = None;
        self.hold = None;
        self.rows_at = None;
    }

    /// Name the server a saved file is named after (see
    /// [`log_file::file_label`]).
    pub(crate) fn set_label(&mut self, server: &str) {
        self.label = log_file::file_label(server);
    }

    /// Whether a poll thread is held: the tests' way to see the tab start
    /// and stop the log.
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.worker.is_some()
    }

    /// Take whatever answers have arrived, the one-shot threads' first,
    /// then send the next request when one is due. Never blocks.
    pub(crate) fn pump(&mut self, now: Instant) {
        while let Ok(answer) = self.side.1.try_recv() {
            self.land(answer);
        }
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

    /// A one-shot thread's answer, as the note it leaves. A download's
    /// answer ends the download either way.
    fn land(&mut self, answer: Side) {
        let full = |path: &PathBuf| path.display().to_string();
        let note = match answer {
            Side::Saved(saved) => {
                self.downloading = false;
                let home = process_var("HOME").map(PathBuf::from);
                let shown = |path: &PathBuf| log_file::shown_path(path, home.as_deref(), Os::HERE);
                match saved {
                    Ok(Saved::Zip(path)) => {
                        let note = t!("gui.admin.log.saved", path = shown(&path)).to_string();
                        self.saved = Some(path);
                        (note, false)
                    }
                    Ok(Saved::Text { path, why }) => {
                        let note = match why {
                            Fallback::NoRoute => t!("gui.admin.log.saved_lines", path = shown(&path)),
                            Fallback::NoFiles => t!("gui.admin.log.saved_no_files", path = shown(&path)),
                        };
                        self.saved = Some(path);
                        (note.to_string(), false)
                    }
                    Err(SaveError::Api(e)) => (gate_message(&e, &t!("gui.admin.log.download_failed")), true),
                    Err(SaveError::Cut) => (t!("gui.admin.log.zip_cut").to_string(), true),
                    Err(SaveError::Nothing) => (t!("gui.admin.log.save_nothing").to_string(), true),
                    Err(SaveError::NoFolder) => (t!("gui.admin.log.no_folder").to_string(), true),
                    Err(SaveError::Io(err)) => (t!("gui.admin.log.save_failed", err = err).to_string(), true),
                }
            }
            // The file manager came up: it says enough.
            Side::Shown(_, Ok(HandOff::Launched)) => return,
            Side::Shown(path, Ok(HandOff::Headless(_))) => {
                (t!("gui.admin.log.file_at", path = full(&path)).to_string(), false)
            }
            Side::Shown(path, Ok(HandOff::Nothing) | Err(_)) => {
                (t!("gui.admin.log.show_failed", path = full(&path)).to_string(), true)
            }
        };
        self.note = Some(note);
    }

    /// `y`: the highlight, or every line shown, onto the clipboard by the
    /// kit's route, and a note saying which and how.
    pub(crate) fn copy(&mut self) {
        let lit = !self.model.highlighted().is_empty();
        let lines = self.model.to_copy();
        if lines.is_empty() {
            self.note = Some((t!("gui.admin.log.copy_nothing").to_string(), false));
            return;
        }
        let text = as_text(&lines, self.zone.as_ref());
        self.note = Some(match clipboard::copy(&text) {
            Copied::Clipboard if lit => (t!("gui.admin.log.copied_highlight").to_string(), false),
            Copied::Clipboard => (t!("gui.admin.log.copied_all").to_string(), false),
            Copied::Terminal => (t!("gui.admin.log.copied_terminal").to_string(), false),
            Copied::Failed => (t!("gui.admin.log.copy_failed").to_string(), true),
        });
    }

    /// `d`: the server's log files, fetched and saved on a thread of their
    /// own. What it saves instead when the server has none (the lines
    /// shown) is taken now, as the reader sees them. One at a time, and
    /// only with a session to ask.
    pub(crate) fn download(&mut self) {
        if self.downloading {
            return;
        }
        let Some(client) = self.client.clone() else { return };
        let lines = as_text(&self.model.shown(), self.zone.as_ref());
        let stem = log_file::file_stem(&self.label, crate::admin::unix_now(), self.zone.as_ref());
        let dir = self.save_dir.clone();
        let side = self.side.0.clone();
        let spawned = std::thread::Builder::new().name("server log download".into()).spawn(move || {
            let saved = save(client.admin_logs_download(), dir, &stem, &lines);
            let _ = side.send(Side::Saved(saved));
        });
        self.note = Some(match spawned {
            Ok(_) => {
                self.downloading = true;
                (t!("gui.admin.log.fetching").to_string(), false)
            }
            Err(e) => (t!("gui.admin.log.save_failed", err = e.to_string()).to_string(), true),
        });
    }

    /// `o`: the file the last download saved, shown in the file manager on
    /// a thread, since the opener is watched for a moment.
    pub(crate) fn show_saved(&mut self) {
        let Some(path) = self.saved.clone() else {
            self.note = Some((t!("gui.admin.log.nothing_saved").to_string(), false));
            return;
        };
        let side = self.side.0.clone();
        let shown = path.clone();
        let spawned = std::thread::Builder::new().name("server log show".into()).spawn(move || {
            let answer = opener::reveal(&shown);
            let _ = side.send(Side::Shown(shown, answer));
        });
        if spawned.is_err() {
            self.note = Some((t!("gui.admin.log.show_failed", path = path.display().to_string()).to_string(), true));
        }
    }

    /// Whether a download is in flight: the header's busy word.
    pub(crate) fn downloading(&self) -> bool {
        self.downloading
    }

    /// The note the last copy, download or show left, once.
    pub(crate) fn take_note(&mut self) -> Option<(String, bool)> {
        self.note.take()
    }

    /// The log is not on screen: a press held on it lets go, and the rows
    /// it was drawn on no longer stand anywhere. The highlight stays.
    pub(crate) fn let_go(&mut self) {
        self.hold = None;
        self.rows_at = None;
    }

    /// The line on screen row `y` of the last frame, clamped to the first
    /// and the last line drawn, so a drag past either end stops there.
    fn seq_at(&self, y: u16) -> Option<u64> {
        let (rect, seqs) = self.rows_at.as_ref()?;
        let row = (y.saturating_sub(rect.y) as usize).min(seqs.len().checked_sub(1)?);
        seqs.get(row).copied()
    }

    /// The press-drag on the lines (clause 29). A press anchors on its line
    /// and clears the highlight; once the pointer has moved off the pressed
    /// cell, the highlight runs from the anchor to the line under it, the
    /// release's included (a terminal that reports no moves between still
    /// highlights the run). A release that never moved was a plain click,
    /// which leaves nothing highlighted. A move with no press held, or none
    /// of the log on screen, is nobody's.
    fn select(&mut self, grip: Grip, at: Position) {
        match grip {
            Grip::Press => {
                self.model.clear_highlight();
                self.hold = self.seq_at(at.y).map(|anchor| Hold { anchor, at, press: at, moved: false });
            }
            Grip::Drag => {
                let (Some(head), Some(hold)) = (self.seq_at(at.y), self.hold.as_mut()) else { return };
                hold.moved |= at != hold.press;
                hold.at = at;
                if hold.moved {
                    self.model.highlight = Some((hold.anchor, head));
                }
            }
            Grip::Release => {
                let Some(mut hold) = self.hold.take() else { return };
                hold.moved |= at != hold.press;
                match self.seq_at(at.y) {
                    Some(head) if hold.moved => self.model.highlight = Some((hold.anchor, head)),
                    _ if !hold.moved => self.model.clear_highlight(),
                    _ => {}
                }
            }
        }
    }

    /// A key while the log has the focus; `rows` is how many lines it
    /// shows. Every key is the log's except the two that leave it; Esc
    /// clears a highlight before it leaves.
    pub(crate) fn key(&mut self, key: KeyEvent, rows: usize) -> LogKey {
        let page = rows.max(1) as i32;
        match key.code {
            KeyCode::Esc if self.model.highlight.is_some() => self.model.clear_highlight(),
            KeyCode::Esc | KeyCode::Char('q') => return LogKey::Leave,
            KeyCode::Up => self.model.scroll_by(1),
            KeyCode::Down => self.model.scroll_by(-1),
            KeyCode::PageUp => self.model.scroll_by(page),
            KeyCode::PageDown => self.model.scroll_by(-page),
            KeyCode::Home => self.model.scroll = usize::MAX,
            KeyCode::End | KeyCode::Char('f') => self.model.follow(),
            KeyCode::Enter => self.menu = Some(self.model.level.index()),
            KeyCode::Char('y') => self.copy(),
            KeyCode::Char('d') => self.download(),
            KeyCode::Char('o') => self.show_saved(),
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
            LogAct::Select(grip, at) => self.select(grip, at),
            LogAct::CopyLines => self.copy(),
            LogAct::Download => self.download(),
        }
    }

    /// The same log on UTC, so a test's clock reads the same anywhere.
    #[cfg(test)]
    pub(crate) fn utc(mut self) -> Self {
        self.zone = None;
        self
    }

    /// The same log saving its downloads into `dir`, a test's own folder,
    /// never the Downloads folder.
    #[cfg(test)]
    pub(crate) fn saving_into(mut self, dir: PathBuf) -> Self {
        self.save_dir = Some(dir);
        self
    }

    /// The server's part of a saved file's name, as a test reads it.
    #[cfg(test)]
    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

/// The download's answer, saved where it belongs (see
/// [`log_file::save_download`]). A refusal, or a server that cannot be
/// reached, leaves the disk alone: the folder is looked for, and the
/// config folder made as the last resort, only once there is something to
/// put in it.
fn save(answer: Result<Vec<u8>, ApiError>, dir: Option<PathBuf>, stem: &str, lines: &str) -> Result<Saved, SaveError> {
    let answer = match answer {
        Err(e) if !matches!(e, ApiError::NotFound(_)) => return Err(SaveError::Api(e)),
        answer => answer,
    };
    match dir.or_else(log_file::downloads_here) {
        Some(dir) => log_file::save_download(answer, &dir, stem, lines),
        None => Err(SaveError::NoFolder),
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
/// that stopped never passes for one that is following. The line rows,
/// and the empty rows under them, are a drag region for the highlight;
/// highlighted lines wear the selection colours across the log's width.
pub(crate) fn draw<A: Clone + 'static>(
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
    log.rows_at = None;
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
    // The copy and the download, for the pointer: the GUI's footer of
    // keys is off by default. Each is drawn whole or not at all, and one
    // that does not fit ends the row; the hint gives way before them.
    let controls = [
        (t!("gui.admin.log.copy"), Some((LogAct::CopyLines, t!("gui.admin.log.tip_copy")))),
        if log.downloading() {
            (t!("gui.admin.log.downloading"), None)
        } else {
            (t!("gui.admin.log.download"), Some((LogAct::Download, t!("gui.admin.log.tip_download"))))
        },
    ];
    for (word, act) in controls {
        let w = width(&word) as u16;
        if x + width(" · ") as u16 + w > right {
            break;
        }
        x += span(frame, x, y, right, " · ", dim());
        let rect = Rect { x, y, width: w, height: 1 };
        match act {
            Some((act, tip)) => {
                let style = if ui.hovers(rect) { bright_bold() } else { dim() };
                span(frame, x, y, right, &word, style);
                ui.click(rect, wrap(act));
                ui.tip_keyed(rect, tip);
            }
            // The kit's busy word: the accent, and nothing to press.
            None => {
                span(frame, x, y, right, &word, Style::default().fg(th().accent));
            }
        }
        x += w;
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

    // The rows the lines stand on this frame, and the drag region over
    // them and the empty rows under them (never the header, nor the
    // failure's row). A drag in progress re-reads the line under the hand
    // against these rows, so the wheel, or lines arriving, under a still
    // pointer move the highlight with the lines.
    let end = held - log.model.scroll;
    let first = end.saturating_sub(rows);
    let seqs: Vec<u64> = log.model.shown()[first..end].iter().map(|l| l.seq).collect();
    if !seqs.is_empty() {
        let lines = Rect { x: area.x, y: top, width: area.width, height: rows as u16 };
        log.rows_at = Some((lines, seqs));
        if let Some(hold) = log.hold.filter(|h| h.moved)
            && let Some(head) = log.seq_at(hold.at.y)
        {
            log.model.highlight = Some((hold.anchor, head));
        }
        ui.drag_region(lines, move |grip, at| wrap(LogAct::Select(grip, at)));
    }

    // The lines, the newest at the bottom of the window onto them; a
    // highlighted one in the selection colours from edge to edge.
    let shown = log.model.shown();
    let more = if legacy_conhost() { " »" } else { " …" };
    for (row, line) in shown[first..end].iter().enumerate() {
        let ly = top + row as u16;
        let lit = log.model.lit(line.seq);
        if lit {
            put(frame, area.x, ly, &" ".repeat(area.width as usize), sel());
        }
        let color = match line.level {
            Level::Error => th().danger,
            Level::Warn => th().gold,
            Level::Info | Level::Debug => th().text,
        };
        let (time, words) = if lit { (sel(), sel()) } else { (dim(), Style::default().fg(color)) };
        let mut lx = area.x;
        lx += span(frame, lx, ly, right, &clock(line.at, log.zone.as_ref()), time);
        lx += span(frame, lx, ly, right, "  ", if lit { sel() } else { Style::default() });
        let text = if line.more { format!("{}{more}", line.text) } else { line.text.clone() };
        span(frame, lx, ly, right, &text, words);
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

    /// The x of the first cell of `needle` on row `y`, counted in cells:
    /// the needle walked by graphemes, each at the cells ratatui fills.
    fn find(buf: &Buffer, y: u16, needle: &str) -> Option<u16> {
        use unicode_segmentation::UnicodeSegmentation;
        let first = needle.graphemes(true).next()?;
        (0..buf.area.width).find(|&x| {
            buf[(x, y)].symbol() == first && {
                let mut cx = x;
                needle.graphemes(true).all(|g| {
                    let ok = cx < buf.area.width && buf[(cx, y)].symbol() == g;
                    cx += crate::kit::grapheme_cells(g).max(1) as u16;
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

    // ── Copy, download and the highlight ────────────────────────────────────

    /// The locale pinned to English while a test reads the log's words.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn seqs(lines: &[&Line]) -> Vec<u64> {
        lines.iter().map(|l| l.seq).collect()
    }

    /// A folder of the test's own under the system's temporary folder,
    /// gone when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("mstream-log-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn files(&self) -> Vec<PathBuf> {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().path()).collect();
            files.sort();
            files
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A server answering the log's two routes: a tail that adds nothing
    /// to the poll, and `status` with `body` to the download. Each request
    /// line comes back on the receiver.
    fn canned(status: u16, body: Vec<u8>) -> (String, Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (seen_tx, seen) = channel::<String>();
        std::thread::spawn(move || {
            for sock in listener.incoming() {
                let Ok(mut sock) = sock else { continue };
                let mut head = Vec::new();
                let mut byte = [0u8; 512];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut byte) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&byte[..n]),
                    }
                }
                let line = String::from_utf8_lossy(&head).lines().next().unwrap_or_default().to_string();
                let (code, kind, bytes) = if line.contains("/logs/download") {
                    let kind = if body.starts_with(b"PK") { "application/zip" } else { "application/json" };
                    (status, kind, body.clone())
                } else {
                    (200, "application/json", br#"{"entries":[],"lastSeq":999999,"capacity":1000}"#.to_vec())
                };
                let _ = seen_tx.send(line);
                let reply = format!(
                    "HTTP/1.1 {code} Canned\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = sock.write_all(reply.as_bytes()).and_then(|()| sock.write_all(&bytes));
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    /// A zip by the signatures the player checks: a local file header at
    /// the start, the end-of-central-directory record at the very end.
    fn zip_bytes() -> Vec<u8> {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.extend_from_slice(b"\x14\x00\x00\x00\x00\x00 a log file's bytes \xff\xfe");
        bytes.extend_from_slice(&empty_zip());
        bytes
    }

    /// The zip of no files: the end record alone.
    fn empty_zip() -> Vec<u8> {
        let mut bytes = b"PK\x05\x06".to_vec();
        bytes.extend_from_slice(&[0; 18]);
        bytes
    }

    /// Pump until a note other than the fetching one comes up.
    fn landed(log: &mut LogUi) -> (String, bool) {
        let fetching = t!("gui.admin.log.fetching").to_string();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            log.pump(Instant::now());
            if let Some(note) = log.take_note().filter(|(text, _)| *text != fetching) {
                return note;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no note within ten seconds");
    }

    /// A log reading `server`, saving into `dir`, on UTC.
    fn downloading_log(server: &str, dir: &Scratch) -> LogUi {
        let mut log = LogUi::new(None).utc().saving_into(dir.0.clone());
        log.start(Client::new(server).expect("client"));
        log.set_label(server);
        log
    }

    #[test]
    fn whole_messages_are_printable_kept_whole_and_capped() {
        let raw = "\n  \nError: ENOENT \u{1b}[31mred\u{1b}[0m\r\n\tat open (fs.js:1:1)\n    at \u{202E}main\u{2066}  \n\n";
        let line = Line::from_entry(&entry(1, "error", raw));
        assert_eq!(line.whole, "Error: ENOENT [31mred[0m\n    at open (fs.js:1:1)\n    at main");
        assert!(line.whole.chars().all(|c| c == '\n' || !c.is_control()), "{:?}", line.whole);
        assert_eq!(line.text, "Error: ENOENT [31mred[0m", "the row's text is as it was");
        assert!(line.more);

        let long = Line::from_entry(&entry(2, "info", &"x".repeat(5000)));
        assert_eq!(long.whole.chars().count(), MESSAGE_CHARS);
        assert_eq!(long.text.chars().count(), KEEP_CHARS, "the row keeps its own cap");
        assert!(!long.more);

        assert_eq!(Line::from_entry(&entry(3, "info", "\n \n")).whole, "", "nothing on any line");
        assert_eq!(Line::from_entry(&entry(4, "info", "a\n\nb")).whole, "a\n\nb", "a blank line between stays");
    }

    #[test]
    fn a_highlight_is_anchored_by_seq_and_survives_the_ring_dropping_old_lines() {
        let mut m = LogModel::new();
        m.take(infos(1, 1000));
        m.highlight = Some((510, 500));
        m.take(infos(1001, 300));
        assert_eq!(m.shown()[0].seq, 301, "the ring dropped the oldest three hundred");
        assert_eq!(seqs(&m.highlighted()), (500..=510).collect::<Vec<_>>());
        m.take(infos(1301, 209));
        assert_eq!(seqs(&m.highlighted()), [510], "its last line still held");
        m.take(infos(1510, 1));
        assert_eq!(m.highlight, None, "gone with its last line");
    }

    #[test]
    fn a_highlight_clears_on_a_new_level_a_restart_and_a_new_server() {
        let mut m = LogModel::new();
        m.take(infos(1, 10));
        m.highlight = Some((3, 5));
        m.set_level(Level::Warn);
        assert_eq!(m.highlight, None, "a new level");

        m.set_level(Level::Info);
        m.highlight = Some((3, 5));
        m.take(infos(11, 2));
        assert_eq!(m.highlight, Some((3, 5)), "arrivals keep it");
        m.take(infos(1, 2));
        assert_eq!(m.highlight, None, "a restarted ring");

        let mut log = LogUi::new(None);
        log.model.take(infos(1, 10));
        log.model.highlight = Some((3, 5));
        log.start(Client::new("http://127.0.0.1:9").unwrap());
        assert_eq!(log.model.highlight, None, "a new server");
        log.stop();
    }

    #[test]
    fn following_keeps_going_under_a_highlight() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 10));
        log.model.highlight = Some((3, 5));
        log.model.take(infos(11, 5));
        assert!(log.model.following(), "new lines still pull the view along");
        assert_eq!(log.model.scroll, 0);
        assert_eq!(seqs(&log.model.highlighted()), [3, 4, 5]);
    }

    #[test]
    fn esc_clears_a_highlight_before_it_leaves_the_log() {
        let mut log = LogUi::new(None);
        log.model.take(infos(1, 10));
        log.model.highlight = Some((3, 5));
        assert_eq!(log.key(key(KeyCode::Esc), 5), LogKey::Taken);
        assert_eq!(log.model.highlight, None);
        assert_eq!(log.key(key(KeyCode::Esc), 5), LogKey::Leave);

        log.model.highlight = Some((3, 5));
        assert_eq!(log.key(key(KeyCode::Char('q')), 5), LogKey::Leave);
        assert_eq!(log.model.highlight, Some((3, 5)), "q leaves and keeps it");
    }

    #[test]
    fn a_press_drag_and_release_highlight_and_a_plain_click_clears() {
        // Thirty lines in eight rows: the header, then 24..=30 on rows 1-7.
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 30));
        let mut ui = Surface::new();
        let size = (60, 8);
        let area = Rect::new(0, 0, 60, 8);
        render(&mut log, &mut ui, size, area, &plain());
        let at = |x, y| Position::new(x, y);
        let select = |log: &mut LogUi, grip, x, y| log.act(LogAct::Select(grip, at(x, y)));

        // Down and up on one row, from row 2 to row 4.
        select(&mut log, Grip::Press, 5, 2);
        select(&mut log, Grip::Drag, 5, 3);
        select(&mut log, Grip::Drag, 7, 4);
        select(&mut log, Grip::Release, 7, 4);
        assert_eq!(log.model.highlight, Some((25, 27)));

        // A plain click on the lines clears it, and lights nothing itself.
        select(&mut log, Grip::Press, 5, 6);
        select(&mut log, Grip::Drag, 5, 6);
        assert_eq!(log.model.highlight, None);
        select(&mut log, Grip::Release, 5, 6);
        assert_eq!(log.model.highlight, None);

        // A sideways drag on one row highlights that row.
        select(&mut log, Grip::Press, 5, 6);
        select(&mut log, Grip::Drag, 30, 6);
        select(&mut log, Grip::Release, 30, 6);
        assert_eq!(log.model.highlight, Some((29, 29)));

        // Past the first and the last drawn line, the drag stops at them.
        select(&mut log, Grip::Press, 5, 3);
        select(&mut log, Grip::Drag, 5, 0);
        assert_eq!(log.model.highlight, Some((26, 24)), "the header's row reads as the first line");
        select(&mut log, Grip::Drag, 5, 40);
        assert_eq!(log.model.highlight, Some((26, 30)), "below the rows, the last");
        select(&mut log, Grip::Release, 90, 40);
        assert_eq!(log.model.highlight, Some((26, 30)), "released anywhere");

        // A press and a release on different rows with no move between.
        select(&mut log, Grip::Press, 5, 1);
        select(&mut log, Grip::Release, 5, 3);
        assert_eq!(log.model.highlight, Some((24, 26)));

        // A move with no press held, or once the log let go, is nobody's.
        select(&mut log, Grip::Drag, 5, 7);
        assert_eq!(log.model.highlight, Some((24, 26)));
        select(&mut log, Grip::Press, 5, 1);
        select(&mut log, Grip::Drag, 5, 2);
        log.let_go();
        select(&mut log, Grip::Drag, 5, 7);
        select(&mut log, Grip::Release, 5, 7);
        assert_eq!(log.model.highlight, Some((24, 25)));
    }

    #[test]
    fn the_wheel_during_a_drag_moves_the_head_with_the_lines() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 30));
        let mut ui = Surface::new();
        let area = Rect::new(0, 0, 60, 8);
        render(&mut log, &mut ui, (60, 8), area, &plain());
        log.act(LogAct::Select(Grip::Press, Position::new(5, 2)));
        log.act(LogAct::Select(Grip::Drag, Position::new(5, 5)));
        assert_eq!(log.model.highlight, Some((25, 28)));
        // The pointer stays on row 5 while the wheel brings older lines.
        log.wheel(true);
        let buf = render(&mut log, &mut ui, (60, 8), area, &plain());
        assert_eq!(log.model.highlight, Some((25, 27)), "the head is the line now under the hand");
        assert!(row(&buf, 5).contains("line 27"), "{}", row(&buf, 5));
        assert_eq!(buf[(40, 5)].bg, th().accent);
        // Lines arriving under a following view move it the other way.
        log.wheel(false);
        log.model.take(infos(31, 2));
        render(&mut log, &mut ui, (60, 8), area, &plain());
        assert_eq!(log.model.highlight, Some((25, 30)));
        // Released, it stays where it was let go.
        log.act(LogAct::Select(Grip::Release, Position::new(5, 5)));
        log.model.take(infos(33, 2));
        render(&mut log, &mut ui, (60, 8), area, &plain());
        assert_eq!(log.model.highlight, Some((25, 30)));
    }

    #[test]
    fn the_copy_text_is_the_clock_a_level_word_and_the_whole_message() {
        let mut m = LogModel::new();
        m.set_level(Level::Debug);
        let mut unparsed = entry(5, "debug", "no time");
        unparsed.t = "yesterday".into();
        m.take(tail(
            vec![
                entry(1, "info", "server started"),
                entry(2, "warn", "slow scan"),
                entry(3, "error", "Error: ENOENT\n    at open (fs.js:1:1)\n\n  at main"),
                entry(4, "error", ""),
                unparsed,
            ],
            5,
        ));
        let text = as_text(&m.shown(), None);
        assert_eq!(
            text,
            "09:00:01  server started\n\
             09:00:02  warn  slow scan\n\
             09:00:03  error  Error: ENOENT\n              at open (fs.js:1:1)\n\n            at main\n\
             09:00:04  error\n\
             --:--:--  no time"
        );
        assert!(!text.ends_with('\n'));
        // The clock is the machine's, as the rows show it.
        if let Some(zone) = crate::admin::tz::local() {
            let line = &m.shown()[0];
            assert!(copy_line(line, Some(&zone)).starts_with(&clock(line.at, Some(&zone))));
        }
    }

    #[test]
    fn y_copies_the_highlight_or_every_shown_line_and_says_how() {
        let _en = english();
        let mut log = LogUi::new(None).utc();
        let note = |log: &mut LogUi| log.take_note().expect("a note");

        clipboard::catch(Copied::Clipboard);
        assert_eq!(log.key(key(KeyCode::Char('y')), 5), LogKey::Taken);
        assert_eq!(note(&mut log), (t!("gui.admin.log.copy_nothing").to_string(), false));
        assert!(clipboard::caught().is_empty(), "nothing went to the clipboard");

        log.model.take(tail(
            vec![entry(1, "info", "one"), entry(2, "debug", "chatter"), entry(3, "warn", "three"), entry(4, "info", "four")],
            4,
        ));
        log.key(key(KeyCode::Char('y')), 5);
        assert_eq!(clipboard::caught(), ["09:00:01  one\n09:00:03  warn  three\n09:00:04  four"], "only lines passing the level");
        assert_eq!(note(&mut log), (t!("gui.admin.log.copied_all").to_string(), false));

        log.model.highlight = Some((3, 1));
        log.act(LogAct::CopyLines);
        assert_eq!(clipboard::caught(), ["09:00:01  one\n09:00:03  warn  three"], "the highlight, the debug line inside it skipped");
        assert_eq!(note(&mut log), (t!("gui.admin.log.copied_highlight").to_string(), false));

        clipboard::catch(Copied::Terminal);
        log.copy();
        assert_eq!(note(&mut log), (t!("gui.admin.log.copied_terminal").to_string(), false));
        clipboard::catch(Copied::Failed);
        log.copy();
        assert_eq!(note(&mut log), (t!("gui.admin.log.copy_failed").to_string(), true));
        assert_eq!(clipboard::caught().len(), 2);
        clipboard::catch(Copied::Clipboard);
    }

    #[test]
    fn highlighted_rows_wear_the_selection_colours_and_the_region_covers_the_line_rows() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 5));
        log.model.highlight = Some((2, 3));
        let mut ui = Surface::new();
        // Spaced, in a 40-cell area from column 4: the header on row 1, a
        // blank row, the five lines on rows 3-7, empty rows 8-9, and the
        // failure on row 10.
        let area = Rect::new(4, 1, 40, 10);
        let look = Look { spaced: true, hint: None };
        log.model.fail(&ApiError::Network("down".into()));
        let buf = render(&mut log, &mut ui, (50, 12), area, &look);
        assert!(row(&buf, 4).contains("line 2") && row(&buf, 5).contains("line 3"), "{}", row(&buf, 4));
        for y in [4, 5] {
            for x in area.x..area.right() {
                assert_eq!((buf[(x, y)].bg, buf[(x, y)].fg), (th().accent, th().on_accent), "({x}, {y})");
            }
            assert_ne!(buf[(area.x - 1, y)].bg, th().accent, "the fill keeps to the log");
        }
        for y in [3, 6] {
            assert_ne!(buf[(area.x + 12, y)].bg, th().accent, "row {y} is not highlighted");
            assert_eq!(buf[(area.x, y)].fg, th().dim, "its time stays dim");
        }
        assert_eq!(buf[(area.x, 10)].fg, th().gold, "the failure's row: {}", row(&buf, 10));

        for y in [1, 2, 10] {
            assert_eq!(ui.arm_region(Position::new(10, y)), None, "row {y} is no line row");
        }
        for y in [3, 7, 9] {
            assert_eq!(ui.arm_region(Position::new(10, y)), Some(LogAct::Select(Grip::Press, Position::new(10, y))), "row {y}");
            ui.release();
        }
        assert_eq!(ui.arm_region(Position::new(area.right(), 5)), None, "not past the log's edge");

        // No lines, no region.
        let mut empty = LogUi::new(None);
        let mut ui = Surface::new();
        render(&mut empty, &mut ui, (50, 12), area, &look);
        assert_eq!(ui.arm_region(Position::new(10, 3)), None);
    }

    #[test]
    fn the_header_offers_copy_and_download_whole_or_not_at_all() {
        let _en = english();
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 3));
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (70, 5), Rect::new(0, 0, 70, 5), &plain());
        assert_eq!(row(&buf, 0).trim_end(), "• following · info ▾ · copy · download");
        let copy = find(&buf, 0, "copy").expect("copy");
        let download = find(&buf, 0, "download").expect("download");
        assert_eq!(ui.hit(Position::new(copy, 0)), Some(LogAct::CopyLines));
        assert_eq!(ui.hit(Position::new(download + 7, 0)), Some(LogAct::Download));
        assert_eq!(ui.hit(Position::new(copy - 2, 0)), None, "the dot between is no control");
        assert_eq!(buf[(copy, 0)].fg, th().dim);
        ui.pointer = Some(Position::new(copy + 1, 0));
        let buf = render(&mut log, &mut ui, (70, 5), Rect::new(0, 0, 70, 5), &plain());
        assert_eq!(buf[(copy, 0)].fg, th().bright);
        assert!(buf[(copy, 0)].modifier.contains(Modifier::BOLD));
        ui.pointer = None;

        // The docked column at 160 columns is 38 cells: at the debug level
        // the download no longer fits, and is left out rather than cut.
        log.model.set_level(Level::Debug);
        let narrow = Rect::new(0, 0, 38, 5);
        let buf = render(&mut log, &mut ui, (38, 5), narrow, &plain());
        assert_eq!(row(&buf, 0).trim_end(), "• following · debug ▾ · copy");
        assert!(!ui.clicks.iter().any(|(_, act)| *act == LogAct::Download));
        log.model.set_level(Level::Info);
        let buf = render(&mut log, &mut ui, (38, 5), narrow, &plain());
        assert_eq!(row(&buf, 0), "• following · info ▾ · copy · download", "at info it fits exactly");

        // While a download runs, the busy word in the accent, nothing to press.
        log.downloading = true;
        let buf = render(&mut log, &mut ui, (70, 5), Rect::new(0, 0, 70, 5), &plain());
        let busy = find(&buf, 0, "downloading…").expect("the busy word");
        assert_eq!(buf[(busy, 0)].fg, th().accent);
        assert_eq!(ui.hit(Position::new(busy + 2, 0)), None);
        log.downloading = false;

        // The hint gives way before the controls do.
        let look = Look { spaced: false, hint: Some(t!("gui.admin.log.hint_undock").to_string()) };
        let buf = render(&mut log, &mut ui, (49, 5), Rect::new(0, 0, 49, 5), &look);
        assert!(row(&buf, 0).ends_with("· download  L undocks"), "{}", row(&buf, 0));
        let buf = render(&mut log, &mut ui, (48, 5), Rect::new(0, 0, 48, 5), &look);
        assert_eq!(row(&buf, 0).trim_end(), "• following · info ▾ · copy · download");
    }

    #[test]
    fn d_saves_a_canned_servers_zip_and_o_shows_it() {
        let _en = english();
        let dir = Scratch::new("zip");
        let (server, seen) = canned(200, zip_bytes());
        let mut log = downloading_log(&server, &dir);

        log.key(key(KeyCode::Char('o')), 5);
        assert_eq!(log.take_note(), Some((t!("gui.admin.log.nothing_saved").to_string(), false)));

        assert_eq!(log.key(key(KeyCode::Char('d')), 5), LogKey::Taken);
        assert!(log.downloading());
        assert_eq!(log.take_note(), Some((t!("gui.admin.log.fetching").to_string(), false)));
        let (note, failed) = landed(&mut log);
        assert!(!log.downloading());
        let files = dir.files();
        assert_eq!(files.len(), 1, "{files:?}");
        let path = &files[0];
        assert_eq!(std::fs::read(path).unwrap(), zip_bytes(), "the server's bytes, untouched");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let stamp = name.strip_prefix("mstream-logs-127.0.0.1-").and_then(|n| n.strip_suffix(".zip")).expect(&name);
        let (date, time) = stamp.split_once('-').expect(&name);
        assert!(date.len() == 8 && time.len() == 6 && stamp.chars().all(|c| c.is_ascii_digit() || c == '-'), "{name}");
        let home = process_var("HOME").map(PathBuf::from);
        let shown = log_file::shown_path(path, home.as_deref(), Os::HERE);
        assert_eq!((note, failed), (t!("gui.admin.log.saved", path = shown).to_string(), false));
        let requests: Vec<String> = seen.try_iter().collect();
        assert!(requests.iter().any(|r| r.starts_with("GET /api/v1/admin/logs/download ")), "{requests:?}");

        // o shows it: under test nothing launches, and the note names the
        // whole path instead.
        log.key(key(KeyCode::Char('o')), 5);
        assert_eq!(landed(&mut log), (t!("gui.admin.log.file_at", path = path.display().to_string()).to_string(), false));
        log.stop();
    }

    #[test]
    fn d_on_a_server_without_the_route_saves_the_shown_lines() {
        let _en = english();
        for (status, body, word) in [
            (404, br#"{"error":"Not Found"}"#.to_vec(), "gui.admin.log.saved_lines"),
            (200, empty_zip(), "gui.admin.log.saved_no_files"),
        ] {
            let dir = Scratch::new(&format!("lines-{status}"));
            let (server, _) = canned(status, body);
            let mut log = downloading_log(&server, &dir);
            log.model.take(tail(vec![entry(1, "info", "one"), entry(2, "debug", "chatter"), entry(3, "error", "three")], 3));
            log.model.highlight = Some((3, 3));
            log.download();
            let (note, failed) = landed(&mut log);
            let files = dir.files();
            assert_eq!(files.len(), 1, "{status}: {files:?}");
            assert_eq!(files[0].extension().unwrap(), "txt");
            let text = std::fs::read_to_string(&files[0]).unwrap();
            assert_eq!(text, "09:00:01  one\n09:00:03  error  three\n", "{status}: every line shown, whatever the highlight");
            let home = process_var("HOME").map(PathBuf::from);
            let shown = log_file::shown_path(&files[0], home.as_deref(), Os::HERE);
            let want = match word {
                "gui.admin.log.saved_lines" => t!("gui.admin.log.saved_lines", path = shown),
                _ => t!("gui.admin.log.saved_no_files", path = shown),
            };
            assert_eq!((note, failed), (want.to_string(), false), "{status}");
            log.stop();
        }

        // No route and no lines: nothing to save, and nothing saved.
        let dir = Scratch::new("lines-none");
        let (server, _) = canned(404, b"{}".to_vec());
        let mut log = downloading_log(&server, &dir);
        log.download();
        assert_eq!(landed(&mut log), (t!("gui.admin.log.save_nothing").to_string(), true));
        assert!(dir.files().is_empty());
        log.stop();
    }

    #[test]
    fn a_refused_download_says_the_hubs_sentence() {
        let _en = english();
        let dir = Scratch::new("refused");
        let (server, _) = canned(403, br#"{"error":"Admin access required"}"#.to_vec());
        let mut log = downloading_log(&server, &dir);
        log.model.take(infos(1, 3));
        log.download();
        let forbidden = ApiError::Forbidden("Admin access required".into());
        assert_eq!(landed(&mut log), (gate_message(&forbidden, &t!("gui.admin.log.download_failed")), true));
        assert!(dir.files().is_empty(), "nothing saved, not even the lines");
        assert!(!log.downloading(), "and the next d may go");

        // A zip cut short is said so, and never saved.
        let (server, _) = canned(200, zip_bytes()[..30].to_vec());
        let mut log = downloading_log(&server, &dir);
        log.model.take(infos(1, 3));
        log.download();
        assert_eq!(landed(&mut log), (t!("gui.admin.log.zip_cut").to_string(), true));
        assert!(dir.files().is_empty());
        log.stop();
    }

    #[test]
    fn one_download_at_a_time() {
        let _en = english();
        let dir = Scratch::new("once");
        let (server, seen) = canned(200, zip_bytes());
        let mut log = downloading_log(&server, &dir);
        log.key(key(KeyCode::Char('d')), 5);
        log.key(key(KeyCode::Char('d')), 5);
        log.act(LogAct::Download);
        landed(&mut log);
        let downloads = seen.try_iter().filter(|r| r.contains("/logs/download")).count();
        assert_eq!(downloads, 1);
        assert_eq!(dir.files().len(), 1);

        // Without a session there is nothing to ask.
        let mut lone = LogUi::new(None);
        lone.download();
        assert!(!lone.downloading());
        assert_eq!(lone.take_note(), None);
        log.stop();
    }
}
