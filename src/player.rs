//! A thin control surface over whatever is actually producing sound.
//!
//! The TUI talks to playback only through this trait. Today the sole
//! implementation is the in-process rodio engine; keeping the boundary here
//! means an external backend (an mpv subprocess over JSON IPC, say) could be
//! dropped in if the engine ever runs into something it can't do — gapless,
//! device hotplug, exotic codecs. Every terminal player surveyed for PLAN.md
//! skipped this abstraction and wished it hadn't.

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
    fn status(&self) -> PlayerStatus;
    /// Drive any background bookkeeping (end-of-track handling). Called on a
    /// timer by the audio thread.
    fn tick(&self);
}

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
