//! The flash page: `mstream-player device flash` without `--yes`, drawn on
//! the admin hub's terminal session (the kit's surface, the hub's loop,
//! its chrome). One worker thread (flow.rs) does everything slow; this
//! page shows what it reports, asks the one question — the board as read,
//! the firmware to put on it, erase first or not — and keeps the board's
//! fate honest: the write cannot be left, and leaving before it restarts
//! the board into what it had.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Wrap};
use rust_i18n::t;

use super::FlashArgs;
use super::engine::{self, DeviceInfo};
use super::firmware::AppDesc;
use super::flow::{self, Cmd, Event, Kind, Phase, Plan};
use super::ports::Candidate;
use crate::admin::{Outcome, Screen, draw_bottom, draw_header_as, frame_ground};
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};

/// The card's four rows, the erase line, two tall buttons, the bottom
/// lines — and the longest label the rows carry.
const MIN_W: u16 = 72;
const MIN_H: u16 = 22;
const COLUMN_W: u16 = 78;
const LABEL_W: u16 = 15;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    Go,
    Cancel,
    ToggleErase,
    Pick(usize),
    Rescan,
    Retry,
    Close,
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

/// A fresh worker, for a retry.
type Respawn = Box<dyn Fn() -> (Sender<Cmd>, Receiver<Event>)>;

