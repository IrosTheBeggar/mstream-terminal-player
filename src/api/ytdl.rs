//! Add from YouTube's routes (mStream `src/api/ytdl.js`, the download
//! record of `src/api/discovery-downloads.js`, and the file explorer's
//! listing and mkdir): the lookup, the add, the job list, the record and
//! the folder calls the GUI's room and window make. Each is meant for a
//! thread of its own, on a client built for that one call (the torrent
//! room's `session_client`) — so the add and mkdir go out on a fresh
//! connection, as the phone's do: a pooled keep-alive socket the server
//! has already dropped can swallow the answer to a request it carried out.
//!
//! The answers keep what every line of the copy is decided by, which
//! [`super::ApiError`] throws away: the status, the server's own words
//! and whether it sent any, a 409's `reason` and `fileName` — and, for a
//! call that got no answer, whether our bound ran out, the connection
//! could not be made, or there was no way to the server at all
//! ([`YtdlCallError`]; the phone's `YtdlApiException`, the webapp's
//! `answerOf`). Which words those become is the rules' to say, not this
//! file's.
//!
//! Two kinds of server answer here, told apart by the ping's `ytdl`
//! generation and, below it, the record probe ([`YtdlTier`]): from
//! generation 1 the caller's own jobs by id, a 409 before anything
//! downloads, reasons and `fileStem`; before it a global tracker by pid
//! that forgets a settled entry after 30 s, a POST that answers
//! `{message}` alone, and a lookup of five fields. Every shape is read
//! leniently: a field of the wrong type reads as nothing and never loses
//! the rest of the answer.

use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::future::Future;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[cfg(not(target_arch = "wasm32"))]
use super::Client;

/// The lookup's bound. The server gives yt-dlp 60 s of its own, and a
/// fresh server may fetch yt-dlp during the first one: past this it is
/// "The server took too long", and the lookup is not sent again by itself.
pub const LOOKUP_BOUND: Duration = Duration::from_secs(30);
/// The add's bound: the server's name check alone may take 15 s before it
/// answers, and a first use may fetch yt-dlp.
pub const ADD_BOUND: Duration = Duration::from_secs(30);
pub const MKDIR_BOUND: Duration = Duration::from_secs(15);
/// The quick reads' bound — the job list, the record, a listing. They
/// answer in milliseconds, so one that hangs is asked again rather than
/// waited out; over a tunnel's bridge, [`TUNNEL_READ_BOUND`].
pub const READ_BOUND: Duration = Duration::from_secs(10);
pub const TUNNEL_READ_BOUND: Duration = Duration::from_secs(20);

/// The record is read this many rows a page, and at most [`RECORD_PAGES`]
/// pages for one read of the Added list ([`Client::ytdl_added`]).
pub const RECORD_PAGE: u32 = 20;
pub const RECORD_PAGES: usize = 5;
/// The record's plug-in name for what `POST /ytdl` added.
pub const RECORD_PLUGIN: &str = "ytdl";

// ── What can go wrong ───────────────────────────────────────────────────────

/// Why a call got no answer at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoAnswer {
    /// Sent, and our bound ran out before the answer came: the server may
    /// be at work on it — an add may have started.
    TimedOut,
    /// Nothing was sent: the connection could not be made although there
    /// is a way to the server's machine — nothing listens at its address
    /// (the server isn't running), it did not take the connection in
    /// time, or the handshake failed.
    Refused,
    /// Nothing was sent: there is no way to the server from here — no
    /// network, no route to its network, or its name does not resolve.
    Offline,
    /// The connection was made and lost before an answer came back: the
    /// request may have been carried out.
    Dropped,
}

impl NoAnswer {
    /// The request certainly never reached the server (the phone's
    /// `ytdlNeverConnected`): an add with this answer started nothing,
    /// and asking again at once finds the same.
    pub fn never_connected(self) -> bool {
        matches!(self, NoAnswer::Refused | NoAnswer::Offline)
    }
}

/// A call to one of Add from YouTube's routes that did not give what it
/// asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YtdlCallError {
    /// No status came back. `detail` is the transport's own account, for
    /// the log — never shown, and without the address (a tunnel's carries
    /// its token).
    Unanswered { why: NoAnswer, detail: String },
    /// The server answered no: any non-2xx but a 401.
    Answered {
        status: u16,
        /// The body's `error`, verbatim, when it is a string with something
        /// in it — the server's own words: "FFmpeg is not available yet",
        /// "Library 'Music' not found", Joi's `"outputCodec" must be one of
        /// [...]`, and its catch-all "Server Error" too (the rules read
        /// that one as a bare failure). `None` for an answer that carried
        /// none: Express's HTML 404 for a route the server does not have, a
        /// proxy's error page, an `error` of only spaces.
        words: Option<String>,
        /// A 409's `reason` (`exists`).
        reason: Option<String>,
        /// A 409's `fileName`: the name the file would have landed as.
        file_name: Option<String>,
    },
    /// 401: the session's problem — the App's own calls see it too.
    Unauthorized,
    /// A 2xx whose body is not what the call reads (the lookup, the lists,
    /// a listing). Never from the add or mkdir: any 2xx is their answer.
    Unreadable { status: u16 },
    /// The call could not be made: a peer's client (these routes are the
    /// user's own server's), an address that would not build.
    Config(String),
}

impl YtdlCallError {
    /// The status that came back, a 401's included.
    pub fn status(&self) -> Option<u16> {
        match self {
            YtdlCallError::Answered { status, .. } | YtdlCallError::Unreadable { status } => Some(*status),
            YtdlCallError::Unauthorized => Some(401),
            YtdlCallError::Unanswered { .. } | YtdlCallError::Config(_) => None,
        }
    }

    /// The server's own words, when it sent any.
    pub fn words(&self) -> Option<&str> {
        match self {
            YtdlCallError::Answered { words, .. } => words.as_deref(),
            _ => None,
        }
    }

    /// The answer carried the server's words (the phone's `fromServer`).
    pub fn from_server(&self) -> bool {
        self.words().is_some()
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            YtdlCallError::Answered { reason, .. } => reason.as_deref(),
            _ => None,
        }
    }

    pub fn file_name(&self) -> Option<&str> {
        match self {
            YtdlCallError::Answered { file_name, .. } => file_name.as_deref(),
            _ => None,
        }
    }

    /// Why no answer came, when none did.
    pub fn no_answer(&self) -> Option<NoAnswer> {
        match self {
            YtdlCallError::Unanswered { why, .. } => Some(*why),
            _ => None,
        }
    }

    /// Our bound ran out: "The server took too long" for the lookup, "not
    /// sure it started" for the add.
    pub fn timed_out(&self) -> bool {
        self.no_answer() == Some(NoAnswer::TimedOut)
    }

    /// See [`NoAnswer::never_connected`].
    pub fn never_connected(&self) -> bool {
        self.no_answer().is_some_and(NoAnswer::never_connected)
    }

    /// Express's own 404, with no words: the server has no such route.
    /// On the record's route that is a server before 6.30.
    pub fn no_route(&self) -> bool {
        matches!(self, YtdlCallError::Answered { status: 404, words: None, .. })
    }
}

/// For the log; what a person reads is the rules' copy.
impl fmt::Display for YtdlCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            YtdlCallError::Unanswered { why, detail } => {
                let what = match why {
                    NoAnswer::TimedOut => "no answer in time",
                    NoAnswer::Refused => "could not connect",
                    NoAnswer::Offline => "no way to the server",
                    NoAnswer::Dropped => "the connection was lost before the answer",
                };
                write!(f, "{what}: {detail}")
            }
            YtdlCallError::Answered { status, words, reason, .. } => {
                write!(f, "HTTP {status}")?;
                if let Some(words) = words {
                    write!(f, ": {words}")?;
                }
                if let Some(reason) = reason {
                    write!(f, " ({reason})")?;
                }
                Ok(())
            }
            YtdlCallError::Unauthorized => write!(f, "not authorized"),
            YtdlCallError::Unreadable { status } => write!(f, "HTTP {status} with a body this client cannot read"),
            YtdlCallError::Config(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for YtdlCallError {}

// ── Lenient fields ──────────────────────────────────────────────────────────

/// A string with something in it; anything else — null, "", a number — is
/// none (the phone's `_text`).
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    })
}

