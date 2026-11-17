//! The engine's own mixer: every Player's sound, summed, for whichever
//! device stream has the floor.
//!
//! rodio's mixer has two halves, and its consuming half — the one that owns
//! every source that is sounding — goes into the device callback. The
//! callback belongs to the stream, the stream to one device, so the sound
//! died with the device: the engine got through a change of output by
//! opening every track again and seeking back to where the music stood,
//! on the thread that answers every command. Over a network that is the
//! track downloaded a second time up to that point, with the player deaf
//! for as long as it took (the device-switch freeze).
//!
//! Here the sources belong to the engine instead. A device stream is a
//! reader: its callback asks the bus for a buffer in its own shape and gets
//! the sum of every voice, or silence when the floor is another stream's.
//! Moving to a new device is then one number changing ([`Bus::give_to`]) —
//! no source is opened, no position is lost, a pause stays a pause and a
//! blend goes on blending.
//!
//! Voices are converted straight to the shape of the stream that is asking,
//! once, as rodio's mixer converts them — and when the next stream has
//! another shape (48 kHz speakers, 44.1 kHz headphones) each voice is taken
//! back out of its converter and given a new one ([`Voice::reshape`]), so a
//! move never leaves a track resampled twice.
//!
//! The device side only ever *tries* the voices' lock: a stream that has
//! just been replaced may still be inside its last pull — parked there, if
//! a download stalled under it — and the one taking over plays silence
//! until that pull comes back rather than wait behind it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use rodio::cpal::{FromSample, Sample as DeviceSample};
use rodio::source::{SeekError, UniformSourceIterator};
use rodio::{ChannelCount, Player, Sample, SampleRate, Source};

/// A device stream's shape — channels and frames a second — which is what
/// every voice is converted to while that stream has the floor.
pub(crate) type Format = (ChannelCount, SampleRate);

type Sound = Box<dyn Source + Send>;

/// The floor's value while no stream has it.
const NOBODY: u64 = 0;

/// The handle the engine keeps, and every device stream a clone of.
#[derive(Clone)]
pub(crate) struct Bus(Arc<Shared>);

struct Shared {
    /// Sounds connected since a callback last looked. The engine's side of
    /// the bus stops here: it never touches a voice that is sounding.
    arrivals: Mutex<Vec<Sound>>,
    /// Raised with every arrival, so the callback's look costs an atomic
    /// load a frame while nothing is arriving.
    arrived: AtomicBool,
    /// What is sounding. Only a device callback takes this, and only by
    /// trying.
    voices: Mutex<Voices>,
    /// The id of the stream that may pull.
    floor: AtomicU64,
}

#[derive(Default)]
struct Voices {
    /// The shape `live` is converted to: the last stream's that pulled.
    format: Option<Format>,
    live: Vec<Voice>,
}

impl Voices {
    /// A stream of another shape has the floor: every voice is converted
    /// again from its own samples.
    fn reshape(&mut self, format: Format) {
        let live = std::mem::take(&mut self.live);
        self.live = live.into_iter().filter_map(|voice| voice.reshape(format)).collect();
        self.format = Some(format);
    }
}

/// One sound on the bus, in the shape of the stream that is playing it.
struct Voice {
    sound: UniformSourceIterator<Lent>,
    /// Where the source lands when its converter is dropped — see [`Lent`].
    back: Arc<Mutex<Option<Sound>>>,
}

impl Voice {
    fn new(source: Sound, format: Format) -> Voice {
        let back = Arc::new(Mutex::new(None));
        let lent = Lent { source: Some(source), back: back.clone() };
        Voice { sound: UniformSourceIterator::new(lent, format.0, format.1), back }
    }

    /// The same sound for a stream of another shape. rodio's converter
    /// keeps what it wraps and has no way to give it up, so the source
    /// comes back through the converter's drop. What the old converter had
    /// read ahead goes with it: a frame or two of a resampled track, some
    /// tens of microseconds.
    fn reshape(self, format: Format) -> Option<Voice> {
        let Voice { sound, back } = self;
        drop(sound);
        let source = back.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()?;
        Some(Voice::new(source, format))
    }
}

