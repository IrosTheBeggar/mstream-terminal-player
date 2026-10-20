//! The SD-card transfer's half of the mStream client (the transfer plan's
//! part A6): the paged sync manifest with its ETag, full metadata in chunks
//! under the server's body cap, embeddings eight paths at a time, the
//! recursive listing a peer offers where it serves no manifest, covers in
//! their three sizes, and backdated plays from a card's log. The files'
//! bytes are [`fetch`]'s.
//!
//! For a federation peer (B9): the parent's peers with their iroh
//! identities ([`TransferPeer`]), a client of the parent aimed at one of
//! them ([`Client::for_peer`]), the peer's `/api/` with the allowance its
//! key's holder may read once mStream gives it ([`PeerApi`]), a folder's
//! own entries, and where mStream keeps its own ffmpeg.
//!
//! Every call here reads its answer through `exchange`, which adds the body
//! to the client's meter and carries the copy purpose when the client was
//! built with them. The rest is the client's own plumbing: the
//! token in its header, the peer rewrite, the loopback token.
//!
//! Async with sync twins, as the rest of the client. Native only.

// The sync engine (the plan's A8 and B1) is this module's caller and lands
// after it; until then only the tests call in.
#![cfg_attr(not(test), allow(dead_code))]

pub mod fetch;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use super::types::{DirListing, LayeredInfo, PlaysAnswer, TranscodeInfo};
use super::urls::{self, ArtSize, TransferBitrate, TransferCodec};
use super::{ApiError, Client, refused, wait};

/// The manifest's page ceiling (`MANIFEST_MAX_LIMIT` in mStream's
/// `sync.js`); a request that names none gets 2000.
pub const MANIFEST_PAGE_MAX: u32 = 5000;
/// What one `metadata/batch` body may weigh. The server's `express.json`
/// stops at `maxRequestSize`, 1 MB unless an admin changed it. Half leaves
/// room for one who lowered it a little; a 413 halves the chunk anyway.
pub const BATCH_BODY_BUDGET: usize = 512 * 1024;
/// And how many paths, whatever they weigh: the answer runs to about 1.5 KB
/// a track, and a peer's proxy gives up 15 s after the request.
pub const BATCH_MAX_PATHS: usize = 2000;
/// `discovery/local/embeddings` takes 1-8 paths (mStream's `discovery.js`).
pub const EMBEDDINGS_PER_CALL: usize = 8;
/// Its 404 for a path it cannot resolve or this user cannot see.
const TRACK_NOT_FOUND: &str = "Track not found";
/// `stats/plays` takes 1-200 plays a call (its `MAX_BATCH`).
pub const PLAYS_PER_CALL: usize = 200;
/// A listing call's ceiling. A manifest page of 5,000 entries is a few MB,
/// longer than the client's 20 s over a relay or a slow uplink.
const LISTING_TIMEOUT: Duration = Duration::from_secs(120);

// ── Plumbing ────────────────────────────────────────────────────────────────

/// One answer whole: its status, its headers, its body.
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Client {
    /// Add every body byte the transfer's calls read to `meter` — the
    /// files, the JSON, the covers and the probes alike, as a federated
    /// peer's daily quota counts them all. One meter may be shared by the
    /// clients of one peer.
    pub fn with_meter(mut self, meter: Option<Arc<AtomicU64>>) -> Self {
        self.meter = meter;
        self
    }

    /// Mark the transfer's requests as a copy (`X-mStream-Purpose: copy`).
    /// For a peer reached directly with a guest token: a key that forbids
    /// copies then answers 403, and the transfer stops for that peer
    /// instead of copying past its owner's choice. The parent's proxies
    /// drop the header, so on a peer client it says nothing.
    pub fn copying(mut self, copying: bool) -> Self {
        self.copying = copying;
        self
    }

    /// A client of this one's server aimed at its federation peer `id`:
    /// reads through the parent's browse proxy, bytes through its stream
    /// proxy, art through its art proxy. It shares this one's connections,
    /// token and loopback token, and has no meter and no copy purpose until
    /// it is given them.
    pub fn for_peer(&self, id: i64) -> Client {
        Client {
            http: self.http.clone(),
            base: self.base.clone(),
            token: self.token.clone(),
            peer: Some(id),
            local_token: self.local_token.clone(),
            plain_listings: AtomicBool::new(false),
            no_waveforms: AtomicBool::new(false),
            meter: None,
            copying: false,
        }
    }

    fn count(&self, bytes: usize) {
        if let Some(meter) = &self.meter {
            meter.fetch_add(bytes as u64, Ordering::Relaxed);
        }
    }

    /// The request every transfer call starts from: the token in the header
    /// (never the URL, so it stays out of the server's logs, even for
    /// `/media`), and the copy purpose when it was asked for.
    fn transfer_request(&self, method: Method, url: &str) -> RequestBuilder {
        let mut req = self.http.request(method, url);
        if let Some(token) = &self.token {
            req = req.header("x-access-token", token);
        }
        if self.copying {
            req = req.header("X-mStream-Purpose", "copy");
        }
        req
    }

    /// One JSON call of the transfer's, answered whole and in any status:
    /// [`Client::send_within`]'s URL and ceiling, with the meter counting
    /// the body. `headers` are sent as given; a value that cannot be a
    /// header is left out rather than failing the call.
    async fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
        headers: &[(&'static str, String)],
    ) -> Result<Reply, ApiError> {
        let url = self.endpoint(path)?;
        let mut req = self.transfer_request(method, url.as_str()).timeout(LISTING_TIMEOUT);
        for (name, value) in headers {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(value) {
                req = req.header(*name, value);
            }
        }
        if let Some(body) = body {
            let text = serde_json::to_string(body)
                .map_err(|e| ApiError::Config(format!("could not encode request: {e}")))?;
            req = req.header("Content-Type", "application/json").body(text);
        }
        let resp = req.send().await.map_err(unreached)?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.map_err(unreached)?.to_vec();
        self.count(body.len());
        Ok(Reply { status, headers, body })
    }
}

/// A transport failure, in words that leave its URL out: a tunnel's
/// carries the tunnel's loopback token (`__lt`), and the words go into a
/// run's report.
fn unreached(e: reqwest::Error) -> ApiError {
    ApiError::Network(e.without_url().to_string())
}

/// A JSON answer read as `T`, a refusal as the client maps one.
fn decode<T: DeserializeOwned>(reply: &Reply, path: &str) -> Result<T, ApiError> {
    if let Some(refusal) = refused(reply.status, &String::from_utf8_lossy(&reply.body), path) {
        return Err(refusal);
    }
    serde_json::from_slice(&reply.body)
        .map_err(|e| ApiError::Decode { endpoint: path.to_string(), message: e.to_string() })
}

// ── Lenient fields ──────────────────────────────────────────────────────────
//
// One odd value must not fail a page of 5,000 entries, and with it a sync
// that has run for an hour. SQLite keeps whatever was written (a REAL
// `modified` survives from before mStream's V24), so the numbers are read
// whole however they came, and anything else is `None`. Two fields of an
// entry are required, and an entry without one fails its page: the path
// and the row id ([`ManifestEntry`]).

/// An integer, a float rounded, or a numeric string; else `None`.
fn whole<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<i64>,
{
    let n = match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64().filter(|f| f.is_finite() && f.abs() < 9.0e18).map(|f| f.round() as i64)
        }),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    Ok(n.and_then(|n| T::try_from(n).ok()))
}

/// A row id however it came, an integer, a whole float or a numeric
/// string; anything else fails the entry. Never rounded: a fractional id
/// read as its neighbour would take that row's place in a name collision.
fn row_id<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let id = match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e18).map(|f| f as i64)),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    id.ok_or_else(|| serde::de::Error::custom("an entry's id is not a whole number"))
}

/// A number or a numeric string, as a float; else `None`.
fn real<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|f| f.is_finite()))
}

/// A string, or a number written out; else `None`.
fn textual<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// A list of strings, its other members and a `null` dropped.
fn strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => {
            items.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        }
        _ => Vec::new(),
    })
}

/// `true` only for a real `true` (the server sends `!!value`).
fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(matches!(serde_json::Value::deserialize(d)?, serde_json::Value::Bool(true)))
}

// ── Shapes ──────────────────────────────────────────────────────────────────

