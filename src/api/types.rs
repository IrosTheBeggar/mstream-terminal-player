//! Response types for the mStream v1 API.
//!
//! Hand-written against mStream's `docs/openapi.yaml` and verified against a
//! live server. Deliberately tolerant: every struct carries `#[serde(default)]`
//! and unknown fields are ignored, so a client built against one server version
//! keeps working against another that adds or drops fields. Only genuinely
//! load-bearing fields (a track's `filepath`) are required.

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

/// Accept an explicit JSON `null` where the server documents a nullable object,
/// yielding `T::default()` instead of failing the whole response.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A SQLite flag: the server hands `0`/`1` (or a real boolean, or null)
/// where a client wants a `bool`.
fn int_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Bool(b) => b,
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    #[serde(default)]
    pub vpaths: Vec<String>,
}

/// `GET /api/v1/ping` — the one-shot capability bootstrap.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ping {
    pub vpaths: Vec<String>,
    pub transcode: Option<TranscodeInfo>,
    #[serde(rename = "noFileModify")]
    pub no_file_modify: bool,
    #[serde(rename = "noUpload")]
    pub no_upload: bool,

    // Discovery features. All are off by default in mStream, and an older
    // server omits the field entirely — which `#[serde(default)]` turns into
    // `false`, the same answer for the same reason.
    /// The local audio-embedding index: similar tracks and similar artists.
    pub discovery: bool,
    /// Great-circle paths between two tracks' embeddings (Sonic Journey).
    #[serde(rename = "discoveryPath")]
    pub discovery_path: bool,
    /// Similarity against peers' local snapshot copies.
    #[serde(rename = "discoveryP2p")]
    pub discovery_p2p: bool,
    /// Similarity federated out to paired servers.
    #[serde(rename = "federationDiscovery")]
    pub federation_discovery: bool,
}

/// What this server can actually do, lifted out of [`Ping`].
///
/// The rule everywhere downstream is **no flag, no probe**: every one of these
/// is disabled by default server-side, and asking anyway earns a 403 that
/// looks like a failure but isn't. Carried as a small `Copy` value so any code
/// deciding whether to offer a feature can hold one without ceremony.
///
/// Each feature is gated on its own flag rather than inferred from another —
/// the server reports them separately, so the client believes them separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub discovery: bool,
    pub discovery_path: bool,
    pub discovery_p2p: bool,
    pub federation_discovery: bool,
}

impl From<&Ping> for Capabilities {
    fn from(ping: &Ping) -> Self {
        Capabilities {
            discovery: ping.discovery,
            discovery_path: ping.discovery_path,
            discovery_p2p: ping.discovery_p2p,
            federation_discovery: ping.federation_discovery,
        }
    }
}

impl Capabilities {
    /// Feature names to show someone asking what this server offers.
    pub fn enabled_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.discovery {
            names.push("similarity");
        }
        if self.discovery_path {
            names.push("sonic journey");
        }
        if self.discovery_p2p {
            names.push("p2p discovery");
        }
        if self.federation_discovery {
            names.push("federated discovery");
        }
        names
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TranscodeInfo {
    /// The server's *default* codec — frequently `opus`, which this player
    /// cannot decode. Informational only: always request a codec explicitly
    /// (see `api::urls::TranscodeCodec`).
    #[serde(rename = "defaultCodec")]
    pub default_codec: Option<String>,
    #[serde(rename = "defaultBitrate")]
    pub default_bitrate: Option<String>,
}

/// Track metadata, as nested under `metadata` on library responses.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub disk: Option<u32>,
    pub year: Option<i32>,
    /// Seconds. Feeds the engine's duration hint so remote tracks skip the
    /// probe fetch.
    pub duration: Option<f64>,
    pub rating: Option<u32>,
    pub bpm: Option<u32>,
    pub hash: Option<String>,
    #[serde(rename = "album-art")]
    pub album_art: Option<String>,
    #[serde(rename = "musical-key")]
    pub musical_key: Option<String>,
    #[serde(rename = "play-count")]
    pub play_count: Option<u64>,
    /// Whether the server holds words for this track, and whether they carry
    /// timings. Cheap to read off a listing, and it saves offering a lyrics
    /// view for a track that has none.
    #[serde(rename = "has-lyrics")]
    pub has_lyrics: bool,
    #[serde(rename = "has-synced-lyrics")]
    pub has_synced_lyrics: bool,
    /// What the file is, as the scanner read it — "mp3", "flac". Not the
    /// extension: a mislabelled file is described by what is inside it.
    pub format: Option<String>,
    /// Bits per second, so 320000 rather than 320.
    pub bitrate: Option<u64>,
    #[serde(rename = "sample-rate")]
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    /// Only lossless formats carry one, so its absence is itself a fact.
    #[serde(rename = "bit-depth")]
    pub bit_depth: Option<u16>,
    #[serde(rename = "file-size")]
    pub file_size: Option<u64>,
    /// The scanner returns a list; most files name one.
    pub genres: Vec<String>,
    /// How many tracks the release has, for "3 of 12".
    #[serde(rename = "track-total")]
    pub track_total: Option<u32>,
    #[serde(rename = "disc-total")]
    pub disc_total: Option<u32>,
}

impl TrackMetadata {
    /// Best available display title: the tag, else nothing (callers fall back
    /// to the filename).
    pub fn display_title(&self) -> Option<&str> {
        self.title.as_deref().filter(|s| !s.is_empty())
    }
}

/// A library track. `filepath` is the vpath-prefixed path used to build
/// `/media/...` URLs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Track {
    pub filepath: String,
    #[serde(default, deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

impl Track {
    /// Filename component of the vpath, for display when tags are missing.
    pub fn file_name(&self) -> &str {
        self.filepath.rsplit('/').next().unwrap_or(&self.filepath)
    }

