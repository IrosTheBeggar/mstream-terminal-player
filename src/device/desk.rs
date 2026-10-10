//! Every board on the desk, on one worker thread: the MP3 Player tab's
//! (and `device flash --yes`'s) view of the Core2s plugged in. It watches
//! the ports every couple of seconds; each board that arrives is asked
//! over USB what it runs — `@status`, else `L` — with no reset, so its
//! music plays on and nothing about it is guessed; one that is ours and
//! starting up — busy listing its library, as after a write — is asked
//! again on the same port until it answers; a board that leaves is
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
//! Every board is measured against the pin, said at once ([`Event::Target`])
//! and never moved: a choice, or a flag, decides what the next write puts
//! on one board ([`Cmd::Choose`], board::Next), not whether a board is up
//! to date. The images are the worker's: the page's own (the flags' choice,
//! else the pin's QIO build) fetched as it starts, on a thread of its own,
//! so the boards are judged while it downloads; any other — the pin's DIO
//! build for a board that runs DIO, a release or a build chosen in the
//! Advanced options sheet — when something needs it. A write waits for its
//! image and never reaches a board's bootloader without it. The release
//! list is asked of GitHub only when the page asks ([`Cmd::Releases`]), and
//! kept for the visit.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use rust_i18n::t;

use super::DeviceError;
use super::board::{Board, By, Count, CountWhy, Free, Heard, Looping, Next, NextState, Probe, Verdict, Work, Written};
use super::engine::{BOOT_LISTEN, DeviceInfo, Engine, restart_loop};
use super::firmware::{AppDesc, Cached, Firmware, Image, ImageFacts, ListWhy, Mode, Origin, Place, Release, Supply};
use super::firmware::{Target, header_line, merged_only, pin_target, place};
use super::flow::{self, Kind, Phase, Plan, Stop};
use super::listen::{self, Asked, Facts, IdentifyWhy, Waits};
use super::ports::Candidate;

/// The engine the worker and its jobs share.
pub(crate) type Shared = Arc<dyn Engine>;

/// How long the release list is kept once GitHub sent it: the visit's,
/// unless the page stays open for long. A failure is never kept.
const LIST_KEEP: Duration = Duration::from_secs(15 * 60);
/// The ROM lines a restart that never reached the firmware puts in the log
/// before it says how many more there were.
const ROM_LOG: usize = 8;

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
    /// …after a write's restart that heard no boot line. With one, the
    /// board is starting up already, and is asked every `waits.again` until
    /// it answers (listen::ask_till_up).
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

/// What the worker starts with.
pub(crate) struct Setup {
    /// Where the images and the release list come from.
    pub supply: Arc<dyn Supply>,
    /// The flags' choice (`--release`, `--firmware`, `--flash-mode`): every
    /// board's next write, until it is written or reset. None: each board's
    /// own default, the pin in its own mode.
    pub preset: Option<Image>,
    /// `--flash-mode`, when the flags named one: the preset is written in
    /// it on every board. With none, a preset release (the pin's tag or
    /// another) follows each board's own mode where the release has that
    /// build, as Update ▸ does: `--release v0.7.0` keeps a board whose ELF
    /// says DIO on DIO, and never puts QIO back on it unasked.
    pub flags_mode: Option<Mode>,
    /// `--port`: that board alone, listed or not.
    pub port: Option<String>,
    pub timing: Timing,
    /// No board is looked at until the page's image is in hand — `--yes`,
    /// which writes whatever it finds, prints its lines in today's order
    /// and never touches a board for an image it cannot have.
    pub firmware_first: bool,
}

/// A next write, as the sheet's Apply sends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    pub image: Image,
    /// Erase the whole flash first (the sheet's Erase box).
    pub erase: bool,
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
    /// Write the board's next image (board::Board::write_image: a choice,
    /// else the pin in its own mode) — after the gate's yes. `erase` as the
    /// gate left it; none lets the choice's Erase box, else the board,
    /// decide once its bootloader is read (erase over anything that is not
    /// ours — `--yes` with no erase flag). Waits for the image, and refused
    /// while another board is in its bootloader. A local build is read
    /// again first: changed since it was picked, the write is refused
    /// ([`Refusal::Changed`]) and the board carries the new one. `image`:
    /// the one the gate named. A board whose next write is another by the
    /// time the yes is read — a choice or a Reset sent just before, which
    /// the page's card had not shown yet, or a mode a listen brought — is
    /// refused ([`Refusal::Moved`]) with nothing touched. None (`--yes`,
    /// which has no gate) writes whatever the next write is.
    Write { port: String, erase: Option<bool>, image: Option<Image> },
    /// Update all: every one of `ports` that still needs an update, one
    /// after another, each with the pin in its own mode (or the one chosen
    /// for it), never erasing and never over anything but an older release
    /// of ours; stops at the first failure. A board whose next write is
    /// another release or a local build is left out, and logged by name.
    UpdateAll { ports: Vec<String> },
    /// The Advanced options sheet's Apply: the board's next write, one
    /// write long — or, with none (Reset, Use defaults), its defaults
    /// again. The image is had at once: a release downloaded, a local build
    /// read and vetted (merged images only), and the board told whole as it
    /// goes. Refused while the board is being written.
    Choose { port: String, choice: Option<Choice> },
    /// What a local path holds, for the sheet before Apply: read and
    /// vetted on a thread, answered by [`Event::Vetted`].
    Vet { path: PathBuf },
    /// The release list, for the sheet's Another release: one request to
    /// GitHub, then kept for the visit; answered by [`Event::Releases`].
    /// Asked again while a request is out, the one answer serves both.
    /// (`device releases` asks the supply itself: no worker, no port.)
    Releases,
    /// Leave: listens let go at once, a read finishes and restarts its
    /// board, a write is never cut; then Released.
    Quit,
}

/// What the worker tells the page, in the order things happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// What every board is measured against: the pin, said first, never
    /// moved.
    Target(Target),
    /// The page's own image downloading (the flags' choice, else the pin's
    /// QIO build): bytes so far, and the total when the server said.
    Download { done: u64, total: Option<u64> },
    /// The page's own image ready to write.
    Firmware { version: String, origin: String, bytes: usize, kind: Origin, mode: Option<Mode> },
    /// The page's own image could not be had: a write that needs it cannot
    /// start (the boards are read all the same; a write asked for later
    /// tries again).
    FirmwareFailed(DeviceError),
    /// Any image the worker gets — the page's own, the pin's other build,
    /// a release or build chosen for a board — as it comes.
    Image { image: Image, state: ImageState },
    /// What a local path holds ([`Cmd::Vet`]): its facts, or why the sheet
    /// cannot take it.
    Vetted { path: PathBuf, result: Result<ImageFacts, DeviceError> },
    /// The release list ([`Cmd::Releases`]), or why not with the releases
    /// this computer can write offline.
    Releases(Result<ReleaseList, ListFailed>),
    /// The watch looked: the boards' ports in the OS's order, and the
    /// other serial ports (the no-player card's "ports seen"). Said after
    /// the first look and whenever either changed.
    Watch { ports: Vec<String>, others: Vec<String> },
    /// A board, whole, as it is now: arrived, heard, working, written,
    /// chosen for.
    Board(Board),
    /// A board unplugged (or, mid-write, gone once its job ended).
    Gone { port: String },
    /// A write reached the board's bootloader and read it: what it found,
    /// what it will do, and the image it writes — for the log and
    /// `--yes`'s lines.
    Plan { port: String, info: DeviceInfo, on_board: Option<AppDesc>, plan: Plan, image: ImageFacts },
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

/// An image as the worker has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImageState {
    /// Being read or downloaded.
    Getting { done: u64, total: Option<u64> },
    Ready(ImageFacts),
    Failed(DeviceError),
}

/// GitHub's list, and when it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReleaseList {
    pub releases: Vec<Release>,
    pub asked: SystemTime,
}

/// The list did not come: why, and what can be written without it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListFailed {
    pub why: ListWhy,
    pub cached: Vec<Cached>,
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
    /// The local build chosen changed since it was picked — rebuilt, or
    /// replaced: the board carries the new one, and nothing was written
    /// until the gate has shown it.
    Changed { was: String, now: String },
    /// The board's next write is not the image the gate named: it moved
    /// after the page drew the gate. The page now has the board as it is.
    Moved,
    /// The image chosen is one the page does not write (an app alone).
    Image(DeviceError),
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
pub(crate) fn spawn(engine: Shared, setup: Setup) -> (Sender<Cmd>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = channel();
    let (event_tx, event_rx) = channel();
    std::thread::Builder::new()
        .name("mstream-desk".to_string())
        .spawn(move || run(engine, setup, &cmd_rx, &event_tx))
        .expect("spawn the device worker");
    (cmd_tx, event_rx)
}

/// What the worker's jobs tell it.
enum Note {
    Log { port: String, text: String, kind: LogKind },
    Work { port: String, id: u64, work: Work },
    /// A listen found the board starting up, and asks on.
    Starting { port: String, id: u64, heard: Heard },
    Counted { port: String, id: u64, pct: u8 },
    Plan { port: String, id: u64, info: DeviceInfo, on_board: Option<AppDesc>, plan: Plan, image: ImageFacts },
    Ended { port: String, id: u64, end: End },
    Progress { image: Image, done: u64, total: Option<u64> },
    Fetched { image: Image, result: Result<Firmware, DeviceError> },
    Listed(Result<ReleaseList, ListFailed>),
}

