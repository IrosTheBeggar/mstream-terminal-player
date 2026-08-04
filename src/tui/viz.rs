//! What the audio looks like.
//!
//! Four ways of drawing the same tap: bars, scope, vectorscope, VU. All of
//! them paint onto [`Canvas`], so they get two pixels per cell and work in
//! any terminal.
//!
//! The bars follow cava's `cavacore` (MIT), which is worth reading: almost
//! none of what makes a spectrum look like music is in the transform. It is
//! in the tilt applied across the bands, and in what happens *between*
//! frames. The scope's triggering follows scope-tui (MIT), which waits for a
//! crossing to hold rather than taking the first one it sees.

use std::f32::consts::PI;
use std::time::Instant;

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
    Vu,
}

pub const VIZ_MODES: [VizMode; 4] =
    [VizMode::Bars, VizMode::Scope, VizMode::Vectorscope, VizMode::Vu];

impl VizMode {
    pub fn title(self) -> &'static str {
        match self {
            VizMode::Bars => "spectrum",
            VizMode::Scope => "scope",
            VizMode::Vectorscope => "vectorscope",
            VizMode::Vu => "vu",
        }
    }

    pub fn next(self) -> Self {
        let at = VIZ_MODES.iter().position(|m| *m == self).unwrap_or(0);
        VIZ_MODES[(at + 1) % VIZ_MODES.len()]
    }

    /// Whether this mode plots samples, and so has a say in whether they are
    /// joined up. Bars and a meter are drawn from numbers, not from samples.
    pub fn plots_samples(self) -> bool {
        matches!(self, VizMode::Scope | VizMode::Vectorscope)
    }
}

/// What the visualiser remembers between frames. Bars fall from where they
/// were and a VU meter is nothing but its ballistics, so neither can be
/// worked out from one buffer of audio.
#[derive(Debug, Default)]
pub struct Visualizer {
    pub mode: VizMode,
    /// Leave the samples as points instead of joining them, scope-tui's
    /// "vintage" mode. A line through samples invents a path between them
    /// that the signal never took, which at high frequencies is a lie the
    /// picture tells confidently; dots only ever claim what was measured.
    pub scatter: bool,
    bars: Bars,
    vu: Vu,
    /// When the last frame was drawn. Every smoothing constant here is a rate
    /// per second, not per frame — the panel is redrawn on a timer that a
    /// slow terminal will miss, and smoothing that quietly changes speed with
    /// the frame rate is how a visualiser ends up feeling different on
    /// someone else's machine.
    drawn: Option<Instant>,
}

/// Assumed for the first frame, and the ceiling for a long gap — coming back
/// to a window left in the background should not make everything lurch.
const FRAME: f32 = 1.0 / 30.0;

impl Visualizer {
    /// `sounding` is whether audio is actually being played. When it is not,
    /// the visualiser is given silence and falls to nothing, rather than
    /// holding the last thing it saw.
    pub fn draw(&mut self, canvas: &mut Canvas, heard: &TapFrame, sounding: bool) {
        let now = Instant::now();
        let elapsed = self
            .drawn
            .replace(now)
            .map_or(FRAME, |then| now.duration_since(then).as_secs_f32())
            .clamp(0.004, 0.25);
        self.draw_after(canvas, heard, elapsed, sounding);
    }

    fn draw_after(
        &mut self,
        canvas: &mut Canvas,
        heard: &TapFrame,
        elapsed: f32,
        sounding: bool,
    ) {
        // Nothing sounding means silence, not the last thing that sounded.
        // The tap goes on holding the tenth of a second before the pause, and
        // reusing it would leave the picture settling out of audio that is no
        // longer being played. Given silence, everything falls the way it
        // falls when a track goes quiet — which is what pausing is.
        let quiet;
        let heard = if sounding {
            heard
        } else {
            quiet = TapFrame { samples: vec![0.0; heard.samples.len()], ..heard.clone() };
            &quiet
        };

        match self.mode {
            VizMode::Bars => {
                self.bars.update(heard, canvas.width() as usize / BAR_STRIDE, elapsed);
                draw_bars(canvas, &self.bars.heights);
            }
            VizMode::Scope => draw_scope(canvas, heard, self.scatter),
            VizMode::Vectorscope => draw_vectorscope(canvas, heard, self.scatter),
            VizMode::Vu => {
                self.vu.update(heard, elapsed);
                draw_vu(canvas, &self.vu);
            }
        }
    }

    /// Leaving one mode should not leave its state to surprise the next
    /// visit.
    pub fn forget(&mut self) {
        self.bars = Bars::default();
        self.vu = Vu::default();
        self.drawn = None;
    }
}

