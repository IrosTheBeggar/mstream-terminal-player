//! Add from YouTube's job tracker: the adds this player started on one
//! server and account, kept in `ytdl.json` across runs. It owns an add from
//! the press — the folder it needs first, then the POST, sent once and
//! never again — and follows each job in `GET /api/v1/ytdl/downloads` until
//! it settles: the note line's words, the one word in Settings and on the
//! room's title row, the From YouTube list (adding, needs a look, and the
//! Added list: the server's record of what came in, merged with this
//! player's own finished jobs) and "Already adding to this folder". Closing
//! the room or the window never cancels an add.
//!
//! A port of the phone's `lib/singletons/ytdl_hub.dart` — claim, settle,
//! retention, the summary, with #231's snapshot of the entries there before
//! the POST and its send-order guard — and of the webapp's
//! `webapp/alpha/ytdl-jobs.js` for a server with the new job list, minus
//! every surface:
//! - from the ping's `ytdl` generation 1 a job follows its entry by the
//!   POST's id, and the entry says how it ended (a reason, the server's
//!   words); only a POST with no answer — our bound, a lost connection, a
//!   proxy's 504 — is looked for by its link and folder, for 2 minutes;
//! - a 409 (the name taken, refused before anything downloaded), and every
//!   other refusal, shows where Add was pressed while that surface is up,
//!   and leaves no row;
//! - below it the phone's rules: the newest entry for the link and folder
//!   that no job holds and that was not there before the POST is claimed,
//!   then followed by its pid; the record (6.30+) or the folder tells how a
//!   job ended that the server's list can't.
//!
//! Nothing here does I/O. A [`Transport`] makes the calls (the GUI's, each
//! on a thread of its own as the torrent room's are), a [`Clock`] tells the
//! time and a [`Store`] keeps the slot; the GUI drives [`Tracker::tick`]
//! from its frame and reads the [`JobEvent`]s back. The tests drive it the
//! same way over a scripted transport and a fake clock, against both server
//! generations.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    Action, CallFailure, Failure, Phase, Problem, Tags, Tier, classify_add, entry_failure, failure_actions, mkdir_failure,
    output_extension, path_crumbs,
};
use crate::api::ytdl::{
    NoAnswer, RECORD_PAGE, RECORD_PAGES, YtdlAdd, YtdlAddedRead, YtdlCallError, YtdlEntry, YtdlListing, YtdlRecord,
    YtdlRecordPage, YtdlStarted, YtdlTags,
};

/// Every 3 s while a job is open; a read that fails doubles it, up to a
/// minute ([`crate::tui::app::keep::keep_backoff`]).
pub(crate) const POLL_BEAT: Duration = Duration::from_secs(3);
/// A POST that got no answer — or a 2xx whose entry never showed up — is
/// looked for by its link and folder this long.
pub(crate) const CLAIM_WINDOW_MS: i64 = 2 * 60 * 1000;
/// On the new server, with no list from before the POST to tell an earlier
/// add's entry by: one started more than this before the press is not this
/// add's. The server's clock against ours — the slack covers a little
/// drift, not a wrong clock.
pub(crate) const CLAIM_SLACK_MS: i64 = 5_000;
/// How long an add waits, once its folder is there, for the record's
/// newest id and the job list before its POST goes; past it the last id
/// read stands in, and no list at all.
pub(crate) const FLOOR_WAIT_MS: i64 = 5_000;
/// A tier probe that got no answer is asked again this long after, while
/// a surface shows.
pub(crate) const REPROBE_MS: i64 = 3_000;
/// What is kept per server and account: the last 20 jobs (open ones
/// always), a row that needs a look for 7 days, the word's "✓ added" for
/// an hour.
pub(crate) const KEEP_JOBS: usize = 20;
pub(crate) const KEEP_FAILED_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub(crate) const ADDED_NEWS_MS: i64 = 60 * 60 * 1000;
/// The new server keeps a settled entry a day: an open job's entry gone
/// sooner died with a restart.
pub(crate) const SERVER_KEEP_MS: i64 = 24 * 60 * 60 * 1000;
/// Server entries of dismissed and replaced rows kept, never to be claimed
/// again; the oldest go first.
const RETIRED_CAP: usize = 100;

// ── What the tracker is given ───────────────────────────────────────────────

/// The time, in ms since the epoch. A job's times are saved, so they must
/// still mean something after a restart; they are never compared with the
/// server's clock but within [`CLAIM_SLACK_MS`].
pub(crate) trait Clock {
    fn now_ms(&self) -> i64;
}

/// The wall clock.
pub(crate) struct WallClock;

impl Clock for WallClock {
    fn now_ms(&self) -> i64 {
        i64::try_from(crate::clock::epoch_ms()).unwrap_or(i64::MAX)
    }
}

/// Where one server and account's jobs are kept between runs, as JSON.
/// Another player sharing the directory — a `--torrent` window — may have
/// written there since the last load.
pub(crate) trait Store {
    /// The slot as it stands; `None` when there is none, or none to read.
    /// `Err` when the file could not be read at all: never taken for an
    /// empty slot, which a save would then write the other player's jobs
    /// out of.
    fn load(&mut self) -> Result<Option<Value>, String>;
    fn save(&mut self, slot: &Value) -> Result<(), String>;
}

/// `ytdl.json`'s slot for a server's identity — never the address it is
/// reached at, which for a tunnel changes per dial — and the account.
pub(crate) struct SlotStore {
    identity: String,
    username: Option<String>,
}

impl SlotStore {
    pub(crate) fn new(identity: impl Into<String>, username: Option<String>) -> SlotStore {
        SlotStore { identity: identity.into(), username }
    }
}

impl Store for SlotStore {
    fn load(&mut self) -> Result<Option<Value>, String> {
        crate::config::load_ytdl_slot(&self.identity, self.username.as_deref())
    }

    fn save(&mut self, slot: &Value) -> Result<(), String> {
        crate::config::save_ytdl_slot(&self.identity, self.username.as_deref(), Some(slot))
    }
}

/// A call's handle: its answer comes back under it.
pub(crate) type CallId = u64;

/// One request to the server, as [`crate::api::ytdl`] makes it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Call {
    /// `POST /api/v1/file-explorer/mkdir`: the folder an add needs first.
    Mkdir { directory: String },
    /// `POST /api/v1/ytdl/` — sent once.
    Add(YtdlAdd),
    /// `GET /api/v1/ytdl/downloads`.
    Jobs,
    /// One page of the download record.
    Records { generation: u32, limit: u32, before: Option<i64> },
    /// One read of the Added list ([`crate::api::Client::ytdl_added`]): the
    /// tier probe's, a refresh's, Show earlier's.
    Added { generation: u32, before: Option<i64> },
    /// A folder's names.
    Listing { directory: String },
}

impl Call {
    /// The call made on `client`, blocking: what a transport's thread runs.
    pub(crate) fn run(&self, client: &crate::api::Client) -> Answer {
        match self {
            Call::Mkdir { directory } => Answer::Mkdir(client.ytdl_mkdir(directory)),
            Call::Add(add) => Answer::Add(client.ytdl_add(add)),
            Call::Jobs => Answer::Jobs(client.ytdl_jobs()),
            Call::Records { generation, limit, before } => {
                Answer::Records(client.ytdl_records(*generation, *limit, *before))
            }
            Call::Added { generation, before } => Answer::Added(client.ytdl_added(*generation, *before)),
            Call::Listing { directory } => Answer::Listing(client.ytdl_listing(directory)),
        }
    }

    /// The call answered with `e`: what a transport hands back for one it
    /// couldn't make (no session to make it on).
    pub(crate) fn failed(&self, e: YtdlCallError) -> Answer {
        match self {
            Call::Mkdir { .. } => Answer::Mkdir(Err(e)),
            Call::Add(_) => Answer::Add(Err(e)),
            Call::Jobs => Answer::Jobs(Err(e)),
            Call::Records { .. } => Answer::Records(Err(e)),
            Call::Added { .. } => Answer::Added(Err(e)),
            Call::Listing { .. } => Answer::Listing(Err(e)),
        }
    }
}

/// What came back for a [`Call`]: the variant of the same name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Answer {
    Mkdir(Result<(), YtdlCallError>),
    Add(Result<YtdlStarted, YtdlCallError>),
    Jobs(Result<Vec<YtdlEntry>, YtdlCallError>),
    Records(Result<Option<YtdlRecordPage>, YtdlCallError>),
    Added(Result<Option<YtdlAddedRead>, YtdlCallError>),
    Listing(Result<YtdlListing, YtdlCallError>),
}

/// An answer of another kind than its call's: a transport's mistake, read
/// as a call that failed, so nothing waits on it for ever.
fn mismatched() -> YtdlCallError {
    YtdlCallError::Config("an answer for another kind of call".into())
}

impl Answer {
    fn mkdir(self) -> Result<(), YtdlCallError> {
        match self {
            Answer::Mkdir(r) => r,
            _ => Err(mismatched()),
        }
    }

    fn add(self) -> Result<YtdlStarted, YtdlCallError> {
        match self {
            Answer::Add(r) => r,
            _ => Err(mismatched()),
        }
    }

    fn jobs(self) -> Result<Vec<YtdlEntry>, YtdlCallError> {
        match self {
            Answer::Jobs(r) => r,
            _ => Err(mismatched()),
        }
    }

    fn records(self) -> Result<Option<YtdlRecordPage>, YtdlCallError> {
        match self {
            Answer::Records(r) => r,
            _ => Err(mismatched()),
        }
    }

    fn added(self) -> Result<Option<YtdlAddedRead>, YtdlCallError> {
        match self {
            Answer::Added(r) => r,
            _ => Err(mismatched()),
        }
    }

    fn listing(self) -> Result<YtdlListing, YtdlCallError> {
        match self {
            Answer::Listing(r) => r,
            _ => Err(mismatched()),
        }
    }
}

/// Makes the calls. [`Transport::send`] must not wait for the answer: it
/// comes back later through [`Transport::answers`] (a thread per call and a
/// channel). A call that can't be made at all — no session to make it on —
/// answers [`YtdlCallError::Config`] there, like any other.
pub(crate) trait Transport {
    fn send(&mut self, id: CallId, call: Call);
    /// What came back since the last ask, in the order it arrived.
    fn answers(&mut self) -> Vec<(CallId, Answer)>;
}

/// A call's failure as the rules read it (A2's table): no "You're offline"
/// — a refused connection and no way there both started nothing — and a
/// 401, the session's problem, worded as the bare status it is.
pub(crate) fn call_failure(e: &YtdlCallError) -> CallFailure {
    match e {
        YtdlCallError::Unanswered { why: NoAnswer::TimedOut, .. } => CallFailure::TimedOut,
        YtdlCallError::Unanswered { why: NoAnswer::Refused | NoAnswer::Offline, .. } => CallFailure::NeverConnected,
        YtdlCallError::Unanswered { why: NoAnswer::Dropped, .. } => CallFailure::Dropped,
        YtdlCallError::Answered { status, words, reason, file_name } => CallFailure::Answered {
            status: *status,
            words: words.clone(),
            reason: reason.clone(),
            file_name: file_name.clone(),
        },
        YtdlCallError::Unauthorized => CallFailure::Answered { status: 401, words: None, reason: None, file_name: None },
        YtdlCallError::Unreadable { .. } => CallFailure::Unreadable,
        YtdlCallError::Config(_) => CallFailure::NotSent,
    }
}

// ── A job ───────────────────────────────────────────────────────────────────

/// Where one add stands, as this player knows it (the phone's and the
/// webapp's states; the phone's after-download name clash is a
/// [`JobState::Failed`] with [`Problem::Exists`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobState {
    /// Pressed: the folder, the reads before the POST, or the POST is out.
    Starting,
    /// The POST got no answer: looked for by link and folder for 2 minutes
    /// ("Starting… the server hasn't answered yet").
    StartUnsure,
    /// The server has it: its entry is followed (by id, or by pid once
    /// claimed), or — just taken by an older server — about to be claimed.
    Adding,
    /// In the library.
    Added,
    /// The server's job ended in error: [`Job::failure`] says why.
    Failed,
    /// Refused before anything started, kept as a row ("Couldn't add").
    StartFailed,
    /// 2 minutes and no entry: "not sure it started" · Check again.
    NotSureStarted,
    /// Its entry went unseen and the server has no record of it.
    NotSureFinished,
    /// Check again found nothing, or the new server's entry died with a
    /// restart: "not found on the server" · Add again.
    NotFound,
}

impl JobState {
    const ALL: [JobState; 9] = [
        JobState::Starting,
        JobState::StartUnsure,
        JobState::Adding,
        JobState::Added,
        JobState::Failed,
        JobState::StartFailed,
        JobState::NotSureStarted,
        JobState::NotSureFinished,
        JobState::NotFound,
    ];

    /// Its name in `ytdl.json`, the phone's and the webapp's spelling.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            JobState::Starting => "starting",
            JobState::StartUnsure => "startUnsure",
            JobState::Adding => "adding",
            JobState::Added => "added",
            JobState::Failed => "failed",
            JobState::StartFailed => "startFailed",
            JobState::NotSureStarted => "notSureStarted",
            JobState::NotSureFinished => "notSureFinished",
            JobState::NotFound => "notFound",
        }
    }

    pub(crate) fn parse(word: &str) -> Option<JobState> {
        JobState::ALL.into_iter().find(|s| s.as_str() == word)
    }

    /// Still in the tracker's hands: counted as adding, and "Already adding".
    pub(crate) fn open(self) -> bool {
        matches!(self, JobState::Starting | JobState::StartUnsure | JobState::Adding)
    }

    /// A row for the list's needs-a-look part.
    pub(crate) fn needs_look(self) -> bool {
        matches!(
            self,
            JobState::Failed
                | JobState::StartFailed
                | JobState::NotSureStarted
                | JobState::NotSureFinished
                | JobState::NotFound
        )
    }

    /// A failure the word counts ("! failed") and the note names ("Couldn't
    /// add …"). A "not sure" row is a guess, not a failure.
    pub(crate) fn failure(self) -> bool {
        matches!(self, JobState::Failed | JobState::StartFailed)
    }

    /// Check again can ask about it.
    fn not_sure(self) -> bool {
        matches!(self, JobState::NotSureStarted | JobState::NotSureFinished | JobState::NotFound)
    }
}

/// How a job came to know its entry's id (the new server).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Claim {
    /// The POST's 2xx named it.
    Post,
    /// Matched by link and folder after a POST with no answer.
    Match,
}

impl Claim {
    fn as_str(self) -> &'static str {
        match self {
            Claim::Post => "post",
            Claim::Match => "match",
        }
    }

    fn parse(word: &str) -> Option<Claim> {
        match word {
            "post" => Some(Claim::Post),
            "match" => Some(Claim::Match),
            _ => None,
        }
    }
}

/// One add, from the press until its row is dismissed. The identity fields
/// are fixed at the press; the tracker moves `state` and fills in what the
/// server reveals as it goes. Times are this player's clock (`server_start`
/// is the server's).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Job {
    /// Made here, unique among this slot's jobs.
    pub id: String,
    /// The canonical link sent: what the entry's `url` and the record's
    /// `origin` equal.
    pub url: String,
    /// The folder as POSTed ("/Music/Daft Punk/Discovery/").
    pub directory: String,
    /// The `outputCodec` sent.
    pub codec: String,
    /// The tags as sent: trimmed, the year only as four digits.
    pub tags: YtdlTags,
    /// What the row calls it: the edited title, else the video's own.
    pub title: String,
    /// The name the file should land as — the room's prediction, then the
    /// server's — for the folder checks.
    pub file_name: String,
    /// The folder made first, when one on the path was new or unchecked: a
    /// Try again on a start failure makes it again only then.
    pub mkdir: Option<String>,
    /// The folder "Couldn't make the folder “…”" names: the first new one.
    pub mkdir_folder: Option<String>,
    pub created_at: i64,
    /// When the POST went out.
    pub sent_at: Option<i64>,
    pub state: JobState,
    /// The new server's entry id, and how it came to be known.
    pub server_id: Option<String>,
    pub claim: Option<Claim>,
    /// An older server's entry, claimed by link and folder; `server_start`
    /// is the entry's `startTime` (either server).
    pub pid: Option<i64>,
    pub server_start: Option<i64>,
    /// When it settled here.
    pub ended_at: Option<i64>,
    /// Hidden from the list ("the server keeps going"); open jobs only.
    pub hidden: bool,
    /// The list has shown its outcome: the word's counts skip it.
    pub seen: bool,
    /// Why it failed (start or download).
    pub failure: Option<Failure>,
    /// The song in the library, `<vpath>/<path>` with no leading slash —
    /// from the entry, the record or the folder; `None` when nothing named
    /// it (no play then).
    pub file_path: Option<String>,
    /// Its download record, which answers for this job only.
    pub record_id: Option<i64>,
    pub bytes: Option<i64>,
    pub track_id: Option<i64>,
    /// The record's newest id just before the POST (6.30+): a record at or
    /// below it predates this add.
    pub record_floor: Option<i64>,
    /// The entries for this link and folder just before the POST (6.30+),
    /// by their keys: each an earlier add's, so none can ever be this one's
    /// — by identity, not by time, so no clock decides. `None` when the
    /// list was not read.
    pub prior: Option<BTreeSet<String>>,
}

impl Job {
    #[allow(clippy::too_many_arguments)]
    fn fresh(
        id: String,
        url: String,
        directory: String,
        codec: String,
        tags: YtdlTags,
        title: String,
        file_name: String,
        mkdir: Option<String>,
        mkdir_folder: Option<String>,
        created_at: i64,
    ) -> Job {
        Job {
            id,
            url,
            directory,
            codec,
            tags,
            title,
            file_name,
            mkdir,
            mkdir_folder,
            created_at,
            sent_at: None,
            state: JobState::Starting,
            server_id: None,
            claim: None,
            pid: None,
            server_start: None,
            ended_at: None,
            hidden: false,
            seen: false,
            failure: None,
            file_path: None,
            record_id: None,
            bytes: None,
            track_id: None,
            record_floor: None,
            prior: None,
        }
    }

    /// Why it failed, for the row's words ([`super::failure_label`]): what
    /// the server said, or — a row saved without one — the hedge.
    pub(crate) fn failure(&self) -> Option<Failure> {
        match self.state {
            JobState::Failed => Some(self.failure.clone().unwrap_or_else(|| unsaid(Phase::Download))),
            JobState::StartFailed => Some(self.failure.clone().unwrap_or_else(|| unsaid(Phase::Start))),
            _ => None,
        }
    }

