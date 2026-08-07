//! Real playback for the browser build.
//!
//! The native player decodes with rodio; a browser already ships a decoder
//! behind `<audio>`, and that is what plays here — it streams with Range
//! requests through the same-origin proxy, speaks every codec the browser
//! does (opus included, which the native decoder can't), and keeps playing
//! when the tab is backgrounded. The deliberate non-choices: cpal's wasm
//! backend rides the deprecated main-thread ScriptProcessorNode, and an
//! AudioWorklet needs cross-origin-isolation headers or a MessagePort
//! firehose — neither survives contact with "a demo site on a static host".
//!
//! WebAudio still earns its keep as the *tap*: the element routes through an
//! AudioContext so a pair of per-channel analysers can copy what is actually
//! sounding into the [`AudioTap`] the visualizer reads. Same-origin matters
//! twice here — a cross-origin element would taint the graph and the
//! analysers would read silence.
//!
//! Everything is polled from `tick()` — position, duration, ended, errors —
//! so there are no retained JS closures; the one future is `play()`'s
//! promise, watched for the autoplay-policy refusal so the UI can say
//! "press a key" instead of playing silence.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use wasm_bindgen_futures::spawn_local;
use web_sys::{
    AnalyserNode, AudioContext, AudioContextState, ChannelSplitterNode, HtmlAudioElement,
};

use crate::clock::Instant;
use crate::engine::tap::AudioTap;
use crate::player::PlayerStatus;
use crate::tui::worker::{AudioCmd, Event};

/// Time-domain window the analysers hold. Matches the tap's own sizing
/// story: enough to draw from, recent enough to be what you are hearing.
const FFT_SIZE: usize = 2048;

pub struct WebAudioPlayer {
    tap: Arc<AudioTap>,
    element: Option<HtmlAudioElement>,
    graph: Option<Graph>,
    /// Whether building the graph was already tried — one element supports
    /// exactly one `createMediaElementSource`, ever, so no retries.
    graph_built: bool,

    /// What the player has been told, as opposed to what the element reports:
    /// the element flickers `paused` while `play()` settles, and the UI
    /// should show the commanded state through that.
    source: String,
    playing: bool,
    paused: bool,
    duration_hint: Option<f64>,

    /// Failures that surface inside futures (autoplay refusals), drained by
    /// `tick` like everything else.
    async_events: Rc<RefCell<VecDeque<Event>>>,
    last_tick: Option<Instant>,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    batch: Vec<f32>,
}

/// The analyser side of the routing. The element itself is the sound path
/// (source → destination); this is the copy the visualizer reads.
struct Graph {
    ctx: AudioContext,
    left: AnalyserNode,
    right: AnalyserNode,
}

impl WebAudioPlayer {
    pub fn new(tap: Arc<AudioTap>) -> Self {
        WebAudioPlayer {
            tap,
            element: None,
            graph: None,
            graph_built: false,
            source: String::new(),
            playing: false,
            paused: false,
            duration_hint: None,
            async_events: Rc::new(RefCell::new(VecDeque::new())),
            last_tick: None,
            scratch_l: vec![0.0; FFT_SIZE],
            scratch_r: vec![0.0; FFT_SIZE],
            batch: Vec::with_capacity(FFT_SIZE * 2),
        }
    }

