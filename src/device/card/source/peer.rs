//! A federation peer's files (the transfer plan's 2.15, part B9): a server
//! the user's own one federates with, read through it.
//!
//! **The listing** ([`list`]) goes through the parent's browse proxy, as no
//! peer serves its sync manifest to a key or a guest: each library's own
//! folder, then `file-explorer/recursive` one top-level folder a call (to
//! stay inside the proxy's 15 s deadline), then `metadata/batch` in chunks,
//! which gives each file's size, `modified` and hashes — what tells a file
//! changed. A file the folders hold and the peer's database doesn't yet is
//! a scan behind: no path missing from that listing is a deletion.
//!
//! **The bytes** ([`reach`]): the direct guest path first — the parent's
//! `peers/:id/access` mints a 24 h guest token, and the peer's own tunnel
//! is dialled with it ([`Dial`]) — every request saying `X-mStream-Purpose:
//! copy`, so a key that forbids copies refuses it (403), and the transfer
//! stops for that peer and says why. Else the parent's stream proxy, which
//! passes the copy purpose on from the card transfer's batch of mStream
//! changes; an older parent's can't, so a peer reachable only through it
//! is the listener's call, asked once, skipped until they allow it.
//!
//! **What a peer allows** ([`Peer`]): at most two of its files at once in
//! this process (a key's three streams are shared with playback); a 429
//! sorted by its Retry-After when it has one (a few seconds: the stream
//! cap; longer: the day's quota), else by its words; the stream cap waited
//! out, three times at most; the quota, or this computer's daily budget
//! ([`super::super::budget`]), parks the peer: its file's bytes so far stay
//! in the card's partial (2.12.8), the run goes on with the other sources'
//! files and ends at a checkpoint that keeps the peer's in its plan, and
//! the peer is asked again after the next 00:00 UTC. Every byte its clients
//! read is counted against its day, and nothing is asked of it once the
//! day is spent ([`Gate`]): not a file, a cover or a vector. A budget the
//! book can't keep (`budget.json` unwritable) stops the peer for the run.
//!
//! **A probe** of a peer's file is a one-byte GET, whichever way its bytes
//! come: its federation gate admits only GETs of `/media/` (mStream's
//! `federation-auth.js` refuses a HEAD with a 403), and a HEAD through an
//! older parent's stream proxy may pull the whole file.
//!
//! **Conversions** are made on this computer ([`super::ffmpeg`]): the
//! peer's bytes fetched whole into a file here, converted, held to their
//! length, given back what the conversion dropped, then poured onto the
//! card. A conversion whose source is past what is left of the day waits
//! for the next one: it can't be taken up partway.

use std::any::Any;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::super::budget::{self, Clock, Day, PeerKey, Spend};
use super::super::engine::{Failed, Fetched, Fetcher, Have, Part, WantedFile};
use super::super::names::Bytes;
use super::super::plan::{codec_of, extension};
use super::ffmpeg::Convert;
use super::http::{AsMade, Finish, Scratch, passing, pause, pour, skip};
use super::length::{self, FileAt};
use crate::api::transfer::fetch::{FAT32_MAX, Fetch, FetchError, LimitKind, Limited, Rewind};
use crate::api::transfer::{ManifestEntry, Metadata};
use crate::api::types::DirectAccessResponse;
use crate::api::urls::TransferBitrate;
use crate::api::{ApiError, Client};

/// Tries of one file, its connection cut: a first, then two more.
const TRIES: u32 = 3;
/// Waits for a peer's stream cap, per file.
const STREAM_TRIES: u32 = 3;
/// A 429 whose Retry-After is longer than this is the day's quota.
const STREAM_RETRY_MAX: u64 = 60;
/// A peer's files fetched at once in this process.
const AT_ONCE: usize = 2;
/// The files a folder listing may hold beside the audio: never a track.
const LISTS: [&str; 3] = ["m3u", "m3u8", "pls"];

// ── The listing ─────────────────────────────────────────────────────────────

/// A peer's libraries, listed (see the module's note).
#[derive(Clone, Debug, Default)]
pub(crate) struct Listed {
    /// The files the peer's database has, as manifest entries: the ledger
    /// fields its metadata gives. No row ids (0): two files that would take
    /// one name are named in path order.
    pub(crate) entries: Vec<ManifestEntry>,
    /// Each one's full metadata.
    pub(crate) metadata: HashMap<String, Option<Metadata>>,
    /// Files the folders hold that the database doesn't yet.
    pub(crate) unscanned: usize,
}