    /// The row's ways on, Dismiss last. `formats`: the menu's options
    /// (Change format is offered only when there is another).
    pub(crate) fn ways(&self, formats: Option<&[&str]>) -> Vec<Way> {
        match self.state {
            JobState::NotSureStarted | JobState::NotSureFinished => vec![Way::CheckAgain, Way::Dismiss],
            JobState::NotFound => vec![Way::AddAgain, Way::Dismiss],
            JobState::Failed | JobState::StartFailed => self
                .failure()
                .map(|f| failure_actions(&f, formats).into_iter().map(Way::from).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// What a note and a row call it: the title, else the file's name, else
    /// the link.
    pub(crate) fn shown_title(&self) -> String {
        let title = self.title.trim();
        if !title.is_empty() {
            return title.to_string();
        }
        let name = self.file_path.as_deref().map(name_of).unwrap_or(&self.file_name);
        if name.is_empty() { self.url.clone() } else { stem_of(name).to_string() }
    }

    /// The folder it landed in, else the one it was added to: "Music/Daft
    /// Punk/Discovery", no slashes at the ends.
    pub(crate) fn folder(&self) -> String {
        self.file_path.as_deref().map(folder_of).unwrap_or_else(|| bare(&self.directory).to_string())
    }

    /// Knows its entry: by id, or by pid.
    fn knows_entry(&self) -> bool {
        self.server_id.is_some() || self.pid.is_some()
    }

    /// [`entry_key`] of the entry it knows.
    fn entry_key(&self) -> Option<String> {
        match (&self.server_id, self.pid) {
            (Some(id), _) => Some(format!("id:{id}")),
            (None, Some(pid)) => Some(pid_key(pid, self.server_start)),
            (None, None) => None,
        }
    }

    /// `e` is the entry it follows: by id, else by pid in its link and folder.
    fn follows(&self, e: &YtdlEntry) -> bool {
        match &self.server_id {
            Some(id) => e.id.as_deref() == Some(id.as_str()),
            None => self.pid.is_some() && e.pid == self.pid && e.url == self.url && e.directory == self.directory,
        }
    }

    fn take_record(&mut self, r: &YtdlRecord) {
        self.file_path = Some(r.filepath.clone()).filter(|p| !p.is_empty());
        self.bytes = r.bytes;
        self.track_id = r.track_id;
        self.record_id = Some(r.id);
    }

    fn to_json(&self) -> Value {
        let tags: serde_json::Map<String, Value> =
            [("title", &self.tags.title), ("artist", &self.tags.artist), ("album", &self.tags.album), ("year", &self.tags.year)]
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.to_string(), Value::String(v.clone())))
                .collect();
        json!({
            "id": self.id,
            "url": self.url,
            "directory": self.directory,
            "codec": self.codec,
            "tags": tags,
            "title": self.title,
            "fileName": self.file_name,
            "mkdir": self.mkdir,
            "mkdirFolder": self.mkdir_folder,
            "createdAt": self.created_at,
            "sentAt": self.sent_at,
            "state": self.state.as_str(),
            "serverId": self.server_id,
            "claim": self.claim.map(Claim::as_str),
            "pid": self.pid,
            "serverStart": self.server_start,
            "endedAt": self.ended_at,
            "hidden": self.hidden,
            "seen": self.seen,
            "failure": self.failure.as_ref().map(failure_json),
            "filePath": self.file_path,
            "recordId": self.record_id,
            "bytes": self.bytes,
            "trackId": self.track_id,
            "recordFloor": self.record_floor,
            "prior": self.prior,
        })
    }

    /// What [`Job::to_json`] wrote, or `None` for an entry nothing could be
    /// matched or shown for (no id, link or folder). A state this build
    /// doesn't know (a newer one wrote it) reads as a "not sure" state
    /// rather than vanishing or being polled for ever: not sure it finished
    /// once the server had it, else not sure it started.
    fn read(v: &Value) -> Option<Job> {
        let id = text(v, "id")?;
        let url = v.get("url")?.as_str()?.to_string();
        let directory = v.get("directory")?.as_str()?.to_string();
        let server_id = text(v, "serverId");
        let pid = int(v, "pid");
        let state = v.get("state").and_then(Value::as_str).and_then(JobState::parse).unwrap_or(
            if server_id.is_some() || pid.is_some() { JobState::NotSureFinished } else { JobState::NotSureStarted },
        );
        let tag = |key: &str| {
            v.get("tags").and_then(|t| t.get(key)).and_then(Value::as_str).unwrap_or_default().to_string()
        };
        Some(Job {
            id,
            url,
            directory,
            codec: text(v, "codec").unwrap_or_else(|| "mp3".into()),
            tags: YtdlTags { title: tag("title"), artist: tag("artist"), album: tag("album"), year: tag("year") },
            title: v.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
            file_name: v.get("fileName").and_then(Value::as_str).unwrap_or_default().to_string(),
            mkdir: text(v, "mkdir"),
            mkdir_folder: text(v, "mkdirFolder"),
            created_at: int(v, "createdAt").unwrap_or(0),
            sent_at: int(v, "sentAt"),
            state,
            claim: if server_id.is_some() { v.get("claim").and_then(Value::as_str).and_then(Claim::parse) } else { None },
            server_id,
            pid,
            server_start: int(v, "serverStart"),
            ended_at: int(v, "endedAt"),
            hidden: v.get("hidden") == Some(&Value::Bool(true)),
            seen: v.get("seen") == Some(&Value::Bool(true)),
            failure: v.get("failure").and_then(read_failure),
            file_path: text(v, "filePath"),
            record_id: int(v, "recordId"),
            bytes: int(v, "bytes"),
            track_id: int(v, "trackId"),
            record_floor: int(v, "recordFloor"),
            prior: v
                .get("prior")
                .and_then(Value::as_array)
                .map(|keys| keys.iter().filter_map(Value::as_str).map(str::to_string).collect()),
        })
    }
}

/// A failure with no words, as a job saved without one reads.
fn unsaid(phase: Phase) -> Failure {
    let problem = if phase == Phase::Download { Problem::Unknown } else { Problem::Other };
    Failure { problem, detail: None, reason: None, text: None, phase }
}

fn phase_str(phase: Phase) -> &'static str {
    match phase {
        Phase::Start => "start",
        Phase::Lookup => "lookup",
        Phase::Download => "download",
    }
}

fn failure_json(f: &Failure) -> Value {
    json!({
        "problem": f.problem.as_str(),
        "detail": f.detail,
        "reason": f.reason,
        "text": f.text,
        "phase": phase_str(f.phase),
    })
}

fn read_failure(v: &Value) -> Option<Failure> {
    let problem = Problem::parse(v.get("problem")?.as_str()?)?;
    let phase = match v.get("phase").and_then(Value::as_str) {
        Some("download") => Phase::Download,
        Some("lookup") => Phase::Lookup,
        _ => Phase::Start,
    };
    Some(Failure { problem, detail: text(v, "detail"), reason: text(v, "reason"), text: text(v, "text"), phase })
}

/// A string with something in it.
fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn int(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

/// What one server and account keep: the jobs, newest first, and the
/// entries never to be claimed again.
#[derive(Debug, Clone, Default)]
struct Slot {
    jobs: Vec<Job>,
    retired: Vec<String>,
}

impl Slot {
    /// Anything unreadable is an empty slot; a job that can't be read is
    /// left out, the rest kept.
    fn read(v: &Value) -> Slot {
        let mut jobs: Vec<Job> = Vec::new();
        for raw in v.get("jobs").and_then(Value::as_array).into_iter().flatten() {
            if let Some(job) = Job::read(raw).filter(|j| !jobs.iter().any(|have| have.id == j.id)) {
                jobs.push(job);
            }
        }
        jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        let retired = v
            .get("retired")
            .and_then(Value::as_array)
            .map(|keys| keys.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        Slot { jobs, retired }
    }

    fn to_json(&self) -> Value {
        json!({
            "jobs": self.jobs.iter().map(Job::to_json).collect::<Vec<_>>(),
            "retired": self.retired,
        })
    }
}

/// What a server entry is known by: its id on the new server; before it,
/// its pid and when it started (an operating system hands a pid out again).
fn entry_key(e: &YtdlEntry) -> String {
    match &e.id {
        Some(id) => format!("id:{id}"),
        None => pid_key(e.pid.unwrap_or(-1), e.start_time),
    }
}

fn pid_key(pid: i64, start: Option<i64>) -> String {
    match start {
        Some(at) => format!("pid:{pid}@{at}"),
        None => format!("pid:{pid}@"),
    }
}

// ── What a surface asks for, and hears back ─────────────────────────────────

/// Which surface pressed Add: where a refusal is shown while it is up.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Origin {
    /// Settings › Add from YouTube.
    Room,
    /// The Files window over this folder (the browse bar's path).
    Folder(String),
}

/// One press of Add to library / Add here.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StartRequest {
    /// The canonical link ([`super::parse_link`]'s `url`).
    pub url: String,
    /// The folder as POSTed ([`super::Target::directory`]).
    pub directory: String,
    pub codec: String,
    /// The fields as typed: sent through [`Tags::request`].
    pub tags: Tags,
    /// What the row calls it.
    pub title: String,
    /// The name it should land as ([`super::NameCheck::name`]).
    pub file_name: String,
    /// The folder to make first (recursively) when one on the path is new
    /// or couldn't be checked; `None`: it is there.
    pub mkdir: Option<String>,
    /// The first new folder's name, for "Couldn't make the folder “…”".
    pub mkdir_folder: Option<String>,
    pub origin: Origin,
}

/// Try again, and its variants: a new add of the same song in the row's
/// place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Redo {
    /// Change format and retry: the format to use.
    pub codec: Option<String>,
    /// Choose a folder / Top of the library: the folder to add into
    /// instead, and the folder to make first there.
    pub directory: Option<String>,
    pub mkdir: Option<String>,
    pub mkdir_folder: Option<String>,
}

/// A row's ways on: a failure's ([`Action`]) and the not-sure rows'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Way {
    Retry,
    ChangeFormat,
    ChooseFolder,
    TopOfLibrary,
    CheckAgain,
    AddAgain,
    Dismiss,
}

impl From<Action> for Way {
    fn from(a: Action) -> Way {
        match a {
            Action::Retry => Way::Retry,
            Action::ChangeFormat => Way::ChangeFormat,
            Action::ChooseFolder => Way::ChooseFolder,
            Action::TopOfLibrary => Way::TopOfLibrary,
            Action::CheckAgain => Way::CheckAgain,
            Action::Dismiss => Way::Dismiss,
        }
    }
}

/// What a start came to, for the surface that pressed it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StartOutcome {
    /// The server took it (a 2xx): the job is adding. The window closes.
    Took,
    /// The POST got no answer: the job is looked for (startUnsure).
    Unsure,
    /// Nothing started, and the surface that pressed it is up: it shows
    /// the failure where Add was pressed. No row is left.
    Refused(Failure),
    /// Nothing started, and nothing that pressed it shows: a "Couldn't add"
    /// row (startFailed), with its note.
    Failed(Failure),
}

/// The note line's words for a job that settled — at most one per job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Note {
    /// "Added “One More Time”", the folder joining where the note is wide.
    Added { title: String, folder: String },
    /// "Couldn't add “…”".
    CouldntAdd { title: String },
}

/// What happened, for the GUI to act on ([`Tracker::take_events`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JobEvent {
    /// A start's answer.
    Started { job: String, origin: Option<Origin>, outcome: StartOutcome },
    /// A job settled: its folder re-lists, and the note (if any) shows.
    Settled { job: String, state: JobState, note: Option<Note> },
    /// A flag the ping gave looks stale — a 403, "FFmpeg is not available
    /// yet", a format turned off, a folder refused: ping again.
    Reping,
    /// The server refused the format (a 400 naming `outputCodec`): the
    /// remembered one goes back to the default.
    FormatOff,
}

/// The one word the jobs leave in Settings and on the room's title row,
/// the first that holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Word {
    /// `adding` (ACCENT): a job is open.
    Adding,
    /// `! failed` (GOLD): a failure the list hasn't shown.
    Failed,
    /// `✓ added` (OK): a song added in the last hour the list hasn't shown.
    Added,
    /// `added songs` (DIM, the title row only): the list has something.
    History,
}

/// The counts behind the [`Word`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub adding: usize,
    pub failed: usize,
    pub added: usize,
    /// The From YouTube list has a row.
    pub history: bool,
    /// When `added` next drops by itself (an hour on), with nothing to say
    /// so: a word showing it is redrawn then.
    pub news_ends: Option<i64>,
}

impl Summary {
    pub(crate) fn word(&self) -> Option<Word> {
        if self.adding > 0 {
            Some(Word::Adding)
        } else if self.failed > 0 {
            Some(Word::Failed)
        } else if self.added > 0 {
            Some(Word::Added)
        } else if self.history {
            Some(Word::History)
        } else {
            None
        }
    }
}

/// One row of the Added list: a download record (6.30+, every device's
/// adds), or one of this player's finished jobs no loaded record answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddedRow {
    pub title: String,
    /// The folder, "Music/Daft Punk/Discovery".
    pub folder: String,
    /// The song, `<vpath>/<path>`; `None` when nothing named the file.
    pub file_path: Option<String>,
    /// The file's extension, lower case — the format's name comes from it
    /// (the record has no codec).
    pub ext: Option<String>,
    pub bytes: Option<i64>,
    /// When it landed: a record's is the server's clock (display only).
    pub at: Option<i64>,
    /// The library still has it (false: "file missing").
    pub present: bool,
    pub record_id: Option<i64>,
    pub track_id: Option<i64>,
    /// The job it came from, when it is one of this player's.
    pub job: Option<String>,
}

// ── The tracker ─────────────────────────────────────────────────────────────

/// What an add waits on before its POST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// Not asked (an older server).
    Skip,
    /// Its read is out.
    Out,
    /// The tier probe is out, and answers it (the floor while the tier is
    /// not known).
    Probe,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mkdir {
    directory: String,
    folder: Option<String>,
}

/// An add before its POST: the folder, the floor and the prior entries.
#[derive(Debug, Clone)]
struct Launch {
    /// The folder being made (its call is out).
    mkdir: Option<Mkdir>,
    floor: Gate,
    prior: Gate,
    /// When the floor and the prior entries stop being waited for: 5 s
    /// after the folder is there.
    until: Option<i64>,
}

/// A job's look past the job list: how an entry that completed, failed or
/// went ended — or Check again's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    /// Its (older) entry completed: the record or the folder names the file.
    Landed,
    /// Its older entry ended in error: the folder tells a name clash.
    /// `gone`: the entry is gone too.
    Erred { gone: bool },
    /// Its entry went unseen: the record or the folder says whether it came
    /// in.
    Vanished,
    /// Check again. `unread`: the record could not be read, so the folder
    /// alone can't say "not found".
    Check { unread: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Jobs,
    Record { page: usize, before: Option<i64> },
    Listing,
}

/// What a call in flight is for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Mkdir { job: String },
    Floor { job: String },
    Prior { job: String },
    Post { job: String },
    /// A poll's read, numbered as it went out.
    Poll { read: u64 },
    Probe,
    Added { before: Option<i64> },
    Look { job: String, step: Step },
}

/// The Added list's record rows (6.30+).
#[derive(Debug, Clone, Default)]
struct AddedList {
    records: Vec<YtdlRecord>,
    /// The smallest id read: Show earlier asks before it.
    cursor: Option<i64>,
    /// Show earlier has more to ask for.
    more: bool,
    /// A read is out.
    out: bool,
}

/// Add from YouTube on one server and account: its jobs and their polling,
/// the tier, and the Added list.
pub(crate) struct Tracker<T: Transport, C: Clock, S: Store> {
    transport: T,
    clock: C,
    store: S,
    /// The ping's `ytdl`.
    generation: u32,
    /// What the server is, once known: from the generation, else the record.
    tier: Option<Tier>,
    /// Every job this player knows here, newest first.
    jobs: Vec<Job>,
    /// Entry keys of rows dismissed or replaced: never claimed again.
    retired: Vec<String>,
    /// Ids this tracker let go: another player's copy of one is not written
    /// back.
    dropped: BTreeSet<String>,
    /// The jobs the slot held settled when it was last read or written
    /// here: one of them gone from it since was let go by another player
    /// (which lets go of settled rows only — a row written while open and
    /// missing since was lost to a race, and is written again).
    on_file: BTreeSet<String>,
    /// The file couldn't be read or written: the next save waits until
    /// then, rather than retrying every frame.
    save_hold: Option<i64>,
    /// The ping's `noMkdir`.
    no_mkdir: bool,
    /// The account was refused a folder this session (a stale `noMkdir`).
    mkdir_refused: bool,

    // This session only.
    origins: BTreeMap<String, Origin>,
    /// Starts that may fail where they were pressed (a row's retry never does).
    inline_ok: BTreeSet<String>,
    launches: BTreeMap<String, Launch>,
    looks: BTreeMap<String, Look>,
    /// Rows whose Check again, Try again or Add again runs.
    busy: BTreeSet<String>,
    /// Jobs whose older entry read `error` while the folder or the tier
    /// couldn't say whether it was a clash: asked again each poll, failed
    /// once the entry is gone.
    saw_error: BTreeSet<String>,
    /// Job-list reads sent so far, and the count when each job's POST went
    /// out and when its entry came to be known: a read sent before either
    /// can't speak for it.
    reads: u64,
    post_mark: BTreeMap<String, u64>,
    known_mark: BTreeMap<String, u64>,

    calls: CallId,
    pending: BTreeMap<CallId, Pending>,
    poll_out: bool,
    poll_at: Option<i64>,
    /// Polls failed in a row.
    fails: u32,
    /// The last poll got no answer at all: open rows read "Waiting for
    /// connection".
    waiting: bool,
    probe_out: bool,
    reprobe_at: Option<i64>,
    added: AddedList,
    /// The newest record id (any plug-in) a first page showed; 0: the
    /// record was empty.
    newest_record: Option<i64>,

    /// The surfaces up: where a refusal shows.
    showing: BTreeSet<Origin>,
    /// The list is up: outcomes are seen as they land.
    watching: bool,
    events: Vec<JobEvent>,
    unsaved: bool,
    changed: bool,
}

impl<T: Transport, C: Clock, S: Store> Tracker<T, C, S> {
    /// The tracker for one server and account (the store's slot), at the
    /// ping's `ytdl` generation. The jobs it kept load; one that was
    /// mid-POST when the player went may have reached the server, so it is
    /// looked for as one that got no answer; open ones are polled on the
    /// first tick.
    pub(crate) fn new(generation: u32, transport: T, clock: C, mut store: S) -> Tracker<T, C, S> {
        let slot = match store.load() {
            Ok(v) => v.map(|v| Slot::read(&v)).unwrap_or_default(),
            Err(e) => {
                tracing::warn!("ytdl: {e}");
                Slot::default()
            }
        };
        let on_file = settled_ids(&slot.jobs);
        let mut jobs = slot.jobs;
        for job in &mut jobs {
            if job.state == JobState::Starting {
                job.state = JobState::StartUnsure;
            }
        }
        let mut t = Tracker {
            transport,
            clock,
            store,
            generation,
            tier: Tier::from_generation(generation),
            jobs,
            retired: slot.retired,
            dropped: BTreeSet::new(),
            on_file,
            save_hold: None,
            no_mkdir: false,
            mkdir_refused: false,
            origins: BTreeMap::new(),
            inline_ok: BTreeSet::new(),
            launches: BTreeMap::new(),
            looks: BTreeMap::new(),
            busy: BTreeSet::new(),
            saw_error: BTreeSet::new(),
            reads: 0,
            post_mark: BTreeMap::new(),
            known_mark: BTreeMap::new(),
            calls: 0,
            pending: BTreeMap::new(),
            poll_out: false,
            poll_at: None,
            fails: 0,
            waiting: false,
            probe_out: false,
            reprobe_at: None,
            added: AddedList::default(),
            newest_record: None,
            showing: BTreeSet::new(),
            watching: false,
            events: Vec::new(),
            unsaved: false,
            changed: false,
        };
        let now = t.now();
        t.prune(now);
        if t.needs_poll() {
            t.poll_at = Some(now);
        }
        t
    }

    // ── driving ─────────────────────────────────────────────────────────────

