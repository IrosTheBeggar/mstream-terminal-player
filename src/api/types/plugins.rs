//! The discovery plug-ins as the Discover room's connected-server rows use
//! them (docs/ux-contracts/discovery-page.md, clauses 100–149): the
//! listing that gates **Keep a copy** and **Open elsewhere**, the jobs a
//! copy runs as, the lookups that mark a row with its copy's state, the
//! collection destination a copy lands in, and the links a `links`
//! plug-in builds.
//!
//! Every shape is the server's (mStream's `docs/openapi.yaml`:
//! `/api/v1/discovery/plugins`, `…/plugins/{name}/jobs`, `…/plugin-jobs*`,
//! `…/collection/destination`, `…/plugins/{name}/resolve`) and as tolerant
//! as the rest of this module: a field a newer server adds is ignored, one
//! an older server leaves out reads as nothing. A job's `result` is kept as
//! the server's JSON and read through [`CopyResult`], one field at a time,
//! so one odd field never loses the rest.
//!
//! The pure halves live here too, ported with their cases from the
//! webapp (mStream `webapp/alpha/discover-jobs.js` `jobRowState`,
//! `manyCounts`, `resolveLayout`; `discovery-page.js` `nameKey`,
//! `jobForAlbum`): what a job's outcome IS ("done" is not "kept"), and
//! where a copy lands.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Deserializer;
use serde::de::DeserializeOwned;

use super::{int_bool, null_default, yes};

/// The copy plug-in (mStream's `federation-copy`): "Keep a copy".
pub const COPY_PLUGIN: &str = "federation-copy";
/// The capability a plug-in that brings files in declares.
pub const ACQUIRE: &str = "acquire";
/// The capability of a plug-in that builds "Open elsewhere" links.
pub const LINKS: &str = "links";
/// `POST …/plugin-jobs/lookup/batch` takes at most this many
/// recommendations a call (the webapp's `LOOKUP_BATCH_MAX`).
pub const LOOKUP_BATCH_MAX: usize = 100;

/// A whole number the server may send as a number or a numeric string;
/// anything else is none.
fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

/// A job's id: required in spirit, but a job the server lists without one
/// reads as 0 rather than failing the whole list.
fn job_id<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(lenient_i64(d)?.unwrap_or(0))
}

/// A peer id as a recommendation carries it: a number, or a numeric string
/// (the schema allows both).
fn peer_id_of(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// One field of a JSON object read as `T`, or `None` when it is absent,
/// null or not that shape — so one odd field never loses the others.
fn field<T: DeserializeOwned>(value: &serde_json::Value, key: &str) -> Option<T> {
    let v = value.get(key)?;
    if v.is_null() {
        return None;
    }
    serde_json::from_value(v.clone()).ok()
}

fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

// ── The listing: the gate ───────────────────────────────────────────────────

/// `GET /api/v1/discovery/plugins` — the plug-ins switched on for this
/// caller and whether this caller may start jobs at all.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DiscoveryPluginsResponse {
    #[serde(deserialize_with = "null_default")]
    pub plugins: Vec<DiscoveryPlugin>,
    /// The acquisition gate for THIS caller; a server that does not say
    /// is read as "not allowed" — hidden, never offered and refused.
    #[serde(deserialize_with = "null_default")]
    pub jobs: PluginJobsGate,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PluginJobsGate {
    #[serde(deserialize_with = "null_default")]
    pub allowed: bool,
}

/// One listed plug-in. The user listing holds only enabled, available
/// ones; a missing `enabled` / `available` reads that way.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DiscoveryPlugin {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub capabilities: Vec<String>,
    /// `server` · `user`: whose configuration it runs on.
    pub scope: Option<String>,
    /// The job scopes it acts on (`song` always; `album`, `artist`,
    /// `artist-missing`, `folder` where it copies more than one song). An
    /// older server that leaves the list out runs the song alone.
    #[serde(deserialize_with = "null_default")]
    pub scopes: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub settings: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub available: bool,
}

impl DiscoveryPlugin {
    /// Switched on, able to run, and declaring `capability`.
    pub fn has(&self, capability: &str) -> bool {
        self.enabled && self.available && self.capabilities.iter().any(|c| c == capability)
    }

    /// Whether a job of `scope` may be asked of it: the song always (the
    /// server's default list), the others only when listed.
    pub fn runs(&self, scope: JobScope) -> bool {
        scope == JobScope::Song || self.scopes.iter().any(|s| s == scope.as_str())
    }
}

impl DiscoveryPluginsResponse {
    /// The copy plug-in, when it is listed able to bring files in.
    pub fn copy_plugin(&self) -> Option<&DiscoveryPlugin> {
        self.plugins.iter().find(|p| p.name == COPY_PLUGIN && p.has(ACQUIRE))
    }

    /// The first plug-in that builds links — "Open elsewhere"'s.
    pub fn links_plugin(&self) -> Option<&DiscoveryPlugin> {
        self.plugins.iter().find(|p| p.has(LINKS))
    }

    /// The copy plug-in listed, running `scope`, and jobs allowed for this
    /// caller — the listing's half of the Keep a copy gate (clause 101).
    pub fn copies(&self, scope: JobScope) -> bool {
        self.jobs.allowed && self.copy_plugin().is_some_and(|p| p.runs(scope))
    }
}

// ── Jobs ────────────────────────────────────────────────────────────────────

/// What a job acts on (the server's `JOB_SCOPES`). Discover offers the
/// first four; a folder job is never started from here and never marks a
/// row (clause 104).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum JobScope {
    #[default]
    Song,
    Album,
    Artist,
    ArtistMissing,
    Folder,
}

impl JobScope {
    pub fn as_str(self) -> &'static str {
        match self {
            JobScope::Song => "song",
            JobScope::Album => "album",
            JobScope::Artist => "artist",
            JobScope::ArtistMissing => "artist-missing",
            JobScope::Folder => "folder",
        }
    }

    pub fn parse(word: &str) -> Option<JobScope> {
        Some(match word.trim() {
            "song" => JobScope::Song,
            "album" => JobScope::Album,
            "artist" => JobScope::Artist,
            "artist-missing" => JobScope::ArtistMissing,
            "folder" => JobScope::Folder,
            _ => return None,
        })
    }

    /// An album or artist job: keyed on names alone, not on the server it
    /// copies from (clause 121).
    pub fn wide(self) -> bool {
        matches!(self, JobScope::Album | JobScope::Artist | JobScope::ArtistMissing)
    }

    /// The scopes Discover offers and marks (never Folder).
    pub fn discover(self) -> bool {
        self != JobScope::Folder
    }
}

/// Where a job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    /// Missing, or a state a newer server added: never live, never kept.
    #[default]
    #[serde(other)]
    Unknown,
}

impl JobState {
    /// Queued or running: the job still moves.
    pub fn live(self) -> bool {
        matches!(self, JobState::Queued | JobState::Running)
    }

    /// Done, failed or cancelled — what "Clear finished" drops.
    pub fn finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
            JobState::Unknown => "unknown",
        }
    }
}

/// What a job was started with beyond its recommendation.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JobParams {
    /// Absent for a song job.
    pub scope: Option<String>,
    /// Where it lands, pinned when it was asked for (mStream since
    /// 2026-10-21): `None` a job from before the pin — it lands where the
    /// account's destination says; `Some(None)` there was nowhere to put
    /// a file; else the place, whatever the account has saved since.
    #[serde(deserialize_with = "pinned")]
    pub destination: Option<Option<CollectionDestination>>,
}

/// A pinned destination: `null` is nowhere, an object the place — and a
/// shape this client cannot read is no pin at all, never the whole job lost.
fn pinned<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<CollectionDestination>>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => Some(None),
        other => serde_json::from_value(other).ok().map(Some),
    })
}

/// One plug-in job (mStream's `DiscoveryPluginJob`). `recommendation` is
/// the server's normalised copy of what was sent — a retry sends it back
/// as it is.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginJob {
    #[serde(deserialize_with = "job_id")]
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub plugin: String,
    /// The job's key: the recording's for a song job, the album's or the
    /// artist's names for the wider scopes — never the server's (clause
    /// 121).
    pub key: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub recommendation: serde_json::Value,
    #[serde(deserialize_with = "null_default")]
    pub params: JobParams,
    #[serde(deserialize_with = "null_default")]
    pub state: JobState,
    /// 0..1, `None` with no estimate (never drawn as a fake 0%).
    pub progress: Option<f64>,
    /// The plug-in's own progress line — the step a running copy shows.
    pub status_text: Option<String>,
    pub result: Option<serde_json::Value>,
    /// The server's own words for a failure.
    pub error: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub attempts: u32,
    #[serde(deserialize_with = "null_default")]
    pub cancel_requested: bool,
    /// ms epochs.
    #[serde(deserialize_with = "lenient_i64")]
    pub created_at: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub updated_at: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub started_at: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub finished_at: Option<i64>,
}

impl PluginJob {
    /// The job's scope: a song job names none; an unknown word is `None`
    /// (a newer server's scope this client does not draw).
    pub fn scope(&self) -> Option<JobScope> {
        match self.params.scope.as_deref() {
            None | Some("") => Some(JobScope::Song),
            Some(word) => JobScope::parse(word),
        }
    }

    pub fn live(&self) -> bool {
        self.state.live()
    }

    /// Running with a cancel asked: winding down, no Cancel left to press.
    pub fn stopping(&self) -> bool {
        self.state == JobState::Running && self.cancel_requested
    }

    /// The server the job copies from — the recommendation's `peer.id`.
    pub fn peer_id(&self) -> Option<i64> {
        peer_id_of(self.recommendation.get("peer").and_then(|p| p.get("id")))
    }

    /// That server's name: the recommendation's, else the result's.
    pub fn peer_name(&self) -> Option<String> {
        self.recommendation
            .get("peer")
            .and_then(|p| text(p, "name"))
            .or_else(|| self.result.as_ref().and_then(|r| r.get("peer")).and_then(|p| text(p, "name")))
    }

    /// A text field of the recommendation (`title`, `artist`, `album`).
    pub fn rec_text(&self, key: &str) -> Option<String> {
        text(&self.recommendation, key)
    }

    /// An album or artist job of ANOTHER server than `peer`: its key
    /// carries no server, so a lookup can hand one server's row another's
    /// copy of the same album (the webapp's `otherPeerJob`). A song job is
    /// the recording wherever it came from, and a job or a row that names
    /// no server is never another's.
    pub fn other_peer(&self, peer: Option<i64>) -> bool {
        let Some(peer) = peer else { return false };
        if !self.scope().is_some_and(JobScope::wide) {
            return false;
        }
        self.peer_id().is_some_and(|theirs| theirs != peer)
    }

    /// The result, read field by field.
    pub fn copy_result(&self) -> CopyResult {
        self.result.as_ref().map(CopyResult::from_value).unwrap_or_default()
    }

    /// A failure saying the server it copies from refuses copies with this
    /// server's key (federation-copy's "does not allow copies"): Retry
    /// would meet the same refusal.
    pub fn refused_by_peer(&self) -> bool {
        self.state == JobState::Failed
            && self.error.as_deref().is_some_and(|e| e.to_ascii_lowercase().contains("does not allow copies"))
    }

    /// A failure saying the server is no longer paired with that peer: ask
    /// the room again rather than retrying (clause 128).
    pub fn unpaired(&self) -> bool {
        self.state == JobState::Failed
            && self.error.as_deref().is_some_and(|e| e.to_ascii_lowercase().contains("no longer paired"))
    }

    /// What the job's row says — the webapp's `jobRowState` for the copy
    /// plug-in, with its 2026-10-19 fixes: "done" is not "kept" (clause
    /// 120).
    pub fn outcome(&self) -> JobOutcome {
        match self.state {
            JobState::Queued => return JobOutcome::Queued,
            JobState::Running => {
                return JobOutcome::Running {
                    progress: self.progress.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0)),
                    step: self.status_text.clone().filter(|s| !s.trim().is_empty()),
                    stopping: self.cancel_requested,
                };
            }
            JobState::Unknown => return JobOutcome::Unknown,
            _ => {}
        }
        let result = self.copy_result();
        let many = result.many();
        match self.state {
            JobState::Failed => {
                if self.refused_by_peer() {
                    return JobOutcome::Refused { many };
                }
                JobOutcome::Failed { error: self.error.clone().filter(|e| !e.trim().is_empty()), many }
            }
            JobState::Cancelled => JobOutcome::Cancelled { many },
            JobState::Done => match many {
                Some(n) => many_outcome(&result, n),
                None => song_outcome(&result),
            },
            JobState::Queued | JobState::Running | JobState::Unknown => JobOutcome::Unknown,
        }
    }
}

/// A finished many-song job (album, artist, artist-missing): where it
/// stopped wins over what landed; then copied, failed, already here,
/// nothing (discover-jobs.js's order). ANY stop word stops it — "in your
/// collection" wants `stopped` null: a `cancelled` on a done row is the
/// cancel that landed as the run ended, and a word this client does not
/// know is a stop all the same, in neutral words, never kept.
fn many_outcome(result: &CopyResult, n: ManyCounts) -> JobOutcome {
    match result.stopped.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
        Some("cancelled") => return JobOutcome::Cancelled { many: Some(n) },
        Some(word) => {
            return match StopReason::parse(word) {
                StopReason::Refused => JobOutcome::Refused { many: Some(n) },
                why => JobOutcome::Stopped { why, many: n },
            };
        }
        None => {}
    }
    if n.copied > 0 {
        return JobOutcome::CopiedMany(n);
    }
    // Copied 0 with failures is a failure, never "already yours".
    if n.failed > 0 || n.albums_failed > 0 {
        return JobOutcome::Failed { error: None, many: Some(n) };
    }
    if n.skipped > 0 || n.albums_owned > 0 {
        return JobOutcome::Owned { filepath: None, many: Some(n) };
    }
    JobOutcome::Nothing { appearances: n.albums_appearance, many: n }
}

/// A finished song job: refused by the peer, copied, already owned, a file
/// in the way, or a result this client cannot read. A peer that refuses
/// copies ends a song's job `done` with `stopped: "refused"` (mStream since
/// 2026-10-21; older servers fail it, `refused_by_peer`).
fn song_outcome(result: &CopyResult) -> JobOutcome {
    if result.stopped.as_deref().map(str::trim) == Some("refused") {
        return JobOutcome::Refused { many: None };
    }
    if let Some(copied) = result.copied.as_ref().and_then(|c| c.filepath.clone()) {
        let vpath = result.copied.as_ref().and_then(|c| c.vpath.clone());
        return JobOutcome::Copied { filepath: copied, vpath };
    }
    match result.skipped.as_deref() {
        Some("owned") => JobOutcome::Owned {
            filepath: result.existing.as_ref().and_then(|e| e.filepath.clone()),
            many: None,
        },
        Some("exists") => JobOutcome::Exists { filepath: result.filepath.clone() },
        _ => JobOutcome::Done,
    }
}

/// Why a many-song job stopped early (`result.stopped`). `cancelled` is
/// the job's own state, not one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The other server's transfer limit.
    Quota,
    /// It stayed too busy to send more.
    Busy,
    /// It went away.
    Peer,
    /// It refuses copies with this server's key — no Retry.
    Refused,
    /// A reason this client does not know (a newer server's): stopped
    /// early all the same — neutral words, Retry.
    Other,
}

impl StopReason {
    /// A stop word, never empty: what this client does not know is
    /// [`StopReason::Other`].
    pub fn parse(word: &str) -> StopReason {
        match word.trim() {
            "quota" => StopReason::Quota,
            "busy" => StopReason::Busy,
            "peer" => StopReason::Peer,
            "refused" => StopReason::Refused,
            _ => StopReason::Other,
        }
    }
}

