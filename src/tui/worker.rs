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
    Album, DirListing, Genre, JourneyStop, Ping, PlaylistSummary, SearchResults, SimilarArtist,
    Track,
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
    /// `self_signed` trusts the server's own TLS certificate — carried per
    /// command because the client is built here, from the one entry that
    /// opted in.
    /// `peer` aims the session at that federated peer of the server: every
    /// read rides the parent's proxies with the parent's token (contract
    /// clause 27).
    /// `identity` is what the session is filed under — the server's URL, or
    /// a tunnel's `mstream+iroh://` id when `server` is that tunnel's
    /// loopback address — and comes back on [`Event::Connected`] unchanged.
    /// `local_token` rides every request to a loopback bridge as `__lt=…`,
    /// the shared tunnel client's gate against other local processes.
    Connect {
        server: String,
        identity: String,
        token: Option<String>,
        self_signed: bool,
        peer: Option<i64>,
        local_token: Option<String>,
    },
    Login {
        server: String,
        identity: String,
        username: String,
        password: String,
        self_signed: bool,
        local_token: Option<String>,
    },
    /// Dial `credential` — a Quick Connect pairing code or a federation
    /// guest ticket — and keep the tunnel under `id` until it is closed,
    /// whichever session is current (contract clause 38). A no-op while
    /// `id` is up or dialling; answers [`Event::TunnelUp`] or
    /// [`Event::TunnelFailed`].
    TunnelOpen { id: String, credential: String },
    /// Drop the tunnel under `id`; answers [`Event::TunnelClosed`]. Sent by
    /// the queue's release policy (contract clause 38's grace).
    TunnelClose { id: String },
    /// Swap what the tunnel under `id` dials with, in place — same port,
    /// same URLs: a refreshed guest ticket, or a new pairing code for the
    /// same server. Silent when it takes; [`Event::TunnelFailed`] when not.
    /// Sent by the guest-ticket refresh (contract clause 27).
    TunnelCredential { id: String, credential: String },
    /// Ask `parent` — reached as `reach` says — for direct access to its
    /// peer `id` (contract clause 27): a guest ticket, or its refusal.
    /// `refresh` asks for a re-mint of a token the peer refused.
    DirectAccess { parent: String, id: i64, reach: crate::tui::app::Reach, refresh: bool },
    /// Swap the session's client for the same identity — a browsed peer
    /// going direct once its own tunnel is up, or back to the parent's
    /// proxy when that tunnel goes — after `server` answers. Nothing about
    /// the session but its transport changes; a connect would re-open the
    /// browser.
    Retarget {
        identity: String,
        server: String,
        token: Option<String>,
        self_signed: bool,
        peer: Option<i64>,
        local_token: Option<String>,
    },
    /// The peers a saved server lists for browsing (contract clause 20),
    /// asked once its ping says `federationBrowse`.
    FederationPeers { parent: String },
    /// Is `server` — reached at `base` — answering at all? The failure
    /// walk's question (contract clause 37): a track that would not open
    /// is skipped when its server answers and held when it does not.
    /// Its own one-shot client: the row's server may not be the session's.
    Probe { server: String, base: String, self_signed: bool, local_token: Option<String> },
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
    /// What the DJ's server offers, asked off-session: its version, its
    /// discovery flags and its libraries (auto-dj contract, clause 19).
    /// `reach` as for [`ApiCmd::AlbumArt`]; `None` asks the session's server.
    DjProbe { identity: String, reach: Option<crate::tui::app::Reach> },
    /// Every genre in the library, for the Auto-DJ genre filter.
    Genres,
    /// Walk from one track to another through the embedding space.
    Journey { start: String, end: String, length: u32 },
    /// One random library track for a Sonic Path end. The side travels with
    /// the command and comes back on the event, the `Library { dest }`
    /// pattern.
    SonicRandom { side: crate::tui::app::SonicSide },
    /// Re-read the ping after a discovery route answered 403: the flag says
    /// whether the feature is switched off or merely not yet scanned — the
    /// route itself deliberately answers both the same way.
    DiscoveryProbe,
    /// Fill a Discover view. `seed` is the track it all hangs off.
    Discover { node: DiscoverNode, seed: Box<Track>, dest: DiscoverDest },
    /// Write a whole track list to a playlist, creating it or replacing what
    /// was there. Sonic Path's "save as playlist" is the only caller.
    SavePlaylist { name: String, files: Vec<String> },
    /// Create an EMPTY playlist — the Playlists room's New. (The bulk
    /// create-or-overwrite above is a different act with a different name.)
    CreatePlaylist { name: String },
    /// Rename a playlist. The route arrived in mStream 5.16.0; an older
    /// server 404s, and the arm words that as the server's age.
    RenamePlaylist { from: String, to: String },
    DeletePlaylist { name: String },
    Search(String),
    /// Fetch and decode one cover, named by the art file a track's metadata
    /// carries. The app caches the answer under that name. `reach` names
    /// the row's own server when it is not the session's (contract clause
    /// 30); `None` asks the session.
    AlbumArt { file: String, reach: Option<crate::tui::app::Reach> },
    /// Fetch a track's shape for the progress bar. Keyed by filepath rather
    /// than by an art file: a waveform belongs to one recording, not to an
    /// album. `reach` as for [`ApiCmd::AlbumArt`].
    Waveform { filepath: String, reach: Option<crate::tui::app::Reach> },
    /// Rate a track on its own server (track-actions contract, clause 11);
    /// `seq` comes back so the App can tell a stale refusal from the latest
    /// write. `reach` as for [`ApiCmd::AlbumArt`].
    RateSong { filepath: String, rating: Option<u32>, seq: u64, reach: Option<crate::tui::app::Reach> },
    /// Add a track to a playlist on its own server (clause 12).
    AddToPlaylist { playlist: String, song: String, reach: Option<crate::tui::app::Reach> },
    /// A track's full block, for the sheet and Song info (clause 8).
    TrackInfo { filepath: String, reach: Option<crate::tui::app::Reach> },
    /// A server's playlist names, for the picker (clause 12).
    PlaylistNames { reach: Option<crate::tui::app::Reach> },
    Shutdown,
}

impl ApiCmd {
    /// The reach a command carries for a row's own server; `None` rides the
    /// session's client. Exhaustive on purpose: a new command must say
    /// whether it aims away from the session, or it does not compile — the
    /// silent alternative, a peer's read answered by the session, is what
    /// the covers did until the 2026-09-20 rig run caught it.
    pub(crate) fn reach(&self) -> Option<&crate::tui::app::Reach> {
        match self {
            ApiCmd::AlbumArt { reach, .. }
            | ApiCmd::Waveform { reach, .. }
            | ApiCmd::DjProbe { reach, .. }
            | ApiCmd::RateSong { reach, .. }
            | ApiCmd::AddToPlaylist { reach, .. }
            | ApiCmd::TrackInfo { reach, .. }
            | ApiCmd::PlaylistNames { reach } => reach.as_ref(),
            // The DJ's turns go to ITS server (auto-dj contract, clause 19).
            ApiCmd::AutoDj(request) | ApiCmd::AutoDjSample { request, .. } => request.reach.as_ref(),
            // Its reach is the parent's and its arm builds the client itself.
            ApiCmd::DirectAccess { .. }
            | ApiCmd::Connect { .. }
            | ApiCmd::Login { .. }
            | ApiCmd::TunnelOpen { .. }
            | ApiCmd::TunnelClose { .. }
            | ApiCmd::TunnelCredential { .. }
            | ApiCmd::Retarget { .. }
            | ApiCmd::FederationPeers { .. }
            | ApiCmd::Probe { .. }
            | ApiCmd::Browse(_)
            | ApiCmd::Library { .. }
            | ApiCmd::Genres
            | ApiCmd::Journey { .. }
            | ApiCmd::SonicRandom { .. }
            | ApiCmd::DiscoveryProbe
            | ApiCmd::Discover { .. }
            | ApiCmd::SavePlaylist { .. }
            | ApiCmd::CreatePlaylist { .. }
            | ApiCmd::RenamePlaylist { .. }
            | ApiCmd::DeletePlaylist { .. }
            | ApiCmd::Search(_)
            | ApiCmd::Shutdown => None,
        }
    }
}