/// A peer's file as a manifest entry, from its full metadata.
pub(crate) fn entry_of(path: &str, m: &Metadata) -> ManifestEntry {
    ManifestEntry {
        filepath: path.to_string(),
        id: 0,
        metadata: m.clone(),
        file_size: m.file_size,
        modified: m.modified,
        hash: m.hash.clone(),
        audio_hash: m.audio_hash.clone(),
        hash_v: None,
        format: m.format.clone(),
        album_id: None,
        artist_id: None,
        created_at: m.created_at.clone(),
    }
}

/// `libraries` of the peer `client` reads through its parent's browse
/// proxy, each call counted on its meter.
pub(crate) fn list(client: &Client, libraries: &[String]) -> Result<Listed, ApiError> {
    let mut paths: Vec<String> = Vec::new();
    for vpath in libraries {
        let top = client.folder(vpath)?;
        paths.extend(top.files.iter().map(|f| format!("{vpath}/{}", f.name)));
        for dir in &top.directories {
            paths.extend(client.recursive_files(&format!("{vpath}/{}", dir.name))?);
        }
    }
    paths.retain(|p| !LISTS.contains(&extension(p).as_str()));
    paths.sort();
    paths.dedup();
    let metadata = client.metadata_batch(&paths)?;
    let mut listed = Listed::default();
    for path in &paths {
        match metadata.get(path).and_then(Option::as_ref) {
            Some(m) => listed.entries.push(entry_of(path, m)),
            None => listed.unscanned += 1,
        }
    }
    listed.metadata = metadata;
    Ok(listed)
}

// ── The way to its bytes ────────────────────────────────────────────────────

/// The peer's own tunnel, dialled with a guest ticket: where its loopback
/// bridge listens, and the token every request to it carries.
pub(crate) struct Tunnel {
    pub(crate) base: String,
    pub(crate) local_token: Option<String>,
    /// iroh carries it through a relay server, not peer to peer.
    pub(crate) relay: bool,
    /// What holds it open; dropped, it closes.
    pub(crate) _keep: Option<Box<dyn Any>>,
}

/// Dials a peer's tunnel.
pub(crate) trait Dial {
    /// The tunnel of `ticket` (the parent's `mstrfedg1:` guest ticket).
    fn dial(&self, ticket: &str) -> Result<Tunnel, String>;
}

/// The shared tunnel client's dial, the player's own (Quick Connect's).
pub(crate) struct IrohDial;

/// An iroh tunnel, shut down when dropped.
struct Open(Option<iroh_tunnel::Tunnel>);

impl Drop for Open {
    fn drop(&mut self) {
        if let (Some(tunnel), Ok(rt)) = (self.0.take(), crate::runtime::handle()) {
            tunnel.begin_shutdown(rt);
        }
    }
}

impl Dial for IrohDial {
    fn dial(&self, ticket: &str) -> Result<Tunnel, String> {
        let dialled = crate::runtime::block_on(iroh_tunnel::connect_tunnel(ticket, 0))?;
        let tunnel = dialled.map_err(|e| e.to_string())?;
        let (base, token) = (tunnel.local_url(), tunnel.local_token());
        let relay = tunnel.path_kind() == 2;
        let keep: Box<dyn Any> = Box::new(Open(Some(tunnel)));
        Ok(Tunnel { base, local_token: Some(token), relay, _keep: Some(keep) })
    }
}

/// How a peer's bytes are reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reach {
    /// The peer's own tunnel with a guest token, each request saying it is
    /// a copy.
    Direct,
    /// The parent's stream proxy, which passes the copy purpose on.
    Proxy,
    /// An older parent's stream proxy, which can't say the bytes are a
    /// copy: the listener allowed it.
    Undeclared,
}

impl Reach {
    /// As one stable word, for the debug command and the tests.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Reach::Direct => "direct",
            Reach::Proxy => "proxy",
            Reach::Undeclared => "proxy-undeclared",
        }
    }
}

