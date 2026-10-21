//! A card synced from the user's own server (the transfer plan's part B1;
//! 2.3, the whole of one run): the server's selection planned against the
//! card and written to it by the engine, every part of the server's side
//! of a sync filled in.
//!
//! **One run**, [`sync`]:
//! 1. The server: `GET /api/` says it serves the manifest, whether it
//!    converts (at which rates, and through a bulk queue or not), and the
//!    user's libraries (an admin's block too, for the local-disk source).
//! 2. The card's settings (`state.json`: [`Settings`]), the run's changes to
//!    them, and the session that binds the card (the engine's steps 1-4),
//!    whose listing maps the new plays of the card's log through its ledger
//!    into this computer's outbox ([`History`]). A card with no state file
//!    isn't one the transfer set up: `card-setup` first; a card that is a
//!    copy of another stops until it has an identity of its own (2.10).
//! 3. The selection, through `ignoreVPaths`: the paged manifest, sent the
//!    revision the last whole read ended with (If-None-Match). A 304 means
//!    "probably unchanged", so the card's ledger stands for the listing
//!    ([`wanted_of_ledger`]); the manifest is read whole every 7 days, every
//!    10th run, and whenever the plan's settings changed (2.10.2). A file
//!    the server can't serve (a dotfile, or one in a dot-folder:
//!    [`plan::hidden`]) is left out of it before anything else, a peer's
//!    too, and the run says how many in one note. Then
//!    `metadata/batch` for the files a decision needs it for, and each
//!    `.ogg`'s first bytes (Opus is copied as it is). The plays waiting for
//!    this server are posted then, a path it moved found again in a whole
//!    listing: a sync that stops later has posted them all the same. Plays
//!    to send that the outbox couldn't take stop the run here.
//! 4. The wanted card ([`plan`](super::plan)), the engine's plan of it, the
//!    fit and the track cap, the guards. A dry run stops here: on a card
//!    whose writes go to memory ([`DryCard`]), so nothing reaches the card.
//! 5. The run, with the source ([`Http`] beneath [`Local`]) and the hooks
//!    ([`SyncHooks`]): each conversion checked on the card (its type, its
//!    length within a second of the manifest's), a converted file's cover
//!    and its own BPM and key put back before the card takes it ([`Retag`]),
//!    each album's thumbnail ([`covers`](super::covers)), and the Auto DJ
//!    index from mStream's embeddings when the card's player reads MPDJ v2
//!    and the card isn't made for someone else.
//! 6. The settings, stored at the run's end before it lets the card go
//!    ([`Hooks::state`]): the selection, the conversion, the source's
//!    nickname and keys, the manifest's schedule, which art each thumbnail
//!    was made from, the plays' posted marks. The run's commits carry them
//!    as ACKS ([`Hooks::acks`]), a commit made for nothing else never.
//!
//! **A card of a multi-card set** (B7, [`sets`]; [`span`](super::span)) takes
//! the files of its range, keeps those it holds of other cards' ranges until
//! their cards hold them, and says at its end what it holds; a card that
//! can't take its range is offered to move its edge or to add a card, and a
//! card's selection is spread over a new set when the listener asks
//! ([`Ask::decide`]).
//!
//! **The server's federation peers** (B9, [`peers`]) are read in the same
//! run, each a server of the card's own: listed through the server's browse
//! proxy, their files fetched under each one's daily budget, a peer whose
//! day is spent waiting in the plan of the checkpoint the run ends with
//! while the other sources' files go in. Each file is known by its origin
//! and its server path ([`Key`]): two servers may hold one path.
//!
//! The listener's way in is the Card view and `device card <drive> sync`,
//! which wait for their UX contract; until then the hidden `device
//! card-sync` runs it ([`command`]).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::autodj::{self, FileIn, Index, IndexIn, Model, NoIndex, Selection, Work};
use super::cards::Cards;
use super::contract::MAX_FILE;
use super::contract::device_txt::DeviceTxt;
use super::contract::mptg::{Entry, Ledger, RecordIn};
use super::contract::msmf::Ack;
use super::contract::order::{compare_file_paths, split_last};
use super::contract::prim::{fnv1a64, hex, hex16, path_hash};
use super::contract::server::{Server, Talk, Verdict, instance_of, server_url_key};
use super::covers::{self, Covers, folders, most_common};
use super::dry::DryCard;
use super::engine::{
    Album, Answers, AutoDj, Commit, Cover, Env, Event, Expect, Failed, Fetched, Fetcher, Guard,
    Have, Hooks, Knobs, Origin, Outcome, Plan, Report, Session, Stop, WantedFile,
};
use super::history::{self, History, Tally};
use super::names::{self, Bytes};
use super::plan::{
    self, Extra, Fit, Kept, Skip, Wanted, codec_of, needing_metadata, ogg_is_opus, oggs_to_sniff,
    wanted_of, wanted_of_ledger,
};
use super::source::Sources;
use super::source::http::{Finish, Http};
use super::source::length::{self, CardAt};
use super::source::local::{self, Local, Reached};
use super::source::peer::Gate;
use super::span::{Decision, Part};
use super::state::{
    self, Convert, FOLDER_IMAGE, Layout, NO_ART, PeerSource, Settings, State, art_key,
    hex as id_hex,
};
use super::tags::{self, Restore};
use super::volume::{self, CardFs, CardWrite, Mounted, Private, STATE_VERSION, Space};
use crate::api::transfer::{MANIFEST_PAGE_MAX, Manifest, ManifestEntry, Metadata};
use crate::api::types::{LayeredInfo, TranscodeInfo};
use crate::api::urls::{TransferBitrate, TransferCodec};
use crate::api::{Client, wait};

/// A whole read of the manifest at least this often (2.10.2: proposed).
const WHOLE_EVERY_SECS: u64 = 7 * 86_400;
const WHOLE_EVERY_RUNS: u32 = 10;
/// The first bytes of an `.ogg` that say whether it is Opus.
const OGG_HEAD: u64 = 512;

mod peers;
mod sets;

pub(crate) use peers::{Federation, PeerSummary, Status as PeerStatus};
pub(crate) use sets::SetSummary;

/// A file of a run, as its hooks find it: its origin and its server path,
/// since two of a run's servers may hold one path.
type Key = (usize, String);

// ── What a sync is asked, and what it says ──────────────────────────────────

/// Changes to the card's settings for this run and the next (stored after
/// a run). None: as the card has it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Change {
    pub(crate) layout: Option<Layout>,
    pub(crate) convert: Option<Convert>,
    pub(crate) codec: Option<TransferCodec>,
    pub(crate) bitrate: Option<TransferBitrate>,
    pub(crate) covers: Option<bool>,
    pub(crate) auto_dj: Option<bool>,
    pub(crate) for_someone_else: Option<bool>,
    pub(crate) removal: Option<super::engine::Removal>,
    /// The server's nickname on this card ("Home").
    pub(crate) nickname: Option<String>,
}

impl Change {
    fn apply(&self, s: &mut Settings) {
        s.layout = self.layout.unwrap_or(s.layout);
        s.conversion.convert = self.convert.unwrap_or(s.conversion.convert);
        s.conversion.codec = self.codec.unwrap_or(s.conversion.codec);
        s.conversion.bitrate = self.bitrate.unwrap_or(s.conversion.bitrate);
        s.covers = self.covers.unwrap_or(s.covers);
        s.auto_dj = self.auto_dj.unwrap_or(s.auto_dj);
        s.for_someone_else = self.for_someone_else.unwrap_or(s.for_someone_else);
        s.removal = self.removal.unwrap_or(s.removal);
    }
}

/// What a sync is asked.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ask {
    /// The libraries, by vpath; None: the card's selection for this server,
    /// else every library the user sees.
    pub(crate) libraries: Option<Vec<String>>,
    pub(crate) change: Change,
    /// The answers to the plan's guards.
    pub(crate) answers: Answers,
    /// The card's server 0, when the card can't tell it from this server,
    /// is this server under another address: its key is confirmed as this
    /// server's (2.11's key rule), and stored.
    pub(crate) same_server: bool,
    /// Read the manifest whole, whatever the schedule says.
    pub(crate) whole: bool,
    /// Plan, and write nothing.
    pub(crate) dry_run: bool,
    /// The server's federation peers to read (B9); None: those the card's
    /// settings name under this server.
    pub(crate) peers: Option<Vec<PeerAsk>>,
    /// None of the server's own libraries is read this run: its files on
    /// the card stay as they are.
    pub(crate) peers_only: bool,
    /// Peers whose files may come through an older server's proxy, which
    /// can't tell them they are copies: the listener's answer, stored on
    /// the card.
    pub(crate) proxy_copies: Vec<u32>,
    /// The listener's daily budget for the peers read, in MiB, kept in this
    /// computer's `budget.json`.
    pub(crate) budget_mb: Option<u64>,
    /// A multi-card set (B7): the card's selection spread over a new set, or
    /// the answer to a card of a set that can't take its range.
    pub(crate) decide: Option<Decision>,
    /// The listener's stop (B4, *Stop at the next save*): once set, a sync
    /// that hasn't written yet stops before it writes ([`Halt::Stopped`]),
    /// and one under way fetches nothing more: the files in flight finish,
    /// and the run ends at a checkpoint whose plan the next sync takes up
    /// (2.12.8's parking, [`STOPPED`]).
    pub(crate) stop: Option<Arc<AtomicBool>>,
}

