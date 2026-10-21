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
//! **A conversion** comes from `/transcode`, into a file on this computer
//! first: it streams with no length and no ranges, and a 200 that ends early
//! (the server's ffmpeg died) looks like any other, so it is held to its
//! source's length ([`length`](super::length)) and made again when it falls
//! short — three tries, or two whole ones that agree with each other but not
//! with the manifest, and it is skipped. Its Content-Type must be the codec
//! asked for (mStream falls back to its own default silently). Then what
//! the conversion dropped is put back ([`Finish`], B1's retag) before the
//! card's one write. An older server's `/transcode` has no cap of its own
//! and a bulk job evicts its listeners' cache, so at most two conversions
//! run at a time in this process ([`TRANSCODES`]); a run's conversions are
//! made ahead of the engine, in its order, two at a time ([`Http::ahead`]),
//! so the card is written while the server encodes. A server with a bulk
//! queue ([`Http::bulk`]) is asked `?bulk=1` (out of its cache, behind its
//! cap), at most as many at once as it runs; a full queue's 503 is waited
//! out for as long as it says, as often as it says it, and never fails the
//! file. Only the queue's, in its words: the server restarting answers 503
//! with a Retry-After too, as a proxy in front of it may, and fails the file
//! as any refusal does.
//!
//! **Ahead of the card** ([`Http::prefetch`]): the run's files of the
//! server's own bytes download up to three at once in the engine's order,
//! ahead of its card writer, so the card never waits on the network for a
//! file that is in: held in memory (256 MiB at most of files started and
//! not yet taken), a file past 64 MiB in this computer's temp folder,
//! deleted once on the card or passed by. A download ahead is given up when
//! the engine passes its file by, and at the listener's stop for every file
//! past the one the engine is on; whatever the run leaves goes with it. A
//! failure that stops the run starts nothing past it, and is the engine's
//! once it comes to that file, the files before it written first.
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
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use super::super::engine::{CARD_WRITE, Failed, Fetched, Fetcher, WantedFile};
use super::super::names::Bytes;
use super::super::plan::codec_of;
use super::length::{self, FileAt};
use crate::api::transfer::CONVERSIONS;
use crate::api::transfer::fetch::{self as wire, FAT32_MAX, Fetch, FetchError};
use crate::api::urls::{TransferBitrate, TransferCodec};
use crate::api::{ApiError, Client};

