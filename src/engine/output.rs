//! The output device, and everything needed to notice it is the wrong one.
//!
//! rodio (through cpal) opens a stream on one concrete device and stays
//! there for the stream's whole life. The operating system does not: plug
//! headphones into the jack, pair a Bluetooth speaker, and the *default*
//! output moves while the old stream plays on at the old endpoint —
//! Windows and macOS both leave it running; pull the device the stream is
//! on and the stream just dies. Neither event reaches the engine on its
//! own, which is how "I plugged in headphones and the speakers kept
//! playing" happens.
//!
//! So three things are watched. The stream's error callback sets a flag
//! when the device dies under it. The identity of the system default is
//! looked up once a second, to notice it moving. And the callback keeps a
//! pulse — when it last asked for a buffer — because a death is not always
//! announced: cpal's WASAPI thread waits without limit on an event a
//! removed endpoint never signals again, and ends without a flag on any
//! error it does get, so on Windows a stream can simply stop asking. What
//! to *do* about any of the three is the engine's business
//! (`Engine::ensure_output`).
//!
//! None of it happens on the engine's thread. Opening a device, asking
//! which one is the default and closing a stream are calls into the
//! operating system's audio service, and around a change of device that
//! service can take its time: an endpoint that has only just appeared, a
//! Bluetooth link not up yet, a stream thread to wait for. Made inline they
//! held the thread that ticks and answers every command — the player went
//! deaf and dumb for as long as the device took (the device-switch freeze).
//! They are made on the device thread instead ([`Desk`]): the engine asks,
//! goes on ticking, and picks the answer up when there is one. A stream
//! being closed is handed to a thread of its own ([`retire`]).
//!
//! Nor does a stream own the sound any more. Its callback reads the
//! engine's [`Bus`]; replacing the stream gives the bus to the new one and
//! the music goes on from the sample it stood at.
//!
//! The stream is still this module's outright, which rodio's
//! `MixerDeviceSink` would not allow: that type starts its cpal stream and
//! keeps it private, with no way to stop it. A running stream is a device
//! callback pulling silence ~23 times a second for as long as the process
//! lives — paused, stopped, never played — and to the operating system an
//! active audio client: coreaudiod holds a PreventUserIdleSystemSleep
//! assertion for the player's PID, so a Mac with the player open never
//! idle-sleeps, and PulseAudio or PipeWire cannot suspend the sink
//! (performance audit #75). Held here, the stream can be suspended when the
//! engine goes quiet and woken before anything needs it again
//! ([`Output::suspend`], [`Output::wake`]).

use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::cpal::{self, BufferSize, SampleFormat, StreamConfig};

#[cfg(not(test))]
use super::DEVICE_POLL;
use super::bus::{Bus, Format};
use super::trace::etrace;

/// The device thread's name. Whatever panics under it — cpal's WASAPI host
/// `expect`s its way through an endpoint that vanished between two calls —
/// is caught there and read as a failed open, so the panic hooks stand back
/// for it as they do for the audio thread (`tui::worker::panics_are_caught`).
pub(crate) const DEVICE_THREAD: &str = "mstream-audio-device";

/// How often the device thread looks up the system default: the engine's
/// device poll.
#[cfg(not(test))]
const LOOK: Duration = DEVICE_POLL;
/// A staged default moves when a test says so; nobody wants a second of
/// waiting to hear about it.
#[cfg(test)]
const LOOK: Duration = Duration::from_millis(100);

/// How long a stream that has started is given to ask for its first buffer
/// before the open counts as failed. Starting proves little: on WASAPI
/// `play` only queues the start, and a device caught mid-change takes the
/// stream and then never runs it. A wired output asks within a few tens of
/// milliseconds; the patience is for the ones that connect first — a
/// Bluetooth link, a network speaker.
#[cfg(not(test))]
const FIRST_PULL: Duration = Duration::from_secs(5);
/// Tests wait this out on purpose.
#[cfg(test)]
const FIRST_PULL: Duration = Duration::from_millis(600);

/// How long a failed open may take before the flight recorder is told
/// about it every time, not only when its reason is new.
const SLOW_FAILURE: Duration = Duration::from_secs(1);

/// How long one device may spend refusing before the walk moves on. A
/// device that refuses a config for what the config is refuses at once; one
/// that takes seconds over each refusal — a Bluetooth endpoint whose
/// headset has gone, still listed, still the default — is not going to like
/// the next config better, and every further try is seconds more of
/// silence before the speakers get their turn.
const DEVICE_BUDGET: Duration = Duration::from_secs(3);

/// How many of a device's listed configurations are tried once its own
/// default has been refused both ways. cpal's WASAPI host lists every
/// common rate in every sample format — some ninety — and a device that
/// refused its own mix format refuses those for the same reason, each at
/// the cost of activating a client; walked to the end, device after
/// device, that was minutes.
const OTHER_CONFIGS: usize = 6;

/// The pulse's clock: milliseconds since the first look at it.
fn clock_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// What a stream's callback says about itself, for the threads that wait
/// on it or wonder about it.
pub(crate) struct Pulse {
    /// Set by the stream's error callback when the device dies under it —
    /// and by the engine, when the callback has stopped answering.
    gone: AtomicBool,
    /// Up for as long as the callback is inside a pull — see
    /// [`Output::suspend`] for why a pull in flight matters.
    pulling: AtomicBool,
    /// When the last pull ended on [`clock_ms`], plus one; zero until the
    /// first.
    pulled: AtomicU64,
    /// Since when pulls have been expected: the open, or the last wake.
    expected: AtomicU64,
}

impl Pulse {
    fn new() -> Pulse {
        Pulse {
            gone: AtomicBool::new(false),
            pulling: AtomicBool::new(false),
            pulled: AtomicU64::new(0),
            expected: AtomicU64::new(clock_ms()),
        }
    }