/// Why a run the listener stopped parked ([`Outcome::parked`]).
pub(crate) const STOPPED: &str = "stopped at the listener's word: the next sync goes on from here";

/// A federation peer a sync is asked to read (B9): its id in the server's
/// peer list, and its libraries (None: the card's selection for it, else
/// every library it grants).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PeerAsk {
    pub(crate) id: u32,
    pub(crate) libraries: Option<Vec<String>>,
}

/// How the server's side was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Listed {
    /// The manifest, every page.
    Whole,
    /// A 304: the card's ledger stands for it.
    NotModified,
    /// Not read: a run of the server's peers alone.
    Unread,
}

impl Listed {
    fn code(self) -> &'static str {
        match self {
            Listed::Whole => "whole",
            Listed::NotModified => "not-modified",
            Listed::Unread => "unread",
        }
    }
}

/// The plan, as a sync says it before it runs.
#[derive(Clone, Debug)]
pub(crate) struct Summary {
    pub(crate) listed: Listed,
    /// Where the bytes come from: `disk`, or why the network ([`local::Mode`]).
    pub(crate) source: &'static str,
    pub(crate) libraries: Vec<String>,
    /// The selection's files, and those the card can't take.
    pub(crate) files: usize,
    pub(crate) skipped: Vec<(String, Skip)>,
    /// The server's files it can't serve, left out of the selection
    /// ([`plan::hidden`]): counted, never skipped one by one.
    pub(crate) hidden: usize,
    pub(crate) fetches: usize,
    pub(crate) fetch_bytes: u64,
    pub(crate) conversions: usize,
    pub(crate) renames: usize,
    pub(crate) kept: usize,
    pub(crate) adopted: usize,
    pub(crate) removals: Vec<String>,
    pub(crate) guards: Vec<Guard>,
    pub(crate) report: Report,
    /// None when the volume's space isn't known.
    pub(crate) fit: Option<Fit>,
    /// Each federation peer asked for: read, or why not.
    pub(crate) peers: Vec<PeerSummary>,
    /// The card's multi-card set, as the run leaves it.
    pub(crate) set: Option<SetSummary>,
    /// A card of no set that doesn't fit: its selection over the cards a set
    /// of it would take, each of its room.
    pub(crate) spread: Vec<Part>,
    /// Every library the account sees on the server, by vpath: what a
    /// card's settings offer (B4).
    pub(crate) offered: Vec<String>,
    /// The listed libraries' files and bytes on the server, by vpath, as
    /// this run read them: the settings' sizes beside each library.
    pub(crate) totals: Vec<(String, usize, u64)>,
}

/// Why a sync stopped before its run ended.
#[derive(Debug)]
pub(crate) enum Halt {
    /// The server: out of reach, the sign-in refused, no manifest route.
    Server(String),
    /// The engine: a guard to answer, a card that changed or refused a
    /// write, a plan it won't make.
    Card(Stop),
    /// The plan doesn't fit the card, or passes the track cap.
    Fit(Fit),
    /// What a sync won't do: a card not set up, a copied card, a library the
    /// server doesn't have.
    Refused(String),
    /// The listener stopped it before it wrote anything ([`Ask::stop`]).
    Stopped,
}

impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Halt::Server(why) => write!(f, "the server: {why}"),
            Halt::Card(stop) => write!(f, "{stop}"),
            Halt::Fit(fit) if fit.over_cap() => {
                write!(f, "{} tracks, past the {} the player holds", fit.tracks, plan::TRACK_CAP)
            }
            Halt::Fit(fit) => write!(f, "it doesn't fit, by {} bytes", fit.short()),
            Halt::Refused(why) => f.write_str(why),
            Halt::Stopped => f.write_str("stopped before anything was written"),
        }
    }
}

/// How a sync ended.
#[derive(Debug)]
pub(crate) enum End {
    /// A dry run: the plan, nothing written.
    Dry,
    Ran(Outcome),
    Halted(Halt),
}

/// What a sync found and did.
#[derive(Debug)]
pub(crate) struct Synced {
    /// The plan, when one was made.
    pub(crate) summary: Option<Summary>,
    pub(crate) end: End,
    /// What the run passed by without stopping: no Auto DJ index and why,
    /// settings not stored, a card with no `device.txt`.
    pub(crate) notes: Vec<String>,
    /// What became of the card's plays, once the card's settings were read.
    pub(crate) plays: Option<Tally>,
    /// Each federation peer asked for, as the run left it.
    pub(crate) peers: Vec<PeerSummary>,
    /// Unix seconds: when a peer whose day is spent may be asked again, the
    /// earliest of them; the caller's to run again then.
    pub(crate) resume_at: Option<u64>,
}

// ── One run ─────────────────────────────────────────────────────────────────

type Halted = (Option<Summary>, Halt);

/// See the module's note. `card` is a volume the drive-safety rule accepted
/// and the listener named; `space` its size and free space, when the OS
/// gives them (no fit without); `reached` how the client reaches its server
/// (the local-disk source's condition); `cards` this computer's cache of
/// cards, brought up to date (the caller saves it); `plays` the outbox of
/// the card's plays; `fed` what the server's peers need of this computer;
/// `envs` each session's clock and ids.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync(
    card: &dyn CardWrite,
    space: Option<Space>,
    client: Arc<Client>,
    reached: Reached,
    ask: &Ask,
    cards: &mut Cards,
    plays: Rc<dyn history::Store>,
    fed: &Federation,
    envs: &mut dyn FnMut() -> Env,
    events: &mut dyn FnMut(Event),
) -> Synced {
    let mut notes = Vec::new();
    let dry = DryCard::new(card);
    let target: &dyn CardWrite = if ask.dry_run { &dry } else { card };
    let (mut history, mut peers, mut resume_at) = (None, Vec::new(), None);
    let mut had = Had {
        notes: &mut notes,
        plays,
        history: &mut history,
        peers: &mut peers,
        resume_at: &mut resume_at,
    };
    let ran = run(target, space, &client, reached, ask, cards, fed, envs, events, &mut had);
    let (summary, end) = match ran {
        Ok((summary, end)) => (Some(summary), end),
        Err((summary, halt)) => (summary, End::Halted(halt)),
    };
    Synced { summary, end, notes, plays: history.map(|h| h.tally), peers, resume_at }
}

/// What a run gives back besides its end: its notes, the card's plays
/// (their outbox, and what the run's look and post made of them), and what
/// came of the peers asked for.
struct Had<'a> {
    notes: &'a mut Vec<String>,
    plays: Rc<dyn history::Store>,
    history: &'a mut Option<History>,
    peers: &'a mut Vec<PeerSummary>,
    resume_at: &'a mut Option<u64>,
}

fn server_halt(e: impl std::fmt::Display) -> Halted {
    (None, Halt::Server(e.to_string()))
}

/// The card's state file, read by 2.12.7's rule without a session: its
/// settings shape the session's knobs.
fn read_state(card: &dyn CardFs) -> Result<Option<State>, Halted> {
    let read = |name: &str| card.read(&format!("/.mstream/{name}"), MAX_FILE);
    let pulled = |_| (None, Halt::Card(Stop::Pulled));
    let (json, new) = (read("state.json").map_err(pulled)?, read("state.new").map_err(pulled)?);
    let private: Private =
        volume::settle_private(json.as_deref(), new.as_deref(), "state", STATE_VERSION);
    Ok(State::of(&private))
}

/// The run's knobs, from the card's settings.
fn knobs_of(s: &Settings) -> Knobs {
    Knobs { removal: s.removal, vpath: s.layout.vpath(), ..Knobs::default() }
}