/// The parent's word on the direct path: the guest ticket to dial and the
/// guest token to send, or why not.
pub(crate) fn ticket(answer: &DirectAccessResponse) -> Result<(String, String), String> {
    if !answer.direct {
        let why = answer.reason.clone();
        return Err(why.unwrap_or_else(|| "the parent offers no direct path".into()));
    }
    match (&answer.direct_ticket, &answer.guest_token) {
        (Some(ticket), Some(token)) if ticket.starts_with("mstrfedg") && !token.is_empty() => {
            Ok((ticket.clone(), token.clone()))
        }
        _ => Err("the access answer is not a guest ticket".into()),
    }
}

/// The bytes' way to a peer, as [`reach`] found it.
pub(crate) struct Reached {
    pub(crate) client: Client,
    pub(crate) reach: Reach,
    pub(crate) tunnel: Option<Tunnel>,
    /// Why the direct path wasn't taken.
    pub(crate) direct_refused: Option<String>,
}

/// The way to peer `id`'s bytes (see the module's note): `parent` is the
/// client of its parent; `declares` says the parent's stream proxy passes
/// the copy purpose on (the card transfer's batch of mStream changes);
/// `undeclared` that the listener allows copies through one that can't.
/// `dial`: None plans only, the guest path's tunnel not dialled (a dry
/// run). Err: no way the listener allowed, and why.
pub(crate) fn reach(
    parent: &Client,
    id: i64,
    declares: bool,
    undeclared: bool,
    dial: Option<&dyn Dial>,
) -> Result<Reached, String> {
    let access = parent.federation_access(id, false).map_err(|e| e.to_string());
    let refused = match access.and_then(|a| ticket(&a)) {
        Ok((ticket, token)) => match dial.map(|d| d.dial(&ticket)) {
            None => {
                let (client, reach) = (parent.for_peer(id), Reach::Direct);
                return Ok(Reached { client, reach, tunnel: None, direct_refused: None });
            }
            Some(Ok(tunnel)) => {
                let client = Client::new(&tunnel.base).map_err(|e| e.to_string())?;
                let client = client.with_token(Some(token));
                let client = client.with_local_token(tunnel.local_token.clone()).copying(true);
                let tunnel = Some(tunnel);
                return Ok(Reached { client, reach: Reach::Direct, tunnel, direct_refused: None });
            }
            Some(Err(why)) => format!("the peer's tunnel didn't open: {why}"),
        },
        Err(why) => why,
    };
    let reach = match (declares, undeclared) {
        (true, _) => Reach::Proxy,
        (false, true) => Reach::Undeclared,
        (false, false) => return Err(refused),
    };
    let client = parent.for_peer(id).copying(reach == Reach::Proxy);
    Ok(Reached { client, reach, tunnel: None, direct_refused: Some(refused) })
}

// ── What holds a peer back ──────────────────────────────────────────────────

/// Why a peer's files aren't fetched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Hold {
    /// Its day is spent, by the budget or by its quota: asked again at
    /// `until` (Unix seconds). Its files wait in the plan.
    Wait { why: String, until: u64 },
    /// It refuses this key's copies, or its guest token, or is out of
    /// reach: its files are skipped, and the next run tries again.
    Stop(String),
}

impl Hold {
    pub(crate) fn why(&self) -> &str {
        match self {
            Hold::Wait { why, .. } | Hold::Stop(why) => why,
        }
    }
}

/// A peer's hold, shared by its source and what else asks it for bytes in
/// a run (its covers, its vectors), and what is left of its day.
#[derive(Clone, Debug, Default)]
pub(crate) struct Gate {
    hold: Arc<Mutex<Option<Hold>>>,
    day: Option<Day>,
}

impl Gate {
    /// The gate of a peer whose day is `day`.
    pub(crate) fn of(day: Day) -> Gate {
        Gate { hold: Arc::default(), day: Some(day) }
    }

    pub(crate) fn hold(&self) -> Option<Hold> {
        self.hold.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set(&self, hold: Hold) {
        *self.hold.lock().unwrap_or_else(|e| e.into_inner()) = Some(hold);
    }

    /// The bytes the peer may still be asked for now: none while something
    /// holds it or its day is spent; None: no cap.
    pub(crate) fn room(&self) -> Option<u64> {
        match self.hold() {
            Some(_) => Some(0),
            None => self.day.as_ref().and_then(Day::left),
        }
    }

    /// It may be asked.
    pub(crate) fn open(&self) -> bool {
        self.room() != Some(0)
    }
}

/// What a 429 asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Limit {
    /// The key's stream cap: ask again after this long.
    Streams(Duration),
    /// The day's quota: Retry-After's seconds to the peer's midnight, when
    /// it said.
    Quota(Option<u64>),
}