/// A track's metadata as `db/metadata/batch` renders it (mStream's
/// `renderMetadataObj`). The manifest's lite block is a strict subset under
/// the same keys, so one type reads both: what the lite block leaves out
/// (`sample-rate`, `channels`, `bit-depth`, `bpm-source`, the totals,
/// `composer`, the ids) is `None` there.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Metadata {
    #[serde(deserialize_with = "textual")]
    pub title: Option<String>,
    /// The primary artist's name.
    #[serde(deserialize_with = "textual")]
    pub artist: Option<String>,
    /// The ARTIST tag as written ("A feat. B").
    #[serde(rename = "artist-display", deserialize_with = "textual")]
    pub artist_display: Option<String>,
    /// Performer credits in tag order. Always `[artist]` in the manifest,
    /// which skips the credits pass (mStream's `sync.js`).
    #[serde(deserialize_with = "strings")]
    pub artists: Vec<String>,
    #[serde(deserialize_with = "textual")]
    pub composer: Option<String>,
    #[serde(deserialize_with = "textual")]
    pub album: Option<String>,
    #[serde(deserialize_with = "whole")]
    pub track: Option<u32>,
    #[serde(deserialize_with = "whole")]
    pub disk: Option<u32>,
    #[serde(rename = "track-total", deserialize_with = "whole")]
    pub track_total: Option<u32>,
    #[serde(rename = "disc-total", deserialize_with = "whole")]
    pub disc_total: Option<u32>,
    #[serde(deserialize_with = "whole")]
    pub year: Option<i32>,
    /// Seconds.
    #[serde(deserialize_with = "real")]
    pub duration: Option<f64>,
    /// The server's cache file name for the cover (`<md5>.<ext>`).
    #[serde(rename = "album-art", deserialize_with = "textual")]
    pub album_art: Option<String>,
    /// 0-10, the caller's own.
    #[serde(deserialize_with = "whole")]
    pub rating: Option<u32>,
    #[serde(rename = "play-count", deserialize_with = "whole")]
    pub play_count: Option<u64>,
    #[serde(rename = "last-played", deserialize_with = "textual")]
    pub last_played: Option<String>,
    /// dB. The server writes `|| null`, so a true 0 dB arrives as `None`.
    #[serde(rename = "replaygain-track", deserialize_with = "real")]
    pub replaygain_track: Option<f64>,
    #[serde(deserialize_with = "real")]
    pub bpm: Option<f64>,
    /// "A minor", "C# major": Camelot is the client's to work out.
    #[serde(rename = "musical-key", deserialize_with = "textual")]
    pub musical_key: Option<String>,
    /// `tag` (the file's own) or `essentia` (the server's analysis).
    #[serde(rename = "bpm-source", deserialize_with = "textual")]
    pub bpm_source: Option<String>,
    #[serde(deserialize_with = "strings")]
    pub genres: Vec<String>,
    /// Bits per second.
    #[serde(deserialize_with = "whole")]
    pub bitrate: Option<u64>,
    #[serde(deserialize_with = "textual")]
    pub format: Option<String>,
    #[serde(rename = "sample-rate", deserialize_with = "whole")]
    pub sample_rate: Option<u32>,
    #[serde(deserialize_with = "whole")]
    pub channels: Option<u16>,
    #[serde(rename = "bit-depth", deserialize_with = "whole")]
    pub bit_depth: Option<u16>,
    #[serde(rename = "file-size", deserialize_with = "whole")]
    pub file_size: Option<u64>,
    #[serde(deserialize_with = "textual")]
    pub hash: Option<String>,
    #[serde(rename = "audio-hash", deserialize_with = "textual")]
    pub audio_hash: Option<String>,
    #[serde(rename = "created-at", deserialize_with = "textual")]
    pub created_at: Option<String>,
    /// The file's mtime, epoch ms.
    #[serde(deserialize_with = "whole")]
    pub modified: Option<i64>,
    #[serde(deserialize_with = "textual")]
    pub source: Option<String>,
    #[serde(rename = "has-lyrics", deserialize_with = "flag")]
    pub has_lyrics: bool,
    #[serde(rename = "has-synced-lyrics", deserialize_with = "flag")]
    pub has_synced_lyrics: bool,
    #[serde(rename = "musicbrainz-recording-id", deserialize_with = "textual")]
    pub musicbrainz_recording_id: Option<String>,
    #[serde(rename = "musicbrainz-track-id", deserialize_with = "textual")]
    pub musicbrainz_track_id: Option<String>,
    #[serde(rename = "acoustid-id", deserialize_with = "textual")]
    pub acoustid_id: Option<String>,
    #[serde(deserialize_with = "textual")]
    pub isrc: Option<String>,
}

/// One row of the sync manifest (mStream's `sync.js`, `toManifestEntry`):
/// the identity a mirror needs to tell a file unchanged, changed or moved
/// without touching it, and the lite metadata.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ManifestEntry {
    /// `vpath/rel`. Required: an entry without one could only be read as a
    /// deletion, so a page that has one fails instead.
    pub filepath: String,
    /// The track's row id: the order pages walk in, and on a name collision
    /// the lower one keeps the name (the card contract's 2.8.3). Required
    /// too: read however it came ([`row_id`]), but an entry without one
    /// fails its page, as one without a path does.
    #[serde(deserialize_with = "row_id")]
    pub id: i64,
    #[serde(default, deserialize_with = "null_or_default")]
    pub metadata: Metadata,
    #[serde(default, rename = "file-size", deserialize_with = "whole")]
    pub file_size: Option<u64>,
    /// The file's mtime, epoch ms.
    #[serde(default, deserialize_with = "whole")]
    pub modified: Option<i64>,
    /// A full MD5 below the server's 25 MB threshold, a sampled digest above
    /// it (`hash-v` 2): not a checksum of a big file.
    #[serde(default, deserialize_with = "textual")]
    pub hash: Option<String>,
    #[serde(default, rename = "audio-hash", deserialize_with = "textual")]
    pub audio_hash: Option<String>,
    #[serde(default, rename = "hash-v", deserialize_with = "whole")]
    pub hash_v: Option<u32>,
    #[serde(default, deserialize_with = "textual")]
    pub format: Option<String>,
    #[serde(default, rename = "album-id", deserialize_with = "whole")]
    pub album_id: Option<i64>,
    #[serde(default, rename = "artist-id", deserialize_with = "whole")]
    pub artist_id: Option<i64>,
    #[serde(default, rename = "created-at", deserialize_with = "textual")]
    pub created_at: Option<String>,
}

fn null_or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// One page as the server sends it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ManifestPage {
    /// The caller's visible set's revision, also sent as the ETag: count,
    /// max id, max mtime and the tracks with art, prefixed with the library
    /// ids. It misses a file changed in place whose mtime is not the newest
    /// (the card contract's part 4.2, A10), hence the periodic full read.
    pub revision: String,
    /// A scan is running: a path missing now may be back on the next page
    /// or the next run, so it is not a deletion.
    #[serde(deserialize_with = "flag")]
    pub scanning: bool,
    /// The cursor for the next page; `None` on the last.
    #[serde(deserialize_with = "whole")]
    pub next: Option<i64>,
    pub entries: Vec<ManifestEntry>,
}

/// One page's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum ManifestAnswer {
    /// A 304 to the first page's `If-None-Match`: probably unchanged.
    NotModified,
    Page(ManifestPage),
}

/// A whole manifest walk.
#[derive(Debug, Clone, PartialEq)]
pub enum Manifest {
    /// The revision sent is still the server's. Only "probably": the
    /// revision misses some in-place changes, so a caller still reads it in
    /// full now and then (the card contract's 2.10.2: every 7th day or 10th
    /// run).
    NotModified,
    Listed(Listing),
}

/// Every entry of the libraries asked for, in id order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    /// The FIRST page's revision, to send as `If-None-Match` next time. A
    /// library that moved during the walk then reads as changed.
    pub revision: String,
    /// Some page said a scan was running.
    pub scanning: bool,
    /// The revision changed between pages: rows came or went mid-walk.
    pub moved: bool,
    pub entries: Vec<ManifestEntry>,
}

impl Listing {
    /// Whether a path missing from the listing may be read as deleted on
    /// the server: never during a scan (mStream's `sync.js` says so), nor
    /// when the library moved while it was read.
    pub fn deletions_trusted(&self) -> bool {
        !self.scanning && !self.moved
    }
}

/// One `metadata/batch` value: the path as the server normalised it, and
/// its metadata, `null` for a path it could not resolve or this user
/// cannot see.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct BatchRow {
    metadata: Option<Metadata>,
}

/// One row of `GET /api/v1/federation/peers` as a transfer reads it: the
/// peer's id in the parent's list, its name there, and its iroh identity (a
/// public key, never a credential), which tells a peer paired again under
/// an id another peer had from the one a card's files came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TransferPeer {
    #[serde(deserialize_with = "whole_or_zero")]
    pub id: i64,
    #[serde(deserialize_with = "textual")]
    pub name: Option<String>,
    #[serde(rename = "endpointId", deserialize_with = "textual")]
    pub endpoint_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct TransferPeers {
    peers: Vec<TransferPeer>,
}

fn whole_or_zero<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    whole::<D, i64>(d).map(|n| n.unwrap_or(0))
}

/// A federation key's allowance on the peer, as its holder reads it (the
/// card transfer's batch of mStream changes: `user.federationAllowance` in
/// the peer's `/api/`). The key's every user and device share it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Allowance {
    /// The key's daily quota in MiB (mStream's MB); 0: none.
    #[serde(rename = "dailyMb", deserialize_with = "whole")]
    pub daily_mb: Option<u64>,
    /// What the key has pulled today, and what is left of the quota: None
    /// with no quota (mStream's `null`).
    #[serde(rename = "usedTodayBytes", deserialize_with = "whole")]
    pub used_today: Option<u64>,
    #[serde(rename = "remainingTodayBytes", deserialize_with = "whole")]
    pub remaining_today: Option<u64>,
    /// Seconds to the quota's reset, the peer's own UTC midnight.
    #[serde(rename = "resetsInSeconds", deserialize_with = "whole")]
    pub resets_in: Option<u64>,
    /// False: the key refuses a request that says it is a copy.
    #[serde(rename = "allowCopies", deserialize_with = "boolean")]
    pub allow_copies: Option<bool>,
    #[serde(rename = "maxStreams", deserialize_with = "whole")]
    pub max_streams: Option<u32>,
}

/// A real `true` or `false`; else `None`.
fn boolean<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    Ok(serde_json::Value::deserialize(d)?.as_bool())
}

/// A peer's `/api/` as a transfer reads it: the layered payload, and the
/// allowance of the key it is read with, when the peer gives one.
#[derive(Debug, Clone, Default)]
pub struct PeerApi {
    pub info: LayeredInfo,
    pub allowance: Option<Allowance>,
}

/// The embedding model an index was built with. Its licence travels with
/// it (the default, `effnet-discogs`, is CC BY-NC-SA 4.0), which decides
/// whether a card may carry an index made from it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct EmbeddingModel {
    #[serde(deserialize_with = "textual")]
    pub id: Option<String>,
    #[serde(deserialize_with = "textual")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct EmbeddingsReply {
    model: EmbeddingModel,
    #[serde(deserialize_with = "whole")]
    dim: Option<u32>,
    tracks: Vec<EmbeddingRow>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct EmbeddingRow {
    #[serde(rename = "filePath")]
    file_path: String,
    #[serde(rename = "audioHash", deserialize_with = "textual")]
    audio_hash: Option<String>,
    #[serde(rename = "notAnalyzed", deserialize_with = "flag")]
    not_analyzed: bool,
    #[serde(deserialize_with = "textual")]
    embedding: Option<String>,
}

/// The embeddings of a list of paths, read eight at a time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embeddings {
    pub model: EmbeddingModel,
    /// Floats per vector (1,280 for the default model).
    pub dim: u32,
    /// One per path the server resolved, in the order asked.
    pub tracks: Vec<Embedding>,
    /// Paths the server answered 404 for on their own: gone or renamed
    /// since the listing, or out of this user's sight.
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embedding {
    pub file_path: String,
    /// The canonical hash the vector is keyed by (the audio hash, else the
    /// file hash): the Auto DJ index's row identity.
    pub audio_hash: Option<String>,
    /// The track resolved but its vector is not made yet.
    pub not_analyzed: bool,
    /// `dim` floats, L2-normalised by the server. `None` when not analysed,
    /// and for a vector that did not decode to `dim` floats.
    pub vector: Option<Vec<f32>>,
}