/// A source on loan to a converter: it plays through like any other, and
/// when the converter is dropped it puts itself back where the voice can
/// find it, rather than end with the converter.
struct Lent {
    source: Option<Sound>,
    back: Arc<Mutex<Option<Sound>>>,
}

impl Drop for Lent {
    fn drop(&mut self) {
        if let Some(source) = self.source.take() {
            *self.back.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(source);
        }
    }
}

impl Iterator for Lent {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        self.source.as_mut()?.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.source.as_ref().map_or((0, Some(0)), |source| source.size_hint())
    }
}

impl Source for Lent {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        self.source.as_ref().map_or(Some(0), |source| source.current_span_len())
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.source.as_ref().map_or(ChannelCount::MIN, |source| source.channels())
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.source.as_ref().map_or(SampleRate::MIN, |source| source.sample_rate())
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.source.as_ref().and_then(|source| source.total_duration())
    }

    #[inline]
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        match self.source.as_mut() {
            Some(source) => source.try_seek(pos),
            None => Ok(()),
        }
    }
}

impl Bus {
    pub(crate) fn new() -> Bus {
        Bus(Arc::new(Shared {
            arrivals: Mutex::new(Vec::new()),
            arrived: AtomicBool::new(false),
            voices: Mutex::new(Voices::default()),
            floor: AtomicU64::new(NOBODY),
        }))
    }

    /// A new Player on the bus: it sounds on whichever stream has the
    /// floor, and goes on sounding when another takes it.
    pub(crate) fn connect(&self) -> Player {
        let (player, sound) = Player::new();
        self.add(sound);
        player
    }