/// What a job's row says (clause 120). `many` carries a many-song job's
/// counts — numbers the Keep and Copies modals show, never the room.
#[derive(Debug, Clone, PartialEq)]
pub enum JobOutcome {
    /// Waiting for a free slot (the copy plug-in runs one at a time).
    Queued,
    /// `progress` 0..1, `None` with no estimate; `step` the plug-in's own
    /// line; `stopping` once a cancel was asked.
    Running { progress: Option<f64>, step: Option<String>, stopping: bool },
    /// A song landed: in your collection, at `filepath` (vpath form).
    Copied { filepath: String, vpath: Option<String> },
    /// An album or artist copy that copied something and was not stopped
    /// early: in your collection.
    CopiedMany(ManyCounts),
    /// Nothing copied, everything already here: already yours. A song
    /// names where the library has it, when the server said.
    Owned { filepath: Option<String>, many: Option<ManyCounts> },
    /// A song skipped because a file already sits at its path — nothing
    /// overwritten (Retry).
    Exists { filepath: Option<String> },
    /// The job failed (the server's words), or a many-song job copied
    /// nothing and something failed.
    Failed { error: Option<String>, many: Option<ManyCounts> },
    /// Stopped early by the other server: the transfer limit, too busy, or
    /// gone (Retry takes only the gaps).
    Stopped { why: StopReason, many: ManyCounts },
    /// The other server refuses copies with this server's key: a song job
    /// that failed so, or a many-song job stopped so. No Retry anywhere.
    Refused { many: Option<ManyCounts> },
    /// Cancelled — a many-song job keeps what landed.
    Cancelled { many: Option<ManyCounts> },
    /// A many-song job with nothing copied, skipped or failed; an artist
    /// who only appears on the other server's albums names how many.
    Nothing { appearances: usize, many: ManyCounts },
    /// Done with a result this client cannot read.
    Done,
    /// A state this client does not know.
    Unknown,
}

impl JobOutcome {
    /// Still moving.
    pub fn live(&self) -> bool {
        matches!(self, JobOutcome::Queued | JobOutcome::Running { .. })
    }

    /// In your collection: something was copied, and nothing stopped it —
    /// clause 120 in one word, which the tests pin; the drawing matches the
    /// two outcomes by name.
    #[cfg(test)]
    pub fn kept(&self) -> bool {
        matches!(self, JobOutcome::Copied { .. } | JobOutcome::CopiedMany(_))
    }

    /// What a finished job offers (the Copies list's row action, and the
    /// Keep a copy window's wherever its own table agrees —
    /// `keep::window_action`): Retry after a failure, a stop or a file in
    /// the way; Copy again after a many-song copy (it takes only the gaps);
    /// Start again after a cancel; Play your copy after a song landed;
    /// nothing after a refusal — the server would refuse again.
    pub fn action(&self) -> Option<JobAction> {
        match self {
            JobOutcome::Queued => Some(JobAction::Cancel),
            JobOutcome::Running { stopping, .. } => (!stopping).then_some(JobAction::Cancel),
            JobOutcome::Copied { .. } => Some(JobAction::Play),
            JobOutcome::CopiedMany(_) => Some(JobAction::CopyAgain),
            JobOutcome::Exists { .. } | JobOutcome::Failed { .. } | JobOutcome::Stopped { .. } => {
                Some(JobAction::Retry)
            }
            JobOutcome::Cancelled { .. } => Some(JobAction::StartAgain),
            JobOutcome::Owned { .. }
            | JobOutcome::Refused { .. }
            | JobOutcome::Nothing { .. }
            | JobOutcome::Done
            | JobOutcome::Unknown => None,
        }
    }

    /// The room's word for it (clause 113): a mark, or none.
    pub fn mark(&self) -> Option<JobMark> {
        Some(match self {
            JobOutcome::Queued => JobMark::Waiting,
            JobOutcome::Running { stopping: false, .. } => JobMark::Copying,
            JobOutcome::Running { stopping: true, .. } => JobMark::Stopping,
            JobOutcome::Copied { .. } | JobOutcome::CopiedMany(_) => JobMark::Kept,
            JobOutcome::Owned { .. } | JobOutcome::Exists { .. } => JobMark::Yours,
            JobOutcome::Failed { .. } => JobMark::Failed,
            JobOutcome::Stopped { .. } => JobMark::Stopped,
            JobOutcome::Refused { .. } => JobMark::Refused,
            JobOutcome::Cancelled { .. } => JobMark::Cancelled,
            JobOutcome::Nothing { .. } | JobOutcome::Done | JobOutcome::Unknown => return None,
        })
    }

    /// A GOLD row in the Copies list: something wants the user.
    pub fn troubled(&self) -> bool {
        matches!(self, JobOutcome::Failed { .. } | JobOutcome::Stopped { .. } | JobOutcome::Refused { .. })
    }
}

/// The one thing a finished (or live) job's row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobAction {
    Cancel,
    Retry,
    CopyAgain,
    StartAgain,
    Play,
}

/// A copy's state as the room draws it on a row's LEAD and a tile's artist
/// row: a glyph or a word, never a number (clause 113). Ordered by
/// precedence — when several jobs speak for one row, the lowest wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobMark {
    /// Running.
    Copying,
    /// Queued: waiting for a free slot.
    Waiting,
    /// Running with a cancel asked.
    Stopping,
    /// Failed (or nothing copied and something failed).
    Failed,
    /// Stopped early by the other server.
    Stopped,
    /// The other server refuses copies.
    Refused,
    /// In your collection.
    Kept,
    /// Already yours: nothing copied, all already here (or a file in the
    /// way).
    Yours,
    /// Cancelled.
    Cancelled,
}

impl JobMark {
    /// Its rank among the marks speaking for one row: live › failed /
    /// stopped / refused › kept › yours › cancelled (the three GOLD marks
    /// share a rank; the newest job decides among them).
    pub fn rank(self) -> u8 {
        match self {
            JobMark::Copying => 0,
            JobMark::Waiting => 1,
            JobMark::Stopping => 2,
            JobMark::Failed | JobMark::Stopped | JobMark::Refused => 3,
            JobMark::Kept => 4,
            JobMark::Yours => 5,
            JobMark::Cancelled => 6,
        }
    }
}

// ── A job's result ──────────────────────────────────────────────────────────

/// A song copy's landed file (`result.copied`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CopiedFile {
    pub vpath: Option<String>,
    /// vpath form: the library's name first.
    pub filepath: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub bytes: Option<f64>,
    pub track_id: Option<f64>,
}

/// Where the library already holds a song (`result.existing`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ExistingFile {
    pub filepath: Option<String>,
    pub by: Option<String>,
}

/// A many-song job's per-song lists — each capped at 500 entries, so only
/// [`CopyCounts`] holds the whole numbers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SongsAccount {
    pub total: Option<f64>,
    #[serde(deserialize_with = "null_default")]
    pub copied: Vec<serde_json::Value>,
    #[serde(deserialize_with = "null_default")]
    pub skipped: Vec<serde_json::Value>,
    #[serde(deserialize_with = "null_default")]
    pub failed: Vec<serde_json::Value>,
}

/// A many-song job's whole numbers (`result.counts`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CopyCounts {
    pub total: Option<f64>,
    pub copied: Option<f64>,
    pub skipped: Option<f64>,
    pub failed: Option<f64>,
}

/// One album an artist job walked.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct WalkedAlbum {
    pub name: Option<String>,
    pub songs: Option<SongsAccount>,
    pub stopped: Option<String>,
    /// Its listing failed.
    pub error: Option<String>,
}

/// One album an artist job left out, and why: `owned` (only-missing: the
/// library has it) or `appearance` (the artist only guests on it).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LeftOutAlbum {
    pub name: Option<String>,
    pub why: Option<String>,
}

/// The server a job copied from, as its result names it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JobPeer {
    pub name: Option<String>,
}

/// A copy plug-in job's `result`, read one field at a time
/// ([`CopyResult::from_value`]): the song shapes (`copied`, or `skipped`
/// with `existing` / `filepath`) and the many-song shapes (`songs`,
/// `counts`, an artist walk's `albums` and `skippedAlbums`, `stopped`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CopyResult {
    pub scope: Option<String>,
    pub copied: Option<CopiedFile>,
    /// `owned` · `exists` (a song job).
    pub skipped: Option<String>,
    pub existing: Option<ExistingFile>,
    pub filepath: Option<String>,
    pub songs: Option<SongsAccount>,
    pub counts: Option<CopyCounts>,
    /// `None` when the result carries no walk at all (not an empty one).
    pub albums: Option<Vec<WalkedAlbum>>,
    pub skipped_albums: Option<Vec<LeftOutAlbum>>,
    pub stopped: Option<String>,
    pub peer: Option<JobPeer>,
    pub destination: Option<CollectionDestination>,
    pub truncated: bool,
}

impl CopyResult {
    pub fn from_value(value: &serde_json::Value) -> CopyResult {
        // A song copy's `copied` is one file; a many-song result's copied
        // songs live under `songs.copied`, never at the top.
        let copied = value.get("copied").filter(|v| v.is_object()).and_then(|_| field(value, "copied"));
        CopyResult {
            scope: text(value, "scope"),
            copied,
            skipped: value.get("skipped").and_then(serde_json::Value::as_str).map(str::to_string),
            existing: field(value, "existing"),
            filepath: text(value, "filepath"),
            songs: field(value, "songs"),
            counts: field(value, "counts"),
            albums: value.get("albums").and_then(serde_json::Value::as_array).map(|list| {
                list.iter().filter(|a| a.is_object()).filter_map(|a| serde_json::from_value(a.clone()).ok()).collect()
            }),
            skipped_albums: value.get("skippedAlbums").and_then(serde_json::Value::as_array).map(|list| {
                list.iter().filter(|a| a.is_object()).filter_map(|a| serde_json::from_value(a.clone()).ok()).collect()
            }),
            stopped: text(value, "stopped"),
            peer: field(value, "peer"),
            destination: field(value, "destination"),
            truncated: value.get("truncated").and_then(serde_json::Value::as_bool).unwrap_or(false),
        }
    }

    /// A many-song result (album, artist): every song accounted for —
    /// `songs.total` or `counts.total` a number.
    pub fn is_many(&self) -> bool {
        self.songs.as_ref().is_some_and(|s| s.total.is_some()) || self.counts.as_ref().is_some_and(|c| c.total.is_some())
    }

    /// A many-song result's numbers ([`ManyCounts`]), or `None` for a song
    /// result.
    pub fn many(&self) -> Option<ManyCounts> {
        self.is_many().then(|| ManyCounts::of(self))
    }

    /// Where a finished copy's files are (the webapp's `landedPath`): the
    /// folder they share — the copied ones; when nothing was copied and
    /// nothing failed or stopped it, the ones it found already in the
    /// library (already yours, a file at the target). `None` when nothing
    /// landed (failed, cancelled before a song, refused, nothing to copy)
    /// or the files share no library; a [`Landed`] with no library when
    /// songs were copied but the result lists none (the counts say so):
    /// the caller draws the destination the job ran with instead.
    pub fn landed(&self) -> Option<Landed> {
        let crumbs = |path: &str| -> Vec<String> { path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect() };
        let one_file = |path: Option<&String>| {
            let c = crumbs(path?);
            (c.len() >= 2).then(|| Landed { vpath: Some(c[0].clone()), dirs: c[1..c.len() - 1].to_vec(), file: c.last().cloned() })
        };
        if let Some(path) = self.copied.as_ref().and_then(|c| c.filepath.as_ref()) {
            return one_file(Some(path));
        }
        match self.skipped.as_deref() {
            Some("owned") => return one_file(self.existing.as_ref().and_then(|e| e.filepath.as_ref())),
            Some("exists") => return one_file(self.filepath.as_ref()),
            _ => {}
        }
        let n = self.many()?;
        let songs = self.songs.clone().unwrap_or_default();
        let found: Vec<&serde_json::Value> = if n.copied > 0 {
            songs.copied.iter().filter_map(|s| s.get("filepath")).collect()
        } else if self.stopped.is_none() && n.failed == 0 && n.albums_failed == 0 && n.skipped > 0 {
            songs.skipped.iter().filter_map(|s| s.get("at")).collect()
        } else {
            return None;
        };
        let lists: Vec<Vec<String>> =
            found.iter().filter_map(|p| p.as_str()).map(crumbs).filter(|c| c.len() >= 2).collect();
        let Some(first) = lists.first() else {
            return (n.copied > 0).then(Landed::default);
        };
        // The folders every file sits under, its own name left out.
        let mut shared = first.len() - 1;
        for c in &lists {
            shared = shared.min(c.len() - 1);
            if let Some(at) = (0..shared).find(|&i| c[i] != first[i]) {
                shared = at;
            }
        }
        (shared >= 1).then(|| Landed { vpath: Some(first[0].clone()), dirs: first[1..shared].to_vec(), file: None })
    }

    /// The songs a many-song copy left in the user's own library, as its
    /// lists name them (Play your copy after an album): the ones it copied,
    /// by their landed files, then the ones it found already there
    /// (skipped `owned`, where the library has each) — never a file in the
    /// way, which may be another song. The lists stop at 500, so this is
    /// what they name; empty where they name none (a result that carries
    /// only counts).
    pub fn kept_songs(&self) -> Vec<KeptSong> {
        let Some(songs) = self.songs.as_ref() else { return Vec::new() };
        let copied = songs.copied.iter().filter_map(|entry| {
            Some(KeptSong {
                from: text(entry, "from"),
                filepath: text(entry, "filepath")?,
                title: text(entry, "title"),
                artist: text(entry, "artist"),
                album: text(entry, "album"),
            })
        });
        let owned = songs.skipped.iter().filter(|entry| text(entry, "why").as_deref() == Some("owned")).filter_map(|entry| {
            Some(KeptSong { from: text(entry, "from"), filepath: text(entry, "at")?, title: None, artist: None, album: None })
        });
        copied.chain(owned).collect()
    }
}

/// One song a many-song copy left in the user's library
/// ([`CopyResult::kept_songs`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeptSong {
    /// The song's file on the server it came from — what the album's
    /// listing names it by.
    pub from: Option<String>,
    /// Its file in the user's library, vpath form.
    pub filepath: String,
    /// A copied song's tags as it landed; a song already there names none.
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// Where a finished copy's files are ([`CopyResult::landed`]): the library,
/// the folders under it, and one song's file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Landed {
    /// `None`: songs were copied, but the result names none of them.
    pub vpath: Option<String>,
    pub dirs: Vec<String>,
    pub file: Option<String>,
}

/// A many-song result's numbers (the webapp's `manyCounts`): songs from
/// `result.counts` — the whole numbers; the lists stop at 500 — else the
/// lists (an older server sends no counts); albums from an artist job's
/// walk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ManyCounts {
    pub total: u64,
    pub copied: u64,
    pub skipped: u64,
    pub failed: u64,
    /// Albums walked without an error.
    pub albums: usize,
    /// Albums whose listing failed.
    pub albums_failed: usize,
    /// Albums left out, for any reason.
    pub albums_left_out: usize,
    /// Left out as already here (only-missing).
    pub albums_owned: usize,
    /// Left out as a guest spot.
    pub albums_appearance: usize,
}

impl ManyCounts {
    fn of(result: &CopyResult) -> ManyCounts {
        let counts = result.counts.as_ref();
        let songs = result.songs.as_ref();
        let whole = |v: Option<f64>| v.filter(|n| n.is_finite() && *n >= 0.0).map(|n| n as u64);
        let num = |count: Option<f64>, list: Option<&Vec<serde_json::Value>>| {
            whole(count).unwrap_or_else(|| list.map(|l| l.len() as u64).unwrap_or(0))
        };
        let copied = num(counts.and_then(|c| c.copied), songs.map(|s| &s.copied));
        let skipped = num(counts.and_then(|c| c.skipped), songs.map(|s| &s.skipped));
        let failed = num(counts.and_then(|c| c.failed), songs.map(|s| &s.failed));
        let total = whole(counts.and_then(|c| c.total))
            .or_else(|| whole(songs.and_then(|s| s.total)))
            .unwrap_or(copied + skipped + failed);
        let walked = result.albums.as_deref().unwrap_or(&[]);
        let left = result.skipped_albums.as_deref().unwrap_or(&[]);
        let why = |w: &str| left.iter().filter(|a| a.why.as_deref() == Some(w)).count();
        ManyCounts {
            total,
            copied,
            skipped,
            failed,
            albums: walked.iter().filter(|a| a.error.is_none()).count(),
            albums_failed: walked.iter().filter(|a| a.error.is_some()).count(),
            albums_left_out: left.len(),
            albums_owned: why("owned"),
            albums_appearance: why("appearance"),
        }
    }
}

// ── Starting, cancelling, listing, clearing ─────────────────────────────────

/// `POST …/plugins/{name}/jobs` — `202 {job, created: true}`, or `200
/// {job, created: false}` with the live job that already covers the
/// recommendation (perhaps from another server, clause 122).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JobStarted {
    #[serde(deserialize_with = "null_default")]
    pub job: PluginJob,
    #[serde(deserialize_with = "null_default")]
    pub created: bool,
}

/// `POST …/plugin-jobs/{id}/cancel` — `cancelled` (a queued job, at once)
/// or `requested` (a running one, flagged).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JobCancelled {
    pub job: Option<PluginJob>,
    pub outcome: Option<String>,
}

/// A cancel's answer: the job as it now stands, or the server's 409 — it
/// had already finished (clause 133: refresh and show the outcome).
#[derive(Debug, Clone, PartialEq)]
pub enum CancelAnswer {
    Cancelled(Box<JobCancelled>),
    AlreadyFinished,
}