    pub fn dispatch(&mut self, cmd: AudioCmd) {
        match cmd {
            AudioCmd::Play { url, duration_hint } => self.play(url, duration_hint),
            AudioCmd::Pause => {
                self.paused = true;
                if let Some(el) = &self.element {
                    let _ = el.pause();
                }
            }
            AudioCmd::Resume => {
                self.paused = false;
                if let Some(el) = &self.element {
                    self.watch_play(el);
                }
            }
            AudioCmd::Stop => {
                if let Some(el) = &self.element {
                    let _ = el.pause();
                    el.set_current_time(0.0);
                }
                self.source.clear();
                self.playing = false;
                self.paused = false;
                self.duration_hint = None;
                self.tap.clear();
            }
            AudioCmd::Seek(to) => {
                if let Some(el) = &self.element {
                    if self.playing && to.is_finite() {
                        el.set_current_time(to.max(0.0));
                    }
                }
            }
            AudioCmd::SetVolume(v) => {
                if let Some(el) = &self.element {
                    el.set_volume(f64::from(v.clamp(0.0, 1.0)));
                }
            }
            // The blend machinery: announcements of the next track and the
            // knobs shaping the handover. A blend needs two decks and this
            // backend is one <audio> element — the engine's crossfade stays
            // a native luxury, and the tab changes tracks the plain way.
            AudioCmd::PrepareNext { .. } | AudioCmd::ClearNext => {}
            AudioCmd::SetCrossfade(_)
            | AudioCmd::SetGapless(_)
            | AudioCmd::SetBlendSkips(_)
            | AudioCmd::SetPauseFade(_) => {}
            AudioCmd::Shutdown => {}
        }
        self.async_events.borrow_mut().push_back(Event::Status(self.status()));
    }

    /// Advance: poll the element for what actually happened since last frame,
    /// copy the analysers into the tap, and report.
    pub fn tick(&mut self) -> Vec<Event> {
        let now = Instant::now();
        let dt = match self.last_tick.replace(now) {
            Some(last) => now.duration_since(last).as_secs_f64(),
            None => 0.0,
        };

        let mut events: Vec<Event> =
            self.async_events.borrow_mut().drain(..).collect();

        if let Some(el) = &self.element {
            if self.playing {
                // A decode or network failure mid-track: report it against
                // the source it happened to, and stand down.
                if let Some(error) = el.error() {
                    let source = std::mem::take(&mut self.source);
                    self.playing = false;
                    self.tap.clear();
                    events.push(Event::PlaybackFailed {
                        source,
                        error: media_error_text(&error),
                    });
                } else if el.ended() {
                    let source = std::mem::take(&mut self.source);
                    self.playing = false;
                    self.tap.clear();
                    // TrackEnded before any status that no longer carries the
                    // source: the app checks the ending against what its last
                    // status said was playing, and a cleared status arriving
                    // first makes the end look stale — dropped, no advance,
                    // and the queue dies at the first track boundary.
                    events.push(Event::TrackEnded { source });
                    events.push(Event::Status(self.status()));
                } else if !self.paused {
                    self.feed_tap(dt);
                }
            }
        }

        events.push(Event::Status(self.status()));
        events
    }

    fn status(&self) -> PlayerStatus {
        let (position, duration, volume) = match &self.element {
            Some(el) => {
                let reported = el.duration();
                let duration = if reported.is_finite() && reported > 0.0 {
                    reported
                } else {
                    self.duration_hint.unwrap_or(0.0)
                };
                (el.current_time(), duration, el.volume() as f32)
            }
            None => (0.0, 0.0, 1.0),
        };
        PlayerStatus {
            playing: self.playing,
            paused: self.paused,
            position: if self.playing { position } else { 0.0 },
            duration: if self.playing { duration } else { 0.0 },
            volume,
            source: self.source.clone(),
        }
    }

    fn play(&mut self, url: String, duration_hint: Option<f64>) {
        let el = match self.ensure_element() {
            Ok(el) => el,
            Err(e) => {
                self.async_events
                    .borrow_mut()
                    .push_back(Event::AudioFailed(format!("no audio element: {e}")));
                return;
            }
        };

        self.source = url.clone();
        self.playing = true;
        self.paused = false;
        self.duration_hint = duration_hint;
        self.tap.clear();

        el.set_src(&url);
        el.load();
        // The context starts suspended until the page has user activation;
        // every Play arrives on a keypress, which is exactly that. Resuming
        // is idempotent, so just always ask.
        if let Some(graph) = &self.graph {
            let _ = graph.ctx.resume();
        }
        self.watch_play(&el);
    }