pub(crate) struct Page {
    cmds: Sender<Cmd>,
    events: Receiver<Event>,
    respawn: Respawn,
    step: Step,
    /// (version, origin) once the firmware is found: the header's right.
    firmware: Option<(String, String)>,
    download: Option<(u64, Option<u64>)>,
    board: Option<DeviceInfo>,
    phase: Option<Phase>,
    percent: Option<u8>,
    note: Option<(String, bool)>,
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
        Page {
            cmds,
            events,
            respawn,
            step: Step::Preparing,
            firmware: None,
            download: None,
            board: None,
            phase: None,
            percent: None,
            note: None,
            ui: Surface::new(),
        }
    }

    /// Fold one of the worker's reports into the page.
    fn apply(&mut self, event: Event) {
        match event {
            Event::Phase(phase) => {
                self.phase = Some(phase);
                if phase == Phase::Writing || phase == Phase::Comparing {
                    self.percent = None;
                }
                if matches!(self.step, Step::Preparing) && matches!(phase, Phase::Connecting | Phase::Reading) {
                    self.step = Step::Probing;
                }
            }
            Event::Download { done, total } => self.download = Some((done, total)),
            Event::Firmware { version, origin, .. } => {
                self.download = None;
                self.firmware = Some((version, origin));
            }
            Event::NoDevice => {
                if self.step != Step::Leaving {
                    self.step = Step::NoDevice;
                }
            }
            Event::Several(list) => {
                if self.step != Step::Leaving {
                    self.step = Step::Several { list, cursor: 0 };
                }
            }
            Event::Board(info) => {
                self.board = Some(info);
                if matches!(self.step, Step::Preparing) {
                    self.step = Step::Probing;
                }
            }
            Event::Probed { on_board, plan } => {
                // Esc while the board was being reached: the worker is now
                // waiting for the answer it will get — Quit.
                if self.step == Step::Cancelling {
                    let _ = self.cmds.send(Cmd::Quit);
                    return;
                }
                let erase = plan.erase;
                self.step = Step::Confirm { on_board, plan, erase };
            }
            Event::Progress(pct) => self.percent = Some(pct),
            Event::Done { version, skipped, boot } => {
                self.phase = None;
                self.step = if self.step == Step::Cancelling {
                    Step::Leaving
                } else {
                    Step::Done { version, skipped, boot }
                };
            }
            Event::Cancelled => self.step = Step::Leaving,
            Event::Failed(e) => {
                self.phase = None;
                self.step = if self.step == Step::Cancelling {
                    Step::Leaving
                } else {
                    Step::Failed { text: e.text(), hint: e.hint() }
                };
            }
        }
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Go => {
                if let Step::Confirm { erase, .. } = &self.step {
                    let erase = *erase;
                    if self.cmds.send(Cmd::Go { erase }).is_ok() {
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
                if self.step == Step::NoDevice {
                    self.step = Step::Preparing;
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
                    self.step = Step::Preparing;
                    self.board = None;
                    self.phase = None;
                    self.percent = None;
                    self.note = None;
                }
            }
            Act::Close => self.leave(),
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
        self.step = Step::Failed { text: t!("note.worker_gone").to_string(), hint: None };
    }

    /// The busy line: the phase, with the download's or the write's percent.
    fn busy(&self) -> Option<String> {
        if !matches!(self.step, Step::Preparing | Step::Probing | Step::Working | Step::Cancelling) {
            return None;
        }
        if self.step == Step::Cancelling {
            return Some(Phase::Restarting.text());
        }
        let phase = self.phase?;
        let mut text = phase.text();
        match phase {
            Phase::Firmware => {
                if let Some((done, Some(total))) = self.download
                    && total > 0
                {
                    text.push_str(&format!(" {}%", (done * 100 / total).min(100)));
                }
            }
            Phase::Writing => {
                if let Some(pct) = self.percent {
                    text.push_str(&format!(" {pct}%"));
                }
            }
            _ => {}
        }
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
            KeyCode::Char('r') => {
                let act = match self.step {
                    Step::NoDevice => Some(Act::Rescan),
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

    fn wheel(&mut self, up: bool, _at: Position) {
        if let Step::Several { list, cursor } = &mut self.step {
            let last = list.len().saturating_sub(1);
            *cursor = if up { cursor.saturating_sub(1) } else { (*cursor + 1).min(last) };
        }
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, page: &mut Page) {
    page.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    let right = page.firmware.as_ref().map(|(version, _)| version.clone()).unwrap_or_default();
    draw_header_as(frame, area, &t!("dev.title"), &right);

    let column_w = area.width.saturating_sub(4).min(COLUMN_W);
    let x = area.x + (area.width - column_w) / 2;
    let mut y = area.y + 2;
    y = wrapped(frame, x, y, column_w, &t!("dev.subtitle"), dim()) + 1;

    match page.step.clone() {
        Step::Preparing | Step::Probing | Step::Cancelling => {
            if let Some(board) = &page.board {
                y = card(frame, x, y, column_w, board, page.firmware.as_ref(), None);
            }
            let _ = y;
        }
        Step::NoDevice => {
            y = line(frame, x, y, column_w, &t!("dev.no_device_title"), gold_bold());
            y = wrapped(frame, x, y + 1, column_w, &t!("dev.no_device_body"), dim()) + 1;
            let close = t!("dev.close");
            buttons(frame, page, x, y, column_w, &t!("dev.rescan"), Act::Rescan, Some((&close, Act::Close)));
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
        }
        Step::Confirm { on_board, plan, erase } => {
            let board = page.board.as_ref().expect("a board before the question");
            y = card(frame, x, y, column_w, board, page.firmware.as_ref(), Some(&on_board));
            y = erase_row(frame, page, x, y + 1, column_w, erase, &plan) + 1;
            let cancel = t!("dev.cancel");
            buttons(frame, page, x, y, column_w, &plan.verb(), Act::Go, Some((&cancel, Act::Cancel)));
        }
        Step::Working => {
            if let Some(board) = &page.board {
                y = card(frame, x, y, column_w, board, page.firmware.as_ref(), None) + 1;
            }
            bar(frame, x, y, column_w, page.phase, page.percent);
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
            buttons(frame, page, x, y + 1, column_w, &t!("dev.close"), Act::Close, None);
        }
        Step::Failed { text, hint } => {
            y = line(frame, x, y, column_w, &t!("dev.failed_title"), gold_bold()) + 1;
            y = wrapped(frame, x, y, column_w, &text, Style::default().fg(th().gold));
            if let Some(hint) = hint {
                y = wrapped(frame, x, y, column_w, &hint, dim());
            }
            let close = t!("dev.close");
            buttons(frame, page, x, y + 1, column_w, &t!("dev.retry"), Act::Retry, Some((&close, Act::Close)));
        }
        Step::Leaving => {}
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

/// The board as read, the port, what is on it (when asked), what will go
/// on it. Returns the row after the card.
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

/// The primary and, beside it, the secondary. Always enabled: every step
/// that draws buttons is one where each of them can be pressed.
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
) {
    let at = Rect { x, y, width: w, height: 3 };
    let rect = kit::tall_button(frame, &mut page.ui, at, primary, true, act);
    if let Some((label, act)) = secondary {
        let at = Rect { x: rect.x + rect.width + 2, y, width: w.saturating_sub(rect.width + 2), height: 3 };
        kit::tall_secondary(frame, &mut page.ui, at, label, act);
    }
}

/// The write's bar: a filled share in the accent, the rest as a track,
/// the phase's words above it. Drawn by hand rather than with ratatui's
/// Gauge, whose remainder swaps the colors (the player's own reason).
fn bar(frame: &mut Frame, x: u16, y: u16, w: u16, phase: Option<Phase>, percent: Option<u8>) {
    let words = match (phase, percent) {
        (Some(Phase::Writing), Some(pct)) => format!("{} {pct}%", Phase::Writing.text()),
        (Some(phase), _) => phase.text(),
        (None, _) => String::new(),
    };
    frame.render_widget(Paragraph::new(Span::styled(words, accent())), Rect { x, y, width: w, height: 1 });
    let filled = match (phase, percent) {
        (Some(Phase::Writing), Some(pct)) => (u32::from(w) * u32::from(pct) / 100) as u16,
        (Some(Phase::Verifying | Phase::Restarting), _) => w,
        _ => 0,
    };
    let track = Rect { x, y: y + 1, width: w, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled("░".repeat(w as usize), dim())), track);
    if filled > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled("█".repeat(filled as usize), accent())),
            Rect { x, y: y + 1, width: filled, height: 1 },
        );
    }
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

    fn draw(page: &mut Page) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
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

    fn to_question(r: &mut Rig, on_board: Option<AppDesc>) {
        for event in [
            Event::Phase(Phase::Firmware),
            Event::Firmware { version: "v0.5.0".into(), origin: "release v0.5.0".into(), bytes: 2_289_360 },
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
        r.events.send(Event::Download { done: 50, total: Some(200) }).unwrap();
        r.page.pump();
        assert_eq!(r.page.busy().as_deref(), Some("getting the firmware… 25%"));
        to_question(&mut r, Some(ours("v0.4.0")));
        let update = Kind::Update { from: "v0.4.0".into() };
        assert!(matches!(r.page.step, Step::Confirm { ref plan, erase: false, .. } if plan.kind == update));
        let frame = draw(&mut r.page);
        assert!(frame.contains("mStream MP3 Player"), "{frame}");
        assert!(frame.contains("v0.5.0"), "the version to install:\n{frame}");
        assert!(frame.contains("COM3 · CH9102 · 921600 baud"), "{frame}");
        assert!(frame.contains("mstream-mp3-player v0.4.0"), "what is on the board:\n{frame}");
        assert!(frame.contains("Update ▸") && frame.contains("Cancel"), "{frame}");
        assert!(frame.contains("[ ] "), "no erase over our own firmware:\n{frame}");
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
        assert!(frame.contains("The board reports"), "{frame}");
        assert!(r.page.finished().is_none());
        r.page.key(key(KeyCode::Enter));
        assert!(matches!(r.page.finished(), Some(Outcome::Quit)));
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
    fn no_board_offers_a_rescan_and_several_boards_a_pick() {
        let _en = english();
        let mut r = rig();
        r.events.send(Event::NoDevice).unwrap();
        r.page.pump();
        let frame = draw(&mut r.page);
        assert!(frame.contains("No Core2 found") && frame.contains("Look again"), "{frame}");
        assert!(frame.contains("USB-C cable"), "the hints:\n{frame}");
        r.page.key(key(KeyCode::Char('r')));
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Rescan));
        assert_eq!(r.page.step, Step::Preparing);

        let board = |port: &str, bridge: &'static str, vid: u16, pid: u16| Candidate {
            port: port.into(),
            bridge,
            usb: serialport::UsbPortInfo { vid, pid, serial_number: None, manufacturer: None, product: None },
        };
        let two = vec![board("COM3", "CH9102", 0x1A86, 0x55D4), board("COM7", "CP210x", 0x10C4, 0xEA60)];
        r.events.send(Event::Several(two)).unwrap();
        r.page.pump();
        let frame = draw(&mut r.page);
        assert!(frame.contains("which one") && frame.contains("▸ COM3"), "{frame}");
        assert!(frame.contains("COM7 · CP210x"), "{frame}");
        r.page.key(key(KeyCode::Down));
        r.page.key(key(KeyCode::Down));
        r.page.key(key(KeyCode::Enter));
        assert_eq!(r.cmds.try_recv(), Ok(Cmd::Pick("COM7".into())), "the cursor stops at the last row");
        assert_eq!(r.page.step, Step::Probing);
    }

    #[test]
    fn a_failure_shows_its_hint_and_r_starts_a_fresh_worker() {
        let _en = english();
        let mut r = rig();
        let busy = DeviceError::Busy { port: "COM3".into(), detail: "Access is denied".into() };
        r.events.send(Event::Failed(busy)).unwrap();
        r.page.pump();
        let frame = draw(&mut r.page);
        assert!(frame.contains("That did not work"), "{frame}");
        assert!(frame.contains("COM3 is in use"), "{frame}");
        assert!(frame.contains("serial monitor"), "the hint:\n{frame}");
        assert!(frame.contains("Try again") && frame.contains("Close"), "{frame}");
        r.page.key(key(KeyCode::Char('r')));
        assert_eq!(r.respawns.load(Ordering::SeqCst), 1);
        assert_eq!(r.page.step, Step::Preparing);
        r.page.pump();
        assert!(matches!(r.page.step, Step::Failed { .. }), "the stub worker is gone: said so");
        r.page.key(key(KeyCode::Esc));
        assert!(matches!(r.page.finished(), Some(Outcome::Quit)));
    }

    #[test]
    fn a_small_window_is_asked_to_grow() {
        let _en = english();
        let mut r = rig();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal.draw(|frame| render(frame, &mut r.page)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..10)
            .flat_map(|y| (0..40).map(move |x| (x, y)))
            .map(|cell| buffer[cell].symbol().to_string())
            .collect();
        assert!(text.contains("larger"), "{text}");
    }
}