/// A 429 sorted: by its Retry-After when it has one (a minute or less is
/// the stream cap's, mStream's 5 s; more is the quota's, the seconds to the
/// peer's UTC midnight), else by its words, an unknown one waited out as a
/// stream cap's for `stream_wait`. A quota's 429 a minute before midnight
/// is so waited out too, and the day is a new one.
pub(crate) fn limit(l: &Limited, stream_wait: Duration) -> Limit {
    match (l.retry_after, l.kind) {
        (Some(wait), _) if wait.as_secs() > STREAM_RETRY_MAX => Limit::Quota(Some(wait.as_secs())),
        (Some(wait), _) => Limit::Streams(wait),
        (None, LimitKind::Quota) => Limit::Quota(None),
        (None, _) => Limit::Streams(stream_wait),
    }
}

// ── At most two at once ─────────────────────────────────────────────────────

/// Each peer's fetches running in this process.
static RUNNING: (Mutex<Vec<(PeerKey, usize)>>, Condvar) = (Mutex::new(Vec::new()), Condvar::new());

/// A fetch's turn of a peer, until it is dropped.
struct Turn(PeerKey);

impl Turn {
    /// Waits while [`AT_ONCE`] of the peer's fetches run; None when `stop`
    /// is set first.
    fn take(key: PeerKey, stop: &AtomicBool) -> Option<Turn> {
        let (lock, cv) = &RUNNING;
        let mut running = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let n = running.iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n);
            if n < AT_ONCE {
                match running.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, n)) => *n += 1,
                    None => running.push((key, 1)),
                }
                return Some(Turn(key));
            }
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            let wait = cv.wait_timeout(running, Duration::from_millis(100));
            running = wait.unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (lock, cv) = &RUNNING;
        let mut running = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, n)) = running.iter_mut().find(|(k, _)| *k == self.0) {
            *n = n.saturating_sub(1);
        }
        running.retain(|(_, n)| *n > 0);
        cv.notify_all();
    }
}

// ── Bytes held to the day ───────────────────────────────────────────────────

/// A partial that takes bytes while the day's budget lasts: the chunk that
/// came while some was left is written (the meter counted it as it came),
/// the next one refused; and while the book takes them, the first that it
/// can't refused too.
struct CapPart<'a> {
    into: &'a mut dyn Part,
    spend: &'a mut Spend,
    spent: bool,
    /// Why the book couldn't take the meter's bytes.
    unkept: Option<String>,
}

impl Write for CapPart<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.spend.left_before(buf.len() as u64) == Some(0) {
            self.spent = true;
            return Err(io::Error::other("the day's budget for the peer is spent"));
        }
        if let Err(why) = self.spend.tick() {
            self.unkept = Some(why);
            return Err(io::Error::other("the peer's budget can't be kept"));
        }
        self.into.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.into.flush()
    }
}

impl Rewind for CapPart<'_> {
    fn start_at(&mut self, start: u64, validator: Option<&str>) -> io::Result<()> {
        self.into.answered(start, validator)
    }
}

/// A writer that is only ever written on, as a partial: a fetch through
/// [`Fetcher::fetch`] rather than into the card's `resume/`.
struct Fresh<'a> {
    into: &'a mut dyn Write,
    written: u64,
}

impl Write for Fresh<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.into.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.into.flush()
    }
}

impl Part for Fresh<'_> {
    fn answered(&mut self, from: u64, _validator: Option<&str>) -> io::Result<()> {
        match from == self.written {
            true => Ok(()),
            false => Err(io::Error::other("the file changed since its first bytes came")),
        }
    }
}

// ── The source ──────────────────────────────────────────────────────────────

