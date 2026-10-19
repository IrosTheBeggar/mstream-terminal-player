//! A sync's federation peers (the transfer plan's 2.15, part B9): each
//! peer the run is asked to read, or the card's settings name, read as a
//! server of the card's own (2.5.5: its rows carry their server, an SRVS
//! entry names it by its parent's key and its id there), its files from
//! [`Peer`] under its daily budget ([`budget`]).
//!
//! For each peer, in turn ([`prepare`]):
//! 1. It is one the parent lists (`federation/peers`), and the one the
//!    card's files came from: a peer paired again under another's id has
//!    another iroh identity, and isn't read while the card holds files of
//!    the one before. The card knows a peer by its identity once it is read.
//! 2. Its day isn't spent (`budget.json`'s `resumeAt`, one account at any
//!    of its parent's addresses): one that waits isn't asked anything, not
//!    even its listing, which its quota counts.
//! 3. A way to its bytes the listener allows ([`peer::reach`]): the direct
//!    guest path, else the parent's proxy when it says the bytes are a
//!    copy (as its `instanceId` tells: [`proxy_says_copies`]), else only
//!    once the listener lets an older parent's proxy carry them (stored on
//!    the card: asked once).
//! 4. Its `/api/`: its libraries, its own id, and its key's allowance
//!    where it shows one (a key that forbids copies is never read). A day
//!    with nothing left, by the budget or the key's quota, parks it there.
//! 5. Its listing, and its wanted files: as the user's own server's are
//!    decided, a conversion made by this computer's ffmpeg where it has one
//!    and skipped (and listed) where it hasn't. A listing that spends what
//!    was left parks it too, the card's rows of it as they are.
//!
//! Whatever a peer comes to is said in its [`PeerSummary`]: read, or why
//! not, its way, what is left of its day and the days its files take.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use super::*;
use crate::api::transfer::TransferPeer;
use crate::device::card::budget::{self, Budget, PeerKey, Spend};
use crate::device::card::contract::server::TalkPeer;
use crate::device::card::source::ffmpeg::{self, Convert};
use crate::device::card::source::peer::{self, Dial, Gate, Hold, Peer, Reach, Setup};

/// What a sync's peers need of this computer (B9).
pub(crate) struct Federation {
    /// `budget.json`'s folder.
    pub(crate) budget: Budget,
    /// The guest path's tunnel.
    pub(crate) dial: Box<dyn Dial>,
    pub(crate) ffmpeg: Ffmpeg,
    /// The roots of the volumes the drive-safety rule accepts: no ffmpeg is
    /// run from one.
    pub(crate) cards: Vec<PathBuf>,
    /// The clock the budget's days are kept by.
    pub(crate) clock: budget::Clock,
    /// A resume's few random minutes.
    pub(crate) jitter: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// A stream cap's 429 that names no Retry-After waits this long.
    pub(crate) stream_wait: std::time::Duration,
}

/// Where a peer's conversions are made.
pub(crate) enum Ffmpeg {
    /// The one this computer has ([`ffmpeg::find`]).
    Find,
    /// This one, or none.
    Given(Option<Arc<dyn Convert>>),
}

impl Federation {
    /// This computer's: the budget beside `cards.json`, iroh's dial, the
    /// ffmpeg found off `cards` (the accepted volumes' roots), the system's
    /// clock, 5 s for a stream cap.
    pub(crate) fn system(cards: Vec<PathBuf>) -> Result<Federation, String> {
        Ok(Federation {
            budget: Budget::system()?,
            dial: Box::new(peer::IrohDial),
            ffmpeg: Ffmpeg::Find,
            cards,
            clock: budget::system_clock(),
            jitter: Arc::new(budget::jitter),
            stream_wait: std::time::Duration::from_secs(5),
        })
    }
}

