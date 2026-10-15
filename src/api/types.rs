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

/// The ping's `transcode` field is the server's defaults object when
/// transcoding is on offer and the literal `false` when it is not — which
/// is every fresh install for the ten-odd seconds its ffmpeg bootstrap
/// takes, and for good wherever that download failed (mStream's
/// `src/api/server-info.js`: `transcode.isDownloaded() && …`). Reading the
/// boolean as `None` is the same answer the missing field already gets;
/// rejecting it failed the whole ping, and the setup wizard's first page
/// opened on "could not reach the server" on every platform.
fn transcode_or_off<'de, D: Deserializer<'de>>(d: D) -> Result<Option<TranscodeInfo>, D::Error> {
    match serde_json::Value::deserialize(d)? {
        v @ serde_json::Value::Object(_) => {
            serde_json::from_value(v).map(Some).map_err(serde::de::Error::custom)
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    #[serde(default)]
    pub vpaths: Vec<String>,
}

/// `GET /api/` — public server identification: no auth required, which the
/// route promises in so many words. `server` is the mStream version.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ServerInfo {
    /// The server's own version, e.g. `"5.13.2"`.
    #[serde(rename = "server")]
    pub version: Option<String>,
    #[serde(rename = "apiVersions")]
    pub api_versions: Vec<String>,
}

/// `GET /api/v1/ping` — the one-shot capability bootstrap.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ping {
    pub vpaths: Vec<String>,
    /// `None` when the server has no transcoding to offer — sent as the
    /// literal `false`, not as a missing field (see [`transcode_or_off`]).
    #[serde(deserialize_with = "transcode_or_off")]
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
    /// This server lists federated peers a local user may browse through
    /// its proxies (mStream #927): on only with federation enabled and at
    /// least one peer. An older build omits the key.
    #[serde(rename = "federationBrowse")]
    pub federation_browse: bool,
    /// This server hands its own devices direct access to its federated
    /// peers — the `access` route (mStream #943). Same shape as
    /// `federationBrowse`: the key's presence says the build has the route,
    /// its value that there is a peer to reach. Whether a given peer
    /// cooperates is answered per peer by the access route.
    #[serde(rename = "federationDirect")]
    pub federation_direct: bool,
    /// The Stats API's version (mStream 6.27): present when the server
    /// takes play sessions at `POST /api/v1/stats/plays`; absent on an
    /// older one, which counts the legacy thirty-second scrobble instead
    /// (play-reporting contract, clause 10).
    pub stats: Option<u32>,
    /// The history-seeded Discover page's generation (mStream's discovery
    /// plan §2.10): 1 = `POST /api/v1/discovery/taste` and its prefs. An
    /// integer like `stats`, so a later body key ships as 2 with no floor
    /// table here; an older server omits it (0).
    #[serde(rename = "discoveryTaste", deserialize_with = "null_default")]
    pub discovery_taste: u32,
    /// At least one discovery plug-in is switched on, so a recommendation
    /// row has actions (`GET /api/v1/discovery/plugins` lists them).
    #[serde(rename = "discoveryPlugins", deserialize_with = "null_default")]
    pub discovery_plugins: bool,
}

/// `GET /api/` — the layered payload (mStream #932): server-wide `features`
/// and the caller's own `user` block, which is where the ping's flags and
/// libraries live there. A peer answers this through its parent's proxy and
/// a guest over the peer's own tunnel, where `/api/v1/ping` is off the
/// allowlist; a [`Ping`] is composed from the two halves the way the server
/// composes its own.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LayeredInfo {
    /// The server's own version, e.g. `"6.28.0"` — what the version floors
    /// of the auto-dj contract (clause 50) are judged against.
    pub server: Option<String>,
    pub features: LayeredFeatures,
    pub user: LayeredUser,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LayeredFeatures {
    #[serde(deserialize_with = "transcode_or_off")]
    pub transcode: Option<TranscodeInfo>,
    pub discovery: bool,
    /// Whether the discovery scan has produced vectors yet (mStream #879).
    /// `None` on a server that does not say — which holds nothing back
    /// (auto-dj contract, clause 36).
    #[serde(rename = "discoveryReady")]
    pub discovery_ready: Option<bool>,
    #[serde(rename = "discoveryP2p")]
    pub discovery_p2p: bool,
    /// The Stats API's version, as the ping carries it.
    pub stats: Option<u32>,
    /// As [`Ping::discovery_taste`]: `/api/` carries it under `features`.
    #[serde(rename = "discoveryTaste", deserialize_with = "null_default")]
    pub discovery_taste: u32,
    #[serde(rename = "discoveryPlugins", deserialize_with = "null_default")]
    pub discovery_plugins: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LayeredUser {
    pub vpaths: Vec<String>,
    #[serde(rename = "noFileModify")]
    pub no_file_modify: bool,
    #[serde(rename = "noUpload")]
    pub no_upload: bool,
    #[serde(rename = "federationDiscovery")]
    pub federation_discovery: bool,
    #[serde(rename = "federationBrowse")]
    pub federation_browse: bool,
    #[serde(rename = "federationDirect")]
    pub federation_direct: bool,
}

impl From<LayeredInfo> for Ping {
    fn from(info: LayeredInfo) -> Ping {
        Ping {
            vpaths: info.user.vpaths,
            transcode: info.features.transcode,
            no_file_modify: info.user.no_file_modify,
            no_upload: info.user.no_upload,
            discovery: info.features.discovery,
            // The layered payload carries no `discoveryPath`: on any build
            // that serves `/api/` it equals `discovery` (the server says so).
            discovery_path: info.features.discovery,
            discovery_p2p: info.features.discovery_p2p,
            federation_discovery: info.user.federation_discovery,
            federation_browse: info.user.federation_browse,
            federation_direct: info.user.federation_direct,
            stats: info.features.stats,
            discovery_taste: info.features.discovery_taste,
            discovery_plugins: info.features.discovery_plugins,
        }
    }
}

/// `GET /api/v1/federation/peers/:id/access` on the wire: what a device
/// needs to reach a peer without the parent in the path — or the parent's
/// word that the peer will not mint (`direct: false`). Tolerant on purpose:
/// which fields arrived decides which of the three answers this is.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DirectAccessResponse {
    pub direct: bool,
    pub reason: Option<String>,
    #[serde(rename = "endpointTicket")]
    pub endpoint_ticket: Option<String>,
    #[serde(rename = "endpointId")]
    pub endpoint_id: Option<String>,
    #[serde(rename = "guestToken")]
    pub guest_token: Option<String>,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
    #[serde(rename = "directTicket")]
    pub direct_ticket: Option<String>,
}