/// A start's answer: the job (new, or the live one covering it), or the
/// server's 409 `destination_changed` — the place the client showed is no
/// longer where files land, nothing started, and the one in force now as
/// a pin reads (clause 146): `Some(None)` nowhere, `Some(Some(place))`,
/// and `None` an answer that named none this client reads — what is drawn
/// stays until the view is read again.
#[derive(Debug, Clone, PartialEq)]
pub enum JobStartAnswer {
    Started(Box<JobStarted>),
    DestinationChanged(Option<Option<CollectionDestination>>),
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JobRunner {
    #[serde(deserialize_with = "null_default")]
    pub running: u32,
    #[serde(deserialize_with = "null_default")]
    pub active: bool,
}

/// `GET …/plugin-jobs` — the caller's jobs, newest first.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PluginJobList {
    #[serde(deserialize_with = "null_default")]
    pub jobs: Vec<PluginJob>,
    pub runner: Option<JobRunner>,
}

/// `POST …/plugin-jobs/clear` — how many finished rows went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct JobsCleared {
    #[serde(deserialize_with = "null_default")]
    pub removed: u64,
}

// ── Lookups ─────────────────────────────────────────────────────────────────

/// One recommendation's lookup (`…/plugin-jobs/lookup`, and each element
/// of the batch's `results`): its keys per scope and the caller's newest
/// job per plug-in and scope under them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JobLookup {
    /// The song's key.
    pub key: Option<String>,
    /// Every scope's key the recommendation's fields allow.
    #[serde(deserialize_with = "null_default")]
    pub keys: BTreeMap<String, serde_json::Value>,
    #[serde(deserialize_with = "null_default")]
    pub jobs: Vec<PluginJob>,
}

impl JobLookup {
    /// The keys a later job is matched to this row by: the song's, then
    /// the album's, the artist's and the only-missing one — never the
    /// folder's (a row's folder key is its own file's, and Discover offers
    /// no folder copy, clause 104).
    pub fn row_keys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |k: &str| {
            if !k.is_empty() && !out.iter().any(|o| o == k) {
                out.push(k.to_string());
            }
        };
        if let Some(key) = &self.key {
            add(key);
        }
        for (scope, key) in &self.keys {
            if scope == "folder" {
                continue;
            }
            if let Some(key) = key.as_str() {
                add(key);
            }
        }
        out
    }
}

/// `POST …/plugin-jobs/lookup/batch` — one result per recommendation, in
/// input order.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JobLookupBatch {
    #[serde(deserialize_with = "null_default")]
    pub results: Vec<JobLookup>,
}

// ── The collection destination ──────────────────────────────────────────────

/// The layout a library with no template of its own renders (mStream's
/// `DEFAULT_LAYOUT`, the album's credit first since 2026-10-19), where the
/// view names none.
pub const DEFAULT_LAYOUT: &str = "{{ALBUMARTIST}}/{{ALBUM}}";

/// The variables a layout may name (mStream's `LAYOUT_VARS`), where the
/// view names none: the six every server knows. [`SERVER_FOLDER`] (mStream
/// since 2026-10-22) counts only where the view lists it — an older server
/// refuses it at Save.
pub const LAYOUT_VARS: [&str; 6] = ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER"];

/// `{{SERVERFOLDER}}`: the folders a copied song sits in on the connected
/// server ([`server_folder`]) — the one variable whose value may be
/// several folders.
pub const SERVER_FOLDER: &str = "SERVERFOLDER";

/// The picker's "Server / Artist / Album": each server's copies apart, by
/// the album's credit too (the webapp's `PEER_LAYOUT`).
pub const PEER_LAYOUT: &str = "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}";

/// The picker's "Same folders as the server": each song where it sits on
/// the server it came from (the webapp's `SERVERFOLDER_LAYOUT`).
pub const SERVERFOLDER_LAYOUT: &str = "{{SERVERFOLDER}}";

/// The longest a layout may be, and the longest a path it renders may be
/// (mStream's path-template.js): past either the server refuses it.
pub const MAX_TEMPLATE_LEN: usize = 500;
pub const MAX_RESOLVED_LEN: usize = 500;

/// Where copies land: a library, a base folder in it, a layout rendered
/// per song (torrent path-template syntax).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CollectionDestination {
    #[serde(deserialize_with = "null_default")]
    pub vpath: String,
    #[serde(deserialize_with = "null_default")]
    pub base: String,
    #[serde(deserialize_with = "null_default")]
    pub layout: String,
    /// `default` · `user`.
    pub source: Option<String>,
}

/// `GET /api/v1/discovery/collection/destination` — the effective
/// destination (`None`: this caller may not put files anywhere — uploads
/// off, or no library) and what the picker needs (clauses 105, 142): the
/// libraries the account may upload to, in the server's order, each with
/// its own template; the layout a library without one renders; the
/// variables a layout may name. `PUT` answers the same view.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CollectionDestinationView {
    pub destination: Option<CollectionDestination>,
    pub saved: Option<CollectionDestination>,
    #[serde(deserialize_with = "null_default")]
    pub libraries: Vec<DestinationLibrary>,
    pub default_layout: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub variables: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DestinationLibrary {
    #[serde(deserialize_with = "null_default")]
    pub vpath: String,
    pub template: Option<String>,
}

impl CollectionDestinationView {
    /// The layout a library renders with nothing saved: its own template,
    /// else the server's default layout (destination.js `destinationFor`).
    pub fn layout_of(&self, vpath: &str) -> String {
        let template = self.libraries.iter().find(|l| l.vpath == vpath).and_then(|l| l.template.clone());
        template
            .filter(|t| !t.trim().is_empty())
            .or_else(|| self.default_layout.clone().filter(|l| !l.trim().is_empty()))
            .unwrap_or_else(|| DEFAULT_LAYOUT.to_string())
    }

    /// The library default — where copies land with nothing saved: the
    /// first library, no base folder, its layout. `None` with no library.
    pub fn library_default(&self) -> Option<CollectionDestination> {
        let first = self.libraries.first()?;
        Some(CollectionDestination {
            vpath: first.vpath.clone(),
            base: String::new(),
            layout: self.layout_of(&first.vpath),
            source: Some("default".into()),
        })
    }

    /// Whether a layout may name `name` (any case): the view's variables,
    /// else the six every server knows.
    pub fn knows(&self, name: &str) -> bool {
        let name = name.to_ascii_uppercase();
        if self.variables.is_empty() {
            LAYOUT_VARS.contains(&name.as_str())
        } else {
            self.variables.iter().any(|v| v.eq_ignore_ascii_case(&name))
        }
    }
}

/// A base folder's names, as the server normalises it (`normalizeBase`):
/// either separator, each name trimmed, empty ones gone.
pub fn base_folders(base: &str) -> Vec<String> {
    base.split(['/', '\\']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

impl CollectionDestination {
    /// The place as a start or a save sends it: its library, base folder and
    /// layout — its `source` is the server's to say.
    pub fn shown(&self) -> serde_json::Value {
        serde_json::json!({ "vpath": self.vpath, "base": self.base, "layout": self.layout })
    }

    /// The same place, as the jobs route reckons it for a start's
    /// `destination_changed` (destination.js `sameDestination`): the
    /// library, the base folder as normalised, the layout character for
    /// character; `source` is not compared.
    pub fn same_place(&self, other: &CollectionDestination) -> bool {
        self.vpath == other.vpath && base_folders(&self.base) == base_folders(&other.base) && self.layout == other.layout
    }

    /// Where a copy of these tags lands, as the modal's LANDS IN line
    /// draws it (clause 105; [`Landing::of`]): the library, the base
    /// folder's names, the rendered layout's — `copies › Boukmanflow ›
    /// Borrowed Tapes`; an empty layout is the default one, as the server
    /// would never keep one.
    pub fn landing(&self, tags: &LayoutTags, by_album: bool) -> Landing {
        let layout = if self.layout.trim().is_empty() { DEFAULT_LAYOUT } else { self.layout.as_str() };
        Landing::of(&self.vpath, &self.base, layout, tags, by_album, &|_| true)
    }
}

/// The tags a layout renders from (the server's `resolveTemplate`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutTags {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub album_artist: Option<String>,
    /// The connected server a copy comes from (`{{PEER}}`).
    pub peer: Option<String>,
    /// The folders the song sits in on that server, inside its library
    /// (`{{SERVERFOLDER}}`, [`server_folder`]): several, `/` between; none
    /// for a song at the library's root.
    pub server_folder: Option<String>,
    /// The variables that stand as a slot ([`SongTag`]) instead of a
    /// value: each song of the copy fills them from its own tags.
    pub slots: Vec<SongTag>,
}

/// A variable the server fills per song — from the copied file's own tags
/// (destination.js `tagsForLayout`), or from where the song sits on the
/// connected server (`{{SERVERFOLDER}}`): where a copy's songs need not
/// agree — a compilation's artists, an album's years, a genre this client
/// cannot see, an album spread over folders — LANDS IN and the PREVIEW
/// draw it as a slot, `‹artist›`, never a value that promises one folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SongTag {
    Artist,
    Year,
    Genre,
    /// `{{SERVERFOLDER}}`: where the song sits, not one of its tags.
    ServerFolder,
}

impl SongTag {
    /// The variable a layout names it by, upper case.
    fn of(name: &str) -> Option<SongTag> {
        match name {
            "ARTIST" => Some(SongTag::Artist),
            "YEAR" => Some(SongTag::Year),
            "GENRE" => Some(SongTag::Genre),
            SERVER_FOLDER => Some(SongTag::ServerFolder),
            _ => None,
        }
    }

    /// Its name in plain words — what the tests' `Landing::crumbs` writes
    /// inside `‹ ›`; a surface draws the locale's word.
    pub fn name(self) -> &'static str {
        match self {
            SongTag::Artist => "artist",
            SongTag::Year => "year",
            SongTag::Genre => "genre",
            SongTag::ServerFolder => "server folder",
        }
    }
}

/// The longest one folder name in a rendered layout may be (mStream's
/// path-template.js).
const MAX_SEGMENT_LEN: usize = 200;

/// One folder name from a tag, as the server cleans it (path-template.js
/// `sanitizeSegment`): path separators, reserved and control characters
/// become `-` a run at a time; `~`, `$HOME` and `${HOME}` each `-` (a tag
/// is not a shell string, and the server refuses both in a rendered path);
/// blank runs one space; no leading or trailing dots and blanks; at most
/// 200 characters.
pub fn sanitize_segment(raw: Option<&str>) -> String {
    let Some(raw) = raw else { return String::new() };
    let reserved = |c: char| matches!(c, '/' | '\\' | ':' | '*' | '?' | '<' | '>' | '|' | '"') || (c as u32) < 0x20;
    let mut folded = String::new();
    let mut in_reserved = false;
    for c in raw.chars() {
        if reserved(c) {
            if !in_reserved {
                folded.push('-');
            }
            in_reserved = true;
        } else {
            folded.push(c);
            in_reserved = false;
        }
    }
    let folded = fold_home(&folded);
    let mut out = String::new();
    let mut in_blank = false;
    for c in folded.chars() {
        if c.is_whitespace() {
            if !in_blank {
                out.push(' ');
            }
            in_blank = true;
        } else {
            out.push(c);
            in_blank = false;
        }
    }
    let trimmed = out.trim_matches(|c: char| c == '.' || c.is_whitespace());
    trimmed.chars().take(MAX_SEGMENT_LEN).collect()
}