/// How a job ended.
enum End {
    Heard(Asked),
    Facts(Result<Facts, DeviceError>),
    Read(Result<Probe, DeviceError>),
    Count(Result<u64, (CountWhy, bool)>),
    Identify { label: String, result: Result<(), IdentifyWhy> },
    Written(Box<Result<Done, Failure>>),
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
    image: ImageFacts,
    looping: Option<Looping>,
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
#[derive(Clone)]
struct Queued {
    erase: Option<bool>,
    /// Update all's: only over an older release of ours.
    guard: bool,
    image: Image,
}

struct Slot {
    board: Board,
    job: Option<Job>,
    /// In the last look at the ports. A board gone with a write or a read
    /// under way is kept until its job ends.
    present: bool,
    queued: Option<Queued>,
}

/// An image in the worker's hands. One that could not be had is asked for
/// again by the next thing that needs it (its reason went to the page).
enum Fw {
    Getting { done: u64, total: Option<u64> },
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
    supply: Arc<dyn Supply>,
    preset: Option<Image>,
    flags_mode: Option<Mode>,
    /// The flags' choice, else the pin's QIO build: fetched first, and what
    /// Download, Firmware and FirmwareFailed are about.
    page_image: Image,
    scope: Option<String>,
    timing: Timing,
    events: Sender<Event>,
    notes_tx: Sender<Note>,
    notes: Receiver<Note>,
    target: Option<Target>,
    images: HashMap<Image, Fw>,
    /// GitHub's list, kept for the visit, and a request out for it.
    listed: Option<(Instant, ReleaseList)>,
    listing: bool,
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
pub(crate) fn run(engine: Shared, setup: Setup, cmds: &Receiver<Cmd>, events: &Sender<Event>) {
    let (notes_tx, notes) = channel();
    let page_image = setup.preset.clone().unwrap_or(Image::Pin(Mode::Qio));
    let firmware_first = setup.firmware_first;
    let mut desk = Desk {
        engine,
        supply: setup.supply,
        preset: setup.preset,
        flags_mode: setup.flags_mode,
        page_image,
        scope: setup.port.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
        timing: setup.timing,
        events: events.clone(),
        notes_tx,
        notes,
        target: pin_target(),
        images: HashMap::new(),
        listed: None,
        listing: false,
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
    desk.fetch(desk.page_image.clone());
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
        let may_look = !firmware_first || matches!(desk.images.get(&desk.page_image), Some(Fw::Ready(_)));
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

    /// The board on `port` judged again and told whole. A board that runs
    /// DIO and is behind the pin has the pin's DIO build fetched for it
    /// now, so Update's yes rarely waits for it.
    fn show(&mut self, i: usize) {
        self.follow_flags(i);
        let target = self.target.clone();
        let board = &mut self.slots[i].board;
        board.verdict = board.judge(target.as_ref());
        let board = board.clone();
        let own = board.default_image();
        if board.pending().is_none() && board.needs_update() && !self.images.contains_key(&own) {
            self.fetch(own);
        }
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

    // ── The images ──────────────────────────────────────────────────────────

    /// An image, had on a thread of its own: a download the first time, the
    /// cache after, a file's read. Asked for while it comes, it comes once.
    fn fetch(&mut self, image: Image) {
        if matches!(self.images.get(&image), Some(Fw::Getting { .. })) {
            return;
        }
        self.images.insert(image.clone(), Fw::Getting { done: 0, total: None });
        self.log(None, Phase::Firmware.text(), LogKind::Phase);
        self.tell(Event::Image { image: image.clone(), state: ImageState::Getting { done: 0, total: None } });
        let (supply, notes) = (self.supply.clone(), self.notes_tx.clone());
        std::thread::Builder::new()
            .name("mstream-firmware".to_string())
            .spawn(move || {
                let result = supply.resolve(&image, &mut |done, total| {
                    let _ = notes.send(Note::Progress { image: image.clone(), done, total });
                });
                let _ = notes.send(Note::Fetched { image, result });
            })
            .expect("spawn the firmware fetch");
    }

    /// A download moved: the page hears it, and a board waiting for this
    /// image hears it at each whole percent.
    fn progress(&mut self, image: Image, done: u64, total: Option<u64>) {
        if !matches!(self.images.get(&image), Some(Fw::Getting { .. })) {
            return;
        }
        self.images.insert(image.clone(), Fw::Getting { done, total });
        if image == self.page_image {
            self.tell(Event::Download { done, total });
        }
        self.tell(Event::Image { image: image.clone(), state: ImageState::Getting { done, total } });
        let pct = |done: u64, total: Option<u64>| total.filter(|t| *t > 0).map(|t| done * 100 / t);
        for i in 0..self.slots.len() {
            let Some(next) = &mut self.slots[i].board.next else { continue };
            if next.image != image {
                continue;
            }
            let moved = match next.state {
                NextState::Getting { done: was, total: had } => pct(was, had) != pct(done, total),
                _ => false,
            };
            if moved {
                next.state = NextState::Getting { done, total };
                self.show(i);
            }
        }
    }

    fn fetched(&mut self, image: Image, result: Result<Firmware, DeviceError>) {
        match result {
            Ok(firmware) => {
                let facts = firmware.facts.clone();
                self.log(None, flow::firmware_line(&facts), LogKind::Fact);
                if let Some(line) = header_line(&facts) {
                    self.log(None, line, LogKind::Quiet);
                }
                self.images.insert(image.clone(), Fw::Ready(Arc::new(firmware)));
                if image == self.page_image {
                    self.tell(Event::Firmware {
                        version: facts.version.clone(),
                        origin: facts.origin.clone(),
                        bytes: facts.bytes,
                        kind: facts.kind,
                        mode: facts.mode,
                    });
                }
                self.tell(Event::Image { image: image.clone(), state: ImageState::Ready(facts.clone()) });
                for i in 0..self.slots.len() {
                    if let Some(next) = &mut self.slots[i].board.next
                        && next.image == image
                    {
                        next.state = next_state(&facts, next.by);
                        self.show(i);
                    }
                }
            }
            Err(e) => {
                self.images.insert(image.clone(), Fw::Failed);
                self.log(None, e.text(), LogKind::Fail);
                if image == self.page_image {
                    self.tell(Event::FirmwareFailed(e.clone()));
                }
                self.tell(Event::Image { image: image.clone(), state: ImageState::Failed(e.clone()) });
                // The writes that waited for it fail with its reason: no
                // board was touched.
                let current = self.all.as_ref().and_then(|run| run.ports.get(run.at).cloned());
                let mut all_failed = false;
                for i in 0..self.slots.len() {
                    let mut changed = false;
                    if let Some(next) = &mut self.slots[i].board.next
                        && next.image == image
                    {
                        next.state = NextState::Failed(e.clone());
                        changed = true;
                    }
                    if self.slots[i].queued.as_ref().is_some_and(|q| q.image == image) {
                        self.slots[i].queued = None;
                        let board = &mut self.slots[i].board;
                        board.work = Work::Idle;
                        board.written = Some(Written::Failed { error: e.clone(), half: false, pct: None });
                        all_failed |= current.as_deref() == Some(board.port());
                        changed = true;
                    }
                    if changed {
                        self.show(i);
                    }
                }
                if all_failed {
                    self.all_step(false, Some(e));
                }
            }
        }
    }

    // ── The choices ─────────────────────────────────────────────────────────

    /// Board `i`'s next write: `image` — or, when it is the board's default
    /// and nothing else is asked (no erase, not the flags'), none at all.
    /// The image is had now: a local build the sheet applies read again, a
    /// release downloaded, one in hand at once.
    fn choose(&mut self, i: usize, image: Image, erase: bool, by: By) {
        let board = &self.slots[i].board;
        if !erase && by != By::Flags && image == board.default_image() {
            self.slots[i].board.next = None;
            return;
        }
        // A file may have been rebuilt since it was last read. The flags'
        // was read as the worker started, and every yes reads it again.
        let coming = matches!(self.images.get(&image), Some(Fw::Getting { .. }));
        if by == By::Sheet && image.is_local() && !coming {
            self.images.remove(&image);
        }
        let state = match self.images.get(&image) {
            Some(Fw::Ready(firmware)) => next_state(&firmware.facts, by),
            Some(Fw::Getting { done, total }) => NextState::Getting { done: *done, total: *total },
            Some(Fw::Failed) | None => {
                self.fetch(image.clone());
                NextState::Getting { done: 0, total: None }
            }
        };
        self.slots[i].board.next = Some(Next { image, erase, by, state });
    }

    /// The flags' choice for `board`: as given where `--flash-mode` named a
    /// mode, or for a file (whose mode is its own); else the release in the
    /// board's own mode wherever it has that build. A board's mode is read
    /// from the ELF its listen brings, after the flags chose on its arrival.
    fn flags_image(&self, board: &Board) -> Option<Image> {
        let preset = self.preset.clone()?;
        if self.flags_mode.is_some() {
            return Some(preset);
        }
        let own = board.default_mode();
        Some(match preset.in_mode(own) {
            Some(image) if image.has(own) != Some(false) => image,
            _ => preset,
        })
    }

    /// Board `i`'s flags' choice kept in step with what is known of it, so
    /// the gate, `--yes`'s plan and the write all name the build the board
    /// will get. Never while it is being written.
    fn follow_flags(&mut self, i: usize) {
        let slot = &self.slots[i];
        let writing = slot.queued.is_some() || slot.job.as_ref().is_some_and(|j| j.kind == JobKind::Write);
        let Some(next) = slot.board.next.as_ref().filter(|n| n.by == By::Flags && !writing) else { return };
        let erase = next.erase;
        match self.flags_image(&slot.board) {
            Some(wanted) if wanted != next.image => self.choose(i, wanted, erase, By::Flags),
            _ => {}
        }
    }

    // ── The release list ────────────────────────────────────────────────────

    /// The list: the one kept for the visit, else one request on a thread
    /// of its own. A failure says why, with what this computer has.
    fn releases(&mut self) {
        if let Some((at, list)) = &self.listed
            && at.elapsed() < LIST_KEEP
        {
            return self.tell(Event::Releases(Ok(list.clone())));
        }
        if self.listing {
            return;
        }
        self.listing = true;
        let (supply, notes) = (self.supply.clone(), self.notes_tx.clone());
        std::thread::Builder::new()
            .name("mstream-releases".to_string())
            .spawn(move || {
                let result = match supply.releases() {
                    Ok(releases) => Ok(ReleaseList { releases, asked: SystemTime::now() }),
                    Err(why) => Err(ListFailed { why, cached: supply.cached() }),
                };
                let _ = notes.send(Note::Listed(result));
            })
            .expect("spawn the release list");
    }

    fn listed(&mut self, result: Result<ReleaseList, ListFailed>) {
        self.listing = false;
        match &result {
            Ok(list) => {
                self.log(None, t!("dev.log_releases", n = list.releases.len()).to_string(), LogKind::Fact);
                self.listed = Some((Instant::now(), list.clone()));
            }
            Err(failed) => {
                let mut line = failed.why.text();
                if let Some(detail) = failed.why.detail() {
                    line.push_str(&format!(" ({detail})"));
                }
                self.log(None, line, LogKind::Fail);
            }
        }
        self.tell(Event::Releases(result));
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
            // The flags' choice is every board's next write, each its own
            // to write or reset.
            if let Some(preset) = self.preset.clone() {
                self.choose(at, preset, false, By::Flags);
            }
            self.show(at);
            self.listen(at, wait, None);
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
                    None => self.listen(i, self.timing.at_rest, None),
                }
            }
            Cmd::Facts { port } => self.light(&port, JobKind::Facts),
            Cmd::Read { port } => self.light(&port, JobKind::Read),
            Cmd::Count { port } => self.light(&port, JobKind::Count),
            Cmd::Identify { port } => self.light(&port, JobKind::Identify),
            Cmd::Write { port, erase, image: named } => {
                let Some(i) = self.board_for(&port, true) else { return };
                if let Some(busy) = self.in_bootloader().filter(|b| !b.eq_ignore_ascii_case(&port)) {
                    let busy = busy.to_string();
                    return self.refuse(&port, Refusal::OneAtATime { busy }, true);
                }
                if self.all.is_some() || self.slots[i].job.as_ref().is_some_and(|j| j.kind.bootloader()) {
                    return self.refuse(&port, Refusal::Busy, true);
                }
                let board = &self.slots[i].board;
                let image = board.write_image();
                let erase = erase.or_else(|| board.pending().filter(|next| next.erase).map(|_| true));
                if named.as_ref().is_some_and(|named| *named != image) {
                    self.show(i);
                    return self.refuse(&port, Refusal::Moved, true);
                }
                if image.is_local() && !self.read_again(i, &image) {
                    return;
                }
                self.queue(i, Queued { erase, guard: false, image });
            }
            Cmd::UpdateAll { ports } => {
                if let Some(busy) = self.in_bootloader() {
                    let busy = busy.to_string();
                    let why = Refusal::OneAtATime { busy };
                    return self.tell(Event::Refused { port: None, why, write: true });
                }
                let named: Vec<usize> =
                    ports.iter().filter_map(|p| self.find(p)).filter(|i| self.slots[*i].present).collect();
                for i in named.iter().copied().filter(|i| self.slots[*i].board.left_out()) {
                    let board = &self.slots[i].board;
                    let what = board.pending().map(next_words).unwrap_or_default();
                    let line = t!("dev.log_all_left", port = board.port(), what = what).to_string();
                    self.log(Some(board.port()), line, LogKind::Fact);
                }
                let wanted: Vec<String> = named
                    .into_iter()
                    .filter(|i| self.slots[*i].board.in_update_all())
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
            Cmd::Choose { port, choice } => {
                let Some(i) = self.board_for(&port, false) else { return };
                let writing = self.slots[i].queued.is_some()
                    || self.slots[i].job.as_ref().is_some_and(|j| j.kind == JobKind::Write);
                if writing {
                    return self.refuse(&port, Refusal::Busy, false);
                }
                match choice {
                    Some(choice) => self.choose(i, choice.image, choice.erase, By::Sheet),
                    None => self.slots[i].board.next = None,
                }
                self.show(i);
            }
            Cmd::Vet { path } => self.vet(path),
            Cmd::Releases => self.releases(),
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

    /// The gate's yes on a local build: the file read again here — a
    /// build folder may have been rebuilt since it was picked — before the
    /// write is queued. A different version or ELF than the gate showed is
    /// refused, with the board carrying the new facts for the gate to show;
    /// a file that cannot be read now fails the write with nothing touched.
    fn read_again(&mut self, i: usize, image: &Image) -> bool {
        let port = self.slots[i].board.port().to_string();
        let shown = self.slots[i].board.pending().and_then(Next::facts);
        let shown = shown.map(|f| (f.version.clone(), f.elf.clone()));
        let result = self.supply.resolve(image, &mut |_, _| {});
        let firmware = match result {
            Ok(firmware) => firmware,
            Err(_) => {
                // Unreadable now: the queued write reads it once more and
                // fails with the reason, nothing touched.
                self.images.insert(image.clone(), Fw::Failed);
                return true;
            }
        };
        let facts = firmware.facts.clone();
        let by = self.slots[i].board.pending().map_or(By::Flags, |next| next.by);
        if let Some(next) = &mut self.slots[i].board.next
            && next.image == *image
        {
            next.state = next_state(&facts, by);
        }
        if by == By::Sheet
            && let Err(e) = merged_only(&facts)
        {
            self.show(i);
            self.refuse(&port, Refusal::Image(e), true);
            return false;
        }
        self.images.insert(image.clone(), Fw::Ready(Arc::new(firmware)));
        match shown {
            Some((version, elf)) if version != facts.version || elf != facts.elf => {
                let line = t!("dev.log_changed", port = port, was = version, now = facts.version).to_string();
                self.log(Some(&port), line, LogKind::Fail);
                self.show(i);
                self.refuse(&port, Refusal::Changed { was: version, now: facts.version }, true);
                false
            }
            _ => true,
        }
    }

    /// A local path read for the sheet, on a thread: its facts, or why the
    /// tab will not take it.
    fn vet(&mut self, path: PathBuf) {
        let (supply, events) = (self.supply.clone(), self.events.clone());
        std::thread::Builder::new()
            .name("mstream-vet".to_string())
            .spawn(move || {
                let result = supply.resolve(&Image::Local(path.clone()), &mut |_, _| {}).and_then(|firmware| {
                    merged_only(&firmware.facts)?;
                    Ok(firmware.facts)
                });
                let line = match &result {
                    Ok(facts) => (flow::local_line(facts), LogKind::Fact),
                    Err(e) => (e.text(), LogKind::Fail),
                };
                let _ = events.send(Event::Log { port: None, text: line.0, kind: line.1 });
                let _ = events.send(Event::Vetted { path, result });
            })
            .expect("spawn the vet");
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
        let image = queued.image.clone();
        self.slots[i].queued = Some(queued);
        self.slots[i].board.work = Work::Queued;
        self.show(i);
        if matches!(self.images.get(&image), Some(Fw::Failed) | None) {
            self.fetch(image);
        }
    }

    /// Start the write that waits, once its board is free and its image is
    /// in hand.
    fn start_queued(&mut self) {
        if self.quitting {
            return;
        }
        let ready = |desk: &Desk, slot: &Slot| {
            slot.job.is_none()
                && slot.queued.as_ref().is_some_and(|q| matches!(desk.images.get(&q.image), Some(Fw::Ready(_))))
        };
        let Some(i) = self.slots.iter().position(|s| ready(self, s)) else { return };
        let queued = self.slots[i].queued.take().expect("queued");
        let Some(Fw::Ready(firmware)) = self.images.get(&queued.image) else { return };
        let firmware = firmware.clone();
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

    /// Ask board `i` what it runs, giving it `wait` to answer — and while
    /// it is ours and starting up, again and again on the same port until
    /// it answers (listen::ask_till_up), the board told meanwhile. `since`:
    /// the board is starting up already (the write's boot line).
    fn listen(&mut self, i: usize, wait: Duration, since: Option<Heard>) {
        let id = self.job(i, JobKind::Listen);
        let stop = self.slots[i].job.as_ref().expect("just made").stop.clone();
        self.slots[i].board.work = Work::Listening;
        self.show(i);
        let (engine, notes, waits) = (self.engine.clone(), self.notes_tx.clone(), self.timing.waits);
        let candidate = self.slots[i].board.candidate.clone();
        std::thread::spawn(move || {
            let port = candidate.port.clone();
            let asked = listen_with(&*engine, &candidate, &notes, |wire, note| {
                let mut told = |heard: &Heard| {
                    let _ = notes.send(Note::Starting { port: port.clone(), id, heard: heard.clone() });
                };
                listen::ask_till_up(wire, &port, wait, waits, since, &stop, note, &mut told)
            });
            let _ = notes.send(Note::Ended { port, id, end: End::Heard(asked) });
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
            Note::Progress { image, done, total } => self.progress(image, done, total),
            Note::Fetched { image, result } => self.fetched(image, result),
            Note::Listed(result) => self.listed(result),
            Note::Work { port, id, work } => {
                if let Some(i) = mine(self, &port, id) {
                    self.slots[i].board.work = work;
                    self.show(i);
                }
            }
            // Said once a listen: the card says why it waits, in place of
            // "said nothing".
            Note::Starting { port, id, heard } => {
                if let Some(i) = mine(self, &port, id) {
                    let board = &mut self.slots[i].board;
                    let first = !matches!(board.heard, Heard::Starting { .. });
                    runs_again(board);
                    board.heard = heard;
                    if first {
                        self.log(Some(&port), t!("dev.log_starting", port = port).to_string(), LogKind::Fact);
                    }
                    self.show(i);
                }
            }
            Note::Counted { port, id, pct } => {
                if let Some(i) = mine(self, &port, id) {
                    self.slots[i].board.count = Count::Running { pct };
                    self.show(i);
                }
            }
            Note::Plan { port, id, info, on_board, plan, image } => {
                if let Some(i) = mine(self, &port, id) {
                    let board = &mut self.slots[i].board;
                    board.flash_mb = info.flash_mb.or(board.flash_mb);
                    board.probe = Some(Ok(Probe { info: info.clone(), on_board: on_board.clone() }));
                    self.show(i);
                }
                self.tell(Event::Plan { port, info, on_board, plan, image });
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
                if matches!(asked.heard, Heard::Status(_) | Heard::Old { .. } | Heard::Starting { .. }) {
                    runs_again(board);
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
                    then_listen = Some((self.timing.arrival, None));
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
            End::Written(written) => match *written {
                Ok(done) => {
                    // What went on, as its own description says — the ELF
                    // the board will report, read from the image.
                    let written = AppDesc {
                        version: done.version.clone(),
                        project: AppDesc::OURS.to_string(),
                        idf: String::new(),
                        elf8: done.image.elf.clone(),
                    };
                    let cure = done.looping.and_then(|_| dio_cure(&done.image));
                    // Its boot line came: the new firmware is up, listing
                    // the card's library before it answers — v0.7.0 takes
                    // a minute and a half for a big one. Asked every few
                    // seconds till then, and the card says why it waits.
                    let since = done.boot.as_deref().filter(|_| done.looping.is_none());
                    let since = since.and_then(listen::parse_boot_line);
                    let since = since.map(|(version, elf)| Heard::Starting { version: Some(version), elf });
                    let wait = if since.is_some() { self.timing.waits.again } else { self.timing.after_write };
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
                        image: done.image,
                        looping: done.looping,
                    });
                    // One board, one write: the choice is spent.
                    board.next = None;
                    // It keeps restarting on the QIO build: the same
                    // version's DIO build is filled in as its next write —
                    // offered, never written by itself.
                    if let Some(cure) = cure {
                        self.choose(i, cure, false, By::Loop);
                    }
                    then_listen = Some((wait, since));
                    all = Some((true, None));
                }
                Err(Failure::Failed { error, half, pct }) => {
                    let board = &mut self.slots[i].board;
                    if half {
                        board.heard = Heard::Nothing;
                    }
                    // The choice stays: Try again writes the same image
                    // through the same gate.
                    board.written = Some(Written::Failed { error: error.clone(), half, pct });
                    self.log(Some(&port), error.text(), LogKind::Fail);
                    if let Some(hint) = error.hint() {
                        self.log(Some(&port), hint, LogKind::Quiet);
                    }
                    all = Some((false, Some(error)));
                }
                Err(Failure::NotOlder { found }) => {
                    let line = t!("dev.log_guard", port = port, found = found).to_string();
                    self.log(Some(&port), line, LogKind::Fail);
                    all = Some((false, None));
                }
            },
        }
        if !self.slots[i].present {
            // Unplugged while it was in its bootloader: told now, dropped.
            self.show(i);
            self.slots.remove(i);
            self.gone(&port);
        } else if let Some((wait, since)) = then_listen.filter(|_| !self.quitting) {
            self.listen(i, wait, since);
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

    /// Queue Update all's board at its place, with the pin in its own mode,
    /// passing over the ones that no longer need it (unplugged, written
    /// meanwhile); done past the last.
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
                slot.present
                    && matches!(slot.board.verdict, Verdict::Update | Verdict::DevUpdate)
                    && slot.board.pending().is_none_or(|next| next.image.is_pin())
            });
            match ready {
                Some(i) => {
                    let ports = run.ports.clone();
                    let image = self.slots[i].board.update_image();
                    self.queue(i, Queued { erase: Some(false), guard: true, image });
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

/// A next write's state once its image is in hand: the page writes a
/// merged image only, so an app alone chosen in the sheet is refused there
/// (the flags may write one: `--firmware` always has).
fn next_state(facts: &ImageFacts, by: By) -> NextState {
    match merged_only(facts) {
        Err(e) if by == By::Sheet => NextState::Failed(e),
        _ => NextState::Ready(facts.clone()),
    }
}

/// The cure for a board that keeps restarting on `facts`: the same
/// source's DIO build, when the image written was QIO and its source has
/// one (a local build has none the player knows).
pub(crate) fn dio_cure(facts: &ImageFacts) -> Option<Image> {
    if facts.mode != Some(Mode::Qio) {
        return None;
    }
    let image = facts.image.in_mode(Mode::Dio)?;
    (image.has(Mode::Dio) != Some(false)).then_some(image)
}

/// The board runs again — it answered, or it is ours coming up: whatever
/// went wrong is behind it — a failed write, or a restart loop (and the DIO
/// image the page offered for it).
fn runs_again(board: &mut Board) {
    if matches!(board.written, Some(Written::Failed { .. })) {
        board.written = None;
    }
    if let Some(Written::Done { looping: looping @ Some(_), .. }) = &mut board.written {
        *looping = None;
        if board.next.as_ref().is_some_and(|next| next.by == By::Loop) {
            board.next = None;
        }
    }
}

/// A next write in a few words, for the log: `v0.7.0 in QIO`, `a local
/// build`.
fn next_words(next: &Next) -> String {
    match (&next.image, next.version(), next.mode()) {
        (Image::Local(_), Some(version), _) => t!("dev.next_local", version = version).to_string(),
        (_, Some(version), Some(mode)) => format!("{version} {}", mode.word()),
        (_, Some(version), None) => version,
        _ => String::new(),
    }
}

/// The log's line for what a listen heard. A board on DIO says so; QIO,
/// the default, goes unsaid.
fn heard_line(board: &Board) -> String {
    let port = board.port();
    let dio = board.mode().is_some_and(|m| m.mode == Mode::Dio);
    match &board.heard {
        Heard::Status(status) if dio => {
            t!("dev.log_heard_mode", port = port, version = status.fw, mode = Mode::Dio.word()).to_string()
        }
        Heard::Status(status) => t!("dev.log_heard", port = port, version = status.fw).to_string(),
        Heard::Old { version: Some(version), .. } if dio => {
            t!("dev.log_heard_old_mode", port = port, version = version, mode = Mode::Dio.word()).to_string()
        }
        Heard::Old { version: Some(version), .. } => {
            t!("dev.log_heard_old", port = port, version = version).to_string()
        }
        Heard::Old { version: None, .. } => t!("dev.log_heard_old_unknown", port = port).to_string(),
        Heard::Starting { .. } => t!("dev.log_starting", port = port).to_string(),
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

/// Listen to one board with no reset, `ask` holding the conversation. A
/// port in use is that board's state; one that would not open, its
/// failure.
fn listen_with(
    engine: &dyn Engine,
    candidate: &Candidate,
    notes: &Sender<Note>,
    ask: impl FnOnce(&mut dyn listen::Wire, listen::Note) -> Result<Asked, DeviceError>,
) -> Asked {
    let port = candidate.port.clone();
    let _ = notes.send(Note::Log {
        port: port.clone(),
        text: t!("dev.log_listen", port = port).to_string(),
        kind: LogKind::Phase,
    });
    let heard = |heard: Heard| Asked { heard, flash_mb: None, boot: None };
    match talk(engine, candidate, notes, ask) {
        Ok(asked) => asked,
        Err(DeviceError::Busy { detail, .. }) => heard(Heard::InUse { detail }),
        Err(e) => heard(Heard::Failed(e)),
    }
}

/// Listen to one board once: `@status`, else `L`. A board starting up is
/// said as such, not waited for.
fn listen_one(
    engine: &dyn Engine,
    candidate: &Candidate,
    wait: Duration,
    waits: Waits,
    stop: &AtomicBool,
    notes: &Sender<Note>,
) -> Asked {
    let port = candidate.port.clone();
    listen_with(engine, candidate, notes, |wire, note| listen::ask(wire, &port, wait, waits, stop, note))
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
/// restart and what the board said after it — its first line, or the
/// ROM's banners again and again. Nothing stops it once it has begun.
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
        let _ = notes.send(Note::Ended { port: port.clone(), id, end: End::Written(Box::new(end)) });
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
    let facts = firmware.facts.clone();
    let plan = flow::plan(on_board.as_ref(), &facts.version, erase);
    let install = matches!(plan.kind, Kind::Install { .. });
    let _ = notes.send(Note::Plan {
        port: port.clone(),
        id,
        info: info.clone(),
        on_board: on_board.clone(),
        plan: plan.clone(),
        image: facts.clone(),
    });
    let what = format!("{} · {}", plan.describe(), flow::image_words(&facts));
    log(t!("dev.log_go", plan = what).to_string(), LogKind::Fact);
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
    let secs = BOOT_LISTEN.as_secs();
    log(t!("dev.log_restart", secs = secs).to_string(), LogKind::Fact);
    match link.restart() {
        Ok(restart) => {
            let looping = restart_loop(&restart).map(|restarts| Looping { restarts, secs });
            if let Some(line) = &restart.boot {
                log(line.clone(), LogKind::Quiet);
            } else {
                // No firmware line: what came instead is the evidence.
                for line in restart.rom.iter().take(ROM_LOG) {
                    log(line.clone(), LogKind::Quiet);
                }
                if restart.rom.len() > ROM_LOG {
                    log(t!("dev.log_rom_more", n = restart.rom.len() - ROM_LOG).to_string(), LogKind::Quiet);
                }
            }
            if let Some(looping) = looping {
                let key = if dio_cure(&facts).is_some() { "dev.log_loop" } else { "dev.log_loop_plain" };
                log(t!(key, secs = secs, n = looping.restarts).to_string(), LogKind::Fail);
            }
            let version = facts.version.clone();
            let boot = restart.boot;
            end(Ok(Done { version, took: since.elapsed(), skipped, install, boot, info, image: facts, looping }))
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
    use crate::device::firmware::Check;
    use crate::device::firmware::tests::{Shelf, desc_bytes, desc_with_elf, elf_bytes, merged_in};

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

    /// The worker's start on `supply`: no flag, every board.
    pub(crate) fn setup(supply: Arc<dyn Supply>) -> Setup {
        Setup { supply, preset: None, flags_mode: None, port: None, timing: QUICK, firmware_first: false }
    }

    /// A worker on `fake`, the pin's two builds of `version` on the test's
    /// shelf in GitHub's place, its two far ends the test's, and what it
    /// said folded as it comes.
    struct Rig {
        cmds: Sender<Cmd>,
        events: Receiver<Event>,
        boards: BTreeMap<String, Board>,
        seen: Vec<Event>,
        trace: Arc<Mutex<Vec<String>>>,
        shelf: Arc<Shelf>,
    }

    fn rig(fake: Fake, version: &str) -> Rig {
        rig_with(fake, Shelf::new(version), None, None, false)
    }

    fn rig_with(fake: Fake, shelf: Shelf, preset: Option<Image>, port: Option<&str>, first: bool) -> Rig {
        let trace = fake.trace();
        let shelf = Arc::new(shelf);
        let setup = Setup {
            supply: shelf.clone(),
            preset,
            flags_mode: None,
            port: port.map(str::to_string),
            timing: QUICK,
            firmware_first: first,
        };
        rig_on(fake, setup, shelf, trace)
    }

    /// A worker on `fake` with the flags `device flash` would hand it:
    /// `--release` and `--flash-mode`, read as the command line reads them.
    fn rig_flags(fake: Fake, release: Option<&str>, mode: Option<Mode>) -> Rig {
        let trace = fake.trace();
        let shelf = Arc::new(Shelf::new("v0.8.0"));
        let preset = Image::from_flags(None, release.map(str::to_string), mode);
        let setup = Setup {
            supply: shelf.clone(),
            preset,
            flags_mode: mode,
            port: None,
            timing: QUICK,
            firmware_first: false,
        };
        rig_on(fake, setup, shelf, trace)
    }

    fn rig_on(fake: Fake, setup: Setup, shelf: Arc<Shelf>, trace: Arc<Mutex<Vec<String>>>) -> Rig {
        let (cmds, events) = spawn(Arc::new(fake), setup);
        Rig { cmds, events, boards: BTreeMap::new(), seen: Vec::new(), trace, shelf }
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

        /// The sheet's Apply (or Reset, with none), read on until the board
        /// carries it: its image in hand, or why not.
        fn choose(&mut self, port: &str, image: Option<Image>) {
            let before = self.seen.len();
            let choice = image.map(|image| Choice { image, erase: false });
            self.send(Cmd::Choose { port: port.into(), choice });
            self.until("the choice carried", |r| {
                r.seen[before..].iter().any(|e| match e {
                    Event::Board(b) if b.port() == port => {
                        b.next.as_ref().is_none_or(|n| !matches!(n.state, NextState::Getting { .. }))
                    }
                    _ => false,
                })
            });
        }

        fn written(&mut self, port: &str, what: &str) {
            self.until(what, |r| {
                let b = r.board(port);
                matches!(b.written, Some(Written::Done { .. })) && b.work == Work::Idle && b.heard != Heard::Nothing
            });
        }

        fn plans(&self) -> Vec<ImageFacts> {
            self.seen
                .iter()
                .filter_map(|e| match e {
                    Event::Plan { image, .. } => Some(image.clone()),
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
        let mut r = rig(Fake::new("status:v0.8.0,old:v0.7.0,silent:v0.7.0"), "v0.8.0");
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
        assert!(r.seen.iter().any(|e| matches!(e, Event::Target(t) if t.version == "v0.8.0")), "the pin");
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
        let mut r = rig(fake, "v0.8.0");
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
        let mut r = rig(Fake::new("status:v0.8.0,status:v0.8.0/held=0.5"), "v0.8.0");
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
        let mut r = rig(Fake::new("other,chip,silent:v0.7.0,fresh"), "v0.8.0");
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
        let mut r = rig(fake, "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
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
        let mut r = rig(Fake::new("failwrite").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: None, image: None });
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
        let mut r = rig(Fake::new("old:v0.7.0/out=1.2").with_pace(Duration::from_millis(2500)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
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
        let mut r = rig(fake, "v0.8.0");
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
        let mut r = rig(fake, "v0.8.0");
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
        let mut r = rig(fake, "v0.8.0");
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
        let mut r = rig(fake, "v0.8.0");
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
        let mut r = rig(fake, "v0.8.0");
        r.settled(2);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("writing", |r| matches!(r.board("FAKE0").work, Work::Writing { pct: Some(_), .. }));
        for cmd in [
            Cmd::Write { port: "FAKE1".into(), erase: Some(false), image: None },
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
        let mut r = rig(fake, "v0.9.0");
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
        let mut r = rig(Fake::new("status:v0.9.0,status:v0.9.0/identify=ui,old:v0.8.0"), "v0.9.0");
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
        let mut r = rig(Fake::new("status:v0.9.0"), "v0.9.0");
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
        let timing = Timing { at_rest: Duration::from_secs(3), ..QUICK };
        let slow = Setup { timing, ..setup(Arc::new(Shelf::new("v0.8.0"))) };
        let fake = Fake::new("status:v0.8.0,silent");
        let (cmds, events) = spawn(Arc::new(fake), slow);
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        cmds.send(Cmd::Quit).unwrap();
        let rest: Vec<Event> = events.iter().collect();
        assert_eq!(rest.last(), Some(&Event::Released));
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());

        // A read under way: the board is reached, read and restarted first.
        let fake = Fake::new("other").with_reach(Duration::from_millis(300));
        let mut r = rig(fake, "v0.8.0");
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
        let mut r = rig(Fake::new("old:v0.7.0").with_pace(Duration::from_millis(300)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("writing", |r| matches!(r.board("FAKE0").work, Work::Writing { .. }));
        let r = r.quit();
        let done = r.seen.iter().position(|e| matches!(e, Event::Board(b) if matches!(b.written, Some(Written::Done { .. }))));
        assert!(done.is_some_and(|at| at < r.seen.len() - 1), "Done, then Released: {:?}", r.seen);
        let trace = ["listen FAKE0", "open 921600", "restart"];
        assert_eq!(r.trace(), trace, "written whole; no listen begun after the Quit");
    }

    #[test]
    fn the_page_gone_is_a_quit() {
        let fake = Fake::new("status:v0.8.0");
        let (cmds, events) = spawn(Arc::new(fake), setup(Arc::new(Shelf::new("v0.8.0"))));
        drop(cmds);
        let rest: Vec<Event> = events.iter().collect();
        assert_eq!(rest.last(), Some(&Event::Released), "and then the worker ends");
    }

    #[test]
    fn with_the_firmware_first_no_board_is_asked_for_an_image_it_cannot_have() {
        let fake = Fake::new("status:v0.8.0");
        let trace = fake.trace();
        let missing = Image::Local("/nowhere/at/all.bin".into());
        let first = Setup { preset: Some(missing), firmware_first: true, ..setup(Arc::new(Shelf::new("v0.8.0"))) };
        let (cmds, events) = spawn(Arc::new(fake), first);
        let failed = events.iter().find(|e| matches!(e, Event::FirmwareFailed(_)));
        assert!(matches!(failed, Some(Event::FirmwareFailed(DeviceError::Firmware(_)))));
        std::thread::sleep(Duration::from_millis(200));
        assert!(trace.lock().unwrap().is_empty(), "no board looked at");
        drop(cmds);
    }

    #[test]
    fn a_named_port_is_the_only_board_and_an_unlisted_one_is_opened_as_named() {
        let mut r = rig_with(Fake::new("status:v0.8.0,old:v0.7.0"), Shelf::new("v0.8.0"), None, Some("fake1"), false);
        r.settled(1);
        assert_eq!(r.boards.keys().collect::<Vec<_>>(), ["FAKE1"]);
        r.quit();
        let mut r = rig_with(Fake::new("nodevice"), Shelf::new("v0.8.0"), None, Some("COM9"), false);
        r.settled(1);
        assert_eq!(r.board("COM9").candidate.bridge, "?");
        assert_eq!(r.board("COM9").verdict, Verdict::Silent);
        r.quit();
    }

    #[test]
    fn no_board_says_so_with_the_other_ports_and_the_watch_goes_on() {
        let mut r = rig(Fake::new("status:v0.8.0/in=1.2"), "v0.8.0");
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
        let mut r = rig(fake, "v0.9.0");
        r.settled(2);
        assert_eq!(r.board("FAKE0").card(), Card::Foreign { kind: CardKind::ExFat, size: Some(63_864_569_856) });
        assert_eq!(r.board("FAKE1").card(), Card::Empty);
        r.quit();
    }

    // ── Starting up ─────────────────────────────────────────────────────────

    /// The board on `port` was told starting up at some point.
    fn seen_starting(r: &Rig, port: &str) -> bool {
        let starting = |b: &Board| b.port() == port && matches!(b.heard, Heard::Starting { .. });
        r.seen.iter().any(|e| matches!(e, Event::Board(b) if starting(b)))
    }

    #[test]
    fn a_board_still_listing_its_library_is_ours_starting_up_and_asked_until_it_answers() {
        let _en = crate::setup::tests::in_locale("en");
        let mut r = rig(Fake::new("old:v0.7.0/listing=1.5"), "v0.8.0");
        r.until("starting up", |r| r.boards.get("FAKE0").is_some_and(|b| b.verdict == Verdict::Starting));
        let busy = r.board("FAKE0");
        assert!(busy.ours(), "an M5Stack Core2, not an unknown board");
        assert_eq!((busy.primary(), busy.card()), (None, Card::Unknown(CardUnknown::Starting)), "no Read the board");
        assert_eq!(busy.work, Work::Listening, "asked on, on the port it holds");
        r.settled(1);
        let board = r.board("FAKE0");
        assert_eq!((board.verdict.clone(), board.version()), (Verdict::Update, Some("v0.7.0")), "it answered");
        let logs = r.logs();
        assert!(logs.contains(&"FAKE0 is starting up — it answers once its library is listed".to_string()), "{logs:?}");
        assert!(!logs.iter().any(|l| l.contains("said nothing")), "{logs:?}");
        assert!(logs.iter().filter(|l| *l == "@status").count() >= 2, "asked again: {logs:?}");
        assert_eq!(r.trace(), ["listen FAKE0"], "one port held the whole time, nothing reset");
        r.quit();
    }

    #[test]
    fn a_board_that_shows_itself_ours_and_never_answers_is_not_answering_once_its_time_is_up() {
        let _en = crate::setup::tests::in_locale("en");
        // FAKE0 prints its log for a minute and never answers; FAKE1 prints
        // nothing at all, and is today's silent board at once.
        let mut r = rig(Fake::new("silent:v0.7.0/listing=60,silent:v0.7.0"), "v0.8.0");
        r.until("FAKE0 starting up", |r| r.boards.get("FAKE0").is_some_and(|b| b.verdict == Verdict::Starting));
        r.settled(2);
        assert_eq!(r.board("FAKE1").verdict, Verdict::Silent);
        let board = r.board("FAKE0");
        assert_eq!((board.verdict.clone(), board.primary()), (Verdict::Silent, Some(Primary::Read)), "today's card");
        assert!(r.logs().contains(&"FAKE0 said nothing — other firmware, or none running".to_string()), "{:?}", r.logs());
        assert!(!seen_starting(&r, "FAKE1"), "a board that prints nothing is never starting up");
        r.quit();
    }

    #[test]
    fn after_its_own_write_a_board_listing_its_library_says_so_until_it_answers() {
        let _en = crate::setup::tests::in_locale("en");
        let mut r = rig(Fake::new("old:v0.7.0/listing=0.8").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        let before = r.seen.len();
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        let starting = |b: &Board| matches!(b.written, Some(Written::Done { .. })) && matches!(b.heard, Heard::Starting { .. });
        r.until("written, then starting up", |r| r.boards.get("FAKE0").is_some_and(starting));
        let board = r.board("FAKE0");
        assert_eq!(board.verdict, Verdict::UpToDate, "the version just written: ✓ just written");
        assert_eq!(board.card(), Card::Unknown(CardUnknown::Starting), "not 'unknown until the firmware runs'");
        assert_eq!(board.version(), Some("v0.8.0"));
        r.written("FAKE0", "answered");
        assert!(matches!(r.board("FAKE0").heard, Heard::Old { version: Some(ref v), .. } if v == "v0.8.0"));
        let after: Vec<String> = r.seen[before..]
            .iter()
            .filter_map(|e| match e {
                Event::Log { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(after.contains(&"FAKE0 is starting up — it answers once its library is listed".to_string()), "{after:?}");
        assert!(!after.iter().any(|l| l.contains("said nothing")), "{after:?}");
        assert_eq!(r.trace(), ["listen FAKE0", "open 921600", "restart", "listen FAKE0"]);
        r.quit();
    }

    #[test]
    fn after_its_own_write_a_board_that_never_answers_is_said_as_today_once_its_time_is_up() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("old:v0.7.0/listing=60").with_pace(Duration::from_millis(60));
        let trace = fake.trace();
        let timing = Timing { waits: Waits { up: Duration::from_millis(800), ..QUICK.waits }, ..QUICK };
        let shelf = Arc::new(Shelf::new("v0.8.0"));
        let mut r = rig_on(fake, Setup { timing, ..setup(shelf.clone()) }, shelf, trace);
        r.settled(1);
        assert_eq!(r.board("FAKE0").verdict, Verdict::Silent, "it never answered this visit");
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("written, then given up on", |r| {
            let b = r.board("FAKE0");
            matches!(b.written, Some(Written::Done { .. })) && b.work == Work::Idle && b.heard == Heard::Silent
        });
        let board = r.board("FAKE0");
        assert!(seen_starting(&r, "FAKE0"), "starting up meanwhile");
        assert_eq!(board.card(), Card::Unknown(CardUnknown::NotRunning), "today's words, and `r` to ask again");
        assert!(r.logs().iter().filter(|l| l.contains("said nothing")).count() >= 2, "{:?}", r.logs());
        r.quit();
    }

    // ── Advanced options ────────────────────────────────────────────────────

    #[test]
    fn a_board_that_runs_dio_is_updated_in_dio_and_says_so() {
        let _en = crate::setup::tests::in_locale("en");
        let mut r = rig(Fake::new("old:v0.7.0/mode=dio").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        let board = r.board("FAKE0");
        assert_eq!(board.mode().map(|m| m.mode), Some(Mode::Dio), "aa45f60e: v0.7.0's DIO build");
        assert_eq!(board.default_image(), Image::Pin(Mode::Dio));
        assert_eq!((board.verdict.clone(), board.primary()), (Verdict::Update, Some(Primary::Update)));
        assert!(board.pending().is_none(), "nothing chosen: its own mode is the default");
        assert!(r.logs().contains(&"FAKE0: v0.7.0 in DIO, too old to report its card".to_string()), "{:?}", r.logs());
        // Its image is had beside the page's, before the yes.
        let dio_ready = |e: &Event| {
            matches!(e, Event::Image { image: Image::Pin(Mode::Dio), state: ImageState::Ready(_) })
        };
        r.until("the pin's DIO build in hand", |r| r.seen.iter().any(dio_ready));
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.written("FAKE0", "written and heard");
        let plans = r.plans();
        assert_eq!(plans.len(), 1);
        assert_eq!((&plans[0].image, plans[0].mode, plans[0].elf.as_str()), (&Image::Pin(Mode::Dio), Some(Mode::Dio), "3523b80e"));
        let board = r.board("FAKE0");
        assert_eq!(board.verdict, Verdict::UpToDate);
        assert_eq!(board.mode().map(|m| m.mode), Some(Mode::Dio), "DIO again, by its new ELF");
        assert!(r.logs().iter().any(|l| l == "write: update, no erase · v0.8.0 in DIO"), "{:?}", r.logs());
        r.quit();
    }

    #[test]
    fn a_yes_for_an_image_the_board_no_longer_has_next_is_refused_with_nothing_touched() {
        let _en = crate::setup::tests::in_locale("en");
        let mut r = rig(Fake::new("old:v0.7.0").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        // The page drew its gate for the pin; a choice it sent just before
        // reached the worker first (a scan held the worker meanwhile).
        let release = Image::release("v0.6.0", Mode::Qio);
        let choice = Choice { image: release.clone(), erase: false };
        r.send(Cmd::Choose { port: "FAKE0".into(), choice: Some(choice) });
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: Some(Image::Pin(Mode::Qio)) });
        r.until("refused", |r| !r.refused().is_empty());
        assert_eq!(r.refused(), [(Some("FAKE0".into()), Refusal::Moved)]);
        let for_the_lock = |e: &Event| matches!(e, Event::Refused { write: true, .. });
        assert!(r.seen.iter().any(for_the_lock), "the refusal the page's lock waits for");
        std::thread::sleep(Duration::from_millis(200));
        assert!(r.resets().is_empty(), "nothing written unseen: {:?}", r.trace());
        assert_eq!(r.board("FAKE0").write_image(), release, "the card has the board as it is");
        // The gate drawn again names the release: that yes writes it.
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: Some(release.clone()) });
        r.written("FAKE0", "the release written and heard");
        assert_eq!(r.plans().iter().map(|f| f.image.clone()).collect::<Vec<_>>(), [release]);
        r.quit();
    }

    #[test]
    fn a_choice_lasts_one_write_and_never_moves_the_verdict() {
        let mut r = rig(Fake::new("old:v0.8.0").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        assert_eq!((r.board("FAKE0").verdict.clone(), r.board("FAKE0").primary()), (Verdict::UpToDate, None));
        r.choose("FAKE0", Some(Image::Pin(Mode::Dio)));
        let board = r.board("FAKE0");
        assert_eq!((board.verdict.clone(), board.primary()), (Verdict::UpToDate, Some(Primary::Write)), "a mode change");
        let ready = |n: &Next| matches!(&n.state, NextState::Ready(f) if f.mode == Some(Mode::Dio));
        assert!(board.pending().is_some_and(|n| n.by == By::Sheet && ready(n)));
        r.choose("FAKE0", Some(Image::release("v0.7.0", Mode::Qio)));
        let board = r.board("FAKE0");
        assert_eq!((board.verdict.clone(), board.primary()), (Verdict::UpToDate, Some(Primary::Back)), "still the pin's chip");
        r.choose("FAKE0", None);
        assert_eq!(r.board("FAKE0").primary(), None, "Reset: the defaults again");
        r.choose("FAKE0", Some(Image::Pin(Mode::Qio)));
        assert!(r.board("FAKE0").next.is_none(), "applying the defaults is no choice");
        r.choose("FAKE0", Some(Image::Pin(Mode::Dio)));
        r.send(Cmd::Write { port: "FAKE0".into(), erase: None, image: None });
        r.written("FAKE0", "the DIO build written and heard");
        let board = r.board("FAKE0");
        assert!(board.next.is_none(), "one board, one write");
        let pinned = |image: &ImageFacts| image.mode == Some(Mode::Dio) && image.check == Check::Pinned;
        assert!(matches!(&board.written, Some(Written::Done { image, looping: None, .. }) if pinned(image)));
        assert_eq!(board.default_mode(), Mode::Dio, "the mode outlives the choice, in the board's own ELF");
        assert_eq!(board.verdict, Verdict::UpToDate);
        r.quit();
    }

    #[test]
    fn a_write_that_leaves_the_board_restarting_offers_its_dio_build_and_writes_nothing_by_itself() {
        let _en = crate::setup::tests::in_locale("en");
        let mut r = rig(Fake::new("old:v0.7.0/loop=qio").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("written, then heard saying nothing", |r| {
            let b = r.board("FAKE0");
            matches!(b.written, Some(Written::Done { .. })) && b.work == Work::Idle && b.heard == Heard::Silent
        });
        let board = r.board("FAKE0");
        assert_eq!(board.looping(), Some(&Looping { restarts: 3, secs: 6 }));
        let next = board.pending().expect("the cure filled in");
        assert_eq!((next.image.clone(), next.by), (Image::Pin(Mode::Dio), By::Loop));
        assert!(matches!(next.state, NextState::Ready(_)), "had before the gate's yes");
        assert_eq!(board.primary(), Some(Primary::Dio));
        assert_eq!(board.verdict, Verdict::UpToDate, "the pin's verdict: v0.8.0 went on");
        assert_eq!(board.card(), Card::Unknown(CardUnknown::NotRunning));
        let logs = r.logs();
        assert!(logs.contains(&"rst:0x10 (RTCWDT_RTC_RESET),boot:0x13 (SPI_FAST_FLASH_BOOT)".to_string()), "{logs:?}");
        assert!(logs.contains(&"no boot line in 6 s: 3 restarts heard — the DIO image is offered".to_string()), "{logs:?}");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(r.trace().iter().filter(|t| t.starts_with("open")).count(), 1, "offered, never written by itself");
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("the DIO build written and heard running", |r| {
            let b = r.board("FAKE0");
            b.looping().is_none() && b.work == Work::Idle && matches!(b.heard, Heard::Old { .. })
        });
        let board = r.board("FAKE0");
        assert!(board.next.is_none() && board.primary().is_none(), "cured: nothing left to do");
        assert_eq!(board.mode().map(|m| m.mode), Some(Mode::Dio));
        r.quit();
    }

    #[test]
    fn update_all_writes_each_board_in_its_own_mode_and_leaves_out_one_with_its_own_release() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("old:v0.7.0,old:v0.7.0/mode=dio,old:v0.6.0").with_pace(Duration::from_millis(60));
        let probe = fake.clone();
        let mut r = rig(fake, "v0.8.0");
        r.settled(3);
        r.choose("FAKE2", Some(Image::release("v0.7.0", Mode::Qio)));
        assert!(r.board("FAKE2").left_out(), "behind the pin, with a release of its own to write");
        r.send(Cmd::UpdateAll { ports: (0..3).map(|i| format!("FAKE{i}")).collect() });
        r.until("all done", |r| r.all().iter().any(|a| matches!(a, All::Done { .. })));
        let both = vec!["FAKE0".to_string(), "FAKE1".to_string()];
        assert!(matches!(r.all().last(), Some(All::Done { ports, passed, .. }) if *ports == both && passed.is_empty()));
        let modes: Vec<Option<Mode>> = r.plans().iter().map(|f| f.mode).collect();
        assert_eq!(modes, [Some(Mode::Qio), Some(Mode::Dio)], "each board in the mode it ran");
        let left = "update all: FAKE2 is left out — its next write is v0.7.0 QIO; write it from its tab";
        assert!(r.logs().contains(&left.to_string()), "{:?}", r.logs());
        assert_eq!(probe.version_on("FAKE2").as_deref(), Some("v0.6.0"), "its own write, from its own tab");
        assert!(r.board("FAKE2").pending().is_some(), "and its choice still waits for it");
        r.quit();
    }

    #[test]
    fn update_all_takes_a_boards_chosen_mode_and_never_its_erase() {
        let fake = Fake::new("old:v0.7.0,old:v0.6.0").with_pace(Duration::from_millis(60));
        let mut r = rig(fake, "v0.8.0");
        r.settled(2);
        let choice = Choice { image: Image::Pin(Mode::Dio), erase: true };
        r.send(Cmd::Choose { port: "FAKE1".into(), choice: Some(choice) });
        r.until("the choice carried", |r| r.board("FAKE1").pending().is_some_and(|n| n.erase));
        assert!(r.board("FAKE1").in_update_all(), "the pin, in a mode chosen for it");
        r.send(Cmd::UpdateAll { ports: vec!["FAKE0".into(), "FAKE1".into()] });
        r.until("all done", |r| r.all().iter().any(|a| matches!(a, All::Done { .. })));
        let plans: Vec<(bool, Option<Mode>)> = r
            .seen
            .iter()
            .filter_map(|e| match e {
                Event::Plan { plan, image, .. } => Some((plan.erase, image.mode)),
                _ => None,
            })
            .collect();
        assert_eq!(plans, [(false, Some(Mode::Qio)), (false, Some(Mode::Dio))], "Update all never erases");
        r.quit();
    }

    #[test]
    fn a_release_chosen_is_had_at_once_and_one_that_cannot_be_had_says_why_and_touches_nothing() {
        let mut r = rig(Fake::new("old:v0.7.0").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        r.choose("FAKE0", Some(Image::release("v0.6.0", Mode::Dio)));
        let next = r.board("FAKE0").pending().cloned().expect("chosen");
        assert_eq!((next.version().as_deref(), next.mode()), (Some("v0.6.0"), Some(Mode::Dio)));
        assert_eq!(r.board("FAKE0").primary(), Some(Primary::Back), "v0.7.0 → v0.6.0 goes back");
        r.choose("FAKE0", Some(Image::release("v0.9.0", Mode::Qio)));
        let failed = |n: &Next| matches!(&n.state, NextState::Failed(e) if e.text().contains("not on the test's shelf"));
        assert!(r.board("FAKE0").pending().is_some_and(failed), "{:?}", r.board("FAKE0").next);
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("the write failed", |r| {
            let b = r.board("FAKE0");
            matches!(b.written, Some(Written::Failed { half: false, .. })) && b.work == Work::Idle
        });
        assert!(r.resets().is_empty(), "no board touched for an image it could not have: {:?}", r.trace());
        assert!(r.board("FAKE0").pending().is_some(), "a failure keeps the choice: Try again writes the same image");
        r.quit();
    }

    #[test]
    fn a_local_build_is_vetted_for_the_sheet_and_read_again_at_the_yes() {
        let _en = crate::setup::tests::in_locale("en");
        let scratch = crate::device::firmware::tests::Scratch::new("desk-local");
        let build = scratch.0.join("firmware.factory.bin");
        let write = |version: &str, elf: &str| {
            let desc = desc_with_elf(version, AppDesc::OURS, &elf_bytes(elf));
            std::fs::write(&build, merged_in(&desc, Mode::Dio)).unwrap();
        };
        write("v0.8.0-5-g4e94418", "be894f8b");
        let mut app = vec![0u8; AppDesc::OFFSET_IN_APP];
        app[0] = 0xE9;
        app.extend_from_slice(&desc_bytes("v0.8.0", AppDesc::OURS));
        let alone = scratch.0.join("firmware.bin");
        std::fs::write(&alone, &app).unwrap();

        let mut r = rig(Fake::new("old:v0.7.0").with_pace(Duration::from_millis(60)), "v0.8.0");
        r.settled(1);
        r.send(Cmd::Vet { path: scratch.0.clone() });
        r.until("the folder vetted", |r| r.seen.iter().any(|e| matches!(e, Event::Vetted { .. })));
        r.send(Cmd::Vet { path: alone.clone() });
        let vetted = |r: &Rig| -> Vec<(PathBuf, Result<ImageFacts, DeviceError>)> {
            r.seen
                .iter()
                .filter_map(|e| match e {
                    Event::Vetted { path, result } => Some((path.clone(), result.clone())),
                    _ => None,
                })
                .collect()
        };
        r.until("the app vetted", |r| vetted(r).len() == 2);
        let found = vetted(&r);
        let folder = found[0].1.as_ref().expect("a merged build");
        assert_eq!((folder.version.as_str(), folder.mode), ("v0.8.0-5-g4e94418", Some(Mode::Dio)));
        assert_eq!(folder.file.as_deref(), Some(build.as_path()), "the folder's merged image, named");
        let refused = found[1].1.as_ref().unwrap_err().text();
        assert!(refused.contains("device flash --firmware"), "an app alone stays the command line's: {refused}");

        r.choose("FAKE0", Some(Image::Local(scratch.0.clone())));
        assert_eq!(r.board("FAKE0").primary(), Some(Primary::Write), "a local build is written, never an update");
        // Rebuilt after it was picked: the yes reads it again, and refuses.
        write("v0.8.0-6-gabcdef0", "be894f8c");
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("refused", |r| !r.refused().is_empty());
        let changed = Refusal::Changed { was: "v0.8.0-5-g4e94418".into(), now: "v0.8.0-6-gabcdef0".into() };
        assert_eq!(r.refused(), [(Some("FAKE0".into()), changed)]);
        let shown = r.board("FAKE0").pending().and_then(Next::facts).map(|f| f.version.clone());
        assert_eq!(shown.as_deref(), Some("v0.8.0-6-gabcdef0"), "the gate shows the new one");
        assert!(r.resets().is_empty(), "nothing written unseen");
        // Seen, and written.
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.written("FAKE0", "the build written and heard");
        let board = r.board("FAKE0");
        let local = |image: &ImageFacts| image.check == Check::Description && image.image.is_local();
        assert!(matches!(&board.written, Some(Written::Done { version, image, .. }) if version == "v0.8.0-6-gabcdef0" && local(image)));
        assert!(board.next.is_none());
        assert_eq!(board.mode().map(|m| m.mode), Some(Mode::Dio), "a build in hand teaches its ELF");
        assert_eq!(board.verdict, Verdict::Newer, "measured against the pin: ahead of it");
        // An app alone chosen in the sheet: refused, never written.
        r.choose("FAKE0", Some(Image::Local(alone.clone())));
        assert!(r.board("FAKE0").pending().is_some_and(|n| matches!(n.state, NextState::Failed(_))));
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.until("refused again", |r| r.refused().len() == 2);
        assert!(matches!(r.refused()[1].1, Refusal::Image(_)));
        assert_eq!(r.resets().iter().filter(|t| t.starts_with("open")).count(), 1, "the one write");
        r.quit();
    }

    #[test]
    fn the_release_list_is_asked_once_a_visit_and_a_failure_is_never_kept() {
        let mut shelf = Shelf::new("v0.8.0");
        *shelf.list.lock().unwrap() = Err(ListWhy::Offline("no route to host".into()));
        shelf.cached = vec![Cached { tag: "v0.7.0".into(), modes: vec![Mode::Dio], since: None }];
        let mut r = rig_with(Fake::new("nodevice"), shelf, None, None, false);
        let answers = |r: &Rig| -> Vec<Result<ReleaseList, ListFailed>> {
            r.seen
                .iter()
                .filter_map(|e| match e {
                    Event::Releases(result) => Some(result.clone()),
                    _ => None,
                })
                .collect()
        };
        r.send(Cmd::Releases);
        r.until("the failure", |r| answers(r).len() == 1);
        let failed = answers(&r)[0].clone().unwrap_err();
        assert_eq!(failed.why, ListWhy::Offline("no route to host".into()));
        assert_eq!(failed.cached[0].tag, "v0.7.0", "what this computer can write without it");
        *r.shelf.list.lock().unwrap() = Ok(crate::device::firmware::tests::listed());
        r.send(Cmd::Releases);
        r.until("the list", |r| answers(r).len() == 2);
        r.send(Cmd::Releases);
        r.until("the list again", |r| answers(r).len() == 3);
        assert!(answers(&r)[1..].iter().all(|a| a.as_ref().is_ok_and(|l| l.releases.len() == 5)));
        assert_eq!(r.shelf.asked.load(Ordering::Relaxed), 2, "the failure was asked again; the list, once");
        r.quit();
    }

    #[test]
    fn a_release_downloading_for_a_board_is_on_its_card_at_each_whole_percent() {
        let shelf = Shelf { pace: Duration::from_millis(200), ..Shelf::new("v0.8.0") };
        let mut r = rig_with(Fake::new("old:v0.7.0"), shelf, None, None, false);
        r.settled(1);
        r.choose("FAKE0", Some(Image::release("v0.7.0", Mode::Dio)));
        let getting: Vec<u64> = r
            .seen
            .iter()
            .filter_map(|e| match e {
                Event::Board(b) => match b.next.as_ref().map(|n| &n.state) {
                    Some(NextState::Getting { done, total: Some(_) }) => Some(*done),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert!(getting.len() >= 3 && getting.windows(2).all(|w| w[0] < w[1]), "the bar moves: {getting:?}");
        let heard = |e: &Event| match e {
            Event::Image { image, state: ImageState::Getting { done, .. } } => *done > 0 && image.tag() == Some("v0.7.0"),
            _ => false,
        };
        assert!(r.seen.iter().any(heard), "and the page hears the image itself");
        assert!(r.board("FAKE0").pending().is_some_and(|n| matches!(n.state, NextState::Ready(_))));
        r.quit();
    }

    #[test]
    fn the_flags_choice_is_each_boards_next_write_and_the_verdict_stays_the_pins() {
        let preset = Image::release("v0.7.0", Mode::Qio);
        let mut r = rig_with(Fake::new("old:v0.8.0,old:v0.6.0"), Shelf::new("v0.8.0"), Some(preset), None, false);
        r.settled(2);
        assert!(r.seen.iter().any(|e| matches!(e, Event::Target(t) if t.version == "v0.8.0")), "the pin, never the flag's");
        let flagged = |e: &Event| matches!(e, Event::Firmware { version, .. } if version == "v0.7.0");
        r.until("the page's image, the flag's", |r| r.seen.iter().any(flagged));
        let (up, behind) = (r.board("FAKE0"), r.board("FAKE1"));
        assert_eq!((up.verdict.clone(), up.primary()), (Verdict::UpToDate, Some(Primary::Back)));
        assert_eq!((behind.verdict.clone(), behind.primary()), (Verdict::Update, Some(Primary::Update)));
        assert!(up.pending().is_some_and(|n| n.by == By::Flags));
        r.quit();
    }

    #[test]
    fn a_release_flag_with_no_mode_keeps_each_board_in_the_mode_its_elf_says() {
        // FAKE0 runs v0.6.0's DIO build, FAKE1 its QIO one, FAKE2 v0.5.0,
        // whose DIO was no choice.
        let spec = "old:v0.6.0/mode=dio,old:v0.6.0,old:v0.5.0/elf=17352e55";
        let mut r = rig_flags(Fake::new(spec).with_pace(Duration::from_millis(60)), Some("v0.7.0"), None);
        r.settled(3);
        let release = |mode| Image::release("v0.7.0", mode);
        let ready = |r: &Rig, port: &str| {
            r.board(port).pending().is_some_and(|n| matches!(n.state, NextState::Ready(_)))
        };
        r.until("each board's build in hand", |r| ["FAKE0", "FAKE1", "FAKE2"].iter().all(|p| ready(r, p)));
        let next = |port: &str| r.board(port).pending().map(|n| (n.image.clone(), n.mode(), n.by));
        let flags = |mode| Some((release(mode), Some(mode), By::Flags));
        assert_eq!(next("FAKE0"), flags(Mode::Dio), "DIO stays DIO");
        assert_eq!(next("FAKE1"), flags(Mode::Qio));
        assert_eq!(next("FAKE2"), flags(Mode::Qio), "v0.5.0's DIO was no choice");
        r.send(Cmd::Write { port: "FAKE0".into(), erase: Some(false), image: None });
        r.written("FAKE0", "written and heard");
        let modes: Vec<Option<Mode>> = r.plans().iter().map(|f| f.mode).collect();
        assert_eq!(modes, [Some(Mode::Dio)], "the build the card named");
        r.quit();

        // The pin's own tag with no mode is the pin in the board's mode: on
        // a board that runs DIO, no choice at all, and nothing put back.
        let mut r = rig_flags(Fake::new("old:v0.7.0/mode=dio"), Some("v0.8.0"), None);
        r.settled(1);
        let own = |r: &Rig| r.board("FAKE0").next.as_ref().is_some_and(|n| n.image == Image::Pin(Mode::Dio));
        r.until("the board's own build, the flags'", own);
        assert!(r.board("FAKE0").pending().is_none(), "the default is no choice");
        r.quit();

        // `--flash-mode` named: that build, whatever the board runs.
        let mut r = rig_flags(Fake::new("old:v0.6.0/mode=dio"), Some("v0.7.0"), Some(Mode::Qio));
        r.settled(1);
        let named = r.board("FAKE0").pending().map(|n| n.image.clone());
        assert_eq!(named, Some(release(Mode::Qio)), "named, so kept");
        r.quit();
    }
}
