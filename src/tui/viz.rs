//! What the audio looks like.
//!
//! Four ways of drawing the same tap: bars, scope, vectorscope, spectrogram.
//! All of them paint onto [`Canvas`], so they get two pixels per cell and
//! work in any terminal.
//!
//! The bars owe their behaviour to cava, which is MIT and worth reading: the
//! difference between a spectrum that looks like music and one that looks
//! like noise is almost entirely in what happens *between* frames — gravity,
//! neighbour smoothing and automatic sensitivity — rather than in the FFT.

use std::collections::VecDeque;
use std::f32::consts::PI;

use ratatui::style::Color;

use crate::engine::tap::TapFrame;
use crate::tui::canvas::Canvas;
use crate::tui::ui::{accent, dim, folder};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VizMode {
    #[default]
    Bars,
    Scope,
    Vectorscope,
    Spectrogram,
}

pub const VIZ_MODES: [VizMode; 4] =
    [VizMode::Bars, VizMode::Scope, VizMode::Vectorscope, VizMode::Spectrogram];

impl VizMode {
    pub fn title(self) -> &'static str {
        match self {
            VizMode::Bars => "spectrum",
            VizMode::Scope => "scope",
            VizMode::Vectorscope => "vectorscope",
            VizMode::Spectrogram => "spectrogram",
        }
    }

    pub fn next(self) -> Self {
        let at = VIZ_MODES.iter().position(|m| *m == self).unwrap_or(0);
        VIZ_MODES[(at + 1) % VIZ_MODES.len()]
    }
}

/// Everything the visualiser remembers between frames. Bars fall from where
/// they were and the spectrogram is nothing but history, so neither can be
/// worked out from one buffer of audio.
#[derive(Debug, Default)]
pub struct Visualizer {
    pub mode: VizMode,
    bars: Bars,
    /// One spectrum per column, oldest first.
    history: VecDeque<Vec<f32>>,
}

impl Visualizer {
    pub fn draw(&mut self, canvas: &mut Canvas, heard: &TapFrame) {
        match self.mode {
            VizMode::Bars => {
                let bars = self.bars.update(heard, canvas.width() as usize / BAR_STRIDE);
                draw_bars(canvas, bars);
            }
            VizMode::Scope => draw_scope(canvas, heard),
            VizMode::Vectorscope => draw_vectorscope(canvas, heard),
            VizMode::Spectrogram => {
                let spectrum = log_bins(&spectrum(&heard.mono()), heard.rate, canvas.height() as usize);
                self.history.push_back(spectrum);
                while self.history.len() > canvas.width() as usize {
                    self.history.pop_front();
                }
                draw_spectrogram(canvas, &self.history);
            }
        }
    }

    /// Leaving one mode should not leave its state to surprise the next
    /// visit — a spectrogram of what was playing ten minutes ago is a lie.
    pub fn forget(&mut self) {
        self.bars = Bars::default();
        self.history.clear();
    }
}

// ── Spectrum ────────────────────────────────────────────────────────────────

/// Samples per transform. At 44.1 kHz this is 46 ms — long enough to resolve
/// a bass note, short enough that the bars move with the music.
const WINDOW: usize = 2048;

/// The band worth drawing. Below 50 Hz is rumble that swamps everything;
/// above 10 kHz is air that never moves the picture. cava's defaults.
const LOW_HZ: f32 = 50.0;
const HIGH_HZ: f32 = 10_000.0;

/// In-place radix-2 Cooley-Tukey. Twiddles accumulate in f64 — in f32 the
/// rounding walks far enough over a 2048-point transform to tilt the top
/// octave.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut span = 2;
    while span <= n {
        let angle = -2.0 * std::f64::consts::PI / span as f64;
        let (step_re, step_im) = (angle.cos(), angle.sin());
        let mut base = 0;
        while base < n {
            let (mut wr, mut wi) = (1.0f64, 0.0f64);
            for k in 0..span / 2 {
                let a = base + k;
                let b = a + span / 2;
                let (wr32, wi32) = (wr as f32, wi as f32);
                let tr = re[b] * wr32 - im[b] * wi32;
                let ti = re[b] * wi32 + im[b] * wr32;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = wr * step_re - wi * step_im;
                wi = wr * step_im + wi * step_re;
                wr = next;
            }
            base += span;
        }
        span <<= 1;
    }
}