    /// One pull, from inside the device callback: two atomic stores and a
    /// look at the clock around it.
    #[inline]
    fn beat(&self, pull: impl FnOnce()) {
        self.pulling.store(true, Ordering::Release);
        pull();
        self.pulled.store(clock_ms() + 1, Ordering::Release);
        self.pulling.store(false, Ordering::Release);
    }

    pub(crate) fn is_gone(&self) -> bool {
        self.gone.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_gone(&self) {
        self.gone.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_pulling(&self) -> bool {
        self.pulling.load(Ordering::Acquire)
    }

    /// The stream is expected to pull from here on: it has just been woken,
    /// or whoever is watching it has just come back from being held still.
    pub(crate) fn expect_from_now(&self) {
        self.expected.store(clock_ms(), Ordering::Release);
    }

    /// Whether the callback is running right now, as far as anyone outside
    /// it can tell: inside a pull, or out of one no longer ago than `fresh`.
    pub(crate) fn answers(&self, fresh: Duration) -> bool {
        if self.is_pulling() {
            return true;
        }
        match self.pulled.load(Ordering::Acquire) {
            0 => false,
            at => clock_ms().saturating_sub(at - 1) <= fresh.as_millis() as u64,
        }
    }

    /// How long the callback has gone without finishing a pull, counted
    /// from when it was last expected to make one.
    pub(crate) fn quiet_for(&self) -> Duration {
        let pulled = self.pulled.load(Ordering::Acquire).saturating_sub(1);
        let since = pulled.max(self.expected.load(Ordering::Acquire));
        Duration::from_millis(clock_ms().saturating_sub(since))
    }

    /// Wait for the first pull, for at most `patience`.
    fn first_pull_within(&self, patience: Duration) -> bool {
        let until = Instant::now() + patience;
        loop {
            if self.pulled.load(Ordering::Acquire) != 0 || self.is_pulling() {
                return true;
            }
            if self.is_gone() || Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// The stream behind an [`Output`].
enum Stream {
    Device(cpal::Stream),
    #[cfg(test)]
    Staged(stage::Puller),
}

impl Stream {
    fn play(&self) -> Result<(), String> {
        match self {
            Stream::Device(stream) => stream.play().map_err(|e| e.to_string()),
            #[cfg(test)]
            Stream::Staged(puller) => puller.play(),
        }
    }

    fn pause(&self) -> Result<(), String> {
        match self {
            Stream::Device(stream) => stream.pause().map_err(|e| e.to_string()),
            #[cfg(test)]
            Stream::Staged(puller) => puller.pause(),
        }
    }
}

/// An open output stream, its name, and what there is to watch about it.
pub(crate) struct Output {
    /// This stream's name on the bus: the floor is given to it by this.
    id: u64,
    /// The device's human-readable name, for notices ("audio moved to
    /// Headphones (WH-1000XM4)").
    name: String,
    /// The identity of the device this stream is on.
    device: Option<String>,
    /// What the system default's identity was when this stream opened: the
    /// same device, unless the default would not open and another carries
    /// the session.
    default_at_open: Option<String>,
    pulse: Arc<Pulse>,
    /// Whether the device callback is running. The open starts it — that
    /// it starts, and asks for a buffer, is what proves the device works,
    /// which is why a missing or broken device is still an error at launch
    /// — and the engine suspends it once nothing has needed it for a while.
    awake: bool,
    stream: Stream,
}

/// The next stream's id. Zero is the bus's "nobody".
fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Output {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The system default this stream opened on, when it did open on it.
    pub(crate) fn default_it_is_on(&self) -> Option<&str> {
        self.default_at_open.as_deref().filter(|default| self.is_on(Some(default)))
    }

    /// The identity of the device this stream is on, where it has one.
    pub(crate) fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    /// Whether this stream is on the device `identity` names.
    pub(crate) fn is_on(&self, identity: Option<&str>) -> bool {
        identity.is_some() && identity == self.device.as_deref()
    }

    pub(crate) fn pulse(&self) -> &Pulse {
        &self.pulse
    }

    /// Whether the device callback is running.
    pub(crate) fn is_awake(&self) -> bool {
        self.awake
    }

    /// Let the device go idle: the callback stops, and with it every reason
    /// the operating system had to keep the audio hardware — and on macOS
    /// the machine — awake on the player's behalf. Nothing on the bus is
    /// touched: the players, their positions and a paused track's decoder
    /// stay exactly where they stand, frozen, until [`Output::wake`].
    /// True when the stream is now asleep. A backend that cannot pause
    /// keeps running (cpal's ALSA host swallows the error on raw hardware
    /// without pause support), which costs what it always did.
    ///
    /// Never under a pull in flight. Pausing waits for the callback it
    /// interrupts — CoreAudio's AudioDeviceStop queues on the lock the IO
    /// thread holds through the callback — and a pull can be stuck: a
    /// track whose download stalled parks the callback in the decoder's
    /// read until the network answers, audit #48's wait. The suspend held
    /// the tick there, and every control queued behind it (review of
    /// audit #75). A stream mid-pull is left running and asked again at a
    /// later tick. A callback that is not pulling cannot start a pull that
    /// blocks, as long as the engine is at rest: a paused player never
    /// reads its decoder, and a stopped one has left the bus.
    pub(crate) fn suspend(&mut self) -> bool {
        if !self.awake || self.is_dead() || self.pulse.is_pulling() {
            return false;
        }
        match self.stream.pause() {
            Ok(()) => {
                self.awake = false;
                true
            }
            Err(e) => {
                etrace!("output would not suspend on {}: {e}", self.name);
                false
            }
        }
    }

    /// Run the callback again. rodio completes a Player's seek, stop and
    /// skip inside the callback — try_seek waits on it outright — so
    /// anything about to lean on a Player comes after this. A stream that
    /// will not restart pulls the death tripwire: whatever happened to the
    /// device while it slept, a new stream is the answer.
    pub(crate) fn wake(&mut self) {
        if self.awake {
            return;
        }
        match self.stream.play() {
            Ok(()) => {
                self.awake = true;
                self.pulse.expect_from_now();
                etrace!("output woke on {}", self.name);
            }
            Err(e) => {
                etrace!("output would not wake on {}: {e}", self.name);
                self.pulse.mark_gone();
            }
        }
    }

    /// Whether the stream reported its own death: the device was
    /// unplugged, the Bluetooth link dropped, the stream invalidated.
    pub(crate) fn is_dead(&self) -> bool {
        self.pulse.is_gone()
    }

    /// Pull the death tripwire by hand. Nobody can unplug hardware from a
    /// test or a smoke, but from here on the recovery path is the same one
    /// a real unplug takes.
    pub(crate) fn pretend_dead(&self) {
        self.pulse.mark_gone();
    }
}

/// Hand `what` — a stream being replaced, mostly — to a thread of its own
/// to be dropped. Closing a stream waits for its callback thread: cpal's
/// WASAPI stream joins it, CoreAudio's stop queues behind the IO thread,
/// and a callback parked on a stalled download keeps either waiting for as
/// long as the network does. That wait is nobody's to make but a thread
/// with nothing else to do. Where no thread can be had, the drop happens
/// here, as it always did.
pub(crate) fn retire<T: Send + 'static>(what: T) {
    let _ = std::thread::Builder::new().name("mstream-audio-retire".into()).spawn(move || drop(what));
}

/// Where outputs come from: the system's devices, or a test's.
pub(crate) trait Backend: Send {
    /// Open a stream reading `bus`, started and proven — it has asked for a
    /// buffer. On the system default; and, when that will not open and
    /// `others` allows it, on whichever other device will.
    fn open(&mut self, bus: &Bus, others: bool) -> Result<Output, String>;

    /// The identity of whatever the system calls the default output right
    /// now.
    fn default_identity(&mut self) -> Option<String>;
}

/// The engine's end of the device thread: asked, never waited on — but for
/// the one open an engine cannot start without.
pub(crate) struct Desk {
    jobs: mpsc::Sender<Job>,
    answers: mpsc::Receiver<Result<Output, String>>,
    /// The system default's identity as the device thread last saw it.
    default: Arc<Mutex<Option<String>>>,
    /// When the open in flight was asked for.
    asked: Option<Instant>,
}

enum Job {
    Open { others: bool },
}

impl Desk {
    /// Start the device thread over `backend`.
    pub(crate) fn start(backend: Box<dyn Backend>, bus: &Bus) -> Result<Desk, String> {
        let (jobs, todo) = mpsc::channel();
        let (answer, answers) = mpsc::channel();
        let default = Arc::new(Mutex::new(None));
        let seen = default.clone();
        let bus = bus.clone();
        std::thread::Builder::new()
            .name(DEVICE_THREAD.into())
            .spawn(move || {
                run(backend, &bus, &todo, &answer, &seen);
                // Everything the thread held goes before it settles: the
                // bus above all, and with it whatever was still sounding.
                drop((bus, todo, answer, seen));
                stay();
            })
            .map_err(|e| format!("the audio device thread would not start: {e}"))?;
        Ok(Desk { jobs, answers, default, asked: None })
    }

    /// Ask for a new output, unless one is already being opened: opens are
    /// one at a time, and the answer to the one in flight comes first.
    pub(crate) fn ask(&mut self, others: bool) {
        if self.asked.is_none() && self.jobs.send(Job::Open { others }).is_ok() {
            self.asked = Some(Instant::now());
        }
    }

    /// How long the open in flight has been out, if one is.
    pub(crate) fn asked_for(&self) -> Option<Duration> {
        self.asked.map(|at| at.elapsed())
    }

    /// The answer to the open in flight, if it has come.
    pub(crate) fn answer(&mut self) -> Option<Result<Output, String>> {
        self.asked?;
        let answer = match self.answers.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(GONE.to_string()),
        };
        self.asked = None;
        Some(answer)
    }

    /// [`Desk::answer`], waited for: the launch's open, which the engine
    /// cannot start without. `None` when `patience` ran out first — the
    /// open is still in flight, and its answer still comes.
    pub(crate) fn wait(&mut self, patience: Duration) -> Option<Result<Output, String>> {
        self.asked?;
        let answer = match self.answers.recv_timeout(patience) {
            Ok(answer) => answer,
            Err(mpsc::RecvTimeoutError::Timeout) => return None,
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(GONE.to_string()),
        };
        self.asked = None;
        Some(answer)
    }

    /// The system default's identity, as of the device thread's last look.
    pub(crate) fn default_now(&self) -> Option<String> {
        self.default.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

/// What an open comes to when the device thread is no longer there to make
/// it.
const GONE: &str = "the audio device thread is gone";

/// Where the device thread goes once its engine is gone. Nowhere, on
/// Windows: cpal initializes COM on the thread that calls it and
/// uninitializes it when that thread ends, and in a process with no window
/// this is the only such thread — the last apartment closing lets COM
/// unload the audio libraries, while cpal's device enumerator (one for the
/// process, made on the first thread to ask) and any stream still being
/// closed go on holding objects from them. The thread that made them stays
/// for as long as the process does, parked, which costs nothing.
#[cfg(windows)]
fn stay() {
    loop {
        std::thread::park();
    }
}

/// Elsewhere it has nothing to keep alive, and ends.
#[cfg(not(windows))]
fn stay() {}

/// The device thread: every call into the system's audio service is made
/// here, one at a time. Between jobs it looks up the default output once
/// per [`LOOK`]. On Windows this is also the thread cpal initializes
/// COM on, which is why it outlives its engine there ([`stay`]).
fn run(
    mut backend: Box<dyn Backend>,
    bus: &Bus,
    jobs: &mpsc::Receiver<Job>,
    answers: &mpsc::Sender<Result<Output, String>>,
    default: &Mutex<Option<String>>,
) {
    // What the last failed open said. An outage is the same refusal every
    // couple of seconds for as long as it lasts, and the flight recorder
    // needs it once — unless a knock was slow, which is news each time.
    let mut last_failure: Option<String> = None;
    loop {
        let opened = match jobs.recv_timeout(LOOK) {
            Ok(Job::Open { others }) => {
                let started = Instant::now();
                let opened = caught(|| backend.open(bus, others)).unwrap_or_else(Err);
                let took = started.elapsed();
                match &opened {
                    Ok(output) => {
                        etrace!("output open took {} ms: on {}", took.as_millis(), output.name());
                        last_failure = None;
                    }
                    Err(e) => {
                        if last_failure.as_ref() != Some(e) || took >= SLOW_FAILURE {
                            etrace!("output open took {} ms: FAILED: {e}", took.as_millis());
                        }
                        last_failure = Some(e.clone());
                    }
                }
                Some(opened)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            // The engine is gone.
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        // The default as it stands now, published before the answer: the
        // engine reads what the new stream opened for against it.
        let seen = caught(|| backend.default_identity()).unwrap_or(None);
        *default.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = seen;
        if let Some(opened) = opened {
            if let Err(mpsc::SendError(unwanted)) = answers.send(opened) {
                // Nobody left to play it.
                if let Ok(output) = unwanted {
                    retire(output);
                }
                return;
            }
        }
    }
}

/// Run a call into the backend, turning a panic under it into the words it
/// panicked with.
fn caught<T>(call: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).map_err(|panic| {
        let said = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("no message");
        format!("the audio backend gave up: {said}")
    })
}

/// The system's devices, through cpal.
#[derive(Default)]
pub(crate) struct System {
    /// What the devices that refused said on the last walk: said again
    /// only when it changes, so an outage's knocks do not fill the flight
    /// recorder with one line each.
    refusals: Vec<String>,
}

impl Backend for System {
    fn open(&mut self, bus: &Bus, others: bool) -> Result<Output, String> {
        let mut refusals = Vec::new();
        let opened = open_walk(bus, others, &mut refusals);
        if refusals != self.refusals {
            for refusal in &refusals {
                etrace!("{refusal}");
            }
            self.refusals = refusals;
        }
        opened
    }

    fn default_identity(&mut self) -> Option<String> {
        quiet_alsa(|| identity(&cpal::default_host().default_output_device()?))
    }
}

/// A device's stable identity when the backend has one (host + endpoint
/// id), its name when not. Only ever compared for equality; two devices
/// sharing a name on a backend without ids is a change this cannot see,
/// which costs a missed move there and nothing anywhere else.
fn identity(device: &cpal::Device) -> Option<String> {
    device
        .id()
        .ok()
        .map(|id| id.to_string())
        .or_else(|| device.description().ok().map(|d| d.name().to_string()))
}

fn name_of(device: &cpal::Device) -> String {
    device
        .description()
        .ok()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|| "unnamed output".to_string())
}

/// Run `f` with fd 2 pointed at /dev/null, then put it back. libasound
/// writes its configuration complaints from C, straight to fd 2 — on a box
/// with no sound card every device walk prints "ALSA lib confmisc.c:…
/// cannot find card '0'" and friends onto the TUI's screen, past the
/// recorder that keeps cpal's own errors off it. Process-wide for the
/// few milliseconds of each call into the backend — never across the wait
/// for a stream's first buffer, which can run to seconds; nothing else may
/// write to the screen's stderr while the TUI owns it anyway.
#[cfg(target_os = "linux")]
fn quiet_alsa<T>(f: impl FnOnce() -> T) -> T {
    use std::os::fd::AsRawFd;
    let Ok(null) = std::fs::OpenOptions::new().write(true).open("/dev/null") else {
        return f();
    };
    // SAFETY: dup/dup2/close on fd 2 and descriptors this function owns;
    // the saved copy is put back on fd 2 and closed whatever `f` does.
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    if saved < 0 {
        return f();
    }
    struct Restore(i32);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.0, libc::STDERR_FILENO);
                libc::close(self.0);
            }
        }
    }
    let _restore = Restore(saved);
    unsafe { libc::dup2(null.as_raw_fd(), libc::STDERR_FILENO) };
    f()
}

/// Elsewhere the backend keeps its own counsel: nothing to quiet.
#[cfg(not(target_os = "linux"))]
fn quiet_alsa<T>(f: impl FnOnce() -> T) -> T {
    f()
}

/// How many backend-specific stream errors one stream may report before
/// they count as the device dying. The variant has no single meaning —
/// ALSA reports a vanished raw hw device this way (`DeviceNotAvailable`
/// is not in its vocabulary; the desktop path never needs it because
/// PipeWire and PulseAudio move streams themselves), and every host uses
/// it for recoverable hiccups too. One is noise; a run of them is a
/// stream that is not coming back, and the new stream that answers is
/// self-limiting either way — it reopens in place and the count starts
/// over with it.
#[cfg(not(windows))]
const BACKEND_ERROR_LIMIT: u32 = 5;
/// On Windows the first is the last: cpal's WASAPI thread leaves its loop
/// on any error it reports, whatever the variant, so a stream that has
/// reported one will never ask for another buffer.
#[cfg(windows)]
const BACKEND_ERROR_LIMIT: u32 = 1;

/// Open an output on the current default device — and, when that will not
/// open and `others` allows it, on the first other device that will: the
/// same walk rodio's `open_default_sink` makes, rebuilt here because that
/// convenience hardcodes an error callback that `eprintln!`s straight
/// onto the TUI's alternate screen (the raw "no longer available. For
/// example, it has been unplugged." a pulled headphone jack used to
/// smear across the UI), because the watch has to ride along, and
/// because the stream itself has to stay ours to suspend. libasound's own
/// complaints during the walk stay off the screen too ([`quiet_alsa`]).
///
/// The other devices are only looked at once the default has refused — or
/// taken the stream and then never run it: a default that opens, which is
/// every open but the odd one, costs nothing but itself. A device that
/// carries the session in the default's place is a stand-in, and the
/// engine goes on trying the default for a while (`MOVE_TRIES`).
fn open_walk(bus: &Bus, others: bool, refusals: &mut Vec<String>) -> Result<Output, String> {
    let host = cpal::default_host();
    let (default_device, default_at_open) = quiet_alsa(|| {
        let device = host.default_output_device();
        let identity = device.as_ref().and_then(identity);
        (device, identity)
    });

    let mut first_err = match default_device {
        Some(device) => {
            match open_device(&device, bus, default_at_open.clone(), refusals) {
                Ok(output) => return Ok(output),
                Err(why) => Some(why),
            }
        }
        None => None,
    };
    if !others {
        return Err(first_err.unwrap_or_else(|| "no default output device".to_string()));
    }
    // The default has had its turn; the rest are tried minus ALSA's "null"
    // driver, which opens happily and plays to nowhere (rodio's own
    // fallback applies the same filter). As the *chosen* default it was
    // eligible — headless boxes route through it on purpose.
    let rest: Vec<cpal::Device> = quiet_alsa(|| {
        let Ok(devices) = host.output_devices() else { return Vec::new() };
        devices
            .filter(|device| {
                let is_default =
                    default_at_open.is_some() && identity(device) == default_at_open;
                let real = device
                    .description()
                    .map(|d| d.driver().is_some_and(|drv| drv != "null"))
                    .unwrap_or(false);
                !is_default && real
            })
            .collect()
    });
    for device in rest {
        match open_device(&device, bus, default_at_open.clone(), refusals) {
            Ok(output) => return Ok(output),
            Err(why) => {
                first_err.get_or_insert(why);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| "no output device found".to_string()))
}

/// One device's open, start to proof: a stream reading the bus, started,
/// that has asked for its first buffer.
fn open_device(
    device: &cpal::Device,
    bus: &Bus,
    default_at_open: Option<String>,
    refusals: &mut Vec<String>,
) -> Result<Output, String> {
    let (name, identity) = quiet_alsa(|| (name_of(device), identity(device)));
    let id = next_id();
    let pulse = Arc::new(Pulse::new());
    let on_error = error_callback(name.clone(), pulse.clone());
    let opened = quiet_alsa(|| open_on(device, on_error, bus, id, &pulse));
    let (stream, config, format) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            refusals.push(format!("output would not open on {name}: {e}"));
            return Err(e);
        }
    };
    if !pulse.first_pull_within(FIRST_PULL) {
        refusals.push(format!("output on {name} started and never asked for a buffer"));
        // Closing it may wait on whatever is keeping it from running.
        retire(stream);
        return Err(format!("{name} is not answering"));
    }
    etrace!(
        "output opened on {name} ({} Hz, {} ch, {format})",
        config.sample_rate,
        config.channels
    );
    Ok(Output {
        id,
        name,
        device: identity,
        default_at_open,
        pulse,
        awake: true,
        stream: Stream::Device(stream),
    })
}