// ── Spectrum ────────────────────────────────────────────────────────────────

/// Samples per transform. At 44.1 kHz this is 46 ms — long enough to resolve
/// a bass note, short enough that the bars move with the music. cava reaches
/// for a second, longer transform to do better in the bass; at 2048 we are
/// already past the resolution its bass buffer gets, so one will do.
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
    // Without it every gain downstream carries a factor of the window length,
    // and the automatic one spends ten seconds finding it.
    let scale = 4.0 / WINDOW as f32;
    (0..WINDOW / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * scale).collect()
}

/// How steeply the bands are tilted upward with frequency, from cava's `eq`
/// curve: `eq[n] *= pow(cut_off_frequency[n + 1], 0.85)`.
///
/// This is the single biggest thing separating a spectrum that looks alive
/// from one where everything happens in the left quarter. Recorded music
/// falls off with frequency — somewhere between pink and steeper — so an
/// untilted spectrum is a bass hill with a dead plain beside it. Across
/// 50 Hz to 10 kHz this lifts the top by about 40 dB, which is the same
/// order as the fall it is answering.
const TILT: f32 = 0.85;

/// Fold the bins into `count` bands spaced logarithmically, because that is
/// how octaves are spaced and how hearing works, then tilt them.
fn log_bins(magnitudes: &[f32], rate: u32, count: usize) -> Vec<f32> {
    if count == 0 || magnitudes.is_empty() || rate == 0 {
        return Vec::new();
    }
    let hz_per_bin = rate as f32 / WINDOW as f32;
    let ratio = HIGH_HZ / LOW_HZ;
    // Tilt of one in the middle of the range rather than at an end, so the
    // automatic gain starts near where it will settle.
    let middle = (LOW_HZ * HIGH_HZ).sqrt().powf(TILT);
    (0..count)
        .map(|band| {
            let from = LOW_HZ * ratio.powf(band as f32 / count as f32);
            let to = LOW_HZ * ratio.powf((band + 1) as f32 / count as f32);
            let first = ((from / hz_per_bin) as usize).min(magnitudes.len() - 1);
            let last = ((to / hz_per_bin) as usize).max(first + 1).min(magnitudes.len());
            // The mean across the band, as cava takes it: its `eq` divides by
            // the bin count. A maximum is punchier and wronger — one strong
            // partial then speaks for a whole octave.
            let bins = &magnitudes[first..last];
            let mean = bins.iter().sum::<f32>() / bins.len() as f32;
            mean * to.powf(TILT) / middle
        })
        .collect()
}

// ── Bars ────────────────────────────────────────────────────────────────────

/// Cells across per bar, so bars are wide enough to read as bars.
const BAR_STRIDE: usize = 2;

/// How long a bar takes to fall its whole height.
///
/// cava reaches this in about a quarter of a second, but by a route that
/// doesn't quite hold still: its fall accumulates per *frame* and its gravity
/// scales as `(66/framerate)^2.5`, which leaves a residual square root of the
/// frame interval. On a machine drawing at ten frames a second rather than
/// thirty the bars fall almost twice as fast. Working in seconds instead
/// keeps cava's quarter-second while making it the same quarter-second
/// everywhere.
const FALL_SECS: f32 = 0.26;

/// How quickly the raw band settles. Short — the transform is jittery frame
/// to frame and this is only there to take the flicker off, with gravity
/// doing the actual shaping. cava folds both jobs into one constant and
/// accepts a slower attack for it.
const SMOOTH_SECS: f32 = 0.03;

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
    /// The integral filter's memory — one per bar.
    memory: Vec<f32>,
    sensitivity: f32,
}

impl Default for Bars {
    fn default() -> Self {
        Bars {
            heights: Vec::new(),
            peaks: Vec::new(),
            falls: Vec::new(),
            memory: Vec::new(),
            sensitivity: 1.0,
        }
    }
}