/// Magnitudes of the most recent [`WINDOW`] samples, one per bin up to
/// Nyquist. Hann-windowed: an abrupt cut at each end of the buffer is a step
/// change, and a step change is broadband noise smeared across every bin.
fn spectrum(mono: &[f32]) -> Vec<f32> {
    let mut re = vec![0.0f32; WINDOW];
    let mut im = vec![0.0f32; WINDOW];
    let take = mono.len().min(WINDOW);
    for (i, &sample) in mono[mono.len() - take..].iter().enumerate() {
        let hann = 0.5 - 0.5 * (2.0 * PI * i as f32 / WINDOW as f32).cos();
        re[i] = sample * hann;
    }
    fft(&mut re, &mut im);
    // Scaled so a full-scale tone comes out near 1.0 rather than near 500.
    // Without it every gain downstream has to carry a factor of the window
    // length, and the automatic one would spend ten seconds finding it.
    let scale = 4.0 / WINDOW as f32;
    (0..WINDOW / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * scale).collect()
}

/// Fold the bins into `count` bands spaced logarithmically, because that is
/// how octaves are spaced and how hearing works. Linear bands give a picture
/// where the bottom two bars carry the entire song.
fn log_bins(magnitudes: &[f32], rate: u32, count: usize) -> Vec<f32> {
    if count == 0 || magnitudes.is_empty() || rate == 0 {
        return Vec::new();
    }
    let hz_per_bin = rate as f32 / WINDOW as f32;
    let ratio = HIGH_HZ / LOW_HZ;
    (0..count)
        .map(|band| {
            let from = LOW_HZ * ratio.powf(band as f32 / count as f32);
            let to = LOW_HZ * ratio.powf((band + 1) as f32 / count as f32);
            let first = (from / hz_per_bin) as usize;
            let last = ((to / hz_per_bin) as usize).max(first + 1).min(magnitudes.len());
            // The loudest bin in the band, not the average: an average buries
            // a single strong partial under the silence either side of it.
            magnitudes[first.min(magnitudes.len().saturating_sub(1))..last]
                .iter()
                .fold(0.0f32, |loudest, m| loudest.max(*m))
        })
        .collect()
}

// ── Bars ────────────────────────────────────────────────────────────────────

/// Cells across per bar, so bars are wide enough to read as bars.
const BAR_STRIDE: usize = 2;

/// How fast a bar picks up speed on the way down, per frame squared.
const FALL_STEP: f32 = 0.028;

/// How much of a bar's height its neighbours inherit, per step away. A spike
/// on its own reads as noise; a spike with shoulders reads as a note.
const SPREAD: f32 = 1.7;

/// Quieter than this and a band is silence as far as the picture is
/// concerned. Sixty decibels is about the range of a room.
const FLOOR_DB: f32 = -60.0;

#[derive(Debug)]
struct Bars {
    heights: Vec<f32>,
    /// Where each bar started falling from, and how long it has been falling.
    peaks: Vec<f32>,
    falls: Vec<f32>,
    sensitivity: f32,
}

impl Default for Bars {
    fn default() -> Self {
        Bars { heights: Vec::new(), peaks: Vec::new(), falls: Vec::new(), sensitivity: 1.0 }
    }
}