/// The error callback every stream carries: the death tripwire's trigger.
fn error_callback(
    heard_on: String,
    pulse: Arc<Pulse>,
) -> impl FnMut(cpal::StreamError) + Clone + Send + 'static {
    let errors = Arc::new(AtomicU32::new(0));
    move |err: cpal::StreamError| {
        // Runs on the OS audio thread: the recorder and an atomic, nothing
        // that can block or re-enter.
        crate::stderrln!("[engine] output stream error on {heard_on}: {err}");
        match err {
            cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated => {
                pulse.mark_gone();
            }
            // An underrun hurts the ear once and heals.
            cpal::StreamError::BufferUnderrun => {}
            // The grab-bag: log-only until the run of them says the stream
            // is gone (raw-ALSA unplug's shape — see BACKEND_ERROR_LIMIT).
            cpal::StreamError::BackendSpecific { .. } => {
                if errors.fetch_add(1, Ordering::Relaxed) + 1 >= BACKEND_ERROR_LIMIT {
                    pulse.mark_gone();
                }
            }
        }
    }
}

/// Why one configuration did not start.
struct NoStart {
    why: String,
    /// The device itself is gone: no other configuration will do better.
    gone: bool,
}

/// One device's open, as rodio makes it (`DeviceSinkBuilder::from_device`
/// then `open_sink_or_fallback`) but keeping the stream: the device's
/// default config with rodio's ~50 ms fixed buffer first, then the same at
/// the device's own buffer size — a fixed size is refused by some devices —
/// and then the first few other configs the device lists, in rodio's
/// order ([`OTHER_CONFIGS`]). A device that says it is gone is not asked
/// again, nor one that has spent its budget refusing ([`DEVICE_BUDGET`]).
fn open_on<E>(
    device: &cpal::Device,
    on_error: E,
    bus: &Bus,
    id: u64,
    pulse: &Arc<Pulse>,
) -> Result<(cpal::Stream, StreamConfig, SampleFormat), String>
where
    E: FnMut(cpal::StreamError) + Clone + Send + 'static,
{
    let default = device.default_output_config().map_err(|e| e.to_string())?;
    let format = default.sample_format();
    let preferred = StreamConfig {
        channels: default.channels(),
        sample_rate: default.sample_rate(),
        buffer_size: BufferSize::Fixed(nearest_power_of_two(default.sample_rate() / 20)),
    };
    let plain = StreamConfig { buffer_size: BufferSize::Default, ..preferred.clone() };
    let attempt = |format: SampleFormat, config: StreamConfig| {
        start(device, format, &config, on_error.clone(), bus.clone(), id, pulse.clone())
            .map(|stream| (stream, config, format))
    };

    let started = Instant::now();
    let spent = || started.elapsed() >= DEVICE_BUDGET;
    let first = match attempt(format, preferred) {
        Ok(opened) => return Ok(opened),
        Err(refusal) => refusal,
    };
    if first.gone || spent() {
        return Err(first.why);
    }
    match attempt(format, plain.clone()) {
        Ok(opened) => return Ok(opened),
        Err(refusal) if refusal.gone => return Err(first.why),
        Err(_) => {}
    }
    let Ok(supported) = rodio::stream::supported_output_configs(device) else {
        return Err(first.why);
    };
    let mut tried = vec![(plain.channels, plain.sample_rate, format)];
    for config in supported {
        let shape = (config.channels(), config.sample_rate(), config.sample_format());
        if tried.contains(&shape) {
            continue;
        }
        if tried.len() > OTHER_CONFIGS || spent() {
            break;
        }
        tried.push(shape);
        match attempt(config.sample_format(), config.config()) {
            Ok(opened) => return Ok(opened),
            Err(refusal) if refusal.gone => break,
            Err(_) => {}
        }
    }
    Err(first.why)
}