#[allow(clippy::too_many_arguments)]
fn run(
    card: &dyn CardWrite,
    space: Option<Space>,
    client: &Arc<Client>,
    reached: Reached,
    ask: &Ask,
    cards: &mut Cards,
    fed: &Federation,
    envs: &mut dyn FnMut() -> Env,
    events: &mut dyn FnMut(Event),
    had: &mut Had<'_>,
) -> Result<(Summary, End), Halted> {
    // 1. The server.
    let info: LayeredInfo = wait(client.layered_info_async()).map_err(server_halt)?;
    if !info.features.sync {
        let why = "it serves no sync manifest (an mStream older than the card transfer)";
        return Err(server_halt(why));
    }

    // 2. The card's settings, then the session.
    let Some(mut state) = read_state(card)? else {
        let why = "the card isn't set up for the transfer: run `device card-setup` on it first";
        return Err((None, Halt::Refused(why.into())));
    };
    let was = state.clone();
    ask.change.apply(&mut state.settings);
    state.settings.stored = true;
    let serial = card.serial().map_err(|_| (None, Halt::Card(Stop::Pulled)))?;
    let expect = Expect { serial, ..Expect::default() };
    let knobs = knobs_of(&state.settings);
    // The card's plays, mapped once the session has listed the card.
    let posted = (state.card_id, state.plays.clone());
    let history = History::new(had.plays.clone(), ask.dry_run, posted);
    let history = had.history.insert(history);
    let opened = Session::open(card, &expect, knobs, &ask.answers, envs(), history, events);
    let mut session = opened.map_err(|stop| (None, Halt::Card(stop)))?;
    let mut clock = envs();
    let (now, set_id) = ((clock.now)(), (clock.id)());
    let notes = &mut *had.notes;
    let was = Was { state: &was, set_id };
    let prepared = prepare(
        &mut session, &mut state, &was, space, client, reached, ask, &info, cards, now, history,
        notes, fed, events,
    );
    let mut p = match prepared {
        Ok(p) => p,
        Err(halted) => {
            // A set's answer stored with no run: this computer's mirror too.
            let set = halted.0.as_ref().and_then(|s| s.set.as_ref());
            if let Some(set) = set.filter(|s| s.written) {
                cards.mirror(&set.set);
            }
            session.close();
            return Err(halted);
        }
    };
    *had.peers = p.summary.peers.clone();
    *had.resume_at = p.summary.peers.iter().filter_map(|s| s.resume_at).min();
    if ask.dry_run {
        session.close();
        return Ok((p.summary, End::Dry));
    }
    // The listener's stop before anything is written: the card as it was.
    if stopped(ask) {
        session.close();
        return Err((Some(p.summary), Halt::Stopped));
    }
    events(Event::Planned { files: p.summary.fetches, bytes: p.summary.fetch_bytes });

    // 5. The run, and 6. the settings at its end ([`SyncHooks::state`]).
    let label = state.label.clone();
    let mut hooks = p.hooks.take().expect("hooks for a run");
    let revision = (p.ours != Some(0)).then(|| p.revision.clone());
    let (url_key, listed, plan_key) = (p.url_key, p.listed, p.plan_key);
    let span = p.span.take();
    hooks.span_cluster = span.as_ref().map(|s| s.cluster);
    hooks.store = Some(Store { state, url_key, listed, now, plan_key, revision, span });
    hooks.history = had.history.take();
    let mut fetcher = Stopping { inner: &mut p.fetcher, stop: ask.stop.clone() };
    let ran = session.run(p.plan, &mut fetcher, &mut hooks, events);
    *had.history = hooks.history.take();
    had.notes.append(&mut hooks.notes);
    *had.resume_at = peers::after(had.peers, &mut p.fetcher.network().peers);
    for s in had.peers.iter().filter(|s| s.relay && s.fetch_bytes > RELAY_WARN) {
        had.notes.push(format!("peer {}'s files come through a relay: a long pull", s.id));
    }
    let outcome = ran.map_err(|stop| (Some(p.summary.clone()), Halt::Card(stop)))?;
    if let Err(why) = &outcome.stored {
        had.notes.push(format!("the card's settings weren't stored: {why}"));
    }
    cards.saw(outcome.card_id, &label, now, outcome.generation, outcome.commit_id);
    // The card's set as its state file now says it: this computer's mirror,
    // which the set's other cards read.
    let set = hooks.store.as_ref().and_then(|s| s.state.set.as_ref());
    if let Some(set) = set.filter(|_| outcome.stored.is_ok()) {
        cards.mirror(set);
    }
    Ok((p.summary, End::Ran(outcome)))
}

/// The card's state file as the sync read it, before the run's changes, and
/// an id for a set the run makes.
struct Was<'a> {
    state: &'a State,
    set_id: u64,
}

/// The card's settings as a run stores them at its end: what it was asked
/// (in `state`), and what it learned of the server.
struct Store {
    state: State,
    url_key: u64,
    listed: Listed,
    now: u64,
    plan_key: u64,
    /// The manifest's revision as the run read it, for a server that isn't
    /// the card's server 0 (whose revision is the root's).
    revision: Option<String>,
    /// The card's set: what the card says of itself in it at the end.
    span: Option<sets::SetEnd>,
}

/// A peer's pull past this many bytes through a relay is worth a warning
/// (the transfer plan's 2.15: warn, don't refuse).
const RELAY_WARN: u64 = 1 << 30;

/// What a plan made ready for its run.
struct Prepared {
    plan: Plan,
    fetcher: Local<Sources>,
    hooks: Option<SyncHooks>,
    summary: Summary,
    listed: Listed,
    /// The manifest's revision, as this run read it.
    revision: String,
    /// Which of the card's servers this one is.
    ours: Option<usize>,
    url_key: u64,
    plan_key: u64,
    span: Option<sets::SetEnd>,
}