/// A guest ticket a parent handed out for one of its peers: the `mstrfedg1:`
/// envelope the tunnel is dialled with, the guest JWT every request to the
/// peer carries, and the token's own times — read off its claims, which is
/// what the peer's wall judges by (contract clause 27).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTicket {
    pub ticket: String,
    pub guest_token: String,
    /// The peer's endpoint id, when the parent could read its ticket: how
    /// one peer listed by two parents would be told apart.
    pub endpoint_id: Option<String>,
    pub issued_at: Option<std::time::SystemTime>,
    pub expires_at: Option<std::time::SystemTime>,
}

/// The parent's answer to an access request, sorted into what the App does
/// with it: dial the peer, stay on the proxy for the session, or ask again
/// later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectAnswer {
    Granted(DirectTicket),
    /// `direct: false` — an older peer, or federation switched off there.
    Denied(String),
    /// The peer could not be reached for the mint, or the answer was not
    /// one of the shapes above.
    Failed(String),
}

/// One row of `GET /api/v1/federation/peers`: a peer this user may browse
/// through the server, by the id every proxy route keys on.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PeerListing {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PeerListingResponse {
    pub peers: Vec<PeerListing>,
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
    /// Peers to browse through this server (contract clause 20).
    pub federation_browse: bool,
    /// The parent offers direct access to its peers (the access route).
    pub federation_direct: bool,
    /// The server takes play sessions (play-reporting contract, clause 10).
    pub stats: bool,
    /// The Discover page's generation, kept as the integer the server sent:
    /// the room asks `>= 1` (discovery-page contract, clause 1), a later
    /// body key a higher floor.
    pub discovery_taste: u32,
    /// Recommendation rows have plug-in actions (discovery-page clause 63).
    pub discovery_plugins: bool,
}

impl Capabilities {
    /// The Discover room's gate (discovery-page contract, clause 1): the
    /// index on AND the taste route there. A peer session is the room's
    /// own rule, not the server's, so it is not read here.
    pub fn taste(&self) -> bool {
        self.discovery && self.discovery_taste >= 1
    }
}

impl From<&Ping> for Capabilities {
    fn from(ping: &Ping) -> Self {
        Capabilities {
            discovery: ping.discovery,
            discovery_path: ping.discovery_path,
            discovery_p2p: ping.discovery_p2p,
            federation_discovery: ping.federation_discovery,
            federation_browse: ping.federation_browse,
            federation_direct: ping.federation_direct,
            stats: ping.stats.is_some(),
            discovery_taste: ping.discovery_taste,
            discovery_plugins: ping.discovery_plugins,
        }
    }
}

/// `POST /api/v1/stats/plays` — the server's word on a batch: every id it
/// names is settled, whichever list it is on (a rejection means "drop it",
/// by contract).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PlaysAnswer {
    pub accepted: Vec<String>,
    pub duplicates: Vec<String>,
    pub rejected: Vec<RejectedPlay>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RejectedPlay {
    pub id: String,
    pub reason: String,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

    /// The bitrate in whole kbps — the server counts bits per second, and
    /// nobody reads a rip as 320.0 — or `None` when unknown or zero.
    pub fn kbps(&self) -> Option<u64> {
        self.bitrate.filter(|b| *b > 0).map(|b| (b as f64 / 1000.0).round() as u64)
    }

    /// The sample rate in kHz as a person writes it — "44.1", "48": the
    /// decimal only when there is something after it.
    pub fn khz_words(&self) -> Option<String> {
        self.sample_rate.filter(|r| *r > 0).map(|rate| {
            let khz = f64::from(rate) / 1000.0;
            if (khz.fract() * 10.0).round() == 0.0 { format!("{khz:.0}") } else { format!("{khz:.1}") }
        })
    }
}