impl Bars {
    fn update(&mut self, heard: &TapFrame, count: usize, elapsed: f32) {
        if count == 0 {
            self.heights.clear();
            return;
        }
        if self.heights.len() != count {
            self.heights = vec![0.0; count];
            self.peaks = vec![0.0; count];
            self.falls = vec![0.0; count];
            self.memory = vec![0.0; count];
        }

        let bands = log_bins(&spectrum(&heard.mono()), heard.rate, count);
        if bands.len() != count {
            return;
        }

        // Decibels over a fixed range with a floor under it. Loudness is
        // logarithmic and so is the ear, but the floor is what matters for
        // the picture: without one every band keeps a sliver of bar and the
        // bottom of the panel becomes a permanent slab.
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

        let settle = (-elapsed / SMOOTH_SECS).exp();
        let mut loudest = 0.0f32;
        for (i, &target) in spread.iter().enumerate() {
            // Everything below is in one domain — smoothed height against
            // smoothed height. Comparing a raw band to a smoothed one lets a
            // bar "fall" upwards, because the peak it falls from was measured
            // on a different scale from the height it is being compared to.
            let smoothed = self.memory[i] * settle + target * (1.0 - settle);
            self.memory[i] = smoothed;

            if smoothed < self.heights[i] {
                // Falling accelerates, as cava's does: a linear decay reads
                // as a lift being lowered, gravity reads as a drop.
                self.falls[i] += elapsed;
                let fallen = (self.falls[i] / FALL_SECS).powi(2);
                self.heights[i] = (self.peaks[i] * (1.0 - fallen)).max(smoothed).max(0.0);
            } else {
                // Rising goes straight there. A transient you cannot see is a
                // transient the visualiser missed.
                self.heights[i] = smoothed;
                self.peaks[i] = smoothed;
                self.falls[i] = 0.0;
            }
            loudest = loudest.max(self.heights[i]);
        }

        // Autosens: a quiet track and a loud one should both fill the panel.
        // Back off quickly when it clips and creep up when there is headroom,
        // so it settles instead of pumping. cava's shape, at rates per second
        // rather than per frame.
        if loudest > 0.98 {
            self.sensitivity *= 1.0 - 1.2 * elapsed;
        } else if loudest > 0.001 && loudest < 0.6 {
            self.sensitivity *= 1.0 + 0.6 * elapsed;
        }
        self.sensitivity = self.sensitivity.clamp(0.02, 500.0);
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
            canvas.set(x, y, ramp(1.0 - y as f32 / height as f32));
        }
    }
}

// ── Scope ───────────────────────────────────────────────────────────────────

/// How many samples past a crossing have to agree before it counts as the
/// start of a wave, from scope-tui. The first crossing you find is as likely
/// to be noise wobbling across zero as the foot of a wave, and locking onto
/// noise is exactly the jitter the trigger exists to remove.
const TRIGGER_DEPTH: usize = 8;

fn draw_scope(canvas: &mut Canvas, heard: &TapFrame, scatter: bool) {
    let width = canvas.width() as usize;
    let height = canvas.height() as i32;
    if width == 0 || height == 0 || heard.samples.is_empty() {
        return;
    }
    let channels = heard.channels.max(1) as usize;
    let frames: Vec<&[f32]> = heard.samples.chunks_exact(channels).collect();
    if frames.is_empty() {
        return;
    }

    // The zero line, so a quiet passage is visibly quiet rather than blank.
    let middle = height / 2;
    for x in 0..canvas.width() as i32 {
        canvas.set(x, middle, dim());
    }

    let span = (width * 4).min(frames.len());
    let start = trigger(&frames, frames.len() - span);
    let shown = &frames[start..start + span];

    // Each channel its own trace and its own colour. Drawn back to front so
    // the left channel, which most ears follow, sits on top.
    let half = height as f32 / 2.0;
    for channel in (0..channels.min(2)).rev() {
        let colour = if channel == 0 { accent() } else { folder() };
        let at = |x: usize| -> i32 {
            let frame = shown[(x * shown.len() / width).min(shown.len() - 1)];
            (half - frame[channel].clamp(-1.0, 1.0) * half) as i32
        };
        let mut previous = at(0);
        for x in 0..width {
            let y = at(x);
            if scatter {
                canvas.set(x as i32, y, colour);
            } else {
                // Joined, so a wave reads as a wave. Each column spans from
                // where the last one ended, or a steep edge comes out as two
                // dots with nothing between them.
                canvas.line((x as i32, previous), (x as i32, y), colour);
            }
            previous = y;
        }
    }
}

/// The first rising crossing that holds, searching only as far as `limit` so
/// there is always a full sweep of samples after it.
fn trigger(frames: &[&[f32]], limit: usize) -> usize {
    let held = |from: usize| {
        (1..=TRIGGER_DEPTH).all(|step| frames.get(from + step).is_some_and(|f| f[0] > 0.0))
    };
    (0..limit).find(|&i| frames[i][0] <= 0.0 && held(i)).unwrap_or(0)
}

// ── Vectorscope ─────────────────────────────────────────────────────────────