/// A string as it came; anything else is empty.
fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        _ => String::new(),
    })
}

/// A whole number — `2001.0` from a lenient encoder too; anything else is
/// none (the phone's `_int`).
fn whole_of(v: &Value) -> Option<i64> {
    let n = v.as_number()?;
    n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0).map(|f| f as i64))
}

fn whole<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(whole_of(&Value::deserialize(d)?))
}

fn whole_or_zero<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(whole_of(&Value::deserialize(d)?).unwrap_or(0))
}

fn number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(Value::deserialize(d)?.as_f64().filter(|f| f.is_finite()))
}

/// `true`, or a SQLite `1`; anything else is false.
fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_i64() == Some(1),
        _ => false,
    })
}

/// The lookup's `year` is yt-dlp's `release_year` — often a number — or
/// the start of `release_date`, a string; the add takes it only as a
/// string. A number reads as its integer digits, and one with a fraction
/// as no year (the phone's `_yearText`).
fn year<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        v @ Value::Number(_) => whole_of(&v).map(|y| y.to_string()),
        Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    })
}

// ── The shapes ──────────────────────────────────────────────────────────────

/// What yt-dlp read off a video: `GET /api/v1/ytdl/metadata`. Every key
/// is `x || null` on the server; an older server sends only the first
/// five.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct YtdlMetadata {
    #[serde(deserialize_with = "text")]
    pub title: Option<String>,
    /// The artist tag, else the creator, else the uploader — so it can be
    /// a channel name ("Daft Punk - Topic"); the tag guess strips those.
    #[serde(deserialize_with = "text")]
    pub artist: Option<String>,
    #[serde(deserialize_with = "text")]
    pub album: Option<String>,
    #[serde(deserialize_with = "year")]
    pub year: Option<String>,
    #[serde(deserialize_with = "text")]
    pub thumbnail: Option<String>,
    /// The name the file would land as, without its extension (yt-dlp's
    /// restricted rule over the raw title). Generation 1 only: an older
    /// server's name is predicted here.
    #[serde(rename = "fileStem", deserialize_with = "text")]
    pub file_stem: Option<String>,
    #[serde(rename = "isLive", deserialize_with = "flag")]
    pub is_live: bool,
    /// yt-dlp's `live_status`: is_live, is_upcoming, was_live, post_live,
    /// not_live.
    #[serde(rename = "liveStatus", deserialize_with = "text")]
    pub live_status: Option<String>,
    #[serde(rename = "durationSec", deserialize_with = "number")]
    pub duration_sec: Option<f64>,
    #[serde(deserialize_with = "text")]
    pub channel: Option<String>,
}

impl YtdlMetadata {
    /// A stream: on air now, upcoming, or just ended and not processed
    /// yet (the server's `isLiveEntry`, the webapp's `isLiveMeta`).
    pub fn is_stream(&self) -> bool {
        self.is_live || matches!(self.live_status.as_deref(), Some("is_live" | "is_upcoming" | "post_live"))
    }
}

/// The tags an add writes, as the edit-tags fields hold them. A blank one
/// is not sent: the server writes nothing for an empty value anyway, and
/// its schema refuses null — and any other key, and with it the add.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtdlTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
}

/// One `POST /api/v1/ytdl/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtdlAdd {
    /// The folder: the library's name, then the path inside it
    /// ("Music/Inbox"). The job echoes it exactly as sent, so a claim
    /// compares it without the slashes at either end.
    pub directory: String,
    /// The link as `https://<host>/watch?v=<id>`: the server's sanitiser
    /// needs the `?v=` (a bare youtu.be link is a 400 there).
    pub url: String,
    /// A format `supportedAudioFiles` marks on.
    pub codec: String,
    pub tags: YtdlTags,
}

impl YtdlAdd {
    /// The body as the route takes it: only the non-blank tags, sent as
    /// typed, and no `metadata` key at all when none is left (the phone's
    /// `add`).
    pub fn body(&self) -> Value {
        let mut body = serde_json::json!({
            "directory": self.directory,
            "url": self.url,
            "outputCodec": self.codec,
        });
        let tags = &self.tags;
        let mut metadata = serde_json::Map::new();
        for (key, value) in [("title", &tags.title), ("artist", &tags.artist), ("album", &tags.album), ("year", &tags.year)] {
            if !value.trim().is_empty() {
                metadata.insert(key.to_string(), Value::String(value.clone()));
            }
        }
        if !metadata.is_empty() {
            body["metadata"] = Value::Object(metadata);
        }
        body
    }
}

/// The add's 200: `{message, id, fileName}` from generation 1, `{message}`
/// before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct YtdlStarted {
    #[serde(deserialize_with = "text")]
    pub message: Option<String>,
    /// The job's id: follow it in the job list. `None` from an older
    /// server — and from a 2xx this client could not read, which still
    /// started: either way the job is looked for by its link and folder.
    #[serde(deserialize_with = "text")]
    pub id: Option<String>,
    /// The name it should land as; `None` when the server's probe could
    /// not read it, and from an older server.
    #[serde(rename = "fileName", deserialize_with = "text")]
    pub file_name: Option<String>,
}

/// One entry of `GET /api/v1/ytdl/downloads`.
///
/// Generation 1: the caller's own jobs, newest first, settled ones kept
/// 24 h (at most 50), with an id, the landed path and record, the words and
/// a reason. Before it: every account's entries mixed, keyed by pid,
/// forgotten 30 s after they settle, with no id, no error and no path —
/// which is why those are claimed by link and folder, then followed by
/// pid. A server restart forgets them all either way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct YtdlEntry {
    #[serde(deserialize_with = "text")]
    pub id: Option<String>,
    /// yt-dlp's process id; `None` on a new server's job that never got one.
    #[serde(deserialize_with = "whole")]
    pub pid: Option<i64>,
    /// The link as the server cleaned it (`?v=` only).
    #[serde(deserialize_with = "string")]
    pub url: String,
    /// Exactly as the add sent it.
    #[serde(deserialize_with = "string")]
    pub directory: String,
    #[serde(rename = "outputCodec", deserialize_with = "string")]
    pub output_codec: String,
    /// `downloading` · `complete` · `error`, kept raw: a value this build
    /// does not know is none of the three below.
    #[serde(deserialize_with = "string")]
    pub status: String,
    /// ms epochs on the SERVER's clock — never compared with ours but
    /// within a slack.
    #[serde(rename = "startTime", deserialize_with = "whole")]
    pub start_time: Option<i64>,
    #[serde(rename = "endTime", deserialize_with = "whole")]
    pub end_time: Option<i64>,
    /// Predicted by the add's probe, then the name it really landed with.
    #[serde(rename = "fileName", deserialize_with = "text")]
    pub file_name: Option<String>,
    /// Once complete: the song's library path ("Music/Inbox/x.mp3": no
    /// leading slash, the disk's own case).
    #[serde(rename = "filePath", deserialize_with = "text")]
    pub file_path: Option<String>,
    /// Once complete: its record's id.
    #[serde(rename = "downloadId", deserialize_with = "whole")]
    pub download_id: Option<i64>,
    /// Once failed: what went wrong, in words fit to show.
    #[serde(deserialize_with = "text")]
    pub error: Option<String>,
    /// Once failed, when a code fits: exists, too-big, too-long, live,
    /// unavailable. None with `error` set: the words are the answer.
    #[serde(deserialize_with = "text")]
    pub reason: Option<String>,
}

impl YtdlEntry {
    pub fn downloading(&self) -> bool {
        self.status == "downloading"
    }

    pub fn complete(&self) -> bool {
        self.status == "complete"
    }

    pub fn failed(&self) -> bool {
        self.status == "error"
    }