/// A library track. `filepath` is the vpath-prefixed path used to build
/// `/media/...` URLs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// The title tag, else the file's name — what every surface prints
    /// for a track when it has one line for it.
    pub fn title_or_file(&self) -> &str {
        self.metadata.display_title().unwrap_or_else(|| self.file_name())
    }

    pub fn display_name(&self) -> String {
        match (self.metadata.artist.as_deref(), self.metadata.display_title()) {
            (Some(a), Some(t)) if !a.is_empty() => format!("{a} - {t}"),
            _ => self.title_or_file().to_string(),
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

/// A file size as a person reads it: whole megabytes past a hundred, a
/// decimal below that, kilobytes under one.
pub fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 100.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
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

/// `GET api/v1/admin/logs/recent?since=<seq>` — the tail of the server's main
/// log ring (mStream's `logger.js` `LogRing.read`), delta-polled by sequence
/// number like [`DiscoveryActivity`]. Entries come oldest first and only past
/// `since`; a `since` the server never reached (it restarted) answers the
/// whole ring with `last_seq` below the cursor. A `capacity` of 0 means the
/// ring is off and nothing will ever arrive.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct LogTail {
    pub entries: Vec<ActivityEntry>,
    pub last_seq: u64,
    pub capacity: u64,
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
    /// Tagged tracks only, so the waterfall never falls back to untagged
    /// picks when the windows return nothing (auto-dj contract, clause 21).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_bpm: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub musical_keys: Vec<String>,
    /// Keyed tracks only, even before the anchor is locked (clause 22).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_musical_key: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_artists: Vec<String>,
    /// The libraries switched OFF for the DJ (clause 20). Named by hand:
    /// the server spells it `ignoreVPaths`, which camelCase would not.
    #[serde(rename = "ignoreVPaths", skip_serializing_if = "Vec::is_empty")]
    pub ignore_vpaths: Vec<String>,
    /// 1–10. Omitted when zero, which the server also reads as "no floor".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_rating: Option<u32>,
    /// The track-length window in seconds; a rail is not sent, and
    /// `allowUnknownDuration` only rides beside a real bound (clause 20).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_unknown_duration: Option<bool>,
    /// How many songs this turn asks for, 1–25; absent at one, the
    /// pre-batch wire shape (clause 27).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
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

// ── The Discover page (taste) ───────────────────────────────────────────────
//
// mStream's history-seeded page (its docs/discovery-page-plan.md §3.1–§3.3;
// this repo's docs/ux-contracts/discovery-page.md). Every field is defensive:
// the route is new, and a reply missing a key it did not need to send must
// still fill the room. Gated on the ping's `discoveryTaste >= 1` — a flag,
// never a probe (plan §2.10).

/// `POST /api/v1/discovery/taste` — the body (plan §3.1). All optional,
/// and nothing is stored between calls: each call is answered from this
/// body alone (the route turned stateless on 2026-10-14; `remember`,
/// `filter` and `staleDays` are a 400 now and never sent).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteRequest {
    /// 7 | 30 | 90 | 0 (all time); the server's default is 30.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<u32>,
    /// Vpath-form paths, at most 12 (the server's `PIN_CAP`, its seed
    /// cap): songs the user picked (clause 14), or the Main answer's whole
    /// batch on the peers ask (clause 30).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pin: Vec<String>,
    /// Seed hashes left out of THIS call (clauses 13, 21), at most 200.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// `library` · `network` · `peers` — the two halves are asked apart
    /// (clause 30), so this is never empty when the App sends it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_artists_only: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_v_paths: Vec<String>,
    /// Caps the server applies to this call; `None` takes its defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<TasteLimit>,
}

/// The `limit` of a taste ask (plan §3.1): only what the room sets is sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteLimit {
    /// The batch size: the peers ask sets it to its pins, so the server
    /// adds no seed of its own to the Main answer's batch (clause 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeds: Option<u32>,
}

/// Where the answer's seeds came from (plan §2.2): the header's phrase
/// (contract clause 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeedSource {
    History,
    Counters,
    Random,
    Pinned,
    /// Missing, or a word a newer server added: the header says nothing it
    /// cannot stand behind.
    #[default]
    #[serde(other)]
    Unknown,
}

/// How a history seed's play ended (plan §2.1): "finished" or "listened to".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TasteOutcome {
    Completed,
    Stopped,
    /// A pin, the random seed, the counters — no play outcome to name.
    #[default]
    #[serde(other)]
    Other,
}

/// One seed the server chose, or one the user pinned.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasteSeed {
    pub hash: String,
    pub filepath: String,
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
    #[serde(deserialize_with = "null_default")]
    pub outcome: TasteOutcome,
    /// ISO time of the seed's latest play in the window.
    pub last_played: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub plays: u64,
    #[serde(deserialize_with = "null_default")]
    pub pinned: bool,
    /// False only when the server says so: a missing key is not a seed the
    /// scan skipped (the modal's "not analyzed yet" is a claim).
    #[serde(default = "yes")]
    pub analyzed: bool,
}

impl TasteSeed {
    pub fn to_track(&self) -> Track {
        Track { filepath: self.filepath.clone(), metadata: self.metadata.clone() }
    }
}

/// A saved exclusion the server could still resolve — the seeds modal's
/// DROPPED section (contract clause 13).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TasteExcluded {
    pub hash: String,
    pub filepath: String,
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TasteModel {
    pub id: Option<String>,
    pub version: Option<String>,
}

/// A song in the caller's own libraries (plan §2.5).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasteTrack {
    pub filepath: String,
    #[serde(deserialize_with = "null_default")]
    pub similarity: f64,
    /// The seed this row sounds like — the row's "sounds like" tooltip.
    pub seed_hash: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
    #[serde(deserialize_with = "null_default")]
    pub genre_tags: Vec<String>,
    pub play_count: Option<u64>,
    /// ISO; `None` is "never played".
    pub last_played: Option<String>,
}

