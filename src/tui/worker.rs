//! Background threads, so neither the network nor the audio device can stall
//! the render loop.
//!
//! Two workers:
//!   * **audio** — owns the engine (created on its own thread, since audio
//!     handles are not portable across threads), ticks it, and reports status.
//!   * **api** — owns the mStream client; every request that could block on
//!     the network happens here.
//!
//! The UI thread owns only state and rendering, and communicates by message.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use crate::api::types::{
    Album, Capabilities, DirListing, Genre, JourneyStop, Ping, PlaylistSummary, SearchResults,
    SimilarArtist, Track,
};
use crate::api::{ApiError, Client};
use crate::discovery::DiscoveredServer;
use crate::dj;
#[cfg(not(target_arch = "wasm32"))]
use crate::engine::Engine;
#[cfg(not(target_arch = "wasm32"))]
use crate::engine::tap::AudioTap;
#[cfg(not(target_arch = "wasm32"))]
use crate::player::PlayerCtl;
use crate::player::PlayerStatus;
use crate::tui::app::Tab;
use crate::tui::art;

/// How often the audio thread ticks the engine and publishes status. Also the
/// upper bound on command latency, so keep it small enough to feel instant.
#[cfg(not(target_arch = "wasm32"))]
const TICK: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, PartialEq)]
pub enum AudioCmd {
    Play { url: String, duration_hint: Option<f64> },
    Pause,
    Resume,
    Stop,
    Seek(f64),
    SetVolume(f32),
    /// Announce what should play after the current track, so a crossfade
    /// can open it ahead of the fade window. Replaces any earlier
    /// announcement; the engine treats a repeat of the same URL as a no-op,
    /// so re-announcing is always safe.
    PrepareNext { url: String, duration_hint: Option<f64> },
    /// Withdraw the announcement: nothing follows the current track.
    ClearNext,
    /// Seconds of blend between tracks; 0 is off.
    SetCrossfade(f32),
    /// Sample-tight transitions when no blend is configured.
    SetGapless(bool),
    /// Manual skips blend for a second instead of breathing.
    SetBlendSkips(bool),
    /// Pause and resume ride a short ramp instead of landing mid-wave.
    SetPauseFade(bool),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApiCmd {
    /// Use an existing token (or none, for public-mode servers).
    Connect { server: String, token: Option<String> },
    Login { server: String, username: String, password: String },
    /// Dial a Quick Connect pairing code, then treat the resulting loopback
    /// address as an ordinary server. A token is carried when reconnecting to
    /// a tunnel server we have already signed in to.
    QuickConnect { code: String, token: Option<String> },
    Browse(String),
    /// Fetch a library view for `dest` — the Library tab, or the Search tab
    /// drilling into an artist or album it found. The destination travels
    /// with the command and comes back on the event, so a second view of
    /// the same data costs a field, not a duplicated command (audit #64).
    Library { node: LibraryNode, dest: Tab },
    /// Ask for the next Auto-DJ track, seeded on what's playing now.
    AutoDj(Box<DjRequest>),
    /// Ask for several picks at once without queueing any of them, so the
    /// panel can show what the current settings actually produce.
    AutoDjSample { request: Box<DjRequest>, count: usize },
    /// Every genre in the library, for the Auto-DJ genre filter.
    Genres,
    /// Walk from one track to another through the embedding space.
    Journey { start: String, end: String, length: u32 },
    /// Fill a Discover view. `seed` is the track it all hangs off.
    Discover { node: DiscoverNode, seed: Box<Track>, dest: DiscoverDest },
    /// Write a whole track list to a playlist, creating it or replacing what
    /// was there. Sonic Path's "save as playlist" is the only caller.
    SavePlaylist { name: String, files: Vec<String> },
    Search(String),
    /// Fetch and decode one cover, named by the art file a track's metadata
    /// carries. The app caches the answer under that name.
    AlbumArt { file: String },
    /// Fetch a track's shape for the progress bar. Keyed by filepath rather
    /// than by an art file: a waveform belongs to one recording, not to an
    /// album.
    Waveform { filepath: String },
    Shutdown,
}

/// Everything needed to ask the server for an Auto-DJ pick: the mode, the
/// panel's settings, and the shape of the session so far.
#[derive(Debug, Clone, PartialEq)]
pub struct DjRequest {
    pub mode: AutoDjMode,
    pub settings: dj::Settings,
    pub seed: Option<Box<Track>>,
    pub ignore_list: Vec<u32>,
    /// Recent track paths, newest first — what the sonic pool measures from.
    pub anchors: Vec<String>,
    /// Recently-played artists, newest first, for the cooldown.
    pub recent_artists: Vec<String>,
    /// Whether the server has the embedding index at all. Without it the
    /// sonic pool must not be requested: the whole call would 403.
    pub sonic_available: bool,
}

/// A view in the Discover tab. Like [`LibraryNode`], it is both the request
/// and the identity of what comes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoverNode {
    /// What to look around *from*: what is playing, or anything you care to
    /// point at. Static, needs no request — and being able to ask about a
    /// track without playing it is why the tab starts here.
    Root,
    /// What to look *at*: songs, or artists. Also static.
    Mode,
    /// Tracks that sound like the seed.
    Tracks,
    /// Artists that sound like the seed's artist.
    Artists,
    /// One artist's ways in. Answered from the artists reply already in
    /// hand, so it costs nothing.
    Artist(String),
}

#[derive(Debug)]
pub enum DiscoverData {
    /// Neighbours with how close each one is. Both views lead their rows
    /// with the number: these arrive in order, so a position says nothing a
    /// rank could not, where the cosine says how much of a neighbour each
    /// one actually is. It used to be dropped on the way to the browser
    /// tab, which is why this carries `SimilarTrack` and not `Track`.
    Tracks(Vec<crate::api::types::SimilarTrack>),
    Artists(Vec<SimilarArtist>),
}

/// Which Discover surface asked, echoed back on the reply.
///
/// Two of them want the same data about different seeds: the browser tab
/// drills from a seed it captured when you opened it, and the now-playing
/// panel follows whatever is on the speakers. Carrying the destination is
/// what audit #64 asks for — the alternative is a second command whose only
/// job is to be a different variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverDest {
    /// The Discover tab in the browser.
    Browser,
    /// The Discover tab of the full-screen view.
    NowPlaying,
}

/// A position in the tag-based library hierarchy. Doubles as the request (what
/// to fetch) and the identity of a view (what a response belongs to), so a
/// slow reply for a screen the user already left can be discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryNode {
    /// The mode menu — static, needs no request.
    Root,
    Artists,
    Artist(String),
    Albums,
    Album { name: String, artist: Option<String> },
    Genres,
    Genre(String),
    Recent,
    /// Your own lists, which are a way of browsing the library like any
    /// other — they were a tab of their own until they turned out to need
    /// every machine the Library tab already had.
    Playlists,
    Playlist(String),
}

#[derive(Debug)]
pub enum LibraryData {
    Artists(Vec<String>),
    Albums(Vec<Album>),
    Genres(Vec<Genre>),
    Playlists(Vec<PlaylistSummary>),
    Tracks(Vec<Track>),
}

/// How Auto-DJ chooses what comes next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoDjMode {
    #[default]
    Off,
    /// Nearest neighbours in the server's audio-embedding space.
    Similar,
    /// Harmonically and rhythmically compatible: Camelot-adjacent keys and
    /// tempo windows around the current track (including half/double time).
    BpmKey,
}

impl AutoDjMode {
    /// Cycle to the next mode this server can actually deliver.
    ///
    /// Offering a mode that would immediately fall back to a different one
    /// wastes a keystroke and misreports what the player is doing.
    pub fn next_available(self, caps: Capabilities) -> Self {
        let next = self.next();
        if next == AutoDjMode::Similar && !caps.discovery {
            // Only ever one hop: Off and BpmKey need nothing from the server.
            return next.next();
        }
        next
    }

    /// The same ring, leftwards. Walks forward until the lap closes rather
    /// than hopping twice: two hops is only "back" when all three modes are
    /// on offer, and without discovery the ring is two long.
    pub fn prev_available(self, caps: Capabilities) -> Self {
        let mut at = self;
        // Bounded by the number of modes rather than by getting home, so a
        // mode this server cannot offer — which nothing walks back round
        // to — settles on something available instead of spinning.
        for _ in 0..3 {
            let next = at.next_available(caps);
            if next == self {
                break;
            }
            at = next;
        }
        at
    }