    /// An entry with something to follow it by — an id, or a pid — and a
    /// link and folder to claim it by; the rest are left out of the list.
    fn usable(&self) -> bool {
        (self.id.is_some() || self.pid.is_some()) && !self.url.is_empty() && !self.directory.is_empty()
    }
}

/// One row of the account's download record (`GET
/// /api/v1/discovery/downloads`, mStream 6.30+) that came from `POST
/// /ytdl` — this player's adds, the web app's, the phone's: durable across
/// restarts. It carries no format: that is the file's extension.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct YtdlRecord {
    #[serde(deserialize_with = "whole_or_zero")]
    pub id: i64,
    #[serde(deserialize_with = "string")]
    pub plugin: String,
    #[serde(deserialize_with = "string")]
    pub vpath: String,
    /// `<vpath>/<relativePath>`, no leading slash.
    #[serde(deserialize_with = "string")]
    pub filepath: String,
    #[serde(rename = "relativePath", deserialize_with = "string")]
    pub relative_path: String,
    /// The cleaned link it was added from (the job's `url`).
    #[serde(deserialize_with = "string")]
    pub origin: String,
    #[serde(deserialize_with = "text")]
    pub title: Option<String>,
    #[serde(deserialize_with = "text")]
    pub artist: Option<String>,
    #[serde(deserialize_with = "text")]
    pub album: Option<String>,
    #[serde(deserialize_with = "whole")]
    pub bytes: Option<i64>,
    /// ms epoch, the server's clock.
    #[serde(rename = "downloadedAt", deserialize_with = "whole")]
    pub downloaded_at: Option<i64>,
    /// The library still has the song (its track row is there); false is
    /// the list's "file missing".
    #[serde(deserialize_with = "flag")]
    pub present: bool,
    #[serde(rename = "trackId", deserialize_with = "whole")]
    pub track_id: Option<i64>,
}

/// One page of the record ([`Client::ytdl_records`]): its ytdl rows, and
/// what the whole page held. Without the server's plug-in filter (before
/// generation 1) the route's `limit` counts every plug-in's rows, so a page
/// of Discover downloads holds no ytdl row and is still not the end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtdlRecordPage {
    /// The page's ytdl rows, newest first.
    pub rows: Vec<YtdlRecord>,
    /// Every row on the page that had an id, any plug-in's.
    pub count: usize,
    /// The `limit` the page was asked for.
    pub limit: u32,
    /// The smallest and largest id on the page, any plug-in's: the next
    /// page's `before`, and the newest record the server holds (a first
    /// page). `None` on an empty page.
    pub oldest_id: Option<i64>,
    pub newest_id: Option<i64>,
    /// The page's earliest `downloadedAt`, any plug-in's.
    pub oldest_at: Option<i64>,
}

impl YtdlRecordPage {
    /// Nothing older is left: the page held no row at all, or fewer than
    /// it asked for. Its own rows still count — a short last page has some.
    pub fn end(&self) -> bool {
        self.count == 0 || self.count < self.limit as usize
    }
}

/// One read of the Added list ([`Client::ytdl_added`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtdlAddedRead {
    /// The ytdl rows read, newest first.
    pub rows: Vec<YtdlRecord>,
    /// The smallest id read: the next read's `before` ("Show earlier").
    pub cursor: Option<i64>,
    /// The record ended on the last page read.
    pub end: bool,
    /// The largest id on the first page read — from the top (no `before`),
    /// the newest record the server holds, any plug-in's: an add's floor
    /// for telling its own record from older ones.
    pub newest_id: Option<i64>,
}

/// A folder as `POST /api/v1/file-explorer` lists it, names only: enough
/// to know it is there and what is in it before an add.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtdlListing {
    /// The folder as the server names it ("/Music/Daft Punk/": a slash at
    /// either end, the disk's own case); the one asked for when it did not
    /// say.
    pub path: String,
    pub directories: Vec<String>,
    pub files: Vec<String>,
}

impl YtdlListing {
    fn of(map: &serde_json::Map<String, Value>, asked: &str) -> YtdlListing {
        let names = |key: &str| -> Vec<String> {
            map.get(key)
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|e| e.get("name").and_then(Value::as_str))
                        .filter(|n| !n.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let path = map.get("path").and_then(Value::as_str).filter(|p| !p.is_empty());
        YtdlListing {
            path: path.unwrap_or(asked).to_string(),
            directories: names("directories"),
            files: names("files"),
        }
    }
}

/// What a server's ytdl routes can do — which rules follow an add, and
/// whether there is a record to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YtdlTier {
    /// Before 6.30: no record (Express's own 404 on the probe), and an add
    /// re-tags a same-name file already in the folder — warned of ahead.
    Older,
    /// 6.30 up to generation 1: the record answers, the tracker is global
    /// and keyed by pid, a settled entry goes after 30 s, the add answers
    /// `{message}` alone, and a name clash fails after the download.
    Full,
    /// Generation 1 (the ping's `ytdl`): jobs by id, a 409 before anything
    /// downloads, reasons, `fileStem`, the record filtered by the server.
    New,
}

impl YtdlTier {
    /// The ping's generation alone: [`YtdlTier::New`] from 1. Below it the
    /// probe decides ([`Client::ytdl_probe_tier`]).
    pub fn from_generation(generation: u32) -> Option<YtdlTier> {
        (generation >= 1).then_some(YtdlTier::New)
    }
}

// ── Reading an answer ───────────────────────────────────────────────────────

/// A non-2xx in the server's words, as the routes send them: `{error}`,
/// and on the add's 409 `{error, reason, fileName}`.
fn refusal(status: u16, text: &str) -> YtdlCallError {
    let body: Option<Value> = serde_json::from_str(text).ok();
    let field = |key: &str| body.as_ref().and_then(|b| b.get(key)).and_then(Value::as_str);
    YtdlCallError::Answered {
        status,
        words: field("error").filter(|w| !w.trim().is_empty()).map(str::to_string),
        reason: field("reason").filter(|r| !r.is_empty()).map(str::to_string),
        file_name: field("fileName").filter(|n| !n.is_empty()).map(str::to_string),
    }
}

/// A 2xx's body as the object every read answers with.
fn object(status: u16, text: &str) -> Result<serde_json::Map<String, Value>, YtdlCallError> {
    match serde_json::from_str(text) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(YtdlCallError::Unreadable { status }),
    }
}

/// A record page's rows read: every row with an id counts, only the
/// ytdl ones are kept.
fn page_of(list: &[Value], limit: u32) -> YtdlRecordPage {
    let mut page = YtdlRecordPage { limit, ..YtdlRecordPage::default() };
    for row in list {
        let Some(id) = row.get("id").and_then(whole_of) else { continue };
        page.count += 1;
        page.oldest_id = Some(page.oldest_id.map_or(id, |o| o.min(id)));
        page.newest_id = Some(page.newest_id.map_or(id, |n| n.max(id)));
        if let Some(at) = row.get("downloadedAt").and_then(whole_of) {
            page.oldest_at = Some(page.oldest_at.map_or(at, |o| o.min(at)));
        }
        if row.get("plugin").and_then(Value::as_str) == Some(RECORD_PLUGIN)
            && let Ok(record) = serde_json::from_value::<YtdlRecord>(row.clone())
        {
            page.rows.push(record);
        }
    }
    page
}

// ── The calls ───────────────────────────────────────────────────────────────

/// When a call that got no answer is sent once more (the phone's
/// `_call`). Never when the connection could not be made: sending again
/// finds the same.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Again {
    /// The add: sent twice it starts two jobs. A lost answer is looked for
    /// in the job list instead.
    Never,
    /// The lookup: a lost connection, but not a bound that ran out — that
    /// is its answer ("The server took too long").
    OnDrop,
    /// The reads, and mkdir (recursive, and fine when the folder is there).
    OnNoAnswer,
}

#[cfg(not(target_arch = "wasm32"))]
impl Again {
    fn after(self, why: NoAnswer) -> bool {
        match self {
            Again::Never => false,
            Again::OnDrop => why == NoAnswer::Dropped,
            Again::OnNoAnswer => !why.never_connected(),
        }
    }
}

