//! The server's log, read as it grows: the Admin tab's log tail (docs/
//! ux-contracts/admin-screen.md, clauses 21-26; docs/ui-kit.md, "Log
//! tail"). The model holds the newest thousand lines of mStream's main log
//! ring, delta-polled by sequence number from `GET api/v1/admin/logs/
//! recent`, and knows whether the view follows the newest line or stands
//! paused where the reader scrolled it. It shows every line the ring
//! holds, whatever its level (http, verbose, debug and silly included);
//! the level only colours a line.
//!
//! The poll runs on a thread of the log's own rather than on the App's api
//! worker or a room's: a slow answer from a busy server must never hold a
//! room's request behind it, nor the music's (PLAN.md risk 63). One request
//! is in flight at most; the next is due two seconds after the last answer,
//! ten after a failure, and never again after a 401, 403 or 405 until the
//! tab is opened again. The screen pumps it once a frame and the pump only
//! ever `try_recv`s, so a frame never waits on the network.
//!
//! A line is drawn whole: its message wraps by words under the clock, its
//! later rows set in by the clock's width, so nothing of it is cut at the
//! log's edge ([`message_rows`]). The view still scrolls by lines, the
//! newest at the bottom; only a line taller than the view is read a row
//! at a time, and the top of the log shows the oldest line from its
//! clock, so every row of every line can be brought into sight.
//!
//! The kit's scroll bar stands on the log's right edge while the lines do
//! not all fit, and counts lines, not rows: wrapping all thousand lines
//! of up to four thousand characters every frame to count their rows
//! would cost more than the bar is worth, so its thumb follows the bottom
//! line's place among the lines and its size the lines the view shows
//! whole ([`draw`]).
//!
//! Drawing takes any surface and a wrapper for the log's own actions, so
//! the host decides what a click on the paused word, a header control or
//! the bar becomes in its action type.
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
use crate::admin::{gate_message, iso_unix};
use crate::api::types::LogTail;
use crate::api::{ApiError, Client};
use crate::kit::clipboard::{self, Copied};
use crate::kit::os::{Os, process_var};
use crate::kit::theme::th;
use crate::kit::{Grip, Surface, dim, grapheme_cells, scroll_items, width};

/// How many lines the player keeps; older ones fall off the front.
pub(crate) const RING: usize = 1000;
/// The pause between an answer and the next request.
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(2);
/// The pause after a failed request, so a server that is down is not
/// asked thirty times a minute.
pub(crate) const POLL_FAILING: Duration = Duration::from_secs(10);
/// The longest message kept: what mStream's own logger cuts a message
/// to, so nothing the server holds is lost. The rows show all of it, and
/// a copy takes all of it.
const MESSAGE_CHARS: usize = 4000;
/// The width of `HH:MM:SS` and its gap, which sets a line's later rows
/// under its first: a copy puts it before a message's later lines, so a
/// stack trace reads under its time stamp, and the view starts every row
/// of a line after its first one past it.
const LATER_LINES: &str = "          ";

/// A line's level, as far as its colour goes. The log shows every line
/// whatever its level; the level picks the colour its rows read in, and
/// the word a copy writes before an error or a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Error,
    Warn,
    /// Everything else, in the text colour.
    Other,
}

impl Level {
    /// Winston's seven levels as the colours read them: `error`, `warn`,
    /// and the rest — `info`, `http`, `verbose`, `debug`, `silly`, and a
    /// level the player has never heard of — alike.
    pub(crate) fn of(raw: &str) -> Level {
        match raw.trim().to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" | "warning" => Level::Warn,
            _ => Level::Other,
        }
    }
}

/// One log line as the player keeps it: its time, its level, and the
/// whole message made printable, which the rows wrap and a copy takes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub seq: u64,
    /// Unix seconds, when the server's time parsed.
    pub at: Option<i64>,
    pub level: Level,
    /// Every line of the message (see [`whole_message`]).
    pub whole: String,
}

impl Line {
    fn from_entry(entry: &crate::api::types::ActivityEntry) -> Line {
        let whole = whole_message(&entry.message);
        Line { seq: entry.seq, at: iso_unix(&entry.t), level: Level::of(&entry.level), whole }
    }
}

/// A message as the rows and a copy carry it: every line, each made
/// printable (no character that acts on a terminal or reorders a reader,
/// a tab as four spaces) and trimmed at its end only, so a stack trace
/// keeps its indentation. Blank lines at either end go, so a message that
/// opens on a newline still starts with what it is about; the whole is
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

/// A message as the rows it takes in `cells` cells, the clock's column
/// left out: each of its lines wrapped by [`wrap_line`], a blank line
/// between two kept as an empty row, and an empty message one empty row,
/// so every line has its clock's row. A log with no cells past the clock
/// gives each line that one row, since none of a message would show.
fn message_rows(whole: &str, cells: usize) -> Vec<String> {
    if cells == 0 {
        return vec![String::new()];
    }
    whole.split('\n').flat_map(|part| wrap_line(part, cells)).collect()
}

