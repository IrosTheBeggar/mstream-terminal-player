//! Playback core: rodio sink management, queue bookkeeping, transport state.
//!
//! Ported from mStream's rust-server-audio (mStream@bec11154). Behavior-compatible
//! with the original except for the audit fixes listed in PLAN.md: volume persists
//! across track changes, manual next/previous bypass loop-one, device failures are
//! errors instead of panics, and removing the current queue entry while stopped no
//! longer starts playback.
//!
//! Phase 2: sources may be local paths or http(s) URLs. URLs stream through
//! stream-download (buffered Read+Seek over range requests — see http.rs).
//! Queue entries carry an optional duration hint so remote tracks don't need
//! a costly second fetch to probe duration.

pub(crate) mod fade;
pub(crate) mod http;
pub(crate) mod tap;

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use rodio::mixer::Mixer;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use serde::Serialize;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

// ── Loop mode ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoopMode {
    None,
    One,
    All,
}

impl LoopMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LoopMode::None => "none",
            LoopMode::One => "one",
            LoopMode::All => "all",
        }
    }
    pub fn next(&self) -> LoopMode {
        match self {
            LoopMode::None => LoopMode::One,
            LoopMode::One => LoopMode::All,
            LoopMode::All => LoopMode::None,
        }
    }
}

/// The one place the serve API's loop mode meets the shared advance rules.
impl From<LoopMode> for crate::advance::Loop {
    fn from(mode: LoopMode) -> Self {
        match mode {
            LoopMode::None => crate::advance::Loop::Off,
            LoopMode::One => crate::advance::Loop::One,
            LoopMode::All => crate::advance::Loop::All,
        }
    }
}

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum EngineError {
    /// Output device could not be opened (missing, removed, or busy).
    NoDevice(String),
    /// Source could not be opened, fetched, or decoded. Carries the reason
    /// for logs/CLI; the serve API maps this to its historical route-specific
    /// message regardless.
    Unplayable(String),
    OutOfBounds,
    EndOfQueue,
    Seek(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NoDevice(e) => write!(f, "Audio device unavailable: {}", e),
            EngineError::Unplayable(e) => write!(f, "Source could not be played: {}", e),
            EngineError::OutOfBounds => write!(f, "Index out of bounds"),
            EngineError::EndOfQueue => write!(f, "Already at end of queue"),
            EngineError::Seek(e) => write!(f, "Seek failed: {}", e),
        }
    }
}

impl std::error::Error for EngineError {}

// ── Queue bookkeeping (kept free of audio handles so it unit-tests without a device) ──

#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub path: String,
    /// Known duration in seconds (e.g. from mStream's DB). Spares remote
    /// sources a second fetch just to probe duration.
    pub duration_hint: Option<f64>,
}

impl QueueEntry {
    pub fn new(path: String) -> Self {
        QueueEntry { path, duration_hint: None }
    }
}

#[derive(Debug)]
pub(crate) struct QueueState {
    pub queue: Vec<QueueEntry>,
    pub index: usize,
    pub shuffle: bool,
    pub loop_mode: LoopMode,
}

/// Pick the next index based on shuffle/loop settings. The rules live in
/// [`crate::advance`], shared with the TUI's queue — this used to be its own
/// copy, synced by hand (audit #62). Nobody counts the shuffle pass here, so
/// a shuffled queue under loop=none still never ends (audit #9, deferred).
pub(crate) fn pick_next(q: &QueueState, manual: bool) -> Option<usize> {
    crate::advance::pick_next(
        q.queue.len(),
        Some(q.index),
        q.shuffle,
        q.loop_mode.into(),
        manual,
        None,
    )
}

pub(crate) use crate::advance::RemoveOutcome;

/// Remove `index` (caller has bounds-checked) and fix up the current index.
/// The arithmetic is [`crate::advance::shift_current`]'s, shared with the
/// TUI's queue; keeping the clamped index and restarting in place on
/// `RemovedCurrent` is this side's policy.
pub(crate) fn apply_remove(q: &mut QueueState, index: usize) -> RemoveOutcome {
    q.queue.remove(index);
    let (shifted, outcome) = crate::advance::shift_current(q.queue.len(), q.index, index);
    q.index = shifted;
    outcome
}

// ── Status snapshots ────────────────────────────────────────────────────────

/// Wire-compatible with rust-server-audio's StatusResponse.
#[derive(Serialize)]
pub struct Status {
    pub playing: bool,
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f32,
    pub file: String,
    pub queue_index: usize,
    pub queue_length: usize,
    pub shuffle: bool,
    pub loop_mode: String,
}

/// Wire-compatible with rust-server-audio's QueueResponse.
#[derive(Serialize)]
pub struct QueueSnapshot {
    pub queue: Vec<String>,
    pub current_index: usize,
}

// ── Player state ────────────────────────────────────────────────────────────

/// A decoded source ready to attach to a fresh sink. Two concrete decoder
/// types (local file vs HTTP reader) — kept as an enum so both open paths
/// share the sink-swap tail without generic-bound gymnastics.
enum Opened {
    Local(Decoder<BufReader<File>>),
    Http(Decoder<http::HttpReader>),
}

/// One source opened, probed and spooling: what [`open_entry`] returns,
/// whether called inline by start_current or from a prepare thread.
struct Prepared {
    opened: Opened,
    path: String,
    duration: f64,
}

/// Open and decode one queue entry, blocking for as long as the source
/// needs — seconds, over a network. Nothing in here touches the state lock:
/// start_current calls it with the lock held (preserving the original's
/// open-then-swap ordering), the prepare thread calls it with no lock at
/// all, which is the whole reason preparing does not freeze the controls
/// (the lesson of findings #48–#50).
///
/// The decoder is always given `byte_len` when we know it (file metadata
/// or HTTP Content-Length): symphonia's FLAC reader needs the total
/// length for binary-search seeking on files without a SEEKTABLE block —
/// rodio 0.20 hardcoded byte_len to None, which made those files
/// unseekable (audit finding #13).
fn open_entry(entry: &QueueEntry) -> Result<Prepared, String> {
    let path = entry.path.clone();
    // Diagnostics here go through `stderrln!`: these fire mid-session
    // from audio-side threads, and in the TUI that stderr lands raw on
    // the alternate screen — where the same news already arrives as
    // PlaybackFailed. Serve and the CLI keep the lines (audit #43).
    let (opened, duration) = if http::is_http_url(&path) {
        let redacted = http::redact_source(&path);
        let (reader, content_length) = http::open(&path).map_err(|e| {
            crate::stderrln!("[engine] open failed for {}: {}", redacted, e);
            e
        })?;
        if content_length.is_none() {
            crate::stderrln!(
                "[engine] {}: no content length — seek limited to downloaded data",
                redacted
            );
        }
        let mut builder = Decoder::builder().with_data(reader).with_seekable(true);
        if let Some(len) = content_length {
            builder = builder.with_byte_len(len);
        }
        let decoder = builder.build().map_err(|e| {
            crate::stderrln!("[engine] decode failed for {}: {}", redacted, e);
            e.to_string()
        })?;
        let duration = entry
            .duration_hint
            .or_else(|| decoder.total_duration().map(|d| d.as_secs_f64()))
            .unwrap_or(0.0);
        (Opened::Http(decoder), duration)
    } else {
        let file = File::open(&path).map_err(|e| {
            crate::stderrln!("[engine] open failed for {}: {}", path, e);
            e.to_string()
        })?;
        let byte_len = file.metadata().ok().map(|m| m.len());
        let mut builder =
            Decoder::builder().with_data(BufReader::new(file)).with_seekable(true);
        if let Some(len) = byte_len {
            builder = builder.with_byte_len(len);
        }
        let decoder = builder.build().map_err(|e| {
            crate::stderrln!("[engine] decode failed for {}: {}", path, e);
            e.to_string()
        })?;
        let duration = entry.duration_hint.unwrap_or_else(|| probe_duration(&path));
        (Opened::Local(decoder), duration)
    };
    Ok(Prepared { opened, path, duration })
}

// ── Crossfade machinery ─────────────────────────────────────────────────────

/// How long before the fade window the next track's open begins. Covers
/// http::OPEN_TIMEOUT plus symphonia's probe with room left over; the cost
/// of being early is only that the next track starts spooling sooner.
const PREPARE_LEAD: f64 = 12.0;

/// The floor under a blend that arrives with almost nothing left to blend
/// with — below this it is a cut with extra steps, and cuts click.
const MIN_FADE: f64 = 0.05;

/// Wall-clock net past the fade window before a lingering outgoing sink is
/// cut whatever its other signals say. Generous: it exists for pathology,
/// not for scheduling.
const OUTGOING_SLACK: Duration = Duration::from_secs(2);

/// The breath on a manual track change — long enough to kill the click of
/// cutting a waveform mid-swing, short enough to feel instant.
const SKIP_FADE: Duration = Duration::from_millis(150);

/// The breath on a stop. Shorter than a skip: a stop should feel like now.
const STOP_FADE: Duration = Duration::from_millis(80);

/// The dip around a seek: down before the jump, up after it. A seek lands
/// mid-waveform on both sides, and forty milliseconds of valley is cheaper
/// on the ear than two clicks.
const SEEK_DIP_DOWN: Duration = Duration::from_millis(10);
const SEEK_DIP_UP: Duration = Duration::from_millis(30);

/// How close to the end the gapless append happens. Late on purpose: an
/// appended source cannot be taken back out of a sink, so this is the
/// window in which a queue edit can go unheeded — ordinarily. A seek
/// backward after the append stretches that window with it: the append
/// stands, like everything else about it, until the track's real end.
const APPEND_LEAD: f64 = 1.5;

/// How much blend this track can carry: the configured seconds, capped at
/// half the track — past that a short track is all blend and no track.
/// Zero (no blend) when crossfade is off or the length is unknown.
fn effective_fade(crossfade: f32, duration: f64) -> f64 {
    if !(crossfade > 0.0) || duration <= 0.0 {
        return 0.0;
    }
    f64::from(crossfade).min(duration / 2.0)
}

/// The window a handover actually gets. The half-track rule guards both
/// ends: the outgoing side through [`effective_fade`], and the incoming
/// side here — a ramp longer than half the incoming track never reaches
/// full volume before the track is over, and the outgoing's down-ramp
/// then outlives the incoming sink and ends in the very cut the blend
/// exists to remove (review finding: short-next hard cut). Clamped to
/// what actually remains, floored so a late handover blends instead of
/// clicking.
fn blend_window(crossfade: f32, outgoing_dur: f64, remaining: f64, incoming_dur: f64) -> f64 {
    let mut window = effective_fade(crossfade, outgoing_dur).min(remaining.max(0.0));
    if incoming_dur > 0.0 {
        window = window.min(incoming_dur / 2.0);
    }
    window.max(MIN_FADE)
}