    pub fn display_name(&self) -> String {
        match (self.metadata.artist.as_deref(), self.metadata.display_title()) {
            (Some(a), Some(t)) if !a.is_empty() => format!("{a} - {t}"),
            (_, Some(t)) => t.to_string(),
            _ => self.file_name().to_string(),
        }
    }
}

/// Seconds as `m:ss`, and `--:--` for anything that isn't a length.
///
/// Here beside [`Track::display_name`] because it is the same kind of thing:
/// how the client renders a fact about a track. It lived in `cmd_library`,
/// which meant the whole render layer imported the CLI smoke-test harness to
/// print a duration (finding #66).
pub fn fmt_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--".to_string();
    }
    let total = seconds.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Album {
    pub name: Option<String>,
    pub artist: Option<String>,
    pub year: Option<i32>,
    pub album_art_file: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArtistsResponse {
    pub artists: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Genre {
    pub name: String,
    pub track_count: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GenresResponse {
    pub genres: Vec<Genre>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AlbumsResponse {
    pub albums: Vec<Album>,
}

// ── File explorer ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DirListing {
    pub path: String,
    pub directories: Vec<DirEntry>,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DirEntry {
    pub name: String,
}

/// One value of `GET api/v1/admin/directories` — the response is a map of
/// vpath name → this. `root` is the folder path as the SERVER sees it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AdminDirEntry {
    /// The library's row id — what the backup routes address it by.
    pub id: i64,
    pub root: String,
    /// Per-library: whether the scanner follows symlinks inside it.
    #[serde(rename = "followSymlinks")]
    pub follow_symlinks: bool,
}

// ── Backups ─────────────────────────────────────────────────────────────────

/// `GET api/v1/admin/backup/destinations`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackupDestinations {
    pub destinations: Vec<BackupDestination>,
}

/// One backup destination: a library copied to a folder on another drive
/// on a schedule, with its most recent run.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackupDestination {
    pub id: i64,
    pub library_id: i64,
    #[serde(deserialize_with = "null_default")]
    pub library_name: String,
    pub dest_path: String,
    /// `after-scan` | `daily` | `manual`.
    pub trigger_type: String,
    pub daily_at_hour: Option<u32>,
    /// Days deleted or changed files stay recoverable in the backup's
    /// trash; 0 = no trash.
    pub retention_days: u32,
    #[serde(deserialize_with = "int_bool")]
    pub enabled: bool,
    /// The per-file pause during a run, ms; 0 = no throttle.
    #[serde(deserialize_with = "null_default")]
    pub inter_file_delay_ms: u32,
    /// The effective exclude patterns (the server's defaults when the row
    /// stores none).
    #[serde(rename = "excludeGlobs")]
    pub exclude_globs: Vec<String>,
    /// The most recent attempt, if any (dedup skips excluded).
    #[serde(rename = "lastRun")]
    pub last_run: Option<BackupRun>,
    pub created_at: String,
}

/// One run — a destination's `lastRun`, and the rows of
/// `GET api/v1/admin/backup/destinations/:id/history`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackupRun {
    pub id: i64,
    /// SQLite UTC `YYYY-MM-DD HH:MM:SS`.
    pub started_at: String,
    pub finished_at: Option<String>,
    /// `running` | `success` | `partial` | `failed` | `skipped`.
    pub status: String,
    pub trigger_reason: Option<String>,
    pub files_copied: u64,
    pub files_unchanged: u64,
    pub files_trashed: u64,
    pub bytes_copied: u64,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackupHistory {
    pub history: Vec<BackupRun>,
}

/// `GET api/v1/admin/backup/status`: the run in flight, if any, and how
/// many tasks wait behind the active scan or backup.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct BackupStatus {
    pub active: Option<ActiveBackup>,
    pub queue_length: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ActiveBackup {
    pub destination_id: i64,
    pub history_id: i64,
    pub library_name: Option<String>,
    pub dest_path: Option<String>,
    pub started_at: Option<String>,
    pub trigger_reason: Option<String>,
    pub files_copied: u64,
    pub files_unchanged: u64,
    pub files_trashed: u64,
    pub bytes_copied: u64,
    /// The previous run's copied + unchanged + trashed — the progress
    /// denominator; None on a destination's first run.
    pub expected_files: Option<u64>,
}

/// `GET api/v1/admin/backup/platform`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct BackupPlatform {
    pub platform: String,
    pub homedir: String,
    pub default_excludes: Vec<String>,
}

/// `POST api/v1/admin/backup/check-path` — the errors a save would raise
/// and the warnings an operator should read first.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PathCheck {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub info: PathCheckInfo,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PathCheckInfo {
    pub dest_exists: bool,
    pub dest_is_empty: Option<bool>,
    pub parent_exists: bool,
    pub same_drive: Option<bool>,
    pub same_drive_reliable: bool,
}

/// `POST api/v1/admin/backup/destinations/:id/run`: `queued`, or
/// `skipped` when a run is already in progress.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct RunAnswer {
    pub status: String,
}

// ── Discovery network (P2P) ─────────────────────────────────────────────────

