//! A file's bytes, streamed to disk (the transfer plan's 2.4): `/media`, or
//! a peer client's stream proxy, resumed with Range and If-Range; a
//! conversion from `/transcode`, its type checked; and the small reads a
//! cover search and a quick fingerprint make. The model is the flasher's
//! `fetch_within` (src/device/firmware.rs): chunks as they land, a stall
//! timeout, a progress hook. Here into a file instead of memory, into a
//! writer that is only ever written on (a card's stage file, which so takes
//! a file's bytes once), or into one that can be cut back ([`Rewind`]: a
//! card's partial download, kept across runs), with a ceiling of the
//! request's own for bodies of gigabytes, and the client's meter counting
//! every byte.
//!
//! A peer's 429s are sorted by their words ([`Limited`]): the parent's
//! stream proxy passes the peer's status and body on, but not its
//! Retry-After (mStream's `federation-stream.js` copies six headers back).

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use reqwest::header::{
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, HeaderMap, HeaderName, IF_RANGE, LAST_MODIFIED,
    RANGE, RETRY_AFTER,
};
use reqwest::{Method, Response, StatusCode};

use crate::api::urls::{self, TransferBitrate, TransferCodec};
use crate::api::{ApiError, Client, extract_error, refused};

/// The longest a fetch goes on with no byte coming, unless the caller sets
/// its own: the flasher's figure. A slow link that still moves is no stall.
pub const STALL: Duration = Duration::from_secs(30);
/// The most one fetch writes: FAT32's largest file. A conversion past it
/// could never land on a card.
pub const FAT32_MAX: u64 = 0xFFFF_FFFF;
/// The slowest average a sized fetch may keep before its own ceiling ends
/// it: a link this slow still moves, which the stall timeout cannot see.
const FLOOR_RATE: u64 = 32 * 1024;
/// A ceiling's fixed part: the connection, the first byte, a busy server.
const CEILING_BASE: Duration = Duration::from_secs(600);
/// The ceiling of a fetch with no size to scale by, a conversion: the
/// server's ffmpeg sends as fast as it encodes, minutes for the longest
/// track on a slow box.
const UNSIZED_CEILING: Duration = Duration::from_secs(2 * 3600);
/// A probe's ceiling: a HEAD, or a few KiB.
const PROBE_CEILING: Duration = Duration::from_secs(60);
/// How much of a refusal's body is read for its words.
const WORDS_MAX: usize = 16 * 1024;
/// A key that forbids copies says so in these words, with a 403 (mStream's
/// `federation-limits.js`, `COPIES_OFF_MESSAGE`).
const COPIES_OFF: &str = "copies are not allowed with this key";

/// A fetch's own ceiling when the caller sets none: ten minutes, and the
/// bytes still to come at [`FLOOR_RATE`]. A fetch with no size to scale by
/// gets [`UNSIZED_CEILING`].
fn ceiling_for(remaining: Option<u64>) -> Duration {
    match remaining {
        Some(bytes) => CEILING_BASE + Duration::from_secs(bytes / FLOOR_RATE),
        None => UNSIZED_CEILING,
    }
}

// ── Shapes ──────────────────────────────────────────────────────────────────

/// Which bytes a fetch asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted<'a> {
    /// The file as the server holds it, byte for byte: `/media`, or a peer
    /// client's stream proxy.
    File(&'a str),
    /// A conversion from `/transcode`. It streams with no length and no
    /// ranges, so it always starts from the beginning, and a 200 that ends
    /// early (ffmpeg died) looks like any other: the caller checks the
    /// file's length in time. Never through a peer: keys and guests cannot
    /// call it.
    Converted { path: &'a str, codec: TransferCodec, bitrate: TransferBitrate },
}

impl Wanted<'_> {
    fn path(&self) -> &str {
        match self {
            Wanted::File(path) | Wanted::Converted { path, .. } => path,
        }
    }
}

/// One download to a file. Built from [`Fetch::file`] or
/// [`Fetch::converted`], then told what it may expect.
#[derive(Debug, Clone)]
pub struct Fetch<'a> {
    wanted: Wanted<'a>,
    expected: Option<u64>,
    resume: Option<&'a str>,
    stall: Duration,
    ceiling: Option<Duration>,
    cap: u64,
}

impl<'a> Fetch<'a> {
    pub fn file(path: &'a str) -> Self {
        Fetch {
            wanted: Wanted::File(path),
            expected: None,
            resume: None,
            stall: STALL,
            ceiling: None,
            cap: FAT32_MAX,
        }
    }

    pub fn converted(path: &'a str, codec: TransferCodec, bitrate: TransferBitrate) -> Self {
        Fetch { wanted: Wanted::Converted { path, codec, bitrate }, ..Fetch::file(path) }
    }

    /// The file's length, as the listing gave it (`file-size`). The server's
    /// own length must match before a byte is written, and so must the file
    /// at the end.
    pub fn expecting(mut self, len: u64) -> Self {
        self.expected = Some(len);
        self
    }

    /// Keep the bytes already at the destination and fetch the rest, if the
    /// file is still the one they came from: `validator` is what the fetch
    /// that wrote them answered ([`Partial::validator`]), sent as If-Range.
    /// The server answers a changed file whole, and the partial goes.
    /// Without a validator, or for a conversion, a fetch starts over.
    pub fn resuming(mut self, validator: Option<&'a str>) -> Self {
        self.resume = validator;
        self
    }

    /// How long no byte may come before the fetch gives up ([`STALL`]).
    pub fn stall(mut self, stall: Duration) -> Self {
        self.stall = stall;
        self
    }

    /// The whole request's ceiling, in place of [`ceiling_for`]'s.
    pub fn ceiling(mut self, ceiling: Duration) -> Self {
        self.ceiling = Some(ceiling);
        self
    }

    /// The most bytes the file may grow to ([`FAT32_MAX`]).
    pub fn cap(mut self, cap: u64) -> Self {
        self.cap = cap;
        self
    }
}

/// What a finished fetch wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The file's length now.
    pub len: u64,
    /// How many of those bytes were there already and kept; 0 for a fetch
    /// from the start.
    pub resumed_from: u64,
    /// The answer's ETag, else its Last-Modified: what a later partial of
    /// this file resumes with.
    pub validator: Option<String>,
    pub content_type: Option<String>,
}

/// What a failed fetch left at the destination: its length, and the
/// validator those bytes were fetched under. A partial with no validator
/// cannot be resumed (a conversion's, or a server that sent none).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Partial {
    pub len: u64,
    pub validator: Option<String>,
}

/// A fetch that did not finish, and what it left behind.
#[derive(Debug)]
pub struct Failed {
    pub error: FetchError,
    pub partial: Partial,
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({} bytes kept)", self.error, self.partial.len)
    }
}

impl std::error::Error for Failed {}

/// Which limit a 429 is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// "Too many concurrent streams": the key's stream cap, shared with
    /// whoever else plays from the peer. Wait and try again.
    Streams,
    /// "Daily transfer quota exceeded": nothing more from this peer until
    /// its day ends, 00:00 UTC.
    Quota,
    /// A 429 in other words (a proxy's, a newer server's).
    Other,
}

/// A 429, sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limited {
    pub kind: LimitKind,
    /// The server's Retry-After, in seconds; `None` when it sent none (the
    /// parent's proxy never does) or sent an HTTP date.
    pub retry_after: Option<Duration>,
    pub message: String,
}