/// The card's server this one is (2.11), by its key or a key confirmed as
/// its own.
fn ours_on(root: Option<&volume::Root>, talk: &Talk) -> Option<usize> {
    let table = root?.manifest.server_table();
    (0..=table.entries.len()).find(|&i| table.same_server(i, talk) == Verdict::Same)
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    session: &mut Session,
    state: &mut State,
    was: &Was<'_>,
    space: Option<Space>,
    client: &Arc<Client>,
    reached: Reached,
    ask: &Ask,
    info: &LayeredInfo,
    cards: &Cards,
    now: u64,
    history: &mut History,
    notes: &mut Vec<String>,
    fed: &Federation,
    events: &mut dyn FnMut(Event),
) -> Result<Prepared, Halted> {
    let refused = |why: String| (None, Halt::Refused(why));
    let Some(root) = session.root() else {
        return Err(refused("the card has no commit: run `device card-setup` on it first".into()));
    };
    let (card_id, generation, commit_id) =
        (root.manifest.header.card_id, root.manifest.header.generation, root.manifest.commit_id);
    if state.card_id != card_id {
        let why = "the card's state file is another identity's: run `device card-setup` on it first";
        return Err(refused(why.into()));
    }
    let (serial, _, _) = session.identity();
    let copied = state::copied(state, card_id, serial.as_ref());
    if copied.or(cards.diverged(card_id, generation, commit_id)).is_some() {
        let why = "the card is a copy of another: give it an identity of its own \
                   (`device card-setup --new-identity`) first";
        return Err(refused(why.into()));
    }
    let device: DeviceTxt = session.device().clone();
    if !device.present {
        let why = "the card has no device.txt yet: synced for the player's first defaults \
                   (MP3 and FLAC, no Auto DJ) until it boots in a player";
        notes.push(why.into());
    }

    // This server, and the card's servers: one source answers every address
    // the listener called this server, and each of its other keys is one a
    // card's server may have (2.11's key rule; a run rewrites server 0's key
    // to the address it read). Its own id, once it gives one, decides first
    // (2.11), and goes into the root and SRVS as it is read.
    let url_key = server_url_key(&client.server());
    let instance = instance_of(info.instance_id.as_deref());
    let settings = &mut state.settings;
    let confirmed = settings.keys(url_key);
    let mut talk = Talk { url_key, instance, confirmed, peer: None };
    let mut ours = ours_on(Some(root), &talk);
    if ours.is_none() && ask.same_server {
        let zero = root.manifest.server_table().root;
        if zero.url_key != 0 && !zero.is_peer() && !talk.confirmed.contains(&zero.url_key) {
            settings.confirm(url_key, zero.url_key);
            talk.confirmed = settings.keys(url_key);
            ours = ours_on(Some(root), &talk);
        }
    }
    // The card's server by its own id, at an address the card had another
    // key for: one source answers both from now on, as the listener's own
    // answer makes it, so its selection and schedule carry over.
    let table = root.manifest.server_table();
    let theirs = ours.and_then(|i| table.get(i).copied());
    if let Some(srv) = theirs.filter(|s| !s.is_peer() && s.url_key != 0 && s.url_key != url_key)
        && !talk.confirmed.contains(&srv.url_key)
    {
        settings.confirm(url_key, srv.url_key);
        talk.confirmed = settings.keys(url_key);
        let (here, there) = (id_hex(url_key), id_hex(srv.url_key));
        notes.push(format!("the card's server by its id: one source answers {here} and {there}"));
    }
    // Another of the card's servers this one is too (an entry with no id at
    // this address, set beside the root before the server gave one): its
    // files join this server's in the plan, paired with its listing, so the
    // listing is read whole (a 304 stands for this server's rows alone).
    let same = table.all_same(&talk);
    let joined: Vec<Server> = same.iter().skip(1).filter_map(|&i| table.get(i).copied()).collect();
    for srv in &joined {
        let key = id_hex(srv.url_key);
        notes.push(format!("the card's server at {key} is this server: its files join this one's"));
    }
    // The card's multi-card set, as it and this computer know it (B7), its
    // server known by every key this one has.
    let peers = settings.peers.iter().any(|p| !p.libraries.is_empty());
    let (set, unread, label) = (state.set.as_ref(), state.set_unread, state.label.as_str());
    let own = sets::Own { set, unread, label, peers };
    let span = sets::SetRun::open(own, cards, card_id, &talk, ask, was.set_id);
    let mut span = span.map_err(refused)?;
    let source = settings.source(url_key);
    if let Some(nickname) = &ask.change.nickname {
        source.nickname = nickname.trim().to_string();
    }

    // The selection. A run of the server's peers alone reads none of its
    // own libraries: its files stand as the card's ledger has them, as
    // after a 304, and its selection is left as it is.
    let libraries = match (span.as_ref().and_then(sets::SetRun::libraries), &ask.libraries) {
        (Some(set), _) => set,
        (None, Some(asked)) => asked.clone(),
        (None, None) if !source.libraries.is_empty() => source.libraries.clone(),
        (None, None) => info.user.vpaths.clone(),
    };
    if !ask.peers_only {
        if let Some(unknown) = libraries.iter().find(|l| !info.user.vpaths.contains(l)) {
            let why = format!("{unknown} isn't a library this account sees on the server");
            return Err(refused(why));
        }
        if libraries.is_empty() {
            return Err(refused("no library to sync".into()));
        }
        source.libraries = libraries.clone();
    }
    // What the server converts, when it does: the rates it lists, its bulk
    // queue (an older server's: 64k-192k, none).
    let transcode = info.features.transcode.as_ref();
    let plan_key = settings.plan_key(&libraries, &device, transcode);
    let source = settings.source(url_key).clone();

    // The rows of this server on the card, which a listing reuses.
    let mut rows: Vec<Kept> = match (root.tags.as_ref(), ours) {
        (Ok(tags), Some(i)) => tags
            .records
            .iter()
            .filter(|e| e.ledger.as_ref().is_some_and(|l| l.server as usize == i))
            .filter_map(Kept::of)
            .collect(),
        _ => Vec::new(),
    };
    let known = match ours {
        Some(0) => root.manifest.server_revision.clone(),
        Some(_) => source.revision.clone(),
        None => String::new(),
    };
    let old_fingerprint = root.tags.as_ref().ok().map(|tags| {
        let of = |e: &Entry| FileIn::of(&e.path, &e.rec, &e.run, e.ledger.as_ref());
        fingerprint(&tags.records.iter().filter_map(of).collect::<Vec<_>>())
    });
    let whole = ask.whole
        || known.is_empty()
        || !joined.is_empty()
        || source.plan_key != plan_key
        || now.saturating_sub(source.full_read) >= WHOLE_EVERY_SECS
        || source.runs.saturating_add(1) >= WHOLE_EVERY_RUNS
        || span.as_ref().is_some_and(sets::SetRun::whole);
    // What the card holds, for its set, read while its root is at hand.
    let held = sets::Rows::of(root, space, span.is_some());

    // 3. The server's side.
    let reads = talk.clone();
    let mut origin = Origin {
        talk,
        server: Server::direct(instance, url_key),
        revision: known.clone(),
        ..Origin::default()
    };
    let others = info.user.vpaths.iter().filter(|v| !libraries.contains(v));
    let ignore: Vec<String> = others.cloned().collect();
    let known_revision = (!whole).then_some(known.as_str());
    let answer = match ask.peers_only {
        false => client.manifest(&ignore, MANIFEST_PAGE_MAX, known_revision, &mut |_| {}),
        true => Ok(Manifest::NotModified),
    };
    let answer = answer.map_err(server_halt)?;
    let conversion = settings.conversion;
    // The files the server can't serve are left out of the selection first,
    // whatever reads the rest ([`plan::leave_hidden_out`]).
    let mut hidden = 0;
    let (listed, mut wanted, entries) = match answer {
        Manifest::NotModified => {
            // A row of one (a disk-mode sync's from before the rule) is left
            // out as its entry would be; a run of the peers alone leaves the
            // server's rows as they are.
            if !ask.peers_only {
                hidden = plan::leave_hidden_out(&mut rows, |k| k.ledger.server_path.as_str());
            }
            let entries: Vec<ManifestEntry> = rows.iter().map(entry_of).collect();
            let listed = if ask.peers_only { Listed::Unread } else { Listed::NotModified };
            (listed, wanted_of_ledger(origin, &rows), entries)
        }
        Manifest::Listed(mut listing) => {
            hidden = plan::leave_hidden_out(&mut listing.entries, |e| e.filepath.as_str());
            origin.revision = listing.revision.clone();
            origin.scanning = !listing.deletions_trusted();
            if listing.deletions_trusted() {
                let audio = |e: &ManifestEntry| e.audio_hash.as_deref().and_then(hex16);
                let hashes = listing.entries.iter().filter_map(audio);
                origin.whole = Some(hashes.collect::<HashSet<_>>());
            }
            history.remap(&reads, &listing.entries, Some(&libraries));
            let kept: HashMap<String, Kept> =
                rows.iter().map(|k| (k.ledger.server_path.clone(), k.clone())).collect();
            let same_plan = source.plan_key == plan_key;
            // A card of a set reads its range's files and those it holds,
            // not the other cards' (B7).
            let cut = span.as_mut().and_then(|s| s.cut(&listing.entries, &held));
            let ours = cut.as_deref().unwrap_or(&listing.entries);
            let mut ogg = HashMap::new();
            for e in oggs_to_sniff(ours, &kept, same_plan) {
                if let Ok(read) = client.read_range(&e.filepath, 0, OGG_HEAD) {
                    ogg.insert(e.filepath.clone(), ogg_is_opus(&read.bytes));
                }
            }
            let need = needing_metadata(ours, &kept, same_plan);
            let metadata = match need.is_empty() {
                true => HashMap::new(),
                false => client.metadata_batch(&need).map_err(server_halt)?,
            };
            let wanted = wanted_of(
                origin,
                ours,
                &metadata,
                &ogg,
                &kept,
                same_plan,
                &device,
                &conversion,
                transcode,
            );
            (Listed::Whole, wanted, listing.entries)
        }
    };
    let revision = wanted.card.origins[0].revision.clone();
    let totals = totals(&entries);
    events(Event::Listed);
    // The card's plays for this server, their paths as it has them now.
    history.post(client, &reads, info.features.stats.is_some(), info.instance_id.is_some());
    // Plays to send that no outbox holds would be lost with the rows this
    // run replaces (2.20.6, item 5): no run until one does.
    if let Some(why) = history.unstored() {
        return Err(refused(format!("{why}: the run would replace the rows they map through")));
    }

    // The server's federation peers, each a server of the card's (B9),
    // their files after the server's own.
    let cx = peers::Context {
        client,
        info,
        talk: &reads,
        root,
        device: &device,
        ask,
        fed,
        reached,
        first_origin: 1,
    };
    let (sides, mut peer_summaries) = peers::prepare(&cx, settings, notes);
    // The files left out that no server serves, the peers' with the
    // server's: one note for them all, never a line each.
    let left_out = hidden + peer_summaries.iter().map(|s| s.hidden).sum::<usize>();
    if left_out > 0 {
        notes.push(plan::hidden_note(left_out));
    }
    let mut peer_fetchers = Vec::new();
    let mut shelves: Vec<Covers> = Vec::new();
    let mut lists: Vec<(Arc<Client>, Option<Gate>)> = vec![(client.clone(), None)];
    for side in sides {
        let at = wanted.card.origins.len();
        assert_eq!(side.peer.origin(), at, "a peer's origin follows the others'");
        merge(&mut wanted, side.wanted);
        peer_fetchers.push(side.peer);
        shelves.push(side.covers);
        lists.push((side.list, Some(side.gate)));
    }

    // 4. The plan.
    let http = Http::new(client.clone(), 0, device.max_channels);
    let mut http = http.map_err(|e| refused(e.to_string()))?;
    http.bulk(transcode.and_then(TranscodeInfo::bulk_slots));
    let sources = Sources { own: Some(http), peers: peer_fetchers };
    let folders_here = local::folders(info.admin.as_ref(), reached, || client.admin_directories());
    let mut fetcher = Local::new(folders_here, 0, &entries, sources);
    let source_mode = fetcher.mode().code();
    // A card of a set takes its range (a set made first when asked); one
    // that can't is offered to move its edge or to add a card, and the
    // listener's answer, given, is planned again.
    let dj = settings.auto_dj && !settings.for_someone_else && device.writes_mpdj();
    let short = |fit: &Option<Fit>| fit.is_some_and(|f| !f.fits() || f.over_cap());
    let whole_selection = span.as_ref().map(|_| wanted.clone());
    if let Some(span) = span.as_mut() {
        span.take(&mut wanted, &libraries, &held, space).map_err(refused)?;
    }
    let mut plan = sets::plan(session, &wanted, &ask.answers, &mut fetcher, span.as_ref())?;
    let mut fit = space.map(|s| plan::fit(&s, &plan.footprint(), dj));
    if let (Some(run), Some(selection)) = (span.as_mut(), &whole_selection)
        && short(&fit)
        && run.decide(ask.decide, notes)
    {
        wanted = selection.clone();
        run.take(&mut wanted, &libraries, &held, space).map_err(refused)?;
        plan = sets::plan(session, &wanted, &ask.answers, &mut fetcher, Some(&*run))?;
        fit = space.map(|s| plan::fit(&s, &plan.footprint(), dj));
    }
    if let Some(span) = span.as_ref() {
        span.unused(ask.decide, notes);
    }
    let spread = match span.is_none() && short(&fit) {
        true => sets::spread(&wanted, &held, space),
        false => Vec::new(),
    };
    // A set made, or an edge moved, that the card can't take yet stays on
    // it all the same: its files for other cards leave once those sync.
    if !ask.dry_run
        && short(&fit)
        && let Some(span) = span.as_mut().filter(|s| s.decided())
    {
        span.keep(session, was.state, notes);
    }
    fill_late(client, &plan, &mut wanted);
    peers::planned(&mut peer_summaries, &fetcher.network().peers, &plan, &wanted);
    let order = plan.fetch_order();
    let converted = |i: &usize| matches!(wanted.card.files[*i].bytes, Bytes::Converted(_));
    let (fetches, fetch_bytes) = plan.fetches();
    let (kept, adopted) = plan.kept();
    let summary = Summary {
        listed,
        source: source_mode,
        libraries,
        files: wanted.card.files.len(),
        skipped: wanted.skipped.clone(),
        hidden,
        fetches,
        fetch_bytes,
        conversions: order.iter().filter(|i| converted(i)).count(),
        renames: plan.renames(),
        kept,
        adopted,
        removals: plan.removals().into_iter().map(str::to_string).collect(),
        guards: plan.guards.clone(),
        report: plan.report.clone(),
        fit,
        peers: peer_summaries,
        set: span.as_ref().and_then(sets::SetRun::summary),
        spread,
        offered: info.user.vpaths.clone(),
        totals,
    };
    if !ask.dry_run {
        if let Some(fit) = fit.filter(|f| !f.fits() || f.over_cap()) {
            return Err((Some(summary), Halt::Fit(fit)));
        }
        if !plan.guards.is_empty() {
            return Err((Some(summary), Halt::Card(Stop::Unanswered(plan.guards.clone()))));
        }
    }

    // The sources' conversions (the server's made ahead in the run's order),
    // and the hooks. Each origin has its covers: the server's own, then each
    // peer's.
    let by_key: HashMap<Key, Extra> = wanted
        .card
        .files
        .iter()
        .zip(&wanted.extras)
        .map(|(f, e)| ((f.origin, f.ledger.server_path.clone()), e.clone()))
        .collect();
    let extras = Rc::new(RefCell::new(by_key));
    let images = matches!(fetcher.mode(), local::Mode::Disk(_)).then(|| fetcher.images());
    shelves.insert(0, Covers::new(client.clone(), images));
    let shelves: Rc<Vec<RefCell<Covers>>> =
        Rc::new(shelves.into_iter().map(RefCell::new).collect());
    let roots = plan.roots().to_vec();
    let card_paths: HashMap<Key, String> = plan
        .fetched_paths()
        .map(|(i, path)| {
            let f = &wanted.card.files[i];
            ((f.origin, f.ledger.server_path.clone()), path.to_string())
        })
        .collect();
    let card_paths = Rc::new(card_paths);
    let retag = || Retag {
        shelves: shelves.clone(),
        extras: extras.clone(),
        card_paths: card_paths.clone(),
        roots: roots.clone(),
        covers_on: settings.covers,
    };
    let lengths = |o: usize| -> HashMap<String, u64> {
        let length = |&i: &usize| {
            let file = &wanted.card.files[i];
            Some((file.ledger.server_path.clone(), wanted.extras[i].duration_ms?))
        };
        let of = |i: &&usize| wanted.card.files[**i].origin == o;
        order.iter().filter(|i| converted(i)).filter(of).filter_map(length).collect()
    };
    // The server's own bytes that come over the network, in the run's order:
    // downloaded ahead of the card.
    let media: Vec<&WantedFile> = order
        .iter()
        .map(|&i| &wanted.card.files[i])
        .filter(|f| f.origin == 0 && f.original() && !fetcher.reads(f))
        .collect();
    let network = fetcher.network();
    for peer in network.peers.iter_mut() {
        peer.finish_with(Box::new(retag()));
        peer.lengths(lengths(peer.origin()));
    }
    if let Some(own) = network.own.as_mut() {
        own.finish_with(Box::new(retag()));
        own.lengths(lengths(0));
        let ahead: Vec<&WantedFile> = order
            .iter()
            .filter(|i| converted(i))
            .map(|&i| &wanted.card.files[i])
            .filter(|f| f.origin == 0)
            .collect();
        if !ask.dry_run {
            own.ahead(&ahead);
            own.prefetch(&media, ask.stop.clone());
        }
    }
    let origin_of: HashMap<usize, usize> =
        plan.origin_at().iter().enumerate().map(|(o, &i)| (i, o)).collect();
    let hooks = SyncHooks {
        client: client.clone(),
        lists,
        origin_of,
        shelves,
        extras,
        covers_on: settings.covers,
        max_channels: device.max_channels,
        roots,
        old_art: settings.art.clone(),
        art: BTreeMap::new(),
        dj,
        dj_off: match (settings.auto_dj, settings.for_someone_else, device.writes_mpdj()) {
            (true, true, _) => Some("the card is made for someone else"),
            (true, false, false) => Some("the card's player doesn't read MPDJ v2"),
            _ => None,
        },
        old_fingerprint,
        refresh: listed == Listed::Whole || some_peer_read(&summary.peers),
        store: None,
        history: None,
        notes: Vec::new(),
        looked: false,
        span_cluster: None,
        last: None,
    };
    let hooks = Some(hooks);
    let span = span.as_ref().and_then(sets::SetRun::end);
    Ok(Prepared { plan, fetcher, hooks, summary, listed, revision, ours, url_key, plan_key, span })
}

