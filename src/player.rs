//! A thin control surface over whatever is actually producing sound.
//!
//! The TUI talks to playback only through this trait. Today the sole
//! implementation is the in-process rodio engine; keeping the boundary here
//! means an external backend (an mpv subprocess over JSON IPC, say) could be
//! dropped in if the engine ever runs into something it can't do — gapless,
//! device hotplug, exotic codecs. Every terminal player surveyed for PLAN.md
//! skipped this abstraction and wished it hadn't.

#[cfg(not(target_arch = "wasm32"))]
use crate::engine::Engine;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerStatus {
    pub playing: bool,
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f32,
    /// The source currently loaded — a URL or path. Empty when idle.
    pub source: String,
}

impl PlayerStatus {
    /// Progress through the current track, 0.0..=1.0. Zero when the duration
    /// is unknown (a live transcode, or a file without duration metadata).
    pub fn progress(&self) -> f64 {
        if self.duration <= 0.0 || !self.duration.is_finite() {
            return 0.0;
        }
        (self.position / self.duration).clamp(0.0, 1.0)
    }

    pub fn is_idle(&self) -> bool {
        self.source.is_empty()
    }
}

pub trait PlayerCtl {
    fn play(&self, source: &str, duration_hint: Option<f64>) -> Result<(), String>;
    fn pause(&self);
    fn resume(&self);
    fn stop(&self);
    fn seek(&self, position: f64) -> Result<(), String>;
    fn set_volume(&self, volume: f32);
    /// Seconds of blend between tracks; 0 is off.
    fn set_crossfade(&self, seconds: f32);
    /// Sample-tight transitions when no blend is configured.
    fn set_gapless(&self, on: bool);
    /// Manual skips blend for a second instead of breathing.
    fn set_blend_skips(&self, on: bool);
    /// Pause and resume ride a short ramp instead of landing mid-wave.
    fn set_pause_fade(&self, on: bool);
    /// Announce what plays after the current source, so a blend can open it
    /// ahead of the fade window. Replaces any earlier announcement.
    fn prepare_next(&self, source: &str, duration_hint: Option<f64>);
    /// Withdraw the announcement: nothing follows the current source.
    fn clear_next(&self);
    fn status(&self) -> PlayerStatus;
    /// Drive any background bookkeeping (end-of-track handling). Called on a
    /// timer by the audio thread.
    fn tick(&self);
}

#[cfg(not(target_arch = "wasm32"))]
impl PlayerCtl for Engine {
    fn play(&self, source: &str, duration_hint: Option<f64>) -> Result<(), String> {
        self.play_source(source.to_string(), duration_hint).map_err(|e| e.to_string())
    }

    fn pause(&self) {
        Engine::pause(self);
    }

    fn resume(&self) {
        Engine::resume(self);
    }

    fn stop(&self) {
        Engine::stop(self);
    }

    fn seek(&self, position: f64) -> Result<(), String> {
        Engine::seek(self, position).map_err(|e| e.to_string())
    }

    fn set_volume(&self, volume: f32) {
        Engine::set_volume(self, volume);
    }

    fn set_crossfade(&self, seconds: f32) {
        Engine::set_crossfade(self, seconds);
    }

    fn set_gapless(&self, on: bool) {
        Engine::set_gapless(self, on);
    }

    fn set_blend_skips(&self, on: bool) {
        Engine::set_blend_skips(self, on);
    }

    fn set_pause_fade(&self, on: bool) {
        Engine::set_pause_fade(self, on);
    }

    fn prepare_next(&self, source: &str, duration_hint: Option<f64>) {
        Engine::prepare_next(self, source.to_string(), duration_hint);
    }

    fn clear_next(&self) {
        Engine::clear_next(self);
    }

    fn status(&self) -> PlayerStatus {
        let s = Engine::status(self);
        PlayerStatus {
            playing: s.playing,
            paused: s.paused,
            position: s.position,
            duration: s.duration,
            volume: s.volume,
            source: s.file,
        }
    }

    fn tick(&self) {
        self.advance_tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_bounded_and_safe() {
        let mut s = PlayerStatus { position: 30.0, duration: 60.0, ..Default::default() };
        assert!((s.progress() - 0.5).abs() < f64::EPSILON);

        // Unknown duration (live transcode) must not divide by zero.
        s.duration = 0.0;
        assert_eq!(s.progress(), 0.0);

        // Position past a stale duration clamps instead of overflowing a gauge.
        s.duration = 10.0;
        s.position = 99.0;
        assert_eq!(s.progress(), 1.0);
    }

    #[test]
    fn idle_when_no_source() {
        assert!(PlayerStatus::default().is_idle());
        let s = PlayerStatus { source: "http://x/a.mp3".into(), ..Default::default() };
        assert!(!s.is_idle());
    }
}
