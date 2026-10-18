//! The user's own server over HTTP (the transfer plan's 2.4, 2.5): the
//! engine's [`Fetcher`] for a sync from it, beneath [`Local`](super::local)
//! when this computer is the server's.
//!
//! **The server's bytes** come from `/media`, streamed straight into the
//! card's stage file as they arrive (A6's [`Client::stream`]): the card
//! takes them once. A connection cut partway is taken up where it stopped,
//! with Range under the ETag the first answer gave (If-Range), three tries
//! in all; a file changed meanwhile, or gone, is skipped for this run.
//!
//! **A server whose files a later run takes up** ([`Http::resuming`]: a
//! peer's, under its daily quota) has them streamed into the card's partial
//! instead (2.12.8, A6's [`Client::stream_part`]): a listed one taken up
//! under the validator its bytes came with, a changed file started over in
//! the same answer, and what came kept for the next run when the fetch
//! stops short. The server's own files simply start over.
//!
//! **A conversion** comes from `/transcode`, into a file on this computer
//! first: it streams with no length and no ranges, and a 200 that ends early
//! (the server's ffmpeg died) looks like any other, so it is held to its
//! source's length ([`length`](super::length)) and made again when it falls
//! short — three tries, or two whole ones that agree with each other but not
//! with the manifest, and it is skipped. Its Content-Type must be the codec
//! asked for (mStream falls back to its own default silently). Then what
//! the conversion dropped is put back ([`Finish`], B1's retag) before the
//! card's one write. The server's `/transcode` has no cap of its own and a
//! bulk job evicts its listeners' cache, so at most two conversions run at a
//! time in this process ([`TRANSCODES`]); a run's conversions are made ahead
//! of the engine, in its order, two at a time ([`Http::ahead`]), so the card
//! is written while the server encodes.
//!
//! Whether a failure stops the run or skips one file ([`Failed::stop`]):
//! lost sign-in, a server out of reach after three tries, and a 429 stop it
//! (it parks at a checkpoint, 2.12.8); anything of one file's — gone,
//! changed, refused, a wrong type, a conversion that won't come whole —
//! skips that file, which the next run tries again.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::super::engine::{Failed, Fetched, Fetcher, Have, Part, WantedFile};
use super::super::names::Bytes;
use super::super::plan::codec_of;
use super::length::{self, FileAt};
use crate::api::transfer::fetch::{FAT32_MAX, Fetch, FetchError, Rewind};
use crate::api::urls::{TransferBitrate, TransferCodec};
use crate::api::{ApiError, Client};

/// Tries of one file: a first, then two more.
const TRIES: u32 = 3;
/// Conversions this process runs at once, every run together.
const TRANSCODES: usize = 2;
/// How far ahead of the engine a run's conversions are made: the one it is
/// on, and this many more.
const AHEAD: usize = 2;
/// Two whole conversions this close to each other are the server's answer
/// for the file, whatever the manifest says of its length.
const AGREE_MS: u64 = 50;

/// The conversions running in this process ([`TRANSCODES`] at most).
static RUNNING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// A conversion's turn, until it is dropped.
struct Turn;

impl Turn {
    /// Waits for a turn; None when `stop` is set first.
    fn take(stop: &AtomicBool) -> Option<Turn> {
        let (lock, cv) = &RUNNING;
        let mut running = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *running >= TRANSCODES {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            let wait = cv.wait_timeout(running, Duration::from_millis(100));
            running = wait.unwrap_or_else(|e| e.into_inner()).0;
        }
        *running += 1;
        Some(Turn)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (lock, cv) = &RUNNING;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        cv.notify_all();
    }
}

/// What a conversion dropped, put back into it before the card takes it:
/// the sync's retag (the album's cover, the source's own BPM and key).
pub(crate) trait Finish {
    /// `path` is the converted file, on this computer. Err: the file is
    /// skipped, and why.
    fn finish(&mut self, file: &WantedFile, path: &Path) -> Result<(), String>;
}

/// Nothing put back.
pub(crate) struct AsMade;

impl Finish for AsMade {
    fn finish(&mut self, _: &WantedFile, _: &Path) -> Result<(), String> {
        Ok(())
    }
}

/// A conversion to make: its source, what it is made into, the length it
/// must come to, and where it goes on this computer.
#[derive(Clone, Debug)]
struct Job {
    server_path: String,
    codec: TransferCodec,
    bitrate: TransferBitrate,
    expected_ms: Option<u64>,
    dest: PathBuf,
}