/// Tries of one file: a first, then two more.
const TRIES: u32 = 3;
/// Conversions this process runs at once, every run together.
const TRANSCODES: usize = CONVERSIONS as usize;
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
    /// Waits for a turn, while `limit` or more run (at most
    /// [`TRANSCODES`]); None when `stop` is set first.
    fn take(stop: &AtomicBool, limit: usize) -> Option<Turn> {
        let (lock, cv) = &RUNNING;
        let mut running = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *running >= limit.clamp(1, TRANSCODES) {
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
/// must come to, and where it goes on this computer; `bulk`, asked as one
/// file of a long job.
#[derive(Clone, Debug)]
struct Job {
    server_path: String,
    codec: TransferCodec,
    bitrate: TransferBitrate,
    bulk: bool,
    expected_ms: Option<u64>,
    dest: PathBuf,
}

/// A conversion made and checked, waiting in its file.
#[derive(Debug)]
struct Made {
    dest: PathBuf,
    content_type: Option<String>,
}

pub(super) fn skip(why: impl Into<String>) -> Failed {
    Failed { why: why.into(), stop: false }
}

pub(super) fn stop(why: impl Into<String>) -> Failed {
    Failed { why: why.into(), stop: true }
}

/// A fetch's failure that another try may mend: the connection, not the
/// file.
pub(super) fn passing(e: &FetchError) -> bool {
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

/// A full bulk queue's Retry-After in seconds, held to between a second and
/// five minutes (mStream says 10).
fn busy_wait(wait: Duration) -> u64 {
    wait.as_secs().clamp(1, 300)
}

/// Waits `n` seconds, or less when `stop` is set.
pub(super) fn pause(seconds: u64, stop: &AtomicBool) {
    pause_while(seconds, &|| stop.load(Ordering::Relaxed));
}

/// [`pause`], cut short once `stopped` says so.
fn pause_while(seconds: u64, stopped: &dyn Fn() -> bool) {
    for _ in 0..seconds * 10 {
        if stopped() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A future run on the shared runtime, given up when `stop` is set (the
/// request is dropped, and the server kills its ffmpeg).
fn until<T>(fut: impl std::future::Future<Output = T>, stop: &AtomicBool) -> Option<T> {
    until_with(fut, &|| stop.load(Ordering::Relaxed))
}

/// [`until`], given up once `stopped` says so (asked every 100 ms).
fn until_with<T>(
    fut: impl std::future::Future<Output = T>,
    stopped: &dyn Fn() -> bool,
) -> Option<T> {
    let watch = async {
        while !stopped() {
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

/// How a run's conversions go: at most `limit` at a time (a bulk queue's
/// cap, else [`TRANSCODES`]), and what the player plays.
#[derive(Clone, Copy, Debug)]
struct Pace {
    limit: usize,
    max_channels: u32,
}

/// Makes one conversion: see the module's note. A conversion that fails
/// leaves no file.
fn make(client: &Client, job: &Job, pace: Pace, stop: &AtomicBool) -> Result<Made, Failed> {
    let made = make_once(client, job, pace, stop);
    if made.is_err() {
        let _ = fs::remove_file(&job.dest);
    }
    made
}

fn make_once(client: &Client, job: &Job, pace: Pace, stop: &AtomicBool) -> Result<Made, Failed> {
    let Some(_turn) = Turn::take(stop, pace.limit) else { return Err(skip("stopped")) };
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
        let fetch = if job.bulk { fetch.bulk() } else { fetch };
        let fetched = loop {
            let fetched = until(client.download_async(&fetch, &job.dest, &mut |_, _| {}), stop);
            // The server's bulk queue is full (its words, not any 503's):
            // asked again when it says, for as long as it says so, and no
            // try is spent.
            match &fetched {
                Some(Err(f)) if job.bulk && !stop.load(Ordering::Relaxed) => match f.error {
                    FetchError::Busy(wait) => pause(busy_wait(wait), stop),
                    _ => break fetched,
                },
                _ => break fetched,
            }
        };
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
        let channels = pace.max_channels;
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

/// What one try at a file's bytes from `/media` came to; None: given up.
type Tried = Option<Result<wire::Fetched, wire::Failed>>;

/// One try at a file's bytes from `/media`, into a writer after the bytes
/// of the file it holds already.
type Try<'t> = dyn FnMut(&Fetch<'_>, &mut dyn Write, u64) -> Tried + 't;

/// A file's bytes from `/media` into `into`, as the server holds them
/// (`size` of them): three tries, each after the first taken up where the
/// one before stopped, with Range under the ETag its answer gave
/// (If-Range). `once` makes a try; `stopped` cuts the waits between them
/// short.
fn media(
    path: &str,
    size: u64,
    into: &mut dyn Write,
    stopped: &dyn Fn() -> bool,
    once: &mut Try<'_>,
) -> Result<Fetched, Failed> {
    let (mut held, mut validator): (u64, Option<String>) = (0, None);
    for attempt in 1..=TRIES {
        if attempt > 1 {
            pause_while(attempt as u64 - 1, stopped);
        }
        let fetch = Fetch::file(path).expecting(size);
        let fetch = fetch.resuming(validator.as_deref());
        match once(&fetch, into, held) {
            None => return Err(skip("stopped")),
            Some(Ok(got)) => return Ok(Fetched { content_type: got.content_type }),
            Some(Err(f)) => {
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
    pace: Pace,
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
    fn new(client: Arc<Client>, jobs: Vec<Job>, pace: Pace, stop: Arc<AtomicBool>) -> Pipeline {
        let key = |(i, j): (usize, &Job)| ((j.server_path.clone(), container_of(j.codec)), i);
        let at = jobs.iter().enumerate().map(key).collect();
        let (line, cv) = (Mutex::new(Line::default()), Condvar::new());
        let shared = Arc::new(Shared { client, jobs, pace, line, cv, stop });
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
        let result = make(&s.client, &s.jobs[i], s.pace, &s.stop);
        let mut line = s.line.lock().unwrap_or_else(|e| e.into_inner());
        line.done.insert(i, result);
        s.cv.notify_all();
    }
}

fn container_of(codec: TransferCodec) -> u8 {
    super::super::plan::container(codec)
}

// ── Ahead of the card ───────────────────────────────────────────────────────

/// The server's own files downloading at once ahead of the engine (the
/// transfer plan's 2.4).
const MEDIA_AT_ONCE: usize = 3;
/// How far ahead of the file the engine is on they go: this many files,
/// and this many bytes of the files started and not yet taken. The file
/// after the engine's always starts, whatever its size.
const MEDIA_AHEAD: usize = 16;
const MEDIA_HELD: u64 = 256 << 20;
/// A file past this size downloads into the run's scratch folder on this
/// computer, not into memory.
const MEDIA_IN_MEMORY: u64 = 64 << 20;

/// How far ahead of the engine the downloads go.
#[derive(Clone, Copy, Debug)]
struct Room {
    /// Files past the one the engine is on.
    files: usize,
    /// Bytes of the files started and not yet taken.
    bytes: u64,
    /// A file past this many bytes downloads into the scratch folder.
    in_memory: u64,
}

impl Room {
    const RUN: Room = Room { files: MEDIA_AHEAD, bytes: MEDIA_HELD, in_memory: MEDIA_IN_MEMORY };
}

/// A file downloaded ahead, waiting for the engine.
enum Held {
    Memory(Vec<u8>),
    /// In the run's scratch folder: deleted once poured or passed by.
    Spooled(PathBuf),
}

impl Held {
    /// Into the card's stage file, [`CARD_WRITE`] a write; a spooled file
    /// deleted after.
    fn pour(self, into: &mut dyn Write) -> Result<(), Failed> {
        match self {
            Held::Memory(bytes) => {
                for piece in bytes.chunks(CARD_WRITE) {
                    into.write_all(piece).map_err(|e| skip(e.to_string()))?;
                }
                Ok(())
            }
            Held::Spooled(path) => {
                let opened = File::open(&path).map_err(|e| skip(format!("{}: {e}", path.display())));
                let poured = opened.and_then(|mut file| pour(&mut file, into));
                let _ = fs::remove_file(&path);
                poured
            }
        }
    }

    fn discard(self) {
        if let Held::Spooled(path) = self {
            let _ = fs::remove_file(path);
        }
    }
}

/// How a download ahead ended.
enum Got {
    Held(Held, Option<String>),
    Failed(Failed),
    /// Given up: the run stopping, the engine passing its file by, or no
    /// room for it on this computer. The engine fetches it itself if it
    /// asks for it.
    Dropped,
}

impl Got {
    fn discard(self) {
        if let Got::Held(held, _) = self {
            held.discard();
        }
    }
}

/// A file to download ahead: its server path and size.
struct Want {
    path: String,
    size: u64,
}

/// Where the downloads ahead stand.
#[derive(Default)]
struct Lane {
    /// The next file to start.
    next: usize,
    /// The file the engine asked for last: files start up to
    /// [`Room::files`] past it.
    on: usize,
    running: HashSet<usize>,
    done: HashMap<usize, Got>,
    /// The bytes of the files started and not yet taken or let go.
    held: u64,
    /// Files the engine passed by: a download of one still running is given
    /// up, and what came of it goes.
    passed: HashSet<usize>,
    /// A file whose failure stops the run: nothing past it starts until the
    /// engine passes it by.
    halted: Option<usize>,
}

struct Ahead {
    client: Arc<Client>,
    wants: Vec<Want>,
    /// The run's scratch folder, for the files past [`Room::in_memory`].
    dir: PathBuf,
    room: Room,
    lane: Mutex<Lane>,
    cv: Condvar,
    stop: AtomicBool,
    /// The listener's stop: nothing more starts, and the downloads past the
    /// engine's file are given up.
    listener: Option<Arc<AtomicBool>>,
}

impl Ahead {
    fn lane(&self) -> MutexGuard<'_, Lane> {
        self.lane.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn listener_stopped(&self) -> bool {
        self.listener.as_ref().is_some_and(|stop| stop.load(Ordering::SeqCst))
    }

    /// File `i`'s download is to be given up.
    fn gives_up(&self, i: usize) -> bool {
        if self.stopped() {
            return true;
        }
        let lane = self.lane();
        lane.passed.contains(&i) || (self.listener_stopped() && i > lane.on)
    }
}

/// The run's files of the server's own bytes, downloaded ahead of the
/// engine in its order ([`MEDIA_AT_ONCE`] at a time).
struct Prefetch {
    shared: Arc<Ahead>,
    /// Each file's place in the order, by its server path.
    at: HashMap<String, usize>,
    workers: Vec<JoinHandle<()>>,
}

impl Prefetch {
    fn new(
        client: Arc<Client>,
        wants: Vec<Want>,
        dir: PathBuf,
        room: Room,
        listener: Option<Arc<AtomicBool>>,
    ) -> Prefetch {
        let at = wants.iter().enumerate().map(|(i, w)| (w.path.clone(), i)).collect();
        let (lane, cv, stop) = (Mutex::new(Lane::default()), Condvar::new(), AtomicBool::new(false));
        let shared = Arc::new(Ahead { client, wants, dir, room, lane, cv, stop, listener });
        let workers = (0..MEDIA_AT_ONCE)
            .map(|_| {
                let shared = shared.clone();
                std::thread::spawn(move || fetch_ahead(&shared))
            })
            .collect();
        Prefetch { shared, at, workers }
    }

    /// The file at `path`, waiting for its download: the bytes and their
    /// Content-Type, or why it failed. None when the engine is to fetch it
    /// itself: not one of these, not started yet, or given up. The files
    /// before it the engine passed by: what came of them goes.
    fn take(&self, path: &str) -> Option<Result<(Held, Option<String>), Failed>> {
        let &i = self.at.get(path)?;
        let s = &self.shared;
        let mut lane = s.lane();
        if lane.passed.contains(&i) {
            return None;
        }
        lane.on = lane.on.max(i);
        let before: Vec<usize> = lane.done.keys().copied().filter(|&k| k < i).collect();
        for k in before {
            if let Some(got) = lane.done.remove(&k) {
                got.discard();
            }
            lane.held -= s.wants[k].size;
            lane.passed.insert(k);
        }
        let running: Vec<usize> = lane.running.iter().copied().filter(|&k| k < i).collect();
        lane.passed.extend(running);
        if lane.halted.is_some_and(|h| h < i) {
            lane.halted = None;
        }
        s.cv.notify_all();
        if lane.next <= i {
            // Not started: the engine's own, and the downloads go on past it.
            lane.next = i + 1;
            return None;
        }
        loop {
            if let Some(got) = lane.done.remove(&i) {
                lane.held -= s.wants[i].size;
                s.cv.notify_all();
                return match got {
                    Got::Held(held, content_type) => Some(Ok((held, content_type))),
                    Got::Failed(failed) => Some(Err(failed)),
                    Got::Dropped => None,
                };
            }
            lane = s.cv.wait(lane).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for Prefetch {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        let mut lane = self.shared.lane();
        for (_, got) in lane.done.drain() {
            got.discard();
        }
    }
}

/// A download ahead's worker: the next file in the order while there is
/// room for it, until there is none left, or the run stops.
fn fetch_ahead(s: &Ahead) {
    loop {
        let i = {
            let mut lane = s.lane();
            loop {
                if s.stopped() || s.listener_stopped() || lane.next >= s.wants.len() {
                    return;
                }
                let i = lane.next;
                let near = i <= lane.on + 1;
                let fits = lane.held + s.wants[i].size <= s.room.bytes;
                let room = i <= lane.on + s.room.files && fits;
                if lane.halted.is_none() && (near || room) {
                    break;
                }
                lane = s.cv.wait(lane).unwrap_or_else(|e| e.into_inner());
            }
            let i = lane.next;
            lane.next += 1;
            lane.held += s.wants[i].size;
            lane.running.insert(i);
            i
        };
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| download(s, i)));
        let got = got.unwrap_or(Got::Dropped);
        let mut lane = s.lane();
        lane.running.remove(&i);
        if lane.passed.contains(&i) {
            lane.held -= s.wants[i].size;
            got.discard();
        } else {
            if matches!(&got, Got::Failed(f) if f.stop) {
                lane.halted = Some(lane.halted.map_or(i, |h| h.min(i)));
            }
            lane.done.insert(i, got);
        }
        s.cv.notify_all();
    }
}

/// A spooled download's file: a write this computer refused is told apart
/// from the network's failure.
struct Spool {
    file: File,
    failed: bool,
}

impl Write for Spool {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf).inspect_err(|_| self.failed = true)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// File `i`, downloaded: into memory, or into the scratch folder past
/// [`Room::in_memory`].
fn download(s: &Ahead, i: usize) -> Got {
    let want = &s.wants[i];
    let gives_up = || s.gives_up(i);
    let client = &s.client;
    let mut once = |fetch: &Fetch<'_>, into: &mut dyn Write, held: u64| {
        until_with(client.stream_async(fetch, into, held, &mut |_, _| {}), &gives_up)
    };
    let ended = |got: Result<Fetched, Failed>, held: Held| match got {
        Ok(fetched) => Got::Held(held, fetched.content_type),
        Err(_) if gives_up() => {
            held.discard();
            Got::Dropped
        }
        Err(failed) => {
            held.discard();
            Got::Failed(failed)
        }
    };
    if want.size <= s.room.in_memory {
        let mut bytes = Vec::with_capacity(want.size as usize);
        let got = media(&want.path, want.size, &mut bytes, &gives_up, &mut once);
        return ended(got, Held::Memory(bytes));
    }
    let path = s.dir.join(format!("media-{i}.bin"));
    let Ok(file) = File::create(&path) else { return Got::Dropped };
    let mut spool = Spool { file, failed: false };
    let got = media(&want.path, want.size, &mut spool, &gives_up, &mut once);
    let Spool { file, failed } = spool;
    drop(file);
    if failed {
        let _ = fs::remove_file(&path);
        return Got::Dropped;
    }
    ended(got, Held::Spooled(path))
}

// ── The source ──────────────────────────────────────────────────────────────

/// A folder of this computer's for a run's conversions, gone when dropped.
pub(super) struct Scratch(pub(super) PathBuf);

impl Scratch {
    pub(super) fn new() -> io::Result<Scratch> {
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
    // Dropped before the scratch folder their files are in.
    pipeline: Option<Pipeline>,
    prefetch: Option<Prefetch>,
    room: Room,
    scratch: Scratch,
    /// Conversions made here and not ahead.
    made: u64,
    /// The server's bulk queue, and how many of its conversions it runs at
    /// once; None: a server without one.
    bulk: Option<u32>,
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
            prefetch: None,
            room: Room::RUN,
            scratch: Scratch::new()?,
            made: 0,
            bulk: None,
        })
    }

    /// The origin of the wanted card whose files these are.
    pub(crate) fn origin(&self) -> usize {
        self.origin
    }

    /// The server takes conversions as bulk work, `slots` at once (its
    /// `/api/`'s `features.transcode.bulk`): each is asked `?bulk=1`, out of
    /// its cache and behind its cap, and no more run at a time than it
    /// takes. Set before [`Http::ahead`].
    pub(crate) fn bulk(&mut self, slots: Option<u32>) {
        self.bulk = slots;
    }

    fn pace(&self) -> Pace {
        let limit = self.bulk.map_or(TRANSCODES, |n| (n as usize).clamp(1, TRANSCODES));
        Pace { limit, max_channels: self.max_channels }
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
            self.pipeline = Some(Pipeline::new(self.client.clone(), jobs, self.pace(), stop));
        }
    }

    /// The run's files of this server's own bytes that come over the
    /// network (none read from this computer's disk), in the order it
    /// fetches them: downloaded from now on, ahead of it ([`Prefetch`]).
    /// `stop` is the listener's: once it is set nothing more starts, and
    /// the downloads past the file the engine is on are given up.
    pub(crate) fn prefetch(&mut self, files: &[&WantedFile], stop: Option<Arc<AtomicBool>>) {
        self.prefetch = None;
        let wants: Vec<Want> = files
            .iter()
            .filter(|f| f.origin == self.origin && f.original())
            .map(|f| Want { path: f.ledger.server_path.clone(), size: f.ledger.server_size })
            .collect();
        if !wants.is_empty() {
            let (dir, room) = (self.scratch.0.clone(), self.room);
            self.prefetch = Some(Prefetch::new(self.client.clone(), wants, dir, room, stop));
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
            bulk: self.bulk.is_some(),
            expected_ms: self.lengths.get(&file.ledger.server_path).copied(),
            // Named as its codec: the retag reads the format by its name.
            dest: self.scratch.0.join(format!("{name}.{}", codec.extension())),
        })
    }

    /// The server's bytes: downloaded ahead when they were, else now,
    /// straight into the card's stage file.
    fn original(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
        let l = &file.ledger;
        if let Some(got) = self.prefetch.as_ref().and_then(|p| p.take(&l.server_path)) {
            let (held, content_type) = got?;
            held.pour(into)?;
            return Ok(Fetched { content_type });
        }
        let (client, stop) = (&self.client, &self.stop);
        let stopped = || stop.load(Ordering::Relaxed);
        let mut once = |fetch: &Fetch<'_>, into: &mut dyn Write, held: u64| {
            Some(client.stream(fetch, into, held, &mut |_, _| {}))
        };
        media(&l.server_path, l.server_size, into, &stopped, &mut once)
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
                make(&self.client, &job, self.pace(), &self.stop)?
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

/// A file of this computer's (a conversion, a download ahead) into the
/// card's stage, whole, [`CARD_WRITE`] a read.
pub(super) fn pour(from: &mut dyn Read, into: &mut dyn Write) -> Result<(), Failed> {
    let mut buf = vec![0u8; CARD_WRITE];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(skip(format!("the file on this computer can't be read: {e}")));
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
}

impl Drop for Http {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.pipeline = None;
        self.prefetch = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

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

    #[test]
    fn a_full_bulk_queue_is_waited_out_and_a_restarting_server_is_not() {
        let whole = opus_bytes(30_000, 2, true);
        let words: &[u8] = br#"{"error":"too many bulk transcodes queued - retry later"}"#;
        let full = || reply("503 Service Unavailable", &[("Retry-After", "1")], words);
        // More full answers than a file has tries: none of them is one.
        let ogg = chunked("200 OK", &[("Content-Type", "audio/ogg")], &[&whole]);
        // A restart's 503 says when as well, in no queue's words.
        let restarting = b"mStream is restarting";
        let restart = reply("503 Service Unavailable", &[("Retry-After", "1")], restarting);
        let (base, seen) = canned(vec![full(), full(), full(), ogg, restart]);
        let client = Arc::new(Client::new(&base).unwrap());
        let mut http = Http::new(client, 0, 2).unwrap();
        http.bulk(Some(1));
        let file = wanted("lib/A/01.wav", Bytes::Converted(CONTAINER_OPUS), 1);
        http.lengths(HashMap::from([("lib/A/01.wav".to_string(), 30_000)]));
        let mut into = Vec::new();
        http.fetch(&file, &mut into).expect("made once the queue had room");
        assert_eq!(into, whole);
        for _ in 0..4 {
            let req = seen.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(req.starts_with("GET /transcode/lib/A/01.wav?codec=opus&bitrate=192k&bulk=1 "), "{req}");
        }
        let failed = http.fetch(&file, &mut Vec::new()).unwrap_err();
        assert!(!failed.stop && failed.why.contains("503"), "the file fails, the run not: {failed:?}");
        assert!(seen.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err(), "asked once, not waited out");
    }

    /// A server of conversions whose first request, whichever it is, gets
    /// no answer until `open` is set; any other is answered at once. Each
    /// request's target is handed back as it comes.
    fn held_first(open: Arc<AtomicBool>, stream: Vec<u8>) -> (String, mpsc::Receiver<String>) {
        let (tx, seen) = mpsc::channel::<String>();
        let (tx, count) = (Mutex::new(tx), AtomicU64::new(0));
        let base = routed(move |asked| {
            let _ = tx.lock().unwrap().send(asked.target.clone());
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                while !open.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            chunked("200 OK", &[("Content-Type", "audio/ogg")], &[&stream])
        });
        (base, seen)
    }

    #[test]
    fn a_bulk_queue_is_asked_no_more_than_it_runs() {
        let stream = opus_bytes(10_000, 2, true);
        let open = Arc::new(AtomicBool::new(false));
        let (base, seen) = held_first(open.clone(), stream.clone());
        let client = Arc::new(Client::new(&base).unwrap());
        let mut http = Http::new(client, 0, 2).unwrap();
        assert_eq!(http.pace().limit, TRANSCODES, "no queue: the transfer's own two");
        http.bulk(Some(8));
        assert_eq!(http.pace().limit, TRANSCODES, "never more than the transfer's own");
        http.bulk(Some(1));
        assert_eq!(http.pace().limit, 1, "the server's one");
        let files: Vec<WantedFile> =
            (1..=2).map(|n| wanted(&format!("lib/A/0{n}.wav"), Bytes::Converted(CONTAINER_OPUS), 1)).collect();
        http.lengths((1..=2).map(|n| (format!("lib/A/0{n}.wav"), 10_000)).collect());
        // Made ahead on two threads: the second isn't asked for while the
        // first holds the server's one slot. (Other tests' conversions take
        // this process's turns too: they can only hold the second back.)
        http.ahead(&files.iter().collect::<Vec<_>>());
        let first = seen.recv_timeout(Duration::from_secs(10)).expect("the first conversion asked");
        assert!(first.contains("&bulk=1"), "{first}");
        let early = seen.recv_timeout(Duration::from_millis(600));
        assert!(early.is_err(), "a second while the first runs: {early:?}");
        open.store(true, Ordering::SeqCst);
        let second = seen.recv_timeout(Duration::from_secs(10)).expect("the second, after the first");
        assert!(second.contains("&bulk=1") && second != first, "{second}");
        for file in &files {
            let mut into = Vec::new();
            http.fetch(file, &mut into).unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(into, stream);
        }
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

    // ── Ahead of the card ───────────────────────────────────────────────────

    /// A loopback `/media`: its base URL, each request's path as it comes,
    /// and the most it answered at once.
    type Media = (String, mpsc::Receiver<String>, Arc<AtomicU64>);

    /// The first three files' requests, as [`first`] sorts them.
    const THREE: [&str; 3] = ["/media/lib/A/00.flac", "/media/lib/A/01.flac", "/media/lib/A/02.flac"];

    /// `/media` of `files`: each answered after `wait`, or its own wait in
    /// `slow`, a path in `refused` with that status instead.
    fn media_of(
        files: &[(String, Vec<u8>)],
        wait: Duration,
        slow: HashMap<String, Duration>,
        refused: HashMap<String, &'static str>,
    ) -> Media {
        let (tx, seen) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let files: HashMap<String, Vec<u8>> = files.iter().cloned().collect();
        let (now, most) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let kept = most.clone();
        let base = routed(move |asked| {
            let path = asked.path();
            let _ = tx.lock().unwrap().send(path.clone());
            let rel = path.trim_start_matches("/media/").to_string();
            let at_once = now.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(at_once, Ordering::SeqCst);
            std::thread::sleep(slow.get(&rel).copied().unwrap_or(wait));
            now.fetch_sub(1, Ordering::SeqCst);
            if let Some(status) = refused.get(&rel) {
                return reply(status, &[], br#"{"error":"no"}"#);
            }
            match files.get(&rel) {
                Some(bytes) => reply("200 OK", &[("ETag", "\"e\"")], bytes),
                None => reply("404 Not Found", &[], br#"{"error":"Not Found"}"#),
            }
        });
        (base, seen, kept)
    }

    /// [`media_of`], every file answered after `wait`.
    fn media_at(files: &[(String, Vec<u8>)], wait: Duration) -> Media {
        media_of(files, wait, HashMap::new(), HashMap::new())
    }

    /// The first `n` requests, sorted.
    fn first(seen: &mpsc::Receiver<String>, n: usize) -> Vec<String> {
        let wait = Duration::from_secs(5);
        let mut asked: Vec<String> = (0..n).map(|_| seen.recv_timeout(wait).unwrap()).collect();
        asked.sort();
        asked
    }

    fn originals(files: &[(String, Vec<u8>)]) -> Vec<WantedFile> {
        let original = Bytes::Original { ogg_opus: false };
        files.iter().map(|(p, b)| wanted(p, original, b.len() as u64)).collect()
    }

    fn named(n: usize, size: usize) -> Vec<(String, Vec<u8>)> {
        (0..n).map(|k| (format!("lib/A/{k:02}.flac"), vec![k as u8 + 1; size + k])).collect()
    }

    /// The files a test's scratch folder holds that were downloaded ahead.
    fn spooled(http: &Http) -> usize {
        let names = fs::read_dir(&http.scratch.0).unwrap().flatten();
        names.filter(|e| e.file_name().to_string_lossy().starts_with("media-")).count()
    }

    fn eventually(what: &str, mut done: impl FnMut() -> bool) {
        let t0 = std::time::Instant::now();
        while !done() {
            assert!(t0.elapsed() < Duration::from_secs(10), "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_servers_files_download_ahead_three_at_a_time_in_the_runs_order() {
        let files = named(8, 1000);
        let (base, seen, most) = media_at(&files, Duration::from_millis(150));
        let mut http = Http::new(Arc::new(Client::new(&base).unwrap()), 0, 2).unwrap();
        let wanted = originals(&files);
        http.prefetch(&wanted.iter().collect::<Vec<_>>(), None);
        // Asked before the engine asks for any: the first three, at once.
        assert_eq!(first(&seen, 3), THREE);
        for (n, file) in wanted.iter().enumerate() {
            let mut into = Vec::new();
            let got = http.fetch(file, &mut into).unwrap_or_else(|e| panic!("{n}: {e:?}"));
            assert_eq!(got.content_type, None);
            assert!(into == files[n].1, "{n}: its bytes");
        }
        assert_eq!(most.load(Ordering::SeqCst), 3, "three at once, never more");
        assert_eq!(seen.try_iter().count(), 5, "each file asked for once");
    }

    #[test]
    fn a_large_file_waits_here_and_nothing_starts_past_the_room_ahead() {
        let files = named(5, 3000);
        let (base, seen, _) = media_at(&files, Duration::ZERO);
        let mut http = Http::new(Arc::new(Client::new(&base).unwrap()), 0, 2).unwrap();
        // One file's room, every file downloaded into a file here.
        http.room = Room { files: 16, bytes: 3500, in_memory: 1000 };
        let wanted = originals(&files);
        http.prefetch(&wanted.iter().collect::<Vec<_>>(), None);
        let here = |http: &Http, k: usize| http.scratch.0.join(format!("media-{k}.bin")).exists();
        let (wait, quiet) = (Duration::from_secs(5), Duration::from_millis(500));
        // The engine's next file and the one after it go whatever their
        // size; nothing past them.
        assert_eq!(first(&seen, 2), ["/media/lib/A/00.flac", "/media/lib/A/01.flac"]);
        assert!(seen.recv_timeout(quiet).is_err(), "no room for a third");
        eventually("two files downloaded here", || spooled(&http) == 2);

        // The engine's first, from its file here, which goes.
        let mut into = Vec::new();
        http.fetch(&wanted[0], &mut into).unwrap();
        assert!(into == files[0].1);
        assert!(!here(&http, 0));
        assert!(seen.recv_timeout(quiet).is_err(), "the next is in; no room past it");
        let mut into = Vec::new();
        http.fetch(&wanted[1], &mut into).unwrap();
        assert!(into == files[1].1);
        assert_eq!(seen.recv_timeout(wait).unwrap(), "/media/lib/A/02.flac");
        assert!(seen.recv_timeout(quiet).is_err(), "no room for a fourth");

        // One passed by (the engine skipped it): what came of it goes. The
        // one asked for, not started yet, is the engine's own; the one past
        // it goes ahead.
        let mut into = Vec::new();
        http.fetch(&wanted[3], &mut into).unwrap();
        assert!(into == files[3].1);
        assert_eq!(first(&seen, 2), ["/media/lib/A/03.flac", "/media/lib/A/04.flac"]);
        eventually("the passed file gone, the last one here", || !here(&http, 2) && here(&http, 4));
        // Whatever the run leaves goes with it.
        let dir = http.scratch.0.clone();
        drop(http);
        assert!(!dir.exists(), "nothing left on this computer");
        assert_eq!(seen.try_iter().count(), 0);
    }

    #[test]
    fn a_failure_ahead_is_the_engines_at_its_file_and_one_that_stops_the_run_halts_them() {
        // A file gone skips that file only.
        let files = named(4, 500);
        let gone = HashMap::from([("lib/A/01.flac".to_string(), "404 Not Found")]);
        let (base, _, _) = media_of(&files, Duration::ZERO, HashMap::new(), gone);
        let mut http = Http::new(Arc::new(Client::new(&base).unwrap()), 0, 2).unwrap();
        let wanted = originals(&files);
        http.prefetch(&wanted.iter().collect::<Vec<_>>(), None);
        for (n, file) in wanted.iter().enumerate() {
            let got = http.fetch(file, &mut Vec::new());
            match n {
                1 => assert!(!got.unwrap_err().stop, "a file gone skips that file"),
                _ => assert!(got.is_ok(), "{n}: {got:?}"),
            }
        }

        // Lost sign-in stops the run: nothing past it starts, the files
        // before it still come.
        let files = named(5, 500);
        let refused = HashMap::from([("lib/A/02.flac".to_string(), "401 Unauthorized")]);
        let slow: HashMap<String, Duration> =
            (0..2).map(|k| (format!("lib/A/{k:02}.flac"), Duration::from_millis(400))).collect();
        let (base, seen, _) = media_of(&files, Duration::ZERO, slow, refused);
        let mut http = Http::new(Arc::new(Client::new(&base).unwrap()), 0, 2).unwrap();
        let wanted = originals(&files);
        http.prefetch(&wanted.iter().collect::<Vec<_>>(), None);
        assert_eq!(first(&seen, 3), THREE);
        assert!(seen.recv_timeout(Duration::from_millis(800)).is_err(), "nothing past the refusal");
        assert!(http.fetch(&wanted[0], &mut Vec::new()).is_ok());
        assert!(http.fetch(&wanted[1], &mut Vec::new()).is_ok());
        let stopped = http.fetch(&wanted[2], &mut Vec::new()).unwrap_err();
        assert!(stopped.stop && stopped.why.contains("sign-in"), "{stopped:?}");
    }

    #[test]
    fn the_listeners_stop_gives_up_the_downloads_past_the_engines_file() {
        let files = named(5, 800);
        let (base, seen, _) = media_at(&files, Duration::from_millis(500));
        let mut http = Http::new(Arc::new(Client::new(&base).unwrap()), 0, 2).unwrap();
        let wanted = originals(&files);
        let stop = Arc::new(AtomicBool::new(false));
        http.prefetch(&wanted.iter().collect::<Vec<_>>(), Some(stop.clone()));
        assert_eq!(first(&seen, 3), THREE);
        let setter = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                stop.store(true, Ordering::SeqCst);
            })
        };
        // The engine's file comes whole; the two past it are given up, and
        // nothing more starts.
        let mut into = Vec::new();
        http.fetch(&wanted[0], &mut into).unwrap();
        assert!(into == files[0].1);
        setter.join().unwrap();
        assert!(seen.recv_timeout(Duration::from_millis(600)).is_err(), "nothing started after");
        // One the engine asks for all the same is its own.
        let mut into = Vec::new();
        http.fetch(&wanted[1], &mut into).unwrap();
        assert!(into == files[1].1);
        assert_eq!(seen.try_iter().collect::<Vec<_>>(), ["/media/lib/A/01.flac"]);
        assert_eq!(spooled(&http), 0);
    }
}
