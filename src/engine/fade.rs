//! An equal-power gain ramp inside the source chain, commanded from outside it.
//!
//! A crossfade is two sinks audible at once with opposing gains, and neither of
//! rodio's tools fits the job: its `crossfade()` helper returns only the
//! overlapped portion of a pre-composed pair, severing seek, pause and position
//! reporting; and ramping `Player::set_volume` from the tick loop would step the
//! gain at whole ticks — a staircase you can hear on anything sustained. So the
//! fade is applied where the samples flow, one multiply at a time.
//!
//! [`Faded`] advances its ramp per *frame consumed*, not per wall-clock second.
//! Paused playback pulls no samples, so a half-finished blend freezes exactly
//! where it sits and resumes intact, with nothing asked of the engine. The
//! engine speaks to the wrapper through [`FadeHandle`] — a few atomics, never a
//! lock, because this sits on the path that feeds the device, where waiting is
//! a gap in the sound (the same rule as [`super::tap`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// The engine's end of a [`Faded`] source. Cheap to clone, safe to command
/// from any thread; the source obeys at its next frame boundary.
#[derive(Debug)]
pub(crate) struct FadeHandle {
    /// f32 bits of where the ramp is headed, 0.0..=1.0.
    target: AtomicU32,
    /// How long the walk there takes. Milliseconds rather than a Duration so
    /// it fits in an atomic beside the others; nobody blends for 49 days.
    millis: AtomicU32,
    /// Bumped on every command. The source polls this — one relaxed-ish load
    /// per frame — and a changed number means new orders. A command's fields
    /// and its bump are not one atom, so a reader racing two commands can act
    /// on a blend of both for one frame; the next boundary re-reads and
    /// converges, and a frame of slightly-wrong gain is nothing.
    /// (`generation` spelled out: `gen` became a reserved word in 2024.)
    generation: AtomicU32,
    /// f32 bits of the ramp position the source last reported. How the engine
    /// sees that an outgoing track has finished going quiet.
    current: AtomicU32,
}

impl FadeHandle {
    pub fn new(initial: f32) -> Arc<Self> {
        let initial = clamp_unit(initial);
        Arc::new(FadeHandle {
            target: AtomicU32::new(initial.to_bits()),
            millis: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            current: AtomicU32::new(initial.to_bits()),
        })
    }

    /// Walk the gain to `target` over `over`, starting from wherever the ramp
    /// stands right now — so re-commanding mid-walk bends the ramp rather
    /// than snapping it.
    pub fn ramp_to(&self, target: f32, over: Duration) {
        self.target.store(clamp_unit(target).to_bits(), Ordering::Relaxed);
        self.millis.store(over.as_millis().min(u32::MAX as u128) as u32, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Jump the gain immediately. What a seek wants: the blend it interrupts
    /// should not audibly finish.
    pub fn snap(&self, value: f32) {
        self.ramp_to(value, Duration::ZERO);
    }

    /// Where the ramp stands, 0.0..=1.0, as of the last frame the source
    /// produced. A paused source reports the position it froze at.
    pub fn position(&self) -> f32 {
        f32::from_bits(self.current.load(Ordering::Acquire))
    }
}

/// NaN-proof clamp to 0..=1; a NaN command decays to silence rather than
/// poisoning every sample multiplied through it.
fn clamp_unit(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 }
}

/// Equal-power law: gain = sin(p·π/2). Two sides of a blend running p and
/// 1−p sit at sin and cos of the same angle, so the summed *power* holds
/// flat — linear amplitude would dip 3 dB at the middle of every blend.
fn curve(p: f32) -> f32 {
    (clamp_unit(p) * std::f32::consts::FRAC_PI_2).sin()
}

/// A source whose amplitude is walked between 0 and 1 on command.
pub(crate) struct Faded<S> {
    inner: S,
    handle: Arc<FadeHandle>,
    /// Ramp position, 0..=1. The gain is [`curve`] of this.
    p: f32,
    target: f32,
    /// Per-frame movement toward `target`; 0.0 once settled.
    step: f32,
    /// Frames until the ramp lands. Counted rather than inferred from `p`:
    /// a hundred float additions drift, and the last frame must land on
    /// `target` exactly, not within epsilon of it.
    frames_left: u64,
    /// Applied to every sample; recomputed only while the ramp moves.
    gain: f32,
    /// The command generation last obeyed. Starts deliberately impossible so
    /// the first frame reads the handle: the engine builds the handle first
    /// and the source a blocking open() later, and orders given in between
    /// must not be lost.
    seen: u32,
    /// Samples left in the current frame. The ramp steps once per frame, not
    /// per sample — per-sample stepping would hand each channel of a frame a
    /// slightly different gain, tilting the stereo image by a hair for the
    /// length of every blend.
    frame_rem: u16,
}

impl<S: Source> Faded<S> {
    pub fn new(inner: S, handle: Arc<FadeHandle>) -> Self {
        let p = f32::from_bits(handle.current.load(Ordering::Acquire));
        Faded {
            inner,
            handle,
            p,
            target: p,
            step: 0.0,
            frames_left: 0,
            gain: curve(p),
            seen: u32::MAX,
            frame_rem: 0,
        }
    }

