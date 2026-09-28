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
pub(crate) mod output;
pub(crate) mod tap;
pub(crate) mod trace;

use trace::etrace;

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use rodio::mixer::Mixer;
use rodio::{Decoder, Player, Source};
use serde::Serialize;

use crate::player::DeviceNotice;
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
    /// A play given up because a newer command made it moot while its
    /// source was still opening. Not a failure: playback stands as it was,
    /// and the newer command says what happens next.
    Superseded,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NoDevice(e) => write!(f, "Audio device unavailable: {}", e),
            EngineError::Unplayable(e) => write!(f, "Source could not be played: {}", e),
            EngineError::OutOfBounds => write!(f, "Index out of bounds"),
            EngineError::EndOfQueue => write!(f, "Already at end of queue"),
            EngineError::Seek(e) => write!(f, "Seek failed: {}", e),
            EngineError::Superseded => write!(f, "Superseded by a newer command"),
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
/// How long a direct play may spend opening before it counts as failed.
/// `http::OPEN_TIMEOUT` clocks the request and headers, but the decode
/// probe after it reads the stream body with the network's full patience
/// — stream-download's watchdog reconnects a stalled read forever, by
/// design. Against a connection that black-holes mid-probe (a wifi flap's
/// signature), that patience parked the audio thread, the state lock and
/// every status behind it until the process died: audio from the old sink
/// kept playing under a "starting" that could never end. The bound turns
/// that into an ordinary failed open, which the queue already knows how
/// to survive.
#[cfg(not(test))]
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// Tests stall sockets for real; nobody wants twenty seconds of it.
#[cfg(test)]
const START_TIMEOUT: Duration = Duration::from_millis(1500);

/// How often a direct open waiting on its thread asks whether it is still
/// wanted (performance audit #79). Short enough that a stop or a newer play
/// feels immediate; the question is a few channel reads.
const SUPERSEDE_POLL: Duration = Duration::from_millis(20);

/// How many opens given up for a newer command may still be running
/// (giving one up does not stop it mid-request) before the next one waits
/// for one of them to finish. Skimming a remote queue would otherwise start
/// an open per keypress, every one pulling its probe over the link the
/// wanted one needs (performance audit #79) — where the old blocking wait,
/// with collapse, never ran more than two in a row.
const MAX_ABANDONED: usize = 2;

/// An open running on its own thread: the channel its answer comes down,
/// and the thread.
struct Opener {
    rx: mpsc::Receiver<Result<Prepared, String>>,
    thread: std::thread::JoinHandle<()>,
}

impl Opener {
    /// Walk away from the open. The receiver goes now, so the answer drops
    /// into a closed channel on the open's own thread the moment it
    /// arrives — the reader, its download and its spool file with it — as
    /// a deadline's abandonment always has. A receiver kept to count the
    /// open by kept that answer too: a live reader downloading a track
    /// nobody wanted, whole, beside the one that was, until something next
    /// looked at the list (review of audit #79). The thread is handed back
    /// for the counting — see [`MAX_ABANDONED`].
    fn give_up(self) -> std::thread::JoinHandle<()> {
        self.thread
    }
}

/// Why a direct open produced no source.
enum OpenError {
    /// It failed, and why — for the logs and the queue's failure path.
    Failed(String),
    /// A newer command made it moot before it finished — with the opener
    /// when one was running, for the engine to give up and count.
    Superseded(Option<Opener>),
}

/// [`open_entry`] with a deadline, for the path that blocks the audio
/// thread. Same thread-and-channel shape as [`spawn_prepare`], and the
/// same abandonment contract: a result that arrives after the deadline
/// drops into a closed channel, taking the reader and its spool file
/// with it.
fn open_entry_bounded(entry: &QueueEntry) -> Result<Prepared, String> {
    open_entry_unless(entry, &mut || false).map_err(|e| match e {
        OpenError::Failed(reason) => reason,
        // Never asked, so never given up; for the match's sake.
        OpenError::Superseded(_) => EngineError::Superseded.to_string(),
    })
}

/// [`open_entry_bounded`], giving up as soon as `superseded` says the
/// source is no longer wanted — see [`await_open`].
fn open_entry_unless(
    entry: &QueueEntry,
    superseded: &mut dyn FnMut() -> bool,
) -> Result<Prepared, OpenError> {
    if !http::is_http_url(&entry.path) {
        // Local files open or fail in microseconds; a thread per open
        // would be pure ceremony.
        return open_entry(entry).map_err(OpenError::Failed);
    }
    let (tx, rx) = mpsc::channel();
    let moved = entry.clone();
    let spawned = std::thread::Builder::new()
        // The prepare thread's name, deliberately: its pact with the
        // panic hook (symphonia panics on malformed files; the hook
        // stands back for this name) is exactly the pact this thread
        // needs.
        .name(PREPARE_THREAD.into())
        .spawn(move || {
            let opened =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_entry(&moved)));
            if let Ok(result) = opened {
                let _ = tx.send(result);
            }
        });
    let Ok(thread) = spawned else {
        // A box that cannot spawn a thread still deserves its music; the
        // unbounded open is the behaviour this path always had.
        return open_entry(entry).map_err(OpenError::Failed);
    };
    await_open(Opener { rx, thread }, superseded)
}

/// Wait for an opener's answer: at most [`START_TIMEOUT`], and only for as
/// long as the source is wanted. The wait parks the audio thread, so a
/// stop or a newer play used to queue behind it — the rest of a doomed
/// open, seconds over a tunnel, twenty against a stall — and then find the
/// unwanted track installed and sounding for a moment before it could act
/// (performance audit #79). Now the thread asks `superseded` every
/// [`SUPERSEDE_POLL`] and walks away on a yes, handing the opener back to
/// be given up ([`Opener::give_up`]) — the same abandonment the deadline
/// uses. The open-then-swap order is untouched: nothing has happened to
/// the playing sink yet.
fn await_open(
    opener: Opener,
    superseded: &mut dyn FnMut() -> bool,
) -> Result<Prepared, OpenError> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match opener.rx.recv_timeout(left.min(SUPERSEDE_POLL)) {
            Ok(result) => return result.map_err(OpenError::Failed),
            // The open thread panicked and the catch dropped the sender.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(OpenError::Failed("the decoder gave up on the stream".into()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return Err(OpenError::Failed(format!(
                        "the stream stalled while opening — gave up after {}s",
                        START_TIMEOUT.as_secs()
                    )));
                }
                if superseded() {
                    return Err(OpenError::Superseded(Some(opener)));
                }
            }
        }
    }
}

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

/// The window when a manual skip BLENDS instead of breathing (opt-in): a
/// second is musical without making the transport feel slow, and it stays
/// fixed rather than borrowing crossfade_seconds — an eight-second blend
/// is lovely at a track's natural end and treacle on a keystroke.
const SKIP_BLEND: Duration = Duration::from_secs(1);

/// The ramp either side of an opt-in soft pause: down before the pause
/// lands, up as the resume begins.
const PAUSE_FADE: Duration = Duration::from_millis(150);

/// The breath on a stop. Shorter than a skip: a stop should feel like now.
const STOP_FADE: Duration = Duration::from_millis(80);

/// The dip around a seek: down before the jump, up after it. A seek lands
/// mid-waveform on both sides, and forty milliseconds of valley is cheaper
/// on the ear than two clicks.
const SEEK_DIP_DOWN: Duration = Duration::from_millis(10);
const SEEK_DIP_UP: Duration = Duration::from_millis(30);

/// What a forward seek must leave on the clock beyond the transition
/// itself, so the next track's open can finish before the window arrives.
/// Two seconds covers an ordinary open with the spool warm behind it; a
/// slower network degrades to a shortened blend rather than a missed one.
const OPEN_RUNWAY: f64 = 2.0;

/// How long a failed open rests before the tick may try it again. Long
/// enough that a dead URL costs a handful of opens per track tail rather
/// than one per tick; short enough that a link merely busy (a seek's spool
/// catch-up) gets its retry while the window can still be met.
const FAILED_RETRY: Duration = Duration::from_secs(4);

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

/// The latest position a *forward* seek may land when a seamless transition
/// is configured, or None when nothing guards the end. A seek that lands
/// inside the transition's own runway starves it: the open cannot finish,
/// the window cannot fit, and the seam the listener configured plays as a
/// hard cut — which is exactly how the bug report read ("skipped to the
/// last minute and the crossfade didn't happen"; the landings were nearer
/// the end than the open could serve). Past the end entirely, the decoder
/// runs dry on the spot and the track dies mid-keystroke. With crossfade
/// and gapless both off there is nothing to starve, and seeking past the
/// end keeping its skip-the-track meaning is the legacy behavior.
fn seek_ceiling(duration: f64, crossfade: f32, gapless: bool) -> Option<f64> {
    if duration <= 0.0 {
        return None;
    }
    let reserve = if crossfade > 0.0 {
        effective_fade(crossfade, duration) + OPEN_RUNWAY
    } else if gapless {
        APPEND_LEAD + OPEN_RUNWAY
    } else {
        return None;
    };
    Some((duration - reserve).max(0.0))
}

/// What plays after the current track, as far as anyone has decided.
enum NextTrack {
    /// Nothing decided — the resting state, and all of it when crossfade
    /// is off.
    Idle,
    /// A thread is opening the pick; its answer arrives on `opener`.
    /// Dropping the receiver is the cancellation: the opener's send fails,
    /// the decoder drops, and its spool file deletes itself.
    Opening { index: Option<usize>, opener: Opener },
    /// Opened, decoded, spooling — waiting for the fade window to arrive.
    Ready { prepared: Prepared, index: Option<usize> },
    /// The open failed, and when. Remembered so the tick does not walk
    /// into the same doomed open every 120 ms; the timestamp is what turned
    /// the latch into a rate limit — one failure used to stand for the
    /// whole rest of the track, which read as "crossfade off" whenever a
    /// transient starved an open late in the track (a seek's spool
    /// catch-up over a slow link was the reported case). Retried on a
    /// clock while enough runway remains; a truly dead URL costs a
    /// handful of opens per track tail, not one per tick.
    Failed { at: Instant },
}

impl NextTrack {
    /// The variant's name, for the flight recorder.
    fn name(&self) -> &'static str {
        match self {
            NextTrack::Idle => "idle",
            NextTrack::Opening { .. } => "opening",
            NextTrack::Ready { .. } => "ready",
            NextTrack::Failed { .. } => "failed",
        }
    }
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
        Ok(thread) => NextTrack::Opening { index, opener: Opener { rx, thread } },
        // A box that cannot spawn a thread still has the ordinary advance
        // path; a blend is not worth an error.
        Err(_) => NextTrack::Failed { at: Instant::now() },
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

