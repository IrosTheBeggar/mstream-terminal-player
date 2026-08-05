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

pub(crate) mod http;
pub(crate) mod tap;

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

struct State {
    /// Arc'd so a blocking call on the sink — a seek waiting for the device
    /// callback to reach data that has not downloaded — can be made on a
    /// clone with the state lock already released. Held across the wait,
    /// the lock froze every other control with it (audit #48).
    sink: Arc<Player>,
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
    q: QueueState,
}

impl State {
    /// Start playing q.queue[q.index]. Opens and decodes BEFORE touching the
    /// running sink, so a bad source leaves current playback untouched (same
    /// ordering as the original).
    ///
    /// The decoder is always given `byte_len` when we know it (file metadata
    /// or HTTP Content-Length): symphonia's FLAC reader needs the total
    /// length for binary-search seeking on files without a SEEKTABLE block —
    /// rodio 0.20 hardcoded byte_len to None, which made those files
    /// unseekable (audit finding #13).
    fn start_current(&mut self, mixer: &Mixer) -> Result<(), EngineError> {
        if self.q.index >= self.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        let entry = self.q.queue[self.q.index].clone();
        let path = entry.path;

        // Diagnostics here go through `stderrln!`: these fire mid-session
        // from the audio thread, and in the TUI that stderr lands raw on
        // the alternate screen — where the same news already arrives as
        // PlaybackFailed. Serve and the CLI keep the lines (audit #43).
        let (opened, duration) = if http::is_http_url(&path) {
            let redacted = http::redact_source(&path);
            let (reader, content_length) = http::open(&path).map_err(|e| {
                crate::stderrln!("[engine] open failed for {}: {}", redacted, e);
                EngineError::Unplayable(e)
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
                EngineError::Unplayable(e.to_string())
            })?;
            let duration = entry
                .duration_hint
                .or_else(|| decoder.total_duration().map(|d| d.as_secs_f64()))
                .unwrap_or(0.0);
            (Opened::Http(decoder), duration)
        } else {
            let file = File::open(&path).map_err(|e| {
                crate::stderrln!("[engine] open failed for {}: {}", path, e);
                EngineError::Unplayable(e.to_string())
            })?;
            let byte_len = file.metadata().ok().map(|m| m.len());
            let mut builder =
                Decoder::builder().with_data(BufReader::new(file)).with_seekable(true);
            if let Some(len) = byte_len {
                builder = builder.with_byte_len(len);
            }
            let decoder = builder.build().map_err(|e| {
                crate::stderrln!("[engine] decode failed for {}: {}", path, e);
                EngineError::Unplayable(e.to_string())
            })?;
            let duration = entry.duration_hint.unwrap_or_else(|| probe_duration(&path));
            (Opened::Local(decoder), duration)
        };

        self.sink.stop();
        let sink = Player::connect_new(mixer);
        sink.set_volume(self.volume);
        match (opened, self.tap.clone()) {
            (Opened::Local(d), Some(tap)) => sink.append(tap::Tapped::new(d, tap)),
            (Opened::Local(d), None) => sink.append(d),
            (Opened::Http(d), Some(tap)) => sink.append(tap::Tapped::new(d, tap)),
            (Opened::Http(d), None) => sink.append(d),
        }
        self.sink = Arc::new(sink);
        self.current_file = path;
        self.duration = duration;
        self.stopped = false;
        self.advance_failures = 0;
        Ok(())
    }

    fn clear_current(&mut self) {
        self.current_file.clear();
        self.duration = 0.0;
        self.stopped = true;
        self.advance_failures = 0;
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
            tap: None,
            current_file: String::new(),
            duration: 0.0,
            stopped: true,
            volume: 1.0,
            advance_failures: 0,
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
        s.start_current(self.device.mixer())
    }

    pub fn pause(&self) {
        self.state.lock().unwrap().sink.pause();
    }

    pub fn resume(&self) {
        self.state.lock().unwrap().sink.play();
    }

    pub fn stop(&self) {
        let mut s = self.state.lock().unwrap();
        s.sink.stop();
        s.clear_current();
    }