/// One Auto DJ turn, as the App composed it (auto-dj contract, clause 19):
/// the DJ's server, how to reach it when it is not the session's, the lane
/// the ask belongs to, and everything the body is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct DjRequest {
    /// The DJ server's identity — the learner's key, and the log's name.
    pub identity: String,
    /// The reach the App resolved for it; `None` rides the session's client.
    pub reach: Option<crate::tui::app::Reach>,
    /// The lane this ask belongs to: a reply under another is dropped
    /// (clause 11), and the pool a lane let go of stays down for it alone.
    pub epoch: u64,
    pub ask: dj::Ask,
}

/// Why a turn came back empty-handed (clauses 30–34) — the App's to say
/// once per lane, and to park the queue on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DjFailure {
    /// A 401, or a 403 that was not a schema rejection.
    Auth,
    /// The server could not be reached; the pick is owed.
    Network(String),
    /// Nothing survived the server's waterfall.
    NoMatch,
    Server(String),
}

/// What a pick or a preview has to say besides its songs — a kind, for the
/// App to put in the user's words (both shells show it; clauses 30, 53).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DjNote {
    /// The pool was let go of this lane: nothing within the range.
    SonicRange,
    /// The pool was let go of this lane: the scan has not reached these tracks.
    SonicUnscanned,
    /// Preview came back empty-handed, and why.
    PreviewFailed(DjFailure),
}

/// What the DJ's server offers, from its `/api/` — or its flat ping, which
/// carries no version and no readiness (clause 19).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DjServerInfo {
    pub version: Option<String>,
    pub discovery: bool,
    /// `None` when the server does not say, which holds nothing back.
    pub discovery_ready: Option<bool>,
    pub libraries: Vec<String>,
}

impl DjServerInfo {
    /// Whether a sonic pool may be asked of this server at all (clauses 23
    /// and 36): discovery on, and the scan not reported unfinished.
    pub fn sonic_usable(&self) -> bool {
        self.discovery && self.discovery_ready != Some(false)
    }
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
/// How many tracks "Recently Added" asks for.
const RECENT_LIMIT: u32 = 100;

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
    /// News about the output device itself: playback moved to a new
    /// device on its own, lost the device, or got one back. Its own
    /// event so the app can pick a tone — losing the only device is an
    /// error, a successful move is one line of info. Either way the
    /// engine has already acted; this is narration, not a request.
    AudioDevice(crate::player::DeviceNotice),
    /// How the tunnel under `id` is reaching its server right now — direct,
    /// through a relay, or between connections. Sent on change by the
    /// sampler that watches every open tunnel.
    TunnelPath { id: String, path: crate::quickconnect::TunnelPath },
    /// The tunnel under `id` changed state: its supervisor is re-dialling,
    /// gave up on a refused credential, or is down. Sent on change.
    TunnelStatus { id: String, status: crate::quickconnect::TunnelStatus },
    /// The tunnel under `id` is up: its server answers at `local_url`, and
    /// every request there must carry `local_token` as `__lt=…`.
    TunnelUp { id: String, local_url: String, local_token: String },
    /// The dial for `id` (or a credential swap on it) failed. `rejected`
    /// means the server refused the credential — a rotated pairing code,
    /// an expired guest token — as opposed to not answering at all.
    TunnelFailed { id: String, rejected: bool, why: String },
    /// The tunnel under `id` was closed on request.
    TunnelClosed { id: String },
    /// The parent's answer about direct access to its peer `id`.
    DirectAccess { parent: String, id: i64, answer: crate::api::types::DirectAnswer },
    /// The session's client now speaks to `server` under the same identity.
    Retargeted { identity: String, server: String, token: Option<String> },
    /// `server` did not answer; the session keeps the transport it had.
    RetargetFailed { identity: String, why: String },
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
    Listing(Box<DirListing>),
    /// Contents of a library view, tagged with the node they belong to and
    /// the tab they were fetched for — the same data serves the Library tab
    /// and a drill out of the search results, and carrying the destination
    /// is what replaced a wholesale second command and event (audit #64).
    Library { node: LibraryNode, dest: Tab, data: LibraryData },
    /// One Auto DJ turn's answer: the songs that passed, in the server's
    /// order, the cursor to round-trip, whether the pool shaped them, the
    /// degrade to say once per lane, and the failure when there is one.
    AutoDjPick {
        epoch: u64,
        songs: Vec<Track>,
        ignore_list: Vec<u32>,
        sonic: bool,
        note: Option<DjNote>,
        failure: Option<DjFailure>,
    },
    /// What the DJ's server offers; `None` when it could not be asked.
    DjProbed { identity: String, info: Option<DjServerInfo> },
    /// What the current Auto-DJ settings produce, for the panel. Carries the
    /// sonic report when there was one — the pool size is the number that
    /// makes the tightness slider tunable.
    AutoDjSample {
        tracks: Vec<Track>,
        pool: Option<crate::api::types::SonicReport>,
        note: Option<DjNote>,
    },
    /// Every genre in the library.
    Genres(Vec<Genre>),
    /// The genres could not be fetched — the picker's empty state.
    GenresFailed(String),
    /// A journey's stops, in order. `note` explains a short or empty arc —
    /// both are answers the server gives deliberately rather than failures.
    /// `length` names the request this answers, since asking for a longer
    /// arc while one is still in flight is a race the UI can lose.
    Journey { stops: Vec<JourneyStop>, note: Option<String>, length: u32, issue: JourneyIssue },
    /// The random pick for one Sonic Path end — `None` when the library
    /// answered empty.
    SonicRandom { side: crate::tui::app::SonicSide, track: Option<Box<Track>> },
    /// The ping's discovery-path flag, fetched to explain a 403.
    DiscoveryProbe { available: bool },
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
    /// The management verbs landed. They carry nothing: no message rides
    /// them — the row appearing, renaming or vanishing is the confirmation
    /// — so the one thing to do is re-ask for an open Playlists view.
    PlaylistCreated,
    PlaylistRenamed,
    PlaylistDeleted,
    /// A rating landed — or, with `error`, did not (track-actions contract,
    /// clause 11); `seq` names the write.
    Rated { filepath: String, rating: Option<u32>, seq: u64, error: Option<String> },
    /// A track went into a playlist, or the server's words for why not.
    AddedToPlaylist { playlist: String, error: Option<String> },
    /// A track's full block; `None` when the server would not say.
    TrackInfo { filepath: String, track: Option<Box<Track>> },
    /// A server's playlist names for the picker; `None` when the ask failed.
    PlaylistNames { names: Option<Vec<String>> },
    /// `query` is the search these results answer — replies can pass each
    /// other now, and the box's contents name the one still wanted.
    SearchResults { query: String, results: Box<SearchResults> },
    /// A cover, decoded and shrunk to terminal scale — or `None` with
    /// `settled` saying which kind of `None` it is: the server's word that
    /// there is no art (remembered), or a failure to ask (forgotten, so
    /// the next track off that album asks again). Art is a nicety: nothing
    /// about it is ever worth a message the user has to read.
    AlbumArt { file: String, art: Option<art::Art>, settled: bool },
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
    /// The peers `parent` lists for browsing, or `None` when the ask
    /// failed — a failed fetch changes nothing (contract clause 20).
    FederationPeers { parent: String, peers: Option<Vec<crate::api::types::PeerListing>> },
    /// The probe's answer: whether `server` answered its public `/api`.
    Reachable { server: String, reachable: bool },
    Error(String),
}

/// What kept a journey from being an ordinary list of stops. Typed rather
/// than read back out of the note's wording: the UIs branch on it — a
/// retry makes sense for an empty arc but not for a feature that is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JourneyIssue {
    /// Nothing structural — the stops (or their absence) are the answer.
    #[default]
    None,
    /// The route answered 403. Deliberately ambiguous server-side; the
    /// app follows up with [`ApiCmd::DiscoveryProbe`] to name the reason.
    Disabled,
    /// An end has no embedding yet; the note names which. The fix is
    /// editing or waiting for the scan, so no retry is offered.
    NotAnalyzed,
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
        // Device news before the status shaped by it, same ordering rule
        // as the end-of-track events below.
        for notice in player.take_device_notices() {
            let _ = events.send(Event::AudioDevice(notice));
        }
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