/// One batch's answer, as [`Client::embeddings_async`] sorts it.
enum EmbeddingsBatch {
    Answer(EmbeddingsReply),
    /// A 404 in the route's own words ([`TRACK_NOT_FOUND`]): one of the
    /// paths did not resolve.
    BadPath,
    /// A 403: discovery is off, or its index is missing.
    Off,
}

/// Base64 of little-endian float32, `dim` of them (or any whole number of
/// them when the server named no `dim`).
fn floats(text: &str, dim: u32) -> Option<Vec<f32>> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(text.trim()).ok()?;
    if bytes.is_empty() || bytes.len() % 4 != 0 || (dim > 0 && bytes.len() != dim as usize * 4) {
        return None;
    }
    Some(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// Who reports a card's plays: `stats/plays`'s `client` block. The server
/// caps the name and the instance id at 64 characters, the version at 32.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayClient {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayOutcome {
    Completed,
    Skipped,
    Stopped,
}

/// The sources the server takes from a client (`SOURCES` without its own
/// `legacy`). A card's plays carry the mode the player was in; one it
/// didn't say is `Device` on a server that takes it (the same batch of
/// mStream changes as its `instanceId`: an older server answers `device`
/// with a 400), else `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaySource {
    Manual,
    Shuffle,
    Autodj,
    Playlist,
    SmartPlaylist,
    Auto,
    Carplay,
    Cast,
    Device,
    Other,
}

/// One play from a card's log as `stats/plays` takes it (`playSchema`):
/// backdated by its own `started_at`, and idempotent by `id`. A repeat
/// comes back among the duplicates and moves nothing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardPlay {
    /// At most 128 characters, and unique across the server: an id another
    /// user sent already is rejected as `invalid`.
    pub id: String,
    /// `vpath/rel` on the server the play is posted to. Plays resolve by
    /// path alone, so a file moved since the transfer is re-mapped first.
    pub file_path: String,
    /// The poster's server's own peer row id, for a peer's track.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<i64>,
    /// Epoch ms. The server takes no more than 5 minutes ahead of its own
    /// clock, and nothing older than its retention (24 months by default):
    /// `bad-time` otherwise.
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    pub played_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub outcome: PlayOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PlaySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub pause_count: u32,
    /// Required for a peer's track; a track of the server's own is looked
    /// up there and this is ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<PlayTrack>,
}

/// What a play says about a track the server cannot look up itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayTrack {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub art_file: Option<String>,
}