    /// Put a sound on the bus. It starts on a frame boundary of whatever
    /// buffer is being filled when the callback next looks, as a sound
    /// added to rodio's mixer does — begun mid-frame it would play with
    /// its channels swapped.
    pub(crate) fn add(&self, sound: impl Source + Send + 'static) {
        let shared = &*self.0;
        shared
            .arrivals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Box::new(sound));
        shared.arrived.store(true, Ordering::Release);
    }

    /// The floor goes to `stream`: from its next buffer that stream plays
    /// the bus, and every other plays silence.
    pub(crate) fn give_to(&self, stream: u64) {
        self.0.floor.store(stream, Ordering::Release);
    }

    /// One device buffer for `stream`, whole frames of `format`: the sum of
    /// every voice — or silence, when the floor is another stream's or the
    /// stream that had it is still inside its last pull. Never waits on the
    /// engine: the arrivals' lock is held there for a push and no longer.
    pub(crate) fn fill<T>(&self, stream: u64, format: Format, out: &mut [T])
    where
        T: DeviceSample + FromSample<Sample>,
    {
        let shared = &*self.0;
        let has_floor = shared.floor.load(Ordering::Acquire) == stream;
        let mut voices = match has_floor.then(|| shared.voices.try_lock()) {
            Some(Ok(voices)) => voices,
            // A source that panicked under an earlier pull left the list as
            // it stood; what is in it still plays.
            Some(Err(TryLockError::Poisoned(poisoned))) => poisoned.into_inner(),
            Some(Err(TryLockError::WouldBlock)) | None => {
                out.fill(T::EQUILIBRIUM);
                return;
            }
        };
        if voices.format != Some(format) {
            voices.reshape(format);
        }
        for frame in out.chunks_mut(usize::from(format.0.get())) {
            if shared.arrived.load(Ordering::Acquire) && shared.arrived.swap(false, Ordering::AcqRel) {
                let mut arrivals =
                    shared.arrivals.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                voices.live.extend(arrivals.drain(..).map(|sound| Voice::new(sound, format)));
            }
            for slot in frame {
                let mut sum = 0.0;
                voices.live.retain_mut(|voice| match voice.sound.next() {
                    Some(sample) => {
                        sum += sample;
                        true
                    }
                    // Run out: a Player that was dropped, a sound that ended.
                    None => false,
                });
                *slot = T::from_sample(sum);
            }
        }
    }

    /// How many voices are sounding, arrivals not yet looked at included.
    #[cfg(test)]
    pub(crate) fn voices(&self) -> usize {
        let arriving = self.0.arrivals.lock().unwrap().len();
        arriving + self.0.voices.lock().unwrap().live.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;

    fn shape(channels: u16, rate: u32) -> Format {
        (NonZero::new(channels).unwrap(), NonZero::new(rate).unwrap())
    }

    /// A source that counts up from one, so what comes out can be checked
    /// against what went in: sample `n` of channel `c` is `n + c / 10`.
    struct Count {
        frame: u32,
        channel: u16,
        channels: u16,
        rate: u32,
        frames: u32,
    }

    impl Count {
        fn new(channels: u16, rate: u32, frames: u32) -> Count {
            Count { frame: 1, channel: 0, channels, rate, frames }
        }
    }

    impl Iterator for Count {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            if self.frame > self.frames {
                return None;
            }
            let value = self.frame as Sample + Sample::from(self.channel) / 10.0;
            self.channel += 1;
            if self.channel == self.channels {
                self.channel = 0;
                self.frame += 1;
            }
            Some(value)
        }
    }

    impl Source for Count {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> ChannelCount {
            NonZero::new(self.channels).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            NonZero::new(self.rate).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
        fn try_seek(&mut self, _: Duration) -> Result<(), SeekError> {
            Ok(())
        }
    }

    fn pull(bus: &Bus, stream: u64, format: Format, frames: usize) -> Vec<f32> {
        let mut out = vec![9.0f32; frames * usize::from(format.0.get())];
        bus.fill(stream, format, &mut out);
        out
    }

    /// What was heard is what was expected, give or take an f32's rounding.
    #[track_caller]
    fn assert_heard(heard: &[f32], expected: &[f32], what: &str) {
        let same = heard.len() == expected.len()
            && heard.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-4);
        assert!(same, "{what}: heard {heard:?}, expected {expected:?}");
    }

    #[test]
    fn the_stream_with_the_floor_hears_the_sum_and_every_other_silence() {
        let bus = Bus::new();
        let stereo = shape(2, 48_000);
        bus.add(Count::new(2, 48_000, 1_000));
        bus.add(Count::new(2, 48_000, 1_000));

        // Nobody has the floor: silence, and the voices stand where they were.
        assert_eq!(pull(&bus, 1, stereo, 2), [0.0; 4]);

        bus.give_to(1);
        assert_heard(&pull(&bus, 1, stereo, 2), &[2.0, 2.2, 4.0, 4.2], "two voices, summed");
        assert_eq!(pull(&bus, 2, stereo, 2), [0.0; 4], "a stream without the floor");

        // The floor moves: the next stream goes on from the very next frame.
        bus.give_to(2);
        assert_eq!(pull(&bus, 1, stereo, 1), [0.0; 2], "the old stream falls silent");
        let next = pull(&bus, 2, stereo, 2);
        assert_heard(&next, &[6.0, 6.2, 8.0, 8.2], "nothing skipped, nothing twice");
    }

    #[test]
    fn a_voice_that_runs_out_leaves_and_the_rest_play_on() {
        let bus = Bus::new();
        let mono = shape(1, 44_100);
        bus.give_to(7);
        bus.add(Count::new(1, 44_100, 2));
        bus.add(Count::new(1, 44_100, 5));
        assert_eq!(pull(&bus, 7, mono, 4), [2.0, 4.0, 3.0, 4.0]);
        assert_eq!(bus.voices(), 1, "the short one is gone");
        assert_eq!(pull(&bus, 7, mono, 2), [5.0, 0.0], "and silence once the last has run out");
        assert_eq!(bus.voices(), 0);
    }

    #[test]
    fn an_arrival_starts_on_a_frame_and_not_between_two_channels() {
        let bus = Bus::new();
        let stereo = shape(2, 48_000);
        bus.give_to(1);
        bus.add(Count::new(2, 48_000, 100));
        let _ = pull(&bus, 1, stereo, 3);
        // Arriving while a buffer is not being filled: it is picked up at
        // the start of the next frame, left sample first.
        bus.add(Count::new(2, 48_000, 100));
        assert_heard(&pull(&bus, 1, stereo, 1), &[4.0 + 1.0, 4.1 + 1.1], "frame four and frame one");
    }

    #[test]
    fn a_stream_of_another_shape_gets_every_voice_converted_from_its_own_samples() {
        // A stereo 48 kHz track on stereo 48 kHz speakers: passed through.
        let bus = Bus::new();
        bus.give_to(1);
        bus.add(Count::new(2, 48_000, 10_000));
        assert_heard(&pull(&bus, 1, shape(2, 48_000), 2), &[1.0, 1.1, 2.0, 2.1], "as it is");

        // The floor goes to a mono 48 kHz stream: the same track, from the
        // frame after the last one played, taken down to one channel the
        // way rodio takes it — the first of each frame.
        bus.give_to(2);
        assert_heard(&pull(&bus, 2, shape(1, 48_000), 3), &[3.0, 4.0, 5.0], "one channel");

        // And on to a stereo stream at twice the rate: both channels are
        // back — the mono conversion was not converted again, the track
        // was — and it plays on from the next frame, each frame of the
        // track now two of the stream's.
        bus.give_to(3);
        let doubled = pull(&bus, 3, shape(2, 96_000), 3);
        assert_heard(&doubled, &[6.0, 6.1, 6.5, 6.6, 7.0, 7.1], "resampled once, in stereo");
        assert_eq!(bus.voices(), 1, "the one voice all along");
    }

    #[test]
    fn a_stream_still_inside_its_last_pull_costs_the_next_one_a_buffer_not_a_wait() {
        let bus = Bus::new();
        let mono = shape(1, 44_100);
        bus.give_to(1);
        bus.add(Count::new(1, 44_100, 1_000));
        let _ = pull(&bus, 1, mono, 1);

        // The old stream's pull, parked: the lock is held as its callback
        // would hold it through a read that has not come back.
        let held = bus.0.voices.lock().unwrap();
        bus.give_to(2);
        let started = std::time::Instant::now();
        assert_eq!(pull(&bus, 2, mono, 4), [0.0; 4], "silence meanwhile");
        assert!(started.elapsed() < Duration::from_millis(100), "and no waiting for it");
        drop(held);
        assert_eq!(pull(&bus, 2, mono, 2), [2.0, 3.0], "then on from where the old one stopped");
    }

    #[test]
    fn a_player_on_the_bus_sounds_and_leaves_when_it_is_dropped() {
        let bus = Bus::new();
        let stereo = shape(2, 44_100);
        bus.give_to(1);
        let player = bus.connect();
        player.append(Count::new(2, 44_100, 50_000));
        let heard = pull(&bus, 1, stereo, 2_000);
        assert!(heard.iter().any(|sample| *sample != 0.0), "the player's sound came through");
        assert_eq!(bus.voices(), 1);

        // Paused, the bus keeps asking and hears silence; the voice stays.
        player.pause();
        let _ = pull(&bus, 1, stereo, 2_000);
        assert!(pull(&bus, 1, stereo, 500).iter().all(|sample| *sample == 0.0));
        assert_eq!(bus.voices(), 1);

        // Dropped, its queue runs out and the voice goes with it.
        drop(player);
        let _ = pull(&bus, 1, stereo, 2_000);
        let _ = pull(&bus, 1, stereo, 2_000);
        assert_eq!(bus.voices(), 0, "a dropped player leaves the bus");
    }
}