/// Every open tunnel, by identity — the session's own and the ones the queue
/// needs — for as long as the api thread lives. One dial in flight per
/// identity at most; a tunnel stays until it is closed on request, whichever
/// session is current (contract clause 38).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct Tunnels {
    slots: std::collections::HashMap<String, TunnelSlot>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct TunnelSlot {
    tunnel: Option<iroh_tunnel::Tunnel>,
    dialling: bool,
}

#[cfg(not(target_arch = "wasm32"))]
type TunnelTable = std::sync::Mutex<Tunnels>;

#[cfg(not(target_arch = "wasm32"))]
fn lock(table: &TunnelTable) -> std::sync::MutexGuard<'_, Tunnels> {
    table.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(not(target_arch = "wasm32"))]
fn api_loop(rx: &Receiver<ApiCmd>, events: &Sender<Event>) {
    let mut client: Option<Arc<Client>> = None;
    // The dial threads and the sampler hold the table too; the sampler only
    // weakly, so it ends with this thread.
    let tunnels: Arc<TunnelTable> = Arc::new(std::sync::Mutex::new(Tunnels::default()));
    spawn_tunnel_sampler(Arc::downgrade(&tunnels), events.clone());
    while let Ok(cmd) = rx.recv() {
        // Connection commands change who `client` *is*, so they stay
        // serialized here — reaching a different server mid-dial is a
        // contradiction, not a feature. Everything else is a read against
        // the current client and answers on its own thread (audit #63):
        // one stalled search used to block every pane behind a 20-second
        // timeout. A tunnel dial takes up to a minute cold, so it runs on
        // its own thread as well and reports back through the events.
        let result = match cmd {
            ApiCmd::Shutdown => break,

            ApiCmd::Connect { server, identity, token, self_signed, peer, local_token } => {
                connect(&mut client, &server, &identity, token, self_signed, peer, local_token)
            }

            ApiCmd::Login { server, identity, username, password, self_signed, local_token } => {
                login(&mut client, &server, &identity, &username, &password, self_signed, local_token)
            }

            ApiCmd::TunnelOpen { id, credential } => {
                tracing::info!("tunnel {}: dialling", tunnel_log_name(&id));
                open_tunnel(&tunnels, id, credential, events.clone());
                None
            }
            ApiCmd::TunnelClose { id } => {
                tracing::info!("tunnel {}: closing — nothing references it", tunnel_log_name(&id));
                Some(close_tunnel(&tunnels, id))
            }
            ApiCmd::TunnelCredential { id, credential } => swap_credential(&tunnels, id, &credential),
            ApiCmd::Retarget { identity, server, token, self_signed, peer, local_token } => {
                Some(retarget(&mut client, &server, &identity, token, self_signed, peer, local_token))
            }

            read => {
                spawn_read(client.clone(), events.clone(), read);
                None
            }
        };

        if let Some(event) = result
            && events.send(event).is_err()
        {
            break;
        }
    }

    // Every tunnel goes down gracefully on the way out; a plain drop would
    // slam the connections shut under whatever was still streaming.
    let table = std::mem::take(&mut *lock(&tunnels));
    if let Ok(rt) = crate::runtime::handle() {
        for slot in table.slots.into_values() {
            if let Some(tunnel) = slot.tunnel {
                tunnel.begin_shutdown(rt);
            }
        }
    }
}

/// The client for a read aimed at a row's own server. `None` when the base
/// will not parse — the read then falls back to the session, whose answer
/// the App's stale-reply guards judge as they would any other.
///
/// Kept, one per reach: a client is a connection pool, and building one
/// per read meant a fresh handshake for every cover, waveform and DJ turn
/// aimed away from the session. A handful of servers is all a queue ever
/// mixes; the oldest goes when the shelf is full.
#[cfg(not(target_arch = "wasm32"))]
fn client_for(reach: &crate::tui::app::Reach) -> Option<Arc<Client>> {
    static KEPT: std::sync::Mutex<Vec<(crate::tui::app::Reach, Arc<Client>)>> = std::sync::Mutex::new(Vec::new());
    const SHELF: usize = 16;
    let mut kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, client)) = kept.iter().find(|(known, _)| known == reach) {
        return Some(client.clone());
    }
    let client = Client::new_with(&reach.base, reach.self_signed)
        .ok()
        .map(|c| c.with_token(reach.token.clone()).with_peer(reach.peer).with_local_token(reach.local_token.clone()))
        .map(Arc::new)?;
    if kept.len() >= SHELF {
        kept.remove(0);
    }
    kept.push((reach.clone(), client.clone()));
    Some(client)
}

/// Answer one read on its own thread, so a slow server holds up this reply
/// and nothing else. In-flight replies against a client that has since been
/// replaced still arrive; the app's stale-reply guards are what drop them,
/// the same as any other answer about somewhere the user no longer is.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_read(client: Option<Arc<Client>>, events: Sender<Event>, cmd: ApiCmd) {
    thread::Builder::new()
        .name("mstream-api-read".into())
        .spawn(move || {
            let event = answer(client.as_deref(), cmd);
            let _ = events.send(event);
        })
        .ok();
}