/// Sort a 429 by its words: the peer says "Too many concurrent streams" for
/// its stream cap and "Daily transfer quota exceeded" for its day, and the
/// words survive the parent's proxy where Retry-After does not.
pub fn limit_of(message: &str, retry_after: Option<Duration>) -> Limited {
    let words = message.to_ascii_lowercase();
    let kind = if words.contains("concurrent stream") {
        LimitKind::Streams
    } else if words.contains("quota") {
        LimitKind::Quota
    } else {
        LimitKind::Other
    };
    Limited { kind, retry_after, message: message.to_string() }
}

#[derive(Debug)]
pub enum FetchError {
    /// Refused, or not reached: 401, 403 (a key that forbids copies too,
    /// [`FetchError::copies_refused`]), 404, 5xx, a dropped connection, or
    /// one that never came ([`ApiError::Network`]).
    Api(ApiError),
    /// A 429: a peer's stream cap or its daily quota.
    Limited(Limited),
    /// No byte came for this long.
    Stalled(Duration),
    /// The request's own ceiling passed with the body still coming.
    TooSlow(Duration),
    /// Not the bytes asked for: the server's default codec after its silent
    /// fallback, or a page in place of a file.
    WrongType { wanted: &'static str, got: Option<String> },
    /// The server's length for the file is not the one expected: the file
    /// changed since it was listed.
    WrongSize { expected: u64, got: u64 },
    /// The body ended before the length the file should have.
    Short { expected: u64, got: u64 },
    /// More bytes than the cap, or than the length expected.
    TooLong { cap: u64 },
    /// A ranged answer that is not the range asked for.
    Range(String),
    /// The file could not be written.
    Disk(String),
}

impl FetchError {
    /// The peer's key forbids copies, and the request said it was one.
    pub fn copies_refused(&self) -> bool {
        matches!(self, FetchError::Api(ApiError::Forbidden(words)) if words == COPIES_OFF)
    }

    pub fn limited(&self) -> Option<&Limited> {
        match self {
            FetchError::Limited(limited) => Some(limited),
            _ => None,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Api(e) => write!(f, "{e}"),
            FetchError::Limited(l) => match l.retry_after {
                Some(wait) => {
                    write!(f, "limited by the server: {} (try again in {} s)", l.message, wait.as_secs())
                }
                None => write!(f, "limited by the server: {}", l.message),
            },
            FetchError::Stalled(d) => write!(f, "no data for {} s", d.as_secs()),
            FetchError::TooSlow(d) => write!(f, "still downloading after {} s", d.as_secs()),
            FetchError::WrongType { wanted, got } => {
                let got = got.as_deref().unwrap_or("no type");
                write!(f, "the server sent {got} where {wanted} was asked for")
            }
            FetchError::WrongSize { expected, got } => {
                write!(f, "the server's file is {got} bytes, not the {expected} it listed")
            }
            FetchError::Short { expected, got } => {
                write!(f, "the download ended at {got} of {expected} bytes")
            }
            FetchError::TooLong { cap } => write!(f, "the download ran past {cap} bytes"),
            FetchError::Range(what) => write!(f, "unexpected range answer: {what}"),
            FetchError::Disk(e) => write!(f, "could not write the file: {e}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// What a probe learned about a file without its body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probed {
    pub len: Option<u64>,
    pub validator: Option<String>,
    pub content_type: Option<String>,
}

/// How to ask whether a file is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeBy {
    /// HEAD: no body at all. The server's own `/media` and a peer reached
    /// directly answer it from the file's stat; a peer meters only GETs as
    /// heavy.
    Head,
    /// GET with `Range: bytes=0-0`: one byte. The way through a parent's
    /// stream proxy, which is registered for GET only: Express answers a
    /// HEAD there with the GET handler, which pulls the whole file over
    /// the peer link (ESTIMATED from mStream's `federation-stream.js`).
    FirstByte,
}

/// The bytes of a ranged read, and what the answer said of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ranged {
    pub bytes: Vec<u8>,
    /// The whole file's length, when the answer gave it.
    pub total: Option<u64>,
    pub validator: Option<String>,
    pub content_type: Option<String>,
}

// ── Headers ─────────────────────────────────────────────────────────────────

fn header(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn length(headers: &HeaderMap) -> Option<u64> {
    header(headers, CONTENT_LENGTH)?.parse().ok()
}

/// The ETag, else Last-Modified: either is what If-Range takes. Express's
/// ETag is a weak one; mStream's `send` compares If-Range by its text all
/// the same.
fn validator(headers: &HeaderMap) -> Option<String> {
    header(headers, ETAG).or_else(|| header(headers, LAST_MODIFIED))
}

/// `bytes 4-9/10` → (Some((4, 9)), Some(10)); `bytes */10` → (None, Some(10)).
fn content_range(headers: &HeaderMap) -> Option<(Option<(u64, u64)>, Option<u64>)> {
    let value = header(headers, CONTENT_RANGE)?;
    let rest = value.strip_prefix("bytes")?.trim();
    let (span, total) = rest.split_once('/')?;
    let total = total.trim().parse::<u64>().ok();
    let span = match span.trim() {
        "*" => None,
        span => {
            let (start, end) = span.split_once('-')?;
            Some((start.trim().parse().ok()?, end.trim().parse().ok()?))
        }
    };
    Some((span, total))
}

/// Retry-After in seconds. Its other form, an HTTP date, is read as none:
/// mStream sends seconds.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    header(headers, RETRY_AFTER)?.parse::<u64>().ok().map(Duration::from_secs)
}

/// A transport failure: the request's ceiling, or anything else that kept
/// the answer from arriving. A connection that never came is a server not
/// reached, its timeout too: reqwest raises the client's connect timeout
/// as a timeout, but a server asleep or behind a firewall that drops the
/// packets is no body coming slowly, and the plan's 2.16 retries the two
/// differently.
fn transport(e: reqwest::Error, ceiling: Duration) -> FetchError {
    if e.is_timeout() && !e.is_connect() {
        FetchError::TooSlow(ceiling)
    } else {
        FetchError::Api(ApiError::Network(e.to_string()))
    }
}

/// The destination, opened to write from `start`: from scratch at 0, else
/// cut back to `start` (anything past the part asked for goes) and wound
/// on to it.
fn open_at(dest: &Path, start: u64) -> std::io::Result<File> {
    if start == 0 {
        return File::create(dest);
    }
    let mut file = OpenOptions::new().write(true).open(dest)?;
    file.set_len(start)?;
    file.seek(SeekFrom::Start(start))?;
    Ok(file)
}

/// A writer that holds the first bytes of the file already and can be cut
/// back: a card's partial download (the card contract's 2.12.8), which a
/// later run takes up where an earlier one left it.
pub trait Rewind: Write {
    /// The answer is good, and its bytes are written from `start` on: the
    /// ones held before it kept, everything past it gone (0: the file starts
    /// over, it changed). `validator` is the answer's (its ETag, else its
    /// Last-Modified), which the bytes from `start` on come under. Called
    /// before the first of them; not at all when nothing is left to come.
    fn start_at(&mut self, start: u64, validator: Option<&str>) -> std::io::Result<()>;
}

/// Where a fetch's bytes go: [`Client::download_async`]'s file,
/// [`Client::stream_async`]'s writer, or [`Client::stream_part_async`]'s.
trait Dest {
    /// The bytes of the file there already.
    fn held(&self) -> u64;
    /// Whether the bytes there can be cut back, so that an answer from an
    /// earlier byte than the ones held can be taken.
    fn rewinds(&self) -> bool;
    /// Ready to write from `start`, under `validator`: everything past it
    /// gone.
    fn start_at(&mut self, start: u64, validator: Option<&str>) -> std::io::Result<()>;
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    /// The length after a write failed with `done` bytes counted.
    fn len_after(&self, done: u64) -> u64;
    /// The whole file landed.
    fn finish(&mut self) -> std::io::Result<()>;
}

/// A file on this computer, opened once the answer is known good.
struct ToFile<'d> {
    path: &'d Path,
    file: Option<File>,
}

impl Dest for ToFile<'_> {
    fn held(&self) -> u64 {
        std::fs::metadata(self.path).map(|m| m.len()).unwrap_or(0)
    }