/// Build and start one stream whose callback asks the bus for every buffer
/// — in whichever sample format the device asked for, and under the pulse.
fn start<E>(
    device: &cpal::Device,
    format: SampleFormat,
    config: &StreamConfig,
    on_error: E,
    bus: Bus,
    id: u64,
    pulse: Arc<Pulse>,
) -> Result<cpal::Stream, NoStart>
where
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    let refused = |why: &str| NoStart { why: why.to_string(), gone: false };
    let shape: Format = (
        NonZero::new(config.channels).ok_or_else(|| refused("the device offers no channels"))?,
        NonZero::new(config.sample_rate)
            .ok_or_else(|| refused("the device offers no sample rate"))?,
    );
    macro_rules! build {
        ($($format:ident => $ty:ty),+ $(,)?) => {
            match format {
                $(SampleFormat::$format => device.build_output_stream::<$ty, _, _>(
                    config,
                    move |data: &mut [$ty], _| pulse.beat(|| bus.fill(id, shape, data)),
                    on_error,
                    None,
                ),)+
                other => return Err(refused(&format!("unsupported sample format {other}"))),
            }
        };
    }
    let stream = build!(
        F32 => f32,
        F64 => f64,
        I8 => i8,
        I16 => i16,
        I24 => cpal::I24,
        I32 => i32,
        I64 => i64,
        U8 => u8,
        U16 => u16,
        U24 => cpal::U24,
        U32 => u32,
        U64 => u64,
    )
    .map_err(|e| NoStart {
        gone: matches!(e, cpal::BuildStreamError::DeviceNotAvailable),
        why: e.to_string(),
    })?;
    stream.play().map_err(|e| NoStart {
        gone: matches!(e, cpal::PlayStreamError::DeviceNotAvailable),
        why: e.to_string(),
    })?;
    Ok(stream)
}