/// `GET api/v1/admin/discovery/p2p/status` — the sidecar's live mesh state,
/// the server's announced identity, and the settings the admin room edits.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscoveryStatus {
    pub enabled: bool,
    /// The p2p-sidecar binary exists for this platform.
    pub binary_found: bool,
    /// Missing, but the server can download it when enabling.
    pub binary_fetchable: bool,
    pub running: bool,
    pub endpoint_id: Option<String>,
    /// The endpoint ticket a friend pastes to befriend this server.
    pub ticket: Option<String>,
    /// Subscribed to the catalog topic — the mesh may still be empty.
    pub joined: bool,
    /// Live gossip links; the sidecar's own count is the authority.
    pub neighbors: u32,
    pub neighbor_ids: Vec<String>,
    pub watchdog: DiscoveryWatchdog,
    pub recovery: DiscoveryRecovery,
    pub known_peers: u32,
    pub community_seeds: bool,
    #[serde(deserialize_with = "null_default")]
    pub server_name: String,
    #[serde(deserialize_with = "null_default")]
    pub server_description: String,
    pub max_peer_db_storage_mb: u64,
    pub auto_fetch_count: u32,
    pub rotation_days: u32,
    pub peer_retention_days: u32,
    pub blocked_peers: Vec<String>,
}

/// The sidecar memory watchdog: `last_rss_mb` is null before the first
/// reading (or where RSS cannot be read); `max_rss_mb` 0 means off.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscoveryWatchdog {
    pub last_rss_mb: Option<f64>,
    pub restarts: u32,
    pub max_rss_mb: u64,
}

/// Crash recovery owns the sidecar while `attempts > 0` or a retry is armed.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscoveryRecovery {
    pub attempts: u32,
    pub retry_pending: bool,
}

/// `GET api/v1/admin/discovery/p2p/catalog` — every server heard from,
/// most useful first, with what the local shelf holds of each.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscoveryCatalog {
    pub peers: Vec<CatalogPeer>,
    /// Rows the server filtered out: their embedding model cannot power
    /// this server's similar-search (`?includeIncompatible=1` shows them).
    pub hidden_incompatible: u32,
    pub local_model_id: Option<String>,
    pub auto_fetch: bool,
    pub storage: CatalogStorage,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogStorage {
    pub used_bytes: u64,
    pub cap_bytes: u64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogPeer {
    /// The peer's endpoint id (64 hex) — the key every action takes.
    pub from: String,
    pub payload: PeerPayload,
    pub updated_at: String,
    /// Heard within the last ~90 s.
    pub online: bool,
    /// Live holders of this peer's current snapshot.
    pub seeders: u32,
    /// What the local shelf holds of this peer, if anything.
    pub fetched: Option<HeldSnapshot>,
    /// `None` = unknown (no local embedding model established yet).
    pub compatible: Option<bool>,
}

/// The peer's own signed announcement — everything in it is REMOTE text.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PeerPayload {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    pub row_count: u64,
    pub snapshot_seq: u64,
    #[serde(deserialize_with = "null_default")]
    pub model_id: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct HeldSnapshot {
    pub snapshot_seq: u64,
    /// The peer has announced a newer snapshot than the one held.
    pub stale: bool,
    pub size_bytes: u64,
    pub fetched_at: String,
    pub first_fetched_at: String,
    /// Immune to rotation.
    pub pinned: bool,
}

/// `GET api/v1/admin/discovery/p2p/activity?since=<seq>` — the discovery
/// slice of the server log, delta-polled by sequence number.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscoveryActivity {
    pub entries: Vec<ActivityEntry>,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ActivityEntry {
    pub seq: u64,
    /// ISO timestamp.
    pub t: String,
    /// `error` | `warn` | `info` | `debug`.
    pub level: String,
    pub message: String,
}

/// `GET api/v1/admin/federation` — the endpoint's state and the mint
/// dialog's defaults.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct FederationParams {
    pub enabled: bool,
    /// Iroh has a build for this platform.
    pub available: bool,
    /// The endpoint is up (an endpoint id exists).
    pub running: bool,
    pub endpoint_id: Option<String>,
    /// Connected to a relay.
    pub online: bool,
    pub relay_url: Option<String>,
    /// What a new key's limits are prefilled with.
    pub limit_defaults: FederationLimits,
    /// The federation-requests inbox is open to discovery peers.
    pub accept_requests: bool,
}

/// Per-key bandwidth caps; 0 = unlimited.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct FederationLimits {
    pub stream_kbps: u64,
    pub daily_mb: u64,
    pub max_streams: u64,
}

/// One key this server minted — a read-only grant for the libraries it
/// names — as `GET api/v1/admin/federation/keys` lists it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FederationKey {
    pub id: i64,
    pub name: String,
    pub library_names: Vec<String>,
    pub stream_kbps: u64,
    pub daily_mb: u64,
    pub max_streams: u64,
    /// SQLite UTC `YYYY-MM-DD HH:MM:SS`; None = never.
    pub expires_at: Option<String>,
    #[serde(deserialize_with = "int_bool")]
    pub expired: bool,
    /// Bytes served to this key today (UTC), live.
    pub usage_today_bytes: u64,
    pub last_used: Option<String>,
    /// The endpoint that redeemed the ticket first (TOFU); None = not yet.
    pub bound_endpoint_id: Option<String>,
    pub bound_at: Option<String>,
    pub created_at: String,
    /// The swap-ready `mstrfed1:` ticket — None while the endpoint is down.
    pub ticket: Option<String>,
}

/// `POST api/v1/admin/federation/keys` — the fresh key and its ticket.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MintedKey {
    pub id: i64,
    pub name: String,
    pub ticket: Option<String>,
}

/// `GET api/v1/admin/federation/peers` — a server this one can read. The
/// row also carries the peer's endpoint ticket and API key; a client has
/// no business with either, so they are not read.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FederationPeer {
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// `ok`, or the last failed test's words; None = never tested.
    pub last_status: Option<String>,
    /// Stamped by an `ok` test only.
    pub last_seen: Option<String>,
    /// The Discover panel may send this peer similarity queries.
    #[serde(deserialize_with = "int_bool")]
    pub use_discovery: bool,
    pub added_at: String,
}

/// `POST api/v1/admin/federation/peers/:id/test`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PeerTest {
    pub ok: bool,
    pub error: Option<String>,
    pub health: Option<PeerHealth>,
}

