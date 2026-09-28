//! The output device, and everything needed to notice it is the wrong one.
//!
//! rodio (through cpal) opens a stream on one concrete device and stays
//! there for the stream's whole life. The operating system does not: plug
//! headphones into the jack, pair a Bluetooth speaker, and the *default*
//! output moves while the old stream plays on at the old endpoint —
//! Windows and macOS both leave it running; pull the device the stream is
//! on and the stream just dies, reporting itself once through an error
//! callback and never producing another sample. Neither event reaches the
//! engine on its own, which is how "I plugged in headphones and the
//! speakers kept playing" happens.
//!
//! So the open carries two tripwires out with it: the stream's error
//! callback sets a flag when the device dies under it, and the identity of
//! the system default at open time is kept so a poll can notice it moving.
//! What to *do* about a pulled tripwire — reopen, reattach, seek back —
//! is the engine's business (`Engine::ensure_output`); this module only
//! opens outputs and answers whether the one in hand is still the right one.
//!
//! It also owns the stream outright, which rodio's `MixerDeviceSink` would
//! not allow: that type starts its cpal stream and keeps it private, with
//! no way to stop it. A running stream is a device callback pulling
//! silence ~23 times a second for as long as the process lives — paused,
//! stopped, never played — and to the operating system an active audio
//! client: coreaudiod holds a PreventUserIdleSystemSleep assertion for the
//! player's PID, so a Mac with the player open never idle-sleeps, and
//! PulseAudio or PipeWire cannot suspend the sink (performance audit #75).
//! Held here, the stream can be suspended when the engine goes quiet and
//! woken before anything needs it again ([`Output::suspend`],
//! [`Output::wake`]).

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::cpal::{self, BufferSize, Sample, SampleFormat, StreamConfig};
use rodio::mixer::Mixer;

use super::trace::etrace;

/// An open output stream, its name, and the two tripwires.
pub(crate) struct Output {
    /// The input half of the mixer the stream's callback drains: every
    /// Player connects here.
    mixer: Mixer,
    /// The device's human-readable name, for notices ("audio moved to
    /// Headphones (WH-1000XM4)").
    name: String,
    /// What the system default's identity was when this stream opened.
    /// The poll compares against this — against what the default WAS, not
    /// against the device we got: when the default cannot be opened and a
    /// fallback carries the session, being off-default is the accepted
    /// state, and only a further *change* of default is news.
    default_at_open: Option<String>,
    /// Set by the stream's error callback when the device dies under it.
    gone: Arc<AtomicBool>,
    /// Whether the device callback is running. The open starts it — that
    /// it starts is part of what proves the device works, which is why a
    /// missing or broken device is still an error at launch — and the
    /// engine suspends it once nothing has needed it for a while.
    awake: bool,
    /// Up for as long as the device callback is inside a pull, raised and
    /// lowered by the callback itself — see [`Output::suspend`] for why a
    /// pull in flight matters.
    pulling: Arc<AtomicBool>,
    /// The device stream. Declared last so it drops last, after the mixer,
    /// the order rodio's own sink keeps.
    stream: cpal::Stream,
}

impl Output {
    pub(crate) fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Whether the device callback is running.
    pub(crate) fn is_awake(&self) -> bool {
        self.awake
    }

    /// Let the device go idle: the callback stops, and with it every reason
    /// the operating system had to keep the audio hardware — and on macOS
    /// the machine — awake on the player's behalf. Nothing in the rodio
    /// chain is touched: the players, their positions and a paused track's
    /// decoder stay exactly where they stand, frozen, until [`Output::wake`].
    /// True when the stream is now asleep. A backend that cannot pause
    /// keeps running (cpal's ALSA host swallows the error on raw hardware
    /// without pause support), which costs what it always did.
    ///
    /// Never under a pull in flight. Pausing waits for the callback it
    /// interrupts — CoreAudio's AudioDeviceStop queues on the lock the IO
    /// thread holds through the callback — and a pull can be stuck: a
    /// track whose download stalled parks the callback in the decoder's
    /// read until the network answers, audit #48's wait. The suspend held
    /// the tick there, and every control queued behind it (review of
    /// audit #75). A stream mid-pull is left running and asked again at a
    /// later tick. A callback that is not pulling cannot start a pull that
    /// blocks, as long as the engine is at rest: a paused player never
    /// reads its decoder, and a stopped one has left the mixer.
    pub(crate) fn suspend(&mut self) -> bool {
        if !self.awake || self.is_dead() || self.pulling.load(Ordering::Acquire) {
            return false;
        }
        match self.stream.pause() {
            Ok(()) => {
                self.awake = false;
                true
            }
            Err(e) => {
                etrace!("output would not suspend on {}: {e}", self.name);
                false
            }
        }
    }