/// One line of a message wrapped at `cells` cells: greedy by words, as
/// the kit's [`crate::kit::wrap_words`] wraps a sentence, and measured as
/// it is, in the cells ratatui draws. It does two things a sentence's wrap
/// does not. The line's own indentation stays on its first row (a stack
/// trace's `    at`) wherever its first word fits beside it, and the
/// spaces inside a row stay as written. And a word wider than a whole row
/// (a path, a token, a run of CJK with no spaces) starts where it stands
/// and breaks at the last cell of every row, a grapheme at a time, where a
/// sentence's wrap would leave it whole to run off the log's edge. A blank
/// line is one empty row.
fn wrap_line(text: &str, cells: usize) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    let mut rest = text;
    let mut first = true;
    loop {
        let from = rest.trim_start_matches(' ');
        let gap = rest.len() - from.len();
        let (word, after) = from.split_at(from.find(' ').unwrap_or(from.len()));
        rest = after;
        if word.is_empty() {
            break;
        }
        let w = width(word);
        // The spaces before a word stay inside a row, and before the
        // line's first word as its indentation; a break swallows them.
        let lead = if first || !row.is_empty() { gap } else { 0 };
        first = false;
        if used + lead + w <= cells {
            row.push_str(&" ".repeat(lead));
            row.push_str(word);
            used += lead + w;
            continue;
        }
        if w <= cells {
            if !row.is_empty() {
                rows.push(std::mem::take(&mut row));
            }
            row.push_str(word);
            used = w;
            continue;
        }
        // Wider than any row: on from here, cut at every row's last cell.
        let head = word.graphemes(true).next().map_or(0, grapheme_cells);
        if used + lead + head <= cells {
            row.push_str(&" ".repeat(lead));
            used += lead;
        } else if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        for grapheme in word.graphemes(true) {
            let gw = grapheme_cells(grapheme);
            if !row.is_empty() && used + gw > cells {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push_str(grapheme);
            used += gw;
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// What the view holds, for scrolling: its rows, and the cells a message
/// row takes (the log's width less the clock's column) — or none before a
/// frame has drawn the log, when every line counts as one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fit {
    rows: usize,
    cells: Option<usize>,
}

impl Fit {
    /// The rows `line` takes.
    fn height(self, line: &Line) -> usize {
        self.cells.map_or(1, |cells| message_rows(&line.whole, cells).len())
    }

    /// The view's rows, never fewer than one, so a log with no room for
    /// a line still scrolls by lines.
    fn rows(self) -> usize {
        self.rows.max(1)
    }
}

/// A move of the view: a line, or a page of rows, older or newer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Older,
    Newer,
    PageOlder,
    PageNewer,
}

/// The rows from the oldest shown line's first one down to the view's
/// bottom row, with line `bottom` at the bottom and `under` of its rows
/// below the view, counted up to `cap` and no further. The view is full
/// when they reach its rows, and stands at the top of the log when they
/// are no more than its rows.
fn depth(shown: &[&Line], bottom: usize, under: usize, fit: Fit, cap: usize) -> usize {
    let mut n = fit.height(shown[bottom]).saturating_sub(under);
    for line in shown[..bottom].iter().rev() {
        if n >= cap {
            break;
        }
        n += fit.height(line);
    }
    n
}

/// The top of the log, as `(scroll, under)`: the oldest line's first row
/// at the view's top, and the bottom wherever the rows run out — the one
/// place the bottom line may be cut short, so the oldest lines are never
/// out of reach above a line that overshoots the view. Where every line
/// fits, that is following.
fn top_of(shown: &[&Line], fit: Fit) -> (usize, usize) {
    let rows = fit.rows();
    let mut filled = 0;
    for (i, line) in shown.iter().enumerate() {
        filled += fit.height(line);
        if filled >= rows {
            return (shown.len() - 1 - i, filled - rows);
        }
    }
    (0, 0)
}

/// `(scroll, under)` kept where the view is full and its bottom line
/// whole: the bottom line no older than the oldest, its rows under the
/// view fewer than it has; the top of the log when the view would not be
/// full (or following, when every line fits); and a line cut at the
/// bottom only at the top of the log, or while a line taller than the
/// view is read — and then never so far that its first row drops below
/// the view's top. A resize goes through here, so the bottom line stays
/// the bottom line however the lines re-flow.
fn settled(shown: &[&Line], scroll: usize, under: usize, fit: Fit) -> (usize, usize) {
    let Some(newest) = shown.len().checked_sub(1) else { return (0, 0) };
    let rows = fit.rows();
    let scroll = scroll.min(newest);
    let bottom = newest - scroll;
    let tall = fit.height(shown[bottom]);
    let under = under.min(tall - 1);
    let depth = depth(shown, bottom, under, fit, rows + 1);
    if depth < rows {
        return top_of(shown, fit);
    }
    if depth == rows || under == 0 {
        return (scroll, under);
    }
    (scroll, if tall > rows { under.min(tall - rows) } else { 0 })
}

/// `(scroll, under)` after `step` (see [`LogModel::go`]).
fn stepped(shown: &[&Line], scroll: usize, under: usize, step: Step, fit: Fit) -> (usize, usize) {
    let rows = fit.rows();
    let Some(bottom) = shown.len().checked_sub(1 + scroll) else { return (scroll, under) };
    let tall = fit.height(shown[bottom]);
    // The bottom line's rows from its first down to the view's bottom.
    let left = tall - under;
    let page = matches!(step, Step::PageOlder | Step::PageNewer);
    let by = if page { rows } else { 1 };
    match step {
        Step::Older | Step::PageOlder => {
            if depth(shown, bottom, under, fit, rows + 1) <= rows {
                return (scroll, under);
            }
            if left > rows {
                return (scroll, (under + by).min(tall - rows));
            }
            // Past the lines a page shows whole, from the bottom one up.
            let mut n = 1;
            if page {
                let mut used = left;
                for line in shown[..bottom].iter().rev() {
                    used += fit.height(line);
                    if used > rows {
                        break;
                    }
                    n += 1;
                }
            }
            (scroll + n, 0)
        }
        Step::Newer | Step::PageNewer => {
            if under > 0 && tall > rows {
                // Lines above it in view: its first row goes to the top.
                if left < rows {
                    return (scroll, tall - rows);
                }
                return (scroll, under.saturating_sub(by));
            }
            if under > 0 && !page {
                return (scroll, 0);
            }
            if scroll == 0 {
                return (0, 0);
            }
            // The lines below that fit a page, at least one. The rows of
            // the bottom line still under the view (at the top of the
            // log) count in that page, so none of them is skipped; when
            // they leave room for no line below, the page shows that line
            // whole first.
            let mut n = 1;
            if page {
                let mut used = under;
                n = 0;
                for line in &shown[bottom + 1..] {
                    used += fit.height(line);
                    if used > rows {
                        break;
                    }
                    n += 1;
                }
                if n == 0 && under > 0 {
                    return (scroll, 0);
                }
                n = n.max(1);
            }
            // A line taller than the view comes in from its first row.
            (scroll - n, fit.height(shown[bottom + n]).saturating_sub(rows))
        }
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
    /// How many shown lines the view's bottom line stands above the
    /// newest; 0 follows, with `under` 0.
    pub scroll: usize,
    /// How many rows of the bottom line are below the view's bottom edge:
    /// 0 but while a line taller than the view is read, and at the top of
    /// the log (see [`settled`]).
    under: usize,
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
            under: 0,
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
    /// arrived under it, so what the reader is looking at stays put — a
    /// tall line read a row at a time included. A highlight goes with a
    /// restarted ring, and once the ring has dropped every line of it; the
    /// view keeps following under one.
    pub(crate) fn take(&mut self, tail: LogTail) {
        if self.answered && tail.last_seq < self.cursor {
            self.lines.clear();
            self.seen = 0;
            self.follow();
            self.highlight = None;
        }
        let newest = self.lines.back().map(|l| l.seq);
        let before = self.lines.len();
        let fresh = tail.entries.iter().filter(|e| newest.is_none_or(|n| e.seq > n));
        self.lines.extend(fresh.map(Line::from_entry));
        let arrived = self.lines.len() - before;
        while self.lines.len() > RING {
            self.lines.pop_front();
        }
        if let Some((anchor, head)) = self.highlight
            && self.lines.front().is_none_or(|front| anchor.max(head) < front.seq)
        {
            self.highlight = None;
        }
        if !self.following() {
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

    /// The lines shown, oldest first: every one held, whatever its level.
    pub(crate) fn shown(&self) -> Vec<&Line> {
        self.lines.iter().collect()
    }

    /// Whether the view rides the newest line, the whole of it in view.
    /// Derived from the offsets, so the header's word and the view can
    /// never disagree.
    pub(crate) fn following(&self) -> bool {
        self.scroll == 0 && self.under == 0
    }

    /// Ride the newest line again.
    pub(crate) fn follow(&mut self) {
        self.scroll = 0;
        self.under = 0;
    }

    /// Move the view. Up and the wheel's notch go a line older: the line
    /// above the bottom one becomes the bottom, whole. A page goes past
    /// the lines the view shows whole, so the line cut at its top becomes
    /// the bottom; down, past as many lines as fit a page below the rows
    /// a line cut at the bottom still hides, never fewer than one, unless
    /// those rows leave no room, when the page shows that line whole. A
    /// line taller than the view is the exception: it goes by
    /// rows (one, or a page) until its first row, or its last, is in
    /// sight, and a step down onto one brings it in from its first row.
    /// The top of the log goes no further, and reaching the newest line's
    /// last row follows again.
    fn go(&mut self, step: Step, fit: Fit) {
        self.settle(fit);
        (self.scroll, self.under) = stepped(&self.shown(), self.scroll, self.under, step, fit);
        self.settle(fit);
    }

    /// The top of the log (Home).
    fn top(&mut self, fit: Fit) {
        (self.scroll, self.under) = top_of(&self.shown(), fit);
    }

    /// Keep the view full and its bottom line whole in `fit` (see
    /// [`settled`]).
    fn settle(&mut self, fit: Fit) {
        (self.scroll, self.under) = settled(&self.shown(), self.scroll, self.under, fit);
    }

    /// The bar's track pressed, or its thumb dragged, at position `at` of
    /// the `of` past its first (the kit's [`crate::kit::bar_jump`], counted
    /// in lines, see [`draw`]): the track's top is the top of the log and
    /// its bottom follows; between, the bottom line stands `of - at` lines
    /// above the newest, whole, which puts the oldest line the view shows
    /// whole at about `at`.
    fn jump(&mut self, at: usize, of: usize, fit: Fit) {
        if at == 0 {
            self.top(fit);
        } else if at >= of {
            self.follow();
        } else {
            (self.scroll, self.under) = (of - at, 0);
        }
        self.settle(fit);
    }

    /// Whether the line numbered `seq` is in the highlight.
    fn lit(&self, seq: u64) -> bool {
        self.highlight.is_some_and(|(a, h)| (a.min(h)..=a.max(h)).contains(&seq))
    }

    /// The lines in the highlight, oldest first.
    pub(crate) fn highlighted(&self) -> Vec<&Line> {
        self.lines.iter().filter(|l| self.lit(l.seq)).collect()
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

    /// Lines that arrived since the log was last on screen, whatever their
    /// level.
    pub(crate) fn unseen(&self) -> usize {
        self.lines.iter().filter(|l| l.seq > self.seen).count()
    }

    /// The log is on screen: what it holds has been seen.
    pub(crate) fn mark_seen(&mut self) {
        self.seen = self.lines.back().map_or(self.cursor, |l| l.seq.max(self.cursor));
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
        Level::Other => {}
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
    /// The paused word.
    Follow,
    /// The bar's endcaps, pressed or held: a line older or newer, as ↑
    /// and ↓ go.
    Older,
    Newer,
    /// The bar's track pressed, or its thumb dragged, at position `at` of
    /// the `of` past its first (see [`LogModel::jump`]).
    Jump { at: usize, of: usize },
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

/// A note the log leaves for the screen.
enum Note {
    /// Words alone, and whether they tell of a failure.
    Words(String, bool),
    /// Words that name a file: the key, whose `%{path}` comes last in every
    /// locale, the path as the note writes it, and whether it tells of a
    /// failure. The path is fitted to the note's cells when the screen
    /// takes it ([`file_note`]).
    File(&'static str, String, bool),
}

impl Note {
    /// Words that tell how something went.
    fn said(text: impl ToString) -> Note {
        Note::Words(text.to_string(), false)
    }

    /// Words that tell of a failure, which the note shows in gold.
    fn failed(text: impl ToString) -> Note {
        Note::Words(text.to_string(), true)
    }
}

/// A note naming a file, in `cells`: the words whole, then the path
/// clipped LEADING to the cells left after them, so the file's own name
/// stays in sight however narrow the window (the kit's path law). The
/// whole path is what `o` shows.
fn file_note(key: &str, path: &str, cells: usize) -> String {
    let room = cells.saturating_sub(width(&t!(key, path = "")));
    let path = if room == 0 { String::new() } else { super::clip_lead(path, room) };
    t!(key, path = path).to_string()
}

/// The log as a screen holds it: the model, the poll thread, where the
/// last frame put it, and what the copy and the download need and say.
pub(crate) struct LogUi {
    pub model: LogModel,
    zone: Option<Zone>,
    worker: Option<Worker>,
    /// How many line rows the last frame had, and the cells a message row
    /// took in it (none until a frame has drawn the lines): what the keys
    /// and the wheel measure a step, a page and the scroll's end in.
    rows: usize,
    cells: Option<usize>,
    /// The session's client, shared with the poll thread, for a download.
    client: Option<Arc<Client>>,
    /// The server's part of a saved file's name.
    label: String,
    /// A press on the lines, until its release or until the log leaves
    /// the screen.
    hold: Option<Hold>,
    /// The line rows the last frame drew (the rows under the lines too)
    /// and the sequence number of the line on each drawn row, top first,
    /// a wrapped line's on every row it took.
    rows_at: Option<(Rect, Vec<u64>)>,
    /// The one-shot threads' channel. It outlives a stop, so a download
    /// still running when the tab is left lands on the next visit.
    side: (Sender<Side>, Receiver<Side>),
    downloading: bool,
    /// The file the last download saved, for `o`.
    saved: Option<PathBuf>,
    /// What the last copy, download or show said, until the screen takes
    /// it.
    note: Option<Note>,
    /// Where a download saves instead of the Downloads folder: a test's
    /// own temporary folder.
    save_dir: Option<PathBuf>,
}

impl LogUi {
    pub(crate) fn new(zone: Option<Zone>) -> Self {
        LogUi {
            model: LogModel::new(),
            zone,
            worker: None,
            rows: 0,
            cells: None,
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
    /// answer ends the download either way. A saved file is named from the
    /// home folder (`~/`); a file shown, or not, by its whole path, which
    /// is all a machine with no desktop gets.
    fn land(&mut self, answer: Side) {
        let full = |path: &PathBuf| path.display().to_string();
        let note = match answer {
            Side::Saved(saved) => {
                self.downloading = false;
                let home = process_var("HOME").map(PathBuf::from);
                let shown = |path: &PathBuf| log_file::shown_path(path, home.as_deref(), Os::HERE);
                match saved {
                    Ok(Saved::Zip(path)) => {
                        let note = Note::File("gui.admin.log.saved", shown(&path), false);
                        self.saved = Some(path);
                        note
                    }
                    Ok(Saved::Text { path, why }) => {
                        let key = match why {
                            Fallback::NoRoute => "gui.admin.log.saved_lines",
                            Fallback::NoFiles => "gui.admin.log.saved_no_files",
                        };
                        let note = Note::File(key, shown(&path), false);
                        self.saved = Some(path);
                        note
                    }
                    Err(SaveError::Api(e)) => {
                        Note::failed(gate_message(&e, &t!("gui.admin.log.download_failed")))
                    }
                    Err(SaveError::Cut) => Note::failed(t!("gui.admin.log.zip_cut")),
                    Err(SaveError::Nothing) => Note::failed(t!("gui.admin.log.save_nothing")),
                    Err(SaveError::NoFolder) => Note::failed(t!("gui.admin.log.no_folder")),
                    Err(SaveError::Taken(name)) => Note::failed(t!("gui.admin.log.names_taken", name = name)),
                    Err(SaveError::Io(err)) => Note::failed(t!("gui.admin.log.save_failed", err = err)),
                }
            }
            // The file manager came up: it says enough.
            Side::Shown(_, Ok(HandOff::Launched)) => return,
            Side::Shown(path, Ok(HandOff::Headless(_))) => Note::File("gui.admin.log.file_at", full(&path), false),
            Side::Shown(path, Ok(HandOff::Nothing) | Err(_)) => {
                Note::File("gui.admin.log.show_failed", full(&path), true)
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
            self.note = Some(Note::said(t!("gui.admin.log.copy_nothing")));
            return;
        }
        let text = as_text(&lines, self.zone.as_ref());
        self.note = Some(match clipboard::copy(&text) {
            Copied::Clipboard if lit => Note::said(t!("gui.admin.log.copied_highlight")),
            Copied::Clipboard => Note::said(t!("gui.admin.log.copied_all")),
            Copied::Terminal => Note::said(t!("gui.admin.log.copied_terminal")),
            Copied::Failed => Note::failed(t!("gui.admin.log.copy_failed")),
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
                Note::said(t!("gui.admin.log.fetching"))
            }
            Err(e) => Note::failed(t!("gui.admin.log.save_failed", err = e.to_string())),
        });
    }

    /// `o`: the file the last download saved, shown in the file manager on
    /// a thread, since the opener is watched for a moment.
    pub(crate) fn show_saved(&mut self) {
        let Some(path) = self.saved.clone() else {
            self.note = Some(Note::said(t!("gui.admin.log.nothing_saved")));
            return;
        };
        let side = self.side.0.clone();
        let shown = path.clone();
        let spawned = std::thread::Builder::new().name("server log show".into()).spawn(move || {
            let answer = opener::reveal(&shown);
            let _ = side.send(Side::Shown(shown, answer));
        });
        if spawned.is_err() {
            self.note = Some(Note::File("gui.admin.log.show_failed", path.display().to_string(), true));
        }
    }

    /// Whether a download is in flight: the header's busy word.
    pub(crate) fn downloading(&self) -> bool {
        self.downloading
    }

    /// The note the last copy, download or show left, once, as words and
    /// whether they tell of a failure, for a note `cells` wide.
    pub(crate) fn take_note(&mut self, cells: usize) -> Option<(String, bool)> {
        Some(match self.note.take()? {
            Note::Words(text, failed) => (text, failed),
            Note::File(key, path, failed) => (file_note(key, &path, cells), failed),
        })
    }

    /// The log is not on screen: a press held on it lets go, and the rows
    /// it was drawn on no longer stand anywhere. The highlight stays.
    pub(crate) fn let_go(&mut self) {
        self.hold = None;
        self.rows_at = None;
    }

    /// The line on screen row `y` of the last frame — any of a wrapped
    /// line's rows means that line — clamped to the first and the last
    /// row drawn, so a drag past either end stops at their lines.
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

    /// The view as the last frame drew it: its rows and its cells. Before
    /// a frame has drawn the lines, `rows` (what the host gives them) with
    /// every line one row.
    fn fit(&self, rows: usize) -> Fit {
        match self.cells {
            Some(cells) => Fit { rows: self.rows, cells: Some(cells) },
            None => Fit { rows, cells: None },
        }
    }

    /// A key while the log has the focus; `rows` is how many rows the host
    /// gives its lines, which counts until a frame has drawn them (see
    /// [`LogUi::fit`]). Every key is the log's except the two that leave
    /// it, and does nothing where it is not named here (Enter among them);
    /// Esc clears a highlight before it leaves.
    pub(crate) fn key(&mut self, key: KeyEvent, rows: usize) -> LogKey {
        let fit = self.fit(rows);
        match key.code {
            KeyCode::Esc if self.model.highlight.is_some() => self.model.clear_highlight(),
            KeyCode::Esc | KeyCode::Char('q') => return LogKey::Leave,
            KeyCode::Up => self.model.go(Step::Older, fit),
            KeyCode::Down => self.model.go(Step::Newer, fit),
            KeyCode::PageUp => self.model.go(Step::PageOlder, fit),
            KeyCode::PageDown => self.model.go(Step::PageNewer, fit),
            KeyCode::Home => self.model.top(fit),
            KeyCode::End | KeyCode::Char('f') => self.model.follow(),
            KeyCode::Char('y') => self.copy(),
            KeyCode::Char('d') => self.download(),
            KeyCode::Char('o') => self.show_saved(),
            _ => {}
        }
        self.model.settle(fit);
        LogKey::Taken
    }

    /// One notch of the wheel over the log: a step as ↑ or ↓ takes. Up
    /// reads older lines, which pauses it; down comes back, and following
    /// resumes at the bottom. Before a frame has drawn the lines it only
    /// keeps to them.
    pub(crate) fn wheel(&mut self, up: bool) {
        self.model.go(if up { Step::Older } else { Step::Newer }, self.fit(1));
    }

    pub(crate) fn act(&mut self, act: LogAct) {
        match act {
            LogAct::Follow => self.model.follow(),
            LogAct::Older => self.wheel(true),
            LogAct::Newer => self.wheel(false),
            LogAct::Jump { at, of } => {
                let fit = self.fit(1);
                self.model.jump(at, of, fit);
            }
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

/// One row of the view: the line it belongs to, whether it is that
/// line's first (the one with the clock), and its part of the message.
struct Drawn {
    seq: u64,
    at: Option<i64>,
    level: Level,
    first: bool,
    text: String,
}

/// The rows a view of `rows` rows shows, top first: the bottom line
/// `scroll` lines above the newest, `under` of its rows left below the
/// view, and the lines above it filling upward, the top one showing its
/// last rows when only they fit. Only the lines in view are wrapped.
fn view(shown: &[&Line], scroll: usize, under: usize, rows: usize, cells: usize) -> Vec<Drawn> {
    let mut drawn = Vec::new();
    let Some(bottom) = shown.len().checked_sub(1 + scroll) else { return drawn };
    for (i, line) in shown[..=bottom].iter().rev().enumerate() {
        let parts = message_rows(&line.whole, cells);
        let skip = if i == 0 { under } else { 0 };
        for (n, text) in parts.into_iter().enumerate().rev().skip(skip) {
            if drawn.len() == rows {
                break;
            }
            drawn.push(Drawn { seq: line.seq, at: line.at, level: line.level, first: n == 0, text });
        }
        if drawn.len() == rows {
            break;
        }
    }
    drawn.reverse();
    drawn
}

/// Whether `shown` needs the scroll bar in a view of `rows` rows whose
/// message rows have `cells` cells: two lines or more that do not all
/// fit whole, and rows enough for the bar's two arrows and a cell of
/// track between them. Only the newest lines are wrapped to find out, up
/// to a row past the view, never the whole ring.
fn needs_bar(shown: &[&Line], rows: usize, cells: usize) -> bool {
    let fit = Fit { rows, cells: Some(cells) };
    shown.len() >= 2 && rows >= 3 && depth(shown, shown.len() - 1, 0, fit, rows + 1) > rows
}

/// The log in `area`: the header on its first row, then the lines, the
/// newest at the bottom, each wrapped whole under its clock — or the one
/// sentence when there are none to show. A failure while lines are held
/// takes the last row, so a log that stopped never passes for one that
/// is following. While the lines do not all fit, the kit's scroll bar
/// stands on the right edge of their rows and they wrap a cell narrower
/// beside it. The line rows, and the empty rows under them, are a drag
/// region for the highlight, the bar's column left out; every row of a
/// highlighted line wears the selection colours across the lines' width.
pub(crate) fn draw<A: Clone + 'static>(
    frame: &mut Frame,
    ui: &mut Surface<A>,
    log: &mut LogUi,
    area: Rect,
    look: &Look,
    wrap: fn(LogAct) -> A,
) {
    log.rows = 0;
    log.cells = None;
    log.rows_at = None;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let right = area.right();
    let y = area.y;

    // The rows the lines get, whether they need the bar (decided at the
    // whole width, so the bar never comes and goes with the cell it takes
    // itself), the cells a message row gets past the clock's column, and
    // the view settled in them before the header says whether it follows:
    // a paused view whose lines all fit is following, and says so on this
    // frame, not the next. A new width re-flows the lines under the same
    // bottom line.
    let top = y + if look.spaced { 2 } else { 1 };
    let below = area.bottom().saturating_sub(top) as usize;
    let held = log.model.shown().len();
    let error_row = log.model.error.is_some() && held > 0 && below > 0;
    let rows = below - usize::from(error_row);
    let whole_width = (area.width as usize).saturating_sub(LATER_LINES.len());
    let bar = needs_bar(&log.model.shown(), rows, whole_width);
    let lines_right = right - u16::from(bar);
    let cells = whole_width.saturating_sub(usize::from(bar));
    log.rows = rows;
    log.cells = Some(cells);
    log.model.settle(Fit { rows, cells: Some(cells) });

    // The header: the follow state, the copy and the download, the hint.
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
    // them and the empty rows under them (never the header, the failure's
    // row nor the bar's column), each row mapped to its line. A drag in
    // progress re-reads the line under the hand against these rows, so
    // the wheel, or lines arriving, under a still pointer move the
    // highlight with the lines.
    let drawn = view(&log.model.shown(), log.model.scroll, log.model.under, rows, cells);
    let seqs: Vec<u64> = drawn.iter().map(|row| row.seq).collect();
    let lines_w = lines_right - area.x;
    if !seqs.is_empty() {
        let lines = Rect { x: area.x, y: top, width: lines_w, height: rows as u16 };
        log.rows_at = Some((lines, seqs));
        if let Some(hold) = log.hold.filter(|h| h.moved)
            && let Some(head) = log.seq_at(hold.at.y)
        {
            log.model.highlight = Some((hold.anchor, head));
        }
        ui.drag_region(lines, move |grip, at| wrap(LogAct::Select(grip, at)));
    }

    // The rows, the newest line's last at the bottom of the window onto
    // them: a line's first row opens on its clock, its later ones under
    // the message's first cell, every one in the line's colour; a
    // highlighted line's rows in the selection colours from edge to edge.
    for (i, row) in drawn.iter().enumerate() {
        let ly = top + i as u16;
        let lit = log.model.lit(row.seq);
        if lit {
            put(frame, area.x, ly, &" ".repeat(lines_w as usize), sel());
        }
        let color = match row.level {
            Level::Error => th().danger,
            Level::Warn => th().gold,
            Level::Other => th().text,
        };
        let (time, words) = if lit { (sel(), sel()) } else { (dim(), Style::default().fg(color)) };
        let mut lx = area.x;
        if row.first {
            lx += span(frame, lx, ly, lines_right, &clock(row.at, log.zone.as_ref()), time);
            lx += span(frame, lx, ly, lines_right, "  ", if lit { sel() } else { Style::default() });
        } else {
            lx = lx.saturating_add(LATER_LINES.len() as u16);
        }
        span(frame, lx, ly, lines_right, &row.text, words);
    }

    // The bar, counted in lines: the thumb stands where the oldest line
    // shown from its clock stands among all the lines, and its length is
    // the share of them the view shows whole, so it is at the bottom while
    // following and at the top at the top of the log, even where the
    // bottom line is cut there. The view's lines run on up from the bottom
    // one and only the bottom one can be cut below the view, so the clocks
    // in view tell both. A line taller than the view, read a row at a
    // time, shows no clock and stands at its own place. The endcaps act as
    // ↑ and ↓ (with the kit's hold-repeat), the track and the thumb move
    // the bottom line, and the bar's cells are registered after the drag
    // region, so a press on them is the bar's whatever the region spans.
    if bar && let Some(bottom) = held.checked_sub(1 + log.model.scroll) {
        let clocks = drawn.iter().filter(|row| row.first).count();
        let first = (bottom + 1).saturating_sub(clocks.max(1));
        let whole = clocks.saturating_sub(usize::from(clocks > 0 && log.model.under > 0)).max(1);
        let of = held.saturating_sub(whole);
        let rect = Rect { x: lines_right, y: top, width: 1, height: rows as u16 };
        let (older, newer) = (wrap(LogAct::Older), wrap(LogAct::Newer));
        let jump = move |at| wrap(LogAct::Jump { at, of });
        scroll_items(frame, ui, rect, held, whole, first.min(of), older, newer, jump);
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
        model.shown().iter().map(|l| l.whole.clone()).collect()
    }

    fn id(act: LogAct) -> LogAct {
        act
    }

    /// Draw the log in `area` of a `w`×`h` screen, and hand back the
    /// buffer and the surface the frame registered.
    fn render(log: &mut LogUi, ui: &mut Surface<LogAct>, size: (u16, u16), area: Rect, look: &Look) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        ui.begin_frame();
        terminal.draw(|frame| draw(frame, ui, log, area, look, id)).unwrap();
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
    fn every_line_shows_whatever_its_level_and_the_level_only_colours_it() {
        assert_eq!(Level::of("error"), Level::Error);
        assert_eq!(Level::of("warn"), Level::Warn);
        assert_eq!(Level::of("WARNING"), Level::Warn);
        for other in ["info", "http", "verbose", "debug", "silly", "notice"] {
            assert_eq!(Level::of(other), Level::Other, "{other}");
        }

        let mut m = LogModel::new();
        m.take(tail(
            vec![
                entry(1, "error", "e"),
                entry(2, "warn", "w"),
                entry(3, "info", "i"),
                entry(4, "http", "h"),
                entry(5, "verbose", "v"),
                entry(6, "debug", "d"),
                entry(7, "silly", "s"),
            ],
            7,
        ));
        assert_eq!(texts(&m), ["e", "w", "i", "h", "v", "d", "s"], "the server's chatter included");
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

        // Three lines arrive (a debug line among them): the view holds on 18.
        log.model.take(tail(
            vec![entry(21, "info", "a"), entry(22, "debug", "b"), entry(23, "warn", "c")],
            23,
        ));
        assert_eq!(log.model.scroll, 5, "every line that arrived counts under the view");
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
    fn end_f_and_reaching_the_newest_line_follow_again() {
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

        assert_eq!(log.key(key(KeyCode::Esc), 5), LogKey::Leave);
        assert_eq!(log.key(key(KeyCode::Char('q')), 5), LogKey::Leave);
        assert_eq!(log.key(key(KeyCode::Char('7')), 5), LogKey::Taken, "every other key is the log's");
    }

    #[test]
    fn the_new_count_skips_what_was_there_at_the_first_answer_and_counts_every_line() {
        let mut m = LogModel::new();
        assert_eq!(m.unseen(), 0, "nothing before the first answer");
        m.take(infos(1, 10));
        assert_eq!(m.unseen(), 0, "what the ring held when the log opened is not new");
        m.take(tail(
            vec![entry(11, "info", "a"), entry(12, "debug", "b"), entry(13, "error", "c"), entry(14, "silly", "d")],
            14,
        ));
        assert_eq!(m.unseen(), 4, "the debug and the silly lines count too");
        m.mark_seen();
        assert_eq!(m.unseen(), 0);
        m.take(tail(vec![entry(15, "warn", "e")], 15));
        assert_eq!(m.unseen(), 1);
    }

    #[test]
    fn a_multi_line_message_shows_every_line_keeps_a_blank_one_between_and_no_control_characters() {
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(
            vec![
                entry(1, "error", "Error: ENOENT\r\n    at open (fs.js:1:1)\n\n    at main"),
                entry(2, "info", "tab\there \u{1b}[31mred\u{1b}[0m"),
                entry(3, "info", "\nopens on a newline"),
                entry(4, "info", "ends on one\n"),
            ],
            4,
        ));
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (60, 10), Rect::new(0, 0, 60, 10), &plain());
        assert_eq!(
            rows(&buf, 1..=9),
            [
                "09:00:01  Error: ENOENT",
                "              at open (fs.js:1:1)",
                "",
                "              at main",
                "09:00:02  tab    here [31mred[0m",
                "09:00:03  opens on a newline",
                "09:00:04  ends on one",
                "",
                "",
            ],
            "the trace's own indentation under the message's first cell, nothing after a closing newline"
        );
        for y in [1, 2, 4] {
            assert_eq!(buf[(14, y)].fg, th().danger, "every row of the error: {}", row(&buf, y));
        }
        assert_eq!(buf[(0, 1)].fg, th().dim);
        assert!(
            (1..8).all(|y| (0..60).all(|x| !buf[(x, y)].symbol().chars().any(char::is_control))),
            "no control character reaches a cell"
        );
    }

    // ── Wrapped lines ───────────────────────────────────────────────────────

    /// A log 31 cells wide: the clock's ten, then twenty for the message
    /// beside the scroll bar's column, which the tests below that overflow
    /// their rows all have (twenty-one where the lines fit).
    const NARROW: u16 = 31;

    const FOX: &str = "the quick brown fox jumps over the lazy dog again";

    /// The scroll bar's glyphs, which stand in its column.
    const BAR: [char; 4] = ['│', '█', '▲', '▼'];

    /// Rows `ys` of `buf` as the lines read, trimmed at their ends: a bar
    /// glyph in the last column, where the bar stands, left off.
    fn rows(buf: &Buffer, ys: std::ops::RangeInclusive<u16>) -> Vec<String> {
        ys.map(|y| {
            let mut text = row(buf, y);
            if text.ends_with(BAR) {
                text.pop();
            }
            text.trim_end().to_string()
        })
        .collect()
    }

    /// Row `y` of `buf` as the line reads (see [`rows`]).
    fn line(buf: &Buffer, y: u16) -> String {
        rows(buf, y..=y).remove(0)
    }

    /// Twelve lines, the even ones two rows at [`NARROW`] and the odd ones
    /// one: `line 2 and some more` / `words`, `line 10 and some` / `more
    /// words`.
    fn twelve(log: &mut LogUi) {
        log.model.take(tail(
            (1..=12)
                .map(|s| {
                    let more = if s % 2 == 0 { " and some more words" } else { "" };
                    entry(s, "info", &format!("line {s}{more}"))
                })
                .collect(),
            12,
        ));
    }

    /// Draw a log of twelve lines in seven rows at [`NARROW`].
    fn seven(log: &mut LogUi, ui: &mut Surface<LogAct>) -> Buffer {
        render(log, ui, (NARROW, 8), Rect::new(0, 0, NARROW, 8), &plain())
    }

    #[test]
    fn a_long_message_wraps_by_words_with_the_clock_on_its_first_row_only() {
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(
            vec![entry(1, "info", FOX), entry(2, "warn", FOX), entry(3, "error", FOX), entry(4, "debug", "short")],
            4,
        ));
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (NARROW, 12), Rect::new(0, 0, NARROW, 12), &plain());
        assert_eq!(
            rows(&buf, 1..=3),
            ["09:00:01  the quick brown fox", "          jumps over the lazy", "          dog again"]
        );
        assert_eq!(row(&buf, 4).trim_end(), "09:00:02  the quick brown fox");
        assert_eq!(row(&buf, 10).trim_end(), "09:00:04  short", "the newest at the bottom of the lines");
        assert_eq!(row(&buf, 11).trim(), "");
        for (ys, color) in [(1..=3, th().text), (4..=6, th().gold), (7..=9, th().danger)] {
            for y in ys {
                assert_eq!(buf[(10, y)].fg, color, "row {y}: {}", row(&buf, y));
            }
        }
        for y in [1, 4, 7] {
            assert_eq!(buf[(0, y)].fg, th().dim, "the clock is dim");
        }
        for y in [2, 3, 5, 6, 8, 9] {
            assert_eq!(&row(&buf, y)[..10], "          ", "row {y} is set in, with no clock");
        }
    }

    #[test]
    fn a_cjk_message_wraps_by_cells_and_never_splits_a_character() {
        // Twenty-one cells for the message: ten characters a row, since the
        // eleventh would end on the twenty-second cell.
        let message = "日本語のログメッセージは二つのセルで数えます";
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(vec![entry(1, "info", message)], 1));
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (31, 6), Rect::new(0, 0, 31, 6), &plain());
        assert_eq!(find(&buf, 1, "日本語のログメッセー"), Some(10), "{}", row(&buf, 1));
        assert_eq!(find(&buf, 2, "ジは二つのセルで数え"), Some(10), "{}", row(&buf, 2));
        assert_eq!(find(&buf, 3, "ます"), Some(10), "{}", row(&buf, 3));
        assert_eq!(buf[(30, 1)].symbol(), " ", "the odd cell is left empty");
        assert!(row(&buf, 1).starts_with("09:00:01  ") && row(&buf, 2).starts_with("          "));

        // However many cells, nothing is lost and no row runs over, but
        // for a row narrower than one character.
        for cells in [1, 2, 3, 7, 20, 21] {
            let parts = message_rows(message, cells);
            assert_eq!(parts.concat(), message, "{cells}");
            assert!(parts.iter().all(|p| width(p) <= cells.max(2)), "{cells}: {parts:?}");
        }
    }

    #[test]
    fn a_word_wider_than_a_row_breaks_at_the_rows_last_cell() {
        // Thirty cells, twenty past the clock: one line fits, so no bar.
        let message = "ENOENT: open /srv/music/library/albums/2026/a-very-long-folder-name/track.flac";
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(vec![entry(1, "error", message)], 1));
        let mut ui = Surface::new();
        let buf = render(&mut log, &mut ui, (30, 6), Rect::new(0, 0, 30, 6), &plain());
        assert_eq!(
            rows(&buf, 1..=4),
            [
                "09:00:01  ENOENT: open /srv/mu",
                "          sic/library/albums/2",
                "          026/a-very-long-fold",
                "          er-name/track.flac",
            ],
            "the path starts where it stands and fills every row"
        );

        // A word that fits a row moves to the next one whole; a line's own
        // indentation stays on its first row; spaces inside a row stay as
        // written; a blank line is one empty row.
        assert_eq!(wrap_line("    at open (/srv/x.js:1:1)", 20), ["    at open", "(/srv/x.js:1:1)"]);
        assert_eq!(wrap_line("a  b", 20), ["a  b"]);
        assert_eq!(wrap_line("", 20), [""]);
        assert_eq!(message_rows("", 20), [""], "an empty message still has its clock's row");
        assert_eq!(message_rows("a\nb", 0), [""], "no cells past the clock: the clock's row alone");
    }

    #[test]
    fn the_newest_line_stands_whole_at_the_bottom_and_one_cut_at_the_top_shows_its_tail() {
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(vec![entry(1, "info", FOX), entry(2, "info", "short"), entry(3, "info", FOX)], 3));
        let mut ui = Surface::new();
        // Five rows: the newest line's three, the short line, and the last
        // row of the oldest.
        let buf = render(&mut log, &mut ui, (NARROW, 6), Rect::new(0, 0, NARROW, 6), &plain());
        assert_eq!(
            rows(&buf, 1..=5),
            [
                "          dog again",
                "09:00:02  short",
                "09:00:03  the quick brown fox",
                "          jumps over the lazy",
                "          dog again",
            ]
        );
        assert!(log.model.following());
        assert_eq!(log.rows_at.as_ref().map(|(_, seqs)| seqs.clone()), Some(vec![1, 2, 3, 3, 3]));
    }

    #[test]
    fn home_shows_the_oldest_line_from_its_clock_however_the_rows_fall() {
        // Six one-row lines, a line of three rows, and one more. With the
        // bottom line always whole the oldest three would be out of reach
        // above seven rows; the top of the log cuts the bottom instead.
        let mut log = LogUi::new(None).utc();
        let mut entries: Vec<ActivityEntry> = (1..=6).map(|s| entry(s, "info", &format!("line {s}"))).collect();
        entries.push(entry(7, "info", FOX));
        entries.push(entry(8, "info", "line 8"));
        log.model.take(tail(entries, 8));
        let mut ui = Surface::new();
        let buf = seven(&mut log, &mut ui);
        assert_eq!(line(&buf, 1), "09:00:04  line 4");

        let top = [
            "09:00:01  line 1",
            "09:00:02  line 2",
            "09:00:03  line 3",
            "09:00:04  line 4",
            "09:00:05  line 5",
            "09:00:06  line 6",
            "09:00:07  the quick brown fox",
        ];
        log.key(key(KeyCode::Home), 7);
        assert_eq!(rows(&seven(&mut log, &mut ui), 1..=7), top);
        assert!(!log.model.following());
        log.key(key(KeyCode::Up), 7);
        log.wheel(true);
        log.key(key(KeyCode::PageUp), 7);
        assert_eq!(rows(&seven(&mut log, &mut ui), 1..=7), top, "nothing is older");

        // Down stands the cut line whole at the bottom, then follows.
        log.key(key(KeyCode::Down), 7);
        let buf = seven(&mut log, &mut ui);
        assert_eq!(line(&buf, 1), "09:00:03  line 3");
        assert_eq!(line(&buf, 7), "          dog again");
        log.key(key(KeyCode::Down), 7);
        assert!(log.model.following());
    }

    #[test]
    fn up_and_down_move_a_line_and_pages_move_as_many_lines_as_fit_a_page_of_rows() {
        let mut log = LogUi::new(None).utc();
        twelve(&mut log);
        let mut ui = Surface::new();
        let buf = seven(&mut log, &mut ui);
        assert_eq!(
            rows(&buf, 1..=7),
            [
                "          words",
                "09:00:09  line 9",
                "09:00:10  line 10 and some",
                "          more words",
                "09:00:11  line 11",
                "09:00:12  line 12 and some",
                "          more words",
            ],
            "line 8 cut at the top shows its last row"
        );
        let bottom = |log: &mut LogUi, ui: &mut Surface<LogAct>| line(&seven(log, ui), 7);

        // ↑ makes the line above the bottom the bottom, whole; ↓ comes back.
        log.key(key(KeyCode::Up), 7);
        assert_eq!(log.model.scroll, 1);
        assert_eq!(rows(&seven(&mut log, &mut ui), 1..=1), ["09:00:07  line 7"]);
        assert_eq!(bottom(&mut log, &mut ui), "09:00:11  line 11");
        log.key(key(KeyCode::Up), 7);
        assert_eq!(bottom(&mut log, &mut ui), "          more words");
        assert_eq!(log.model.scroll, 2);
        log.key(key(KeyCode::Down), 7);
        log.key(key(KeyCode::Down), 7);
        assert!(log.model.following());

        // PgUp goes past the four lines the view shows whole (12, 11, 10
        // and 9), so line 8, cut at the top, becomes the bottom.
        log.key(key(KeyCode::PageUp), 7);
        assert_eq!(log.model.scroll, 4);
        let buf = seven(&mut log, &mut ui);
        assert_eq!(line(&buf, 6), "09:00:08  line 8 and some more");
        assert_eq!(line(&buf, 7), "          words");
        // Another page would leave the view short: it stops at the top.
        log.key(key(KeyCode::PageUp), 7);
        assert_eq!(rows(&seven(&mut log, &mut ui), 1..=1), ["09:00:01  line 1"]);
        assert_eq!(bottom(&mut log, &mut ui), "09:00:05  line 5");
        // PgDn brings in the lines below that fit a page: 6 to 9, then the rest.
        log.key(key(KeyCode::PageDown), 7);
        assert_eq!(bottom(&mut log, &mut ui), "09:00:09  line 9");
        log.key(key(KeyCode::PageDown), 7);
        assert!(log.model.following());
        assert_eq!(bottom(&mut log, &mut ui), "          more words");

        // The wheel steps as the arrows do.
        log.wheel(true);
        assert_eq!(bottom(&mut log, &mut ui), "09:00:11  line 11");
        log.wheel(true);
        log.wheel(false);
        assert_eq!(bottom(&mut log, &mut ui), "09:00:11  line 11");
        log.wheel(false);
        assert!(log.model.following());
    }

    #[test]
    fn a_page_down_from_the_top_shows_the_rows_of_the_line_cut_at_the_bottom_first() {
        // A line of four rows, one of five, then ten of one: the top of the
        // log in seven rows cuts the second line after its third row.
        let lines = |third: &str| {
            let mut entries = vec![
                entry(1, "info", "one\none b\none c\none d"),
                entry(2, "info", "two\ntwo b\ntwo c\ntwo d\ntwo e"),
                entry(3, "info", third),
            ];
            entries.extend((4..=12).map(|s| entry(s, "info", &format!("line {s}"))));
            tail(entries, 12)
        };
        let mut log = LogUi::new(None).utc();
        log.model.take(lines("line 3"));
        let mut ui = Surface::new();
        seven(&mut log, &mut ui);
        log.key(key(KeyCode::Home), 7);
        assert_eq!(
            rows(&seven(&mut log, &mut ui), 5..=7),
            ["09:00:02  two", "          two b", "          two c"]
        );
        // The page counts the two rows still under the view: they come
        // in, and five lines below them.
        log.key(key(KeyCode::PageDown), 7);
        assert_eq!(
            rows(&seven(&mut log, &mut ui), 1..=7),
            [
                "          two d",
                "          two e",
                "09:00:03  line 3",
                "09:00:04  line 4",
                "09:00:05  line 5",
                "09:00:06  line 6",
                "09:00:07  line 7",
            ]
        );

        // When the line below would not fit beside those rows, the page
        // stands the cut line whole at the bottom first.
        let mut log = LogUi::new(None).utc();
        log.model.take(lines("three\nthree b\nthree c\nthree d\nthree e\nthree f"));
        seven(&mut log, &mut ui);
        log.key(key(KeyCode::Home), 7);
        log.key(key(KeyCode::PageDown), 7);
        assert_eq!(
            rows(&seven(&mut log, &mut ui), 1..=7),
            [
                "          one c",
                "          one d",
                "09:00:02  two",
                "          two b",
                "          two c",
                "          two d",
                "          two e",
            ]
        );
        log.key(key(KeyCode::PageDown), 7);
        assert_eq!(
            rows(&seven(&mut log, &mut ui), 1..=7),
            [
                "09:00:03  three",
                "          three b",
                "          three c",
                "          three d",
                "          three e",
                "          three f",
                "09:00:04  line 4",
            ]
        );

        // Paging down from the top to the newest passes every row.
        let mut seen = std::collections::HashSet::new();
        log.key(key(KeyCode::Home), 7);
        for _ in 0..20 {
            seen.extend(rows(&seven(&mut log, &mut ui), 1..=7));
            if log.model.following() {
                break;
            }
            log.key(key(KeyCode::PageDown), 7);
        }
        assert!(log.model.following());
        let mut every: Vec<String> = ["one", "one b", "one c", "one d", "two", "two b", "two c", "two d", "two e"]
            .into_iter()
            .chain(["three", "three b", "three c", "three d", "three e", "three f"])
            .map(String::from)
            .collect();
        every.extend((4..=12).map(|s| format!("line {s}")));
        for text in every {
            assert!(seen.iter().any(|row| row.ends_with(&format!("  {text}"))), "{text} never shown: {seen:?}");
        }
    }

    #[test]
    fn wrapped_lines_that_all_fit_follow_and_one_more_row_lets_the_view_pause() {
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(vec![entry(1, "info", FOX), entry(2, "info", FOX)], 2));
        let mut ui = Surface::new();
        seven(&mut log, &mut ui);
        log.key(key(KeyCode::Up), 7);
        log.wheel(true);
        assert!(log.model.following(), "six rows in seven: nothing to scroll");
        log.model.take(tail(vec![entry(3, "info", "line 3 and some more words")], 3));
        seven(&mut log, &mut ui);
        log.key(key(KeyCode::Up), 7);
        assert!(!log.model.following(), "eight rows in seven");
        log.key(key(KeyCode::Down), 7);
        assert!(log.model.following());
    }

    #[test]
    fn a_line_taller_than_the_view_is_read_a_row_at_a_time() {
        // A stack trace of nine rows, under a line, in a view of five.
        let trace = "Error: boom\n    at a\n    at b\n    at c\n    at d\n    at e\n    at f\n    at g\n    at h";
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(vec![entry(1, "info", "before"), entry(2, "error", trace)], 2));
        let mut ui = Surface::new();
        let five = |log: &mut LogUi, ui: &mut Surface<LogAct>| {
            rows(&render(log, ui, (NARROW, 6), Rect::new(0, 0, NARROW, 6), &plain()), 1..=5)
        };
        let frames = |from: char| -> Vec<String> {
            (0..5).map(|i| format!("              at {}", (from as u8 + i) as char)).collect()
        };
        assert_eq!(five(&mut log, &mut ui), frames('d'), "following: its last rows");

        // ↑ a row at a time to its first row, the clock's.
        log.key(key(KeyCode::Up), 5);
        assert!(!log.model.following());
        assert_eq!(five(&mut log, &mut ui), frames('c'));
        for _ in 0..3 {
            log.key(key(KeyCode::Up), 5);
        }
        let head = five(&mut log, &mut ui);
        assert_eq!(head[0], "09:00:02  Error: boom");
        assert_eq!(head[1..], frames('a')[..4]);
        // Then the line above it, here the top of the log.
        log.key(key(KeyCode::Up), 5);
        let top = five(&mut log, &mut ui);
        assert_eq!(top[..2], ["09:00:01  before", "09:00:02  Error: boom"]);

        // ↓ brings the trace in from its first row, then a row at a time.
        log.key(key(KeyCode::Down), 5);
        assert_eq!(five(&mut log, &mut ui), head);
        for _ in 0..3 {
            log.key(key(KeyCode::Down), 5);
        }
        assert!(!log.model.following());
        log.key(key(KeyCode::Down), 5);
        assert!(log.model.following(), "its last row at the bottom follows");

        // A page goes by a page of rows inside it, no further than its ends.
        log.key(key(KeyCode::PageUp), 5);
        assert_eq!(five(&mut log, &mut ui), head);
        log.key(key(KeyCode::PageDown), 5);
        assert!(log.model.following());

        // Lines arriving under it hold the rows the reader is on.
        log.key(key(KeyCode::Up), 5);
        let held = five(&mut log, &mut ui);
        log.model.take(tail(vec![entry(3, "info", "after")], 3));
        assert_eq!(five(&mut log, &mut ui), held);
        assert!(!log.model.following());
    }

    #[test]
    fn a_resize_re_flows_the_lines_under_the_same_bottom_line() {
        let mut log = LogUi::new(None).utc();
        twelve(&mut log);
        let mut ui = Surface::new();
        seven(&mut log, &mut ui);
        log.key(key(KeyCode::Up), 7);
        log.key(key(KeyCode::Up), 7);
        let wide = render(&mut log, &mut ui, (60, 8), Rect::new(0, 0, 60, 8), &plain());
        assert_eq!(line(&wide, 7), "09:00:10  line 10 and some more words", "one row each now");
        assert_eq!(line(&wide, 1), "09:00:04  line 4 and some more words");
        assert!(!log.model.following());
        let buf = seven(&mut log, &mut ui);
        assert_eq!(rows(&buf, 6..=7), ["09:00:10  line 10 and some", "          more words"]);
    }

    #[test]
    fn every_row_of_a_highlighted_line_is_lit_and_any_of_its_rows_means_the_line() {
        let mut log = LogUi::new(None).utc();
        twelve(&mut log);
        let mut ui = Surface::new();
        seven(&mut log, &mut ui);
        // Rows: 8's last on 1, 9 on 2, 10 on 3-4, 11 on 5, 12 on 6-7.
        assert_eq!(log.seq_at(1), Some(8), "the tail of a line cut at the top");
        assert_eq!(log.seq_at(4), Some(10), "a later row");
        assert_eq!(log.seq_at(7), Some(12));
        assert_eq!(log.seq_at(40), Some(12), "below the rows, the last line");

        log.model.highlight = Some((10, 10));
        let buf = seven(&mut log, &mut ui);
        for y in [3, 4] {
            for x in 0..NARROW - 1 {
                assert_eq!((buf[(x, y)].bg, buf[(x, y)].fg), (th().accent, th().on_accent), "({x}, {y})");
            }
        }
        for y in [2, 5] {
            assert_ne!(buf[(20, y)].bg, th().accent, "row {y} is another line");
        }

        // A drag from a later row to another line's later row.
        let select = |log: &mut LogUi, grip, y| log.act(LogAct::Select(grip, Position::new(5, y)));
        select(&mut log, Grip::Press, 4);
        assert_eq!(log.model.highlight, None, "the press cleared it");
        select(&mut log, Grip::Drag, 7);
        select(&mut log, Grip::Release, 7);
        assert_eq!(log.model.highlight, Some((10, 12)));
        let buf = seven(&mut log, &mut ui);
        assert!((3..=7).all(|y| buf[(20, y)].bg == th().accent), "every row of 10, 11 and 12");
        // From the cut line's tail at the top, a row down.
        select(&mut log, Grip::Press, 1);
        select(&mut log, Grip::Drag, 2);
        select(&mut log, Grip::Release, 2);
        assert_eq!(log.model.highlight, Some((8, 9)));
        // A plain click on a later row lights nothing.
        select(&mut log, Grip::Press, 6);
        select(&mut log, Grip::Release, 6);
        assert_eq!(log.model.highlight, None);
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
    fn the_header_follows_in_the_ok_colour_or_pauses_dim() {
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
        let hint = t!("gui.admin.log.hint_undock").to_string();
        let hx = find(&buf, 2, &hint).expect("the hint");
        assert_eq!(hx + width(&hint) as u16, area.right(), "right-aligned in the area");
        assert_eq!(buf[(hx, 2)].fg, th().dim);
        assert_eq!(row(&buf, 3).trim(), "", "a blank row when spaced");
        assert!(row(&buf, 4).contains("line"), "lines from the third row");

        // Paused: the bullet and the word dim, and the word follows again.
        log.key(key(KeyCode::Up), 5);
        let buf = render(&mut log, &mut ui, (80, 12), area, &look);
        let paused = find(&buf, 2, &t!("gui.admin.log.paused")).expect("paused");
        assert_eq!(buf[(4, 2)].fg, th().dim);
        assert_eq!(buf[(paused, 2)].fg, th().dim);
        assert_eq!(ui.hit(Position::new(paused, 2)), Some(LogAct::Follow));
        assert!(find(&buf, 2, &word).is_none());

        // Under the pointer the paused word is BRIGHT and BOLD.
        ui.pointer = Some(Position::new(paused, 2));
        let buf = render(&mut log, &mut ui, (80, 12), area, &look);
        assert_eq!(buf[(paused, 2)].fg, th().bright);
        assert!(buf[(paused, 2)].modifier.contains(Modifier::BOLD));
        ui.pointer = None;

        // Without a hint the row ends after the download.
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
    fn the_header_names_no_level_and_enter_does_nothing() {
        let _guard = english();
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 30));
        let mut ui = Surface::new();
        let area = Rect::new(0, 0, 60, 8);
        let buf = render(&mut log, &mut ui, (60, 8), area, &plain());
        assert_eq!(row(&buf, 0).trim_end(), "• following · copy · download");
        assert!(!row(&buf, 0).contains('▾'), "no level control");

        // Enter is the log's, and moves, opens and lights nothing.
        log.key(key(KeyCode::Up), 7);
        log.model.highlight = Some((25, 26));
        let before = render(&mut log, &mut ui, (60, 8), area, &plain());
        let clicks = ui.clicks.len();
        let view = (log.model.scroll, log.model.under, log.model.highlight);
        assert_eq!(log.key(key(KeyCode::Enter), 7), LogKey::Taken);
        assert_eq!((log.model.scroll, log.model.under, log.model.highlight), view);
        assert_eq!(render(&mut log, &mut ui, (60, 8), area, &plain()), before);
        assert_eq!(ui.clicks.len(), clicks, "nothing new to click");
    }

    // ── The scroll bar ──────────────────────────────────────────────────────

    /// The rows of column `x` that hold the bar's thumb.
    fn thumb(buf: &Buffer, x: u16) -> Vec<u16> {
        (0..buf.area.height).filter(|&y| buf[(x, y)].symbol() == "█").collect()
    }

    /// Sixty one-row lines in a 60×12 log: the header, then eleven rows of
    /// lines with the bar at x 59 — ▲ on row 1, the track on rows 2-10, ▼
    /// on row 11.
    fn sixty(log: &mut LogUi, ui: &mut Surface<LogAct>) -> Buffer {
        render(log, ui, (60, 12), Rect::new(0, 0, 60, 12), &plain())
    }

    #[test]
    fn the_bar_stands_only_while_the_lines_overflow_and_narrows_them_by_a_cell() {
        // Six lines, then one exactly fifty cells long, which one row past
        // the clock holds at the log's whole width: seven rows in seven.
        let long = format!("{}wordy", "word ".repeat(9));
        assert_eq!(width(&long), 50);
        let mut entries: Vec<ActivityEntry> = (1..=6).map(|s| entry(s, "info", &format!("line {s}"))).collect();
        entries.push(entry(7, "info", &long));
        let mut log = LogUi::new(None).utc();
        log.model.take(tail(entries, 7));
        let mut ui = Surface::new();
        let area = Rect::new(0, 0, 60, 8);
        let buf = render(&mut log, &mut ui, (60, 8), area, &plain());
        assert!((1..8).all(|y| buf[(59, y)].symbol() != "▲" && !thumb(&buf, 59).contains(&y)), "no bar");
        assert_eq!(row(&buf, 7), format!("09:00:07  {long}"), "the long line in one row, to the log's edge");
        assert_eq!(log.cells, Some(50));
        assert_eq!(
            ui.arm_region(Position::new(59, 3)),
            Some(LogAct::Select(Grip::Press, Position::new(59, 3))),
            "the lines' rows run to the edge"
        );
        ui.release();

        // One line more overflows: the bar on the last column, and the
        // lines beside it a cell narrower, so the long line takes two rows.
        log.model.take(tail(vec![entry(8, "info", "line 8")], 8));
        let buf = render(&mut log, &mut ui, (60, 8), area, &plain());
        assert_eq!(buf[(59, 1)].symbol(), "▲");
        assert_eq!(buf[(59, 7)].symbol(), "▼");
        assert!((2..7).all(|y| matches!(buf[(59, y)].symbol(), "│" | "█")), "the track between");
        assert_eq!(log.cells, Some(49));
        assert_eq!(
            rows(&buf, 5..=7),
            ["09:00:07  word word word word word word word word word", "          wordy", "09:00:08  line 8"]
        );
        let thumb_at = thumb(&buf, 59);
        assert!(!thumb_at.is_empty() && thumb_at.iter().all(|&y| buf[(59, y)].fg == th().accent), "the thumb in the accent");
        let track: Vec<u16> = (2..7).filter(|y| !thumb_at.contains(y)).collect();
        assert!(!track.is_empty() && track.iter().all(|&y| buf[(59, y)].fg == th().dim), "the track is dim");
        assert_eq!((buf[(59, 1)].fg, buf[(59, 7)].fg), (th().dim, th().dim), "and the endcaps");
        assert_eq!(ui.arm_region(Position::new(59, 3)), None, "the bar's column is no line row");
        assert_eq!(ui.arm_region(Position::new(58, 3)), Some(LogAct::Select(Grip::Press, Position::new(58, 3))));
        ui.release();

        // The bar brightens under the pointer, as every clickable does.
        ui.pointer = Some(Position::new(59, 4));
        let buf = render(&mut log, &mut ui, (60, 8), area, &plain());
        assert_eq!(buf[(59, 1)].fg, th().bright);
        assert!(thumb(&buf, 59).iter().all(|&y| buf[(59, y)].fg == th().bright));
        assert!(ui.hovering_clickable());
    }

    #[test]
    fn the_thumb_stands_at_the_bottom_while_following_and_climbs_as_the_view_scrolls() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 60));
        let mut ui = Surface::new();
        let following = thumb(&sixty(&mut log, &mut ui), 59);
        assert_eq!(following.last(), Some(&10), "on the track's last cell: {following:?}");

        let mut last = following[0];
        for _ in 0..4 {
            log.key(key(KeyCode::PageUp), 11);
            let now = thumb(&sixty(&mut log, &mut ui), 59);
            assert!(now[0] < last, "a page older climbs: {now:?} after {last}");
            last = now[0];
        }
        log.key(key(KeyCode::Home), 11);
        assert_eq!(thumb(&sixty(&mut log, &mut ui), 59).first(), Some(&2), "the top of the log, the track's first cell");
        log.key(key(KeyCode::End), 11);
        assert_eq!(thumb(&sixty(&mut log, &mut ui), 59), following);
    }

    #[test]
    fn at_the_top_of_the_log_the_thumb_is_at_the_top_though_the_bottom_line_is_cut() {
        // Six lines, a line of three rows and one more, in seven rows: the
        // top of the log cuts the long line after its first row.
        let mut log = LogUi::new(None).utc();
        let mut entries: Vec<ActivityEntry> = (1..=6).map(|s| entry(s, "info", &format!("line {s}"))).collect();
        entries.push(entry(7, "info", FOX));
        entries.push(entry(8, "info", "line 8"));
        log.model.take(tail(entries, 8));
        let mut ui = Surface::new();
        assert_eq!(thumb(&seven(&mut log, &mut ui), NARROW - 1), [4, 5, 6], "following: at the bottom");
        log.key(key(KeyCode::Home), 7);
        let buf = seven(&mut log, &mut ui);
        assert_eq!(line(&buf, 7), "09:00:07  the quick brown fox");
        assert_eq!(thumb(&buf, NARROW - 1), [2, 3, 4, 5], "from the top, six lines of eight long");
        // ↓ stands the cut line whole at the bottom, a step down the bar.
        log.key(key(KeyCode::Down), 7);
        assert_eq!(thumb(&seven(&mut log, &mut ui), NARROW - 1), [3, 4, 5]);
    }

    #[test]
    fn the_bars_endcaps_step_a_line_and_repeat_while_held() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 60));
        let mut ui = Surface::new();
        sixty(&mut log, &mut ui);
        let (up, down) = (Position::new(59, 1), Position::new(59, 11));
        assert_eq!(ui.hit(up), Some(LogAct::Older));
        assert_eq!(ui.hit(down), Some(LogAct::Newer));

        // A press steps at once, as ↑ does; held, it repeats after the
        // kit's pause, through the surface as a screen's loop drives it.
        log.act(LogAct::Older);
        assert_eq!(log.model.scroll, 1);
        assert!(!log.model.following());
        sixty(&mut log, &mut ui);
        ui.arm_bars(up);
        assert!(ui.holding_bar());
        assert_eq!(ui.hold_action(), None, "the pause before the first repeat");
        std::thread::sleep(crate::kit::ARROW_DELAY + Duration::from_millis(30));
        let repeat = ui.hold_action().expect("the repeat");
        assert_eq!(repeat, LogAct::Older);
        log.act(repeat);
        assert_eq!(log.model.scroll, 2);
        ui.release();
        assert!(!ui.holding_bar());
        assert_eq!(ui.hold_action(), None, "the release ends it");

        // ▼ comes back a line at a time, and the newest line follows again.
        sixty(&mut log, &mut ui);
        log.act(ui.hit(down).unwrap());
        assert_eq!(log.model.scroll, 1);
        log.act(LogAct::Newer);
        assert!(log.model.following());
        let buf = sixty(&mut log, &mut ui);
        assert!(row(&buf, 0).contains(&*t!("gui.admin.log.following")), "{}", row(&buf, 0));
    }

    #[test]
    fn a_track_press_jumps_and_the_thumb_follows_a_drag() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 60));
        let mut ui = Surface::new();
        sixty(&mut log, &mut ui);
        // Eleven lines whole of sixty: forty-nine positions past the first,
        // over the nine cells of track.
        let mid = Position::new(59, 6);
        assert_eq!(ui.hit(mid), Some(LogAct::Jump { at: 25, of: 49 }));
        log.act(LogAct::Jump { at: 25, of: 49 });
        assert_eq!(log.model.scroll, 24, "the bottom line twenty-four above the newest");
        let buf = sixty(&mut log, &mut ui);
        assert_eq!(line(&buf, 11), "09:00:36  line 36");
        assert_eq!(line(&buf, 1), "09:00:26  line 26", "line 26, about the 25th position, the oldest shown");
        assert!(thumb(&buf, 59).contains(&6), "the thumb under the press: {:?}", thumb(&buf, 59));

        // The press armed a drag: the thumb follows the hand to the top of
        // the log, past the bar's end, and back down to following.
        ui.arm_bars(mid);
        assert!(ui.holding_bar() && !ui.gripping());
        let to_top = ui.drag_action(Position::new(59, 2)).expect("the thumb follows");
        assert_eq!(to_top, LogAct::Jump { at: 0, of: 49 });
        log.act(to_top);
        let buf = sixty(&mut log, &mut ui);
        assert_eq!(line(&buf, 1), "09:00:01  line 1", "the top of the log");
        let past = ui.drag_action(Position::new(59, 40)).expect("past the bar's end");
        log.act(past);
        assert!(log.model.following(), "the bottom of the track follows");
        ui.release();
    }

    #[test]
    fn a_press_on_the_bar_starts_no_highlight_and_a_drag_from_the_lines_moves_no_thumb() {
        let mut log = LogUi::new(None).utc();
        log.model.take(infos(1, 60));
        let mut ui = Surface::new();
        let buf = sixty(&mut log, &mut ui);
        let thumb_was = thumb(&buf, 59);

        // A press on the bar's column, on every one of its rows, is the
        // bar's alone.
        for y in 1..=11 {
            assert_eq!(ui.arm_region(Position::new(59, y)), None, "row {y}");
            assert!(!ui.gripping());
        }

        // A drag from the lines across the bar and up past it: the
        // highlight follows the hand, and the view and the thumb stay.
        let press = ui.arm_region(Position::new(10, 8)).expect("the lines take the press");
        log.act(press);
        ui.arm_bars(Position::new(10, 8));
        assert!(ui.gripping() && !ui.holding_bar());
        for at in [Position::new(59, 6), Position::new(59, 2)] {
            let moved = ui.drag_action(at).expect("the grip follows the hand");
            assert!(matches!(moved, LogAct::Select(Grip::Drag, _)), "{moved:?}");
            log.act(moved);
        }
        let released = ui.release_at(Position::new(59, 2)).expect("the release is the grip's");
        log.act(released);
        assert_eq!(log.model.highlight, Some((57, 51)));
        assert!(log.model.following());
        assert_eq!(thumb(&sixty(&mut log, &mut ui), 59), thumb_was);
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

        log.model.take(LogTail { entries: vec![], last_seq: 0, capacity: 1000 });
        assert_eq!(sentence(&mut log, &mut ui), (t!("gui.admin.log.empty").to_string(), th().dim));

        log.model.fail(&ApiError::Network("connection refused".into()));
        let (text, fg) = sentence(&mut log, &mut ui);
        assert!(text.starts_with(&*t!("gui.admin.log.failed")), "{text}");
        assert_eq!(fg, th().gold);
        assert!(!log.model.gated, "a network failure only slows the poll");

        // With lines held (a debug line is one), the failure takes the last
        // row under them.
        log.model.take(tail(vec![entry(1, "debug", "quiet")], 1));
        log.model.fail(&ApiError::Network("connection refused".into()));
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
            if let Some(note) = log.take_note(usize::MAX).filter(|(text, _)| *text != fetching) {
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

        let long = Line::from_entry(&entry(2, "info", &"x".repeat(5000)));
        assert_eq!(long.whole.chars().count(), MESSAGE_CHARS);

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
    fn a_highlight_clears_on_a_restart_and_a_new_server() {
        let mut m = LogModel::new();
        m.take(infos(1, 10));
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
        let note = |log: &mut LogUi| log.take_note(usize::MAX).expect("a note");

        clipboard::catch(Copied::Clipboard);
        assert_eq!(log.key(key(KeyCode::Char('y')), 5), LogKey::Taken);
        assert_eq!(note(&mut log), (t!("gui.admin.log.copy_nothing").to_string(), false));
        assert!(clipboard::caught().is_empty(), "nothing went to the clipboard");

        log.model.take(tail(
            vec![entry(1, "info", "one"), entry(2, "debug", "chatter"), entry(3, "warn", "three"), entry(4, "info", "four")],
            4,
        ));
        log.key(key(KeyCode::Char('y')), 5);
        assert_eq!(
            clipboard::caught(),
            ["09:00:01  one\n09:00:02  chatter\n09:00:03  warn  three\n09:00:04  four"],
            "every line, the debug one too"
        );
        assert_eq!(note(&mut log), (t!("gui.admin.log.copied_all").to_string(), false));

        log.model.highlight = Some((3, 1));
        log.act(LogAct::CopyLines);
        assert_eq!(clipboard::caught(), ["09:00:01  one\n09:00:02  chatter\n09:00:03  warn  three"], "the highlight");
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
        assert_eq!(row(&buf, 0).trim_end(), "• following · copy · download");
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

        // A cell short of the download, it is left out rather than cut;
        // at twenty-nine cells it fits exactly.
        let narrow = Rect::new(0, 0, 28, 5);
        let buf = render(&mut log, &mut ui, (28, 5), narrow, &plain());
        assert_eq!(row(&buf, 0).trim_end(), "• following · copy");
        assert!(!ui.clicks.iter().any(|(_, act)| *act == LogAct::Download));
        let buf = render(&mut log, &mut ui, (29, 5), Rect::new(0, 0, 29, 5), &plain());
        assert_eq!(row(&buf, 0), "• following · copy · download");

        // While a download runs, the busy word in the accent, nothing to press.
        log.downloading = true;
        let buf = render(&mut log, &mut ui, (70, 5), Rect::new(0, 0, 70, 5), &plain());
        let busy = find(&buf, 0, "downloading…").expect("the busy word");
        assert_eq!(buf[(busy, 0)].fg, th().accent);
        assert_eq!(ui.hit(Position::new(busy + 2, 0)), None);
        log.downloading = false;

        // The hint gives way before the controls do.
        let look = Look { spaced: false, hint: Some(t!("gui.admin.log.hint_undock").to_string()) };
        let buf = render(&mut log, &mut ui, (40, 5), Rect::new(0, 0, 40, 5), &look);
        assert!(row(&buf, 0).ends_with("· download  L undocks"), "{}", row(&buf, 0));
        let buf = render(&mut log, &mut ui, (39, 5), Rect::new(0, 0, 39, 5), &look);
        assert_eq!(row(&buf, 0).trim_end(), "• following · copy · download");
    }

    #[test]
    fn d_saves_a_canned_servers_zip_and_o_shows_it() {
        let _en = english();
        let dir = Scratch::new("zip");
        let (server, seen) = canned(200, zip_bytes());
        let mut log = downloading_log(&server, &dir);

        log.key(key(KeyCode::Char('o')), 5);
        assert_eq!(log.take_note(usize::MAX), Some((t!("gui.admin.log.nothing_saved").to_string(), false)));

        assert_eq!(log.key(key(KeyCode::Char('d')), 5), LogKey::Taken);
        assert!(log.downloading());
        assert_eq!(log.take_note(usize::MAX), Some((t!("gui.admin.log.fetching").to_string(), false)));
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
    fn a_note_naming_a_file_keeps_its_words_whole_and_clips_the_path_at_its_front() {
        // Every locale ends these sentences with the path (the setup
        // tests pin it), so a clip at its front loses folders, never words.
        let _en = english();
        let path = "~/Downloads/mstream-logs-quickconnect-abcdefghijkl-20261002-090000.zip";
        let keys = [
            "gui.admin.log.saved",
            "gui.admin.log.saved_lines",
            "gui.admin.log.saved_no_files",
            "gui.admin.log.file_at",
            "gui.admin.log.show_failed",
        ];
        for key in keys {
            let words = t!(key, path = "").to_string();
            // The note at 100, 136 and 176 columns, and with room to spare.
            for cells in [30, 66, 106, 400] {
                let note = file_note(key, path, cells);
                let room = cells.saturating_sub(width(&words));
                if room >= width(path) {
                    assert_eq!(note, format!("{words}{path}"), "{key} at {cells}");
                } else if room > 0 {
                    let tail = &path[path.len() - (room - 1)..];
                    assert_eq!(note, format!("{words}…{tail}"), "{key} at {cells}");
                    assert_eq!(width(&note), cells, "{key} at {cells}");
                } else {
                    assert_eq!(note, words, "{key} at {cells}: the words, and no stray mark");
                }
            }
        }

        // The download's note, at a 176-column window: the words, then
        // the path, whole.
        let mut log = LogUi::new(None);
        let saved = PathBuf::from("/srv/drop/mstream-logs-127.0.0.1-20261002-090000.zip");
        log.land(Side::Saved(Ok(Saved::Zip(saved.clone()))));
        let (note, failed) = log.take_note(106).expect("a note");
        let whole = "saved · o shows it · /srv/drop/mstream-logs-127.0.0.1-20261002-090000.zip";
        assert_eq!((note.as_str(), failed), (whole, false));
        // And at 100 columns, the file's own name in what is left.
        log.land(Side::Shown(saved, Ok(HandOff::Headless("MSTREAM_NO_OPEN".into()))));
        let (note, _) = log.take_note(30).expect("a note");
        assert_eq!(note, "the file is at …002-090000.zip");
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
            assert_eq!(
                text,
                "09:00:01  one\n09:00:02  chatter\n09:00:03  error  three\n",
                "{status}: every line shown, whatever the highlight"
            );
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
        assert_eq!(lone.take_note(usize::MAX), None);
        log.stop();
    }
}