impl TasteTrack {
    pub fn to_track(&self) -> Track {
        Track { filepath: self.filepath.clone(), metadata: self.metadata.clone() }
    }
}

/// An album card in the caller's libraries (plan §2.6): `/db/albums`' shape
/// (snake case, as that route sends it) plus the taste facts (camel case).
/// Its songs are left out of `library.tracks`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TasteAlbum {
    pub name: Option<String>,
    pub year: Option<i32>,
    pub album_art_file: Option<String>,
    /// Set when one credit covers the whole album.
    pub album_artist: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub artists: Vec<String>,
    /// Across all libraries — `/db/albums`' meaning, unchanged.
    #[serde(deserialize_with = "null_default")]
    pub track_count: u32,
    #[serde(deserialize_with = "null_default")]
    pub similarity: f64,
    /// The album's songs in the caller's libraries — the row's LENGTH
    /// (contract clause 51).
    #[serde(rename = "scopedTracks", deserialize_with = "null_default")]
    pub scoped_tracks: u32,
    #[serde(rename = "playedTracks", deserialize_with = "null_default")]
    pub played_tracks: u32,
    #[serde(rename = "lastPlayed")]
    pub last_played: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub tracks: Vec<String>,
}

impl TasteAlbum {
    /// The artist the Albums room's drill asks `album-songs` with: the one
    /// credit when there is one, else the first named.
    pub fn artist(&self) -> Option<&str> {
        self.album_artist
            .as_deref()
            .or_else(|| self.artists.first().map(String::as_str))
            .filter(|a| !a.is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TasteLibrary {
    #[serde(deserialize_with = "null_default")]
    pub tracks: Vec<TasteTrack>,
    #[serde(deserialize_with = "null_default")]
    pub albums: Vec<TasteAlbum>,
    #[serde(deserialize_with = "null_default")]
    pub capped: bool,
}

/// Where a lead or a peer's song was found: a p2p row names the endpoint,
/// a federation row the peer's id on this server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LeadPeer {
    pub id: Option<i64>,
    pub endpoint_id: Option<String>,
    pub name: Option<String>,
}

/// One row from another server: a p2p lead (the answer's `network` half,
/// which the play-now room no longer asks for) or a connected server's
/// song. The server stamps `source` on each so the row is a ready-to-send
/// recommendation (plan §2.7); `raw` keeps it exactly as it came, for a
/// resolve that forwards the row verbatim (the later Explore room's) — a
/// typed copy would drop what a newer server added. The
/// typed fields are read leniently off `raw`: a lead with an odd field
/// still lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TasteLead {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    /// Seconds, when the row carries one.
    pub duration: Option<f64>,
    pub similarity: f64,
    pub recording_mbid: Option<String>,
    pub seed_hash: Option<String>,
    /// `p2p` · `federation`.
    pub source: Option<String>,
    /// A federation row's path ON THE PEER — what the parent's stream proxy
    /// plays. A p2p lead has none.
    pub filepath: Option<String>,
    pub peer: LeadPeer,
    pub raw: serde_json::Value,
}

impl TasteLead {
    pub fn from_raw(raw: serde_json::Value) -> TasteLead {
        let text = |key: &str| {
            raw.get(key).and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
        };
        let number = |key: &str| {
            raw.get(key).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        };
        let peer = raw.get("peer");
        let peer = LeadPeer {
            id: peer.and_then(|p| p.get("id")).and_then(serde_json::Value::as_i64),
            endpoint_id: peer
                .and_then(|p| p.get("endpointId"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            name: peer.and_then(|p| p.get("name")).and_then(serde_json::Value::as_str).map(str::to_string),
        };
        TasteLead {
            artist: text("artist"),
            title: text("title"),
            album: text("album"),
            year: number("year").filter(|y| y.is_finite()).map(|y| y as i32),
            duration: number("duration").filter(|d| d.is_finite() && *d > 0.0),
            similarity: number("similarity").filter(|s| s.is_finite()).unwrap_or(0.0),
            recording_mbid: text("recordingMbid"),
            seed_hash: text("seedHash"),
            source: text("source"),
            filepath: text("filepath"),
            peer,
            raw,
        }
    }

    /// The row as a queue track: the peer's path and the row's own facts,
    /// which are also the play report's snapshot (play-reporting clause 7).
    /// `None` for a lead with no file to play.
    pub fn to_track(&self) -> Option<Track> {
        let filepath = self.filepath.clone()?;
        Some(Track {
            filepath,
            metadata: TrackMetadata {
                title: self.title.clone(),
                artist: self.artist.clone(),
                album: self.album.clone(),
                year: self.year,
                duration: self.duration,
                ..TrackMetadata::default()
            },
        })
    }
}

impl<'de> Deserialize<'de> for TasteLead {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(TasteLead::from_raw(serde_json::Value::deserialize(d)?))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NetworkSearched {
    #[serde(deserialize_with = "null_default")]
    pub peers: u32,
    #[serde(deserialize_with = "null_default")]
    pub tracks: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TasteNetwork {
    #[serde(deserialize_with = "null_default")]
    pub results: Vec<TasteLead>,
    #[serde(deserialize_with = "null_default")]
    pub searched: NetworkSearched,
}

/// One federated peer's part of the answer (contract clause 70).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TastePeerGroup {
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// This call's outcome, not a stored status.
    #[serde(deserialize_with = "null_default")]
    pub reachable: bool,
    /// Another sonic model: its vectors can't be compared with ours.
    #[serde(deserialize_with = "null_default")]
    pub mismatched: bool,
    /// ISO, from the server's peer table — may be stale.
    pub last_seen: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub results: Vec<TasteLead>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PeersSearched {
    #[serde(deserialize_with = "null_default")]
    pub peers: u32,
    #[serde(deserialize_with = "null_default")]
    pub unreachable: u32,
    #[serde(deserialize_with = "null_default")]
    pub mismatched: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TastePeers {
    #[serde(deserialize_with = "null_default")]
    pub peers: Vec<TastePeerGroup>,
    #[serde(deserialize_with = "null_default")]
    pub searched: PeersSearched,
}

/// `POST /api/v1/discovery/taste` — the answer (plan §3.1). `network` and
/// `peers` are `None` when the source was not asked, is not advertised for
/// this caller, or the seed is the server's random pick.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasteResponse {
    /// The window the seeds were read from (0 = all time).
    pub window: Option<u32>,
    /// The window asked for, when it held nothing and the server widened.
    pub widened_from: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    pub seed_source: SeedSource,
    /// The server's random seed is in use: nothing to seed from yet.
    #[serde(deserialize_with = "null_default")]
    pub fallback: bool,
    #[serde(deserialize_with = "null_default")]
    pub seeds: Vec<TasteSeed>,
    #[serde(deserialize_with = "null_default")]
    pub excluded: Vec<TasteExcluded>,
    /// Pins the server dropped (unknown or forbidden paths).
    #[serde(deserialize_with = "null_default")]
    pub dropped: u32,
    /// The scan has not run: the seeds are listed, nothing is ranked.
    #[serde(deserialize_with = "null_default")]
    pub not_analyzed: bool,
    pub model: Option<TasteModel>,
    pub library: Option<TasteLibrary>,
    pub network: Option<TasteNetwork>,
    pub peers: Option<TastePeers>,
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

// ── Torrents (the Add-torrent room; docs/ux-contracts/add-torrent.md) ───────

/// `GET /api/v1/torrent/preflight` — whether this server takes a torrent
/// from this user at all, and why not when it doesn't. There is no ping
/// flag for torrents: this answer is the gate.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TorrentPreflight {
    pub active: bool,
    #[serde(rename = "clientType")]
    pub client_type: Option<String>,
    /// The torrent client's name as people say it ("Transmission").
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "noUpload")]
    pub no_upload: bool,
    #[serde(rename = "userAllowed")]
    pub user_allowed: bool,
    /// The server's own words for what stands in the way, when something does.
    pub reason: Option<String>,
}

impl TorrentPreflight {
    /// The three global gates the record's screen checks: a client is
    /// selected, uploads are on, this user may. (Per-library mapping is
    /// enforced by `/torrent/add` itself.)
    pub fn ok(&self) -> bool {
        self.active && self.user_allowed && !self.no_upload
    }
}

/// `GET /api/v1/torrent/path-templates` — per-library destination
/// templates, `{{ARTIST}}/{{ALBUM}}` and friends.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentTemplates {
    pub vpaths: std::collections::HashMap<String, TorrentTemplateEntry>,
    #[serde(rename = "suggestedTemplate")]
    pub suggested_template: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentTemplateEntry {
    pub template: Option<String>,
}

/// `POST /api/v1/torrent/auto-detect` — the server's own guess at the
/// metadata. `confidence` is "high" or "low"; `ok: false` carries the
/// reason in `message`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentDetect {
    pub ok: bool,
    pub confidence: Option<String>,
    pub metadata: Option<TorrentDetectMeta>,
    pub message: Option<String>,
}

/// The detected fields. Numbers arrive as numbers (a year), so each is a
/// raw JSON value read through [`TorrentDetectMeta::text`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentDetectMeta {
    pub artist: Option<serde_json::Value>,
    pub album: Option<serde_json::Value>,
    pub year: Option<serde_json::Value>,
}

impl TorrentDetectMeta {
    /// A field as text — a string as itself, a number spelled out, nothing
    /// for null or absent.
    pub fn text(value: &Option<serde_json::Value>) -> String {
        match value {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }
}

/// `POST /api/v1/torrent/seed-existing` — are the torrent's files already
/// on disk? `outcome` is one of `seeded`, `already_in_daemon`,
/// `partial_match`, `no_match`, `match_unmapped`, `pad_files_missing`,
/// `invalid_torrent`, `daemon_error`; the other fields ride whichever
/// outcome needs them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SeedCheck {
    pub ok: bool,
    pub outcome: String,
    /// The torrent's own name, when the server could read it.
    pub name: Option<String>,
    /// `invalid_torrent` / `daemon_error`: the server's words.
    pub error: Option<String>,
    /// `seeded` / `match_unmapped` / `pad_files_missing`: which library.
    pub vpath: Option<String>,
    /// `pad_files_missing`: which client type cannot recreate the pads.
    #[serde(rename = "clientType")]
    pub client_type: Option<String>,
    /// `partial_match`: where some of the files already live.
    pub matches: Vec<SeedMatch>,
}

/// One place a partial match found files: a library-relative path, and
/// how much of the torrent is there.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SeedMatch {
    pub vpath: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    pub matched: Option<u64>,
    pub total: Option<u64>,
    pub missing: Option<u64>,
}

