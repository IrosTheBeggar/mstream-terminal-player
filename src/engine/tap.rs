//! A copy of what is being played, for anything that wants to draw it.
//!
//! The engine hands its decoded samples straight to the sink, so nothing else
//! ever sees them. [`Tapped`] sits in that path and keeps the most recent few
//! thousand in a ring, which the UI reads on its own schedule.
//!
//! The one rule: **this must never make the audio thread wait.** It runs on
//! the path that feeds the device, where a stall is a gap in the sound. So
//! the lock is only ever tried, never taken — a batch that arrives while the
//! UI is reading is dropped, which costs one frame of a visualizer and
//! nothing else.
//!
//! Each [`Tapped`] also carries a switch. During a crossfade two sources play
//! at once, and two of them pushing one ring would interleave their batches
//! into garbage — so at handover the engine flips the outgoing source's
//! switch off, and the ring belongs to the track you are moving into.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use rodio::source::SeekError;
#[cfg(not(target_arch = "wasm32"))]
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// rodio's sample type, for the build that has no rodio. The tap is plain
/// data on wasm — and f32 is what rodio 0.22's `Sample` already is, so the
/// frames are the same shape on both targets.
#[cfg(target_arch = "wasm32")]
pub type Sample = f32;

/// How much audio the tap holds, in frames — one sample per channel, so the
/// stretch of time held does not depend on how many channels the source has.
/// Sized in interleaved samples it did: 7.1 kept an eighth of what mono kept,
/// permanently short of one FFT window. Two windows' worth — enough to draw
/// from, short enough that what it holds is what you are hearing.
pub const TAP_FRAMES: usize = 4096;

/// How many samples a source gathers before handing them over. Taking the
/// lock per sample would be ninety thousand times a second for no benefit;
/// this makes it about twenty.
#[cfg(not(target_arch = "wasm32"))]
const BATCH: usize = 2048;

/// The most recent audio, interleaved exactly as it went to the device.
#[derive(Debug, Default)]
pub struct AudioTap {
    ring: Mutex<Ring>,
}

impl AudioTap {
    pub fn new() -> Arc<Self> {
        Arc::new(AudioTap::default())
    }

    /// From the audio thread — or, on wasm, from the stub that synthesises a
    /// signal for the visualizer. Never waits — see the module note.
    pub(crate) fn push(&self, batch: &[Sample], rate: u32, channels: u16) {
        if let Ok(mut ring) = self.ring.try_lock() {
            ring.push(batch, rate, channels);
        }
    }

    /// From the UI thread. `false` means either nothing has played yet or
    /// the audio thread is mid-handover; both say "ask again next frame".
    ///
    /// Fills a frame the caller keeps. The answer is the same size every
    /// time and the visualiser asks thirty times a second, so what used to
    /// be a fresh ~32 KB vec per frame is a copy into the same allocation.
    pub fn frame_into(&self, out: &mut TapFrame) -> bool {
        let Ok(ring) = self.ring.try_lock() else { return false };
        ring.frame_into(out)
    }

    /// The same, allocating — for tests that want the answer once, like
    /// [`TapFrame::mono`] beside it.
    #[cfg(test)]
    pub fn frame(&self) -> Option<TapFrame> {
        let mut out = TapFrame::default();
        self.frame_into(&mut out).then_some(out)
    }

    /// Everything held describes where we no longer are.
    pub fn clear(&self) {
        if let Ok(mut ring) = self.ring.try_lock() {
            ring.reset();
        }
    }
}

/// A copy of the tap's contents, oldest sample first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TapFrame {
    /// Interleaved by channel, as it was played.
    pub samples: Vec<Sample>,
    pub rate: u32,
    pub channels: u16,
}

impl TapFrame {
    /// One value per frame, channels averaged. What a waveform or an FFT
    /// wants; the vectorscope is the one that needs them kept apart.
    /// Allocating, for tests and anyone who wants the answer once. The
    /// visualiser is the hot caller and it keeps its own buffer.
    #[cfg(test)]
    pub fn mono(&self) -> Vec<Sample> {
        let mut out = Vec::new();
        self.mono_into(&mut out);
        out
    }

    /// The same, into a buffer the caller keeps. The visualiser asks thirty
    /// times a second and the answer is the same size every time, so the
    /// allocation is worth not making.
    pub fn mono_into(&self, out: &mut Vec<Sample>) {
        out.clear();
        let channels = self.channels.max(1) as usize;
        if channels == 1 {
            out.extend_from_slice(&self.samples);
            return;
        }
        out.extend(
            self.samples
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<Sample>() / channels as Sample),
        );
    }

}