    /// Pick up any new command. Called at frame boundaries only.
    fn sync(&mut self) {
        let generation = self.handle.generation.load(Ordering::Acquire);
        if generation == self.seen {
            return;
        }
        self.seen = generation;
        self.target = clamp_unit(f32::from_bits(self.handle.target.load(Ordering::Relaxed)));
        let millis = self.handle.millis.load(Ordering::Relaxed);
        let frames = self.inner.sample_rate().get() as u64 * millis as u64 / 1000;
        if frames == 0 {
            self.p = self.target;
            self.step = 0.0;
            self.frames_left = 0;
            self.gain = curve(self.p);
            self.handle.current.store(self.p.to_bits(), Ordering::Release);
        } else {
            self.step = (self.target - self.p) / frames as f32;
            self.frames_left = frames;
        }
    }

    /// Move the ramp one frame's worth, if it is moving at all.
    fn advance(&mut self) {
        if self.frames_left == 0 {
            return;
        }
        self.frames_left -= 1;
        if self.frames_left == 0 {
            self.p = self.target;
            self.step = 0.0;
        } else {
            self.p += self.step;
        }
        self.gain = curve(self.p);
        self.handle.current.store(self.p.to_bits(), Ordering::Release);
    }
}

impl<S: Source> Iterator for Faded<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        if self.frame_rem == 0 {
            self.frame_rem = self.inner.channels().get();
            self.sync();
            self.advance();
        }
        self.frame_rem -= 1;
        Some(sample * self.gain)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for Faded<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// Forwarded untouched — the same lesson as [`super::tap`]: the default
    /// impl refuses, and a wrapper that forgets this makes everything behind
    /// it unseekable. The ramp itself is deliberately left alone: what a
    /// seek means for a blend in progress is the engine's policy, and it
    /// says so through the handle.
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source of constant full-scale samples, so whatever comes out of the
    /// wrapper *is* the gain that was applied to it.
    struct Steady {
        rate: u32,
        channels: u16,
    }