/// `${HOME}`, `$HOME` where no letter, digit or `_` follows it, and `~`,
/// each one `-` — the server's `/\$\{HOME\}|\$HOME\b|~/g`.
fn fold_home(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let home = rest.strip_prefix("$HOME").filter(|after| !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'));
        if let Some(after) = rest.strip_prefix("${HOME}").or(home) {
            out.push('-');
            rest = after;
        } else {
            out.push(if c == '~' { '-' } else { c });
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// A value that is a path of folders (`{{SERVERFOLDER}}`), as the server
/// cleans it (path-template.js `sanitizeFolders`): split on either
/// separator, each name cleaned as [`sanitize_segment`] cleans a tag, the
/// ones left empty (`.` and `..` among them) gone, `/` between — it may add
/// folders to a path, never climb out of it.
pub fn sanitize_folders(raw: Option<&str>) -> String {
    let Some(raw) = raw else { return String::new() };
    raw.split(['/', '\\']).map(|name| sanitize_segment(Some(name))).filter(|name| !name.is_empty()).collect::<Vec<_>>().join("/")
}

/// A song's `{{SERVERFOLDER}}` (federation-copy.js `serverFolderOf`): the
/// folders its file sits in on the connected server, by its path there as
/// the copy knows it, inside that server's library — the path's names
/// trimmed, the empty ones and `.` gone (`normalizePeerPath`), the file's
/// name and the library's own (the first) dropped, the rest cleaned as
/// [`sanitize_folders`] cleans them: `peerlib/Boukmanflow/Borrowed
/// Tapes/02.mp3` → `Boukmanflow/Borrowed Tapes`; a song at the library's
/// root has none (`""`), which drops it like any empty variable.
pub fn server_folder(filepath: &str) -> String {
    let names: Vec<&str> = filepath.split(['/', '\\']).map(str::trim).filter(|name| !name.is_empty() && *name != ".").collect();
    let Some((_, dirs)) = names.split_last() else { return String::new() };
    sanitize_folders(Some(&dirs.iter().skip(1).copied().collect::<Vec<_>>().join("/")))
}

/// A song's file name as it lands (destination.js `safeFileName`): the
/// path's last name, reserved and control characters `-`, blank runs one
/// space, no leading or trailing dots and blanks; "track" when nothing is
/// left — the picker's preview draws it as the path's last crumb.
pub fn safe_file_name(path: &str) -> String {
    let last = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or_default();
    let name = sanitize_segment(Some(last));
    if name.is_empty() || name == ".." { "track".to_string() } else { name }
}

/// `layout` with the base folder `base` in front of it (the webapp's and
/// the phone's `foldBase`). The picker keeps no base folder of its own
/// since 2026-10-23 (a destination saved with one — older data — opens
/// as the start of the layout): the server files a song at base folder /
/// rendered layout, and so files it at the same place from base `""` and
/// this layout — except where a folder's name has a brace
/// ([`braced_folder`]: a layout reads it as a variable, or refuses it),
/// and past [`MAX_TEMPLATE_LEN`], where the layout is refused. No base
/// folder: `layout` as it is.
pub fn fold_base(base: &str, layout: &str) -> String {
    let folders = base_folders(base);
    if folders.is_empty() { layout.to_string() } else { format!("{}/{layout}", folders.join("/")) }
}

/// The first of `base`'s folders a layout cannot hold as that folder: one
/// whose name has a brace (`{{PEER}}` files under the server's name).
/// `None` when there is none.
pub fn braced_folder(base: &str) -> Option<String> {
    base_folders(base).into_iter().find(|folder| folder.contains(['{', '}']))
}

/// A layout's leading folders that name no variable — where every copy
/// lands whatever its tags (`From friends/{{ARTIST}}` → `From friends`,
/// each trimmed) — and the rest of it as typed (`{{ARTIST}}`): the
/// webapp's `layoutLead`.
pub fn layout_lead(layout: &str) -> (Vec<String>, String) {
    let mut folders = Vec::new();
    let mut start = 0;
    let ends = layout.char_indices().filter(|(_, c)| *c == '/' || *c == '\\').map(|(i, _)| i).chain(std::iter::once(layout.len()));
    for end in ends {
        let segment = &layout[start..end];
        if segment.contains(['{', '}']) {
            return (folders, layout[start..].to_string());
        }
        let name = segment.trim();
        if !name.is_empty() {
            folders.push(name.to_string());
        }
        start = end + 1;
    }
    (folders, String::new())
}

/// `layout` starting in `folder` instead of its own leading folders (the
/// picker's "Start from an existing folder…"; the webapp's
/// `startLayoutAt`): the folder's names, then the rest of the layout as
/// typed; an empty folder leaves the rest alone.
pub fn start_layout_at(folder: &str, layout: &str) -> String {
    let (_, rest) = layout_lead(layout);
    let mut parts = vec![base_folders(folder).join("/")];
    parts.push(rest);
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

/// Why the server would refuse a layout, or a path it renders (the
/// server's own codes, path-template.js `validateLayout` and
/// `validateResolvedPath`, as the webapp and the phone mirror them): the
/// picker's words come from these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutProblem {
    /// `empty_template`: nothing typed.
    EmptyTemplate,
    /// `template_too_long`: past [`MAX_TEMPLATE_LEN`].
    TemplateTooLong,
    /// `unbalanced_braces`: a brace outside a `{{NAME}}` token.
    UnbalancedBraces,
    /// `unknown_variable`: a token the view does not list, as typed.
    UnknownVariable { token: String },
    /// `invalid_chars`: a control character.
    InvalidChars,
    /// `absolute_template` / `absolute_path`: starts with a separator.
    Absolute,
    /// `empty_path`: the sample renders no folder.
    EmptyPath,
    /// `path_too_long`: the rendered path passes [`MAX_RESOLVED_LEN`].
    PathTooLong,
    /// `traversal`: a `..` folder.
    Traversal,
    /// `home_string`: `~`, `$HOME` or `${HOME}`.
    Home,
    /// `drive_letter` / `drive_letter_segment`: a folder like `C:`.
    DriveLetter,
}

/// `$HOME` where no letter, digit or `_` follows it, or `${HOME}` — the
/// server's `/\$HOME\b|\$\{HOME\}/`.
fn names_home(text: &str) -> bool {
    text.contains("${HOME}")
        || text.match_indices("$HOME").any(|(at, _)| {
            !text[at + "$HOME".len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        })
}

fn drive_letter(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':')
}

/// Why the server would refuse `path`, a rendered path inside the library
/// (path-template.js `validateResolvedPath`), or `Ok`.
pub fn validate_resolved_path(path: &str) -> Result<(), LayoutProblem> {
    if path.is_empty() {
        return Err(LayoutProblem::EmptyPath);
    }
    if path.chars().count() > MAX_RESOLVED_LEN {
        return Err(LayoutProblem::PathTooLong);
    }
    if path.chars().any(|c| (c as u32) < 0x20) {
        return Err(LayoutProblem::InvalidChars);
    }
    if path.starts_with(['/', '\\']) {
        return Err(LayoutProblem::Absolute);
    }
    if drive_letter(path) {
        return Err(LayoutProblem::DriveLetter);
    }
    if path.contains('~') || names_home(path) {
        return Err(LayoutProblem::Home);
    }
    for segment in path.split(['/', '\\']) {
        if segment == ".." {
            return Err(LayoutProblem::Traversal);
        }
        if drive_letter(segment) {
            return Err(LayoutProblem::DriveLetter);
        }
    }
    Ok(())
}

/// Why the server would refuse `layout` (path-template.js
/// `validateLayout`), or `Ok`: empty or too long, a stray brace, a
/// variable `known` refuses (the view's list, so an older server's refusal
/// of `{{SERVERFOLDER}}` is said here), a control character, a leading
/// separator, then the path the server's sample renders
/// ([`validate_resolved_path`] over Pink Floyd's Dark Side on Sam's
/// server, cut at [`MAX_RESOLVED_LEN`] as the server cuts it).
pub fn validate_layout(layout: &str, known: &dyn Fn(&str) -> bool) -> Result<(), LayoutProblem> {
    if layout.is_empty() {
        return Err(LayoutProblem::EmptyTemplate);
    }
    if layout.chars().count() > MAX_TEMPLATE_LEN {
        return Err(LayoutProblem::TemplateTooLong);
    }
    let pieces = pieces(layout);
    let stripped: String = pieces.iter().filter_map(|p| if let Piece::Text(text) = p { Some(*text) } else { None }).collect();
    if stripped.contains(['{', '}']) {
        return Err(LayoutProblem::UnbalancedBraces);
    }
    for piece in &pieces {
        if let Piece::Var { name, raw } = piece
            && !known(name)
        {
            return Err(LayoutProblem::UnknownVariable { token: (*raw).to_string() });
        }
    }
    if layout.chars().any(|c| (c as u32) < 0x20) {
        return Err(LayoutProblem::InvalidChars);
    }
    if layout.starts_with(['/', '\\']) {
        return Err(LayoutProblem::Absolute);
    }
    let sample = LayoutTags {
        artist: Some("Pink Floyd".into()),
        album: Some("The Dark Side of the Moon".into()),
        year: Some("1973".into()),
        genre: Some("Progressive Rock".into()),
        album_artist: Some("Pink Floyd".into()),
        peer: Some("Sam's server".into()),
        server_folder: Some("Pink Floyd/The Dark Side of the Moon".into()),
        slots: Vec::new(),
    };
    let rendered: Vec<String> = layout_folders(layout, &sample, known)
        .iter()
        .map(|folder| folder.iter().map(|(text, _)| text.as_str()).collect::<String>())
        .collect();
    let path: String = rendered.join("/").chars().take(MAX_RESOLVED_LEN).collect();
    validate_resolved_path(&path)
}

/// One piece of a layout as the server's scanner reads it: its own text,
/// or a variable — `{{`, blanks, a name (a letter or `_`, then letters,
/// digits, `_`), blanks, `}}` — with the name and the token as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Var { name: String, raw: &'a str },
}

/// A layout's pieces. Anything that is not a token is text, scanned on
/// from the next character, as a regex scan would.
fn pieces(layout: &str) -> Vec<Piece<'_>> {
    let token = |s: &str| -> Option<(String, usize)> {
        let inner = s.strip_prefix("{{")?;
        let start = inner.len() - inner.trim_start().len();
        let name: String = inner[start..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
            return None;
        }
        let after = &inner[start + name.len()..];
        let blanks = after.len() - after.trim_start().len();
        let len = 2 + start + name.len() + blanks + 2;
        after[blanks..].starts_with("}}").then_some((name, len))
    };
    let mut out = Vec::new();
    let mut rest = layout;
    while let Some(open) = rest.find("{{") {
        match token(&rest[open..]) {
            Some((name, len)) => {
                if open > 0 {
                    out.push(Piece::Text(&rest[..open]));
                }
                out.push(Piece::Var { name: name.to_ascii_uppercase(), raw: &rest[open..open + len] });
                rest = &rest[open + len..];
            }
            None => {
                out.push(Piece::Text(&rest[..open + 1]));
                rest = &rest[open + 1..];
            }
        }
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

/// A variable's value for these tags, cleaned as the server cleans it —
/// `{{SERVERFOLDER}}` a run of folders, `/` between; a name it has no tag
/// for renders nothing.
fn tag_value(name: &str, tags: &LayoutTags) -> String {
    match name {
        "ARTIST" => sanitize_segment(tags.artist.as_deref()),
        "ALBUM" => sanitize_segment(tags.album.as_deref()),
        "YEAR" => sanitize_segment(tags.year.as_deref()),
        "GENRE" => sanitize_segment(tags.genre.as_deref()),
        "ALBUMARTIST" => {
            sanitize_segment(tags.album_artist.as_deref().filter(|a| !a.is_empty()).or(tags.artist.as_deref()))
        }
        "PEER" => sanitize_segment(tags.peer.as_deref()),
        SERVER_FOLDER => sanitize_folders(tags.server_folder.as_deref()),
        _ => String::new(),
    }
}

/// What a run of a landing's folder is (clauses 105, 143).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    /// Words the folder's name holds as drawn.
    Text,
    /// A variable the view does not list, kept as typed — the PREVIEW's
    /// GOLD (the server's words decide at Save).
    Unknown,
    /// A variable each song fills from its own tags ([`SongTag`]), written
    /// `‹artist›`.
    Slot(SongTag),
}

/// A layout's folders as the server renders them (its `resolveLayout`,
/// which the webapp mirrors as discover-jobs.js does): each a run of
/// pieces — words, a variable `known` refuses kept as typed for the
/// picker's PREVIEW to draw GOLD (clause 143), or a variable the tags hold
/// as a slot ([`SongTag`]), which keeps its folder whatever a song brings. A
/// variable with no value drops out, blanks trim, empty folders go:
/// `{{ARTIST}}/{{ALBUM}}` → `Boukmanflow`, `Borrowed Tapes`; a value of
/// several folders (`{{SERVERFOLDER}}`) opens a folder at each `/`, as the
/// server splits the path once every variable is in — `Copy of
/// {{SERVERFOLDER}}` → `Copy of Boukmanflow`, `Borrowed Tapes`. The server
/// renders the real path and has the last word.
pub fn layout_folders(layout: &str, tags: &LayoutTags, known: &dyn Fn(&str) -> bool) -> Vec<Vec<(String, Run)>> {
    let mut folders: Vec<Vec<(String, Run)>> = vec![Vec::new()];
    let push = |folders: &mut Vec<Vec<(String, Run)>>, text: &str, run: Run| {
        if text.is_empty() {
            return;
        }
        let folder = folders.last_mut().expect("a folder");
        match folder.last_mut() {
            Some((have, kind)) if *kind == run && !matches!(run, Run::Slot(_)) => have.push_str(text),
            _ => folder.push((text.to_string(), run)),
        }
    };
    // Words, a folder opened at each separator.
    let words = |folders: &mut Vec<Vec<(String, Run)>>, text: &str| {
        for (i, part) in text.split(['/', '\\']).enumerate() {
            if i > 0 {
                folders.push(Vec::new());
            }
            push(folders, part, Run::Text);
        }
    };
    for piece in pieces(layout) {
        match piece {
            Piece::Var { name, raw } if !known(&name) => push(&mut folders, raw, Run::Unknown),
            Piece::Var { name, .. } => match SongTag::of(&name).filter(|slot| tags.slots.contains(slot)) {
                Some(slot) => push(&mut folders, &format!("‹{}›", slot.name()), Run::Slot(slot)),
                None => words(&mut folders, &tag_value(&name, tags)),
            },
            Piece::Text(text) => words(&mut folders, text),
        }
    }
    // Each folder trimmed at both ends, as the server trims a segment.
    for folder in &mut folders {
        if let Some((text, Run::Text)) = folder.first_mut() {
            *text = text.trim_start().to_string();
        }
        if let Some((text, Run::Text)) = folder.last_mut() {
            *text = text.trim_end().to_string();
        }
        folder.retain(|(text, _)| !text.is_empty());
    }
    folders.retain(|folder| !folder.is_empty());
    folders
}

/// The variables of `layout` this copy has no value for — their folders
/// drop out of the path (the webapp's `missingVars`): a variable `known`
/// takes, that is no slot of these tags, whose value renders nothing; each
/// once, in the layout's order, upper case.
fn missing_vars(layout: &str, tags: &LayoutTags, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for piece in pieces(layout) {
        if let Piece::Var { name, .. } = piece
            && known(&name)
            && !SongTag::of(&name).is_some_and(|slot| tags.slots.contains(&slot))
            && tag_value(&name, tags).is_empty()
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

/// Where a copy lands, as the Keep modal's LANDS IN and the picker's
/// preview draw it (the webapp's `scopeTarget`, clauses 105 and 143): the
/// library, then its folders — the base folder's names, what the layout
/// rendered — each a run of pieces ([`layout_folders`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Landing {
    pub library: String,
    pub folders: Vec<Vec<(String, Run)>>,
    /// An artist copy lands album by album: "one folder per album" stands
    /// after the folders.
    pub per_album: bool,
    /// A song's copy alone: a slot's note speaks of the one song's tags.
    pub one_song: bool,
    /// The variables this copy has no value for, left out of the path
    /// ([`missing_vars`]): the picker's preview names them.
    pub missing: Vec<String>,
}

impl Landing {
    /// A place rendered for one copy's tags. `by_album` — an artist copy —
    /// renders the layout up to its first folder naming `{{ALBUM}}`,
    /// `{{YEAR}}` or `{{SERVERFOLDER}}` (each album has its own) and marks
    /// the rest [`Landing::per_album`]; a layout naming no album folder
    /// adds nothing (every album shares that folder).
    pub fn of(vpath: &str, base: &str, layout: &str, tags: &LayoutTags, by_album: bool, known: &dyn Fn(&str) -> bool) -> Landing {
        let mut folders: Vec<Vec<(String, Run)>> = base_folders(base).into_iter().map(|f| vec![(f, Run::Text)]).collect();
        let raw: Vec<&str> = layout.split(['/', '\\']).collect();
        let album_folder = |folder: &&str| {
            pieces(folder)
                .iter()
                .any(|p| matches!(p, Piece::Var { name, .. } if name == "ALBUM" || name == "YEAR" || name == SERVER_FOLDER))
        };
        let cut = if by_album { raw.iter().position(album_folder) } else { None };
        let rendered = match cut {
            Some(at) => raw[..at].join("/"),
            None => layout.to_string(),
        };
        folders.extend(layout_folders(&rendered, tags, known));
        let missing = missing_vars(&rendered, tags, known);
        Landing { library: vpath.to_string(), folders, per_album: cut.is_some(), one_song: false, missing }
    }

    /// A finished copy's folder ([`CopyResult::landed`]): real folders,
    /// never a slot.
    pub fn landed(vpath: &str, dirs: &[String]) -> Landing {
        let folders = dirs.iter().map(|d| vec![(d.clone(), Run::Text)]).collect();
        Landing { library: vpath.to_string(), folders, per_album: false, one_song: false, missing: Vec::new() }
    }

    /// The path's length past the library, in characters, as the server
    /// measures a rendered path against [`MAX_RESOLVED_LEN`] (a slot's
    /// words standing in for its value).
    pub fn rendered_len(&self) -> usize {
        self.words(&|slot| format!("‹{}›", slot.name()))[1..].join("/").chars().count()
    }

    /// The library, then each folder as plain words, a slot `‹artist›` —
    /// the tests' words; a surface words its slots in its locale
    /// ([`Landing::words`]).
    #[cfg(test)]
    pub fn crumbs(&self) -> Vec<String> {
        self.words(&|slot| format!("‹{}›", slot.name()))
    }

    /// The library, then each folder as plain words, a slot as `slot`
    /// words it — a surface's own, in its locale.
    pub fn words(&self, slot: &dyn Fn(SongTag) -> String) -> Vec<String> {
        let mut out = vec![self.library.clone()];
        out.extend(self.folders.iter().map(|folder| {
            folder
                .iter()
                .map(|(text, run)| match run {
                    Run::Slot(s) => slot(*s),
                    _ => text.clone(),
                })
                .collect::<String>()
        }));
        out
    }

    /// The slots the path shows, each once, in the order it shows them —
    /// what the note with it names (the webapp's `scopeTarget` `slots`).
    pub fn slots(&self) -> Vec<SongTag> {
        let mut out: Vec<SongTag> = Vec::new();
        for (_, run) in self.folders.iter().flatten() {
            if let Run::Slot(slot) = run
                && !out.contains(slot)
            {
                out.push(*slot);
            }
        }
        out
    }
}

// ── Open elsewhere ──────────────────────────────────────────────────────────

/// One link a `links` plug-in built (mStream's links.js): `kind` is
/// `canonical` (the recording's own MusicBrainz ids) or `search`; `label`
/// the plug-in's words. The room shows the label, never the URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PluginLink {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub label: String,
    #[serde(deserialize_with = "null_default")]
    pub url: String,
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
}

impl PluginLink {
    /// Only an https link goes to the operating system's opener, which
    /// would run any other scheme (clause 140).
    pub fn is_web(&self) -> bool {
        self.url.trim().to_ascii_lowercase().starts_with("https://")
    }

    /// The CANONICAL section's; anything else is a search.
    pub fn canonical(&self) -> bool {
        self.kind == "canonical"
    }
}

/// `POST …/plugins/{name}/resolve` on a `links` plug-in.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PluginResolveResponse {
    pub result: Option<PluginLinksResult>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PluginLinksResult {
    #[serde(deserialize_with = "null_default")]
    pub links: Vec<PluginLink>,
}

// ── Facts for the Keep modal ────────────────────────────────────────────────

/// One row of `/api/v1/db/artists-albums` with the facts "Only the albums
/// you don't have" is reckoned from: the album artist, every credit,
/// whether it is a compilation (the [`super::Album`] the rooms read keeps
/// none of these).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ArtistAlbumRow {
    pub name: Option<String>,
    pub album_artist: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub artists: Vec<String>,
    #[serde(deserialize_with = "int_bool")]
    pub compilation: bool,
    pub track_count: Option<f64>,
    /// The album's year, and the earliest of its songs' (the V71
    /// aggregate) — where a lone missing album lands names it.
    pub year: Option<i32>,
    pub year_min: Option<i32>,
}

impl ArtistAlbumRow {
    /// Its year as the webapp's `missingOne` reads it: `year`, else
    /// `year_min`; none for 0.
    pub fn year_of(&self) -> Option<i32> {
        self.year.filter(|y| *y > 0).or(self.year_min.filter(|y| *y > 0))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ArtistAlbumRows {
    #[serde(deserialize_with = "null_default")]
    pub albums: Vec<ArtistAlbumRow>,
}

/// mStream's `src/db/name-key.js`: how the copy matches album names —
/// blank runs one space, trimmed, curly quotes straight, dashes one
/// hyphen, lower case (the webapp's `nameKey`, byte for byte).
pub fn name_key(raw: Option<&str>) -> String {
    let Some(raw) = raw else { return String::new() };
    let mut out = String::new();
    let mut blank = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            if !blank {
                out.push(' ');
            }
            blank = true;
            continue;
        }
        blank = false;
        out.push(match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}' | '\u{2212}' => '-',
            other => other,
        });
    }
    out.trim().to_lowercase()
}