/// What plays after the current track, as far as anyone has decided.
enum NextTrack {
    /// Nothing decided — the resting state, and all of it when crossfade
    /// is off.
    Idle,
    /// A thread is opening the pick; its answer arrives on `rx`. Dropping
    /// the receiver is the cancellation: the opener's send fails, the
    /// decoder drops, and its spool file deletes itself.
    Opening { index: Option<usize>, rx: mpsc::Receiver<Result<Prepared, String>> },
    /// Opened, decoded, spooling — waiting for the fade window to arrive.
    Ready { prepared: Prepared, index: Option<usize> },
    /// The open failed. Remembered so the tick does not walk into the same
    /// doomed open every 120 ms; the track's natural end then takes the
    /// ordinary advance path, which gives the URL its one fresh try.
    Failed,
}

/// The committed pick for the track after this one, or None when no
/// transition should happen. A *blend* never blends a track into itself,
/// so with `allow_self` false, loop-one and picks landing back on the
/// current row are refused. A *gapless* append is the opposite case —
/// looping a track's seam sample-tight is what the feature is for — so
/// gapless passes `allow_self` true and the loop gets its second reader
/// of the same source (C4 review: the loop seam always gapped).
/// Committing at prepare time matters under shuffle — pick_next rolls
/// dice, and they must be rolled once, here, not again at handover.
fn next_candidate(q: &QueueState, allow_self: bool) -> Option<(QueueEntry, Option<usize>)> {
    if q.loop_mode == LoopMode::One {
        if !allow_self {
            return None;
        }
        return q.queue.get(q.index).map(|entry| (entry.clone(), Some(q.index)));
    }
    let index = pick_next(q, false)?;
    if index == q.index && !allow_self {
        return None;
    }
    Some((q.queue[index].clone(), Some(index)))
}

/// The prepare thread's name — the TUI's panic hook recognises it, the same
/// way it recognises the audio thread, and stands back: its panics are
/// caught below, and "recovering" the terminal for a caught panic tears the
/// screen down under a UI that is still running (audit #32).
pub(crate) const PREPARE_THREAD: &str = "mstream-prepare";

/// Gapless: hand the prepared source to the playing sink itself. rodio
/// crosses queued sources sample-tight, which is the entire feature; the
/// price is that an append cannot be taken back, so it happens as late as
/// [`APPEND_LEAD`] allows and the bookkeeping waits for the boundary in
/// [`State::promote_appended`].
fn append_gapless(s: &mut State) {
    let NextTrack::Ready { prepared, index } = std::mem::replace(&mut s.next, NextTrack::Idle)
    else {
        return;
    };
    let (fade, tap_live) = attach(&s.sink, prepared.opened, s.tap.clone(), 1.0);
    s.appended =
        Some(Appended { path: prepared.path, duration: prepared.duration, index, fade, tap_live });
}

/// Open `entry` on a short-lived thread of its own. The open can block for
/// the network's full patience, and the whole point of preparing is that
/// nobody holds the state lock — or the audio — waiting on it.
fn spawn_prepare(entry: QueueEntry, index: Option<usize>) -> NextTrack {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(PREPARE_THREAD.into())
        .spawn(move || {
            // Caught, because symphonia panics on malformed files (audit
            // #32): the sender drops unfired and the tick reads the
            // disconnect as a failed open. The catch alone is not the
            // whole defence — the process panic hook runs at the panic
            // site, *before* any catch — which is why the TUI's hook
            // knows this thread by name and stands back for it.
            let opened =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_entry(&entry)));
            if let Ok(result) = opened {
                let _ = tx.send(result);
            }
        });
    match spawned {
        Ok(_) => NextTrack::Opening { index, rx },
        // A box that cannot spawn a thread still has the ordinary advance
        // path; a blend is not worth an error.
        Err(_) => NextTrack::Failed,
    }
}

/// Wrap `opened` in the chain every source gets — the tap copy first, so
/// the ring sees the track at full amplitude rather than the blend (the
/// same reasoning as listening under volume: draw the track, not the
/// knob), then the fade — and append it to `sink`. Returns the source's
/// controls for whoever will own them.
fn attach(
    sink: &Player,
    opened: Opened,
    tap: Option<Arc<tap::AudioTap>>,
    initial_gain: f32,
) -> (Arc<fade::FadeHandle>, Arc<AtomicBool>) {
    let fade = fade::FadeHandle::new(initial_gain);
    let live = Arc::new(AtomicBool::new(true));
    match (opened, tap) {
        (Opened::Local(d), Some(tap)) => {
            sink.append(fade::Faded::new(tap::Tapped::new(d, tap, live.clone()), fade.clone()))
        }
        (Opened::Local(d), None) => sink.append(fade::Faded::new(d, fade.clone())),
        (Opened::Http(d), Some(tap)) => {
            sink.append(fade::Faded::new(tap::Tapped::new(d, tap, live.clone()), fade.clone()))
        }
        (Opened::Http(d), None) => sink.append(fade::Faded::new(d, fade.clone())),
    }
    (fade, live)
}

/// A sink on its way out of a blend: still connected to the mixer, ramping
/// to silence, waiting to be dropped. Its presence is also the latch that
/// keeps blends strictly one at a time.
struct Outgoing {
    sink: Arc<Player>,
    /// Watched, not commanded: position 0.0 means the ramp finished and
    /// nothing audible remains even if the source has not drained — a
    /// stalled download can hold a silent sink open indefinitely, and the
    /// latch it holds would block every later blend.
    fade: Arc<fade::FadeHandle>,
    /// The blend window, kept so the deadline can be re-armed: the wall
    /// clock runs through a pause, and a deadline armed once at handover
    /// would come due mid-pause — the first tick after resume then cut
    /// the outgoing at audible gain (review finding: pause/resume pop).
    fade_dur: Duration,
    deadline: Instant,
}

/// A source appended to the playing sink for a gapless boundary: rodio
/// crosses queued sources sample-tight, and this is the bookkeeping that
/// catches up once it has.
struct Appended {
    path: String,
    duration: f64,
    /// The committed queue slot (serve), or None for a TUI announcement —
    /// the same split as a blend handover.
    index: Option<usize>,
    fade: Arc<fade::FadeHandle>,
    tap_live: Arc<AtomicBool>,
}

struct State {
    /// Arc'd so a blocking call on the sink — a seek waiting for the device
    /// callback to reach data that has not downloaded — can be made on a
    /// clone with the state lock already released. Held across the wait,
    /// the lock froze every other control with it (audit #48).
    sink: Arc<Player>,
    /// The playing source's gain ramp, settled at 1.0 outside a blend.
    /// Every source is wrapped whether or not crossfade is on: one code
    /// path, and the wrapper at rest is a multiply per sample.
    fade: Arc<fade::FadeHandle>,
    /// The playing source's tap switch, flipped off when the source is
    /// retired into `outgoing` — two sources must never share the ring.
    tap_live: Arc<AtomicBool>,
    /// Where a copy of the audio goes, when someone is drawing it. Serve mode
    /// attaches none and pays nothing.
    tap: Option<Arc<tap::AudioTap>>,
    current_file: String,
    duration: f64,
    stopped: bool,
    /// Desired volume; survives sink recreation on track change (audit fix #1).
    volume: f32,
    /// Failed opens in the current walk for something playable, counted
    /// across ticks — [`Engine::advance_tick`] attempts one source per call.
    advance_failures: usize,
    /// Seconds of blend between tracks. 0.0 — the default — is off. Off
    /// (with gapless also off) means no prefetch and no transitions, as
    /// before Phase C; the soft cuts on manual skip/stop/seek are the one
    /// deliberate global departure (C4) — the old engine clicked.
    crossfade: f32,
    /// Sample-tight transitions when no blend is configured: the prepared
    /// next is appended to the playing sink instead of overlapped on a
    /// second one. Off by default for the same compatibility reason.
    gapless: bool,
    /// The source waiting inside the sink for its gapless boundary.
    appended: Option<Appended>,
    /// What the TUI said should play after the current track. The TUI keeps
    /// its own queue and feeds this engine one source at a time, so unlike
    /// serve mode the engine cannot pick a next; it has to be told. Consulted
    /// ahead of the internal queue, consumed by the handover, withdrawn by
    /// [`Engine::clear_next`] and by any new play or stop.
    pending_next: Option<QueueEntry>,
    next: NextTrack,
    /// Sinks on their way out — a blend's outgoing half, and the short
    /// breaths of skips and stops. A Vec rather than one slot because the
    /// breaths overlap in ordinary use (stop during a skip, skip during a
    /// skip), and the single slot forced the newcomer to hard-cut whatever
    /// held it — the exact click this machinery exists to remove (C4
    /// review). Bounded by [`State::retire_softly`]; blends stay serialized
    /// by the transition gate checking emptiness.
    outgoing: Vec<Outgoing>,
    q: QueueState,
}

impl State {
    /// Start playing q.queue[q.index]. Opens and decodes BEFORE touching the
    /// running sink, so a bad source leaves current playback untouched (same
    /// ordering as the original) — a blend in progress included, which is
    /// cut only once there is something real to replace it with.
    fn start_current(&mut self, mixer: &Mixer) -> Result<(), EngineError> {
        if self.q.index >= self.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        let entry = self.q.queue[self.q.index].clone();
        let prepared = open_entry(&entry).map_err(EngineError::Unplayable)?;
        self.install(mixer, prepared);
        Ok(())
    }

    /// The sink swap every start shares: whatever was mid-blend,
    /// mid-prepare, or already appended is a plan that is not happening,
    /// the track being left gets a skip-length breath instead of a click
    /// (C4), and `prepared` becomes the current sound.
    fn install(&mut self, mixer: &Mixer, prepared: Prepared) {
        self.cancel_overlap();
        self.silence_appended();
        self.retire_softly(SKIP_FADE);
        let sink = Player::connect_new(mixer);
        sink.set_volume(self.volume);
        self.attach_fresh(&sink, prepared.opened, 1.0);
        self.sink = Arc::new(sink);
        self.current_file = prepared.path;
        self.duration = prepared.duration;
        self.stopped = false;
        self.advance_failures = 0;
    }

    /// [`attach`], and the fresh controls become the current ones.
    fn attach_fresh(&mut self, sink: &Player, opened: Opened, initial_gain: f32) {
        let (fade, live) = attach(sink, opened, self.tap.clone(), initial_gain);
        self.fade = fade;
        self.tap_live = live;
    }

    /// Retire the playing sink with a breath instead of a cut: its fade is
    /// commanded to zero over `window` and the sink parks in `outgoing` to
    /// drain. A sink with nothing audible left is simply stopped — no
    /// breath is owed to silence.
    fn retire_softly(&mut self, window: Duration) {
        // A paused sink is silent, and silence is owed no breath — parked,
        // it would be a zombie: its sample-clocked ramp can never advance,
        // its drain never comes, and the deadline is deliberately skipped
        // while paused. It held the drainer gate forever and came BACK on
        // the next resume (C4 review). Stopped is stopped.
        if self.sink.empty() || self.stopped || self.sink.is_paused() {
            self.sink.stop();
            return;
        }
        self.fade.ramp_to(0.0, window);
        self.tap_live.store(false, Ordering::Relaxed);
        // Bounded: past three simultaneous drainers someone is mashing
        // keys, and the oldest — deepest into its ramp, so the quietest —
        // is the right one to cut outright.
        if self.outgoing.len() >= 3 {
            self.outgoing.remove(0).sink.stop();
        }
        self.outgoing.push(Outgoing {
            sink: self.sink.clone(),
            fade: self.fade.clone(),
            fade_dur: window,
            deadline: Instant::now() + window + OUTGOING_SLACK,
        });
    }