fn draw_vectorscope(canvas: &mut Canvas, heard: &TapFrame, scatter: bool) {
    let width = canvas.width() as f32;
    let height = canvas.height() as f32;
    if heard.channels < 2 || width == 0.0 || height == 0.0 {
        return;
    }

    let radius = (width / 2.0).min(height / 2.0);
    let (cx, cy) = (width / 2.0, height / 2.0);

    // Crosshair, from scope-tui: without it a quiet passage is a dot in a
    // void and there is nothing to judge the lean against.
    for x in 0..canvas.width() as i32 {
        canvas.set(x, cy as i32, dim());
    }
    for y in 0..canvas.height() as i32 {
        canvas.set(cx as i32, y, dim());
    }

    // The goniometer turn: mid up the middle, side across. Mono collapses to
    // a vertical line, a wide mix opens into a cloud, and anything out of
    // phase leans over. scope-tui plots the channels straight onto the axes,
    // which puts mono on the diagonal; the turn is what broadcast metering
    // does and it makes "is this mono" a glance rather than a judgement.
    let root_half = std::f32::consts::FRAC_1_SQRT_2;
    let frames = heard.samples.chunks_exact(heard.channels as usize);
    let total = frames.len().max(1);
    let mut previous: Option<(i32, i32)> = None;
    for (index, frame) in frames.enumerate() {
        let (left, right) = (frame[0], frame[1]);
        let side = (right - left) * root_half;
        let mid = (right + left) * root_half;
        let x = (cx + side.clamp(-1.0, 1.0) * radius) as i32;
        let y = (cy - mid.clamp(-1.0, 1.0) * radius) as i32;
        // Older samples are drawn dimmer, so the trace has a direction
        // instead of being a static scribble.
        let age = index as f32 / total as f32;
        let colour = if age > 0.65 { accent() } else { folder() };
        match previous.filter(|_| !scatter) {
            // Joined, the samples make one continuous figure — the Lissajous
            // curve a hardware goniometer draws, because its beam has to
            // travel between the points too.
            Some(from) => canvas.line(from, (x, y), colour),
            None => canvas.set(x, y, colour),
        }
        previous = Some((x, y));
    }
}

// ── VU ──────────────────────────────────────────────────────────────────────

/// How long a VU meter takes to reach 99% of a steady tone, and the thing
/// that makes it a VU meter rather than a level bar. A one-pole reaches 99%
/// in about 4.6 time constants, so this is the 300 ms of the standard.
const VU_ATTACK: f32 = 0.300 / 4.6;

/// How long the peak marker sits before it starts to fall, and how fast it
/// falls once it does. A meter that only shows the average hides clipping;
/// one that only shows peaks tells you nothing about how loud it feels.
const PEAK_HOLD: f32 = 0.9;
const PEAK_FALL_DB: f32 = 24.0;

/// The quietest the meter bothers to show.
const VU_FLOOR_DB: f32 = -48.0;

#[derive(Debug)]
struct Vu {
    /// Mean square per channel, under the VU ballistic.
    power: [f32; 2],
    /// Peak per channel, in decibels, and how long it has been held.
    peak_db: [f32; 2],
    held: [f32; 2],
    channels: usize,
}

/// Hand-written for one field: a derived `Default` would start the peaks at
/// 0.0 dB — full scale — and `forget()` rebuilds this state every time the
/// mode is entered, so every visit would open on a phantom peak spending
/// seconds falling from the right edge. Peaks start on the floor: nothing
/// has been heard yet.
impl Default for Vu {
    fn default() -> Self {
        Vu { power: [0.0; 2], peak_db: [VU_FLOOR_DB; 2], held: [0.0; 2], channels: 0 }
    }
}

impl Vu {
    fn update(&mut self, heard: &TapFrame, elapsed: f32) {
        let channels = (heard.channels.max(1) as usize).min(2);
        self.channels = channels;
        let settle = (-elapsed / VU_ATTACK).exp();

        for channel in 0..channels {
            let samples = heard.samples.iter().skip(channel).step_by(heard.channels.max(1) as usize);
            let (mut sum, mut count, mut loudest) = (0.0f32, 0usize, 0.0f32);
            for sample in samples {
                sum += sample * sample;
                count += 1;
                loudest = loudest.max(sample.abs());
            }
            if count == 0 {
                continue;
            }
            // Power averaged, then rooted for display: that is what RMS is,
            // and averaging the samples themselves would read zero on any
            // symmetrical waveform.
            let mean_square = sum / count as f32;
            self.power[channel] = self.power[channel] * settle + mean_square * (1.0 - settle);

            let peak = 20.0 * loudest.max(1e-9).log10();
            if peak >= self.peak_db[channel] {
                self.peak_db[channel] = peak;
                self.held[channel] = 0.0;
            } else {
                self.held[channel] += elapsed;
                if self.held[channel] > PEAK_HOLD {
                    self.peak_db[channel] -= PEAK_FALL_DB * elapsed;
                }
            }
            self.peak_db[channel] = self.peak_db[channel].max(VU_FLOOR_DB);
        }
    }