/// The listed files and bytes of each library, by the vpath their server
/// path starts with, in the order the libraries first come.
fn totals(entries: &[ManifestEntry]) -> Vec<(String, usize, u64)> {
    let mut totals: Vec<(String, usize, u64)> = Vec::new();
    for e in entries {
        let vpath = e.filepath.split('/').next().unwrap_or_default();
        let size = e.file_size.unwrap_or(0);
        match totals.iter_mut().find(|(v, ..)| v == vpath) {
            Some((_, files, bytes)) => {
                *files += 1;
                *bytes += size;
            }
            None => totals.push((vpath.to_string(), 1, size)),
        }
    }
    totals
}

/// The listener's stop is set.
fn stopped(ask: &Ask) -> bool {
    ask.stop.as_ref().is_some_and(|stop| stop.load(Ordering::SeqCst))
}

/// A run's source with the listener's stop in front of it: once the stop
/// is set every file still to fetch waits ([`STOPPED`]), so the run ends
/// at a checkpoint with its plan on the card, the files in flight done.
struct Stopping<'f, F: Fetcher> {
    inner: &'f mut F,
    stop: Option<Arc<AtomicBool>>,
}

impl<F: Fetcher> Fetcher for Stopping<'_, F> {
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
        self.inner.fetch(file, into)
    }

    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed> {
        self.inner.read_range(file, start, len)
    }

    fn resumable(&self, file: &WantedFile) -> bool {
        self.inner.resumable(file)
    }

    fn fetch_part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn super::engine::Part,
    ) -> Result<Fetched, Failed> {
        self.inner.fetch_part(file, have, into)
    }

    fn waits(&self, file: &WantedFile) -> Option<String> {
        match self.stop.as_ref().is_some_and(|stop| stop.load(Ordering::SeqCst)) {
            true => Some(STOPPED.to_string()),
            false => self.inner.waits(file),
        }
    }
}

/// A ledger row as a manifest entry, for the local-disk source's check of
/// its folders after a 304.
fn entry_of(k: &Kept) -> ManifestEntry {
    let l = &k.ledger;
    ManifestEntry {
        filepath: l.server_path.clone(),
        id: l.mstream_id as i64,
        metadata: Metadata::default(),
        file_size: Some(l.server_size),
        modified: i64::try_from(l.server_modified).ok(),
        hash: None,
        audio_hash: None,
        hash_v: None,
        format: None,
        album_id: None,
        artist_id: None,
        created_at: None,
    }
}

/// The full metadata of the conversions the run fetches that the plan was
/// made without (a row reused, a 304): `bpm-source` says which BPM and key
/// are the file's own, to put back. A server that can't answer leaves them
/// as they are: the retag puts back the cover alone.
fn fill_late(client: &Client, plan: &Plan, wanted: &mut Wanted) {
    let lacks = |&i: &usize| {
        let file = &wanted.card.files[i];
        file.origin == 0 && matches!(file.bytes, Bytes::Converted(_)) && !wanted.extras[i].full
    };
    let lacking: Vec<usize> = plan.fetch_order().into_iter().filter(lacks).collect();
    if lacking.is_empty() {
        return;
    }
    let path = |&i: &usize| wanted.card.files[i].ledger.server_path.clone();
    let paths: Vec<String> = lacking.iter().map(path).collect();
    let Ok(found) = client.metadata_batch(&paths) else { return };
    for i in lacking {
        if let Some(Some(m)) = found.get(&wanted.card.files[i].ledger.server_path) {
            wanted.extras[i].fill(m);
        }
    }
}

/// `other`'s wanted card after `into`'s: its origins numbered on from
/// `into`'s.
fn merge(into: &mut Wanted, other: Wanted) {
    let at = into.card.origins.len();
    into.card.origins.extend(other.card.origins);
    let files = other.card.files.into_iter().map(|f| WantedFile { origin: f.origin + at, ..f });
    into.card.files.extend(files);
    into.extras.extend(other.extras);
    into.skipped.extend(other.skipped);
}