/// What a call is for (the phone's `needBody`).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wants {
    /// A read: its 2xx's body, so one lost on the way is no answer.
    Body,
    /// The add, mkdir: the status alone answers them.
    Status,
}

/// Why a send got no answer, read off the transport's error before it is
/// turned into words: a connection that could not be made is never an add
/// that may have started.
#[cfg(not(target_arch = "wasm32"))]
fn unanswered(e: reqwest::Error) -> YtdlCallError {
    // A connect that ran out of time is a connect first: nothing was sent.
    let why = if e.is_connect() {
        if no_way_there(&e) { NoAnswer::Offline } else { NoAnswer::Refused }
    } else if e.is_timeout() {
        NoAnswer::TimedOut
    } else {
        NoAnswer::Dropped
    };
    YtdlCallError::Unanswered { why, detail: account(&e.without_url()) }
}

/// The connect failed for want of a way there: the network is down, or
/// unreachable from here, or the name did not resolve (hyper's "dns
/// error"). A refused connection, one that timed out and a failed
/// handshake all had a way there.
#[cfg(not(target_arch = "wasm32"))]
fn no_way_there(e: &reqwest::Error) -> bool {
    use std::io::ErrorKind;
    let mut at: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = at {
        if err.to_string() == "dns error" {
            return true;
        }
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && matches!(io.kind(), ErrorKind::NetworkUnreachable | ErrorKind::NetworkDown | ErrorKind::AddrNotAvailable)
        {
            return true;
        }
        at = err.source();
    }
    false
}

/// An error and every cause under it, for the log.
#[cfg(not(target_arch = "wasm32"))]
fn account(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut at = std::error::Error::source(e);
    while let Some(err) = at {
        text.push_str(": ");
        text.push_str(&err.to_string());
        at = err.source();
    }
    text
}

/// [`super::wait`] for these calls: the runtime's own failure is a call
/// that could not be made.
#[cfg(not(target_arch = "wasm32"))]
fn finish<T>(fut: impl Future<Output = Result<T, YtdlCallError>>) -> Result<T, YtdlCallError> {
    crate::runtime::block_on(fut).map_err(YtdlCallError::Config)?
}

#[cfg(not(target_arch = "wasm32"))]
impl Client {
    /// `GET /api/v1/ytdl/metadata?url=` — yt-dlp's reading of the video,
    /// bounded by [`LOOKUP_BOUND`]. A failed lookup is the server's bare
    /// 500 (`{error: "Server Error"}`); a 2xx this client cannot read is
    /// as good as none ([`YtdlCallError::Unreadable`]).
    pub async fn ytdl_metadata_async(&self, url: &str) -> Result<YtdlMetadata, YtdlCallError> {
        self.ytdl_metadata_within(url, LOOKUP_BOUND).await
    }

    pub fn ytdl_metadata(&self, url: &str) -> Result<YtdlMetadata, YtdlCallError> {
        finish(self.ytdl_metadata_async(url))
    }

    async fn ytdl_metadata_within(&self, url: &str, longest: Duration) -> Result<YtdlMetadata, YtdlCallError> {
        let path = format!("api/v1/ytdl/metadata?url={}", super::urlencode(url));
        let (status, text) = self.ytdl_call(Method::GET, &path, None, longest, Again::OnDrop, Wants::Body).await?;
        let map = object(status, &text)?;
        // Every field is lenient: an object always reads.
        serde_json::from_value(Value::Object(map)).map_err(|_| YtdlCallError::Unreadable { status })
    }

    /// `POST /api/v1/ytdl/` (the route's own trailing slash): start the
    /// download into a folder that must be there, bounded by
    /// [`ADD_BOUND`]. Any 2xx is "started", whatever its body says —
    /// reading a garbled 200 as a failure would invite an Add again, and a
    /// second job. Never sent twice: an add with no answer is looked for
    /// in the job list, unless it [never connected](NoAnswer::never_connected).
    pub async fn ytdl_add_async(&self, add: &YtdlAdd) -> Result<YtdlStarted, YtdlCallError> {
        self.ytdl_add_within(add, ADD_BOUND).await
    }

    pub fn ytdl_add(&self, add: &YtdlAdd) -> Result<YtdlStarted, YtdlCallError> {
        finish(self.ytdl_add_async(add))
    }