/// What came of a peer a sync was asked to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Listed and planned.
    Read,
    /// Its day is spent: asked again after `resume_at`.
    Waiting,
    /// The parent lists no such peer, or no peers at all.
    Unlisted,
    /// The parent lists another server under its id (another iroh
    /// identity than the card's peer's).
    OtherPeer,
    /// Only an older parent's proxy reaches it, which can't tell the peer
    /// these bytes are copies: the listener's say, asked once.
    AskProxy,
    /// Its key forbids copies, or the run refused it (a library it doesn't
    /// grant, a budget that can't be kept).
    Refused,
    /// It didn't answer.
    Unreachable,
}

impl Status {
    /// As one stable word, for the debug command and the tests.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Status::Read => "read",
            Status::Waiting => "waiting",
            Status::Unlisted => "unlisted",
            Status::OtherPeer => "other-peer",
            Status::AskProxy => "ask-proxy",
            Status::Refused => "refused",
            Status::Unreachable => "unreachable",
        }
    }
}

/// A peer, as a sync says it.
#[derive(Clone, Debug)]
pub(crate) struct PeerSummary {
    pub(crate) id: u32,
    pub(crate) status: Status,
    /// Why, for any status but read.
    pub(crate) why: Option<String>,
    /// The way its bytes came: `proxy` is a parent taken to pass the copy
    /// purpose on by its `instanceId` alone ([`proxy_says_copies`]).
    pub(crate) reach: Option<Reach>,
    pub(crate) libraries: Vec<String>,
    /// The selection's files, and those the card can't take.
    pub(crate) files: usize,
    pub(crate) skipped: Vec<(String, Skip)>,
    /// What the run fetches of it, and the UTC days that takes.
    pub(crate) fetches: usize,
    pub(crate) fetch_bytes: u64,
    pub(crate) days: u64,
    /// What is left of its day, and a whole day's budget; None: no cap.
    pub(crate) left: Option<u64>,
    pub(crate) cap: Option<u64>,
    /// Where its conversions are made: `server` (mStream's own ffmpeg),
    /// `path`, or none.
    pub(crate) ffmpeg: Option<&'static str>,
    /// Its bytes come through a relay: a long pull is worth a warning.
    pub(crate) relay: bool,
    /// Unix seconds: when a peer that waits may be asked again.
    pub(crate) resume_at: Option<u64>,
}

impl PeerSummary {
    fn of(id: u32, status: Status, why: Option<String>) -> PeerSummary {
        PeerSummary {
            id,
            status,
            why,
            reach: None,
            libraries: Vec::new(),
            files: 0,
            skipped: Vec::new(),
            fetches: 0,
            fetch_bytes: 0,
            days: 0,
            left: None,
            cap: None,
            ffmpeg: None,
            relay: false,
            resume_at: None,
        }
    }

    pub(crate) fn json(&self) -> Value {
        let skip = |(path, why): &(String, Skip)| json!({ "path": path, "why": why.code() });
        json!({
            "id": self.id, "status": self.status.code(), "why": self.why,
            "reach": self.reach.map(Reach::code), "libraries": self.libraries, "files": self.files,
            "skipped": self.skipped.iter().map(skip).collect::<Vec<_>>(),
            "fetch": { "files": self.fetches, "bytes": self.fetch_bytes, "days": self.days },
            "left": self.left, "cap": self.cap, "ffmpeg": self.ffmpeg, "relay": self.relay,
            "resumeAt": self.resume_at,
        })
    }
}

/// A peer a run reads: its files (origin 0 of its own wanted card, put
/// after the others' by the caller), its source, its covers, and the client
/// of its parent's browse proxy.
pub(super) struct PeerSide {
    pub(super) wanted: Wanted,
    pub(super) peer: Peer,
    pub(super) covers: Covers,
    pub(super) list: Arc<Client>,
    pub(super) gate: Gate,
    pub(super) summary: PeerSummary,
}