/// One read, answered. Failures map onto events here: only 401 means the
/// session is no good; 403 is a permission or feature-flag answer that
/// shouldn't bounce the user to a login form.
#[cfg(not(target_arch = "wasm32"))]
fn answer(client: Option<&Client>, cmd: ApiCmd) -> Event {
    // Direct access is asked of the parent as the App reached it — never
    // through a peer client's rewrite, and not necessarily the session.
    if let ApiCmd::DirectAccess { parent, id, reach, refresh } = cmd {
        let answer = match client_for(&reach) {
            Some(c) => direct_answer(c.federation_access(id, refresh)),
            None => crate::api::types::DirectAnswer::Failed("the parent's address will not parse".into()),
        };
        return Event::DirectAccess { parent, id, answer };
    }
    // The probe needs no session: it asks the row's own server, which may
    // be one the session never reached.
    if let ApiCmd::Probe { server, base, self_signed, local_token } = cmd {
        let reachable = Client::new_with(&base, self_signed)
            .map(|c| c.with_local_token(local_token))
            .and_then(|c| c.server_info())
            .is_ok();
        return Event::Reachable { server, reachable };
    }
    // A row's own server when it is not the session's (contract clause 30):
    // its cover and its shape come from a client built for the reach the
    // App resolved — a tunnel's loopback with its token, a saved server
    // with its own token and trust, a peer through its parent.
    let own = cmd.reach().and_then(client_for);
    let c = match (own.as_deref(), client) {
        (Some(own), _) => own,
        (None, Some(session)) => session,
        (None, None) => return Event::Error("not connected to a server".into()),
    };
    let answered = match cmd {
        ApiCmd::Browse(path) => {
            c.file_explorer(&path).map(|l| Event::Listing(Box::new(l)))
        }
        ApiCmd::Library { node, dest } => crate::api::wait(load_library(c, &node))
            .map(|data| Event::Library { node, dest, data }),
        // Neither turn nor probe fails as an error: the App reads the answer.
        ApiCmd::AutoDj(request) => {
            crate::api::wait(async { Ok::<_, ApiError>(autodj_pick(c, &request).await) })
                .map(|picked| pick_event(picked, &request))
        }
        ApiCmd::AutoDjSample { request, count } => {
            crate::api::wait(autodj_sample(c, &request, count))
        }
        ApiCmd::DjProbe { identity, .. } => {
            crate::api::wait(async { Ok::<_, ApiError>(dj_probe(c).await) })
                .map(|info| Event::DjProbed { identity, info })
        }
        // The track verbs answer with their outcome rather than an error
        // event: the App reverts a rating, words a failed add, or shows a
        // sheet without its block (track-actions contract).
        ApiCmd::RateSong { filepath, rating, seq, .. } => {
            let error = crate::api::wait(c.rate_song_async(&filepath, rating)).err().map(|e| e.to_string());
            Ok(Event::Rated { filepath, rating, seq, error })
        }
        ApiCmd::AddToPlaylist { playlist, song, .. } => {
            let error =
                crate::api::wait(c.playlist_add_song_async(&playlist, &song)).err().map(|e| e.to_string());
            Ok(Event::AddedToPlaylist { playlist, error })
        }
        ApiCmd::TrackInfo { filepath, .. } => {
            let track = c.metadata(&filepath).ok().map(Box::new);
            Ok(Event::TrackInfo { filepath, track })
        }
        ApiCmd::PlaylistNames { .. } => {
            let names = c.playlists().ok().map(|list| list.into_iter().map(|p| p.name).collect());
            Ok(Event::PlaylistNames { names })
        }
        // A dead session is the session's business; anything else is the
        // picker's to say (auto-dj contract, clause 48).
        ApiCmd::Genres => match c.genres() {
            Ok(genres) => Ok(Event::Genres(genres)),
            Err(ApiError::Unauthorized) => Err(ApiError::Unauthorized),
            Err(e) => Ok(Event::GenresFailed(e.to_string())),
        },
        ApiCmd::Journey { start, end, length } => {
            crate::api::wait(journey(c, &start, &end, length))
        }
        ApiCmd::SonicRandom { side } => c
            .random_song(&crate::api::types::RandomSongRequest::default())
            .map(|r| Event::SonicRandom { side, track: r.songs.into_iter().next().map(Box::new) }),
        ApiCmd::DiscoveryProbe => {
            c.ping().map(|ping| Event::DiscoveryProbe { available: ping.discovery_path })
        }
        // A failed ask is `None`: nothing about the saved peers changes.
        ApiCmd::FederationPeers { parent } => {
            Ok(Event::FederationPeers { parent, peers: c.federation_peers().ok() })
        }
        ApiCmd::Discover { node, seed, dest } => {
            crate::api::wait(discover(c, &node, &seed, dest))
        }
        ApiCmd::SavePlaylist { name, files } => {
            let count = files.len();
            c.playlist_save(&name, &files).map(|()| Event::PlaylistSaved { name, count })
        }
        // The management verbs word their own failures — `<what failed>:
        // <the server's words>` — so the generic fallthrough never has to
        // guess what the user was doing (contract clause 50).
        ApiCmd::CreatePlaylist { name } => match c.playlist_new(&name) {
            Ok(()) => Ok(Event::PlaylistCreated),
            Err(ApiError::Unauthorized) => Err(ApiError::Unauthorized),
            Err(e) => Ok(Event::Error(format!("couldn't create {name}: {e}"))),
        },
        ApiCmd::RenamePlaylist { from, to } => match c.playlist_rename(&from, &to) {
            Ok(()) => Ok(Event::PlaylistRenamed),
            Err(ApiError::Unauthorized) => Err(ApiError::Unauthorized),
            // The route is 5.16.0+: a 404 is the server's age, not a
            // missing playlist — worded so it reads as old, not broken.
            Err(ApiError::NotFound(_)) => Ok(Event::Error(
                "this server can't rename playlists — it needs mStream 5.16".into(),
            )),
            Err(e) => Ok(Event::Error(format!("couldn't rename {from}: {e}"))),
        },
        ApiCmd::DeletePlaylist { name } => match c.playlist_delete(&name) {
            Ok(()) => Ok(Event::PlaylistDeleted),
            Err(ApiError::Unauthorized) => Err(ApiError::Unauthorized),
            Err(e) => Ok(Event::Error(format!("couldn't delete {name}: {e}"))),
        },
        ApiCmd::Search(query) => {
            c.search(&query).map(|r| Event::SearchResults { query, results: Box::new(r) })
        }
        ApiCmd::AlbumArt { file, .. } => {
            // The waveform's rule, because this cache burned without it: a
            // 404 and bytes that won't decode are the server's own word
            // that there is no art — settled, remembered, never asked
            // again. A transport failure is not an answer, and remembering
            // it as one meant a fetch that died with the wifi left that
            // album coverless for the rest of the session. Decoded here so
            // the render loop only ever meets covers already at terminal
            // scale.
            let answer = c.album_art(&file);
            let settled = matches!(&answer, Ok(_) | Err(ApiError::NotFound(_)));
            let art = answer.ok().and_then(|bytes| art::decode(&bytes));
            Ok(Event::AlbumArt { file, art, settled })
        }
        ApiCmd::Waveform { filepath, .. } => {
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
        // The probe answered above, before the session client was needed.
        ApiCmd::Connect { .. }
        | ApiCmd::Login { .. }
        | ApiCmd::TunnelOpen { .. }
        | ApiCmd::TunnelClose { .. }
        | ApiCmd::TunnelCredential { .. }
        | ApiCmd::DirectAccess { .. }
        | ApiCmd::Retarget { .. }
        | ApiCmd::Probe { .. }
        | ApiCmd::Shutdown => return Event::Error("connection change routed as a read".into()),
    };
    match answered {
        Ok(event) => event,
        Err(ApiError::Unauthorized) => Event::Unauthorized,
        Err(e) => Event::Error(e.to_string()),
    }
}

/// Dial `credential` for `id` on its own thread and install the tunnel. A
/// dial already in flight makes this a no-op; a tunnel already up is simply
/// reported again. The thread answers `TunnelUp` or `TunnelFailed`. A close that lands while the
/// dial is out wins: the tunnel is shut down as soon as it arrives.
#[cfg(not(target_arch = "wasm32"))]
fn open_tunnel(tunnels: &Arc<TunnelTable>, id: String, credential: String, events: Sender<Event>) {
    {
        let mut table = lock(tunnels);
        let slot = table.slots.entry(id.clone()).or_default();
        if let Some(tunnel) = &slot.tunnel {
            // Already up: say so again, so an App whose picture of this
            // tunnel lagged never waits on a dial that will not happen.
            let _ = events.send(Event::TunnelUp {
                id,
                local_url: tunnel.local_url(),
                local_token: tunnel.local_token(),
            });
            return;
        }
        if slot.dialling {
            return; // the dial in flight will report
        }
        slot.dialling = true;
    }
    let tunnels = Arc::clone(tunnels);
    let _ = thread::Builder::new().name("mstream-tunnel-dial".into()).spawn(move || {
        let dialled = match crate::runtime::block_on(iroh_tunnel::connect_tunnel(&credential, 0)) {
            Ok(dialled) => dialled,
            Err(why) => Err(iroh_tunnel::DialError::Local(why)),
        };
        let event = match dialled {
            Ok(tunnel) => {
                let local_url = tunnel.local_url();
                let local_token = tunnel.local_token();
                let mut table = lock(&tunnels);
                match table.slots.get_mut(&id) {
                    Some(slot) if slot.dialling => {
                        slot.dialling = false;
                        slot.tunnel = Some(tunnel);
                        Event::TunnelUp { id, local_url, local_token }
                    }
                    // Closed while the dial was out.
                    _ => {
                        drop(table);
                        if let Ok(rt) = crate::runtime::handle() {
                            tunnel.begin_shutdown(rt);
                        }
                        Event::TunnelClosed { id }
                    }
                }
            }
            Err(e) => {
                lock(&tunnels).slots.remove(&id);
                // A refused credential is the one failure a re-dial with
                // the same code cannot fix; everything else is a server
                // that did not answer.
                Event::TunnelFailed { id, rejected: e.is_rejected(), why: e.to_string() }
            }
        };
        let _ = events.send(event);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn close_tunnel(tunnels: &Arc<TunnelTable>, id: String) -> Event {
    let closed = lock(tunnels).slots.remove(&id).and_then(|slot| slot.tunnel);
    if let Some(tunnel) = closed
        && let Ok(rt) = crate::runtime::handle()
    {
        tunnel.begin_shutdown(rt);
    }
    Event::TunnelClosed { id }
}

#[cfg(not(target_arch = "wasm32"))]
fn swap_credential(tunnels: &Arc<TunnelTable>, id: String, credential: &str) -> Option<Event> {
    let failed = |why: String| Some(Event::TunnelFailed { id: id.clone(), rejected: false, why });
    let table = lock(tunnels);
    let Some(tunnel) = table.slots.get(&id).and_then(|slot| slot.tunnel.as_ref()) else {
        return failed("no open tunnel to update".into());
    };
    let rt = match crate::runtime::handle() {
        Ok(rt) => rt,
        Err(e) => return failed(e),
    };
    match tunnel.set_credential(credential, rt) {
        Ok(()) => None,
        Err(e) => failed(e.to_string()),
    }
}

/// A tunnel identity as the log should show it. A peer's identity carries
/// `id@parent`, which the log's scrubber would read as a URL's userinfo and
/// redact; spelled out, it is just a row number.
#[cfg(not(target_arch = "wasm32"))]
fn tunnel_log_name(id: &str) -> String {
    match id.strip_prefix(crate::config::PEER_ID_PREFIX).and_then(|rest| rest.split_once('@')) {
        Some((row, parent)) => format!("peer {row} of {}", crate::quickconnect::display_server(parent)),
        None => crate::quickconnect::display_server(id),
    }
}

/// Watch every open tunnel and tell the UI when one changes state or path.
/// Holds only a Weak: when the api thread drops the table, the next sample
/// fails to upgrade and this thread ends. The first sample of a tunnel is
/// sent unconditionally, so a fresh one shows its state within a beat. The
/// shared client's own event ring is drained into the log on the way.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_tunnel_sampler(tunnels: std::sync::Weak<TunnelTable>, events: Sender<Event>) {
    let _ = thread::Builder::new().name("mstream-tunnel-watch".into()).spawn(move || {
        let mut last: std::collections::HashMap<String, (u8, u8)> = Default::default();
        loop {
            let Some(tunnels) = tunnels.upgrade() else { return };
            let mut seen = Vec::new();
            {
                let table = lock(&tunnels);
                for (id, slot) in &table.slots {
                    let Some(tunnel) = &slot.tunnel else { continue };
                    if let Some(lines) = tunnel.drain_events() {
                        let shown = tunnel_log_name(id);
                        for line in lines.lines() {
                            tracing::info!("tunnel {shown}: {line}");
                        }
                    }
                    seen.push((id.clone(), tunnel.status(), tunnel.path_kind()));
                }
            }
            drop(tunnels);
            last.retain(|id, _| seen.iter().any(|(seen_id, _, _)| seen_id == id));
            for (id, status, path) in seen {
                let before = last.insert(id.clone(), (status, path));
                if before.map(|(s, _)| s) != Some(status) {
                    let status = crate::quickconnect::TunnelStatus::from_code(status);
                    if events.send(Event::TunnelStatus { id: id.clone(), status }).is_err() {
                        return;
                    }
                }
                if before.map(|(_, p)| p) != Some(path) {
                    let path = crate::quickconnect::TunnelPath::from_kind(path);
                    if events.send(Event::TunnelPath { id, path }).is_err() {
                        return;
                    }
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
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
    // A peer answers no ping through the proxy; its layered `/api` does.
    let ping = if c.peer().is_some() { c.ping_via_info()? } else { c.ping()? };
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
    self_signed: bool,
    peer: Option<i64>,
    local_token: Option<String>,
) -> Option<Event> {
    let c = match Client::new_with(server, self_signed) {
        Ok(c) => c.with_token(token.clone()).with_peer(peer).with_local_token(local_token),
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
    self_signed: bool,
    local_token: Option<String>,
) -> Option<Event> {
    let mut c = match Client::new_with(server, self_signed) {
        Ok(c) => c.with_local_token(local_token),
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

/// Swap the session's client for the same identity once `server` answers.
/// A peer's layered `GET /api` is the ping either way — the loopback of a
/// direct peer serves it plainly, the parent's proxy serves it rewritten.
#[cfg(not(target_arch = "wasm32"))]
fn retarget(
    client: &mut Option<Arc<Client>>,
    server: &str,
    identity: &str,
    token: Option<String>,
    self_signed: bool,
    peer: Option<i64>,
    local_token: Option<String>,
) -> Event {
    let failed = |why: String| Event::RetargetFailed { identity: identity.to_string(), why };
    let c = match Client::new_with(server, self_signed) {
        Ok(c) => c.with_token(token.clone()).with_peer(peer).with_local_token(local_token),
        Err(e) => return failed(e.to_string()),
    };
    if let Err(e) = c.ping_via_info() {
        return failed(e.to_string());
    }
    let server = c.server();
    *client = Some(Arc::new(c));
    Event::Retargeted { identity: identity.to_string(), server, token }
}

/// Sort the parent's access answer (contract clause 27): a grant with every
/// field is a ticket, `direct: false` is a refusal that holds for the
/// session, and anything else — the peer unreachable for the mint (a 502),
/// a 200 missing fields — is transient.
#[cfg(not(target_arch = "wasm32"))]
fn direct_answer(
    result: Result<crate::api::types::DirectAccessResponse, ApiError>,
) -> crate::api::types::DirectAnswer {
    use crate::api::types::{DirectAnswer, DirectTicket};
    let response = match result {
        Ok(response) => response,
        Err(e) => return DirectAnswer::Failed(e.to_string()),
    };
    if !response.direct {
        return DirectAnswer::Denied(
            response.reason.unwrap_or_else(|| "the parent declined direct access".to_string()),
        );
    }
    let (Some(ticket), Some(guest_token)) = (response.direct_ticket, response.guest_token) else {
        return DirectAnswer::Failed("the access answer is missing its ticket".into());
    };
    if !ticket.starts_with("mstrfedg") || guest_token.is_empty() {
        return DirectAnswer::Failed("the access answer is not a guest ticket".into());
    }
    let (issued_at, expires_at) = jwt_times(&guest_token);
    DirectAnswer::Granted(DirectTicket {
        ticket,
        guest_token,
        endpoint_id: response.endpoint_id.filter(|id| !id.is_empty()),
        issued_at,
        expires_at,
    })
}

/// The `iat` and `exp` claims of a JWT, read without verifying it — the
/// peer verifies; this side only needs to know when to ask for a new one.
#[cfg(not(target_arch = "wasm32"))]
fn jwt_times(token: &str) -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    use base64::Engine as _;
    let Some(payload) = token.split('.').nth(1) else { return (None, None) };
    let normalised: String = payload.chars().filter(|c| *c != '=').collect();
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(normalised) else {
        return (None, None);
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return (None, None) };
    let at = |key: &str| {
        claims
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|secs| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
    };
    (at("iat"), at("exp"))
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

// ── Auto DJ ─────────────────────────────────────────────────────────────────

/// One turn's answer, whatever happened: the songs that passed, the cursor
/// to round-trip, and the failure or degrade the App may say out loud once
/// per lane (auto-dj contract, clauses 26 and 30–34).
#[derive(Debug, Clone)]
pub(crate) struct Picked {
    pub(crate) songs: Vec<Track>,
    pub(crate) ignore_list: Vec<u32>,
    /// Whether the pool shaped these picks — the badge's second glyph.
    pub(crate) sonic: bool,
    pub(crate) note: Option<DjNote>,
    pub(crate) pool: Option<crate::api::types::SonicReport>,
    pub(crate) failure: Option<DjFailure>,
}

impl Picked {
    fn failed(ignore_list: Vec<u32>, failure: DjFailure) -> Picked {
        Picked { songs: Vec::new(), ignore_list, sonic: false, note: None, pool: None, failure: Some(failure) }
    }
}

/// The DJ's second voice (clause 63): the shell's log on the native build,
/// where a filter that quietly does nothing would otherwise be
/// indistinguishable from one that works; the browser build has none. The
/// App's DJ and track modules log through here too.
pub(crate) fn dj_log(line: String) {
    #[cfg(not(target_arch = "wasm32"))]
    tracing::info!("{line}");
    #[cfg(target_arch = "wasm32")]
    let _ = line;
}

/// The DJ server's name in the log: the tunnel's short form on the native
/// build, the identity itself in the browser, which has no tunnels.
fn dj_log_name(identity: &str) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tunnel_log_name(identity)
    }
    #[cfg(target_arch = "wasm32")]
    {
        identity.to_string()
    }
}

/// The two keys a pool is asked with, let go of together (clause 30).
const SONIC_KEYS: [&str; 2] = ["similarTo", "minSimilarity"];


/// Which keys a server will not take — learned from a `"<key>" is not
/// allowed` rejection for the rest of the process (clause 25) — and which a
/// lane has let go of after its pool failed (clause 30), by epoch, so a new
/// lane asks again. In memory only, by design: persisting "this server
/// rejected X" would outlive the upgrade that fixes it, where a process
/// lifetime is long enough to stop repeated failures and short enough to
/// notice an upgrade.
#[derive(Default)]
struct DjLearner {
    rejected: std::collections::HashMap<String, std::collections::HashSet<String>>,
    suppressed: std::collections::HashMap<String, (u64, std::collections::HashSet<String>)>,
}

fn learner() -> std::sync::MutexGuard<'static, DjLearner> {
    static LEARNER: std::sync::OnceLock<std::sync::Mutex<DjLearner>> = std::sync::OnceLock::new();
    LEARNER.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

impl DjLearner {
    /// Strip what this server is known not to take, saying what went.
    fn filter(&self, identity: &str, epoch: u64, body: &mut serde_json::Value) -> Vec<String> {
        let mut dropped = Vec::new();
        let Some(map) = body.as_object_mut() else { return dropped };
        let rejected = self.rejected.get(identity);
        let suppressed =
            self.suppressed.get(identity).filter(|(e, _)| *e == epoch).map(|(_, keys)| keys);
        map.retain(|key, _| {
            let gone = rejected.is_some_and(|r| r.contains(key))
                || suppressed.is_some_and(|s| s.contains(key));
            if gone {
                dropped.push(key.clone());
            }
            !gone
        });
        dropped
    }

    /// A key the server named in a rejection: never sent to it again this
    /// process. True when it is news.
    fn learn(&mut self, identity: &str, key: &str) -> bool {
        self.rejected.entry(identity.to_string()).or_default().insert(key.to_string())
    }

    /// Keys a lane lets go of — valid, but the server cannot act on them
    /// now (a pool with nothing in range, a library not yet scanned).
    fn suppress(&mut self, identity: &str, epoch: u64, keys: &[&str]) {
        let entry = self.suppressed.entry(identity.to_string()).or_insert((epoch, Default::default()));
        if entry.0 != epoch {
            *entry = (epoch, Default::default());
        }
        for key in keys {
            entry.1.insert((*key).to_string());
        }
    }

    fn all_suppressed(&self, identity: &str, epoch: u64, keys: &[&str]) -> bool {
        self.suppressed
            .get(identity)
            .filter(|(e, _)| *e == epoch)
            .is_some_and(|(_, set)| keys.iter().all(|k| set.contains(*k)))
    }
}

/// The key a Joi rejection names — `"<key>" is not allowed`, its quotes
/// escaped or not, since callers may pass the raw JSON body or the message
/// already read out of it. `None` for any other message: the request then
/// failed for some other reason, and resending a smaller body would only
/// fail again with less information.
pub(crate) fn not_allowed_key(message: &str) -> Option<String> {
    let at = message.find(" is not allowed")?;
    let head = message[..at].trim_end().trim_end_matches(['"', '\\']);
    let key: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let quoted = head[..head.len() - key.len()].ends_with(['"', '\\']);
    (!key.is_empty() && quoted).then_some(key)
}

/// What a refused turn means, read off the status and the body (clauses
/// 25 and 30–32).
#[derive(Debug, Clone, PartialEq)]
enum Refusal {
    /// A schema rejection naming a key: learn it and go again.
    Learn(String),
    /// The pool cannot be honoured: let it go for the lane, and say so —
    /// or not, when the user switched discovery off themselves.
    Degrade(Option<DjNote>),
    Auth,
    Network(String),
    NoMatch,
    Other(String),
}

fn classify(err: &ApiError, sonic_asked: bool) -> Refusal {
    match err {
        ApiError::Unauthorized => Refusal::Auth,
        ApiError::Network(message) => Refusal::Network(message.clone()),
        ApiError::Decode { .. } | ApiError::Config(_) => Refusal::Other(err.to_string()),
        ApiError::Forbidden(message) | ApiError::NotFound(message) | ApiError::Server { message, .. } => {
            // The body is the signal, not the status: mStream answered a
            // schema rejection with 403 up to 6.11.0 and 400 since.
            if let Some(key) = not_allowed_key(message) {
                return Refusal::Learn(key);
            }
            if sonic_asked {
                let lower = message.to_lowercase();
                if lower.contains("similarity range") {
                    return Refusal::Degrade(Some(DjNote::SonicRange));
                }
                if lower.contains("analyzed") {
                    return Refusal::Degrade(Some(DjNote::SonicUnscanned));
                }
                // Switched off server-side since the probe — the user's own
                // change, so being told is noise; a 404 is a dead seed path.
                if lower.contains("discovery is disabled") || matches!(err, ApiError::NotFound(_)) {
                    return Refusal::Degrade(None);
                }
            }
            match err {
                ApiError::Forbidden(_) => Refusal::Auth,
                ApiError::Server { status: 400, .. } => Refusal::NoMatch,
                _ => Refusal::Other(err.to_string()),
            }
        }
    }
}

/// One Auto DJ turn against the DJ's server: the ask, less whatever the
/// server is known not to take; a schema rejection learned and retried; a
/// failing pool let go of for the lane and the same pick taken without it;
/// the keyword filter over the answer, re-asked with the fresh cursor when
/// it blocked everything, five times, then the last answer whole (clauses
/// 25, 26, 30). Never an error: the App reads the failure and speaks.
pub(crate) async fn autodj_pick(client: &Client, request: &DjRequest) -> Picked {
    let identity = request.identity.as_str();
    let epoch = request.epoch;
    let name = dj_log_name(identity);
    let mut ask = request.ask.clone();
    if ask.sonic_asked() && learner().all_suppressed(identity, epoch, &SONIC_KEYS) {
        ask = ask.without_sonic();
    }
    let mut note: Option<DjNote> = None;
    let mut last: Option<crate::api::types::RandomSongsResponse> = None;
    let mut attempts = 0;
    while attempts < 5 {
        attempts += 1;
        let mut body = match serde_json::to_value(ask.request()) {
            Ok(body) => body,
            Err(e) => return Picked::failed(ask.ignore_list, DjFailure::Server(e.to_string())),
        };
        let dropped = learner().filter(identity, epoch, &mut body);
        if !dropped.is_empty() {
            dj_log(format!("[dj] {name}: dropped for this server: {}", dropped.join(", ")));
        }
        let mut answer = client.random_songs_raw_async(body).await;
        // The learner's loop: every pass removes one key for good, so it
        // ends by construction. Only a not-allowed body retries here.
        while let Err(err) = &answer {
            let Refusal::Learn(key) = classify(err, ask.sonic_asked()) else { break };
            if learner().learn(identity, &key) {
                dj_log(format!("[dj] {name}: rejected \"{key}\" — dropping it for the rest of this session"));
            }
            let mut body = serde_json::to_value(ask.request()).unwrap_or_default();
            learner().filter(identity, epoch, &mut body);
            answer = client.random_songs_raw_async(body).await;
        }
        match answer {
            Ok(response) => {
                let sonic = ask.sonic_asked();
                if response.songs.is_empty() {
                    return Picked { failure: Some(DjFailure::NoMatch), ..Picked { songs: Vec::new(), ignore_list: response.ignore_list, sonic, note, pool: response.sonic, failure: None } };
                }
                let accepted: Vec<Track> =
                    response.songs.iter().filter(|t| !ask.keyword_blocked(t)).cloned().collect();
                if !accepted.is_empty() {
                    return Picked { songs: accepted, ignore_list: response.ignore_list, sonic, note, pool: response.sonic, failure: None };
                }
                // Blocked in full: the fresh cursor means the next answer
                // is different candidates.
                dj_log(format!("[dj] {name}: every song of the answer was a keyword hit — asking again"));
                ask.ignore_list = response.ignore_list.clone();
                last = Some(response);
            }
            Err(err) => match classify(&err, ask.sonic_asked()) {
                Refusal::Degrade(say) => {
                    dj_log(format!("[dj] {name}: the pool cannot be honoured ({err}) — playing without it this lane"));
                    learner().suppress(identity, epoch, &SONIC_KEYS);
                    if let Some(say) = say {
                        note = Some(say);
                    }
                    ask = ask.without_sonic();
                }
                Refusal::Auth => {
                    dj_log(format!("[dj] {name}: {err} — the session behind the DJ is no good"));
                    return Picked::failed(ask.ignore_list, DjFailure::Auth);
                }
                Refusal::Network(message) => {
                    return Picked::failed(ask.ignore_list, DjFailure::Network(message));
                }
                Refusal::NoMatch => return Picked::failed(ask.ignore_list, DjFailure::NoMatch),
                Refusal::Learn(_) | Refusal::Other(_) => {
                    dj_log(format!("[dj] {name}: random-songs failed: {err}"));
                    return Picked::failed(ask.ignore_list, DjFailure::Server(err.to_string()));
                }
            },
        }
    }
    // Every try was blocked in full: the last answer whole, rather than a
    // queue stalled forever by an over-eager filter (clause 26).
    match last {
        Some(response) => Picked {
            songs: response.songs,
            ignore_list: response.ignore_list,
            sonic: ask.sonic_asked(),
            note,
            pool: response.sonic,
            failure: None,
        },
        None => Picked::failed(ask.ignore_list, DjFailure::NoMatch),
    }
}

/// The turn's answer as the event the App consumes.
pub(crate) fn pick_event(picked: Picked, request: &DjRequest) -> Event {
    Event::AutoDjPick {
        epoch: request.epoch,
        songs: picked.songs,
        ignore_list: picked.ignore_list,
        sonic: picked.sonic,
        note: picked.note,
        failure: picked.failure,
    }
}

/// Take several picks in a row without committing to any of them, feeding
/// each back into the next call's cooldown and cursor so the sample shows
/// variety rather than the same track three times (clause 53).
pub(crate) async fn autodj_sample(
    client: &Client,
    request: &DjRequest,
    count: usize,
) -> Result<Event, ApiError> {
    let mut scratch = request.clone();
    scratch.ask.opener = false;
    let mut tracks: Vec<Track> = Vec::new();
    let mut pool = None;
    let mut note = None;
    for _ in 0..count {
        let picked = autodj_pick(client, &scratch).await;
        pool = picked.pool.clone().or(pool);
        note = note.or(picked.note.clone());
        if let Some(failure) = picked.failure {
            // A sample that finds nothing is an answer, not an error: it is
            // exactly what a too-tight setting looks like.
            note = note.or(Some(DjNote::PreviewFailed(failure)));
            break;
        }
        let Some(track) = picked.songs.into_iter().next() else { break };
        scratch.ask.ignore_list = picked.ignore_list;
        if let Some(artist) = track.metadata.artist.clone() {
            scratch.ask.recent_artists.insert(0, artist);
        }
        if tracks.iter().any(|t: &Track| t.filepath == track.filepath) {
            break; // the pool is exhausted; more calls would repeat
        }
        tracks.push(track);
    }
    Ok(Event::AutoDjSample { tracks, pool, note })
}

/// What the DJ's server offers (clause 19): its layered `/api/` — version,
/// discovery and readiness, libraries — or, on a server that answers only
/// the flat ping, the flags and libraries alone.
pub(crate) async fn dj_probe(client: &Client) -> Option<DjServerInfo> {
    match client.layered_info_async().await {
        Ok(info) => Some(DjServerInfo {
            version: info.server,
            discovery: info.features.discovery,
            discovery_ready: info.features.discovery_ready,
            libraries: info.user.vpaths,
        }),
        Err(_) => client.ping_async().await.ok().map(|ping| DjServerInfo {
            version: None,
            discovery: ping.discovery,
            discovery_ready: None,
            libraries: ping.vpaths,
        }),
    }
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
        // reconfigured since the ping. The 403 deliberately reads the same
        // for "switched off" and "nothing scanned yet"; the app follows up
        // with [`ApiCmd::DiscoveryProbe`] before naming a reason.
        return Ok(Event::Journey {
            stops: Vec::new(),
            note: None,
            length,
            issue: JourneyIssue::Disabled,
        });
    };

    let note = journey_note(&response, length);
    // An arc that couldn't be plotted has no stops worth showing; the note
    // carries the whole answer.
    let issue = if response.not_analyzed.any() {
        JourneyIssue::NotAnalyzed
    } else {
        JourneyIssue::None
    };
    let stops = if response.not_analyzed.any() { Vec::new() } else { response.results };
    Ok(Event::Journey { stops, note, length, issue })
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
    fn a_joi_rejection_names_its_key_escaped_or_not() {
        assert_eq!(not_allowed_key(r#"{"error":"\"minSimilarity\" is not allowed"}"#).as_deref(), Some("minSimilarity"));
        assert_eq!(not_allowed_key(r#""limit" is not allowed"#).as_deref(), Some("limit"));
        assert_eq!(not_allowed_key("No songs match criteria").as_deref(), None);
        assert_eq!(not_allowed_key("is not allowed").as_deref(), None, "no key, no lesson");
    }

    #[test]
    fn refusals_are_sorted_into_learn_degrade_auth_and_the_rest() {
        let server = |status: u16, message: &str| ApiError::Server { status, message: message.into() };
        assert_eq!(classify(&server(400, r#""limit" is not allowed"#), true), Refusal::Learn("limit".into()));
        assert_eq!(classify(&ApiError::Forbidden(r#""requireBpm" is not allowed"#.into()), false), Refusal::Learn("requireBpm".into()), "the body is the signal, not the status");
        assert_eq!(classify(&server(400, "No songs within the similarity range match criteria"), true), Refusal::Degrade(Some(DjNote::SonicRange)));
        assert_eq!(classify(&server(400, "Sonic seed track has not been analyzed yet"), true), Refusal::Degrade(Some(DjNote::SonicUnscanned)));
        assert_eq!(classify(&ApiError::Forbidden("discovery is disabled".into()), true), Refusal::Degrade(None));
        assert_eq!(classify(&ApiError::NotFound("Track not found".into()), true), Refusal::Degrade(None), "a dead pin");
        // The same words without a pool asked are not a degrade.
        assert_eq!(classify(&server(400, "No songs match criteria"), false), Refusal::NoMatch);
        assert_eq!(classify(&ApiError::Forbidden("discovery is disabled".into()), false), Refusal::Auth);
        assert_eq!(classify(&ApiError::Unauthorized, true), Refusal::Auth);
        assert!(matches!(classify(&ApiError::Network("refused".into()), true), Refusal::Network(_)));
    }

    #[test]
    fn the_learner_forgets_a_lanes_pool_with_the_lane_but_never_a_rejected_key() {
        let mut learned = DjLearner::default();
        let body = || serde_json::json!({"limit": 4, "similarTo": ["a"], "minSimilarity": 0.55, "ignoreList": []});
        assert!(learned.learn("http://s", "limit"));
        assert!(!learned.learn("http://s", "limit"), "not news twice");
        learned.suppress("http://s", 7, &SONIC_KEYS);
        let mut lane7 = body();
        let mut dropped = learned.filter("http://s", 7, &mut lane7);
        dropped.sort();
        assert_eq!(dropped, ["limit", "minSimilarity", "similarTo"]);
        assert!(lane7.get("ignoreList").is_some());
        let mut lane8 = body();
        assert_eq!(learned.filter("http://s", 8, &mut lane8), ["limit"], "a new lane asks for its pool again");
        let mut other = body();
        assert!(learned.filter("http://other", 7, &mut other).is_empty(), "keyed by server");
        assert!(learned.all_suppressed("http://s", 7, &SONIC_KEYS));
        assert!(!learned.all_suppressed("http://s", 8, &SONIC_KEYS));
    }

    /// The rig leg (plan T4): the real api thread against two live mStream
    /// servers paired over federation. Run with the parent's address in
    /// `MSTREAM_RIG_PARENT` (the server that lists the other as a peer):
    ///
    /// ```sh
    /// MSTREAM_RIG_PARENT=http://127.0.0.1:3041 cargo test -- --ignored rig
    /// ```
    ///
    /// Connect to the parent, read its peer list, ask for direct access to
    /// the first peer, dial the peer's own tunnel with the guest ticket,
    /// fetch a byte range of a track from the peer's loopback with the
    /// guest token and the loopback token, then close the tunnel.
    #[test]
    #[ignore = "needs the two-server rig; see the doc comment"]
    fn rig_a_peer_is_reached_directly_with_a_guest_ticket() {
        use crate::api::types::DirectAnswer;
        use std::time::Duration;
        let Ok(parent) = std::env::var("MSTREAM_RIG_PARENT") else {
            eprintln!("MSTREAM_RIG_PARENT not set — skipping");
            return;
        };
        let (events_tx, events) = mpsc::channel();
        let api = spawn_api(events_tx);
        let wait = |what: &str, pick: &dyn Fn(&Event) -> bool| -> Event {
            let deadline = std::time::Instant::now() + Duration::from_secs(90);
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                let event = events.recv_timeout(left).unwrap_or_else(|_| panic!("timed out waiting for {what}"));
                if pick(&event) {
                    return event;
                }
                eprintln!("  (skipping {event:?})");
            }
        };

        api.send(ApiCmd::Connect {
            server: parent.clone(),
            identity: parent.clone(),
            token: None,
            self_signed: false,
            peer: None,
            local_token: None,
        })
        .unwrap();
        let connected = wait("Connected", &|e| matches!(e, Event::Connected { .. } | Event::NeedsLogin { .. } | Event::Error(_)));
        let Event::Connected { ping, .. } = connected else { panic!("the rig's parent must be public: {connected:?}") };
        assert!(ping.federation_direct, "the parent offers direct access");
        assert!(ping.federation_browse);

        api.send(ApiCmd::FederationPeers { parent: parent.clone() }).unwrap();
        let listed = wait("FederationPeers", &|e| matches!(e, Event::FederationPeers { .. }));
        let Event::FederationPeers { peers: Some(peers), .. } = listed else { panic!("{listed:?}") };
        let peer = peers.first().expect("the parent lists a peer").clone();
        eprintln!("peer {} (id {})", peer.name, peer.id);

        let reach = crate::tui::app::Reach {
            base: parent.clone(),
            token: None,
            self_signed: false,
            peer: None,
            local_token: None,
        };
        api.send(ApiCmd::DirectAccess { parent: parent.clone(), id: peer.id, reach, refresh: false }).unwrap();
        let answer = wait("DirectAccess", &|e| matches!(e, Event::DirectAccess { .. }));
        let Event::DirectAccess { answer: DirectAnswer::Granted(ticket), .. } = answer else {
            panic!("the parent grants a ticket: {answer:?}")
        };
        assert!(ticket.ticket.starts_with("mstrfedg1:"));
        assert!(ticket.expires_at.is_some(), "the guest JWT carries its times");

        let pid = crate::config::peer_identity(&parent, peer.id);
        api.send(ApiCmd::TunnelOpen { id: pid.clone(), credential: ticket.ticket.clone() }).unwrap();
        let up = wait("TunnelUp", &|e| matches!(e, Event::TunnelUp { .. } | Event::TunnelFailed { .. }));
        let Event::TunnelUp { local_url, local_token, .. } = up else { panic!("the peer's tunnel comes up: {up:?}") };
        eprintln!("peer tunnel at {local_url}");

        // The peer's own `/api` answers the guest, and its bytes come plain.
        let client = Client::new(&local_url)
            .unwrap()
            .with_token(Some(ticket.guest_token.clone()))
            .with_local_token(Some(local_token.clone()));
        let info = client.ping_via_info().expect("the peer answers its layered /api to a guest");
        assert!(info.vpaths.iter().any(|v| v == "demo"), "the granted library is visible: {:?}", info.vpaths);
        let url = crate::api::urls::with_local_token(
            crate::api::urls::media_url(&local_url, "demo/Boukmanflow/6AM.mp3", Some(&ticket.guest_token)).unwrap(),
            Some(&local_token),
        );
        let bytes = crate::runtime::block_on(async {
            let resp = reqwest::Client::new()
                .get(&url)
                .header("range", "bytes=0-99")
                .send()
                .await
                .expect("the range request reaches the peer");
            assert_eq!(resp.status().as_u16(), 206, "{url}");
            resp.bytes().await.unwrap().len()
        })
        .unwrap();
        assert_eq!(bytes, 100);

        // A second peer tunnel request for the same identity re-reports it.
        api.send(ApiCmd::TunnelOpen { id: pid.clone(), credential: ticket.ticket.clone() }).unwrap();
        wait("TunnelUp again", &|e| matches!(e, Event::TunnelUp { .. }));

        api.send(ApiCmd::TunnelClose { id: pid.clone() }).unwrap();
        wait("TunnelClosed", &|e| matches!(e, Event::TunnelClosed { .. }));
        api.send(ApiCmd::Shutdown).unwrap();
    }

    #[test]
    fn the_access_answer_is_sorted_into_a_ticket_a_refusal_or_a_retry() {
        use crate::api::types::{DirectAccessResponse, DirectAnswer};
        use base64::Engine as _;
        // A guest JWT with readable times: issued at 1700000000, one day long.
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"federationGuest":true,"iat":1700000000,"exp":1700086400}"#);
        let jwt = format!("eyJhbGciOiJIUzI1NiJ9.{claims}.sig");
        let granted = DirectAccessResponse {
            direct: true,
            reason: None,
            endpoint_ticket: Some("endpointabc".into()),
            endpoint_id: Some("abc".into()),
            guest_token: Some(jwt.clone()),
            expires_at: Some("2023-11-15T22:13:20.000Z".into()),
            direct_ticket: Some("mstrfedg1:eyJ0IjoiZW5kcG9pbnRhYmMiLCJnIjoiLi4uIn0".into()),
        };
        match direct_answer(Ok(granted)) {
            DirectAnswer::Granted(ticket) => {
                assert_eq!(ticket.guest_token, jwt);
                assert_eq!(ticket.endpoint_id.as_deref(), Some("abc"));
                let epoch = std::time::UNIX_EPOCH;
                assert_eq!(ticket.issued_at, Some(epoch + std::time::Duration::from_secs(1_700_000_000)));
                assert_eq!(ticket.expires_at, Some(epoch + std::time::Duration::from_secs(1_700_086_400)));
            }
            other => panic!("a full grant is a ticket, got {other:?}"),
        }

        // `direct: false` is the parent's word: the proxy for the session.
        let denied = DirectAccessResponse {
            direct: false,
            reason: Some("peer does not offer guest access".into()),
            ..Default::default()
        };
        assert_eq!(
            direct_answer(Ok(denied)),
            DirectAnswer::Denied("peer does not offer guest access".into())
        );

        // A 200 missing its ticket, and a peer unreachable for the mint
        // (the parent's 502), are transient.
        let half = DirectAccessResponse { direct: true, ..Default::default() };
        assert!(matches!(direct_answer(Ok(half)), DirectAnswer::Failed(_)));
        let down = ApiError::Server { status: 502, message: "Peer unreachable".into() };
        assert!(matches!(direct_answer(Err(down)), DirectAnswer::Failed(why) if why.contains("502") || why.contains("unreachable")));

        // A token whose payload is not JSON still yields a ticket, with no
        // times to schedule by.
        let opaque = DirectAccessResponse {
            direct: true,
            guest_token: Some("not.a.jwt".into()),
            direct_ticket: Some("mstrfedg1:xyz".into()),
            ..Default::default()
        };
        match direct_answer(Ok(opaque)) {
            DirectAnswer::Granted(ticket) => assert_eq!((ticket.issued_at, ticket.expires_at), (None, None)),
            other => panic!("{other:?}"),
        }
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
}