/// A string's length as the server's validator counts it (a JavaScript
/// string's: UTF-16 units).
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s` cut to at most `most` UTF-16 units, at a character boundary.
fn clip(s: &str, most: usize) -> String {
    let mut units = 0;
    s.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= most
        })
        .collect()
}

impl PlayTrack {
    /// The snapshot within the server's caps: names cut to 512, a hash or
    /// an art file name that is too long left out (a cut one would name
    /// something else).
    fn within_caps(&self) -> PlayTrack {
        let name = |s: &Option<String>| s.as_deref().map(|s| clip(s, 512));
        let whole = |s: &Option<String>, most: usize| s.clone().filter(|s| js_len(s) <= most);
        PlayTrack {
            title: name(&self.title),
            artist: name(&self.artist),
            album: name(&self.album),
            duration_ms: self.duration_ms,
            hash: whole(&self.hash, 128),
            art_file: whole(&self.art_file, 512),
        }
    }
}

/// The `client` block's version as the server takes it: at most 32
/// characters. None for an empty one, and for a longer one, left out (a cut
/// one would name another).
pub fn play_version(version: &str) -> Option<String> {
    (!version.is_empty() && js_len(version) <= 32).then(|| version.to_string())
}

/// What the server's schema refuses in `play`, which fails the whole batch
/// it rides in with a 400, every time; None for a play it takes.
pub fn play_fault(play: &CardPlay) -> Option<String> {
    if play.id.is_empty() || js_len(&play.id) > 128 {
        return Some(format!("an id of {} characters (1-128)", js_len(&play.id)));
    }
    if play.file_path.is_empty() || js_len(&play.file_path) > 2048 {
        return Some(format!("play {}: a path of 1-2048 characters", play.id));
    }
    if play.session_id.as_deref().is_some_and(|s| js_len(s) > 128) {
        return Some(format!("play {}: a session id over 128 characters", play.id));
    }
    if play.peer_id.is_some_and(|p| p < 1) {
        return Some(format!("play {}: a peer id below 1", play.id));
    }
    None
}

/// The body of one `stats/plays` call, checked against the server's schema
/// first: one field it refuses fails the whole batch with a 400, so the
/// fault is named here instead.
pub fn plays_body(client: &PlayClient, plays: &[CardPlay]) -> Result<serde_json::Value, ApiError> {
    let wrong = |what: String| Err(ApiError::Config(format!("not a play the server takes: {what}")));
    if plays.is_empty() || plays.len() > PLAYS_PER_CALL {
        return wrong(format!("{} plays in one call (1-{PLAYS_PER_CALL})", plays.len()));
    }
    let name = js_len(&client.name);
    if name == 0 || name > 64 {
        return wrong("a client name of 1-64 characters".to_string());
    }
    if client.version.as_deref().is_some_and(|v| js_len(v) > 32)
        || client.instance_id.as_deref().is_some_and(|v| js_len(v) > 64)
    {
        return wrong("a client version over 32 characters or an instance id over 64".to_string());
    }
    let mut checked = Vec::with_capacity(plays.len());
    for play in plays {
        if let Some(fault) = play_fault(play) {
            return wrong(fault);
        }
        let mut play = play.clone();
        play.track = play.track.as_ref().map(PlayTrack::within_caps);
        checked.push(play);
    }
    Ok(serde_json::json!({ "client": client, "plays": checked }))
}

/// `paths` cut into runs whose JSON array stays within `budget` bytes and
/// `most` entries, in order. A path heavier than the budget on its own
/// still goes, alone: the server is the judge of its own cap.
fn chunks(paths: &[String], budget: usize, most: usize) -> Vec<&[String]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut weight = 2; // the brackets
    for (i, path) in paths.iter().enumerate() {
        // The string as JSON writes it, and its comma.
        let cost = serde_json::to_string(path).map(|j| j.len()).unwrap_or(path.len() + 2) + 1;
        if i > start && (weight + cost > budget || i - start >= most) {
            out.push(&paths[start..i]);
            start = i;
            weight = 2;
        }
        weight += cost;
    }
    if start < paths.len() {
        out.push(&paths[start..]);
    }
    out
}

// ── What the server converts ────────────────────────────────────────────────

/// How many conversions a transfer runs at once on a server with no bulk
/// queue, and the most on one that has it: two, so a library's conversion
/// doesn't take the server's CPU from its listeners.
pub const CONVERSIONS: u32 = 2;

impl TranscodeInfo {
    /// Whether `/transcode` makes `codec` at `bitrate` as asked: the shared
    /// 64k-192k always, MP3's 256k and 320k only where the server lists
    /// them. Nothing past those is asked of a server, which would make its
    /// default without saying so. Opus stays at 192k whatever is listed:
    /// libopus refuses more than 256k a channel, and a mono source's 320k
    /// would fail after the answer began.
    pub fn takes(&self, codec: TransferCodec, bitrate: TransferBitrate) -> bool {
        if bitrate.shared() {
            return true;
        }
        let listed = self.codecs.as_ref().and_then(|codecs| codecs.get(codec.as_str()));
        let listed = listed.is_some_and(|c| c.bitrates.iter().any(|b| b == bitrate.as_str()));
        codec == TransferCodec::Mp3 && listed
    }

    /// `wanted` when the server makes `codec` at it ([`TranscodeInfo::takes`]),
    /// else the highest bitrate below it that it does: MP3 at 320k is 192k on
    /// a server older than the card transfer's batch of mStream changes.
    pub fn best(&self, codec: TransferCodec, wanted: TransferBitrate) -> TransferBitrate {
        let rates = TransferBitrate::ALL.into_iter().rev();
        let mut below = rates.filter(|b| b.kbps() <= wanted.kbps());
        below.find(|&b| self.takes(codec, b)).unwrap_or(TransferBitrate::K64)
    }

    /// The server's bulk queue (`?bulk=1`): how many of a transfer's
    /// conversions it runs at once, at most [`CONVERSIONS`]; None for a
    /// server without the queue, where a transfer keeps to [`CONVERSIONS`]
    /// of its own accord.
    pub fn bulk_slots(&self) -> Option<u32> {
        let bulk = self.bulk.as_ref()?;
        Some(bulk.concurrency.filter(|&n| n >= 1).unwrap_or(CONVERSIONS).min(CONVERSIONS))
    }
}

// ── Endpoints ───────────────────────────────────────────────────────────────

impl Client {
    /// One page of `POST /api/v1/sync/manifest`: the entries after `cursor`
    /// (from the start for `None`), at most `limit` (1-5000), from every
    /// library the user sees but `ignore` — the route has no include
    /// filter, so one library is asked for as all the others ignored.
    ///
    /// `known` is the revision a previous walk ended with. Sent on the first
    /// page only, as the server reads it only there: a 304 then means
    /// nothing changed that the revision sees.
    pub async fn manifest_page_async(
        &self,
        ignore: &[String],
        cursor: Option<i64>,
        limit: u32,
        known: Option<&str>,
    ) -> Result<ManifestAnswer, ApiError> {
        const PATH: &str = "api/v1/sync/manifest";
        let mut body = serde_json::json!({
            "limit": limit.clamp(1, MANIFEST_PAGE_MAX),
            "ignoreVPaths": ignore,
        });
        if let Some(cursor) = cursor {
            body["cursor"] = serde_json::json!(cursor.max(0));
        }
        let mut headers = Vec::new();
        if let (None, Some(known)) = (cursor, known) {
            headers.push(("If-None-Match", format!("\"{known}\"")));
        }
        let reply = self.exchange(Method::POST, PATH, Some(&body), &headers).await?;
        if reply.status == StatusCode::NOT_MODIFIED {
            return Ok(ManifestAnswer::NotModified);
        }
        let mut page: ManifestPage = decode(&reply, PATH)?;
        if page.revision.is_empty() {
            // The ETag carries the same revision, quoted.
            page.revision = reply
                .headers
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().trim_start_matches("W/").trim_matches('"').to_string())
                .unwrap_or_default();
        }
        Ok(ManifestAnswer::Page(page))
    }

    pub fn manifest_page(
        &self,
        ignore: &[String],
        cursor: Option<i64>,
        limit: u32,
        known: Option<&str>,
    ) -> Result<ManifestAnswer, ApiError> {
        wait(self.manifest_page_async(ignore, cursor, limit, known))
    }

    /// The whole manifest, page after page of `limit`, or the 304 that
    /// says `known` still holds. `progress` hears the entries read so far.
    pub async fn manifest_async(
        &self,
        ignore: &[String],
        limit: u32,
        known: Option<&str>,
        progress: &mut (dyn FnMut(usize) + Send),
    ) -> Result<Manifest, ApiError> {
        let mut listing = Listing::default();
        let mut cursor: Option<i64> = None;
        loop {
            let page = match self.manifest_page_async(ignore, cursor, limit, known).await? {
                ManifestAnswer::Page(page) => page,
                // Only the first page is ever answered so.
                ManifestAnswer::NotModified if cursor.is_none() => return Ok(Manifest::NotModified),
                ManifestAnswer::NotModified => {
                    return Err(ApiError::Decode {
                        endpoint: "api/v1/sync/manifest".to_string(),
                        message: "a 304 to a page after the first".to_string(),
                    });
                }
            };
            if cursor.is_none() {
                listing.revision = page.revision.clone();
            } else if page.revision != listing.revision {
                listing.moved = true;
            }
            listing.scanning |= page.scanning;
            listing.entries.extend(page.entries);
            progress(listing.entries.len());
            match page.next {
                // Keyset paging only moves forward; anything else would
                // walk the same page for ever.
                Some(next) if cursor.is_none_or(|c| next > c) => cursor = Some(next),
                Some(next) => {
                    return Err(ApiError::Decode {
                        endpoint: "api/v1/sync/manifest".to_string(),
                        message: format!("the next page's cursor {next} does not move past {cursor:?}"),
                    });
                }
                None => return Ok(Manifest::Listed(listing)),
            }
        }
    }

    pub fn manifest(
        &self,
        ignore: &[String],
        limit: u32,
        known: Option<&str>,
        progress: &mut (dyn FnMut(usize) + Send),
    ) -> Result<Manifest, ApiError> {
        wait(self.manifest_async(ignore, limit, known, progress))
    }

    /// Full metadata for `paths` (`POST /api/v1/db/metadata/batch`), in as
    /// many calls as the server's body cap needs ([`BATCH_BODY_BUDGET`],
    /// [`BATCH_MAX_PATHS`]). A chunk the server still finds too large (a
    /// 413: an admin lowered `maxRequestSize`) is halved and sent again.
    /// Every path asked for has a key; `None` is one the server could not
    /// resolve or this user cannot see. On a peer client the calls go
    /// through the parent's proxy, which allows this route.
    pub async fn metadata_batch_async(
        &self,
        paths: &[String],
    ) -> Result<HashMap<String, Option<Metadata>>, ApiError> {
        self.metadata_batch_within(paths, BATCH_BODY_BUDGET, BATCH_MAX_PATHS).await
    }

    async fn metadata_batch_within(
        &self,
        paths: &[String],
        budget: usize,
        most: usize,
    ) -> Result<HashMap<String, Option<Metadata>>, ApiError> {
        const PATH: &str = "api/v1/db/metadata/batch";
        let mut found = HashMap::with_capacity(paths.len());
        let mut queue: VecDeque<&[String]> = chunks(paths, budget, most).into();
        while let Some(chunk) = queue.pop_front() {
            let body = serde_json::json!(chunk);
            let reply = self.exchange(Method::POST, PATH, Some(&body), &[]).await?;
            if reply.status.as_u16() == 413 && chunk.len() > 1 {
                let (front, back) = chunk.split_at(chunk.len() / 2);
                queue.push_front(back);
                queue.push_front(front);
                continue;
            }
            let mut rows: HashMap<String, Option<BatchRow>> = decode(&reply, PATH)?;
            for path in chunk {
                let metadata = rows.remove(path).flatten().and_then(|row| row.metadata);
                found.insert(path.clone(), metadata);
            }
        }
        Ok(found)
    }

    pub fn metadata_batch(
        &self,
        paths: &[String],
    ) -> Result<HashMap<String, Option<Metadata>>, ApiError> {
        wait(self.metadata_batch_async(paths))
    }

    /// The embeddings of `paths` (`POST /api/v1/discovery/local/embeddings`),
    /// eight a call. One path the server cannot resolve fails its whole call
    /// with a 404, so such a batch is split in halves until the path stands
    /// alone; it lands in `missing`, and the others are read. Send manifest
    /// paths: every split costs calls.
    ///
    /// `None` means discovery is off on this server (a 403), as for
    /// [`Client::similar_tracks`]. A 404 in other words than a bad path's
    /// is an error: a server without the route, or a parent's proxy with
    /// no such peer.
    pub async fn embeddings_async(&self, paths: &[String]) -> Result<Option<Embeddings>, ApiError> {
        let mut out = Embeddings::default();
        let mut model: Option<(EmbeddingModel, u32)> = None;
        let mut queue: VecDeque<&[String]> = paths.chunks(EMBEDDINGS_PER_CALL).collect();
        while let Some(batch) = queue.pop_front() {
            let reply = match self.embeddings_batch(batch).await? {
                EmbeddingsBatch::Off => return Ok(None),
                EmbeddingsBatch::BadPath if batch.len() > 1 => {
                    let (front, back) = batch.split_at(batch.len() / 2);
                    queue.push_front(back);
                    queue.push_front(front);
                    continue;
                }
                EmbeddingsBatch::BadPath => {
                    out.missing.extend(batch.iter().cloned());
                    continue;
                }
                EmbeddingsBatch::Answer(reply) => reply,
            };
            let dim = reply.dim.unwrap_or(0);
            match &model {
                None => model = Some((reply.model.clone(), dim)),
                // One index has one model; vectors from two cannot be
                // compared, so a walk across a re-index is no answer.
                Some((was, was_dim)) if *was != reply.model || *was_dim != dim => {
                    return Err(ApiError::Decode {
                        endpoint: "api/v1/discovery/local/embeddings".to_string(),
                        message: "the embedding model changed while it was read".to_string(),
                    });
                }
                Some(_) => {}
            }
            for row in reply.tracks {
                let vector = match (&row.embedding, row.not_analyzed) {
                    (Some(text), false) => floats(text, dim),
                    _ => None,
                };
                out.tracks.push(Embedding {
                    file_path: row.file_path,
                    audio_hash: row.audio_hash,
                    not_analyzed: row.not_analyzed,
                    vector,
                });
            }
        }
        if let Some((model, dim)) = model {
            out.model = model;
            out.dim = dim;
        }
        Ok(Some(out))
    }

    async fn embeddings_batch(&self, batch: &[String]) -> Result<EmbeddingsBatch, ApiError> {
        const PATH: &str = "api/v1/discovery/local/embeddings";
        let body = serde_json::json!({ "filePaths": batch });
        let reply = self.exchange(Method::POST, PATH, Some(&body), &[]).await?;
        match reply.status {
            StatusCode::FORBIDDEN => Ok(EmbeddingsBatch::Off),
            StatusCode::NOT_FOUND => {
                // A bad path is the route's own words (mStream's
                // `discovery.js`, `resolveSeedTrack`). Every refusal is
                // `{error}`, so the words decide: a parent's proxy with no
                // such peer says "Peer not found" (`federation-browse.js`),
                // and a server without the route answers with its
                // framework's page.
                let words = serde_json::from_slice::<serde_json::Value>(&reply.body)
                    .ok()
                    .and_then(|v| v.get("error")?.as_str().map(str::to_string));
                match words {
                    Some(words) if words.trim().eq_ignore_ascii_case(TRACK_NOT_FOUND) => {
                        Ok(EmbeddingsBatch::BadPath)
                    }
                    Some(words) => Err(ApiError::NotFound(format!("{PATH} ({words})"))),
                    None => Err(ApiError::NotFound(PATH.to_string())),
                }
            }
            _ => decode(&reply, PATH).map(EmbeddingsBatch::Answer),
        }
    }

    pub fn embeddings(&self, paths: &[String]) -> Result<Option<Embeddings>, ApiError> {
        wait(self.embeddings_async(paths))
    }

    /// Every audio file under `directory`, as `vpath/rel` paths, from
    /// `POST /api/v1/file-explorer/recursive`: paths only, no sizes. A
    /// peer's stand-in for the manifest it does not serve to keys and
    /// guests; [`Client::metadata_batch_async`] then gives the sizes and
    /// hashes. One top-level folder a call keeps a big library inside the
    /// parent proxy's 15 s deadline.
    pub async fn recursive_files_async(&self, directory: &str) -> Result<Vec<String>, ApiError> {
        const PATH: &str = "api/v1/file-explorer/recursive";
        let body = serde_json::json!({ "directory": directory });
        let reply = self.exchange(Method::POST, PATH, Some(&body), &[]).await?;
        decode(&reply, PATH)
    }

    pub fn recursive_files(&self, directory: &str) -> Result<Vec<String>, ApiError> {
        wait(self.recursive_files_async(directory))
    }

    /// One folder's own entries (`POST /api/v1/file-explorer`, no metadata):
    /// its folders, and the files beside them, which a library's top holds
    /// where [`Client::recursive_files_async`] of its folders doesn't look.
    pub async fn folder_async(&self, directory: &str) -> Result<DirListing, ApiError> {
        const PATH: &str = "api/v1/file-explorer";
        let body = serde_json::json!({ "directory": directory, "pullMetadata": false });
        let reply = self.exchange(Method::POST, PATH, Some(&body), &[]).await?;
        decode(&reply, PATH)
    }

    pub fn folder(&self, directory: &str) -> Result<DirListing, ApiError> {
        wait(self.folder_async(directory))
    }

    /// `GET /api/`, counted as the transfer's calls are: for a peer client
    /// the peer's own, through the parent's browse proxy, with its key's
    /// allowance when the peer gives one.
    pub async fn peer_api_async(&self) -> Result<PeerApi, ApiError> {
        const PATH: &str = "api/";
        let reply = self.exchange(Method::GET, PATH, None, &[]).await?;
        let info: LayeredInfo = decode(&reply, PATH)?;
        let raw: serde_json::Value = serde_json::from_slice(&reply.body).unwrap_or_default();
        let allowance = raw.get("user").and_then(|u| u.get("federationAllowance")).cloned();
        let allowance = allowance.and_then(|a| serde_json::from_value::<Allowance>(a).ok());
        Ok(PeerApi { info, allowance })
    }

    pub fn peer_api(&self) -> Result<PeerApi, ApiError> {
        wait(self.peer_api_async())
    }

    /// The parent's federation peers (`GET /api/v1/federation/peers`), with
    /// their iroh identities. On the parent's own client.
    pub async fn transfer_peers_async(&self) -> Result<Vec<TransferPeer>, ApiError> {
        const PATH: &str = "api/v1/federation/peers";
        let reply = self.exchange(Method::GET, PATH, None, &[]).await?;
        decode::<TransferPeers>(&reply, PATH).map(|p| p.peers)
    }

    pub fn transfer_peers(&self) -> Result<Vec<TransferPeer>, ApiError> {
        wait(self.transfer_peers_async())
    }

    /// The folder mStream keeps its own ffmpeg in (`GET
    /// /api/v1/admin/transcode`, its `ffmpegDirectory`), as the server sees
    /// it: an admin's, while the admin API answers. None when it names none.
    pub async fn admin_ffmpeg_folder_async(&self) -> Result<Option<String>, ApiError> {
        const PATH: &str = "api/v1/admin/transcode";
        let reply = self.exchange(Method::GET, PATH, None, &[]).await?;
        let value: serde_json::Value = decode(&reply, PATH)?;
        let folder = value.get("ffmpegDirectory").and_then(|v| v.as_str());
        Ok(folder.filter(|f| !f.trim().is_empty()).map(str::to_string))
    }

    pub fn admin_ffmpeg_folder(&self) -> Result<Option<String>, ApiError> {
        wait(self.admin_ffmpeg_folder_async())
    }

    /// A cover in one of its sizes, raw bytes in whatever format the server
    /// holds (the small copies keep the original's). A peer client asks the
    /// parent's art proxy, which passes the size on.
    pub async fn album_art_sized_async(&self, file: &str, size: ArtSize) -> Result<Vec<u8>, ApiError> {
        let url =
            urls::album_art_sized_url(&self.server(), self.peer, file, size).map_err(ApiError::Config)?;
        let url = urls::with_local_token(url, self.local_token.as_deref());
        let resp = self.transfer_request(Method::GET, &url).send().await.map_err(unreached)?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(unreached)?;
        self.count(bytes.len());
        match refused(status, &String::from_utf8_lossy(&bytes), &format!("album-art/{file}")) {
            Some(refusal) => Err(refusal),
            None => Ok(bytes.to_vec()),
        }
    }

    pub fn album_art_sized(&self, file: &str, size: ArtSize) -> Result<Vec<u8>, ApiError> {
        wait(self.album_art_sized_async(file, size))
    }

    /// Post one batch of a card's plays (at most [`PLAYS_PER_CALL`]) through
    /// the client's play route, [`Client::report_plays_async`], with the
    /// card's own `client` block. Every id the answer names is settled,
    /// whichever list it is on. A federation key or a guest is refused with
    /// 403: plays go to the user's own server, a peer's with `peer_id`.
    pub async fn report_card_plays_async(
        &self,
        client: &PlayClient,
        plays: &[CardPlay],
    ) -> Result<PlaysAnswer, ApiError> {
        let body = plays_body(client, plays)?;
        self.report_plays_async(body).await
    }

    pub fn report_card_plays(
        &self,
        client: &PlayClient,
        plays: &[CardPlay],
    ) -> Result<PlaysAnswer, ApiError> {
        wait(self.report_card_plays_async(client, plays))
    }
}