/// The peer's own answer — remote text.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PeerHealth {
    pub libraries: Vec<String>,
}

/// `GET api/v1/admin/federation/requests` — pairing asks in both
/// directions; the catalog derives its relationship column from them.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct FederationRequests {
    pub accept_requests: bool,
    pub requests: Vec<FederationRequest>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FederationRequest {
    pub id: i64,
    pub peer_endpoint_id: String,
    /// Self-asserted by the remote server.
    pub peer_name: Option<String>,
    /// `in` | `out`.
    pub direction: String,
    /// `received` | `accepted` | `granting` | `completed` | `pending-delivery`
    /// | `delivered` | `rejected` | `refused` | `cancelled` | `expired`.
    pub state: String,
    /// Remote text on an inbound request, ours on an outbound one.
    #[serde(deserialize_with = "null_default")]
    pub message: String,
    /// What they offer (inbound) or what we offered (outbound).
    pub offered_libraries: Vec<String>,
    /// The delivery ladder: failures so far, and when the next try is.
    pub fail_count: u32,
    pub next_attempt_at: Option<String>,
    pub reject_reason: Option<String>,
    /// The peer row a completed exchange created.
    pub created_peer_id: Option<i64>,
    /// SQLite UTC `YYYY-MM-DD HH:MM:SS`.
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileEntry {
    pub name: String,
    /// File extension as classified by the server ("mp3", "flac", ...).
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Present only when the listing asked for metadata, and doubly wrapped on
    /// the wire — see [`FileMetadata`].
    pub metadata: Option<FileMetadata>,
}

/// The `metadata` a file-explorer listing carries when `pullMetadata` is set.
///
/// Two layers on purpose, and both matter: the outer one is the server's own
/// canonical `<vpath>/<relative path>` for the file, and the inner one is the
/// tags — `null` for a file that is on disk but not in the database, which is
/// every playlist file and anything added since the last scan.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileMetadata {
    pub filepath: String,
    pub metadata: Option<TrackMetadata>,
}

// ── Search ──────────────────────────────────────────────────────────────────

/// `POST /api/v1/db/search` — five parallel result categories.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchResults {
    pub artists: Vec<SearchGroup>,
    pub albums: Vec<SearchGroup>,
    /// Tracks whose *title* matched.
    pub title: Vec<SearchTrack>,
    /// Tracks whose *filepath* matched.
    pub files: Vec<SearchTrack>,
    /// Tracks whose stored lyrics matched.
    pub lyrics: Vec<SearchTrack>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.artists.is_empty()
            && self.albums.is_empty()
            && self.title.is_empty()
            && self.files.is_empty()
            && self.lyrics.is_empty()
    }
}

/// An artist or album hit. (The server also sends `filepath: false` on these
/// rows as a "not a track" sentinel; we distinguish by type instead.)
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchGroup {
    pub name: String,
    pub album_art_file: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchTrack {
    pub name: String,
    pub filepath: String,
    pub album_art_file: Option<String>,
    /// Documented nullable — the track row can vanish between match and
    /// enrichment.
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

// ── Auto-DJ ─────────────────────────────────────────────────────────────────

/// A tempo window, inclusive. Also what [`crate::dj::bpm_windows`] works in —
/// it had its own identical `BpmRange` and a closure to convert between them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BpmWindow {
    pub min: f64,
    pub max: f64,
}

/// Request body for `POST /api/v1/db/random-songs`, which returns a single
/// track. Empty fields are omitted so the server's filter waterfall only sees
/// constraints we actually mean.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomSongRequest {
    /// Round-trip cursor: echo back what the previous response returned, and
    /// the server keeps the session from repeating itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_list: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bpm_ranges: Vec<BpmWindow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bpm_ranges_wide: Vec<BpmWindow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub musical_keys: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_artists: Vec<String>,
    /// 1–10. Omitted when zero, which the server also reads as "no floor".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_rating: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    /// Only meaningful alongside `genres`: "whitelist" (default) or
    /// "blacklist". Note the deliberate asymmetry server-side — a whitelist
    /// blocks untagged tracks, a blacklist lets them through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre_mode: Option<String>,

    // The sonic pool. The server's Joi schema binds these with `.and(...)`,
    // so sending one without the other is a 400 — see `with_sonic_pool`.
    /// 1–8 seed paths, averaged into a centroid when there is more than one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub similar_to: Vec<String>,
    /// Raw cosine floor, 0..1. A hard constraint: the server's waterfall
    /// relaxes tempo, key and artist *within* the pool and never widens past
    /// it, failing loudly instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_similarity: Option<f64>,
}

/// Most seed paths the server will average into a centroid.
pub const SIMILAR_TO_MAX: usize = 8;

