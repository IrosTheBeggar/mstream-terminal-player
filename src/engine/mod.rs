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

/// Pick the next index based on shuffle/loop settings. `manual` skips honor
/// shuffle and loop-all wrapping but are never trapped by loop-one — that mode
/// only applies to automatic track-end advancement.
pub(crate) fn pick_next(q: &QueueState, manual: bool) -> Option<usize> {
    if q.queue.is_empty() {
        return None;
    }
    if !manual && q.loop_mode == LoopMode::One {
        return Some(q.index);
    }
    if q.shuffle {
        if q.queue.len() <= 1 {
            return Some(0);
        }
        let offset = fastrand::usize(1..q.queue.len());
        return Some((q.index + offset) % q.queue.len());
    }
    let next = q.index + 1;
    if next < q.queue.len() {
        Some(next)
    } else if q.loop_mode == LoopMode::All {
        Some(0)
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    EmptiedQueue,
    RemovedBeforeCurrent,
    RemovedCurrent,
    RemovedAfterCurrent,
}

/// Remove `index` (caller has bounds-checked) and fix up the current index.
pub(crate) fn apply_remove(q: &mut QueueState, index: usize) -> RemoveOutcome {
    q.queue.remove(index);
    if q.queue.is_empty() {
        q.index = 0;
        return RemoveOutcome::EmptiedQueue;
    }
    if index < q.index {
        q.index -= 1;
        return RemoveOutcome::RemovedBeforeCurrent;
    }
    if index == q.index {
        if q.index >= q.queue.len() {
            q.index = q.queue.len() - 1;
        }
        return RemoveOutcome::RemovedCurrent;
    }
    RemoveOutcome::RemovedAfterCurrent
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
    sink: Player,
    current_file: String,
    duration: f64,
    stopped: bool,
    /// Desired volume; survives sink recreation on track change (audit fix #1).
    volume: f32,
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

        let (opened, duration) = if http::is_http_url(&path) {
            let redacted = http::redact_source(&path);
            let (reader, content_length) = http::open(&path).map_err(|e| {
                eprintln!("[engine] open failed for {}: {}", redacted, e);
                EngineError::Unplayable(e)
            })?;
            if content_length.is_none() {
                eprintln!(
                    "[engine] {}: no content length — seek limited to downloaded data",
                    redacted
                );
            }
            let mut builder = Decoder::builder().with_data(reader).with_seekable(true);
            if let Some(len) = content_length {
                builder = builder.with_byte_len(len);
            }
            let decoder = builder.build().map_err(|e| {
                eprintln!("[engine] decode failed for {}: {}", redacted, e);
                EngineError::Unplayable(e.to_string())
            })?;
            let duration = entry
                .duration_hint
                .or_else(|| decoder.total_duration().map(|d| d.as_secs_f64()))
                .unwrap_or(0.0);
            (Opened::Http(decoder), duration)
        } else {
            let file = File::open(&path).map_err(|e| {
                eprintln!("[engine] open failed for {}: {}", path, e);
                EngineError::Unplayable(e.to_string())
            })?;
            let byte_len = file.metadata().ok().map(|m| m.len());
            let mut builder =
                Decoder::builder().with_data(BufReader::new(file)).with_seekable(true);
            if let Some(len) = byte_len {
                builder = builder.with_byte_len(len);
            }
            let decoder = builder.build().map_err(|e| {
                eprintln!("[engine] decode failed for {}: {}", path, e);
                EngineError::Unplayable(e.to_string())
            })?;
            let duration = entry.duration_hint.unwrap_or_else(|| probe_duration(&path));
            (Opened::Local(decoder), duration)
        };

        self.sink.stop();
        let sink = Player::connect_new(mixer);
        sink.set_volume(self.volume);
        match opened {
            Opened::Local(d) => sink.append(d),
            Opened::Http(d) => sink.append(d),
        }
        self.sink = sink;
        self.current_file = path;
        self.duration = duration;
        self.stopped = false;
        Ok(())
    }

    fn clear_current(&mut self) {
        self.current_file.clear();
        self.duration = 0.0;
        self.stopped = true;
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
            sink,
            current_file: String::new(),
            duration: 0.0,
            stopped: true,
            volume: 1.0,
            q: QueueState {
                queue: Vec::new(),
                index: 0,
                shuffle: false,
                loop_mode: LoopMode::None,
            },
        }));

        Ok(Engine { state, device })
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
        if !position.is_finite() || position < 0.0 {
            return Err(EngineError::Seek("invalid position".to_string()));
        }
        let s = self.state.lock().unwrap();
        s.sink
            .try_seek(Duration::from_secs_f64(position))
            .map_err(|e| EngineError::Seek(e.to_string()))
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
                let _ = s.sink.try_seek(Duration::ZERO);
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
    /// serve poll loop (and later the TUI tick). Skips unplayable tracks, at
    /// most one full pass over the queue per tick (same as the original).
    pub fn advance_tick(&self) {
        let mut s = self.state.lock().unwrap();
        if !(s.sink.empty() && !s.stopped && !s.q.queue.is_empty()) {
            return;
        }
        let mut attempts = 0;
        loop {
            match pick_next(&s.q, false) {
                Some(idx) => {
                    s.q.index = idx;
                    match s.start_current(self.device.mixer()) {
                        Ok(()) => break,
                        Err(_) => {
                            attempts += 1;
                            if attempts >= s.q.queue.len() {
                                s.clear_current();
                                break;
                            }
                        }
                    }
                }
                None => {
                    s.clear_current();
                    break;
                }
            }
        }
    }
}

// ── Duration detection via symphonia (local files only) ────────────────────

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
