//! Every board on the desk, on one worker thread: the MP3 Player tab's
//! (and `device flash --yes`'s) view of the Core2s plugged in. It watches
//! the ports every couple of seconds; each board that arrives is asked
//! over USB what it runs — `@status`, else `L` — with no reset, so its
//! music plays on and nothing about it is guessed; a board that leaves is
//! dropped. Everything else waits for the page to ask: ask again, read the
//! board's bootloader (a reset, said out loud), count the card's free
//! space, show "This one" on its screen, and the write — today's flow.rs
//! pieces, the baud ladder and the write with its retry — for one board,
//! or for every board that needs it, one after another.
//!
//! The worker runs each conversation as a job on a thread of its own, so
//! a slow board never stalls the others, and keeps the rules: one job a
//! board; one board in its bootloader on the whole desk (a write or a
//! read), and while it is there the others are looked at, never written;
//! a port another program holds is that board's state, and the worker
//! never takes it back by itself. The page hears the whole board each time
//! any of it changes ([`Event::Board`]), and lets the worker go with
//! [`Cmd::Quit`]: every listen lets its port go at once, a read finishes
//! and restarts the board it reset, a write is never cut — and then
//! [`Event::Released`] says every port is free.
//!
//! Before the boards, the target: the pin's tag is known at once, so the
//! boards are judged while the image downloads beside them (on a thread
//! of its own); a write waits for the image, and never reaches a board's
//! bootloader without it.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use rust_i18n::t;

use super::DeviceError;
use super::board::{Board, Count, CountWhy, Free, Heard, Probe, Verdict, Work, Written};
use super::engine::{DeviceInfo, Engine};
use super::firmware::{AppDesc, Firmware, Origin, Place, Source, Target, place};
use super::flow::{self, Kind, Phase, Plan, Stop};
use super::listen::{self, Asked, Facts, IdentifyWhy, Waits};
use super::ports::Candidate;

/// The engine the worker and its jobs share.
pub(crate) type Shared = Arc<dyn Engine>;

/// The worker's clocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Timing {
    /// The port watch.
    pub watch: Duration,
    /// How long the worker waits for a command before it looks at its
    /// jobs' news again.
    pub tick: Duration,
    /// `@status`'s answer from a board that was plugged in already: one
    /// running answers in its next loop pass.
    pub at_rest: Duration,
    /// …from a board plugged in while the page watched: it may be booting
    /// from the cable's power, and answers once its setup is done.
    pub arrival: Duration,
    /// …after a write's restart (the boot line came already: the setup
    /// mounts the card and reads the library's index).
    pub after_write: Duration,
    pub waits: Waits,
}

impl Timing {
    pub const REAL: Timing = Timing {
        watch: Duration::from_secs(2),
        tick: Duration::from_millis(25),
        at_rest: Duration::from_millis(1200),
        arrival: Duration::from_secs(8),
        after_write: Duration::from_secs(12),
        waits: Waits::REAL,
    };
}

/// What the page tells the worker. Boards are named by their port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Ask the board again, with no reset: Ask again (`r`) on a board in
    /// use, or after anything that may have changed it.
    Listen { port: String },
    /// The console's `L` for Details: the flash chip's size, the running
    /// app's version and ELF. No reset; boards that answer only.
    Facts { port: String },
    /// Read the board ▸: its bootloader — which resets it, the screen dark
    /// for a few seconds — then the board restarted as it was, at once.
    Read { port: String },
    /// Count the card's free space (`@count`): boards that answer
    /// `@status` only, never during a write.
    Count { port: String },
    /// Show on the player (`@identify <port>`): boards that answer
    /// `@status` only, never during a write.
    Identify { port: String },
    /// Write the firmware to the board — after the gate's yes. `erase` as
    /// the gate left it; none lets the board decide once its bootloader is
    /// read (erase over anything that is not ours — `--yes` with no erase
    /// flag). Waits for the image, and refused while another board is in
    /// its bootloader.
    Write { port: String, erase: Option<bool> },
    /// Update all: every one of `ports` that still needs an update, one
    /// after another, never erasing and never over anything but an older
    /// release of ours; stops at the first failure.
    UpdateAll { ports: Vec<String> },
    /// Leave: listens let go at once, a read finishes and restarts its
    /// board, a write is never cut; then Released.
    Quit,
}

/// What the worker tells the page, in the order things happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// What every board is measured against — said first when the source
    /// knows it (the pin, a release), and again once the image says.
    Target(Target),
    /// The image's download: bytes so far, and the total when the server
    /// said.
    Download { done: u64, total: Option<u64> },
    /// The image ready to write.
    Firmware { version: String, origin: String, bytes: usize, kind: Origin },
    /// The image could not be had: no write can start (the boards are read
    /// all the same; a write asked for later tries again).
    FirmwareFailed(DeviceError),
    /// The watch looked: the boards' ports in the OS's order, and the
    /// other serial ports (the no-player card's "ports seen"). Said after
    /// the first look and whenever either changed.
    Watch { ports: Vec<String>, others: Vec<String> },
    /// A board, whole, as it is now: arrived, heard, working, written.
    Board(Board),
    /// A board unplugged (or, mid-write, gone once its job ended).
    Gone { port: String },
    /// A write reached the board's bootloader and read it: what it found,
    /// and what it will do — for the log and `--yes`'s lines.
    Plan { port: String, info: DeviceInfo, on_board: Option<AppDesc>, plan: Plan },
    /// Show on the player's answer: the label on the board's screen, or
    /// why not.
    Identified { port: String, label: String, result: Result<(), IdentifyWhy> },
    /// Update all, as it goes.
    All(All),
    /// A command that cannot run now, and why. Nothing was touched.
    /// `write`: it was a Write or Update all — the refusal the page that
    /// locked itself at the gate's yes waits for. A refusal of a command
    /// sent before that yes (Details' `L`, a count) can name the same board
    /// and arrive after it, and the write it came ahead of goes on.
    Refused { port: Option<String>, why: Refusal, write: bool },
    /// A line for the page's log.
    Log { port: Option<String>, text: String, kind: LogKind },
    /// The ports could not be listed at all (said once; the watch goes on).
    Failed(DeviceError),
    /// After Quit: every port let go, every board the worker reset
    /// restarted. The worker has ended.
    Released,
}

/// How a log line is meant: a phase (the accent), a fact, a quiet line
/// (the host lines word for word, a boot line), a failure (gold).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogKind {
    Phase,
    Fact,
    Quiet,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// No board on that port.
    NoBoard,
    /// Another board is in its bootloader (a write, a read, Update all):
    /// one at a time — look, don't write.
    OneAtATime { busy: String },
    /// The board is busy with something else.
    Busy,
    /// It does not answer `@status` (a count, Show on the player) or the
    /// console (Details' facts).
    NotAnswering,
    /// Update all found no board that needs an update.
    NothingToUpdate,
    /// The worker is letting go.
    Leaving,
}

/// Update all, as it goes: the boards it writes, in order, and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum All {
    /// Writing `ports[at]`; the ones before it are done.
    Running { ports: Vec<String>, at: usize },
    /// Every board written, one after another, in `took` — but the ones
    /// `passed` over: unplugged before their turn, or no longer behind.
    Done { ports: Vec<String>, passed: Vec<String>, took: Duration },
    /// Stopped at `ports[at]` — its write failed (`error`), or its
    /// bootloader showed something Update all does not write over — and
    /// the rest were not started.
    Stopped { ports: Vec<String>, at: usize, error: Option<DeviceError> },
}

/// Start the worker. The page keeps the two channel ends; the worker ends
/// once Quit (or the page's end dropped) has let every port go.
/// `firmware_first`: no board is looked at until the image is in hand —
/// `--yes`, which writes whatever it finds, prints its lines in today's
/// order and never touches a board for an image it cannot have.
pub(crate) fn spawn(
    engine: Shared,
    source: Source,
    port: Option<String>,
    timing: Timing,
    firmware_first: bool,
) -> (Sender<Cmd>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = channel();
    let (event_tx, event_rx) = channel();
    std::thread::Builder::new()
        .name("mstream-desk".to_string())
        .spawn(move || run(engine, source, port, timing, firmware_first, &cmd_rx, &event_tx))
        .expect("spawn the device worker");
    (cmd_tx, event_rx)
}

/// What the worker's jobs tell it.
enum Note {
    Log { port: String, text: String, kind: LogKind },
    Work { port: String, id: u64, work: Work },
    Counted { port: String, id: u64, pct: u8 },
    Plan { port: String, id: u64, info: DeviceInfo, on_board: Option<AppDesc>, plan: Plan },
    Ended { port: String, id: u64, end: End },
    Fetched(Result<Firmware, DeviceError>),
}

/// How a job ended.
enum End {
    Heard(Asked),
    Facts(Result<Facts, DeviceError>),
    Read(Result<Probe, DeviceError>),
    Count(Result<u64, (CountWhy, bool)>),
    Identify { label: String, result: Result<(), IdentifyWhy> },
    Written(Result<Done, Failure>),
    /// Told to stop before it learned anything.
    Stopped,
}

struct Done {
    version: String,
    took: Duration,
    skipped: bool,
    install: bool,
    boot: Option<String>,
    info: DeviceInfo,
}