    /// Call `play()` and watch the promise: the browser's autoplay policy
    /// refuses pages that have never seen a real keystroke, and silence with
    /// a spinning UI is the worst possible answer. Say what to do instead.
    fn watch_play(&self, el: &HtmlAudioElement) {
        let Ok(promise) = el.play() else {
            self.async_events
                .borrow_mut()
                .push_back(Event::AudioFailed("the browser refused to start playback".into()));
            return;
        };
        let queue = self.async_events.clone();
        spawn_local(async move {
            if wasm_bindgen_futures::JsFuture::from(promise).await.is_err() {
                queue.borrow_mut().push_back(Event::AudioFailed(
                    "the browser blocked sound until you press a key — press space to resume"
                        .into(),
                ));
            }
        });
    }

    fn ensure_element(&mut self) -> Result<HtmlAudioElement, String> {
        if let Some(el) = &self.element {
            return Ok(el.clone());
        }
        let el = HtmlAudioElement::new().map_err(|e| format!("{e:?}"))?;
        el.set_preload("auto");
        self.element = Some(el.clone());
        self.build_graph(&el);
        Ok(el)
    }

    /// Route the element through an AudioContext so the analysers can read
    /// it. Best-effort: without a graph the element still sounds, the
    /// visualizer just has nothing to draw — the demo degrades to exactly
    /// what the native player does when the tap is empty.
    fn build_graph(&mut self, el: &HtmlAudioElement) {
        if self.graph_built {
            return;
        }
        self.graph_built = true;
        self.graph = try_build_graph(el);
    }

    /// Copy roughly the audio that played since last frame from the
    /// analysers into the tap. The analysers hold the most recent FFT_SIZE
    /// samples; taking the tail sized by wall-clock keeps the ring's
    /// contents close to a continuous stream rather than overlapping
    /// snapshots.
    fn feed_tap(&mut self, dt: f64) {
        let Some(graph) = &self.graph else { return };
        if graph.ctx.state() != AudioContextState::Running {
            return;
        }
        let rate = graph.ctx.sample_rate();
        let fresh = ((dt * f64::from(rate)) as usize).clamp(1, FFT_SIZE);

        graph.left.get_float_time_domain_data(&mut self.scratch_l);
        graph.right.get_float_time_domain_data(&mut self.scratch_r);

        self.batch.clear();
        for i in (FFT_SIZE - fresh)..FFT_SIZE {
            self.batch.push(self.scratch_l[i]);
            self.batch.push(self.scratch_r[i]);
        }
        self.tap.push(&self.batch, rate as u32, 2);
    }
}

fn try_build_graph(el: &HtmlAudioElement) -> Option<Graph> {
    let ctx = AudioContext::new().ok()?;
    let source = ctx.create_media_element_source(el).ok()?;
    // Once routed, the element's own output is muted — the graph *is* the
    // sound path now, so destination comes first and the taps hang off the
    // side.
    source.connect_with_audio_node(&ctx.destination()).ok()?;

    let splitter: ChannelSplitterNode =
        ctx.create_channel_splitter_with_number_of_outputs(2).ok()?;
    source.connect_with_audio_node(&splitter).ok()?;

    let left = ctx.create_analyser().ok()?;
    let right = ctx.create_analyser().ok()?;
    left.set_fft_size(FFT_SIZE as u32);
    right.set_fft_size(FFT_SIZE as u32);
    splitter.connect_with_audio_node_and_output(&left, 0).ok()?;
    splitter.connect_with_audio_node_and_output(&right, 1).ok()?;

    Some(Graph { ctx, left, right })
}

fn media_error_text(error: &web_sys::MediaError) -> String {
    let message = error.message();
    if !message.is_empty() {
        return message;
    }
    match error.code() {
        1 => "playback aborted".to_string(),
        2 => "a network error stopped the stream".to_string(),
        3 => "the browser could not decode this stream".to_string(),
        4 => "this source is not playable here".to_string(),
        other => format!("media error {other}"),
    }
}
