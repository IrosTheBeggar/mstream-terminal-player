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
    Album, BpmWindow, DirListing, Genre, Ping, PlaylistSummary, RandomSongRequest, SearchResults,
    Track,
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
    /// address as an ordinary server.
    QuickConnect { code: String },
    Browse(String),
    Library(LibraryNode),
    /// Ask for the next Auto-DJ track, seeded on what's playing now.
    AutoDj { mode: AutoDjMode, seed: Option<Box<Track>>, ignore_list: Vec<u32> },
    Playlists,
    LoadPlaylist(String),
    Search(String),
    Shutdown,
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
    Connected {
        server: String,
        username: Option<String>,
        token: Option<String>,
        ping: Box<Ping>,
    },
    /// Servers that answered an mDNS browse.
    ServersDiscovered(Vec<DiscoveredServer>),
    /// The Quick Connect tunnel is up and reachable at `local_url`, but the
    /// server still wants credentials — the secret gates the pipe, not the API.
    TunnelReady { local_url: String },
    Listing(Box<DirListing>),
    /// Contents of a library view, tagged with the node they belong to.
    Library { node: LibraryNode, data: LibraryData },
    /// Auto-DJ candidates, best first. `note` explains any fallback that had
    /// to happen so the UI can say so out loud.
    AutoDjPick { candidates: Vec<Track>, ignore_list: Vec<u32>, note: Option<String> },
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
                if let Some(err) = apply_audio_cmd(player, cmd, &mut suppress_end) {
                    let _ = events.send(Event::Error(err));
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

    while let Ok(cmd) = rx.recv() {
        let result = match cmd {
            ApiCmd::Shutdown => break,

            ApiCmd::Connect { server, token } => connect(&mut client, &server, token, events),

            ApiCmd::Login { server, username, password } => {
                login(&mut client, &server, &username, &password, events)
            }

            ApiCmd::QuickConnect { code } => match quick_connect(&code) {
                Ok(opened) => {
                    let url = opened.local_url.clone();
                    bridge = Some(opened);
                    // A public-mode server answers straight away; anything else
                    // needs a login over the freshly-opened tunnel.
                    match connect(&mut client, &url, None, events) {
                        Some(Event::Unauthorized) => Some(Event::TunnelReady { local_url: url }),
                        other => other,
                    }
                }
                Err(e) => Some(Event::Error(e)),
            },

            ApiCmd::Browse(path) => with_client(client.as_ref(), |c| {
                c.file_explorer(&path).map(|l| Event::Listing(Box::new(l)))
            }),

            ApiCmd::Library(node) => with_client(client.as_ref(), |c| {
                load_library(c, &node).map(|data| Event::Library { node: node.clone(), data })
            }),

            ApiCmd::AutoDj { mode, seed, ignore_list } => with_client(client.as_ref(), |c| {
                autodj_pick(c, mode, seed.as_deref(), ignore_list).map(
                    |(candidates, ignore_list, note)| Event::AutoDjPick {
                        candidates,
                        ignore_list,
                        note,
                    },
                )
            }),

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

        if let Some(event) = result {
            if events.send(event).is_err() {
                break;
            }
        }
    }

    drop(bridge);
}

/// Parse a pairing code and bring the tunnel up on loopback.
fn quick_connect(code: &str) -> Result<crate::quickconnect::TunnelBridge, String> {
    let parsed = crate::quickconnect::parse_code(code)?;
    crate::quickconnect::open_bridge(&parsed)
}

fn connect(
    client: &mut Option<Client>,
    server: &str,
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
            Some(Event::Connected { server, username: None, token, ping: Box::new(ping) })
        }
        Err(ApiError::Unauthorized) => Some(Event::Unauthorized),
        Err(e) => Some(Event::Error(e.to_string())),
    }
}

fn login(
    client: &mut Option<Client>,
    server: &str,
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

type AutoDjResult = Result<(Vec<Track>, Vec<u32>, Option<String>), ApiError>;

/// Choose what Auto-DJ should play next.
///
/// Similarity is best-effort: the server may have discovery switched off, or
/// simply not have embedded this track yet. Rather than stalling, both cases
/// fall through to tempo/key matching and say why.
fn autodj_pick(
    client: &Client,
    mode: AutoDjMode,
    seed: Option<&Track>,
    ignore_list: Vec<u32>,
) -> AutoDjResult {
    match mode {
        AutoDjMode::Off => Ok((Vec::new(), ignore_list, None)),

        AutoDjMode::BpmKey => pick_by_tempo_and_key(client, seed, ignore_list, None),

        AutoDjMode::Similar => {
            let Some(seed) = seed else {
                return pick_by_tempo_and_key(client, None, ignore_list, None);
            };
            match client.similar_tracks(&seed.filepath, SIMILAR_LIMIT)? {
                None => pick_by_tempo_and_key(
                    client,
                    Some(seed),
                    ignore_list,
                    Some("similarity is switched off on this server — matching tempo and key"),
                ),
                Some(found) if found.not_analyzed => pick_by_tempo_and_key(
                    client,
                    Some(seed),
                    ignore_list,
                    Some("this track hasn't been analysed yet — matching tempo and key"),
                ),
                Some(found) if found.results.is_empty() => pick_by_tempo_and_key(
                    client,
                    Some(seed),
                    ignore_list,
                    Some("nothing sounded similar — matching tempo and key"),
                ),
                Some(found) => Ok((
                    found.results.into_iter().map(|r| r.into_track()).collect(),
                    ignore_list,
                    None,
                )),
            }
        }
    }
}

fn pick_by_tempo_and_key(
    client: &Client,
    seed: Option<&Track>,
    ignore_list: Vec<u32>,
    note: Option<&str>,
) -> AutoDjResult {
    let mut request = RandomSongRequest { ignore_list, ..Default::default() };
    let mut note = note.map(str::to_string);

    if let Some(seed) = seed {
        if let Some(bpm) = seed.metadata.bpm {
            let to_window = |r: dj::BpmRange| BpmWindow { min: r.min, max: r.max };
            request.bpm_ranges = dj::bpm_windows(f64::from(bpm), dj::TIGHT_TOLERANCE)
                .into_iter()
                .map(to_window)
                .collect();
            request.bpm_ranges_wide = dj::bpm_windows(f64::from(bpm), dj::WIDE_TOLERANCE)
                .into_iter()
                .map(to_window)
                .collect();
        }
        request.musical_keys = dj::compatible_keys(seed.metadata.musical_key.as_deref());

        // Be honest when there was nothing to match on: the pick is really
        // just random, and the user should know that rather than assume the
        // tempo matching is broken.
        if request.bpm_ranges.is_empty() && request.musical_keys.is_empty() && note.is_none() {
            note = Some("no tempo or key tags on this track — picking at random".to_string());
        }
    }

    let response = client.random_song(&request)?;
    Ok((response.songs, response.ignore_list, note))
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
