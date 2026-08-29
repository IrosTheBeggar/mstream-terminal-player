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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::mixer::Mixer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, cpal};

use super::trace::etrace;

/// An open output stream, its name, and the two tripwires.
pub(crate) struct Output {
    sink: MixerDeviceSink,
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
}

impl Output {
    pub(crate) fn mixer(&self) -> &Mixer {
        self.sink.mixer()
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
/// smear across the UI), and because the tripwires have to ride along.
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
        let flag = gone.clone();
        let errors = Arc::new(AtomicU32::new(0));
        let heard_on = name.clone();
        let opened = DeviceSinkBuilder::from_device(device)
            .map_err(|e| e.to_string())
            .and_then(|builder| {
                builder
                    .with_error_callback(move |err: cpal::StreamError| {
                        // Runs on the OS audio thread: the recorder and an
                        // atomic, nothing that can block or re-enter.
                        crate::stderrln!("[engine] output stream error on {heard_on}: {err}");
                        match err {
                            cpal::StreamError::DeviceNotAvailable
                            | cpal::StreamError::StreamInvalidated => {
                                flag.store(true, Ordering::Relaxed);
                            }
                            // An underrun hurts the ear once and heals.
                            cpal::StreamError::BufferUnderrun => {}
                            // The grab-bag: log-only until the run of them
                            // says the stream is gone (raw-ALSA unplug's
                            // shape — see BACKEND_ERROR_LIMIT).
                            cpal::StreamError::BackendSpecific { .. } => {
                                if errors.fetch_add(1, Ordering::Relaxed) + 1
                                    >= BACKEND_ERROR_LIMIT
                                {
                                    flag.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    })
                    .open_sink_or_fallback()
                    .map_err(|e| e.to_string())
            });
        match opened {
            Ok(mut sink) => {
                sink.log_on_drop(false);
                etrace!("output opened on {name}");
                return Ok(Output { sink, name, default_at_open, gone });
            }
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| "no output device found".to_string()))
}
