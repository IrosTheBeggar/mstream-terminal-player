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

/// How often a context that should be running, and is not, is asked again.
const RESUME_AGAIN: Duration = Duration::from_secs(1);

/// What to do with the audio context this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStep {
    Keep,
    Suspend,
    Resume,
}

/// Where the loop's next pass comes from (performance audit #125).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// The next animation frame: something is waiting to be shown, or the
    /// picture is moving.
    Frame,
    /// A timer this far off: the next poll tick.
    After(Duration),
}

/// When to run the next pass, once this one is done.
///
/// `urgent` is work already queued — effects to dispatch, a reply or an
/// audio event to fold in — which the next frame should act on. `animating`
/// is the visualizer drawing from the audio at the fast poll: frames keep
/// its 30 a second on the display's beat, where a timer would land them a
/// frame early or late at random. Anything else waits for the poll tick
/// the native loop would have woken at, measured from the last draw.
pub fn next_wake(urgent: bool, animating: bool, since_render: Duration, poll: Duration) -> Wake {
    if urgent || animating {
        return Wake::Frame;
    }
    match poll.checked_sub(since_render) {
        Some(left) if !left.is_zero() => Wake::After(left),
        _ => Wake::Frame,
    }
}

/// How much audio the analysers must hold. The tap is fed once a pass
/// rather than once a frame (performance audit #125), so each copy has to
/// reach back to where the last one ended, or the ring the visualizer reads
/// holds two stretches with a seam between. Passes come every 33 ms while
/// the visualizer draws, and the old 2048-frame window (43 ms at 48 kHz)
/// was one late frame away from a seam; reaching past a whole slow poll
/// leaves room for a stalled frame, and for the first pass after a pause.
const ANALYSER_REACH: Duration = Duration::from_millis(150);