    fn rewinds(&self) -> bool {
        true
    }

    fn start_at(&mut self, start: u64, _validator: Option<&str>) -> std::io::Result<()> {
        self.file = Some(open_at(self.path, start)?);
        Ok(())
    }

    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match &mut self.file {
            Some(file) => file.write_all(bytes),
            None => Err(std::io::Error::other("not opened")),
        }
    }

    fn len_after(&self, done: u64) -> u64 {
        self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(done, |m| m.len())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        match &self.file {
            Some(file) => file.sync_data(),
            None => Ok(()),
        }
    }
}

/// A writer that holds `held` bytes of the file already and is only ever
/// written on: a card's stage file, which takes the bytes once, as they come.
struct ToStream<'d> {
    into: &'d mut dyn Write,
    held: u64,
}

impl Dest for ToStream<'_> {
    fn held(&self) -> u64 {
        self.held
    }

    fn rewinds(&self) -> bool {
        false
    }

    fn start_at(&mut self, start: u64, _validator: Option<&str>) -> std::io::Result<()> {
        match start == self.held {
            true => Ok(()),
            false => Err(std::io::Error::other("a stream is never cut back")),
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.into.write_all(bytes)
    }

    fn len_after(&self, done: u64) -> u64 {
        done
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.into.flush()
    }
}

/// A [`Rewind`] that holds `held` bytes of the file already.
struct ToPart<'d> {
    into: &'d mut dyn Rewind,
    held: u64,
}

impl Dest for ToPart<'_> {
    fn held(&self) -> u64 {
        self.held
    }

    fn rewinds(&self) -> bool {
        true
    }

    fn start_at(&mut self, start: u64, validator: Option<&str>) -> std::io::Result<()> {
        self.into.start_at(start, validator)
    }

    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.into.write_all(bytes)
    }

    fn len_after(&self, done: u64) -> u64 {
        done
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.into.flush()
    }
}

// ── Fetching ────────────────────────────────────────────────────────────────

impl Client {
    /// The URL `wanted`'s bytes come from: `/media`, a peer client's stream
    /// proxy (which forwards Range and If-Range), or `/transcode`. The token
    /// is not in it: the transfer's requests carry it in the header.
    pub fn bytes_url(&self, wanted: &Wanted<'_>) -> Result<String, ApiError> {
        let server = self.server();
        let url = match (*wanted, self.peer) {
            (Wanted::File(path), Some(peer)) => urls::peer_media_url(&server, peer, path, None),
            (Wanted::File(path), None) => urls::media_url(&server, path, None),
            (Wanted::Converted { .. }, Some(_)) => {
                return Err(ApiError::Config(
                    "a federated peer has no /transcode: its files are converted on this computer"
                        .to_string(),
                ));
            }
            (Wanted::Converted { path, codec, bitrate }, None) => {
                urls::transfer_transcode_url(&server, path, codec, bitrate, None)
            }
        }
        .map_err(ApiError::Config)?;
        Ok(urls::with_local_token(url, self.local_token.as_deref()))
    }