    /// An appended gapless source cannot be taken back out of its sink, but
    /// a still-QUEUED one can be made inaudible: snapped silent before its
    /// first sample, it plays out under the retirement machinery without
    /// ever being heard. A source whose boundary has already crossed is a
    /// different animal — it is the sounding track, and snapping it is the
    /// full-scale click this machinery exists to prevent (rodio crosses the
    /// boundary on the audio thread; promotion waits for the next tick, a
    /// window every stop or skip can land in; C4 review). So: promote
    /// first, and let the caller's soft retire treat it as what it is.
    fn silence_appended(&mut self) {
        if self.appended.is_some() && self.sink.len() <= 1 {
            self.promote_appended();
            return;
        }
        if let Some(appended) = self.appended.take() {
            appended.fade.snap(0.0);
            appended.tap_live.store(false, Ordering::Relaxed);
        }
    }

    /// Stop what is playing with a breath instead of a click: the audible
    /// sink ramps out and drains in the outgoing slot while the bookkeeping
    /// clears now — status says stopped immediately, the ear gets 80ms of
    /// mercy.
    fn stop_softly(&mut self) {
        // Whatever else is draining — a blend's outgoing, a skip's breath —
        // keeps its own ramp; the drainer list holds them all now, so a
        // stop no longer buys its slot by hard-cutting the previous tenant
        // at whatever gain it stood (C4 review).
        self.silence_appended();
        self.retire_softly(STOP_FADE);
        self.invalidate_next();
        self.pending_next = None;
        self.current_file.clear();
        self.duration = 0.0;
        self.stopped = true;
        self.advance_failures = 0;
    }

    /// The sink crossed a gapless boundary on its own: the appended source
    /// is what is sounding, so the bookkeeping catches up — the same shape
    /// as a blend handover, without the second sink.
    fn promote_appended(&mut self) {
        let Some(next) = self.appended.take() else { return };
        // Noted before current_file is overwritten: a self-loop's lap.
        let looped = next.path == self.current_file;
        self.tap_live.store(false, Ordering::Relaxed);
        self.fade = next.fade;
        self.tap_live = next.tap_live;
        self.current_file = next.path;
        self.duration = next.duration;
        self.advance_failures = 0;
        match next.index {
            Some(index) => {
                // Any queue mutation since the append discarded nothing —
                // an append is irrevocable — but the seatbelt still holds
                // the index inside the queue that exists now.
                if index < self.q.queue.len() {
                    self.q.index = index;
                }
            }
            None => {
                let hint = (self.duration > 0.0).then_some(self.duration);
                self.q.queue =
                    vec![QueueEntry { path: self.current_file.clone(), duration_hint: hint }];
                self.q.index = 0;
                // A self-loop keeps its announcement: the pending that fed
                // this lap is the same track again, and the app cannot
                // re-announce what it never sees change — a same-source
                // boundary emits no event. Kept, the loop sustains itself.
                if !looped {
                    self.pending_next = None;
                }
            }
        }
    }

    /// Forget whatever was decided or prepared about the next track. Any
    /// queue mutation calls this: the committed pick was made against a
    /// queue that no longer exists, and over-forgetting only costs a
    /// re-open, where under-forgetting plays the wrong track.
    fn invalidate_next(&mut self) {
        self.next = NextTrack::Idle;
    }

    /// A change of plan mid-blend: cut the outgoing sink now and forget the
    /// prepared next. What every manual transport action wants — nobody
    /// pressing next wants the old track audibly lingering under the new.
    fn cancel_overlap(&mut self) {
        for out in self.outgoing.drain(..) {
            out.sink.stop();
        }
        self.invalidate_next();
    }

    /// A seek keeps the surviving track and cuts the blend around it: the
    /// outgoing sink stops, a half-risen fade snaps to full. The prepared
    /// next, if any, stays — a seek does not change what comes after.
    fn snap_out_of_blend(&mut self) {
        if self.outgoing.is_empty() {
            return;
        }
        for out in self.outgoing.drain(..) {
            out.sink.stop();
        }
        self.fade.snap(1.0);
    }

    /// Drop an outgoing sink once nothing audible remains of it: source
    /// drained, or ramp at silence with the source still going. The
    /// wall-clock deadline is the net under both, skipped while paused —
    /// a paused blend is frozen, not late.
    fn retire_outgoing(&mut self) {
        self.outgoing.retain(|out| {
            let spent = out.sink.empty()
                || out.fade.position() <= 0.0
                || (!out.sink.is_paused() && Instant::now() >= out.deadline);
            if spent {
                out.sink.stop();
            }
            !spent
        });
    }

    fn clear_current(&mut self) {
        self.current_file.clear();
        self.duration = 0.0;
        self.stopped = true;
        self.advance_failures = 0;
        self.pending_next = None;
        self.cancel_overlap();
    }
}

pub struct Engine {
    // Field order matters: `state` (and the Players inside it) must drop
    // before the device sink that owns the output stream.
    state: Arc<Mutex<State>>,
    device: MixerDeviceSink,
}

impl Engine {
    pub fn new() -> Result<Self, EngineError> {
        let mut device = DeviceSinkBuilder::open_default_sink()
            .map_err(|e| EngineError::NoDevice(e.to_string()))?;
        device.log_on_drop(false);
        let sink = Player::connect_new(device.mixer());

        let state = Arc::new(Mutex::new(State {
            sink: Arc::new(sink),
            fade: fade::FadeHandle::new(1.0),
            tap_live: Arc::new(AtomicBool::new(true)),
            tap: None,
            current_file: String::new(),
            duration: 0.0,
            stopped: true,
            volume: 1.0,
            advance_failures: 0,
            crossfade: 0.0,
            gapless: false,
            appended: None,
            pending_next: None,
            next: NextTrack::Idle,
            outgoing: Vec::new(),
            q: QueueState {
                queue: Vec::new(),
                index: 0,
                shuffle: false,
                loop_mode: LoopMode::None,
            },
        }));

        Ok(Engine { state, device })
    }

    /// Send a copy of everything played from here on to `tap`. Takes effect
    /// on the next source, since the one already in the sink is past reach.
    pub fn attach_tap(&self, tap: Arc<tap::AudioTap>) {
        self.state.lock().unwrap().tap = Some(tap);
    }