impl RandomSongRequest {
    /// Attach the sonic pool, or leave it off entirely.
    ///
    /// Both fields go on together or neither does — the server enforces that
    /// with `.and('similarTo', 'minSimilarity')`, so half a pool is a 400
    /// rather than a looser search.
    pub fn with_sonic_pool(mut self, seeds: &[String], threshold: Option<f64>) -> Self {
        let seeds: Vec<String> = seeds.iter().take(SIMILAR_TO_MAX).cloned().collect();
        match (seeds.is_empty(), threshold) {
            (false, Some(threshold)) => {
                self.similar_to = seeds;
                self.min_similarity = Some(threshold);
            }
            _ => {
                self.similar_to.clear();
                self.min_similarity = None;
            }
        }
        self
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RandomSongsResponse {
    pub songs: Vec<Track>,
    #[serde(rename = "ignoreList")]
    pub ignore_list: Vec<u32>,
    /// Present only when the request carried a sonic pool.
    pub sonic: Option<SonicReport>,
}

/// What the sonic pool did on this pick — the feedback that makes the
/// tightness slider tunable rather than guesswork.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SonicReport {
    /// Cosine of the pick against the seed or centroid. Null when the pick
    /// somehow has no vector.
    pub similarity: Option<f64>,
    /// How many analysed tracks sit inside the threshold at all, before the
    /// other filters cut it down.
    #[serde(rename = "poolSize")]
    pub pool_size: u32,
}

/// `POST /api/v1/discovery/local/similar/tracks` — nearest neighbours in the
/// embedding space the discovery worker builds.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarTracksResponse {
    /// True when the seed exists but hasn't been embedded yet. Transient, not
    /// an error — the results list is simply empty.
    #[serde(rename = "notAnalyzed")]
    pub not_analyzed: bool,
    pub results: Vec<SimilarTrack>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarTrack {
    pub filepath: String,
    /// Cosine similarity to the seed, 0..1.
    pub similarity: f64,
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

impl SimilarTrack {
    pub fn into_track(self) -> Track {
        Track { filepath: self.filepath, metadata: self.metadata }
    }
}

/// `POST /api/v1/discovery/local/similar/artists` — artists whose overall
/// sound sits near the seed's, each with a way in.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarArtistsResponse {
    /// The seed artist has no embedded tracks yet.
    #[serde(rename = "notAnalyzed")]
    pub not_analyzed: bool,
    /// The server stopped walking the ranking before filling the limit, so a
    /// short list means "we stopped looking", not "there is no more".
    pub capped: bool,
    pub results: Vec<SimilarArtist>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarArtist {
    pub artist: String,
    pub similarity: f64,
    /// How many of this artist's tracks the discovery worker has embedded —
    /// a similarity drawn from three tracks deserves less trust than one
    /// drawn from fifty.
    #[serde(rename = "analyzedCount")]
    pub analyzed_count: u32,
    /// The model's own style guesses for this artist, not file tags.
    #[serde(rename = "genreTags", deserialize_with = "null_default")]
    pub genre_tags: Vec<String>,
    /// Up to two of this artist's tracks that sit closest to the *seed's*
    /// sound — playable doorways that continue what you were listening to,
    /// rather than whatever the artist is best known for.
    #[serde(rename = "entryPoints")]
    pub entry_points: Vec<Track>,
}

// ── Sonic Journey ───────────────────────────────────────────────────────────

/// `POST /api/v1/discovery/local/path` — a walk along the great-circle arc
/// between two tracks' embeddings, each waypoint snapped to a real track.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JourneyResponse {
    /// Which ends the discovery worker hasn't embedded yet. Per-end rather
    /// than one flag, so a message can name the end that is still waiting.
    #[serde(rename = "notAnalyzed")]
    pub not_analyzed: NotAnalyzed,
    /// Both seeds are included, so this *is* the queue. It can come up short
    /// when the library runs out of visible tracks — short is an answer, not
    /// an error.
    pub results: Vec<JourneyStop>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NotAnalyzed {
    pub start: bool,
    pub end: bool,
}

impl NotAnalyzed {
    pub fn any(&self) -> bool {
        self.start || self.end
    }

    /// Which end to name in a message, when one of them is holding things up.
    pub fn which(&self) -> &'static str {
        match (self.start, self.end) {
            (true, true) => "neither end has",
            (true, false) => "the starting track hasn't",
            (false, true) => "the destination hasn't",
            (false, false) => "both ends have",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JourneyStop {
    pub filepath: String,
    /// Position along the arc: 0 at the start, 1 at the destination.
    pub t: f64,
    /// How close this track sits to the waypoint it stands in for.
    pub similarity: f64,
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

impl JourneyStop {
    pub fn to_track(&self) -> Track {
        Track { filepath: self.filepath.clone(), metadata: self.metadata.clone() }
    }

    /// Artist and title where they exist, the filename otherwise — the same
    /// rule [`Track::display_name`] follows.
    pub fn metadata_display(&self) -> String {
        self.to_track().display_name()
    }
}

/// Bounds the server puts on the journey length, which counts both seeds.
pub const JOURNEY_MIN_LENGTH: u32 = 4;
pub const JOURNEY_MAX_LENGTH: u32 = 32;
pub const JOURNEY_DEFAULT_LENGTH: u32 = 14;

// ── Waveform ────────────────────────────────────────────────────────────────

/// `GET /api/v1/db/waveform` — the shape of a track, for drawing under the
/// progress bar.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WaveformResponse {
    /// Peak magnitude per bar, 0–255. One value, not a min/max pair: the
    /// server measures |sample| and bins by peak-of-peaks, so the shape is
    /// symmetric and a drawing only ever needs half of it.
    ///
    /// 800 of them, whatever the track's length — but nothing here depends
    /// on that. Every consumer resamples to however many columns it has, so
    /// a server that changes its mind costs nobody anything.
    pub waveform: Vec<u8>,
}

// ── Playlists ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PlaylistSummary {
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(fmt_duration(0.0), "0:00");
        assert_eq!(fmt_duration(59.6), "1:00");
        assert_eq!(fmt_duration(60.029), "1:00");
        assert_eq!(fmt_duration(3725.0), "62:05");
        assert_eq!(fmt_duration(f64::NAN), "--:--");
        assert_eq!(fmt_duration(-1.0), "--:--");
    }

    #[test]
    fn parses_live_file_explorer_metadata() {
        // Captured verbatim from a live mStream with `pullMetadata: true`,
        // trimmed to the fields this player reads. Two layers of `metadata`
        // is not a typo — see [`FileMetadata`] — and the inner one really is
        // null for the playlist file sitting in the same folder.
        let json = r#"{"path":"/library/3-1 Remixes/13 Horrible Remixes/","directories":[],
            "files":[{"type":"mp3","name":"01 - My Rave.mp3","metadata":{
            "filepath":"library/3-1 Remixes/13 Horrible Remixes/01 - My Rave.mp3",
            "metadata":{"artist":"3-1 Remixes","album":"13 Horrible Remixes By 3-1","track":1,
            "disk":null,"title":"My Rave (Nid & Sancy)","duration":238.655,"year":2009,
            "album-art":null,"rating":null,"play-count":null,"bpm":130,"musical-key":"G major",
            "genres":["Progressive Electronic"],"bitrate":271000,"format":"mp3",
            "sample-rate":44100,"channels":2,"bit-depth":null,"file-size":8087296,
            "track-total":13,"disc-total":1,"audio-hash":"51dbe5b4","bpm-source":"essentia",
            "has-lyrics":false,"has-synced-lyrics":false}}},
            {"type":"m3u","name":"13 Horrible Remixes.m3u","metadata":{
            "filepath":"library/3-1 Remixes/13 Horrible Remixes/13 Horrible Remixes.m3u",
            "metadata":null}}]}"#;
        let listing: DirListing = serde_json::from_str(json).unwrap();

        let tags = listing.files[0].metadata.as_ref().unwrap();
        assert_eq!(tags.filepath, "library/3-1 Remixes/13 Horrible Remixes/01 - My Rave.mp3");
        let meta = tags.metadata.as_ref().unwrap();
        assert_eq!(meta.duration, Some(238.655));
        assert_eq!(meta.bpm, Some(130));
        assert_eq!(meta.musical_key.as_deref(), Some("G major"));
        assert_eq!(meta.title.as_deref(), Some("My Rave (Nid & Sancy)"));
        assert_eq!(meta.year, Some(2009));

        // What the file is. Every one of these rode along on the listing and
        // was thrown away for months because the struct did not name it —
        // `bitrate` and `genres` were sitting in this very capture, unread.
        assert_eq!(meta.format.as_deref(), Some("mp3"));
        assert_eq!(meta.bitrate, Some(271000), "bits per second, not kilobits");
        assert_eq!(meta.sample_rate, Some(44100));
        assert_eq!(meta.channels, Some(2));
        assert_eq!(meta.bit_depth, None, "lossy, so there is none to give");
        assert_eq!(meta.file_size, Some(8087296));
        assert_eq!(meta.genres, vec!["Progressive Electronic".to_string()]);
        assert_eq!(meta.track_total, Some(13));
        assert_eq!(meta.disc_total, Some(1));

        // Present but empty: on disk, not in the database.
        assert!(listing.files[1].metadata.as_ref().unwrap().metadata.is_none());
    }

    #[test]
    fn parses_live_search_shape() {
        // Captured verbatim from a live mStream response.
        let json = r#"{"artists":[],"albums":[],"title":[],"files":[{"name":"testlib/sine-60s.mp3",
            "album_art_file":null,"filepath":"testlib/sine-60s.mp3","metadata":{"title":null,
            "artist":null,"album":null,"album-art":null,"year":null,"track":null,"disk":null,
            "duration":60.029,"rating":null,"bpm":null,"musical-key":"A minor","genres":[],
            "has-lyrics":false,"has-synced-lyrics":false,"replaygain-track":null}}],"lyrics":[]}"#;
        let r: SearchResults = serde_json::from_str(json).unwrap();
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].filepath, "testlib/sine-60s.mp3");
        assert_eq!(r.files[0].metadata.duration, Some(60.029));
        assert_eq!(r.files[0].metadata.musical_key.as_deref(), Some("A minor"));
        assert!(!r.is_empty());
    }

    #[test]
    fn tolerates_null_metadata_object() {
        let t: SearchTrack =
            serde_json::from_str(r#"{"name":"x","filepath":"lib/x.mp3","metadata":null}"#).unwrap();
        assert!(t.metadata.title.is_none());
    }

    #[test]
    fn tolerates_missing_and_unknown_fields() {
        // Only filepath is load-bearing; everything else may be absent, and
        // fields we don't model must not break parsing.
        let t: Track =
            serde_json::from_str(r#"{"filepath":"lib/a.mp3","brand_new_field":{"x":1}}"#).unwrap();
        assert_eq!(t.file_name(), "a.mp3");
        assert_eq!(t.display_name(), "a.mp3");
    }

    #[test]
    fn display_name_prefers_tags() {
        let t: Track = serde_json::from_str(
            r#"{"filepath":"lib/a.mp3","metadata":{"title":"Song","artist":"Band"}}"#,
        )
        .unwrap();
        assert_eq!(t.display_name(), "Band - Song");

        let t: Track =
            serde_json::from_str(r#"{"filepath":"lib/a.mp3","metadata":{"title":"Solo"}}"#).unwrap();
        assert_eq!(t.display_name(), "Solo");
    }

    #[test]
    fn parses_live_file_explorer_shape() {
        let json = r#"{"path":"/testlib/","files":[{"type":"flac","name":"noise-60s.flac"}],
            "directories":[{"name":"Terminal Test"}]}"#;
        let d: DirListing = serde_json::from_str(json).unwrap();
        assert_eq!(d.path, "/testlib/");
        assert_eq!(d.files[0].kind.as_deref(), Some("flac"));
        assert_eq!(d.directories[0].name, "Terminal Test");
        // The pre-`pullMetadata` shape, which is also what the fallback
        // request gets back: no `metadata` key at all.
        assert!(d.files[0].metadata.is_none());
    }

    #[test]
    fn parses_ping_transcode_defaults() {
        let json = r#"{"vpaths":["testlib"],"transcode":{"defaultCodec":"opus",
            "defaultBitrate":"96k"},"noMkdir":false,"noUpload":false}"#;
        let p: Ping = serde_json::from_str(json).unwrap();
        assert_eq!(p.vpaths, vec!["testlib"]);
        assert_eq!(p.transcode.unwrap().default_codec.as_deref(), Some("opus"));
    }

    #[test]
    fn the_sonic_pool_is_sent_whole_or_not_at_all() {
        let seeds = vec!["lib/a.mp3".to_string()];

        let body = serde_json::to_value(
            RandomSongRequest::default().with_sonic_pool(&seeds, Some(0.62)),
        )
        .unwrap();
        assert_eq!(body["similarTo"], serde_json::json!(["lib/a.mp3"]));
        assert_eq!(body["minSimilarity"], 0.62);

        // A threshold with no seed, or a seed with no threshold, would be a
        // 400 from the server's `.and(...)` — neither may reach the wire.
        for half in [
            RandomSongRequest::default().with_sonic_pool(&seeds, None),
            RandomSongRequest::default().with_sonic_pool(&[], Some(0.62)),
        ] {
            let body = serde_json::to_value(half).unwrap();
            assert!(body.get("similarTo").is_none(), "got {body}");
            assert!(body.get("minSimilarity").is_none(), "got {body}");
        }
    }

    #[test]
    fn more_seeds_than_the_server_averages_are_trimmed() {
        let seeds: Vec<String> = (0..20).map(|i| format!("lib/{i}.mp3")).collect();
        let request = RandomSongRequest::default().with_sonic_pool(&seeds, Some(0.5));
        assert_eq!(request.similar_to.len(), SIMILAR_TO_MAX, "the server caps this at 8");
        // The most recent picks are the ones worth anchoring to, and callers
        // pass them newest-first.
        assert_eq!(request.similar_to[0], "lib/0.mp3");
    }

    #[test]
    fn an_empty_request_sends_an_empty_body() {
        // Every filter is optional, and a filter we don't mean must not
        // appear — the server's waterfall branches on field presence.
        let body = serde_json::to_value(RandomSongRequest::default()).unwrap();
        assert_eq!(body, serde_json::json!({}));
    }

    #[test]
    fn reads_the_sonic_report_when_the_server_sends_one() {
        let json = r#"{"songs":[],"ignoreList":[4,5],
            "sonic":{"similarity":0.7213,"poolSize":1247}}"#;
        let r: RandomSongsResponse = serde_json::from_str(json).unwrap();
        let sonic = r.sonic.unwrap();
        assert_eq!(sonic.similarity, Some(0.7213));
        assert_eq!(sonic.pool_size, 1247);

        // And copes when it doesn't: non-sonic picks omit the whole object.
        let r: RandomSongsResponse =
            serde_json::from_str(r#"{"songs":[],"ignoreList":[]}"#).unwrap();
        assert!(r.sonic.is_none());
    }

    #[test]
    fn reads_discovery_flags_from_a_live_ping() {
        // Captured verbatim from demo.mstream.io, which runs the local
        // embedding index but neither p2p nor federation.
        let json = r#"{"vpaths":["library"],"transcode":{"defaultCodec":"opus",
            "defaultBitrate":"96k"},"noMkdir":true,"noUpload":true,"noFileModify":true,
            "allowYoutubeDownload":false,"discovery":true,"discoveryPath":true,
            "discoveryP2p":false,"federationDiscovery":false}"#;
        let caps = Capabilities::from(&serde_json::from_str::<Ping>(json).unwrap());
        assert_eq!(
            caps,
            Capabilities {
                discovery: true,
                discovery_path: true,
                discovery_p2p: false,
                federation_discovery: false,
            }
        );
        assert_eq!(caps.enabled_names(), vec!["similarity", "sonic journey"]);
    }

    #[test]
    fn a_server_that_reports_nothing_is_treated_as_having_nothing() {
        // An older mStream omits these fields entirely. Absent must mean off,
        // or the client probes routes that answer 403 — which is exactly the
        // failure feature detection exists to avoid.
        let caps = Capabilities::from(&serde_json::from_str::<Ping>(r#"{"vpaths":[]}"#).unwrap());
        assert_eq!(caps, Capabilities::default());
        assert!(caps.enabled_names().is_empty());
    }
}

// ── Setup-wizard shapes ──────────────────────────────────────────────────────

/// `GET api/v1/admin/iroh` — Quick Connect's state and pairing ticket.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IrohStatus {
    pub enabled: bool,
    pub available: bool,
    pub running: bool,
    pub online: bool,
    /// The composite pairing ticket the mobile apps scan. Present once the
    /// endpoint is up; the wizard renders it as a QR.
    pub qr: Option<String>,
}