/// Some peer is read, whole as a peer always is: the run's Auto DJ vectors
/// are read again.
fn some_peer_read(peers: &[PeerSummary]) -> bool {
    peers.iter().any(|p| p.status == PeerStatus::Read)
}

// ── What the server's side adds to a run ────────────────────────────────────

/// What a commit's Auto DJ index is built from, as a key: equal keys build
/// equal indexes from equal embeddings.
fn fingerprint(files: &[FileIn]) -> u64 {
    let mut lines: Vec<String> = files
        .iter()
        .map(|f| {
            let fields = [
                f.path.clone(),
                hex(&f.hash),
                f.title.clone(),
                f.artists.join("\u{1e}"),
                format!("{} {} {}", f.bpm10, f.camelot, f.analysed),
            ];
            fields.join("\u{1f}")
        })
        .collect();
    lines.sort();
    fnv1a64(lines.join("\n").as_bytes())
}

/// A converted file's retag (A4's writer): the album's cover as a baseline
/// JPEG front cover, and the source's own BPM and key where its tags had
/// them (`bpm-source` `tag`); mStream's analysis goes only to the card's
/// records (BPM_ANALYSED). Done on this computer's copy, before the card's
/// one write of the file.
struct Retag {
    /// Each origin's covers.
    shelves: Rc<Vec<RefCell<Covers>>>,
    extras: Rc<RefCell<HashMap<Key, Extra>>>,
    /// Each fetched file's card path: its album folder.
    card_paths: Rc<HashMap<Key, String>>,
    roots: Vec<String>,
    covers_on: bool,
}

impl Finish for Retag {
    fn finish(&mut self, file: &WantedFile, path: &Path) -> Result<(), String> {
        let server = &file.ledger.server_path;
        let key = (file.origin, server.clone());
        let extra = self.extras.borrow().get(&key).cloned().unwrap_or_default();
        let shelf = self.shelves.get(file.origin).filter(|_| self.covers_on);
        let cover = match shelf {
            Some(shelf) => {
                let folders = match self.card_paths.get(&key) {
                    Some(card) => folders(card, server, &self.roots),
                    None => vec![split_last(server).0.to_string()],
                };
                let image = shelf.borrow_mut().image(&folders, extra.album_art.as_deref());
                image.as_deref().and_then(covers::baseline_jpeg)
            }
            None => None,
        };
        if cover.is_none() && extra.bpm_tag.is_none() && extra.key_tag.is_none() {
            return Ok(());
        }
        let (bpm, key) = (extra.bpm_tag.as_deref(), extra.key_tag.as_deref());
        tags::restore_path(path, &Restore { cover: cover.as_deref(), bpm, key })
    }
}

/// The engine's hooks for a sync from the user's own server, and its peers.
struct SyncHooks {
    client: Arc<Client>,
    /// Each origin's client for its listing's calls (the server's own, a
    /// peer's browse proxy), and a peer's gate.
    lists: Vec<(Arc<Client>, Option<Gate>)>,
    /// Which origin each of the card's servers is, in the run's numbering.
    origin_of: HashMap<usize, usize>,
    /// Each origin's covers.
    shelves: Rc<Vec<RefCell<Covers>>>,
    extras: Rc<RefCell<HashMap<Key, Extra>>>,
    covers_on: bool,
    max_channels: u32,
    roots: Vec<String>,
    /// Each album folder's art key as its thumbnail was made from it, as
    /// the card's settings say, and as this run leaves them.
    old_art: BTreeMap<u64, u64>,
    art: BTreeMap<u64, u64>,
    /// An Auto DJ index is wanted: on, the card's player reads MPDJ v2, and
    /// not for someone else.
    dj: bool,
    dj_off: Option<&'static str>,
    /// The key of the files the root's index was built from.
    old_fingerprint: Option<u64>,
    /// A whole read: the embeddings are read again even when the files
    /// are the same, the server's analysis having moved on.
    refresh: bool,
    /// The settings the run's end stores; set for a run, not for a plan.
    store: Option<Store>,
    /// The card's plays: their marks go into the commits' ACKS and the
    /// settings stored. Set for a run.
    history: Option<History>,
    notes: Vec<String>,
    /// The thumbnail step ran: the art it left is what is stored.
    looked: bool,
    /// A card of a set: the final commit's ledger is read for its end, its
    /// files' loads by this cluster.
    span_cluster: Option<u32>,
    last: Option<sets::Final>,
}

impl SyncHooks {
    /// A row's file by its origin: its key, when its server is one the run
    /// reads.
    fn key_of(&self, l: &Ledger) -> Option<Key> {
        let origin = *self.origin_of.get(&(l.server as usize))?;
        Some((origin, l.server_path.clone()))
    }

    /// The album's `album-art` names as this run's listing gives them; None
    /// when it gave none of its files (a 304, another server's rows).
    fn listed_art(&self, album: &Album<'_>) -> Option<Vec<String>> {
        let extras = self.extras.borrow();
        let listed = |l: &Ledger| extras.get(&self.key_of(l)?).filter(|e| e.listed);
        if !album.files.iter().any(|(_, l)| listed(l).is_some()) {
            return None;
        }
        Some(album.files.iter().filter_map(|(_, l)| listed(l)?.album_art.clone()).collect())
    }

    /// The album's `album-art` names as the server's metadata gives them
    /// now, for its files of the server's own (origin 0); None when it has
    /// none of them, or the server doesn't answer.
    fn art_now(&self, album: &Album<'_>) -> Option<Vec<String>> {
        let extras = self.extras.borrow();
        let keys = album.files.iter().filter_map(|(_, l)| self.key_of(l));
        let keys = keys.filter(|k| k.0 == 0 && extras.contains_key(k));
        let paths: Vec<String> = keys.map(|(_, p)| p).collect();
        drop(extras);
        if paths.is_empty() {
            return None;
        }
        let found = self.client.metadata_batch(&paths).ok()?;
        let art = |p: &String| found.get(p)?.as_ref()?.album_art.clone().filter(|a| !a.is_empty());
        Some(paths.iter().filter_map(art).collect())
    }

    /// A peer of the run whose day is spent, or that refused: nothing more
    /// is asked of it.
    fn held(&self, origin: usize) -> bool {
        self.lists.get(origin).and_then(|(_, gate)| gate.as_ref()).is_some_and(|g| !g.open())
    }
}

impl Hooks for SyncHooks {
    fn conversion(
        &mut self,
        file: &WantedFile,
        fetched: &Fetched,
        card: &dyn CardFs,
        stage: &str,
    ) -> Result<(), String> {
        let Bytes::Converted(container) = file.bytes else { return Ok(()) };
        let codec = codec_of(container).ok_or("not a conversion a sync makes")?;
        let got = fetched.content_type.as_deref();
        if !got.is_some_and(|t| codec.answers(t)) {
            let got = got.unwrap_or("no type");
            let asked = codec.content_type();
            return Err(format!("the server sent {got} where {asked} was asked for"));
        }
        let size = card.stat(stage).map_err(|e| e.to_string())?.map_or(0, |i| i.size);
        let heard = length::heard(codec, &CardAt { card, path: stage, size })?;
        let extras = self.extras.borrow();
        let key = (file.origin, file.ledger.server_path.clone());
        let expected = extras.get(&key).and_then(|e| e.duration_ms);
        length::check(&heard, expected, self.max_channels)
    }

    fn cover(&mut self, album: &Album<'_>) -> Cover {
        if !self.covers_on {
            return Cover::Gone;
        }
        self.looked = true;
        let hash = path_hash(album.folder);
        let old = self.old_art.get(&hash).copied();
        // The album's origin is its first file's; a peer whose day is spent
        // isn't asked, and its album stays as it was.
        let first = album.files.iter().min_by(|a, b| compare_file_paths(a.0, b.0));
        let origin = first.and_then(|(_, l)| self.key_of(l)).map(|(o, _)| o);
        if origin.is_some_and(|o| self.held(o)) {
            if let Some(key) = old {
                self.art.insert(hash, key);
            }
            return Cover::Keep;
        }
        // The art the album's files name: as this run's listing says. Files
        // whose art it didn't read (a 304, another server's rows) stay as
        // they were, unless a thumbnail made before is lost: then as their
        // metadata says now (step 10 rewrites any that isn't there).
        let lost = !album.on_card && old.is_some_and(|key| key != NO_ART);
        let names = match self.listed_art(album) {
            Some(names) => names,
            None => match lost.then(|| self.art_now(album)).flatten() {
                Some(names) => names,
                None => {
                    if let Some(key) = old {
                        self.art.insert(hash, key);
                    }
                    return Cover::Keep;
                }
            },
        };
        let key = art_key(names.iter().map(String::as_str));
        // Made before from this art, or from the folder's image while no art
        // is named, and there: kept.
        if let Some(was) = old.filter(|&was| was == key || (key, was) == (NO_ART, FOLDER_IMAGE))
            && album.on_card
        {
            self.art.insert(hash, was);
            return Cover::Keep;
        }
        // Looked at before with no art named and no folder image found: the
        // device's own cover order (its files' pictures), not probed again.
        if (key, old) == (NO_ART, Some(NO_ART)) {
            self.art.insert(hash, NO_ART);
            return Cover::Gone;
        }
        let Some(&(card_path, ledger)) = first else { return Cover::Gone };
        let Some(shelf) = origin.and_then(|o| self.shelves.get(o)) else { return Cover::Keep };
        let folders = folders(card_path, &ledger.server_path, &self.roots);
        let art = most_common(names.iter().map(String::as_str)).map(str::to_string);
        let image = shelf.borrow_mut().image(&folders, art.as_deref());
        match image.as_deref().and_then(covers::thumbnail) {
            Some((pixels, source_bytes)) => {
                // With no art named, the image is the folder's.
                self.art.insert(hash, if key == NO_ART { FOLDER_IMAGE } else { key });
                Cover::New { pixels, source_bytes }
            }
            None if key == NO_ART => {
                self.art.insert(hash, NO_ART);
                Cover::Gone
            }
            // mStream names art that didn't come: the one there stays, and
            // the next run looks again.
            None if album.on_card => Cover::Keep,
            None => Cover::Gone,
        }
    }