impl Bars {
    fn update(&mut self, heard: &TapFrame, count: usize) -> &[f32] {
        if count == 0 {
            self.heights.clear();
            return &self.heights;
        }
        if self.heights.len() != count {
            self.heights = vec![0.0; count];
            self.peaks = vec![0.0; count];
            self.falls = vec![0.0; count];
        }

        let bands = log_bins(&spectrum(&heard.mono()), heard.rate, count);
        if bands.len() != count {
            return &self.heights;
        }
        // Decibels, over a fixed range with a floor under it. Loudness is
        // logarithmic and so is the ear, but the floor is what matters for
        // the picture: without it every band keeps a sliver of bar and the
        // bottom of the panel is a permanent slab.
        let raised: Vec<f32> = bands
            .iter()
            .map(|band| {
                let db = 20.0 * (band * self.sensitivity).max(1e-9).log10();
                ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
            })
            .collect();

        // Neighbour smoothing, cava's "monstercat": every bar lifts the ones
        // beside it by a share that decays with distance.
        let mut spread = raised.clone();
        for (i, &lifted) in raised.iter().enumerate() {
            for (j, bar) in spread.iter_mut().enumerate() {
                if i != j {
                    *bar = bar.max(lifted / SPREAD.powi(i.abs_diff(j) as i32));
                }
            }
        }

        let mut loudest = 0.0f32;
        for (i, &target) in spread.iter().enumerate() {
            if target >= self.heights[i] {
                // Rising is instant. A transient you cannot see is a
                // transient the visualiser missed.
                self.heights[i] = target;
                self.peaks[i] = target;
                self.falls[i] = 0.0;
            } else {
                // Falling accelerates. A linear decay reads as a lift being
                // lowered; gravity reads as something dropping.
                self.falls[i] += FALL_STEP;
                let fallen = self.peaks[i] - self.falls[i] * self.falls[i];
                self.heights[i] = fallen.max(target).max(0.0);
                if self.heights[i] <= target {
                    self.peaks[i] = target;
                    self.falls[i] = 0.0;
                }
            }
            loudest = loudest.max(self.heights[i]);
        }

        // Autosens: a quiet track and a loud one should both fill the panel.
        // Back off quickly when it clips and creep up when there is headroom,
        // so it settles instead of pumping.
        if loudest > 1.0 {
            self.sensitivity *= 0.98;
        } else if loudest > 0.001 && loudest < 0.6 {
            self.sensitivity *= 1.002;
        }
        self.sensitivity = self.sensitivity.clamp(0.05, 200.0);

        &self.heights
    }
}

fn draw_bars(canvas: &mut Canvas, bars: &[f32]) {
    let height = canvas.height() as i32;
    for (i, &bar) in bars.iter().enumerate() {
        let top = height - (bar.clamp(0.0, 1.0) * height as f32).round() as i32;
        let x = (i * BAR_STRIDE) as i32;
        for y in top..height {
            // Colour by how high the pixel is rather than how tall the bar
            // is, so the gradient stands still while the bars move through
            // it — the whole picture flashing at once is a headache.
            let heat = 1.0 - y as f32 / height as f32;
            for across in 0..BAR_STRIDE as i32 - 1 {
                canvas.set(x + across, y, ramp(heat));
            }
        }
    }
}

// ── Scope ───────────────────────────────────────────────────────────────────

fn draw_scope(canvas: &mut Canvas, heard: &TapFrame) {
    let mono = heard.mono();
    let width = canvas.width() as usize;
    if mono.is_empty() || width == 0 {
        return;
    }

    // Start at a rising zero crossing. Without it the wave slides sideways a
    // random distance every frame and the whole thing shimmers; with it a
    // steady note stands still, which is what an oscilloscope is for.
    let span = (width * 4).min(mono.len());
    let searchable = mono.len() - span;
    let start = (0..searchable)
        .find(|&i| mono[i] <= 0.0 && mono[i + 1] > 0.0)
        .unwrap_or(0);
    let shown = &mono[start..start + span];

    let middle = canvas.height() as f32 / 2.0;
    let at = |x: usize| -> i32 {
        let sample = shown[(x * shown.len() / width).min(shown.len() - 1)];
        (middle - sample.clamp(-1.0, 1.0) * middle) as i32
    };
    let mut previous = at(0);
    for x in 0..width {
        let y = at(x);
        canvas.line((x as i32, previous), (x as i32, y), accent());
        previous = y;
    }
}

// ── Vectorscope ─────────────────────────────────────────────────────────────

fn draw_vectorscope(canvas: &mut Canvas, heard: &TapFrame) {
    let width = canvas.width() as f32;
    let height = canvas.height() as f32;
    if heard.channels < 2 || width == 0.0 || height == 0.0 {
        return;
    }

    // The goniometer turn: mid up the middle, side across. Mono collapses to
    // a vertical line, a wide mix opens into a cloud, and anything out of
    // phase leans over. Cells are half as wide as they are tall even after
    // the half blocks, so the horizontal gets the shorter radius.
    let radius = (width / 2.0).min(height / 2.0);
    let (cx, cy) = (width / 2.0, height / 2.0);
    let root_half = std::f32::consts::FRAC_1_SQRT_2;

    for (index, frame) in heard.samples.chunks_exact(heard.channels as usize).enumerate() {
        let (left, right) = (frame[0], frame[1]);
        let side = (right - left) * root_half;
        let mid = (right + left) * root_half;
        let x = cx + side.clamp(-1.0, 1.0) * radius;
        let y = cy - mid.clamp(-1.0, 1.0) * radius;
        // Older samples sit further back in the buffer and are drawn dimmer,
        // which gives the trace a direction instead of a static scribble.
        let age = index as f32 / (heard.samples.len() / heard.channels as usize).max(1) as f32;
        canvas.set(x as i32, y as i32, if age > 0.65 { accent() } else { folder() });
    }
}