    /// Run the callback again. rodio completes a Player's seek, stop and
    /// skip inside the callback — try_seek waits on it outright — so
    /// anything about to lean on a Player comes after this. A stream that
    /// will not restart pulls the death tripwire: whatever happened to the
    /// device while it slept, the rebuild is the answer.
    pub(crate) fn wake(&mut self) {
        if self.awake {
            return;
        }
        match self.stream.play() {
            Ok(()) => {
                self.awake = true;
                etrace!("output woke on {}", self.name);
            }
            Err(e) => {
                etrace!("output would not wake on {}: {e}", self.name);
                self.gone.store(true, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Whether the stream reported its own death: the device was
    /// unplugged, the Bluetooth link dropped, the stream invalidated.
    pub(crate) fn is_dead(&self) -> bool {
        self.gone.load(Ordering::Relaxed)
    }

    /// Whether the system default now names a different device than it
    /// did when this stream opened. A default that disappeared entirely
    /// is not a move — there is nothing to move to, and if OUR device
    /// died with it the error callback says so.
    pub(crate) fn default_moved(&self) -> bool {
        match default_identity() {
            Some(now) => self.default_at_open.as_deref() != Some(now.as_str()),
            None => false,
        }
    }

    /// Pull the death tripwire by hand. Nobody can unplug hardware from a
    /// test, but from here on the recovery path is the same one a real
    /// unplug takes.
    #[cfg(test)]
    pub(crate) fn pretend_dead(&self) {
        self.gone.store(true, Ordering::Relaxed);
    }
}

/// A device's stable identity when the backend has one (host + endpoint
/// id), its name when not. Only ever compared for equality; two devices
/// sharing a name on a backend without ids is a change this cannot see,
/// which costs a missed rebuild there and nothing anywhere else.
fn identity(device: &cpal::Device) -> Option<String> {
    device
        .id()
        .ok()
        .map(|id| id.to_string())
        .or_else(|| device.description().ok().map(|d| d.name().to_string()))
}

fn name_of(device: &cpal::Device) -> String {
    device
        .description()
        .ok()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|| "unnamed output".to_string())
}

/// The identity of whatever the system calls the default output right now.
fn default_identity() -> Option<String> {
    identity(&cpal::default_host().default_output_device()?)
}

/// How many backend-specific stream errors one stream may report before
/// they count as the device dying. The variant has no single meaning —
/// ALSA reports a vanished raw hw device this way (`DeviceNotAvailable`
/// is not in its vocabulary; the desktop path never needs it because
/// PipeWire and PulseAudio move streams themselves), and every host uses
/// it for recoverable hiccups too. One is noise; a run of them is a
/// stream that is not coming back, and the rebuild that answers is
/// self-limiting either way — it reopens in place and the count starts
/// over with the fresh stream.
const BACKEND_ERROR_LIMIT: u32 = 5;

/// Open an output on the current default device, walking the other
/// devices if the default is missing or will not open — the same walk
/// rodio's `open_default_sink` makes, rebuilt here because that
/// convenience hardcodes an error callback that `eprintln!`s straight
/// onto the TUI's alternate screen (the raw "no longer available. For
/// example, it has been unplugged." a pulled headphone jack used to
/// smear across the UI), because the tripwires have to ride along, and
/// because the stream itself has to stay ours to suspend.
pub(crate) fn open() -> Result<Output, String> {
    let host = cpal::default_host();
    let default_device = host.default_output_device();
    let default_at_open = default_device.as_ref().and_then(identity);

    let mut candidates: Vec<cpal::Device> = Vec::new();
    candidates.extend(default_device);
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            // The default is already first in line; the rest join behind
            // it minus ALSA's "null" driver, which opens happily and
            // plays to nowhere (rodio's own fallback applies the same
            // filter). As the *chosen* default it stays eligible —
            // headless boxes route through it on purpose.
            let is_default =
                default_at_open.is_some() && identity(&device) == default_at_open;
            let real = device
                .description()
                .map(|d| d.driver().is_some_and(|drv| drv != "null"))
                .unwrap_or(false);
            if !is_default && real {
                candidates.push(device);
            }
        }
    }

    let mut first_err: Option<String> = None;
    for device in candidates {
        let name = name_of(&device);
        let gone = Arc::new(AtomicBool::new(false));
        let pulling = Arc::new(AtomicBool::new(false));
        match open_on(&device, error_callback(name.clone(), gone.clone()), &pulling) {
            Ok((stream, mixer)) => {
                etrace!("output opened on {name}");
                return Ok(Output {
                    mixer,
                    name,
                    default_at_open,
                    gone,
                    awake: true,
                    pulling,
                    stream,
                });
            }
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| "no output device found".to_string()))
}

/// The error callback every stream carries: the tripwires' trigger.
fn error_callback(
    heard_on: String,
    flag: Arc<AtomicBool>,
) -> impl FnMut(cpal::StreamError) + Clone + Send + 'static {
    let errors = Arc::new(AtomicU32::new(0));
    move |err: cpal::StreamError| {
        // Runs on the OS audio thread: the recorder and an atomic, nothing
        // that can block or re-enter.
        crate::stderrln!("[engine] output stream error on {heard_on}: {err}");
        match err {
            cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated => {
                flag.store(true, Ordering::Relaxed);
            }
            // An underrun hurts the ear once and heals.
            cpal::StreamError::BufferUnderrun => {}
            // The grab-bag: log-only until the run of them says the stream
            // is gone (raw-ALSA unplug's shape — see BACKEND_ERROR_LIMIT).
            cpal::StreamError::BackendSpecific { .. } => {
                if errors.fetch_add(1, Ordering::Relaxed) + 1 >= BACKEND_ERROR_LIMIT {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

/// One device's open, as rodio makes it (`DeviceSinkBuilder::from_device`
/// then `open_sink_or_fallback`) but keeping the stream: the device's
/// default config with rodio's ~50 ms fixed buffer first, then every
/// config the device lists, in rodio's order, at the device's own buffer
/// size — a fixed size is refused by some devices.
fn open_on<E>(
    device: &cpal::Device,
    on_error: E,
    pulling: &Arc<AtomicBool>,
) -> Result<(cpal::Stream, Mixer), String>
where
    E: FnMut(cpal::StreamError) + Clone + Send + 'static,
{
    let default = device.default_output_config().map_err(|e| e.to_string())?;
    let preferred = StreamConfig {
        channels: default.channels(),
        sample_rate: default.sample_rate(),
        buffer_size: BufferSize::Fixed(nearest_power_of_two(default.sample_rate() / 20)),
    };
    let format = default.sample_format();
    start(device, format, &preferred, on_error.clone(), pulling.clone()).or_else(|first| {
        let supported =
            rodio::stream::supported_output_configs(device).map_err(|_| first.clone())?;
        for config in supported {
            let opened = start(
                device,
                config.sample_format(),
                &config.config(),
                on_error.clone(),
                pulling.clone(),
            );
            if opened.is_ok() {
                return opened;
            }
        }
        Err(first)
    })
}

/// Build and start one stream: a fresh rodio mixer whose consuming half
/// the device callback drains with rodio's own fill loop
/// (`MixerDeviceSink::init_stream`), in whichever sample format the
/// device asked for — `pulling` up for the length of every pull.
fn start<E>(
    device: &cpal::Device,
    format: SampleFormat,
    config: &StreamConfig,
    on_error: E,
    pulling: Arc<AtomicBool>,
) -> Result<(cpal::Stream, Mixer), String>
where
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    let channels = NonZero::new(config.channels).ok_or("the device offers no channels")?;
    let rate = NonZero::new(config.sample_rate).ok_or("the device offers no sample rate")?;
    let (mixer, mut samples) = rodio::mixer::mixer(channels, rate);
    macro_rules! build {
        ($($format:ident => $ty:ty),+ $(,)?) => {
            match format {
                $(SampleFormat::$format => device.build_output_stream::<$ty, _, _>(
                    config,
                    move |data: &mut [$ty], _| {
                        pulling.store(true, Ordering::Release);
                        for out in data.iter_mut() {
                            *out = samples
                                .next()
                                .map(Sample::from_sample)
                                .unwrap_or(<$ty as Sample>::EQUILIBRIUM);
                        }
                        pulling.store(false, Ordering::Release);
                    },
                    on_error,
                    None,
                ),)+
                other => return Err(format!("unsupported sample format {other}")),
            }
        };
    }
    let stream = build!(
        F32 => f32,
        F64 => f64,
        I8 => i8,
        I16 => i16,
        I24 => cpal::I24,
        I32 => i32,
        I64 => i64,
        U8 => u8,
        U16 => u16,
        U24 => cpal::U24,
        U32 => u32,
        U64 => u64,
    )
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, mixer))
}

/// The power of two nearest `n`, ties going down — rodio's rounding for
/// its buffer (`math::nearest_multiple_of_two`, private to rodio): 2048
/// frames at 44.1 and 48 kHz, where 50 ms would be 2205 and 2400.
fn nearest_power_of_two(n: u32) -> u32 {
    if n <= 1 {
        return 1;
    }
    let next = n.next_power_of_two();
    let prev = next >> 1;
    if n - prev <= next - n { prev } else { next }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_rounds_the_way_rodio_rounds_it() {
        // 50 ms at the common rates: rodio's from_device lands every one of
        // these on 2048 frames (4096 at 96 kHz), and the owned open has to
        // ask the device for the same buffer rodio's would have.
        assert_eq!(nearest_power_of_two(44_100 / 20), 2048);
        assert_eq!(nearest_power_of_two(48_000 / 20), 2048);
        assert_eq!(nearest_power_of_two(96_000 / 20), 4096);
        assert_eq!(nearest_power_of_two(8_000 / 20), 512);
        // Ties go down, as rodio's do: 3072 sits exactly between.
        assert_eq!(nearest_power_of_two(3072), 2048);
        assert_eq!(nearest_power_of_two(1), 1);
        assert_eq!(nearest_power_of_two(0), 1);
    }
}