    fn autodj(&mut self, commit: &Commit<'_>) -> AutoDj {
        if let Some(cluster) = self.span_cluster {
            self.last = Some(sets::Final::of(commit, cluster));
        }
        if !self.dj {
            if let Some(why) = self.dj_off.take() {
                self.notes.push(format!("no Auto DJ index: {why}"));
            }
            return AutoDj::None;
        }
        let of = |r: &RecordIn| FileIn::of(&r.path, &r.rec, &r.run, r.ledger.as_ref());
        let files: Vec<FileIn> = commit.records.iter().filter_map(of).collect();
        let same_files = self.old_fingerprint == Some(fingerprint(&files));
        if commit.old.is_some() && !self.refresh && same_files {
            return AutoDj::Same;
        }
        // Each origin's rows' vectors from its own server (a peer's through
        // the browse proxy, its quota counting them), all of one model: the
        // server's own, else the first peer's. A peer that is held is asked
        // nothing; its rows have no neighbours until it is.
        let mut model: Option<Model> = None;
        let mut vectors = HashMap::new();
        let mut why_none: Option<String> = None;
        for (o, server) in commit.origins.iter().enumerate() {
            let Some(server) = *server else { continue };
            let Some((client, gate)) = self.lists.get(o).cloned() else { continue };
            if gate.as_ref().is_some_and(|g| !g.open()) {
                continue;
            }
            let mut hash_of: HashMap<&str, [u8; 16]> = HashMap::new();
            for r in commit.records {
                let Some(l) = r.ledger.as_ref().filter(|l| l.server == server) else { continue };
                if let Some(h) = l.canonical_hash().filter(|_| !l.server_path.is_empty()) {
                    hash_of.insert(&l.server_path, h);
                }
            }
            if hash_of.is_empty() {
                continue;
            }
            let whose = if o == 0 { "the server".to_string() } else { format!("peer origin {o}") };
            let paths: Vec<String> = hash_of.keys().map(|p| p.to_string()).collect();
            let found = match client.embeddings(&paths) {
                Ok(Some(found)) => found,
                Ok(None) => {
                    why_none.get_or_insert(format!("{whose}'s discovery is off"));
                    continue;
                }
                Err(e) => {
                    why_none.get_or_insert(format!("{whose}'s embeddings didn't come: {e}"));
                    continue;
                }
            };
            let id = found.model.id.as_deref().unwrap_or("");
            let version = found.model.version.as_deref().unwrap_or("");
            let Some(theirs) = Model::known(id, version) else {
                let why = format!("the model {id} {version} isn't one the index lists");
                why_none.get_or_insert(why);
                continue;
            };
            match &model {
                Some(was) if *was != theirs => {
                    let note = format!("{whose}'s rows have no Auto DJ neighbours: another model");
                    self.notes.push(note);
                    continue;
                }
                Some(_) => {}
                None => model = Some(theirs),
            }
            // By the path asked for, else by the hash it came keyed by, when
            // that is a file's of the commit.
            let hashes: HashSet<[u8; 16]> = hash_of.values().copied().collect();
            for t in found.tracks {
                let by_hash = t.audio_hash.as_deref().and_then(hex16).filter(|h| hashes.contains(h));
                let hash = hash_of.get(t.file_path.as_str()).copied().or(by_hash);
                if let (Some(v), Some(h)) = (t.vector, hash) {
                    vectors.insert(h, v);
                }
            }
        }
        let Some(model) = model else {
            let why = why_none.unwrap_or_else(|| "no file on the card has an embedding yet".into());
            self.notes.push(format!("no Auto DJ index: {why}"));
            return AutoDj::None;
        };
        let input = IndexIn {
            generation: commit.generation,
            card_id: commit.card_id,
            built_time: commit.now,
            tags_generation: commit.generation,
            model,
            selection: Selection::DEFAULT,
            files,
            vectors,
        };
        match autodj::index(commit.device, &input, commit.old, &Work::default()) {
            Ok(Index::New { file, .. }) => AutoDj::New(file),
            Ok(Index::Same) => AutoDj::Same,
            Ok(Index::None(why)) => {
                let why = match why {
                    NoIndex::NotRead => "the card's player doesn't read it".to_string(),
                    NoIndex::NoRows => "no file on the card has an embedding yet".to_string(),
                    NoIndex::TooManyRows(n) => format!("{n} rows, past the 65,535 an index holds"),
                };
                self.notes.push(format!("no Auto DJ index: {why}"));
                AutoDj::None
            }
            Err(e) => {
                self.notes.push(format!("no Auto DJ index: {e}"));
                AutoDj::None
            }
        }
    }

    /// The plays the run settled, acknowledged in each commit it makes.
    fn acks(&mut self, carried: &[Ack]) -> Vec<Ack> {
        match &self.history {
            Some(history) => history.acks(carried),
            None => carried.to_vec(),
        }
    }

    /// The settings as asked, and what a run that ended learned: the
    /// manifest's schedule, which art each thumbnail was made from, and the
    /// plays' posted marks. A parked run's thumbnails weren't looked at:
    /// their art stays.
    fn state(&mut self, outcome: &Outcome, seq: u64) -> Option<Vec<u8>> {
        let store = self.store.as_mut()?;
        if let Some(history) = &self.history {
            store.state.plays = history.marks();
        }
        let settings = &mut store.state.settings;
        if outcome.parked.is_none() && store.listed != Listed::Unread {
            let source = settings.source(store.url_key);
            if store.listed == Listed::Whole {
                (source.full_read, source.runs, source.plan_key) = (store.now, 0, store.plan_key);
                if let Some(revision) = &store.revision {
                    source.revision = revision.clone();
                }
            } else {
                source.runs = source.runs.saturating_add(1);
            }
        }
        // The art each thumbnail was made from, once the thumbnail step ran:
        // at the run's end, or before a source's wait parked it.
        if outcome.parked.is_none() || self.looked {
            settings.art = std::mem::take(&mut self.art);
        }
        // What the card now says of itself in its set.
        if let Some(end) = store.span.as_mut() {
            store.state.set = Some(end.settle(outcome, self.last.as_ref(), store.listed));
        }
        Some(store.state.text(seq))
    }
}

// ── The hidden command ──────────────────────────────────────────────────────

/// Whether the client reaches its server on this computer, directly: a
/// loopback host (the GUI's `same_machine` rule). The command line has no
/// tunnel bridge, which only the player dials.
fn reached(server: &str) -> Reached {
    let Ok(url) = url::Url::parse(server) else { return Reached::Elsewhere };
    let loopback = match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if loopback { Reached::Loopback } else { Reached::Elsewhere }
}

/// `device card-sync <volume>`, undocumented: syncs the card at `volume`
/// (where it is mounted, or the OS's name for it), which must be one the
/// volume layer accepts — a refused volume, or one that isn't a card at
/// all, is never opened — from the server of the terminal's saved session.
/// Prints the plan and what came of it as JSON; the engine's events go to
/// stderr as they come, and while it copies, a line of its rate every 30 s
/// ([`Rate`]). With `ask.dry_run`, the plan only: nothing is
/// written. For the real-card checks: the listener's `device card <drive>
/// sync` waits for its UX contract. Exit 0: done and verified, or a dry run;
/// 1 when the sync stopped (a guard to answer, a fit, the server, the
/// card); 2 when the volume isn't one it may write, the volumes can't be
/// listed, or there is no saved session.
pub(crate) fn command(volume: &str, ask: &Ask, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let found = match volume::find() {
        Ok(found) => found,
        Err(e) => {
            let _ = writeln!(err, "mstream-player: the volumes can't be listed: {e}");
            return 2;
        }
    };
    let mut saved = || Client::resolve(None, None).map_err(|e| e.to_string());
    on(&found, volume, ask, &mut saved, out, err)
}