/// The power of two nearest `n`, ties going down — rodio's rounding for
/// its buffer (`math::nearest_multiple_of_two`, private to rodio): 2048
/// frames at 44.1 and 48 kHz, where 50 ms would be 2205 and 2400.
fn nearest_power_of_two(n: u32) -> u32 {
    if n <= 1 {
        return 1;
    }
    let next = n.next_power_of_two();
    let prev = next >> 1;
    if n - prev <= next - n { prev } else { next }
}

/// Devices made of threads, for tests: a stage's stream asks the bus for
/// buffers at a device's pace, and a test says when the default moves, how
/// long an open takes, and how a stream dies — with an error, as CoreAudio
/// reports a pulled device, or without a word, as WASAPI's does. Nothing
/// here needs a sound card, so what the engine does about a device changing
/// is tested on every machine the suite runs on.
#[cfg(test)]
pub(crate) mod stage {
    use super::*;

    /// A stage's script, shared by the test and the backend it hands out.
    #[derive(Clone)]
    pub(crate) struct Stage(Arc<Mutex<Plot>>);

    struct Plot {
        /// The system default, by name; a device's name is its identity.
        default: Option<String>,
        /// A second device that opens whenever it is tried.
        spare: Option<String>,
        /// What the default says when asked to open, if it refuses.
        refuses: Option<String>,
        /// How long an open takes before it answers.
        takes: Duration,
        /// Whether a stream opened from here on ever asks for a buffer.
        silent: bool,
        /// How long closing a stream takes.
        closing_takes: Duration,
        /// The shape streams open in.
        shape: Format,
        /// How many opens have been asked for, answered or not.
        asked: usize,
        /// Every stream opened, newest last.
        streams: Vec<(String, Arc<Flags>)>,
    }

