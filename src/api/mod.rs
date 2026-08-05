//! Client for the mStream v1 JSON API.
//!
//! Sync on the outside, async underneath on the shared runtime — the TUI will
//! call these from a worker thread. Auth is the raw JWT in an `x-access-token`
//! header (never `Bearer`, and never in the query string for API calls, so it
//! stays out of server logs); only *stream* URLs carry `?token=`, which is what
//! lets them be handed straight to the playback engine.
//!
//! Servers with no users configured run in "public mode" and authenticate every
//! request, so a token-less client is valid and supported.

pub mod server_url;
pub mod types;
pub mod urls;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;

use crate::runtime;
use types::*;
use urls::TranscodeCodec;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The directory to ask [`Client::file_explorer`] for when what you want is
/// "wherever it makes sense to start".
///
/// The server resolves it: one library and you land inside it, several and
/// you get the list to choose from. Asking for `""` always gives the list —
/// which on the common single-library setup is one row that everyone has to
/// step through before reaching any music.
pub const BEST_START: &str = "~";

#[derive(Debug)]
pub enum ApiError {
    /// Could not reach the server at all.
    Network(String),
    /// 401 — no token, expired token, or bad credentials.
    Unauthorized,
    /// 403 — authenticated but not allowed. mStream also uses this for
    /// "feature disabled" and for request-validation failures, so it must not
    /// be confused with [`ApiError::Unauthorized`]: it never means "log in
    /// again".
    Forbidden(String),
    NotFound(String),
    /// Any other non-2xx, with the server's `error` message when it sent one.
    Server { status: u16, message: String },
    /// 2xx whose body wasn't the shape we expected.
    Decode { endpoint: String, message: String },
    Config(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(e) => write!(f, "could not reach server: {e}"),
            ApiError::Unauthorized => {
                write!(f, "not authorized — run `mstream-player login` (or the token expired)")
            }
            ApiError::Forbidden(what) => write!(f, "not permitted: {what}"),
            ApiError::NotFound(what) => write!(f, "not found: {what}"),
            ApiError::Server { status, message } => write!(f, "server error {status}: {message}"),
            ApiError::Decode { endpoint, message } => {
                write!(f, "unexpected response from {endpoint}: {message}")
            }
            ApiError::Config(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ApiError {}

pub struct Client {
    http: reqwest::Client,
    /// Always ends in `/` so `Url::join` appends instead of replacing the last
    /// segment — required for servers hosted under a reverse-proxy subpath.
    base: Url,
    token: Option<String>,
    /// Set once this server has shown it can't answer a listing that asks for
    /// metadata, so the fallback costs one wasted request per session rather
    /// than one per folder.
    plain_listings: AtomicBool,
}

impl Client {
    pub fn new(server: &str) -> Result<Self, ApiError> {
        let mut base = Url::parse(server)
            .map_err(|e| ApiError::Config(format!("invalid server URL '{server}': {e}")))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(ApiError::Config(format!(
                "server URL must be http or https, got '{}'",
                base.scheme()
            )));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }

        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| ApiError::Config(format!("could not build http client: {e}")))?;

        Ok(Client { http, base, token: None, plain_listings: AtomicBool::new(false) })
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    /// Build a client from explicit overrides, falling back to the most
    /// recently used server. A stored token is looked up by server, so one is
    /// never sent to a host that didn't issue it.
    ///
    /// The config file is read only where an answer is actually missing.
    /// Loading it up front made a broken config defeat `--server` and
    /// `--token` together — the two flags whose whole job is to be the way
    /// round a config you cannot load (finding #42).
    pub fn resolve(server: Option<&str>, token: Option<&str>) -> Result<Self, ApiError> {
        let server = match server {
            // Whatever a `--server` flag carries gets the same treatment as
            // something typed into the connect screen.
            Some(server) => server_url::normalize(server).map_err(ApiError::Config)?,
            None => Self::remembered_server()?,
        };

        let client = Client::new(&server)?;
        let token = match token {
            Some(token) => Some(token.to_string()),
            None => {
                let credentials = crate::config::load_credentials().map_err(ApiError::Config)?;
                crate::config::token_for(&credentials, &server)
            }
        };
        Ok(client.with_token(token))
    }

    /// The server a bare command means: the most recently used one.
    fn remembered_server() -> Result<String, ApiError> {
        let config = crate::config::load().map_err(ApiError::Config)?;
        match crate::config::most_recent_server(&config) {
            // A tunnel server is remembered by identity, not address, and
            // reaching it means dialling its pairing code — which only the
            // player does. Say so rather than failing on a parse.
            Some(entry) if crate::quickconnect::is_tunnel_id(&entry.url) => Err(ApiError::Config(
                "the last server was reached with Quick Connect, which these commands \
                 cannot dial — pass --server <url>, or use the player"
                    .to_string(),
            )),
            Some(entry) => Ok(entry.url.clone()),
            None => Err(ApiError::Config(
                "no server given and none remembered — run \
                 `mstream-player login --server <url> --user <name>`"
                    .to_string(),
            )),
        }
    }

    /// Server base URL, without the trailing slash, for building stream URLs.
    pub fn server(&self) -> String {
        self.base.as_str().trim_end_matches('/').to_string()
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// True when the base URL would put credentials on the wire in the clear
    /// beyond the local network — callers should warn before sending them.
    pub fn is_insecure_remote(&self) -> bool {
        server_url::crosses_the_internet_unencrypted(self.base.as_str())
    }

    // ── Plumbing ────────────────────────────────────────────────────────────

    fn endpoint(&self, path: &str) -> Result<Url, ApiError> {
        self.base
            .join(path)
            .map_err(|e| ApiError::Config(format!("could not build URL for {path}: {e}")))
    }

    fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, ApiError> {
        let url = self.endpoint(path)?;
        let mut req = self.http.request(method, url);
        if let Some(token) = &self.token {
            req = req.header("x-access-token", token);
        }
        if let Some(body) = &body {
            req = req.header("Content-Type", "application/json").body(
                serde_json::to_string(body)
                    .map_err(|e| ApiError::Config(format!("could not encode request: {e}")))?,
            );
        }

        let (status, text) = runtime::block_on(async move {
            let resp = req.send().await.map_err(|e| ApiError::Network(e.to_string()))?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| ApiError::Network(e.to_string()))?;
            Ok::<_, ApiError>((status, text))
        })
        .map_err(ApiError::Network)??;

        match status {
            StatusCode::UNAUTHORIZED => return Err(ApiError::Unauthorized),
            StatusCode::FORBIDDEN => return Err(ApiError::Forbidden(extract_error(&text))),
            StatusCode::NOT_FOUND => return Err(ApiError::NotFound(path.to_string())),
            s if !s.is_success() => {
                return Err(ApiError::Server {
                    status: s.as_u16(),
                    message: extract_error(&text),
                });
            }
            _ => {}
        }

        serde_json::from_str(&text).map_err(|e| ApiError::Decode {
            endpoint: path.to_string(),
            message: e.to_string(),
        })
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.send(Method::GET, path, None)
    }

    fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, ApiError> {
        self.send(Method::POST, path, Some(body))
    }

    // ── Endpoints ───────────────────────────────────────────────────────────

    /// Authenticate and store the returned token on this client.
    pub fn login(&mut self, username: &str, password: &str) -> Result<LoginResponse, ApiError> {
        let resp: LoginResponse = self.post(
            "api/v1/auth/login",
            serde_json::json!({ "username": username, "password": password }),
        )?;
        self.token = Some(resp.token.clone());
        Ok(resp)
    }

    /// Capability bootstrap — "called once after login".
    pub fn ping(&self) -> Result<Ping, ApiError> {
        self.get("api/v1/ping")
    }

    /// Browse a directory.
    ///
    /// An empty string lists the libraries (vpaths); [`BEST_START`] asks the
    /// server to pick the most useful place instead.
    /// `pullMetadata` costs the server one batched query for the whole folder
    /// and buys durations, BPM and key for every row — without it a track
    /// queued from the file browser is a filename with no length, and Auto-DJ
    /// seeded from one has no tempo to match.
    pub fn file_explorer(&self, directory: &str) -> Result<DirListing, ApiError> {
        if !self.plain_listings.load(Ordering::Relaxed) {
            let body =
                serde_json::json!({ "directory": directory, "pullMetadata": true });
            match self.post("api/v1/file-explorer", body) {
                Ok(listing) => return Ok(listing),
                // The server validates this body against a schema that rejects
                // keys it doesn't know, so one too old to have the parameter
                // answers 400 rather than ignoring it. Browsing has to work
                // everywhere and the tags are a nicety, so ask once and then
                // stop asking.
                Err(ApiError::Server { status: 400, .. }) => {
                    self.plain_listings.store(true, Ordering::Relaxed);
                }
                Err(e) => return Err(e),
            }
        }
        self.post("api/v1/file-explorer", serde_json::json!({ "directory": directory }))
    }

    pub fn artists(&self) -> Result<Vec<String>, ApiError> {
        let r: ArtistsResponse = self.get("api/v1/db/artists")?;
        Ok(r.artists)
    }

    pub fn albums(&self) -> Result<Vec<Album>, ApiError> {
        let r: AlbumsResponse = self.get("api/v1/db/albums")?;
        Ok(r.albums)
    }

    pub fn artist_albums(&self, artist: &str) -> Result<Vec<Album>, ApiError> {
        let r: AlbumsResponse =
            self.post("api/v1/db/artists-albums", serde_json::json!({ "artist": artist }))?;
        Ok(r.albums)
    }

    pub fn album_songs(&self, album: &str, artist: Option<&str>) -> Result<Vec<Track>, ApiError> {
        let mut body = serde_json::json!({ "album": album });
        if let Some(artist) = artist {
            body["artist"] = serde_json::Value::String(artist.to_string());
        }
        self.post("api/v1/db/album-songs", body)
    }

    /// Ask the Auto-DJ picker for one track matching the given constraints.
    ///
    /// The server answers 400 when nothing survives its fallback waterfall;
    /// that's an ordinary "no pick", so it comes back as an empty `songs`
    /// list rather than an error.
    pub fn random_song(
        &self,
        request: &RandomSongRequest,
    ) -> Result<RandomSongsResponse, ApiError> {
        let body = serde_json::to_value(request)
            .map_err(|e| ApiError::Config(format!("could not encode request: {e}")))?;
        match self.post("api/v1/db/random-songs", body) {
            Ok(response) => Ok(response),
            Err(ApiError::Server { status: 400, .. }) => Ok(RandomSongsResponse::default()),
            Err(e) => Err(e),
        }
    }

    /// Tracks that sound like `filepath`, nearest first.
    ///
    /// Returns `None` when the server has discovery collection switched off —
    /// it answers 403 by house convention for a disabled feature, which is a
    /// configuration state rather than a failure the user can act on.
    pub fn similar_tracks(
        &self,
        filepath: &str,
        limit: u32,
    ) -> Result<Option<SimilarTracksResponse>, ApiError> {
        let body = serde_json::json!({ "filePath": filepath, "limit": limit });
        match self.post("api/v1/discovery/local/similar/tracks", body) {
            Ok(response) => Ok(Some(response)),
            Err(ApiError::Forbidden(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Artists whose overall sound sits near this one's, each with up to two
    /// tracks that lead in from where the listener already is.
    ///
    /// `None` means discovery is switched off, as with
    /// [`Client::similar_tracks`].
    pub fn similar_artists(
        &self,
        artist: &str,
        limit: u32,
    ) -> Result<Option<SimilarArtistsResponse>, ApiError> {
        let body = serde_json::json!({ "artist": artist, "limit": limit });
        match self.post("api/v1/discovery/local/similar/artists", body) {
            Ok(response) => Ok(Some(response)),
            Err(ApiError::Forbidden(_)) => Ok(None),
            // The server treats an artist with nothing visible to this user
            // as one that doesn't exist; an empty list says that better than
            // an error does.
            Err(ApiError::NotFound(_)) => Ok(Some(SimilarArtistsResponse::default())),
            Err(e) => Err(e),
        }
    }

    /// A journey from one track to another through the embedding space.
    ///
    /// `length` counts the total rows including both seeds, so the answer is
    /// the queue. Like [`Client::similar_tracks`], `None` means the server has
    /// discovery switched off rather than that anything went wrong.
    pub fn journey(
        &self,
        start: &str,
        end: &str,
        length: u32,
    ) -> Result<Option<JourneyResponse>, ApiError> {
        let body = serde_json::json!({
            "startFilePath": start,
            "endFilePath": end,
            "length": length.clamp(JOURNEY_MIN_LENGTH, JOURNEY_MAX_LENGTH),
        });
        match self.post("api/v1/discovery/local/path", body) {
            Ok(response) => Ok(Some(response)),
            Err(ApiError::Forbidden(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn genres(&self) -> Result<Vec<Genre>, ApiError> {
        let r: GenresResponse = self.get("api/v1/db/genres")?;
        Ok(r.genres)
    }

    /// Tracks in a genre.
    ///
    /// Deliberately the flat song list rather than albums-in-a-genre: the
    /// `/api/v1/db/genre/albums` route lives in mStream's velvet-stubs module
    /// and is only mounted when the server runs `ui: velvet`, so a general
    /// client cannot depend on it. Same story for the decade endpoints.
    pub fn genre_songs(&self, genre: &str) -> Result<Vec<Track>, ApiError> {
        self.post("api/v1/db/genre-songs", serde_json::json!({ "genre": genre }))
    }

    pub fn recently_added(&self, limit: u32) -> Result<Vec<Track>, ApiError> {
        self.post("api/v1/db/recent/added", serde_json::json!({ "limit": limit }))
    }

    /// Metadata for one track — used to fill the engine's duration hint
    /// without making it probe the remote stream.
    pub fn metadata(&self, filepath: &str) -> Result<Track, ApiError> {
        self.post("api/v1/db/metadata", serde_json::json!({ "filepath": filepath }))
    }

    pub fn search(&self, query: &str) -> Result<SearchResults, ApiError> {
        self.post("api/v1/db/search", serde_json::json!({ "search": query }))
    }

    pub fn playlists(&self) -> Result<Vec<PlaylistSummary>, ApiError> {
        self.get("api/v1/playlist/getall")
    }

    pub fn playlist_load(&self, name: &str) -> Result<Vec<Track>, ApiError> {
        self.post(
            "api/v1/playlist/load",
            serde_json::json!({ "playlistname": name }),
        )
    }

    // ── Stream URLs ─────────────────────────────────────────────────────────

    /// Direct (untranscoded) stream URL for a track's vpath.
    pub fn media_url(&self, filepath: &str) -> Result<String, ApiError> {
        urls::media_url(&self.server(), filepath, self.token.as_deref()).map_err(ApiError::Config)
    }

    /// Transcoded stream URL. The codec is always explicit — see
    /// [`urls::TranscodeCodec`].
    pub fn transcode_url(
        &self,
        filepath: &str,
        codec: TranscodeCodec,
        bitrate: Option<&str>,
    ) -> Result<String, ApiError> {
        urls::transcode_url(&self.server(), filepath, codec, bitrate, self.token.as_deref())
            .map_err(ApiError::Config)
    }
}

/// Pull mStream's `{"error": "..."}` out of a failure body, falling back to a
/// trimmed excerpt of whatever was actually returned.
fn extract_error(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = v.get("error").and_then(|e| e.as_str()) {
            return msg.to_string();
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "(empty response)".to_string();
    }
    trimmed.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_that_exist_to_route_round_the_config_do_not_need_it() {
        let scratch = crate::config::testing::Scratch::new("resolve-explicit");
        std::fs::write(scratch.dir.join("config.toml"), "this is not toml [[[").unwrap();

        // Both answers given: nothing about this command needs the file, so
        // its state should not decide whether the command runs.
        let Ok(client) = Client::resolve(Some("http://host:3000"), Some("t")) else {
            panic!("--server and --token should not consult the config");
        };
        assert_eq!(client.server(), "http://host:3000");
        assert_eq!(client.token(), Some("t"));

        // A missing answer still reads it, and still reports it plainly.
        let Err(err) = Client::resolve(None, None) else {
            panic!("a config this broken cannot name a server");
        };
        assert!(err.to_string().contains("is not valid"), "got: {err}");
    }

    #[test]
    fn normalizes_base_url_for_subpaths() {
        let c = Client::new("http://host:3000").unwrap();
        assert_eq!(c.endpoint("api/v1/ping").unwrap().as_str(), "http://host:3000/api/v1/ping");

        // A reverse-proxied server under a subpath must keep the prefix.
        let c = Client::new("http://host/mstream").unwrap();
        assert_eq!(c.endpoint("api/v1/ping").unwrap().as_str(), "http://host/mstream/api/v1/ping");

        let c = Client::new("http://host/mstream/").unwrap();
        assert_eq!(c.endpoint("api/v1/ping").unwrap().as_str(), "http://host/mstream/api/v1/ping");
    }

    #[test]
    fn server_strips_trailing_slash_for_stream_urls() {
        let c = Client::new("http://host:3000").unwrap();
        assert_eq!(c.server(), "http://host:3000");
        assert_eq!(
            c.with_token(Some("t".into())).media_url("lib/a.mp3").unwrap(),
            "http://host:3000/media/lib/a.mp3?token=t"
        );
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(Client::new("ftp://host").is_err());
        assert!(Client::new("not a url").is_err());
    }

    #[test]
    fn flags_plaintext_remote_servers_only() {
        assert!(Client::new("http://music.example.com").unwrap().is_insecure_remote());
        assert!(!Client::new("https://music.example.com").unwrap().is_insecure_remote());
        assert!(!Client::new("http://localhost:3000").unwrap().is_insecure_remote());
        assert!(!Client::new("http://127.0.0.1:3000").unwrap().is_insecure_remote());
        // A LAN server over http is the normal mStream deployment, not a
        // finding — see server_url::crosses_the_internet_unencrypted.
        assert!(!Client::new("http://192.168.1.71:3999").unwrap().is_insecure_remote());
    }

    #[test]
    fn extracts_server_error_messages() {
        assert_eq!(extract_error(r#"{"error":"Playlist not found"}"#), "Playlist not found");
        assert_eq!(extract_error("boom"), "boom");
        assert_eq!(extract_error("   "), "(empty response)");
    }
}