#[derive(Debug, Default)]
struct Ring {
    samples: Vec<Sample>,
    /// Where the next sample goes. Once this has wrapped the buffer is full,
    /// which `wrapped` records — without it a half-filled ring reads as
    /// several thousand samples of silence.
    write: usize,
    wrapped: bool,
    rate: u32,
    channels: u16,
}

impl Ring {
    fn push(&mut self, batch: &[Sample], rate: u32, channels: u16) {
        // A source of a different shape: what is in here describes the old
        // one, and interleaving two channel counts together is nonsense.
        if self.rate != rate || self.channels != channels {
            self.reset();
            self.rate = rate;
            self.channels = channels;
        }
        let want = TAP_FRAMES * channels.max(1) as usize;
        if self.samples.len() != want {
            self.samples = vec![0.0; want];
        }
        for &sample in batch {
            self.samples[self.write] = sample;
            self.write += 1;
            if self.write == self.samples.len() {
                self.write = 0;
                self.wrapped = true;
            }
        }
    }

    fn frame_into(&self, out: &mut TapFrame) -> bool {
        if self.samples.is_empty() || (!self.wrapped && self.write == 0) {
            return false;
        }
        out.samples.clear();
        if self.wrapped {
            out.samples.extend_from_slice(&self.samples[self.write..]);
        }
        out.samples.extend_from_slice(&self.samples[..self.write]);
        out.rate = self.rate;
        out.channels = self.channels;
        true
    }

    fn reset(&mut self) {
        self.write = 0;
        self.wrapped = false;
        self.samples.fill(0.0);
    }
}

/// A source that keeps a copy of everything passing through it.
#[cfg(not(target_arch = "wasm32"))]
pub struct Tapped<S> {
    inner: S,
    tap: Arc<AudioTap>,
    /// While false the samples still flow, but none are copied — the shape
    /// of a source that has been retired into the outgoing half of a blend.
    live: Arc<AtomicBool>,
    batch: Vec<Sample>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<S> Tapped<S> {
    pub fn new(inner: S, tap: Arc<AudioTap>, live: Arc<AtomicBool>) -> Self {
        Tapped { inner, tap, live, batch: Vec::with_capacity(BATCH) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S: Source> Iterator for Tapped<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        if self.live.load(Ordering::Relaxed) {
            self.batch.push(sample);
            if self.batch.len() >= BATCH {
                let rate = self.inner.sample_rate().get();
                let channels = self.inner.channels().get();
                self.tap.push(&self.batch, rate, channels);
                self.batch.clear();
            }
        }
        Some(sample)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S: Source> Source for Tapped<S> {
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

    /// Forwarded, and then some.
    ///
    /// `Source::try_seek` defaults to refusing, so a wrapper that forgets it
    /// makes everything behind it unseekable — silently, since the engine
    /// reports the error as a failed seek rather than a missing forward. What
    /// is buffered is from before the seek, so it goes with it.
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let sought = self.inner.try_seek(pos);
        if sought.is_ok() {
            self.batch.clear();
            self.tap.clear();
        }
        sought
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source of counting samples, so what comes out of the tap can be
    /// checked against what went in.
    struct Ramp {
        next: u32,
        rate: u32,
        channels: u16,
    }

    impl Ramp {
        fn new(rate: u32, channels: u16) -> Self {
            Ramp { next: 0, rate, channels }
        }
    }

    impl Iterator for Ramp {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            self.next += 1;
            Some(self.next as Sample)
        }
    }

    impl Source for Ramp {
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
            self.next = 0;
            Ok(())
        }
    }

    fn drain<S: Source>(source: &mut S, count: usize) {
        for _ in 0..count {
            source.next();
        }
    }

    /// A switch in its factory position. Tests that throw it get their own.
    fn live() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(true))
    }

    #[test]
    fn a_retired_source_stops_feeding_the_ring_but_keeps_sounding() {
        let tap = AudioTap::new();
        let switch = live();
        let mut source = Tapped::new(Ramp::new(44100, 2), tap.clone(), switch.clone());
        drain(&mut source, BATCH);
        let before = tap.frame().expect("live and pushing");

        // Thrown mid-flight, the way a crossfade handover throws it.
        switch.store(false, Ordering::Relaxed);
        assert_eq!(source.next(), Some((BATCH + 1) as Sample), "the audio itself still flows");
        drain(&mut source, BATCH * 2);
        assert_eq!(tap.frame().unwrap(), before, "but the ring stopped hearing it");
    }

