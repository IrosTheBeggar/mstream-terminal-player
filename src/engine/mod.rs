//! Playback core: rodio sink management, queue bookkeeping, transport state.
//!
//! Ported from mStream's rust-server-audio (mStream@bec11154). Behavior-compatible
//! with the original except for the audit fixes listed in PLAN.md: volume persists
//! across track changes, manual next/previous bypass loop-one, device failures are
//! errors instead of panics, and removing the current queue entry while stopped no
//! longer starts playback.

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
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
    /// File could not be opened or decoded.
    Unplayable,
    OutOfBounds,
    EndOfQueue,
    Seek(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NoDevice(e) => write!(f, "Audio device unavailable: {}", e),
            EngineError::Unplayable => write!(f, "File could not be played"),
            EngineError::OutOfBounds => write!(f, "Index out of bounds"),
            EngineError::EndOfQueue => write!(f, "Already at end of queue"),
            EngineError::Seek(e) => write!(f, "Seek failed: {}", e),
        }
    }
}

impl std::error::Error for EngineError {}

// ── Queue bookkeeping (kept free of audio handles so it unit-tests without a device) ──

#[derive(Debug)]
pub(crate) struct QueueState {
    pub queue: Vec<String>,
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

struct State {
    sink: Sink,
    current_file: String,
    duration: f64,
    stopped: bool,
    /// Desired volume; survives sink recreation on track change (audit fix #1).
    volume: f32,
    q: QueueState,
}

impl State {
    /// Start playing q.queue[q.index]. Opens and decodes BEFORE touching the
    /// running sink, so a bad file leaves current playback untouched (same
    /// ordering as the original).
    fn start_current(&mut self, handle: &OutputStreamHandle) -> Result<(), EngineError> {
        if self.q.index >= self.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        let path = self.q.queue[self.q.index].clone();
        let file = File::open(&path).map_err(|e| {
            eprintln!("[engine] open failed for {}: {}", path, e);
            EngineError::Unplayable
        })?;
        let source = Decoder::new(BufReader::new(file)).map_err(|e| {
            eprintln!("[engine] decode failed for {}: {}", path, e);
            EngineError::Unplayable
        })?;
        let duration = probe_duration(&path);

        self.sink.stop();
        let sink = match Sink::try_new(handle) {
            Ok(s) => s,
            Err(e) => {
                self.stopped = true;
                return Err(EngineError::NoDevice(e.to_string()));
            }
        };
        sink.set_volume(self.volume);
        sink.append(source);
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
    _stream: OutputStream,
    handle: OutputStreamHandle,
    state: Arc<Mutex<State>>,
}

impl Engine {
    pub fn new() -> Result<Self, EngineError> {
        let (stream, handle) =
            OutputStream::try_default().map_err(|e| EngineError::NoDevice(e.to_string()))?;
        let sink = Sink::try_new(&handle).map_err(|e| EngineError::NoDevice(e.to_string()))?;

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

        Ok(Engine { _stream: stream, handle, state })
    }

    /// Clear the queue, add one file, play it.
    pub fn play_file(&self, path: String) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        s.q.queue.clear();
        s.q.queue.push(path);
        s.q.index = 0;
        s.start_current(&self.handle)
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
        Status {
            playing: !is_empty && !is_paused,
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
                s.start_current(&self.handle)
            }
            None => Err(EngineError::EndOfQueue),
        }
    }

    pub fn previous_manual(&self) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        if s.q.index == 0 {
            if s.q.loop_mode == LoopMode::All && !s.q.queue.is_empty() {
                s.q.index = s.q.queue.len() - 1;
                s.start_current(&self.handle)
            } else {
                let _ = s.sink.try_seek(Duration::ZERO);
                Ok(())
            }
        } else {
            s.q.index -= 1;
            s.start_current(&self.handle)
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

    /// Append one file; if the queue was empty and nothing is playing, start.
    /// Failure to start is deliberately not an error (matches the original).
    pub fn queue_add(&self, file: String) {
        let mut s = self.state.lock().unwrap();
        let was_empty = s.q.queue.is_empty();
        s.q.queue.push(file);
        if was_empty && s.sink.empty() {
            s.q.index = 0;
            let _ = s.start_current(&self.handle);
        }
    }

    pub fn queue_add_many(&self, files: Vec<String>) {
        let mut s = self.state.lock().unwrap();
        let was_empty = s.q.queue.is_empty();
        s.q.queue.extend(files);
        if was_empty && s.sink.empty() && !s.q.queue.is_empty() {
            s.q.index = 0;
            let _ = s.start_current(&self.handle);
        }
    }

    pub fn queue_play_index(&self, index: usize) -> Result<(), EngineError> {
        let mut s = self.state.lock().unwrap();
        if index >= s.q.queue.len() {
            return Err(EngineError::OutOfBounds);
        }
        s.q.index = index;
        s.start_current(&self.handle)
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
                    let _ = s.start_current(&self.handle);
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
        QueueSnapshot { queue: s.q.queue.clone(), current_index: s.q.index }
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
                    match s.start_current(&self.handle) {
                        Ok(()) => break,
                        Err(EngineError::NoDevice(e)) => {
                            eprintln!("[engine] audio device lost during advance: {}", e);
                            s.clear_current();
                            break;
                        }
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

// ── Duration detection via symphonia ────────────────────────────────────────

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

    fn q(len: usize, index: usize) -> QueueState {
        QueueState {
            queue: (0..len).map(|i| format!("t{}", i)).collect(),
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
        assert_eq!(state.queue[1], "t2");
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
