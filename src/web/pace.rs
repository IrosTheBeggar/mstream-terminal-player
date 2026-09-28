//! When the browser shell spends power: the decisions behind its timers,
//! with nothing of the browser in them.
//!
//! Pure on purpose. Everything else under src/web builds only for wasm32,
//! where `cargo test` does not run; this module is also compiled natively
//! (main.rs) so the rules have tests.

use std::time::Duration;

use crate::clock::Instant;

/// How long nothing has to sound before the audio context is suspended.
/// Longer than any gap the app leaves between tracks — a track's end and
/// the next Play are one pass of the loop apart — so an advancing queue
/// never parks the context; short enough that a paused tab lets the
/// machine sleep soon after.
pub const CONTEXT_IDLE: Duration = Duration::from_secs(5);

/// What to do with the audio context this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStep {
    Keep,
    Suspend,
    Resume,
}

/// The audio context's idle clock (performance audit #123).
///
/// A running AudioContext keeps the browser's audio output open — and on a
/// Mac, keeps the machine from idle-sleeping — whether or not anything is
/// playing through it. The element pausing does not close it, and neither
/// does silence: the analysers the visualizer reads are pull nodes, which
/// switch Chromium's silent-sink detection off. So the shell suspends it
/// itself once nothing has sounded for [`CONTEXT_IDLE`], and resumes it the
/// moment the element really plays again.
#[derive(Debug, Default)]
pub struct ContextIdle {
    /// When the element stopped sounding; `None` while it sounds.
    since: Option<Instant>,
    /// Whether this clock suspended the context. `suspend()` is a promise,
    /// and the context reads "running" until it settles — the flag keeps
    /// that to one call rather than one a pass.
    suspended: bool,
}

impl ContextIdle {
    /// One pass: `sounding` is the element's own word (not paused, not
    /// ended, no error), `running` the context's state.
    ///
    /// Sounding while suspended resumes whatever started it. The app's own
    /// Play and Resume already asked (see [`ContextIdle::resumed`]); this
    /// catches the element being started from outside the app — a media
    /// key, or the OS's Now Playing controls — which would otherwise play
    /// into a suspended graph: the promise resolves, the position freezes,
    /// and nothing sounds.
    pub fn step(&mut self, sounding: bool, running: bool, now: Instant) -> ContextStep {
        if sounding {
            self.since = None;
            if self.suspended {
                self.suspended = false;
                return ContextStep::Resume;
            }
            return ContextStep::Keep;
        }
        let since = *self.since.get_or_insert(now);
        // A context that is not running was never started (the autoplay
        // policy) or is already asleep: nothing to suspend.
        if !self.suspended && running && now.duration_since(since) >= CONTEXT_IDLE {
            self.suspended = true;
            return ContextStep::Suspend;
        }
        ContextStep::Keep
    }

    /// The app asked the context to resume (Play, Resume): the idle clock
    /// starts over.
    pub fn resumed(&mut self) {
        self.since = None;
        self.suspended = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn a_short_pause_keeps_the_context_running() {
        let t0 = Instant::now();
        let mut idle = ContextIdle::default();
        assert_eq!(idle.step(true, true, t0), ContextStep::Keep);
        // Paused for just under the grace, then playing again.
        assert_eq!(idle.step(false, true, t0 + secs(1.0)), ContextStep::Keep);
        assert_eq!(idle.step(false, true, t0 + secs(5.9)), ContextStep::Keep);
        assert_eq!(idle.step(true, true, t0 + secs(6.0)), ContextStep::Keep);
        // The clock started over: another short pause is short again.
        assert_eq!(idle.step(false, true, t0 + secs(7.0)), ContextStep::Keep);
        assert_eq!(idle.step(false, true, t0 + secs(11.9)), ContextStep::Keep);
    }

    #[test]
    fn a_long_pause_suspends_once_and_playing_again_resumes() {
        let t0 = Instant::now();
        let mut idle = ContextIdle::default();
        assert_eq!(idle.step(false, true, t0), ContextStep::Keep);
        assert_eq!(idle.step(false, true, t0 + CONTEXT_IDLE), ContextStep::Suspend);
        // The promise has not settled and the context still reads running:
        // no second call.
        assert_eq!(idle.step(false, true, t0 + CONTEXT_IDLE + secs(0.1)), ContextStep::Keep);
        assert_eq!(idle.step(false, false, t0 + secs(60.0)), ContextStep::Keep);
        // Started from outside the app: the context wakes with it.
        assert_eq!(idle.step(true, false, t0 + secs(61.0)), ContextStep::Resume);
        assert_eq!(idle.step(true, true, t0 + secs(61.1)), ContextStep::Keep);
    }

    #[test]
    fn the_apps_own_resume_restarts_the_clock() {
        let t0 = Instant::now();
        let mut idle = ContextIdle::default();
        idle.step(false, true, t0);
        assert_eq!(idle.step(false, true, t0 + secs(5.0)), ContextStep::Suspend);
        idle.resumed();
        // The Play was refused (autoplay), so the element is still paused:
        // the grace runs again from here rather than suspending at once, and
        // there is no suspend of ours left to undo.
        assert_eq!(idle.step(false, true, t0 + secs(5.1)), ContextStep::Keep);
        assert_eq!(idle.step(false, true, t0 + secs(10.0)), ContextStep::Keep);
        assert_eq!(idle.step(true, true, t0 + secs(10.1)), ContextStep::Keep);
    }

    #[test]
    fn a_context_that_never_ran_is_left_alone() {
        let t0 = Instant::now();
        let mut idle = ContextIdle::default();
        assert_eq!(idle.step(false, false, t0), ContextStep::Keep);
        assert_eq!(idle.step(false, false, t0 + secs(30.0)), ContextStep::Keep);
        // Not ours to resume either.
        assert_eq!(idle.step(true, false, t0 + secs(31.0)), ContextStep::Keep);
    }
}
