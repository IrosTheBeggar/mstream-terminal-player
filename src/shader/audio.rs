//! The audio texture: what a preset sees of the music.
//!
//! A 512×2 single-channel image bound as `iChannel0`. Row 0 is the spectrum,
//! which a shader reads as `texture(iChannel0, vec2(f, 0.25))`; row 1 is the
//! waveform, `vec2(t, 0.75)`. That much is the ShaderToy convention. The
//! curve inside it is the mobile app's, from `audio_texture.cpp` in
//! mstream_music, ported step for step rather than rebuilt from the
//! terminal's own spectrum in [`crate::tui::viz`] — because the presets were
//! tuned against that file, and a preset reacts to the numbers it was tuned
//! with or it does not look like itself:
//!
//! 1. the newest 1024 samples, Hann-windowed;
//! 2. each bin's magnitude normalised to the amplitude of the tone that
//!    would make it (2/Σw), so a full-scale sine reads 0 dB whatever the
//!    transform's length;
//! 3. smoothed in the linear domain, so the bars do not strobe;
//! 4. mapped through a dB window onto 0..255.
//!
//! `test/golden/audio_texture/` holds what the unmodified C++ uploads for a
//! known signal, and the tests hold this to it.
//!
//! One difference, on purpose. Android smooths once per batch of PCM,
//! about thirty times a second, whatever the time between them; this
//! smooths by the time that passed — `viz.rs`'s rule, since a window that
//! redraws at 144 Hz must not settle five times faster than one at 30. At
//! thirty updates a second the two are the same thing: α = s^(30·dt).
//!
//! The iOS and desktop builds of the mobile app use a different curve (a
//! square root with automatic gain, `spectrum_source.dart`), so a preset
//! reacts differently there than on Android. This follows Android, whose
//! comments say the presets were authored against its curve.

use crate::tui::viz::fft;

/// Bins across, and samples across the waveform row.
pub const WIDTH: usize = 512;
pub const HEIGHT: usize = 2;

const FFT_SIZE: usize = 1024;

/// How often Android's smoothing steps, in steps per second: the rate its
/// PCM batches arrive at.
const ANDROID_RATE: f32 = 30.0;

/// The response curve, and the one thing about the texture a person may
/// tune: the mobile app has a panel for exactly these three.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    /// What maps to 0. Lower surfaces more quiet detail.
    pub min_db: f32,
    /// What maps to 255. Raise it if loud passages clip to white.
    pub max_db: f32,
    /// How much of the last value each step keeps: 0 is raw and flickery,
    /// 0.8 is Web Audio's default. Per Android step — see the module note.
    pub smoothing: f32,
}

impl Default for Curve {
    /// Android's calibrated defaults.
    fn default() -> Self {
        Curve { min_db: -69.7, max_db: -20.7, smoothing: 0.27 }
    }
}

pub struct AudioTexture {
    curve: Curve,
    window: Vec<f32>,
    /// 2/Σw: the magnitude a unit-amplitude tone gives, inverted.
    norm: f32,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Each bin's smoothed magnitude, linear, carried between updates.
    smoothed: Vec<f32>,
    bytes: Vec<u8>,
}

impl Default for AudioTexture {
    fn default() -> Self {
        AudioTexture::new()
    }
}