    /// One staged stream's switches.
    struct Flags {
        playing: AtomicBool,
        /// Stops asking for buffers, and says nothing.
        mute: AtomicBool,
        closed: AtomicBool,
        pulse: Arc<Pulse>,
    }

    impl Stage {
        /// A stage whose default device is `default`: 44.1 kHz stereo,
        /// opening at once.
        pub(crate) fn new(default: &str) -> Stage {
            Stage(Arc::new(Mutex::new(Plot {
                default: Some(default.to_string()),
                spare: None,
                refuses: None,
                takes: Duration::ZERO,
                silent: false,
                closing_takes: Duration::ZERO,
                shape: (NonZero::new(2).unwrap(), NonZero::new(44_100).unwrap()),
                asked: 0,
                streams: Vec::new(),
            })))
        }

        pub(crate) fn backend(&self) -> Box<dyn Backend> {
            Box::new(self.clone())
        }

        fn plot(&self) -> std::sync::MutexGuard<'_, Plot> {
            self.0.lock().unwrap()
        }

        /// The system default becomes `name` — or nothing at all.
        pub(crate) fn set_default(&self, name: Option<&str>) {
            self.plot().default = name.map(str::to_string);
        }

        /// A second device, `name`, that opens whenever the walk reaches it.
        pub(crate) fn set_spare(&self, name: Option<&str>) {
            self.plot().spare = name.map(str::to_string);
        }

        /// The default refuses to open, saying `why` — or opens again.
        pub(crate) fn refuse(&self, why: Option<&str>) {
            self.plot().refuses = why.map(str::to_string);
        }

        /// Opens take this long to answer from here on.
        pub(crate) fn opens_take(&self, takes: Duration) {
            self.plot().takes = takes;
        }