    /// Whether this mode can work against the given server.
    pub fn available(self, caps: Capabilities) -> bool {
        self != AutoDjMode::Similar || caps.discovery
    }

    pub fn next(self) -> Self {
        match self {
            AutoDjMode::Off => AutoDjMode::Similar,
            AutoDjMode::Similar => AutoDjMode::BpmKey,
            AutoDjMode::BpmKey => AutoDjMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AutoDjMode::Off => "off",
            AutoDjMode::Similar => "similar",
            AutoDjMode::BpmKey => "tempo+key",
        }
    }

    /// Anything unrecognised falls back to off rather than refusing to start.
    pub fn from_label(label: &str) -> Self {
        match label {
            "similar" => AutoDjMode::Similar,
            "tempo+key" => AutoDjMode::BpmKey,
            _ => AutoDjMode::Off,
        }
    }
}

/// How many tracks "Recently Added" asks for.
const RECENT_LIMIT: u32 = 100;

/// Candidates to request from the similarity index. More than one because the
/// nearest neighbour is often already sitting in the queue.
const SIMILAR_LIMIT: u32 = 15;

#[derive(Debug)]
pub enum Event {
    Status(PlayerStatus),
    /// A track finished on its own (not a user stop), named by the source
    /// that ran out. The name is what lets the UI tell this from the end of a
    /// track it has already moved past.
    TrackEnded { source: String },
    /// The engine crossfaded into the announced next track by itself: the
    /// source changed with no Play asked for and nothing ran out. The UI
    /// moves its cursor without starting anything — the audio has already
    /// moved.
    HandedOver { from: String, to: String },
    /// The audio device could not be opened; playback is unavailable.
    AudioFailed(String),
    /// How the Quick Connect tunnel is reaching the server right now —
    /// direct, through a relay, or between tunnels. Sent on change by a
    /// sampler that lives exactly as long as the bridge does.
    TunnelPath(crate::quickconnect::TunnelPath),
    /// One source would not play — wrong format, gone from the server, or
    /// something this decoder doesn't speak. The rest of the queue is fine.
    /// Named for the same reason [`Event::TrackEnded`] is, and more urgently:
    /// an open can take as long as the network does to give up.
    PlaybackFailed { source: String, error: String },
    Connected {
        /// Where this session's requests go. For a tunnel this is the loopback
        /// bridge, which is exactly why it cannot also be the identity.
        server: String,
        /// What to remember the server as: the same URL for a direct
        /// connection, a `mstream+iroh://` identity for a tunnel.
        id: String,
        username: Option<String>,
        token: Option<String>,
        ping: Box<Ping>,
    },
    /// Servers that answered an mDNS browse.
    ServersDiscovered(Vec<DiscoveredServer>),
    /// We reached this server but it wants credentials. Distinct from
    /// [`Event::Unauthorized`], which means an established session went bad.
    NeedsLogin { server: String },
    /// The Quick Connect tunnel is up and reachable at `local_url`, but the
    /// server still wants credentials — the secret gates the pipe, not the API.
    TunnelReady { local_url: String, id: String },
    Listing(Box<DirListing>),
    /// Contents of a library view, tagged with the node they belong to and
    /// the tab they were fetched for — the same data serves the Library tab
    /// and a drill out of the search results, and carrying the destination
    /// is what replaced a wholesale second command and event (audit #64).
    Library { node: LibraryNode, dest: Tab, data: LibraryData },
    /// Auto-DJ candidates, best first. `note` explains any fallback that had
    /// to happen so the UI can say so out loud.
    AutoDjPick { candidates: Vec<Track>, ignore_list: Vec<u32>, note: Option<String> },
    /// What the current Auto-DJ settings produce, for the panel. Carries the
    /// sonic report when there was one — the pool size is the number that
    /// makes the tightness slider tunable.
    AutoDjSample {
        tracks: Vec<Track>,
        pool: Option<crate::api::types::SonicReport>,
        note: Option<String>,
    },
    /// Every genre in the library.
    Genres(Vec<Genre>),
    /// A journey's stops, in order. `note` explains a short or empty arc —
    /// both are answers the server gives deliberately rather than failures.
    /// `length` names the request this answers, since asking for a longer
    /// arc while one is still in flight is a race the UI can lose.
    Journey { stops: Vec<JourneyStop>, note: Option<String>, length: u32 },
    /// A Discover view's contents, tagged with the node they belong to.
    /// `seed` is the filepath it was asked about. The browser tab tells a
    /// stale reply by its node; the now-playing panel follows the speakers,
    /// where the node never changes and the seed is the only thing that does.
    Discover {
        node: DiscoverNode,
        data: DiscoverData,
        note: Option<String>,
        dest: DiscoverDest,
        seed: String,
    },
    /// A playlist was written. Carries the name so the confirmation can say
    /// which one, and how many tracks went into it.
    PlaylistSaved { name: String, count: usize },
    /// `query` is the search these results answer — replies can pass each
    /// other now, and the box's contents name the one still wanted.
    SearchResults { query: String, results: Box<SearchResults> },
    /// A cover, decoded and shrunk to terminal scale — or `None` for any
    /// kind of failure. Art is a nicety: nothing about it is ever worth a
    /// message the user has to read.
    AlbumArt { file: String, art: Option<art::Art> },
    /// A track's shape, or `None` for every flavour of "there isn't one".
    /// Like art, never worth a message: the bar it decorates draws perfectly
    /// well without it.
    /// The shape of a track, or the news that it has none.
    ///
    /// `settled` is the difference between the server answering "no
    /// waveform" — which it will answer the same way forever, so the answer
    /// is worth keeping — and nobody answering at all. Collapsing the two
    /// meant one dropped connection cached a permanent "this track has no
    /// shape", on the endpoint whose whole design assumes the first call is
    /// the slow one.
    Waveform { filepath: String, bars: Option<Vec<u8>>, settled: bool },
    /// Credentials are missing or expired — the UI drops back to the
    /// connect screen.
    Unauthorized,
    Error(String),
}

// ── Audio thread ────────────────────────────────────────────────────────────

/// The audio thread's name — also how the panic hook recognises it.
pub(crate) const AUDIO_THREAD: &str = "mstream-audio";

/// Whether a panicking thread cleans up after itself. The audio loop runs
/// under an unwind guard and reports its own death as
/// [`Event::AudioFailed`], so the process-wide hook must stand back for it:
/// "recovering" the terminal there would tear the screen down under a UI
/// that is still running (audit #32). The crossfade prepare thread is the
/// same story with a smaller blast radius — its panics are caught at the
/// spawn and read as a failed open, so a malformed file costs a blend, not
/// the terminal.
pub fn panics_are_caught(thread: Option<&str>) -> bool {
    thread == Some(AUDIO_THREAD) || thread == Some(crate::engine::PREPARE_THREAD)
}

/// Returns the tap alongside the command channel: the engine is built on the
/// audio thread, so the UI cannot reach in for it afterwards, but the tap
/// itself is just a buffer and can be made here and handed to both.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_audio(events: Sender<Event>) -> (Sender<AudioCmd>, Arc<AudioTap>) {
    let (tx, rx) = mpsc::channel();
    let tap = AudioTap::new();
    let theirs = tap.clone();
    thread::Builder::new()
        .name(AUDIO_THREAD.into())
        .spawn(move || audio_loop(&rx, &events, theirs))
        .expect("failed to spawn audio thread");
    (tx, tap)
}

/// Keep answering the door so the UI's sends never error; the player stays
/// usable for browsing with no audio at all.
#[cfg(not(target_arch = "wasm32"))]
fn drain_until_shutdown(rx: &Receiver<AudioCmd>) {
    while let Ok(cmd) = rx.recv() {
        if cmd == AudioCmd::Shutdown {
            break;
        }
    }
}

/// The words inside a panic payload, if it carried any.
#[cfg(not(target_arch = "wasm32"))]
fn panic_note(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message")
}