    /// The frame's hook: what came back is taken in, and a poll, a re-probe
    /// or a POST whose wait ran out goes. True when anything a surface
    /// shows changed.
    pub(crate) fn tick(&mut self) -> bool {
        for (id, answer) in self.transport.answers() {
            self.answer(id, answer);
        }
        let now = self.now();
        let ready: Vec<String> = self
            .launches
            .iter()
            .filter(|(_, l)| l.mkdir.is_none() && l.until.is_some_and(|until| due(now, until, FLOOR_WAIT_MS)))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            self.try_post(&id);
        }
        if self.needs_poll() {
            let at = *self.poll_at.get_or_insert(now + poll_delay(self.fails));
            if !self.poll_out && due(now, at, poll_delay(u32::MAX)) {
                self.send_poll();
            }
        } else {
            self.poll_at = None;
            if self.waiting {
                self.waiting = false;
                self.changed = true;
            }
        }
        if self.tier.is_none()
            && !self.probe_out
            && self.surfaces_up()
            && self.reprobe_at.is_some_and(|at| due(now, at, REPROBE_MS))
        {
            self.probe();
        }
        self.flush();
        std::mem::take(&mut self.changed)
    }

    /// A call is out: the GUI's loop keeps brisk until it comes back.
    pub(crate) fn in_flight(&self) -> bool {
        !self.pending.is_empty()
    }

    /// What happened since the last ask, in order.
    pub(crate) fn take_events(&mut self) -> Vec<JobEvent> {
        std::mem::take(&mut self.events)
    }

    /// A surface that shows the jobs or their word opened (the room, the
    /// Files window, the list): the tier and the Added list are read, and
    /// an open job is looked at now.
    pub(crate) fn opened(&mut self) {
        self.refresh_added();
        if self.needs_poll() && !self.poll_out {
            self.poll_at = Some(self.now());
        }
        self.flush();
    }

    /// The surface `origin` is up (or gone): a start it pressed that fails
    /// meanwhile is shown there, and leaves no row.
    pub(crate) fn set_showing(&mut self, origin: Origin, on: bool) {
        if on {
            self.showing.insert(origin);
        } else {
            self.showing.remove(&origin);
        }
    }

    /// The From YouTube list is up (or closed): while it is, outcomes are
    /// seen as they land.
    pub(crate) fn watch(&mut self, on: bool) {
        self.watching = on;
        if on {
            self.mark_seen();
        }
    }

    /// The list showed every outcome so far: the word's failed and added
    /// counts clear.
    pub(crate) fn mark_seen(&mut self) {
        let mut changed = false;
        for job in &mut self.jobs {
            if !job.state.open() && !job.seen {
                job.seen = true;
                changed = true;
            }
        }
        if changed {
            self.touch();
        }
        self.flush();
    }

    /// The ping's `noMkdir` (a Try again doesn't remake a folder then).
    pub(crate) fn set_no_mkdir(&mut self, no_mkdir: bool) {
        self.no_mkdir = no_mkdir;
    }

    // ── reading ─────────────────────────────────────────────────────────────

    /// The tier the rules read: the older rules until it is known.
    pub(crate) fn tier(&self) -> Tier {
        self.tier.unwrap_or(Tier::Older)
    }

    #[cfg(test)]
    pub(crate) fn tier_known(&self) -> bool {
        self.tier.is_some()
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }

    /// Every job, newest first.
    #[cfg(test)]
    pub(crate) fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub(crate) fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// The list's adding part: open and not hidden, newest first.
    pub(crate) fn adding(&self) -> Vec<&Job> {
        self.jobs.iter().filter(|j| j.state.open() && !j.hidden).collect()
    }

    /// The list's needs-a-look part: not hidden, at most 7 days old.
    pub(crate) fn needs_look(&self) -> Vec<&Job> {
        let now = self.now();
        self.jobs.iter().filter(|j| !j.hidden && j.state.needs_look() && fresh(j, now)).collect()
    }

    /// The last poll got no answer: open rows read "Waiting for connection".
    pub(crate) fn waiting(&self) -> bool {
        self.waiting
    }

    /// The account was refused a folder this session: the surfaces offer
    /// what they offer under `noMkdir`.
    pub(crate) fn mkdir_refused(&self) -> bool {
        self.mkdir_refused
    }

    /// The row's Check again, Try again or Add again runs.
    #[cfg(test)]
    pub(crate) fn row_busy(&self, id: &str) -> bool {
        self.busy.contains(id)
    }

    /// Which surface pressed `id`'s Add (this session only).
    pub(crate) fn origin_of(&self, id: &str) -> Option<&Origin> {
        self.origins.get(id)
    }

    /// The newest job `origin` pressed this session (a Try again keeps its
    /// row's): the Files door's word carries it.
    pub(crate) fn latest_from(&self, origin: &Origin) -> Option<&Job> {
        self.jobs.iter().find(|j| self.origins.get(&j.id) == Some(origin))
    }

    /// An open job (hidden too: the server keeps going) adds `url` to
    /// `directory`: "Already adding to this folder".
    pub(crate) fn is_adding(&self, url: &str, directory: &str) -> bool {
        self.jobs.iter().any(|j| j.state.open() && j.url == url && bare(&j.directory) == bare(directory))
    }

    /// The word's counts: open jobs; failures, and songs added in the last
    /// hour, the list hasn't shown; whether the list has anything at all.
    pub(crate) fn summary(&self) -> Summary {
        let now = self.now();
        let mut s = Summary::default();
        for job in &self.jobs {
            if job.hidden {
                continue;
            }
            s.history = true;
            if job.state.open() {
                s.adding += 1;
                continue;
            }
            if job.seen {
                continue;
            }
            if job.state.failure() && fresh(job, now) {
                s.failed += 1;
            }
            if let Some(ended) = job.ended_at.filter(|e| job.state == JobState::Added && now - e <= ADDED_NEWS_MS) {
                s.added += 1;
                let ends = ended + ADDED_NEWS_MS;
                s.news_ends = Some(s.news_ends.map_or(ends, |have| have.min(ends)));
            }
        }
        s.history |= self.records_shown() && !self.added.records.is_empty();
        s
    }

    /// The Added list: on 6.30+ the account's record (paged) with this
    /// player's finished jobs no record answers yet; before it (and until
    /// the tier is known) this player's finished jobs. One row per song: a
    /// job whose record is loaded — or names the same file as one — is left
    /// to the record's row, and one older than the loaded pages comes with
    /// Show earlier rather than out of order above it.
    pub(crate) fn added(&self) -> Vec<AddedRow> {
        let full = self.records_shown();
        let loaded: BTreeSet<i64> = if full { self.added.records.iter().map(|r| r.id).collect() } else { BTreeSet::new() };
        let files: BTreeSet<String> = if full {
            self.added.records.iter().filter(|r| !r.filepath.is_empty()).map(|r| r.filepath.to_lowercase()).collect()
        } else {
            BTreeSet::new()
        };
        let cursor = if self.added.more { self.added.cursor } else { None };
        let record_shows = |j: &Job| {
            j.record_id.is_some_and(|id| loaded.contains(&id) || cursor.is_some_and(|c| id < c))
                || j.file_path.as_ref().is_some_and(|p| files.contains(&p.to_lowercase()))
        };
        let mut rows: Vec<AddedRow> = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Added && !(full && record_shows(j)))
            .map(row_of_job)
            .collect();
        if full {
            rows.extend(self.added.records.iter().map(|r| row_of_record(r, &self.jobs)));
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.at.unwrap_or(0)));
        rows
    }

    /// The Added list is being read (the probe, a refresh, Show earlier).
    pub(crate) fn added_loading(&self) -> bool {
        self.added.out || self.probe_out
    }

    /// Show earlier has more to ask for: a page counts every plug-in's
    /// rows, so only an empty page, or one shorter than asked for, ends it.
    pub(crate) fn can_show_earlier(&self) -> bool {
        self.records_shown() && self.added.more
    }

    // ── acting ──────────────────────────────────────────────────────────────

    /// Add to library / Add here: a starting job at once, then the folder
    /// (when one is new or unchecked) and, on 6.30+, the reads before the
    /// POST, then the POST — sent once, never again. Its id; how it went
    /// comes back as [`JobEvent::Started`].
    pub(crate) fn start(&mut self, req: StartRequest) -> String {
        let now = self.now();
        let id = self.fresh_id();
        let sent = req.tags.request();
        let tag = |key: &str| sent.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or_default();
        let tags = YtdlTags { title: tag("title"), artist: tag("artist"), album: tag("album"), year: tag("year") };
        let mkdir = req.mkdir.clone().map(|directory| Mkdir { directory, folder: req.mkdir_folder.clone() });
        let job = Job::fresh(
            id.clone(),
            req.url,
            req.directory,
            req.codec,
            tags,
            req.title,
            req.file_name,
            req.mkdir,
            req.mkdir_folder,
            now,
        );
        self.origins.insert(id.clone(), req.origin);
        self.inline_ok.insert(id.clone());
        self.jobs.insert(0, job);
        self.touch();
        self.launch(&id, mkdir);
        self.flush();
        id
    }

    /// Try again (a failed row, a start failure), Change format and retry,
    /// Choose a folder, Top of the library: a new add of the same song in
    /// place of `id`'s row, its entry never claimed again. A start failure
    /// that had a folder made makes it again first — it may have failed
    /// there — unless the account may not make folders; one that never
    /// needed a folder never asks for one. A failure leaves a row (the list
    /// is up). The new job's id; `None` for an open or unknown one.
    pub(crate) fn retry(&mut self, id: &str, redo: Redo) -> Option<String> {
        let old = self.job(id).filter(|j| !j.state.open())?.clone();
        let now = self.now();
        let codec = redo.codec.unwrap_or_else(|| old.codec.clone());
        // The folder it goes to, the folder kept as made first, and the one
        // made now.
        let (directory, made, made_folder, mkdir) = match redo.directory {
            Some(directory) => {
                let mkdir = redo.mkdir.clone().map(|directory| Mkdir { directory, folder: redo.mkdir_folder.clone() });
                (directory, redo.mkdir, redo.mkdir_folder, mkdir)
            }
            None => {
                let remake = old.state == JobState::StartFailed && old.mkdir.is_some() && !self.may_not_mkdir();
                let folder = match &old.failure {
                    Some(f) if f.problem == Problem::MkdirFailed => f.detail.clone(),
                    _ => old.mkdir_folder.clone(),
                };
                let mkdir =
                    old.mkdir.clone().filter(|_| remake).map(|directory| Mkdir { directory, folder: folder.clone() });
                (old.directory.clone(), old.mkdir.clone(), folder, mkdir)
            }
        };
        let next_id = self.fresh_id();
        let next = Job::fresh(
            next_id.clone(),
            old.url.clone(),
            directory,
            codec.clone(),
            old.tags.clone(),
            old.title.clone(),
            renamed(&old.file_name, &codec),
            made,
            made_folder,
            now,
        );
        if let Some(origin) = self.origins.get(id).cloned() {
            self.origins.insert(next_id.clone(), origin);
        }
        self.forget(id);
        self.jobs.insert(0, next);
        self.busy.insert(next_id.clone());
        self.touch();
        self.launch(&next_id, mkdir);
        self.flush();
        Some(next_id)
    }

    /// "Not found on the server" → Add again: the same add, POSTed once
    /// more (no folder made). False for an open, busy or unknown one.
    pub(crate) fn add_again(&mut self, id: &str) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else { return false };
        if job.state.open() || self.busy.contains(id) {
            return false;
        }
        let key = job.entry_key();
        job.state = JobState::Starting;
        job.server_id = None;
        job.claim = None;
        job.pid = None;
        job.server_start = None;
        job.sent_at = None;
        job.ended_at = None;
        job.seen = false;
        job.failure = None;
        job.file_path = None;
        job.record_id = None;
        job.bytes = None;
        job.track_id = None;
        job.record_floor = None;
        job.prior = None;
        if let Some(key) = key {
            self.retire_key(key);
        }
        self.saw_error.remove(id);
        self.inline_ok.remove(id);
        self.busy.insert(id.to_string());
        self.touch();
        self.launch(id, None);
        self.flush();
        true
    }

    /// "Not sure it started / finished" → Check again: the server's job
    /// list, the record (6.30+) and the folder, for this link and folder.
    /// Found → adding or added; "not found on the server" only once every
    /// one of them answered — one that couldn't be read leaves the row as
    /// it was, to check again. False when it can't be asked now.
    pub(crate) fn check_again(&mut self, id: &str) -> bool {
        if !self.job(id).is_some_and(|j| j.state.not_sure()) || self.looks.contains_key(id) {
            return false;
        }
        self.busy.insert(id.to_string());
        self.look(id, Look::Check { unread: false });
        self.redraw();
        self.flush();
        true
    }

    /// Dismiss a row: a settled job goes, its entry never claimed again;
    /// an open one is hidden (the server keeps going).
    pub(crate) fn dismiss(&mut self, id: &str) {
        let Some(job) = self.job(id) else { return };
        if job.state.open() {
            return self.hide(id);
        }
        self.forget(id);
        self.flush();
    }

    /// Hide from this list: an open job is still followed; the row goes.
    pub(crate) fn hide(&mut self, id: &str) {
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else { return };
        if !job.state.open() || job.hidden {
            return;
        }
        job.hidden = true;
        self.touch();
        self.flush();
    }

    /// Read the record's first page again (6.30+; while the tier is not
    /// known, the probe). Older servers list this player's jobs and ask
    /// nothing.
    pub(crate) fn refresh_added(&mut self) {
        match self.tier {
            None => self.probe(),
            Some(Tier::Full | Tier::New) if !self.added.out => {
                self.added.out = true;
                self.redraw();
                self.send(Call::Added { generation: self.generation, before: None }, Pending::Added { before: None });
            }
            _ => {}
        }
    }

    /// "Show earlier": the rows before the oldest one read. False when
    /// there is nothing more, or a read is out.
    pub(crate) fn show_earlier(&mut self) -> bool {
        let Some(before) = self.added.cursor.filter(|_| self.can_show_earlier() && !self.added_loading()) else {
            return false;
        };
        self.added.out = true;
        self.redraw();
        self.send(Call::Added { generation: self.generation, before: Some(before) }, Pending::Added { before: Some(before) });
        true
    }

    // ── the add ─────────────────────────────────────────────────────────────

    /// An add's sends before its POST. On 6.30+ the record's newest id and
    /// the job list are read alongside the folder (the probe stands in for
    /// the record while the tier is not known), and kept before the POST:
    /// no record and no entry older than this add can ever settle it. An
    /// older server, which keeps an ended entry 30 s and has no record, is
    /// asked neither.
    fn launch(&mut self, id: &str, mkdir: Option<Mkdir>) {
        let now = self.now();
        let mut launch = Launch { mkdir: None, floor: Gate::Skip, prior: Gate::Skip, until: None };
        if self.tier != Some(Tier::Older) {
            launch.floor = match self.tier {
                Some(_) => {
                    let floor = Call::Records { generation: self.generation, limit: 1, before: None };
                    self.send(floor, Pending::Floor { job: id.to_string() });
                    Gate::Out
                }
                None => {
                    self.probe();
                    Gate::Probe
                }
            };
            self.send(Call::Jobs, Pending::Prior { job: id.to_string() });
            launch.prior = Gate::Out;
        }
        match mkdir {
            Some(m) => {
                self.send(Call::Mkdir { directory: m.directory.clone() }, Pending::Mkdir { job: id.to_string() });
                launch.mkdir = Some(m);
            }
            None => launch.until = Some(now + FLOOR_WAIT_MS),
        }
        self.launches.insert(id.to_string(), launch);
        self.try_post(id);
    }

    /// The folder answered: there (any 2xx, a body that would not parse
    /// included), or the add fails as the rules word a folder that couldn't
    /// be made. Never asked again here — it is recursive, so a Try again
    /// makes it again.
    fn on_mkdir(&mut self, id: &str, answer: Result<(), YtdlCallError>) {
        let now = self.now();
        let Some(mkdir) = self.launches.get_mut(id).and_then(|l| l.mkdir.take()) else { return };
        if let Err(e) = answer
            && let Some(f) = mkdir_failure(&call_failure(&e), mkdir.folder.as_deref(), &mkdir.directory)
        {
            if f.problem == Problem::MkdirOff {
                self.mkdir_refused = true;
                self.events.push(JobEvent::Reping);
            }
            return self.start_failed(id, f);
        }
        if let Some(launch) = self.launches.get_mut(id) {
            launch.until = Some(now + FLOOR_WAIT_MS);
        }
        self.try_post(id);
    }

    /// The record's newest id now; a read that failed takes the last one
    /// read.
    fn on_floor(&mut self, id: &str, answer: Result<Option<YtdlRecordPage>, YtdlCallError>) {
        let newest = match answer {
            Ok(Some(page)) => Some(page.newest_id.unwrap_or(0)),
            _ => None,
        };
        if let Some(n) = newest {
            self.note_newest(n);
        }
        let fallback = self.newest_record;
        let Some(launch) = self.launches.get_mut(id).filter(|l| l.floor == Gate::Out) else { return };
        launch.floor = Gate::Done;
        if let Some(job) = self.job_mut(id) {
            job.record_floor = newest.or(fallback);
        }
        self.try_post(id);
    }

    /// The tracker's entries for this link and folder now: none of them is
    /// this add's.
    fn on_prior(&mut self, id: &str, answer: Result<Vec<YtdlEntry>, YtdlCallError>) {
        let Some(launch) = self.launches.get_mut(id).filter(|l| l.prior == Gate::Out) else { return };
        launch.prior = Gate::Done;
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.prior = answer.ok().map(|entries| {
                entries.iter().filter(|e| e.url == job.url && e.directory == job.directory).map(entry_key).collect()
            });
        }
        self.try_post(id);
    }

    /// The POST, once the folder is there and the reads before it answered
    /// — or their wait ran out.
    fn try_post(&mut self, id: &str) {
        let now = self.now();
        let Some(launch) = self.launches.get(id) else { return };
        if launch.mkdir.is_some() {
            return;
        }
        let waits = |g: Gate| matches!(g, Gate::Out | Gate::Probe);
        let late = launch.until.is_some_and(|until| due(now, until, FLOOR_WAIT_MS));
        if !late && (waits(launch.floor) || waits(launch.prior)) {
            return;
        }
        let floor_unread = waits(launch.floor);
        self.launches.remove(id);
        let fallback = self.newest_record;
        if floor_unread && let Some(job) = self.job_mut(id) {
            job.record_floor = fallback;
        }
        self.post(id);
    }

    fn post(&mut self, id: &str) {
        let now = self.now();
        let reads = self.reads;
        let Some(job) = self.job_mut(id) else { return };
        job.sent_at = Some(now);
        let add = YtdlAdd {
            directory: job.directory.clone(),
            url: job.url.clone(),
            codec: job.codec.clone(),
            tags: job.tags.clone(),
        };
        self.post_mark.insert(id.to_string(), reads);
        self.touch();
        // Saved before it goes: a row the player closed on while the POST
        // was out comes back looked for.
        self.flush();
        self.send(Call::Add(add), Pending::Post { job: id.to_string() });
    }

    /// The POST's answer: a 2xx is a job the server has (followed by its id
    /// when the answer names one, else claimed by link and folder); one
    /// that went out and got no answer is looked for; anything else
    /// started nothing.
    fn on_post(&mut self, id: &str, answer: Result<YtdlStarted, YtdlCallError>) {
        let now = self.now();
        self.busy.remove(id);
        if !self.job(id).is_some_and(|j| j.state == JobState::Starting) {
            return;
        }
        let origin = self.origins.get(id).cloned();
        match answer {
            Ok(started) => {
                if let Some(sid) = started.id.as_deref() {
                    // A job that took this entry by its link and folder took
                    // this one's: it looks again.
                    for other in self.jobs.iter_mut().filter(|j| j.id != id && j.state.open()) {
                        if other.claim == Some(Claim::Match) && other.server_id.as_deref() == Some(sid) {
                            other.state = JobState::StartUnsure;
                            other.server_id = None;
                            other.claim = None;
                            other.server_start = None;
                        }
                    }
                    self.known_mark.insert(id.to_string(), self.reads);
                }
                if let Some(job) = self.job_mut(id) {
                    job.state = JobState::Adding;
                    job.claim = started.id.as_ref().map(|_| Claim::Post);
                    job.server_id = started.id;
                    if let Some(name) = started.file_name {
                        job.file_name = name;
                    }
                }
                self.events.push(JobEvent::Started { job: id.to_string(), origin, outcome: StartOutcome::Took });
                // Read at once: an older server's entry is claimed on the
                // first poll after the 2xx.
                self.poll_at = Some(now);
                self.touch();
            }
            Err(e) => {
                let f = classify_add(&call_failure(&e));
                match f.problem {
                    Problem::StartUnsure => {
                        if let Some(job) = self.job_mut(id) {
                            job.state = JobState::StartUnsure;
                        }
                        self.events.push(JobEvent::Started { job: id.to_string(), origin, outcome: StartOutcome::Unsure });
                        self.poll_at = Some(now + poll_delay(0));
                        self.touch();
                    }
                    problem => {
                        match problem {
                            Problem::UploadsOff | Problem::FfmpegNotReady => self.events.push(JobEvent::Reping),
                            Problem::BadFormat => {
                                self.events.push(JobEvent::FormatOff);
                                self.events.push(JobEvent::Reping);
                            }
                            _ => {}
                        }
                        self.start_failed(id, f);
                    }
                }
            }
        }
    }

    /// Nothing started: shown where Add was pressed while that surface is
    /// up (no row is left), else a "Couldn't add" row and its note. A lost
    /// library's name comes from the folder when the server's words lack
    /// it.
    fn start_failed(&mut self, id: &str, failure: Failure) {
        self.launches.remove(id);
        self.busy.remove(id);
        let Some(job) = self.job(id) else { return };
        let failure = if failure.problem == Problem::LibraryGone && failure.detail.is_none() {
            Failure { detail: path_crumbs(&job.directory).into_iter().next(), ..failure }
        } else {
            failure
        };
        let origin = self.origins.get(id).cloned();
        let inline = self.inline_ok.contains(id) && origin.as_ref().is_some_and(|o| self.showing.contains(o));
        if inline {
            self.events.push(JobEvent::Started { job: id.to_string(), origin, outcome: StartOutcome::Refused(failure) });
            self.forget(id);
            return;
        }
        self.events.push(JobEvent::Started {
            job: id.to_string(),
            origin,
            outcome: StartOutcome::Failed(failure.clone()),
        });
        if let Some(job) = self.job_mut(id) {
            job.failure = Some(failure);
        }
        self.settle(id, JobState::StartFailed);
    }

    // ── polling ─────────────────────────────────────────────────────────────

    fn needs_poll(&self) -> bool {
        self.jobs.iter().any(|j| matches!(j.state, JobState::Adding | JobState::StartUnsure))
    }

    fn send_poll(&mut self) {
        self.reads += 1;
        self.poll_out = true;
        self.send(Call::Jobs, Pending::Poll { read: self.reads });
    }

    fn on_poll(&mut self, read: u64, answer: Result<Vec<YtdlEntry>, YtdlCallError>) {
        self.poll_out = false;
        let waiting = match answer {
            Ok(entries) => {
                self.fails = 0;
                self.apply(&entries, read);
                false
            }
            Err(e) => {
                self.fails = self.fails.saturating_add(1);
                e.no_answer().is_some()
            }
        };
        if waiting != self.waiting {
            self.waiting = waiting;
            self.changed = true;
        }
        self.poll_at = Some(self.now() + poll_delay(self.fails));
    }

    /// One read of the job list (`read`, as numbered when it went out)
    /// moves every open job: those that know their entry follow it; the
    /// others, newest first, take the newest entry for their link and
    /// folder that no job holds, or give up after 2 minutes.
    fn apply(&mut self, entries: &[YtdlEntry], read: u64) {
        let now = self.now();
        let knowing: Vec<String> = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Adding && j.knows_entry())
            .map(|j| j.id.clone())
            .collect();
        for id in knowing {
            if self.looks.contains_key(&id) {
                continue;
            }
            let Some(job) = self.job(&id).filter(|j| j.state == JobState::Adding) else { continue };
            match entries.iter().find(|e| job.follows(e)).cloned() {
                Some(e) => self.follow(&id, &e),
                // A read sent before the entry came to be known can't say it
                // is gone: the server makes it just before its 2xx.
                None if read <= self.known_mark.get(&id).copied().unwrap_or(0) => {}
                None => self.vanished(&id),
            }
        }
        let mut held = self.held();
        let new = self.tier == Some(Tier::New);
        let looking: Vec<String> = self
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::StartUnsure | JobState::Adding) && !j.knows_entry())
            .map(|j| j.id.clone())
            .collect();
        for id in looking {
            if self.looks.contains_key(&id) {
                continue;
            }
            // A list read before this job's POST went out (a poll that was
            // in flight when Add was pressed) can't hold its entry, only an
            // earlier add's: the next read's will.
            if read <= self.post_mark.get(&id).copied().unwrap_or(0) {
                continue;
            }
            let Some(job) = self.job(&id).filter(|j| matches!(j.state, JobState::StartUnsure | JobState::Adding)) else {
                continue;
            };
            // Another add of this link into this folder has its POST out: an
            // entry for them is as likely its own.
            if self.sibling_sending(job) {
                continue;
            }
            match unclaimed_for(job, entries, &held, new).cloned() {
                Some(e) => {
                    held.insert(entry_key(&e));
                    self.claim(&id, &e);
                    self.follow(&id, &e);
                }
                None if since(now, job.sent_at.unwrap_or(job.created_at)) > CLAIM_WINDOW_MS => {
                    if job.state == JobState::StartUnsure {
                        self.settle(&id, JobState::NotSureStarted);
                    } else {
                        // Taken (2xx) but never seen in the list: it ended unseen.
                        self.vanished(&id);
                    }
                }
                None => {}
            }
        }
    }

    fn sibling_sending(&self, job: &Job) -> bool {
        self.jobs.iter().any(|x| {
            x.id != job.id
                && x.state == JobState::Starting
                && x.sent_at.is_some()
                && x.url == job.url
                && x.directory == job.directory
        })
    }

    /// Every entry some job holds, or a dismissed one held.
    fn held(&self) -> BTreeSet<String> {
        self.retired.iter().cloned().chain(self.jobs.iter().filter_map(Job::entry_key)).collect()
    }

    fn claim(&mut self, id: &str, e: &YtdlEntry) {
        let reads = self.reads;
        let Some(job) = self.job_mut(id) else { return };
        job.state = JobState::Adding;
        job.ended_at = None;
        match &e.id {
            Some(sid) => {
                job.server_id = Some(sid.clone());
                job.claim = Some(Claim::Match);
            }
            None => job.pid = e.pid,
        }
        job.server_start = e.start_time;
        self.known_mark.insert(id.to_string(), reads);
        self.touch();
    }

    /// What an entry's status makes of the job that follows it: complete →
    /// added, error → a failure; downloading (or a status this build doesn't
    /// know) → still adding. The new server's entry names the file and
    /// says why it failed; an older one's leaves that to the record and the
    /// folder.
    fn follow(&mut self, id: &str, e: &YtdlEntry) {
        let Some(job) = self.job_mut(id) else { return };
        let mut moved = false;
        if e.start_time.is_some() && job.server_start != e.start_time {
            job.server_start = e.start_time;
            moved = true;
        }
        if let Some(name) = e.file_name.as_ref().filter(|n| **n != job.file_name) {
            job.file_name = name.clone();
            moved = true;
        }
        let said = e.id.is_some();
        if e.complete() {
            if said {
                job.file_path = e.file_path.clone();
                job.record_id = e.download_id;
                self.settle(id, JobState::Added);
            } else {
                self.look(id, Look::Landed);
            }
        } else if e.failed() {
            if said {
                job.failure = entry_failure(&e.status, e.reason.as_deref(), e.error.as_deref(), e.file_name.as_deref());
                self.settle(id, JobState::Failed);
            } else {
                self.erred(id, false);
            }
        } else if moved {
            self.touch();
        }
    }

    /// An older entry ended in `error`. Only 6.30+ refuses a name clash (an
    /// older server re-tags instead): there the predicted name in the
    /// folder is a clash, else a failure. While the folder won't list (or
    /// the tier is not known) the job stays adding and the next poll asks
    /// again — a clash read as a plain failure would offer a Try again that
    /// fails the same way. Once the entry is gone it settles on what is
    /// known.
    fn erred(&mut self, id: &str, gone: bool) {
        self.saw_error.insert(id.to_string());
        match self.tier {
            Some(Tier::Older) => {
                if let Some(job) = self.job_mut(id) {
                    job.failure = Some(unsaid(Phase::Download));
                }
                self.settle(id, JobState::Failed);
            }
            _ => {
                if self.tier.is_none() {
                    self.probe();
                }
                self.look(id, Look::Erred { gone });
            }
        }
    }

    /// A job whose entry is gone without this player seeing it end (away
    /// past an older server's 30 s, a restart, never seen at all): the
    /// record (6.30+) or the folder (older) says added; else not sure it
    /// finished — or, on the new server inside its day, not found. A read
    /// that fails leaves it for the next poll. One seen in `error` settles
    /// as a failure.
    fn vanished(&mut self, id: &str) {
        if self.saw_error.contains(id) {
            return self.erred(id, true);
        }
        self.look(id, Look::Vanished);
    }

    // ── looks: the record and the folder ────────────────────────────────────

    fn look(&mut self, id: &str, kind: Look) {
        let step = match kind {
            Look::Erred { .. } => Step::Listing,
            Look::Check { .. } => Step::Jobs,
            Look::Landed | Look::Vanished if self.tier == Some(Tier::Older) => Step::Listing,
            Look::Landed | Look::Vanished => Step::Record { page: 0, before: None },
        };
        self.looks.insert(id.to_string(), kind);
        self.step(id, step);
    }

    fn step(&mut self, id: &str, step: Step) {
        let call = match step {
            Step::Jobs => Call::Jobs,
            Step::Record { before, .. } => Call::Records { generation: self.generation, limit: RECORD_PAGE, before },
            Step::Listing => match self.job(id) {
                Some(job) => Call::Listing { directory: job.directory.clone() },
                None => return self.end_look(id),
            },
        };
        self.send(call, Pending::Look { job: id.to_string(), step });
    }

    fn end_look(&mut self, id: &str) {
        if let Some(Look::Check { .. }) = self.looks.remove(id) {
            self.busy.remove(id);
            self.redraw();
        }
    }

    /// The job is still what the look was for: open for an entry's look,
    /// not sure for Check again (a dismiss meanwhile ends it).
    fn still(&self, id: &str, kind: Look) -> bool {
        self.job(id).is_some_and(|j| match kind {
            Look::Check { .. } => j.state.not_sure(),
            _ => j.state == JobState::Adding,
        })
    }

    /// Check again's job list: its entry (by id, or its pid), or — one that
    /// never knew an entry — the newest for its link and folder that no job
    /// holds. Found → adding again, followed; else the record or the folder.
    fn on_check_jobs(&mut self, id: &str, answer: Result<Vec<YtdlEntry>, YtdlCallError>) {
        let Ok(entries) = answer else { return self.end_look(id) };
        let Some(job) = self.job(id) else { return self.end_look(id) };
        let entry = if job.knows_entry() {
            entries.iter().find(|e| job.follows(e))
        } else {
            unclaimed_for(job, &entries, &self.held(), self.tier == Some(Tier::New))
        }
        .cloned();
        match entry {
            Some(e) => {
                self.end_look(id);
                self.claim(id, &e);
                self.follow(id, &e);
                let at = self.now() + poll_delay(0);
                self.poll_at = Some(self.poll_at.map_or(at, |have| have.min(at)));
            }
            None if self.tier == Some(Tier::Older) => self.step(id, Step::Listing),
            None => self.step(id, Step::Record { page: 0, before: None }),
        }
    }

    /// One page of the record, for a job's look: its record (the newest row
    /// that answers for it and that no other job holds), else the next page
    /// — until the last one (empty, or short), one that reaches back past
    /// the job's start, floor or record, or [`RECORD_PAGES`] pages. A page at
    /// all says the route is there (6.30+); Express's 404 says it isn't.
    fn on_record(&mut self, id: &str, page: usize, before: Option<i64>, answer: Result<Option<YtdlRecordPage>, YtdlCallError>) {
        let found = match answer {
            Err(_) => return self.record_unread(id),
            Ok(None) => {
                if self.generation == 0 {
                    self.set_tier(Tier::Older);
                }
                return self.record_missing(id, true);
            }
            Ok(Some(found)) => found,
        };
        if self.tier.is_none() {
            self.set_tier(Tier::Full);
        }
        if before.is_none() {
            self.note_newest(found.newest_id.unwrap_or(0));
        }
        let taken: BTreeSet<i64> = self.jobs.iter().filter(|j| j.id != id).filter_map(|j| j.record_id).collect();
        let Some(job) = self.job(id) else { return self.end_look(id) };
        if let Some(r) = found.rows.iter().find(|r| !taken.contains(&r.id) && answers(job, r, false)).cloned() {
            self.end_look(id);
            if let Some(job) = self.job_mut(id) {
                job.take_record(&r);
            }
            return self.settle(id, JobState::Added);
        }
        let past_start = job.server_start.zip(found.oldest_at).is_some_and(|(start, at)| at < start);
        let past_floor = job.record_floor.zip(found.oldest_id).is_some_and(|(floor, oldest)| oldest <= floor);
        let past_record = job.record_id.zip(found.oldest_id).is_some_and(|(want, oldest)| oldest <= want);
        if found.end() || page + 1 >= RECORD_PAGES || past_start || past_floor || past_record {
            return self.record_missing(id, false);
        }
        self.step(id, Step::Record { page: page + 1, before: found.oldest_id });
    }

    /// The record has no row for the job (`no_route`: there is no record).
    fn record_missing(&mut self, id: &str, no_route: bool) {
        let Some(kind) = self.looks.get(id).copied() else { return };
        match kind {
            Look::Landed => self.step(id, Step::Listing),
            Look::Vanished | Look::Check { .. } if no_route => self.step(id, Step::Listing),
            Look::Vanished => {
                self.end_look(id);
                let young = self.job(id).is_some_and(|j| since(self.now(), j.created_at) <= SERVER_KEEP_MS);
                let state = if self.tier == Some(Tier::New) && young { JobState::NotFound } else { JobState::NotSureFinished };
                self.settle(id, state);
            }
            Look::Check { .. } if self.tier == Some(Tier::New) => {
                self.end_look(id);
                self.settle(id, JobState::NotFound);
            }
            Look::Check { .. } => self.step(id, Step::Listing),
            Look::Erred { .. } => self.end_look(id),
        }
    }

    /// The record couldn't be read: an entry that completed is still added
    /// (the record joins the row once a page holds it); a gone one is asked
    /// about again next poll; Check again asks the folder, which then can't
    /// say "not found" alone.
    fn record_unread(&mut self, id: &str) {
        let Some(kind) = self.looks.get(id).copied() else { return };
        match kind {
            Look::Landed => self.step(id, Step::Listing),
            Look::Check { .. } if self.tier != Some(Tier::New) => {
                self.looks.insert(id.to_string(), Look::Check { unread: true });
                self.step(id, Step::Listing);
            }
            _ => self.end_look(id),
        }
    }

    /// The folder's listing: the job's predicted name as the folder lists
    /// it (the disk's case), `""` when it listed without it, `None` when it
    /// did not list.
    fn on_listing(&mut self, id: &str, answer: Result<YtdlListing, YtdlCallError>) {
        let Some(kind) = self.looks.get(id).copied() else { return };
        self.end_look(id);
        let Some(job) = self.job(id) else { return };
        let want = job.file_name.to_lowercase();
        let name = answer.ok().map(|listing| {
            if want.is_empty() {
                return String::new();
            }
            listing.files.into_iter().find(|f| f.to_lowercase() == want).unwrap_or_default()
        });
        let path = name.as_deref().filter(|n| !n.is_empty()).map(|n| format!("{}/{n}", bare(&job.directory)));
        match kind {
            Look::Landed => {
                if let Some(job) = self.job_mut(id) {
                    job.file_path = path;
                }
                self.settle(id, JobState::Added);
            }
            Look::Erred { gone } => {
                let full = self.tier == Some(Tier::Full);
                if !gone && (!full || name.is_none()) {
                    return;
                }
                self.saw_error.remove(id);
                let failure = if full && path.is_some() {
                    Failure { problem: Problem::Exists, ..unsaid(Phase::Download) }
                } else {
                    unsaid(Phase::Download)
                };
                if let Some(job) = self.job_mut(id) {
                    job.failure = Some(failure);
                }
                self.settle(id, JobState::Failed);
            }
            Look::Vanished => match (name, path) {
                (None, _) => {}
                (Some(_), Some(path)) => {
                    if let Some(job) = self.job_mut(id) {
                        job.file_path = Some(path);
                    }
                    self.settle(id, JobState::Added);
                }
                (Some(_), None) => self.settle(id, JobState::NotSureFinished),
            },
            Look::Check { unread } => match (name, path) {
                (_, Some(path)) => {
                    if let Some(job) = self.job_mut(id) {
                        job.file_path = Some(path);
                    }
                    self.settle(id, JobState::Added);
                }
                (Some(_), None) if !unread && self.tier.is_some() => self.settle(id, JobState::NotFound),
                _ => {}
            },
        }
    }

    // ── settling ────────────────────────────────────────────────────────────

    /// `id` reached `state`: kept, told once ([`JobEvent::Settled`], its
    /// note), seen when the list is up. A song that came in refreshes the
    /// Added list (6.30+).
    fn settle(&mut self, id: &str, state: JobState) {
        let now = self.now();
        let seen = self.watching;
        let Some(job) = self.job_mut(id) else { return };
        job.state = state;
        job.ended_at = Some(now);
        job.seen = seen;
        let note = match state {
            JobState::Added => Some(Note::Added { title: job.shown_title(), folder: job.folder() }),
            JobState::Failed | JobState::StartFailed => Some(Note::CouldntAdd { title: job.shown_title() }),
            _ => None,
        };
        self.saw_error.remove(id);
        self.events.push(JobEvent::Settled { job: id.to_string(), state, note });
        self.touch();
        if state == JobState::Added && self.records_shown() {
            self.refresh_added();
        }
    }

    // ── the tier and the Added list ─────────────────────────────────────────

    /// Below generation 1, once per session: the Added list's first read —
    /// an answer is 6.30+ and the list with it, Express's 404 an older
    /// server; anything else leaves it unknown, asked again while a surface
    /// shows.
    fn probe(&mut self) {
        if self.tier.is_some() || self.probe_out {
            return;
        }
        self.probe_out = true;
        self.reprobe_at = None;
        self.redraw();
        self.send(Call::Added { generation: self.generation, before: None }, Pending::Probe);
    }

    fn on_probe(&mut self, answer: Result<Option<YtdlAddedRead>, YtdlCallError>) {
        self.probe_out = false;
        match answer {
            Ok(Some(read)) => {
                self.set_tier(Tier::Full);
                self.note_newest(read.newest_id.unwrap_or(0));
                self.added.records = read.rows;
                self.added.cursor = read.cursor;
                self.added.more = !read.end;
                self.attribute();
            }
            Ok(None) => self.set_tier(Tier::Older),
            Err(_) => self.reprobe_at = Some(self.now() + REPROBE_MS),
        }
        // The adds that waited on it for their floor.
        let floor = if self.tier == Some(Tier::Full) { self.newest_record } else { None };
        let waiting: Vec<String> =
            self.launches.iter().filter(|(_, l)| l.floor == Gate::Probe).map(|(id, _)| id.clone()).collect();
        for id in waiting {
            if let Some(launch) = self.launches.get_mut(&id) {
                launch.floor = Gate::Done;
            }
            if let Some(job) = self.job_mut(&id) {
                job.record_floor = floor;
            }
            self.try_post(&id);
        }
        self.redraw();
    }

    /// A read of the Added list: from the top, the newest rows replace the
    /// top of the list (pages Show earlier brought stay below them); from
    /// `before`, the next rows join below. One that fails keeps what was
    /// there.
    fn on_added(&mut self, before: Option<i64>, answer: Result<Option<YtdlAddedRead>, YtdlCallError>) {
        self.added.out = false;
        self.redraw();
        let read = match answer {
            Ok(Some(read)) => read,
            Ok(None) => {
                if self.generation == 0 {
                    self.set_tier(Tier::Older);
                }
                return;
            }
            Err(_) => return,
        };
        match before {
            None => {
                self.note_newest(read.newest_id.unwrap_or(0));
                let cut = read.cursor;
                match (cut, self.added.cursor) {
                    (Some(cut), Some(older)) if !read.end && older < cut => {
                        let fresh: BTreeSet<i64> = read.rows.iter().map(|r| r.id).collect();
                        let kept: Vec<YtdlRecord> =
                            self.added.records.drain(..).filter(|r| r.id < cut && !fresh.contains(&r.id)).collect();
                        self.added.records = read.rows;
                        self.added.records.extend(kept);
                    }
                    _ => {
                        self.added.records = read.rows;
                        self.added.cursor = cut;
                        self.added.more = !read.end;
                    }
                }
            }
            Some(before) => {
                let have: BTreeSet<i64> = self.added.records.iter().map(|r| r.id).collect();
                self.added.records.extend(read.rows.into_iter().filter(|r| !have.contains(&r.id)));
                self.added.cursor = read.cursor.or(Some(before));
                if read.end {
                    self.added.more = false;
                }
            }
        }
        self.attribute();
    }

    /// Added jobs that settled before their record could be read take it
    /// once a loaded page holds it — bounded by the job's start or floor, a
    /// record answering for one job only — so the list shows the song once
    /// and a later add can't take it.
    fn attribute(&mut self) {
        let mut taken: BTreeSet<i64> = self.jobs.iter().filter_map(|j| j.record_id).collect();
        let mut changed = false;
        for job in self.jobs.iter_mut().filter(|j| j.state == JobState::Added && j.record_id.is_none()) {
            if let Some(r) = self.added.records.iter().find(|r| !taken.contains(&r.id) && answers(job, r, true)) {
                job.take_record(r);
                taken.insert(r.id);
                changed = true;
            }
        }
        if changed {
            self.touch();
        }
    }

    fn set_tier(&mut self, tier: Tier) {
        if self.tier != Some(tier) {
            self.tier = Some(tier);
            self.redraw();
        }
    }

    /// The record's rows are the Added list's (6.30+).
    fn records_shown(&self) -> bool {
        matches!(self.tier, Some(Tier::Full | Tier::New))
    }

    fn note_newest(&mut self, id: i64) {
        if self.newest_record.is_none_or(|had| id > had) {
            self.newest_record = Some(id);
        }
    }

    // ── keeping ─────────────────────────────────────────────────────────────

    /// Failed rows go after 7 days; at most [`KEEP_JOBS`] stay, the oldest
    /// settled ones dropped first. Open jobs always stay.
    fn prune(&mut self, now: i64) {
        let stale: Vec<String> = self
            .jobs
            .iter()
            .filter(|j| !j.state.open() && j.state.needs_look() && !fresh(j, now))
            .map(|j| j.id.clone())
            .collect();
        for id in stale {
            self.let_go(&id);
        }
        let mut extra = self.jobs.len().saturating_sub(KEEP_JOBS);
        let mut at = self.jobs.len();
        while extra > 0 && at > 0 {
            at -= 1;
            if self.jobs[at].state.open() {
                continue;
            }
            let id = self.jobs[at].id.clone();
            self.forget(&id);
            extra -= 1;
        }
    }

    /// A row goes, its entry never claimed again.
    fn forget(&mut self, id: &str) {
        if let Some(key) = self.job(id).and_then(Job::entry_key) {
            self.retire_key(key);
        }
        self.let_go(id);
    }

    /// A row goes.
    fn let_go(&mut self, id: &str) {
        let Some(at) = self.jobs.iter().position(|j| j.id == id) else { return };
        self.jobs.remove(at);
        self.dropped.insert(id.to_string());
        self.launches.remove(id);
        self.looks.remove(id);
        self.busy.remove(id);
        self.saw_error.remove(id);
        self.post_mark.remove(id);
        self.known_mark.remove(id);
        self.inline_ok.remove(id);
        self.touch();
    }

    fn retire_key(&mut self, key: String) {
        if !self.retired.contains(&key) {
            self.retired.push(key);
        }
        if self.retired.len() > RETIRED_CAP {
            let extra = self.retired.len() - RETIRED_CAP;
            self.retired.drain(..extra);
        }
    }

    /// The slot written, when anything in it changed: pruned, and merged
    /// with what another player sharing the directory wrote since — its
    /// jobs this tracker never held (or let go) are written back with ours,
    /// so neither clobbers the other's. They join this tracker on the next
    /// load. A settled row the other let go meanwhile (dismissed, replaced
    /// by a retry, pruned) goes here too, its entry retired with it:
    /// written back, it would come back there. An open one stays whatever
    /// the file says. A file that can't be read is never merged as an
    /// empty one; the save waits a beat and goes again.
    fn flush(&mut self) {
        if !self.unsaved {
            return;
        }
        let now = self.now();
        if let Some(at) = self.save_hold {
            if !due(now, at, poll_delay(0)) {
                return;
            }
            self.save_hold = None;
        }
        let stored = match self.store.load() {
            Ok(v) => v.map(|v| Slot::read(&v)).unwrap_or_default(),
            Err(e) => return self.hold_save(now, &e),
        };
        let let_go: Vec<String> = self
            .jobs
            .iter()
            .filter(|j| self.on_file.contains(&j.id) && !j.state.open() && !stored.jobs.iter().any(|s| s.id == j.id))
            .map(|j| j.id.clone())
            .collect();
        for id in let_go {
            self.forget(&id);
        }
        self.prune(now);
        let mut slot = Slot { jobs: self.jobs.clone(), retired: self.retired.clone() };
        for job in stored.jobs {
            if !self.dropped.contains(&job.id) && !slot.jobs.iter().any(|j| j.id == job.id) {
                slot.jobs.push(job);
            }
        }
        for key in stored.retired {
            if !slot.retired.contains(&key) {
                slot.retired.insert(0, key);
            }
        }
        if slot.retired.len() > RETIRED_CAP {
            let extra = slot.retired.len() - RETIRED_CAP;
            slot.retired.drain(..extra);
        }
        slot.jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        // The other's retired entries are never claimed here either.
        self.retired = slot.retired.clone();
        match self.store.save(&slot.to_json()) {
            Ok(()) => {
                self.unsaved = false;
                self.on_file = settled_ids(&slot.jobs);
            }
            Err(e) => self.hold_save(now, &e),
        }
    }

    /// The file couldn't be read or written: still unsaved, asked again a
    /// beat on.
    fn hold_save(&mut self, now: i64, e: &str) {
        tracing::warn!("ytdl: {e}");
        self.save_hold = Some(now + poll_delay(0));
    }

    // ── plumbing ────────────────────────────────────────────────────────────

    fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    fn send(&mut self, call: Call, why: Pending) {
        self.calls += 1;
        let id = self.calls;
        self.pending.insert(id, why);
        self.transport.send(id, call);
    }

    fn answer(&mut self, id: CallId, answer: Answer) {
        let Some(why) = self.pending.remove(&id) else { return };
        match why {
            Pending::Mkdir { job } => self.on_mkdir(&job, answer.mkdir()),
            Pending::Floor { job } => self.on_floor(&job, answer.records()),
            Pending::Prior { job } => self.on_prior(&job, answer.jobs()),
            Pending::Post { job } => self.on_post(&job, answer.add()),
            Pending::Poll { read } => self.on_poll(read, answer.jobs()),
            Pending::Probe => self.on_probe(answer.added()),
            Pending::Added { before } => self.on_added(before, answer.added()),
            Pending::Look { job, step } => {
                let Some(kind) = self.looks.get(&job).copied() else { return };
                if !self.still(&job, kind) {
                    return self.end_look(&job);
                }
                match step {
                    Step::Jobs => self.on_check_jobs(&job, answer.jobs()),
                    Step::Record { page, before } => self.on_record(&job, page, before, answer.records()),
                    Step::Listing => self.on_listing(&job, answer.listing()),
                }
            }
        }
    }

    fn job_mut(&mut self, id: &str) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    fn fresh_id(&self) -> String {
        loop {
            let id = format!("yt-{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
            if !self.jobs.iter().any(|j| j.id == id) {
                return id;
            }
        }
    }

    fn may_not_mkdir(&self) -> bool {
        self.no_mkdir || self.mkdir_refused
    }

    fn surfaces_up(&self) -> bool {
        self.watching || !self.showing.is_empty()
    }

    /// A job changed: saved, and redrawn.
    fn touch(&mut self) {
        self.unsaved = true;
        self.changed = true;
    }

    /// Only what a surface shows changed.
    fn redraw(&mut self) {
        self.changed = true;
    }

    #[cfg(test)]
    fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    #[cfg(test)]
    fn clock_mut(&mut self) -> &mut C {
        &mut self.clock
    }

    #[cfg(test)]
    fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }
}