/// What a [`Peer`] is made of, besides its client.
pub(crate) struct Setup {
    /// An index into the wanted card's origins.
    pub(crate) origin: usize,
    pub(crate) reach: Reach,
    pub(crate) spend: Spend,
    pub(crate) gate: Gate,
    /// The conversion on this computer; None: no ffmpeg, so the files that
    /// need one were skipped at the plan.
    pub(crate) convert: Option<Arc<dyn Convert>>,
    /// What the player plays: a conversion of more channels is refused.
    pub(crate) max_channels: u32,
    pub(crate) clock: Clock,
    /// A resume's few random minutes.
    pub(crate) jitter: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// A stream cap's 429 that names no Retry-After waits this long.
    pub(crate) stream_wait: Duration,
}

/// A peer's files, as the engine's source of them (see the module's note).
pub(crate) struct Peer {
    s: Setup,
    client: Arc<Client>,
    /// A whole day's budget, read when the run began; None: no cap.
    cap: Option<u64>,
    finish: Box<dyn Finish>,
    /// Each conversion's expected length, by its source's server path.
    lengths: HashMap<String, u64>,
    stop: Arc<AtomicBool>,
    made: u64,
    // Dropped after the client, its last user.
    scratch: Scratch,
    tunnel: Option<Tunnel>,
}

impl Peer {
    /// The source of the peer `client` reaches (a [`Reached`]'s), keeping
    /// `tunnel` open while it lives.
    pub(crate) fn new(s: Setup, client: Arc<Client>, tunnel: Option<Tunnel>) -> io::Result<Peer> {
        let cap = s.spend.cap();
        let (finish, lengths) = (Box::new(AsMade), HashMap::new());
        let (stop, scratch) = (Arc::new(AtomicBool::new(false)), Scratch::new()?);
        Ok(Peer { s, client, cap, finish, lengths, stop, made: 0, scratch, tunnel })
    }

    pub(crate) fn origin(&self) -> usize {
        self.s.origin
    }

    /// Its id in its parent's peer list.
    pub(crate) fn peer_id(&self) -> u32 {
        self.s.spend.key().peer_id
    }

    pub(crate) fn gate(&self) -> &Gate {
        &self.s.gate
    }

    /// The peer's bytes come through a relay: a multi-gigabyte pull is
    /// worth a warning.
    pub(crate) fn relayed(&self) -> bool {
        self.tunnel.as_ref().is_some_and(|t| t.relay)
    }

    /// What a conversion dropped, put back before the card takes it.
    pub(crate) fn finish_with(&mut self, finish: Box<dyn Finish>) {
        self.finish = finish;
    }

    /// The length each conversion must come to, by its source's server path.
    pub(crate) fn lengths(&mut self, lengths: HashMap<String, u64>) {
        self.lengths = lengths;
    }

    /// What is left of the peer's day now; None: no cap.
    pub(crate) fn left(&self) -> Option<u64> {
        self.s.spend.left()
    }

    /// A whole day's budget for the peer, as the run began; None: no cap.
    pub(crate) fn cap(&self) -> Option<u64> {
        self.cap
    }

    /// At the run's end: the meter's last bytes into the book, and a day
    /// spent by then (by its covers, its vectors) parks the peer as a file
    /// cut at the budget does.
    pub(crate) fn settle(&mut self) {
        if let Err(why) = self.s.spend.sync() {
            self.unkept(why);
        } else if self.s.gate.hold().is_none() && self.s.spend.left() == Some(0) {
            self.spent();
        }
    }

    /// Nothing more is asked of the peer: something holds it, or its day is
    /// spent, which parks it.
    fn held(&mut self) -> Result<(), Failed> {
        if let Some(hold) = self.s.gate.hold() {
            return Err(skip(hold.why().to_string()));
        }
        match self.s.spend.left() {
            Some(0) => Err(self.spent()),
            _ => Ok(()),
        }
    }

    /// The book can't keep the peer's bytes (`why`): it is stopped for the
    /// run, as nothing it pulls could be counted.
    fn unkept(&mut self, why: String) -> Failed {
        let why = format!("its budget can't be kept: {why}");
        self.s.gate.set(Hold::Stop(why.clone()));
        skip(why)
    }

    /// The peer's day is spent: it waits, in the book, until `retry_after`
    /// seconds from now (the peer's word) or the next 00:00 UTC, and a few
    /// minutes. A wait the book can't keep stops it instead: the next run
    /// would ask it again the same day.
    fn wait(&mut self, why: String, retry_after: Option<u64>) -> Failed {
        let now = (self.s.clock)();
        let until = budget::resume_after(now, retry_after, (self.s.jitter)());
        if let Err(e) = self.s.spend.park(until) {
            return self.unkept(e);
        }
        self.s.gate.set(Hold::Wait { why: why.clone(), until });
        skip(why)
    }