/// What [`prepare`] reads the peers with.
pub(super) struct Context<'a> {
    /// The parent's client, and its `/api/`.
    pub(super) client: &'a Arc<Client>,
    pub(super) info: &'a LayeredInfo,
    /// The parent as the run reads it.
    pub(super) talk: &'a Talk,
    pub(super) root: &'a volume::Root,
    pub(super) device: &'a DeviceTxt,
    pub(super) ask: &'a Ask,
    pub(super) fed: &'a Federation,
    pub(super) reached: Reached,
    /// The first origin the peers take: the user's own server's is 0 when
    /// the run reads it.
    pub(super) first_origin: usize,
}

/// The conversion of this computer the run's peers have.
fn converter(cx: &Context<'_>) -> Option<Arc<dyn Convert>> {
    match &cx.fed.ffmpeg {
        Ffmpeg::Given(given) => given.clone(),
        Ffmpeg::Find => {
            // mStream's own, when this computer is the server's and the
            // session an admin's: its folder is one of this computer's.
            let admin = cx.info.admin.as_ref();
            let os = |p: &str| (p == "win32") == cfg!(windows);
            let same_os = admin.is_some_and(|a| a.platform.as_deref().is_none_or(os));
            let beside = same_os && cx.reached != Reached::Elsewhere;
            let folder = beside.then(|| cx.client.admin_ffmpeg_folder().ok().flatten()).flatten();
            let folder = folder.map(PathBuf::from).filter(|f| f.is_absolute());
            let path = std::env::var_os("PATH");
            let found = ffmpeg::find(folder.as_deref(), path.as_deref(), &cx.fed.cards)?;
            Some(Arc::new(found))
        }
    }
}

/// What a conversion made on this computer takes: every rate of both
/// codecs (Opus stays at 192k at most, as on a server).
fn made_here() -> TranscodeInfo {
    let rates = json!(["64k", "96k", "128k", "192k", "256k", "320k"]);
    let codecs = json!({ "codecs": { "mp3": { "bitrates": rates }, "opus": { "bitrates": rates }}});
    serde_json::from_value(codecs).unwrap_or_default()
}

/// The parent's stream proxy says to the peer that the bytes it carries are
/// copies (`X-mStream-Purpose`, the card transfer's batch of mStream
/// changes). mStream says nothing of its proxy in `/api/`, so this is
/// inferred from the `instanceId` it does say, the same batch's, which came
/// after the proxy's change in it: a parent of that batch whole has both.
/// One with the id and not the proxy's change would carry copies its peer
/// never sees declared; the run still refuses a key that forbids them when
/// the peer shows its allowance (step 4), never an older peer's. So the
/// inference is made here alone, until the batch says it in its own words.
fn proxy_says_copies(info: &LayeredInfo) -> bool {
    info.instance_id.is_some()
}

/// The peers the run reads: the ones asked for, else the ones the card's
/// settings name under this parent.
fn asked(cx: &Context<'_>, settings: &Settings) -> Vec<PeerAsk> {
    if let Some(asked) = &cx.ask.peers {
        return asked.clone();
    }
    let keys = parent_keys(cx.talk);
    let of_this = |p: &&PeerSource| keys.contains(&p.parent_key) && !p.libraries.is_empty();
    let ask = |p: &PeerSource| PeerAsk { id: p.peer_id, libraries: None };
    settings.peers.iter().filter(of_this).map(ask).collect()
}

/// The parent's keys: the one of its URL now, then the others the listener
/// called it by.
fn parent_keys(talk: &Talk) -> Vec<u64> {
    std::iter::once(talk.url_key).chain(talk.confirmed.iter().copied()).collect()
}

/// Each peer of the run (see the module's note): the ones read, and what
/// came of every one asked.
pub(super) fn prepare(
    cx: &Context<'_>,
    settings: &mut Settings,
    notes: &mut Vec<String>,
) -> (Vec<PeerSide>, Vec<PeerSummary>) {
    let asked = asked(cx, settings);
    let (mut sides, mut summaries) = (Vec::new(), Vec::new());
    if asked.is_empty() {
        return (sides, summaries);
    }
    let listed: Vec<TransferPeer> = match cx.client.transfer_peers() {
        Ok(listed) => listed,
        Err(e) => {
            notes.push(format!("the server lists no federation peers: {e}"));
            let none = |a: &PeerAsk| PeerSummary::of(a.id, Status::Unlisted, Some(e.to_string()));
            return (sides, asked.iter().map(none).collect());
        }
    };
    let convert = converter(cx);
    let mut seen = std::collections::HashSet::new();
    for a in asked.iter().filter(|a| seen.insert(a.id)) {
        let origin = cx.first_origin + sides.len();
        match side(cx, settings, a, &listed, convert.clone(), origin) {
            Ok(side) => {
                summaries.push(side.summary.clone());
                sides.push(side);
            }
            Err(summary) => summaries.push(summary),
        }
    }
    (sides, summaries)
}

