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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// How much audio the tap holds. Two FFT windows at any rate we are likely to
/// meet — enough to draw from, short enough that what it holds is what you
/// are hearing.
pub const TAP_SAMPLES: usize = 8192;

/// How many samples a source gathers before handing them over. Taking the
/// lock per sample would be ninety thousand times a second for no benefit;
/// this makes it about twenty.
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

    /// From the audio thread. Never waits — see the module note.
    fn push(&self, batch: &[Sample], rate: u32, channels: u16) {
        if let Ok(mut ring) = self.ring.try_lock() {
            ring.push(batch, rate, channels);
        }
    }

    /// From the UI thread. `None` means either nothing has played yet or the
    /// audio thread is mid-handover; both say "ask again next frame".
    pub fn frame(&self) -> Option<TapFrame> {
        let ring = self.ring.try_lock().ok()?;
        ring.frame()
    }

    /// Everything held describes where we no longer are.
    pub fn clear(&self) {
        if let Ok(mut ring) = self.ring.try_lock() {
            ring.reset();
        }
    }
}

/// A copy of the tap's contents, oldest sample first.
#[derive(Debug, Clone, PartialEq)]
pub struct TapFrame {
    /// Interleaved by channel, as it was played.
    pub samples: Vec<Sample>,
    pub rate: u32,
    pub channels: u16,
}

impl TapFrame {
    /// One value per frame, channels averaged. What a waveform or an FFT
    /// wants; the vectorscope is the one that needs them kept apart.
    pub fn mono(&self) -> Vec<Sample> {
        let channels = self.channels.max(1) as usize;
        if channels == 1 {
            return self.samples.clone();
        }
        self.samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<Sample>() / channels as Sample)
            .collect()
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
        if self.samples.len() != TAP_SAMPLES {
            self.samples = vec![0.0; TAP_SAMPLES];
        }
        for &sample in batch {
            self.samples[self.write] = sample;
            self.write += 1;
            if self.write == TAP_SAMPLES {
                self.write = 0;
                self.wrapped = true;
            }
        }
    }

    fn frame(&self) -> Option<TapFrame> {
        if self.samples.is_empty() || (!self.wrapped && self.write == 0) {
            return None;
        }
        let mut samples = Vec::with_capacity(if self.wrapped { TAP_SAMPLES } else { self.write });
        if self.wrapped {
            samples.extend_from_slice(&self.samples[self.write..]);
        }
        samples.extend_from_slice(&self.samples[..self.write]);
        Some(TapFrame { samples, rate: self.rate, channels: self.channels })
    }

    fn reset(&mut self) {
        self.write = 0;
        self.wrapped = false;
        self.samples.fill(0.0);
    }
}

/// A source that keeps a copy of everything passing through it.
pub struct Tapped<S> {
    inner: S,
    tap: Arc<AudioTap>,
    batch: Vec<Sample>,
}

impl<S> Tapped<S> {
    pub fn new(inner: S, tap: Arc<AudioTap>) -> Self {
        Tapped { inner, tap, batch: Vec::with_capacity(BATCH) }
    }
}

impl<S: Source> Iterator for Tapped<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        self.batch.push(sample);
        if self.batch.len() >= BATCH {
            let rate = self.inner.sample_rate().get();
            let channels = self.inner.channels().get();
            self.tap.push(&self.batch, rate, channels);
            self.batch.clear();
        }
        Some(sample)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

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

    #[test]
    fn the_tap_holds_the_most_recent_audio_oldest_first() {
        let tap = AudioTap::new();
        let mut source = Tapped::new(Ramp::new(44100, 2), tap.clone());

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
        drain(&mut source, TAP_SAMPLES * 2);
        let frame = tap.frame().unwrap();
        assert_eq!(frame.samples.len(), TAP_SAMPLES);
        let newest = frame.samples[TAP_SAMPLES - 1];
        assert_eq!(frame.samples[0], newest - (TAP_SAMPLES - 1) as Sample, "one run, in order");
    }

    #[test]
    fn a_seek_is_forwarded_and_takes_the_stale_audio_with_it() {
        let tap = AudioTap::new();
        let mut source = Tapped::new(Ramp::new(44100, 2), tap.clone());
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
    fn a_source_of_a_different_shape_does_not_get_interleaved_with_the_last() {
        let tap = AudioTap::new();
        let mut stereo = Tapped::new(Ramp::new(44100, 2), tap.clone());
        drain(&mut stereo, BATCH);
        assert_eq!(tap.frame().unwrap().channels, 2);

        let mut mono = Tapped::new(Ramp::new(48000, 1), tap.clone());
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