    fn spent(&mut self) -> Failed {
        self.wait("the day's budget for the peer is spent".into(), None)
    }

    /// A 429: None when its wait is over and the file may be asked again.
    fn limited(&mut self, l: &Limited, streams: &mut u32) -> Option<Failed> {
        match limit(l, self.s.stream_wait) {
            Limit::Streams(_) if *streams >= STREAM_TRIES => {
                Some(skip(format!("the peer's streams stayed busy: {}", l.message)))
            }
            Limit::Streams(wait) => {
                *streams += 1;
                let wait = wait.max(Duration::from_millis(1));
                let until = std::time::Instant::now() + wait;
                while std::time::Instant::now() < until && !self.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20).min(wait));
                }
                None
            }
            Limit::Quota(retry_after) => {
                let why = format!("the peer's daily quota is spent: {}", l.message);
                Some(self.wait(why, retry_after))
            }
        }
    }

    /// A failure past the tries, as the run takes it: the peer's own refusals
    /// — a key that forbids copies, any other 403, a guest pass refused, a
    /// peer out of reach (its parent's proxy says so with a 502) — hold it
    /// for the run; the server's sign-in lost stops the run; anything of one
    /// file's skips that file.
    fn failed(&mut self, e: FetchError) -> Failed {
        let hold = if e.copies_refused() {
            Some("the peer's owner doesn't let this server's key copy its files".to_string())
        } else {
            match &e {
                FetchError::Api(ApiError::Forbidden(_)) => Some(format!("the peer refused: {e}")),
                FetchError::Api(ApiError::Unauthorized) if self.s.reach == Reach::Direct => {
                    Some(format!("the peer refused its guest pass: {e}"))
                }
                FetchError::Api(ApiError::Unauthorized) => {
                    let why = format!("the server no longer takes this sign-in: {e}");
                    return super::http::stop(why);
                }
                FetchError::Api(ApiError::Network(_))
                | FetchError::Api(ApiError::Server { status: 502 | 504, .. })
                | FetchError::Stalled(_) => Some(format!("the peer is out of reach: {e}")),
                _ => None,
            }
        };
        if let Some(why) = hold {
            self.s.gate.set(Hold::Stop(why.clone()));
            return skip(why);
        }
        skip(e.to_string())
    }

    /// The peer's bytes into the card's partial, which holds `have`'s of
    /// them already when there is one, while the day lasts.
    fn part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn Part,
    ) -> Result<Fetched, Failed> {
        self.held()?;
        let turn = Turn::take(self.s.spend.key(), &self.stop);
        let Some(_turn) = turn else { return Err(skip("stopped")) };
        let l = &file.ledger;
        let (mut held, mut validator) = match have {
            Some(h) => (h.bytes, Some(h.validator.clone())),
            None => (0, None),
        };
        let (mut attempt, mut streams) = (1, 0);
        let client = self.client.clone();
        loop {
            let fetch = Fetch::file(&l.server_path).expecting(l.server_size);
            let fetch = fetch.resuming(validator.as_deref());
            let spend = &mut self.s.spend;
            let mut capped = CapPart { into: &mut *into, spend, spent: false, unkept: None };
            let got = client.stream_part(&fetch, &mut capped, held, &mut |_, _| {});
            let (spent, unkept) = (capped.spent, capped.unkept.take());
            // A file that came whole is the card's, whatever the book says.
            let unkept = unkept.or_else(|| self.s.spend.sync().err()).map(|e| self.unkept(e));
            let f = match got {
                Ok(got) => return Ok(Fetched { content_type: got.content_type }),
                Err(f) => f,
            };
            (held, validator) = (f.partial.len, f.partial.validator.clone());
            if let Some(unkept) = unkept {
                return Err(unkept);
            }
            if spent {
                return Err(self.spent());
            }
            match &f.error {
                FetchError::Limited(l) => match self.limited(l, &mut streams) {
                    Some(failed) => return Err(failed),
                    None => continue,
                },
                e if passing(e) && attempt < TRIES => {
                    pause(attempt as u64, &self.stop);
                    attempt += 1;
                }
                _ => return Err(self.failed(f.error)),
            }
        }
    }

    /// The peer's bytes of `file` into `dest` on this computer, whole: a
    /// conversion's source. Cut connections are taken up in the run.
    fn source(&mut self, file: &WantedFile, dest: &Path) -> Result<(), Failed> {
        let l = &file.ledger;
        let (mut attempt, mut streams) = (1, 0);
        let mut validator: Option<String> = None;
        let client = self.client.clone();
        loop {
            let fetch = Fetch::file(&l.server_path).expecting(l.server_size).cap(FAT32_MAX);
            let fetch = fetch.resuming(validator.as_deref());
            let got = client.download(&fetch, dest, &mut |_, _| {});
            let unkept = self.s.spend.sync().err().map(|e| self.unkept(e));
            let f = match got {
                Ok(_) => return Ok(()),
                Err(f) => f,
            };
            if let Some(unkept) = unkept {
                return Err(unkept);
            }
            validator = f.partial.validator.clone();
            match &f.error {
                FetchError::Limited(l) => match self.limited(l, &mut streams) {
                    Some(failed) => return Err(failed),
                    None => continue,
                },
                e if passing(e) && attempt < TRIES => {
                    pause(attempt as u64, &self.stop);
                    attempt += 1;
                }
                _ => return Err(self.failed(f.error)),
            }
        }
    }

    /// A conversion on this computer: the peer's bytes fetched whole, made
    /// into the card's codec, held to their length, given back what the
    /// conversion dropped, and poured into `into`.
    fn converted(
        &mut self,
        file: &WantedFile,
        container: u8,
        into: &mut dyn Write,
    ) -> Result<Fetched, Failed> {
        self.held()?;
        let Some(convert) = self.s.convert.clone() else {
            return Err(skip("no ffmpeg on this computer to convert it with"));
        };
        let codec = codec_of(container).ok_or_else(|| skip("not a conversion a sync makes"))?;
        let bitrate = TransferBitrate::from_kbps(file.kbps as u32)
            .ok_or_else(|| skip(format!("no conversion at {}k", file.kbps)))?;
        let size = file.ledger.server_size;
        if self.cap.is_some_and(|cap| size > cap) {
            return Err(skip("its source is larger than a day's budget for the peer"));
        }
        if self.s.spend.left().is_some_and(|left| left < size) {
            return Err(self.spent());
        }
        let turn = Turn::take(self.s.spend.key(), &self.stop);
        let Some(_turn) = turn else { return Err(skip("stopped")) };
        self.made += 1;
        let ext = extension(&file.ledger.server_path);
        let from = self.scratch.0.join(format!("source-{}.{ext}", self.made));
        let to = self.scratch.0.join(format!("made-{}.{}", self.made, codec.extension()));
        let fetched = self.source(file, &from);
        let made = fetched.and_then(|()| {
            convert.convert(&from, &to, codec, bitrate, &self.stop).map_err(skip)
        });
        let _ = fs::remove_file(&from);
        let poured = made.and_then(|()| {
            let at = FileAt::open(&to).map_err(|e| e.to_string());
            let heard = at.and_then(|at| length::heard(codec, &at));
            let expected = self.lengths.get(&file.ledger.server_path).copied();
            let checked = heard.and_then(|h| length::check(&h, expected, self.s.max_channels));
            checked.map_err(|e| skip(format!("the conversion is {e}")))?;
            self.finish.finish(file, &to).map_err(skip)?;
            let mut made = File::open(&to).map_err(|e| skip(e.to_string()))?;
            pour(&mut made, into)
        });
        let _ = fs::remove_file(&to);
        poured.map(|()| Fetched { content_type: Some(codec.content_type().to_string()) })
    }
}

