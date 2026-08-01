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

use crate::api::types::{DirListing, Ping, PlaylistSummary, SearchResults, Track};
use crate::api::{ApiError, Client};
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
    Browse(String),
    Playlists,
    LoadPlaylist(String),
    Search(String),
    Shutdown,
}

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
    Listing(Box<DirListing>),
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

    while let Ok(cmd) = rx.recv() {
        let result = match cmd {
            ApiCmd::Shutdown => break,

            ApiCmd::Connect { server, token } => connect(&mut client, &server, token, events),

            ApiCmd::Login { server, username, password } => {
                login(&mut client, &server, &username, &password, events)
            }

            ApiCmd::Browse(path) => with_client(client.as_ref(), |c| {
                c.file_explorer(&path).map(|l| Event::Listing(Box::new(l)))
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
        Err(ApiError::Unauthorized) => Some(Event::Unauthorized),
        Err(e) => Some(Event::Error(e.to_string())),
    }
}