// ── Spectrogram ─────────────────────────────────────────────────────────────

fn draw_spectrogram(canvas: &mut Canvas, history: &VecDeque<Vec<f32>>) {
    let width = canvas.width() as usize;
    let height = canvas.height() as i32;
    // Newest on the right, so it scrolls the way reading does.
    let start = width.saturating_sub(history.len());
    for (column, spectrum) in history.iter().enumerate() {
        let x = (start + column) as i32;
        for (band, &magnitude) in spectrum.iter().enumerate() {
            // Low frequencies at the bottom, like every other spectrogram.
            let y = height - 1 - band as i32;
            let db = 20.0 * magnitude.max(1e-9).log10();
            let heat = (db - FLOOR_DB) / -FLOOR_DB;
            if heat > 0.05 {
                canvas.set(x, y, ramp(heat.min(1.0)));
            }
        }
    }
}

// ── Colour ──────────────────────────────────────────────────────────────────

/// Quiet to loud, through the theme's own colours so a configured accent
/// carries into the visualiser.
fn ramp(heat: f32) -> Color {
    let heat = heat.clamp(0.0, 1.0);
    let stops = [dim(), folder(), accent(), Color::White];
    let scaled = heat * (stops.len() - 1) as f32;
    let lower = (scaled as usize).min(stops.len() - 2);
    match (rgb(stops[lower]), rgb(stops[lower + 1])) {
        // Named or indexed colours cannot be mixed, so the ramp steps between
        // them instead. Coarser, but it never comes out the wrong colour.
        (Some(from), Some(to)) => blend(from, to, scaled - lower as f32),
        _ => stops[if scaled - lower as f32 > 0.5 { lower + 1 } else { lower }],
    }
}

fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::White => Some((255, 255, 255)),
        Color::Black => Some((0, 0, 0)),
        _ => None,
    }
}