/// `POST /api/v1/torrent/add` — the torrent is with the client.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentAdded {
    pub ok: bool,
    pub name: Option<String>,
    #[serde(rename = "downloadPath")]
    pub download_path: Option<String>,
    /// The client already had it — its own wording, not a failure.
    #[serde(rename = "isDuplicate")]
    pub is_duplicate: bool,
    /// The add succeeded but the root-folder rename did not: a warning.
    #[serde(rename = "renameWarning")]
    pub rename_warning: Option<String>,
}

/// What a torrent submission carries: exactly one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentSource {
    File { name: String, bytes: Vec<u8> },
    Magnet(String),
}

/// The `/torrent/add` request, as the room assembles it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentAddRequest {
    pub vpath: String,
    /// Folders above the destination folder, `/`-joined; empty for none.
    pub sub_path: String,
    pub directory_name: String,
    pub rename_root: bool,
    pub source: TorrentSource,
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
    fn the_layered_payload_composes_a_ping_from_its_two_halves() {
        // What a rig server answered on `GET /api/` (2026-09-20): the flags
        // and the libraries sit under `user`, the transcode under `features`.
        let raw = r#"{
            "server": "6.28.0", "apiVersions": ["1"],
            "features": { "discoveryReady": false, "discovery": true, "discoveryP2p": false,
                          "transcode": { "defaultCodec": "opus", "defaultBitrate": "96k" },
                          "supportedAudioFiles": ["mp3"], "stats": 2 },
            "user": { "vpaths": ["demo"], "noMkdir": false, "noUpload": true, "noFileModify": false,
                      "federationDiscovery": false, "federationBrowse": true, "federationDirect": true,
                      "federationInbox": 0, "vpathMetaData": {} },
            "admin": true
        }"#;
        let info: LayeredInfo = serde_json::from_str(raw).unwrap();
        let ping = Ping::from(info);
        assert_eq!(ping.vpaths, vec!["demo".to_string()]);
        assert!(ping.federation_browse && ping.federation_direct && !ping.federation_discovery);
        assert_eq!(ping.stats, Some(2), "the Stats API's version rides `features`");
        assert!(ping.discovery && ping.discovery_path && !ping.discovery_p2p);
        assert!(ping.no_upload && !ping.no_file_modify);
        assert_eq!(ping.transcode.as_ref().and_then(|t| t.default_codec.clone()).as_deref(), Some("opus"));

        // A guest's answer over the peer's own tunnel: no transcode, one library.
        let guest: LayeredInfo =
            serde_json::from_str(r#"{"features":{"transcode":false},"user":{"vpaths":["demo"],"federationGuest":true}}"#)
                .unwrap();
        let ping = Ping::from(guest);
        assert_eq!(ping.vpaths, vec!["demo".to_string()]);
        assert!(ping.transcode.is_none());
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
    fn a_ping_with_transcode_off_is_still_a_ping() {
        // Captured from a live 6.26.0 bundle in its first seconds, before the
        // ffmpeg bootstrap had finished: `transcode` is the literal `false`,
        // and the setup wizard's first page said "could not reach the server"
        // over it (invalid type: boolean `false`, expected struct
        // TranscodeInfo). The boolean means what the missing field means.
        let json = r#"{"vpaths":[],"noMkdir":false,"noUpload":false,"noFileModify":false,
            "federationDiscovery":false,"federationBrowse":false,"federationDirect":false,
            "federationInbox":0,"vpathMetaData":{},"transcode":false,
            "supportedAudioFiles":{"mp3":true,"flac":true},"discovery":true,
            "discoveryP2p":false,"playlists":[],"discoveryPath":true}"#;
        let p: Ping = serde_json::from_str(json).unwrap();
        assert!(p.transcode.is_none());
        assert!(p.discovery && p.discovery_path && !p.discovery_p2p, "the rest still reads");
        assert!(Capabilities::from(&p).discovery_path, "and the capabilities lift out of it");

        // The other spellings of "nothing to offer" read the same way, and
        // the object still reads as the object.
        for json in [r#"{"transcode":null}"#, r#"{"transcode":true}"#, r#"{}"#] {
            let p: Ping = serde_json::from_str(json).unwrap();
            assert!(p.transcode.is_none(), "{json}");
        }
        let p: Ping = serde_json::from_str(r#"{"transcode":{"defaultCodec":"mp3"}}"#).unwrap();
        assert_eq!(p.transcode.unwrap().default_codec.as_deref(), Some("mp3"));
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
                federation_browse: false,
                federation_direct: false,
                stats: false,
                discovery_taste: 0,
                discovery_plugins: false,
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

    #[test]
    fn the_discover_rooms_flags_come_from_the_ping_and_the_layered_payload() {
        // mStream's plan §2.10: an integer generation, plus the plug-in flag
        // both payloads already carried.
        let ping: Ping = serde_json::from_str(
            r#"{"vpaths":[],"discovery":true,"discoveryTaste":1,"discoveryPlugins":true}"#,
        )
        .unwrap();
        let caps = Capabilities::from(&ping);
        assert_eq!((caps.discovery_taste, caps.discovery_plugins), (1, true));
        assert!(caps.taste());
        // The route without the index, or the index on an older server, is
        // no room (discovery-page clause 1).
        let older: Ping = serde_json::from_str(r#"{"discovery":true,"discoveryTaste":null}"#).unwrap();
        assert!(!Capabilities::from(&older).taste());
        let off: Ping = serde_json::from_str(r#"{"discovery":false,"discoveryTaste":2}"#).unwrap();
        assert!(!Capabilities::from(&off).taste());

        let info: LayeredInfo = serde_json::from_str(
            r#"{"features":{"discovery":true,"discoveryTaste":1,"discoveryPlugins":true},"user":{}}"#,
        )
        .unwrap();
        let caps = Capabilities::from(&Ping::from(info));
        assert!(caps.taste() && caps.discovery_plugins);
    }

    #[test]
    fn a_taste_answer_parses_defensively_and_keeps_each_lead_verbatim() {
        let row = serde_json::json!({
            "artist": "Lead", "title": "Far", "album": null, "year": "1999", "duration": 241.5,
            "similarity": 0.71, "recordingMbid": "MB-1", "exportId": "mbid:mb-1",
            "peer": { "endpointId": "e1", "name": "Sam's" }, "source": "p2p", "seedHash": "h1",
            "somethingNewer": { "kept": true }
        });
        let json = serde_json::json!({
            "seedSource": "counters", "widenedFrom": 30, "window": 0,
            "seeds": [{ "hash": "h1", "filepath": "m/a.mp3", "metadata": null }],
            "excluded": null,
            "library": { "filter": "someday", "tracks": [{ "filepath": "m/b.mp3", "metadata": { "title": "B" } }],
                         "albums": [{ "name": "Al", "artists": ["X", "Y"], "scopedTracks": 3 }] },
            "network": { "results": [row.clone()] },
            "peers": null
        });
        let answer: TasteResponse = serde_json::from_value(json).unwrap();
        assert_eq!(answer.seed_source, SeedSource::Counters);
        assert_eq!((answer.window, answer.widened_from), (Some(0), Some(30)));
        let seed = &answer.seeds[0];
        assert!(seed.analyzed, "a missing `analyzed` is not a claim the scan skipped it");
        assert_eq!(seed.outcome, TasteOutcome::Other);
        assert!(answer.excluded.is_empty() && answer.peers.is_none());
        let library = answer.library.unwrap();
        assert_eq!(library.tracks[0].to_track().title_or_file(), "B");
        assert_eq!(library.albums[0].artist(), Some("X"));
        assert_eq!(library.albums[0].scoped_tracks, 3);

        let lead = &answer.network.unwrap().results[0];
        assert_eq!(lead.raw, row, "the row round-trips unchanged for the resolve");
        assert_eq!((lead.title.as_deref(), lead.album.as_deref()), (Some("Far"), None));
        assert_eq!((lead.year, lead.duration), (Some(1999), Some(241.5)));
        assert_eq!(lead.peer.name.as_deref(), Some("Sam's"));
        assert_eq!(lead.peer.id, None);
        assert!(lead.to_track().is_none(), "a lead has no file to play");

        // A federation row: the peer's path and id, a track to queue.
        let fed = TasteLead::from_raw(serde_json::json!({
            "artist": "Them", "title": "Song", "duration": 180, "similarity": 0.9,
            "filepath": "music/s.mp3", "peer": { "id": 3, "name": "Attic" }, "source": "federation"
        }));
        let track = fed.to_track().unwrap();
        assert_eq!(track.filepath, "music/s.mp3");
        assert_eq!(track.metadata.duration, Some(180.0));
    }

    #[test]
    fn a_taste_body_sends_only_what_was_set() {
        let body = TasteRequest {
            window: Some(0),
            exclude: vec!["h1".into()],
            sources: vec!["library".into()],
            new_artists_only: Some(false),
            ..TasteRequest::default()
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({ "window": 0, "exclude": ["h1"], "sources": ["library"], "newArtistsOnly": false })
        );
        // No pins, no exclusions: neither key is sent (an empty list is
        // nothing to say).
        let bare = TasteRequest { window: Some(30), ..TasteRequest::default() };
        assert_eq!(serde_json::to_value(&bare).unwrap(), serde_json::json!({ "window": 30 }));
        // The peers ask's batch: its pins and a seed limit of their count.
        let peers = TasteRequest {
            pin: vec!["a.mp3".into(), "b.mp3".into()],
            limit: Some(TasteLimit { seeds: Some(2) }),
            ..TasteRequest::default()
        };
        assert_eq!(
            serde_json::to_value(&peers).unwrap(),
            serde_json::json!({ "pin": ["a.mp3", "b.mp3"], "limit": { "seeds": 2 } })
        );
    }

    #[test]
    fn the_log_tail_parses_the_routes_json() {
        // The shape of `LogRing.read`: raw winston levels, ISO times, and a
        // stack trace riding inside the message after a newline. Fields the
        // player does not name are ignored rather than refused.
        let json = r#"{"entries":[
            {"seq":41,"t":"2026-10-02T09:15:04.120Z","level":"info","message":"scan finished"},
            {"seq":42,"t":"2026-10-02T09:15:05.000Z","level":"error",
             "message":"Error: ENOENT\n    at open (fs.js:1:1)","meta":{"x":1}}],
            "lastSeq":42,"capacity":1000,"extra":true}"#;
        let tail: LogTail = serde_json::from_str(json).unwrap();
        assert_eq!(tail.last_seq, 42);
        assert_eq!(tail.capacity, 1000);
        assert_eq!(tail.entries.len(), 2);
        assert_eq!(tail.entries[0].seq, 41);
        assert_eq!(tail.entries[0].t, "2026-10-02T09:15:04.120Z");
        assert_eq!(tail.entries[1].level, "error");
        assert_eq!(tail.entries[1].message, "Error: ENOENT\n    at open (fs.js:1:1)");

        // An older or trimmed answer without the ring's size reads as off.
        let bare: LogTail = serde_json::from_str(r#"{"entries":[],"lastSeq":7}"#).unwrap();
        assert_eq!(bare.capacity, 0);
        assert_eq!(bare.last_seq, 7);
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

/// One row of `GET /admin/users`, keyed by username there. The four
/// permission flags default the way the server creates a user (folders
/// and uploads on, file changes on, server audio off) so an older server
/// that omits one reads as it behaves.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AdminUser {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub vpaths: Vec<String>,
    #[serde(rename = "allowMkdir", default = "yes")]
    pub allow_mkdir: bool,
    #[serde(rename = "allowUpload", default = "yes")]
    pub allow_upload: bool,
    #[serde(rename = "allowFileModify", default = "yes")]
    pub allow_file_modify: bool,
    #[serde(rename = "allowServerAudio", default)]
    pub allow_server_audio: bool,
    #[serde(rename = "allowTorrent", default)]
    pub allow_torrent: bool,
}

impl Default for AdminUser {
    fn default() -> Self {
        AdminUser {
            admin: false,
            vpaths: Vec::new(),
            allow_mkdir: true,
            allow_upload: true,
            allow_file_modify: true,
            allow_server_audio: false,
            allow_torrent: false,
        }
    }
}


// ── Stats API v2 (/api/v1/stats/*), mStream 6.27 ─────────────────────────────
// The read side of the server's listening log, for `mstream-player stats`.
// A `track` is the server's metadata object in either shape the webapp
// handles (flat, or `{filepath, metadata}`), so it rides as a Value and the
// stats page reads a title and artist out of it.

/// The resolved range a stats read covered.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct StatsPeriod {
    pub label: String,
    pub from: String,
    pub to: String,
    pub tz: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct OriginSlice {
    pub plays: u64,
    #[serde(rename = "listenedMs")]
    pub listened_ms: u64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Origins {
    pub local: OriginSlice,
    pub peers: OriginSlice,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct StreakDays {
    pub current: u32,
    pub longest: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Sessions {
    pub count: u64,
    #[serde(rename = "avgMs")]
    pub avg_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct TopDay {
    pub date: String,
    pub plays: u64,
    #[serde(rename = "listenedMs")]
    pub listened_ms: u64,
}

/// `GET /stats/summary`: the period's totals as data.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatsSummary {
    pub period: StatsPeriod,
    pub events: u64,
    pub plays: u64,
    #[serde(rename = "uniqueTracks")]
    pub unique_tracks: u64,
    #[serde(rename = "uniqueArtists")]
    pub unique_artists: u64,
    #[serde(rename = "uniqueAlbums")]
    pub unique_albums: u64,
    #[serde(rename = "listenedMs")]
    pub listened_ms: u64,
    pub skips: u64,
    #[serde(rename = "skipRate")]
    pub skip_rate: Option<f64>,
    #[serde(rename = "completionRate")]
    pub completion_rate: Option<f64>,
    pub discoveries: u64,
    #[serde(rename = "libraryCoveragePct")]
    pub library_coverage_pct: Option<f64>,
    pub sessions: Sessions,
    #[serde(rename = "streakDays")]
    pub streak_days: StreakDays,
    #[serde(rename = "topDay")]
    pub top_day: Option<TopDay>,
    #[serde(rename = "peakHour")]
    pub peak_hour: Option<i32>,
    #[serde(rename = "peakWeekday")]
    pub peak_weekday: Option<i32>,
    pub origins: Origins,
}

/// One bucket of `GET /stats/timeseries`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct TimeBucket {
    pub bucket: String,
    pub plays: u64,
    pub skips: u64,
    #[serde(rename = "listenedMs")]
    pub listened_ms: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatsTimeseries {
    pub bucket: String,
    pub items: Vec<TimeBucket>,
}

/// One ranked row of `GET /stats/top`. A track row carries `track` (and,
/// for a federated peer's track, `origin: "peer"` with `peer_name`); a
/// group row (artists/albums/genres) carries `name`, `tracks`, and for an
/// album its `artist`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TopItem {
    pub rank: u32,
    pub plays: u64,
    #[serde(rename = "listenedMs")]
    pub listened_ms: u64,
    pub share: f64,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub tracks: Option<u64>,
    #[serde(rename = "lastPlayed")]
    pub last_played: Option<String>,
    pub origin: Option<String>,
    #[serde(rename = "peerName")]
    pub peer_name: Option<String>,
    pub track: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatsTop {
    pub entity: String,
    pub metric: String,
    pub items: Vec<TopItem>,
}

/// One play of `GET /stats/history`, newest first.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HistoryItem {
    pub id: String,
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "playedMs")]
    pub played_ms: u64,
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<u64>,
    /// `completed` · `skipped` · `stopped`.
    pub outcome: String,
    pub counted: bool,
    /// `legacy` marks a play recorded through the old scrobble route.
    pub source: Option<String>,
    pub client: Option<String>,
    /// `local` · `peer`.
    pub origin: String,
    #[serde(rename = "peerName")]
    pub peer_name: Option<String>,
    pub track: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatsHistory {
    pub items: Vec<HistoryItem>,
    pub next: Option<String>,
}

/// One selectable period from `GET /stats/periods`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PeriodOption {
    pub period: String,
    pub offset: i32,
    pub label: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatsPeriods {
    pub earliest: Option<String>,
    pub latest: Option<String>,
    pub periods: Vec<PeriodOption>,
}