/// `GET api/v1/scan/status` — the enrichment passes that run AFTER the
/// file scan (waveforms, album art, lyrics, discovery embeddings, audio
/// analysis, AcoustID), each with its live state.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ScanStatus {
    pub enrichment: Vec<EnrichmentPass>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EnrichmentPass {
    /// The pass kind: `waveform`, `albumart`, `lyrics`, `discovery`,
    /// `audioanalysis` or `acoustid`.
    pub pass: String,
    /// `idle` | `queued` | `running` | `disabled`.
    pub state: String,
    pub progress: Option<EnrichmentProgress>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EnrichmentProgress {
    pub attempted: u64,
    pub total: Option<u64>,
}

/// One row of `GET api/v1/scan/progress` — per-library scan state.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ScanProgressRow {
    pub vpath: String,
    /// Percent complete, when the server can estimate a total.
    pub pct: Option<u32>,
    pub scanned: u64,
}

// ── Torrents (admin) ─────────────────────────────────────────────────────────

/// `GET /admin/torrent`: the chosen client, the access policy, and every
/// client's saved non-secret fields (passwords are never returned).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TorrentParams {
    #[serde(default)]
    pub client: String,
    #[serde(rename = "enabledFor", default)]
    pub enabled_for: String,
    #[serde(default)]
    pub transmission: TorrentClientConfig,
    #[serde(default)]
    pub qbittorrent: TorrentClientConfig,
    #[serde(default)]
    pub deluge: TorrentClientConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TorrentClientConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default, deserialize_with = "null_default")]
    pub username: String,
    #[serde(rename = "rpcPath", default)]
    pub rpc_path: Option<String>,
    #[serde(rename = "useHttps", default)]
    pub use_https: bool,
    /// A host is saved — the daemon may still be unreachable.
    #[serde(default)]
    pub configured: bool,
}

