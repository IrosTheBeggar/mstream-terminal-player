//! A thin control surface over whatever is actually producing sound.
//!
//! The TUI talks to playback only through this trait. Today the sole
//! implementation is the in-process rodio engine; keeping the boundary here
//! means an external backend (an mpv subprocess over JSON IPC, say) could be
//! dropped in if the engine ever runs into something it can't do — an
//! exotic codec, say (gapless and device hotplug, the original examples
//! here, it has since learned). Every terminal player surveyed for PLAN.md
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

/// News from the backend about the output device itself — playback moved
/// to a new default, lost its device, got one back. Carried as data so
/// each driver picks its own surface: the TUI's message line, serve's
/// stderr.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceNotice {
    pub text: String,
    /// True while playback is impossible (no usable output device at
    /// all); false when the notice reports a successful move or recovery.
    pub lost: bool,
}

/// What a clip's run-out does: hand main back at once, or hold main paused
/// for the next clip (Preview all). Peer-recs contract §1.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsideThen {
    /// A single press: main comes back as soon as the clip runs out.
    Restore,
    /// A chain: main stays paused for `ASIDE_HOLD` after a run-out, so the
    /// next clip can take over without main sounding in between.
    Hold,
}

/// Where the aside clip stands, as the engine sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsidePhase {
    /// The clip is opening: the request, the head start, the decode probe.
    Loading,
    Playing,
    /// Paused by the App (the band's toggle); main stays paused.
    Paused,
    /// No clip: the last one ran out under `Hold`, or the App asked to
    /// hold. Main stays paused until the next clip, a stop, or the hold's
    /// end.
    Held,
}

/// The aside clip, published every tick while a clip session lives. It is
/// kept apart from [`PlayerStatus`], which stays main's: a clip is never a
/// play, never scrobbled and never seen by the server-audio remote.
#[derive(Debug, Clone, PartialEq)]
pub struct AsideStatus {
    /// The App's name for the clip; while held, the last clip's.
    pub ticket: u64,
    pub phase: AsidePhase,
    /// Seconds of the clip handed to the device; 0 while loading or held.
    pub position: f64,
    /// The clip's length as its decoder knows it; 0 when unknown (the App
    /// draws 30 s then).
    pub duration: f64,
}

/// Why a clip ended. Every ticket the App sends gets exactly one.
#[derive(Debug, Clone, PartialEq)]
pub enum AsideEnd {
    /// It played to its end. Its answer stays kept.
    RanOut,
    /// The App asked (x, Esc, the room no longer drawn).
    Stopped,
    /// A newer clip, or a hold, took its place. The session goes on.
    Superseded,
    /// A main transport command ended it. Nothing is restored.
    Taken,
    /// The clip's own fault: it would not load, broke off, or stalled.
    /// The App forgets the stored answer for that URL.
    Failed(String),
    /// Not the clip's fault: no output device, not an http(s) URL, or a
    /// backend that plays no clips. The answer stays.
    Refused(String),
}

