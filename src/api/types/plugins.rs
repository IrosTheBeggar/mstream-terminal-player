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

    /// What a finished job offers (the canvas's buttons, the Copies list's
    /// row action): Retry after a failure, a stop or a file in the way;
    /// Copy again after a many-song copy (it takes only the gaps); Start
    /// again after a cancel; Play your copy after a song landed; nothing
    /// after a refusal — the server would refuse again.
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

/// Where copies land: a library, a base folder in it, a layout rendered
/// per song (torrent path-template syntax).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
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
/// off, or no library) and what a picker would need. Read-only here in
/// v1 (clause 105).
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
}

/// The longest a rendered layout may be, and one folder name in it
/// (mStream's path-template.js).
const MAX_RESOLVED_LEN: usize = 500;
const MAX_SEGMENT_LEN: usize = 200;

/// One folder name from a tag, as the server cleans it: path separators,
/// reserved and control characters become `-`, blank runs one space, no
/// leading or trailing dots and blanks, at most 200 characters.
pub fn sanitize_segment(raw: Option<&str>) -> String {
    let Some(raw) = raw else { return String::new() };
    let reserved = |c: char| matches!(c, '/' | '\\' | ':' | '*' | '?' | '<' | '>' | '|' | '"') || (c as u32) < 0x20;
    let mut out = String::new();
    let mut in_reserved = false;
    let mut in_blank = false;
    for c in raw.chars() {
        if reserved(c) {
            if !in_reserved {
                out.push('-');
            }
            in_reserved = true;
            in_blank = false;
        } else if c.is_whitespace() {
            if !in_blank {
                out.push(' ');
            }
            in_blank = true;
            in_reserved = false;
        } else {
            out.push(c);
            in_reserved = false;
            in_blank = false;
        }
    }
    let trimmed = out.trim_matches(|c: char| c == '.' || c.is_whitespace());
    trimmed.chars().take(MAX_SEGMENT_LEN).collect()
}

/// A layout rendered from `tags`: `{{ARTIST}}/{{ALBUM}}` → `Boukmanflow/
/// Borrowed Tapes` — a variable with no value drops its segment, blanks
/// trim, empty segments go (the server's `resolveLayout`, which the
/// webapp mirrors as discover-jobs.js does). The server renders the real
/// path and has the last word; this is the line the modal shows before
/// the first copy (clause 105).
pub fn resolve_layout(layout: &str, tags: &LayoutTags) -> String {
    let value = |name: &str| -> String {
        match name {
            "ARTIST" => sanitize_segment(tags.artist.as_deref()),
            "ALBUM" => sanitize_segment(tags.album.as_deref()),
            "YEAR" => sanitize_segment(tags.year.as_deref()),
            "GENRE" => sanitize_segment(tags.genre.as_deref()),
            "ALBUMARTIST" => sanitize_segment(
                tags.album_artist.as_deref().filter(|a| !a.is_empty()).or(tags.artist.as_deref()),
            ),
            "PEER" => sanitize_segment(tags.peer.as_deref()),
            _ => String::new(),
        }
    };
    // The server's token: `{{`, blanks, a name (a letter or `_`, then
    // letters, digits, `_`), blanks, `}}`. Anything else is text, scanned
    // on from the next character, as a regex scan would.
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
    let mut substituted = String::new();
    let mut rest = layout;
    while let Some(open) = rest.find("{{") {
        substituted.push_str(&rest[..open]);
        match token(&rest[open..]) {
            Some((name, len)) => {
                substituted.push_str(&value(&name.to_ascii_uppercase()));
                rest = &rest[open + len..];
            }
            None => {
                substituted.push('{');
                rest = &rest[open + 1..];
            }
        }
    }
    substituted.push_str(rest);
    let path = substituted
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    path.chars().take(MAX_RESOLVED_LEN).collect()
}

impl CollectionDestination {
    /// Where a copy of these tags lands, as the modal's LANDS IN line
    /// draws it (clause 105): the library, the base folder's names, then
    /// the rendered layout's — `copies › Boukmanflow › Borrowed Tapes`.
    pub fn crumbs(&self, tags: &LayoutTags) -> Vec<String> {
        let mut out = vec![self.vpath.clone()];
        let base = self.base.replace('\\', "/");
        out.extend(base.split('/').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
        let layout = if self.layout.trim().is_empty() { "{{ARTIST}}/{{ALBUM}}" } else { self.layout.as_str() };
        out.extend(resolve_layout(layout, tags).split('/').filter(|s| !s.is_empty()).map(str::to_string));
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
    let who = name_key(Some(artist));
    let here: std::collections::BTreeSet<String> =
        local.iter().map(|a| name_key(a.name.as_deref())).filter(|k| !k.is_empty()).collect();
    let named: Vec<&ArtistAlbumRow> =
        remote.iter().filter(|a| a.name.as_deref().is_some_and(|n| !n.trim().is_empty())).collect();
    let theirs = |a: &ArtistAlbumRow| {
        name_key(a.album_artist.as_deref()) == who
            || (!a.compilation && a.artists.iter().any(|n| name_key(Some(n)) == who))
    };
    let owned = named.iter().filter(|a| here.contains(&name_key(a.name.as_deref()))).count();
    let missing =
        named.iter().filter(|a| theirs(a) && !here.contains(&name_key(a.name.as_deref()))).count();
    (owned, missing)
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
        // ALBUMARTIST falls back to the artist; PEER is the server it came from; lower case names work.
        let peer = LayoutTags { artist: Some("Kim".into()), peer: Some("Stage Peer B".into()), ..LayoutTags::default() };
        assert_eq!(resolve_layout("{{peer}}/{{ALBUMARTIST}}/{{ALBUM}}", &peer), "Stage Peer B/Kim");
        // An unknown or malformed token is left as written; a lone brace too.
        assert_eq!(resolve_layout("{{NOPE}}/{{1x}}/{{ALBUM", &tags("A", "B")), "{{1x}}/{{ALBUM");
        let dest = CollectionDestination { vpath: "copies".into(), base: "From peers\\ ".into(), layout: "{{ARTIST}}/{{ALBUM}}".into(), source: None };
        assert_eq!(dest.crumbs(&tags("Boukmanflow", "Borrowed Tapes")), ["copies", "From peers", "Boukmanflow", "Borrowed Tapes"]);
        let bare = CollectionDestination { vpath: "demo".into(), ..CollectionDestination::default() };
        assert_eq!(bare.crumbs(&tags("Boukmanflow", "each album")), ["demo", "Boukmanflow", "each album"], "an empty layout is the default one");
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
            track_count: None,
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