    pub fn seek(&self, position: f64) -> Result<(), EngineError> {
        let target = seek_target(position)?;
        // Take a handle and let the state go before asking. try_seek blocks
        // on rodio's feedback channel until the device callback performs
        // the seek, and past the downloaded range that callback is waiting
        // on the network — possibly forever, since the download loop never
        // gives a dead server up. The wait is this caller's to make; the
        // lock was making it everyone's (audit #48).
        let sink = self.state.lock().unwrap().sink.clone();
        sink.try_seek(target).map_err(|e| EngineError::Seek(e.to_string()))
    }

    pub fn set_volume(&self, volume: f32) {
        let mut s = self.state.lock().unwrap();
        let v = if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 1.0 };
        s.volume = v;
        s.sink.set_volume(v);
    }

    pub fn status(&self) -> Status {
        let s = self.state.lock().unwrap();
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
                s.start_current(self.device.mixer())
            }
            None => Err(EngineError::EndOfQueue),
        }
    }

    pub fn previous_manual(&self) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        if s.q.index == 0 {
            if s.q.loop_mode == LoopMode::All && !s.q.queue.is_empty() {
                s.q.index = s.q.queue.len() - 1;
                s.start_current(self.device.mixer())
            } else {
                // The restart is a seek like any other — same discipline as
                // [`Engine::seek`], even though the start of the track has
                // almost always downloaded by now.
                let sink = s.sink.clone();
                drop(s);
                let _ = sink.try_seek(Duration::ZERO);
                Ok(())
            }
        } else {
            s.q.index -= 1;
            s.start_current(self.device.mixer())
        }
    }

    pub fn set_shuffle(&self, value: bool) {
        self.state.lock().unwrap().q.shuffle = value;
    }

    pub fn cycle_loop(&self) -> LoopMode {
        let mut s = self.state.lock().unwrap();
        s.q.loop_mode = s.q.loop_mode.next();
        s.q.loop_mode
    }

    /// Append one source; if the queue was empty and nothing is playing, start.
    /// Failure to start is deliberately not an error (matches the original).
    pub fn queue_add(&self, file: String) {
        self.queue_add_entry(QueueEntry::new(file));
    }

    pub fn queue_add_entry(&self, entry: QueueEntry) {
        let mut s = self.state.lock().unwrap();
        let was_empty = s.q.queue.is_empty();
        s.q.queue.push(entry);
        if was_empty && s.sink.empty() {
            s.q.index = 0;
            let _ = s.start_current(self.device.mixer());
        }
    }

    pub fn queue_add_many(&self, files: Vec<String>) {
        let mut s = self.state.lock().unwrap();
        let was_empty = s.q.queue.is_empty();
        s.q.queue.extend(files.into_iter().map(QueueEntry::new));
        if was_empty && s.sink.empty() && !s.q.queue.is_empty() {
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
        s.start_current(self.device.mixer())
    }

    pub fn queue_remove(&self, index: usize) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        if index >= s.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        match apply_remove(&mut s.q, index) {
            RemoveOutcome::EmptiedQueue => {
                s.sink.stop();
                s.clear_current();
            }
            RemoveOutcome::RemovedCurrent => {
                // Audit fix #4: only restart playback if we were actually
                // playing; the original started audio as a side effect of
                // removing a track while stopped.
                if !s.stopped {
                    let _ = s.start_current(self.device.mixer());
                }
            }
            RemoveOutcome::RemovedBeforeCurrent | RemoveOutcome::RemovedAfterCurrent => {}
        }
        Ok(())
    }

    pub fn queue_clear(&self) {
        let mut s = self.state.lock().unwrap();
        s.sink.stop();
        s.q.queue.clear();
        s.q.index = 0;
        s.clear_current();
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
    pub fn advance_tick(&self) {
        let mut s = self.state.lock().unwrap();
        if !(s.sink.empty() && !s.stopped && !s.q.queue.is_empty()) {
            return;
        }
        match pick_next(&s.q, false) {
            None => s.clear_current(),
            Some(idx) => {
                s.q.index = idx;
                if s.start_current(self.device.mixer()).is_ok() {
                    return;
                }
                s.advance_failures += 1;
                if s.advance_failures >= s.q.queue.len() {
                    s.clear_current();
                }
            }
        }
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