/// One peer of the run, or what came of it.
fn side(
    cx: &Context<'_>,
    settings: &mut Settings,
    a: &PeerAsk,
    listed: &[TransferPeer],
    convert: Option<Arc<dyn Convert>>,
    origin: usize,
) -> Result<PeerSide, PeerSummary> {
    let id = a.id;
    let not = |status: Status, why: String| PeerSummary::of(id, status, Some(why));
    let Some(row) = listed.iter().find(|p| p.id == id as i64) else {
        return Err(not(Status::Unlisted, format!("the server lists no peer {id}")));
    };
    let fed = cx.fed;
    let now = (fed.clock)();
    // Its account: one, at whichever of its parent's addresses it is read.
    let keys = parent_keys(cx.talk);
    let key = PeerKey { parent_key: cx.talk.url_key, peer_id: id };
    let limit = cx.ask.budget_mb;
    let book = fed.budget.with(|b| {
        b.adopt(key, &keys);
        if limit.is_some() {
            b.set_limit(key, limit);
        }
        b.waiting(key, now)
    });
    match book {
        Err(why) => return Err(not(Status::Refused, format!("its budget can't be kept: {why}"))),
        Ok(Some(until)) => {
            let why = "its day is spent".to_string();
            return Err(PeerSummary { resume_at: Some(until), ..not(Status::Waiting, why) });
        }
        Ok(None) => {}
    }

    // The card's peer, by its iroh identity once the card knows it: one
    // paired again under its id is another server, while the card holds
    // files of the one before (a peer never read takes the new one).
    let source = settings.peer_source(&keys, id);
    let endpoint = row.endpoint_id.clone().unwrap_or_default();
    let another = !source.endpoint.is_empty() && !endpoint.is_empty() && source.endpoint != endpoint;
    if another && holds_files(cx, id) {
        let why = format!("the server's peer {id} isn't the server this card's files came from");
        return Err(not(Status::OtherPeer, why));
    }
    if cx.ask.proxy_copies.contains(&id) {
        source.proxy_copies = true;
    }

    // The way to its bytes: an older parent's proxy only with the
    // listener's say. Its `/api/` and its listing go through the parent's
    // browse proxy, its bytes as reached, every byte counted.
    let declares = proxy_says_copies(cx.info);
    let dial = (!cx.ask.dry_run).then_some(fed.dial.as_ref());
    let reached = match peer::reach(cx.client, id as i64, declares, source.proxy_copies, dial) {
        Ok(reached) => reached,
        Err(why) => {
            let proxy = "only the server's proxy reaches it, which can't say it copies";
            return Err(not(Status::AskProxy, format!("{proxy} ({why})")));
        }
    };
    let meter = Arc::new(AtomicU64::new(0));
    let list = Arc::new(cx.client.for_peer(id as i64).with_meter(Some(meter.clone())));
    let bytes = Arc::new(reached.client.with_meter(Some(meter.clone())));
    let api = match list.peer_api() {
        Ok(api) => api,
        Err(e) => return Err(not(Status::Unreachable, e.to_string())),
    };
    if api.allowance.as_ref().is_some_and(|a| a.allow_copies == Some(false)) {
        let why = "the peer's owner doesn't let this server's key copy its files".to_string();
        return Err(not(Status::Refused, why));
    }
    let granted = &api.info.user.vpaths;
    let libraries = match (&a.libraries, source.libraries.is_empty()) {
        (Some(asked), _) => asked.clone(),
        (None, false) => source.libraries.clone(),
        (None, true) => granted.clone(),
    };
    if let Some(unknown) = libraries.iter().find(|l| !granted.contains(l)) {
        return Err(not(Status::Refused, format!("{unknown} isn't a library the peer grants")));
    }
    if libraries.is_empty() {
        return Err(not(Status::Refused, "the peer grants no library".into()));
    }
    let (allowance, clock) = (api.allowance.clone(), fed.clock.clone());
    let spend = Spend::new(fed.budget.clone(), key, &keys, meter.clone(), allowance, clock);
    let unkept = |why: String| not(Status::Refused, format!("its budget can't be kept: {why}"));
    let mut spend = spend.map_err(unkept)?;
    // A day spent (the listener's budget, or the key's quota as the peer
    // says it) asks nothing more of it, not its listing: it waits.
    if spend.left() == Some(0) {
        return Err(spent(fed, &mut spend, id));
    }

    // Its files, decided as the user's own server's are.
    let transcode = convert.as_ref().map(|_| made_here());
    let plan_key = settings.plan_key(&libraries, cx.device, transcode.as_ref());
    let listing = match peer::list(&list, &libraries) {
        Ok(listing) => listing,
        Err(e) => return Err(not(Status::Unreachable, format!("its listing didn't come: {e}"))),
    };
    let instance = instance_of(api.info.instance_id.as_deref());
    let talk = Talk { peer: Some(TalkPeer { id, instance }), ..cx.talk.clone() };
    let ours = ours_on(Some(cx.root), &talk);
    let rows: Vec<Kept> = match (cx.root.tags.as_ref(), ours) {
        (Ok(tags), Some(i)) => tags
            .records
            .iter()
            .filter(|e| e.ledger.as_ref().is_some_and(|l| l.server as usize == i))
            .filter_map(Kept::of)
            .collect(),
        _ => Vec::new(),
    };
    let kept: HashMap<String, Kept> =
        rows.iter().map(|k| (k.ledger.server_path.clone(), k.clone())).collect();
    let source = settings.peer_source(&keys, id);
    let same_plan = source.plan_key == plan_key;
    let mut ogg = HashMap::new();
    for e in oggs_to_sniff(&listing.entries, &kept, same_plan) {
        if spend.left() == Some(0) {
            break;
        }
        if let Ok(read) = bytes.read_range(&e.filepath, 0, OGG_HEAD) {
            ogg.insert(e.filepath.clone(), ogg_is_opus(&read.bytes));
        }
    }
    // The listing spent what was left: it waits, the card's rows of it as
    // they are.
    if spend.left() == Some(0) {
        return Err(spent(fed, &mut spend, id));
    }
    let scanning = listing.unscanned > 0;
    let audio = |e: &ManifestEntry| e.audio_hash.as_deref().and_then(hex16);
    let origin_of = Origin {
        talk,
        server: Server::peer(instance, cx.talk.url_key, id),
        revision: String::new(),
        scanning,
        whole: (!scanning).then(|| listing.entries.iter().filter_map(audio).collect()),
    };
    let conversion = settings.conversion;
    let wanted = wanted_of(
        origin_of,
        &listing.entries,
        &listing.metadata,
        &ogg,
        &kept,
        same_plan,
        cx.device,
        &conversion,
        transcode.as_ref(),
    );
    // Read: its selection, its plan's settings, and its identity and name
    // as its parent lists them, the card's from now on.
    let source = settings.peer_source(&keys, id);
    (source.libraries, source.plan_key) = (libraries.clone(), plan_key);
    if !endpoint.is_empty() {
        source.endpoint = endpoint;
    }
    if let Some(name) = &row.name {
        source.nickname = name.trim().to_string();
    }

    let gate = Gate::of(spend.day());
    let setup = Setup {
        origin,
        reach: reached.reach,
        spend,
        gate: gate.clone(),
        convert: convert.clone(),
        max_channels: cx.device.max_channels,
        clock: fed.clock.clone(),
        jitter: fed.jitter.clone(),
        stream_wait: fed.stream_wait,
    };
    let peer = Peer::new(setup, bytes.clone(), reached.tunnel);
    let peer = peer.map_err(|e| not(Status::Refused, e.to_string()))?;
    let room = gate.clone();
    let covers = Covers::peer(bytes, list.clone(), Box::new(move || room.room()));
    let summary = PeerSummary {
        reach: Some(reached.reach),
        libraries,
        files: wanted.card.files.len(),
        skipped: wanted.skipped.clone(),
        left: peer.left(),
        cap: peer.cap(),
        ffmpeg: convert.as_ref().map(|c| c.found()),
        relay: peer.relayed(),
        ..PeerSummary::of(id, Status::Read, reached.direct_refused.clone())
    };
    Ok(PeerSide { wanted, peer, covers, list, gate, summary })
}