/// A next track taken over by a play — see [`State::take_ahead`].
enum Ahead {
    Ready(Prepared),
    Opening(Opener),
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
    /// (with gapless and prepare_plain also off) means no prefetch and no
    /// transitions, as before Phase C; the soft cuts on manual
    /// skip/stop/seek are the one deliberate global departure (C4) — the
    /// old engine clicked.
    crossfade: f32,
    /// Sample-tight transitions when no blend is configured: the prepared
    /// next is appended to the playing sink instead of overlapped on a
    /// second one. Off by default for the same compatibility reason.
    gapless: bool,
    /// With no transition configured, prepare the next track anyway and
    /// start it from the prepared decoder at the plain cut, so the boundary
    /// never waits on its open — which, synchronous, held the state lock
    /// and serve's whole loop through a file probe or a network fetch
    /// (performance audit #77). The seam is still the hard cut. Off by
    /// default like the others: prefetch is a behavior the engine never
    /// shows unasked (serve's `--prefetch` asks).
    prepare_plain: bool,
    /// The source waiting inside the sink for its gapless boundary.
    appended: Option<Appended>,
    /// Manual skips blend for [`SKIP_BLEND`] instead of breathing (C6).
    blend_skips: bool,
    /// Pause rides a [`PAUSE_FADE`] ramp down instead of landing mid-wave
    /// (C6). The pause itself is applied by the tick once the ramp lands.
    pause_fade: bool,
    /// A soft pause in flight: the moment past which the tick may land it,
    /// whatever the ramp reports. None means no pause is owed.
    pausing: Option<Instant>,
    /// A silenced remnant still queued in the LIVE sink: the one queue edit
    /// the irrevocable append can heed (deleting the appended row) snaps
    /// the source silent, but rodio would still play its silence to
    /// completion at the boundary — dead air for the removed track's whole
    /// length (verify round, critic). This flag is the record; the tick
    /// skips the remnant the moment it becomes current.
    orphaned_tail: bool,
    /// Which "waiting" gate the recorder was last told about, so a gate
    /// that holds for minutes is one line rather than eight a second.
    ///
    /// The tick calls `crossfade_step` at ~8 Hz whether or not anything has
    /// changed, so a level-triggered line here is a firehose: a player
    /// paused near a track boundary would fill the log's whole in-memory
    /// ring with one repeated sentence in about four minutes, evicting the
    /// session it exists to hold (PR #5 review). Edge-triggered, the same
    /// gate says its piece once and then keeps quiet.
    gate_noted: Option<&'static str>,
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
        let prepared = open_entry_bounded(&entry).map_err(EngineError::Unplayable)?;
        self.install(mixer, prepared);
        Ok(())
    }

    /// The sink swap every start shares: whatever was mid-blend,
    /// mid-prepare, or already appended is a plan that is not happening,
    /// the track being left gets a skip-length breath instead of a click
    /// (C4), and `prepared` becomes the current sound.
    fn install(&mut self, mixer: &Mixer, prepared: Prepared) {
        self.cancel_overlap();
        self.adopt_appended_for_retire();
        // A skip can blend (C6, opt-in): when something is audibly playing,
        // the leaving track gets a one-second window instead of a breath
        // and the incoming rises through the same second. Nothing audible —
        // the natural-advance fallback, a stopped or paused sink — keeps
        // the plain start; the blend is for the keystroke that interrupts.
        let audible = !self.sink.empty() && !self.stopped && !self.sink.is_paused();
        let blending_skip = self.blend_skips && audible;
        self.retire_softly(if blending_skip { SKIP_BLEND } else { SKIP_FADE });
        // A pause owed but not yet landed is cancelled by choosing a new
        // track: the user's later intent wins, and the new track plays.
        self.pausing = None;
        // Any orphaned remnant leaves with the retired sink, where the
        // fleet's reaping bounds it; the flag described the OLD sink.
        self.orphaned_tail = false;
        let sink = Player::connect_new(mixer);
        sink.set_volume(self.volume);
        self.attach_fresh(&sink, prepared.opened, if blending_skip { 0.0 } else { 1.0 });
        if blending_skip {
            self.fade.ramp_to(1.0, SKIP_BLEND);
        }
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
        // the next resume (C4 review). Stopped is stopped — EXCEPT that
        // after a soft stop, self.sink still aliases the parked drainer
        // until the next start replaces it, and stopping "the old current"
        // here would hard-cut that breath at speaking gain (verify round,
        // critic: the /stop-then-/play click). A parked sink belongs to
        // the fleet's retirement, not to this exit.
        if self.sink.empty() || self.stopped || self.sink.is_paused() {
            let parked =
                self.outgoing.iter().any(|out| Arc::ptr_eq(&out.sink, &self.sink));
            if !parked {
                self.sink.stop();
            }
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

    /// Catch up with a gapless boundary that crossed since the last look.
    /// Public mutators call this FIRST, before staging any index or queue
    /// state: promotion writes both, and it must never overwrite what a
    /// caller has just decided (fix-round review: the promotion buried in
    /// install() clobbered freshly staged state).
    fn promote_if_crossed(&mut self) {
        if self.appended.is_some() && self.sink.len() <= 1 {
            self.promote_appended();
        } else if self.orphaned_tail && self.sink.len() <= 1 {
            // The silenced remnant of a deleted row became the sink's
            // current: skip it now, so its silence never stalls the
            // advance. The skip lands at the next periodic access and the
            // ordinary end-of-track machinery takes over from there.
            self.sink.skip_one();
            self.orphaned_tail = false;
        }
    }

    /// An appended source, at a caller about to retire the sink. Still
    /// queued, it is snapped silent before its first sample — click-free —
    /// and plays out unheard under the retirement. If its boundary crossed
    /// while THIS caller held the lock (an open can take seconds), the
    /// appended IS the sounding track: its fade and tap are adopted so the
    /// retire breathes it out instead of snapping it at full scale — but
    /// no queue bookkeeping happens here, because the caller's own staging
    /// supersedes the appended slot's (fix-round review).
    fn adopt_appended_for_retire(&mut self) {
        let Some(appended) = self.appended.take() else { return };
        if self.sink.len() <= 1 {
            self.tap_live.store(false, Ordering::Relaxed);
            self.fade = appended.fade;
            self.tap_live = appended.tap_live;
        } else {
            appended.fade.snap(0.0);
            appended.tap_live.store(false, Ordering::Relaxed);
        }
    }

    /// Hush the long drainers: a blend's outgoing half re-ramps to silence
    /// over a stop's breath, from wherever its ramp stands, deadline
    /// tightened to match. The short breaths are left entirely alone —
    /// they ARE the click-free cut, and cutting a cut is how the fleet's
    /// own reason for existing was being defeated (fix-round review: every
    /// start hard-stopped the fleet; a stop let a 30s blend tail play on).
    fn hush_drainers(&mut self) {
        for out in &mut self.outgoing {
            if out.fade_dur > SKIP_FADE {
                out.fade.ramp_to(0.0, STOP_FADE);
                out.fade_dur = STOP_FADE;
                out.deadline = Instant::now() + STOP_FADE + OUTGOING_SLACK;
            }
        }
    }

    /// Stop what is playing with a breath instead of a click: the audible
    /// sink ramps out and drains in the outgoing slot while the bookkeeping
    /// clears now — status says stopped immediately, the ear gets 80ms of
    /// mercy.
    fn stop_softly(&mut self) {
        // The boundary first: a stop landing on a crossed seam should stop
        // the track that is SOUNDING — promotion advances the index so a
        // later resume restarts the right row (verify round, sweep).
        self.promote_if_crossed();
        // A stop means everything winds down NOW: the blend's long tail is
        // hushed to a stop-length ramp (left alone it played on for the
        // whole blend window — fix-round review, critic), the breaths
        // finish the few ms they have, and the playing sink joins them.
        self.hush_drainers();
        self.adopt_appended_for_retire();
        self.retire_softly(STOP_FADE);
        self.pausing = None;
        self.orphaned_tail = false;
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

    /// The next track, taken out of the slot, when it is `source` and has
    /// already been opened or is opening: a manual pick of what was
    /// announced (or committed) to follow, inside its prepare window. The
    /// play takes it over rather than throwing it away and fetching the
    /// same track again with the audio thread waiting (performance audit
    /// #79). Anything else stays where it was.
    fn take_ahead(&mut self, source: &str) -> Option<Ahead> {
        match std::mem::replace(&mut self.next, NextTrack::Idle) {
            NextTrack::Ready { prepared, .. } if prepared.path == source => {
                Some(Ahead::Ready(prepared))
            }
            NextTrack::Opening { index, opener }
                if match index {
                    None => self.pending_next.as_ref().is_some_and(|e| e.path == source),
                    Some(at) => self.q.queue.get(at).is_some_and(|e| e.path == source),
                } =>
            {
                Some(Ahead::Opening(opener))
            }
            other => {
                self.next = other;
                None
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

    /// A change of plan mid-blend: hush the blend out of the air and forget
    /// the prepared next. Nobody pressing next wants the old track
    /// lingering under the new — but nobody wants the click of a hard cut
    /// either, so the blend gets a stop-length ramp down, and the short
    /// breaths already draining are left to finish in the fleet.
    fn cancel_overlap(&mut self) {
        self.hush_drainers();
        self.invalidate_next();
    }

    /// A seek keeps the surviving track and takes the blend out from
    /// around it: the outgoing half is hushed down a stop-length ramp, a
    /// half-risen fade snaps to full. The prepared next, if any, stays —
    /// a seek does not change what comes after.
    /// Say a gate is holding — once per stretch of it holding, not once
    /// per tick. See [`State::gate_noted`].
    fn note_gate(&mut self, what: &'static str) {
        if self.gate_noted == Some(what) {
            return;
        }
        self.gate_noted = Some(what);
        etrace!("{what}");
    }

    fn snap_out_of_blend(&mut self) {
        if self.outgoing.is_empty() {
            return;
        }
        // Hushed, not stopped: the leaving half of the blend was mid-ramp
        // at speaking gain, and a hard stop there is a click like any
        // other (fix-round review made every audible cut a ramp).
        self.hush_drainers();
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

    /// Whether nothing here can change until a command arrives: stopped, or
    /// paused with the pause landed and a track still in the sink — and no
    /// breath draining, no orphaned remnant to skip, no open in flight.
    /// Every other state has something the tick will do on its own.
    fn at_rest(&self) -> bool {
        let still = self.stopped || (self.sink.is_paused() && !self.sink.empty());
        still
            && self.pausing.is_none()
            && self.outgoing.is_empty()
            && !self.orphaned_tail
            && !matches!(self.next, NextTrack::Opening { .. })
    }

    /// Seconds left of the sounding track — negative once it has run past
    /// the length it claimed — or None when nothing is sounding toward a
    /// known end: stopped, paused or ramping into a pause, sink empty, or
    /// a length the engine never learned.
    fn until_end(&self) -> Option<f64> {
        if self.stopped
            || self.sink.is_paused()
            || self.pausing.is_some()
            || self.sink.empty()
            || self.duration <= 0.0
        {
            return None;
        }
        Some(self.duration - self.sink.get_pos().as_secs_f64())
    }
}

/// How far ahead of a track's computed end a driver aims its next tick,
/// so the tick lands as the source runs dry rather than just after.
const END_EARLY: f64 = 0.03;

/// The shortest wait near a track's end. rodio moves the position every
/// 5 ms of audio; ticking faster would only read the same number again.
const END_POLL: Duration = Duration::from_millis(5);

/// How long past its claimed length a track may run before the driver
/// stops watching for its end closely. A length that undershoots — a wrong
/// hint, a VBR header without a frame count — would otherwise hold the
/// driver at END_POLL until the real end, which could be minutes of 200
/// ticks a second; with the grace it costs at most a second of them.
const END_GRACE: f64 = 1.0;

/// When a driver that ticks every `base` should tick next. Settled, once
/// per [`DEVICE_POLL`] (performance audit #81). With a track sounding
/// toward a known end, just before that end — never sooner than
/// [`END_POLL`], never later than `base` — because the tick is what notices
/// a source ran out and starts the next one, and at a flat 250 ms the
/// notice came U(0, 250) ms late: silence added to every natural boundary
/// (performance audit #76; the gap Phase 1 #8 accepted, narrowed without
/// changing the cut). Anything else keeps `base`.
fn next_tick(settled: bool, until_end: Option<f64>, base: Duration) -> Duration {
    if settled {
        return base.max(DEVICE_POLL);
    }
    match until_end {
        Some(left) if left > -END_GRACE => {
            Duration::from_secs_f64((left - END_EARLY).max(0.0)).clamp(END_POLL.min(base), base)
        }
        _ => base,
    }
}

/// How often the system default output is compared with the one the
/// stream opened on. Plugging in headphones (or a Bluetooth speaker
/// connecting) moves the default without touching the running stream —
/// the OS leaves it playing on the old endpoint — so the only way to
/// hear about it is to ask. Also how long the drivers wait between ticks
/// while the engine is [settled](Engine::settled): the watch is then the
/// only thing the tick has to do, and it keeps its pace.
pub(crate) const DEVICE_POLL: Duration = Duration::from_secs(1);

/// How long a failed output rebuild rests before the next try: the
/// outage where nothing will open at all (the lone Bluetooth headset
/// switched off, say). The engine holds its state and knocks until a
/// device answers.
const REBUILD_RETRY: Duration = Duration::from_secs(2);

/// How long a stopped engine — or one that has never played anything —
/// keeps the device stream running before it lets it sleep (performance
/// audit #75). Long enough that a stop's breath, and whatever a stop left
/// in the mixer, have long since played out; short next to what a running
/// stream costs: the machine it keeps from idle-sleeping.
#[cfg(not(test))]
const IDLE_STOPPED: Duration = Duration::from_secs(5);
/// How long a landed pause keeps it running. Longer than a stop: resuming
/// from a sleeping stream restarts the device, which takes a few ms on a
/// wired output but can clip the first few hundred on a Bluetooth link,
/// and a short pause should come back exactly as it always has.
#[cfg(not(test))]
const IDLE_PAUSED: Duration = Duration::from_secs(30);
/// Tests watch the stream go to sleep; nobody wants half a minute of it.
#[cfg(test)]
const IDLE_STOPPED: Duration = Duration::from_millis(400);
#[cfg(test)]
const IDLE_PAUSED: Duration = Duration::from_millis(800);

/// The output device under watch, and the watch's own bookkeeping.
struct OutputWatch {
    out: output::Output,
    /// When something last needed the device callback: a command about to
    /// lean on a Player, or a tick that found the engine anything but at
    /// rest. The idle clock that suspends the stream runs from here.
    active_at: Instant,
    /// When the default-device identity was last polled.
    polled: Instant,
    /// When a rebuild last failed outright, so the retries pace
    /// themselves instead of hammering the host every tick.
    failed_at: Option<Instant>,
    /// Whether the current outage has been announced — once, not once
    /// per retry.
    outage_told: bool,
}

impl OutputWatch {
    fn mixer(&self) -> &Mixer {
        self.out.mixer()
    }
}

pub struct Engine {
    // Field order matters: `state` (and the Players inside it) must drop
    // before the output that owns the device stream.
    state: Arc<Mutex<State>>,
    /// Behind a lock because a dead device is replaced mid-session — see
    /// [`Engine::ensure_output`]. Where both locks are held, the order is
    /// state first, output second, always.
    output: Mutex<OutputWatch>,
    /// Device news waiting for whoever drives this engine (the TUI's
    /// audio worker, serve's loop). Bounded by [`Engine::push_notice`];
    /// drained by [`Engine::take_device_notices`].
    notices: Mutex<Vec<DeviceNotice>>,
    /// The threads of opens given up for a newer command, and when each
    /// was given up — see [`MAX_ABANDONED`]. Only the threads: the answers
    /// were let go with their receivers ([`Opener::give_up`]), and drop on
    /// those threads as they finish. Pruned once finished, or once older
    /// than [`START_TIMEOUT`], the most any open is waited.
    abandoned: Mutex<Vec<(Instant, std::thread::JoinHandle<()>)>>,
    /// Calls blocked on the device callback right now: a seek's try_seek,
    /// made with the state lock released (audit #48). The stream is never
    /// suspended under one — its answer would never come. Both drivers
    /// tick on the thread that runs their commands, so a wait and a tick
    /// never overlap there; the count is what keeps that true for any
    /// driver that ticks from elsewhere.
    callback_waits: AtomicUsize,
}

/// One call waiting on the device callback, counted for as long as it
/// waits — see [`Engine::callback_waits`].
struct CallbackWait<'a>(&'a AtomicUsize);

impl<'a> CallbackWait<'a> {
    fn new(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        CallbackWait(count)
    }
}

impl Drop for CallbackWait<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Engine {
    pub fn new() -> Result<Self, EngineError> {
        let device = output::open().map_err(EngineError::NoDevice)?;
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
            prepare_plain: false,
            appended: None,
            blend_skips: false,
            pause_fade: false,
            pausing: None,
            orphaned_tail: false,
            gate_noted: None,
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

        Ok(Engine {
            state,
            output: Mutex::new(OutputWatch {
                out: device,
                active_at: Instant::now(),
                polled: Instant::now(),
                failed_at: None,
                outage_told: false,
            }),
            notices: Mutex::new(Vec::new()),
            abandoned: Mutex::new(Vec::new()),
            callback_waits: AtomicUsize::new(0),
        })
    }

    /// Before a direct open starts: wait, for as long as the play is still
    /// wanted, until fewer than [`MAX_ABANDONED`] given-up opens are still
    /// running.
    fn make_room(&self, superseded: &mut dyn FnMut() -> bool) -> Result<(), OpenError> {
        loop {
            {
                let mut gone = self.abandoned.lock().unwrap();
                gone.retain(|(since, thread)| {
                    since.elapsed() < START_TIMEOUT && !thread.is_finished()
                });
                if gone.len() < MAX_ABANDONED {
                    return Ok(());
                }
            }
            std::thread::sleep(SUPERSEDE_POLL);
            if superseded() {
                return Err(OpenError::Superseded(None));
            }
        }
    }

    /// Wake the device stream for a call about to lean on a Player, and
    /// restart the idle clock (performance audit #75). rodio performs
    /// seeks, stops and skips inside the device callback, and a new sink
    /// only sounds once the callback pulls it: against a suspended stream
    /// a seek would wait forever and a play would start silent. Takes only
    /// the output lock, so it may run under the state lock or before it.
    fn wake(&self) {
        let mut w = self.output.lock().unwrap();
        w.active_at = Instant::now();
        w.out.wake();
    }

    /// [`Engine::wake`], then [`Engine::ensure_output`]: the entry every
    /// mutator about to lean on the sink makes. The wake goes first so a
    /// stream that will not restart — its device gone while it slept — is
    /// rebuilt before the caller touches it.
    fn wake_output(&self) {
        self.wake();
        self.ensure_output();
    }

    /// The tick's half of the device's sleep (performance audit #75), the
    /// part made under the state lock. Whenever the engine is not at rest
    /// the stream is awake; the mutators wake it themselves, and this is
    /// the net under any path that starts sound without asking. At rest,
    /// the answer is whether the rest is a stop (a pause otherwise), for
    /// [`Engine::rest_output_after`] to act on once the state lock is let
    /// go.
    fn rest_output(&self, s: &State) -> Option<bool> {
        if !s.at_rest() {
            let mut w = self.output.lock().unwrap();
            w.active_at = Instant::now();
            if !w.out.is_awake() {
                etrace!("output woken by the tick");
                w.out.wake();
            }
            return None;
        }
        Some(s.stopped)
    }

    /// Once the engine has been at rest long enough — [`IDLE_STOPPED`] or
    /// [`IDLE_PAUSED`] — the stream is suspended: the callback stops, and
    /// the operating system stops keeping the audio hardware — and on
    /// macOS the machine — awake for a player nobody is listening to.
    /// Nothing is lost by the sleep: the rodio chain freezes where it
    /// stands, position and decoder included. Called with the state lock
    /// let go: a suspend waits on the callback in flight, and nothing that
    /// waits on the callback may hold the lock every control needs (audit
    /// #48; [`output::Output::suspend`] also refuses a callback stuck
    /// mid-pull). A command landing in between restarts the idle clock
    /// through its own wake, so the verdict read under the lock cannot put
    /// a new play to sleep.
    fn rest_output_after(&self, stopped: bool) {
        let after = if stopped { IDLE_STOPPED } else { IDLE_PAUSED };
        let mut w = self.output.lock().unwrap();
        if !w.out.is_awake()
            || w.active_at.elapsed() < after
            || self.callback_waits.load(Ordering::Acquire) > 0
        {
            return;
        }
        if w.out.suspend() {
            etrace!(
                "output suspended ({} for {:.1}s)",
                if stopped { "stopped" } else { "paused" },
                after.as_secs_f64()
            );
        }
    }

    /// Keep the output stream on the device the system says it should be
    /// on. Two tripwires (see [`output`]): the stream's error callback —
    /// the device died under us — and a poll of the system default — a
    /// new device became the default while the old stream plays on
    /// unaware. Either way the cure is the same rebuild. Called from the
    /// tick and, through [`Engine::wake_output`], from every mutator about
    /// to lean on the sink; the healthy path costs an atomic load, plus one
    /// identity poll a second.
    fn ensure_output(&self) {
        let why = {
            let mut w = self.output.lock().unwrap();
            if let Some(at) = w.failed_at {
                if at.elapsed() < REBUILD_RETRY {
                    return;
                }
            }
            if w.out.is_dead() {
                "the device was lost"
            } else {
                if w.polled.elapsed() < DEVICE_POLL {
                    return;
                }
                w.polled = Instant::now();
                if !w.out.default_moved() {
                    return;
                }
                "the default output changed"
            }
        };
        etrace!("output rebuild: {why}");
        self.rebuild_output();
    }

    /// Move playback onto a freshly-opened output, keeping as much of the
    /// moment as can be kept: the track, its position, its pause state,
    /// the volume. The sinks themselves are past saving — every Player
    /// feeds the old stream's mixer, whose consuming half died with the
    /// stream — so the current source is reopened and seeked back to
    /// where it stood.
    fn rebuild_output(&self) {
        // The replacement first, before anything is torn down: while
        // nothing will open, the engine keeps the old output and its
        // state exactly as they are and knocks again in REBUILD_RETRY.
        let fresh = match output::open() {
            Ok(out) => out,
            Err(e) => {
                let mut w = self.output.lock().unwrap();
                w.failed_at = Some(Instant::now());
                let first = !w.outage_told;
                w.outage_told = true;
                drop(w);
                if first {
                    etrace!("output rebuild failed: {e}");
                    self.push_notice(
                        "audio device lost — playback resumes when one comes back".to_string(),
                        true,
                    );
                }
                return;
            }
        };
        let name = fresh.name().to_string();

        let mut s = self.state.lock().unwrap();
        // A gapless boundary that crossed before the device died decides
        // WHICH track the rebuild resumes; promote it before the snapshot.
        s.promote_if_crossed();
        let position = s.sink.get_pos();
        let was_paused = s.sink.is_paused() || s.pausing.is_some();
        let resume = (!s.stopped && !s.current_file.is_empty()).then(|| s.current_file.clone());

        // Everything attached to the old mixer is unrecoverable. Blends
        // and breaths mid-drain simply end; a source appended for a
        // gapless seam died inside the old sink, so its slot clears and
        // the transition machinery re-prepares against the new one. A
        // prepared-but-unattached next (Opening/Ready) survives on
        // purpose: its decoder belongs to no sink yet.
        for out in s.outgoing.drain(..) {
            out.sink.stop();
        }
        s.appended = None;
        s.orphaned_tail = false;
        s.pausing = None;
        s.sink.stop();

        // The swap. The old output drops here — after its players
        // stopped, the same order Engine's own drop keeps.
        let was_outage = {
            let mut w = self.output.lock().unwrap();
            let old = std::mem::replace(&mut w.out, fresh);
            // The fresh stream opens running; the idle clock starts over
            // with it, so a paused resume still gets its full grace.
            w.active_at = Instant::now();
            w.polled = Instant::now();
            w.failed_at = None;
            let was = w.outage_told;
            w.outage_told = false;
            drop(old);
            was
        };

        // A fresh sink either way, so the engine never holds a sink wired
        // to a mixer that no longer plays.
        let sink = Player::connect_new(self.output.lock().unwrap().mixer());
        sink.set_volume(s.volume);
        let restore = match resume {
            None => {
                s.sink = Arc::new(sink);
                s.fade = fade::FadeHandle::new(1.0);
                s.tap_live = Arc::new(AtomicBool::new(true));
                None
            }
            Some(file) => {
                // The queue entry when it still names the playing file
                // (serve keeps a real queue; the TUI mirrors one entry),
                // else a synthesized one carrying the duration we know.
                let entry = s
                    .q
                    .queue
                    .get(s.q.index)
                    .filter(|e| e.path == file)
                    .cloned()
                    .unwrap_or_else(|| QueueEntry {
                        path: file.clone(),
                        duration_hint: (s.duration > 0.0).then_some(s.duration),
                    });
                match open_entry_bounded(&entry) {
                    Ok(prepared) => {
                        // Zero gain: the seek back to position lands under
                        // silence and the gain snaps up after it — the
                        // seek dip's trick, stretched over a device swap.
                        let (fade, live) = attach(&sink, prepared.opened, s.tap.clone(), 0.0);
                        s.fade = fade;
                        s.tap_live = live;
                        s.duration = prepared.duration;
                        s.sink = Arc::new(sink);
                        Some((s.sink.clone(), s.fade.clone()))
                    }
                    Err(e) => {
                        // The source would not reopen — gone with the wifi,
                        // say. An empty sink under a live current_file is
                        // exactly what a track running out looks like, and
                        // the ordinary advance machinery takes it from here.
                        etrace!("rebuild could not reopen {}: {e}", http::redact_source(&file));
                        s.sink = Arc::new(sink);
                        s.fade = fade::FadeHandle::new(1.0);
                        s.tap_live = Arc::new(AtomicBool::new(true));
                        None
                    }
                }
            }
        };
        drop(s);

        // The seek waits on the new device's callback reaching the data,
        // and past the spooled range that is the network's schedule — the
        // state lock must not wait with it (the lesson of audit #48).
        if let Some((sink, fade)) = restore {
            if position > Duration::from_millis(250) {
                let _waiting = CallbackWait::new(&self.callback_waits);
                let sought = sink.try_seek(position);
                etrace!(
                    "rebuild seek to {:.2}: {}",
                    position.as_secs_f64(),
                    if sought.is_ok() { "ok" } else { "FAILED" }
                );
            }
            if was_paused {
                sink.pause();
            }
            // Same resurrection guard as Engine::seek: only the handle
            // that is still current gets its gain back.
            let s = self.state.lock().unwrap();
            if Arc::ptr_eq(&s.fade, &fade) && !s.stopped {
                fade.snap(1.0);
            }
        }

        etrace!("output rebuilt on {name}");
        let text = if was_outage {
            format!("audio is back on {name}")
        } else {
            format!("audio moved to {name}")
        };
        self.push_notice(text, false);
    }

    /// Queue a line of device news for the driver to surface. Bounded:
    /// serve drains on its loop and the TUI worker every tick, but
    /// nothing REQUIRES a driver to drain, and a player left alone for a
    /// weekend must not grow a vector.
    fn push_notice(&self, text: String, lost: bool) {
        let mut notes = self.notices.lock().unwrap();
        if notes.len() >= 8 {
            notes.remove(0);
        }
        notes.push(DeviceNotice { text, lost });
    }

    /// Device news since the last call, oldest first.
    pub fn take_device_notices(&self) -> Vec<DeviceNotice> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }

    /// Pretend the device died under the stream, exactly as the error
    /// callback would report it — the recovery path from here on is the
    /// one a real unplug takes.
    #[cfg(test)]
    fn pretend_device_lost(&self) {
        self.output.lock().unwrap().out.pretend_dead();
    }

    /// Send a copy of everything played from here on to `tap`. Takes effect
    /// on the next source, since the one already in the sink is past reach.
    pub fn attach_tap(&self, tap: Arc<tap::AudioTap>) {
        self.state.lock().unwrap().tap = Some(tap);
    }

    /// Clear the queue, add one source (path or URL), play it.
    pub fn play_source(&self, source: String, duration_hint: Option<f64>) -> Result<(), EngineError> {
        self.play_source_unless(source, duration_hint, &mut || false)
    }

    /// [`Engine::play_source`] for a driver that can tell when the play has
    /// been overtaken — the TUI's audio thread, whose channel may hold a
    /// newer play or a stop by the time a slow open finishes. `superseded`
    /// is asked while the open waits (see [`await_open`]); on a yes the
    /// open is abandoned and the answer is [`EngineError::Superseded`], the
    /// playing sink untouched, exactly as a failed open leaves it.
    pub fn play_source_unless(
        &self,
        source: String,
        duration_hint: Option<f64>,
        superseded: &mut dyn FnMut() -> bool,
    ) -> Result<(), EngineError> {
        etrace!("play {} hint={:?}", http::redact_source(&source), duration_hint);
        // Before the state lock, here and in every mutator below: the
        // rebuild takes that lock itself, and this Mutex does not forgive
        // a second lock from the same thread.
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        // Boundary first, staging second — every mutator's discipline now:
        // promotion writes the index and queue, and must never overwrite
        // what this caller is about to decide (fix-round review).
        s.promote_if_crossed();
        // Before the announcement goes: a pick of the very track it
        // announced keeps the open already made for it.
        let ahead = s.take_ahead(&source);
        let redacted = http::redact_source(&source);
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
        // Opened and decoded BEFORE the running sink is touched, with the
        // lock held — start_current's order — so a bad source, or one given
        // up on, leaves current playback as it was.
        let opened = match ahead {
            Some(Ahead::Ready(prepared)) => {
                etrace!("play takes over the prepared {redacted}");
                Ok(prepared)
            }
            Some(Ahead::Opening(opener)) => {
                etrace!("play takes over the open of {redacted}");
                await_open(opener, superseded)
            }
            None => {
                let entry = s.q.queue[0].clone();
                // Only a network open is ever left running; a local one is
                // over in microseconds.
                let room =
                    if http::is_http_url(&entry.path) { self.make_room(superseded) } else { Ok(()) };
                room.and_then(|()| open_entry_unless(&entry, superseded))
            }
        };
        match opened {
            Ok(mut prepared) => {
                // The play's own hint, like a fresh open would take it.
                if let Some(hint) = duration_hint {
                    prepared.duration = hint;
                }
                s.install(self.output.lock().unwrap().mixer(), prepared);
                Ok(())
            }
            Err(OpenError::Failed(e)) => Err(EngineError::Unplayable(e)),
            Err(OpenError::Superseded(opener)) => {
                etrace!("play {redacted} given up: a newer command came");
                if let Some(opener) = opener {
                    self.abandoned.lock().unwrap().push((Instant::now(), opener.give_up()));
                }
                Err(EngineError::Superseded)
            }
        }
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
            etrace!("announce refused: names the playing source");
            return;
        }
        etrace!("announce {} hint={:?}", http::redact_source(&source), duration_hint);
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
        etrace!("announcement withdrawn");
        let mut s = self.state.lock().unwrap();
        s.pending_next = None;
        s.invalidate_next();
    }

    /// Pause everything audible — during a blend that is two sinks, and
    /// pausing only one would leave the other playing under the silence.
    pub fn pause(&self) {
        let mut s = self.state.lock().unwrap();
        if s.sink.is_paused() || s.pausing.is_some() {
            return;
        }
        // The soft pause (C6, opt-in): ramp down first, land the pause when
        // the ramp does — the tick performs the landing, since nothing here
        // may sleep. Without the option, or with nothing audible to fade,
        // the pause lands now, exactly as it always has.
        if s.pause_fade && !s.stopped && !s.sink.empty() {
            s.fade.ramp_to(0.0, PAUSE_FADE);
            s.pausing = Some(Instant::now() + PAUSE_FADE + Duration::from_millis(400));
            return;
        }
        s.sink.pause();
        for out in &s.outgoing {
            out.sink.pause();
        }
    }

    /// Land a soft pause whose ramp has finished (or whose net came due).
    /// Runs from the tick; the fade froze at silence, so the pause itself
    /// is inaudible wherever in the callback cycle it lands.
    fn land_pause(s: &mut State) {
        let due = match s.pausing {
            Some(deadline) => s.fade.position() <= 0.0 || Instant::now() >= deadline,
            None => return,
        };
        if due {
            s.pausing = None;
            s.sink.pause();
            for out in &s.outgoing {
                out.sink.pause();
            }
        }
    }

    pub fn resume(&self) {
        // A pause long enough put the device to sleep; the resume is what
        // wakes it (audit #75), and a stream that will not restart is
        // rebuilt — paused, where it stood — before the resume lands.
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        // A soft pause still mid-ramp: the resume overtakes it — cancel the
        // landing and ramp straight back up from wherever the fade stands.
        let overtaking = s.pausing.take().is_some();
        if (overtaking || s.sink.is_paused()) && s.pause_fade && !s.stopped {
            s.fade.ramp_to(1.0, PAUSE_FADE);
        }
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
        // Awake so the stop can finish: the breath, and the stopped
        // sink's source leaving the mixer, both happen in the callback.
        // No rebuild for a dead device — there is nothing left to keep.
        self.wake();
        self.state.lock().unwrap().stop_softly();
    }

    pub fn seek(&self, position: f64) -> Result<(), EngineError> {
        let mut target = seek_target(position)?;
        // Counted from before the wake to the end of the wait: the tick
        // never suspends the stream under a seek (audit #75).
        let _waiting = CallbackWait::new(&self.callback_waits);
        // A seek against a dead device would never return: try_seek waits
        // on a feedback the dead callback can never send — and a sleeping
        // one is the same wait, so the stream wakes first. Rebuild first —
        // and when no device would open at all, refuse rather than wedge
        // this thread until one comes back.
        self.wake_output();
        if self.output.lock().unwrap().out.is_dead() {
            return Err(EngineError::NoDevice("no output device".to_string()));
        }
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
            s.promote_if_crossed();
            // Seeking mid-blend keeps the track being seeked; hearing the
            // old one still draining behind a jump is disorienting.
            s.snap_out_of_blend();
            // A seek is fresh runway: a next that failed under the old
            // geometry deserves its retry under the new one. The Failed
            // latch exists to stop the *tick* re-walking a doomed open
            // every 120ms inside the window; a user move is a different
            // occasion, and without this an open that failed once late in
            // the track stayed failed to the end and the seam went out
            // as a cut.
            if matches!(s.next, NextTrack::Failed { .. }) {
                etrace!("seek resets a failed open");
                s.next = NextTrack::Idle;
            }
            // Forward seeks stop short of a configured transition's runway
            // — see [`seek_ceiling`]. Never backward from where playback
            // already is: a clamp that yanked the cursor back would turn
            // a keystroke near the end into a rewind. And only when
            // something is actually lined up to follow — on the queue's
            // last track with nothing announced there is no transition to
            // starve, and holding the listener away from the end of the
            // final track would be the ceiling outliving its reason
            // (pre-merge review). pick_next is pure, so asking it here
            // commits nothing.
            if let Some(ceiling) = seek_ceiling(s.duration, s.crossfade, s.gapless) {
                let follows = s.pending_next.is_some()
                    || next_candidate(&s.q, !(s.crossfade > 0.0)).is_some();
                let current = s.sink.get_pos().as_secs_f64();
                let asked = target.as_secs_f64();
                if follows && asked > current && asked > ceiling {
                    target = Duration::from_secs_f64(ceiling.max(current));
                    etrace!("seek {asked:.1} clamped to {:.1} (transition runway)",
                        target.as_secs_f64());
                }
            }
            (s.sink.clone(), s.fade.clone())
        };
        // The dip (C4): down before the jump, up after it. The down-ramp
        // has a device callback or two to land while try_seek waits on the
        // seek being performed; the up-ramp then rises from the new spot.
        fade.ramp_to(0.0, SEEK_DIP_DOWN);
        let sought = sink.try_seek(target).map_err(|e| EngineError::Seek(e.to_string()));
        etrace!("seek to {:.2}: {} (pos {:.2})", target.as_secs_f64(),
            if sought.is_ok() { "ok" } else { "FAILED" }, sink.get_pos().as_secs_f64());
        // try_seek can block for as long as the network makes it, and a
        // stop or skip can retire this sink in the meantime — its fade then
        // belongs to a drainer ramping to silence, and the up-ramp would
        // resurrect a track the engine reports as gone, at full volume,
        // until the deadline clicked it off (C4 review). The ptr_eq alone
        // was not enough: a stop parks the sink WITHOUT replacing s.fade,
        // so the handle still compared current (fix-round review) — hence
        // the stopped flag and the drainer-membership check.
        {
            let s = self.state.lock().unwrap();
            let still_current = Arc::ptr_eq(&s.fade, &fade)
                && !s.stopped
                && !s.outgoing.iter().any(|out| Arc::ptr_eq(&out.fade, &fade));
            if still_current {
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

    /// Prepare the next track ahead of a plain cut (performance audit
    /// #77): no blend, no append, the same hard cut — started from a
    /// decoder opened ahead of time instead of one opened at the boundary
    /// with the lock held. A configured transition outranks it.
    pub fn set_prepare_plain(&self, on: bool) {
        self.state.lock().unwrap().prepare_plain = on;
    }

    /// Manual skips blend for a second instead of breathing (C6).
    pub fn set_blend_skips(&self, on: bool) {
        self.state.lock().unwrap().blend_skips = on;
    }

    /// Pause and resume ride a short ramp instead of landing mid-wave (C6).
    pub fn set_pause_fade(&self, on: bool) {
        self.state.lock().unwrap().pause_fade = on;
    }

    /// Whether nothing can change until a command arrives: stopped, or a
    /// landed pause, with no breath draining, no ramp owed, no open in
    /// flight. A driver may tick lazily then — once per [`DEVICE_POLL`],
    /// which is all the device watch asks — where it otherwise ticks every
    /// ~100 ms to catch the end of a track, a blend's steps and position
    /// (performance audit #81).
    pub fn settled(&self) -> bool {
        self.state.lock().unwrap().at_rest()
    }

    /// How long a driver that otherwise ticks every `base` may wait before
    /// the next tick — see [`next_tick`]: a second while settled, and near
    /// a track's end just long enough to catch it running out.
    pub fn tick_wait(&self, base: Duration) -> Duration {
        let s = self.state.lock().unwrap();
        next_tick(s.at_rest(), s.until_end(), base)
    }

    pub fn status(&self) -> Status {
        let mut s = self.state.lock().unwrap();
        // A crossed gapless boundary is promoted before answering, not a
        // tick later: without this, /status reported the old file carrying
        // the new track's position for up to a poll interval (C4 review).
        s.promote_if_crossed();
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
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        s.promote_if_crossed();
        match pick_next(&s.q, true) {
            Some(idx) => {
                s.q.index = idx;
                let started = s.start_current(self.output.lock().unwrap().mixer());
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
        // The restart branch below seeks, and a seek against a dead or a
        // sleeping device never returns — same reasoning as Engine::seek.
        let _waiting = CallbackWait::new(&self.callback_waits);
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        s.promote_if_crossed();
        if s.q.index == 0 {
            if s.q.loop_mode == LoopMode::All && !s.q.queue.is_empty() {
                s.q.index = s.q.queue.len() - 1;
                let started = s.start_current(self.output.lock().unwrap().mixer());
                if started.is_err() {
                    // Same as next_manual: the index moved, the start did
                    // not, and the committed pick answers for neither.
                    s.invalidate_next();
                }
                started
            } else {
                // The restart is a seek like any other — same discipline as
                // [`Engine::seek`], even though the start of the track has
                // almost always downloaded by now. Same blend policy, the
                // same dip — and the same refusal while no device exists,
                // since try_seek against a dead sink waits forever.
                if self.output.lock().unwrap().out.is_dead() {
                    return Err(EngineError::NoDevice("no output device".to_string()));
                }
                s.snap_out_of_blend();
                let sink = s.sink.clone();
                let fade = s.fade.clone();
                drop(s);
                fade.ramp_to(0.0, SEEK_DIP_DOWN);
                let _ = sink.try_seek(Duration::ZERO);
                // Same resurrection guard as Engine::seek, all three
                // clauses: only the still-current handle gets the up-ramp.
                let s = self.state.lock().unwrap();
                let still_current = Arc::ptr_eq(&s.fade, &fade)
                    && !s.stopped
                    && !s.outgoing.iter().any(|out| Arc::ptr_eq(&out.fade, &fade));
                if still_current {
                    fade.ramp_to(1.0, SEEK_DIP_UP);
                }
                Ok(())
            }
        } else {
            s.q.index -= 1;
            let started = s.start_current(self.output.lock().unwrap().mixer());
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
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        s.promote_if_crossed();
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
            let _ = s.start_current(self.output.lock().unwrap().mixer());
        }
    }

    pub fn queue_add_many(&self, files: Vec<String>) {
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        s.promote_if_crossed();
        let was_empty = s.q.queue.is_empty();
        s.q.queue.extend(files.into_iter().map(QueueEntry::new));
        s.invalidate_next();
        if was_empty && (s.stopped || s.sink.empty()) && !s.q.queue.is_empty() {
            s.q.index = 0;
            let _ = s.start_current(self.output.lock().unwrap().mixer());
        }
    }

    pub fn queue_play_index(&self, index: usize) -> Result<(), EngineError> {
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        s.promote_if_crossed();
        if index >= s.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        s.q.index = index;
        let started = s.start_current(self.output.lock().unwrap().mixer());
        if started.is_err() {
            s.invalidate_next();
        }
        started
    }

    pub fn queue_remove(&self, index: usize) -> Result<(), EngineError> {
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        s.promote_if_crossed();
        if index >= s.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        // Deleting the very row that sits appended is the one queue edit
        // the irrevocable-append race CAN heed: the source is still queued
        // (a crossed boundary was promoted above), so the snap is silent
        // and the deleted track is genuinely never heard. Without this the
        // removal was cosmetic — the audio played anyway.
        if s.appended.as_ref().and_then(|a| a.index) == Some(index) {
            if let Some(appended) = s.appended.take() {
                appended.fade.snap(0.0);
                appended.tap_live.store(false, Ordering::Relaxed);
                // The silent remnant is still queued in the live sink, and
                // must be skipped at its boundary rather than played to
                // its silent completion (verify round, critic).
                s.orphaned_tail = true;
            }
        }
        let outcome = apply_remove(&mut s.q, index);
        // An appended commitment's slot moves with the queue like the
        // current index does — for EVERY outcome, not just the arms that
        // felt like it: the failed-restart path left it unshifted before
        // (fix-round review).
        let len = s.q.queue.len();
        if let Some(appended) = &mut s.appended {
            if let Some(committed) = appended.index {
                let (shifted, _) = crate::advance::shift_current(len, committed, index);
                appended.index = Some(shifted);
            }
        }
        match outcome {
            RemoveOutcome::EmptiedQueue => {
                s.stop_softly();
            }
            RemoveOutcome::RemovedCurrent => {
                s.invalidate_next();
                // Audit fix #4: only restart playback if we were actually
                // playing; the original started audio as a side effect of
                // removing a track while stopped.
                if !s.stopped {
                    let _ = s.start_current(self.output.lock().unwrap().mixer());
                }
            }
            RemoveOutcome::RemovedBeforeCurrent | RemoveOutcome::RemovedAfterCurrent => {
                s.invalidate_next();
            }
        }
        Ok(())
    }

    pub fn queue_clear(&self) {
        // Awake for the stop's sake, as in Engine::stop.
        self.wake();
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
        // The periodic leg of the device watch: commands guard themselves
        // on entry, and this covers the stretches where nobody is pressing
        // anything — the exact stretch an unplug usually lands in.
        self.ensure_output();
        let mut s = self.state.lock().unwrap();
        Self::land_pause(&mut s);
        s.retire_outgoing();
        self.crossfade_step(&mut s);
        // Awake unless at rest — ahead of the advance below, which is never
        // at rest — and asleep once the rest has lasted (audit #75).
        let resting = self.rest_output(&s);
        if !(s.sink.empty() && !s.stopped && !s.q.queue.is_empty()) {
            // At rest always leaves here — stopped, or paused over a track
            // still in the sink — and the sleep waits for the lock to go.
            drop(s);
            if let Some(stopped) = resting {
                self.rest_output_after(stopped);
            }
            return;
        }
        etrace!("track ran out (next={}, announced={})",
            s.next.name(), s.pending_next.is_some());
        // A prepared next committed its row when the prepare began, and
        // under shuffle that is where the dice were rolled: rolling them
        // again here missed the prepared decoder (N-2)/(N-1) of the time,
        // threw it away and opened another pick with the lock held
        // (performance audit #77). Any queue edit since would have
        // invalidated it, so a commitment still standing is to this queue.
        let committed = match &s.next {
            NextTrack::Ready { prepared, index: Some(index) }
                if s.q.queue.get(*index).is_some_and(|e| e.path == prepared.path) =>
            {
                Some(*index)
            }
            _ => None,
        };
        match committed.or_else(|| pick_next(&s.q, false)) {
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
                    s.install(self.output.lock().unwrap().mixer(), prepared);
                    return;
                }
                if s.start_current(self.output.lock().unwrap().mixer()).is_ok() {
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
        s.promote_if_crossed();

        let blending = s.crossfade > 0.0;
        if !blending && !s.gapless && !s.prepare_plain {
            // Toggled off with something in flight: forget what is not yet
            // committed. An overlap already sounding retires on its own,
            // and an appended source promotes above.
            if !matches!(s.next, NextTrack::Idle) {
                etrace!("transitions off; forgetting the in-flight next");
                s.invalidate_next();
            }
            return;
        }
        if s.stopped || s.q.queue.is_empty() {
            return;
        }

        // Collect the opener's answer if one has arrived.
        s.next = match std::mem::replace(&mut s.next, NextTrack::Idle) {
            NextTrack::Opening { index, opener } => match opener.rx.try_recv() {
                Ok(Ok(prepared)) => {
                    etrace!("prepared {} ({:.1}s)",
                        http::redact_source(&prepared.path), prepared.duration);
                    NextTrack::Ready { prepared, index }
                }
                // open_entry already told stderr what went wrong.
                Ok(Err(e)) => {
                    etrace!("open FAILED: {e}");
                    NextTrack::Failed { at: Instant::now() }
                }
                // The opener panicked — symphonia does, on malformed files
                // (audit #32) — which reads the same as a failed open.
                Err(mpsc::TryRecvError::Disconnected) => {
                    etrace!("open FAILED: opener panicked");
                    NextTrack::Failed { at: Instant::now() }
                }
                Err(mpsc::TryRecvError::Empty) => NextTrack::Opening { index, opener },
            },
            other => other,
        };

        // A seam prepared for gapless must never survive into a blend: a
        // crossfade toggled on mid-lap leaves a self-AIMED pending (and
        // possibly a self-opened Ready) behind, and blends do not
        // self-blend. Self-aimed is the operative word: a TUI announcement
        // (index None) naming the playing source, or a commitment to the
        // playing ROW itself. A different row that happens to hold the
        // same file is a lawful blend — refusing it by path alone respawned
        // the same open every tick for the whole window (fix-round review).
        // The app withdraws its side in the same keystroke; this is the
        // engine's own refusal, for whatever ordering the messages arrive.
        if blending {
            let self_pending =
                s.pending_next.as_ref().is_some_and(|e| e.path == s.current_file);
            let self_ready = matches!(&s.next,
                NextTrack::Ready { prepared, index }
                    if prepared.path == s.current_file
                        && index.is_none_or(|committed| committed == s.q.index));
            if self_pending {
                s.pending_next = None;
            }
            if self_pending || self_ready {
                etrace!("self-aimed next dropped (a blend never self-blends)");
                s.invalidate_next();
            }
        }

        // Paused means frozen: the position cannot reach the window, and a
        // handover under a paused track would start sounding on its own.
        if s.sink.is_paused() {
            if matches!(s.next, NextTrack::Ready { .. }) {
                s.note_gate("blend waiting: paused");
            }
            return;
        }
        // Transitions are strictly one at a time: a draining blend, an
        // already-appended next, or a not-yet-skipped orphan is the gate
        // for anything further — appending behind an orphan would put
        // three sources in one sink and confuse every boundary check.
        if !s.outgoing.is_empty() || s.appended.is_some() || s.orphaned_tail {
            if matches!(s.next, NextTrack::Ready { .. }) {
                s.note_gate("blend waiting: a transition is still draining");
            }
            return;
        }
        // Past every gate: the next one that closes is news again.
        s.gate_noted = None;
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

        // A transient failure earns another try on a clock, not never: the
        // one-way latch inside the window meant a single starved open late
        // in a track silenced the seam for good, and the listener heard
        // "crossfade off". Rate-limiting keeps what the latch was for — a
        // dead URL is still not walked into every tick. The gate is only
        // "could a retry still be HEARD": any runway past the open itself,
        // because blend_window caps at what remains and a shortened blend
        // beats a cut. It was `fade + OPEN_RUNWAY` at first, which with a
        // long fade excluded every retry there was — a 30s fade opens its
        // window at 42s out, the first open fails at 32s out, and 32 is
        // never greater than 32 (the listening-session trace).
        if let NextTrack::Failed { at } = s.next {
            if at.elapsed() >= FAILED_RETRY && remaining > OPEN_RUNWAY {
                etrace!("failed open rests over; retrying ({remaining:.1}s remaining)");
                s.next = NextTrack::Idle;
            }
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
                etrace!("preparing {} ({remaining:.1}s remaining, fade {fade:.1}s)",
                    http::redact_source(&entry.path));
                s.next = spawn_prepare(entry, index);
            }
        }
        if matches!(s.next, NextTrack::Ready { .. }) {
            if blending && remaining <= fade {
                etrace!("handover at {remaining:.2}s remaining");
                self.handover(s);
            } else if !blending && s.gapless && remaining <= APPEND_LEAD {
                etrace!("gapless append at {remaining:.2}s remaining");
                append_gapless(s);
            }
            // A plain prepare waits in Ready for the source to run out;
            // the advance at the boundary installs it.
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

        let sink = Player::connect_new(self.output.lock().unwrap().mixer());
        sink.set_volume(s.volume);
        s.attach_fresh(&sink, prepared.opened, 0.0);
        s.fade.ramp_to(1.0, fade_dur);
        s.sink = Arc::new(sink);
        s.orphaned_tail = false;
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

    /// Whether the device stream is running, or asleep (audit #75).
    #[cfg(test)]
    fn output_awake(&self) -> bool {
        self.output.lock().unwrap().out.is_awake()
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
    fn a_stream_that_stalls_mid_probe_fails_the_start_instead_of_wedging() {
        // Headers arrive, a few bytes arrive, then nothing — forever. The
        // shape a wifi flap leaves behind, and the one `http::open`'s own
        // timeout cannot catch: the open succeeds, and it is the decode
        // probe after it that used to wait out stream-download's eternal
        // reconnect patience on the audio thread, under the state lock,
        // with a "starting" on screen that could never end.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 1000000\r\n\
                          content-type: audio/mpeg\r\n\r\nID3\x04\x00\x00\x00\x00\x00\x00",
                    );
                    let _ = stream.flush();
                    // And now: silence, for longer than anyone will wait.
                    std::thread::sleep(Duration::from_secs(60));
                });
            }
        });

        let entry =
            QueueEntry { path: format!("http://{addr}/stalled.mp3"), duration_hint: None };
        let started = Instant::now();
        let err = match open_entry_bounded(&entry) {
            Err(e) => e,
            Ok(_) => panic!("a stalled stream must not open"),
        };
        let waited = started.elapsed();
        assert!(err.contains("stalled while opening"), "{err}");
        // Bounded is the claim, not sharp: a busy CI box wakes late.
        assert!(waited < Duration::from_secs(10), "took {waited:?}");
    }

    /// A server that sends headers and a few bytes, then nothing, forever:
    /// an open that can only end by a deadline or by being given up.
    fn stalled_url() -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 1000000\r\n\
                          content-type: audio/mpeg\r\n\r\nID3\x04\x00\x00\x00\x00\x00\x00",
                    );
                    let _ = stream.flush();
                    std::thread::sleep(Duration::from_secs(60));
                });
            }
        });
        format!("http://{addr}/stalled.mp3")
    }

    #[test]
    fn an_open_nobody_wants_any_more_is_given_up_at_once() {
        // The same stall the deadline exists for, but a newer command
        // arrives 200 ms in: the wait ends then, not at START_TIMEOUT
        // (audit #79) — and without anyone calling it a failure.
        let entry = QueueEntry { path: stalled_url(), duration_hint: None };
        let started = Instant::now();
        let mut asked = 0;
        let outcome = open_entry_unless(&entry, &mut || {
            asked += 1;
            started.elapsed() > Duration::from_millis(200)
        });
        let waited = started.elapsed();
        assert!(matches!(outcome, Err(OpenError::Superseded(Some(_)))), "given up, not failed");
        assert!(waited < START_TIMEOUT / 2, "took {waited:?}");
        assert!(asked >= 5, "asked every poll while it waited ({asked} times)");

        // Nobody superseding: the deadline still ends it, as before.
        let started = Instant::now();
        let outcome = open_entry_unless(&entry, &mut || false);
        assert!(matches!(outcome, Err(OpenError::Failed(ref e)) if e.contains("stalled while opening")));
        assert!(started.elapsed() >= START_TIMEOUT, "the full deadline, not less");
    }

    /// A WAV that answers after `delay`, sends enough at once for an open
    /// to finish, then trickles the rest for longer than any test runs —
    /// and reports when the client hangs up.
    fn trickling_wav_server(delay: Duration) -> (String, mpsc::Receiver<Instant>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (hung_up, hang_ups) = mpsc::channel();
        // Twenty seconds of WAV: eighteen seconds of trickle, well past
        // the end of any test.
        let body = Arc::new(wav_bytes(20));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (body, hung_up) = (body.clone(), hung_up.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = [0u8; 2048];
                    let _ = stream.read(&mut request);
                    std::thread::sleep(delay);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body[..600_000]);
                    for chunk in body[600_000..].chunks(8_192) {
                        if stream.write_all(chunk).is_err() {
                            let _ = hung_up.send(Instant::now());
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                });
            }
        });
        (format!("http://{addr}/trickling.wav"), hang_ups)
    }

    #[test]
    fn a_given_up_open_takes_its_download_with_it_when_it_finishes() {
        // An open given up mid-request runs on, and what it opens must not
        // outlive it. The engine used to keep the receiver to count the
        // open by, which kept the answer too: a live reader downloading the
        // whole unwanted track beside the wanted one until something next
        // pruned the list (review of audit #79). Given up, the answer drops
        // on the open's own thread the moment it arrives.
        let (url, hang_ups) = trickling_wav_server(Duration::from_millis(100));
        let entry = QueueEntry { path: url, duration_hint: None };

        // Waited for, the same open succeeds: a real track, not a failure
        // whose connection would close on its own. Dropped here, which is
        // the hang-up the server reports first.
        assert!(open_entry_bounded(&entry).is_ok(), "the track opens");
        hang_ups.recv_timeout(Duration::from_secs(3)).expect("a dropped track hangs up");

        let asked = Instant::now();
        let outcome =
            open_entry_unless(&entry, &mut || asked.elapsed() > Duration::from_millis(20));
        let Err(OpenError::Superseded(Some(opener))) = outcome else {
            panic!("given up while its request was out");
        };
        let thread = opener.give_up();
        let until = Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() {
            assert!(Instant::now() < until, "the open never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
        let finished = Instant::now();
        // Finished, and its download went with it: the server hears the
        // hang-up, not a request for the rest of the track.
        let hung_up = hang_ups
            .recv_timeout(Duration::from_secs(3))
            .expect("the download ran on after its open was given up");
        let after = hung_up.saturating_duration_since(finished);
        assert!(after < Duration::from_secs(1), "hung up {after:?} after the open finished");
    }

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

    #[test]
    fn the_seek_ceiling_guards_exactly_the_configured_transition() {
        // Nothing configured: nothing to starve, no ceiling — seeking past
        // the end keeps its legacy skip-the-track meaning.
        assert_eq!(seek_ceiling(240.0, 0.0, false), None);
        // A blend reserves its window plus the open's runway.
        assert_eq!(seek_ceiling(240.0, 6.0, false), Some(240.0 - 6.0 - OPEN_RUNWAY));
        // Crossfade outranks gapless, the same precedence as everywhere.
        assert_eq!(seek_ceiling(240.0, 6.0, true), Some(240.0 - 6.0 - OPEN_RUNWAY));
        // Gapless alone reserves the append lead plus the runway.
        assert_eq!(seek_ceiling(240.0, 0.0, true), Some(240.0 - APPEND_LEAD - OPEN_RUNWAY));
        // The half-track cap flows through: a 10s track under a 30s blend
        // carries 5s of fade, not 30.
        assert_eq!(seek_ceiling(10.0, 30.0, false), Some(10.0 - 5.0 - OPEN_RUNWAY));
        // A track shorter than its own reserve pins the ceiling at zero
        // rather than going negative.
        assert_eq!(seek_ceiling(1.0, 30.0, false), Some(0.0));
        // Unknown duration: no end to guard.
        assert_eq!(seek_ceiling(0.0, 6.0, true), None);
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

    /// A server that sends the first `sent_first` bytes of a long WAV and
    /// holds the rest until the flag it hands back is raised — on every
    /// connection, the download watchdog's range reconnects included, so
    /// the stall lasts exactly as long as the test wants it to.
    fn held_wav_server(seconds: usize, sent_first: usize) -> (String, Arc<AtomicBool>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let release = Arc::new(AtomicBool::new(false));
        let released = release.clone();
        std::thread::spawn(move || {
            let body = Arc::new(wav_bytes(seconds));
            for stream in listener.incoming().flatten() {
                let (body, released) = (body.clone(), released.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut head = [0u8; 2048];
                    let read = stream.read(&mut head).unwrap_or(0);
                    let request = String::from_utf8_lossy(&head[..read]).to_ascii_lowercase();
                    let from = request
                        .split("range: bytes=")
                        .nth(1)
                        .and_then(|range| range.split('-').next())
                        .and_then(|at| at.trim().parse::<usize>().ok());
                    let start = from.unwrap_or(0).min(body.len());
                    let status = match from {
                        Some(_) => format!(
                            "206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                            body.len() - 1,
                            body.len()
                        ),
                        None => "200 OK".to_string(),
                    };
                    let sent = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len() - start
                    );
                    let held = sent_first.max(start);
                    let _ = stream.write_all(sent.as_bytes());
                    let _ = stream.write_all(&body[start..held]);
                    let _ = stream.flush();
                    while !released.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let _ = stream.write_all(&body[held..]);
                });
            }
        });
        (format!("http://{addr}/held.wav"), release)
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

    /// A server that answers each request with a whole WAV and counts the
    /// requests — how a test proves an open was reused, not repeated.
    fn counting_wav_server(seconds: usize) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
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
                    let _ = stream.write_all(&body);
                });
            }
        });
        (format!("http://{addr}/counted.wav"), hits)
    }

    /// A server whose first `fail_first` requests are refused with a 500 —
    /// the shape of an open starved by a busy link — and which serves the
    /// WAV honestly after that.
    fn flaky_wav_server(seconds: usize, fail_first: usize) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                let body = wav_bytes(seconds);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut head = [0u8; 2048];
                    let _ = stream.read(&mut head);
                    if n <= fail_first {
                        let _ = stream.write_all(
                            b"HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n",
                        );
                        return;
                    }
                    let sent = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(sent.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        (format!("http://{addr}/flaky.wav"), hits)
    }

    /// A duration hint running long is the usual road to a missed blend:
    /// the sink empties while the fade point is still "seconds away", and
    /// the prepared decoder must then be installed, not thrown away and
    /// fetched again with the lock held (hush round pin for the reuse).
    ///
    /// `cargo test a_missed_blend_reuses -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_missed_blend_reuses_the_prepared_decoder() {
        let local = std::env::temp_dir().join("mstream-reuse-a.wav");
        std::fs::write(&local, wav_bytes(3)).unwrap();
        let (url, hits) = counting_wav_server(3);

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        // The hint says 8s; the audio is 3. The blend window never comes.
        engine.play_source(local.to_string_lossy().into_owned(), Some(8.0)).unwrap();
        engine.queue_add(url.clone());

        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            if engine.status().file == url {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(8), "the next track never came");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "the prepared open was repeated instead of reused"
        );
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// Serve's `--prefetch` (audit #77): no blend and no append — the same
    /// hard cut — but the next track is opened ahead of the boundary and
    /// started from that decoder, where it used to be opened at the
    /// boundary with the lock and serve's whole loop held.
    ///
    /// `cargo test a_plain_prepare -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_plain_prepare_opens_ahead_and_still_cuts_at_the_end() {
        let local = std::env::temp_dir().join("mstream-plain-a.wav");
        std::fs::write(&local, wav_bytes(3)).unwrap();
        let (url, hits) = counting_wav_server(3);

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_prepare_plain(true);
        engine.queue_add_many(vec![local.to_string_lossy().into_owned(), url.clone()]);

        let started = std::time::Instant::now();
        let mut opened_ahead = false;
        let switched = loop {
            engine.advance_tick();
            let status = engine.status();
            if status.file == url {
                break started.elapsed();
            }
            // Opened ahead: the one request lands while the first track
            // still plays — and nothing blends or appends meanwhile.
            opened_ahead |= hits.load(Ordering::SeqCst) == 1 && status.playing;
            assert!(!engine.overlap_active() && !engine.appended_waiting(), "a plain cut, not a transition");
            assert!(started.elapsed() < Duration::from_secs(8), "the next track never came");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(opened_ahead, "the next track was not opened ahead of the boundary");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the boundary opened it again");
        // A cut at the end, not a blend's early switch.
        assert!(switched > Duration::from_secs_f64(2.8), "switched at {switched:?}");
        std::thread::sleep(Duration::from_millis(600));
        let later = engine.status();
        assert_eq!(later.queue_index, 1);
        assert!(later.playing && later.position > 0.3, "the prepared track sounds: {:.2}", later.position);
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// A manual pick of the announced next inside its prepare window —
    /// `n` in a track's last seconds with gapless on, the default — used
    /// to throw the prepared decoder away and fetch the track again with
    /// the audio thread waiting (audit #79). The play takes it over.
    ///
    /// `cargo test a_manual_pick_of -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_manual_pick_of_the_announced_next_takes_over_its_open() {
        let local = std::env::temp_dir().join("mstream-takeover-a.wav");
        std::fs::write(&local, wav_bytes(20)).unwrap();
        let (url, hits) = counting_wav_server(4);

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        // A hint of 5 s puts the whole track inside PREPARE_LEAD: the
        // announcement is opened at the first tick.
        engine.play_source(local.to_string_lossy().into_owned(), Some(5.0)).unwrap();
        engine.prepare_next(url.clone(), Some(4.0));
        let started = std::time::Instant::now();
        while !matches!(engine.state.lock().unwrap().next, NextTrack::Ready { .. }) {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(5), "the announcement never opened");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        engine.play_source(url.clone(), Some(4.0)).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the play fetched the track again");
        std::thread::sleep(Duration::from_millis(600));
        let status = engine.status();
        assert_eq!(status.file, url);
        assert!(status.playing && status.position > 0.3, "the taken-over track plays");
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// A play given up for a newer command leaves playback as it was — the
    /// open-then-swap order, the same as a failed open.
    ///
    /// `cargo test a_play_given_up -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_play_given_up_leaves_the_old_track_playing() {
        let local = std::env::temp_dir().join("mstream-given-up-a.wav");
        std::fs::write(&local, wav_bytes(30)).unwrap();
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(local.to_string_lossy().into_owned(), None).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let before = engine.status();

        let asked = std::time::Instant::now();
        let outcome = engine.play_source_unless(stalled_url(), None, &mut || {
            asked.elapsed() > Duration::from_millis(300)
        });
        assert!(matches!(outcome, Err(EngineError::Superseded)), "{outcome:?}");
        assert!(asked.elapsed() < Duration::from_secs(1), "gave up in {:?}", asked.elapsed());
        std::thread::sleep(Duration::from_millis(400));
        let after = engine.status();
        assert_eq!(after.file, before.file, "the old track is still the one playing");
        assert!(after.playing && after.position > before.position + 0.5);
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// Opens given up for newer commands keep running until their request
    /// ends, so skimming could stack them one per keypress; past
    /// MAX_ABANDONED the next open waits for one to finish — for as long
    /// as it is itself still wanted (audit #79).
    ///
    /// `cargo test given_up_opens -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn given_up_opens_still_running_hold_the_next_one_back() {
        let (url, hits) = counting_wav_server(3);
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        let mut running = Vec::new();
        for _ in 0..MAX_ABANDONED {
            let (tx, rx) = mpsc::channel::<()>();
            running.push(tx);
            let thread = std::thread::spawn(move || {
                let _ = rx.recv();
            });
            engine.abandoned.lock().unwrap().push((Instant::now(), thread));
        }

        // Full: the open waits, and is given up in its turn, never started.
        let asked = Instant::now();
        let outcome =
            engine.play_source_unless(url.clone(), None, &mut || asked.elapsed() > Duration::from_millis(300));
        assert!(matches!(outcome, Err(EngineError::Superseded)), "{outcome:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "no open may start while the room is full");

        // One of them ends: there is room, and the play goes ahead.
        drop(running.pop());
        engine.play_source(url.clone(), None).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(engine.status().file, url);
        engine.stop();
    }

    /// Under shuffle the prepare rolls the dice for the next row; the
    /// boundary used to roll them again, miss the prepared decoder most of
    /// the time, and open another row with the lock held (audit #77).
    ///
    /// `cargo test a_shuffled_boundary -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_shuffled_boundary_plays_the_row_that_was_prepared() {
        let rows: Vec<String> = (0..5)
            .map(|i| {
                let path = std::env::temp_dir().join(format!("mstream-shuffle-{i}.wav"));
                std::fs::write(&path, wav_bytes(2)).unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect();
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_prepare_plain(true);
        engine.set_shuffle(true);
        engine.queue_add_many(rows.clone());

        // Three boundaries: a re-roll agreeing with the prepare by chance
        // each time is (1/4)^3.
        let started = std::time::Instant::now();
        let mut committed: Option<usize> = None;
        let mut index = engine.status().queue_index;
        let mut crossed = 0;
        while crossed < 3 {
            engine.advance_tick();
            if let NextTrack::Ready { index: Some(at), .. } = &engine.state.lock().unwrap().next {
                committed = Some(*at);
            }
            let now = engine.status().queue_index;
            if now != index {
                assert_eq!(Some(now), committed, "the boundary played a row nobody prepared");
                committed = None;
                index = now;
                crossed += 1;
            }
            assert!(started.elapsed() < Duration::from_secs(15), "only {crossed} boundaries");
            std::thread::sleep(Duration::from_millis(20));
        }
        engine.stop();
        for row in rows {
            let _ = std::fs::remove_file(row);
        }
    }

    /// The other half of the listening-session bug: an open that failed
    /// once late in the track was latched failed to the end, and the seam
    /// went out as a cut. The latch is now a rate limit — the first open
    /// fails (a 500 standing in for a starved link), the retry lands, the
    /// blend fires — and it stays a limit: two hits here, not one per tick.
    ///
    /// `cargo test a_failed_open_gets -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_failed_open_gets_its_retry_and_the_blend_still_fires() {
        let local = std::env::temp_dir().join("mstream-retry-a.wav");
        std::fs::write(&local, wav_bytes(16)).unwrap();
        let (url, hits) = flaky_wav_server(3, 1);

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(2.0);
        engine.play_source(local.to_string_lossy().into_owned(), Some(16.0)).unwrap();
        engine.queue_add(url.clone());

        // Prepare opens at remaining <= 14 (2s in), eats the 500, and rests
        // FAILED_RETRY before the fresh try. The blend window arrives at
        // remaining 2; the retry must have landed Ready well before it.
        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            if engine.overlap_active() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "no blend fired; the failed open was never retried"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "one failure and one success — a rate limit, not a tick loop"
        );
        assert_eq!(engine.status().file, url);
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// The ceiling protects a transition; with nothing lined up to follow
    /// — the queue's last track, nothing announced — there is no
    /// transition, and a forward seek keeps its legacy skip-the-track
    /// meaning even with crossfade configured (pre-merge review: the
    /// clamp held listeners away from the end of the final track for no
    /// one's benefit).
    ///
    /// `cargo test the_last_track -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn the_last_track_seeks_free_of_the_ceiling() {
        let local = std::env::temp_dir().join("mstream-lasttrack-a.wav");
        std::fs::write(&local, wav_bytes(10)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(2.0);
        engine.play_source(local.to_string_lossy().into_owned(), Some(10.0)).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        // Past the end on the queue's only track: no ceiling, the track
        // ends — the pre-crossfade meaning of seeking past the end.
        engine.seek(999.0).unwrap();
        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            let s = engine.status();
            if !s.playing {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "the seek was clamped ({:.2}s) though nothing follows",
                s.position
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// The listening-session trace's exact geometry: a long fade whose
    /// first open fails INSIDE `fade + OPEN_RUNWAY` of the end. The old
    /// retry gate demanded more runway than a first failure can ever
    /// leave under a long fade (a 30s fade's window opens 42s out, the
    /// open fails 32s out, and 32 is never greater than 32) — so the seam
    /// went out as a cut. The gate now only asks whether a retry could
    /// still be heard.
    ///
    /// `cargo test a_failed_open_inside -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_failed_open_inside_the_window_still_retries() {
        let local = std::env::temp_dir().join("mstream-lategate-a.wav");
        std::fs::write(&local, wav_bytes(20)).unwrap();
        let (url, hits) = flaky_wav_server(3, 1);

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(9.0);
        engine.play_source(local.to_string_lossy().into_owned(), Some(20.0)).unwrap();
        engine.queue_add(url.clone());
        std::thread::sleep(Duration::from_millis(400));

        // The ceiling (20 − 9 − 2 = 9s) lands the seek at 11s remaining —
        // inside the old `fade + OPEN_RUNWAY` bar of 11, so the immediate
        // prepare's 500 left a Failed the old gate would have kept for
        // the rest of the track.
        engine.seek(999.0).unwrap();
        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            if engine.overlap_active() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(9),
                "no blend fired; the in-window failure was never retried"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one failure, one landed retry");
        assert_eq!(engine.status().file, url);
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// The bug a human listening session found: seeking near the end of a
    /// track killed the crossfade — the landing left less runway than the
    /// next track's open needed (and past the end, the decoder died on
    /// the keystroke). A forward seek now stops at the ceiling, and the
    /// blend fires from there.
    ///
    /// `cargo test a_seek_toward_the_end -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_seek_toward_the_end_stops_short_and_the_blend_still_fires() {
        let local = std::env::temp_dir().join("mstream-seekclamp-a.wav");
        std::fs::write(&local, wav_bytes(10)).unwrap();
        let (url, _hits) = counting_wav_server(3);

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(2.0);
        engine.play_source(local.to_string_lossy().into_owned(), Some(10.0)).unwrap();
        engine.queue_add(url.clone());
        std::thread::sleep(Duration::from_millis(400));

        // Aim far past the end. Unclamped this was the end of the track:
        // the decoder ran dry mid-keystroke and the seam went out as a
        // hard advance.
        engine.seek(999.0).unwrap();
        let s = engine.status();
        // Ceiling: 10s − (2s fade + OPEN_RUNWAY) = 6s.
        assert!(
            (s.position - 6.0).abs() < 0.8,
            "expected to land at the ceiling (~6s), got {:.2}",
            s.position
        );
        assert!(s.playing, "the clamped seek must leave the track sounding");

        // From the ceiling the open has its runway; the blend must open
        // within it (window starts at 2s remaining, i.e. ~2s from now).
        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            if engine.overlap_active() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "no blend fired after the clamped landing"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let s = engine.status();
        assert_eq!(s.file, url, "status flips to the incoming track at fade start");
        engine.stop();
        let _ = std::fs::remove_file(&local);
    }

    /// Breaths frozen by a pause outlive their wall-clock deadlines and
    /// drain after resume — the multi-drainer shape of the re-arm the
    /// single-outgoing test already pins (hush round pin).
    ///
    /// `cargo test paused_breaths -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn paused_breaths_survive_resume_and_drain() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["p1", "p2", "p3"]
            .iter()
            .map(|n| dir.join(format!("mstream-pbreath-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(3)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.queue_add_many(
            names.iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        );
        std::thread::sleep(Duration::from_millis(250));

        engine.next_manual().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        engine.next_manual().unwrap();
        assert!(engine.drainers() >= 2, "two skips, two breaths");

        engine.pause();
        std::thread::sleep(Duration::from_secs(3)); // far past both deadlines
        engine.advance_tick();
        assert!(engine.drainers() >= 2, "paused breaths are frozen, not late");

        engine.resume();
        let resumed = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(resumed.elapsed() < Duration::from_secs(2), "the breaths never drained");
            std::thread::sleep(Duration::from_millis(30));
        }
        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// A second skip inside the first skip's breath: both breathe in the
    /// fleet, and rapid fire caps at the bound instead of clicking — the
    /// fleet finally doing the job its comment always claimed (hush round).
    ///
    /// `cargo test rapid_skips -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn rapid_skips_stack_breaths_and_the_bound_holds() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = (0..5)
            .map(|n| dir.join(format!("mstream-rapid-{n}.wav")))
            .collect();
        for name in &names {
            std::fs::write(name, wav_bytes(3)).unwrap();
        }

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.queue_add_many(
            names.iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        );
        std::thread::sleep(Duration::from_millis(250));

        for _ in 0..4 {
            engine.next_manual().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let stacked = engine.drainers();
        assert!(stacked >= 2, "breaths were cut, not stacked: {stacked}");
        assert!(stacked <= 3, "the fleet bound gave way: {stacked}");

        let waited = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(waited.elapsed() < Duration::from_secs(2), "the stack never drained");
            std::thread::sleep(Duration::from_millis(30));
        }
        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// A stop mid-blend hushes the blend's long tail to a stop-length ramp
    /// — left alone it drained for the whole blend window, seconds of a
    /// track the engine reported stopped (hush round, critic finding).
    ///
    /// `cargo test a_stop_mid_blend_hushes -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_stop_mid_blend_hushes_the_long_tail() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-hush-a.wav");
        let second = dir.join("mstream-hush-b.wav");
        std::fs::write(&first, wav_bytes(9)).unwrap();
        std::fs::write(&second, wav_bytes(9)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(4.0);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);

        let started = std::time::Instant::now();
        while !engine.overlap_active() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(8), "no blend ever started");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(400)); // into the 4s blend

        engine.stop();
        // The 4s tail must be gone within a stop's breath plus slack, not
        // the remaining ~3.6s of blend window.
        let stopped = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(
                stopped.elapsed() < Duration::from_millis(1200),
                "the blend tail played on past the hush"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// The same file on two queue rows is a lawful blend, not a seam: the
    /// path-scoped refusal churned a fresh open every tick and the blend
    /// never came (hush round). Index-scoped, it blends.
    ///
    /// `cargo test duplicate_rows_blend -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn duplicate_rows_blend_like_any_other_neighbours() {
        let tiny = std::env::temp_dir().join("mstream-duprows.wav");
        std::fs::write(&tiny, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_crossfade(1.5);
        let path = tiny.to_string_lossy().into_owned();
        engine.queue_add_many(vec![path.clone(), path]);

        let started = std::time::Instant::now();
        loop {
            engine.advance_tick();
            let status = engine.status();
            if status.queue_index == 1 {
                assert!(
                    engine.overlap_active(),
                    "the duplicate advanced without its blend — refusal misfired"
                );
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(6), "no handover ever came");
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.stop();
        let _ = std::fs::remove_file(&tiny);
    }

    /// A manual jump staged while a gapless boundary sat crossed and
    /// unpromoted: the promotion happens FIRST, at the top of the mutator,
    /// so it can never overwrite the index the caller stages (hush round —
    /// the promotion buried in install() clobbered it).
    ///
    /// `cargo test a_jump_over_a_crossed -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_jump_over_a_crossed_boundary_lands_where_aimed() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["1", "2", "3"]
            .iter()
            .map(|n| dir.join(format!("mstream-jump-{n}.wav")))
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

        // Arm the append, then stop ticking and sleep past the boundary so
        // it sits crossed and unpromoted — then jump.
        let started = std::time::Instant::now();
        while !engine.appended_waiting() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(3), "nothing was ever appended");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_secs_f64(
            (3.2 - started.elapsed().as_secs_f64()).max(0.1),
        ));

        engine.queue_play_index(2).unwrap();
        let status = engine.status();
        assert_eq!(status.file, names[2].to_string_lossy(), "the jump landed where aimed");
        assert_eq!(status.queue_index, 2, "and the index is the caller's, not the boundary's");

        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// Deleting the row that sits appended is the one append the race CAN
    /// heed: still queued, it is snapped silent and genuinely never plays
    /// (hush round — before, the removal was cosmetic).
    ///
    /// `cargo test a_removed_appended_row -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_removed_appended_row_is_never_heard() {
        let dir = std::env::temp_dir();
        let names: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|n| dir.join(format!("mstream-heed-{n}.wav")))
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

        let started = std::time::Instant::now();
        while !engine.appended_waiting() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(3), "nothing was ever appended");
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.queue_remove(1).unwrap();

        // The deleted track's name must never surface — and its SILENCE
        // must not stall the queue either: the snapped remnant used to
        // play its whole length at gain zero before the successor came
        // (verify round, critic). c is due by ~3s; the stall pushed it
        // past 6.
        let mut heard_c = false;
        while started.elapsed() < Duration::from_secs(5) {
            engine.advance_tick();
            let status = engine.status();
            assert!(
                status.file != names[1].to_string_lossy(),
                "the deleted track played anyway"
            );
            if status.file == names[2].to_string_lossy() {
                heard_c = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(heard_c, "the successor never came — the orphan's silence stalled the queue");

        engine.stop();
        for name in &names {
            let _ = std::fs::remove_file(name);
        }
    }

    /// The boundary is visible through a bare status() with no ticks at
    /// all — the promote-on-read that keeps /status truthful (pin for the
    /// d5cb417 change the fix-round review found untested).
    ///
    /// `cargo test a_bare_status_poll -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_bare_status_poll_sees_the_gapless_boundary() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-bare-a.wav");
        let second = dir.join("mstream-bare-b.wav");
        std::fs::write(&first, wav_bytes(3)).unwrap();
        std::fs::write(&second, wav_bytes(3)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_gapless(true);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);
        let started = std::time::Instant::now();
        while !engine.appended_waiting() {
            engine.advance_tick();
            assert!(started.elapsed() < Duration::from_secs(3), "nothing was ever appended");
            std::thread::sleep(Duration::from_millis(50));
        }

        // No more ticks — only status polls, straight through the boundary.
        while started.elapsed() < Duration::from_secs(5) {
            let status = engine.status();
            if status.file == second.to_string_lossy() {
                assert_eq!(status.queue_index, 1, "the read promoted the whole boundary");
                engine.stop();
                let _ = std::fs::remove_file(&first);
                let _ = std::fs::remove_file(&second);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("a bare status poll never saw the boundary");
    }

    /// Blend skips (C6): with the option on, a manual skip retires the old
    /// track over a full second — at half a second its ramp is still
    /// audibly mid-descent, where a breath would already be silence.
    ///
    /// `cargo test a_blended_skip -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_blended_skip_crosses_in_a_second_not_a_breath() {
        let dir = std::env::temp_dir();
        let first = dir.join("mstream-skipblend-a.wav");
        let second = dir.join("mstream-skipblend-b.wav");
        std::fs::write(&first, wav_bytes(4)).unwrap();
        std::fs::write(&second, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_blend_skips(true);
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);
        std::thread::sleep(Duration::from_millis(400));

        engine.next_manual().unwrap();
        assert!(engine.status().playing, "the chosen track is on at once");
        std::thread::sleep(Duration::from_millis(500));
        let positions = engine.drainer_positions();
        assert!(
            positions.iter().any(|&p| p > 0.2 && p < 0.9),
            "half a second in, a blended skip is mid-ramp, not a finished breath: {positions:?}"
        );

        // And it still ends: drained within the second plus a tick or two.
        let waited = std::time::Instant::now();
        while engine.overlap_active() {
            engine.advance_tick();
            assert!(waited.elapsed() < Duration::from_secs(2), "the skip blend never ended");
            std::thread::sleep(Duration::from_millis(30));
        }
        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// The soft pause (C6): the pause itself lands a beat after the key,
    /// once the ramp has reached silence — and resume ramps back up from
    /// wherever it stood. With the option off, everything is as it was.
    ///
    /// `cargo test a_soft_pause -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_soft_pause_lands_after_its_ramp_and_resumes_whole() {
        let tiny = std::env::temp_dir().join("mstream-softpause.wav");
        std::fs::write(&tiny, wav_bytes(4)).unwrap();

        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.set_pause_fade(true);
        engine.play_source(tiny.to_string_lossy().into_owned(), Some(4.0)).unwrap();
        std::thread::sleep(Duration::from_millis(300));

        engine.pause();
        assert!(!engine.status().paused, "the pause rides the ramp, not the keystroke");
        let asked = std::time::Instant::now();
        while !engine.status().paused {
            engine.advance_tick();
            assert!(asked.elapsed() < Duration::from_secs(1), "the pause never landed");
            std::thread::sleep(Duration::from_millis(30));
        }

        engine.resume();
        assert!(!engine.status().paused, "resume is immediate");
        let before = engine.status().position;
        std::thread::sleep(Duration::from_millis(400));
        assert!(engine.status().position > before, "and playback truly moves again");

        engine.stop();
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
        // Two megabytes before the stall: ~11s of audio, so plenty remains
        // playable when the seek unblocks — the earlier 500KB version
        // passed by coincidence, its audio exhausted before the up-ramp
        // could have sounded (fix-round review).
        let url = stalling_wav_server(30, 2_000_000, Duration::from_secs(4));
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
    /// scale (C4 review). Without ticks, the boundary can only be caught
    /// inside the stop itself (promote_if_crossed at its top, and
    /// adopt_appended_for_retire's own crossed check behind it).
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
        // And the breath itself survived the start: install hushes long
        // drainers but leaves breaths to finish — hard-cutting the very
        // breath this gate was fixed to respect was the fleet-defeating
        // click (hush round).
        assert!(engine.drainers() >= 1, "the stop's breath was cut, not kept");
        // Surviving in the LIST is not surviving: retire_softly's early
        // exit used to stop() the parked sink through its self.sink alias,
        // leaving the entry in the fleet with its ramp frozen at speaking
        // gain (verify round, critic). A live breath ramps to silence
        // within a couple hundred ms; a stopped one stays stuck high.
        std::thread::sleep(Duration::from_millis(400));
        for position in engine.drainer_positions() {
            assert!(position <= 0.05, "the breath was silently hard-stopped at {position}");
        }

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

    /// `cargo test a_lost_device -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_lost_device_is_rebuilt_where_the_music_stood() {
        // Nobody can unplug hardware from a test, but the tripwire the
        // stream's error callback pulls is ours to pull by hand — from
        // there the recovery is the path a real unplug takes: reopen,
        // reattach, seek back to where the music stood.
        let tiny = std::env::temp_dir().join("mstream-device-rebuild.wav");
        std::fs::write(&tiny, wav_bytes(30)).unwrap();
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(tiny.to_string_lossy().into_owned(), None).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
        let before = engine.status();
        assert!(before.playing && before.position > 0.5, "the test needs a track underway");

        engine.pretend_device_lost();
        engine.advance_tick();

        let after = engine.status();
        assert_eq!(after.file, before.file, "the rebuild must resume the same track");
        assert!(after.playing, "the rebuild must leave the track playing");
        assert!(
            after.position >= before.position - 0.75,
            "resumed at {:.2}s but the music stood at {:.2}s",
            after.position,
            before.position
        );
        let notices = engine.take_device_notices();
        assert!(
            notices.iter().any(|n| !n.lost),
            "a successful move must be announced: {notices:?}"
        );

        // And it is genuinely sounding again: the position advances.
        std::thread::sleep(Duration::from_millis(800));
        let later = engine.status();
        assert!(
            later.position > after.position + 0.3,
            "position froze after the rebuild ({:.2}s then {:.2}s)",
            after.position,
            later.position
        );
        engine.stop();
    }

    /// `cargo test a_paused_player_survives -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_paused_player_survives_the_device_swap_paused() {
        let tiny = std::env::temp_dir().join("mstream-device-rebuild-paused.wav");
        std::fs::write(&tiny, wav_bytes(30)).unwrap();
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(tiny.to_string_lossy().into_owned(), None).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        engine.pause();
        let held = engine.status();
        assert!(held.paused);

        engine.pretend_device_lost();
        engine.advance_tick();

        let after = engine.status();
        assert!(after.paused, "a paused player must come back paused");
        assert_eq!(after.file, held.file);
        assert!(
            (after.position - held.position).abs() < 0.75,
            "the pause held at {:.2}s but the rebuild reports {:.2}s",
            held.position,
            after.position
        );

        // The resume is an ordinary resume: the new sink plays on.
        engine.resume();
        std::thread::sleep(Duration::from_millis(800));
        let later = engine.status();
        assert!(later.playing);
        assert!(
            later.position > after.position + 0.3,
            "position froze after resume ({:.2}s then {:.2}s)",
            after.position,
            later.position
        );
        engine.stop();
    }

    /// A State with no device behind it: the sink is a rodio Player that
    /// nothing pulls, which is all the bookkeeping predicates need.
    fn bare_state() -> State {
        State {
            sink: Arc::new(Player::new().0),
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
            prepare_plain: false,
            appended: None,
            blend_skips: false,
            pause_fade: false,
            pausing: None,
            orphaned_tail: false,
            gate_noted: None,
            pending_next: None,
            next: NextTrack::Idle,
            outgoing: Vec::new(),
            q: QueueState { queue: Vec::new(), index: 0, shuffle: false, loop_mode: LoopMode::None },
        }
    }

    /// A sink with a track in it, as far as the bookkeeping can tell.
    fn loaded_sink() -> Arc<Player> {
        let sink = Player::new().0;
        sink.append(rodio::source::Zero::new(
            std::num::NonZero::new(2).unwrap(),
            std::num::NonZero::new(44_100).unwrap(),
        ));
        Arc::new(sink)
    }

    #[test]
    fn at_rest_is_stopped_or_a_landed_pause_with_nothing_in_flight() {
        // Never played, and stopped: nothing will happen without a command.
        let mut s = bare_state();
        assert!(s.at_rest(), "a fresh engine is at rest");

        // Playing is never at rest.
        s.sink = loaded_sink();
        s.stopped = false;
        assert!(!s.at_rest(), "a sounding track is not at rest");

        // A landed pause with the track still in the sink is.
        s.sink.pause();
        assert!(s.at_rest(), "a landed pause is at rest");
        // A soft pause still ramping down is not: the tick owes it a landing.
        s.pausing = Some(Instant::now());
        assert!(!s.at_rest(), "a pause still ramping is not at rest");
        s.pausing = None;
        // A prepared next waiting out the pause changes nothing; an open in
        // flight is the tick's to collect.
        s.next = NextTrack::Failed { at: Instant::now() };
        assert!(s.at_rest());
        let (_tx, rx) = mpsc::channel();
        let opener = Opener { rx, thread: std::thread::spawn(|| {}) };
        s.next = NextTrack::Opening { index: None, opener };
        assert!(!s.at_rest(), "an open in flight is not at rest");
        s.next = NextTrack::Idle;
        // An orphaned remnant is the tick's to skip.
        s.orphaned_tail = true;
        assert!(!s.at_rest());
        s.orphaned_tail = false;

        // A pause over an empty sink is a track that ran out under the
        // pause key: the advance is still owed, so it is not rest.
        s.sink = Arc::new(Player::new().0);
        s.sink.pause();
        assert!(!s.at_rest(), "an ended track under a pause still advances");

        // Stopped with a breath still draining is not rest either: the
        // callback has to play the breath out.
        let mut s = bare_state();
        s.outgoing.push(Outgoing {
            sink: loaded_sink(),
            fade: fade::FadeHandle::new(1.0),
            fade_dur: STOP_FADE,
            deadline: Instant::now() + STOP_FADE,
        });
        assert!(!s.at_rest(), "a draining breath is not at rest");
    }

    #[test]
    fn the_next_tick_lands_on_the_end_of_the_track_and_nowhere_faster() {
        let base = Duration::from_millis(250);
        let ms = |d: Duration| d.as_millis();
        // Settled: the device watch's pace, whatever the base.
        assert_eq!(next_tick(true, None, base), DEVICE_POLL);
        assert_eq!(next_tick(true, None, Duration::from_secs(2)), Duration::from_secs(2));
        // Nothing sounding toward a known end: the base.
        assert_eq!(next_tick(false, None, base), base);
        // Far from the end: the base, never longer.
        assert_eq!(next_tick(false, Some(90.0), base), base);
        // Inside the last quarter second: just before the end.
        assert_eq!(ms(next_tick(false, Some(0.2), base)), 170);
        assert_eq!(ms(next_tick(false, Some(0.05), base)), 20);
        // At or past the end, the floor: the source is about to run dry.
        assert_eq!(next_tick(false, Some(0.01), base), END_POLL);
        assert_eq!(next_tick(false, Some(-0.5), base), END_POLL);
        // Well past a length that undershot: the base again, so a wrong
        // duration costs at most a second of fast ticks, not minutes.
        assert_eq!(next_tick(false, Some(-1.5), base), base);
        // A base shorter than the floor is never stretched.
        assert_eq!(next_tick(false, Some(0.0), Duration::from_millis(2)), Duration::from_millis(2));
    }

    #[test]
    fn until_end_counts_only_a_track_sounding_toward_a_known_end() {
        let mut s = bare_state();
        assert_eq!(s.until_end(), None, "stopped");
        s.sink = loaded_sink();
        s.stopped = false;
        assert_eq!(s.until_end(), None, "no known length");
        s.duration = 180.0;
        // Nothing pulls this sink, so the position stands at zero.
        assert_eq!(s.until_end(), Some(180.0));
        s.pausing = Some(Instant::now());
        assert_eq!(s.until_end(), None, "ramping into a pause");
        s.pausing = None;
        s.sink.pause();
        assert_eq!(s.until_end(), None, "paused");
        s.sink = Arc::new(Player::new().0);
        assert_eq!(s.until_end(), None, "ran out: the tick's advance is due at once");
    }

    /// Run `f` on its own thread and give it `limit`: a call that waits on
    /// a callback that never comes must fail the test, not hang it.
    fn within<T: Send + 'static>(
        limit: Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit).expect("the call never returned — waiting on a sleeping device?")
    }

    /// Tick for `span`, the way a driver would.
    fn tick_for(engine: &Engine, span: Duration) {
        let until = Instant::now() + span;
        while Instant::now() < until {
            engine.advance_tick();
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    /// `cargo test the_device_sleeps -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn the_device_sleeps_when_nothing_needs_it_and_wakes_for_what_does() {
        // The stream used to run for the whole process — stopped, paused,
        // never played — holding the machine awake (audit #75). Now it
        // sleeps after a quiet spell, and everything that needs the
        // callback wakes it first: a seek would otherwise wait forever.
        let first = std::env::temp_dir().join("mstream-idle-a.wav");
        let second = std::env::temp_dir().join("mstream-idle-b.wav");
        std::fs::write(&first, wav_bytes(30)).unwrap();
        std::fs::write(&second, wav_bytes(30)).unwrap();
        let engine = Arc::new(Engine::new().unwrap());
        engine.set_volume(0.0);

        // Never played: awake at the open (it proves the device), asleep
        // once the stopped threshold passes.
        engine.advance_tick();
        assert!(engine.output_awake(), "the open starts the stream");
        tick_for(&engine, IDLE_STOPPED + Duration::from_millis(300));
        assert!(!engine.output_awake(), "a never-played engine lets the device sleep");

        // A play wakes it, and the track sounds: the position advances.
        engine.queue_add_many(vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]);
        assert!(engine.output_awake(), "a play wakes the device before it starts");
        tick_for(&engine, Duration::from_millis(900));
        let played = engine.status();
        assert!(played.playing && played.position > 0.4, "the play sounded: {:.2}", played.position);

        // Playing never sleeps, however long it runs.
        tick_for(&engine, IDLE_PAUSED + Duration::from_millis(300));
        assert!(engine.output_awake(), "a playing engine keeps the device");

        // A landed pause sleeps after the (longer) paused threshold.
        engine.pause();
        tick_for(&engine, IDLE_STOPPED + Duration::from_millis(100));
        assert!(engine.output_awake(), "a pause gets longer grace than a stop");
        tick_for(&engine, IDLE_PAUSED);
        assert!(!engine.output_awake(), "a landed pause lets the device sleep");
        let held = engine.status();
        assert!(held.paused);

        // A seek against the sleeping stream wakes it and lands.
        let seeker = engine.clone();
        within(Duration::from_secs(5), move || seeker.seek(12.0)).expect("the seek landed");
        let sought = engine.status();
        assert!(sought.paused, "a seek keeps the pause");
        assert!((sought.position - 12.0).abs() < 0.5, "landed at {:.2}", sought.position);

        // Asleep again, then a resume wakes it and the track runs on.
        tick_for(&engine, IDLE_PAUSED + Duration::from_millis(300));
        assert!(!engine.output_awake());
        engine.resume();
        assert!(engine.output_awake(), "a resume wakes the device");
        tick_for(&engine, Duration::from_millis(900));
        let resumed = engine.status();
        assert!(resumed.playing && resumed.position > sought.position + 0.4,
            "resumed from {:.2} to {:.2}", sought.position, resumed.position);

        // Next against a sleeping device: the second track starts and runs.
        engine.pause();
        tick_for(&engine, IDLE_PAUSED + Duration::from_millis(300));
        assert!(!engine.output_awake());
        let skipper = engine.clone();
        within(Duration::from_secs(5), move || skipper.next_manual()).expect("next started");
        tick_for(&engine, Duration::from_millis(900));
        let next = engine.status();
        assert_eq!(next.queue_index, 1);
        assert!(next.playing && next.position > 0.4, "next sounded: {:.2}", next.position);

        // A stop, and the stopped threshold.
        engine.stop();
        tick_for(&engine, IDLE_STOPPED + Duration::from_millis(400));
        assert!(!engine.output_awake(), "a stopped engine lets the device sleep");
        // Previous restarts by seeking — against a sleeping device too.
        let restarter = engine.clone();
        within(Duration::from_secs(5), move || restarter.previous_manual()).expect("previous");
        engine.stop();
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    /// `cargo test a_seek_waiting -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_seek_waiting_on_the_callback_holds_the_device_awake() {
        // A seek waits on the callback with the state lock released (audit
        // #48); a tick from another thread must not put the device to sleep
        // under it, or the answer never comes.
        let tiny = std::env::temp_dir().join("mstream-idle-wait.wav");
        std::fs::write(&tiny, wav_bytes(30)).unwrap();
        let engine = Engine::new().unwrap();
        engine.set_volume(0.0);
        engine.play_source(tiny.to_string_lossy().into_owned(), None).unwrap();
        engine.pause();
        let waiting = CallbackWait::new(&engine.callback_waits);
        tick_for(&engine, IDLE_PAUSED + Duration::from_millis(300));
        assert!(engine.output_awake(), "a waiting seek holds the device awake");
        drop(waiting);
        tick_for(&engine, IDLE_PAUSED + Duration::from_millis(300));
        assert!(!engine.output_awake(), "and lets it go when the wait ends");
        let _ = std::fs::remove_file(&tiny);
    }

    /// `cargo test a_pull_stuck -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an audio device"]
    fn a_pull_stuck_on_the_network_keeps_the_device_and_not_the_controls() {
        // A download that stalls mid-track parks the device callback in the
        // decoder's read (audit #48's shape), and suspending the stream
        // waits for the callback in flight. The sleep, tried under such a
        // pull once a stop's threshold passed, held the tick — serve's
        // loop, the TUI's audio thread, every control behind them — until
        // the network answered (review of audit #75). The stream stays up
        // instead, and sleeps once the pull comes back.
        //
        // The open wants about half a megabyte before it answers: three
        // seconds of this WAV, then the hold.
        let (url, release) = held_wav_server(30, 600_000);
        let engine = Arc::new(Engine::new().unwrap());
        engine.set_volume(0.0);
        engine.play_source(url, Some(30.0)).unwrap();

        // What arrived plays out and the position stops: the callback is
        // waiting on the network.
        let started = Instant::now();
        let mut last = -1.0;
        loop {
            std::thread::sleep(Duration::from_millis(150));
            let at = engine.status().position;
            if at > 1.0 && at == last {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(8), "the stall never came ({at:.2})");
            last = at;
        }

        // Stopped under the stuck pull, then ticked past everything a stop
        // leaves to settle — its breath waits out the outgoing slack, the
        // callback being in no state to play it — and the stopped
        // threshold: the ticks come back, and so does status.
        engine.stop();
        let span = STOP_FADE + OUTGOING_SLACK + IDLE_STOPPED + Duration::from_millis(400);
        let ticker = engine.clone();
        within(span + Duration::from_secs(1), move || {
            tick_for(&ticker, span);
            assert!(ticker.settled(), "the stop has settled");
        });
        let asker = engine.clone();
        assert!(!within(Duration::from_millis(500), move || asker.status()).playing);
        assert!(engine.output_awake(), "a stream mid-pull is left running");

        // The rest arrives, the pull returns, the stopped track leaves the
        // mixer: now the stream sleeps.
        release.store(true, Ordering::Release);
        let ticker = engine.clone();
        within(Duration::from_secs(8), move || {
            while ticker.output_awake() {
                ticker.advance_tick();
                std::thread::sleep(Duration::from_millis(40));
            }
        });
    }
}