        /// Streams opened from here on never ask for a buffer — or do.
        pub(crate) fn open_silent(&self, silent: bool) {
            self.plot().silent = silent;
        }

        /// Closing a stream takes this long from here on.
        pub(crate) fn closing_takes(&self, takes: Duration) {
            self.plot().closing_takes = takes;
        }

        /// Streams open in this shape from here on.
        pub(crate) fn set_shape(&self, channels: u16, rate: u32) {
            self.plot().shape = (NonZero::new(channels).unwrap(), NonZero::new(rate).unwrap());
        }

        /// The newest stream's device is pulled, and says so: the error
        /// callback's flag goes up and no more buffers are asked for.
        pub(crate) fn unplug(&self) {
            if let Some((_, flags)) = self.plot().streams.last() {
                flags.mute.store(true, Ordering::Release);
                flags.pulse.mark_gone();
            }
        }

        /// The newest stream stops asking for buffers and says nothing —
        /// WASAPI's way with an endpoint that went away.
        pub(crate) fn go_quiet(&self) {
            if let Some((_, flags)) = self.plot().streams.last() {
                flags.mute.store(true, Ordering::Release);
            }
        }

        /// The newest stream asks for buffers again, as one does when the
        /// machine it runs on wakes.
        pub(crate) fn speak_again(&self) {
            if let Some((_, flags)) = self.plot().streams.last() {
                flags.mute.store(false, Ordering::Release);
            }
        }

        /// How many opens have been asked for.
        pub(crate) fn asked(&self) -> usize {
            self.plot().asked
        }

