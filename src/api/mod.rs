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
#[cfg(not(target_arch = "wasm32"))]
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;

#[cfg(not(target_arch = "wasm32"))]
use crate::runtime;
use types::*;
use urls::TranscodeCodec;

#[cfg(not(target_arch = "wasm32"))]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// The ceiling for the one endpoint that can make the server work before it
/// can answer.
///
/// [`Client::waveform_async`] documents a first call as taking as long as
/// ffmpeg takes to decode the track — up to 30 seconds. Under the shared
/// [`REQUEST_TIMEOUT`] that is not a slow success, it is a guaranteed
/// failure: the exact case the endpoint exists to serve could never come
/// back, and the caller cached the timeout as "this track has no shape".
#[cfg(not(target_arch = "wasm32"))]
const DECODE_TIMEOUT: Duration = Duration::from_secs(45);

/// The ceiling for a call that waits on a torrent daemon's round trip; the
/// browser build has no per-request timeout to set.
#[cfg(not(target_arch = "wasm32"))]
fn daemon_ceiling() -> Option<std::time::Duration> {
    Some(DECODE_TIMEOUT)
}
#[cfg(target_arch = "wasm32")]
fn daemon_ceiling() -> Option<std::time::Duration> {
    None
}
/// A discovery snapshot download: the server answers once the transfer is
/// verified, and a cross-network pull can take minutes (its own ceiling
/// is ten).
#[cfg(not(target_arch = "wasm32"))]
const FETCH_TIMEOUT: Duration = Duration::from_secs(600);

/// The torrent routes' ceiling: a seed check hashes the torrent's files
/// on the server's disk and auto-detect may reach for tags, both slower
/// than any listing. The record waits 45 seconds too.
/// Spelled out rather than gated: the browser build passes it through
/// [`Client::post_multipart`], which drops it there (the fetch backend
/// owns its own ceiling).
const DETECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

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

/// The discovery catalog's one-argument peer actions — each is
/// `POST api/v1/admin/discovery/p2p/<route> {endpointId}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAction {
    /// Drop the held snapshot (the catalog row stays).
    RemoveSnapshot,
    /// Drop an offline server from the catalog now; it returns on its
    /// next announcement. Refused (409) while its snapshot is held.
    Forget,
    /// Blocklist + snapshot + catalog row, all in one server-side action.
    Block,
    Unblock,
}

impl PeerAction {
    fn path(self) -> &'static str {
        match self {
            PeerAction::RemoveSnapshot => "api/v1/admin/discovery/p2p/peer-dbs/remove",
            PeerAction::Forget => "api/v1/admin/discovery/p2p/forget",
            PeerAction::Block => "api/v1/admin/discovery/p2p/block",
            PeerAction::Unblock => "api/v1/admin/discovery/p2p/unblock",
        }
    }
}

/// A federation key's expiry, as the limits route takes it: leave it as it
/// is, clear it, or set a new future cutoff (which is also how an expired
/// key is renewed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpiryChange {
    Keep,
    Never,
    /// ISO 8601, in the future.
    At(String),
}

/// A backup destination to add: `POST api/v1/admin/backup/destinations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBackupDestination {
    pub library_id: i64,
    pub dest_path: String,
    /// `after-scan` | `daily` | `manual`.
    pub trigger_type: String,
    /// Required with `daily`.
    pub daily_at_hour: Option<u32>,
    pub retention_days: u32,
    pub inter_file_delay_ms: u32,
    /// None = the server's defaults, and whatever they become later.
    pub exclude_globs: Option<Vec<String>>,
}

/// The credentials a torrent probe sends: the fields a client lacks
/// (qBittorrent has no RPC path, Deluge no username) stay `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TorrentCreds {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: String,
    pub rpc_path: Option<String>,
    pub use_https: bool,
}

impl TorrentCreds {
    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({
            "host": self.host,
            "port": self.port,
            "password": self.password,
            "useHttps": self.use_https,
        });
        if let Some(u) = &self.username {
            body["username"] = serde_json::json!(u);
        }
        if let Some(p) = &self.rpc_path {
            body["rpcPath"] = serde_json::json!(p);
        }
        body
    }
}

/// The fields a `PATCH` may carry; None leaves a field alone.
/// `exclude_globs`: None = untouched, Some(None) = back to the server's
/// defaults, Some(Some(list)) = pinned to that list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupPatch {
    pub dest_path: Option<String>,
    pub trigger_type: Option<String>,
    pub daily_at_hour: Option<Option<u32>>,
    pub retention_days: Option<u32>,
    pub inter_file_delay_ms: Option<u32>,
    pub enabled: Option<bool>,
    pub exclude_globs: Option<Option<Vec<String>>>,
}

impl BackupPatch {
    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        if let Some(v) = &self.dest_path {
            body.insert("destPath".into(), serde_json::json!(v));
        }
        if let Some(v) = &self.trigger_type {
            body.insert("triggerType".into(), serde_json::json!(v));
        }
        if let Some(v) = &self.daily_at_hour {
            body.insert("dailyAtHour".into(), serde_json::json!(v));
        }
        if let Some(v) = self.retention_days {
            body.insert("retentionDays".into(), serde_json::json!(v));
        }
        if let Some(v) = self.inter_file_delay_ms {
            body.insert("interFileDelayMs".into(), serde_json::json!(v));
        }
        if let Some(v) = self.enabled {
            body.insert("enabled".into(), serde_json::json!(v));
        }
        if let Some(v) = &self.exclude_globs {
            body.insert("excludeGlobs".into(), serde_json::json!(v));
        }
        serde_json::Value::Object(body)
    }
}

/// The discovery network's numeric settings, each its own route and body
/// key. Every one applies from the server's next check, no restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySetting {
    /// Disk the downloaded peer snapshots may use, total (10–100000 MB).
    MaxStorageMb,
    /// Days of silence before an offline server leaves the list (0 = never).
    PeerRetentionDays,
    /// How many servers' snapshots to keep downloaded automatically (0–50).
    AutoFetchCount,
    /// Days before a downloaded snapshot may be swapped out (0 = never).
    RotationDays,
    /// The sidecar memory watchdog's ceiling in MB (0 = off).
    SidecarMaxRssMb,
}

impl DiscoverySetting {
    /// The route and the body key it reads.
    fn route(self) -> (&'static str, &'static str) {
        match self {
            DiscoverySetting::MaxStorageMb => {
                ("api/v1/admin/discovery/p2p/max-storage", "maxPeerDbStorageMb")
            }
            DiscoverySetting::PeerRetentionDays => {
                ("api/v1/admin/discovery/p2p/peer-retention", "peerRetentionDays")
            }
            DiscoverySetting::AutoFetchCount => {
                ("api/v1/admin/discovery/p2p/auto-fetch-count", "autoFetchCount")
            }
            DiscoverySetting::RotationDays => ("api/v1/admin/discovery/p2p/rotation", "rotationDays"),
            DiscoverySetting::SidecarMaxRssMb => {
                ("api/v1/admin/discovery/p2p/sidecar-max-rss", "sidecarMaxRssMb")
            }
        }
    }

    /// The server's own bounds for the value (mirrors its Joi schema).
    pub fn bounds(self) -> (u64, u64) {
        match self {
            DiscoverySetting::MaxStorageMb => (10, 100_000),
            DiscoverySetting::PeerRetentionDays | DiscoverySetting::RotationDays => (0, 3650),
            DiscoverySetting::AutoFetchCount => (0, 50),
            DiscoverySetting::SidecarMaxRssMb => (0, 100_000),
        }
    }
}

/// Run the async core to completion for the sync (native) surface.
///
/// The client is async at its core — one implementation serving both the
/// browser build (awaited on the JS event loop) and this wrapper, which
/// parks the caller's worker thread on the shared runtime. Kept `pub(crate)`
/// so worker code can drive the shared async helpers the same way.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn wait<T>(fut: impl Future<Output = Result<T, ApiError>>) -> Result<T, ApiError> {
    runtime::block_on(fut).map_err(ApiError::Network)?
}

pub struct Client {
    http: reqwest::Client,
    /// Always ends in `/` so `Url::join` appends instead of replacing the last
    /// segment — required for servers hosted under a reverse-proxy subpath.
    base: Url,
    token: Option<String>,
    /// Aimed at a federated peer: every API path is rewritten onto the
    /// parent's browse proxy, art onto its art proxy (contract clause 27).
    /// `base` and `token` are then the parent's.
    peer: Option<i64>,
    /// Aimed at a tunnel's loopback bridge: the token every request must
    /// carry as `__lt=…`, or the shared tunnel client drops the connection.
    local_token: Option<String>,
    /// Set once this server has shown it can't answer a listing that asks for
    /// metadata, so the fallback costs one wasted request per session rather
    /// than one per folder.
    plain_listings: AtomicBool,
    /// Set once this server has said it has no ffmpeg. Waveforms are the one
    /// optional feature the ping does *not* advertise, so the house rule —
    /// no flag, no probe — has nothing to read. This is the next best thing:
    /// probe once, believe the answer, and stop asking for the session.
    no_waveforms: AtomicBool,
}

impl Client {
    pub fn new(server: &str) -> Result<Self, ApiError> {
        Self::new_with(server, false)
    }