    impl Iterator for Steady {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            Some(1.0)
        }
    }

    impl Source for Steady {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> ChannelCount {
            ChannelCount::new(self.channels).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            SampleRate::new(self.rate).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
        fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
            Ok(())
        }
    }

    /// 1 kHz mono: one frame per sample, and a 100 ms ramp is exactly 100
    /// frames — the arithmetic in these tests stays legible.
    fn mono() -> Steady {
        Steady { rate: 1000, channels: 1 }
    }

    fn take(faded: &mut Faded<Steady>, count: usize) -> Vec<Sample> {
        (0..count).map(|_| faded.next().unwrap()).collect()
    }

    #[test]
    fn a_settled_fade_is_a_constant_multiply() {
        let mut full = Faded::new(mono(), FadeHandle::new(1.0));
        assert!(take(&mut full, 8).iter().all(|&s| s == 1.0), "unity passes samples unchanged");

        let mut silent = Faded::new(mono(), FadeHandle::new(0.0));
        assert!(take(&mut silent, 8).iter().all(|&s| s == 0.0));

        // The half-way point of the *ramp* is the equal-power point of the
        // gain: sin(π/4), not 0.5. This is the curve being the curve.
        let mut half = Faded::new(mono(), FadeHandle::new(0.5));
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!(take(&mut half, 4).iter().all(|&s| (s - expected).abs() < 1e-6));
    }

    #[test]
    fn a_ramp_arrives_on_time_and_reports_where_it_stands() {
        let handle = FadeHandle::new(1.0);
        let mut faded = Faded::new(mono(), handle.clone());
        take(&mut faded, 5);

        handle.ramp_to(0.0, Duration::from_millis(100));
        let heard = take(&mut faded, 100);
        assert!(heard.windows(2).all(|w| w[1] < w[0]), "downward the whole way");
        // Half-way down the walk the gain passes through equal power.
        assert!((heard[49] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02, "{}", heard[49]);
        assert_eq!(*heard.last().unwrap(), 0.0, "the 100th frame lands exactly at silence");
        assert_eq!(handle.position(), 0.0, "and the handle heard about it");

        assert!(take(&mut faded, 10).iter().all(|&s| s == 0.0), "settled means settled");
    }

    #[test]
    fn stereo_frames_share_one_gain() {
        let handle = FadeHandle::new(1.0);
        let mut faded = Faded::new(Steady { rate: 1000, channels: 2 }, handle.clone());
        handle.ramp_to(0.0, Duration::from_millis(50));
        let heard: Vec<Sample> = (0..100).map(|_| faded.next().unwrap()).collect();
        for frame in heard.chunks_exact(2) {
            assert_eq!(frame[0], frame[1], "left and right move together");
        }
        assert!(heard[0] > heard[2], "and the ramp still ramps, frame to frame");
    }

    #[test]
    fn a_new_command_bends_the_ramp_from_where_it_stands() {
        let handle = FadeHandle::new(1.0);
        let mut faded = Faded::new(mono(), handle.clone());
        handle.ramp_to(0.0, Duration::from_millis(100));
        take(&mut faded, 50);
        let stood = handle.position();
        assert!((stood - 0.5).abs() < 0.02, "half the walk walked: {stood}");

        // Turn around mid-descent: the climb starts at 0.5, not at 0.
        handle.ramp_to(1.0, Duration::from_millis(50));
        let heard = take(&mut faded, 50);
        assert!(heard[0] > curve(0.4), "no dip to the floor on the way back up");
        assert!(heard.windows(2).all(|w| w[1] >= w[0]), "upward the whole way");
        assert_eq!(*heard.last().unwrap(), 1.0);
    }

    #[test]
    fn a_zero_length_ramp_is_a_jump() {
        let handle = FadeHandle::new(1.0);
        let mut faded = Faded::new(mono(), handle.clone());
        handle.snap(0.0);
        assert_eq!(faded.next().unwrap(), 0.0, "silent from the very first sample");
        assert_eq!(handle.position(), 0.0);
    }

    #[test]
    fn orders_given_before_the_source_exists_are_not_lost() {
        // The engine's actual sequence: handle first, then a blocking open,
        // then the source. A ramp commanded during the open must be standing
        // orders by the first sample, not a lost letter.
        let handle = FadeHandle::new(0.0);
        handle.ramp_to(1.0, Duration::ZERO);
        let mut faded = Faded::new(mono(), handle.clone());
        assert_eq!(faded.next().unwrap(), 1.0, "the pre-birth command was obeyed");
    }

    #[test]
    fn nonsense_commands_decay_to_silence_not_poison() {
        let handle = FadeHandle::new(1.0);
        let mut faded = Faded::new(mono(), handle.clone());
        handle.snap(f32::NAN);
        // A NaN gain would turn every sample after it into NaN — full-scale
        // noise on many DACs. Silence is the only safe reading.
        assert_eq!(faded.next().unwrap(), 0.0);
        handle.snap(7.0);
        assert_eq!(faded.next().unwrap(), 1.0, "over-unity clamps to unity");
    }

    /// A source that halves its sample rate partway through, the way a
    /// decoder's spans legally can.
    struct Shifty {
        emitted: usize,
        switch_at: usize,
    }

    impl Iterator for Shifty {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            self.emitted += 1;
            Some(1.0)
        }
    }

    impl Source for Shifty {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> ChannelCount {
            ChannelCount::new(1).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            SampleRate::new(if self.emitted < self.switch_at { 1000 } else { 500 }).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
        fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
            Ok(())
        }
    }

    #[test]
    fn a_ramp_survives_a_mid_stream_rate_change() {
        // The ramp is a count of frames, fixed when the command is picked
        // up: a rate change mid-walk stretches its wall-clock time (100
        // frames at half the rate take twice as long) but the walk still
        // lands exactly on target, monotonically — no stall, no overshoot,
        // no divide by the new rate. That wall-time stretch is the
        // documented trade; this pins the landing.
        let handle = FadeHandle::new(1.0);
        let mut faded = Faded::new(Shifty { emitted: 0, switch_at: 50 }, handle.clone());
        handle.ramp_to(0.0, Duration::from_millis(100)); // 100 frames at 1kHz
        let heard = take_shifty(&mut faded, 100);
        assert!(heard.windows(2).all(|w| w[1] < w[0]), "downward the whole way through");
        assert_eq!(*heard.last().unwrap(), 0.0, "and it lands exactly, whatever the rate did");
        assert_eq!(handle.position(), 0.0);
    }

    fn take_shifty(faded: &mut Faded<Shifty>, count: usize) -> Vec<Sample> {
        (0..count).map(|_| faded.next().unwrap()).collect()
    }

    #[test]
    fn a_seek_reaches_the_inner_source_and_leaves_the_ramp_alone() {
        let handle = FadeHandle::new(0.5);
        let mut faded = Faded::new(mono(), handle.clone());
        take(&mut faded, 4);
        faded.try_seek(Duration::from_secs(1)).expect("forwarded to the source");
        let expected = curve(0.5);
        assert_eq!(faded.next().unwrap(), expected, "gain policy on seek belongs to the engine");
    }
}