/// A conversion made and checked, waiting in its file.
#[derive(Debug)]
struct Made {
    dest: PathBuf,
    content_type: Option<String>,
}

fn skip(why: impl Into<String>) -> Failed {
    Failed { why: why.into(), stop: false }
}

fn stop(why: impl Into<String>) -> Failed {
    Failed { why: why.into(), stop: true }
}

/// A fetch's failure that another try may mend: the connection, not the
/// file.
fn passing(e: &FetchError) -> bool {
    matches!(
        e,
        FetchError::Api(ApiError::Network(_))
            | FetchError::Stalled(_)
            | FetchError::TooSlow(_)
            | FetchError::Short { .. }
    )
}

/// A failure, as the run takes it: the whole run's, or this file's.
fn failed(e: FetchError) -> Failed {
    match &e {
        FetchError::Api(ApiError::Unauthorized) => {
            stop(format!("the server no longer takes this sign-in: {e}"))
        }
        FetchError::Api(ApiError::Network(_)) => stop(format!("the server is out of reach: {e}")),
        FetchError::Limited(_) => stop(e.to_string()),
        _ => skip(e.to_string()),
    }
}

/// Waits `n` seconds, or less when `stop` is set.
fn pause(seconds: u64, stop: &AtomicBool) {
    for _ in 0..seconds * 10 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A future run on the shared runtime, given up when `stop` is set (the
/// request is dropped, and the server kills its ffmpeg).
fn until<T>(fut: impl std::future::Future<Output = T>, stop: &AtomicBool) -> Option<T> {
    let watch = async {
        while !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let both = async {
        tokio::select! {
            out = fut => Some(out),
            () = watch => None,
        }
    };
    crate::runtime::block_on(both).ok().flatten()
}

/// Makes one conversion: see the module's note. A conversion that fails
/// leaves no file.
fn make(client: &Client, job: &Job, max_channels: u32, stop: &AtomicBool) -> Result<Made, Failed> {
    let made = make_once(client, job, max_channels, stop);
    if made.is_err() {
        let _ = fs::remove_file(&job.dest);
    }
    made
}

fn make_once(client: &Client, job: &Job, channels: u32, stop: &AtomicBool) -> Result<Made, Failed> {
    let Some(_turn) = Turn::take(stop) else { return Err(skip("stopped")) };
    let mut whole_before: Option<u64> = None;
    let mut why = String::new();
    for attempt in 1..=TRIES {
        if attempt > 1 {
            pause(attempt as u64 - 1, stop);
        }
        if stop.load(Ordering::Relaxed) {
            return Err(skip("stopped"));
        }
        let fetch = Fetch::converted(&job.server_path, job.codec, job.bitrate).cap(FAT32_MAX);
        let fetched = until(client.download_async(&fetch, &job.dest, &mut |_, _| {}), stop);
        let got = match fetched {
            None => return Err(skip("stopped")),
            Some(Ok(got)) => got,
            Some(Err(f)) if passing(&f.error) && attempt < TRIES => {
                why = f.error.to_string();
                continue;
            }
            Some(Err(f)) => return Err(failed(f.error)),
        };
        let heard = FileAt::open(&job.dest)
            .map_err(|e| e.to_string())
            .and_then(|at| length::heard(job.codec, &at));
        let checked = heard.clone().and_then(|h| length::check(&h, job.expected_ms, channels));
        match checked {
            Ok(()) => return Ok(Made { dest: job.dest.clone(), content_type: got.content_type }),
            Err(e) => {
                why = e;
                // The same whole length twice: the server's answer for it.
                let whole = heard.ok().filter(|h| h.ended).map(|h| h.ms);
                if let (Some(now), Some(before)) = (whole, whole_before)
                    && now.abs_diff(before) <= AGREE_MS
                {
                    return Err(skip(format!("the conversion is {why}, twice")));
                }
                whole_before = whole;
            }
        }
    }
    Err(skip(format!("the conversion didn't come whole: {why}")))
}

// ── Ahead of the engine ─────────────────────────────────────────────────────

/// Where the conversions made ahead stand.
#[derive(Default)]
struct Line {
    /// The next job to start.
    next: usize,
    /// The job the engine is on: jobs start up to [`AHEAD`] past it.
    on: usize,
    done: HashMap<usize, Result<Made, Failed>>,
    /// Jobs the engine passed by, whose results went.
    passed: HashSet<usize>,
}

struct Shared {
    client: Arc<Client>,
    jobs: Vec<Job>,
    max_channels: u32,
    line: Mutex<Line>,
    cv: Condvar,
    stop: Arc<AtomicBool>,
}

/// A run's conversions, made in its order on two threads.
struct Pipeline {
    shared: Arc<Shared>,
    at: HashMap<(String, u8), usize>,
    workers: Vec<JoinHandle<()>>,
}

impl Pipeline {
    fn new(client: Arc<Client>, jobs: Vec<Job>, channels: u32, stop: Arc<AtomicBool>) -> Pipeline {
        let key = |(i, j): (usize, &Job)| ((j.server_path.clone(), container_of(j.codec)), i);
        let at = jobs.iter().enumerate().map(key).collect();
        let (line, cv) = (Mutex::new(Line::default()), Condvar::new());
        let shared = Arc::new(Shared { client, jobs, max_channels: channels, line, cv, stop });
        let workers = (0..TRANSCODES)
            .map(|_| {
                let shared = shared.clone();
                std::thread::spawn(move || work(&shared))
            })
            .collect();
        Pipeline { shared, at, workers }
    }

    /// The conversion of `path` into `container`, waiting for it: None when
    /// it isn't one of the run's.
    fn take(&self, path: &str, container: u8) -> Option<Result<Made, Failed>> {
        let &i = self.at.get(&(path.to_string(), container))?;
        let s = &self.shared;
        let mut line = s.line.lock().unwrap_or_else(|e| e.into_inner());
        if i > line.on {
            line.on = i;
            s.cv.notify_all();
        }
        // The ones before it the engine passed by: their files go. One asked
        // for after it was passed is made again, by the caller.
        if line.passed.contains(&i) {
            return None;
        }
        let passed: Vec<usize> = line.done.keys().copied().filter(|&k| k < i).collect();
        for k in passed {
            if let Some(Ok(made)) = line.done.remove(&k) {
                let _ = fs::remove_file(made.dest);
            }
            line.passed.insert(k);
        }
        loop {
            if let Some(result) = line.done.remove(&i) {
                return Some(result);
            }
            if s.stop.load(Ordering::Relaxed) {
                return Some(Err(skip("stopped")));
            }
            line = s.cv.wait(line).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn work(s: &Shared) {
    loop {
        let i = {
            let mut line = s.line.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if s.stop.load(Ordering::Relaxed) || line.next >= s.jobs.len() {
                    return;
                }
                if line.next <= line.on + AHEAD {
                    break;
                }
                line = s.cv.wait(line).unwrap_or_else(|e| e.into_inner());
            }
            line.next += 1;
            line.next - 1
        };
        let result = make(&s.client, &s.jobs[i], s.max_channels, &s.stop);
        let mut line = s.line.lock().unwrap_or_else(|e| e.into_inner());
        line.done.insert(i, result);
        s.cv.notify_all();
    }
}

fn container_of(codec: TransferCodec) -> u8 {
    super::super::plan::container(codec)
}

// ── The source ──────────────────────────────────────────────────────────────

/// A folder of this computer's for a run's conversions, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> io::Result<Scratch> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mstream-card-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The files of one server over HTTP.
pub(crate) struct Http {
    client: Arc<Client>,
    /// The server these are: an index into the wanted card's origins.
    origin: usize,
    /// What the player plays: a conversion of more channels is refused.
    max_channels: u32,
    /// Each conversion's expected length, by its source's server path.
    lengths: HashMap<String, u64>,
    finish: Box<dyn Finish>,
    stop: Arc<AtomicBool>,
    // Dropped before the scratch folder its files are in.
    pipeline: Option<Pipeline>,
    scratch: Scratch,
    /// Conversions made here and not ahead.
    made: u64,
    /// A later run takes up what a run leaves of a file (2.12.8).
    resumable: bool,
}

impl Http {
    pub(crate) fn new(client: Arc<Client>, origin: usize, max_channels: u32) -> io::Result<Http> {
        Ok(Http {
            client,
            origin,
            max_channels,
            lengths: HashMap::new(),
            finish: Box::new(AsMade),
            stop: Arc::new(AtomicBool::new(false)),
            pipeline: None,
            scratch: Scratch::new()?,
            made: 0,
            resumable: false,
        })
    }

    /// A later run takes up the bytes a run leaves of this server's files
    /// (2.12.8): a peer's (B9), whose daily quota a second fetch would spend
    /// again. Off for the user's own server, whose files simply start over.
    pub(crate) fn resuming(&mut self, on: bool) {
        self.resumable = on;
    }

    /// What a conversion dropped, put back before the card takes it.
    pub(crate) fn finish_with(&mut self, finish: Box<dyn Finish>) {
        self.finish = finish;
    }

    /// The length each conversion must come to, by its source's server path
    /// (the manifest's `duration`).
    pub(crate) fn lengths(&mut self, lengths: HashMap<String, u64>) {
        self.lengths = lengths;
    }

    /// The run's conversions, in the order it fetches them: made from now
    /// on, ahead of it.
    pub(crate) fn ahead(&mut self, files: &[&WantedFile]) {
        let jobs: Vec<Job> = files
            .iter()
            .filter(|f| f.origin == self.origin)
            .enumerate()
            .filter_map(|(i, f)| self.job(f, &format!("ahead-{i}")))
            .collect();
        self.pipeline = None;
        if !jobs.is_empty() {
            let stop = Arc::new(AtomicBool::new(false));
            self.pipeline = Some(Pipeline::new(self.client.clone(), jobs, self.max_channels, stop));
        }
    }

    /// The job of a file that is a conversion.
    fn job(&self, file: &WantedFile, name: &str) -> Option<Job> {
        let Bytes::Converted(container) = file.bytes else { return None };
        let codec = codec_of(container)?;
        Some(Job {
            server_path: file.ledger.server_path.clone(),
            codec,
            bitrate: TransferBitrate::from_kbps(file.kbps as u32)?,
            expected_ms: self.lengths.get(&file.ledger.server_path).copied(),
            // Named as its codec: the retag reads the format by its name.
            dest: self.scratch.0.join(format!("{name}.{}", codec.extension())),
        })
    }

    fn original(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
        let l = &file.ledger;
        let (mut held, mut validator): (u64, Option<String>) = (0, None);
        for attempt in 1..=TRIES {
            if attempt > 1 {
                pause(attempt as u64 - 1, &self.stop);
            }
            let fetch = Fetch::file(&l.server_path).expecting(l.server_size);
            let fetch = fetch.resuming(validator.as_deref());
            match self.client.stream(&fetch, into, held, &mut |_, _| {}) {
                Ok(got) => return Ok(Fetched { content_type: got.content_type }),
                Err(f) => {
                    (held, validator) = (f.partial.len, f.partial.validator.clone());
                    if !passing(&f.error) || attempt == TRIES {
                        return Err(failed(f.error));
                    }
                    if held > 0 && validator.is_none() {
                        let why = format!("{}, and the server gave nothing to resume it by", f.error);
                        return Err(skip(why));
                    }
                }
            }
        }
        unreachable!("the last try returns")
    }

    /// The server's bytes into the card's partial, which holds `have`'s of
    /// them already when there is one: the rest under its validator, a file
    /// changed since started over in the same answer. Three tries, each
    /// from where the partial is then; one with no validator to go on by
    /// starts over, which a partial can.
    fn part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn Part,
    ) -> Result<Fetched, Failed> {
        let l = &file.ledger;
        let (mut held, mut validator) = match have {
            Some(h) => (h.bytes, Some(h.validator.clone())),
            None => (0, None),
        };
        let mut part = AsRewind(into);
        for attempt in 1..=TRIES {
            if attempt > 1 {
                pause(attempt as u64 - 1, &self.stop);
            }
            let fetch = Fetch::file(&l.server_path).expecting(l.server_size);
            let fetch = fetch.resuming(validator.as_deref());
            match self.client.stream_part(&fetch, &mut part, held, &mut |_, _| {}) {
                Ok(got) => return Ok(Fetched { content_type: got.content_type }),
                Err(f) => {
                    (held, validator) = (f.partial.len, f.partial.validator.clone());
                    if !passing(&f.error) || attempt == TRIES {
                        return Err(failed(f.error));
                    }
                }
            }
        }
        unreachable!("the last try returns")
    }

    fn converted(
        &mut self,
        file: &WantedFile,
        container: u8,
        into: &mut dyn Write,
    ) -> Result<Fetched, Failed> {
        let path = &file.ledger.server_path;
        let ahead = self.pipeline.as_ref().and_then(|p| p.take(path, container));
        let made = match ahead {
            Some(result) => result?,
            None => {
                self.made += 1;
                let job = self.job(file, &format!("now-{}", self.made)).ok_or_else(|| {
                    skip(format!("no conversion to a {}k {container} file", file.kbps))
                })?;
                make(&self.client, &job, self.max_channels, &self.stop)?
            }
        };
        let poured = self.finish.finish(file, &made.dest).map_err(skip).and_then(|()| {
            let mut from = File::open(&made.dest).map_err(|e| skip(e.to_string()))?;
            pour(&mut from, into)
        });
        let _ = fs::remove_file(&made.dest);
        poured.map(|()| Fetched { content_type: made.content_type })
    }
}

/// The engine's partial as the client's [`Rewind`].
struct AsRewind<'a>(&'a mut dyn Part);

impl Write for AsRewind<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Rewind for AsRewind<'_> {
    fn start_at(&mut self, start: u64, validator: Option<&str>) -> io::Result<()> {
        self.0.answered(start, validator)
    }
}

/// A file into the card's stage, whole.
fn pour(from: &mut dyn Read, into: &mut dyn Write) -> Result<(), Failed> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(skip(format!("the conversion on this computer can't be read: {e}")));
            }
        };
        into.write_all(&buf[..n]).map_err(|e| skip(e.to_string()))?;
    }
}

