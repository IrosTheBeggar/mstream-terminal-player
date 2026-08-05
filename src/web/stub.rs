//! Stand-ins for the two worker threads, answering synchronously from
//! [`canned`] data.
//!
//! The native player runs an audio thread (rodio) and an api thread (HTTP);
//! the browser spike runs neither. Commands the App dispatches land here,
//! and events come back out of [`Stub::tick`] on the next frame — the same
//! message flow, minus the latency and minus the sound. Playback is a clock,
//! not a decoder; the one honest signal is the synthesised waveform pushed
//! into the [`AudioTap`] so the visualizer has something true to draw.

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::Arc;

use crate::api::types::SonicReport;
use crate::clock::Instant;
use crate::engine::tap::AudioTap;
use crate::player::PlayerStatus;
use crate::tui::app::Effect;
use crate::tui::worker::{ApiCmd, AudioCmd, AutoDjMode, DiscoverData, DiscoverNode, Event};

use super::canned;

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

    pub fn dispatch(&mut self, effect: Effect) {
        match effect {
            Effect::Audio(cmd) => self.audio(cmd),
            Effect::Api(cmd) => self.api(cmd),
            Effect::Discover => self.events.push_back(Event::ServersDiscovered(canned::lan_servers())),
            // Nothing durable to save to in a spike. localStorage is the
            // obvious home when this grows up.
            Effect::SaveSession => {}
        }
    }

    /// Advance the fake clock, feed the visualizer, and hand back what the
    /// workers would have sent since last frame.
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

    fn audio(&mut self, cmd: AudioCmd) {
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

    fn api(&mut self, cmd: ApiCmd) {
        let event = match cmd {
            ApiCmd::Connect { server, .. } => Event::Connected {
                server: server.clone(),
                id: server,
                username: None,
                token: None,
                ping: Box::new(canned::ping()),
            },
            ApiCmd::Login { server, username, .. } => Event::Connected {
                server: server.clone(),
                id: server,
                username: Some(username),
                token: Some("demo-token".to_string()),
                ping: Box::new(canned::ping()),
            },
            ApiCmd::QuickConnect { .. } => Event::Error(
                "Quick Connect needs the native player — the tunnel is iroh, not HTTP".to_string(),
            ),
            ApiCmd::Browse(path) => Event::Listing(Box::new(canned::listing(&path))),
            ApiCmd::Library { node, dest } => {
                Event::Library { data: canned::library_data(&node), node, dest }
            }
            ApiCmd::AutoDj(request) => {
                let mut ignore_list = request.ignore_list.clone();
                let pool: Vec<_> = canned::tracks()
                    .into_iter()
                    .filter(|t| canned::track_id(t).is_none_or(|id| !ignore_list.contains(&id)))
                    .collect();
                match pool.get(fastrand::usize(0..pool.len().max(1))) {
                    Some(pick) => {
                        ignore_list.extend(canned::track_id(pick));
                        Event::AutoDjPick {
                            candidates: vec![pick.clone()],
                            ignore_list,
                            note: None,
                        }
                    }
                    // Everything played once: the server would reset here.
                    None => Event::AutoDjPick {
                        candidates: canned::tracks().into_iter().take(1).collect(),
                        ignore_list: Vec::new(),
                        note: Some("every track played once — starting the pool over".to_string()),
                    },
                }
            }
            ApiCmd::AutoDjSample { request, count } => {
                let mut tracks = canned::tracks();
                fastrand::shuffle(&mut tracks);
                tracks.truncate(count);
                let pool = (request.sonic_available && request.mode == AutoDjMode::Similar)
                    .then_some(SonicReport { similarity: Some(0.72), pool_size: 847 });
                Event::AutoDjSample { tracks, pool, note: None }
            }
            ApiCmd::Genres => Event::Genres(canned::genres()),
            ApiCmd::Journey { length, .. } => {
                Event::Journey { stops: canned::journey(length), note: None, length }
            }
            ApiCmd::Discover { node, seed } => {
                let data = match node {
                    DiscoverNode::Artists | DiscoverNode::Artist(_) => {
                        DiscoverData::Artists(canned::similar_artists(&seed))
                    }
                    _ => DiscoverData::Tracks(canned::similar_tracks(&seed)),
                };
                Event::Discover { node, data, note: None }
            }
            ApiCmd::Playlists => Event::Playlists(canned::playlists()),
            ApiCmd::LoadPlaylist(name) => {
                let tracks = canned::playlist_tracks(&name);
                Event::PlaylistTracks { name, tracks }
            }
            ApiCmd::Search(query) => {
                let results = Box::new(canned::search(&query));
                Event::SearchResults { query, results }
            }
            // The canned library has no covers; "no art" is the same answer
            // the real worker gives for a track whose art went missing.
            ApiCmd::AlbumArt { file } => Event::AlbumArt { file, art: None },
            ApiCmd::Shutdown => return,
        };
        self.events.push_back(event);
    }
}

// ── The signal the visualizer draws ─────────────────────────────────────────

/// A tiny groove box: kick and hats on a 124 BPM grid, a bass line walking a
/// four-bar loop, an arpeggio on top. Not the track "playing" — there isn't
/// one — but honest input for the waveform, FFT and vectorscope, which is
/// what the visualizer screens are demoing.
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