/// What of an artist's albums the library already holds, by NAME, as the
/// copy itself reckons it (federation-copy's planArtistAlbums over owned.js
/// — the webapp's `dmArtistOwnership`), never by comparing counts:
/// `owned` the other server's albums whose name the library has; `missing`
/// the other server's albums that are the artist's own (album artist, or
/// a credit outside a compilation — an album they only appear on is never
/// copied) and whose name the library lacks: what "Only the albums you
/// don't have" copies (clause 107).
pub fn artist_ownership(artist: &str, remote: &[ArtistAlbumRow], local: &[ArtistAlbumRow]) -> (usize, usize) {
    let here = album_keys(local);
    let owned = remote
        .iter()
        .filter(|a| a.name.as_deref().is_some_and(|n| !n.trim().is_empty()))
        .filter(|a| here.contains(&name_key(a.name.as_deref())))
        .count();
    (owned, missing_albums(artist, remote, local).len())
}

/// The library's album names, folded.
fn album_keys(local: &[ArtistAlbumRow]) -> std::collections::BTreeSet<String> {
    local.iter().map(|a| name_key(a.name.as_deref())).filter(|k| !k.is_empty()).collect()
}

/// The other server's albums "Only the albums you don't have" copies
/// ([`artist_ownership`]'s `missing`): the artist's own, named, the library
/// lacking them — so a lone one is named where it lands, its year too
/// (clause 105).
pub fn missing_albums<'a>(artist: &str, remote: &'a [ArtistAlbumRow], local: &[ArtistAlbumRow]) -> Vec<&'a ArtistAlbumRow> {
    let who = name_key(Some(artist));
    let here = album_keys(local);
    let theirs = |a: &ArtistAlbumRow| {
        name_key(a.album_artist.as_deref()) == who
            || (!a.compilation && a.artists.iter().any(|n| name_key(Some(n)) == who))
    };
    remote
        .iter()
        .filter(|a| theirs(a) && !here.contains(&name_key(a.name.as_deref())))
        .filter(|a| a.name.as_deref().is_some_and(|n| !n.trim().is_empty()))
        .collect()
}