    /// Where each channel sits on the meter, 0..=1, and where its peak is.
    fn readings(&self) -> Vec<(f32, f32)> {
        (0..self.channels)
            .map(|channel| {
                let rms = self.power[channel].max(1e-18).sqrt();
                let db = 20.0 * rms.max(1e-9).log10();
                (scale(db), scale(self.peak_db[channel]))
            })
            .collect()
    }
}

fn scale(db: f32) -> f32 {
    ((db - VU_FLOOR_DB) / -VU_FLOOR_DB).clamp(0.0, 1.0)
}

fn draw_vu(canvas: &mut Canvas, vu: &Vu) {
    let width = canvas.width() as i32;
    let height = canvas.height() as i32;
    let readings = vu.readings();
    if width < 4 || height < 4 || readings.is_empty() {
        return;
    }

    // Two bars and a row of marks, with the bars given the room.
    let ticks = height - 1;
    let per_bar = (ticks / readings.len() as i32).max(1);
    let thickness = (per_bar - 1).max(1);

    for (channel, &(level, peak)) in readings.iter().enumerate() {
        let top = channel as i32 * per_bar;
        let filled = (level * width as f32).round() as i32;
        for x in 0..filled {
            // Coloured by where the pixel is on the scale, not by how loud
            // it is now, so the hot end stays where it is and the bar walks
            // into it.
            let colour = zone(x as f32 / width as f32);
            for y in top..top + thickness {
                canvas.set(x, y, colour);
            }
        }
        // The peak, held: the gap between it and the bar is the crest
        // factor, which is to say how hard the track has been squashed.
        let marker = ((peak * width as f32).round() as i32).clamp(0, width - 1);
        if marker > filled {
            for y in top..top + thickness {
                canvas.set(marker, y, Color::White);
            }
        }
    }

    // Where the scale changes character: -24, -12 and -6 dB. Numbers would
    // need character rows the picture is using, and the colour says it.
    for db in [-24.0, -12.0, -6.0] {
        let x = (scale(db) * width as f32) as i32;
        canvas.set(x, height - 1, zone(scale(db)));
    }
}

