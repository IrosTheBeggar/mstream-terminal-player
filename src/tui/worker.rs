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

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::api::types::{
    Album, Capabilities, DirListing, Genre, JourneyStop, Ping, PlaylistSummary, SearchResults,
    SimilarArtist, Track,
};
use crate::api::{ApiError, Client};
use crate::discovery::DiscoveredServer;
use crate::dj;
use crate::engine::Engine;
use crate::player::{PlayerCtl, PlayerStatus};

/// How often the audio thread ticks the engine and publishes status. Also the
/// upper bound on command latency, so keep it small enough to feel instant.
const TICK: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, PartialEq)]
pub enum AudioCmd {
    Play { url: String, duration_hint: Option<f64> },
    Pause,
    Resume,
    Stop,
    Seek(f64),
    SetVolume(f32),
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
    Library(LibraryNode),
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
    Discover { node: DiscoverNode, seed: Box<Track> },
    Playlists,
    LoadPlaylist(String),
    Search(String),
    /// Open an artist or album that a search turned up. The same request the
    /// Library tab makes, under its own name so the reply can be told apart:
    /// one event with two possible destinations is how a slow reply ends up
    /// in the wrong column.
    SearchDrill(LibraryNode),
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
    /// The mode menu — static, needs no request.
    Root,
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
    Tracks(Vec<Track>),
    Artists(Vec<SimilarArtist>),
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
}

#[derive(Debug)]
pub enum LibraryData {
    Artists(Vec<String>),
    Albums(Vec<Album>),
    Genres(Vec<Genre>),
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
    /// The current track finished on its own (not a user stop).
    TrackEnded,
    /// The audio device could not be opened; playback is unavailable.
    AudioFailed(String),
    /// One source would not play — wrong format, gone from the server, or
    /// something this decoder doesn't speak. The rest of the queue is fine.
    PlaybackFailed(String),
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
    /// Contents of a library view, tagged with the node they belong to.
    Library { node: LibraryNode, data: LibraryData },
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
    Journey { stops: Vec<JourneyStop>, note: Option<String> },
    /// A Discover view's contents, tagged with the node they belong to.
    Discover { node: DiscoverNode, data: DiscoverData, note: Option<String> },
    /// Contents of an artist or album reached from the search results.
    SearchDrill { node: LibraryNode, data: LibraryData },
    Playlists(Vec<PlaylistSummary>),
    PlaylistTracks { name: String, tracks: Vec<Track> },
    SearchResults(Box<SearchResults>),
    /// Credentials are missing or expired — the UI drops back to the
    /// connect screen.
    Unauthorized,
    Error(String),
}

// ── Audio thread ────────────────────────────────────────────────────────────

pub fn spawn_audio(events: Sender<Event>) -> Sender<AudioCmd> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("mstream-audio".into())
        .spawn(move || audio_loop(&rx, &events))
        .expect("failed to spawn audio thread");
    tx
}