// ── The rules the tracker keeps ─────────────────────────────────────────────

/// The ids of the settled jobs among `jobs`.
fn settled_ids(jobs: &[Job]) -> BTreeSet<String> {
    jobs.iter().filter(|j| !j.state.open()).map(|j| j.id.clone()).collect()
}

/// The poll's next beat after `fails` failed reads in a row.
fn poll_delay(fails: u32) -> i64 {
    i64::try_from(crate::tui::app::keep::keep_backoff(POLL_BEAT, fails).as_millis()).unwrap_or(i64::MAX)
}

/// `at` has come — or lies further ahead than a beat could: a wall clock
/// set back must not hold anything for as long as it went back.
fn due(now: i64, at: i64, span: i64) -> bool {
    now >= at || at - now > span
}

/// How long ago `at` was, either way: a wall clock set back must not stall
/// a bound for as long as it went back (the webapp's `since`).
fn since(now: i64, at: i64) -> i64 {
    now.saturating_sub(at).saturating_abs()
}

/// A needs-a-look row younger than 7 days.
fn fresh(job: &Job, now: i64) -> bool {
    now - job.ended_at.unwrap_or(job.created_at) <= KEEP_FAILED_MS
}

/// The newest entry for `job`'s link and folder that no job holds and that
/// was not there before its POST ([`Job::prior`]). Without that list: on the
/// new server, whose list keeps a day of settled entries, none started well
/// before the press; on an older one (30 s of them), any.
fn unclaimed_for<'a>(job: &Job, entries: &'a [YtdlEntry], held: &BTreeSet<String>, new: bool) -> Option<&'a YtdlEntry> {
    let floor = job.created_at - CLAIM_SLACK_MS;
    let mut best: Option<&YtdlEntry> = None;
    for e in entries.iter().filter(|e| e.url == job.url && e.directory == job.directory) {
        let key = entry_key(e);
        if held.contains(&key) {
            continue;
        }
        let earlier = match &job.prior {
            Some(prior) => prior.contains(&key),
            None => new && !e.start_time.is_some_and(|at| at >= floor),
        };
        if earlier {
            continue;
        }
        if best.is_none_or(|b| e.start_time.unwrap_or(i64::MIN) > b.start_time.unwrap_or(i64::MIN)) {
            best = Some(e);
        }
    }
    best
}