/// `GET /admin/torrent/status`: a live probe of the saved credentials.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TorrentStatus {
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub configured: bool,
    #[serde(rename = "clientType", default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Transmission's RPC number; a number on the wire.
    #[serde(rename = "rpcVersion", default)]
    pub rpc_version: Option<serde_json::Value>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `POST /admin/torrent/<client>/test` and `/connect` — always HTTP 200; a
/// failed probe is `ok: false` with the daemon's sentence in `message`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProbeAnswer {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(rename = "rpcVersion", default)]
    pub rpc_version: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// `GET /admin/torrent/list` — never an HTTP error: an unreachable daemon
/// is an empty list with `error` set.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TorrentList {
    #[serde(default)]
    pub torrents: Vec<Torrent>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(rename = "clientType", default)]
    pub client_type: Option<String>,
}

/// One torrent as the daemon reports it, normalised by the server. The
/// rates are floats on the wire for Deluge, so they are floats here.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Torrent {
    #[serde(rename = "infoHash", default)]
    pub info_hash: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub status: String,
    /// 0.0 to 1.0.
    #[serde(default)]
    pub percent: f64,
    #[serde(rename = "rateDownload", default)]
    pub rate_download: f64,
    #[serde(rename = "rateUpload", default)]
    pub rate_upload: f64,
    #[serde(default)]
    pub eta: f64,
    #[serde(rename = "sizeBytes", default)]
    pub size_bytes: u64,
    #[serde(rename = "errorMessage", default, deserialize_with = "null_default")]
    pub error_message: String,
    #[serde(rename = "managedByMstream", default)]
    pub managed_by_mstream: bool,
    #[serde(rename = "managedBy", default)]
    pub managed_by: Option<String>,
    #[serde(rename = "addedAt", default)]
    pub added_at: f64,
}