    /// A refusal's body, read to [`WORDS_MAX`] and counted, as the client's
    /// error: a 429 sorted into a [`Limited`].
    async fn refusal(&self, mut resp: Response, path: &str, stall: Duration) -> FetchError {
        let status = resp.status();
        let wait = retry_after(resp.headers());
        let mut body = Vec::new();
        while body.len() < WORDS_MAX {
            match tokio::time::timeout(stall, resp.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    self.count(chunk.len());
                    body.extend_from_slice(&chunk);
                }
                _ => break,
            }
        }
        let text = String::from_utf8_lossy(&body);
        if status == StatusCode::TOO_MANY_REQUESTS {
            return FetchError::Limited(limit_of(&extract_error(&text), wait));
        }
        FetchError::Api(refused(status, &text, path).unwrap_or_else(|| ApiError::Server {
            status: status.as_u16(),
            message: extract_error(&text),
        }))
    }

    /// Stream `fetch` into `dest`, resuming the part already there when
    /// [`Fetch::resuming`] says it may. `progress` hears the file's length
    /// as it grows, and the whole length when known.
    ///
    /// Nothing at `dest` changes until the answer is known to be the right
    /// bytes: a refusal, a 429, a wrong type or a wrong length leaves it as
    /// it was. Once writing starts, a failure keeps what was written and
    /// says so ([`Failed::partial`]). A finished file is synced to disk.
    ///
    /// The file writes are plain blocking writes: the sync twin runs them
    /// on the caller's own thread.
    pub async fn download_async(
        &self,
        fetch: &Fetch<'_>,
        dest: &Path,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<Fetched, Failed> {
        self.fetch_into(fetch, &mut ToFile { path: dest, file: None }, progress).await
    }

    /// [`Client::download_async`] into a writer instead of a file: the bytes
    /// go into `into` as they come — a card's stage file, which so takes
    /// them once — after the `held` bytes of the file it holds already, from
    /// a try that was cut. Those are kept, and the rest fetched from there
    /// with Range, only under [`Fetch::resuming`]'s validator (If-Range). A
    /// writer is never cut back: an answer that would start the file over
    /// (it changed since, or a resume with no validator) fails with nothing
    /// written, and the caller starts again into a new writer. Nothing is
    /// flushed past `into.flush()`: syncing it is its owner's.
    pub async fn stream_async(
        &self,
        fetch: &Fetch<'_>,
        into: &mut dyn Write,
        held: u64,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<Fetched, Failed> {
        self.fetch_into(fetch, &mut ToStream { into, held }, progress).await
    }

    pub fn stream(
        &self,
        fetch: &Fetch<'_>,
        into: &mut dyn Write,
        held: u64,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<Fetched, Failed> {
        let streamed = crate::runtime::block_on(self.stream_async(fetch, into, held, progress));
        streamed.unwrap_or_else(|e| {
            let partial = Partial { len: held, validator: None };
            Err(Failed { error: FetchError::Api(ApiError::Network(e)), partial })
        })
    }

    /// [`Client::stream_async`] into a writer that can be cut back: a card's
    /// partial download, which holds `held` bytes of the file from a run
    /// before. They are kept, and the rest fetched with Range under
    /// [`Fetch::resuming`]'s validator (If-Range); an answer that starts the
    /// file over (it changed since, or there is no validator) is taken in
    /// the same request, the writer told first ([`Rewind::start_at`]), so a
    /// changed file costs no second ask. Nothing is flushed past
    /// `into.flush()`: syncing it is its owner's.
    pub async fn stream_part_async(
        &self,
        fetch: &Fetch<'_>,
        into: &mut dyn Rewind,
        held: u64,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<Fetched, Failed> {
        self.fetch_into(fetch, &mut ToPart { into, held }, progress).await
    }

    pub fn stream_part(
        &self,
        fetch: &Fetch<'_>,
        into: &mut dyn Rewind,
        held: u64,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<Fetched, Failed> {
        let streamed = crate::runtime::block_on(self.stream_part_async(fetch, into, held, progress));
        streamed.unwrap_or_else(|e| {
            let partial = Partial { len: held, validator: None };
            Err(Failed { error: FetchError::Api(ApiError::Network(e)), partial })
        })
    }

    async fn fetch_into<D: Dest>(
        &self,
        fetch: &Fetch<'_>,
        dest: &mut D,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<Fetched, Failed> {
        let on_disk = dest.held();
        // A partial is resumed only under the validator it was fetched with
        // (else a file changed since would be stitched from two), never past
        // the length the file should have, and never for a conversion.
        let fits = fetch.expected.is_none_or(|e| on_disk <= e);
        let mut from = match (fetch.wanted, fetch.resume) {
            (Wanted::File(_), Some(_)) if on_disk > 0 && fits => on_disk,
            _ => 0,
        };
        let validator_kept = fetch.resume.filter(|_| from > 0).map(str::to_string);
        let mut kept = Partial { len: on_disk, validator: validator_kept };
        let failed = |error: FetchError, partial: &Partial| Failed { error, partial: partial.clone() };
        let url = self.bytes_url(&fetch.wanted).map_err(|e| failed(FetchError::Api(e), &kept))?;
        let path = fetch.wanted.path();
        // At most twice: a resume the server cannot place starts over.
        let (mut resp, ceiling) = loop {
            let left = fetch.expected.map(|e| e.saturating_sub(from));
            let ceiling = fetch.ceiling.unwrap_or_else(|| ceiling_for(left));
            let mut req = self.transfer_request(Method::GET, &url).timeout(ceiling);
            if from > 0 {
                req = req.header(RANGE, format!("bytes={from}-"));
                if let Some(validator) = fetch.resume {
                    req = req.header(IF_RANGE, validator);
                }
            }
            let resp = match tokio::time::timeout(fetch.stall, req.send()).await {
                Err(_) => return Err(failed(FetchError::Stalled(fetch.stall), &kept)),
                Ok(Err(e)) => return Err(failed(transport(e, ceiling), &kept)),
                Ok(Ok(resp)) => resp,
            };
            if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE && from > 0 {
                // Nothing lies past what is here: it is the whole file, or
                // it is not part of this file and the fetch starts over.
                let headers = resp.headers().clone();
                let total = content_range(&headers).and_then(|(_, total)| total);
                let _ = self.refusal(resp, path, fetch.stall).await;
                if total == Some(from) && fetch.expected.is_none_or(|e| e == from) {
                    progress(from, Some(from));
                    return Ok(Fetched {
                        len: from,
                        resumed_from: from,
                        validator: validator(&headers).or(kept.validator.clone()),
                        content_type: header(&headers, CONTENT_TYPE),
                    });
                }
                // What is here is no part of the file any more.
                from = 0;
                kept.validator = None;
                continue;
            }
            break (resp, ceiling);
        };
        let status = resp.status();
        if !status.is_success() {
            return Err(failed(self.refusal(resp, path, fetch.stall).await, &kept));
        }
        let headers = resp.headers().clone();
        // Where this answer starts, and the whole file's length by it. A 200
        // is the whole file from its first byte: the file changed since the
        // partial (If-Range said so), or the server ignored the range.
        let (start, total) = if status == StatusCode::PARTIAL_CONTENT {
            match content_range(&headers) {
                Some((Some((start, _)), total)) if from > 0 => (start, total),
                _ => {
                    let error = FetchError::Range("a 206 that is not the part asked for".into());
                    return Err(failed(error, &kept));
                }
            }
        } else {
            // The part here, if any, is of a file the server no longer has.
            kept.validator = None;
            (0, length(&headers))
        };
        if start != 0 && start != from {
            let error = FetchError::Range(format!("asked from byte {from}, answered from {start}"));
            return Err(failed(error, &kept));
        }
        if let (Some(expected), Some(total)) = (fetch.expected, total)
            && expected != total
        {
            return Err(failed(FetchError::WrongSize { expected, got: total }, &kept));
        }
        let limit = fetch.expected.unwrap_or(fetch.cap).min(fetch.cap);
        if total.is_some_and(|t| t > limit) {
            return Err(failed(FetchError::TooLong { cap: limit }, &kept));
        }
        let content_type = header(&headers, CONTENT_TYPE);
        if let Wanted::Converted { codec, .. } = fetch.wanted
            && !content_type.as_deref().is_some_and(|t| codec.answers(t))
        {
            let error = FetchError::WrongType { wanted: codec.content_type(), got: content_type };
            return Err(failed(error, &kept));
        }

        // The answer is the right one; the file is written from here.
        let resumable = matches!(fetch.wanted, Wanted::File(_));
        let written_under = if !resumable {
            None
        } else if start > 0 {
            validator(&headers).or(kept.validator.clone())
        } else {
            validator(&headers)
        };
        let at = |len: u64| Partial { len, validator: written_under.clone().filter(|_| len > 0) };
        if start < on_disk && !dest.rewinds() {
            let error = FetchError::Range(format!(
                "asked from byte {from}, answered from {start}: the file changed since its first \
                 {on_disk} bytes came"
            ));
            return Err(failed(error, &kept));
        }
        let starting = dest.start_at(start, written_under.as_deref());
        starting.map_err(|e| failed(FetchError::Disk(e.to_string()), &kept))?;
        let total = total.or(fetch.expected);
        let mut done = start;
        loop {
            let chunk = match tokio::time::timeout(fetch.stall, resp.chunk()).await {
                Err(_) => {
                    return Err(Failed { error: FetchError::Stalled(fetch.stall), partial: at(done) });
                }
                Ok(Err(e)) => return Err(Failed { error: transport(e, ceiling), partial: at(done) }),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(chunk))) => chunk,
            };
            self.count(chunk.len());
            if done + chunk.len() as u64 > limit {
                return Err(Failed { error: FetchError::TooLong { cap: limit }, partial: at(done) });
            }
            if let Err(e) = dest.write_all(&chunk) {
                let len = dest.len_after(done);
                return Err(Failed { error: FetchError::Disk(e.to_string()), partial: at(len) });
            }
            done += chunk.len() as u64;
            progress(done, total);
        }
        if let Some(expected) = total
            && done != expected
        {
            return Err(Failed { error: FetchError::Short { expected, got: done }, partial: at(done) });
        }
        if let Err(e) = dest.finish() {
            return Err(Failed { error: FetchError::Disk(e.to_string()), partial: at(done) });
        }
        Ok(Fetched { len: done, resumed_from: start, validator: written_under, content_type })
    }

    pub fn download(
        &self,
        fetch: &Fetch<'_>,
        dest: &Path,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<Fetched, Failed> {
        crate::runtime::block_on(self.download_async(fetch, dest, progress)).unwrap_or_else(|e| {
            let len = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            let partial = Partial { len, validator: None };
            Err(Failed { error: FetchError::Api(ApiError::Network(e)), partial })
        })
    }

    /// `len` bytes of a file from `start` (`Range: bytes=start-end`), and
    /// what the answer said of the whole: the two 4 KiB reads of a quick
    /// fingerprint, without the file. Shorter at the file's end; empty past
    /// it. A server that ignores the range is read for the first `len`
    /// bytes when `start` is 0, and is an error otherwise: the rest of its
    /// answer is never read.
    pub async fn read_range_async(
        &self,
        path: &str,
        start: u64,
        len: u64,
    ) -> Result<Ranged, FetchError> {
        if len == 0 {
            return Ok(Ranged::default());
        }
        let url = self.bytes_url(&Wanted::File(path)).map_err(FetchError::Api)?;
        let req = self
            .transfer_request(Method::GET, &url)
            .timeout(PROBE_CEILING)
            .header(RANGE, format!("bytes={start}-{}", start + len - 1));
        let mut resp = match tokio::time::timeout(STALL, req.send()).await {
            Err(_) => return Err(FetchError::Stalled(STALL)),
            Ok(Err(e)) => return Err(transport(e, PROBE_CEILING)),
            Ok(Ok(resp)) => resp,
        };
        let headers = resp.headers().clone();
        let described = |bytes: Vec<u8>, total: Option<u64>| Ranged {
            bytes,
            total,
            validator: validator(&headers),
            content_type: header(&headers, CONTENT_TYPE),
        };
        let total = match resp.status() {
            StatusCode::PARTIAL_CONTENT => match content_range(&headers) {
                Some((Some((at, _)), total)) if at == start => total,
                _ => return Err(FetchError::Range(format!("not the part from byte {start}"))),
            },
            StatusCode::OK if start == 0 => length(&headers),
            StatusCode::OK => {
                return Err(FetchError::Range("the whole file, where a part was asked for".into()));
            }
            StatusCode::RANGE_NOT_SATISFIABLE => {
                let total = content_range(&headers).and_then(|(_, total)| total);
                let _ = self.refusal(resp, path, STALL).await;
                return Ok(described(Vec::new(), total));
            }
            _ => return Err(self.refusal(resp, path, STALL).await),
        };
        let mut bytes = Vec::new();
        while (bytes.len() as u64) < len {
            match tokio::time::timeout(STALL, resp.chunk()).await {
                Err(_) => return Err(FetchError::Stalled(STALL)),
                Ok(Err(e)) => return Err(transport(e, PROBE_CEILING)),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(chunk))) => {
                    self.count(chunk.len());
                    bytes.extend_from_slice(&chunk);
                }
            }
        }
        bytes.truncate(len as usize);
        Ok(described(bytes, total))
    }

    pub fn read_range(&self, path: &str, start: u64, len: u64) -> Result<Ranged, FetchError> {
        crate::runtime::block_on(self.read_range_async(path, start, len))
            .map_err(|e| FetchError::Api(ApiError::Network(e)))?
    }

    /// Whether a file is there, and its length, without its body: the
    /// named folder images a cover search tries (mStream lists no images).
    /// `None` for a 404.
    pub async fn probe_async(&self, path: &str, by: ProbeBy) -> Result<Option<Probed>, FetchError> {
        if by == ProbeBy::FirstByte {
            return match self.read_range_async(path, 0, 1).await {
                Ok(read) => Ok(Some(Probed {
                    len: read.total,
                    validator: read.validator,
                    content_type: read.content_type,
                })),
                Err(FetchError::Api(ApiError::NotFound(_))) => Ok(None),
                Err(e) => Err(e),
            };
        }
        let url = self.bytes_url(&Wanted::File(path)).map_err(FetchError::Api)?;
        let req = self.transfer_request(Method::HEAD, &url).timeout(PROBE_CEILING);
        let resp = match tokio::time::timeout(STALL, req.send()).await {
            Err(_) => return Err(FetchError::Stalled(STALL)),
            Ok(Err(e)) => return Err(transport(e, PROBE_CEILING)),
            Ok(Ok(resp)) => resp,
        };
        match resp.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => {
                let headers = resp.headers();
                Ok(Some(Probed {
                    len: length(headers),
                    validator: validator(headers),
                    content_type: header(headers, CONTENT_TYPE),
                }))
            }
            _ => Err(self.refusal(resp, path, STALL).await),
        }
    }

    pub fn probe(&self, path: &str, by: ProbeBy) -> Result<Option<Probed>, FetchError> {
        crate::runtime::block_on(self.probe_async(path, by))
            .map_err(|e| FetchError::Api(ApiError::Network(e)))?
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::stub::{Step, canned, chunked, json, reply, scripted};
    use super::*;

    /// A temp directory of the test's own, gone when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("mstream-player-fetch-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn file(&self, name: &str, bytes: Option<&[u8]>) -> PathBuf {
            let path = self.0.join(name);
            if let Some(bytes) = bytes {
                std::fs::write(&path, bytes).unwrap();
            }
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn recv(seen: &std::sync::mpsc::Receiver<String>) -> String {
        seen.recv_timeout(Duration::from_secs(5)).expect("the request reached the stub").to_ascii_lowercase()
    }

    #[test]
    fn a_conversion_streams_chunked_into_the_file_and_its_type_is_checked() {
        let scratch = Scratch::new("convert");
        let dest = scratch.file("0001.tmp", None);
        let (base, seen) = canned(vec![
            chunked("200 OK", &[("Content-Type", "audio/ogg")], &[b"OggS", b"page2", b"end"]),
            // The server's silent fallback: its default codec, not the one asked.
            chunked("200 OK", &[("Content-Type", "audio/mpeg")], &[b"ID3"]),
        ]);
        let meter = Arc::new(AtomicU64::new(0));
        let client = Client::new(&base).unwrap().with_token(Some("tok".into())).with_meter(Some(meter.clone()));
        let fetch = Fetch::converted("lib/Artist A/01.flac", TransferCodec::Opus, TransferBitrate::K192);
        let mut heard = Vec::new();
        let got = client.download(&fetch, &dest, &mut |done, total| heard.push((done, total))).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"OggSpage2end");
        let want = Fetched { len: 12, resumed_from: 0, validator: None, content_type: Some("audio/ogg".into()) };
        assert_eq!(got, want);
        assert_eq!(heard.last(), Some(&(12, None)), "no length to tell: a conversion streams without one");
        assert_eq!(meter.load(Ordering::Relaxed), 12);
        let req = recv(&seen);
        let line = "get /transcode/lib/artist%20a/01.flac?codec=opus&bitrate=192k http/1.1\r\n";
        assert!(req.starts_with(line), "{req}");
        assert!(req.contains("x-access-token: tok\r\n"), "{req}");
        assert!(!req.contains("token=tok"), "the token rides the header, not the URL");
        assert!(!req.contains("\r\nrange:"), "a conversion has no ranges to resume by");

        let Err(failed) = client.download(&fetch, &dest, &mut |_, _| {}) else {
            panic!("the wrong codec came back")
        };
        match failed.error {
            FetchError::WrongType { wanted, got } => {
                assert_eq!((wanted, got.as_deref()), ("audio/ogg", Some("audio/mpeg")));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"OggSpage2end", "untouched until the answer is right");
    }

    #[test]
    fn a_partial_resumes_with_if_range_and_a_changed_file_starts_over() {
        let scratch = Scratch::new("resume");
        let dest = scratch.file("0002.tmp", Some(b"0123"));
        let etag = "W/\"a-18f\"";
        let (base, seen) = canned(vec![
            reply("206 Partial Content", &[("Content-Range", "bytes 4-9/10"), ("ETag", etag)], b"456789"),
            // The file changed: If-Range fails and the whole new file comes.
            reply("200 OK", &[("ETag", "W/\"a-190\"")], b"abcdefghij"),
            // All of it is here already.
            reply("416 Range Not Satisfiable", &[("Content-Range", "bytes */10")], b""),
            reply("200 OK", &[("ETag", etag)], b"0123456789"),
            // A partial longer than the file is no part of it: once more,
            // from the start.
            reply("416 Range Not Satisfiable", &[("Content-Range", "bytes */10")], b""),
            reply("200 OK", &[("ETag", etag)], b"0123456789"),
            // Changed, and grown past what the listing said.
            reply("200 OK", &[("ETag", "W/\"c-1\"")], b"0123456789ab"),
        ]);
        let client = Client::new(&base).unwrap();
        let mut heard = Vec::new();
        let fetch = Fetch::file("lib/Album B/01.flac").expecting(10).resuming(Some(etag));
        let got = client.download(&fetch, &dest, &mut |done, total| heard.push((done, total))).unwrap();
        let req = recv(&seen);
        assert!(req.starts_with("get /media/lib/album%20b/01.flac http/1.1\r\n"), "{req}");
        assert!(req.contains("range: bytes=4-\r\n") && req.contains("if-range: w/\"a-18f\"\r\n"), "{req}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123456789");
        assert_eq!((got.len, got.resumed_from, got.validator.as_deref()), (10, 4, Some(etag)));
        assert_eq!(heard.last(), Some(&(10, Some(10))), "the whole length, resumed part included");

        std::fs::write(&dest, b"0123").unwrap();
        let got = client.download(&fetch, &dest, &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"abcdefghij", "the old part is gone, not stitched");
        assert_eq!((got.resumed_from, got.validator.as_deref()), (0, Some("W/\"a-190\"")));
        let _ = recv(&seen);

        std::fs::write(&dest, b"0123456789").unwrap();
        let got = client.download(&fetch, &dest, &mut |_, _| {}).unwrap();
        assert_eq!((got.len, got.resumed_from), (10, 10), "a 416 over the whole file is a finished one");
        assert!(recv(&seen).contains("range: bytes=10-\r\n"));

        // No validator, no resume: the bytes there cannot be vouched for.
        std::fs::write(&dest, b"0123").unwrap();
        let fresh = Fetch::file("lib/Album B/01.flac").expecting(10);
        let got = client.download(&fresh, &dest, &mut |_, _| {}).unwrap();
        assert_eq!((got.len, got.resumed_from), (10, 0));
        assert!(!recv(&seen).contains("\r\nrange:"));

        std::fs::write(&dest, b"0123456789XY").unwrap();
        let fetch = Fetch::file("lib/Album B/01.flac").resuming(Some(etag));
        let got = client.download(&fetch, &dest, &mut |_, _| {}).unwrap();
        assert_eq!((got.len, got.resumed_from), (10, 0));
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123456789");
        assert!(recv(&seen).contains("range: bytes=12-\r\n"));
        assert!(!recv(&seen).contains("\r\nrange:"), "the second ask is for the whole file");

        std::fs::write(&dest, b"0123").unwrap();
        let fetch = Fetch::file("lib/Album B/01.flac").expecting(10).resuming(Some(etag));
        let failed = client.download(&fetch, &dest, &mut |_, _| {}).unwrap_err();
        assert!(matches!(failed.error, FetchError::WrongSize { expected: 10, got: 12 }), "{:?}", failed.error);
        assert_eq!(failed.partial, Partial { len: 4, validator: None }, "a part of a file that is gone");
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123", "and still untouched");
    }

    #[test]
    fn a_stream_takes_the_bytes_once_and_resumes_only_where_it_stopped() {
        let etag = "W/\"a-18f\"";
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: {etag}\r\n");
        let cut = format!("{head}Connection: close\r\n\r\n0123");
        let (base, seen) = canned(vec![
            // Cut after four bytes: what came is in the stream, and says so.
            cut.into_bytes(),
            reply("206 Partial Content", &[("Content-Range", "bytes 4-9/10"), ("ETag", etag)], b"456789"),
            // The file changed since: the whole new one, which the stream
            // can't take on top of the old part.
            reply("200 OK", &[("ETag", "W/\"a-190\"")], b"abcdefghij"),
            reply("200 OK", &[("ETag", etag)], b"0123456789"),
        ]);
        let client = Client::new(&base).unwrap();
        let mut into: Vec<u8> = Vec::new();
        let fetch = Fetch::file("lib/Album B/01.flac").expecting(10);
        let failed = client.stream(&fetch, &mut into, 0, &mut |_, _| {}).unwrap_err();
        assert_eq!(failed.partial, Partial { len: 4, validator: Some(etag.into()) }, "{}", failed.error);
        assert_eq!(into, b"0123");
        let _ = recv(&seen);

        let resume = Fetch::file("lib/Album B/01.flac").expecting(10).resuming(Some(etag));
        let mut heard = Vec::new();
        let got = client.stream(&resume, &mut into, 4, &mut |done, total| heard.push((done, total))).unwrap();
        let req = recv(&seen);
        assert!(req.contains("range: bytes=4-\r\n") && req.contains("if-range: w/\"a-18f\"\r\n"), "{req}");
        assert_eq!(into, b"0123456789", "written on, never over");
        assert_eq!((got.len, got.resumed_from, got.validator.as_deref()), (10, 4, Some(etag)));
        assert_eq!(heard.last(), Some(&(10, Some(10))));

        let mut into = b"0123".to_vec();
        let failed = client.stream(&resume, &mut into, 4, &mut |_, _| {}).unwrap_err();
        assert!(matches!(&failed.error, FetchError::Range(why) if why.contains("changed")), "{:?}", failed.error);
        assert_eq!(into, b"0123", "nothing written over the old part");
        let _ = recv(&seen);

        // A fresh stream takes the whole file.
        let mut into = Vec::new();
        let got = client.stream(&fetch, &mut into, 0, &mut |_, _| {}).unwrap();
        assert_eq!((into.as_slice(), got.resumed_from), (b"0123456789".as_slice(), 0));
    }

    /// A card's partial, in memory: its bytes, and each start it was told.
    #[derive(Default)]
    struct Part {
        bytes: Vec<u8>,
        starts: Vec<(u64, Option<String>)>,
    }

    impl Write for Part {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Rewind for Part {
        fn start_at(&mut self, start: u64, validator: Option<&str>) -> std::io::Result<()> {
            self.bytes.truncate(start as usize);
            self.starts.push((start, validator.map(String::from)));
            Ok(())
        }
    }

    #[test]
    fn a_partial_is_taken_up_and_a_changed_file_starts_it_over_in_one_answer() {
        let (etag, date) = ("W/\"a-18f\"", "Mon, 21 Sep 2026 14:13:20 GMT");
        let (base, seen) = canned(vec![
            reply("206 Partial Content", &[("Content-Range", "bytes 4-9/10"), ("ETag", etag)], b"456789"),
            // The file changed: the whole new one, in the same answer.
            reply("200 OK", &[("ETag", "W/\"a-190\"")], b"abcdefghij"),
            // All of it there: nothing to start.
            reply("416 Range Not Satisfiable", &[("Content-Range", "bytes */10")], b""),
            // Taken up by the date of a file that had no ETag.
            reply("206 Partial Content", &[("Content-Range", "bytes 4-9/10")], b"456789"),
        ]);
        let client = Client::new(&base).unwrap();
        let fetch = Fetch::file("lib/Album B/01.flac").expecting(10).resuming(Some(etag));
        let mut part = Part { bytes: b"0123".to_vec(), ..Part::default() };
        let got = client.stream_part(&fetch, &mut part, 4, &mut |_, _| {}).unwrap();
        let req = recv(&seen);
        assert!(req.contains("range: bytes=4-\r\n") && req.contains("if-range: w/\"a-18f\"\r\n"), "{req}");
        assert_eq!((part.bytes.as_slice(), got.resumed_from), (b"0123456789".as_slice(), 4));
        assert_eq!(part.starts, [(4, Some(etag.to_string()))]);

        let mut part = Part { bytes: b"0123".to_vec(), ..Part::default() };
        let got = client.stream_part(&fetch, &mut part, 4, &mut |_, _| {}).unwrap();
        assert_eq!(part.bytes, b"abcdefghij", "the old part is gone, not stitched");
        assert_eq!(part.starts, [(0, Some("W/\"a-190\"".to_string()))], "told before the new bytes");
        assert_eq!((got.resumed_from, got.validator.as_deref()), (0, Some("W/\"a-190\"")));
        let _ = recv(&seen);

        let mut part = Part { bytes: b"0123456789".to_vec(), ..Part::default() };
        let got = client.stream_part(&fetch, &mut part, 10, &mut |_, _| {}).unwrap();
        assert_eq!((got.len, got.resumed_from, part.starts.len()), (10, 10, 0), "whole already");
        assert!(recv(&seen).contains("range: bytes=10-\r\n"));

        let by_date = Fetch::file("lib/Album B/01.flac").expecting(10).resuming(Some(date));
        let mut part = Part { bytes: b"0123".to_vec(), ..Part::default() };
        client.stream_part(&by_date, &mut part, 4, &mut |_, _| {}).unwrap();
        let req = recv(&seen);
        assert!(req.contains("if-range: mon, 21 sep 2026 14:13:20 gmt\r\n"), "{req}");
        assert_eq!(part.starts, [(4, Some(date.to_string()))], "no ETag: the date it was asked by");
        assert_eq!(part.bytes, b"0123456789");
    }

    #[test]
    fn both_429s_are_told_apart_by_their_words_and_leave_the_partial_alone() {
        let scratch = Scratch::new("limits");
        let dest = scratch.file("0003.tmp", Some(b"0123"));
        let (base, _seen) = canned(vec![
            // A peer reached directly: its own words and its Retry-After.
            reply("429 Too Many Requests", &[("Retry-After", "5"), ("Content-Type", "application/json")],
                br#"{"error":"Too many concurrent streams"}"#),
            reply(
                "429 Too Many Requests",
                &[("Retry-After", "3600")],
                br#"{"error":"Daily transfer quota exceeded"}"#,
            ),
            // Through the parent's proxy: the words come, Retry-After does not.
            json("429 Too Many Requests", r#"{"error":"Daily transfer quota exceeded"}"#),
            json("429 Too Many Requests", r#"{"error":"Too many concurrent streams"}"#),
            json("403 Forbidden", r#"{"error":"copies are not allowed with this key"}"#),
        ]);
        let client = Client::new(&base).unwrap().copying(true);
        let fetch = Fetch::file("lib/a.flac").expecting(10).resuming(Some("\"e1\""));
        let next = || client.download(&fetch, &dest, &mut |_, _| {}).unwrap_err();

        let failed = next();
        let limited = failed.error.limited().expect("a 429");
        assert_eq!((limited.kind, limited.retry_after), (LimitKind::Streams, Some(Duration::from_secs(5))));
        assert_eq!(failed.partial, Partial { len: 4, validator: Some("\"e1\"".into()) }, "still resumable");
        let failed = next();
        let limited = failed.error.limited().unwrap();
        assert_eq!((limited.kind, limited.retry_after), (LimitKind::Quota, Some(Duration::from_secs(3600))));
        let failed = next();
        let limited = failed.error.limited().unwrap();
        assert_eq!((limited.kind, limited.retry_after), (LimitKind::Quota, None));
        let failed = next();
        assert_eq!(failed.error.limited().map(|l| (l.kind, l.retry_after)), Some((LimitKind::Streams, None)));
        let failed = next();
        assert!(failed.error.copies_refused(), "{:?}", failed.error);
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123");

        assert_eq!(limit_of("Slow down", None).kind, LimitKind::Other);
    }

    #[test]
    fn a_download_that_stalls_or_crawls_fails_and_keeps_what_came() {
        let scratch = Scratch::new("stall");
        let dest = scratch.file("0004.tmp", None);
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nETag: \"s1\"\r\nConnection: close\r\n\r\n";
        let mut first = head.to_vec();
        first.extend_from_slice(b"0123456789");
        let mut crawl = vec![Step::Send(head.to_vec())];
        for _ in 0..30 {
            crawl.push(Step::Send(b"x".to_vec()));
            crawl.push(Step::Wait(Duration::from_millis(100)));
        }
        // A server each: one connection at a time, and the first holds its
        // socket in silence.
        let (silent, _seen) = scripted(vec![vec![Step::Send(first), Step::Wait(Duration::from_secs(4))]]);
        let (slow, _seen) = scripted(vec![crawl]);
        let client = Client::new(&silent).unwrap();

        let t0 = std::time::Instant::now();
        let fetch = Fetch::file("lib/a.flac").expecting(100).stall(Duration::from_millis(300));
        let failed = client.download(&fetch, &dest, &mut |_, _| {}).unwrap_err();
        assert!(matches!(failed.error, FetchError::Stalled(_)), "{:?}", failed.error);
        assert_eq!(failed.partial, Partial { len: 10, validator: Some("\"s1\"".into()) });
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123456789", "the part that came is kept, to resume");
        assert!(t0.elapsed() < Duration::from_secs(3), "given up after the stall, not the server's silence");

        // Bytes keep coming, too slowly for the request's own ceiling.
        let client = Client::new(&slow).unwrap();
        let fetch = Fetch::file("lib/a.flac").stall(Duration::from_secs(2)).ceiling(Duration::from_millis(700));
        let failed = client.download(&fetch, &dest, &mut |_, _| {}).unwrap_err();
        assert!(matches!(failed.error, FetchError::TooSlow(_)), "{:?}", failed.error);
        assert!(failed.partial.len > 0 && failed.partial.len < 30, "{:?}", failed.partial);
    }

    #[test]
    fn a_length_other_than_the_listings_is_refused_and_a_short_body_is_caught() {
        let scratch = Scratch::new("sizes");
        let dest = scratch.file("0005.tmp", None);
        let (base, _seen) = canned(vec![
            reply("200 OK", &[], b"0123456789ab"),
            chunked("200 OK", &[], &[b"012345"]),
            chunked("200 OK", &[("Content-Type", "audio/ogg")], &[b"OggS", b"more"]),
        ]);
        let client = Client::new(&base).unwrap();
        let failed = client.download(&Fetch::file("lib/a.flac").expecting(10), &dest, &mut |_, _| {}).unwrap_err();
        assert!(matches!(failed.error, FetchError::WrongSize { expected: 10, got: 12 }), "{:?}", failed.error);
        assert!(!dest.exists(), "refused before a byte was written");

        let failed = client.download(&Fetch::file("lib/a.flac").expecting(10), &dest, &mut |_, _| {}).unwrap_err();
        assert!(matches!(failed.error, FetchError::Short { expected: 10, got: 6 }), "{:?}", failed.error);
        assert_eq!(failed.partial.len, 6);

        let fetch = Fetch::converted("lib/a.flac", TransferCodec::Opus, TransferBitrate::K96).cap(5);
        let failed = client.download(&fetch, &dest, &mut |_, _| {}).unwrap_err();
        assert!(matches!(failed.error, FetchError::TooLong { cap: 5 }), "{:?}", failed.error);
        assert_eq!(failed.partial, Partial { len: 4, validator: None }, "a conversion's part is never resumed");
    }

    #[test]
    fn a_probe_asks_by_head_or_by_one_byte() {
        let (base, seen) = canned(vec![
            // A HEAD's answer: the length, and no body.
            b"HTTP/1.1 200 OK\r\nContent-Length: 1234\r\nContent-Type: image/jpeg\r\nETag: \"c1\"\r\n\
              Connection: close\r\n\r\n"
                .to_vec(),
            reply("206 Partial Content", &[("Content-Range", "bytes 0-0/5678"), ("ETag", "\"c2\"")], b"x"),
            reply("404 Not Found", &[], b""),
            reply("404 Not Found", &[], b""),
        ]);
        let meter = Arc::new(AtomicU64::new(0));
        let client = Client::new(&base).unwrap().with_meter(Some(meter.clone()));
        let got = client.probe("lib/Album B/cover.jpg", ProbeBy::Head).unwrap().expect("there");
        assert_eq!((got.len, got.validator.as_deref()), (Some(1234), Some("\"c1\"")));
        assert_eq!(got.content_type.as_deref(), Some("image/jpeg"));
        assert!(recv(&seen).starts_with("head /media/lib/album%20b/cover.jpg http/1.1\r\n"));

        let peer = Client::new(&base).unwrap().with_meter(Some(meter.clone())).with_peer(Some(3));
        let got = peer.probe("lib/Album B/folder.jpg", ProbeBy::FirstByte).unwrap().expect("there");
        assert_eq!(got.len, Some(5678), "the whole length, from Content-Range");
        let req = recv(&seen);
        let line = "get /api/v1/federation/peers/3/stream/lib/album%20b/folder.jpg http/1.1\r\n";
        assert!(req.starts_with(line), "{req}");
        assert!(req.contains("range: bytes=0-0\r\n"), "{req}");
        assert_eq!(meter.load(Ordering::Relaxed), 1, "one byte crossed, and a HEAD none");

        assert_eq!(client.probe("lib/Album B/front.jpg", ProbeBy::Head).unwrap(), None);
        assert_eq!(peer.probe("lib/Album B/front.jpg", ProbeBy::FirstByte).unwrap(), None);
    }

    #[test]
    fn a_ranged_read_takes_its_bytes_and_no_more() {
        let block = vec![7u8; 4096];
        let (base, seen) = canned(vec![
            reply("206 Partial Content", &[("Content-Range", "bytes 8192-12287/100000")], &block),
            // A server that ignores ranges: only the first bytes are kept.
            reply("200 OK", &[], &[1u8; 9000]),
            reply("200 OK", &[], &[1u8; 10]),
            reply("416 Range Not Satisfiable", &[("Content-Range", "bytes */100")], b""),
        ]);
        let client = Client::new(&base).unwrap();
        let got = client.read_range("lib/a.flac", 8192, 4096).unwrap();
        assert_eq!((got.bytes.len(), got.total), (4096, Some(100000)));
        assert!(recv(&seen).contains("range: bytes=8192-12287\r\n"));
        let got = client.read_range("lib/a.flac", 0, 4096).unwrap();
        assert_eq!((got.bytes.len(), got.total), (4096, Some(9000)));
        assert!(matches!(client.read_range("lib/a.flac", 4096, 4), Err(FetchError::Range(_))));
        let got = client.read_range("lib/a.flac", 200, 16).unwrap();
        assert_eq!((got.bytes.len(), got.total), (0, Some(100)), "past the end: nothing, and the length");
    }

    #[test]
    fn a_peers_bytes_come_through_its_parents_proxy_and_it_has_no_transcode() {
        let parent = Client::new("http://parent:3000/").unwrap().with_token(Some("pt".into())).with_peer(Some(3));
        assert_eq!(
            parent.bytes_url(&Wanted::File("lib/Söng.flac")).unwrap(),
            "http://parent:3000/api/v1/federation/peers/3/stream/lib/S%C3%B6ng.flac"
        );
        let converted =
            Wanted::Converted { path: "lib/a.flac", codec: TransferCodec::Opus, bitrate: TransferBitrate::K192 };
        assert!(matches!(parent.bytes_url(&converted), Err(ApiError::Config(_))));
        let bridge = Client::new("http://127.0.0.1:4242").unwrap().with_local_token(Some("lt".into()));
        let url = bridge.bytes_url(&Wanted::File("lib/a.flac")).unwrap();
        assert_eq!(url, "http://127.0.0.1:4242/media/lib/a.flac?__lt=lt", "a bridge's own token rides along");

        assert_eq!(ceiling_for(None), UNSIZED_CEILING);
        assert_eq!(ceiling_for(Some(0)), CEILING_BASE);
        let four_gb = ceiling_for(Some(FAT32_MAX));
        let hours = four_gb.as_secs() / 3600;
        assert!((36..40).contains(&hours), "FAT32's largest file at the floor rate: {four_gb:?}");
    }

    /// A name that never resolves: no connection ever comes, as from a
    /// server asleep or behind a firewall that drops the packets, and only
    /// the client's connect timeout ends the wait. Nothing leaves the
    /// machine.
    struct NoAnswer;

    impl reqwest::dns::Resolve for NoAnswer {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(std::future::pending())
        }
    }

    #[test]
    fn a_connection_that_never_comes_is_a_server_not_reached_not_a_slow_body() {
        let scratch = Scratch::new("unreached");
        let dest = scratch.file("0006.tmp", None);
        let mut client = Client::new("http://asleep.invalid/").unwrap();
        // The client's own, with a connect timeout short enough for a test.
        client.http = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(200))
            .dns_resolver(NoAnswer)
            .build()
            .unwrap();
        let unreached = |e: &FetchError| matches!(e, FetchError::Api(ApiError::Network(_)));

        let t0 = std::time::Instant::now();
        let failed = client.download(&Fetch::file("lib/a.flac").expecting(10), &dest, &mut |_, _| {}).unwrap_err();
        assert!(unreached(&failed.error), "not still downloading: {:?}", failed.error);
        assert!(t0.elapsed() < Duration::from_secs(5), "the connect timeout, not the stall's 30 s");
        assert_eq!(failed.partial, Partial::default());
        assert!(!dest.exists(), "nothing was asked, nothing written");

        let probed = client.probe("lib/cover.jpg", ProbeBy::Head).unwrap_err();
        assert!(unreached(&probed), "{probed:?}");
        let read = client.read_range("lib/a.flac", 0, 4096).unwrap_err();
        assert!(unreached(&read), "{read:?}");
    }
}