impl Fetcher for Peer {
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
        if file.origin != self.s.origin {
            return Err(skip("a file of another server"));
        }
        match file.bytes {
            Bytes::Converted(container) => self.converted(file, container, into),
            _ => self.part(file, None, &mut Fresh { into, written: 0 }),
        }
    }

    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed> {
        if file.origin != self.s.origin {
            return Err(skip("a file of another server"));
        }
        self.held()?;
        let read = self.client.read_range(&file.ledger.server_path, start, len as u64);
        if let Err(why) = self.s.spend.tick() {
            self.unkept(why);
        }
        read.map(|r| r.bytes).map_err(|e| skip(e.to_string()))
    }

    fn resumable(&self, file: &WantedFile) -> bool {
        file.origin == self.s.origin && file.original()
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

    fn waits(&self, origin: usize) -> Option<String> {
        match self.s.gate.hold() {
            Some(Hold::Wait { why, .. }) if origin == self.s.origin => Some(why),
            _ => None,
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.s.spend.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::transfer::fetch::limit_of;

    #[test]
    fn a_429_is_sorted_by_its_retry_after_else_by_its_words() {
        let five = Duration::from_secs(5);
        let at = |words: &str, after: Option<u64>| {
            limit(&limit_of(words, after.map(Duration::from_secs)), five)
        };
        // An mStream that passes Retry-After on: the stream cap's 5 s, the
        // quota's seconds to the peer's midnight.
        assert_eq!(at("Too many concurrent streams", Some(5)), Limit::Streams(Duration::from_secs(5)));
        assert_eq!(at("Daily transfer quota exceeded", Some(38_000)), Limit::Quota(Some(38_000)));
        // A quota's a minute before midnight: waited out, a new day by then.
        assert_eq!(at("Daily transfer quota exceeded", Some(40)), Limit::Streams(Duration::from_secs(40)));
        // An older parent's proxy drops it: the words decide.
        assert_eq!(at("Daily transfer quota exceeded", None), Limit::Quota(None));
        assert_eq!(at("Too many concurrent streams", None), Limit::Streams(five));
        assert_eq!(at("slow down", None), Limit::Streams(five), "an unknown one is waited out");
    }

    #[test]
    fn only_a_whole_guest_ticket_opens_the_direct_path() {
        let granted = DirectAccessResponse {
            direct: true,
            direct_ticket: Some("mstrfedg1:abc".into()),
            guest_token: Some("g.t.s".into()),
            ..DirectAccessResponse::default()
        };
        assert_eq!(ticket(&granted), Ok(("mstrfedg1:abc".to_string(), "g.t.s".to_string())));
        let declined = DirectAccessResponse { reason: Some("an older build".into()), ..Default::default() };
        assert_eq!(ticket(&declined), Err("an older build".to_string()));
        let half =
            DirectAccessResponse { direct: true, guest_token: Some("g".into()), ..Default::default() };
        assert!(ticket(&half).is_err());
        let pairing = DirectAccessResponse { direct_ticket: Some("mstr1:xyz".into()), ..granted };
        assert!(ticket(&pairing).is_err(), "a pairing code is no guest ticket");
    }

    #[test]
    fn a_peer_is_asked_nothing_once_held_or_its_day_is_spent() {
        let (day, meter) = Day::of(Some(100));
        let gate = Gate::of(day);
        assert_eq!((gate.room(), gate.open()), (Some(100), true));
        meter.fetch_add(60, Ordering::Relaxed);
        assert_eq!(gate.room(), Some(40), "what its covers may still take");
        meter.fetch_add(60, Ordering::Relaxed);
        assert_eq!((gate.room(), gate.open()), (Some(0), false), "spent: nothing more");
        let (free, _) = Day::of(None);
        let gate = Gate::of(free);
        assert_eq!((gate.room(), gate.open()), (None, true), "no cap");
        gate.set(Hold::Stop("refused".into()));
        assert_eq!((gate.room(), gate.open()), (Some(0), false), "held");
    }

    #[test]
    fn a_peers_fetches_take_turns_two_at_a_time() {
        let key = PeerKey { parent_key: 0x0BAD_F00D, peer_id: 77 };
        let stop = AtomicBool::new(false);
        let (a, b) = (Turn::take(key, &stop).unwrap(), Turn::take(key, &stop).unwrap());
        let other = Turn::take(PeerKey { peer_id: 78, ..key }, &stop);
        assert!(other.is_some(), "another peer's are its own");
        let stopping = AtomicBool::new(true);
        assert!(Turn::take(key, &stopping).is_none(), "a third waits, and gives up when stopped");
        drop(a);
        let c = Turn::take(key, &stop);
        assert!(c.is_some(), "one gone, one more");
        drop((b, c));
    }
}