/// [`command`] on the volumes found; `saved` the saved session's client,
/// asked for only once the volume is one it may write.
fn on(
    found: &[volume::Found],
    volume: &str,
    ask: &Ask,
    saved: &mut dyn FnMut() -> Result<Client, String>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let cards = found.iter().filter(|f| f.accepted()).map(|f| f.volume.path.clone()).collect();
    let Some(found) = super::setup::pick(found, volume) else {
        let _ = writeln!(err, "mstream-player: {volume} isn't a volume this computer lists");
        return 2;
    };
    let Some(card) = Mounted::of(found) else {
        let codes: Vec<&str> = found.refusals.iter().map(volume::Refusal::code).collect();
        let _ = writeln!(err, "mstream-player: {volume} is refused: {}", codes.join(", "));
        return 2;
    };
    let client = match saved() {
        Ok(client) => client,
        Err(e) => {
            let _ = writeln!(err, "mstream-player: no saved session: {e}");
            return 2;
        }
    };
    let reached = reached(&client.server());
    let fed = match Federation::system(cards) {
        Ok(fed) => fed,
        Err(e) => {
            let _ = writeln!(err, "mstream-player: {e}");
            return 2;
        }
    };
    let mut cards = Cards::load();
    let synced = {
        let mut rate = Rate::default();
        let mut say = |e: Event| {
            if let Some(line) = event_line(&e) {
                let _ = writeln!(err, "{line}");
            }
            if let Some(line) = rate.heard(&e, Instant::now()) {
                let _ = writeln!(err, "{line}");
            }
        };
        let (space, client) = (found.volume.space, Arc::new(client));
        let plays: Rc<dyn history::Store> = Rc::new(history::ConfigStore);
        let envs = &mut Env::system;
        sync(&card, space, client, reached, ask, &mut cards, plays, &fed, envs, &mut say)
    };
    let printed = serde_json::to_string_pretty(&synced_json(&synced, ask.dry_run));
    let _ = writeln!(out, "{}", printed.unwrap_or_default());
    let kept = synced.summary.as_ref().and_then(|s| s.set.as_ref()).is_some_and(|s| s.written);
    if (matches!(synced.end, End::Ran(_)) || kept)
        && let Err(e) = cards.save()
    {
        let _ = writeln!(err, "mstream-player: {e}");
    }
    match &synced.end {
        End::Dry => 0,
        End::Ran(outcome) if outcome.verified.is_ok() && outcome.parked.is_none() => 0,
        _ => 1,
    }
}

/// An engine event as a line on stderr; none for a download's progress.
fn event_line(e: &Event) -> Option<String> {
    Some(match e {
        Event::TookOver(note) => format!("took over a stale run.lock: {}", note.trim()),
        Event::ForeignPlan(id) => format!("deleted another card's plan ({})", id_hex(*id)),
        Event::Deleted(path) => format!("deleted {path}"),
        Event::Progress { .. } | Event::Listed => return None,
        Event::Planned { files, bytes } => format!("planned: {files} files ({bytes} bytes)"),
        Event::Wrote { path, bytes } => format!("wrote {path} ({bytes} bytes)"),
        Event::Renamed { from, to } => format!("renamed {from} to {to}"),
        Event::Failed { server_path, why } => format!("skipped {server_path}: {why}"),
        Event::Committed { generation, last } => {
            format!("committed generation {generation}{}", if *last { " (final)" } else { "" })
        }
    })
}

/// How often [`Rate`] says where the copying stands.
const RATE_EVERY: Duration = Duration::from_secs(30);

/// The hidden command's line of the copying's rate, for watching a long
/// run: the files and bytes written of the plan's, the rate over the last
/// stretch and since the copying began, and the time left at the recent
/// rate. One every [`RATE_EVERY`], and one when the last file is in.
#[derive(Default)]
struct Rate {
    files: usize,
    bytes: u64,
    done_files: usize,
    done_bytes: u64,
    /// When the copying began, and the last line: when, and the bytes then.
    began: Option<Instant>,
    said: Option<(Instant, u64)>,
}

impl Rate {
    fn heard(&mut self, e: &Event, now: Instant) -> Option<String> {
        match e {
            Event::Planned { files, bytes } => {
                (self.files, self.bytes) = (*files, *bytes);
                self.began = Some(now);
                self.said = Some((now, 0));
                return None;
            }
            Event::Wrote { bytes, .. } => {
                self.done_files += 1;
                self.done_bytes += bytes;
            }
            Event::Failed { .. } => self.done_files += 1,
            _ => return None,
        }
        let (began, (then, then_bytes)) = (self.began?, self.said?);
        let last = self.done_files >= self.files;
        if now.duration_since(then) < RATE_EVERY && !last {
            return None;
        }
        self.said = Some((now, self.done_bytes));
        let mbs = |bytes: u64, since: Instant| {
            bytes as f64 / 1e6 / now.duration_since(since).as_secs_f64().max(0.001)
        };
        let recent = mbs(self.done_bytes - then_bytes, then);
        let overall = mbs(self.done_bytes, began);
        let left = self.bytes.saturating_sub(self.done_bytes) as f64 / 1e6;
        let to_go = match (left / recent.max(0.001)) as u64 {
            _ if last => String::new(),
            secs if secs < 60 => "; under a minute to go".to_string(),
            secs if secs < 3600 => format!("; about {} min to go", secs / 60),
            secs => format!("; about {} h {} min to go", secs / 3600, secs % 3600 / 60),
        };
        let (done, all) = (self.done_bytes as f64 / 1e9, self.bytes as f64 / 1e9);
        let stretch = now.duration_since(then).as_secs();
        Some(format!(
            "rate: {} of {} files, {done:.1} of {all:.1} GB; {recent:.1} MB/s the last {stretch} s, \
             {overall:.1} MB/s in all{to_go}",
            self.done_files, self.files,
        ))
    }
}

fn guard_json(g: &Guard) -> Value {
    match g {
        Guard::Newer(files) => json!({ "guard": "newer", "files": files }),
        Guard::DiskCheck(pairs) => json!({ "guard": "disk-check", "pairs": pairs }),
        Guard::Adopt(paths) => json!({ "guard": "adopt", "paths": paths }),
        Guard::Removals { paths, strong } => {
            json!({ "guard": "removals", "strong": strong, "paths": paths })
        }
        Guard::Server { origin, rows } => {
            json!({ "guard": "server", "origin": origin, "rows": rows })
        }
    }
}

fn report_json(r: &Report) -> Value {
    let refused = |s: &names::Skipped| json!({ "path": s.server_path, "why": format!("{:?}", s.why) });
    let refused: Vec<Value> = r.refused.iter().map(refused).collect();
    let failed: Vec<Value> = r.failed.iter().map(|(p, why)| json!({ "path": p, "why": why })).collect();
    json!({
        "edited": r.edited, "gone": r.gone, "unknown": r.unknown, "kept": r.kept,
        "refused": refused, "bigFolders": r.big_folders, "failed": failed, "spared": r.spared,
    })
}

fn synced_json(s: &Synced, dry_run: bool) -> Value {
    let summary = s.summary.as_ref().map(|p| {
        let fit = p.fit.map(|f| {
            let (needed, room, tracks) = (f.needed, f.room, f.tracks);
            json!({ "needed": needed, "room": room, "tracks": tracks, "fits": f.fits(),
                    "overCap": f.over_cap() })
        });
        let skip = |(path, why): &(String, Skip)| json!({ "path": path, "why": why.code() });
        let skipped: Vec<Value> = p.skipped.iter().map(skip).collect();
        json!({
            "listing": p.listed.code(),
            "source": p.source,
            "libraries": p.libraries,
            "files": p.files,
            "skipped": skipped,
            "hidden": p.hidden,
            "fetch": { "files": p.fetches, "bytes": p.fetch_bytes, "conversions": p.conversions },
            "renames": p.renames,
            "kept": p.kept,
            "adopted": p.adopted,
            "removals": p.removals,
            "guards": p.guards.iter().map(guard_json).collect::<Vec<_>>(),
            "report": report_json(&p.report),
            "fit": fit,
            "peers": p.peers.iter().map(PeerSummary::json).collect::<Vec<_>>(),
            "set": p.set.as_ref().map(SetSummary::json),
            "spread": sets::parts_json(&p.spread),
        })
    });
    let (outcome, stopped) = match &s.end {
        End::Dry => (Value::Null, Value::Null),
        End::Halted(halt) => (Value::Null, halt.to_string().into()),
        End::Ran(o) => {
            let verified = match &o.verified {
                Ok(()) => Value::Bool(true),
                Err(why) => why.clone().into(),
            };
            let outcome = json!({
                "generation": o.generation, "cardId": id_hex(o.card_id),
                "commitId": id_hex(o.commit_id), "records": o.records, "written": o.written,
                "renamed": o.renamed, "deleted": o.deleted, "parked": o.parked,
                "verified": verified, "report": report_json(&o.report),
            });
            (outcome, Value::Null)
        }
    };
    json!({
        "dryRun": dry_run, "plan": summary, "outcome": outcome, "stopped": stopped,
        "notes": s.notes, "plays": s.plays.as_ref().map(Tally::json),
        "peers": s.peers.iter().map(PeerSummary::json).collect::<Vec<_>>(), "resumeAt": s.resume_at,
    })
}

#[cfg(test)]
pub(crate) mod tests;