/// A peer whose day is spent before its files are planned: it waits, in the
/// book, until the next 00:00 UTC and a few minutes.
fn spent(fed: &Federation, spend: &mut Spend, id: u32) -> PeerSummary {
    let until = budget::resume_after((fed.clock)(), None, (fed.jitter)());
    match spend.park(until) {
        Ok(()) => {
            let why = Some("its day is spent".to_string());
            PeerSummary { resume_at: Some(until), ..PeerSummary::of(id, Status::Waiting, why) }
        }
        Err(e) => {
            let why = Some(format!("its budget can't be kept: {e}"));
            PeerSummary::of(id, Status::Refused, why)
        }
    }
}

/// The card holds files of its peer `id` of this parent: its rows of the
/// SRVS entry that is that peer (by its parent and its id, whatever its own
/// id). A card whose rows can't be read is taken to hold some.
fn holds_files(cx: &Context<'_>, id: u32) -> bool {
    let talk = Talk { peer: Some(TalkPeer { id, ..TalkPeer::default() }), ..cx.talk.clone() };
    let Some(i) = ours_on(Some(cx.root), &talk) else { return false };
    let of = |e: &Entry| e.ledger.as_ref().is_some_and(|l| l.server as usize == i);
    match cx.root.tags.as_ref() {
        Ok(tags) => tags.records.iter().any(of),
        Err(_) => true,
    }
}