/// The analysers' time-domain window for a context running at `rate`: the
/// power of two that covers [`ANALYSER_REACH`], within what Web Audio
/// allows (2048, the old fixed size, up to 32768).
pub fn analyser_window(rate: f32) -> usize {
    let frames = (f64::from(rate) * ANALYSER_REACH.as_secs_f64()).ceil() as usize;
    frames.next_power_of_two().clamp(2048, 32768)
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
    /// Whether the context has ever been seen running. One that never has
    /// is waiting for the page's first keystroke (the autoplay policy), and
    /// asking it again only puts a warning in the console.
    ran: bool,
    /// When a resume was last asked for, by anyone.
    asked: Option<Instant>,
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
    ///
    /// The same freeze follows a resume the browser turned down — the
    /// app's own ask goes out from the loop's pass, and an engine can want
    /// a gesture for it — or a context the system interrupted (a phone
    /// call). So a context that has run before and is not running under a
    /// sounding element is asked again, once a second, for as long as that
    /// lasts.
    pub fn step(&mut self, sounding: bool, running: bool, now: Instant) -> ContextStep {
        self.ran |= running;
        if sounding {
            self.since = None;
            if self.suspended {
                self.suspended = false;
                self.asked = Some(now);
                return ContextStep::Resume;
            }
            let again = self.asked.is_none_or(|at| now.duration_since(at) >= RESUME_AGAIN);
            if !running && self.ran && again {
                self.asked = Some(now);
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
    /// starts over, and the ask counts toward the once a second.
    pub fn resumed(&mut self, now: Instant) {
        self.since = None;
        self.suspended = false;
        self.asked = Some(now);
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
        idle.resumed(t0 + secs(5.05));
        // The Play was refused (autoplay), so the element is still paused:
        // the grace runs again from here rather than suspending at once, and
        // there is no suspend of ours left to undo.
        assert_eq!(idle.step(false, true, t0 + secs(5.1)), ContextStep::Keep);
        assert_eq!(idle.step(false, true, t0 + secs(10.0)), ContextStep::Keep);
        assert_eq!(idle.step(true, true, t0 + secs(10.1)), ContextStep::Keep);
    }

    #[test]
    fn a_refused_resume_is_asked_again_once_a_second() {
        // Paused past the grace, then the app's own Resume, which the
        // browser turns down: the element sounds into a suspended context.
        let t0 = Instant::now();
        let mut idle = ContextIdle::default();
        idle.step(true, true, t0);
        idle.step(false, true, t0 + secs(1.0));
        assert_eq!(idle.step(false, true, t0 + secs(6.0)), ContextStep::Suspend);
        idle.resumed(t0 + secs(10.0));
        // Not straight away: the app's own ask may still be settling.
        assert_eq!(idle.step(true, false, t0 + secs(10.0)), ContextStep::Keep);
        assert_eq!(idle.step(true, false, t0 + secs(10.5)), ContextStep::Keep);
        assert_eq!(idle.step(true, false, t0 + secs(11.0)), ContextStep::Resume);
        assert_eq!(idle.step(true, false, t0 + secs(11.5)), ContextStep::Keep);
        assert_eq!(idle.step(true, false, t0 + secs(12.0)), ContextStep::Resume);
        // Taken: nothing more to ask.
        assert_eq!(idle.step(true, true, t0 + secs(12.1)), ContextStep::Keep);
        assert_eq!(idle.step(true, true, t0 + secs(20.0)), ContextStep::Keep);
    }

    #[test]
    fn a_context_the_system_interrupted_is_asked_again() {
        // Suspended from outside, not by this clock, with the element still
        // playing.
        let t0 = Instant::now();
        let mut idle = ContextIdle::default();
        assert_eq!(idle.step(true, true, t0), ContextStep::Keep);
        assert_eq!(idle.step(true, false, t0 + secs(30.0)), ContextStep::Resume);
        assert_eq!(idle.step(true, false, t0 + secs(30.1)), ContextStep::Keep);
        assert_eq!(idle.step(true, true, t0 + secs(30.2)), ContextStep::Keep);
    }

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn at_rest_the_loop_sleeps_until_the_next_poll_tick() {
        // Just drawn: the whole interval.
        assert_eq!(next_wake(false, false, ms(0), ms(100)), Wake::After(ms(100)));
        // Woken early by something that did not draw: what is left of it.
        assert_eq!(next_wake(false, false, ms(30), ms(100)), Wake::After(ms(70)));
        // A tick that is due or overdue is not a timer of zero.
        assert_eq!(next_wake(false, false, ms(100), ms(100)), Wake::Frame);
        assert_eq!(next_wake(false, false, ms(250), ms(100)), Wake::Frame);
    }

    #[test]
    fn queued_work_and_a_moving_picture_take_the_next_frame() {
        assert_eq!(next_wake(true, false, ms(0), ms(100)), Wake::Frame);
        assert_eq!(next_wake(false, true, ms(5), ms(33)), Wake::Frame);
    }

    #[test]
    fn the_analysers_reach_back_past_a_slow_poll() {
        for rate in [22_050.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0] {
            let window = analyser_window(rate);
            assert!(window.is_power_of_two(), "{rate}");
            let reach = window as f64 / f64::from(rate);
            assert!(reach >= 0.15 || window == 32768, "{rate}: {reach}s");
        }
        assert_eq!(analyser_window(44_100.0), 8192);
        assert_eq!(analyser_window(48_000.0), 8192);
        assert_eq!(analyser_window(96_000.0), 16384);
        // Web Audio's bounds.
        assert_eq!(analyser_window(8_000.0), 2048);
        assert_eq!(analyser_window(384_000.0), 32768);
    }

    #[test]
    fn a_context_that_never_ran_is_left_alone() {
        let t0 = Instant::now();
        let mut idle = ContextIdle::default();
        assert_eq!(idle.step(false, false, t0), ContextStep::Keep);
        assert_eq!(idle.step(false, false, t0 + secs(30.0)), ContextStep::Keep);
        // Not ours to resume either, now or later.
        assert_eq!(idle.step(true, false, t0 + secs(31.0)), ContextStep::Keep);
        assert_eq!(idle.step(true, false, t0 + secs(40.0)), ContextStep::Keep);
    }
}