    async fn ytdl_add_within(&self, add: &YtdlAdd, longest: Duration) -> Result<YtdlStarted, YtdlCallError> {
        let (_, text) =
            self.ytdl_call(Method::POST, "api/v1/ytdl/", Some(add.body()), longest, Again::Never, Wants::Status).await?;
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    /// `GET /api/v1/ytdl/downloads` — the server's job list, unwrapped from
    /// `{downloads}`; an entry with nothing to follow or claim it by is
    /// left out ([`YtdlEntry`]).
    pub async fn ytdl_jobs_async(&self) -> Result<Vec<YtdlEntry>, YtdlCallError> {
        let (status, text) =
            self.ytdl_call(Method::GET, "api/v1/ytdl/downloads", None, self.read_bound(), Again::OnNoAnswer, Wants::Body).await?;
        let map = object(status, &text)?;
        // A list that isn't one is a read that failed, never "every job
        // went": that would settle the open ones as gone.
        let Some(Value::Array(list)) = map.get("downloads") else {
            return Err(YtdlCallError::Unreadable { status });
        };
        Ok(list
            .iter()
            .filter(|v| v.is_object())
            .filter_map(|v| serde_json::from_value::<YtdlEntry>(v.clone()).ok())
            .filter(YtdlEntry::usable)
            .collect())
    }

    pub fn ytdl_jobs(&self) -> Result<Vec<YtdlEntry>, YtdlCallError> {
        finish(self.ytdl_jobs_async())
    }

    /// `GET /api/v1/discovery/downloads?limit=&before=` — one page of the
    /// account's record, newest first, its ytdl rows kept. `?plugin=ytdl`
    /// only from `generation` 1: an older record's schema refuses the key
    /// with a 400, so there the page holds every plug-in's rows and is
    /// narrowed here. `limit` is held to 1–500; `before` is a record id.
    ///
    /// `Ok(None)` when the server has no such route — Express's own 404,
    /// with no words: a server before 6.30. Anything else that fails is an
    /// error, a 404 that carries words included.
    pub async fn ytdl_records_async(
        &self,
        generation: u32,
        limit: u32,
        before: Option<i64>,
    ) -> Result<Option<YtdlRecordPage>, YtdlCallError> {
        let limit = limit.clamp(1, 500);
        let mut query = Vec::new();
        if YtdlTier::from_generation(generation).is_some() {
            query.push(format!("plugin={RECORD_PLUGIN}"));
        }
        query.push(format!("limit={limit}"));
        if let Some(before) = before.filter(|b| *b > 0) {
            query.push(format!("before={before}"));
        }
        let path = format!("api/v1/discovery/downloads?{}", query.join("&"));
        let asked = self.ytdl_call(Method::GET, &path, None, self.read_bound(), Again::OnNoAnswer, Wants::Body).await;
        let (status, text) = match asked {
            Err(e) if e.no_route() => return Ok(None),
            answer => answer?,
        };
        let map = object(status, &text)?;
        let Some(Value::Array(list)) = map.get("downloads") else {
            return Err(YtdlCallError::Unreadable { status });
        };
        Ok(Some(page_of(list, limit)))
    }

    pub fn ytdl_records(
        &self,
        generation: u32,
        limit: u32,
        before: Option<i64>,
    ) -> Result<Option<YtdlRecordPage>, YtdlCallError> {
        finish(self.ytdl_records_async(generation, limit, before))
    }

    /// One read of the Added list, newest first from `before` (`None`: the
    /// newest): pages of [`RECORD_PAGE`] until one holds a ytdl row or the
    /// record ends, at most [`RECORD_PAGES`] (the phone's `_readAdded`) —
    /// an unfiltered page of Discover downloads holds none and is not the
    /// end. `Ok(None)` on a server before 6.30, as [`Client::ytdl_records`].
    pub async fn ytdl_added_async(
        &self,
        generation: u32,
        before: Option<i64>,
    ) -> Result<Option<YtdlAddedRead>, YtdlCallError> {
        let mut read = YtdlAddedRead { cursor: before, ..YtdlAddedRead::default() };
        for page in 0..RECORD_PAGES {
            let Some(found) = self.ytdl_records_async(generation, RECORD_PAGE, read.cursor).await? else {
                return Ok(None);
            };
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
        Ok(Some(read))
    }

    pub fn ytdl_added(&self, generation: u32, before: Option<i64>) -> Result<Option<YtdlAddedRead>, YtdlCallError> {
        finish(self.ytdl_added_async(generation, before))
    }

    /// The server's tier: [`YtdlTier::New`] from the ping's generation
    /// without asking anything; below it the record's first read is the
    /// probe (the phone's `probeTier`) — an answer is [`YtdlTier::Full`]
    /// and the Added list's first read with it, Express's 404
    /// [`YtdlTier::Older`]. An error leaves the tier unknown: the older
    /// rules hold until it answers, and it is asked again.
    pub async fn ytdl_probe_tier_async(
        &self,
        generation: u32,
    ) -> Result<(YtdlTier, Option<YtdlAddedRead>), YtdlCallError> {
        if let Some(tier) = YtdlTier::from_generation(generation) {
            return Ok((tier, None));
        }
        Ok(match self.ytdl_added_async(generation, None).await? {
            Some(read) => (YtdlTier::Full, Some(read)),
            None => (YtdlTier::Older, None),
        })
    }

    pub fn ytdl_probe_tier(&self, generation: u32) -> Result<(YtdlTier, Option<YtdlAddedRead>), YtdlCallError> {
        finish(self.ytdl_probe_tier_async(generation))
    }

    /// `POST /api/v1/file-explorer/mkdir {directory}`, bounded by
    /// [`MKDIR_BOUND`]: recursive, and fine when the folder is already
    /// there — which is what makes its one more try safe. Any 2xx means
    /// the folder is there. 403 "Create Folder Disabled" for an account
    /// that may not make folders (the ping's `noMkdir`).
    pub async fn ytdl_mkdir_async(&self, directory: &str) -> Result<(), YtdlCallError> {
        let body = serde_json::json!({ "directory": directory });
        self.ytdl_call(Method::POST, "api/v1/file-explorer/mkdir", Some(body), MKDIR_BOUND, Again::OnNoAnswer, Wants::Status)
            .await?;
        Ok(())
    }

    pub fn ytdl_mkdir(&self, directory: &str) -> Result<(), YtdlCallError> {
        finish(self.ytdl_mkdir_async(directory))
    }

    /// `POST /api/v1/file-explorer {directory}` — a folder's names, its
    /// files' for the name check before an add, with a missing folder's
    /// 404 in the server's words ("Library 'Music' not found" is not
    /// "Folder not found"; a 500 on 6.26–6.28.0). Plain, no
    /// `pullMetadata`: the tags are not wanted, and a server from before
    /// the key refuses it.
    pub async fn ytdl_listing_async(&self, directory: &str) -> Result<YtdlListing, YtdlCallError> {
        let body = serde_json::json!({ "directory": directory });
        let (status, text) =
            self.ytdl_call(Method::POST, "api/v1/file-explorer", Some(body), self.read_bound(), Again::OnNoAnswer, Wants::Body)
                .await?;
        Ok(YtdlListing::of(&object(status, &text)?, directory))
    }

    pub fn ytdl_listing(&self, directory: &str) -> Result<YtdlListing, YtdlCallError> {
        finish(self.ytdl_listing_async(directory))
    }

    // ── Plumbing ────────────────────────────────────────────────────────────

    /// The quick reads' bound: a tunnel's bridge is slower to answer.
    fn read_bound(&self) -> Duration {
        if self.local_token.is_some() { TUNNEL_READ_BOUND } else { READ_BOUND }
    }

    /// One call to the user's own server, sent once more when it got no
    /// answer and `again` allows: the status and body of a 2xx, else
    /// what went wrong.
    async fn ytdl_call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        longest: Duration,
        again: Again,
        wants: Wants,
    ) -> Result<(u16, String), YtdlCallError> {
        self.own_server().map_err(|e| YtdlCallError::Config(e.to_string()))?;
        match self.ytdl_send(method.clone(), path, body.clone(), longest, wants).await {
            Err(YtdlCallError::Unanswered { why, detail }) if again.after(why) => {
                let route = path.split('?').next().unwrap_or(path);
                tracing::info!("ytdl: {route} got no answer ({detail}); asking once more");
                self.ytdl_send(method, path, body, longest, wants).await
            }
            answer => answer,
        }
    }

    async fn ytdl_send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        longest: Duration,
        wants: Wants,
    ) -> Result<(u16, String), YtdlCallError> {
        let req = self.request(method, path, body, Some(longest)).map_err(|e| YtdlCallError::Config(e.to_string()))?;
        let resp = req.send().await.map_err(unanswered)?;
        let status = resp.status();
        let text = match resp.text().await {
            Ok(text) => text,
            // A read's body lost on the way is no answer, asked again as
            // one. Anything else was answered by its status: an add whose
            // 200 came back did start, and a refusal is a refusal.
            Err(e) if wants == Wants::Body && status.is_success() => return Err(unanswered(e)),
            Err(_) => String::new(),
        };
        match status {
            StatusCode::UNAUTHORIZED => Err(YtdlCallError::Unauthorized),
            s if !s.is_success() => Err(refusal(s.as_u16(), &text)),
            s => Ok((s.as_u16(), text)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::{canned_server_bodies, json_reply, reply, seen_json};
    use serde_json::json;

    fn client(port: u16) -> Client {
        Client::new(&format!("http://127.0.0.1:{port}")).unwrap().with_token(Some("tok".into()))
    }

    /// A server that takes every connection and answers none, holding each
    /// open; it hands back one message per connection.
    fn silent_server() -> (u16, std::sync::mpsc::Receiver<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for sock in listener.incoming() {
                let Ok(sock) = sock else { return };
                held.push(sock);
                let _ = tx.send(());
            }
        });
        (port, rx)
    }

    fn connections(seen: &std::sync::mpsc::Receiver<()>) -> usize {
        std::thread::sleep(Duration::from_millis(100));
        seen.try_iter().count()
    }

    #[test]
    fn a_lookup_reads_either_servers_answer_and_keeps_the_years_digits() {
        // The new server's metadataOf (mStream #1123) — year as yt-dlp's
        // release_year, a number — then the five fields an older one sends.
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({ "title": "Remote Hit", "artist": "Nova", "album": "Night Ferry", "year": 2019,
                "thumbnail": "https://i.ytimg.com/vi/abc/maxresdefault.jpg", "fileStem": "Remote_Hit", "isLive": false,
                "liveStatus": "not_live", "durationSec": 253, "channel": "Nova - Topic" })),
            json_reply("200 OK", json!({ "title": "One More Time", "artist": "Daft Punk - Topic", "album": null, "year": "2001", "thumbnail": "" })),
            json_reply("200 OK", json!({ "title": 7, "year": 2001.0, "isLive": "yes", "durationSec": "253" })),
            json_reply("200 OK", json!({ "year": 2001.5, "liveStatus": "is_upcoming" })),
        ]);
        let api = client(port);
        let link = "https://www.youtube.com/watch?v=abc";
        let meta = api.ytdl_metadata(link).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Remote Hit"));
        assert_eq!(meta.year.as_deref(), Some("2019"), "a number reads as its digits");
        assert_eq!(meta.file_stem.as_deref(), Some("Remote_Hit"));
        assert_eq!((meta.duration_sec, meta.channel.as_deref()), (Some(253.0), Some("Nova - Topic")));
        assert!(!meta.is_live && !meta.is_stream());
        let (head, _) = seen.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            head.starts_with("GET /api/v1/ytdl/metadata?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3Dabc HTTP/1.1\r\n"),
            "{head}"
        );
        assert!(head.to_ascii_lowercase().contains("x-access-token: tok\r\n"), "{head}");

        let old = api.ytdl_metadata(link).unwrap();
        assert_eq!(old.year.as_deref(), Some("2001"));
        assert_eq!((old.album, old.thumbnail, old.file_stem), (None, None, None), "null, empty and absent are all none");
        assert_eq!(old.artist.as_deref(), Some("Daft Punk - Topic"));

        // One odd field never loses the rest.
        let odd = api.ytdl_metadata(link).unwrap();
        assert_eq!((odd.title, odd.year.as_deref(), odd.is_live, odd.duration_sec), (None, Some("2001"), false, None));
        let fraction = api.ytdl_metadata(link).unwrap();
        assert_eq!(fraction.year, None, "a year with a fraction is no year");
        assert!(fraction.is_stream(), "an upcoming stream is a stream");
    }

    #[test]
    fn a_failed_lookup_keeps_the_status_and_whether_the_server_had_words() {
        let (port, _seen) = canned_server_bodies(vec![
            json_reply("500 Internal Server Error", json!({ "error": "Server Error" })),
            json_reply("500 Internal Server Error", json!({ "error": "yt-dlp is not installed" })),
            json_reply("400 Bad Request", json!({ "error": "Invalid YouTube URL - missing video ID" })),
            json_reply("500 Internal Server Error", json!({ "error": "   " })),
            reply("401 Unauthorized", "application/json", br#"{"error":"Authentication Error"}"#),
            reply("200 OK", "text/html", b"<html>a proxy's page</html>"),
            reply("404 Not Found", "text/html", b"<pre>Cannot GET /api/v1/ytdl/metadata</pre>"),
        ]);
        let api = client(port);
        let link = "https://www.youtube.com/watch?v=abc";
        let bare = api.ytdl_metadata(link).unwrap_err();
        assert_eq!((bare.status(), bare.words()), (Some(500), Some("Server Error")), "the catch-all, verbatim");
        assert_eq!(api.ytdl_metadata(link).unwrap_err().words(), Some("yt-dlp is not installed"));
        let link_refused = api.ytdl_metadata(link).unwrap_err();
        assert_eq!(link_refused.status(), Some(400));
        assert!(link_refused.from_server());
        let blank = api.ytdl_metadata(link).unwrap_err();
        assert_eq!((blank.status(), blank.from_server()), (Some(500), false), "spaces are no words");
        assert_eq!(api.ytdl_metadata(link).unwrap_err(), YtdlCallError::Unauthorized);
        assert_eq!(api.ytdl_metadata(link).unwrap_err(), YtdlCallError::Unreadable { status: 200 });
        let express = api.ytdl_metadata(link).unwrap_err();
        assert!(express.no_route() && !express.from_server(), "Express's own 404: {express:?}");
    }

    #[test]
    fn an_add_sends_only_the_tags_with_something_in_them() {
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({ "message": "Download started", "id": "3b0f6c1e", "fileName": "Remote_Hit.mp3" })),
            json_reply("200 OK", json!({ "message": "Download started" })),
            reply("200 OK", "text/html", b"started, probably"),
        ]);
        let api = client(port);
        let add = YtdlAdd {
            directory: "Music/Inbox".into(),
            url: "https://www.youtube.com/watch?v=abc".into(),
            codec: "opus".into(),
            tags: YtdlTags { title: "Remote Hit".into(), artist: " Nova ".into(), album: "  ".into(), year: String::new() },
        };
        let started = api.ytdl_add(&add).unwrap();
        assert_eq!(started.id.as_deref(), Some("3b0f6c1e"));
        assert_eq!(started.file_name.as_deref(), Some("Remote_Hit.mp3"));
        let (line, body) = seen_json(&seen);
        assert!(line.starts_with("POST /api/v1/ytdl/ HTTP/1.1"), "{line}");
        assert_eq!(
            body,
            json!({ "directory": "Music/Inbox", "url": "https://www.youtube.com/watch?v=abc", "outputCodec": "opus",
                "metadata": { "title": "Remote Hit", "artist": " Nova " } }),
            "blank tags dropped, the rest sent as typed"
        );

        // An older server: `{message}` alone — started, nothing to follow by.
        let none = YtdlAdd { tags: YtdlTags::default(), ..add.clone() };
        let old = api.ytdl_add(&none).unwrap();
        assert_eq!((old.message.as_deref(), old.id), (Some("Download started"), None));
        assert!(seen_json(&seen).1.get("metadata").is_none(), "no tags, no metadata key at all");

        // A 2xx this client cannot read still started: never a failure.
        assert_eq!(api.ytdl_add(&add).unwrap(), YtdlStarted::default());
    }

    #[test]
    fn an_adds_refusals_keep_the_reason_the_name_and_the_words() {
        let (port, _seen) = canned_server_bodies(vec![
            json_reply("409 Conflict", json!({ "error": "“Remote_Hit.mp3” is already in this folder, so nothing was added.",
                "reason": "exists", "fileName": "Remote_Hit.mp3" })),
            json_reply("403 Forbidden", json!({ "error": "Uploading Disabled" })),
            json_reply("400 Bad Request", json!({ "error": "\"outputCodec\" must be one of [mp3, flac]" })),
            json_reply("500 Internal Server Error", json!({ "error": "FFmpeg is not available yet" })),
            json_reply("404 Not Found", json!({ "error": "Library 'Music' not found" })),
            reply("404 Not Found", "text/html", b"<pre>Cannot POST /api/v1/ytdl/</pre>"),
        ]);
        let api = client(port);
        let add = YtdlAdd { directory: "Music".into(), url: "https://www.youtube.com/watch?v=abc".into(), codec: "mp3".into(), ..YtdlAdd::default() };
        let clash = api.ytdl_add(&add).unwrap_err();
        assert_eq!(clash.status(), Some(409));
        assert_eq!((clash.reason(), clash.file_name()), (Some("exists"), Some("Remote_Hit.mp3")));
        assert!(clash.words().unwrap().starts_with("“Remote_Hit.mp3” is already"));
        assert_eq!(api.ytdl_add(&add).unwrap_err().words(), Some("Uploading Disabled"));
        assert!(api.ytdl_add(&add).unwrap_err().words().unwrap().contains("outputCodec"));
        assert_eq!(api.ytdl_add(&add).unwrap_err().words(), Some("FFmpeg is not available yet"));
        let gone = api.ytdl_add(&add).unwrap_err();
        assert_eq!((gone.status(), gone.words()), (Some(404), Some("Library 'Music' not found")));
        assert!(!gone.no_route(), "a 404 in words is no missing route");
        assert!(api.ytdl_add(&add).unwrap_err().no_route());
    }

    #[test]
    fn an_add_is_never_sent_twice() {
        let add = YtdlAdd { directory: "Music".into(), url: "https://www.youtube.com/watch?v=abc".into(), codec: "mp3".into(), ..YtdlAdd::default() };
        // The connection dropped after the server read the add: it may
        // have started, so it is looked for — never sent again.
        let (port, seen) = canned_server_bodies(vec![Vec::new(), json_reply("200 OK", json!({ "id": "second" }))]);
        let dropped = client(port).ytdl_add(&add).unwrap_err();
        assert_eq!(dropped.no_answer(), Some(NoAnswer::Dropped), "{dropped:?}");
        assert!(!dropped.never_connected());
        seen.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err(), "one request reached the server");

        // Out of time: the same.
        let (port, connected) = silent_server();
        let late = wait_add(&client(port), &add);
        assert!(late.timed_out(), "{late:?}");
        assert_eq!(connections(&connected), 1);
    }

    fn wait_add(api: &Client, add: &YtdlAdd) -> YtdlCallError {
        finish(api.ytdl_add_within(add, Duration::from_millis(300))).unwrap_err()
    }

    #[test]
    fn a_lookup_out_of_time_is_its_answer_but_a_lost_one_is_asked_again() {
        let (port, connected) = silent_server();
        let late = finish(client(port).ytdl_metadata_within("https://www.youtube.com/watch?v=abc", Duration::from_millis(300)))
            .unwrap_err();
        assert!(late.timed_out(), "{late:?}");
        assert_eq!(connections(&connected), 1, "\"The server took too long\" — not asked again");

        let (port, seen) = canned_server_bodies(vec![Vec::new(), json_reply("200 OK", json!({ "title": "Remote Hit" }))]);
        let meta = client(port).ytdl_metadata("https://www.youtube.com/watch?v=abc").unwrap();
        assert_eq!(meta.title.as_deref(), Some("Remote Hit"));
        assert_eq!(seen.try_iter().count(), 2, "the dropped one, then the one more try");
    }

    #[test]
    fn a_server_that_isnt_there_is_told_from_no_way_there() {
        // Nothing listens on the discard port: refused, before anything was sent.
        let refused = Client::new("http://127.0.0.1:9").unwrap().ytdl_jobs().unwrap_err();
        assert_eq!(refused.no_answer(), Some(NoAnswer::Refused), "{refused:?}");
        assert!(refused.never_connected());
        assert!(!refused.to_string().contains("127.0.0.1:9/api"), "no address in the log line: {refused}");
        // A name that never resolves (RFC 2606): no way there.
        let unnamed = Client::new("http://ytdl-rig.invalid:3000").unwrap().ytdl_jobs().unwrap_err();
        assert_eq!(unnamed.no_answer(), Some(NoAnswer::Offline), "{unnamed:?}");
        assert!(unnamed.never_connected());
    }

    #[test]
    fn the_reads_are_asked_once_more_when_the_answer_is_lost_or_late() {
        let (port, connected) = silent_server();
        let api = client(port);
        let jobs = api.ytdl_call(Method::GET, "api/v1/ytdl/downloads", None, Duration::from_millis(300), Again::OnNoAnswer, Wants::Body);
        let late = finish(jobs).unwrap_err();
        assert!(late.timed_out());
        assert_eq!(connections(&connected), 2, "a read that ran out of time is asked once more");

        let (port, seen) = canned_server_bodies(vec![Vec::new(), json_reply("200 OK", json!({ "downloads": [] }))]);
        assert!(client(port).ytdl_jobs().unwrap().is_empty());
        assert_eq!(seen.try_iter().count(), 2);
        let (port, seen) = canned_server_bodies(vec![Vec::new(), Vec::new()]);
        assert_eq!(client(port).ytdl_jobs().unwrap_err().no_answer(), Some(NoAnswer::Dropped), "once more, and no more");
        assert_eq!(seen.try_iter().count(), 2);
    }

    #[test]
    fn a_body_lost_after_its_status_is_no_answer_to_a_read_but_an_add_that_started() {
        // The head says a hundred bytes; the connection closes after twelve.
        let cut = || b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"downloads\"".to_vec();
        let (port, seen) = canned_server_bodies(vec![cut(), json_reply("200 OK", json!({ "downloads": [] }))]);
        assert!(client(port).ytdl_jobs().unwrap().is_empty(), "asked once more, and answered");
        assert_eq!(seen.try_iter().count(), 2);

        let (port, seen) = canned_server_bodies(vec![cut(), json_reply("200 OK", json!({ "id": "second" }))]);
        let add = YtdlAdd { directory: "Music".into(), url: "https://www.youtube.com/watch?v=abc".into(), codec: "mp3".into(), ..YtdlAdd::default() };
        assert_eq!(client(port).ytdl_add(&add).unwrap(), YtdlStarted::default(), "its 200 came back: it started");
        assert_eq!(seen.try_iter().count(), 1, "and it was never sent again");
    }

    #[test]
    fn the_job_list_reads_both_servers_and_leaves_out_what_cannot_be_followed() {
        let new = json!({ "id": "3b0f", "pid": 4242, "url": "https://www.youtube.com/watch?v=abc", "directory": "Music/Inbox",
            "outputCodec": "mp3", "status": "complete", "startTime": 1_761_900_000_000_i64, "endTime": 1_761_900_030_000_i64,
            "fileName": "Remote_Hit.mp3", "filePath": "Music/Inbox/Remote_Hit.mp3", "downloadId": 17, "error": null, "reason": null });
        let failed = json!({ "id": "9c1d", "pid": null, "url": "https://www.youtube.com/watch?v=def", "directory": "Music",
            "outputCodec": "opus", "status": "error", "startTime": 1, "endTime": 2, "fileName": null, "filePath": null,
            "downloadId": null, "error": "Sign in to confirm you’re not a bot.", "reason": null });
        // An older server's entry: pid and nothing to say how it went.
        let old = json!({ "pid": 977, "url": "https://www.youtube.com/watch?v=ghi", "directory": "/Music/x/", "outputCodec": "mp3",
            "status": "downloading", "startTime": 1_761_900_000_000_i64 });
        let unusable = [
            json!({ "url": "https://www.youtube.com/watch?v=jkl", "directory": "Music", "status": "downloading" }),
            json!({ "pid": 5, "directory": "Music" }),
            json!({ "id": "x", "url": "https://www.youtube.com/watch?v=mno" }),
            json!("not an entry"),
        ];
        let mut list = vec![new, failed, old];
        list.extend(unusable);
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({ "downloads": list })),
            json_reply("200 OK", json!({ "jobs": [] })),
        ]);
        let api = client(port);
        let jobs = api.ytdl_jobs().unwrap();
        assert_eq!(jobs.len(), 3, "{jobs:?}");
        assert!(jobs[0].complete() && jobs[0].file_path.as_deref() == Some("Music/Inbox/Remote_Hit.mp3"));
        assert_eq!((jobs[0].download_id, jobs[0].end_time), (Some(17), Some(1_761_900_030_000)));
        assert!(jobs[1].failed() && jobs[1].pid.is_none() && jobs[1].reason.is_none());
        assert_eq!(jobs[1].error.as_deref(), Some("Sign in to confirm you’re not a bot."));
        assert!(jobs[2].downloading() && jobs[2].id.is_none() && jobs[2].pid == Some(977));
        assert_eq!(jobs[2].directory, "/Music/x/", "the folder exactly as the add sent it");
        assert!(seen_json(&seen).0.starts_with("GET /api/v1/ytdl/downloads HTTP/1.1"));
        // A list that isn't there is a failed read, not an empty one.
        assert_eq!(api.ytdl_jobs().unwrap_err(), YtdlCallError::Unreadable { status: 200 });
    }

    fn record(id: i64, plugin: &str) -> Value {
        json!({ "id": id, "plugin": plugin, "userId": 1, "username": null, "jobId": null, "vpath": "Music",
            "filepath": format!("Music/Inbox/song{id}.mp3"), "relativePath": format!("Inbox/song{id}.mp3"),
            "fileHash": "abc", "origin": "https://www.youtube.com/watch?v=abc", "title": "Remote Hit", "artist": "Nova",
            "album": null, "bytes": 4_018_112, "downloadedAt": 1_761_900_000_000_i64 + id, "removedAt": null,
            "removedBy": null, "present": id % 2 == 0, "trackId": 300 + id })
    }

    #[test]
    fn the_record_is_filtered_by_the_server_from_generation_one_and_here_before_it() {
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({ "downloads": [record(12, "ytdl"), record(11, "ytdl")] })),
            json_reply("200 OK", json!({ "downloads": [record(9, "youtube"), record(8, "ytdl"), json!({ "plugin": "ytdl" }), record(7, "federation-copy")] })),
            json_reply("400 Bad Request", json!({ "error": "\"plugin\" is not allowed" })),
            reply("404 Not Found", "text/html", b"<pre>Cannot GET /api/v1/discovery/downloads</pre>"),
            json_reply("404 Not Found", json!({ "error": "Not Found" })),
        ]);
        let api = client(port);
        let page = api.ytdl_records(1, 20, None).unwrap().unwrap();
        assert!(seen_json(&seen).0.starts_with("GET /api/v1/discovery/downloads?plugin=ytdl&limit=20 HTTP/1.1"));
        assert_eq!(page.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![12, 11]);
        assert_eq!((page.count, page.oldest_id, page.newest_id), (2, Some(11), Some(12)));
        assert!(page.end(), "two of twenty: nothing older");
        let row = &page.rows[0];
        assert_eq!((row.filepath.as_str(), row.relative_path.as_str()), ("Music/Inbox/song12.mp3", "Inbox/song12.mp3"));
        assert_eq!((row.present, row.track_id, row.bytes, row.album.as_deref()), (true, Some(312), Some(4_018_112), None));
        assert!(!page.rows[1].present, "the list's file missing");

        // Before the generation: no plugin key, every plug-in's rows on the
        // page — counted, narrowed here.
        let mixed = api.ytdl_records(0, 3, Some(10)).unwrap().unwrap();
        assert!(seen_json(&seen).0.starts_with("GET /api/v1/discovery/downloads?limit=3&before=10 HTTP/1.1"));
        assert_eq!(mixed.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![8]);
        assert_eq!((mixed.count, mixed.oldest_id, mixed.newest_id), (3, Some(7), Some(9)), "a row with no id is not counted");
        assert!(!mixed.end(), "a full page of three is not the end");
        assert_eq!(mixed.oldest_at, Some(1_761_900_000_007));

        assert_eq!(api.ytdl_records(1, 20, None).unwrap_err().words(), Some("\"plugin\" is not allowed"));
        seen_json(&seen);
        assert_eq!(api.ytdl_records(0, 9999, None).unwrap(), None, "Express's own 404: before 6.30");
        assert!(seen_json(&seen).0.contains("limit=500"), "the limit held to the route's");
        assert!(api.ytdl_records(0, 20, None).is_err(), "a 404 in words is no missing route");
    }

    #[test]
    fn an_added_read_pages_past_other_plug_ins_rows_but_not_for_ever() {
        let page = |ids: std::ops::Range<i64>, plugin: &str| {
            json_reply("200 OK", json!({ "downloads": ids.rev().map(|id| record(id, plugin)).collect::<Vec<_>>() }))
        };
        let (port, seen) = canned_server_bodies(vec![
            // A first page of Discover downloads, then one with a ytdl row.
            page(81..101, "youtube"),
            json_reply("200 OK", json!({ "downloads": (61..81).rev().map(|id| record(id, if id == 70 { "ytdl" } else { "youtube" })).collect::<Vec<_>>() })),
            // Five pages of nothing of ours: the read stops there, not the end.
            page(400..420, "youtube"), page(380..400, "youtube"), page(360..380, "youtube"), page(340..360, "youtube"), page(320..340, "youtube"),
            // A short last page.
            page(1..4, "youtube"),
        ]);
        let api = client(port);
        let read = api.ytdl_added(0, None).unwrap().unwrap();
        assert_eq!(read.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![70]);
        assert_eq!((read.cursor, read.newest_id, read.end), (Some(61), Some(100), false));
        assert!(seen_json(&seen).0.starts_with("GET /api/v1/discovery/downloads?limit=20 "));
        assert!(seen_json(&seen).0.starts_with("GET /api/v1/discovery/downloads?limit=20&before=81 "));

        let empty = api.ytdl_added(0, Some(420)).unwrap().unwrap();
        assert!(empty.rows.is_empty() && !empty.end, "five pages, then it stops");
        assert_eq!(empty.cursor, Some(320));
        assert_eq!(seen.try_iter().count(), 5);

        let last = api.ytdl_added(0, Some(4)).unwrap().unwrap();
        assert!(last.rows.is_empty() && last.end);
        assert_eq!(last.cursor, Some(1));
    }

    #[test]
    fn the_tier_comes_from_the_generation_else_the_records_answer() {
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({ "downloads": [record(5, "ytdl")] })),
            reply("404 Not Found", "text/html", b"<pre>Cannot GET /api/v1/discovery/downloads</pre>"),
            json_reply("500 Internal Server Error", json!({ "error": "Server Error" })),
        ]);
        let api = client(port);
        assert_eq!(api.ytdl_probe_tier(1).unwrap(), (YtdlTier::New, None));
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err(), "the generation asks nothing");
        let (tier, added) = api.ytdl_probe_tier(0).unwrap();
        assert_eq!(tier, YtdlTier::Full);
        assert_eq!(added.unwrap().rows.len(), 1, "the probe is the Added list's first read");
        assert_eq!(api.ytdl_probe_tier(0).unwrap(), (YtdlTier::Older, None));
        assert!(api.ytdl_probe_tier(0).is_err(), "unknown: asked again later");
        assert_eq!(YtdlTier::from_generation(0), None);
        assert_eq!(YtdlTier::from_generation(2), Some(YtdlTier::New));
    }

    #[test]
    fn mkdir_takes_any_2xx_and_a_refusal_in_its_words() {
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({})),
            reply("204 No Content", "text/plain", b""),
            json_reply("403 Forbidden", json!({ "error": "Create Folder Disabled" })),
            // Lost, then asked again: recursive, so the folder is just there.
            Vec::new(),
            json_reply("200 OK", json!({})),
        ]);
        let api = client(port);
        api.ytdl_mkdir("Music/Inbox/New").unwrap();
        let (line, body) = seen_json(&seen);
        assert!(line.starts_with("POST /api/v1/file-explorer/mkdir HTTP/1.1"), "{line}");
        assert_eq!(body, json!({ "directory": "Music/Inbox/New" }));
        api.ytdl_mkdir("Music/Inbox/New").unwrap();
        let off = api.ytdl_mkdir("Music/Inbox/New").unwrap_err();
        assert_eq!((off.status(), off.words()), (Some(403), Some("Create Folder Disabled")));
        api.ytdl_mkdir("Music/Inbox/New").unwrap();
    }

    #[test]
    fn a_listing_is_names_and_a_missing_folder_keeps_its_words() {
        let (port, seen) = canned_server_bodies(vec![
            json_reply("200 OK", json!({ "path": "/Music/Daft Punk/", "directories": [{ "name": "Discovery" }, { "name": "" }],
                "files": [{ "type": "mp3", "name": "One_More_Time.mp3" }, { "type": "flac" }] })),
            json_reply("200 OK", json!({ "files": [] })),
            json_reply("404 Not Found", json!({ "error": "Folder not found" })),
        ]);
        let api = client(port);
        let listing = api.ytdl_listing("Music/daft punk").unwrap();
        assert_eq!(listing.path, "/Music/Daft Punk/", "the server's own spelling");
        assert_eq!((listing.directories, listing.files), (vec!["Discovery".to_string()], vec!["One_More_Time.mp3".to_string()]));
        assert_eq!(seen_json(&seen).1, json!({ "directory": "Music/daft punk" }), "plain: no pullMetadata");
        assert_eq!(api.ytdl_listing("Music/Inbox").unwrap().path, "Music/Inbox", "the one asked for when it says none");
        assert_eq!(api.ytdl_listing("Music/Gone").unwrap_err().words(), Some("Folder not found"));
    }

    #[test]
    fn a_peers_client_asks_nothing() {
        // The routes are off the federation allowlist: the user's own server only.
        let (port, seen) = canned_server_bodies(vec![json_reply("200 OK", json!({ "downloads": [] }))]);
        let peer = client(port).with_peer(Some(3));
        assert!(matches!(peer.ytdl_jobs(), Err(YtdlCallError::Config(_))));
        assert!(matches!(peer.ytdl_mkdir("Music/x"), Err(YtdlCallError::Config(_))));
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err(), "nothing was sent");
    }

    #[test]
    fn a_tunnel_client_carries_its_bridge_token_behind_the_link() {
        let (port, seen) = canned_server_bodies(vec![json_reply("200 OK", json!({ "title": "Remote Hit" }))]);
        let api = client(port).with_local_token(Some("bridge".into()));
        assert_eq!(api.read_bound(), TUNNEL_READ_BOUND);
        api.ytdl_metadata("https://youtu.be/abc?t=1").unwrap();
        let line = seen_json(&seen).0;
        assert!(line.starts_with("GET /api/v1/ytdl/metadata?url=https%3A%2F%2Fyoutu.be%2Fabc%3Ft%3D1&__lt=bridge "), "{line}");
    }
}