/// One clip's end, named by its ticket.
#[derive(Debug, Clone, PartialEq)]
pub struct AsideEnded {
    pub ticket: u64,
    pub end: AsideEnd,
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
    /// [`PlayerCtl::play`], giving the open up as soon as `superseded`
    /// answers true — a newer play or a stop has arrived, and this track is
    /// no longer wanted. Giving up is not a failure: Ok, with playback as it
    /// was. A backend whose opens cannot be abandoned just plays.
    fn play_unless(
        &self,
        source: &str,
        duration_hint: Option<f64>,
        superseded: &mut dyn FnMut() -> bool,
    ) -> Result<(), String> {
        let _ = superseded;
        self.play(source, duration_hint)
    }
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
    /// Whether nothing can change until a command arrives — stopped, or a
    /// landed pause, with nothing draining or opening — so the audio thread
    /// may tick lazily. A backend that cannot say keeps the brisk tick.
    fn settled(&self) -> bool {
        false
    }
    /// Device news since the last call, oldest first. Backends that
    /// cannot lose an output device (the browser's) have none.
    fn take_device_notices(&self) -> Vec<DeviceNotice> {
        Vec::new()
    }
    /// Play `url` aside of main, as the clip named `ticket`: a preview that
    /// pauses main once it has opened, and gives main back when it ends.
    /// The open never blocks; its fate arrives through
    /// [`PlayerCtl::take_aside_ends`]. An `Err` is a refusal, the backend's
    /// fault and not the clip's. A backend without clips refuses them all.
    fn aside_play(&self, ticket: u64, url: &str, then: AsideThen) -> Result<(), String> {
        let _ = (ticket, url, then);
        Err("this player plays no previews".into())
    }
    /// Pause the sounding clip named `ticket`; main stays paused.
    fn aside_pause(&self, ticket: u64) {
        let _ = ticket;
    }
    /// Resume the paused clip named `ticket`.
    fn aside_resume(&self, ticket: u64) {
        let _ = ticket;
    }
    /// End the clip session: `None` whatever runs, `Some` only that
    /// ticket's. Main comes back when `restore` and the session paused it.
    fn aside_stop(&self, ticket: Option<u64>, restore: bool) {
        let _ = (ticket, restore);
    }
    /// End the clip in hand and hold main paused for `span` (capped by the
    /// backend), while the App resolves the next clip.
    fn aside_hold(&self, span: std::time::Duration) {
        let _ = span;
    }
    /// The clip session as it stands; `None` without one.
    fn aside_status(&self) -> Option<AsideStatus> {
        None
    }
    /// Clip ends since the last call, oldest first.
    fn take_aside_ends(&self) -> Vec<AsideEnded> {
        Vec::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl PlayerCtl for Engine {
    fn play(&self, source: &str, duration_hint: Option<f64>) -> Result<(), String> {
        self.play_source(source.to_string(), duration_hint).map_err(|e| e.to_string())
    }

    fn play_unless(
        &self,
        source: &str,
        duration_hint: Option<f64>,
        superseded: &mut dyn FnMut() -> bool,
    ) -> Result<(), String> {
        match self.play_source_unless(source.to_string(), duration_hint, superseded) {
            Ok(()) | Err(crate::engine::EngineError::Superseded) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
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

    fn settled(&self) -> bool {
        Engine::settled(self)
    }

    fn take_device_notices(&self) -> Vec<DeviceNotice> {
        Engine::take_device_notices(self)
    }

    fn aside_play(&self, ticket: u64, url: &str, then: AsideThen) -> Result<(), String> {
        Engine::play_aside(self, ticket, url, then)
    }

    fn aside_pause(&self, ticket: u64) {
        Engine::pause_aside(self, ticket);
    }

    fn aside_resume(&self, ticket: u64) {
        Engine::resume_aside(self, ticket);
    }

    fn aside_stop(&self, ticket: Option<u64>, restore: bool) {
        Engine::stop_aside(self, ticket, restore);
    }

    fn aside_hold(&self, span: std::time::Duration) {
        Engine::hold_aside(self, span);
    }

    fn aside_status(&self) -> Option<AsideStatus> {
        Engine::aside_status(self)
    }

    fn take_aside_ends(&self) -> Vec<AsideEnded> {
        Engine::take_aside_ends(self)
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

    /// A backend that knows only main playback, as the browser's and the
    /// worker's fakes do.
    struct MainOnly;

    impl PlayerCtl for MainOnly {
        fn play(&self, _: &str, _: Option<f64>) -> Result<(), String> {
            Ok(())
        }
        fn pause(&self) {}
        fn resume(&self) {}
        fn stop(&self) {}
        fn seek(&self, _: f64) -> Result<(), String> {
            Ok(())
        }
        fn set_volume(&self, _: f32) {}
        fn set_crossfade(&self, _: f32) {}
        fn set_gapless(&self, _: bool) {}
        fn set_blend_skips(&self, _: bool) {}
        fn set_pause_fade(&self, _: bool) {}
        fn prepare_next(&self, _: &str, _: Option<f64>) {}
        fn clear_next(&self) {}
        fn status(&self) -> PlayerStatus {
            PlayerStatus::default()
        }
        fn tick(&self) {}
    }

    #[test]
    fn a_backend_without_clips_refuses_them_and_has_none_to_tell() {
        let player = MainOnly;
        let refused = player.aside_play(1, "https://cdn.example/clip.mp3", AsideThen::Restore);
        assert_eq!(refused, Err("this player plays no previews".to_string()));
        player.aside_pause(1);
        player.aside_resume(1);
        player.aside_hold(std::time::Duration::from_secs(5));
        player.aside_stop(None, true);
        assert_eq!(player.aside_status(), None);
        assert_eq!(player.take_aside_ends(), []);
    }

    #[test]
    fn idle_when_no_source() {
        assert!(PlayerStatus::default().is_idle());
        let s = PlayerStatus { source: "http://x/a.mp3".into(), ..Default::default() };
        assert!(!s.is_idle());
    }
}