/// `r` is `job`'s download record: the one the entry named, else its link
/// as `origin`, in its folder (without the slashes, ignoring case — the
/// record keeps the disk's), and not older than the job: written at or
/// after the claimed entry's start (both the server's clock) and newer
/// than the record's newest row before the POST. With neither bound known
/// (a job saved by an older build, a floor that couldn't be read) any match
/// counts — unless `bounded`.
fn answers(job: &Job, r: &YtdlRecord, bounded: bool) -> bool {
    if let Some(id) = job.record_id {
        return r.id == id;
    }
    if r.origin != job.url || folder_of(&r.filepath).to_lowercase() != bare(&job.directory).to_lowercase() {
        return false;
    }
    if let Some(start) = job.server_start
        && r.downloaded_at.is_none_or(|at| at < start)
    {
        return false;
    }
    if job.record_floor.is_some_and(|floor| r.id <= floor) {
        return false;
    }
    !bounded || job.server_start.is_some() || job.record_floor.is_some()
}

/// The predicted name with another format's extension.
fn renamed(name: &str, codec: &str) -> String {
    if name.is_empty() { String::new() } else { format!("{}.{}", stem_of(name), output_extension(codec)) }
}

fn row_of_job(j: &Job) -> AddedRow {
    let name = j.file_path.as_deref().map(name_of).unwrap_or(&j.file_name);
    AddedRow {
        title: j.shown_title(),
        folder: j.folder(),
        file_path: j.file_path.clone(),
        ext: ext_of(name).or_else(|| Some(output_extension(&j.codec)).filter(|e| !e.is_empty())),
        bytes: j.bytes,
        at: j.ended_at,
        present: true,
        record_id: j.record_id,
        track_id: j.track_id,
        job: Some(j.id.clone()),
    }
}

fn row_of_record(r: &YtdlRecord, jobs: &[Job]) -> AddedRow {
    let name = name_of(&r.filepath);
    AddedRow {
        title: r.title.clone().unwrap_or_else(|| stem_of(name).to_string()),
        folder: folder_of(&r.filepath),
        file_path: Some(r.filepath.clone()).filter(|p| !p.is_empty()),
        ext: ext_of(name),
        bytes: r.bytes,
        at: r.downloaded_at,
        present: r.present,
        record_id: Some(r.id),
        track_id: r.track_id,
        job: jobs.iter().find(|j| j.record_id == Some(r.id)).map(|j| j.id.clone()),
    }
}

/// `/Music/Daft Punk/` and `Music/Daft Punk` are one folder: compared
/// without the slashes at either end.
fn bare(path: &str) -> &str {
    path.trim_matches('/')
}

/// The folder with one slash at either end (`/` for none): how a server
/// spells the folder it listed.
#[cfg(test)]
fn slashed(path: &str) -> String {
    let b = bare(path);
    if b.is_empty() { "/".into() } else { format!("/{b}/") }
}

/// `Music/Daft Punk/One_More_Time.mp3` → `Music/Daft Punk`.
fn folder_of(filepath: &str) -> String {
    let b = bare(filepath);
    b.rfind('/').map(|at| b[..at].to_string()).unwrap_or_default()
}

fn name_of(filepath: &str) -> &str {
    let b = bare(filepath);
    b.rfind('/').map_or(b, |at| &b[at + 1..])
}

fn stem_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