impl Fetcher for Http {
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
        if file.origin != self.origin {
            return Err(skip("a file of another server"));
        }
        match file.bytes {
            Bytes::Converted(container) => self.converted(file, container, into),
            _ => self.original(file, into),
        }
    }

    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed> {
        if file.origin != self.origin {
            return Err(skip("a file of another server"));
        }
        let read = self.client.read_range(&file.ledger.server_path, start, len as u64);
        read.map(|r| r.bytes).map_err(failed)
    }

    fn resumable(&self, file: &WantedFile) -> bool {
        self.resumable && file.origin == self.origin && file.original()
    }

    fn fetch_part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn Part,
    ) -> Result<Fetched, Failed> {
        if !Fetcher::resumable(self, file) {
            into.answered(0, None).map_err(|e| skip(e.to_string()))?;
            return self.fetch(file, into);
        }
        self.part(file, have, into)
    }
}

impl Drop for Http {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.pipeline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::super::length::tests::opus_bytes;
    use super::*;
    use crate::api::transfer::stub::{canned, chunked, reply, routed};
    use crate::device::card::contract::mptg::{CONTAINER_OPUS, Ledger};

    fn wanted(path: &str, bytes: Bytes, size: u64) -> WantedFile {
        let ledger = Ledger { server_path: path.into(), server_size: size, ..Ledger::default() };
        let kbps = if matches!(bytes, Bytes::Converted(_)) { 192 } else { 0 };
        WantedFile { ledger, bytes, kbps, size, ..WantedFile::default() }
    }