impl AudioTexture {
    pub fn new() -> AudioTexture {
        // Symmetric Hann, in f32 and summed in order, as the C++ builds it:
        // the golden comparison is tight enough to notice otherwise.
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos()))
            .collect();
        let sum: f32 = window.iter().fold(0.0, |acc, w| acc + w);
        AudioTexture {
            curve: Curve::default(),
            norm: if sum > 0.0 { 2.0 / sum } else { 1.0 },
            window,
            re: vec![0.0; FFT_SIZE],
            im: vec![0.0; FFT_SIZE],
            smoothed: vec![0.0; WIDTH],
            // All zero until the first update, as Android seeds it.
            bytes: vec![0; WIDTH * HEIGHT],
        }
    }

    /// Replace the curve. A window with nothing in it (max at or below min)
    /// is refused and the old one kept; smoothing is held to 0..=0.99.
    /// Android's `setParams`, rule for rule. Only the golden run turns these
    /// knobs until the tunables arrive (PLAN.md, 10.2).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_curve(&mut self, curve: Curve) {
        if curve.max_db > curve.min_db {
            self.curve.min_db = curve.min_db;
            self.curve.max_db = curve.max_db;
        }
        self.curve.smoothing = curve.smoothing.clamp(0.0, 0.99);
    }

    /// The texture for the newest samples, `mono` oldest first, `elapsed`
    /// seconds after the last update. Only the tail is read; a history
    /// shorter than the transform counts as silence before it began, which
    /// is what Android's ring holds before it fills. Paused, the caller
    /// passes silence, and everything falls away at the smoothing's pace.
    pub fn update(&mut self, mono: &[f32], elapsed: f32) -> &[u8] {
        let take = mono.len().min(FFT_SIZE);
        let pad = FFT_SIZE - take;
        let recent = &mono[mono.len() - take..];
        let sample = |i: usize| if i < pad { 0.0 } else { recent[i - pad] };

        for i in 0..FFT_SIZE {
            self.re[i] = sample(i) * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);

        let keep = self.curve.smoothing.powf(elapsed * ANDROID_RATE);
        let range = self.curve.max_db - self.curve.min_db;
        for bin in 0..WIDTH {
            let (re, im) = (self.re[bin], self.im[bin]);
            let magnitude = (re * re + im * im).sqrt() * self.norm;
            self.smoothed[bin] = keep * self.smoothed[bin] + (1.0 - keep) * magnitude;
            let db = 20.0 * self.smoothed[bin].max(1e-7).log10();
            let level = ((db - self.curve.min_db) / range).clamp(0.0, 1.0);
            self.bytes[bin] = (level * 255.0) as u8;
        }

        for x in 0..WIDTH {
            let s = sample(FFT_SIZE - WIDTH + x).clamp(-1.0, 1.0);
            self.bytes[WIDTH + x] = ((0.5 + 0.5 * s) * 255.0) as u8;
        }
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signal `test/golden/audio_texture/generate.cpp` feeds the
    /// reference, bit for bit: integer arithmetic, and float operations that
    /// are exact or singly rounded, in the same order.
    struct Signal {
        lcg: u32,
        bass: u32,
        mid: u32,
        high: u32,
    }

    impl Signal {
        fn new() -> Signal {
            Signal { lcg: 0x2545_F491, bass: 0, mid: 0, high: 0 }
        }

        fn noise(&mut self) -> f32 {
            self.lcg = self.lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.lcg >> 8) as f32 * (1.0 / 16_777_216.0) * 2.0 - 1.0
        }

        fn triangle(phase: u32) -> f32 {
            let t = (phase >> 8) as f32 * (1.0 / 16_777_216.0);
            4.0 * (t - 0.5).abs() - 1.0
        }

        fn frame(&mut self) -> (f32, f32) {
            self.bass = self.bass.wrapping_add(5_843_493);
            self.mid = self.mid.wrapping_add(42_852_281);
            self.high = self.high.wrapping_add(292_174_646);
            let left = Signal::triangle(self.bass) * 0.5 + self.noise() * 0.05;
            let right =
                Signal::triangle(self.mid) * 0.25 + Signal::triangle(self.high) * 0.1 + self.noise() * 0.05;
            (left, right)
        }
    }

    fn fnv1a(mut hash: u64, value: f32) -> u64 {
        for byte in value.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
        hash
    }

    #[test]
    fn it_uploads_what_the_mobile_app_uploads_for_the_same_signal() {
        let golden: &[u8] = include_bytes!("../../test/golden/audio_texture/golden.bin");
        assert_eq!(&golden[..8], b"MSATGLD1");
        let word = |at: usize| u32::from_le_bytes(golden[at..at + 4].try_into().unwrap()) as usize;
        let (steps, frames) = (word(8), word(12));
        let expected_hash = u64::from_le_bytes(golden[16..24].try_into().unwrap());
        assert_eq!(golden.len(), 24 + steps * WIDTH * HEIGHT);

        // The input first, so a generator that drifted is named as that
        // rather than as a hundred differing bytes.
        let mut signal = Signal::new();
        let mut hash = 14_695_981_039_346_656_037u64;
        let mut batches: Vec<Vec<f32>> = Vec::new();
        for step in 0..steps {
            let mut mono = Vec::with_capacity(frames);
            for _ in 0..frames {
                let (left, right) = if step < 8 { signal.frame() } else { (0.0, 0.0) };
                hash = fnv1a(fnv1a(hash, left), right);
                mono.push(0.5 * (left + right));
            }
            batches.push(mono);
        }
        assert_eq!(hash, expected_hash, "the signal made here is not the one the reference was fed");

        // One Android smoothing step per update, exactly.
        let elapsed = 1.0 / ANDROID_RATE;
        assert_eq!(elapsed * ANDROID_RATE, 1.0);

        let mut texture = AudioTexture::new();
        let mut history: Vec<f32> = Vec::new();
        let (mut differing, mut compared) = (0, 0);
        for (step, batch) in batches.iter().enumerate() {
            if step == 5 {
                texture.set_curve(Curve { min_db: -80.0, max_db: -30.0, smoothing: 0.6 });
            }
            history.extend_from_slice(batch);
            let got = texture.update(&history, elapsed);
            let want = &golden[24 + step * WIDTH * HEIGHT..24 + (step + 1) * WIDTH * HEIGHT];
            for (i, (&g, &w)) in got.iter().zip(want).enumerate() {
                compared += 1;
                if g != w {
                    differing += 1;
                }
                let (row, x) = (i / WIDTH, i % WIDTH);
                assert!(
                    g.abs_diff(w) <= 1,
                    "upload {step}, {} {x}: {g} here, {w} from the reference",
                    if row == 0 { "bin" } else { "sample" }
                );
            }
        }
        // On the Mac that generated the vectors the match is exact: none of
        // the 10,240 bytes differ. The allowance is for another platform's
        // libm rounding a cosf or log10f the other way at a truncation
        // boundary, which should be rare — a curve that was merely close
        // would differ everywhere by one.
        assert!(differing * 100 <= compared, "{differing} of {compared} bytes differ by one");
    }

    fn tone(amplitude: f32, cycles_per_window: f32) -> Vec<f32> {
        (0..FFT_SIZE)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * cycles_per_window * i as f32 / FFT_SIZE as f32;
                amplitude * phase.sin()
            })
            .collect()
    }

    #[test]
    fn a_tones_level_is_its_amplitude_in_decibels() {
        // A tone centred on bin 64, at -40 dB: normalised to amplitude, its
        // bin reads 0.01, and the default window puts -40 dB at
        // (−40 + 69.7) / 49 of the way up.
        let mut texture = AudioTexture::new();
        texture.set_curve(Curve { smoothing: 0.0, ..Curve::default() });
        let bytes = texture.update(&tone(0.01, 64.0), 1.0 / ANDROID_RATE);
        let expected = ((-40.0f32 + 69.7) / 49.0 * 255.0) as u8;
        assert!(bytes[64].abs_diff(expected) <= 2, "bin 64 read {}, not ~{expected}", bytes[64]);
        assert!(bytes[20] < 40 && bytes[200] < 40, "the tone stays in its bin");
    }

    #[test]
    fn silence_reads_as_nothing_and_a_flat_line() {
        let mut texture = AudioTexture::new();
        let bytes = texture.update(&[0.0; FFT_SIZE], 1.0);
        assert!(bytes[..WIDTH].iter().all(|&b| b == 0));
        assert!(bytes[WIDTH..].iter().all(|&b| b == 127), "the waveform's middle");
    }

    #[test]
    fn smoothing_is_a_rate_per_second_not_per_update() {
        // Two updates of a sixtieth of a second each land where one of a
        // thirtieth does: a fast window must not settle faster than a slow.
        let loud = tone(0.5, 32.0);
        let mut fast = AudioTexture::new();
        let mut slow = AudioTexture::new();
        fast.update(&loud, 1.0 / 60.0);
        fast.update(&loud, 1.0 / 60.0);
        slow.update(&loud, 1.0 / 30.0);
        for bin in 0..WIDTH {
            let (a, b) = (fast.smoothed[bin], slow.smoothed[bin]);
            assert!((a - b).abs() <= 1e-4 * b.abs().max(1e-6), "bin {bin}: {a} vs {b}");
        }
    }

    #[test]
    fn a_curve_with_nothing_in_it_is_refused_and_smoothing_is_bounded() {
        let mut texture = AudioTexture::new();
        texture.set_curve(Curve { min_db: -20.0, max_db: -20.0, smoothing: 1.5 });
        assert_eq!(texture.curve.min_db, -69.7);
        assert_eq!(texture.curve.max_db, -20.7);
        assert_eq!(texture.curve.smoothing, 0.99);
        texture.set_curve(Curve { min_db: -60.0, max_db: -10.0, smoothing: -1.0 });
        assert_eq!(texture.curve, Curve { min_db: -60.0, max_db: -10.0, smoothing: 0.0 });
    }
}