fn ext_of(name: &str) -> Option<String> {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => Some(name[dot + 1..].to_lowercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::ytdl::{Surface, Words, failure_label};

    /// The player's clock when every test starts.
    const T0: i64 = 1_762_600_000_000;
    /// The server's clock runs ahead of the player's: nothing may compare
    /// the two but within a slack.
    const SKEW: i64 = 2_000;

    const LINK: &str = "https://www.youtube.com/watch?v=FGBhQbmPwH8";
    const OTHER: &str = "https://www.youtube.com/watch?v=yca6UsllwYs";
    const THIRD: &str = "https://www.youtube.com/watch?v=IluRBvnYMoY";
    const DIR: &str = "/Music/Daft Punk/Discovery/";

    /// The name the server gives a link's file, without the extension.
    fn stem(url: &str) -> &'static str {
        match url {
            LINK => "One_More_Time",
            OTHER => "Aerodynamic",
            _ => "Digital_Love",
        }
    }

    fn title(url: &str) -> &'static str {
        match url {
            LINK => "One More Time",
            OTHER => "Aerodynamic",
            _ => "Digital Love",
        }
    }

    fn request(url: &str, directory: &str, origin: Origin) -> StartRequest {
        StartRequest {
            url: url.into(),
            directory: directory.into(),
            codec: "mp3".into(),
            tags: Tags { title: format!(" {} ", title(url)), artist: "Daft Punk".into(), album: String::new(), year: "01".into() },
            title: title(url).into(),
            file_name: format!("{}.mp3", stem(url)),
            mkdir: None,
            mkdir_folder: None,
            origin,
        }
    }

    fn answered(status: u16, words: &str) -> YtdlCallError {
        YtdlCallError::Answered { status, words: Some(words.into()), reason: None, file_name: None }
    }

    fn unanswered(why: NoAnswer) -> YtdlCallError {
        YtdlCallError::Unanswered { why, detail: "the test's".into() }
    }

    fn entry(id: Option<&str>, pid: i64, url: &str, status: &str, start: i64) -> YtdlEntry {
        YtdlEntry {
            id: id.map(str::to_string),
            pid: Some(pid),
            url: url.into(),
            directory: DIR.into(),
            output_codec: "mp3".into(),
            status: status.into(),
            start_time: Some(start),
            ..YtdlEntry::default()
        }
    }

    fn record(id: i64, plugin: &str, filepath: &str, origin: &str, at: i64) -> YtdlRecord {
        YtdlRecord {
            id,
            plugin: plugin.into(),
            vpath: "Music".into(),
            filepath: filepath.into(),
            relative_path: filepath.split_once('/').map(|(_, rest)| rest.to_string()).unwrap_or_default(),
            origin: origin.into(),
            title: None,
            artist: None,
            album: None,
            bytes: Some(4_200_000),
            downloaded_at: Some(at),
            present: true,
            track_id: Some(id * 10),
        }
    }

    fn kind(call: &Call) -> &'static str {
        match call {
            Call::Mkdir { .. } => "mkdir",
            Call::Add(_) => "add",
            Call::Jobs => "jobs",
            Call::Records { .. } => "records",
            Call::Added { .. } => "added",
            Call::Listing { .. } => "listing",
        }
    }

    /// The scripted transport: what went out, what comes back.
    #[derive(Default)]
    struct Wire {
        out: Vec<(CallId, Call)>,
        back: Vec<(CallId, Answer)>,
        log: Vec<Call>,
    }

    impl Transport for Wire {
        fn send(&mut self, id: CallId, call: Call) {
            self.log.push(call.clone());
            self.out.push((id, call));
        }

        fn answers(&mut self) -> Vec<(CallId, Answer)> {
            std::mem::take(&mut self.back)
        }
    }

    struct Tock(i64);

    impl Clock for Tock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    #[derive(Debug, Clone, Default)]
    struct Shelf {
        slot: Option<Value>,
    }

    impl Store for Shelf {
        fn load(&mut self) -> Result<Option<Value>, String> {
            Ok(self.slot.clone())
        }

        fn save(&mut self, slot: &Value) -> Result<(), String> {
            self.slot = Some(slot.clone());
            Ok(())
        }
    }

    /// A server of either generation, as its routes answer: generation 1
    /// (ids, the 409, reasons, its own record filter), or 0 — with the
    /// record (6.30–6.33) or without it (before 6.30).
    struct Fake {
        generation: u32,
        /// The download record; `None`: Express's own 404.
        records: Option<Vec<YtdlRecord>>,
        /// The job list, newest first.
        entries: Vec<YtdlEntry>,
        /// The list answers empty, as if the server hadn't the job yet.
        unlisted: bool,
        /// Folders, no slashes at the ends, and their files.
        folders: BTreeMap<String, Vec<String>>,
        now: i64,
        next_pid: i64,
        next_job: u32,
        next_record: i64,
        /// What the next call of a kind answers instead.
        fail: BTreeMap<&'static str, VecDeque<YtdlCallError>>,
        /// The next add starts its job, and its answer is lost on the way.
        lose_add: bool,
        /// The next add starts its job, and answers a 2xx this player can't
        /// read.
        garble_add: bool,
        /// Calls of these kinds wait, unanswered, until let go.
        hold: BTreeSet<&'static str>,
    }

    impl Fake {
        fn new(generation: u32, record: bool) -> Fake {
            Fake {
                generation,
                records: record.then(Vec::new),
                entries: Vec::new(),
                unlisted: false,
                folders: [("Music".to_string(), Vec::new()), (bare(DIR).to_string(), Vec::new())].into(),
                now: T0 + SKEW,
                next_pid: 4100,
                next_job: 0,
                next_record: 500,
                fail: BTreeMap::new(),
                lose_add: false,
                garble_add: false,
                hold: BTreeSet::new(),
            }
        }

        fn fail(&mut self, kind: &'static str, e: YtdlCallError) {
            self.fail.entry(kind).or_default().push_back(e);
        }

        fn files(&mut self, folder: &str) -> &mut Vec<String> {
            self.folders.entry(bare(folder).to_string()).or_default()
        }

        fn answer(&mut self, call: &Call) -> Answer {
            if let Some(e) = self.fail.get_mut(kind(call)).and_then(VecDeque::pop_front) {
                return call.failed(e);
            }
            let new = self.generation >= 1;
            match call {
                Call::Mkdir { directory } => {
                    self.files(directory);
                    Answer::Mkdir(Ok(()))
                }
                Call::Add(add) => {
                    let name = format!("{}.{}", stem(&add.url), output_extension(&add.codec));
                    let taken = self.folders.get(bare(&add.directory)).is_some_and(|f| f.iter().any(|n| n.eq_ignore_ascii_case(&name)));
                    if new && taken {
                        return Answer::Add(Err(YtdlCallError::Answered {
                            status: 409,
                            words: Some(format!("“{name}” is already in this folder, so nothing was added.")),
                            reason: Some("exists".into()),
                            file_name: Some(name),
                        }));
                    }
                    self.next_pid += 1;
                    self.next_job += 1;
                    let id = new.then(|| format!("job-{}", self.next_job));
                    self.entries.insert(0, YtdlEntry {
                        id: id.clone(),
                        pid: Some(self.next_pid),
                        url: add.url.clone(),
                        directory: add.directory.clone(),
                        output_codec: add.codec.clone(),
                        status: "downloading".into(),
                        start_time: Some(self.now),
                        file_name: new.then(|| name.clone()),
                        ..YtdlEntry::default()
                    });
                    if std::mem::take(&mut self.lose_add) {
                        return Answer::Add(Err(unanswered(NoAnswer::TimedOut)));
                    }
                    if std::mem::take(&mut self.garble_add) {
                        return Answer::Add(Ok(YtdlStarted::default()));
                    }
                    Answer::Add(Ok(YtdlStarted { message: Some("Download started".into()), id, file_name: new.then_some(name) }))
                }
                Call::Jobs => Answer::Jobs(Ok(if self.unlisted { Vec::new() } else { self.entries.clone() })),
                Call::Records { generation, limit, before } => Answer::Records(Ok(self.page(*generation, *limit, *before))),
                Call::Added { generation, before } => Answer::Added(Ok(self.read(*generation, *before))),
                Call::Listing { directory } => Answer::Listing(match self.folders.get(bare(directory)) {
                    Some(files) => Ok(YtdlListing { path: slashed(directory), directories: Vec::new(), files: files.clone() }),
                    None => Err(answered(404, "Folder not found")),
                }),
            }
        }

        /// One record page as `GET /discovery/downloads` answers it: the
        /// server's filter from generation 1, every plug-in's rows before.
        fn page(&self, generation: u32, limit: u32, before: Option<i64>) -> Option<YtdlRecordPage> {
            let all = self.records.as_ref()?;
            let limit = limit.clamp(1, 500);
            let mut rows: Vec<&YtdlRecord> = all
                .iter()
                .filter(|r| generation == 0 || r.plugin == "ytdl")
                .filter(|r| before.filter(|b| *b > 0).is_none_or(|b| r.id < b))
                .collect();
            rows.sort_by_key(|r| std::cmp::Reverse(r.id));
            rows.truncate(limit as usize);
            Some(YtdlRecordPage {
                count: rows.len(),
                limit,
                oldest_id: rows.iter().map(|r| r.id).min(),
                newest_id: rows.iter().map(|r| r.id).max(),
                oldest_at: rows.iter().filter_map(|r| r.downloaded_at).min(),
                rows: rows.into_iter().filter(|r| r.plugin == "ytdl").cloned().collect(),
            })
        }

        /// [`crate::api::Client::ytdl_added`]'s paging over [`Fake::page`].
        fn read(&self, generation: u32, before: Option<i64>) -> Option<YtdlAddedRead> {
            let mut read = YtdlAddedRead { cursor: before, ..YtdlAddedRead::default() };
            for page in 0..RECORD_PAGES {
                let found = self.page(generation, RECORD_PAGE, read.cursor)?;
                if page == 0 {
                    read.newest_id = found.newest_id;
                }
                read.cursor = found.oldest_id.or(read.cursor);
                let end = found.end();
                read.rows.extend(found.rows);
                if end {
                    read.end = true;
                    break;
                }
                if !read.rows.is_empty() {
                    break;
                }
            }
            Some(read)
        }

        /// The running job for `url` comes in: the file lands, its record is
        /// written (6.30+), and the new server's entry names both.
        fn complete(&mut self, url: &str) {
            let at = self.entries.iter().position(|e| e.url == url && e.downloading()).expect("a running job");
            let name = format!("{}.{}", stem(url), output_extension(&self.entries[at].output_codec));
            let folder = bare(&self.entries[at].directory).to_string();
            self.files(&folder).push(name.clone());
            let path = format!("{folder}/{name}");
            let mut download = None;
            if let Some(records) = &mut self.records {
                self.next_record += 1;
                records.push(record(self.next_record, "ytdl", &path, url, self.now));
                download = Some(self.next_record);
            }
            let new = self.generation >= 1;
            let e = &mut self.entries[at];
            e.status = "complete".into();
            e.end_time = Some(self.now);
            if new {
                e.file_path = Some(path);
                e.download_id = download;
                e.file_name = Some(name);
            }
        }

        /// The running job for `url` ends in error: with a reason and words
        /// on the new server, bare before it.
        fn fail_job(&mut self, url: &str, reason: Option<&str>, error: Option<&str>) {
            let (new, now) = (self.generation >= 1, self.now);
            let e = self.entries.iter_mut().find(|e| e.url == url && e.downloading()).expect("a running job");
            e.status = "error".into();
            e.end_time = Some(now);
            if new {
                e.reason = reason.map(str::to_string);
                e.error = Some(error.unwrap_or("the download failed").to_string());
            }
        }
    }

    /// A tracker over the scripted transport, and the server answering it.
    struct Rig {
        t: Tracker<Wire, Tock, Shelf>,
        fake: Fake,
    }

    impl Rig {
        fn new(fake: Fake) -> Rig {
            Rig::with_store(fake, Shelf::default())
        }

        fn with_store(fake: Fake, shelf: Shelf) -> Rig {
            Rig { t: Tracker::new(fake.generation, Wire::default(), Tock(T0), shelf), fake }
        }

        /// Tick, answering every call the server isn't holding, until
        /// nothing more goes out.
        fn serve(&mut self) {
            for _ in 0..200 {
                self.t.tick();
                self.fake.now = self.t.clock_mut().0 + SKEW;
                let out = std::mem::take(&mut self.t.transport_mut().out);
                let (held, go): (Vec<_>, Vec<_>) = out.into_iter().partition(|(_, c)| self.fake.hold.contains(kind(c)));
                self.t.transport_mut().out = held;
                if go.is_empty() {
                    return;
                }
                for (id, call) in go {
                    let answer = self.fake.answer(&call);
                    self.t.transport_mut().back.push((id, answer));
                }
            }
            panic!("the tracker never went quiet");
        }

        fn wait(&mut self, ms: i64) {
            self.t.clock_mut().0 += ms;
            self.serve();
        }

        /// The calls of a kind that are out, taken to answer by hand.
        fn take(&mut self, k: &str) -> Vec<(CallId, Call)> {
            let out = std::mem::take(&mut self.t.transport_mut().out);
            let (mine, rest): (Vec<_>, Vec<_>) = out.into_iter().partition(|(_, c)| kind(c) == k);
            self.t.transport_mut().out = rest;
            mine
        }

        fn reply(&mut self, id: CallId, answer: Answer) {
            self.t.transport_mut().back.push((id, answer));
        }

        /// How many calls of a kind went out.
        fn sent(&mut self, k: &str) -> usize {
            self.t.transport_mut().log.iter().filter(|c| kind(c) == k).count()
        }

        fn out(&mut self) -> Vec<&'static str> {
            self.t.transport_mut().out.iter().map(|(_, c)| kind(c)).collect()
        }

        fn add(&mut self, url: &str) -> String {
            self.t.start(request(url, DIR, Origin::Room))
        }

        fn job(&self, id: &str) -> &Job {
            self.t.job(id).expect("the job")
        }

        fn state(&self, id: &str) -> JobState {
            self.job(id).state
        }

        fn events(&mut self) -> Vec<JobEvent> {
            self.t.take_events()
        }

        fn saved(&mut self) -> Slot {
            Slot::read(self.t.store_mut().slot.as_ref().expect("a saved slot"))
        }

        /// Opened on a server below generation 1: its tier probed.
        fn probed(fake: Fake) -> Rig {
            let mut rig = Rig::new(fake);
            rig.t.opened();
            rig.serve();
            rig
        }
    }

    fn notes(events: &[JobEvent]) -> Vec<Note> {
        events
            .iter()
            .filter_map(|e| match e {
                JobEvent::Settled { note: Some(note), .. } => Some(note.clone()),
                _ => None,
            })
            .collect()
    }

    fn outcome(events: &[JobEvent]) -> Option<StartOutcome> {
        events.iter().find_map(|e| match e {
            JobEvent::Started { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
    }

    fn says(f: &Failure, tier: Tier) -> &'static str {
        failure_label(f, &Words { surface: Surface::List, server: "Home server", codec: "mp3", tier }).key
    }

    /// The server with `ytdl >= 1`: a job follows the POST's id, the entry
    /// says how it ended, and a refusal comes before anything downloads.
    mod new_server {
        use super::*;

        #[test]
        fn a_job_follows_the_posts_id_until_it_lands_and_is_noted_once() {
            let mut rig = Rig::new(Fake::new(1, true));
            let home = "/Music/Daft Punk/Homework/";
            let req = StartRequest {
                mkdir: Some(home.into()),
                mkdir_folder: Some("Homework".into()),
                ..request(LINK, home, Origin::Room)
            };
            let id = rig.t.start(req);
            assert_eq!(rig.state(&id), JobState::Starting);
            assert_eq!(rig.t.summary().word(), Some(Word::Adding));
            assert!(rig.t.is_adding(LINK, "Music/Daft Punk/Homework"));
            // The floor, the job list and the folder go out together; the
            // POST waits for all three.
            assert_eq!(rig.out(), ["records", "jobs", "mkdir"]);
            rig.serve();
            assert_eq!(rig.sent("add"), 1);
            let Some(Call::Add(add)) = rig.t.transport_mut().log.iter().find(|c| kind(c) == "add").cloned() else { panic!() };
            assert_eq!(add.directory, home);
            assert_eq!(
                add.tags,
                YtdlTags { title: "One More Time".into(), artist: "Daft Punk".into(), ..YtdlTags::default() },
                "trimmed, and a year that isn't four digits left out"
            );
            assert!(rig.fake.folders.contains_key("Music/Daft Punk/Homework"));
            let job = rig.job(&id);
            assert_eq!((job.state, job.server_id.as_deref(), job.claim), (JobState::Adding, Some("job-1"), Some(Claim::Post)));
            assert_eq!(job.record_floor, Some(0), "the record was empty before it");
            assert_eq!(job.prior, Some(BTreeSet::new()));
            assert_eq!(rig.events(), [JobEvent::Started { job: id.clone(), origin: Some(Origin::Room), outcome: StartOutcome::Took }]);

            rig.wait(3000);
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::Adding, "still downloading");
            rig.fake.complete(LINK);
            rig.wait(3000);
            let job = rig.job(&id);
            assert_eq!(job.state, JobState::Added);
            assert_eq!(job.file_path.as_deref(), Some("Music/Daft Punk/Homework/One_More_Time.mp3"));
            assert_eq!(job.record_id, Some(501));
            assert_eq!(
                rig.events(),
                [JobEvent::Settled {
                    job: id.clone(),
                    state: JobState::Added,
                    note: Some(Note::Added { title: "One More Time".into(), folder: "Music/Daft Punk/Homework".into() }),
                }]
            );

            // Nothing open: the list isn't read again, and nothing is told twice.
            let reads = rig.sent("jobs");
            rig.wait(3000);
            rig.wait(60_000);
            assert_eq!(rig.sent("jobs"), reads);
            assert!(rig.events().is_empty());
            // The Added list was read again: the song once, the record's row
            // carrying the job.
            let rows = rig.t.added();
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!((row.record_id, row.job.as_deref(), row.ext.as_deref()), (Some(501), Some(id.as_str()), Some("mp3")));
            assert_eq!(row.folder, "Music/Daft Punk/Homework");
            assert_eq!(rig.t.summary().word(), Some(Word::Added));
        }

        #[test]
        fn a_409_is_refused_where_add_was_pressed_and_leaves_no_row() {
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.files(DIR).push("One_More_Time.mp3".into());
            rig.t.set_showing(Origin::Room, true);
            let id = rig.add(LINK);
            rig.serve();
            let events = rig.events();
            let Some(StartOutcome::Refused(f)) = outcome(&events) else { panic!("{events:?}") };
            assert_eq!((f.problem, f.detail.as_deref()), (Problem::Exists, Some("One_More_Time.mp3")));
            assert!(notes(&events).is_empty(), "the room says it");
            assert!(rig.t.job(&id).is_none() && rig.t.jobs().is_empty(), "no row");
            assert_eq!(rig.t.summary().word(), None);

            // Pressed in the Files window, closed before the answer: a row,
            // and its note.
            let window = Origin::Folder(DIR.into());
            let id = rig.t.start(request(LINK, DIR, window.clone()));
            rig.t.set_showing(window, false);
            rig.serve();
            let job = rig.job(&id);
            assert_eq!(job.state, JobState::StartFailed);
            assert_eq!(job.ways(Some(&["mp3", "opus"])), [Way::ChangeFormat, Way::Dismiss]);
            assert_eq!(rig.t.latest_from(&Origin::Folder(DIR.into())).map(|j| j.id.as_str()), Some(id.as_str()));
            assert!(rig.t.latest_from(&Origin::Room).is_none(), "the room's refusal left no row");
            let events = rig.events();
            assert!(matches!(outcome(&events), Some(StartOutcome::Failed(f)) if f.problem == Problem::Exists));
            assert_eq!(notes(&events), [Note::CouldntAdd { title: "One More Time".into() }]);
            assert_eq!(rig.t.summary().word(), Some(Word::Failed));
            assert_eq!(rig.sent("jobs"), 2, "the list before each POST; nothing polled for jobs that never started");
        }

        #[test]
        fn a_failed_job_reads_the_servers_reason_or_its_words() {
            for (reason, error, problem, key) in [
                (Some("too-big"), "it's over the server's 500 MB limit", Problem::TooBig, "gui.ytdl.list.too_big"),
                (Some("unavailable"), "Video unavailable", Problem::Unavailable, "gui.ytdl.list.unavailable"),
                (Some("exists"), "a file with that name landed meanwhile", Problem::Exists, "gui.ytdl.list.landed"),
                (None, "Sign in to confirm you’re not a bot.", Problem::Other, "gui.ytdl.list.said"),
            ] {
                let mut rig = Rig::new(Fake::new(1, true));
                let id = rig.add(LINK);
                rig.serve();
                rig.events();
                rig.fake.fail_job(LINK, reason, Some(error));
                rig.wait(3000);
                let job = rig.job(&id);
                assert_eq!(job.state, JobState::Failed, "{reason:?}");
                let f = job.failure().expect("why");
                assert_eq!((f.problem, f.phase), (problem, Phase::Download), "{reason:?}");
                assert_eq!(says(&f, rig.t.tier()), key);
                assert_eq!(notes(&rig.events()), [Note::CouldntAdd { title: "One More Time".into() }]);
                assert_eq!(rig.sent("listing"), 0, "the entry says why: no folder is read");
            }
        }

        #[test]
        fn a_lost_answer_is_looked_for_by_link_and_folder_and_never_takes_an_earlier_add() {
            // The web app added this song to this folder an hour ago: the
            // server lists that entry for a day.
            let earlier = entry(Some("job-web"), 77, LINK, "complete", T0 + SKEW - 3_600_000);

            // The POST went and nothing came back: the server never took it.
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.entries.push(earlier.clone());
            rig.fake.fail("add", unanswered(NoAnswer::Dropped));
            let id = rig.add(LINK);
            rig.serve();
            assert_eq!(rig.state(&id), JobState::StartUnsure);
            assert_eq!(outcome(&rig.events()), Some(StartOutcome::Unsure));
            assert_eq!(rig.job(&id).prior, Some(["id:job-web".to_string()].into()));
            for _ in 0..40 {
                rig.wait(3000);
                assert_eq!(rig.job(&id).server_id, None, "never the earlier add's entry");
            }
            assert_eq!(rig.state(&id), JobState::StartUnsure, "two minutes, not yet past them");
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::NotSureStarted);
            assert_eq!(rig.job(&id).ways(None), [Way::CheckAgain, Way::Dismiss]);
            assert_eq!(rig.sent("add"), 1, "never sent twice");

            // The list before the POST couldn't be had: the clock's slack
            // stands in for it.
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.entries.push(earlier.clone());
            rig.fake.hold.insert("jobs");
            rig.fake.fail("add", answered(504, "Gateway Timeout"));
            let id = rig.add(LINK);
            rig.wait(5000);
            rig.take("jobs");
            rig.fake.hold.clear();
            rig.serve();
            assert_eq!((rig.state(&id), rig.job(&id).prior.clone()), (JobState::StartUnsure, None), "a proxy's 504");
            rig.wait(3000);
            assert_eq!(rig.job(&id).server_id, None);

            // The server took it and the answer was lost: its entry is
            // claimed, and followed to the end.
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.entries.push(earlier);
            rig.fake.lose_add = true;
            let id = rig.add(LINK);
            rig.serve();
            assert_eq!(rig.state(&id), JobState::StartUnsure);
            rig.wait(3000);
            let job = rig.job(&id);
            assert_eq!((job.state, job.server_id.as_deref(), job.claim), (JobState::Adding, Some("job-1"), Some(Claim::Match)));
            rig.fake.complete(LINK);
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::Added);
        }

        #[test]
        fn a_list_read_before_the_id_came_back_cant_say_its_entry_is_gone() {
            let mut rig = Rig::new(Fake::new(1, true));
            rig.add(OTHER);
            rig.serve();
            // A poll goes out…
            rig.fake.hold.insert("jobs");
            rig.wait(3000);
            let poll = rig.take("jobs");
            let then = rig.fake.entries.clone();
            rig.fake.hold.clear();
            // …the next add's 2xx names its job while it is out…
            let id = rig.add(LINK);
            rig.serve();
            assert_eq!(rig.job(&id).server_id.as_deref(), Some("job-2"));
            // …and the list, read before the server made that job, comes back.
            rig.reply(poll[0].0, Answer::Jobs(Ok(then)));
            rig.serve();
            assert_eq!(rig.state(&id), JobState::Adding, "not gone: not there yet");
            assert_eq!(rig.sent("records"), 2, "no record looked for; the floors only");
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::Adding);
            rig.fake.complete(LINK);
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::Added);
        }

        #[test]
        fn a_2xx_with_no_id_to_read_is_claimed_by_link_and_folder() {
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.entries.push(entry(Some("job-web"), 77, LINK, "complete", T0 + SKEW - 3_600_000));
            rig.fake.garble_add = true;
            let id = rig.add(LINK);
            rig.serve();
            let job = rig.job(&id);
            assert_eq!((job.state, job.server_id.as_deref(), job.claim), (JobState::Adding, Some("job-1"), Some(Claim::Match)));
            assert_eq!(outcome(&rig.events()), Some(StartOutcome::Took), "a 2xx started it, whatever its body");
        }

        #[test]
        fn one_that_never_connected_started_nothing_and_says_so() {
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.fail("add", unanswered(NoAnswer::Refused));
            let id = rig.add(LINK);
            rig.serve();
            let f = rig.job(&id).failure().expect("why");
            assert_eq!((rig.state(&id), f.problem), (JobState::StartFailed, Problem::Unreachable));
            assert_eq!(says(&f, Tier::New), "gui.ytdl.list.didnt_start");
            assert_eq!(rig.job(&id).ways(None), [Way::Retry, Way::Dismiss]);
            let next = rig.t.retry(&id, Redo::default()).expect("a new add");
            rig.serve();
            assert_eq!(rig.state(&next), JobState::Adding);
            assert_eq!(rig.t.jobs().len(), 1, "in the row's place");
        }

        #[test]
        fn check_again_finds_an_entry_that_turned_up_late() {
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.lose_add = true;
            rig.fake.unlisted = true;
            let id = rig.add(LINK);
            rig.serve();
            rig.wait(CLAIM_WINDOW_MS + 3000);
            assert_eq!(rig.state(&id), JobState::NotSureStarted);
            rig.fake.unlisted = false;
            assert!(rig.t.check_again(&id));
            assert!(rig.t.row_busy(&id));
            assert!(!rig.t.check_again(&id), "one at a time");
            rig.serve();
            assert!(!rig.t.row_busy(&id));
            let job = rig.job(&id);
            assert_eq!((job.state, job.server_id.as_deref(), job.ended_at), (JobState::Adding, Some("job-1"), None));
            rig.fake.complete(LINK);
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::Added);
        }

        #[test]
        fn an_entry_gone_with_a_restart_is_found_in_the_record_or_not_found() {
            let mut rig = Rig::new(Fake::new(1, true));
            let landed = rig.add(LINK);
            rig.serve();
            // It came in just as the server restarted, which forgets its list.
            rig.fake.complete(LINK);
            rig.fake.entries.clear();
            rig.wait(3000);
            let job = rig.job(&landed);
            assert_eq!((job.state, job.record_id), (JobState::Added, Some(501)));
            assert_eq!(job.file_path.as_deref(), Some("Music/Daft Punk/Discovery/One_More_Time.mp3"));

            // This one died with the restart.
            let died = rig.add(OTHER);
            rig.serve();
            rig.fake.entries.clear();
            rig.wait(3000);
            assert_eq!(rig.state(&died), JobState::NotFound);
            assert_eq!(rig.job(&died).ways(None), [Way::AddAgain, Way::Dismiss]);
            assert!(rig.t.add_again(&died));
            rig.serve();
            let job = rig.job(&died);
            assert_eq!((job.state, job.server_id.as_deref()), (JobState::Adding, Some("job-3")));
            assert_eq!(rig.sent("add"), 3);
            assert_eq!(rig.sent("mkdir"), 0);
        }

        #[test]
        fn the_folder_goes_first_once_and_a_refusal_there_starts_nothing() {
            let home = "/Music/Daft Punk/Homework/";
            let into_home = |origin| StartRequest {
                mkdir: Some(home.into()),
                mkdir_folder: Some("Homework".into()),
                ..request(LINK, home, origin)
            };

            // The account may not make folders after all (a stale noMkdir).
            let mut rig = Rig::new(Fake::new(1, true));
            rig.t.set_showing(Origin::Room, true);
            rig.fake.fail("mkdir", answered(403, "Create Folder Disabled"));
            rig.t.start(into_home(Origin::Room));
            rig.serve();
            assert!(rig.t.mkdir_refused());
            let events = rig.events();
            assert_eq!(events[0], JobEvent::Reping);
            let Some(StartOutcome::Refused(f)) = outcome(&events) else { panic!("{events:?}") };
            assert_eq!((f.problem, f.detail.as_deref()), (Problem::MkdirOff, Some("Homework")));
            assert_eq!((rig.sent("mkdir"), rig.sent("add")), (1, 0), "asked once, and nothing POSTed");

            // Any other failure there names the folder; nothing up, so a row,
            // whose Try again makes the folder again.
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.fail("mkdir", answered(500, "EACCES"));
            let id = rig.t.start(into_home(Origin::Room));
            rig.serve();
            let f = rig.job(&id).failure().expect("why");
            assert_eq!((f.problem, f.detail.as_deref()), (Problem::MkdirFailed, Some("Homework")));
            assert_eq!(rig.job(&id).ways(None), [Way::Retry, Way::ChooseFolder, Way::Dismiss]);
            let next = rig.t.retry(&id, Redo::default()).expect("a new add");
            rig.serve();
            assert_eq!((rig.sent("mkdir"), rig.state(&next)), (2, JobState::Adding));
            assert_eq!(rig.job(&next).mkdir.as_deref(), Some(home));

            // Choose a folder: into another one, made first there.
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.fail("mkdir", answered(500, "EACCES"));
            let id = rig.t.start(into_home(Origin::Room));
            rig.serve();
            let inbox = "/Music/Inbox/";
            let redo = Redo { directory: Some(inbox.into()), mkdir: Some(inbox.into()), mkdir_folder: Some("Inbox".into()), ..Redo::default() };
            let next = rig.t.retry(&id, redo).expect("a new add");
            rig.serve();
            let job = rig.job(&next);
            assert_eq!((job.state, job.directory.as_str(), job.mkdir.as_deref()), (JobState::Adding, inbox, Some(inbox)));

            // A 2xx this player couldn't read is the folder made: on it goes.
            let mut rig = Rig::new(Fake::new(1, true));
            rig.fake.fail("mkdir", YtdlCallError::Unreadable { status: 200 });
            let id = rig.t.start(into_home(Origin::Room));
            rig.serve();
            assert_eq!(rig.state(&id), JobState::Adding);
        }

        #[test]
        fn a_stale_flag_asks_for_a_ping_and_a_format_turned_off_is_forgotten() {
            for (e, want) in [
                (answered(403, "Uploading Disabled"), vec![JobEvent::Reping]),
                (answered(500, "FFmpeg is not available yet"), vec![JobEvent::Reping]),
                (answered(400, "\"outputCodec\" must be one of [mp3, flac]"), vec![JobEvent::FormatOff, JobEvent::Reping]),
                (answered(500, "disk full"), vec![]),
            ] {
                let mut rig = Rig::new(Fake::new(1, true));
                rig.t.set_showing(Origin::Room, true);
                rig.fake.fail("add", e.clone());
                rig.add(LINK);
                rig.serve();
                let events = rig.events();
                let flags: Vec<JobEvent> =
                    events.iter().filter(|e| matches!(e, JobEvent::Reping | JobEvent::FormatOff)).cloned().collect();
                assert_eq!(flags, want, "{e:?}");
                assert!(matches!(outcome(&events), Some(StartOutcome::Refused(_))));
                assert!(rig.t.jobs().is_empty());
            }
        }

        #[test]
        fn change_format_and_retry_replaces_the_row_and_its_entry_is_never_claimed_again() {
            let mut rig = Rig::new(Fake::new(1, true));
            let id = rig.add(LINK);
            rig.serve();
            rig.fake.fail_job(LINK, Some("exists"), Some("a file named One_More_Time.mp3 landed meanwhile"));
            rig.wait(3000);
            let f = rig.job(&id).failure().expect("why");
            assert_eq!((f.problem, f.detail.as_deref()), (Problem::Exists, Some("One_More_Time.mp3")));
            assert_eq!(rig.job(&id).ways(Some(&["mp3", "opus"])), [Way::ChangeFormat, Way::Dismiss]);
            assert_eq!(rig.job(&id).ways(Some(&["mp3"])), [Way::Dismiss], "no other format to change to");

            assert!(rig.t.retry(&rig.t.jobs()[0].id.clone(), Redo { codec: Some("opus".into()), ..Redo::default() }).is_some());
            assert!(rig.t.job(&id).is_none(), "the old row went");
            let next = rig.t.jobs()[0].clone();
            assert_eq!(rig.t.jobs().len(), 1);
            assert_eq!((next.codec.as_str(), next.file_name.as_str(), next.state), ("opus", "One_More_Time.opus", JobState::Starting));
            assert!(rig.t.row_busy(&next.id));
            assert_eq!(rig.t.retired, ["id:job-1"]);
            rig.serve();
            assert!(!rig.t.row_busy(&next.id));
            let Some(Call::Add(add)) = rig.t.transport_mut().log.iter().rev().find(|c| kind(c) == "add").cloned() else { panic!() };
            assert_eq!(add.codec, "opus");
            assert_eq!(rig.job(&next.id).server_id.as_deref(), Some("job-2"));
            assert!(rig.t.retry(&next.id, Redo::default()).is_none(), "an open job isn't retried");
        }

        #[test]
        fn the_list_is_read_every_3_s_only_while_a_job_is_open_and_less_often_unanswered() {
            let mut rig = Rig::new(Fake::new(1, true));
            rig.wait(3000);
            rig.wait(60_000);
            assert_eq!(rig.sent("jobs"), 0, "nothing open: never asked");
            rig.add(LINK);
            rig.serve();
            let n = rig.sent("jobs");
            assert_eq!(n, 2, "the list before the POST, and once at the 2xx");
            rig.wait(2_999);
            assert_eq!(rig.sent("jobs"), n);
            rig.wait(1);
            assert_eq!(rig.sent("jobs"), n + 1);

            // No answer: rows wait for the connection, and the beat backs off.
            rig.fake.fail("jobs", unanswered(NoAnswer::Refused));
            rig.fake.fail("jobs", unanswered(NoAnswer::TimedOut));
            rig.wait(3000);
            assert_eq!(rig.sent("jobs"), n + 2);
            assert!(rig.t.waiting());
            rig.wait(5_999);
            assert_eq!(rig.sent("jobs"), n + 2, "6 s after one failure");
            rig.wait(1);
            assert_eq!(rig.sent("jobs"), n + 3);
            rig.wait(11_999);
            assert_eq!(rig.sent("jobs"), n + 3, "12 s after two");
            rig.wait(1);
            assert_eq!(rig.sent("jobs"), n + 4);
            assert!(!rig.t.waiting(), "answered again");
            rig.wait(3000);
            assert_eq!(rig.sent("jobs"), n + 5);
            // A refusal (not "no answer") backs off too, without "waiting".
            rig.fake.fail("jobs", answered(500, "Server Error"));
            rig.wait(3000);
            assert!(!rig.t.waiting());
            assert_eq!(rig.sent("jobs"), n + 6);
            rig.wait(6000);
            assert_eq!(rig.sent("jobs"), n + 7);

            // A clock set back an hour doesn't hold the beat for an hour.
            rig.t.clock_mut().0 -= 3_600_000;
            rig.serve();
            assert_eq!(rig.sent("jobs"), n + 8);

            rig.fake.complete(LINK);
            rig.wait(3000);
            let settled = rig.sent("jobs");
            rig.wait(30_000);
            assert_eq!(rig.sent("jobs"), settled, "settled: the beat stops");
        }
    }

    /// Servers without `ytdl`: 6.30–6.33 (the record) and before (none).
    /// The phone's rules — claim by link and folder, follow by pid, ask the
    /// record and the folder.
    mod older_server {
        use super::*;

        #[test]
        fn the_tier_is_the_records_probe_and_unknown_reads_older() {
            let rig = Rig::probed(Fake::new(0, true));
            assert_eq!((rig.t.tier(), rig.t.tier_known()), (Tier::Full, true));
            let rig = Rig::probed(Fake::new(0, false));
            assert_eq!((rig.t.tier(), rig.t.tier_known()), (Tier::Older, true));

            // No answer: the older rules, asked again only while a surface
            // shows.
            let mut fake = Fake::new(0, true);
            fake.fail("added", unanswered(NoAnswer::TimedOut));
            let mut rig = Rig::probed(fake);
            assert_eq!((rig.t.tier(), rig.t.tier_known()), (Tier::Older, false));
            rig.wait(10_000);
            assert_eq!(rig.sent("added"), 1, "nothing shows: not asked again");
            rig.t.set_showing(Origin::Room, true);
            rig.serve();
            assert_eq!((rig.sent("added"), rig.t.tier()), (2, Tier::Full));
        }

        #[test]
        fn a_full_servers_job_is_claimed_by_link_and_folder_and_settled_by_its_record() {
            let mut fake = Fake::new(0, true);
            // Discover's downloads share the record: the floor is any
            // plug-in's newest, and their rows are no songs of this list.
            fake.records.as_mut().unwrap().push(record(40, "discover-copy", "Music/Peers/a.mp3", "peer:1", T0 - 10));
            let mut rig = Rig::probed(fake);
            let id = rig.add(LINK);
            assert_eq!(rig.out(), ["records", "jobs"], "no folder to make");
            rig.serve();
            let job = rig.job(&id);
            assert_eq!((job.state, job.record_floor, job.server_id.as_deref()), (JobState::Adding, Some(40), None));
            assert_eq!(job.pid, Some(4101), "claimed on the first read after the 2xx");
            assert_eq!(outcome(&rig.events()), Some(StartOutcome::Took));

            rig.fake.complete(LINK);
            rig.wait(3000);
            let job = rig.job(&id);
            assert_eq!((job.state, job.record_id), (JobState::Added, Some(501)));
            assert_eq!(job.file_path.as_deref(), Some("Music/Daft Punk/Discovery/One_More_Time.mp3"));
            assert_eq!(job.track_id, Some(5010));
            assert_eq!(notes(&rig.events()).len(), 1);
            let rows = rig.t.added();
            assert_eq!(rows.len(), 1, "the record's row, with the job");
            assert_eq!(rows[0].job.as_deref(), Some(id.as_str()));
        }

        #[test]
        fn an_entry_there_before_the_post_is_never_claimed_for_it() {
            // The web app added this song to this folder 20 s ago; the
            // tracker still lists its entry.
            let earlier = entry(None, 90, LINK, "complete", T0 + SKEW - 20_000);

            // The POST went and nothing came back; the server never took it.
            let mut rig = Rig::probed(Fake::new(0, true));
            rig.fake.entries.push(earlier.clone());
            rig.fake.fail("add", unanswered(NoAnswer::TimedOut));
            let id = rig.add(LINK);
            rig.serve();
            assert_eq!(rig.state(&id), JobState::StartUnsure);
            assert_eq!(rig.job(&id).prior, Some([pid_key(90, Some(T0 + SKEW - 20_000))].into()));
            for _ in 0..41 {
                rig.wait(3000);
                assert_eq!(rig.job(&id).pid, None, "never the earlier add's entry");
            }
            assert_eq!(rig.state(&id), JobState::NotSureStarted);

            // The server took it and the answer was lost: its own entry.
            let mut rig = Rig::probed(Fake::new(0, true));
            rig.fake.entries.push(earlier);
            rig.fake.lose_add = true;
            let id = rig.add(LINK);
            rig.serve();
            rig.wait(3000);
            assert_eq!((rig.state(&id), rig.job(&id).pid), (JobState::Adding, Some(4101)));
        }

        #[test]
        fn a_list_read_before_the_post_went_out_never_claims_for_it() {
            // Before 6.30: no list is read before the POST, so only the send
            // order keeps an earlier add's entry from being taken.
            let mut rig = Rig::probed(Fake::new(0, false));
            let other = rig.add(OTHER);
            rig.serve();
            assert_eq!(rig.job(&other).pid, Some(4101));
            rig.fake.entries.push(entry(None, 90, LINK, "complete", T0 + SKEW - 10_000));
            // A poll goes out…
            rig.fake.hold.insert("jobs");
            rig.wait(3000);
            let poll = rig.take("jobs");
            assert_eq!(poll.len(), 1);
            let then = rig.fake.entries.clone();
            rig.fake.hold.clear();
            // …Add is pressed while it is out, and the server takes it…
            let id = rig.add(LINK);
            rig.serve();
            assert_eq!((rig.state(&id), rig.job(&id).pid), (JobState::Adding, None));
            // …and the list, as the server read it before the POST, comes back.
            rig.reply(poll[0].0, Answer::Jobs(Ok(then)));
            rig.serve();
            assert_eq!(rig.job(&id).pid, None, "only an earlier add's entry is in it");
            rig.wait(3000);
            assert_eq!(rig.job(&id).pid, Some(4102), "the next read's, its own");
        }

        #[test]
        fn a_failure_is_told_by_the_folder_on_a_full_server_and_hedged_before_it() {
            let mut rig = Rig::probed(Fake::new(0, true));
            let id = rig.add(LINK);
            rig.serve();
            // 6.30+ refuses a name clash after the download: the predicted name
            // in the folder after an error is one.
            rig.fake.files(DIR).push("one_more_time.MP3".into());
            rig.fake.fail_job(LINK, None, None);
            rig.wait(3000);
            let f = rig.job(&id).failure().expect("why");
            assert_eq!((rig.state(&id), f.problem, f.phase), (JobState::Failed, Problem::Exists, Phase::Download));
            assert_eq!(says(&f, Tier::Full), "gui.ytdl.list.landed_unnamed");
            assert_eq!(rig.job(&id).ways(None), [Way::ChangeFormat, Way::Dismiss]);
            // Without the name, a failure the server didn't explain: the hedge.
            let id = rig.add(OTHER);
            rig.serve();
            rig.fake.fail_job(OTHER, None, None);
            rig.wait(3000);
            let f = rig.job(&id).failure().expect("why");
            assert_eq!((f.problem, says(&f, Tier::Full)), (Problem::Unknown, "gui.ytdl.list.hedged"));
            assert_eq!(rig.job(&id).ways(None), [Way::Retry, Way::Dismiss]);

            // A folder that won't list leaves it adding, asked again each
            // poll; once the entry is gone it fails on what is known.
            let id = rig.add(THIRD);
            rig.serve();
            rig.fake.fail_job(THIRD, None, None);
            rig.fake.fail("listing", answered(500, "Server Error"));
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::Adding);
            rig.fake.entries.retain(|e| e.url != THIRD);
            rig.fake.fail("listing", answered(500, "Server Error"));
            rig.wait(3000);
            assert_eq!(rig.state(&id), JobState::Failed);

            // Before 6.30 a same-name file is re-tagged, never refused: failed
            // at once, with the older hedge, and no folder read.
            let mut rig = Rig::probed(Fake::new(0, false));
            let id = rig.add(LINK);
            rig.serve();
            rig.fake.fail_job(LINK, None, None);
            rig.wait(3000);
            let f = rig.job(&id).failure().expect("why");
            assert_eq!((rig.state(&id), says(&f, Tier::Older)), (JobState::Failed, "gui.ytdl.list.hedged_older"));
            assert_eq!(rig.sent("listing"), 0);
            // …and one that came in is named by the folder.
            let id = rig.add(OTHER);
            rig.serve();
            rig.fake.complete(OTHER);
            rig.wait(3000);
            let job = rig.job(&id);
            assert_eq!((job.state, job.file_path.as_deref()), (JobState::Added, Some("Music/Daft Punk/Discovery/Aerodynamic.mp3")));
            assert_eq!(rig.sent("records"), 0, "no record to ask");
        }

        #[test]
        fn an_entry_gone_unseen_settles_by_the_record_or_the_folder() {
            // 6.30+: away past the tracker's 30 s; the record says it came in.
            let mut rig = Rig::probed(Fake::new(0, true));
            let landed = rig.add(LINK);
            rig.serve();
            rig.fake.complete(LINK);
            rig.fake.entries.clear();
            rig.wait(3000);
            assert_eq!((rig.state(&landed), rig.job(&landed).record_id), (JobState::Added, Some(501)));
            // Gone with no record: not sure it finished, then Check again.
            let lost = rig.add(OTHER);
            rig.serve();
            rig.fake.entries.clear();
            rig.wait(3000);
            assert_eq!(rig.state(&lost), JobState::NotSureFinished);
            assert_eq!(rig.job(&lost).ways(None), [Way::CheckAgain, Way::Dismiss]);
            assert!(rig.t.check_again(&lost));
            rig.serve();
            assert_eq!(rig.state(&lost), JobState::NotFound, "no entry, no record, not in the folder");
            // A record that couldn't be read can't say "not found".
            let mut rig = Rig::probed(Fake::new(0, true));
            let lost = rig.add(OTHER);
            rig.serve();
            rig.fake.entries.clear();
            rig.wait(3000);
            rig.fake.fail("records", answered(500, "Server Error"));
            assert!(rig.t.check_again(&lost));
            rig.serve();
            assert_eq!(rig.state(&lost), JobState::NotSureFinished);

            // Before 6.30 the folder alone.
            let mut rig = Rig::probed(Fake::new(0, false));
            let landed = rig.add(LINK);
            rig.serve();
            rig.fake.complete(LINK);
            rig.fake.entries.clear();
            rig.wait(3000);
            let job = rig.job(&landed);
            assert_eq!((job.state, job.file_path.as_deref()), (JobState::Added, Some("Music/Daft Punk/Discovery/One_More_Time.mp3")));
            let lost = rig.add(OTHER);
            rig.serve();
            rig.fake.entries.clear();
            rig.wait(3000);
            assert_eq!(rig.state(&lost), JobState::NotSureFinished);
        }

        #[test]
        fn the_reads_before_the_post_are_waited_for_5_s_at_most() {
            let mut fake = Fake::new(0, true);
            fake.records.as_mut().unwrap().push(record(40, "ytdl", "Music/Old/a.mp3", OTHER, T0 - 10));
            let mut rig = Rig::probed(fake);
            rig.fake.hold.extend(["records", "jobs"]);
            let id = rig.add(LINK);
            rig.serve();
            rig.wait(4_999);
            assert_eq!(rig.sent("add"), 0);
            rig.wait(1);
            assert_eq!(rig.sent("add"), 1);
            let job = rig.job(&id);
            assert_eq!((job.record_floor, job.prior.clone()), (Some(40), None), "the last id read, and no list");
            // Their answers, late, change nothing.
            rig.fake.hold.clear();
            rig.serve();
            assert_eq!(rig.job(&id).prior, None);
            assert_eq!(rig.state(&id), JobState::Adding);
        }

        #[test]
        fn the_added_list_pages_back_and_a_refresh_keeps_what_show_earlier_brought() {
            let mut fake = Fake::new(0, true);
            let records = fake.records.as_mut().unwrap();
            for i in 1..=25 {
                records.push(record(i, "ytdl", &format!("Music/Old/s{i}.mp3"), OTHER, T0 - 100_000 + i));
            }
            // A Discover copy among them: a page counts it, the list leaves it out.
            records.push(record(30, "discover-copy", "Music/Peers/p.mp3", "peer:1", T0 - 50_000));
            let mut rig = Rig::probed(fake);
            assert_eq!(rig.t.added().len(), 19, "the probe's page: twenty rows, one of them Discover's");
            assert!(rig.t.can_show_earlier());
            assert!(rig.t.show_earlier());
            assert!(rig.t.added_loading());
            assert!(!rig.t.show_earlier(), "one read at a time");
            rig.serve();
            assert_eq!(rig.t.added().len(), 25);
            assert!(!rig.t.can_show_earlier() && !rig.t.show_earlier(), "a short page is the end");

            // A song lands while the record can't be read: the folder names
            // it, and the refresh after it gives the job its record.
            let id = rig.add(LINK);
            rig.serve();
            rig.fake.complete(LINK);
            rig.fake.fail("records", answered(500, "Server Error"));
            rig.wait(3000);
            let job = rig.job(&id);
            assert_eq!((job.state, job.record_id, job.track_id), (JobState::Added, Some(501), Some(5010)));
            let rows = rig.t.added();
            assert_eq!(rows.len(), 26, "every record once, the earlier pages kept below the new top");
            assert_eq!(rows[0].job.as_deref(), Some(id.as_str()));
            assert_eq!(rows.iter().filter(|r| r.job.is_some()).count(), 1);
            let ids: BTreeSet<Option<i64>> = rows.iter().map(|r| r.record_id).collect();
            assert_eq!(ids.len(), 26);
        }

        #[test]
        fn an_unknown_tiers_add_waits_on_the_probe_for_its_floor() {
            let mut rig = Rig::new(Fake::new(0, true));
            rig.fake.records.as_mut().unwrap().push(record(12, "ytdl", "Music/Old/a.mp3", OTHER, T0 - 10));
            let id = rig.add(LINK);
            assert_eq!(rig.out(), ["added", "jobs"], "the probe stands in for the floor");
            rig.serve();
            assert_eq!(rig.t.tier(), Tier::Full);
            assert_eq!(rig.job(&id).record_floor, Some(12));
            assert_eq!(rig.state(&id), JobState::Adding);
            // The probe's page is the Added list: its row and none twice.
            assert_eq!(rig.t.added().len(), 1);
        }
    }

    // ── shared by both ──────────────────────────────────────────────────────

    #[test]
    fn jobs_are_kept_per_slot_and_one_mid_post_comes_back_looked_for() {
        let mut rig = Rig::new(Fake::new(1, true));
        rig.fake.hold.insert("add");
        let id = rig.add(LINK);
        rig.serve();
        // Saved before the POST went out.
        let slot = rig.t.store_mut().slot.clone().expect("saved");
        assert_eq!(slot["jobs"][0]["state"], "starting");
        assert!(slot["jobs"][0]["sentAt"].is_i64());
        // The player closes with it out; the server takes it all the same.
        let add = rig.take("add");
        let _ = rig.fake.answer(&add[0].1);
        let shelf = rig.t.store_mut().clone();
        let mut fake = std::mem::replace(&mut rig.fake, Fake::new(1, true));
        fake.hold.clear();

        let mut rig = Rig::with_store(fake, shelf);
        assert_eq!(rig.state(&id), JobState::StartUnsure);
        assert_eq!(rig.t.summary().word(), Some(Word::Adding));
        rig.serve();
        let job = rig.job(&id);
        assert_eq!((job.state, job.server_id.as_deref()), (JobState::Adding, Some("job-1")), "looked for on the first tick");
        // Every field comes back as it went.
        rig.fake.fail_job(LINK, Some("too-long"), Some("it took longer than an hour"));
        rig.wait(3000);
        assert_eq!(rig.saved().jobs, rig.t.jobs());
        assert!(rig.t.jobs()[0].failure.is_some());
    }

    #[test]
    fn a_slot_this_build_cant_read_is_ignored_and_a_state_it_doesnt_know_reads_not_sure() {
        let at = T0 - 5;
        let shelf = Shelf {
            slot: Some(json!({
                "jobs": [
                    { "id": "a", "url": LINK, "directory": DIR, "state": "paused", "pid": 3, "createdAt": at },
                    { "id": "b", "url": LINK, "directory": DIR, "state": "queued", "createdAt": at - 1 },
                    { "id": "", "url": LINK, "directory": DIR },
                    { "url": LINK, "directory": DIR },
                    "junk",
                    { "id": "c", "url": OTHER, "directory": DIR, "state": "added", "createdAt": at + 1, "failure": { "problem": "nope" } },
                    { "id": "c", "url": THIRD, "directory": DIR, "state": "added", "createdAt": at + 1 },
                ],
                "retired": ["id:x", 4],
            })),
        };
        let rig = Rig::with_store(Fake::new(1, true), shelf);
        let states: Vec<(&str, JobState)> = rig.t.jobs().iter().map(|j| (j.id.as_str(), j.state)).collect();
        assert_eq!(states, [("c", JobState::Added), ("a", JobState::NotSureFinished), ("b", JobState::NotSureStarted)]);
        assert_eq!((rig.job("c").url.as_str(), rig.job("c").failure.clone()), (OTHER, None));
        assert_eq!(rig.t.retired, ["id:x"]);
        for junk in [json!([1, 2]), json!("x"), json!({ "jobs": "x" }), json!(null)] {
            let rig = Rig::with_store(Fake::new(1, true), Shelf { slot: Some(junk) });
            assert!(rig.t.jobs().is_empty());
        }
    }

    #[test]
    fn another_players_jobs_in_the_slot_survive_this_ones_saves_and_a_dismissed_one_stays_gone() {
        let mut rig = Rig::new(Fake::new(1, true));
        let mine = rig.add(LINK);
        rig.serve();
        rig.fake.complete(LINK);
        rig.wait(3000);
        // A second player on this server and account writes its own add.
        let mut slot = rig.t.store_mut().slot.clone().expect("saved");
        let theirs = json!({ "id": "theirs", "url": OTHER, "directory": DIR, "state": "adding", "serverId": "job-9", "createdAt": T0 + 60_000 });
        slot["jobs"].as_array_mut().unwrap().push(theirs);
        slot["retired"] = json!(["id:job-8"]);
        rig.t.store_mut().slot = Some(slot);
        rig.t.mark_seen();
        let saved = rig.saved();
        let ids: Vec<&str> = saved.jobs.iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["theirs", mine.as_str()]);
        assert_eq!(saved.retired, ["id:job-8"]);
        assert!(rig.t.job("theirs").is_none(), "it joins this one on the next load");
        // Dismissed here: not written back from the other's copy.
        rig.t.dismiss(&mine);
        let saved = rig.saved();
        let ids: Vec<&str> = saved.jobs.iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["theirs"]);
        assert_eq!(saved.retired, ["id:job-8", "id:job-1"]);
        let shelf = rig.t.store_mut().clone();
        let rig = Rig::with_store(Fake::new(1, true), shelf);
        assert!(rig.t.job("theirs").is_some());
    }

    /// One slot two players hold at once (two GUIs on one account); the
    /// file can be made unreadable, then unwritable.
    #[derive(Clone, Default)]
    struct Shared(std::rc::Rc<std::cell::RefCell<Option<Value>>>, std::rc::Rc<std::cell::Cell<(bool, bool)>>);

    impl Store for Shared {
        fn load(&mut self) -> Result<Option<Value>, String> {
            if self.1.get().0 {
                return Err("could not read ytdl.json: Permission denied".into());
            }
            Ok(self.0.borrow().clone())
        }

        fn save(&mut self, slot: &Value) -> Result<(), String> {
            if self.1.get().1 {
                return Err("could not write ytdl.json: No space left on device".into());
            }
            *self.0.borrow_mut() = Some(slot.clone());
            Ok(())
        }
    }

    impl Shared {
        fn ids(&self) -> Vec<String> {
            Slot::read(self.0.borrow().as_ref().expect("a saved slot")).jobs.into_iter().map(|j| j.id).collect()
        }
    }

    /// [`Rig::serve`] for a tracker on a [`Shared`] slot.
    fn serve_shared(t: &mut Tracker<Wire, Tock, Shared>, fake: &mut Fake) {
        for _ in 0..200 {
            t.tick();
            fake.now = t.clock_mut().0 + SKEW;
            let out = std::mem::take(&mut t.transport_mut().out);
            if out.is_empty() {
                return;
            }
            for (id, call) in out {
                let answer = fake.answer(&call);
                t.transport_mut().back.push((id, answer));
            }
        }
        panic!("the tracker never went quiet");
    }

    #[test]
    fn a_row_another_player_let_go_goes_here_too_and_is_never_written_back() {
        let shared = Shared::default();
        let mut fake = Fake::new(1, true);
        let mut first = Tracker::new(1, Wire::default(), Tock(T0), shared.clone());
        let done = first.start(request(LINK, DIR, Origin::Room));
        serve_shared(&mut first, &mut fake);
        fake.complete(LINK);
        first.clock_mut().0 += 3000;
        serve_shared(&mut first, &mut fake);
        assert_eq!(first.job(&done).map(|j| j.state), Some(JobState::Added));
        // A second player on this server and account opens, and loads it.
        let mut second = Tracker::new(1, Wire::default(), Tock(T0 + 3000), shared.clone());
        assert!(second.job(&done).is_some());

        // The first dismisses it while the second adds a song of its own.
        first.dismiss(&done);
        let open = second.start(request(OTHER, DIR, Origin::Room));
        serve_shared(&mut second, &mut fake);
        assert_eq!(shared.ids(), std::slice::from_ref(&open), "the dismissed row is not written back");
        assert!(second.job(&done).is_none(), "and leaves the second player's list too");
        assert!(second.held().contains("id:job-1"), "its entry never claimed again there either");
        let third = Tracker::new(1, Wire::default(), Tock(T0 + 6000), shared.clone());
        assert!(third.job(&done).is_none() && third.job(&open).is_some());

        // An open row stays whatever the file says: the file deleted by hand.
        *shared.0.borrow_mut() = None;
        second.dismiss(&open);
        assert!(second.job(&open).is_some_and(|j| j.hidden));
        assert_eq!(shared.ids(), [open]);
    }

    #[test]
    fn a_row_lost_to_a_race_while_open_is_written_again_when_it_lands() {
        let shared = Shared::default();
        let mut fake = Fake::new(1, true);
        let mut t = Tracker::new(1, Wire::default(), Tock(T0), shared.clone());
        let mine = t.start(request(LINK, DIR, Origin::Room));
        serve_shared(&mut t, &mut fake);
        // Another player's save that read the file just before this add's
        // went without it.
        *shared.0.borrow_mut() = Some(json!({ "jobs": [], "retired": [] }));
        fake.complete(LINK);
        t.clock_mut().0 += 3000;
        serve_shared(&mut t, &mut fake);
        assert_eq!(t.job(&mine).map(|j| j.state), Some(JobState::Added), "never taken for let go elsewhere");
        assert_eq!(shared.ids(), [mine]);
    }

    #[test]
    fn a_file_that_cant_be_read_is_never_saved_over_as_an_empty_one() {
        let shared = Shared::default();
        let mut fake = Fake::new(1, true);
        let mut t = Tracker::new(1, Wire::default(), Tock(T0), shared.clone());
        let mine = t.start(request(LINK, DIR, Origin::Room));
        serve_shared(&mut t, &mut fake);
        // Another player on this account writes its own add beside it.
        let theirs = json!({ "id": "theirs", "url": OTHER, "directory": DIR, "state": "adding", "serverId": "job-9", "createdAt": T0 + 1 });
        shared.0.borrow_mut().as_mut().unwrap()["jobs"].as_array_mut().unwrap().push(theirs);

        // The file can't be read as this one's job lands: nothing is written.
        shared.1.set((true, false));
        fake.complete(LINK);
        t.clock_mut().0 += 3000;
        serve_shared(&mut t, &mut fake);
        assert_eq!(t.job(&mine).map(|j| j.state), Some(JobState::Added));
        assert_eq!(shared.ids(), ["theirs".to_string(), mine.clone()], "the other player's add is still there");
        // Readable again: the save waits its beat, then goes, merged.
        shared.1.set((false, false));
        t.clock_mut().0 += 1000;
        t.tick();
        assert_eq!(Slot::read(shared.0.borrow().as_ref().unwrap()).jobs[1].state, JobState::Adding, "not yet");
        t.clock_mut().0 += 2000;
        t.tick();
        let saved = Slot::read(shared.0.borrow().as_ref().unwrap());
        assert_eq!(saved.jobs.iter().map(|j| (j.id.as_str(), j.state)).collect::<Vec<_>>(), [("theirs", JobState::Adding), (mine.as_str(), JobState::Added)]);

        // A write that fails is no write: the dismissal goes once one lands,
        // and nothing this player holds is taken for let go meanwhile.
        shared.1.set((false, true));
        t.dismiss(&mine);
        assert_eq!(shared.ids().len(), 2);
        shared.1.set((false, false));
        t.clock_mut().0 += 3000;
        t.tick();
        assert_eq!(shared.ids(), ["theirs"]);
    }

    #[test]
    fn twenty_jobs_are_kept_open_ones_always_and_a_failure_for_seven_days() {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        let mut jobs: Vec<Value> = (0..22)
            .map(|i| {
                let at = T0 - 1000 * i;
                json!({ "id": format!("j{i}"), "url": LINK, "directory": DIR, "state": "added", "createdAt": at, "endedAt": at, "serverId": format!("job-{i}") })
            })
            .collect();
        jobs.push(json!({ "id": "open", "url": OTHER, "directory": DIR, "state": "adding", "createdAt": T0 - 100_000, "serverId": "job-open" }));
        jobs.push(json!({ "id": "stale", "url": OTHER, "directory": DIR, "state": "failed", "createdAt": T0 - 8 * DAY, "endedAt": T0 - 8 * DAY }));
        let rig = Rig::with_store(Fake::new(1, true), Shelf { slot: Some(json!({ "jobs": jobs })) });
        let ids: Vec<&str> = rig.t.jobs().iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids.len(), KEEP_JOBS);
        assert!(ids.contains(&"open"), "open jobs always stay");
        assert!(!ids.contains(&"stale") && !ids.contains(&"j19"));
        assert_eq!(rig.t.retired, ["id:job-21", "id:job-20", "id:job-19"], "the oldest settled ones, never claimed again");

        // A failure shows for seven days, then goes.
        let failed = json!({ "id": "f", "url": OTHER, "directory": DIR, "state": "failed", "createdAt": T0 - 7 * DAY + 5000, "endedAt": T0 - 7 * DAY + 5000 });
        let mut rig = Rig::with_store(Fake::new(1, true), Shelf { slot: Some(json!({ "jobs": [failed] })) });
        assert_eq!(rig.t.needs_look().len(), 1);
        assert_eq!(rig.t.summary().word(), Some(Word::Failed));
        rig.wait(5001);
        assert!(rig.t.needs_look().is_empty());
        assert_eq!(rig.t.summary().failed, 0);
        rig.add(LINK);
        rig.serve();
        assert!(rig.t.job("f").is_none(), "pruned with the next save");
    }

    #[test]
    fn the_word_is_adding_then_an_unseen_failure_then_an_hours_news_then_history() {
        let mut rig = Rig::new(Fake::new(1, true));
        assert_eq!(rig.t.summary().word(), None, "nothing at all: no word");
        rig.add(LINK);
        rig.add(OTHER);
        rig.serve();
        assert_eq!(rig.t.summary(), Summary { adding: 2, history: true, ..Summary::default() });
        rig.fake.complete(LINK);
        rig.fake.fail_job(OTHER, Some("too-long"), Some("it took longer than an hour"));
        rig.wait(3000);
        let ended = rig.t.clock_mut().0;
        let s = rig.t.summary();
        assert_eq!((s.adding, s.failed, s.added, s.news_ends), (0, 1, 1, Some(ended + ADDED_NEWS_MS)));
        assert_eq!(s.word(), Some(Word::Failed));
        // The list shows them: seen, and the word is the list's own.
        rig.t.watch(true);
        assert_eq!(rig.t.summary().word(), Some(Word::History));
        // While it is up, what lands is seen at once.
        let third = rig.add(THIRD);
        rig.serve();
        rig.fake.complete(THIRD);
        rig.wait(3000);
        assert!(rig.job(&third).seen);
        rig.t.watch(false);
        // Unseen, "✓ added" lasts an hour.
        let home = "/Music/Daft Punk/Homework/";
        rig.t.start(StartRequest { mkdir: Some(home.into()), ..request(LINK, home, Origin::Room) });
        rig.serve();
        rig.fake.complete(LINK);
        rig.wait(3000);
        assert_eq!(rig.t.summary().word(), Some(Word::Added));
        rig.wait(ADDED_NEWS_MS);
        assert_eq!(rig.t.summary().word(), Some(Word::Added));
        rig.wait(1);
        assert_eq!(rig.t.summary().word(), Some(Word::History));

        // Hidden, an open job counts for nothing — and is still adding there.
        let inbox = "/Music/Inbox/";
        let open = rig.t.start(StartRequest { mkdir: Some(inbox.into()), ..request(THIRD, inbox, Origin::Room) });
        rig.serve();
        assert_eq!(rig.t.summary().word(), Some(Word::Adding));
        assert_eq!(rig.t.adding().len(), 1);
        rig.t.dismiss(&open);
        assert!(rig.job(&open).hidden, "an open job is hidden, not dropped");
        assert_eq!(rig.t.summary().word(), Some(Word::History));
        assert!(rig.t.is_adding(THIRD, "Music/Inbox"));
        assert!(!rig.t.is_adding(THIRD, DIR), "by link and folder");
        assert!(rig.t.adding().is_empty());
    }

    #[test]
    fn every_api_error_reads_as_the_rules_failure() {
        let rows = [
            (unanswered(NoAnswer::TimedOut), CallFailure::TimedOut),
            (unanswered(NoAnswer::Refused), CallFailure::NeverConnected),
            (unanswered(NoAnswer::Offline), CallFailure::NeverConnected),
            (unanswered(NoAnswer::Dropped), CallFailure::Dropped),
            (YtdlCallError::Unauthorized, CallFailure::Answered { status: 401, words: None, reason: None, file_name: None }),
            (YtdlCallError::Unreadable { status: 200 }, CallFailure::Unreadable),
            (YtdlCallError::Config("a peer".into()), CallFailure::NotSent),
        ];
        for (e, want) in rows {
            assert_eq!(call_failure(&e), want, "{e:?}");
        }
        let e = YtdlCallError::Answered { status: 409, words: Some("x".into()), reason: Some("exists".into()), file_name: Some("a.mp3".into()) };
        assert_eq!(
            call_failure(&e),
            CallFailure::Answered { status: 409, words: Some("x".into()), reason: Some("exists".into()), file_name: Some("a.mp3".into()) }
        );
    }

    /// Each call made there and then on the real client, so a canned
    /// server answers them in the order they go out.
    struct Direct {
        client: crate::api::Client,
        back: Vec<(CallId, Answer)>,
    }

    impl Transport for Direct {
        fn send(&mut self, id: CallId, call: Call) {
            let answer = call.run(&self.client);
            self.back.push((id, answer));
        }

        fn answers(&mut self) -> Vec<(CallId, Answer)> {
            std::mem::take(&mut self.back)
        }
    }

    #[test]
    fn each_call_goes_to_its_route_on_the_client() {
        use crate::api::tests::{canned_server_bodies, json_reply, seen_json};
        let path = "Music/Daft Punk/Discovery/One_More_Time.mp3";
        let done = json!({ "id": "job-1", "pid": 4101, "url": LINK, "directory": DIR, "outputCodec": "mp3", "status": "complete",
            "startTime": T0, "endTime": T0 + 9000, "fileName": "One_More_Time.mp3", "filePath": path, "downloadId": 8 });
        let row = json!({ "id": 8, "plugin": "ytdl", "vpath": "Music", "filepath": path, "relativePath": "Daft Punk/Discovery/One_More_Time.mp3",
            "origin": LINK, "downloadedAt": T0 + 9000, "present": true, "trackId": 31 });
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({ "downloads": [{ "id": 7, "plugin": "ytdl", "filepath": "Music/a.mp3", "origin": OTHER }] })),
            json_reply("200 OK", json!({ "downloads": [] })),
            json_reply("200 OK", json!({ "message": "Download started", "id": "job-1", "fileName": "One_More_Time.mp3" })),
            json_reply("200 OK", json!({ "downloads": [done] })),
            json_reply("200 OK", json!({ "downloads": [row] })),
        ]);
        let client = crate::api::Client::new(&format!("http://127.0.0.1:{port}")).expect("a client");
        let mut t = Tracker::new(1, Direct { client, back: Vec::new() }, Tock(T0), Shelf::default());
        let id = t.start(request(LINK, DIR, Origin::Room));
        for _ in 0..10 {
            t.tick();
        }
        let lines: Vec<(String, Value)> = (0..5).map(|_| seen_json(&seen)).collect();
        let starts = ["GET /api/v1/discovery/downloads?plugin=ytdl&limit=1 ", "GET /api/v1/ytdl/downloads ", "POST /api/v1/ytdl/ ",
            "GET /api/v1/ytdl/downloads ", "GET /api/v1/discovery/downloads?plugin=ytdl&limit=20 "];
        for ((line, _), start) in lines.iter().zip(starts) {
            assert!(line.starts_with(start), "{line} / {start}");
        }
        assert_eq!(
            lines[2].1,
            json!({ "directory": DIR, "url": LINK, "outputCodec": "mp3", "metadata": { "title": "One More Time", "artist": "Daft Punk" } })
        );
        let job = t.job(&id).expect("the job");
        assert_eq!((job.state, job.record_floor, job.record_id), (JobState::Added, Some(7), Some(8)));
        assert_eq!(job.file_path.as_deref(), Some(path));
        assert!(!t.in_flight());
        let rows = t.added();
        assert_eq!((rows.len(), rows[0].track_id, rows[0].job.as_deref()), (1, Some(31), Some(id.as_str())));
    }

    #[test]
    fn a_record_answers_for_its_own_job_only() {
        let mut job = Job::fresh("j".into(), LINK.into(), DIR.into(), "mp3".into(), YtdlTags::default(), "t".into(), "One_More_Time.mp3".into(), None, None, T0);
        let r = |id: i64, path: &str, origin: &str, at: i64| record(id, "ytdl", path, origin, at);
        let here = "Music/Daft Punk/Discovery/One_More_Time.mp3";
        assert!(answers(&job, &r(9, here, LINK, T0), false), "nothing to bound it by: any match");
        assert!(!answers(&job, &r(9, here, LINK, T0), true), "…unless bounded");
        assert!(answers(&job, &r(9, "music/daft punk/DISCOVERY/x.mp3", LINK, T0), false), "the folder ignoring case");
        assert!(!answers(&job, &r(9, here, OTHER, T0), false));
        assert!(!answers(&job, &r(9, "Music/Daft Punk/x.mp3", LINK, T0), false));
        job.server_start = Some(T0 + SKEW);
        assert!(!answers(&job, &r(9, here, LINK, T0), false), "written before the entry started");
        assert!(answers(&job, &r(9, here, LINK, T0 + SKEW), true));
        job.record_floor = Some(9);
        assert!(!answers(&job, &r(9, here, LINK, T0 + SKEW), false), "at or below the floor");
        job.record_id = Some(3);
        assert!(answers(&job, &r(3, "elsewhere/x.mp3", OTHER, 0), false), "the one the entry named");
        assert!(!answers(&job, &r(10, here, LINK, T0 + SKEW), false));
        assert_eq!(renamed("One_More_Time.mp3", "aac"), "One_More_Time.m4a");
        assert_eq!(renamed("", "opus"), "");
        assert_eq!((folder_of(here), name_of(here), ext_of("x.MP3")), ("Music/Daft Punk/Discovery".into(), "One_More_Time.mp3", Some("mp3".into())));
    }
}