/// What the plan fetches of each peer, and the days it takes.
pub(super) fn planned(summaries: &mut [PeerSummary], peers: &[Peer], plan: &Plan, wanted: &Wanted) {
    for p in peers {
        let o = p.origin();
        let fetched = plan.fetched_paths().map(|(i, _)| &wanted.card.files[i]);
        let fetched = fetched.filter(|f| f.origin == o);
        let (n, bytes) = fetched.fold((0, 0), |(n, b), f| (n + 1, b + f.size));
        let Some(s) = summaries.iter_mut().find(|s| s.id == p.peer_id()) else { continue };
        (s.fetches, s.fetch_bytes) = (n, bytes);
        s.days = budget::days_for(bytes, s.left, s.cap);
    }
}

/// What came of each peer the run read, once it ended: one whose day was
/// spent waits until its time, one that refused stops; and the earliest a
/// waiting peer may be asked again.
pub(super) fn after(summaries: &mut [PeerSummary], peers: &mut [Peer]) -> Option<u64> {
    for p in peers.iter_mut() {
        p.settle();
        let read = |s: &&mut PeerSummary| s.status == Status::Read && s.id == p.peer_id();
        let Some(s) = summaries.iter_mut().find(read) else { continue };
        s.left = p.left();
        match p.gate().hold() {
            Some(Hold::Wait { why, until }) => {
                (s.status, s.why, s.resume_at) = (Status::Waiting, Some(why), Some(until));
            }
            Some(Hold::Stop(why)) => (s.status, s.why) = (Status::Refused, Some(why)),
            None => {}
        }
    }
    summaries.iter().filter_map(|s| s.resume_at).min()
}
