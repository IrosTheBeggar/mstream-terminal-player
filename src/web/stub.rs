//! Stand-in for the audio thread, until the WebAudio milestone.
//!
//! The native player runs an audio thread (rodio); the browser build doesn't
//! yet. Audio commands land here and playback is a clock, not a decoder —
//! but the queue, transport and end-of-track flow upstairs are all real. The
//! one honest signal is the synthesised waveform pushed into the [`AudioTap`]
//! so the visualizer has something true to draw.

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::Arc;

use crate::clock::Instant;
use crate::engine::tap::AudioTap;
use crate::player::PlayerStatus;
use crate::tui::worker::{AudioCmd, Event};

const RATE: u32 = 44_100;
/// When a Play carries no duration hint (a transcode would), pretend this.
const FALLBACK_DURATION: f64 = 217.0;
/// The most synth to render after a stall (a backgrounded tab pauses
/// requestAnimationFrame; catching up minutes of sine waves helps no one).
const MAX_SYNTH: f64 = 0.2;

pub struct Stub {
    events: VecDeque<Event>,
    tap: Arc<AudioTap>,
    last_tick: Option<Instant>,

    // What the "engine" is doing.
    source: String,
    playing: bool,
    paused: bool,
    position: f64,
    duration: f64,
    volume: f32,

    synth: Synth,
    buffer: Vec<f32>,
}

impl Stub {
    pub fn new(tap: Arc<AudioTap>) -> Self {
        Stub {
            events: VecDeque::new(),
            tap,
            last_tick: None,
            source: String::new(),
            playing: false,
            paused: false,
            position: 0.0,
            duration: 0.0,
            volume: 1.0,
            synth: Synth::default(),
            buffer: Vec::new(),
        }
    }

    /// Advance the fake clock, feed the visualizer, and hand back what the
    /// audio thread would have sent since last frame.
    pub fn tick(&mut self) -> Vec<Event> {
        let now = Instant::now();
        let dt = match self.last_tick.replace(now) {
            Some(last) => now.duration_since(last).as_secs_f64(),
            None => 0.0,
        };

        if self.playing && !self.paused {
            self.position += dt;
            let frames = ((dt.min(MAX_SYNTH)) * RATE as f64) as usize;
            self.synth.render(frames, self.volume, &mut self.buffer);
            self.tap.push(&self.buffer, RATE, 2);

            if self.position >= self.duration {
                let ended = std::mem::take(&mut self.source);
                self.playing = false;
                self.position = 0.0;
                self.duration = 0.0;
                self.tap.clear();
                self.events.push_back(Event::Status(self.status()));
                self.events.push_back(Event::TrackEnded { source: ended });
            }
        }

        self.events.push_back(Event::Status(self.status()));
        self.events.drain(..).collect()
    }

    fn status(&self) -> PlayerStatus {
        PlayerStatus {
            playing: self.playing,
            paused: self.paused,
            position: self.position,
            duration: self.duration,
            volume: self.volume,
            source: self.source.clone(),
        }
    }

    pub fn dispatch(&mut self, cmd: AudioCmd) {
        match cmd {
            AudioCmd::Play { url, duration_hint } => {
                self.source = url;
                self.playing = true;
                self.paused = false;
                self.position = 0.0;
                self.duration = duration_hint.unwrap_or(FALLBACK_DURATION).max(1.0);
                self.tap.clear();
            }
            AudioCmd::Pause => self.paused = true,
            AudioCmd::Resume => self.paused = false,
            AudioCmd::Stop => {
                self.source.clear();
                self.playing = false;
                self.paused = false;
                self.position = 0.0;
                self.duration = 0.0;
                self.tap.clear();
            }
            AudioCmd::Seek(to) => {
                if self.playing {
                    self.position = to.clamp(0.0, self.duration);
                }
            }
            AudioCmd::SetVolume(v) => self.volume = v.clamp(0.0, 2.0),
            // The blend machinery: announcements of what plays next and the
            // knobs that shape the handover. The stub has one imaginary
            // track and no second deck — nothing to prepare, nothing to
            // blend into.
            AudioCmd::PrepareNext { .. } | AudioCmd::ClearNext => {}
            AudioCmd::SetCrossfade(_)
            | AudioCmd::SetGapless(_)
            | AudioCmd::SetBlendSkips(_)
            | AudioCmd::SetPauseFade(_) => {}
            AudioCmd::Shutdown => {}
        }
        self.events.push_back(Event::Status(self.status()));
    }
}

// ── The signal the visualizer draws ─────────────────────────────────────────

/// A tiny groove box: kick and hats on a 124 BPM grid, a bass line walking a
/// four-bar loop, an arpeggio on top. Not the track "playing" — decode is the
/// next milestone — but honest input for the waveform, FFT and vectorscope,
/// which is what the visualizer screens are demoing.
struct Synth {
    t: f64,
    bass_phase: f32,
    lead_phase: f32,
    lead_phase_r: f32,
}

impl Default for Synth {
    fn default() -> Self {
        Synth { t: 0.0, bass_phase: 0.0, lead_phase: 0.0, lead_phase_r: 0.0 }
    }
}

const BPM: f64 = 124.0;
const BASS_NOTES: [f32; 4] = [110.0, 87.31, 98.0, 130.81]; // A2 F2 G2 C3
const ARP: [f32; 4] = [2.0, 3.0, 4.0, 6.0];

impl Synth {
    fn render(&mut self, frames: usize, volume: f32, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(frames * 2);
        let step = 1.0 / RATE as f64;
        let spb = 60.0 / BPM;

        for _ in 0..frames {
            self.t += step;
            let beat = self.t / spb;
            let beat_pos = (beat.fract()) as f32;
            let bar = (beat / 4.0) as usize;
            let eighth = (beat * 2.0) as usize;
            let eighth_pos = ((beat * 2.0).fract()) as f32;

            let bass_freq = BASS_NOTES[bar % BASS_NOTES.len()];
            self.bass_phase = (self.bass_phase + TAU * bass_freq / RATE as f32) % TAU;
            let bass = self.bass_phase.sin() * 0.42;

            let lead_freq = bass_freq * ARP[eighth % ARP.len()];
            self.lead_phase = (self.lead_phase + TAU * lead_freq / RATE as f32) % TAU;
            self.lead_phase_r = (self.lead_phase_r + TAU * lead_freq * 1.01 / RATE as f32) % TAU;
            let gate = (-5.0 * eighth_pos).exp() * 0.26;

            let kick = (TAU * 52.0 * self.t as f32).sin() * (-9.0 * beat_pos).exp() * 0.85;
            let hat = (fastrand::f32() - 0.5) * (-24.0 * eighth_pos).exp() * 0.3;

            let bed = bass + kick + hat;
            let l = ((bed + self.lead_phase.sin() * gate) * volume * 0.6).clamp(-1.0, 1.0);
            let r = ((bed + self.lead_phase_r.sin() * gate) * volume * 0.6).clamp(-1.0, 1.0);
            out.push(l);
            out.push(r);
        }
    }
}