    #[test]
    fn a_files_bytes_are_resumed_where_the_connection_was_cut() {
        let etag = "W/\"a-18f\"";
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: {etag}\r\n");
        let cut = format!("{head}Connection: close\r\n\r\n0123");
        let (base, seen) = canned(vec![
            cut.into_bytes(),
            reply("206 Partial Content", &[("Content-Range", "bytes 4-9/10"), ("ETag", etag)], b"456789"),
            reply("404 Not Found", &[], br#"{"error":"Not Found"}"#),
            reply("401 Unauthorized", &[], br#"{"error":"Authentication Error"}"#),
        ]);
        let client = Arc::new(Client::new(&base).unwrap().with_token(Some("tok".into())));
        let mut http = Http::new(client, 0, 2).unwrap();
        let mut into = Vec::new();
        let got = http.fetch(&wanted("lib/A/01.flac", Bytes::Original { ogg_opus: false }, 10), &mut into);
        assert!(got.is_ok(), "{got:?}");
        assert_eq!(into, b"0123456789");
        let _ = seen.recv_timeout(Duration::from_secs(5));
        let second = seen.recv_timeout(Duration::from_secs(5)).unwrap().to_ascii_lowercase();
        assert!(second.contains("range: bytes=4-") && second.contains("if-range: w/\"a-18f\""), "{second}");

        let original = Bytes::Original { ogg_opus: false };
        let gone = http.fetch(&wanted("lib/A/02.flac", original, 10), &mut Vec::new());
        assert!(!gone.unwrap_err().stop, "a file gone skips that file");
        let signed_out = http.fetch(&wanted("lib/A/03.flac", original, 10), &mut Vec::new());
        assert!(signed_out.unwrap_err().stop, "lost sign-in stops the run");
        let other = WantedFile { origin: 1, ..wanted("lib/A/04.flac", Bytes::Original { ogg_opus: false }, 10) };
        assert!(http.fetch(&other, &mut Vec::new()).is_err(), "another server's file isn't this one's");
    }

    /// A card's partial in memory, as the engine's is: its bytes, and each
    /// answer it was told of.
    #[derive(Default)]
    struct Partial {
        bytes: Vec<u8>,
        answers: Vec<(u64, Option<String>)>,
    }

    impl Write for Partial {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Part for Partial {
        fn answered(&mut self, from: u64, validator: Option<&str>) -> io::Result<()> {
            self.bytes.truncate(from as usize);
            self.answers.push((from, validator.map(String::from)));
            Ok(())
        }
    }

    #[test]
    fn a_peers_partial_is_taken_up_under_its_validator_and_a_changed_file_starts_over() {
        let (etag, date) = ("W/\"a-18f\"", "Mon, 21 Sep 2026 14:13:20 GMT");
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: {etag}\r\n");
        let rest = [("Content-Range", "bytes 4-9/10"), ("ETag", etag)];
        let (base, seen) = canned(vec![
            reply("206 Partial Content", &rest, b"456789"),
            // Changed since: the whole new file, in that same answer.
            reply("200 OK", &[("ETag", "W/\"a-190\"")], b"abcdefghij"),
            // A new partial cut after four bytes, then taken up in the run.
            format!("{head}Connection: close\r\n\r\n0123").into_bytes(),
            reply("206 Partial Content", &rest, b"456789"),
            // By the date of a file that had no ETag.
            reply("206 Partial Content", &[("Content-Range", "bytes 4-9/10")], b"456789"),
            // The user's own server: from the first byte, no range.
            reply("200 OK", &[("ETag", etag)], b"0123456789"),
        ]);
        let next = || seen.recv_timeout(Duration::from_secs(5)).unwrap().to_ascii_lowercase();
        let client = Arc::new(Client::new(&base).unwrap());
        let mut http = Http::new(client, 0, 2).unwrap();
        http.resuming(true);
        let file = wanted("lib/A/01.flac", Bytes::Original { ogg_opus: false }, 10);
        assert!(http.resumable(&file));
        assert!(!http.resumable(&wanted("lib/A/01.wav", Bytes::Converted(CONTAINER_OPUS), 10)));
        let take = |validator: &str| Have { bytes: 4, validator: validator.to_string() };
        let held = || Partial { bytes: b"0123".to_vec(), ..Partial::default() };
        let asked_from_4 = |req: &str| {
            req.contains("range: bytes=4-\r\n") && req.contains("if-range: w/\"a-18f\"\r\n")
        };

        let mut part = held();
        http.fetch_part(&file, Some(&take(etag)), &mut part).unwrap();
        let req = next();
        assert!(asked_from_4(&req), "{req}");
        assert_eq!(part.bytes, b"0123456789");
        assert_eq!(part.answers, [(4, Some(etag.to_string()))]);

        let mut part = held();
        http.fetch_part(&file, Some(&take(etag)), &mut part).unwrap();
        assert_eq!(part.bytes, b"abcdefghij", "the old part is gone, not stitched");
        assert_eq!(part.answers, [(0, Some("W/\"a-190\"".to_string()))]);
        let _ = next();

        let mut part = Partial::default();
        http.fetch_part(&file, None, &mut part).unwrap();
        assert_eq!(part.bytes, b"0123456789");
        let (_, again) = (next(), next());
        assert!(asked_from_4(&again), "the cut taken up from the partial: {again}");

        let mut part = held();
        http.fetch_part(&file, Some(&take(date)), &mut part).unwrap();
        let req = next();
        assert!(req.contains("if-range: mon, 21 sep 2026 14:13:20 gmt\r\n"), "{req}");
        assert_eq!(part.bytes, b"0123456789");

        // Not resuming: started over, as the engine's default fetch does.
        http.resuming(false);
        let mut part = held();
        http.fetch_part(&file, Some(&take(etag)), &mut part).unwrap();
        assert_eq!(part.bytes, b"0123456789");
        assert_eq!(part.answers, [(0, None)]);
        let req = next();
        assert!(!req.contains("\r\nrange:"), "{req}");
    }

    #[test]
    fn a_conversion_is_held_to_its_length_and_made_again_when_short() {
        let whole = opus_bytes(181_500, 2, true);
        let short = opus_bytes(60_000, 2, false);
        let ogg = [("Content-Type", "audio/ogg")];
        let (base, seen) = canned(vec![
            // Short (ffmpeg died), then whole.
            chunked("200 OK", &ogg, &[&short]),
            chunked("200 OK", &ogg, &[&whole]),
            // The server's silent fallback: another codec.
            chunked("200 OK", &[("Content-Type", "audio/mpeg")], &[b"ID3"]),
            // Whole twice, but not the manifest's length: the server's answer.
            chunked("200 OK", &ogg, &[&opus_bytes(90_000, 2, true)]),
            chunked("200 OK", &ogg, &[&opus_bytes(90_010, 2, true)]),
        ]);
        let client = Arc::new(Client::new(&base).unwrap());
        let mut http = Http::new(client, 0, 2).unwrap();
        let file = wanted("lib/A/01.wav", Bytes::Converted(CONTAINER_OPUS), 4_000_000);
        http.lengths(HashMap::from([("lib/A/01.wav".to_string(), 181_000)]));
        let mut into = Vec::new();
        let got = http.fetch(&file, &mut into).expect("the second try is whole");
        assert_eq!(got.content_type.as_deref(), Some("audio/ogg"));
        assert_eq!(into, whole);
        for _ in 0..2 {
            let req = seen.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(req.starts_with("GET /transcode/lib/A/01.wav?codec=opus&bitrate=192k "), "{req}");
        }

        let wrong = http.fetch(&file, &mut Vec::new()).unwrap_err();
        assert!(!wrong.stop && wrong.why.contains("audio/mpeg"), "{wrong:?}");
        let twice = http.fetch(&file, &mut Vec::new()).unwrap_err();
        assert!(!twice.stop && twice.why.contains("twice"), "{twice:?}");
        assert!(fs::read_dir(&http.scratch.0).unwrap().next().is_none(), "nothing left on this computer");
    }

    /// Puts a mark at the end of each conversion it finishes.
    struct Mark;

    impl Finish for Mark {
        fn finish(&mut self, _: &WantedFile, path: &Path) -> Result<(), String> {
            let mut file = fs::OpenOptions::new().append(true).open(path).map_err(|e| e.to_string())?;
            file.write_all(b"MARK").map_err(|e| e.to_string())
        }
    }

    #[test]
    fn a_runs_conversions_are_made_ahead_two_at_a_time_and_finished_before_the_card() {
        let streams: Vec<Vec<u8>> = (1..=4).map(|n| opus_bytes(n * 10_000, 2, true)).collect();
        let (tx, seen) = std::sync::mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let served = streams.clone();
        let base = routed(move |asked| {
            let _ = tx.lock().unwrap().send(asked.target.clone());
            // `/transcode/lib/A/0<n>.wav`: the stream of its length.
            let n = asked.path().trim_end_matches(".wav").chars().last().and_then(|c| c.to_digit(10));
            let stream = &served[n.unwrap_or(1) as usize - 1];
            chunked("200 OK", &[("Content-Type", "audio/ogg")], &[stream])
        });
        let client = Arc::new(Client::new(&base).unwrap());
        let mut http = Http::new(client, 0, 2).unwrap();
        http.finish_with(Box::new(Mark));
        let files: Vec<WantedFile> =
            (1..=4).map(|n| wanted(&format!("lib/A/0{n}.wav"), Bytes::Converted(CONTAINER_OPUS), 1)).collect();
        let lengths = (1..=4u64).map(|n| (format!("lib/A/0{n}.wav"), n * 10_000)).collect();
        http.lengths(lengths);
        http.ahead(&files.iter().collect::<Vec<_>>());
        // Made ahead: the first requests come before the engine asks.
        let first = seen.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(first.contains("/transcode/lib/A/0"), "{first}");
        for (n, file) in files.iter().enumerate() {
            let mut into = Vec::new();
            http.fetch(file, &mut into).unwrap_or_else(|e| panic!("{n}: {e:?}"));
            let mut want = streams[n].clone();
            want.extend_from_slice(b"MARK");
            assert_eq!(into, want, "{n}: finished, then poured");
        }
    }
}