/// A finished artist or only-missing job as it speaks for ONE album — the
/// webapp's `jobForAlbum` (clause 115): its key sits on every row and tile
/// of the artist on that server, but its walk copied only the artist's own
/// albums and left out the ones they only guest on (and, only-missing,
/// the ones the library had). So it says, for `album`:
/// - walked, no error: that album's own account;
/// - its listing failed: nothing — unless the run stopped there (the last
///   walked): stopped;
/// - left out as `owned`: already yours;
/// - left out as a guest spot, never walked, or no walk to read: nothing.
///
/// A live or failed artist job, and every other scope, is the job itself.
pub fn job_for_album(job: &PluginJob, album: Option<&str>) -> Option<PluginJob> {
    let scope = job.scope();
    if !matches!(scope, Some(JobScope::Artist | JobScope::ArtistMissing)) || job.state != JobState::Done {
        return Some(job.clone());
    }
    let result = job.copy_result();
    let want = name_key(album);
    if want.is_empty() || (result.albums.is_none() && result.skipped_albums.is_none()) {
        return None;
    }
    let walked = result.albums.as_deref().unwrap_or(&[]);
    let left = result.skipped_albums.as_deref().unwrap_or(&[]);
    let raw = job.result.clone().unwrap_or(serde_json::Value::Null);
    let view = |songs: Option<&SongsAccount>, stopped: Option<&str>, extra: Option<serde_json::Value>| {
        let songs = songs.cloned().unwrap_or_default();
        let total = songs
            .total
            .filter(|t| t.is_finite())
            .unwrap_or((songs.copied.len() + songs.skipped.len() + songs.failed.len()) as f64);
        let mut result = serde_json::json!({
            "scope": raw.get("scope").cloned().unwrap_or(serde_json::Value::String(scope.map(JobScope::as_str).unwrap_or("artist").to_string())),
            "songs": { "total": total, "copied": songs.copied, "skipped": songs.skipped, "failed": songs.failed },
            "counts": { "total": total, "copied": songs.copied.len(), "skipped": songs.skipped.len(), "failed": songs.failed.len() },
            "stopped": stopped,
            "peer": raw.get("peer").cloned().unwrap_or(serde_json::Value::Null),
        });
        if let (Some(extra), Some(obj)) = (extra, result.as_object_mut())
            && let Some(more) = extra.as_object()
        {
            for (k, v) in more {
                obj.insert(k.clone(), v.clone());
            }
        }
        PluginJob { result: Some(result), ..job.clone() }
    };
    if let Some(at) = walked.iter().position(|a| name_key(a.name.as_deref()) == want) {
        let one = &walked[at];
        let last = at + 1 == walked.len();
        if one.error.is_some() {
            return match (&result.stopped, last) {
                (Some(stopped), true) => Some(view(None, Some(stopped), None)),
                _ => None,
            };
        }
        // No per-album account: the walk's whole.
        let Some(songs) = one.songs.as_ref() else { return Some(job.clone()) };
        return Some(view(Some(songs), one.stopped.as_deref(), None));
    }
    let skipped = left.iter().find(|a| name_key(a.name.as_deref()) == want)?;
    if skipped.why.as_deref() == Some("owned") {
        let extra = serde_json::json!({ "skippedAlbums": [{ "name": skipped.name, "why": "owned" }] });
        return Some(view(None, None, Some(extra)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(over: serde_json::Value) -> PluginJob {
        let mut base = serde_json::json!({
            "id": 1, "plugin": COPY_PLUGIN, "state": "queued", "progress": null, "statusText": null,
            "result": null, "error": null, "cancelRequested": false, "createdAt": 1_760_000_000_000_i64,
            "recommendation": { "artist": "Neon Harbor", "title": "Salt & Static", "peer": { "id": 1, "name": "Sam's server" } },
        });
        for (k, v) in over.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    /// An album result as federation-copy reports it (the webapp test's `many`).
    fn many(over: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "scope": "album", "album": { "name": "Night Ferry", "artist": "Nova" },
            "songs": { "total": 4, "copied": [{ "from": "a" }, { "from": "b" }], "skipped": [{ "from": "c", "why": "owned" }], "failed": [] },
            "bytes": 10, "stopped": null, "peer": { "id": 1, "name": "Sam's server" },
        });
        for (k, v) in over.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    #[test]
    fn the_listing_gates_on_the_copy_plugin_its_scopes_and_the_jobs_flag() {
        let json = serde_json::json!({
            "plugins": [
                { "name": "links", "title": "Open elsewhere", "capabilities": ["links"], "scope": "server", "enabled": true, "scopes": ["song"], "settings": [], "available": true },
                { "name": "federation-copy", "capabilities": ["acquire"], "scopes": ["song", "album", "artist", "artist-missing", "folder"] },
                { "name": "youtube", "capabilities": ["acquire", "lookup"], "available": false }
            ],
            "jobs": { "allowed": true }
        });
        let listing: DiscoveryPluginsResponse = serde_json::from_value(json).unwrap();
        assert_eq!(listing.copy_plugin().map(|p| p.name.as_str()), Some(COPY_PLUGIN));
        assert_eq!(listing.links_plugin().map(|p| p.name.as_str()), Some("links"));
        for scope in [JobScope::Song, JobScope::Album, JobScope::Artist, JobScope::ArtistMissing] {
            assert!(listing.copies(scope), "{scope:?}");
        }
        // Jobs refused for this caller: hidden, whatever is listed.
        let mut refused = listing.clone();
        refused.jobs.allowed = false;
        assert!(!refused.copies(JobScope::Song));
        // An older server that lists no scopes runs the song alone.
        let old: DiscoveryPluginsResponse = serde_json::from_value(serde_json::json!({
            "plugins": [{ "name": "federation-copy", "capabilities": ["acquire"] }], "jobs": { "allowed": true }
        }))
        .unwrap();
        assert!(old.copies(JobScope::Song) && !old.copies(JobScope::Album));
        // No jobs block at all reads as not allowed; an unavailable plug-in is not there.
        let bare: DiscoveryPluginsResponse =
            serde_json::from_value(serde_json::json!({ "plugins": [{ "name": "federation-copy", "capabilities": ["acquire"] }] })).unwrap();
        assert!(!bare.copies(JobScope::Song));
        let off: DiscoveryPluginsResponse = serde_json::from_value(serde_json::json!({
            "plugins": [{ "name": "federation-copy", "capabilities": ["acquire"], "available": false }], "jobs": { "allowed": true }
        }))
        .unwrap();
        assert!(off.copy_plugin().is_none());
        assert_eq!(serde_json::from_value::<DiscoveryPluginsResponse>(serde_json::json!({ "plugins": null, "jobs": null })).unwrap(), DiscoveryPluginsResponse::default());
    }

    #[test]
    fn a_job_reads_tolerantly_and_names_its_scope_and_server() {
        let j = job(serde_json::json!({ "params": { "scope": "album" }, "id": "42", "createdAt": "17" }));
        assert_eq!((j.id, j.scope(), j.created_at), (42, Some(JobScope::Album), Some(17)));
        assert_eq!((j.peer_id(), j.peer_name().as_deref()), (Some(1), Some("Sam's server")));
        assert_eq!(job(serde_json::json!({ "params": null })).scope(), Some(JobScope::Song));
        assert_eq!(job(serde_json::json!({ "params": { "scope": "galaxy" } })).scope(), None, "a scope this client does not know");
        assert_eq!(job(serde_json::json!({ "state": "paused" })).state, JobState::Unknown);
        let string_peer = job(serde_json::json!({ "recommendation": { "peer": { "id": "4" } }, "result": { "peer": { "name": "C" } } }));
        assert_eq!((string_peer.peer_id(), string_peer.peer_name().as_deref()), (Some(4), Some("C")));
        // Another server's album job is not this one's; a song job is the recording anywhere.
        let album = job(serde_json::json!({ "params": { "scope": "album" } }));
        assert!(album.other_peer(Some(4)) && !album.other_peer(Some(1)) && !album.other_peer(None));
        assert!(!job(serde_json::json!({})).other_peer(Some(4)));
        let nameless = job(serde_json::json!({ "params": { "scope": "artist" }, "recommendation": { "artist": "X" } }));
        assert!(!nameless.other_peer(Some(4)), "a job that names no server is never another's");
    }

    #[test]
    fn queued_and_running_say_where_they_stand_and_a_cancel_takes_the_button_away() {
        assert_eq!(job(serde_json::json!({})).outcome(), JobOutcome::Queued);
        assert_eq!(job(serde_json::json!({})).outcome().action(), Some(JobAction::Cancel));
        let r = job(serde_json::json!({ "state": "running", "progress": 0.432, "statusText": "song 2 of 3 · Late Train Verse" })).outcome();
        assert_eq!(r, JobOutcome::Running { progress: Some(0.432), step: Some("song 2 of 3 · Late Train Verse".into()), stopping: false });
        assert_eq!((r.mark(), r.action(), r.live()), (Some(JobMark::Copying), Some(JobAction::Cancel), true));
        // No estimate: no number at all, never a fake 0%.
        let blind = job(serde_json::json!({ "state": "running", "statusText": "  " })).outcome();
        assert_eq!(blind, JobOutcome::Running { progress: None, step: None, stopping: false });
        let stopping = job(serde_json::json!({ "state": "running", "progress": 0.5, "cancelRequested": true })).outcome();
        assert_eq!((stopping.mark(), stopping.action()), (Some(JobMark::Stopping), None));
        assert_eq!(job(serde_json::json!({})).outcome().mark(), Some(JobMark::Waiting));
    }

    #[test]
    fn a_finished_song_copy_is_copied_already_owned_or_a_file_in_the_way() {
        let copied = job(serde_json::json!({ "state": "done", "result": { "copied": { "vpath": "music", "filepath": "music/Marlowe Vale/Night Ferry/03 Paper Lanterns.flac" } } })).outcome();
        assert_eq!(copied, JobOutcome::Copied { filepath: "music/Marlowe Vale/Night Ferry/03 Paper Lanterns.flac".into(), vpath: Some("music".into()) });
        assert_eq!((copied.kept(), copied.mark(), copied.action()), (true, Some(JobMark::Kept), Some(JobAction::Play)));
        let owned = job(serde_json::json!({ "state": "done", "result": { "skipped": "owned", "existing": { "filepath": "music/x/y.flac", "by": "hash" } } })).outcome();
        assert_eq!(owned, JobOutcome::Owned { filepath: Some("music/x/y.flac".into()), many: None });
        assert_eq!((owned.kept(), owned.mark(), owned.action()), (false, Some(JobMark::Yours), None));
        let exists = job(serde_json::json!({ "state": "done", "result": { "skipped": "exists", "filepath": "music/x/y.flac" } })).outcome();
        assert_eq!(exists, JobOutcome::Exists { filepath: Some("music/x/y.flac".into()) });
        assert_eq!((exists.mark(), exists.action()), (Some(JobMark::Yours), Some(JobAction::Retry)));
        // An unknown result is just done: no mark, no action.
        let done = job(serde_json::json!({ "state": "done", "result": { "echoed": "x" } })).outcome();
        assert_eq!((done.clone(), done.mark(), done.action()), (JobOutcome::Done, None, None));
        assert_eq!(job(serde_json::json!({ "state": "done" })).outcome(), JobOutcome::Done);
    }

    #[test]
    fn failed_carries_the_servers_words_and_a_refusal_offers_no_retry() {
        let failed = job(serde_json::json!({ "state": "failed", "error": "Stage Peer B is unreachable (connect ECONNREFUSED)" })).outcome();
        assert_eq!(failed, JobOutcome::Failed { error: Some("Stage Peer B is unreachable (connect ECONNREFUSED)".into()), many: None });
        assert_eq!((failed.mark(), failed.action(), failed.troubled()), (Some(JobMark::Failed), Some(JobAction::Retry), true));
        assert_eq!(job(serde_json::json!({ "state": "failed" })).outcome(), JobOutcome::Failed { error: None, many: None });
        let refused = job(serde_json::json!({ "state": "failed", "error": "copy from Stage Peer C failed: Stage Peer C does not allow copies with this server's key" }));
        assert!(refused.refused_by_peer());
        assert_eq!(refused.outcome(), JobOutcome::Refused { many: None });
        assert_eq!((refused.outcome().mark(), refused.outcome().action()), (Some(JobMark::Refused), None));
        // Since 2026-10-21 the server ends a refused song's job done, with
        // `stopped: "refused"` and nothing copied.
        let stopped = job(serde_json::json!({ "state": "done", "params": { "destination": null },
            "result": { "stopped": "refused", "peer": { "id": 3, "name": "Stage Peer C" }, "destination": null } })).outcome();
        assert_eq!((stopped.clone(), stopped.mark(), stopped.action()), (JobOutcome::Refused { many: None }, Some(JobMark::Refused), None));
        let unpaired = job(serde_json::json!({ "state": "failed", "error": "this server is no longer paired with that peer" }));
        assert!(unpaired.unpaired() && !unpaired.refused_by_peer());
        let cancelled = job(serde_json::json!({ "state": "cancelled" })).outcome();
        assert_eq!((cancelled.clone(), cancelled.mark(), cancelled.action()), (JobOutcome::Cancelled { many: None }, Some(JobMark::Cancelled), Some(JobAction::StartAgain)));
    }

    #[test]
    fn done_is_not_kept_for_an_album_or_an_artist_copy() {
        let counts = |copied, skipped, failed| ManyCounts { total: 4, copied, skipped, failed, ..ManyCounts::default() };
        // In your collection, with the counts; startable again for the gaps.
        let done = job(serde_json::json!({ "state": "done", "params": { "scope": "album" }, "result": many(serde_json::json!({})) })).outcome();
        assert_eq!(done, JobOutcome::CopiedMany(counts(2, 1, 0)));
        assert_eq!((done.kept(), done.action()), (true, Some(JobAction::CopyAgain)));
        // Nothing copied, all already here: already yours.
        let owned = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "songs": { "total": 2, "copied": [], "skipped": [{}, {}], "failed": [] } })) })).outcome();
        assert_eq!(owned, JobOutcome::Owned { filepath: None, many: Some(ManyCounts { total: 2, skipped: 2, ..ManyCounts::default() }) });
        // Copied 0 with a failure is a failure (Retry), never "already yours" — even beside a skip.
        let failed = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "songs": { "total": 1, "copied": [], "skipped": [], "failed": [{ "from": "a", "error": "gone" }] }, "counts": { "total": 1, "copied": 0, "skipped": 0, "failed": 1 } })) })).outcome();
        assert_eq!(failed, JobOutcome::Failed { error: None, many: Some(ManyCounts { total: 1, failed: 1, ..ManyCounts::default() }) });
        assert_eq!(failed.action(), Some(JobAction::Retry));
        let mixed = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "songs": { "total": 2, "copied": [], "skipped": [{}], "failed": [{}] } })) })).outcome();
        assert!(matches!(mixed, JobOutcome::Failed { .. }), "{mixed:?}");
        // An artist job whose album listings failed copied nothing either.
        let listings = job(serde_json::json!({ "state": "done", "params": { "scope": "artist" },
            "result": many(serde_json::json!({ "scope": "artist", "albums": [{ "name": "A", "error": "peer said 500" }], "skippedAlbums": [], "songs": { "total": 0, "copied": [], "skipped": [], "failed": [] } })) })).outcome();
        assert!(matches!(listings, JobOutcome::Failed { many: Some(ManyCounts { albums_failed: 1, .. }), .. }), "{listings:?}");
    }

    #[test]
    fn the_counts_come_from_result_counts_never_the_capped_lists() {
        let capped: Vec<serde_json::Value> = (0..500).map(|_| serde_json::json!({})).collect();
        let big = job(serde_json::json!({ "state": "done", "params": { "scope": "artist" },
            "result": many(serde_json::json!({ "scope": "artist", "songs": { "total": 1200, "copied": capped, "skipped": [], "failed": capped },
                "counts": { "total": 1200, "copied": 700, "skipped": 0, "failed": 500 }, "truncated": true })) }));
        assert!(big.copy_result().truncated);
        assert!(matches!(big.outcome(), JobOutcome::CopiedMany(ManyCounts { total: 1200, copied: 700, failed: 500, .. })));
        // Counts say nothing was copied though a list (an old shape) is not empty: counts win.
        let zero = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "counts": { "total": 3, "copied": 0, "skipped": 3, "failed": 0 } })) })).outcome();
        assert!(matches!(zero, JobOutcome::Owned { .. }), "{zero:?}");
        // A result with counts alone (no lists) is still a many-song result.
        let bare = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": { "scope": "album", "counts": { "total": 2, "copied": 2, "skipped": 0, "failed": 0 }, "stopped": null } })).outcome();
        assert_eq!(bare, JobOutcome::CopiedMany(ManyCounts { total: 2, copied: 2, ..ManyCounts::default() }));
    }

    #[test]
    fn nothing_to_copy_names_a_guest_spot_and_all_their_albums_here_is_already_yours() {
        let guest = job(serde_json::json!({ "state": "done", "params": { "scope": "artist" },
            "result": many(serde_json::json!({ "scope": "artist", "albums": [], "skippedAlbums": [{ "name": "Comp", "why": "appearance" }, { "name": "Feat", "why": "appearance" }],
                "songs": { "total": 0, "copied": [], "skipped": [], "failed": [] }, "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 } })) })).outcome();
        assert!(matches!(guest, JobOutcome::Nothing { appearances: 2, .. }), "{guest:?}");
        assert_eq!((guest.mark(), guest.action()), (None, None), "no mark on the page: the modals say it");
        let none = job(serde_json::json!({ "state": "done", "params": { "scope": "artist-missing" },
            "result": many(serde_json::json!({ "scope": "artist-missing", "albums": [], "skippedAlbums": [], "songs": { "total": 0, "copied": [], "skipped": [], "failed": [] } })) })).outcome();
        assert!(matches!(none, JobOutcome::Nothing { appearances: 0, .. }), "{none:?}");
        let have = job(serde_json::json!({ "state": "done", "params": { "scope": "artist-missing" },
            "result": many(serde_json::json!({ "scope": "artist-missing", "albums": [], "skippedAlbums": [{ "name": "A", "why": "owned" }, { "name": "B", "why": "owned" }],
                "songs": { "total": 0, "copied": [], "skipped": [], "failed": [] } })) })).outcome();
        assert!(matches!(have, JobOutcome::Owned { many: Some(ManyCounts { albums_owned: 2, albums_left_out: 2, .. }), .. }), "{have:?}");
    }

    #[test]
    fn stopped_early_wins_over_what_landed_and_a_refusal_has_no_retry() {
        let quota = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "stopped": "quota", "songs": { "total": 4, "copied": [{}], "skipped": [], "failed": [{ "from": "b", "error": "limit" }] } })) })).outcome();
        assert!(matches!(quota, JobOutcome::Stopped { why: StopReason::Quota, many: ManyCounts { copied: 1, failed: 1, .. } }), "{quota:?}");
        assert_eq!((quota.mark(), quota.action(), quota.kept()), (Some(JobMark::Stopped), Some(JobAction::Retry), false));
        let busy = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "stopped": "busy", "songs": { "total": 3, "copied": [{}, {}, {}], "skipped": [], "failed": [] } })) })).outcome();
        assert!(matches!(busy, JobOutcome::Stopped { why: StopReason::Busy, .. }), "stopped early is never in your collection");
        let peer = job(serde_json::json!({ "state": "done", "params": { "scope": "album" }, "result": many(serde_json::json!({ "stopped": "peer" })) })).outcome();
        assert!(matches!(peer, JobOutcome::Stopped { why: StopReason::Peer, .. }));
        let refused = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "stopped": "refused", "songs": { "total": 1, "copied": [], "skipped": [], "failed": [{}] }, "counts": { "total": 1, "copied": 0, "skipped": 0, "failed": 1 } })) })).outcome();
        assert!(matches!(refused, JobOutcome::Refused { many: Some(_) }), "{refused:?}");
        assert_eq!((refused.mark(), refused.action()), (Some(JobMark::Refused), None));
        let cancelled = job(serde_json::json!({ "state": "cancelled", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "stopped": "cancelled", "songs": { "total": 4, "copied": [{}], "skipped": [], "failed": [] } })) })).outcome();
        assert_eq!(cancelled, JobOutcome::Cancelled { many: Some(ManyCounts { total: 4, copied: 1, ..ManyCounts::default() }) });
    }

    #[test]
    fn any_stop_word_on_a_done_row_is_a_stop_never_in_your_collection() {
        // Done with stopped "cancelled": the cancel landed as the run ended
        // — cancelled, Start again; never kept.
        let late = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "stopped": "cancelled", "songs": { "total": 4, "copied": [{}, {}], "skipped": [], "failed": [] } })) })).outcome();
        assert_eq!(late, JobOutcome::Cancelled { many: Some(ManyCounts { total: 4, copied: 2, ..ManyCounts::default() }) });
        assert_eq!((late.mark(), late.action(), late.kept()), (Some(JobMark::Cancelled), Some(JobAction::StartAgain), false));
        // A newer server's word: stopped in neutral words, Retry.
        let newer = job(serde_json::json!({ "state": "done", "params": { "scope": "artist" },
            "result": many(serde_json::json!({ "stopped": "maintenance", "songs": { "total": 3, "copied": [{}, {}, {}], "skipped": [], "failed": [] } })) })).outcome();
        assert!(matches!(newer, JobOutcome::Stopped { why: StopReason::Other, many: ManyCounts { copied: 3, .. } }), "{newer:?}");
        assert_eq!((newer.mark(), newer.action(), newer.kept()), (Some(JobMark::Stopped), Some(JobAction::Retry), false));
        // Blank is no word: copied with nothing stopping it is kept.
        let blank = job(serde_json::json!({ "state": "done", "params": { "scope": "album" },
            "result": many(serde_json::json!({ "stopped": " ", "songs": { "total": 1, "copied": [{}], "skipped": [], "failed": [] } })) })).outcome();
        assert!(blank.kept(), "{blank:?}");
    }

    #[test]
    fn the_marks_rank_live_then_trouble_then_kept_then_yours_then_cancelled() {
        let mut marks = vec![JobMark::Cancelled, JobMark::Yours, JobMark::Kept, JobMark::Refused, JobMark::Stopping, JobMark::Waiting, JobMark::Copying];
        marks.sort_by_key(|m| m.rank());
        assert_eq!(marks, vec![JobMark::Copying, JobMark::Waiting, JobMark::Stopping, JobMark::Refused, JobMark::Kept, JobMark::Yours, JobMark::Cancelled]);
        assert_eq!(JobMark::Failed.rank(), JobMark::Stopped.rank());
    }

    #[test]
    fn a_lookup_names_its_keys_but_never_the_folders() {
        let found: JobLookup = serde_json::from_value(serde_json::json!({
            "key": "text:3f9a",
            "keys": { "song": "text:3f9a", "album": "album:7c1d", "artist": "artist:e0b2", "artist-missing": "artist-missing:e0b2", "folder": "folder:91ac", "odd": null },
            "jobs": [{ "id": 3, "plugin": "federation-copy", "state": "done", "key": "album:7c1d", "params": { "scope": "album" } }]
        }))
        .unwrap();
        assert_eq!(found.row_keys(), vec!["text:3f9a", "album:7c1d", "artist:e0b2", "artist-missing:e0b2"]);
        assert_eq!(found.jobs[0].scope(), Some(JobScope::Album));
        assert_eq!(JobLookup::default().row_keys(), Vec::<String>::new());
    }

    /// A layout rendered whole, `/` between its folders — the server's
    /// `resolveLayout` path.
    fn resolve_layout(layout: &str, tags: &LayoutTags) -> String {
        layout_folders(layout, tags, &|_| true)
            .iter()
            .map(|folder| folder.iter().map(|(text, _)| text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn crumbs(place: &CollectionDestination, tags: &LayoutTags) -> Vec<String> {
        place.landing(tags, false).crumbs()
    }

    #[test]
    fn the_layout_renders_as_the_server_renders_it() {
        let tags = |artist: &str, album: &str| LayoutTags { artist: Some(artist.into()), album: Some(album.into()), ..LayoutTags::default() };
        assert_eq!(resolve_layout("{{ARTIST}}/{{ALBUM}}", &tags("Boukmanflow", "Borrowed Tapes")), "Boukmanflow/Borrowed Tapes");
        // Reserved characters and blanks folded, dots trimmed, a missing variable's segment dropped.
        assert_eq!(resolve_layout("{{ ARTIST }}/{{YEAR}}/{{ALBUM}}", &tags("AC/DC", "  ...Live:  at  Donington.. ")), "AC-DC/Live- at Donington");
        // Control characters (a tab among them) are reserved first; blanks fold after.
        assert_eq!(sanitize_segment(Some("a\u{1}b\t\tc")), "a-b-c");
        assert_eq!(sanitize_segment(Some("a  \t  b")), "a - b");
        assert_eq!(sanitize_segment(None), "");
        assert_eq!(sanitize_segment(Some(&"x".repeat(300))).chars().count(), 200);
        // A tag is not a shell string: `~`, `$HOME` and `${HOME}` fold to `-`,
        // each on its own, after the reserved runs (path-template.js).
        assert_eq!(sanitize_segment(Some("~/Mixes")), "--Mixes");
        assert_eq!(sanitize_segment(Some("$HOME Tapes")), "- Tapes");
        assert_eq!(sanitize_segment(Some("${HOME}~$HOME")), "---");
        assert_eq!(sanitize_segment(Some("$HOMEtown $HOME_1 $HOMÉ")), "$HOMEtown $HOME_1 $HOMÉ", "a word that goes on is a name");
        assert_eq!(sanitize_segment(Some("$HOME-made")), "--made");
        assert_eq!(sanitize_segment(Some(" ~ ")), "-");
        assert_eq!(resolve_layout("{{ARTIST}}/{{ALBUM}}", &tags("~kim", "${HOME}")), "-kim/-");
        // ALBUMARTIST falls back to the artist; PEER is the server it came from; lower case names work.
        let peer = LayoutTags { artist: Some("Kim".into()), peer: Some("Stage Peer B".into()), ..LayoutTags::default() };
        assert_eq!(resolve_layout("{{peer}}/{{ALBUMARTIST}}/{{ALBUM}}", &peer), "Stage Peer B/Kim");
        // An unknown name renders nothing; a malformed token is left as written; a lone brace too.
        assert_eq!(resolve_layout("{{NOPE}}/{{1x}}/{{ALBUM", &tags("A", "B")), "{{1x}}/{{ALBUM");
        // A value's blanks stay inside its folder; the text around a dropped one trims.
        assert_eq!(resolve_layout(" {{GENRE}} - {{ALBUM}} /x", &tags("A", "Borrowed Tapes")), "- Borrowed Tapes/x");
        let dest = CollectionDestination { vpath: "copies".into(), base: "From peers\\ ".into(), layout: "{{ARTIST}}/{{ALBUM}}".into(), source: None };
        assert_eq!(crumbs(&dest, &tags("Boukmanflow", "Borrowed Tapes")), ["copies", "From peers", "Boukmanflow", "Borrowed Tapes"]);
        let bare = CollectionDestination { vpath: "demo".into(), ..CollectionDestination::default() };
        assert_eq!(crumbs(&bare, &tags("Boukmanflow", "Borrowed Tapes")), ["demo", "Boukmanflow", "Borrowed Tapes"], "an empty layout is the default one");
    }

    #[test]
    fn an_artist_copy_lands_one_folder_per_album_and_a_preview_keeps_a_refused_variable() {
        let artist = LayoutTags { artist: Some("Boukmanflow".into()), album_artist: Some("Boukmanflow".into()), ..LayoutTags::default() };
        // The first folder naming the album or the year, and every folder
        // after it, are each album's own (the webapp's scopeTarget).
        let landing = Landing::of("copies", "from-peers", "{{ARTIST}}/{{YEAR}} - {{ ALBUM }}/disc", &artist, true, &|_| true);
        assert_eq!((landing.crumbs(), landing.per_album), (vec!["copies".into(), "from-peers".into(), "Boukmanflow".into()], true));
        let landing = Landing::of("copies", "", "{{ALBUMARTIST}}/{{album}}", &artist, true, &|_| true);
        assert_eq!((landing.crumbs(), landing.per_album), (vec!["copies".into(), "Boukmanflow".into()], true));
        // A layout naming no album folder adds nothing: every album shares it.
        let landing = Landing::of("copies", "", "{{GENRE}}/{{ARTIST}}", &artist, true, &|_| true);
        assert_eq!((landing.crumbs(), landing.per_album), (vec!["copies".into(), "Boukmanflow".into()], false));
        // A variable the server would refuse stands as typed, flagged, in its own run.
        let song = LayoutTags { artist: Some("Boukmanflow".into()), album: Some("Borrowed Tapes".into()), year: Some("2019".into()), ..LayoutTags::default() };
        let known = |name: &str| LAYOUT_VARS.contains(&name);
        let landing = Landing::of("copies", "", "{{ARTST}}/{{YEAR}} - {{ALBUM}}", &song, false, &known);
        assert_eq!(landing.folders, vec![vec![("{{ARTST}}".to_string(), Run::Unknown)], vec![("2019 - Borrowed Tapes".to_string(), Run::Text)]]);
        let landing = Landing::of("copies", "", "x {{ARTST}} y", &song, false, &known);
        assert_eq!(landing.folders, vec![vec![("x ".to_string(), Run::Text), ("{{ARTST}}".to_string(), Run::Unknown), (" y".to_string(), Run::Text)]]);
        // A finished copy's folder, as it landed.
        assert_eq!(Landing::landed("copies", &["Boukmanflow".into()]).crumbs(), ["copies", "Boukmanflow"]);
        assert_eq!(safe_file_name("peerlib/Boukmanflow/Borrowed Tapes/02 - Late: Train?.mp3"), "02 - Late- Train-.mp3");
        assert_eq!(safe_file_name("peerlib/.."), "track");
    }

    #[test]
    fn a_base_folder_folds_into_the_layout_and_a_layouts_lead_is_where_every_copy_lands() {
        assert_eq!(fold_base("", "{{ARTIST}}/{{ALBUM}}"), "{{ARTIST}}/{{ALBUM}}");
        assert_eq!(fold_base(" from-peers\\2025 archive ", "{{ARTIST}}"), "from-peers/2025 archive/{{ARTIST}}");
        assert_eq!(braced_folder("from-peers/{{PEER}}/x"), Some("{{PEER}}".into()));
        assert_eq!(braced_folder("a/b}c"), Some("b}c".into()));
        assert_eq!(braced_folder("from-peers"), None);
        let lead = |layout: &str| layout_lead(layout);
        assert_eq!(lead("From friends/{{ARTIST}}/{{ALBUM}}"), (vec!["From friends".to_string()], "{{ARTIST}}/{{ALBUM}}".to_string()));
        assert_eq!(lead(" a //b/{{X}}/c"), (vec!["a".to_string(), "b".to_string()], "{{X}}/c".to_string()));
        assert_eq!(lead("{{ARTIST}}"), (Vec::new(), "{{ARTIST}}".to_string()));
        assert_eq!(lead("plain/name"), (vec!["plain".to_string(), "name".to_string()], String::new()));
        assert_eq!(lead(""), (Vec::new(), String::new()));
        assert_eq!(start_layout_at("from-peers", "From friends/{{ARTIST}}"), "from-peers/{{ARTIST}}");
        assert_eq!(start_layout_at("", "From friends/{{ARTIST}}"), "{{ARTIST}}");
        assert_eq!(start_layout_at("x/y", "plain"), "x/y");
        assert_eq!(start_layout_at("x", ""), "x");
    }

    #[test]
    fn the_validator_refuses_what_the_server_refuses() {
        let known = |name: &str| LAYOUT_VARS.contains(&name) || name == SERVER_FOLDER;
        let check = |layout: &str| validate_layout(layout, &known);
        assert_eq!(check("{{ARTIST}}/{{ALBUM}}"), Ok(()));
        assert_eq!(check("From friends"), Ok(()));
        assert_eq!(check("{{SERVERFOLDER}}"), Ok(()));
        assert_eq!(check(""), Err(LayoutProblem::EmptyTemplate));
        assert_eq!(check(&"x".repeat(501)), Err(LayoutProblem::TemplateTooLong));
        assert_eq!(check("{{ARTIST}"), Err(LayoutProblem::UnbalancedBraces));
        assert_eq!(check("a}b"), Err(LayoutProblem::UnbalancedBraces));
        // An unknown variable as typed, blanks and case kept — and one the
        // view does not list (an older server's `{{SERVERFOLDER}}`).
        assert_eq!(check("{{ Artst }}/{{ALBUM}}"), Err(LayoutProblem::UnknownVariable { token: "{{ Artst }}".into() }));
        let older = |name: &str| LAYOUT_VARS.contains(&name);
        assert_eq!(validate_layout("{{SERVERFOLDER}}", &older), Err(LayoutProblem::UnknownVariable { token: "{{SERVERFOLDER}}".into() }));
        assert_eq!(check("a\tb"), Err(LayoutProblem::InvalidChars));
        assert_eq!(check("/x"), Err(LayoutProblem::Absolute));
        assert_eq!(check("\\x"), Err(LayoutProblem::Absolute));
        // The path the server's sample renders is checked last.
        assert_eq!(check(" "), Err(LayoutProblem::EmptyPath));
        assert_eq!(check("a/../b"), Err(LayoutProblem::Traversal));
        assert_eq!(check("~/x"), Err(LayoutProblem::Home));
        assert_eq!(check("x/$HOME"), Err(LayoutProblem::Home));
        assert_eq!(check("x/${HOME}"), Err(LayoutProblem::UnbalancedBraces), "its braces are stray braces first, as the server reads them");
        assert_eq!(validate_resolved_path("x/${HOME}"), Err(LayoutProblem::Home));
        assert_eq!(check("$HOMEtown"), Ok(()), "a word that goes on is a name");
        assert_eq!(check("C:/x"), Err(LayoutProblem::DriveLetter));
        assert_eq!(check("x/d:y"), Err(LayoutProblem::DriveLetter));
        // The sample's path is cut at the server's length, never refused for it.
        assert_eq!(check(&"{{SERVERFOLDER}}/".repeat(29)), Ok(()));
        assert_eq!(validate_resolved_path(&"x".repeat(501)), Err(LayoutProblem::PathTooLong));
        assert_eq!(validate_resolved_path("a/b"), Ok(()));
    }

    #[test]
    fn a_variable_with_no_value_is_named_as_missing_and_the_path_is_measured() {
        let tags = LayoutTags { artist: Some("Boukmanflow".into()), album: Some("Borrowed Tapes".into()), ..LayoutTags::default() };
        let landing = Landing::of("copies", "", "{{ARTIST}}/{{YEAR}}/{{ALBUM}}/{{GENRE}}", &tags, false, &|_| true);
        assert_eq!(landing.crumbs(), ["copies", "Boukmanflow", "Borrowed Tapes"]);
        assert_eq!(landing.missing, ["YEAR", "GENRE"]);
        // A slot is no missing value; the server folder of a song at the
        // library's root is; a name the view does not list is not counted;
        // each once.
        let slotted = LayoutTags { slots: vec![SongTag::Genre], server_folder: Some(String::new()), ..tags.clone() };
        let landing = Landing::of("copies", "", "{{GENRE}}/{{SERVERFOLDER}}/{{NOPE}}/{{YEAR}}/{{YEAR}}", &slotted, false, &|name| name != "NOPE");
        assert_eq!(landing.missing, ["SERVERFOLDER", "YEAR"]);
        // An artist copy's cut keeps the album's folders out of the count.
        let landing = Landing::of("copies", "", "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", &tags, true, &|_| true);
        assert_eq!((landing.per_album, landing.missing.is_empty()), (true, true));
        assert_eq!(Landing::of("copies", "base", "{{ARTIST}}/{{ALBUM}}", &tags, false, &|_| true).rendered_len(), "base/Boukmanflow/Borrowed Tapes".len());
        assert!(Landing::landed("copies", &["a".into()]).missing.is_empty());
    }

    #[test]
    fn a_slot_stands_for_a_variable_each_song_fills_inside_its_folders_own_words() {
        let tags = LayoutTags {
            artist: Some("Boukmanflow".into()),
            album: Some("Night Market Sampler".into()),
            year: Some("2024".into()),
            album_artist: Some("Various Artists".into()),
            slots: vec![SongTag::Artist, SongTag::Genre],
            ..LayoutTags::default()
        };
        // The 2026-10-22 bug's layout: the artist is each song's, so no one folder is promised.
        let landing = Landing::of("copies", "", "From friends/{{ARTIST}}", &tags, false, &|_| true);
        assert_eq!(landing.folders, vec![vec![("From friends".to_string(), Run::Text)], vec![("‹artist›".to_string(), Run::Slot(SongTag::Artist))]]);
        assert_eq!((landing.crumbs(), landing.slots()), (vec!["copies".into(), "From friends".into(), "‹artist›".into()], vec![SongTag::Artist]));
        // The folder keeps its literal words around the slot; a slot is
        // never trimmed, never dropped, never merged with its neighbour; the
        // album and its credit stay values; a slot the layout does not name
        // shows nowhere.
        let landing = Landing::of("copies", "", " {{GENRE}}{{ARTIST}} - x/{{ALBUMARTIST}}/{{YEAR}} {{ALBUM}}", &tags, false, &|_| true);
        assert_eq!(
            landing.folders[0],
            vec![("‹genre›".to_string(), Run::Slot(SongTag::Genre)), ("‹artist›".to_string(), Run::Slot(SongTag::Artist)), (" - x".to_string(), Run::Text)]
        );
        assert_eq!(landing.crumbs()[2..], ["Various Artists".to_string(), "2024 Night Market Sampler".into()]);
        assert_eq!(landing.slots(), vec![SongTag::Genre, SongTag::Artist], "each once, in the path's order");
        assert_eq!(Landing::of("copies", "", "{{ALBUM}}", &tags, false, &|_| true).slots(), vec![]);
        // Two of one slot side by side stay two: each song writes its value twice.
        let twice = Landing::of("copies", "", "{{ARTIST}}{{ARTIST}}/{{ARTIST}}", &tags, false, &|_| true);
        let slot = ("‹artist›".to_string(), Run::Slot(SongTag::Artist));
        assert_eq!(twice.folders, vec![vec![slot.clone(), slot.clone()], vec![slot]]);
        assert_eq!((twice.crumbs()[1].as_str(), twice.slots()), ("‹artist›‹artist›", vec![SongTag::Artist]));
        // A surface words a slot its own way; a refused variable stays as typed.
        assert_eq!(landing.words(&|slot| format!("<{}>", slot.name().to_uppercase()))[1], "<GENRE><ARTIST> - x");
        let refused = Landing::of("copies", "", "{{ARTIST}}/{{GENRE}}", &tags, false, &|name| name != "GENRE");
        assert_eq!(refused.folders[1], vec![("{{GENRE}}".to_string(), Run::Unknown)]);
        // An artist copy's slot stands before its cut.
        let landing = Landing::of("copies", "", "{{GENRE}}/{{ARTIST}}/{{YEAR}}", &tags, true, &|_| true);
        assert_eq!((landing.crumbs(), landing.per_album), (vec!["copies".into(), "‹genre›".into(), "‹artist›".into()], true));
    }

    #[test]
    fn a_songs_server_folder_is_its_path_inside_the_peers_library() {
        // Its folders on the peer, the library's own name and the file's dropped.
        assert_eq!(server_folder("peerlib/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.MP3"), "Boukmanflow/Borrowed Tapes");
        assert_eq!(server_folder("peerlib/Mixes/x.mp3"), "Mixes");
        // A song at the library's root has none; nor has no path.
        assert_eq!(server_folder("peerlib/02.mp3"), "");
        assert_eq!(server_folder("02.mp3"), "");
        assert_eq!(server_folder(""), "");
        // Each name cleaned as a tag is (reserved characters, `~`, dots and
        // blanks); a name that cleans to nothing goes — `..` can never climb
        // out of the library — as do empty names, `.` and a lead separator.
        assert_eq!(server_folder("peerlib/AC:DC/ ..Live.. /~kim/x.mp3"), "AC-DC/Live/-kim");
        assert_eq!(server_folder("peerlib/../../etc/x.mp3"), "etc");
        assert_eq!(server_folder("/peerlib//a/./b\\c/x.mp3"), "a/b/c");
        assert_eq!(server_folder("peerlib/  /x.mp3"), "");
        // A `.` or a blank name before the library is no library: the
        // library is the first real name (the server's normalizePeerPath).
        assert_eq!(server_folder("./peerlib/a/x.mp3"), "a");
        assert_eq!(server_folder(" /peerlib/a/x.mp3"), "a");
        assert_eq!(server_folder("peerlib/a/x.mp3/ "), "a");
        // The value as the layout takes it is cleaned again, folder by folder.
        assert_eq!(sanitize_folders(Some("a\\b/../c:d/")), "a/b/c-d");
        assert_eq!(sanitize_folders(None), "");
    }

    #[test]
    fn the_server_folder_renders_as_many_folders_as_it_holds() {
        let tags = LayoutTags {
            album_artist: Some("Boukmanflow".into()),
            album: Some("Borrowed Tapes".into()),
            server_folder: Some("Boukmanflow/Borrowed Tapes".into()),
            ..LayoutTags::default()
        };
        let place = |layout: &str| CollectionDestination { vpath: "copies".into(), base: "from-peers".into(), layout: layout.into(), source: None };
        // The preset: the peer's own folders under the base.
        assert_eq!(crumbs(&place("{{SERVERFOLDER}}"), &tags), ["copies", "from-peers", "Boukmanflow", "Borrowed Tapes"]);
        assert_eq!(crumbs(&place("From friends/{{ serverfolder }}"), &tags), ["copies", "from-peers", "From friends", "Boukmanflow", "Borrowed Tapes"]);
        // Inside a folder's literal words, its folders follow the words: the
        // server substitutes first, then splits the path.
        assert_eq!(resolve_layout("Copy of {{SERVERFOLDER}}", &tags), "Copy of Boukmanflow/Borrowed Tapes");
        let landing = Landing::of("copies", "", "Copy of {{SERVERFOLDER}} (peer)/{{ALBUM}}", &tags, false, &|_| true);
        assert_eq!(landing.crumbs(), ["copies", "Copy of Boukmanflow", "Borrowed Tapes (peer)", "Borrowed Tapes"]);
        assert_eq!(landing.folders.len(), 3, "each its own folder");
        // A song at the peer's root: empty, so it drops out like any empty variable.
        let root = LayoutTags { server_folder: Some(String::new()), ..tags.clone() };
        assert_eq!(crumbs(&place("{{SERVERFOLDER}}"), &root), ["copies", "from-peers"]);
        assert_eq!(resolve_layout("From friends/{{SERVERFOLDER}}/x", &root), "From friends/x");
        assert_eq!(resolve_layout("{{SERVERFOLDER}}", &LayoutTags::default()), "", "a download has no peer path");
        // A value from anywhere is cleaned before it renders: never a `..`.
        let odd = LayoutTags { server_folder: Some("../a//..\\b:c".into()), ..LayoutTags::default() };
        assert_eq!(resolve_layout("{{SERVERFOLDER}}", &odd), "a/b-c");
        // An album spread over folders: one slot, in its folder's words.
        let spread = LayoutTags { slots: vec![SongTag::ServerFolder], ..tags.clone() };
        let landing = Landing::of("copies", "", "Copy of {{SERVERFOLDER}}", &spread, false, &|_| true);
        assert_eq!(landing.folders, vec![vec![("Copy of ".to_string(), Run::Text), ("‹server folder›".to_string(), Run::Slot(SongTag::ServerFolder))]]);
        assert_eq!(landing.slots(), vec![SongTag::ServerFolder]);
        // An artist copy: the first folder naming it is each album's own.
        let landing = Landing::of("copies", "", "{{ALBUMARTIST}}/{{SERVERFOLDER}}/x", &tags, true, &|_| true);
        assert_eq!((landing.crumbs(), landing.per_album), (vec!["copies".into(), "Boukmanflow".into()], true));
        // The validator: a view that lists it knows it; the six every server
        // knows do not include it.
        let view = |vars: &[&str]| CollectionDestinationView { variables: vars.iter().map(|v| v.to_string()).collect(), ..Default::default() };
        assert!(view(&["ARTIST", "SERVERFOLDER"]).knows("serverfolder"));
        assert!(!view(&LAYOUT_VARS).knows(SERVER_FOLDER));
        assert!(!view(&[]).knows(SERVER_FOLDER), "a view naming none: the six");
        let refused = Landing::of("copies", "", "{{SERVERFOLDER}}", &tags, false, &|name| view(&LAYOUT_VARS).knows(name));
        assert_eq!(refused.folders, vec![vec![("{{SERVERFOLDER}}".to_string(), Run::Unknown)]], "an older server's GOLD");
    }

    #[test]
    fn a_finished_copy_landed_where_its_files_share_a_folder() {
        let result = |v: serde_json::Value| CopyResult::from_value(&v);
        let landed = |vpath: &str, dirs: &[&str], file: Option<&str>| {
            Some(Landed { vpath: Some(vpath.into()), dirs: dirs.iter().map(|d| d.to_string()).collect(), file: file.map(str::to_string) })
        };
        // One song: its own folder and file; already yours: where the library has it; a file in the way: its folder.
        assert_eq!(result(serde_json::json!({ "copied": { "filepath": "copies/Boukmanflow/Borrowed Tapes/02.mp3" } })).landed(),
            landed("copies", &["Boukmanflow", "Borrowed Tapes"], Some("02.mp3")));
        assert_eq!(result(serde_json::json!({ "skipped": "owned", "existing": { "filepath": "music/Boukmanflow/02.mp3" } })).landed(),
            landed("music", &["Boukmanflow"], Some("02.mp3")));
        assert_eq!(result(serde_json::json!({ "skipped": "exists", "filepath": "copies/x/02.mp3" })).landed(), landed("copies", &["x"], Some("02.mp3")));
        // Many: the folder the copied files share; the skipped ones' when nothing was copied and nothing failed.
        let songs = |copied: serde_json::Value, skipped: serde_json::Value, failed: u64| serde_json::json!({ "scope": "album",
            "songs": { "total": 3, "copied": copied, "skipped": skipped, "failed": [] }, "counts": { "total": 3, "copied": 9, "failed": failed }, "stopped": null });
        let mut two = songs(serde_json::json!([{ "filepath": "copies/B/Tapes/1.mp3" }, { "filepath": "copies/B/Tapes/2.mp3" }]), serde_json::json!([]), 0);
        two["counts"]["copied"] = serde_json::json!(2);
        assert_eq!(result(two.clone()).landed(), landed("copies", &["B", "Tapes"], None));
        two["songs"]["copied"][1]["filepath"] = serde_json::json!("copies/B/Other/2.mp3");
        assert_eq!(result(two.clone()).landed(), landed("copies", &["B"], None), "their common folder");
        two["songs"]["copied"][1]["filepath"] = serde_json::json!("music/2.mp3");
        assert_eq!(result(two).landed(), None, "no library they share");
        let mut owned = songs(serde_json::json!([]), serde_json::json!([{ "at": "music/B/Tapes/1.mp3" }]), 0);
        owned["counts"] = serde_json::json!({ "total": 1, "copied": 0, "skipped": 1, "failed": 0 });
        assert_eq!(result(owned.clone()).landed(), landed("music", &["B", "Tapes"], None));
        owned["counts"]["failed"] = serde_json::json!(1);
        assert_eq!(result(owned).landed(), None, "a failure: nothing claimed");
        // Copied, but the lists name none (an older server): no library — the caller draws where it ran.
        let counted = serde_json::json!({ "scope": "album", "counts": { "total": 2, "copied": 2, "skipped": 0, "failed": 0 }, "stopped": null });
        assert_eq!(result(counted).landed(), Some(Landed::default()));
        assert_eq!(result(serde_json::json!({ "scope": "album", "counts": { "total": 0, "copied": 0 } })).landed(), None);
        assert_eq!(result(serde_json::json!({})).landed(), None);
    }

    #[test]
    fn a_many_song_copy_keeps_what_it_copied_and_what_it_found_already_there_never_a_file_in_the_way() {
        let result = CopyResult::from_value(&serde_json::json!({ "scope": "album", "stopped": null,
            "songs": { "total": 5,
                "copied": [{ "from": "peerlib/B/Tapes/2.mp3", "vpath": "copies", "filepath": "copies/B/Tapes/2.mp3", "title": "Two", "artist": "B", "album": "Tapes" },
                    { "from": "peerlib/B/Tapes/4.mp3", "title": "a copied song naming no file" }],
                "skipped": [{ "from": "peerlib/B/Tapes/1.mp3", "why": "owned", "at": "music/B/Tapes/1.mp3" },
                    { "from": "peerlib/B/Tapes/3.mp3", "why": "exists", "at": "copies/B/Tapes/3.mp3" }],
                "failed": [{ "from": "peerlib/B/Tapes/5.mp3", "error": "gone" }] } }));
        let kept = result.kept_songs();
        let two = KeptSong {
            from: Some("peerlib/B/Tapes/2.mp3".into()),
            filepath: "copies/B/Tapes/2.mp3".into(),
            title: Some("Two".into()),
            artist: Some("B".into()),
            album: Some("Tapes".into()),
        };
        let one = KeptSong { from: Some("peerlib/B/Tapes/1.mp3".into()), filepath: "music/B/Tapes/1.mp3".into(), ..KeptSong::default() };
        assert_eq!(kept, vec![two, one], "copied first, then already there; the file in the way may be another song");
        // A song result, or a many-song one that carries counts alone, names none.
        assert!(CopyResult::from_value(&serde_json::json!({ "copied": { "filepath": "copies/x.mp3" } })).kept_songs().is_empty());
        let counted = serde_json::json!({ "scope": "album", "counts": { "total": 2, "copied": 2, "skipped": 0, "failed": 0 } });
        assert!(CopyResult::from_value(&counted).kept_songs().is_empty());
    }

    #[test]
    fn the_destination_view_reads_null_as_nowhere_to_copy() {
        let view: CollectionDestinationView = serde_json::from_value(serde_json::json!({
            "destination": { "vpath": "demo", "base": "", "layout": "{{ARTIST}}/{{ALBUM}}", "source": "default" },
            "saved": null, "libraries": [{ "vpath": "demo", "template": null }, { "vpath": "testlib" }],
            "defaultLayout": "{{ARTIST}}/{{ALBUM}}", "variables": ["ARTIST", "ALBUM"]
        }))
        .unwrap();
        assert_eq!(view.destination.as_ref().map(|d| d.vpath.as_str()), Some("demo"));
        assert_eq!(view.libraries.len(), 2);
        let none: CollectionDestinationView = serde_json::from_value(serde_json::json!({ "destination": null, "libraries": [] })).unwrap();
        assert!(none.destination.is_none());
        assert_eq!(none.library_default(), None, "no library, no default");
    }

    #[test]
    fn the_library_default_is_the_first_library_its_template_and_the_same_place_ignores_the_source() {
        let view: CollectionDestinationView = serde_json::from_value(serde_json::json!({
            "destination": { "vpath": "copies", "base": "from-peers", "layout": "{{ALBUMARTIST}}/{{ALBUM}}", "source": "user" },
            "libraries": [{ "vpath": "music", "template": "{{ARTIST}}/{{YEAR}} - {{ALBUM}}" }, { "vpath": "copies", "template": null }],
            "defaultLayout": "{{ARTIST}}/{{ALBUM}}", "variables": ["ARTIST", "ALBUM", "YEAR"]
        }))
        .unwrap();
        let default = view.library_default().unwrap();
        assert_eq!((default.vpath.as_str(), default.base.as_str(), default.layout.as_str()), ("music", "", "{{ARTIST}}/{{YEAR}} - {{ALBUM}}"));
        assert_eq!(view.layout_of("copies"), "{{ARTIST}}/{{ALBUM}}", "no template: the server's default layout");
        assert!(view.knows("year") && !view.knows("PEER"), "the view's variables, any case");
        assert!(CollectionDestinationView::default().knows("PEER"), "none listed: the six every server knows");
        // The jobs route's test: the base as normalised, the layout exactly, the source not at all.
        let place = view.destination.clone().unwrap();
        let shown = CollectionDestination { base: " from-peers/ ".into(), source: None, ..place.clone() };
        assert!(place.same_place(&shown));
        assert!(!place.same_place(&CollectionDestination { layout: "{{ALBUMARTIST}}/{{ALBUM}} ".into(), ..place.clone() }));
        assert_eq!(place.shown(), serde_json::json!({ "vpath": "copies", "base": "from-peers", "layout": "{{ALBUMARTIST}}/{{ALBUM}}" }));
    }

    #[test]
    fn a_job_reads_its_pinned_destination_absent_null_or_the_place() {
        let pinned = job(serde_json::json!({ "params": { "scope": "album", "destination": { "vpath": "copies", "base": "", "layout": "{{ARTIST}}/{{ALBUM}}" } } }));
        assert_eq!(pinned.params.destination.clone().flatten().map(|d| d.vpath), Some("copies".into()));
        assert_eq!(pinned.scope(), Some(JobScope::Album));
        let nowhere = job(serde_json::json!({ "params": { "destination": null } }));
        assert_eq!(nowhere.params.destination, Some(None), "pinned to nowhere");
        assert_eq!(job(serde_json::json!({ "params": { "scope": "album" } })).params.destination, None, "from before the pin");
        assert_eq!(job(serde_json::json!({ "params": null })).params.destination, None);
        let odd = job(serde_json::json!({ "params": { "scope": "album", "destination": "copies" } }));
        assert_eq!((odd.params.destination.clone(), odd.scope()), (None, Some(JobScope::Album)), "an odd pin is none, the job whole");
    }

    #[test]
    fn only_https_links_go_to_the_opener() {
        let link = |url: &str| PluginLink { id: "x".into(), label: "X".into(), url: url.into(), kind: "search".into() };
        assert!(link("https://musicbrainz.org/recording/1").is_web());
        assert!(link("  HTTPS://www.deezer.com/search/x").is_web());
        for bad in ["http://example.com", "javascript:alert(1)", "file:///etc/passwd", "spotify:track:1", ""] {
            assert!(!link(bad).is_web(), "{bad}");
        }
        let parsed: PluginResolveResponse = serde_json::from_value(serde_json::json!({
            "plugin": "links", "result": { "links": [{ "id": "mb-recording", "label": "MusicBrainz recording", "url": "https://musicbrainz.org/recording/x", "kind": "canonical" }] }
        }))
        .unwrap();
        let links = parsed.result.unwrap().links;
        assert!(links[0].canonical() && links[0].is_web());
    }

    #[test]
    fn name_key_is_the_servers_byte_for_byte() {
        // src/db/name-key.js's cases (the webapp's test holds both to it).
        assert_eq!(name_key(Some("Borrowed Tapes")), "borrowed tapes");
        assert_eq!(name_key(Some("  borrowed\tTAPES ")), "borrowed tapes");
        assert_eq!(name_key(Some("Cold Open\u{2019}s B\u{2010}Sides")), "cold open's b-sides");
        assert_eq!(name_key(Some("\u{201C}Quoted\u{201D} \u{2014} Dash")), "\"quoted\" - dash");
        assert_eq!(name_key(Some("Beyoncé")), "beyoncé");
        assert_eq!(name_key(Some("R.E.M.")), "r.e.m.");
        assert_eq!(name_key(Some("")), "");
        assert_eq!(name_key(None), "");
    }

    fn row(name: &str, album_artist: Option<&str>, artists: &[&str], compilation: bool) -> ArtistAlbumRow {
        ArtistAlbumRow {
            name: Some(name.into()),
            album_artist: album_artist.map(str::to_string),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            compilation,
            ..ArtistAlbumRow::default()
        }
    }

    #[test]
    fn what_you_dont_have_is_reckoned_by_name_never_by_counts() {
        // The stage: the peer holds one Boukmanflow album, the library three others.
        let remote = [row("Borrowed Tapes", Some("Boukmanflow"), &["Boukmanflow"], false)];
        let local = [row("One", Some("Boukmanflow"), &[], false), row("Two", None, &[], false), row("Three", None, &[], false)];
        assert_eq!(artist_ownership("Boukmanflow", &remote, &local), (0, 1), "three here, and the peer's one still missing");
        // A partly-kept album counts as owned by name.
        let local = [row("borrowed  TAPES", None, &[], false)];
        assert_eq!(artist_ownership("Boukmanflow", &remote, &local), (1, 0));
        // An album they only guest on is never theirs to copy; a credit outside a compilation is.
        let remote = [
            row("Block Party Comp", Some("Various Artists"), &["Boukmanflow", "Kim"], true),
            row("Duets", Some("Kim"), &["Kim", "Boukmanflow"], false),
            row("  ", Some("Boukmanflow"), &[], false),
        ];
        assert_eq!(artist_ownership("boukmanflow", &remote, &[]), (0, 1));
    }

    #[test]
    fn the_albums_you_dont_have_carry_their_year_as_the_webapp_reads_it() {
        // The listing's year, else its songs' earliest; 0 is none (the webapp's missingOne).
        let rows: ArtistAlbumRows = serde_json::from_value(serde_json::json!({ "albums": [
            { "name": "Second Shift", "album_artist": "Boukmanflow", "year": 2019, "year_min": 2018 },
            { "name": "Night Bus", "album_artist": "Boukmanflow", "year": 0, "year_min": 2021 },
            { "name": "Loose Ends", "album_artist": "Boukmanflow", "year": null, "year_min": 0 },
            { "name": "Borrowed Tapes", "album_artist": "Boukmanflow", "year": 2020 }
        ] }))
        .unwrap();
        let local = [row("borrowed tapes", None, &[], false)];
        let missing: Vec<(&str, Option<i32>)> =
            missing_albums("Boukmanflow", &rows.albums, &local).iter().map(|a| (a.name.as_deref().unwrap_or_default(), a.year_of())).collect();
        assert_eq!(missing, vec![("Second Shift", Some(2019)), ("Night Bus", Some(2021)), ("Loose Ends", None)]);
    }

    fn walked(name: &str, copied: usize, skipped: usize, failed: usize, stopped: Option<&str>) -> serde_json::Value {
        let list = |n: usize| (0..n).map(|_| serde_json::json!({})).collect::<Vec<_>>();
        serde_json::json!({ "name": name, "year": 2021, "stopped": stopped,
            "songs": { "total": copied + skipped + failed, "copied": list(copied), "skipped": list(skipped), "failed": list(failed) } })
    }

    fn artist_job(scope: &str, albums: Vec<serde_json::Value>, skipped: serde_json::Value, stopped: Option<&str>) -> PluginJob {
        job(serde_json::json!({ "state": "done", "params": { "scope": scope },
            "result": { "scope": scope, "artist": { "name": "Boukmanflow" }, "albums": albums, "skippedAlbums": skipped,
                "songs": { "total": 0, "copied": [], "skipped": [], "failed": [] }, "stopped": stopped, "peer": { "id": 1, "name": "Stage Peer B" } } }))
    }

    #[test]
    fn a_finished_artist_copy_speaks_for_each_album_as_its_walk_did() {
        let state = |j: Option<PluginJob>| j.map(|j| j.outcome());
        // The job copied elsewhere, but every song of THIS album failed.
        let mixed = artist_job("artist", vec![walked("Borrowed Tapes", 0, 0, 2, None), walked("Second Wind", 3, 0, 0, None)], serde_json::json!([]), None);
        assert!(matches!(state(job_for_album(&mixed, Some("Borrowed Tapes"))), Some(JobOutcome::Failed { .. })));
        assert!(matches!(state(job_for_album(&mixed, Some("second wind"))), Some(JobOutcome::CopiedMany(_))));
        // Named as the peer's listing spells it: folded.
        let folded = artist_job("artist", vec![walked("borrowed  TAPES", 2, 0, 0, None)], serde_json::json!([]), None);
        assert!(matches!(state(job_for_album(&folded, Some("Borrowed Tapes"))), Some(JobOutcome::CopiedMany(_))));
        // Only-missing left it out: the library has it.
        let owned = artist_job("artist-missing", vec![walked("Second Wind", 3, 0, 0, None)], serde_json::json!([{ "name": "Borrowed Tapes", "why": "owned" }]), None);
        assert!(matches!(state(job_for_album(&owned, Some("Borrowed Tapes"))), Some(JobOutcome::Owned { .. })));
        // A compilation they only guest on: nothing claimed.
        let guest = artist_job("artist", vec![walked("Borrowed Tapes", 2, 0, 0, None)], serde_json::json!([{ "name": "Block Party Comp", "why": "appearance" }]), None);
        assert_eq!(job_for_album(&guest, Some("Block Party Comp")), None);
        // Its listing failed and the walk went on: nothing; the run stopped there: stopped.
        let mut listing = walked("Borrowed Tapes", 0, 0, 0, None);
        listing["error"] = serde_json::json!("Stage Peer B answered 500");
        let on = artist_job("artist", vec![listing.clone(), walked("Second Wind", 3, 0, 0, None)], serde_json::json!([]), None);
        assert_eq!(job_for_album(&on, Some("Borrowed Tapes")), None);
        let quota = artist_job("artist", vec![walked("Second Wind", 3, 0, 0, None), listing], serde_json::json!([]), Some("quota"));
        assert!(matches!(state(job_for_album(&quota, Some("Borrowed Tapes"))), Some(JobOutcome::Stopped { why: StopReason::Quota, .. })));
        // Stopped mid-album: that album stopped; one the walk never reached says nothing.
        let mid = artist_job("artist", vec![walked("Borrowed Tapes", 1, 0, 0, Some("busy"))], serde_json::json!([]), Some("busy"));
        assert!(matches!(state(job_for_album(&mid, Some("Borrowed Tapes"))), Some(JobOutcome::Stopped { why: StopReason::Busy, .. })));
        assert_eq!(job_for_album(&mid, Some("Never Reached")), None);
        // No walk to read: nothing claimed; no album named: nothing.
        let blind = job(serde_json::json!({ "state": "done", "params": { "scope": "artist" }, "result": { "counts": { "total": 2, "copied": 2, "skipped": 0, "failed": 0 }, "stopped": null } }));
        assert_eq!(job_for_album(&blind, Some("Borrowed Tapes")), None);
        assert_eq!(job_for_album(&folded, None), None);
        // A live or failed artist job, and an album job, are the job itself.
        let live = job(serde_json::json!({ "state": "running", "params": { "scope": "artist" } }));
        assert_eq!(job_for_album(&live, Some("X")), Some(live.clone()));
        let album = job(serde_json::json!({ "state": "done", "params": { "scope": "album" }, "result": many(serde_json::json!({})) }));
        assert_eq!(job_for_album(&album, Some("Other")), Some(album.clone()));
    }

    #[test]
    fn the_scopes_round_trip_and_only_the_wide_ones_ignore_the_server() {
        for scope in [JobScope::Song, JobScope::Album, JobScope::Artist, JobScope::ArtistMissing, JobScope::Folder] {
            assert_eq!(JobScope::parse(scope.as_str()), Some(scope));
        }
        assert!(JobScope::Album.wide() && JobScope::ArtistMissing.wide() && !JobScope::Song.wide() && !JobScope::Folder.wide());
        assert!(!JobScope::Folder.discover() && JobScope::Song.discover());
        assert!(JobState::Queued.live() && JobState::Running.live() && !JobState::Done.live());
        assert!(JobState::Cancelled.finished() && !JobState::Unknown.finished());
    }
}