#[cfg(test)]
pub(crate) mod stub {
    //! A loopback server for the transfer's tests: `canned_server`'s shape
    //! (one answer per connection, in turn, each request handed back), but
    //! it reads a request's body to its length too, and an answer can be a
    //! script — bytes, then silence, then more bytes.

    use std::io::{Read, Write};
    use std::time::Duration;

    pub(crate) enum Step {
        Send(Vec<u8>),
        Wait(Duration),
    }

    /// Serves `scripts` one connection each, in order. The base URL, and
    /// every request whole (head and body) as it came.
    pub(crate) fn scripted(scripts: Vec<Vec<Step>>) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            for script in scripts {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                let head_end = loop {
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                    match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break request.len(),
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while request.len() < head_end + length {
                    match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = seen_tx.send(String::from_utf8_lossy(&request).to_string());
                for step in script {
                    match step {
                        Step::Send(bytes) => {
                            let _ = sock.write_all(&bytes);
                            let _ = sock.flush();
                        }
                        Step::Wait(pause) => std::thread::sleep(pause),
                    }
                }
            }
        });
        (format!("http://127.0.0.1:{port}"), seen_rx)
    }

    /// One plain answer per connection.
    pub(crate) fn canned(replies: Vec<Vec<u8>>) -> (String, std::sync::mpsc::Receiver<String>) {
        scripted(replies.into_iter().map(|r| vec![Step::Send(r)]).collect())
    }

    /// An answer with a length and `Connection: close`, and `headers`.
    pub(crate) fn reply(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        let mut out = out.into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// A JSON answer.
    pub(crate) fn json(status: &str, body: &str) -> Vec<u8> {
        reply(status, &[("Content-Type", "application/json")], body.as_bytes())
    }

    /// A chunked answer with no length, as `/transcode` streams one.
    pub(crate) fn chunked(status: &str, headers: &[(&str, &str)], pieces: &[&[u8]]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n");
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        let mut out = out.into_bytes();
        for piece in pieces {
            out.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            out.extend_from_slice(piece);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n\r\n");
        out
    }

    /// The JSON body of a request `scripted` handed back.
    pub(crate) fn body_of(request: &str) -> serde_json::Value {
        let body = request.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or_default();
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
    }

    /// A request as [`routed`] hands it to its handler.
    #[derive(Clone, Debug)]
    pub(crate) struct Asked {
        pub(crate) method: String,
        /// The path and query, as sent (percent-encoded).
        pub(crate) target: String,
        /// Each header, its name lowercase.
        pub(crate) headers: Vec<(String, String)>,
        pub(crate) body: Vec<u8>,
    }

    impl Asked {
        pub(crate) fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
        }

        /// The path, percent-decoded, without its query.
        pub(crate) fn path(&self) -> String {
            let raw = self.target.split('?').next().unwrap_or_default().as_bytes();
            let mut out = Vec::with_capacity(raw.len());
            let mut i = 0;
            while i < raw.len() {
                let hex = raw.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
                match (raw[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
                    (b'%', Some(b)) => {
                        out.push(b);
                        i += 3;
                    }
                    (c, _) => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            String::from_utf8_lossy(&out).into_owned()
        }

        /// The query's value of `key`, as sent.
        pub(crate) fn query(&self, key: &str) -> Option<&str> {
            let query = self.target.split_once('?')?.1;
            let pairs = query.split('&').filter_map(|p| p.split_once('='));
            pairs.into_iter().find(|(k, _)| *k == key).map(|(_, v)| v)
        }

        pub(crate) fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
        }
    }

    /// A link of `total` bytes a second that no one connection takes more
    /// than `each` of: a LAN as one stream and three measured it. Answers go
    /// out in pieces of [`Pace::PIECE`], each when both clocks allow it, so
    /// a late wake-up is made up by the pieces after it.
    pub(crate) struct Pace {
        each: f64,
        total: f64,
        /// When the link is free next.
        link: std::sync::Mutex<std::time::Instant>,
    }

    impl Pace {
        const PIECE: usize = 16 * 1024;

        pub(crate) fn new(each: f64, total: f64) -> std::sync::Arc<Pace> {
            let link = std::sync::Mutex::new(std::time::Instant::now());
            std::sync::Arc::new(Pace { each, total, link })
        }

        fn send(&self, sock: &mut std::net::TcpStream, bytes: &[u8]) -> std::io::Result<()> {
            let until = |t: std::time::Instant| {
                let now = std::time::Instant::now();
                if t > now {
                    std::thread::sleep(t - now);
                }
            };
            let mut mine = std::time::Instant::now();
            for piece in bytes.chunks(Pace::PIECE) {
                until(mine);
                let at = {
                    let mut free = self.link.lock().unwrap_or_else(|e| e.into_inner());
                    let at = (*free).max(std::time::Instant::now());
                    *free = at + Duration::from_secs_f64(piece.len() as f64 / self.total);
                    at
                };
                until(at);
                sock.write_all(piece)?;
                mine = mine.max(at) + Duration::from_secs_f64(piece.len() as f64 / self.each);
            }
            Ok(())
        }
    }

    /// A server whose answers depend on the request, not its turn: every
    /// connection on a thread of its own, its one request handed to
    /// `answer`, whose bytes go back before the connection closes. For
    /// requests that come in no set order (a run's, made on several
    /// threads). It serves until the test process ends.
    pub(crate) fn routed(answer: impl Fn(&Asked) -> Vec<u8> + Send + Sync + 'static) -> String {
        routed_at(None, answer)
    }

    /// [`routed`], its answers sent at `pace`.
    pub(crate) fn routed_at(
        pace: Option<std::sync::Arc<Pace>>,
        answer: impl Fn(&Asked) -> Vec<u8> + Send + Sync + 'static,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let answer = std::sync::Arc::new(answer);
        std::thread::spawn(move || {
            for sock in listener.incoming() {
                let Ok(mut sock) = sock else { continue };
                let (answer, pace) = (answer.clone(), pace.clone());
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 8192];
                    let head_end = loop {
                        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            break at + 4;
                        }
                        match sock.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&request[..head_end]).to_string();
                    let mut lines = head.lines();
                    let first: Vec<&str> = lines.next().unwrap_or_default().split(' ').collect();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let length = headers
                        .iter()
                        .find(|(n, _)| n == "content-length")
                        .and_then(|(_, v)| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    while request.len() < head_end + length {
                        match sock.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let asked = Asked {
                        method: first.first().copied().unwrap_or_default().to_string(),
                        target: first.get(1).copied().unwrap_or_default().to_string(),
                        headers,
                        body: request[head_end..].to_vec(),
                    };
                    let bytes = answer(&asked);
                    let _ = match &pace {
                        Some(pace) => pace.send(&mut sock, &bytes),
                        None => sock.write_all(&bytes),
                    };
                    let _ = sock.flush();
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::stub::{body_of, canned, json, reply};
    use super::*;

    fn recv(seen: &std::sync::mpsc::Receiver<String>) -> String {
        seen.recv_timeout(Duration::from_secs(5)).expect("the request reached the stub")
    }

    /// `/api/`'s transcode block, as a server sends it.
    fn transcode(block: &str) -> TranscodeInfo {
        let api = format!(r#"{{"server":"6.32.0","features":{{"sync":true,"transcode":{block}}}}}"#);
        let info: super::super::types::LayeredInfo = serde_json::from_str(&api).expect("an answer");
        info.features.transcode.expect("a server that converts")
    }

    #[test]
    fn mp3_reaches_320k_and_conversions_queue_only_where_the_server_says_so() {
        use TransferBitrate::{K64, K128, K192, K256, K320};
        use TransferCodec::{Mp3, Opus};
        // The card transfer's batch of mStream changes: each codec's rates,
        // and the bulk queue. Opus never past 192k, whatever is listed.
        let new = transcode(
            r#"{"defaultCodec":"opus","defaultBitrate":"96k","bulk":{"concurrency":1},
                "codecs":{"mp3":{"bitrates":["64k","96k","128k","192k","256k","320k"],
                                 "sampleRates":[32000,44100,48000]},
                          "opus":{"bitrates":["64k","96k","128k","192k","256k","320k"]},
                          "aac":{"bitrates":["64k","96k","128k","192k"]}}}"#,
        );
        assert!(new.takes(Mp3, K320) && new.takes(Mp3, K256) && new.takes(Opus, K192));
        assert!(!new.takes(Opus, K320) && !new.takes(Opus, K256));
        assert_eq!((new.best(Mp3, K320), new.best(Opus, K320), new.best(Mp3, K128)), (K320, K192, K128));
        assert_eq!(new.bulk_slots(), Some(1), "the server's cap, below the transfer's own");

        // An older server: 64k-192k of each, no queue.
        let old = transcode(r#"{"defaultCodec":"opus","defaultBitrate":"96k"}"#);
        assert!(!old.takes(Mp3, K256) && !old.takes(Mp3, K320) && old.takes(Mp3, K192));
        assert_eq!((old.best(Mp3, K320), old.best(Mp3, K256), old.best(Mp3, K64)), (K192, K192, K64));
        assert_eq!(old.bulk_slots(), None);

        // A list without 256k: 320k as asked, 256k the next one down.
        let some = r#"{"codecs":{"mp3":{"bitrates":["64k","128k","320k"]}},"bulk":{"concurrency":8}}"#;
        let some = transcode(some);
        assert_eq!((some.best(Mp3, K320), some.best(Mp3, K256)), (K320, K192), "the shared set needs no list");
        assert_eq!(some.bulk_slots(), Some(2), "never more than the transfer's own two");

        // An odd value costs its own field, never the answer.
        let odd = transcode(r#"{"defaultCodec":"opus","codecs":"lots","bulk":{"concurrency":"two"}}"#);
        assert_eq!(odd.default_codec.as_deref(), Some("opus"));
        assert!(odd.codecs.is_none() && !odd.takes(Mp3, K320));
        assert_eq!(odd.bulk_slots(), Some(2), "a queue all the same, at the transfer's own pace");
        assert_eq!(transcode(r#"{"bulk":{"concurrency":0}}"#).bulk_slots(), Some(2));
    }

    fn entry(id: i64, path: &str) -> String {
        format!(
            r#"{{"filepath":"{path}","metadata":{{"title":"Track {id}","artist":"Artist A",
            "artist-display":"Artist A feat. B","artists":["Artist A"],"album":"Album B","album-art":"0f.jpeg",
            "year":2001,"track":{id},"disk":1,"duration":181.5,"rating":null,"bpm":124,
            "musical-key":"A minor","genres":["Synth"],"has-lyrics":false,"has-synced-lyrics":false,
            "replaygain-track":-6.5}},"id":{id},"file-size":4096,"modified":1700000000123,
            "hash":"aa{id}","audio-hash":"bb{id}","hash-v":2,"format":"flac","album-id":7,
            "artist-id":3,"created-at":"2026-01-02 03:04:05"}}"#
        )
    }

    #[test]
    fn a_manifest_walk_reads_every_field_and_sends_the_cursor_on() {
        let first = format!(
            r#"{{"revision":"1.2:2:11:1700:2","scanning":false,"next":10,"entries":[{}]}}"#,
            entry(10, "lib/Artist A/Album B/01.flac")
        );
        let second = format!(
            r#"{{"revision":"1.2:2:11:1700:2","scanning":true,"next":null,"entries":[{}]}}"#,
            entry(11, "lib/Artist A/Album B/02.flac")
        );
        let (base, seen) = canned(vec![
            reply(
                "200 OK",
                &[("Content-Type", "application/json"), ("ETag", "\"1.2:2:11:1700:2\"")],
                first.as_bytes(),
            ),
            json("200 OK", &second),
        ]);
        let client = Client::new(&base).unwrap().with_token(Some("tok".into()));
        let mut heard = Vec::new();
        let ignore = vec!["other".to_string()];
        let Manifest::Listed(listing) = client.manifest(&ignore, 1, None, &mut |n| heard.push(n)).unwrap() else {
            panic!("a full listing")
        };

        let first_req = recv(&seen);
        assert!(first_req.starts_with("POST /api/v1/sync/manifest HTTP/1.1\r\n"), "{first_req}");
        assert!(first_req.to_ascii_lowercase().contains("x-access-token: tok\r\n"));
        assert!(!first_req.to_ascii_lowercase().contains("if-none-match"), "no revision known, none sent");
        assert_eq!(body_of(&first_req), serde_json::json!({ "limit": 1, "ignoreVPaths": ["other"] }));
        let second_req = recv(&seen);
        assert_eq!(body_of(&second_req)["cursor"], 10, "the page after the last id seen");

        assert_eq!(heard, [1, 2]);
        assert_eq!(listing.revision, "1.2:2:11:1700:2");
        assert!(listing.scanning, "one page said a scan was running");
        assert!(!listing.moved);
        assert!(!listing.deletions_trusted(), "a missing path is not a deletion mid-scan");
        let e = &listing.entries[0];
        assert_eq!(e.filepath, "lib/Artist A/Album B/01.flac");
        assert_eq!((e.id, e.file_size, e.modified), (10, Some(4096), Some(1_700_000_000_123)));
        assert_eq!((e.hash.as_deref(), e.audio_hash.as_deref(), e.hash_v), (Some("aa10"), Some("bb10"), Some(2)));
        assert_eq!((e.format.as_deref(), e.album_id, e.artist_id), (Some("flac"), Some(7), Some(3)));
        assert_eq!(e.created_at.as_deref(), Some("2026-01-02 03:04:05"));
        let m = &e.metadata;
        assert_eq!(m.title.as_deref(), Some("Track 10"));
        assert_eq!(m.artist_display.as_deref(), Some("Artist A feat. B"));
        assert_eq!(m.artists, ["Artist A"]);
        assert_eq!((m.track, m.disk, m.year), (Some(10), Some(1), Some(2001)));
        assert_eq!((m.duration, m.bpm, m.replaygain_track), (Some(181.5), Some(124.0), Some(-6.5)));
        assert_eq!(m.musical_key.as_deref(), Some("A minor"));
        assert_eq!(m.album_art.as_deref(), Some("0f.jpeg"));
        assert_eq!(m.genres, ["Synth"]);
        assert_eq!(m.sample_rate, None, "the lite block has no fidelity fields");
    }

    #[test]
    fn a_known_revision_is_sent_on_the_first_page_and_a_304_ends_the_walk() {
        let not_modified = b"HTTP/1.1 304 Not Modified\r\nETag: \"1:5:9:77:5\"\r\nConnection: close\r\n\r\n";
        let (base, seen) = canned(vec![not_modified.to_vec()]);
        let client = Client::new(&base).unwrap();
        let got = client.manifest(&[], MANIFEST_PAGE_MAX, Some("1:5:9:77:5"), &mut |_| {}).unwrap();
        assert_eq!(got, Manifest::NotModified);
        let req = recv(&seen);
        assert!(req.to_ascii_lowercase().contains("if-none-match: \"1:5:9:77:5\"\r\n"), "{req}");
        assert_eq!(body_of(&req), serde_json::json!({ "limit": 5000, "ignoreVPaths": [] }));

        // Mid-walk the revision is not asked about: the server would ignore
        // it, and a page must come whatever the tag says. The limits are
        // the server's own.
        let page = r#"{"revision":"1:5:9:77:5","scanning":false,"next":null,"entries":[]}"#;
        let (base, seen) = canned(vec![json("200 OK", page)]);
        let client = Client::new(&base).unwrap();
        let got = client.manifest_page(&[], Some(-3), 9000, Some("1:5:9:77:5")).unwrap();
        let want = ManifestPage { revision: "1:5:9:77:5".into(), ..Default::default() };
        assert_eq!(got, ManifestAnswer::Page(want));
        let req = recv(&seen);
        assert!(!req.to_ascii_lowercase().contains("if-none-match"), "{req}");
        assert_eq!(body_of(&req), serde_json::json!({ "limit": 5000, "ignoreVPaths": [], "cursor": 0 }));
    }

    #[test]
    fn a_library_that_moves_mid_walk_keeps_its_first_revision_and_says_so() {
        let page = |revision: &str, next: &str, e: String| {
            format!(r#"{{"revision":"{revision}","scanning":false,"next":{next},"entries":[{e}]}}"#)
        };
        let first = page("r1", "4", entry(4, "lib/a.mp3"));
        let second = page("r2", "null", entry(9, "lib/b.mp3"));
        let (base, seen) = canned(vec![json("200 OK", &first), json("200 OK", &second)]);
        let client = Client::new(&base).unwrap();
        let Manifest::Listed(listing) = client.manifest(&[], 1, Some("r0"), &mut |_| {}).unwrap() else {
            panic!("a full listing")
        };
        assert_eq!(listing.revision, "r1", "the revision the walk started from");
        assert!(listing.moved && !listing.deletions_trusted());
        let _ = recv(&seen);
        let later = recv(&seen);
        assert!(!later.to_ascii_lowercase().contains("if-none-match"), "only the first page asks: {later}");

        // A cursor that does not move would walk one page for ever.
        let stuck = format!(r#"{{"revision":"r","next":4,"entries":[{}]}}"#, entry(4, "lib/a.mp3"));
        let (base, _seen) = canned(vec![json("200 OK", &stuck), json("200 OK", &stuck)]);
        let client = Client::new(&base).unwrap();
        assert!(matches!(client.manifest(&[], 1, None, &mut |_| {}), Err(ApiError::Decode { .. })));
    }

    #[test]
    fn an_odd_value_costs_its_field_not_the_page() {
        // A REAL mtime from before mStream's V24, a numeric string, a null
        // block, a bool where a number was expected, a list with a stray.
        let page: ManifestPage = serde_json::from_str(
            r#"{"revision":"r","scanning":0,"next":"12","entries":[{"filepath":"lib/a.mp3","id":3,
            "metadata":null,"file-size":"2048","modified":1700000000123.6,"hash-v":true},
            {"filepath":"lib/b.mp3","id":"4","metadata":{"track":"7","year":1999.0,"genres":["A",3,null],
            "artists":null,"bpm":"121.5","has-lyrics":1,"composer":12}},{"filepath":"lib/c.mp3","id":5.0}]}"#,
        )
        .unwrap();
        assert!(!page.scanning, "only a real true says scanning");
        assert_eq!(page.next, Some(12));
        assert_eq!(page.entries.iter().map(|e| e.id).collect::<Vec<_>>(), [3, 4, 5], "an id however it came");
        let a = &page.entries[0];
        assert_eq!(a.metadata, Metadata::default());
        assert_eq!((a.file_size, a.modified, a.hash_v), (Some(2048), Some(1_700_000_000_124), None));
        let b = &page.entries[1].metadata;
        assert_eq!((b.track, b.year, b.bpm), (Some(7), Some(1999), Some(121.5)));
        assert_eq!(b.genres, ["A"]);
        assert!(b.artists.is_empty() && !b.has_lyrics);
        assert_eq!(b.composer.as_deref(), Some("12"));

        // But a row with no path, or no id to place it by, is never read as
        // anything.
        let broken = serde_json::from_str::<ManifestPage>(r#"{"revision":"r","entries":[{"id":3}]}"#);
        assert!(broken.is_err(), "an entry without its path fails the page");
        for id in ["", r#","id":null"#, r#","id":"x""#, r#","id":3.5"#] {
            let page = format!(r#"{{"revision":"r","entries":[{{"filepath":"lib/a.mp3"{id}}}]}}"#);
            let broken = serde_json::from_str::<ManifestPage>(&page);
            assert!(broken.is_err(), "an entry without a whole id fails the page: {page}");
        }
    }

    #[test]
    fn full_metadata_arrives_in_chunks_under_the_body_cap() {
        let paths: Vec<String> = (0..5).map(|i| format!("lib/Folder {i}/track {i}.flac")).collect();
        // 27 bytes a path as JSON, a comma, 2 for the brackets: two paths
        // fit in 70 bytes, a third does not.
        let cut = chunks(&paths, 70, 100);
        assert_eq!(cut.iter().map(|c| c.len()).collect::<Vec<_>>(), [2, 2, 1]);
        let cut = chunks(&paths, BATCH_BODY_BUDGET, 2);
        assert_eq!(cut.iter().map(|c| c.len()).collect::<Vec<_>>(), [2, 2, 1], "and never past the count");
        let heavy = vec!["x".repeat(200)];
        assert_eq!(chunks(&heavy, 70, 100).len(), 1, "a path over the budget still goes, alone");
        assert!(chunks(&[], 70, 100).is_empty());

        let full = |p: &str| {
            format!(
                r#""{p}":{{"filepath":"{p}","metadata":{{"title":"T","sample-rate":44100,"channels":2,
                "bit-depth":16,"bpm-source":"essentia","track-total":12,"disc-total":1,
                "composer":"Writer C","musicbrainz-recording-id":"mb-1","isrc":"XX0000000001",
                "play-count":3,"last-played":"2026-09-01 10:00:00"}}}}"#
            )
        };
        let (base, seen) = canned(vec![
            json("200 OK", &format!("{{{},{}}}", full(&paths[0]), full(&paths[1]))),
            // The server's cap is lower than ours: the chunk is halved.
            json("413 Payload Too Large", r#"{"error":"request entity too large"}"#),
            json("200 OK", &format!("{{{}}}", full(&paths[2]))),
            json("200 OK", &format!(r#"{{"{}":{{"filepath":"{}","metadata":null}}}}"#, paths[3], paths[3])),
            json("200 OK", &format!("{{{}}}", full(&paths[4]))),
        ]);
        let meter = Arc::new(AtomicU64::new(0));
        let client = Client::new(&base).unwrap().with_meter(Some(meter.clone()));
        let found = wait(client.metadata_batch_within(&paths, 70, 100)).unwrap();
        assert_eq!(found.len(), 5, "every path asked for has its key");
        let m = found[&paths[0]].as_ref().expect("the first resolved");
        assert_eq!((m.sample_rate, m.channels, m.bit_depth), (Some(44100), Some(2), Some(16)));
        assert_eq!((m.bpm_source.as_deref(), m.track_total, m.disc_total), (Some("essentia"), Some(12), Some(1)));
        assert_eq!((m.composer.as_deref(), m.isrc.as_deref()), (Some("Writer C"), Some("XX0000000001")));
        assert_eq!(m.musicbrainz_recording_id.as_deref(), Some("mb-1"));
        assert_eq!((m.play_count, m.last_played.as_deref()), (Some(3), Some("2026-09-01 10:00:00")));
        assert!(found[&paths[3]].is_none(), "the server could not resolve it");
        assert!(found[&paths[4]].is_some());

        let bodies: Vec<serde_json::Value> = (0..5).map(|_| body_of(&recv(&seen))).collect();
        assert_eq!(bodies[0], serde_json::json!([paths[0], paths[1]]));
        assert_eq!(bodies[1], serde_json::json!([paths[2], paths[3]]));
        assert_eq!(bodies[2], serde_json::json!([paths[2]]), "halved after the 413");
        assert_eq!(bodies[3], serde_json::json!([paths[3]]));
        assert_eq!(bodies[4], serde_json::json!([paths[4]]));
        assert!(meter.load(Ordering::Relaxed) > 500, "every answer's body was counted");
    }

    fn vector_b64(values: &[f32]) -> String {
        use base64::Engine;
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn a_404_batch_of_embeddings_is_split_until_the_bad_path_stands_alone() {
        let paths: Vec<String> = ["lib/a.mp3", "lib/gone.mp3", "lib/c.mp3"].map(String::from).to_vec();
        let answer = |rows: &[(&str, Option<&[f32]>)]| {
            let tracks: Vec<serde_json::Value> = rows
                .iter()
                .map(|(p, v)| match v {
                    Some(v) => serde_json::json!({
                        "filePath": p, "audioHash": format!("h-{p}"), "notAnalyzed": false,
                        "embedding": vector_b64(v)
                    }),
                    None => serde_json::json!({
                        "filePath": p, "audioHash": null, "notAnalyzed": true, "embedding": null
                    }),
                })
                .collect();
            let model = serde_json::json!({ "id": "model-x", "version": "1" });
            let body = serde_json::json!({ "model": model, "dim": 2, "tracks": tracks });
            json("200 OK", &body.to_string())
        };
        let not_found = || json("404 Not Found", r#"{"error":"Track not found"}"#);
        let (base, seen) = canned(vec![
            not_found(),                                      // all three
            answer(&[("lib/a.mp3", Some(&[0.6, 0.8]))]),      // [a]
            not_found(),                                      // [gone, c]
            not_found(),                                      // [gone]
            answer(&[("lib/c.mp3", None)]),                   // [c]
        ]);
        let client = Client::new(&base).unwrap();
        let got = client.embeddings(&paths).unwrap().expect("discovery is on");
        let asked: Vec<serde_json::Value> = (0..5).map(|_| body_of(&recv(&seen))["filePaths"].clone()).collect();
        assert_eq!(asked[0], serde_json::json!(["lib/a.mp3", "lib/gone.mp3", "lib/c.mp3"]));
        assert_eq!(asked[1], serde_json::json!(["lib/a.mp3"]));
        assert_eq!(asked[2], serde_json::json!(["lib/gone.mp3", "lib/c.mp3"]));
        assert_eq!(asked[3], serde_json::json!(["lib/gone.mp3"]));
        assert_eq!(asked[4], serde_json::json!(["lib/c.mp3"]));

        assert_eq!(got.missing, ["lib/gone.mp3"]);
        assert_eq!((got.model.id.as_deref(), got.model.version.as_deref()), (Some("model-x"), Some("1")));
        assert_eq!(got.dim, 2);
        assert_eq!(got.tracks.len(), 2, "in the order asked");
        assert_eq!(got.tracks[0].vector, Some(vec![0.6, 0.8]), "little-endian float32, decoded");
        assert_eq!(got.tracks[0].audio_hash.as_deref(), Some("h-lib/a.mp3"));
        assert!(got.tracks[1].not_analyzed && got.tracks[1].vector.is_none());
    }

    #[test]
    fn embeddings_say_none_when_discovery_is_off_and_fail_where_the_route_is_missing() {
        let (base, seen) = canned(vec![
            json("403 Forbidden", r#"{"error":"Discovery is disabled"}"#),
            reply("404 Not Found", &[("Content-Type", "text/html")], b"<pre>Cannot POST</pre>"),
            json("404 Not Found", r#"{"error":"Peer not found"}"#),
        ]);
        let client = Client::new(&base).unwrap();
        let paths = vec!["lib/a.mp3".to_string()];
        assert_eq!(client.embeddings(&paths).unwrap(), None);
        let older = client.embeddings(&paths);
        assert!(matches!(older, Err(ApiError::NotFound(_))), "an older server, not a bad path: {older:?}");

        // The parent's proxy speaks `{error}` too: a peer it does not have
        // is an error at the first call, never every path split off as
        // missing. (A split would have met the stub's closed door, not
        // these words.)
        let gone = Client::new(&base).unwrap().with_peer(Some(9));
        let several: Vec<String> = (0..4).map(|i| format!("lib/{i}.mp3")).collect();
        match gone.embeddings(&several) {
            Err(ApiError::NotFound(what)) => assert!(what.contains("Peer not found"), "{what}"),
            other => panic!("no such peer, not four missing paths: {other:?}"),
        }
        recv(&seen);
        recv(&seen);
        let req = recv(&seen);
        let line = "POST /api/v1/federation/peers/9/api/api/v1/discovery/local/embeddings HTTP/1.1\r\n";
        assert!(req.starts_with(line), "{req}");
        assert_eq!(body_of(&req)["filePaths"].as_array().map(Vec::len), Some(4), "all four, in one call");

        // A vector of the wrong length is no vector; nor is bad base64.
        assert_eq!(floats(&vector_b64(&[1.0, 0.0, 0.0]), 2), None);
        assert_eq!(floats("not base64!", 2), None);
        assert_eq!(floats(&vector_b64(&[0.5; 3]), 0), Some(vec![0.5; 3]), "no dim named: any whole count");
    }

    #[test]
    fn a_peers_listing_and_metadata_go_through_the_parents_proxy() {
        let (base, seen) = canned(vec![
            json("200 OK", r#"["lib/Artist A/01.flac","lib/Artist A/02.flac"]"#),
            json(
                "200 OK",
                r#"{"lib/Artist A/01.flac":{"filepath":"lib/Artist A/01.flac","metadata":{"file-size":10}}}"#,
            ),
        ]);
        let client = Client::new(&base).unwrap().with_token(Some("parent".into())).with_peer(Some(3));
        let files = client.recursive_files("lib/Artist A").unwrap();
        assert_eq!(files, ["lib/Artist A/01.flac", "lib/Artist A/02.flac"]);
        let req = recv(&seen);
        assert!(
            req.starts_with("POST /api/v1/federation/peers/3/api/api/v1/file-explorer/recursive HTTP/1.1\r\n"),
            "{req}"
        );
        assert_eq!(body_of(&req), serde_json::json!({ "directory": "lib/Artist A" }));
        let found = client.metadata_batch(&files[..1]).unwrap();
        assert_eq!(found[&files[0]].as_ref().and_then(|m| m.file_size), Some(10));
        let req = recv(&seen);
        assert!(req.starts_with("POST /api/v1/federation/peers/3/api/api/v1/db/metadata/batch "), "{req}");
    }

    #[test]
    fn covers_come_sized_counted_and_marked_as_a_copy() {
        let (base, seen) = canned(vec![
            reply("200 OK", &[("Content-Type", "image/jpeg")], b"\xff\xd8\xff\xe0jpeg"),
            reply("200 OK", &[("Content-Type", "image/png")], b"\x89PNG"),
            reply("404 Not Found", &[], b""),
        ]);
        let meter = Arc::new(AtomicU64::new(0));
        let client = Client::new(&base)
            .unwrap()
            .with_token(Some("tok".into()))
            .with_meter(Some(meter.clone()))
            .copying(true);
        assert_eq!(client.album_art_sized("ab.jpeg", ArtSize::Large).unwrap(), b"\xff\xd8\xff\xe0jpeg");
        let req = recv(&seen).to_ascii_lowercase();
        assert!(req.starts_with("get /album-art/ab.jpeg?compress=l http/1.1\r\n"), "{req}");
        assert!(req.contains("x-access-token: tok\r\n") && req.contains("x-mstream-purpose: copy\r\n"), "{req}");

        let peer = Client::new(&base).unwrap().with_peer(Some(5));
        assert_eq!(peer.album_art_sized("cd.png", ArtSize::Full).unwrap(), b"\x89PNG");
        let req = recv(&seen);
        assert!(req.starts_with("GET /api/v1/federation/peers/5/art/cd.png HTTP/1.1\r\n"), "{req}");
        assert!(!req.to_ascii_lowercase().contains("x-mstream-purpose"), "only a copying client says so");

        assert!(matches!(client.album_art_sized("none.jpeg", ArtSize::Small), Err(ApiError::NotFound(_))));
        assert_eq!(meter.load(Ordering::Relaxed), 8, "the cover's bytes, not the other client's");
    }

    #[test]
    fn a_cards_plays_go_out_backdated_with_its_own_client_block() {
        let client_block = PlayClient {
            name: "mstream-core2".into(),
            version: Some("0.12.0".into()),
            instance_id: Some("0123456789abcdef".into()),
        };
        let play = CardPlay {
            id: "core2-0a0b0c0d0e0f-1122334455667788-42".into(),
            file_path: "lib/Artist A/Album B/01.flac".into(),
            peer_id: None,
            started_at: 1_760_000_000_000,
            ended_at: None,
            played_ms: 181_000,
            duration_ms: Some(181_500),
            outcome: PlayOutcome::Completed,
            source: Some(PlaySource::SmartPlaylist),
            session_id: None,
            pause_count: 1,
            track: Some(PlayTrack {
                title: Some("é".repeat(600)),
                hash: Some("h".repeat(129)),
                ..Default::default()
            }),
        };
        let (base, seen) = canned(vec![json(
            "200 OK",
            r#"{"accepted":["core2-0a0b0c0d0e0f-1122334455667788-42"],"duplicates":[],"rejected":[]}"#,
        )]);
        let client = Client::new(&base).unwrap().with_token(Some("tok".into()));
        let answer = client.report_card_plays(&client_block, std::slice::from_ref(&play)).unwrap();
        assert_eq!(answer.accepted, [play.id.clone()]);

        let req = recv(&seen);
        assert!(req.starts_with("POST /api/v1/stats/plays HTTP/1.1\r\n"), "{req}");
        let body = body_of(&req);
        assert_eq!(
            body["client"],
            serde_json::json!({ "name": "mstream-core2", "version": "0.12.0", "instanceId": "0123456789abcdef" })
        );
        let sent = &body["plays"][0];
        assert_eq!(sent["startedAt"], 1_760_000_000_000u64, "the card's own time, not now");
        assert_eq!(sent["filePath"], "lib/Artist A/Album B/01.flac");
        assert_eq!((sent["playedMs"].clone(), sent["durationMs"].clone()), (181_000.into(), 181_500.into()));
        assert_eq!(sent["outcome"], "completed");
        assert_eq!(sent["source"], "smart-playlist");
        assert_eq!(sent["pauseCount"], 1);
        assert!(sent.get("peerId").is_none() && sent.get("endedAt").is_none(), "absent, not null: {sent}");
        assert_eq!(sent["track"]["title"].as_str().map(|t| t.chars().count()), Some(512), "cut to the cap");
        assert!(sent["track"].get("hash").is_none(), "a hash too long to send is left out");

        // What the server's schema would refuse fails here, whole.
        let many = vec![play.clone(); PLAYS_PER_CALL + 1];
        assert!(matches!(plays_body(&client_block, &many), Err(ApiError::Config(_))));
        assert!(matches!(plays_body(&client_block, &[]), Err(ApiError::Config(_))));
        let long_id = CardPlay { id: "x".repeat(129), ..play.clone() };
        assert!(matches!(plays_body(&client_block, &[long_id]), Err(ApiError::Config(_))));
        let nameless = PlayClient { name: String::new(), ..client_block.clone() };
        assert!(matches!(plays_body(&nameless, std::slice::from_ref(&play)), Err(ApiError::Config(_))));
        assert_eq!(clip("aé😀b", 3), "aé", "a character never splits");

        // One play at a time, and the version a card's device.txt gives.
        assert_eq!(play_fault(&play), None);
        let pathless = CardPlay { file_path: String::new(), ..play.clone() };
        let far = CardPlay { file_path: "é".repeat(2049), ..play.clone() };
        let peerless = CardPlay { peer_id: Some(0), ..play.clone() };
        for refused in [pathless, far, peerless] {
            assert!(play_fault(&refused).is_some(), "{refused:?}");
            let body = plays_body(&client_block, &[play.clone(), refused]);
            assert!(matches!(body, Err(ApiError::Config(_))), "it fails the batch whole");
        }
        assert_eq!(play_version("0.12.0").as_deref(), Some("0.12.0"));
        assert_eq!(play_version(&"😀".repeat(16)).map(|v| v.chars().count()), Some(16), "32 units");
        assert_eq!((play_version(&"😀".repeat(17)), play_version("")), (None, None));
    }

    #[test]
    fn play_outcomes_and_sources_are_spelled_as_the_server_takes_them() {
        // mStream's `OUTCOMES` and `SOURCES` (src/stats/store.js), less the
        // `legacy` a client may not claim.
        let spelled = |v: serde_json::Value| v.as_str().unwrap_or_default().to_string();
        let outcomes = [PlayOutcome::Completed, PlayOutcome::Skipped, PlayOutcome::Stopped];
        let names: Vec<String> = outcomes.iter().map(|o| spelled(serde_json::to_value(o).unwrap())).collect();
        assert_eq!(names, ["completed", "skipped", "stopped"]);
        let sources = [
            PlaySource::Manual,
            PlaySource::Shuffle,
            PlaySource::Autodj,
            PlaySource::Playlist,
            PlaySource::SmartPlaylist,
            PlaySource::Auto,
            PlaySource::Carplay,
            PlaySource::Cast,
            PlaySource::Device,
            PlaySource::Other,
        ];
        let names: Vec<String> = sources.iter().map(|s| spelled(serde_json::to_value(s).unwrap())).collect();
        let wanted = [
            "manual", "shuffle", "autodj", "playlist", "smart-playlist", "auto", "carplay", "cast", "device", "other",
        ];
        assert_eq!(names, wanted);
    }
}