enum Failure {
    Failed { error: DeviceError, half: bool, pct: Option<u8> },
    /// Update all's look at the bootloader found something it does not
    /// write over; the board was restarted as it was.
    NotOlder { found: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobKind {
    Listen,
    Facts,
    Read,
    Count,
    Identify,
    Write,
}

impl JobKind {
    /// The board is reset into its bootloader: one such job on the desk.
    fn bootloader(self) -> bool {
        matches!(self, JobKind::Read | JobKind::Write)
    }
}

struct Job {
    kind: JobKind,
    id: u64,
    stop: Arc<AtomicBool>,
}

/// A write asked for and not started yet.
#[derive(Clone, Copy)]
struct Queued {
    erase: Option<bool>,
    /// Update all's: only over an older release of ours.
    guard: bool,
}

struct Slot {
    board: Board,
    job: Option<Job>,
    /// In the last look at the ports. A board gone with a write or a read
    /// under way is kept until its job ends.
    present: bool,
    queued: Option<Queued>,
}

enum Fw {
    Fetching,
    Ready(Arc<Firmware>),
    Failed,
}

struct AllRun {
    ports: Vec<String>,
    at: usize,
    passed: Vec<String>,
    since: Instant,
}

struct Desk {
    engine: Shared,
    source: Source,
    scope: Option<String>,
    timing: Timing,
    events: Sender<Event>,
    notes_tx: Sender<Note>,
    notes: Receiver<Note>,
    target: Option<Target>,
    firmware: Fw,
    slots: Vec<Slot>,
    others: Vec<String>,
    /// The first look at the ports is done.
    watched: bool,
    last_scan: Option<Instant>,
    list_failed: bool,
    all: Option<AllRun>,
    next_id: u64,
    quitting: bool,
}

/// The worker's whole life. Every `send` may fail once the page is gone;
/// the page's end dropped is a Quit.
pub(crate) fn run(
    engine: Shared,
    source: Source,
    scope: Option<String>,
    timing: Timing,
    firmware_first: bool,
    cmds: &Receiver<Cmd>,
    events: &Sender<Event>,
) {
    let (notes_tx, notes) = channel();
    let mut desk = Desk {
        engine,
        target: source.target(),
        source,
        scope: scope.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
        timing,
        events: events.clone(),
        notes_tx,
        notes,
        firmware: Fw::Fetching,
        slots: Vec::new(),
        others: Vec::new(),
        watched: false,
        last_scan: None,
        list_failed: false,
        all: None,
        next_id: 0,
        quitting: false,
    };
    if let Some(target) = desk.target.clone() {
        desk.tell(Event::Target(target));
    }
    desk.fetch();
    let mut page = true;
    loop {
        if page {
            match cmds.recv_timeout(desk.timing.tick) {
                Ok(cmd) => desk.command(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    page = false;
                    desk.leave();
                }
            }
        } else {
            std::thread::sleep(desk.timing.tick);
        }
        while let Ok(note) = desk.notes.try_recv() {
            desk.note(note);
        }
        let due = desk.last_scan.is_none_or(|at| at.elapsed() >= desk.timing.watch);
        let may_look = !firmware_first || matches!(desk.firmware, Fw::Ready(_));
        if !desk.quitting && due && may_look {
            desk.scan();
        }
        desk.start_queued();
        if desk.quitting && desk.slots.iter().all(|s| s.job.is_none()) {
            desk.tell(Event::Released);
            return;
        }
    }
}

impl Desk {
    fn tell(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn log(&self, port: Option<&str>, text: String, kind: LogKind) {
        self.tell(Event::Log { port: port.map(str::to_string), text, kind });
    }

    fn find(&self, port: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.board.port().eq_ignore_ascii_case(port))
    }

    /// The board on `port` judged again and told whole.
    fn show(&mut self, i: usize) {
        let target = self.target.clone();
        let board = &mut self.slots[i].board;
        board.verdict = board.judge(target.as_ref());
        let board = board.clone();
        self.tell(Event::Board(board));
    }

    /// The board in its bootloader, or about to be: a read, a write, or a
    /// write waiting its turn.
    fn in_bootloader(&self) -> Option<&str> {
        self.slots
            .iter()
            .find(|s| s.job.as_ref().is_some_and(|j| j.kind.bootloader()) || s.queued.is_some())
            .map(|s| s.board.port())
    }

    // ── The image ───────────────────────────────────────────────────────────

    /// The image, fetched on a thread of its own: a download the first
    /// time, the cache after, a file's read.
    fn fetch(&mut self) {
        self.firmware = Fw::Fetching;
        self.log(None, Phase::Firmware.text(), LogKind::Phase);
        let source = self.source.clone();
        let (events, notes) = (self.events.clone(), self.notes_tx.clone());
        std::thread::Builder::new()
            .name("mstream-firmware".to_string())
            .spawn(move || {
                let result = source.resolve(&mut |done, total| {
                    let _ = events.send(Event::Download { done, total });
                });
                let _ = notes.send(Note::Fetched(result));
            })
            .expect("spawn the firmware fetch");
    }

    fn fetched(&mut self, result: Result<Firmware, DeviceError>) {
        match result {
            Ok(firmware) => {
                let target = self.source.target_of(&firmware);
                self.tell(Event::Firmware {
                    version: firmware.version.clone(),
                    origin: firmware.origin.clone(),
                    bytes: firmware.bytes(),
                    kind: firmware.kind,
                });
                let kb = firmware.bytes() / 1024;
                let line = format!("firmware: {} ({}, {kb} KB)", firmware.version, firmware.origin);
                self.log(None, line, LogKind::Fact);
                self.firmware = Fw::Ready(Arc::new(firmware));
                // The boards judged again first: whoever has heard the new
                // target has heard every verdict it changed.
                if self.target.as_ref() != Some(&target) {
                    self.target = Some(target.clone());
                    for i in 0..self.slots.len() {
                        self.show(i);
                    }
                    self.tell(Event::Target(target));
                }
            }
            Err(e) => {
                self.firmware = Fw::Failed;
                self.log(None, e.text(), LogKind::Fail);
                self.tell(Event::FirmwareFailed(e.clone()));
                // The writes that waited for it fail with its reason: no
                // board was touched.
                for i in 0..self.slots.len() {
                    if self.slots[i].queued.take().is_some() {
                        let board = &mut self.slots[i].board;
                        board.work = Work::Idle;
                        board.written = Some(Written::Failed { error: e.clone(), half: false, pct: None });
                        self.show(i);
                    }
                }
                self.all_step(false, Some(e));
            }
        }
    }

    // ── The ports ───────────────────────────────────────────────────────────

    /// The boards `--port` asks for: the one named, listed or not (a port
    /// named by hand is opened as named); else every Core2-shaped one.
    fn scoped(&self, found: Vec<Candidate>) -> Vec<Candidate> {
        let Some(name) = &self.scope else { return found };
        let named: Vec<Candidate> = found.into_iter().filter(|c| c.port.eq_ignore_ascii_case(name)).collect();
        if named.is_empty() { vec![Candidate::bare(name)] } else { named }
    }

    /// The watch: who arrived, who left. A board is known by its port and
    /// its USB serial — the same port with another board on it is a board
    /// that left and one that arrived.
    fn scan(&mut self) {
        self.last_scan = Some(Instant::now());
        let found = match self.engine.candidates() {
            Ok(found) => {
                self.list_failed = false;
                self.scoped(found)
            }
            Err(e) => {
                if !self.list_failed {
                    self.list_failed = true;
                    self.log(None, e.text(), LogKind::Fail);
                    self.tell(Event::Failed(e));
                }
                return;
            }
        };
        if !self.watched {
            self.log(None, Phase::Scanning.text(), LogKind::Phase);
        }
        let same = |a: &Candidate, b: &Candidate| {
            a.port.eq_ignore_ascii_case(&b.port) && a.usb.serial_number == b.usb.serial_number
        };
        let mut changed = !self.watched;
        let mut i = 0;
        while i < self.slots.len() {
            let here = found.iter().any(|c| same(c, &self.slots[i].board.candidate));
            self.slots[i].present = here;
            let held = self.slots[i].job.as_ref().is_some_and(|j| j.kind.bootloader());
            if here || held {
                i += 1;
                continue;
            }
            let slot = self.slots.remove(i);
            if let Some(job) = slot.job {
                job.stop.store(true, Ordering::Relaxed);
            }
            self.gone(slot.board.port());
            changed = true;
        }
        // Update all's board unplugged before its turn came: passed over.
        let current = self.all.as_ref().and_then(|run| run.ports.get(run.at));
        if current.is_some_and(|port| self.find(port).is_none()) {
            self.all_turn();
        }
        for candidate in &found {
            if self.slots.iter().any(|s| same(candidate, &s.board.candidate)) {
                continue;
            }
            let wait = if self.watched {
                let line = t!("dev.log_plugged", port = candidate.port).to_string();
                self.log(Some(&candidate.port), line, LogKind::Fact);
                self.timing.arrival
            } else {
                self.timing.at_rest
            };
            self.slots.push(Slot { board: Board::new(candidate.clone()), job: None, present: true, queued: None });
            let at = self.slots.len() - 1;
            self.show(at);
            self.listen(at, wait);
            changed = true;
        }
        let order = |s: &Slot| found.iter().position(|c| same(c, &s.board.candidate)).unwrap_or(usize::MAX);
        self.slots.sort_by_key(order);
        let others = self.engine.others();
        if others != self.others {
            self.others = others;
            changed = true;
        }
        self.watched = true;
        if changed {
            let ports = self.slots.iter().map(|s| s.board.port().to_string()).collect();
            self.tell(Event::Watch { ports, others: self.others.clone() });
        }
    }

    fn gone(&mut self, port: &str) {
        self.log(Some(port), t!("dev.log_unplugged", port = port).to_string(), LogKind::Fact);
        self.tell(Event::Gone { port: port.to_string() });
    }

    // ── The page's word ─────────────────────────────────────────────────────

    fn command(&mut self, cmd: Cmd) {
        if self.quitting {
            if cmd != Cmd::Quit {
                let write = matches!(cmd, Cmd::Write { .. } | Cmd::UpdateAll { .. });
                self.tell(Event::Refused { port: None, why: Refusal::Leaving, write });
            }
            return;
        }
        match cmd {
            Cmd::Quit => self.leave(),
            Cmd::Listen { port } => {
                let Some(i) = self.board_for(&port, false) else { return };
                match &self.slots[i].job {
                    // Asked already: the answer is coming.
                    Some(job) if job.kind == JobKind::Listen => {}
                    Some(_) => self.refuse(&port, Refusal::Busy, false),
                    None if self.slots[i].queued.is_some() => self.refuse(&port, Refusal::Busy, false),
                    None => self.listen(i, self.timing.at_rest),
                }
            }
            Cmd::Facts { port } => self.light(&port, JobKind::Facts),
            Cmd::Read { port } => self.light(&port, JobKind::Read),
            Cmd::Count { port } => self.light(&port, JobKind::Count),
            Cmd::Identify { port } => self.light(&port, JobKind::Identify),
            Cmd::Write { port, erase } => {
                let Some(i) = self.board_for(&port, true) else { return };
                if let Some(busy) = self.in_bootloader().filter(|b| !b.eq_ignore_ascii_case(&port)) {
                    let busy = busy.to_string();
                    return self.refuse(&port, Refusal::OneAtATime { busy }, true);
                }
                if self.all.is_some() || self.slots[i].job.as_ref().is_some_and(|j| j.kind.bootloader()) {
                    return self.refuse(&port, Refusal::Busy, true);
                }
                self.queue(i, Queued { erase, guard: false });
            }
            Cmd::UpdateAll { ports } => {
                if let Some(busy) = self.in_bootloader() {
                    let busy = busy.to_string();
                    let why = Refusal::OneAtATime { busy };
                    return self.tell(Event::Refused { port: None, why, write: true });
                }
                let wanted: Vec<String> = ports
                    .iter()
                    .filter_map(|p| self.find(p))
                    .filter(|i| self.slots[*i].present && self.slots[*i].board.needs_update())
                    .map(|i| self.slots[i].board.port().to_string())
                    .collect();
                if wanted.is_empty() {
                    let why = Refusal::NothingToUpdate;
                    return self.tell(Event::Refused { port: None, why, write: true });
                }
                self.log(None, t!("dev.log_all", list = wanted.join(", ")).to_string(), LogKind::Phase);
                self.all = Some(AllRun { ports: wanted, at: 0, passed: Vec::new(), since: Instant::now() });
                self.all_turn();
            }
        }
    }

    /// `write`: the refusal answers a Write (see [`Event::Refused`]).
    fn refuse(&self, port: &str, why: Refusal, write: bool) {
        self.tell(Event::Refused { port: Some(port.to_string()), why, write });
    }

    /// The board on `port`, or a refusal said.
    fn board_for(&self, port: &str, write: bool) -> Option<usize> {
        let found = self.find(port);
        if found.is_none() {
            self.refuse(port, Refusal::NoBoard, write);
        }
        found
    }

    /// A job other than a listen or a write, once its rules allow: nothing
    /// else on the board; nothing while another board is in its bootloader
    /// (look, don't write — a read is a reset too); a count and Show on the
    /// player only on a board that answers `@status`, Details' `L` on one
    /// that answers at all.
    fn light(&mut self, port: &str, kind: JobKind) {
        let Some(i) = self.board_for(port, false) else { return };
        if let Some(busy) = self.in_bootloader() {
            let busy = busy.to_string();
            return self.refuse(port, Refusal::OneAtATime { busy }, false);
        }
        if self.slots[i].job.is_some() {
            return self.refuse(port, Refusal::Busy, false);
        }
        let board = &self.slots[i].board;
        let answers = match kind {
            JobKind::Count | JobKind::Identify => board.answers_status(),
            JobKind::Facts => matches!(board.heard, Heard::Status(_) | Heard::Old { .. }),
            _ => true,
        };
        if !answers {
            return self.refuse(port, Refusal::NotAnswering, false);
        }
        if kind == JobKind::Read {
            self.log(Some(port), t!("dev.log_read", port = port).to_string(), LogKind::Phase);
        }
        self.start(i, kind);
    }

    /// Queue a write on board `i`: whatever light job runs on it lets its
    /// port go first; the image may still be coming.
    fn queue(&mut self, i: usize, queued: Queued) {
        if let Some(job) = &self.slots[i].job {
            job.stop.store(true, Ordering::Relaxed);
        }
        self.slots[i].queued = Some(queued);
        self.slots[i].board.work = Work::Queued;
        self.show(i);
        if matches!(self.firmware, Fw::Failed) {
            self.fetch();
        }
    }

    /// Start the write that waits, once its board is free and the image is
    /// in hand.
    fn start_queued(&mut self) {
        if self.quitting {
            return;
        }
        let Fw::Ready(firmware) = &self.firmware else { return };
        let firmware = firmware.clone();
        let Some(i) = self.slots.iter().position(|s| s.queued.is_some() && s.job.is_none()) else { return };
        let queued = self.slots[i].queued.take().expect("queued");
        let guard = if queued.guard { self.target.clone() } else { None };
        let id = self.job(i, JobKind::Write);
        let (engine, notes) = (self.engine.clone(), self.notes_tx.clone());
        let candidate = self.slots[i].board.candidate.clone();
        self.slots[i].board.written = None;
        self.slots[i].board.work = Work::Writing { phase: Phase::Connecting, pct: None };
        self.show(i);
        std::thread::spawn(move || write_job(&*engine, &candidate, &firmware, queued.erase, guard, &notes, id));
    }

    /// Let go: every job that can stop, stops; a write runs to its end.
    fn leave(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.all = None;
        for slot in &mut self.slots {
            if slot.queued.take().is_some() {
                slot.board.work = Work::Idle;
            }
            if let Some(job) = &slot.job
                && job.kind != JobKind::Write
            {
                job.stop.store(true, Ordering::Relaxed);
            }
        }
    }

    // ── Jobs ────────────────────────────────────────────────────────────────

    /// A job on board `i`: its id and its stop.
    fn job(&mut self, i: usize, kind: JobKind) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.slots[i].job = Some(Job { kind, id, stop: Arc::new(AtomicBool::new(false)) });
        id
    }

    /// Ask board `i` what it runs, giving it `wait` to answer.
    fn listen(&mut self, i: usize, wait: Duration) {
        let id = self.job(i, JobKind::Listen);
        let stop = self.slots[i].job.as_ref().expect("just made").stop.clone();
        self.slots[i].board.work = Work::Listening;
        self.show(i);
        let (engine, notes, waits) = (self.engine.clone(), self.notes_tx.clone(), self.timing.waits);
        let candidate = self.slots[i].board.candidate.clone();
        std::thread::spawn(move || {
            let asked = listen_one(&*engine, &candidate, wait, waits, &stop, &notes);
            let _ = notes.send(Note::Ended { port: candidate.port.clone(), id, end: End::Heard(asked) });
        });
    }

    /// The light jobs, each on a thread of its own.
    fn start(&mut self, i: usize, kind: JobKind) {
        let id = self.job(i, kind);
        let stop = self.slots[i].job.as_ref().expect("just made").stop.clone();
        self.slots[i].board.work = match kind {
            JobKind::Read => Work::Reading,
            JobKind::Count => Work::Counting,
            JobKind::Identify => Work::Identifying,
            _ => Work::Listening,
        };
        if kind == JobKind::Count {
            self.slots[i].board.count = Count::Running { pct: 0 };
        }
        self.show(i);
        let (engine, notes, waits) = (self.engine.clone(), self.notes_tx.clone(), self.timing.waits);
        let candidate = self.slots[i].board.candidate.clone();
        std::thread::spawn(move || {
            let port = candidate.port.clone();
            let end = match kind {
                JobKind::Read => read_job(&*engine, &candidate, &stop, &notes),
                JobKind::Count => End::Count(
                    talk(&*engine, &candidate, &notes, |wire, note| {
                        let mut progress = |pct: u8| {
                            let _ = notes.send(Note::Counted { port: port.clone(), id, pct });
                        };
                        Ok(listen::count(wire, &port, waits, &stop, &mut progress, note))
                    })
                    .unwrap_or_else(|e| Err((CountWhy::Port(e), false))),
                ),
                JobKind::Identify => {
                    let label = listen::label_for(&port);
                    let result = talk(&*engine, &candidate, &notes, |wire, note| {
                        Ok(listen::identify(wire, &port, &label, waits, &stop, note))
                    })
                    .unwrap_or_else(|e| Err(IdentifyWhy::Port(e)));
                    End::Identify { label, result }
                }
                _ => End::Facts(talk(&*engine, &candidate, &notes, |wire, note| {
                    listen::facts(wire, &port, waits, &stop, note)
                })),
            };
            let _ = notes.send(Note::Ended { port, id, end });
        });
    }

    // ── What the jobs say ───────────────────────────────────────────────────

    fn note(&mut self, note: Note) {
        let mine = |desk: &Desk, port: &str, id: u64| {
            desk.find(port).filter(|i| desk.slots[*i].job.as_ref().is_some_and(|j| j.id == id))
        };
        match note {
            Note::Log { port, text, kind } => self.log(Some(&port), text, kind),
            Note::Fetched(result) => self.fetched(result),
            Note::Work { port, id, work } => {
                if let Some(i) = mine(self, &port, id) {
                    self.slots[i].board.work = work;
                    self.show(i);
                }
            }
            Note::Counted { port, id, pct } => {
                if let Some(i) = mine(self, &port, id) {
                    self.slots[i].board.count = Count::Running { pct };
                    self.show(i);
                }
            }
            Note::Plan { port, id, info, on_board, plan } => {
                if let Some(i) = mine(self, &port, id) {
                    let board = &mut self.slots[i].board;
                    board.flash_mb = info.flash_mb.or(board.flash_mb);
                    board.probe = Some(Ok(Probe { info: info.clone(), on_board: on_board.clone() }));
                    self.show(i);
                }
                self.tell(Event::Plan { port, info, on_board, plan });
            }
            Note::Ended { port, id, end } => {
                let Some(i) = mine(self, &port, id) else { return };
                let slot = &mut self.slots[i];
                slot.job = None;
                // A write asked for meanwhile stopped this job; it waits on.
                slot.board.work = if slot.queued.is_some() { Work::Queued } else { Work::Idle };
                self.ended(i, end);
            }
        }
    }

    fn ended(&mut self, i: usize, end: End) {
        let port = self.slots[i].board.port().to_string();
        let mut then_listen = None;
        let mut all = None;
        match end {
            End::Stopped => {}
            // Stopped part way: nothing learned, nothing claimed.
            End::Heard(Asked { heard: Heard::Nothing, .. }) => {}
            End::Heard(asked) => {
                let board = &mut self.slots[i].board;
                if matches!(asked.heard, Heard::Status(_) | Heard::Old { .. })
                    && matches!(board.written, Some(Written::Failed { .. }))
                {
                    // It runs again: whatever went wrong is behind it.
                    board.written = None;
                }
                board.flash_mb = asked.flash_mb.or(board.flash_mb);
                board.heard = asked.heard;
                let failed = matches!(board.heard, Heard::InUse { .. } | Heard::Failed(_));
                let line = heard_line(board);
                self.log(Some(&port), line, if failed { LogKind::Fail } else { LogKind::Fact });
            }
            End::Facts(Ok(facts)) => {
                let board = &mut self.slots[i].board;
                board.flash_mb = facts.flash_mb.or(board.flash_mb);
                if let (Heard::Old { version: version @ None, elf }, Some(found)) =
                    (&mut board.heard, facts.version)
                {
                    *version = Some(found);
                    *elf = facts.elf;
                }
            }
            End::Facts(Err(e)) => self.log(Some(&port), e.text(), LogKind::Fail),
            End::Read(probe) => {
                let ours = matches!(&probe, Ok(Probe { on_board: Some(desc), .. }) if desc.is_ours());
                match &probe {
                    Ok(found) => {
                        self.log(Some(&port), flow::descriptor_line(found.on_board.as_ref()), LogKind::Fact);
                        self.log(Some(&port), t!("dev.log_read_done", port = port).to_string(), LogKind::Fact);
                    }
                    Err(e) => self.log(Some(&port), e.text(), LogKind::Fail),
                }
                let board = &mut self.slots[i].board;
                if let Ok(found) = &probe {
                    board.flash_mb = found.info.flash_mb.or(board.flash_mb);
                }
                board.probe = Some(probe);
                // Restarted, our firmware may well answer now (a board
                // that had hung): asked once more, with a boot's wait.
                if ours {
                    then_listen = Some(self.timing.arrival);
                }
            }
            End::Count(Ok(free)) => {
                let board = &mut self.slots[i].board;
                if let Heard::Status(status) = &mut board.heard {
                    status.free = Free::Bytes(free);
                }
                board.count = Count::Done { free };
                let free = flow::grouped(free as usize);
                let line = t!("dev.log_count_done", port = port, free = free).to_string();
                self.log(Some(&port), line, LogKind::Fact);
            }
            End::Count(Err((why, part_way))) => self.slots[i].board.count = Count::Refused { why, part_way },
            End::Identify { label, result } => {
                if result.is_ok() {
                    let line = t!("dev.log_identify", port = port, label = label).to_string();
                    self.log(Some(&port), line, LogKind::Fact);
                }
                self.tell(Event::Identified { port: port.clone(), label, result });
            }
            End::Written(Ok(done)) => {
                let elf = done.boot.as_deref().and_then(listen::parse_boot_line).and_then(|(_, elf)| elf);
                let written = AppDesc {
                    version: done.version.clone(),
                    project: AppDesc::OURS.to_string(),
                    idf: String::new(),
                    elf8: elf.unwrap_or_default(),
                };
                let board = &mut self.slots[i].board;
                // What the board said before is history; until it speaks
                // again, the image just written is what it runs.
                board.heard = Heard::Nothing;
                board.count = Count::Idle;
                board.flash_mb = done.info.flash_mb.or(board.flash_mb);
                board.probe = Some(Ok(Probe { info: done.info, on_board: Some(written) }));
                board.written = Some(Written::Done {
                    version: done.version,
                    took: done.took,
                    skipped: done.skipped,
                    install: done.install,
                    boot: done.boot,
                });
                then_listen = Some(self.timing.after_write);
                all = Some((true, None));
            }
            End::Written(Err(Failure::Failed { error, half, pct })) => {
                let board = &mut self.slots[i].board;
                if half {
                    board.heard = Heard::Nothing;
                }
                board.written = Some(Written::Failed { error: error.clone(), half, pct });
                self.log(Some(&port), error.text(), LogKind::Fail);
                if let Some(hint) = error.hint() {
                    self.log(Some(&port), hint, LogKind::Quiet);
                }
                all = Some((false, Some(error)));
            }
            End::Written(Err(Failure::NotOlder { found })) => {
                self.log(Some(&port), t!("dev.log_guard", port = port, found = found).to_string(), LogKind::Fail);
                all = Some((false, None));
            }
        }
        if !self.slots[i].present {
            // Unplugged while it was in its bootloader: told now, dropped.
            self.show(i);
            self.slots.remove(i);
            self.gone(&port);
        } else if let Some(wait) = then_listen.filter(|_| !self.quitting) {
            self.listen(i, wait);
        } else {
            self.show(i);
        }
        if let Some((ok, error)) = all {
            self.all_step(ok, error);
        }
    }

    // ── Update all ──────────────────────────────────────────────────────────

    /// The current board's write ended: on to the next that still needs
    /// one, or stop at a failure.
    fn all_step(&mut self, ok: bool, error: Option<DeviceError>) {
        let Some(run) = &mut self.all else { return };
        if !ok {
            let (ports, at) = (run.ports.clone(), run.at);
            self.all = None;
            let port = ports.get(at).cloned().unwrap_or_default();
            self.log(Some(&port), t!("dev.log_all_stopped", port = port).to_string(), LogKind::Fail);
            self.tell(Event::All(All::Stopped { ports, at, error }));
            return;
        }
        run.at += 1;
        self.all_turn();
    }

    /// Queue Update all's board at its place, passing over the ones that
    /// no longer need it (unplugged, written meanwhile); done past the
    /// last.
    fn all_turn(&mut self) {
        loop {
            let Some(run) = &self.all else { return };
            let Some(port) = run.ports.get(run.at).cloned() else {
                let (ports, passed, took) = (run.ports.clone(), run.passed.clone(), run.since.elapsed());
                self.all = None;
                let n = ports.len() - passed.len();
                self.log(None, t!("dev.log_all_done", n = n).to_string(), LogKind::Fact);
                self.tell(Event::All(All::Done { ports, passed, took }));
                return;
            };
            let at = run.at;
            let ready = self.find(&port).filter(|i| {
                let slot = &self.slots[*i];
                slot.present && matches!(slot.board.verdict, Verdict::Update | Verdict::DevUpdate)
            });
            match ready {
                Some(i) => {
                    let ports = run.ports.clone();
                    self.queue(i, Queued { erase: Some(false), guard: true });
                    self.tell(Event::All(All::Running { ports, at }));
                    return;
                }
                None => {
                    self.log(Some(&port), t!("dev.log_all_skip", port = port).to_string(), LogKind::Fact);
                    if let Some(run) = &mut self.all {
                        run.passed.push(port);
                        run.at += 1;
                    }
                }
            }
        }
    }
}

/// The log's line for what a listen heard.
fn heard_line(board: &Board) -> String {
    let port = board.port();
    match &board.heard {
        Heard::Status(status) => t!("dev.log_heard", port = port, version = status.fw).to_string(),
        Heard::Old { version: Some(version), .. } => {
            t!("dev.log_heard_old", port = port, version = version).to_string()
        }
        Heard::Old { version: None, .. } => t!("dev.log_heard_old_unknown", port = port).to_string(),
        Heard::Silent | Heard::Nothing => t!("dev.log_silent", port = port).to_string(),
        Heard::InUse { detail } => DeviceError::Busy { port: port.to_string(), detail: detail.clone() }.text(),
        Heard::Failed(e) => e.text(),
    }
}

/// A conversation on the board's port, opened with no reset; the port is
/// let go when it ends. The host lines go to the log, word for word.
fn talk<T>(
    engine: &dyn Engine,
    candidate: &Candidate,
    notes: &Sender<Note>,
    with: impl FnOnce(&mut dyn listen::Wire, listen::Note) -> Result<T, DeviceError>,
) -> Result<T, DeviceError> {
    let port = candidate.port.clone();
    let mut wire = engine.listen(candidate)?;
    let mut note = |line: String| {
        let _ = notes.send(Note::Log { port: port.clone(), text: line, kind: LogKind::Quiet });
    };
    with(&mut *wire, &mut note)
}

/// Listen to one board: `@status`, else `L`, with no reset. A port in use
/// is that board's state; one that would not open, its failure.
fn listen_one(
    engine: &dyn Engine,
    candidate: &Candidate,
    wait: Duration,
    waits: Waits,
    stop: &AtomicBool,
    notes: &Sender<Note>,
) -> Asked {
    let port = candidate.port.clone();
    let _ = notes.send(Note::Log {
        port: port.clone(),
        text: t!("dev.log_listen", port = port).to_string(),
        kind: LogKind::Phase,
    });
    let heard = |heard: Heard| Asked { heard, flash_mb: None, boot: None };
    match talk(engine, candidate, notes, |wire, note| listen::ask(wire, &port, wait, waits, stop, note)) {
        Ok(asked) => asked,
        Err(DeviceError::Busy { detail, .. }) => heard(Heard::InUse { detail }),
        Err(e) => heard(Heard::Failed(e)),
    }
}

/// Every board in `found`, asked at once, each on a thread of its own: the
/// command line's `device list`, with no worker and no reset.
pub(crate) fn listen_all(engine: &dyn Engine, found: &[Candidate], wait: Duration, waits: Waits) -> Vec<Board> {
    let (notes, _quiet) = channel();
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let asks: Vec<_> = found
            .iter()
            .map(|candidate| {
                let (notes, stop) = (notes.clone(), &stop);
                scope.spawn(move || listen_one(engine, candidate, wait, waits, stop, &notes))
            })
            .collect();
        asks.into_iter()
            .zip(found)
            .map(|(ask, candidate)| {
                let mut board = Board::new(candidate.clone());
                let asked = ask.join().unwrap_or(Asked { heard: Heard::Silent, flash_mb: None, boot: None });
                board.heard = asked.heard;
                board.flash_mb = asked.flash_mb;
                board
            })
            .collect()
    })
}