    #[test]
    fn the_tap_holds_the_most_recent_audio_oldest_first() {
        let tap = AudioTap::new();
        let mut source = Tapped::new(Ramp::new(44100, 2), tap.clone(), live());

        // Nothing handed over yet: a batch at a time, so the first samples
        // are still in the source.
        drain(&mut source, BATCH - 1);
        assert!(tap.frame().is_none(), "nothing to draw before the first batch");

        drain(&mut source, 1);
        let frame = tap.frame().expect("the first batch");
        assert_eq!(frame.samples.len(), BATCH);
        assert_eq!(frame.rate, 44100);
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.samples[0], 1.0, "oldest first");
        assert_eq!(frame.samples[BATCH - 1], BATCH as Sample);

        // Past the ring's length it keeps the newest and drops the oldest.
        let held = TAP_FRAMES * 2; // interleaved, and this source is stereo
        drain(&mut source, held * 2);
        let frame = tap.frame().unwrap();
        assert_eq!(frame.samples.len(), held);
        let newest = frame.samples[held - 1];
        assert_eq!(frame.samples[0], newest - (held - 1) as Sample, "one run, in order");
    }

    #[test]
    fn a_seek_is_forwarded_and_takes_the_stale_audio_with_it() {
        let tap = AudioTap::new();
        let mut source = Tapped::new(Ramp::new(44100, 2), tap.clone(), live());
        drain(&mut source, BATCH * 2);
        assert!(tap.frame().is_some());

        // Forwarded: the default impl refuses, so reaching the Ramp at all is
        // the thing being proved.
        source.try_seek(Duration::from_secs(1)).expect("forwarded to the source");
        assert!(tap.frame().is_none(), "what was held was from before the seek");

        drain(&mut source, BATCH);
        assert_eq!(tap.frame().unwrap().samples[0], 1.0, "and it fills again from there");
    }

    #[test]
    fn the_ring_spans_the_same_frames_however_many_channels() {
        // Sized in interleaved samples, a 7.1 source kept an eighth of the
        // frames a mono one did — permanently too few for one FFT window,
        // however long it played.
        let frames_held = |channels: u16| {
            let tap = AudioTap::new();
            let mut source = Tapped::new(Ramp::new(44100, channels), tap.clone(), live());
            drain(&mut source, 8 * 8192);
            tap.frame().expect("a full ring").mono().len()
        };
        assert_eq!(frames_held(8), frames_held(1), "7.1 holds the frames mono holds");
    }

    #[test]
    fn refilling_a_kept_frame_reuses_its_allocation() {
        let tap = AudioTap::new();
        let mut source = Tapped::new(Ramp::new(44100, 2), tap.clone(), live());
        drain(&mut source, TAP_FRAMES * 4);

        let mut kept = TapFrame::default();
        assert!(tap.frame_into(&mut kept));
        let capacity = kept.samples.capacity();

        drain(&mut source, BATCH);
        assert!(tap.frame_into(&mut kept));
        assert_eq!(kept.samples.capacity(), capacity, "the second fill reuses the buffer");
        assert_eq!(Some(kept.clone()), tap.frame(), "and answers what frame() answers");
    }

    #[test]
    fn a_source_of_a_different_shape_does_not_get_interleaved_with_the_last() {
        let tap = AudioTap::new();
        let mut stereo = Tapped::new(Ramp::new(44100, 2), tap.clone(), live());
        drain(&mut stereo, BATCH);
        assert_eq!(tap.frame().unwrap().channels, 2);

        let mut mono = Tapped::new(Ramp::new(48000, 1), tap.clone(), live());
        drain(&mut mono, BATCH);
        let frame = tap.frame().unwrap();
        assert_eq!((frame.rate, frame.channels), (48000, 1));
        assert_eq!(frame.samples.len(), BATCH, "the stereo audio went with its shape");
    }

    #[test]
    fn mono_averages_the_channels() {
        let frame = TapFrame {
            samples: vec![1.0, 0.0, -0.5, -0.5, 0.25, 0.75],
            rate: 44100,
            channels: 2,
        };
        assert_eq!(frame.mono(), vec![0.5, -0.5, 0.5]);

        let mono = TapFrame { samples: vec![0.1, -0.9], rate: 44100, channels: 1 };
        assert_eq!(mono.mono(), vec![0.1, -0.9], "already one per frame");
    }
}