/// Boil a burst of queued commands down to what it all amounted to.
///
/// A Play holds this thread through a whole open and format probe, so a run
/// of them — someone leaning on `n` through remote tracks — used to be paid
/// for one doomed fetch at a time, with every later command waiting in line
/// behind opens for tracks nobody wanted any more (audit #50).
///
/// What survives: the last Play or Stop decides the transport, and anything
/// transport-shaped before it was about a source that is gone by the end of
/// the batch. After the decider, pauses and resumes are kept in order, only
/// the last Seek matters, and only the last announcement (PrepareNext or
/// ClearNext) — an announcement before the decider was about a track the
/// batch has already moved past. Volume and the crossfade length are
/// sticky, so the last of each is kept wherever it was said. A Shutdown
/// makes everything else moot.
#[cfg(not(target_arch = "wasm32"))]
fn collapse(batch: Vec<AudioCmd>) -> Vec<AudioCmd> {
    if batch.contains(&AudioCmd::Shutdown) {
        return vec![AudioCmd::Shutdown];
    }
    let decider =
        batch.iter().rposition(|cmd| matches!(cmd, AudioCmd::Play { .. } | AudioCmd::Stop));
    let volume = batch.iter().rev().find(|cmd| matches!(cmd, AudioCmd::SetVolume(_)));
    let crossfade = batch.iter().rev().find(|cmd| matches!(cmd, AudioCmd::SetCrossfade(_)));
    let gapless = batch.iter().rev().find(|cmd| matches!(cmd, AudioCmd::SetGapless(_)));
    let blend_skips = batch.iter().rev().find(|cmd| matches!(cmd, AudioCmd::SetBlendSkips(_)));
    let pause_fade = batch.iter().rev().find(|cmd| matches!(cmd, AudioCmd::SetPauseFade(_)));

    let mut kept: Vec<AudioCmd> = Vec::new();
    kept.extend(volume.cloned());
    kept.extend(crossfade.cloned());
    kept.extend(gapless.cloned());
    kept.extend(blend_skips.cloned());
    kept.extend(pause_fade.cloned());
    if let Some(at) = decider {
        kept.push(batch[at].clone());
    }
    let mut seek: Option<&AudioCmd> = None;
    let mut announced: Option<&AudioCmd> = None;
    for cmd in &batch[decider.map_or(0, |at| at + 1)..] {
        match cmd {
            AudioCmd::Pause | AudioCmd::Resume => kept.push(cmd.clone()),
            AudioCmd::Seek(_) => seek = Some(cmd),
            AudioCmd::PrepareNext { .. } | AudioCmd::ClearNext => announced = Some(cmd),
            _ => {}
        }
    }
    kept.extend(seek.cloned());
    kept.extend(announced.cloned());
    kept
}

#[cfg(not(target_arch = "wasm32"))]
fn audio_loop(rx: &Receiver<AudioCmd>, events: &Sender<Event>, tap: Arc<AudioTap>) {
    let engine = match Engine::new() {
        Ok(e) => e,
        Err(e) => {
            let _ = events.send(Event::AudioFailed(e.to_string()));
            drain_until_shutdown(rx);
            return;
        }
    };
    engine.attach_tap(tap);
    listen_guarded(&engine, rx, events);
}

/// Run the command loop under an unwind guard: symphonia has known panics
/// on malformed files, and uncaught, one killed this thread — the global
/// hook then restored the terminal under the still-running UI, and every
/// later command vanished into a dead channel with nothing said (audit
/// #32). Caught, it is just a worse kind of [`Event::AudioFailed`]: the
/// same event, and the same degraded-but-browsable player the no-device
/// path has always produced.
#[cfg(not(target_arch = "wasm32"))]
fn listen_guarded(player: &dyn PlayerCtl, rx: &Receiver<AudioCmd>, events: &Sender<Event>) {
    // The player is never touched again after a caught panic — whatever it
    // was mid-way through stays where it fell — which is what makes the
    // unwind-safety assertion honest.
    let listened =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listen(player, rx, events)));
    if let Err(panic) = listened {
        let _ = events.send(Event::AudioFailed(format!(
            "the audio engine crashed: {}",
            panic_note(panic.as_ref())
        )));
        drain_until_shutdown(rx);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn listen(player: &dyn PlayerCtl, rx: &Receiver<AudioCmd>, events: &Sender<Event>) {
    let mut watch = EndWatch::default();

    'listening: loop {
        let batch = match rx.recv_timeout(TICK) {
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => Vec::new(),
            Ok(first) => {
                // Whatever else has queued up is taken now and boiled down,
                // rather than paid for one blocking open at a time.
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                collapse(batch)
            }
        };
        for cmd in batch {
            if cmd == AudioCmd::Shutdown {
                break 'listening;
            }
            watch.note(&cmd);
            // A source that won't play is a different kind of problem
            // from a command that failed: the queue can carry on past it,
            // and should, so it gets its own event. It goes out under the
            // name of the source that would not play, because a play is
            // the one command that can sit here long enough for the
            // answer to be about a track nobody is waiting for any more.
            let starting = match &cmd {
                AudioCmd::Play { url, .. } => Some(url.clone()),
                _ => None,
            };
            if let Some(err) = apply_audio_cmd(player, cmd) {
                let event = match starting {
                    Some(source) => {
                        watch.play_failed(&source);
                        Event::PlaybackFailed { source, error: err }
                    }
                    None => Event::Error(err),
                };
                let _ = events.send(event);
            }
        }

        player.tick();
        let status = player.status();
        // Sent before the status that reports the transition, and down the
        // same channel, so the UI always learns which track ended — or which
        // one the engine blended into — before the status says so.
        if let Some(passing) = watch.observe(&status.source) {
            let event = match passing {
                Passing::Ended(source) => Event::TrackEnded { source },
                Passing::HandedOver { from, to } => Event::HandedOver { from, to },
            };
            let _ = events.send(event);
        }

        if events.send(Event::Status(status)).is_err() {
            break; // UI is gone
        }
    }

    player.stop();
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_audio_cmd(player: &dyn PlayerCtl, cmd: AudioCmd) -> Option<String> {
    match cmd {
        AudioCmd::Play { url, duration_hint } => return player.play(&url, duration_hint).err(),
        AudioCmd::Pause => player.pause(),
        AudioCmd::Resume => player.resume(),
        AudioCmd::Stop => player.stop(),
        AudioCmd::Seek(pos) => return player.seek(pos).err(),
        AudioCmd::SetVolume(v) => player.set_volume(v),
        AudioCmd::SetCrossfade(seconds) => player.set_crossfade(seconds),
        AudioCmd::SetGapless(on) => player.set_gapless(on),
        AudioCmd::SetBlendSkips(on) => player.set_blend_skips(on),
        AudioCmd::SetPauseFade(on) => player.set_pause_fade(on),
        AudioCmd::PrepareNext { url, duration_hint } => player.prepare_next(&url, duration_hint),
        AudioCmd::ClearNext => player.clear_next(),
        AudioCmd::Shutdown => {}
    }
    None
}

/// What a status transition meant, when it meant something.
#[derive(Debug, PartialEq)]
enum Passing {
    /// A source ran out on its own.
    Ended(String),
    /// The engine crossfaded from one source into another by itself.
    HandedOver { from: String, to: String },
}

/// Which source went away on its own, if one did — and since crossfade,
/// which one the engine moved to on its own.
///
/// Track-end detection lives on this thread rather than in the UI: it sees
/// every status transition, so it can tell "the track finished" from "the
/// user pressed stop" without the UI having to infer it from polling.
///
/// `asked_to_stop` is a standing answer, not a one-shot flag, and that is the
/// whole point. It used to be armed by anything that *might* empty the source
/// and disarmed by the next transition — but a play never empties anything
/// (the engine decodes the new source, then swaps sinks, with a file loaded
/// throughout). So the flag went up at the start of every track and was still
/// up when that track ended, where it ate the one transition it was never
/// meant to cover. Playback stopped after a single song, every time.
///
/// `expecting` is what separates the two ways a source can change under a
/// running player: a Play this thread performed (the UI already moved), and
/// a blend handover the engine performed alone (the UI must be told). It is
/// cleared when the expected source arrives — or when its play fails, so a
/// doomed open cannot masquerade as a later handover's excuse.
#[derive(Default)]
#[cfg(not(target_arch = "wasm32"))]
struct EndWatch {
    /// The source last seen loaded. Kept rather than a bare "there was one"
    /// because it is the only place the name still exists when the end is
    /// noticed: the status that spotted it is the one reporting nothing.
    source: String,
    asked_to_stop: bool,
    /// The source a Play command promised, until it shows up.
    expecting: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl EndWatch {
    /// Note what a command asks of playback. Only a stop can account for a
    /// source disappearing, and only a play for one source becoming another;
    /// everything else acts on the source already loaded.
    fn note(&mut self, cmd: &AudioCmd) {
        match cmd {
            AudioCmd::Stop => {
                self.asked_to_stop = true;
                self.expecting = None;
            }
            AudioCmd::Play { url, .. } => {
                self.asked_to_stop = false;
                self.expecting = Some(url.clone());
            }
            _ => {}
        }
    }

    /// The play that was promised is not coming; stop watching for it.
    fn play_failed(&mut self, source: &str) {
        if self.expecting.as_deref() == Some(source) {
            self.expecting = None;
        }
    }

    /// What this status transition amounted to, if anything.
    fn observe(&mut self, now: &str) -> Option<Passing> {
        let was = std::mem::replace(&mut self.source, now.to_string());
        if now.is_empty() {
            return (!was.is_empty() && !self.asked_to_stop).then(|| Passing::Ended(was));
        }
        if self.expecting.as_deref() == Some(now) {
            // The start we asked for arrived; nothing to report.
            self.expecting = None;
            return None;
        }
        if !was.is_empty() && was != now {
            return Some(Passing::HandedOver { from: was, to: now.to_string() });
        }
        None
    }
}

/// Browse for servers on its own thread — mDNS listens for a fixed window, and
/// that shouldn't hold up a pairing attempt queued behind it.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_discovery(events: Sender<Event>) {
    thread::Builder::new()
        .name("mstream-mdns".into())
        .spawn(move || {
            let found = crate::discovery::browse(DISCOVERY_WINDOW).unwrap_or_default();
            let _ = events.send(Event::ServersDiscovered(found));
        })
        .ok();
}

/// How long to listen for adverts. Long enough for a quiet network to answer,
/// short enough not to feel stuck.
#[cfg(not(target_arch = "wasm32"))]
const DISCOVERY_WINDOW: Duration = Duration::from_secs(3);

// ── API thread ──────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_api(events: Sender<Event>) -> Sender<ApiCmd> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("mstream-api".into())
        .spawn(move || api_loop(&rx, &events))
        .expect("failed to spawn api thread");
    tx
}