fn blend(from: (u8, u8, u8), to: (u8, u8, u8), amount: f32) -> Color {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount) as u8;
    Color::Rgb(mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn sine(hz: f32, rate: u32, samples: usize) -> Vec<f32> {
        (0..samples).map(|i| (2.0 * PI * hz * i as f32 / rate as f32).sin()).collect()
    }

    fn frame(samples: Vec<f32>, rate: u32, channels: u16) -> TapFrame {
        TapFrame { samples, rate, channels }
    }

    #[test]
    fn the_transform_puts_a_tone_in_the_right_bin() {
        // A 1 kHz tone at 44.1 kHz over 2048 samples belongs in bin
        // 1000 / (44100/2048) = 46.4, so bin 46.
        let rate = 44100;
        let magnitudes = spectrum(&sine(1000.0, rate, WINDOW));
        let peak = magnitudes
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(bin, _)| bin)
            .unwrap();
        assert!((45..=47).contains(&peak), "1 kHz landed in bin {peak}");

        // And silence has no peak to find.
        let quiet = spectrum(&vec![0.0; WINDOW]);
        assert!(quiet.iter().all(|m| *m < 1e-3), "silence is silent");
    }

    #[test]
    fn a_transform_of_a_known_shape_matches_what_it_should_be() {
        // A constant signal is all bin zero and nothing else — the one case
        // whose answer can be written down without trusting the code.
        let mut re = vec![1.0f32; 8];
        let mut im = vec![0.0f32; 8];
        fft(&mut re, &mut im);
        assert!((re[0] - 8.0).abs() < 1e-4, "{re:?}");
        for bin in 1..8 {
            assert!(re[bin].abs() < 1e-4 && im[bin].abs() < 1e-4, "bin {bin}: {re:?} {im:?}");
        }
    }

    #[test]
    fn bands_are_spaced_by_octave_not_by_hertz() {
        // Two tones an octave apart should land the same distance apart in
        // bands, which is the whole point of log spacing.
        let rate = 44100;
        let bands = |hz: f32| {
            let magnitudes = spectrum(&sine(hz, rate, WINDOW));
            let bands = log_bins(&magnitudes, rate, 32);
            bands
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(band, _)| band as i32)
                .unwrap()
        };
        let (low, mid, high) = (bands(400.0), bands(800.0), bands(1600.0));
        let (first, second) = (mid - low, high - mid);
        // 32 bands across 50 Hz to 10 kHz is 7.6 octaves, so an octave is
        // about 4.2 bands and consecutive ones round to 4 or 5. The point is
        // that they are the same distance apart give or take that rounding:
        // spaced by hertz, the second gap would be twice the first.
        assert!((first - second).abs() <= 1, "octaves apart: {low} {mid} {high}");
        assert!((3..=5).contains(&first), "about four bands to an octave: {first}");
    }

    #[test]
    fn bars_rise_at_once_and_fall_under_gravity() {
        let mut bars = Bars::default();
        let rate = 44100;
        let loud = frame(sine(1000.0, rate, WINDOW), rate, 1);
        let silence = frame(vec![0.0; WINDOW], rate, 1);

        bars.update(&loud, 16);
        let struck = bars.heights.iter().cloned().fold(0.0f32, f32::max);
        assert!(struck > 0.0, "a tone should move something");

        // The first frame of silence has barely dropped; several frames later
        // it has dropped much further than the difference between them.
        bars.update(&silence, 16);
        let after_one = bars.heights.iter().cloned().fold(0.0f32, f32::max);
        for _ in 0..8 {
            bars.update(&silence, 16);
        }
        let after_nine = bars.heights.iter().cloned().fold(0.0f32, f32::max);
        assert!(after_one < struck, "it should be falling");
        assert!(after_nine < after_one, "and still falling");
        assert!(
            struck - after_one < after_one - after_nine,
            "the fall should accelerate: {struck} -> {after_one} -> {after_nine}"
        );
    }

    #[test]
    fn a_neighbour_of_a_struck_bar_is_lifted_but_not_as_far() {
        let mut bars = Bars::default();
        let rate = 44100;
        bars.update(&frame(sine(1000.0, rate, WINDOW), rate, 1), 24);

        let peak = bars
            .heights
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        let beside = if peak + 1 < bars.heights.len() { peak + 1 } else { peak - 1 };
        assert!(bars.heights[beside] > 0.0, "the neighbour is lifted");
        assert!(bars.heights[beside] < bars.heights[peak], "but not as far");
    }

    #[test]
    fn every_mode_draws_something_and_none_of_them_panic() {
        let rate = 44100;
        // Interleaved stereo, the channels deliberately different so the
        // vectorscope has something other than a line to draw.
        let samples: Vec<f32> = sine(440.0, rate, 4096)
            .into_iter()
            .zip(sine(660.0, rate, 4096))
            .flat_map(|(l, r)| [l, r])
            .collect();
        let heard = frame(samples, rate, 2);

        for mode in VIZ_MODES {
            let mut viz = Visualizer { mode, ..Default::default() };
            let mut canvas = Canvas::new(Rect { x: 0, y: 0, width: 40, height: 10 });
            // Twice, so the modes that carry state over a frame do.
            viz.draw(&mut canvas, &heard);
            viz.draw(&mut canvas, &heard);
            let drawn: usize = canvas
                .into_lines()
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|s| s.content.chars().filter(|c| *c != ' ').count())
                        .sum::<usize>()
                })
                .sum();
            assert!(drawn > 0, "{} drew nothing", mode.title());
        }
    }

    #[test]
    fn a_canvas_with_no_room_is_not_a_crash() {
        let rate = 44100;
        let heard = frame(sine(440.0, rate, 512), rate, 1);
        for (width, height) in [(0, 0), (1, 1), (0, 6), (6, 0)] {
            let mut viz = Visualizer::default();
            for mode in VIZ_MODES {
                viz.mode = mode;
                let mut canvas = Canvas::new(Rect { x: 0, y: 0, width, height });
                viz.draw(&mut canvas, &heard);
            }
        }
    }

    #[test]
    fn the_modes_come_round_in_order() {
        let mut mode = VizMode::default();
        let seen: Vec<VizMode> = (0..VIZ_MODES.len())
            .map(|_| {
                let now = mode;
                mode = mode.next();
                now
            })
            .collect();
        assert_eq!(seen, VIZ_MODES.to_vec());
        assert_eq!(mode, VizMode::default(), "and back to the start");
    }
}