    /// [`Client::new`], with the per-server trust knob: `self_signed` skips
    /// TLS verification, for a server presenting its own certificate. Only
    /// callers holding that server's saved entry pass true — the flag lives
    /// on [`crate::config::ServerEntry`], never process-wide.
    pub fn new_with(server: &str, self_signed: bool) -> Result<Self, ApiError> {
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

        #[cfg(not(target_arch = "wasm32"))]
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .danger_accept_invalid_certs(self_signed)
            .build()
            .map_err(|e| ApiError::Config(format!("could not build http client: {e}")))?;
        // The fetch backend has no connect timeout to set; the browser owns
        // the socket and applies its own — TLS trust included, so the flag
        // cannot mean anything there.
        #[cfg(target_arch = "wasm32")]
        let http = {
            let _ = self_signed;
            reqwest::Client::new()
        };

        Ok(Client {
            http,
            base,
            token: None,
            peer: None,
            local_token: None,
            plain_listings: AtomicBool::new(false),
            no_waveforms: AtomicBool::new(false),
        })
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    /// Aim this client at the parent's federated peer `id`: reads go
    /// through `/api/v1/federation/peers/{id}/api/…`, art through
    /// `…/art/…`. `None` is the parent itself.
    pub fn with_peer(mut self, peer: Option<i64>) -> Self {
        self.peer = peer;
        self
    }

    pub fn peer(&self) -> Option<i64> {
        self.peer
    }

    /// Every request to a tunnel bridge carries its loopback token as
    /// `__lt=…`; `None` for a server reached directly.
    pub fn with_local_token(mut self, token: Option<String>) -> Self {
        self.local_token = token;
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
    #[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    fn remembered_server() -> Result<String, ApiError> {
        let config = crate::config::load().map_err(ApiError::Config)?;
        match crate::config::preferred_server(&config) {
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
        let path = match self.peer {
            // The proxy takes the peer's own API path whole after `/api/`,
            // so `api/v1/db/albums` becomes `…/peers/3/api/api/v1/db/albums`.
            Some(peer) => match path.strip_prefix("album-art/") {
                Some(file) => format!("api/v1/federation/peers/{peer}/art/{file}"),
                None => format!("api/v1/federation/peers/{peer}/api/{path}"),
            },
            None => path.to_string(),
        };
        let mut url = self
            .base
            .join(&path)
            .map_err(|e| ApiError::Config(format!("could not build URL for {path}: {e}")))?;
        if let Some(token) = &self.local_token {
            url.query_pairs_mut().append_pair("__lt", token);
        }
        Ok(url)
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, ApiError> {
        self.send_within(method, path, body, None).await
    }

    /// `send`, with a ceiling of this request's own instead of the client's.
    ///
    /// Only [`DECODE_TIMEOUT`] uses it. The browser build ignores the
    /// argument because the fetch backend has no per-request timeout to
    /// set — the browser owns that, as it does the connect timeout.
    async fn send_within<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        longest: Option<std::time::Duration>,
    ) -> Result<T, ApiError> {
        let url = self.endpoint(path)?;
        #[allow(unused_mut)]
        let mut req = self.http.request(method, url);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(longest) = longest {
            req = req.timeout(longest);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = longest;
        if let Some(token) = &self.token {
            req = req.header("x-access-token", token);
        }
        if let Some(body) = &body {
            req = req.header("Content-Type", "application/json").body(
                serde_json::to_string(body)
                    .map_err(|e| ApiError::Config(format!("could not encode request: {e}")))?,
            );
        }
        self.finish(req, path).await
    }

    /// Send a built request and map the answer: only 401 is a session
    /// problem, the other failures carry the server's words as
    /// [`extract_error`] reads them out of the body.
    async fn finish<T: DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        path: &str,
    ) -> Result<T, ApiError> {
        let resp = req.send().await.map_err(|e| ApiError::Network(e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| ApiError::Network(e.to_string()))?;

        match status {
            StatusCode::UNAUTHORIZED => return Err(ApiError::Unauthorized),
            StatusCode::FORBIDDEN => return Err(ApiError::Forbidden(extract_error(&text))),
            StatusCode::NOT_FOUND => return Err(ApiError::NotFound(path.to_string())),
            s if !s.is_success() => {
                return Err(ApiError::Server { status: s.as_u16(), message: extract_error(&text) });
            }
            _ => {}
        }

        serde_json::from_str(&text).map_err(|e| ApiError::Decode {
            endpoint: path.to_string(),
            message: e.to_string(),
        })
    }

    /// A multipart POST — the torrent routes' shape, the user's and the
    /// admin's. `longest` is the request's own ceiling: a seed check
    /// hashes files on the server.
    async fn post_multipart<T: DeserializeOwned>(
        &self,
        path: &str,
        form: Multipart,
        longest: Option<std::time::Duration>,
    ) -> Result<T, ApiError> {
        let url = self.endpoint(path)?;
        let (content_type, body) = form.finish();
        #[allow(unused_mut)]
        let mut req = self.http.request(Method::POST, url);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(longest) = longest {
            req = req.timeout(longest);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = longest;
        if let Some(token) = &self.token {
            req = req.header("x-access-token", token);
        }
        req = req.header("Content-Type", content_type).body(body);
        self.finish(req, path).await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.send(Method::GET, path, None).await
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, ApiError> {
        self.send(Method::POST, path, Some(body)).await
    }

    // ── Endpoints ───────────────────────────────────────────────────────────
    //
    // Each endpoint is written once, async — that version is the browser
    // client. The `#[cfg]`-gated sync twin with the same name minus `_async`
    // is the native surface every existing caller keeps using; it parks the
    // worker thread via [`wait`] and nothing else.

    /// Authenticate. The async core doesn't store the token — a shared `&self`
    /// can't — so callers decide: the sync wrapper stores it on this client,
    /// the web worker rebuilds its client with [`Client::with_token`].
    pub async fn login_async(
        &self,
        username: &str,
        password: &str,
    ) -> Result<LoginResponse, ApiError> {
        self.post(
            "api/v1/auth/login",
            serde_json::json!({ "username": username, "password": password }),
        )
        .await
    }

    /// Authenticate WITHOUT storing the token — for callers holding the
    /// client behind an Arc (the setup wizard's worker), which rebuild a
    /// token-carrying client from the response instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn login_shared(
        &self,
        username: &str,
        password: &str,
    ) -> Result<LoginResponse, ApiError> {
        wait(self.login_async(username, password))
    }

    /// Authenticate and store the returned token on this client.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn login(&mut self, username: &str, password: &str) -> Result<LoginResponse, ApiError> {
        let resp = wait(self.login_async(username, password))?;
        self.token = Some(resp.token.clone());
        Ok(resp)
    }

    /// Capability bootstrap — "called once after login".
    pub async fn ping_async(&self) -> Result<Ping, ApiError> {
        self.get("api/v1/ping").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn ping(&self) -> Result<Ping, ApiError> {
        wait(self.ping_async())
    }

    /// The capability bootstrap for a federated peer: `/api/v1/ping` is
    /// off the federation allowlist, but the layered `GET /api` is on it
    /// and carries the same keys — the libraries the parent's key may
    /// read, and the flags a peer never gets to keep (contract clause 26).
    pub async fn ping_via_info_async(&self) -> Result<Ping, ApiError> {
        self.get("api/").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn ping_via_info(&self) -> Result<Ping, ApiError> {
        wait(self.ping_via_info_async())
    }

    /// The peers this user may browse through the server (contract clause
    /// 20): `GET /api/v1/federation/peers`, answered only while federation
    /// is on there.
    pub async fn federation_peers_async(&self) -> Result<Vec<PeerListing>, ApiError> {
        self.get::<PeerListingResponse>("api/v1/federation/peers").await.map(|r| r.peers)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn federation_peers(&self) -> Result<Vec<PeerListing>, ApiError> {
        wait(self.federation_peers_async())
    }

    /// `GET /api/v1/federation/peers/{id}/access` — direct access to one
    /// of this server's peers for the caller's own device (mStream #943).
    /// Always on the parent's plain client, never through a peer client's
    /// rewrite. `refresh` asks the parent to re-mint a token the peer just
    /// refused.
    pub async fn federation_access_async(
        &self,
        id: i64,
        refresh: bool,
    ) -> Result<crate::api::types::DirectAccessResponse, ApiError> {
        let path = if refresh {
            format!("api/v1/federation/peers/{id}/access?refresh=1")
        } else {
            format!("api/v1/federation/peers/{id}/access")
        };
        self.get(&path).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn federation_access(
        &self,
        id: i64,
        refresh: bool,
    ) -> Result<crate::api::types::DirectAccessResponse, ApiError> {
        wait(self.federation_access_async(id, refresh))
    }

    /// `GET /api/` — the server's version and API generations. The one
    /// endpoint that answers without auth, which is what lets the Manage
    /// Servers screen show a version for servers it holds no token for.
    pub async fn server_info_async(&self) -> Result<ServerInfo, ApiError> {
        self.get("api/").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn server_info(&self) -> Result<ServerInfo, ApiError> {
        wait(self.server_info_async())
    }

    /// Browse a directory.
    ///
    /// An empty string lists the libraries (vpaths); [`BEST_START`] asks the
    /// server to pick the most useful place instead.
    /// `pullMetadata` costs the server one batched query for the whole folder
    /// and buys durations, BPM and key for every row — without it a track
    /// queued from the file browser is a filename with no length, and Auto-DJ
    /// seeded from one has no tempo to match.
    pub async fn file_explorer_async(&self, directory: &str) -> Result<DirListing, ApiError> {
        if !self.plain_listings.load(Ordering::Relaxed) {
            let body =
                serde_json::json!({ "directory": directory, "pullMetadata": true });
            match self.post("api/v1/file-explorer", body).await {
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
        self.post("api/v1/file-explorer", serde_json::json!({ "directory": directory })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn file_explorer(&self, directory: &str) -> Result<DirListing, ApiError> {
        wait(self.file_explorer_async(directory))
    }

    pub async fn artists_async(&self) -> Result<Vec<String>, ApiError> {
        let r: ArtistsResponse = self.get("api/v1/db/artists").await?;
        Ok(r.artists)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn artists(&self) -> Result<Vec<String>, ApiError> {
        wait(self.artists_async())
    }

    pub async fn albums_async(&self) -> Result<Vec<Album>, ApiError> {
        let r: AlbumsResponse = self.get("api/v1/db/albums").await?;
        Ok(r.albums)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn albums(&self) -> Result<Vec<Album>, ApiError> {
        wait(self.albums_async())
    }

    pub async fn artist_albums_async(&self, artist: &str) -> Result<Vec<Album>, ApiError> {
        let r: AlbumsResponse = self
            .post("api/v1/db/artists-albums", serde_json::json!({ "artist": artist }))
            .await?;
        Ok(r.albums)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn artist_albums(&self, artist: &str) -> Result<Vec<Album>, ApiError> {
        wait(self.artist_albums_async(artist))
    }

    pub async fn album_songs_async(
        &self,
        album: &str,
        artist: Option<&str>,
    ) -> Result<Vec<Track>, ApiError> {
        let mut body = serde_json::json!({ "album": album });
        if let Some(artist) = artist {
            body["artist"] = serde_json::Value::String(artist.to_string());
        }
        self.post("api/v1/db/album-songs", body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn album_songs(&self, album: &str, artist: Option<&str>) -> Result<Vec<Track>, ApiError> {
        wait(self.album_songs_async(album, artist))
    }

    /// Ask the Auto-DJ picker for one track matching the given constraints.
    ///
    /// The server answers 400 when nothing survives its fallback waterfall;
    /// that's an ordinary "no pick", so it comes back as an empty `songs`
    /// list rather than an error.
    pub async fn random_song_async(
        &self,
        request: &RandomSongRequest,
    ) -> Result<RandomSongsResponse, ApiError> {
        let body = serde_json::to_value(request)
            .map_err(|e| ApiError::Config(format!("could not encode request: {e}")))?;
        match self.post("api/v1/db/random-songs", body).await {
            Ok(response) => Ok(response),
            Err(ApiError::Server { status: 400, .. }) => Ok(RandomSongsResponse::default()),
            Err(e) => Err(e),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn random_song(
        &self,
        request: &RandomSongRequest,
    ) -> Result<RandomSongsResponse, ApiError> {
        wait(self.random_song_async(request))
    }

    /// Tracks that sound like `filepath`, nearest first.
    ///
    /// Returns `None` when the server has discovery collection switched off —
    /// it answers 403 by house convention for a disabled feature, which is a
    /// configuration state rather than a failure the user can act on.
    pub async fn similar_tracks_async(
        &self,
        filepath: &str,
        limit: u32,
    ) -> Result<Option<SimilarTracksResponse>, ApiError> {
        let body = serde_json::json!({ "filePath": filepath, "limit": limit });
        match self.post("api/v1/discovery/local/similar/tracks", body).await {
            Ok(response) => Ok(Some(response)),
            Err(ApiError::Forbidden(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn similar_tracks(
        &self,
        filepath: &str,
        limit: u32,
    ) -> Result<Option<SimilarTracksResponse>, ApiError> {
        wait(self.similar_tracks_async(filepath, limit))
    }

    /// Artists whose overall sound sits near this one's, each with up to two
    /// tracks that lead in from where the listener already is.
    ///
    /// `None` means discovery is switched off, as with
    /// [`Client::similar_tracks`].
    pub async fn similar_artists_async(
        &self,
        artist: &str,
        limit: u32,
    ) -> Result<Option<SimilarArtistsResponse>, ApiError> {
        let body = serde_json::json!({ "artist": artist, "limit": limit });
        match self.post("api/v1/discovery/local/similar/artists", body).await {
            Ok(response) => Ok(Some(response)),
            Err(ApiError::Forbidden(_)) => Ok(None),
            // The server treats an artist with nothing visible to this user
            // as one that doesn't exist; an empty list says that better than
            // an error does.
            Err(ApiError::NotFound(_)) => Ok(Some(SimilarArtistsResponse::default())),
            Err(e) => Err(e),
        }
    }

    // No sync twin: its only callers (worker::discover) drive the async
    // version through `wait` themselves. Same for `journey_async` below.

    /// A journey from one track to another through the embedding space.
    ///
    /// `length` counts the total rows including both seeds, so the answer is
    /// the queue. Like [`Client::similar_tracks`], `None` means the server has
    /// discovery switched off rather than that anything went wrong.
    pub async fn journey_async(
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
        match self.post("api/v1/discovery/local/path", body).await {
            Ok(response) => Ok(Some(response)),
            Err(ApiError::Forbidden(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn genres_async(&self) -> Result<Vec<Genre>, ApiError> {
        let r: GenresResponse = self.get("api/v1/db/genres").await?;
        Ok(r.genres)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn genres(&self) -> Result<Vec<Genre>, ApiError> {
        wait(self.genres_async())
    }

    /// Tracks in a genre.
    ///
    /// Deliberately the flat song list rather than albums-in-a-genre: the
    /// `/api/v1/db/genre/albums` route lives in mStream's velvet-stubs module
    /// and is only mounted when the server runs `ui: velvet`, so a general
    /// client cannot depend on it. Same story for the decade endpoints.
    pub async fn genre_songs_async(&self, genre: &str) -> Result<Vec<Track>, ApiError> {
        self.post("api/v1/db/genre-songs", serde_json::json!({ "genre": genre })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn genre_songs(&self, genre: &str) -> Result<Vec<Track>, ApiError> {
        wait(self.genre_songs_async(genre))
    }

    pub async fn recently_added_async(&self, limit: u32) -> Result<Vec<Track>, ApiError> {
        self.post("api/v1/db/recent/added", serde_json::json!({ "limit": limit })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn recently_added(&self, limit: u32) -> Result<Vec<Track>, ApiError> {
        wait(self.recently_added_async(limit))
    }

    /// Metadata for one track — used to fill the engine's duration hint
    /// without making it probe the remote stream.
    pub async fn metadata_async(&self, filepath: &str) -> Result<Track, ApiError> {
        self.post("api/v1/db/metadata", serde_json::json!({ "filepath": filepath })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn metadata(&self, filepath: &str) -> Result<Track, ApiError> {
        wait(self.metadata_async(filepath))
    }

    pub async fn search_async(&self, query: &str) -> Result<SearchResults, ApiError> {
        self.post("api/v1/db/search", serde_json::json!({ "search": query })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn search(&self, query: &str) -> Result<SearchResults, ApiError> {
        wait(self.search_async(query))
    }

    pub async fn playlists_async(&self) -> Result<Vec<PlaylistSummary>, ApiError> {
        self.get("api/v1/playlist/getall").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn playlists(&self) -> Result<Vec<PlaylistSummary>, ApiError> {
        wait(self.playlists_async())
    }

    pub async fn playlist_load_async(&self, name: &str) -> Result<Vec<Track>, ApiError> {
        self.post(
            "api/v1/playlist/load",
            serde_json::json!({ "playlistname": name }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn playlist_load(&self, name: &str) -> Result<Vec<Track>, ApiError> {
        wait(self.playlist_load_async(name))
    }

    /// Write `files` to the playlist called `title`, creating it or replacing
    /// its contents outright — the server's save is an overwrite, not an
    /// append, and it says so by taking the whole list every time.
    ///
    /// The reply is `{}`, so there is nothing to read back; the unit type is
    /// what says the call is only worth its success.
    pub async fn playlist_save_async(
        &self,
        title: &str,
        files: &[String],
    ) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .post(
                "api/v1/playlist/save",
                serde_json::json!({ "title": title, "songs": files }),
            )
            .await?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn playlist_save(&self, title: &str, files: &[String]) -> Result<(), ApiError> {
        wait(self.playlist_save_async(title, files))
    }

    /// Create an EMPTY playlist. The server answers 400 when the name is
    /// already taken; the error carries its words.
    pub async fn playlist_new_async(&self, title: &str) -> Result<(), ApiError> {
        let _: serde_json::Value =
            self.post("api/v1/playlist/new", serde_json::json!({ "title": title })).await?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn playlist_new(&self, title: &str) -> Result<(), ApiError> {
        wait(self.playlist_new_async(title))
    }

    /// Rename a playlist. The route arrived in mStream 5.16.0 — an older
    /// server 404s, which callers word as the missing feature it is.
    pub async fn playlist_rename_async(&self, from: &str, to: &str) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .post(
                "api/v1/playlist/rename",
                serde_json::json!({ "oldName": from, "newName": to }),
            )
            .await?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn playlist_rename(&self, from: &str, to: &str) -> Result<(), ApiError> {
        wait(self.playlist_rename_async(from, to))
    }

    pub async fn playlist_delete_async(&self, name: &str) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .post("api/v1/playlist/delete", serde_json::json!({ "playlistname": name }))
            .await?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn playlist_delete(&self, name: &str) -> Result<(), ApiError> {
        wait(self.playlist_delete_async(name))
    }

    // ── Torrents (docs/ux-contracts/add-torrent.md) ─────────────────────────

    /// Whether this server takes a torrent from this user, and why not
    /// when it doesn't — the Add-torrent room's gate (no ping flag exists).
    /// Asked with an empty path: the global gates only; the per-library
    /// mapping is `/torrent/add`'s own check.
    pub async fn torrent_preflight_async(&self) -> Result<TorrentPreflight, ApiError> {
        self.get("api/v1/torrent/preflight?path=").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn torrent_preflight(&self) -> Result<TorrentPreflight, ApiError> {
        wait(self.torrent_preflight_async())
    }

    /// Per-library destination templates. Best-effort for callers: an
    /// older server without the route answers 404, and the legacy
    /// `Artist/Album` layout still applies.
    pub async fn torrent_path_templates_async(&self) -> Result<TorrentTemplates, ApiError> {
        self.get("api/v1/torrent/path-templates").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn torrent_path_templates(&self) -> Result<TorrentTemplates, ApiError> {
        wait(self.torrent_path_templates_async())
    }

    /// Ask the server to read artist/album/year out of a `.torrent`.
    /// `vpath` lets the server use that library's tag knowledge.
    pub async fn torrent_auto_detect_async(
        &self,
        bytes: &[u8],
        filename: &str,
        vpath: Option<&str>,
    ) -> Result<TorrentDetect, ApiError> {
        let mut form = Multipart::new();
        if let Some(vpath) = vpath.filter(|v| !v.is_empty()) {
            form.field("vpath", vpath);
        }
        form.file("torrentFile", filename, bytes);
        self.post_multipart("api/v1/torrent/auto-detect", form, Some(DETECT_TIMEOUT)).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn torrent_auto_detect(
        &self,
        bytes: &[u8],
        filename: &str,
        vpath: Option<&str>,
    ) -> Result<TorrentDetect, ApiError> {
        wait(self.torrent_auto_detect_async(bytes, filename, vpath))
    }

    /// Are the torrent's files already on disk somewhere the user can
    /// see? Scans every library the user has (the server intersects with
    /// their access), and seeds outright when everything is there.
    pub async fn torrent_seed_existing_async(
        &self,
        bytes: &[u8],
        filename: &str,
    ) -> Result<SeedCheck, ApiError> {
        let mut form = Multipart::new();
        form.file("torrentFile", filename, bytes);
        self.post_multipart("api/v1/torrent/seed-existing", form, Some(DETECT_TIMEOUT)).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn torrent_seed_existing(&self, bytes: &[u8], filename: &str) -> Result<SeedCheck, ApiError> {
        wait(self.torrent_seed_existing_async(bytes, filename))
    }

    /// Hand the torrent to the server's client, to land at
    /// `<vpath>/<sub_path>/<directory_name>`.
    pub async fn torrent_add_async(&self, req: &TorrentAddRequest) -> Result<TorrentAdded, ApiError> {
        let mut form = Multipart::new();
        form.field("vpath", &req.vpath);
        if !req.sub_path.is_empty() {
            form.field("subPath", &req.sub_path);
        }
        form.field("directoryName", &req.directory_name);
        form.field("renameRoot", if req.rename_root { "true" } else { "false" });
        match &req.source {
            TorrentSource::Magnet(magnet) => {
                form.field("magnet", magnet);
            }
            TorrentSource::File { name, bytes } => {
                form.file("torrentFile", name, bytes);
            }
        }
        self.post_multipart("api/v1/torrent/add", form, Some(DETECT_TIMEOUT)).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn torrent_add(&self, req: &TorrentAddRequest) -> Result<TorrentAdded, ApiError> {
        wait(self.torrent_add_async(req))
    }

    /// The shape of a track, for drawing under the progress bar.
    ///
    /// `None` is every flavour of "there isn't one", and none of them is an
    /// error the user can do anything about:
    ///
    /// * **503** — the server has no ffmpeg. That is a property of the
    ///   server, not the track, so it latches: every later call short-
    ///   circuits and the session costs one wasted request rather than one
    ///   per track.
    /// * **500** — ffmpeg's own verdict on this content. The server writes a
    ///   failure marker and will answer the same way forever, so this is a
    ///   settled answer rather than something to retry.
    /// * **404** — not in the database, or a federated track: mStream's
    ///   federation stream puts waveforms out of scope deliberately, so a
    ///   peer's track simply has no shape to fetch.
    ///
    /// The first call for a track the server hasn't seen can take as long as
    /// ffmpeg takes to decode it — up to 30 seconds. Callers must treat this
    /// as an enhancement that may arrive late, or never.
    pub async fn waveform_async(&self, filepath: &str) -> Result<Option<Vec<u8>>, ApiError> {
        if self.no_waveforms.load(Ordering::Relaxed) {
            return Ok(None);
        }
        // A vpath contains slashes, spaces and anything else a filename can,
        // and this is the one endpoint here that takes one as a query
        // parameter rather than in a JSON body.
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("filepath", filepath)
            .finish();
        let path = format!("api/v1/db/waveform?{query}");
        #[cfg(not(target_arch = "wasm32"))]
        let sent =
            self.send_within::<WaveformResponse>(Method::GET, &path, None, Some(DECODE_TIMEOUT))
                .await;
        #[cfg(target_arch = "wasm32")]
        let sent = self.get::<WaveformResponse>(&path).await;
        match sent {
            // An empty array is "nothing to draw", not a flat line.
            Ok(response) => Ok((!response.waveform.is_empty()).then_some(response.waveform)),
            Err(ApiError::Server { status: 503, .. }) => {
                self.no_waveforms.store(true, Ordering::Relaxed);
                Ok(None)
            }
            Err(ApiError::Server { status: 500, .. })
            | Err(ApiError::NotFound(_))
            | Err(ApiError::Forbidden(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn waveform(&self, filepath: &str) -> Result<Option<Vec<u8>>, ApiError> {
        wait(self.waveform_async(filepath))
    }

    /// The cover image a track's `album-art` metadata names — raw bytes,
    /// whatever format the server holds it in.
    ///
    /// This is the one non-JSON GET in the client, so it does its own small
    /// version of [`Client::send`]: same header auth, same status mapping,
    /// but the body stays bytes instead of being read as text.
    pub async fn album_art_async(&self, file: &str) -> Result<Vec<u8>, ApiError> {
        let url = match self.peer {
            Some(peer) => urls::peer_art_url(&self.server(), peer, file),
            None => urls::album_art_url(&self.server(), file),
        }
        .map_err(ApiError::Config)?;
        let url = urls::with_local_token(url, self.local_token.as_deref());
        let mut req = self.http.get(&url);
        if let Some(token) = &self.token {
            req = req.header("x-access-token", token);
        }

        let resp = req.send().await.map_err(|e| ApiError::Network(e.to_string()))?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| ApiError::Network(e.to_string()))?;

        match status {
            StatusCode::UNAUTHORIZED => Err(ApiError::Unauthorized),
            StatusCode::NOT_FOUND => Err(ApiError::NotFound(format!("album-art/{file}"))),
            s if !s.is_success() => Err(ApiError::Server {
                status: s.as_u16(),
                message: extract_error(&String::from_utf8_lossy(&bytes)),
            }),
            _ => Ok(bytes.to_vec()),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn album_art(&self, file: &str) -> Result<Vec<u8>, ApiError> {
        wait(self.album_art_async(file))
    }

    // ── Stream URLs ─────────────────────────────────────────────────────────

    /// Direct (untranscoded) stream URL for a track's vpath.
    pub fn media_url(&self, filepath: &str) -> Result<String, ApiError> {
        urls::media_url(&self.server(), filepath, self.token.as_deref())
            .map(|url| urls::with_local_token(url, self.local_token.as_deref()))
            .map_err(ApiError::Config)
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
            .map(|url| urls::with_local_token(url, self.local_token.as_deref()))
            .map_err(ApiError::Config)
    }

    // ── Setup-wizard admin endpoints ────────────────────────────────────────
    //
    // All of these sit behind the admin guard. On a fresh install with zero
    // accounts every request is authenticated as an implicit admin, which is
    // exactly the window the setup wizard runs in; once it creates the first
    // user it logs in and keeps going with the token.

    /// The library folders the server already has, as vpath name → entry.
    /// BTreeMap so a reopened wizard lists them in a stable order.
    pub async fn admin_directories_async(
        &self,
    ) -> Result<std::collections::BTreeMap<String, crate::api::types::AdminDirEntry>, ApiError>
    {
        self.get("api/v1/admin/directories").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_directories(
        &self,
    ) -> Result<std::collections::BTreeMap<String, crate::api::types::AdminDirEntry>, ApiError>
    {
        wait(self.admin_directories_async())
    }

    /// Add a library folder. `vpath` is the short name apps see
    /// (`^[a-zA-Z0-9-]+$` server-side); the server queues a scan on its own.
    pub async fn admin_add_directory_async(
        &self,
        directory: &str,
        vpath: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.send(
            Method::PUT,
            "api/v1/admin/directory",
            Some(serde_json::json!({ "directory": directory, "vpath": vpath })),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_add_directory(
        &self,
        directory: &str,
        vpath: &str,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_add_directory_async(directory, vpath))
    }

    /// Remove a library folder by vpath. The files stay on disk; the server
    /// drops the library (and, on its next scan, its tracks) from the database.
    pub async fn admin_remove_directory_async(
        &self,
        vpath: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.send(
            Method::DELETE,
            "api/v1/admin/directory",
            Some(serde_json::json!({ "vpath": vpath })),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_remove_directory(&self, vpath: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_remove_directory_async(vpath))
    }

    /// A library's follow-symlinks flag. Takes effect on that library's
    /// next scan — the server says so, and so does the UI.
    pub async fn admin_set_follow_symlinks_async(
        &self,
        vpath: &str,
        follow: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(
            "api/v1/admin/directory/follow-symlinks",
            serde_json::json!({ "vpath": vpath, "followSymlinks": follow }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_set_follow_symlinks(
        &self,
        vpath: &str,
        follow: bool,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_set_follow_symlinks_async(vpath, follow))
    }

    /// The ADMIN file explorer: any directory on the server's own disk (the
    /// public one only walks the libraries). `~` is the server user's home,
    /// resolved server-side; the listing's `path` is the absolute form.
    pub async fn admin_file_explorer_async(&self, directory: &str) -> Result<DirListing, ApiError> {
        self.post("api/v1/admin/file-explorer", serde_json::json!({ "directory": directory }))
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_file_explorer(&self, directory: &str) -> Result<DirListing, ApiError> {
        wait(self.admin_file_explorer_async(directory))
    }

    /// Create a user. The wizard's first user is `admin: true` with every
    /// vpath — creating it is what closes the fresh install's open window.
    pub async fn admin_create_user_async(
        &self,
        username: &str,
        password: &str,
        vpaths: &[String],
        admin: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.send(
            Method::PUT,
            "api/v1/admin/users",
            Some(serde_json::json!({
                "username": username,
                "password": password,
                "vpaths": vpaths,
                "admin": admin,
            })),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_create_user(
        &self,
        username: &str,
        password: &str,
        vpaths: &[String],
        admin: bool,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_create_user_async(username, password, vpaths, admin))
    }

    /// Set the auto-update posture: `notify`, `stage` or `auto`.
    pub async fn admin_update_mode_async(&self, mode: &str) -> Result<serde_json::Value, ApiError> {
        self.post("api/v1/admin/update/settings", serde_json::json!({ "mode": mode })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_update_mode(&self, mode: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_update_mode_async(mode))
    }

    /// Toggle server-side audio (the jukebox this very binary provides).
    pub async fn admin_auto_boot_audio_async(
        &self,
        enabled: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(
            "api/v1/admin/config/auto-boot-server-audio",
            serde_json::json!({ "autoBootServerAudio": enabled }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_auto_boot_audio(&self, enabled: bool) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_auto_boot_audio_async(enabled))
    }

    /// Toggle the discovery network. Enabling may make the server download
    /// its p2p sidecar before answering, so give it the decode ceiling
    /// rather than the ordinary request timeout.
    pub async fn admin_discovery_enabled_async(
        &self,
        enabled: bool,
    ) -> Result<serde_json::Value, ApiError> {
        let body = serde_json::json!({ "enabled": enabled });
        #[cfg(not(target_arch = "wasm32"))]
        return self
            .send_within(
                Method::POST,
                "api/v1/admin/discovery/p2p/enabled",
                Some(body),
                Some(DECODE_TIMEOUT),
            )
            .await;
        #[cfg(target_arch = "wasm32")]
        return self.post("api/v1/admin/discovery/p2p/enabled", body).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_enabled(&self, enabled: bool) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_enabled_async(enabled))
    }

    /// Toggle federation — the invite system for sharing libraries with
    /// paired servers.
    pub async fn admin_federation_enabled_async(
        &self,
        enabled: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.post("api/v1/admin/federation", serde_json::json!({ "enabled": enabled })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_enabled(&self, enabled: bool) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_enabled_async(enabled))
    }

    /// Quick Connect state + pairing ticket (admin).
    pub async fn admin_iroh_async(&self) -> Result<IrohStatus, ApiError> {
        self.get("api/v1/admin/iroh").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_iroh(&self) -> Result<IrohStatus, ApiError> {
        wait(self.admin_iroh_async())
    }

    // ── The discovery network (P2P), route for route with the webapp's
    //    Discovery page. Everything here is admin-gated. ───────────────────

    pub async fn admin_discovery_status_async(&self) -> Result<DiscoveryStatus, ApiError> {
        self.get("api/v1/admin/discovery/p2p/status").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_status(&self) -> Result<DiscoveryStatus, ApiError> {
        wait(self.admin_discovery_status_async())
    }

    /// The catalog. `include_incompatible` lifts the server's hide-by-default
    /// filter on peers whose embedding model cannot serve this server.
    pub async fn admin_discovery_catalog_async(
        &self,
        include_incompatible: bool,
    ) -> Result<DiscoveryCatalog, ApiError> {
        let path = if include_incompatible {
            "api/v1/admin/discovery/p2p/catalog?includeIncompatible=1"
        } else {
            "api/v1/admin/discovery/p2p/catalog"
        };
        self.get(path).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_catalog(
        &self,
        include_incompatible: bool,
    ) -> Result<DiscoveryCatalog, ApiError> {
        wait(self.admin_discovery_catalog_async(include_incompatible))
    }

    /// The discovery log ring past `since` (0 = everything it holds).
    pub async fn admin_discovery_activity_async(
        &self,
        since: u64,
    ) -> Result<DiscoveryActivity, ApiError> {
        self.get(&format!("api/v1/admin/discovery/p2p/activity?since={since}")).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_activity(&self, since: u64) -> Result<DiscoveryActivity, ApiError> {
        wait(self.admin_discovery_activity_async(since))
    }

    /// Join the discovery network, optionally opening the federation
    /// request inbox in the same breath (which turns federation on). The
    /// server may download its sidecar before answering, so this gets the
    /// decode ceiling like [`Client::admin_discovery_enabled_async`].
    pub async fn admin_discovery_join_network_async(
        &self,
        accept_requests: bool,
    ) -> Result<serde_json::Value, ApiError> {
        let body = if accept_requests {
            serde_json::json!({ "enabled": true, "acceptFederationRequests": true })
        } else {
            serde_json::json!({ "enabled": true })
        };
        #[cfg(not(target_arch = "wasm32"))]
        return self
            .send_within(
                Method::POST,
                "api/v1/admin/discovery/p2p/enabled",
                Some(body),
                Some(DECODE_TIMEOUT),
            )
            .await;
        #[cfg(target_arch = "wasm32")]
        return self.post("api/v1/admin/discovery/p2p/enabled", body).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_join_network(
        &self,
        accept_requests: bool,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_join_network_async(accept_requests))
    }

    /// The server's name on the network (1–64 chars, no `|`). The server
    /// re-announces at once when a snapshot is published.
    pub async fn admin_discovery_set_name_async(
        &self,
        name: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.post("api/v1/admin/discovery/p2p/name", serde_json::json!({ "name": name })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_set_name(&self, name: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_set_name_async(name))
    }

    /// The catalog blurb beside the name (up to 180 chars, no `|`).
    pub async fn admin_discovery_set_description_async(
        &self,
        description: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(
            "api/v1/admin/discovery/p2p/description",
            serde_json::json!({ "description": description }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_set_description(
        &self,
        description: &str,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_set_description_async(description))
    }

    /// Befriend a server by its endpoint ticket (or bare endpoint id),
    /// persisted to the config so the friendship survives restarts.
    pub async fn admin_discovery_befriend_async(
        &self,
        peer: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(
            "api/v1/admin/discovery/p2p/join",
            serde_json::json!({ "peer": peer, "persist": true }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_befriend(&self, peer: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_befriend_async(peer))
    }

    /// Download (or refresh) a catalog peer's snapshot. The server answers
    /// when the verified transfer is done — minutes, across networks — so
    /// this call carries its own ceiling. A manual download arrives pinned.
    pub async fn admin_discovery_fetch_async(
        &self,
        endpoint_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        let body = serde_json::json!({ "endpointId": endpoint_id });
        #[cfg(not(target_arch = "wasm32"))]
        return self
            .send_within(
                Method::POST,
                "api/v1/admin/discovery/p2p/peer-dbs/fetch",
                Some(body),
                Some(FETCH_TIMEOUT),
            )
            .await;
        #[cfg(target_arch = "wasm32")]
        return self.post("api/v1/admin/discovery/p2p/peer-dbs/fetch", body).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_fetch(&self, endpoint_id: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_fetch_async(endpoint_id))
    }

    /// Pin (rotation immunity) or unpin a held snapshot.
    pub async fn admin_discovery_pin_async(
        &self,
        endpoint_id: &str,
        pinned: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(
            "api/v1/admin/discovery/p2p/peer-dbs/pin",
            serde_json::json!({ "endpointId": endpoint_id, "pinned": pinned }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_pin(
        &self,
        endpoint_id: &str,
        pinned: bool,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_pin_async(endpoint_id, pinned))
    }

    /// The one-argument peer actions: remove a snapshot, forget, block,
    /// unblock.
    pub async fn admin_discovery_peer_async(
        &self,
        action: PeerAction,
        endpoint_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(action.path(), serde_json::json!({ "endpointId": endpoint_id })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_peer(
        &self,
        action: PeerAction,
        endpoint_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_peer_async(action, endpoint_id))
    }

    /// One of the numeric settings; the server validates the bounds.
    pub async fn admin_discovery_setting_async(
        &self,
        setting: DiscoverySetting,
        value: u64,
    ) -> Result<serde_json::Value, ApiError> {
        let (path, key) = setting.route();
        let mut body = serde_json::Map::new();
        body.insert(key.to_string(), serde_json::json!(value));
        self.post(path, serde_json::Value::Object(body)).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_discovery_setting(
        &self,
        setting: DiscoverySetting,
        value: u64,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_discovery_setting_async(setting, value))
    }

    /// Federation's state (admin) — the discovery room reads `available`
    /// to decide whether "ask to federate" exists on this platform.
    pub async fn admin_federation_async(&self) -> Result<FederationParams, ApiError> {
        self.get("api/v1/admin/federation").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation(&self) -> Result<FederationParams, ApiError> {
        wait(self.admin_federation_async())
    }

    /// Pairing requests in both directions — the catalog's relationship
    /// column derives from them.
    pub async fn admin_federation_requests_async(&self) -> Result<FederationRequests, ApiError> {
        self.get("api/v1/admin/federation/requests").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_requests(&self) -> Result<FederationRequests, ApiError> {
        wait(self.admin_federation_requests_async())
    }

    /// Ask a discovery peer to federate: an optional message (≤ 500 chars)
    /// and the libraries offered back if they accept. No access changes
    /// hands now — 409 when a request is already open with that peer.
    pub async fn admin_federation_request_send_async(
        &self,
        endpoint_id: &str,
        offer_vpaths: &[String],
        message: Option<&str>,
    ) -> Result<serde_json::Value, ApiError> {
        let mut body = serde_json::json!({ "endpointId": endpoint_id, "offerVpaths": offer_vpaths });
        if let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) {
            body["message"] = serde_json::json!(message);
        }
        self.post("api/v1/admin/federation/requests", body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_request_send(
        &self,
        endpoint_id: &str,
        offer_vpaths: &[String],
        message: Option<&str>,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_request_send_async(endpoint_id, offer_vpaths, message))
    }

    // ── The rest of federation's admin surface: keys, requests, peers ──────

    /// The keys minted here, each with its swap-ready ticket while the
    /// endpoint runs.
    pub async fn admin_federation_keys_async(&self) -> Result<Vec<FederationKey>, ApiError> {
        self.get("api/v1/admin/federation/keys").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_keys(&self) -> Result<Vec<FederationKey>, ApiError> {
        wait(self.admin_federation_keys_async())
    }

    /// Mint a read-only key for `vpaths` (1–64-char name); `expires_at` is
    /// ISO 8601 in the future, or None for never.
    pub async fn admin_federation_mint_async(
        &self,
        name: &str,
        vpaths: &[String],
        limits: &FederationLimits,
        expires_at: Option<&str>,
    ) -> Result<MintedKey, ApiError> {
        let mut body = serde_json::json!({
            "name": name,
            "vpaths": vpaths,
            "streamKbps": limits.stream_kbps,
            "dailyMb": limits.daily_mb,
            "maxStreams": limits.max_streams,
        });
        if let Some(expires_at) = expires_at {
            body["expiresAt"] = serde_json::json!(expires_at);
        }
        self.post("api/v1/admin/federation/keys", body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_mint(
        &self,
        name: &str,
        vpaths: &[String],
        limits: &FederationLimits,
        expires_at: Option<&str>,
    ) -> Result<MintedKey, ApiError> {
        wait(self.admin_federation_mint_async(name, vpaths, limits, expires_at))
    }

    /// A key's limits, applied from its next request; the expiry is
    /// tri-state so a limit tweak never restarts an expiry clock by accident.
    pub async fn admin_federation_key_limits_async(
        &self,
        id: i64,
        limits: &FederationLimits,
        expiry: ExpiryChange,
    ) -> Result<serde_json::Value, ApiError> {
        let mut body = serde_json::json!({
            "streamKbps": limits.stream_kbps,
            "dailyMb": limits.daily_mb,
            "maxStreams": limits.max_streams,
        });
        match expiry {
            ExpiryChange::Keep => {}
            ExpiryChange::Never => body["expiresAt"] = serde_json::Value::Null,
            ExpiryChange::At(iso) => body["expiresAt"] = serde_json::json!(iso),
        }
        self.post(&format!("api/v1/admin/federation/keys/{id}/limits"), body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_key_limits(
        &self,
        id: i64,
        limits: &FederationLimits,
        expiry: ExpiryChange,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_key_limits_async(id, limits, expiry))
    }

    /// Revoke a key: live streams on it are cut at once.
    pub async fn admin_federation_key_revoke_async(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        self.send(Method::DELETE, &format!("api/v1/admin/federation/keys/{id}"), None).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_key_revoke(&self, id: i64) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_key_revoke_async(id))
    }

    /// Let the ticket be claimed again (the friend reinstalled).
    pub async fn admin_federation_key_reset_binding_async(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(&format!("api/v1/admin/federation/keys/{id}/reset-binding"), serde_json::json!({}))
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_key_reset_binding(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_key_reset_binding_async(id))
    }

    /// Accept an inbound request: mint for `vpaths` and send the ticket
    /// back; `accept_their_offer` also takes what they offered.
    pub async fn admin_federation_request_accept_async(
        &self,
        id: i64,
        vpaths: &[String],
        limits: &FederationLimits,
        expires_at: Option<&str>,
        accept_their_offer: bool,
    ) -> Result<serde_json::Value, ApiError> {
        let mut body = serde_json::json!({
            "vpaths": vpaths,
            "streamKbps": limits.stream_kbps,
            "dailyMb": limits.daily_mb,
            "maxStreams": limits.max_streams,
            "acceptTheirOffer": accept_their_offer,
        });
        if let Some(expires_at) = expires_at {
            body["expiresAt"] = serde_json::json!(expires_at);
        }
        self.post(&format!("api/v1/admin/federation/requests/{id}/accept"), body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_request_accept(
        &self,
        id: i64,
        vpaths: &[String],
        limits: &FederationLimits,
        expires_at: Option<&str>,
        accept_their_offer: bool,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_request_accept_async(id, vpaths, limits, expires_at, accept_their_offer))
    }

    /// Decline an inbound request; the server ignores that peer's asks
    /// for seven days.
    pub async fn admin_federation_request_reject_async(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(&format!("api/v1/admin/federation/requests/{id}/reject"), serde_json::json!({})).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_request_reject(&self, id: i64) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_request_reject_async(id))
    }

    /// Withdraw an outbound request that has not been answered.
    pub async fn admin_federation_request_cancel_async(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(&format!("api/v1/admin/federation/requests/{id}/cancel"), serde_json::json!({})).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_request_cancel(&self, id: i64) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_request_cancel_async(id))
    }

    /// Drop a finished record (409 while the exchange is still live).
    pub async fn admin_federation_request_dismiss_async(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        self.send(Method::DELETE, &format!("api/v1/admin/federation/requests/{id}"), None).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_request_dismiss(&self, id: i64) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_request_dismiss_async(id))
    }

    /// The inbox switch, live.
    pub async fn admin_federation_accept_requests_async(
        &self,
        enabled: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.post("api/v1/admin/federation/accept-requests", serde_json::json!({ "enabled": enabled }))
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_accept_requests(
        &self,
        enabled: bool,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_accept_requests_async(enabled))
    }

    /// The servers this one can read.
    pub async fn admin_federation_peers_async(&self) -> Result<Vec<FederationPeer>, ApiError> {
        self.get("api/v1/admin/federation/peers").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_peers(&self) -> Result<Vec<FederationPeer>, ApiError> {
        wait(self.admin_federation_peers_async())
    }

    /// Add a peer from a friend's `mstrfed1:` ticket; the server tests it
    /// in the background. 400 for a ticket that does not parse or one
    /// already added.
    pub async fn admin_federation_peer_add_async(
        &self,
        ticket: &str,
        name: Option<&str>,
    ) -> Result<serde_json::Value, ApiError> {
        let mut body = serde_json::json!({ "ticket": ticket });
        if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) {
            body["name"] = serde_json::json!(name);
        }
        self.post("api/v1/admin/federation/peers", body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_peer_add(
        &self,
        ticket: &str,
        name: Option<&str>,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_peer_add_async(ticket, name))
    }

    /// Test a peer now — the server dials it and waits for its health
    /// answer, so this gets the decode ceiling.
    pub async fn admin_federation_peer_test_async(&self, id: i64) -> Result<PeerTest, ApiError> {
        let path = format!("api/v1/admin/federation/peers/{id}/test");
        #[cfg(not(target_arch = "wasm32"))]
        return self
            .send_within(Method::POST, &path, Some(serde_json::json!({})), Some(DECODE_TIMEOUT))
            .await;
        #[cfg(target_arch = "wasm32")]
        return self.post(&path, serde_json::json!({})).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_peer_test(&self, id: i64) -> Result<PeerTest, ApiError> {
        wait(self.admin_federation_peer_test_async(id))
    }

    /// Whether the Discover panel may query this peer.
    pub async fn admin_federation_peer_discovery_async(
        &self,
        id: i64,
        enabled: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(
            &format!("api/v1/admin/federation/peers/{id}/discovery"),
            serde_json::json!({ "enabled": enabled }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_peer_discovery(
        &self,
        id: i64,
        enabled: bool,
    ) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_peer_discovery_async(id, enabled))
    }

    /// Forget a peer; its bridge is closed with it.
    pub async fn admin_federation_peer_remove_async(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        self.send(Method::DELETE, &format!("api/v1/admin/federation/peers/{id}"), None).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_federation_peer_remove(&self, id: i64) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_federation_peer_remove_async(id))
    }

    // ── Backups: the admin's destinations, their runs, the live status ─────

    pub async fn admin_backup_destinations_async(&self) -> Result<Vec<BackupDestination>, ApiError> {
        let list: BackupDestinations = self.get("api/v1/admin/backup/destinations").await?;
        Ok(list.destinations)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_destinations(&self) -> Result<Vec<BackupDestination>, ApiError> {
        wait(self.admin_backup_destinations_async())
    }

    pub async fn admin_backup_status_async(&self) -> Result<BackupStatus, ApiError> {
        self.get("api/v1/admin/backup/status").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_status(&self) -> Result<BackupStatus, ApiError> {
        wait(self.admin_backup_status_async())
    }

    /// The server's platform, home and default exclude patterns.
    pub async fn admin_backup_platform_async(&self) -> Result<BackupPlatform, ApiError> {
        self.get("api/v1/admin/backup/platform").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_platform(&self) -> Result<BackupPlatform, ApiError> {
        wait(self.admin_backup_platform_async())
    }

    /// Preview what saving this path would say: the hard errors and the
    /// warnings (same drive, exists with files, will be created…).
    /// `exclude_dest_id` lets an edit skip its own row in the overlap check.
    pub async fn admin_backup_check_path_async(
        &self,
        library_id: i64,
        dest_path: &str,
        exclude_dest_id: Option<i64>,
    ) -> Result<PathCheck, ApiError> {
        let mut body = serde_json::json!({ "libraryId": library_id, "destPath": dest_path });
        if let Some(id) = exclude_dest_id {
            body["excludeDestId"] = serde_json::json!(id);
        }
        self.post("api/v1/admin/backup/check-path", body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_check_path(
        &self,
        library_id: i64,
        dest_path: &str,
        exclude_dest_id: Option<i64>,
    ) -> Result<PathCheck, ApiError> {
        wait(self.admin_backup_check_path_async(library_id, dest_path, exclude_dest_id))
    }

    pub async fn admin_backup_add_async(
        &self,
        dest: &NewBackupDestination,
    ) -> Result<serde_json::Value, ApiError> {
        let mut body = serde_json::json!({
            "libraryId": dest.library_id,
            "destPath": dest.dest_path,
            "triggerType": dest.trigger_type,
            "retentionDays": dest.retention_days,
            "enabled": true,
            "interFileDelayMs": dest.inter_file_delay_ms,
        });
        if let Some(hour) = dest.daily_at_hour {
            body["dailyAtHour"] = serde_json::json!(hour);
        }
        if let Some(globs) = &dest.exclude_globs {
            body["excludeGlobs"] = serde_json::json!(globs);
        }
        self.post("api/v1/admin/backup/destinations", body).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_add(&self, dest: &NewBackupDestination) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_backup_add_async(dest))
    }

    /// Change any field but the library; a queued run picks the new
    /// settings up when its turn comes, a running one finishes with the old.
    pub async fn admin_backup_patch_async(
        &self,
        id: i64,
        patch: &BackupPatch,
    ) -> Result<serde_json::Value, ApiError> {
        self.send(Method::PATCH, &format!("api/v1/admin/backup/destinations/{id}"), Some(patch.body()))
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_patch(&self, id: i64, patch: &BackupPatch) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_backup_patch_async(id, patch))
    }

    /// Drop the schedule and its history; the files on disk stay.
    pub async fn admin_backup_remove_async(&self, id: i64) -> Result<serde_json::Value, ApiError> {
        self.send(Method::DELETE, &format!("api/v1/admin/backup/destinations/{id}"), None).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_remove(&self, id: i64) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_backup_remove_async(id))
    }

    /// Run now — `queued`, or `skipped` while a run is still in progress.
    /// A disabled destination answers 400.
    pub async fn admin_backup_run_async(&self, id: i64) -> Result<RunAnswer, ApiError> {
        self.post(&format!("api/v1/admin/backup/destinations/{id}/run"), serde_json::json!({})).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_run(&self, id: i64) -> Result<RunAnswer, ApiError> {
        wait(self.admin_backup_run_async(id))
    }

    /// The most recent runs, newest first.
    pub async fn admin_backup_history_async(&self, id: i64, limit: u32) -> Result<Vec<BackupRun>, ApiError> {
        let h: BackupHistory =
            self.get(&format!("api/v1/admin/backup/destinations/{id}/history?limit={limit}")).await?;
        Ok(h.history)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_backup_history(&self, id: i64, limit: u32) -> Result<Vec<BackupRun>, ApiError> {
        wait(self.admin_backup_history_async(id, limit))
    }

    // ── Torrents (admin) ────────────────────────────────────────────────────

    /// The chosen client, the access policy, every client's saved fields.
    pub async fn admin_torrent_params_async(&self) -> Result<TorrentParams, ApiError> {
        self.get("api/v1/admin/torrent").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_params(&self) -> Result<TorrentParams, ApiError> {
        wait(self.admin_torrent_params_async())
    }

    /// Choose the client: `disabled`, `transmission`, `qbittorrent`, `deluge`.
    /// Every client keeps its saved credentials across a switch.
    pub async fn admin_torrent_set_client_async(&self, client: &str) -> Result<serde_json::Value, ApiError> {
        self.post("api/v1/admin/torrent/client", serde_json::json!({ "client": client })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_set_client(&self, client: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_torrent_set_client_async(client))
    }

    /// Who may add torrents: `all` or `whitelist`.
    pub async fn admin_torrent_set_policy_async(&self, enabled_for: &str) -> Result<serde_json::Value, ApiError> {
        self.post("api/v1/admin/torrent/enabled-for", serde_json::json!({ "enabledFor": enabled_for })).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_set_policy(&self, enabled_for: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_torrent_set_policy_async(enabled_for))
    }

    /// One user's place on the whitelist.
    pub async fn admin_user_torrent_access_async(&self, username: &str, allow: bool) -> Result<serde_json::Value, ApiError> {
        self.post(
            "api/v1/admin/users/torrent-access",
            serde_json::json!({ "username": username, "allowTorrent": allow }),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_user_torrent_access(&self, username: &str, allow: bool) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_user_torrent_access_async(username, allow))
    }

    /// Probe a daemon with the given credentials: `connect` false saves
    /// nothing, true saves them after a good probe. Always HTTP 200 — read
    /// [`ProbeAnswer::ok`].
    pub async fn admin_torrent_probe_async(
        &self,
        client: &str,
        creds: &TorrentCreds,
        connect: bool,
    ) -> Result<ProbeAnswer, ApiError> {
        let verb = if connect { "connect" } else { "test" };
        self.send_within(Method::POST, &format!("api/v1/admin/torrent/{client}/{verb}"), Some(creds.body()), daemon_ceiling())
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_probe(&self, client: &str, creds: &TorrentCreds, connect: bool) -> Result<ProbeAnswer, ApiError> {
        wait(self.admin_torrent_probe_async(client, creds, connect))
    }

    /// Forget a client's credentials; the daemon keeps everything.
    pub async fn admin_torrent_disconnect_async(&self, client: &str) -> Result<serde_json::Value, ApiError> {
        self.post(&format!("api/v1/admin/torrent/{client}/disconnect"), serde_json::json!({})).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_disconnect(&self, client: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_torrent_disconnect_async(client))
    }

    /// A live probe of the saved credentials.
    pub async fn admin_torrent_status_async(&self) -> Result<TorrentStatus, ApiError> {
        self.send_within(Method::GET, "api/v1/admin/torrent/status", None, daemon_ceiling()).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_status(&self) -> Result<TorrentStatus, ApiError> {
        wait(self.admin_torrent_status_async())
    }

    /// Everything the daemon knows, mStream's rows marked.
    pub async fn admin_torrent_list_async(&self) -> Result<TorrentList, ApiError> {
        self.send_within(Method::GET, "api/v1/admin/torrent/list", None, daemon_ceiling()).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_list(&self) -> Result<TorrentList, ApiError> {
        wait(self.admin_torrent_list_async())
    }

    /// Drop an mStream-added torrent from the daemon, files kept. A 404 is
    /// the server refusing a torrent mStream did not add.
    pub async fn admin_torrent_remove_async(&self, info_hash: &str) -> Result<RemoveAnswer, ApiError> {
        self.send(Method::DELETE, &format!("api/v1/admin/torrent/{info_hash}"), None).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_remove(&self, info_hash: &str) -> Result<RemoveAnswer, ApiError> {
        wait(self.admin_torrent_remove_async(info_hash))
    }

    /// The cached daemon-side view of every library.
    pub async fn admin_torrent_vpath_access_async(&self) -> Result<VpathAccess, ApiError> {
        self.get("api/v1/admin/torrent/vpath-access").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_vpath_access(&self) -> Result<VpathAccess, ApiError> {
        wait(self.admin_torrent_vpath_access_async())
    }

    /// Re-run the probe for one library, or every library when `None`.
    pub async fn admin_torrent_auto_detect_async(&self, vpath: Option<&str>) -> Result<VpathAccess, ApiError> {
        let body = match vpath {
            Some(v) => serde_json::json!({ "vpathName": v }),
            None => serde_json::json!({}),
        };
        self.send_within(Method::POST, "api/v1/admin/torrent/vpath-access/auto-detect", Some(body), daemon_ceiling())
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_auto_detect(&self, vpath: Option<&str>) -> Result<VpathAccess, ApiError> {
        wait(self.admin_torrent_auto_detect_async(vpath))
    }

    /// Type the daemon's path for a library; the server verifies it with the
    /// same probe. A 422 carries the daemon's reason.
    pub async fn admin_torrent_manual_mapping_async(&self, vpath: &str, daemon_path: &str) -> Result<serde_json::Value, ApiError> {
        self.send_within(
            Method::POST,
            "api/v1/admin/torrent/vpath-access/manual",
            Some(serde_json::json!({ "vpathName": vpath, "daemonPath": daemon_path })),
            daemon_ceiling(),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_manual_mapping(&self, vpath: &str, daemon_path: &str) -> Result<serde_json::Value, ApiError> {
        wait(self.admin_torrent_manual_mapping_async(vpath, daemon_path))
    }

    /// Every library's template plus the server's variables and sample.
    pub async fn admin_torrent_path_templates_async(&self) -> Result<PathTemplates, ApiError> {
        self.get("api/v1/admin/torrent/path-templates").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_path_templates(&self) -> Result<PathTemplates, ApiError> {
        wait(self.admin_torrent_path_templates_async())
    }

    /// Save a library's template; `None` clears it (freeform entry again).
    pub async fn admin_torrent_set_template_async(&self, vpath: &str, template: Option<&str>) -> Result<TemplateSaved, ApiError> {
        self.send(
            Method::PUT,
            &format!("api/v1/admin/torrent/path-templates/{vpath}"),
            Some(serde_json::json!({ "template": template })),
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_set_template(&self, vpath: &str, template: Option<&str>) -> Result<TemplateSaved, ApiError> {
        wait(self.admin_torrent_set_template_async(vpath, template))
    }

    /// Hand the server a `.torrent` for content already on disk: multipart,
    /// one file, the libraries to search (none = every library). Always HTTP
    /// 200 — the outcome is in the body. The server hashes the files before
    /// it answers, so the call gets the torrent routes' own ceiling.
    pub async fn admin_torrent_seed_existing_async(
        &self,
        file_name: &str,
        bytes: &[u8],
        vpaths: &[String],
    ) -> Result<SeedOutcome, ApiError> {
        let mut form = Multipart::new();
        if !vpaths.is_empty() {
            form.field("vpaths", &serde_json::to_string(vpaths).unwrap_or_default());
        }
        form.file("torrentFile", file_name, bytes);
        self.post_multipart("api/v1/admin/torrent/seed-existing", form, Some(DETECT_TIMEOUT)).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_torrent_seed_existing(&self, file_name: &str, bytes: &[u8], vpaths: &[String]) -> Result<SeedOutcome, ApiError> {
        wait(self.admin_torrent_seed_existing_async(file_name, bytes, vpaths))
    }

    /// Every user with their flags, keyed by username.
    pub async fn admin_users_async(&self) -> Result<std::collections::BTreeMap<String, AdminUser>, ApiError> {
        self.get("api/v1/admin/users").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn admin_users(&self) -> Result<std::collections::BTreeMap<String, AdminUser>, ApiError> {
        wait(self.admin_users_async())
    }

    /// Per-library scan progress (works for any signed-in user; on a fresh
    /// zero-account server too).
    pub async fn scan_progress_async(&self) -> Result<Vec<ScanProgressRow>, ApiError> {
        self.get("api/v1/scan/progress").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan_progress(&self) -> Result<Vec<ScanProgressRow>, ApiError> {
        wait(self.scan_progress_async())
    }

    /// The enrichment passes' live state — what runs AFTER the file
    /// scan. Same auth tier as scan_progress.
    pub async fn scan_status_async(&self) -> Result<crate::api::types::ScanStatus, ApiError> {
        self.get("api/v1/scan/status").await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan_status(&self) -> Result<crate::api::types::ScanStatus, ApiError> {
        wait(self.scan_status_async())
    }

}

/// A `multipart/form-data` body, assembled by hand: the torrent routes
/// are the API's only multipart, and reqwest's feature for it would pull
/// a MIME-guessing dependency in for one boundary string. Same encoding
/// on both builds.
pub(crate) struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    pub(crate) fn new() -> Self {
        Multipart {
            boundary: format!("----mstream-player-{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..)),
            body: Vec::new(),
        }
    }

    /// A header parameter value: quotes and line breaks would end the
    /// part early, so they are replaced rather than escaped (the server
    /// only ever shows the filename back).
    fn param(value: &str) -> String {
        value.chars().map(|c| if c == '"' || c == '\r' || c == '\n' { '_' } else { c }).collect()
    }

    pub(crate) fn field(&mut self, name: &str, value: &str) -> &mut Self {
        self.body.extend(format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n",
            self.boundary,
            Self::param(name)
        ).into_bytes());
        self.body.extend(value.as_bytes());
        self.body.extend(b"\r\n");
        self
    }

    pub(crate) fn file(&mut self, name: &str, filename: &str, bytes: &[u8]) -> &mut Self {
        self.body.extend(format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: application/x-bittorrent\r\n\r\n",
            self.boundary,
            Self::param(name),
            Self::param(filename)
        ).into_bytes());
        self.body.extend(bytes);
        self.body.extend(b"\r\n");
        self
    }

    /// The `Content-Type` header value and the finished body.
    pub(crate) fn finish(mut self) -> (String, Vec<u8>) {
        self.body.extend(format!("--{}--\r\n", self.boundary).into_bytes());
        (format!("multipart/form-data; boundary={}", self.boundary), self.body)
    }
}

/// Pull mStream's `{"error": "..."}` out of a failure body, falling back to a
/// trimmed excerpt of whatever was actually returned. The torrent routes'
/// shape — `{ ok: false, error: <code>, message: <words> }` — reads as the
/// sentence, never the code.
fn extract_error(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        // Newer routes answer `{error: <code>, message: <sentence>}`; the
        // sentence is the one for a human.
        let message = v.get("message").and_then(|m| m.as_str()).filter(|m| !m.trim().is_empty());
        let error = v.get("error").and_then(|e| e.as_str());
        if let Some(text) = message.or(error) {
            return text.to_string();
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
    fn a_peer_client_rewrites_every_path_onto_the_parents_proxies() {
        // Contract clause 27: reads through the browse proxy, art through
        // the art proxy; the parent's base and token throughout.
        let client = Client::new("http://parent:3000/").unwrap().with_token(Some("pt".into())).with_peer(Some(3));
        assert_eq!(
            client.endpoint("api/v1/db/albums").unwrap().as_str(),
            "http://parent:3000/api/v1/federation/peers/3/api/api/v1/db/albums"
        );
        assert_eq!(
            client.endpoint("album-art/cover.jpeg").unwrap().as_str(),
            "http://parent:3000/api/v1/federation/peers/3/art/cover.jpeg"
        );
        assert_eq!(client.server(), "http://parent:3000", "the base is the parent's");
        assert_eq!(client.peer(), Some(3));
        let plain = Client::new("http://parent:3000").unwrap();
        assert_eq!(plain.endpoint("api/v1/db/albums").unwrap().as_str(), "http://parent:3000/api/v1/db/albums");
    }

    #[test]
    fn a_multipart_body_carries_fields_and_the_file_between_its_boundary() {
        let mut form = Multipart::new();
        form.field("vpath", "music").field("renameRoot", "true");
        form.file("torrentFile", "vela \"deluxe\".torrent", b"d4:infod4:name4:Velaee");
        let (content_type, body) = form.finish();
        let boundary = content_type.strip_prefix("multipart/form-data; boundary=").unwrap().to_string();
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"vpath\"\r\n\r\nmusic\r\n")));
        assert!(text.contains("name=\"renameRoot\"\r\n\r\ntrue\r\n"));
        assert!(
            text.contains("name=\"torrentFile\"; filename=\"vela _deluxe_.torrent\"\r\nContent-Type: application/x-bittorrent\r\n\r\nd4:infod4:name4:Velaee\r\n"),
            "quotes in a filename cannot end the part early: {text}"
        );
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
        assert_eq!(text.matches(&format!("--{boundary}")).count(), 4, "three parts and the close");
    }

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
    fn server_info_reads_the_version_and_tolerates_its_absence() {
        // `GET /api/` — "server" is the mStream version; a future shape
        // that drops or adds fields must not break the read.
        let info: ServerInfo = serde_json::from_str(
            r#"{"server":"5.13.2","apiVersions":["1"],"features":{"subsonic":false}}"#,
        )
        .unwrap();
        assert_eq!(info.version.as_deref(), Some("5.13.2"));
        assert_eq!(info.api_versions, vec!["1"]);

        let bare: ServerInfo = serde_json::from_str("{}").unwrap();
        assert_eq!(bare.version, None);
    }

    #[test]
    fn a_self_signed_client_still_builds_on_the_verified_path() {
        // The flag only loosens TLS verification; everything else about the
        // client — base URL handling above all — is the same construction.
        let c = Client::new_with("https://attic.local:3000", true).unwrap();
        assert_eq!(c.server(), "https://attic.local:3000");
        assert!(Client::new_with("not a url", true).is_err());
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
        // The torrent routes carry a code under `error` and the sentence
        // under `message`: the sentence is the one for a human.
        assert_eq!(
            extract_error(r#"{"ok":false,"error":"no_source","message":"Provide a .torrent file"}"#),
            "Provide a .torrent file"
        );
    }
}