#[cfg(not(target_arch = "wasm32"))]
fn api_loop(rx: &Receiver<ApiCmd>, events: &Sender<Event>) {
    let mut client: Option<Arc<Client>> = None;
    // Held for as long as this thread lives; dropping it closes the tunnel out
    // from under the client, so it is explicitly dropped on the way out. An
    // Arc so the path sampler can watch it without owning it.
    #[allow(unused_assignments)]
    let mut bridge: Option<Arc<crate::quickconnect::TunnelBridge>> = None;
    // The tunnel session's two names, once one is open: the loopback address
    // requests go to, and the identity it is remembered by.
    let mut tunnel: Option<(String, String)> = None;
    // What the connected server said it can do. Nothing optional is probed
    // before this says so.
    let mut caps = Capabilities::default();

    while let Ok(cmd) = rx.recv() {
        // Connection commands change who `client` *is*, so they stay
        // serialized here — reaching a different server mid-dial is a
        // contradiction, not a feature. Everything else is a read against
        // the current client and answers on its own thread (audit #63):
        // one stalled search used to block every pane behind a 20-second
        // timeout, and a tunnel dial held the line for the better part of
        // a minute.
        let result = match cmd {
            ApiCmd::Shutdown => break,

            ApiCmd::Connect { server, token } => {
                connect(&mut client, &server, &server.clone(), token)
            }

            ApiCmd::Login { server, username, password } => {
                // Signing in to a tunnel server goes over the open bridge, but
                // is filed under the endpoint id — the loopback port is gone
                // by the next run.
                let (endpoint, id) = resolve_target(&server, tunnel.as_ref());
                login(&mut client, &endpoint, &id, &username, &password)
            }

            ApiCmd::QuickConnect { code, token } => match quick_connect(&code) {
                Ok((id, opened)) => {
                    let url = opened.local_url.clone();
                    // Dial over the new tunnel while the old one is still up.
                    // Installing it here would drop the old bridge, and its
                    // Drop closes the loopback listener the *current* session
                    // is streaming through — so a code that opens but doesn't
                    // answer used to leave the UI on a session whose port had
                    // just been pulled out from under it (finding #20).
                    let answer = connect(&mut client, &url, &id, token);
                    if !tunnel_answered(&answer) {
                        // `opened` drops here, closing the tunnel that just
                        // failed and only that one. `client`, `bridge` and
                        // `tunnel` are untouched, so the session the user is
                        // on carries on working while they read the error.
                        answer
                    } else {
                        let opened = Arc::new(opened);
                        spawn_path_sampler(Arc::downgrade(&opened), events.clone());
                        bridge = Some(opened);
                        tunnel = Some((url.clone(), id.clone()));
                        // A public-mode server answers straight away; anything
                        // else needs a login over the freshly-opened tunnel.
                        match answer {
                            Some(Event::NeedsLogin { .. }) => {
                                Some(Event::TunnelReady { local_url: url, id })
                            }
                            other => other,
                        }
                    }
                }
                Err(e) => Some(Event::Error(e)),
            },


            read => {
                spawn_read(client.clone(), caps, events.clone(), read);
                None
            }

        };

        // One place to learn what the server offers, so a new way of
        // connecting can't forget to ask.
        if let Some(Event::Connected { ping, .. }) = &result {
            caps = Capabilities::from(ping.as_ref());
        }

        if let Some(event) = result {
            if events.send(event).is_err() {
                break;
            }
        }
    }

    drop(bridge);
}

/// Answer one read on its own thread, so a slow server holds up this reply
/// and nothing else. In-flight replies against a client that has since been
/// replaced still arrive; the app's stale-reply guards are what drop them,
/// the same as any other answer about somewhere the user no longer is.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_read(
    client: Option<Arc<Client>>,
    caps: Capabilities,
    events: Sender<Event>,
    cmd: ApiCmd,
) {
    thread::Builder::new()
        .name("mstream-api-read".into())
        .spawn(move || {
            let event = answer(client.as_deref(), caps, cmd);
            let _ = events.send(event);
        })
        .ok();
}

/// One read, answered. Failures map onto events here: only 401 means the
/// session is no good; 403 is a permission or feature-flag answer that
/// shouldn't bounce the user to a login form.
#[cfg(not(target_arch = "wasm32"))]
fn answer(client: Option<&Client>, caps: Capabilities, cmd: ApiCmd) -> Event {
    let Some(c) = client else {
        return Event::Error("not connected to a server".into());
    };
    let answered = match cmd {
        ApiCmd::Browse(path) => {
            c.file_explorer(&path).map(|l| Event::Listing(Box::new(l)))
        }
        ApiCmd::Library { node, dest } => crate::api::wait(load_library(c, &node))
            .map(|data| Event::Library { node, dest, data }),
        ApiCmd::AutoDj(request) => {
            crate::api::wait(autodj_pick(c, caps, &request)).map(|picked| Event::AutoDjPick {
                candidates: picked.tracks,
                ignore_list: picked.ignore_list,
                note: picked.note,
            })
        }
        ApiCmd::AutoDjSample { request, count } => {
            crate::api::wait(autodj_sample(c, caps, &request, count))
        }
        ApiCmd::Genres => c.genres().map(Event::Genres),
        ApiCmd::Journey { start, end, length } => {
            crate::api::wait(journey(c, &start, &end, length))
        }
        ApiCmd::Discover { node, seed, dest } => {
            crate::api::wait(discover(c, &node, &seed, dest))
        }
        ApiCmd::SavePlaylist { name, files } => {
            let count = files.len();
            c.playlist_save(&name, &files).map(|()| Event::PlaylistSaved { name, count })
        }
        ApiCmd::Search(query) => {
            c.search(&query).map(|r| Event::SearchResults { query, results: Box::new(r) })
        }
        ApiCmd::AlbumArt { file } => {
            // Every failure is "no art" — a fetch that 404s, bytes that
            // don't decode, even a dead session, which the next real
            // request will report in its own voice. Decoded here so the
            // render loop only ever meets covers already at terminal scale.
            let art = c.album_art(&file).ok().and_then(|bytes| art::decode(&bytes));
            Ok(Event::AlbumArt { file, art })
        }
        ApiCmd::Waveform { filepath } => {
            // Same rule as art: a shape nobody could draw is not news. The
            // client already folds the server's four ways of saying "no
            // waveform" into `Ok(None)`; anything left is a real transport
            // failure, which is worth less than a message here — but it is
            // not an answer, so it must not be remembered as one.
            let answer = c.waveform(&filepath);
            let settled = answer.is_ok();
            Ok(Event::Waveform { filepath, bars: answer.ok().flatten(), settled })
        }
        // The connection commands never reach here; api_loop keeps them.
        ApiCmd::Connect { .. }
        | ApiCmd::Login { .. }
        | ApiCmd::QuickConnect { .. }
        | ApiCmd::Shutdown => return Event::Error("connection change routed as a read".into()),
    };
    match answered {
        Ok(event) => event,
        Err(ApiError::Unauthorized) => Event::Unauthorized,
        Err(e) => Event::Error(e.to_string()),
    }
}