/// `DELETE /admin/torrent/:infoHash`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RemoveAnswer {
    #[serde(default)]
    pub ok: bool,
    #[serde(rename = "daemonRemoveOk", default = "yes")]
    pub daemon_remove_ok: bool,
    #[serde(rename = "daemonRemoveError", default)]
    pub daemon_remove_error: Option<String>,
}

fn yes() -> bool {
    true
}

/// `GET /admin/torrent/vpath-access`: one row per library, keyed by name.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct VpathAccess {
    #[serde(rename = "clientType", default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub vpaths: std::collections::BTreeMap<String, AccessRow>,
    #[serde(default)]
    pub error: Option<String>,
}

/// The daemon-side view of one library: the confidence ladder
/// (verified / inferred / pending / unconfirmed), how it was learned.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AccessRow {
    #[serde(rename = "daemonPath", default)]
    pub daemon_path: Option<String>,
    #[serde(rename = "mstreamWritable", default)]
    pub mstream_writable: Option<bool>,
    #[serde(default, deserialize_with = "null_default")]
    pub confidence: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(rename = "lastProbedAt", default)]
    pub last_probed_at: Option<serde_json::Value>,
    #[serde(rename = "lastError", default)]
    pub last_error: Option<String>,
}

/// `GET /admin/torrent/path-templates`: each library's template plus the
/// server's variable list, suggestion and sample metadata for previews.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct PathTemplates {
    #[serde(default)]
    pub vpaths: std::collections::BTreeMap<String, TemplateRow>,
    #[serde(rename = "supportedVars", default)]
    pub supported_vars: Vec<String>,
    #[serde(rename = "suggestedTemplate", default)]
    pub suggested_template: String,
    #[serde(rename = "sampleMetadata", default)]
    pub sample_metadata: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TemplateRow {
    #[serde(default)]
    pub template: Option<String>,
}

/// `PUT /admin/torrent/path-templates/:vpath`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TemplateSaved {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(rename = "samplePath", default)]
    pub sample_path: Option<String>,
}

/// `POST /admin/torrent/seed-existing`: one file's outcome, always HTTP 200.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct SeedOutcome {
    #[serde(default)]
    pub ok: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub outcome: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub vpath: Option<String>,
    #[serde(rename = "matchedRoot", default)]
    pub matched_root: Option<String>,
    /// Where the daemon was told the content lives (`seeded`).
    #[serde(rename = "addedAt", default)]
    pub added_at: Option<String>,
    #[serde(rename = "mappingConfidence", default)]
    pub mapping_confidence: Option<String>,
    #[serde(rename = "padFilesTotal", default)]
    pub pad_files_total: Option<u32>,
    #[serde(rename = "padFilesPresent", default)]
    pub pad_files_present: Option<u32>,
    #[serde(rename = "clientType", default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub matched: Option<u32>,
    #[serde(default)]
    pub total: Option<u32>,
    #[serde(default)]
    pub missing: Vec<String>,
    #[serde(rename = "checkedVpaths", default)]
    pub checked_vpaths: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// One row of `GET /admin/users`, keyed by username there.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AdminUser {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub vpaths: Vec<String>,
    #[serde(rename = "allowTorrent", default)]
    pub allow_torrent: bool,
}