/// Green while there is room, the accent as it fills, red where it is asking
/// for trouble.
fn zone(across: f32) -> Color {
    let db = VU_FLOOR_DB + across * -VU_FLOOR_DB;
    if db >= -6.0 {
        Color::Red
    } else if db >= -18.0 {
        accent()
    } else {
        folder()
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

    fn stereo(left: Vec<f32>, right: Vec<f32>, rate: u32) -> TapFrame {
        let samples = left.into_iter().zip(right).flat_map(|(l, r)| [l, r]).collect();
        frame(samples, rate, 2)
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
        let rate = 44100;
        let loudest_band = |hz: f32| {
            let bands = log_bins(&spectrum(&sine(hz, rate, WINDOW)), rate, 32);
            bands
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(band, _)| band as i32)
                .unwrap()
        };
        let (low, mid, high) = (loudest_band(400.0), loudest_band(800.0), loudest_band(1600.0));
        let (first, second) = (mid - low, high - mid);
        // 32 bands across 50 Hz to 10 kHz is 7.6 octaves, so an octave is
        // about 4.2 bands and consecutive ones round to 4 or 5. The point is
        // that they are the same distance apart give or take that rounding:
        // spaced by hertz, the second gap would be twice the first.
        assert!((first - second).abs() <= 1, "octaves apart: {low} {mid} {high}");
        assert!((3..=5).contains(&first), "about four bands to an octave: {first}");
    }

    #[test]
    fn the_tilt_lifts_the_top_of_the_range_against_the_bottom() {
        // A flat spectrum — every bin the same — is what the tilt exists to
        // bend. Recorded music falls off with frequency, so drawn flat it is
        // a bass hill with a dead plain beside it.
        //
        // Not tested with tones: a band up at 8 kHz spans dozens of bins and
        // a band at 100 Hz spans one, so averaging across the band cancels
        // most of the tilt for a signal that occupies a single bin. Which is
        // right — real music is broadband — but it makes a pure tone the one
        // signal that says nothing about this.
        let bands = log_bins(&vec![1.0; WINDOW / 2], 44100, 32);
        let (low, high) = (bands[0], bands[31]);
        assert!(high > low * 20.0, "the top should be lifted well clear: {low} vs {high}");
        // And it climbs all the way rather than jumping at one end.
        assert!(
            bands.windows(2).all(|pair| pair[1] > pair[0]),
            "each band above the last: {bands:?}"
        );
    }

    #[test]
    fn bars_rise_at_once_and_fall_under_gravity() {
        let mut bars = Bars::default();
        let rate = 44100;
        let loud = frame(sine(1000.0, rate, WINDOW), rate, 1);
        let silence = frame(vec![0.0; WINDOW], rate, 1);

        for _ in 0..4 {
            bars.update(&loud, 16, FRAME);
        }
        let struck = bars.heights.iter().cloned().fold(0.0f32, f32::max);
        assert!(struck > 0.0, "a tone should move something");

        bars.update(&silence, 16, FRAME);
        let after_one = bars.heights.iter().cloned().fold(0.0f32, f32::max);
        for _ in 0..8 {
            bars.update(&silence, 16, FRAME);
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
    fn a_slow_terminal_gets_the_same_fall_as_a_fast_one() {
        // Every constant here is a rate per second, so nine frames at a
        // thirtieth should land where three frames at a tenth do. Without
        // that, the visualiser feels different on a machine that cannot keep
        // up — which is exactly the machine you would want it to behave on.
        let rate = 44100;
        let loud = frame(sine(1000.0, rate, WINDOW), rate, 1);
        let silence = frame(vec![0.0; WINDOW], rate, 1);

        let settle = |bars: &mut Bars, step: f32, frames: usize| {
            for _ in 0..4 {
                bars.update(&loud, 16, step);
            }
            for _ in 0..frames {
                bars.update(&silence, 16, step);
            }
            bars.heights.iter().cloned().fold(0.0f32, f32::max)
        };
        let fast = settle(&mut Bars::default(), 1.0 / 30.0, 9);
        let slow = settle(&mut Bars::default(), 1.0 / 10.0, 3);
        assert!((fast - slow).abs() < 0.12, "three tenths either way: {fast} vs {slow}");
    }

    #[test]
    fn a_neighbour_of_a_struck_bar_is_lifted_but_not_as_far() {
        let mut bars = Bars::default();
        let rate = 44100;
        for _ in 0..4 {
            bars.update(&frame(sine(1000.0, rate, WINDOW), rate, 1), 24, FRAME);
        }

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
    fn the_trigger_waits_for_a_crossing_that_holds() {
        // A single sample flicking across zero, then the real wave. Taking
        // the first crossing would lock onto the flick and the picture would
        // jump about; the depth check walks past it.
        let mut samples: Vec<f32> = vec![-0.01; 4];
        samples.push(0.02); // noise: up for one sample only
        samples.extend([-0.01; 4]);
        samples.extend(sine(200.0, 44100, 400)); // starts at zero and rises
        let frames: Vec<&[f32]> = samples.chunks_exact(1).collect();

        let at = trigger(&frames, frames.len() - 64);
        assert!(at >= 8, "walked past the single-sample flick, landed at {at}");
        assert!(frames[at][0] <= 0.0 && frames[at + 1][0] > 0.0, "and it is a real crossing");
    }

    #[test]
    fn the_vu_meter_settles_towards_the_level_rather_than_jumping_to_it() {
        let rate = 44100;
        // A full-scale tone is 0.707 RMS, which is -3 dB.
        let loud = stereo(sine(440.0, rate, 2048), sine(440.0, rate, 2048), rate);

        let mut vu = Vu::default();
        vu.update(&loud, FRAME);
        let (first, _) = vu.readings()[0];
        assert!(first > 0.0, "it should have started moving");

        // The standard is 99% of the way in 300 ms, so most of the journey
        // is done by then and the rest of the second adds almost nothing.
        // (The needle covers ground fast at the start because the scale is
        // in decibels: 40% of the power is already 85% of the way up.)
        for _ in 1..(0.3 / FRAME) as usize {
            vu.update(&loud, FRAME);
        }
        let (at_300ms, _) = vu.readings()[0];
        for _ in 0..30 {
            vu.update(&loud, FRAME);
        }
        let (settled, peak) = vu.readings()[0];
        assert!(at_300ms > first, "still climbing at 300 ms: {first} -> {at_300ms}");
        assert!(
            (settled - at_300ms).abs() < 0.02,
            "and all but arrived by then: {at_300ms} vs {settled}"
        );
        assert!(settled > 0.85, "a second of a loud tone should be near the top: {settled}");
        assert!(peak >= settled, "the peak is never under the average: {peak} vs {settled}");

        // And it falls back rather than sticking.
        let silence = stereo(vec![0.0; 2048], vec![0.0; 2048], rate);
        for _ in 0..30 {
            vu.update(&silence, FRAME);
        }
        let (quiet, _) = vu.readings()[0];
        assert!(quiet < 0.3, "a second of silence should have dropped it: {quiet}");
    }

    #[test]
    fn the_peak_marker_holds_before_it_falls() {
        let rate = 44100;
        let mut vu = Vu::default();
        vu.update(&stereo(sine(440.0, rate, 2048), sine(440.0, rate, 2048), rate), FRAME);
        let struck = vu.readings()[0].1;

        let silence = stereo(vec![0.0; 2048], vec![0.0; 2048], rate);
        // Inside the hold it has not moved.
        for _ in 0..((PEAK_HOLD / FRAME) as usize - 4) {
            vu.update(&silence, FRAME);
        }
        assert_eq!(vu.readings()[0].1, struck, "held");

        for _ in 0..30 {
            vu.update(&silence, FRAME);
        }
        assert!(vu.readings()[0].1 < struck, "and then released");
    }

    #[test]
    fn the_meter_opens_with_no_phantom_peak_pinned_at_the_top() {
        // A derived `Default` started `peak_db` at 0.0 — full scale — and
        // `forget()` rebuilds this state on every visit to the mode, so
        // every visit opened on a peak nothing had struck, spending seconds
        // walking it down from the right edge.
        let mut vu = Vu::default();
        vu.update(&frame(vec![0.0; 2048], 44100, 1), FRAME);
        let (_, peak) = vu.readings()[0];
        assert!(peak < 0.05, "silence has struck no peak, got {peak}");
    }

    /// How many pixels a mode lights.
    fn lit(mode: VizMode, scatter: bool, heard: &TapFrame) -> usize {
        let mut viz = Visualizer { mode, scatter, ..Default::default() };
        let mut canvas = Canvas::new(Rect { x: 0, y: 0, width: 60, height: 12 });
        viz.draw_after(&mut canvas, heard, FRAME, true);
        canvas
            .into_lines()
            .iter()
            .map(|line| {
                line.spans.iter().map(|s| s.content.chars().filter(|c| *c != ' ').count()).sum::<usize>()
            })
            .sum()
    }

    #[test]
    fn dots_draw_less_than_joining_them_up_does() {
        // The whole of the trade: a line invents the path between two
        // samples, so it always covers more ground than the samples do. If
        // dots were not sparser, the toggle would not be doing anything.
        let rate = 44100;
        let heard = stereo(sine(2000.0, rate, 4096), sine(3000.0, rate, 4096), rate);

        for mode in [VizMode::Scope, VizMode::Vectorscope] {
            let joined = lit(mode, false, &heard);
            let dotted = lit(mode, true, &heard);
            assert!(dotted > 0, "{} drew nothing as dots", mode.title());
            assert!(
                dotted < joined,
                "{}: dots {dotted} should be sparser than lines {joined}",
                mode.title()
            );
        }

        // And the modes drawn from numbers rather than samples ignore it.
        for mode in [VizMode::Bars, VizMode::Vu] {
            assert!(!mode.plots_samples());
            assert_eq!(lit(mode, false, &heard), lit(mode, true, &heard), "{}", mode.title());
        }
    }

    #[test]
    fn the_values_fall_to_zero_while_nothing_is_sounding() {
        // Measured on the state rather than the picture. What "empty" looks
        // like differs by mode — a meter keeps its held peak and its scale, a
        // scope draws silence as a flat line — but underneath, every value
        // the visualiser is holding should reach zero.
        let rate = 44100;
        let loud = stereo(sine(440.0, rate, 4096), sine(660.0, rate, 4096), rate);
        let canvas = || Canvas::new(Rect { x: 0, y: 0, width: 50, height: 10 });

        let mut viz = Visualizer { mode: VizMode::Bars, ..Default::default() };
        for _ in 0..12 {
            viz.draw_after(&mut canvas(), &loud, FRAME, true);
        }
        assert!(viz.bars.heights.iter().cloned().fold(0.0f32, f32::max) > 0.1, "playing");

        // Fed the loud buffer throughout — which is what the tap really holds
        // after a pause. Reusing it is exactly the thing being ruled out.
        for _ in 0..40 {
            viz.draw_after(&mut canvas(), &loud, FRAME, false);
        }
        let left = viz.bars.heights.iter().cloned().fold(0.0f32, f32::max);
        assert!(left < 0.01, "the bars should be down: {left}");

        let mut viz = Visualizer { mode: VizMode::Vu, ..Default::default() };
        for _ in 0..12 {
            viz.draw_after(&mut canvas(), &loud, FRAME, true);
        }
        assert!(viz.vu.readings()[0].0 > 0.5, "playing");
        for _ in 0..40 {
            viz.draw_after(&mut canvas(), &loud, FRAME, false);
        }
        assert_eq!(viz.vu.readings()[0].0, 0.0, "the meter should read nothing");
    }

    #[test]
    fn a_trace_of_silence_is_a_flat_line_across_the_middle() {
        let rate = 44100;
        // Deliberately the *loud* buffer throughout the paused stretch: that
        // is what the tap really holds after a pause, and a visualiser that
        // reused it would sit there showing a track that is not playing.
        let loud = stereo(sine(440.0, rate, 4096), sine(660.0, rate, 4096), rate);
        // Which character rows carry signal. The reference lines are drawn in
        // the dim colour and are furniture, not signal: a scope showing
        // silence still draws its zero line, and should.
        let signal_rows = |viz: &mut Visualizer, sounding: bool| -> Vec<usize> {
            let mut canvas = Canvas::new(Rect { x: 0, y: 0, width: 50, height: 10 });
            viz.draw_after(&mut canvas, &loud, FRAME, sounding);
            canvas
                .into_lines()
                .iter()
                .enumerate()
                .filter(|(_, line)| {
                    line.spans.iter().any(|span| {
                        span.content.chars().any(|c| c != ' ') && span.style.fg != Some(dim())
                    })
                })
                .map(|(row, _)| row)
                .collect()
        };

        for mode in [VizMode::Scope, VizMode::Vectorscope] {
            let mut viz = Visualizer { mode, ..Default::default() };
            for _ in 0..12 {
                signal_rows(&mut viz, true);
            }
            let playing = signal_rows(&mut viz, true);
            assert!(playing.len() > 2, "{} barely drew while playing: {playing:?}", mode.title());

            // A second of not sounding. What "fallen away" looks like differs
            // by mode: bars and a meter have nothing left to draw at all,
            // while a scope drawing silence honestly is a flat line on the
            // centre and a vectorscope is a point on it.
            for _ in 0..30 {
                signal_rows(&mut viz, false);
            }
            let paused = signal_rows(&mut viz, false);
            assert!(!paused.is_empty(), "{} lost its trace entirely", mode.title());
            assert!(
                paused.iter().all(|row| row.abs_diff(5) <= 1),
                "{} should have collapsed onto the middle: {paused:?}",
                mode.title()
            );

            // And it comes back when the audio does.
            for _ in 0..12 {
                signal_rows(&mut viz, true);
            }
            let again = signal_rows(&mut viz, true);
            assert!(again.len() > paused.len(), "{} never came back: {again:?}", mode.title());
        }
    }

    #[test]
    fn every_mode_draws_something_and_none_of_them_panic() {
        let rate = 44100;
        // The channels deliberately different, so the vectorscope has
        // something other than a line to draw.
        let heard = stereo(sine(440.0, rate, 4096), sine(660.0, rate, 4096), rate);

        for mode in VIZ_MODES {
            let mut viz = Visualizer { mode, ..Default::default() };
            let mut canvas = Canvas::new(Rect { x: 0, y: 0, width: 40, height: 10 });
            // Several times, so the modes that carry state across a frame do.
            for _ in 0..6 {
                viz.draw_after(&mut canvas, &heard, FRAME, true);
            }
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
        let heard = stereo(sine(440.0, rate, 512), sine(440.0, rate, 512), rate);
        for (width, height) in [(0, 0), (1, 1), (0, 6), (6, 0), (3, 2)] {
            let mut viz = Visualizer::default();
            for mode in VIZ_MODES {
                viz.mode = mode;
                let mut canvas = Canvas::new(Rect { x: 0, y: 0, width, height });
                viz.draw_after(&mut canvas, &heard, FRAME, true);
            }
        }
        // And mono, which the vectorscope has nothing to say about.
        let mono = frame(sine(440.0, rate, 512), rate, 1);
        let mut viz = Visualizer::default();
        for mode in VIZ_MODES {
            viz.mode = mode;
            let mut canvas = Canvas::new(Rect { x: 0, y: 0, width: 20, height: 6 });
            viz.draw_after(&mut canvas, &mono, FRAME, true);
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