    /// Clear the queue, add one source (path or URL), play it.
    pub fn play_source(&self, source: String, duration_hint: Option<f64>) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        s.q.queue.clear();
        s.q.queue.push(QueueEntry { path: source, duration_hint });
        s.q.index = 0;
        // A new track makes the old announcement about a future that is not
        // happening; the caller re-announces once this one is playing. The
        // committed pick goes with it *now*, not via start_current's
        // success path: the queue it was made against has just been
        // replaced, and a failed start would otherwise leave it alive to
        // blend a track from a queue that no longer exists (the sibling of
        // the failed-jump review finding, caught by its verifier).
        s.pending_next = None;
        s.invalidate_next();
        s.start_current(self.device.mixer())
    }

    /// Announce what should play after the current track, so a blend can
    /// open it ahead of the fade window. Replaces any earlier announcement;
    /// announcing the same source again only refreshes its duration hint,
    /// so a repeated announcement never throws away an open in flight.
    pub fn prepare_next(&self, source: String, duration_hint: Option<f64>) {
        let mut s = self.state.lock().unwrap();
        // A track never BLENDS into itself. The app refuses to announce
        // one, and this is the belt to that suspender: a handover into the
        // playing source would change nothing status can show, and the
        // watchers upstream would never learn it happened. The gapless
        // repeat-one seam is the sanctioned exception — its cursor never
        // needs to move, so nothing upstream needs telling.
        if s.current_file == source && !(s.gapless && s.crossfade <= 0.0) {
            return;
        }
        if s.pending_next.as_ref().map(|e| e.path.as_str()) == Some(source.as_str()) {
            if let Some(entry) = &mut s.pending_next {
                entry.duration_hint = duration_hint;
            }
            return;
        }
        s.pending_next = Some(QueueEntry { path: source, duration_hint });
        s.invalidate_next();
    }

    /// Withdraw the announcement: nothing follows the current track.
    pub fn clear_next(&self) {
        let mut s = self.state.lock().unwrap();
        s.pending_next = None;
        s.invalidate_next();
    }

    /// Pause everything audible — during a blend that is two sinks, and
    /// pausing only one would leave the other playing under the silence.
    pub fn pause(&self) {
        let s = self.state.lock().unwrap();
        s.sink.pause();
        for out in &s.outgoing {
            out.sink.pause();
        }
    }

    pub fn resume(&self) {
        let mut s = self.state.lock().unwrap();
        s.sink.play();
        for out in &mut s.outgoing {
            out.sink.play();
            // The wall clock ran through the pause while the blend stood
            // frozen; a deadline that came due in that time would cut the
            // outgoing at audible gain on the very next tick. Re-arm with
            // the full window — the deadline is a pathology net, and a
            // generous net still catches what it exists for.
            out.deadline = Instant::now() + out.fade_dur + OUTGOING_SLACK;
        }
    }

    pub fn stop(&self) {
        self.state.lock().unwrap().stop_softly();
    }

    pub fn seek(&self, position: f64) -> Result<(), EngineError> {
        let target = seek_target(position)?;
        // Take a handle and let the state go before asking. try_seek blocks
        // on rodio's feedback channel until the device callback performs
        // the seek, and past the downloaded range that callback is waiting
        // on the network — possibly forever, since the download loop never
        // gives a dead server up. The wait is this caller's to make; the
        // lock was making it everyone's (audit #48).
        let (sink, fade) = {
            let mut s = self.state.lock().unwrap();
            // rodio's seek order is sink-wide, and under gapless a sink can
            // briefly hold two tracks. A boundary that has already crossed
            // is promoted before the order is placed so the seek aims at
            // what is actually sounding; the residue — a boundary crossing
            // in the ~5ms between placing the order and the periodic access
            // consuming it — is accepted (C4 review).
            if s.appended.is_some() && s.sink.len() <= 1 {
                s.promote_appended();
            }
            // Seeking mid-blend keeps the track being seeked; hearing the
            // old one still draining behind a jump is disorienting.
            s.snap_out_of_blend();
            (s.sink.clone(), s.fade.clone())
        };
        // The dip (C4): down before the jump, up after it. The down-ramp
        // has a device callback or two to land while try_seek waits on the
        // seek being performed; the up-ramp then rises from the new spot.
        fade.ramp_to(0.0, SEEK_DIP_DOWN);
        let sought = sink.try_seek(target).map_err(|e| EngineError::Seek(e.to_string()));
        // try_seek can block for as long as the network makes it, and a
        // stop or skip can retire this sink in the meantime — its fade then
        // belongs to a drainer ramping to silence, and the up-ramp would
        // resurrect a track the engine reports as gone, at full volume,
        // until the deadline clicked it off (C4 review). Ramp up only if
        // the handle still steers what is current.
        {
            let s = self.state.lock().unwrap();
            if Arc::ptr_eq(&s.fade, &fade) {
                fade.ramp_to(1.0, SEEK_DIP_UP);
            }
        }
        sought
    }

    pub fn set_volume(&self, volume: f32) {
        let mut s = self.state.lock().unwrap();
        let v = if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 1.0 };
        s.volume = v;
        s.sink.set_volume(v);
        // Every draining sink sits at the user's volume; the blends and
        // breaths live in the sources' fade wrappers, so the two never
        // multiply.
        for out in &s.outgoing {
            out.sink.set_volume(v);
        }
    }

    /// Seconds of blend between tracks; 0 turns it off. Serve mode sets
    /// this once at boot from --crossfade; the TUI will set it from config
    /// (Phase C3). Turning it off mid-flight abandons any preparation on
    /// the next tick; a blend already sounding finishes on its own.
    pub fn set_crossfade(&self, seconds: f32) {
        let mut s = self.state.lock().unwrap();
        s.crossfade = if seconds.is_finite() { seconds.clamp(0.0, 30.0) } else { 0.0 };
    }

    /// Sample-tight transitions when no blend is configured (C4): the
    /// prepared next is appended to the playing sink instead of overlapped
    /// on a second one. A configured crossfade outranks it.
    pub fn set_gapless(&self, on: bool) {
        self.state.lock().unwrap().gapless = on;
    }

    pub fn status(&self) -> Status {
        let mut s = self.state.lock().unwrap();
        // A crossed gapless boundary is promoted before answering, not a
        // tick later: without this, /status reported the old file carrying
        // the new track's position for up to a poll interval (C4 review).
        if s.appended.is_some() && s.sink.len() <= 1 {
            s.promote_appended();
        }
        let is_empty = s.sink.empty();
        let is_paused = s.sink.is_paused();
        // `stopped` is ours and set synchronously; sink.empty() lags a stop()
        // by one audio callback, which made /status report playing=true for a
        // few ms after /stop (audit finding #12, inherited from the original).
        Status {
            playing: !is_empty && !is_paused && !s.stopped,
            paused: is_paused,
            position: s.sink.get_pos().as_secs_f64(),
            duration: s.duration,
            volume: s.volume,
            file: s.current_file.clone(),
            queue_index: s.q.index,
            queue_length: s.q.queue.len(),
            shuffle: s.q.shuffle,
            loop_mode: s.q.loop_mode.as_str().to_string(),
        }
    }

    pub fn next_manual(&self) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        match pick_next(&s.q, true) {
            Some(idx) => {
                s.q.index = idx;
                let started = s.start_current(self.device.mixer());
                if started.is_err() {
                    // The index moved even though the start failed, so a
                    // pick committed against the old position is now a lie
                    // — the invariant handover leans on (review finding:
                    // failed jumps kept stale commitments).
                    s.invalidate_next();
                }
                started
            }
            None => Err(EngineError::EndOfQueue),
        }
    }

    pub fn previous_manual(&self) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        if s.q.index == 0 {
            if s.q.loop_mode == LoopMode::All && !s.q.queue.is_empty() {
                s.q.index = s.q.queue.len() - 1;
                let started = s.start_current(self.device.mixer());
                if started.is_err() {
                    // Same as next_manual: the index moved, the start did
                    // not, and the committed pick answers for neither.
                    s.invalidate_next();
                }
                started
            } else {
                // The restart is a seek like any other — same discipline as
                // [`Engine::seek`], even though the start of the track has
                // almost always downloaded by now. Same blend policy and
                // the same dip, too.
                s.snap_out_of_blend();
                let sink = s.sink.clone();
                let fade = s.fade.clone();
                drop(s);
                fade.ramp_to(0.0, SEEK_DIP_DOWN);
                let _ = sink.try_seek(Duration::ZERO);
                // Same resurrection guard as Engine::seek: only the
                // still-current handle gets the up-ramp.
                let s = self.state.lock().unwrap();
                if Arc::ptr_eq(&s.fade, &fade) {
                    fade.ramp_to(1.0, SEEK_DIP_UP);
                }
                Ok(())
            }
        } else {
            s.q.index -= 1;
            let started = s.start_current(self.device.mixer());
            if started.is_err() {
                s.invalidate_next();
            }
            started
        }
    }

    pub fn set_shuffle(&self, value: bool) {
        let mut s = self.state.lock().unwrap();
        s.q.shuffle = value;
        // The committed next pick was made under the old rules.
        s.invalidate_next();
    }

    pub fn cycle_loop(&self) -> LoopMode {
        let mut s = self.state.lock().unwrap();
        s.q.loop_mode = s.q.loop_mode.next();
        s.invalidate_next();
        s.q.loop_mode
    }

    /// Append one source; if the queue was empty and nothing is playing, start.
    /// Failure to start is deliberately not an error (matches the original).
    ///
    /// No duration hint: the only way into this queue is a serve route, and
    /// a route is handed a path and nothing else. `queue_add_entry` used to
    /// sit here taking one, but every caller it ever had passed `None`
    /// (finding #68). Hints still reach the player that has them, through
    /// [`Engine::play_source`].
    pub fn queue_add(&self, file: String) {
        let mut s = self.state.lock().unwrap();
        let was_empty = s.q.queue.is_empty();
        s.q.queue.push(QueueEntry::new(file));
        // A different queue can mean a different next.
        s.invalidate_next();
        // `stopped` beside the empty check: a soft stop leaves the old sink
        // audibly draining its breath for a beat, and gating on emptiness
        // alone meant /stop-then-add (or clear-then-add) landed in that
        // beat and never started (C4 review, critic).
        if was_empty && (s.stopped || s.sink.empty()) {
            s.q.index = 0;
            let _ = s.start_current(self.device.mixer());
        }
    }

    pub fn queue_add_many(&self, files: Vec<String>) {
        let mut s = self.state.lock().unwrap();
        let was_empty = s.q.queue.is_empty();
        s.q.queue.extend(files.into_iter().map(QueueEntry::new));
        s.invalidate_next();
        if was_empty && (s.stopped || s.sink.empty()) && !s.q.queue.is_empty() {
            s.q.index = 0;
            let _ = s.start_current(self.device.mixer());
        }
    }

    pub fn queue_play_index(&self, index: usize) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        if index >= s.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        s.q.index = index;
        let started = s.start_current(self.device.mixer());
        if started.is_err() {
            s.invalidate_next();
        }
        started
    }

    pub fn queue_remove(&self, index: usize) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        if index >= s.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        match apply_remove(&mut s.q, index) {
            RemoveOutcome::EmptiedQueue => {
                s.stop_softly();
            }
            RemoveOutcome::RemovedCurrent => {
                s.invalidate_next();
                // Audit fix #4: only restart playback if we were actually
                // playing; the original started audio as a side effect of
                // removing a track while stopped.
                if !s.stopped {
                    let _ = s.start_current(self.device.mixer());
                }
            }
            RemoveOutcome::RemovedBeforeCurrent | RemoveOutcome::RemovedAfterCurrent => {
                s.invalidate_next();
                // An appended source's committed slot moves with the queue
                // like the current index does — unshifted, promotion after
                // a shrink pointed the index at the wrong row and the
                // advance from there double-played or skipped (C4 review).
                // The audio itself is beyond reach; if the removed row WAS
                // the appended one, the shift leaves the index on its
                // successor, which is where the advance should resume.
                let len = s.q.queue.len();
                if let Some(appended) = &mut s.appended {
                    if let Some(committed) = appended.index {
                        let (shifted, _) = crate::advance::shift_current(len, committed, index);
                        appended.index = Some(shifted);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn queue_clear(&self) {
        let mut s = self.state.lock().unwrap();
        s.stop_softly();
        s.q.queue.clear();
        s.q.index = 0;
    }

    pub fn queue_snapshot(&self) -> QueueSnapshot {
        let s = self.state.lock().unwrap();
        QueueSnapshot {
            queue: s.q.queue.iter().map(|e| e.path.clone()).collect(),
            current_index: s.q.index,
        }
    }

    /// Advance to the next track if the current one finished. Called from the
    /// serve poll loop and the TUI tick. Skips unplayable tracks one attempt
    /// per call: every open can block for as long as the network cap allows,
    /// and looping here held the state lock — and serve mode's whole request
    /// loop — through every doomed open in the queue (audit #49). The count
    /// of failures lives in [`State`], so the walk still gives up after one
    /// lap; it just yields between steps.
    ///
    /// Also drives the blend machinery: retiring spent outgoing sinks,
    /// preparing the next track ahead of the fade window, and handing over
    /// when it opens. When a blend succeeds the sink never empties between
    /// tracks, so the ordinary advance below never fires; when preparation
    /// failed or came too late, it fires exactly as it always has.
    pub fn advance_tick(&self) {
        let mut s = self.state.lock().unwrap();
        s.retire_outgoing();
        self.crossfade_step(&mut s);
        if !(s.sink.empty() && !s.stopped && !s.q.queue.is_empty()) {
            return;
        }
        match pick_next(&s.q, false) {
            None => s.clear_current(),
            Some(idx) => {
                s.q.index = idx;
                // A prepared decoder for exactly this entry — the blend or
                // append that missed its window (a duration hint running
                // long is the usual road here) — is installed rather than
                // thrown away and re-fetched with the lock held, which paid
                // for every such track twice (C4 review; the deferred
                // decoder-reuse item, finally done).
                let ready_for_idx = matches!(&s.next,
                    NextTrack::Ready { prepared, .. } if prepared.path == s.q.queue[idx].path);
                if ready_for_idx {
                    let NextTrack::Ready { prepared, .. } =
                        std::mem::replace(&mut s.next, NextTrack::Idle)
                    else {
                        unreachable!("matched Ready above, under the same lock");
                    };
                    s.install(self.device.mixer(), prepared);
                    return;
                }
                if s.start_current(self.device.mixer()).is_ok() {
                    return;
                }
                // The index moved and the start did not — the same rule as
                // the manual paths: a committed pick answers for neither.
                s.invalidate_next();
                s.advance_failures += 1;
                if s.advance_failures >= s.q.queue.len() {
                    s.clear_current();
                }
            }
        }
    }

    /// One tick of the blend machine: collect a finished open, start the
    /// next open when the window approaches, hand over when it arrives.
    /// Each is a step and none of them waits — the opens themselves live on
    /// their own thread, because this runs with the state lock held and
    /// findings #48–#50 are what happens when that lock meets a network.
    fn crossfade_step(&self, s: &mut State) {
        // The gapless boundary check lives ABOVE every gate, the off-gate
        // included: an append is irrevocable, and its bookkeeping has no
        // other road — behind the gate, toggling gapless off in the append
        // window orphaned the sounding track (status lied for its whole
        // length, then it played twice; C4 review). A boundary cannot cross
        // while paused (no samples flow), so ordering past the pause gate
        // is tidiness; ordering past the off-gate is correctness.
        if s.appended.is_some() && s.sink.len() <= 1 {
            s.promote_appended();
        }

        let blending = s.crossfade > 0.0;
        if !blending && !s.gapless {
            // Toggled off with something in flight: forget what is not yet
            // committed. An overlap already sounding retires on its own,
            // and an appended source promotes above.
            if !matches!(s.next, NextTrack::Idle) {
                s.invalidate_next();
            }
            return;
        }
        if s.stopped || s.q.queue.is_empty() {
            return;
        }

        // Collect the opener's answer if one has arrived.
        s.next = match std::mem::replace(&mut s.next, NextTrack::Idle) {
            NextTrack::Opening { index, rx } => match rx.try_recv() {
                Ok(Ok(prepared)) => NextTrack::Ready { prepared, index },
                // open_entry already told stderr what went wrong.
                Ok(Err(_)) => NextTrack::Failed,
                // The opener panicked — symphonia does, on malformed files
                // (audit #32) — which reads the same as a failed open.
                Err(mpsc::TryRecvError::Disconnected) => NextTrack::Failed,
                Err(mpsc::TryRecvError::Empty) => NextTrack::Opening { index, rx },
            },
            other => other,
        };

        // A seam prepared for gapless must never survive into a blend: a
        // crossfade toggled on mid-lap leaves a self-aimed pending (and
        // possibly a self-opened Ready) behind, and blends do not
        // self-blend — whatever an earlier mode left standing. The app
        // withdraws its side in the same keystroke; this is the engine's
        // own refusal, for whatever ordering the messages arrive in.
        if blending {
            let self_prepared = s.pending_next.as_ref().is_some_and(|e| e.path == s.current_file)
                || matches!(&s.next,
                    NextTrack::Ready { prepared, .. } if prepared.path == s.current_file);
            if self_prepared {
                s.pending_next = None;
                s.invalidate_next();
            }
        }

        // Paused means frozen: the position cannot reach the window, and a
        // handover under a paused track would start sounding on its own.
        if s.sink.is_paused() {
            return;
        }
        // Transitions are strictly one at a time: a draining blend or an
        // already-appended next is the gate for anything further.
        if !s.outgoing.is_empty() || s.appended.is_some() {
            return;
        }
        // A duration the engine does not know is a fade point — or an
        // append point — it cannot find; live transcodes stay on the
        // ordinary advance path. (A duration *hint* that is wrong misplaces
        // the transition exactly as far as it misplaces the progress bar;
        // both trust it the same.)
        if s.duration <= 0.0 {
            return;
        }
        let fade = if blending { effective_fade(s.crossfade, s.duration) } else { 0.0 };
        if blending && fade <= 0.0 {
            return;
        }
        let remaining = s.duration - s.sink.get_pos().as_secs_f64();

        // A transient failure earns a fresh try when the window recedes: a
        // seek back re-opens minutes of runway in which a network blip may
        // well have healed. Resetting only *outside* the window keeps the
        // no-retry-every-tick property inside it (review finding: the
        // Failed latch was one-way).
        if matches!(s.next, NextTrack::Failed) && remaining > fade + PREPARE_LEAD {
            s.next = NextTrack::Idle;
        }

        if matches!(s.next, NextTrack::Idle) && remaining <= fade + PREPARE_LEAD {
            // The TUI's announcement outranks the internal queue: where one
            // is set the queue is a single mirrored entry with no next of
            // its own, and where the queue has answers (serve) nobody
            // announces.
            let candidate = s
                .pending_next
                .clone()
                .map(|entry| (entry, None))
                .or_else(|| next_candidate(&s.q, !blending));
            if let Some((entry, index)) = candidate {
                s.next = spawn_prepare(entry, index);
            }
        }
        if matches!(s.next, NextTrack::Ready { .. }) {
            if blending && remaining <= fade {
                self.handover(s);
            } else if !blending && remaining <= APPEND_LEAD {
                append_gapless(s);
            }
        }
    }

    /// The blend itself: retire the current sink into `outgoing` ramping
    /// down, start the prepared track on a fresh sink ramping up over the
    /// same window. Status flips to the incoming track here, at fade start
    /// — the streaming players' convention, and the moment the position
    /// starts counting from zero.
    fn handover(&self, s: &mut State) {
        let NextTrack::Ready { prepared, index } =
            std::mem::replace(&mut s.next, NextTrack::Idle)
        else {
            return;
        };
        let remaining = (s.duration - s.sink.get_pos().as_secs_f64()).max(0.0);
        let fade_secs = blend_window(s.crossfade, s.duration, remaining, prepared.duration);
        let fade_dur = Duration::from_secs_f64(fade_secs);

        s.fade.ramp_to(0.0, fade_dur);
        s.tap_live.store(false, Ordering::Relaxed);
        s.outgoing.push(Outgoing {
            sink: s.sink.clone(),
            fade: s.fade.clone(),
            fade_dur,
            deadline: Instant::now() + fade_dur + OUTGOING_SLACK,
        });

        let sink = Player::connect_new(self.device.mixer());
        sink.set_volume(s.volume);
        s.attach_fresh(&sink, prepared.opened, 0.0);
        s.fade.ramp_to(1.0, fade_dur);
        s.sink = Arc::new(sink);
        s.current_file = prepared.path;
        s.duration = prepared.duration;
        s.stopped = false;
        s.advance_failures = 0;
        match index {
            Some(index) => {
                // Any queue mutation since the prepare discarded it, so the
                // committed index still holds; the bounds check is a seatbelt.
                if index < s.q.queue.len() {
                    s.q.index = index;
                }
            }
            None => {
                // A TUI announcement: the engine's queue mirrors what is
                // playing, one entry at a time (play_source semantics), so
                // the blend replaces that entry. The announcement is spent;
                // the TUI sends a fresh one when it moves its own cursor.
                let hint = (s.duration > 0.0).then_some(s.duration);
                s.q.queue = vec![QueueEntry { path: s.current_file.clone(), duration_hint: hint }];
                s.q.index = 0;
                s.pending_next = None;
            }
        }
    }

    /// Whether a blend is running — the outgoing half still draining.
    #[cfg(test)]
    fn overlap_active(&self) -> bool {
        !self.state.lock().unwrap().outgoing.is_empty()
    }

    /// How many sinks are draining — blends and breaths together.
    #[cfg(test)]
    fn drainers(&self) -> usize {
        self.state.lock().unwrap().outgoing.len()
    }

    /// Whether a gapless source sits appended, waiting for its boundary.
    #[cfg(test)]
    fn appended_waiting(&self) -> bool {
        self.state.lock().unwrap().appended.is_some()
    }

    /// Each draining sink's ramp position — how a test hears a resurrection.
    #[cfg(test)]
    fn drainer_positions(&self) -> Vec<f32> {
        self.state.lock().unwrap().outgoing.iter().map(|out| out.fade.position()).collect()
    }
}

// ── Duration detection via symphonia (local files only) ────────────────────

/// Turn a wire position into a Duration, or refuse it. Finite and
/// non-negative have been checked since finding #11; magnitude was the
/// dimension that check missed (finding #27) — from_secs_f64 panics past
/// what a Duration can hold, so `POST /seek {"position":1e300}` was a
/// remote abort of the whole jukebox.
fn seek_target(position: f64) -> Result<Duration, EngineError> {
    if !position.is_finite() || position < 0.0 {
        return Err(EngineError::Seek("invalid position".to_string()));
    }
    Duration::try_from_secs_f64(position)
        .map_err(|_| EngineError::Seek("position out of range".to_string()))
}

fn probe_duration(path: &str) -> f64 {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return 0.0,
    };

    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = match symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) {
        Ok(p) => p,
        Err(_) => return 0.0,
    };

    if let Some(track) = probed.format.default_track() {
        if let Some(n_frames) = track.codec_params.n_frames {
            if let Some(sr) = track.codec_params.sample_rate {
                if sr > 0 {
                    return n_frames as f64 / sr as f64;
                }
            }
        }
        if let Some(tb) = track.codec_params.time_base {
            if let Some(n_frames) = track.codec_params.n_frames {
                let d = tb.calc_time(n_frames);
                return d.seconds as f64 + d.frac;
            }
        }
    }
    0.0
}

// ── Tests (pure queue logic; no audio device required) ─────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seek_too_large_for_a_duration_is_refused_not_a_panic() {
        assert_eq!(seek_target(12.5).unwrap(), Duration::from_secs_f64(12.5));
        assert!(seek_target(0.0).is_ok());
        // 1e300 went straight into Duration::from_secs_f64, which panics —
        // one POST /seek and the process was gone with exit 101.
        for bad in [1e300, 1e20, f64::NAN, f64::INFINITY, -1.0] {
            assert!(seek_target(bad).is_err(), "{bad} must be refused");
        }
    }

    /// A playable WAV: 44.1k stereo 16-bit of quiet tone, `seconds` long.
    fn wav_bytes(seconds: usize) -> Vec<u8> {
        let rate = 44_100usize;
        let frames = rate * seconds;
        let mut data = Vec::with_capacity(44 + frames * 4);
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&((36 + frames * 4) as u32).to_le_bytes());
        data.extend_from_slice(b"WAVEfmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&(rate as u32).to_le_bytes());
        data.extend_from_slice(&((rate * 4) as u32).to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&16u16.to_le_bytes());
        data.extend_from_slice(b"data");
        data.extend_from_slice(&((frames * 4) as u32).to_le_bytes());
        for i in 0..frames {
            let v = ((i as f32 * 0.05).sin() * 2000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&v.to_le_bytes());
        }
        data
    }

    /// A server that answers one request with the start of a long WAV, goes
    /// quiet for `stall`, and only then sends the rest — the shape of a
    /// track whose tail has not downloaded yet.
    fn stalling_wav_server(seconds: usize, sent_first: usize, stall: Duration) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let body = wav_bytes(seconds);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut head = [0u8; 2048];
                    let _ = stream.read(&mut head);
                    let sent = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(sent.as_bytes());
                    let _ = stream.write_all(&body[..sent_first]);
                    let _ = stream.flush();
                    std::thread::sleep(stall);
                    let _ = stream.write_all(&body[sent_first..]);
                });
            }
        });
        format!("http://{addr}/stalling.wav")
    }

    /// `cargo test a_blocked_seek -- --ignored --nocapture` (local, no server)
    #[test]
    #[ignore = "needs an audio device"]
    fn a_blocked_seek_does_not_take_the_rest_of_the_controls_with_it() {
        // Enough audio to play from, a tail that takes eight seconds to
        // arrive, and a seek pointed straight into the gap. try_seek waits
        // on the device callback, the callback waits on the network — and
        // the state lock used to wait with them, so pause, stop and status
        // were all frozen for as long as the server felt like (audit #48).
        let url = stalling_wav_server(30, 500_000, Duration::from_secs(8));
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(url, Some(30.0)).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        // The device sink cannot be shared across threads, but the state
        // lock — the thing every other control queues on — can be probed
        // directly. That is the wait a second caller would feel.
        let state = engine.state.clone();
        let (probed, waited) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let asked = std::time::Instant::now();
            drop(state.lock().unwrap());
            let _ = probed.send(asked.elapsed());
        });

        // Blocks until the tail arrives; the result is not the point.
        let _ = engine.seek(20.0);

        let held = waited.recv_timeout(Duration::from_secs(20)).expect("the probe ran");
        println!(">>> the state lock came free in {held:?}");
        assert!(
            held < Duration::from_millis(500),
            "the lock was held {held:?} behind a seek that is waiting on the network"
        );
        engine.stop();
    }

    /// `cargo test giving_up_on_a_dead_queue -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn giving_up_on_a_dead_queue_takes_one_attempt_per_tick() {
        // One real track that runs out, three that cannot open behind it.
        // The old advance loop tried every one of them in a single call
        // with the lock held; each attempt is now one tick's worth, so
        // whatever is watching the engine gets a word in between (audit
        // #49). Local paths fail in microseconds — the point here is the
        // shape of the retreat, not its speed.
        let tiny = std::env::temp_dir().join("mstream-advance-test.wav");
        std::fs::write(&tiny, wav_bytes(1)).unwrap();
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(tiny.to_string_lossy().into_owned(), None).unwrap();
        for missing in ["one", "two", "three"] {
            engine.queue_add(format!("C:\\no\\such\\dir\\{missing}.wav"));
        }

        // Let the real track run out on its own.
        let gone = std::time::Instant::now();
        while !engine.status().file.is_empty() && engine.status().playing {
            assert!(gone.elapsed() < Duration::from_secs(5), "the track never ended");
            std::thread::sleep(Duration::from_millis(50));
        }

        for tick in 1..=3 {
            engine.advance_tick();
            assert!(
                !engine.status().file.is_empty(),
                "tick {tick} should have tried one dead track and kept the rest for later"
            );
        }
        engine.advance_tick();
        assert!(engine.status().file.is_empty(), "four ticks in, the queue is given up");
        let _ = std::fs::remove_file(&tiny);
    }

    /// Two short tracks blended: the handover must come early, the queue
    /// index must follow it, and status must never report the silence the
    /// pre-crossfade engine had between tracks (finding #8's audible gap).
    ///
    /// `cargo test a_crossfade_hands_over -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_crossfade_hands_over_early_and_never_goes_quiet() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-crossfade-a.wav");
        let second = dir.join("mstream-crossfade-b.wav");
        std::fs::write(&first, wav_bytes(4)).unwrap();
        std::fs::write(&second, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        let started = std::time::Instant::now();
        let mut switched_at = None;
        let mut quiet_before_switch = false;
        while started.elapsed() < Duration::from_secs(12) {
            engine.advance_tick();
            let status = engine.status();
            if status.file.is_empty() {
                // The queue ran out. Before the handover this is the old
                // inter-track gap — the thing the blend exists to remove.
                quiet_before_switch = switched_at.is_none();
                break;
            }
            if switched_at.is_none() && status.queue_index == 1 {
                switched_at = Some(started.elapsed());
                assert!(engine.overlap_active(), "the old sink should still be draining");
                assert!(status.playing, "a blend is playing, not a gap");
                assert!(status.position < 1.0, "the incoming track counts from zero");
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        assert!(!quiet_before_switch, "went quiet without ever handing over");
        let switched = switched_at.expect("the handover never happened");
        println!(">>> handed over at {switched:?}, queue done in {:?}", started.elapsed());
        // 4s track, 1.5s fade: the switch belongs near t=2.5. Ceilings are
        // loose for CI-grade timers; the claim is early-at-all, not sharp.
        assert!(switched > Duration::from_secs_f64(1.0), "at {switched:?} this is a skip");
        assert!(switched < Duration::from_secs_f64(3.6), "at {switched:?} the blend missed");
        // Eight seconds of audio, one and a half of them shared: the whole
        // queue must finish visibly sooner than the tracks played apart.
        assert!(started.elapsed() < Duration::from_secs_f64(7.6));

        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// The TUI-mode shape of a blend: a single-entry queue via play_source,
    /// the next track announced from outside, and the engine handing over
    /// on its own — the contract Phase C3's worker wiring relies on.
    ///
    /// `cargo test an_announced_next -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn an_announced_next_blends_without_a_queue() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-crossfade-f.wav");
        let second = dir.join("mstream-crossfade-g.wav");
        std::fs::write(&first, wav_bytes(4)).unwrap();
        std::fs::write(&second, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        engine.play_source(first.to_string_lossy().into_owned(), Some(4.0)).unwrap();
        engine.prepare_next(second.to_string_lossy().into_owned(), Some(4.0));

        let started = std::time::Instant::now();
        let handed = loop {
            engine.advance_tick();
            // Re-announcing the unchanged URL every poll: the engine
            // promises this is a no-op that never restarts the open. If
            // that promise broke, the open would restart every 50 ms,
            // never reach Ready, and this test would time out (review
            // finding: the idempotency contract was untested).
            engine.prepare_next(second.to_string_lossy().into_owned(), Some(4.0));
            let status = engine.status();
            assert!(!status.file.is_empty(), "went quiet instead of blending");
            if status.file == second.to_string_lossy() {
                break started.elapsed();
            }
            assert!(started.elapsed() < Duration::from_secs(6), "no handover ever came");
            std::thread::sleep(Duration::from_millis(50));
        };
        println!(">>> announced next took over at {handed:?}");
        assert!(handed > Duration::from_secs_f64(1.0), "a takeover this early is a skip");
        assert!(handed < Duration::from_secs_f64(3.6), "the blend missed its window");

        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// Gapless (C4): with no blend configured, the transition must come at
    /// the track's natural end — not early like a blend — with the sink
    /// never emptying between tracks, and the bookkeeping following the
    /// boundary within a tick.
    ///
    /// `cargo test gapless_crosses -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn gapless_crosses_the_boundary_without_a_gap_or_an_early_switch() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-gapless-a.wav");
        let second = dir.join("mstream-gapless-b.wav");
        std::fs::write(&first, wav_bytes(4)).unwrap();
        std::fs::write(&second, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        let started = std::time::Instant::now();
        let mut switched_at = None;
        while started.elapsed() < Duration::from_secs(12) {
            engine.advance_tick();
            let status = engine.status();
            if status.file.is_empty() {
                assert!(switched_at.is_some(), "went quiet without ever crossing");
                break;
            }
            if switched_at.is_none() && status.queue_index == 1 {
                switched_at = Some(started.elapsed());
                assert!(status.playing, "the boundary is seamless, not a gap");
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let switched = switched_at.expect("the boundary never crossed");
        println!(">>> gapless boundary at {switched:?}, done in {:?}", started.elapsed());
        // The whole point against a blend: the switch comes at the END.
        assert!(switched > Duration::from_secs_f64(3.7), "at {switched:?} this was a blend");
        assert!(switched < Duration::from_secs_f64(5.0), "at {switched:?} there was a gap");
        assert!(started.elapsed() > Duration::from_secs_f64(7.7), "audio went missing");

        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// A stop breathes (C4): the bookkeeping clears now, the sink drains
    /// its 80ms in the outgoing slot and is retired by the tick.
    ///
    /// `cargo test a_stop_breathes -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_stop_breathes_instead_of_clicking() {
        let tiny = std::env::temp_dir().join("mstream-softstop.wav");
        std::fs::write(&tiny, wav_bytes(3)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(tiny.to_string_lossy().into_owned(), Some(3.0)).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        engine.stop();
        let status = engine.status();
        assert!(status.file.is_empty() && !status.playing, "stopped now, to every asker");
        assert!(engine.overlap_active(), "while the sink breathes out in the slot");

        let waited = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(waited.elapsed() < Duration::from_secs(3), "the breath never ended");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_file(&tiny);
    }

    /// Gapless repeat-one: the loop seam crosses sample-tight, lap after
    /// lap, self-sustained — the case the C4 review found always gapped.
    ///
    /// `cargo test the_loop_seam -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn the_loop_seam_crosses_gaplessly_lap_after_lap() {
        let tiny = std::env::temp_dir().join("mstream-seam.wav");
        std::fs::write(&tiny, wav_bytes(3)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        engine.cycle_loop(); // None -> One
        engine.queue_add(tiny.to_string_lossy().into_owned());

        // Two full laps and change: the file must never empty, the index
        // must never move, and the position must be seen wrapping.
        let started = std::time::Instant::now();
        let mut wrapped = 0;
        let mut last_pos = 0.0;
        while started.elapsed() < Duration::from_secs(8) {
            engine.advance_tick();
            let status = engine.status();
            assert!(!status.file.is_empty(), "the seam gapped at lap {wrapped}");
            assert_eq!(status.queue_index, 0, "a loop goes nowhere");
            if status.position < last_pos - 1.0 {
                wrapped += 1;
            }
            last_pos = status.position;
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(wrapped >= 2, "only {wrapped} laps in 8s of a 3s loop");

        engine.stop();
        let _ = std::fs::remove_file(&tiny);
    }

    /// Crossfade toggled on mid-lap of a gapless loop: whatever seam the
    /// old mode prepared — pending or already Ready — must be refused, and
    /// the loop must fall back to the ordinary repeat restart rather than
    /// ever blending the track into itself (the pending-seam bug).
    ///
    /// `cargo test a_seam_never_survives -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_seam_never_survives_into_a_blend() {
        let tiny = std::env::temp_dir().join("mstream-seamblend.wav");
        std::fs::write(&tiny, wav_bytes(3)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        engine.cycle_loop(); // None -> One
        engine.queue_add(tiny.to_string_lossy().into_owned());

        // Let the seam machinery arm — prepare fires early on a 3s track —
        // then flip the mode mid-lap.
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(800) {
            engine.advance_tick();
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.set_crossfade(6.0);

        // Two laps of watching: a surviving seam would show up as a blend —
        // an overlap while the index stays put. The lawful outcome is the
        // plain loop-one restart, which never overlaps.
        while started.elapsed() < Duration::from_secs(8) {
            engine.advance_tick();
            let status = engine.status();
            assert!(
                !engine.overlap_active(),
                "the seam survived into a blend at t={:?}",
                started.elapsed()
            );
            assert!(!status.file.is_empty(), "the loop died instead");
            assert_eq!(status.queue_index, 0);
            std::thread::sleep(Duration::from_millis(50));
        }

        engine.stop();
        let _ = std::fs::remove_file(&tiny);
    }

    /// A transient prepare failure heals when the window recedes: seek back
    /// past it and the retry gets its blend (the Failed latch reset — the
    /// prior review's one fix that shipped without a test, per the audit).
    ///
    /// `cargo test a_healed_failure -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_healed_failure_retries_after_a_seek_back_and_blends() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-heal-a.wav");
        let second = dir.join("mstream-heal-b.wav");
        std::fs::write(&first, wav_bytes(15)).unwrap();
        // b does not exist yet: the first prepare fails like a network blip.

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.0);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        // Into the prepare window (remaining <= 13 of 15s), where the open
        // of the missing b fails and the latch parks at Failed.
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(2600) {
            engine.advance_tick();
            std::thread::sleep(Duration::from_millis(50));
        }

        // The blip heals, and the user seeks back — runway reopens.
        std::fs::write(&second, wav_bytes(4)).unwrap();
        engine.seek(0.0).unwrap();

        // The retry must land a real BLEND before a's natural end: an
        // overlap seen at the switch is the discriminator — without the
        // reset, the transition falls to the ordinary gap-advance instead.
        let mut blended = false;
        while started.elapsed() < Duration::from_secs(22) {
            engine.advance_tick();
            let status = engine.status();
            if status.queue_index == 1 {
                blended = engine.overlap_active();
                break;
            }
            assert!(!status.file.is_empty(), "playback died before the retry");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(blended, "the healed failure never got its blend");

        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// A stop that lands while a seek is blocked on the network must stay a
    /// stop: the seek's up-ramp may only fire on the handle that still
    /// steers the current sink, never on one retired in the meantime —
    /// resurrected, the "stopped" track came back at full volume until the
    /// deadline clicked it off (C4 review).
    ///
    /// `cargo test a_blocked_seeks_up_ramp -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_blocked_seeks_up_ramp_cannot_resurrect_a_stopped_track() {
        let url = stalling_wav_server(30, 500_000, Duration::from_secs(4));
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(url, Some(30.0)).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        // The stop lands from another thread while the seek below is still
        // waiting on data that has not downloaded.
        let state = engine.state.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            state.lock().unwrap().stop_softly();
        });

        let _ = engine.seek(20.0); // past the downloaded range; blocks ~4s
        stopper.join().unwrap();

        // The retired sink's ramp went to silence and stayed there. With
        // the up-ramp misaimed, its position would be pinned at 1.0.
        std::thread::sleep(Duration::from_millis(250));
        for position in engine.drainer_positions() {
            assert!(position <= 0.05, "a stopped track was ramped back up to {position}");
        }
        engine.stop();
    }

    /// A queue shrink while a track sits appended must shift the committed
    /// slot: promotion then points at the row the audio actually is, and
    /// the advance after it plays what really follows — unshifted, the
    /// third track here was skipped outright (C4 review).
    ///
    /// `cargo test a_queue_shrink_mid_append -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_queue_shrink_mid_append_still_plays_what_follows() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["x", "y", "z"]
            .iter()
            .map(|n| dir.join(format!("mstream-shrink-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(3)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        engine.queue_add_many(
            names.iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        );

        // y is appended (committed at index 1); then x's row is removed —
        // the queue is [y, z] and the commitment must follow y to index 0.
        let started = std::time::Instant::now();
        while !engine.appended_waiting() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(3), "nothing was ever appended");
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.queue_remove(0).unwrap();

        // z must still be reached: promotion lands on y at index 0, and the
        // ordinary advance carries on to z at index 1.
        let mut heard_z = false;
        while started.elapsed() < Duration::from_secs(12) {
            engine.advance_tick();
            let status = engine.status();
            if status.file == names[2].to_string_lossy() {
                heard_z = true;
            }
            if status.file.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(heard_z, "the unshifted commitment skipped the last track");

        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// A stop landing in the window between a gapless boundary crossing
    /// (audio thread) and its promotion (next tick) must treat the sounding
    /// source as current — promoted and breathed out, not snapped at full
    /// scale (C4 review). Without ticks, the promotion can ONLY come from
    /// silence_appended's own boundary check.
    ///
    /// `cargo test a_stop_at_the_boundary -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_stop_at_the_boundary_breathes_the_sounding_track_not_snaps_it() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-boundary-a.wav");
        let second = dir.join("mstream-boundary-b.wav");
        std::fs::write(&first, wav_bytes(3)).unwrap();
        std::fs::write(&second, wav_bytes(3)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        // Tick until the append is in, then STOP ticking and sleep past
        // the boundary — the exact unpromoted window.
        let started = std::time::Instant::now();
        while !engine.appended_waiting() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(3), "nothing was ever appended");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_secs_f64(
            (3.2 - started.elapsed().as_secs_f64()).max(0.1),
        ));

        engine.stop();
        assert!(!engine.appended_waiting(), "the boundary was seen by the stop itself");
        assert!(engine.overlap_active(), "the sounding track breathes out in the slot");
        // A breathing ramp retires in well under a second; the snapped-dead
        // handle of the OLD source would sit until the ~2s deadline.
        let waited = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(
                waited.elapsed() < Duration::from_secs(1),
                "retirement waited on the deadline — the wrong source was breathed"
            );
            std::thread::sleep(Duration::from_millis(30));
        }

        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// Toggling gapless off while a track is already appended must not
    /// orphan it: the boundary still promotes, the status still follows,
    /// and the track plays exactly once (C4 review).
    ///
    /// `cargo test gapless_toggled_off -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn gapless_toggled_off_mid_append_still_promotes_the_boundary() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-orphan-a.wav");
        let second = dir.join("mstream-orphan-b.wav");
        std::fs::write(&first, wav_bytes(4)).unwrap();
        std::fs::write(&second, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        // Tick into the append window (remaining <= 1.5s of a 4s track),
        // then flip the panel toggle off with the append already in.
        let started = std::time::Instant::now();
        while !engine.appended_waiting() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(4), "nothing was ever appended");
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.set_gapless(false);

        // The boundary must still be seen and the status must follow it.
        let mut switched = false;
        while started.elapsed() < Duration::from_secs(10) {
            engine.advance_tick();
            let status = engine.status();
            if status.file.is_empty() {
                break;
            }
            if status.queue_index == 1 {
                switched = true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(switched, "the appended track was orphaned — status never followed it");
        // Exactly once: were it orphaned, the ordinary advance would play
        // the second track AGAIN after the sink drained, pushing the total
        // run well past two tracks' worth.
        assert!(started.elapsed() < Duration::from_secs(9), "the appended track played twice");

        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// Clearing the queue and immediately adding must start playback even
    /// though the cleared track's stop-breath is still audibly draining —
    /// the autostart gate reads `stopped`, not the sink (C4 review, critic).
    ///
    /// `cargo test an_add_during_the_stop_breath -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn an_add_during_the_stop_breath_still_starts_playback() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["v", "w"]
            .iter()
            .map(|n| dir.join(format!("mstream-addbreath-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(4)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.queue_add(names[0].to_string_lossy().into_owned());
        std::thread::sleep(Duration::from_millis(300));

        // The wire sequence: POST /queue/clear, POST /queue/add — the add
        // lands well inside the 80ms breath.
        engine.queue_clear();
        engine.queue_add(names[1].to_string_lossy().into_owned());
        let status = engine.status();
        assert_eq!(status.file, names[1].to_string_lossy(), "the added track started");
        assert!(status.playing);

        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// Pause, then pick a different track: the paused sink must be stopped
    /// outright, not parked — parked it was a zombie no condition could
    /// retire, holding the transition gate shut for the whole session and
    /// becoming audible again on the next resume (C4 review).
    ///
    /// `cargo test a_paused_sink_is_stopped -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_paused_sink_is_stopped_not_parked_when_another_track_starts() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["t", "u"]
            .iter()
            .map(|n| dir.join(format!("mstream-pausepark-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(4)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        engine.queue_add_many(
            names.iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        );
        std::thread::sleep(Duration::from_millis(300));

        // The routine flow: pause, then choose another track.
        engine.pause();
        engine.queue_play_index(1).unwrap();
        assert_eq!(engine.drainers(), 0, "a paused sink is stopped, never parked");

        // And the transitions this zombie used to disable still work: the
        // new track is playing (a fresh start is not paused), its blend
        // machinery unobstructed.
        assert!(engine.status().playing, "the chosen track plays");

        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// A stop landing inside a skip's breath must not buy its drainer slot
    /// by hard-cutting the breath at speaking gain — the drainer list holds
    /// them both, and both retire on their own ramps (C4 review fleet fix).
    ///
    /// `cargo test breaths_share -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn breaths_share_the_drainer_list_instead_of_cutting_each_other() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["r", "s"]
            .iter()
            .map(|n| dir.join(format!("mstream-breaths-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(3)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.queue_add_many(
            names.iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        );
        std::thread::sleep(Duration::from_millis(300));

        // Skip (one breath begins), then stop 30ms into it (a second).
        engine.next_manual().unwrap();
        std::thread::sleep(Duration::from_millis(30));
        engine.stop();
        assert_eq!(
            engine.drainers(),
            2,
            "the skip's breath and the stop's breath drain side by side"
        );

        let waited = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(waited.elapsed() < Duration::from_secs(3), "the breaths never ended");
            std::thread::sleep(Duration::from_millis(30));
        }
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// A failed play must take the committed pick down with it: play_source
    /// replaces the whole queue before it opens anything, and a pick
    /// committed against the old queue surviving a failed open would blend
    /// into a track from a queue that no longer exists (the fix-verify
    /// pass caught this as the failed-jump finding's uncovered sibling).
    ///
    /// `cargo test a_failed_play_discards -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_failed_play_discards_the_committed_pick() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-crossfade-m.wav");
        let second = dir.join("mstream-crossfade-n.wav");
        std::fs::write(&first, wav_bytes(4)).unwrap();
        std::fs::write(&second, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        // Local opens land in microseconds: the pick of track n is
        // committed and Ready almost immediately.
        std::thread::sleep(Duration::from_millis(400));
        engine.advance_tick();

        // A play that cannot start replaces the queue and fails; the old
        // playback carries on, and the old commitment must be gone.
        let missing = dir.join("mstream-crossfade-no-such.wav");
        assert!(engine.play_source(missing.to_string_lossy().into_owned(), None).is_err());

        // Ride out the rest of the first track: the blend into n must
        // never fire — n belongs to a queue that was thrown away.
        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            let status = engine.status();
            assert!(
                status.file != second.to_string_lossy(),
                "a stale committed pick blended out of a replaced queue"
            );
            if status.file.is_empty() {
                break; // the old track ran out; the dead entry ends the queue
            }
            assert!(started.elapsed() < Duration::from_secs(8), "playback never wound down");
            std::thread::sleep(Duration::from_millis(50));
        }

        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// The wall clock runs through a pause; the blend must not. Deadline =
    /// fade (1.5s) + slack (2s) = 3.5s, and the pause outlasts it.
    ///
    /// `cargo test a_paused_blend -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_paused_blend_survives_a_pause_longer_than_its_deadline() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-crossfade-p.wav");
        let second = dir.join("mstream-crossfade-q.wav");
        std::fs::write(&first, wav_bytes(4)).unwrap();
        std::fs::write(&second, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        let started = std::time::Instant::now();
        while !engine.overlap_active() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(6), "no blend ever started");
            std::thread::sleep(Duration::from_millis(30));
        }

        engine.pause();
        std::thread::sleep(Duration::from_secs(4)); // well past the 3.5s deadline
        engine.advance_tick(); // the ticks keep coming while paused
        assert!(engine.overlap_active(), "the deadline must not fire mid-pause");

        engine.resume();
        engine.advance_tick();
        assert!(engine.overlap_active(), "resume re-arms the net; the blend goes on");

        // And the blend still ends on its own, the ordinary way.
        let resumed = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(resumed.elapsed() < Duration::from_secs(5), "the outgoing never retired");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(engine.status().playing, "the incoming carried on");

        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// A queue edit between prepare and handover must discard the committed
    /// pick: the blend then lands on what the edited queue actually says
    /// comes next, never on a track no longer in it.
    ///
    /// `cargo test a_queue_edit_mid_prepare -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_queue_edit_mid_prepare_discards_the_committed_pick() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["h", "i", "j"]
            .iter()
            .map(|n| dir.join(format!("mstream-crossfade-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(4)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        engine.queue_add_many(
            names.iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        );

        // Local files open in microseconds: by now track i is committed
        // and Ready. Then i leaves the queue.
        std::thread::sleep(Duration::from_millis(400));
        engine.advance_tick();
        engine.queue_remove(1).unwrap();

        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            let status = engine.status();
            if status.queue_index == 1 && status.file == names[2].to_string_lossy() {
                break; // the blend landed on j, the re-committed pick
            }
            assert!(
                status.file != names[1].to_string_lossy(),
                "the blend landed on the removed track"
            );
            assert!(started.elapsed() < Duration::from_secs(8), "no handover ever came");
            std::thread::sleep(Duration::from_millis(50));
        }

        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// `cargo test a_manual_next_mid_blend -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_manual_next_mid_blend_cuts_the_outgoing_sink() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["c", "d", "e"]
            .iter()
            .map(|n| dir.join(format!("mstream-crossfade-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(3)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        engine.queue_add_many(
            names.iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        );

        // Ride the ticks until the first blend is audibly under way.
        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            if engine.status().queue_index == 1 && engine.overlap_active() {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(6), "no blend ever started");
            std::thread::sleep(Duration::from_millis(30));
        }

        // A manual skip mid-blend: the blend must not outlive the skip —
        // but since C4 the leaving track breathes out for SKIP_FADE in the
        // slot instead of clicking, so "cut" means "gone within a breath",
        // not "gone this instant".
        engine.next_manual().expect("a third track was queued");
        let status = engine.status();
        assert_eq!(status.queue_index, 2);
        assert!(status.playing);
        let waited = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(
                waited.elapsed() < Duration::from_secs(2),
                "the skip breath never ended — the blend outlived the skip"
            );
            std::thread::sleep(Duration::from_millis(30));
        }

        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// The tap hears real decoded audio, not silence and not nothing.
    ///
    /// `MSTREAM_TRACK="<library path>" cargo test the_tap_hears -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a live server and MSTREAM_TRACK"]
    fn the_tap_hears_what_is_playing() {
        let path = std::env::var("MSTREAM_TRACK").expect("MSTREAM_TRACK");
        let client = crate::api::Client::resolve(None, None).unwrap();
        let url = client.media_url(&path).unwrap();

        let engine = Engine::new().unwrap();
        // The tap sits inside the source, so it sees the music at full
        // amplitude however loud you are listening. That is what you want
        // drawn: the track, not the volume knob.
        engine.set_volume(0.0);
        let tap = tap::AudioTap::new();
        engine.attach_tap(tap.clone());
        engine.play_source(url, None).unwrap();

        // Past any leading silence, and past the first batch.
        std::thread::sleep(Duration::from_secs(3));
        let frame = tap.frame().expect("the tap should be holding audio by now");
        let peak = frame.samples.iter().fold(0.0f32, |loudest, s| loudest.max(s.abs()));
        println!(
            ">>> {} samples  {} Hz  {} ch  peak {peak:.3}",
            frame.samples.len(),
            frame.rate,
            frame.channels
        );
        engine.stop();

        let held = tap::TAP_FRAMES * frame.channels as usize;
        assert_eq!(frame.samples.len(), held, "the ring should be full");
        assert!(frame.rate >= 8000, "a real sample rate, got {}", frame.rate);
        assert!((1..=8).contains(&frame.channels), "a real channel count");
        assert!(peak > 0.01, "the tap heard silence: peak {peak}");
        assert_eq!(frame.mono().len(), tap::TAP_FRAMES, "the same frames whatever the shape");
    }

    /// Seeking repeatedly in one streamed track keeps playing.
    ///
    /// Kept because the opposite was reported, investigated and wrong: a
    /// replay script that ended on a `frame` step printed its last screen
    /// twice, and two identical samples read as a stall. This asks the engine
    /// directly, where there is no screen to misread.
    ///
    /// `MSTREAM_TRACK="<library path>" cargo test seeking_more_than_once -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a live server and MSTREAM_TRACK"]
    fn seeking_more_than_once_keeps_playing() {
        let path = std::env::var("MSTREAM_TRACK").expect("MSTREAM_TRACK");
        let client = crate::api::Client::resolve(None, None).unwrap();
        let url = client.media_url(&path).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(url, None).unwrap();

        let report = |label: &str| {
            let s = engine.status();
            println!(">>> {label}: pos={:.2} playing={} dur={:.2}", s.position, s.playing, s.duration);
            s.position
        };

        std::thread::sleep(Duration::from_secs(4));
        report("after 4s");

        println!(">>> SEEK 1 -> 70");
        engine.seek(70.0).unwrap();
        for i in 0..6 {
            std::thread::sleep(Duration::from_secs(1));
            report(&format!("seek1 +{}s", i + 1));
        }

        println!(">>> SEEK 2 -> 20");
        engine.seek(20.0).unwrap();
        let mut positions = Vec::new();
        for i in 0..10 {
            std::thread::sleep(Duration::from_secs(1));
            positions.push(report(&format!("seek2 +{}s", i + 1)));
        }

        let moved = positions.last().unwrap() - positions.first().unwrap();
        println!(">>> advanced {moved:.2}s over the 9s after the second seek");
        assert!(moved > 5.0, "playback stalled after the second seek");
    }

    fn q(len: usize, index: usize) -> QueueState {
        QueueState {
            queue: (0..len).map(|i| QueueEntry::new(format!("t{}", i))).collect(),
            index,
            shuffle: false,
            loop_mode: LoopMode::None,
        }
    }

    #[test]
    fn advance_linear_and_end() {
        let state = q(3, 0);
        assert_eq!(pick_next(&state, false), Some(1));
        let state = q(3, 2);
        assert_eq!(pick_next(&state, false), None);
    }

    #[test]
    fn advance_empty_queue() {
        let state = q(0, 0);
        assert_eq!(pick_next(&state, false), None);
        assert_eq!(pick_next(&state, true), None);
    }

    #[test]
    fn loop_all_wraps() {
        let mut state = q(3, 2);
        state.loop_mode = LoopMode::All;
        assert_eq!(pick_next(&state, false), Some(0));
        assert_eq!(pick_next(&state, true), Some(0));
    }

    #[test]
    fn loop_one_repeats_auto_but_not_manual() {
        let mut state = q(3, 1);
        state.loop_mode = LoopMode::One;
        // Auto-advance honors loop-one.
        assert_eq!(pick_next(&state, false), Some(1));
        // Manual next escapes it (audit fix #2).
        assert_eq!(pick_next(&state, true), Some(2));
        // Manual next at the end of the queue under loop-one ends the queue.
        state.index = 2;
        assert_eq!(pick_next(&state, true), None);
    }

    #[test]
    fn shuffle_picks_a_different_index() {
        let mut state = q(5, 2);
        state.shuffle = true;
        for seed in 0..50 {
            fastrand::seed(seed);
            let next = pick_next(&state, false).unwrap();
            assert_ne!(next, 2);
            assert!(next < 5);
        }
    }

    #[test]
    fn shuffle_single_track() {
        let mut state = q(1, 0);
        state.shuffle = true;
        assert_eq!(pick_next(&state, false), Some(0));
    }

    #[test]
    fn a_fade_never_swallows_more_than_half_the_track() {
        assert_eq!(effective_fade(6.0, 300.0), 6.0);
        assert_eq!(effective_fade(6.0, 7.0), 3.5, "a short track caps the blend at half");
        assert_eq!(effective_fade(0.0, 300.0), 0.0, "off is off");
        assert_eq!(effective_fade(6.0, 0.0), 0.0, "no known length, no fade point");
        assert_eq!(effective_fade(-3.0, 300.0), 0.0);
        assert_eq!(effective_fade(f32::NAN, 300.0), 0.0);
    }

    #[test]
    fn the_blend_window_respects_both_tracks() {
        // The ordinary case: the configured length, nothing in the way.
        assert_eq!(blend_window(6.0, 300.0, 100.0, 300.0), 6.0);
        // A short incoming track halves the window against ITS length, or
        // its whole life is a partial ramp and its end is a hard cut.
        assert_eq!(blend_window(30.0, 300.0, 30.0, 12.0), 6.0);
        // What actually remains bounds everything.
        assert_eq!(blend_window(6.0, 300.0, 2.0, 300.0), 2.0);
        // An unknown incoming length caps nothing — there is no length to
        // halve. Nothing refuses to blend *into* the unknown (only out of
        // it), so a hint-less stream that turns out short still ends in a
        // cut; accepted, since a cap cannot exist without a number.
        assert_eq!(blend_window(6.0, 300.0, 100.0, 0.0), 6.0);
        // The floor holds against a window that arrived too late.
        assert!(blend_window(6.0, 300.0, 0.0, 300.0) >= MIN_FADE);
        assert!(blend_window(6.0, 300.0, -3.0, 300.0) >= MIN_FADE, "past the end still blends");
    }

    #[test]
    fn nothing_ever_blends_into_itself_but_gapless_loops_its_seam() {
        let mut state = q(3, 1);
        state.loop_mode = LoopMode::One;
        assert!(next_candidate(&state, false).is_none(), "loop-one repeats, it does not blend");
        // The gapless side of the same coin: the seam is the point.
        let (entry, index) = next_candidate(&state, true).expect("gapless loops the seam");
        assert_eq!((entry.path.as_str(), index), ("t1", Some(1)));

        let mut single = q(1, 0);
        single.loop_mode = LoopMode::All;
        assert!(
            next_candidate(&single, false).is_none(),
            "one track looping is loop-one in effect"
        );
        assert!(next_candidate(&single, true).is_some(), "and gapless loops that too");

        let plain = q(3, 0);
        let (entry, index) = next_candidate(&plain, false).expect("a plain queue has a next");
        assert_eq!((entry.path.as_str(), index), ("t1", Some(1)));

        let end = q(3, 2);
        assert!(next_candidate(&end, false).is_none(), "nothing follows the last track unlooped");
        assert!(next_candidate(&end, true).is_none(), "gapless invents no next either");
    }

    #[test]
    fn remove_before_current_shifts_index() {
        let mut state = q(4, 2);
        assert_eq!(apply_remove(&mut state, 0), RemoveOutcome::RemovedBeforeCurrent);
        assert_eq!(state.index, 1);
        assert_eq!(state.queue.len(), 3);
    }

    #[test]
    fn remove_after_current_keeps_index() {
        let mut state = q(4, 1);
        assert_eq!(apply_remove(&mut state, 3), RemoveOutcome::RemovedAfterCurrent);
        assert_eq!(state.index, 1);
    }

    #[test]
    fn remove_current_mid_queue_points_at_successor() {
        let mut state = q(4, 1);
        assert_eq!(apply_remove(&mut state, 1), RemoveOutcome::RemovedCurrent);
        // Index unchanged — it now points at the track that followed.
        assert_eq!(state.index, 1);
        assert_eq!(state.queue[1].path, "t2");
    }

    #[test]
    fn remove_current_at_end_clamps() {
        let mut state = q(3, 2);
        assert_eq!(apply_remove(&mut state, 2), RemoveOutcome::RemovedCurrent);
        assert_eq!(state.index, 1);
    }

    #[test]
    fn remove_last_track_empties() {
        let mut state = q(1, 0);
        assert_eq!(apply_remove(&mut state, 0), RemoveOutcome::EmptiedQueue);
        assert_eq!(state.index, 0);
        assert!(state.queue.is_empty());
    }
}