/// Read the board: its bootloader (a reset), then restarted at once. A
/// stop is heard between the baud ladder's rungs.
fn read_job(engine: &dyn Engine, candidate: &Candidate, stop: &AtomicBool, notes: &Sender<Note>) -> End {
    let port = candidate.port.clone();
    let tell = |event: flow::Event| {
        if let flow::Event::Log(text) = event {
            let _ = notes.send(Note::Log { port: port.clone(), text, kind: LogKind::Fact });
        }
    };
    let quit = || stop.load(Ordering::Relaxed);
    match flow::open(engine, candidate, None, Some(&quit), &tell) {
        Ok(mut link) => {
            let info = link.info().clone();
            let read = link.app_desc();
            // Restarted as it was, never held dark while somebody decides.
            link.let_go();
            End::Read(read.map(|on_board| Probe { info, on_board }))
        }
        Err(Stop::Quit) => End::Stopped,
        Err(Stop::Failed(e)) => {
            // A wrong flash was reset by the engine on its way out; a wrong
            // chip, or no sync, may have left the board in its bootloader.
            if matches!(e, DeviceError::NoSync { .. } | DeviceError::WrongChip { .. }) {
                engine.let_go(candidate);
            }
            End::Read(Err(e))
        }
    }
}

/// The write, start to finish, for one board: its bootloader reached and
/// read, Update all's look (`guard`), the plan, the erase, the write, the
/// restart and its first line. Nothing stops it once it has begun.
fn write_job(
    engine: &dyn Engine,
    candidate: &Candidate,
    firmware: &Firmware,
    erase: Option<bool>,
    guard: Option<Target>,
    notes: &Sender<Note>,
    id: u64,
) {
    let port = candidate.port.clone();
    let since = Instant::now();
    let pct = Cell::new(None);
    let touched = Cell::new(false);
    let log = |text: String, kind: LogKind| {
        let _ = notes.send(Note::Log { port: port.clone(), text, kind });
    };
    let work = |phase: Phase, pct: Option<u8>| {
        let _ = notes.send(Note::Work { port: port.clone(), id, work: Work::Writing { phase, pct } });
    };
    let tell = |event: flow::Event| match event {
        flow::Event::Phase(phase) => {
            if matches!(phase, Phase::Erasing | Phase::Writing) {
                touched.set(true);
            }
            if phase == Phase::Comparing {
                pct.set(None);
            }
            log(phase.text(), LogKind::Phase);
            work(phase, pct.get());
        }
        flow::Event::Progress(n) => {
            touched.set(true);
            // The log gets the tens; the board every percent.
            if n.is_multiple_of(10) && pct.get() != Some(n) {
                log(format!("{n}%"), LogKind::Fact);
            }
            pct.set(Some(n));
            work(Phase::Writing, Some(n));
        }
        flow::Event::Log(text) => log(text, LogKind::Fact),
    };
    let end = |end: Result<Done, Failure>| {
        let _ = notes.send(Note::Ended { port: port.clone(), id, end: End::Written(end) });
    };
    let failed = |error: DeviceError, half: bool, at: Option<u8>| Err(Failure::Failed { error, half, pct: at });

    tell(flow::Event::Phase(Phase::Connecting));
    let mut link = match flow::open(engine, candidate, None, None, &tell) {
        Ok(link) => link,
        Err(Stop::Failed(e)) => {
            if matches!(e, DeviceError::NoSync { .. } | DeviceError::WrongChip { .. }) {
                engine.let_go(candidate);
            }
            return end(failed(e, false, None));
        }
        Err(Stop::Quit) => return end(failed(DeviceError::Gone { port: port.clone() }, false, None)),
    };
    let info = link.info().clone();
    tell(flow::Event::Phase(Phase::Reading));
    let on_board = match link.app_desc() {
        Ok(desc) => desc,
        Err(e) => {
            link.let_go();
            return end(failed(e, false, None));
        }
    };
    log(flow::descriptor_line(on_board.as_ref()), LogKind::Fact);
    if let Some(target) = &guard {
        let older = on_board
            .as_ref()
            .is_some_and(|d| d.is_ours() && place(&d.version, &target.version) == Place::Older);
        if !older {
            link.let_go();
            return end(Err(Failure::NotOlder { found: flow::on_board_text(on_board.as_ref()) }));
        }
    }
    let plan = flow::plan(on_board.as_ref(), &firmware.version, erase);
    let install = matches!(plan.kind, Kind::Install { .. });
    let _ = notes.send(Note::Plan {
        port: port.clone(),
        id,
        info: info.clone(),
        on_board: on_board.clone(),
        plan: plan.clone(),
    });
    log(t!("dev.log_go", plan = plan.describe()).to_string(), LogKind::Fact);
    if plan.erase {
        tell(flow::Event::Phase(Phase::Erasing));
        if let Err(e) = link.erase() {
            return end(failed(e, true, None));
        }
    }
    let (link, skipped) = match flow::write(engine, candidate, link, firmware, &tell) {
        Ok(done) => done,
        Err(e) => return end(failed(e, touched.get(), pct.get())),
    };
    log(t!(if skipped { "dev.log_same" } else { "dev.log_checked" }).to_string(), LogKind::Fact);
    tell(flow::Event::Phase(Phase::Restarting));
    log(t!("dev.log_restart", secs = super::engine::BOOT_LISTEN.as_secs()).to_string(), LogKind::Fact);
    match link.restart() {
        Ok(boot) => {
            if let Some(line) = &boot {
                log(line.clone(), LogKind::Quiet);
            }
            let version = firmware.version.clone();
            end(Ok(Done { version, took: since.elapsed(), skipped, install, boot, info }))
        }
        // Written and checked, and the reset's line would not go: the image
        // is whole; the board needs its reset button, not another write.
        Err(e) => end(failed(e, false, None)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::device::board::{Card, CardKind, CardUnknown, Primary};
    use crate::device::engine::fake::Fake;
    use crate::device::firmware::tests::{desc_bytes, merged_bytes};

    /// The worker's clocks in miniature: the boards here answer at once, or
    /// never, so a wait is only how long a silent board takes — long
    /// enough that a board that does answer is never taken for a silent
    /// one on a machine busy with the whole suite.
    pub(crate) const QUICK: Timing = Timing {
        watch: Duration::from_millis(100),
        tick: Duration::from_millis(5),
        at_rest: Duration::from_millis(800),
        arrival: Duration::from_millis(800),
        after_write: Duration::from_millis(800),
        waits: listen::tests::QUICK,
    };

    /// An image in miniature, named per test (they run in parallel).
    pub(crate) fn image(test: &str, version: &str) -> PathBuf {
        let name = format!("mstream-player-desk-{}-{test}-{version}.bin", std::process::id());
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, merged_bytes(&desc_bytes(version, AppDesc::OURS))).unwrap();
        path
    }

    /// A worker on `fake`, the image `version` beside it, its two far ends
    /// the test's, and what it said folded as it comes.
    struct Rig {
        cmds: Sender<Cmd>,
        events: Receiver<Event>,
        boards: BTreeMap<String, Board>,
        seen: Vec<Event>,
        trace: Arc<Mutex<Vec<String>>>,
        image: PathBuf,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.image);
        }
    }

    fn rig(test: &str, fake: Fake, version: &str) -> Rig {
        rig_with(test, fake, version, None, false)
    }

    fn rig_with(test: &str, fake: Fake, version: &str, port: Option<&str>, first: bool) -> Rig {
        let trace = fake.trace();
        let image = image(test, version);
        let source = Source::Local(image.clone());
        let (cmds, events) = spawn(Arc::new(fake), source, port.map(str::to_string), QUICK, first);
        Rig { cmds, events, boards: BTreeMap::new(), seen: Vec::new(), trace, image }
    }

    impl Rig {
        fn fold(&mut self, event: Event) {
            match &event {
                Event::Board(board) => {
                    self.boards.insert(board.port().to_string(), board.clone());
                }
                Event::Gone { port } => {
                    self.boards.remove(port);
                }
                _ => {}
            }
            self.seen.push(event);
        }

        /// Read on until `done` holds of the rig, five seconds at most.
        fn until(&mut self, what: &str, done: impl Fn(&Rig) -> bool) {
            let t0 = Instant::now();
            while !done(self) {
                assert!(t0.elapsed() < Duration::from_secs(5), "never: {what}\n{:#?}", self.seen);
                if let Ok(event) = self.events.recv_timeout(Duration::from_millis(20)) {
                    self.fold(event);
                }
            }
        }

        /// Every board heard once — nothing asking, nothing under way — and
        /// judged against the image's own version: a file's target is known
        /// once it is read, which a busy machine can do after a board
        /// answered (the board is judged again then).
        fn settled(&mut self, n: usize) {
            self.until(&format!("{n} boards heard"), |r| {
                r.seen.iter().any(|e| matches!(e, Event::Target(_)))
                    && r.boards.len() == n
                    && r.boards.values().all(|b| b.verdict != Verdict::Asking && b.work == Work::Idle)
            });
        }

        fn board(&self, port: &str) -> &Board {
            self.boards.get(port).unwrap_or_else(|| panic!("{port} on the desk: {:?}", self.boards.keys()))
        }

        fn send(&self, cmd: Cmd) {
            self.cmds.send(cmd).unwrap();
        }

        fn trace(&self) -> Vec<String> {
            self.trace.lock().unwrap().clone()
        }

        /// The trace without the conversations: what reset a board.
        fn resets(&self) -> Vec<String> {
            let talk = |t: &String| t.starts_with("listen") || t.starts_with("identify") || t.starts_with("count");
            self.trace().into_iter().filter(|t| !talk(t)).collect()
        }

        fn refused(&self) -> Vec<(Option<String>, Refusal)> {
            self.seen
                .iter()
                .filter_map(|e| match e {
                    Event::Refused { port, why, .. } => Some((port.clone(), why.clone())),
                    _ => None,
                })
                .collect()
        }

        fn all(&self) -> Vec<All> {
            self.seen
                .iter()
                .filter_map(|e| match e {
                    Event::All(all) => Some(all.clone()),
                    _ => None,
                })
                .collect()
        }

        fn logs(&self) -> Vec<String> {
            self.seen
                .iter()
                .filter_map(|e| match e {
                    Event::Log { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        }

        fn quit(mut self) -> Rig {
            self.send(Cmd::Quit);
            self.until("released", |r| r.seen.last() == Some(&Event::Released));
            self
        }
    }

    #[test]
    fn opening_listens_to_every_board_and_resets_none() {
        let _en = crate::setup::tests::in_locale("en");
        let mut r = rig("open", Fake::new("status:v0.8.0,old:v0.7.0,silent:v0.7.0"), "v0.8.0");
        r.settled(3);
        let up = r.board("FAKE0");
        assert_eq!(up.verdict, Verdict::UpToDate);
        assert!(matches!(up.card(), Card::Fat { size: Some(63_864_569_856), .. }), "{:?}", up.card());
        let old = r.board("FAKE1");
        assert_eq!(old.verdict, Verdict::Update, "@err 7, then L's version");
        assert_eq!(old.version(), Some("v0.7.0"));
        assert_eq!(old.flash_mb, Some(16), "L's partition line");
        assert_eq!(old.card(), Card::Unknown(CardUnknown::OldFirmware));
        assert_eq!(old.primary(), Some(Primary::Update));
        let silent = r.board("FAKE2");
        assert_eq!(silent.verdict, Verdict::Silent);
        assert_eq!(silent.primary(), Some(Primary::Read));
        assert!(r.resets().is_empty(), "nothing reset, nothing opened but to listen: {:?}", r.trace());
        let all_three = ["FAKE0", "FAKE1", "FAKE2"];
        assert!(r.seen.iter().any(|e| matches!(e, Event::Watch { ports, .. } if ports == &all_three)));
        assert!(r.seen.iter().any(|e| matches!(e, Event::Target(t) if t.version == "v0.8.0")), "the image's own version");
        let logs = r.logs();
        assert!(logs.contains(&"listening on FAKE0 — DTR and RTS low, no reset".to_string()), "{logs:?}");
        assert!(logs.iter().any(|l| l.starts_with("@status fw=v0.8.0")), "the host lines, word for word: {logs:?}");
        assert!(logs.contains(&"FAKE1: v0.7.0, too old to report its card".to_string()), "{logs:?}");
        r.quit();
    }

    #[test]
    fn a_board_plugged_in_is_heard_without_a_reset_and_one_unplugged_is_dropped() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("status:v0.8.0,old:v0.7.0/in=1.2,status:v0.9.0/out=1.2");
        let mut r = rig("plug", fake, "v0.8.0");
        r.settled(2);
        assert!(r.boards.contains_key("FAKE2") && !r.boards.contains_key("FAKE1"));
        r.until("FAKE1 in, FAKE2 out", |r| {
            r.boards.get("FAKE1").is_some_and(|b| b.verdict == Verdict::Update) && !r.boards.contains_key("FAKE2")
        });
        assert!(r.seen.iter().any(|e| matches!(e, Event::Gone { port } if port == "FAKE2")));
        assert!(r.logs().contains(&"FAKE1 plugged in: asking it over USB, without a restart".to_string()));
        assert!(r.resets().is_empty(), "{:?}", r.trace());
        r.quit();
    }

    #[test]
    fn a_port_another_program_holds_is_that_boards_state_and_ask_again_reads_it_once_free() {
        let mut r = rig("busy", Fake::new("status:v0.8.0,status:v0.8.0/held=0.5"), "v0.8.0");
        r.settled(2);
        assert_eq!(r.board("FAKE0").verdict, Verdict::UpToDate, "the other board is read as usual");
        let held = r.board("FAKE1");
        assert_eq!(held.verdict, Verdict::InUse);
        assert_eq!(held.card(), Card::Unknown(CardUnknown::InUse));
        assert!(!r.seen.iter().any(|e| matches!(e, Event::Failed(_))), "not the page's failure");
        std::thread::sleep(Duration::from_millis(600));
        assert!(matches!(r.board("FAKE1").heard, Heard::InUse { .. }), "never taken back by itself");
        r.send(Cmd::Listen { port: "FAKE1".into() });
        r.until("asked again", |r| r.board("FAKE1").verdict == Verdict::UpToDate);
        r.quit();
    }

    #[test]
    fn read_the_board_is_the_one_reset_and_restarts_it_as_it_was() {
        let mut r = rig("read", Fake::new("other,chip,silent:v0.7.0,fresh"), "v0.8.0");
        r.settled(4);
        r.send(Cmd::Read { port: "FAKE0".into() });
        r.send(Cmd::Read { port: "FAKE1".into() });
        r.until("FAKE0 read", |r| matches!(r.board("FAKE0").verdict, Verdict::Other { .. }));
        assert_eq!(r.board("FAKE0").verdict, Verdict::Other { name: "arduino-lib-builder".into() });
        assert_eq!(r.board("FAKE0").primary(), Some(Primary::Install));
        assert_eq!(r.board("FAKE0").card(), Card::Unknown(CardUnknown::NotOurs));
        let one_at_a_time = Refusal::OneAtATime { busy: "FAKE0".into() };
        assert_eq!(r.refused(), [(Some("FAKE1".into()), one_at_a_time)], "one board in its bootloader at a time");
        assert_eq!(r.resets(), ["open 921600", "let go"], "reset, read, restarted at once");
        r.send(Cmd::Read { port: "FAKE1".into() });
        r.until("FAKE1 read", |r| matches!(r.board("FAKE1").verdict, Verdict::NotCore2 { .. }));
        assert_eq!(r.board("FAKE1").verdict, Verdict::NotCore2 { found: "4 MB".into() });
        assert_eq!(r.board("FAKE1").primary(), None, "not a Core2: nothing offered");
        r.send(Cmd::Read { port: "FAKE2".into() });
        r.until("FAKE2 read and asked again", |r| {
            let b = r.board("FAKE2");
            b.probe.is_some() && b.work == Work::Idle
        });
        let ours = r.board("FAKE2");
        assert_eq!((ours.verdict.clone(), ours.version()), (Verdict::Update, Some("v0.7.0")), "ours, not running");
        assert_eq!(ours.card(), Card::Unknown(CardUnknown::NotRunning));
        r.send(Cmd::Read { port: "FAKE3".into() });
        r.until("FAKE3 read", |r| r.board("FAKE3").verdict == Verdict::Blank);
        assert_eq!(r.board("FAKE3").primary(), Some(Primary::Install));
        r.quit();
    }

    #[test]
    fn a_write_resets_writes_restarts_and_listens_again_for_the_new_verdict() {
        let fake = Fake::new("old:v0.7.0").with_pace(Duration::from_millis(100));
        let probe = fake.clone();
        let mut r = rig("write", fake, "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false) });
        r.until("written and heard", |r| {
            let b = r.board("FAKE0");
            matches!(b.written, Some(Written::Done { .. })) && b.work == Work::Idle && b.heard != Heard::Nothing
        });
        let board = r.board("FAKE0");
        assert_eq!(board.verdict, Verdict::UpToDate, "v0.8.0 says so through L");
        let Some(Written::Done { version, skipped: false, install: false, boot: Some(boot), .. }) = &board.written
        else {
            panic!("{:?}", board.written)
        };
        assert_eq!(version, "v0.8.0");
        assert!(boot.starts_with("mstream-mp3-player v0.8.0"), "{boot}");
        assert_eq!(probe.version_on("FAKE0").as_deref(), Some("v0.8.0"));
        let trace = ["listen FAKE0", "open 921600", "restart", "listen FAKE0"];
        assert_eq!(r.trace(), trace, "listened, reset once for the write, listened again");
        let plan = r.seen.iter().find_map(|e| match e {
            Event::Plan { plan, .. } => Some(plan.describe()),
            _ => None,
        });
        assert_eq!(plan.as_deref(), Some("update, no erase"));
        let pcts: Vec<u8> = r
            .seen
            .iter()
            .filter_map(|e| match e {
                Event::Board(Board { work: Work::Writing { pct: Some(p), .. }, .. }) => Some(*p),
                _ => None,
            })
            .collect();
        assert!(pcts.contains(&50) && pcts.contains(&100), "the board carries the write's percent: {pcts:?}");
        r.quit();
    }

    #[test]
    fn a_failed_write_leaves_the_board_half_written_with_try_again() {
        let mut r = rig("failed", Fake::new("failwrite").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: None });
        r.until("failed", |r| {
            matches!(r.board("FAKE0").written, Some(Written::Failed { .. })) && r.board("FAKE0").work == Work::Idle
        });
        let board = r.board("FAKE0");
        let failed = &board.written;
        assert!(matches!(failed, Some(Written::Failed { half: true, error: DeviceError::Link(_), .. })), "{failed:?}");
        assert_eq!(board.verdict, Verdict::HalfWritten);
        assert_eq!(board.primary(), Some(Primary::TryAgain));
        assert_eq!(board.card(), Card::Unknown(CardUnknown::HalfWritten));
        r.quit();
    }

    #[test]
    fn a_board_unplugged_mid_write_fails_half_written_and_then_goes() {
        let mut r = rig("unplug", Fake::new("old:v0.7.0/out=1.2").with_pace(Duration::from_millis(2500)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false) });
        r.until("gone", |r| r.seen.iter().any(|e| matches!(e, Event::Gone { .. })));
        let last = r.seen.iter().rev().find_map(|e| match e {
            Event::Board(b) => Some(b.clone()),
            _ => None,
        });
        let written = last.and_then(|b| b.written);
        assert!(matches!(written, Some(Written::Failed { half: true, error: DeviceError::Gone { .. }, .. })), "{written:?}");
        assert!(r.boards.is_empty());
        r.quit();
    }

    #[test]
    fn update_all_writes_each_board_that_needs_it_one_after_another_and_never_installs() {
        let fake = Fake::new("old:v0.7.0,status:v0.8.0,old:v0.6.0-37-g221d99d,other").with_pace(Duration::from_millis(60));
        let probe = fake.clone();
        let mut r = rig("all", fake, "v0.8.0");
        r.settled(4);
        let every: Vec<String> = (0..4).map(|i| format!("FAKE{i}")).collect();
        assert_eq!(r.board("FAKE2").verdict, Verdict::DevUpdate);
        r.send(Cmd::UpdateAll { ports: every });
        r.until("all done", |r| r.all().iter().any(|a| matches!(a, All::Done { .. })));
        let both = vec!["FAKE0".to_string(), "FAKE2".to_string()];
        assert_eq!(
            r.all()[..2],
            [All::Running { ports: both.clone(), at: 0 }, All::Running { ports: both.clone(), at: 1 }],
            "the two that need it, in order"
        );
        assert!(matches!(&r.all()[2], All::Done { ports, passed, .. } if *ports == both && passed.is_empty()));
        let resets = ["open 921600", "restart", "open 921600", "restart"];
        assert_eq!(r.resets(), resets, "one board in its bootloader at a time");
        assert_eq!(probe.version_on("FAKE0").as_deref(), Some("v0.8.0"));
        assert_eq!(probe.version_on("FAKE2").as_deref(), Some("v0.8.0"));
        assert_eq!(probe.version_on("FAKE1").as_deref(), Some("v0.8.0"), "up to date: untouched");
        assert_eq!(probe.version_on("FAKE3"), None, "other firmware: never installed over");
        let plans: Vec<String> = r
            .seen
            .iter()
            .filter_map(|e| match e {
                Event::Plan { plan, .. } => Some(plan.describe()),
                _ => None,
            })
            .collect();
        assert_eq!(plans, ["update, no erase", "update, no erase"], "never an erase");
        r.quit();
    }

    #[test]
    fn update_all_passes_over_a_board_unplugged_before_its_turn() {
        // FAKE1 leaves while FAKE0 is written: after both were heard, before
        // its turn.
        let fake = Fake::new("old:v0.7.0,old:v0.6.0/out=1.2").with_pace(Duration::from_millis(2500));
        let mut r = rig("all-gone", fake, "v0.8.0");
        r.settled(2);
        r.send(Cmd::UpdateAll { ports: vec!["FAKE0".into(), "FAKE1".into()] });
        r.until("done", |r| r.all().iter().any(|a| matches!(a, All::Done { .. })));
        let done = r.all().last().cloned();
        let (both, gone) = (vec!["FAKE0".to_string(), "FAKE1".to_string()], vec!["FAKE1".to_string()]);
        assert!(matches!(done, Some(All::Done { ports, passed, .. }) if ports == both && passed == gone));
        assert_eq!(r.resets(), ["open 921600", "restart"], "only the board still there was written");
        r.quit();
    }

    #[test]
    fn update_all_stops_at_the_first_failure_and_never_starts_the_next() {
        let fake = Fake::new("old:v0.7.0/fail=write,old:v0.6.0").with_pace(Duration::from_millis(60));
        let probe = fake.clone();
        let mut r = rig("all-stop", fake, "v0.8.0");
        r.settled(2);
        r.send(Cmd::UpdateAll { ports: vec!["FAKE0".into(), "FAKE1".into()] });
        r.until("stopped", |r| r.all().iter().any(|a| matches!(a, All::Stopped { .. })));
        let stopped = r.all().last().cloned().unwrap();
        assert!(matches!(stopped, All::Stopped { at: 0, error: Some(DeviceError::Link(_)), .. }), "{stopped:?}");
        assert_eq!(probe.version_on("FAKE1").as_deref(), Some("v0.6.0"), "the next board was never started");
        assert!(!r.seen.iter().any(|e| matches!(e, Event::Plan { port, .. } if port == "FAKE1")));
        assert_eq!(r.board("FAKE0").verdict, Verdict::HalfWritten);
        r.quit();
    }

    #[test]
    fn update_all_lets_a_board_go_untouched_when_its_bootloader_shows_other_firmware() {
        // It said v0.7.0 over USB; its flash holds someone else's image.
        let fake = Fake::new("status:v0.7.0/flash=other/fw=v0.7.0");
        let mut r = rig("guard", fake, "v0.8.0");
        r.settled(1);
        assert_eq!(r.board("FAKE0").verdict, Verdict::Update);
        r.send(Cmd::UpdateAll { ports: vec!["FAKE0".into()] });
        r.until("stopped", |r| r.all().iter().any(|a| matches!(a, All::Stopped { .. })));
        assert!(matches!(r.all().last(), Some(All::Stopped { at: 0, error: None, .. })));
        assert_eq!(r.resets(), ["open 921600", "let go"], "read, then restarted as it was: nothing written");
        r.quit();
    }

    #[test]
    fn during_a_write_the_other_boards_are_looked_at_and_never_written() {
        // FAKE2 arrives while FAKE0 is written.
        let fake = Fake::new("old:v0.7.0,status:v0.9.0/free=?,status:v0.8.0/in=1.2").with_pace(Duration::from_millis(2500));
        let mut r = rig("look", fake, "v0.8.0");
        r.settled(2);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false) });
        r.until("writing", |r| matches!(r.board("FAKE0").work, Work::Writing { pct: Some(_), .. }));
        for cmd in [
            Cmd::Write { port: "FAKE1".into(), erase: Some(false) },
            Cmd::Count { port: "FAKE1".into() },
            Cmd::Identify { port: "FAKE1".into() },
            Cmd::Read { port: "FAKE1".into() },
            Cmd::UpdateAll { ports: vec!["FAKE1".into()] },
        ] {
            r.send(cmd);
        }
        r.send(Cmd::Listen { port: "FAKE1".into() });
        r.until("the arrival heard during the write", |r| {
            r.boards.get("FAKE2").is_some_and(|b| b.verdict == Verdict::UpToDate)
        });
        r.until("the write done", |r| matches!(r.board("FAKE0").written, Some(Written::Done { .. })));
        let busy = Refusal::OneAtATime { busy: "FAKE0".into() };
        let refused = r.refused();
        assert_eq!(refused.len(), 5, "{refused:?}");
        assert!(refused.iter().all(|(_, why)| *why == busy), "{refused:?}");
        // Which refusals answer a write: the page unlocks on those alone.
        let writes: Vec<bool> = r
            .seen
            .iter()
            .filter_map(|e| match e {
                Event::Refused { write, .. } => Some(*write),
                _ => None,
            })
            .collect();
        assert_eq!(writes, [true, false, false, false, true], "the Write and Update all, nothing else");
        let listens = r.trace().iter().filter(|t| *t == "listen FAKE1").count();
        assert_eq!(listens, 2, "looking is allowed: {:?}", r.trace());
        assert_eq!(r.trace().iter().filter(|t| t.starts_with("open")).count(), 1, "one write: {:?}", r.trace());
        r.quit();
    }

    #[test]
    fn the_count_reports_its_progress_and_free_bytes_and_says_why_it_would_not() {
        let fake = Fake::new("status:v0.9.0/free=?,status:v0.9.0/state=playing,old:v0.7.0")
            .with_pace(Duration::from_millis(100));
        let mut r = rig("count", fake, "v0.9.0");
        r.settled(3);
        assert!(matches!(r.board("FAKE0").card(), Card::Fat { free: Free::NotCounted, .. }));
        r.send(Cmd::Count { port: "FAKE0".into() });
        r.until("counted", |r| matches!(r.board("FAKE0").count, Count::Done { .. }));
        // The fake counts 60 % of its 63,864,569,856-byte card free.
        let free = 38_318_741_910;
        assert_eq!(r.board("FAKE0").count, Count::Done { free });
        assert!(matches!(r.board("FAKE0").card(), Card::Fat { free: Free::Bytes(n), .. } if n == free));
        let counting = |b: &Board| matches!(b.card(), Card::Fat { free: Free::Counting { pct: Some(_) }, .. });
        let pcts: Vec<u8> = r
            .seen
            .iter()
            .filter_map(|e| match e {
                Event::Board(b) if b.port() == "FAKE0" && counting(b) => match b.count {
                    Count::Running { pct } => Some(pct),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert!(pcts.contains(&50), "the percent as it goes, on the card's row: {pcts:?}");
        r.send(Cmd::Count { port: "FAKE1".into() });
        r.until("refused", |r| matches!(r.board("FAKE1").count, Count::Refused { .. }));
        assert_eq!(r.board("FAKE1").count, Count::Refused { why: CountWhy::Playing, part_way: false });
        r.send(Cmd::Count { port: "FAKE2".into() });
        r.until("not offered", |r| !r.refused().is_empty());
        assert_eq!(r.refused(), [(Some("FAKE2".into()), Refusal::NotAnswering)]);
        r.quit();
    }

    #[test]
    fn show_on_the_player_puts_the_port_on_its_screen_or_says_why_not() {
        let mut r = rig("identify", Fake::new("status:v0.9.0,status:v0.9.0/identify=ui,old:v0.8.0"), "v0.9.0");
        r.settled(3);
        let answers = |r: &Rig| {
            r.seen
                .iter()
                .filter_map(|e| match e {
                    Event::Identified { port, label, result } => Some((port.clone(), label.clone(), result.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        r.send(Cmd::Identify { port: "FAKE0".into() });
        r.until("shown", |r| answers(r).len() == 1);
        r.send(Cmd::Identify { port: "FAKE1".into() });
        r.until("refused", |r| answers(r).len() == 2);
        let both = [
            ("FAKE0".to_string(), "FAKE0".to_string(), Ok(())),
            ("FAKE1".to_string(), "FAKE1".to_string(), Err(IdentifyWhy::Ui)),
        ];
        assert_eq!(answers(&r), both);
        assert!(r.trace().contains(&"identify FAKE0 FAKE0".to_string()));
        r.send(Cmd::Identify { port: "FAKE2".into() });
        r.until("not offered to old firmware", |r| !r.refused().is_empty());
        assert_eq!(r.refused(), [(Some("FAKE2".into()), Refusal::NotAnswering)]);
        r.quit();
    }

    #[test]
    fn details_facts_come_from_l_without_a_reset() {
        let mut r = rig("facts", Fake::new("status:v0.9.0"), "v0.9.0");
        r.settled(1);
        assert_eq!(r.board("FAKE0").flash_mb, None, "@status says no flash size");
        r.send(Cmd::Facts { port: "FAKE0".into() });
        r.until("facts", |r| r.board("FAKE0").flash_mb == Some(16));
        assert!(r.resets().is_empty());
        r.quit();
    }

    #[test]
    fn quit_lets_every_listen_go_at_once_and_waits_for_a_read_to_restart_its_board() {
        // A silent board takes the whole of its wait: Quit cuts it short.
        let slow = Timing { at_rest: Duration::from_secs(3), ..QUICK };
        let image = image("quit", "v0.8.0");
        let fake = Fake::new("status:v0.8.0,silent");
        let (cmds, events) = spawn(Arc::new(fake), Source::Local(image.clone()), None, slow, false);
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        cmds.send(Cmd::Quit).unwrap();
        let rest: Vec<Event> = events.iter().collect();
        let _ = std::fs::remove_file(&image);
        assert_eq!(rest.last(), Some(&Event::Released));
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());

        // A read under way: the board is reached, read and restarted first.
        let fake = Fake::new("other").with_reach(Duration::from_millis(300));
        let mut r = rig("quit-read", fake, "v0.8.0");
        r.settled(1);
        r.send(Cmd::Read { port: "FAKE0".into() });
        let trace = r.trace.clone();
        let t0 = Instant::now();
        while !trace.lock().unwrap().iter().any(|t| t.starts_with("open")) {
            assert!(t0.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let r = r.quit();
        assert_eq!(r.resets(), ["open 921600", "let go"], "the board it reset was restarted before Released");
    }

    #[test]
    fn a_write_is_never_cut_by_a_quit() {
        let mut r = rig("quit-write", Fake::new("old:v0.7.0").with_pace(Duration::from_millis(300)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false) });
        r.until("writing", |r| matches!(r.board("FAKE0").work, Work::Writing { .. }));
        let r = r.quit();
        let done = r.seen.iter().position(|e| matches!(e, Event::Board(b) if matches!(b.written, Some(Written::Done { .. }))));
        assert!(done.is_some_and(|at| at < r.seen.len() - 1), "Done, then Released: {:?}", r.seen);
        let trace = ["listen FAKE0", "open 921600", "restart"];
        assert_eq!(r.trace(), trace, "written whole; no listen begun after the Quit");
    }

    #[test]
    fn the_page_gone_is_a_quit() {
        let image = image("gone", "v0.8.0");
        let fake = Fake::new("status:v0.8.0");
        let (cmds, events) = spawn(Arc::new(fake), Source::Local(image.clone()), None, QUICK, false);
        drop(cmds);
        let rest: Vec<Event> = events.iter().collect();
        let _ = std::fs::remove_file(&image);
        assert_eq!(rest.last(), Some(&Event::Released), "and then the worker ends");
    }

    #[test]
    fn with_the_firmware_first_no_board_is_asked_for_an_image_it_cannot_have() {
        let fake = Fake::new("status:v0.8.0");
        let trace = fake.trace();
        let missing = Source::Local("/nowhere/at/all.bin".into());
        let (cmds, events) = spawn(Arc::new(fake), missing, None, QUICK, true);
        let failed = events.iter().find(|e| matches!(e, Event::FirmwareFailed(_)));
        assert!(matches!(failed, Some(Event::FirmwareFailed(DeviceError::Firmware(_)))));
        std::thread::sleep(Duration::from_millis(200));
        assert!(trace.lock().unwrap().is_empty(), "no board looked at");
        drop(cmds);
    }

    #[test]
    fn a_named_port_is_the_only_board_and_an_unlisted_one_is_opened_as_named() {
        let mut r = rig_with("named", Fake::new("status:v0.8.0,old:v0.7.0"), "v0.8.0", Some("fake1"), false);
        r.settled(1);
        assert_eq!(r.boards.keys().collect::<Vec<_>>(), ["FAKE1"]);
        r.quit();
        let mut r = rig_with("bare", Fake::new("nodevice"), "v0.8.0", Some("COM9"), false);
        r.settled(1);
        assert_eq!(r.board("COM9").candidate.bridge, "?");
        assert_eq!(r.board("COM9").verdict, Verdict::Silent);
        r.quit();
    }

    #[test]
    fn no_board_says_so_with_the_other_ports_and_the_watch_goes_on() {
        let mut r = rig("none", Fake::new("status:v0.8.0/in=1.2"), "v0.8.0");
        r.until("watched", |r| r.seen.iter().any(|e| matches!(e, Event::Watch { .. })));
        let none = |e: &Event| matches!(e, Event::Watch { ports, others } if ports.is_empty() && others == &["FAKECOM1"]);
        assert!(r.seen.iter().any(none));
        r.settled(1);
        let one = |e: &Event| matches!(e, Event::Watch { ports, others } if ports == &["FAKE0"] && others.is_empty());
        assert!(r.seen.iter().any(one));
        r.quit();
    }

    #[test]
    fn a_card_the_player_cannot_read_and_no_card_are_said_as_such() {
        let fake = Fake::new("status:v0.9.0/card=exfat/free=?/tracks=-,status:v0.9.0/card=none/size=-/free=?/tracks=-");
        let mut r = rig("cards", fake, "v0.9.0");
        r.settled(2);
        assert_eq!(r.board("FAKE0").card(), Card::Foreign { kind: CardKind::ExFat, size: Some(63_864_569_856) });
        assert_eq!(r.board("FAKE1").card(), Card::Empty);
        r.quit();
    }
}