fn audio_loop(rx: &Receiver<AudioCmd>, events: &Sender<Event>) {
    let engine = match Engine::new() {
        Ok(e) => e,
        Err(e) => {
            let _ = events.send(Event::AudioFailed(e.to_string()));
            // Keep draining commands so the UI's sends don't fail; it stays
            // usable for browsing even with no audio device.
            while let Ok(cmd) = rx.recv() {
                if cmd == AudioCmd::Shutdown {
                    break;
                }
            }
            return;
        }
    };
    let player: &dyn PlayerCtl = &engine;

    // Track-end detection lives here rather than in the UI: this thread sees
    // every status transition, so it can tell "the track finished" from "the
    // user pressed stop" without the UI having to infer it from polling.
    let mut had_source = false;
    let mut suppress_end = false;

    loop {
        match rx.recv_timeout(TICK) {
            Ok(AudioCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(cmd) => {
                // A source that won't play is a different kind of problem
                // from a command that failed: the queue can carry on past it,
                // and should, so it gets its own event.
                let starting = matches!(cmd, AudioCmd::Play { .. });
                if let Some(err) = apply_audio_cmd(player, cmd, &mut suppress_end) {
                    let event = if starting {
                        Event::PlaybackFailed(err)
                    } else {
                        Event::Error(err)
                    };
                    let _ = events.send(event);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        player.tick();
        let status = player.status();
        let has_source = !status.source.is_empty();
        if had_source && !has_source {
            if suppress_end {
                suppress_end = false;
            } else {
                let _ = events.send(Event::TrackEnded);
            }
        }
        had_source = has_source;

        if events.send(Event::Status(status)).is_err() {
            break; // UI is gone
        }
    }

    player.stop();
}

fn apply_audio_cmd(
    player: &dyn PlayerCtl,
    cmd: AudioCmd,
    suppress_end: &mut bool,
) -> Option<String> {
    match cmd {
        AudioCmd::Play { url, duration_hint } => {
            // Swapping tracks keeps a source loaded throughout, so this is not
            // an end-of-track transition.
            *suppress_end = true;
            return player.play(&url, duration_hint).err();
        }
        AudioCmd::Pause => player.pause(),
        AudioCmd::Resume => player.resume(),
        AudioCmd::Stop => {
            *suppress_end = true;
            player.stop();
        }
        AudioCmd::Seek(pos) => return player.seek(pos).err(),
        AudioCmd::SetVolume(v) => player.set_volume(v),
        AudioCmd::Shutdown => {}
    }
    None
}

/// Browse for servers on its own thread — mDNS listens for a fixed window, and
/// that shouldn't hold up a pairing attempt queued behind it.
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
const DISCOVERY_WINDOW: Duration = Duration::from_secs(3);

// ── API thread ──────────────────────────────────────────────────────────────

pub fn spawn_api(events: Sender<Event>) -> Sender<ApiCmd> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("mstream-api".into())
        .spawn(move || api_loop(&rx, &events))
        .expect("failed to spawn api thread");
    tx
}

fn api_loop(rx: &Receiver<ApiCmd>, events: &Sender<Event>) {
    let mut client: Option<Client> = None;
    // Held for as long as this thread lives; dropping it closes the tunnel out
    // from under the client, so it is explicitly dropped on the way out.
    #[allow(unused_assignments)]
    let mut bridge: Option<crate::quickconnect::TunnelBridge> = None;
    // The tunnel session's two names, once one is open: the loopback address
    // requests go to, and the identity it is remembered by.
    let mut tunnel: Option<(String, String)> = None;
    // What the connected server said it can do. Nothing optional is probed
    // before this says so.
    let mut caps = Capabilities::default();

    while let Ok(cmd) = rx.recv() {
        let result = match cmd {
            ApiCmd::Shutdown => break,

            ApiCmd::Connect { server, token } => {
                connect(&mut client, &server, &server.clone(), token, events)
            }

            ApiCmd::Login { server, username, password } => {
                // Signing in to a tunnel server goes over the open bridge, but
                // is filed under the endpoint id — the loopback port is gone
                // by the next run.
                let (endpoint, id) = resolve_target(&server, tunnel.as_ref());
                login(&mut client, &endpoint, &id, &username, &password, events)
            }

            ApiCmd::QuickConnect { code, token } => match quick_connect(&code) {
                Ok((id, opened)) => {
                    let url = opened.local_url.clone();
                    bridge = Some(opened);
                    tunnel = Some((url.clone(), id.clone()));
                    // A public-mode server answers straight away; anything else
                    // needs a login over the freshly-opened tunnel.
                    match connect(&mut client, &url, &id, token, events) {
                        Some(Event::NeedsLogin { .. }) => {
                            Some(Event::TunnelReady { local_url: url, id })
                        }
                        other => other,
                    }
                }
                Err(e) => Some(Event::Error(e)),
            },

            ApiCmd::Browse(path) => with_client(client.as_ref(), |c| {
                c.file_explorer(&path).map(|l| Event::Listing(Box::new(l)))
            }),

            ApiCmd::SearchDrill(node) => with_client(client.as_ref(), |c| {
                load_library(c, &node).map(|data| Event::SearchDrill { node: node.clone(), data })
            }),
            ApiCmd::Library(node) => with_client(client.as_ref(), |c| {
                load_library(c, &node).map(|data| Event::Library { node: node.clone(), data })
            }),

            ApiCmd::AutoDj(request) => with_client(client.as_ref(), |c| {
                autodj_pick(c, caps, &request).map(|picked| Event::AutoDjPick {
                    candidates: picked.tracks,
                    ignore_list: picked.ignore_list,
                    note: picked.note,
                })
            }),

            ApiCmd::AutoDjSample { request, count } => with_client(client.as_ref(), |c| {
                autodj_sample(c, caps, &request, count)
            }),

            ApiCmd::Genres => {
                with_client(client.as_ref(), |c| c.genres().map(Event::Genres))
            }

            ApiCmd::Journey { start, end, length } => with_client(client.as_ref(), |c| {
                journey(c, &start, &end, length)
            }),

            ApiCmd::Discover { node, seed } => {
                with_client(client.as_ref(), |c| discover(c, &node, &seed))
            }

            ApiCmd::Playlists => {
                with_client(client.as_ref(), |c| c.playlists().map(Event::Playlists))
            }

            ApiCmd::LoadPlaylist(name) => with_client(client.as_ref(), |c| {
                c.playlist_load(&name)
                    .map(|tracks| Event::PlaylistTracks { name: name.clone(), tracks })
            }),

            ApiCmd::Search(query) => with_client(client.as_ref(), |c| {
                c.search(&query).map(|r| Event::SearchResults(Box::new(r)))
            }),
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

/// Parse a pairing code, bring the tunnel up on loopback, and report the
/// identity the code names alongside it.
fn quick_connect(code: &str) -> Result<(String, crate::quickconnect::TunnelBridge), String> {
    let parsed = crate::quickconnect::parse_code(code)?;
    let id = parsed.server_id();
    Ok((id, crate::quickconnect::open_bridge(&parsed)?))
}

/// Split a connect target into (where to send bytes, what to remember it as).
/// They differ only for a tunnel, which the UI names either way round: by its
/// identity when reconnecting, by the loopback URL when the login form is
/// carrying what the tunnel just published.
fn resolve_target(server: &str, tunnel: Option<&(String, String)>) -> (String, String) {
    match tunnel {
        Some((local_url, id)) if server == id || server == local_url => {
            (local_url.clone(), id.clone())
        }
        _ => (server.to_string(), server.to_string()),
    }
}

fn connect(
    client: &mut Option<Client>,
    server: &str,
    id: &str,
    token: Option<String>,
    _events: &Sender<Event>,
) -> Option<Event> {
    let c = match Client::new(server) {
        Ok(c) => c.with_token(token.clone()),
        Err(e) => return Some(Event::Error(e.to_string())),
    };
    match c.ping() {
        Ok(ping) => {
            let server = c.server();
            *client = Some(c);
            Some(Event::Connected {
                server,
                id: id.to_string(),
                username: None,
                token,
                ping: Box::new(ping),
            })
        }
        // Reaching the server and being asked to sign in is a normal outcome
        // of picking one, not an authorization failure.
        Err(ApiError::Unauthorized) => Some(Event::NeedsLogin { server: c.server() }),
        Err(e) => Some(Event::Error(e.to_string())),
    }
}

fn login(
    client: &mut Option<Client>,
    server: &str,
    id: &str,
    username: &str,
    password: &str,
    _events: &Sender<Event>,
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
    match c.ping() {
        Ok(ping) => {
            let server = c.server();
            *client = Some(c);
            Some(Event::Connected {
                server,
                id: id.to_string(),
                username: Some(username.to_string()),
                token: Some(token),
                ping: Box::new(ping),
            })
        }
        Err(e) => Some(Event::Error(e.to_string())),
    }
}

fn load_library(client: &Client, node: &LibraryNode) -> Result<LibraryData, ApiError> {
    Ok(match node {
        // The mode menu is static; the UI fills it in without asking.
        LibraryNode::Root => LibraryData::Artists(Vec::new()),
        LibraryNode::Artists => LibraryData::Artists(client.artists()?),
        LibraryNode::Artist(artist) => LibraryData::Albums(client.artist_albums(artist)?),
        LibraryNode::Albums => LibraryData::Albums(client.albums()?),
        LibraryNode::Album { name, artist } => {
            LibraryData::Tracks(client.album_songs(name, artist.as_deref())?)
        }
        LibraryNode::Genres => LibraryData::Genres(client.genres()?),
        LibraryNode::Genre(genre) => LibraryData::Tracks(client.genre_songs(genre)?),
        LibraryNode::Recent => LibraryData::Tracks(client.recently_added(RECENT_LIMIT)?),
    })
}

/// One answer from the picker.
struct Picked {
    tracks: Vec<Track>,
    ignore_list: Vec<u32>,
    note: Option<String>,
    pool: Option<crate::api::types::SonicReport>,
}

type AutoDjResult = Result<Picked, ApiError>;

/// Choose what Auto-DJ should play next.
///
/// Similarity is best-effort: the server may have discovery switched off, or
/// simply not have embedded this track yet. Rather than stalling, both cases
/// fall through to tempo/key matching and say why.
fn autodj_pick(client: &Client, caps: Capabilities, request: &DjRequest) -> AutoDjResult {
    let ignore_list = request.ignore_list.clone();
    match request.mode {
        AutoDjMode::Off => {
            Ok(Picked { tracks: Vec::new(), ignore_list, note: None, pool: None })
        }

        AutoDjMode::BpmKey => pick_by_tempo_and_key(client, request, None),

        AutoDjMode::Similar => {
            let Some(seed) = request.seed.as_deref() else {
                return pick_by_tempo_and_key(client, request, None);
            };
            // No flag, no probe: ping already said there is no index here, so
            // asking would spend a round trip to be told 403.
            if !caps.discovery {
                return pick_by_tempo_and_key(
                    client,
                    request,
                    Some("this server has no similarity index — matching tempo and key"),
                );
            }
            match client.similar_tracks(&seed.filepath, SIMILAR_LIMIT)? {
                // Backstop for a server reconfigured mid-session; the flag
                // above is what normally keeps us out of here.
                None => pick_by_tempo_and_key(
                    client,
                    request,
                    Some("similarity was switched off on this server — matching tempo and key"),
                ),
                Some(found) if found.not_analyzed => pick_by_tempo_and_key(
                    client,
                    request,
                    Some("this track hasn't been analysed yet — matching tempo and key"),
                ),
                Some(found) if found.results.is_empty() => pick_by_tempo_and_key(
                    client,
                    request,
                    Some("nothing sounded similar — matching tempo and key"),
                ),
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

fn pick_by_tempo_and_key(
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

    let response = match client.random_song(&body) {
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
            let response = client.random_song(&relaxed)?;
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
fn autodj_sample(
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
        let picked = match autodj_pick(client, caps, &scratch) {
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
fn discover(
    client: &Client,
    node: &DiscoverNode,
    seed: &Track,
) -> Result<Event, ApiError> {
    let disabled = |data| Event::Discover {
        node: node.clone(),
        data,
        note: Some("discovery is switched off on this server".into()),
    };

    match node {
        // Both are answered without asking the server: the mode menu is
        // static, and an artist's ways in arrived with the artist list.
        DiscoverNode::Root | DiscoverNode::Artist(_) => Ok(Event::Discover {
            node: node.clone(),
            data: DiscoverData::Tracks(Vec::new()),
            note: None,
        }),

        DiscoverNode::Tracks => {
            let Some(found) = client.similar_tracks(&seed.filepath, DISCOVER_LIMIT)? else {
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
                data: DiscoverData::Tracks(
                    found.results.into_iter().map(|r| r.into_track()).collect(),
                ),
                note,
            })
        }

        DiscoverNode::Artists => {
            let Some(artist) = seed.metadata.artist.as_deref().filter(|a| !a.trim().is_empty())
            else {
                return Ok(Event::Discover {
                    node: node.clone(),
                    data: DiscoverData::Artists(Vec::new()),
                    note: Some("this track has no artist tag to compare against".into()),
                });
            };
            let Some(found) = client.similar_artists(artist, DISCOVER_LIMIT)? else {
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
            })
        }
    }
}

/// Fetch a journey and translate the ways it can legitimately come up short
/// into something worth reading.
fn journey(client: &Client, start: &str, end: &str, length: u32) -> Result<Event, ApiError> {
    let Some(response) = client.journey(start, end, length)? else {
        // Gated on `discoveryPath`, so this only happens if the server was
        // reconfigured since the ping.
        return Ok(Event::Journey {
            stops: Vec::new(),
            note: Some("discovery is switched off on this server".into()),
        });
    };

    let note = journey_note(&response, length);
    // An arc that couldn't be plotted has no stops worth showing; the note
    // carries the whole answer.
    let stops = if response.not_analyzed.any() { Vec::new() } else { response.results };
    Ok(Event::Journey { stops, note })
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

/// Run a request against the connected client, mapping failures onto events.
fn with_client<F>(client: Option<&Client>, f: F) -> Option<Event>
where
    F: FnOnce(&Client) -> Result<Event, ApiError>,
{
    let Some(client) = client else {
        return Some(Event::Error("not connected to a server".into()));
    };
    match f(client) {
        Ok(event) => Some(event),
        // Only 401 means the session is no good; 403 is a permission or
        // feature-flag answer that shouldn't bounce the user to a login form.
        Err(ApiError::Unauthorized) => Some(Event::Unauthorized),
        Err(e) => Some(Event::Error(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