/// Watch how the tunnel is reaching the server and tell the UI when it
/// changes. Holds only a Weak: when the bridge is dropped (a new session,
/// shutdown), the next sample fails to upgrade and the thread ends. The
/// first sample is sent unconditionally so a fresh session shows its state
/// within a beat of connecting.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_path_sampler(
    bridge: std::sync::Weak<crate::quickconnect::TunnelBridge>,
    events: Sender<Event>,
) {
    let _ = thread::Builder::new().name("mstream-tunnel-path".into()).spawn(move || {
        let mut last: Option<crate::quickconnect::TunnelPath> = None;
        loop {
            let Some(bridge) = bridge.upgrade() else { return };
            let path = bridge.path();
            drop(bridge);
            if last != Some(path) {
                last = Some(path);
                if events.send(Event::TunnelPath(path)).is_err() {
                    return;
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Parse a pairing code, bring the tunnel up on loopback, and report the
/// identity the code names alongside it.
#[cfg(not(target_arch = "wasm32"))]
fn quick_connect(code: &str) -> Result<(String, crate::quickconnect::TunnelBridge), String> {
    let parsed = crate::quickconnect::parse_code(code)?;
    let id = parsed.server_id();
    Ok((id, crate::quickconnect::open_bridge(&parsed)?))
}

/// Split a connect target into (where to send bytes, what to remember it as).
/// They differ only for a tunnel, which the UI names either way round: by its
/// identity when reconnecting, by the loopback URL when the login form is
/// carrying what the tunnel just published.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_target(server: &str, tunnel: Option<&(String, String)>) -> (String, String) {
    match tunnel {
        Some((local_url, id)) if server == id || server == local_url => {
            (local_url.clone(), id.clone())
        }
        _ => (server.to_string(), server.to_string()),
    }
}

/// Whether a dial reached the server it was aimed at.
///
/// An allowlist rather than "not an error", because this decides whether a
/// working tunnel gets torn down: an outcome nobody has thought about yet
/// should keep the session that is already up, not replace it.
#[cfg(not(target_arch = "wasm32"))]
fn tunnel_answered(answer: &Option<Event>) -> bool {
    matches!(answer, Some(Event::Connected { .. } | Event::NeedsLogin { .. }))
}

/// The tail both ways in share: ping the server, and only once it answers
/// make its client the one every later read goes through.
///
/// Installing the client before the ping would leave a session pointing at a
/// server that never replied, so the order here is the point.
#[cfg(not(target_arch = "wasm32"))]
fn establish(
    client: &mut Option<Arc<Client>>,
    c: Client,
    id: &str,
    username: Option<String>,
    token: Option<String>,
) -> Result<Event, ApiError> {
    let ping = c.ping()?;
    let server = c.server();
    *client = Some(Arc::new(c));
    Ok(Event::Connected { server, id: id.to_string(), username, token, ping: Box::new(ping) })
}

#[cfg(not(target_arch = "wasm32"))]
fn connect(
    client: &mut Option<Arc<Client>>,
    server: &str,
    id: &str,
    token: Option<String>,
) -> Option<Event> {
    let c = match Client::new(server) {
        Ok(c) => c.with_token(token.clone()),
        Err(e) => return Some(Event::Error(e.to_string())),
    };
    // Taken before the client moves; it is the address that was reached,
    // which is not always the string that was asked for.
    let reached = c.server();
    match establish(client, c, id, None, token) {
        Ok(event) => Some(event),
        // Reaching the server and being asked to sign in is a normal outcome
        // of picking one, not an authorization failure.
        Err(ApiError::Unauthorized) => Some(Event::NeedsLogin { server: reached }),
        Err(e) => Some(Event::Error(e.to_string())),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn login(
    client: &mut Option<Arc<Client>>,
    server: &str,
    id: &str,
    username: &str,
    password: &str,
) -> Option<Event> {
    let mut c = match Client::new(server) {
        Ok(c) => c,
        Err(e) => return Some(Event::Error(e.to_string())),
    };
    let token = match c.login(username, password) {
        Ok(resp) => resp.token,
        Err(ApiError::Unauthorized) => {
            return Some(Event::Error("login failed — check the username and password".into()));
        }
        Err(e) => return Some(Event::Error(e.to_string())),
    };
    match establish(client, c, id, Some(username.to_string()), Some(token)) {
        Ok(event) => Some(event),
        Err(e) => Some(Event::Error(e.to_string())),
    }
}

/// Shared by the native api thread (via `api::wait`) and the web worker
/// (awaited on the browser's event loop) — as is everything below it that
/// takes a `&Client`. One brain, two drivers.
pub(crate) async fn load_library(
    client: &Client,
    node: &LibraryNode,
) -> Result<LibraryData, ApiError> {
    Ok(match node {
        // The mode menu is static; the UI fills it in without asking.
        LibraryNode::Root => LibraryData::Artists(Vec::new()),
        LibraryNode::Artists => LibraryData::Artists(client.artists_async().await?),
        LibraryNode::Artist(artist) => {
            LibraryData::Albums(client.artist_albums_async(artist).await?)
        }
        LibraryNode::Albums => LibraryData::Albums(client.albums_async().await?),
        LibraryNode::Album { name, artist } => {
            LibraryData::Tracks(client.album_songs_async(name, artist.as_deref()).await?)
        }
        LibraryNode::Genres => LibraryData::Genres(client.genres_async().await?),
        LibraryNode::Genre(genre) => {
            LibraryData::Tracks(client.genre_songs_async(genre).await?)
        }
        LibraryNode::Recent => {
            LibraryData::Tracks(client.recently_added_async(RECENT_LIMIT).await?)
        }
        LibraryNode::Playlists => LibraryData::Playlists(client.playlists_async().await?),
        LibraryNode::Playlist(name) => {
            LibraryData::Tracks(client.playlist_load_async(name).await?)
        }
    })
}

/// One answer from the picker.
pub(crate) struct Picked {
    pub(crate) tracks: Vec<Track>,
    pub(crate) ignore_list: Vec<u32>,
    pub(crate) note: Option<String>,
    pool: Option<crate::api::types::SonicReport>,
}

type AutoDjResult = Result<Picked, ApiError>;

/// Choose what Auto-DJ should play next.
///
/// Similarity is best-effort: the server may have discovery switched off, or
/// simply not have embedded this track yet. Rather than stalling, both cases
/// fall through to tempo/key matching and say why.
pub(crate) async fn autodj_pick(
    client: &Client,
    caps: Capabilities,
    request: &DjRequest,
) -> AutoDjResult {
    let ignore_list = request.ignore_list.clone();
    match request.mode {
        AutoDjMode::Off => {
            Ok(Picked { tracks: Vec::new(), ignore_list, note: None, pool: None })
        }

        AutoDjMode::BpmKey => pick_by_tempo_and_key(client, request, None).await,

        AutoDjMode::Similar => {
            let Some(seed) = request.seed.as_deref() else {
                return pick_by_tempo_and_key(client, request, None).await;
            };
            // No flag, no probe: ping already said there is no index here, so
            // asking would spend a round trip to be told 403.
            if !caps.discovery {
                return pick_by_tempo_and_key(
                    client,
                    request,
                    Some("this server has no similarity index — matching tempo and key"),
                )
                .await;
            }
            match client.similar_tracks_async(&seed.filepath, SIMILAR_LIMIT).await? {
                // Backstop for a server reconfigured mid-session; the flag
                // above is what normally keeps us out of here.
                None => {
                    pick_by_tempo_and_key(
                        client,
                        request,
                        Some("similarity was switched off on this server — matching tempo and key"),
                    )
                    .await
                }
                Some(found) if found.not_analyzed => {
                    pick_by_tempo_and_key(
                        client,
                        request,
                        Some("this track hasn't been analysed yet — matching tempo and key"),
                    )
                    .await
                }
                Some(found) if found.results.is_empty() => {
                    pick_by_tempo_and_key(
                        client,
                        request,
                        Some("nothing sounded similar — matching tempo and key"),
                    )
                    .await
                }
                Some(found) => Ok(Picked {
                    tracks: found.results.into_iter().map(|r| r.into_track()).collect(),
                    ignore_list,
                    note: None,
                    pool: None,
                }),
            }
        }
    }
}

async fn pick_by_tempo_and_key(
    client: &Client,
    request: &DjRequest,
    note: Option<&str>,
) -> AutoDjResult {
    let (body, tag_note) = dj::build_random_request(
        &request.settings,
        request.seed.as_deref(),
        request.ignore_list.clone(),
        &request.anchors,
        &request.recent_artists,
        request.sonic_available,
    );
    let sonic_asked = body.min_similarity.is_some();

    let response = match client.random_song_async(&body).await {
        Ok(response) => response,
        // A hard sonic pool fails loudly by design — the server would rather
        // say "nothing is that similar" than quietly play something that
        // isn't. It answers 400 for both an empty pool and a seed it hasn't
        // analysed. Retry once without the pool so the session keeps moving,
        // and say what happened rather than leaving the queue to run dry.
        Err(ApiError::Server { status: 400, message }) if sonic_asked => {
            let (relaxed, _) = dj::build_random_request(
                &request.settings,
                request.seed.as_deref(),
                request.ignore_list.clone(),
                &request.anchors,
                &request.recent_artists,
                false,
            );
            let response = client.random_song_async(&relaxed).await?;
            return Ok(Picked {
                tracks: response.songs,
                ignore_list: response.ignore_list,
                note: Some(format!("{} — loosen the sonic pool", trim_period(&message))),
                pool: None,
            });
        }
        Err(e) => return Err(e),
    };

    Ok(Picked {
        tracks: response.songs,
        ignore_list: response.ignore_list,
        note: note.map(str::to_string).or(tag_note),
        pool: response.sonic,
    })
}

/// Take several picks in a row without committing to any of them, feeding
/// each back into the next call's cooldown so the sample shows variety rather
/// than the same track three times.
pub(crate) async fn autodj_sample(
    client: &Client,
    caps: Capabilities,
    request: &DjRequest,
    count: usize,
) -> Result<Event, ApiError> {
    let mut scratch = request.clone();
    // Always sample through the random-songs path: it is the one the panel's
    // settings actually drive, and the only one that reports a pool size.
    scratch.mode = AutoDjMode::BpmKey;

    let mut tracks: Vec<Track> = Vec::new();
    let mut pool = None;
    let mut note = None;
    for _ in 0..count {
        let picked = match autodj_pick(client, caps, &scratch).await {
            Ok(picked) => picked,
            // A sample that finds nothing is an answer, not an error: it is
            // exactly what a too-tight setting looks like.
            Err(ApiError::Server { status: 400, message }) => {
                note = Some(trim_period(&message));
                break;
            }
            Err(e) => return Err(e),
        };
        pool = picked.pool.or(pool);
        note = note.or(picked.note);
        let Some(track) = picked.tracks.into_iter().next() else { break };
        scratch.ignore_list = picked.ignore_list;
        if let Some(artist) = track.metadata.artist.clone() {
            scratch.recent_artists.insert(0, artist);
        }
        if tracks.iter().any(|t: &Track| t.filepath == track.filepath) {
            break; // the pool is exhausted; more calls would repeat
        }
        tracks.push(track);
    }

    Ok(Event::AutoDjSample { tracks, pool, note })
}

/// How many neighbours a Discover view asks for. Deep enough to browse,
/// short enough that the tail is still relevant rather than noise.
const DISCOVER_LIMIT: u32 = 40;

/// Fill a Discover view.
///
/// Both routes have the same three non-answers — the feature is off, the
/// seed hasn't been embedded yet, or the ranking was walked as far as the
/// server was willing to go. None is a failure, so each gets a sentence and
/// an empty list rather than an error.
pub(crate) async fn discover(
    client: &Client,
    node: &DiscoverNode,
    seed: &Track,
    dest: DiscoverDest,
) -> Result<Event, ApiError> {
    let disabled = |data| Event::Discover {
        node: node.clone(),
        data,
        note: Some("discovery is switched off on this server".into()),
        dest,
        seed: seed.filepath.clone(),
    };

    match node {
        // All three are answered without asking the server: the two menus
        // are static, and an artist's ways in arrived with the artist list.
        DiscoverNode::Root | DiscoverNode::Mode | DiscoverNode::Artist(_) => Ok(Event::Discover {
            node: node.clone(),
            data: DiscoverData::Tracks(Vec::new()),
            note: None,
            dest,
            seed: seed.filepath.clone(),
        }),

        DiscoverNode::Tracks => {
            let Some(found) =
                client.similar_tracks_async(&seed.filepath, DISCOVER_LIMIT).await?
            else {
                return Ok(disabled(DiscoverData::Tracks(Vec::new())));
            };
            let note = if found.not_analyzed {
                Some("this track hasn't been analysed yet".to_string())
            } else if found.results.is_empty() {
                Some("nothing in your library sounds like this".to_string())
            } else {
                None
            };
            Ok(Event::Discover {
                node: node.clone(),
                data: DiscoverData::Tracks(found.results),
                note,
                dest,
                seed: seed.filepath.clone(),
            })
        }

        DiscoverNode::Artists => {
            let Some(artist) = seed.metadata.artist.as_deref().filter(|a| !a.trim().is_empty())
            else {
                return Ok(Event::Discover {
                    node: node.clone(),
                    data: DiscoverData::Artists(Vec::new()),
                    note: Some("this track has no artist tag to compare against".into()),
                    dest,
                    seed: seed.filepath.clone(),
                });
            };
            let Some(found) = client.similar_artists_async(artist, DISCOVER_LIMIT).await? else {
                return Ok(disabled(DiscoverData::Artists(Vec::new())));
            };
            let note = if found.not_analyzed {
                Some(format!("none of {artist}'s tracks have been analysed yet"))
            } else if found.results.is_empty() {
                Some(format!("nothing in your library sounds like {artist}"))
            } else if found.capped {
                // The server stops walking a long ranking, so a short list
                // here means "stopped looking", not "there is no more".
                Some(format!(
                    "{} artists — the server stopped searching before the list ran out",
                    found.results.len()
                ))
            } else {
                None
            };
            Ok(Event::Discover {
                node: node.clone(),
                data: DiscoverData::Artists(found.results),
                note,
                dest,
                seed: seed.filepath.clone(),
            })
        }
    }
}

/// Fetch a journey and translate the ways it can legitimately come up short
/// into something worth reading.
pub(crate) async fn journey(
    client: &Client,
    start: &str,
    end: &str,
    length: u32,
) -> Result<Event, ApiError> {
    let Some(response) = client.journey_async(start, end, length).await? else {
        // Gated on `discoveryPath`, so this only happens if the server was
        // reconfigured since the ping.
        return Ok(Event::Journey {
            stops: Vec::new(),
            note: Some("discovery is switched off on this server".into()),
            length,
        });
    };

    let note = journey_note(&response, length);
    // An arc that couldn't be plotted has no stops worth showing; the note
    // carries the whole answer.
    let stops = if response.not_analyzed.any() { Vec::new() } else { response.results };
    Ok(Event::Journey { stops, note, length })
}

/// What, if anything, needs saying about a journey the server returned.
///
/// Every case here is one the route produces deliberately — an unanalysed
/// end, two identical seeds, a library that ran out of visible waypoints —
/// so none of them is an error, and each deserves its own sentence.
pub(crate) fn journey_note(
    response: &crate::api::types::JourneyResponse,
    asked: u32,
) -> Option<String> {
    if response.not_analyzed.any() {
        return Some(format!(
            "{} been analysed yet — the discovery worker gets to it in its own time",
            response.not_analyzed.which()
        ));
    }
    let got = response.results.len();
    if got == 2 && asked > 2 {
        // There is no arc between a point and itself: identical seeds (or
        // duplicate copies of one recording) short-circuit to just the ends.
        return Some("those are the same track — nothing to travel through".to_string());
    }
    if (got as u32) < asked {
        return Some(format!("the library ran out at {got} of {asked} stops"));
    }
    None
}

fn trim_period(message: &str) -> String {
    message.trim().trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(url: &str) -> AudioCmd {
        AudioCmd::Play { url: url.to_string(), duration_hint: None }
    }

    fn ended(name: &str) -> Option<Passing> {
        Some(Passing::Ended(name.to_string()))
    }

    #[test]
    fn a_track_running_out_is_an_end_even_though_starting_it_took_a_command() {
        let mut watch = EndWatch::default();
        watch.note(&play("http://x/a.mp3"));
        assert_eq!(watch.observe("http://x/a.mp3"), None, "it is playing, not ending");
        assert_eq!(
            watch.observe(""),
            ended("http://x/a.mp3"),
            "and then it ran out on its own, under its own name"
        );
    }

    #[test]
    fn swapping_tracks_never_looks_like_an_ending_or_a_handover() {
        let mut watch = EndWatch::default();
        watch.note(&play("http://x/a.mp3"));
        watch.observe("http://x/a.mp3");

        // The engine decodes the next source before it drops the old sink, so
        // the polls either side of a skip both see a file loaded — and the
        // swap this thread performed itself must not read as the engine
        // moving on alone.
        watch.note(&play("http://x/b.mp3"));
        assert_eq!(watch.observe("http://x/b.mp3"), None);
        // Pausing and seeking leave the source exactly where it was.
        watch.note(&AudioCmd::Pause);
        assert_eq!(watch.observe("http://x/b.mp3"), None);
        watch.note(&AudioCmd::Seek(30.0));
        assert_eq!(watch.observe("http://x/b.mp3"), None);

        assert_eq!(
            watch.observe(""),
            ended("http://x/b.mp3"),
            "and the track swapped in ends under its own name, not the one before it"
        );
    }

    #[test]
    fn stopping_is_not_an_ending_but_it_only_answers_for_itself() {
        let mut watch = EndWatch::default();
        watch.note(&play("http://x/a.mp3"));
        watch.observe("http://x/a.mp3");

        // Silence that was asked for must not walk the queue on.
        watch.note(&AudioCmd::Stop);
        assert_eq!(watch.observe(""), None);

        // A stop that lands with nothing playing has no transition to explain
        // — and it is exactly what the app sends when the queue runs out. It
        // must not still be answering for a track started long afterwards.
        watch.note(&AudioCmd::Stop);
        assert_eq!(watch.observe(""), None);
        watch.note(&play("http://x/b.mp3"));
        watch.observe("http://x/b.mp3");
        assert_eq!(
            watch.observe(""),
            ended("http://x/b.mp3"),
            "a later track still ends on its own"
        );
    }

    #[test]
    fn a_source_change_nobody_asked_for_is_a_handover() {
        let mut watch = EndWatch::default();
        watch.note(&play("http://x/a.mp3"));
        watch.observe("http://x/a.mp3");

        // No command in between: the engine blended into the announced next
        // on its own, and the UI must be told whose cursor to move.
        assert_eq!(
            watch.observe("http://x/b.mp3"),
            Some(Passing::HandedOver {
                from: "http://x/a.mp3".to_string(),
                to: "http://x/b.mp3".to_string(),
            })
        );
        // The blended-into track then runs out like any other.
        assert_eq!(watch.observe(""), ended("http://x/b.mp3"));
    }

    #[test]
    fn a_play_that_failed_cannot_excuse_a_later_handover() {
        let mut watch = EndWatch::default();
        watch.note(&play("http://x/a.mp3"));
        watch.observe("http://x/a.mp3");

        // The user asked for b, but its open failed — the thread reported
        // PlaybackFailed and playback stayed on a. If the expectation of b
        // survived that, a later blend into b would be eaten as "the start
        // we asked for", and the cursor would stay behind.
        watch.note(&play("http://x/b.mp3"));
        watch.play_failed("http://x/b.mp3");
        assert_eq!(
            watch.observe("http://x/b.mp3"),
            Some(Passing::HandedOver {
                from: "http://x/a.mp3".to_string(),
                to: "http://x/b.mp3".to_string(),
            }),
            "the failed play's promise expired with it"
        );
    }

    /// A player whose open blows up, the way symphonia can on a malformed
    /// file. Everything else is inert.
    struct Grenade;

    impl crate::player::PlayerCtl for Grenade {
        fn play(&self, source: &str, _hint: Option<f64>) -> Result<(), String> {
            panic!("decoder exploded on {source}");
        }
        fn pause(&self) {}
        fn resume(&self) {}
        fn stop(&self) {}
        fn seek(&self, _position: f64) -> Result<(), String> {
            Ok(())
        }
        fn set_volume(&self, _volume: f32) {}
        fn set_crossfade(&self, _seconds: f32) {}
        fn set_gapless(&self, _on: bool) {}
        fn set_blend_skips(&self, _on: bool) {}
        fn set_pause_fade(&self, _on: bool) {}
        fn prepare_next(&self, _source: &str, _duration_hint: Option<f64>) {}
        fn clear_next(&self) {}
        fn status(&self) -> crate::player::PlayerStatus {
            crate::player::PlayerStatus::default()
        }
        fn tick(&self) {}
    }

    #[test]
    fn a_decoder_panic_becomes_audio_failed_not_a_dead_thread() {
        // (The panic message this prints is the test's own grenade going
        // off — cargo captures it unless the test fails.)
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let listener = thread::spawn(move || listen_guarded(&Grenade, &cmd_rx, &event_tx));
        cmd_tx.send(play("http://x/bad.flac")).unwrap();

        // The crash is reported on the channel, not left as a hole where
        // the audio thread used to be.
        let failed = loop {
            match event_rx.recv_timeout(Duration::from_secs(5)).expect("an event") {
                Event::AudioFailed(what) => break what,
                _ => continue, // status ticks may land first
            }
        };
        assert!(failed.contains("decoder exploded"), "the cause is named: {failed}");

        // Later commands still have somewhere to go, and Shutdown lands.
        cmd_tx.send(play("http://x/next.mp3")).expect("the channel is still alive");
        cmd_tx.send(AudioCmd::Shutdown).unwrap();
        listener.join().expect("the thread ended on its own terms");
    }

    #[test]
    fn the_panic_hook_stands_back_only_for_the_threads_that_catch() {
        assert!(panics_are_caught(Some(AUDIO_THREAD)));
        // The prepare thread catches at its spawn; the hook firing there
        // would tear the terminal down for a panic already handled — a
        // malformed file must cost a blend, not the screen (audit #32).
        assert!(panics_are_caught(Some(crate::engine::PREPARE_THREAD)));
        assert!(!panics_are_caught(Some("mstream-api")), "the api thread is not caught");
        assert!(!panics_are_caught(None), "an unnamed thread is not caught");
    }

    #[test]
    fn a_burst_of_commands_boils_down_to_what_it_amounted_to() {
        let vol = |v| AudioCmd::SetVolume(v);
        // Leaning on next: only the last of the run is opened at all.
        assert_eq!(
            collapse(vec![play("a"), play("b"), play("c")]),
            vec![play("c")],
            "one open for a run of skips"
        );
        // Scrubbing: the positions passed through were never wanted.
        assert_eq!(
            collapse(vec![AudioCmd::Seek(5.0), AudioCmd::Seek(6.0), AudioCmd::Seek(7.0)]),
            vec![AudioCmd::Seek(7.0)]
        );
        // The last word on the transport wins, whichever way it fell.
        assert_eq!(
            collapse(vec![play("a"), AudioCmd::Seek(30.0), AudioCmd::Stop]),
            vec![AudioCmd::Stop],
            "a stop after skips means silence, not one more fetch"
        );
        assert_eq!(collapse(vec![AudioCmd::Stop, play("b")]), vec![play("b")]);
        // A seek aimed at a track that got replaced dies with it; one aimed
        // at the track that plays survives.
        assert_eq!(collapse(vec![AudioCmd::Seek(30.0), play("b")]), vec![play("b")]);
        assert_eq!(
            collapse(vec![play("b"), AudioCmd::Seek(30.0)]),
            vec![play("b"), AudioCmd::Seek(30.0)]
        );
        // Volume is sticky, so the last one said is kept wherever it was
        // said — and a pause after the deciding play still lands.
        assert_eq!(
            collapse(vec![vol(0.2), play("a"), vol(0.8), play("b"), AudioCmd::Pause]),
            vec![vol(0.8), play("b"), AudioCmd::Pause]
        );
        // Shutdown makes the rest moot.
        assert_eq!(
            collapse(vec![play("a"), AudioCmd::Shutdown, play("b")]),
            vec![AudioCmd::Shutdown]
        );
    }

    #[test]
    fn announcements_collapse_to_the_last_word_after_the_decider() {
        let prep = |url: &str| AudioCmd::PrepareNext { url: url.to_string(), duration_hint: None };
        // An announcement before the decider was about a track the batch
        // has already moved past — applying it late would hand the engine
        // a next that belongs to nothing.
        assert_eq!(collapse(vec![prep("b"), play("c")]), vec![play("c")]);
        // After the decider, only the last announcement counts, whichever
        // shape it takes.
        assert_eq!(
            collapse(vec![play("c"), prep("d"), AudioCmd::ClearNext]),
            vec![play("c"), AudioCmd::ClearNext]
        );
        assert_eq!(
            collapse(vec![play("c"), AudioCmd::ClearNext, prep("d")]),
            vec![play("c"), prep("d")]
        );
        // The blend length is sticky like volume: kept wherever it was
        // said, last one wins.
        assert_eq!(
            collapse(vec![AudioCmd::SetCrossfade(4.0), play("c")]),
            vec![AudioCmd::SetCrossfade(4.0), play("c")]
        );
        assert_eq!(
            collapse(vec![AudioCmd::SetCrossfade(2.0), play("c"), AudioCmd::SetCrossfade(6.0)]),
            vec![AudioCmd::SetCrossfade(6.0), play("c")]
        );
        // And gapless the same.
        assert_eq!(
            collapse(vec![AudioCmd::SetGapless(true), play("c"), AudioCmd::SetGapless(false)]),
            vec![AudioCmd::SetGapless(false), play("c")]
        );
        // The C6 pair ride the same sticky rule.
        assert_eq!(
            collapse(vec![AudioCmd::SetBlendSkips(true), play("c"), AudioCmd::SetPauseFade(true)]),
            vec![AudioCmd::SetBlendSkips(true), AudioCmd::SetPauseFade(true), play("c")]
        );
    }

    #[test]
    fn the_mode_ring_steps_back_one_whatever_its_length() {
        let all = Capabilities { discovery: true, ..Default::default() };
        for mode in [AutoDjMode::Off, AutoDjMode::Similar, AutoDjMode::BpmKey] {
            assert_eq!(mode.next_available(all).prev_available(all), mode);
        }
        // Without discovery the ring is two long, where "forward twice" —
        // the old way back — is a lap.
        let few = Capabilities::default();
        for mode in [AutoDjMode::Off, AutoDjMode::BpmKey] {
            assert_eq!(mode.next_available(few).prev_available(few), mode);
            assert_ne!(mode.prev_available(few), mode, "left always moves");
        }
    }

    #[test]
    fn a_tunnel_login_goes_over_the_bridge_but_is_filed_under_the_identity() {
        let tunnel = (
            "http://127.0.0.1:51234".to_string(),
            "mstream+iroh://endpointabc".to_string(),
        );

        // The login form carries the loopback URL the tunnel published...
        assert_eq!(
            resolve_target("http://127.0.0.1:51234", Some(&tunnel)),
            (tunnel.0.clone(), tunnel.1.clone())
        );
        // ...and a reconnect names the same session by its identity. Both
        // have to reach the bridge, and both have to be remembered as the id.
        assert_eq!(
            resolve_target("mstream+iroh://endpointabc", Some(&tunnel)),
            (tunnel.0.clone(), tunnel.1.clone())
        );
    }

    #[test]
    fn only_a_tunnel_that_answered_may_replace_the_one_in_use() {
        // Installing a new bridge drops the old one, and its Drop closes the
        // loopback listener the current session streams through. So the test
        // is not "did the code parse" but "did the server on the other end
        // reply" — a pairing code can dial fine and reach nothing.
        let ping = || Box::new(crate::api::types::Ping::default());
        assert!(tunnel_answered(&Some(Event::Connected {
            server: "http://127.0.0.1:51234".into(),
            id: "mstream+iroh://endpointabc".into(),
            username: None,
            token: None,
            ping: ping(),
        })));
        assert!(
            tunnel_answered(&Some(Event::NeedsLogin { server: "http://127.0.0.1:51234".into() })),
            "reached it and was asked to sign in — the tunnel works"
        );

        assert!(
            !tunnel_answered(&Some(Event::Error("no route to host".into()))),
            "the dial failed, so the session already up keeps its bridge"
        );
        assert!(!tunnel_answered(&None));
        // Anything else is not a success either: this decides whether a
        // working tunnel is torn down, so it lists what may do that.
        assert!(!tunnel_answered(&Some(Event::Unauthorized)));
        assert!(!tunnel_answered(&Some(Event::TunnelReady {
            local_url: "http://127.0.0.1:51234".into(),
            id: "mstream+iroh://endpointabc".into(),
        })));
    }

    #[test]
    fn a_journey_names_the_end_that_is_holding_it_up() {
        use crate::api::types::{JourneyResponse, NotAnalyzed};
        let waiting = |start, end| {
            journey_note(
                &JourneyResponse {
                    not_analyzed: NotAnalyzed { start, end },
                    results: Vec::new(),
                },
                14,
            )
            .unwrap()
        };
        // Per-end, so the message can point at the one still waiting rather
        // than shrugging at both.
        assert!(waiting(true, false).contains("starting track"));
        assert!(waiting(false, true).contains("destination"));
        assert!(waiting(true, true).contains("neither end"));
    }

    #[test]
    fn a_short_arc_is_explained_rather_than_treated_as_a_failure() {
        use crate::api::types::{JourneyResponse, JourneyStop};
        let stops = |n: usize| JourneyResponse {
            results: (0..n).map(|_| JourneyStop::default()).collect(),
            ..Default::default()
        };
        // Waypoints snap to visible tracks, and a small library runs out.
        assert!(journey_note(&stops(9), 14).unwrap().contains("ran out at 9 of 14"));
        // Exactly two rows means both ends are the same recording.
        assert!(journey_note(&stops(2), 14).unwrap().contains("same track"));
        // A full arc needs no explanation.
        assert!(journey_note(&stops(14), 14).is_none());
        // …and a four-stop journey that came back whole is not "the same
        // track" just because two of its rows are the seeds.
        assert!(journey_note(&stops(4), 4).is_none());
    }

    #[test]
    fn a_direct_server_is_its_own_identity() {
        let direct = ("http://host:3000".to_string(), "http://host:3000".to_string());
        assert_eq!(resolve_target("http://host:3000", None), direct);

        // An unrelated server is not swallowed by an open tunnel.
        let tunnel = ("http://127.0.0.1:1".to_string(), "mstream+iroh://x".to_string());
        assert_eq!(resolve_target("http://host:3000", Some(&tunnel)), direct);
    }
}