        /// The devices with a stream still open on them, oldest first.
        pub(crate) fn open_on(&self) -> Vec<String> {
            self.plot()
                .streams
                .iter()
                .filter(|(_, flags)| !flags.closed.load(Ordering::Acquire))
                .map(|(name, _)| name.clone())
                .collect()
        }
    }

    impl Backend for Stage {
        fn open(&mut self, bus: &Bus, others: bool) -> Result<Output, String> {
            let (default, spare, refuses, takes) = {
                let mut plot = self.plot();
                plot.asked += 1;
                (plot.default.clone(), plot.spare.clone(), plot.refuses.clone(), plot.takes)
            };
            std::thread::sleep(takes);
            let mut first_err = None;
            if let Some(name) = &default {
                match &refuses {
                    Some(why) => first_err = Some(why.clone()),
                    None => match self.raise(bus, name, default.clone()) {
                        Ok(output) => return Ok(output),
                        Err(why) => first_err = Some(why),
                    },
                }
            }
            match spare {
                Some(name) if others => self.raise(bus, &name, default),
                _ => Err(first_err.unwrap_or_else(|| "no output device found".to_string())),
            }
        }

        fn default_identity(&mut self) -> Option<String> {
            self.plot().default.clone()
        }
    }

    impl Stage {
        /// One staged stream, started — and proven, as a real one is.
        fn raise(
            &self,
            bus: &Bus,
            name: &str,
            default_at_open: Option<String>,
        ) -> Result<Output, String> {
            let (shape, silent, closing_takes) = {
                let plot = self.plot();
                (plot.shape, plot.silent, plot.closing_takes)
            };
            let id = next_id();
            let pulse = Arc::new(Pulse::new());
            let flags = Arc::new(Flags {
                playing: AtomicBool::new(true),
                mute: AtomicBool::new(silent),
                closed: AtomicBool::new(false),
                pulse: pulse.clone(),
            });
            self.plot().streams.push((name.to_string(), flags.clone()));
            let puller = Puller::start(bus.clone(), id, shape, flags, closing_takes);
            if !pulse.first_pull_within(FIRST_PULL) {
                retire(puller);
                return Err(format!("{name} is not answering"));
            }
            Ok(Output {
                id,
                name: name.to_string(),
                device: Some(name.to_string()),
                default_at_open,
                pulse,
                awake: true,
                stream: Stream::Staged(puller),
            })
        }
    }

    /// A staged stream: a thread asking the bus for ten milliseconds of
    /// sound every ten milliseconds, as a device's callback would.
    pub(crate) struct Puller {
        flags: Arc<Flags>,
        closing_takes: Duration,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Puller {
        fn start(
            bus: Bus,
            id: u64,
            shape: Format,
            flags: Arc<Flags>,
            closing_takes: Duration,
        ) -> Puller {
            let theirs = flags.clone();
            let thread = std::thread::spawn(move || {
                let period = Duration::from_millis(10);
                let frames = shape.1.get() as usize / 100;
                let mut buffer = vec![0.0f32; frames * usize::from(shape.0.get())];
                let mut due = Instant::now();
                while !theirs.closed.load(Ordering::Acquire) {
                    let asks = theirs.playing.load(Ordering::Acquire)
                        && !theirs.mute.load(Ordering::Acquire);
                    if asks {
                        theirs.pulse.beat(|| bus.fill(id, shape, &mut buffer));
                    }
                    // A device's clock does not wait for a late callback:
                    // a pull that overran is followed by the ones it owes.
                    due += period;
                    let now = Instant::now();
                    if due > now {
                        std::thread::sleep(due - now);
                    } else if now - due > Duration::from_millis(200) {
                        due = now;
                    }
                }
            });
            Puller { flags, closing_takes, thread: Some(thread) }
        }

        pub(super) fn play(&self) -> Result<(), String> {
            self.flags.playing.store(true, Ordering::Release);
            Ok(())
        }

        pub(super) fn pause(&self) -> Result<(), String> {
            self.flags.playing.store(false, Ordering::Release);
            Ok(())
        }
    }

    impl Drop for Puller {
        fn drop(&mut self) {
            std::thread::sleep(self.closing_takes);
            self.flags.closed.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_rounds_the_way_rodio_rounds_it() {
        // 50 ms at the common rates: rodio's from_device lands every one of
        // these on 2048 frames (4096 at 96 kHz), and the owned open has to
        // ask the device for the same buffer rodio's would have.
        assert_eq!(nearest_power_of_two(44_100 / 20), 2048);
        assert_eq!(nearest_power_of_two(48_000 / 20), 2048);
        assert_eq!(nearest_power_of_two(96_000 / 20), 4096);
        assert_eq!(nearest_power_of_two(8_000 / 20), 512);
        // Ties go down, as rodio's do: 3072 sits exactly between.
        assert_eq!(nearest_power_of_two(3072), 2048);
        assert_eq!(nearest_power_of_two(1), 1);
        assert_eq!(nearest_power_of_two(0), 1);
    }

    #[test]
    fn a_pulse_tells_a_callback_that_is_running_from_one_that_stopped() {
        let pulse = Pulse::new();
        let fresh = Duration::from_millis(80);
        assert!(!pulse.answers(fresh), "nothing has pulled yet");
        assert!(!pulse.first_pull_within(Duration::from_millis(20)));

        pulse.beat(|| {});
        assert!(pulse.answers(fresh), "a pull just made");
        assert!(pulse.first_pull_within(Duration::ZERO));
        assert!(pulse.quiet_for() < fresh);

        // No more pulls: the quiet grows, and the callback stops counting
        // as there.
        std::thread::sleep(Duration::from_millis(120));
        assert!(!pulse.answers(fresh), "the last pull is too long ago");
        assert!(pulse.quiet_for() >= Duration::from_millis(100));

        // Woken: the quiet is counted from the wake, not from before the
        // sleep — but nothing has answered yet.
        pulse.expect_from_now();
        assert!(pulse.quiet_for() < fresh);
        assert!(!pulse.answers(fresh));

        // Inside a pull the callback is there, however long the pull runs.
        pulse.beat(|| {
            std::thread::sleep(Duration::from_millis(120));
            assert!(pulse.answers(fresh), "mid-pull");
        });
    }

    #[test]
    fn the_device_thread_answers_one_open_at_a_time_and_survives_a_backend_that_panics() {
        struct Shaky {
            calls: usize,
        }
        impl Backend for Shaky {
            fn open(&mut self, _: &Bus, _: bool) -> Result<Output, String> {
                self.calls += 1;
                if self.calls == 1 {
                    panic!("could not open property store");
                }
                Err(format!("call {}", self.calls))
            }
            fn default_identity(&mut self) -> Option<String> {
                Some(format!("default after {}", self.calls))
            }
        }

        let bus = Bus::new();
        let mut desk = Desk::start(Box::new(Shaky { calls: 0 }), &bus).unwrap();
        assert!(desk.answer().is_none(), "nothing asked, nothing answered");

        desk.ask(true);
        desk.ask(true);
        let first = desk.wait(Duration::from_secs(5)).expect("answered");
        let Err(first) = first else { panic!("the panicking open is a failed open") };
        assert!(first.contains("could not open property store"), "{first}");
        assert_eq!(desk.default_now().as_deref(), Some("default after 1"));

        // The second ask was dropped while the first was out; the thread is
        // still there for the next.
        assert!(desk.wait(Duration::from_millis(50)).is_none());
        desk.ask(false);
        let second = desk.wait(Duration::from_secs(5)).expect("answered");
        assert_eq!(second.err().as_deref(), Some("call 2"));
    }

    #[test]
    fn a_staged_stream_plays_the_bus_until_it_is_told_to_stop() {
        let stage = stage::Stage::new("Speakers");
        let bus = Bus::new();
        let mut backend = stage.backend();
        let mut output = backend.open(&bus, true).expect("the stage opens");
        assert_eq!(output.name(), "Speakers");
        assert_eq!(output.default_it_is_on(), Some("Speakers"));
        assert!(output.is_awake());
        assert!(output.pulse().answers(Duration::from_millis(200)));

        // Suspended, it stops asking; woken, it asks again.
        assert!(output.suspend());
        std::thread::sleep(Duration::from_millis(150));
        assert!(!output.pulse().answers(Duration::from_millis(100)));
        output.wake();
        std::thread::sleep(Duration::from_millis(60));
        assert!(output.pulse().answers(Duration::from_millis(100)));

        // Pulled: dead, and it says so.
        stage.unplug();
        assert!(output.is_dead());
        assert!(!output.suspend(), "a dead stream is not suspended");
        drop(output);
        assert!(stage.open_on().is_empty(), "the stream closed with its output");

        // A default that will not open: the spare carries the session when
        // other devices may be tried, and nothing does when they may not.
        stage.refuse(Some("mid-change"));
        stage.set_spare(Some("Monitor"));
        assert_eq!(backend.open(&bus, false).err().as_deref(), Some("mid-change"));
        let spare = backend.open(&bus, true).expect("the spare opens");
        assert_eq!(spare.name(), "Monitor");
        assert_eq!(spare.default_it_is_on(), None, "not the default's device");

        // A stream that never asks for a buffer is no output at all.
        stage.refuse(None);
        stage.open_silent(true);
        let silent = backend.open(&bus, true).err().expect("unproven");
        assert!(silent.contains("not answering"), "{silent}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod alsa_tests {
    use super::quiet_alsa;

    /// fd 2's (device, inode), to tell which file it is.
    fn stderr_file() -> (u64, u64) {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(libc::STDERR_FILENO, &mut st) }, 0);
        (st.st_dev as u64, st.st_ino as u64)
    }

    #[test]
    fn the_alsa_walk_writes_to_dev_null_and_gives_fd_2_back() {
        let before = stderr_file();
        let null = {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata("/dev/null").expect("/dev/null");
            (m.dev(), m.ino())
        };
        let inside = quiet_alsa(stderr_file);
        assert_eq!(inside, null, "the walk's fd 2 is /dev/null");
        assert_eq!(stderr_file(), before, "and fd 2 is the screen's again after");
        // A panicking walk puts it back too.
        let _ = std::panic::catch_unwind(|| quiet_alsa(|| panic!("the walk failed")));
        assert_eq!(stderr_file(), before);
    }
}
